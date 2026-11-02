//! Configuration (PRD §5.20): the eleven settings categories, an atomic save
//! that keeps the user's comments, and recovery from a damaged file (§5.1.1).
//!
//! Two representations coexist on purpose:
//! * the typed structs below, which every product module reads;
//! * a dotted-path view (`get_str`/`set_str`) over the same data, which is what
//!   the QML settings page binds to — one code path for a hundred widgets
//!   instead of a property per setting.

use crate::annotation::{Kind, FONT_MAX, FONT_MIN};
use crate::encode::Format;
use crate::geometry::Unit;
use crate::naming::Collision;
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

/// Keys that need the process to restart before they take effect (§5.20.2:
/// "需要重启的设置应明确标记").
pub const RESTART_KEYS: &[&str] = &[
    "general.language",
    "general.launch_at_startup",
    "advanced.data_dir",
    "advanced.cli_enabled",
];

/// §5.20.2's 「需要重启的设置应明确标记」 answered for one key. The list above is the
/// whole of the judgement, and this is the only function that reads it: the import
/// preview and the settings page have to mark the same key the same way, and a second
/// copy of the list could only ever disagree with the first.
pub fn needs_restart(key: &str) -> bool {
    RESTART_KEYS.contains(&key)
}

/// §5.16.1 bindable action ids. Hot corners and per-app exclusions name these.
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
/// **前十二枚与 [`ACTIONS`] 同序同 id**，这一条不是靠这段话保证的：`tests` 里那一格逐枚比对
/// 那十二个 id，漏一枚、重一枚、换了序都红。
///
/// 末尾三枚（`menu` / `settings` / `none`）不在 §5.16.1 那十二名里，它们是 §5.18.2 那四枚
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
/// 做不动的那三枚（`switch_group` / `pin_edit` / `custom_task`）只说这一枚动作*本来*是什么，
/// 接没接线由设置页现问归宿表、现拼
/// 那两个字——把「未接线」写死进这一格，接上的那天它就成了假话，而那正是本模块头部记过的
/// 同一枚坏（名单不会错，它只会先旧）。
pub const ACTION_TEXTS: [(&str, &str, &str); 15] = [
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
pub const TRAY_ACTIONS: &[&str] = &[
    "menu",
    "settings",
    "capture",
    "paste_pin",
    "toggle_pins",
    "show_all_pins",
    "hide_all_pins",
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
    /// §5.1.2 开机自启.
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
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            default_mode: CaptureMode::Region,
            include_cursor: false,
            freeze_frame: true,
            highlight_element: true,
            magnifier_radius: 8,
            magnifier_zoom: 8,
            magnifier_grid: true,
            magnifier_coordinates: true,
            magnifier_color_format: "hex_upper".into(),
            show_dimensions: true,
            size_unit: Unit::default(),
            delay_seconds: 0,
            snap_to_window: true,
        }
    }
}

/// One tool's remembered pen (§5.7.1). Both fields are optional because a hand-edit
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkey {
    /// Action id → key spec (`ctrl+shift+x`).
    pub actions: BTreeMap<String, String>,
    pub exclusions: Vec<AppExclusion>,
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
        Hotkey {
            actions,
            exclusions: Vec::new(),
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
            accent: "#0078D7".into(),
            toolbar_placement: "auto".into(),
            ui_font_size: 9,
            magnifier_border: 1,
            magnifier_corner_radius: 6,
            magnifier_foreground: "#FFFFFF".into(),
            magnifier_background: "#1F1F1F".into(),
        }
    }
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
    /// §5.21.1 第 3 步「系统获取版本信息」的那份东西从哪儿来。本 build 只读本机路径；
    /// 写 URL 会被 `[about cfg]` 那行数出来，并由检查更新自己交回一句拒绝（见
    /// [`crate::update::Answer::RemoteRefused`]）。
    pub release_feed: String,
    /// §5.21.3 attach diagnostics to feedback.
    pub send_diagnostics: bool,
}

impl Default for About {
    fn default() -> Self {
        Self {
            auto_check_updates: true,
            check_every_days: 7,
            channel: "stable".into(),
            release_feed: String::new(),
            send_diagnostics: false,
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
    pub fn preview_import(&self, incoming: &Config, scope: ImportScope) -> Vec<ImportChange> {
        let before = self.flat().unwrap_or_default();
        let after = incoming.flat().unwrap_or_default();
        let mut out = Vec::new();
        for (key, to) in &after {
            if !scope.covers(key) {
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
        out
    }

    /// §5.20.3 step 5.
    ///
    /// The corrections are collected across the whole apply, in order, without
    /// repeats: one write can make the gate say something about a pair of keys
    /// (`hotkey.actions.capture, toggle_pins: f13 is bound twice`), and every
    /// later write would otherwise repeat that line.
    pub fn apply_import(
        &mut self,
        incoming: &Config,
        scope: ImportScope,
    ) -> Result<Vec<String>, ConfigError> {
        let mut corrections: Vec<String> = Vec::new();
        for change in self.preview_import(incoming, scope) {
            for warning in self.set_str(&change.key, &change.to)? {
                if !corrections.contains(&warning) {
                    corrections.push(warning);
                }
            }
        }
        Ok(corrections)
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
        clamp(
            &mut w,
            "appearance.ui_font_size",
            &mut self.appearance.ui_font_size,
            6,
            48,
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
            ("appearance.accent", &mut self.appearance.accent, "#0078D7"),
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
            if Kind::from_key(&key).is_none() {
                w.push(format!(
                    "annotation.tool_style.{key}: no such tool, dropped"
                ));
                continue;
            }
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
        if !matches!(
            self.appearance.toolbar_placement.as_str(),
            "auto" | "top" | "bottom" | "left" | "right"
        ) {
            w.push(format!(
                "appearance.toolbar_placement: unknown {:?}, using auto",
                self.appearance.toolbar_placement
            ));
            self.appearance.toolbar_placement = "auto".into();
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
        w
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

/// §5.20.3 suggests a limited import range; `All` is the opt-in escape hatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportScope {
    /// Hotkeys, annotation styles, palette, theme, output directory and naming.
    ///
    /// §5.20.3's list also names 分组配置 (group configuration), and this range
    /// does **not** carry it: groups live in the pin store
    /// (`falcon_core::pin::PinStore::groups`), not in `config.toml`, so there is
    /// no key here for them to travel through. Export/import moves the settings
    /// file; a group travels only if the whole store travels.
    Suggested,
    All,
}

const SUGGESTED_PREFIXES: &[&str] = &[
    "hotkey.",
    "annotation.",
    "appearance.",
    "pin.",
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
            },
        );
        c.annotation.tool_style.insert(
            "rect".into(),
            ToolStyle {
                color: Some("#0078D7FF".into()),
                width: Some(5),
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
        assert!(!hotkey.restart_required);
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
}
