//! Configuration (PRD §5.20): the eleven settings categories, an atomic save
//! that keeps the user's comments, and recovery from a damaged file (§5.1.1).
//!
//! Two representations coexist on purpose:
//! * the typed structs below, which every product module reads;
//! * a dotted-path view (`get_str`/`set_str`) over the same data, which is what
//!   the QML settings page binds to — one code path for a hundred widgets
//!   instead of a property per setting.

use crate::annotation::{Kind, EFFECT_MAX, EFFECT_MIN, FONT_MAX, FONT_MIN};
use crate::encode::Format;
use crate::geometry::Unit;
use crate::naming::Collision;
use crate::pin::{GroupFate, GroupId, PinId, PinSet};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(String),
    #[error("bad toml: {0}")]
    Parse(String),
    #[error("no such setting: {0}")]
    UnknownKey(String),
    #[error("{key}: {reason}")]
    InvalidValue { key: String, reason: String },
}

/// §5.1.2 那一枚开关在档里的名字。**它是机器那一侧的键**：`qt_bridge` 改这一行时要同时拧
/// `HKCU\...\Run` 里那条启动项（见 `platform_windows::autostart`），只写档不动机器就是
/// 「配置说开了而注册表里没有」。写成一名常量而不是让两处各抄一遍串：页上的写手、启动的
/// 回填、下面那份名单，问的是同一枚键。
pub const AUTOSTART_KEY: &str = "general.launch_at_startup";

/// 全仓**唯一**一份「哪些键改了要重启进程」的名单（§5.20.2「需要重启的设置应明确标记」）。
///
/// 每一枚都有一条 [`RESTART_EVIDENCE`] 作证，`restart_audit()` 逐枚核 —— 名单与对照表对不上就是
/// 红，所以这一列不是抄来的：加一枚进来得先写出读者在哪、拨下去读得回什么。
///
/// `general.show_tray` 自本轮起在名单上，这一枚是**补标**而不是新决定：它的读者只有
/// `qt_bridge::hotkey::install` 那一手（`tray_from(general.show_tray, …)` → `Pump::start`），
/// 而 install 全仓只有 `main.rs` 里调用一次 —— 页上拨下去，托盘既不消失也不出现，直到下一趟进程。
/// 「既不即时、又没标记」的那一枚就是它（§5.20.2 第一、二条同时不成立）。
pub const RESTART_KEYS: &[&str] = &[
    "general.language",
    AUTOSTART_KEY,
    "general.show_tray",
    "advanced.data_dir",
    "advanced.cli_enabled",
];

/// 名单里那些**一整族**的键，一枚前缀盖住它底下所有行。
///
/// 为什么不把每一枚动作键逐个列进 [`RESTART_KEYS`]：`hotkey.actions` 是用户自己配出来的一张
/// `BTreeMap`，加一枚动作就多一枚键，逐枚抄的那份名单只会先旧（[`ACTION_TEXTS`] 头顶记过同一枚坏）。
/// §5.20.2 第二条要标的单位本来就是「页上那一行」，而页上就是一枚动作一行。
///
/// 这一族的证据与 `show_tray` 同一枚：读者只有 `hotkey::install` 里那一次
/// `settings::with(|s| s.config.hotkey.actions.clone())`，注册出去的键序活在
/// `Pump::start` 的那一份请求表里，全仓没有第二处重注册 —— 页上把 `capture` 改成 `f13`，
/// 按 `ctrl+shift+a` 照样截一张，而按 `f13` 什么都不来，直到下一趟进程。
pub const RESTART_PREFIXES: &[&str] = &["hotkey.actions."];

/// §5.20.2's 「需要重启的设置应明确标记」 answered for one key. The list above is the
/// whole of the judgement, and this is the only function that reads it: the import
/// preview and the settings page have to mark the same key the same way, and a second
/// copy of the list could only ever disagree with the first.
pub fn needs_restart(key: &str) -> bool {
    RESTART_KEYS.contains(&key) || RESTART_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// 名单在**这一份档**上盖住的每一枚键，按档数出来：[`RESTART_KEYS`] 里这份档有的那几枚，
/// 加上 [`RESTART_PREFIXES`] 那一族底下的每一枚。
///
/// 为什么要有这一手：页上标的枚数与量规那一格问的是「标了几枚」，而名单里有一枚是**一整族**。
/// 只比 `RESTART_KEYS.len()` 的那一处会把这一族数成零枚，于是页上每多一枚用户自己配的动作键，
/// 那一格就多一枚「多标了」—— 而它一枚也没标错。前缀展开只写这一遍，页侧与门侧再各数一次
/// 就是第二份实现（#281，与本模块头部那一句同一枚理由）。
pub fn restart_marks(cfg: &Config) -> Vec<String> {
    cfg.flat()
        .map(|flat| flat.keys().filter(|k| needs_restart(k)).cloned().collect())
        .unwrap_or_default()
}

/// 一枚键「什么时候才真的动」的三种答案 —— §5.20.2 第一、二条落到一枚键上的那一句判断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyTiming {
    /// 每一趟现读：拨完立刻读得回它的效果。这一档**不许**进名单。
    Instant,
    /// 只有进程起来那一次有人读：不重启就不动 ⇒ **必须**标记。
    Startup,
    /// 拧的是机器自己（注册表那一类）：按下去这一趟就改了机器，而机器的效果在下一次登录。
    /// 允许标记，那句理由由 [`RestartEvidence::probe`] 说，不由名单外的一句注释说。
    Machine,
}

/// 对照表的一行：一枚键，它的读者在哪，以及「拨一下之后读什么能看见它动了」。
///
/// 这一族存在的理由就是本轮要回答的那一问：散文式的「这枚大概要重启吧」判不了，而
/// `reader` 与 `probe` 两栏是可查的（grep 得出读者、门里读得回探针），[`restart_audit`] 拿它
/// 逐行核 [`RESTART_KEYS`]，两处对不上就红。
#[derive(Clone, Copy, Debug)]
pub struct RestartEvidence {
    pub key: &'static str,
    pub timing: ApplyTiming,
    /// 读者在哪一只手（`module::fn` 的形状）。`None` = 桥里 grep 不到任何调用点：
    /// **拨了什么都不动，重启也不动**。这一档由 [`restart_unread`] 单独列出来 —— 它既不是
    /// 「无需重启、即时生效」，一句「需重启」也救不了它，§5.20.2 那两句对它两句都不成立。
    pub reader: Option<&'static str>,
    /// 「拨这一枚 → 立刻读回」那一句可判的话。
    pub probe: &'static str,
}

/// §5.20.2 那两句的对照表。表内的键只有两类：**名单上的那些**，和**本轮查出该进名单的那些**。
/// 表外的一切键，按第一句读作「无需重启、即时生效」—— 那一读法有证人：设置页每一行写值走的都是
/// `write` → `Config::set_str` → 原子落盘，而各产品那一族的读者（`pin_cfg` / `capture_cfg` /
/// `hot_corner_cfg` 每一秒 / `tray_cfg` 每一次点击 / `history_open_in` 每一趟）都是现问 config，
/// 不是进程起来那一次读。
pub const RESTART_EVIDENCE: &[RestartEvidence] = &[
    RestartEvidence {
        key: "general.language",
        timing: ApplyTiming::Startup,
        reader: None,
        probe: "界面那一句中文：今天读不到 —— i18n 还没落地，这一枚键在桥里没有任何调用点",
    },
    RestartEvidence {
        key: AUTOSTART_KEY,
        timing: ApplyTiming::Machine,
        reader: Some("qt_bridge::settings_view::write → platform_windows::autostart::sync"),
        probe: "HKCU\\...\\Run 里那一条启动项：按下去当场在/不在；下一次登录才是它生效的时候",
    },
    RestartEvidence {
        key: "general.show_tray",
        timing: ApplyTiming::Startup,
        reader: Some(
            "qt_bridge::hotkey::install → tray_from → Pump::start（全仓只在 main.rs 调一次）",
        ),
        probe: "系统托盘里那个图标：拨下去它不动，下一趟进程才跟着 `Some(TraySpec)` / `None` 分岔",
    },
    RestartEvidence {
        key: "hotkey.actions.",
        timing: ApplyTiming::Startup,
        reader: Some("qt_bridge::hotkey::install（读一次 actions，交给 Pump::start 的请求表）"),
        probe: "按下刚改的那串键：本进程里注册的仍是旧的那一枚，下一趟进程才换",
    },
    RestartEvidence {
        key: "advanced.data_dir",
        timing: ApplyTiming::Startup,
        reader: Some(
            "部分现读：config::data_dir 由 history_data_dir / 指南 / 区域档每一趟问；\
                      已经开着的 history.db 句柄与本场会话的贴图落点不换",
        ),
        probe: "新写的一张图落在哪：现读 ⇒ 当场跟着走；已开着的库句柄与本场会话的落点要下一趟",
    },
    RestartEvidence {
        key: "advanced.cli_enabled",
        timing: ApplyTiming::Startup,
        reader: Some("qt_bridge::cli_gate → settings::advanced_cfg().cli_enabled（main.rs，一趟 argv 判成 Run 之后现读一次）"),
        probe: "命令行认不认参数：下一趟进程带参数进来才问得到（页上那枚开关按下去不改本进程）；\
                关着的下一趟交回的是一句点名 `cli_enabled` 的拒绝并退 1，而不是静默退 0",
    },
];

/// 一枚键在对照表里的那一行；前缀那一族由 `hotkey.actions.` 一行作证，所以这里也按前缀问。
pub fn restart_evidence(key: &str) -> Option<&'static RestartEvidence> {
    RESTART_EVIDENCE
        .iter()
        .find(|e| e.key == key || (e.key.ends_with('.') && key.starts_with(e.key)))
}

/// 对照表与名单对不上的那些话。空 = §5.20.2 第二句在这一版里判得出「一致」。
///
/// 四条判据，每条都是能grep能读回的，不是形容词：
/// ① 表里说只有进程起来才动（`Startup`）而名单不标 ⇒ **漏标**；
/// ② 表里说每一趟现读（`Instant`）而名单标了 ⇒ **标错**（页上多一处没人要的「重启吧」）；
/// ③ 名单里的键在表上没有一行 ⇒ 标了而没有证人；
/// ④ 名单里的键在 `Config` 的键清单里根本没有 ⇒ 名单指着一枚已经不存在的键。
pub fn restart_audit() -> Vec<String> {
    let mut out = Vec::new();
    for e in RESTART_EVIDENCE {
        // 一枚键在名单上有没有影子。前缀那一族（`key` 结尾带 `.`）问的是 `RESTART_PREFIXES`
        // 里有没有同一枚前缀，而不是拿它去 `contains` 一枚带点的路径。
        let listed = if e.key.ends_with('.') {
            RESTART_PREFIXES.contains(&e.key)
        } else {
            RESTART_KEYS.contains(&e.key)
        };
        match (e.timing, listed) {
            (ApplyTiming::Startup, false) => out.push(format!(
                "漏标：{} 只有进程起来那一次有人读（{}），名单却不标它",
                e.key,
                e.reader.unwrap_or("没有读者")
            )),
            (ApplyTiming::Instant, true) => {
                out.push(format!("标错：{} 每一趟现读，名单却把它标成需重启", e.key))
            }
            _ => {}
        }
    }
    for key in RESTART_KEYS.iter().chain(RESTART_PREFIXES.iter()) {
        if restart_evidence(key).is_none() {
            out.push(format!("名单上 {key} 没有对照表那一行：标了而没有证人"));
        }
    }
    let known = Config::default().flat().unwrap_or_default();
    for key in RESTART_KEYS {
        if !known.contains_key(*key) {
            out.push(format!(
                "名单上 {key} 在键清单里不存在：它指的是一枚已经搬走或改名的键"
            ));
        }
    }
    out
}

/// 表上「压根没有读者」的那些键：拨下去什么都不动，重启也不动。
///
/// 这一列**不进** [`restart_audit`] 的红：一句「这枚键没人读」修的不是名单，而是那一族还没落地的
/// 功能（`language` 等），把它算成名单的错只会让门红在一件与本轮无关的事上。它在这里被数出来，
/// 是因为 §5.20.2 那两句对这一族键两句都不成立 —— 「即时生效」不成立，「标了需重启所以别指望它」
/// 也不成立，而用户需要的是有人去接那一只手。
pub fn restart_unread() -> Vec<&'static str> {
    RESTART_EVIDENCE
        .iter()
        .filter(|e| e.reader.is_none())
        .map(|e| e.key)
        .collect()
}

/// §5.16.1 bindable action ids. Hot corners and per-app exclusions name these.
///
/// **末尾那一枚不在 §5.16.1 那一张「可绑定范围」的表里**，它是 §5.9.13 第三步点名的那一枚
/// 「预设快捷键」：那一节自己写了「用户通过预设快捷键、托盘菜单或热区取消穿透」，而规则第一句
/// 是「必须提供明确且可靠的退出穿透方式」。一张*建议*名单抵不过一节*操作*步骤，所以这里发的
/// 是文档欠的那一只手而不是替那张表添一行；两处不一致记进台账，不当作已解决（与
/// `qt_bridge::hotkey::Work::Whiteboard` 在 §5.18.1 那一列上记的是同一枚冲突、同一枚办法）。
/// 它也只写「解除」：开启仍是 §5.9.13 第一步那一句「用户对贴图启用」，走贴图自己的菜单。
pub const ACTIONS: &[&str] = &[
    "capture",
    "capture_custom",
    "capture_repeat",
    "capture_active_window",
    "paste_pin",
    "toggle_pins",
    "show_all_pins",
    "hide_all_pins",
    "switch_group",
    "solo",
    "pin_edit",
    "custom_task",
    "release_click_through",
];

/// §5.21.2 第二枚「快捷键设置页应提供对应功能说明」的那一份名单：一枚动作名、它的中文名、
/// 这一枚会做什么。**名单外的名字一处也不许再有一份中文**，因为「同一个动作在两个地方各叫
/// 一个名」在这一档里必须是量不出来的，而它只能靠「只有一本名册」成立 —— 下面这五枚读者是
/// 逐个 grep 现读来的，不是从这段话说出来的：
/// * 设置页那一行 → [`crate::config::action_texts`]（`qt_bridge::settings_view::action_of`）；
/// * 托盘菜单里那一项、与按下之后运行日志里那一句排队 → `qt_bridge::hotkey::Work::label`
///   （它只经 `name_of` 问这张表）；
/// * `--hotkeys` 那份报告的归宿列 → 同一个 `Work::label`（`hotkey::report` 里那一格）；
/// * 配置那一层证词里的 `tray labels` 一行 → [`Tray::label`]。
///
/// 只有 `白板` 与 `退出` 那两枚留在 `hotkey::Work::label` 里 —— 它们既没有可绑定动作名、
/// 也没有一句要递给设置页的说明。
///
/// **历史里那一格不是这里的读者**，这一句要单独写着：`settings::history_rows_joined` 印的是
/// `rec.source`（这一次来自热键、角落还是托盘），与一枚动作叫什么无关。草稿里曾把「历史里的
/// 那一格」列进上面那串读者，而那一列没有经一次现读 —— 它和本轮一并修掉的 [`Tray::label`]
/// 私字典是同一枚坏的两面：一面是替别人家认领一枚读者，另一面是在别处再养一本名册。
///
/// **前十三枚与 [`ACTIONS`] 同序同 id**，这一条不是靠这段话保证的：`tests` 里那一格逐枚比对
/// 那十三个 id，漏一枚、重一枚、换了序都红。
///
/// 末尾三枚（`menu` / `settings` / `none`）不在 [`ACTIONS`] 那十三名里，它们是 §5.18.2 那四枚
/// 托盘键独有的写法。把它们并进来而不是另起一张表，是因为页上问的是同一件事——「这一格写的
/// 这个名字，是什么意思」；而它们**不能**走 [`crate::hotkey::Work::of`] 那条通路（`menu` 与
/// `none` 压根没有 `Work`，`settings` 也不是可绑定动作，见 `hotkey::tray_run_of` 自己的说明：
/// 把 `settings` 塞进那条路会读出一张假的「未接线」）。归宿问哪张表由设置页按行的形状分头问，
/// 名字与说明在这里只有一份。
///
/// 每一句说明写的都是 `crate::hotkey::run` 里那一臂**此刻真做得动**的事，没有一句写「会弹出
/// 某个对话框」而对面其实没有那扇窗 —— `capture_custom` 是这条规矩的一条正例：P166 之前它只有
/// 名字没有那扇窗，所以它的说明写的是*动作本来是什么*而不是「打开对话框」；P166 把 §5.3.5 那扇
/// 「自定义截图」窗落成了码，这一句「按填好的坐标与尺寸截一块区域，不经过遮罩」如今是**眼前做得
/// 到的事**的读数，改它反而要让说明与窗各说一套。
/// 做不动的那两枚（`pin_edit` / `custom_task`）曾经只说这一枚动作*本来*是什么；自本轮起它们
/// 各有一只手（`qt_bridge::hotkey::Work::PinEdit` 把洞交回选中的那几枚占的那一块、
/// `Work::CustomTask` 判归 [`crate::tasks`] 而跑归 `platform_windows::open::dispatch`），这两句
/// 因此读的是**眼前做得到的事**。接没接线仍然不由这一格说：那两个字由设置页现问归宿表、现拼
/// 那两个字——把「未接线」写死进这一格，接上的那天它就成了假话，而那正是本模块头部记过的
/// 同一枚坏（名单不会错，它只会先旧）。
pub const ACTION_TEXTS: [(&str, &str, &str); 16] = [
    ("capture", "截图", "冻住此刻的屏幕并罩上遮罩，拖一块就截一块"),
    ("capture_custom", "自定义截图", "按填好的坐标与尺寸截一块区域，不经过遮罩"),
    ("capture_repeat", "重复上次截图", "把上一次框过的那块区域原样再截一次，贴成一张图"),
    ("capture_active_window", "活动窗口截图", "截取此刻接手键盘的那一扇窗，贴成一张图"),
    ("paste_pin", "贴出剪贴板", "把剪贴板里的图贴到屏幕上，剪贴板没有可贴的东西就拒"),
    ("toggle_pins", "隐藏/显示全部贴图", "把屏幕上全部贴图收起来，再按一次放开"),
    ("show_all_pins", "显示全部贴图", "放开全部贴图（只置成显示，不跟着上一次的态翻）"),
    ("hide_all_pins", "隐藏全部贴图", "收起全部贴图（只置成隐藏，不跟着上一次的态翻）"),
    ("switch_group", "切换分组", "在贴图分组之间轮流切换显示哪一组"),
    (
        "solo",
        "独显",
        "只留下此刻选中的那几张贴图，其余的按 pin.solo_dim_opacity 压暗（为 0 则隐藏）；再按一次全部还原；一枚也没选中时直接拒绝",
    ),
    ("pin_edit", "贴图编辑", "让选中的贴图进入标注编辑"),
    ("custom_task", "自定义命令", "跑一条用户自己写好的命令行任务"),
    (
        "release_click_through",
        "解除鼠标穿透",
        "把此刻正在穿透鼠标的那几张贴图全部收回到点得动，一枚也没有时照实说没有；开启仍走贴图自己的菜单",
    ),
    ("menu", "托盘菜单", "点开托盘菜单"),
    ("settings", "首选项", "打开设置窗"),
    ("none", "不绑定", "这一格什么都不做"),
];

/// 一枚动作名的中文名与功能说明。名单外的名字交回 `None` = 这一行没有可递的说明。
///
/// 这一枚函数是那张表的**唯一门面**：设置页与托盘都从这里读，两处各抄一份中文迟早不一样。
pub fn action_texts(id: &str) -> Option<(&'static str, &'static str)> {
    ACTION_TEXTS
        .iter()
        .find(|(a, _, _)| *a == id)
        .map(|(_, name, desc)| (*name, *desc))
}

/// §5.18.2 那四枚键各自能填的动作。
///
/// 名单不是这里发明的：`menu` 与 `settings` 是 §5.18.2 两条规则点名"必须留得住"的那两处入口，
/// 其余全部取自 §5.18.1 那张菜单建议表里本进程**已经有一只手接得住**的事。表里剩下的
/// 「自定义截图」「图片分组」「历史记录」「帮助」「检查更新」「退出」不在这里出现 —— 它们
/// 要么没有对应的 `Work` 臂，要么是 §5.18.1 让菜单去做的多步流程；把它们列进来就再造一次
/// "配得上却什么都不做"那一族（P140 在热区那一头刚刚把它关掉，见
/// `qt_bridge::hot_corner::check` 的逐名判据）。
///
/// 末尾那一枚 `release_click_through` 是个**例外，不是新的一行**：§5.18.1 那一列压根没有
/// 「解除鼠标穿透」，而 §5.9.13 第三步明写「用户通过预设快捷键、**托盘菜单**或热区取消穿透」，
/// 规则第一句还要这条退出**明确且可靠**。两处不一致时按功能那一节办（与
/// `qt_bridge::hotkey::Work::Whiteboard` 在 §5.4.4 上记的是同一枚冲突、同一枚办法），冲突记进
/// 台账、不当作已解决。列进这里而不只是列进托盘菜单，是因为 §5.18.2 那四枚鼠标键是**另一只
/// 不穿过贴图的手**：右键开菜单当然也能解除，而把这一枚配到中键上的人不该被菜单那一层拦着。
pub const TRAY_ACTIONS: &[&str] = &[
    "menu",
    "settings",
    "capture",
    "paste_pin",
    "toggle_pins",
    "show_all_pins",
    "hide_all_pins",
    "release_click_through",
    "none",
];

/// 能把人带到托盘菜单或设置页的那两枚动作 —— §5.18.2 的两条规则问的就是这一枚名单。
///
/// 文档把那件事说了两遍（"至少保留一种可打开托盘菜单或设置页的入口"与"不允许所有入口都配置为
/// 无法进入设置的动作"），但它是**同一枚判据的两面**，所以这里只有一份名单、一处判断。
pub const TRAY_ENTRY_ACTIONS: &[&str] = &["menu", "settings"];

/// 四枚键的默认动作。顺序就是 [`Tray::entries`] 的顺序；`Default` 与"改回本键默认"读同一份，
/// 因为分成两处写迟早会不一样。
const TRAY_DEFAULTS: [(&str, &str); 4] = [
    ("left", "capture"),
    ("double", "capture"),
    ("middle", "none"),
    ("right", "menu"),
];

/// The five actions PRD §5.15 lets a hot corner run. It is a subset of
/// [`ACTIONS`]: a corner that starts a capture is fine, a corner that opens a
/// custom command is a surprise the user did not ask for.
pub const HOT_CORNER_ACTIONS: &[&str] = &[
    "show_all_pins",
    "hide_all_pins",
    "toggle_pins",
    "capture",
    "solo",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// §5.19.1 跟随系统主题: `System` is a question to the machine, not a third look.
///
/// The expansion lives here rather than at the call site because the machine's answer
/// can only be read at runtime (one registry value, `apps_use_light_theme` on the
/// platform side) while the three-way fork has to be gradable in `cargo test` - the
/// same split as the glyph rasterizer's: core decides, the platform reads.
///
/// `system_light` is that answer: `Some(true)` = the machine is on light, `Some(false)` =
/// dark, `None` = it did not say (value absent, key absent, read failed). `None` lands on
/// `Light` because light is what Windows itself falls back to when that value is missing:
/// agreeing with the machine's own default is checkable by anyone, guessing dark would be
/// wrong in a direction nothing can re-check.
///
/// §5.19.2 手动主题 wins over the machine by construction - `Light` and `Dark` ignore the
/// answer entirely. No arm returns `System`, which is the point: everything downstream of
/// this function has a drawable result.
pub const fn resolve_theme(requested: Theme, system_light: Option<bool>) -> Theme {
    match requested {
        Theme::Light => Theme::Light,
        Theme::Dark => Theme::Dark,
        Theme::System => match system_light {
            Some(true) => Theme::Light,
            Some(false) => Theme::Dark,
            None => Theme::Light,
        },
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// Full-screen mask with free selection (§5.3.1).
    #[default]
    Region,
    Window,
    /// Element under the cursor (§5.3.3).
    Element,
    Fullscreen,
    /// Re-shoot the previous rect (§5.2.3).
    Repeat,
}

/// §5.9.15: how a pin treats transparent pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaBg {
    /// Not drawn, not clickable.
    #[default]
    Transparent,
    /// Looks transparent, whole window clickable.
    Pseudo,
    CheckerDark,
    CheckerLight,
}

/// §5.9.15's four modes as the text that stores them. The settings widget spells
/// a mode, the menu labels one, and the file carries one — so the id domain
/// lives beside the enum rather than inside whichever widget asked last.
pub const ALPHA_BG_IDS: &[&str] = &["transparent", "pseudo", "checker_dark", "checker_light"];

impl AlphaBg {
    /// The text this mode is stored as. Written next to the enum so a
    /// `snake_case` rename has to be made here too, where
    /// [`ALPHA_BG_IDS`]'s own test can catch it.
    pub fn id(self) -> &'static str {
        match self {
            AlphaBg::Transparent => "transparent",
            AlphaBg::Pseudo => "pseudo",
            AlphaBg::CheckerDark => "checker_dark",
            AlphaBg::CheckerLight => "checker_light",
        }
    }

    pub fn from_id(text: &str) -> Option<AlphaBg> {
        ALPHA_BG_IDS
            .iter()
            .position(|id| *id == text)
            .map(|at| match at {
                0 => AlphaBg::Transparent,
                1 => AlphaBg::Pseudo,
                2 => AlphaBg::CheckerDark,
                _ => AlphaBg::CheckerLight,
            })
    }

    /// The mode a menu item carries: a QML list index reaches the enum through
    /// here, so the item order on screen and [`ALPHA_BG_IDS`] are the same order.
    pub fn from_index(index: u32) -> Option<AlphaBg> {
        let at = index as usize;
        if at < ALPHA_BG_IDS.len() {
            Some(AlphaBg::from_id(ALPHA_BG_IDS[at]).expect("ALPHA_BG_IDS maps to itself"))
        } else {
            None
        }
    }

    /// [`Self::from_index`]'s other half, and the number a qproperty carries.
    pub fn index(self) -> u32 {
        match self {
            AlphaBg::Transparent => 0,
            AlphaBg::Pseudo => 1,
            AlphaBg::CheckerDark => 2,
            AlphaBg::CheckerLight => 3,
        }
    }
}

/// §5.2.2: the tool the annotation stage opens with. The config has to reject a
/// typo without knowing how to draw, so the id list lives here.
pub const ANNOTATION_TOOLS: &[&str] = &[
    "rect",
    "ellipse",
    "line",
    "arrow",
    "curve",
    "polyline",
    "polygon",
    "text",
    "number",
    "highlighter",
    "blur",
    "mosaic",
    "pen",
    "eraser",
    "counter",
    "stamp",
    "callout",
    "crop",
    "spotlight",
    "measure",
    "select",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// §5.1.2 开机自启. 这一枚是**机器那一侧**的状态在档里的镜像：写它的人要同时拧
    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 里那条启动项，而启动时读一回
    /// 机器、以机器为准回填它（两边各说一面的那一种漂移在这里没有写法）。见
    /// `platform_windows::autostart` 与 `qt_bridge::settings::startup_autostart_banner`。
    pub launch_at_startup: bool,
    pub start_minimized: bool,
    pub show_tray: bool,
    /// §5.1.3 closing the window keeps the app running.
    pub close_to_tray: bool,
    pub confirm_exit: bool,
    /// `auto`, or a BCP 47 tag such as `zh-CN` / `en`.
    pub language: String,
}

impl Default for General {
    fn default() -> Self {
        Self {
            // PRD §5.1.2 makes autostart a user choice, never a default.
            launch_at_startup: false,
            start_minimized: true,
            show_tray: true,
            close_to_tray: true,
            confirm_exit: false,
            language: "auto".into(),
        }
    }
}

/// §5.2.4 / §5.3.5 第四步那枚「延时时间」的界：`0` 是不延时，上界 300 秒。
///
/// 写成两枚公有的常数而不是只留在 [`Config::normalize`] 那一次 `clamp` 里：`qt_bridge::custom`
/// 那一扇「自定义截图」对话框要在**按下之前**就拒掉一个越界的数（写进档里再被 `clamp` 悄悄改成
/// 300，页上打的与屏幕上数的是两回事），而同一枚界在两处各写一遍数字，迟早一处改了另一处不说。
/// `clamp` 用的就是这两枚，所以「档里的钳位」与「页上的拒绝」问的是同一个数。
pub const DELAY_SECONDS_MIN: u32 = 0;
pub const DELAY_SECONDS_MAX: u32 = 300;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capture {
    pub default_mode: CaptureMode,
    /// §5.5.3 cursor inclusion.
    pub include_cursor: bool,
    /// §5.2.5 freeze the desktop before the mask appears. Off saves the freeze
    /// cost but shows a live screen while selecting.
    pub freeze_frame: bool,
    /// §5.3.3 highlight the window/element under the cursor.
    pub highlight_element: bool,
    /// §5.4.1 magnifier geometry.
    /// §5.4.1 第 4 步「隐藏放大镜」与 §5.19.3 最后一枚「是否显示放大镜」：放大镜那一族
    /// 其余各枚（`magnifier_radius` / `magnifier_zoom` / `magnifier_grid` /
    /// `magnifier_coordinates` / `magnifier_color_format`）都在档里，唯独这一枚此前没有键。
    /// 它是这一族里唯一真正缺的那一格，不是任何既有键的同义，所以补这一枚而不是再造。
    pub magnifier_enabled: bool,
    pub magnifier_radius: u32,
    pub magnifier_zoom: u32,
    pub magnifier_grid: bool,
    pub magnifier_coordinates: bool,
    /// A [`crate::colors::ColorFormat`] name.
    pub magnifier_color_format: String,
    /// §5.3.5 the live size readout.
    pub show_dimensions: bool,
    /// §5.3.7 step 1: which unit the readout opens on. `px` is the physical pixels the result
    /// detail names, `dip` is the same rect divided by the screen the selection lies on.
    pub size_unit: Unit,
    /// §5.2.4 default for the delayed-capture picker; 0 shoots immediately.
    pub delay_seconds: u32,
    /// §5.3.6 snap selection edges to the window under the cursor.
    pub snap_to_window: bool,
    /// §5.2.6 规则第一句「可按设置决定是否包含**窗口阴影**」的那一格。
    ///
    /// 曾经是一枚没有读者的句子（PRD 第 238 行）：窗口截图的框一直是 DWM 可见边，
    /// 而可见边天生不含阴影，所以「包含阴影」从来无处生效。自 [`crate::capture::window_picture`]
    /// 起这一枚有读者：开着 ⇒ 每边外扩一圈并把阴影画进那一圈（宽度见
    /// [`crate::capture::window_shadow_margin`]），关着 ⇒ 就是今天那张图。
    ///
    /// 默认关着有两个理由：一是这一带的其余各枚（[`Output::corner_radius`]、
    /// [`Output::shadow_size`]）出厂都是「不额外加东西」那一侧；二是开着它会挪动
    /// §5.2.5「上一次」记住的那个矩形，而那一格的行为不该由一枚新键悄悄改。
    pub window_shadow: bool,
    /// §5.2.6 规则第一句「可按设置决定是否包含**圆角**」的那一格。
    ///
    /// 开着 ⇒ 窗口截图的四角按 [`Self::window_corner_radius`] 决定多大的圆角切掉；
    /// 关着 ⇒ 方角，就是今天那张图。切掉的那四格里外头是透明还是填色，仍归
    /// [`Output::corner_fill`] 说（§5.5.6 规则第一句「或按配置填充背景色」），
    /// 所以这一枚不新增第二份口径，只决定这条路上切不切。
    pub window_corner: bool,
    /// §5.5.6 步骤 3 的两读法：「设置圆角半径，或使用系统窗口原始圆角」。
    ///
    /// `0` 是第二种读法 —— 去问平台这扇窗自己被你画的窗框切了多大的角
    /// （[`crate::capture::WindowExtras::window_corner_radius`]）；`>0` 是第一种，
    /// 用这个物理像素半径，谁也不问了。分岔本体在
    /// [`crate::capture::resolved_corner_radius`]，它把「系统答不出」读成**不切**而不是猜一个数。
    ///
    /// 与 [`Output::corner_radius`] 是两枚而不是同一枚，因为那一枚说的是**导出图片**的圆角
    /// （选区截完再切，§5.5.6 步骤 2 那句「在截图输出设置中开启圆角」），这一枚说的是
    /// §5.2.6 那一扇窗；两格同名的话，关掉输出圆角会连带关掉窗口圆角，而那是两问。
    pub window_corner_radius: u32,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            default_mode: CaptureMode::Region,
            include_cursor: false,
            freeze_frame: true,
            highlight_element: true,
            magnifier_enabled: true,
            magnifier_radius: 8,
            magnifier_zoom: 8,
            magnifier_grid: true,
            magnifier_coordinates: true,
            magnifier_color_format: "hex_upper".into(),
            show_dimensions: true,
            size_unit: Unit::default(),
            delay_seconds: 0,
            snap_to_window: true,
            // §5.2.6 的三格出厂都是「不额外加东西」那一侧：今天的窗口截图就是这个样子，
            // 新键只给用户一个开关，不给一张不同的图。
            window_shadow: false,
            window_corner: false,
            window_corner_radius: 0,
        }
    }
}

