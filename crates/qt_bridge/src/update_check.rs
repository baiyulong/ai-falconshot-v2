//! §5.21.1 的那一扇门：托盘那枚「检查更新」按下去到底做什么，以及那句判决里的三样东西
//! （版本号 / 更新说明 / 下载入口）是不是**产品自己写的**那三样。
//!
//! PRD 第 1441 行把「检查更新」列进托盘菜单建议，而 `hotkey::tray_menu` 头顶那封欠账信从
//! P138 起一直记着它没有手（「菜单建议里的 帮助 / 检查更新 两条，本 build 都还没有能跑的
//! 那只手」）。第 1552–1555 行那四步里，第 1、2 步要的是菜单上那一项和它后面的归宿，第 3 步
//! 要的是「系统获取版本信息」，第 4 步要的是那三样内容。这一档逐格问这五件事，外加两件
//! 边界：档上没配清单该说没配，那一份取不回来该说取不回（而不是「找不到文件」）。
//!
//! ## 为什么这一档不开窗、也不等任何人
//!
//! 它读的全是「一份档 → 一个决定」：真往本档目录里种五份 fixture 与两份清单、真走产品那个
//! [`Settings::open`] 读回来（`Config::load` ⇒ `validate` ⇒ 归一，一次也不绕），再把读回来的
//! 那两枚 `[about]` 键递给**产品那一层用的同一只函数** [`falcon_core::update::check`]。于是
//! 锁屏会话也判得出，也不必有人的手放在鼠标上。
//!
//! ## ⑫⑬⑭ 那三行**真的**联网，只联到自己身上
//!
//! 第 3 步那一只手（[`platform_windows::fetch`]，wininet）如今是真的，而「真的那一只手」在
//! 这一档不能拿假货当证人。所以门在本机起一扇只服务自己的 HTTP/1.0 服务器 ——
//! `127.0.0.1:` + 系统给的那枚口，一次请求也没有出过这台机器。三行各量一种形状：一份 200 的
//! 清单（真取回 ⇒ 真判决）、一枚 404（对面答的不是清单），以及一份比
//! [`falcon_core::update::FEED_LIMIT`] 大的东西（超限是**失败**，不是「拿前一段做判决」）。
//!
//! 这三行在没有网络的机器上会红，那是它们本份的一部分：一句「取回成功」在这一档只有对着真
//! socket 才分得出真假。①–⑨ 那几行不碰这一只手，它们走的是下面那枚 `refusing` 的假货 ——
//! 那几行判的是分发（哪一枚键走到哪一只手）与话术（三种失败各说各的话），那些事不需要网络。
//!
//! ## ⑯–㉑ 自动那一支：§5.21.1 那四步**之外**的两枚键
//!
//! `[about] auto_check_updates` 与 `check_every_days` 是本项目自己加的（PRD 那一节只写用户按的
//! 那一次），它们要的读者是「启动那一趟自己起不起检查」。判决住在 core
//! [`falcon_core::update::auto_plan`]、戳住在 [`crate::update_auto`]，这一档把这两样各问一次：
//! 四支读数各自配上它自己的那句为什么（⑯），戳写下去再读回来必须是同一个天数的差（⑰），坏了
//! 的那一行与未来的那一天都不许被读成「今天刚查过」（⑱），点不点戳的两半各归其位（⑲），产品
//! 自己那份戳不是本档可写的东西（⑳），而那一只手确实接在产品启动那一趟上（㉑）。
//!
//! 这一档**不**跑 [`crate::update_auto::startup`] 本体：它一联网，本档其余各行的读数就要看这台
//! 机器此刻通不通 —— 而那一件事正是 ⑫⑬⑭ 用一扇自己的服务器挡掉的。㉑ 那一格因此读的是 `main.rs`
//! 这一处接线本身，与 `hint_check` 那一族扫 `.qml` 与 `session.rs` 同一枚办法。
//!
//! ## 这一档不声称的那三件事
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
//! 三，自动那一支的**启动本体**（[`crate::update_auto::startup`]）本档一次也没跑过：跑了就等于让
//! 上面那些格子依赖这台机器此刻通不通。㉑ 那一格读的是它在 `main.rs` 里的**接线位置**，而「真启动
//! 一次、屏幕上读到那句自动判决」这一腿自此记在这份欠账里，不由 ⑯–㉑ 那六格说成已了。
//!
//! ## 用户那份档
//!
//! 五份 fixture 与两份清单都落在本档自己的目录里，`%APPDATA%\ai-falconshot\config.toml` 一个
//! 字节也不动（§9.1 第十五条）：这一条不只是注释——五枚 `pinned` 各自钉在自己那份 fixture 上，
//! 所以「这一趟改了用户的档」连结构上都不成立。用户档里那两枚 `[about]` 键此刻是什么，只作为
//! 读数印在最后一行，不参与任何判定。

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use falcon_core::config::LoadSource;
use falcon_core::update::{self, Answer, TRAY_WORD};

