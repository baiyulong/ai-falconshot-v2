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

    // The `--pin` gauge's plan, read once at completion and split by index. Empty in
    // every session nobody asked to measure, which is what keeps the leg below inert
    // for a user. Read in `Component.onCompleted` rather than in a property
    // initializer on purpose: `session` is a *required* property, and a binding that
    // calls a method on it can be evaluated before the delegate's creator has
    // assigned it - a TypeError that takes the whole document down (§8-R14).
    //
    // The fields are read by index, and the order is `pin_check::plan_line`'s:
    // 0 目录, 1 qml.png, 2 qml.jpg, 3 never.png, 4 贴图 id, 5 一个不存在的 id,
    // 6 延迟 ms, 7 对话框默认名, 8 推开尝试上限.
    property var gauge: []

    // True only between the moment the dialog leg asked for a real dialog and the moment
    // it stopped watching. The gauge reads the *first* line it was given under a key, so
    // a key two phases could both write is a row graded on the wrong phase's answer -
    // and the phase above calls `accept()` directly, which is a completion this run did
    // not press. This property is what keeps `dlg-accepted` a click's word.
    property bool dlgLeg: false

    // How many times phase 2 may ask the shell for its button before it stops asking,
    // and how many times it has actually asked. The first run answered `dialogs=0` 400 ms
    // after `open()` — Qt had marked the dialog visible while Win32 had not made the
    // window yet — so the press waits for the native window instead of failing on the
    // first turn. Three runs put that window's arrival at the third wait, at the fourth,
    // and past the fourth, which is the same measurement passing and failing on a count
    // alone; the count therefore comes from the plan rather than from a second copy here.
    // The cap still has to exist: a dialog the shell never shows must end the run with a
    // `pushed=no`, not with an endless poll.
    readonly property int pushTriesMax: Number(win.gauge[8]) || 10
    property int pushTries: 0

    // Every attempt's own answer, kept apart so the count is not the only evidence: which
    // window the shell had (or did not have) at each wait is what says whether the
    // `pushed=yes` below was earned by waiting or given for free.
    property string pushTriesLog: ""

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
        win.gauge = win.session.pinGaugePlan().split("|")
    }

    // Style bits are the one thing a property change cannot carry by itself.
    function restyle() {
        shim.applyStyle(win, view.topmost, view.click_through)
    }

    // One line to the `--pin` gauge, in the `key|value` shape `pin_check` grades.
    function gaugeSay(key, value) {
        win.session.pinGaugeReport(key + "|" + value)
    }

    // A local path as the text a `QUrl` arrives in. Spaces become `%20` because that is
    // the form a dialog hands over, and it is the reason `save::local_path` decodes: a
    // name that reaches the disk with a literal `%20` in it is a file the user cannot
    // open.
    function dlgUrl(local) {
        return "file:///" + String(local).replace(/\\/g, "/").replace(/ /g, "%20")
    }

    // Walk this window's two output items the way the menu's own handlers do, and
    // report what each one answered.
    //
    // The sentences come back from `session.status` for the dialog leg, because
    // `onAccepted` drops `savePin`'s return value and §5.9.13 says the string has to
    // reach the screen anyway - so the gauge reads the user-visible line rather than
    // a log line, which is the only way an English clause inside a Chinese prefix
    // (`保存失败：pin 3 has never been drawn`, what §10 第 37 项 ① found) shows up as
    // the thing a user would actually have read.
    //
    // This first phase does not open a dialog. Everything after it that does runs in
    // its own timed phase, because a door has to be *on the screen* while something
    // presses its button and that cannot be fitted into this event-loop turn.
    function runGauge() {
        gaugeSay("menu", saveAsItem.text)
        gaugeSay("filters", saveAs.nameFilters.join(";;"))
        // The members are read rather than assumed: `selectedNameFilter` came back as
        // a `QQuickFileNameFilter` object in the first P37 run, and the one probe that
        // enumerated it found `name`, `extensions`, `globs` and `index` - empty here,
        // because a dialog that has never been shown has not selected a filter yet.
        win.readFilter("filter")
        gaugeSay("before", String(saveAs.selectedFile))
        // The question this asks, rather than the answer it assumes: can a
        // `FileDialog` that was never `open()`ed still fire its own `accepted`? The
        // row grades what happened either way, and the two writes below are the same
        // two statements the menu items make, so the door is measured whether or not
        // a dialog that has never been shown is willing to accept.
        const url = win.dlgUrl(win.gauge[1])
        saveAs.currentFile = url
        gaugeSay("current", String(saveAs.currentFile))
        gaugeSay("call-accept", win.session.status)
        saveAs.accept()
        gaugeSay("call-after", win.session.status)
        // The PNG leg goes through the URL text, which is exactly what `onAccepted`
        // passes; the JPG leg keeps the plain path, so both spellings of the name are
        // graded by the same run.
        gaugeSay("png-url", url)
        gaugeSay("png", win.session.savePin(win.pinId, url, ""))
        gaugeSay("jpg", win.session.savePin(win.pinId, win.gauge[2], ""))
        // The two legs that have to fail: an id the set never handed out, so the
        // door answers before it reaches the clipboard or the disk.
        gaugeSay("never-copy", win.session.copyPin(Number(win.gauge[5])))
        gaugeSay("never-save", win.session.savePin(Number(win.gauge[5]), win.gauge[3], ""))
        // §10 第 41 项 ②: the refusal above is this door's click-facing before-state, so its
        // text is read back here and `pin_check` grades the pair the way the mask door has
        // since P39. `session.status` is one slot, so any leg that writes it between this
        // line and `accept-was` replaces the product's sentence with a leg's own - which is
        // exactly what P40's JPG arm did to this door, and why that arm no longer runs here
        // (it is `jpgArmLeg` below, past the click). The schedule order is a judged row now,
        // so this placement has a reader rather than only a comment.
        gaugeSay("pre-save", win.session.status)
        win.openDialogLeg()
    }

    // The dialog leg, phase 1 of 3: hand the native dialog a folder and a name, then
    // open it for real. §10 第 38 项 ① is the reason this exists rather than another
    // `accept()` above - until P38 the gauge had proved what `onAccepted` does with the
    // text it is handed, never that the door opens the way a user's click opens it.
    //
    // `currentFolder` is set and the name is a bare stem, both measured necessities: a
    // `currentFile` naming a folder that is not there makes the dialog answer `accepted`
    // about a millisecond after `open()` and the process dies (P38's probe C,
    // `exit=-1073741819`), and a name without a folder would be resolved against
    // whatever the user's Documents is - which is not a place this run gets to write.
    //
    // The stem goes in through `currentFile` rather than `fileName`, because there is no
    // `fileName` to write: P38's probe F found the assignment answers
    // `Cannot assign to non-existent property "fileName"` and the read-back is
    // `undefined`, exactly as qmllint's `[missing-property]` said. A bare name still
    // reaches the shell - `currentFile` set to a path without an extension came back from
    // a real accept with the selected filter's extension on it (probe E), which is the
    // reading the gauge wants.
    function openDialogLeg() {
        const stem = String(win.gauge[0]) + "/" + String(win.gauge[7])
        saveAs.currentFolder = win.dlgUrl(String(win.gauge[0]) + "/")
        saveAs.currentFile = win.dlgUrl(stem)
        gaugeSay("dlg-folder", String(saveAs.currentFolder))
        gaugeSay("dlg-name", String(saveAs.currentFile))
        win.dlgLeg = true
        saveAs.open()
        // Read in the same turn that asked for it, and again from the phase that
        // presses the button: whether `open()` comes back with the dialog already up
        // is a number about this Qt, not a promise to a user, so the first is reported
        // and the second is graded.
        gaugeSay("dlg-open", String(saveAs.visible))
        dlgPush.restart()
    }

    // Phase 2: press the dialog's own default button - but only once it is actually on
    // the screen, which is not the same moment as Qt saying it opened. The first P38 run
    // answered `dialogs=0 mine_tops=12 mine_visible=1`, and the one visible top-level of
    // this process was the pin window itself: 400 ms after `open()` the shell had not
    // made its window yet. So the attempt repeats until the window is there, up to a cap.
    //
    // One attempt's answer is what the gauge grades, and it is the last one: the earlier
    // waits are the same question asked too early, and their full text travels under a key
    // of its own so that the count and the reasons stay readable. `accept-was` and the
    // other first-attempt readings are taken once, because they are the same members on
    // every attempt and the gauge grades the pair across the click rather than across the
    // waiting - an `onAccepted` that ran leaves a 已保存到 sentence behind, and nothing
    // else in this process writes one at this moment.
    function pushDialogLeg() {
        win.pushTries += 1
        if (win.pushTries === 1) {
            gaugeSay("dlg-shown", String(saveAs.visible))
            win.readFilter("dlg-shown-filter")
            gaugeSay("accept-was", win.session.status)
        }
        const said = win.session.pushDialogButton()
        win.pushTriesLog += (win.pushTries === 1 ? "" : " ~ ") + String(said)
        if (said.startsWith("pushed=yes") || win.pushTries >= win.pushTriesMax) {
            gaugeSay("dlg-push", said)
            gaugeSay("dlg-push-tries", String(win.pushTries))
            gaugeSay("dlg-push-log", win.pushTriesLog)
            dlgTail.restart()
        } else {
            dlgPush.restart()
        }
    }

    // Phase 3: what the click left behind.
    //
    // The dismiss is guarded on `visible` on purpose. A door the click already closed
    // is gone, and calling `reject()` on it would be a second completion this run did
    // not measure - P38's whole claim is that the tail readings come from a click.
    function finishDialogLeg() {
        gaugeSay("accept", win.session.status)
        win.readFilter("dlg-filter")
        gaugeSay("dlg-after", String(saveAs.visible))
        if (saveAs.visible) {
            gaugeSay("dlg-dismiss", "reject")
            saveAs.reject()
        }
        // Closed last, after the reject above: that one is this phase's own dismissal and
        // is meant to be reported. From here nothing else may answer under the click's
        // keys.
        win.dlgLeg = false
        win.jpgArmLeg()
        gaugeSay("done", win.gauge[0])
    }

    // §10 第 40 项 ②, at this door: a name with no extension takes the format of the arm
    // the dialog was showing, so the leg hands the door one arm's own text, read off
    // `nameFilters` because the *selected* arm is read-only (P38's probe E found the
    // assignment). The JPG arm is the one the configured `[output] format` would never
    // pick on this machine, so it is what tells a forwarded filter from one dropped at
    // the boundary - and the boundary is the question, since the legs that call the door
    // with a name already carrying an extension cannot see the difference either, and a
    // real dialog click cannot (the shell suffixes a bare name before `onAccepted` reads it).
    //
    // It runs here, after the click and its tail readings, for the reason §10 第 41 项 ②
    // registered: this leg writes `session.status`, and the graded pair above needs that
    // slot to still hold the product's own refusal when the click arrives. P40 had it
    // first in the schedule, which left `pin dialog accept` comparing two file names
    // instead of two sentences. The mask door solved the same problem by running its arm
    // *above* its refusal; this door's refusal is a leg of its own, further up, so the
    // arm goes below everything the click is measured on.
    function jpgArmLeg() {
        const bareName = win.dlgUrl(String(win.gauge[0]) + "/" + win.gauge[7] + "-f")
        const arm = win.filterExt(1)
        gaugeSay("f-jpg", arm + "|" + win.session.savePin(win.pinId, bareName, arm))
    }

    // `selectedNameFilter` three times, because it is read-only (assigning to it is a
    // hard `TypeError`, P38's probe E) and the three readings are the story: empty
    // before any dialog, the shell's default while one is up, and whatever the accepted
    // file's extension says afterwards. The `|` inside the value is what the gauge's
    // `name|extensions|index` shape already uses.
    function readFilter(key) {
        const chosen = saveAs.selectedNameFilter
        gaugeSay(key, chosen.name + "|" + chosen.extensions.join(",") + "|" + chosen.index)
    }

    // One arm of the menu, in the shape the selected arm arrives in: `JPG 图片 (*.jpg)` →
    // `*.jpg`. Same reading as the toolbar's function of this name, for the same reason -
    // an arm cannot be pointed at, so a leg that wants to ask what one of them does reads
    // the list.
    function filterExt(i) {
        const text = String(saveAs.nameFilters[i])
        const from = text.lastIndexOf("(") + 1
        const to = text.lastIndexOf(")")
        return to > from ? text.substring(from, to) : ""
    }

    Timer {
        id: gaugeLeg
        // The delay comes from the plan (`pin_check::LEG_DELAY_MS`) rather than being
        // written twice, and the window's native side is what it waits for: Win32 has
        // no frame for this window until the event loop has exposed it, so a leg run
        // straight from `onCompleted` would report a missing window as a missing
        // feature.
        interval: Number(win.gauge[6]) || 400
        repeat: false
        // Stopped for every delegate that is not the planned pin, and for every
        // ordinary session: `pinId` is -1 until `assign` and a plan with no gauge run
        // is one field long, not eight.
        running: win.gauge.length === 9 && String(win.gauge[4]) === String(win.pinId)
        onTriggered: win.runGauge()
    }

    // The two waits of the dialog leg, same interval from the same plan. The first is
    // the native dialog's own loop coming up after `open()`; the second is the
    // `onAccepted` tail, which cannot have run in the turn that pressed the button -
    // the shell unwinds its modal loop before Qt gets the signal.
    Timer {
        id: dlgPush
        interval: Number(win.gauge[6]) || 400
        repeat: false
        onTriggered: win.pushDialogLeg()
    }

    Timer {
        id: dlgTail
        interval: Number(win.gauge[6]) || 400
        repeat: false
        onTriggered: win.finishDialogLeg()
    }

    // The gauge's witness that the signal fired, taken from outside the door so the
    // `onAccepted` below stays the one statement §5.9.17 asks for. It is not the whole
    // claim: this object cannot write a file, so the bytes at `dialog.png` are what say
    // the product's own handler ran.
    //
    // `dlgLeg` is the half of the condition that says *which* completion is being
    // witnessed. Without it the direct `accept()` in the first phase could answer under
    // this key, and since the gauge reads the first line a key produced, the click's own
    // row would be graded on a sentence the gauge wrote itself.
    Connections {
        target: saveAs
        function onAccepted() {
            if (win.gauge.length === 9 && win.dlgLeg) win.gaugeSay("dlg-accepted", String(saveAs.selectedFile))
        }
        function onRejected() {
            if (win.gauge.length === 9 && win.dlgLeg) win.gaugeSay("dlg-rejected", String(saveAs.selectedFile))
        }
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
        // `id` for the `--pin` gauge, which reports the text it is shown rather than
        // assuming the menu still says what §5.9.17 calls it.
        MenuItem { id: saveAsItem; text: "另存为…"; onTriggered: saveAs.open() }

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
    // formats this app writes from a pin - and since P40 it is a second time over, because
    // §5.5.5 step 3 counts 格式 among the three things chosen and the arm therefore decides
    // the format of a name typed *without* one. Both halves travel from this one list.
    //
    // The name is handed over as the URL text rather than a local path, because this
    // Qt gives JS no way to convert one: `selectedFile.toLocalFile()` measured
    // `undefined` - a TypeError on every accept, P37 - and `save::local_path` is now
    // the single place that reads a `file:///…` back into the path it names. The arm is
    // handed over flattened the same way `readFilter` reports it, because
    // `selectedNameFilter` is an object and `String()` of one is not the text the owner
    // can read.
    FileDialog {
        id: saveAs
        title: "另存为"
        fileMode: FileDialog.SaveFile
        nameFilters: ["PNG 图片 (*.png)", "JPG 图片 (*.jpg)", "BMP 图片 (*.bmp)"]
        onAccepted: {
            win.session.savePin(
                win.pinId,
                String(saveAs.selectedFile),
                saveAs.selectedNameFilter.extensions.join(",")
            )
        }
    }
}
