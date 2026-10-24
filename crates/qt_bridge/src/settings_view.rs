//! §5.20.1 设置分类 与 §5.20.2 设置即时生效 —— 设置页那一侧的模型。
//!
//! 盘上早就有两份各管一段的码：[`crate::settings`] 拥有这一份文件（谁读、谁写、写不动
//! 了记在哪），[`falcon_core::config`] 拥有每一枚键的语法（`flat` 数得出有哪些键、
//! `set_str` 决定一个字串能不能成为这一枚键的值）。**这一档两头都不占**，它回答的是文档
//! §5.20 那两节问的两件事：
//!
//! * 「设置页包含哪几类」—— 十一类的名单，和每一类在该页上列出哪些行；
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

use falcon_core::config::{needs_restart, Config, ImportScope, RESTART_KEYS};

/// §5.20.1 那十一类，名字与顺序都照文档里那一列数字（1 常规 … 11 关于与更新）。
///
/// 每类后面跟的是 `Config` 的**段名**而不是点分前缀：一枚键归哪一类，问的是它头一段叫什么
/// —— 拿前缀去 `starts_with` 会漏掉嵌套（`pin.alpha_bg` 与假想的 `pinish.x` 在这一族里是两
/// 件不同的事），而段名是 `Config` 结构体的字段，编译器盯着。
///
/// `card` 归进「贴图」而不是自己成一类：文档只有十一类，而 `Card` 那一段全是贴图卡片怎么
/// 画（圆角、阴影、边框、放大镜之外的那几枚），把它单列出来就成了第十二类 —— 名单是文档的，
/// 不是这里的发明。
pub const CATEGORIES: [(&str, &[&str]); 11] = [
    ("常规", &["general"]),
    ("截图", &["capture"]),
    ("标注", &["annotation"]),
    ("贴图", &["pin", "card"]),
    ("输出", &["output"]),
    ("快捷键", &["hotkey"]),
    ("热区", &["hot_corner"]),
    ("外观", &["appearance"]),
    ("历史记录", &["history"]),
    ("高级与命令行", &["advanced"]),
    ("关于与更新", &["about"]),
];

/// §5.15 那一页在名单里的类号。写成一枚常量而不是 `6`：页上的行、门里的那格、单测里的断言
/// 问的是同一件事——「热区是第几类」。
pub const CORNER: usize = 6;

/// 页上的一行：一枚键、它此刻的值、和它要不要重启。
pub struct Row {
    pub key: String,
    pub value: String,
    pub restart: bool,
}

impl Row {
    /// 页上那一格的样子，也是这一档每行印出来的样子。`（需重启）` 就是 §5.20.2 第二条要的
    /// 「明确标记」——它挂在行尾而不是单起一行，这样读的人拿行数还能对上「这一类几枚」。
    pub fn text(&self) -> String {
        if self.restart {
            format!("{}={}（需重启）", self.key, self.value)
        } else {
            format!("{}={}", self.key, self.value)
        }
    }
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

