import QtQuick
import QtQuick.Controls
import QtQuick.Window
import dev.falconshot 1.0

// §5.14.4 的那一扇「历史记录管理」窗（PRD 第 1305–1317 行）。
//
// 这一页上**没有一份判据、没有一枚字节数、也没有一句拒绝的措辞**。页顶那四枚钮的名字
// （`historyWindowButtons()`）、每一行那五枚动作钮的名字（`historyWindowActions()`）、翻页那两枚
// （`historyWindowFlipWords()`）、窗标题（`historyWindowTitle()` = PRD 第 1309 行那一串字）、表头那枚
// 全选的两个字（`historyWindowSelectAll()`）、二次确认那一问与两枚答（`historyWindowConfirmWords()`）
// 全部出自 [`crate::history_window`]；页上抄一份中文，改一天「钮上写着删除、递进产品的却是清空」
// 就只能靠数出来（#281，与 `CustomCaptureWindow.qml` 头顶那封信同一个理由，而门里
// `history window` 那一格判的正是「页上真的造出几枚钮 == 名单长度」，页顶与行上各数一枚）。
//
// 删与清也一次也不在这一页上判：
// * 「选择单条、多条或全部记录」（第 1310 行）+「点击删除」（第 1311 行）叫的是
//   `session.historyDelete(ids)` —— 那句话的作者仍是 [`crate::mask::MaskState::delete_history`]，
//   页上显示的就是它交回的原话，不是页自己拼的「删除成功」；
// * 「系统释放相应磁盘空间」（第 1312 行）那枚字节数住在产品那句话里，页上不数第二遍；
//   占用那一行 `historyWindowUsage()` 的算术与措辞唯一作者是 [`crate::settings::history_usage_line`]；
// * §8.7「提示用户清理历史或调整存储上限」那一问在占用**顶到上限**时才亮：那一行的字出自
//   `historyWindowAdvice()`（措辞唯一作者 [`crate::settings::history_pressure_line`]，「顶到没有」
//   那一判住在 core `Retention::at_cap`），没顶到就是空串、整格连高度一起没有；
// * 「清空全部历史前应二次确认」（第 1316 行）在这里是**两把闸**：页上先亮一问（这一页唯一的
//   状态机），真人点「确认清空」才轮到 `historyClear(true)`；点「取消」递出去的是 `false`，而页上
//   把这一下答的**产品原话**显示出来 ——「二次确认没点头，一行也不动。」窗自己吞掉那一次点击、
//   屏幕上什么都不说，就是让页与产品各说一句（#281 的反面）；
// * 「不得删除用户明确收藏或锁定的记录」（第 1317 行）里「用户明确锁定」那一半的对面是每一行
//   右边那枚「锁上 / 解开」，它叫的是 `historyLock(id, locked)`。
//
// 回放的五条腿也一次也不在这一页上判（§5.14.2 第 4 步「用户可重新复制、保存、标注或贴图」加
// §5.14.3 第 4 步「重新进入原选区」）：每一行右边那一排五枚钮，钮上的名字是 [`crate::history_window::ACTIONS`]
// 递来的原文，页上按**它在名单里排第几**决定递哪一只手 —— 页上自己抄一份「复制→哪条腿」的对照表，
// 就是给「钮上写着保存、递进产品的却是贴图」那一族缺陷长出第二个作者（#281）。名字认不出来 ⇒
// 一枚腿也不叫（[`crate::history_window::leg_of`] 同一口径：不许猜一条最像的）。落盘、写剪贴板、
// 建贴图、开遮罩，全在产品那一头；页上显示的仍是它交回的原话。
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
    /// 每一行右边那五枚动作钮的名字，与翻页那两枚的名字（名单的唯一作者是
    /// [`crate::history_window::ACTIONS`] 与 `FLIP_ROUTES`）。
    property var actions: []
    property var flipWords: []
    property string headerText: ""
    property string selectWord: ""
    property string askWord: ""
    property string yesWord: ""
    property string noWord: ""

    /// 「只看已取消」那一枚勾选框上的几个字（桥递回的原文，作者
    /// [`crate::history_window::CANCELLED_FILTER_WORD`]），与此刻那枚勾的状态。
    ///
    /// 这枚状态**只住在这里**：两记读清单的调用（[`win.readRows`] 与 [`win.fetchMore`]）递的都是
    /// 这一枚 property，Rust 那一头不替页上记一遍 —— `MaskState` 每开一扇遮罩就清一遍会话格子，
    /// 把勾寄在那种格子里就会有一趟「屏上那枚勾还亮着、清单却回到全量」，而从这扇窗按「标注」
    /// 或「重进选区」正是那样一趟。两记调用不同值时那句「清单到底了」就成了猜（#453）。
    property string cancelWord: ""
    property bool cancelledOnly: false

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

    /// §8.7 那一问：占用顶到上限时那句「清理或调上限」的话，没顶到时空串。与 `usage` 同一枚
    /// 读法（不绑 invokable、每次现读），因为「删过一行之后还顶不顶到」是每次都换的数。
    /// 整列的字都出自桥（`historyWindowAdvice()`），页上不拼一句、也不判「顶到没顶到」。
    property string advice: ""

    /// 那一格此刻画没画（高度是不是零）。门将来要读的是绑在本文件那枚 id 上的**值**，
    /// 而不是「桥那一格递了字符串」——递到了桥里不等于屏上有字。
    readonly property bool adviceDrawn: adviceBox.height > 0

    /// 一页多少行。绑**值**不抄数：唯一作者是 `history_window::PAGE_ROWS`，页上抄一个 `200`
    /// 就是给同一枚页宽第二个作者，改一天调页宽的人看得见桥、看不见这一行。
    property int pageSize: 0

    /// 更旧那一页读回来的话（`historyWindowListSentence()` 的原文），与 `tailed`：空串 ⇒
    /// 这一趟还没往尾巴上走过一次，屏上那一格连高度也没有。
    property string listLine: ""
    property bool tailed: false

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
    // §5.19.2 主题色：绑 `Session.accent` 那枚**值**（桥那侧已按此刻那套餐具夹过 3:1），`=== ""`
    // 那一臂只兜第一帧。三窗与此同一只口径，判据不许在这儿再写一遍（#281）。
    readonly property color accent_fill: win.session.accent !== ""
        ? win.session.accent
        : (dark ? "#4C8DF6" : "#2563B8")

    // §5.19.4 界面字体与字号：这一页上那几枚写死的 `font.pixelSize` 自此乘的是共享对象那一份
    // 倍数（`uiFont.px(...)`），页上不自己算 `ui_font ÷ 出厂磅`，也不自己兜空族 —— 四扇画着字的
    // 窗共读同一份（#281）。P149 之前这一格只长在设置页那一栋里，改档不动这一扇窗的像素。
    UiFont {
        id: uiFont
        session: win.session
    }

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
    ///
    /// 它同时把清单换回**第一页**：`tailed` 与那一行话跟着清掉。删过一行之后手里那份"到底了"
    /// 不再作数 —— 底下也许正空出一个位置，而页上不许拿上一趟的读数回答这一趟的问题。
    function readRows() {
        win.pageSize = Number(win.session.historyWindowPageSize())
        win.rows = win.splitPipe(win.session.historyRowsFiltered(win.pageSize, win.cancelledOnly))
        win.tailed = false
        win.listLine = ""
        win.usage = String(win.session.historyWindowUsage())
        // §8.7 那一问与占用同一趟读：两行问的是同一次 SQLite 的那一份占用，分两处读就会有一瞬间
        // 是「占用是新的、那一问是删之前那一趟的」。
        win.advice = String(win.session.historyWindowAdvice())
    }

    /// §7.1「分页或虚拟列表」里分页那一支：拿手里最后一行当游标，问一次「比它更旧的那一页」，
    /// 交回的每一行接在清单尾巴上 —— 游标因此自己往前走，页上不记第二枚"读到第几页"的状态。
    ///
    /// 两记读清单递的是同一枚 `win.cancelledOnly`：筛与不筛换的是**这一问**的口径，第一页与更旧
    /// 那一页若各说各的，那句「到底了」就不清楚是筛过的到底了还是全量到底了（#453）。
    ///
    /// 空串有几种可能（到底了 / 游标那一行已被删掉 / 库打不开），页上一次也不替它们猜：跟着
    /// 把桥那句话摆进 `win.listLine`，屏上显示的就是产品自己说的哪一种。「到底了」之后不再问
    /// （`tailed`），因为每一记滚动都去开一次 SQLite 就是把一扇窗变成一台轮询机。
    function fetchMore() {
        if (win.tailed || win.rows.length === 0)
            return ""
        const cursor = win.idOf(win.rows[win.rows.length - 1])
        const page = String(win.session.historyOlderRows(cursor, win.pageSize, win.cancelledOnly))
        win.listLine = String(win.session.historyWindowListSentence())
        if (page === "") {
            win.tailed = true
            return win.listLine
        }
        win.rows = win.rows.concat(win.splitPipe(page))
        return win.listLine
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

    /// 每一行右边那五枚钮的手：递的是哪一只手，全看这个名字在名单里排第几。认不出来的名字
    /// 一枚腿也不叫 —— 页上不许替它猜一条最像的，猜出来的那条腿与钮上的名字没有任何东西保证相等
    /// （与 `history_window::leg_of` 同一口径）。话仍由产品交回，页上只把它摆在这一行顶上。
    function doAction(word, id) {
        if (word === win.actions[0])
            win.note = String(win.session.historyCopy(id))
        else if (word === win.actions[1])
            win.note = String(win.session.historyReplay(id))
        else if (word === win.actions[2])
            win.note = String(win.session.historyPin(id))
        else if (word === win.actions[3])
            win.note = String(win.session.historyAnnotate(id))
        else if (word === win.actions[4])
            win.note = String(win.session.historyReenter(id))
        else
            return ""
        // 回放会往盘上落一张文件、贴图会多一枚窗，但**都不会**动库里那一行 —— 页上这一遍重读
        // 就是把「清单与占用有没有跟着变」这件事交给 SQLite 现读，而不是页上记一枚「应该没变」。
        win.readRows()
        return win.note
    }

    /// 翻页那两枚钮：往上一条还是往下一条。锚点**不由页上递进去**（`historyFlip` 头顶那段：
    /// 屏幕上那只框此刻停在第几行只有一个权威），这一扇窗上没有那只框，所以这一枚交回的是
    /// 产品自己那句「翻页没跑」或它翻成的那一行 —— 页上不替它编第二种答。
    function doFlip(back) {
        win.note = String(win.session.historyFlip(back))
        return win.note
    }

    /// 这一行是不是 §5.14.3 第 3 步要找的那一条「已取消」的记录：问的是桥。页上不许自己
    /// `contains` —— `monitor_id` 是自由文本，一块叫「已取消的屏」的显示器会把没取消的一行
    /// 标成取消过的，而用户点的正是那一枚「重进选区」。
    function cancelledOf(line) {
        return win.session.historyRowCancelled(line) ? 1 : 0
    }

    /// 那一枚行号此刻还在不在清单上。比的是**每一行头一段那枚数**整等，不是 `contains`：
    /// 第 17 行、`1700×700` 里的那枚 7，都不算第 7 行（与 `history_window::row_present` 同一把刀）。
    function presentOf(id) {
        for (let i = 0; i < win.rows.length; i++)
            if (win.idOf(win.rows[i]) === id)
                return 1
        return 0
    }

    /// 回放前后各读一遍的那三枚读数，粘成一串：占用那一行是**产品自己的原文**，页上不许把
    /// `original=` 与 `thumb=` 相加（那一枚加法在本档只有一个作者）；贴图枚数是 `session.count`
    /// 那一格现成的读数；在场那一枚问的是 [`win.presentOf`]。
    function shapeOf(id) {
        return win.usage + "|" + win.session.count + "|" + win.presentOf(id)
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
        // ① 第 1309 行「用户打开“历史记录管理”」：窗在、标题是那一串字、页顶四枚与行上五枚
        //    真造出来了、清单列了几行。行上那一排读的是**第一行 delegate 里**那一枚 Repeater 的
        //    `count`（`itemAtIndex` 交回的是真造出来的那一个对象，不是名单长度）。
        const first = list.itemAtIndex(0)
        win.report("open|" + (win.visible ? 1 : 0) + "|" + buttonRows.count + "|"
                   + (win.title === win.headerText && win.headerText.length > 0 ? 1 : 0) + "|"
                   + win.rows.length + "|" + (first ? first.actionCount : 0))

        // ② 第 1310 行「选择…全部记录」：表头那一枚点下去，勾上的枚数与清单枚数一并递回。
        win.selectAll(true)
        win.report("select-all|" + win.checkedCount() + "|" + win.rows.length)
        win.selectAll(false)

        // ⑨⑩⑪⑫ §5.14.2 第 4 步那三件事（复制 / 保存 / 贴图）与「回放不改写记录」那一枚不变量。
        //     三条腿走的是**行上那三枚钮同一只手**（`win.doAction`），所以「钮真在页上」与
        //     「这条腿真跑得动」在这一趟里是同一件事。名字认不出来的那一支不在这儿 ——
        //     标注与重进选区那两枚开的是一整扇遮罩，会把本进程数窗那一笔账改到别档的判据上，
        //     它们在页上长在每一行上、叫得动，由扫源码那一格钉住，不靠这一趟冒充跑过。
        //     跑在删那几格**之前**：计划点名的那一枚（`win.plan[1]`）到 ④ 就被删掉了。
        const target = Number(win.plan[1])
        const shapeBefore = win.shapeOf(target)
        win.report("replay-copy|" + target + "|" + win.doAction(win.actions[0], target))
        win.report("replay-save|" + target + "|" + win.doAction(win.actions[1], target))
        win.report("replay-pin|" + target + "|" + win.doAction(win.actions[2], target))
        win.report("replay-shape|" + shapeBefore + "|" + win.shapeOf(target))


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
                font.pixelSize: uiFont.px(12)
                font.family: uiFont.family
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: win.note
                color: win.word_fill
                font.pixelSize: uiFont.px(12)
                font.family: uiFont.family
                elide: Text.ElideRight
                width: parent.width - win.usage.length * 8 - 20
            }
        }

        // §8.7「提示用户清理历史或调整存储上限」：占用顶到上限才有的那一行。整串字出自桥
        // （`historyWindowAdvice()`），页上不拼一句、也不判顶到没顶到；没顶到时它是空串，于是
        // 整格连高度一起没有（下面清单的高度跟着把这 0 减掉，不空出一块白）。
        // 折行不省略：那一句要递的是「两条路 + 自动清理不动锁着的行」，截掉后半就成了一句
        // 「已到上限」——那是把一句话说成吓人的半句。
        Text {
            id: adviceBox

            width: parent.width
            text: win.advice
            visible: win.advice !== ""
            height: visible ? implicitHeight : 0
            color: win.word_fill
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
            wrapMode: Text.Wrap
        }

        // §7.1 分页那一支最后说的话：「读了 … 里更旧的一页：3 行（累计 203 行）」、「比第 42 行
        // 更旧的一页是空的，清单到底了」与「清单没读：… 」三种各是一句，全出自桥
        // （`historyWindowListSentence()`）。页上不替它们编：把「到底了」与「你手里那一行已经不
        // 在了」都说成"没有更多"，就是一句反话（#453）。没读过更旧那一页时它是空串，整格连高度
        // 一起没有 —— 一切正常不该有一行字。
        Text {
            id: listLineBox

            width: parent.width
            text: win.listLine
            visible: win.listLine !== ""
            height: visible ? implicitHeight : 0
            color: win.faint_fill
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
            wrapMode: Text.Wrap
        }

        // 表头那一枚全选与「只看已取消」那一枚筛：两个名字都出自桥，页上一个也不抄。
        Row {
            spacing: 14

            CheckBox {
                text: win.selectWord
                onToggled: win.selectAll(checked)
            }

            // §5.14.3 第 3 步「找到标记为“已取消”的记录」里**筛**那一半在这一页上的对面：勾上
            // 就把清单换成「行末写着那一字的哪些行」，递进 [`win.readRows`] 的是这一枚状态本身，
            // 筛住在查询里（`falcon_core::history` 那一记 WHERE），页上不读完再自己挑 —— 挑完就
            // 有一页只有几行、底下一句「到底了」，而那句此刻根本不是真的。
            CheckBox {
                text: win.cancelWord

                onToggled: {
                    win.cancelledOnly = checked
                    win.readRows()
                }
            }
        }

        Rectangle {
            width: parent.width
            height: 1
            color: win.edge_fill
        }

        // 清单。每一行：勾选框 + 那一行原文 + 一枚锁 + 那一行自己的五枚动作钮。行与字段那把刀由
        // Rust 递（上面那段口径），五枚钮的名字与次序由名单递（`win.actions`）。
        ListView {
            id: list
            width: parent.width
            height: parent.height - 150 - adviceBox.height - listLineBox.height
            clip: true
            model: win.rows
            spacing: 6
            // §7.1 分页那一支的入口：滚到尾巴上就问一次「比手里最后一行更旧的」。这一支不需要
            // 一枚钮，也就不需要一枚钮上的字 —— 页上此刻还没有一处写着"读取更早的记录"那样的中文。
            onAtYEndChanged: {
                if (atYEnd)
                    win.fetchMore()
            }
            delegate: Row {
                id: rowItem
                required property string modelData
                readonly property int rowId: win.idOf(modelData)
                /// 行上这一排此刻**真造出**几枚：`open` 那一格的第五枚读数读的就是这一枚
                /// （`list.itemAtIndex(0).actionCount`），名单长度不算造出来的东西 —— 与页顶
                /// 那枚 `buttonRows` 同一口径，`Row` 自己没有 `count`，所以 id 长在 Repeater 上。
                property int actionCount: rowActions.count
                spacing: 8
                CheckBox {
                    checked: win.picked.indexOf(win.idOf(modelData)) >= 0
                    onToggled: win.toggle(win.idOf(modelData))
                }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: modelData
                    // §5.14.3 第 3 步那一条「已取消」的记录，页上给的是**颜色**而不是第二个词：
                    // 字样已经在产品拼好的那一行里，页上再写一句就是长出第二种「取消了」。
                    color: win.cancelledOf(modelData) ? win.faint_fill : win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                    width: 300
                    wrapMode: Text.Wrap
                }
                Button {
                    anchors.verticalCenter: parent.verticalCenter
                    text: String(modelData).indexOf("已锁定") >= 0 ? "解开" : "锁上"
                    onClicked: win.toggleLock(win.idOf(modelData))
                }
                Repeater {
                    id: rowActions
                    model: win.actions
                    Button {
                        required property string modelData
                        anchors.verticalCenter: parent.verticalCenter
                        text: modelData
                        // 递的是**这一行**的行号：外层那一行原文在这枚钮的作用域里被名单遮住了，
                        // 所以从 `rowItem.rowId` 拿，而不是在这里再切一次原文（第二把刀）。
                        onClicked: win.doAction(modelData, rowItem.rowId)
                    }
                }
            }
        }

        // 翻页那两枚：§5.14.2 第 2 步「上一条、下一条」在这一扇窗上的第二个入口（遮罩上早就有
        // Ctrl+↑/↓ 那一对）。名字出自名单，两枚走的是同一条腿（`FLIP_ROUTES` 那一格钉的正是
        // 「两位名字、一条腿」，配成两条就有两个「什么算翻成了一页」）。
        Row {
            spacing: 10
            Repeater {
                id: flipRows
                model: win.flipWords
                Button {
                    required property string modelData
                    text: modelData
                    onClicked: win.doFlip(modelData === win.flipWords[0])
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
                font.pixelSize: uiFont.px(13)
                font.family: uiFont.family
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
        win.actions = win.splitPipe(win.session.historyWindowActions())
        win.flipWords = win.splitPipe(win.session.historyWindowFlipWords())
        win.selectWord = String(win.session.historyWindowSelectAll())
        win.cancelWord = String(win.session.historyWindowCancelledWord())
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
