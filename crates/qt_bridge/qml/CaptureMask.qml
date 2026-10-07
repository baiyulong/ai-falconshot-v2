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
            if (mouse.pressed) {
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

        onDoubleClicked: if (view.commitHole()) win.grabbed()

        Keys.onEscapePressed: (event) => {
            event.accept()
            // The ladder, not a property: the first Esc un-selects, the second one
            // cancels. `endMask` destroys these windows from the model, and doing
            // that while one of them is inside a key handler is a use-after-free
            // QML will not warn about - so it goes through the event loop.
            if (!view.stepBack()) {
                Qt.callLater(() => win.session.endMask())
            }
        }

        Keys.onReturnPressed: (event) => {
            event.accept()
            if (view.commitHole()) win.grabbed()
        }

        Keys.onEnterPressed: (event) => {
            event.accept()
            if (view.commitHole()) win.grabbed()
        }

        Keys.onLeftPressed: (event) => win.step(-1, 0, event.modifiers)
        Keys.onRightPressed: (event) => win.step(1, 0, event.modifiers)
        Keys.onUpPressed: (event) => win.step(0, -1, event.modifiers)
        Keys.onDownPressed: (event) => win.step(0, 1, event.modifiers)
    }
}
