//! §5.14.4 的那一扇「历史记录管理」窗（PRD 第 1305–1317 行），与 §5.18.1 第 1438 行那枚托盘项。
//!
//! **这一档存在的原因**：PRD 第 1309 行写「用户打开“历史记录管理”。」，而 §5.18.1 那十枚菜单建议里
//! 「历史记录」那一枚至今在托盘上没有对面 —— `hotkey.rs` 头顶那封欠账信自己记着它
//! （「菜单建议里的 图片分组 / 历史记录 / 帮助 / 检查更新 四条，本 build 都还没有能跑的那只手」）。
//! 需要澄清的是：**§5.14.4 那四条规则里的每一条都不缺一只手**，缺的是那扇窗：
//! * 「选择单条、多条或全部记录」（第 1310 行）对面是 [`crate::mask::MaskState::delete_history`]（P98）；
//! * 「点击删除」（第 1311 行）对面是同一只手加 [`crate::mask::MaskState::clear_history`]（P99）；
//! * 「系统释放相应磁盘空间」（第 1312 行）对面是 `falcon_core::history::Freed`（P89）；
//! * 「清空全部历史前应二次确认」（第 1316 行）对面是 `clear_history(false)` 那一臂 —— 它**连库都不打开**；
//! * 「自动清理不得删除…锁定的记录」（第 1317 行）对面是 `clear_history(true)` 里钉死的 `include_locked = false`。
//!
//! 也就是说：这五格全部只被**量具**递过（`--autosave` 那一档经 `AnnotationToolbar.qml` 里那四条
//! history 腿叫过同一批 invokable），一次也没有被**一扇用户能打开的窗**递过。PRD 第 1309 行要的是
//! 后者，而 `HistoryWindow.qml` 在 P171 之前是一枚 0 命中的名字（前置档 `p171_pre_A.txt` 逐档现读）。
//! 这一档因此不重新发明任何一只手：窗上那四枚钮叫的是 `session.historyRows / historyDelete /
//! historyClear / historyLock` 那四枚**已经存在**的 invokable，本页**不写第二份判据、不数第二遍字节**。
//!
//! **本档手里唯一新东西**是两格：
//! 1. 「用户打开」这一格第一次有了被打开的东西 —— 一扇由 Rust 决定存在与否的窗（
//!    [`crate::session::qobject::Session::history_window_open`]，与 `custom_open` / `settings_open`
//!    同一副骨架：谁造这扇窗，销毁就归 `main.qml`）；
//! 2. §5.14.4 规则 1 自此有了**页上那一问**：窗上的「清空全部」不直接递 `true`，它先亮出
//!    [`CONFIRM_ASK`] 那一句，真人要点第二枚钮才轮到产品收到 `confirmed = true`。产品那枚 `bool`
//!    与页上那一问是**两把闸**而不是同一把闸写两遍：取消那一臂递 `false` 并让用户读回产品原话
//!    （「二次确认没点头，一行也不动。」），而不是让窗自己吞掉这一次点击、屏幕上什么都不说。
//!
//! **它不碰用户的历史**（§9.1 第十五条）：本档在 `engine.load()` 之前把 `MaskState` 的
//! `history` / `history_dir` 两格钉进一枚进程私有的 temp 沙盒（[`retarget`]），并在沙盒库里
//! 种下 [`ROWS`] 行已知尺寸的记录（[`arm`]）。窗上那几条腿删的就是这几行，所以第 1312 行
//! 「释放相应磁盘空间」有对面可读的字节数，而 `%APPDATA%\ai-falconshot\history\` 在这一趟
//! 前后一字也不动（由门外的尺量那一枚目录 sha，不靠本档这句话）。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::state::Check;

/// PRD 第 1309 行那一串字，**窗标题的唯一作者**。这一档把它当判据的一部分而不只是措辞：
/// 那一行写的不是「打开历史」也不是「历史浏览」，而是「历史记录管理」这四个字，
/// 门里 `history window` 那一格判的正是「页上那扇窗的 title 是不是这一串」。
pub const TITLE: &str = "历史记录管理";

/// §5.18.1 第 1438 行那一列菜单建议里的写法就是「历史记录」这四个字，**托盘那一项的名字唯一作者**
/// 是这一枚常数：[`crate::hotkey::Work::History`] 的 `label()` 读它，本档 ⑨ 那一格也判它。
/// 窗标题（[`TITLE`]）与菜单名分家是有意的两件事 —— 菜单要说「这里能翻到历史」，
/// 窗标题要念的是 PRD 第 1309 行给那扇窗起的那个名。
pub const TRAY_WORD: &str = "历史记录";

/// 页上那四枚钮的名字，次序就是 §5.14.4 那四步说话的样子：刷新（读第 1309 行之后要有东西）、
/// 删除所选（第 1310–1311 行「选择单条、多条」+「点击删除」）、清空全部（「或全部」+ 规则 1）、
/// 关闭。名单住在这一档而不是长在 `.qml` 里，与 `custom::CELLS` 同一理由：页上抄一份名单，
/// 改一天「钮上写着删除、递进产品的却是清空」只能靠数出来（#281）。
pub const BUTTONS: [&str; 4] = ["刷新", "删除所选", "清空全部", "关闭"];

/// 清单表头那一枚勾选框（第 1310 行「选择…**全部**记录」在页上的形状）。
pub const SELECT_ALL: &str = "全选";

/// 按下「清空全部」之后页上亮出的那一问（规则 1「应二次确认」的**页上那一半**）。
/// 两枚答钮的名字也在这里，因为页上抄一份就会与 [`crate::mask::MaskState::clear_history`]
/// 那句「二次确认没点头」分家成两套词汇。
pub const CONFIRM_ASK: &str = "真的要清空全部历史记录吗？这一问之后再点确认才会真的删。";
pub const CONFIRM_YES: &str = "确认清空";
pub const CONFIRM_NO: &str = "取消";

/// 量具那几条腿之间隔多久。这一档的腿**不等动画**（删与清都是同步过桥的一问一答），
/// 所以这一枚比 `custom::LEG_MS` 短；它是这一档自己的读数，印在 `[history-window wait]` 那一行。
pub const LEG_MS: u64 = 120;

/// 种进沙盒库的行数与各行的角色：**单条那一枚 / 多条那两枚 / 清空那一枚 / 锁定的那一枚**。
/// 五枚各有各的死法，没有一行是装饰 —— 少一行，§5.14.4 那四步里就有一步没人答。
pub const ROWS: usize = 5;

/// 闹钟地板：五枚种行 + 造窗 + 八条腿过一次桥 + 销毁。比 `custom::MIN_WINDOW_MS` 低，
/// 因为这一档没有延时那一条腿（那一枚要真的数满一枚秒）。
pub const MIN_WINDOW_MS: i32 = 6000;

/// 一页读多少行。**唯一作者**：窗那一侧第一页读这一枚（`historyWindowPageSize()` 把它递给
/// `HistoryWindow.qml`，页上自此不抄一个 `200`），更旧那一页也读这一枚（[`crate::mask::MaskState::list_history_older`]）。
///
/// 它是「一页」而不是「清单一共能有多少行」：§7.1 那句「历史记录加载应支持分页或虚拟列表」
/// 今天靠的是虚拟列表那一支（页上那枚 `ListView`），而第一页之外更旧的那些行此前**在屏上
/// 不可达** —— 库里第 201 行往后谁也点不到，而页上不会说一句「还有更旧的没显示」。分页那一支
/// 自此由 `older_than` 那一页接上（任务 #33）。
pub const PAGE_ROWS: i32 = 200;

/// 清单里此刻那几行原文（产品自己的读腿 [`crate::mask::MaskState::list_history`] 交回来的那一串，
/// `|` 分行）。回放那几格判据用它来问「这一枚行号还在不在库里」，本函数**不**自己开第二座库。
///
/// 筛那一枚递的是 `false`：本档种的那几行没有一行是被取消过的（[`plant`] 里 `cancelled: false`），
/// 开着「只看已取消」来问「第 7 行还在不在」，问出来的是一张空的筛过的清单，那一格就会红在
/// 谁也没筛过的东西上。页上那枚勾是真人的状态，不归本档替它按。
pub fn rows_now() -> Vec<String> {
    crate::mask::with(|m| m.list_history(PAGE_ROWS as usize, false))
        .split('|')
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// 那一行 `Record` 此刻在不在清单上。判的是**分节后的第一段整等**那一枚行号，与 [`row_cancelled`]
/// 同一把刀：`contains("7")` 会把第 17 行、把 1700×700 那一枚尺寸、把任何一处写着 7 的中文都
/// 当成第 7 行，于是「回放之后那一行还在」会被读成一次谁也没删过的幸存。
pub fn row_present(rows: &[String], id: i64) -> bool {
    rows.iter()
        .any(|l| l.split(" · ").next().unwrap_or("").trim() == id.to_string())
}

/// 本档 §5.14.2 第 4 步那三条回放腿（复制 / 保存 / 贴图）点名的那一枚行号：计划的第 1 段，
/// 也就是 [`ROWS`] 那五种角色里「单条那一枚」。**唯一作者仍是 [`arm`] 种下的那五行**，这一枚
/// 只是把同一串字从**门自己那份计划**里读回来，于是页上 `win.plan[1]` 与 Rust 这一头问的
/// 是同一个数（单测 `the_gauge_and_the_page_point_at_the_same_row` 钉的就是这一条）。
/// 读不出来 ⇒ `-1`，那一枚行号在库里不存在，产品交回的是它自己那句「库里没有第 -1 行」。
pub fn replay_id() -> i64 {
    with(|g| {
        let p: Vec<&str> = g.plan.split('|').collect();
        seg(&p, 1)
    })
}

/// 每一枚动作钮对应的那一条腿。**三格一起读**：钮上的名字（[`ACTIONS`] 的第几枚）、页上必须叫
/// 的那个 `cxx_name`、以及那条腿背后**已有的**那位主人。本表一位 handler 也不实现：它就是把
/// 「钮上写着保存、递进产品的却是贴图」这一族缺陷做成一格判据（`the_page_binds_the_leg_the_roster_names`
/// 拿它去扫 `HistoryWindow.qml`，`every_leg_of_the_roster_is_declared_on_the_bridge` 拿它去扫
/// `session.rs`）。主人那一列写的是**档与函数名**，不是这一档抄的判据 —— 它哪天挪了地方，
/// 扫源码那一格不会红（扫的是 cxx_name），而门里那一格会因为句子点名点不到而红。
pub const ROUTES: [(&str, &str, &str); ACTIONS.len()] = [
    (
        "复制",
        "historyCopy",
        "mask::MaskState::copy_history_content",
    ),
    (
        "保存",
        "historyReplay",
        "mask::MaskState::replay_history",
    ),
    (
        "贴图",
        "historyPin",
        "mask::replay_history + state::PinState::add_from_file",
    ),
    (
        "标注",
        "historyAnnotate",
        "capture::freeze + history::stamp_row + mask::MaskState::open",
    ),
    (
        "重进选区",
        "historyReenter",
        "mask::MaskState::restore_history_selection",
    ),
];

/// 翻页那两枚钮走的腿：两位名字、**一条**腿（`historyFlip(back)`）。§5.14.2 第 2 步要的是
/// 「上一条、下一条」两枚钮，而 §5.14.2 的翻页与 §5.14.3 的重新进入原选区在产品里对的是
/// 同一件事（`mask::flip_history_selection` 头上那段），所以这里两枚名字配同一枚 cxx_name ——
/// 配成两条就是让页上长出第二套「什么算翻成了一页」。
pub const FLIP_ROUTES: [(&str, &str); 2] = [
    (FLIP_BACK, "historyFlip"),
    (FLIP_FWD, "historyFlip"),
];

/// 动作名 → 页上该叫的那一条腿。认不出来的名字 ⇒ `None`（页上不许替它猜一条：猜出来的那条腿
/// 与钮上的名字没有任何东西保证相等）。
pub fn leg_of(name: &str) -> Option<&'static str> {
    ROUTES.iter().find(|(n, _, _)| *n == name).map(|(_, l, _)| *l)
}

