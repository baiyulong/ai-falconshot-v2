//! PRD §5.21.1「检查更新」那四步里**能被算出来的那三步**：第 3 步「系统获取版本信息」、
//! 第 4 步里「有新版本时显示版本号、更新说明和下载入口」那三样**内容**。
//!
//! 这一层不联网，而且这一句不是免责声明、是这一枚模块的形状：第 3 步要的「获取」在这里
//! 落地成**读一份本机的清单**，清单路径来自 `[about] release_feed`。为什么不先写一个 HTTP
//! 客户端——因为写了也没人能验：本仓库今天没有任何一台稳定的发布服务器，任何一句「取回成功」
//! 都会是这一代编出来给自己听的判据。所以取回这一手是**读档**，读不到就说读不到；而 PRD 那
//! 三样内容（版本号 / 更新说明 / 下载入口）在两种来源下长的是同一张脸，将来真接上网络时，
//! 换的只有喂进 [`decide`] 的那串字，判决和那三句话一个字不改。
//!
//! 第 4 步的「显示」不在这里——那一手要的是一扇窗，本模块只交出一句**可以被显示**的话。
//! 窗有没有把这句话说出来，判据不归这层（见 §10 本轮那一行的结转）。
//!
//! #281：本层是这三句话唯一的作者。桥那侧的托盘、selftest 的行、门的读回都从这里取，
//! 一处也不另抄。

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
    /// 写了值，但这个 build 不联网：清单的来源只认本机路径。这一句是**拒绝**，不是失败重试。
    RemoteRefused(String),
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
            Answer::RemoteRefused(feed) => format!(
                "检查更新：本 build 不联网，about.release_feed 只认本机文件路径，{feed:?} 不会去取"
            ),
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

/// 第 3 步的那一只手：拿档上那两枚键（`release_feed`、`channel`）和包自己的版本号，交回一个
/// 结局。**读文件这件事在这里发生**——不在 `decide` 里，因为 `decide` 是纯函数，纯函数才能在
/// 没有盘上清单的机器（CI、selftest）被逐格判过；而这一枚函数是那一格与真实盘面之间唯一一座桥。
pub fn check(feed: &str, channel: &str, current: &str) -> Answer {
    match read_feed(feed) {
        Err(answer) => answer,
        Ok(text) => decide(&text, channel, current),
    }
}

/// `feed` 是一枚路径。**不以盘上真相为判据的那几条**（空、含 scheme）先在这里拒掉，剩下的
/// 才去读——顺序是有意义的：`https://…` 交回的是「本 build 不联网」这句拒绝，而不是
/// 「找不到文件」这句失败；两者在用户耳朵里是两件事，前者是这一层的边界，后者是他的盘出了状况。
fn read_feed(feed: &str) -> Result<String, Answer> {
    let trimmed = feed.trim();
    if trimmed.is_empty() {
        return Err(Answer::NoFeed);
    }
    if trimmed.contains("://") {
        return Err(Answer::RemoteRefused(trimmed.to_string()));
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

/// §5.21.1 之外的那半句：`[about] check_every_days` 与 `auto_check_updates` 两枚键**今天有读者**
/// （config 那一层把它们读出来，selftest 那一行数过它们），但没有任何一只手在**自动**发起检查。
/// 这一枚纯函数把那两枚键的判决先写成产品的话，是为了当那一只手落下去时，它要写的判据已经
/// 在这里、不必再发明一遍。`last_days` 是「上一次自动检查距今天数」，还没有地方存它——那是
/// 结转里那一行，不是这一格能假装的事。
pub fn auto_decision(auto: bool, every_days: u32, last_days: Option<u32>) -> &'static str {
    if !auto {
        return "自动检查关了（about.auto_check_updates=false），这一趟不会自己发起";
    }
    match last_days {
        None => "没有上次检查的记录，自动检查可以跑",
        Some(days) if days >= every_days => "距上次自动检查已过天数够数，自动检查可以跑",
        Some(_) => "距上次自动检查还不够 about.check_every_days 那枚天数，这一趟跳过",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# 一份人手写的清单
channel=stable version=1.30.0 notes=修了三张贴图的坑 url=https://example.invalid/stable
channel=beta version=1.31.0-beta notes= url=
";

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
            Answer::RemoteRefused("https://x/y".into()).download_url(),
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
        assert_eq!(check("", "stable", "1.0.0"), Answer::NoFeed);
        assert!(check("", "stable", "1.0.0")
            .sentence()
            .contains("release_feed 是空的"));
    }

    #[test]
    fn a_url_is_refused_before_any_attempt_to_read_it() {
        // 这一格的判据是**话术**：把 https:// 读成「文件不存在」是把这一层的边界说成用户的盘
        // 出了状况，而这两件事用户处理方式完全不同。
        let answer = check("https://example.invalid/stable", "stable", "1.0.0");
        let Answer::RemoteRefused(feed) = &answer else {
            panic!("{answer:?}")
        };
        assert_eq!(feed, "https://example.invalid/stable");
        assert!(
            answer.sentence().contains("不联网"),
            "{}",
            answer.sentence()
        );
    }

    #[test]
    fn a_missing_file_says_which_one_it_tried() {
        let answer = check("no_such_release_manifest.bin", "stable", "1.0.0");
        let Answer::Unreadable(why) = &answer else {
            panic!("{answer:?}")
        };
        assert!(why.contains("no_such_release_manifest.bin"), "{why}");
    }

    #[test]
    fn has_manifest_is_true_only_for_rows_that_read_a_manifest() {
        assert!(decide(SAMPLE, "stable", "1.0.0").has_manifest());
        assert!(!Answer::NoFeed.has_manifest());
        assert!(!Answer::RemoteRefused("x".into()).has_manifest());
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

    #[test]
    fn auto_decision_reaches_the_three_readings() {
        assert!(auto_decision(false, 7, None).contains("自动检查关了"));
        assert!(auto_decision(true, 7, None).contains("没有上次检查的记录"));
        assert!(auto_decision(true, 7, Some(3)).contains("这一趟跳过"));
        assert!(auto_decision(true, 7, Some(7)).contains("够数"));
    }

    use crate::config::Config;
}
