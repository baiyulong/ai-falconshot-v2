import QtQuick
import QtQuick.Window
import dev.falconshot 1.0

// §5.2's mask: the frozen desktop, dimmed everywhere except the selection.
//
// One window per screen, sized and placed by the mask state rather than by
// `Screen`, because a two-monitor flow has to cover both and the pixels it shows
// were captured before any of these windows existed. The window owns no decisions:
// the hole, the dim and which path draws them are read off `MaskView`, which reads
// them off Rust.
//
// The window is opaque on purpose. "Dimmed" means *the frozen frame pushed toward
// black*, not the live desktop showing through - so nothing here is transparent and
// nothing here can accidentally capture the tool that is drawing it.
Window {
    id: win

    required property PinShim shim
    required property Session session

    // Created by the Instantiator before it knows which screen it is: index -1
    // reads as not-live, which is the same reason a pin window starts at id -1.
    readonly property MaskView view: MaskView { index: -1 }

    /// The grip the pointer is over - or holding. The numbers are Rust's
    /// (`MaskState::grip_code`): `0` nothing, `1` a new rectangle, `2..9` the eight
    /// handles in `Handle::all()` order (Nw, N, Ne, E, Se, S, Sw, W), `10` the whole
    /// selection. A number is what crosses the bridge, because the alternative is
    /// QML matching on an enum it does not own.
    property int grip: 0

    /// The hole changed under *this* window's pointer. `main.qml` reloads the other
    /// mask windows on it: the selection is one desktop-space rectangle and every
    /// window draws its own clipped piece of it, so a drag on screen 0 has to move
    /// the dim on screen 1 before the button is let go.
    signal grabbed()

    function cursorFor(g) {
        switch (g) {
        case 2:  case 6:  return Qt.SizeFDiagCursor
        case 3:  case 7:  return Qt.SizeVerCursor
        case 4:  case 8:  return Qt.SizeBDiagCursor
        case 5:  case 9:  return Qt.SizeHorCursor
        case 10:          return Qt.SizeAllCursor
        case 1:           return Qt.CrossCursor
        default:          return Qt.ArrowCursor
        }
    }

    // §5.3.9's "配合修饰键进行更细或更大步长调整": one device pixel a keystroke,
    // ten with Shift, and Alt moves the bottom-right edge instead of the rectangle.
    // The step is a *device* pixel because that is the unit the PRD's title names -
    // 像素级控制 - not the unit the pointer arrives in.
    function step(dx, dy, mods) {
        // §5.14.2 的翻页自 P113 起住在 `chord()` 里（Ctrl+Up / Ctrl+Down），而上面那四行
        // `Keys.on*Pressed` 是**任何**修饰键都会触发的，所以这一格得先让开那一档：不让开，一次
        // Ctrl+Up 就会同时做两件事 —— 把框往上挪一像素、又翻一页。
        //
        // 让开的只有"Ctrl **单独**"那一档（不带 Shift、也不带 Alt），可观测面因此最小：Shift 在
        // §5.3.9 里说的是步长（一步 ↔ 十步），Alt 说的是挪哪条边，两枚各自与 Ctrl 同按时仍然
        // 照原样走 —— `Ctrl+Shift+Up` 从前是十步、现在还是十步，`Ctrl+Alt+Up` 从前是挪右下边、
        // 现在还是。产品改口的只有裸的那一枚：§5.3.9 只承诺了"更细或更大步长"，而那两档由 Shift
        // 与 Alt 各自占着，Ctrl 单独一档产品从前什么也没说，现在说的是历史翻页。
        // 两条腿因此按任意顺序都不互踩：`chord()` 里那两枚也只认裸的。
        if ((mods & Qt.ControlModifier) !== 0
            && (mods & (Qt.ShiftModifier | Qt.AltModifier)) === 0) {
            return
        }
        const n = (mods & Qt.ShiftModifier) ? 10 : 1
        view.nudgeHole(dx * n, dy * n, (mods & Qt.AltModifier) !== 0)
    }

    // §5.7.5's 完成折线, §5.7.13's 放大落位 and §5.3's 完成选区 are the same two
    // events - a double-click and `Enter` - so the ladder is stated once here instead
    // of in three handlers that could each forget half of it. *Which* of the two ink
    // finishers is pending is Rust's answer (`finishInk`), not this function's; what
    // this level knows is that a keystroke the layer claims is not the crop's.
    function finishInkOrHole() {
        if (view.finishInk()) {
            return true
        }
        return view.commitHole()
    }

    // The `Instantiator` says which screen this is after the object exists, and
    // `MaskView` holds the answer - so this window forwards rather than letting
    // main.qml reach through two levels to set it.
    function assign(index) {
        view.assign(index)
    }

    // 把 `Ctrl+…` 的匹配抽成一个 win 上的函数，量具才能拿与真按键**完全相同的两个数值**
    // (key, modifiers) 走一遍这条分支，而不是去合成一次按键：一次注入的按键会落进当时有
    // 焦点的那个应用，而跑级时焦点在谁的窗口里，产品说不出、也不该替用户决定（第 46 项 ②）。
    // 抽出去之后 `Keys.onPressed` → `chord()` 那一步（Qt 把 WM_KEYDOWN 翻成 event.key /
    // event.modifiers）本轮没有判定行 —— 那是这条腿欠的部分，不是它覆盖的部分，写在这里而
    // 不是留在日志里。返回值 = 原来那句 `event.accepted = true` 的条件。
    function chord(key, modifiers) {
        if ((modifiers & Qt.ControlModifier) === 0) {
            return false
        }
        const shifted = (modifiers & Qt.ShiftModifier) !== 0
        const undo = key === Qt.Key_Z && !shifted
        const redo = key === Qt.Key_Y || (key === Qt.Key_Z && shifted)
        // `Ctrl+C`, and only without Shift: `Ctrl+Shift+C` is not this program's,
        // and a copy that swallowed a chord it did not implement would leave the
        // user with no way to find out what the chord does.
        const copy = key === Qt.Key_C && !shifted
        // §4.1's `Ctrl+S` = "打开保存流程或执行快速保存", and the two halves of that
        // sentence are now two chords: the unshifted one opens the dialog §5.5.5 asks
        // for, `Ctrl+Shift+S` is §5.5.4's 快速保存 and opens nothing. Shift is the
        // whole difference, which is why the shifted chord is named here rather than
        // left to fall through: an unimplemented `Ctrl+Shift+S` would open the dialog
        // by accident or nothing at all, and a user could not tell which.
        const save = key === Qt.Key_S && !shifted
        const quick = key === Qt.Key_S && shifted
        // §5.14.2 step 2「使用上一条、下一条快捷键浏览记录」：裸 Ctrl 配 Up/Down。Alt 那一枚
        // 也要查，因为上面那四枚（Z/Y/C/S）都不带方向键，而方向键在 `step()` 那头还有一位主人
        // —— Alt 单独一档说的是"挪哪条边"，`Ctrl+Alt+Up` 自 P113 起仍是挪边，不翻页。
        // 这两枚的条件与 `step()` 头上让开的那一条必须**一字同**（Ctrl 而既不带 Shift 也不带
        // Alt）：错开一寸，一次按键就会同时挪一像素又翻一页，而这两件事谁也不会报错。
        const alted = (modifiers & Qt.AltModifier) !== 0
        const older = key === Qt.Key_Up && !shifted && !alted
        const newer = key === Qt.Key_Down && !shifted && !alted
        if (!undo && !redo && !copy && !save && !quick && !older && !newer) {
            return false
        }
        // The toolbar owns the gesture, key or click alike: it is the item that
        // has to remember whether the copy got through, and a second path to the
        // same `copySelection()` is a path that colours its own sentence wrong.
        if (copy) {
            inkBar.copy()
            return true
        }
        if (save) {
            inkBar.save()
            return true
        }
        if (quick) {
            inkBar.quickSave()
            return true
        }
        if (older || newer) {
            // 只递方向。锚点（此刻停在第几行）、库里那一侧还有没有行、以及"翻过去就是重新进入
            // 那一行的原选区"全在 Rust 那一头：`Session::historyFlip` → `settings::neighbor_id_for`
            // → `MaskState::restore_history_selection` → 同一枚 `set_hole`。界面既不该猜锚点，
            // 也不该拿一句中文去判翻没翻成 —— 那两句都是产品自己说的话，屏幕与量具读同一份。
            session.historyFlip(older)
            // 与 undo/redo 那一臂同一个收尾：Rust 那头的框动了，这扇窗要把它读回来。翻不动时
            // 那次 reload 读到的是同一枚矩形，`main.qml` 的那句 "Reload-only: it never re-emits
            // grabbed" 保证这里不会长成一次循环。
            win.grabbed()
            return true
        }
        if (redo ? view.redoStep() : view.undoStep()) {
            win.grabbed()
        }
        return true
    }

    color: "black"
    flags: Qt.Window | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint
    // The `--mask` harness finds these by title to put `PrintWindow` on them, and
    // the monitor name is what makes two of them tellable apart in the report. No
    // spaces: `pinTopLevels` splits its fields on whitespace, so a title with a
    // space in it is a title the harness cannot match.
    title: "falconshot-mask-" + view.name

    // Which of the two dim paths is actually drawing: the chosen one, unless the
    // shader did not compile - and `QT_QUICK_BACKEND=software` was measured (2026-10-06)
    // to leave `status` at 1 (Uncompiled) rather than fail loudly. Falling back here
    // is what makes the software RHI a supported configuration instead of a black
    // screen: without this line the mask paints *nothing* over the desktop.
    readonly property bool drawShader: win.view.shader && effect.status === ShaderEffect.Compiled

    visible: view.live && view.shown

    // Qt's screen geometry, already device-independent - the space a Window lives
    // in. Dividing a Win32 rect by the wrong scale is how a mask ends up covering
    // three quarters of its own screen.
    x: view.win_x
    y: view.win_y
    width: view.win_w
    height: view.win_h

    onFrameSwapped: view.noteSwap()

    // The keyboard half of the interaction (§5.3.9) only reaches a window that has
    // the focus, and a mask made from a background process has no reason to be
    // given it. Asking once it is actually on screen is also why this is not in
    // `Component.onCompleted`: the first window may be built while `shown` is still
    // false, and activating a hidden window is a request the platform drops.
    onVisibleChanged: if (visible) win.requestActivate()

    Component.onCompleted: {
        // The provider has to be on the engine this window renders with, and the
        // mask may be the first window the process ever makes.
        shim.install(win)
        view.reload()
        surface.forceActiveFocus()
    }

    // The frozen desktop, stretched into the window it belongs to.
    //
    // `cache: false` because the key carries the revision, not the URL's shape;
    // `asynchronous: false` because a mask that fades in its background is a mask
    // the user starts selecting on before the picture is there.
    Image {
        id: frozen
        anchors.fill: parent
        cache: false
        asynchronous: false
        fillMode: Image.Stretch
        visible: !win.drawShader
        source: win.view.key !== ""
                ? "image://falconshot/" + win.view.key
                : ""
    }

    // Path A - four dim rectangles around the hole. Four quads over the screen,
    // and the only thing that works when the RHI is software or the shader failed
    // to load, which is why it is kept rather than deleted.
    Item {
        anchors.fill: parent
        visible: !win.drawShader

        Rectangle {
            x: 0; y: 0
            width: parent.width
            height: Math.max(0, win.view.hole_y)
            color: Qt.rgba(0, 0, 0, win.view.dim_alpha)
        }
        Rectangle {
            x: 0; y: win.view.hole_y + win.view.hole_h
            width: parent.width
            height: Math.max(0, parent.height - (win.view.hole_y + win.view.hole_h))
            color: Qt.rgba(0, 0, 0, win.view.dim_alpha)
        }
        Rectangle {
            x: 0; y: win.view.hole_y
            width: Math.max(0, win.view.hole_x)
            height: Math.max(0, win.view.hole_h)
            color: Qt.rgba(0, 0, 0, win.view.dim_alpha)
        }
        Rectangle {
            x: win.view.hole_x + win.view.hole_w; y: win.view.hole_y
            width: Math.max(0, parent.width - (win.view.hole_x + win.view.hole_w))
            height: Math.max(0, win.view.hole_h)
            color: Qt.rgba(0, 0, 0, win.view.dim_alpha)
        }
    }

    // Path B - one pass that writes the dimmed and the un-dimmed pixel side by
    // side, so the frame and the dim arrive as a single quad. Chosen for the shape
    // of the hole it can express (a rounded, feathered selection is a fragment
    // away), not for speed: P1 measured no advantage at 4K.
    //
    // `status` is reported to Rust either way, because "the .qsb was not in the
    // package" is exactly the failure a green build can hide (plan §9.1).
    ShaderEffect {
        id: effect
        anchors.fill: parent
        // Stays visible whenever the shader path was *chosen*, not only when it
        // compiled: `status` only resolves on an item that is in the scene graph, so
        // gating this on `drawShader` would deadlock it at Uncompiled.
        visible: win.view.shader
        supportsAtlasTextures: false

        property ShaderEffectSource src: ShaderEffectSource {
            sourceItem: frozen
            // `drawShader`, not `view.shader`: `hideSource` is what keeps the
            // desktop from being drawn twice, and a source hidden by an effect that
            // then fails to draw is a black screen. Measured on the software RHI,
            // where the chosen path is the shader, the status is 1, the rectangles
            // take over - and the grab still read 0.000 until this line stopped
            // hiding the picture the fallback was supposed to dim.
            hideSource: win.drawShader
        }
        property color dimColor: Qt.rgba(0, 0, 0, win.view.dim_alpha)
        property vector4d hole: Qt.vector4d(win.view.hole_x, win.view.hole_y,
                                            win.view.hole_w, win.view.hole_h)
        property vector2d deviceSize: Qt.vector2d(width * win.view.dpr,
                                                  height * win.view.dpr)
        property real dpr: win.view.dpr

        fragmentShader: "dim_hole.frag.qsb"
        onStatusChanged: win.view.noteShaderStatus(status)
        Component.onCompleted: win.view.noteShaderStatus(status)
    }

    // A new frame key means Rust made a *new* slot, whose `shader_status` starts at
    // none, while this window's `ShaderEffect` was never destroyed and therefore has
    // no `statusChanged` left to offer. The two sides then disagree about which dim
    // path is on screen, and the gauge grades that disagreement: measured on the round
    // after a warm reopen as `dim path - rectangles (fallback)` and
    // `shader ... FAIL None`, with the readback itself right at `dim=0.406`.
    //
    // Re-asserting on the key change, rather than only from `onStatusChanged`, is what
    // keeps Rust's copy of the fact as live as QML's drawing of it. `Connections` is
    // the only form this takes: a `key` change is a signal of `view`, not of the
    // window, and `onKeyChanged` at the root is a document error measured this run
    // ("Cannot assign to non-existent property").
    Connections {
        target: win.view
        function onKeyChanged() {
            win.view.noteShaderStatus(effect.status)
        }
    }

    // §5.7's ink, and a *second* texture rather than a second drawing of the desktop:
    // Rust composites the frozen frame and every object into one layer, transparent
    // outside the selection, and this item puts it over the dim. That is what keeps
    // the two dim paths above it untouched - and it is why `--ink` measures the dim
    // again after the strokes: if the transparency were wrong, an un-dimmed copy of
    // the desktop would replace the dim the user has not selected.
    //
    // The key changes with every repaint, for the same reason the frozen frame's does
    // (§3.6 constraint 8): `cache: false` only re-requests when the URL changes, so a
    // stroke published under the previous stroke's key would be in the document, in
    // the canvas and nowhere on screen.
    Image {
        anchors.fill: parent
        cache: false
        asynchronous: false
        fillMode: Image.Stretch
        source: win.view.overlay_key !== ""
                ? "image://falconshot/" + win.view.overlay_key
                : ""
    }

    // §5.2.3's selection edge. 2 device-independent pixels, which is 4 at 200% -
    // the number the user sees as "the box around what I picked".
    Rectangle {
        x: win.view.hole_x
        y: win.view.hole_y
        width: Math.max(0, win.view.hole_w)
        height: Math.max(0, win.view.hole_h)
        color: "transparent"
        border.color: "white"
        border.width: 2
    }

    // §5.3.5's live size readout, gated by the flow's own copy of
    // `[capture] show_dimensions` (`win.view.show_dimensions`, snapshotted
    // in Rust at `open`). `selected` is the other half of the gate: a hole
    // that has not been made yet has no width to name, and this is the one
    // line on the mask whose value comes from the same `hole_w`/`hole_h`
    // that the eight handles were placed by, so a readout that disagrees
    // with the box means the box itself is wrong.
    Text {
        x: win.view.hole_x
        y: Math.max(0, win.view.hole_y - 22)
        visible: win.view.selected && win.view.show_dimensions
        color: "white"
        text: Math.max(0, win.view.hole_w) + " × " + Math.max(0, win.view.hole_h)
    }

    // Where the eight grips sit, as fractions of the hole, in `Handle::all()`'s
    // order - so `index + 2` is the grip code the pointer takes there.
    readonly property var gripFx: [0, 0.5, 1, 1, 1, 0.5, 0, 0]
    readonly property var gripFy: [0, 0, 0, 0.5, 1, 1, 1, 0.5]

    // §5.3.1's eight adjustment points. Drawn, never clicked: the hit test that
    // decides what a press means is Rust's (`view.hitTestAt`), and a second copy of
    // it here - one MouseArea per handle - is how a cursor and a drag start
    // disagreeing about which edge the user took.
    Item {
        anchors.fill: parent
        visible: win.view.selected

        Repeater {
            model: 8

            delegate: Rectangle {
                required property int index

                x: win.view.hole_x + win.gripFx[index] * win.view.hole_w - 4
                y: win.view.hole_y + win.gripFy[index] * win.view.hole_h - 4
                width: 8
                height: 8
                radius: 1
                color: "white"
                border.color: "black"
                border.width: 1
            }
        }
    }

    // The whole pointer surface. One MouseArea for the entire screen is the shape
    // the state machine expects: a press anywhere is either a grip on the existing
    // rectangle or the start of a new one, and Rust decides which - so there is no
    // dead zone between the handles to explain.
    MouseArea {
        id: surface
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton
        hoverEnabled: true
        focus: true
        cursorShape: win.cursorFor(win.grip)

        onPressed: (mouse) => {
            win.grip = view.pressAt(mouse.x, mouse.y)
            win.grabbed()
        }

        onPositionChanged: (mouse) => {
            // The MouseArea's own state, not the event's: this event type's members
            // are x, y, button, buttons, modifiers, source, isClick, wasHeld,
            // accepted, flags - there is no `pressed`, so the test below was false on
            // every move and a drag never reached Rust as a drag.
            if (surface.pressed) {
                view.dragTo(mouse.x, mouse.y)
            } else {
                win.grip = view.hitTestAt(mouse.x, mouse.y)
            }
            win.grabbed()
        }

        onReleased: (mouse) => {
            view.releaseAt()
            win.grip = view.hitTestAt(mouse.x, mouse.y)
            win.grabbed()
        }

        // The drag can be ended by something other than a release inside this
        // window. Leaving the grab in place would leave Rust moving a rectangle for
        // a button nobody is holding.
        onCanceled: view.releaseAt()

        onDoubleClicked: if (win.finishInkOrHole()) win.grabbed()

        Keys.onEscapePressed: (event) => {
            // `accepted`, not `accept()`: this event type's members are key, text,
            // modifiers, isAutoRepeat, count, nativeScanCode, accepted, and the one
            // method `matches()`. `accept()` is the C++-side name - qmllint said so,
            // `Member "accept" not found on type "QQuickKeyEvent"`, in the batch of
            // warnings that was read as noise and left.
            event.accepted = true
            // The ladder, not a property: the first Esc un-selects, the second one
            // cancels. `endMask` destroys these windows from the model, and doing
            // that while one of them is inside a key handler is a use-after-free
            // QML will not warn about - so it goes through the event loop.
            if (!view.stepBack()) {
                Qt.callLater(() => win.session.endMask())
            }
        }

        Keys.onReturnPressed: (event) => {
            event.accepted = true
            if (win.finishInkOrHole()) win.grabbed()
        }

        Keys.onEnterPressed: (event) => {
            event.accepted = true
            if (win.finishInkOrHole()) win.grabbed()
        }

        Keys.onLeftPressed: (event) => win.step(-1, 0, event.modifiers)
        Keys.onRightPressed: (event) => win.step(1, 0, event.modifiers)
        Keys.onUpPressed: (event) => win.step(0, -1, event.modifiers)
        Keys.onDownPressed: (event) => win.step(0, 1, event.modifiers)

        // §5.7.19's 撤销 / 重做 and §5.5.2's 复制.
        //
        // Tested against `event.key` in the generic `pressed` rather than with
        // `Keys.onZPressed`: this type's per-key signals are the ones its metadata
        // declares - the arrows, Escape, Return, Enter, the digits, the media keys -
        // and the letters are not among them, so an `onZPressed` handler is a name the
        // linter could not resolve and nothing was willing to guess about.
        //
        // Ctrl is tested here because the arrow rows above fire with any modifier,
        // and `Ctrl+Shift+Z` is accepted alongside `Ctrl+Y` because that is what
        // people who came from another program type.
        Keys.onPressed: (event) => {
            if (win.chord(event.key, event.modifiers)) {
                event.accepted = true
            }
        }
    }

    // §5.7.11 steps 2-3: the field the user types into, sitting on the box Rust
    // measured for it. Declared after the MouseArea so it is the item under the
    // pointer inside that box - which is what makes step 5 work the way the PRD
    // writes it: a click anywhere else reaches `surface`, and `pressAt` there means
    // "done", because the layer only ever sees a press that the field did not take.
    //
    // The field holds the letters while they are being typed and the layer holds them
    // afterwards, so exactly one of the two draws a given string. Both drawing it is a
    // doubled glyph, and that is why the layer keeps no preview of a text box.
    TextInput {
        id: field
        visible: win.view.text_editing
        x: win.view.text_x
        y: win.view.text_y
        // At least a pixel, or a box the font could not measure is a field nobody can
        // see, click or type into.
        width: Math.max(1, win.view.text_w)
        height: Math.max(1, win.view.text_h)
        clip: true
        text: ""
        color: Qt.rgba(((win.view.pen_rgb >> 16) & 255) / 255,
                       ((win.view.pen_rgb >> 8) & 255) / 255,
                       (win.view.pen_rgb & 255) / 255, 1.0)
        // Device-independent, and Rust divided: the pen's 字号 is a count of device
        // pixels, and a field sized in the window's own units would be a third of the
        // height of the text it stands in for on a 300% screen.
        font.pixelSize: win.view.text_font
        // No wrap: the box is measured as one line per paragraph, so a field that
        // wrapped would show the line in more rows than the committed object has.
        wrapMode: TextInput.NoWrap
        selectByMouse: true
        persistentSelection: false
        // No border: qmllint answers `Type border is used but it is not resolved` for
        // this group on TextInput, and a name nothing can resolve is a name that cannot
        // be shown to draw - the caret and the letters are what marks the box.

        onVisibleChanged: {
            // A reopened box is a new one: Rust starts its string empty, so the field
            // has to say so too - and the old string would not fire `onTextChanged` on
            // its own, which is how the two copies would go on disagreeing silently.
            if (visible) {
                field.text = ""
                field.forceActiveFocus()
            } else {
                surface.forceActiveFocus()
            }
        }

        // The whole current string, on every change. Rust keeps the string and nothing
        // else - no caret, no selection, no IME preedit - because the field owns those.
        onTextChanged: win.view.setText(text)

        // `Keys` rather than `onAccepted`, and `accepted` rather than a fall-through:
        // `Enter` means "this text is finished" here, and if the event went on up the
        // chain the window's own handler would read it as 完成选区 and close the mask
        // on a crop the user has not looked at.
        Keys.onReturnPressed: (event) => {
            event.accepted = true
            if (win.view.finishInk()) win.grabbed()
        }

        Keys.onEnterPressed: (event) => {
            event.accepted = true
            if (win.view.finishInk()) win.grabbed()
        }

        // The same ladder `surface` runs, from the item that has the keys while a box
        // is open: the first Esc drops the half-typed text, the next un-selects.
        Keys.onEscapePressed: (event) => {
            event.accepted = true
            if (!win.view.stepBack()) {
                Qt.callLater(() => win.session.endMask())
            }
        }
    }

    // §5.7's toolbar, last so it is on top of the pointer surface: a button has to
    // win the click it is under, and the only way to make that true without a
    // `z` on every item is to be the sibling that comes after.
    //
    // `view` and `session` are handed over rather than reached through `win`, so
    // this file stays the only one that knows a mask window has a `MaskView` on it.
    AnnotationToolbar {
        id: inkBar
        view: win.view
        session: win.session

        // 量具的那条 `Ctrl+Shift+S` 腿要从窗口这层的 `chord()` 走进来，而面板够不着 `win`：
        // 与 `view`/`session` 同一条规矩，宿主也是递进去的，不是让它去爬窗口。
        chordHost: win

        // The same propagation the pointer does: undo, redo and 全清 change a
        // desktop-space picture that both windows draw.
        onGrabbed: win.grabbed()
    }
}
