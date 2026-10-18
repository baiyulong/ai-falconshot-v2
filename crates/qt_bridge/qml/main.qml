import QtQuick
import dev.falconshot 1.0

// The app's root. It has no scene of its own on purpose: what this process shows
// is either a pin window or a capture mask, both of which are full citizens of the
// window list and neither of which wants a parent. §5.1's "no main window, a tray
// icon" ends up looking exactly like this - a headful engine whose root object is
// invisible.
Item {
    id: app

    property PinShim shim: PinShim {}
    property Session session: Session {}

    /// The mask windows, indexed by Qt screen index. Kept by the two signals rather
    /// than read back: this Qt's `Instantiator` has `count`, `object` (singular) and
    /// `delegateModelAccess`, and no `objects` list - measured against 6.10.1's own
    /// `qml/QtQml/Models/plugins.qmltypes`, because a wrong property name inside a
    /// function body is a JS `TypeError` that on Windows never reaches stdout (§8-R14).
    property var maskWindows: []

    /// The live pin windows, keyed by the pin id each one shows - keyed rather than
    /// listed because the only thing that identifies a pin's window across a change of
    /// the set is the id: the pin set is bottom-first (`state::ids()`), so closing one
    /// pin moves the slot index of every pin after it.
    property var pinWindows: ({})

    /// §5.5.11 步骤 3/4 的那一声"有事发生了"。**绑值而不写 `Connections`**，是 P119 两封信
    /// 教的形状：cxx-qt 不给 qproperty 做驼峰化，`#[qproperty(i32, dialog_epoch)]` 的 notify
    /// 就叫 `dialog_epochChanged`，而 `onDialogEpochChanged` 场上没有那一枚信号 —— 名字写错
    /// 是**静默**的（#316）；把一枚大小写混着的 `onDialog_epochChanged` 留在产品码里，下一次
    /// 手写仍旧会错。所以这里声明本文件自己的属性，它的 notify 由 Qt 生成为
    /// `dialogTickChanged`，与 `HintWindow.qml` 那三枚已被运行期读过的 `onShownChanged` /
    /// `onArmedChanged` / `onCountingChanged` 同一形状；而 handler 长在**声明它的那个对象**的
    /// 根上，不在 `Connections` 里 —— 归属与姓名是两件独立的事（#321）。
    readonly property int dialogTick: app.session.dialog_epoch
    onDialogTickChanged: app.redrawPins()

    Component.onCompleted: {
        // First, because it is the one thing here that another thread is waiting for:
        // the hot key pump can only reach this object through the handle cxx-qt gives
        // from inside it, and `main` installs the pump right after `engine.load()`
        // returns - which is when this block runs. Handing it over later would be a
        // key press with nowhere to go.
        app.session.handThread()
        // §5.19.1 步骤 2：把「这一眼看上去是深色还是浅色」落到 `Session.theme` 那一格上。
        // 排在 `syncPins` 与 `refreshMasks` 之前，为的是这两条**造窗**的腿第一次建出来的窗，
        // 其第一帧就已经是它最终的那套餐具，而不是先亮一帧浅色再被 notify 改成深色。
        // **这一句不覆盖 `HintWindow`**：它不是这两条腿造的，而是根 `Item` 里直接声明的一枚
        // 对象（第 171 行），组件构造期就存在、比本块任何一行都早 ⇒ 它的第一帧无从被这里的
        // 次序决定。它靠的是另一件事：`HintWindow.qml` 那套餐具绑的是 `hint.session.theme`
        // 这枚值属性的**绑定**，notify 一到就重新求值 —— 于是贴图/遮罩那一族靠次序、提示窗
        // 那一族靠绑定，两族各有各的形状，不是同一条担保。
        app.session.refreshTheme()
        app.session.markLoaded()
        app.session.refresh()
        // The pins a restored session came back with get their windows here rather
        // than waiting on the notify `refresh` just sent: the legs that measure this
        // process start their one wait off a window's own completion, and the earlier
        // the first frame, the less of that wait is spent on nothing.
        app.syncPins()
        app.session.refreshMasks()
        app.session.refreshWarm()
        // §8.5's watch, last: the screen set and the geometry of every screen in it
        // are only worth listening to once there is a session to answer and windows
        // to reload. What it connected to is counted in C++ and printed by `--probe`,
        // because proving the wiring is the whole of what a run on one machine can
        // do - the half that unplugs a monitor stays out of reach.
        app.shim.watchScreens()
    }

    // The `--mask` gauge's second and later rounds, which is the only way this
    // process can measure what a hot key costs: by then Qt, the engine, the compiled
    // document and one presented frame are all paid for, and none of that is a
    // per-capture cost. Rust owns the arithmetic and the count; this Timer exists
    // because nothing else here can act after the event loop starts.
    //
    // `running` binds to a qproperty rather than calling an invokable in a loop:
    // `warm_left` notifies as it counts down, so the timer stops itself, and a
    // handler that re-armed it would run rounds the plan never asked for.
    //
    // The two names are not spelled the same way, and this is measured rather than
    // assumed: cxx-qt keeps a qproperty's Rust identifier (`warm_left`,
    // `warm_step`, `mask_count` - read out of the generated moc table) and camelCases
    // an invokable (`refreshWarm`, `warmRound`). An undefined property inside a
    // binding is only a warning and leaves the binding at 0, which is why the first
    // run of this timer loaded the document and then did nothing at all.
    Timer {
        id: warmRounds
        interval: app.session.warm_step
        repeat: true
        running: app.session.warm_left > 0
        // `onTriggered`, the signal handler - `triggered` is not a property, and
        // assigning to a non-existent one is a document error that loses the whole
        // file, not just this timer (§8-R14).
        onTriggered: {
            app.session.warmRound()
            // Same screens, so no window was rebuilt - they have to be told the
            // frame key changed, or they keep showing the round before this one.
            app.refreshOtherMasks(null)
        }
    }

    // §5.2.4 延时截图的那枚倒计时，和上面的 `warmRounds` 同一形状、同一条理由：事件环跑起来
    // 之后只有 QML 的 `Timer` 能做事。区别在谁等谁 —— 这枚定时器等的是一次已经按下的开始，
    // 而它触发的那次调用里才真的冻画面。于是「画面冻在倒计时结束那一刻」不是这句注释的说法，
    // 是 `interval: delay_step` 与 `running: delay_left > 0` 这两行本身：第一枚数落地之前，
    // Rust 那边没有任何一格叫过 `capture::freeze()`。
    //
    // `running` 依旧绑值不绑调用（同 `warmRounds` 那条教训）：数完最后一枚时 Rust 把
    // `delay_left` 设成 0，这枚定时器就此自己停，不需要谁去 `stop()`，也不会多冻一张画面。
    Timer {
        id: countdown
        interval: app.session.delay_step
        repeat: true
        running: app.session.delay_left > 0
        onTriggered: app.session.delayRound()
    }

    // One window per pin, made and retired by `syncPins` below rather than by a model.
    //
    // `Instantiator` was the first shape and it is measured out: `model` given a new
    // `int` is a property write, not a row change, and this Qt answers it by destroying
    // every object and building the survivors again. `--pin`'s close leg watched a pile
    // of three lose one and got `removed 0`, `removed 1`, `removed 2`, `added 0`,
    // `added 1` - with brand new native windows for the two pins that had not moved
    // (P55, `.qoder/scratch/p55_pin_diag4.log`). The leg then died inside `closePin`
    // with `gaugeSay ... is not a function`, because the window speaking that line was
    // one of the ones being destroyed, and the fresh slot-0 window - which reads
    // `pinId 0`, the leg's own subject - started the whole run again from `menu`.
    //
    // A `Component` creates the same `PinWindow` on the same completion path
    // (`shim.install`, then `assign`), and the id-keyed registry below is what makes the
    // surviving windows keep the windows they already had.
    Component {
        id: pinWindowComponent

        PinWindow {
            shim: app.shim
            session: app.session
        }
    }

    // One mask per screen, while a capture flow is running.
    //
    // A second `Instantiator` rather than a shared delegate: a pin's model is the
    // set of pictures the user owns and a mask's model is the list of screens, and the
    // two change for entirely unrelated reasons - and the pin's is now not a model at
    // all (see the `Component` above). `index` is the Qt screen index, which is what
    // the mask state matched its frozen frame against, and that one does only ever
    // grow and empty as a whole.
    //
    // `Instantiator`, not `Repeater`: a Repeater parents what it makes into the scene,
    // and a Window is not scene content. Measured - a Repeater over the same model and
    // the same delegate left `topLevelWindows=0`.
    Instantiator {
        id: masks
        model: app.session.mask_count

        delegate: CaptureMask {
            shim: app.shim
            session: app.session
            onGrabbed: app.refreshOtherMasks(this)
        }

        // Neither `index` nor `modelData` exists inside a QQmlInstantiator delegate -
        // measured on 6.10.1 - so the slot comes from the signal. `assign` rather than
        // a property write for the same reason as the pin windows: the object owns its
        // view and the two have to agree before the first frame.
        onObjectAdded: (index, object) => {
            object.assign(index)
            app.maskWindows[index] = object
        }

        // `endMask` comes through here: the model is the slot count, so closing the
        // overlay is what retires the windows.
        onObjectRemoved: (index) => { app.maskWindows[index] = null }
    }

    // 屏幕上那一句话的窗。一个进程只有一条最近的话，所以这是直接声明的一个窗，不是第二个
    // `Instantiator`：贴图按用户拥有的图片数、遮罩按屏幕数改变成员，而这一扇从不会因为多了一句话
    // 而多出来。
    //
    // 它写在根 `Item` 里面，而 `Window` 不是场景内容——`Instantiator` 的 delegate 走的是同一条路
    // （父对象是那个非 Item 的 QObject）。这里是不是也成立，由 `--probe` 的 Qt 消息清单来回答：那条
    // 腿把任何 `warn:` 判成失败，而不是由这段注释担保。
    HintWindow {
        session: app.session
    }

    /// One selection, in desktop pixels; N windows, each drawing its own clipped
    /// piece of it. The window that took the pointer event has already reloaded
    /// itself, so this is the other one - told by the signal rather than by a timer,
    /// because a dim that catches up a frame late is a dim the user watches lag the
    /// mouse across the seam.
    ///
    /// Reload-only: it never re-emits `grabbed`, which is what stops two windows
    /// from refreshing each other forever.
    function refreshOtherMasks(from) {
        for (let i = 0; i < maskWindows.length; ++i) {
            const m = maskWindows[i]
            if (m && m !== from) {
                m.view.reload()
            }
        }
    }

    /// Bring the window set back to the pin set: one window for every id the state
    /// holds, and none for an id it dropped.
    ///
    /// The id is the whole of the bookkeeping, which is why nothing here re-points a
    /// window at another pin: `assign` runs once, at creation, and the window keeps the
    /// pin it was made for until it is destroyed. `repinWindows` did the opposite - it
    /// let a model destroy every window and then tried to tell the rebuilt ones which
    /// pin they showed, from a slot index that the closing of an earlier pin had moved.
    ///
    /// Created in `pinId(0..count)` order, which is the state's bottom-first order, so
    /// the stacking the pile's rows are graded on comes out the way the `Instantiator`
    /// left it.
    ///
    /// `close()` before `destroy()`: the second drops the QML object, the first is what
    /// takes the native window off the screen, and a leg that counts windows cannot tell
    /// a closed pin from a live one whose pixels are simply not shown.
    ///
    /// Parented to `shim` rather than to `app`, which is the parent the `Instantiator`
    /// gave these windows when it made them: a `Window` is not scene content, and what
    /// the object needs from a parent is only a lifetime to die with.
    function syncPins() {
        const live = []
        for (let i = 0; i < app.session.count; ++i) {
            const id = String(app.session.pinId(i))
            live.push(id)
            if (!app.pinWindows[id]) {
                const w = pinWindowComponent.createObject(app.shim)
                app.pinWindows[id] = w
                w.assign(Number(id))
            }
        }
        for (const held of Object.keys(app.pinWindows)) {
            if (!live.includes(held)) {
                const w = app.pinWindows[held]
                delete app.pinWindows[held]
                w.close()
                w.destroy()
            }
        }
    }

    // §8.5 - a screen changed resolution, scaling or existence.
    //
    // Qt is the one that notices, and it has to be Qt: the hot key pump's window is
    // message-only, so the platform's display-change broadcast never reaches it, and
    // delivering that broadcast by hand would mean writing to windows this process
    // does not own. QScreen's own signals and the application's add/remove pair cover
    // both halves of the clause - 分辨率 and 插拔 - from inside this one program.
    Connections {
        target: app.shim

        function onScreensChanged() {
            app.refitPins()
        }
    }

    // A pin closed, or one opened, or the batch menu destroyed a group - every one of
    // those writes `count`, and `count` is the only thing this file is told about.
    //
    // The sync waits one loop turn, and that turn is not decoration: the change usually
    // *is* a pin window's own menu item, so `closePin` is still on the stack of the
    // window being retired when the notify arrives - and P55's run above is what a
    // destroyed QML object under a running function looks like. One turn is nothing a
    // user can see, and `restart` coalesces a burst of changes into one read of a state
    // that is already final.
    Timer {
        id: pinSync
        interval: 0
        repeat: false
        onTriggered: app.syncPins()
    }

    Connections {
        target: app.session

        function onCountChanged() {
            pinSync.restart()
        }
    }

    /// 把每一扇贴图窗的视线重新问一次 Rust：只 `reload`，不造窗也不关窗。
    ///
    /// §5.5.11 那一族藏与放只改 `view_data` 里的那一枚 `live`，桌面没动、贴图没挪，所以
    /// 这一格不叫 `desktopChanged` —— 那是下一枚函数要问的问题，一张对话框回答不了它。
    function redrawPins() {
        for (const held of Object.keys(app.pinWindows)) {
            app.pinWindows[held].view.reload()
        }
    }

    /// Rust decides which pins are now outside what is left of the desktop and pulls
    /// them back in, and says the sentence when it had to move one. This only makes
    /// the windows look again - and looks again whether or not a pin moved, because
    /// the x/y every window draws at is divided by a dpr that a scaling change alters
    /// while leaving every physical pixel where it was. A handler that trusted the
    /// sentence would reload half of what changed.
    ///
    /// Reload-only, like `refreshOtherMasks` above: nothing here re-emits, so one
    /// screen change is one refit rather than a signal that feeds its own handler.
    function refitPins() {
        app.session.desktopChanged()
        app.redrawPins()
    }
}
