import QtQuick
import QtQuick.Controls
import QtQuick.Window
import dev.falconshot 1.0

// §5.12.3 与 §5.12.4 的那一扇「图片分组管理」窗（PRD 第 1232–1253 行），托盘第 1437 行那枚
// 「图片分组」点开的就是它。
//
// 这一页上**没有一条判据、没有一次计数、也没有一句中文是页自己写的**。窗标题（`groupWindowTitle()`
// = PRD 第 1218 行那六个字）、六枚钮的名字（`groupWindowButtons()`）、每行右边那句
// 「显示中 / 已隐藏」（`groupWindowStateWords()`）、删除那一问与三枚答（`groupWindowDeleteWords()`）、
// 关掉这一页那一枚（`groupWindowCloseWord()`）与新建 / 改名那两名（`groupWindowNames()`）
// 全部出自 [`crate::group_window`]；页上抄一份，改一天「钮上写着重命名、递进产品的却是删除」
// 就只能靠数出来（#281，与 `HistoryWindow.qml` 头顶那封信同一个理由，而门里 `the page opens`
// 那一格判的正是「页上真的造出几枚钮 == 名单长度」）。
//
// 分组那一族的事也一次也不在这一页上判：
// * 「输入分组名称 → 保存后创建空分组」（第 1219–1221 行）叫的是 `session.createGroup(name)`，
//   页上不判空名、不查重名 —— 那一格的真值在 core（`group_name` / `name_taken`），
//   失败时页上摆的是产品 `status` 那句原话，不是页自己拼的「名字不能用」；
// * 「删除、重命名或拖动排序」（第 1238 行）是 `renameGroup` / `moveGroup` / `deleteGroup`；
// * 「分组名称不能为空」（第 1244 行）与「排序变化应同步反映到快捷切换菜单」（第 1245 行）
//   这两格在这页上没有对面 —— 前者由产品那句话回答，后者由 `session.groups` 那**一串字的次序**
//   回答：本页与 `PinWindow.qml` 的快捷切换菜单解的是同一串，谁也没重排；
// * 「删除非空分组时，应询问贴图如何处理」（第 1243 行）在这里是**两把闸**：页上先亮一问
//   （这一页唯一的状态机），真人点「退回未分组」才递 `destroy=0`、点「随组销毁」才递 `1`；
//   点「取消」什么都不递 —— 门里 `the ask comes before the cut` 那一格判的就是「按了删除而一次
//   也没答」之后清单少没少一行。页上把这一下答的产品原话显示出来，窗自己吞掉那一次点击、
//   屏幕上什么都不说，就是让页与产品各说一句（#281 的反面）；
// * 「系统统一更新该分组内贴图的可见状态」（第 1253 行）页上不数：那一枚 `live` 的真值在模型里，
//   页上只在量具那一趟现问一次 `groupWindowLive(id)` 把数递回去，而本档另在**同一拍**抄一份
//   模型自己的读数对它（`crate::group_window::Snap`）。
//
// 这一扇窗与 `SettingsWindow.qml` / `CustomCaptureWindow.qml` / `HistoryWindow.qml` 一样，是本进程里
// 由 **Rust 决定存在与否**的窗：`Session.group_window_open` 那一格是 0 就没有这个对象
// （`main.qml` 的 `syncGroup`），所以产品会话与另外几道门不会被一扇没人要的顶层窗搅掉各自的窗清点。
// 它**不**看 `settings_open`、`custom_open` 或 `history_window_open`：四扇窗各有各的理由。
Window {
    id: win

    required property Session session

    /// `--group-window` 那一档的计划，七段（`groupWindowPlan()` 原文）：
    /// `腿等待 ms|甲组号|乙组号|丙组号|贴图A|贴图B|贴图C`。
    /// 空串 = 这一趟没有量具的腿要跑：产品会话里真人自己点，别的门里整条躲开。
    ///
    /// 只在 `Component.onCompleted` 读一次：Rust 那一头在 `engine.load()` 之前就把计划写好，
    /// 而绑一枚 invokable 的绑定只会跑一遍（`main.qml` 里 `warmRounds` 钉下的教训）。
    property var plan: []

    /// 页面上那些字的原文，页上不许多写一份。
    property var buttons: []
    property var stateWords: []
    property string headerText: ""
    property string closeWord: ""
    property string askWord: ""
    property string keepWord: ""
    property string destroyWord: ""
    property string cancelWord: ""
    property string newName: ""
    property string renamedName: ""

    /// 清单此刻那几行，解的是 `session.groups` 那一串（`编号/名称/显不显/几枚`）。
    /// **次序一格也不改**：那一串的次序就是 core `groups` 那个 vec 本体的次序。
    property var rows: []

    /// 那六只手此刻对着哪一组。它是这一页自己的一格状态（不是模型里的数）：真人点一行就是选它，
    /// 量具那几条腿把编号递给同一批手 —— 所以它不是第二份状态（#281）。
    property int focusId: -1

    /// 名字那一格此刻写着什么。「新建分组」与「重命名」两只手读的是同一格。
    property string typed: ""

    /// 删除那一问在不在场上，以及它问的是哪一组。门里 `confirm 那一问` 的对面就是这一枚。
    property bool asking: false
    property int askId: -1

    /// 页上那一句结果 / 拒绝的原文。它**只有产品那一个作者**：每叫完一只手就把 `session.status`
    /// 抄上屏，页上不自己拼一句「已删除」。
    property string note: ""

    /// 计划够不够跑那几条腿。绑**值**不绑长度：`plan` 是本文件自己声明的属性。
    readonly property bool armed: win.plan.length === 7

    // §5.19.1 的那套餐具，与 `HistoryWindow.qml` / `CustomCaptureWindow.qml` 同一族。
    readonly property bool dark: win.session.theme === "dark"
    readonly property color page_fill: dark ? "#12151B" : "#F2F4F7"
    readonly property color card_fill: dark ? "#1B1F27" : "#FFFFFF"
    readonly property color word_fill: dark ? "#F2F4F7" : "#12151B"
    readonly property color faint_fill: dark ? "#8B95A5" : "#5A6472"
    readonly property color edge_fill: dark ? "#39424F" : "#B4BEC9"
    readonly property color accent_fill: dark ? "#4C8DF6" : "#2563B8"

    // 一扇正常的管理窗：有边框、进任务栏、接键盘。第 1218 行写的是「图片分组管理」那样一扇窗，
    // 不是盖在桌面上的浮层，所以这里**不**写 `flags`。
    title: win.headerText
    width: 720
    height: 500
    color: win.page_fill

    function splitPipe(s) {
        const text = String(s)
        return text === "" ? [] : text.split("|")
    }

    function report(line) {
        win.session.groupWindowReport(line)
    }

    /// `session.groups` 那一串的解法，与 `PinWindow.qml` 的 `decodeGroups` 同一把刀：字段之间
    /// `\u001c`、组之间 `\u001d`（两枚控制字符住在 `session::group_encode` 那侧）。这里再抄一份
    /// 解析不是第二份判据 —— 判据是「第几段是什么」，而那一格由桥的文档钉着。
    function decodeGroups(raw) {
        const out = []
        if (!raw || raw.length === 0)
            return out
        const recs = String(raw).split("\u001d")
        for (let i = 0; i < recs.length; ++i) {
            const f = recs[i].split("\u001c")
            if (f.length < 4)
                continue
            out.push({ id: Number(f[0]), name: f[1], shown: f[2] === "1", count: Number(f[3]) })
        }
        return out
    }

    /// 重读清单。**每一只手之后都叫它**：第 1245 行那句「排序变化同步反映到快捷切换菜单」要的是
    /// 页上的次序跟着 core 动，而它跟着动的对面是桥现递的那一串，不是页自己挪的一行。
    function readRows() {
        win.rows = win.decodeGroups(win.session.groups)
    }

    function rowIndexOf(id) {
        for (let i = 0; i < win.rows.length; i++)
            if (win.rows[i].id === id)
                return i
        return -1
    }

    function rowOf(id) {
        const at = win.rowIndexOf(id)
        return at >= 0 ? win.rows[at] : null
    }

    /// 页上那一行此刻写着的成员枚数；这一行不在清单上就是 -1。门里 ① 那一格拿它看**页这一侧**
    /// 数出来的甲组两枚与丙组零枚，而不是只信桥在 `arm()` 里自己数的那一遍。
    function countOf(id) {
        const r = win.rowOf(id)
        return r === null ? -1 : r.count
    }

    /// 那一行此刻写着的显隐那一句。它是**页上看得见的那一串**（`stateWords` 由桥递），
    /// 门里 `hide covers the whole group` 两格读的就是它，而不是模型里那一枚 bool。
    function wordOf(id) {
        const r = win.rowOf(id)
        if (r === null)
            return ""
        return r.shown ? win.stateWords[0] : win.stateWords[1]
    }

    /// 页上此刻那几行的编号次序，编码成 `id,id,id`。门里 `the order moves together` 拿它与
    /// `crate::group_window::order_text()` 逐位对（`,` 是这一串的字段刀，`|` 是报告的刀，两把不混用）。
    function orderText() {
        const all = []
        for (let i = 0; i < win.rows.length; i++)
            all.push(win.rows[i].id)
        return all.join(",")
    }

    /// 选一行做那六只手的对面；被删掉的那一枚自动落到清单头一行。页上不替模型记任何一格状态。
    function focus(id) {
        win.focusId = id
    }

    function focusFallback() {
        if (win.rowIndexOf(win.focusId) < 0)
            win.focusId = win.rows.length > 0 ? win.rows[0].id : -1
    }

    /// 「新建分组」那枚钮的手。空名照叫：core 会交回 -1 并把「名字不能用」是哪一句写进 `status`，
    /// 页上拦一次就等于替产品发明第二种拒绝的说法（与 `HistoryWindow.doDelete` 同一枚口径）。
    function doCreate(name) {
        const id = Number(win.session.createGroup(name))
        win.readRows()
        win.note = String(win.session.status)
        return id
    }

    function doRename(id, name) {
        const rc = Number(win.session.renameGroup(id, name))
        win.readRows()
        win.note = String(win.session.status)
        return rc
    }

    function doMove(id, delta) {
        const rc = Number(win.session.moveGroup(id, delta))
        win.readRows()
        win.note = String(win.session.status)
        return rc
    }

    function doToggle(id) {
        const shown = Number(win.session.toggleGroupShown(id))
        win.readRows()
        win.note = String(win.session.status)
        return shown
    }

    /// 那一只手真的按归宿删完了东西。`destroy` 是 1 就是「随组销毁」，是 0 就是「退回未分组」——
    /// 两枚答钮各递一枚，页上不自己数有几枚贴图（那句话的枚数在产品的 `status` 里）。
    function doDelete(id, destroy) {
        const n = Number(win.session.deleteGroup(id, destroy))
        win.readRows()
        win.focusFallback()
        win.note = String(win.session.status)
        return n
    }

    /// 「删除」那枚钮：**它一次也不删**，它只把那一问亮出来，并把问的是哪一组记住。
    function askDelete(id) {
        if (win.rowIndexOf(id) < 0)
            return
        win.askId = id
        win.asking = true
    }

    /// 「取消」：问收起来，产品一次也没被叫到。页上不写一句「没删」——那件事由清单少没少一行
    /// 自己说出来，而门里那一格量的正是它没少。
    function cancelAsk() {
        win.asking = false
        win.askId = -1
    }

    /// 两枚归宿答钮：这一刻才轮到那一只手。
    function confirmAsk(destroy) {
        const id = win.askId
        win.asking = false
        win.askId = -1
        if (id >= 0)
            win.doDelete(id, destroy)
    }

    // `--group-window` 的十条腿，一次跑完。每一格都读**页这一侧**看得见的数
    // （`visible` / delegate 的 `count` / 清单那几行 / 现问的 `groupWindowLive`），
    // 而不是「我们叫过哪只手」。
    function runGauge() {
        const ga = Number(win.plan[1])
        const gb = Number(win.plan[2])
        const gc = Number(win.plan[3])
        const pa = Number(win.plan[4])
        const pb = Number(win.plan[5])
        const pc = Number(win.plan[6])

        // ① 第 1218 行「用户打开“图片分组管理”」与第 1236 行「分组管理页」那一格的全部对面：
        //    窗在不在、标题是不是那一串字、页上真的造出几枚钮、这一趟列出几行。
        //    `visible` 是窗自己问 Qt 的那一枚数而不是「我们叫过 show()」，`buttonRows.count` 是
        //    delegate 的枚数而不是名单长度。
        win.report("open|" + (win.visible ? 1 : 0) + "|" + buttonRows.count + "|"
                   + (win.title === win.headerText && win.headerText.length > 0 ? 1 : 0) + "|"
                   + win.rows.length + "|" + win.countOf(ga) + "|" + win.countOf(gc))

        // ② 第 1219–1221 行「新建分组 → 输入分组名称 → 保存后创建空分组」：新行真的一枚成员也没有。
        const made = win.doCreate(win.newName)
        const fresh = win.rowOf(made)
        win.report("create|" + made + "|" + (fresh === null ? -1 : fresh.count) + "|"
                   + (fresh === null ? "" : fresh.name) + "|" + win.rows.length)

        // ③ 第 1244 行的那一臂：页上不拦空名，所以拒绝的话是产品说的，而清单**没多**一行。
        const blank = win.doCreate("")
        win.report("create-blank|" + blank + "|" + win.rows.length + "|" + win.note)

        // ④ 第 1238 行「重命名」：改完页上那一行**自己**写着新名。
        const rn = win.doRename(made, win.renamedName)
        const renamed = win.rowOf(made)
        win.report("rename|" + rn + "|" + (renamed === null ? "" : renamed.name) + "|" + win.note)

        // ⑤ 第 1245 行「排序变化应同步反映到快捷切换菜单」：把最后一行**上移**一格。
        //  为什么不是下移：`move_group` 越界时交回 `Ok(())` 而一动不动，下移最后一格会量不出
        //  「挪过」；页上递动之前与动之后两串次序，本档再自己去读 core 那一串对第三遍。
        //  尾部那一枚是页挪的**哪一组** —— 本档拿它与 ② 新建回来的编号对，挪错一组就红。
        const before = win.orderText()
        const mv = win.doMove(made, -1)
        win.report("move|" + mv + "|" + win.orderText() + "|" + before + "|" + made)

        // ⑥⑦ 第 1252–1253 行「执行显示、隐藏或切换」与「统一更新该分组内贴图的可见状态」：
        //    甲组那两枚跟着走，没进这一组的第三枚照旧。三枚 `live` 都是**页现问桥**的那一眼。
        //    尾部那四枚编号是页自己交回来的**它按的是哪一组、问的是哪三枚** —— 本档拿它与
        //    `arm()` 写进计划的那六枚逐个对：页去点了别的组、这一格就红，而不是把别人的数读成绿的。
        const hid = win.doToggle(ga)
        win.report("toggle-hide|" + hid + "|" + win.wordOf(ga) + "|"
                   + win.session.groupWindowLive(pa) + "|" + win.session.groupWindowLive(pb) + "|"
                   + win.session.groupWindowLive(pc) + "|" + ga + "|" + pa + "|" + pb + "|" + pc)
        const lit = win.doToggle(ga)
        win.report("toggle-show|" + lit + "|" + win.wordOf(ga) + "|"
                   + win.session.groupWindowLive(pa) + "|" + win.session.groupWindowLive(pb) + "|"
                   + win.session.groupWindowLive(pc) + "|" + ga + "|" + pa + "|" + pb + "|" + pc)

        // ⑧ 规则 1 的第一臂：按下「删除」而**没答**那一问 ⇒ 产品一次也没被叫到。
        //    `asked` 必须在收起之前取，因为门里那一格问的是「按下去的那一刻它亮了没有」。
        win.askDelete(ga)
        const asked = win.asking ? 1 : 0
        win.cancelAsk()
        win.report("delete-asked|" + asked + "|" + win.rows.length + "|"
                   + (win.rowIndexOf(ga) >= 0 ? 1 : 0) + "|" + ga)

        // ⑨ 答「退回未分组」：那一组没了，它的两枚贴图还在集里、还在场上。
        win.askDelete(ga)
        win.confirmAsk(0)
        win.report("delete-ungroup|" + win.note + "|" + win.rows.length + "|"
                   + win.session.groupWindowHeld(pa) + "|" + win.session.groupWindowLive(pa)
                   + "|" + ga + "|" + pa)

        // ⑩ 答「随组销毁」：那一枚贴图连窗一起没了 —— 与 ⑨ 是这一问的两件事，读反了就是把
        //    销毁读成退回。问的是**乙组**（甲在 ⑨ 已经删了），而种下的归属是「甲拿前两枚、乙拿
        //    第三枚」（见 `crate::group_window::arm` 里那三份区间），所以这一枚是 `pc` 不是 `pb`。
        win.askDelete(gb)
        win.confirmAsk(1)
        win.report("delete-destroy|" + win.note + "|" + win.rows.length + "|"
                   + win.session.groupWindowHeld(pc) + "|" + win.session.groupWindowLive(pc)
                   + "|" + gb + "|" + pc)
    }

    // ── 页面上摆出来的东西 ────────────────────────────────────────────────────────────────────
    Column {
        id: page
        anchors.fill: parent
        anchors.margins: 14
        spacing: 10

        // 顶上那一行：名字输入 + 产品此刻那一句。句子里的枚数与归宿都是产品说的，页上不数第二遍。
        Row {
            width: parent.width
            spacing: 10
            TextField {
                id: nameField
                anchors.verticalCenter: parent.verticalCenter
                width: 240
                text: win.typed
                onTextChanged: if (win.typed !== text) win.typed = text
                color: win.word_fill
                font.pixelSize: 13
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: win.note
                color: win.faint_fill
                font.pixelSize: 12
                elide: Text.ElideRight
                width: parent.width - 260
                wrapMode: Text.NoWrap
            }
        }

        Rectangle {
            width: parent.width
            height: 1
            color: win.edge_fill
        }

        // 清单。一行：名称 + 几枚 + 那一句话，点一行就是选它（那六只手的对面）。
        ListView {
            id: list
            width: parent.width
            height: parent.height - 170
            clip: true
            model: win.rows
            spacing: 6
            delegate: Rectangle {
                required property var modelData
                required property int index
                width: list.width
                height: line.height + 12
                radius: 4
                color: win.card_fill
                border.width: win.focusId === modelData.id ? 2 : 1
                border.color: win.focusId === modelData.id ? win.accent_fill : win.edge_fill

                MouseArea {
                    anchors.fill: parent
                    onClicked: win.focus(modelData.id)
                }

                Row {
                    id: line
                    anchors.left: parent.left
                    anchors.leftMargin: 10
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 12

                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: modelData.name
                        color: win.word_fill
                        font.pixelSize: 13
                        width: 300
                        elide: Text.ElideRight
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        // 「几枚」是 core 数过抬过来的那一格（`pins_in_group`），页上一格也不自己数。
                        text: modelData.count
                        color: win.faint_fill
                        font.pixelSize: 12
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: win.stateWords[modelData.shown ? 0 : 1]
                        color: win.faint_fill
                        font.pixelSize: 12
                    }
                }
            }
        }

        // 六枚钮。名字全部来自桥，一枚也不多、一枚也不少 —— 门里那一格数的就是这一枚 `count`：
        // 它是「页上真的造出几枚」的读数，不是名单长度（`Row` 自己没有 `count`，
        // 所以这一枚 id 长在 Repeater 上而不是长在 Row 上）。
        Row {
            spacing: 8
            Repeater {
                id: buttonRows
                model: win.buttons
                Button {
                    required property string modelData
                    required property int index
                    text: modelData
                    onClicked: {
                        const id = win.focusId
                        // 「新建分组」那一枚不需要对面（它造出一枚新的），其余五枚没有对面就什么都不叫。
                        if (index > 0 && win.rowIndexOf(id) < 0)
                            return
                        if (index === 0)
                            win.focus(win.doCreate(win.typed))
                        else if (index === 1)
                            win.doRename(id, win.typed)
                        else if (index === 2)
                            win.doMove(id, -1)
                        else if (index === 3)
                            win.doMove(id, 1)
                        else if (index === 4)
                            win.doToggle(id)
                        else
                            win.askDelete(id)
                        win.focusFallback()
                    }
                }
            }

            // 关掉这一页那一枚，与上面六枚不同族（六枚改模型，这一枚改窗自己在不在），
            // 所以它**不**在 `win.buttons` 那份名单里 —— 门里数 delegate 时才不会多算一枚。
            Button {
                text: win.closeWord
                onClicked: win.session.group_window_open = 0
            }
        }
    }

    // 那一问。它只在这一页里存在，而它**不**自己宣布删成了什么：答完之后那句仍由产品交回
    // （`confirmAsk` 里那一只手），取消那一臂连一句也不说。
    Rectangle {
        anchors.centerIn: parent
        width: 480
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
                width: 440
            }

            Row {
                spacing: 10
                Button {
                    text: win.keepWord
                    onClicked: win.confirmAsk(0)
                }
                Button {
                    text: win.destroyWord
                    onClicked: win.confirmAsk(1)
                }
                Button {
                    text: win.cancelWord
                    onClicked: win.cancelAsk()
                }
            }
        }
    }

    Component.onCompleted: {
        win.headerText = String(win.session.groupWindowTitle())
        win.buttons = win.splitPipe(win.session.groupWindowButtons())
        win.stateWords = win.splitPipe(win.session.groupWindowStateWords())
        const words = win.splitPipe(win.session.groupWindowDeleteWords())
        win.askWord = words.length > 0 ? String(words[0]) : ""
        win.keepWord = words.length > 1 ? String(words[1]) : ""
        win.destroyWord = words.length > 2 ? String(words[2]) : ""
        win.cancelWord = words.length > 3 ? String(words[3]) : ""
        win.closeWord = String(win.session.groupWindowCloseWord())
        const names = win.splitPipe(win.session.groupWindowNames())
        win.newName = names.length > 0 ? String(names[0]) : ""
        win.renamedName = names.length > 1 ? String(names[1]) : ""
        win.plan = win.splitPipe(win.session.groupWindowPlan())

        // 场上与场下只有 `show()` 与 `hide()` 两个入口，`visible` 一处也不写 —— 与
        // `HintWindow.qml` 同一封报错信（赋值之后 visible 仍是 false）。
        win.show()
        win.readRows()
        win.focusFallback()
        // `born` 是那一格 BLOCKED 的分界证人：窗一行也没递回来时，报告里那句「造出来了而腿没跑」
        // 与「压根没被造出来」全靠这一行分开（`crate::group_window::measure` 头顶那句）。
        win.report("born")
        if (win.armed)
            win.runGauge()
    }
}
