//! PRD §5.21.1「检查更新」那四步里**能被算出来的那三步**：第 3 步「系统获取版本信息」、
//! 第 4 步里「有新版本时显示版本号、更新说明和下载入口」那三样**内容**。
//!
//! 这一层**不联网**，而且这一句不是免责声明、是这一枚模块的形状：第 3 步要的「获取」在这里是
//! **一只手作为参数**（[`Getter`]）。取回字的那一只手住在平台那一层（`platform_windows::fetch`，
//! 走这台机器的 wininet），本层只把取回来的那串字判成版本号、更新说明与下载入口。
//!
//! 为什么拆成两半而不是在这里直接联网：判决要能在没有网络的机器上被逐格问过（CI、selftest、
//! 以及**没把** `release_feed` 写成地址的那些档），而网络那一只手必须有一个能塞进假货的位置 ——
//! 否则「取回失败」那一格的判据只能靠真断一次网线来证（#261）。
//!
//! 取回这一手**是**真的自它落地那天起：`https://` 那一枚 `release_feed` 如今会被去取。此前它
//! 交回的是一句「本 build 不联网」的拒绝，那一句是本仓库还没有 HTTP 客户端那一刻的形状，
//! 已经跟着那一格一起退役（留下的那一格是 [`Answer::FetchFailed`]，说的是**这一次**没取回来，
//! 不是「这一层不肯取」）。本地路径那一支照旧能喂：一份盘上的清单与一份取回来的清单走的是
//! 同一只 [`decide`]、长的是同一张脸。
//!
//! 第 4 步的「显示」不在这里——那一手要的是一扇窗，本模块只交出一句**可以被显示**的话。
//! 窗有没有把这句话说出来，判据不归这层（见 §10 本轮那一行的结转）。
//!
//! #281：本层是这几句话唯一的作者。桥那侧的托盘、selftest 的行、门的读回都从这里取，
//! 一处也不另抄。§6.3 那一枚「涉及远程获取时应明确提示网络请求」的那句话也在这里
//! （[`net_notice`]），因为只有这一层同时看得见「这是地址」与「闸肯不肯取」。

/// §5.18.1 第 1441 行那一列菜单建议里的写法就是「检查更新」四个字，**托盘那一项的名字唯一作者**
/// 在这里（与 `history_window::TRAY_WORD` / `group_window::TRAY_WORD` 同一分工，#281）：托盘那一
/// 项、`--update-check` 门里那一格、selftest 数的名单，问的都必须是这一枚字面量。窗标题与菜单名
/// 是分开的两件事——菜单要说「这里是检查更新」，而那一句判决的话另有作者（[`Answer::sentence`]）。
pub const TRAY_WORD: &str = "检查更新";

/// §5.21.1 第 4 步那枚「下载入口」**上钮写的字**，唯一作者在这里（与 [`TRAY_WORD`] 同一分工，
/// #281）：页上那个 `Text`、桥侧递回给窗的那一串第二段、门里那一格问的都是这一枚字面量。
///
/// 为什么不把 PRD 那四个字「下载入口」原样上钮：第 1,555 行说的是**显示什么内容**（入口 = 那一串
/// 地址），而钮要说的是将要发生的一件事。这一枚把两头都留着 —— 「打开」是动作，「下载入口」是
/// 文档原词，所以门还能拿 PRD 那三枚字在这枚钮上核一次对账，而不是核一句我们自己起的名字。
pub const DOWNLOAD_WORD: &str = "打开下载入口";

/// 与 `config.rs` 里 `[about].channel` 那一族同名同数的名单。这一层之所以要再写一枚，不是
/// 因为那份名单不权威——是因为 [`Manifest::parse`] 要在**没有 Config** 的时候也能拒绝一个
/// 不认识的通道，而 core 的 Config 是一整份档的解析器，为一行清单拉起它不值得。两处必须
/// 同数，所以有一条测试专门盯着它（[`tests::channels_match_the_config_roster`]）。
pub const CHANNELS: [&str; 3] = ["stable", "beta", "nightly"];

/// 清单上一个通道的三枚键。PRD 第 4 步点名的三样——版本号、更新说明、下载入口——就是这三枚，
/// 一一对得上，这一层不替它发明第四样。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
}

/// 一份清单解析完了的样子。`refused` 与 `parsed` 同形于 `send_to` 那一族：一行没被认下来
/// 不等于整份清单作废，但**必须数得出来**，否则「清单上写的是 1.30 而我读到 1.20」这种事故
/// 就只能靠运气发现。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub entries: Vec<(String, Release)>,
    pub refused: Vec<String>,
}

