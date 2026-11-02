//! §5.20.1 设置分类 与 §5.20.2 设置即时生效 —— 设置页那一侧的模型。
//!
//! 盘上早就有两份各管一段的码：[`crate::settings`] 拥有这一份文件（谁读、谁写、写不动
//! 了记在哪），[`falcon_core::config`] 拥有每一枚键的语法（`flat` 数得出有哪些键、
//! `set_str` 决定一个字串能不能成为这一枚键的值）。**这一档两头都不占**，它回答的是文档
//! §5.20 那两节问的两件事：
//!
//! * 「设置页包含哪几类」—— 那份名单（§5.20.1 的十一类一枚不少、顺序不乱，P178 起再加一类
//!   「托盘」），和每一类在该页上列出哪些行；
//! * 「改完多久生效」—— 一枚键是即时生效还是要重启，这句话由谁说了算。
//!
//! 第二件此前只有一位读者：`preview_import` 里那句「这一项要不要重启」直接查
//! [`falcon_core::config::RESTART_KEYS`]，而设置页要标同一枚键只能再抄一份名单。抄的那一份
//! 不会错，它只是会**先旧**：加一枚 `RESTART_KEYS` 的时候改码的人看着的是导入预览那一行，
//! 不是页上那一格。所以 [`falcon_core::config::needs_restart`] 是名单唯一的门面，本模块与
//! 导入预览共用它（#281）。
//!
//! 写的那一条腿一格也没新判据：`write` 调的是 `Config::set_str`（语法与校正都在门里），
//! 落盘调的是 [`crate::settings::Settings::save`]（原子写与失败记账也都在门里）。本模块加
//! 的只有**顺序**：先过闸、过了才写、写完才把「被改成了什么」原样交回调用方 —— §5.20.2 要
//! 的就是页上说的和内存里存的是同一个数，而不是各说一个。
//!
//! 界面那一侧（`SettingsWindow.qml` 与托盘里的「首选项」入口）自 P142 起也接在这一档上，
//! 但接的是**同一批格子**：页上那排标签读 [`category_line`]、每一页的行读 [`page_line`]、
//! 点某一行改哪一枚键读 [`key_at`]、改值走 [`write`] —— 窗一处也不自己判「这枚键归哪一类」、
//! 一处也不自己拼 `（需重启）` 那几个字。P141 那一轮把模型与窗分开，是为了让模型这一半先有
//! 一把不靠像素的尺子；这一轮加的是尺子量不到的那一部分：一扇真窗递回来的那些行。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use falcon_core::config::{action_texts, needs_restart, Config, ImportScope, RESTART_KEYS};

/// 页顶那排标签的**唯一**来源。前十一枚是 §5.20.1 那一列数字（1 常规 … 11 关于与更新），
/// 逐字逐序照抄；第十二枚「托盘」是 §5.18.2 第一步（PRD 第 1,448 行）要的那一格，放在
/// 「快捷键」与「热区」之间，其余十一枚的相对次序一枚没动。
///
/// 每类后面跟的是 `Config` 的**段名**而不是点分前缀：一枚键归哪一类，问的是它头一段叫什么
/// —— 拿前缀去 `starts_with` 会漏掉嵌套（`pin.alpha_bg` 与假想的 `pinish.x` 在这一族里是两
/// 件不同的事），而段名是 `Config` 结构体的字段，编译器盯着。
///
/// `card` 归进「贴图」而不是自己成一类：§5.20.1 那一列里没有「卡片」，而 `Card` 那一段全是贴图
/// 卡片怎么画（圆角、阴影、边框、放大镜之外的那几枚），把它单列出来是往一份**建议**里塞一页文档
/// 没点过的名字 —— 能归进去的就不另开。
///
/// `tray` **不**归进「快捷键」了，这一处自 P178 起改了读法，改的原因写在下面，不是随手：
/// §5.18.2 第一步（PRD 第 1,448 行）写的是「用户打开“首选项 > 托盘”。」，而 P142 起这一档把
/// 四枚 `tray.*` 挂到了「快捷键」那一页上，理由是「§5.20.1 那十一类里没有托盘，两处不能同时
/// 字面满足」。那次取舍读错了轻重：§5.20.1 的原话是「**建议**设置页包含：1. 常规 … 11. 关于与
/// 更新。」（PRD 第 1,505–1,517 行）—— 它给的是一份**下限名单**，十一类一枚不许少、次序不许乱，
/// 但没有一处说「只有这十一类」；§5.18.2 那一行则是规范性步骤的第一步，真人今天照着它走第一步就
/// 走不下去（页上没有「托盘」这一格可点）。所以下限名单照旧逐字逐序全在，另加第十二类
/// 「托盘」，两处第一次同时字面满足。代价是类号往后挪了一格，故 [`CORNER`] 是常量而不是 `6`：
/// 「首选项 > 热区」（PRD 第 1,328 行）指的还是同一页。
///
/// `whiteboard` 归进「截图」是上面 `card` 那一条的同一个理由，且它比 `card` 更没有悬念：§5.4.4
/// 第一步写的就是"从截图工具栏进入白板模式"，那一块板子是一张**像素由做而非抓出来的截图**，两枚
/// 键调的正是它的底色与大小 —— 文档没有为它点过名，就不开。
pub const CATEGORIES: [(&str, &[&str]); 12] = [
    ("常规", &["general"]),
    ("截图", &["capture", "whiteboard"]),
    ("标注", &["annotation"]),
    ("贴图", &["pin", "card"]),
    ("输出", &["output"]),
    ("快捷键", &["hotkey"]),
    ("托盘", &["tray"]),
    ("热区", &["hot_corner"]),
    ("外观", &["appearance"]),
    ("历史记录", &["history"]),
    ("高级与命令行", &["advanced"]),
    ("关于与更新", &["about"]),
];

/// 「关于与更新」那一页在 [`CATEGORIES`] 里的**名字**，唯一作者就是名单第 12 行自己：写成一枚
/// 具名常量，是为了让页脚那一块（[`update_words`]）与单测问的是同一个问题——「哪一页递更新那两枚
/// 钮」——而不是各自数一遍「第 11 类」。[`CORNER`] / [`TRAY_PAGE`] 记的是「第几类叫什么」，这一枚
/// 记的是反过来的那一问；两处若有一天分家，红的是名单，不是窗上的一个下标。
pub const ABOUT_PAGE: &str = CATEGORIES[11].0;

/// §5.15 那一页在名单里的类号。写成一枚常量而不是 `6`：页上的行、门里的那格、单测里的断言
/// 问的是同一件事——「热区是第几类」。P178 之后它是 7 而不是 6，这一枚常量就是那次改动只需要
/// 动一处的证据。
pub const CORNER: usize = 7;

/// §5.18.2 那一页在名单里的类号，与 [`CORNER`] 同一个理由：页上的行、门里的那格、单测里的断言
/// 问的是同一件事——「托盘是第几类」。它存在的对面是 PRD 第 1,448 行那一句「用户打开“首选项 >
/// 托盘”。」，而那一句在 P178 之前是走不下去的第一步。
pub const TRAY_PAGE: usize = 6;

/// 「标注」那一页在名单里的类号，与 [`CORNER`] / [`TRAY_PAGE`] 同一个理由：§5.7.22 那一句「用户
/// 打开调色板管理」在页上的落点就是这一类（那一排色与 `[annotation]` 那两枚名单键都归它），而
/// 页上的行、门里的那格、单测里的断言问的是同一件事——「标注是第几类」。
pub const ANNOT_PAGE: &str = CATEGORIES[2].0;

/// 这一扇窗在机器那一侧的名字 —— 顶层窗普查用来认它的那一枚。
///
/// 对面是 `SettingsWindow.qml` 里 `title: "falconshot-settings"` 那一行，与
/// `hint_check::TITLE` / `mask_check::TITLE` 同形：Rust 侧要按标题找一扇窗，就得有一个名字，
/// 而这个名字**不带空格**（`pinTopLevels` 是按空白切字段的一行串，带空格的名字切不开）。
/// 它与 [`crate::r13::parse_levels`] 一起构成「桌面上真的多了一枚设置窗」这一格的读数，
/// 而不是「桥里那一格 `settings_open` 写了 1」—— 后者只说 Rust 点了头。
pub const TITLE: &str = "falconshot-settings";

/// 页上的一行：一枚键、它此刻的值、和它要不要重启。
pub struct Row {
    pub key: String,
    pub value: String,
    pub restart: bool,
}

impl Row {
    /// 页上那一格的样子，也是这一档每行印出来的样子。`（需重启）` 就是 §5.20.2 第二条要的
    /// 「明确标记」——它挂在行尾而不是单起一行，这样读的人拿行数还能对上「这一类几枚」。
    ///
    /// §5.21.2 第二枚「快捷键设置页应提供对应功能说明」走的是**同一格的位置**、同一枚道理：
    /// 一行动作多出的那一句「名：说明」由 [`NOTE_SEP`] 与 `key=value` 隔开、缀在本行尾部，
    /// 不另起一行，所以「这一类几枚」这一枚读数从头到尾没有因为加说明而变过（[`page_line`]
    /// 切出来的枚数与 [`rows`] 的行数仍然一枚不多一枚不少，
    /// `the_page_hands_the_window_one_piece_per_row` 那一格盯着）。
    pub fn text(&self) -> String {
        let note = match action_of(&self.key, &self.value) {
            Some((name, desc, wired)) => format!(
                "{NOTE_SEP}{name}：{desc}{}",
                if wired { "" } else { UNWIRED_MARK }
            ),
            None => String::new(),
        };
        let entry = match doc_entry_of(&self.key) {
            Some(s) => format!("{NOTE_SEP}{s}"),
            None => String::new(),
        };
        let restart = if self.restart { "（需重启）" } else { "" };
        format!(
            "{}={}{}{note}{entry}{restart}",
            self.key, self.value, restart
        )
    }
}

/// 页上一行的**说明**从本行分开的那一枚串。与 [`UNWIRED_MARK`] 同处一处家：`--selftest` 那一格
/// 问「这一行有没有硬拼一句」要认的就是它，在这里再写一遍空格加破折号就是第二份实现。
pub const NOTE_SEP: &str = " —— ";

/// 页上那一格后缀：名单里点得出名、本 build 却还没有一只 [`crate::hotkey::Work`] 接得住，
/// 就缀这两个字。它与 `hotkey::plan` 报告里那句「未接线（不注册，键留给系统）」说的不是同一个
/// 对象 —— 那一句讲的是一枚**已经配好的键**注册不注册，这两个字讲的是一枚**动作名**做得到
/// 做不到。
/// 住在这里是因为只有页要拼它；`--selftest` 那一格问「这一行标了没有」读的也是这一枚，
/// 于是这个措辞在整个 crate 里只有一个出处（#281）。
pub const UNWIRED_MARK: &str = "（未接线）";

/// §5.21.2 那一枚说明的家：这一行如果点名了一枚动作，交回 `(中文名, 功能说明, 本 build
/// 接不接得住)`；没有点名动作的行交回 `None` = 页上这一行不递说明。
///
/// `pub(crate)` 是因为 `--selftest` 那一格要问同一件事：页上「该不该有这一句」由这里答一次，
/// 量具去**反推页上那一串的长相**就是同一份判断的第二份实现。
///
/// 同一页上两种形状，动作名的位置不一样，而**分头的地方只有这一处**：
/// * `hotkey.actions.<id>` —— 名在**键的尾巴**上，值那一头是一枚组合键；
/// * `tray.<哪一枚键>` —— 名在**值里**，键那一头是左键/双击/中键/右键。
///
/// 「接不接得住」现问归宿表，这里不抄第四份「哪几枚没接线」的名单：热键那一头问
/// [`crate::hotkey::Work::of`]，托盘那一头问 [`crate::hotkey::tray_run_of`]。那两张表是装配
/// 与门共用的那两张，抄出来的一份不会错、只会先旧（本模块头部为 `RESTART_KEYS` 记过同一枚坏）。
/// 而托盘那一头**不许**换成 `Work::of`：`menu` 与 `none` 压根没有 `Work`，把它们塞进那条通路
/// 会给一按就开的托盘右键读出一张假红 —— 假的不是产品，是查表的方式（`tray_run_of` 自己写着）。
pub(crate) fn action_of(key: &str, value: &str) -> Option<(&'static str, &'static str, bool)> {
    if let Some(id) = key.strip_prefix("hotkey.actions.") {
        let (name, desc) = action_texts(id)?;
        return Some((name, desc, crate::hotkey::Work::of(id).is_some()));
    }
    if key.starts_with("tray.") {
        let (name, desc) = action_texts(value)?;
        return Some((
            name,
            desc,
            !matches!(
                crate::hotkey::tray_run_of(value),
                crate::hotkey::TrayRun::Unwired(_)
            ),
        ));
    }
    None
}

/// §5.21.2 第三枚「命令行配置处应提供参数文档入口」的那一处挂钩：这一枚键如果是**命令行**的配置，
/// 页上就递一句入口；其余键交回 `None` = 这一行不递。
///
/// 句子本身不住在这里 —— 它住在 [`crate::cli::doc_entry`]，因为入口指的是那份文档，而文档的主人
/// 只有那一个（在这里再抄一遍 `--help` 就成了第二份拼写，本轮要修的正是这个）。这里只回答
/// **「哪一行该递」**，与 [`action_of`] 只回答「哪一行该有说明」是同一枚形状。
///
/// 认的是键名而不是类号：`advanced.log_level` 与 `advanced.data_dir` 与命令行同页（类号 9 的
/// `CATEGORIES` 那一行），但它们不是命令行的配置，给它们各缀一句「参数文档」就成了六行一样的
/// 噪音。「命令行配置处」里「处」字指的是那一枚配置，不是整页 —— 这一条读法已记进台账。
pub(crate) fn doc_entry_of(key: &str) -> Option<String> {
    if key == "advanced.cli_enabled" {
        return Some(crate::cli::doc_entry());
    }
    None
}

/// 一枚键归哪几类：**全部**命中而不是第一枚命中。
///
/// 空与多是两件事。空 = 设置页上没有这一行，用户永远看不见它（§5.20.1 的分类就成了假名单）；
/// 多 = 同一枚键在两类里各有一格，改哪一格算哪格没人说。合成一句「归属不对」就答不出这一趟
/// 是漏了还是重了，而这两种坏的修法不一样。
pub fn owners(key: &str) -> Vec<usize> {
    let head = key.split('.').next().unwrap_or(key);
    CATEGORIES
        .iter()
        .enumerate()
        .filter(|(_, (_, sections))| sections.contains(&head))
        .map(|(i, _)| i)
        .collect()
}

/// 类名 → 类号。页上的标签是中文那一列，所以这一枚是给「用户点了哪一类」用的反查。
pub fn index_of(name: &str) -> Option<usize> {
    CATEGORIES.iter().position(|(n, _)| *n == name)
}

/// §5.21.1 第 2 步与第 4 步在页上的那一块：第 `which` 类该递的两枚钮名，用 [`RECORD`] 连着交回；
/// **不是**「关于与更新」那一页时空串 = 窗把整块不画（与 [`crate::guide::line_for`] 交回空串
/// 让引导那一块整个消失，是同一形状）。
///
/// 这里只答「哪一页递」，两枚字一个也不在这里起：检查那一枚是 [`falcon_core::update::TRAY_WORD`]
/// （§5.18.1 第 1,441 行菜单项的同一名单源），下载那一枚是 [`falcon_core::update::DOWNLOAD_WORD`]。
/// 与 [`action_of`] / [`doc_entry_of`] 只答「哪一行该有说明 / 该递入口」是同一枚分工（#281）：
/// 窗里一旦自己写「关于与更新」或者「检查更新」，改一天名单第 12 行就成了页上写着旧字。
pub fn update_words(which: usize) -> String {
    if index_of(ABOUT_PAGE) != Some(which) {
        return String::new();
    }
    format!(
        "{}{RECORD}{}",
        falcon_core::update::TRAY_WORD,
        falcon_core::update::DOWNLOAD_WORD
    )
}

/// 某一类的行，读的是**本进程手里那一份** config —— 页上显示的值必须是这一台机器正在跑的
/// 那个值，而不是出厂默认，否则「即时生效」这句话在这一档里就没对象了。
pub fn rows(which: usize) -> Vec<Row> {
    crate::settings::with(|s| rows_of(&s.config, which))
}

/// 同一件事对任意一份 config 做：headless 的判据得能在没有会话、没有屏幕的地方问。
pub fn rows_of(cfg: &Config, which: usize) -> Vec<Row> {
    let keys = match cfg.flat() {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    keys.iter()
        .filter(|(k, _)| owners(k).contains(&which))
        .map(|(k, v)| Row {
            key: k.clone(),
            value: v.clone(),
            restart: needs_restart(k),
        })
        .collect()
}

/// 一次写的全部读数。
///
/// `stored` 是写完之后**门里那一枚键**的值，不是调用方打进来的串：这两个不一样正是 §5.20.2
/// 第一条那一格的对面（用户打 9000、门留下 5000，页上得说 5000）。`None` 是这一枚键压根不
/// 存在，与「存在但值为空串」是两件事，所以它是 `Option` 而不是 `String`。
pub struct Written {
    pub key: String,
    pub asked: String,
    pub stored: Option<String>,
    /// 门替这一枚键改了什么，原样带回来（[`Config::set_str`] 的那些句子）。
    pub corrections: Vec<String>,
    pub restart: bool,
    pub save_ms: u64,
    /// 语法闸那句拒。拒了的写**没动** config，这一族有单测盯着。
    pub refused: Option<String>,
    /// 落盘那句拒。值留在内存里（§9.4 对失败保存定的规矩），但这一趟不许报成功（§5.17）。
    pub save_failed: Option<String>,
}

impl Written {
    pub fn ok(&self) -> bool {
        self.refused.is_none() && self.save_failed.is_none()
    }

    /// 一行说完一次写：写没写成、成了什么、要不要重启、盘花了多少毫秒。
    pub fn text(&self) -> String {
        let value = self
            .stored
            .as_deref()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(这一枚键不存在)".to_string());
        let mark = if self.restart { "需重启" } else { "即时" };
        if let Some(why) = &self.refused {
            return format!(
                "[settings] {} 打成 {:?} 未写入（{why}）：内存里仍是 {value}；{mark}；未落盘",
                self.key, self.asked
            );
        }
        if let Some(why) = &self.save_failed {
            return format!(
                "[settings] {} = {} -> {value}（{mark}）已改内存、**写盘失败**：{why}",
                self.key, self.asked
            );
        }
        let fixed = if self.corrections.is_empty() {
            String::new()
        } else {
            format!(" 被改成 {}", self.corrections.join(" / "))
        };
        format!(
            "[settings] {} = {} -> {value}（{mark}）落盘 {} ms{fixed}",
            self.key, self.asked, self.save_ms
        )
    }
}

/// 只动传进来的那一份 config 的那一腿：过闸、回读、不碰盘。
///
/// 拆出来是因为判据要能在没有文件的地方问同一句话：[`check`] 与单测用它，页与门用
/// [`write`]。两条腿共用这一枚，所以「门怎么说」这件事不会分成两份。
pub fn write_into(cfg: &mut Config, key: &str, asked: &str) -> Written {
    let mut out = Written {
        key: key.to_string(),
        asked: asked.to_string(),
        stored: None,
        corrections: Vec::new(),
        restart: needs_restart(key),
        save_ms: 0,
        refused: None,
        save_failed: None,
    };
    match cfg.set_str(key, asked) {
        Ok(corrections) => out.corrections = corrections,
        Err(e) => out.refused = Some(e.to_string()),
    }
    out.stored = cfg.get_str(key);
    out
}

/// 写一行并落盘：页上改一档、文件就得跟着变，这是 §5.20.2 第三条那一支（「明确的保存」）。
///
/// 拒了的写**不**落盘：语法闸没过时内存里那一份也没动，此时写盘等于把别处的改动顺手带下
/// 去，而这一趟用户只看见一次失败的点击。
pub fn write(key: &str, asked: &str) -> Written {
    crate::settings::with(|s| {
        let mut out = write_into(&mut s.config, key, asked);
        if out.refused.is_none() {
            let before = s.problems.len();
            out.save_ms = s.save();
            if s.problems.len() > before {
                out.save_failed = Some(s.problems[before].clone());
            }
        }
        out
    })
}

/// 这一档自己钉的落点：写盘只准写到 `%TEMP%\\falconshot-settings-door\\pinned-config.toml`。
///
/// 本档会真写文件，而 [`crate::settings`] 读的是 `%APPDATA%\\ai-falconshot\\config.toml` ——
/// 那一枚不钉就是拿用户自己的设置档当量具的草稿纸（§9.1 第十五条那一族：走过产品的门就得把
/// 产品的副作用一起带走）。`pin_writes` 只挪**写**的那一头，读照旧，所以这一档报的
/// `read=` 仍是这台机器真在读的那一份。
pub fn pin() -> PathBuf {
    let dir = door_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut path = dir;
    path.push("pinned-config.toml");
    crate::settings::pin_writes(&path)
}

fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_SETTINGS_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-settings-door");
    dir
}

