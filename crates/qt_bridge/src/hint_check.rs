//! The hint window, read back from outside it.
//!
//! `--hint <ms>` is the door that asks the product to say one sentence and then looks at
//! the screen to see whether a user would have read it. §5.9.13 asks for 「保存成功可显示
//! 短暂状态提示」 and §8.5 for 「并提示」, and both had been satisfied in the only sense the
//! code could satisfy them: [`crate::session::qobject::Session::status`] carried a line of
//! Chinese that *nothing on the pin side was bound to*. §10 第 48 项 wrote that down as it
//! was found - 第 54 项 ② quotes the five readings at `PinWindow.qml:158/160/178/234/254`
//! and all of them are a gauge reading the string back, never a person reading it - so the
//! gap this file measures is the difference between a sentence that exists and a sentence
//! that arrives.
//!
//! ## What it does not claim
//!
//! The window is found and photographed, not watched: whether the sentence was *legible*
//! (a font, a contrast ratio, a human at that monitor) is not a number this process can
//! produce, and §9.1's rule about not grading a judgement on this crate's own phrasing
//! applies - the ink row counts distinct colours a window drew, which says the text was
//! painted and nothing about whether it could be read.
//!
//! And the two halves of 「短暂」 are measured by two runs, not one. A single run can only
//! read the surface at one moment, so it grades whichever direction its `hold` put it in:
//! the default `hold` longer than the run says 还在, and `--hint-hold` shorter than the run
//! says 已收. The lifetime row prints which of the two it was, so an archive cannot be read
//! as covering both.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use falcon_core::geometry::PhysRect;
use falcon_core::pin::PinId;

use crate::pin_check::said;
use crate::r13::{parse_levels, Report};
use crate::session::shim;
use crate::state;
use crate::state::Check;

/// The hint window's own title - and it has one on purpose.
///
/// A pin sets no title, which is why [`crate::pin_check`] has to pick its window by
/// overlap; the mask titles itself for the same reason this file needs. It carries no
/// spaces because `pinTopLevels` is a whitespace-split field list.
pub const TITLE: &str = "falconshot-hint";

/// The directory the door writes into, under the OS temp dir, and the one file it
/// produces. `plant` deletes a previous run's copy before starting: a file the last run
/// left behind would answer for a save that never happened.
pub const DIR: &str = "falconshot-hint-gauge";
pub const NAME: &str = "hint.png";

/// How long the surface stays up after a sentence arrives, unless the command line says
/// otherwise. Two and a half seconds is 提示 rather than 弹窗: long enough to be read at a
/// normal pace, short enough that it leaves without being dismissed. It is a Rust number and
/// `HintWindow.qml` reads it out of [`anchor_line`] rather than writing it again; the QML
/// fallback for a malformed anchor repeats it, which is the same arrangement
/// `PinWindow.qml` has with [`crate::pin_check::LEG_DELAY_MS`].
pub const HOLD_MS: i32 = 2600;

/// How long after its window exists the leg asks the product to save. One event-loop step
/// is not enough: a QWindow's native side is made on first exposure, which is a later step
/// than `Component.onCompleted` - the reason `--pin` waits the same interval and the reason
/// this door must too, or the read-back would find no window and call that a missing feature.
pub const LEG_DELAY_MS: i32 = 400;

/// The shortest `--hint` run that could have measured anything: the leg's wait, several
/// frames for the surface to be composed, and a read-back that is not competing with the
/// first paint. `main` raises a shorter request rather than refusing it, and prints both
/// numbers (§10 第 38 项 ②).
pub const MIN_MS: i32 = 2000;

/// How many distinct colours count as "this window drew something besides its own
/// background".
///
/// The card is a flat fill with a 1 px border, so a hint that never painted its text is a
/// handful of colours: the fill, the border, the rounded-corner blends and nothing else.
/// Antialiased glyphs at 13 px produce dozens of steps between the ink and the card. Eight
/// is the floor this file is prepared to call ink, and the row prints the number it actually
/// counted, so the first run of this door is also the measurement that says whether the floor
/// is anywhere near the real value.
pub const INK_MIN_DISTINCT: usize = 8;

/// Slack, in device pixels, for the two geometry rows - the same allowance
/// [`crate::pin_check::RECT_SLACK`] exists for: Qt rounds a logical rect to integers on the
/// way to Win32, and a rounding is not a window in the wrong place.
const GEOM_SLACK: i32 = 4;

/// The card's two fills, copied from `HintWindow.qml`'s two `card_fill` arms on purpose.
///
/// §5.19.1 步骤 2 asks that the palette *follow* the resolved theme, and the only claim a
/// photograph can answer is which of these two RGB triples covers the window. So this file
/// keeps both and [`palette_row`] grades the one the screen chose against the one Rust
/// resolved - the pair of them written down is what makes a QML binding that quietly never
/// fire (`session.theme` unread, a `===` that is false for every value) a FAIL rather than an
/// unexamined green.
const CARD_DARK: [u8; 3] = [0x12, 0x15, 0x1B];
const CARD_LIGHT: [u8; 3] = [0xF2, 0xF4, 0xF7];

/// Which face a measured colour is, and the single line that decides it.
///
/// 128 is not a taste. The two candidates sit at [`luma`] **20** and **243** (`[18,21,27]` 走
/// `(18*299 + 21*587 + 27*114)/1000 = 20787/1000`，整数除；`[242,244,247]` 同一式子给 243），
/// 所以分界在深色一侧留下 **108** 的余量、浅色一侧 **115**，两枚之间隔着 **223**。按卡片自己的
/// `E6` 预乘一遍是 `[16,18,24]` 与 `[218,220,222]` ⇒ **18** 与 **219**，仍在各自那一侧 ——
/// 这正是 QML 里那两个臂共用一枚 alpha 的理由，也是这一格敢被一张照片判的理由。
///
/// 上面这四个数是 P119 从 [`luma`] 那台机器里重新问出来的，此前这里写的是 21/107/222/19/220：
/// 一枚整数除的余数被人手抄成了四舍五入，而同一档里 `亮度 {}` 那一格印的是 20 —— 注释与它脚下
/// 那一行的读数在同一份档里互相反对（#239/#279 那一族）。
const LUMA_SPLIT: u32 = 128;

/// ITU-R BT.601 luma, integer form - the weightings are the standard's own, scaled by 1000 so
/// no float enters a row that gets read back out of an archive.
fn luma(px: [u8; 3]) -> u32 {
    (u32::from(px[0]) * 299 + u32::from(px[1]) * 587 + u32::from(px[2]) * 114) / 1000
}

/// The opaque colour a bitmap is mostly made of, and how many pixels hold it.
///
/// 平手时赢的是**后见**那一枚，不是先见：[`Iterator::max_by_key`] 对相等的键取最后遇到的那个，
/// 而这一档此前写的是「first-seen order wins」。这条腿真正的判据只有一枚 bit（哪一侧），平手
/// 那一局无论取哪一枚都在同一侧，所以写反的那句注释从没被测到过 —— P119 起下面那格单测把
/// 「400 对 300 的混合判得出面、判不出套」这件事钉住，并当众取走「先见」那半句。
///
/// 半深半浅的那一张照片这一格**会**放它过绿：`n` 与总数一起印进句子，占比因此看得见，但
/// 判据不吃它。这不是疏漏而是这一格的天花板 —— PrintWindow 还回来的是一张可能被合成过的图，
/// 把它和四枚字面量逐格比就是本文件替屏幕猜像素。其余三枚颜色（边框、字、提醒）由
/// `the_hint_palette_is_bound_to_the_theme_and_written_once` 那一格在源码里判，两格各管一面。
fn dominant_ink(pixels: &[u8]) -> Option<([u8; 3], usize)> {
    let mut seen: Vec<([u8; 3], usize)> = Vec::new();
    for px in pixels.as_chunks::<4>().0 {
        if px[3] < 128 {
            continue;
        }
        let rgb = [px[0], px[1], px[2]];
        match seen.iter_mut().find(|(c, _)| *c == rgb) {
            Some(hit) => hit.1 += 1,
            None => seen.push((rgb, 1)),
        }
    }
    seen.into_iter().max_by_key(|(_, n)| *n)
}

