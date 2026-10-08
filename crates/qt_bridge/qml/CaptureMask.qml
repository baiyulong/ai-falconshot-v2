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

        // §5.7.19's 撤销 / 重做.
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
            if ((event.modifiers & Qt.ControlModifier) === 0) {
                return
            }
            const shifted = (event.modifiers & Qt.ShiftModifier) !== 0
            const undo = event.key === Qt.Key_Z && !shifted
            const redo = event.key === Qt.Key_Y || (event.key === Qt.Key_Z && shifted)
            if (!undo && !redo) {
                return
            }
            event.accepted = true
            if (redo ? view.redoStep() : view.undoStep()) {
                win.grabbed()
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

        // The same propagation the pointer does: undo, redo and 全清 change a
        // desktop-space picture that both windows draw.
        onGrabbed: win.grabbed()
    }
}