/// 从**这一档真写了的那份文件**里抠出 `[hot_corner]` 里那一枚键的字面值。
///
/// 不用 `toml`、也不用 config 自己的解析器：那一层和被测的写路共用同一个 `Config`，配不上
/// 就成了两边一起错（`autosave_check` 为同一件事写过同一段话）。找不到就答 `None`，而那
/// 一句「找不到」本身也是这一格要印出来的读数。
fn disk_value(path: &Path, section: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut inside = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            inside = t[1..t.len() - 1].trim() == section;
            continue;
        }
        if inside {
            if let Some(rest) = t.strip_prefix(key) {
                if let Some(v) = rest.trim().strip_prefix('=') {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// headless 的四格：没有屏幕、没有文件、没有会话也判得出。
///
/// 每一格一行，格与行之间不掺措辞：数出来的是枚数、逐字比对的是值。[`check`] 是这一枚的门面。
pub fn table() -> (bool, Vec<String>) {
    let cfg = Config::default();
    let flat = cfg.flat();
    let keys = match flat {
        Ok(f) => f,
        Err(e) => return (false, vec![format!("分类 红：flat() 读不出来 {e}")]),
    };

    let mut rows: Vec<String> = Vec::new();
    let mut ok = true;

    // ① 名单是不是把这份 config 分完了：漏一枚 = 页上看不见这一行；重一枚 = 两格改同一值。
    let unowned: Vec<&String> = keys.keys().filter(|k| owners(k).is_empty()).collect();
    let shared: Vec<(&String, usize)> = keys
        .keys()
        .map(|k| (k, owners(k).len()))
        .filter(|(_, n)| *n != 1)
        .collect();
    let cover = unowned.is_empty() && shared.is_empty();
    ok &= cover;
    rows.push(format!(
        "分类 {}：{} 枚键，无归属 {} 枚 {:?}，多归属 {} 枚 {:?}",
        if cover { "绿" } else { "红" },
        keys.len(),
        unowned.len(),
        unowned,
        shared.len(),
        shared
    ));

    // ② 每一类都得有行：一类空着就是名单上一格点不开的入口，比不列更坏（#433 那一族）。
    let empty: Vec<&str> = CATEGORIES
        .iter()
        .enumerate()
        .filter(|(i, _)| rows_of(&cfg, *i).is_empty())
        .map(|(_, (n, _))| *n)
        .collect();
    ok &= empty.is_empty();
    rows.push(format!(
        "每类有行 {}：{} 类，空类 {:?}",
        if empty.is_empty() { "绿" } else { "红" },
        CATEGORIES.len(),
        empty
    ));

    // ③ 「需重启」这一标只许落在名单上那四枚，一枚不多一枚不少：多了页上就多一处「重启吧」，
    // 少了就是改完不动而用户不知道。热区那一类全不标——§5.15 的采样腿每秒重读一次配置。
    let marked: Vec<String> = (0..CATEGORIES.len())
        .flat_map(|i| rows_of(&cfg, i))
        .filter(|r| r.restart)
        .map(|r| r.key.clone())
        .collect();
    let mut want: Vec<String> = RESTART_KEYS.iter().map(|k| k.to_string()).collect();
    want.sort();
    let mut got = marked.clone();
    got.sort();
    let marks_ok = want == got;
    ok &= marks_ok;
    let corner_marked = rows_of(&cfg, CORNER).iter().filter(|r| r.restart).count();
    rows.push(format!(
        "重启标记 {}：页上标了 {} 枚，名单 {} 枚；热区那 {} 枚里标了 {corner_marked} 枚",
        if marks_ok { "绿" } else { "红" },
        marked.len(),
        want.len(),
        rows_of(&cfg, CORNER).len()
    ));

    // ④ 写的那三条路：改得动、被改了要说什么、拒了的写不许碰值。
    let mut probe = Config::default();
    let before_dwell = probe.get_str("hot_corner.dwell_ms").unwrap_or_default();
    let over = write_into(&mut probe, "hot_corner.dwell_ms", "9000");
    let in_range = write_into(&mut probe, "hot_corner.dwell_ms", "900");
    let corner_before = probe
        .get_str("hot_corner.corners.top_left")
        .unwrap_or_default();
    let bogus = write_into(&mut probe, "hot_corner.corners.top_left", "nope");
    let corner_after = probe
        .get_str("hot_corner.corners.top_left")
        .unwrap_or_default();
    let unknown = write_into(&mut probe, "hot_corner.nope", "1");
    let write_ok = over.corrections.len() == 1
        && over.stored.as_deref() == Some("5000")
        && !over.restart
        && in_range.refused.is_none()
        && in_range.corrections.is_empty()
        && in_range.stored.as_deref() == Some("900")
        && bogus.refused.is_some()
        && corner_after == corner_before
        && unknown.refused.is_some();
    ok &= write_ok;
    rows.push(format!(
        "写路 {}：默认停留 {} ms；打 9000 -> {:?} 校正 {} 条；打 900 -> {:?}；\
         角落打 nope 拒（值仍是 {:?}）；不存在的键 {:?} 拒",
        if write_ok { "绿" } else { "红" },
        before_dwell,
        over.stored,
        over.corrections.len(),
        in_range.stored,
        corner_after,
        unknown.refused.is_some()
    ));

    (ok, rows)
}

/// [`table`] 递给读的人的那一面：一枚判决（还是 bool，梯子里那一格要的就是它），和一行读数。
///
/// 把四格合成一行只在这里做一次，所以 `--selftest` 那一格与 `--settings` 那一档读到的是同一
/// 行字（#281：一枚判据只许写一份，连它的措辞也是）。
pub fn line() -> (bool, String) {
    let (ok, rows) = table();
    (ok, rows.join(" ｜ "))
}

/// [`line`] 换成门的那一枚判决。两个名字、一处合成：梯子里那一格收 bool，档里那一行收
/// [`crate::state::Check`]，而它们读的必须是同一行字。
pub fn check() -> (crate::state::Check, String) {
    let (ok, detail) = line();
    (crate::state::Check::from(ok), detail)
}

/// `--settings` 的那扇门。五行，每行一格，全都在这一台机器上真读过。
///
/// * `settings headless` —— [`check`]，出厂默认那份 config，不需要屏幕也不需要文件。
/// * `settings categories` —— **这一台机器手里那一份** config 的每一类各列了几行，外加页上
///   标了「需重启」的那几枚。
/// * `settings corner page` —— §5.15 那一页的全部行，逐字印出来。
/// * `settings live write` —— §5.20.2 第一条的真对面：写一次 dwell_ms，然后**三处读者**各问
///   一次（进程里那一份、常驻采样腿 `Runner::refresh`、以及盘上这份文件），然后写回去。
/// * `settings refusal` —— §5.20.2 反面那一臂：一枚打不进去的值，内存里的值与落盘次数都不许动。
pub fn measure() -> (crate::state::Check, String) {
    use crate::r13::Report;
    use crate::state::Check;
    use falcon_core::hotcorner::Corner;

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (headless, detail) = check();
    rep.row("settings headless", headless, detail);

    // 页上点的是一行中文标签而不是编号，所以「名字 → 类号 → 那些行」这一整条路都得走一遍真
    // 的：标签写错、名单少一列、反查不到，三件坏事都会在这一格红。编号打在名字后面，读的人
    // 才看得出这一格用的是哪一个号。
    let found: Vec<Option<usize>> = CATEGORIES.iter().map(|(n, _)| index_of(n)).collect();
    let counts: Vec<String> = CATEGORIES
        .iter()
        .enumerate()
        .map(|(i, (name, _))| match found[i] {
            Some(who) => format!("{name}#{who}={}", rows(who).len()),
            None => format!("{name}#反查不到=无"),
        })
        .collect();
    let total: usize = (0..CATEGORIES.len()).map(|i| rows(i).len()).sum();
    let marked: Vec<String> = (0..CATEGORIES.len())
        .flat_map(rows)
        .filter(|r| r.restart)
        .map(|r| r.key.clone())
        .collect();
    let named_ok =
        found.iter().all(|f| f.is_some()) && marked.len() == RESTART_KEYS.len() && total > 0;
    let cat_count = CATEGORIES.len();
    rep.row(
        "settings categories",
        Check::from(named_ok),
        format!(
            "读={}；{cat_count} 类共 {total} 行：{}；标需重启 {} 枚 {:?}",
            crate::settings::with(|s| s.line()),
            counts.join(" "),
            marked.len(),
            marked
        ),
    );

    let corner = rows(CORNER);
    let bound: Vec<String> = Corner::ALL
        .iter()
        .filter_map(|c| {
            corner
                .iter()
                .find(|r| r.key == format!("hot_corner.corners.{}", c.key()))
                .map(|r| r.text())
        })
        .collect();
    let page_ok = !corner.is_empty() && bound.len() == Corner::ALL.len();
    rep.row(
        "settings corner page",
        Check::from(page_ok),
        format!(
            "{} 行：{}；四角 {}",
            corner.len(),
            corner
                .iter()
                .map(|r| r.text())
                .collect::<Vec<_>>()
                .join(" "),
            if bound.is_empty() {
                "(页上没有一角)".to_string()
            } else {
                bound.join(" ")
            }
        ),
    );

    // §5.20.2 第一条要的是「无需重启的设置应即时生效」。生效的对面不是「内存里换了个数」——
    // 那一句在写的那一瞬间就成立，谁都会绿。这里问的是**另一条腿**：常驻采样腿每次快照重读
    // 配置的那一枚函数，和它据以重建的那份几何。显示器答不出来就 Blocked，不算红。
    match crate::capture::monitor_table() {
        Err(e) => {
            rep.row(
                "settings live write",
                Check::Blocked,
                format!("这台机器答不出显示器（{e}）：采样腿没有快照可重读"),
            );
        }
        Ok(monitors) if monitors.is_empty() => rep.row(
            "settings live write",
            Check::Blocked,
            "显示器清单是空的（这一台没答）：没有几何可重建".to_string(),
        ),
        Ok(monitors) => {
            let before = crate::settings::hot_corner_cfg();
            let mut runner = crate::hot_corner::Runner::new(before.clone(), monitors.clone());
            // 挑一枚一定不等于当前值、又落在门的量程里（0..=5000）的数：写一个「改成它自己」
            // 的值是拿一次不动作冒充一次生效。
            let target: u32 = if before.dwell_ms == 900 { 700 } else { 900 };
            let wrote = write("hot_corner.dwell_ms", &target.to_string());
            let after = crate::settings::hot_corner_cfg();
            let moved = runner.refresh(after.clone(), monitors.clone());
            let pinned = crate::settings::with(|s| s.write_to.clone());
            let on_disk = disk_value(&pinned, "hot_corner", "dwell_ms");
            // 把这一档自己改的东西改回去：下一趟（和下一位读这份钉档的人）拿到的还是原来那份。
            let back = write("hot_corner.dwell_ms", &before.dwell_ms.to_string());
            let ok = wrote.ok()
                && wrote.stored.as_deref() == Some(target.to_string().as_str())
                && !wrote.restart
                && after.dwell_ms == target
                && moved
                && runner.config().dwell_ms == target
                && on_disk.as_deref() == Some(target.to_string().as_str())
                && back.ok();
            rep.row(
                "settings live write",
                Check::from(ok),
                format!(
                    "{}；改完三处读者：内存 {}，采样腿 rebuilds={} 读到 {}，盘上 {:?}；{}；\
                     钉的落点 {}",
                    wrote.text(),
                    after.dwell_ms,
                    runner.rebuilds,
                    runner.config().dwell_ms,
                    on_disk,
                    back.text(),
                    pinned.display()
                ),
            );
        }
    }

    // 反面那一臂：拒了的写什么都不能动。落盘次数与值都要同一份读数，因为「拒了但还是写了」
    // 正是这一族唯一能伤到用户的那一种。
    let saves_before = crate::settings::with(|s| s.saves);
    let key = format!("hot_corner.corners.{}", Corner::ALL[0].key());
    let value = crate::settings::with(|s| s.config.get_str(&key).unwrap_or_default());
    let refused = write(&key, "nope");
    let still = crate::settings::with(|s| s.config.get_str(&key).unwrap_or_default());
    let saves_after = crate::settings::with(|s| s.saves);
    let ok = refused.refused.is_some() && still == value && saves_after == saves_before;
    rep.row(
        "settings refusal",
        Check::from(ok),
        format!(
            "{}；值仍 {:?}；落盘次数 {} -> {}",
            refused.text(),
            still,
            saves_before,
            saves_after
        ),
    );

    rep.finish("settings door")
}

// ──────────────────────────────────────────────────────────────────────────────
// §5.20.1 的那一扇窗：界面读法，与 `--settings-window` 那一档的四条腿
// ──────────────────────────────────────────────────────────────────────────────

/// 类名与行交给界面的那一个分隔符。选 `\u{1d}` 而不是 `|`／换行，理由与
/// [`crate::session`] 里 `group_encode` 那一枚相同：**值是用户自己打的**
/// （`annotation.font_family` 里出现一个 `|` 完全合法），任何可见分隔符都会把一行切成两行。
/// 这一串是给 QML 的 `Repeater` 读的，不是给人读的一句话 —— 人要读的那句在
/// [`Row::text`] 与 [`Written::text`] 里，而这两处本档都不重新拼。
pub const RECORD: &str = "\u{1d}";

/// 名单上那几类的标签，顺序就是 §5.20.1 那一列数字、「托盘」插在「快捷键」之后。页上那排
/// 标签的**唯一**来源：QML 里再抄一份
/// 中文名单，就是第十二类与第一类并存的那一种坏（#281）。
pub fn category_line() -> String {
    CATEGORIES
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<&str>>()
        .join(RECORD)
}

/// 某一类那一页的全部行，每行是 [`Row::text`] 原文 —— 所以 `（需重启）` 那三个字的措辞
/// 只在 `Row::text` 里写过一次，窗只是把它画出来。
pub fn page_line(which: usize) -> String {
    rows(which)
        .iter()
        .map(|r| r.text())
        .collect::<Vec<String>>()
        .join(RECORD)
}

/// 页上第 `index` 行管的是哪一枚键、此刻写着什么，两样由 [`RECORD`] 连着交给界面。
///
/// 界面点行时要的是这个数对，而不是「把那一格从第一个 `=` 切开」：后者是同一份语法的第二份
/// 实现（#281），而值里出现 `=`（保存路径、字体名）就会切错。分隔符用 `RECORD` 而不是 `=`
/// 也是同一理由 —— 这一串是给桥读的，不是给人读的一句话。
pub fn row_pair(which: usize, index: usize) -> Option<String> {
    rows(which)
        .get(index)
        .map(|r| format!("{}{RECORD}{}", r.key, r.value))
}

/// 页上第 `index` 行**整格**的样子。窗口读的是 [`page_line`] 那一串切出来的第 `index` 枚，
/// 本档读的是这一枚：两条腿各拿各的手数同一行，才谈得上「对」。
pub fn row_text(which: usize, index: usize) -> Option<String> {
    rows(which).get(index).map(Row::text)
}

/// 一行要的是哪一只手。`Switch` 与 `Field` 是 §5.20.1 那一页上两种**不同形状**的格子，而
/// 哪一种形状归哪一枚键，判的是档里存着的类型（[`Config::get_bool`]），不是页上那串文本。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Kind {
    /// 一枚开关，此刻在这一头。
    Switch(bool),
    /// 一格文本框：这一行没有可翻的一头。
    Field,
}

impl Kind {
    /// 交给界面的那一段：`on` / `off` / `field`。窗只是把它和 `page_line` 那同一行的第 i 枚
    /// 并排画出来，**不在这里猜** —— 于是「这一行是开关」这件事在进程里只有一处判断。
    pub fn token(&self) -> &'static str {
        match self {
            Kind::Switch(true) => "on",
            Kind::Switch(false) => "off",
            Kind::Field => "field",
        }
    }

    pub fn is_switch(&self) -> bool {
        matches!(self, Kind::Switch(_))
    }
}

/// 任意一份 config 里那一枚键要哪一只手。独立出来是因为这一枚判断的分岔本体必须有单测能对着
/// 一份内存里的档问，而 [`crate::settings`] 那一格只有活的那一份。
pub fn kind_of(cfg: &Config, key: &str) -> Kind {
    match cfg.get_bool(key) {
        Some(v) => Kind::Switch(v),
        None => Kind::Field,
    }
}

/// 「这一行翻一头要打进哪一枚值」。`None` = 这一行不是开关，无一头可翻。
///
/// 纯函数，而且与 [`kind_of`] 是同一枚判断的两种读法（不在这里再问一次类型）：翻的目标是
/// **现值的反面**，所以它既不看计划也不看页上那串文本 —— 开关按下去永远改得动，这正是它不需要
/// [`pick`] 那一层避让的原因（#433 那一族恒绿的毛病在定义上成立不了）。
pub fn flip_to(cfg: &Config, key: &str) -> Option<&'static str> {
    match kind_of(cfg, key) {
        Kind::Switch(v) => Some(if v { "false" } else { "true" }),
        Kind::Field => None,
    }
}

/// 页上第 `index` 行要哪一只手。行号越界交回 [`Kind::Field`]：那一行压根不在页上，
/// 画不出开关也画不出文本框，而界面拿到的那一串长度与 [`page_line`] 切出来的长度必须始终一样。
pub fn row_kind(which: usize, index: usize) -> Kind {
    rows(which)
        .get(index)
        .map(|r| crate::settings::with(|s| kind_of(&s.config, &r.key)))
        .unwrap_or(Kind::Field)
}

/// 一整页的手型，用 [`RECORD`] 连着 —— 与 [`page_line`] 一一对应：第 i 枚说的就是页上第 i 行。
///
/// 窗自己不数第 i 行是什么类型，它读这一串。让界面按 `value == "true"` 猜，就是 #281 那一族
/// 的第三次犯（把 `key=value` 从第一个 `=` 切开是同一份语法的第二份实现，猜类型是同一份判断的
/// 第二份实现）。
pub fn kinds_line(which: usize) -> String {
    let cfg = crate::settings::with(|s| s.config.clone());
    rows_of(&cfg, which)
        .iter()
        .map(|r| kind_of(&cfg, &r.key).token())
        .collect::<Vec<&str>>()
        .join(RECORD)
}

/// 一次没动的写：把「为什么不动」原样装进 [`Written::refused`]，值读的是**此刻内存里**那一个。
///
/// 走这一枚而不是让调用方自己拼 `Written`，为的是「拒了」这一形状在本档只有一份 —— 页上印的
/// 那句与量具对的那句因此是同一句（#281）。
fn untouched(key: &str, asked: &str, why: String) -> Written {
    Written {
        key: key.to_string(),
        asked: asked.to_string(),
        stored: crate::settings::with(|s| s.config.get_str(key)),
        corrections: Vec::new(),
        restart: needs_restart(key),
        save_ms: 0,
        refused: Some(why),
        save_failed: None,
    }
}

/// 用户的手按下第 `index` 行的开关：把那枚 bool 翻到另一头，走的正是 [`write`] 那一条完整的路
/// （语法闸、`Settings::save()`、`Written::text`）—— 这里不开第二条落盘的路，否则「页上按一下」
/// 与「按保存」会落成两件不同的事。
///
/// **不是 bool 的行一处也不碰**：交回一句 `refused` 写着「这一行不是开关」的 `Written`。按下去
/// 没有反应是坏，按下去把一枚文本键写成 `false` 更坏（`Config::set_str` 对文本键是把串存下来，
/// 它会成，所以这一层拦不住就得在这一层拦）。
pub fn toggle(which: usize, index: usize) -> Written {
    // 两样都克隆出来：`rows()` 交回的是**临时**那一个 Vec，借着它的一行往下走，`write` 一改
    // 模型就成了一处「读着已被改掉的快照」。克隆这一行只要两个串。
    let (key, shown) = match rows(which).get(index) {
        Some(r) => (r.key.clone(), r.value.clone()),
        None => return untouched("", "", format!("第 {which} 类没有第 {index} 行：无物可翻")),
    };
    let cfg = crate::settings::with(|s| s.config.clone());
    match flip_to(&cfg, &key) {
        Some(head) => write(&key, head),
        None => untouched(
            &key,
            "(翻一头)",
            format!("{key} 存的不是一枚开关（此刻写着 {shown:?}）：这一行没有可翻的一头"),
        ),
    }
}

/// 桥上的那一层：Qt 交回来的是 `i32`，本档的行号是 `usize`。负数不是「倒数第一行」，它是一枚
/// 不可能的号，所以折成 [`toggle`] 那一句「无物可翻」—— 句子仍出自本档，桥上不安另一套措辞。
pub fn toggle_row(which: i32, index: i32) -> Written {
    match (usize::try_from(which), usize::try_from(index)) {
        (Ok(w), Ok(i)) => toggle(w, i),
        _ => untouched(
            "",
            "",
            format!("第 {which} 类第 {index} 行是一枚不可能的号：无物可翻"),
        ),
    }
}

/// 一行的样子，值由调用方给。开工那一刻的读数要用**记住的那个值**拼，而不是现在模型里的值 ——
/// 四条腿跑完之后模型早已不是开工时那份了。
fn text_with(key: &str, value: &str) -> String {
    Row {
        key: key.to_string(),
        value: value.to_string(),
        restart: needs_restart(key),
    }
    .text()
}

/// 把 `asked` 打进**克隆的**那一份 config，问门留下了什么、有没有拒。克隆这一句是本档全部的
/// 分量所在：这道算术一行也不落到盘上、也不动活的那一份，而页上那一格「本该长成什么样」正是
/// 从它算出来的 —— 于是门替用户改过的那些数（9000 变 5000）在这张尺上也算过一次。
fn gate_says(key: &str, asked: &str) -> (Option<String>, bool) {
    let mut cfg = crate::settings::with(|s| s.config.clone());
    let out = write_into(&mut cfg, key, asked);
    (out.stored, out.refused.is_some())
}

/// 「把 `asked` 打进去之后，页上这一行该长成什么样」。被拒的写没动值，所以这一枚对四条腿
/// 都成立：保存那腿算出来是新值，取消与拒那两腿算出来仍是原来那一枚。
fn expected_text(key: &str, asked: &str) -> String {
    let (stored, _) = gate_says(key, asked);
    text_with(key, &stored.unwrap_or_default())
}

/// 下面这四条腿点的键，全在 §5.15 那一页（[`CORNER`]）上：这一档开着的是哪一页，判的就是
/// PRD 第 1328 行那一句「用户打开"首选项 > 热区"」。键名钉在这一档而不是写在 QML 里，
/// 换一枚键不必改界面，而窗照计划做的那件事仍然是真做。
///
/// 值只是**候选**：`arm()` 会先问一次「这一枚键此刻是不是就是这个数」，是的话换下一个。
/// 拿一枚与当前值相同的数去量「改完页上跟着变」，那一张尺永远绿而永远什么也没量到
/// （#433 那一族：恒绿的尺是装饰）。
pub const SAVE_KEY: &str = "hot_corner.dwell_ms";
pub const SAVE_CANDIDATES: [&str; 3] = ["700", "800", "900"];
pub const CANCEL_KEY: &str = "hot_corner.trigger_dip";
pub const CANCEL_CANDIDATES: [&str; 3] = ["12", "13", "14"];
pub const REFUSE_KEY: &str = "hot_corner.corners.top_left";
pub const REFUSE_VALUE: &str = "nope";
/// 第四条腿按的那枚开关。挑 `require_shift` 有理由：它是 bool（所以这一腿量的确实是开关而不是
/// 文本框），而翻成 `true` 只是**多**要一个修饰键 —— 这一趟里热区不会因此更容易触发一次截图，
/// 读完还原成开工那一枚就回到原状。`hot_corner.enabled` 是同一页上更醒目的那一枚，但把它翻成
/// `true` 等于在量具跑的过程中真把热区打开，那是拿用户的桌面当草稿纸。
///
/// bool 没有「候选值」可言：翻一头必然是另一个数，所以这一腿不需要 [`pick`] 那一层避让，
/// 也不会有 #433 那一族恒绿的毛病（当前值与目标值相同这件事在开关上定义上不成立）。
pub const TOGGLE_KEY: &str = "hot_corner.require_shift";

/// 窗开起来之后隔多久跑腿。这一格要的是「窗已经在桌面上了」而不是「文档编译过了」，
/// 而 Qt 从 `createObject` 到第一帧之间那段时间没有任何一条腿能借。
pub const LEG_MS: i32 = 400;
/// 这一档的地板：腿的等待 + 读回的余量。比 `--hint` 那一档宽，因为这里跑的是**整份名单逐页 +
/// 五条腿**，每一页都是过一次桥。
///
/// 1,600 → 2,400：P160 把逐页那一趟从「每页一次桥」改成「每页两次桥」（引导那一句也要过一次），
/// 又在中间加了一条真会落盘的腿（「知道了」写那份 `guide_seen.txt`）。这条地板是闹钟的下限，
/// 不是某一格的读数 —— 加它的原因是「腿多了而闹钟没多」这一种坏会红的不是判据、是整趟没跑完。
pub const MIN_WINDOW_MS: i32 = 2_400;

/// `--settings-window` 那一档在自己这一趟里看见过的东西。
///
/// 四条腿各存一份 `(行号, 键, 要打进去的值, 开工那一刻的值)`（开关那一腿没有「要打进去的值」，
/// 只有开工那一刻的那一头 —— 反面是档里的类型算出来的，不是计划里钉的）：行号是**这一趟**在 §5.15 那一页
/// 上的行号，开工那一刻的值存在这里，为的是读完回来能对一句「这一趟有没有把用户的设置留在
/// 别处」—— 那一句的答案必须是「没有，还是开工时那个」，而不是「大概是吧」。
#[derive(Debug)]
struct WindowDoor {
    armed: bool,
    /// 没开成窗的原因。`armed == false` 时这一句就是 Blocked 那一格的答案。
    reason: String,
    page: usize,
    plan: String,
    lines: Vec<String>,
    /// `arm()` 起表的那一刻，窗递回来的每一行按它算一枚「落在第几毫秒」的戳。戳只喂给第 ① 格
    /// 那一句话当读数（[`seen`]），不进任何一格的判据，也不并进 `lines` —— 那一份是窗
    /// 递回来的**原文**，四条腿的判据全在上面 split，掺一个数进去就是把判据改了（#302）。
    ///
    /// 起点取 `arm()` 而不是 `state::quit_after()`：前者在 `engine.load()` 之前，所以这个数只会
    /// 比闹钟实际烧掉的**更大，不会更小**。本轮要问的是「窗出生太晚、腿没跑完就退场」这一形在
    /// 红趟与绿趟之间到底差了多少毫秒，跨趟比的是同一枚起点，不是闹钟的账。
    armed_at: Option<Instant>,
    stamps: Vec<u128>,
    saves_at_arm: u32,
    save_idx: usize,
    save_asked: String,
    save_at_arm: String,
    cancel_idx: usize,
    cancel_draft: String,
    cancel_at_arm: String,
    refuse_idx: usize,
    refuse_at_arm: String,
    toggle_idx: usize,
    toggle_at_arm: String,
    /// §5.21.2 第四枚：`arm()` 那一刻，每一页各自欠着的那一句引导有多长（字数），还欠几枚，
    /// 以及本进程已经动过那份状态文件几回。
    ///
    /// 三枚都存在这里而不是到 `window_measure` 再去问，是同一条时刻的规矩（§9.1 第十条续）：
    /// 窗在 `engine.load()` 之后会**真按一次「知道了」**，那一按改变了 `guide::current()` 答
    /// 的每一页。事后去问桥，问的是按完之后的世界，而窗递回来的那逐页一行读的是按之前那一刻 ——
    /// 拿后者对前者，第一页那一格永远红，而产品是对的。
    guide_expect: Vec<usize>,
    guide_pending: usize,
    guide_attempts: u32,
}

impl WindowDoor {
    const fn none() -> Self {
        Self {
            armed: false,
            reason: String::new(),
            page: CORNER,
            plan: String::new(),
            lines: Vec::new(),
            armed_at: None,
            stamps: Vec::new(),
            saves_at_arm: 0,
            save_idx: 0,
            save_asked: String::new(),
            save_at_arm: String::new(),
            cancel_idx: 0,
            cancel_draft: String::new(),
            cancel_at_arm: String::new(),
            refuse_idx: 0,
            refuse_at_arm: String::new(),
            toggle_idx: 0,
            toggle_at_arm: String::new(),
            guide_expect: Vec::new(),
            guide_pending: 0,
            guide_attempts: 0,
        }
    }
}

static WINDOW: Mutex<WindowDoor> = Mutex::new(WindowDoor::none());

fn with_window<R>(f: impl FnOnce(&mut WindowDoor) -> R) -> R {
    let mut held = WINDOW.lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut held)
}

fn reported(prefix: &str) -> Vec<String> {
    with_window(|w| {
        w.lines
            .iter()
            .filter_map(|l| l.split_once('|'))
            .filter(|(k, _)| *k == prefix)
            .map(|(_, v)| v.to_string())
            .collect()
    })
}

/// 窗一共递回来了哪些行，每一行末尾挂一枚「这一行落在 `arm()` 之后第几毫秒」的戳。
///
/// 零行的那一格用它把「哪一种坏」印出来而不是猜：`born` 没有是窗没被造出来，`born` 有而 `tick`
/// 没有是那条 Timer 腿没跑，`tick` 有而 `open` 没有是 `runGauge` 半路断了。三样都是同一句读数的
/// 不同形状，判据一条也没多写（#281）。
///
/// 后面那一截 `@Nms` 是把「Timer 腿没跑」这一形再问一句「窗是不是生得太晚、闹钟先响了」的尺：
/// 红趟与绿趟可以跑同一枚 exe，那时 `born` 有没有、`tick` 缺不缺都说不清迟到多少毫秒，而「出生
/// 太晚」与「Timer 坏了」要修的是不同的码。戳不写进 `WindowDoor::lines`（那一份是窗递回来的
/// 原文，四条腿的判据全在上面 split），只在这里拼上 —— 它不进任何一格的判定。
pub fn seen() -> Vec<String> {
    with_window(|w| {
        w.lines
            .iter()
            .enumerate()
            .map(|(i, l)| match w.stamps.get(i) {
                Some(ms) => format!("{l} @{ms}ms"),
                None => l.clone(),
            })
            .collect()
    })
}

/// 计划那一串，十四段：`腿等待 ms | 开在哪一类 | 保存的行号 | 保存的键 | 保存的值 |
/// 取消的行号 | 取消的键 | 取消那份草稿 | 拒的行号 | 拒的键 | 拒的值 | 开关的行号 | 开关的键 |
/// 带那两枚钮的是哪一页`。
///
/// 缺一窗里那条腿就不跑（同 `pinGaugePlan` 那一族：空串 = 产品会话与另外七道门，界面这几条腿
/// 整条躲开）。行号与键**两样都给**是有意的：窗拿行号定位那一行，拿键核对「这一行确实是我说
/// 的要动的那一枚」，两者对不上时窗什么都不动 —— 指错一行去写值是比不写更坏的那一种。
///
/// 最后那两枚没有「值」那一段：开关打的不是串，翻的是此刻那一头的**反面**，而反面是档里的类型
/// 与内存里的现值算出来的 —— 计划里写死一枚 `true` 就等于让窗去猜「翻完该长成什么样」。
pub fn plan_line() -> String {
    with_window(|w| w.plan.clone())
}

/// 窗读它自己那一格的入口：这一趟该不该有一扇设置窗（`main.qml` 在
/// `Component.onCompleted` 里叫一次 [`crate::session`] 那一枚 `refreshSettings`，
/// 那一枚把这里的答复抬进 qproperty）。
///
/// **两档都要那一扇窗**：`--settings-window` 的四条腿与 §5.20.3 那两枚钮点的是同一扇
/// [`crate::settings_view`] 造出来的窗，而 `settings_open` 只有一枚 —— 谁 armed 就造，
/// 造出来之后窗照自己手里那副计划决定跑哪几条腿（两档互相拒并列，见 `main.rs` 的 refusals，
/// 所以这一趟至多只有一副计划）。
pub fn window_wanted() -> bool {
    with_window(|w| w.armed) || io_wanted() || pal_wanted()
}