impl Manifest {
    /// 一行一个通道：`channel=stable version=1.30.0 notes=... url=https://...`。
    ///
    /// 值的边界按第一条「下一个键名=」切，所以 `notes` 里可以有中文、可以有竖线、可以有空格，
    /// 只要不写 `url=` 这三个字符。这不是偷懒，是让一份人手写的清单不必学转义语法。
    ///
    /// 认不下来的行**逐行**交回原因：空行与 `#` 注释不算行也不进 refused（它们不是「想被解析
    /// 而没被解析」的东西），其余每一枚不认识的行都留一句。缺 `version` 的通道行、通道名不在
    /// [`CHANNELS`] 里的行、重复的通道，三种都在这里落下。
    pub fn parse(text: &str) -> Manifest {
        let mut out = Manifest::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some(rest) = line.strip_prefix("channel=") else {
                out.refused
                    .push(format!("{line}：这一行不以 channel= 开头"));
                continue;
            };
            let (name_raw, after_version, has_version) = cut(rest, "version=");
            if !has_version {
                out.refused.push(format!("{line}：这一行没有 version="));
                continue;
            }
            let name = name_raw.trim();
            if name.is_empty() {
                out.refused.push(format!("{line}：channel= 后面没有通道名"));
                continue;
            }
            if !CHANNELS.contains(&name) {
                out.refused.push(format!("{name} is not a release channel"));
                continue;
            }
            let (version_raw, after_notes, has_notes) = cut(after_version, "notes=");
            let version = version_raw.trim();
            if version.is_empty() {
                out.refused.push(format!("{name}：version= 后面是空的"));
                continue;
            }
            let (notes_raw, url_raw, _has_url) = if has_notes {
                cut(after_notes, "url=")
            } else {
                ("", "", false)
            };
            let notes = if has_notes {
                notes_raw.trim().to_string()
            } else {
                String::new()
            };
            let url = url_raw.trim().to_string();
            let name = name.to_string();
            if out.entries.iter().any(|(n, _)| *n == name) {
                out.refused
                    .push(format!("{name}：这个通道这一份清单里写过两遍"));
                continue;
            }
            out.entries.push((
                name,
                Release {
                    version: version.to_string(),
                    notes,
                    url,
                },
            ));
        }
        out
    }

    pub fn release(&self, channel: &str) -> Option<&Release> {
        self.entries
            .iter()
            .find(|(n, _)| n == channel)
            .map(|(_, r)| r)
    }
}

/// 切出 `key=` 之前那段，并说这个键**在不在**。第三个返回值不是装饰：`channel=stable
/// version=1.2` 这一行没有 `notes=`，把「没有这个键」和「有这个键但值为空」读成同一件事，
/// 就会把一份只写到版本号的清单读成「说明是空的、入口也是空的」——它确实该被认下来（版本号
/// 是这一层唯一不能少的东西），但要靠**知道**后面没有键来认，而不是靠猜。
fn cut<'a>(text: &'a str, key: &str) -> (&'a str, &'a str, bool) {
    match text.find(key) {
        Some(at) => (&text[..at], &text[at + key.len()..], true),
        None => (text, "", false),
    }
}

/// 点分数字版本拆段。`1.29.0` → `[1,29,0]`。任何一段不是纯数字就交回 `None`——这一手不猜，
/// `1.30-beta` 这种带后缀的号本层的读数「不认识」，而「不认识」在 [`decide`] 里是一句拒绝、
/// 不是一次「大概差不多」。
pub fn version_parts(text: &str) -> Option<Vec<u64>> {
    let parts: Vec<&str> = text.trim().split('.').collect();
    if parts.is_empty() {
        return None;
    }
    let mut nums = Vec::with_capacity(parts.len());
    for part in parts {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        nums.push(part.parse::<u64>().ok()?);
    }
    Some(nums)
}

/// 比两枚版本号。段数不等时短的那边按 0 补，所以 `1.30` 与 `1.30.0` 是**同一个版本**而不是
/// 「后者更新」——这一判不是审美：清单作者写 `1.30` 而包里写 `1.30.0` 的时候，报一次「发现
/// 新版本」就等于让用户去下一份和自己一样的东西。
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (version_parts(candidate), version_parts(current)) {
        (Some(a), Some(b)) => {
            let len = a.len().max(b.len());
            for i in 0..len {
                let x = *a.get(i).unwrap_or(&0);
                let y = *b.get(i).unwrap_or(&0);
                if x != y {
                    return x > y;
                }
            }
            false
        }
        _ => false,
    }
}