use crate::hotkey::{tray_labels, tray_routes, Work};
use crate::r13::Report;
use crate::settings::Settings;
use crate::state::Check;

/// ①–⑨ 那几行走的**假的手**。那五份 fixture 配的是盘上清单，一次也不该走到它上面，而 C 那份
/// 是 URL —— 它要的读数是「这一句被交给了取回那一只手」与「取不回来的那句原话」，不是「这一档
/// 真去连了 `example.invalid`」：那一句要靠 DNS 与一台不存在的服务器，门在那一天会红得没有理由。
///
/// 递进来的那一句被记在 `asked` 里。**这一枚记录就是那一格的证人**：地址那一支若被当成路径
/// 分发，`asked` 会是空的而判决会是 `Unreadable`，两者在这一档读得开。
fn refusing(
    asked: &RefCell<Option<String>>,
) -> impl Fn(&str) -> Result<String, String> + '_ {
    move |url: &str| {
        asked.borrow_mut().replace(url.to_string());
        Err("这一档没有真取回：递给假的手".to_string())
    }
}

/// 门在本机起的那一扇服务器。它只听 `127.0.0.1`（**不是** `0.0.0.0`：那一枚口不该让这台机器
/// 以外的东西连得上，一次请求也不该出过这台机器），端口交给系统（`:0`）所以并行两趟不抢同一枚。
struct Door {
    port: u16,
    stop: Arc<AtomicBool>,
}

impl Door {
    /// 起。返回的端口用来拼那句 `http://127.0.0.1:{port}/…`，而那三种形状由路径决定。
    /// 起不动就交回 `None`：门把这一件事照实印出来，不假装量过（也不 panice —— 锁屏的机器上
    ///  bind 失败是一次「取不到」，不是一个崩溃）。
    fn start() -> Option<Door> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let up = higher();
        let spawned = std::thread::Builder::new()
            .name("falconshot-update-door".to_string())
            .spawn(move || {
                // 监听口设成非阻塞，那条线程才能在每次循环里看一眼「门要不要我停」——
                // 阻塞在 `accept` 上的线程是不肯放的，而这一档每次都该把自己起的那只手关干净。
                let _ = listener.set_nonblocking(true);
                let manifest = format!(
                    "channel=stable version={up} notes=真取回的那一份 url=https://example.invalid/real\n"
                );
                while !flag.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => serve(stream, &manifest),
                        Err(_) => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
            });
        spawned.ok().map(|_| Door { port, stop })
    }

    /// 那一句要递给 [`falcon_core::update::check`] 的地址。
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, path)
    }
}