/// 从候选里挑一枚**与此刻那个值不一样**的。全都一样（或键压根不在页上）就返回 `None`，
/// `arm` 据此不开窗并说清为什么。
fn pick(candidates: &[&'static str; 3], current: &str) -> Option<&'static str> {
    candidates.iter().copied().find(|c| *c != current)
}

/// 开档时在册：找到四枚键在页上的行号、记下它们当时的值与落盘次数，再拼出计划。返回的那句话
/// 是打印给读档人看的「这一趟动了谁」，不是判据。
///
/// 它必须在 `engine.load()` **之前**跑完，理由与 `--autosave` 那一档写在同一段注释里：
/// `SettingsWindow.qml` 只在 `Component.onCompleted` 读一次计划。
pub fn arm() -> String {
    let page_rows = rows(CORNER);
    // §5.21.2 第四枚的前置，与上面那四枚键一样**必须写在 `engine.load()` 之前**：窗里那条腿会
    // 真按一次「知道了」，按过之后 `guide::current()` 答的已经不是这一趟开局那一份状态。基准
    // 取在按之前，事后那一格才有对得像（§9.1 第十条续，与 `--autosave` 为计数器记的那句同源）。
    let guide_at_arm = crate::guide::current();
    let guide_expect: Vec<usize> = (0..CATEGORIES.len())
        .map(|i| crate::guide::line_for(&guide_at_arm, i).chars().count())
        .collect();
    let guide_pending = guide_at_arm.pending();
    let guide_attempts = crate::guide::attempts();
    let guide_line = guide_expect
        .iter()
        .enumerate()
        .map(|(i, n)| format!("{}#{i}={n}", CATEGORIES[i].0))
        .collect::<Vec<String>>()
        .join(" ");
    let at_arm = |key: &str| -> String {
        page_rows
            .iter()
            .find(|r| r.key == key)
            .map(|r| r.value.clone())
            .unwrap_or_default()
    };
    let idx = |key: &str| page_rows.iter().position(|r| r.key == key);

    let (si, ci, ri, ti) = (
        idx(SAVE_KEY),
        idx(CANCEL_KEY),
        idx(REFUSE_KEY),
        idx(TOGGLE_KEY),
    );
    let (save_at_arm, cancel_at_arm, refuse_at_arm, toggle_at_arm) = (
        at_arm(SAVE_KEY),
        at_arm(CANCEL_KEY),
        at_arm(REFUSE_KEY),
        at_arm(TOGGLE_KEY),
    );
    let asked = [
        si.and_then(|_| pick(&SAVE_CANDIDATES, &save_at_arm)),
        ci.and_then(|_| pick(&CANCEL_CANDIDATES, &cancel_at_arm)),
    ];
    // 开关那一腿的前置与另外三腿**不是一件事**：另外三腿要的是「有一个改得动的值」，这一腿要的是
    // 「这一行是一枚开关」。所以这里过滤的是类型而不是候选：类型由 [`row_kind`] 判（档里那一位），
    // 页上只画。指一枚文本行去翻一头，窗会照着计划把一行画成开关、把值写成 `false`。
    let sw = ti.filter(|i| matches!(row_kind(CORNER, *i), Kind::Switch(_)));
    let saves_at_arm = crate::settings::with(|s| s.saves);
    // 计划第 13 段（0 起）是「哪一页带那两枚钮」的类号，由本文件算：窗自己数一遍，名单一改就留下
    // 两份「关于与更新是第几页」。名单真没有那一页时交回 `CATEGORIES.len()` —— 那一枚页号换不过去，
    // 最后一格判红而不是判绿。
    let about = index_of(ABOUT_PAGE).unwrap_or(CATEGORIES.len());

    let reason = match (si, ci, ri, ti, sw, asked[0], asked[1]) {
        (Some(si), Some(ci), Some(ri), _, Some(ti), Some(save_asked), Some(cancel_draft)) => {
            let plan = format!(
                "{LEG_MS}|{CORNER}|{si}|{SAVE_KEY}|{save_asked}|{ci}|{CANCEL_KEY}|{cancel_draft}|{ri}|{REFUSE_KEY}|{REFUSE_VALUE}|{ti}|{TOGGLE_KEY}|{about}"
            );
            with_window(|w| {
                w.armed = true;
                w.armed_at = Some(Instant::now());
                w.reason = String::new();
                w.page = CORNER;
                w.plan = plan.clone();
                w.saves_at_arm = saves_at_arm;
                w.save_idx = si;
                w.save_asked = save_asked.to_string();
                w.save_at_arm = save_at_arm.clone();
                w.cancel_idx = ci;
                w.cancel_draft = cancel_draft.to_string();
                w.cancel_at_arm = cancel_at_arm.clone();
                w.refuse_idx = ri;
                w.refuse_at_arm = refuse_at_arm.clone();
                w.toggle_idx = ti;
                w.toggle_at_arm = toggle_at_arm.clone();
                w.guide_expect = guide_expect.clone();
                w.guide_pending = guide_pending;
                w.guide_attempts = guide_attempts;
            });
            return format!(
                "计划 {plan}；开在 热区#{CORNER}（{} 行）；开工时四枚键 = {:?} / {:?} / {:?} / {:?}；落盘次数 {saves_at_arm}；引导落点={}（开工时还欠 {guide_pending} 枚、本趟之前动过盘 {guide_attempts} 回）；开工时逐页引导字数 {} 枚 {}",
                page_rows.len(),
                save_at_arm,
                cancel_at_arm,
                refuse_at_arm,
                toggle_at_arm,
                crate::guide::path().display(),
                guide_expect.len(),
                guide_line
            );
        }
        (None, _, _, _, _, _, _) => {
            format!("{SAVE_KEY} 不在 热区 那一页上（{} 行里没有这一行）", page_rows.len())
        }
        (_, None, _, _, _, _, _) => {
            format!("{CANCEL_KEY} 不在 热区 那一页上（{} 行里没有这一行）", page_rows.len())
        }
        (_, _, None, _, _, _, _) => {
            format!("{REFUSE_KEY} 不在 热区 那一页上（{} 行里没有这一行）", page_rows.len())
        }
        (_, _, _, None, _, _, _) => {
            format!("{TOGGLE_KEY} 不在 热区 那一页上（{} 行里没有这一行）", page_rows.len())
        }
        (Some(_), Some(_), Some(_), Some(i), None, _, _) => format!(
            "{TOGGLE_KEY} 在页上（第 {i} 行）但不是一枚开关：这一趟没有开关可按，不开窗"
        ),
        (Some(_), Some(_), Some(_), _, Some(_), None, _) => format!(
            "{SAVE_KEY} 此刻已是候选之一（{save_at_arm:?} 对 {:?}）：没有一枚改得动的值，这一趟不开窗",
            SAVE_CANDIDATES
        ),
        (Some(_), Some(_), Some(_), _, Some(_), Some(_), None) => format!(
            "{CANCEL_KEY} 此刻已是候选之一（{cancel_at_arm:?} 对 {:?}）：没有一枚草稿可种，这一趟不开窗",
            CANCEL_CANDIDATES
        ),
    };
    with_window(|w| {
        w.armed = false;
        w.reason = reason.clone();
        w.plan = String::new();
    });
    reason
}

/// 界面递回来的一行。前缀是那一格的名字，其余原样存着，读回时按前缀挑。
///
/// 一本账还是两本账：上面那一档（`--settings-window`）与 §5.20.3 那一档（`--settings-io`）
/// 共用桥上 `settingsGaugeReport` 那**一枚** invokable —— 桥上要是有两枚入口，页上就得自己写
/// 「我是哪一档」那一层判断，那是措辞不是读数。所以分派在这里做，依据是「这一趟哪一档 armed」；
/// 两档互相拒并列（`main.rs` 的 refusals），因此至多一本，产品会话里两本都不收。
pub fn report(line: &str) {
    if pal_takes_report() {
        with_pal(|p| p.lines.push(line.to_string()));
    } else if io_takes_report() {
        with_io(|i| i.lines.push(line.to_string()));
    } else {
        with_window(|w| {
            let at = w.armed_at.map(|t| t.elapsed().as_millis()).unwrap_or(0);
            w.lines.push(line.to_string());
            w.stamps.push(at);
        });
    }
}

/// `--settings-window` 的那扇门。七行，每行一格，全都对面是**这一趟真开着的那扇窗**。
///
/// 与 [`measure`] 的分工写在这里而不是含糊过去：那一档五格读的是**模型**（`rows` /
/// `write_into` / 采样腿 / 盘上那份文件），一扇窗也不开；这一档六格读的是**界面**——每一格
/// 的对面都是 `SettingsWindow.qml` 经过桥上那几枚 invokable 递回来的话，而本档手里
/// 同一件事的另一份读数（模型自己数的行数、config 里此刻的值）用来对，不用来说。
/// 两档读同一枚键却各问各的问题，所以两档都要在。
pub fn window_measure(want_ms: i32) -> (crate::state::Check, String) {
    use crate::r13::Report;
    use crate::state::Check;

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (
        armed,
        reason,
        page,
        saves_at_arm,
        save_idx,
        save_asked,
        save_at_arm,
        cancel_idx,
        cancel_draft,
        cancel_at_arm,
        refuse_idx,
        refuse_at_arm,
        toggle_idx,
        toggle_at_arm,
        guide_expect,
        guide_pending,
        guide_attempts,
    ) = with_window(|w| {
        (
            w.armed,
            w.reason.clone(),
            w.page,
            w.saves_at_arm,
            w.save_idx,
            w.save_asked.clone(),
            w.save_at_arm.clone(),
            w.cancel_idx,
            w.cancel_draft.clone(),
            w.cancel_at_arm.clone(),
            w.refuse_idx,
            w.refuse_at_arm.clone(),
            w.toggle_idx,
            w.toggle_at_arm.clone(),
            w.guide_expect.clone(),
            w.guide_pending,
            w.guide_attempts,
        )
    });
    if !armed {
        // 这一档压根没被点开过：门自己不该跑到这里，但走到了就要说清楚，不能拿空列表判红。
        rep.row(
            "settings window",
            Check::Blocked,
            format!("这一趟没有 {want_ms} ms 的计划：{reason}"),
        );
        return rep.finish("settings-window door");
    }

    // ① 窗在不在、开在哪一类、页上有没有行。`visible` 是窗自己问 Qt 的那一枚，不是「我们
    // 叫过 show()」：HintWindow 那一档学到的正是这两件事可以不一样。
    let open: Vec<String> = reported("open");
    if open.is_empty() {
        // 两种坏要分开印，而且不在这格里猜哪一种：把窗真递回来的**所有**行原样印出来。gen A 印出
        // 的是零行，探针两趟印出的是 `born`（窗在、当时那份计划 11 段）而没有 `open` —— 这一格不改判据，
        // 它只让下一趟不必再靠猜。
        let seen_lines = seen();
        rep.row(
            "settings window",
            Check::Blocked,
            format!(
                "窗没有递回 `open` 行（qml_loaded={}）；窗一共递回 {} 行 {:?}（每行末尾 `@Nms` = \
                 该行落在 `arm()` 之后 N 毫秒，本趟闹钟 {want_ms} ms —— 出生晚到贴着闹钟，与 Timer \
                 没触发，是两种要修在不同地方的坏）",
                crate::state::qml_loaded(),
                seen_lines.len(),
                seen_lines
            ),
        );
    } else {
        let f: Vec<&str> = open[0].split('|').collect();
        let want_rows = rows(page).len();
        let ok = f.len() == 4
            && f[0].parse::<usize>() == Ok(page)
            && f[1] == "1"
            && f[2].parse::<usize>() == Ok(CATEGORIES.len())
            && f[3].parse::<usize>() == Ok(want_rows);
        rep.row(
            "settings window",
            Check::from(ok),
            format!(
                "页={} 可见={} 类={} 行={}；模型说该类 {} 行（{}）",
                f.first().unwrap_or(&"?"),
                f.get(1).copied().unwrap_or("?"),
                f.get(2).copied().unwrap_or("?"),
                f.get(3).copied().unwrap_or("?"),
                want_rows,
                crate::state::qml_loaded()
            ),
        );
    }

    // ② 整份名单逐页过一遍桥：这一格才是 §5.20.1 那一份名单加「托盘」在**窗上**的读数。类号、
    // 行数两样都要对上，而「对不上」的两种坏（少一页、页数错）在这一行里分开印。
    let pages: Vec<String> = reported("page");
    let model: Vec<usize> = (0..CATEGORIES.len()).map(|i| rows(i).len()).collect();
    let mut seen = vec![None; CATEGORIES.len()];
    let mut stray = 0usize;
    for line in &pages {
        let mut it = line.split('|');
        match (it.next().and_then(|v| v.parse::<usize>().ok()), it.next()) {
            (Some(i), Some(count)) if i < CATEGORIES.len() => seen[i] = count.parse::<usize>().ok(),
            _ => stray += 1,
        }
    }
    let missing: Vec<usize> = (0..CATEGORIES.len())
        .filter(|i| seen[*i].is_none())
        .collect();
    let wrong: Vec<String> = (0..CATEGORIES.len())
        .filter(|i| seen[*i].is_some_and(|c| c != model[*i]))
        .map(|i| {
            format!(
                "{}#{}={}",
                CATEGORIES[i].0,
                i,
                seen[i].map_or("-".to_string(), |c| c.to_string())
            )
        })
        .collect();
    let pages_ok = !pages.is_empty() && missing.is_empty() && wrong.is_empty() && stray == 0;
    rep.row(
        "settings pages",
        Check::from(pages_ok),
        format!(
            "窗递回 {} 页；模型 {} 类共 {} 行 {:?}；窗上缺 {:?}、不符 {:?}、越界 {stray} 行",
            pages.len(),
            CATEGORIES.len(),
            model.iter().sum::<usize>(),
            model,
            missing,
            wrong
        ),
    );

    // 本档这一侧的读数一律走这一枚：它问的是**活的**那一份 config 此刻写着什么，与窗递回来的
    // 那些行文本是两侧的数。
    let value_of = |key: &str| crate::settings::with(|s| s.config.get_str(key).unwrap_or_default());

    // ③ 保存那一腿：窗报的是「按之前页上那一行写着什么」与「按下保存之后页上那一行写着什么」，
    // 本档拿**模型自己算出来的**那两枚样子去对（`text_with` / `expected_text`），再加第三次读 ——
    // 活的 config 里那枚键现在是什么。三枚都对上，§5.20.2 第三条那一支「明确的保存」才算过了屏：
    // 窗自己说「我改了那一格」在这里不算数，那一句是措辞不是读数（#455）。
    //
    // 对完之后本档自己动手把这一枚键改回开工时那个值，并把「改回之后页上那一行」也读一次：
    // 这一格判的是两件事 —— 页跟着值走、页也跟着还原走。
    let saves: Vec<String> = reported("save");
    let sf: Vec<&str> = saves
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let want_before = text_with(SAVE_KEY, &save_at_arm);
    let want_after = expected_text(SAVE_KEY, &save_asked);
    let want_value = gate_says(SAVE_KEY, &save_asked)
        .0
        .unwrap_or_else(|| "(门说这一枚键不存在)".to_string());
    let moved = sf.len() == 4
        && sf[0] == SAVE_KEY
        && sf[1] == save_asked
        && sf[2] == want_before
        && sf[3] == want_after
        && value_of(SAVE_KEY) == want_value
        && want_value != save_at_arm;
    let back = write(SAVE_KEY, &save_at_arm);
    let page_back = row_text(page, save_idx);
    let restored = back.ok()
        && value_of(SAVE_KEY) == save_at_arm
        && page_back.as_deref() == Some(want_before.as_str());
    rep.row(
        "settings page save",
        Check::from(moved && restored),
        format!(
            "窗那侧 {}；本档算出的两枚样子 {:?} -> {:?}；config 这侧 开工 {:?} -> 改后 {:?}\
             （门留下 {:?}）；还原 {}，还原后 config {:?}、页上 {:?}",
            saves
                .first()
                .cloned()
                .unwrap_or_else(|| "(窗没递 save)".to_string()),
            want_before,
            want_after,
            save_at_arm,
            value_of(SAVE_KEY),
            want_value,
            back.text(),
            save_at_arm,
            page_back,
        ),
    );

    // ④ 取消那一腿：草稿进了输入框、点的是取消 ⇒ 那一行的文本一次也不许变，而本档这侧那枚键
    // 也还是开工那一个。窗递回来的是**两次读**（种草稿之前、按取消之后），两枚必须逐字相同 ——
    // 「草稿没成为值」这件事在屏幕上只有一种样子：那一格根本没动过。
    let cancels: Vec<String> = reported("cancel");
    let cf: Vec<&str> = cancels
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let cancel_row = text_with(CANCEL_KEY, &cancel_at_arm);
    let cancel_back = row_text(page, cancel_idx);
    let cancel_ok = cf.len() == 4
        && cf[0] == CANCEL_KEY
        && cf[1] == cancel_draft
        && cf[1] != cancel_at_arm
        && cf[2] == cf[3]
        && cf[3] == cancel_row
        && cancel_back.as_deref() == Some(cancel_row.as_str())
        && value_of(CANCEL_KEY) == cancel_at_arm;
    rep.row(
        "settings page cancel",
        Check::from(cancel_ok),
        format!(
            "窗那侧 {}；本档这侧 {:?}（开工 {:?}；草稿 {:?} 从未成为值，页上该行始终是 {:?}，\
             按行号 {cancel_idx} 读回的是 {:?}）",
            cancels
                .first()
                .cloned()
                .unwrap_or_else(|| "(窗没递 cancel)".to_string()),
            value_of(CANCEL_KEY),
            cancel_at_arm,
            cancel_draft,
            cancel_row,
            cancel_back
        ),
    );

    // ⑤ 拒那一腿：语法闸拦下的写在页上也不许留下痕迹。这一格只数「那一行变了没有」——措辞由
    // `Written::text` 说，页上就是那句话的对面，而本档手里那枚键值仍是开工那一个。它正是
    // §5.20.2 反面那一臂（「拒了的写一行都不许动」）在屏幕这一侧的对面。
    let refuses: Vec<String> = reported("refuse");
    let rf: Vec<&str> = refuses
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let refuse_row = text_with(REFUSE_KEY, &refuse_at_arm);
    let refuse_back = row_text(page, refuse_idx);
    let refuse_ok = rf.len() == 4
        && rf[0] == REFUSE_KEY
        && rf[1] == REFUSE_VALUE
        && rf[1] != refuse_at_arm
        && rf[2] == rf[3]
        && rf[3] == refuse_row
        && refuse_back.as_deref() == Some(refuse_row.as_str())
        && value_of(REFUSE_KEY) == refuse_at_arm;
    rep.row(
        "settings page refusal",
        Check::from(refuse_ok),
        format!(
            "窗那侧 {}；本档这侧 {:?}（开工 {:?}；页上该行 {:?} 一次也没变，按行号 {refuse_idx} \
             读回的是 {:?}）",
            refuses
                .first()
                .cloned()
                .unwrap_or_else(|| "(窗没递 refuse)".to_string()),
            value_of(REFUSE_KEY),
            refuse_at_arm,
            refuse_row,
            refuse_back
        ),
    );

    // ⑤½ 开关那一腿：这一格量的是**另一种手**。上面三腿走的是「选中一行 → 往文本框里打一串 →
    // 按保存」，而 §5.20.2 那一页上 bool 那几枚键要的形状是一枚开关 —— 页上把一行画成开关、
    // 还是画成文本框，这个判断本身过去没有任何对面（§10 第 150 项③「量具的手 ≠ 用户的手」）。
    //
    // 三条判据各管一件事，缺一枚就有一种坏能绿：
    //  1. 那一行翻了一头（`before` / `after` 与本档从**克隆的那一份**算出的两枚样子逐字相同）；
    //  2. 页上画出的开关**枚数**与本档数的枚数相同 —— 这一枚才是「那一行确实被画成了开关」的
    //     读数，不是「Rust 说它是开关」的复述：窗数的是它自己手里那串 `kinds` 切出来的长度；
    //  3. 本档读回该行的手型仍是翻过之后的那一头（`on` / `off` 跟着值走，不是跟着画走）。
    let flips: Vec<String> = reported("toggle");
    let ff: Vec<&str> = flips
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let asked_head = if toggle_at_arm == "true" {
        "false"
    } else {
        "true"
    };
    let want_before = text_with(TOGGLE_KEY, &toggle_at_arm);
    let want_after = expected_text(TOGGLE_KEY, asked_head);
    let want_value = gate_says(TOGGLE_KEY, asked_head)
        .0
        .unwrap_or_else(|| "(门说这一枚键不存在)".to_string());
    let cfg_now = crate::settings::with(|s| s.config.clone());
    let model_switches = rows_of(&cfg_now, page)
        .iter()
        .filter(|r| kind_of(&cfg_now, &r.key).is_switch())
        .count();
    let counted = ff.len() == 4
        && ff[0] == TOGGLE_KEY
        && ff[2] == want_after
        && ff[1] == want_before
        && ff[3].parse::<usize>() == Ok(model_switches)
        && model_switches > 0
        && value_of(TOGGLE_KEY) == want_value
        && want_value != toggle_at_arm;
    let head_now = row_kind(page, toggle_idx).token();
    let want_head = if want_value == "true" { "on" } else { "off" };
    rep.row(
        "settings row switch",
        Check::from(counted && head_now == want_head),
        format!(
            "窗那侧 {}；本档算出的两枚样子 {:?} -> {:?}（翻一头打的是 {asked_head:?}）；\
             config 这侧 开工 {:?} -> 翻后 {want_value:?}；页上画出的开关 窗数 {:?} / 本档数 {model_switches} 枚；\
             按行号 {toggle_idx} 读回的手型 {head_now:?}（要的是 {want_head:?}）",
            flips
                .first()
                .cloned()
                .unwrap_or_else(|| "(窗没递 toggle)".to_string()),
            want_before,
            want_after,
            toggle_at_arm,
            ff.get(3).copied().unwrap_or("?"),
        ),
    );

    // ⑤¾ §5.21.2 第四枚的两格。第一格量的是「逐页过一次桥，每一页手里那句引导有多长」，
    // 对面是 `arm()` 那一刻钉下的 [`WindowDoor::guide_expect`] —— 不是 `window_measure` 再去问
    // 一次桥：窗在这一格之前已经真按过一次「知道了」，那时桥答的是按完之后的世界。
    //
    // 三种坏分开印：少一行（窗没逐页问）、数错（问到了但不是那一句）、越界（窗自己拼了一行不存在的
    // 类号）。第二格量的是**只递一次**那一半：按之前窗手里有那一句、那一块真的画着（`visible`）
    // 且块里的字数与手里那句相同（属性有而块里没写上去 = 绑定断了），按之后两句都归零。
    let guides: Vec<String> = reported("guide");
    let mut got: Vec<Option<usize>> = vec![None; CATEGORIES.len()];
    let mut stray_guide = 0usize;
    for line in &guides {
        let mut it = line.split('|');
        match (it.next().and_then(|v| v.parse::<usize>().ok()), it.next()) {
            (Some(i), Some(n)) if i < CATEGORIES.len() => got[i] = n.parse::<usize>().ok(),
            _ => stray_guide += 1,
        }
    }
    let missing_g: Vec<usize> = (0..CATEGORIES.len())
        .filter(|i| got[*i].is_none())
        .collect();
    let wrong_g: Vec<String> = (0..CATEGORIES.len())
        .filter(|i| got[*i].is_some_and(|c| guide_expect.get(*i).copied().unwrap_or(0) != c))
        .map(|i| {
            format!(
                "{}#{}=窗{:?}/开工{}",
                CATEGORIES[i].0,
                i,
                got[i],
                guide_expect.get(i).copied().unwrap_or(0)
            )
        })
        .collect();
    let handed_at_arm = guide_expect.iter().filter(|n| **n > 0).count();
    rep.row(
        "settings guide census",
        Check::from(
            missing_g.is_empty() && wrong_g.is_empty() && stray_guide == 0 && handed_at_arm > 0,
        ),
        format!(
            "窗递回 guide 行 {} 行（逐页一行）；缺页={} 数不对={} 越界={stray_guide}；\
             开工那一刻欠着引导的页={handed_at_arm} 枚 / 名单里 {} 枚 / 状态里还欠 {guide_pending} 枚",
            guides.len(),
            if missing_g.is_empty() {
                "无".to_string()
            } else {
                missing_g
                    .iter()
                    .map(|i| format!("{}#{i}", CATEGORIES[*i].0))
                    .collect::<Vec<String>>()
                    .join(",")
            },
            if wrong_g.is_empty() {
                "无".to_string()
            } else {
                wrong_g.join(",")
            },
            crate::guide::ENTRIES.len(),
        ),
    );

    let acked: Vec<String> = reported("guideack");
    let gf: Vec<&str> = acked
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let want_before = guide_expect.get(page).copied().unwrap_or(0);
    let is_complex = crate::guide::body(page).is_some();
    // 最后那两枚是钮上的字：页上画的与桥此刻说的。比的是**串**而不是长度 —— 「知道了」与
    // 「我知道了」长度不同，但「知道了」与「知道了 」这种只差一格的坏，长度照样抓不住。
    //
    // `reported()` 已经在第一枚 `|` 处切过并只交回尾巴，所以这里的 gf[0] 是**类号**而不是前缀 ——
    // 上一稿把前缀也算进段数，于是每一枚下标都往后挪了一位：那一稿从没被窗递回过一行，
    // 所以这个坏一直没开口（见 V1.161 块 ④）。段数现在就印在这一格里。
    let want_shape = 8;
    let got_shape = gf.len();
    let word_same = gf.len() == want_shape && gf[6] == gf[7] && !gf[6].is_empty();
    let ack_ok = gf.len() == want_shape
        && gf[0].parse::<usize>() == Ok(page)
        && gf[1].parse::<usize>() == Ok(want_before)
        && gf[2] == "1"
        && gf[3].parse::<usize>() == Ok(want_before)
        && gf[4].parse::<usize>() == Ok(0)
        && gf[5] == "0"
        && is_complex
        && want_before > 0
        && word_same;
    rep.row(
        "settings guide ack",
        Check::from(ack_ok),
        format!(
            "窗递回 guideack 行 {:?}（递回 {got_shape} 段，要 {want_shape} 段：类号|收之前手里字数|收之前画没画|\
             收之前块里字数|收之后手里字数|收之后画没画|钮上画的字|桥说该写的字）；开工那一刻这一页欠 \
             {want_before} 字（复杂页={is_complex}）；钮上那两枚字对上了={word_same}；本趟动过那份状态文件 {} 回",
            acked.first().cloned().unwrap_or("(窗没递)".to_string()),
            crate::guide::attempts().saturating_sub(guide_attempts),
        ),
    );
    // 这一格不是判据，是读数：那一趟按下去之后，盘上那份文件现在多大、里面收下过几枚。
    rep.note("settings guide file", crate::guide::file_summary());

    // ⑥ 副作用那一格：这一趟真落盘几次。窗里保存一次 + 窗里开关一次 + 本档还原那两枚键各一次
    // = 四，多一位就是别处也在写，少一位就是那几条腿里有一条压根没落盘（而页上却写着成了）。
    // 这一格不参与上面任何一格的判据，它判的是「这一档有没有把用户的设置档当草稿纸」。
    let back_toggle = write(TOGGLE_KEY, &toggle_at_arm);
    let kind_back = row_kind(page, toggle_idx).token().to_string();
    let now_saves = crate::settings::with(|s| s.saves);
    let landed = now_saves.saturating_sub(saves_at_arm);
    rep.row(
        "settings saves",
        Check::from(landed == 4 && back_toggle.ok() && kind_back == if toggle_at_arm == "true" { "on" } else { "off" }),
        format!(
            "落盘次数 {} -> {}，本档走了 {landed} 次（要的是 4：窗里保存 + 窗里开关 + 本档两笔还原；\
             开关还原 {}，还原后该行手型 {kind_back:?}；钉的落点 {}）",
            saves_at_arm,
            now_saves,
            back_toggle.text(),
            pin().display()
        ),
    );

    // ⑦ §5.21.1 第 2 与第 4 步那两枚钮在**屏上**的读数。窗换到计划第 13 段交回的那一页、读「那一块
    // 画没画 / 那枚入口画没画 / 两枚钮上此刻写的字」，再换回开工那一页。三件事在这一格分开判：
    // 开工那一页压根没有这一块、桥说该有的那一页真把这一块画了出来（页号由窗从计划里抄，所以对得上
    // 就证明那一枚类号是过桥递过去的、不是页上另数的一份）、以及「还没有地址」时那枚入口不画。
    // 按下去的那一只手不在这一格里 —— 它仍记在 `update_check` 那档的 Blocked。
    let updates: Vec<String> = reported("update");
    let u: Vec<Vec<&str>> = updates.iter().map(|l| l.split('|').collect()).collect();
    let about_idx = index_of(ABOUT_PAGE);
    let words_line = about_idx.map(update_words).unwrap_or_default();
    let words: Vec<&str> = words_line.split(RECORD).collect();
    let off_page = u.first().is_some_and(|f| {
        f.len() == 5
            && f[0] == page.to_string()
            && f[1] == "0"
            && f[2] == "0"
            && f[3].is_empty()
            && f[4].is_empty()
    });
    let on_page = u.get(1).is_some_and(|f| {
        f.len() == 5
            && about_idx.is_some_and(|i| f[0] == i.to_string())
            && f[1] == "1"
            && f[2] == "0"
            && f.get(3).copied() == words.first().copied()
            && f.get(4).copied() == words.get(1).copied()
    });
    rep.row(
        "settings update row drawn",
        Check::from(updates.len() == 2 && off_page && on_page),
        format!(
            "窗递回 update 行 {} 条（要 2：开工那一页 + 换过去那一页）；开工页 {page} 上没有这一块={off_page}；\
             「{}」那一页（类号 {} / 共 {} 页）上这一块画了、两枚钮上的字等于桥此刻说的字={on_page}；\
             桥交回的那两枚字={words:?}；原样 {:?}",
            updates.len(),
            ABOUT_PAGE,
            about_idx.map_or("?".to_string(), |i| i.to_string()),
            CATEGORIES.len(),
            updates,
        ),
    );

    // 关窗那一腿只有读数：它证明的是「QML 写得动 `settings_open` 那一格」，也就是产品里
    // 真人点关闭时那一只手真的通。它是 §5.20.2 第三条「离开设置页」那一半的前提，不是一判。
    rep.note(
        "settings window close",
        format!(
            "settings_open 读回 {:?}（窗自己写 0 之后再问一次桥）",
            reported("close")
                .first()
                .cloned()
                .unwrap_or_else(|| "(没递)".to_string())
        ),
    );

    rep.finish("settings-window door")
}

// ──────────────────────────────────────────────────────────────────────────────
// §5.20.3 的那两枚钮：导出配置 / 导入配置，与「将被覆盖的项目」那一格
// ──────────────────────────────────────────────────────────────────────────────
//
// 上面那一档量的是「页上的一行跟着值走不走」（§5.20.2），这一档量的是另一件事：**页能不能
// 把这一台机器的设置交出去，再拿回一份文件来决定改哪几枚**。§5.20.3 那五步里没有一步是
// 「按一枚钮改一个数」，所以计划那十四段在这里一段也用不上 —— 这一档有自己的计划、自己的
// 账、自己那六格，`window_measure` 一行也不参与（两档互相拒并列，见 `main.rs` 的 refusals）。
//
// 三只手全在 `settings.rs` 里已经存在过：`config_export` / `import_read` / `config_import`。
// 本模块加的只有**给界面的那一层行**（[`io_preview_line`]：总数一句、一项一行，用 `RECORD`
// 连着）和这一档的读数。措辞与算术都不重写（#281）：页上那一列行的每一个字都出自
// [`crate::settings::import_row_text`]，与 `--import-config` 打在控制台上的那一份同源。

/// 两枚钮各点一份文件。两枚都钉在 [`pin`] 那个目录里 —— 用户的 `%APPDATA%` 不是这一趟的草稿纸。
pub const IO_EXPORT_NAME: &str = "p143-export.toml";
pub const IO_IMPORT_NAME: &str = "p143-import.toml";

/// 建议范围**之内**的一枚键：`arm_io` 把导入那份档里的它改成另一个数，于是页上那一列行必须
/// 有它（没有，就是「预览那一手压根没跑」）。两个数都是 1..64 之间的合法值，门不会校正它 ——
/// 校正过的写会让「页上说的」与「盘上存的」各长一个样子，那一格本档不掺进来。
pub const PLANTED_KEY: &str = "annotation.stroke_width";
pub const PLANTED_CANDIDATES: [&str; 3] = ["5", "6", "7"];

/// 建议范围**之外**的一枚键：同一份档里也改了它，而页上不许出现它。这一格才是 PRD 那一列
/// 「范围建议」的对面 —— 只数「行里有几枚」的尺子把 `All` 也量成绿，所以这里数的是**两把尺之差**：
/// 同一份档在 `Suggested` 下不含它、在 `All` 下含它（#433：分母要非空，0 才是数出来的 0）。
pub const STRAY_KEY: &str = "history.max_items";
pub const STRAY_CANDIDATES: [&str; 3] = ["300", "400", "500"];

/// 这一档的地板：窗、两次过一次桥的文件动作、还原本档那一次写。比 `MIN_WINDOW_MS` 宽，
/// 因为这里每一步都碰盘。
pub const MIN_IO_MS: i32 = 3_000;

/// `--settings-io` 那一档在自己这一趟里看见过的东西。
#[derive(Debug)]
struct IoDoor {
    armed: bool,
    reason: String,
    plan: String,
    export_at: PathBuf,
    import_at: PathBuf,
    lines: Vec<String>,
    saves_at_arm: u32,
    /// 本机这两枚键在开工那一刻写着什么（还原要用**记住的那个数**，不是现在那份）。
    planted_from: String,
    planted_to: String,
    stray_from: String,
    stray_to: String,
    /// 导出那份档在 `arm` 那一刻有几枚键 —— 计划里没有这个数，它是本档这一侧的对。
    keys_at_arm: usize,
    /// 种完档那一刻，那份档「会盖掉哪几枚」—— 建议范围内与全部范围各一列**键名**。
    ///
    /// 为什么必须记在开档时而不是读回时再问一次：这一档的腿里有一次是真的 `apply_import`，
    /// 而 `preview_import` 对的是**内存里此刻的那一份**。等五条腿都跑完再来问，范围内那一枚
    /// 已经被应用成了档里带的数 —— 那一列行当场变成空，③④ 两格问的就不是「窗画对了没有」而
    /// 是「应用之后还剩什么可应用」。开档那一刻的数与窗在腿里看到的数是同一个时刻的数，所以对
    /// 得上；分母非空由种进去的那两枚键保证（#453：0 不能既是「没有」又是「问错了时刻」）。
    preview_at_arm: usize,
    names_at_arm: Vec<String>,
    all_names_at_arm: Vec<String>,
}

impl IoDoor {
    const fn none() -> Self {
        Self {
            armed: false,
            reason: String::new(),
            plan: String::new(),
            export_at: PathBuf::new(),
            import_at: PathBuf::new(),
            lines: Vec::new(),
            saves_at_arm: 0,
            planted_from: String::new(),
            planted_to: String::new(),
            stray_from: String::new(),
            stray_to: String::new(),
            keys_at_arm: 0,
            preview_at_arm: 0,
            names_at_arm: Vec::new(),
            all_names_at_arm: Vec::new(),
        }
    }
}

static IO: Mutex<IoDoor> = Mutex::new(IoDoor::none());

fn with_io<R>(f: impl FnOnce(&mut IoDoor) -> R) -> R {
    let mut held = IO.lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut held)
}

fn io_reported(prefix: &str) -> Vec<String> {
    with_io(|i| {
        i.lines
            .iter()
            .filter_map(|l| l.split_once('|'))
            .filter(|(k, _)| *k == prefix)
            .map(|(_, v)| v.to_string())
            .collect()
    })
}

/// 计划三段：`腿等待 ms | 导出去哪份档 | 导入读哪份档`。
///
/// 段与段之间是 `|` 而不是 `RECORD` —— 这一串与上面那一档的十四段同一族（[`plan_line`]），
/// 而窗那侧切计划用的也是 `splitPlan`。两枚路径里不可能出现 `|`：那是 Windows 文件名的禁字。
pub fn io_plan_line() -> String {
    with_io(|i| i.plan.clone())
}

/// 这一趟该不该有一扇设置窗。**两只手都要那一扇窗**：上面那一档的四条腿，和这一档的两枚钮。
///
/// 它们是同一扇窗的两个计划，`main.qml` 那一格 `settings_open` 只有一枚 —— 谁 armed 就造，
/// 造出来之后各看各的计划决定跑哪几条腿（两档互相拒并列，所以这一趟至多只有一副计划）。
pub fn io_wanted() -> bool {
    with_io(|i| i.armed)
}

/// 界面递回来的一行进的是哪一本账：两档的窗共用 `settingsGaugeReport` 那**一枚** invokable
/// （桥上的入口只许有一个，否则页上就得自己写「我是哪一档」那一层判断 —— 那是措辞不是读数），
/// 所以分派在 [`report`] 里做，依据是「这一趟哪一档 armed」。两档互相拒并列，因此至多一本。
fn io_takes_report() -> bool {
    with_io(|i| i.armed)
}

/// 开档时在册：算出两份文件的路径、把导入那一份**现在就种在盘上**、记住两枚键开工时的值。
///
/// 种档这一手为什么在 `arm` 而不是在腿里：§5.20.3 的步骤 3 是「用户在**其他设备**选择导入配
/// 置」，这一台机器上没有第二台，所以这一档必须交出一份**不是本页导出的**档才能问出「会被盖掉
/// 哪几枚」。那份档由活的 config 克隆出来、改两枚键、走 core 自己的 `export_text` 落盘 ——
/// 于是页上那一列行的对面是一份真档，而不是本模块手写的三行 toml。
///
/// 返回的那句是给读档人看的「这一趟动了谁」，不是判据。
pub fn arm_io() -> String {
    let dir = door_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return refuse_io(format!("{} 造不出来：{e}", dir.display()));
    }
    let mut export_at = dir.clone();
    export_at.push(IO_EXPORT_NAME);
    let mut import_at = dir.clone();
    import_at.push(IO_IMPORT_NAME);

    let live = crate::settings::with(|s| s.config.clone());
    let keys_at_arm = match live.flat() {
        Ok(f) => f.len(),
        Err(e) => return refuse_io(format!("活的 config 数不出键：{e}")),
    };
    let at = |key: &str| live.get_str(key).unwrap_or_default();
    let (planted_from, stray_from) = (at(PLANTED_KEY), at(STRAY_KEY));
    let (Some(planted_to), Some(stray_to)) = (
        pick(&PLANTED_CANDIDATES, &planted_from),
        pick(&STRAY_CANDIDATES, &stray_from),
    ) else {
        return refuse_io(format!(
            "{PLANTED_KEY}({planted_from:?}) 或 {STRAY_KEY}({stray_from:?}) 此刻已是候选之一：\
             没有一枚改得动的值，这一趟不开窗"
        ));
    };

    // 种那一份档：克隆 → 两枚键各过一次语法闸 → core 自己的导出。两枚都必须真改得动，
    // 拒了或存不下就不开窗 —— 拿一份「与本机一字不差」的档去问「会盖掉哪几枚」，那一格
    // 永远是 0 枚，是恒绿的尺（#433）。
    let mut incoming = live.clone();
    let (planted_ok, stray_ok) = (
        write_into(&mut incoming, PLANTED_KEY, planted_to),
        write_into(&mut incoming, STRAY_KEY, stray_to),
    );
    for (key, w, want) in [
        (PLANTED_KEY, &planted_ok, planted_to),
        (STRAY_KEY, &stray_ok, stray_to),
    ] {
        if w.refused.is_some() {
            return refuse_io(format!(
                "{key} 种不进那份档：{}",
                w.refused.as_deref().unwrap_or("")
            ));
        }
        // 门留下的值必须就是要打进去的那一枚：被校正过的话，本档手里「档里带的数」与
        // 「应用之后该看到的数」就分成两个数，那一格当不了对面的读数。
        if w.stored.as_deref() != Some(want) {
            return refuse_io(format!(
                "{key} 门留下的值({:?})与要打进去的({want:?})不同：这一趟拿它当不了对面的数",
                w.stored
            ));
        }
    }
    let text = match incoming.export_text() {
        Ok(t) => t,
        Err(e) => return refuse_io(format!("那份档导出不了：{e}")),
    };
    if let Err(e) = std::fs::write(&import_at, text.as_bytes()) {
        return refuse_io(format!("{} 写不下去：{e}", import_at.display()));
    }
    // 导出那一份先删掉：腿里那一手会真写它。留着上一趟的那份，「档在不在」这一格就先绿了。
    let _ = std::fs::remove_file(&export_at);

    // 分母在此刻量，不在读回时量（#453 的那一族：0 必须能说是哪一种 0）。
    //
    // 这一档的腿里有一次真的 `apply_import`，而 `preview_import` 比的是**内存里此刻那一份**
    // 与档里那一份。五条腿跑完再来问，范围内那一枚已经成了档里带的数 —— 于是「将被覆盖」
    // 当场空掉，③ 那格问的就不再是「窗画对了没有」。这里走的是页在腿里走的**同一只手**
    // （`import_read` 读盘上那份档、过一次语法闸、再对内存里那一份），所以量到的就是窗将要
    // 看到的；差别只有时刻。两枚键都在这两列里该在的位置上，否则这一趟没有分母，当场拒。
    let suggested = match crate::settings::import_read(&import_at, ImportScope::Suggested) {
        Ok(r) => r,
        Err(e) => return refuse_io(format!("种完档再读它，读不出：{e}")),
    };
    let all = match crate::settings::import_read(&import_at, ImportScope::All) {
        Ok(r) => r,
        Err(e) => return refuse_io(format!("同一份档按全部范围读，读不出：{e}")),
    };
    let names_at_arm: Vec<String> = suggested.changes.iter().map(|c| c.key.clone()).collect();
    let all_names_at_arm: Vec<String> = all.changes.iter().map(|c| c.key.clone()).collect();
    if !names_at_arm.iter().any(|k| k == PLANTED_KEY) {
        return refuse_io(format!(
            "{PLANTED_KEY} 改成了 {planted_to:?} 却没有出现在建议范围的「将被覆盖」里\
             （{names_at_arm:?}）：这一趟的预览那一格没有分母"
        ));
    }
    if !all_names_at_arm.iter().any(|k| k == STRAY_KEY) {
        return refuse_io(format!(
            "{STRAY_KEY} 改成了 {stray_to:?} 却没有出现在全部范围的「将被覆盖」里\
             （{all_names_at_arm:?}）：「建议范围不含它」就没有对的那一侧"
        ));
    }
    if names_at_arm.iter().any(|k| k == STRAY_KEY) {
        return refuse_io(format!(
            "{STRAY_KEY} 在建议范围里就出现了（{names_at_arm:?}）：范围那一格的两问当场同真，\
             这一趟的尺量不出范围"
        ));
    }

    let plan = format!("{}|{}|{}", LEG_MS, export_at.display(), import_at.display());
    let saves_at_arm = crate::settings::with(|s| s.saves);
    let preview_at_arm = names_at_arm.len();
    with_io(|i| {
        i.armed = true;
        i.reason = String::new();
        i.plan = plan.clone();
        i.export_at = export_at.clone();
        i.import_at = import_at.clone();
        i.lines.clear();
        i.saves_at_arm = saves_at_arm;
        i.planted_from = planted_from.clone();
        i.planted_to = planted_to.to_string();
        i.stray_from = stray_from.clone();
        i.stray_to = stray_to.to_string();
        i.keys_at_arm = keys_at_arm;
        i.preview_at_arm = preview_at_arm;
        i.names_at_arm = names_at_arm.clone();
        i.all_names_at_arm = all_names_at_arm.clone();
    });
    format!(
        "计划 {plan}；导出那份档要 {keys_at_arm} 枚键对上；导入那份档种了 {PLANTED_KEY}\
         {planted_from:?}->{planted_to:?}（建议范围内）与 {STRAY_KEY}{stray_from:?}->{stray_to:?}\
         （范围外）；开档时预览 {preview_at_arm} 行 {names_at_arm:?}、全部范围 {} 行 \
         （{STRAY_KEY} 只在后者）；落盘次数 {saves_at_arm}",
        all_names_at_arm.len()
    )
}