/// One tool's remembered pen (§5.7.1). Every field is optional because a hand-edit
/// may speak about only one of them: `[annotation.tool_style.rect] width = 5` is a
/// user who changed the rect's line width, not one who also reset its colour. A tool
/// with no entry was never taught, so the pen it starts on is `[annotation]`'s own
/// scalars - which is what [`crate::annotation::model::Style`]'s defaults and the
/// defaults below have to agree with, or the row and the pen are two sources of truth.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolStyle {
    /// Anything [`crate::colors::parse`] reads. The layer writes `#RRGGBBAA`, because
    /// the alpha is part of the pen and a six-digit hex cannot carry it.
    pub color: Option<String>,
    /// Line width in physical pixels, the same 1..=64 the pen takes (§5.7.20).
    pub width: Option<u32>,
    /// §5.7.9 像素化块大小 / §5.7.10 模糊强度, in picture pixels. Only 马赛克 and 模糊
    /// have a strength to remember, and the number means a block size in one tool and a
    /// blur radius in the other — so it lives per tool rather than in `[annotation]`'s
    /// `blur_radius`, which is one scalar for two different quantities.
    pub intensity: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Annotation {
    /// Tool id activated on entry (§5.7); empty = keep the last used tool.
    pub default_tool: String,
    pub stroke_width: u32,
    pub stroke_color: String,
    pub fill_color: String,
    pub font_family: String,
    pub font_size: u32,
    /// `straight` | `elbow` | `curve` (§5.7.4).
    pub arrow_style: String,
    /// §5.7.7 mosaic / §5.7.8 blur strength in pixels.
    pub blur_radius: u32,
    /// §5.20.3 the palette travels with exported settings.
    pub palette: Vec<String>,
    /// §5.7.22 step 3 「保存为调色板方案」: one saved band per entry, spelled
    /// `名字=#RRGGBB #RRGGBB …` — one `=` between the name and the colours, one space
    /// between two colours. The shape is chosen by the writer this key goes through:
    /// [`Config::set_str`] parses an array by splitting on `,`, so a comma anywhere in an
    /// entry would silently become a second scheme, and a space is what a colour list never
    /// contains. Empty by default: a band nobody has saved is not a scheme the file invents.
    pub palette_schemes: Vec<String>,
    /// §7.2 undo depth; plan §6.4 fixed 200 steps and a 400 ms merge window.
    pub undo_limit: usize,
    pub undo_merge_ms: u32,
    /// §5.7.1 keep the toolbar visible while hovering the selection.
    pub show_toolbar_on_hover: bool,
    /// §5.7.1 记忆该工具最近一次使用的样式 / §9.2 样式记忆重启有效. One entry per
    /// tool the user has actually taught, keyed by the tool's own spelling
    /// ([`crate::annotation::model::Kind::key`]).
    pub tool_style: BTreeMap<String, ToolStyle>,
}

impl Default for Annotation {
    fn default() -> Self {
        Self {
            default_tool: String::new(),
            stroke_width: 3,
            stroke_color: "#E81123".into(),
            fill_color: "#00000000".into(),
            // The family §5.7.11's 中文 annotations are laid out in, which is
            // [`crate::annotation::model::Style`]'s default as well. The two have to be
            // the same word from the moment this key is read by anything: while nothing
            // read it, `"Segoe UI"` here was a comment about a font the product never
            // asked for, and wiring the key without changing it would have moved every
            // default pen to a family chosen for Latin text.
            font_family: "Microsoft YaHei UI".into(),
            font_size: 18,
            arrow_style: "straight".into(),
            blur_radius: 12,
            palette: default_palette(),
            palette_schemes: Vec::new(),
            undo_limit: 200,
            undo_merge_ms: 400,
            show_toolbar_on_hover: true,
            tool_style: BTreeMap::new(),
        }
    }
}

fn default_palette() -> Vec<String> {
    [
        "#E81123", "#F38B00", "#FFB900", "#7A7A7A", "#FFFFFF", "#000000", "#0078D7", "#16C60C",
        "#B4009E", "#00B7C3",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// The one grammar a saved palette scheme ([`Annotation::palette_schemes`]) is written and read
/// with: `名字=#RRGGBB #RRGGBB …`. `None` is the answer for an entry with no `=`, a name that is
/// only spaces, or a colour list that is only spaces — those are not half a scheme, they are a
/// different key.
///
/// Both directions live here because the page composes entries and the file reader has to agree
/// with it: a second parser would be a second opinion about what the user saved.
pub fn scheme_split(entry: &str) -> Option<(&str, Vec<&str>)> {
    let (name, colors) = entry.split_once('=')?;
    let name = name.trim();
    let colors: Vec<&str> = colors.split_whitespace().collect();
    if name.is_empty() || colors.is_empty() {
        return None;
    }
    Some((name, colors))
}

/// Which names a scheme may carry. `=` is the separator inside an entry, `,` is what
/// [`Config::set_str`] splits an array on, `|` is what the settings window's own rows and the
/// gauge legs split on, and a control character (including `RECORD`) would break every one of
/// those readers. Everything else — Chinese, spaces, digits — is the user's to name.
pub fn scheme_name_ok(name: &str) -> bool {
    !name.is_empty() && !name.contains(['=', ',', '|']) && !name.chars().any(char::is_control)
}

/// Render one scheme. `None` when the name is not one the storage can keep inside one entry, or
/// when a colour carries a separator that would split the entry's colour list.
pub fn scheme_join(name: &str, colors: &[String]) -> Option<String> {
    if !scheme_name_ok(name.trim())
        || colors
            .iter()
            .any(|c| c.contains([' ', ',', '=', '|']) || c.chars().any(char::is_control))
    {
        return None;
    }
    Some(format!("{}={}", name.trim(), colors.join(" ")))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pin {
    /// §5.9.1 default scale percent.
    pub default_zoom: u32,
    /// §5.9.2 percent.
    pub default_opacity: u32,
    pub default_topmost: bool,
    pub default_click_through: bool,
    pub alpha_bg: AlphaBg,
    /// §5.9.6 the mouse wheel changes size.
    pub wheel_resizes: bool,
    /// §5.10.4 clicking a stacked pin cycles instead of raising.
    pub cycle_on_click: bool,
    /// §6.3 crash-recovery snapshot cadence in seconds; 0 disables.
    pub state_save_seconds: u32,
    /// §5.11.1 Solo dims the others instead of hiding them.
    pub solo_dim_opacity: u32,
    /// §5.9.10 固定尺寸缩略图的「预设尺寸」的宽，物理像素。这是那个盒子的横边：
    /// [`crate::pin::PinItem::enter_fixed_thumbnail`] 收到它之后按等比装入，所以这一
    /// 枚与 [`Self::thumb_fixed_height`] 一起只决定盒子，不决定缩略图真正呈现的尺寸——
    /// 后者的算式只有那一个作者。默认取 [`crate::pin::THUMB_FIXED_DEFAULT`]，而不是
    /// 在这里再写一个 240，好让「预设」这个数字只有一个出处。
    pub thumb_fixed_width: u32,
    /// §5.9.10 预设尺寸的高，物理像素。与 [`Self::thumb_fixed_width`] 同上一枚盒子。
    pub thumb_fixed_height: u32,
    /// §5.8.4 方式二第 3 步「更新当前贴图或创建新贴图，具体行为由设置决定」里那一枚设置。
    /// `true` 是把拖进来的那一张换到**这一张**上、`false` 是让落点**新钉一枚**。为什么默认
    /// `false`：那一格的代价不对称 —— 换上去会把这一张原有的来历（历史、文件、缓存）咽下去，
    /// 新钉一枚最多是屏幕上多一格。落点怎么算这句决定，只有 [`crate::drop::Landing::plan`]
    /// 一个作者，页与桥都不再判第二遍（#281）。
    pub drop_replaces_current: bool,
}

impl Default for Pin {
    fn default() -> Self {
        Self {
            default_zoom: 100,
            default_opacity: 100,
            default_topmost: true,
            default_click_through: false,
            alpha_bg: AlphaBg::Transparent,
            wheel_resizes: true,
            cycle_on_click: false,
            state_save_seconds: 5,
            solo_dim_opacity: 35,
            thumb_fixed_width: crate::pin::THUMB_FIXED_DEFAULT.w,
            thumb_fixed_height: crate::pin::THUMB_FIXED_DEFAULT.h,
            drop_replaces_current: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Card {
    /// §5.8.5 step 4 宽度, in physical pixels. The one key that decides where a long
    /// line breaks, so [`crate::annotation::raster::wrap`] is asked with it.
    pub width: u32,
    /// §5.8.5 step 4 字号. Shares its unit and its default with
    /// [`Annotation::font_size`], because a second font-size story is a second number
    /// nothing checks against the first.
    pub font_size: u32,
    /// §5.8.5 step 4 字体 — a system family name, never shipped inside the package
    /// (see [`crate::annotation::raster::Glyphs`]).
    pub font_family: String,
    /// §5.8.5 step 4 背景颜色. The ink is not a key: §5.8.5 asks for four settings and
    /// this is the fourth, and §7.4's contrast promise is answered by deriving the ink
    /// from this value rather than by asking the user for a fifth that can disagree.
    pub background: String,
}

impl Default for Card {
    fn default() -> Self {
        Self {
            width: 480,
            font_size: 18,
            font_family: "Microsoft YaHei UI".into(),
            background: "#FFFFFF".into(),
        }
    }
}

/// §5.12 那一族（组名、成员归属、排序）在档里的样子 —— §5.20.3 建议范围第六枚「分组配置」
/// 唯一的那份载体。
///
/// **它为什么必须存在**：在这一枚结构出现之前，[`ImportScope::Suggested`] 的说明写的是「groups
/// 不住 config.toml，`PinSet::groups` 才是它们的家，这里没有键供它们过档」。那句是实话，而它当时
/// 的对面是「所以 §5.20.3 建议的那一族没进导入导出」。现在这一族有了自己的格子，实话改成
/// 「它们过的就是这一档」。
///
/// **为什么是一枚数组而不是「排序数组 + 成员 map」两格**：[`Config::save`] 走的是
/// [`merge_table`]，而那一手**从不删键**（「档里手写的、schema 不再认的键原样留着」是它头顶
/// 那句话）。map 的形状一旦落进那一手，删掉一组就会在档里留下 `[groups.members].<旧名>`
/// 那一格洗不掉的尸体，下一次读档时它又是一枚「有成员而排序里没有」的孤儿 ⇒ 那一组复活。
/// 一组删不掉，就是「配得上却做不到」的另一件。数组整格换（`*dv = sv.clone()`），删掉的那一枚
/// 是真的没了 —— 往返判据那一枚单测钉的就是这一句。
///
/// 三样各住一处，为的都是让它走**同一条**往返管线（[`Config::export_text`] →
/// [`Config::parse_import`] → [`Config::preview_import`] → [`Config::apply_changes`]）：
/// * 排序 = 数组的次序本身（§5.12.3「拖动排序」的那一串，也是快捷切换菜单的次序）；
/// * 组名与成员归属 = 数组里那一枚字符串，语法 `组名=编号 编号 …`（等号后为空 = 这一组今天
///   没有贴图），见 [`group_split`] / [`group_join`]。
///
/// 这一枚语法不是新造的：它就是把 [`Annotation::palette_schemes`] 那一份「名字=值 值」（
/// [`scheme_split`] / [`scheme_join`]）用在编号上。同一枚理由也在那儿记着 —— 页上写它的手与
/// 档里读它的手必须是同一份判断，否则用户存的与自己读回来的就是两套。而成员之间用**空格**不用
/// 逗号，为的是 [`coerce`] 那一手按逗号拆数组：内层再用逗号，一枚组就被拆成两枚（`palette_schemes`
/// 头顶记的是同一件事）。
///
/// **为什么身份是组名而不是编号**：`PinSet` 那边的 `GroupId` 是本进程发的（`next_group`），换一台
/// 机器同一枚号指的是另一组贴图 —— 把本机才说得通的话写进一份要带去另一台机器的档，就是那句
/// 「配置说开了而机器不是这样」的另一面。§5.12.1「组名不得重复」正是这一族能被命名的前提。
/// 成员那一头的 `PinId` 同理**不**跨机器，所以 [`sync_store_groups`] 只搬这台机器认得的那些枚，
/// 认不得的原样报出去（[`GroupSync::unknown_members`]），一枚也不静默吞掉。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Groups {
    /// 一枚组一行，次序就是机器上的次序，行内带着它的成员。
    pub entries: Vec<String>,
}

/// 这一族最多几组。上界是给档那道闸的，不是给界面的：§5.12.1 没写死几枚，而一份十万组的档
/// 摆到快捷切换那一列与设置页那一路都走不动。
pub const GROUPS_MAX: usize = 64;

/// 一组最多几枚贴图。与 [`GROUPS_MAX`] 同一枚理由，界上那一列成员名也是人要看的东西。
pub const GROUP_MEMBERS_MAX: usize = 512;

/// 一枚组名在**这一份档**里说不说得出口。`None` = 说得出口。
///
/// 拦下的每一样都是档与页自己的形状，不是 §5.12 的规矩（空名与重名归 `PinSet::group_name` /
/// `name_taken` 判，那里不许有第二份实现）：
/// * `,` —— [`coerce`] 那一手按逗号拆数组，名字里带一枚逗号会在拆回来时变成两枚组；
/// * `=` —— 它是 [`group_split`] 那一枚行内的分隔，而页上一行也是 `key=value`，窗从第一个 `=`
///   切开（`SettingsWindow.qml` 头部为这一枚切法记过同一封信）；
/// * `|` —— 页上的行与量规的腿都按它切；
/// * 控制字符（含换行、制表、`RECORD`）—— 一份 TOML 的一行里塞进换行，读回来的不是那一件事，
///   而 `RECORD` 是桥层拿来自家拼接的行分隔。
///
/// 这一套字符闸与 [`scheme_name_ok`] 是同一份口径（那一个族先立过这块牌子）；这里写第二遍
/// 只因为报错要说得出**拦下的是哪一枚字符**，而 bool 答不了这一问。
pub fn group_name_bad(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("组名不能是空的");
    }
    if name.contains(',') {
        return Some("组名里不能有逗号：档里一行就是一枚组，逗号是它的尽头");
    }
    if name.contains('=') {
        return Some("组名里不能有等号：它分开的是组名与成员");
    }
    if name.contains('|') {
        return Some("组名里不能有竖线：页上的行拿它切开");
    }
    if name.chars().any(char::is_control) {
        return Some("组名里不能有控制字符（换行、制表那一类）");
    }
    None
}

/// 档里一行（[`Groups::entries`] 的一枚）拆成组名与成员。`None` = 这一行说的不是枚组
/// （名字是空的、或只有空白），与 [`scheme_split`] 同一枚判断：半个组不是一枚组。
///
/// 认两种写法：`组名=1 2 3` 与光一枚 `组名`（= 空组）。前者是 [`group_join`] 写的，后者是
/// 人手工敲进档里的 —— 拒掉它只会让人以为自己的档没生效。
pub fn group_split(entry: &str) -> Option<(String, Vec<String>)> {
    let (name, members) = match entry.split_once('=') {
        Some((n, m)) => (n, m),
        None => (entry, ""),
    };
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((
        name.to_string(),
        members.split_whitespace().map(String::from).collect(),
    ))
}

/// 一枚组写成档里的一行。`None` = 这一枚组名或这一批编号写不出这样一行（[`group_name_bad`]
/// 与「编号得是数字」两道闸，写之前先过）。
pub fn group_join(name: &str, members: &[String]) -> Option<String> {
    let name = name.trim();
    if group_name_bad(name).is_some() {
        return None;
    }
    if members.iter().any(|m| m.parse::<u64>().is_err()) {
        return None;
    }
    Some(format!("{name}={}", members.join(" ")))
}

/// 这一族对一件事的说法：一枚组名，与它此刻有哪几枚贴图（编号原样字符串）。
///
/// 这一族对一件事的说法：一枚组名，与它此刻有哪几枚贴图（编号原样字符串）。
///
/// 这是档里那一行（[`group_split`]）与机器里那一枚组（[`roster_of`]）合成的同一份**顺序表**，
/// 也是 [`sync_store_groups`] 与 `PinSet` 之间递的那一样东西：两边各自拼成这一份，才谈得上比。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupRoster {
    pub name: String,
    pub members: Vec<String>,
}

/// 一条 [`plan_group_sync`] 的步子：说得出「要做什么」而没有做，所以单测跑得动它，而桥那一侧
/// 只需要按序调用 `PinSet` 已有的公开手（#281：分组那一族的一份规矩不许有第二处作者）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupStep {
    /// 机器里那一枚改名叫 `to`（档里只有大小写不同的那一枚：§5.12.1 的重名规矩是大小写不敏感的，
    /// 所以新建一支会被机器拒掉，改名字才是那一句「档说的是它」的对面）。
    Rename { from: String, to: String },
    /// 档里没有这一组 ⇒ 删掉它。**成员退回未分组**而不是随组销毁：§5.12.3 那一问
    /// 「删除非空分组时应询问贴图如何处理」在这里没有一次可问的界面（导入是一次成批的写，
    /// 而「询问」是产品那一侧的一扇窗），选销毁会带走贴图，选退回一格也不丢。
    Remove { name: String },
    /// 档里有而机器里没有 ⇒ 新建。
    Create { name: String },
    /// 把这几枚贴图搬进这一组（`members` 是**这一组该有的全部**，不是增量：搬进来这一步自带
    /// 「从别组拿走」，所以别组那些枚不需要一条自己的步子）。
    Take { name: String, members: Vec<String> },
    /// 这几枚谁也不要 ⇒ 退回未分组。只有「档里哪一组都不提它」的成员才会走到这一支。
    Ungroup { name: String, members: Vec<String> },
    /// 排序：把这一组挪 `delta` 格（`PinSet::move_group` 自己饱和夹边）。
    Move { name: String, delta: i32 },
}

/// 一次 [`sync_store_groups`] 的账。每一枚都是**这台机器上真的动了**的数，不是档里说了什么。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupSync {
    pub created: usize,
    pub renamed: usize,
    pub removed: usize,
    /// 搬进组的贴图枚数（含「从别组搬来」的那些）。
    pub moved_in: usize,
    /// 退回未分组的贴图枚数。
    pub moved_out: usize,
    pub reordered: usize,
    /// 档里点名了、而这台机器上没有的贴图编号。原样报出来，不静默（§8.3）。
    pub unknown_members: Vec<String>,
    /// `PinSet` 自己拒掉的那几步（重名、越界之类），一句一步，带着原因。
    pub refused: Vec<String>,
}

impl GroupSync {
    /// §5.20.3 步骤 5 在分组这一族上那一行话。
    ///
    /// 它写在 core 是因为这一句有两个读者：命令行 `--import-config --confirm` 的那一段输出，与
    /// 设置页上「导入配置」那一格。两处各拼一遍中文，早晚会有一句在骗另一句（#281）。
    /// 零枚也要说得出：「一项也没动」与「没去动」在日志里必须长得不一样（与
    /// [`crate::config`] 头顶那句 §5.20.3 步骤 4 的同一规矩）。
    pub fn line(&self) -> String {
        let mut parts = vec![format!(
            "分组：新建 {}、改名 {}、删除 {}（成员退回未分组）、搬进 {}、退回 {}、挪序 {}",
            self.created, self.renamed, self.removed, self.moved_in, self.moved_out, self.reordered
        )];
        if !self.unknown_members.is_empty() {
            parts.push(format!(
                "档里点名的 {} 枚贴图这台机器没有：{}",
                self.unknown_members.len(),
                self.unknown_members.join(", ")
            ));
        }
        for r in &self.refused {
            parts.push(format!("拒：{r}"));
        }
        parts.join("；")
    }

    /// 这一趟真的动过机器吗。
    pub fn touched(&self) -> bool {
        self.created + self.renamed + self.removed + self.moved_in + self.moved_out + self.reordered
            > 0
    }
}

/// 机器那一侧的顺序表读法：`PinSet` 里那一列组，按 §5.12.3 的 quick-switch 次序配上各自成员。
///
/// 与 [`Config::roster`] 是同一份形状的两个来源（一个从档读、一个从机器读），所以
/// [`plan_group_sync`] 拿两边比得出同一件事。**只有这一处**把 `PinSet` 那一列组拼成 roster
/// （#281）：导出与导入两侧都走这一手，于是「档里说的」与「机器上的」永远同一枚口径。
pub fn roster_of(set: &PinSet) -> Vec<GroupRoster> {
    set.groups()
        .iter()
        .map(|g| GroupRoster {
            name: g.name.clone(),
            members: set
                .pins_in_group(g.id)
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
        })
        .collect()
}

/// 机器里有没有这一枚组名。名字就是这一族的身份（见 [`Groups`] 头顶那句），所以按名找。
fn group_id_of(set: &PinSet, name: &str) -> Option<GroupId> {
    set.groups().iter().find(|g| g.name == name).map(|g| g.id)
}

/// 两份成员列说的是同一批贴图吗（不比次序、不比重复）。
fn same_ids(a: &[String], b: &[String]) -> bool {
    a.iter().all(|x| b.contains(x)) && b.iter().all(|x| a.contains(x))
}

/// 两份顺序表之间「机器那一枚 == 档里那一枚」的配对：返回机器那侧的下标 → 档那侧的下标，
/// `None` = 档里没有这一枚（⇒ 该删）。
///
/// 两轮：先同名，再只差大小写的那一枚。每一枚只被配一次，按档里的次序先到先得（一枚机器组只配
/// 一枚档里组，纯函数不赌上游归没归一）。
///
/// 这一份判断**只许有一个作者**（#281）：[`plan_group_sync`] 按它走步子，
/// [`Config::diff_groups`] 按它报「将被覆盖的项目」。两处各配一遍的话，档里写 `Ref` 而机器上叫
/// `ref` 时报出去的就是「删掉 ref、新建 Ref」而做的却是「改名」—— 而 §5.20.3 步骤 5 那句
/// 「用户确认后应用」，用户确认的正是他看见的那一列。
pub fn match_groups(current: &[GroupRoster], target: &[GroupRoster]) -> Vec<Option<usize>> {
    let mut claim: Vec<Option<usize>> = vec![None; current.len()];
    let mut used: Vec<bool> = vec![false; target.len()];
    for round in 0..2 {
        for ti in 0..target.len() {
            if used[ti] {
                continue;
            }
            let pick = (0..current.len()).find(|&ci| {
                claim[ci].is_none()
                    && if round == 0 {
                        current[ci].name == target[ti].name
                    } else {
                        current[ci].name.eq_ignore_ascii_case(&target[ti].name)
                    }
            });
            if let Some(ci) = pick {
                claim[ci] = Some(ti);
                used[ti] = true;
            }
        }
    }
    claim
}

/// 一份档说要什么形状 ⇒ 机器要走出哪几步。**纯函数，一枚也不动**，所以单测钉得住它，而
/// [`sync_store_groups`] 只需按序把步子交给 `PinSet` 已有的公开手（#281：分组那一族不许有
/// 第二处作者）。
///
/// 步子的次序就是执行的次序，而这一序有道理：① 先删（腾出组名，成员退回未分组）；② 再改名
/// （腾出更多组名）；③ 再新建；④ 搬成员进来（此时每一枚目标组都已经在机器上了）；⑤ 再把
/// 谁也不要的退回未分组；⑥ 最后挪序 —— 前五步都会动 `groups` 那一列的位置，先挪就白挪一遍。
///
/// 「档里那枚与机器上那枚只差大小写」走**改名**而不是新建：理由见 [`match_groups`] 头顶那一段。
pub fn plan_group_sync(current: &[GroupRoster], target: &[GroupRoster]) -> Vec<GroupStep> {
    let claim = match_groups(current, target);
    let matched: Vec<bool> = (0..target.len())
        .map(|ti| claim.iter().any(|c| *c == Some(ti)))
        .collect();

    let mut steps: Vec<GroupStep> = Vec::new();
    // ① 删（不销毁贴图，见 [`GroupStep::Remove`]）
    for (ci, c) in current.iter().enumerate() {
        if claim[ci].is_none() {
            steps.push(GroupStep::Remove {
                name: c.name.clone(),
            });
        }
    }
    // ② 改名
    for (ci, ti) in claim.iter().enumerate() {
        if let Some(ti) = ti {
            if current[ci].name != target[*ti].name {
                steps.push(GroupStep::Rename {
                    from: current[ci].name.clone(),
                    to: target[*ti].name.clone(),
                });
            }
        }
    }
    // ③ 新建
    for ti in 0..target.len() {
        if !matched[ti] {
            steps.push(GroupStep::Create {
                name: target[ti].name.clone(),
            });
        }
    }
    // ④ 成员：这一组该有的全部（`Take` 自带「从别组搬来」，所以别组不需要自己的步子）。
    // 新建的那一组也归这一步：它不在 `current` 里，③ 只把它建出来、成员是空的，而档里写着
    // 它该有哪些 —— 只遍历 `current` 的话，「新建 + 带成员」那一份档落下来就是一枚空组，
    // 机器与档各执一份成员表，而那正是本文件头顶要挡的「报了却落不下去」。
    for ti in 0..target.len() {
        let here: [String; 0] = [];
        let now = match claim.iter().position(|c| *c == Some(ti)) {
            Some(ci) => &current[ci].members[..],
            None => &here[..],
        };
        if !same_ids(now, &target[ti].members) {
            steps.push(GroupStep::Take {
                name: target[ti].name.clone(),
                members: target[ti].members.clone(),
            });
        }
    }
    // ⑤ 谁也不要：档里哪一组都不提的成员，从它现在那一组退回未分组
    let wanted: Vec<&String> = target.iter().flat_map(|g| g.members.iter()).collect();
    for ci in 0..current.len() {
        let Some(ti) = claim[ci] else { continue };
        let left: Vec<String> = current[ci]
            .members
            .iter()
            .filter(|m| !wanted.contains(m))
            .cloned()
            .collect();
        if !left.is_empty() {
            steps.push(GroupStep::Ungroup {
                name: target[ti].name.clone(),
                members: left,
            });
        }
    }
    // ⑥ 排序：先在一份影子列上算出 delta，挪的步子才与 `PinSet::move_group` 的夹边同一条算式
    let mut living: Vec<String> = current
        .iter()
        .enumerate()
        .filter(|(ci, _)| claim[*ci].is_some())
        .map(|(ci, _)| target[claim[ci].unwrap()].name.clone())
        .collect();
    for ti in 0..target.len() {
        if !matched[ti] {
            living.push(target[ti].name.clone());
        }
    }
    for i in 0..target.len() {
        let name = &target[i].name;
        let Some(pos) = living.iter().position(|n| n == name) else {
            continue;
        };
        if pos != i {
            steps.push(GroupStep::Move {
                name: name.clone(),
                delta: i as i32 - pos as i32,
            });
            let moved = living.remove(pos);
            let at = i.min(living.len());
            living.insert(at, moved);
        }
    }
    steps
}

/// §5.20.3 步骤 5 在分组这一族上的那一手：把一份档说的形状落到 `PinSet` 上，带回来一份账。
///
/// 每一手都是 `PinSet` 自己的公开手（`create_group`/`rename_group`/`delete_group`/
/// `assign_group`/`move_group`），这里只做编排与记账，不新写一份分组规矩（#281）。
/// 收尾还**对一次账**（[`roster_of`] 与档再比一遍）：搬不动的那些（机器拒了的名字、档里点名
/// 而这台机器没有的贴图）必须留得下一句话，§8.3「别静默吞掉」在分组这一族上要的就是这一句。
pub fn sync_store_groups(set: &mut PinSet, roster: &[GroupRoster]) -> GroupSync {
    let mut out = GroupSync::default();
    let current = roster_of(set);
    let have: Vec<String> = set.items().iter().map(|p| p.id.to_string()).collect();
    let mut lost: Vec<String> = Vec::new();
    let note_lost = |ids: &[String], lost: &mut Vec<String>| {
        for id in ids {
            if !have.iter().any(|h| h == id) && !lost.contains(id) {
                lost.push(id.clone());
            }
        }
    };

    for step in plan_group_sync(&current, roster) {
        match step {
            GroupStep::Remove { name } => {
                let Some(id) = group_id_of(set, &name) else {
                    out.refused
                        .push(format!("删「{name}」：机器上没有这一枚组"));
                    continue;
                };
                match set.delete_group(id, GroupFate::Ungroup) {
                    Ok(n) => {
                        out.removed += 1;
                        out.moved_out += n;
                    }
                    Err(e) => out.refused.push(format!("删「{name}」：{e}")),
                }
            }
            GroupStep::Rename { from, to } => {
                let Some(id) = group_id_of(set, &from) else {
                    out.refused
                        .push(format!("改名「{from}」→「{to}」：机器上没有这一枚组"));
                    continue;
                };
                match set.rename_group(id, &to) {
                    Ok(()) => out.renamed += 1,
                    Err(e) => out.refused.push(format!("改名「{from}」→「{to}」：{e}")),
                }
            }
            GroupStep::Create { name } => match set.create_group(&name) {
                Ok(_) => out.created += 1,
                Err(e) => out.refused.push(format!("新建「{name}」：{e}")),
            },
            GroupStep::Take { name, members } => {
                let Some(id) = group_id_of(set, &name) else {
                    out.refused
                        .push(format!("把成员搬进「{name}」：组没建起来"));
                    note_lost(&members, &mut lost);
                    continue;
                };
                note_lost(&members, &mut lost);
                let known: Vec<PinId> = members
                    .iter()
                    .filter(|m| have.iter().any(|h| h == *m))
                    .filter_map(|m| m.parse::<PinId>().ok())
                    .collect();
                if known.is_empty() {
                    continue;
                }
                let before = set.pins_in_group(id);
                match set.assign_group(&known, Some(id)) {
                    Ok(_) => out.moved_in += known.iter().filter(|p| !before.contains(p)).count(),
                    Err(e) => out.refused.push(format!("搬进「{name}」：{e}")),
                }
            }
            GroupStep::Ungroup { name, members } => {
                let Some(id) = group_id_of(set, &name) else {
                    out.refused
                        .push(format!("把「{name}」的成员退回未分组：机器上没有这一枚组"));
                    continue;
                };
                let known: Vec<PinId> = members
                    .iter()
                    .filter_map(|m| m.parse::<PinId>().ok())
                    .collect();
                let before = set.pins_in_group(id);
                match set.assign_group(&known, None) {
                    Ok(_) => out.moved_out += before.iter().filter(|p| known.contains(p)).count(),
                    Err(e) => out.refused.push(format!("退回未分组：{e}")),
                }
            }
            GroupStep::Move { name, delta } => {
                let Some(id) = group_id_of(set, &name) else {
                    out.refused
                        .push(format!("挪序「{name}」：机器上没有这一枚组"));
                    continue;
                };
                let before = set.group_ids();
                match set.move_group(id, delta) {
                    Ok(()) => {
                        if set.group_ids() != before {
                            out.reordered += 1;
                        }
                    }
                    Err(e) => out.refused.push(format!("挪序「{name}」：{e}")),
                }
            }
        }
    }
    out.unknown_members = lost;

    // 对账：档里说的（扣掉这台机器压根没有的那几枚贴图）与机器上落的，再比一遍。
    let now = roster_of(set);
    let names = |r: &[GroupRoster]| r.iter().map(|g| g.name.clone()).collect::<Vec<_>>();
    let want: Vec<GroupRoster> = roster
        .iter()
        .map(|g| GroupRoster {
            name: g.name.clone(),
            members: g
                .members
                .iter()
                .filter(|m| have.contains(m))
                .cloned()
                .collect(),
        })
        .collect();
    let (got, expect) = (names(&now), names(&want));
    for miss in expect.iter().filter(|n| !got.contains(n)) {
        out.refused
            .push(format!("档里说要有「{miss}」，机器上没有"));
    }
    for extra in got.iter().filter(|n| !expect.contains(n)) {
        out.refused
            .push(format!("机器上多出一枚「{extra}」，档里没有"));
    }
    let common = |list: &Vec<String>, other: &Vec<String>| -> Vec<String> {
        list.iter().filter(|n| other.contains(n)).cloned().collect()
    };
    if common(&expect, &got) != common(&got, &expect) {
        out.refused.push("分组次序与档里不同".to_string());
    }
    for w in &want {
        if let Some(g) = now.iter().find(|n| n.name == w.name) {
            if !same_ids(&g.members, &w.members) {
                out.refused.push(format!(
                    "「{}」落的成员是 {}，档里写的是 {}",
                    w.name,
                    g.members.join(", "),
                    w.members.join(", ")
                ));
            }
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Output {
    /// §5.5.1 always put the result on the clipboard.
    pub auto_copy: bool,
    /// §5.5.2 automatic file save without a dialog.
    pub auto_save: bool,
    /// Empty = the pictures folder (§6.2).
    pub save_dir: String,
    pub format: Format,
    /// §5.5.5 JPG quality.
    pub jpeg_quality: u32,
    /// §5.5.3 naming template, see [`crate::naming`].
    pub naming_template: String,
    /// §5.5.3's 自定义前缀 — the text `%p` of [`Self::naming_template`] stands for.
    pub naming_prefix: String,
    pub collision: Collision,
    /// §5.5.6 rounded corners; 0 off.
    pub corner_radius: u32,
    /// §5.5.7 drop shadow; 0 off. The shadow's own margin in physical pixels - the
    /// export grows by this much on every side, which is where the fade lives.
    pub shadow_size: u32,
    /// §5.5.6 规则 ("圆角外部区域应透明，或按配置填充背景色") 的那枚"按配置"。
    ///
    /// Empty is 透明, the section's first reading and the shipped one; anything
    /// [`crate::colors::parse`] reads fills the four corners instead. It is a colour
    /// *string* rather than an enum because that is how every other colour this file
    /// carries is spelled (`appearance.accent`, `annotation.stroke_color`), and because
    /// a fill has to be able to say "partly" — `#00000000` is a thing a user may want.
    pub corner_fill: String,
    /// §5.5.7 step 2 的边框宽度, physical pixels; 0 off. Kept separate from
    /// `annotation.stroke_width`: that one is a pen's, this one is the export's.
    pub border_width: u32,
    /// §5.5.7 step 2 的边框颜色. Only read when [`Self::border_width`] is non-zero.
    pub border_color: String,
    /// §5.5.7 step 2 的阴影强度, percent of full black. The section asks for 强度 and
    /// 边距 but no 阴影颜色, so the hue stays the one [`crate::imageops::Plate`] ships
    /// with (black) and only its alpha moves — inventing a fifth key would be a setting
    /// the requirement never asked for.
    pub shadow_strength: u32,
    /// PRD 491（§5.5.4 快速保存）步骤 4 那一档：落盘之后自动把目标打开一次。
    ///
    /// 曾经是一枚没有读者的键（第 35 项 ⑨），P45 起由 `MaskState::quick_out` 读取；
    /// 成功通知上那一次点击不受它管，开关关着通知照样点得开。
    pub reveal_after_save: bool,
    /// PRD §5.5.11 文件保存对话框联动步骤 1 的那一格开关。
    ///
    /// 关着 ⇒ 整段联动不发生（藏 0 枚、恢复 0 枚）；开着 ⇒ 出现保存对话框时把**可能遮挡
    /// 它**的贴图临时藏起来，对话框关闭后放回。判断本体在 [`crate::dialog_link`]，这里只有
    /// 用户那一格。`Default` 是 `false`：PRD 把这一条写成"用户在设置中开启"，不是一枚默认行为。
    ///
    /// 本节号是对着 PRD 的目录数的（575 行那一节），因此它与下面两枚邻居**不同源**：
    /// `share_targets` 标的 §5.5.11、`send_to_apps` 标的 §5.5.12 都少了一格，真号是 §5.5.9 /
    /// §5.5.10 —— 全线错号那一笔账是计划第 175 项，至今 52 处产品码未订正，本轮不顺手改。
    pub hide_pins_on_dialog: bool,
    /// §5.5.11 share targets shown in the toolbar.
    pub share_targets: Vec<String>,
    /// §5.5.12 "send to app" entries: `name|executable`.
    pub send_to_apps: Vec<String>,
    /// §5.5.10 refresh the last output instead of making a new one.
    pub reuse_last_file: bool,
    /// PRD §5.9.17 步骤 1 的那一格开关：「复制图片时同时提供文件」。
    ///
    /// 关着 ⇒ 复制只交 CF_DIB + PNG（§5.9.16 那一问，一直是那样）；开着 ⇒ 同一次
    /// `OpenClipboard`/`EmptyClipboard` 交易里多出第三路 `CF_HDROP`，指向一张按
    /// [`crate::naming`] 那族既有规则命名的临时 PNG。写成两枚键的样子而不是让它默认发生，
    /// 是因为 PRD 把那一句写在"用户在设置中开启"下面：`Default` 是 `false`。
    ///
    /// 它与一枚闲着的键 [`Advanced::privacy_no_temp_files`] **不是**同一问：那枚标的是 §7.5，
    /// 而 §7.5 那四条（域名提示、响应大小、参数注入、路径校验）里没有一句说"不许落盘"，
    /// 本轮不替它编读者。这一枚管的是 §6.2 第 1600-1601 行：「临时文件应按策略自动清理」+
    /// 「应避免在退出时删除仍被剪贴板引用的文件」—— 持有与清理的规则在
    /// `qt_bridge::state::PinState::clip_pending_files` 那一侧，不在这里。
    pub copy_as_file: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            auto_copy: true,
            auto_save: false,
            save_dir: String::new(),
            format: Format::Png,
            jpeg_quality: 92,
            naming_template: crate::naming::DEFAULT_TEMPLATE.into(),
            naming_prefix: "falcon".into(),
            collision: Collision::Increment,
            corner_radius: 0,
            shadow_size: 0,
            corner_fill: String::new(),
            border_width: 0,
            border_color: "#000000".into(),
            shadow_strength: 50,
            reveal_after_save: false,
            hide_pins_on_dialog: false,
            share_targets: Vec::new(),
            send_to_apps: Vec::new(),
            reuse_last_file: false,
            copy_as_file: false,
        }
    }
}

/// §5.16.2 one exclusion rule.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppExclusion {
    /// Executable name or path pattern, matched case-insensitively.
    pub app: String,
    /// Action ids this rule silences; empty = every global hotkey.
    pub actions: Vec<String>,
}

/// §5.16.2 那一串规则在档里的名字。
///
/// 与 [`AUTOSTART_KEY`] 同一枚理由：页上那一行的写手、[`Config::add_exclusion`]、以及「哪一枚
/// 键在页上写不动」那一句抱怨，问的是同一枚键，一处一名就是三份拼写。它与
/// [`Config::set_str`] 的 `Err` 那一臂一起被单测钉着（`hotkey_exclusions_keep_their_shape`）。
pub const EXCLUSION_KEY: &str = "hotkey.exclusions";

/// [`Config::add_exclusion`] 的两种成。
///
/// 为什么不是 `set_str` 那一族的 `Result<Vec<String>, ConfigError>`：「这一位前台此刻已经被一枚
/// 规则管着了」既不是「写坏了」也不是「写成了」，而是一句**一个字节也没动档**的读数。塞进
/// `Err` 那一侧，页上就只能拿同一句「未写入」去盖两种不同的事；塞进 `Ok` 里那一串校正，又是
/// 拿 `Vec<String>` 说一件它说不出名的东西。所以这一问自己有一枚返回类型 —— 而**字**归页上那一头
/// 说（`qt_bridge::settings_view` 的 `exclusion_*_said`）：core 交的是事实，不是措辞。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExclusionAdd {
    /// 添成了。带的是 [`Config::validate`] 对这整份档的那一份校正，与 `set_str` 交回的是同一串。
    Added(Vec<String>),
    /// 一枚也没添：此刻已经管得住这一位前台的那一枚规则，它的 `app` 写着 `matched`。
    Already { matched: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkey {
    /// Action id → key spec (`ctrl+shift+x`).
    pub actions: BTreeMap<String, String>,
    pub exclusions: Vec<AppExclusion>,
    /// §5.16.1 第十三名 `custom_task` 要跑的那一条命令，**用户自己写**的那一份原文。
    ///
    /// 它是 `String` 而不是 `Vec<String>`：数组那一族的 `flat()`/`coerce()` 用 `, ` 连接、按 `,`
    /// 拆回（`output.send_to_apps` 那一条写的就是这一形状），而一条命令行里最不缺的就是逗号 ——
    /// 存进数组的那一天，`--export,--out` 会被拆成两枚参数。这里要的恰好相反：进去一串字、出来
    /// 一串字，怎么拆是第一枚参数、哪一段是参数块，全归 [`crate::tasks`] 那一判，档不参与。
    ///
    /// 出厂空着，而不是放一条示例：这一格的内容是要在这台机器上真的跑得起来的，替用户写一条
    /// 「例如 notepad」就是让一枚出厂动作对着别人的程序。空的那一支由 [`crate::tasks`] 自己说
    /// 「还没写过命令」，它和「写了一条跑不了的命令」是两件事，各有各的那句话。
    pub command: String,
}

impl Default for Hotkey {
    fn default() -> Self {
        let mut actions = BTreeMap::new();
        actions.insert("capture".into(), "ctrl+shift+a".into());
        actions.insert("capture_custom".into(), "ctrl+shift+f".into());
        actions.insert("capture_repeat".into(), "ctrl+shift+x".into());
        actions.insert("capture_active_window".into(), "print".into());
        actions.insert("paste_pin".into(), "ctrl+shift+v".into());
        actions.insert("toggle_pins".into(), "ctrl+shift+p".into());
        actions.insert("switch_group".into(), "ctrl+shift+g".into());
        actions.insert("solo".into(), "ctrl+shift+s".into());
        actions.insert("pin_edit".into(), "ctrl+shift+e".into());
        // `release_click_through` **不在这一列里**，而这一格是照搬已有那三枚（`show_all_pins` /
        // `hide_all_pins` / `custom_task`）的形状：它们在 [`ACTIONS`] 名单上、有归宿、页上却不成行，
        // 因为出厂默认不替它们占一枚键。这一枚跟着同一枚形状走的两枚理由：① 出厂就抢一枚
        // `ctrl+shift+?` 是一项产品决定（浏览器拿 `ctrl+shift+t` 当「重新打开关闭的标签页」），
        // 而 §5.16.1 那张「可绑定范围」的表里没有这一行，抢键的账该由 §5.16.1 第五步「系统检查
        // 冲突」那一次由用户点下去；② 页上成不成行读的是这一份默认表（`settings_view::rows_of`
        // 走 `Config::flat()`），动它就动了设置页那几枚硬计数。
        //
        // 于是**开箱那一条退出走的是托盘**：§5.9.13 规则第一句「必须提供明确且可靠的退出穿透
        // 方式」在这一份默认档上是靠 [`crate::config::TRAY_ACTIONS`] 与那一枚托盘菜单项成立的
        // （`qt_bridge::hotkey::click_through_exits` 判的就是这一句，两半各一次），而这一枚 id
        // 一旦被人配上手就真的接得住（`Work::ReleaseClickThrough`）。欠的那一步——出厂带不带一枚
        // 预设组合键——记在本轮的报告里，不由这一段注释冒充已完成。
        Hotkey {
            actions,
            exclusions: Vec::new(),
            command: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotCorner {
    pub enabled: bool,
    /// Corner id → action id; an empty value means "this corner is off".
    pub corners: BTreeMap<String, String>,
    /// Distance from the vertex that still counts as "the corner", in DIP.
    pub trigger_dip: u32,
    /// §5.15 step 3 停留时间: how long the pointer must sit in the corner.
    pub dwell_ms: u32,
    /// Require a modifier so maximising a window never fires one by accident.
    pub require_ctrl: bool,
    pub require_shift: bool,
    /// §5.15 规则: ignore a repeat trigger inside this window, so one sweep
    /// across the top of the screen is one action.
    pub repeat_guard_ms: u32,
    /// §5.15 规则: while a fullscreen app or game owns the desktop, the corners
    /// wait. Opt-in in the sense that the user decides whether a corner may
    /// interrupt one; the corner machine itself is the caller of `set_paused`.
    pub pause_on_fullscreen: bool,
}

impl Default for HotCorner {
    fn default() -> Self {
        let mut corners = BTreeMap::new();
        corners.insert("top_left".into(), String::new());
        corners.insert("top_right".into(), String::new());
        corners.insert("bottom_left".into(), String::new());
        corners.insert("bottom_right".into(), "capture".into());
        HotCorner {
            enabled: false,
            corners,
            trigger_dip: 8,
            dwell_ms: 300,
            require_ctrl: false,
            require_shift: false,
            repeat_guard_ms: 800,
            pause_on_fullscreen: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    /// §5.19.2 主题色.
    pub accent: String,
    /// §5.19.4 toolbar side: `auto` | `top` | `bottom` | `left` | `right`.
    pub toolbar_placement: String,
    /// §5.19.4 interface font size, in points.
    pub ui_font_size: u32,
    /// §5.19.4「支持调整界面字体和字号」里**字体**那一半（字号是上面那一枚）。
    ///
    /// 它排在 `ui_font_size` 旁边而不是 `[annotation] font_family` 那里，因为两处说的不是
    /// 一件事：那一枚写的是**标注画在图上**的那行字（像素由 raster 侧吃掉，进导出的图片），
    /// 这一枚写的是**界面自己的字**（菜单、设置页、贴图窗的边框那些不属于任何一张图的东西）。
    /// 合成一枚键就会让改界面字体的那一次点击顺手改掉图片里的字，而那两个字在 PRD 里隔
    /// 了六节。
    pub ui_font_family: String,
    /// §5.19.3 magnifier frame.
    pub magnifier_border: u32,
    pub magnifier_corner_radius: u32,
    pub magnifier_foreground: String,
    pub magnifier_background: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            accent: ACCENT_DEFAULT.into(),
            toolbar_placement: TOOLBAR_PLACEMENT_DEFAULT.into(),
            ui_font_size: 9,
            ui_font_family: UI_FONT_FAMILY_DEFAULT.into(),
            magnifier_border: 1,
            magnifier_corner_radius: 6,
            magnifier_foreground: "#FFFFFF".into(),
            magnifier_background: "#1F1F1F".into(),
        }
    }
}

/// §5.19.2「设置主题色」的出厂那一枚，也是**任何**读不出来的主题色的归宿（[`accent_on`] 的
/// 非法臂、[`Config::validate`] 那句校正、以及 [`Appearance::default`] 三处同源在这一枚字上）。
pub const ACCENT_DEFAULT: &str = "#0078D7";

/// 界面字体族与主题色之外没有别的默认了 —— 写着这句是为了记下**这里没有**第二份 `#0078D7`：
/// 全档只有上面那一枚。
pub const TOOLBAR_PLACEMENT_DEFAULT: &str = "auto";

/// §5.19.4「支持调整工具栏方向和位置」里**位置**那一枚的闭集，唯一的出处。
///
/// 归一（[`Config::validate`] 里那一格）、设置页想递给用户的候选、以及日后那把量具的尺子都读
/// 这一份；写第二份名单的那一天，页上就能选出一枚档里存不下的落点。
pub const TOOLBAR_PLACEMENTS: &[&str] = &["auto", "top", "bottom", "left", "right"];

/// 工具栏的两枚**方向**词（同一句里的"方向"）。`left` / `right` 那一头是一列（vertical），
/// `top` / `bottom` 那一头是一排（horizontal）—— 方向不是第六枚键，它是落点的函数，
/// 见 [`toolbar_side`]。
pub const TOOLBAR_SIDES: &[&str] = &["horizontal", "vertical"];

/// §5.19.4「支持调整界面字体和字号」中**字号**的两条界。
///
/// 下界 6 不是随手挑的：再小下去，100% 缩放上那行中文就已经是墨点了；上界 48 与
/// [`card::width`] 那一族同一枚理由 —— 一扇 760 px 宽的窗装不下 49 磅的正文，而「设得过界」
/// 这件事在页上必须是**被夹过并说出来**（[`Config::set_str`] 的 corrections），不是被拒到
/// 一个字也存不下去。
pub const UI_FONT_MIN: u32 = 6;
pub const UI_FONT_MAX: u32 = 48;

/// 界面字体族的出厂默认。与 [`Annotation::font_family`] 恰好同名同字，那是两件事用上了
/// 这台机器上同一枚真的装了的中文字体，不是把两枚键合成一枚的理由（见上面那段的分界）。
pub const UI_FONT_FAMILY_DEFAULT: &str = "Microsoft YaHei UI";

/// §7.4「深色与浅色主题均应保证文本和控件对比度」与 §5.19.4 最后那句的那条底线：**3:1**。
///
/// 取的是 WCAG 2.1 AA 给**非文本 UI 组件**（控件边界、状态指示、图形色）定的那一档，而不是
/// 正文那 4.5:1 —— 主题色在本产品里画的是按钮底、选中行的边框、开关那一头（`SettingsWindow.qml`
/// 那几处 `accent_fill` 的读者），正是组件那一类。写成常量而不是散在各处裸 `3.0`，是因为
/// 「哪一档算基本可读」这一句判断只许有一处回答。
pub const ACCENT_MIN_CONTRAST: f64 = 3.0;

/// 那两套餐具的**页底**：§5.19.1 浅色与深色各一面。对比度的判断要吃一枚"底"，而底只有这两面，
/// 所以底名住在这里；**页上那些 `page_fill` 仍是画的那一份**（QML 与本档之间此刻还没有一条
/// 能把这枚字递过去的绑定，那需要 `Session` 上多一枚外观纪元，见 qt_bridge 那侧的说明）。
pub const SURFACE_LIGHT: &str = "#F2F4F7";
pub const SURFACE_DARK: &str = "#12151B";

/// §5.19.2 步骤 1 那两枚手动主题的**拼法**（`system` / `light` / `dark`），与
/// [`Theme`] 的 serde 表示同序同字。
///
/// 它是 [`set_str`] 那一格闸门认得的名单：一枚 enum 键在没有这份名单时，拼错的字面会走到
/// `from_document` 那里变成一句关于**整份文件**的抱怨（`unknown variant …`），而用户刚打过
/// 的只是那一格 —— 与 [`ALPHA_BG_IDS`] / [`Unit::labels_line`] 两轮为同一件事立的牌子同形。
pub const THEME_IDS: &[&str] = &["system", "light", "dark"];

/// 工具栏落点的解：`auto` 吃选区的长短边，其余四枚原样交回。
///
/// `auto` 的读法与 [`crate::geometry`] 那侧「长边优先」同一枚直觉：横长的选区把工具栏摆在
/// **上**边（离指针最远的一边，PRD §5.5 那一族按钮不会盖住正在画的那条线），竖长的摆在
/// **左**边。选区还没框出来（两边都是 0）时按横长走 —— `auto` 必须永远答出一枚落点，
/// 答不出就把「auto 是一枚没实现的落点」这件事推给界面去猜。
pub fn toolbar_edge(placement: &str, sel_w: i32, sel_h: i32) -> &'static str {
    // 四枚字面各写一次而不是 `=> placement`：那一写法要的是 `'static`，而传进来的那一枚借用
    // 活不到那时候（编译器的这一问正好替本档说出了「交回的是名单里那四枚落点，不是用户打过
    // 的那串字」这件事）。
    match placement {
        "top" => "top",
        "bottom" => "bottom",
        "left" => "left",
        "right" => "right",
        // `auto` 与任何认不出的字面都走 `auto` 那一臂：[`Config::validate`] 已经替**从档里读
        // 来的**那一枚归过一，这里再拦一次是给**没经过那道闸的调用方**（测试、以及日后任何
        // 直接拼字符串的手）兜底，认不出的字面不许变成一枚看不见的落点。
        _ => {
            if sel_h > sel_w {
                "left"
            } else {
                "top"
            }
        }
    }
}

/// 工具栏方向 = 落点的函数，不是第六枚键。
///
/// 这一句是 §5.19.4「支持调整工具栏方向和位置」里"方向"那一半的全部读法：摆在上下就是一排
/// （horizontal），摆在左右就是一列（vertical）。分开出第二枚键会让两枚键各说一面 ——
/// 用户把方向设成 horizontal、位置设成 left，界面只能违反其中一句。
pub fn toolbar_side(placement: &str, sel_w: i32, sel_h: i32) -> &'static str {
    match toolbar_edge(placement, sel_w, sel_h) {
        "left" | "right" => TOOLBAR_SIDES[1],
        _ => TOOLBAR_SIDES[0],
    }
}

/// §5.19.4 高分辨率那一枚的算术本体：磅 → 像素。
///
/// 96/72 是 Qt 在**没有别的说明时**用的定率（1 磅 = 1/72 英寸、逻辑 96 像素/英寸），所以
/// 9 磅正好是 12 像素 —— 与本仓界面里那一路 `font.pixelSize: 12` 同一枚数，这不是巧合，那是
/// 出厂字号 9 磅在 100% 缩放下应当长成的样子（`UI_FONT_MIN..UI_FONT_MAX` 夹住的就是这一档）。
///
/// **真实 DPR 那一乘不归这里**：QML 的 `font.pixelSize` 吃的是与设备无关的像素，缩放由 Qt 按
/// 那一面屏自己乘；界面因此优先绑 [`Appearance::ui_font_size`] 的**磅**（`font.pointSize`），
/// 这一枚函数只服务「要说出一枚像素数」的读者（设置页那一行的预览）。把两处都乘一遍就是
/// 高分屏上那行字大两号的那一种坏。
pub const fn ui_font_px(points: u32) -> u32 {
    // `* 4 / 3` 而不是 `as f64 * 96.0 / 72.0`：整型这一条路对 9 磅这种整数点精确，而 `const fn`
    // 里浮点比较与舍入都要额外一层，换不来任何一句能被测试说清的东西。
    points * 4 / 3
}

/// WCAG 2.1 的相对亮度（那一串 sRGB 线性化与 0.2126/0.7152/0.0722 的加权和）。
///
/// 读的是 [`crate::colors::parse`]，所以本档能认的形状（`#RGB`/`#RRGGBB`/`rgb()`/`hsl()`）
/// 这里就能算；认不出交回 `None` = 这一枚字面不配得到一个对比度，而不是得到一个 0。
fn luminance(color: &str) -> Option<f64> {
    let [r, g, b, _] = crate::colors::parse(color)?;
    let channel = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b))
}

/// 两枚色之间的对比度（1.0..21.0）。任一枚读不出来就交回 `None`。
pub fn contrast_ratio(foreground: &str, background: &str) -> Option<f64> {
    let a = luminance(foreground)?;
    let b = luminance(background)?;
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    Some((hi + 0.05) / (lo + 0.05))
}

/// §5.19.4 最后那句「所有主题下均应满足基本可读性和对比度要求」的那一手：把一枚主题色夹到
/// 能在这一块底上看得见的位置。
///
/// 只动**明度**（HSL 的 L），色相与饱和度留着 —— 用户挑的是一枚蓝，回来的还得是那枚蓝；
/// 底色亮（浅主题）就往暗处走、底色暗（深主题）就往亮处走，一步一步直到
/// [`ACCENT_MIN_CONTRAST`] 或撞到 0/1 两端。两端都撞开了还不够（一枚纯黑纯白也救不了与它同头的
/// 底），那也如实交回**夹到尽头那一枚**：界面拿到的是「能给的最好的一枚」，而不是 `None` 这种
/// 要调用方自己发明兜底的东西。
///
/// 读不出来的字面（非法颜色）走出厂默认 [`Appearance::default`] 的那枚 `#0078D7`：那一枚在
/// 两套餐具上都是够的（`accent_keeps_hue_and_meets_the_floor` 逐面点过），所以「用户还没打过
/// 一枚合法色」这一格不会画出一枚谁也没见过的字。
pub fn accent_on(accent: &str, surface: &str) -> String {
    // 两枚字面都要读得出来，否则这里没有任何一手能算对比度：非法的颜色走出厂那一枚兜底，
    // 非法的底色把用户那一枚**原样**交回（底色不归调用方发明，而「原样」是这一格里唯一
    // 不需要新事实的回答）。
    let [r, g, b, _] = match crate::colors::parse(accent) {
        Some(rgba) => rgba,
        None => return fallback_accent(surface),
    };
    let Some(surface_lum) = luminance(surface) else {
        return accent.to_string();
    };
    // 底色亮 = 往暗处夹，底色暗 = 往亮处夹。这一句判断的就是方向，别的都没有。
    let dir = if surface_lum >= 0.5 { -1.0 } else { 1.0 };
    // [`crate::colors`] 的 HSL 是「色相 0..360 度、饱和度与明度 0..=100 **百分数**」，不是
    // 0..1 的比例 —— 按比例去夹会把一枚蓝一路夹到 1.0 那一头（= 白），而那一枚才是坏。
    let (h, s, l0) = crate::colors::rgb_to_hsl(r, g, b);
    for step in 0..=100u32 {
        let l = (l0 + dir * f64::from(step)).clamp(0.0, 100.0);
        let (nr, ng, nb) = crate::colors::hsl_to_rgb(h, s, l);
        let hex = crate::colors::format([nr, ng, nb, 255], crate::colors::ColorFormat::HexUpper);
        // `step == 100` 那一臂就是注释里说的「撞到尽头」：够不够都交回这一枚。
        if contrast_ratio(&hex, surface).is_some_and(|c| c >= ACCENT_MIN_CONTRAST) || step == 100 {
            return hex;
        }
    }
    accent.to_string()
}

/// 非法主题色的归宿：出厂那一枚 [`ACCENT_DEFAULT`]，对它该看见的底走**与主路完全同一条**夹法
/// （递归一步就到底：那一枚字面是本档自己写的、读得出来，所以这一次不会再落进非法臂）。
fn fallback_accent(surface: &str) -> String {
    accent_on(ACCENT_DEFAULT, surface)
}

/// §5.18.2 自定义托盘鼠标动作：四枚键 = 左键、双击、中键、右键各跑哪一枚动作。
///
/// 默认值不是随手挑的：它是本轮改码**之前**这台机器上真实在跑的那一身行为（左键与双击都起截图、
/// 右键开菜单、中键没人理会）。这样做的是"变成可配"这一件事本身，而不是顺手替用户改掉他早就
/// 习惯的点击 —— 后者是另一条需求，文档没写，这里就不发明。
///
/// 四枚键**即时生效**：泵线程每一次点击都现问一次配置，所以这一节不进 [`RESTART_KEYS`]，
/// 设置页上那四行也就不会挂着`（需重启）`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tray {
    /// 左键单击。
    pub left: String,
    /// 左键双击。
    pub double: String,
    /// 中键单击。
    pub middle: String,
    /// 右键单击。
    pub right: String,
}

