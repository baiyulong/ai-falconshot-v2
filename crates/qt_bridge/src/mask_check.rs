//! The mask, checked against the pixels it is supposed to be showing.
//!
//! `--mask <ms>` freezes the desktop, puts one screen-sized mask window on every
//! screen, lets the event loop paint for `ms`, and then answers three questions
//! that a screenshot tool cannot answer by looking:
//!
//! * **Is there one per screen, at the right size?** A mask that covers 1536x960 of
//!   a 3072x1920 monitor is a mask that divides by the wrong scale, and it looks
//!   plausible until a selection near the edge is asked for.
//! * **Is what it shows the frame it was given?** `PrintWindow` with the flag R13
//!   measured as the one that works asks the window to paint itself, so the
//!   comparison is between this process's own two answers rather than between a
//!   picture and a memory of one.
//! * **Is the dim the arithmetic?** Outside the hole the pixels must come back at
//!   `1 - DIM` of the frozen frame, inside at `1.00`. A number, so that "the dim
//!   looks about right" is not what a review has to rest on.
//!
//! A fourth line reports the same two ratios read through Qt's own screen grab
//! ([`grab_note`]). It is a note and not a row: it exists so that a `0.000` from the
//! `PrintWindow` leg has a witness beside it, and not a second grade to average.
//!
//! Sampling is a grid over the whole window, and a sample is only usable when the
//! source pixel is bright enough to divide by. A desktop that is black everywhere
//! makes the ratio unmeasurable, which is reported as `SKIP` with the counts that
//! explain why rather than as a failure.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use falcon_core::capture::{LiveGate, ScreenSnapshot};
use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect};

use crate::mask;
use crate::mask_view::shim;
use crate::r13::{parse_levels, parse_size, Report, Stats, Top};
use crate::state::Check;
use crate::{capture, session};

/// The title prefix `CaptureMask.qml` gives its windows, and the only way this
/// harness can tell a mask from a pin: both are `QQuickWindow`, and the mask is the
/// one whose name this process chose. No space in it, because the shim's window
/// line splits its fields on whitespace.
///
/// 这一格**不是**第二处拼写，是 [`crate::mask::MASK_TITLE_PREFIX`] 的别名：实时背景那两阶段
/// 自检也要按标题认遮罩窗（[`crate::mask::MaskState::mask_windows`]），而同一件事抄两遍就是
/// 迟早分家（#281）。别名指向生产那一枚，于是这里改一个字都改不动 —— 要改只能改 `mask.rs`，
/// 而那一枚有 QML 那头逐字对账的单测看着。
pub(crate) const TITLE: &str = crate::mask::MASK_TITLE_PREFIX;

/// The selection the **product** opened with, read before this harness plants its
/// own hole. `None` = the door never ran [`open`].
///
/// 自由区域那一档进场**没有选区**（§5.3.1 步 1：用户还没拖）。这一格是那件事在门上的读者：
/// 门自己种的那块洞是**门要的**（量暗化与洞要用一块已知大小的洞），它**不能**替产品回答
/// 「进场带不带选区」——所以产品在门种洞之前交回来的那一块被记在这里，由 [`measure`] 那一行
/// 单独判。少这一行的话，「`apply_opening` 忘了用 `opening_hole`」这一族错（单测量的是那枚
/// 纯函数，不是这条接线）在门上就看不见。
static ENTRY_HOLE: Mutex<Option<PhysRect>> = Mutex::new(None);

/// Per-channel floor below which a source pixel cannot carry a ratio. Black times
/// anything is black, so a dim measured over a black desktop is 0/0 and not 0.400.
pub(crate) const MIN_SOURCE: u32 = 32;

/// How far from the selection edge a sample has to be to count, in
/// device-independent pixels. The border is 2 DIP wide, and a sample on it is a
/// white pixel rather than a measurement; the slack is doubled so a half-pixel
/// rounding at the edge is not read as a dim of 1.0.
pub(crate) const BORDER_DIP: f64 = 4.0;

/// The tolerance on both ratio rows. Composition is allowed a rounding or two; a
/// dim layer that is 5% off is a different alpha, and 0.05 is far below that.
pub(crate) const RATIO_TOL: f64 = 0.05;

/// How long a *warm* mask may take to show its first frame: the same ceiling the
/// freeze is held to in [`crate::state::capture_selftest`], because both are inside
/// the one promise (§9.2, hot key to mask visible). Today's `--mask` run cannot
/// honour it and is not judged on it - it measures a process that starts Qt, loads
/// QML and then creates the window, in that order, for the first time. The number
/// it prints (1205 ms on this machine, 2026-10-06) is what the pre-warm has to
/// reduce, and the ceiling is what the pre-warmed run will be checked against.
pub const FIRST_FRAME_MS: u64 = 400;

/// 这一档自己钉的配置写回落点。
///
/// §9.1 第十五条那一条协议：`--ink` / `--reveal` / `--autosave` 各钉各的，而 `--mask` 此前是四扇
/// 开遮罩的门里**唯一**没钉的一扇。欠着的理由在 [`open`] 那一枚 `frozen` 参数上：
/// `MaskState::close()` 走的是产品自己那句 [`crate::settings::write_back`]，而
/// [`crate::settings::Settings::save`] 序列化的是**整份** config 到 `write_to` —— 于是内存里翻的
/// 那一位会在遮罩关掉那一刻落进用户 `%APPDATA%` 里那份档。量具替用户改了产品行为，而且是静默地
/// 改（#433 的反面）。
///
/// 与 `crate::reveal_check::pin` 同一条口径：只搬**目的地**，`read=` 仍是用户那份档（所以
/// `apply=` 数的还是用户自己设的那几枚）。`pin_writes` 不替创建者 mkdir，所以先把目录开出来。
pub fn pin_settings() -> PathBuf {
    let _ = std::fs::create_dir_all(gauge_dir());
    crate::settings::pin_writes(&pinned_path())
}

/// 这一档自己的目录。与 `--ink` 那一枚（`falconshot-ink-gauge`）、`--reveal` 那一枚
/// （`falconshot-reveal-door`）分开：三档钉的是同一件事的三个落点，共用一枚就变成「上一趟留下的
/// 那份档被这一趟读回来」，而那正是 `crate::reveal_check::door_dir` 头上写过的那个形状。
fn gauge_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-mask-gauge");
    dir
}

/// 钉的那个落点。[`pin_settings`] 与下面两格 `pinned settings writes`（[`measure`] 与
/// [`live_measure`] 各一格）各自算一次同一个数，所以它得是函数而不是 `pin_settings` 的返回值
/// （`crate::reveal_check::pinned_path` 同一条理由）。
fn pinned_path() -> PathBuf {
    let mut dir = gauge_dir();
    dir.push("pinned-config.toml");
    dir
}