impl Drop for Door {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// 一次**真**请求的读数：递出去的那一句、产品交回的那句话、以及那一枚判决。
/// 三个名字而不是 `(String, String, Answer)`：⑫⑬⑭ 那一族读的是 `answer`，写成一串下标
/// 就会有人把 `.1` 与 `.2` 用反，而那是「句子对上了、判决没对上」那种最难看的假绿。
struct Ask {
    url: String,
    said: String,
    answer: Answer,
}

/// 一份「头 + 一整块体」的答。写完就把写的那一半关掉：`HTTP/1.0` + `Connection: close` 且不写
/// `Content-Length` ⇒ 体到连接尽头为止，wininet 认这套。
fn answer(stream: &mut TcpStream, status: &str, body: &[u8]) {
    let _ = head(stream, status);
    let _ = stream.write_all(body);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// 只写头，交回「写进去了没有」。超限那一格要这一枚而不是 [`answer`]：它得在头之后一段一段地写，
/// 而那里体是一整块。
fn head(stream: &mut TcpStream, status: &str) -> std::io::Result<()> {
    stream.write_all(
        format!(
            "HTTP/1.0 {status}\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Connection: close\r\n\r\n"
        )
        .as_bytes(),
    )
}

/// 一次请求、一个答。这一扇服务器不懂 HTTP，它只看那第一行里的路径是哪一条，答完就把连接关掉。
fn serve(mut stream: TcpStream, manifest: &str) {
    // `accept` 交回来的连接**继承**监听口的非阻塞档，所以这里第一件事是把它换回阻塞：
    // 非阻塞的读会在内核还没把那一行送完时就交回 WouldBlock，那一趟于是被读成「那边没给状态码」。
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut got = Vec::new();
    let mut byte = [0u8; 1];
    while got.len() < 4096 {
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            break;
        }
        got.push(byte[0]);
        if got.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let path = String::from_utf8_lossy(&got)
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    match path.as_str() {
        "/new.txt" => answer(&mut stream, "200 OK", manifest.as_bytes()),
        // 超限那一格要的是**比上限还大**的一份：逐段写，写到超过 [`update::FEED_LIMIT`] 为止。
        // 拿一枚固定的大小写死在这里，改一天 FEED_LIMIT 调小，这一格会悄悄变成「取回成功」。
        "/big.txt" => {
            let piece = "x".repeat(64 * 1024);
            let mut sent = 0usize;
            let mut ok = head(&mut stream, "200 OK").is_ok();
            while ok && sent <= update::FEED_LIMIT {
                ok = stream.write_all(piece.as_bytes()).is_ok();
                sent += piece.len();
            }
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
        _ => answer(
            &mut stream,
            "404 Not Found",
            b"there is no manifest at this path\n",
        ),
    }
}

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
/// C：feed 是一枚 URL —— 该被交给取回那一只手，而失败那句说的是那一趟，不是用户的盘。
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
    // 顺手量的第二件事是分发：这一份 feed 是盘上路径，那只假的手**一次也不该被问到** —— 它一旦被
    // 问到，说明地址/路径那一道分流在这一层就走错了，而⑦那一格只能量出反方向。
    let (path_a, a, _) = &planted_all[0];
    let asked_a = RefCell::new(None);
    let answer_a = update::check(
        &a.config.about.release_feed,
        &a.config.about.channel,
        &pkg,
        &refusing(&asked_a),
    );
    let said_a = answer_a.sentence();
    let three = said_a.contains(&format!("发现新版本 {up}"))
        && said_a.contains("本轮那三样都在")
        && said_a.contains("https://example.invalid/new");
    rep.row(
        "a newer manifest says the three named things",
        Check::from(
            matches!(answer_a, Answer::Available { .. }) && three && answer_a.has_manifest()
                && asked_a.borrow().is_none(),
        ),
        format!(
            "{said_a} ← {}；那一只假的手一次也没被问到（{:?}）—— 盘上路径走的是盘",
            path_a.display(),
            *asked_a.borrow()
        ),
    );

    // ---- ⑤ 「跟着档走」那一问：这份档写的是 beta，而同一份清单上 stable 那行明明更新。
    // 通道被写死的话这一格会读出 Available，于是它是这一档里唯一能分出「问了档」与「没问档」的
    // 那一格 —— 所以它判的是**两句都在**：判决是 UpToDate，且 stable 那一枚更高的号就在 detail 里。
    let (path_b, b, _) = &planted_all[1];
    let answer_b = update::check(
        &b.config.about.release_feed,
        &b.config.about.channel,
        &pkg,
        &refusing(&RefCell::new(None)),
    );
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
    // 分不清该去配档还是去看盘；把一枚地址读成「文件不存在」是把这一层的边界说成他的盘出了状况。
    let (_, d, _) = &planted_all[3];
    let answer_d = update::check(
        &d.config.about.release_feed,
        &d.config.about.channel,
        &pkg,
        &refusing(&RefCell::new(None)),
    );
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

    // ---- ⑦ C 那份是这一档里唯一一枚**地址**。它要的三个读数：这一句确实被交给了取回那一只手
    // （`asked_c` 记的就是那一句，一字未改），判决说的是「这一趟没取回来」而不是「盘上找不到」，
    // 而失败的那半句原话是那一只手交回的、core 一个字也没另编（#281）。
    // 这一格**不**去联 `example.invalid`：门要的读数是分发与话术，那两件事不需要一台真的服务器，
    // 而真联网那三行是下面的 ⑫⑬⑭⑮。
    let (path_c, c, _) = &planted_all[2];
    let asked_c = RefCell::new(None);
    let answer_c = update::check(
        &c.config.about.release_feed,
        &c.config.about.channel,
        &pkg,
        &refusing(&asked_c),
    );
    let said_c = answer_c.sentence();
    rep.row(
        "a url goes to the fetch hand and keeps its words",
        Check::from(
            matches!(&answer_c, Answer::FetchFailed { feed, why } if feed.as_str() == c.config.about.release_feed.as_str() && why.contains("递给假的手"))
                && asked_c.borrow().as_deref() == Some(c.config.about.release_feed.as_str())
                && !said_c.contains("找不到")
                && !said_c.contains("读不出来")
                && !answer_c.has_manifest(),
        ),
        format!(
            "递进那只手的那一句 = {:?}；{said_c} ← {}",
            *asked_c.borrow(),
            path_c.display()
        ),
    );

    let (_, e, _) = &planted_all[4];
    let answer_e = update::check(
        &e.config.about.release_feed,
        &e.config.about.channel,
        &pkg,
        &refusing(&RefCell::new(None)),
    );
    let said_e = answer_e.sentence();
    rep.row(
        "an unreadable manifest names the file it tried",
        Check::from(
            matches!(&answer_e, Answer::Unreadable(why) if why.contains("nothing-here.txt"))
                && !said_e.contains("没取回来")
                && !said_e.contains("网络"),
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

    // ---- ⑫⑬⑭⑮ 那四行**真的**联网，只联到自己身上：门在本机 `127.0.0.1` 上起的那一扇服务器，
    // 递进去的是产品自己那一只手 [`crate::session::fetch_feed`]（wininet），不是这一档另抄的一份
    // （#281）。三行各量一种形状：200 的一份清单（真取回 ⇒ 真判决）、一枚 404（对面答的不是清单），
    // 以及一份比 [`update::FEED_LIMIT`] 大的东西。第四行量的是**话**：那一句「这一趟要向谁取」在
    // 地址那一支说得出、在盘上路径那一支一个字也不说（§6.3）。
    let cap = update::FEED_LIMIT.to_string();
    let door = Door::start();
    let asked = door.as_ref().map(|door| {
        let ask = |path: &str| {
            let url = door.url(path);
            let answer = update::check(&url, "stable", &pkg, &crate::session::fetch_feed);
            Ask {
                said: answer.sentence(),
                url,
                answer,
            }
        };
        (ask("new.txt"), ask("gone.txt"), ask("big.txt"))
    });

    match &asked {
        None => {
            // bind 不动 = 这一趟拿不到对面。门把这三行发 **Blocked** 并把原因印出来：一句
            // 「取回成功」在没有真 socket 的机器上是不可以绿的（#261），而把它删掉会让这一族
            // 看上去量过了。⑮ 那一行不受这一支影响（它只分地址与路径的形，不需要对面）。
            let why = "这一趟起不动那一扇本机服务器（`TcpListener::bind(\"127.0.0.1:0\")` 没成交），\
                       真取回的那三行没有对面 —— 发 Blocked，不拿假货当证人。"
                .to_string();
            for name in [
                "a real fetch of a real manifest reaches a real verdict",
                "a refusal from the far end is told as itself, not as a missing file",
                "a manifest bigger than the cap fails instead of deciding on half of it",
            ] {
                rep.row(name, Check::Blocked, why.clone());
            }
        }
        Some((ok, refused, oversize)) => {
            // ---- ⑫ 200 那一支。判决必须落在**服务器写的那一份**上：那枚比包里高的号、那句
            // 「真取回的那一份」、那一枚 `…/real` 的地址，三样都只可能来自 socket 的另一头 ——
            // 本档目录里那份 newer.txt 写的是「本轮那三样都在」与 `…/new`，走错了门一眼就读得开。
            rep.row(
                "a real fetch of a real manifest reaches a real verdict",
                Check::from(
                    matches!(ok.answer, Answer::Available { .. })
                        && ok.said.contains(&format!("发现新版本 {up}"))
                        && ok.said.contains("真取回的那一份")
                        && ok.said.contains("https://example.invalid/real")
                        && !ok.said.contains("本轮那三样都在")
                        && ok.answer.download_url() == Some("https://example.invalid/real"),
                ),
                format!("{} → {}", ok.url, ok.said),
            );

            // ---- ⑬ 对面答 404。要的是**那句里带那枚数**，且它不是一张盘上找不到的脸：
            // 「读不出来（…）」那一支说的是用户的硬盘，这一支说的是那台服务器，两件事在用户耳朵里
            // 是两个方向（#453）。404 的那一份体（`there is no manifest…`）一个字也不许进判决 ——
            // 那才是「非 2xx 也被当成清单去 parse」那条路的标本。
            rep.row(
                "a refusal from the far end is told as itself, not as a missing file",
                Check::from(
                    matches!(&refused.answer, Answer::FetchFailed { feed, why } if feed == &refused.url && why.contains("404"))
                        && refused.said.contains("状态码 404")
                        && !refused.said.contains("读不出来")
                        && !refused.said.contains("no manifest")
                        && !refused.answer.has_manifest(),
                ),
                format!("{} → {}", refused.url, refused.said),
            );

            // ---- ⑭ 比上限大的那一份。判决那句里必须有**那一枚上限的数**：它是「本程序不许这么
            // 大」而不是「那边不给」，而那句里的数是 core 交下来的（`FEED_LIMIT` 走的是参数，不是
            // 这一档写的常量 —— 改小它，这一格跟着改，不会悄悄变成「取回成功」）。
            // 「拿前一段做判决」在这条路上是**读不出来**的：一份被剪尾巴的清单会读出半个通道。
            rep.row(
                "a manifest bigger than the cap fails instead of deciding on half of it",
                Check::from(
                    matches!(&oversize.answer, Answer::FetchFailed { feed, why } if feed == &oversize.url && why.contains(cap.as_str()))
                        && oversize.said.contains("一半")
                        && !matches!(oversize.answer, Answer::Available { .. } | Answer::UpToDate { .. })
                        && !oversize.answer.has_manifest(),
                ),
                format!(
                    "{}（对面写了超过 {} 字节）→ {}",
                    oversize.url,
                    update::FEED_LIMIT,
                    oversize.said
                ),
            );
        }
    }

    // ---- ⑮ §6.3 那句「应明确提示网络请求」到底有没有说、有没有多说。这一行只分**形状**，
    // 所以它不依赖那一扇服务器起没起；但它用的那一句地址就是 ⑫ 真取过的那一句（门起着时），
    // 于是「承诺要联网的那一句」与「真联了网的那一句」是同一串字，不是两串长得像的字。
    let net_url = door
        .as_ref()
        .map(|d| d.url("new.txt"))
        .unwrap_or_else(|| "http://127.0.0.1:9/new.txt".to_string());
    let said_net = update::running_line(&net_url);
    let said_disk = update::running_line(&manifest_newer.display().to_string());
    let disk_feed = planted_all[3]
        .1
        .config
        .about
        .release_feed
        .clone();
    rep.row(
        "the promise says a request only where a request happens",
        Check::from(
            said_net.contains("网络请求")
                && said_net.contains("127.0.0.1")
                && said_net.starts_with(update::RUNNING)
                && said_disk == update::RUNNING
                && !said_disk.contains("网络")
                && update::running_line(&disk_feed) == update::RUNNING,
        ),
        format!(
            "地址那支（{net_url}，{}）：{said_net} ／ 盘上那支：{said_disk} ／ 空那支：{}",
            if door.is_some() {
                "⑫ 刚才真从这一句取回过"
            } else {
                "这一档自己写的形状，不冒充取过"
            },
            update::running_line(&disk_feed),
        ),
    );

    // ─────────────────────────────── ⑯–㉑ 自动那一支（§5.21.1 之外的两枚键）

    // 本档那枚戳目录：与上面五份 fixture 同一枚根，另起一支子目录，为的是让 ⑳ 那一问
    // （「产品自己那份 `last_auto_check.txt` 这一趟动过没有」）有一个明显不同的路径可比。
    let stamp_dir = dir.join("auto-stamp");
    let stamp_file = stamp_dir.join(crate::update_auto::STAMP_FILE);
    let _ = std::fs::create_dir_all(&stamp_dir);
    let home = crate::update_auto::dir().join(crate::update_auto::STAMP_FILE);
    let home_scan = || {
        std::fs::metadata(&home)
            .ok()
            .map(|m| (m.len(), m.modified().ok()))
    };
    let home_before = home_scan();

    // 递进 `plan` 的「今天」由本档写死一枚（20 000 天前后那一枚数），不拿这台机器的钟：否则
    // ⑰⑱ 那两格的读数会跟着日期走，而 ⑯ 的四支要的是**同一枚**今天下四种档的形状。
    let day = 20_000u64;
    let put = |stamped: Option<u64>| match stamped {
        Some(d) => {
            let _ = std::fs::write(&stamp_file, format!("{d}\n"));
        }
        None => {
            let _ = std::fs::remove_file(&stamp_file);
        }
    };

    // ---- ⑯ 四支读数各问一次，每一支都把「起不起」（`wanted`）与「那句为什么」（`why`）一起判。
    // 只读句子的那一版判不出「跳过那一支真跳过了没有」，而这一族唯一的坏法就是做的事与说的话
    // 分家（#281）；四句还须各不同（#453），两句一样的话就等于少一支读数。
    let mut readings: Vec<String> = Vec::new();
    let mut whys: Vec<&'static str> = Vec::new();
    let mut agrees = true;
    for (auto, every, stamped, now, want, phrase) in [
        (false, 7, Some(day), day, false, "自动检查关了"),
        (true, 7, None, day, true, "没有上次自动检查的记录"),
        (true, 7, Some(day), day + 7, true, "够 about.check_every_days 那枚数"),
        (true, 7, Some(day), day + 3, false, "这一趟跳过"),
    ] {
        put(stamped);
        let plan = crate::update_auto::plan(auto, every, &stamp_dir, Some(now));
        agrees &= plan.wanted == want && plan.why.contains(phrase);
        if !whys.contains(&plan.why) {
            whys.push(plan.why);
        }
        readings.push(format!(
            "auto={auto} every={every} 戳={:?} 今天={now} → {}：{}",
            stamped,
            if plan.wanted { "起" } else { "跳过" },
            plan.why
        ));
    }
    rep.row(
        "the two auto keys have a reader that counts days",
        Check::from(agrees && whys.len() == 4),
        format!(
            "{} —— 四句为什么各不同（{} 枚）。递进去的『今天』是本档写死的 {day}，不是这台机器的钟。",
            readings.join("；"),
            whys.len()
        ),
    );

    // ---- ⑰ 门自己写一枚戳，再让下一趟读它：写下去的那一枚数与读回来的那个差，必须是同一天数
    // 的差。这一格钉的是「点戳的那一手」与「读戳的那一手」对得上号 —— 一边写时刻、一边读天数，
    // 够数那一支会永远不够，而屏幕上与日志里都不会露出任何形状。
    put(None);
    let wrote = crate::update_auto::mark(&stamp_dir, Some(day));
    let (wrote_ok, wrote_said) = match &wrote {
        Ok(where_) => (true, where_.clone()),
        Err(why) => (false, format!("没写成：{why}")),
    };
    let gap = crate::update_auto::last_days(&stamp_dir, Some(day + 9));
    let on_disk = std::fs::read_to_string(&stamp_file).unwrap_or_default();
    rep.row(
        "the stamp the auto run leaves is the one the next run reads",
        Check::from(wrote_ok && gap == Some(9) && on_disk.trim() == day.to_string()),
        format!(
            "写下第 {day} 天（{wrote_said}）→ 盘上 {:?} → 第 {} 天读回来是 {:?} 天以前",
            on_disk,
            day + 9,
            gap
        ),
    );

    // ---- ⑱ 那一行读不出天数、与戳上的那一天比今天还晚（钟被往回拨过、或者上一趟写的是别的
    // 机器的天），两支都必须被读成「没有记录」，而不是 `0` 也不是「今天刚查过」。读成 0 的那一版
    // 从此每一次启动都跳过，而这件事在任何屏幕上都看不见 —— 静默一整族更新正是这一支最坏的坏。
    let mut junk: Vec<String> = Vec::new();
    let mut junk_all_none = true;
    for line in ["", "   \n", "not-a-day", "-1", "20000.5"] {
        let _ = std::fs::write(&stamp_file, line);
        let read = crate::update_auto::last_days(&stamp_dir, Some(day));
        junk_all_none &= read.is_none();
        junk.push(format!("{line:?}→{read:?}"));
    }
    put(Some(day + 5));
    let future = crate::update_auto::last_days(&stamp_dir, Some(day));
    let future_plan = crate::update_auto::plan(true, 7, &stamp_dir, Some(day));
    rep.row(
        "an unreadable stamp is never read as today",
        Check::from(
            junk_all_none
                && future.is_none()
                && future_plan.wanted
                && future_plan.why.contains("没有上次自动检查的记录"),
        ),
        format!(
            "{}；未来的那一枚（戳=第 {} 天，今天={day}）读成 {:?} → {}",
            junk.join(" "),
            day + 5,
            future,
            future_plan.why
        ),
    );

    // ---- ⑲ 点不点戳的两半各归其位。入口那一半：把泵**能递到的每一枚**工作的 tag 都问一遍，
    // 再加上页上那枚「页」，交回真的必须只有自动那一枚。结果那一半拿这一档已经算出来的判决问：
    // ⑫/④ 真取回的那一份（Available）点，⑦ 那一次没取回（FetchFailed）不点，⑤ 没配清单（NoFeed）
    // 点 —— 没配清单不点的那一版，会让一台压根没配 `release_feed` 的机器每次启动都重问一遍。
    let tags: Vec<String> = routes
        .iter()
        .map(|(_, w)| w.tag())
        .chain(std::iter::once("页".to_string()))
        .collect();
    let entries_ok = tags.iter().all(|t| !crate::update_auto::stamped_by(t))
        && crate::update_auto::stamped_by(crate::update_auto::AUTO_TAG);
    let results_ok = crate::update_auto::should_stamp(crate::update_auto::AUTO_TAG, &answer_a)
        && crate::update_auto::should_stamp(crate::update_auto::AUTO_TAG, &answer_d)
        && !crate::update_auto::should_stamp(crate::update_auto::AUTO_TAG, &answer_c)
        && !crate::update_auto::should_stamp("页", &answer_a);
    rep.row(
        "only the auto entry consumes the interval, and only after a finished ask",
        Check::from(entries_ok && results_ok),
        format!(
            "{} 枚入口的 tag（托盘/热键那一路全部 + 页）里没有「{}」一枚，故没有一枚会点戳；自动那一枚会点。\
             结果那一半：Available→点 / NoFeed→点 / FetchFailed→不点（下一次启动再问一遍）/ 手动那一支带着\
             最成的判决也不点。判决各是：{:?} / {:?} / {:?}",
            tags.len(),
            crate::update_auto::AUTO_TAG,
            answer_a,
            answer_d,
            answer_c
        ),
    );

    // ---- ⑳ 产品自己那份戳不是本档可写的东西。`mark` 的落点是**递给它的那枚目录**，而这一趟递给
    // 它的从来不是 `update_auto::dir()` 那一枚：于是数据目录里那份文件在本档前后是同一枚读数
    // （在不在、多少字节、改动时间）。这一格也是 §9.1 第十五条在这一族的形状——不是「我们小心」，
    // 是写完之后再问一次盘上有没有变。顺带问落点认不认 `FALCONSHOT_UPDATE_STAMP_DIR`：认，门才有
    // 自己的那一份可写。
    let home_after = home_scan();
    let env_stamp = std::env::var_os(crate::update_auto::ENV_DIR);
    let data_dir = crate::settings::with(|s| falcon_core::config::data_dir(&s.config));
    let follows = match &env_stamp {
        Some(d) if !d.is_empty() => crate::update_auto::dir() == PathBuf::from(d),
        _ => crate::update_auto::dir() == data_dir,
    };
    rep.row(
        "this door never writes the product's own stamp",
        Check::from(follows && home_before == home_after),
        format!(
            "{}={:?} → 落点 {:?}（数据目录 {:?}，{} 那一支）；本档往自动戳伸过 {} 次手，每一次递的都是\
             自己的目录 {:?}；产品那份 {} 前后 {:?} → {:?}。env 那一支的优先次序另有单测（`dir_in` 两支\
             各问一次），本趟在什么环境下就是什么读数，不假装量过另一支。",
            crate::update_auto::ENV_DIR,
            env_stamp,
            crate::update_auto::dir(),
            data_dir,
            if env_stamp.as_ref().is_some_and(|d| !d.is_empty()) {
                "env"
            } else {
                "数据目录"
            },
            crate::update_auto::attempts(),
            stamp_dir,
            home.display(),
            home_before,
            home_after
        ),
    );

    // ---- ㉑ 那一支自动**住在产品那一趟里**：`main.rs` 只在一处叫它，那一处紧贴着 `if plain {`，
    // 排在 `engine.load` 与 `hotkey::install` 之后（GUI 线程句柄要到那一刻才可能到手）、排在
    // `app.exec()` 之前（判决投进的是那个环）。这一格读的是源码本身：接线写错的地方（比如把
    // 自动那一支也放进度具档）在编译上全都是合法的，只有读源码问得到。数的是**原文子串**，所以
    // `main.rs` 的注释里把那枚名字带括号写一遍，这一格就报两处 —— 那是这一格的本份，不是毛病。
    const MAIN_SRC: &str = include_str!("main.rs");
    const WIRED: &str = "if plain {\n        let said = update_auto::startup();";
    let calls = MAIN_SRC.matches("update_auto::startup()").count();
    let at = MAIN_SRC.find("update_auto::startup()");
    let ahead = at.map_or(String::new(), |i| MAIN_SRC[..i].to_string());
    let behind = at.map_or(String::new(), |i| MAIN_SRC[i..].to_string());
    rep.row(
        "the auto arm runs in the product startup only, inside the event loop",
        Check::from(
            calls == 1
                && at.is_some()
                && MAIN_SRC.contains(WIRED)
                && ahead.contains("engine.load(")
                && ahead.contains("hotkey::install(")
                && behind.contains("app.exec()"),
        ),
        format!(
            "{} 处 `update_auto::startup()`；那一处前面有 engine.load 与 hotkey::install（{}／{}），\
             后面有 app.exec（{}）；接线原文按 `if plain {{ … }}` 那一形读过：{}",
            calls,
            ahead.contains("engine.load("),
            ahead.contains("hotkey::install("),
            behind.contains("app.exec()"),
            MAIN_SRC.contains(WIRED)
        ),
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