impl Default for Tray {
    fn default() -> Self {
        let mut tray = Self {
            left: String::new(),
            double: String::new(),
            middle: String::new(),
            right: String::new(),
        };
        for (name, action) in TRAY_DEFAULTS {
            tray.set_entry(name, action);
        }
        tray
    }
}

impl Tray {
    /// 按 §5.18.2 点名的键位顺序交出 (键名, 当前动作) 四枚。
    ///
    /// 判据、归一、门、页都读这一份顺序：四枚字段在外面各写一遍，加第五枚时漏一处就少查一格，
    /// 而"少查一格"在门上是绿的不是红的。
    pub fn entries(&self) -> [(&'static str, &str); 4] {
        [
            ("left", self.left.as_str()),
            ("double", self.double.as_str()),
            ("middle", self.middle.as_str()),
            ("right", self.right.as_str()),
        ]
    }

    /// 按键名写一枚动作。名单外的名字**不写**：这一枚只服务上面那四枚，写错名字必须是这里
    /// 静悄悄地什么都不发生，而不是把另一枚键顶掉。
    pub fn set_entry(&mut self, name: &str, action: &str) {
        match name {
            "left" => self.left = action.to_string(),
            "double" => self.double = action.to_string(),
            "middle" => self.middle = action.to_string(),
            "right" => self.right = action.to_string(),
            _ => {}
        }
    }

    /// 本键的默认动作，供归一落回。名单外的键名交 `"none"`。
    pub fn default_entry(name: &str) -> &'static str {
        TRAY_DEFAULTS
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .unwrap_or("none")
    }

    /// §5.18.1 那张菜单表与 §5.18.2 那四枚键共用的动作名 → 人话。门与页都读它，这样
    /// "这一格显示什么"与"这一格存什么"不会因为两处各写一份字典而漂开。
    ///
    /// **这一枚函数曾经自己养着一本名册**：八枚键各写死一句中文，而那几句与
    /// [`ACTION_TEXTS`] 里同名的几枚**不一样**（`paste_pin` 那里叫「贴图」、这里叫「贴出剪贴板」，
    /// `settings` 是「打开设置页」对「首选项」，`none` 是「不做任何事」对「不绑定」），
    /// 名单也只有八枚 —— 其余四枚落进 `other` 那一臂，交回的是英文 id 本身。本轮把名册交还给
    /// 那张唯一的表，是因为同一轮里「一枚动作的中文名」第一次有了一本正账（#281）：留着第二本
    /// 不会天天错，它只会在两本各改各的那天起，让配置那一层的证词与托盘上那一项各说一个名。
    ///
    /// 名单外的值**原样交回**而不是折成一句"未知动作"：证词里看见的是盘上真存着的那串字，
    /// 不是这里替它起的名字。返回类型仍是 `Cow` 而不是 `&'static str`，是因为这一枚的调用方
    /// 要的那一串本来就可能来自一份配置而不是名单。
    pub fn label(action: &str) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(action_texts(action).map_or(action, |(name, _)| name))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub enabled: bool,
    /// §5.14.1 第二条"用户可开启『保存所有截图记录』，包括被取消的截图"那一枚开关。
    ///
    /// 默认 `false` 是按 PRD 的字面来的：它写的是"可开启"，不是"默认开启" —— 一次没要
    /// 结果的 Esc 就往盘上长一张图，是替用户做了他要不要还没说过的决定。它也不与
    /// [`Self::enabled`] 合并：`enabled` 管的是"这次截图记不记"，这一枚管的是"没做成
    /// 的那次记不记"，两个问题各有自己的分岔（Esc 那一头的读者是
    /// `qt_bridge::mask::esc_owes_row`，确认那一头不看这一枚）。
    pub save_all: bool,
    /// §5.14.4 the three retention conditions; 0 disables that one.
    pub max_items: u32,
    pub max_age_days: u32,
    pub max_size_mb: u32,
    /// §5.14.2 keep the full-resolution original on disk.
    pub keep_originals: bool,
    /// §6.2 thumbnail edge in pixels.
    pub thumb_px: u32,
    /// §5.14.5 locked entries survive cleanup.
    pub exempt_locked: bool,
}