/// 动作名 → 那条腿背后的主人（这一档不说它，只报它住在哪儿）。
pub fn owner_of(name: &str) -> Option<&'static str> {
    ROUTES.iter().find(|(n, _, _)| *n == name).map(|(_, _, o)| *o)
}

/// [`ROUTES`] 的原文，`|` 分枚、`>` 分三格。量具把它随 `replay-*` 那几行一起递回报告，
/// 读门的人因此能在同一格里看见「这一枚钮本该叫谁」。
pub fn routes() -> String {
    ROUTES
        .iter()
        .map(|(n, leg, owner)| format!("{n}>{leg}>{owner}"))
        .collect::<Vec<_>>()
        .join("|")
}

/// 产品那几句**拒绝**的原话前缀。字样不是本档定的：每一位都写在别人的 `format!` 里，
/// 而 `the_refusal_words_are_read_from_the_files_that_write_them` 那一格拿源码扫这一串 ——
/// 哪天负责那一句的档改了字样，红的是这一格，而不是窗上把一次拒绝报成一次成功。
/// 「图不在了」那一臂（`…的图取不回来（…），库里那一行还在。`）走的是同一位作者的前缀，
/// 所以本档不需要第二种「没有了」的说法。
pub const REFUSALS: [&str; 6] = [
    "内容没递",   // mask::copy_history_content
    "回放没跑",   // mask::replay_history
    "回放没写成", // mask::replay_history 的落盘那一臂
    "清单没读",   // mask::list_history
    "翻页没跑",   // session::history_flip
    "没递出去",   // platform_windows::clip 占用者那一臂接在产品前缀之后
];

/// 这一句是不是产品说「我没做成」。判的是**前缀**而不是全文含 `没`：产品的成功句里也可能出现
/// 「没」这个字（「剪贴板上此刻那份东西没动」是拒绝句的一部分，而「库里那一行还在」也是），
/// 含字判法会把成功读成拒绝、也会把拒绝读成成功，两种都往用户脸上递错话。
pub fn refused(said: &str) -> bool {
    let head = said.trim_start();
    REFUSALS
        .iter()
        .any(|w| head.starts_with(w) || head.contains(w))
}

/// 产品那句话有没有点名它动的是**这一枚**行号。门里三条回放腿与 `lock a row` 那一格共用的尺子，
/// 与 [`crate::history_window::measure`] 里第一版那句 `said.contains(&format!("第 {locked} 行"))`
/// 同形，只是抽出来给单测问得到。
pub fn names_row(id: i64, said: &str) -> bool {
    said.contains(&format!("第 {id} 行"))
}

/// 占用原文里 `records=` 那一枚数。读不出来 ⇒ `None`，理由与 [`image_bytes_of`] 同一条：
/// 把读不懂说成 0，就是让「回放长出了一行」有可能因为一句印歪了的话而绿。
pub fn record_count_of(usage: &str) -> Option<i64> {
    usage
        .split_whitespace()
        .find_map(|w| w.strip_prefix("records="))
        .and_then(|v| v.parse::<i64>().ok())
}

/// §5.14.2 第 4 步「用户可重新**复制、保存、标注或贴图**」那四件事在**每一行**上的名字，
/// 尾巴再加 §5.14.3 第 4 步「重新进入原选区」那一枚 —— 两节 PRD 加起来五件事，页上就是五枚钮，
/// 一件也不多、一件也不少。名单住在这一档而不是长在 `.qml` 里，与 [`BUTTONS`] 同一理由：
/// 页上抄一份名单，改一天「钮上写着保存、递进产品的却是贴图」就只能靠人数（#281）。
///
/// 次序是有意的一枚读数：QML 那一边按 `modelData === win.actions[i]`  dispatch，
/// 而这一档给每一枚名字配的是**一位**既有的手（复制 →
/// [`crate::mask::MaskState::copy_history_content`]、保存 →
/// [`crate::mask::MaskState::replay_history`]、贴图 → 那一位加
/// [`crate::state::PinState::add_from_file`]、标注 → 那一位加
/// [`falcon_core::history::stamp_row`]、重进选区 →
/// [`crate::mask::MaskState::restore_history_selection`]）。本档一只也不新造编解码、
/// 不新造剪贴板写入、不新造贴图装配。
pub const ACTIONS: [&str; 5] = ["复制", "保存", "贴图", "标注", "重进选区"];

/// §5.14.2 第 2 步「使用上一条、下一条快捷键浏览记录」在这一扇窗上的两枚名字。遮罩那一头
/// 早就有 Ctrl+↑/↓（`CaptureMask.qml`），这一头是同一对动作的**第二个入口**，走的仍是
/// `crate::session` 里那一扇 `history_flip` 的门 —— 名字在这里，落点与「什么算翻成了一页」
/// 仍旧只在 `mask` 那一档说了算，本档不另定义一次翻页。
pub const FLIP_BACK: &str = "上一条";
pub const FLIP_FWD: &str = "下一条";

/// §5.14.3 第 3 步「找到标记为**已取消**的记录」里那三个字。
///
/// **这一档不是它的作者**：清单里那一行挂不挂这三个字，由
/// [`crate::settings::history_rows_joined`] 那一处说了算（写进去的也是那一处）。本档只是把
/// 那位作者挂在行尾的那一节**读回来**，好让界面不必自己去猜中文字样 ——
/// 单测 `the_cancelled_mark_is_read_from_the_row_that_writes_it` 钉住的正是这条依赖：
/// 那一天 settings 改了字样而这里没跟上，红的是测试，而不是窗上少了一枚徽记、没人知道。
pub const CANCELLED_WORD: &str = "已取消";

/// §5.14.3 第 3 步「找到标记为“已取消”的记录」里**筛**那一半在页上的形状：那一枚勾选框的名字。
///
/// 它与 [`CANCELLED_WORD`] 是两件事而不是同一件事写两遍：后者是**挂在某一行末尾**的那两个字
/// （作者仍是 `crate::settings::history_rows_joined`，本档只读回来），前者是**用户手里那枚问句**
/// —— 「把清单筛成只剩带那一字的行」。三位读者共用这一串字（#281）：页上那枚框经
/// [`crate::session::Session::history_window_cancelled_word`] 读它，而
/// [`crate::mask::MaskState::list_history`] 与
/// [`crate::mask::MaskState::list_history_older`] 那两句「读了 … （只看已取消）：N 行」也插的
/// 是它 —— 屏上勾着的字样与产品说的筛法哪天分了家，用户就会看着一枚勾读一份全量清单。
pub const CANCELLED_FILTER_WORD: &str = "只看已取消";

/// 本档的计划与窗递回来的那些行。
struct Gate {
    /// 这一趟有没有一份计划。`false` 时 `measure` 交回 BLOCKED 而不是 FAIL —— 那一格问的是
    /// 「这一趟是产品会话还是本档」，而不是「页上少了一行」。
    armed: bool,
    /// 没 armed 的那一句理由，原样印进报告。
    reason: String,
    /// `腿等待 ms|单条行号|多条甲|多条乙|被清空那枚|锁定那枚|开工时行数`（7 段）。
    plan: String,
    /// 窗递回来的每一行原文。
    lines: Vec<String>,
    /// 沙盒落点，印在报告里给跑门的人核对「删的不是用户那一份」。
    hdir: PathBuf,
    /// 五枚行号，次序与 [`ROWS`] 那五种角色同序。
    ids: [i64; ROWS],
    /// 各行**此刻**在盘上值多少字节（缩略图 + 原图），删之前量到的。
    bytes: [u64; ROWS],
    /// 开工时的记录数与占用那一行原文。
    rows_at_arm: i64,
    usage_at_arm: String,
}

fn gate() -> &'static Mutex<Gate> {
    static G: OnceLock<Mutex<Gate>> = OnceLock::new();
    G.get_or_init(|| {
        Mutex::new(Gate {
            armed: false,
            reason: String::new(),
            plan: String::new(),
            lines: Vec::new(),
            hdir: PathBuf::new(),
            ids: [0; ROWS],
            bytes: [0; ROWS],
            rows_at_arm: 0,
            usage_at_arm: String::new(),
        })
    })
}

fn with<R>(f: impl FnOnce(&mut Gate) -> R) -> R {
    let mut g = gate().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut g)
}

