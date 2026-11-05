//! §5.8.4 方式二「用户将网页图片拖到已存在的贴图窗口」的那一扇门。
//!
//! PRD §5.8.4 那一段（方式二那一支的三步与其下的三条规则）给了三步（拖到**已存在**的贴图窗 → 取到原图或可用图像数据 → 更新当前贴图
//! 或创建新贴图，「具体行为由设置决定」）与三条规则（网络不可用要提示、获取失败**不得**用低质量
//! 占位图静默替代、遵守目标网站与浏览器可提供的数据范围）。这一档逐格问这三步与那三条规则。
//!
//! ## 为什么这一档不开窗、也不等任何人
//!
//! 它读的是「一串字 → 一堆判据」：归堆、上限、落点表、换来源时要跟着走的每一格、取回那一只手的
//! 三种失败、闸的三枚答、页上那一枚开关。除最后一格之外，每一格都不需要屏幕上真有一扇贴图窗 ——
//! 而**恰恰是这一件**让它能在锁屏的机器上判得出（§9.1 第十条）。
//!
//! ## ⑧⑨⑩⑪ 那四行**真的**联网，只联到自己身上
//!
//! PRD 那一句「获取失败时，不得用低质量占位图静默替代」要的证人不是假货：门在本机起一扇只听
//! `127.0.0.1`（**不是** `0.0.0.0`）的 HTTP/1.0 服务器，端口交给系统，一次请求也没出过这台机器。
//! 四种形状各答一次：一份真的 PNG（200，体是 [`falcon_core::encode`] 现编的）、一枚 404、一份比
//! 递进去的上限还大的体、以及一份 200 但不是图片的体。**四种坏各红各的**（#453）：把 404 与超限
//! 挤成一句「取不回」，用户就分不出该换地址还是该压小图片。
//!
//! 这四行在没有网络栈的机器上会红或发 Blocked，那是它们本份的一部分 —— 一句「取回成功」在这一档
//! 只有对着真 socket 才分得出真假（#261）。
//!
//! ## 这一档不声称的那两件事
//!
//! 一，它一次也没有真把一张图片拖到过一扇真的贴图窗上：落下那一下要一次真的 OS 拖放事件、要鼠标
//! 按住一张图从一个浏览器窗口挪过来，而这一档跑在 `engine.load()` 之前。下面 ⑲ 那一格发
//! **Blocked**，不是删掉的一格，也不拿 ①–⑰ 的读数冒充。
//!
//! 二，它也没跑过 [`crate::drop_in::start`] 本体：那一手的第一枚参数是 `Pin<&mut Session>`（写
//! `session.status` 的那一枚把手），只有 QML 线程拿得到。所以这一档量的是它**调用的那三手**
//! （分格、判类、那句联网提示）与它**交给的那两手**（`state::land_drop`、`falcon_core::drop`），
//! 而不是它自己。两件事都写在 ⑲ 那一句里，不分开说成已了。
//!
//! ## 用户那份档
//!
//! fixture 图片、两份本机 config 都落在本档自己的目录里，`%APPDATA%` 那份 config 一个字节也不动
//! （§9.1 第十五条）：这一条不只是注释 —— 判「那一枚开关读得对不对」用的是本档自己写的那两份
//! fixture，递给 [`crate::settings::Settings::open`] 的是本档目录里的路径。用户那份档此刻那枚键是
//! 什么，只作为读数印在最后，不参与任何判定。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use falcon_core::config::Config;
use falcon_core::drop::{self, Dropped, Landing};
use falcon_core::encode::{self, EncodeOptions, Format};
use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect};
use falcon_core::link::Web;
use falcon_core::pin::{FILE_PIN_MAX, FILE_PIN_SAID_MAX};
use falcon_core::update;

use crate::r13::Report;
use crate::settings::Settings;
use crate::settings_view;
use crate::state::{self, Arrived, Arming, Check, Decision};

/// ⑪ 那一格用来撞「不是图片」的体：一段 plainly 不是任何图片格式的字。
const NOT_AN_IMAGE: &[u8] = b"this is a plain text body, not a picture of any kind\n";
/// ⑩ 超限那一格的体。**刻意不是** [`drop::IMAGE_LIMIT`]：那一个是产品的一格（32 MiB），门为了撞
/// 一次超限写那么大的体，每次跑都要往 loopback 上压 32 MiB，而那一格真正判的是「**递进去的那枚
/// 上限**到了就失败、且不交回一半」。所以这里递进一小枚上限、体写得比它大，判决的形状一个字不差。
const OVER_BODY: usize = 256 * 1024;
/// 上面那一格递进去的上限。它必须**小于** [`drop::IMAGE_LIMIT`]，否则这一档量的就不是超限那一支
/// （见下面的 `the_oversize_probe_sits_below_the_products_own_ceiling`）。
const PROBE_LIMIT: usize = 64 * 1024;

/// 门在本机起的那一扇服务器。四种形状由路径决定，一次请求也不出这台机器。
struct Door {
    port: u16,
    stop: Arc<AtomicBool>,
}

impl Door {
    /// 起。起不动交回 `None`：那一趟 ⑧⑨⑩⑪ 发 Blocked 并印出原因，不拿假货当证人，也不 panic ——
    /// bind 失败在一台机器上是一次「取不到」，不是一个崩溃。
    fn start(picture: Vec<u8>) -> Option<Door> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("falconshot-drop-door".to_string())
            .spawn(move || {
                // 非阻塞的监听口：那条线程要在每次循环里看一眼「门要不要我停」，阻塞在 `accept`
                // 上的线程是不肯放的。
                let _ = listener.set_nonblocking(true);
                while !flag.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => serve(stream, &picture),
                        Err(_) => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
            });
        spawned.ok().map(|_| Door { port, stop })
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, path)
    }
}

