import QtQuick
import QtQuick.Controls
import QtQuick.Window
import dev.falconshot 1.0

// §5.14.4 的那一扇「历史记录管理」窗（PRD 第 1305–1317 行）。
//
// 这一页上**没有一份判据、没有一枚字节数、也没有一句拒绝的措辞**。四枚钮的名字
// （`historyWindowButtons()`）、窗标题（`historyWindowTitle()` = PRD 第 1309 行那一串字）、表头那枚
// 全选的两个字（`historyWindowSelectAll()`）、二次确认那一问与两枚答（`historyWindowConfirmWords()`）
// 全部出自 [`crate::history_window`]；页上抄一份中文，改一天「钮上写着删除、递进产品的却是清空」
// 就只能靠数出来（#281，与 `CustomCaptureWindow.qml` 头顶那封信同一个理由，而门里
// `history window` 那一格判的正是「页上真的造出几枚钮 == 名单长度」）。
//
// 删与清也一次也不在这一页上判：
// * 「选择单条、多条或全部记录」（第 1310 行）+「点击删除」（第 1311 行）叫的是
//   `session.historyDelete(ids)` —— 那句话的作者仍是 [`crate::mask::MaskState::delete_history`]，
//   页上显示的就是它交回的原话，不是页自己拼的「删除成功」；
// * 「系统释放相应磁盘空间」（第 1312 行）那枚字节数住在产品那句话里，页上不数第二遍；
//   占用那一行 `historyWindowUsage()` 的算术与措辞唯一作者是 [`crate::settings::history_usage_line`]；
// * 「清空全部历史前应二次确认」（第 1316 行）在这里是**两把闸**：页上先亮一问（这一页唯一的
//   状态机），真人点「确认清空」才轮到 `historyClear(true)`；点「取消」递出去的是 `false`，而页上
//   把这一下答的**产品原话**显示出来 ——「二次确认没点头，一行也不动。」窗自己吞掉那一次点击、
//   屏幕上什么都不说，就是让页与产品各说一句（#281 的反面）；
// * 「不得删除用户明确收藏或锁定的记录」（第 1317 行）里「用户明确锁定」那一半的对面是每一行
//   右边那枚「锁上 / 解开」，它叫的是 `historyLock(id, locked)`。
//
// 这一扇窗与 `SettingsWindow.qml` / `CustomCaptureWindow.qml` 一样，是本进程里由 **Rust 决定存在
// 与否**的窗：`Session.history_window_open` 那一格是 0 就没有这个对象（`main.qml` 的 `syncHistory`），
// 所以产品会话与另外几道门不会被一扇没人要的顶层窗搅掉各自的窗清点。它**不**看 `settings_open`，
// 也不看 `custom_open`：三扇窗各有各的理由，一方开着不该把另一方带出来。
Window {
    id: win

    required property Session session

    /// `--history-window` 那一档的计划，七段（`historyWindowPlan()` 原文）：
    /// `腿等待 ms|单条行号|多条甲|多条乙|被清空那枚|锁定那枚|开工时行数`。
    /// 空串 = 这一趟没有量具的腿要跑：产品会话里真人自己点，别的门里整条躲开。
    ///
    /// 只在 `Component.onCompleted` 读一次：Rust 那一头在 `engine.load()` 之前就把计划写好，
    /// 而绑一枚 invokable 的绑定只会跑一遍（`main.qml` 里 `warmRounds` 钉下的教训）。
    property var plan: []

    /// 四枚钮的名字、那一问与两枚答、表头那两个字 —— 桥递回来的原文，页上不许多写一份。
    property var buttons: []
    property string headerText: ""
    property string selectWord: ""
    property string askWord: ""
    property string yesWord: ""
    property string noWord: ""

    /// 清单此刻那几行原文（每行第一段是裸行号，这是 `settings::history_rows_joined` 头顶钉下的
    /// 口径：界面要点某一行，它从这一串里拿回 SQLite 的那个 id，而不是从一句中文里猜数字）。
    property var rows: []

    /// 勾着的那几枚行号。勾选框写的就是这一串，那枚「删除所选」读的也是这一串 —— 量具设它走的
    /// 是同一条通路，所以它不是第二份状态（#281）。
    property var picked: []

    /// 二次确认那一问在不在场上。它是这一页唯一的状态，而门里 `confirm 二次` 那一格判的
    /// 就是「按下去的那一刻它亮了没有」。
    property bool asking: false

    /// 占用那一行。它**不绑 invokable**（绑定只会跑一遍），由 [`win.readRows`] 每次现读。
    property string usage: ""

    /// 页上那一句结果 / 拒绝的原文。两位作者：产品的句子与清空那一问的答，页上不自己拼一句
    /// 「删除成功」。
    property string note: ""

    /// 计划够不够跑那几条腿。绑**值**不绑长度：`plan` 是本文件自己声明的属性。
    readonly property bool armed: win.plan.length === 7

    // §5.19.1 的那套餐具，与 `CustomCaptureWindow.qml` / `SettingsWindow.qml` 同一族。
    readonly property bool dark: win.session.theme === "dark"
    readonly property color page_fill: dark ? "#12151B" : "#F2F4F7"
    readonly property color card_fill: dark ? "#1B1F27" : "#FFFFFF"
    readonly property color word_fill: dark ? "#F2F4F7" : "#12151B"
    readonly property color faint_fill: dark ? "#8B95A5" : "#5A6472"
    readonly property color edge_fill: dark ? "#39424F" : "#B4BEC9"
    readonly property color accent_fill: dark ? "#4C8DF6" : "#2563B8"

    // 一扇正常的管理窗：有边框、进任务栏、接键盘。第 1309 行写的是「历史记录管理」那样一扇窗，
    // 不是盖在桌面上的浮层，所以这里**不**写 `flags`。
    title: win.headerText
    width: 760
    height: 520
    color: win.page_fill

    function splitPipe(s) {
        const text = String(s)
        return text === "" ? [] : text.split("|")
    }

    function report(line) {
        win.session.historyWindowReport(line)
    }

    /// 一行的行号：第一段那枚裸数字。读不出来 ⇒ -1，它在库里不存在，产品那句会当场说
    /// 「点名的行号在库里一个也没有」——页上不替它圆。
    function idOf(line) {
        const head = String(line).split(" · ")[0]
        const n = Number(head)
        return Number.isFinite(n) ? n : -1
    }

    /// 重读清单与占用。每一次删 / 清 / 锁之后都叫它：第 1312 行那句「释放相应磁盘空间」在这一页上
    /// 必须**跟着动**，而它动的对面是 SQLite 现读的那一份，不是页上自己减出来的一枚数。
    function readRows() {
        win.rows = win.splitPipe(win.session.historyRows(200))
        win.usage = String(win.session.historyWindowUsage())
    }

    function checkedCount() {
        return win.picked.length
    }

    function toggle(id) {
        const out = []
        for (let i = 0; i < win.picked.length; i++)
            if (win.picked[i] !== id)
                out.push(win.picked[i])
        for (let j = 0; j < win.rows.length; j++)
            if (win.idOf(win.rows[j]) === id && win.picked.indexOf(id) < 0)
                out.push(id)
        win.picked = out
    }

    /// 表头那一枚「全选」：勾上就是把清单里每一枚行号都点齐（第 1310 行「…**或全部**记录」），
    /// 取消勾就是把这一串清空。行号从清单推出来，页上不抄一个 200、也不抄一个枚数。
    function selectAll(on) {
        if (!on) {
            win.picked = []
            return
        }
        const all = []
        for (let i = 0; i < win.rows.length; i++) {
            const id = win.idOf(win.rows[i])
            if (id > 0)
                all.push(id)
        }
        win.picked = all
    }

    /// 「删除所选」那一枚钮的手。空点名也照叫：产品自己有一句「没有点名任何一行，一行也不删」，
    /// 页上拦一次就等于替产品发明第二种「没删」的说法。
    function doDelete() {
        win.note = String(win.session.historyDelete(win.picked.join("|")))
        win.picked = []
        win.readRows()
    }

    /// 刷新那一枚钮：什么都不改，只把清单与占用再读一遍。
    function doRefresh() {
        win.readRows()
    }

    /// 「清空全部」那一枚钮：它**不**清任何东西，它只把那一问亮出来。第 1316 行那句
    /// 「应二次确认」在这一页上的形状就是这一句。
    function askClear() {
        win.asking = true
    }

    /// 「取消」：问收起来，产品收到 `false` 并把它那一句原话摆回页上。
    function cancelClear() {
        win.asking = false
        win.note = String(win.session.historyClear(false))
        win.readRows()
        return win.note
    }

    /// 「确认清空」：这一刻才轮到 `true`。清完照样重读一遍 —— 页上那句「剩下几行」的对面
    /// 是 SQLite，不是上一次读数加一减一。
    function confirmClear() {
        win.asking = false
        win.note = String(win.session.historyClear(true))
        win.picked = []
        win.readRows()
        return win.note
    }

    /// 每一行右边那枚锁。锁与解都叫同一只手 `historyLock`，页上不自己记一枚 bool ——
    /// 那一格的真值在库里，页上每次现读。
    function toggleLock(id) {
        const at = win.rowIndexOf(id)
        const marked = at >= 0 && String(win.rows[at]).indexOf("已锁定") >= 0
        win.note = String(win.session.historyLock(id, !marked))
        win.readRows()
    }

    function rowIndexOf(id) {
        for (let i = 0; i < win.rows.length; i++)
            if (win.idOf(win.rows[i]) === id)
                return i
        return -1
    }

    /// 清单里此刻有没有写着这一行锁着。门里 `lock a row` 那一格读的就是这一枚 —— 它问的是
    /// **页上看得见的那一句**，而不是库里那一列，因为第 1317 行那句「用户明确锁定」要的是
    /// 用户看得见自己锁过什么。
    function lockedShown(id) {
        return win.rowIndexOf(id) >= 0 && String(win.rows[win.rowIndexOf(id)]).indexOf("已锁定") >= 0 ? 1 : 0
    }

    /// 删完之后清单头一行那一枚行号，用来答「活下来的那一枚是谁」。空清单 ⇒ -1。
    function survivor() {
        return win.rows.length === 1 ? win.idOf(win.rows[0]) : -1
    }

    /// `--history-window` 的八条腿，一次跑完。每一格都读**页这一侧**看得见的数
    /// （`visible` / delegate 的 `count` / 清单行数），而不是「我们叫过哪只手」。
    function runGauge() {
        // ① 第 1309 行「用户打开“历史记录管理”」：窗在、标题是那一串字、四枚钮真造出来了、
        //    清单列了几行。
        win.report("open|" + (win.visible ? 1 : 0) + "|" + buttonRows.count + "|"
                   + (win.title === win.headerText && win.headerText.length > 0 ? 1 : 0) + "|"
                   + win.rows.length)

        // ② 第 1310 行「选择…全部记录」：表头那一枚点下去，勾上的枚数与清单枚数一并递回。
        win.selectAll(true)
        win.report("select-all|" + win.checkedCount() + "|" + win.rows.length)
        win.selectAll(false)

        // ③ 规则 1 的第一臂：按下「清空全部」→ 那一问亮着 → 答「取消」→ 产品收到 false。
        win.askClear()
        const asked = win.asking ? 1 : 0
        const refused = win.cancelClear()
        win.report("clear-unconfirmed|" + refused + "|" + asked + "|" + win.rows.length)

        // ④ 第 1311 行「点击删除」的单条那一档：只勾计划点名的那一枚，走的是页上那枚钮同一只手。
        win.picked = [Number(win.plan[1])]
        win.doDelete()
        win.report("delete-one|" + win.note + "|" + win.rows.length)

        // ⑤ 同一句的多条那一档。
        win.picked = [Number(win.plan[2]), Number(win.plan[3])]
        win.doDelete()
        win.report("delete-many|" + win.note + "|" + win.rows.length)

        // ⑥ 规则 2 的前半：那一枚锁是**页上那枚钮**打上的，而打完之后清单那一行自己得写着锁定。
        win.note = String(win.session.historyLock(Number(win.plan[5]), true))
        win.readRows()
        win.report("lock|" + win.note + "|" + win.lockedShown(Number(win.plan[5])))

        // ⑦ 规则 1 的另一臂与规则 2 的后半同一次答：确认后清掉的是没锁的那一枚，锁着的那一枚留下。
        win.askClear()
        const said = win.confirmClear()
        win.report("clear-confirmed|" + said + "|" + win.rows.length + "|" + win.survivor())

        // ⑧ 第 1312 行「释放相应磁盘空间」：删完之后的占用那一行，页上此刻看得见的那一串。
        win.report("usage-after|" + win.usage)
    }

    // ── 页面上摆出来的东西 ────────────────────────────────────────────────────────────────────
    Column {
        id: page
        anchors.fill: parent
        anchors.margins: 14
        spacing: 10

        // 顶上那一行：占用 + 刷新。占用那一行是 §6.2「显示历史记录占用多少空间」在这一扇窗上的
        // 对面，页上不做算术。
        Row {
            width: parent.width
            spacing: 10
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: win.usage
                color: win.faint_fill
                font.pixelSize: 12
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: win.note
                color: win.word_fill
                font.pixelSize: 12
                elide: Text.ElideRight
                width: parent.width - win.usage.length * 8 - 20
            }
        }

        // 表头那一枚全选，名字来自桥。
        CheckBox {
            text: win.selectWord
            onToggled: win.selectAll(checked)
        }

        Rectangle {
            width: parent.width
            height: 1
            color: win.edge_fill
        }

        // 清单。每一行：勾选框 + 那一行原文 + 一枚锁。行与字段那把刀由 Rust 递（上面那段口径）。
        ListView {
            id: list
            width: parent.width
            height: parent.height - 150
            clip: true
            model: win.rows
            spacing: 6
            delegate: Row {
                required property string modelData
                spacing: 8
                CheckBox {
                    checked: win.picked.indexOf(win.idOf(modelData)) >= 0
                    onToggled: win.toggle(win.idOf(modelData))
                }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: modelData
                    color: win.word_fill
                    font.pixelSize: 13
                    width: 560
                    wrapMode: Text.Wrap
                }
                Button {
                    anchors.verticalCenter: parent.verticalCenter
                    text: String(modelData).indexOf("已锁定") >= 0 ? "解开" : "锁上"
                    onClicked: win.toggleLock(win.idOf(modelData))
                }
            }
        }

        // 四枚钮。名字全部来自桥，一枚也不多、一枚也不少 —— 门里那一格数的就是 Repeater 那枚
        // `count`：它是「页上真的造出几枚」的读数，不是名单长度（`Row` 自己没有 `count`，
        // 所以这一枚 id 长在 Repeater 上而不是长在 Row 上）。
        Row {
            spacing: 10
            Repeater {
                id: buttonRows
                model: win.buttons
                Button {
                    required property string modelData
                    text: modelData
                    onClicked: {
                        if (modelData === win.buttons[0])
                            win.doRefresh()
                        else if (modelData === win.buttons[1])
                            win.doDelete()
                        else if (modelData === win.buttons[2])
                            win.askClear()
                        else
                            win.session.history_window_open = 0
                    }
                }
            }
        }
    }

    // 二次确认那一层。它只在这一页里存在，而它**不**自己宣布删成了什么：答完之后那句
    // 仍由产品交回（`confirmClear` / `cancelClear` 那两手）。
    Rectangle {
        anchors.centerIn: parent
        width: 460
        height: askCol.height + 28
        visible: win.asking
        color: win.card_fill
        border.width: 1
        border.color: win.accent_fill

        Column {
            id: askCol
            anchors.centerIn: parent
            spacing: 12

            Text {
                text: win.askWord
                color: win.word_fill
                font.pixelSize: 13
                wrapMode: Text.Wrap
                width: 420
            }

            Row {
                spacing: 10
                Button {
                    text: win.yesWord
                    onClicked: win.confirmClear()
                }
                Button {
                    text: win.noWord
                    onClicked: win.cancelClear()
                }
            }
        }
    }

    Component.onCompleted: {
        win.headerText = String(win.session.historyWindowTitle())
        win.buttons = win.splitPipe(win.session.historyWindowButtons())
        win.selectWord = String(win.session.historyWindowSelectAll())
        const words = win.splitPipe(win.session.historyWindowConfirmWords())
        win.askWord = words.length > 0 ? String(words[0]) : ""
        win.yesWord = words.length > 1 ? String(words[1]) : ""
        win.noWord = words.length > 2 ? String(words[2]) : ""
        win.plan = win.splitPipe(win.session.historyWindowPlan())

        // 场上与场下只有 `show()` 与 `hide()` 两个入口，`visible` 一处也不写 —— 与
        // `HintWindow.qml` 同一封报错信（赋值之后 visible 仍是 false）。
        win.show()
        win.readRows()
        // `born` 是那一格 BLOCKED 的分界证人：窗一行也没递回来时，报告里那句「造出来了而腿没跑」
        // 与「压根没被造出来」全靠这一行分开（`crate::history_window::measure` 头顶那句）。
        win.report("born")
        if (win.armed)
            win.runGauge()
    }
}