/// `--live` 那一档要求的**最短**等待毫秒数。
///
/// 这个数不是估的，是从一次真机读数推的：2026-11-07 解锁桌面上跑的那一趟 `--ignored` affinity
/// 探针量到这台机器**整屏读一次**是 312 / 297 / 309 ms，而 §5.2.1 步 2 后半支钉死的两阶段自检要
/// 读**两次**（摘之前、摘之后）⇒ 光自检就约 620 ms；过了闸之后刷新循环每一帧还要再读一次整屏
/// （节拍 `mask.rs` 的 `LIVE_BUDGET_MS`）。加上 Qt/QML 冷起（`cold of` 那格 2026-10-06 读到
/// 1205 ms）与页侧那枚 `Timer` 的粒度，6000 ms 能让刷新跑到约十帧上下 —— 而下面 `live frame of`
/// 那格判的是 `revision` 与 `live_frames` 之间的一条算术，两帧以下咬不住。
pub const MIN_LIVE_MS: i32 = 6000;

/// Freeze and open the mask. Must run before the QML engine, so the windows exist
/// when the event loop starts painting.
///
/// `frozen` 是这一趟钉在 `[capture] freeze_frame` 上的那一位，**只在内存里翻**。翻内存而不翻
/// `MaskState.capture`，是因为 `MaskState::open` 里有一句 `self.capture = crate::settings::capture_cfg();`，
/// 它坐在 `open_from_freeze` 之内，会把事先摆在状态上的任何值冲掉。
///
/// 这一枚参数存在的理由不是整洁，是**读数会不会红在别人身上**：`false` 那一侧起一条实时刷新腿，
/// 而那条腿在整个循环里一直持有 `WDA_EXCLUDEFROMCAPTURE`（`crates/qt_bridge/src/mask.rs` 的
/// `live_thread`，`restore_all()` 只在循环断了之后才跑），把遮罩那几扇窗从**所有**屏幕读取里摘
/// 出去。本档 `dim of` / `hole of` 两族正是用 [`platform_windows::print::print_window`] 读那几扇窗
/// 自己的像素，摘着的时候读到的是底下的桌面。而 `PrintWindow` 在一扇被摘的窗上到底交回什么，这台
/// 机器上**没有证人**（`crates/platform-windows/src/affinity.rs` 那一枚 `--ignored` 探针在 `exclude`
/// **之前**读 `print_window`），所以这条前提必须钉死，不许交给用户档里那一位碰巧是什么 —— 否则
/// 同一档在两台机器上量的是两件不同的事（#261）。
///
/// 写回落点由调用方钉（[`pin_settings`]）：这一格只翻内存，而翻完的那一份会被 `close()` 写出去。
pub fn open(shader: bool, frozen: bool) -> Result<String, String> {
    crate::settings::with(|s| s.config.capture.freeze_frame = frozen);
    // The moment a hot key would have been pressed. Every later stamp is measured
    // against this one when the cold path is split up.
    crate::state::stamp("asked");
    let opened = mask::open_from_freeze(shader)?;
    // 门自己种洞之前，先把**产品进场时**那一块记下来：自由区域这一档进场**没有选区**
    // （§5.3.1 步 1）。这一格答的是产品那一件事，门自己那块洞不参与。
    *ENTRY_HOLE.lock().unwrap_or_else(PoisonError::into_inner) = Some(mask::with(|m| m.hole));
    // The hole this harness measures is **planted here**, and it is planted
    // *off-centre* on purpose.
    //
    // The rows below ask the mask two questions about the same hole: "is the area
    // outside the selection dimmed to `1 - DIM`?" and "is the area inside it left
    // at full brightness?". The second one can only fail if the hole is somewhere a
    // mistake would not put it - and a hole in the middle of the screen is its own
    // mirror image. A shader that flipped the hole's vertical test drew it in the
    // same place, so the row stayed green while the user saw no bright rectangle
    // inside the selection at all (measured 2026-11-12: the hole landed mirrored,
    // the whole screen stayed dim, and this harness reported `hole=1.000`).
    //
    // The planted hole is the top-left quarter of the primary, so its mirror image
    // is the bottom-left quarter - a different place, and a row that can see it.
    let planted = opened
        .snap
        .monitors
        .iter()
        .find(|m| m.info.primary)
        .map(|m| m.info.bounds)
        .map(|b| {
            PhysRect::new(
                b.x + (b.w / 8) as i32,
                b.y + (b.h / 8) as i32,
                b.w / 4,
                b.h / 4,
            )
        });
    if let Some(hole) = planted {
        mask::with(|m| m.set_hole(hole));
    }
    let (pixels, hole) = mask::with(|m| (m.pixels_ms.unwrap_or(0), m.hole));
    // Every number this harness prints depends on which RHI drew them, and the only
    // way to ask is the environment - Qt has no "which backend did I pick" call that
    // does not need a window first.
    let rhi = std::env::var("QT_QUICK_BACKEND").unwrap_or_else(|_| "default".to_string());
    Ok(format!(
        "{} screen(s), backend={}, rhi={rhi}, freeze {} ms, pixels {} ms, hole={hole}, shader={shader}, freeze_frame={frozen}",
        opened.slots, opened.snap.backend, opened.ms, pixels
    ))
}