/// 不开窗的那一支：把原因存进本档，好让 `io_measure` 的 BLOCKED 那一格说得出是谁。
fn refuse_io(reason: String) -> String {
    with_io(|i| {
        i.armed = false;
        i.reason = reason.clone();
        i.plan = String::new();
    });
    reason
}

/// 界面上那两枚钮的第一只：导出。路径先过 [`crate::save::local_path`]（真人从 `FileDialog`
/// 递回的是 `file:///C:/…`，量具递的是裸路径 —— 那一枚函数两条都认，所以桥上不必分岔）。
///
/// 交回的是 [`crate::settings::config_export`] 那一整句，本档一个字也不改：页上写着「写出几枚
/// 键到哪份档」，那句话与 `--export-config` 打在控制台上的必须是同一句。
pub fn io_export(target: &str) -> String {
    let path = crate::save::local_path(target.trim());
    match crate::settings::config_export(Path::new(path.as_ref())) {
        Ok(line) => line,
        Err(e) => format!("[export] 失败：{e}"),
    }
}

/// 第二只钮的第一步（§5.20.3 步骤 4）：总数一句 + 一项一行，`RECORD` 连着交给界面。
///
/// 第一枚元素是 [`crate::settings::import_note`] 那句总数，其余是
/// [`crate::settings::import_row_text`] 那些行 —— 窗拿 `slice(1)` 画列表，把第 0 枚当抬头。
/// 读不出那份档时交回**一句**错误而没有行：空列表与「没得盖掉」在页上是两种坏，本档把它们
/// 分开印（同上面那一档把 `born` 与 `tick` 分开的那一个理由）。
pub fn io_preview_line(target: &str) -> String {
    let path = crate::save::local_path(target.trim());
    let read = match crate::settings::import_read(Path::new(path.as_ref()), ImportScope::Suggested)
    {
        Ok(r) => r,
        Err(e) => return format!("[import] 读不出那份档：{e}"),
    };
    let mut parts = vec![crate::settings::import_note(
        read.changes.len(),
        read.warnings.len(),
        ImportScope::Suggested,
    )];
    parts.extend(read.changes.iter().map(crate::settings::import_row_text));
    parts.join(RECORD)
}

/// 那一列行下面的一只手（§5.20.3 步骤 5）：确认之后才真写。
///
/// 走的是 `config_import(…, confirm = true)` —— 与 `--import-config --confirm` 同一只手，
/// 所以「页上点了确认」与「命令行加了 --confirm」之间不存在两套落盘码。
pub fn io_apply(target: &str) -> String {
    let path = crate::save::local_path(target.trim());
    match crate::settings::config_import(Path::new(path.as_ref()), ImportScope::Suggested, true) {
        Ok(line) => line,
        Err(e) => format!("[import] 失败：{e}"),
    }
}

/// `--settings-io` 的那扇门。六格，每格的对面都是**这一趟真开着的那扇窗**递回来的行与盘上
/// 那份真文件，本档手里那一份模型只用来对，不用来说。
pub fn io_measure(want_ms: i32) -> (crate::state::Check, String) {
    use crate::r13::Report;
    use crate::state::Check;

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (
        armed,
        reason,
        export_at,
        import_at,
        saves_at_arm,
        planted_from,
        planted_to,
        stray_from,
        stray_to,
        keys_at_arm,
    ) = with_io(|i| {
        (
            i.armed,
            i.reason.clone(),
            i.export_at.clone(),
            i.import_at.clone(),
            i.saves_at_arm,
            i.planted_from.clone(),
            i.planted_to.clone(),
            i.stray_from.clone(),
            i.stray_to.clone(),
            i.keys_at_arm,
        )
    });
    if !armed {
        rep.row(
            "settings io",
            Check::Blocked,
            format!("这一趟没有 {want_ms} ms 的计划：{reason}"),
        );
        return rep.finish("settings-io door");
    }
    // 开档那一刻的两列键名与那一个行数 —— ③④ 的分母（读回时再问一次问的是别人，见 `arm_io`）。
    let (preview_at_arm, names_at_arm, all_names_at_arm) = with_io(|i| {
        (
            i.preview_at_arm,
            i.names_at_arm.clone(),
            i.all_names_at_arm.clone(),
        )
    });

    let value_of = |key: &str| {
        crate::settings::with(|s| {
            s.config
                .get_str(key)
                .unwrap_or_else(|| "(读不出)".to_string())
        })
    };

    // ① 窗在不在。这一格是其余五格的前提：钮在页上、页在这一趟真的开过，才谈得上「用户点了
    //    导出配置」。`visible` 是窗自己问 Qt 的那一枚。
    let open: Vec<String> = io_reported("io-open");
    let of: Vec<&str> = open
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let open_ok = of.len() == 3 && of[1] == "1" && of[2].parse::<usize>() == Ok(CATEGORIES.len());
    rep.row(
        "settings io",
        Check::from(open_ok),
        format!(
            "窗递回 {} 行 io-open，最近一句 {:?}（类数 {} = 模型 {} 枚，可见 {}）；qml_loaded={}",
            open.len(),
            open.first(),
            of.get(2).copied().unwrap_or("?"),
            CATEGORIES.len(),
            of.get(1).copied().unwrap_or("?"),
            crate::state::qml_loaded()
        ),
    );

    // ② 步骤 1 与 2：页上那枚钮按下去，盘上多出一份**读得通**的档，且它的键数与活的 config 同一枚。
    //    这里不拿「[export] 那一行在不在」当读数：那句是措辞。判的是文件 —— 存在、非空、
    //    `parse_import` 读得通、flat 数得出 `keys_at_arm` 枚。
    let exports: Vec<String> = io_reported("export");
    let ef: Vec<&str> = exports
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let written_path = ef.first().copied().unwrap_or("");
    let export_named = export_at.display().to_string();
    let disk = std::fs::read(&export_at);
    let (bytes, back_keys) = match &disk {
        Ok(b) => {
            let text = String::from_utf8_lossy(b);
            // 读回来数一遍，用的不是本模块拼的那份文本：`parse_import` 交回的是「这份档
            // 自己说得通吗」与校正过的那一份 —— 于是「导出的是活的那一枚」这件事对面是盘。
            let parsed = Config::parse_import(&text)
                .ok()
                .and_then(|(c, _)| c.flat().ok().map(|f| f.len()));
            (b.len(), parsed)
        }
        Err(_) => (0, None),
    };
    let export_ok = exports.len() == 1
        && written_path == export_named
        && bytes > 0
        && back_keys == Some(keys_at_arm);
    rep.row(
        "settings io export",
        Check::from(export_ok),
        format!(
            "窗那侧 {:?}（一次）；盘上 {} = {bytes} B，读回来 {} 枚键，本档开工数的 {:?} -> {:?}",
            exports.first(),
            export_at.display(),
            back_keys.map_or("读不出".to_string(), |k| k.to_string()),
            keys_at_arm,
            if back_keys == Some(keys_at_arm) {
                "同"
            } else {
                "不同"
            }
        ),
    );

    // ③ 「将被覆盖的项目」那一格画了几行 vs 开档那一刻桥那侧数出几行，两个数并在一行里；
    //    读回时再问一次的第三个数**只印不判**。
    //
    //    为什么不拿读回时那一次当分母：这一档的腿里真的应用过一次，`preview_import` 比的是内存
    //    里此刻那一份，所以「现在还剩几行可应用」在这一格里注定比窗当时看到的少（gen A 实测
    //    窗画 1 行、现读 0 行 —— 窗没错，问的时刻错了）。开档时那一问走的是页在腿里的同一只手，
    //    与窗看到的是同一时刻，所以对得上；现读那一次留在行里当读数，好让读档人看得出两个时刻差在哪。
    let previews: Vec<String> = io_reported("preview");
    let pf: Vec<&str> = previews
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let drawn = pf.first().and_then(|v| v.parse::<usize>().ok());
    let read = crate::settings::import_read(&import_at, ImportScope::Suggested);
    let model_rows = read.as_ref().ok().map(|r| r.changes.len());
    let preview_ok = previews.len() == 1 && drawn == Some(preview_at_arm);
    rep.row(
        "settings io preview rows",
        Check::from(preview_ok),
        format!(
            "窗画了 {} 行；本档开档时数到 {preview_at_arm} 行 {names_at_arm:?} -> {:?}；\
             读回时再问一次得 {:?} 行（腿里已应用过，此刻内存与档已同，只作读数不作判据）；\
             窗那侧原样 {:?}（抬头：{:?}）",
            pf.first().copied().unwrap_or("?"),
            if drawn == Some(preview_at_arm) {
                "对"
            } else {
                "不对"
            },
            model_rows,
            previews.first(),
            io_preview_head(&import_at)
        ),
    );

    // ④ 范围那一格：种进去的两枚键，一枚在建议范围内、一枚在范围外。页上那一列行必须**只有
    //    前者**。后面的 `All` 那一问是分母：同一份档在 All 下含后者 ⇒ 「不含它」是范围这一枚
    //    判据的读数，而不是那份档坏掉（没有这一问，0 枚可以是任何一种坏）。
    //    三问都用开档时那两列名字，理由与 ③ 同一枚：读回时建议范围那一列可能已经空掉，那时
    //    「范围内含 PLANTED」会以「没得可含」的姿态绿不掉，而「范围外不含 STRAY」会**假绿** ——
    //    空列表里当然没有它。#433 的那一条正是这个：恒绿的尺当不了判据。
    let names: Vec<String> = read
        .as_ref()
        .map(|r| r.changes.iter().map(|c| c.key.clone()).collect())
        .unwrap_or_default();
    let all_names: Vec<String> = crate::settings::import_read(&import_at, ImportScope::All)
        .map(|r| r.changes.iter().map(|c| c.key.clone()).collect())
        .unwrap_or_default();
    let scope_ok = names_at_arm.iter().any(|k| k == PLANTED_KEY)
        && !names_at_arm.iter().any(|k| k == STRAY_KEY)
        && all_names_at_arm.iter().any(|k| k == STRAY_KEY);
    rep.row(
        "settings io scope",
        Check::from(scope_ok),
        format!(
            "开档时建议范围 {PLANTED_KEY}（范围内）在内 = {}、{STRAY_KEY}（范围外）在内 = {}；\
             同一份档按全部范围数到 {} 枚、含 {STRAY_KEY} = {}（档里带 {stray_to:?}）；\
             建议范围逐名 {:?}、全部范围逐名 {:?}；\
             读回时再数一遍：建议 {:?} / 全部 {:?}",
            names_at_arm.iter().any(|k| k == PLANTED_KEY),
            names_at_arm.iter().any(|k| k == STRAY_KEY),
            all_names_at_arm.len(),
            all_names_at_arm.iter().any(|k| k == STRAY_KEY),
            names_at_arm,
            all_names_at_arm,
            names,
            all_names
        ),
    );

    // ⑤ 步骤 5：确认之后，范围内那枚键真的成了那份档里的数，范围外那一枚**一次也没动**。
    //    之后本档把范围内那一枚改回开工时的数 —— 这一格判的是两件事：应用真的落了，而且
    //    这一趟没把用户的设置留在别处。落盘次数要的是 2（页上确认一次 + 本档还原一次）。
    let applies: Vec<String> = io_reported("apply");
    let af: Vec<&str> = applies
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let applied_path = af.first().copied().unwrap_or("");
    let import_named = import_at.display().to_string();
    let after_apply = value_of(PLANTED_KEY);
    let stray_after = value_of(STRAY_KEY);
    let back = write(PLANTED_KEY, &planted_from);
    let restored = value_of(PLANTED_KEY) == planted_from && back.refused.is_none();
    let now_saves = crate::settings::with(|s| s.saves);
    let landed = now_saves.saturating_sub(saves_at_arm);
    rep.row(
        "settings io apply",
        Check::from(
            applies.len() == 1
                && applied_path == import_named
                && after_apply == planted_to
                && stray_after == stray_from
                && restored
                && landed == 2,
        ),
        format!(
            "窗那侧 {:?}；{PLANTED_KEY} 开工 {:?} -> 应用后 {:?}（档里带的 {:?}）-> 还原后 {:?}；\
             {STRAY_KEY} 全程 {:?}（建议范围之外，一次也没动）；落盘次数 {} -> {}（要的是 2：\
             页上确认 + 本档还原，钉的落点 {}）",
            applies.first(),
            planted_from,
            after_apply,
            planted_to,
            value_of(PLANTED_KEY),
            stray_after,
            saves_at_arm,
            now_saves,
            pin().display()
        ),
    );

    // ⑥ 关窗：窗自己写 0 之后再问一次桥。与上面那一档同一格的问题、不同的一趟，所以它在这一档
    //    里也只是读数（判据写在 `window_measure` 那一份里，这里不重写）。
    rep.note(
        "settings io close",
        format!(
            "settings_open 读回 {:?}；本档递回 {} 行（导出 {:?} / 预览 {:?} / 应用 {:?}）",
            io_reported("io-close")
                .first()
                .cloned()
                .unwrap_or_else(|| "(没递)".to_string()),
            with_io(|i| i.lines.len()),
            exports.len(),
            previews.len(),
            applies.len()
        ),
    );

    rep.finish("settings-io door")
}