impl Default for History {
    fn default() -> Self {
        Self {
            enabled: true,
            save_all: false,
            max_items: 1000,
            max_age_days: 90,
            max_size_mb: 4096,
            keep_originals: true,
            thumb_px: 256,
            exempt_locked: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    /// §5.17 register the `snip` command line entry point.
    pub cli_enabled: bool,
    /// `off` | `error` | `warn` | `info` | `debug` | `trace`.
    pub log_level: String,
    /// Empty = the platform default under `%APPDATA%` (§6.1).
    pub data_dir: String,
    /// §5.6 run OCR / QR recognition right after a capture.
    pub recognize_text: bool,
    /// §7.2 write a recovery snapshot of live pins.
    pub crash_recovery: bool,
    /// §7.5 keep the frozen frame in memory only, never on disk.
    pub privacy_no_temp_files: bool,
}

impl Default for Advanced {
    fn default() -> Self {
        Self {
            cli_enabled: false,
            log_level: "info".into(),
            data_dir: String::new(),
            recognize_text: false,
            crash_recovery: true,
            privacy_no_temp_files: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct About {
    /// §5.21.1
    pub auto_check_updates: bool,
    pub check_every_days: u32,
    /// `stable` | `beta` | `nightly`. 名单只有一个作者：[`crate::update::CHANNELS`]。
    /// 这里再写一遍字面量的话，加一枚通道就会变成「Config 认为合法、而清单那一层拒绝」——
    /// 用户看到的是一句「别针配好了但检查更新说不认识」，那是两张表互相矛盾的形状的标本。
    pub channel: String,
    /// §5.21.1 第 3 步「系统获取版本信息」的那份东西从哪儿来。两种形态都认：一枚本机路径按路径
    /// 读，一句 `http(s)` 地址**真的去取**（`platform_windows::fetch`，走这台机器的 wininet）。
    /// 地址那一支是后来的：本 build 早期它交回的是一句「只认本机路径」的拒绝，那一格已随
    /// HTTP 那一只手落地一起退役，现在取不回说的是 [`crate::update::Answer::FetchFailed`]。
    /// §6.3 那一枚「应明确提示网络请求」由 [`crate::update::net_notice`] 在那一趟发起前说。
    pub release_feed: String,
    /// §5.21.3 attach diagnostics to feedback.
    pub send_diagnostics: bool,
    /// §5.21.3 第 4 步「跳转到反馈渠道」里**那一枚渠道从哪儿来** —— 本 crate 里它唯一的作者。
    ///
    /// 与 [`About::release_feed`] 同一分工：那一枚说的是「版本信息去哪儿取」，这一枚说的是
    /// 「反馈去哪儿交」。但它**只**是一串地址的字，形态上不比 [`About::release_feed`] 宽：读者走的
    /// 是 `qt_bridge::session::open_web` 那一道闸（`falcon_core::link::classify` 只认 `http`/`https`
    /// 且域名全 ASCII），写一枚本机路径进来换来的是那道闸自己那句「读不出主机名」，不是「于是按
    /// 路径打开一个文件夹」。认不认得也不由这里判，这里只负责「档上写的是哪一串」。
    ///
    /// 出厂**空着**：替用户预填一句地址，就是让一枚出厂默认把用户的机器往别人那台服务器上指。
    /// 空着的后果由读者那一份句子说清楚（「还没配反馈渠道」≠「配的地址打不开」，两件事两句话，
    /// 与 `update::Answer::NoFeed` 同一枚道理），这里不替它圆场。
    ///
    /// 它是**即时生效**的那一枚：读者是每一次按下「发送反馈」那一刻现问的（见
    /// `qt_bridge::feedback_window`），没有一扇挂着的窗等着新值，故不进 [`RESTART_KEYS`]。
    pub feedback_channel: String,
}

impl Default for About {
    fn default() -> Self {
        Self {
            auto_check_updates: true,
            check_every_days: 7,
            channel: "stable".into(),
            release_feed: String::new(),
            send_diagnostics: false,
            feedback_channel: String::new(),
        }
    }
}

/// §5.4.4 白板模式与 §5.4.5 透明白板模式：那一身底色与那块画布多大。
///
/// 两枚键存的是**用户打的那串字**，不是解析后的值：页上显示的就是盘上存的，判据读的也是
/// 同一串，解释只发生在 [`Whiteboard::surface`] 与 [`Whiteboard::size`] 这两一步上 —— 与
/// `[tray]` 那四枚键同形（#281：一份名单只能有一处解释它）。
///
/// `background` 那一格同时是 §5.4.4 与 §5.4.5 的分岔：`transparent` 就是透明白板，所以文档
/// 里那两节在这儿是一枚键而不是两枚 —— 两枚键会互相矛盾（"底色是白的"与"没有底色"不能同时
/// 为真），而一枚键不能。
///
/// 两枚键**即时生效**：白板每一趟都现读，没有一扇挂着的窗等着新值，故不进 [`RESTART_KEYS`]。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Whiteboard {
    /// §5.4.4 步骤 2「选择白板背景颜色」；写成 `transparent` 便是 §5.4.5。
    pub background: String,
    /// §5.4.4 步骤 3「与屏幕或指定区域匹配」：`screen` 或 `WxH`。
    pub size: String,
}

impl Default for Whiteboard {
    fn default() -> Self {
        Self {
            background: crate::whiteboard::DEFAULT_COLOUR.to_string(),
            size: crate::whiteboard::DEFAULT_SIZE.to_string(),
        }
    }
}

impl Whiteboard {
    /// 这一格此刻要的是哪一枚面。默认值是白，所以"读不出颜色"必须说出来而不是替用户猜。
    pub fn surface(
        &self,
    ) -> Result<crate::whiteboard::Surface, crate::whiteboard::WhiteboardError> {
        crate::whiteboard::Surface::parse(&self.background)
    }

    /// 这一格此刻要的是多大一块。
    pub fn size(&self) -> Result<crate::whiteboard::Size, crate::whiteboard::WhiteboardError> {
        crate::whiteboard::Size::parse(&self.size)
    }

    /// 两枚键的默认值，供归一落回；名单外的键名交 `None`（这一节只有两枚，写错名字不该把
    /// 另一枚顶掉 —— 与 [`Tray::set_entry`] 同一理由）。
    pub fn default_for(name: &str) -> Option<&'static str> {
        match name {
            "background" => Some(crate::whiteboard::DEFAULT_COLOUR),
            "size" => Some(crate::whiteboard::DEFAULT_SIZE),
            _ => None,
        }
    }

    /// 这一节里哪一枚键读不出来。`validate` 与设置页的闸共用的就是这一份判据：闸拦的是
    /// "正在打字的那一格"，归一处理的是"盘上已经存着的那一格"，两处若是两套规则，盘上就会
    /// 存着一份页上说不清的档。
    pub fn violations(&self) -> Vec<(&'static str, String)> {
        let mut bad = Vec::new();
        if let Err(e) = self.surface() {
            bad.push(("background", e.to_string()));
        }
        if let Err(e) = self.size() {
            bad.push(("size", e.to_string()));
        }
        bad
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub capture: Capture,
    pub whiteboard: Whiteboard,
    pub annotation: Annotation,
    pub pin: Pin,
    pub card: Card,
    /// §5.12 那一族（组名、成员归属、排序）。它是 §5.20.3 建议范围里第六枚「分组配置」在档里的
    /// 家 —— 导出/导入那一趟带的就是这一格。
    pub groups: Groups,
    pub output: Output,
    pub hotkey: Hotkey,
    pub tray: Tray,
    pub hot_corner: HotCorner,
    pub appearance: Appearance,
    pub history: History,
    pub advanced: Advanced,
    pub about: About,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadSource {
    /// No file yet; the app runs on defaults.
    #[default]
    Defaults,
    /// Read straight from the file.
    File,
    /// The file could not be understood; it was moved aside (§5.1.1).
    Recovered,
}

#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub source: LoadSource,
    /// Where the unreadable file went.
    pub backup: Option<PathBuf>,
    /// Values that were out of range and had to be corrected.
    pub warnings: Vec<String>,
}

pub const FILE_NAME: &str = "config.toml";

impl Config {
    /// An unreadable config never blocks startup (§5.1.1): defaults win, and the
    /// damaged file is kept as `config.toml.broken-<ts>`.
    pub fn load(path: &Path) -> (Config, LoadReport) {
        let mut report = LoadReport::default();
        let mut cfg = match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.source = LoadSource::Defaults;
                Config::default()
            }
            Err(e) => {
                report.source = LoadSource::Defaults;
                report.warnings.push(format!("{}: {e}", path.display()));
                Config::default()
            }
            Ok(text) => match toml_edit::de::from_str::<Config>(&text) {
                Ok(cfg) => {
                    report.source = LoadSource::File;
                    cfg
                }
                Err(e) => {
                    report.source = LoadSource::Recovered;
                    report.warnings.push(format!("{}: {e}", path.display()));
                    report.backup = quarantine(path);
                    Config::default()
                }
            },
        };
        report.warnings.extend(cfg.validate());
        (cfg, report)
    }

    /// Atomic write that keeps the existing file's comments and layout (§6.1).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| ConfigError::Io(format!("{}: {e}", dir.display())))?;
            }
        }
        let fresh = self.to_document()?;
        let text = match std::fs::read_to_string(path) {
            Ok(existing) => match existing.parse::<DocumentMut>() {
                Ok(mut doc) => {
                    merge_table(doc.as_table_mut(), fresh.as_table());
                    doc.to_string()
                }
                // Already unreadable, so a clean rewrite loses nothing usable.
                Err(_) => fresh.to_string(),
            },
            Err(_) => fresh.to_string(),
        };
        atomic_write(path, text.as_bytes())
    }

    pub fn to_document(&self) -> Result<DocumentMut, ConfigError> {
        let mut doc =
            toml_edit::ser::to_document(self).map_err(|e| ConfigError::Parse(e.to_string()))?;
        promote_tables(doc.as_table_mut());
        Ok(doc)
    }

    pub fn export_text(&self) -> Result<String, ConfigError> {
        Ok(self.to_document()?.to_string())
    }

    /// §5.20.3 step 4: a file is parsed and corrected before anything is
    /// touched, and what had to be corrected travels with it so the dialog can
    /// say "these three values were out of range" instead of quieting them.
    pub fn parse_import(text: &str) -> Result<(Config, Vec<String>), ConfigError> {
        let mut cfg: Config =
            toml_edit::de::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let warnings = cfg.validate();
        Ok((cfg, warnings))
    }

    /// Every reachable setting as `section.key` → printed value, sorted. The
    /// settings page enumerates this, so a key missing here is a key no widget
    /// can bind to either.
    pub fn flat(&self) -> Result<BTreeMap<String, String>, ConfigError> {
        let doc = self.to_document()?;
        let mut out = BTreeMap::new();
        flatten(doc.as_table(), "", &mut out);
        Ok(out)
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        self.flat().ok()?.get(key).cloned()
    }

    pub fn has_key(&self, key: &str) -> bool {
        self.flat().map(|f| f.contains_key(key)).unwrap_or(false)
    }

    /// The stored **type** of one setting, read from the document rather than from
    /// the text `flat()` rendered. `Some` means the file really carries a boolean
    /// here, so a widget may offer a switch; `None` covers both "no such key" and
    /// "this key is text/number/array".
    ///
    /// Sniffing `get_str(key) == "true"` instead would draw a switch for any key
    /// whose *content* happens to be the word `true` — `annotation.font_family =
    /// "true"` is a legal file — and the click would then overwrite a text field.
    /// §5.20.2 asks the page to mark each row with the kind of hand it takes, and
    /// that decision has to live where the type lives.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        let parts: Vec<&str> = key.split('.').collect();
        let doc = self.to_document().ok()?;
        find_value(&doc, &parts)?.as_bool()
    }

    /// Write a dotted setting from the string a settings widget produced. The
    /// stored type decides how the text is parsed, so a bool never becomes the
    /// string `"true"`.
    ///
    /// The value then passes the same gate a file passes when it is read
    /// ([`Config::validate`]), and whatever that gate corrected comes back as the
    /// error-free part of the report: §5.20.2 wants the settings page to say「这条
    /// 被改成了 96」out loud instead of quietly disagreeing with what was typed, and
    /// a widget that could store 500 for a key the file can only carry as 96 would
    /// show one number until the next launch.
    pub fn set_str(&mut self, key: &str, raw: &str) -> Result<Vec<String>, ConfigError> {
        let parts: Vec<&str> = key.split('.').collect();
        if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
            return Err(ConfigError::UnknownKey(key.to_string()));
        }
        let mut doc = self.to_document()?;
        let old = find_value(&doc, &parts)
            .cloned()
            .ok_or_else(|| ConfigError::UnknownKey(key.to_string()))?;
        // Two maps hold text with a grammar of its own, and a widget that
        // accepts `nope` here would fail at registration time instead.
        let mut raw = raw;
        let canonical;
        // §5.19.2 那一枚主题的名字也要一条自己的落点：`canonical` 已经被热键那一格占了，
        // 两枚键在同一条 `set_str` 里各借各的串，谁也不顶掉谁。
        let theme_canonical;
        // §5.19.4 那枚工具栏落点同理，第三枚各借各的。
        let placement_canonical;
        if parts[0] == "hotkey"
            && parts[1] == "actions"
            && parts.len() == 3
            && !raw.trim().is_empty()
        {
            canonical = crate::hotkeys::parse_bindable(raw)
                .map_err(|e| ConfigError::InvalidValue {
                    key: key.to_string(),
                    reason: e.to_string(),
                })?
                .canonical();
            raw = canonical.as_str();
        }
        if parts[0] == "hot_corner" && parts[1] == "corners" && parts.len() == 3 {
            let text = raw.trim();
            if !text.is_empty() && !HOT_CORNER_ACTIONS.contains(&text) {
                return Err(ConfigError::InvalidValue {
                    key: key.to_string(),
                    reason: format!("{text} is not a bindable action"),
                });
            }
        }
        if parts.len() == 2 && parts[0] == "tray" {
            // §5.18.2 的四枚键都是闭集字符串。名单外就在这儿拒掉，而不是让页面上那一格文本框
            // 把 `nope` 存下来、等下一次开档时才变成一句关于整份档的抱怨 —— §5.18.2 的规则是
            // 一条会拦下的规则，拦在被打字的那一格上才有用。
            let text = raw.trim();
            if !TRAY_ACTIONS.contains(&text) {
                return Err(ConfigError::InvalidValue {
                    key: key.to_string(),
                    reason: format!("{text} is not a tray action ({})", TRAY_ACTIONS.join(", ")),
                });
            }
        }
        if parts.len() == 2 && parts[0] == "whiteboard" {
            // §5.4.4 与 §5.4.5 的两枚键各用各的读法，读的正是产品那两枚解析器本身 —— 闸与
            // 判据之间没有第二份名单，所以"页上收下、下一次开档才抱怨"这一族在此关死。
            let text = raw.trim();
            let why = match parts[1] {
                "background" => crate::whiteboard::Surface::parse(text)
                    .err()
                    .map(|e| e.to_string()),
                "size" => crate::whiteboard::Size::parse(text)
                    .err()
                    .map(|e| e.to_string()),
                _ => None,
            };
            if let Some(reason) = why {
                return Err(ConfigError::InvalidValue {
                    key: key.to_string(),
                    reason,
                });
            }
        }
        if parts.len() == 2 && parts[0] == "pin" && parts[1] == "alpha_bg" {
            // The stored type is an enum, so there is no correction the gate can
            // make here — but without this rung the typo surfaces at
            // `from_document` as a complaint about the whole document ("bad toml:
            // unknown variant …"), which is a message about a file the settings
            // page never touched rather than about the one box that was typed into
            // (§5.8.3 wants the second one).
            let text = raw.trim();
            if AlphaBg::from_id(text).is_none() {
                return Err(ConfigError::InvalidValue {
                    key: key.to_string(),
                    reason: format!(
                        "{text} is not a background mode ({})",
                        ALPHA_BG_IDS.join(", ")
                    ),
                });
            }
        }
        if parts.len() == 2 && parts[0] == "capture" && parts[1] == "size_unit" {
            // §5.3.7 第一步的那一格在设置页上是一行文本框（枚举键在页上都是文本框）。闸放在这儿
            // 而不是 `validate()` 里，是因为 `Unit` 是**存进档的类型**：拼错的名字到了
            // `from_document` 那里会变成一句关于整份文件的抱怨（"unknown variant …"），而不是
            // 关于用户刚打过字的那一格的回答 —— 与上面 `pin.alpha_bg` 那一条是同一枚理由。
            let text = raw.trim();
            match Unit::from_label(text) {
                // 认下了就按名单上的拼法存进档：` px ` 与 `px` 存的是同一枚字面。
                // 只把 trim 用在判断上、把原样交给 `coerce`，会被这一格自己的闸放行、
                // 再在下一行的 `from_document` 那里顶回来 —— 交回的是"unknown variant
                // ` px `"这样一句关于整份文件的抱怨，而用户刚打的明明是一个合法的单位。
                Some(unit) => raw = unit.label(),
                None => {
                    return Err(ConfigError::InvalidValue {
                        key: key.to_string(),
                        reason: format!("{text} is not a size unit ({})", Unit::labels_line()),
                    })
                }
            }
        }
        if parts.len() == 2 && parts[0] == "appearance" && parts[1] == "theme" {
            // §5.19.2 步骤 1 的那两枚手动主题（加 §5.19.1 那一枚「跟随系统」）是闭集，而它存
            // 的是**枚举**：拼错的字面会一路走到 `from_document`，在那里变成一句关于整份文件的
            // 抱怨（`unknown variant 'nope', expected one of …`）—— 与上面 `pin.alpha_bg` /
            // `capture.size_unit` 两格立过的同一块牌子。认下了按名单的拼法存（`Dark ` 与 `dark`
            // 是同一枚字面）。
            let text = raw.trim().to_ascii_lowercase();
            if !THEME_IDS.contains(&text.as_str()) {
                return Err(ConfigError::InvalidValue {
                    key: key.to_string(),
                    reason: format!("{raw} is not a theme ({})", THEME_IDS.join(", ")),
                });
            }
            theme_canonical = text;
            raw = theme_canonical.as_str();
        }
        if parts.len() == 2 && parts[0] == "appearance" && parts[1] == "toolbar_placement" {
            // §5.19.4「支持调整工具栏方向和位置」那一枚**闭集**。它是 String 而不是 enum（档里
            // 存的是字面），所以在 `from_document` 那一级读不出任何异常 —— 没有这一格闸，页上
            // 那枚文本框就能把 `sideways` 存下来，等下一次开档才变成一句关于整份文件的归一，
            // 而用户看着的是自己刚打进去的那串字。与 [`TRAY_ACTIONS`] / `whiteboard` 两格
            // 「String 闭集拦在打字那一格」同形。
            let text = raw.trim().to_ascii_lowercase();
            if !TOOLBAR_PLACEMENTS.contains(&text.as_str()) {
                return Err(ConfigError::InvalidValue {
                    key: key.to_string(),
                    reason: format!(
                        "{raw} is not a toolbar placement ({})",
                        TOOLBAR_PLACEMENTS.join(", ")
                    ),
                });
            }
            placement_canonical = text;
            raw = placement_canonical.as_str();
        }
        let next = coerce(&old, parts[parts.len() - 1], raw)?;
        let slot = find_value_mut(&mut doc, &parts)
            .ok_or_else(|| ConfigError::UnknownKey(key.to_string()))?;
        let decor = slot.decor().clone();
        *slot = next;
        slot.decor_mut().clone_from(&decor);
        *self = toml_edit::de::from_document::<Config>(doc)
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        Ok(self.validate())
    }

    /// §5.16.2 规则第二句「应提供当前前台程序快速添加功能」在**档**上的那一腿。
    ///
    /// [`Config::set_str`] 到这一枚键是走不通的，而那是对的：`hotkey.exclusions` 是一串表
    /// （`[[hotkey.exclusions]]`），`coerce` 的数组分支会把打进来的一串字按 `,` 拆成
    /// `["x"]` 那样根本不成规则的东西，最后在 `from_document` 那里变成一句关于整份文件的抱怨 ——
    /// core 那枚 `hotkey_exclusions_keep_their_shape` 钉的就是这一句 `is_err()`。所以数组表要
    /// 一枚**自己的门**，与 [`Config::set_roster`] 为 `[groups]` 那张顺序表开的同一枚：形状只在
    /// 这一处知道，页那边只递过来一个名字、听回来一句事实。
    ///
    /// 添进去的那一枚规则 `actions` 是**空的** —— 按 [`AppExclusion`] 那一格的读法那就是「这一位
    /// 前台跟前一枚全局快捷键也不响」，正是 §5.16.2 那一句「指定应用忽略快捷键」最短的一条。
    /// 「只忽略其中几枚」要的是动作那一列，而页上此刻还没有递它的格子（这笔债记在
    /// `qt_bridge::settings_view` 那一手头上），这里就不发明一个眼下没人填的参数。
    ///
    /// 存的是**可执行文件名**（`notepad.exe`）而不是整条路径：PRD 操作步骤 2 写的是「可执行文件名
    /// 或路径匹配规则」，而 [`crate::hotkeys::app_matches`] 拿模式同时去比全路径与文件名，名字那一
    /// 枚因此两边都接得住；它把「同一个程序装在两处」也算作同一位 —— 那是「快速添加」这一枚钮想要
    /// 的意思。路径那一头仍归人手工写进档，这一格不拦它（上面那个 `app_matches` 就是两边都认的
    /// 那位作者）。
    pub fn add_exclusion(&mut self, app: &str) -> Result<ExclusionAdd, ConfigError> {
        let name = app.trim();
        if name.is_empty() {
            // 与 `validate` 里那一句「dropped a rule with no app」是同一条判据，只是这一趟拦在写
            // **之前**：一条没有名字的规则加进档，`validate` 当场把它删掉，页上那句「已添加」就
            // 成了谎 —— 而这一档定的规矩是「门留下的那一枚值才算做成的事」（见 `Written::stored`）。
            return Err(ConfigError::InvalidValue {
                key: EXCLUSION_KEY.to_string(),
                reason: "没有名字，添进去是一条认不出谁的规则".to_string(),
            });
        }
        // 「已经在不在」用的是 core 自己那两枚判断（[`crate::hotkeys::app_matches`] 与那一格的
        // 空列读法），页上不第二次数：这一枚新规则管的是**每一枚**快捷键，所以只有同样管到底的
        // 那一枚才算已经在了 —— 一枚只管住 `capture` 的规则，挡不下「把这一位整个静音」。
        if let Some(rule) = self
            .hotkey
            .exclusions
            .iter()
            .find(|e| crate::hotkeys::app_matches(&e.app, name) && e.actions.is_empty())
        {
            return Ok(ExclusionAdd::Already {
                matched: rule.app.clone(),
            });
        }
        self.hotkey.exclusions.push(AppExclusion {
            app: name.to_string(),
            actions: Vec::new(),
        });
        Ok(ExclusionAdd::Added(self.validate()))
    }

    /// §5.12 那一族的**顺序表**读法：按 [`Groups::entries`] 的次序，一行一枚组，拆开就是名字与成员。
    ///
    /// 这是档的形状（一串 `组名=编号 编号`）与机器的形状（`PinSet` 里那一列 `Group`，见
    /// [`roster_of`]）之间唯一的翻译处，所以 [`Config::set_roster`]、[`Config::diff_groups`]、
    /// [`plan_group_sync`] 三手都读它 —— 两处各数一遍就是 §5.12.3 那一句「排序只有一份」的反面。
    /// 读不出的那一行（名字是空的）在这里就是不存在：[`Config::validate_groups`] 已经把它报过了。
    pub fn roster(&self) -> Vec<GroupRoster> {
        self.groups
            .entries
            .iter()
            .filter_map(|line| group_split(line))
            .map(|(name, members)| GroupRoster { name, members })
            .collect()
    }

    /// 把一份顺序表写回档的那一枚数组（排序与成员都在里面）。与 [`Config::roster`] 互为逆：
    /// `set_roster(&self.roster())` 不改任何东西 —— 往返判据那格单测钉的就是这一句。
    ///
    /// 写不出行的那几枚（名字带分隔符之类）原样**略过**而不硬塞一串坏字符：这一手唯一的调用者
    /// 是导出，而一份别人要拿去另一台机器的档里塞进一枚读不回的组，就是 [`Config::validate`]
    /// 那道闸早就拦下的那一件事。略过也要说得出（返回的就是被略过的枚名）。
    pub fn set_roster(&mut self, roster: &[GroupRoster]) -> Vec<String> {
        let mut skipped = Vec::new();
        self.groups.entries = Vec::new();
        for g in roster {
            match group_join(&g.name, &g.members) {
                Some(line) => self.groups.entries.push(line),
                None => skipped.push(g.name.clone()),
            }
        }
        skipped
    }

    /// §5.20.3 步骤 4 在分组这一族上的那一半：一份新档会把哪几组**盖掉、新增、删掉、挪序**。
    ///
    /// 为什么这一族不能跟着 `flat()` 那一枚一枚比：那只有「两边的同一格值不同」的手，对这一族
    /// 只会给出一行 `groups.entries = "甲=1 2, 乙=" → "甲=1 2"` —— 一整串字符，而 §5.20.3 第四句
    /// 要的是「显示将被覆盖的**项目**」：用户看得懂的应该是「乙 这一组要没了（它现在带着 1、2 枚
    /// 贴图）」这一句。逐键那一手也看不见次序变了而值没变的那种挪序。所以这一族自己开一张 diff，
    /// 一枚组一行（`groups.added.<名>` / `groups.removed.<名>` / `groups.members.<名>`，外加一行
    /// 排序 `groups.order`）。
    ///
    /// 这些键**只**是报出去的名字，不是档里真有的键（档里只有 `groups.entries` 一枚数组）。
    /// 所以 [`Config::apply_changes`] 认的是前缀 `groups.`，整格换一次，不拿它们去 `set_str`。
    ///
    /// 「哪一枚就是那一枚」用的是 [`match_groups`] —— 与 [`plan_group_sync`] 同一份配对。这一句
    /// 不是讲究：那份判断若写两遍，一份文件里写着 `Ref` 而机器上叫 `ref` 时，报的就会是
    /// 「删掉 ref、新建 Ref」而做的是「改名」，而 §5.20.3 步骤 5 那句「用户确认后应用」确认的
    /// 正是用户看见的那一列。
    pub fn diff_groups(&self, incoming: &Config) -> Vec<ImportChange> {
        let before = self.roster();
        let after = incoming.roster();
        let claim = match_groups(&before, &after);
        let names =
            |r: &[GroupRoster]| -> Vec<String> { r.iter().map(|g| g.name.clone()).collect() };
        let mut out = Vec::new();
        if names(&before) != names(&after) {
            out.push(ImportChange {
                key: "groups.order".into(),
                from: names(&before).join(", "),
                to: names(&after).join(", "),
                restart_required: false,
            });
        }
        for (ti, t) in after.iter().enumerate() {
            let Some(ci) = claim.iter().position(|c| *c == Some(ti)) else {
                out.push(ImportChange {
                    key: format!("groups.added.{}", t.name),
                    from: String::new(),
                    to: t.members.join(", "),
                    restart_required: false,
                });
                continue;
            };
            if before[ci].name != t.name {
                out.push(ImportChange {
                    key: format!("groups.renamed.{}", t.name),
                    from: before[ci].name.clone(),
                    to: t.name.clone(),
                    restart_required: false,
                });
            }
            if !same_ids(&before[ci].members, &t.members) {
                out.push(ImportChange {
                    key: format!("groups.members.{}", t.name),
                    from: before[ci].members.join(", "),
                    to: t.members.join(", "),
                    restart_required: false,
                });
            }
        }
        for (ci, c) in before.iter().enumerate() {
            if claim[ci].is_none() {
                out.push(ImportChange {
                    key: format!("groups.removed.{}", c.name),
                    from: c.members.join(", "),
                    to: String::new(),
                    restart_required: false,
                });
            }
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// §5.20.3 步骤 5：按**已经报给用户的那一列**去应用。
    ///
    /// 与 [`Config::apply_import`] 的分别是这一列由调用方递进来，而不是在这里重新算一次。为的是
    /// 步骤 4 与步骤 5 之间那一句「用户确认后应用」：用户确认的是他看见的那一列，而桥那一侧会在
    /// 中间把档里**没有的那一族**挑掉（[`missing_families`]：一份没写 `[groups]` 的档不该把本机的
    /// 分组清成空的）。在这里重新 preview 一遍就会把挑掉的那几行又加回来，报的与做的就不是
    /// 同一件事了。
    pub fn apply_changes(
        &mut self,
        incoming: &Config,
        changes: &[ImportChange],
    ) -> Result<Vec<String>, ConfigError> {
        let mut corrections: Vec<String> = Vec::new();
        let mut groups_applied = false;
        let mut exclusions_applied = false;
        for change in changes {
            // 这一族整格换，不逐键写：新的一组在档里还没有那一格，`set_str` 只会回一句
            // 「no such setting」（[`find_value_mut`] 只认档里已有的键），而被删掉的那一组的格子
            // 换掉之后还留着。两样都不是「应用了这一列」。
            if change.key.starts_with("groups.") {
                if !groups_applied {
                    self.groups = incoming.groups.clone();
                    groups_applied = true;
                    for warning in self.validate() {
                        if !corrections.contains(&warning) {
                            corrections.push(warning);
                        }
                    }
                }
                continue;
            }
            // §5.16.2 那一串忽略规则与上面 `groups.` 是同一形的一桩坏：**一串表**，而逐键那一臂
            // 递的是 `flat()` render 出来的**一串字**（`app=notepad.exe, actions=`）。`set_str` 把它
            // 交给 `coerce` 的数组分支按 `,` 拆成一枚字符串，再在 `from_document` 那里顶回来 ——
            // `invalid type: string "app=notepad.exe", expected struct AppExclusion`，一整趟导入
            // 就此 Err 中止。此前只有手改过 config.toml 的人才撞得到它，而 §5.16.2 规则第二句
            // 那枚「把当前前台程序加进来」的钮一上，产品自己就会写出这样一行，于是这一族必须
            // 整格换（与 [`Config::add_exclusion`] 同一位作者知道形状，#281）。
            if change.key == EXCLUSION_KEY {
                if !exclusions_applied {
                    self.hotkey.exclusions = incoming.hotkey.exclusions.clone();
                    exclusions_applied = true;
                    for warning in self.validate() {
                        if !corrections.contains(&warning) {
                            corrections.push(warning);
                        }
                    }
                }
                continue;
            }
            for warning in self.set_str(&change.key, &change.to)? {
                if !corrections.contains(&warning) {
                    corrections.push(warning);
                }
            }
        }
        Ok(corrections)
    }

    /// §5.18.2 那两条规则的**唯一**一处判断：四枚入口里必须至少有一枚能把人带到托盘菜单或
    /// 设置页。交出空表就是合规。
    ///
    /// `validate()` 拿它决定要不要强行留一扇窗，门拿它数违规，两边读的是同一句判断 —— 写两遍
    /// 就会有两份各自会漂的口径，而这两份一旦不一致，报出来的"合规"就没有证人。
    pub fn tray_violations(&self) -> Vec<String> {
        if self
            .tray
            .entries()
            .iter()
            .any(|(_, action)| TRAY_ENTRY_ACTIONS.contains(action))
        {
            return Vec::new();
        }
        let roster = self
            .tray
            .entries()
            .iter()
            .map(|(name, action)| format!("{name}={action}"))
            .collect::<Vec<_>>()
            .join(", ");
        vec![format!(
            "tray: none of the four entries ({roster}) opens the tray menu or the settings page"
        )]
    }

    /// §5.20.3 step 4: what the incoming file would overwrite, item by item.
    ///
    /// 分组那一族不走逐键比的那一臂（它要的是「次序变了」与「这一组要没了」两条，见
    /// [`Config::diff_groups`]），其余每一枚键还是两半 `flat()` 对着数。
    pub fn preview_import(&self, incoming: &Config, scope: ImportScope) -> Vec<ImportChange> {
        let before = self.flat().unwrap_or_default();
        let after = incoming.flat().unwrap_or_default();
        let mut out = Vec::new();
        for (key, to) in &after {
            if !scope.covers(key) {
                continue;
            }
            if key.starts_with("groups.") {
                continue;
            }
            let Some(from) = before.get(key) else {
                continue;
            };
            if from != to {
                out.push(ImportChange {
                    key: key.clone(),
                    from: from.clone(),
                    to: to.clone(),
                    restart_required: needs_restart(key),
                });
            }
        }
        if scope.covers("groups.entries") {
            out.extend(self.diff_groups(incoming));
        }
        out
    }

    /// §5.20.3 step 5.
    ///
    /// The corrections are collected across the whole apply, in order, without
    /// repeats: one write can make the gate say something about a pair of keys
    /// (`hotkey.actions.capture, toggle_pins: f13 is bound twice`), and every
    /// later write would otherwise repeat that line.
    ///
    /// 它就是「preview 那一列，然后原样应用」：本体在 [`Config::apply_changes`] 里，这里只是把
    /// 两半接起来。要在步骤 4 与步骤 5 之间对那一列动手（比如把档里压根没写的那一族挑掉），
    /// 走 `apply_changes` 递那一列进去，不要重算 preview —— 用户确认的是他看见的那一列。
    pub fn apply_import(
        &mut self,
        incoming: &Config,
        scope: ImportScope,
    ) -> Result<Vec<String>, ConfigError> {
        let changes = self.preview_import(incoming, scope);
        self.apply_changes(incoming, &changes)
    }

    /// Clamp and correct unusable values, reporting each change so the settings
    /// page can say so instead of silently disagreeing (§8.3).
    pub fn validate(&mut self) -> Vec<String> {
        let mut w = Vec::new();
        clamp(
            &mut w,
            "capture.magnifier_radius",
            &mut self.capture.magnifier_radius,
            1,
            64,
        );
        clamp(
            &mut w,
            "capture.magnifier_zoom",
            &mut self.capture.magnifier_zoom,
            1,
            64,
        );
        clamp(
            &mut w,
            "capture.delay_seconds",
            &mut self.capture.delay_seconds,
            DELAY_SECONDS_MIN,
            DELAY_SECONDS_MAX,
        );
        // §5.5.6 步骤 3 的那枚半径。0 = 「用系统窗口原始圆角」（见
        // [`Capture::window_corner_radius`] 的 doc），上界与 [`Output::corner_radius`] 同一条 64，
        // 理由是同一件事不该有两副界：窗口截图切的圆角与导出图片切的圆角，一种数一种夹法。
        //
        // 这一族**没有** `window_shadow_margin` 那一枚键，是有意少写的：阴影留多宽已经有人说过
        // （[`Output::shadow_size`]，§5.5.7 的「边距」），再落一枚同义的键就是让两格各说一面。
        clamp(
            &mut w,
            "capture.window_corner_radius",
            &mut self.capture.window_corner_radius,
            0,
            64,
        );
        clamp(
            &mut w,
            "pin.default_zoom",
            &mut self.pin.default_zoom,
            10,
            800,
        );
        clamp(
            &mut w,
            "pin.default_opacity",
            &mut self.pin.default_opacity,
            10,
            100,
        );
        clamp(
            &mut w,
            "pin.solo_dim_opacity",
            &mut self.pin.solo_dim_opacity,
            10,
            100,
        );
        clamp(
            &mut w,
            "pin.state_save_seconds",
            &mut self.pin.state_save_seconds,
            0,
            3600,
        );
        // §5.9.10 预设盒子的一边。下界是 1 而不是某个"看起来像缩略图"的数：0 会让
        // [`crate::pin::PinItem::enter_fixed_thumbnail`] 退回默认盒子，那等于让用户
        // 写下 0 却拿到 240，静默且不可解；夹成 1 就把那个 0 变成一根线，诚实。上界与
        // `card.width` 同一条 4096，因为一个比桌面还宽的"缩略图"盒子同样没有意义。
        clamp(
            &mut w,
            "pin.thumb_fixed_width",
            &mut self.pin.thumb_fixed_width,
            1,
            4096,
        );
        clamp(
            &mut w,
            "pin.thumb_fixed_height",
            &mut self.pin.thumb_fixed_height,
            1,
            4096,
        );
        clamp(
            &mut w,
            "appearance.ui_font_size",
            &mut self.appearance.ui_font_size,
            UI_FONT_MIN,
            UI_FONT_MAX,
        );
        clamp(
            &mut w,
            "appearance.magnifier_border",
            &mut self.appearance.magnifier_border,
            0,
            8,
        );
        clamp(
            &mut w,
            "appearance.magnifier_corner_radius",
            &mut self.appearance.magnifier_corner_radius,
            0,
            48,
        );
        clamp(
            &mut w,
            "hot_corner.trigger_dip",
            &mut self.hot_corner.trigger_dip,
            1,
            100,
        );
        clamp(
            &mut w,
            "hot_corner.dwell_ms",
            &mut self.hot_corner.dwell_ms,
            0,
            5_000,
        );
        clamp(
            &mut w,
            "hot_corner.repeat_guard_ms",
            &mut self.hot_corner.repeat_guard_ms,
            100,
            10_000,
        );
        clamp(
            &mut w,
            "output.jpeg_quality",
            &mut self.output.jpeg_quality,
            1,
            100,
        );
        clamp(
            &mut w,
            "output.corner_radius",
            &mut self.output.corner_radius,
            0,
            64,
        );
        clamp(
            &mut w,
            "output.shadow_size",
            &mut self.output.shadow_size,
            0,
            96,
        );
        clamp(
            &mut w,
            "output.border_width",
            &mut self.output.border_width,
            0,
            64,
        );
        clamp(
            &mut w,
            "output.shadow_strength",
            &mut self.output.shadow_strength,
            0,
            100,
        );
        clamp(
            &mut w,
            "annotation.stroke_width",
            &mut self.annotation.stroke_width,
            1,
            64,
        );
        clamp(
            &mut w,
            "annotation.font_size",
            &mut self.annotation.font_size,
            FONT_MIN,
            FONT_MAX,
        );
        // The card's font size is clamped with the text tool's own two constants, so the
        // file keeps one font-size range rather than two that can disagree. The width's
        // floor is the padding's: below 160 physical pixels nothing is left to wrap into.
        clamp(
            &mut w,
            "card.font_size",
            &mut self.card.font_size,
            FONT_MIN,
            FONT_MAX,
        );
        clamp(&mut w, "card.width", &mut self.card.width, 160, 4096);
        clamp(
            &mut w,
            "annotation.blur_radius",
            &mut self.annotation.blur_radius,
            1,
            96,
        );
        clamp(
            &mut w,
            "annotation.undo_merge_ms",
            &mut self.annotation.undo_merge_ms,
            0,
            5000,
        );
        clamp(
            &mut w,
            "history.max_items",
            &mut self.history.max_items,
            0,
            100_000,
        );
        clamp(
            &mut w,
            "history.max_age_days",
            &mut self.history.max_age_days,
            0,
            3650,
        );
        clamp(
            &mut w,
            "history.max_size_mb",
            &mut self.history.max_size_mb,
            0,
            1_048_576,
        );
        clamp(
            &mut w,
            "history.thumb_px",
            &mut self.history.thumb_px,
            32,
            1024,
        );
        clamp(
            &mut w,
            "about.check_every_days",
            &mut self.about.check_every_days,
            1,
            365,
        );
        clamp_usize(
            &mut w,
            "annotation.undo_limit",
            &mut self.annotation.undo_limit,
            10,
            5000,
        );

        for (key, value, fallback) in [
            (
                "annotation.stroke_color",
                &mut self.annotation.stroke_color,
                "#E81123",
            ),
            (
                "annotation.fill_color",
                &mut self.annotation.fill_color,
                "#00000000",
            ),
            (
                "appearance.accent",
                &mut self.appearance.accent,
                ACCENT_DEFAULT,
            ),
            (
                "appearance.magnifier_foreground",
                &mut self.appearance.magnifier_foreground,
                "#FFFFFF",
            ),
            (
                "appearance.magnifier_background",
                &mut self.appearance.magnifier_background,
                "#1F1F1F",
            ),
            (
                "output.border_color",
                &mut self.output.border_color,
                "#000000",
            ),
            ("card.background", &mut self.card.background, "#FFFFFF"),
        ] {
            if crate::colors::parse(value).is_none() {
                w.push(format!("{key}: {value} is not a colour, using {fallback}"));
                *value = fallback.to_string();
            }
        }
        // `output.corner_fill` is the one colour this file lets be blank: §5.5.6's rule
        // reads 圆角外部应透明 **或** 按配置填充, so "" is a value and not a missing one.
        // A non-empty one nobody can read still has to be said out loud and cleared — the
        // alternative is a user who typed a fill, saw no warning, and gets transparency.
        if !self.output.corner_fill.is_empty()
            && crate::colors::parse(&self.output.corner_fill).is_none()
        {
            w.push(format!(
                "output.corner_fill: {} is not a colour, the corners stay transparent",
                self.output.corner_fill
            ));
            self.output.corner_fill.clear();
        }
        let palette = std::mem::take(&mut self.annotation.palette);
        for c in palette {
            if crate::colors::parse(&c).is_some() {
                self.annotation.palette.push(c);
            } else {
                w.push(format!("annotation.palette: dropped {c}"));
            }
        }
        if self.annotation.palette.is_empty() {
            self.annotation.palette = default_palette();
        }
        // §5.7.22 step 3. Same rule as the band above: a value this gate cannot read is reported
        // and dropped, never guessed at — a scheme the file invented a name for is the one thing
        // a user saving their own colours does not expect to find on the page later. A scheme
        // keeps the colours it can read (a bad hex says nothing about the six good ones beside
        // it), and an entry with nothing left, no name, or a name already taken goes.
        let saved: Vec<String> = std::mem::take(&mut self.annotation.palette_schemes);
        let mut taken: Vec<String> = Vec::new();
        for entry in saved {
            let Some((name, colors)) = scheme_split(&entry) else {
                w.push(format!(
                    "annotation.palette_schemes: {entry} is not `name=colour colour`, dropped"
                ));
                continue;
            };
            let mut kept: Vec<&str> = Vec::new();
            for c in &colors {
                if crate::colors::parse(c).is_some() {
                    kept.push(c);
                } else {
                    w.push(format!("annotation.palette_schemes.{name}: dropped {c}"));
                }
            }
            if kept.is_empty() {
                w.push(format!(
                    "annotation.palette_schemes.{name}: no colour left, dropped"
                ));
                continue;
            }
            if taken.iter().any(|n| n == name) {
                w.push(format!(
                    "annotation.palette_schemes.{name}: a second scheme of that name, dropped"
                ));
                continue;
            }
            // The kept list goes back out through the same writer the page uses, so an entry that
            // the reader would break apart again never reaches the file: a name holding a
            // separator, or a colour holding one, is dropped here rather than stored as two.
            let kept: Vec<String> = kept.into_iter().map(str::to_string).collect();
            match scheme_join(name, &kept) {
                Some(entry) => {
                    taken.push(name.to_string());
                    self.annotation.palette_schemes.push(entry);
                }
                None => w.push(format!(
                    "annotation.palette_schemes.{name}: a separator inside it, dropped"
                )),
            }
        }

        if !self.annotation.default_tool.is_empty()
            && !ANNOTATION_TOOLS.contains(&self.annotation.default_tool.as_str())
        {
            w.push(format!(
                "annotation.default_tool: unknown {:?}, cleared",
                self.annotation.default_tool
            ));
            self.annotation.default_tool.clear();
        }
        // §5.7.1's memory is keyed by the tool's own spelling, so a key that names no
        // tool is reported and dropped: guessing at a typo would hand one tool another
        // tool's colour. An unreadable value loses that one field, not the entry — a
        // bad colour says nothing about the line width written beside it.
        let remembered: Vec<(String, ToolStyle)> = std::mem::take(&mut self.annotation.tool_style)
            .into_iter()
            .collect();
        for (key, style) in remembered {
            let Some(kind) = Kind::from_key(&key) else {
                w.push(format!(
                    "annotation.tool_style.{key}: no such tool, dropped"
                ));
                continue;
            };
            let mut kept = style;
            if let Some(color) = &kept.color {
                if crate::colors::parse(color).is_none() {
                    w.push(format!(
                        "annotation.tool_style.{key}.color: {color} is not a colour, forgotten"
                    ));
                    kept.color = None;
                }
            }
            if let Some(width) = kept.width {
                let mut width = width;
                clamp(
                    &mut w,
                    &format!("annotation.tool_style.{key}.width"),
                    &mut width,
                    1,
                    64,
                );
                kept.width = Some(width);
            }
            if let Some(intensity) = kept.intensity {
                if !kind.has_effect() {
                    // A strength on a tool that has none is a number nothing reads, and
                    // the file would keep saying it after the toolbar stopped showing
                    // it. Reported rather than quietly moved somewhere else.
                    w.push(format!(
                        "annotation.tool_style.{key}.intensity: only 马赛克 and 模糊 have a strength, forgotten"
                    ));
                    kept.intensity = None;
                } else {
                    let mut intensity = intensity;
                    clamp(
                        &mut w,
                        &format!("annotation.tool_style.{key}.intensity"),
                        &mut intensity,
                        EFFECT_MIN,
                        EFFECT_MAX,
                    );
                    kept.intensity = Some(intensity);
                }
            }
            self.annotation.tool_style.insert(key, kept);
        }
        if !matches!(
            self.annotation.arrow_style.as_str(),
            "straight" | "elbow" | "curve"
        ) {
            w.push(format!(
                "annotation.arrow_style: unknown {:?}, using straight",
                self.annotation.arrow_style
            ));
            self.annotation.arrow_style = "straight".into();
        }
        if !TOOLBAR_PLACEMENTS.contains(&self.appearance.toolbar_placement.as_str()) {
            w.push(format!(
                "appearance.toolbar_placement: unknown {:?}, using {TOOLBAR_PLACEMENT_DEFAULT}",
                self.appearance.toolbar_placement
            ));
            self.appearance.toolbar_placement = TOOLBAR_PLACEMENT_DEFAULT.into();
        }
        // §5.19.4「支持调整界面字体和字号」里字体那一半。空串与只有空白的串不是一枚字体名：
        // 界面拿它会退回到 Qt 自己的那一套（每台机器不一样），于是「用户把字体清掉了」与
        // 「用户没动过」在屏幕上成了两件事。**夹回出厂那一枚并说出来**，与
        // `output.naming_template` 那一格空串的处理同一条路。
        if self.appearance.ui_font_family.trim().is_empty() {
            w.push(format!(
                "appearance.ui_font_family: empty, using {}",
                UI_FONT_FAMILY_DEFAULT
            ));
            self.appearance.ui_font_family = UI_FONT_FAMILY_DEFAULT.into();
        }
        if !crate::update::CHANNELS.contains(&self.about.channel.as_str()) {
            w.push(format!(
                "about.channel: unknown {:?}, using stable",
                self.about.channel
            ));
            self.about.channel = "stable".into();
        }
        if !matches!(
            self.advanced.log_level.as_str(),
            "off" | "error" | "warn" | "info" | "debug" | "trace"
        ) {
            w.push(format!(
                "advanced.log_level: unknown {:?}, using info",
                self.advanced.log_level
            ));
            self.advanced.log_level = "info".into();
        }
        if crate::colors::ColorFormat::from_name(&self.capture.magnifier_color_format).is_none() {
            w.push(format!(
                "capture.magnifier_color_format: unknown {:?}, using hex_upper",
                self.capture.magnifier_color_format
            ));
            self.capture.magnifier_color_format = "hex_upper".into();
        }
        if self.output.naming_template.is_empty() {
            w.push("output.naming_template: empty, using the default".to_string());
            self.output.naming_template = crate::naming::DEFAULT_TEMPLATE.into();
        } else if let Some(bad) = unknown_token(&self.output.naming_template) {
            w.push(format!(
                "output.naming_template: unknown %{} in {:?}, using the default",
                bad, self.output.naming_template
            ));
            self.output.naming_template = crate::naming::DEFAULT_TEMPLATE.into();
        }
        for (field, dir) in [
            ("output.save_dir", &mut self.output.save_dir),
            ("advanced.data_dir", &mut self.advanced.data_dir),
        ] {
            if !dir.is_empty() && !Path::new(dir.as_str()).is_absolute() {
                w.push(format!("{field}: {dir} must be absolute, ignored"));
                dir.clear();
            }
        }

        // A spec the grammar rejects is unbound rather than kept, because a
        // string that never registers is worse than no binding: the user thinks
        // the key works. Valid text is canonicalised so `CTRL+SHIFT+A` and
        // `shift+ctrl+a` stop being two different bindings.
        let specs: Vec<(String, String)> = self
            .hotkey
            .actions
            .iter()
            .map(|(id, spec)| (id.clone(), spec.clone()))
            .collect();
        for (id, spec) in specs {
            if spec.trim().is_empty() {
                continue;
            }
            match crate::hotkeys::parse_bindable(&spec) {
                Ok(parsed) => {
                    let canonical = parsed.canonical();
                    if canonical != spec {
                        w.push(format!(
                            "hotkey.actions.{id}: written {spec:?}, stored {canonical:?}"
                        ));
                        self.hotkey.actions.insert(id, canonical.clone());
                    }
                }
                Err(e) => {
                    w.push(format!("hotkey.actions.{id}: {e}, cleared"));
                    self.hotkey.actions.insert(id, String::new());
                }
            }
        }
        // §8.4: report a clash, never pick a winner by rewriting the file.
        for conflict in crate::hotkeys::conflicts(&self.hotkey.actions) {
            match conflict.reserved {
                Some(why) => w.push(format!(
                    "hotkey.actions.{}: {} is reserved by Windows ({why})",
                    conflict.actions.join(", "),
                    conflict.display
                )),
                None => w.push(format!(
                    "hotkey.actions.{}: {} is bound twice",
                    conflict.actions.join(", "),
                    conflict.display
                )),
            }
        }
        self.hotkey.exclusions.retain(|e| {
            let ok = !e.app.trim().is_empty();
            if !ok {
                w.push("hotkey.exclusions: dropped a rule with no app".to_string());
            }
            ok
        });

        let bad_corners: Vec<(String, String)> = self
            .hot_corner
            .corners
            .iter()
            .filter(|(_, action)| {
                !action.is_empty() && !HOT_CORNER_ACTIONS.contains(&action.as_str())
            })
            .map(|(corner, action)| (corner.clone(), action.clone()))
            .collect();
        for (corner, action) in bad_corners {
            w.push(format!(
                "hot_corner.corners.{corner}: unknown action {action:?}, cleared"
            ));
            self.hot_corner.corners.insert(corner, String::new());
        }

        // §5.18.2：先按名单归一，再问"至少留一扇能进菜单或设置页的入口"。这个顺序要紧 ——
        // 名单外的值先落回本键默认，`tray_violations` 才不会拿着一串垃圾去判"没有一枚能打开
        // 菜单"，报出来的原因才是真的那一个。
        let tray_bad: Vec<(&'static str, String)> = self
            .tray
            .entries()
            .iter()
            .filter(|(_, action)| !TRAY_ACTIONS.contains(action))
            .map(|(name, action)| (*name, action.to_string()))
            .collect();
        for (name, action) in tray_bad {
            let fallback = Tray::default_entry(name);
            w.push(format!(
                "tray.{name}: {action:?} is not a tray action, using {fallback}"
            ));
            self.tray.set_entry(name, fallback);
        }
        // 两条例是同一枚判据的两面，所以这里只问一次；违规就强行把右键落回 `menu`，
        // 因为"没有一扇窗进得去"是一台会用不起来的机器，不是用户可以带着跑的配置。
        for reason in self.tray_violations() {
            w.push(format!("{reason}; tray.right falls back to \"menu\""));
            self.tray.set_entry("right", "menu");
        }
        // §5.4.4 / §5.4.5：两枚键都是"读不出就得落回"的字符串。落回默认而不是猜用户想写的
        // 是哪一种颜色 —— 一页档里存着 `beige` 这样的名字时，任何猜测都是替用户发明一份名单。
        for (name, reason) in self.whiteboard.violations() {
            let fallback = Whiteboard::default_for(name).expect("both keys are listed");
            w.push(format!("whiteboard.{name}: {reason}, using {fallback}"));
            match name {
                "background" => self.whiteboard.background = fallback.to_string(),
                "size" => self.whiteboard.size = fallback.to_string(),
                _ => {}
            }
        }
        self.validate_groups(&mut w);
        w
    }

    /// §5.12 那一族的闸：把 [`Groups::entries`] 那一串行修成一份说得出口的顺序表，每一处修正
    /// 都留一句在账上（§8.3「不得静默」）。
    ///
    /// 一行过三道，按行的次序（先到的名字赢，与 `PinSet::create_group` 拒第二枚重名是同一份
    /// 口径 —— #281：判断住在机器那一侧，这里只是别再存一份说不出口的）：
    /// ① 名字：空的、带分隔符的、大小写重名的、过了 [`GROUPS_MAX`] 的，一律**丢下并报出去**；
    /// ② 成员：只留读得出编号的那几枚（贴图编号是 `u64`，把 `red` 留在档里就是留下一枚必然的
    ///    失败），去重、过 [`GROUP_MEMBERS_MAX`]；
    /// ③ 整行按 [`group_join`] 重新写一遍。归一在闸这里做一次，而不是每次读都偷偷不一样 ——
    ///    往返判据（导出→导入→再导出**逐字节**相同）要的就是这一句：人手敲的 ` 甲 = 1  2 ` 与
    ///    导出的 `甲=1 2` 说的同一件事，存下来必须也是同一串字节。
    fn validate_groups(&mut self, w: &mut Vec<String>) {
        let raw = std::mem::take(&mut self.groups.entries);
        let mut kept: Vec<String> = Vec::new();
        let mut dropped: Vec<String> = Vec::new();
        for line in raw {
            let Some((name, members)) = group_split(&line) else {
                if !line.trim().is_empty() {
                    dropped.push(format!("{line}（读不出组名）"));
                }
                continue;
            };
            if let Some(reason) = group_name_bad(&name) {
                dropped.push(format!("{name}（{reason}）"));
                continue;
            }
            if kept.iter().any(|k| {
                k.split_once('=')
                    .map(|(n, _)| n)
                    .unwrap_or(k.as_str())
                    .eq_ignore_ascii_case(&name)
            }) {
                dropped.push(name);
                continue;
            }
            if kept.len() >= GROUPS_MAX {
                dropped.push(name);
                continue;
            }
            let mut ok: Vec<String> = Vec::new();
            let mut bad: Vec<String> = Vec::new();
            let mut over = 0usize;
            for m in members {
                if m.parse::<u64>().is_err() {
                    if !bad.contains(&m) {
                        bad.push(m);
                    }
                } else if !ok.contains(&m) {
                    if ok.len() >= GROUP_MEMBERS_MAX {
                        over += 1;
                    } else {
                        ok.push(m);
                    }
                }
            }
            if !bad.is_empty() {
                w.push(format!(
                    "groups.members.{name}: 丢下 {} 枚读不出编号的成员（{}）—— 成员是贴图编号",
                    bad.len(),
                    bad.join(" ")
                ));
            }
            if over > 0 {
                w.push(format!(
                    "groups.members.{name}: 一组最多 {GROUP_MEMBERS_MAX} 枚，丢下后面 {over} 枚"
                ));
            }
            let fresh = group_join(&name, &ok).expect("name passed the gate, members are numbers");
            if fresh != line {
                w.push(format!("groups: {line:?} 存的是 {fresh:?}"));
            }
            kept.push(fresh);
        }
        if !dropped.is_empty() {
            w.push(format!(
                "groups: 丢下 {} 枚组（{}）—— 只有第一枚留下",
                dropped.len(),
                dropped.join(", ")
            ));
        }
        self.groups.entries = kept;
    }
}

fn clamp(w: &mut Vec<String>, key: &str, v: &mut u32, lo: u32, hi: u32) {
    let c = (*v).clamp(lo, hi);
    if c != *v {
        w.push(format!("{key}: {v} is out of range, using {c}"));
        *v = c;
    }
}

fn clamp_usize(w: &mut Vec<String>, key: &str, v: &mut usize, lo: usize, hi: usize) {
    let c = (*v).clamp(lo, hi);
    if c != *v {
        w.push(format!("{key}: {v} is out of range, using {c}"));
        *v = c;
    }
}

/// The first `%x` the naming engine would emit verbatim, i.e. a typo.
fn unknown_token(template: &str) -> Option<char> {
    let mut it = template.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            continue;
        }
        match it.next() {
            None => return Some('%'),
            Some('%') => {}
            Some('Y' | 'y' | 'm' | 'd' | 'H' | 'M' | 'S' | 'p' | 'i' | 'w' | 'h' | 't' | 'B') => {}
            Some(other) => return Some(other),
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportChange {
    pub key: String,
    pub from: String,
    pub to: String,
    pub restart_required: bool,
}

/// §5.20.3 的范围建议就是 [`ImportScope::Suggested`] 那一份名单；`All` 是明说的逃生口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportScope {
    /// §5.20.3「范围建议」那六枚：快捷键、标注样式、调色板、主题、输出目录和命名规则、**分组配置**。
    ///
    /// 分组那一族走的是**同一趟**管线，没有另开一条路：它就是 [`Config`] 上的一格
    /// [`Config::groups`]，所以 `flat()`、`preview_import`、`apply_import`、`export_text`
    /// 那几手一件也不必改口径。它在 [`Config::preview_import`] 里只是换了**一张 diff**
    /// （[`Config::diff_groups`]，理由写在那一手的 doc 上），不是换了范围。
    ///
    /// 这一格的话从前写的是「分组不带，因为分组活在贴图档里而不在 config.toml 里」——
    /// 那一句对着 §5.20.3 的第六枚就是不合格，而「带不过去」与「带得过去但没人写」是两件事：
    /// 今天 [`Config::groups`] 由导出填、由 [`sync_store_groups`] 落回机器。
    Suggested,
    All,
}

const SUGGESTED_PREFIXES: &[&str] = &[
    "hotkey.",
    "annotation.",
    "appearance.",
    "pin.",
    "groups.",
    "output.save_dir",
    "output.naming_template",
    "output.naming_prefix",
    "output.collision",
    "output.format",
];

impl ImportScope {
    pub fn covers(self, key: &str) -> bool {
        match self {
            ImportScope::All => true,
            ImportScope::Suggested => SUGGESTED_PREFIXES
                .iter()
                .any(|p| key.starts_with(p) || key == p.trim_end_matches('.')),
        }
    }
}

/// 本机这一份档的形状认到第几版。
///
/// **今天 `Config` 里没有这一枚字段**，而且不该硬加：加一枚 `general.config_version` 之类的键，
/// 先撞上设置页那条「每一枚键恰好属于一个类」（`every_default_key_belongs_to_exactly_one_category`），
/// 换来的只是页上一行没人看得懂的数字。所以这一枚常数是**读的那一只手**：别的机器（或未来版本的
/// 自己）递过来的档若自带 `version`，比这个数新就不导入 —— 与贴图档
/// [`crate::pin::SNAPSHOT_VERSION`] 那一条「不许拿新档喂旧程序」是同一枚规矩，而那一条早就立在
/// `PinStore` 的加载路径上（「没有这一句，一份新档会被旧程序当旧档读」）；这一枚只是别让它
/// 只有一处证人。自己写的档不带版本行，所以「没有版本行」= 按本机的 {CONFIG_VERSION} 理解。
pub const CONFIG_VERSION: u64 = 1;

/// 档里可能出现的族名 = [`Config`] 的字段名，一一对着 TOML 的顶层表。
///
/// 它存在的理由只有一个：[`inspect_file`] 要问得出「这份档**没写**哪一族」，而反序列化问不出 ——
/// 缺的那一节一律落成默认值，与「写了而每一枚都恰好是默认值」在 `Config` 上完全同色。这一份名单
/// 是本机 schema 的名单，不是设置页的类别表（那是 `qt_bridge::settings_view::CATEGORIES`，
/// 它有「贴图 = pin + card + groups」这种合并，单位与这里不同）。
pub const FAMILIES: &[&str] = &[
    "general",
    "capture",
    "whiteboard",
    "annotation",
    "pin",
    "card",
    "groups",
    "output",
    "hotkey",
    "tray",
    "hot_corner",
    "appearance",
    "history",
    "advanced",
    "about",
];

/// §5.20.3 步骤 4「系统**校验文件**并显示将被覆盖的项目」里前那半句的账。
///
/// 三样各一格，因为它们要办的是三件不同的事：
/// * [`FileReport::blocking`] —— 这一份档**不能**拿来导入（版本比本机新）。桥那一侧要挡住
///   步骤 5，并把这一句原样显示出来；
/// * [`FileReport::missing`] —— 这一族档里压根没有。导入**不动**它们（[`prune_missing`]），
///   而这一句必须显示出来：否则「我导了一份只写热键的档」在用户眼里就成了「什么都没变」，
///   与「我把没写的族都清了」是两种截然不同的期待；
/// * [`FileReport::warnings`] —— 读了而要说一句的：值被闸归一过的（[`Config::parse_import`]
///   带回来的那一段）、档里这台机器认不得的族、`version` 写成不是整数的。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileReport {
    pub blocking: Vec<String>,
    pub missing: Vec<String>,
    pub warnings: Vec<String>,
}

impl FileReport {
    /// 这份档挡不挡得住导入。
    pub fn refused(&self) -> bool {
        !self.blocking.is_empty()
    }

    /// 校验文件那一句要说的一切，按「先挡住、再没写、再提醒」的次序。
    ///
    /// 它住在 core 是因为这一句有两个读者（命令行 `--import-config` 与设置页那一格），而两处
    /// 各拼一遍中文就会有一句在骗另一句（#281）。
    pub fn says(&self) -> Vec<String> {
        let mut out = self.blocking.clone();
        if !self.missing.is_empty() {
            out.push(format!(
                "档里没写这些族，导入不动它们：{}",
                self.missing.join(", ")
            ));
        }
        out.extend(self.warnings.iter().cloned());
        out
    }
}

/// §5.20.3 步骤 4 那一问「这文件校不校得过」的唯一一手：读、校验、归一，带回一份档与一份账。
///
/// 它就是 [`Config::parse_import`] 加上一遍**顶层表**的检查。为什么不能只靠 `parse_import`：
/// 反序列化对「档里没写 `[groups]`」与「写了而空的」给的是同一个 `Config`，而这两件事在导入时
/// 是两种做法 —— 前者**不许**把本机的分组清成空的（§5.20.3 那句「范围建议」的另一面就是「没在
/// 范围内的原样保留」）。要问出这一句只能问那张表本身，而那张表只有 `DocumentMut` 看得见。
///
/// `Err` = 连读也读不成（坏文件），这一句由调用方原样显示，绝不回落成「那就按默认的导」。
pub fn inspect_file(text: &str) -> Result<(Config, FileReport), ConfigError> {
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| ConfigError::Parse(e.to_string()))?;
    let (cfg, normalized) = Config::parse_import(text)?;
    let mut rep = FileReport {
        warnings: normalized,
        ..Default::default()
    };

    // 版本号：带了就得读得出，且不许比本机新。
    match doc.get("version") {
        None => {}
        Some(Item::Value(v)) => match v.as_integer() {
            Some(n) if n < 0 => rep
                .blocking
                .push("档里的 version 是负数，这份档读不成就先不读".to_string()),
            Some(n) if (n as u64) > CONFIG_VERSION => rep.blocking.push(format!(
                "这份档写着 version = {n}，这台机器只认到 {CONFIG_VERSION}：先升级 Falconshot，再导入"
            )),
            Some(_) => {}
            None => rep.warnings.push(format!(
                "档里的 version 是 {}，不是版本号 ⇒ 按「没有版本行」处理",
                render(v)
            )),
        },
        Some(_) => rep.warnings.push(
            "档里的 version 是一节表而不是一个数 ⇒ 按「没有版本行」处理".to_string(),
        ),
    }

    // 没写的族 + 本机认不得的族。
    for fam in FAMILIES {
        if doc.get(*fam).is_none() {
            rep.missing.push((*fam).to_string());
        }
    }
    for (key, item) in doc.as_table().iter() {
        if key == "version" || FAMILIES.contains(&key) {
            continue;
        }
        let rows = match item {
            Item::Table(t) => t.iter().count(),
            Item::Value(_) => 1,
            _ => 0,
        };
        rep.warnings.push(format!(
            "档里这一族这台机器认不得：{key}（{rows} 行，原样丢下）"
        ));
    }
    Ok((cfg, rep))
}

