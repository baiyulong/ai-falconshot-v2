//! §5.8.4 方式二「用户将网页图片拖到已存在的贴图窗口」在桥侧的那一手：把页交来的那一串分格、
//! 认好每一格是什么，该取的去取、该装的装，最后把屏幕上那一行交回。
//!
//! # 这一层只做三段，一段也不判
//!
//! 分格与判类是 [`falcon_core::drop`]（那一格是文件、那句是地址、那句两种都不是 —— 三句话各自的
//! 作者都在 core），装进贴图是 [`crate::state::PinState::land`]，取字节是
//! `platform_windows::fetch::bytes`（与 `[about] release_feed` 那份清单**同一只手**，所以「这一句
//! 地址取不取得回」在整个程序里只有一个答案）。这里要是也判一遍「什么算地址」，就有了第二套会各自
//! 漂移的规则 —— 而这一条路的终点是把**外来的字节**喂进屏幕。
//!
//! # 为什么只有取回那一趟走线程
//!
//! PRD §5.8.4 要的是「拖进来就成图」，没说要拖多快；而 wininet 那一趟最坏要走三段超时。反过来，
//! 读盘与译像素**留在 GUI 线程**：那正是 §5.8.3（对话框挑一批文件）已经走的形状，把它做成两套
//! 线程模型只会让「一批里第 3 枚读失败」在两处各说一遍。真正不能留在 GUI 线程的只有网络。
//!
//! # 为什么这三手是 `pub(crate)` 而 [`start`] 不是
//!
//! [`pieces`]、[`one`]、[`running_line`] 三手不要任何人把手：递一串字进去就交回读数，所以
//! `--drop-in` 那一档（[`crate::drop_check`]）量得到它们。[`start`] 的第一枚参数是
//! `Pin<&mut Session>` —— 那一枚把手只有 QML 线程拿得到，所以「整趟跑一次」在门里问不出读数，
//! 那一格在 `--drop-in` 的报告里发的是 BLOCKED，不是拿这三手的读数冒充。
//!
//! # 三件不许静默的事
//!
//! - 取回失败：[`falcon_core::drop::IMAGE_LIMIT`] 超限、404、连不上、那一份不是图片，各留自己那句
//!   原话（`FetchError` 与 [`crate::state`] 里的 `read_refusal` 各是一个作者，这里不改写、也不
//!   合并成一句「拖入失败」，#453）；
//! - 拿不到手的结果（线程投不进 Qt 的事件环）：那一格当场放下来，并把已经到手的枚数写进 stderr，
//!   绝不让那枚贴图永久不再接受落下；
//! - 落点由那枚设置决定与否：那句话由 [`falcon_core::drop::Landing::plan`] 交回，随结果一起上屏幕
//!   （见 [`crate::state::PinState::land`]），所以「设置开着却没换」这一格有它自己的说法。

use std::path::PathBuf;
use std::pin::Pin;

use cxx_qt_lib::QString;
use falcon_core::drop::{self, Dropped};
use falcon_core::link::Web;

use crate::session::qobject::Session;
use crate::state::{self, Arrived, Decision};