/// `HintWindow.qml` 的源码，编译期抄进来，只为下面那两格源码扫描用。
///
/// 扫描而不是运行，是因为这一档要问的是「颜色是不是只长在一处」：运行期的 `hint palette` 只
/// 看得见卡片那一枚主色，而边框、字色、提醒色三处若被谁抄回字面量，照片上照样是一套深色。
#[cfg(test)]
const HINT_QML: &str = include_str!("../qml/HintWindow.qml");

/// 其余四枚 QML 面 + `Session` 的声明本体，只为下面那格「handler 的名字对不对得上信号」抄进来。
///
/// 五枚面一起读而不是只读浮窗那一枚，是因为写错信号名这一族缺陷**不会报错**：Qt 只印一行
/// `warn:` 到 stderr，而那一行不在任何一格判据里（P119 之前它是零枚读者）。`Session` 的
/// qproperty 清单是那一格的尺子，而尺子必须与界面分开放 —— 见那格自己的注释。
#[cfg(test)]
const QML_FACES: [(&str, &str); 5] = [
    ("main.qml", include_str!("../qml/main.qml")),
    ("CaptureMask.qml", include_str!("../qml/CaptureMask.qml")),
    ("PinWindow.qml", include_str!("../qml/PinWindow.qml")),
    (
        "AnnotationToolbar.qml",
        include_str!("../qml/AnnotationToolbar.qml"),
    ),
    ("HintWindow.qml", include_str!("../qml/HintWindow.qml")),
];

#[cfg(test)]
const SESSION_SRC: &str = include_str!("session.rs");

/// What the door needs to remember between the QML leg and the read-back.
struct Plan {
    /// Empty dir means "not a `--hint` run", which is what keeps every other session -
    /// including the resident app - from running a leg at all.
    dir: PathBuf,
    file: PathBuf,
    id: PinId,
    /// The lifetime the QML side was handed, kept here so the row that grades 「短暂」 is
    /// graded against the number this run was actually told, not against [`HOLD_MS`].
    hold: i32,
    lines: Vec<String>,
}

impl Plan {
    const fn none() -> Plan {
        Plan {
            dir: PathBuf::new(),
            file: PathBuf::new(),
            id: 0,
            hold: HOLD_MS,
            lines: Vec::new(),
        }
    }
}

static PLAN: Mutex<Plan> = Mutex::new(Plan::none());

fn with<R>(f: impl FnOnce(&mut Plan) -> R) -> R {
    let mut plan = PLAN.lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut plan)
}

/// The hint window's anchor and lifetime, as one `|`-separated line read once at
/// `Component.onCompleted`: `主屏 x | 主屏 y | 主屏 w | 主屏 h | dpr | hold 毫秒`.
///
/// Empty when this run is not allowed a hint surface, which is how `HintWindow.qml` knows to
/// stay out of it - the same one-signal shape [`crate::pin_check::plan_line`] uses. The
/// geometry comes from Qt rather than from the attached `Screen` object for the reason
/// spelled out in `PinWindow.qml`: which screen a window is on is resolved from its x/y/w/h,
/// and a width divided by a ratio read from that resolution is a binding loop the engine
/// reports instead of solving. These four numbers are the *primary* screen's, in
/// device-independent pixels, which is the space a `Window`'s geometry lives in.
pub fn anchor_line() -> String {
    if !state::hint_surface() {
        return String::new();
    }
    let (px, py, pw, ph) = shim::primary_bounds();
    let hold = with(|p| p.hold);
    format!("{px}|{py}|{pw}|{ph}|{}|{hold}", shim::device_pixel_ratio())
}

/// The door's plan for the QML leg: `延迟 ms | 贴图 id | 保存路径`. Empty outside a
/// `--hint` run.
pub fn plan_line() -> String {
    with(|p| {
        if p.dir.as_os_str().is_empty() {
            return String::new();
        }
        format!("{}|{}|{}", LEG_DELAY_MS, p.id, p.file.display())
    })
}

/// The lifetime this run was asked for, written by `main` before the engine exists.
pub fn set_hold(ms: i32) {
    with(|p| p.hold = ms);
}

/// The same number on the way out, for the footer line. Printing [`HOLD_MS`] there instead
/// would be the report disagreeing with the run it describes: the QML side was handed the
/// command line's value, and 「短暂」's two halves are told apart by exactly that number.
pub fn hold() -> i32 {
    with(|p| p.hold)
}

/// One line the hint window read off its own door.
///
/// The first one is stamped: 「短暂」 is measured as an interval from the moment a sentence
/// arrived to the moment the surface was photographed, and the only honest clock for that is
/// this process's own. Nothing here arms the run's end - a `--hint` run exists *to* wait, and
/// the leg's one call has no tail to catch, so `main`'s floor is the closer.
pub fn report(line: &str) {
    with(|p| {
        if p.lines.is_empty() {
            state::stamp("hint shown");
        }
        p.lines.push(line.to_string());
    });
}

