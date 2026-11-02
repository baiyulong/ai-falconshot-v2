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
    let _ = std::fs::remove_dir_all(&hdir);
    let _ = std::fs::create_dir_all(&hdir);
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

/// 这一档的报告：九格判定 + 沙盒落点 + 窗递回的每一行原样附在后面。
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
        let ok = visible == "1" && btns == BUTTONS.len() && titled == "1" && listed == rows_at_arm;
        rep.row(
            "history window",
            if ok { Check::Pass } else { Check::Fail },
            format!(
                "visible={visible} 钮={btns}/{} 标题={titled} 清单 {listed} 行（开工 {rows_at_arm} 行）",
                BUTTONS.len()
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

    // ⑨ §5.18.1 第 1438 行那枚「历史记录」：判的是**装配出来的那份菜单**里有它，而不是
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
}