/// 步骤 4 与步骤 5 之间那一句「档里没写的族不动」：把落在那些族上的行挑下来。
///
/// 纯函数，返回 `(留下的, 挑下来的)` 两半 —— 挑下来的那几行**不是**丢掉的垃圾，桥那一侧要把它们
/// 也显示出来（「这一族档里没写，所以『甲』这一组原样留着」），否则用户看见的那一列与他确认的
/// 那一列就不是同一件事。而 [`Config::apply_changes`] 拿的是这里留下的那一列，于是「报的」与
/// 「做的」同一份名单（#281）。
pub fn prune_missing(
    changes: &[ImportChange],
    missing: &[String],
) -> (Vec<ImportChange>, Vec<ImportChange>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for c in changes {
        let family = c.key.split('.').next().unwrap_or(c.key.as_str());
        if missing.iter().any(|m| m == family) {
            dropped.push(c.clone());
        } else {
            kept.push(c.clone());
        }
    }
    (kept, dropped)
}

fn flatten(tbl: &Table, prefix: &str, out: &mut BTreeMap<String, String>) {
    for (key, item) in tbl.iter() {
        let path = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        match item {
            Item::Table(t) => flatten(t, &path, out),
            Item::Value(Value::InlineTable(t)) => flatten_inline(t, &path, out),
            Item::Value(v) => {
                out.insert(path, render(v));
            }
            _ => {}
        }
    }
}

fn flatten_inline(tbl: &InlineTable, prefix: &str, out: &mut BTreeMap<String, String>) {
    for (key, value) in tbl.iter() {
        let path = format!("{prefix}.{key}");
        match value {
            Value::InlineTable(t) => flatten_inline(t, &path, out),
            v => {
                out.insert(path, render(v));
            }
        }
    }
}

fn render(v: &Value) -> String {
    match v {
        Value::String(s) => s.value().to_string(),
        Value::Integer(i) => i.value().to_string(),
        Value::Float(f) => f.value().to_string(),
        Value::Boolean(b) => b.value().to_string(),
        Value::Datetime(d) => d.value().to_string(),
        Value::Array(a) => a.iter().map(render).collect::<Vec<_>>().join(", "),
        Value::InlineTable(t) => {
            let mut parts = Vec::new();
            for (k, v) in t.iter() {
                parts.push(format!("{k}={}", render(v)));
            }
            parts.join(", ")
        }
    }
}

/// Walk `section.[sub.]key`. A map such as `[hotkey.actions]` serialises as an
/// inline table, whose leaves are values rather than items. Paths in this
/// schema are never deeper than three segments.
fn find_value<'a>(doc: &'a DocumentMut, parts: &[&str]) -> Option<&'a Value> {
    if parts.len() < 2 {
        return None;
    }
    let head = doc.get(parts[0])?;
    if parts.len() == 2 {
        return match head {
            Item::Table(t) => t.get(parts[1])?.as_value(),
            Item::Value(Value::InlineTable(t)) => t.get(parts[1]),
            _ => None,
        };
    }
    let mid = head.as_table()?.get(parts[1])?;
    match mid {
        Item::Value(Value::InlineTable(t)) => t.get(parts[2]),
        Item::Table(t) => t.get(parts[2])?.as_value(),
        _ => None,
    }
}

fn find_value_mut<'a>(doc: &'a mut DocumentMut, parts: &[&str]) -> Option<&'a mut Value> {
    if parts.len() < 2 {
        return None;
    }
    let tail = parts[parts.len() - 1];
    if parts.len() == 2 {
        return match doc.get_mut(parts[0])? {
            Item::Table(t) => t.get_mut(tail)?.as_value_mut(),
            Item::Value(Value::InlineTable(t)) => t.get_mut(tail),
            _ => None,
        };
    }
    let mid = doc.get_mut(parts[0])?.as_table_mut()?.get_mut(parts[1])?;
    match mid {
        Item::Value(Value::InlineTable(t)) => t.get_mut(tail),
        Item::Table(t) => t.get_mut(tail)?.as_value_mut(),
        _ => None,
    }
}

/// The serialiser writes every struct it meets as one inline table, but a
/// configuration file is something a person opens in an editor: `[section]`
/// headers, one value per line. Hoist them back.
fn promote_tables(tbl: &mut Table) {
    let mut heads = Vec::new();
    for (key, item) in tbl.iter() {
        if let Item::Value(Value::InlineTable(inline)) = item {
            let mut table = Table::new();
            for (k, v) in inline.iter() {
                table.insert(k, Item::Value(v.clone()));
            }
            table.decor_mut().set_prefix(
                "
",
            );
            heads.push((key.to_string(), table));
        }
    }
    for (key, mut table) in heads {
        promote_tables(&mut table);
        if let Some(item) = tbl.get_mut(&key) {
            *item = Item::Table(table);
        }
    }
}

/// Copy every value from `src` into `dst`, keeping `dst`'s decor and comments.
/// Keys the schema no longer knows are left alone, not deleted.
fn merge_table(dst: &mut Table, src: &Table) {
    for (key, item) in src.iter() {
        match item {
            Item::Table(st) => {
                if let Some(Item::Table(dt)) = dst.get_mut(key) {
                    merge_table(dt, st);
                    continue;
                }
                dst.insert(key, Item::Table(st.clone()));
            }
            Item::Value(sv) => {
                if let Some(dv) = dst.get_mut(key).and_then(Item::as_value_mut) {
                    let decor = dv.decor().clone();
                    *dv = sv.clone();
                    dv.decor_mut().clone_from(&decor);
                    continue;
                }
                dst.insert(key, Item::Value(sv.clone()));
            }
            other => {
                dst.insert(key, other.clone());
            }
        }
    }
}

/// Parse `raw` into the type `old` already has.
fn coerce(old: &Value, leaf: &str, raw: &str) -> Result<Value, ConfigError> {
    let bad = |reason: String| ConfigError::InvalidValue {
        key: leaf.to_string(),
        reason,
    };
    Ok(match old {
        Value::Boolean(_) => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Value::from(true),
            "0" | "false" | "no" | "off" | "" => Value::from(false),
            other => return Err(bad(format!("{other} is not a boolean"))),
        },
        Value::Integer(_) => raw
            .trim()
            .replace([' ', '_'], "")
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| bad(format!("{} is not a whole number", raw.trim())))?,
        Value::Float(_) => raw
            .trim()
            .parse::<f64>()
            .map(Value::from)
            .map_err(|_| bad(format!("{} is not a number", raw.trim())))?,
        Value::Array(_) => {
            let mut arr = Array::new();
            for part in raw.split(',') {
                let p = part.trim();
                if !p.is_empty() {
                    arr.push(p);
                }
            }
            Value::Array(arr)
        }
        Value::InlineTable(_) => return Err(bad("this setting is not a single value".into())),
        _ => Value::from(raw.to_string()),
    })
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    use std::io::Write;
    let mut f = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|e| ConfigError::Io(format!("{}: {e}", path.display())))?;
    f.as_file_mut()
        .write_all(bytes)
        .map_err(|e| ConfigError::Io(format!("{}: {e}", path.display())))?;
    // `commit` is fsync + rename, which is exactly the rule in plan §6.1.
    f.commit()
        .map_err(|e| ConfigError::Io(format!("{}: {e}", path.display())))?;
    Ok(())
}

/// `config.toml.broken-<unix seconds>`; a collision adds `_2`, `_3`, …
fn quarantine(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut i = 0u32;
    loop {
        let suffix = if i == 0 {
            format!("{name}.broken-{secs}")
        } else {
            format!("{name}.broken-{secs}_{i}")
        };
        let cand = path.with_file_name(suffix);
        if !cand.exists() {
            return match std::fs::rename(path, &cand) {
                Ok(()) => Some(cand),
                Err(_) => None,
            };
        }
        i += 1;
    }
}

/// Where a save with no configured directory goes: the user's pictures folder
/// (§6.2 lets the user point the auto-save directory somewhere else, and says where
/// it goes when they have not).
///
/// `None` is this machine declining to say where its pictures are, which is a different
/// answer from "somewhere else then": a caller that invented a directory here would put
/// screenshots where nobody chose to put them, so the refusal travels up as a refusal.
/// The known folder is asked of the shell rather than built from `%USERPROFILE%\Pictures`,
/// because a redirected or OneDrive-held pictures folder is a real configuration and
/// concatenating a home path would quietly save beside it instead of into it.
pub fn pictures_dir() -> Option<PathBuf> {
    directories::UserDirs::new().and_then(|d| d.picture_dir().map(Path::to_path_buf))
}

