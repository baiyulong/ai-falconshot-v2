//! §5.21.1 的那一扇门：托盘那枚「检查更新」按下去到底做什么，以及那句判决里的三样东西
//! （版本号 / 更新说明 / 下载入口）是不是**产品自己写的**那三样。
//!
//! PRD 第 1441 行把「检查更新」列进托盘菜单建议，而 `hotkey::tray_menu` 头顶那封欠账信从
//! P138 起一直记着它没有手（「菜单建议里的 帮助 / 检查更新 两条，本 build 都还没有能跑的
//! 那只手」）。第 1552–1555 行那四步里，第 1、2 步要的是菜单上那一项和它后面的归宿，第 3 步
//! 要的是「系统获取版本信息」，第 4 步要的是那三样内容。这一档逐格问这五件事，外加两件
//! 边界：档上没配清单该说没配，清单写的是 URL 该说本 build 不取回。
//!
//! ## 为什么这一档不开窗、也不等任何人
//!
//! 它读的全是「一份档 → 一个决定」：真往本档目录里种五份 fixture 与两份清单、真走产品那个
//! [`Settings::open`] 读回来（`Config::load` ⇒ `validate` ⇒ 归一，一次也不绕），再把读回来的
//! 那两枚 `[about]` 键递给**产品那一层用的同一只函数** [`falcon_core::update::check`]。于是
//! 锁屏会话也判得出，也不必有人的手放在鼠标上。
//!
//! ## 这一档不声称的那两件事
//!
//! 一，它一次也没有真往那枚菜单项投过一次点击（`WM_TRAY` 那一发要在跑的泵上），所以「真人
//! 点中它走的是不是 `Work::Update` 这一臂」由下面那一格发 **Blocked**，不是删掉的一格，也不
//! 是拿上面那几格的读数冒充。真装配出来的那份菜单在 `--hotkeys` 那一腿里数得到。
//!
//! 二，第 4 步那句「显示」里那一枚**可以点的**下载入口，自 P179 起住在「关于与更新」那一页的
//! 页脚（`qml/SettingsWindow.qml` 的 `updateRow`，两只手都经过 `crate::session` 那一次现问）；
//! 这一档量的是**递出去的字与地址**（下面 ⑨ 那一格），不是「窗上真画出来了一枚钮、真有人按了
//! 它」。后半那一半由开窗那一族与真人那一次各自作证，不由这一档说成已了。
//!
//! ## 用户那份档
//!
//! 五份 fixture 与两份清单都落在本档自己的目录里，`%APPDATA%\ai-falconshot\config.toml` 一个
//! 字节也不动（§9.1 第十五条）：这一条不只是注释——五枚 `pinned` 各自钉在自己那份 fixture 上，
//! 所以「这一趟改了用户的档」连结构上都不成立。用户档里那两枚 `[about]` 键此刻是什么，只作为
//! 读数印在最后一行，不参与任何判定。

use std::path::{Path, PathBuf};

use falcon_core::config::LoadSource;
use falcon_core::update::{self, Answer, TRAY_WORD};

use crate::hotkey::{tray_labels, tray_routes, Work};
use crate::r13::Report;
use crate::settings::Settings;
use crate::state::Check;

/// 本档自己的目录。
///
/// `FALCONSHOT_UPDATE_DOOR_DIR` 是给跑级档用的：同一台机器上两趟并行时各自的 fixture 得是各自
/// 的文件，否则后写的那一份会替前一趟改题。没有它时退到 pid 之外的固定临时目录，与
/// `tray_click_check` 同一形状。
fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_UPDATE_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-update-door");
    dir
}