/// Plant the pin the leg will save and the directory it will save into. Must run before the
/// QML engine, for the reason [`plan_line`] exists.
pub fn plant() -> Result<String, String> {
    state::stamp("hint arm");
    let mut dir = std::env::temp_dir();
    dir.push(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(NAME);
    let _ = std::fs::remove_file(&file);
    let id = state::add_demo(320, 200)?;
    with(|p| {
        p.dir = dir.clone();
        p.file = file.clone();
        p.id = id;
    });
    Ok(format!("贴图 {id} 将保存到 {}", file.display()))
}

/// The one top-level this door is about, out of the list Qt really made.
fn hint_window() -> Option<crate::r13::Top> {
    parse_levels(&shim::top_levels())
        .into_iter()
        .find(|t| t.title == TITLE)
}

/// The shim's field dump for that window, as the whitespace-separated tokens
/// `pin_shim.cpp`'s `describeWindow` prints into `pinWindowReport`. `None` when the window is
/// not in the report at all, which is a different absence from "the report says 0": it means
/// the run that asked for a hint never got a native window for it.
fn hint_fields() -> Option<String> {
    let report = shim::window_report();
    report
        .split("\n  ")
        .find(|block| block.contains(&format!("title={TITLE}")))
        .map(|block| block.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// `FLAG=1` out of a field dump, or `None` when the flag is not in the dump at all.
///
/// Missing is not 0: a shim that stopped reporting `NOACTIVATE` would otherwise read as a
/// window that activates, which is the one thing this row exists to notice.
fn flag(fields: &str, name: &str) -> Option<bool> {
    fields
        .split_whitespace()
        .find(|t| t.starts_with(&format!("{name}=")))
        .and_then(|t| t.strip_prefix(&format!("{name}=")))
        .and_then(|v| v.parse::<i32>().ok())
        .map(|v| v != 0)
}

/// The primary screen's rectangle in *device* pixels, from the same two numbers a pin's
/// geometry was built from - Qt's DIP rect and the primary's ratio - so the comparison in
/// [`measure`] is between two readings of one conversion rather than between a DIP rect and a
/// `GetWindowRect` one.
fn primary_phys() -> PhysRect {
    let (x, y, w, h) = shim::primary_bounds();
    let d = shim::device_pixel_ratio().max(0.01);
    PhysRect::new(
        (x as f64 * d).round() as i32,
        (y as f64 * d).round() as i32,
        ((w as f64 * d).round() as i32).max(1) as u32,
        ((h as f64 * d).round() as i32).max(1) as u32,
    )
}

/// Count the distinct opaque-ish colours a bitmap drew.
///
/// Keyed by the RGB triple alone, with the alpha channel left out: a translucent card painted
/// on a transparent surface keeps one (r,g,b) at two different alphas, and that is the flat
/// fill this row is trying to exclude, not a piece of text.
fn distinct_ink(pixels: &[u8]) -> usize {
    let mut seen: Vec<[u8; 3]> = Vec::new();
    for px in pixels.as_chunks::<4>().0 {
        if px[3] < 128 {
            continue;
        }
        let rgb = [px[0], px[1], px[2]];
        if !seen.contains(&rgb) {
            seen.push(rgb);
        }
    }
    seen.len()
}

/// How many pixels the bitmap actually covers, for the same row to print next to the count.
fn covered(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] >= 128)
        .count()
}

/// Read the surface back and grade it. Called after the event loop has run, while this
/// process's windows are still up - the arrangement every other gauge in this crate uses.
pub fn measure() -> (Check, String) {
    state::stamp("hint read");
    let lines = with(|p| p.lines.clone());
    let hold = with(|p| p.hold);
    let want = with(|p| p.file.clone());

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    rep.row(
        "hint leg",
        Check::from(!lines.is_empty()),
        format!(
            "{} 行来自浮窗自己的腿；起步用了 {:?} ms；原文 {}",
            lines.len(),
            state::between("hint arm", "hint shown"),
            lines.join(" ⏎ ")
        ),
    );

    // Qt 自己的话，原样印出来。这一档判的是"窗在不在场上"，而 QML 里一条接错的名字、一个
    // 解析不了的属性，报出来的也是"窗不在场"——不加这一行，这两件事在同一份日志里长一样。
    // 它是 `note` 而不是判定行：一个 `warn:` 是不是这一档的缺陷，要人来读（`--probe` 那一档
    // 已经把它判成失败，这是本 crate 里唯一一处这样用的地方，不该由这里复制）。
    let msgs = state::messages();
    let bad: Vec<&str> = msgs
        .lines()
        .filter(|line| line.starts_with("warn:") || line.starts_with("error:"))
        .collect();
    rep.note(
        "hint messages",
        if bad.is_empty() {
            "Qt 没有报过 warn/error".to_string()
        } else {
            format!("{} 条：{}", bad.len(), bad.join(" ⏎ "))
        },
    );

    let saved = said(&lines, "saved").unwrap_or("");
    // 判的是产品那句话的两个端点：它得是那句成功话（`state::save_image` 写进 `status` 的
    // 「已保存到 …」），并且它得点名这一档要的文件。只比前缀是拿自己写的句子当判据，而
    // 路径对上才是"这条腿真的驱动了一次落盘"的证据。
    let named = saved.contains(NAME);
    rep.row(
        "saved sentence",
        Check::from(saved.starts_with("已保存到") && named),
        format!(
            "{saved:?}（产品自己的那句话，取自 savePin 的返回；提到 {} ={named}）",
            NAME
        ),
    );

    let Some(win) = hint_window() else {
        // 没有这扇窗，下面每一格都只是「这一档没跑」的另一种说法，所以它们仍然全部发出来
        // （§10 第 48 项 ⑯ 的理由在这条腿上原样成立：缺窗的那一趟欠的格子和有窗的一样多）。
        // 顺序也一致——读日志的人拿这一份和拿一份跑通的对照时，不该先对一遍行名。
        let rest = format!("本进程没有标题为 {TITLE} 的顶层窗");
        for label in [
            "hint window",
            "hint lifetime",
            "hint styles",
            "hint focus",
            "hint on screen",
            "hint bitmap",
            "hint ink",
            "hint palette",
        ] {
            rep.row(label, Check::Fail, rest.clone());
        }
        // 这一格不发那条句子：文件在不在磁盘上是一件事，浮窗开没开是另一件，把前者写成后者
        // 的附庸就少了一条能分辨"产品存了但没说话"的证据。
        rep.row(
            "hint file",
            Check::from(want.exists()),
            format!("{} {}", want.display(), bytes(&std::fs::read(&want).ok())),
        );
        return rep.finish("hint window");
    };
    let Some(fields) = hint_fields() else {
        // The window is in Qt's list but not in the field dump: one of the two readers is
        // describing a window the other never saw, which is the shim's problem rather than
        // the product's - and it is said here rather than folded into the row above.
        rep.row(
            "hint window",
            Check::Pass,
            format!(
                "hwnd=0x{:x} class={} visible={} phys={:?}",
                win.hwnd, win.class, win.visible, win.phys
            ),
        );
        let why = format!("字段清单里没有这扇窗（title={TITLE}），一格都读不到");
        for label in ["hint lifetime", "hint styles", "hint focus"] {
            rep.row(label, Check::Fail, why.clone());
        }
        geometry_rows(&mut rep, &win);
        for label in ["hint bitmap", "hint ink", "hint palette"] {
            rep.row(label, Check::Fail, why.clone());
        }
        rep.row(
            "hint file",
            Check::from(want.exists()),
            format!("{} {}", want.display(), bytes(&std::fs::read(&want).ok())),
        );
        return rep.finish("hint window");
    };

    rep.row(
        "hint window",
        Check::Pass,
        format!(
            "hwnd=0x{:x} class={} visible={} phys={:?} {}",
            win.hwnd, win.class, win.visible, win.phys, fields
        ),
    );

    // 这一格是「短暂」本身。它按本次的 hold 与读回时刻的间隔决定期待哪一面，并把期待的那一面
    // 写在句子里——一趟跑只能量到一面，所以批处理里这一档点两次（hold 长于跑、hold 短于跑）。
    let elapsed = state::between("hint shown", "hint read");
    // 期待的那一面提到外面来，因为两行都要它：下面这行判它，`hint ink` 靠它知道这张照片该不该
    // 有字。一条规则写两遍就是下一对会漂移的副本。
    let expect_up = elapsed.is_some_and(|ms| ms < hold as u64);
    match elapsed {
        None => rep.row(
            "hint lifetime",
            Check::Fail,
            "腿没有报过任何一行，无从计算间隔".to_string(),
        ),
        Some(ms) => rep.row(
            "hint lifetime",
            Check::from(win.visible == expect_up),
            format!(
                "本次量的是「{}」：从句子到读回隔了 {ms} ms 对 hold {hold} ms，应当在{}，实际 visible={}",
                if expect_up { "还在" } else { "已收" },
                if expect_up { "场上" } else { "离场" },
                win.visible as i32
            ),
        ),
    }

    let want_flags = [
        // FRAMELESS 在 shim 里的定义是"不是子窗"（`pin_shim.cpp` 的 `(style & WS_CHILDWINDOW)`），
        // 所以对任何顶层窗它都是 1：这一行真正证明"无边框"的是下面那枚 CAPTION=0。两者都留着，
        // 但把话说到 CAPTION 上，免得有人把这格读成它没有说的那件事。
        ("FRAMELESS", Some(true)),
        ("CAPTION", Some(false)),
        ("TOOLWINDOW", Some(true)),
        ("NOACTIVATE", Some(true)),
        ("APPWINDOW", Some(false)),
        // 这扇窗自己声明了 StaysOnTop，所以它上了判据：声明而不量的旗位，就是下一趟会变脏
        // 的那一种。LAYERED 只作读数——它是 Qt 为逐像素 alpha 自己决定要不要给的那一枚。
        ("TOPMOST", Some(true)),
        ("LAYERED", None),
    ];
    let mut wrong: Vec<String> = Vec::new();
    let mut read: Vec<String> = Vec::new();
    for (name, expect) in want_flags {
        let got = flag(&fields, name);
        read.push(format!(
            "{name}={}",
            got.map_or("?".to_string(), |v| (v as i32).to_string())
        ));
        if let (Some(want), Some(got)) = (expect, got) {
            if want != got {
                wrong.push(format!("{name} 期望 {} 实到 {}", want as i32, got as i32));
            }
        }
    }
    let missing: Vec<&str> = want_flags
        .iter()
        .filter(|(name, expect)| expect.is_some() && flag(&fields, name).is_none())
        .map(|(name, _)| *name)
        .collect();
    rep.row(
        "hint styles",
        Check::from(wrong.is_empty() && missing.is_empty()),
        if missing.is_empty() {
            format!(
                "{}{}",
                read.join(" "),
                if wrong.is_empty() {
                    "".to_string()
                } else {
                    format!(" - {}", wrong.join("；"))
                }
            )
        } else {
            format!(
                "shim 没有报告 {}（这一格不能靠缺字段蒙过去）",
                missing.join(" / ")
            )
        },
    );

    match platform_windows::proc::foreground_window() {
        None => rep.row(
            "hint focus",
            Check::Blocked,
            "shell 没有回答当前前台窗，这一格是机器不给读，不是产品抢了焦点".to_string(),
        ),
        Some(fg) => {
            let who = platform_windows::proc::pid_of_window(fg)
                .and_then(platform_windows::proc::process_name)
                .unwrap_or_else(|| "未知进程".to_string());
            rep.row(
                "hint focus",
                Check::from(fg != win.hwnd),
                format!(
                    "前台窗是 {who}（hwnd=0x{fg:x}），浮窗 hwnd=0x{:x} - 提示不该成为前台",
                    win.hwnd
                ),
            );
        }
    }

    geometry_rows(&mut rep, &win);

    match platform_windows::print::print_window(win.hwnd, win.phys.w, win.phys.h, 2) {
        None => {
            rep.row(
                "hint bitmap",
                Check::Blocked,
                "PrintWindow(flag 2) 没有还回位图".to_string(),
            );
            rep.row(
                "hint ink",
                Check::Blocked,
                "没有位图，这一格连样本都没有（不是没画字）".to_string(),
            );
            // P119：这一格此前在这一支**不发**，而本档自己的规则写在缺窗那一支的头上
            // （「缺窗的那一趟欠的格子和有窗的一样多」）：PrintWindow 还不回来位图时，
            // `hint palette` 与上面两格是同一个「没有样本」，不是「判过了」。少发一行的代价不是
            // 少一行字，而是下一趟读的人拿两份档对行名时对不上 —— 那一格会像被人跳过了。
            // 期待值仍由 [`palette_row`] 自己算，这里不走那一枚函数而是要它那一支的句子，
            // 因为它的 `!expect_up` 臂写的理由是「收之后的窗」，与这一支的理由不同。
            rep.row(
                "hint palette",
                Check::Blocked,
                format!(
                    "Rust 判 {:?}；PrintWindow 没有还回位图，餐具无从可问（不是没画）",
                    crate::settings::startup_theme_face()
                ),
            );
        }
        Some(frame) => {
            let n = covered(&frame.pixels);
            rep.row(
                "hint bitmap",
                Check::from(n > 0),
                format!("{}x{} 覆盖像素 {n} 个", frame.width, frame.height),
            );
            let kinds = distinct_ink(&frame.pixels);
            // 「已收」那一支里这一格没有东西可判：按这条分支自己的口径窗已经离场，而 PrintWindow
            // 拍的是一张不在场的窗——`p54_hint_g.log` 覆盖像素照旧 106720 个，颜色只有 1 种。把
            // 「屏幕答不了」写成 FAIL 会让下一趟读的人去找一个不存在的渲染缺陷，所以它写成 SKIP，
            // 整趟的出口代码因此是 3 而不是 1。画没画字由「还在」那一支判（`p54_hint_f.log`：4607 种）。
            rep.row(
                "hint ink",
                if expect_up {
                    Check::from(kinds >= INK_MIN_DISTINCT)
                } else {
                    Check::Blocked
                },
                format!(
                    "不同的颜色 {kinds} 种，地板是 {INK_MIN_DISTINCT} 种（{}）",
                    if expect_up {
                        "一块平涂的卡片只有它自己和边框"
                    } else {
                        "本支量的是收之后的窗：不在场的窗拍不出字，这一格交给「还在」那一支"
                    }
                ),
            );
            palette_row(&mut rep, &frame.pixels, expect_up);
        }
    }

    rep.row(
        "hint file",
        Check::from(want.exists()),
        format!("{} {}", want.display(), bytes(&std::fs::read(&want).ok())),
    );

    rep.finish("hint window")
}

/// The one geometry row, emitted from both read paths so a window that is in the top-level
/// list but missing from the field dump still says where it is.
fn geometry_rows(rep: &mut Report, win: &crate::r13::Top) {
    let desk = primary_phys();
    let inside = win.phys.x >= desk.x - GEOM_SLACK
        && win.phys.y >= desk.y - GEOM_SLACK
        && win.phys.right() <= desk.right() + GEOM_SLACK
        && win.phys.bottom() <= desk.bottom() + GEOM_SLACK;
    // 「底部」用主屏高度的四分之一来判，而不是用 QML 里那条横幅的尺寸：把版面数字抄进 Rust
    // 就是§10 反复说的那种「两处各写一次」的数，而浮窗该在下方这件事不需要精确到像素。
    let lower = win.phys.bottom() >= desk.bottom() - (desk.h as i32 / 4);
    rep.row(
        "hint on screen",
        Check::from(inside && lower),
        format!(
            "浮窗 {:?} 主屏(设备像素) {:?} 在主屏内={} 下四分之一内={} 距主屏底 {} px",
            win.phys,
            desk,
            inside,
            lower,
            desk.bottom() - win.phys.bottom()
        ),
    );
}

/// §5.19.1 步骤 2 的那一行：屏幕上这一套是哪一套，和 Rust 判的是不是同一套。
///
/// Two readings meet here and neither is the other's guess - #281 的同族：「显示的那份」与
/// 「应用的那份」必须在一枚档里并判。[`crate::settings::startup_theme_face`] 交回的期待值与
/// `[theme cfg]` 那一行、`Session.theme` 那枚 qproperty 走的是**同一趟**注册表读数（P119 起由
/// `OnceLock` 担保，此前这句只是注释）；占多的那一枚不透明色是照片自己答的。一枚 binding 悄悄
/// 永不触发 —— `theme` 拼错、`===` 对每个值都是 false、`refreshTheme()` 没人叫 —— 屏幕上会留着
/// 浅色卡而 Rust 说 `dark`，这一格就是为那一种反面而存在。
///
/// **这一格的红侧只在机器答 dark 的那台机器上可达**：浅色机上「未初始化」与「正确」两臂都读成
/// 浅色，判据两侧同色 ⇒ 它绿得没有内容（#232 那一族）。这不是这一格的缺陷而是它的对偶 ——
/// `system.theme=` 与 `Rust 判 {face:?}` 都印在句子里，读的人第一眼就知道这一趟有没有资格当证人；
/// 本档 P119 的两趟门都跑在一台 `system.theme=dark` 的机器上，那一侧是可达的。
///
/// The grade is [`LUMA_SPLIT`]'s comparison and not an RGB equality, because the card is
/// translucent: what PrintWindow hands back may be premultiplied, may be composited over the
/// desktop, and is in any case this file's guess if it is pinned to the literal. The two faces
/// sit 223 apart on that scale, so the ordering is the part of the claim a photograph can answer.
/// The exact match is still printed, just not judged - a drift to a third colour would show as
/// `逐格相等=false` beside a green, which is the shape that gets read rather than assumed.
///
/// A window that has already been taken away photographs no palette at all, so that arm is
/// [`Check::Blocked`] for the same reason [`measure`]'s `hint ink` row gives it.
fn palette_row(rep: &mut Report, pixels: &[u8], expect_up: bool) {
    let face = crate::settings::startup_theme_face();
    let expect_dark = face == "dark";
    let card = if expect_dark { CARD_DARK } else { CARD_LIGHT };
    let say = format!(
        "Rust 判 {face:?} → 卡片 {card:?}（亮度 {}，{}）；分界 {LUMA_SPLIT}",
        luma(card),
        if expect_dark { "深色" } else { "浅色" }
    );
    if !expect_up {
        rep.row(
            "hint palette",
            Check::Blocked,
            format!("{say} - 本支量的是收之后的窗，照片里没有餐具可问，这一格交给「还在」那一支"),
        );
        return;
    }
    match dominant_ink(pixels) {
        None => rep.row(
            "hint palette",
            Check::Blocked,
            format!("{say} - 位图里一枚不透明色都没有：没有样本，不是判错"),
        ),
        Some((rgb, n)) => {
            let got_dark = luma(rgb) < LUMA_SPLIT;
            // 总数由这一格自己数，不借 `hint ink` 那一格的样本：两格问的是同一张图的两侧，
            // 而占比要跟这一格的 `n` 同一趟、同一枚数组。上面那一支 `Some` 保证 `n >= 1`，
            // 所以 `total` 不会是 0 —— 这里不再为它加分叉。
            let total = pixels.as_chunks::<4>().0.len();
            rep.row(
                "hint palette",
                Check::from(got_dark == expect_dark),
                format!(
                    "屏幕这套：最占多的不透明色 {rgb:?}（{n}/{total} 像素，占比 {}%，亮度 {}，{}）；{say}；同一面={}（判的是分界两侧这一枚 bit，不是四枚颜色）逐格相等={}",
                    n * 100 / total,
                    luma(rgb),
                    if got_dark { "深色" } else { "浅色" },
                    got_dark == expect_dark,
                    rgb == card
                ),
            );
        }
    }
}

fn bytes(v: &Option<Vec<u8>>) -> String {
    v.as_ref()
        .map(|b| format!("{} B", b.len()))
        .unwrap_or_else(|| "没有文件".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `anchor_line` is the one place a QML window's lifetime and screen come from, so its
    /// field count and order are a contract with the other language. Written as a shape test
    /// rather than a value test: the values are this machine's desktop.
    #[test]
    fn the_anchor_line_is_six_fields_or_nothing() {
        let empty = String::new();
        assert_eq!(empty.split('|').count(), 1, "空串就是空串");

        state::set_hint_surface(true);
        let line = anchor_line();
        let parts: Vec<&str> = line.split('|').collect();
        assert_eq!(parts.len(), 6, "{line:?} 不是六格");
        for p in &parts[..4] {
            assert!(p.parse::<i32>().is_ok(), "{p:?} 不是像素数");
        }
        let dpr: f64 = parts[4].parse().expect("dpr 不是数");
        assert!(
            (0.01..=8.0).contains(&dpr),
            "dpr {dpr} 不在任何缩放的范围内"
        );
        assert_eq!(parts[5].parse::<i32>(), Ok(HOLD_MS));
        state::set_hint_surface(false);
        assert!(anchor_line().is_empty(), "不该有浮窗的运行报出了锚点");
    }

    /// A flag the shim stopped reporting must not read as a flag that is off.
    #[test]
    fn a_missing_field_is_not_a_zero() {
        let fields = "class=QWindow visible=1 FRAMELESS=1 CAPTION=0 TOOLWINDOW=1";
        assert_eq!(flag(fields, "FRAMELESS"), Some(true));
        assert_eq!(flag(fields, "CAPTION"), Some(false));
        assert_eq!(flag(fields, "NOACTIVATE"), None);
    }

    /// The ink metric's two ends: a flat card is not ink, and anything with real glyph
    /// antialiasing is.
    #[test]
    fn distinct_colours_separate_a_flat_fill_from_text() {
        let flat: Vec<u8> = (0..400).flat_map(|_| [18u8, 21, 27, 230]).collect();
        assert_eq!(distinct_ink(&flat), 1);
        assert_eq!(covered(&flat), 400);

        let mut mixed: Vec<u8> = Vec::new();
        for i in 0..400 {
            let ink = i % 7 == 0;
            mixed.extend_from_slice(if ink {
                &[242, 244, 247, 255]
            } else {
                &[18, 21, 27, 230]
            });
        }
        assert_eq!(distinct_ink(&mixed), 2);

        // Nothing above the alpha cut counts, so a window that painted only transparency
        // reports one colour - which is the all-transparent PrintWindow answer this row
        // exists to notice.
        let clear: Vec<u8> = (0..400).flat_map(|_| [0u8, 0, 0, 0]).collect();
        assert_eq!(distinct_ink(&clear), 0);
        assert_eq!(covered(&clear), 0);
    }

    /// The title is matched as a whole field against a whitespace-split list, so a space in
    /// it would silently match nothing.
    #[test]
    fn the_title_survives_the_field_splitter() {
        assert!(!TITLE.contains(' '), "{TITLE} 不能有空格");
        assert!(TITLE.starts_with("falconshot-"), "标题得认得出是谁的窗");
    }

    /// `LUMA_SPLIT` 这一枚分界之所以可以是一枚常数，唯一的理由是两套餐具离它都远。把这一格
    /// 写成断言而不是注释：改 `CARD_DARK` / `CARD_LIGHT` 里的任何一枚，本格当场断，而
    /// `hint palette` 那一行不会跟着改口。
    ///
    /// 两枚预乘过的值一并算进来，因为 PrintWindow 还回来的是哪一种是这台机器的决定：预乘是
    /// `E6`(=230/255) 一级缩放，它对亮度差的影响是乘一个系数，而系数不改变谁在分界哪一边 ——
    /// 前提是两枚 alpha 相同，所以这里同时判这一点。
    ///
    /// P119：「算进来」这一句此前只是注释 —— 这一格一枚预乘也没算过，而 [`LUMA_SPLIT`] 的注释
    /// 拿它当理由。现在它真算，并把注释里那四个数一并钉成断言：那枚注释当时写的是
    /// 21/107/222/19/220，而 [`luma`] 自己答 20/108/223/18/219（整数除不是四舍五入）。
    #[test]
    fn the_two_card_fills_land_on_opposite_sides_of_the_split() {
        let d = luma(CARD_DARK);
        let l = luma(CARD_LIGHT);
        assert_eq!((d, l), (20, 243), "亮度本身漂了，分界那一套理由要重写");
        assert!(d < LUMA_SPLIT, "深色卡片亮度 {d} 越过了分界 {LUMA_SPLIT}");
        assert!(l >= LUMA_SPLIT, "浅色卡片亮度 {l} 没过分界 {LUMA_SPLIT}");
        // 余量：离分界最近的那一边也还剩多少。108 与 115 是这两个数自己的读数。
        assert_eq!(
            (LUMA_SPLIT - d, l - LUMA_SPLIT),
            (108, 115),
            "两侧余量不再是注释里那两个数"
        );
        assert!(
            LUMA_SPLIT - d >= 100,
            "深色一侧的余量只剩 {}",
            LUMA_SPLIT - d
        );
        assert!(
            l - LUMA_SPLIT >= 100,
            "浅色一侧的余量只剩 {}",
            l - LUMA_SPLIT
        );
        assert_eq!(l - d, 223, "两套餐具的间距不是注释里那枚数");
        assert!(l - d >= 200, "两套餐具只差 {}，照片分不开", l - d);

        // 预乘过的那两种读数也要各自待在自己那一侧：`E6` 一级缩放。
        let pd = luma(premultiply(CARD_DARK, 0xE6));
        let pl = luma(premultiply(CARD_LIGHT, 0xE6));
        assert_eq!((pd, pl), (18, 219), "预乘后的亮度不是注释里那两个数");
        assert!(pd < LUMA_SPLIT, "预乘过的深色 {pd} 翻过了分界");
        assert!(pl >= LUMA_SPLIT, "预乘过的浅色 {pl} 没过分界");

        // 两枚 card_fill 的 alpha 必须相同，否则缩放系数不一样，上面那三枚余量就不归 RGB 管。
        let fills: Vec<&str> = HINT_QML
            .lines()
            .filter(|line| line.contains("card_fill:"))
            .collect();
        assert_eq!(fills.len(), 1, "card_fill 那一行现在不止一处：{fills:?}");
        let mut arms: Vec<String> = Vec::new();
        for (i, _) in fills[0].match_indices('"') {
            let s: String = fills[0][i + 1..].chars().take(9).collect();
            let hex = s.strip_prefix('#').unwrap_or("");
            if hex.len() == 8 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                arms.push(s);
            }
        }
        assert_eq!(arms.len(), 2, "card_fill 的两个臂读成 {arms:?}");
        assert_eq!(
            &arms[0][1..3],
            &arms[1][1..3],
            "两套餐具的 alpha 不同了：{arms:?}"
        );
    }

    /// 调色板只能长在 `session.theme` 上。这一格是源码扫描：`HintWindow.qml` 里凡是带引号的
    /// 十六进制颜色，只许出现在写了 `dark ?` 的那四行上。任何一处把字面量抄回 `color:`，就是
    /// §5.19.1 那句「按主题切换」在源码里的反面 —— 而运行期的 `hint palette` 只量得到卡片
    /// 那一枚主色，量不到边框、字色与提醒色有没有各自逃走。
    #[test]
    fn the_hint_palette_is_bound_to_the_theme_and_written_once() {
        assert!(
            HINT_QML.contains("readonly property bool dark: hint.session.theme === \"dark\""),
            "深色这一问没有绑在 session.theme 上"
        );
        let lit: Vec<&str> = HINT_QML
            .lines()
            .filter(|line| line.contains("\"#"))
            .collect();
        assert_eq!(
            lit.len(),
            4,
            "带引号的颜色行现在是 {} 行：{lit:?}",
            lit.len()
        );
        for line in &lit {
            assert!(line.contains("dark ?"), "这一行的颜色不跟主题走：{line}");
            assert!(
                line.contains("readonly property color"),
                "这一行不是调色板格：{line}"
            );
        }
        assert_eq!(HINT_QML.matches("\"#").count(), 8, "四行应该是八枚字面量");
        // 旧的四处字面量不许留在别处当第五套餐具。四枚是**四个色位**各一条：卡片、边框、字、
        // 提醒 —— P119 之前这里逐枚点了三枚而标题写「四处」（#310 那一族：枚数与点名各说各话），
        // 少的那一枚正是边框，而它是四枚里唯一一处 `border.color:` 的形状。
        for old in [
            "color: \"#E612151B",
            "border.color: \"#33596B\"",
            "color: \"#F2F4F7\"",
            "color: \"#F2B04C",
        ] {
            assert!(!HINT_QML.contains(old), "还有一处直接写在场上：{old}");
        }
    }

    /// `dominant_ink` 的两端：一张平涂卡片要答得出是哪一套，一半深一半浅的窗不能被读成绿的。
    #[test]
    fn the_dominant_colour_is_the_card_and_not_the_noise() {
        let dark_card: Vec<u8> = (0..1000).flat_map(|_| [18u8, 21, 27, 230]).collect();
        assert_eq!(dominant_ink(&dark_card), Some(([18, 21, 27], 1000)));
        assert!(luma([18, 21, 27]) < LUMA_SPLIT);

        // 字是几百枚抗锯齿步阶、边框是一整圈，卡片仍是占多的那一枚。
        let mut with_text = dark_card;
        for _ in 0..200 {
            with_text.extend_from_slice(&[242u8, 244, 247, 255]);
            with_text.extend_from_slice(&[51u8, 0x59, 0x6B, 255]);
        }
        assert_eq!(dominant_ink(&with_text), Some(([18, 21, 27], 1000)));

        // 一枚都没有：透明的窗，这一格是 Blocked 而不是判错。
        let clear: Vec<u8> = (0..400).flat_map(|_| [0u8, 0, 0, 0]).collect();
        assert_eq!(dominant_ink(&clear), None);

        // 半深半浅：占多的那一枚只有 400，另一半 300，读数仍写第一枚 —— 而这一趟的
        // `n` 与总数一起印出来，读的人看得见它并不是一整张卡片。
        let mut mixed: Vec<u8> = (0..400).flat_map(|_| [18u8, 21, 27, 230]).collect();
        mixed.extend((0..300).flat_map(|_| [242u8, 244, 247, 255]));
        assert_eq!(dominant_ink(&mixed), Some(([18, 21, 27], 400)));

        // 真平手：`max_by_key` 取最后遇到的那一枚，所以赢的是**浅色**。这一格把 `LUMA_SPLIT`
        // 注释里那句「平手取后见」钉住（此前它写的是先见，而这一支零枚读者）。
        let mut tie: Vec<u8> = (0..300).flat_map(|_| [18u8, 21, 27, 230]).collect();
        tie.extend((0..300).flat_map(|_| [242u8, 244, 247, 255]));
        assert_eq!(dominant_ink(&tie), Some(([242, 244, 247], 300)));
    }

    /// `E6` 一级缩放，与 PrintWindow 可能还回来的那一种预乘存储同式：每个通道乘 alpha 再除
    /// 255（整数除，与 [`luma`] 一样不做四舍五入）。
    fn premultiply(px: [u8; 3], alpha: u8) -> [u8; 3] {
        let one = |v: u8| (u32::from(v) * u32::from(alpha) / 255) as u8;
        [one(px[0]), one(px[1]), one(px[2])]
    }

    /// `Connections` 里一枚 handler 的名字若对不上目标对象的任何信号，Qt **不报错也不调用**：
    /// 它往 stderr 印一行 `warn:` 就完了，而那一行在这一档的所有判据之外。P109 那条「倒计时归零
    /// ⇒ 收走屏幕上那一句」正是这样死了十轮，最后由 P119 新加的 `[watch]` 腿在 `--autosave`
    /// 门档里第一次把它读出来（`HintWindow.qml:166:5: … no signal of the target matches the
    /// name`）。这一格把那种形状做成源码扫描，理由与 `pin window pixels` 那几格一样：运行期
    /// 证人只有门跑起来才在场，而门不是每次都跑。
    ///
    /// 尺子是 `Session` 自己声明的 qproperty 清单，不是本文件抄的一份名字表：cxx-qt 生成 notify
    /// 时**不做驼峰化**，`#[qproperty(i32, delay_left)]` 的信号就叫 `delay_leftChanged`，而 Qt
    /// 匹配 handler 时只把 `on` 之后第一枚字母的大小写归一。于是每一枚带下划线的那一格都有两种
    /// 写法（`ondelay_leftChanged` / `onDelay_leftChanged`）而只有**一种**驼峰到底的写法是死的 ——
    /// 这一格判的正是那一枚死的：它若在场，说明有人给一扇窗挂了一条永远不会跑的腿。
    #[test]
    fn a_camel_cased_session_notify_never_hangs_on_a_window() {
        let mut underscored: Vec<String> = Vec::new();
        // `match_indices` 交回来的是 `(字节下标, 匹配到的那一枚模式本身)`，**不是**模式之后的
        //  remainder。P119 首跑把第二个字段当成了尾巴，于是 `tail.find(')')` 恒为 `None`、
        // 每一枚都 `continue`，尺子读回空集（#269 那一族的 Rust 版）。下面按字节下标切尾巴，
        // 切点与 `#[` 同为 ASCII，字符边界不成问题。
        const OPEN: &str = "#[qproperty(";
        for (at, _) in SESSION_SRC.match_indices(OPEN) {
            let tail = &SESSION_SRC[at + OPEN.len()..];
            let Some(close) = tail.find(')') else {
                continue;
            };
            let inner = &tail[..close];
            // `#[qproperty(i32, delay_left)]` 的第二个字段是 Rust 那侧的名字，原样进 moc。
            if let Some((_, name)) = inner.split_once(',') {
                let name = name.trim();
                if name.contains('_') {
                    underscored.push(name.to_string());
                }
            }
        }
        assert!(
            !underscored.is_empty(),
            "`Session` 读不出带下划线的 qproperty，这一格就成了空集上的绿（#269）"
        );
        assert!(
            underscored.contains(&"delay_left".to_string()),
            "尺子里没有 delay_left：{underscored:?}"
        );
        // 尺子读了哪几枚，当场印出来：这一格的绿若只由「不在场」构成，读者看不见它量的是几枚
        // 名字，也就无从发现它其实一枚都没读到（#271：标签里的数没有归档读数供它）。
        println!(
            "ruler: {} underscored qproperties on Session -> {underscored:?}",
            underscored.len()
        );

        for (face, text) in QML_FACES {
            // 判**代码**而不是整档文本。P119 首跑按原文 `contains` 扫，被本文件里那段
            // 记录 Qt 那行 `warn:` 的引文判红 —— 引文不是 handler（#313/#253/#268 同一族：
            // 把逐字引用当活句判）。注释之外的形状才是 Qt 会去接线的那些。
            let code = qml_code(text);
            println!(
                "strip {face}: 原文 {} 字符 -> 去注释 {} 字符（去掉 {}）",
                text.chars().count(),
                code.chars().count(),
                text.chars().count() - code.chars().count()
            );
            for name in &underscored {
                let camel = camel_case(name);
                let dead = format!("on{camel}Changed");
                assert!(
                    !code.contains(&dead),
                    "{face} 的代码里挂着 {dead}：那一枚信号在 Qt 场上不存在，handler 永不触发"
                );
            }
        }
        // 剥注释这一腿自己的反证：它若把 handler 一起剥了，上面那一串 `!contains` 就全是空集
        // 上的绿（#269）。所以当场再问一次——活的名字必须活在**剥过之后**的文本里。
        let hint_code = qml_code(HINT_QML);
        assert!(
            hint_code.len() < HINT_QML.len(),
            "本文件去注释前后一样长 ⇒ 这条腿从未真的在剥"
        );
        // 被点名的那一格当场再判一次它自己的活形状，免得这一趟只是「旧名字不在了」。
        assert!(
            HINT_QML.contains("readonly property bool counting: hint.session.delay_left > 0"),
            "收窗那条腿没有绑在 delay_left 的值上"
        );
        // P119 第 #321 封信：上一格问的是「串在不在档里」，而 `function onCountingChanged()`
        // 当时**在**、判据当场绿、Qt 却在运行期印出同一形状的第二封信
        // （`HintWindow.qml:178:5: … Detected function "onCountingChanged" in Connections
        // element`）。`Connections` 拿 `on…` 前缀去问它的 `target`，而 `counting` 声明在本窗
        // 的根上、从来不是 Session 的信号 —— 一条 handler 长在哪个对象里，和它叫什么名字，
        // 是两件独立的事。这一格自此问的是**归属**：那条 handler 既不能在任何一枚
        // `Connections { … }` 的括号里，也必须坐在根对象那一层缩进上。
        const LIVE: &str = "onCountingChanged:";
        let spans = connections_spans(&hint_code);
        println!(
            "connections: HintWindow.qml 里 {} 枚 Connections 跨度 -> {spans:?}",
            spans.len()
        );
        // 跨度尺子自己的反证：本档确有 Connections，读回空集就是「不在任何括号里」这一问
        // 成了空集上的真（#269 同一族）。
        assert!(
            !spans.is_empty(),
            "跨度尺子读回空集：本档确有 Connections，那条「不在括号里」遂成恒真"
        );
        let Some(at) = hint_code.find(LIVE) else {
            panic!("`counting` 翻了面而没人接：根上没有 {LIVE} 那条 handler");
        };
        let line_start = hint_code[..at].rfind('\n').map(|n| n + 1).unwrap_or(0);
        let head = &hint_code[line_start..at];
        let indent = head.chars().filter(|c| *c == ' ').count();
        let in_conn = spans.iter().filter(|(s, e)| *s <= at && at < *e).count();
        println!(
            "handler 归属: 第 {} 行、缩进 {} 空格、落在 {} 枚 Connections 跨度之内",
            hint_code[..at].matches('\n').count() + 1,
            indent,
            in_conn
        );
        assert!(
            in_conn == 0,
            "{LIVE} 挂在 Connections 的括号里：那一枚是 Session 的 target，场上没有 countingChanged"
        );
        assert!(
            head.trim().is_empty() && indent == 4,
            "{LIVE} 不在根对象那一层缩进上（实测缩进 {indent}）：归属这一问还是没有答"
        );
        assert!(
            !hint_code.contains("function onCountingChanged"),
            "根上那条之外还留着一枚 `function` 形状的旧 handler：两处同名、Qt 只接其中一枚"
        );
        // 那段引文自己仍然在场：它不在，说明本档把 P109〜P118 那十轮的死名字抹干净了，
        // 而这条腿的来历就没有可读的落处 —— 判红的一侧与记录的一侧当场各报一次。
        assert!(
            HINT_QML.contains("onDelayLeftChanged"),
            "注释里那段来历不见了：读者看不出这一格为什么存在"
        );
    }

    /// [`qml_code`] 交回的文本里，每一枚 `Connections { … }` 的**字节跨度**（左端是那枚词
    /// 自己，右端是配平之后那一枚 `}` 之后一格）。只认 `Connections` 与 `{` 之间全是空白
    /// 那一种形状：注释虽已剥掉，一枚不相干的词后面跟着不相干的括号仍不该并成一格跨度。
    ///
    /// 找不到的话，「handler 不在任何 Connections 里」就成了空集上的真 —— 所以调用方要么
    /// 当场数出至少一枚，要么在合成反例里自己种一枚（见
    /// [`connections_spans_sees_a_handler_inside_and_not_outside`]）。
    fn connections_spans(code: &str) -> Vec<(usize, usize)> {
        const OPEN: &str = "Connections";
        let bytes = code.as_bytes();
        let mut out = Vec::new();
        let mut at = 0usize;
        while let Some(rel) = code[at..].find(OPEN) {
            let start = at + rel;
            at = start + OPEN.len();
            let Some(rel_brace) = code[at..].find('{') else {
                break;
            };
            if !code[at..at + rel_brace].trim().is_empty() {
                continue;
            }
            let mut depth = 0usize;
            let mut i = at + rel_brace;
            while i < bytes.len() {
                match bytes[i] {
                    b'{' => depth += 1,
                    b'}' if depth > 0 => {
                        depth -= 1;
                        if depth == 0 {
                            out.push((start, i + 1));
                            at = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            if depth != 0 {
                break;
            }
        }
        out
    }

    /// [`connections_spans`] 自己那一腿的反证：跨度尺若把括号里的 handler 读成「不在括号里」，
    /// 上面那一格就恒绿；若把根上那条读成「在括号里」，它就恒红。两种形状一次种在同一枚合成档
    /// 里，各判各的。
    #[test]
    fn connections_spans_sees_a_handler_inside_and_not_outside() {
        let src = "\
Window {
    id: hint
    readonly property bool counting: hint.session.delay_left > 0
    onCountingChanged: hint.shown = false
    Connections {
        target: hint.session
        function onCountingChanged() {
            hint.shown = false
        }
    }
}
";
        let spans = connections_spans(src);
        assert_eq!(
            spans.len(),
            1,
            "合成档里明明种了一枚 Connections：{spans:?}"
        );
        let (s, e) = spans[0];
        let root_at = src.find("onCountingChanged: hint").expect("根上那条不在场");
        let inner_at = src
            .find("function onCountingChanged()")
            .expect("括号里那条不在场");
        assert!(root_at < s, "根上那条被算进了括号的左端之前");
        assert!(
            inner_at >= s && inner_at < e,
            "括号里那条没有被跨度尺看见：跨度={s}..{e}、handler={inner_at}"
        );
        // 散文里一枚不带括号的 `Connections` 不该开跨度：上面那一格正是靠这条把「词」与
        // 「块」分开的，否则一句 `// … Connections …` 之外的引用能吞掉半档。
        assert!(connections_spans("Window {\n property var a: 1\n}\n").is_empty());
    }

    /// 把 QML 档里的注释剥掉，只留代码。行注释 `//…` 到行尾，块注释 `/*…*/` 可嵌套，
    /// 字符串字面量之内的 `//` 不算注释（否则一条 `"http://…"` 会把整行代码吞掉）。
    /// 只认双引号这一枚界符：QML 没有字符字面量，单引号在代码里不是界符，把它当界符反而会让
    /// 一枚落在字符串外的 `'` 把后面半档读成字符串。换行原样保留，剥过的文本仍可按行号对回原档。
    fn qml_code(text: &str) -> String {
        #[derive(PartialEq)]
        enum Mode {
            Code,
            Line,
            Block,
            Str,
        }
        let mut out = String::with_capacity(text.len());
        let mut mode = Mode::Code;
        let mut depth = 0usize;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match &mode {
                Mode::Code => {
                    if c == '/' && chars.peek() == Some(&'/') {
                        chars.next();
                        mode = Mode::Line;
                    } else if c == '/' && chars.peek() == Some(&'*') {
                        chars.next();
                        depth = 1;
                        mode = Mode::Block;
                    } else {
                        if c == '"' {
                            mode = Mode::Str;
                        }
                        out.push(c);
                    }
                }
                Mode::Line => {
                    if c == '\n' {
                        out.push(c);
                        mode = Mode::Code;
                    }
                }
                Mode::Block => {
                    if c == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        depth -= 1;
                        if depth == 0 {
                            mode = Mode::Code;
                        }
                    } else if c == '/' && chars.peek() == Some(&'*') {
                        chars.next();
                        depth += 1;
                    } else if c == '\n' {
                        out.push(c);
                    }
                }
                Mode::Str => {
                    out.push(c);
                    if c == '\\' {
                        if let Some(n) = chars.next() {
                            out.push(n);
                        }
                    } else if c == '"' {
                        mode = Mode::Code;
                    }
                }
            }
        }
        out
    }

    /// [`qml_code`] 自己那一腿的反证：上面那格判的是「代码里没有死名字」，而它整个建立在
    /// 「注释被剥掉、代码被留下」这枚断言之上。这格把两边各钉一枚形状，免得那格退化成
    /// 「剥得多就绿、剥得少也绿」——红的一侧不可达（#274/#301 一族）。
    #[test]
    fn qml_code_strips_comments_but_not_the_code_they_quote() {
        let src = "// onDelayLeftChanged 是死的\n\
                   Text { text: \"a//b\" /* onMaskCountChanged 也不在场 */ }\n\
                   /* 外层 /* 嵌套 */ 仍在注释里 */ function onCountingChanged() {}\n";
        let got = qml_code(src);
        // 注释里那两个名字都不该活下来。
        assert!(!got.contains("onDelayLeftChanged"), "行注释没被剥掉：{got}");
        assert!(!got.contains("onMaskCountChanged"), "块注释没被剥掉：{got}");
        assert!(!got.contains("仍在注释里"), "嵌套块注释只剥了一层：{got}");
        // 代码与字符串里的内容一字不少。
        assert!(
            got.contains("function onCountingChanged()"),
            "活 handler 被剥掉了：{got}"
        );
        assert!(got.contains("\"a//b\""), "字符串里的 // 被当成注释：{got}");
        // 行号仍然对得回去：换行一枚没丢。
        assert_eq!(
            got.matches('\n').count(),
            src.matches('\n').count(),
            "剥注释把换行也吃了，读者按行号回不到原档"
        );
    }

    /// `delay_left` → `DelayLeft`：只用来构造上面那一枚**不该在场**的名字。
    fn camel_case(name: &str) -> String {
        let mut out = String::new();
        for part in name.split('_') {
            let mut first = true;
            for c in part.chars() {
                out.push(if first {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                });
                first = false;
            }
        }
        out
    }
}