/// 那一列行的抬头句：本档自己现读那份档、现拼那句总数（窗画的是同一句，两处都出自
/// [`crate::settings::import_note`]，所以这一行只是把「桥那侧说的话」原样再印一次给读档的人）。
fn io_preview_head(path: &Path) -> String {
    match crate::settings::import_read(path, ImportScope::Suggested) {
        Ok(r) => {
            crate::settings::import_note(r.changes.len(), r.warnings.len(), ImportScope::Suggested)
        }
        Err(e) => format!("(读不出：{e})"),
    }
}

// ── §5.7.22 步骤 1/2/3 那一格 ──────────────────────────────────────────────

/// **调色板管理**。上面两档动的是「一行一档」的普通键，这一格动的是同一档里那两枚**名单**键：
/// 排本身，和存下来的方案。
///
/// 这一格存在的理由是一条很旧的账：`falcon_core::colors::Palette` 早就有 `add` / `remove` /
/// `move_to` / `contains`，也在单测里跑过，而产品里**一枚也没有用它**（P181 之前色带是页上写死的
/// 数组，P182 之后它只在命令行导出导入里露过一次脸）。页上这一格不自带第二套「怎么加、怎么去重、
/// 怎么挪」的规矩：四只手都调那一枚模型，所以「模型定的规矩」与「按下去发生的事」中间没有分岔。
///
/// 落盘也不另开一条路：每一只手算出的那一份名单，交回 [`write`] —— 同一道语法闸
/// （`Config::set_str` + `validate`）、同一次原子写（`Settings::save`）、同一句
/// [`Written::text`]。页上说的和档里存的因此只能是同一份。
pub const PAL_BAND_KEY: &str = "annotation.palette";
pub const PAL_SCHEME_KEY: &str = "annotation.palette_schemes";

/// 排里最少留一枚。`Config::validate` 对**空名单**的做法是回填 10 枚默认、而不记一句警告，所以
/// 「删到空」这一件事页上不许答应：答应了，落盘回来的会是别人那一排色，而页上写的还是「删掉了」。
pub const PAL_MIN: usize = 1;

/// 档里那一排原文 ↔ 模型那一排色。两个方向都只走 [`crate::settings`] 已有的那两枚（#281）：
/// 桥里再抄一份「怎么把一串字读成色」就会和导出那一路分家。
fn pal_band_of(stored: &[String]) -> falcon_core::colors::Palette {
    crate::settings::palette_from_stored(stored)
}

fn pal_stored_of(palette: &falcon_core::colors::Palette) -> Vec<String> {
    palette.to_text().lines().map(str::to_string).collect()
}

/// 步骤 2 的四只手，**纯的**：交回的是「这一枚键该写成什么」，坏处交回一句为什么不动。不落盘、
/// 不开窗，所以单测与 `--settings-palette` 那扇门问的是同一只手（与 [`write_into`] 同一族规矩）。
pub fn pal_add_list(stored: &[String], hex: &str) -> Result<Vec<String>, String> {
    let Some(c) = falcon_core::colors::parse(hex) else {
        return Err(format!("{hex} 读不成一枚色"));
    };
    let mut p = pal_band_of(stored);
    if !p.add(c) {
        return Err(format!(
            "{hex} 已经在这一排里（模型定的规矩：重复的只留头一枚）"
        ));
    }
    Ok(pal_stored_of(&p))
}

pub fn pal_remove_list(stored: &[String], at: usize) -> Result<Vec<String>, String> {
    let mut p = pal_band_of(stored);
    if at >= p.colors.len() {
        return Err(format!(
            "第 {} 枚不在这一排里（这一排 {} 枚）",
            at + 1,
            p.colors.len()
        ));
    }
    if p.colors.len() <= PAL_MIN {
        return Err(format!(
            "这一排只剩 {} 枚：档对空名单的做法是回填 10 枚默认而不记一句警告，所以「删掉最后那一枚」\
             这一件事页上不许答应",
            p.colors.len()
        ));
    }
    p.remove(at);
    Ok(pal_stored_of(&p))
}

pub fn pal_move_list(stored: &[String], at: usize, delta: i32) -> Result<Vec<String>, String> {
    let mut p = pal_band_of(stored);
    if at >= p.colors.len() {
        return Err(format!(
            "第 {} 枚不在这一排里（这一排 {} 枚）",
            at + 1,
            p.colors.len()
        ));
    }
    let to = at as i32 + delta;
    // 模型的 `move_to` 对越界的目标是**夹住**（并发编辑时不 panic），页上要的却是「有没有真的动」：
    // 排头那一枚按「上一格」应当说不，而不是夹回原地然后报一句改好了。
    if to < 0 {
        return Err("这一枚已经是排头，没有上一格".to_string());
    }
    if to as usize >= p.colors.len() {
        return Err(format!(
            "这一枚已经是排尾（第 {} 枚），没有下一格",
            p.colors.len()
        ));
    }
    if !p.move_to(at, to as usize) {
        return Err(format!(
            "第 {} 枚挪不动（这一排 {} 枚）",
            at + 1,
            p.colors.len()
        ));
    }
    Ok(pal_stored_of(&p))
}

pub fn pal_edit_list(stored: &[String], at: usize, hex: &str) -> Result<Vec<String>, String> {
    let mut p = pal_band_of(stored);
    if at >= p.colors.len() {
        return Err(format!(
            "第 {} 枚不在这一排里（这一排 {} 枚）",
            at + 1,
            p.colors.len()
        ));
    }
    let Some(c) = falcon_core::colors::parse(hex) else {
        return Err(format!("{hex} 读不成一枚色"));
    };
    if p.colors[at] == c {
        return Err(format!("{hex} 就是第 {} 枚此刻的色，没有改", at + 1));
    }
    if let Some(i) = p.colors.iter().position(|x| *x == c) {
        return Err(format!("{hex} 已经是第 {} 枚", i + 1));
    }
    p.colors[at] = c;
    Ok(pal_stored_of(&p))
}

/// 步骤 3 的三只手，同一形状。名字那一头的规矩住在 core
/// （[`falcon_core::config::scheme_join`]）：页拼一份、档读一份，两处各写一遍就会分家。
pub fn pal_scheme_save(
    schemes: &[String],
    name: &str,
    band: &[String],
) -> Result<Vec<String>, String> {
    let colors = pal_stored_of(&pal_band_of(band));
    if colors.is_empty() {
        return Err("这一排是空的，存不成方案".to_string());
    }
    let Some(entry) = falcon_core::config::scheme_join(name, &colors) else {
        return Err(format!(
            "{name:?} 存不成方案的名字：名字里不许有 = , | 或控制字符"
        ));
    };
    let want = name.trim();
    if schemes
        .iter()
        .filter_map(|e| falcon_core::config::scheme_split(e))
        .any(|(n, _)| n == want)
    {
        return Err(format!("{want} 已经是一枚方案的名字（要改就先删掉那一枚）"));
    }
    let mut out = schemes.to_vec();
    out.push(entry);
    Ok(out)
}

pub fn pal_scheme_colors(schemes: &[String], name: &str) -> Result<Vec<String>, String> {
    let want = name.trim();
    let Some((_, colors)) = schemes
        .iter()
        .filter_map(|e| falcon_core::config::scheme_split(e))
        .find(|(n, _)| *n == want)
    else {
        return Err(format!("没有名叫 {want} 的方案"));
    };
    let p = pal_band_of(&colors.iter().map(|c| c.to_string()).collect::<Vec<_>>());
    if p.colors.is_empty() {
        return Err(format!("{want} 那一枚方案里读不出一枚色"));
    }
    Ok(pal_stored_of(&p))
}

pub fn pal_scheme_drop(schemes: &[String], name: &str) -> Result<Vec<String>, String> {
    let want = name.trim();
    let out: Vec<String> = schemes
        .iter()
        .filter(|e| {
            falcon_core::config::scheme_split(e)
                .map(|(n, _)| n != want)
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    if out.len() == schemes.len() {
        return Err(format!("没有名叫 {want} 的方案"));
    }
    Ok(out)
}

/// 页上那一排的**现读**：一枚一行 `序号|色`，[`RECORD`] 连着。画的是模型那一份名单，不是页上抄
/// 的数组 —— 这一格与 P181 之前的色带区别就在这一句。
pub fn pal_band_rows(cfg: &Config) -> String {
    pal_stored_of(&pal_band_of(&cfg.annotation.palette))
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}|{c}", i + 1))
        .collect::<Vec<_>>()
        .join(RECORD)
}

/// 页上方案那一列的现读：一条一行 `名字|枚数|色 色 …`。
pub fn pal_scheme_rows(cfg: &Config) -> String {
    cfg.annotation
        .palette_schemes
        .iter()
        .filter_map(|e| falcon_core::config::scheme_split(e))
        .map(|(n, c)| format!("{n}|{}|{}", c.len(), c.join(" ")))
        .collect::<Vec<_>>()
        .join(RECORD)
}

fn pal_now() -> (Vec<String>, Vec<String>) {
    crate::settings::with(|s| {
        (
            s.config.annotation.palette.clone(),
            s.config.annotation.palette_schemes.clone(),
        )
    })
}

pub fn pal_band_now() -> String {
    crate::settings::with(|s| pal_band_rows(&s.config))
}

pub fn pal_scheme_now() -> String {
    crate::settings::with(|s| pal_scheme_rows(&s.config))
}

/// 拒了的写**不**过桥、不落盘，句子照 [`Written::text`] 的形状交回页上：页上不另写一句「失败」。
fn pal_refused(key: &str, asked: &str, why: String) -> Written {
    Written {
        key: key.to_string(),
        asked: asked.to_string(),
        stored: crate::settings::with(|s| s.config.get_str(key)),
        corrections: Vec::new(),
        restart: needs_restart(key),
        save_ms: 0,
        refused: Some(why),
        save_failed: None,
    }
}

/// 算出来的那一份名单交回唯一的一条写路。名单之间用 `, ` 连，是因为 `Config::set_str` 的数组分支
/// 按 `,` 切 —— 而色与方案名都不许带逗号（上面那些纯函数钉着一头，core 那道闸钉着另一头）。
fn pal_write_list(key: &str, list: &[String]) -> Written {
    write(key, &list.join(", "))
}

pub fn pal_add(hex: &str) -> Written {
    let (band, _) = pal_now();
    match pal_add_list(&band, hex) {
        Err(why) => pal_refused(PAL_BAND_KEY, hex, why),
        Ok(list) => pal_write_list(PAL_BAND_KEY, &list),
    }
}

pub fn pal_remove(at: i32) -> Written {
    let asked = format!("{at}");
    let (band, _) = pal_now();
    if at < 0 {
        return pal_refused(PAL_BAND_KEY, &asked, "第 0 枚之前没有色".to_string());
    }
    match pal_remove_list(&band, at as usize) {
        Err(why) => pal_refused(PAL_BAND_KEY, &asked, why),
        Ok(list) => pal_write_list(PAL_BAND_KEY, &list),
    }
}

pub fn pal_move(at: i32, delta: i32) -> Written {
    let asked = format!("{at}+{delta}");
    let (band, _) = pal_now();
    if at < 0 {
        return pal_refused(PAL_BAND_KEY, &asked, "第 0 枚之前没有色".to_string());
    }
    match pal_move_list(&band, at as usize, delta) {
        Err(why) => pal_refused(PAL_BAND_KEY, &asked, why),
        Ok(list) => pal_write_list(PAL_BAND_KEY, &list),
    }
}

pub fn pal_edit(at: i32, hex: &str) -> Written {
    let asked = format!("{at}={hex}");
    let (band, _) = pal_now();
    if at < 0 {
        return pal_refused(PAL_BAND_KEY, &asked, "第 0 枚之前没有色".to_string());
    }
    match pal_edit_list(&band, at as usize, hex) {
        Err(why) => pal_refused(PAL_BAND_KEY, &asked, why),
        Ok(list) => pal_write_list(PAL_BAND_KEY, &list),
    }
}

pub fn pal_save_scheme(name: &str) -> Written {
    let (band, schemes) = pal_now();
    match pal_scheme_save(&schemes, name, &band) {
        Err(why) => pal_refused(PAL_SCHEME_KEY, name, why),
        Ok(list) => pal_write_list(PAL_SCHEME_KEY, &list),
    }
}

/// 应用一枚方案：动的**是排**那一枚键，所以它的对面是 `--import-palette --confirm` 那一步
/// （§5.7.22 步骤 5「导入文件后应用相同配置」），只是这一份名单来自本机存下的方案而不是文件。
pub fn pal_apply_scheme(name: &str) -> Written {
    let (_, schemes) = pal_now();
    match pal_scheme_colors(&schemes, name) {
        Err(why) => pal_refused(PAL_BAND_KEY, name, why),
        Ok(list) => pal_write_list(PAL_BAND_KEY, &list),
    }
}

pub fn pal_drop_scheme(name: &str) -> Written {
    let (_, schemes) = pal_now();
    match pal_scheme_drop(&schemes, name) {
        Err(why) => pal_refused(PAL_SCHEME_KEY, name, why),
        Ok(list) => pal_write_list(PAL_SCHEME_KEY, &list),
    }
}

pub const PAL_ADD_WORD: &str = "新增一枚";
pub const PAL_DEL_WORD: &str = "删除";
pub const PAL_UP_WORD: &str = "上移";
pub const PAL_DOWN_WORD: &str = "下移";
pub const PAL_EDIT_WORD: &str = "改这枚";
pub const PAL_SAVE_WORD: &str = "保存为方案";
pub const PAL_APPLY_WORD: &str = "应用方案";
pub const PAL_DROP_WORD: &str = "删方案";
pub const PAL_TITLE: &str = "调色板管理";

/// 这一格上的字：**一处作者**。第 0 枚是抬头，后面八枚是钮。窗拿 [`RECORD`] 切开照着画，
/// 一枚也不自己写 —— 与 [`update_words`] 同一族理由（改一天措辞只需要动这里，页上抄第二份就会
/// 分家），而这一格比那一族多一枚，因为调色板要的手有八只。
pub fn pal_word_line() -> String {
    [
        PAL_TITLE,
        PAL_ADD_WORD,
        PAL_DEL_WORD,
        PAL_UP_WORD,
        PAL_DOWN_WORD,
        PAL_EDIT_WORD,
        PAL_SAVE_WORD,
        PAL_APPLY_WORD,
        PAL_DROP_WORD,
    ]
    .join(RECORD)
}

// ── `--settings-palette` 那一档 ────────────────────────────────────────────

/// 这一档的地板：窗 + 六次真按 + 还原两次。每一按都碰盘，比 `MIN_IO_MS` 只宽不窄。
pub const MIN_PAL_MS: i32 = 5_000;

/// 门要按的色从这一池里挑，挑的是**开工那一刻不在排里**的那一枚（[`pick`] 同一形状）。写死一池
/// 而不是随机：门的读数要能被下一趟复算，而随机的那一排色明天就不是今天这一排。
pub const PAL_HEX_POOL: [&str; 6] = [
    "#123456", "#ABCDEF", "#0F1E2D", "#D51E2C", "#6B4FBB", "#17A398",
];

/// 门种下去的那一枚方案的名字与它那三枚色。名字里不许有 `= , |` 与控制字符（core 那道钉），
/// 而那三枚色**与开工时那一排不同**是本档能问出「应用真的落了」的唯一条件。
pub const PAL_PLANT_NAME: &str = "门种的那一枚";
pub const PAL_PLANT_COLORS: [&str; 3] = ["#FF0000", "#00FF00", "#0000FF"];

/// 「保存为方案」那枚名字框用的名字：从这一池里挑一枚此刻还不是方案名的。
pub const PAL_NAME_POOL: [&str; 4] = ["门试甲", "门试乙", "门试丙", "门试丁"];

/// `--settings-palette` 在自己这一趟里看见过的东西。
#[derive(Debug)]
struct PalDoor {
    armed: bool,
    reason: String,
    plan: String,
    lines: Vec<String>,
    page: usize,
    /// 开工那一刻档里那两枚名单**原文**（还原要的是这两串，不是现在那两份）。
    band_at_arm: Vec<String>,
    schemes_at_arm: Vec<String>,
    /// 开工那一刻模型数出来的枚数 —— 计划里那三枚行号是从它算的，读档的人要能复算。
    band_count: usize,
    add_hex: String,
    del_at: usize,
    move_at: usize,
    move_delta: i32,
    edit_at: usize,
    edit_hex: String,
    save_name: String,
    saves_at_arm: u32,
}

impl PalDoor {
    const fn none() -> Self {
        Self {
            armed: false,
            reason: String::new(),
            plan: String::new(),
            lines: Vec::new(),
            page: 0,
            band_at_arm: Vec::new(),
            schemes_at_arm: Vec::new(),
            band_count: 0,
            add_hex: String::new(),
            del_at: 0,
            move_at: 0,
            move_delta: 0,
            edit_at: 0,
            edit_hex: String::new(),
            save_name: String::new(),
            saves_at_arm: 0,
        }
    }
}

static PAL: Mutex<PalDoor> = Mutex::new(PalDoor::none());

fn with_pal<R>(f: impl FnOnce(&mut PalDoor) -> R) -> R {
    let mut held = PAL.lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut held)
}

fn pal_reported(prefix: &str) -> Vec<String> {
    with_pal(|p| {
        p.lines
            .iter()
            .filter_map(|l| l.split_once('|'))
            .filter(|(k, _)| *k == prefix)
            .map(|(_, v)| v.to_string())
            .collect()
    })
}

/// 计划十段：`腿等待 ms | 开在哪一类 | 新增那枚色 | 删的行号 | 移的行号 | 移的方向 | 改色的行号 |
/// 改成的色 | 保存用的方案名 | 应用哪一枚方案`。
///
/// 行号全是 **0 起**的，与 [`crate::settings_view::arm`] 那四枚行号同一个口径（页上
/// `settingsRowAt(which, i)` 收的也是这个），而段里的色与名字是门自己从上面那三池里挑的 —— 窗
/// 一处也不猜「按下去该写什么」，指错一枚色去按新增是比不按更坏的那一种。
pub fn pal_plan_line() -> String {
    with_pal(|p| p.plan.clone())
}

/// 这一趟该不该有一扇设置窗：第三副计划，同一扇窗。
pub fn pal_wanted() -> bool {
    with_pal(|p| p.armed)
}

/// 三档共用桥上那一枚 `settingsGaugeReport`，所以分派在这里做，依据是「这一趟哪一档 armed」；
/// 三档互相拒并列（`main.rs` 的 refusals），因此至多一本。
fn pal_takes_report() -> bool {
    with_pal(|p| p.armed)
}

fn refuse_pal(reason: String) -> String {
    with_pal(|p| {
        p.armed = false;
        p.reason = reason.clone();
        p.plan = String::new();
    });
    reason
}

/// 开档时在册：数一遍开工那一排，挑出色、行号与名字，把要应用的那一枚方案**现在就种进档里**，
/// 再拼出计划。返回的那句是给读档人看的「这一趟动了谁」，不是判据。
///
/// 种方案必须写在 `engine.load()` **之前**（窗只在 `Component.onCompleted` 读一次计划），而它走的是
/// [`write`] —— 门不另开一条落盘路。应用那一腿的对面因此是一份真档里存着的方案，而不是本模块
/// 手写的串。
pub fn arm_pal() -> String {
    let page = index_of(ANNOT_PAGE).unwrap_or(CATEGORIES.len());
    let (band_at_arm, schemes_at_arm) = pal_now();
    let model = pal_band_of(&band_at_arm);
    let count = model.colors.len();
    let saves_at_arm = crate::settings::with(|s| s.saves);
    if count < 1 {
        return refuse_pal(format!(
            "档里那一排此刻是空的（{} 枚），这一档没有一行可以按",
            band_at_arm.len()
        ));
    }
    // 新增与改色各挑一枚「此刻不在排里」的色：已经在排里的那一枚会被模型的规矩挡下，那一腿就
    // 只能报一句拒 —— 而本档要问的是按下去**动没动**。
    let Some(add_hex) = PAL_HEX_POOL.into_iter().find(|h| {
        falcon_core::colors::parse(h)
            .map(|c| !model.contains(&c))
            .unwrap_or(false)
    }) else {
        return refuse_pal(format!(
            "{} 里没有一枚色不在这一排中",
            PAL_HEX_POOL.join(" ")
        ));
    };
    let model_strs = pal_stored_of(&model);
    let Some(edit_hex) = PAL_HEX_POOL
        .into_iter()
        .filter(|h| *h != add_hex)
        .find(|h| {
            falcon_core::colors::parse(h)
                .map(|c| !model_strs.iter().any(|s| s == h) && !model.contains(&c))
                .unwrap_or(false)
        })
    else {
        return refuse_pal("没有第二枚不在这一排中的色可以改".to_string());
    };
    let Some(save_name) = PAL_NAME_POOL.into_iter().map(str::trim).find(|n| {
        !schemes_at_arm
            .iter()
            .filter_map(|e| falcon_core::config::scheme_split(e))
            .any(|(existing, _)| existing == *n)
    }) else {
        return refuse_pal(format!("{} 全都已经是方案名", PAL_NAME_POOL.join(" ")));
    };

    // 计划里那三枚行号是从「按下去之后那一排长什么样」算的，不是从开工那一刻的排：新增落在**尾**，
    // 所以挪的是第 count 枚（0 起）往上一格，删的正是挪完停在那一格的那一枚。三步走完，排回到
    // 开工时的长度 —— 这一句是下面「还原」那一格对得上的前提。
    let (move_at, move_delta) = (count, -1);
    let del_at = count - 1;
    let edit_at = 0;

    // 种下要应用的那一枚方案。它与开工那一排**必须不同**，否则应用那一腿读不出动静。
    let planted: Vec<String> = PAL_PLANT_COLORS.iter().map(|s| s.to_string()).collect();
    if pal_band_of(&planted) == model {
        return refuse_pal("门种的那三枚色与档里那一排一模一样，应用那一腿问不出动静".to_string());
    }
    let list = match pal_scheme_save(&schemes_at_arm, PAL_PLANT_NAME, &planted) {
        Ok(l) => l,
        Err(why) => return refuse_pal(why),
    };
    let planted_at = pal_write_list(PAL_SCHEME_KEY, &list);
    if planted_at.refused.is_some() || planted_at.save_failed.is_some() {
        return refuse_pal(format!(
            "种不进档：{}",
            planted_at.refused.clone().unwrap_or_default()
        ));
    }

    let plan = format!(
        "{LEG_MS}|{page}|{add_hex}|{del_at}|{move_at}|{move_delta}|{edit_at}|{edit_hex}|{save_name}|{PAL_PLANT_NAME}"
    );
    with_pal(|p| {
        p.armed = true;
        p.reason = String::new();
        p.page = page;
        p.plan = plan.clone();
        p.band_at_arm = band_at_arm.clone();
        p.schemes_at_arm = schemes_at_arm.clone();
        p.band_count = count;
        p.add_hex = add_hex.to_string();
        p.del_at = del_at;
        p.move_at = move_at;
        p.move_delta = move_delta;
        p.edit_at = edit_at;
        p.edit_hex = edit_hex.to_string();
        p.save_name = save_name.to_string();
        p.saves_at_arm = saves_at_arm;
    });
    format!(
        "计划 {plan}；开在 {ANNOT_PAGE}#{page}；开工时那一排 {count} 枚（原文 {} 枚）、方案 {} 条；\
         新增 {:?} 改 {:?} 存名为 {:?} 应用 {:?}；落盘次数 {saves_at_arm}（种方案那一次在它之后）",
        band_at_arm.len(),
        schemes_at_arm.len(),
        add_hex,
        edit_hex,
        save_name,
        PAL_PLANT_NAME
    )
}