/// 包里这一枚版本号。门与产品读的是**同一枚** `env!("CARGO_PKG_VERSION")`，所以这一档不需要
/// 知道产品用的是谁：写死一个「1.0.0」的测试会在那天变成一句假绿。
fn packed() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 比包里那一枚**高**的版本号：把第一段加一。刻意不写死 `99.0.0`——万一将来真到了那一天，
/// 「清单上那枚更新」这一族会整族变成假绿，而这一族是唯一能分出「读了档」与「没读档」的。
fn higher() -> String {
    let mut parts = update::version_parts(&packed()).unwrap_or_default();
    if parts.is_empty() {
        parts.push(0);
    }
    parts[0] += 1;
    parts
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// 一份 fixture：档名、写进档里的那几行、以及这一格要求它读回来成什么样子。
struct Fixture {
    name: &'static str,
    text: String,
    /// 读回来之后那两枚 `[about]` 键该是什么。
    feed: String,
    channel: &'static str,
}

/// 把一枚值写成 TOML 基本字符串。这一步不是讲究：上一版把 `C:\Users\…` 原样塞进双引号，TOML 把
/// `\U` 读成一个坏转义，那三份 fixture 于是整份走 `Recovered`（回落到 `Config::default()`），
/// `release_feed` 变成空的 —— ④⑤⑧ 三格一起读到「还没配发布清单」，而那句本来只该属于 D 那份。
/// 门把判决原话印进 detail，所以红得起来；能红而不必假绿，靠的就是这一行照原样打印。
fn toml_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn store(name: &'static str, feed: &str, channel: &'static str) -> Fixture {
    Fixture {
        name,
        text: format!(
            "[about]\nrelease_feed = {}\nchannel = {}\n",
            toml_string(feed),
            toml_string(channel)
        ),
        feed: feed.to_string(),
        channel,
    }
}

/// A：清单上那枚比包里高，通道 stable —— 第 4 步点名的三样都该在这句话里。
/// B：同一份清单上 stable 那行**比包里高**、beta 那行**与包里同**，而档上写的是 beta —— 这一份是
/// 「跟着档走」那一问的形状：谁要是把通道写死成 stable，这一格会读出 Available 而不是 UpToDate。
/// C：feed 是一枚 URL —— 该在伸手之前先被拒。
/// D：压根没写 release_feed —— 该说「没东西可取」，而不是「取失败」。
/// E：写了本机路径但那一份档不在盘上 —— 该说出它试的是哪一份。
/// 五份 fixture，顺序就是 `measure` 后面写下标的那一份：A  newer / B channel / C url / D empty /
/// E missing。清单路径在这里就落成本档目录里的绝对路径，所以门换目录（并行两趟）时 fixture 的
/// 内容跟着换，不会留着上一次那趟的读数。
fn fixtures(dir: &Path) -> Vec<Fixture> {
    let manifest = |name: &str| dir.join(name).display().to_string();
    vec![
        store("a-newer.toml", &manifest("newer.txt"), "stable"),
        store("b-channel.toml", &manifest("same.txt"), "beta"),
        store("c-url.toml", "https://example.invalid/stable", "stable"),
        Fixture {
            name: "d-empty.toml",
            text: "[about]\nchannel = \"stable\"\n".to_string(),
            feed: String::new(),
            channel: "stable",
        },
        store("e-missing.toml", &manifest("nothing-here.txt"), "stable"),
    ]
}

fn planted(fixture: &Fixture) -> (PathBuf, Settings, String) {
    let path = door_dir().join(fixture.name);
    let seed = match std::fs::write(&path, &fixture.text) {
        Ok(()) => "种下了".to_string(),
        Err(e) => format!("种不下去：{e}"),
    };
    let settings = Settings::open(&path, Some(&path));
    (path, settings, seed)
}

fn source(s: &Settings) -> &'static str {
    match s.source {
        LoadSource::Defaults => "defaults",
        LoadSource::File => "file",
        LoadSource::Recovered => "recovered",
    }
}

pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let dir = door_dir();
    let _ = std::fs::create_dir_all(&dir);
    let pkg = packed();
    let up = higher();

    // 两份清单先落盘。`stable` 与 `beta` 两行**刻意不等**：只有两行不同，才分得出这一趟问的
    // 是档上那一枚通道、还是代码里写死的那一枚。
    let manifest_newer = dir.join("newer.txt");
    let manifest_same = dir.join("same.txt");
    let missing = dir.join("nothing-here.txt");
    let wrote = |path: &PathBuf, text: String| match std::fs::write(path, text) {
        Ok(()) => "种下了",
        Err(_) => "种不下去",
    };
    let w1 = wrote(
        &manifest_newer,
        format!(
            "# 一份比包里新的清单\nchannel=stable version={up} notes=本轮那三样都在 url=https://example.invalid/new\n\
             channel=beta version=0.0.1 notes=旧的 url=\n"
        ),
    );
    let w2 = wrote(
        &manifest_same,
        format!(
            "channel=stable version={up} notes=这一行不该被读到 url=https://example.invalid/wrong\n\
             channel=beta version={pkg} notes= url=\n"
        ),
    );
    let _ = std::fs::remove_file(&missing);
    let made = fixtures(&dir);
    let planted_all: Vec<(PathBuf, Settings, String)> = made.iter().map(planted).collect();

    // ---- ① 后面每一格都要问这一格：五份档得是真从文件里读出来的，而不是解析失败之后那一份
    // `Config::default()`。`feed` 读回的那枚字符串也要对上，因为 `[about] release_feed` 是这一档
    // 唯一的输入：它被 serde 丢掉的那天，下面四格会一起变成「都在说同一句话」的假绿。
    let loaded_ok = planted_all
        .iter()
        .zip(made.iter())
        .all(|((_, s, seed), f)| {
            s.source == LoadSource::File
                && s.backup.is_none()
                && seed == "种下了"
                && s.config.about.release_feed == f.feed
                && s.config.about.channel == f.channel
        });
    rep.row(
        "update fixtures load",
        Check::from(loaded_ok),
        format!(
            "五份 fixture：{}；两份清单各 {w1} / {w2}（稳定那行 version={up}、beta 那行 version={pkg}）；\
             本档目录 {}；用户那份档不在这里、也不被写（五枚 pinned 各钉在自己那份上）",
            planted_all
                .iter()
                .zip(made.iter())
                .map(|((p, s, seed), f)| format!(
                    "{} {}（feed={:?} 通道 {}，{seed}）",
                    p.display(),
                    source(s),
                    f.feed,
                    f.channel
                ))
                .collect::<Vec<_>>()
                .join("；"),
            dir.display()
        ),
    );

    // ---- ② 第 1441 行那一项在不在**装配出来的**那份菜单上。判的是 `tray_menu` 真的把它排进
    // 去了，而不是 `Work::Update` 这一枚枚举存在（与 P171/P172 那两档同一枚理由，#281）。
    let labels = tray_labels();
    let at = labels.iter().position(|name| name == TRAY_WORD);
    rep.row(
        "tray 检查更新",
        Check::from(at.is_some() && labels.last().is_none_or(|n| n != TRAY_WORD)),
        format!(
            "菜单 {} 项：{}（找的是「{TRAY_WORD}」那一枚原名，落在第 {:?} 位；退出仍是最后一项）",
            labels.len(),
            labels.join(" / "),
            at.map(|i| i + 1)
        ),
    );

    // ---- ③ 那一项按下去走的是哪一手。读的是同一只 `tray_menu` 的 `(id, work)` 那一侧。
    let routes = tray_routes();
    let mine = routes.iter().find(|(id, work)| {
        labels
            .get(
                routes
                    .iter()
                    .position(|(r, _)| *r == *id)
                    .unwrap_or(usize::MAX),
            )
            .is_some_and(|name| name == TRAY_WORD)
            && *work == Work::Update
    });
    rep.row(
        "the row routes to Work::Update",
        Check::from(
            mine.is_some()
                && Work::Update.action().is_none()
                && Work::of("update").is_none()
                && routes.iter().filter(|(_, w)| *w == Work::Update).count() == 1,
        ),
        format!(
            "菜单里那一项的 id={:?}；{} 枚工作进了菜单，其中 Work::Update 恰一枚；\
             它没有可绑定动作名（action()={:?}），而 Work::of(\"update\")={:?} —— §5.16.1 那张表\
             没有被替它发明第十三名",
            mine.map(|(id, _)| *id),
            routes.len(),
            Work::Update.action(),
            Work::of("update")
        ),
    );

    // ---- ④ 第 4 步点名的三样。递出去的是**档上读回来的那两枚键**，不是这一档自己写的常量。
    let (path_a, a, _) = &planted_all[0];
    let answer_a = update::check(&a.config.about.release_feed, &a.config.about.channel, &pkg);
    let said_a = answer_a.sentence();
    let three = said_a.contains(&format!("发现新版本 {up}"))
        && said_a.contains("本轮那三样都在")
        && said_a.contains("https://example.invalid/new");
    rep.row(
        "a newer manifest says the three named things",
        Check::from(
            matches!(answer_a, Answer::Available { .. }) && three && answer_a.has_manifest(),
        ),
        format!("{said_a} ← {}", path_a.display()),
    );

    // ---- ⑤ 「跟着档走」那一问：这份档写的是 beta，而同一份清单上 stable 那行明明更新。
    // 通道被写死的话这一格会读出 Available，于是它是这一档里唯一能分出「问了档」与「没问档」的
    // 那一格 —— 所以它判的是**两句都在**：判决是 UpToDate，且 stable 那一枚更高的号就在 detail 里。
    let (path_b, b, _) = &planted_all[1];
    let answer_b = update::check(&b.config.about.release_feed, &b.config.about.channel, &pkg);
    let said_b = answer_b.sentence();
    rep.row(
        "the ask follows the channel in the config",
        Check::from(
            matches!(answer_b, Answer::UpToDate { .. })
                && said_b.contains(&pkg)
                && !said_b.contains(&up),
        ),
        format!(
            "档上通道 {} → {said_b}；同一份清单 stable 那行是 {up}（它没被问到）← {}",
            b.config.about.channel,
            path_b.display()
        ),
    );

    // ---- ⑥⑦⑧ 那三句边界。三句都必须**互不相同**：把「没配」与「取不到」说成同一句，用户就
    // 分不清该去配档还是去看盘；把 URL 读成「文件不存在」是把这一层的边界说成他的盘出了状况。
    let (_, d, _) = &planted_all[3];
    let answer_d = update::check(&d.config.about.release_feed, &d.config.about.channel, &pkg);
    let said_d = answer_d.sentence();
    rep.row(
        "no feed refuses instead of failing",
        Check::from(
            answer_d == Answer::NoFeed
                && said_d.contains("release_feed 是空的")
                && !answer_d.has_manifest(),
        ),
        said_d,
    );

    let (_, c, _) = &planted_all[2];
    let answer_c = update::check(&c.config.about.release_feed, &c.config.about.channel, &pkg);
    let said_c = answer_c.sentence();
    rep.row(
        "a url is refused before it is read",
        Check::from(
            matches!(&answer_c, Answer::RemoteRefused(feed) if feed == "https://example.invalid/stable")
                && said_c.contains("不联网")
                && !said_c.contains("不存在"),
        ),
        said_c,
    );

    let (_, e, _) = &planted_all[4];
    let answer_e = update::check(&e.config.about.release_feed, &e.config.about.channel, &pkg);
    let said_e = answer_e.sentence();
    rep.row(
        "an unreadable manifest names the file it tried",
        Check::from(
            matches!(&answer_e, Answer::Unreadable(why) if why.contains("nothing-here.txt"))
                && !said_e.contains("不联网"),
        ),
        said_e,
    );

    // ---- ⑨ 第 4 步那枚「下载入口」自此不只在句子里：它有一枚**可以点的**钮，而钮归
    // 「关于与更新」那一页页脚。这一格判四件事，全都在没有屏幕的地方问得到：
    // 一，那枚地址是**这次判决自己交回的**（`download_url()`），不是谁把那串字从句子中切出来的；
    // 二，同一次判决里没有入口的各格交回 `None`（⑤ 那句「已经是最新」没有地址可开）；
    // 三，页脚那两枚字只在「关于与更新」那一页递，别页交回空串 —— 号是从名单反查的，不是数的；
    // 四，两枚字都是 core 那两枚常量，本档不另抄一份（#281）。
    let about = crate::settings_view::index_of(crate::settings_view::ABOUT_PAGE);
    let words_line = about.map_or(String::new(), crate::settings_view::update_words);
    let pieces: Vec<&str> = words_line.split(crate::settings_view::RECORD).collect();
    let check_word = pieces.first().copied().unwrap_or_default();
    let down_word = pieces.get(1).copied().unwrap_or_default();
    let other_pages_empty = about
        .map(|i| {
            (0..crate::settings_view::CATEGORIES.len())
                .filter(|&k| k != i)
                .all(|k| crate::settings_view::update_words(k).is_empty())
        })
        .unwrap_or(false);
    rep.row(
        "the page offers a clickable download entry",
        Check::from(
            answer_a.download_url() == Some("https://example.invalid/new")
                && answer_b.download_url().is_none()
                && answer_d.download_url().is_none()
                && check_word == TRAY_WORD
                && down_word == update::DOWNLOAD_WORD
                && down_word.contains("下载入口")
                && other_pages_empty,
        ),
        format!(
            "入口地址 {:?}（同一次判决交回，不从句子切）；页上两枚字 {check_word:?} + {down_word:?} \
             出自 core 那两枚常量，只在「{}」（第 {:?} 类）递，其余 {} 类一律空串；没有入口的那两格 \
             交回 None：{:?} / {:?}",
            answer_a.download_url(),
            crate::settings_view::ABOUT_PAGE,
            about,
            crate::settings_view::CATEGORIES.len() - 1,
            answer_b.download_url(),
            answer_d.download_url()
        ),
    );

    // ---- ⑩ 用户那份档：只作为**盘上读数**印出来，不参与判定，也不打开它（§9.1 第十五条的判据
    // 在级档那头的 before/after 两枚 sha 上，不在这里）。本档自己那五份 fixture 走的是
    // `Settings::open` + pinned，结构上到不了用户那份路径。
    let user_path = falcon_core::config::config_path(&falcon_core::config::Config::default());
    let reading = std::fs::metadata(&user_path).map_or_else(
        |e| format!("读不出（{e}）"),
        |m| format!("{} 字节，改动时间 {:?}", m.len(), m.modified().ok()),
    );
    rep.note(
        "用户那份档此刻",
        format!("{}：{} —— 本档一次也没写过它", user_path.display(), reading),
    );

    // ---- ⑪ 这一档不量过的那件事，落在报告里而不是只落在注释里。
    rep.row(
        "the tray row was really clicked",
        Check::Blocked,
        "这一档没有跑着的泵，也没有往那枚菜单项投过一次真点击；真人点中它走哪一臂，由 `--hotkeys` \
         那一腿的菜单清点与 `hotkey` 那一路的 HISTORY 读数作证，本轮级档里那一腿各跑各的。页脚那两枚\
         钮这一档也只量到**递出去的字与地址**（⑨）：这一趟没有开过那扇窗，也没有拿真手指按过它们，\
         窗上「画出来了吗」与「按下去写了什么」由开窗那一族（`--settings-window`）与真人那一次各自\
         作证。"
            .to_string(),
    );

    rep.finish("update-check door")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `higher()` 不许先旧：它比 `packed()` 高，且 `is_newer` 认这个高。写死一个数的那一格会在
    /// 版本号走到那一天变成假绿，而这一族全部靠它。
    #[test]
    fn the_fixture_version_is_always_above_the_packed_one() {
        assert!(
            update::is_newer(&higher(), &packed()),
            "{} 不比 {} 高",
            higher(),
            packed()
        );
        assert!(!update::is_newer(&packed(), &higher()));
    }

    /// 五份 fixture 的形状：D 不写 feed、C 的 feed 是一枚 URL，其余三档指本档目录。这一格不碰
    /// 盘，只保证 `measure` 拿到的顺序与它后面写的那四枚下标对得上——顺序错了不会红，只会让
    /// 每一格读错档，那是最难发现的一种坏。
    #[test]
    fn the_five_fixtures_keep_their_order() {
        let dir = door_dir();
        let made = fixtures(&dir);
        let names: Vec<&str> = made.iter().map(|f| f.name).collect();
        assert_eq!(
            names,
            vec![
                "a-newer.toml",
                "b-channel.toml",
                "c-url.toml",
                "d-empty.toml",
                "e-missing.toml"
            ]
        );
        assert_eq!(
            made[1].channel, "beta",
            "B 那份必须是 beta，否则「跟着档走」那一问不存在"
        );
        assert!(made[3].feed.is_empty());
        assert!(made[2].feed.contains("://"));
    }

    /// 那三份指本档目录的 fixture，路径里必然带反斜杠，而反斜杠在 TOML 基本字符串里是转义符。
    /// 这一格不碰盘、也不看 `Settings::open` 交回什么，它只看**写出去的那几行字**：每一枚反斜杠
    /// 后面必须还有一枚反斜杠。上一版没有这一格，于是 `\U` 进了档、TOML 拒收、三份 fixture 走
    /// `Recovered`，红的是三格离它很远的判决 —— 这一格把那一条路直接堵住。
    #[test]
    fn a_backslash_in_a_fixture_path_is_never_alone() {
        for fixture in fixtures(&door_dir()) {
            let text = &fixture.text;
            let mut rest = text.as_str();
            while let Some(at) = rest.find('\\') {
                assert!(
                    rest.as_bytes().get(at + 1) == Some(&b'\\'),
                    "{} 里第 {at} 枚反斜杠是单的：{text}",
                    fixture.name
                );
                rest = &rest[at + 2..];
            }
        }
        assert_eq!(toml_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