/// 一次检查的全部可能结局。每一枚都带得上自己那句**产品的话**（[`Answer::sentence`]），
/// 所以门读回的那一句、托盘浮出来的那一句、selftest 数的那一行，是同一枚函数吐的同一串字。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// 档上 `[about] release_feed` 是空的——没得取。
    NoFeed,
    /// 写的是地址、也确实去取了，可那一份没取回来：连不上、状态码不是 2xx、比 [`FEED_LIMIT`]
    /// 大、读不出字、或者那一句根本过不了 [`crate::link`] 那道闸。`{why}` 是取回那一只手交回的
    /// **原话**，本层不替它重述（#281）；`{feed}` 留着是为了让用户看得见是哪一句没通 —— 一份档上
    /// 可能同时留着两枚地址。
    FetchFailed { feed: String, why: String },
    /// 是本机路径，但读不出字来（不存在、是目录、没有权限）。原因原样带出去。
    Unreadable(String),
    /// 读出来了，可这个通道在这一份清单上一行也没有。
    NoChannel(String),
    /// 第 4 步的反面：没有新版本。带**两枚**号——包里这枚和清单上那枚——因为只报一枚的话，
    /// 「已是最新」和「清单上那枚其实更旧」在读回里长一个样。
    UpToDate { current: String, listed: String },
    /// 第 4 步点名的三样：版本号、更新说明、下载入口。
    Available {
        current: String,
        release: Box<Release>,
    },
}

impl Answer {
    /// 那一句产品的话。`pub` 的意义在于**只此一处**：谁要显示，谁就调用它，不许抄一遍字。
    pub fn sentence(&self) -> String {
        match self {
            Answer::NoFeed => {
                "检查更新：还没配发布清单（about.release_feed 是空的），这一趟没东西可取"
                    .to_string()
            }
            Answer::FetchFailed { feed, why } => {
                format!("检查更新：{feed:?} 那一份没取回来（{why}）")
            }
            Answer::Unreadable(why) => format!("检查更新：清单读不出来（{why}）"),
            Answer::NoChannel(channel) => format!("检查更新：清单里没有 {channel} 这一行"),
            Answer::UpToDate { current, listed } => {
                format!("已是最新版本 {current}（清单上 {listed} 不比它新）")
            }
            Answer::Available { current, release } => format!(
                "发现新版本 {}（当前 {current}）：{}｜下载入口 {}",
                release.version,
                if release.notes.is_empty() {
                    "（清单没写说明）".to_string()
                } else {
                    release.notes.clone()
                },
                if release.url.is_empty() {
                    "（清单没写入口）".to_string()
                } else {
                    release.url.clone()
                }
            ),
        }
    }

    /// 这一趟**取到清单了没有**。判决话术之外唯一需要分支的一判：托盘那一行只有在真读到清单时
    /// 才追加「下载」那一枚入口，否则菜单会多出一行点了只会重述失败。
    pub fn has_manifest(&self) -> bool {
        matches!(self, Answer::UpToDate { .. } | Answer::Available { .. })
    }

    /// 第 4 步那枚「下载入口」的**地址本体**：话术里那串字之外，页上的钮要知道自己会打开什么。
    /// 只有「有新版本、且清单写了入口」这一格交回 `Some`——其余各格交回 `None`，因为失败的话术
    /// 里没有地址，钮若照旧可点就是在打开一枚没有的东西。
    pub fn download_url(&self) -> Option<&str> {
        match self {
            Answer::Available { release, .. } if !release.url.is_empty() => Some(&release.url),
            _ => None,
        }
    }
}

/// 一份清单最多允许多大。**这一枚数是产品的话、不是机制的数**：它说的是「本程序认一份版本
/// 清单该有多大」，而把它交给 `fetch` 去执行的是调用者（`platform_windows::fetch::text(url,
/// FEED_LIMIT)`）。超了是**失败**（[`Answer::FetchFailed`]），不是「拿前面那一段做判决」——
/// 一份被剪掉尾巴的清单会读出半个通道。
///
/// 1 MiB 比任何一份真实的三通道清单大两个数量级（清单是一行一行的 `channel= version= notes=
/// url=`，人写的），因此这一枚数真正拦的是「喂进来的不是清单」：一台把 `release_feed` 指到
/// 首页、指到一份 JSON、指到一张图片的档。
pub const FEED_LIMIT: usize = 1024 * 1024;

/// 第 3 步「获取」那一只手的**形状**。`Err` 那一串必须是**已经成句**的话（真机那一只手交回的
/// 是 `platform_windows::fetch::FetchError` 的 `Display`），本层只把它放进
/// [`Answer::FetchFailed`] 的 `{why}`，一个字也不另编（#281）。
pub type Getter<'a> = &'a dyn Fn(&str) -> Result<String, String>;

