import QtQuick
import QtQuick.Controls
import QtQuick.Window
import QtQuick.Dialogs
import dev.falconshot 1.0

// §5.20.1 的那一扇窗：页顶一排标签、页上那一列行、明确的保存与取消。
//
// 这份文件里**没有一份名单**。那几类叫什麼（`settingsCategories()`）、每一类哪几行、每行
// 写着什么值（`settingsPage()`）、**哪一行要画成一枚开关**（`settingsRowKinds()`），全都来自
// [`crate::settings_view`] —— 它来自 `Config::flat()`
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

    /// `--settings-window` 那一档的计划，十四段（`settingsGaugePlan()` 原文）。空串 = 这一趟
    /// 没有量具的腿要跑：产品会话里真人自己点，七道门里整条躲开（同 `pinGaugePlan` /
    /// `hintGaugePlan` 那一族的口径 —— 量具的代码不许长成用户看得见的一次点击）。
    ///
    /// 只在 `Component.onCompleted` 读一次：Rust 那一头在 `engine.load()` 之前就把计划写好，
    /// 而绑一枚 invokable 的绑定只会跑一遍（`main.qml` 里 `warmRounds` 钉下的教训）。
    property var plan: []

    /// 页顶那一排标签，与本类那几行的**文本**。行文本是 `Row::text` 原文，所以
    /// `（需重启）` 那三个字在这里只是被画出来，不是在这里拼的。
    property var cats: []
    property var rowsText: []

    /// 本类那几行各自要哪一只手：`on` / `off` / `field`，与 `rowsText` **同一次取、同长度**。
    ///
    /// 这一枚存在的原因是 §10 第 150 项③那句「量具的手 ≠ 用户的手」的另一半：页上过去只有一种
    /// 格子（一行文本 + 一枚输入框），于是 bool 那几枚键在屏幕上没有它自己的形状。本文件**不猜**
    /// 哪一行是开关 —— 按 `value === "true"` 猜会把一枚恰好写着 `true` 的字体名画成开关，按下
    /// 去就写掉了那一格（而那一步在语法闸上是过得去的）。判的是档里存着的类型，出自
    /// `settingsRowKinds()`。
    property var rowsKind: []

    /// 当前是哪一类。开在哪一类由计划说了算（`--settings-window` 开在 §5.15 那一页，因为
    /// PRD 第 1328 行写的是「用户打开"首选项 > 热区"」）；产品会话从 0 开始。
    property int which: 0

    /// §5.21.2 第四枚：这一页此刻欠着的那一句「首次使用引导」。空串 = 桥答「不递」，
    /// 下面那一块连高度一起没有。
    ///
    /// 窗**不**判「哪一页算复杂」「这一页递过没有」—— 那两件事住在 `crate::guide` 那张表与
    /// 那份文件里（#281）。这一枚只是每一次翻页重新问过桥之后，留在窗手里的那句原文。
    property string guideText: ""

    /// 「知道了」那枚钮上此刻该写的字，桥答的那一份原文（`settingsGuideAckWord()`）。
    ///
    /// 它存在的原因与 `rowsKind` 一模一样：那一句话里写着「按「知道了」之后……」，而钮上写的字
    /// 是页上另一处 —— 两处各抄一遍，改一处就会留下「句里说的字与钮上的字不是同一枚」这种坏，
    /// 而它只在屏幕上露出来。所以字从桥取一次，页上只画它；量具再比「画出来的」与「此刻桥说的」
    /// 是不是同一串（`legGuide` 那七段之外的两枚）。
    property string ackWordText: ""

    /// 页上那一块**此刻**画没画、块里那串字此刻写着什么、钮上那两个字此刻写着什么。三枚都是
    /// `readonly` 绑到本文件那三枚 id 上的**值**：腿里读的是它们，而不是 `win.guideBox` 那种
    /// 根本不存在于窗上的属性（P160 第一趟门就是这么红的：`win.guideBox.visible` 在 391 行抛
    /// TypeError，后面四条写值的腿一条也没跑）。
    readonly property bool guideDrawn: guideBox.visible
    readonly property string guidePainted: guideWord.text
    readonly property string ackWordDrawn: ackWord.text

    /// §5.21.1 第 2 与第 4 步在页脚的那两枚钮名：桥交回的两段原文（`updateWords()`）。空数组 =
    /// 这一页压根不是「关于与更新」，于是那一块连高度都没有。窗一处也不写这两枚字，也不数类号。
    property var updateWords: []

    /// §5.21.3 那一块要写的每一枚字，桥递回的那一列原文（`feedbackWords()`，七枚，次序由
    /// `feedback_window::ON_PAGE` 说了算）。空数组 = 这一页压根不是「关于与更新」，于是那一块
    /// 连高度一起没有。与 `updateWords` 同一族：本文件一处也不写这几枚字、也不数类号。
    property var feedbackWords: []

    /// 这一块展开了没有：第 1 步那一次点击之后才是 1，收起那一句回 0。它管的只有**画不画**那三步：
    /// 递不递东西、那一格空着要不要拒，全都住在桥那两手后面，窗一处也不判（#281）。
    property bool feedbackOpen: false

    /// 第 2 步的产物：桥现生成、现脱敏的那几行诊断（`feedbackAsk()` 交回的原文）。窗不裁它、
    /// 不数它、也不在它前面再拼一句「以下是诊断」—— 那一格的抬头也是桥递的字。
    property string feedbackDiag: ""

    /// 「此刻按下去会递出去的那一段字」：`feedbackPreview()` 交回的原文，与「发送」那一手共用的
    /// 正是同一只手，所以预览与实际出去的东西不会是两份读数。
    property string feedbackText: ""

    /// §5.16.2 规则第二句「应提供当前前台程序快速添加功能」那一枚钮上此刻该写的字，桥交回的原文
    /// （`settingsExclusionWord()`）。空串 = 这一页压根不是「快捷键」，下面那一块连高度一起没有。
    /// 与 `updateWords` 同一族：本文件一处也不写这几个字、也不数类号 —— 窗里抄一份措辞，改一天
    /// Rust 那一头就得留一句页上写着旧字。
    property string exclusionWord: ""

    /// §8.4 第一句「显示冲突的组合键和相关功能」在页上的那一列：桥交回的原文
    /// （`settingsConflicts()`）按 `RECORD` 切好的每一行。空数组有两种读法，窗对两样的画法一样：
    /// 这一页压根不是「快捷键」，或者这一趟一行也没什么要用户看（出厂那九枚全成）。
    property var conflictLines: []

    /// 那一列此刻画没画、画了几行。与 `updateBlockDrawn`、`guideDrawn` 同一族：腿里读的是绑到
    /// 本文件那两枚 id 上的**值** —— 「桥那一格递了几行」不等于「屏上有那几行」。
    readonly property bool conflictBlockDrawn: conflictBox.visible
    readonly property int conflictRowCount: conflictList.count

    /// 上一次「检查更新」**落地**那一条线的第二段：那枚入口的地址。空串 = 那一次没有入口，那枚钮
    /// 画不出来；它绑的是桥投回来的那一格（`update_line`），不是本文件另数出来的一份「有没有新版本」，
    /// 也不是窗自己再问一次桥。
    property string updateDownloadUrl: ""

    /// 页脚那一块此刻画没画、那枚入口此刻画没画、两枚钮上此刻写着什么。与上面 `guideDrawn` 同一族：
    /// 门读的是绑到本文件那几枚 id 上的**值** —— 「桥里那一格递了两枚字」不等于「屏上有那枚钮」。
    readonly property bool updateBlockDrawn: updateRow.visible
    readonly property bool updateEntryDrawn: dlBtn.visible
    readonly property string updateCheckPainted: checkWord.text
    readonly property string updateEntryPainted: dlWord.text

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

    /// §5.7.22 那一档给窗的计划，十二段（腿等待 ms、开在哪一类、新增那枚色、删的行号、挪的行号、
    /// 挪的方向、改色的行号、改成的色、保存用的方案名、应用哪一枚方案、导出落到哪份档、导入读哪份
    /// 档）。与上面两副计划同一个口径：产品会话里是空串，那一档的腿整条不跑。最后两段是**裸路径**，
    /// 而页上那两枚钮从 `FileDialog` 拿到的是 `file:///…` —— `save::local_path` 两种都认，所以窗
    /// 不必知道自己此刻是哪一趟。
    property var palPlan: []

    /// 页上那一排色：一枚一行 `序号|色`，画的是**桥现读**的那份名单（`settingsPaletteBand`），
    /// 不是本文件抄的一份数组 —— 加一枚、删一枚、挪一枚，页上跟着长的就是档里落下的那一排。
    property var palBand: []

    /// 方案那一列：一枚一行 `名字|枚数|色 色 …`，同一个理由。
    property var palSchemes: []

    /// 这一格上的字：第 0 枚是抬头，后面十二枚是那十二枚钮。窗一处也不自己写措辞（#281）——
    /// 与 `updateWords` 同一族，改一天说法只需要动 Rust 那一头。
    property var palWords: []

    /// 那几只手各自说的话：交回的是 `Written::text` 那一整句，原样印、不复述。
    property string palStatus: ""

    /// 「导入调色板」那一步在页上的两格读数：抬头那一句（那份档自己说的「读到几枚、换成哪几枚」，
    /// 或「读不出那份档」那一句）与下面那一列**会被换成的色**。与上面 `ioNote` / `ioRows` 同一族：
    /// §5.7.22 步骤 5 写的是「用户确认后写入」，而确认的对象必须是画出来的那几枚。
    property string palIoNote: ""

    /// 同一趟的第二格。一行一枚，画的是桥交回的那几行，不是本页抄的一份名单。
    property var palIoRows: []

    /// 真人从「导入调色板」那张对话框选中的那份档，等下一次按「确认应用这一份」时交给桥。与上面
    /// 那枚 `pendingImport` 各管各的：两扇对话框、两双手，混用一格就会有一趟的确认写进另一趟。
    property string pendingPalIoImport: ""

    /// 选中哪一枚**色**（0 起，页上那一排的行号）。`-1` 时「删除 / 上移 / 下移 / 改这枚」四枚一律
    /// 不动 —— 与上面那枚 `pick` 同一个理由：没选中就按下去这件事在屏幕上必须有对面。
    property int palPick: -1

    /// 选中哪一枚**方案**（0 起）。`-1` 时「应用方案 / 删方案」两枚不动：应用一枚没画出来的方案，
    /// 等于让页上多说一句档里没做的事。
    property int palScheme: -1

    /// 两枚选中各自有没有对面。绑值不绑长度：`palPick` 是本文件声明的属性，notify 由 Qt 生成。
    readonly property bool hasPalPick: win.palPick >= 0 && win.palPick < win.palBand.length
    readonly property bool hasPalScheme: win.palScheme >= 0 && win.palScheme < win.palSchemes.length

    /// 计划够不够跑那几条腿。绑**值**不绑长度：`plan` 是本文件自己声明的属性，它的 notify 由
    /// Qt 生成（`HintWindow.qml` 那三封 `#316` / `#321` 的信教的形状）。
    readonly property bool armed: win.plan.length === 14

    /// §5.20.3 那一档够不够跑它那两条腿。与上面那一枚一样绑**值**，而段的数目是 3 不是 14：
    /// 两档共用这一扇窗，各看各的计划决定跑哪几条腿（`main.rs` 的 refusals 保证不会同时够长）。
    readonly property bool ioArmed: win.ioPlan.length === 3

    /// §5.7.22 那一档够不够跑它那六按加两只 IO 手。同一枚绑**值**的写法，而段的数目是 12：三档共用
    /// 这一扇窗，各看各的计划决定跑哪几条腿（`main.rs` 的 refusals 把它们钉成互斥，所以不需要
    /// 「先跑哪一条」那一层排序）。
    readonly property bool palArmed: win.palPlan.length === 12

    /// 这一页上此刻有没有可改的行。空页不是一扇「什么都没有」的窗：它是一枚点不开的入口。
    readonly property bool hasRows: win.rowsText.length > 0

    /// 「将被覆盖的项目」那一格此刻有没有东西可确认。没有列行就没有确认按钮可按 —— 与上面那一枚
    /// 同一个理由：§5.20.3 步骤 5 说的是「用户确认后才写入」，而确认的对象必须是画出来的那几行。
    readonly property bool hasIoRows: win.ioRows.length > 0

    /// 「会被换成的那一排」此刻在不在页上。没有那一列行就没有「确认应用这一份」可按 —— 与上面那一枚
    /// 同一理由，也只有一条腿的区别：那一格确认的是几枚**键**，这一格确认的是几枚**色**。
    readonly property bool hasPalIoRows: win.palIoRows.length > 0

    // §5.19.1 的那套餐具：两枚完整切换的（窗体、卡片、正文、强调），深色浅色各一套。绑的是
    // `Session.theme` 那枚**值**，notify 一到就重新求值 —— 与 `HintWindow.qml` 同一族（靠绑定，
    // 不靠 `main.qml` 里那几条造窗腿的次序，因为这扇窗是 `createObject` 造出来的）。
    readonly property bool dark: win.session.theme === "dark"
    readonly property color page_fill: dark ? "#12151B" : "#F2F4F7"
    readonly property color card_fill: dark ? "#1B1F27" : "#FFFFFF"
    readonly property color word_fill: dark ? "#F2F4F7" : "#12151B"
    readonly property color edge_fill: dark ? "#39424F" : "#B4BEC9"
    readonly property color pick_fill: dark ? "#2C3D57" : "#DCE7F5"
    // §5.19.2 主题色：绑 `Session.accent` 那一枚**值**（P149），而那一格里躺的已经是
    // `settings::accent_face` 按**此刻生效的那套餐具**夹过对比度的那一枚 —— 页上不再自己挑色，
    // 也不再写第二份「不够 3:1 就往哪个方向走」（#281）。`=== ""` 那一臂只在启动那第一帧、
    // `refreshAppearance()` 还没落地时成立，兜的是画师原本写死的两枚字面。
    readonly property color accent_fill: win.session.accent !== ""
        ? win.session.accent
        : (dark ? "#4C8DF6" : "#2563B8")

    // §5.19.4 界面字体与字号。P149 起这一页是那两枚键**唯一**的读者，而「唯一读者」在四扇画着
    // 字的窗上恰恰是缺陷的另一面：另外三扇把 `font.pixelSize` 写死，改档不动那些像素。倍数、
    // 空族兜底、取整地板自此搬到 `UiFont.qml` 那一枚共享对象 —— 全仓一份，本文件与那三扇共读
    // 同一份（#281）。页上写的 `uiFont.px(13)` 与 `uiFont.family` 就是它；`win.uiPx` /
    // `uiFont.family` 那些行自此不再存在，不是留着第二份拼法。
    UiFont {
        id: uiFont
        session: win.session
    }

    // 一扇正常的窗：有边框、进任务栏、接键盘。§5.20.1 说的是「设置页」，不是一个盖在桌面上的
    // 浮层，所以这里**不**写 `flags`：`Qt.Tool` / `FramelessWindowHint` 属于贴图与提示窗那一族。
    title: "falconshot-settings"
    width: 760
    // 520 → 700：这一扇窗现在下面多了一整节（两枚钮、那句总数、那一列行、两枚确认）。原来的
    // 300 行高随之收到 180 —— 这一节里没有任何一格是判据（量的都是 `visible`、类数、行数），
    // 尺寸改动的对面是「页上的东西在不在屏幕上」这一件 P142 也欠着的事。
    // 700 → 760：P160 在那一排标签与那一列行之间插进了「首次使用引导」那一块（一句中文折成
    // 三四行 + 一枚「知道了」）。这一枚改动**不是**判据的对面：窗高没有进任何一格读数，它是
    // 「那句写得长的话别被挤掉」这一件事的代价，写在这里是为了让下一轮读得出为什么是 760。
    // 760 → 1040：P183 在页脚又加了一整节「调色板管理」（那一排色、四枚手上的钮、那一列方案、
    // 存名字的框，合计约 280 px）。与上面两枚改动同一个口径：**窗高没有进任何一格读数**，它是
    // 「页上画着的东西得在屏幕上」这一件事的代价，写在这里是为了让下一轮读得出为什么是 1040。
    // 1040 → 1300：本轮（§5.21.3）那一块**展开**时约 260 px（诊断那几行、填字的那一格、那枚
    // 勾选框、预览那一段、两枚钮）。它默认收着，只多那一枚钮的 30 px，可是页上没有任何一处
    // 会滚动 —— 按一次第 1 步就把下面的导出/导入两枚钮顶到窗外，等于让用户为了反馈把配置弄丢。
    // 与上面三枚改动同一个口径：窗高不进任何一格读数。
    height: 1300
    color: win.page_fill

    /// 一行一串地切开。**空串切出来是 `[""]`，长度 1** —— 这一枚函数存在的理由就是那个 1：
    /// 一页空行在窗上会长成一格写着空串的行，而「这一类有没有行」正是 §5.20.1 那一页要判的事。
    function splitLine(s) {
        const text = String(s)
        return text === "" ? [] : text.split("\u{1d}")
    }

    /// 计划那一串的分法，和页/类那两串**不一样**，所以这里有两枚函数而不是一枚：Rust 那边
    /// `settingsCategories()` / `settingsPage(i)` 交的是 `RECORD`（`\u{1d}`）连接的行，而
    /// `settingsGaugePlan()` 交的是 `|` 连接的十四段（`settings_view::arm` 拼的就是这一串，
    /// `[settings-window arm]` 那一行印的也是它）。gen A 与紧跟着的探针（`p142_probe_debug_A.log`）
    /// 量到的是把计划喂给了行那把刀：`born=["1|1"]` —— 一段、窗可见，于是 `armed` 不成立，量具的腿
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
    ///
    /// 手型与行文本在**同一枚函数里**一起取：两串各自过一次桥就会有一个瞬间是「行是新的一串、
    /// 手型是旧的一串」，那一瞬间里第 i 行的开关会安到邻居那一行上。长度对不上是坏，所以这里
    /// 一次取两样而不是两处各取一次。
    ///
    /// 引导那一句也在这一枚里取：翻页与「按了知道了之后的重新问」走的是同一条路，而那一句话
    /// 与那几行属于同一页 —— 分两处取就会有一瞬间是「行是这一页的、引导是上一页的」。
    function refreshPage() {
        win.rowsText = win.pageText(win.which)
        win.rowsKind = win.splitLine(win.session.settingsRowKinds(win.which))
        win.guideText = String(win.session.settingsGuide(win.which))
        win.ackWordText = String(win.session.settingsGuideAckWord())
        win.updateWords = win.splitLine(win.session.updateWords(win.which))
        // §5.21.3 那一块的每一枚字也在这一枚里取：它与那几行同属一页（与上面那两枚同一个理由）。
        win.feedbackWords = win.splitLine(win.session.feedbackWords(win.which))
        // 换页就把这一块收回去：第 1 步那一次点击属于**这一页**，留在窗手里的那几行诊断与那一段
        // 预览因此不该跨页活着 —— 与下面那枚 `updateDownloadUrl` 同一个口径。清掉不判任何事。
        win.feedbackOpen = false
        win.feedbackDiag = ""
        win.feedbackText = ""
        // 那一枚「把当前前台程序加进来」的字也在这一枚里取：它与那几行同属一页，分两处取就会有一
        // 瞬间是「行是这一页的、钮是上一页的」。
        win.exclusionWord = String(win.session.settingsExclusionWord(win.which))
        // §8.4 那一列也在这一枚里取：它与那几行同属一页，分两处取就会有一瞬间是「行是这一页的、
        // 那一列是上一页的」。本页没有那一列时桥交回空串，splitLine 因此给一枚空数组。
        win.conflictLines = win.splitLine(win.session.settingsConflicts(win.which))
        // 换页就换掉上一次那一问的结果：入口那枚钮的存在必须跟着**这一页**的那一次点击走，
        // 不能是别处问出来的旧地址。清空在这里不判任何事 —— 判「有没有地址」的是桥那一次交回。
        win.updateDownloadUrl = ""
    }

    /// 「知道了」那枚钮：把这一页记成「引导递过了」，交回的话原样进状态行，窗不重写它。
    /// 这一页压根没有引导时一次桥也不过 —— 按一枚画不出来的钮，等于让页上多出一句没人问过的话。
    function ackGuide() {
        if (win.guideText === "")
            return
        win.statusLine = String(win.session.settingsGuideAck(win.which))
        win.guideText = ""
    }

    /// 「检查更新」那枚钮（§5.21.1 第 2 步那一次**真的有人按了**的点击）。这里只**起**这一趟：
    /// 取回那份清单自 P184 起真的走网络（最坏是 resolve + connect + receive 三段超时），而按下去的
    /// 那一手在 GUI 线程上 —— 在它上面等结果，僵住的是桌上**每一扇**窗。桥交回的是「这一趟在跑」
    /// 那一句（地址那一支还带 §6.3 要的那句网络请求提示），原样进状态行，窗不重写它。
    ///
    /// 上一次那枚入口地址在这里作废：入口那枚钮的存在必须跟着**这一次**问走（与 `refreshPage`
    /// 里那一清同一口径）。判决落地不由这一枚函数等，由下面那条 `Connections` 送进来。
    function askUpdate() {
        win.updateDownloadUrl = ""
        win.statusLine = String(win.session.updateAsk())
    }

    /// 落地那一条线：桥在工作线程上取回清单、把结果投回 GUI 线程写进 `update_line`，那一串是
    /// `那句判决 + RECORD + 那枚入口地址` —— 与页上 `splitLine` 早就在用的那把刀同一个格式。
    /// 段数不够就是桥没交回两样，两格都不写值；这里不第二次问档，也不在本文件拼第三段。
    function landUpdate(line) {
        const parts = win.splitLine(line)
        if (parts.length < 2)
            return
        win.statusLine = String(parts[0])
        win.updateDownloadUrl = String(parts[1])
    }

    /// 那枚入口按下去（第 4 步「下载入口」自此不只在句子里）：递回的是**窗手里那一句**，桥开的
    /// 也就是同一句 —— 不再第二次取清单，于是「页上写着的那一趟」与「按下去开的那一句」不会是
    /// 两趟的读数。地址那一格是空串时一次桥也不过：按一枚画不出来的钮，等于让页上多出一句没人
    /// 问过的话（与上面 `ackGuide` 同一形状）。
    function openDownload() {
        if (win.updateDownloadUrl === "")
            return
        win.statusLine = String(win.session.updateOpen(win.updateDownloadUrl))
    }

    /// 第 `i` 枚字，桥没递够那一列时答空串。页上不自己补一枚字，也不数「第几枚该是哪一步」——
    /// 那一列的次序与枚数只有一位主人（`feedback_window::ON_PAGE`，门 ② 那一格钉的就是它）。
    function feedbackWord(i) {
        return i < win.feedbackWords.length ? String(win.feedbackWords[i]) : ""
    }

    /// §5.21.3 第 1 步那一次点击：让第 2 步真的发生一次。桥交回的是**已脱敏的那几行**，原样进
    /// 那一格，窗不改写它；这一块自此展开，第 3 步那两样才看得见。按下去一次对外动作也没有 ——
    /// 递东西的是下面那两手。
    function feedbackAsk() {
        win.feedbackDiag = String(win.session.feedbackAsk())
        win.feedbackOpen = true
        win.feedbackRefresh()
    }

    /// 第 3 步那两样（用户填的那句话 + 那枚勾选框）改了，就重新问一次「此刻按下去会递出去什么」。
    ///
    /// 这里**不**写 `statusLine`：这一条绑定每改一个字跑一次，把整段包刷进状态行会把屏幕上那一行
    /// 变成打字机的回声（桥上那一手也不记账 —— 记账的那一枚是 `feedbackSend`）。
    function feedbackRefresh() {
        win.feedbackText = String(win.session.feedbackPreview(feedDesc.text, feedbackLogs.checked))
    }

    /// 第 4 步：递出去。窗不判那一格空不空、渠道配没配 —— 那两句话各有其主（`feedback_window::send`
    /// 与它下面的 `channel_leg`），桥交回哪一句屏幕上就是哪一句。
    function feedbackSend() {
        win.statusLine = String(win.session.feedbackSend(feedDesc.text, feedbackLogs.checked))
    }

    /// 收起这一块：桥交回那句「什么都没递」，窗不重写它，也不留下任何一段看着像递出去过的字。
    function feedbackClose() {
        win.statusLine = String(win.session.feedbackClose())
        win.feedbackOpen = false
        win.feedbackDiag = ""
        win.feedbackText = ""
    }

    /// 这一行要的是开关吗。`rowsKind` 比 `rowsText` 短时（桥那侧交回空串的那一趟）答 `false` ——
    /// 宁可少画一枚开关，也不把开关安到没有那一行的位置去。
    function isSwitch(i) {
        return i >= 0 && i < win.rowsKind.length && String(win.rowsKind[i]) !== "field"
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

    /// §5.20.2「设置项变更后即时生效」在这一页上的那一只手：写完一枚 `[appearance]` 键之后
    /// 重新问一次桥，把主题那一枚词与那四枚外观值取回界面上。
    ///
    /// 只叫 `refreshTheme()` 一句：那四格**骑在同一手里**（`session.rs::refresh_theme` 的尾巴
    /// 就是 `refresh_appearance`），而「先 theme 后那四格」的次序是那一手在 Rust 里担保的 ——
    /// `accent` 吃的底就是刚写进 `Session.theme` 的那一枚词（`settings::accent_face`），两句摊在
    /// 页上就有被倒过来写的一日。这里不判任何事 —— 判据全在桥那三枚读者里，页上只是递话（#281）。
    ///
    /// 这四枚键没有一枚在 `RESTART_KEYS` 里（core 的名单），所以这一只手就是它们「即时」的全部
    /// 对面；在名单里的那几枚（`general.language` 等）页上带着`（需重启）`那几个字，这一只手
    /// 对它们什么都不做。
    function syncAppearance() {
        win.session.refreshTheme()
    }

    /// 保存：把输入框那串交过桥（闸门、落盘、`Written::text` 都在 Rust 那侧），再把这一页
    /// 重新取一次 —— 页上那一格跟着值走，这件事不靠任何一处 `property alias` 的运气。
    function saveDraft() {
        if (win.pick < 0) {
            win.statusLine = "没有选中任何一行"
            return
        }
        win.statusLine = String(win.session.settingsSet(win.pickKey, field.text))
        if (String(win.pickKey).indexOf("appearance.") === 0)
            win.syncAppearance()
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

    /// 按下第 `i` 行那一枚开关：桥那侧翻一头，走的仍是 `write` 那一条完整的路（语法闸、原子
    /// 落盘），页上印的是那句 `Written::text` 原文，然后重新取一次这一页 —— 行文本与手型一起
    /// 跟着值走。
    ///
    /// 这一枚与 `saveDraft` 是**两种手**，不是一条路的两个别名：它不需要先选中一行，也不碰输入框
    /// 里那串草稿（`refreshPage` 只改 `rowsText` / `rowsKind`）。§5.20.2 那一页上 bool 那几枚键要
    /// 的形状就是这个，而不是「把 true 打在文本框里再按保存」。
    function toggleRow(i) {
        const k = Number(i)
        if (k < 0 || k >= win.rowsText.length)
            return
        win.statusLine = String(win.session.settingsToggle(win.which, k))
        win.refreshPage()
    }

    /// §5.16.2 规则第二句那一枚「把当前前台程序加进来」：一次桥，页上印的是那句 `Written::text`
    /// 原文 —— 添成了哪一枚、还是为什么一枚也没添，都是桥说的话，这里不替它圆场。
    ///
    /// 与 `toggleRow` 同一种手：不选中任何一行、不碰输入框里那串草稿，按下去就是一次完整的写
    /// （过闸与落盘都在 Rust 那一头）。`refreshPage()` 那一趟把 `hotkey.exclusions` 那一行的新值
    /// 取回来，于是「页上那一行」跟着「档里那一串规则」走，而不是跟着这次点击的说法走。
    ///
    /// 桥那侧那一手自己会读**此刻**在前台的是谁；窗既不知道、也不该知道那是谁 —— 在这里问一次
    /// 「谁是前台」再递过去，就是给同一件系统事实第二个主人，而钮按下去到那一刻之间换了的窗归谁。
    function addForeground() {
        if (win.exclusionWord === "")
            return
        win.statusLine = String(win.session.addForegroundExclusion())
        win.refreshPage()
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
        // 那份档里可能有 `[appearance]` 那几格：导入是一次**成批**的写，页上没有「这一枚键是不是
        // 外观」这一问可问 ⇒ 无条件重新取一次那四枚值。§5.20.2 的即时生效不该在导入这一支上掉。
        win.syncAppearance()
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

    // ── §5.7.22 步骤 1/2/3 那一格背后的手 ──────────────────────────────────────
    //
    // 每一只手都是同样三步：问过桥、把桥那句话原样画进状态行、再重新读一次页上那一排。本文件
    // 一处也不自己改 `palBand` / `palSchemes` 这两个数组 —— 页上画的、内存里的、档里落的因此
    // 只能是一排色（#281）。量具那六按调的就是下面这几枚，与真人按的是同一条路。

    /// 重新问一次桥：那一排色与那一列方案**此刻**长什么样。两串一起取，不是两处各取一次：分开取
    /// 就会有一瞬间是「排是新的、方案是旧的」，而「保存为方案」那一腿问的正是这两枚对不对得上。
    function refreshPal() {
        win.palBand = win.splitLine(win.session.settingsPaletteBand())
        win.palSchemes = win.splitLine(win.session.settingsPaletteSchemes())
    }

    /// 页上第 `at` 行**此刻写的那枚色**（`序号|色` 从第一个 `|` 切开后的尾巴）。读不出交回空串：
    /// 那一格在页上就是「这一行没有色可画」，而不是窗替它编一枚。
    function palHex(at) {
        const parts = String(win.palBand[at] || "").split("|")
        return parts.length === 2 ? String(parts[1]) : ""
    }

    /// 页上第 `at` 枚方案**此刻写的那三个字段**里的名字。同一枚道理：切不开就是没有。
    function palSchemeName(at) {
        const parts = String(win.palSchemes[at] || "").split("|")
        return parts.length === 3 ? String(parts[0]) : ""
    }

    function doPalAdd(hex) {
        const line = String(win.session.settingsPaletteAdd(hex))
        win.palStatus = line
        win.refreshPal()
        win.refreshPage()
        return line
    }

    function doPalDel(at) {
        const line = String(win.session.settingsPaletteDel(at))
        win.palStatus = line
        win.refreshPal()
        win.refreshPage()
        return line
    }

    function doPalMove(at, delta) {
        const line = String(win.session.settingsPaletteMove(at, delta))
        win.palStatus = line
        win.refreshPal()
        win.refreshPage()
        return line
    }

    function doPalEdit(at, hex) {
        const line = String(win.session.settingsPaletteEdit(at, hex))
        win.palStatus = line
        win.refreshPal()
        win.refreshPage()
        return line
    }

    function doPalSaveScheme(name) {
        const line = String(win.session.settingsPaletteSaveScheme(name))
        win.palStatus = line
        win.refreshPal()
        win.refreshPage()
        return line
    }

    function doPalApplyScheme(name) {
        const line = String(win.session.settingsPaletteApplyScheme(name))
        win.palStatus = line
        win.refreshPal()
        win.refreshPage()
        return line
    }

    function doPalDropScheme(name) {
        const line = String(win.session.settingsPaletteDropScheme(name))
        win.palStatus = line
        win.refreshPal()
        win.refreshPage()
        return line
    }

    // ── §5.7.22 步骤 4 与 5 在页上的那三只手 ────────────────────────────────────
    //
    // 步骤 4（导出成一份谁都能拿记事本改的档）与步骤 5（从那份档读回来、确认后才换掉这一排）此前
    // 只有命令行那一副（`--export-palette` / `--import-palette --confirm`）—— 页上没有钮，普通用户
    // 走不到。这三枚函数与那两条旗标背后是**同一批手**（`settingsPaletteExport` 那一枚 invokable
    // 后面就是 `settings::palette_export`，确认那一枚后面就是 `settings::palette_import_page(.., true)`），
    // 所以这一页不存在第二条落盘的路，也不存在第二份「未应用」的说法。措辞同样一处不写：画的是桥
    // 交回的那一整句（#281）。

    /// 步骤 4：把此刻这一排写出去。交回的那一句带着「本机几枚 → 写出几枚到哪份档」，页上只是画它；
    /// 量具那一趟另把它报给门，因为门判的是**盘上那份文件**读回来是不是同一排，而不是这句话像不像。
    function doPalIoExport(target) {
        const line = String(win.session.settingsPaletteExport(target))
        win.palStatus = line
        return line
    }

    /// 步骤 5 的第一步：那份档会把这一排换成哪几枚。**一个字节也不写** —— 写要下面那枚「确认应用
    /// 这一份」。切开用的是 `settingsPage` 同一把刀，第 0 枚是那份档自己说的抬头、其余才是色。
    /// 读不出那份档（含「一枚颜色也没读出来」那一种）时桥交回**一句**而没有行：那一格在页上是
    /// 「一行也没画出来 + 那句话」，而不是「画了一张空表」—— 「没得可换」与「那份档读不出」是两种坏。
    function doPalIoPreview(target) {
        win.pendingPalIoImport = target
        const parts = win.splitLine(win.session.settingsPaletteImportPreview(target))
        win.palIoNote = parts.length > 0 ? String(parts[0]) : ""
        win.palIoRows = parts.slice(1)
        return parts
    }

    /// 步骤 5 的第二步：确认之后才真写。写完收起那一列行、重新读一次页上那一排 —— 那一列行说的是
    /// 刚才那一份档，而此刻档里躺着的已经是它了。
    function doPalIoApply() {
        const line = String(win.session.settingsPaletteImportApply(win.pendingPalIoImport))
        win.palStatus = line
        win.pendingPalIoImport = ""
        win.palIoRows = []
        win.palIoNote = ""
        win.refreshPal()
        win.refreshPage()
        return line
    }

    /// 不应用：那一列行收起来，一个字节也不递过桥。这一枚与 `doPalIoApply` 的分别是「调不调
    /// `settingsPaletteImportApply`」—— 与上面 `cancelImport` 同一支（明确的保存与取消）。
    function cancelPalIo() {
        win.pendingPalIoImport = ""
        win.palIoRows = []
        win.palIoNote = ""
        win.palStatus = "不应用：那份档一枚色也没换"
    }

    /// 选中那一枚方案：它的行号。桥那侧的「应用 / 删」收的都是**名字**，所以这里递的是页上那一行
    /// 此刻写的名字，不是窗自己数的第几枚。
    function pickPalScheme(at) {
        win.palScheme = at
    }

    function applyPickedScheme() {
        if (!win.hasPalScheme)
            return ""
        return win.doPalApplyScheme(win.palSchemeName(win.palScheme))
    }

    function dropPickedScheme() {
        if (!win.hasPalScheme)
            return ""
        return win.doPalDropScheme(win.palSchemeName(win.palScheme))
    }

    /// 真人从「保存为方案」那枚框里打的名字：空串一次桥也不过 —— 按一枚画不出名字的钮，等于让
    /// 档里多一行没有名字的方案，而 core 那道闸会把它丢掉：页上说的与档里存的就分家了。
    function saveSchemeNamed() {
        const name = String(palNameField.text)
        if (name === "")
            return ""
        return win.doPalSaveScheme(name)
    }

    /// 下面那五枚钮各递的是**选中那一枚的行号**与输入框里那一串字：窗不数「第几枚该删」，也不
    /// 猜「这枚钮按下去该写什么色」。没有选中就没有钮（`hasPalPick`），一次桥也不过不成。
    function addTypedHex() {
        return win.doPalAdd(String(palHexField.text))
    }

    function deletePickedSwatch() {
        if (!win.hasPalPick)
            return ""
        return win.doPalDel(win.palPick)
    }

    function movePickedUp() {
        if (!win.hasPalPick)
            return ""
        return win.doPalMove(win.palPick, -1)
    }

    function movePickedDown() {
        if (!win.hasPalPick)
            return ""
        return win.doPalMove(win.palPick, 1)
    }

    function editPickedSwatch() {
        if (!win.hasPalPick)
            return ""
        return win.doPalEdit(win.palPick, String(palHexField.text))
    }

    /// `--settings-palette` 那六按加两枚 IO 手（⑦ 导出、⑧ 预览、⑨ 确认应用，共九次真按）。报回的每
    /// 一行前缀是那格的名字，其余原样存进 Rust 那一侧的账。
    ///
    /// 报的是**窗那一侧的读数**：`pal-open` 里那枚 visible 是窗自己问 Qt 的，`add` / `move` / `del`
    /// 里那两个数是页上真的画了几行（`palBand.length`，不是桥交回了几枚），`edit` 里那两枚色是页上
    /// 那一行按之前与按之后**写的字**。段与段之间只有一个 `|`，所以这里一处也不报整行文本 ——
    /// `序号|色` 那一串撑进报回的行会把段数撑爆，那一格当场判红而不是判过（`pal_band_flat` 交回的
    /// 是空格连着的，正是为了这一句）。
    function runPalGauge() {
        // 这一档开在「标注」那一页（计划第 1 段），而窗出生时停在第 0 类：先换页再读数，
        // 否则 `pal-open` 报的是「窗在这一类上」——那一格的对面是门点名的那一类。
        win.which = Number(win.palPlan[1])
        win.refreshPage()
        win.refreshPal()
        win.session.settingsGaugeReport(
            "pal-open|" + win.which + "|" + (win.visible ? 1 : 0) + "|" + win.cats.length + "|"
            + win.palBand.length + "|" + win.palSchemes.length)
        // ① 新增一枚（步骤 2 的第一只手）：页上那一排多一行。
        const addWas = win.palBand.length
        win.doPalAdd(String(win.palPlan[2]))
        win.session.settingsGaugeReport("add|" + addWas + "|" + win.palBand.length)
        // ② 挪一枚：行数不许变。少了这一格，「挪」与「什么都没按」在屏幕上长得一样（§5.7.21 那
        //    一条说的是拖，本档按的是钮，走的是模型里同一枚 `move_to`）。
        const moveWas = win.palBand.length
        win.doPalMove(Number(win.palPlan[4]), Number(win.palPlan[5]))
        win.session.settingsGaugeReport("move|" + moveWas + "|" + win.palBand.length)
        // ③ 删一枚：少一行。按的是计划里那一枚行号，指错一枚去删是比不删更坏的那一种。
        const delWas = win.palBand.length
        win.doPalDel(Number(win.palPlan[3]))
        win.session.settingsGaugeReport("del|" + delWas + "|" + win.palBand.length)
        // ④ 改这一枚：报的是页上那一行按之前写的字与按之后写的字 —— 只有行数对上的「改」不算改。
        const editAt = Number(win.palPlan[6])
        const editWant = String(win.palPlan[7])
        const editWas = win.palHex(editAt)
        win.doPalEdit(editAt, editWant)
        win.session.settingsGaugeReport(
            "edit|" + editAt + "|" + editWant + "|" + editWas + "|" + win.palHex(editAt))
        // ⑤ 保存为方案（步骤 3）：那一列多一行，名字是计划里给的那一枚。
        const saveName = String(win.palPlan[8])
        const saveWas = win.palSchemes.length
        win.doPalSaveScheme(saveName)
        win.session.settingsGaugeReport("save|" + saveName + "|" + saveWas + "|" + win.palSchemes.length)
        // ⑦ 步骤 4 在页上的那一枚钮：把此刻这一排写进计划里那一份档。报的是**窗用的那枚路径**加桥
        //    交回的那一整句 —— 那一格判的是「按的是计划里那一份档」，而「导出真的把这一排带出去了」
        //    的对面是盘上那份文件自己的内容（Rust 那一头读它，不读这句话）。
        const exportAt = String(win.palPlan[10])
        win.session.settingsGaugeReport(
            "pal-export|" + exportAt + "|" + win.doPalIoExport(exportAt))
        // ⑧ 步骤 5 的第一枚钮：那份档「会换成哪几枚」在页上画成了几行。报的是**窗画**的那一列行数
        //    （`palIoRows.length`，不是桥交回了几枚 —— 抬头那句画在单独那一格里，不算进这一列）与
        //    抬头那一句。分母是开档那一刻按页上同一只手算出来的行数，所以「一行也没画」与「画了别的
        //    数」在这一格里都是红的；「那份档读不出」那一种坏交回**一句而没有行** ⇒ 画 0 行、对不上
        //    分母，正是这里要拦的那一个。
        const importAt = String(win.palPlan[11])
        const ioParts = win.doPalIoPreview(importAt)
        win.session.settingsGaugeReport(
            "pal-preview|" + win.palIoRows.length + "|"
            + String(ioParts.length > 0 ? ioParts[0] : ""))
        // ⑨ 步骤 5 的第二枚钮：确认之后那一排真的成了那份档里那几枚。两串都是**桥现读**的
        //    （`settingsPaletteBandFlat`），不是窗自己数的，所以「页上画了但没落盘」在这一格里过不去。
        //    这一腿排在 ⑥ 之前：那一格问的「按之前不是这一串」要的是门种的那三枚还在盘上，被这一腿
        //    换掉就没了对面 —— 它因此改问「按之前是导入留下的那一串」。
        const ioWas = String(win.session.settingsPaletteBandFlat())
        win.doPalIoApply()
        win.session.settingsGaugeReport(
            "pal-import|" + importAt + "|" + ioWas + "|" + String(win.session.settingsPaletteBandFlat()))
        // ⑥ 应用门种的那一枚方案：这一趟里唯一一处「按之前不是这一串、按之后是那一串」。报的是
        //    整排色的那一串空格连着的字，因为它才是「换掉的是哪几枚」这一问的对面。
        const applyName = String(win.palPlan[9])
        const bandWas = String(win.session.settingsPaletteBandFlat())
        win.doPalApplyScheme(applyName)
        win.session.settingsGaugeReport(
            "apply|" + applyName + "|" + bandWas + "|" + String(win.session.settingsPaletteBandFlat()))
        // 逐页那一格排在六按之后、关闭之前：它问的是「这一类此刻有几行」，而本档新写进
        // `[annotation]` 的那一枚键从第一按起就在页上了 —— 放在最前面读到的也是同一个数，
        // 放在最后则多答一句「改到这会儿页上还有几行」。
        win.session.settingsGaugeReport(
            "pal-page|" + win.which + "|" + win.rowsText.length)
        win.session.settings_open = 0
        win.session.settingsGaugeReport("pal-close|" + win.session.settings_open)
        win.hide()
    }

    // ── `--settings-window` 那四条腿 ──────────────────────────────────────────
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

    /// 这一页此刻画出了几枚开关。窗数的是它**手里那串** `rowsKind`，不是桥那一头的名单 ——
    /// 「那一行确实长成了一枚开关」这件事在屏幕这一侧只有这一个读数。
    function countSwitches() {
        let n = 0
        for (let i = 0; i < win.rowsKind.length; ++i) {
            if (win.isSwitch(i))
                n++
        }
        return n
    }

    /// 第四条腿：按的是页上那一枚**真的开关**，不是「选中一行、往文本框里打 true、按保存」。
    ///
    /// 这一枚与上面那三腿的分别是手，不是路径：`toggleRow` 调的 `settingsToggle` 在 Rust 那侧
    /// 走的仍是 `write` 那一条（闸门、落盘、`Written::text`）。报回的是五行 ——
    /// `toggle|键|按之前那一行|按之后那一行|这一页画出了几枚开关`：段里多出来的那一枚是数出来的
    /// 开关数，它才是这一腿独有的那件东西（前三腿没有一种坏能让「页上画错形状」露出来）。
    /// 行号与键名照样先核对，对不上整条腿不跑，只递一行 `abandon`。
    function legToggle(idx, key) {
        const i = Number(idx)
        win.refreshPage()
        const pair = win.splitLine(win.session.settingsRowAt(win.which, i))
        if (pair[0] !== key) {
            win.session.settingsGaugeReport(
                "toggle|abandon|row=" + i + "|key=" + pair[0] + "|want=" + key)
            return
        }
        const before = String(win.rowsText[i] || "")
        const drawn = win.countSwitches()
        win.toggleRow(i)
        const after = String(win.rowsText[i] || "")
        win.session.settingsGaugeReport("toggle|" + key + "|" + before + "|" + after + "|" + drawn)
    }

    /// 整份名单逐页过一次桥。这一条才是那份名单在**窗上**的读数：类数、每类几行，
    /// 数的是窗自己拿到的那些串，而不是 Rust 手里那份清单。
    ///
    /// 自 P160 起每一页多递一行 `guide|类号|字数`：同一趟里把「哪几页欠着一句引导」也过一遍桥。
    /// 递的是**字数**而不是那一串中文，理由是那一句话的原文在 Rust 手里、窗只是画它 —— 窗能答的
    /// 独有读数只有「这一页有没有拿到一句」。
    function censusPages() {
        for (let i = 0; i < win.cats.length; ++i) {
            win.session.settingsGaugeReport("page|" + i + "|" + win.pageText(i).length)
            win.session.settingsGaugeReport("guide|" + i + "|" + String(win.session.settingsGuide(i)).length)
        }
    }

    /// 第五条腿：按下那一枚**真的「知道了」**之前与之后，各读一次页上那一块。
    ///
    /// 报回九段：`guideack|类号|收之前手里那句的字数|收之前那块画没画|收之前块里那串字的字数|
    /// 收之后手里那句的字数|收之后那块画没画|钮上此刻画的字|此刻桥说该写的字`。桥那侧的
    /// `settings_view::reported()` 在第一枚 `|` 处切过只交回尾巴，所以判据读到的**类号是第 0 段、
    /// 一共八段** —— 这一腿头一版按九段写下标，整条腿因此一直判不过（V1.161 块 ④ 记着这一次）。
    /// 中间那一枚「块里
    /// 那串字的字数」是这一腿独有的一枚读数：它问的是**屏幕上那一个 Text 此刻写的**东西，而不是
    /// 窗手里那枚属性 —— 属性有而块里没写上去，是绑定断了的形状，只比属性读不出来。最后两枚是
    /// 同一个形状的另一种坏：那一句话里写的字与那一枚钮上写的字，在桥那侧是同一枚常量，页上抄
    /// 第二份就会分家，而分家只在屏幕上露出来。
    ///
    /// 这一腿与前面四条的区别在**它按的不是设置**：那四条改的是档里的一枚键，这一条改的是
    /// `guide_seen.txt` 里的一行，而窗递回的那一句「收下」的话，只有按过之后再去问一次桥才
    /// 读得出来它到底收没收到。
    function legGuide() {
        win.refreshPage()
        const before = String(win.guideText)
        const drawn = win.guideDrawn ? 1 : 0
        const painted = String(win.guidePainted).length
        const word = String(win.ackWordDrawn)
        win.ackGuide()
        win.session.settingsGaugeReport(
            "guideack|" + win.which + "|" + before.length + "|" + drawn + "|" + painted + "|"
            + String(win.guideText).length + "|" + (win.guideDrawn ? 1 : 0) + "|"
            + word + "|" + String(win.session.settingsGuideAckWord()))
    }

    /// §5.21.1 第 2 与第 4 步那两枚钮在**屏上**的读数，一次递两行
    /// `update|类号|那一块画没画|那枚入口画没画|第一枚钮写的字|第二枚钮写的字`：第一行读的是窗开工
    /// 时那一页（那一页压根没有这一块），第二行读的是计划第 13 段（0 起，共十四段）交回的那一
    /// 页 —— 页号从计划里抄，本文件一次不数「关于与更新是第几页」。
    ///
    /// 本腿不按那两枚钮、不写档、不联网：它换页、读数、把页换回去，所以「落盘次数」那一格照常只数
    /// 前面四腿。第二行里那枚入口该是**没画**的：地址只由「检查更新」那一次判决交回，而本腿一次也
    /// 没问过 —— 画出来才是要修的坏。
    function legUpdate() {
        const mine = win.which
        win.session.settingsGaugeReport(
            "update|" + mine + "|" + (win.updateBlockDrawn ? 1 : 0) + "|"
            + (win.updateEntryDrawn ? 1 : 0) + "|" + String(win.updateCheckPainted) + "|"
            + String(win.updateEntryPainted))
        const want = Number(win.plan[13])
        win.which = want
        win.refreshPage()
        win.session.settingsGaugeReport(
            "update|" + want + "|" + (win.updateBlockDrawn ? 1 : 0) + "|"
            + (win.updateEntryDrawn ? 1 : 0) + "|" + String(win.updateCheckPainted) + "|"
            + String(win.updateEntryPainted))
        win.which = mine
        win.refreshPage()
    }

    function runGauge() {
        win.session.settingsGaugeReport(
            "open|" + win.which + "|" + (win.visible ? 1 : 0) + "|" + win.cats.length + "|"
            + win.rowsText.length)
        win.censusPages()
        // 第五条腿排在逐页之后、那四条写值的腿之前：逐页那一条读的是「每一页欠不欠一句」，
        // 而这一腿会把**当前这一页**记成收过了 —— 先数完再收，否则逐页那一趟读到的是收过之后
        // 的状态，逐页那一格就永远答不出「复杂页有几页」。
        win.legGuide()
        // 顺序是定的：保存、还原前的值不归本文件管（Rust 读完回来自己改回去），取消与拒那两腿
        // 各点各的键，所以这几腿互不改对方的前置（开关那一腿按的是第四枚键，见 `legToggle`）。
        win.legRow("save", win.plan[2], win.plan[3], win.plan[4], true)
        win.legRow("cancel", win.plan[5], win.plan[6], win.plan[7], false)
        win.legRow("refuse", win.plan[8], win.plan[9], win.plan[10], true)
        // 第四条腿按开关。次序放在最后是有原因的：前三腿各点各的键，而这一腿读的 `countSwitches`
        // 数的是**整页**的手型，页上此刻有没有那一行、有几行，都归前面那几腿改过之后的状态。
        win.legToggle(win.plan[11], win.plan[12])
        // 第六腿排在四条写值的腿之后：那四腿读的是开工那一页的 `rowsText`，而本腿会把页换到
        // 「关于与更新」再换回来 —— 换页改的只是画哪一页，一次也不落盘，所以它既不动那四腿的
        // 前置，也不动「落盘次数」那一格。
        win.legUpdate()
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
            font.pixelSize: uiFont.px(18)
            font.family: uiFont.family
            font.bold: true
        }

        // 页顶那一排标签：一枚一类，枚数与次序全由桥那一份名单说了算，本文件一处不数。
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
                        font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
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
            font.pixelSize: uiFont.px(13)
            font.family: uiFont.family
            opacity: 0.75
        }

        // §5.21.2 第四枚那一块：复杂页第一次打开时递的那一句「首次使用引导」。
        //
        // `visible` 与 `height` 一起绑：位置器（`Column`）对看不见的子项是否让位，本仓库没有一份
        // 跑通过的读数可以引，所以这里不赌它 —— 不递那一页时这一块连高度都没有，页上那一列行
        // 因此长在它们一直长在的位置上。
        //
        // 窗这里一处也不判「这一页复杂吗、收过没」：`win.guideText` 是桥答的那一句原文，
        // 按「知道了」交回去的也是桥自己那句话（`guide::ack`）。
        Rectangle {
            id: guideBox

            visible: win.guideText !== ""
            height: visible ? guideWord.implicitHeight + guideBtn.height + 20 : 0
            width: parent.width
            radius: 6
            color: win.pick_fill
            border.color: win.accent_fill
            border.width: 1

            Text {
                id: guideWord

                anchors.top: parent.top
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.margins: 10
                text: win.guideText
                color: win.word_fill
                font.pixelSize: uiFont.px(13)
                font.family: uiFont.family
                wrapMode: Text.Wrap
                // 引导那句是**一整句**中文（一行动作说明的两倍长），中间省略会把「改了要重启」
                // 那半句切掉，而那一半正是这一句要递的东西。折行，不省略。
            }

            Rectangle {
                id: guideBtn

                anchors.right: parent.right
                anchors.rightMargin: 10
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 8
                width: ackWord.implicitWidth + 18
                height: 24
                radius: 5
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: ackWord

                    anchors.centerIn: parent
                    // 这两个字不是本页自己起的名字：`crate::guide::ACK_WORD` 从桥递过来，与引导句
                    // 里「按「知道了」之后…」那一段是同一枚常量（见 `ackWordText` 头顶那段）。
                    text: win.ackWordText
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.ackGuide()
                }
            }
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
                            // 那一行是开关时给开关让出它的位置：留出的是开关那 34 px 加两侧边距。
                            // 这一枚**不是**「有没有开关」的第二份判断 —— 它读的是 `isSwitch`，
                            // 而 `isSwitch` 读的是桥交回的那串手型。
                            anchors.rightMargin: win.isSwitch(index) ? 54 : 10
                            text: String(modelData)
                            color: win.word_fill
                            font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                            // 折行而不是中间省略。P158 之前这一行只有 `key=value`，中间省略省掉的是
                            // 值那一头（一长串保存路径），读的人还能把行认出来；自 P158 起动作行尾部
                            // 多了一句「 —— 名：说明」，同一把刀省的就是那句说明本身 —— 而 §5.21.2
                            // 第二枚要的正读得到它。`elide` 与 `wrapMode` 是互斥的两条路（Qt 里 elide
                            // 赢），所以这里换折行、把那一句留下；行高读的是 `implicitHeight`，两行的
                            // 那一行自己长高，不需要这里再数一行。
                            wrapMode: Text.Wrap
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: win.selectRow(index)
                        }

                        // 那一枚开关：本文件里唯一一处「按一下就改值」的形状。它自己不判事 ——
                        // 在哪一头读 `rowsKind[index]`（桥那侧的 `on`/`off`），按下去交
                        // `toggleRow`，落盘与闸门在 Rust 那侧。`Rectangle` 而不是 `Switch`：
                        // 本模块只用 QtQuick 自带的 `Rectangle`/`Text`/`MouseArea` 那一族餐具，
                        // 与上面「保存」「取消」那两枚同一形状（`Switch` 属于
                        // QtQuick.Controls，引进来就要一并引进它那一套主题色，而 §5.19.1 那套餐具
                        // 是本窗自己一套颜色说了算）。
                        Rectangle {
                            id: rowSwitch

                            visible: win.isSwitch(index)
                            width: 34
                            height: 18
                            radius: 9
                            anchors.verticalCenter: parent.verticalCenter
                            anchors.right: parent.right
                            anchors.rightMargin: 10
                            color: String(win.rowsKind[index]) === "on" ? win.accent_fill : win.edge_fill
                            border.color: win.edge_fill
                            border.width: 1

                            // 那一颗钮扣：在 `on` 那一头时停在右边。它是 `rowsKind` 的函数，
                            // 不是某一次点击的记忆 —— 于是「页上画的那一头」与「内存里的那一枚」
                            // 之间不存在两条路。
                            Rectangle {
                                width: 14
                                height: 14
                                radius: 7
                                anchors.verticalCenter: parent.verticalCenter
                                x: String(win.rowsKind[index]) === "on" ? parent.width - width - 2 : 2
                                Behavior on x { NumberAnimation { duration: 90 } }
                                color: "#FFFFFF"
                            }

                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: win.toggleRow(index)
                            }
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
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
        }

        Row {
            spacing: 8

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "新值"
                color: win.word_fill
                font.pixelSize: uiFont.px(13)
                font.family: uiFont.family
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
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
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
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.cancelDraft()
                }
            }
        }

        // ── §5.16.2 规则第二句那一枚「把当前前台程序加进来」，只在「快捷键」那一页 ──────────
        //
        // 页上 `hotkey.exclusions` 那一行仍然**写不动**（它是一串表，`Config::set_str` 到它那里就是
        // `Err`，core 那枚 `hotkey_exclusions_keep_their_shape` 钉的正是这一句），而这一枚钮是它
        // 唯一一副从产品里写得进的手：按下去问的是**此刻**在前台的那一个程序，添成一枚「这位跟前
        // 一枚全局快捷键也不响」的规则。
        //
        // 窗一处也不判事：画不画这一块问的是桥（`settingsExclusionWord()` 空串 = 这一页压根不递），
        // 钮上那几个字也是桥的原文；按下去说了什么，走的是 `Written::text` 那一整句 ——「添成了」
        // 「已经有一位管着了」「说不上来谁在跟前」三臂里说哪一臂，由 Rust 那一头定，不由这里猜。
        // `visible` 与 `height` 一起绑，与上面 `updateRow` 同一处理由：位置器对看不见的子项让不让位，
        // 本仓库没有一份跑通过的读数可以引。
        Row {
            id: exclusionRow

            spacing: 8
            visible: win.exclusionWord !== ""
            height: visible ? exclusionBtn.height : 0

            Rectangle {
                id: exclusionBtn

                width: exclusionLabel.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: exclusionLabel

                    anchors.centerIn: parent
                    text: win.exclusionWord
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.addForeground()
                }
            }
        }

        // ── §8.4 第一句「显示冲突的组合键和相关功能」在「快捷键」那一页上的那一列 ──────────
        //
        // 屏幕上那一格（浮窗 `Session.status`）读的是一句话，最多原样引三枚，其余的那几行来这里：
        // 页上这一列拿的是**全列**。整列的字都出自桥（`settingsConflicts()` 原文按 `RECORD` 切），
        // 窗一处也不拼一句、也不数「哪几行该看一眼」—— 那一判住在 Rust 的 `hotkey::RowKind` 里，
        // 写下那一句的当场就定了身份（#281、#453）。
        //
        // 空数组两种读法画法一样（这一页压根不是「快捷键」/ 这一趟一行也没什么要用户看），整块连
        // 高度一起没有；`visible` 与 `height` 一起绑，与上面 `exclusionRow`、`updateRow` 同一处理由。
        Column {
            id: conflictBox

            spacing: 4
            visible: win.conflictLines.length > 0
            height: visible ? implicitHeight : 0
            width: parent.width

            Repeater {
                id: conflictList

                model: win.conflictLines
                delegate: Text {
                    width: conflictBox.width
                    // 每一行都是 `Row::said` 那一整串，中间不许省略：被系统挡下的那一枚与共用
                    // 同一串键的那两枚动作，说得一半不剩就是没说（与上面 `guideWord` 同一理由）。
                    text: modelData
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                    wrapMode: Text.Wrap
                }
            }
        }

        // ── §5.21.1 第 2 步与第 4 步的那两枚钮，只在「关于与更新」那一页页脚 ──────────────
        //
        // 第一枚递的是那一次「点击」（第 2 步），第二枚是第 4 步那句「显示版本号、更新说明和
        // 下载入口」里那枚**可以点的**东西 —— 在 P179 之前那串地址只在句子里出现过，不在钮上。
        //
        // 窗一处也不判事：画不画这一块问的是桥（`updateWords()` 空串 = 这一页压根不递），两枚字
        // 也是桥的原文（`falcon_core::update` 那两枚常量，#281）；第二枚的 `visible` 绑的是
        // 「那一次判决有没有交回地址」，与句子里那句「清单没写入口」出自同一次问。
        //
        // `visible` 与 `height` 一起绑：位置器对看不见的子项是否让位，本仓库没有一份跑通过的读数
        // 可以引（与上面 `guideBox` 同一处理由），所以这里不赌它。
        Row {
            id: updateRow

            spacing: 8
            visible: win.updateWords.length >= 2
            height: visible ? checkBtn.height : 0

            Rectangle {
                id: checkBtn

                width: checkWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: checkWord

                    anchors.centerIn: parent
                    text: win.updateWords.length >= 1 ? String(win.updateWords[0]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    // 这一趟还在路上时不接第二下。桥那一臂（`start_update_check`）自己也拒 ——
                    // 那里拒的是「两趟各取各的清单、后落地的盖掉先落地的」这一种坏，而页与托盘
                    // 共用那一手；这里的 `enabled` 只是少让人按一次注定被拒的钮。
                    enabled: !win.session.update_busy
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.askUpdate()
                }
            }

            Rectangle {
                id: dlBtn

                visible: win.updateDownloadUrl !== ""
                width: dlWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.accent_fill

                Text {
                    id: dlWord

                    anchors.centerIn: parent
                    text: win.updateWords.length >= 2 ? String(win.updateWords[1]) : ""
                    color: "#FFFFFF"
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.openDownload()
                }
            }
        }

        // ── §5.21.3 那四步，只在「关于与更新」那一页页脚 ────────────────────────────────
        //
        // 第 1 步那一枚钮在桥递得出那一列字的时候就画着；第 2、3、4 步在它被按过之后才摊开 ——
        // 那四步是有先后的（先点、再生成、再填、再递），页把它们一次全摆出来就是替用户把第一步
        // 省掉了，而第 2 步的那几行还没生成，那一格只会是空的。
        //
        // 窗一处也不判事：这一块画不画、上面写哪几枚字，全来自桥那一份原文；按下去说了什么也全
        // 来自桥（空着那一格拒不拒、渠道配没配，页都不知道，也不该知道）。
        //
        // `>= 7` 是 `feedback_window::ON_PAGE` 的枚数：桥少递一枚就是名单与页分家，那一格宁可不
        // 画也不画半块。`visible` 与 `height` 一起绑，与上面 `updateRow`、`exclusionRow` 同一处理
        // 由（位置器对看不见的子项是否让位，本仓库没有一份跑通过的读数可以引）。
        Column {
            id: feedbackBox

            spacing: 8
            width: parent.width
            visible: win.feedbackWords.length >= 7
            height: visible ? implicitHeight : 0

            Rectangle {
                id: feedbackAskBtn

                width: feedbackAskWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: feedbackAskWord

                    anchors.centerIn: parent
                    text: win.feedbackWord(0)
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.feedbackAsk()
                }
            }

            Column {
                id: feedbackSteps

                spacing: 8
                width: parent.width
                visible: win.feedbackOpen
                height: visible ? implicitHeight : 0

                Text {
                    width: parent.width
                    text: win.feedbackWord(3)
                    color: win.word_fill
                    opacity: 0.7
                    font.pixelSize: uiFont.px(12)
                    font.family: uiFont.family
                }

                // 第 2 步的产物：桥现生成、现脱敏的那几行原文。窗不裁它 —— 用户要看清递的是
                // 什么才有第 4 步那一次点头可言；太长的话由上面那一格折行显示，最多八行。
                Text {
                    width: parent.width
                    text: win.feedbackDiag
                    color: win.word_fill
                    font.pixelSize: uiFont.px(12)
                    font.family: uiFont.family
                    wrapMode: Text.Wrap
                    maximumLineCount: 8
                    elide: Text.ElideRight
                }

                Row {
                    spacing: 8

                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: win.feedbackWord(1)
                        color: win.word_fill
                        font.pixelSize: uiFont.px(13)
                        font.family: uiFont.family
                    }

                    TextField {
                        id: feedDesc

                        width: 420
                        onTextChanged: win.feedbackRefresh()
                    }
                }

                // 第 3 步那枚「决定」：它改的是**出去的那一段字**，不是屏幕上的一句装饰 ——
                // 这一条由门 ⑤ 那一格判，页上这一枚只负责把它的样子递给桥。
                CheckBox {
                    id: feedbackLogs

                    text: win.feedbackWord(2)
                    onCheckedChanged: win.feedbackRefresh()
                }

                Text {
                    width: parent.width
                    text: win.feedbackWord(4)
                    color: win.word_fill
                    opacity: 0.7
                    font.pixelSize: uiFont.px(12)
                    font.family: uiFont.family
                }

                Text {
                    width: parent.width
                    text: win.feedbackText
                    color: win.word_fill
                    font.pixelSize: uiFont.px(12)
                    font.family: uiFont.family
                    wrapMode: Text.Wrap
                    maximumLineCount: 10
                    elide: Text.ElideRight
                }

                Row {
                    id: feedbackSendRow

                    spacing: 8

                    Rectangle {
                        id: feedbackSendBtn

                        width: feedbackSendWord.implicitWidth + 22
                        height: 30
                        radius: 6
                        color: win.accent_fill

                        Text {
                            id: feedbackSendWord

                            anchors.centerIn: parent
                            text: win.feedbackWord(5)
                            color: "#FFFFFF"
                            font.pixelSize: uiFont.px(13)
                            font.family: uiFont.family
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: win.feedbackSend()
                        }
                    }

                    Rectangle {
                        id: feedbackCloseBtn

                        width: feedbackCloseWord.implicitWidth + 22
                        height: 30
                        radius: 6
                        color: win.card_fill
                        border.color: win.edge_fill
                        border.width: 1

                        Text {
                            id: feedbackCloseWord

                            anchors.centerIn: parent
                            text: win.feedbackWord(6)
                            color: win.word_fill
                            font.pixelSize: uiFont.px(13)
                            font.family: uiFont.family
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: win.feedbackClose()
                        }
                    }
                }
            }
        }

        Text {
            width: parent.width
            text: win.statusLine
            color: win.word_fill
            opacity: 0.85
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
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
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
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
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
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
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
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
                            font.pixelSize: uiFont.px(12)
                            font.family: uiFont.family
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
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
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
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
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
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
            elide: Text.ElideRight
            maximumLineCount: 1
        }

        // ── §5.7.22 步骤 1/2/3：调色板管理那一格 ────────────────────────────────
        //
        // 与上面那一节同一枚道理：这一格在产品会话里**一直看得见** —— PRD 那一条说的是设置页上的
        // 一双手，不是量具专用的一层皮。跑不跑腿由最下面那枚 Timer 决定，看得见看不见由配置决定。
        // 抬头与那十二枚钮写的字全来自 `settingsPaletteWords()`，本文件一处不抄第二份（#281）。
        Text {
            text: win.palWords.length > 0 ? String(win.palWords[0]) : ""
            color: win.word_fill
            font.pixelSize: uiFont.px(14)
            font.family: uiFont.family
            font.bold: true
        }

        // 那一排色：一枚一行，行首一块**真色**。画的是 `settingsPaletteBand()` 现读的那份名单，
        // 所以「按了新增之后页上多一枚」这件事的对面是档里那一排，不是本页抄的一份数组。
        Flickable {
            width: parent.width
            height: 84
            contentHeight: palList.height
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: palList

                width: parent.width
                spacing: 3

                Repeater {
                    model: win.palBand

                    Rectangle {
                        id: palRow

                        // 这一行此刻写的那枚色。绑的是 `win.palBand`（`palHex` 里读的就是它），
                        // 所以桥那侧改一笔、页上这一块跟着换 —— 色块与那串字因此不可能分家。
                        readonly property string sw: win.palHex(index)

                        width: palList.width
                        height: 22
                        radius: 4
                        color: win.palPick === index ? win.pick_fill : win.card_fill
                        border.color: win.palPick === index ? win.accent_fill : win.edge_fill
                        border.width: 1

                        Row {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 6

                            Rectangle {
                                width: 14
                                height: 14
                                radius: 3
                                // 空串不是一枚颜色：那一格画成本页的底，而不是让 Qt 报一句
                                // `Invalid color definition` —— 「这一行没画出色」本来就该看得见。
                                color: palRow.sw === "" ? win.page_fill : palRow.sw
                                border.color: win.edge_fill
                                border.width: 1
                            }

                            Text {
                                text: String(modelData)
                                color: win.word_fill
                                font.pixelSize: uiFont.px(12)
                                font.family: uiFont.family
                                elide: Text.ElideRight
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: win.palPick = index
                        }
                    }
                }
            }
        }

        Row {
            spacing: 8

            TextField {
                id: palHexField

                width: 160
                placeholderText: "#RRGGBB"
            }

            Rectangle {
                width: palAddWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.accent_fill

                Text {
                    id: palAddWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 1 ? String(win.palWords[1]) : ""
                    color: "#FFFFFF"
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.addTypedHex()
                }
            }

            Rectangle {
                visible: win.hasPalPick
                width: palDelWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: palDelWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 2 ? String(win.palWords[2]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.deletePickedSwatch()
                }
            }

            Rectangle {
                visible: win.hasPalPick
                width: palUpWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: palUpWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 3 ? String(win.palWords[3]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.movePickedUp()
                }
            }

            Rectangle {
                visible: win.hasPalPick
                width: palDownWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: palDownWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 4 ? String(win.palWords[4]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.movePickedDown()
                }
            }

            Rectangle {
                visible: win.hasPalPick
                width: palEditWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: palEditWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 5 ? String(win.palWords[5]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.editPickedSwatch()
                }
            }
        }

        // 那一列方案：一枚一行 `名字|枚数|色 色 …`。画的是桥现读的那份名单，所以「存成一枚方案」
        // 之后页上多出的那一行，对面是档里那一枚键，不是本页数出来的枚数。
        Flickable {
            width: parent.width
            height: 62
            contentHeight: palSchemeList.height
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: palSchemeList

                width: parent.width
                spacing: 3

                Repeater {
                    model: win.palSchemes

                    Rectangle {
                        width: palSchemeList.width
                        height: 22
                        radius: 4
                        color: win.palScheme === index ? win.pick_fill : win.card_fill
                        border.color: win.palScheme === index ? win.accent_fill : win.edge_fill
                        border.width: 1

                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            anchors.left: parent.left
                            anchors.leftMargin: 8
                            anchors.right: parent.right
                            anchors.rightMargin: 8
                            text: String(modelData)
                            color: win.word_fill
                            font.pixelSize: uiFont.px(12)
                            font.family: uiFont.family
                            elide: Text.ElideMiddle
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: win.pickPalScheme(index)
                        }
                    }
                }
            }
        }

        Row {
            spacing: 8

            TextField {
                id: palNameField

                width: 160
                placeholderText: "方案名"
                onAccepted: win.saveSchemeNamed()
            }

            Rectangle {
                width: palSaveWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.accent_fill

                Text {
                    id: palSaveWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 6 ? String(win.palWords[6]) : ""
                    color: "#FFFFFF"
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.saveSchemeNamed()
                }
            }

            Rectangle {
                visible: win.hasPalScheme
                width: palApplyWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: palApplyWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 7 ? String(win.palWords[7]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.applyPickedScheme()
                }
            }

            Rectangle {
                visible: win.hasPalScheme
                width: palDropWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: palDropWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 8 ? String(win.palWords[8]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.dropPickedScheme()
                }
            }
        }

        // ── §5.7.22 步骤 4 与 5：那一排的导出与导入 ─────────────────────────────
        //
        // 与上面配置那一节同一枚道理：这一格在产品会话里**一直看得见** —— PRD 那两条说的是设置页上的
        // 两双手，不是量具专用的一层皮。跑不跑腿由最下面那枚 Timer 决定。四枚钮写的字全来自
        // `settingsPaletteWords()`（#281），按下去调的是上面那四枚函数，与量具同一条路。
        Row {
            spacing: 8

            Rectangle {
                width: palExportWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: palExportWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 9 ? String(win.palWords[9]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                // 钮自己只开一张对话框：落到哪份档、写出几枚，全在对话框与 Rust 那一头。
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: palExportDialog.open()
                }
            }

            Rectangle {
                width: palImportWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: palImportWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 10 ? String(win.palWords[10]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                // 选完档当场只做一件事：把「会换成哪一排」问出来画在下面。「导入即写」在这里是没有
                // 的 —— 换要下面那枚「确认应用这一份」按下去才发生（§5.7.22 步骤 5）。
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: palImportDialog.open()
                }
            }
        }

        Text {
            visible: win.palIoNote !== ""
            text: win.palIoNote
            color: win.word_fill
            opacity: 0.75
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
            elide: Text.ElideRight
            maximumLineCount: 1
        }

        // 那份档「会换成的那一排」：一枚一行。抬头那句与这些行出自同一次 `settingsPaletteImportPreview`，
        // 所以「画了几枚」这件事的对面是那份档自己说的话，不是本页写的一份名单（#281）。
        Flickable {
            visible: win.hasPalIoRows
            width: parent.width
            height: 84
            contentHeight: palIoList.height
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: palIoList

                width: parent.width
                spacing: 3

                Repeater {
                    model: win.palIoRows

                    Rectangle {
                        width: palIoList.width
                        height: palIoRowWord.implicitHeight + 8
                        radius: 4
                        color: win.card_fill
                        border.color: win.edge_fill
                        border.width: 1

                        Text {
                            id: palIoRowWord

                            anchors.verticalCenter: parent.verticalCenter
                            anchors.left: parent.left
                            anchors.leftMargin: 8
                            anchors.right: parent.right
                            anchors.rightMargin: 8
                            text: String(modelData)
                            color: win.word_fill
                            font.pixelSize: uiFont.px(12)
                            font.family: uiFont.family
                            elide: Text.ElideMiddle
                        }
                    }
                }
            }
        }

        Row {
            visible: win.hasPalIoRows
            spacing: 8

            Rectangle {
                width: palIoApplyWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.accent_fill

                Text {
                    id: palIoApplyWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 11 ? String(win.palWords[11]) : ""
                    color: "#FFFFFF"
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.doPalIoApply()
                }
            }

            Rectangle {
                width: palIoCancelWord.implicitWidth + 22
                height: 30
                radius: 6
                color: win.card_fill
                border.color: win.edge_fill
                border.width: 1

                Text {
                    id: palIoCancelWord

                    anchors.centerIn: parent
                    text: win.palWords.length > 12 ? String(win.palWords[12]) : ""
                    color: win.word_fill
                    font.pixelSize: uiFont.px(13)
                    font.family: uiFont.family
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: win.cancelPalIo()
                }
            }
        }

        Text {
            visible: win.palStatus !== ""
            width: parent.width
            text: win.palStatus
            color: win.word_fill
            opacity: 0.85
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
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
                font.pixelSize: uiFont.px(13)
                font.family: uiFont.family
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

    // 检查更新**落地**那一条腿。桥在工作线程上取那份清单，把结果投回 GUI 线程写进 `update_line`
    // （那句判决 + 那枚地址），这一条只负责把落地那一句摆进状态行、把地址交给入口那枚钮。
    //
    // 为什么窗不直接绑 `status`：`status` 是全场共用的那一行，判决落地之前用户可能又按了别的键，
    // 那一格已经是别的话，而入口那枚钮的可点性必须跟着**这一次判决**走（桥头顶记的是同一句）。
    Connections {
        target: win.session

        function onUpdate_lineChanged() {
            win.landUpdate(win.session.update_line)
        }
    }

    Component.onCompleted: {
        win.cats = win.splitLine(win.session.settingsCategories())
        win.plan = win.splitPlan(win.session.settingsGaugePlan())
        win.ioPlan = win.splitPlan(win.session.settingsIoPlan())
        // 第三副计划，与上面两副同一枚口径：只在出生时读一次，空串就是这一趟没有门要跑。
        // 那十二枚钮写的字**每一趟都读**：这一格在产品会话里也画，而页上不抄第二份措辞。
        win.palPlan = win.splitPlan(win.session.settingsPalettePlan())
        win.palWords = win.splitLine(win.session.settingsPaletteWords())
        win.refreshPal()
        if (win.armed)
            win.which = Number(win.plan[1])
        win.refreshPage()
        // 场上与场下只有 `show()` 与 `hide()` 两个入口，`visible` 一处也不写 —— 与
        // `HintWindow.qml` 同一封报错信（`p54_hint_d.log`：赋值之后 visible 仍是 false）。
        win.show()
        // 这一行不是判据，是**到此一处的记号**：`--settings-window` 那一档在窗没递回任何一行时，
        // 得能分清「窗压根没被造出来」与「窗造出来了、那四条腿没跑」。前两趟只能印前一句
        // （`p142_alone_A.log`），而这两种坏要修的是不同的码。`born` 不进任何一格的判定。
        //
        // 第三段是 P143 加的：这一扇窗现在读两副计划，`born|14|0|1` 与 `born|0|3|1` 说的就是两种坏
        // （窗读到了窗的那份计划、和窗压根没读到 io 那份计划）。第四段是 P183 加的同一件事：三副
        // 计划并排印，`born|0|0|1|0` 那种样子才分得出「门点了名、窗一次也没读到」，而 `born|0|0|1|10`
        // 说的是三副计划里只有窗自己那两副没到、第三副读到了。
        win.session.settingsGaugeReport(
            "born|" + win.plan.length + "|" + win.ioPlan.length + "|" + (win.visible ? 1 : 0)
            + "|" + win.palPlan.length)
    }

    // 量具那四条腿：等窗真的在桌面上（`LEG_MS` 由计划给），然后一次跑完。
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

    // §5.7.22 那六条腿：与上面两枚**同一形状、第三副计划**。三档共用这一扇窗，各自的计划够长才跑
    // 各自的腿；同一趟里不会有两副都够长（`main.rs` 的 refusals 把它们钉成互斥），所以这里不需要
    // 「先跑哪一条」那一层排序 —— 那一层一旦写出来就是第二个权威（#281）。
    Timer {
        id: palLeg

        interval: Number(win.palPlan[0]) || 400
        repeat: false
        running: win.palArmed
        // 与 `tick` / `io-tick` 同一个理由：把「这一枚 Timer 没触发」与「`runPalGauge` 半路抛」
        // 分开印。它不进任何一格的判定。
        onTriggered: {
            win.session.settingsGaugeReport("pal-tick|" + win.palPlan.length)
            win.runPalGauge()
        }
    }

    // 「导出配置」那张对话框：落到哪份档由对话框决定，本页只把那串 URL 原样递过桥 —— 与
    // `PinWindow.qml` 里 `saveAs` 同一族（这一台 Qt 上 `selectedFile.toLocalFile()` 量到的是
    // `undefined`，P37；`save::local_path` 是全场唯一一处把 `file:///…` 读回路径的手）。
    //
    // §5.5.11 步骤 3/4 的第三、第四条腿长在这一页而长在 P124 之前那两页之外：导出**就是**一次
    // 保存操作，那张原生框同样会压在贴图上。绑 `visible` 而不写 `onAccepted`/`onRejected`，理由
    // 与 `PinWindow.qml:2068` 同一封：「这张对话框不在了」就是步骤 4 的全部条件，接受、取消、
    // 还是用户直接关窗，三条腿之后贴图都该回来。这里既不判谁遮挡、也不递开关 —— 谁藏、藏几层、
    // 什么时候放回，全在 `falcon_core::dialog_link` 那一头记，屏幕上藏没藏由 `[output]
    // hide_pins_on_dialog` 一枚说了算（#281：两处各判一次就是两份事实）。
    FileDialog {
        id: exportDialog

        title: "导出配置"
        fileMode: FileDialog.SaveFile
        nameFilters: ["TOML 配置 (*.toml)"]
        defaultSuffix: "toml"
        onVisibleChanged: visible ? win.session.dialogOpened() : win.session.dialogClosed()
        onAccepted: win.doExport(String(exportDialog.selectedFile))
    }

    // 「导入配置」那张对话框：选完档当场只做「会盖掉哪几枚」这一问，写入要下面那枚「确认应用」
    // 按下去才发生（§5.20.3 步骤 5）。这里没有第二次 `open()`：真人按钮就是开这张对话框，别的
    // 什么都不按。
    //
    // 联动这一格走的是同一式：PRD §5.5.11 说的是「文件保存对话框」，而这一页把导出与导入做成
    // 一对镜像按钮 —— 只接一枚就是让同一扇窗的同一件事有两种答法。`dialog_opened` 记层数而不是
    // 记布尔，所以「导出框还没关就开导入框」那种叠法也有一本账（最后一张关掉才放回）。
    FileDialog {
        id: importDialog

        title: "导入配置"
        fileMode: FileDialog.OpenFile
        nameFilters: ["TOML 配置 (*.toml)", "所有文件 (*)"]
        onVisibleChanged: visible ? win.session.dialogOpened() : win.session.dialogClosed()
        onAccepted: win.doImportPreview(String(importDialog.selectedFile))
    }

    // 「导出调色板」那张对话框（§5.7.22 步骤 4）。落点由对话框决定，本页只把那串 URL 原样递过桥 ——
    // 与上面 `exportDialog` 同一族（这一台 Qt 上 `selectedFile.toLocalFile()` 量到的是 `undefined`，
    // P37；`save::local_path` 是全场唯一一处把 `file:///…` 读回路径的手）。
    //
    // 后缀是 txt 而不是 toml：步骤 4 递给人的是「一组色」，一枚一行、谁都能拿记事本改（写那份文件的
    // [`crate::settings::palette_export`] 就是按这个形状写的，读它的是 `Palette::from_text`）。
    //
    // 联动那一格也照上面两枚同一条接：这一页现在有四张原生框，只接两张就是让同一件事有两种答法 ——
    // `dialog_opened` 记的是层数，所以哪一张开着、开了几层，全在 `falcon_core::dialog_link` 那一头。
    FileDialog {
        id: palExportDialog

        title: win.palWords.length > 9 ? String(win.palWords[9]) : ""
        fileMode: FileDialog.SaveFile
        nameFilters: ["调色板 (*.txt)", "所有文件 (*)"]
        defaultSuffix: "txt"
        onVisibleChanged: visible ? win.session.dialogOpened() : win.session.dialogClosed()
        onAccepted: win.doPalIoExport(String(palExportDialog.selectedFile))
    }

    // 「导入调色板」那张对话框（§5.7.22 步骤 5 的第一步）：选完档当场只做「会换成哪一排」这一问，
    // 换要下面那枚「确认应用这一份」按下去才发生。这里没有第二次 `open()`：真人的钮就是开这张对话框。
    FileDialog {
        id: palImportDialog

        title: win.palWords.length > 10 ? String(win.palWords[10]) : ""
        fileMode: FileDialog.OpenFile
        nameFilters: ["调色板 (*.txt)", "所有文件 (*)"]
        onVisibleChanged: visible ? win.session.dialogOpened() : win.session.dialogClosed()
        onAccepted: win.doPalIoPreview(String(palImportDialog.selectedFile))
    }
}