    // ① 十一类是不是把这份 config 分完了：漏一枚 = 页上看不见这一行；重一枚 = 两格改同一值。
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
/// * `settings categories` —— **这一台机器手里那一份** config 的十一类各列了几行，外加页上
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
    rep.row(
        "settings categories",
        Check::from(named_ok),
        format!(
            "读={}；十一类共 {total} 行：{}；标需重启 {} 枚 {:?}",
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
// §5.20.1 的那一扇窗：界面读法，与 `--settings-window` 那一档的三条腿
// ──────────────────────────────────────────────────────────────────────────────

/// 类名与行交给界面的那一个分隔符。选 `\u{1d}` 而不是 `|`／换行，理由与
/// [`crate::session`] 里 `group_encode` 那一枚相同：**值是用户自己打的**
/// （`appearance.font_family` 里出现一个 `|` 完全合法），任何可见分隔符都会把一行切成两行。
/// 这一串是给 QML 的 `Repeater` 读的，不是给人读的一句话 —— 人要读的那句在
/// [`Row::text`] 与 [`Written::text`] 里，而这两处本档都不重新拼。
pub const RECORD: &str = "\u{1d}";

/// 十一类的标签，顺序就是 §5.20.1 那一列数字。页上那排标签的**唯一**来源：QML 里再抄一份
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

/// 一行的样子，值由调用方给。开工那一刻的读数要用**记住的那个值**拼，而不是现在模型里的值 ——
/// 三条腿跑完之后模型早已不是开工时那份了。
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

/// 「把 `asked` 打进去之后，页上这一行该长成什么样」。被拒的写没动值，所以这一枚对三条腿
/// 都成立：保存那腿算出来是新值，取消与拒那两腿算出来仍是原来那一枚。
fn expected_text(key: &str, asked: &str) -> String {
    let (stored, _) = gate_says(key, asked);
    text_with(key, &stored.unwrap_or_default())
}

/// 下面这三条腿点的键，全在 §5.15 那一页（[`CORNER`]）上：这一档开着的是哪一页，判的就是
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

/// 窗开起来之后隔多久跑腿。这一格要的是「窗已经在桌面上了」而不是「文档编译过了」，
/// 而 Qt 从 `createObject` 到第一帧之间那段时间没有任何一条腿能借。
pub const LEG_MS: i32 = 400;
/// 这一档的地板：腿的等待 + 读回的余量。比 `--hint` 那一档宽，因为这里跑的是**十一页 +
/// 三条腿**，每一页都是过一次桥。
pub const MIN_WINDOW_MS: i32 = 1_600;

/// `--settings-window` 那一档在自己这一趟里看见过的东西。
///
/// 三条腿各存一份 `(行号, 键, 要打进去的值, 开工那一刻的值)`：行号是**这一趟**在 §5.15 那一页
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
    saves_at_arm: u32,
    save_idx: usize,
    save_asked: String,
    save_at_arm: String,
    cancel_idx: usize,
    cancel_draft: String,
    cancel_at_arm: String,
    refuse_idx: usize,
    refuse_at_arm: String,
}

impl WindowDoor {
    const fn none() -> Self {
        Self {
            armed: false,
            reason: String::new(),
            page: CORNER,
            plan: String::new(),
            lines: Vec::new(),
            saves_at_arm: 0,
            save_idx: 0,
            save_asked: String::new(),
            save_at_arm: String::new(),
            cancel_idx: 0,
            cancel_draft: String::new(),
            cancel_at_arm: String::new(),
            refuse_idx: 0,
            refuse_at_arm: String::new(),
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

/// 窗一共递回来了哪些行（原文）。零行的那一格用它把「哪一种坏」印出来而不是猜：`born` 没有是
/// 窗没被造出来，`born` 有而 `tick` 没有是那条 Timer 腿没跑，`tick` 有而 `open` 没有是 `runGauge`
/// 半路断了。三样都是同一句读数的不同形状，判据一条也没多写（#281）。
pub fn seen() -> Vec<String> {
    with_window(|w| w.lines.clone())
}

/// 计划那一串，十一段：`腿等待 ms | 开在哪一类 | 保存的行号 | 保存的键 | 保存的值 |
/// 取消的行号 | 取消的键 | 取消那份草稿 | 拒的行号 | 拒的键 | 拒的值`。
///
/// 缺一窗里那条腿就不跑（同 `pinGaugePlan` 那一族：空串 = 产品会话与另外七道门，界面这几条腿
/// 整条躲开）。行号与键**两样都给**是有意的：窗拿行号定位那一行，拿键核对「这一行确实是我说
/// 的要动的那一枚」，两者对不上时窗什么都不动 —— 指错一行去写值是比不写更坏的那一种。
pub fn plan_line() -> String {
    with_window(|w| w.plan.clone())
}

/// 窗读它自己那一格的入口：这一趟该不该有一扇设置窗（`main.qml` 在
/// `Component.onCompleted` 里叫一次 [`crate::session`] 那一枚 `refreshSettings`，
/// 那一枚把这里的答复抬进 qproperty）。
///
/// **两档都要那一扇窗**：`--settings-window` 的三条腿与 §5.20.3 那两枚钮点的是同一扇
/// [`crate::settings_view`] 造出来的窗，而 `settings_open` 只有一枚 —— 谁 armed 就造，
/// 造出来之后窗照自己手里那副计划决定跑哪几条腿（两档互相拒并列，见 `main.rs` 的 refusals，
/// 所以这一趟至多只有一副计划）。
pub fn window_wanted() -> bool {
    with_window(|w| w.armed) || io_wanted()
}

/// 从候选里挑一枚**与此刻那个值不一样**的。全都一样（或键压根不在页上）就返回 `None`，
/// `arm` 据此不开窗并说清为什么。
fn pick(candidates: &[&'static str; 3], current: &str) -> Option<&'static str> {
    candidates.iter().copied().find(|c| *c != current)
}

/// 开档时在册：找到三枚键在页上的行号、记下它们当时的值与落盘次数，再拼出计划。返回的那句话
/// 是打印给读档人看的「这一趟动了谁」，不是判据。
///
/// 它必须在 `engine.load()` **之前**跑完，理由与 `--autosave` 那一档写在同一段注释里：
/// `SettingsWindow.qml` 只在 `Component.onCompleted` 读一次计划。
pub fn arm() -> String {
    let page_rows = rows(CORNER);
    let at_arm = |key: &str| -> String {
        page_rows
            .iter()
            .find(|r| r.key == key)
            .map(|r| r.value.clone())
            .unwrap_or_default()
    };
    let idx = |key: &str| page_rows.iter().position(|r| r.key == key);

    let (si, ci, ri) = (idx(SAVE_KEY), idx(CANCEL_KEY), idx(REFUSE_KEY));
    let (save_at_arm, cancel_at_arm, refuse_at_arm) =
        (at_arm(SAVE_KEY), at_arm(CANCEL_KEY), at_arm(REFUSE_KEY));
    let asked = [
        si.and_then(|_| pick(&SAVE_CANDIDATES, &save_at_arm)),
        ci.and_then(|_| pick(&CANCEL_CANDIDATES, &cancel_at_arm)),
    ];
    let saves_at_arm = crate::settings::with(|s| s.saves);

    let reason = match (si, ci, ri, asked[0], asked[1]) {
        (Some(si), Some(ci), Some(ri), Some(save_asked), Some(cancel_draft)) => {
            let plan = format!(
                "{LEG_MS}|{CORNER}|{si}|{SAVE_KEY}|{save_asked}|{ci}|{CANCEL_KEY}|{cancel_draft}|{ri}|{REFUSE_KEY}|{REFUSE_VALUE}"
            );
            with_window(|w| {
                w.armed = true;
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
            });
            return format!(
                "计划 {plan}；开在 热区#{CORNER}（{} 行）；开工时三枚键 = {:?} / {:?} / {:?}；落盘次数 {saves_at_arm}",
                page_rows.len(),
                save_at_arm,
                cancel_at_arm,
                refuse_at_arm
            );
        }
        (None, _, _, _, _) => format!("{SAVE_KEY} 不在 热区 那一页上（{} 行里没有这一行）", page_rows.len()),
        (_, None, _, _, _) => format!("{CANCEL_KEY} 不在 热区 那一页上（{} 行里没有这一行）", page_rows.len()),
        (_, _, None, _, _) => format!("{REFUSE_KEY} 不在 热区 那一页上（{} 行里没有这一行）", page_rows.len()),
        (Some(_), Some(_), Some(_), None, _) => format!(
            "{SAVE_KEY} 此刻已是候选之一（{save_at_arm:?} 对 {:?}）：没有一枚改得动的值，这一趟不开窗",
            SAVE_CANDIDATES
        ),
        (Some(_), Some(_), Some(_), Some(_), None) => format!(
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
    if io_takes_report() {
        with_io(|i| i.lines.push(line.to_string()));
    } else {
        with_window(|w| w.lines.push(line.to_string()));
    }
}

/// `--settings-window` 的那扇门。六行，每行一格，全都对面是**这一趟真开着的那扇窗**。
///
/// 与 [`measure`] 的分工写在这里而不是含糊过去：那一档五格读的是**模型**（`rows` /
/// `write_into` / 采样腿 / 盘上那份文件），一扇窗也不开；这一档六格读的是**界面**——每一格
/// 的对面都是 `SettingsWindow.qml` 经过那四枚桥上的 invokable 递回来的话，而本档手里
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
        // 的是零行，探针两趟印出的是 `born`（窗在、计划 11 段）而没有 `open` —— 这一格不改判据，
        // 它只让下一趟不必再靠猜。
        let seen_lines = seen();
        rep.row(
            "settings window",
            Check::Blocked,
            format!(
                "窗没有递回 `open` 行（qml_loaded={}）；窗一共递回 {} 行 {:?}",
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

    // ② 十一页逐页过一遍桥：这一格才是 §5.20.1 那一份名单在**窗上**的读数。类号、行数两样
    // 都要对上，而「对不上」的两种坏（少一页、页数错）在这一行里分开印。
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

    // ⑥ 副作用那一格：这一趟真落盘几次。窗里保存一次 + 本档还原一次 = 二，多一位就是别处也在
    // 写，少一位就是那两条腿里有一条压根没落盘（而页上却写着成了）。这一格不参与上面任何一格的
    // 判据，它判的是「这一档有没有把用户的设置档当草稿纸」。
    let now_saves = crate::settings::with(|s| s.saves);
    let landed = now_saves.saturating_sub(saves_at_arm);
    rep.row(
        "settings saves",
        Check::from(landed == 2),
        format!(
            "落盘次数 {} -> {}，本档走了 {landed} 次（要的是 2：窗里保存 + 本档还原；钉的落点 {}）",
            saves_at_arm,
            now_saves,
            pin().display()
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
// 「按一枚钮改一个数」，所以计划那十一段在这里一段也用不上 —— 这一档有自己的计划、自己的
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
/// 段与段之间是 `|` 而不是 `RECORD` —— 这一串与上面那一档的十一段同一族（[`plan_line`]），
/// 而窗那侧切计划用的也是 `splitPlan`。两枚路径里不可能出现 `|`：那是 Windows 文件名的禁字。
pub fn io_plan_line() -> String {
    with_io(|i| i.plan.clone())
}

/// 这一趟该不该有一扇设置窗。**两只手都要那一扇窗**：上面那一档的三条腿，和这一档的两枚钮。
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Check;

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

    /// 名单是文档的：十一类、逐字、顺序也照那一列。类名一旦与 §5.20.1 不同，页上的标签就
    /// 是这一档自己发明的第二份名单。
    #[test]
    fn the_eleven_categories_are_the_prd_list_in_order() {
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
                "热区",
                "外观",
                "历史记录",
                "高级与命令行",
                "关于与更新",
            ]
        );
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

    #[test]
    fn the_check_table_is_green_on_the_defaults() {
        let (ok, rows) = table();
        assert!(ok, "headless 四格有红：{rows:?}");
        assert_eq!(rows.len(), 4, "{rows:?}");
        let (verdict, line) = check();
        assert_eq!(verdict, Check::Pass);
        assert_eq!(line.split(" ｜ ").count(), 4, "{line}");
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

    /// `arm()` 交出去的是**这一趟**的行号：窗拿行号定位、拿键名核对「这一行确实是说要动的那
    /// 一枚」，两样对不上它什么都不动。候选那两条同时盯 #433——种下去的草稿必须真改得动那一格，
    /// 否则「保存后页上跟着变」是一枚永远绿的尺。
    #[test]
    fn arming_names_rows_the_window_can_actually_change() {
        let said = arm();
        assert!(window_wanted(), "没开成窗：{said}");
        let plan = plan_line();
        let p: Vec<&str> = plan.split('|').collect();
        assert_eq!(p.len(), 11, "计划 {plan:?} 不是十一段：{said}");
        assert_eq!(p[0], LEG_MS.to_string());
        assert_eq!(p[1], CORNER.to_string());

        let live = rows(CORNER);
        let idx = |k: &str| live.iter().position(|r| r.key == k).unwrap();
        for (at, key) in [(2usize, SAVE_KEY), (5, CANCEL_KEY), (8, REFUSE_KEY)] {
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

        // 这一趟的读数留在 WINDOW 里会让下一枚读到开过窗的状态，本档的三条腿只认门那一趟。
        with_window(|w| {
            w.armed = false;
            w.plan = String::new();
        });
    }

    /// 三条腿的「这一行该长成什么样」是从**克隆的那一份**算的，所以两件事得当面成立：门替用户
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
}
