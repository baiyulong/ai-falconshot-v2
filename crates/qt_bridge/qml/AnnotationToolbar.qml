import QtQuick
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
            // sizes no cell is lit and the user is left aiming blind.
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: bar.view.pen_font + "px"
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

        // Edits, then the two ways out. 确认选区 is the same thing Enter already
        // does; the crop-and-new-pin that §5.7 asks for after it is M4b, which is why
        // this says 选区 and not 完成.
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
                label: "确认选区"
                usable: bar.view.selected
                onTapped: bar.commit()
            }

            Cell {
                label: "取消"
                onTapped: bar.cancel()
            }
        }
    }
}