/// §6.3「涉及在线识别或远程图片获取时，应明确提示网络请求」那一枚键里，**「要不要提示 + 提示
/// 的那一句」**的入口：句子本体住在 [`web_notice`] 那一手里，这一枚只加那道闸。
///
/// 只在「这一句确实会被去取」时交回一句话：判据与取回那一只手共用 [`crate::link`] 那一道闸，
/// 所以不会出现「提示了要联网、结果一句拒绝说它不是地址」。`None` = 不是 http(s)、或域名含
/// 非 ASCII —— 那两种形态一次网络请求都不发，于是也不该提示。
///
/// 递进去的 `what` 说的是这一趟要去取的是**什么**：`[about] release_feed` 那一趟取的是清单
/// （[`running_line`] 递 `那份清单`），§5.8.4 方式二那一趟拖进来的是图片（`crates/qt_bridge` 的
/// `drop_in` 递 `那一份图片`）。句式、向哪儿说、以及「什么形态不提示」这三件事只在这里写过一次；
/// 两个读者各编一句，§6.3 那一枚键就有了两个作者，而其中一个迟早会忘了说要联网。
pub fn net_notice(url: &str, what: &str) -> Option<String> {
    match crate::link::classify(url) {
        crate::link::Target::Web(web) if web.openable() => Some(web_notice(&web, what)),
        _ => None,
    }
}

/// [`net_notice`] 那一句的**本体**：host 已经由 [`crate::link`] 解析出来的那一位，直接拿它说话。
///
/// 为什么单独立这一手、而不是让第二个读者去叫 [`net_notice`] 再 `unwrap_or` 一句：`net_notice`
/// 的 `None` 那一支说的是「这一句压根不会被去取」，而手里已经是 [`crate::link::Web`] 的那一位
/// **早过了同一道闸**（`drop::Dropped::classify` 只在 `openable()` 时才交回 `Web`）。逼它从
/// `None` 里编一个替代品，就是给 §6.3 那一枚键多开一个「想不起来说要联网」的出口（#433）。
pub fn web_notice(web: &crate::link::Web, what: &str) -> String {
    format!("这一趟要向 {} 取{what}：那是一次网络请求。", web.display_host())
}

/// 第 3 步「在路上」那一句：按下去到落地之间，屏幕上那一行写的字。它**不是** [`Answer`] 的
/// 一格 —— 那一刻还没有判决，而把「还没有判决」写成一种判决正是 #453 拦的那件事。
pub const RUNNING: &str = "检查更新：这一趟已经在跑，取回那一份清单之前还没有判决。";

/// 按下去那一刻那一行的**唯一**作者：在路上那一句，加上（只有真会被去取的那句地址才跟着的）
/// 那一次网络请求的提示。§6.3 那一枚「应明确提示网络请求」必须在这一趟**发起前**说出口 ——
/// 等落地那一句再提示，用户看到的是请求已经发生了。
pub fn running_line(feed: &str) -> String {
    match net_notice(feed, "那份清单") {
        Some(notice) => format!("{RUNNING}{notice}"),
        None => RUNNING.to_string(),
    }
}

/// 第 3 步的那一只手：拿档上那两枚键（`release_feed`、`channel`）、包自己的版本号，与**取回字
/// 的那一只手**（[`Getter`]），交回一个结局。**读盘与取回都在这里发生**——不在 `decide` 里，因为
/// `decide` 是纯函数，纯函数才能在没有盘上清单的机器（CI、selftest）被逐格判过；而这一枚函数是
/// 那一格与真实盘面／真实网络之间唯一一座桥。
pub fn check(feed: &str, channel: &str, current: &str, get: Getter) -> Answer {
    match read_feed(feed, get) {
        Err(answer) => answer,
        Ok(text) => decide(&text, channel, current),
    }
}

/// `feed` 或是一枚路径、或是一句地址。**不以盘上真相为判据的那几条**（空）先在这里拒掉，
/// 剩下的按形态分给两只手：含 `://` 的递给 [`Getter`]（**这一层自己一次 socket 都不碰**），
/// 其余的按路径读盘。
///
/// 顺序是有意义的：一句取不回来的地址交回的是「那份清单没取回来」，而不是「找不到文件」——
/// 两者在用户耳朵里是两件事，前者问他的是网络与那台服务器，后者问他的是自己的盘（#453）。
fn read_feed(feed: &str, get: Getter) -> Result<String, Answer> {
    let trimmed = feed.trim();
    if trimmed.is_empty() {
        return Err(Answer::NoFeed);
    }
    if trimmed.contains("://") {
        return get(trimmed).map_err(|why| Answer::FetchFailed {
            feed: trimmed.to_string(),
            why,
        });
    }
    std::fs::read_to_string(trimmed).map_err(|e| Answer::Unreadable(format!("{trimmed}：{e}")))
}