/// 窗标题（页上 `title:` 绑的就是这一枚，桥是唯一作者）。
pub fn title() -> String {
    TITLE.to_string()
}

/// 四枚钮的名字，`|` 连接 —— 与 `custom::labels()` 同一把刀（名字里没有 `|`，页上只需要 `split("|")`）。
pub fn buttons() -> String {
    BUTTONS.join("|")
}

/// 表头那一枚勾选框上的两个字。
pub fn select_all() -> String {
    SELECT_ALL.to_string()
}

/// 「只看已取消」那一枚勾选框上的那几个字（[`CANCELLED_FILTER_WORD`] 的原文）。页上不抄中文，
/// 抄一份就多出第二位作者：改一天那句中文的人看得见桥、看不见屏幕（与 [`select_all`]、
/// [`actions`] 同一把刀）。
pub fn cancelled_filter_word() -> String {
    CANCELLED_FILTER_WORD.to_string()
}

/// 每一行那五枚动作钮的名字，`|` 连接 —— 与 [`BUTTONS`] 同一把刀。
pub fn actions() -> String {
    ACTIONS.join("|")
}

/// 翻页那两枚钮的名字，次序是 [`FLIP_BACK`]、[`FLIP_FWD`]（页上第一枚是"往旧里走"）。
pub fn flip_words() -> String {
    [FLIP_BACK, FLIP_FWD].join("|")
}

/// 清单里递回来的这一行，是不是 §5.14.3 第 3 步要找的那一条「已取消」的记录。
///
/// 判的是**整节相等**（` · ` 分节之后有一节正好是 [`CANCELLED_WORD`]），不是
/// `line.contains("已取消")`。差别在真实数据上是有的：`monitor_id` 那一列是从系统读回来的
/// 自由文本（[`crate::settings::history_rows_joined`] 只把里面的 `|` 洗成 `/`，其余原样印），
/// 一块叫「已取消的屏」的显示器会让 `contains` 那一版把一行**没有**取消的记录标成取消了的，
/// 而那一行正是用户要去点「重进选区」的那一枚。分节这把刀与页上 [`crate::settings`] 写这行
/// 时用的是同一串字，两边都改才会一起红 —— 这就是本函数存在的理由（见
/// `the_cancelled_mark_survives_a_row_that_only_looks_like_one`）。
pub fn row_cancelled(line: &str) -> bool {
    line.split(" · ").any(|seg| seg.trim() == CANCELLED_WORD)
}

/// 二次确认那三串字（一问 + 两枚答钮），按 [`CONFIRM_ASK`]、[`CONFIRM_YES`]、[`CONFIRM_NO`] 次序
/// `|` 连成一串：页上不写一份中文，写了就有两处作者。
pub fn confirm_words() -> String {
    [CONFIRM_ASK, CONFIRM_YES, CONFIRM_NO].join("|")
}

/// §6.2「显示历史记录占用多少空间」在**这一扇窗上**的读法：三臂各印得出东西 ——
/// 开着 ⇒ 真问 SQLite、关着 ⇒ 说开关、开不上 ⇒ 说打不开。算术与措辞的唯一作者仍是
/// [`crate::settings::history_usage_line`]（P110），本函数一行也不自己拼（#281）。
///
/// **读的是 [`crate::mask::MaskState`] 那两枚会话快照**而不是 [`crate::settings`] 的全局落点，
/// 理由与 [`crate::mask::MaskState::delete_history`] 头上那句同一条：这一趟的落点是本档钉进
/// 沙盒的那一枚，去问全局那一枚就把量具的数读成用户的数。
pub fn usage_line() -> String {
    let (cfg, dir) = crate::mask::with(|m| (m.history.clone(), m.history_dir.clone()));
    let (Some(cfg), Some(dir)) = (cfg, (!dir.as_os_str().is_empty()).then_some(dir)) else {
        return "history records=? original=?B thumb=?B db=?B total=?B (这一场没有历史)"
            .to_string();
    };
    match crate::settings::history_open_in(&dir, &cfg) {
        Ok(Some(store)) => match store.usage() {
            Ok(u) => crate::settings::history_usage_line(
                u.records,
                u.original_bytes,
                u.thumb_bytes,
                u.db_bytes,
            ),
            Err(e) => {
                format!("history records=? original=?B thumb=?B db=?B total=?B (usage 读不回: {e})")
            }
        },
        Ok(None) => "history records=0 original=0B thumb=0B db=0B total=0B (开关关着)".to_string(),
        Err(e) => format!("history records=? original=?B thumb=?B db=?B total=?B (打不开: {e})"),
    }
}

/// §8.7「提示用户清理历史或调整存储上限」在**这一扇窗上**的对面：占用顶到某一枚上限 ⇒ 那一句
/// 话；没顶到 ⇒ 空串，页上因此连那一格的高度也不给。
///
/// 判「顶到没有」不在这里，也不在措辞那一头：它读的是与自动清理**同一份**上限语义（`0` = 这一枚
/// 没设），所以住在 core 的 `Retention::at_cap` 里；本函数只把已经问出来的占用与上限交给
/// [`crate::settings::history_pressure_line`] 那唯一一位作者（#281）。
///
/// 「开关关着」「库打不开」「这一场没有历史」「`usage` 读不回」四臂一律交回空串而不是编一句
/// 「该清理了」：问不出占用就判不了上限，而上面那一行 §6.2 已经替这四臂各说了一句它自己的话
/// （那里写着 `records=?` 或 `(打不开: …)`），这里再猜一次就是给同一件坏第二个说法。
pub fn advice_line() -> String {
    let (cfg, dir) = crate::mask::with(|m| (m.history.clone(), m.history_dir.clone()));
    let (Some(cfg), Some(dir)) = (cfg, (!dir.as_os_str().is_empty()).then_some(dir)) else {
        return String::new();
    };
    let policy = falcon_core::history::Retention::from(&cfg);
    match crate::settings::history_open_in(&dir, &cfg) {
        Ok(Some(store)) => match store.usage() {
            Ok(usage) if policy.at_cap(&usage) => {
                crate::settings::history_pressure_line(usage.records, policy.max_items)
            }
            Ok(_) => String::new(),
            Err(_) => String::new(),
        },
        Ok(None) => String::new(),
        Err(_) => String::new(),
    }
}

/// 本档自己的沙盒落点：一枚进程私有的 temp 目录，可用 `FALCONSHOT_HISTORY_DOOR_DIR` 钉走
/// （跑门的那只 `.cmd` 钉的就是这一枚，于是那一档删掉的文件在门后仍可数）。
fn hist_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_HISTORY_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push(format!("falconshot-history-window-{}", std::process::id()));
    dir
}

/// 回放「保存 / 贴图」那两条腿的落点：一枚与沙盒库**并肩**的 temp 目录，可用
/// `FALCONSHOT_HISTORY_WINDOW_DOOR_DIR` 钉走。
///
/// 为什么这一档也要钉 `naming.dir` 而 `--autosave` 那档非钉不可、本档从前可以不钉：从前本档
/// 那四条腿只**删**东西，一句「删了历史第 N 行」不写盘；§5.14.2 第 4 步的「保存」与「贴图」
/// 走的却是 [`crate::save::quick_to` 那唯一一道落盘缝合]，而它写的名字用的是**用户自己的**
/// 命名规则与目录。不钉就是指门上按一次「保存」会在用户的图片文件夹里多出一张他没见过的新图
/// —— 那是 §9.1 第十五条那句话的反面。落点与库分开，因为那一格判的是「历史目录里没多出东西」，
/// 混在一枚目录里就数不出来了（与 `autosave_check::retarget` 同一分工）。
fn write_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_HISTORY_WINDOW_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "falconshot-history-window-out-{}",
        std::process::id()
    ));
    dir
}

/// 把 `MaskState` 那两枚落点钉进本档的沙盒，并把 `[history]` 那七枚键**全部钉成本档自己的形状**
/// （`enabled` / `keep_originals` / `thumb_px` / `exempt_locked` 都不跟着用户那份档走）。
///
/// 与 `autosave_check::retarget()` 的分别是有意的一件事而不是巧合：那一档把 `keep_originals` 与
/// `thumb_px` 留给用户的档，因为它判的是保留策略；本档判的是**删掉之后少了多少字节**，所以
/// 「那一行到底带着图」必须是本档说定的，否则同一扇门在 `keep_originals = false` 的机器上会
/// 量出另一份字节数，而那一格红的还是判据而不是那台机器。
///
/// 三枚上限钉 0：本档不要自动清理插进来动那五行 —— 种完立刻被 `cleanup` 收走的话，
/// 「窗上点删除」与「库里本来就有几行」就不是同一件事了。
fn retarget() -> PathBuf {
    let hdir = hist_dir();
    let wdir = write_dir();
    let _ = std::fs::remove_dir_all(&hdir);
    let _ = std::fs::remove_dir_all(&wdir);
    let _ = std::fs::create_dir_all(&hdir);
    let _ = std::fs::create_dir_all(&wdir);
    crate::mask::with(|m| {
        let mut h = crate::settings::history_cfg();
        h.enabled = true;
        h.save_all = false;
        h.max_items = 0;
        h.max_age_days = 0;
        h.max_size_mb = 0;
        h.keep_originals = true;
        h.thumb_px = 256;
        h.exempt_locked = true;
        m.history = Some(h);
        m.history_dir = hdir.clone();
        m.naming.dir = wdir.to_string_lossy().into_owned();
    });
    hdir
}

