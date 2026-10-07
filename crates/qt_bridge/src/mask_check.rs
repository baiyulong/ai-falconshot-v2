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
//! explain why rather than as a failure - plan §9.4 判据 ⑦.

use falcon_core::capture::ScreenSnapshot;
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
const TITLE: &str = "falconshot-mask-";

/// Per-channel floor below which a source pixel cannot carry a ratio. Black times
/// anything is black, so a dim measured over a black desktop is 0/0 and not 0.400.
const MIN_SOURCE: u32 = 32;

/// How far from the selection edge a sample has to be to count, in
/// device-independent pixels. The border is 2 DIP wide, and a sample on it is a
/// white pixel rather than a measurement; the slack is doubled so a half-pixel
/// rounding at the edge is not read as a dim of 1.0.
const BORDER_DIP: f64 = 4.0;

/// The tolerance on both ratio rows. Composition is allowed a rounding or two; a
/// dim layer that is 5% off is a different alpha, and 0.05 is far below that.
const RATIO_TOL: f64 = 0.05;

/// How long a *warm* mask may take to show its first frame: the same ceiling the
/// freeze is held to in [`crate::state::capture_selftest`], because both are inside
/// the one promise (§9.2, hot key to mask visible). Today's `--mask` run cannot
/// honour it and is not judged on it - it measures a process that starts Qt, loads
/// QML and then creates the window, in that order, for the first time. The number
/// it prints (1205 ms on this machine, 2026-10-06) is what the pre-warm has to
/// reduce, and the ceiling is what the pre-warmed run will be checked against.
const FIRST_FRAME_MS: u64 = 400;

/// Freeze and open the mask. Must run before the QML engine, so the windows exist
/// when the event loop starts painting.
pub fn open(shader: bool) -> Result<String, String> {
    let opened = mask::open_from_freeze(shader)?;
    let (pixels, hole) = mask::with(|m| (m.pixels_ms.unwrap_or(0), m.hole));
    // Every number this harness prints depends on which RHI drew them, and the only
    // way to ask is the environment - Qt has no "which backend did I pick" call that
    // does not need a window first.
    let rhi = std::env::var("QT_QUICK_BACKEND").unwrap_or_else(|_| "default".to_string());
    Ok(format!(
        "{} screen(s), backend={}, rhi={rhi}, freeze {} ms, pixels {} ms, hole={hole}, shader={shader}",
        opened.slots, opened.snap.backend, opened.ms, pixels
    ))
}

/// The ratio of one sample: what the mask shows, over what was frozen there.
/// Summing the three channels instead of comparing them one by one makes the
/// number one division rather than three, so the median of a set of them is the
/// median of one quantity.
fn ratio(src: [u8; 4], got: [u8; 4]) -> f64 {
    let a = src[0] as u32 + src[1] as u32 + src[2] as u32;
    let b = got[0] as u32 + got[1] as u32 + got[2] as u32;
    if a == 0 {
        return f64::NAN;
    }
    b as f64 / a as f64
}

fn median(v: &[f64]) -> Option<f64> {
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
struct Sides {
    inside: Vec<f64>,
    outside: Vec<f64>,
    dark: usize,
    off_desktop: usize,
    on_border: usize,
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
fn samples(
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
fn flag_column(rep: &mut Report, win: &Top) {
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
fn ratio_row(count: usize, value: Option<f64>, want: f64, sides: &Sides) -> (Check, String) {
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
pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
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

    // The default hole, asserted as a relationship: the middle half of the primary
    // is a quarter of its area and inside it. Content-independent, so it holds on
    // any desktop on any number of screens.
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