/// 拿着清单的字做判决：纯函数，`check` 与 selftest 与单元测试都走这一枚。
pub fn decide(text: &str, channel: &str, current: &str) -> Answer {
    let manifest = Manifest::parse(text);
    match manifest.release(channel) {
        None => Answer::NoChannel(channel.to_string()),
        Some(release) if is_newer(&release.version, current) => Answer::Available {
            current: current.to_string(),
            release: Box::new(release.clone()),
        },
        Some(release) => Answer::UpToDate {
            current: current.to_string(),
            listed: release.version.clone(),
        },
    }
}

/// §5.21.1 之外的那半句：`[about] auto_check_updates` 与 `check_every_days` 是本项目自己加的两枚
/// 键（PRD 那一节只写用户点的那一次），它们要的读者是「启动那一趟自己起不起检查」。
///
/// 判决与那句产品的话出自**同一次分支**：分成两枚函数写，就会出现「做的事」与「说的话」各有
/// 一套想法的一天（#281），而这两枚键的坏正好是那种谁也看不见谁的形状。
///
/// `last_days` = 「上一次自动检查距今天数」。读不到记录（没写过、档坏了、机器钟走过表）交回
/// `None`，那一支与「从没检查过」走同一条路：起跑一次比替用户按住一整族更新少坏一些。
pub struct Auto {
    pub wanted: bool,
    pub why: &'static str,
}