/// 一行种进沙盒后**立刻**问回来的那两样：行号与它此刻在盘上值多少字节。
///
/// 字节按 [`falcon_core::history`] 里 `bytes_of` 那三件算（缩略图 + 原图 `metadata` 长度），
/// 不是只读原图长度：本档判据对面是产品那句「收回 N 字节」，而 N 是两件之和（P115 立下的
/// 同一条口径）。读不回图 ⇒ 算 0，与 `bytes_of` 的 `None => thumb` 同形。
fn plant(
    store: &mut falcon_core::history::History,
    cfg: &falcon_core::config::History,
    role: &'static str,
    w: u32,
    h: u32,
    rgba: [u8; 4],
    at_ms: i64,
) -> Result<(i64, u64), String> {
    let frame = falcon_core::frame::Frame::filled(w, h, rgba)
        .map_err(|e| format!("{role}：图拼不出（{e}）"))?;
    let meta = falcon_core::history::CaptureMeta {
        at_ms,
        offset_min: platform_windows::clock::wall_clock().1,
        monitor_id: role,
        rect: falcon_core::PhysRect::new(4, 6, w, h),
        source: falcon_core::history::Source::Manual,
        cancelled: false,
    };
    let id = store
        .record(&meta, &frame, cfg)
        .map_err(|e| format!("{role}：写不进沙盒库（{e}）"))?;
    let rec = store
        .get(id)
        .map_err(|e| format!("{role}：种下第 {id} 行却读不回它（{e}）"))?;
    let thumb = rec
        .as_ref()
        .and_then(|r| r.thumb.as_ref())
        .map_or(0, |v| v.len() as u64);
    let path = rec.as_ref().and_then(|r| store.path_of(r));
    let file = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map_or(0, |m| m.len());
    Ok((id, thumb + file))
}

/// 在**引擎之前**钉落点、种五行、把行号写进计划。答的是这一趟印出来的那句话；
/// 前置摆不出来 ⇒ `armed = false` 而 `reason` 是那句原话 —— 计划保持空串，窗上一条腿也不跑，
/// `measure` 就此交回 BLOCKED 而不是拿不存在的行号去删。
pub fn arm() -> String {
    let hdir = retarget();
    let cfg = crate::mask::with(|m| m.history.clone());
    let Some(cfg) = cfg else {
        with(|g| {
            g.armed = false;
            g.reason = "钉完落点却读不回 [history] 那一份快照".to_string();
        });
        return format!("落点 {}：{}", hdir.display(), gate_reason());
    };
    let mut store = match crate::settings::history_open_in(&hdir, &cfg) {
        Ok(Some(s)) => s,
        Ok(None) => {
            with(|g| {
                g.armed = false;
                g.reason = "本档把 enabled 钉成真是白钉（分岔走关了）".to_string();
            });
            return format!("落点 {}：{}", hdir.display(), gate_reason());
        }
        Err(e) => {
            with(|g| {
                g.armed = false;
                g.reason = format!("打不开沙盒库（{e}）");
            });
            return format!("落点 {}：{}", hdir.display(), gate_reason());
        }
    };
    // 五枚种行**共用这一枚戳**（P103 那条读数订下来的同一条理由）：`newest` 先按 `captured_at`
    // 降序再按 `id` 降序，戳若各读各的钟，清单的头一段点名的就不是本档想要的那一行。
    let at_ms = falcon_core::capture::now_ms();
    let roles = [
        ("本档种的：单条那一枚", 64u32, 48u32, [255, 0, 0, 255]),
        ("本档种的：多条甲", 40, 32, [0, 255, 0, 255]),
        ("本档种的：多条乙", 36, 28, [0, 0, 255, 255]),
        ("本档种的：被清空那一枚", 32, 24, [255, 255, 0, 255]),
        ("本档种的：锁定那一枚", 24, 20, [255, 0, 255, 255]),
    ];
    let mut ids = [0i64; ROWS];
    let mut bytes = [0u64; ROWS];
    for (i, (role, w, h, rgba)) in roles.into_iter().enumerate() {
        match plant(&mut store, &cfg, role, w, h, rgba, at_ms) {
            Ok((id, b)) => {
                ids[i] = id;
                bytes[i] = b;
            }
            Err(e) => {
                with(|g| {
                    g.armed = false;
                    g.reason = e.clone();
                });
                return format!("落点 {}：{}", hdir.display(), gate_reason());
            }
        }
    }
    let usage = store
        .usage()
        .map(|u| {
            crate::settings::history_usage_line(
                u.records,
                u.original_bytes,
                u.thumb_bytes,
                u.db_bytes,
            )
        })
        .unwrap_or_else(|e| format!("history usage 读不回（{e}）"));
    let rows = store.count().unwrap_or(0) as i64;
    let plan = format!(
        "{LEG_MS}|{}|{}|{}|{}|{}|{rows}",
        ids[0], ids[1], ids[2], ids[3], ids[4]
    );
    with(|g| {
        g.armed = true;
        g.reason = String::new();
        g.plan = plan.clone();
        g.lines.clear();
        g.hdir = hdir.clone();
        g.ids = ids;
        g.bytes = bytes;
        g.rows_at_arm = rows;
        g.usage_at_arm = usage.clone();
    });
    format!(
        "沙盒 {}；计划 {plan}；种下 {ROWS} 行合计 {} 字节；开工占用 {usage}",
        hdir.display(),
        bytes.iter().sum::<u64>()
    )
}

fn gate_reason() -> String {
    with(|g| {
        if g.reason.is_empty() {
            "理由没写".to_string()
        } else {
            g.reason.clone()
        }
    })
}

/// 窗在 `Component.onCompleted` 读的那一串。空串 = 这一趟没有量具的腿要跑（产品会话里真人自己点，
/// 另外几道门里整条躲开 —— 量具的代码不许长成用户看得见的一次点击）。
pub fn plan_line() -> String {
    with(|g| g.plan.clone())
}

/// 这一趟该不该有一扇「历史记录管理」窗 —— `main.qml` 那一条 `syncHistory` 唯一的读数，
/// 与 [`crate::custom::window_wanted`] 同一分工。
pub fn window_wanted() -> bool {
    with(|g| g.armed)
}

/// 窗递回来的一行。前缀之后的那一段由 [`row_of`] 挑出来。
pub fn report(line: &str) {
    with(|g| g.lines.push(line.to_string()));
}

pub fn seen() -> Vec<String> {
    with(|g| g.lines.clone())
}

/// 同一个前缀递回来的那几段（**每一段前缀之后的全部内容**），按递上来的次序。
fn rows_of(prefix: &str) -> Vec<String> {
    with(|g| {
        g.lines
            .iter()
            .filter_map(|l| l.split_once('|'))
            .filter(|(k, _)| *k == prefix)
            .map(|(_, v)| v.to_string())
            .collect()
    })
}

/// 计划里那一段读成一个行号。读不出来 ⇒ `-1`，那一行在沙盒库里压根不存在，
/// 于是产品交回的是「点名的行号在库里一个也没有」而不是把判据蒙过去。
fn seg(plan: &[&str], i: usize) -> i64 {
    plan.get(i)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(-1)
}