/// Every slot's first presented frame so far, as one short line.
pub fn swap_line() -> String {
    mask::with(|m| {
        m.slots
            .iter()
            .map(|s| format!("{}={:?}", s.name, s.first_swap_ms))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Tear the mask down and open it again, inside a process that has already paid for
/// Qt, the QML engine, the compiled document and a presented frame.
///
/// This is what §10-14 ② has been asking for: the cold `cold of` figure contains
/// roughly a second of start-up that a resident product never pays on a hot key, and
/// without a second round there is no number for the half it *does* pay. A failed
/// round comes back as text rather than an error, because the caller is a timer
/// handler with nowhere to send one.
///
/// The round's own first frame cannot be in its own line: `open()` returns before
/// the event loop has painted, so the number only exists by the time the *next*
/// round starts. Each line therefore reports the round before it, and the last is
/// read by `main.rs` before [`measure`] closes the flow.
pub fn warm_round(shader: bool) -> String {
    let prev = swap_line();
    mask::with(|m| m.close());
    // `frozen` 递 `true` 而不是把参数长出去：暖几趟只由 `--mask` / `--ink` 两档点得起来
    // （`refusals` 里 `--warm-rounds` 少了这两档就被拒），而那两扇钉的都是 `true`。多一枚参数
    // 等于给一件只有一种答的事留两处作者（#281）。
    match open(shader, true) {
        Ok(line) => format!("prev-swap[{prev}] | {line}"),
        Err(e) => format!("prev-swap[{prev}] | 开不起来：{e}"),
    }
}

/// The ratio of one sample: what the mask shows, over what was frozen there.
/// Summing the three channels instead of comparing them one by one makes the
/// number one division rather than three, so the median of a set of them is the
/// median of one quantity.
pub(crate) fn ratio(src: [u8; 4], got: [u8; 4]) -> f64 {
    let a = src[0] as u32 + src[1] as u32 + src[2] as u32;
    let b = got[0] as u32 + got[1] as u32 + got[2] as u32;
    if a == 0 {
        return f64::NAN;
    }
    b as f64 / a as f64
}

pub(crate) fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut sorted: Vec<f64> = v.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    Some(sorted[sorted.len() / 2])
}

/// What one window offered: the ratios either side of the hole, and how many grid
/// points were thrown away. The counts are part of the result because a `SKIP`
/// with no numbers beside it is not an explanation.
pub(crate) struct Sides {
    pub(crate) inside: Vec<f64>,
    pub(crate) outside: Vec<f64>,
    pub(crate) dark: usize,
    pub(crate) off_desktop: usize,
    pub(crate) on_border: usize,
}

/// Every usable sample one bitmap offers, split by which side of the hole it is on.
///
/// Coordinates are the awkward part of any mask measurement, so they are spelled
/// out: `cover` is the part of the desktop the bitmap shows, in *device* pixels;
/// `hole` is the selection in device pixels in the same virtual-desktop space; and
/// `got` is the bitmap. A sample at `(x, y)` of `got` is therefore the desktop point
/// `cover`'s origin plus `(x, y)` *scaled by however many device pixels that bitmap
/// pixel covers* - `PrintWindow` is asked for the device size and usually answers
/// with it, and Qt's screen grab answers with the primary screen's device size on
/// this machine, but "usually" is not a thing a measurement may assume. Nothing here
/// divides by the display scale: both bitmaps are already at device resolution, and
/// saying so is what makes the two paths' numbers comparable.
pub(crate) fn samples(
    cover: PhysRect,
    got: &Frame,
    snap: &ScreenSnapshot,
    hole: &PhysRect,
    dpr: f64,
) -> Sides {
    let (w, h) = (got.width as i32, got.height as i32);
    if w <= 0 || h <= 0 {
        return Sides {
            inside: Vec::new(),
            outside: Vec::new(),
            dark: 0,
            off_desktop: 0,
            on_border: 0,
        };
    }
    let scale_x = cover.w.max(1) as f64 / w as f64;
    let scale_y = cover.h.max(1) as f64 / h as f64;
    // How far a bitmap pixel is allowed to sit from the hole's edge and still be
    // called a border sample - in bitmap pixels, since that is where the grid runs.
    let border = (BORDER_DIP * dpr / scale_x.max(1.0)) as i32;
    let mut sides = Sides {
        inside: Vec::new(),
        outside: Vec::new(),
        dark: 0,
        off_desktop: 0,
        on_border: 0,
    };
    // Roughly two thousand samples, whatever the screen is.
    let step = ((w as f64 * h as f64 / 2000.0).sqrt().ceil() as i32).max(1);
    for y in (0..h).step_by(step as usize) {
        for x in (0..w).step_by(step as usize) {
            let at = PhysPoint::new(
                cover.x + (x as f64 * scale_x).round() as i32,
                cover.y + (y as f64 * scale_y).round() as i32,
            );
            let Some(src) = snap.color_at(at) else {
                sides.off_desktop += 1;
                continue;
            };
            if (src[0].max(src[1]).max(src[2]) as u32) < MIN_SOURCE {
                sides.dark += 1;
                continue;
            }
            // Distance to the nearest vertical and horizontal edge, signed at the
            // edge and growing in both directions: 0 is on the boundary, +3 is
            // either 3 px inside or 3 px outside, and both are the border.
            let dx = (at.x - hole.x).min(hole.right() - 1 - at.x).abs();
            let dy = (at.y - hole.y).min(hole.bottom() - 1 - at.y).abs();
            if dx <= border || dy <= border {
                sides.on_border += 1;
                continue;
            }
            let r = ratio(src, got.get(x as u32, y as u32));
            if r.is_nan() {
                sides.dark += 1;
                continue;
            }
            if hole.contains(at) {
                sides.inside.push(r);
            } else {
                sides.outside.push(r);
            }
        }
    }
    sides
}

/// One window's three `PrintWindow` flags, as notes. R13 measured the pin window;
/// a mask is the same kind of window at screen size, so this run closes the
/// §10 item 13 ③ column without a second harness. `ctrl` has no meaning here (a
/// mask contains no control colour) and `flat` is the reading that matters: a flag
/// that paints nothing answers `flat=1.000`.
pub(crate) fn flag_column(rep: &mut Report, win: &Top) {
    for flag in [2u32, 0, 4] {
        let detail =
            match platform_windows::print::print_window(win.hwnd, win.phys.w, win.phys.h, flag) {
                Some(f) => Stats::of(&f).line(),
                None => "nothing".to_string(),
            };
        rep.note(&format!("pw:mask flag {flag}"), detail);
    }
}

/// The same two medians, read through a path that shares nothing with
/// `PrintWindow`: `QScreen::grabWindow(0)` asks Qt's own backend for the composited
/// primary screen.
///
/// Every scored row comes from one window's bitmap, so when that path answers
/// `dim=0.000` there are two very different explanations - the mask really is black,
/// or `PrintWindow` cannot see the way the mask was drawn. P6 hit exactly that
/// disagreement with a GDI `BitBlt` of the screen DC against a Qt Quick window, and
/// the `flat=` column of the rows above is the only other witness available. This is
/// the second one, and deliberately not a scored row: which path is the trustworthy
/// one is a property of the machine and the RHI, not a promise the product makes to a
/// user, and a check that grades a witness measures the witness instead of the mask.
fn grab_note(snap: &ScreenSnapshot, hole: &PhysRect) -> String {
    // The order is not interchangeable: `pinScreenGrabInfo` describes whatever the
    // last grab left behind, and the grab clears it before it starts, so asking
    // first always answers "null".
    let bytes = session::shim::screen_grab();
    let info = session::shim::screen_grab_info();
    let Some((w, h)) = parse_size(&info) else {
        return format!("no size ({info:?})");
    };
    let got = match Frame::from_rgba(w, h, bytes) {
        Ok(f) => f,
        Err(e) => return format!("{w}x{h}: {e}"),
    };
    // What the grab covers, in device pixels. `pinPrimary*` is Qt's geometry -
    // logical pixels - and using it here makes the scale a half and the medians
    // nonsense (measured: `dim=0.109`). The snapshot's own primary monitor is the
    // device-pixel rectangle this bitmap is a photograph of, and it is the same
    // reference the ratios divide by, so the two cannot disagree about the desktop.
    let primary = snap
        .monitors
        .iter()
        .find(|m| m.info.primary)
        .or_else(|| snap.monitors.first());
    let Some(monitor) = primary else {
        return format!("{w}x{h} grabbed, but the freeze reports no monitor");
    };
    let cover = monitor.info.bounds;
    let sides = samples(cover, &got, snap, hole, monitor.info.scale.ratio());
    let read = |v: Option<f64>| v.map(|v| format!("{v:.3}")).unwrap_or_else(|| "-".into());
    format!(
        "{w}x{h} of {cover} dim={} hole={} ({}+{} usable; dark={} border={})",
        read(median(&sides.outside)),
        read(median(&sides.inside)),
        sides.outside.len(),
        sides.inside.len(),
        sides.dark,
        sides.on_border
    )
}

/// A ratio row's verdict: a median within tolerance, or a `SKIP` that says which
/// of the three reasons it has (no light, no samples on that side, no freeze).
pub(crate) fn ratio_row(
    count: usize,
    value: Option<f64>,
    want: f64,
    sides: &Sides,
) -> (Check, String) {
    let why = match value {
        None => format!(
            "measured=- want {want:.3} ±{RATIO_TOL} ({} usable; dark={} off-desktop={} border={})",
            count, sides.dark, sides.off_desktop, sides.on_border
        ),
        Some(v) => format!(
            "measured={v:.3} want {want:.3} ±{RATIO_TOL} ({count} usable; dark={} border={})",
            sides.dark, sides.on_border
        ),
    };
    let verdict = match value {
        None => Check::Blocked,
        Some(v) => Check::from((v - want).abs() <= RATIO_TOL),
    };
    (verdict, why)
}

/// The check itself, after the event loop has painted.
/// The QML engine's own voice, as a row of either mask gauge.
///
/// Every other row there is a measurement of Rust's state machine and the pixels it
/// published. A document that does not build changes none of them: a handler naming
/// a signal the type does not declare, or a member the event object has no name for,
/// leaves the dim, the hole and the ink reading exactly as they should, because the
/// answer they measure never went through the thing that failed to load.
///
/// `.qml` is the filter. A complaint about a document names the file that could not
/// be built - the shim prefixes the log context onto the text for exactly that
/// reason - while Qt's own exit chatter (`QDxgiVSyncService not destroyed in time`)
/// names nothing.
pub(crate) fn qml_row(rep: &mut Report) {
    let bad: Vec<String> = crate::state::messages()
        .lines()
        .filter(|line| {
            !line.starts_with("debug:")
                && !line.starts_with("info:")
                && (line.contains(".qml") || line.contains("qml/"))
        })
        .map(str::to_string)
        .collect();
    rep.row(
        "no qml complaints",
        Check::from(bad.is_empty()),
        bad.join(" | "),
    );
}

pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
    // §10 第 41 项 ④: this gauge reads the desktop in every one of its scored rows, and
    // until now the only thing it could say about the session was what its rows refused to.
    crate::r13::scene_row(&mut rep);
    // 翻档协议的另一半：`main` 在开遮罩之前钉过写回落点，所以 [`open`] 在内存里翻的那一位
    // `freeze_frame` 哪怕走完 `close()` 也碰不到用户 `%APPDATA%` 里那份档。读与写两个数都从
    // `Settings::line()` 里取（`pin_writes` 只动 destination，`read=` 仍是本轮真读的那份）。
    // 这一格此前不存在，是因为这一档此前不翻任何东西；`--live` 把「翻成 `false`」带进同一枚
    // `open` 之后，不钉就是量具替用户把实时背景打开了。
    let (read, write) = crate::reveal_check::read_and_write();
    let want = pinned_path().display().to_string();
    rep.row(
        "pinned settings writes",
        Check::from(write == want && read != write),
        format!("settings line 的 read={read:?} 与 write_to={write:?}（要求后者 == {want:?} 且两者不同）"),
    );
    let snap = capture::current();
    let levels = parse_levels(&session::shim::top_levels());
    let masks: Vec<&Top> = levels
        .iter()
        .filter(|t| t.title.starts_with(TITLE))
        .collect();

    let screens = shim::screens().len();
    let slots = mask::with(|m| m.slots.len());
    rep.row(
        "one per screen",
        Check::from(slots > 0 && slots == screens && masks.len() == slots),
        format!("qt screens={screens} slots={slots} windows={}", masks.len()),
    );

    let (phys_hole, shader, desktop, problems) =
        mask::with(|m| (m.hole, m.shader, m.desktop(), m.problems.clone()));

    // The selection the **product** opened with, read before the harness planted
    // its own hole: free-region capture enters with **no** selection (§5.3.1 step 1).
    // This is the only row that answers that question - the harness's own hole is
    // measured below, and it must not answer it for the product.
    let entry = *ENTRY_HOLE.lock().unwrap_or_else(PoisonError::into_inner);
    rep.row(
        "the product opens with no selection",
        Check::from(entry.is_some_and(|r| r.is_empty())),
        format!("{entry:?}"),
    );

    // The hole, asserted as a relationship: it is inside the desktop and it is not
    // empty. Content-independent, so it holds on any desktop on any number of
    // screens - and it is the *planted* hole now (see [`open`]), not whatever the
    // product opens with.
    let fits = phys_hole
        .intersection(&desktop)
        .is_some_and(|i| i == phys_hole)
        && !phys_hole.is_empty();
    rep.row(
        "hole fits desktop",
        Check::from(fits),
        format!("{phys_hole} of {desktop}"),
    );

    for slot in mask::with(|m| m.slots.clone()) {
        let tag = slot.name.clone();
        let want_title = format!("{TITLE}{}", slot.name);
        let Some(win) = masks.iter().find(|t| t.title == want_title) else {
            rep.row(
                &format!("window {tag}"),
                Check::Fail,
                format!("no top-level titled {want_title}"),
            );
            continue;
        };

        // Device pixels: the monitor's own size, because a mask that is 1536x960 on
        // a 3072x1920 screen is a mask that divided by the scale twice.
        let close = (win.phys.w as i32 - slot.bounds.w as i32).abs() <= 2
            && (win.phys.h as i32 - slot.bounds.h as i32).abs() <= 2
            && (win.phys.x - slot.bounds.x).abs() <= 2
            && (win.phys.y - slot.bounds.y).abs() <= 2;
        rep.row(
            &format!("rect of {tag}"),
            Check::from(close),
            format!(
                "window={}x{}@{},{} monitor={}",
                win.phys.w, win.phys.h, win.phys.x, win.phys.y, slot.bounds
            ),
        );

        let decode = shim::self_check(&slot.key);
        let sized = decode.contains(&format!("w={} h={}", slot.bounds.w, slot.bounds.h))
            && !decode.contains("null=1");
        rep.row(&format!("texture of {tag}"), Check::from(sized), decode);

        // Which of the two dim paths is on screen, and whether the shader said it
        // compiled. P6 measured 0 as the compiled answer; `QT_QUICK_BACKEND=software`
        // was measured this run to leave it at 1 (Uncompiled) rather than fail loudly,
        // and `CaptureMask.qml` falls back to the rectangles on exactly that reading.
        // A non-zero status is therefore not the mask failing, it is the mask changing
        // how it draws - and `dim of`/`hole of` are the rows that say whether the
        // fallback carried it.
        let path = match (shader, slot.shader_status) {
            (false, _) => "rectangles (chosen)",
            (true, Some(0)) => "shader",
            (true, _) => "rectangles (fallback)",
        };
        rep.note(&format!("dim path {tag}"), path.to_string());
        if shader {
            rep.row(
                &format!("shader of {tag}"),
                match slot.shader_status {
                    Some(0) => Check::Pass,
                    Some(_) => Check::Blocked,
                    None => Check::Fail,
                },
                format!("{:?} (0 = compiled)", slot.shader_status),
            );
        } else {
            rep.note(&format!("shader of {tag}"), "off (rectangles)".to_string());
        }

        // Did the mask appear at all? That part is this process's own doing, and is
        // scored. How long it took is reported underneath, because in this mode the
        // clock starts at `open`, which is before the QML engine exists: the number
        // is dominated by Qt's own start-up, and §10's mask pre-warm is what has to
        // turn it into the warm-path figure [`FIRST_FRAME_MS`] is a ceiling for.
        rep.row(
            &format!("frame of {tag}"),
            match slot.first_swap_ms {
                Some(_) => Check::Pass,
                None if win.visible => Check::Fail,
                None => Check::Blocked,
            },
            match slot.first_swap_ms {
                Some(_) => format!("first of {} swap(s)", slot.swaps),
                None if win.visible => "visible but never presented a frame".to_string(),
                None => "the window says it is not visible".to_string(),
            },
        );
        rep.note(
            &format!("cold of {tag}"),
            match slot.first_swap_ms {
                Some(ms) => format!("{ms} ms since open, {FIRST_FRAME_MS} ms is the warm ceiling"),
                None => "-".to_string(),
            },
        );
        // A window that presented nothing has nothing to read back: `PrintWindow`
        // would return the last frame or a blank one, and either would be a
        // measurement of the wrong thing.
        if slot.first_swap_ms.is_none() || !win.visible {
            rep.row(
                &format!("dim of {tag}"),
                Check::Blocked,
                "no presented frame to read".to_string(),
            );
            continue;
        }

        flag_column(&mut rep, win);
        let Some(got) = platform_windows::print::print_window(win.hwnd, win.phys.w, win.phys.h, 2)
        else {
            rep.row(
                &format!("dim of {tag}"),
                Check::Blocked,
                "PrintWindow(flag 2) produced no bitmap for this window".to_string(),
            );
            continue;
        };
        if got.width != win.phys.w || got.height != win.phys.h {
            rep.note(
                &format!("bitmap of {tag}"),
                format!(
                    "{}x{} for a {}x{} window",
                    got.width, got.height, win.phys.w, win.phys.h
                ),
            );
        }
        let Some(frozen) = &snap else {
            rep.row(
                &format!("dim of {tag}"),
                Check::Blocked,
                "the freeze is gone; nothing to compare against".to_string(),
            );
            continue;
        };
        let sides = samples(win.phys, &got, frozen, &phys_hole, slot.scale.ratio());
        for (label, want, values) in [
            ("dim", 1.0 - mask::DIM, &sides.outside),
            ("hole", 1.0, &sides.inside),
        ] {
            let (verdict, detail) = ratio_row(values.len(), median(values), want, &sides);
            rep.row(&format!("{label} of {tag}"), verdict, detail);
        }
    }

    // The second witness is one call for the whole run rather than one per window:
    // Qt grabs the primary screen, and the hole it shows or hides is the same hole
    // every scored row was measured against. This is the last thing read before the
    // flow is closed, because after `close` the windows it is looking at are gone.
    match &snap {
        Some(frozen) => rep.note("grab of desktop", grab_note(frozen, &phys_hole)),
        None => rep.note(
            "grab of desktop",
            "the freeze is gone; no reference pixels".to_string(),
        ),
    }

    rep.row(
        "no problems",
        Check::from(problems.is_empty()),
        problems.join(" | "),
    );
    qml_row(&mut rep);

    // §5.3.7 步骤三「系统根据当前显示器缩放比例换算尺寸」在真机上的那一对数字。这一格是新长的
    // 门臂，因为 Rust 的单测能算换算，却算不出"过桥之后屏幕上还剩不还剩这两个数"：这台机器的
    // 缩放是 2.00，所以同一张选区在 px 那一格读 1536×960、在 dip 那一格读 768×480。
    //
    // 判据不从 `size_shown()` 自己推导（那是拿公式验公式）：px 那一格必须等于 `phys_hole`，而
    // 那是上面 `dim of` / `hole of` 两族**采样屏上像素**时用的同一枚矩形——屏幕上量出来的洞与
    // 读数说的是同一块地方；dip 那一格必须等于它除过 [`MaskState::size_scale`] 的缩放（选区自己
    // 那张屏的缩放，由 `monitor_slot_for` 挑，与窗自己的 `slot.scale` 在这台单屏机上正好是同一个
    // 数，多屏时是两枚——那一半本轮没有证人）。外加一条"切单位不许动选区"：单位是一句读数，
    // 不是第二次拖拽。
    let (px, dip, scale, moved) = mask::with(|m| {
        let before = m.hole;
        m.set_unit(0);
        let px = m.size_shown();
        m.set_unit(1);
        let dip = m.size_shown();
        m.set_unit(0);
        (px, dip, m.size_scale().ratio(), m.hole != before)
    });
    // 比较在**显示那一边**做：dip 是 px 除过缩放后四舍五入的，所以 |px/scale − dip| 的上界是
    // 0.5，取 0.75 既容得下舍入、又容不下任何一次真正的错缩放（那种差是整倍，不是半像素）。
    // A 代把这行写成了 `|px − dip/scale|`——在 100% 的机器上两种写法一样，在这台 200% 上它把
    // 1536 与 384 比，于是自己 FAIL 在自己身上（`p162`/`p163` 的仪器自缺陷清单各记一笔）。
    let close = |px_side: u32, dip_side: u32| {
        (f64::from(px_side) / scale - f64::from(dip_side)).abs() < 0.75
    };
    rep.row(
        "unit of desktop",
        Check::from(
            !moved && px == (phys_hole.w, phys_hole.h) && close(px.0, dip.0) && close(px.1, dip.1),
        ),
        format!(
            "px={}×{}（采样洞 {}×{}）dip={}×{} scale={:.2} 切单位动选区={}",
            px.0, px.1, phys_hole.w, phys_hole.h, dip.0, dip.1, scale, moved
        ),
    );

    rep.note("mask state", mask::with(|m| m.line()));

    // Ending the flow, and checking it ended: a screen-sized texture is ~24 MB per
    // monitor, so a mask that is dismissed without releasing its frames leaks the
    // whole desktop on every screenshot. The readback is the provider's own answer
    // for the key, not a memory of what was stored.
    let keys = mask::with(|m| {
        let keys: Vec<String> = m.slots.iter().map(|s| s.key.clone()).collect();
        m.close();
        keys
    });
    let left: Vec<&String> = keys
        .iter()
        .filter(|k| !shim::self_check(k).contains("null=1"))
        .collect();
    rep.row(
        "textures released",
        Check::from(left.is_empty()),
        format!(
            "{} of {} still in Qt's process after close",
            left.len(),
            keys.len()
        ),
    );
    rep.finish("mask check")
}

/// `--live <ms>` 那一档：`[capture] freeze_frame` **关着**跑一整场遮罩，量实时背景那一支自己。
///
/// 与 [`measure`] 分成两档、而不是给它加一枚开关，是因为两档量的东西**互斥**：本档那趟刷新线程
/// 在整个循环里一直持有 `WDA_EXCLUDEFROMCAPTURE`（`crates/qt_bridge/src/mask.rs` 的 `live_thread`，
/// `restore_all()` 只在循环断了之后才跑），而 [`measure`] 的 `dim of` / `hole of` 两族正是用
/// [`platform_windows::print::print_window`] 读那几扇窗自己的像素 —— 摘着的时候读到的是底下的
/// 桌面。`PrintWindow` 在一扇被摘的窗上到底交回什么，这台机器上**没有证人**，所以本档压根不量
/// 那两个比值；而这句话同时写进报告（`pixels on glass` 那一格 note），好让"少两族"是被说出来的
/// 而不是被忘了的（#433）。
///
/// 判据仍归 [`falcon_core::capture::live_gate`]：本档不另起一套（#281），只把闸**答过没有**、
/// 答完之后那几帧**灌没灌进去**、以及摘出去的那几扇窗**还回来了没有**这三件事各自读回来。
///
/// `asked_ms` 只印不判，与 `crate::reveal_check::measure` 同一条口径。真正被要求的是
/// [`MIN_LIVE_MS`]：`main` 那一侧已经把这一档的毫秒数抬到不低于它，因为下面 `live frame of`
/// 那族判的是 `revision == 1 + live_frames` 那一条算术，两帧以下咬不住。
pub fn live_measure(asked_ms: i32) -> (Check, String) {
    let mut rep = Report::new();
    // 锁着的桌面上，闸答的是锁屏那一块的像素差，不是这台机器屏幕读取腿的脾气 —— 2026-11-05/06
    // 那三次读数就是这么被撤回的。这一格先把会话本身摆在报告头上。
    let scene = crate::r13::scene_row(&mut rep);
    // 上面那一格是读数，这一格才是判据，而本档是二十几档里少数**敢**把锁屏判成"这一趟不算"的：
    // 它量的就是"背景跟着桌面走"，桌面不在的时候闸答什么都不作数。判 `Blocked` 不判 `Fail`，
    // 因为对面是机器不是这份代码，`verdict_code` 会把它交成退出码 3；`Fail` 压过 `Blocked`，
    // 所以真有毛病时这一格也遮不住。`scene_row` 那一头不敢这么判，是因为它还坐在不看屏幕的
    // 门上（`--pin` 锁着解锁两趟全绿），一份判据两处口径就是 #281。
    let readable = scene.read();
    rep.row(
        "premise desktop readable",
        if readable {
            Check::Pass
        } else {
            Check::Blocked
        },
        format!(
            "hwnd={} locked={} ⇒ read={readable}（`Scene::read` 要求有窗口且不是锁屏；\
             那一枚读数的原文在上面 `desktop scene` 那一格）",
            match scene.hwnd {
                Some(h) => format!("{h:#x}"),
                None => "无".to_string(),
            },
            scene.locked
        ),
    );

    // 翻档协议：`main` 在开遮罩之前钉过写回落点，所以 [`open`] 在内存里翻成 `false` 的那一位
    // `freeze_frame` 哪怕走完 `close()` 也碰不到用户 `%APPDATA%` 里那份档。与 [`measure`] 那一格
    // 同一个标签、同一条判据、同一个落点 —— 两份报告不会出现在同一次运行里。
    let (read, write) = crate::reveal_check::read_and_write();
    let want = pinned_path().display().to_string();
    rep.row(
        "pinned settings writes",
        Check::from(write == want && read != write),
        format!(
            "settings line 的 read={read:?} 与 write_to={write:?}（要求后者 == {want:?} 且两者不同）"
        ),
    );

    // 前提，两处各读一次：内存里那份 settings 上的值，与 `MaskState` 进场那一刻抄走的快照。
    // 两枚都得是 `false` 这一档量的才是实时背景 —— 只翻前者而 `open()` 那句
    // `self.capture = crate::settings::capture_cfg()` 没抄到，报告就会拿一场冻住的遮罩去判
    // 「背景跟着桌面走」，而它自己看不出来（#261）。分成两句读，是因为 `capture_cfg()` 自己就是
    // `settings::with(...)`，嵌进另一个 `with` 里就是同一把非重入锁锁两次。
    let settings_ff = crate::settings::capture_cfg().freeze_frame;
    let state_ff = mask::with(|m| m.capture.freeze_frame);
    rep.row(
        "premise freeze_frame off",
        Check::from(!settings_ff && !state_ff),
        format!(
            "settings 里 freeze_frame={settings_ff}、遮罩进场抄到的那份 {state_ff}（两枚都得是 false）"
        ),
    );

    // census 里那几扇遮罩窗，在关掉**之前**认出来：`close()` 之后窗就没了，而「摘出去的还回来了
    // 没有」只能在关掉之后读。
    let masks: Vec<Top> = parse_levels(&session::shim::top_levels())
        .into_iter()
        .filter(|t| t.title.starts_with(TITLE))
        .collect();

    let (asked, wanted, frames, said) =
        mask::with(|m| (m.live_asked, m.live_wanted(), m.live_frames, m.live_last.clone()));
    let reading = mask::with(|m| m.live_reading.clone());
    let held = mask::with(|m| m.live_excluded.clone());
    let problems = mask::with(|m| m.problems.clone());
    let slots = mask::with(|m| m.slots.clone());
    let first_swaps: Vec<String> = slots
        .iter()
        .map(|s| format!("{}={:?}", s.name, s.first_swap_ms))
        .collect();

    // 「问过闸没有」与「闸答了什么」分成两格，照搬 `MaskState` 自己那两枚字段的分法。detail 里
    // 逐扇印 `first_swap_ms` 是**唯一**能看见 `arm_live` 那一句静默停手的地方：有一扇窗一帧也没
    // 呈现过，它就在 `!slots.iter().all(|s| s.first_swap_ms.is_some())` 上直接交回 false，一句
    // 也不说、也不记 `problems` —— 而定时器照旧转、`live_wanted()` 照旧答 true，用户看到的是一张
    // 不动的图和一片安静（#433）。那一笔产品侧的债不在门上替它编一格状态，记在 `live arm debt`
    // 那一格 note 里，而这一格的 detail 是它在真机上的证人。
    //
    // 所以"停手"那一支的形状是 **true**，不是 false：`live_wanted()` 第三条腿在闸还没答时答 true
    // （那一句拒绝是搭着邮箱交回来的，得让定时器转上一圈才取得到），于是 false 只可能是「闸答过、
    // 答的是不许」（定时器**如约**停）或「这一场压根没去问」。把 false 念成停手就是把如约那一支念
    // 成债，而 2026-11-08 那一趟印出来的正是这句错话：`live_wanted=false`，逐扇 `first_swap_ms`
    // 却全是 `Some`（闸答的 `ReadDoesNotFollow`）。四枚读数各念各的，谁也不替谁编。
    let arm = match (&reading, wanted) {
        (Some(_), false) => {
            "闸答过了、答的是不许 ⇒ 定时器如约停（`live_wanted()` 答过之后逐字复读闸那一格）"
        }
        (Some(_), true) => "闸答了放行 ⇒ 定时器该转，下面 `live frame of` 那族按放行那一支判",
        (None, true) => "闸一直没答而定时器还在转 ⇒ 这一支才是 `arm_live` 静默停手的形状：\
                         逐扇 first_swap_ms 里该有一枚 None（全是 Some 就得另找原因）",
        (None, false) => "这一场压根没去问闸（freeze_frame 开着，或遮罩不在场上）",
    };
    rep.row(
        "live arm asked",
        Check::from(asked),
        format!("{arm}；live_wanted={wanted}，逐扇 first_swap_ms=[{}]", first_swaps.join(" ")),
    );

    match &reading {
        Some(r) => {
            // 判 Pass 而**不**判 `gate == Allowed`：PRD §5.2.1 步 2 后半支钉死的口径是「A 差 &&
            // B 不差 才许起刷新定时器；任何一格不成立 ⇒ 留冻的那一帧，并把为什么…说出去」，所以
            // 放行与那几枚拒绝都是**如约**的答，把它们判红等于替用户宣布他的屏幕读取腿该长什么
            // 样。这一格判的是另一件事：闸**答过**了没有 —— `None` 是「这一场压根没量过」，与
            // 「量过了、答是不许」不是一句话（#453）。
            rep.row(
                "gate read back",
                Check::Pass,
                format!(
                    "gate={:?} 摘之前={:?} 摘之后={:?} 要摘 {} 摘成 {} 两次抓屏 {} ms",
                    r.gate, r.stage_a, r.stage_b, r.asked, r.excluded, r.grab_ms
                ),
            );
            rep.note(
                "gate vs 2026-11-07",
                format!(
                    "解锁桌面上那趟 `--ignored` 探针量到的是「摘之前整块差 212751 枚、摘之后洋红\
                     采不到」⇒ 同一台机器上这一格该读到 Allowed；读到别的档不是失败，是这一趟\
                     没证到刷新（下面 `live frame of` 那族按拒绝那一支判）。本轮 gate={:?}",
                    r.gate
                ),
            );
        }
        None => rep.row(
            "gate read back",
            Check::Blocked,
            "这一场没量过（`live_reading` 是 None）—— 与「量过了、答是不许」不是一句话"
                .to_string(),
        ),
    }
    // 印的是 `live_last`（产品**自己**说过的那一句），不是拿读数重算一遍 `live_notice(r)`：
    // 重算就是同一句话的第二处作者（#281），而它俩要是分了家，屏幕上那一格念的与报告里印的就
    // 不是同一件事。
    rep.note(
        "gate said",
        if said.is_empty() {
            "-".to_string()
        } else {
            said.clone()
        },
    );
    // #433：闸的每一档都该留下一句话，`Allowed` 那一档也不例外（背景真的在动时画面只有约 3 fps，
    // 不说一句就被读成"卡住了"）。判空而不判内容 —— 内容是 `live_notice` 那五句，抄一份针在这里
    // 又是第二处作者。
    rep.row(
        "gate said a sentence",
        Check::from(!said.is_empty()),
        format!("live_last {} 个字（原文在上面 `gate said` 那一格）", said.chars().count()),
    );

    for slot in &slots {
        let tag = slot.name.clone();
        // 回灌那一支的算术。`open()` 自己 `take_revision()` 拿走了 1，所以刷新灌过 N 帧之后
        // `slot.revision == 1 + N`；一帧没灌（闸没过）则是 `revision == 1 && frames == 0`。两支
        // 各自咬得住，而且**互相**咬得住：放行却一帧没灌 = 刷新腿没干活；拒绝却灌了帧 = 正是
        // PRD §5.2.1 不许的形状（起刷新就是把截图工具自己拍进图里）。
        //
        // 前提两条，都只在这一档成立：① 这一趟进程里 `open()` 只跑过一次（`--live` 拒绝陪同，
        // 也没有 `--warm-rounds`），所以 `next_revision` 从 0 起、`open()` 拿走的就是 1；② 每一帧
        // 都灌到了**这一枚**槽。② 唯一的破法是 `live_publish` 在新那张图里认不出这块显示器
        // （拔线，§8.5 那一族）—— 那种时候 `live_frames` 照旧加、这一枚槽的 `revision` 不动，
        // 而 `live_last` 里躺着那句「认不出 …（显示器断开了？）」。detail 把它一起印出来，好让
        // 这两种红分得开（#453）。
        let (verdict, want_rev, why) = match &reading {
            None => (Check::Blocked, "-".to_string(), "闸没答过，这一格无从判"),
            Some(r) if r.gate == LiveGate::Allowed => (
                if frames > 0 && slot.revision == 1 + frames {
                    Check::Pass
                } else {
                    Check::Fail
                },
                (1 + frames).to_string(),
                "放行 ⇒ 灌过 N 帧之后 revision 该是 1 + N",
            ),
            Some(_) => (
                if frames == 0 && slot.revision == 1 {
                    Check::Pass
                } else {
                    Check::Fail
                },
                "1".to_string(),
                "闸拒绝 ⇒ 该留在冻住的那一帧（revision 1、frames 0）",
            ),
        };
        rep.row(
            &format!("live frame of {tag}"),
            verdict,
            format!(
                "live_frames={frames} revision={} 要求={want_rev}（{why}）；live_last={said:?}",
                slot.revision
            ),
        );

        // 灌进去的那一张**此刻还在 Qt 的纹理仓库里**、而且尺寸是这一枚显示器自己的。这是一枚
        // **只证到"存进去了"**的证人：`pin_shim.cpp` 的 `maskSelfCheck` 压根不调 `requestImage`，
        // 所以"URL 换了、页侧真的重取了、玻璃上真的是新那张"这一段它证不了 —— 那一段欠的证人由
        // `pixels on glass` 那一格 note 说破。
        let decode = shim::self_check(&slot.key);
        let sized = decode.contains(&format!("w={} h={}", slot.bounds.w, slot.bounds.h))
            && !decode.contains("null=1");
        rep.row(
            &format!("live texture of {tag}"),
            Check::from(sized),
            format!("key={} → {decode}", slot.key),
        );
    }

    rep.row(
        "no problems",
        Check::from(problems.is_empty()),
        problems.join(" | "),
    );
    qml_row(&mut rep);
    rep.note("mask state", mask::with(|m| m.line()));
    rep.note(
        "pixels on glass",
        "本档不读屏上像素：刷新腿整个循环持有 WDA_EXCLUDEFROMCAPTURE，而 PrintWindow 在一扇被摘\
         的窗上交回什么这台机器没有证人（affinity.rs 那枚 `--ignored` 探针读 print_window 是在 \
         exclude **之前**）。`dim of` / `hole of` 两族因此只在 --mask 那一档里跑；「灌进去了、\
         URL 换了、画面真的动了」这一段到本轮为止门上的正面证人还欠着。"
            .to_string(),
    );
    rep.note(
        "live arm debt",
        "`arm_live` 在「有一扇窗还没呈现过第一帧」时静默交回 false（不说、不记 problems），\
         本档 `live arm asked` 那一格的 detail 是它在真机上的证人；产品侧那一笔债记在这里。"
            .to_string(),
    );
    rep.note(
        "live ask",
        format!("要求 {asked_ms} ms（`main` 抬到不低于 {MIN_LIVE_MS} ms）"),
    );

    // 收工，并且验它收干净了。两族读数，各咬各的：
    // * 纹理：`close()` 逐枚 `drop_frame`，一张整屏纹理约 24 MB，漏一枚就是用户看得见的漏。
    // * affinity：`close()` 把 `live_excluded` `take` 空并逐枚 `restore`，所以**关掉之后**再读一遍
    //   census 那几扇遮罩窗；还摘着的就是真漏 —— 而那些 HWND 会被 Qt 回收、发给别的窗，到时候
    //   摘着的是**别人的**窗（`platform_windows::affinity` 头上那段警告）。用 census 那一份 HWND
    //   而不是 `held` 那一份来判，是因为闸拒绝时 `live_tick` 已经把 `held` 清空了（那趟线程自己
    //   `restore_all` 过），拿空表当判据就是拿"没东西可还"冒充"还了"（#274）。`held` 仍印在
    //   detail 里，它是「这一趟到底摘过没有」那一半的证人。
    let keys: Vec<String> = slots.iter().map(|s| s.key.clone()).collect();
    mask::with(|m| m.close());
    let left: Vec<&String> = keys
        .iter()
        .filter(|k| !shim::self_check(k).contains("null=1"))
        .collect();
    rep.row(
        "textures released",
        Check::from(left.is_empty()),
        format!(
            "{} of {} still in Qt's process after close",
            left.len(),
            keys.len()
        ),
    );

    let mut still = 0usize;
    let mut plain = 0usize;
    let mut gone = 0usize;
    for t in &masks {
        match platform_windows::affinity::current(mask::narrow(t.hwnd)) {
            Some(platform_windows::affinity::Affinity::Plain) => plain += 1,
            // 窗已经没了 ⇒ 那一档跟窗一起没了，没有东西可漏。这与「还摘着」不是一句话，也与
            // 「读到 Plain」不是一句话（#453）：前者证到了还，后者什么都没证到。
            None => gone += 1,
            Some(other) => {
                still += 1;
                rep.note(
                    &format!("affinity of {}", t.title),
                    format!("关掉之后仍读回 {other:?}"),
                );
            }
        }
    }
    rep.row(
        "affinity handed back",
        if still > 0 {
            Check::Fail
        } else if plain > 0 {
            Check::Pass
        } else {
            Check::Blocked
        },
        format!(
            "{plain} 扇读回 Plain、{still} 扇还摘着、{gone} 扇问不出（窗没了）；close 之前 \
             live_excluded 里躺着 {} 枚 HWND {:?}",
            held.len(),
            held
        ),
    );

    rep.finish("live check")
}

/// 遮罩压根没开起来的那一趟：报告仍要有一份，而它说的是**为什么这一档什么都没量**。
///
/// 与 `crate::reveal_check::open_failed` 同一枚形状 —— Blocked 而不是 Fail：开不起来是这一档
/// 没跑成，不是产品答错了。而它必须存在：`main` 那一侧「开了没有」与「量了没有」是两个时刻，
/// 少了这一格，开不起来的那一趟交回的是一份空报告，与"这一档没被点名"分不开（#453）。
pub fn open_failed(e: &str) -> (Check, String) {
    let mut rep = Report::new();
    rep.row(
        "live mask opened",
        Check::Blocked,
        format!("遮罩开不起来（{e}），这一档下面每一格都没得问"),
    );
    rep.finish("live check")
}