/// 页交来的那一串（[`crate::settings_view::RECORD`] 分的格）→ 屏幕上那一行。
///
/// 三条出路各有一句，且都经过 [`crate::state::PinState::arm_drop`] 那一对：
/// 1. 这一枚贴图上已经有一趟在路上 → 当场拒，不起线程、不装东西（两趟都装到一枚上，用户看见的
///    是「我明明拖了第二张」）；
/// 2. 这一批里没有一句地址（全是要读盘的那些，或一格可用的都没有）→ 就在 GUI 线程上装完，
///    不绕线程；
/// 3. 有一句以上的地址要去取 → 交一条起了名的线程，取回的结果**投回** GUI 线程再装。
///
/// `joined` 是分好格的那一串，而不是 [`Vec<PathBuf>`]：本机这代 Qt 给 JS 的只有整句 URL
/// （P37 量过 `toLocalFile` 在运行时是 `undefined`），页只能把那一串字递过桥 —— 认它们的手在这里，
/// 而认完之后的判据仍然在 core（[`crate::save::local_path`] 只负责把 `file:` 那一支变成路径）。
pub(crate) fn start(mut this: Pin<&mut Session>, id: i64, joined: &str) -> QString {
    let id = id as u64;
    // 闸在**判类之前**：一趟已经在了，第二趟连分格都不必做 —— 分格会读用户打的那一串，而这里的
    // 每一句都可能带着一次网络请求的代价。
    let armed = state::with(|s| s.arm_drop(id));
    match armed {
        state::Arming::Busy => {
            return this.as_mut().say(&format!(
                "贴图 {id} 上已经有一张拖进来的图片在路上：等它落地再拖下一张，屏幕上现在还是原来那一张。"
            ));
        }
        state::Arming::Gone => {
            return this.as_mut().say(&format!(
                "贴图 {id} 已经不在桌上了：这一趟没有落点，拖进来的那几格一枚也不装。"
            ));
        }
        state::Arming::Taken => {}
    }

    let piles = drop::sort(pieces(joined));
    // 落点**在这里**定，且只定这一次：`plan` 数的是这一批里有几格可用，而取回之后那一数只会变小
    // （一句 404 就少一格）。取回之后再问一遍，屏幕上说的与真做的就会是两件事 —— 见
    // [`falcon_core::drop::Landing::plan`] 头顶那句「什么时候问它」。
    let decision = Decision::at(crate::settings::drop_replaces(), piles.usable());
    let mut arrived: Vec<Arrived> = piles.files.iter().cloned().map(Arrived::File).collect();
    let mut failed: Vec<String> = piles.refused.clone();

    if piles.web.is_empty() {
        // 没有一句地址要取：这一趟从按下到落地全在 GUI 线程上，`land_drop` 顺手把那一格放下来。
        let line = state::land_drop(id, &arrived, &decision, &failed);
        this.as_mut().refresh();
        return this.as_mut().say(&line);
    }

    let web = piles.web.clone();
    let running = running_line(id, &web);
    // 手上已经有几格（文件那一堆）—— 记在起线程**之前**：线程起不动时 `arrived` 已经归了那个闭包，
    // 而那句失败要说的是「本来可以装上几格」。
    let in_hand = arrived.len();
    let spawned = std::thread::Builder::new()
        .name("falconshot-drop".to_string())
        .spawn(move || {
            for piece in &web {
                let host = piece.display_host();
                match platform_windows::fetch::bytes(&piece.url, drop::IMAGE_LIMIT) {
                    Ok(bytes) => arrived.push(Arrived::Remote { bytes, host }),
                    Err(e) => failed.push(format!("{host}：{e}")),
                }
            }
            // 投不进去也得把那一格放下来，并把已经到手了几格写进 stderr：`dropping` 挂在 `true` 上
            // 而不放，这枚贴图从此不再接受任何一次落下，而用户看不见为什么。
            let got = arrived.len();
            let bad = failed.len();
            let posted = crate::hotkey::post_to_gui(move |mut session| {
                let line = state::land_drop(id, &arrived, &decision, &failed);
                session.as_mut().refresh();
                session.as_mut().say(&line);
            });
            match posted {
                crate::hotkey::Posted::In => {}
                crate::hotkey::Posted::NoHand => {
                    state::with(|s| s.disarm_drop(id));
                    eprintln!(
                        "falconshot: 拖入贴图 {id} 的取回落不了地（QML 线程的句柄还没到手），\
                         已到手 {got} 张、另有 {bad} 句没有取回"
                    );
                }
                crate::hotkey::Posted::Refused(why) => {
                    state::with(|s| s.disarm_drop(id));
                    eprintln!(
                        "falconshot: 拖入贴图 {id} 的取回落不了地（{why}），\
                         已到手 {got} 张、另有 {bad} 句没有取回"
                    );
                }
            }
        });
    if let Err(e) = spawned {
        // 线程起不动：已经到手的那几格文件也**不装** —— 装一半（文件装上、地址那几格没了）就是
        // 用户眼里的「我拖了五张怎么只有两张」。这一句把两头的数都说出口。
        state::with(|s| s.disarm_drop(id));
        return this.as_mut().say(&format!(
            "拖入贴图 {id}：起不动取回图片的线程（{e}），这一趟没有跑；\
             手上的 {in_hand} 枚文件也没有装上。",
        ));
    }
    // §6.3 那句「应明确提示网络请求」必须在请求**发起之前**出口：等落地再提示，用户看到的是请求
    // 已经发生了。这一句也不带判决 —— 那一刻还没有判决。
    this.as_mut().say(&running)
}

