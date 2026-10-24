import QtQuick
import QtQuick.Controls
import QtQuick.Window
import QtQuick.Dialogs
import dev.falconshot 1.0

// §5.20.1 的那一扇窗：页顶一排标签、页上那一列行、明确的保存与取消。
//
// 这份文件里**没有一份名单**。十一类叫什麼（`settingsCategories()`）、每一类哪几行、每行
// 写着什么值（`settingsPage()`），全都来自 [`crate::settings_view`] —— 它来自 `Config::flat()`
// 与 §5.20.1 那一列。这里再抄一份中文类名，就是第十二类与第一类并存的那一种坏（#281）；而
// 把 `key=value` 在本文件里从第一个 `=` 切开，是同一份语法的第二份实现，值里出现 `=`（保存
// 路径、字体名）当场切错 —— 所以点中一行拿的是 `settingsRowAt()` 交回的那一对。
//
// 这一扇窗也是本进程里唯一一扇**由 Rust 决定存在与否**的窗：`Session.settings_open` 那一格
// 是 0 就没有这个对象（`main.qml` 的 `syncSettings`），产品会话与另外七道门因此不会被一扇
// 没人要的顶层窗搅掉各自的窗清点 —— 这一条与 `HintWindow.qml` 头顶那封信同一个理由。
Window {
    id: win

    required property Session session

    /// `--settings-window` 那一档的计划，十一段（`settingsGaugePlan()` 原文）。空串 = 这一趟
    /// 没有量具的腿要跑：产品会话里真人自己点，七道门里整条躲开（同 `pinGaugePlan` /
    /// `hintGaugePlan` 那一族的口径 —— 量具的代码不许长成用户看得见的一次点击）。
    ///
    /// 只在 `Component.onCompleted` 读一次：Rust 那一头在 `engine.load()` 之前就把计划写好，
    /// 而绑一枚 invokable 的绑定只会跑一遍（`main.qml` 里 `warmRounds` 钉下的教训）。
    property var plan: []

    /// 页顶那十一枚标签，与本类那几行的**文本**。行文本是 `Row::text` 原文，所以
    /// `（需重启）` 那三个字在这里只是被画出来，不是在这里拼的。
    property var cats: []
    property var rowsText: []

    /// 当前是哪一类。开在哪一类由计划说了算（`--settings-window` 开在 §5.15 那一页，因为
    /// PRD 第 1328 行写的是「用户打开"首选项 > 热区"」）；产品会话从 0 开始。
    property int which: 0

    /// 选中那一行：它的行号、它管的那枚键。`pick` 是 -1 时输入框与那两枚按钮一律不动 ——
    /// 「没选中就按保存」这件事在屏幕上必须有对面，而它不是「写一枚不存在的键」。
    property int pick: -1
    property string pickKey: ""

    /// 状态行：只有 `Written::text` 那一整句，和取消那一句本文件自己的话。页上说的话与内存里
    /// 发生的事因此只能是同一句 —— 这一格不复述、不改写、不重新格式化。
    property string statusLine: ""

    /// §5.20.3 那一档给窗的计划，三段（腿等待 ms、导出落到哪份档、导入读哪份档）。空串在产品
    /// 会话与其余各档里，于是那两条量具腿整条不跑 —— 与上面那枚 `plan` 同一个口径。
    property var ioPlan: []

    /// 「将被覆盖的项目」那一格：桥那侧第 0 枚那句总数当抬头，其余画成一列行。页上画的与那句
    /// 总数出自同一次 `settingsImportPreview`，所以「画了几行」这件事对面是那份档本身。
    property var ioRows: []
    property string ioNote: ""

    /// 那两枚钮各自说的话（`config_export` / `config_import` 那一整句，原样印）。
    property string ioStatus: ""

    /// 真人从「导入配置」那张对话框选中的那份档，等下一次按「确认应用」时交给桥。留着的是
    /// `FileDialog` 交回的那串 URL 原文 —— 桥那一头 `save::local_path` 认它，也认计划里那条裸路径，
    /// 所以这一格不必知道自己是哪一趟。
    property string pendingImport: ""

    /// 计划够不够跑那几条腿。绑**值**不绑长度：`plan` 是本文件自己声明的属性，它的 notify 由
    /// Qt 生成（`HintWindow.qml` 那三封 `#316` / `#321` 的信教的形状）。
    readonly property bool armed: win.plan.length === 11

    /// §5.20.3 那一档够不够跑它那两条腿。与上面那一枚一样绑**值**，而段的数目是 3 不是 11：
    /// 两档共用这一扇窗，各看各的计划决定跑哪几条腿（`main.rs` 的 refusals 保证不会同时够长）。
    readonly property bool ioArmed: win.ioPlan.length === 3

    /// 这一页上此刻有没有可改的行。空页不是一扇「什么都没有」的窗：它是一枚点不开的入口。
    readonly property bool hasRows: win.rowsText.length > 0

    /// 「将被覆盖的项目」那一格此刻有没有东西可确认。没有列行就没有确认按钮可按 —— 与上面那一枚
    /// 同一个理由：§5.20.3 步骤 5 说的是「用户确认后才写入」，而确认的对象必须是画出来的那几行。
    readonly property bool hasIoRows: win.ioRows.length > 0

    // §5.19.1 的那套餐具：两枚完整切换的（窗体、卡片、正文、强调），深色浅色各一套。绑的是
    // `Session.theme` 那枚**值**，notify 一到就重新求值 —— 与 `HintWindow.qml` 同一族（靠绑定，
    // 不靠 `main.qml` 里那几条造窗腿的次序，因为这扇窗是 `createObject` 造出来的）。
    readonly property bool dark: win.session.theme === "dark"
    readonly property color page_fill: dark ? "#12151B" : "#F2F4F7"
    readonly property color card_fill: dark ? "#1B1F27" : "#FFFFFF"
    readonly property color word_fill: dark ? "#F2F4F7" : "#12151B"
    readonly property color edge_fill: dark ? "#39424F" : "#B4BEC9"
    readonly property color pick_fill: dark ? "#2C3D57" : "#DCE7F5"
    readonly property color accent_fill: dark ? "#4C8DF6" : "#2563B8"

    // 一扇正常的窗：有边框、进任务栏、接键盘。§5.20.1 说的是「设置页」，不是一个盖在桌面上的
    // 浮层，所以这里**不**写 `flags`：`Qt.Tool` / `FramelessWindowHint` 属于贴图与提示窗那一族。
    title: "falconshot-settings"
    width: 760
    // 520 → 700：这一扇窗现在下面多了一整节（两枚钮、那句总数、那一列行、两枚确认）。原来的
    // 300 行高随之收到 180 —— 这一节里没有任何一格是判据（量的都是 `visible`、类数、行数），
    // 尺寸改动的对面是「页上的东西在不在屏幕上」这一件 P142 也欠着的事。
    height: 700
    color: win.page_fill

    /// 一行一串地切开。**空串切出来是 `[""]`，长度 1** —— 这一枚函数存在的理由就是那个 1：
    /// 一页空行在窗上会长成一格写着空串的行，而「这一类有没有行」正是 §5.20.1 那一页要判的事。
    function splitLine(s) {
        const text = String(s)
        return text === "" ? [] : text.split("\u{1d}")
    }

    /// 计划那一串的分法，和页/类那两串**不一样**，所以这里有两枚函数而不是一枚：Rust 那边
    /// `settingsCategories()` / `settingsPage(i)` 交的是 `RECORD`（`\u{1d}`）连接的行，而
    /// `settingsGaugePlan()` 交的是 `|` 连接的十一段（`settings_view::arm` 拼的就是这一串，
    /// `[settings-window arm]` 那一行印的也是它）。gen A 与紧跟着的探针（`p142_probe_debug_A.log`）
    /// 量到的是把计划喂给了行那把刀：`born=["1|1"]` —— 一段、窗可见，于是 `armed` 不成立，那三条腿
    /// 一整趟没跑。段里若真出现 `|`，段数就不对，`armed` 直接不成立、门当场少一行可读，不会假装成
    /// 一次成功的读。
    function splitPlan(s) {
        const text = String(s)
        return text === "" ? [] : text.split("|")
    }

    /// 重新问一次桥：第 `i` 类那几行现在长什么样。腿里每读一次「页上那一行」都走这一枚，
    /// 而不是拿 `rowsText` 的旧快照 —— 这一格要的对面恰恰是「改完之后页跟着变了没有」。
    function pageText(i) {
        return win.splitLine(win.session.settingsPage(i))
    }

    /// 把当前类的行取进 `rowsText`：页上画的是这一枚，本档递回的也是这一枚。
    function refreshPage() {
        win.rowsText = win.pageText(win.which)
    }

    /// 点中第 `i` 行：键与当前值从 `settingsRowAt` 那一对里来（不在这里切 `=`），输入框填上
    /// 当前值 —— 真人改一档时看到的就是那一枚键此刻的数。
    function selectRow(i) {
        const pair = win.splitLine(win.session.settingsRowAt(win.which, i))
        if (pair.length !== 2) {
            win.pick = -1
            win.pickKey = ""
            field.text = ""
            return
        }
        win.pick = i
        win.pickKey = pair[0]
        field.text = pair[1]
        win.statusLine = "选中 " + pair[0]
    }

    /// 保存：把输入框那串交过桥（闸门、落盘、`Written::text` 都在 Rust 那侧），再把这一页
    /// 重新取一次 —— 页上那一格跟着值走，这件事不靠任何一处 `property alias` 的运气。
    function saveDraft() {
        if (win.pick < 0) {
            win.statusLine = "没有选中任何一行"
            return
        }
        win.statusLine = String(win.session.settingsSet(win.pickKey, field.text))
        win.refreshPage()
    }

    /// 取消：草稿丢掉，一个字节也不递过桥。这一枚与 `saveDraft` 的分别是「调不调
    /// `settingsSet`」—— 这就是 §5.20.2 第三条那一支（明确的保存与取消按钮）全部的对面。
    function cancelDraft() {
        if (win.pick < 0) {
            win.statusLine = "没有选中任何一行"
            return
        }
        win.refreshPage()
        win.selectRow(win.pick)
        win.statusLine = "取消：草稿未写入"
    }

    // ── §5.20.3 的那两枚钮背后的手 ────────────────────────────────────────────
    //
    // 三枚函数页上那两枚钮与量具那两条腿**共用**：按下去调的就是这一批，这里不另写一份导出码
    // （#281）。传进去的目标路径有两种长相 —— 真人从 `FileDialog` 拿到的 `file:///C:/…`，和计划
    // 里那条裸路径 —— Rust 那一头 `save::local_path` 两种都认，所以桥上不必分岔。

    /// 「导出配置」那一枚（步骤 1 与 2）：交回的是 `config_export` 那一整句，页上只是把它画出来。
    function doExport(target) {
        const line = String(win.session.settingsExportConfig(target))
        win.ioStatus = line
        return line
    }

    /// 「导入配置」那一枚的第一步（步骤 3 与 4）：把那份档「会盖掉哪几枚」取进页上那一列行。
    /// 切开用的是 `settingsPage` 同一把刀（`RECORD`），第 0 枚是那句总数、其余才是行。读不出那份
    /// 档时桥交回**一句**而没有行：那一格在页上是「一行也没画出来 + 那句话」，不是空列表 ——
    /// 「没得盖掉」与「那份档读不出」是两种坏，这里不把它们混成一句自己的话。
    function doImportPreview(target) {
        win.pendingImport = target
        const parts = win.splitLine(win.session.settingsImportPreview(target))
        win.ioNote = parts.length > 0 ? String(parts[0]) : ""
        win.ioRows = parts.slice(1)
        return parts
    }

    /// 步骤 5：确认之后才真写。走的是 `--import-config --confirm` 那同一只手，所以「页上点了确
    /// 认」与「命令行加了 --confirm」之间不存在两套落盘码。写完那一列行就收起来：它说的是刚才
    /// 那一份档，而那份档已经不再是此刻的状态。
    function doImportApply() {
        const line = String(win.session.settingsImportApply(win.pendingImport))
        win.ioStatus = line
        win.pendingImport = ""
        win.ioRows = []
        win.ioNote = ""
        // 页上那一列行里可能有刚被覆盖掉的那一枚键的值：重新取一次，别让它停在按之前的读数上。
        win.refreshPage()
        return line
    }

    /// 不应用：那一列行收起来，一个字节也不递过桥。这一枚与 `doImportApply` 的分别是「调不调
    /// `settingsImportApply`」—— 与上面 `cancelDraft` 同一支（明确的保存与取消）。
    function cancelImport() {
        win.pendingImport = ""
        win.ioRows = []
        win.ioNote = ""
        win.ioStatus = "不应用：那份档一项也没写"
    }

    /// `--settings-io` 那两条腿。报回的每一行前缀是那格的名字，其余原样存进 Rust 那一侧的账。
    ///
    /// 报的是**窗那一侧的读数**：`io-open` 里那枚 visible 是窗自己问 Qt 的，`preview` 里那个数是
    /// 页上真的画了几行（`ioRows.length`，不是桥交回了几枚）。路径两段原样回递 —— 判的那件事是
    /// 「页上按的是计划里那两份档」，而不是「那句话写得像不像」。
    function runIoGauge() {
        win.session.settingsGaugeReport(
            "io-open|" + win.which + "|" + (win.visible ? 1 : 0) + "|" + win.cats.length)
        win.session.settingsGaugeReport(
            "export|" + win.ioPlan[1] + "|" + win.doExport(win.ioPlan[1]))
        const parts = win.doImportPreview(win.ioPlan[2])
        win.session.settingsGaugeReport(
            "preview|" + win.ioRows.length + "|" + String(parts.length > 0 ? parts[0] : ""))
        win.session.settingsGaugeReport(
            "apply|" + win.ioPlan[2] + "|" + win.doImportApply())
        // 与上面那一档同一个次序：先写 0 再递读数，销毁排在下一环（`main.qml` 那枚 `interval: 0`
        // 的 Timer），所以读数在这一句里还取得到，而窗随后真的没了。
        win.session.settings_open = 0
        win.session.settingsGaugeReport("io-close|" + win.session.settings_open)
        win.hide()
    }

    // ── `--settings-window` 那三条腿 ──────────────────────────────────────────
    //
    // 每条腿走的都是**真人的那一只手**：选中一行、往输入框里打一串、按保存（或取消）。
    // 按下去调的是上面那两枚函数，不是这里另写的一份 —— 量具踩的路与用户踩的路必须是同一条。
    //
    // 报回的那一行是 `种类|键|打进去的值|按之前页上那一行|按之后页上那一行`：本档判的是**那两格
    // 文本**（与 Rust 自己算出来的样子对），不是这里说的话。值与行号两样都核对过才动手：
    // 指错一行去写值是比不写更坏的那一种，所以那种情况下整条腿不跑，只递回一行 `abandon`。
    //
    // 行文本里出现 `|` 会把报回的那一行撑成六段、当场判红。这一页上那三枚键的值全是数字或
    // 枚举名，撑不出来；真撑出来了，红得也对 —— 那一格本来就是在问「页上那一行是什么」。
    function legRow(kind, idx, key, value, commit) {
        const i = Number(idx)
        win.refreshPage()
        const pair = win.splitLine(win.session.settingsRowAt(win.which, i))
        if (pair[0] !== key) {
            win.session.settingsGaugeReport(
                kind + "|abandon|row=" + i + "|key=" + pair[0] + "|want=" + key)
            return
        }
        const before = String(win.rowsText[i] || "")
        win.selectRow(i)
        field.text = value
        if (commit)
            win.saveDraft()
        else
            win.cancelDraft()
        const after = String(win.rowsText[i] || "")
        win.session.settingsGaugeReport(kind + "|" + key + "|" + value + "|" + before + "|" + after)
    }

    /// 十一页逐页过一次桥。这一条才是 §5.20.1 那一份名单在**窗上**的读数：类数、每类几行，
    /// 数的是窗自己拿到的那些串，而不是 Rust 手里那份清单。
    function censusPages() {
        for (let i = 0; i < win.cats.length; ++i)
            win.session.settingsGaugeReport("page|" + i + "|" + win.pageText(i).length)
    }

    function runGauge() {
        win.session.settingsGaugeReport(
            "open|" + win.which + "|" + (win.visible ? 1 : 0) + "|" + win.cats.length + "|"
            + win.rowsText.length)
        win.censusPages()
        // 顺序是定的：保存、还原前的值不归本文件管（Rust 读完回来自己改回去），取消与拒那两腿
        // 各点各的键，所以三腿互不改对方的前置。
        win.legRow("save", win.plan[2], win.plan[3], win.plan[4], true)
        win.legRow("cancel", win.plan[5], win.plan[6], win.plan[7], false)
        win.legRow("refuse", win.plan[8], win.plan[9], win.plan[10], true)
        // 关闭那一腿先写 0 再递读数：写是**同步** notify，`main.qml` 的销毁排在下一环（那枚
        // `interval: 0` 的 Timer），所以读数在这一句里还取得到，而窗随后真的没了。
        win.session.settings_open = 0
        win.session.settingsGaugeReport("close|" + win.session.settings_open)
        win.hide()
    }

    Column {
        x: 16
        y: 14
        width: parent.width - 32
        spacing: 10

        Text {
            text: "首选项"
            color: win.word_fill
            font.pixelSize: 18
            font.bold: true
        }

        // 页顶那一排标签：十一枚，一枚一类，顺序就是 §5.20.1 里那一列数字。
        Flow {
            width: parent.width
            spacing: 6

            Repeater {
                model: win.cats

                Rectangle {
                    // `index` / `modelData` 走 Repeater 的上下文属性，与本仓库里已经跑过 qmllint
                    // 且退出码为 0 的那几处同一个写法（`PinWindow.qml` 分组那一列读的就是裸
                    // `modelData`）。不写成 `required property`：那一条路在这里没有对面跑过，
                    // 而一枚写错的必需属性是**当场放弃创建**，窗会安静地什么都不画。

                    width: tabWord.implicitWidth + 20
                    height: 28
                    radius: 6
                    color: win.which === index ? win.pick_fill : win.card_fill
                    border.color: win.which === index ? win.accent_fill : win.edge_fill
                    border.width: 1

                    Text {
                        id: tabWord

                        anchors.centerIn: parent
                        text: String(modelData)
                        color: win.word_fill
                        font.pixelSize: 13
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            win.which = index
                            win.pick = -1
                            win.pickKey = ""
                            field.text = ""
                            win.refreshPage()
                        }
                    }
                }
            }
        }

        Text {
            text: win.cats.length > 0 ? String(win.cats[win.which]) + "（" + win.rowsText.length + " 行）" : ""
            color: win.word_fill
            font.pixelSize: 13
            opacity: 0.75
        }

        // 那一列行。`Flickable` 而不是 `ListView`：本页的行是 `settingsPage()` 一次交回的
        // 一串字，Repeater 的 model 拿一个 JS 数组就够，而一行的样子由 `Row::text` 说了算。
        Flickable {
            width: parent.width
            height: 180
            contentHeight: rowList.height
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: rowList

                width: parent.width
                spacing: 4

                Repeater {
                    model: win.rowsText

                    Rectangle {
                        width: rowList.width
                        height: rowWord.implicitHeight + 14
                        radius: 5
                        color: win.pick === index ? win.pick_fill : win.card_fill
                        border.color: win.pick === index ? win.accent_fill : win.edge_fill
                        border.width: 1

                        Text {
                            id: rowWord

                            anchors.verticalCenter: parent.verticalCenter
                            anchors.left: parent.left
                            anchors.leftMargin: 10
                            anchors.right: parent.right
                            anchors.rightMargin: 10
                            text: String(modelData)
                            color: win.word_fill
                            font.pixelSize: 13
                            elide: Text.ElideMiddle
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: win.selectRow(index)
                        }
                    }
                }
            }
        }

        Text {
            visible: !win.hasRows
            text: "这一类没有可改的行"
            color: win.word_fill
            opacity: 0.6
            font.pixelSize: 12
        }

        Row {
            spacing: 8

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "新值"
                color: win.word_fill
                font.pixelSize: 13
                visible: win.pick >= 0
            }

            TextField {
                id: field

                width: 360
                visible: win.pick >= 0
                placeholderText: "改这一枚键的值"
                onAccepted: win.saveDraft()
            }

            Rectangle {
                width: saveWord.implicitWidth + 22
                height: 30
                radius: 6
                visible: win.pick >= 0
                color: win.accent_fill

                Text {
                    id: saveWord

                    anchors.centerIn: parent
                    text: "保存"
                    color: "#FFFFFF"
                    font.pixelSize: 13
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.saveDraft()
                }
            }

            Rectangle {
                width: cancelWord.implicitWidth + 22
                height: 30
                radius: 6
                visible: win.pick >= 0
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: cancelWord

                    anchors.centerIn: parent
                    text: "取消"
                    color: win.word_fill
                    font.pixelSize: 13
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.cancelDraft()
                }
            }
        }

        Text {
            width: parent.width
            text: win.statusLine
            color: win.word_fill
            opacity: 0.85
            font.pixelSize: 12
            elide: Text.ElideRight
            maximumLineCount: 1
        }

        // ── §5.20.3：导出配置 / 导入配置 那两枚钮，与「将被覆盖的项目」那一格 ──────────
        //
        // 这枚区块在产品会话里**一直看得见** —— PRD 那一条说的是设置页上的两只手，不是量具专用的
        // 一层皮。因此它没有 `visible: win.ioArmed` 那一枚开关：跑不跑腿由下面那枚 Timer 决定，
        // 看得见看不见由配置决定。按下去调的是上面那三枚函数，与量具同一条路。
        Row {
            spacing: 8

            Rectangle {
                width: exportWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: exportWord

                    anchors.centerIn: parent
                    text: "导出配置"
                    color: win.word_fill
                    font.pixelSize: 13
                }

                // 钮自己只开一张对话框：落到哪份档、写出几枚键，全在对话框与 Rust 那一头。
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: exportDialog.open()
                }
            }

            Rectangle {
                width: importWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: importWord

                    anchors.centerIn: parent
                    text: "导入配置"
                    color: win.word_fill
                    font.pixelSize: 13
                }

                // 选完档当场只做一件事：把「会盖掉哪几枚」问出来画在下面。「导入即写」在这里是
                // 没有的 —— 写要下面那枚「确认应用」按下去才发生（§5.20.3 步骤 5）。
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: importDialog.open()
                }
            }
        }

        Text {
            visible: win.ioNote !== ""
            text: win.ioNote
            color: win.word_fill
            opacity: 0.75
            font.pixelSize: 12
            elide: Text.ElideRight
            maximumLineCount: 1
        }

        // 「将被覆盖的项目」那一列。抬头那句总数与这些行出自同一次 `settingsImportPreview`，
        // 所以「画了几行」这件事的对面是那份档，不是本页写的一份名单（#281）。
        Flickable {
            visible: win.hasIoRows
            width: parent.width
            height: 84
            contentHeight: ioList.height
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: ioList

                width: parent.width
                spacing: 3

                Repeater {
                    model: win.ioRows

                    Rectangle {
                        width: ioList.width
                        height: ioRowWord.implicitHeight + 8
                        radius: 4
                        color: win.card_fill
                        border.color: win.edge_fill
                        border.width: 1

                        Text {
                            id: ioRowWord

                            anchors.verticalCenter: parent.verticalCenter
                            anchors.left: parent.left
                            anchors.leftMargin: 8
                            anchors.right: parent.right
                            anchors.rightMargin: 8
                            text: String(modelData)
                            color: win.word_fill
                            font.pixelSize: 12
                            elide: Text.ElideMiddle
                        }
                    }
                }
            }
        }

        Row {
            visible: win.hasIoRows
            spacing: 8

            Rectangle {
                width: applyWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.accent_fill

                Text {
                    id: applyWord

                    anchors.centerIn: parent
                    text: "确认应用"
                    color: "#FFFFFF"
                    font.pixelSize: 13
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.doImportApply()
                }
            }

            Rectangle {
                width: dropWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: dropWord

                    anchors.centerIn: parent
                    text: "不应用"
                    color: win.word_fill
                    font.pixelSize: 13
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.cancelImport()
                }
            }
        }

        Text {
            visible: win.ioStatus !== ""
            width: parent.width
            text: win.ioStatus
            color: win.word_fill
            opacity: 0.85
            font.pixelSize: 12
            elide: Text.ElideRight
            maximumLineCount: 1
        }

        Rectangle {
            width: closeWord.implicitWidth + 22
            height: 30
            radius: 6
            color: win.card_fill
            border.color: win.edge_fill
            border.width: 1

            Text {
                id: closeWord

                anchors.centerIn: parent
                text: "关闭"
                color: win.word_fill
                font.pixelSize: 13
            }

            // 关窗那一枚写的是 `settings_open`，不是 `visible`：谁造这扇窗，销毁就归谁。
            // 这里 `hide()` 一句也不写 —— 写完 0 之后 `main.qml` 那一条腿会 `close()` 再
            // `destroy()`，把「关掉」这件事只留一处做（#281）。
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: win.session.settings_open = 0
            }
        }
    }

    Component.onCompleted: {
        win.cats = win.splitLine(win.session.settingsCategories())
        win.plan = win.splitPlan(win.session.settingsGaugePlan())
        win.ioPlan = win.splitPlan(win.session.settingsIoPlan())
        if (win.armed)
            win.which = Number(win.plan[1])
        win.refreshPage()
        // 场上与场下只有 `show()` 与 `hide()` 两个入口，`visible` 一处也不写 —— 与
        // `HintWindow.qml` 同一封报错信（`p54_hint_d.log`：赋值之后 visible 仍是 false）。
        win.show()
        // 这一行不是判据，是**到此一处的记号**：`--settings-window` 那一档在窗没递回任何一行时，
        // 得能分清「窗压根没被造出来」与「窗造出来了、那三条腿没跑」。前两趟只能印前一句
        // （`p142_alone_A.log`），而这两种坏要修的是不同的码。`born` 不进任何一格的判定。
        //
        // 第三段是 P143 加的：这一扇窗现在读两副计划，`born|11|1` 与 `born|0|1` 在 `--settings-io`
        // 那一趟里说的是两种坏（窗读到了窗的那份计划、和窗压根没读到 io 那份计划）。
        win.session.settingsGaugeReport(
            "born|" + win.plan.length + "|" + win.ioPlan.length + "|" + (win.visible ? 1 : 0))
    }

    // 量具那三条腿：等窗真的在桌面上（`LEG_MS` 由计划给），然后一次跑完。
    Timer {
        id: gaugeLeg

        interval: Number(win.plan[0]) || 400
        repeat: false
        running: win.armed
        // `tick` 是这一枚 Timer 真的走到过的记号：gen A/B 与探针三趟都是「窗在场（`born`）、计划
        // 够长（11 段），可 `open` 一行也没有」。把「Timer 没触发」与「`runGauge` 半路抛」这两种
        // 坏分开印，才谈得上修哪一个。它不进任何一格的判定。
        onTriggered: {
            win.session.settingsGaugeReport("tick|" + win.plan.length)
            win.runGauge()
        }
    }

    // §5.20.3 那两条腿：与上面那一枚**同一形状、另一副计划**。两档共用这一扇窗，各自的计划够长
    // 才跑各自的腿；同一趟里两副计划不会都够长（`main.rs` 的 refusals 把它们钉成互斥），所以这里
    // 不需要「先跑哪一条」那一层排序 —— 那一层一旦写出来就是第二个权威（#281）。
    Timer {
        id: ioLeg

        interval: Number(win.ioPlan[0]) || 400
        repeat: false
        running: win.ioArmed
        // 与 `tick` 同一个理由：把「这一枚 Timer 没触发」与「`runIoGauge` 半路抛」分开印。
        // 它不进任何一格的判定。
        onTriggered: {
            win.session.settingsGaugeReport("io-tick|" + win.ioPlan.length)
            win.runIoGauge()
        }
    }

    // 「导出配置」那张对话框：落到哪份档由对话框决定，本页只把那串 URL 原样递过桥 —— 与
    // `PinWindow.qml` 里 `saveAs` 同一族（这一台 Qt 上 `selectedFile.toLocalFile()` 量到的是
    // `undefined`，P37；`save::local_path` 是全场唯一一处把 `file:///…` 读回路径的手）。
    FileDialog {
        id: exportDialog

        title: "导出配置"
        fileMode: FileDialog.SaveFile
        nameFilters: ["TOML 配置 (*.toml)"]
        defaultSuffix: "toml"
        onAccepted: win.doExport(String(exportDialog.selectedFile))
    }

    // 「导入配置」那张对话框：选完档当场只做「会盖掉哪几枚」这一问，写入要下面那枚「确认应用」
    // 按下去才发生（§5.20.3 步骤 5）。这里没有第二次 `open()`：真人按钮就是开这张对话框，别的
    // 什么都不按。
    FileDialog {
        id: importDialog

        title: "导入配置"
        fileMode: FileDialog.OpenFile
        nameFilters: ["TOML 配置 (*.toml)", "所有文件 (*)"]
        onAccepted: win.doImportPreview(String(importDialog.selectedFile))
    }
}
