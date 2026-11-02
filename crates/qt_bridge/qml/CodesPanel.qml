import QtQuick
import dev.falconshot 1.0

// Repeater 的代理是一块**嵌套组件**，它要读外面那两枚 id（根 `panel` 的三格读数、以及
// `col.width` 这一格宽度），而默认的对象创建语境不保证「外面」还是外面 —— qmllint 因此把
// 每一处都记成一笔 Unqualified access，并在结尾给出这一句解法。绑上之后代理只在根组件的
// 语境里造，外面就一定是外面。
//
// 其余七份文档没有这一行，因为那七份里没有「嵌套组件读外层 id」这件事；本轮不为「与别人一致」
// 把这五笔留着 —— 台账里那一格计数从 98 起，多出来的每一笔都要有人解释。
pragma ComponentBehavior: Bound

// §5.6.2 步骤 4 与 §5.6.3 步骤 3 的那一张「扫码结果」。
//
// 一节写二维码、一节写条形码，这里是**一块**面板：读的那一只手（`readMaskCodes` /
// `readPinCodes`）交回的是「这一张图里所有的码」，每行自己宣布它的类型。两扇门各摆一块面板就会
// 有两套「读到没读到」的判据，而第三枚「既不是二维码也不是条形码」的格子没人负责（#281）。
//
// 它一枚也不判：表、忙、故障三格由桥写，这一层把它们摆出来，手势递回桥 —— 复制走 `copyCodeAt`，
// 打开走 `openCodeAt`，重新扫码走 [`Self::reRecognized`]。
//
// 为什么复制与打开都只递**序号**而不递界面上那串字：两处都有理由，而且不是同一个。
// ① 显示那一格的内容被 `codes_encode` 的转义碰过（外来数据里的分隔字符），把显示字送回剪贴板
//    就是改了用户扫到的内容；② 内容长的行在窗上是**折行加省略**的，窗上那串字本来就不是全串。
// 于是「用户看到的第一行」与「用户复制到剪贴板的那一串」各自诚实：一个字不改的是 Rust 存着的
// 那一份，看得下的那一份由宽度决定。
Rectangle {
    id: panel

    required property Session session

    /// 主人有没有在**这一扇窗**里点过「扫码」。三格读数挂在 Session 上是全局的一格，而面板是
    /// 每扇窗各一枚：少了这一格，一场多屏截图里另一扇窗会凭空摆出别人那一屏的表（与
    /// `OcrPanel.qml` 同一封信）。
    property bool asked: false

    /// 按完复制、按完打开，桥自己那句话落在这儿 —— 不并进 [`Session::status`]：那一句是全场的，
    /// 这一句只关于这一块面板。
    property string said: ""

    /// 等着用户点头打开的那一行的序号；`-1` = 此刻没有。§5.6.2 的安全规则「打开外部链接前应显示
    /// 完整域名」落在这格上：**没有**一只能直接调 `openCodeAt` 的路径 —— 那只手只在这块面板把
    /// 域名摆出来、而用户按了「确认」之后才被叫到。
    ///
    /// 表一落地它就归 `-1`（`onCodeListChanged`）：序号是这一趟的序号，换一趟之后同一个数字指的
    /// 是另一枚码，而「确认打开」那一句里显示的域名是上一趟的。
    property int pending: -1

    /// 用户按了「重新扫码」。来源由主人决定，面板没有那张图的地址。
    signal reRecognized()

    width: 420
    height: col.implicitHeight + 16
    visible: panel.asked && (panel.session.codes_busy
                             || panel.codeList.length > 0
                             || panel.session.codes_error.length > 0
                             || panel.said.length > 0)

    color: "#f0232326"
    border.color: "#55555c"
    border.width: 1
    radius: 6

    /// 六格与 `session::codes_encode` 一一对上：`序号 类型 内容 域名 能不能开 为什么不能开`。
    /// 字段之间 `\u001c`、行之间 `\u001d`，与 `PinWindow.qml` 的 `decodeGroups` 同一族。
    ///
    /// 凑不满六格的行**丢掉**而不是猜：一行少一格就意味着那一列往后全错位，猜出来的「域名」
    /// 会被当成真的域名摆给用户看 —— 这一格是安全规则那一格，没有「大概是对的」这一档。
    function decodeCodes(raw) {
        var out = []
        if (!raw || raw.length === 0)
            return out
        var recs = raw.split("\u001d")
        for (var i = 0; i < recs.length; ++i) {
            var f = recs[i].split("\u001c")
            if (f.length < 6)
                continue
            out.push({
                index: Number(f[0]),
                format: f[1],
                text: f[2],
                host: f[3],
                openable: f[4] === "1",
                why: f[5]
            })
        }
        return out
    }

    readonly property var codeList: panel.decodeCodes(panel.session.codes_rows)

    /// 找不到那一行时的形状。用它而不是 `null`：`null.host` 是一条 TypeError，而**看不见**的
    /// 那一格绑定照样会求值 —— 窗不会因为它 `visible: false` 就不算那句表达式。
    readonly property var noRow: ({ index: -1, format: "", host: "", text: "", openable: false,
                                    why: "" })

    /// [`Self::pending`] 那一行的内容。**按序号找而不是按位置找**：凑不满六格的行会被
    /// [`Self::decodeCodes`] 丢掉，于是位置与序号可以差一格；按位置取的话，摆给用户看的域名与
    /// `openCodeAt` 开掉的那一枚就不是同一枚码了。
    readonly property var pendingRow: {
        for (var i = 0; i < panel.codeList.length; ++i)
            if (panel.codeList[i].index === panel.pending)
                return panel.codeList[i]
        return panel.noRow
    }

    // 表换了趟就把手势那一格收回来：`pending` 指的是**这一趟**的第几行。
    onCodeListChanged: panel.pending = -1

    component Chip: Rectangle {
        id: chip

        property string label: ""
        property bool usable: true
        signal clicked()

        width: cap.implicitWidth + 14
        height: 22
        radius: 3
        color: area.containsMouse && chip.usable ? "#3a3a41" : "transparent"

        Text {
            id: cap
            anchors.centerIn: parent
            text: chip.label
            font.pixelSize: 12
            color: chip.usable ? "#f2f2f2" : "#6a6a70"
        }

        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            enabled: chip.usable
            cursorShape: chip.usable ? Qt.PointingHandCursor : Qt.ArrowCursor
            onReleased: if (containsMouse) chip.clicked()
        }
    }

    Column {
        id: col
        x: 8
        y: 8
        width: panel.width - 16
        spacing: 6

        Row {
            spacing: 8

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "扫码结果"
                font.pixelSize: 13
                color: "#f2f2f2"
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "本机识别，不联网"
                font.pixelSize: 11
                color: "#8e8e96"
            }
        }

        // 进度那一格（§5.6.1「识别期间应显示进度状态」对本节同样成立）。读的是桥那一格，
        // 不是窗自己点下去那一刻置的旗：活儿归桥管。
        Text {
            visible: panel.session.codes_busy
            text: "扫码中…"
            font.pixelSize: 12
            color: "#2d7ff7"
        }

        // 故障那一行写的是引擎的原话。§5.6.2 安全规则「对无法识别或损坏的二维码显示明确提示」
        // 落在这儿：`rxing` 答「这一张里一枚也没有」与答「引擎读不动」是两句话，这里原样摆出来，
        // 不替它们合并。
        Text {
            visible: !panel.session.codes_busy && panel.session.codes_error.length > 0
            text: panel.session.codes_error + "（可重新框选后再点一次「扫码」）"
            font.pixelSize: 12
            color: "#ff9f0a"
            wrapMode: Text.WordWrap
            width: col.width
        }

        Repeater {
            model: panel.codeList

            Rectangle {
                id: card

                required property var modelData

                width: col.width
                height: body.implicitHeight + 12
                radius: 4
                color: "#1e1e22"
                border.color: "#3a3a41"
                border.width: 1

                Column {
                    id: body
                    x: 6
                    y: 6
                    width: card.width - 12
                    spacing: 4

                    // 第一行：类型（§5.6.3 步骤 3 那一问的答）与域名（§5.6.2 安全规则那一问的答）。
                    // 不是链接的时候那一格空着，就只报类型 —— 编一句「没有域名」是窗替用户下的判断。
                    Text {
                        width: body.width
                        text: card.modelData.host.length > 0
                              ? card.modelData.format + " · " + card.modelData.host
                              : card.modelData.format
                        font.pixelSize: 11
                        color: "#8e8e96"
                        elide: Text.ElideRight
                    }

                    Text {
                        width: body.width
                        text: card.modelData.text
                        font.pixelSize: 12
                        color: "#f2f2f2"
                        wrapMode: Text.WrapAnywhere
                        // 长的内容折到三行就省略：全的那一串由「复制」交回剪贴板，而那只手读的是
                        // Rust 存着的原样字节。窗上这一格的本份是「让用户认得出这是哪一枚码」。
                        maximumLineCount: 3
                        elide: Text.ElideRight
                    }

                    // 「是链接但本轮不开」那句（IDN 那一支）。它与「不是链接」分得开：后者这里
                    // 一个字也不说，因为打不开不是一件事，需要说的人才有一件事。
                    Text {
                        visible: card.modelData.host.length > 0 && !card.modelData.openable
                        width: body.width
                        text: card.modelData.why
                        font.pixelSize: 11
                        color: "#ff9f0a"
                        wrapMode: Text.WordWrap
                    }

                    Row {
                        spacing: 4

                        Chip {
                            label: "复制"
                            usable: !panel.session.codes_busy
                            // 递的是行上那一格**序号**（`codes_encode` 写进去的），不是 Repeater 的
                            // 位置：位置会因为一行被丢掉而挪，序号不会。见 [`Self::pendingRow`]。
                            onClicked: panel.said = panel.session.copyCodeAt(card.modelData.index)
                        }

                        // 灰的不是「这一枚读不出」，是「这一枚不是能开的那一类」：域名那一格空、
                        // 或桥明说了不开（`openable` 假）。按下去的是**确认**那一格，不是这一格。
                        Chip {
                            label: "打开"
                            usable: card.modelData.openable
                            onClicked: panel.pending = card.modelData.index
                        }
                    }
                }
            }
        }

        // 确认那一格：完整域名摆出来，用户点头之后才有 `openCodeAt` 那一次调用。域名读的是
        // `codeList[panel.pending].host` —— 与桥那边 `openCodeAt` 自己算出来的那一枚同出一手
        // （`falcon_core::link`），窗不拼一句自己理解的域名。
        Rectangle {
            id: confirm

            visible: panel.pendingRow.openable
            width: col.width
            height: line.implicitHeight + 12
            radius: 4
            color: "#26262b"
            border.color: "#ff9f0a"
            border.width: 1

            Column {
                x: 6
                y: 6
                width: confirm.width - 12
                spacing: 4

                Text {
                    id: line
                    width: parent.width
                    text: "将在默认浏览器打开：" + panel.pendingRow.host
                    font.pixelSize: 12
                    color: "#f2f2f2"
                    wrapMode: Text.WrapAnywhere
                }

                Text {
                    visible: panel.pendingRow.text !== panel.pendingRow.host
                    width: parent.width
                    text: "完整内容：" + panel.pendingRow.text
                    font.pixelSize: 11
                    color: "#8e8e96"
                    wrapMode: Text.WrapAnywhere
                    maximumLineCount: 2
                    elide: Text.ElideRight
                }

                Row {
                    spacing: 4

                    Chip {
                        label: "确认打开"
                        onClicked: {
                            panel.said = panel.session.openCodeAt(panel.pending)
                            panel.pending = -1
                        }
                    }

                    Chip {
                        label: "取消"
                        onClicked: panel.pending = -1
                    }
                }
            }
        }

        Text {
            visible: panel.said.length > 0
            text: panel.said
            font.pixelSize: 12
            color: "#8e8e96"
            wrapMode: Text.WordWrap
            width: col.width
        }

        Row {
            spacing: 4
            visible: !panel.session.codes_busy

            Chip {
                label: "重新扫码"
                onClicked: panel.reRecognized()
            }

            Chip {
                label: "收起"
                onClicked: {
                    panel.asked = false
                    panel.pending = -1
                }
            }
        }
    }
}