/// Where `config.toml` lives: the OS config dir, unless `advanced.data_dir`
/// points somewhere else (§6.1).
pub fn config_path(cfg: &Config) -> PathBuf {
    if !cfg.advanced.data_dir.is_empty() {
        return Path::new(&cfg.advanced.data_dir).join(FILE_NAME);
    }
    directories::BaseDirs::new()
        .map(|d| d.config_dir().join("ai-falconshot").join(FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(FILE_NAME))
}

/// The directory `config_path` and `history.db` both live under — the same
/// resolution rule (empty = platform default under `%APPDATA%`), without the
/// trailing filename. P75: this is what `--selftest --history` hands to
/// `history_open_in`, so the probe and the config file always share a home.
pub fn data_dir(cfg: &Config) -> PathBuf {
    if !cfg.advanced.data_dir.is_empty() {
        return PathBuf::from(&cfg.advanced.data_dir);
    }
    directories::BaseDirs::new()
        .map(|d| d.config_dir().join("ai-falconshot"))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whiteboard::{Size, Surface};

    #[test]
    fn action_texts_answer_every_bindable_id_in_order() {
        // §5.21.2 那一页递出去的每一句中文，头一枚读者是这张表自己：id 与 [`ACTIONS`] 逐枚
        // 对齐（不同序就是错位，页上会拿 A 的说明配 B 那一行）、一枚不重、一句不空。
        assert_eq!(ACTION_TEXTS.len(), ACTIONS.len() + 3);
        for (i, id) in ACTIONS.iter().enumerate() {
            assert_eq!(&ACTION_TEXTS[i].0, id, "第 {i} 枚与 ACTIONS 不同序");
        }
        let mut seen: Vec<&str> = Vec::new();
        for (id, name, desc) in &ACTION_TEXTS {
            assert!(!name.is_empty(), "{id} 没有中文名");
            assert!(!desc.is_empty(), "{id} 没有说明");
            assert!(!desc.contains('\n'), "{id} 的说明换行");
            assert!(!seen.contains(id), "{id} 出现两次");
            seen.push(id);
        }
        // 末尾那三枚是谁、有几枚，由 TRAY_ACTIONS 答，不是这里重新数一遍（#297）。
        let extra: Vec<&str> = TRAY_ACTIONS
            .iter()
            .copied()
            .filter(|a| !ACTIONS.contains(a))
            .collect();
        assert_eq!(extra, vec!["menu", "settings", "none"]);
        assert_eq!(
            ACTION_TEXTS[ACTIONS.len()..]
                .iter()
                .map(|t| t.0)
                .collect::<Vec<&str>>(),
            extra
        );
        // 出厂默认里那九枚绑定，每一枚都问得出说明；名单外问不出（页上就不该硬拼一句）。
        let c = Config::default();
        for id in c.hotkey.actions.keys() {
            assert!(action_texts(id).is_some(), "{id} 没有说明");
        }
        assert!(action_texts("nope").is_none());
    }

    /// §5.9.13 规则第一句「必须提供明确且可靠的退出穿透方式」在**配置这一层**的那一半。
    ///
    /// 判的是档与名册，不是屏幕：一枚「不穿过贴图」的入口要成立，先得在码上有一个名字能被那只
    /// 入口点出来。三问各一枚 ——
    /// ① 这一枚动作在 [`ACTIONS`] 名单上 ⇒ §5.16.1 那一页的键、以及 §5.16.2 那一条排除都认得它，
    ///    一句「预设快捷键」才配得出来；
    /// ② 它在 [`ACTION_TEXTS`] 上有名有说明 ⇒ 托盘菜单里那一项、设置页那一行、运行日志里那一句
    ///    读的是同一枚名（#281），而「明确」二字要的就是这一枚：菜单上写着的字得说出它做什么；
    /// ③ 它在 [`TRAY_ACTIONS`] 上 ⇒ §5.18.2 那四枚鼠标键里任意一枚都能被配成这一手，那是**另一只
    ///    不经过贴图**的手。
    ///
    /// **这一格判不到的那半句**要写着：入口真在不在菜单上、按下去真收不收得回那扇窗，归
    /// `qt_bridge::hotkey::click_through_exits` 与那一族的装配判据（在桥那一层的单测里），
    /// 这里一枚也冒充不了 —— 名册上有名字而场上没有手，正是本仓记过的那族「配得上却什么都不做」。
    #[test]
    fn click_through_has_a_name_that_does_not_need_the_mouse() {
        const ESCAPE: &str = "release_click_through";
        assert!(
            ACTIONS.contains(&ESCAPE),
            "{ESCAPE} 不在可绑定名单里：快捷键那一页配不出这一手"
        );
        let (name, desc) = action_texts(ESCAPE).expect("{ESCAPE} 在名单里却没有名册那一行");
        assert!(
            !name.is_empty() && !desc.is_empty(),
            "{ESCAPE} 的名或说明是空串"
        );
        assert!(!desc.contains('\n'), "{ESCAPE} 的说明会切行");
        assert!(
            TRAY_ACTIONS.contains(&ESCAPE),
            "{ESCAPE} 进不了 §5.18.2 那四枚鼠标键的名单"
        );
        // 这一枚动作**只**做「解除」：它的名字与说的每一句都必须说出收回，而不是翻一次态。
        // 理由在 PRD 第三步那三个字「取消穿透」——一枚会把贴图按成点不到的键，不是可靠的退出。
        assert!(
            name.contains("解除") && !name.contains("切换"),
            "{ESCAPE} 在名册上叫 {name:?}：退出那一枚入口不许是一枚开关"
        );
        // 出厂默认里没有它那一枚键，这一条是**读数**而不是判据：写上它是为了让「开箱那一条退出
        // 走的是托盘」这件事在码上有一句真话，而不是靠注释里的一句愿话。
        let c = Config::default();
        assert_eq!(
            c.hotkey.actions.get(ESCAPE).map(String::as_str),
            None,
            "出厂默认给这一枚动作占了一枚键 —— 那是替 §5.16.1 那张表决定它自己没决定的事"
        );
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let c = Config::default();
        let text = c.export_text().unwrap();
        let (back, warnings) = Config::parse_import(&text).unwrap();
        assert_eq!(back, c);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn export_is_an_edited_by_hand_shaped_file() {
        // The serialiser's natural shape is one inline table per section, which
        // no one would keep in an editor. Writing must produce `[section]`
        // headers, and a nested map must become its own header too.
        let text = Config::default().export_text().unwrap();
        for header in [
            "[general]",
            "[capture]",
            "[whiteboard]",
            "[annotation]",
            "[annotation.tool_style]",
            "[pin]",
            "[card]",
            "[output]",
            "[hotkey]",
            "[hotkey.actions]",
            "[tray]",
            "[hot_corner]",
            "[hot_corner.corners]",
            "[appearance]",
            "[history]",
            "[advanced]",
            "[about]",
        ] {
            assert!(text.contains(header), "missing {header} in\n{text}");
        }
        assert!(
            text.contains("capture = \"ctrl+shift+a\""),
            "hotkey map lost its leaf\n{text}"
        );
        assert!(text.contains('\n'), "one flat line\n{text}");
    }

    #[test]
    fn every_category_is_readable_by_path() {
        let c = Config::default();
        let flat = c.flat().unwrap();
        for section in [
            "general.",
            "capture.",
            "whiteboard.",
            "annotation.",
            "pin.",
            "card.",
            "output.",
            "hotkey.",
            "tray.",
            "hot_corner.",
            "appearance.",
            "history.",
            "advanced.",
            "about.",
        ] {
            assert!(
                flat.keys().any(|k| k.starts_with(section)),
                "missing section {section}"
            );
        }
        assert_eq!(c.get_str("capture.magnifier_zoom").as_deref(), Some("8"));
        assert_eq!(c.get_str("output.format").as_deref(), Some("png"));
        assert_eq!(c.get_str("general.show_tray").as_deref(), Some("true"));
        assert_eq!(
            c.get_str("output.naming_template").as_deref(),
            Some("%p_%Y-%m-%d_%H%M%S")
        );
        assert_eq!(
            c.get_str("hotkey.actions.capture").as_deref(),
            Some("ctrl+shift+a")
        );
        assert_eq!(
            c.get_str("hot_corner.corners.bottom_right").as_deref(),
            Some("capture")
        );
        assert_eq!(
            c.get_str("annotation.palette").as_deref(),
            Some("#E81123, #F38B00, #FFB900, #7A7A7A, #FFFFFF, #000000, #0078D7, #16C60C, #B4009E, #00B7C3")
        );
        assert_eq!(c.get_str("card.width").as_deref(), Some("480"));
        assert_eq!(c.get_str("card.font_size").as_deref(), Some("18"));
        assert_eq!(c.get_str("card.background").as_deref(), Some("#FFFFFF"));
        assert!(c.has_key("history.max_items"));
        assert!(!c.has_key("history.nope"));
        assert!(!c.has_key("history"));
    }

    #[test]
    fn a_tools_pen_survives_the_file_it_was_written_to() {
        // §9.2 样式记忆在重启后仍然有效, and this is the round trip a restart takes:
        // the layer writes `#RRGGBBAA` because the alpha is part of the pen, the file
        // carries it, and the layer reads it back.
        let mut c = Config::default();
        for (tool, color, width) in [
            ("rect", "#123456EF", 9u32),
            ("arrow", "#FFB900FF", 2),
            ("mosaic", "#00000000", 64),
        ] {
            c.annotation.tool_style.insert(
                tool.into(),
                ToolStyle {
                    color: Some(color.into()),
                    width: Some(width),
                    intensity: None,
                },
            );
        }
        let text = c.export_text().unwrap();
        let (back, warnings) = Config::parse_import(&text).unwrap();
        assert_eq!(back, c);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(
            text.contains("[annotation.tool_style.rect]"),
            "one header per tool\n{text}"
        );
        assert_eq!(
            back.get_str("annotation.tool_style.rect.width").as_deref(),
            Some("9")
        );
        assert_eq!(
            back.get_str("annotation.tool_style.mosaic.color")
                .as_deref(),
            Some("#00000000")
        );
    }

    #[test]
    fn a_remembered_style_naming_no_tool_is_dropped_not_renamed() {
        // `rec` is one keystroke from `rect`, and guessing would give the rect another
        // tool's pen. The layer has no way to name a tool either, so the config is the
        // place that says so.
        let mut c = Config::default();
        c.annotation.tool_style.insert(
            "rec".into(),
            ToolStyle {
                color: Some("#E81123FF".into()),
                width: Some(4),
                intensity: None,
            },
        );
        c.annotation.tool_style.insert(
            "rect".into(),
            ToolStyle {
                color: Some("#0078D7FF".into()),
                width: Some(5),
                intensity: None,
            },
        );
        assert_eq!(
            c.validate(),
            vec!["annotation.tool_style.rec: no such tool, dropped".to_string()]
        );
        assert_eq!(c.annotation.tool_style.len(), 1);
        assert_eq!(c.annotation.tool_style["rect"].width, Some(5));
    }

    #[test]
    fn half_a_bad_style_does_not_spill_on_the_other_half() {
        let mut c = Config::default();
        c.annotation.tool_style.insert(
            "ellipse".into(),
            ToolStyle {
                color: Some("crimson-ish".into()),
                width: Some(200),
                intensity: None,
            },
        );
        let warnings = c.validate();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("ellipse.color"), "{warnings:?}");
        assert!(warnings[1].contains("ellipse.width"), "{warnings:?}");
        let kept = &c.annotation.tool_style["ellipse"];
        assert_eq!(kept.color, None, "the unreadable colour is gone");
        assert_eq!(kept.width, Some(64), "the width was clamped, not dropped");
    }

    #[test]
    fn a_hand_written_entry_with_one_field_keeps_it() {
        // The file is something a person edits, so half an entry is a legal thing to
        // write: the tool's width changes, its colour is left to `[annotation]`.
        let mut c: Config = toml_edit::de::from_str(
            r#"
[annotation.tool_style.rect]
width = 5
"#,
        )
        .unwrap();
        assert_eq!(c.annotation.tool_style.len(), 1);
        assert_eq!(c.annotation.tool_style["rect"].width, Some(5));
        assert_eq!(c.annotation.tool_style["rect"].color, None);
        assert!(c.validate().is_empty());
    }

    #[test]
    fn a_strength_survives_the_file_and_only_lives_where_it_means_something() {
        // §9.2's restart for §5.7.10's 模糊强度: the number has to come back, and it has
        // to come back as *that tool's* number. One row of the table for both tools is
        // how a tuned radius reappeared on a mosaic.
        let mut c: Config = toml_edit::de::from_str(
            r#"
[annotation.tool_style.blur]
intensity = 24
[annotation.tool_style.mosaic]
intensity = 4
"#,
        )
        .unwrap();
        assert!(c.validate().is_empty(), "{:?}", c.validate());
        assert_eq!(c.annotation.tool_style["blur"].intensity, Some(24));
        assert_eq!(c.annotation.tool_style["mosaic"].intensity, Some(4));
        assert_eq!(
            c.get_str("annotation.tool_style.blur.intensity").as_deref(),
            Some("24")
        );

        let text = c.export_text().unwrap();
        let (back, warnings) = Config::parse_import(&text).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.annotation.tool_style["blur"].intensity, Some(24));
        assert_eq!(back.annotation.tool_style["mosaic"].intensity, Some(4));

        // Out of range is clamped like every other number, and a strength written on a
        // tool that has none is reported rather than kept as decoration.
        let mut c: Config = toml_edit::de::from_str(
            r#"
[annotation.tool_style.blur]
intensity = 500
[annotation.tool_style.rect]
intensity = 8
"#,
        )
        .unwrap();
        let warnings = c.validate();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(c.annotation.tool_style["blur"].intensity, Some(EFFECT_MAX));
        assert_eq!(c.annotation.tool_style["rect"].intensity, None);
    }

    #[test]
    fn a_partial_file_fills_in_from_defaults() {
        let c: Config = toml_edit::de::from_str(
            r#"
[capture]
magnifier_zoom = 12
"#,
        )
        .unwrap();
        assert_eq!(c.capture.magnifier_zoom, 12);
        assert_eq!(c.capture.magnifier_radius, 8);
        assert!(c.general.show_tray);
        assert_eq!(c.hotkey.actions["capture"], "ctrl+shift+a");
    }

    #[test]
    fn set_str_keeps_the_stored_type() {
        let mut c = Config::default();
        c.set_str("capture.magnifier_zoom", "16").unwrap();
        assert_eq!(c.capture.magnifier_zoom, 16);
        c.set_str("general.show_tray", "off").unwrap();
        assert!(!c.general.show_tray);
        c.set_str("output.format", "jpg").unwrap();
        assert_eq!(c.output.format, Format::Jpg);
        c.set_str("output.save_dir", "D:/shots").unwrap();
        assert_eq!(c.output.save_dir, "D:/shots");
        // A thousands separator from a spin box still means a number.
        c.set_str("history.max_items", "1 000").unwrap();
        assert_eq!(c.history.max_items, 1000);
        // Nested maps and lists are reachable through the same path API.
        c.set_str("hotkey.actions.capture", "f13").unwrap();
        assert_eq!(c.hotkey.actions["capture"], "f13");
        c.set_str("annotation.palette", "#FF0000, #00FF00").unwrap();
        assert_eq!(c.annotation.palette, ["#FF0000", "#00FF00"]);
        // A key spec has a grammar; a widget must not be able to store "nope".
        c.set_str("hotkey.actions.solo", "CTRL+SHIFT+S").unwrap();
        assert_eq!(c.hotkey.actions["solo"], "ctrl+shift+s");
        c.set_str("hotkey.actions.toggle_pins", "").unwrap();
        assert_eq!(c.hotkey.actions["toggle_pins"], "");
        assert!(c.set_str("hotkey.actions.solo", "nope").is_err());
        assert!(c.set_str("hotkey.actions.solo", "a").is_err());
        assert!(c.set_str("hot_corner.corners.top_left", "capture").is_ok());
        assert!(c.set_str("hot_corner.corners.top_left", "fly").is_err());
        assert!(c.set_str("capture.magnifier_zoom", "big").is_err());
        assert!(c.set_str("general.show_tray", "maybe").is_err());
        assert!(c.set_str("nope", "1").is_err());
        assert!(c.set_str("capture", "1").is_err());
        assert!(c.set_str("capture.nope", "1").is_err());
        assert!(c.set_str("capture.magnifier.", "1").is_err());
        // A rejected write leaves the config untouched.
        assert_eq!(c.capture.magnifier_zoom, 16);
    }

    /// §5.9.15 的四枚模式是设置件唯一能拼出的写法，而拼错时交回的那句话必须点名
    /// 它被写进的那一枚键：这一格的存储类型是一枚枚举，所以在 coerce 之后整份文档
    /// 停止解析 —— 设置页会看见一句关于**整个文件**的抱怨，而不是关于它自己那一个
    /// 输入框的（§5.8.3「应显示错误信息」要的是后者）。
    #[test]
    fn a_background_mode_is_written_by_its_own_name_or_answered_by_that_key() {
        let mut c = Config::default();
        c.set_str("pin.alpha_bg", "checker_dark").unwrap();
        assert_eq!(c.pin.alpha_bg, AlphaBg::CheckerDark);
        assert_eq!(c.get_str("pin.alpha_bg").as_deref(), Some("checker_dark"));

        let err = c.set_str("pin.alpha_bg", "plaid").unwrap_err();
        match err {
            ConfigError::InvalidValue { key, reason } => {
                assert_eq!(key, "pin.alpha_bg", "{key} / {reason}");
                assert!(reason.contains("plaid"), "{reason}");
                assert!(reason.contains("checker_light"), "{reason}");
            }
            other => panic!("一枚写错的背景模式被答成 {other:?}"),
        }
        // 被拒的那一次写没动过任何值。
        assert_eq!(c.pin.alpha_bg, AlphaBg::CheckerDark);
        assert!(c.set_str("pin.alpha_bg", "").is_err());
        assert_eq!(c.pin.alpha_bg, AlphaBg::CheckerDark);
    }

    /// §5.3.7 第一步的「选择单位」自 P164 起是一枚设置项，所以它走的必须是
    /// `pin.alpha_bg` 走过的那一道闸：拼错时交回的一句话点名 `capture.size_unit`
    /// 这一枚键，并把唯一合法的两枚写法念出来。少了这道闸，`Unit` 的
    /// `snake_case` 反序列化会在 `from_document` 那一步让整份文件解析失败 ——
    /// 设置页拿到的将是一句关于整个文件的抱怨，而不是关于它自己那一个输入框的。
    #[test]
    fn a_size_unit_is_written_by_its_own_name_or_answered_by_that_key() {
        let mut c = Config::default();
        assert_eq!(c.get_str("capture.size_unit").as_deref(), Some("px"));

        c.set_str("capture.size_unit", "dip").unwrap();
        assert_eq!(c.capture.size_unit, Unit::Dip);
        assert_eq!(c.get_str("capture.size_unit").as_deref(), Some("dip"));

        let err = c.set_str("capture.size_unit", "pixels").unwrap_err();
        match err {
            ConfigError::InvalidValue { key, reason } => {
                assert_eq!(key, "capture.size_unit", "{key} / {reason}");
                assert!(reason.contains("pixels"), "{reason}");
                assert!(reason.contains("px"), "{reason}");
                assert!(reason.contains("dip"), "{reason}");
            }
            other => panic!("一枚写错的单位被答成 {other:?}"),
        }
        // 被拒的那一次写没动过任何值，也不许把 `dip` 换成大写再来一次。
        assert_eq!(c.capture.size_unit, Unit::Dip);
        assert!(c.set_str("capture.size_unit", "PX").is_err());
        assert!(c.set_str("capture.size_unit", "").is_err());
        assert_eq!(c.capture.size_unit, Unit::Dip);
        // 两头空格是打字时带进来的，不是拼错的第三种写法：认，且认下之后存进档的
        // 是名单上那一枚字面本身 —— 否则这一格会被自己的闸放行、再在 `from_document`
        // 那里被顶成一句关于整份文件的抱怨。这一句是 P164 第一趟 `cargo test` 真报出来的
        // 那一手（`unknown variant \` px \``），不是补写的。
        c.set_str("capture.size_unit", " px ").unwrap();
        assert_eq!(c.capture.size_unit, Unit::Px);
        assert_eq!(c.get_str("capture.size_unit").as_deref(), Some("px"));
        assert!(
            c.export_text().unwrap().contains("size_unit = \"px\""),
            "打过空格的那一次写把原文存进了档"
        );
        assert!(c.set_str("capture.size_unit", "d ip").is_err());
        assert_eq!(c.capture.size_unit, Unit::Px);
    }

    /// 单位这一枚枚举在文件里怎么拼、在页上怎么亮，自 P164 起是同一张名单
    /// （[`crate::geometry::Unit::ALL`]）：`serde` 的 `snake_case` 改了拼法、或
    /// `label()` 与序列化偏离，都在这一格被抓住，而不是让遮罩读到出厂那一格。
    #[test]
    fn the_two_size_units_are_spelled_from_one_list() {
        let text = Config::default().export_text().unwrap();
        assert!(
            text.contains("size_unit = \"px\""),
            "文件里的出厂单位不是 px：\n{text}"
        );
        for unit in Unit::ALL {
            let mut c = Config::default();
            c.set_str("capture.size_unit", unit.label()).unwrap();
            let line = format!("size_unit = \"{}\"", unit.label());
            let written = c.export_text().unwrap();
            assert!(written.contains(&line), "{line} 没被这样写进文件");
            let (back, warnings) = Config::parse_import(&written).unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(back.capture.size_unit, unit, "{line} 读不回来");
        }
    }

    /// 一张表、四枚模式：枚举里加一行而名单没跟上、或 `snake_case` 改了拼法，都在
    /// 这一格被抓住，而不是在一份再也读不进来的文件上被抓。
    #[test]
    fn the_four_background_modes_are_spelled_from_one_list() {
        let all = [
            AlphaBg::Transparent,
            AlphaBg::Pseudo,
            AlphaBg::CheckerDark,
            AlphaBg::CheckerLight,
        ];
        let spelled: Vec<&str> = all.iter().map(|m| m.id()).collect();
        assert_eq!(spelled, ALPHA_BG_IDS.to_vec());
        for m in all {
            assert_eq!(AlphaBg::from_id(m.id()), Some(m), "{}", m.id());
        }
        // 反证（#274）：一把只会照名单点头的尺，读一枚真不在名单上的名字必须交 None。
        assert_eq!(AlphaBg::from_id("Transparent"), None);
        assert_eq!(AlphaBg::from_id(" checker_dark"), None);
        assert_eq!(AlphaBg::from_id("checker_darkk"), None);
        assert_eq!(AlphaBg::default(), AlphaBg::Transparent);
    }

    /// A menu carries an index, a file carries a name, and the pin holds the
    /// enum: all three have to agree on one order, or the third row of the menu
    /// paints the second board (§5.9.15 lists four modes in a fixed order).
    #[test]
    fn a_background_mode_answers_its_menu_index() {
        let all = [
            AlphaBg::Transparent,
            AlphaBg::Pseudo,
            AlphaBg::CheckerDark,
            AlphaBg::CheckerLight,
        ];
        for (at, m) in all.iter().enumerate() {
            let i = at as u32;
            assert_eq!(AlphaBg::from_index(i), Some(*m), "第 {i} 行");
            assert_eq!(m.index(), i, "{}", m.id());
            assert_eq!(
                AlphaBg::from_index(m.index()),
                Some(*m),
                "{} 的序号回不来",
                m.id()
            );
            assert_eq!(
                AlphaBg::from_id(ALPHA_BG_IDS[at]),
                Some(*m),
                "第 {i} 行与名单不符"
            );
        }
        // 反证（#274）：越界的序号不是「最后那一种」，是不认。
        assert_eq!(AlphaBg::from_index(4), None);
        assert_eq!(AlphaBg::from_index(u32::MAX), None);
    }

    /// #170: the load path ran [`Config::validate`] and the write path ran
    /// nothing, so a widget could store a value the same file can never carry on
    /// the next launch. Writing and reading have to agree on one value.
    #[test]
    fn a_write_is_gated_like_a_read() {
        // Out of range: the file is clamped on load, so a write must not keep it.
        let mut big = Config::default();
        let corrections = big.set_str("card.font_size", "500").unwrap();
        assert_eq!(
            big.card.font_size, FONT_MAX,
            "the write path stored 500 while the file can only carry 96"
        );
        // And it has to say which one it changed: a silent 96 is a settings page
        // that disagrees with what the user typed (§5.20.2).
        assert_eq!(corrections.len(), 1, "{corrections:?}");
        assert!(corrections[0].contains("card.font_size"), "{corrections:?}");
        assert!(
            corrections[0].contains("out of range, using 96"),
            "{corrections:?}"
        );
        assert_eq!(
            big.get_str("card.font_size").as_deref(),
            Some("96"),
            "a value this config just wrote does not read back the same"
        );

        // An unreadable colour: load repairs it to the default, so write does too.
        let mut bg = Config::default();
        bg.set_str("card.background", "papercolor").unwrap();
        assert_eq!(
            bg.card.background, "#FFFFFF",
            "the write path kept a colour the load path repairs"
        );

        // A relative directory: load ignores it, so a write may not keep it.
        let mut dir = Config::default();
        dir.output.save_dir = "C:/shots".into();
        dir.set_str("output.save_dir", "shots").unwrap();
        assert_eq!(
            dir.output.save_dir, "",
            "the write path kept a relative dir the load path clears"
        );

        // And the gate changes nothing that was already legal.
        let mut ok = Config::default();
        assert!(ok.set_str("card.font_size", "24").unwrap().is_empty());
        assert_eq!(ok.card.font_size, 24);

        // A key the file does not have is still a refusal, not a clamp.
        let mut miss = Config::default();
        assert!(matches!(
            miss.set_str("card.nope", "1"),
            Err(ConfigError::UnknownKey(_))
        ));
    }

    /// §5.20.3 step 5 applies key by key through [`Config::set_str`], so the
    /// gate a file passes on parse has to pass on apply too — otherwise an
    /// imported 500 becomes the stored 500 and the restart gets a different card.
    #[test]
    fn applying_an_import_is_gated_too() {
        let mut incoming = Config::default();
        incoming.card.font_size = 500;
        incoming.card.width = 1_000_000;
        let mut target = Config::default();
        target.apply_import(&incoming, ImportScope::All).unwrap();
        assert_eq!(
            target.card.font_size, FONT_MAX,
            "the import path went around the gate"
        );
        assert_eq!(
            target.card.width, 4096,
            "the import path went around the gate"
        );

        // Two files that collide only together: `capture` already holds f13 on this
        // machine, and the incoming file teaches `toggle_pins` the same key. Each
        // side is legal, and `parse_import` — which reads the incoming file alone —
        // cannot see it. The apply is the only place that can say it out loud.
        // §8.4 reports a clash and never picks a winner, so both keys stay as
        // written and the report is all the user gets.
        let mut target = Config::default();
        target.hotkey.actions.insert("capture".into(), "f13".into());
        let mut incoming = Config::default();
        incoming
            .hotkey
            .actions
            .insert("capture".into(), "f13".into());
        incoming
            .hotkey
            .actions
            .insert("toggle_pins".into(), "f13".into());
        let changes = target.preview_import(&incoming, ImportScope::All);
        assert!(
            !changes.iter().any(|c| c.key == "hotkey.actions.capture"),
            "both sides hold f13, so this must not be an overwrite: {changes:?}"
        );
        let corrections = target.apply_import(&incoming, ImportScope::All).unwrap();
        assert!(
            corrections.iter().any(|c| c.contains("is bound twice")
                && c.contains("capture")
                && c.contains("toggle_pins")),
            "{corrections:?}"
        );
        assert_eq!(target.hotkey.actions["capture"], "f13");
        assert_eq!(target.hotkey.actions["toggle_pins"], "f13");
        // The same clash is reported once, not once per write: anyone counting the
        // line count off the report would otherwise read N clashes out of one.
        assert_eq!(
            corrections
                .iter()
                .filter(|c| c.contains("is bound twice"))
                .count(),
            1,
            "{corrections:?}"
        );
    }

    /// A hand-edited file with two actions on one combination is reported, not
    /// silently re-resolved (§8.4: nothing is overwritten for the user).
    /// A hand-edited or foreign file with two actions on one combination is
    /// described, not silently re-resolved (§8.4: nothing is overwritten for the
    /// user).
    #[test]
    fn a_conflicting_hotkey_file_is_described_not_repaired() {
        let (mut c, warnings) = Config::parse_import(
            r#"
[hotkey]
actions = { capture = "CTRL+SHIFT+A", solo = "shift+ctrl+a", pin_edit = "f1" }
"#,
        )
        .unwrap();
        assert_eq!(c.hotkey.actions["capture"], "ctrl+shift+a");
        assert_eq!(c.hotkey.actions["solo"], "ctrl+shift+a");
        // Bare `f1` cannot be registered globally, so it is dropped rather than kept.
        assert_eq!(c.hotkey.actions["pin_edit"], "");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("capture, solo") && w.contains("Ctrl+Shift+A")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("written")),
            "{warnings:?}"
        );
        // Re-running the check finds the same conflict: validate repairs values,
        // it does not arbitrate between two live bindings.
        let again = c.validate();
        assert_eq!(again.len(), 1, "{again:?} vs {warnings:?}");
        assert!(again.iter().any(|w| w.contains("bound twice")), "{again:?}");
    }

    #[test]
    fn validate_fixes_values_and_says_so() {
        let mut c = Config::default();
        c.pin.default_opacity = 900;
        c.annotation.arrow_style = "spiral".into();
        c.annotation.palette = ["#FFFFFF", "not-a-colour"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        c.annotation.stroke_color = "reddish".into();
        c.output.jpeg_quality = 0;
        c.output.naming_template = "%p_%Q".into();
        c.capture.magnifier_color_format = "crayon".into();
        c.output.save_dir = "relative/dir".into();
        c.hot_corner
            .corners
            .insert("top_left".into(), "nope".into());
        let warnings = c.validate();
        assert_eq!(c.pin.default_opacity, 100);
        assert_eq!(c.annotation.arrow_style, "straight");
        assert_eq!(c.annotation.palette, ["#FFFFFF"]);
        assert_eq!(c.annotation.stroke_color, "#E81123");
        assert_eq!(c.output.jpeg_quality, 1);
        assert_eq!(c.output.naming_template, crate::naming::DEFAULT_TEMPLATE);
        assert_eq!(c.capture.magnifier_color_format, "hex_upper");
        assert!(c.output.save_dir.is_empty());
        assert_eq!(c.hot_corner.corners["top_left"], "");
        for needle in [
            "pin.default_opacity",
            "arrow_style",
            "palette",
            "stroke_color",
            "jpeg_quality",
            "naming_template",
            "magnifier_color_format",
            "save_dir",
            "hot_corner.corners.top_left",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "no warning for {needle}: {warnings:?}"
            );
        }
        // The defaults need no repair at all.
        assert!(Config::default().validate().is_empty());
        // A palette of nothing comes back rather than an empty swatch row.
        let mut empty = Config::default();
        empty.annotation.palette.clear();
        assert!(empty.validate().is_empty());
        assert_eq!(empty.annotation.palette, default_palette());
    }

    /// §5.9.10 的「预设尺寸」是配置而不是常量：两枚键有默认、能被 `validate` 夹回
    /// 合理范围、并且随导出/导入走完整条管线（`flat` 是设置页与导出枚举键的那只眼睛，
    /// 一个不在 `flat` 里的键就是一个没人能绑的键）。
    #[test]
    fn the_preset_thumbnail_size_is_a_configured_value() {
        // Defaults agree with the core constant, so 240 has one author.
        let c = Config::default();
        assert_eq!(c.pin.thumb_fixed_width, crate::pin::THUMB_FIXED_DEFAULT.w);
        assert_eq!(c.pin.thumb_fixed_height, crate::pin::THUMB_FIXED_DEFAULT.h);
        let flat = c.flat().unwrap();
        assert_eq!(flat["pin.thumb_fixed_width"], "240", "{flat:?}");
        assert_eq!(flat["pin.thumb_fixed_height"], "240", "{flat:?}");

        // Out-of-range values are clamped and reported, like every other number.
        let mut c = Config::default();
        c.pin.thumb_fixed_width = 0;
        c.pin.thumb_fixed_height = 9999;
        let warnings = c.validate();
        assert_eq!(c.pin.thumb_fixed_width, 1);
        assert_eq!(c.pin.thumb_fixed_height, 4096);
        for needle in ["pin.thumb_fixed_width", "pin.thumb_fixed_height"] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "no warning for {needle}: {warnings:?}"
            );
        }

        // The keys travel through the export text and back through import.
        let mut c = Config::default();
        c.pin.thumb_fixed_width = 160;
        c.pin.thumb_fixed_height = 90;
        let text = c.export_text().unwrap();
        let (back, warns) = Config::parse_import(&text).unwrap();
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(back.pin.thumb_fixed_width, 160);
        assert_eq!(back.pin.thumb_fixed_height, 90);
    }

    /// §5.8.4 方式二第 3 步那句「具体行为由设置决定」在档上的那一枚键。
    ///
    /// 钉三件事，缺一件就是那一格还没真的接上：**默认**是较保守的那一头（不换掉用户已有的那一枚
    /// 贴图），**类型**是布尔（`get_bool` 认得它，页上递的才是开关而不是一格文本框，见
    /// `settings_view::kind_of`），**导出/导入**带得走（不在 `flat()` 里的一枚键就是一个页上点不到
    /// 也没人读的键）。至于「这一枚设了 true 之后落点是换还是新钉」不在这儿判 —— 那是
    /// [`crate::drop::Landing::plan`] 那一张表，本模块只存那一格。
    #[test]
    fn the_drop_lands_by_a_key_that_is_stored_and_switch_shaped() {
        let c = Config::default();
        assert!(
            !c.pin.drop_replaces_current,
            "默认那一头必须是「另钉一枚」：换掉用户已有的一张是不可逆的那一支"
        );
        let flat = c.flat().unwrap();
        assert_eq!(flat["pin.drop_replaces_current"], "false", "{flat:?}");
        assert_eq!(c.get_bool("pin.drop_replaces_current"), Some(false));
        assert_eq!(
            c.get_str("pin.drop_replaces_current").as_deref(),
            Some("false"),
            "页上那一行读的就是这一串"
        );

        let mut c = Config::default();
        c.set_str("pin.drop_replaces_current", "true").unwrap();
        assert_eq!(c.get_bool("pin.drop_replaces_current"), Some(true));
        assert!(c.pin.drop_replaces_current);
        // 一枚布尔被写成文本那一族坏：`set_str` 走 `coerce`，翻成 `true`/`false` 才存得进去，
        // 存进去之后 `validate` 不该有话说（它没有数值范围可夹）。
        assert!(c.validate().is_empty(), "{:?}", c.validate());

        let text = c.export_text().unwrap();
        let (back, warns) = Config::parse_import(&text).unwrap();
        assert!(warns.is_empty(), "{warns:?}");
        assert!(back.pin.drop_replaces_current, "走了一趟档就不是那一头了");
        // 它跟着「贴图」那一类走（§5.20.1 那一列里 `pin` 归贴图），建议范围也得覆盖它，否则
        // §5.20.3 导出那一趟会把它丢掉。
        assert!(
            ImportScope::Suggested.covers("pin.drop_replaces_current"),
            "导出会丢掉这一枚键"
        );
    }

    /// §5.7.22 步骤 3 「保存为调色板方案」: the page writes one whole list through the same
    /// `set_str` a text row uses, so the grammar of an entry is decided here rather than on the
    /// page - and an empty list must still be a row, or there is no key to write the first
    /// scheme into.
    #[test]
    fn a_saved_scheme_round_trips_through_the_key_the_page_writes() {
        let mut c = Config::default();
        assert_eq!(
            c.get_str("annotation.palette_schemes").as_deref(),
            Some(""),
            "a scheme list nobody has saved is still a row the page can write"
        );

        let one = scheme_join("工作", &["#E81123".into(), "#0078D7".into()]).unwrap();
        assert_eq!(one, "工作=#E81123 #0078D7");
        let (name, colors) = scheme_split(&one).unwrap();
        assert_eq!((name, colors), ("工作", vec!["#E81123", "#0078D7"]));
        assert!(scheme_split("没有等号").is_none());
        assert!(scheme_split("空名单=").is_none());
        assert!(scheme_split("=只有色没有名").is_none());
        assert!(scheme_join("带,号的", &["#E81123".into()]).is_none());
        assert!(scheme_join("带=号的", &["#E81123".into()]).is_none());
        assert!(scheme_join("带|号的", &["#E81123".into()]).is_none());
        assert!(scheme_join("带\n行的", &["#E81123".into()]).is_none());
        assert!(scheme_join("工作", &["#E81123 #0078D7".into()]).is_none());
        assert!(scheme_join("", &["#E81123".into()]).is_none());

        c.set_str(
            "annotation.palette_schemes",
            "工作=#E81123 #0078D7, 旧=#FFFFFF",
        )
        .unwrap();
        assert_eq!(
            c.annotation.palette_schemes,
            ["工作=#E81123 #0078D7", "旧=#FFFFFF"]
        );
        // and the file that comes back out reads in as the same two schemes
        let text = c.export_text().unwrap();
        let (again, warnings) = Config::parse_import(&text).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            again.annotation.palette_schemes,
            c.annotation.palette_schemes
        );

        let mut fix = Config::default();
        fix.annotation.palette_schemes = vec![
            "甲=#E81123 not-a-colour".into(),
            "甲=#0078D7".into(),
            "没有等号".into(),
            "乙=nope".into(),
            "带|号=#E81123".into(),
        ];
        let warnings = fix.validate();
        assert_eq!(fix.annotation.palette_schemes, ["甲=#E81123"]);
        for needle in [
            "甲: dropped not-a-colour",
            "a second scheme of that name",
            "is not `name=colour colour`",
            "乙: no colour left",
            "带|号: a separator inside it",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "no warning for {needle}: {warnings:?}"
            );
        }
    }

    /// §5.5.6's "或按配置填充背景色" and §5.5.7 step 2's 边框颜色、宽度、阴影强度和边距
    /// are read once, when the mask opens, and never argued with afterwards - so the
    /// arguing has to happen here, in the one place that runs before the picture exists.
    #[test]
    fn decoration_keys_are_read_as_colours_and_clamped_as_numbers() {
        let mut shipped = Config::default();
        assert!(
            shipped.validate().is_empty(),
            "the shipped decoration needs no repair: {:?}",
            shipped.validate()
        );
        assert!(shipped.output.corner_fill.is_empty());
        assert_eq!(shipped.output.corner_radius, 0);
        assert_eq!(shipped.output.shadow_size, 0);

        // An unreadable fill is not the same defect as an unreadable accent: §5.5.6
        // allows 透明, so "" is a value, and the only honest reading of a typo there is
        // "the corner you asked for is gone". Cleared *and* said.
        let mut c = Config::default();
        c.output.corner_fill = "ivory".into();
        c.output.border_color = "not-a-colour".into();
        c.output.border_width = 999;
        c.output.shadow_strength = 999;
        let warnings = c.validate();
        assert!(
            c.output.corner_fill.is_empty(),
            "{:?}",
            c.output.corner_fill
        );
        assert_eq!(c.output.border_color, "#000000");
        assert_eq!(c.output.border_width, 64);
        assert_eq!(c.output.shadow_strength, 100);
        for needle in [
            "output.corner_fill",
            "output.border_color",
            "output.border_width",
            "output.shadow_strength",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "no warning for {needle}: {warnings:?}"
            );
        }

        // A fill that reads is kept alpha and all, including an all-transparent one:
        // `#00000000` is a corner the user asked for, not a parse failure.
        let mut keep = Config::default();
        keep.output.corner_fill = "#FFFFFF80".into();
        keep.output.border_color = "#112233FF".into();
        assert!(keep.validate().is_empty(), "{:?}", keep.validate());
        assert_eq!(keep.output.corner_fill, "#FFFFFF80");
        assert_eq!(keep.output.border_color, "#112233FF");
    }

    /// §5.8.5 step 4 names four 卡片设置 and all four are read here rather than
    /// argued with at paint time: a card asked for at 500 px font or a 12 px column
    /// is a card that renders nothing, and "nothing" is not an answer the user can
    /// read off the screen.
    #[test]
    fn card_keys_are_clamped_and_its_background_repaired() {
        let mut shipped = Config::default();
        assert!(
            shipped.validate().is_empty(),
            "the shipped card needs no repair: {:?}",
            shipped.validate()
        );

        let mut c = Config::default();
        c.card.font_size = 500;
        c.card.width = 1_000_000;
        c.card.background = "papercolor".into();
        let warnings = c.validate();
        assert_eq!(c.card.font_size, FONT_MAX);
        assert_eq!(c.card.width, 4096);
        assert_eq!(c.card.background, "#FFFFFF");
        for needle in ["card.font_size", "card.width", "card.background"] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "no warning for {needle}: {warnings:?}"
            );
        }

        // The floor matters more than the ceiling here: a zero font size would leave
        // the breaker with a column it can never fill, and a zero width with a card
        // no frame can be built from.
        let mut low = Config::default();
        low.card.font_size = 0;
        low.card.width = 0;
        low.validate();
        assert_eq!(low.card.font_size, FONT_MIN);
        assert_eq!(low.card.width, 160);
    }

    #[test]
    fn save_rewrites_values_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "# my settings\n[capture]\n# the zoom I like\nmagnifier_zoom = 4  # four\n\n[output]\nformat = \"bmp\"\n",
        )
        .unwrap();
        let (mut c, report) = Config::load(&path);
        assert_eq!(report.source, LoadSource::File);
        assert_eq!(c.capture.magnifier_zoom, 4);
        assert_eq!(c.output.format, Format::Bmp);
        c.capture.magnifier_zoom = 9;
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my settings"), "{text}");
        assert!(text.contains("the zoom I like"), "{text}");
        assert!(text.contains("four"), "{text}");
        assert!(text.contains("magnifier_zoom = 9"), "{text}");
        // Sections the user never wrote are appended on save.
        assert!(text.contains("freeze_frame"), "{text}");
        let (again, _) = Config::load(&path);
        assert_eq!(again.capture.magnifier_zoom, 9);
        assert_eq!(again.output.format, Format::Bmp);
        assert_eq!(again.capture.magnifier_radius, 8);
    }

    #[test]
    fn a_damaged_file_is_kept_and_defaults_win() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "[capture\n  ??? not toml").unwrap();
        let (c, report) = Config::load(&path);
        assert_eq!(report.source, LoadSource::Recovered);
        let backup = report.backup.clone().expect("a backup path");
        assert!(backup.exists());
        assert!(!path.exists(), "the damaged file must be moved aside");
        assert!(backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("config.toml.broken-"));
        assert_eq!(c, Config::default());
        // The next save writes a clean file.
        c.save(&path).unwrap();
        let (c2, r2) = Config::load(&path);
        assert_eq!(r2.source, LoadSource::File);
        assert_eq!(c2, c);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join(FILE_NAME);
        let (c, report) = Config::load(&path);
        assert_eq!(report.source, LoadSource::Defaults);
        assert_eq!(c, Config::default());
        c.save(&path).unwrap();
        assert!(path.exists(), "save creates the parent directory");
    }

    #[test]
    fn import_preview_only_lists_what_changes() {
        let mut incoming = Config::default();
        incoming
            .hotkey
            .actions
            .insert("capture".into(), "f13".into());
        incoming.general.language = "en".into();
        let current = Config::default();
        let changes = current.preview_import(&incoming, ImportScope::All);
        let keys: Vec<&str> = changes.iter().map(|c| c.key.as_str()).collect();
        assert!(keys.contains(&"hotkey.actions.capture"), "{keys:?}");
        assert!(keys.contains(&"general.language"), "{keys:?}");
        let language = changes
            .iter()
            .find(|c| c.key == "general.language")
            .unwrap();
        assert!(language.restart_required, "language needs a restart");
        let hotkey = changes
            .iter()
            .find(|c| c.key == "hotkey.actions.capture")
            .unwrap();
        // §5.20.2 第二句「需要重启的设置应明确标记」逐枚验的那一列里，`hotkey.actions.*` 是**漏标**
        // 的那一枚：绑定只在 `hotkey::install` 里上一次，而那一只手全仓只有启动时调一次（见
        // [`RESTART_EVIDENCE`] 与 [`restart_audit`]）。这一行从前钉的是「热键不用重启」，而那句
        // 是这一枚标记没标时顺带写下的话，不是验出来的 —— 现在钉的是真的那一面。
        assert!(
            hotkey.restart_required,
            "改绑定要重挂 RegisterHotKey，而那只手只在启动时挂"
        );
        assert_eq!(hotkey.from, "ctrl+shift+a");
        assert_eq!(hotkey.to, "f13");

        // The suggested range from §5.20.3 does not touch general settings.
        let scoped = current.preview_import(&incoming, ImportScope::Suggested);
        assert_eq!(scoped.len(), 1, "{scoped:?}");
        assert_eq!(scoped[0].key, "hotkey.actions.capture");
        assert!(ImportScope::Suggested.covers("annotation.palette"));
        assert!(ImportScope::Suggested.covers("output.naming_template"));
        assert!(ImportScope::Suggested.covers("appearance.theme"));
        assert!(!ImportScope::Suggested.covers("history.max_items"));
        assert!(!ImportScope::Suggested.covers("general.show_tray"));

        let mut applied = current.clone();
        applied
            .apply_import(&incoming, ImportScope::Suggested)
            .unwrap();
        assert_eq!(applied.hotkey.actions["capture"], "f13");
        assert_eq!(applied.general.language, "auto", "out of scope stays put");
        applied.apply_import(&incoming, ImportScope::All).unwrap();
        assert_eq!(applied.general.language, "en");
        // Importing yourself changes nothing.
        assert!(current
            .preview_import(&current, ImportScope::All)
            .is_empty());
    }

    #[test]
    fn hotkey_exclusions_keep_their_shape() {
        let text = r#"
[hotkey]
actions = { capture = "ctrl+shift+a" }

[[hotkey.exclusions]]
app = "game.exe"
actions = [ "capture" ]

[[hotkey.exclusions]]
app = "C:/Apps/presentation.exe"
actions = []
"#;
        let c: Config = toml_edit::de::from_str(text).unwrap();
        assert_eq!(c.hotkey.exclusions.len(), 2);
        assert_eq!(c.hotkey.exclusions[0].app, "game.exe");
        assert_eq!(c.hotkey.exclusions[0].actions, ["capture"]);
        assert!(c.hotkey.exclusions[1].actions.is_empty());
        // A rule is a table, so the text path refuses to scribble over it, and a
        // rule with no app is dropped instead of matching everything.
        let mut c2 = c.clone();
        assert!(c2.set_str("hotkey.exclusions", "x").is_err());
        c2.hotkey.exclusions.push(AppExclusion::default());
        let warnings = c2.validate();
        assert!(
            warnings.iter().any(|w| w.contains("exclusions")),
            "{warnings:?}"
        );
        assert_eq!(c2.hotkey.exclusions.len(), 2);
    }

    /// §5.16.2 规则第二句那一枚「快速添加当前前台程序」在档上的往返。
    ///
    /// 这一格钉的不是「能 push 一枚结构体」，而是**页上那一只手有没有一条完整的路走到盘再走回
    /// 来**：`add_exclusion` 添的那一枚，`flat()` 要 render 得出（页上那一行跟着变）、`save`/`load`
    /// 要带得回（下一次开档还是这一位）、`export_text`/`parse_import` 也要带得回（§5.20.3 那两枚钮
    /// 走的是这一条）。三样缺一样，页上那句「已加进来」说的就只是一次内存里的动作。
    ///
    /// 「重复」那一支选的是**拒**（交回 [`ExclusionAdd::Already`]，一枚规则也不添），而不是默默再
    /// 塞一条一模一样的：`AppExclusion` 没有 id，两条同名的规则在 `[[hotkey.exclusions]]` 里除了
    /// 让人手工删档时多删一行以外什么都不干，而页上连删的那一只手还没有 —— 那一趟要是报「已添加」，
    /// 用户按三次就会在三条重复规则上读到同一句成功。
    #[test]
    fn the_foreground_app_added_from_the_page_comes_back_as_a_rule() {
        let mut c = Config::default();
        assert!(c.hotkey.exclusions.is_empty(), "出厂不许替用户挡着谁");

        // ① 添一枚：门没有为这份档改过任何东西，所以校正那一串是空的。
        let added = c.add_exclusion("notepad.exe").unwrap();
        assert_eq!(added, ExclusionAdd::Added(vec![]), "{added:?}");
        assert_eq!(c.hotkey.exclusions.len(), 1);
        assert_eq!(c.hotkey.exclusions[0].app, "notepad.exe");
        assert!(
            c.hotkey.exclusions[0].actions.is_empty(),
            "快速添加添的是「整个静音」那一枚形状"
        );
        // 页上那一行读得到的正是它（`Row::text` 拿的就是 `flat()` 的这一串）。
        assert!(
            c.get_str(EXCLUSION_KEY)
                .is_some_and(|v| v.contains("notepad.exe")),
            "{:?}",
            c.get_str(EXCLUSION_KEY)
        );

        // ② 重复：大小写与路径两种写法都算同一位 —— 判断用的是 `app_matches`，与运行时静音读的是
        // 同一位作者，页上不第二次数。
        for again in [
            "NOTEPAD.EXE",
            " notepad.exe ",
            r"C:\Windows\System32\notepad.exe",
        ] {
            let out = c.add_exclusion(again).unwrap();
            assert_eq!(
                out,
                ExclusionAdd::Already {
                    matched: "notepad.exe".to_string()
                },
                "{again} 被当成一件新事添了进去"
            );
        }
        assert_eq!(c.hotkey.exclusions.len(), 1, "重复的那几趟一枚也不许多添");

        // ③ 一枚管到底的通配规则，也已经管住了这一位。
        let mut any = Config::default();
        any.hotkey.exclusions.push(AppExclusion {
            app: "*".to_string(),
            actions: Vec::new(),
        });
        assert_eq!(
            any.add_exclusion("game.exe").unwrap(),
            ExclusionAdd::Already {
                matched: "*".to_string()
            },
            "通配已经挡着他了，还添一条就是页上多说一句"
        );
        // 而一枚只管住 `capture` 的规则挡不下「整个静音」，那一趟是真添了一枚。
        let mut one = Config::default();
        one.hotkey.exclusions.push(AppExclusion {
            app: "game.exe".to_string(),
            actions: vec!["capture".to_string()],
        });
        assert!(matches!(
            one.add_exclusion("game.exe").unwrap(),
            ExclusionAdd::Added(_)
        ));
        assert_eq!(one.hotkey.exclusions.len(), 2);

        // ④ 没有名字的写不进档：`validate` 会把那条规则删掉，所以让页上此刻就报「已添加」是撒谎。
        for blank in ["", "   "] {
            let mut c3 = Config::default();
            assert!(
                c3.add_exclusion(blank).is_err(),
                "空名字 {blank:?} 竟然添成了"
            );
            assert!(c3.hotkey.exclusions.is_empty(), "被拒的那一趟动了档");
        }

        // ⑤ 落盘再开回来，还是这一位。tempdir，绝不碰用户那份 config.toml。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        c.save(&path).unwrap();
        let (back, report) = Config::load(&path);
        assert_eq!(report.source, LoadSource::File);
        assert_eq!(back.hotkey.exclusions, c.hotkey.exclusions);

        // ⑥ §5.20.3 那两枚钮走的那一条路：导出那份文本，再当别人的档读回来。
        let text = c.export_text().unwrap();
        let (incoming, warnings) = Config::parse_import(&text).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(incoming.hotkey.exclusions, c.hotkey.exclusions);
        let mut applied = Config::default();
        applied
            .apply_import(&incoming, ImportScope::All)
            .expect("应用那份档");
        assert_eq!(applied.hotkey.exclusions, c.hotkey.exclusions);
        assert!(applied
            .get_str(EXCLUSION_KEY)
            .is_some_and(|v| v.contains("notepad.exe")));
    }

    /// §5.16.1 第十三名 `[hotkey] command` 的存储形状：命令行是**一枚**字符串，不是一串参数。
    ///
    /// 这一格钉的不是「能存」，而是「存的时候没有人替它拆」：数组那一族的 `flat()` 用 `, ` 连接、
    /// `coerce()` 按 `,` 拆回（上面 `hotkey_exclusions_keep_their_shape` 与
    /// `a_saved_scheme_round_trips_through_the_key_the_page_writes` 两处都是那一形状），而一条
    /// 命令行里最不缺的就是逗号 —— 一旦拆过，`--export,--out` 就成了两枚参数，而那正是
    /// [`crate::tasks`] 判据要防的第一件事（它只在第一处空白切一刀，其余原样递给对面）。这一层
    /// 没有第二道闸，所以坏的那一支在这里照存：多行、越界、看不见字符都由 [`crate::tasks`] 说出
    /// 它那一句，档不第二次判（#281）。
    #[test]
    fn the_command_line_is_stored_as_one_string_and_never_split() {
        let mut c = Config::default();
        // 出厂空着，而且空着也是一行：页上要能写下第一条命令，先得有那一格可以写字。
        assert_eq!(c.hotkey.command, "", "出厂不许替用户写一条示例");
        assert_eq!(
            c.get_str("hotkey.command").as_deref(),
            Some(""),
            "没写过命令时页上没有这一行，用户没有地方写第一条"
        );
        // 逗号、反斜杠、引号、中文、开关——一枚也不许被拆走或改写。
        let line = r#"tool.exe --export,--out "C:\我的 图\a.txt" -y"#;
        c.set_str("hotkey.command", line).unwrap();
        assert_eq!(c.hotkey.command, line);
        assert_eq!(c.get_str("hotkey.command").as_deref(), Some(line));
        // 判据不在这儿：这一格既不拦多行也不拦越界，所以它也不会为此说一句。
        let warnings = c.validate();
        assert!(
            !warnings.iter().any(|w| w.contains("hotkey.command")),
            "{warnings:?}"
        );
        // 出档 → 进档：同一串字，一个字符不多、一个不少。
        let text = c.export_text().unwrap();
        let (back, warns) = Config::parse_import(&text).unwrap();
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(back.hotkey.command, line);
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut c = Config::default();
        for i in 0..5 {
            c.capture.magnifier_zoom = 2 + i;
            c.save(&path).unwrap();
        }
        assert_eq!(c.capture.magnifier_zoom, 6);
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n != FILE_NAME)
            .collect();
        assert!(left.is_empty(), "stray temp files: {left:?}");
        let (back, _) = Config::load(&path);
        assert_eq!(back.capture.magnifier_zoom, 6);
    }

    #[test]
    fn naming_template_tokens_are_the_ones_the_engine_knows() {
        assert_eq!(unknown_token("%p_%Y-%m-%d_%H%M%S_%i_%w_%h_%t_%B_%%"), None);
        assert_eq!(unknown_token("%Q"), Some('Q'));
        assert_eq!(unknown_token("%"), Some('%'));
        assert_eq!(unknown_token("no tokens"), None);
    }

    /// §5.19.1's three arms against §5.19.2's override, in one grid: only `System` asks the
    /// machine, and a silent machine must not turn into a third look.
    #[test]
    fn system_expands_while_manual_theme_ignores_the_machine() {
        use Theme::{Dark, Light, System};
        assert_eq!(resolve_theme(System, Some(true)), Light);
        assert_eq!(resolve_theme(System, Some(false)), Dark);
        assert_eq!(resolve_theme(System, None), Light);
        assert_eq!(resolve_theme(Light, Some(true)), Light);
        assert_eq!(resolve_theme(Light, Some(false)), Light);
        assert_eq!(resolve_theme(Light, None), Light);
        assert_eq!(resolve_theme(Dark, Some(true)), Dark);
        assert_eq!(resolve_theme(Dark, Some(false)), Dark);
        assert_eq!(resolve_theme(Dark, None), Dark);
    }

    /// The invariant everything downstream leans on: the answer to "which look is this" is
    /// always drawable. A machine that says nothing is not allowed to leak `System` past
    /// this function, because every consumer of the return value has to paint something.
    #[test]
    fn the_resolver_never_hands_system_back() {
        for requested in [Theme::System, Theme::Light, Theme::Dark] {
            for system_light in [Some(true), Some(false), None] {
                let got = resolve_theme(requested, system_light);
                assert_ne!(
                    got,
                    Theme::System,
                    "{requested:?} + {system_light:?} -> {got:?}"
                );
                assert!(matches!(got, Theme::Light | Theme::Dark));
            }
        }
    }

    /// §5.18.2 的四枚键默认就是本轮改码**之前**这台机器真实在跑的那一身：左键与双击都起截图、
    /// 右键开菜单、中键没人理会。断言写成字面值而不是引用 `Tray::default()` —— 这一条证的正是
    /// "变成可配"这件事本身没有顺手改掉用户已经习惯的点击。
    #[test]
    fn the_tray_defaults_are_the_clicks_this_machine_already_made() {
        let tray = Tray::default();
        assert_eq!(tray.left, "capture");
        assert_eq!(tray.double, "capture");
        assert_eq!(tray.middle, "none");
        assert_eq!(tray.right, "menu");
        assert!(Config::default().tray_violations().is_empty());
    }

    /// 文档把同一件事说了两遍（"至少保留一种…"与"不允许所有…"），判据只有一枚，所以违规
    /// 只该交出**一句**。两句就是两份会各自漂的口径，而它们一旦不一致，"合规"就没有证人。
    #[test]
    fn a_tray_with_no_way_in_says_it_once() {
        let mut cfg = Config::default();
        cfg.tray.left = "capture".into();
        cfg.tray.double = "paste_pin".into();
        cfg.tray.middle = "none".into();
        cfg.tray.right = "toggle_pins".into();
        let bad = cfg.tray_violations();
        assert_eq!(bad.len(), 1, "{bad:?}");
        // 一句里把四枚逐名交出来：只说"有一枚不合规"答不出是哪四枚配成了这一身。
        for name in [
            "left=capture",
            "double=paste_pin",
            "middle=none",
            "right=toggle_pins",
        ] {
            assert!(bad[0].contains(name), "{name} 不在证词里：{}", bad[0]);
        }
    }

    /// 违规的档开档时不许原样留着：没有一扇窗进得去的托盘是一台用不起来的机器，所以
    /// `validate` 把右键落回 `menu`，并把这一手作为一条 warning 说出来（改过用户写的值
    /// 就要报，与这一段里其余的归一同一条口径）。
    #[test]
    fn validate_keeps_a_window_open_and_says_so() {
        let mut cfg = Config::default();
        cfg.tray.left = "none".into();
        cfg.tray.double = "none".into();
        cfg.tray.middle = "none".into();
        cfg.tray.right = "capture".into();
        let warnings = cfg.validate();
        assert_eq!(cfg.tray.right, "menu", "右键没有落回 menu");
        assert!(
            warnings.iter().any(|l| l.contains("tray.right falls back")),
            "{warnings:?}"
        );
        assert!(
            cfg.tray_violations().is_empty(),
            "修完还违规：{:?}",
            cfg.tray
        );
    }

    /// §5.4.4 与 §5.4.5 的默认值就是"白板"这两个字在本机上的字面意思：一块与屏幕一样大的
    /// 白色画布。默认值不承担 §5.4.5 —— 透明必须是用户打出来的 `transparent`。
    #[test]
    fn the_whiteboard_defaults_are_a_white_screen_sized_board() {
        let cfg = Config::default();
        assert_eq!(cfg.whiteboard.background, "#FFFFFF");
        assert_eq!(cfg.whiteboard.size, "screen");
        assert_eq!(
            cfg.get_str("whiteboard.background").as_deref(),
            Some("#FFFFFF"),
            "页上读不到就是没有这一行"
        );
        assert_eq!(
            cfg.get_str("whiteboard.size").as_deref(),
            Some("screen"),
            "页上读不到就是没有这一行"
        );
        assert_eq!(
            cfg.whiteboard.surface().unwrap(),
            Surface::Solid([255, 255, 255])
        );
        assert_eq!(cfg.whiteboard.size().unwrap(), Size::Screen);
        assert!(cfg.whiteboard.violations().is_empty());
        for name in ["background", "size"] {
            assert!(
                !needs_restart(&format!("whiteboard.{name}")),
                "whiteboard.{name} 被标成需重启"
            );
        }
    }

    /// 读不出的两枚键各落回各的默认，各说一句：合并成一句就答不出是哪一枚键出的错，而
    /// 这一族的归一与 `[tray]` 那一段同一条口径 —— 改过用户写的值就要报。
    #[test]
    fn a_whiteboard_that_cannot_be_read_falls_back_and_says_so() {
        let mut cfg = Config::default();
        cfg.whiteboard.background = "beige".into();
        cfg.whiteboard.size = "0x0".into();
        let warnings = cfg.validate();
        assert_eq!(cfg.whiteboard.background, "#FFFFFF", "底色没有落回默认");
        assert_eq!(cfg.whiteboard.size, "screen", "尺寸没有落回默认");
        for key in ["whiteboard.background", "whiteboard.size"] {
            assert!(
                warnings.iter().any(|l| l.starts_with(&format!("{key}:"))),
                "{key} 没被说出来：{warnings:?}"
            );
        }
        assert!(cfg.whiteboard.violations().is_empty(), "修完还违规");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    /// 页上打字那一格当场拒（与 `the_box_refuses_an_action_that_is_not_on_the_list` 同一
    /// 条 §5.20.2 的理由），且拒的是**产品真正会用的那枚解析器** —— 闸若自带一份名单，
    /// 盘上就能存下一页解释不了的值。
    #[test]
    fn the_whiteboard_box_refuses_what_the_product_cannot_read() {
        let mut cfg = Config::default();
        for (key, bad) in [
            ("whiteboard.background", "beige"),
            ("whiteboard.background", "1280x720"),
            ("whiteboard.size", "big"),
            ("whiteboard.size", "#FFFFFF"),
        ] {
            let err = cfg.set_str(key, bad).unwrap_err();
            match err {
                ConfigError::InvalidValue { key: k, reason } => {
                    assert_eq!(k, key);
                    assert!(reason.contains(bad), "{reason}");
                }
                other => panic!("交回的是 {other:?}，不是 InvalidValue"),
            }
        }
        assert_eq!(
            cfg.whiteboard.background, "#FFFFFF",
            "被拒的一格不该已经改掉"
        );
        assert_eq!(cfg.whiteboard.size, "screen", "被拒的一格不该已经改掉");
        // §5.4.5 的那一个字面必须收得下，拒掉它就是把透明白板拒出了配置。
        cfg.set_str("whiteboard.background", "transparent").unwrap();
        cfg.set_str("whiteboard.size", "1280x720").unwrap();
        assert_eq!(cfg.whiteboard.surface().unwrap(), Surface::Transparent);
        assert_eq!(
            cfg.whiteboard.size().unwrap(),
            Size::Canvas {
                width: 1280,
                height: 720
            }
        );
        assert!(cfg.whiteboard.violations().is_empty());
    }

    /// 两枚键要能跟着一次写档、一次读档走完全程，否则 §5.4.4 的"选颜色"就只是页上的
    /// 一格装饰 —— 与 `the_tray_keys_travel_through_the_file` 同一格判据。
    #[test]
    fn the_whiteboard_keys_travel_through_the_file() {
        let mut cfg = Config::default();
        cfg.set_str("whiteboard.background", "#102030").unwrap();
        cfg.set_str("whiteboard.size", "800x600").unwrap();
        let flat = cfg.flat().unwrap();
        for key in ["whiteboard.background", "whiteboard.size"] {
            assert!(flat.contains_key(key), "{key} 不在 flat() 里");
        }
        assert_eq!(
            flat.get("whiteboard.background").map(String::as_str),
            Some("#102030"),
            "flat() 里那格不是打进去的值"
        );
        let text = cfg.export_text().unwrap();
        assert!(text.contains("[whiteboard]"), "段头没了：\n{text}");
        let (back, warnings) = Config::parse_import(&text).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.whiteboard, cfg.whiteboard, "走了一趟档就不是那一身了");
    }

    /// 顺序要紧：名单外的值先落回本键默认，再问"至少留一扇进得去的入口"。反过来写的话
    /// 一个 `nope` 会被算成"进不去"，报出来的原因就是假的那一个 —— 这一条测的是假原因出不来。
    #[test]
    fn an_unknown_tray_action_is_normalized_before_the_rule_runs() {
        let mut cfg = Config::default();
        cfg.tray.right = "nope".into();
        let warnings = cfg.validate();
        assert_eq!(
            cfg.tray.right,
            Tray::default_entry("right"),
            "没有落回本键默认"
        );
        assert!(
            warnings
                .iter()
                .any(|l| l.contains("tray.right") && l.contains("not a tray action")),
            "{warnings:?}"
        );
        assert!(
            !warnings.iter().any(|l| l.contains("opens the tray menu")),
            "归一之后不该再有违规：{warnings:?}"
        );
    }

    /// §5.18.2 的四枚键即时生效：都不进 `RESTART_KEYS`。挂了 `（需重启）` 的设置页行是
    /// §5.20.2 第二条的判据，标错与漏标同样是错的读数。
    #[test]
    fn no_tray_key_owes_a_restart() {
        for name in ["left", "double", "middle", "right"] {
            assert!(
                !needs_restart(&format!("tray.{name}")),
                "tray.{name} 被标成需重启"
            );
        }
    }

    /// 页面上那一格打字进去时就要拦下，而不是等下一次开档才变成一句关于整份档的抱怨 ——
    /// §5.18.2 是一条会拦人的规则，拦在被打字的那一格上才有用。
    #[test]
    fn the_box_refuses_an_action_that_is_not_on_the_list() {
        let mut cfg = Config::default();
        let err = cfg.set_str("tray.middle", "nope").unwrap_err();
        match err {
            ConfigError::InvalidValue { key, reason } => {
                assert_eq!(key, "tray.middle");
                assert!(reason.contains("nope"), "{reason}");
                assert!(reason.contains("menu"), "名单没有说出来：{reason}");
            }
            other => panic!("交回的是 {other:?}，不是 InvalidValue"),
        }
        assert_eq!(cfg.tray.middle, "none", "被拒的一格不该已经改掉");
        assert!(cfg.set_str("tray.middle", "settings").is_ok());
        assert_eq!(cfg.tray.middle, "settings");
        assert!(cfg.tray_violations().is_empty());
    }

    /// 四枚键得能跟着一次写档、一次读档走完全程：`flat()` 是导出与预览那两格数的分母，
    /// 段没进去就是"键在档里没人读"那一族的又一次发病。
    #[test]
    fn the_tray_keys_travel_through_the_file() {
        let mut cfg = Config::default();
        cfg.set_str("tray.left", "settings").unwrap();
        cfg.set_str("tray.double", "show_all_pins").unwrap();
        let flat = cfg.flat().unwrap();
        for key in ["tray.left", "tray.double", "tray.middle", "tray.right"] {
            assert!(flat.contains_key(key), "{key} 不在 flat() 里");
        }
        assert_eq!(
            flat.get("tray.left").map(String::as_str),
            Some("settings"),
            "flat() 里那格不是打进去的值"
        );
        let text = cfg.export_text().unwrap();
        let (back, warnings) = Config::parse_import(&text).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.tray, cfg.tray, "走了一趟档就不是那一身了");
    }

    /// [`Config::get_bool`] 交的是**档里存着的那一位**，不是 `flat()` 渲染出来的那串字。
    ///
    /// 这一枚判错的方向是「把文本键当开关」：`set_str` 对文本键是把串原样存下来，语法闸不会
    /// 拦，所以页上要是靠自己猜文本，按一下就写掉一枚字体名。数与串都算不上一枚开关。
    #[test]
    fn a_boolean_is_decided_by_the_stored_type_not_by_its_text() {
        let mut cfg = Config::default();
        cfg.set_str("annotation.font_family", "true").unwrap();
        assert_eq!(
            cfg.get_str("annotation.font_family").as_deref(),
            Some("true"),
            "这一问的对面没了：页上那格写着 true 却是一枚文本"
        );
        assert_eq!(cfg.get_bool("annotation.font_family"), None);
        assert_eq!(cfg.get_bool("history.max_items"), None, "整数不是开关");
        assert_eq!(cfg.get_bool("pin.alpha_bg"), None, "枚举串不是开关");
        assert_eq!(cfg.get_bool("hot_corner.corners.top_left"), None);
        assert_eq!(cfg.get_bool("general.language"), None);
        assert_eq!(
            cfg.get_bool("nope.nope"),
            None,
            "不在册的键不许猜出一枚开关"
        );

        assert_eq!(cfg.get_bool("hot_corner.enabled"), Some(false));
        cfg.set_str("hot_corner.enabled", "true").unwrap();
        assert_eq!(cfg.get_bool("hot_corner.enabled"), Some(true));
        assert_eq!(cfg.get_str("hot_corner.enabled").as_deref(), Some("true"));
        assert!(
            cfg.set_str("hot_corner.enabled", "maybe").is_err(),
            "bool 键的门此刻不拦任何文本：上面那两枚读数就没有对面"
        );
    }

    /// §5.19.4「支持调整工具栏方向和位置」：**位置**那一枚是闭集、**方向**不是第六枚键而是
    /// 落点的函数。这两句坏各长什么样：名单外的一枚落点存进档（下一次开档才被归一，页上却
    /// 已经画着它），以及把 `left` 那一头画成一排。
    #[test]
    fn toolbar_edge_and_side_are_one_function_not_two_lists() {
        // 名单里每一枚落点都答得出方向，且方向只有那两枚词。
        for edge in TOOLBAR_PLACEMENTS {
            let side = toolbar_side(edge, 800, 200);
            assert!(TOOLBAR_SIDES.contains(&side), "{edge} 走出了 {side}");
        }
        assert_eq!(
            toolbar_edge("top", 10, 9_000),
            "top",
            "钉住的上边不许被选区长短挪走"
        );
        assert_eq!(toolbar_edge("right", 10, 9_000), "right");
        assert_eq!(
            toolbar_side("left", 9_000, 10),
            "vertical",
            "左边那一头是一列"
        );
        assert_eq!(toolbar_side("bottom", 9_000, 10), "horizontal");
        // `auto` 吃长短边：横长贴上、竖长贴左，而**没框出来**（两枚 0）也必须答得出落点。
        assert_eq!(toolbar_edge("auto", 800, 200), "top");
        assert_eq!(toolbar_edge("auto", 200, 800), "left");
        assert_eq!(toolbar_edge("auto", 0, 0), "top");
        assert_eq!(toolbar_side("auto", 200, 800), "vertical");
        // 认不出的字面走 `auto` 那一臂，而不是留下一枚谁也画不出来的落点。
        assert_eq!(toolbar_edge("sideways", 800, 200), "top");
        // 归一那道闸读的是同一份名单：这一条是「页上递的候选」与「档里存的」不会分家的对面。
        let mut c = Config::default();
        let before = c.appearance.toolbar_placement.clone();
        let refused = c
            .set_str("appearance.toolbar_placement", "sideways")
            .unwrap_err();
        assert!(refused.to_string().contains("sideways"), "{refused}");
        assert_eq!(c.appearance.toolbar_placement, before, "拒了的写不许碰值");
        // 大小写与尾巴上的空格都认，认下按名单的拼法存。
        c.set_str("appearance.toolbar_placement", " Left ").unwrap();
        assert_eq!(c.appearance.toolbar_placement, "left");
        // 从**档里**读来的那一枚没经过页上这一格，仍由 `validate` 归一，且说出来。
        c.appearance.toolbar_placement = "sideways".into();
        let w = c.validate();
        assert_eq!(c.appearance.toolbar_placement, TOOLBAR_PLACEMENT_DEFAULT);
        assert!(
            w.iter().any(|l| l.contains("appearance.toolbar_placement")),
            "{w:?}"
        );
        for edge in ["auto", "top", "bottom", "left", "right"] {
            assert!(c.set_str("appearance.toolbar_placement", edge).is_ok());
            assert_eq!(c.appearance.toolbar_placement, edge);
        }
    }

    /// §5.19.4「支持调整界面字体和字号」里**字号**那两端的界：过界要夹且说出来，界内不许多话。
    /// 顺带钉住磅 → 像素那一定率（9 磅 = 12 像素，与本仓界面此刻写死的那一路 `pixelSize` 同数）。
    #[test]
    fn ui_font_size_is_clamped_at_both_ends_and_px_rate_is_the_default_one() {
        assert_eq!(ui_font_px(9), 12);
        assert_eq!(ui_font_px(UI_FONT_MIN), 8);
        assert_eq!(ui_font_px(UI_FONT_MAX), 64);

        let mut c = Config::default();
        assert_eq!(
            c.appearance.ui_font_size, 9,
            "出厂那一枚正是界面在画的那一号"
        );
        let over = c.set_str("appearance.ui_font_size", "500").unwrap();
        assert_eq!(c.appearance.ui_font_size, UI_FONT_MAX);
        assert!(
            over.iter().any(|l| l.contains("appearance.ui_font_size")),
            "{over:?}"
        );
        let under = c.set_str("appearance.ui_font_size", "0").unwrap();
        assert_eq!(c.appearance.ui_font_size, UI_FONT_MIN);
        assert!(
            under.iter().any(|l| l.contains("appearance.ui_font_size")),
            "{under:?}"
        );
        let ok = c.set_str("appearance.ui_font_size", "14").unwrap();
        assert!(ok.is_empty(), "界内不许有话：{ok:?}");
        assert_eq!(c.appearance.ui_font_size, 14);
        assert_eq!(c.get_str("appearance.ui_font_size").as_deref(), Some("14"));
    }

    /// §5.19.4 里**字体**那一半（`appearance.ui_font_family`）。空的与只有空白的那一枚不是一枚
    /// 字体名 —— 界面拿它会退回 Qt 自己的那一套，而那一套每台机器不一样。
    #[test]
    fn ui_font_family_has_a_reader_side_floor_and_survives_a_round_trip() {
        let mut c = Config::default();
        assert_eq!(c.appearance.ui_font_family, UI_FONT_FAMILY_DEFAULT);
        // 这枚键在 `flat()` 里，所以设置页上有它那一行；不在册的键页上是点不到的。
        assert_eq!(
            c.get_str("appearance.ui_font_family").as_deref(),
            Some(UI_FONT_FAMILY_DEFAULT)
        );
        assert!(c.has_key("appearance.ui_font_family"));
        // 改名、落盘、再读回来：导出/导入那条腿（§5.20.3 的范围建议里点名的「主题」）不许把它丢了。
        c.set_str("appearance.ui_font_family", "Arial").unwrap();
        assert_eq!(c.appearance.ui_font_family, "Arial");
        let text = c.export_text().unwrap();
        let (back, warns) = Config::parse_import(&text).unwrap();
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(back.appearance.ui_font_family, "Arial");
        assert!(ImportScope::Suggested.covers("appearance.ui_font_family"));
        // 空串与空白都夹回出厂那一枚，且**说出来**。
        for blank in ["", "   "] {
            let mut e = Config::default();
            let fixes = e.set_str("appearance.ui_font_family", blank).unwrap();
            assert_eq!(
                e.appearance.ui_font_family, UI_FONT_FAMILY_DEFAULT,
                "{blank:?}"
            );
            assert!(
                fixes
                    .iter()
                    .any(|l| l.contains("appearance.ui_font_family")),
                "{fixes:?}"
            );
        }
        // 开关那一判（`get_bool`）不许把这枚文本键认成开关：`annotation.font_family = "true"`
        // 那样的合法值一旦被当成开关，点一下就写掉了人家一整格。
        assert_eq!(
            Config::default().get_bool("appearance.ui_font_family"),
            None
        );
    }

    /// §5.19.2 步骤 1「用户选择浅色或深色主题」：那两枚加上 §5.19.1 那一枚「跟随系统」是闭集，
    /// 拼错的字面必须在**被打字那一格**拦下，而不是到 `from_document` 那里变成一句关于整份文件的
    /// 抱怨（`pin.alpha_bg` / `capture.size_unit` 两轮立过同一块牌子）。
    #[test]
    fn theme_names_are_a_closed_set_at_the_widget() {
        let mut c = Config::default();
        for name in THEME_IDS {
            c.set_str("appearance.theme", name).unwrap();
        }
        assert_eq!(c.appearance.theme, Theme::Dark, "最后一按是 dark");
        assert_eq!(c.get_str("appearance.theme").as_deref(), Some("dark"));
        // 大小写与尾巴上的空格都认，认下之后按名单的拼法存 —— 页上打 `Dark ` 与打 `dark`
        // 存的是同一枚字面，否则下一次开档读回来的是两行不同的文本。
        c.set_str("appearance.theme", "Light ").unwrap();
        assert_eq!(c.appearance.theme, Theme::Light);
        assert_eq!(c.get_str("appearance.theme").as_deref(), Some("light"));
        // 拦下的那一趟：值与档都不许动。
        let was = c.appearance.theme;
        let refused = c.set_str("appearance.theme", "nope").unwrap_err();
        assert!(refused.to_string().contains("nope"), "{refused}");
        assert_eq!(c.appearance.theme, was, "拒了的写不许碰值");
    }

    /// §5.19.4 最后那句「所有主题下均应满足基本可读性和对比度要求」+ §7.4 那一枚对比度：
    /// 夹法只动明度、两套餐具各过一遍底线、非法颜色不许长成第二枚事实。
    #[test]
    fn accent_guard_keeps_hue_and_meets_the_floor_on_both_surfaces() {
        // 出厂那一枚在两套餐具上本来就够了 ⇒ 夹过之后一字不改（不许为了显出手腕把色挪一格）。
        for surface in [SURFACE_LIGHT, SURFACE_DARK] {
            let kept = accent_on(ACCENT_DEFAULT, surface);
            assert_eq!(kept, ACCENT_DEFAULT, "{surface} 上出厂色本就够了");
            assert!(
                contrast_ratio(&kept, surface).unwrap() >= ACCENT_MIN_CONTRAST,
                "{surface}: {kept}"
            );
        }
        // 一枚与浅底几乎同色的主题色（纯白对 `#F2F4F7` 只有 1.1:1）：夹到够为止，且夹出来的
        // 那一枚比底**暗** —— 浅主题往暗处走。
        let sunk = accent_on("#FFFFFF", SURFACE_LIGHT);
        assert!(
            contrast_ratio(&sunk, SURFACE_LIGHT).unwrap() >= ACCENT_MIN_CONTRAST,
            "{sunk}"
        );
        assert_ne!(sunk, "#FFFFFF");
        assert!(
            luminance(&sunk).unwrap() < luminance(SURFACE_LIGHT).unwrap(),
            "浅底该往暗处夹"
        );
        // 深主题里同一枚要往**亮**处走：同一把尺、两枚归宿，这就是「所有主题下」那句话的对面。
        let lifted = accent_on("#2A2E36", SURFACE_DARK);
        assert!(
            contrast_ratio(&lifted, SURFACE_DARK).unwrap() >= ACCENT_MIN_CONTRAST,
            "{lifted}"
        );
        assert!(
            luminance(&lifted).unwrap() > luminance(SURFACE_DARK).unwrap(),
            "深底该往亮处夹"
        );
        // 色相不许丢：一枚绿夹完还得是那枚绿（饱和度与色相都留着，动的只有明度）。
        let (h0, s0, l0) = crate::colors::rgb_to_hsl(0, 0xFF, 0);
        let [r2, g2, b2, _] = crate::colors::parse(&accent_on("#00FF00", SURFACE_LIGHT)).unwrap();
        let (h1, s1, l1) = crate::colors::rgb_to_hsl(r2, g2, b2);
        assert!((h0 - h1).abs() < 1.0, "色相动了：{h0} -> {h1}");
        assert!((s0 - s1).abs() < 1.0, "饱和度动了：{s0} -> {s1}");
        assert!(l1 < l0, "浅底上明度只许往下走：{l0} -> {l1}");
        // 读不出来的那一枚走出厂归宿（而且仍过底线），非法底色则原样交回，不发明第三枚字面。
        assert_eq!(accent_on("not a colour", SURFACE_LIGHT), ACCENT_DEFAULT);
        assert!(
            contrast_ratio(&accent_on("not a colour", SURFACE_DARK), SURFACE_DARK).unwrap()
                >= ACCENT_MIN_CONTRAST
        );
        assert_eq!(
            accent_on(ACCENT_DEFAULT, "also not a colour"),
            ACCENT_DEFAULT
        );
        // 对比度本体：黑与白是 21:1 这一枚众所皆知的对面，读不出来的交回 None 而不是 0。
        assert!((contrast_ratio("#000000", "#FFFFFF").unwrap() - 21.0).abs() < 0.05);
        assert_eq!(
            contrast_ratio("#000000", "#000000").map(|c| c.round()),
            Some(1.0)
        );
        assert!(contrast_ratio("nope", "#FFFFFF").is_none());
    }

    /// 整段 `[appearance]` 此刻在页上、在导出里各有几枚：这一格盯着**枚数**，加一枚键而忘了
    /// 接线（`flat` 认不到、或建议范围不覆盖）在这一格就红，而不是等到用户发现导出档里没有它。
    #[test]
    fn the_appearance_section_is_nine_keys_all_reachable() {
        let c = Config::default();
        let flat = c.flat().unwrap();
        let mine: Vec<&String> = flat
            .keys()
            .filter(|k| k.starts_with("appearance."))
            .collect();
        assert_eq!(mine.len(), 9, "九枚键各一行：{:?}", mine);
        for key in [
            "appearance.theme",
            "appearance.accent",
            "appearance.toolbar_placement",
            "appearance.ui_font_size",
            "appearance.ui_font_family",
            "appearance.magnifier_border",
            "appearance.magnifier_corner_radius",
            "appearance.magnifier_foreground",
            "appearance.magnifier_background",
        ] {
            assert!(
                flat.contains_key(key),
                "{key} 不在 `flat()` 里 ⇒ 页上点不到它"
            );
            assert!(ImportScope::Suggested.covers(key), "{key} 会被导出丢掉");
        }
    }

    // ================= §5.20.3 分组配置与导入导出（本轮新接的那一族）=================
    //
    // 这一族测试不得碰用户真实的 config.toml：所有落盘都在 `tempfile::tempdir()` 里，与
    // `save_rewrites_values_and_keeps_comments` 那一格同一写法。

    /// 三枚贴图（编号 1、2、3）与机器上的两枚组：`甲` 带着 1 与 2。
    fn pins_and_two_groups() -> PinSet {
        let mut set = PinSet::default();
        let frame = crate::frame::Frame::filled(8, 8, [10, 20, 30, 255]).unwrap();
        for id in 1..=3 {
            set.add(
                crate::pin::PinItem::from_frame(
                    id,
                    &frame,
                    crate::geometry::PhysPoint::new(0, 0),
                    &Pin::default(),
                )
                .unwrap(),
            );
        }
        let a = set.create_group("甲").unwrap();
        set.assign_group(&[1, 2], Some(a)).unwrap();
        set.create_group("乙").unwrap();
        set
    }

    /// 往返判据（本轮要钉的就是这一句）：**导出 → 导入 → 再导出**，逐字节相同，而分组那一族
    /// 就在这一趟里带着走。两枚机器组、中文组名、成员编号、以及一枚空组，都不许在路上掉。
    #[test]
    fn the_group_family_round_trips_byte_for_byte() {
        let set = pins_and_two_groups();
        let mut c = Config::default();
        // 导出带的是机器上那一份（桥那一侧就是这么递的），而不是一份自己编的名单。
        assert!(
            c.set_roster(&roster_of(&set)).is_empty(),
            "没有一枚组写不出行"
        );
        let first = c.export_text().unwrap();
        assert!(
            first.contains("[groups]"),
            "导出的档里就得有这一节：{first}"
        );
        assert!(
            first.contains(r#""甲=1 2""#) && first.contains(r#""乙=""#),
            "组名与成员都要在档里：{first}"
        );

        let (back, report) = inspect_file(&first).unwrap();
        assert!(
            !report.refused(),
            "自己导出的档不该被自己挡下：{:?}",
            report.blocking
        );
        assert!(
            report.missing.is_empty(),
            "导出的档带齐了每一族，缺哪一族就报哪一族：{:?}",
            report.missing
        );
        assert_eq!(back.groups, c.groups, "读回来的分组与导出的同一份");
        assert_eq!(
            back.roster(),
            roster_of(&set),
            "顺序表原样回来（次序也算在内）"
        );

        let second = back.export_text().unwrap();
        assert_eq!(first, second, "导出→导入→再导出必须逐字节相同");
    }

    /// 同一判据在**人手写过**的档上也成立：`save` 走 [`merge_table`] 保注释，而保注释那条路
    /// 从不删键 —— 所以一枚组删掉之后不许在档里留尸体，下一次读档也不许因此复活。
    /// 这一格钉的就是那个复活（它一旦存在，「删除分组」在第二次开程序时就是假的）。
    #[test]
    fn a_saved_file_keeps_comments_and_never_resurrects_a_deleted_group() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut c = Config::default();
        c.general.show_tray = false;
        c.save(&path).unwrap();
        // 用户自己加的注释与空行（本仓的档就是这个样子，不是机器写的那一份）。
        let body = std::fs::read_to_string(&path).unwrap();
        let touched = format!(
            "# 我自己的那句注释\n\n{}",
            body.replace("[general]", "# 托盘那一枚别动\n[general]")
        );
        std::fs::write(&path, &touched).unwrap();

        let (mut loaded, _) = Config::load(&path);
        let mut set = pins_and_two_groups();
        loaded.set_roster(&roster_of(&set));
        loaded.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# 我自己的那句注释"),
            "头顶那句用户的话不能被导出抹掉：{text}"
        );
        assert!(
            text.contains("# 托盘那一枚别动"),
            "节里的注释跟着那一节走，save 不许洗掉它：{text}"
        );
        assert!(text.contains("甲=1 2"), "分组要落得下去：{text}");

        // 现在删掉 `乙`，再存一次：档里**不许**留下它的一行残骸，也不许在下次读档时复活。
        let gone = set.group_ids()[1];
        assert_eq!(set.group(gone).unwrap().name, "乙");
        set.delete_group(gone, crate::pin::GroupFate::Ungroup)
            .unwrap();
        let (mut again, _) = Config::load(&path);
        again.set_roster(&roster_of(&set));
        again.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("乙"), "删掉的组在档里不许留一行：{text}");
        let (reread, _) = Config::load(&path);
        assert_eq!(
            reread
                .roster()
                .iter()
                .map(|g| g.name.clone())
                .collect::<Vec<_>>(),
            vec!["甲".to_string()],
            "第二次读档读回来的就是机器上那一份，不多一枚复活的"
        );
    }

    /// §5.20.3 步骤 4「显示将被覆盖的项目」在分组这一族上的那一列：给一份新档，报得出
    /// 哪一组要新增、哪一组要没、哪一组的成员变了、次序变了。**一枚具体的组名**一行，
    /// 而不是「groups.entries 这一整串字符变了」。
    #[test]
    fn the_preview_names_which_groups_a_file_would_overwrite() {
        let mut set = pins_and_two_groups();
        let mut current = Config::default();
        current.set_roster(&roster_of(&set));

        // 一份档：`甲` 的成员改成 1、`乙` 没了、多出一枚 `丙` 带着 3，次序也换了。
        let mut incoming = Config::default();
        incoming.groups.entries = vec!["丙=3".to_string(), "甲=1".to_string()];

        let changes = current.preview_import(&incoming, ImportScope::All);
        let keys: Vec<&str> = changes.iter().map(|c| c.key.as_str()).collect();
        assert!(keys.contains(&"groups.added.丙"), "{keys:?}");
        assert!(keys.contains(&"groups.removed.乙"), "{keys:?}");
        assert!(
            keys.contains(&"groups.members.甲"),
            "成员从 1、2 变成 1 必须报出来：{keys:?}"
        );
        assert!(keys.contains(&"groups.order"), "次序变了要有一行：{keys:?}");
        let removed = changes
            .iter()
            .find(|c| c.key == "groups.removed.乙")
            .unwrap();
        assert!(removed.to.is_empty(), "档里没有它：{removed:?}");
        assert!(!removed.restart_required, "分组落回机器是当场的事");
        let members = changes
            .iter()
            .find(|c| c.key == "groups.members.甲")
            .unwrap();
        assert_eq!(members.from, "1, 2", "从谁变成谁，两边都要说：{members:?}");
        assert_eq!(members.to, "1");
        // 建议范围（§5.20.3 那六枚）带着这一族，不是只有 All 才带。
        let scoped = current.preview_import(&incoming, ImportScope::Suggested);
        assert!(
            scoped.iter().any(|c| c.key.starts_with("groups.")),
            "「分组配置」是建议范围的第六枚，Suggested 就得带它：{:?}",
            scoped.iter().map(|c| c.key.as_str()).collect::<Vec<_>>()
        );

        // 步骤 5：按报出去的那一列应用，落得下去（不是一句「报了但什么都不做」）。
        let mut applied = current.clone();
        applied.apply_import(&incoming, ImportScope::All).unwrap();
        assert_eq!(applied.groups, incoming.groups);
        let sync = sync_store_groups(&mut set, &applied.roster());
        assert!(sync.touched(), "{:?}", sync.line());
        assert_eq!(
            roster_of(&set),
            vec![
                GroupRoster {
                    name: "丙".into(),
                    members: vec!["3".into()]
                },
                GroupRoster {
                    name: "甲".into(),
                    members: vec!["1".into()]
                },
            ],
            "机器落的就是档里那一份：{}",
            sync.line()
        );
        assert_eq!(set.pins_in_group(set.group_ids()[1]), vec![1], "2 被拿走");
        assert!(
            set.groups().iter().all(|g| g.name != "乙"),
            "删掉的组真没了"
        );
        assert_eq!(
            set.get(2).unwrap().group_id,
            None,
            "谁也不要的那枚退回未分组，而不是随组销毁"
        );
        assert!(sync.refused.is_empty(), "{}", sync.line());
    }

    /// 配对只有一份作者：档里写 `Ref` 而机器上叫 `ref` ⇒ 报的是**改名**，做的也是改名，
    /// 而且那两枚贴图一枚也不掉（新建一支会被 §5.12.1 的重名规矩拒掉，那就是「报了而做不到」）。
    #[test]
    fn a_case_twin_is_a_rename_in_both_the_report_and_the_machine() {
        let mut set = PinSet::default();
        let frame = crate::frame::Frame::filled(8, 8, [0, 0, 0, 255]).unwrap();
        for id in 1..=2 {
            set.add(
                crate::pin::PinItem::from_frame(
                    id,
                    &frame,
                    crate::geometry::PhysPoint::new(0, 0),
                    &Pin::default(),
                )
                .unwrap(),
            );
        }
        let g = set.create_group("ref").unwrap();
        set.assign_group(&[1], Some(g)).unwrap();

        let current = roster_of(&set);
        let target = vec![GroupRoster {
            name: "Ref".into(),
            members: vec!["1".into()],
        }];
        assert_eq!(
            plan_group_sync(&current, &target),
            vec![GroupStep::Rename {
                from: "ref".into(),
                to: "Ref".into()
            }],
            "没有 Create（会被拒），也没有 Remove（会掉贴图）"
        );
        let sync = sync_store_groups(&mut set, &target);
        assert_eq!(sync.renamed, 1, "{}", sync.line());
        assert_eq!(sync.created, 0);
        assert_eq!(sync.moved_in, 0, "成员一枚也没变");
        assert_eq!(set.group(g).unwrap().name, "Ref");
        assert_eq!(set.pins_in_group(g), vec![1], "改名不改成员");
        assert!(sync.refused.is_empty(), "{}", sync.line());
    }

    /// 「这一步真的动了机器吗」的另一半：档里点名的贴图编号**这台机器没有**时，一句要说得出，
    /// 而落下去的只有认得的那些（§8.3 不得静默；`PinId` 不跨机器是 [`Groups`] 头顶那句话）。
    #[test]
    fn members_the_machine_does_not_have_are_named_not_swallowed() {
        let mut set = pins_and_two_groups();
        let target = vec![
            GroupRoster {
                name: "甲".into(),
                members: vec!["2".into(), "99".into()],
            },
            GroupRoster {
                name: "新组".into(),
                members: vec!["7".into(), "8".into()],
            },
        ];
        let sync = sync_store_groups(&mut set, &target);
        // 判的是「三枚点名的编号一枚也不许被咽下去」，不是它们被哪一组先认领：`unknown_members`
        // 是按步序交的账，而「甲」与「新组」谁先落 Take 归 [`plan_group_sync`] 那一步的顺序说了算。
        let mut lost = sync.unknown_members.clone();
        lost.sort();
        assert_eq!(lost, vec!["7", "8", "99"], "{:?}", sync.unknown_members);
        assert_eq!(
            set.pins_in_group(set.group_ids()[0]),
            vec![2],
            "认得的 2 落下去"
        );
        assert_eq!(
            set.group_ids().len(),
            2,
            "一枚新建（乙被删）、一枚还在：{}",
            sync.line()
        );
        assert!(sync.created == 1 && sync.removed == 1, "{}", sync.line());
        assert!(
            sync.line().contains("这台机器没有"),
            "那句要说出来的话得在：{}",
            sync.line()
        );
        assert!(
            roster_of(&set)
                .iter()
                .all(|g| g.members != vec!["99".to_string()]),
            "没有编号 99 的贴图，就不许有一份写着它的机器"
        );
    }

    /// §5.20.3 步骤 4 的「校验文件」：坏文件、缺族、版本号比本机新，三样各有一句，绝不静默。
    #[test]
    fn a_file_is_checked_before_anything_is_applied() {
        // ① 坏文件：连读也读不成 ⇒ Err，调用方原样显示这一句。
        assert!(matches!(
            inspect_file("[general\nlanguage = "),
            Err(ConfigError::Parse(_))
        ));
        // 认得出 TOML 而认不出这一族（枚举值写错）也算读不成，不是"那就按默认导进去"。
        assert!(inspect_file("appearance = { theme = \"nope\" }").is_err());

        // ② 版本号比本机新 ⇒ 挡住，一句说清是哪一枚数。
        let future = format!("version = {}\n[general]\n", CONFIG_VERSION + 1);
        let (cfg, fresh_rep) = inspect_file(&future).unwrap();
        assert_eq!(cfg, Config::default(), "读得成，只是不许应用");
        assert!(fresh_rep.refused(), "比本机新的档必须挡下来");
        assert!(
            fresh_rep.blocking[0].contains("先升级"),
            "{:?}",
            fresh_rep.blocking
        );
        // 没有版本行 = 按本机这一版理解（我们自己导出的档就是这样），不挡。
        let (_, ours_rep) = inspect_file(&Config::default().export_text().unwrap()).unwrap();
        assert!(!ours_rep.refused());
        assert!(
            ours_rep.missing.is_empty(),
            "自己导出的档带齐了每一族：{:?}",
            ours_rep.missing
        );
        // 版本行写成了字，不是数 ⇒ 提醒一句，按「没有版本行」处理，不假报。
        let (_, word_rep) = inspect_file("version = \"one\"\n").unwrap();
        assert!(!word_rep.refused());
        assert!(
            word_rep.warnings.iter().any(|w| w.contains("不是版本号")),
            "{:?}",
            word_rep.warnings
        );

        // ③ 缺族：一份只写热键的档。`[groups]` 没写 ⇒ 档里没这一族 ⇒ **不动**本机的分组。
        let (part, part_rep) = inspect_file("[hotkey]\nactions = { capture = \"f13\" }\n").unwrap();
        assert!(
            part_rep.missing.contains(&"groups".to_string()),
            "{:?}",
            part_rep.missing
        );
        assert!(part_rep.missing.contains(&"history".to_string()));
        assert!(!part_rep.missing.contains(&"hotkey".to_string()));
        assert!(
            part_rep
                .says()
                .iter()
                .any(|s| s.contains("档里没写这些族") && s.contains("groups")),
            "{:?}",
            part_rep.says()
        );
        // 本机认不得的那一节也要说一句，不许悄悄丢了它。
        let (_, stranger_rep) = inspect_file("[general]\n[made_up]\nfoo = 1\n").unwrap();
        assert!(
            stranger_rep.warnings.iter().any(|w| w.contains("made_up")),
            "{:?}",
            stranger_rep.warnings
        );

        // ④ 报的与做的同一份名单：没写的族从那一列里挑下来，而挑下来的**显示**得出来。
        let mut current = Config::default();
        let set = pins_and_two_groups();
        current.set_roster(&roster_of(&set));
        let all = current.preview_import(&part, ImportScope::All);
        let (kept, dropped) = prune_missing(&all, &part_rep.missing);
        assert!(
            all.iter().any(|c| c.key.starts_with("groups.")),
            "不对比一下就说不过去：一份没写 [groups] 而机器上有组的档，逐键比不出事，\
             而 diff_groups 比得出「每一枚组都要没」—— 所以这一行必须由 prune 挑掉"
        );
        assert!(
            kept.iter().any(|c| c.key == "hotkey.actions.capture"),
            "{kept:?}"
        );
        assert!(
            kept.iter().all(|c| !c.key.starts_with("groups.")),
            "{kept:?}"
        );
        assert!(dropped.iter().any(|c| c.key.starts_with("groups.removed")));
        let mut applied = current.clone();
        applied.apply_changes(&part, &kept).unwrap();
        assert_eq!(
            applied.groups, current.groups,
            "档里没写这一族 ⇒ 本机的分组原样留着"
        );
        assert_eq!(
            applied.hotkey.actions["capture"], "f13",
            "写了的那一族才落得下去"
        );
    }

    /// 档那道闸：读得出的写法归一成一种写法，读不出的那一行丢下且说得出为什么（§8.3），
    /// 而归一之后再过一次导出 ⇒ 往返判据的字节相同不是运气。
    #[test]
    fn the_gate_normalizes_a_hand_written_group_list() {
        let mut c = Config::default();
        c.groups.entries = vec![
            "  甲  = 1  2 1 ".to_string(), // 首尾空白、重复编号
            "乙".to_string(),              // 手写的光一枚名字 = 空组
            "甲=3".to_string(),            // 大小写重名（同一枚「甲」）⇒ 第二枚丢下
            "有,逗号=1".to_string(),       // 分隔符
            "=1 2".to_string(),            // 没有名字
            "丙=red".to_string(),          // 成员不是编号
        ];
        let warnings = c.validate();
        assert!(
            warnings
                .iter()
                .any(|w| w == "groups: \"  甲  = 1  2 1 \" 存的是 \"甲=1 2\""),
            "{warnings:?}"
        );
        assert!(warnings.iter().any(|w| w.contains("乙")), "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("读不出编号的成员") && w.contains("red")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("丢下 3 枚组")),
            "逗号那枚、空名那枚、重名那枚，三枚一起说：{warnings:?}"
        );
        assert_eq!(
            c.groups.entries,
            vec!["甲=1 2".to_string(), "乙=".to_string(), "丙=".to_string()],
            "{warnings:?}"
        );
        // 归一之后再走一趟闸：一个字也不许再变（幂等 = 往返判据的另一半）。
        let again = c.validate();
        assert!(again.iter().all(|w| !w.contains("groups")), "{again:?}");
        let text = c.export_text().unwrap();
        let (back, _) = inspect_file(&text).unwrap();
        assert_eq!(back.groups, c.groups);
        assert_eq!(c.export_text().unwrap(), text);
    }

    /// §5.20.2 那两句的**可判**版本：名单与对照表逐枚对得上（`restart_audit()` 空），
    /// 而「一整族」那种标记（`hotkey.actions.*`）在 `needs_restart` 上问得出、在名单外的一枚
    /// 也好端端地不说要重启。
    #[test]
    fn the_restart_marks_match_the_machine_and_nothing_else() {
        assert!(restart_audit().is_empty(), "{:?}", restart_audit());
        // 补标的两枚（本轮查出来的漏标）。
        assert!(
            needs_restart("general.show_tray"),
            "读者只有 install 那一次"
        );
        assert!(needs_restart("hotkey.actions.capture"), "同上，一整族");
        assert!(needs_restart("hotkey.actions.toggle_pins"));
        // 名单外的一枚不许被前缀顺手套走。
        assert!(!needs_restart("annotation.palette"));
        assert!(!needs_restart("general.language2"));
        // 按档数出来的那一列：五枚逐名的 + `hotkey.actions` 那一族出厂那九枚，一枚不多一枚不少。
        // 数错的那一面就是设置页那一格的红字（页上标 14 而名单只报 5 —— 它一枚也没标错）。
        let marks = restart_marks(&Config::default());
        assert_eq!(
            marks.len(),
            RESTART_KEYS.len() + Config::default().hotkey.actions.len(),
            "{marks:?}"
        );
        for key in RESTART_KEYS {
            assert!(marks.contains(&key.to_string()), "名单上 {key} 不在档里");
        }
        assert!(marks.iter().any(|k| k == "hotkey.actions.capture"));
        assert!(!marks.iter().any(|k| k.starts_with("annotation.")));
        assert!(
            restart_marks(&Config::default())
                .windows(2)
                .all(|w| w[0] <= w[1]),
            "按档数出来的那一列没有序"
        );
        // 表上「压根没有读者」的那些：§5.20.2 两句对它都不成立，报出来而不是算成名单的错。
        //
        // 这一格的前提**自本轮起变了**：`advanced.cli_enabled` 那一行原来写的是 `reader: None`
        // （「命令行没人认参数」），所以它同 `general.language` 一起在这一列里。现在它有了读者
        // （`qt_bridge::cli_gate`），于是它从这一列出去 —— 留在名单里（`timing: Startup`）不变，
        // 因为 argv 的解释确实只在一趟进程起来那一刻发生一次。**没有删掉这一格**，改的是它判的
        // 那一枚键：`language` 仍然必须报「没人读」，`cli_enabled` 现在必须**不**报。
        let unread = restart_unread();
        assert!(
            unread.contains(&"general.language"),
            "i18n 那一枚键没有读者，§5.20.2 两句对它都不成立：{unread:?}"
        );
        assert!(
            !unread.contains(&"advanced.cli_enabled"),
            "命令行开关自本轮起有读者（qt_bridge::cli_gate），不许再停在「压根没人读」那一列：{unread:?}"
        );
    }
}