impl Drop for Door {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// 只写头，交回「写进去了没有」。超限那一格要在头之后一段一段地写。
fn head(stream: &mut TcpStream, status: &str, kind: &str) -> std::io::Result<()> {
    stream.write_all(
        format!("HTTP/1.0 {status}\r\nContent-Type: {kind}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
}

/// 一次请求、一个答。这一扇服务器不懂 HTTP，它只看那第一行里的路径是哪一条，答完就把连接关掉。
fn serve(mut stream: TcpStream, picture: &[u8]) {
    // `accept` 交回来的连接**继承**监听口的非阻塞档，所以第一件事是把它换回阻塞：非阻塞的读会在
    // 内核还没把那一行送完时就交回 WouldBlock，那一趟于是被读成「那边没给路径」。
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
        "/pic.png" => {
            let _ = head(&mut stream, "200 OK", "image/png");
            let _ = stream.write_all(picture);
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
        "/text.txt" => {
            let _ = head(&mut stream, "200 OK", "text/plain; charset=utf-8");
            let _ = stream.write_all(NOT_AN_IMAGE);
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
        "/big.png" => {
            let piece = vec![0u8; 64 * 1024];
            let mut sent = 0usize;
            let mut ok = head(&mut stream, "200 OK", "image/png").is_ok();
            while ok && sent < OVER_BODY {
                ok = stream.write_all(&piece).is_ok();
                sent += piece.len();
            }
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
        _ => {
            let _ = head(&mut stream, "404 Not Found", "text/plain; charset=utf-8");
            let _ = stream.write_all(b"there is no picture at this path\n");
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    }
}

/// 本档自己的目录。`FALCONSHOT_DROP_DOOR_DIR` 是给跑级档用的：同一台机器上两趟并行时各自的图片
/// fixture 得是各自的文件。没有它时退到固定临时目录，与 `update_check::door_dir` 同一形状。
fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_DROP_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-drop-door");
    dir
}

/// 现编一张真实图片落进本档目录，交回 (路径, 字节, 宽, 高)。
///
/// 为什么由 `encode` 现编而不是放一份写死的字节：这一档判的是「盘上那一份/取回那一份**译得出
/// 多大一张图**」，而写死的字节会在哪天 `encode` 换实现时对不上；现编的这一份永远是自己写的。
fn plant_picture(dir: &Path, name: &str, w: u32, h: u32, tint: [u8; 4]) -> Option<(PathBuf, Vec<u8>)> {
    let frame = Frame::filled(w, h, tint).ok()?;
    let bytes = encode::encode(
        &frame,
        &EncodeOptions {
            format: Format::Png,
            ..Default::default()
        },
    )
    .ok()?;
    let path = dir.join(name);
    if std::fs::write(&path, &bytes).is_err() {
        return None;
    }
    Some((path, bytes))
}

/// 本档自己的一份 config：只写 `[pin] drop_replaces_current` 那一枚键。
///
/// 为什么不走用户那一份：那一枚开关的默认值与「档上写着 true 就读成 true」两件事，必须在门能
/// 说清自己写了什么的那份档上问（§9.1 第十五条），而进程手里那一份 `SETTINGS` 是 `OnceLock` ——
/// 它**换不掉**，所以「现读」那一半由 ⑮ 那格的源码形状与读数一起证，不是由这里改档再重启。
fn store_switch(dir: &Path, name: &str, value: &str) -> Option<Config> {
    let path = dir.join(name);
    std::fs::write(&path, format!("[pin]\ndrop_replaces_current = {value}\n")).ok()?;
    Some(Settings::open(&path, Some(&path)).config)
}

/// `--drop-in` 报告里那一句「这一档不量过的事」。单独一只手写，是因为它同时是 ⑲ 那一格的正文与
/// 这份欠账的落笔处（#281：同一句欠账在两处各写一遍，总有一天一处新过一处）。
fn the_unmeasured_leg() -> String {
    "这一档没有跑过 [`crate::drop_in::start`] 本体，也没有真把一张图片拖到过一扇贴图窗上：前者要的 \
     `Pin<&mut Session>` 只有 QML 线程拿得到（`session.status` 那一格），后者要的是一次真的 OS 拖放 \
     —— 按住浏览器里的一张图挪到窗上再松手。这一档跑在 `engine.load()` 之前，两样都拿不到。所以 \
     ①–⑰ 量的是它**调用的那三手**（`pieces`／`one`／`running_line`）与它**交给的那两手** \
     （`falcon_core::drop::sort`、`state::land_drop`），`drop_in.rs` 的接线位置由 ④⑩⑮⑰ 那四格的 \
     源码形状读；「屏幕上真出现那一行、窗真换了一张」至今没有真机读数。穿透模式（§5.9.12 那枚 \
     `Qt.WindowTransparentForInput`）那一支也没有真机读数：整扇窗对输入透明时这一格收不到落下， \
     PRD 给的出口是 §5.9.13 那一枚「关掉穿透」。"
        .to_string()
}

pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let dir = door_dir();
    let _ = std::fs::create_dir_all(&dir);
    const SRC: &str = include_str!("drop_in.rs");
    const PIN_QML: &str = include_str!("../qml/PinWindow.qml");

    // 三张真图片：`big` 是拖进来的**原**那一张，`small` 用来换它（尺寸必须不同，否则「换来源」
    // 那一族要跟着走的每一格都看不出差别），`wire` 是给 ⑧ 那格由 socket 送出去的字节。
    let big = plant_picture(&dir, "drop-big.png", 60, 40, [200, 30, 30, 255]);
    let small = plant_picture(&dir, "drop-small.png", 30, 20, [30, 60, 200, 255]);
    let wire = plant_picture(&dir, "drop-wire.png", 40, 24, [30, 180, 60, 255]);
    let planted = match (&big, &small, &wire) {
        (Some(_), Some(_), Some(_)) => "三张真图片都种进了本档目录",
        _ => "种图片失败（编码或落盘），⑤ 之后各格没有对面",
    };
    rep.note("fixtures", format!("{}：{:?}", planted, dir));

    // ---- ① 归堆保序，且 `file:` 那一支由**盘上那一只手**认成路径。
    //
    // 一次拖进来的那一串里，第 i 格说第 i 格的话：阶梯落位按拖进来的次序排（与 §5.8.3 同一个承诺），
    // 而次序在代码里最容易在某天为了「先取文件更省一次网络」被改掉，所以这里读的是**名字**，不是枚数。
    let percent = format!("file:///{}", dir.join("a%20b.png").display()).replace('\\', "/");
    let joined = [
        percent.as_str(),
        "https://img.example.invalid/a.png",
        "mailto:nobody@example.invalid",
        "https://img.example.invalid/c.png",
    ]
    .join(crate::settings_view::RECORD);
    let piles = drop::sort(crate::drop_in::pieces(&joined));
    let decoded = piles
        .files
        .first()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    rep.row(
        "cells pile up in the order they came",
        Check::from(
            piles.files.len() == 1
                && piles.web.len() == 2
                && piles.refused.len() == 1
                && piles.usable() == 3
                && decoded.contains("a b.png")
                && !decoded.contains("%20")
                && piles.web[0].url.ends_with("/a.png")
                && piles.web[1].url.ends_with("/c.png"),
        ),
        format!(
            "四格（{} + 一句 mailto + 两句 http）→ 文件 {} 枚、地址 {} 枚、拒绝 {} 句，可用 {} 枚；\
             `file:` 那一支经 [`crate::save::local_path`] 变成 {:?}（百分号已解、多余斜杠已并），\
             地址两枚仍是拖进来的次序",
            dir.join("a%20b.png").display(),
            piles.files.len(),
            piles.web.len(),
            piles.refused.len(),
            piles.usable(),
            decoded,
        ),
    );

    // ---- ② 一批超过一次贴图的上限：**取回之前**就判掉，超出的一枚各有一句。
    //
    // 判在归堆这一步是这一格的正文：让第 33 句地址各付一次网络的代价、再把它们逐句退回屏幕上那一行，
    // 是最贵的那一种坏法。上限与 §5.8.3 那一批对话框同一枚数（`FILE_PIN_MAX`），两批挡的是同一件事。
    let urls: Vec<Dropped> = (0..(FILE_PIN_MAX + 8))
        .map(|i| Dropped::classify(&format!("https://img.example.invalid/{i}.png")))
        .collect();
    let capped = drop::sort(urls);
    let mixed = drop::sort(
        (0..10)
            .map(|i| Dropped::File(PathBuf::from(format!("C:/x/{i}.png"))))
            .chain((0..(FILE_PIN_MAX - 4)).map(|i| {
                Dropped::classify(&format!("https://img.example.invalid/m{i}.png"))
            })),
    );
    let all_capped = capped
        .refused
        .iter()
        .all(|w| w.contains("一次最多贴") && w.contains("再挑少一点"));
    rep.row(
        "a batch over the cap is refused pre-fetch",
        Check::from(
            capped.usable() == FILE_PIN_MAX
                && capped.refused.len() == 8
                && all_capped
                && mixed.files.len() == 10
                && mixed.web.len() == FILE_PIN_MAX - 10
                && mixed.refused.len() == FILE_PIN_MAX - 4 - (FILE_PIN_MAX - 10),
        ),
        format!(
            "{} 句地址 → 可用 {} 枚、拒绝 {} 句（每句都点名上限 {}，且没有一句真去取）；文件 10 枚混 \
             {} 句地址（共 {} 格）→ 文件 {} 枚、地址 {} 枚、拒绝 {} 句（两批共用一枚上界，谁先来谁先占）",
            FILE_PIN_MAX + 8,
            capped.usable(),
            capped.refused.len(),
            FILE_PIN_MAX,
            FILE_PIN_MAX - 4,
            mixed.files.len() + mixed.web.len() + mixed.refused.len(),
            mixed.files.len(),
            mixed.web.len(),
            mixed.refused.len(),
        ),
    );

    // ---- ③ 三种「不是图片」各说一句（#453），而那一句可取的地址拿回的是**同一次解析**的 `Web`。
    //
    // 判据共用 [`falcon_core::link`] 那道闸，所以不会出现「这里说是地址、`fetch` 那里说不是」，
    // 也不会出现「提示要联网的那句」与「真去取的那句」长得像但不是同一串字。
    let sayings: Vec<String> = ["只是一段文字", "mailto:nobody@example.invalid", "https://ex\u{e4}mple.invalid/a.png"]
        .iter()
        .map(|raw| match Dropped::classify(raw) {
            Dropped::Refused(why) => why,
            other => panic!("{raw} 不该被认成可用的一格：{other:?}"),
        })
        .collect();
    let good = match Dropped::classify("https://user:pass@img.example.invalid:8443/x.png") {
        Dropped::Web(web) => web,
        other => panic!("{other:?}"),
    };
    rep.row(
        "the three non-pictures answer apart",
        Check::from(
            sayings.windows(2).all(|w| w[0] != w[1])
                && sayings[0].contains("既不是一份文件")
                && sayings[1].contains("mailto")
                && sayings[2].contains("非 ASCII")
                && good.host == "img.example.invalid"
                && good.port == Some(8443)
                && good.url.contains("img.example.invalid:8443"),
        ),
        format!(
            "{}；那一句可取的：host={} 端口={:?}（userinfo 不进域名），取的那一句与说的那一枚域名同出一次解析",
            sayings.join(" ／ "),
            good.host,
            good.port,
        ),
    );

    // ---- ④ 落点那张表四格穷尽、四句各不同，而且**整趟只问一次**。
    //
    // 「什么时候问」是这一格的正文：`plan` 数的是这一批里有几格可用，取回之后那一数只会变小（一句
    // 404 就少一格）。取回之后再问一遍，屏幕上说的与真做的就会是两件事。所以这里同时读表与读接线：
    // `Decision::at(` 在 `drop_in.rs` 里只出现一次，且出现在 `.spawn(` 之前。
    let table: Vec<(Landing, &str, &str)> = [(true, 1usize), (true, 3), (false, 1), (true, 0)]
        .iter()
        .map(|(replaces, count)| {
            let d = Decision::at(*replaces, *count);
            (d.landing, d.said, d.landing.word())
        })
        .collect();
    let saids: Vec<&str> = table.iter().map(|t| t.1).collect();
    let once = SRC.matches("Decision::at(").count() == 1
        && SRC
            .find("Decision::at(")
            .zip(SRC.find(".spawn("))
            .map_or(false, |(d, s)| d < s);
    rep.row(
        "the landing table is exhaustive and asked once",
        Check::from(
            table[0].0 == Landing::Replace
                && table[1].0 == Landing::New
                && table[2].0 == Landing::New
                && table[3].0 == Landing::Nothing
                && (0..4).all(|i| (i + 1..4).all(|j| saids[i] != saids[j]))
                && table[3].2 == "不动"
                && !table[3].1.contains("另钉")
                && once
                && !SRC.contains("Landing::plan("),
        ),
        format!(
            "四格：{} → 「{}」／{} → 「{}」／{} → 「{}」／{} → 「{}」；整只 crate 里 `Landing::plan(` \
             只由 [`crate::state::Decision::at`] 叫一次（桥侧 {} 处），且 `Decision::at(` 排在 \
             `.spawn(` 之前：{} 处",
            "开着+一枚",
            saids[0],
            "开着+三枚",
            saids[1],
            "关着+一枚",
            saids[2],
            "一格也没有",
            saids[3],
            SRC.matches("Decision::at(").count(),
            once,
        ),
    );

    // ---- ⑤⑥⑦⑧⑨⑩ 那六格需要**名册里真有一枚贴图**：本档现种一枚，逐格问它。
    let at = PhysPoint::new(320, 240);
    let Some((big_path, _)) = big.as_ref() else {
        for name in [
            "the roster carries a real picture to replace",
            "a replaced source moves the crop and the provenance",
            "a landed batch decided before the fetch keeps its word",
            "a real fetch of a real picture reaches a real pin",
            "a refusal from the far end is told as itself",
            "an over-cap body is a failure, not half a picture",
            "bytes that are not a picture never reach the screen",
            "the line stops quoting and history takes the rest",
            "one pin holds one drop in flight",
            "the switch is on the pin page and parses",
            "the switch is read at press time, not at startup",
            "the notice names a request only when one happens",
            "the page hands whole urls over",
            "a real drop onto a real pin window",
        ] {
            // 那一格在两条路上都得说同一句欠账：降级那趟除了没种出图片，「没跑过 `start` 本体」与
            // 「没真拖过一次」这两件一个字也不许少（#433：坏法更多的一趟不许把话说得更短）。
            let why = if name == "a real drop onto a real pin window" {
                format!(
                    "{}/而本档那三张 fixture 图片也没种出来（见上面那行 `fixtures`），后面各格同样没有对面。",
                    the_unmeasured_leg()
                )
            } else {
                "本档那三张 fixture 图片没种出来（见上面那行 `fixtures`），这一格没有对面。".to_string()
            };
            rep.row(name, Check::Blocked, why);
        }
        rep.note("user config", "本档一次也没写过它（见下面那一行读数）".to_string());
        return rep.finish("drop-in door");
    };
    let first: Option<u64> = state::with(|s| s.add_from_file(&big_path.display().to_string(), at).ok());
    let Some(id) = first else {
        return rep.finish("drop-in door: 名册建不起一枚贴图，本档后面各格没有对面");
    };
    let roster0 = state::with(|s| s.ids().len());
    let read = |id: u64| -> (PhysRect, String, bool) {
        state::with(|s| {
            let p = s.set.get(id);
            (
                p.map(|x| x.src_rect).unwrap_or(PhysRect::new(0, 0, 0, 0)),
                p.map(|x| format!("{:?}", x.image)).unwrap_or_default(),
                p.is_some(),
            )
        })
    };
    let before = read(id);
    rep.row(
        "the roster carries a real picture to replace",
        Check::from(roster0 >= 1 && before.2 && before.0.w == 60 && before.0.h == 40),
        format!(
            "名册 {} 枚；贴图 {id} 的源矩形 {}、来历 {}，这一格在名册里活着={} —— 后面每一格都在这一枚上问",
            roster0, before.0, before.1, before.2
        ),
    );

    // ---- ⑥ 换来源要跟着走的每一格：那一刀（`src_rect`）、来历（`Volatile`）、枚数（不增）。
    //
    // 这一格钉的是「不是一套新代码」：`adopt_source` 与 §7.2 崩溃恢复共用同一只手，桥侧不自己夹第二遍；
    // 来历留在 `Cache(path)` 的那一版会让下一份快照把**旧**那一张再写一遍，崩溃恢复就把旧的复活回来。
    let crop = state::with(|s| s.edit(id, |p, _, _| p.crop(&PhysRect::new(10, 10, 40, 20))));
    let cropped = read(id);
    let (small_path, _) = small.as_ref().expect("上面已经判过三张 fixture");
    let decision = Decision::at(true, 1);
    let replaced = state::land_drop(
        id,
        &[Arrived::File(small_path.clone())],
        &decision,
        &[],
    );
    let after = read(id);
    rep.row(
        "a replaced source moves the crop and the provenance",
        Check::from(
            crop.as_ref().map_or(false, |r| r.is_ok())
                && cropped.0 == PhysRect::new(10, 10, 40, 20)
                && after.0 == PhysRect::new(10, 10, 20, 10)
                && after.1.starts_with("Volatile")
                && state::with(|s| s.ids().len()) == roster0
                && replaced.contains("换这一枚")
                && replaced.contains("已换到贴图"),
        ),
        format!(
            "裁到 40x20（源矩形 {}）→ 换进一张 30x20 → 源矩形 {}（那一刀跟着夹进新那一张）、来历 {}、\
             名册仍是 {} 枚；屏幕上那一行：{replaced}",
            cropped.0, after.0, after.1, roster0
        ),
    );

    // ---- ⑦ 落点是**按下那一刻**定的，取回之后不许重算。
    //
    // 这一格是 ④ 那一句「只问一次」的行为证人：造出 `plan` 唯一会漂的那一趟 —— 开关开着、两句地址、
    // 其中一句 404。`Decision::at(true, 2)` 答 `New`（一次只换得起一枚），若落地时按**到手的**枚数
    // 重算，同一趟就会翻成 `Replace`：屏幕上说的是「各钉一枚新的」，真做的却是换掉这一枚。
    let keep = read(id);
    let many = Decision::at(true, 2);
    let line = state::land_drop(
        id,
        &[Arrived::File(small_path.clone())],
        &many,
        &["那边答的是状态码 404，不是这一趟要取的那一份".to_string()],
    );
    let grew = state::with(|s| s.ids().len());
    let untouched = read(id);
    rep.row(
        "a landed batch decided before the fetch keeps its word",
        Check::from(
            many.landing == Landing::New
                && grew == roster0 + 1
                && untouched.0 == keep.0
                && line.contains("另钉一枚")
                && !line.contains("换这一枚")
                && line.contains("状态码 404"),
        ),
        format!(
            "开关开着 + 两格可用 → 落点 {}（{} 句里一句 404）→ 只有一枚到手：名册 {} → {} 枚，原那 \
             一枚的源矩形仍是 {}（没被换掉）；那一行：{line}",
            many.landing.word(),
            2,
            roster0,
            grew,
            untouched.0,
        ),
    );

    // ⑦ 新钉出来的那一枚后面两格要用；把它收掉，名册回到本档起手那一份。
    let extra = state::with(|s| s.ids().len());
    if extra > roster0 {
        let new_id = state::with(|s| s.ids().last().copied());
        if let Some(n) = new_id {
            state::with(|s| s.close(&[n]));
        }
    }

    // ---- ⑧⑨⑩⑪ 那四格对着**本机那一扇服务器**跑。
    let server = Door::start(wire.as_ref().map(|(_, b)| b.clone()).unwrap_or_default());
    match &server {
        None => {
            let why = "这一趟起不动那一扇本机服务器（`TcpListener::bind(\"127.0.0.1:0\")` 没成交），\
                       真取回的那四格没有对面 —— 发 Blocked，不拿假货当证人。"
                .to_string();
            for name in [
                "a real fetch of a real picture reaches a real pin",
                "a refusal from the far end is told as itself",
                "an over-cap body is a failure, not half a picture",
                "bytes that are not a picture never reach the screen",
            ] {
                rep.row(name, Check::Blocked, why.clone());
            }
        }
        Some(door) => {
            // ---- ⑧ 200 + 一份真 PNG：**这一整个程序里那同一只手**取回，落地成一名新的贴图。
            //
            // 「真的」在这一格有两层：socket 那头是本档起的，取回用的是 `platform_windows::fetch`
            // （wininet，与 `[about] release_feed` 那份清单同一只手），而译像素走的是与文件那一支
            // **同一个** `state::drop_new`。落地那一枚的源矩形是 40x24 —— 那只能是服务器写的字节。
            let ok_url = door.url("pic.png");
            let ok_web = Dropped::classify(&ok_url);
            let mut arrived: Vec<Arrived> = Vec::new();
            let mut failed: Vec<String> = Vec::new();
            let mut ok_said = String::new();
            if let Dropped::Web(web) = &ok_web {
                let host = web.display_host();
                match platform_windows::fetch::bytes(&web.url, drop::IMAGE_LIMIT) {
                    Ok(bytes) => {
                        ok_said = format!("取回 {} 字节", bytes.len());
                        arrived.push(Arrived::Remote { bytes, host });
                    }
                    Err(e) => failed.push(format!("{host}：{e}")),
                }
            }
            let landed = state::land_drop(id, &arrived, &Decision::at(false, arrived.len()), &failed);
            let new_ids = state::with(|s| s.ids().clone());
            let fresh = if new_ids.len() > roster0 {
                new_ids.last().copied()
            } else {
                None
            };
            let pinned = fresh.map(read).unwrap_or((PhysRect::new(0, 0, 0, 0), String::new(), false));
            let box_size = fresh.map(|n| {
                state::with(|s| {
                    let v = s.view_data(n);
                    (v.size_w, v.live)
                })
            });
            rep.row(
                "a real fetch of a real picture reaches a real pin",
                Check::from(
                    arrived.len() == 1
                        && failed.is_empty()
                        && fresh.is_some()
                        && ok_said.contains("字节")
                        && landed.contains("钉成贴图")
                        && landed.contains("从 127.0.0.1")
                        && pinned.0 == PhysRect::new(0, 0, 40, 24)
                        && box_size.map_or(false, |(w, live)| live && w > 0),
                ),
                format!(
                    "{ok_url} → {ok_said} → 新贴图源矩形 {}（{}），窗宽 {} px；那一行：{landed}",
                    pinned.0,
                    pinned.1,
                    box_size.map_or(0, |(w, _)| w),
                ),
            );
            // 收掉这一格新钉的那一枚，⑨⑩⑪ 仍在 ⑤ 起手那一枚上问。
            if let Some(n) = fresh {
                state::with(|s| s.close(&[n]));
            }

            // ---- ⑨ 对面答 404：那一句说的是**那台服务器**，不是用户的盘，也不是「换上了别的」。
            let gone = match Dropped::classify(&door.url("gone.png")) {
                Dropped::Web(web) => web,
                other => panic!("本机地址该是可用的一格：{other:?}"),
            };
            let gone_said = match platform_windows::fetch::bytes(&gone.url, drop::IMAGE_LIMIT) {
                Ok(bytes) => format!("不该取回（{} 字节）", bytes.len()),
                Err(e) => format!("{e}"),
            };
            let here = read(id);
            let line = state::land_drop(
                id,
                &[],
                &Decision::at(true, 1),
                &[format!("{}：{gone_said}", gone.display_host())],
            );
            let now = read(id);
            rep.row(
                "a refusal from the far end is told as itself",
                Check::from(
                    gone_said.contains("状态码 404")
                        && !gone_said.contains("读不出来")
                        && now.0 == here.0
                        && state::with(|s| s.ids().len()) == roster0
                        && line.contains("状态码 404")
                        && line.contains("没被换掉")
                        && line.contains("屏幕上还是它原来那一张"),
                ),
                format!(
                    "{} → {gone_said}；换进去的那一手没跑（源矩形仍 {}，名册 {} 枚）；那一行同时说了 \
                     那台服务器答了什么与本枚贴图此刻还是哪一张：{line}",
                    gone.url,
                    now.0,
                    roster0,
                ),
            );

            // ---- ⑩ 递进去的上限到了就是**失败**：一枚字节也不交回，不拿一半当整份（PRD 那句
            // 「不得用低质量占位图静默替代」在这一支上的形状）。
            let big_url = door.url("big.png");
            let probe = platform_windows::fetch::bytes(&big_url, PROBE_LIMIT);
            let probe_said = match &probe {
                Ok(bytes) => format!("不该成交（取回 {} 字节）", bytes.len()),
                Err(e) => format!("{e}"),
            };
            let limit_said = PROBE_LIMIT.to_string();
            rep.row(
                "an over-cap body is a failure, not half a picture",
                Check::from(
                    matches!(&probe, Err(platform_windows::fetch::FetchError::TooLarge(n)) if *n == PROBE_LIMIT)
                        && probe_said.contains(limit_said.as_str())
                        && probe_said.contains("一半")
                        && probe.as_ref().map_or(true, |b| b.is_empty())
                        && SRC.contains("drop::IMAGE_LIMIT")
                        && drop::IMAGE_LIMIT > update::FEED_LIMIT * 16,
                ),
                format!(
                    "{}（对面写了 {OVER_BODY} 字节）递进去的上限 {PROBE_LIMIT} → {probe_said}；产品 \
                     那一趟递进去的是 [`falcon_core::drop::IMAGE_LIMIT`]（{} 字节，接线原文在 \
                     `drop_in.rs`），它比一份清单的上限大一档：{}",
                    big_url,
                    drop::IMAGE_LIMIT,
                    update::FEED_LIMIT,
                ),
            );

            // ---- ⑪ 取回来了、也在上限之内，但那一份**不是图片**：屏幕上仍是原来那一张。
            //
            // 这一格是 PRD 那一句「不得用低质量占位图静默替代」在**像素**那一头的对面：坏字节过了
            // 网络那一关，但过不了译像素那一关，而那一关失败时 `frames` 里不会插进任何东西。
            let junk = match Dropped::classify(&door.url("text.txt")) {
                Dropped::Web(web) => web,
                other => panic!("{other:?}"),
            };
            let junk_arrived = platform_windows::fetch::bytes(&junk.url, drop::IMAGE_LIMIT)
                .ok()
                .map(|bytes| Arrived::Remote {
                    host: junk.display_host(),
                    bytes,
                });
            let here = read(id);
            let line = junk_arrived
                .as_ref()
                .map(|a| state::land_drop(id, std::slice::from_ref(a), &Decision::at(true, 1), &[]))
                .unwrap_or_else(|| "那一格压根没取回来，这一格没有对面".to_string());
            let now = read(id);
            rep.row(
                "bytes that are not a picture never reach the screen",
                Check::from(
                    junk_arrived.is_some()
                        && line.contains("从 127.0.0.1")
                        && !line.contains("已换到贴图")
                        && now.0 == here.0
                        && now.1 == here.1
                        && state::with(|s| s.ids().len()) == roster0,
                ),
                format!(
                    "{} → 取回一份 text/plain → 落点说：{line}；这一枚的源矩形与来历（{} / {}）一个字没动，\
                     名册仍是 {} 枚 —— 没有一种占位图能进这一条路，因为那一支只有「真的译出来的一张」\
                     与「一句拒绝」两种答",
                    junk.url,
                    now.0,
                    now.1,
                    roster0,
                ),
            );
        }
    }

    // ---- ⑫ 屏幕上那一行放不下时，多出来的那些句进**运行历史**，一句也不咽（#433）。
    //
    // 一行提示到第四句就不像提示、像日志 —— 与 §5.8.3 那一批同一条规矩。而「没引」不等于「没发生」，
    // 所以这一格同时读那一行的句数与 `problems` 的行数：两者加起来必须等于递进去的句数。
    let troubles: Vec<String> = (0..(FILE_PIN_SAID_MAX + 4))
        .map(|i| format!("那一份比这一趟允许的大小（{} 字节）还大，本程序不拿一半的内容当成整份", i))
        .collect();
    let problems0 = state::with(|s| s.problems.len());
    // 递进去的是一批**全失败**的落点（`Nothing` 那一臂：它不添自己的那一句，所以「引了几句 / 记了几行」
    // 这一格数得到的就是这里递进去的句数，不用再加一只手的账）。
    let line = state::land_drop(id, &[], &Decision::at(false, 0), &troubles);
    let problems1 = state::with(|s| s.problems.len());
    let quoted = troubles.len() - FILE_PIN_SAID_MAX;
    rep.row(
        "the line stops quoting and history takes the rest",
        Check::from(
            problems1 == problems0 + quoted
                && line.contains(&format!("另有 {quoted} 句逐行记进运行历史"))
                && !line.contains(&troubles[FILE_PIN_SAID_MAX])
                && state::with(|s| s.problems.last().map_or(false, |p| p.contains("不拿一半的内容当成整份"))),
        ),
        format!(
            "递进去 {} 句失败（上限 {} 句）→ 那一行引 {} 句并说「另有 {quoted} 句逐行记进运行历史」，\
             历史 {} 行 → {} 行；最后一行原文：{:?}",
            troubles.len(),
            FILE_PIN_SAID_MAX,
            FILE_PIN_SAID_MAX,
            problems0,
            problems1,
            state::with(|s| s.problems.last().cloned().unwrap_or_default()),
        ),
    );

    // ---- ⑬ 一枚贴图同时只挂一趟落下，而三枚答各说一件事（#453）。
    //
    // 「还在路上」与「这一枚已经不在了」是两件事：用户能做的只有那两样里的各一样。而**成与败都算
    // 落地**：`dropping` 挂在 `true` 上不放，这枚贴图从此不再接受任何一次落下，而用户看不见为什么。
    let gone_arming = state::with(|s| s.arm_drop(999_999));
    let a1 = state::with(|s| s.arm_drop(id));
    let busy_while = (state::with(|s| s.dropping(id)), state::with(|s| s.arm_drop(id)));
    // 全失败的一趟也算落地：这一格用 ⑫ 那一趟的形状（一枚也没有），落地后那一格必须放下来。
    let _ = state::land_drop(id, &[], &Decision::at(true, 0), &["一句失败".to_string()]);
    let after_all = state::with(|s| (s.dropping(id), s.arm_drop(id)));
    state::with(|s| s.disarm_drop(id));
    rep.row(
        "one pin holds one drop in flight",
        Check::from(
            gone_arming == Arming::Gone
                && a1 == Arming::Taken
                && busy_while.0
                && busy_while.1 == Arming::Busy
                && after_all.0 == false
                && after_all.1 == Arming::Taken,
        ),
        format!(
            "没这一枚 → {}；第一趟 → {}（dropping=true）；第二趟 → {}；落地后（那一趟**全失败**也算）→ \
             dropping={}，再拖 → {}",
            match gone_arming {
                Arming::Gone => "Gone",
                Arming::Busy => "Busy",
                Arming::Taken => "Taken",
            },
            match a1 {
                Arming::Gone => "Gone",
                Arming::Busy => "Busy",
                Arming::Taken => "Taken",
            },
            match busy_while.1 {
                Arming::Gone => "Gone",
                Arming::Busy => "Busy",
                Arming::Taken => "Taken",
            },
            after_all.0,
            match after_all.1 {
                Arming::Gone => "Gone",
                Arming::Busy => "Busy",
                Arming::Taken => "Taken",
            },
        ),
    );

    // ---- ⑭ 那一枚设置：页上有一行、那一行是开关、档上写着 true 就读成 true、默认在保守那一头。
    //
    // 「默认 false」与「类型是布尔」两件事在 core 有自己的格子（`config.rs` 那一族的
    // `the_drop_lands_by_a_key_that_is_stored_and_switch_shaped`）；这一格读的是**桥这一侧**：
    // 页上有没有那一行（`settings_view::rows_of` 走的是 `flat()`，不在表里的一枚就是页上点不到），
    // 以及一份本档写的档读回来是什么。
    let pin_page = settings_view_index_of("pin");
    let off_cfg = Config::default();
    let on = store_switch(&dir, "switch-on.toml", "true");
    let off = store_switch(&dir, "switch-off.toml", "false");
    let rows = settings_view_rows(pin_page, &off_cfg);
    let row = rows.iter().find(|(k, _, _)| k == "pin.drop_replaces_current");
    let line_shape = row.as_ref().map(|(_, v, k)| (v.clone(), k.clone()));
    rep.row(
        "the switch is on the pin page and parses",
        Check::from(
            row.is_some()
                && line_shape.as_ref().map_or(false, |(_, k)| k == "off")
                && on.as_ref().map_or(false, |c| c.pin.drop_replaces_current)
                && off.as_ref().map_or(false, |c| !c.pin.drop_replaces_current)
                && !off_cfg.pin.drop_replaces_current,
        ),
        format!(
            "第 {} 类（贴图）里 {} 行，其中 `pin.drop_replaces_current={}`、页上那一格是 {}；本档写的两份 \
             档读回来：写 true → {}，写 false → {}（出厂默认 {}）",
            pin_page,
            rows.len(),
            row.as_ref().map(|(_, v, _)| v.clone()).unwrap_or_default(),
            line_shape
                .as_ref()
                .map(|(_, k)| k.clone())
                .unwrap_or_else(|| "页上没有这一行".to_string()),
            on.map_or("读不出".to_string(), |c| c.pin.drop_replaces_current.to_string()),
            off.map_or("读不出".to_string(), |c| c.pin.drop_replaces_current.to_string()),
            Config::default().pin.drop_replaces_current,
        ),
    );

    // ---- ⑮ 那一枚开关是**按下那一刻现读**的，不是进程启动那一趟的快照。
    //
    // 为什么这一半只能读源码形状：进程手里那一份 `SETTINGS` 是 `OnceLock`，本档换不掉它，所以
    // 「改完档立刻拖一次」在两趟之间问不出差别（§5.20.2 那句「即时生效」由 `--settings-window`
    // 那一族量）。这一格读的是**谁在什么时刻取值**：`drop_in.rs` 里那一处调用排在 `fn start(` 之内
    // —— 每按一次走一次，而不是 `static` / `init` 那一族的读数 —— 且排在 `.spawn(` 之前，请求发出
    // 之前值已定；那只手本身是 `settings::with` 现取。而整段 `[pin]` 的快照（`settings::pin_cfg()`）
    // 在拖入这一条路上一个字也不读，因为快照说的是**新贴图长什么样**，而那一枚开关说的是**这一趟落到
    // 哪一格**。次序上它必须在 `drop::sort(` **之后**：落点要数这一批里有几格可用。
    let no_snapshot = !SRC.contains("drop_replaces_current") && !SRC.contains("pin_cfg()");
    let read_at = SRC.find("settings::drop_replaces()");
    let in_start = SRC
        .find("fn start(")
        .zip(read_at)
        .map_or(false, |(f, r)| f < r);
    let before_fetch = read_at
        .zip(SRC.find(".spawn("))
        .map_or(false, |(r, s)| r < s);
    rep.row(
        "the switch is read at press time, not at startup",
        Check::from(
            SRC.matches("settings::drop_replaces()").count() == 1
                && in_start
                && before_fetch
                && read_at
                    .zip(SRC.find("drop::sort("))
                    .map_or(false, |(r, s)| r > s)
                && SRC.matches("drop::sort(").count() == 1
                && no_snapshot
                && crate::settings::drop_replaces()
                    == crate::settings::with(|st| st.config.pin.drop_replaces_current),
        ),
        format!(
            "`drop_in.rs` 里 `settings::drop_replaces()` {} 处，位置在 `fn start(` 之内（{}）、在 \
             `.spawn(` 之前（{}）、在 `drop::sort(` 之后（{}，落点要数得出这一批有几格）；那一只手现取\
             （`settings.rs` 里它是 `with(|s| s.config.pin.drop_replaces_current)`）；拖入这一条路 \
             不读整段 `[pin]` 的快照（源码里出现 `drop_replaces_current` = {}，快照那一族带不带这一枚 \
             = {}）；此刻这台机器读到的那一头：{}",
            SRC.matches("settings::drop_replaces()").count(),
            in_start,
            before_fetch,
            read_at
                .zip(SRC.find("drop::sort("))
                .map_or(false, |(r, s)| r > s),
            SRC.contains("drop_replaces_current"),
            SRC.contains("pin_cfg()"),
            crate::settings::drop_replaces(),
        ),
    );

    // ---- ⑯ 那句「应明确提示网络请求」只在真会联网的那一趟出口，且说的域名就是要去取的那些。
    //
    // 句子本体共用 [`falcon_core::update::web_notice`] 那一个作者（一台机器时原样引它）；多台时
    // 「向 N 台机器取」是另一件事，所以有它自己的一句，但列出的仍是同一手解析出的 host。第五枚之后
    // 折成「等」：首屏放不下几十枚域名，而数目本身照说。
    let one_host: Vec<Web> = vec![Web {
        url: "https://img.example.invalid/a.png".to_string(),
        host: "img.example.invalid".to_string(),
        port: None,
        idn: false,
    }];
    let many_hosts: Vec<Web> = (0..7)
        .map(|i| Web {
            url: format!("https://h{i}.example.invalid/{i}.png"),
            host: format!("h{i}.example.invalid"),
            port: None,
            idn: false,
        })
        .collect();
    let said_one = crate::drop_in::running_line(7, &one_host);
    let said_many = crate::drop_in::running_line(7, &many_hosts);
    let ported = Web {
        url: "https://img.example.invalid:8443/a.png".to_string(),
        host: "img.example.invalid".to_string(),
        port: Some(8443),
        idn: false,
    };
    rep.row(
        "the notice names a request only when one happens",
        Check::from(
            said_one.contains("在路上")
                && said_one.contains("网络请求")
                && said_one.contains(&update::web_notice(&one_host[0], "那一份图片"))
                && said_one.contains("img.example.invalid")
                && said_many.contains("7 台机器")
                && said_many.contains("h0.example.invalid")
                && said_many.contains("h4.example.invalid")
                && !said_many.contains("h5.example.invalid")
                && update::net_notice(&dir.join("drop-big.png").display().to_string(), "那一份图片")
                    .is_none()
                && update::web_notice(&ported, "那一份图片").contains("8443"),
        ),
        format!(
            "一句地址：{said_one} ／ 七句（五枚之后折成「等」）：{said_many} ／ 盘上那一份文件：{} \
             —— §6.3 那一句只在真会走网络的那一趟出口，而说出来的域名与端口就是要去取的那一枚",
            update::net_notice(
                &dir.join("drop-big.png").display().to_string(),
                "那一份图片"
            )
            .unwrap_or_else(|| "一个字也不说".to_string()),
        ),
    );

    // ---- ⑰ 页上那一手只做「把 `mimeData.urls` 串成一串 + 递 `pinId`」，一句也不判。
    //
    // `String(urls[i])` 而不是 `urls[i].toLocalFile()`：本机这代 Qt 给 JS 的 `QUrl` 一个成员都读不
    // 出来（P37 量过 `TypeError`），所以递过桥的只能是整句 URL，认它的手在 `crate::save::local_path`。
    // 分隔符是 `settings_view::RECORD` 那一枚控制字符 `\u001d`（列表分隔符的唯一作者）。
    let drop_block = PIN_QML
        .find("DropArea {")
        .map(|i| PIN_QML[i..PIN_QML.len().min(i + 900)].to_string())
        .unwrap_or_default();
    rep.row(
        "the page hands whole urls over",
        Check::from(
            drop_block.contains("onDropped: (drop) =>")
                && drop_block.contains("drop.mimeData.urls")
                && drop_block.contains("String(urls[i])")
                && drop_block.contains("String.fromCharCode(0x1d)")
                && drop_block.contains("win.session.pinDrop(win.pinId,")
                && drop_block.contains("drop.accept()")
                && !drop_block.contains("toLocalFile")
                && !drop_block.contains("Landing")
                && !drop_block.contains("http")
                && SRC.contains("settings_view::RECORD")
                && SRC.contains("save::local_path"),
        ),
        format!(
            "PinWindow.qml 的 `DropArea` 那一块 {} 字：递过桥的是整句 URL（`toLocalFile` 出现 {} 次）、\
             分隔符是 `String.fromCharCode(0x1d)`；页上不判文件/地址、也不判换不换（那一手在 \
             `falcon_core::drop` 与 `crate::drop_in`）",
            drop_block.len(),
            drop_block.matches("toLocalFile").count(),
        ),
    );

    // ---- ⑱ 用户那份档：只作为**盘上读数**印出来，不参与判定，也不打开它（§9.1 第十五条的判据在
    // 级档那头的 before/after 两枚 sha 上，不在这里）。
    let user_path = falcon_core::config::config_path(&Config::default());
    let reading = std::fs::metadata(&user_path).map_or_else(
        |e| format!("读不出（{e}）"),
        |m| format!("{} 字节，改动时间 {:?}", m.len(), m.modified().ok()),
    );
    let user_value = crate::settings::with(|s| s.config.pin.drop_replaces_current);
    rep.note(
        "用户那份档此刻",
        format!(
            "{}：{}；进程读到那一头 = {user_value} —— 本档一次也没写过它，判据用的是本档自己那两份 \
             fixture（⑭ 那一格）",
            user_path.display(),
            reading,
        ),
    );

    // ---- ⑲ 这一档不量过的那件事，落在报告里而不是只落在注释里。
    rep.row(
        "a real drop onto a real pin window",
        Check::Blocked,
        the_unmeasured_leg(),
    );

    rep.finish("drop-in door")
}

/// 「贴图」那一类在 [`settings_view::CATEGORIES`] 里的类号。
///
/// 从名单自己数出来，不写死一枚 `3`：那一族的类号在 P178（加「热区」那一页）时集体挪过一次位，
/// 写死下标的格子那天会读到别人那一页的行。判据是「哪一类的前缀里有 `pin`」，而 `[pin]` 归贴图
/// 是名单第 4 行自己写的。
fn settings_view_index_of(prefix: &str) -> usize {
    settings_view::CATEGORIES
        .iter()
        .position(|(_, prefixes)| prefixes.contains(&prefix))
        .unwrap_or(usize::MAX)
}

/// 某一类页上的行，读的是**递进来的那一份** config；交回 `(键, 值, 那一格的样子)`。
///
/// 第三枚来自 [`settings_view::kind_of`] —— 页上画开关还是画文本框，整个程序只有那一个判断，
/// 这里引用它而不是再问一次「是不是布尔」。
fn settings_view_rows(which: usize, cfg: &Config) -> Vec<(String, String, String)> {
    settings_view::rows_of(cfg, which)
        .iter()
        .map(|r| {
            (
                r.key.clone(),
                r.value.clone(),
                settings_view::kind_of(cfg, &r.key).token().to_string(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⑩ 那一格递进去的上限**必须小于**产品那一枚：不然那一格读到的就不是「超限」那一支，而是
    /// 一次成功的取回 —— 那一形坏法在屏幕上和报告里都不出声，只有这里问得到。
    #[test]
    fn the_oversize_probe_sits_below_the_products_own_ceiling() {
        assert!(PROBE_LIMIT < drop::IMAGE_LIMIT, "{PROBE_LIMIT} vs {}", drop::IMAGE_LIMIT);
        assert!(OVER_BODY > PROBE_LIMIT, "那一份体撞不到递进去的上限");
    }

    /// ⑲ 那一格的**欠账正文**只能有一处：`the_unmeasured_leg` 既写进 ⑲ 那一格，也是这份报告里
    /// 唯一说「没跑过 `start` 本体」的地方。写成一只手，是为了那一句话不会在两处各自新过一遍。
    #[test]
    fn the_unmeasured_leg_names_both_debts_once() {
        let line = the_unmeasured_leg();
        assert!(line.contains("drop_in::start"), "{line}");
        assert!(line.contains("拖"), "{line}");
        assert!(line.contains("Blocked") || line.contains("没有真机读数"), "{line}");
        assert!(line.contains("穿透"), "{line}");
    }

    /// 分格这一手要能在没有窗的地方问：`RECORD` 分的格、trim、空的一格不算一格。三件各判一次，
    /// 坏法各不同（多喂一格、少切一格、把空格当成「不是图片」那一句）。
    ///
    /// 递进去的四格里有两格是空的（一格全空、一格只有一个空格），它们既不该成为一句拒绝、也不该
    /// 挤掉后面那一句 —— 页上 `String.fromCharCode(0x1d)` join 出来的串末尾就是这种形状。
    #[test]
    fn the_joined_string_splits_into_the_cells_it_claims() {
        let joined = format!(
            " https://a.example.invalid/1.png{sep} {sep}{sep}https://b.example.invalid/2.png ",
            sep = crate::settings_view::RECORD
        );
        let cells = crate::drop_in::pieces(&joined);
        let piles = drop::sort(cells.clone());
        assert_eq!(cells.len(), 2, "{cells:?}：两格空的，一格也不该算进来");
        assert_eq!(piles.usable(), 2, "{piles:?}：两头那两格都被 trim 掉了空格，仍是可用的地址");
        assert!(piles.refused.is_empty(), "{piles:?}：空格子不该换来一句「不是图片」");
        assert!(
            piles.web[0].url.ends_with("1.png") && piles.web[1].url.ends_with("2.png"),
            "{piles:?}：次序是拖进来的次序"
        );
        // 裸路径不当文件认：那是用户打的一串字，不是拖进来的一份文件。两种写法（反斜杠与正斜杠）
        // 都换来一句拒绝，而那句由 [`falcon_core::link`] 那一个作者说 —— 这里不重述它的字，
        // 只钉住「本层没有替它开第二条读盘的路」。
        for raw in ["C:\\x\\2.png", "C:/x/2.png"] {
            let bare = crate::drop_in::one(raw);
            assert!(
                matches!(&bare, Dropped::Refused(why) if !why.is_empty()),
                "{raw} → {bare:?}：裸路径不该被认成一份文件"
            );
        }
    }
}