/// `--settings-palette` 的那扇门。八格，每格对面是**这一趟真开着的那扇窗**递回来的行，加本档
/// 自己现读的档。
pub fn pal_measure(want_ms: i32) -> (crate::state::Check, String) {
    use crate::r13::Report;
    use crate::state::Check;

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (
        armed,
        reason,
        page,
        band_at_arm,
        schemes_at_arm,
        count,
        add_hex,
        del_at,
        move_at,
        move_delta,
        edit_at,
        edit_hex,
        save_name,
        saves_at_arm,
    ) = with_pal(|p| {
        (
            p.armed,
            p.reason.clone(),
            p.page,
            p.band_at_arm.clone(),
            p.schemes_at_arm.clone(),
            p.band_count,
            p.add_hex.clone(),
            p.del_at,
            p.move_at,
            p.move_delta,
            p.edit_at,
            p.edit_hex.clone(),
            p.save_name.clone(),
            p.saves_at_arm,
        )
    });
    if !armed {
        rep.row(
            "settings palette",
            Check::Blocked,
            format!("这一趟没有 {want_ms} ms 的计划：{reason}"),
        );
        return rep.finish("settings-palette door");
    }

    // ① 窗在不在、开在哪一类、那一格画了几行。**画**的行数只由窗数得出来：Rust 手里那一串
    // `pal_band_rows` 是模型说的话，窗说的是屏幕上的话。
    let open: Vec<String> = pal_reported("pal-open");
    let want_band = pal_stored_of(&pal_band_of(&band_at_arm)).len();
    // 种下去的那一枚方案**在窗出生之前**就进了档（`arm_pal` 走的是唯一那条写路），所以页上开工
    // 那一刻画出的方案行是「开工时读到的 + 种的那一枚」。少加这一枚，`pal-open` 与 `palette save`
    // 两格判的都是「页上比一个没有对面读数的数多一行」。
    let want_scheme = schemes_at_arm.len() + 1;
    let of: Vec<&str> = open
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let open_ok = of.len() == 5
        && of[0].parse::<usize>() == Ok(page)
        && of[1] == "1"
        && of[2].parse::<usize>() == Ok(CATEGORIES.len())
        && of[3].parse::<usize>() == Ok(want_band)
        && of[4].parse::<usize>() == Ok(want_scheme);
    rep.row(
        "settings palette",
        Check::from(open_ok),
        format!(
            "页={} 可见={} 类={} 排={}行 方案={}行；模型说开工那趟该是 排={want_band} 方案={want_scheme}\
             （开工时模型数出 {count} 枚、原文 {} 枚）；窗一共递回 {} 行",
            of.first().copied().unwrap_or("?"),
            of.get(1).copied().unwrap_or("?"),
            of.get(2).copied().unwrap_or("?"),
            of.get(3).copied().unwrap_or("?"),
            of.get(4).copied().unwrap_or("?"),
            band_at_arm.len(),
            with_pal(|p| p.lines.len())
        ),
    );

    // ② 步骤 2 那三按（新增 / 挪 / 删）：窗报的是**它自己画的那几行的行数**，本档判的是行数之间
    //    那三种差 —— 加一枚多一行、挪一枚行数不变、删一枚少一行。少了「行数不变」这一格，
    //    「挪」与「什么都没按」在屏幕上长得一样。
    let legs = [("add", 1i32), ("move", 0), ("del", -1)];
    for (kind, step) in legs {
        let got: Vec<String> = pal_reported(kind);
        let gf: Vec<&str> = got
            .first()
            .map(|l| l.split('|').collect())
            .unwrap_or_default();
        let (before, after) = (
            gf.first().and_then(|v| v.parse::<i32>().ok()),
            gf.get(1).and_then(|v| v.parse::<i32>().ok()),
        );
        rep.row(
            &format!("palette {kind}"),
            Check::from(
                got.len() == 1 && before.zip(after).map(|(b, a)| a - b == step) == Some(true),
            ),
            format!(
                "窗那侧 {:?}；这一按要的是行数差 {step}（{}）",
                got.first(),
                if kind == "move" {
                    "挪一枚不该多一枚也不该少一枚"
                } else {
                    "加与删各是一枚"
                }
            ),
        );
    }

    // ③ 改色：按之前与按之后**那一行**的文本。这里判的是窗报回来的那两串确实不同，而且改完那一行
    //    写的就是计划里那枚色 —— 只有行数对上的「改」不算改（它可能是删了再加）。
    let edit: Vec<String> = pal_reported("edit");
    let ef: Vec<&str> = edit
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    rep.row(
        "palette edit",
        Check::from(
            ef.len() == 4
                && ef[0] == edit_at.to_string()
                && ef[1] == edit_hex
                && ef[2] != ef[3]
                && ef[3] == edit_hex,
        ),
        format!(
            "窗那侧 {:?}；计划里改的是第 {edit_at} 枚（0 起）成 {edit_hex}，报回的是按之前那枚色 {:?}",
            edit.first(),
            ef.get(2).copied()
        ),
    );

    // ④⑤ 步骤 3：保存为方案、应用一枚方案。保存那一格数的是方案那一列**多了一行**，应用那一格
    //    数的是排真的长成了门种的那三枚 —— 这一趟里唯一一处「按之前不是这个样、按之后是这个样」。
    let save: Vec<String> = pal_reported("save");
    let sf: Vec<&str> = save
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let applied: Vec<String> = pal_reported("apply");
    let af: Vec<&str> = applied
        .first()
        .map(|l| l.split('|').collect())
        .unwrap_or_default();
    let mut planted_list = schemes_at_arm.clone();
    planted_list.push(
        falcon_core::config::scheme_join(
            PAL_PLANT_NAME,
            &PAL_PLANT_COLORS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default(),
    );
    rep.row(
        "palette save",
        Check::from(
            sf.len() == 3
                && sf[0] == save_name
                && sf[1].parse::<i32>() == Ok(want_scheme as i32)
                && sf[2].parse::<i32>() == Ok(want_scheme as i32 + 1),
        ),
        format!(
            "窗那侧 {:?}；开工时方案 {} 条，按下去该是 {} 条（名字 {:?}）",
            save.first(),
            want_scheme,
            want_scheme + 1,
            save_name
        ),
    );
    let applied_band = pal_scheme_colors(&planted_list, PAL_PLANT_NAME).unwrap_or_default();
    let want_band_after = pal_band_flat_of(&applied_band);
    // 「按之前」那一串**不是**开工那一排：应用那一腿排在四按之后，页上此刻那一排已经被加、挪、删、
    // 改过。本档不重新读屏幕，也不猜——它用页上那四只手同一批**纯函数**把这一趟点名的四次按下
    // 复算一遍（`pal_*_list` 就是桥那四枚 invokable 里面调的那一枚）。复算不出来（任何一手该被
    // 拒）就交回空串，那一格当场判红而不是放行。
    let want_band_before = pal_replay(
        &band_at_arm,
        &add_hex,
        move_at,
        move_delta,
        del_at,
        edit_at,
        &edit_hex,
    )
    .unwrap_or_default();
    rep.row(
        "palette apply",
        Check::from(
            af.len() == 3
                && af[0] == PAL_PLANT_NAME
                && af[1] == want_band_before
                && af[2] == want_band_after,
        ),
        format!(
            "窗那侧 {:?}；门种的那一枚方案该把排从 {want_band_before} 改成 {want_band_after}\
             （按之前不是这一串才算应用到了）",
            applied.first()
        ),
    );

    // ⑥ 盘上此刻真的带着这一串 —— 页上说的与文件里存的是两件事，这一格问后一件。读的是**本档自己
    //    钉的那份文件**，不是 `%APPDATA%` 里用户那一枚（§9.1 第十五条）。
    let pinned = pin();
    let disk_band = disk_value(&pinned, "annotation", "palette").unwrap_or_default();
    let disk_schemes = disk_value(&pinned, "annotation", "palette_schemes").unwrap_or_default();
    rep.row(
        "palette on disk",
        Check::from(
            disk_band.contains(PAL_PLANT_COLORS[0])
                && disk_schemes.contains(PAL_PLANT_NAME)
                && disk_schemes.contains(&save_name),
        ),
        format!(
            "钉的落点 {}：`palette` 那一行 {:?}；`palette_schemes` 那一行 {:?}",
            pinned.display(),
            disk_band,
            disk_schemes
        ),
    );

    // ⑦ 还原：本档把两枚名单改回开工时的原文，改完再问一次桥。落盘次数要的是 9（种 1 + 按 6 +
    //    还原 2 —— 排与方案各一次）。
    let back_band = pal_write_list(PAL_BAND_KEY, &band_at_arm);
    let back_schemes = pal_write_list(PAL_SCHEME_KEY, &schemes_at_arm);
    let now = pal_now();
    let now_saves = crate::settings::with(|s| s.saves);
    let landed = now_saves.saturating_sub(saves_at_arm);
    rep.row(
        "palette restored",
        Check::from(
            back_band.refused.is_none()
                && back_schemes.refused.is_none()
                && now.0 == band_at_arm
                && now.1 == schemes_at_arm
                && landed == 9,
        ),
        format!(
            "排：开工 {:?} -> 此刻 {:?}；方案：开工 {} 条 -> 此刻 {} 条；落盘次数 {} -> {}（要的是 9：\
             种方案 1 + 六按 6 + 还原 2，钉的落点 {}）",
            band_at_arm,
            now.0,
            schemes_at_arm.len(),
            now.1.len(),
            saves_at_arm,
            now_saves,
            pinned.display()
        ),
    );

    // ⑧ 逐页那一趟里，「标注」那一页此刻有几行 —— 本档多出来的那一枚键（`palette_schemes`）在这一格里
    //    露一次脸：页上的行数跟着 `flat()` 走，不是窗自己数别的什么。
    let page_rows: Vec<String> = pal_reported("pal-page");
    let want_page = rows(page).len();
    rep.row(
        "palette page rows",
        Check::from(
            page_rows.len() == 1
                && page_rows[0]
                    .split('|')
                    .nth(1)
                    .and_then(|v| v.parse::<usize>().ok())
                    == Some(want_page),
        ),
        format!(
            "窗报回 {:?}；模型说 {ANNOT_PAGE}#{page} 该有 {want_page} 行",
            page_rows
        ),
    );

    rep.note(
        "palette close",
        format!(
            "settings_open 读回 {:?}；本档递回 {} 行",
            pal_reported("pal-close")
                .first()
                .cloned()
                .unwrap_or_else(|| "(没递)".to_string()),
            with_pal(|p| p.lines.len())
        ),
    );

    rep.finish("settings-palette door")
}

/// 两枚「按名字算那一排该长什么样」的小手。判据要的对面是 core 那一份语法，不是本模块自己再拼
/// 一遍串；而报回的那一行是按 `|` 切的，所以这里交回的是**色与色之间一个空格**的那一串 ——
/// [`pal_band_rows`] 里那枚 `序号|色` 的分隔字会撑爆报回的段。
fn pal_band_flat_of(list: &[String]) -> String {
    pal_stored_of(&pal_band_of(list)).join(" ")
}

/// 把这一趟门点名的四次按下（加、挪、删、改）按页上那条腿的次序复算一遍，交回**应用那一腿按之前**
/// 该长着的那一串。
///
/// 走的是桥那四枚 invokable 里面调的同一批纯函数，所以这里没有第二份「怎么加一枚色」；任何一手
/// 在该被拒的时候（越界、重复、读不出色）这一手答 `None`，让对面那一格判红而不是拿旧值蒙过去。
fn pal_replay(
    band: &[String],
    add_hex: &str,
    move_at: usize,
    move_delta: i32,
    del_at: usize,
    edit_at: usize,
    edit_hex: &str,
) -> Option<String> {
    let added = pal_add_list(band, add_hex).ok()?;
    let moved = pal_move_list(&added, move_at, move_delta).ok()?;
    let deleted = pal_remove_list(&moved, del_at).ok()?;
    let edited = pal_edit_list(&deleted, edit_at, edit_hex).ok()?;
    Some(pal_band_flat_of(&edited))
}

pub fn pal_band_flat(cfg: &Config) -> String {
    pal_band_flat_of(&cfg.annotation.palette)
}

pub fn pal_band_flat_now() -> String {
    crate::settings::with(|s| pal_band_flat(&s.config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Check;

    /// 三档门（窗 / io / 调色板）在**同一进程里**共用的那一把排他锁。理由不是「稳妥」，是这两件
    /// 事实：① `pin()` 钉的落点是**一个路径**，两档各自往它写整份档，后写的那一份就是前一份的
    /// 读数（P183 第一次把两档放进同一趟时，io 那一档 import 的那一份当场把门种的方案抹成空
    /// 串）；② `crate::settings` 里那枚 `saves` 是全进程一个计数器，而「开这一档多落了几次盘」
    /// 正是这两档的判据分母。一把锁把两档排开，比给每一档配一份草稿纸少一处权威（#281）——
    /// 钉的落点是产品那一头 `main.rs` 也走的一个名字，不该为量具再造第二个。
    static DOOR_SERIAL: Mutex<()> = Mutex::new(());

    fn door_serial() -> std::sync::MutexGuard<'static, ()> {
        DOOR_SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn every_default_key_belongs_to_exactly_one_category() {
        let cfg = Config::default();
        let keys = cfg.flat().unwrap();
        assert!(
            keys.len() > 50,
            "这份清单只有 {} 枚键，分类判据可能什么都没读到",
            keys.len()
        );
        for k in keys.keys() {
            assert_eq!(owners(k).len(), 1, "{k} 归了 {:?} 类", owners(k));
        }
    }

    /// §5.21.1 第 2/4 步那块页脚的唯一一问：只有「关于与更新」那一页递两枚字，其余每一页递空串。
    /// 三件事在这一格分开红：类的**号**不是数出来的（`ABOUT_PAGE` 反查自己）、两枚字不是这里抄的
    /// （等的是 core 那两枚常量）、越界与负数不许变成「递了点什么」。
    #[test]
    fn only_the_about_page_offers_the_update_buttons() {
        let about = index_of(ABOUT_PAGE).expect("名单里必须有「关于与更新」");
        assert_eq!(
            update_words(about),
            format!(
                "{}{RECORD}{}",
                falcon_core::update::TRAY_WORD,
                falcon_core::update::DOWNLOAD_WORD
            )
        );
        assert_eq!(update_words(about).split(RECORD).count(), 2);
        for i in 0..CATEGORIES.len() {
            if i != about {
                assert_eq!(update_words(i), "", "第 {i} 类递了更新那两枚钮");
            }
        }
        assert_eq!(update_words(usize::MAX), "");
    }

    /// 名单的下限是文档的：§5.20.1 那十一类逐字、逐序一枚不许动；第十二类「托盘」是 §5.18.2
    /// 第一步（PRD 第 1,448 行）要的那一格，理由写在 [`CATEGORIES`] 头顶。这一格问两件事：
    /// 把「托盘」那一枚抽掉之后，剩下的串与文档那一列**逐字相同**（下限名单没被偷偷换掉），
    /// 以及每一枚都反查得到自己的类号（页顶那排标签与门里那一格问的是同一个次序）。
    #[test]
    fn the_prd_roster_is_in_order_and_the_tray_page_is_the_only_addition() {
        let names: Vec<&str> = CATEGORIES.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec![
                "常规",
                "截图",
                "标注",
                "贴图",
                "输出",
                "快捷键",
                "托盘",
                "热区",
                "外观",
                "历史记录",
                "高级与命令行",
                "关于与更新",
            ]
        );
        let floor: Vec<&str> = names.iter().filter(|n| **n != "托盘").copied().collect();
        assert_eq!(
            floor,
            vec![
                "常规",
                "截图",
                "标注",
                "贴图",
                "输出",
                "快捷键",
                "热区",
                "外观",
                "历史记录",
                "高级与命令行",
                "关于与更新",
            ],
            "§5.20.1 那一份下限名单被改动了"
        );
        assert_eq!(floor.len(), 11, "下限名单不止十一枚了");
        assert_eq!(
            names.iter().filter(|n| **n == "托盘").count(),
            1,
            "「托盘」不止一格"
        );
        assert_eq!(index_of("托盘"), Some(TRAY_PAGE), "托盘不在它自己的类号上");
        assert_eq!(index_of("热区"), Some(CORNER), "热区不在它自己的类号上");
        for (i, name) in names.iter().enumerate() {
            assert_eq!(index_of(name), Some(i), "{name} 反查不到自己的类号");
        }
        assert_eq!(index_of("设置"), None);
    }

    #[test]
    fn no_category_is_an_empty_entry_on_the_page() {
        let cfg = Config::default();
        for (i, (name, _)) in CATEGORIES.iter().enumerate() {
            let rows = rows_of(&cfg, i);
            assert!(!rows.is_empty(), "{name} 一类没有一行：页上点不开的空入口");
        }
        assert!(!rows_of(&cfg, CORNER).is_empty(), "热区那一类是空的");
        assert_eq!(rows_of(&cfg, CATEGORIES.len()).len(), 0);
    }

    /// 「需重启」标记只许来自 [`RESTART_KEYS`]。这一条同时盯着两件事：页上标的枚数，和热区
    /// 那一类一枚也不标 —— 后者是 §5.20.2 第一条在热区这一页上的对面（采样腿每秒重读配置）。
    #[test]
    fn the_restart_marks_are_the_restart_list_and_nothing_else() {
        let cfg = Config::default();
        let mut marked: Vec<String> = (0..CATEGORIES.len())
            .flat_map(|i| rows_of(&cfg, i))
            .filter(|r| r.restart)
            .map(|r| r.key)
            .collect();
        marked.sort();
        let mut want: Vec<String> = RESTART_KEYS.iter().map(|k| k.to_string()).collect();
        want.sort();
        assert_eq!(marked, want);
        assert!(
            rows_of(&cfg, CORNER)
                .iter()
                .all(|r| !r.restart && !r.text().contains("需重启")),
            "热区那一类里有行被标成需重启"
        );
        assert!(needs_restart("general.language"));
        assert!(!needs_restart("hot_corner.dwell_ms"));
        assert!(!needs_restart(""));
    }

    #[test]
    fn a_write_reports_what_the_gate_made_of_it() {
        let mut cfg = Config::default();
        let over = write_into(&mut cfg, "hot_corner.dwell_ms", "9000");
        assert_eq!(over.stored.as_deref(), Some("5000"));
        assert_eq!(over.corrections.len(), 1, "{:?}", over.corrections);
        assert!(over.refused.is_none());
        assert!(!over.restart);
        assert!(over.text().contains("5000"));

        let in_range = write_into(&mut cfg, "hot_corner.dwell_ms", "900");
        assert_eq!(in_range.stored.as_deref(), Some("900"));
        assert!(in_range.corrections.is_empty());
        assert!(in_range.refused.is_none());
    }

    /// 拒了的写一行都不许动：值不变、落盘次数由调用方看着，而这一枚函数不能把「没写进去」
    /// 印成「写成了」。缺席的一枚键与值为空串的一枚键也不能混成同一个 `None`。
    #[test]
    fn a_refused_write_leaves_the_value_where_it_was() {
        let mut cfg = Config::default();
        let before = cfg.get_str("hot_corner.corners.top_left");
        let bogus = write_into(&mut cfg, "hot_corner.corners.top_left", "nope");
        assert!(bogus.refused.is_some(), "{:?}", bogus.stored);
        assert!(!bogus.ok());
        assert_eq!(
            cfg.get_str("hot_corner.corners.top_left"),
            before,
            "被拒的写把值改了"
        );
        assert!(bogus.text().contains("未写入"));

        let missing = write_into(&mut cfg, "hot_corner.not_a_key", "1");
        assert!(missing.refused.is_some());
        assert_eq!(missing.stored, None);
        assert!(missing.text().contains("(这一枚键不存在)"));
    }

    #[test]
    fn the_corner_page_rows_are_the_corner_keys() {
        let cfg = Config::default();
        let rows = rows_of(&cfg, CORNER);
        for k in [
            "hot_corner.enabled",
            "hot_corner.trigger_dip",
            "hot_corner.dwell_ms",
            "hot_corner.require_ctrl",
            "hot_corner.require_shift",
            "hot_corner.repeat_guard_ms",
            "hot_corner.pause_on_fullscreen",
            "hot_corner.corners.bottom_right",
        ] {
            assert!(rows.iter().any(|r| r.key == k), "{k} 不在热区那一页上");
        }
        assert!(rows.iter().all(|r| r.key.starts_with("hot_corner.")));
    }

    /// §5.18.2 那四枚键在页上属于「托盘」那一类（PRD 第 1,448 行「用户打开“首选项 > 托盘”。」
    /// 的那一格，P178 起真的存在；此前它们借住在「快捷键」上，那一处读法记在 [`CATEGORIES`]
    /// 头顶的旧文里）。
    ///
    /// 这一格问的是页那侧的四件事：四枚真出现在**那一类**的行里、那一类里没有别人的键、它们在
    /// 页上不挂「需重启」（泵线程每次点击现问一次配置），以及**语法闸走的是页那一条腿** ——
    /// 打成名单外的字必须是「未写入」且内存里仍是原值，而不是页上先收下、下次开机才发现存不下
    /// （§5.8.3 要的是当场说）。
    #[test]
    fn the_four_tray_keys_are_rows_the_gate_can_refuse() {
        let cfg = Config::default();
        let tray = index_of("托盘").expect("托盘那一类反查不到类号");
        assert_eq!(tray, TRAY_PAGE, "托盘那一类不在钉下的类号上");
        let rows = rows_of(&cfg, tray);
        for name in ["left", "double", "middle", "right"] {
            let key = format!("tray.{name}");
            assert!(rows.iter().any(|r| r.key == key), "{key} 不在托盘那一页上");
        }
        // 搬家搬干净：这一页只许有四枚 tray.*，快捷键那一页只许有 hotkey.*。两页各数一遍，
        // 比 P178 之前那一格更严（那时「这一类全是热键」根本没法断言）。
        assert!(
            rows.iter().all(|r| r.key.starts_with("tray.")),
            "托盘那一页上混进了别人的键：{:?}",
            rows.iter().map(|r| r.key.clone()).collect::<Vec<String>>()
        );
        assert_eq!(rows.len(), 4, "托盘那一页不止四枚：{} 行", rows.len());
        let hotkey = index_of("快捷键").expect("快捷键那一类反查不到类号");
        assert!(
            rows_of(&cfg, hotkey)
                .iter()
                .all(|r| r.key.starts_with("hotkey.")),
            "快捷键那一页上还有没搬走的键"
        );
        assert!(
            rows.iter()
                .filter(|r| r.key.starts_with("tray."))
                .all(|r| !r.restart),
            "托盘那四枚被标成需重启了"
        );

        let mut cfg = Config::default();
        let bogus = write_into(&mut cfg, "tray.left", "nope");
        assert!(bogus.refused.is_some(), "{:?}", bogus.stored);
        assert_eq!(bogus.stored.as_deref(), Some("capture"), "被拒的写把值改了");
        assert!(bogus.text().contains("未写入"));

        let wired = write_into(&mut cfg, "tray.double", "settings");
        assert!(wired.ok(), "{:?}", wired.refused);
        assert_eq!(wired.stored.as_deref(), Some("settings"));
        assert!(wired.corrections.is_empty(), "{:?}", wired.corrections);
    }

    /// §5.5.11 步骤 1 的那一枚键自此在页上有它自己的对面：`[output] hide_pins_on_dialog` 是
    /// 「输出」那一类里的一行、画得出一枚开关（翻两头交回的是**存下来的那一枚**而不是字面
    /// `"true"`）、且不挂「需重启」—— 读它的那一手是 `session::dialog_opened`，每一扇对话框现
    /// 问一次，所以页上改完、下一扇对话框就按新的来。P155 之前页上没有一处点名它，第 176 项欠的
    /// 正是这一格。
    ///
    /// 打成名单外的字时原值一格也不动，与托盘那四枚同一族：「当场说」比「下次开机才发现存不下」
    /// 便宜（§5.20.2 反面那一臂）。
    #[test]
    fn the_dialog_linkage_switch_is_a_row_the_page_can_flip() {
        let key = "output.hide_pins_on_dialog";
        let out = index_of("输出").expect("输出那一类反查不到类号");
        let cfg = Config::default();
        let rows = rows_of(&cfg, out);
        assert!(rows.iter().any(|r| r.key == key), "{key} 不在输出那一页上");
        assert_eq!(
            rows.iter().filter(|r| r.key == key).count(),
            1,
            "{key} 在输出那一页上不止一行：两处点面会各改一次"
        );
        assert!(
            rows.iter().all(|r| r.key.starts_with("output.")),
            "输出那一类里混进了别的段的行"
        );

        assert_eq!(
            kind_of(&cfg, key),
            Kind::Switch(false),
            "{key} 在页上画不出开关：那一行只是一段文本"
        );
        assert_eq!(flip_to(&cfg, key), Some("true"));
        assert!(
            !needs_restart(key),
            "{key} 被标成需重启：页上改完下一扇对话框不会按新的来，那句话就没了对面"
        );

        let mut cfg = Config::default();
        let on = write_into(&mut cfg, key, "true");
        assert!(on.ok(), "{:?}", on.refused);
        assert_eq!(on.stored.as_deref(), Some("true"));
        assert!(on.corrections.is_empty(), "{:?}", on.corrections);
        assert!(
            cfg.output.hide_pins_on_dialog,
            "页上写进去了而内存里那一枚没翻"
        );
        assert_eq!(kind_of(&cfg, key), Kind::Switch(true));
        assert_eq!(
            flip_to(&cfg, key),
            Some("false"),
            "开着那一头没有可翻回去的对面"
        );

        let off = write_into(&mut cfg, key, "false");
        assert!(off.ok(), "{:?}", off.refused);
        assert!(!cfg.output.hide_pins_on_dialog);

        // 同名单里的写法不是「拒」：`on` 落的是存下来的那一枚 bool，回读到的拼法是 `true`。
        let syn = write_into(&mut cfg, key, "on");
        assert!(syn.ok(), "{:?}", syn.refused);
        assert_eq!(syn.stored.as_deref(), Some("true"));
        assert!(cfg.output.hide_pins_on_dialog, "`on` 没把那一枚翻过去");

        // 只有名单外的拼法才拦下。本轮第一版在这一臂写的是 `yes`，而 `yes` 与 `on`/`1`/`true`
        // 同在那八枚里（`crates/core/src/config.rs` 的 `coerce`），断言因此红在 `Some("true")`
        // 上 —— 拦与收的那份名单不是猜的，是从那一枚函数读的。
        let bogus = write_into(&mut cfg, key, "nope");
        assert!(bogus.refused.is_some(), "{:?}", bogus.stored);
        assert!(bogus.text().contains("未写入"));
        assert!(cfg.output.hide_pins_on_dialog, "被拒的写把值改了");
    }

    /// §5.3.7 第一步那句「用户在截图设置…选择单位」自此在页上有一行：`capture.size_unit`
    /// 是「截图」那一类里的一行、画成一格文本框（它存的是一枚枚举，不是 bool）、不挂「需重启」
    /// —— 读它的那一手是 `MaskState::open()`，每按一次热键现取一次快照，所以页上改完、
    /// 下一场截图就按新的来。
    ///
    /// **P164 没有为这一格新画一件控件**：页是 [`Config::flat()`] 生成的，落一枚持久键就是
    /// 落一行；`SettingsWindow.qml` 一字未改。这一格钉的正是那句话的对面 —— 行真在那一页上、
    /// 手型真是文本框、出厂亮的是 `px`，以及名单外的拼法当场被拦且拦下的那句点名这一枚键，
    /// 而不是让整份档在 `from_document` 那里抱怨一句 "unknown variant"（§5.20.2 反面那一臂）。
    #[test]
    fn the_size_unit_is_a_row_the_capture_page_can_refuse() {
        let key = "capture.size_unit";
        let shot = index_of("截图").expect("截图那一类反查不到类号");
        let cfg = Config::default();
        let rows = rows_of(&cfg, shot);
        assert!(rows.iter().any(|r| r.key == key), "{key} 不在截图那一页上");
        assert_eq!(
            rows.iter().filter(|r| r.key == key).count(),
            1,
            "{key} 在截图那一页上不止一行：两处点面会各改一次"
        );
        assert_eq!(owners(key), vec![shot], "{key} 的归属不是截图那一类");

        let row = rows.iter().find(|r| r.key == key).unwrap();
        assert_eq!(row.value, "px", "页上出厂亮着的不是 px");
        assert!(!row.restart, "{key} 被标成需重启：下一场截图本就现读档");
        assert_eq!(
            kind_of(&cfg, key),
            Kind::Field,
            "{key} 在页上画不出一格文本框：它存的是一枚枚举，不是开关"
        );
        assert_eq!(flip_to(&cfg, key), None, "文本框没有可翻的一头");

        let mut cfg = Config::default();
        let dip = write_into(&mut cfg, key, "dip");
        assert!(dip.ok(), "{:?}", dip.refused);
        assert_eq!(dip.stored.as_deref(), Some("dip"));
        assert!(dip.corrections.is_empty(), "{:?}", dip.corrections);
        assert_eq!(
            cfg.capture.size_unit,
            falcon_core::geometry::Unit::Dip,
            "页上写进去了而内存里那一枚没改"
        );

        let bogus = write_into(&mut cfg, key, "plaid");
        assert!(bogus.refused.is_some(), "{:?}", bogus.stored);
        assert!(bogus.text().contains("未写入"));
        assert!(
            bogus.text().contains(key),
            "{} 没点名这一枚键",
            bogus.text()
        );
        assert_eq!(
            cfg.capture.size_unit,
            falcon_core::geometry::Unit::Dip,
            "被拒的写把值改了"
        );

        // 页上收下的拼法与遮罩那两枚芯片是同一张名单：芯片的序、字样的序、页上认下的拼法
        // 自此一枚对一枚（`SIZE_UNITS` 是 `Unit::ALL` 的别名，不是第二份清单）。
        let mut cfg = Config::default();
        for (at, unit) in crate::mask::SIZE_UNITS.iter().enumerate() {
            let written = write_into(&mut cfg, key, unit.label());
            assert!(written.ok(), "{unit:?} 被打回：{:?}", written.refused);
            assert_eq!(written.stored.as_deref(), Some(unit.label()));
            assert_eq!(
                crate::mask::unit_code(*unit),
                at as i32,
                "{unit:?} 的序号与名单不同序"
            );
            assert_eq!(cfg.capture.size_unit, *unit, "页上写的不是芯片那枚单位");
        }
        assert_eq!(
            cfg.get_str(key).as_deref(),
            Some("dip"),
            "页上最后一枚写法没留在档里"
        );
    }

    /// §5.5.11 那一腿在 QML 里的对面：**每一枚** `FileDialog` 都得把 `visible` 绑到桥上那两声。
    ///
    /// 绑 `accepted`/`rejected` 不算，这一格也不许那样绑：取消那一下不走 `accepted`，而「门关了
    /// 却没人叫 `dialogClosed`」正是层数永远停在一的那种漏 —— 桥那侧数的是开与关的**次数**，不是
    /// 门此刻在不在（[`falcon_core::dialog_link::Hidden`]）。
    ///
    /// 分文件配对数，而不是全场配一对：P155 之前设置页那两枚没绑、贴图那枚绑了，一把「有没有人
    /// 绑」的尺在那一趟是绿的。总数钉死在四，是为了让「两半都是零」这种配平也过不了 ——
    /// 多了少了都先看这一格再改。
    #[test]
    fn every_file_dialog_in_the_qml_tells_the_bridge_it_opened() {
        let files = [
            (
                "SettingsWindow.qml",
                include_str!("../qml/SettingsWindow.qml"),
            ),
            ("PinWindow.qml", include_str!("../qml/PinWindow.qml")),
            (
                "AnnotationToolbar.qml",
                include_str!("../qml/AnnotationToolbar.qml"),
            ),
        ];
        let mut total = 0usize;
        for (name, src) in files {
            let dialogs = src.matches("FileDialog {").count();
            let bound = src
                .lines()
                .map(str::trim)
                .filter(|t| {
                    t.starts_with("onVisibleChanged: visible ? ")
                        && t.contains(".session.dialogOpened()")
                        && t.ends_with(".session.dialogClosed()")
                })
                .count();
            assert_eq!(
                dialogs, bound,
                "{name} 里 {dialogs} 枚 FileDialog 而绑了 {bound} 枚"
            );
            total += dialogs;
        }
        assert_eq!(
            total, 4,
            "P155 数到四枚：设置页导出/导入各一、贴图 saveAs 一、标注栏一。对不上就先看这一格"
        );
    }

    #[test]
    fn the_check_table_is_green_on_the_defaults() {
        let (ok, rows) = table();
        assert!(ok, "headless 四格有红：{rows:?}");
        assert_eq!(rows.len(), 4, "{rows:?}");
        let (verdict, line) = check();
        assert_eq!(verdict, Check::Pass);
        assert_eq!(line.split(" ｜ ").count(), 4, "{line}");
    }

    /// §5.21.2 第二枚「快捷键设置页应提供对应功能说明」：那一页上每一枚点了名的一行都递得出
    /// 「名：说明」，一枚没点名的行不许硬拼一句。
    ///
    /// 判据不重算：「该不该有这一句」问 [`action_of`]（页自己用的那一处，不是这里另数一遍键名），
    /// 「有没有这一句」读 [`Row::text`] 交回的串。跑的是 [`Config::default()`] 那一份内存里的档，
    /// 一行也不落到盘上。
    ///
    /// P178 起这一格走**两页**：四枚 `tray.*` 搬到了「托盘」那一页，而它们点出的说明与
    /// `hotkey.actions.*` 用的是同一处作者（[`action_of`]），所以「搬走之后那一页还递不递得出
    /// 说明」是这一格现在也要回答的问题。两页合起来的枚数与搬家前**同一枚 13**，一枚不多一枚
    /// 不少——下面那两枚分页数（9 / 4）钉的就是这件事。
    #[test]
    fn the_hotkey_page_explains_every_action_row() {
        let cfg = Config::default();
        let hotkey_page = index_of("快捷键").expect("快捷键不在名单里");
        let mut page = rows_of(&cfg, hotkey_page);
        let tray_rows = rows_of(&cfg, TRAY_PAGE);
        assert_eq!(tray_rows.len(), 4, "托盘那一页不止四枚");
        page.extend(tray_rows);
        assert_eq!(page.len(), rows_of(&cfg, hotkey_page).len() + 4);
        let mut naming = 0usize;
        let mut marked: Vec<String> = Vec::new();
        let mut quiet: Vec<String> = Vec::new();
        for r in page.iter() {
            let t = r.text();
            assert!(
                !t.contains(RECORD),
                "{}={} 的说明里含分隔符：一行会被切成两行",
                r.key,
                r.value
            );
            match action_of(&r.key, &r.value) {
                Some((name, desc, wired)) => {
                    naming += 1;
                    assert!(t.contains(name), "{} 没递出名 {name}", r.key);
                    assert!(t.contains(desc), "{} 没递出说明", r.key);
                    assert!(
                        t.starts_with(&format!("{}={}", r.key, r.value)),
                        "{} 的行首被说明改掉了：{t}",
                        r.key
                    );
                    assert_eq!(t.contains(UNWIRED_MARK), !wired, "{} 的接线标记", r.key);
                    if !wired {
                        marked.push(r.key.clone());
                    }
                }
                None => {
                    assert!(!t.contains(NOTE_SEP), "{} 没点名动作却拼了一句：{t}", r.key);
                    quiet.push(r.key.clone());
                }
            }
        }
        // 出厂默认这两页上点了名的是十三行：快捷键那一页九枚热键绑定、托盘那一页四枚鼠标动作。
        // 名单外那一行只有 `hotkey.exclusions`（它点的是一串动作，不是一枚动作名），所以两页合
        // 起来十四行、其中十三行点名——搬家前后的枚数一枚没变，变的只是它们站在哪一页上。
        assert_eq!(naming, 13, "九枚热键动作加四枚托盘键，一枚不多一枚不少");
        assert_eq!(page.len(), 14, "两页合起来的行数不是搬家前那十四行");
        assert_eq!(
            rows_of(&cfg, hotkey_page).len(),
            10,
            "快捷键那一页不是九枚绑定加一枚排除"
        );
        assert_eq!(
            quiet,
            vec!["hotkey.exclusions"],
            "页上没点名动作的行不止排除那一枚"
        );
        // 还没一只手接得住的那两枚：§5.16.1 的表里 `custom_task` 压根没有默认绑定，所以它
        // 此刻不成行，剩下两枚成行、各带一个标记。`capture_custom` 自 P166 起不在这两枚里了 ——
        // 它的那只手是 `hotkey::Work::CustomCapture`，落点是 §5.3.5 那扇对话框窗，而这一格读的
        // 正是归宿表，不是页上的写法。
        assert_eq!(
            marked,
            vec!["hotkey.actions.pin_edit", "hotkey.actions.switch_group"],
            "标了「未接线」的行不是那两枚"
        );

        // 假红那一族：`menu` 与 `settings` 没有 `Work`（`Work::of` 认不得它们），可它们一按
        // 就有对面。这一格就是 `tray_run_of` 头部那句话的证人。
        for value in ["menu", "settings", "none", "capture", "paste_pin"] {
            let row = Row {
                key: "tray.right".into(),
                value: value.into(),
                restart: false,
            };
            let t = row.text();
            assert!(t.contains(NOTE_SEP), "{value} 没递出说明：{t}");
            assert!(!t.contains(UNWIRED_MARK), "{value} 被标成了未接线：{t}");
        }
        assert!(Row {
            key: "tray.right".into(),
            value: "menu".into(),
            restart: false
        }
        .text()
        .contains("托盘菜单"));
        // 同一枚写法在两头的读法。`menu` 不是可绑定动作，出厂默认的 `hotkey.actions` 里没有
        // 这样一行；但若有人手把它填进去，这一行照旧递出名与说明，而标记交回**真的**「未接线」
        // —— `Work::of` 认不得它，那条通路确实没有一只手，`plan` 也就会留下它那一句
        // 「未接线（不注册，键留给系统）」。页上这一句与那一句说的是同一件事的两面，不是假红：
        // 假红只发生在拿 `Work::of` 去问**托盘**那一头的时候（上面那五行刚刚证过它不会）。
        let odd = action_of("hotkey.actions.menu", "f13");
        assert!(
            odd.is_some_and(|(_, _, wired)| !wired),
            "menu 作热键时该递出说明且标未接线"
        );
        // 认得出的名才有说明。`validate` 只归一**组合键**、不删名单外的动作名（上面那一段
        // 注释写着它只 clear 语法不通的 spec），所以 `hotkey.actions.nonsense` 这样一行真的会
        // 到页上 —— 这一枚交回 `None` 就是对的：页上不替一个发明的名字编一句中文。
        assert!(action_of("hotkey.actions.capture", "ctrl+shift+a").is_some());
        assert!(action_of("hotkey.actions.nonsense", "f13").is_none());
        assert!(action_of("general.theme", "light").is_none());
    }

    /// 界面读的那三串（类名、整页、单行那一双）与 [`rows`] 之间只许有 [`RECORD`] 这一道缝：
    /// 切出来多一枚或少一枚，就是窗里藏了第二份解析（#281）。这一枚不钉具体行数——行数是活的
    /// config 给的，两趟之间可以不同——它钉的是「同一趟里两条腿一样多，且逐字相同」。
    #[test]
    fn the_page_hands_the_window_one_piece_per_row() {
        let cats_line = category_line();
        let cats: Vec<&str> = cats_line.split(RECORD).collect();
        assert_eq!(cats.len(), CATEGORIES.len(), "{cats:?}");
        assert_eq!(
            cats,
            CATEGORIES.iter().map(|(n, _)| *n).collect::<Vec<&str>>()
        );

        for which in 0..CATEGORIES.len() {
            let live = rows(which);
            let page = page_line(which);
            let pieces: Vec<&str> = page.split(RECORD).collect();
            assert_eq!(
                pieces.len(),
                live.len(),
                "第 {which} 类：页上 {} 枚、模型里 {} 枚",
                pieces.len(),
                live.len()
            );
            for (i, r) in live.iter().enumerate() {
                assert_eq!(
                    pieces[i],
                    r.text(),
                    "第 {which} 类第 {i} 行不是 Row::text 原文"
                );
                assert_eq!(row_text(which, i).as_deref(), Some(r.text().as_str()));
                let line = row_pair(which, i).unwrap();
                let pair: Vec<&str> = line.split(RECORD).collect();
                assert_eq!(
                    pair.len(),
                    2,
                    "第 {which} 类第 {i} 行切出 {} 枚",
                    pair.len()
                );
                assert_eq!(pair[0], r.key, "第 {which} 类第 {i} 行的键名");
                assert_eq!(pair[1], r.value, "第 {which} 类第 {i} 行的值");
            }
            // 越界是「没有这一行」而不是空串：窗拿空串会画出一格谁也没要的行。
            assert!(row_pair(which, live.len()).is_none());
            assert!(row_text(which, live.len()).is_none());
        }
        assert_eq!(page_line(CATEGORIES.len()), "");
    }

    /// 窗里那两枚 `armed` 绑的是**段数**，而段数唯一的权威是本文件拼计划的那两串。
    ///
    /// P179 的三趟（`p179_settingswindow_{F,G,H}.txt`）量到的正是这一处错位：计划长了第十四段
    /// （「哪一页带那两枚钮」），`SettingsWindow.qml` 里那句还停在十三，于是 `armed` 整趟不成立、
    /// Timer 一次没触发、`open` 一行也没递回来 —— 窗明明在场（`born|14|0|1`：十四段、io 计划空、
    /// 可见），门却读成零页。字面量留在窗里是刻意的（段里真混进 `|` 时段数就不对，窗应当当场不跑
    /// 而不是猜），这一格只把它钉在桥那侧的读数上：两份事实仍只有一处说话。
    fn qml_plan_guard(name: &str) -> usize {
        const QML: &str = include_str!("../qml/SettingsWindow.qml");
        let head = format!("readonly property bool {name}:");
        QML.lines()
            .map(str::trim)
            .find(|t| t.starts_with(&head))
            .and_then(|t| t.rsplit("=== ").next())
            .and_then(|t| t.trim().parse::<usize>().ok())
            .unwrap_or_else(|| panic!("QML 里读不到 `{name}` 那枚段数字面量"))
    }

    /// 窗的原文。下面几枚判据读的是**这一份份档案**，不是桥记忆里的那一句。
    const WINDOW_QML: &str = include_str!("../qml/SettingsWindow.qml");

    /// 计划十四段的**名字**，写的顺序就是 [`arm`] 那句 `format!` 拼串的顺序；[`arm_io`] 那三段的
    /// 名字在 [`IO_PLAN_ROLES`] 与 [`PAL_PLAN_ROLES`]。三份名单只有一位读者（下面几枚判据），
    /// 它们不改任何行为。
    ///
    /// 窗手里拿的是 `win.plan[N]` 里那枚 `N`，而「第 N 段是什么意思」这件事在本仓只写在拼串那一处。
    /// P179 的 `p179_settingswindow_{F,G,H}.txt` 量到的是这一族里「段数没对齐」那一形；剩下那一形
    /// ——段数对得上、含义错位（把键名当行号使）——由这两枚判据钉住，见
    /// [`the_plan_role_list_names_in_order_what_arm_composes`] 与
    /// [`the_window_reads_plan_segments_at_the_positions_the_bridge_orders_them`]。
    const PLAN_ROLES: [&str; 14] = [
        "leg_ms",
        "page",
        "save_row",
        "save_key",
        "save_value",
        "cancel_row",
        "cancel_key",
        "cancel_draft",
        "refuse_row",
        "refuse_key",
        "refuse_value",
        "toggle_row",
        "toggle_key",
        "about_page",
    ];
    const IO_PLAN_ROLES: [&str; 3] = ["leg_ms", "export_file", "import_file"];

    /// P183 那第三副计划，顺序对着 [`arm_pal`] 那句 `format!`。行号三段全是 **0 起**的排行号，
    /// 名字两枚是方案名 —— 名单里写清这一件事，窗里那九处 `win.palPlan[N]` 才有对面可对齐。
    const PAL_PLAN_ROLES: [&str; 10] = [
        "leg_ms",
        "page",
        "add_hex",
        "del_row",
        "move_row",
        "move_delta",
        "edit_row",
        "edit_hex",
        "save_name",
        "apply_name",
    ];

    /// `arm()` 此刻拼出来的那一串：第 i 段必须真是 `PLAN_ROLES[i]` 那一件事。拼串的顺序改了而
    /// 名单没改（或反过来）都当场红。名单里冒出一个本格不认识的名字也红——那是「新加了一段而
    /// 没人说清它是什么」。
    #[test]
    fn the_plan_role_list_names_in_order_what_arm_composes() {
        let _serial = door_serial();
        let said = arm();
        assert!(window_wanted(), "没开成窗：{said}");
        let plan = plan_line();
        let p: Vec<&str> = plan.split('|').collect();
        assert_eq!(
            p.len(),
            PLAN_ROLES.len(),
            "计划 {plan:?} 的段数与这份名单不齐：{said}"
        );

        let live = rows(CORNER);
        let row_of = |key: &str| {
            live.iter()
                .position(|r| r.key == key)
                .map(|i| i.to_string())
                .unwrap_or_else(|| panic!("§{CORNER} 那一页上没有 {key}：{said}"))
        };
        let val_of = |key: &str| {
            live.iter()
                .find(|r| r.key == key)
                .map(|r| r.value.clone())
                .unwrap_or_default()
        };
        let about = index_of(ABOUT_PAGE).unwrap_or(CATEGORIES.len()).to_string();

        for (i, role) in PLAN_ROLES.iter().enumerate() {
            let got = p[i];
            match *role {
                "leg_ms" => assert_eq!(got, LEG_MS.to_string(), "第 {i} 段该是腿等待的毫秒数"),
                "page" => assert_eq!(got, CORNER.to_string(), "第 {i} 段该是开在哪一类"),
                "save_row" => assert_eq!(got, row_of(SAVE_KEY), "第 {i} 段该是 {SAVE_KEY} 的行号"),
                "save_key" => assert_eq!(got, SAVE_KEY, "第 {i} 段该是保存那腿的键名"),
                "save_value" => {
                    assert!(
                        SAVE_CANDIDATES.contains(&got),
                        "第 {i} 段 {got:?} 不在 {SAVE_KEY} 的候选里"
                    );
                    assert_ne!(got, val_of(SAVE_KEY), "第 {i} 段与此刻那个值同一个数");
                }
                "cancel_row" => {
                    assert_eq!(got, row_of(CANCEL_KEY), "第 {i} 段该是 {CANCEL_KEY} 的行号");
                }
                "cancel_key" => assert_eq!(got, CANCEL_KEY, "第 {i} 段该是取消那腿的键名"),
                "cancel_draft" => {
                    assert!(
                        CANCEL_CANDIDATES.contains(&got),
                        "第 {i} 段 {got:?} 不在 {CANCEL_KEY} 的候选里"
                    );
                    assert_ne!(got, val_of(CANCEL_KEY), "第 {i} 段与此刻那个值同一个数");
                }
                "refuse_row" => {
                    assert_eq!(got, row_of(REFUSE_KEY), "第 {i} 段该是 {REFUSE_KEY} 的行号");
                }
                "refuse_key" => assert_eq!(got, REFUSE_KEY, "第 {i} 段该是拒那腿的键名"),
                "refuse_value" => assert_eq!(got, REFUSE_VALUE, "第 {i} 段该是拒那腿要打的值"),
                "toggle_row" => {
                    assert_eq!(got, row_of(TOGGLE_KEY), "第 {i} 段该是 {TOGGLE_KEY} 的行号");
                }
                "toggle_key" => assert_eq!(got, TOGGLE_KEY, "第 {i} 段该是开关那腿的键名"),
                "about_page" => assert_eq!(
                    got, about,
                    "第 {i} 段该是「哪一页带那两枚钮」的类号（名单一改窗就数错，见格④）"
                ),
                other => panic!("名单里冒出本格不认识的名字 {other:?}"),
            }
        }
    }

    /// 一行里 `{field}[<数字>]` 那几枚下标，按出现的先后交回。
    fn qml_indices_in(line: &str, field: &str) -> Vec<usize> {
        let pat = format!("{field}[");
        let mut rest = line;
        let mut hits = Vec::new();
        while let Some(at) = rest.find(&pat) {
            let tail = &rest[at + pat.len()..];
            let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
            if digits.is_empty() {
                rest = tail;
                continue;
            }
            hits.push(
                digits
                    .parse::<usize>()
                    .unwrap_or_else(|e| panic!("窗里 {field}[{digits}] 不是个下标：{e}")),
            );
            rest = &tail[digits.len()..];
        }
        hits
    }

    /// 整份窗原文里（注释行不算）读 `{field}` 的每一枚下标，连同它所在的那一行。
    fn qml_plan_indices(field: &str) -> Vec<(usize, usize)> {
        WINDOW_QML
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim_start().starts_with("//"))
            .flat_map(|(n, l)| {
                qml_indices_in(l, field)
                    .into_iter()
                    .map(move |at| (n + 1, at))
            })
            .collect()
    }

    /// 由 `needle` 唯一确定的那**一行**里，窗读 `{field}` 的下标。命中 0 行或多行都不算读数：
    /// 窗改了写法，本格就该先问「那一处现在叫什么」，而不是照旧答案绿一次。
    fn qml_statement(needle: &str, field: &str) -> (usize, Vec<usize>) {
        let hit: Vec<usize> = WINDOW_QML
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains(needle))
            .map(|(n, _)| n + 1)
            .collect();
        let line = match hit.as_slice() {
            [one] => *one,
            other => panic!(
                "窗里 `{needle}` 命中 {} 行（{other:?}），本格要的是唯一一行",
                other.len()
            ),
        };
        let text = WINDOW_QML.lines().nth(line - 1).unwrap_or_default();
        (line, qml_indices_in(text, field))
    }

    /// 窗里那几处读计划的位置，逐枚与名单对齐；并且**每一枚**下标都得被本格点名一次——
    /// 新写一条腿而不登记进来，这一枚当场不绿（P179 之前留下的正是「两枚数段数的字面量在册、
    /// 其余按位置自说自话」那一份不对称）。P183 起是三副计划：`win.plan` 十一处、`win.ioPlan`
    /// 四处、`win.palPlan` 十处。
    #[test]
    fn the_window_reads_plan_segments_at_the_positions_the_bridge_orders_them() {
        let pos = |roles: &[&str], role: &str| {
            roles
                .iter()
                .position(|r| *r == role)
                .unwrap_or_else(|| panic!("名单里没有 {role}"))
        };

        let plan_cases: [(&str, &[&str]); 7] = [
            ("interval: Number(win.plan[", &["leg_ms"]),
            ("win.which = Number(win.plan[", &["page"]),
            ("const want = Number(win.plan[", &["about_page"]),
            (
                "win.legRow(\"save\", ",
                &["save_row", "save_key", "save_value"],
            ),
            (
                "win.legRow(\"cancel\", ",
                &["cancel_row", "cancel_key", "cancel_draft"],
            ),
            (
                "win.legRow(\"refuse\", ",
                &["refuse_row", "refuse_key", "refuse_value"],
            ),
            ("win.legToggle(", &["toggle_row", "toggle_key"]),
        ];
        let io_cases: [(&str, &[&str]); 4] = [
            ("interval: Number(win.ioPlan[", &["leg_ms"]),
            ("\"export|\" + win.ioPlan[", &["export_file", "export_file"]),
            ("doImportPreview(win.ioPlan[", &["import_file"]),
            ("\"apply|\" + win.ioPlan[", &["import_file"]),
        ];
        // P183 那第三副计划：九行、十段一枚不缺。`doPalMove` 那一行一次读两段（行号与方向），
        // 所以是九行而不是十行——「哪一行读了哪几段」由这枚名单钉，不由窗自己数。
        let pal_cases: [(&str, &[&str]); 9] = [
            ("interval: Number(win.palPlan[", &["leg_ms"]),
            ("win.which = Number(win.palPlan[", &["page"]),
            ("doPalAdd(String(win.palPlan[", &["add_hex"]),
            ("doPalDel(Number(win.palPlan[", &["del_row"]),
            ("doPalMove(Number(win.palPlan[", &["move_row", "move_delta"]),
            ("const editAt = Number(win.palPlan[", &["edit_row"]),
            ("const editWant = String(win.palPlan[", &["edit_hex"]),
            ("const saveName = String(win.palPlan[", &["save_name"]),
            ("const applyName = String(win.palPlan[", &["apply_name"]),
        ];

        let mut accounted: Vec<(usize, usize)> = Vec::new();
        for (needle, roles) in plan_cases {
            let (line, got) = qml_statement(needle, "win.plan");
            let want: Vec<usize> = roles.iter().map(|r| pos(&PLAN_ROLES, r)).collect();
            assert_eq!(
                got, want,
                "窗里第 {line} 行读的下标与名单不是一件事：{needle}"
            );
            accounted.extend(got.into_iter().map(|at| (line, at)));
        }
        let mut accounted_io: Vec<(usize, usize)> = Vec::new();
        for (needle, roles) in io_cases {
            let (line, got) = qml_statement(needle, "win.ioPlan");
            let want: Vec<usize> = roles.iter().map(|r| pos(&IO_PLAN_ROLES, r)).collect();
            assert_eq!(
                got, want,
                "窗里第 {line} 行读 io 计划的下标与名单不是一件事：{needle}"
            );
            accounted_io.extend(got.into_iter().map(|at| (line, at)));
        }
        let mut accounted_pal: Vec<(usize, usize)> = Vec::new();
        for (needle, roles) in pal_cases {
            let (line, got) = qml_statement(needle, "win.palPlan");
            let want: Vec<usize> = roles.iter().map(|r| pos(&PAL_PLAN_ROLES, r)).collect();
            assert_eq!(
                got, want,
                "窗里第 {line} 行读调色板计划的下标与名单不是一件事：{needle}"
            );
            accounted_pal.extend(got.into_iter().map(|at| (line, at)));
        }

        for (field, roles, list) in [
            ("win.plan", PLAN_ROLES.as_slice(), accounted),
            ("win.ioPlan", IO_PLAN_ROLES.as_slice(), accounted_io),
            ("win.palPlan", PAL_PLAN_ROLES.as_slice(), accounted_pal),
        ] {
            let mut all = qml_plan_indices(field);
            let mut named = list.clone();
            all.sort();
            named.sort();
            assert_eq!(
                all, named,
                "窗里有一处 `{field}` 下标没被本格点名（新写了一条腿？还是写法变了）"
            );
            for (line, at) in &all {
                assert!(
                    *at < roles.len(),
                    "窗里第 {line} 行读 `{field}[{at}]`，而那份名单只有 {} 段",
                    roles.len()
                );
            }
        }
    }

    /// `arm()` 交出去的是**这一趟**的行号：窗拿行号定位、拿键名核对「这一行确实是说要动的那
    /// 一枚」，两样对不上它什么都不动。候选那两条同时盯 #433——种下去的草稿必须真改得动那一格，
    /// 否则「保存后页上跟着变」是一枚永远绿的尺。
    #[test]
    fn arming_names_rows_the_window_can_actually_change() {
        let _serial = door_serial();
        let said = arm();
        assert!(window_wanted(), "没开成窗：{said}");
        let plan = plan_line();
        let p: Vec<&str> = plan.split('|').collect();
        assert_eq!(p.len(), 14, "计划 {plan:?} 不是十四段：{said}");
        assert_eq!(
            qml_plan_guard("armed"),
            p.len(),
            "窗里 `armed` 数的那枚段数与桥交回来的不是同一份"
        );
        assert_eq!(p[0], LEG_MS.to_string());
        assert_eq!(p[1], CORNER.to_string());
        assert_eq!(
            p[13],
            index_of(ABOUT_PAGE).unwrap().to_string(),
            "计划第 13 段不是「关于与更新」那一页的类号"
        );

        let live = rows(CORNER);
        let idx = |k: &str| live.iter().position(|r| r.key == k).unwrap();
        for (at, key) in [
            (2usize, SAVE_KEY),
            (5, CANCEL_KEY),
            (8, REFUSE_KEY),
            (11, TOGGLE_KEY),
        ] {
            assert_eq!(p[at], idx(key).to_string(), "{key} 的行号");
            assert_eq!(p[at + 1], key, "第 {at} 段的键名");
        }
        assert!(SAVE_CANDIDATES.contains(&p[4]), "{:?} 不在候选里", p[4]);
        assert_ne!(
            p[4],
            live[idx(SAVE_KEY)].value,
            "保存那腿种的草稿等于当前值"
        );
        assert!(CANCEL_CANDIDATES.contains(&p[7]), "{:?} 不在候选里", p[7]);
        assert_ne!(
            p[7],
            live[idx(CANCEL_KEY)].value,
            "取消那腿种的草稿等于当前值"
        );
        assert_eq!(p[10], REFUSE_VALUE);
        assert!(
            gate_says(REFUSE_KEY, REFUSE_VALUE).1,
            "拒那腿的键此刻不拒了：第三条腿没有对象"
        );
        // 开关那一腿的前置与另外三腿不同：它不问「有没有一枚改得动的值」（bool 翻一头必然改得
        // 动），它问「计划指的那一行是不是一枚开关」。页上画成什么形状由 `settingsRowKinds()`
        // 说了算，按下去翻不翻由 [`toggle`] 说了算 —— 这两处都得在按之前就把文本键挡住，因为
        // `Config::coerce` 会把 `"false"` 原样收进一枚 String 键，闸门当场抓不到这一次写。
        assert_eq!(p[12], TOGGLE_KEY);
        let trow = p[11]
            .parse::<usize>()
            .expect("索引 11 那一段（开关那一腿的行号）不是数");
        assert!(
            matches!(row_kind(CORNER, trow), Kind::Switch(_)),
            "计划索引 11 那一段指的行 {trow} 不是一枚开关：{said}"
        );
        assert_eq!(
            live[trow].key, TOGGLE_KEY,
            "第 {trow} 行管的不是 {TOGGLE_KEY}：行号与键名对不上"
        );

        // 这一趟的读数留在 WINDOW 里会让下一枚读到开过窗的状态，本档的四条腿只认门那一趟。
        with_window(|w| {
            w.armed = false;
            w.plan = String::new();
        });
    }

    /// 四条腿的「这一行该长成什么样」是从**克隆的那一份**算的，所以两件事得当面成立：门替用户
    /// 校正过的数（9000 → 5000）在页上也得跟着校正，而这一趟算术一行也不落到盘上、也不动活的
    /// 那一份。
    #[test]
    fn the_expected_row_comes_from_the_gate_not_from_the_asking() {
        let before = crate::settings::with(|s| s.config.get_str(SAVE_KEY));
        let (stored, refused) = gate_says(SAVE_KEY, "9000");
        assert_eq!(
            stored.as_deref(),
            Some("5000"),
            "{:?} / {:?}",
            stored,
            refused
        );
        assert!(!refused);
        let want = expected_text(SAVE_KEY, "9000");
        assert_eq!(want, text_with(SAVE_KEY, "5000"));
        assert!(want.contains("=5000"), "{want}");
        assert!(
            !want.contains("9000"),
            "页上那一格把用户打的数当成了门留下的数：{want}"
        );

        // 拒了的那一枚：expected 仍是「此刻的值」而不是空串——取消与拒两条腿共用这一枚函数。
        let now = crate::settings::with(|s| s.config.get_str(REFUSE_KEY)).unwrap_or_default();
        assert_eq!(
            expected_text(REFUSE_KEY, REFUSE_VALUE),
            text_with(REFUSE_KEY, &now)
        );
        assert_eq!(
            crate::settings::with(|s| s.config.get_str(SAVE_KEY)),
            before,
            "克隆那一腿把活的那一份改了"
        );
        assert!(text_with("general.language", "zh-CN").ends_with("（需重启）"));
        assert!(!text_with(SAVE_KEY, "700").contains("需重启"));
    }

    /// §5.20.3 那一档开出来的是**两份真文件**与一列真行，四件事得当面成立：
    /// ① 计划三段、两枚路径都在门自己的目录下（用户的 `%APPDATA%` 不是草稿纸）；
    /// ② 种下去的那份档与本机活的 config **不是**一字不差 —— 否则「会被盖掉哪几枚」恒为 0 枚，
    ///    是恒绿的尺（#433）；
    /// ③ 建议范围内含 `PLANTED_KEY` 而不含 `STRAY_KEY`，同一份档按全部范围则含后者 —— 后一问是
    ///    分母：没有它，「不含」可以是任何一种坏；
    /// ④ 页上那一列行的文本出自 `import_row_text`，与 `--import-config` 打在控制台上的同一份
    ///    定义（#281），而「读不出那份档」与「没得盖掉」在桥那侧是两种交回形状。
    #[test]
    fn the_io_door_plants_a_file_the_page_can_actually_diff() {
        let _serial = door_serial();
        let before_saves = crate::settings::with(|s| s.saves);
        let planted_from = crate::settings::with(|s| s.config.get_str(PLANTED_KEY));
        let stray_from = crate::settings::with(|s| s.config.get_str(STRAY_KEY));
        let said = arm_io();
        assert!(io_wanted(), "没开成 io 这一趟：{said}");
        // 这一档的对面同样是那一扇窗：`window_wanted` 自此是两只手之或，io armed 就得为真。
        assert!(window_wanted(), "io 开着而窗不开了：{said}");

        let plan = io_plan_line();
        let p: Vec<&str> = plan.split('|').collect();
        assert_eq!(p.len(), 3, "计划 {plan:?} 不是三段：{said}");
        assert_eq!(
            qml_plan_guard("ioArmed"),
            p.len(),
            "窗里 `ioArmed` 数的那枚段数与桥交回来的不是同一份"
        );
        assert_eq!(p[0], LEG_MS.to_string());
        let dir = door_dir();
        let export_at = Path::new(p[1]);
        let import_at = Path::new(p[2]);
        for (at, name) in [(export_at, IO_EXPORT_NAME), (import_at, IO_IMPORT_NAME)] {
            assert_eq!(
                at.file_name().and_then(|s| s.to_str()),
                Some(name),
                "计划第?段 {plan}"
            );
            assert_eq!(at.parent(), Some(dir.as_path()), "{plan}");
        }
        // 导出那一份在开档时是**没有**的：留着上一趟的那份，「档在不在」这一格就先绿了。
        assert!(
            !export_at.exists(),
            "{} 还带着上一趟的那份",
            export_at.display()
        );

        let text = std::fs::read_to_string(import_at)
            .unwrap_or_else(|e| panic!("{}：{e}", import_at.display()));
        let (incoming, _) = Config::parse_import(&text).expect("种的那份档读不通");
        let to = incoming.get_str(PLANTED_KEY).unwrap_or_default();
        let from = planted_from.clone().unwrap_or_default();
        assert!(
            PLANTED_CANDIDATES.contains(&to.as_str()),
            "{to:?} 不在候选里：{said}"
        );
        assert_ne!(to, from, "种进去的值与本机此刻一样：那一格会恒绿");
        assert_ne!(
            incoming.get_str(STRAY_KEY).unwrap_or_default(),
            stray_from.clone().unwrap_or_default(),
            "范围外那一枚没真种进档里：下面那一问「全部范围含它」就没有分母"
        );

        let scope = crate::settings::import_read(import_at, ImportScope::Suggested)
            .unwrap_or_else(|e| panic!("建议范围内读不出那份档：{e}"));
        let names: Vec<&str> = scope.changes.iter().map(|c| c.key.as_str()).collect();
        assert!(
            names.contains(&PLANTED_KEY),
            "建议范围里不含 {PLANTED_KEY}：{names:?}（分母：{said}）"
        );
        assert!(
            !names.contains(&STRAY_KEY),
            "范围外的 {STRAY_KEY} 混进了建议范围：{names:?}"
        );
        let all = crate::settings::import_read(import_at, ImportScope::All)
            .unwrap_or_else(|e| panic!("全部范围读不出那份档：{e}"));
        let all_names: Vec<&str> = all.changes.iter().map(|c| c.key.as_str()).collect();
        assert!(
            all_names.contains(&STRAY_KEY),
            "同一份档按全部范围也不含 {STRAY_KEY}：{all_names:?} —— 那份档坏掉而不是范围窄"
        );

        // 窗拿的那一串：第 0 枚是那句总数，其余一一对着模型自己数出来的那些行。
        let line = io_preview_line(p[2]);
        let drawn: Vec<&str> = line.split(RECORD).collect();
        assert_eq!(
            drawn.len(),
            scope.changes.len() + 1,
            "桥交回 {} 枚而模型数到 {} 枚",
            drawn.len(),
            scope.changes.len()
        );
        assert_eq!(
            drawn[0],
            crate::settings::import_note(
                scope.changes.len(),
                scope.warnings.len(),
                ImportScope::Suggested
            ),
            "抬头那句与 CLI 那句不是同一句"
        );
        for (i, c) in scope.changes.iter().enumerate() {
            assert_eq!(
                drawn[i + 1],
                crate::settings::import_row_text(c),
                "第 {i} 行与 CLI 那一行不同"
            );
        }

        // 两种坏分开交回：读不出 = 一句而没有行；没得盖掉 = 一句 + 零行。
        let missing = io_preview_line("C:\\这一台机器上不可能有的那份档.toml");
        assert!(
            !missing.contains(RECORD) && missing.contains("读不出"),
            "读不出那份档时交回的是：{missing:?}"
        );

        // 本档没动活的 config、也没落一次盘：种档走的是克隆那一份，落点是门自己的目录。
        assert_eq!(
            crate::settings::with(|s| s.config.get_str(PLANTED_KEY)),
            planted_from,
            "活的 config 被种档那一手改了"
        );
        assert_eq!(
            crate::settings::with(|s| s.config.get_str(STRAY_KEY)),
            stray_from,
            "活的 config 被种档那一手改了（范围外那一枚）"
        );
        assert_eq!(
            crate::settings::with(|s| s.saves),
            before_saves,
            "开档这一手落了盘"
        );

        // 分母确实存进了本档，存的正是此刻用同一只手读到的那两列 —— ③④ 拿它当对面，
        // 存错一枚就是判据空转。
        let stored = with_io(|i| {
            (
                i.preview_at_arm,
                i.names_at_arm.clone(),
                i.all_names_at_arm.clone(),
            )
        });
        assert_eq!(
            stored.0,
            scope.changes.len(),
            "开档存的行数({})与本档现读({})不同：{said}",
            stored.0,
            scope.changes.len()
        );
        assert_eq!(
            stored.1,
            names.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
            "开档存的建议范围键名与现读不同"
        );
        assert_eq!(
            stored.2,
            all_names.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
            "开档存的全部范围键名与现读不同"
        );

        // 这一问是「分母为什么要存在开档时」的证人，不是我的推理：把范围内那一枚**应用掉**，
        // 再用同一只手问一次 —— `preview_import` 比的是内存里此刻那一份，那一列行当场空掉。
        // 于是拿读回时那一问判 ④，「范围外不含 STRAY」会以「没得可含」的姿态假绿（#433）。
        // 这里用的是本地克隆：活的 config 与它的落盘次数一次也没被碰。
        let mut post = crate::settings::with(|s| s.config.clone());
        let w = write_into(&mut post, PLANTED_KEY, &to);
        assert!(
            w.refused.is_none(),
            "把 {PLANTED_KEY} 写成 {to:?} 被门拒了：那一问就没有对面"
        );
        let post_names: Vec<String> = post
            .preview_import(&incoming, ImportScope::Suggested)
            .iter()
            .map(|c| c.key.clone())
            .collect();
        assert!(
            post_names.is_empty(),
            "应用之后建议范围还剩 {post_names:?}：时刻这一问在本机上不成立，\
             上面那句「存开档时」就成了无的之矢"
        );
        assert_eq!(
            stored.0,
            scope.changes.len(),
            "上面那一问把本档存的分母也改了：那不是存下来的数"
        );

        // 这一趟的读数留在 IO 里会让下一枚读到的不是它自己那一趟。
        with_io(|i| {
            i.armed = false;
            i.plan.clear();
            i.lines.clear();
        });
        assert!(!io_wanted());
    }

    /// §5.7.22 那一档开出来的是**一扇窗里的第三副计划**与一枚真种进档的方案，五件事得当面成立：
    /// ① 计划十段、`palArmed` 数的正是十段（P179 那一形：段数没对齐，整趟腿一条也不跑）；
    /// ② 门挑的**两枚色都不在开工那一排里** —— 否则新增与改色那两腿当场被模型的规矩挡下，
    ///    「按下去动没动」就成了一枚恒红的尺（#433 的反面：恒绿也是这一形）；
    /// ③ 三枚行号按页上那条腿的次序走，走完那一排**回到开工时的长度**（加一枚、挪一枚、删一枚），
    ///    且复算出来的那一串**不等于**门种的那三枚 —— 相等就是「应用」那一腿按之前按之后一个样，
    ///    判据空转；
    /// ④ 种下去的那一枚方案**在盘上**（走的是唯一那条写路，不是本模块手写的串），而排一字未动、
    ///    落盘次数恰好 +1 —— 这一条与上面 io 那一档相反：io 种的是门自己的档，这一档种的是
    ///    **设置里那一枚键**，所以它必须落一次盘，也必须只落一次；
    /// ⑤ 保存用的名字此刻还不是任何一枚方案的名字（否则「多一行」那一腿永远读不出动静）。
    #[test]
    fn the_palette_door_plants_a_scheme_the_page_can_actually_apply() {
        let _serial = door_serial();
        // 钉在**任何一次写之前**：这一档的 ④ 问的「种下去的那一枚在不在盘上」，盘上指的是门自己
        // 草稿纸上那一份。少这一句，本单测就往用户活的 config 里种方案 —— P183 第一趟就是这么漏
        // 的：那一趟在 ④ 之前炸，那颗方案留在 `%APPDATA%\ai-falconshot\config.toml` 里，下一趟的
        // 门只能拒「门种的那一枚 已经是一枚方案的名字」。
        let before_write_to = crate::settings::with(|s| s.write_to.clone());
        let _pinned = pin();
        // 钉的那一份先抹掉、再当面问一句「里头没有上一趟种的方案」。不抹就是恒绿的尺（#433）：
        // ④ 那一句「种的那一枚在不在盘上」会读着**上一趟**留下的那一行绿，而这一趟的写落没落地
        // 一个字也不欠。
        let _ = std::fs::remove_file(&_pinned);
        let before_disk = disk_value(&_pinned, "annotation", "palette_schemes").unwrap_or_default();
        assert!(
            !before_disk.contains(PAL_PLANT_NAME),
            "钉的落点还带着上一趟种的那一枚：④ 问不出这一趟的动静"
        );
        let before_saves = crate::settings::with(|s| s.saves);
        let said = arm_pal();
        assert!(pal_wanted(), "没开成调色板这一趟：{said}");
        // 这一档的对面同样是那一扇窗：`window_wanted` 自此是三档之或。
        assert!(window_wanted(), "调色板开着而窗不开了：{said}");

        let plan = pal_plan_line();
        let p: Vec<&str> = plan.split('|').collect();
        assert_eq!(p.len(), 10, "计划 {plan:?} 不是十段：{said}");
        assert_eq!(
            qml_plan_guard("palArmed"),
            p.len(),
            "窗里 `palArmed` 数的那枚段数与桥交回来的不是同一份"
        );
        assert_eq!(p[0], LEG_MS.to_string());
        assert_eq!(p[1], index_of(ANNOT_PAGE).unwrap().to_string());

        let (band_at_arm, schemes_at_arm) =
            with_pal(|d| (d.band_at_arm.clone(), d.schemes_at_arm.clone()));
        let count = pal_stored_of(&pal_band_of(&band_at_arm)).len();
        assert_eq!(
            p[4],
            count.to_string(),
            "挪的那一枚是新增之后才存在的那一行"
        );
        assert_eq!(p[3], (count - 1).to_string());
        assert_eq!(p[5], "-1");
        assert_eq!(p[6], "0");
        assert!(count > 0, "开工那一排是空的：{said}");

        // ② 两枚色都得**不在**开工那一排里。
        let flat_at_arm = pal_band_flat_of(&band_at_arm);
        for at in [2usize, 7] {
            let hex = &p[at];
            assert!(
                falcon_core::colors::parse(hex).is_some(),
                "{hex:?} 读不成一枚色：{said}"
            );
            assert!(
                !flat_at_arm.contains(hex),
                "{hex} 已经在开工那一排里，那一腿按下去只会被模型挡下：{flat_at_arm}"
            );
            assert!(
                !PAL_PLANT_COLORS
                    .iter()
                    .any(|plant| plant.eq_ignore_ascii_case(hex)),
                "{hex} 撞上了门自己种的那三枚：应用那一腿读不出动静"
            );
        }

        // ③ 复算那四按：加、挪、删走完回到开工时的长度，改色把它变成**另一串**。
        let after_three = pal_remove_list(
            &pal_move_list(
                &pal_add_list(&band_at_arm, p[2]).unwrap(),
                p[4].parse().unwrap(),
                -1,
            )
            .unwrap(),
            p[3].parse().unwrap(),
        )
        .unwrap();
        assert_eq!(
            after_three,
            pal_stored_of(&pal_band_of(&band_at_arm)),
            "加挪删走完那一排回不到开工时的样子：还原那一格的对面上站着的是别人"
        );
        let replayed = pal_replay(
            &band_at_arm,
            p[2],
            p[4].parse().unwrap(),
            -1,
            p[3].parse().unwrap(),
            p[6].parse().unwrap(),
            p[7],
        )
        .expect("门点名的四按里有一按该被拒：那一趟压根跑不完");
        assert_ne!(replayed, flat_at_arm, "改色那一腿按之前按之后一个样");
        assert_ne!(
            replayed,
            pal_band_flat_of(
                &PAL_PLANT_COLORS
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            ),
            "应用那一腿的「按之前」与「按之后」是同一串：判据空转"
        );

        // ⑤ 与 ④。
        assert!(
            pal_scheme_save(&schemes_at_arm, p[8], &band_at_arm).is_ok(),
            "{said}"
        );
        assert_eq!(p[9], PAL_PLANT_NAME);
        let on_disk = disk_value(&_pinned, "annotation", "palette_schemes").unwrap_or_default();
        assert!(
            on_disk.contains(PAL_PLANT_NAME),
            "种的那一枚方案不在钉的落点上：{on_disk:?}（{said}）"
        );
        assert_eq!(
            crate::settings::with(|s| s.config.annotation.palette.clone()),
            band_at_arm,
            "种方案那一手动了排"
        );
        assert_eq!(
            crate::settings::with(|s| s.saves),
            before_saves + 1,
            "开档这一手应当只落一次盘（种方案那一次）"
        );

        // 收尾：把档还原、把这趟的读数从静态里清掉，下一枚读到的才是它自己那一趟。
        let back = pal_write_list(PAL_SCHEME_KEY, &schemes_at_arm);
        assert!(back.ok(), "还原种的那一枚没写成：{}", back.text());
        with_pal(|d| {
            d.armed = false;
            d.plan.clear();
            d.lines.clear();
        });
        assert!(!pal_wanted());
        // 还原的这一次写**也在钉的那一头**走完，才把写回落点交还给本进程原来那一份：早一步交还，
        // 收尾这一写就落在用户活的 config 上 —— 那正是上面那一段要拦的事。
        crate::settings::pin_writes(&before_write_to);
    }

    /// 开关的分岔本体：**档里存着 bool 才算开关**。这一枚判错的坏不是「少画一格」，而是
    /// 「把一枚文本键写成 `false`」—— 而那一步在语法闸上是**过得去**的（`coerce` 对文本键就是把
    /// 串存下来，见 `core::config` 那一枚同名单测），所以拦不住就得在这一层拦。
    ///
    /// 数、枚举串、内联表里那一枚文本、不在册的键，四样一律 [`Kind::Field`]；真 bool 两头的
    /// 反面互为对方。这一枚全在内存的一份档上问，不落盘、不开窗。
    #[test]
    fn a_switch_is_a_stored_bool_not_the_word_true() {
        let mut cfg = Config::default();
        cfg.set_str("annotation.font_family", "true").unwrap();
        assert_eq!(
            cfg.get_str("annotation.font_family").as_deref(),
            Some("true"),
            "页上那一格写着 true —— 猜文本的那一手在这里就会画出一枚开关"
        );
        assert_eq!(kind_of(&cfg, "annotation.font_family"), Kind::Field);
        assert_eq!(flip_to(&cfg, "annotation.font_family"), None);
        for key in [
            "history.max_items",
            "pin.alpha_bg",
            "hot_corner.corners.top_left",
            "general.language",
            "appearance.theme",
            "nope.nope",
        ] {
            assert_eq!(kind_of(&cfg, key), Kind::Field, "{key} 不是一枚开关");
            assert_eq!(flip_to(&cfg, key), None, "{key} 没有可翻的一头");
        }

        assert_eq!(kind_of(&cfg, TOGGLE_KEY), Kind::Switch(false));
        assert_eq!(flip_to(&cfg, TOGGLE_KEY), Some("true"));
        cfg.set_str(TOGGLE_KEY, "true").unwrap();
        assert_eq!(kind_of(&cfg, TOGGLE_KEY), Kind::Switch(true));
        assert_eq!(flip_to(&cfg, TOGGLE_KEY), Some("false"));

        // 开关与「需重启」是两件事，一枚键可以同时是这两样：页上那一格两样都得说。
        for key in ["general.launch_at_startup", "advanced.cli_enabled"] {
            assert!(
                matches!(kind_of(&cfg, key), Kind::Switch(_)),
                "{key} 在档里不是开关：那一行画不出开关"
            );
            assert!(needs_restart(key), "{key} 不再重启名单里：那句话就没了对面");
            assert!(
                text_with(key, "true").contains("（需重启）"),
                "{key} 那一格没标需重启：{}",
                text_with(key, "true")
            );
        }
    }

    /// 按在一行**不是开关**的行上：交回一句 `refused`，值一格也不动。这一枚一路不调 [`write`]，
    /// 所以它在单测里不落盘 —— 「拒了的写一行都不许动」（§5.20.2 反面那一臂）在开关这一侧的对面。
    ///
    /// 行号越界也走这一枚：它不许假装成一枚存在的键（`stored` 必须是 `None`），否则页上会印出
    /// 一句「内存里仍是 …」，而那个 … 是谁也没要过的一行。
    #[test]
    fn a_row_that_is_not_a_switch_refuses_the_click_and_moves_nothing() {
        let cfg = crate::settings::with(|s| s.config.clone());
        let (which, index, key) = (0..CATEGORIES.len())
            .filter_map(|w| {
                rows_of(&cfg, w).iter().enumerate().find_map(|(i, r)| {
                    if kind_of(&cfg, &r.key) == Kind::Field {
                        Some((w, i, r.key.clone()))
                    } else {
                        None
                    }
                })
            })
            .next()
            .expect("整份名单里一枚文本行也没有：这一问没有对面");

        let out = toggle(which, index);
        assert!(!out.ok(), "{key} 被一枚开关按动了");
        assert_eq!(out.key, key, "拒的那一句报的不是这一行管的键");
        assert!(
            out.refused.is_some(),
            "{key} 那一行按下去没有一句拒：页上只剩一次没有解释的不动（{}）",
            out.text()
        );
        assert_eq!(
            out.stored.as_deref(),
            cfg.get_str(&key).as_deref(),
            "{key} 拒了之后值却不一样了"
        );
        assert_eq!(
            out.asked, "(翻一头)",
            "拒的那一句里出现的应当是「翻一头」这个动作，而不是一串假值"
        );

        let len = rows_of(&cfg, which).len();
        let far = toggle(which, len);
        assert!(!far.ok() && far.refused.is_some(), "越界的一行按出了成功");
        assert_eq!(far.stored, None, "没有这一行却报出了一个值");
    }

    /// [`kinds_line`] 与 [`page_line`] 必须是同一把刀切出来的**同长**两串：窗按行号读第 i 枚时，
    /// 差一位就是把某一行的手型安到了邻居头上。顺带钉下「这一页上真有开关」—— 全在空表上成立的
    /// 判据不算判据（#433：分母要非空）。
    #[test]
    fn the_kind_line_lines_up_with_the_page_line() {
        let cfg = crate::settings::with(|s| s.config.clone());
        let mut switches = 0usize;
        let mut fields = 0usize;
        for w in 0..CATEGORIES.len() {
            let live = rows_of(&cfg, w);
            let kinds: Vec<String> = if live.is_empty() {
                Vec::new()
            } else {
                kinds_line(w).split(RECORD).map(str::to_string).collect()
            };
            assert_eq!(kinds.len(), live.len(), "第 {w} 类的手型串与行串不同长");
            for (i, r) in live.iter().enumerate() {
                let k = kind_of(&cfg, &r.key);
                assert_eq!(kinds[i], k.token(), "第 {w} 类第 {i} 行的手型");
                if k.is_switch() {
                    switches += 1;
                    assert!(
                        r.value == "true" || r.value == "false",
                        "{} 是开关却写着 {:?}",
                        r.key,
                        r.value
                    );
                } else {
                    fields += 1;
                    assert_eq!(kinds[i], "field");
                }
            }
        }
        assert!(switches > 0, "整页一枚开关也没有：上面那些判据全在空表上");
        assert!(fields > 0, "整页一枚文本也没有：那一格输入框就没有对象了");
        assert_eq!(kinds_line(CATEGORIES.len()), "", "越界的一类不许有手型");
    }
}