/// 那一串 → 每一格是什么。分格用 [`crate::settings_view::RECORD`]：那一枚控制字符是这一家**列表**
/// 分隔符的唯一作者，而选它的理由（值是用户自己打的，任何可见分隔符都会把一行切成两行）在这里
/// 同样成立。空的格子不算一格：页上拖进来的是一串 URL，中间不会有意夹一枚空。
pub(crate) fn pieces(joined: &str) -> Vec<Dropped> {
    joined
        .split(crate::settings_view::RECORD)
        .map(|piece| piece.trim())
        .filter(|piece| !piece.is_empty())
        .map(one)
        .collect()
}

/// 一格。`file:` 那一支先由 [`crate::save::local_path`] 认成路径 —— 它是这一整个程序里「对话框与
/// 拖放交回来的那串 URL 字怎么变成一枚路径」的唯一作者（百分号、`file:///C:/` 的那个多余斜杠、
/// `file://主机/共享` 那一支的 `\\主机`），本层不自己切第二遍。
///
/// 剩下的整句交给 [`Dropped::classify`]。**不认裸路径**（`C:\图\a.png` 那样的文本）：那是用户打的
/// 一串字而不是拖进来的一份文件。`link` 那一道闸把它答成一句拒绝 —— 那一串里的 `C:` 先被读成一枚
/// scheme，所以出口是「c 那一类地址，本程序只取浏览器那一类（http 与 https）」那一句（这一格的真机
/// 读数写在 `--drop-in` 那一档的 ③ 与分格那一格单测里，#455：这一句曾写成「既不是一份文件，也不是
/// 一句可以取回的图片地址」，那是 `link` 对**别的那种**文本说的话）；读盘那一条路只走 `file:` 那一支。
/// 为裸路径开一条「看起来像路径就当路径读」的第二判据，代价是把判决权交给一串没被引起来的文本。
pub(crate) fn one(piece: &str) -> Dropped {
    match piece.split_once(':') {
        Some((head, _)) if head.eq_ignore_ascii_case("file") => {
            Dropped::File(PathBuf::from(&*crate::save::local_path(piece)))
        }
        _ => Dropped::classify(piece),
    }
}

/// 「在路上」那一句：这一趟要去取几格、向哪些机器取，加上 §6.3 那句网络请求的提示。
///
/// 句子的本体共用 [`falcon_core::update::web_notice`] 那一个作者（一台机器时原样引它）；多台时
/// 「这一趟要向 N 台机器取」是**另一件事**，所以有它自己的一句，但列出的仍是 [`Web::display_host`]
/// —— 与真会被去取的那些 host 同出一格，不会出现「提示的是 A、取的是 B」。首屏放不下几十枚域名，
/// 所以第五枚之后折成「等」，数目本身照说。
pub(crate) fn running_line(id: u64, web: &[Web]) -> String {
    let mut hosts: Vec<String> = Vec::new();
    for piece in web {
        let host = piece.display_host();
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    let notice = if hosts.len() == 1 {
        falcon_core::update::web_notice(&web[0], "那一份图片")
    } else {
        let listed = if hosts.len() > 5 {
            format!("{} 等", hosts[..5].join("、"))
        } else {
            hosts.join("、")
        };
        format!(
            "这一趟要向 {} 台机器取那一份图片（{listed}）：那是网络请求。",
            hosts.len()
        )
    };
    format!(
        "拖入贴图 {id}：这一趟要去取 {} 句地址里的图片，在路上。{notice}",
        web.len()
    )
}
