import QtQuick
import QtQuick.Window
import QtQuick.Controls
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

    /// §5.4.1 / §5.3.9 第 4 步 —— 放大镜这一块面板摆在哪一格，本页**不再自己记**。
    ///
    /// 以前这里是两格 `property real cursorX/cursorY`，由 `surface` 的按下/移动/松开三处写。
    /// 那本账只有鼠标一条腿：键盘把选区挪过一格之后屏幕上没有新的事件进来，面板就留在原地，
    /// 里面画的却是刚被挪掉的那条边 —— 「面板摆在指针那儿、说的是另一颗像素」。现在页上绑的
    /// 是 `view.magnifier_anchor_x/y`，那正是桥这一趟真的去采的那一点在本窗的读法（作者：
    /// `crate::mask::MaskState::magnifier_at_point`，经 `crate::mask::MagnifierData` 交出），
    /// 于是像素、坐标那一句、色板、面板摆在哪一格四件事同一次装配、同一颗点，不可能再来自两趟。
    ///
    /// 换算成窗内 DIP 也仍然只有一位作者：`MaskState::local_dip`，与选区、高亮框、文本框、
    /// 圆角把手同一把尺子（#281）。

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
    //
    // §5.3.9 第 4 步「放大镜同步显示边缘像素」不在这一格里算：这一手只把方向与步长递过桥，
    // `view.nudgeHole` 在桥里挪完选区之后紧接着把放大镜重采样到刚被挪掉的那条边（作者：
    // `crate::mask::MaskState::nudge_magnified` + `falcon_core::geometry::nudge_sample`）。
    // 页上一旦自己算"哪条边动了"，那就是同一份几何的第二个作者（#281）。
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
        // 选区是**一整个桌面矩形**，每一扇窗画的是它自己裁出来的那一块 —— 键盘挪过一格之后
        // 别的屏上那块暗区也要跟着挪，而 `main.qml` 只认这一枚信号（`refreshOtherMasks` 只
        // reload、不再回发，所以两条腿不会互相刷新到永远）。上面那三处鼠标 handler 早已这么
        // 办；方向键这一条腿以前没有，跨屏时另一扇屏上的框就差这一格。
        win.grabbed()
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
        // §5.3.6 第二步的后半「或使用对应快捷操作」：`Ctrl+D`（D = dimensions）把键盘交给
        // 尺寸那一格。没有选区时这一枚**不接管**——那两格还没长出来，把焦点递给一枚不可见的
        // 文本框是一次谁也看不见的失败，而"这枚键没反应"与"这枚键被别的接走了"必须能分开说。
        const sizing = key === Qt.Key_D && !shifted && !alted && win.view.selected
        if (!undo && !redo && !copy && !save && !quick && !older && !newer && !sizing) {
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
        if (sizing) {
            sizeBar.focusWidth()
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

    // 截图那一路的底是黑的：暗化层画在它上面，桌面从此看不见。§5.4.5 那一块透明白板要的
    // 是反面 —— 底跟着透明、暗化归零（两格都由 `view.board_clear` / `view.dim_alpha` 递过来，
    // 界面不自己判是哪一块板），用户才看得穿自己那块画布。
    color: view.board_clear ? "transparent" : "black"
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

    // §5.3.2 第三步「系统高亮窗口边界」：光标此刻最直接指向的那一扇**桌面上的窗**。
    //
    // 与下面那条 §5.2.3 的选区边**同一族画法**（一张 `color: "transparent"` 的
    // `Rectangle`，只描边，2 DIP），因为它是同一块屏幕上的第二把框：读的是
    // `view.hover_*` 那五格，而五格与 `view.hole_*` 出自 Rust 里同一位作者
    // (`MaskState::local_dip`)，所以在 200% 屏上高亮框与选区不会差出一像素。
    //
    // 它画在选区边**之前**，于是选区那条白边压在上面：一扇窗被选中之后，用户要看的
    // 是"我选了哪一块"，不是"这块底下原本是哪扇窗"。
    //
    // 亮不亮只看 `view.hover_live`，不看尺寸，也不看 `view.selected` —— 遮罩是带着
    // 默认选区开门的（`MaskState::default_hole`），拿 `selected` 当闸门这一路会永远不亮；
    // 真正的闸门是 Rust 那一格的 `hover_taken`（按过一次就不再报，Esc 第一档清掉选区
    // 之后又回来），页上只是读它的结果。
    //
    // 这一页**不绑** `hover_liveChanged` 之类的信号名：`#[qproperty(bool, hover_live)]`
    // 生成的 notify 是 `hover_liveChanged` 而不是驼峰，绑错是静默的（本仓踩过两次）。
    // 绑值这条路没有名字可绑错。
    Rectangle {
        x: win.view.hover_x
        y: win.view.hover_y
        width: Math.max(0, win.view.hover_w)
        height: Math.max(0, win.view.hover_h)
        visible: win.view.hover_live
        color: "transparent"
        border.color: "#4cc2ff"
        border.width: 2
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

    // §5.3.7 那枚单位在屏幕上的字样：下标是 Rust 的 `size_unit`，字样表也是 Rust 的
    // `unitNames()`（其本身又只是 `SIZE_UNITS` 里那两枚单位的 `Unit::label`）—— 与下面那枚
    // `anchorName` 同一条口径：**序号过桥、句子留在 Rust**，这一页上不出现 "px"/"dip" 两个
    // 字面量，所以清单长出第三格时不会有一行字还停在旧的两格上。
    readonly property string sizeUnit: win.view.unitNames().split("|")[win.view.size_unit]

    // §5.3.5's live size readout, gated by the flow's own copy of
    // `[capture] show_dimensions` (`win.view.show_dimensions`, snapshotted
    // in Rust at `open`). `selected` is the other half of the gate: a hole
    // that has not been made yet has no width to name.
    //
    // §5.3.7 起这一行读 `size_w`/`size_h` 而不是 `hole_w`/`hole_h`，理由不是风格。后两格是
    // **窗内几何**：`local_dip` 先把跨屏的选区裁进这一扇窗、再除过这一扇窗自己的缩放，它回答
    // "这一扇窗把洞画在哪、多大"；而这一行回答"你圈的这块有多大"——一张跨了两扇窗的选区没有
    // "这一扇窗的宽"这种尺寸，两扇窗各报一个数就是两句谎。换绑之后**两扇窗上这一行同字**，
    // 因为 `size_w`/`size_h` 与 `size_unit` 都来自 Rust 的同一份样本。下面那八枚把手仍然跟着
    // `hole_w`/`hole_h`：那是画法，不归这句读数管。
    Text {
        x: win.view.hole_x
        y: Math.max(0, win.view.hole_y - 22)
        visible: win.view.selected && win.view.show_dimensions
        color: "white"
        text: win.view.size_w + " × " + win.view.size_h + " " + win.sizeUnit
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

    // §5.7.3 步 4 的那一枚圆角把手，与上面八枚同一规：画出来给人握，而**不在页上点** —— 一枚
    // 按下的归属是 `pressAt` 里 Rust 那一位作者判的，这里再摆一枚 MouseArea 就是让光标与拖拽
    // 起点各答一次「用户握住了哪一枚」。位置也是 Rust 的（core 的 `grip_point`，加上本窗那一次
    // 除法），页上一处坐标都不自己算；只有这一枚点画多大是页自己的事，因为那是给人看的数而不是
    // 给人算的数。圆而不是方，是为了与那八枚角把手在屏幕上分得开：这一枚拧的是角有多圆。
    Rectangle {
        visible: win.view.radius_live
        x: win.view.radius_x - 4
        y: win.view.radius_y - 4
        width: 8
        height: 8
        radius: 4
        color: "white"
        border.color: "black"
        border.width: 1
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
            // §5.7.18 步 2「按住 Ctrl 拖动」的那只手：先把**这一枚按下**按着 Ctrl 没有递过去，
            // 再让 Rust 判这一趟拖的是原物还是副本。次序在这里是有内容的：`pressAt` 里
            // `Layer::press` 读一次就把那一格归 false，写晚了这一枚按下就按"没按 Ctrl"办了。
            //
            // 页上不自己判"点在谁身上"（那是 Rust 那一位作者的 `topmost_at`），也不自己存"这
            // 一趟是克隆"—— 中途松手不把已经开始的克隆改回移动，所以这一格只在按下递一次，
            // 不像上面那枚 Shift 每一寸移动都重写。
            view.holdClone((mouse.modifiers & Qt.ControlModifier) !== 0)
            win.grip = view.pressAt(mouse.x, mouse.y)
            // §5.4.1：按下这一点也算指针落到某一枚像素上，放大镜跟上；§5.4.2 之后按 C
            // 取的就是这一颗（`view.pickColor` 读的是桥里刚记下的那一点）。面板摆在哪一格
            // 也从这一趟来（`view.magnifier_anchor_x/y`），页上不再自己抄一份。
            view.updateMagnifier(mouse.x, mouse.y)
            win.grabbed()
        }

        onPositionChanged: (mouse) => {
            // The MouseArea's own state, not the event's: this event type's members
            // are x, y, button, buttons, modifiers, source, isClick, wasHeld,
            // accepted, flags - there is no `pressed`, so the test below was false on
            // every move and a drag never reached Rust as a drag.
            if (surface.pressed) {
                // §5.3.8「用户应可以临时解除比例约束」的那只手：Alt 按住的这一段拖拽走自由
                // 比例，松手就回到芯片那一格的锁（它不进 `view.ratio_index`，所以没有一把
                // "临时按着却永久留在解除态"的门）。
                //
                // 是 Alt 而不是 Shift 说"松开锁"：这一枚键说的是**锁住**（见下面那一手），
                // 一只手同时按两枚键各说一件相反的事，屏幕上没有谁能猜对是哪一件。Shift 在
                // 键盘那一趟是方向键的 ×10 步进（`win.step`）—— 那是 KeyPress，这只鼠标按着
                // 的这一段从不发出，所以两回事不会撞进同一次手势。
                //
                // §5.7.4 step 3「按住约束键可创建正圆」的那只手就是 Shift：按着的这一段，笔
                // 拖出来的那一框锁成 1:1。每一寸移动都照当下这颗键重写一次（`holdConstraint`
                // 不是一把门），所以中途松手、下一寸就回到自由比例。
                view.holdConstraint((mouse.modifiers & Qt.ShiftModifier) !== 0)
                view.dragTo(
                    mouse.x,
                    mouse.y,
                    (mouse.modifiers & Qt.AltModifier) !== 0
                )
            } else {
                win.grip = view.hitTestAt(mouse.x, mouse.y)
                // §5.3.2 第二、三步：没按住的那一段每一寸移动都要问一次「这一枚光标落在
                // 哪一扇窗」，上面那块高亮框读的就是这一手的结果。按下那一段**不**问 ——
                // `view.pressAt` 在 Rust 里已经为那一点算过一次（`MaskState::press`），
                // 再问一遍就是同一件事有两个时刻，而高亮说 A、单击选中 B 正是这么长出来的。
                view.hoverWindow(mouse.x, mouse.y)
            }
            // §5.4.1：拖拽与悬停都是"指针在屏幕上走到了这一点"，放大镜跟的是**指针**而不是
            // 选区，所以两条都要更新。§5.4.2 按 `C` 取色读的就是桥里被这一次调用记下那一点。
            view.updateMagnifier(mouse.x, mouse.y)
            win.grabbed()
        }

        onReleased: (mouse) => {
            view.releaseAt()
            win.grip = view.hitTestAt(mouse.x, mouse.y)
            view.updateMagnifier(mouse.x, mouse.y)
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
            // §5.4.2 —— 裸按 `C` 取色。放在 `win.chord(...)` **之前**判，是为了把
            // `Ctrl+C`（复制选区）与 `Ctrl+Shift+C`（不接管）留给上面那条通道；否则裸 C 一
            // 旦吞下按键，`chord()` 那一路的 `copy` 分支就跑不到了。
            //
            // 修饰键的判据：**不带 Ctrl、不带 Shift、不带 Alt** 才算裸 C。Shift 在
            // §5.3.9 里是 ×10 步进、Alt 是"挪哪条边"（都在方向键那一档），Ctrl 是复制；
            // 三枚里任何一枚按下时 C 都不该被取色器接走。
            //
            // 这一格还**不能**在文本框正被输入时接：`field` 拿到焦点时它的 `Keys.on*` 那
            // 一族会先收到事件并吃掉，本 surface 的 handler 就不会跑；所以这里不用二次判
            // 焦点——那一族的分工在 §5.7.11 步 5 的注释里已经说过一次。
            if ((event.modifiers & Qt.ControlModifier) === 0
                && (event.modifiers & (Qt.ShiftModifier | Qt.AltModifier)) === 0
                && event.key === Qt.Key_C) {
                view.pickColor()
                event.accepted = true
                return
            }
            // §5.7.16 步 3「修改……文字内容」的入口键。双击与 Enter 早被 `finishInkOrHole`
            // 那条梯子说成"收尾"（§5.7.11 步 3、§5.7.5、§5.7.13 共用那两个事件），裸按别的
            // 字母又各有其人，所以**起**编辑取 F2：它与 `chord()` 那一族（每枚都要求 Ctrl）
            // 和裸 C 都不撞。
            //
            // 不带修饰键才算这一枚，与裸 C 同一规（Shift 在 §5.3.9 是 ×10 步进、Alt 是"挪哪
            // 条边"、Ctrl 是复制）。
            //
            // 框已经开着时到不了这里：那一枚 `TextEdit` 有焦点，它的 `Keys.on*` 一族先吃掉
            // 事件，本 surface 的 handler 就不跑 —— 所以这里不必二次判焦点（与 §5.7.11 步 5
            // 那一段同一分工）。门开不开、哪一枚改得，是 Rust 那一位作者的话（见
            // `MaskState::edit_text`）；页上不许写第二套判据，也不许自己算框。
            if ((event.modifiers & (Qt.ControlModifier | Qt.ShiftModifier | Qt.AltModifier)) === 0
                && event.key === Qt.Key_F2) {
                // `true` = 框开了，另一扇窗也要重读这六格（`text_seed` 与该当哪一扇亮）；
                // 没开时屏幕上那句"为什么没反应"已经由 Rust 写进 `message` 那一格。
                if (view.editText()) {
                    win.grabbed()
                }
                event.accepted = true
                return
            }
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
    TextEdit {
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
        // 那一串字的颜色读的是**这一只框**的读数，不是画笔的：§5.7.16 步 3 改字那一只框里，
        // 提交上去的那一枚保留它自己的颜色，绑 `pen_rgb` 的字段会在框关掉那一刻换一次色 ——
        // 用户刚刚看着自己编辑的那一串就是旧的那一枚颜色。落新框时 `text_rgb` 就是画笔的那一枚
        // （`Layer::typing_colour` 走 `box_style`，没有框时它答画笔），所以这一格两条腿都诚实。
        // alpha 仍旧按 1.0 铺，与 `pen_rgb` 那一条同规。
        color: Qt.rgba(((win.view.text_rgb >> 16) & 255) / 255,
                       ((win.view.text_rgb >> 8) & 255) / 255,
                       (win.view.text_rgb & 255) / 255, 1.0)
        // Device-independent, and Rust divided: 字号 is stored as a count of device
        // pixels, and a field sized in the window's own units would be a third of the
        // height of the text it stands for on a 300% screen. The number read back is the
        // open box's own (§5.7.16 步 3 改的是那一枚对象的字号时，这一格跟着对象走)，不是
        // 工具栏上亮着的那一枚 —— `pen_font` 才是。
        font.pixelSize: win.view.text_font
        // The same style number in string form, and it reads the *box*'s family rather
        // than the pen's for the same reason 字号 does: Rust measures this box with
        // [`crate::annotate::Layer::box_style`], which is the object's own style while
        // §5.7.16 step 3's 改字 box is open. A box that drew in the pen's family would be
        // previewing a shape the committed object does not have. 字体 for the toolbar stays
        // `pen_family` — one of the two knobs is the pen's answer, the other the field's.
        font.family: win.view.text_family
        // No wrap still, and for the same reason: Rust measures the box as one line per
        // paragraph, so a field that folded a paragraph on its own would show it in more
        // rows than the committed object has. What changes is who ends a paragraph: that is
        // the user's own keystroke now - Shift+Enter, below - rather than a fold this item
        // decides for a string it was never told to break.
        wrapMode: TextEdit.NoWrap
        selectByMouse: true
        persistentSelection: false
        // No border: qmllint answers `Type border is used but it is not resolved` for
        // this group on TextEdit too, and a name nothing can resolve is a name that cannot
        // be shown to draw - the caret and the letters are what marks the box.

        onVisibleChanged: {
            // A reopened box is a new one: Rust starts its string empty, so the field
            // has to say so too - and the old string would not fire `onTextChanged` on
            // its own, which is how the two copies would go on disagreeing silently.
            //
            // §5.7.16 步 3 的那一只不是新的一只：它预先放着那一枚对象自己那句话，而那句话
            // 只有 Rust 有（[`Layer::typing_text`] 是唯一的一份），所以这一行读的是
            // `win.view.text_seed` 而不是字面的 `""` —— 新的一枚那一格本来就是空串。光标
            // 落在句尾：改字的人多半是要在末尾添一个词或删一个字，落在句首会让第一次
            // 敲键插在整串之前。
            if (visible) {
                field.text = win.view.text_seed
                field.cursorPosition = field.text.length
                field.forceActiveFocus()
            } else {
                surface.forceActiveFocus()
            }
        }

        // The whole current string, on every change. Rust keeps the string and nothing
        // else - no caret, no selection, no IME preedit - because the field owns those.
        onTextChanged: win.view.setText(text)

        // `Keys` rather than `onAccepted`, and `accepted` rather than a fall-through:
        // plain `Enter` means "this text is finished" here, and if the event went on up the
        // chain the window's own handler would read it as 完成选区 and close the mask
        // on a crop the user has not looked at.
        //
        // `accepted` is written before the two branches split, and on both of them, for a
        // second reason: a TextEdit that has the focus inserts a line of its own on Return,
        // shift held or not. Letting it would mean a newline next to the one put in below,
        // and a finish the user did not ask for - so this handler is the one that wins the
        // keystroke, whichever way it then goes.
        //
        // §5.7.11 规则三「文本框应支持换行」: Shift+Enter is the paragraph break. It goes in
        // at the caret through the item's own `insert`, which is what both changes the
        // string and fires `onTextChanged` below - one write, one hand carrying it to Rust,
        // no second copy of the string here and no second call to `setText`. The box itself
        // then grows downward on its own, because its height is the one Rust measured for
        // the string it was just given; nothing about a line count is computed on this side.
        Keys.onReturnPressed: (event) => {
            event.accepted = true
            if (event.modifiers & Qt.ShiftModifier)
                field.insert(field.cursorPosition, "\n")
            else if (win.view.finishInk())
                win.grabbed()
        }

        // The numpad's Enter is a different code, and the same two gestures.
        Keys.onEnterPressed: (event) => {
            event.accepted = true
            if (event.modifiers & Qt.ShiftModifier)
                field.insert(field.cursorPosition, "\n")
            else if (win.view.finishInk())
                win.grabbed()
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

    // §5.3.8 第一、二步的入口：比例那一排。
    //
    // 它长在**窗口**上而不是长在 `AnnotationToolbar` 里，是因为 PRD 的顺序是"先挑比例，再
    // 拖动创建选区"，而那块面板是 `visible: view.selected`：一张还没拖出选区的屏幕上没有它。
    //
    // 每一张屏各有一条，而它们说的是同一个状态 —— 锁住在 Rust 的 `MaskState`（桌面空间），
    // 所以在 1 号屏按下 1:1，2 号屏那一排跟着亮。这与"选区是一张桌面空间的图、两张窗各画
    // 自己那部分"是同一条口径，不是两份各自记着比例的界面。
    component RatioCell: Rectangle {
        id: cell

        property string label: ""
        property bool active: false
        // 与 `AnnotationToolbar` 的 `Cell` 同一枚 `required`：漏了一枚芯片的句子在构造期就
        // 发红，而不是等 hover。句子的内容不在这档 —— 那四句在 Rust 的 `RATIO_CELLS` 里。
        required property string hint
        signal tapped()

        width: cap.implicitWidth + 14
        height: 22
        radius: 3
        color: cell.active ? "#2d7ff7"
                           : (area.containsMouse ? "#3a3a41" : "transparent")

        Text {
            id: cap
            anchors.centerIn: parent
            text: cell.label
            font.pixelSize: 12
            color: "#f2f2f2"
        }

        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onReleased: if (containsMouse) cell.tapped()
        }

        ToolTip.text: cell.hint
        ToolTip.visible: area.containsMouse
        ToolTip.delay: 300
    }

    Rectangle {
        id: ratioBar

        // 透明白板（§5.4.5）没有"拖出一张新选区"这一步：那张画布自己的尺寸就是选区，
        // 所以那一排在它上面没有可锁的东西。`board_clear` 是这一条分支唯一的读数。
        visible: !win.view.board_clear

        color: "#f0232326"
        border.color: "#55555c"
        border.width: 1
        radius: 6

        anchors.horizontalCenter: parent.horizontalCenter
        y: 4
        width: ratioLine.implicitWidth + 12
        height: ratioLine.implicitHeight + 12

        Row {
            id: ratioLine
            anchors.centerIn: parent
            spacing: 4

            Repeater {
                model: win.view.ratioNames().split("|")

                delegate: RatioCell {
                    required property int index
                    required property var modelData

                    label: modelData
                    hint: win.view.ratioHints().split("|")[index]
                    active: win.view.ratio_index === index
                    onTapped: {
                        if (win.view.pickRatio(index)) {
                            // 表里那一格自此是"当前比例"的唯一主人：文本框里上一句自定义就此
                            // 作废，而不是留着一句看着像还在的话。
                            ratioField.text = ""
                            win.grabbed()
                        }
                    }
                }
            }

            // 「或输入自定义比例」（§5.3.8 第二步的后半）。
            TextField {
                id: ratioField

                width: 96
                height: 22
                font.pixelSize: 12
                placeholderText: "自定义 16:10"
                placeholderTextColor: "#6a6a70"
                // 绑 Rust 的那句话而不是让面板自己记：两张窗、一个状态，与上面那排芯片同理。
                text: win.view.ratio_spec
                // 解不出来时点灰而不是清掉用户打的字：那句是他写的，而 `ratio_live` 说的是
                // 这一句锁不上 —— 屏幕上得同时看得见这两件事。
                color: (win.view.ratio_index < 0 && !win.view.ratio_live) ? "#8a8a90" : "#f2f2f2"

                // 焦点是这条腿唯一真正的风险：`surface` 的 `focus: true` 是 Esc 与方向键的入口，
                // 而 Controls 的 `TextField` 默认吃鼠标焦点。点进文本框之后那两枚键就跟着走进
                // 文本框 —— 于是这一格自己接住 Esc（先收回打字，取消仍归窗口），并把焦点递回去。
                Keys.onEscapePressed: (event) => {
                    event.accepted = true
                    win.view.pickRatio(0)
                    // 清的是**这一格**：用户打过字之后 `text:` 那条绑定就没了（QML 里赋值会
                    // 摧毁绑定），所以光改 Rust 的那一格不会让这里的句子消失。
                    ratioField.text = ""
                    win.grabbed()
                    surface.forceActiveFocus()
                }
                onEditingFinished: win.view.applyRatioSpec(text)
            }
        }
    }

    // §5.3.6 第二步的「尺寸输入区域」：两格打字的地方，外加两句只读的话——现在多大、从哪一角扩展。
    //
    // 它长在**窗口**上、长在比例那一排下面，与比例那条同一枚理由：PRD 的顺序是"先有一张选区，
    // 再往里打尺寸"，而 `AnnotationToolbar` 那块面板说的是标注，不是选区自己。
    //
    // 两格文本框**不**绑 `hole_w`/`hole_h`。绑过一次、用户打一个字，那条绑定就没了（QML 里赋值
    // 摧毁绑定），此后拖拽不再更新它——屏幕上就留着一对说谎的数字，而第 162 项 ⑤ 那一格记过的
    // 正是这一族。所以"现在多大"由旁边那一行 Text 绑着说，文本框只说"我要改成多少"，提交成功
    // 就清空：一格只会有一个主人。
    Rectangle {
        id: sizeBar

        visible: win.view.selected && !win.view.board_clear

        // 被拒的那一次点灰而不是清字：那对数字是用户写的，而"没换掉尺寸"得看得见（与比例
        // 文本框的 `ratio_live` 同一形状）。
        property bool refused: false
        // 序号在 Rust（`size_anchor`），句子也在 Rust（`sizeAnchorNames()`）——这里只做一次下标。
        property string anchorName: win.view.sizeAnchorNames().split("|")[win.view.size_anchor]

        function commit() {
            const took = win.view.applySize(Number(widthField.text), Number(heightField.text))
            sizeBar.refused = !took
            if (took) {
                widthField.text = ""
                heightField.text = ""
                // 两张屏、一个状态：与比例那排同一枚 `grabbed`，另一扇窗的选区跟着重画。
                win.grabbed()
            }
        }

        function focusWidth() {
            widthField.forceActiveFocus()
        }

        color: "#f0232326"
        border.color: "#55555c"
        border.width: 1
        radius: 6

        anchors.horizontalCenter: parent.horizontalCenter
        y: ratioBar.y + ratioBar.height + 4
        width: sizeLine.implicitWidth + 12
        height: sizeLine.implicitHeight + 12

        Row {
            id: sizeLine
            anchors.centerIn: parent
            spacing: 4

            Text {
                // §5.3.7：这一行与选区框上那行同源（`size_w`/`size_h` + `win.sizeUnit`），所以
                // px 那一格上，"当前 1024×768 px"、框上那行、以及快速保存那句「（选区 1024×768
                // 像素）」是**同一个数说了三次**，而不是三处各算一遍各说各的。
                text: "当前 " + win.view.size_w + "×" + win.view.size_h + " " + win.sizeUnit
                font.pixelSize: 12
                color: "#9a9aa0"
                anchors.verticalCenter: parent.verticalCenter
            }

            TextField {
                id: widthField
                width: 52
                height: 22
                font.pixelSize: 12
                placeholderText: "宽"
                placeholderTextColor: "#6a6a70"
                color: sizeBar.refused ? "#e06a6a" : "#f2f2f2"
                validator: IntValidator { bottom: 1; top: 99999 }

                // 焦点这一条腿与比例文本框同一条风险：`surface` 的 `focus: true` 是 Esc 与
                // 方向键的入口。Esc 在这里只收回键盘，不清选区——`stepBack()` 那一档归窗口。
                Keys.onEscapePressed: (event) => {
                    event.accepted = true
                    widthField.text = ""
                    heightField.text = ""
                    sizeBar.refused = false
                    surface.forceActiveFocus()
                }
                onEditingFinished: sizeBar.commit()
            }

            Text {
                text: "×"
                font.pixelSize: 12
                color: "#9a9aa0"
                anchors.verticalCenter: parent.verticalCenter
            }

            TextField {
                id: heightField
                width: 52
                height: 22
                font.pixelSize: 12
                placeholderText: "高"
                placeholderTextColor: "#6a6a70"
                color: sizeBar.refused ? "#e06a6a" : "#f2f2f2"
                validator: IntValidator { bottom: 1; top: 99999 }
                Keys.onEscapePressed: (event) => {
                    event.accepted = true
                    widthField.text = ""
                    heightField.text = ""
                    sizeBar.refused = false
                    surface.forceActiveFocus()
                }
                onEditingFinished: sizeBar.commit()
            }

            // 第一条规则「应明确选区从哪个方向或锚点扩展」在屏幕上的那一句。它是**读数**不是
            // 按钮：锚点的主人只有拖拽那一个，做成一格可挑的芯片就多长一把与"我刚按住哪一角"
            // 打架的第二主人。
            Text {
                text: "从 " + sizeBar.anchorName + " 扩展"
                font.pixelSize: 12
                color: "#9a9aa0"
                anchors.verticalCenter: parent.verticalCenter

                ToolTip.text: "按住过哪一角，就打尺寸就从哪一角扩展；拖边与整块移动不改这一枚"
                ToolTip.visible: tipArea.containsMouse
                ToolTip.delay: 300

                MouseArea {
                    id: tipArea
                    anchors.fill: parent
                    hoverEnabled: true
                }
            }

            // §5.3.7 第一、二步「用户在…**尺寸输入区域**选择单位 / 在 px 与 dip 之间切换」：
            // 两枚芯片。清单与句子都在 Rust（`unitNames()` 是 `SIZE_UNITS` 那两枚单位的
            // `Unit::label`，`unitHints()` 是它们各自的悬停句），与上面那排比例芯片同一形状、
            // 同一枚 `RatioCell` —— 把 ["px","dip"] 抄进这一页，第 `i` 枚钮说的就不再是第 `i`
            // 个单位，而单位说错的代价是**数字本身**：屏幕上 683、裁下去 1024。
            //
            // 亮着的那一格跟着 Rust 交回的序号（`size_unit`，reload 之后重写），不跟着"刚才
            // 点了哪一枚"：两张屏各有一排芯片，在 1 号屏点 dip 之后 2 号屏那一排得跟着亮 ——
            // 单位与 §5.3.8 的锁、§5.3.6 的锚点一样住在 `MaskState`（桌面空间），不住在页里。
            Repeater {
                model: win.view.unitNames().split("|")

                delegate: RatioCell {
                    required property int index
                    required property var modelData

                    label: modelData
                    hint: win.view.unitHints().split("|")[index]
                    active: win.view.size_unit === index
                    onTapped: win.view.setUnit(index)
                }
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

    // §5.4.1 —— 放大镜这一块面板。放在**所有其他兄弟之后**，屏幕上永远在最上层：
    // `MouseArea` 已经 `anchors.fill: parent`，一张浮在选区上/工具栏上的小面板要能压过它。
    //
    // 只在**这扇窗真的被 hover** 且**配置开着放大镜**时可见。前者靠 `surface.containsMouse`
    // 报信（多屏时只光标所在那一扇窗收到 hover，别的窗 `containsMouse = false`，屏幕上
    // 就只有一份放大镜）；后者靠桥里的 `magnifier_show` —— 那一格就是 §5.19.3「是否显示
    // 放大镜」这一枚配置在这里的读者，关掉之后桥每次 hover 都回 `false`，页上什么都不画。
    //
    // 面板的**位置**跟着"这一趟真的去采的那一颗点"走：`x = magPanel.atX + offset`，其中
    // `atX/atY` 绑的是桥里的 `view.magnifier_anchor_x/y`（`crate::mask::MagnifierData::anchor_x`
    // 在本窗的读法）。以前这里读的是页上自己记的 `win.cursorX/cursorY`，而那一本账只有鼠标
    // 一条腿会写 —— §5.3.9 第 4 步要的正是"键盘把选区挪过一格之后，这一块面板说的是那条边"，
    // 于是页上那份副本被取消，位置与像素从此同一次装配（#281）。
    // 靠右边缘时翻到左边、靠下边缘时翻到上面，PRD §5.4.1 那句"靠近屏幕边缘时自动调整位置"。
    Rectangle {
        id: magPanel
        readonly property real offset: 18
        readonly property real atX: win.view.magnifier_anchor_x
        readonly property real atY: win.view.magnifier_anchor_y
        readonly property real dipW: win.view.magnifier_w / Math.max(1.0, win.view.dpr)
        readonly property real dipH: win.view.magnifier_h / Math.max(1.0, win.view.dpr)
        readonly property real rowH: 20
        readonly property real pad: 6
        // §5.19.3「放大镜外观」那四枚 `[appearance] magnifier_*` 键的读者：底、字（与描边那
        // 一笔）、边框宽度、圆角，四枚都从桥里那一趟 hover 来 —— 归一与"读不出来的串落回哪"
        // 只有一个作者（`crate::mask::MagnifierLook::from_appearance`），页上不自己解析颜色
        // （#281）。`!== ""` 那一臂只兜第一帧：第一次 `updateMagnifier` 落地之前那两枚
        // qproperty 还是空串，这里走画师原本写死的那两枚字面（与 `CustomCaptureWindow.qml`
        // 的 `accent_fill` 同一只口径，判据不许在这儿再写一遍）。
        readonly property color bgColor: win.view.magnifier_background !== ""
                                         ? win.view.magnifier_background : "#202020"
        readonly property color fgColor: win.view.magnifier_foreground !== ""
                                         ? win.view.magnifier_foreground : "#E0E0E0"
        // 屏幕翻转的两种情形：被采样的那一点太靠右 / 太靠下时，面板摆到它的另一侧。这里用的是
        // 这一扇窗自己的宽高（DIP，`win.width`/`win.height` = `win.view.win_w/win_h`）；
        // 两扇窗各自判各自，屏幕上就同时只有一块面板跑到边缘外那一档。
        readonly property real flipX: win.width - (atX + offset + dipW + pad) < 0
        readonly property real flipY: win.height - (atY + offset + dipH + rowH * 2 + pad * 2) < 0
        x: Math.max(0, flipX
                       ? atX - offset - dipW
                       : atX + offset)
        y: Math.max(0, flipY
                       ? atY - offset - (dipH + rowH * 2 + pad * 2)
                       : atY + offset)
        width: Math.max(dipW, 96) + pad * 2
        height: dipH + rowH * 2 + pad * 2
        visible: win.view.magnifier_show && surface.containsMouse
        // 这四行以前是四枚字面（`#202020` / `#808080` / `1` / `2`），档里那四枚键一个像素也
        // 穿不过去。描边那一笔吃的是**前景色**：档里没有第五枚颜色键 —— `magnifier_border`
        // 在 core 是一枚 u32 宽度、上下限住在 `Config::validate` 的 `clamp` 里 —— 而一块面板
        // 总要有笔画边，桥不替它挑一枚新色（`crate::mask::MagnifierLook::foreground` 头上
        // 是同一条口径）。
        color: bgColor
        border.color: fgColor
        border.width: win.view.magnifier_border
        radius: win.view.magnifier_corner_radius

        Column {
            anchors.fill: parent
            anchors.margins: magPanel.pad
            spacing: 2

            Item {
                width: magPanel.dipW
                height: magPanel.dipH
                Image {
                    id: magImage
                    anchors.fill: parent
                    cache: false
                    asynchronous: false
                    // 放大纹理**已经**是 nearest-neighbour 出来的（core 里 `magnifier()`
                    // 用 `fill_rect` 每格 `zoom × zoom` 铺过），页上再走一次 `Fast` 会把
                    // 那些硬边糊掉——所以这里是 `None`：不缩放，1:1 摆。
                    smooth: false
                    fillMode: Image.PreserveAspectFit
                    source: win.view.magnifier_key !== ""
                            ? "image://falconshot/" + win.view.magnifier_key
                            : ""
                }
                // §5.4.1 第 3 步的像素网格：overlay 一层 Canvas，把每颗**源**像素的框线
                // 画出。`magnifier_pixel_w` 是每边源格数、`zoom` 是每格放大几倍 —— 只有
                // 桥里 `magnifier_grid=true` 时才画（配置在 §5.4.1 那族里就叫"像素网格"）。
                Canvas {
                    id: magGrid
                    anchors.fill: parent
                    visible: win.view.magnifier_grid && win.view.magnifier_zoom > 1
                    onPaint: {
                        const ctx = getContext("2d")
                        ctx.reset()
                        const n = win.view.magnifier_pixel_w
                        if (n <= 0 || width <= 0 || height <= 0) {
                            return
                        }
                        const stepX = width / n
                        const stepY = height / n
                        ctx.strokeStyle = "#404040"
                        ctx.lineWidth = 1
                        for (let i = 1; i < n; i++) {
                            ctx.beginPath()
                            ctx.moveTo(i * stepX, 0)
                            ctx.lineTo(i * stepX, height)
                            ctx.stroke()
                            ctx.beginPath()
                            ctx.moveTo(0, i * stepY)
                            ctx.moveTo(width, i * stepY)
                            ctx.lineTo(width, i * stepY)
                            ctx.stroke()
                        }
                    }
                }
            }

            Row {
                height: magPanel.rowH
                spacing: 4
                // §5.4.1 第 3 步的**色板**：一小块按当前那颗源像素的 RGB 上色。
                // `has_color=false` 时（跨屏缝外、指针跑到快照之外）不画，句子里也不留
                // 一句"这一颗是黑色"——那本身是一个假的颜色。
                // 它外面那一圈 1px 灰环描的是**取到的那颗像素**、不是面板：§5.19.3 那四枚
                // 键没有一枚说的是这一块，所以它不跟着档走（跟着前景色走会让一枚深色样本
                // 在深底上彻底看不见）。
                Rectangle {
                    visible: win.view.magnifier_has_color
                    width: magPanel.rowH - 4
                    height: magPanel.rowH - 4
                    anchors.verticalCenter: parent.verticalCenter
                    color: "#" + ("000000"
                        + ((win.view.magnifier_rgb >>> 0) & 0xFFFFFF).toString(16))
                        .slice(-6).toUpperCase()
                    border.color: "#808080"
                    border.width: 1
                }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    // §5.4.1 第 3 步的**坐标**：`magnifier_coord` 空 = 配置里
                    // `magnifier_coordinates=false`，这一档页上不留空行；此时色板那格
                    // 仍然显示，颜色那句也仍然显示。
                    visible: win.view.magnifier_coord.length > 0
                    text: win.view.magnifier_coord
                    // §5.19.3 的「前景色」在这一行上是**字**：坐标那一句与下面那一句颜色，
                    // 就是面板上仅有的两处字，两行吃同一枚键（`magnifier_foreground`）。
                    color: magPanel.fgColor
                    font.pixelSize: 11
                }
            }

            Text {
                width: parent.width
                height: magPanel.rowH
                verticalAlignment: Text.AlignVCenter
                // §5.4.1 第 3 步的**颜色值**：字符串按 §5.19.3 那族里 `magnifier_color_format`
                // 装配（HEX/RGB/HSL 三族都走 `falcon_core::colors::format`），与按 C 复制
                // 进剪贴板的那一句**同一次装配**（同一份配置、同一次锁里交出的同一份
                // `MagnifierData`）。
                visible: win.view.magnifier_color.length > 0
                text: win.view.magnifier_color
                color: magPanel.fgColor
                font.pixelSize: 11
                elide: Text.ElideRight
            }
        }
    }
}