pub fn auto_plan(auto: bool, every_days: u32, last_days: Option<u32>) -> Auto {
    let (wanted, why) = if !auto {
        (
            false,
            "自动检查关了（about.auto_check_updates=false），这一趟不会自己发起",
        )
    } else {
        match last_days {
            None => (
                true,
                "没有上次自动检查的记录，这一趟自己起（about.check_every_days 不必够数）",
            ),
            Some(days) if days >= every_days => (
                true,
                "距上次自动检查已过天数够 about.check_every_days 那枚数，这一趟自己起",
            ),
            Some(_) => (
                false,
                "距上次自动检查还不够 about.check_every_days 那枚天数，这一趟跳过",
            ),
        }
    };
    Auto { wanted, why }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# 一份人手写的清单
channel=stable version=1.30.0 notes=修了三张贴图的坑 url=https://example.invalid/stable
channel=beta version=1.31.0-beta notes= url=
";

    /// 一次也没碰 socket 的那一只**假的手**：「取回」在这一层是一枚参数，因此正面那一臂
    /// （取回来一份新清单）在 core 就能证，不需要哪台稳定的发布服务器。反面那一臂（真取不回）
    /// 的话术原样带出，由 `platform_windows::fetch` 那一侧负责；`--update-check` 那一档对着
    /// **本机**起的那一扇服务器量真的那一臂。
    const FETCHED: &str = "channel=stable version=1.31.0 notes=取回来的那一份 url=https://example.invalid/dl\n";

    /// 递给 [`check`] 那一只**永远用不上**的手：路径那一支不该走到它上面。
    fn never_taken(_url: &str) -> Result<String, String> {
        panic!("本地路径被当成地址递给了取回的手");
    }

    #[test]
    fn a_known_channel_line_parses_and_an_unknown_one_is_refused() {
        let ok = Manifest::parse("channel=stable version=1.2.3 notes=n url=u");
        assert_eq!(ok.entries.len(), 1);
        assert!(ok.refused.is_empty(), "{:?}", ok.refused);
        assert_eq!(ok.release("stable").unwrap().version, "1.2.3");

        let bad = Manifest::parse("channel=weekly version=1.2.3 notes=n url=u");
        assert!(bad.entries.is_empty());
        assert_eq!(
            bad.refused,
            vec!["weekly is not a release channel".to_string()]
        );
    }

    #[test]
    fn a_line_without_a_version_is_refused_not_defaulted() {
        // 「缺 version 就当成 0.0.0」会把一份写坏的清单读成「有新版可下」，那正是这一层最坏的
        // 一种错：它看起来是绿的。
        let m = Manifest::parse("channel=stable notes=只有说明");
        assert!(m.entries.is_empty());
        assert_eq!(m.refused.len(), 1, "{:?}", m.refused);
        assert!(m.refused[0].contains("没有 version="), "{:?}", m.refused);
    }

    #[test]
    fn a_duplicated_channel_is_refused_rather_than_shadowed() {
        let m = Manifest::parse(
            "channel=stable version=1.1 notes=a url=b\nchannel=stable version=1.2 notes=c url=d",
        );
        assert_eq!(m.entries.len(), 1);
        assert!(m.refused[0].contains("写过两遍"), "{:?}", m.refused);
        assert_eq!(m.release("stable").unwrap().version, "1.1");
    }

    #[test]
    fn comments_and_blank_lines_are_not_rows() {
        let m = Manifest::parse("\n# 一行注释\n\nchannel=beta version=2.0 notes= url=\n");
        assert_eq!(m.entries.len(), 1);
        assert!(m.refused.is_empty(), "{:?}", m.refused);
    }

    #[test]
    fn version_compare_is_numeric_not_lexical() {
        // 字符串比法会说 "1.9" > "1.29"，而这一判的后果是用户被劝去下一份更旧的东西。
        assert!(is_newer("1.29.0", "1.9.0"));
        assert!(!is_newer("1.9.0", "1.29.0"));
        assert!(!is_newer("1.30", "1.30.0"));
        assert!(!is_newer("1.30.0", "1.30"));
        assert!(is_newer("2.0.0", "1.99.99"));
    }

    #[test]
    fn a_version_this_layer_does_not_read_is_not_a_new_version() {
        assert_eq!(version_parts("1.30-beta"), None);
        assert!(!is_newer("1.30-beta", "1.20.0"));
        assert_eq!(version_parts(""), None);
    }

    #[test]
    fn decide_names_the_three_things_the_prd_asks_for() {
        let answer = decide(SAMPLE, "stable", "1.29.0");
        let Answer::Available { release, current } = &answer else {
            panic!("expected Available, got {answer:?}");
        };
        assert_eq!(current, "1.29.0");
        assert_eq!(release.version, "1.30.0");
        let line = answer.sentence();
        assert!(line.contains("发现新版本 1.30.0"), "{line}");
        assert!(line.contains("修了三张贴图的坑"), "{line}");
        assert!(
            line.contains("下载入口 https://example.invalid/stable"),
            "{line}"
        );
    }

    /// 两枚钮上的字各有唯一作者，而**下载那一枚里留着 PRD 第 1,555 行那三枚字**：这一格判的不是
    /// 好不好看，是「窗上那两个字将来改了，文档那一行还核不核得上」。
    #[test]
    fn the_two_button_words_have_one_author_each_and_keep_the_prd_word() {
        assert_eq!(TRAY_WORD, "检查更新");
        assert_eq!(DOWNLOAD_WORD, "打开下载入口");
        assert!(DOWNLOAD_WORD.contains("下载入口"), "{DOWNLOAD_WORD}");
        assert_ne!(DOWNLOAD_WORD, TRAY_WORD);
    }

    #[test]
    fn the_download_entry_is_an_address_only_when_there_is_one_to_open() {
        // 页上那枚钮可点不可点，问的是这一枚函数，不是那句里有没有一串像地址的字。
        assert_eq!(
            decide(SAMPLE, "stable", "1.29.0").download_url(),
            Some("https://example.invalid/stable")
        );
        let no_entry = decide(
            "channel=stable version=1.30.0 notes=n url=\n",
            "stable",
            "1.29.0",
        );
        assert!(matches!(no_entry, Answer::Available { .. }), "{no_entry:?}");
        assert_eq!(no_entry.download_url(), None, "{}", no_entry.sentence());
        assert_eq!(decide(SAMPLE, "stable", "1.30.0").download_url(), None);
        assert_eq!(Answer::NoFeed.download_url(), None);
        assert_eq!(
            Answer::FetchFailed {
                feed: "https://x/y".into(),
                why: "连不上".into()
            }
            .download_url(),
            None
        );
    }

    #[test]
    fn up_to_date_reports_both_numbers() {
        let answer = decide(SAMPLE, "stable", "1.30.0");
        assert!(matches!(answer, Answer::UpToDate { .. }), "{answer:?}");
        let line = answer.sentence();
        assert!(
            line.contains("已是最新版本 1.30.0") && line.contains("清单上 1.30.0"),
            "{line}"
        );
    }

    #[test]
    fn a_channel_that_is_not_on_the_manifest_is_its_own_answer() {
        let answer = decide(SAMPLE, "nightly", "1.0.0");
        assert_eq!(answer, Answer::NoChannel("nightly".to_string()));
        assert!(
            answer.sentence().contains("清单里没有 nightly"),
            "{}",
            answer.sentence()
        );
    }

    #[test]
    fn an_empty_feed_is_not_reported_as_a_failure() {
        assert_eq!(check("", "stable", "1.0.0", &never_taken), Answer::NoFeed);
        assert!(check("", "stable", "1.0.0", &never_taken)
            .sentence()
            .contains("release_feed 是空的"));
    }

    /// 地址那一支**取回来了**：判决用的是那一只手交回的字，而这一格在 core 就能证 —— 不需要
    /// 任何一台稳定的发布服务器，也不需要一次真 socket（#261：证人是一枚参数）。
    #[test]
    fn an_address_is_fetched_and_the_fetched_words_are_judged() {
        let asked = std::cell::Cell::new(String::new());
        let get = |url: &str| {
            asked.set(url.to_string());
            Ok(FETCHED.to_string())
        };
        let answer = check("https://example.invalid/RELEASES.txt", "stable", "1.0.0", &get);
        assert_eq!(
            asked.into_inner(),
            "https://example.invalid/RELEASES.txt",
            "那句地址没原样递给取回的手"
        );
        let Answer::Available { release, .. } = &answer else {
            panic!("{}", answer.sentence())
        };
        assert_eq!(release.version, "1.31.0");
        assert_eq!(release.notes, "取回来的那一份");
        assert_eq!(release.url, "https://example.invalid/dl");
        assert_eq!(answer.download_url(), Some("https://example.invalid/dl"));
    }

    /// 首尾空白剥掉再递：档里 `"  https://x/y  "` 与 `"https://x/y"` 是同一份清单，而取回那一只
    /// 手收到带空白的串时，`link` 那道闸会把主机名读歪。
    #[test]
    fn the_fetched_address_is_the_trimmed_one() {
        let asked = std::cell::Cell::new(String::new());
        let get = |url: &str| {
            asked.set(url.to_string());
            Ok(FETCHED.to_string())
        };
        check("  https://example.invalid/RELEASES.txt ", "stable", "1.0.0", &get);
        assert_eq!(asked.into_inner(), "https://example.invalid/RELEASES.txt");
    }

    /// 取不回的那一格里，话术**不能**是「找不到文件」：那是把用户自己的盘出了状况说成是他的
    /// 网络出了状况（#453 的两个方向都算）。这一格同时把取回那一只手那句原话一个字不改地留着。
    #[test]
    fn a_failed_fetch_names_the_address_and_keeps_the_getters_sentence() {
        let answer = check(
            "https://example.invalid/RELEASES.txt",
            "stable",
            "1.0.0",
            &|_| Err("取不回那一句地址：连不上那台服务器（系统返回 12002）".to_string()),
        );
        let Answer::FetchFailed { feed, why } = &answer else {
            panic!("{answer:?}")
        };
        assert_eq!(feed, "https://example.invalid/RELEASES.txt");
        assert!(why.contains("12002"), "{why}");
        let line = answer.sentence();
        assert!(line.contains("没取回来") && line.contains("12002"), "{line}");
        assert!(!line.contains("找不到文件"), "{line} 把网络的事说成了盘的事");
        assert!(!answer.has_manifest(), "{line} 说它读到了一份清单");
        assert_eq!(answer.download_url(), None);
    }

    /// 本地路径那一支**一次也不**走到取回的手上：`never_taken` 一被调用就 panic，因此这一格的
    /// 证人就是「没 panic」，而它守的是「配了盘上清单的用户不会被偷偷联网」（§6.3 的反面）。
    #[test]
    fn a_local_path_never_reaches_the_getter() {
        let answer = check(
            "no_such_release_manifest.bin",
            "stable",
            "1.0.0",
            &never_taken,
        );
        let Answer::Unreadable(why) = &answer else {
            panic!("{answer:?}")
        };
        assert!(why.contains("no_such_release_manifest.bin"), "{why}");
    }

    /// §6.3「涉及在线识别或远程图片获取时，应明确提示网络请求」那一枚键的那一句话：只有
    /// **确实会被去取**的那一句地址才提示。判据与取回那一只手共用 [`crate::link`] 那道闸，
    /// 所以不会出现「提示了要联网，结果一句拒绝说它不是地址」。
    ///
    /// 两个读者各递自己那一枚名词，句式只在这里写过一次：换掉那枚名词，提示的还是同一句「要联网」；
    /// 换掉那一句式，两处会一起换 —— 而 §6.3 要的正是「说了没有」，不是「说的是哪样东西」。
    #[test]
    fn the_network_notice_is_said_only_for_an_address_that_would_be_fetched() {
        let said = net_notice("https://releases.example.invalid/RELEASES.txt", "那份清单")
            .expect("该提示的一次网络请求没提示");
        assert!(
            said.contains("releases.example.invalid") && said.contains("网络请求"),
            "{said}"
        );
        assert!(said.starts_with("这一趟要向"), "{said}");
        assert!(said.contains("那份清单"), "{said}");
        // §5.8.4 方式二那一位读者：同一句句式，另一样东西。
        let picture = net_notice("https://img.example.invalid/a.png", "那一份图片")
            .expect("远程图片也是远程获取，§6.3 对它说同一句话");
        assert!(picture.starts_with("这一趟要向"), "{picture}");
        assert!(picture.contains("那一份图片"), "{picture}");
        assert!(picture.contains("网络请求"), "{picture}");
        assert_ne!(said, picture, "两位读者说的是同一句还是同一枚宾语，得分得出来");
        // 端口不是默认口时那枚数跟着一起说：`:8443` 换的是**另一台服务**。
        let ported = net_notice("http://127.0.0.1:8443/RELEASES.txt", "那份清单")
            .expect("门那一档走的就是这一枚形态");
        assert!(ported.contains("127.0.0.1:8443"), "{ported}");
        // 三种都不会发一次请求的形态，一种也不提示。
        assert!(net_notice("https://exämple.invalid/RELEASES.txt", "那份清单").is_none());
        assert!(net_notice("ftp://releases.example.invalid/x", "那份清单").is_none());
        assert!(net_notice("", "那份清单").is_none());
        // 图片那一位读者撞上同样的形态，也一样不提示（不许因为宾语换了就多发一次「要联网」）。
        assert!(net_notice("ftp://img.example.invalid/a.png", "那一份图片").is_none());
    }

    /// 在路上那一句里**不许**有任何判决的形状（「已是最新」「发现新版本」「取不到」都不许）：
    /// 那一时刻三样都还没发生，而屏幕上那一行被读成一次判决的日子必然比被读成等待的日子多。
    /// 地址那一支要**在这趟发起前**把网络请求说出来（§6.3），本地路径那一支不许提网络。
    #[test]
    fn the_running_line_promises_no_verdict() {
        let local = running_line("C:/releases/RELEASES.txt");
        assert_eq!(local, RUNNING, "{local}");
        for banned in ["已是最新", "发现新版本", "没取回来", "读不出来", "清单里没有"] {
            assert!(!local.contains(banned), "{local} 把判决写进了等待里");
        }
        let remote = running_line("https://releases.example.invalid/RELEASES.txt");
        assert!(remote.starts_with(RUNNING), "{remote}");
        assert!(remote.contains("网络请求"), "{remote} 没把这一次联网说出来");
    }

    /// [`FEED_LIMIT`] 是产品的话、不是机制的数：它必须比任何一份人写的三通道清单大得多，否则
    /// 「清单太大」这一格会在真清单上亮。这一格钉的是那枚数量级，写死的数一旦改小到贴着
    /// 真实清单，这里就红。
    #[test]
    fn the_feed_limit_is_far_above_a_hand_written_manifest() {
        assert!(
            SAMPLE.len() * 100 < FEED_LIMIT,
            "上限 {FEED_LIMIT} 离一份样例清单（{} 字节）不够远",
            SAMPLE.len()
        );
    }

    #[test]
    fn has_manifest_is_true_only_for_rows_that_read_a_manifest() {
        assert!(decide(SAMPLE, "stable", "1.0.0").has_manifest());
        assert!(!Answer::NoFeed.has_manifest());
        assert!(!Answer::FetchFailed {
            feed: "https://x/y".into(),
            why: "连不上".into()
        }
        .has_manifest());
        assert!(!Answer::Unreadable("x".into()).has_manifest());
        assert!(!Answer::NoChannel("beta".into()).has_manifest());
    }

    #[test]
    fn channels_match_the_config_roster() {
        // #281 那一族的形状：通道名单在 core 的 Config 和这一层各有一份字面量，加一枚而忘了
        // 另一处时，清单会拒绝一个 Config 认为合法的通道。这一格是那两枚字面量唯一的接头。
        let c = Config::default();
        let mut seen = Vec::new();
        for name in ["stable", "beta", "nightly"] {
            let mut cfg = c.clone();
            cfg.about.channel = name.to_string();
            cfg.validate();
            assert_eq!(cfg.about.channel, name, "{name} 被 validate 改写了");
            seen.push(name);
        }
        assert_eq!(seen.len(), CHANNELS.len());
        assert_eq!(c.about.channel, "stable");
    }

    /// 四支各证一次「做的事」与「说的话」出自同一次分支：每一格都把 `wanted` 与那一句一起判，
    /// 一支也不许只读句子 —— 只读句子的那一版判不出「跳过那一支真跳过了没有」。
    #[test]
    fn auto_plan_pairs_each_reading_with_its_verdict() {
        let off = auto_plan(false, 7, None);
        assert!(!off.wanted, "关了还是自己起了：{}", off.why);
        assert!(off.why.contains("自动检查关了"), "{}", off.why);

        let first = auto_plan(true, 7, None);
        assert!(first.wanted, "没记录就该起跑：{}", first.why);
        assert!(
            first.why.contains("没有上次自动检查的记录"),
            "{}",
            first.why
        );

        let due = auto_plan(true, 7, Some(7));
        assert!(due.wanted, "够数却没起：{}", due.why);
        assert!(
            due.why.contains("够 about.check_every_days 那枚数"),
            "{}",
            due.why
        );

        let early = auto_plan(true, 7, Some(3));
        assert!(!early.wanted, "不够数还是自己起了：{}", early.why);
        assert!(early.why.contains("这一趟跳过"), "{}", early.why);
    }

    use crate::config::Config;
}