/// 从产品那句话里读出「收回 N 字节」那一枚 N。读不出来 ⇒ `None`，格子当场红，
/// 而不是把一句没有字节数的好话当成一次成功的删除。
///
/// 「那一枚 N 紧跟着『收回』、且后面就是『字节』」是本档唯一的认法：产品写的是
/// `收回 {n} 字节`，**收回与数字之间有一枚空格**（`mask.rs` 的 `format!` 原文），
/// 而这一档的第一版按「收回之后第一个字符就是数字」去读，把三格删除全读成了 `None`
/// —— 单测里那三条断言当场把它抓住的。跳到第一个数字更宽松，也就更坏：一句
/// 「收回失败，剩下 3 行」会被读成释放了 3 字节。所以这里数完数字还要**点名它的单位**。
fn said_bytes(said: &str) -> Option<u64> {
    let tail = said.split("收回").nth(1)?.trim_start();
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || !tail[digits.len()..].trim_start().starts_with("字节") {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// 同一格只该被答一遍：递上来两枚就是页上有两处跑了同一条腿，而本档读的是第一枚 ——
/// 让它悄悄数第一枚，第二枚就永远读不到。数目不对当场红，话里带着枚数。
fn counted(rep: &mut crate::r13::Report, prefix: &str) -> Option<String> {
    let got = rows_of(prefix);
    if got.len() != 1 {
        rep.row(
            prefix,
            Check::Fail,
            format!("这条腿递了 {} 枚（应当一枚）", got.len()),
        );
        return None;
    }
    got.into_iter().next()
}

/// 删那一族的三条腿（单条 / 多条）共用的一格判定。
///
/// 判的是**行号与字节数同时**对得上：行号对而字节错，说明删的是别人那一行；字节对而行号错，反之。
/// 行号比的是集合而不是次序 —— [`falcon_core::history::Freed`] 里那串行号排的是 `History::delete`
/// 自己的那一份，本档不替 SQLite 规定先删哪一行（把它写进判据就是让判据跟着别人的排序走）。
fn delete_leg(
    rep: &mut crate::r13::Report,
    prefix: &str,
    label: &str,
    want: &[i64],
    want_bytes: u64,
    expect_rows: i64,
) {
    let Some(got) = counted(rep, prefix) else {
        return;
    };
    let s: Vec<&str> = got.split('|').collect();
    let said = s.first().copied().unwrap_or("");
    let shown = s.get(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
    let named: Vec<String> = said
        .split("删了历史第 ")
        .nth(1)
        .and_then(|rest| rest.split(" 行").next())
        .map(|names| names.split('、').map(str::to_string).collect())
        .unwrap_or_default();
    let mut want_s: Vec<String> = want.iter().map(i64::to_string).collect();
    let mut got_s = named;
    want_s.sort();
    got_s.sort();
    let (same_rows, right_bytes) = (want_s == got_s, said_bytes(said) == Some(want_bytes));
    rep.row(
        label,
        if same_rows && right_bytes && shown == expect_rows {
            Check::Pass
        } else {
            Check::Fail
        },
        format!(
            "{prefix} 递回：{said}｜点名的 {want:?} 对得上 {same_rows}、收回 {want_bytes} 字节对得上 \
             {right_bytes}、清单 {shown} 行应当 {expect_rows} 行"
        ),
    );
}

/// 那一行占用原文里 `original=` 与 `thumb=` 两枚数之和（见 [`image_bytes`] 头顶的理由）。
/// 任何一枚读不出来 ⇒ `None`：让一次读不懂的占用变成 0，就是让「释放」那一格因为
/// 一句印歪了的话而红，或者更坏 —— 因为两枚都读成 0 而绿。
fn image_bytes_of(usage: &str) -> Option<u64> {
    let mut out: Option<u64> = None;
    for word in usage.split_whitespace() {
        if let Some(v) = word.strip_prefix("original=") {
            out = Some(v.strip_suffix('B').unwrap_or(v).parse::<u64>().ok()?);
        } else if let Some(v) = word.strip_prefix("thumb=") {
            let v = v.strip_suffix('B').unwrap_or(v).parse::<u64>().ok()?;
            out = Some(out? + v);
        }
    }
    out
}

/// 这一档的报告：十三格判定 + 沙盒落点 + 窗递回的每一行原样附在后面。
///
/// 前八格判的是 §5.14.4 那一节（打开 / 全选 / 二次确认 / 单条 / 多条 / 锁 / 清空 / 释放空间），
/// ⑨–⑫ 四格判的是 §5.14.2 第 4 步与 §5.14.3 第 4 步（复制 / 保存 / 贴图，加「回放不改写记录」
/// 那一枚不变量），⑬ 判的是 §5.18.1 那枚托盘项。
///
/// 判的全是**窗那一侧**递回来的话；本档手里那份模型只用来对、不用来说（与 `--custom-window`
/// 同一分工）。窗一行也没递回来 ⇒ BLOCKED：那一格问的是「这一台机器上有没有那扇窗」，
/// 而 QML 文档编译不过与页上少一格是两种坏。
pub fn measure(want_ms: i32) -> (Check, String) {
    use crate::r13::Report;

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (armed, reason, plan, bytes, rows_at_arm, usage_at_arm, hdir) = with(|g| {
        (
            g.armed,
            g.reason.clone(),
            g.plan.clone(),
            g.bytes,
            g.rows_at_arm,
            g.usage_at_arm.clone(),
            g.hdir.clone(),
        )
    });
    if !armed {
        rep.row(
            "history window",
            Check::Blocked,
            format!("这一趟没有 {want_ms} ms 的计划：{reason}"),
        );
        return rep.finish("history-window door");
    }
    let seen = seen();
    if seen.is_empty() {
        rep.row(
            "history window",
            Check::Blocked,
            "窗没有递回任何一行 —— 「窗压根没被造出来」与「造出来了而腿没跑」在这一趟还没分开过；\
             分界是页上那行 `born`（见 `HistoryWindow.qml`）"
                .to_string(),
        );
        return rep.finish("history-window door");
    }
    let p: Vec<&str> = plan.split('|').collect();
    let (one, many_a, many_b, victim, locked) =
        (seg(&p, 1), seg(&p, 2), seg(&p, 3), seg(&p, 4), seg(&p, 5));
    let sum = |which: &[usize]| -> u64 { which.iter().map(|i| bytes[*i]).sum() };

    // ① 第 1309 行「用户打开“历史记录管理”」那一格的全部对面：窗在不在、标题是不是那一串字、
    //    页上真的造出几枚钮、清单这一趟列出几行。`visible` 是窗自己问 Qt 的那一枚数而不是
    //    「我们叫过 show()」，`btns` 是 delegate 的 `count` 而不是名单长度（与 `custom` ① 同一口径）。
    if let Some(open) = counted(&mut rep, "open") {
        let s: Vec<&str> = open.split('|').collect();
        let visible = s.first().copied().unwrap_or("");
        let btns = s.get(1).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
        let titled = s.get(2).copied().unwrap_or("");
        let listed = s.get(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        // 第五枚读数是**行上**那一排钮的枚数（delegate 里那一枚 Repeater 的 `count`）：
        // §5.14.2 第 4 步那四件事加 §5.14.3 第 4 步那一件，要在清单的每一行上都造得出五枚，
        // 而不是页顶再来一排。名字对得上而枚数不对 ⇒ 页上少长了钮，而名单还是那五枚。
        let acts = s.get(4).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
        let ok = visible == "1"
            && btns == BUTTONS.len()
            && titled == "1"
            && listed == rows_at_arm
            && acts == ACTIONS.len();
        rep.row(
            "history window",
            if ok { Check::Pass } else { Check::Fail },
            format!(
                "visible={visible} 页顶钮={btns}/{} 行上钮={acts}/{} 标题={titled} 清单 {listed} 行（开工 {rows_at_arm} 行）",
                BUTTONS.len(),
                ACTIONS.len()
            ),
        );
    }

    // ② 第 1310 行「选择…**全部**记录」：表头那一枚全选点下去，勾上的枚数与清单行数、
    //    与开工时那几行三者必须同数。勾上 0 枚也是一种"点到了"，所以三枚数一起判。
    if let Some(all) = counted(&mut rep, "select-all") {
        let s: Vec<&str> = all.split('|').collect();
        let checked = s.first().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let shown = s.get(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        rep.row(
            "select all",
            if checked == rows_at_arm && shown == checked {
                Check::Pass
            } else {
                Check::Fail
            },
            format!("勾上 {checked} 枚 / 清单 {shown} 行 / 开工 {rows_at_arm} 行"),
        );
    }

    // ③ 规则 1「清空全部历史前应二次确认」的第一臂：页上那一问亮出来、真人答「取消」，
    //    产品收到的仍是 `false`，交回的仍是它那一句原话，清单少一行都算红。
    if let Some(no) = counted(&mut rep, "clear-unconfirmed") {
        let s: Vec<&str> = no.split('|').collect();
        let said = s.first().copied().unwrap_or("");
        let asked = s.get(1).copied().unwrap_or("");
        let shown = s.get(2).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let ok = said.contains("二次确认没点头") && asked == "1" && shown == rows_at_arm;
        rep.row(
            "confirm before clear",
            if ok { Check::Pass } else { Check::Fail },
            format!("页上先亮一问（问到={asked}）再答；取消递出去那句 = {said}；清单仍 {shown} 行"),
        );
    }

    // ④⑤ 第 1310–1311 行那两步的另两档：单条与多条。
    delete_leg(
        &mut rep,
        "delete-one",
        "delete one",
        &[one],
        sum(&[0]),
        rows_at_arm - 1,
    );
    delete_leg(
        &mut rep,
        "delete-many",
        "delete many",
        &[many_a, many_b],
        sum(&[1, 2]),
        rows_at_arm - 3,
    );

    // ⑥ 规则 2 的前半：那一枚**锁**是产品自己打上的（`historyLock`），不是本档种下去的 ——
    //    「用户明确收藏或锁定」这半句要的对面是"用户能按的地方"，而按完之后清单上那一行
    //    自己得写着锁定，否则页与库各说一句。
    //
    //    三枚都要对上才算：产品那句话得是**锁上**那一条臂（`mask.rs` 的 `format!` 原文是
    //    「锁上了历史第 N 行…」，第一版在本档里 grep 的是「锁定」——那是**清单行**与清空那句
    //    的字，不是锁这一臂的字，于是页上真的锁成了而本档判红，generation A 的
    //    `lock a row` 就是这么红的），那句话得点名本档计划里的那一枚行号，而清单上读回的
    //    那一行自己得写着「已锁定」。
    if let Some(lock) = counted(&mut rep, "lock") {
        let s: Vec<&str> = lock.split('|').collect();
        let said = s.first().copied().unwrap_or("");
        let marked = s.get(1).copied().unwrap_or("");
        let named = said.contains(&format!("第 {locked} 行"));
        rep.row(
            "lock a row",
            if said.contains("锁上") && named && marked == "1" {
                Check::Pass
            } else {
                Check::Fail
            },
            format!(
                "第 {locked} 行：{said}（点名第 {locked} 行 = {named}、清单上读回的锁 = {marked}）"
            ),
        );
    }

    // ⑦ 规则 1 的另一臂（真人点了确认）与规则 2 的后半同判：清掉的只有没锁的那一枚，
    //    锁着的那一枚按产品自己那句话留下。
    if let Some(cleared) = counted(&mut rep, "clear-confirmed") {
        let s: Vec<&str> = cleared.split('|').collect();
        let said = s.first().copied().unwrap_or("");
        let shown = s.get(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let survivor = s.get(2).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let right_bytes = said_bytes(said) == Some(bytes[3]);
        let ok = shown == 1 && survivor == locked && right_bytes && said.contains("锁定项");
        rep.row(
            "clear keeps locked",
            if ok { Check::Pass } else { Check::Fail },
            format!(
                "确认后递回：{said}｜清单剩 {shown} 行（应当 1）、活下来的那枚 = {survivor} 应当 \
                 {locked}、该被清掉的那枚 = {victim}（收回 {} 字节对得上 {right_bytes}）",
                bytes[3]
            ),
        );
    }

    // ⑧ 第 1312 行「系统释放相应磁盘空间」：删前与删后各量一次占用，差必须等于产品四条腿各自
    //    说过的那些字节之和 —— 而不是让产品那句话自己给自己作证。取 `original + thumb` 两枚而不取
    //    `total`：`total` 含 `history.db` 自己那一份，而 SQLite 删行不缩库（理由写在 [`image_bytes_of`]）。
    if let Some(after) = counted(&mut rep, "usage-after") {
        let before = image_bytes_of(&usage_at_arm);
        let now = image_bytes_of(&after);
        let want = sum(&[0, 1, 2, 3]);
        let released = match (before, now) {
            (Some(b), Some(a)) if b >= a => Some(b - a),
            _ => None,
        };
        rep.row(
            "space released",
            if released == Some(want) {
                Check::Pass
            } else {
                Check::Fail
            },
            format!(
                "删前 {before:?}（{usage_at_arm}）→ 删后 {now:?}（{after}）；差 {released:?} 应当等于\
                 产品说过收回的字节之和 {want}"
            ),
        );
    }

    // ⑨⑩⑪ §5.14.2 第 4 步「用户可重新**复制、保存**……**贴图**」那三件事在这一扇窗上的形状。
    //     三条腿各自递回 `腿名|点名的行号|产品原话`（**行号在前**：产品那句里可能带着占用剪贴板
    //     那个窗的标题，而标题里出现 `|` 不该把后面的字段挤走）。判的是两件事：那句话点名了
    //     本档计划里的那一枚行号，而且它不是产品自己那句拒绝。
    //
    //     「剪贴板此刻被谁占着」是环境的答案而不是页上少了一条腿，所以 copy 那一臂被拒时记
    //     `Blocked` 并**原样附上产品的话**（同 reveal_check 那档对「探针举不到就不判」的处理）；
    //     点名点不到、或句子里压根没有那一枚行号，才是页上真的没递到那条腿 —— 那是 FAIL。
    for (prefix, label, leg) in [
        ("replay-copy", "replay copy", "historyCopy"),
        ("replay-save", "replay save", "historyReplay"),
        ("replay-pin", "replay pin", "historyPin"),
    ] {
        let Some(got) = counted(&mut rep, prefix) else {
            continue;
        };
        let s: Vec<&str> = got.split('|').collect();
        let named = seg(&s, 0);
        let said = s.get(1..).map(|t| t.join("|")).unwrap_or_default();
        let hits = names_row(named, &said);
        let no = refused(&said);
        // 被点名那一枚行号在本档计划里的位置：`one` 是回放腿的靶（[`replay_id`]），与页上
        // `win.plan[1]` 同一个数。靶不对 ⇒ 这一条腿递回来的句子再像话也是在说别的一行。
        let on_target = named == one && named > 0;
        let verdict = if !on_target {
            Check::Fail
        } else if no && said.contains("剪贴板") {
            Check::Blocked
        } else if no || !hits {
            Check::Fail
        } else {
            Check::Pass
        };
        rep.row(
            label,
            verdict,
            format!(
                "{leg} 递回（第 {named} 行，靶={one}）：{said}｜点名本档那一枚 {hits}、拒了 {no}"
            ),
        );
    }

    // ⑫ §5.14.3 那一步的**不变量**在这一扇窗上的对面：回放一枚行之后，库里的行数、历史目录里
    //    那些图的字节、被回放的那一行还在不在，三件事都不许动。页上递回的是**产品那两行占用的原文**
    //    （回放前后各读一遍 `historyWindowUsage()`）再加贴图枚数与「那一行在场吗」两枚 0/1，本档
    //    只比这两个读数 —— 把「没长出一行」写成页上的一句自信，就是让这一格跟着页上的措辞走而不是
    //    跟着 SQLite 走；而让页自己去把 `original=` 与 `thumb=` 相加，就是在本档之外长出第二份
    //    [`image_bytes_of`]（#281）。枚数与在场那两枚是**读**而不是算，页上递数没有这个问题。
    //
    //    贴图那枚数（`pins`）**该**加一，而它加在哪儿不归这一格判：那是 §5.9.1 那一节的账。
    if let Some(shape) = counted(&mut rep, "replay-shape") {
        let s: Vec<&str> = shape.split('|').collect();
        // 页上递的是**两次读数各三段**：`占用原文|贴图枚数|那一行在场吗`，前后各一遍。
        let (usage_before, usage_after) = (
            s.first().copied().unwrap_or(""),
            s.get(3).copied().unwrap_or(""),
        );
        let num = |i: usize| s.get(i).and_then(|v| v.parse::<i64>().ok());
        let (pin_before, pin_after) = (num(1), num(4));
        let (here_before, here_after) = (num(2), num(5));
        let (rec_before, rec_after) = (record_count_of(usage_before), record_count_of(usage_after));
        let (byte_before, byte_after) = (image_bytes_of(usage_before), image_bytes_of(usage_after));
        let same_rows = match (rec_before, rec_after) {
            (Some(b), Some(a)) => b == a,
            _ => false,
        };
        let same_bytes = match (byte_before, byte_after) {
            (Some(b), Some(a)) => b == a,
            _ => false,
        };
        let still_there = here_before == Some(1) && here_after == Some(1);
        let pinned = match (pin_before, pin_after) {
            (Some(b), Some(a)) => a == b + 1,
            _ => false,
        };
        rep.row(
            "replay leaves history alone",
            if same_rows && same_bytes && still_there && pinned {
                Check::Pass
            } else {
                Check::Fail
            },
            format!(
                "回放前 记录 {rec_before:?} 行 / 图 {byte_before:?} B / 第 {one} 行在场 {here_before:?} \
                 / 贴图 {pin_before:?} 枚 → 回放后 记录 {rec_after:?} 行 / 图 {byte_after:?} B / 在场 \
                 {here_after:?} / 贴图 {pin_after:?} 枚（判的是：行数与历史目录里的字节一字不动、那一行\
                 还在、贴图多一枚）；两行占用原文 {usage_before} ｜ {usage_after}"
            ),
        );
    }

    // ⑬ §5.18.1 第 1438 行那枚「历史记录」：判的是**装配出来的那份菜单**里有它，而不是
    //    `Work::History` 这一枚枚举存在。菜单的唯一作者是 `hotkey::tray_menu`，本档只问它要一份名字。
    let menu = crate::hotkey::tray_labels();
    let has = menu.iter().any(|name| name == TRAY_WORD);
    rep.row(
        "tray 历史记录",
        if has { Check::Pass } else { Check::Fail },
        format!(
            "菜单 {} 项：{}（找的是「{TRAY_WORD}」那一枚原名）",
            menu.len(),
            menu.join(" / ")
        ),
    );

    rep.note("沙盒落点", format!("{}", hdir.display()));
    rep.note("窗递回的行", format!("{} 枚", seen.len()));
    for line in seen {
        rep.note("", line);
    }
    rep.finish("history-window door")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r13::Report;

    /// §9.1 第一位单测证人：`image_bytes_of` 是本档「释放」那一格唯一的算术，而它读的是
    /// **别人拼出来的那一行**（[`crate::settings::history_usage_line`]）。三臂各问一问：
    /// 正常一行、开关关着那一行、读不回那一行 —— 最后一臂要的答是 `None` 而不是 `Some(0)`，
    /// 因为把读不懂说成 0 就是让一次失败的删除有可能绿。
    #[test]
    fn the_usage_line_is_counted_not_guessed() {
        let said = crate::settings::history_usage_line(3, 1_000, 200, 4_096);
        assert_eq!(image_bytes_of(&said), Some(1_200));
        assert_eq!(
            image_bytes_of("history records=0 original=0B thumb=0B db=0B total=0B (开关关着)"),
            Some(0)
        );
        assert_eq!(image_bytes_of("打不开 history.db"), None);
        assert_eq!(
            image_bytes_of("history records=? original=?B thumb=?B"),
            None
        );
    }

    /// §8.7 那一问在这一扇窗上的接线：页只把桥交回的那一串画出来，一个字也不自己拼（#281）。
    /// 读的是**去掉注释行以后**的窗原文 —— 与 [`crate::settings_view`] 里为 `exclusionWord` 立的
    /// 同一把刀：散文里提一次那几个字不算抄写，那正是注释在做的事。
    #[test]
    fn the_advice_line_is_drawn_from_the_bridge_not_retyped() {
        const QML: &str = include_str!("../qml/HistoryWindow.qml");
        let drawn: Vec<&str> = QML
            .lines()
            .map(str::trim)
            .filter(|t| !t.starts_with("//"))
            .collect();
        // ① 那一问从桥读，而且与占用同一次 `readRows` —— 删过一行之后顶不顶到上限是换了数的。
        assert!(
            drawn.iter().any(|l| l.contains("win.session.historyWindowAdvice()")),
            "那一问没从桥读"
        );
        assert!(
            drawn.iter().any(|l| l.contains("win.advice")),
            "画的那一格没把 `win.advice` 接过来"
        );
        // ② 页上不抄那串中文：抄一份，改一天措辞就得开两处，而晚一步的那一处永远是屏幕。
        for phrase in ["已到存储上限", "自动清理只动", "把上限调大"] {
            assert!(!drawn.iter().any(|l| l.contains(phrase)), "页上抄走了「{phrase}」");
        }
        // ③ 那句话此刻确实只出自那唯一一位作者。
        let said = crate::settings::history_pressure_line(9, 9);
        assert!(
            said.contains("已到存储上限") && said.contains("自动清理只动") && said.contains("把上限调大"),
            "{said}"
        );
    }

    /// 「收回 N 字节」那一枚 N 只从产品那句话里读，读不出来就是 `None`（判据当场红）。
    /// 三臂：删成了、清空成了、被拒了 —— 第三臂**没有**字节数，它必须是 `None`，
    /// 否则一次拒绝会被读成一次释放了 0 字节的成功。
    #[test]
    fn the_released_bytes_are_the_products_own_sentence() {
        assert_eq!(said_bytes("删了历史第 7 行，收回 123 字节。"), Some(123));
        assert_eq!(
            said_bytes("清空了历史 2 行，收回 9 字节；锁定项按 §5.14.4 那条规则留下。"),
            Some(9)
        );
        assert_eq!(said_bytes("清空没跑：二次确认没点头，一行也不动。"), None);
    }

    /// 门的前置：钉落点、种五行、计划七段读回来正是那五种角色。跑完把这一档钉回没 armed 的样子，
    /// 免得 `window_wanted()` 在后面的单测里替产品会话招出一扇窗。
    #[test]
    fn the_plan_names_the_five_rows_it_planted() {
        // `arm()` 会把本档那枚**进程私有**的沙盒目录连根删掉再种，同趟里另一枚 arm 单测
        // 正握着那座 SQLite 时就得到 `database is locked`（P187 第一趟正是这一条红的）。
        let _guard = plan_lock();
        let said = arm();
        assert!(said.contains("计划 "), "{said}");
        assert!(window_wanted(), "{said}");
        let planned = plan_line();
        let p: Vec<&str> = planned.split('|').collect();
        assert_eq!(p.len(), 7, "{p:?}");
        assert_eq!(p[0], LEG_MS.to_string());
        for i in 1..=5 {
            assert!(
                seg(&p, i) > 0,
                "第 {i} 段读不成行号（{}），沙盒库里那五行没种成：{said}",
                p[i]
            );
        }
        assert_eq!(seg(&p, 6), ROWS as i64);
        with(|g| {
            g.armed = false;
            g.plan.clear();
            g.lines.clear();
        });
    }

    /// 同一格被答两遍 ⇒ `counted` 交回 `None` 并**自己**记一格 FAIL：这一格判的是本档的读法，
    /// 而不是窗那侧少跑了一条腿。删那一族少一枚读数就少一格判定，这里钉住"两枚也是红"。
    #[test]
    fn a_leg_answered_twice_is_red_not_silently_first_wins() {
        report("dup|first");
        report("dup|second");
        let mut rep = Report::new();
        assert_eq!(counted(&mut rep, "dup"), None);
        report("solo|only");
        assert_eq!(counted(&mut rep, "solo").as_deref(), Some("only"));
        with(|g| g.lines.clear());
    }

    /// §5.14.2 第 4 步那四件事 + §5.14.3 第 4 步那一件事，在名单上就是五枚、各一名、
    /// 次序固定、名字里不含 `|`（页上只需要 `split("|")`）。这一格钉的是**覆盖**：
    /// 五枚里少一枚，PRD 那一行就还有一件事没有真人能点到 —— 本仓库那一族「逻辑齐备、
    /// 生产路径零调用」正是这样长回来的。
    #[test]
    fn the_action_roster_covers_every_replay_the_prd_asks_for() {
        assert_eq!(
            ACTIONS,
            ["复制", "保存", "贴图", "标注", "重进选区"],
            "§5.14.2 第 4 步的四枚 + §5.14.3 第 4 步的重进选区，一枚也不能少"
        );
        let joined = actions();
        assert_eq!(joined.split('|').count(), ACTIONS.len(), "`|` 只能当分隔符用");
        let mut uniq: Vec<&str> = ACTIONS.to_vec();
        uniq.sort_unstable();
        let before = uniq.len();
        uniq.dedup();
        assert_eq!(before, uniq.len(), "两枚钮同名，页上就分不出递的是哪只手");
        assert_eq!(flip_words(), "上一条|下一条");
    }

    /// 那一行是不是「已取消」的，判据要钉在**写这一节的那一位**身上：这里不自己造字符串，
    /// 而是拿 [`crate::settings::history_rows_joined`] 真的拼一行出来读回来。settings 那天
    /// 改了字样，这一格当场红 —— 而不是窗上少一枚徽记、没人知道。
    #[test]
    fn the_cancelled_mark_is_read_from_the_row_that_writes_it() {
        let rows = [
            row(7, "主屏", false, false),
            row(8, "主屏", true, false),
            row(9, "主屏", true, true),
        ];
        let joined = crate::settings::history_rows_joined(&rows, -480);
        let lines: Vec<&str> = joined.split('|').collect();
        assert_eq!(lines.len(), 3, "三行该恰好三个段：{joined}");
        assert!(!row_cancelled(lines[0]), "没取消的那一行不该被标上：{}", lines[0]);
        assert!(row_cancelled(lines[1]), "取消的那一行要认得出来：{}", lines[1]);
        assert!(
            row_cancelled(lines[2]),
            "又取消又锁定的那一行仍是 §5.14.3 要找的那一条：{}",
            lines[2]
        );
    }

    /// `contains` 那一版会在这里说谎：`monitor_id` 是自由文本（settings 只洗掉里面的 `|`），
    /// 一块叫「已取消的屏」的显示器能把一行**没有**取消的记录标成取消了的，而用户点的正是
    /// 那一枚「重进选区」。分节整等那一版必须挡住这一臂。
    #[test]
    fn the_cancelled_mark_survives_a_row_that_only_looks_like_one() {
        let tricky = row(11, "已取消的屏", false, false);
        let line = crate::settings::history_rows_joined(&[tricky], -480);
        assert!(line.contains(CANCELLED_WORD), "夹具要让 `contains` 那一版上当：{line}");
        assert!(
            !row_cancelled(&line),
            "屏名里带着那三个字，不等于这一行被取消过：{line}"
        );
        // 反向也要挡住：真的那一节前后多一个空格仍旧算（页上抄的那一份不能因为空白分家）。
        assert!(row_cancelled(
            "12 · 2026-01-01 00:00 · 位置 10,20 · 64×32 · 主屏 · region · 已取消 "
        ));
    }

    /// 一行 `Record` 夹具。字段一次写全而不是 `..Default::default()`：`Record` 哪天多一列，
    /// 这里编译不过，比让新列带着默认值混进判据好。
    fn row(id: i64, monitor: &str, cancelled: bool, locked: bool) -> falcon_core::history::Record {
        falcon_core::history::Record {
            id,
            captured_at: 1_700_000_000_000,
            monitor_id: monitor.to_string(),
            rect: falcon_core::PhysRect::new(10, 20, 64, 32),
            source: falcon_core::history::Source::Region,
            path: None,
            thumb: None,
            cancelled,
            locked,
            ocr_text: None,
            note: String::new(),
        }
    }

    // ── §5.14.2 第 4 步 / §5.14.3 第 4 步：那五枚钮的**接线**判据 ──────────────────────────
    //
    // 三格源码扫描（`include_str!`），与本仓 `hint_check::a_camel_cased_session_notify_never_hangs_on_a_window`
    // 同一族：运行期的证人只有门跑起来才在场，而「QML 按钮没有后端」这一族缺陷**不会报错**——
    // 页上一枚钮叫一条不存在的腿，Qt 顶多往 stderr 印一行，屏幕上那枚钮照样能按，按下去什么也不发生。
    // 门不是每次都跑，所以这一族在编不出来的时候就得红。
    #[cfg(test)]
    const PAGE: &str = include_str!("../qml/HistoryWindow.qml");
    #[cfg(test)]
    const BRIDGE: &str = include_str!("session.rs");
    #[cfg(test)]
    const MASK_SRC: &str = include_str!("mask.rs");
    /// 分岔本体那一头（`history_rows_older_at` 住在 `settings.rs`）。上面那格扫的是桥与页，
    /// 这一格扫的是**再往里一段**：接线接到 mask 为止不算通，本体的对面得真的是 core 那一枚
    /// 翻页的手。
    #[cfg(test)]
    const SETTINGS_SRC: &str = include_str!("settings.rs");

    /// 一把尺子锁两枚会互相踩的 arm 单测（`arm()` 写的是进程那一份 [`gate`]，而 cargo 同进程
    /// 多线程跑测试）。锁住的那两枚各自把门钉回没 armed 的样子，所以第三枚读 `window_wanted()`
    /// 的单测不必排队。
    fn plan_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
        L.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// [`ROUTES`] 与 [`ACTIONS`] 是同一张名单的两列：同长、同序、一枚不多一枚不少，
    /// 每一枚都有腿名与主人。这一格钉的是**表自己**；表与页、表与桥的对账在下面两格。
    #[test]
    fn every_action_has_exactly_one_leg_and_one_owner() {
        assert_eq!(
            ROUTES.len(),
            ACTIONS.len(),
            "钮与腿的枚数分了家：{:?} vs {:?}",
            ACTIONS,
            ROUTES
        );
        for (i, (name, leg, owner)) in ROUTES.iter().enumerate() {
            assert_eq!(*name, ACTIONS[i], "第 {i} 枚的名字与名单分家");
            assert!(!leg.is_empty() && !leg.contains('|'), "第 {i} 枚的腿名不是一枚词");
            assert!(!owner.is_empty(), "{name} 没有主人 —— 那一枚钮就是个装饰");
            assert_eq!(leg_of(name), Some(*leg), "{name} 问不回自己的腿");
            assert_eq!(owner_of(name), Some(*owner), "{name} 问不回自己的主人");
        }
        // 认不出来的名字不许被猜成任何一条腿（页上递错名字时这一格挡住「猜一条最像的」）。
        assert_eq!(leg_of("删除所选"), None);
        assert_eq!(leg_of(""), None);
        assert_eq!(routes().split('|').count(), ROUTES.len());
        // 翻页两枚名字走**同一条**腿：两枚配两条就是页上长出第二套「什么算翻成了一页」。
        assert_eq!(FLIP_ROUTES.len(), 2);
        assert_eq!(FLIP_ROUTES[0].1, FLIP_ROUTES[1].1, "上一条与下一条该走同一条腿");
        assert_eq!(FLIP_ROUTES[0].0, FLIP_BACK);
        assert_eq!(FLIP_ROUTES[1].0, FLIP_FWD);
    }

    /// 名单上每一枚钮，在 `HistoryWindow.qml` 的**代码**里（剥过注释，尺子是
    /// `hint_check::qml_code`，全档唯一一把）真的叫了它那一条例子；反向也判：页上叫的
    /// `history*` 腿必须是名单上的那五条加已实现的读腿，一枚游散的 handler 也算红。
    #[test]
    fn the_page_binds_the_leg_the_roster_names() {
        let code = crate::hint_check::qml_code(PAGE);
        assert!(
            code.contains("session.historyWindowReport("),
            "剥注释之后连量具那条腿都不在了 ⇒ 这把尺子今天量的是空集（#269）"
        );
        for (name, leg, _) in ROUTES.iter() {
            let call = format!("session.{leg}(");
            assert!(
                code.contains(&call),
                "钮「{name}」该叫 {call}，页上的代码里没有它 —— 「逻辑齐备、生产路径零调用」那一族"
            );
        }
        for (name, leg) in FLIP_ROUTES.iter() {
            assert!(
                code.contains(&format!("session.{leg}(")),
                "翻页那枚「{name}」没叫 {leg}"
            );
        }
        // 名字出自桥，不在页上抄一份中文（与 `win.buttons` 同一口径）。
        assert!(
            code.contains("win.actions[") || code.contains("win.actionWord("),
            "页上按名字派活的那一条腿没读到桥递回的名单，而是在抄一份中文"
        );
        assert!(
            !code.contains('"') || !code.contains("\"复制\"") ,
            "页上抄了一份动作名单的中文（名单的唯一作者是 [`ACTIONS`]）"
        );
        // 五枚钮该长在**每一行**上：delegate 里那枚 Repeater 的 model 是名单。
        assert!(
            code.contains("model: win.actions"),
            "行上的钮不是从名单来的，门里数不出五枚"
        );
    }

    /// 每一枚被点名的腿，在 `Session` 那副桥上**有声明**（`#[cxx_name = "…"]`）。这一格是
    /// 「QML 按钮没有后端」的另一半尺子：页上叫得再响，桥没声明就是按下去什么都不发生，
    /// 而 cxx-qt 的错只往 stderr 印一行。
    #[test]
    fn every_leg_of_the_roster_is_declared_on_the_bridge() {
        let mut checked = 0usize;
        for leg in ROUTES
            .iter()
            .map(|(_, l, _)| *l)
            .chain(FLIP_ROUTES.iter().map(|(_, l)| *l))
        {
            let needle = format!("#[cxx_name = \"{leg}\"]");
            assert!(
                BRIDGE.contains(&needle),
                "桥上没有 {leg} 的声明：页上那一枚钮递到的是空气（名单见 [`ROUTES`]）"
            );
            checked += 1;
        }
        assert_eq!(
            checked,
            ROUTES.len() + FLIP_ROUTES.len(),
            "扫描一枚也没跳过 ⇒ 这一格才是空的"
        );
        // 名字也是 qproperty 的话就该按 snake_case 绑；这一格只是把话说死：这五条里没有一条是属性。
        for (name, leg, _) in ROUTES.iter() {
            assert!(
                !BRIDGE.contains(&format!("#[qproperty(")) || !leg.contains('_'),
                "{name} 的腿名 {leg} 带下划线，绑错一个字母就静静失效"
            );
        }
    }

    /// §7.1「历史记录加载应支持分页或虚拟列表」里**分页那一支**的接线，四段一段不能少：页上
    /// 叫了那三条腿 ⇒ 桥声明了它们 ⇒ 桥递进的是 mask 那一头的产品腿 ⇒ 那条腿叫的是 settings
    /// 那一头的分岔本体 ⇒ 本体叫的是 core 的 `older_than`。
    ///
    /// 少任何一段，屏上那条清单就停在第一页上，而页上不会说一句「还有更旧的没显示」—— 库里
    /// 第 201 行往后谁也点不到，这一族的形状与「钮上写着保存、递进产品的却是贴图」是同一个，
    /// 只是它不报错（#261：没有证人不给判决；#433：不许静默）。
    ///
    /// 与 `the_page_binds_the_leg_the_roster_names` 同一把刀：扫的是**剥过注释**的页码，
    /// 注释里提一句不算接上。
    #[test]
    fn the_older_page_is_wired_from_the_page_to_core() {
        let code = crate::hint_check::qml_code(PAGE);
        for leg in [
            "historyWindowPageSize",
            "historyOlderRows",
            "historyWindowListSentence",
        ] {
            assert!(code.contains(&format!("session.{leg}(")), "页上没叫 {leg}");
            assert!(
                BRIDGE.contains(&format!("#[cxx_name = \"{leg}\"]")),
                "桥上没有 {leg} 的声明：页上那一记滚动递到的是空气"
            );
        }
        // 页宽从桥来 ⇒ 页上不该再有一处写着 200 那一枚数（抄的那一份只会先旧）。
        assert!(
            !code.contains("historyRows(200)"),
            "页上还在抄一个 200：页宽的第二位作者又长出来了"
        );
        assert!(code.contains("win.fetchMore()"), "尾巴上那一记滚动没有把手");
        assert!(
            code.contains("onAtYEndChanged"),
            "清单滚到底不叫任何东西 ⇒ 分页那一支在屏上仍然是零"
        );
        assert!(
            MASK_SRC.contains("history_rows_older_at("),
            "mask 那条产品腿没叫分岔本体"
        );
        assert!(
            SETTINGS_SRC.contains(".older_than("),
            "分岔本体没叫 core 的 `older_than` ⇒ 那一枚在库里翻页的手仍然是零产品读者"
        );
        assert!(
            MASK_SRC.contains("fn list_history_older("),
            "产品腿本身不在了：那一页由谁读回来？"
        );
    }

    /// 拒绝的字样从**写它的那一档**读回来：[`REFUSALS`] 里每一串都必须能在 mask.rs 或 session.rs
    /// 的源码里找到原样。哪天负责那一句的档改了字样，这一格红 —— 而不是窗上把一次拒绝报成成功
    /// （与 `the_cancelled_mark_is_read_from_the_row_that_writes_it` 同一枚依赖方向）。
    #[test]
    fn the_refusal_words_are_read_from_the_files_that_write_them() {
        let mut hit = 0usize;
        for word in REFUSALS {
            let here = MASK_SRC.contains(word) || BRIDGE.contains(word);
            assert!(
                here,
                "「{word}」在本档读得回来的那两档里找不到原话：要么字样改了，要么这一串是本档自己编的"
            );
            hit += usize::from(here);
        }
        assert_eq!(hit, REFUSALS.len(), "扫描一枚也没读到 ⇒ 尺子是空的");
    }

    /// 「拒绝」与「成了」在两把尺子上各问一遍。三臂：一次成功、一次剪贴板被占、一次图不在了
    /// （§5.14.4 的自动清理或被人删掉的原图走的就是第三臂）—— 后两臂必须是拒绝形。
    #[test]
    fn a_refusal_is_never_read_as_a_success() {
        assert!(
            !refused("把历史第 7 行的图递回剪贴板：64×48。"),
            "成功句被读成了拒绝"
        );
        assert!(!refused("回放了历史第 7 行：已保存到 shot-001.png。"));
        assert!(refused("内容没递：这一场没有历史可读。"));
        assert!(refused("内容没递：C:\\x 的库里没有第 7 行。"));
        assert!(refused("回放没跑：打不开 history.db（no such table）"));
        // 图取不回来那一臂（`keep_originals` 开着而原图被删 ⇒ `History::frame` 两路都空）。
        assert!(refused(
            "内容没递：第 7 行的图取不回来（history row 7 has no picture left），库里那一行还在。"
        ));
        // 剪贴板被占那一臂的话里有「剪贴板」，门里因此记 Blocked 而不是 FAIL（环境的答案）。
        let occupied = "剪贴板被别的程序占着——第 7 行的图没递出去，剪贴板上此刻那份东西没动。";
        assert!(refused(occupied), "{occupied}");
        assert!(names_row(7, occupied));
        assert!(
            !names_row(7, "删了历史第 17 行，收回 123 字节。"),
            "第 17 行不许被读成第 7 行"
        );
        assert!(names_row(7, "回放了历史第 7 行：已保存到 a.png。"));
    }

    /// 那一枚行数与那些字节都从产品自己拼出来的那一行里读；读不懂 ⇒ `None`，与
    /// [`image_bytes_of`] 同一条口径（把读不懂说成 0，就是让「回放长出一行」有机会绿）。
    #[test]
    fn the_record_count_is_read_from_the_products_own_line() {
        let said = crate::settings::history_usage_line(3, 1_000, 200, 4_096);
        assert_eq!(record_count_of(&said), Some(3));
        assert_eq!(
            record_count_of("history records=0 original=0B thumb=0B db=0B total=0B (开关关着)"),
            Some(0)
        );
        assert_eq!(record_count_of("打不开 history.db"), None);
        assert_eq!(record_count_of("history records=?B"), None);
    }

    /// 「那一行还在不在」判的是**分节后的第一段整等**那一枚行号，不是 `contains`：清单里
    /// 一行写着 `1700×700`，`contains("7")` 那一版会把「第 7 行还在」蒙对一次根本没在的。
    #[test]
    fn a_row_is_found_by_its_number_not_by_a_digit() {
        let rows: Vec<String> = crate::settings::history_rows_joined(
            &[row(7, "主屏", false, false), row(17, "主屏", false, false)],
            -480,
        )
        .split('|')
        .map(str::to_string)
        .collect();
        assert_eq!(rows.len(), 2, "夹具没拼出两行：{rows:?}");
        assert!(row_present(&rows, 7));
        assert!(row_present(&rows, 17));
        assert!(!row_present(&rows, 8), "没有的那一枚不该被算成在场");
        assert!(!row_present(&[], 7), "空清单里不该有任何一行在场");
        assert!(!row_present(&["没有行号的散文".to_string()], 7));
    }

    /// 门里那三条回放腿点名的那一枚，与页上 `win.plan[1]` 用的是同一个数：计划第 2 段。
    /// 跑完把门钉回没 armed 的样子（与 `the_plan_names_the_five_rows_it_planted` 同一笔收尾账）。
    #[test]
    fn the_gauge_and_the_page_point_at_the_same_row() {
        let _guard = plan_lock();
        let said = arm();
        assert!(window_wanted(), "{said}");
        let target = replay_id();
        assert!(target > 0, "计划第 2 段读不成行号：{said}");
        assert_eq!(
            target,
            seg(&plan_line().split('|').collect::<Vec<_>>(), 1),
            "回放腿与页上点名的不是同一枚行号"
        );
        // 那一枚此刻真的在场（清单从产品那一条读腿里取，本档不开第二座库）。
        let rows = rows_now();
        assert!(
            row_present(&rows, target),
            "种下的第 {target} 行不在清单上：{rows:?}"
        );
        // 不留原图的那一臂：本档把 `keep_originals` 钉成 true，所以那一行的字节里带着原图。
        // 这一格只核对读得回来的占用那一行有 records= 与 original= 两枚可读的数（回放判据靠它）。
        let usage = usage_line();
        assert_eq!(record_count_of(&usage), Some(ROWS as i64), "{usage}");
        assert!(image_bytes_of(&usage).is_some(), "{usage}");
        with(|g| {
            g.armed = false;
            g.plan.clear();
            g.lines.clear();
        });
    }

    /// `keep_originals = false` 的那一行手里只有缩略图，而走的是**同一批腿**：名单上不为它
    /// 另开一枚钮、产品那句按**取回来的那张图的尺寸**说话（`{}×{}` 出自
    /// [`crate::mask::MaskState::copy_history_content`] 的 `format!`），页上不替它把
    /// 128 那一档报成 800。这一格钉的是这一条**不是**拒绝句，且点名了那一行。
    #[test]
    fn a_thumbnail_only_row_walks_the_same_legs_and_says_its_own_size() {
        let thumb_sized = "把历史第 9 行的图递回剪贴板：128×64。";
        assert!(!refused(thumb_sized));
        assert!(names_row(9, thumb_sized));
        assert!(
            !ACTIONS.iter().any(|n| n.contains("缩略图")),
            "缩略图那一臂不该长成第六枚钮"
        );
        // 铺回屏上时按那一行的矩形重画 —— 那是 core 的 `stamp_row` 与它的两条单测在管，
        // 本档不复述那条算术，只记下：剪贴板/落盘这两条腿交回的是**取回来的那一张**，不放大。
        let saved = "回放了历史第 9 行：已保存到 shot-009.png。";
        assert!(!refused(saved) && names_row(9, saved));
    }
}
