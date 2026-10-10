import QtQuick
import QtQuick.Dialogs
import dev.falconshot 1.0

// §5.7's annotation toolbar, as a panel that floats next to the selection.
//
// It owns no state at all. Every tick, every highlighted chip and every greyed-out
// action is read off `MaskView`, because the layer behind it remembers a style per
// tool (§5.7.1) - a toolbar that kept its own idea of the current pen would be
// showing one colour while Rust drew with another. So the buttons only ever *call*,
// and the picture they show afterwards comes back through `view.reload()`.
//
// Plain `Rectangle`/`Text` items rather than `Controls.Button`: a row of tool buttons
// whose count grows with `TOOLS`, and the width of each one has to be its own label.
// A styled Button adds padding, an implicit size and a background that differs with
// the platform style, none of which is asked for here.
Rectangle {
    id: bar

    required property MaskView view
    required property Session session

    /// The panel's own parent - the mask window's content item. Bounds come from
    /// here rather than from a passed-in `win`, so the bar can be dropped into any
    /// screen's window and still know how big that window is.
    readonly property Item host: parent

    /// §5.7.21's 快速切换 row. Six, because that is how many fit beside the pens
    /// without the panel outgrowing a small screen; the full custom palette is
    /// §5.7.22 and lives behind a later round.
    readonly property var palette: [
        0xff3b30, 0xff9500, 0x34c759, 0x0a84ff, 0xaf52de, 0x1c1c1e
    ]

    /// The three named widths (§5.7.20 step 1), in device pixels. The wheel over them
    /// is step 3's fine-tune, so this row is a shortcut and not the only way.
    readonly property var pens: [4, 8, 16]

    /// §5.7.13 step 5's 放大倍数, same shape: three named values plus a wheel that
    /// reaches the rest of the range Rust clamps to.
    readonly property var magnifications: [100, 200, 400]

    /// §5.7.11 step 4's 字号, in device pixels - the middle cell is the model's own
    /// default, so the row says what the pen actually is the first time it appears.
    readonly property var font_sizes: [12, 18, 28]

    /// Whether the last thing that left the program - 复制 or 保存 - got through. Kept
    /// for one thing only: the colour of the sentence Rust wrote beside it. It is this
    /// panel's own answer to the call this panel made, not a second copy of a Rust fact,
    /// which is why it is a property of the bar rather than a `MaskView` one.
    property bool delivered: false

    /// Set by the 存完再钉图 cell and read once by [`Self.saveAs`]'s `onAccepted`, which
    /// clears it. The dialog is the same door for both acts - §10 第 43 项 ① asks that the
    /// pin be made *out of the file that landed*, and the file's name is only known after
    /// the user has typed it - so the flag carries the second half across that gap rather
    /// than a second dialog being built. It is cleared on rejection too: a cancelled 另存为
    /// must not leave the next plain 保存 quietly pinning.
    property bool pinAfterSave: false

    /// The `--ink` gauge's plan for this door, read by index in
    /// [`crate::annotate_check::mask_gauge_plan`]'s order: 0 目录, 1 对话框默认名,
    /// 2 延迟 ms, 3 推开尝试上限, 4 窗 index. One field when nobody asked for the leg,
    /// which is what keeps it out of a product session and out of a `--mask` run.
    property var gauge: []

    /// True only between the moment the leg asked for a real dialog and the moment it
    /// stopped watching. The gauge reads the *first* line it was given under a key, so this
    /// is what keeps `dlg-accepted` the click's own word rather than a reading from some
    /// other completion of this door.
    property bool dlgLeg: false

    /// How many times the leg may ask the shell for its button, and how many times it has.
    /// The count comes from the plan because the pin's three runs put that window's arrival
    /// anywhere between the third wait and past the fourth, and a cap written in two
    /// languages is two numbers that can disagree.
    readonly property int pushTriesMax: Number(bar.gauge[3]) || 10
    property int pushTries: 0

    /// Every attempt's own answer, kept apart so the count is not the only evidence.
    property string pushTriesLog: ""

    color: "#f0232326"
    border.color: "#55555c"
    border.width: 1
    radius: 6

    // Only once there is something to annotate: the pen clips to the selection, so
    // a toolbar before the first drag offers a stroke that cannot be seen.
    visible: view.selected

    width: body.implicitWidth + 12
    height: body.implicitHeight + 12

    // Under the selection, centred on it, and above it when the bottom edge of the
    // screen leaves no room. Clamped on both axes, because a selection that touches
    // a screen edge is the common case, not the odd one.
    x: {
        const cx = view.hole_x + view.hole_w / 2
        return Math.max(4, Math.min(cx - width / 2, host.width - width - 4))
    }
    y: {
        const below = view.hole_y + view.hole_h + 10
        if (below + height <= host.height - 4) {
            return below
        }
        return Math.max(4, view.hole_y - height - 10)
    }

    /// The mask window has to reload the *other* screen's window after a click that
    /// changed Rust's answer - the selection and the ink are one desktop-space state
    /// drawn by two windows. Same reason `CaptureMask` has its own `grabbed`.
    signal grabbed()

    // The plan is read once, at the moment this toolbar comes into being: `plant` has
    // already run - it has to, because the freeze happens before any window exists - so a
    // session that asked for the leg has an answer here, and every other session gets the
    // one field that turns the leg's timer off.
    Component.onCompleted: bar.gauge = bar.session.maskGaugePlan().split("|")

    function commit() {
        if (view.commitHole()) {
            bar.grabbed()
        }
    }

    function cancel() {
        // Through the event loop, exactly as the Esc key does: `endMask` destroys
        // these windows from the model, and doing that from inside a handler that is
        // still running on one of them is a use-after-free QML will not warn about.
        Qt.callLater(() => session.endMask())
    }

    /// §5.5.2's 复制, from this cell or from `Ctrl+C` - both come through here, so
    /// there is one gesture with one owner rather than two paths that can disagree
    /// about what a copy does to the flow.
    ///
    /// It does not close the mask, whatever the outcome: see
    /// [`crate::mask::MaskState::copy_out`]. `grabbed()` fires either way, because the
    /// sentence is on the one mask state both windows read and the toolbar has to
    /// re-read it after the write rather than before.
    function copy() {
        bar.delivered = bar.view.copySelection()
        bar.grabbed()
    }

    /// §5.5.5's 另存为, from this cell or from `Ctrl+S`: the dialog is opened, and the
    /// answer comes back through [`Self.delivered`] when the user names a file.
    ///
    /// Nothing is decided here about *which* format - the extension the user typed is
    /// the format, and a name without one becomes the configured default in Rust. The
    /// prompt that §5.4.5 and §5.5.5 ask for when that format cannot hold the
    /// selection's transparency is Rust's sentence too, for the same reason the size of
    /// a copy is: two panels that each read the picture and each decided what to say
    /// about it would say two different things.
    ///
    /// `saveAs` is named unqualified because it is a document-scope id, not a member of
    /// this Rectangle: `bar.saveAs` is a name qmllint reports as
    /// `Member "saveAs" not found on type "Rectangle"`, and every other id reference in
    /// this file is written the short way for the same reason.
    function save() {
        saveAs.open()
    }

    /// §5.5.4's 快速保存, from this cell or from `Ctrl+Shift+S`: the same stitched picture
    /// 保存 writes, into the file `[output]` names, with no dialog at all.
    ///
    /// Nothing is asked of the user, which is the feature rather than a shortcut for it -
    /// §5.5.4 step 3 is "无需打开文件对话框，直接保存截图". What the settings have to have
    /// is step 1's 目录 and 格式, and a machine that cannot name either one answers in
    /// Rust's sentence instead of writing somewhere nobody chose.
    function quickSave() {
        bar.delivered = bar.view.quickSave()
        bar.grabbed()
    }

    /// PRD 433 / 459's 完成后可…贴图, in the one order that makes the floating picture an
    /// artifact rather than a second rendering: 另存为 first, then a 贴图 made from the file
    /// that landed (§10 第 43 项 ①).
    ///
    /// The dialog is opened by [`Self.pinAfterSave`] rather than by a second door, because
    /// the name the pin must be made from is not known until the user has typed it. Both
    /// halves are Rust's sentence: `MaskState::save_then_pin` answers the write and the pin
    /// in one line, and the pin half is the only place a user learns that the file exists
    /// while nothing floated.
    function saveAndPin() {
        bar.pinAfterSave = true
        saveAs.open()
    }

    // The `--ink` gauge at this door. Everything above is the product's own path; what
    // follows only ever runs when a plan was planted, and it asks the one question
    // [`crate::annotate_check::save_rows`]'s two rows cannot: those name the file and call
    // the encoder from Rust, so `onAccepted` above is never entered and a handler that does
    // not work would pass them anyway. Here the door opens by `save()` and closes by a
    // click on the shell's own default button.
    //
    // One line to the gauge, in the `key|value` shape `annotate_check` grades.
    function gaugeSay(key, value) {
        bar.session.maskGaugeReport(key + "|" + value)
    }

    // A local path as the text a `QUrl` arrives in, because `currentFile` and
    // `currentFolder` take URLs and the plan carries a native path.
    function dlgUrl(local) {
        return "file:///" + String(local).replace(/\\/g, "/").replace(/ /g, "%20")
    }

    // `selectedNameFilter` is read-only and hands back an object, not a string, so it is
    // flattened here the way the pin window's same probe found it.
    function readFilter(key) {
        const chosen = saveAs.selectedNameFilter
        gaugeSay(key, chosen.name + "|" + chosen.extensions.join(",") + "|" + chosen.index)
    }

    // The extension text of one arm of the menu, spelled the way `selectedNameFilter`
    // spells the arm actually selected: `PNG 图片 (*.png)` → `*.png`. That member cannot be
    // pointed at an arm of this choosing - assigning to it is the hard `TypeError` P38's
    // probe E found - so a leg that wants to ask what the JPG arm does reads the list
    // instead, and hands the door the same text `onAccepted` would hand it for that arm.
    function filterExt(i) {
        const text = String(saveAs.nameFilters[i])
        const from = text.lastIndexOf("(") + 1
        const to = text.lastIndexOf(")")
        return to > from ? text.substring(from, to) : ""
    }

    // Phase 1: name the file, then open the door the way the cell does. The name is the
    // gauge's and the extension is the shell's, which is the point - the file
    // `mask save via real click` reads back is the one this run could only predict.
    function openDialogLeg() {
        const stem = String(bar.gauge[0]) + "/" + String(bar.gauge[1])
        saveAs.currentFolder = bar.dlgUrl(String(bar.gauge[0]) + "/")
        saveAs.currentFile = bar.dlgUrl(stem)
        gaugeSay("dlg-folder", String(saveAs.currentFolder))
        gaugeSay("dlg-name", String(saveAs.currentFile))
        // §10 第 40 项 ② at this door, before the dialog is in the way: a name with no
        // extension takes the format of the arm the dialog was showing, so the leg hands the
        // door the JPG arm's own text and a bare stem. JPG is the arm the configured
        // `[output] format` would never pick on this machine, which is what tells a
        // forwarded filter from one dropped at the boundary - and the boundary is the thing
        // being asked, since every leg that runs in Rust cannot see whether QML passes the
        // argument at all, and a real dialog click cannot either (the shell suffixes the
        // name before `onAccepted` reads it).
        //
        // It runs *above* the refusal underneath on purpose: `message` is one slot, and
        // P39's `accept-was`/`pre-save` pair has to stay that refusal's two halves.
        const bareName = bar.dlgUrl(String(bar.gauge[0]) + "/" + String(bar.gauge[1]) + "-f")
        const arm = bar.filterExt(1)
        const wrote = bar.view.saveSelection(bareName, arm)
        bar.gaugeSay("f-jpg", arm + "|" + String(wrote) + "|" + bar.view.message)

        // 存完再钉图 (§10 第 43 项 ①, PRD 433 / 459), asked at this same door one rung
        // further along: the file is written first and the pin is made *out of that file*,
        // so what floats afterwards is the artifact rather than a second reading of the
        // selection. `saveAndPin` is the cell's own invokable and the name is the gauge's,
        // so no dialog has to be in the way to ask it. `message` is read back because the
        // door answers both acts in one sentence, and the id in that sentence is the only
        // handle the report has on the pin the product says it made.
        const pinName = bar.dlgUrl(String(bar.gauge[0]) + "/" + String(bar.gauge[1]) + "-p.png")
        const pinned = bar.view.saveAndPin(pinName, "")
        bar.gaugeSay("pin-after", String(pinned) + "|" + bar.view.message)

        // The click's before-half has to be a sentence, not silence. P38 read
        // `accept-was ""` here - a true reading, and an empty one, so the graded pair
        // measured "from nothing to 已保存到" and could not tell an `onAccepted` that wrote
        // the product's sentence from one that merely happened to be the first to write
        // anything. So the leg asks the product to refuse a save first: `saveSelection` →
        // `MaskState::save_out` → `save::write` → `Refusal::BadName` writes
        // `MaskState::message` in the product's own words, and `gaugeSay` only ever reads it.
        //
        // An extension this program cannot write is the refusal chosen, not an unwritable
        // directory: this run's first attempt named a directory this run never made and the
        // save *succeeded*, because `core::encode::save` makes the parent before writing. A
        // refused name leaves nothing on disk and its sentence carries no operating-system
        // wording, so "the product wrote this" is readable off the sentence itself.
        const refused = String(bar.gauge[0]) + "/" + String(bar.gauge[1]) + ".not-a-format"
        // The empty filter is honest here: no dialog is open, so no arm is showing - and the
        // name carries an extension of its own, which is what the owner reads first.
        bar.view.saveSelection(bar.dlgUrl(refused), "")
        gaugeSay("pre-save", bar.view.message)
        bar.dlgLeg = true
        saveAs.open()
        // Read in the turn that asked for it, and again from the phase that presses:
        // whether `open()` comes back with the dialog up is a fact about this Qt, and the
        // graded one is whether the window exists when a button is looked for.
        gaugeSay("dlg-open", String(saveAs.visible))
        dlgPush.restart()
    }

    // Phase 2: press the dialog's own default button, once it is on the screen. The same
    // measurement as the pin's leg: Qt marks the dialog visible inside `open()` while Win32
    // has not made the `#32770` yet, so the attempt repeats until it has, up to the cap the
    // plan carries. `accept-was` is taken on the first attempt only, because it is the
    // before half of a pair graded across the click rather than across the waiting.
    function pushDialogLeg() {
        bar.pushTries += 1
        if (bar.pushTries === 1) {
            gaugeSay("dlg-shown", String(saveAs.visible))
            bar.readFilter("dlg-shown-filter")
            gaugeSay("accept-was", bar.view.message)
        }
        const said = bar.session.pushDialogButton()
        bar.pushTriesLog += (bar.pushTries === 1 ? "" : " ~ ") + String(said)
        if (said.startsWith("pushed=yes") || bar.pushTries >= bar.pushTriesMax) {
            gaugeSay("dlg-push", said)
            gaugeSay("dlg-push-tries", String(bar.pushTries))
            gaugeSay("dlg-push-log", bar.pushTriesLog)
            dlgTail.restart()
        } else {
            dlgPush.restart()
        }
    }

    // Phase 3: what the click left behind. The dismiss is guarded on `visible` because a
    // door the click already closed is gone, and `reject()` on it would be a second
    // completion this run did not measure.
    function finishDialogLeg() {
        gaugeSay("accept", bar.view.message)
        bar.readFilter("dlg-filter")
        gaugeSay("dlg-after", String(saveAs.visible))
        if (saveAs.visible) {
            gaugeSay("dlg-dismiss", "reject")
            saveAs.reject()
        }
        bar.dlgLeg = false
        bar.quickLeg()
        gaugeSay("done", bar.gauge[0])
    }

    // §5.5.4's 快速保存, asked the way the cell asks it: `quickSave` is the very function
    // the 快速保存 `Cell` taps, so this leg covers the panel's own handler and not only the
    // invokable beneath it - which is the one rung every Rust-side leg cannot see.
    //
    // The census that follows is this run's own definition of "opens no dialog": the probe
    // counts this process's `#32770` windows and, when the count is 0, presses nothing at
    // all. A quick door that did open one would be answered by `pushed=yes`, and the window
    // dismissed - §10 第 38 项 ①'s rule that a probe always cleans up after itself, kept
    // even at a door whose whole claim is that there is nothing to clean up.
    //
    // It runs *last*, after the dialog's before/after pair has been read, because `message`
    // is one slot and P39's `accept-was`/`pre-save` comparison has to finish before anything
    // else speaks through it.
    function quickLeg() {
        bar.quickSave()
        bar.gaugeSay("q-cell", String(bar.delivered) + "|" + bar.view.message)
        bar.gaugeSay("q-census", bar.session.pushDialogButton())
    }

    Timer {
        id: gaugeLeg
        interval: Number(bar.gauge[2]) || 400
        repeat: false
        // Stopped for every window but the one the plan named - several screens bind this
        // same document, and a leg that ran on each would open the dialog once per monitor -
        // and for every run that planted no plan at all.
        running: bar.gauge.length === 5 && String(bar.gauge[4]) === String(bar.view.index)
        onTriggered: bar.openDialogLeg()
    }

    Timer {
        id: dlgPush
        interval: Number(bar.gauge[2]) || 400
        repeat: false
        onTriggered: bar.pushDialogLeg()
    }

    Timer {
        id: dlgTail
        interval: Number(bar.gauge[2]) || 400
        repeat: false
        onTriggered: bar.finishDialogLeg()
    }

    // The gauge's witness that the signal fired, taken from outside the door so the
    // `onAccepted` below stays the one statement §5.5.5 asks for. It is not the whole
    // claim: the file it names is the rest of it.
    Connections {
        target: saveAs
        function onAccepted() {
            if (bar.gauge.length === 5 && bar.dlgLeg) {
                bar.gaugeSay("dlg-accepted", String(saveAs.selectedFile))
            }
        }
        function onRejected() {
            if (bar.gauge.length === 5 && bar.dlgLeg) {
                bar.gaugeSay("dlg-rejected", String(saveAs.selectedFile))
            }
        }
    }

    // One button. `tapped` is the only thing a delegate has to say; everything the
    // button *looks* like is a property the caller binds to `view`.
    component Cell: Rectangle {
        id: cell

        property string label: ""
        property bool active: false
        property bool usable: true
        signal tapped()

        width: cap.implicitWidth + 14
        height: 24
        radius: 3
        // The checked state is a fill, not a border: at 24 px tall a 1 px border is
        // most of the difference between "this one" and "those ones".
        color: cell.active ? "#2d7ff7"
                           : (area.containsMouse && cell.usable ? "#3a3a41" : "transparent")

        Text {
            id: cap
            anchors.centerIn: parent
            text: cell.label
            // `font.pixelSize` and not a point size: this panel is drawn at the
            // window's device ratio, and a point size would make the bar's height
            // depend on the system font DPI rather than on the screen it is on.
            font.pixelSize: 12
            color: cell.usable ? "#f2f2f2" : "#6a6a70"
        }

        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            enabled: cell.usable
            cursorShape: cell.usable ? Qt.PointingHandCursor : Qt.ArrowCursor
            // Release-on-target, not press: a press that slides off the button is a
            // pointer that wanted to draw on the picture under the panel.
            onReleased: if (containsMouse) cell.tapped()
        }
    }

    // A row's own name, so a row of 小/中/大 cells does not have to guess which ladder it
    // is lighting. Not a `Cell`: it takes no click, and a button that cannot be pressed is
    // the shape that promises one.
    component Caption: Text {
        id: caption

        property string label: ""

        anchors.verticalCenter: parent.verticalCenter
        text: caption.label
        font.pixelSize: 12
        color: "#8e8e96"
    }

    // One colour. A square would read as a second tool button; the thing users look
    // for is the colour itself, with the ring as the "this is the pen" mark.
    component Swatch: Rectangle {
        id: swatch

        property int rgb: 0
        property bool active: false
        signal tapped()

        width: 18
        height: 18
        radius: 9
        // `Qt.rgba`, not `Qt.rgb`: there is no `Qt.rgb` in QML - it is a C++ macro -
        // and a plain int would be read as 0xAARRGGBB, so the palette's 0x00RRGGBB
        // values would each draw a fully transparent chip. Nothing to click.
        color: Qt.rgba(((rgb >> 16) & 255) / 255, ((rgb >> 8) & 255) / 255,
                       (rgb & 255) / 255, 1.0)
        border.color: swatch.active ? "#ffffff" : "#00000000"
        border.width: swatch.active ? 2 : 0
        scale: area2.containsMouse ? 1.15 : 1.0

        MouseArea {
            id: area2
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onReleased: if (containsMouse) swatch.tapped()
        }
    }

    Column {
        id: body
        x: 6
        y: 6
        spacing: 4

        // Tools. The labels are Rust's (`toolNames()`, in `TOOLS` order), so the
        // button at index `i` cannot name the tool at index `i + 1`.
        Row {
            spacing: 2

            Repeater {
                model: bar.view.toolNames().split("|")

                delegate: Cell {
                    required property int index
                    required property var modelData

                    label: modelData
                    active: bar.view.tool === index
                    onTapped: bar.view.selectTool(index)
                }
            }
        }

        // Pen: colour, then the three widths, then the two style switches. One row
        // because they are one decision - what the next stroke looks like.
        Row {
            spacing: 6

            Repeater {
                model: bar.palette

                delegate: Swatch {
                    required property var modelData

                    rgb: modelData
                    active: bar.view.pen_rgb === modelData
                    onTapped: bar.view.setColor((modelData >> 16) & 255,
                                             (modelData >> 8) & 255,
                                             modelData & 255)
                }
            }

            Item { width: 1; height: 18 }

            // Its own Row so the wheel belongs to the widths and to nothing else:
            // §5.7.20 step 3 is "hover *the button* and roll to fine-tune", and a
            // handler over the whole pen row would turn a wheel by a colour chip into
            // a line-width change nobody asked for.
            //
            // A handler installed on the Row it is declared in, which is why this one
            // is a child of `pens`: a pointer handler is a QObject, not an Item - it
            // has no width, no height and no `anchors` to fill - so the area it hears
            // over is its parent item's, and the Row's is exactly the three buttons.
            Row {
                id: pens
                spacing: 2

                Repeater {
                    model: bar.pens

                    delegate: Cell {
                        required property int index
                        required property var modelData

                        // 细 / 中 / 粗, indexed against the same list the click sends.
                        label: ["细", "中", "粗"][index]
                        active: bar.view.pen_width === modelData
                        onTapped: bar.view.setWidth(modelData)
                    }
                }

                WheelHandler {
                    // `wheel` is this type's signal, and `angleDelta` is on the event
                    // it hands over: the handler has no `angleDelta` property of its
                    // own, and `onAngleChanged` names nothing that exists.
                    onWheel: (event) => {
                        const notch = event.angleDelta.y
                        if (notch === 0) {
                            return
                        }
                        bar.view.setWidth(bar.view.pen_width + (notch > 0 ? 1 : -1))
                        bar.grabbed()
                    }
                }
            }

            Item { width: 1; height: 18 }

            Cell {
                label: "虚线"
                active: bar.view.dashed
                onTapped: {
                    bar.view.applyDashed(!bar.view.dashed)
                    bar.grabbed()
                }
            }

            Cell {
                label: "填充"
                active: bar.view.filled
                onTapped: {
                    bar.view.applyFilled(!bar.view.filled)
                    bar.grabbed()
                }
            }
        }

        // §5.7.13 step 5's three settings, and only for the tool they belong to. The
        // row collapses instead of greying out: a 边框 switch is not a rectangle's
        // switch, and offering one that is lit for a tool with no border is a knob that
        // lies about what it tunes. `height` goes to 0 as well as `visible`, because the
        // `Column` above adds its spacing per child and a hidden row should not leave a
        // gap the user can see.
        Row {
            spacing: 6
            visible: bar.view.zoom_tool
            height: visible ? implicitHeight : 0

            Repeater {
                model: bar.magnifications

                delegate: Cell {
                    required property int index
                    required property var modelData

                    label: modelData + "%"
                    active: bar.view.zoom_percent === modelData
                    onTapped: {
                        bar.view.applyZoomPercent(modelData)
                        bar.grabbed()
                    }
                }
            }

            // Owned by this Row for the reason `pens` gives: the handler hears over its
            // parent item, and that item is the magnification cells and nothing else.
            WheelHandler {
                onWheel: (event) => {
                    const notch = event.angleDelta.y
                    if (notch === 0) {
                        return
                    }
                    bar.view.applyZoomPercent(bar.view.zoom_percent + (notch > 0 ? 50 : -50))
                    bar.grabbed()
                }
            }

            // The number the buttons and the wheel agree on, because between two named
            // values none of the three cells is lit and the user is left aiming blind.
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: bar.view.zoom_percent + "%"
                font.pixelSize: 12
                color: "#f2f2f2"
            }

            Item { width: 1; height: 18 }

            Cell {
                label: "边框"
                active: bar.view.zoom_border
                onTapped: {
                    bar.view.applyZoomBorder(!bar.view.zoom_border)
                    bar.grabbed()
                }
            }

            Cell {
                label: "连接线"
                active: bar.view.connection_line
                onTapped: {
                    bar.view.applyConnectionLine(!bar.view.connection_line)
                    bar.grabbed()
                }
            }
        }

        // §5.7.11 step 4's row, and only for the tool it belongs to. 颜色 is not here
        // because every pen already has one two rows up; 字体 is not here because a family
        // name wants a string field rather than a cell.
        //
        // What this row cannot preview is 背景 and 描边. Step 4 comes before step 5's
        // click-away, and while the box is open its pixels belong to the `TextInput`, not
        // to the ink layer - so the first sight of a plate or a ring is the committed
        // object. 字号 is the exception, and the one that makes this row more than a
        // promise about the next object: `text_font` is the same style number the cells
        // light, so the field the user is typing into resizes on the click.
        Row {
            spacing: 6
            visible: bar.view.text_tool
            height: visible ? implicitHeight : 0

            Caption {
                label: "字号"
            }

            Row {
                id: fonts
                spacing: 2

                Repeater {
                    model: bar.font_sizes

                    delegate: Cell {
                        required property int index
                        required property var modelData

                        // 小 / 中 / 大 against the same list the click sends, the shape
                        // `pens` gives 细 / 中 / 粗.
                        label: ["小", "中", "大"][index]
                        active: bar.view.pen_font === modelData
                        onTapped: {
                            bar.view.setFontSize(modelData)
                            bar.grabbed()
                        }
                    }
                }

                // Owned by `fonts`, for the reason the pen row gives: the handler hears
                // over its parent item, which is the three size cells and nothing else.
                WheelHandler {
                    onWheel: (event) => {
                        const notch = event.angleDelta.y
                        if (notch === 0) {
                            return
                        }
                        bar.view.setFontSize(bar.view.pen_font + (notch > 0 ? 2 : -2))
                        bar.grabbed()
                    }
                }
            }

            // The number the cells and the wheel agree on, because between two named
            // sizes no cell is lit and the user is left aiming blind. The family rides on
            // the same readout rather than getting a knob of its own: it has no cell to
            // light, and the row that does have one is §5.7.11's 字体 control, which is
            // not this round's - but a size with no family beside it would leave the file's
            // `font_family` row arriving somewhere nothing can look at.
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: bar.view.pen_font + "px · " + bar.view.pen_family
                font.pixelSize: 12
                color: "#f2f2f2"
            }

            Item { width: 1; height: 18 }

            Caption {
                label: "对齐"
            }

            Repeater {
                model: ["左对齐", "居中", "右对齐"]

                delegate: Cell {
                    required property int index
                    required property var modelData

                    // `index` is the code Rust's ladder answers to: 0 左, 1 中, 2 右.
                    label: modelData
                    active: bar.view.text_align === index
                    onTapped: {
                        bar.view.setAlign(index)
                        bar.grabbed()
                    }
                }
            }

            Item { width: 1; height: 18 }

            Cell {
                label: "背景"
                active: bar.view.text_bg
                onTapped: {
                    bar.view.applyTextBg(!bar.view.text_bg)
                    bar.grabbed()
                }
            }

            Cell {
                label: "描边"
                active: bar.view.text_outline
                onTapped: {
                    bar.view.applyTextOutline(!bar.view.text_outline)
                    bar.grabbed()
                }
            }
        }

        // §5.7.14's 擦除到透明, and only for the tool it belongs to - the third row of
        // the shape 放大 and 文本 already give.
        //
        // The words beside the switch are the rule, not decoration: PRD §5.7.14 lets the
        // mode exist *only* if its effect is stated, and nothing on this screen can show
        // it. QML draws the frozen desktop *under* the ink layer, so a pixel the eraser
        // refilled with the screenshot and a pixel the eraser cleared both read as the
        // desktop. The difference is in the picture that leaves the mask.
        Row {
            spacing: 6
            visible: bar.view.eraser_tool
            height: visible ? implicitHeight : 0

            Caption {
                label: "橡皮"
            }

            Cell {
                label: "擦除到透明"
                active: bar.view.erase_transparent
                onTapped: {
                    bar.view.applyEraseTransparent(!bar.view.erase_transparent)
                    bar.grabbed()
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                // One line for each state, because the switch off is the state the rule
                // calls 默认 and a user who turned it on once has to be able to see that
                // they are back in it. The effect is stated as the picture rather than as
                // a file: what the hole becomes on disk is the encoder's answer (a PNG
                // keeps it, a format without alpha fills it with white).
                text: bar.view.erase_transparent
                      ? "会连原始截图的像素一起擦掉"
                      : "只擦标注，原始截图的像素保留"
                font.pixelSize: 12
                color: bar.view.erase_transparent ? "#ff9f0a" : "#8e8e96"
            }
        }

        // Edits, then the ways out. 确认选区 is the same thing Enter already
        // does; the crop-and-new-pin that §5.7 asks for after it is M4b, which is why
        // this says 选区 and not 完成.
        //
        // 复制 and 保存 are the two that actually leave the program with something
        // (§5.5.2, §5.5.5), and they are greyed off with the selection rather than left
        // clickable, because a delivery with nothing selected is a refusal the user has
        // to be handed a sentence for. `Ctrl+C` and `Ctrl+S` still reach Rust's answer.
        // 快速保存 joins them from §5.5.4 with the same greying and the same chord rule,
        // and with no dialog to open - which is why its refusal has to be a sentence too:
        // a door that opens nothing leaves the message line as the only thing a user sees.
        Row {
            spacing: 2

            Cell {
                label: "撤销"
                usable: bar.view.can_undo
                onTapped: {
                    if (bar.view.undoStep()) {
                        bar.grabbed()
                    }
                }
            }

            Cell {
                label: "重做"
                usable: bar.view.can_redo
                onTapped: {
                    if (bar.view.redoStep()) {
                        bar.grabbed()
                    }
                }
            }

            Cell {
                label: "全清"
                usable: bar.view.objects > 0
                onTapped: {
                    if (bar.view.clearInk()) {
                        bar.grabbed()
                    }
                }
            }

            Item { width: 8; height: 1 }

            Cell {
                label: "复制"
                usable: bar.view.selected
                onTapped: bar.copy()
            }

            // §5.5.5: the same stitched picture, to a file. Greyed with the selection
            // for the reason 复制 is - a save with nothing selected is a refusal the
            // user has to be handed a sentence for, not a dialog.
            Cell {
                label: "保存"
                usable: bar.view.selected
                onTapped: bar.save()
            }

            // §5.5.4: the same write with no dialog in front of it. Next to 保存 because
            // that is where the user looks for a save, and named 快速保存 because the two
            // have to be tellable apart on screen - one asks for a name, this one takes
            // the one the settings already hold.
            Cell {
                label: "快速保存"
                usable: bar.view.selected
                onTapped: bar.quickSave()
            }

            // PRD 433's "完成后可复制、保存或贴图" read as one flow instead of two: the
            // picture goes to a file and the file comes back as a 贴图, so what floats is
            // the artifact the user named (§10 第 43 项 ①). Greyed with the selection for
            // the same reason 保存 is.
            Cell {
                label: "存完再钉图"
                usable: bar.view.selected
                onTapped: bar.saveAndPin()
            }

            Cell {
                label: "确认选区"
                usable: bar.view.selected
                onTapped: bar.commit()
            }

            Cell {
                label: "取消"
                onTapped: bar.cancel()
            }

            // What the last 复制 or 保存 did, in Rust's words. Visible only once there is
            // something to say: an empty label would take width from a sentence that
            // is not there, and the row would sit one cell further left than it did.
            Text {
                visible: bar.view.message.length > 0
                anchors.verticalCenter: parent.verticalCenter
                leftPadding: 8
                text: bar.view.message
                font.pixelSize: 12
                color: bar.delivered ? "#8e8e96" : "#ff9f0a"
            }
        }
    }

    // §5.5.5 step 2-4. The dialog decides the name and the extension, and
    // `saveSelection` reads the extension to pick the encoder, so the filter list below
    // is the list of formats this app writes from the mask - the same floor §5.5.5's
    // "至少支持 PNG、JPG、BMP" asks for, and the same three the pin menu offers.
    //
    // The list carries a second duty since P40: step 3 counts 格式 as one of the three
    // things the user chooses, so the arm that was showing decides the format of a name
    // typed without an extension, and `[output] format` only speaks when no arm names one.
    FileDialog {
        id: saveAs
        title: "另存为"
        fileMode: FileDialog.SaveFile
        nameFilters: ["PNG 图片 (*.png)", "JPG 图片 (*.jpg)", "BMP 图片 (*.bmp)"]
        onAccepted: {
            // `String()` rather than `.toLocalFile()`: this Qt hands JS a `QUrl` object
            // with no members at all - `toLocalFile`, `isFile` and `path` all measured
            // `undefined`, and the call threw a TypeError that §8-R14 keeps off stdout -
            // so the URL text is what travels and `save::local_path` reads it as a path.
            //
            // The second argument is the arm of the menu the user was looking at, in the
            // same flattened shape `readFilter` reports: §5.5.5 step 3 counts 格式 as one of
            // the three things chosen, so a name typed without an extension is that arm's
            // and not the setting's. An empty text is what a dialog that has selected no
            // filter answers with (`||-1`, read here the first time), and `save::write`
            // treats it as "the menu named nothing".
            // 存完再钉图 (§10 第 43 项 ①) is the same answer with one more act: the file
            // this name became is what gets pinned, so `saveAndPin` is handed exactly what
            // `saveSelection` would have been and the flag is put down before anything else
            // can read it. A pin that refused leaves the file behind and says so in the
            // same line - which is why the cell's verdict is not `bar.delivered` alone.
            const picked = String(saveAs.selectedFile)
            const chosen = saveAs.selectedNameFilter.extensions.join(",")
            bar.delivered = bar.pinAfterSave
                ? bar.view.saveAndPin(picked, chosen)
                : bar.view.saveSelection(picked, chosen)
            bar.pinAfterSave = false
            bar.grabbed()
        }
        onRejected: bar.pinAfterSave = false
    }
}
