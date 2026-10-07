import QtQuick
import QtQuick.Window
import QtQuick.Controls
import QtQuick.Dialogs
import dev.falconshot 1.0

// §5.9 as a window. It owns no picture and decides nothing: every gesture is
// handed to a PinView, which is a thin cover over the Rust state machine, and
// the geometry it displays is read back from there. That is what keeps the drag
// and wheel-zoom path from re-encoding a single pixel (plan rule 4).
Window {
    id: win

    required property PinShim shim
    required property Session session

    // Not `required`: a QQmlInstantiator delegate is created before the slot it
    // fills is known, and a required property that is missing aborts creation.
    // -1 is a pin id the state machine never hands out, so an unassigned window
    // reads as not-live rather than as somebody else's pin.
    property int pinId: -1

    // The view is filled by `assign`, not by a binding: the Instantiator tells
    // this window which pin it is *after* it is created, and a `pin_id` binding
    // would race its own change handler for who updates first.
    readonly property PinView view: PinView {}
    readonly property real dpr: Screen.devicePixelRatio

    function assign(id) {
        win.pinId = id
        win.view.pin_id = id
        win.view.reload()
    }

    color: "transparent"

    // Frameless, no taskbar entry, and optionally top-most - all three as Qt
    // window flags rather than hand-set styles, so Qt owns the native side.
    // `applyStyle` below writes the same bits and the probe reads them back.
    flags: Qt.Window | Qt.FramelessWindowHint | Qt.Tool
           | (view.topmost ? Qt.WindowStaysOnTopHint : 0)
           | (view.click_through ? Qt.WindowTransparentForInput : 0)

    // A closed pin has no window, and neither does a thumbnail that narrowed the
    // shown region to a line - Qt would round that to zero and the pin would
    // look lost rather than small.
    visible: view.live && view.size_w > 1 && view.size_h > 1

    // The state machine is in physical pixels; a QML window is sized in logical
    // ones. Dividing here is the only place that translation happens, and the
    // probe measures whether the native window agrees.
    width: view.size_w / win.dpr
    height: view.size_h / win.dpr
    x: view.pos_x / win.dpr
    y: view.pos_y / win.dpr
    opacity: view.opacity / 100.0

    // No `onPinIdChanged`: `pin_id` is not a binding either, and a reload fired
    // from here would read the *previous* pin's state, because `assign` writes
    // `pinId` before it writes `view.pin_id`.
    Component.onCompleted: {
        // The provider has to be on the engine this window renders with, and it
        // is C++-only, so it is installed from here rather than from main().
        shim.install(win)
        view.reload()
        shim.applyStyle(win, view.topmost, view.click_through)
    }

    // Style bits are the one thing a property change cannot carry by itself.
    function restyle() {
        shim.applyStyle(win, view.topmost, view.click_through)
    }

    Connections {
        target: win.view
        function onTopmostChanged() { win.restyle() }
        function onClick_throughChanged() { win.restyle() }
    }

    // The picture. `revision` is inside the URL, so a content change is the only
    // thing that makes Qt ask the provider for a new image; drag and zoom never
    // touch it. `cache: false` because the ids are recycled by the revision, not
    // by the URL.
    Image {
        id: picture
        anchors.fill: parent
        fillMode: Image.Stretch
        smooth: win.view.smooth
        cache: false
        // No key yet means no request yet: an Image that asks the provider before
        // Component.onCompleted has installed it logs a warning and paints nothing.
        source: win.view.image_key !== ""
                ? "image://falconshot/" + win.view.image_key
                : ""
    }

    // §5.9.11 step 2: the right-drag selection, shown while it is being drawn.
    Rectangle {
        id: band
        visible: dragArea.bandOn
        color: "#22ffffff"
        border.color: "#ccffffff"
        border.width: 1
    }

    MouseArea {
        id: dragArea
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        // The drag deltas are kept in global coordinates on purpose: the window
        // moves out from under the cursor, so a local delta would lose the
        // remainder of every pixel and a long drag would drift.
        property real lastX
        property real lastY
        property real startX
        property real startY
        property bool bandOn: false

        // `globalX`/`globalY` were the Qt 5 names and are not on this type - qmllint:
        // `Member "globalX" not found on type "QQuickMouseEvent"`. Its members are x,
        // y, button, buttons, modifiers, source, isClick, wasHeld, accepted, flags, so
        // the global point comes from `Item.mapToGlobal(x, y)`, which answers in the
        // same global device-independent units the two `last*` properties hold.
        function globalAt(event) {
            return dragArea.mapToGlobal(event.x, event.y)
        }

        onPressed: (event) => {
            win.view.reload()
            const g = globalAt(event)
            dragArea.lastX = g.x
            dragArea.lastY = g.y
            dragArea.startX = event.x
            dragArea.startY = event.y
            dragArea.bandOn = event.button === Qt.RightButton
            band.x = event.x
            band.y = event.y
            band.width = 1
            band.height = 1
        }

        onPositionChanged: (event) => {
            if (dragArea.bandOn && (event.buttons & Qt.RightButton)) {
                band.x = Math.min(dragArea.startX, event.x)
                band.y = Math.min(dragArea.startY, event.y)
                band.width = Math.abs(event.x - dragArea.startX)
                band.height = Math.abs(event.y - dragArea.startY)
                return
            }
            if (event.buttons & Qt.LeftButton) {
                // §5.9.1 - the floor that keeps a pin findable is the state
                // machine's, so a drag towards off-screen comes back clamped.
                const g = globalAt(event)
                var dx = Math.round((g.x - dragArea.lastX) * win.dpr)
                var dy = Math.round((g.y - dragArea.lastY) * win.dpr)
                if (dx !== 0 || dy !== 0) {
                    win.view.dragMove(dx, dy)
                    dragArea.lastX += dx / win.dpr
                    dragArea.lastY += dy / win.dpr
                }
            }
        }

        onReleased: (event) => {
            if (!dragArea.bandOn) {
                return
            }
            dragArea.bandOn = false
            var travelled = Math.abs(event.x - dragArea.startX)
                            + Math.abs(event.y - dragArea.startY)
            if (travelled < 4) {
                // No drag means the menu (§5.9.12 "打开贴图菜单").
                pinMenu.popup()
                return
            }
            // The selection is handed over in desktop pixels, the same space a
            // crop arrives in; what it maps to inside the picture is decided in
            // Rust.
            var x = win.view.pos_x + Math.round(band.x * win.dpr)
            var y = win.view.pos_y + Math.round(band.y * win.dpr)
            var w = Math.round(band.width * win.dpr)
            var h = Math.round(band.height * win.dpr)
            win.view.freeThumbnailWindow(x, y, w, h)
        }

        // §5.9.2 - the pixel under the cursor is the one that stays put.
        // §5.9.4 - the same wheel with Ctrl held is opacity instead of zoom, and
        // how low it may go is the state machine's floor, not this file's.
        onWheel: (wheel) => {
            if (wheel.modifiers & Qt.ControlModifier) {
                win.view.opacityStep(wheel.angleDelta.y > 0)
            } else {
                win.view.wheelZoom(wheel.angleDelta.y > 0,
                                   Math.round(wheel.x * win.dpr),
                                   Math.round(wheel.y * win.dpr))
            }
            wheel.accepted = true
        }

        // §5.9.10 - Shift+double-click is the shortcut the requirement names.
        // Leaving a thumbnail mode is the menu's, or another press of the same
        // shortcut for the fixed one; a plain double-click is not spoken for.
        onDoubleClicked: (event) => {
            if (event.modifiers & Qt.ShiftModifier) {
                win.view.toggleThumbnail()
            }
        }
    }

    // The single-pin menu (§5.9.12 to §5.9.18). §5.10's batch menu acts on a
    // selection instead of on this window and is Phase 2.
    Menu {
        id: pinMenu

        MenuItem { text: "复制图片"; onTriggered: win.session.copyPin(win.pinId) }
        MenuItem { text: "另存为…"; onTriggered: saveAs.open() }

        MenuSeparator {}

        MenuItem { text: "放大"; onTriggered: win.view.zoomStep(true) }
        MenuItem { text: "缩小"; onTriggered: win.view.zoomStep(false) }
        MenuItem { text: "重置缩放"; onTriggered: win.view.resetZoom() }

        MenuSeparator {}

        MenuItem { text: "顺时针旋转"; onTriggered: win.view.rotateCw() }
        MenuItem { text: "逆时针旋转"; onTriggered: win.view.rotateCcw() }
        MenuItem { text: "水平翻转"; onTriggered: win.view.flipH() }
        MenuItem { text: "垂直翻转"; onTriggered: win.view.flipV() }

        MenuSeparator {}

        // Grayscale and invert are toggles of the picture, and the picture's own
        // state is not a window property - so they read as actions, not checks.
        MenuItem { text: "灰度"; onTriggered: win.view.toggleGrayscale() }
        MenuItem { text: "反色"; onTriggered: win.view.toggleInvert() }
        MenuItem { text: "平滑缩放"; checkable: true; checked: win.view.smooth
                   onTriggered: win.view.applySmooth(checked) }

        MenuSeparator {}

        MenuItem { text: "固定缩略图"; onTriggered: win.view.toggleThumbnail() }
        MenuItem { text: "退出缩略图"; onTriggered: win.view.exitThumbnail() }
        MenuItem { text: "撤销裁剪"; onTriggered: win.view.undoCrop() }
        MenuItem { text: "恢复原图"; onTriggered: win.view.restoreOriginal() }

        MenuSeparator {}

        MenuItem { text: "更透明"; onTriggered: win.view.opacityStep(false) }
        MenuItem { text: "更实"; onTriggered: win.view.opacityStep(true) }
        // §5.9.4 asks for a one-key way back to fully opaque, which the two steps
        // above are not: from the floor they need ten presses.
        MenuItem { text: "不透明度 100%"; onTriggered: win.view.applyOpacity(100) }
        MenuItem { text: "始终置顶"; checkable: true; checked: win.view.topmost
                   onTriggered: win.view.applyTopmost(checked) }
        MenuItem { text: "鼠标穿透"; checkable: true; checked: win.view.click_through
                   onTriggered: win.view.applyClickThrough(checked) }

        MenuSeparator {}

        MenuItem { text: "重置视图"; onTriggered: win.view.resetView() }
        MenuItem { text: "关闭"; onTriggered: win.session.closePin(win.pinId) }
    }

    // §5.9.17. The dialog decides the name and the extension; `savePin` reads the
    // extension to pick the encoder, so the filter order below is also the list of
    // formats this app writes from a pin.
    FileDialog {
        id: saveAs
        title: "另存为"
        fileMode: FileDialog.SaveFile
        nameFilters: ["PNG 图片 (*.png)", "JPG 图片 (*.jpg)", "BMP 图片 (*.bmp)"]
        onAccepted: win.session.savePin(win.pinId, saveAs.selectedFile.toLocalFile())
    }
}
