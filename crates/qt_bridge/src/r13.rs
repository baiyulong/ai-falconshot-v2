//! R13 - which capture path can see which kind of window.
//!
//! The plan asks for a matrix before M1 can be signed off: GDI, WGC,
//! `PrintWindow` and Qt's own screen grab each have to be measured against the
//! same pixels, because the product's promise "a pin can be captured again" is
//! only true on the paths that actually see the pin. Measuring that through the
//! ordinary code path is impossible - xcap's window capture retries `PrintWindow`
//! with three flag sets and then falls back to a `BitBlt` of the region, so a
//! success there may mean "the desktop behind the window was read". Each leg is
//! therefore called separately, and each answer is printed.
//!
//! The control is a solid magenta patch this process plants at a place it
//! chooses. A photograph would make the matrix depend on what happened to be on
//! screen; a colour makes "this path did not see the window" a number.

use std::collections::HashMap;

use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect};
use falcon_core::pin::PinId;

use crate::state::Check;
use crate::{capture, session, state};

/// Magenta: the least-used colour in ordinary desktop content, so a hit here is
/// very unlikely to be a coincidence.
const CTRL: [u8; 4] = [255, 0, 255, 255];
const CTRL_W: u32 = 240;
const CTRL_H: u32 = 160;
/// Per-channel distance still counted as the control. A composited path is
/// allowed one rounding; it is not allowed a different colour.
const TOL: u8 = 8;
/// Well inside the smallest monitor this project targets, and away from 0,0 in
/// case a window manager refuses to place a window in a corner.
const CTRL_AT: PhysPoint = PhysPoint::new(200, 200);

/// The control patch: flat magenta with a black edge, so a path that captures a
/// *slightly* wrong rectangle still reports most of the colour rather than
/// nothing, and the ratio says how wrong it was.
pub fn control_frame() -> Frame {
    let mut f = Frame::filled(CTRL_W, CTRL_H, CTRL).expect("the control patch is 240x160");
    let (w, h) = (CTRL_W, CTRL_H);
    for r in [
        PhysRect::new(0, 0, w, 2),
        PhysRect::new(0, h as i32 - 2, w, 2),
        PhysRect::new(0, 0, 2, h),
        PhysRect::new(w as i32 - 2, 0, 2, h),
    ] {
        f.fill_rect(&r, [0, 0, 0, 255]);
    }
    f
}

/// One frame, measured. `ctrl` is the answer to "did this path see the window",
/// `flat` is the answer to "or did it hand back one repeated colour".
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub w: u32,
    pub h: u32,
    pub ctrl: f64,
    pub flat: f64,
}

impl Stats {
    pub(crate) fn of(f: &Frame) -> Stats {
        let n = f.width as usize * f.height as usize;
        if n == 0 {
            return Stats {
                w: f.width,
                h: f.height,
                ctrl: 0.0,
                flat: 1.0,
            };
        }
        let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
        let mut modal = 0usize;
        let mut ctrl = 0usize;
        for px in f.pixels.as_chunks::<4>().0 {
            let rgb = [px[0], px[1], px[2]];
            let seen = counts.entry(rgb).or_insert(0);
            *seen += 1;
            modal = modal.max(*seen);
            if rgb[0].abs_diff(CTRL[0]) <= TOL
                && rgb[1].abs_diff(CTRL[1]) <= TOL
                && rgb[2].abs_diff(CTRL[2]) <= TOL
            {
                ctrl += 1;
            }
        }
        Stats {
            w: f.width,
            h: f.height,
            ctrl: ctrl as f64 / n as f64,
            flat: modal as f64 / n as f64,
        }
    }

    /// The one-line reading of a frame: size, how much of it is the control, how
    /// much of it is one repeated colour. Also what `--mask` prints per
    /// `PrintWindow` flag, because a mask window has no control colour in it and
    /// `flat` is the number that says so.
    pub(crate) fn line(&self) -> String {
        format!(
            "{}x{} ctrl={:.3} flat={:.3}",
            self.w, self.h, self.ctrl, self.flat
        )
    }
    /// Half the control colour, which is what a correctly placed patch on a
    /// correctly cropped frame produces (the edge is black by design).
    fn saw_control(&self) -> bool {
        self.ctrl > 0.5
    }
}

/// The rows that decide the exit code and the rows that only report. Shared with
/// [`crate::mask_check`], because a mask has the same two kinds of line: a verdict
/// and a reading.
pub(crate) struct Report {
    out: Vec<String>,
    worst: Check,
}

impl Report {
    /// Nothing said yet, and nothing failing yet.
    pub(crate) fn new() -> Report {
        Report {
            out: Vec::new(),
            worst: Check::Pass,
        }
    }

    pub(crate) fn row(&mut self, label: &str, verdict: Check, detail: String) {
        if verdict > self.worst {
            self.worst = verdict;
        }
        self.out
            .push(format!("{label:<22} {:<4} {detail}", verdict.tag()));
    }

    /// A measurement with no verdict attached. The matrix exists to record which
    /// paths are blind, and "blind" is a property of the machine rather than a
    /// failing test - until it is the only path left, which the scored rows above
    /// are the ones to notice.
    pub(crate) fn note(&mut self, label: &str, detail: String) {
        self.out.push(format!("{label:<22} {:<4} {detail}", "-"));
    }

    pub(crate) fn finish(mut self, title: &str) -> (Check, String) {
        let verdict = self.worst;
        self.out.push(format!(
            "{title}: {}",
            match verdict {
                Check::Pass => "PASS",
                Check::Blocked => "BLOCKED",
                Check::Fail => "FAIL",
            }
        ));
        (verdict, self.out.join("\n"))
    }
}

/// Plant the control. Must run before the QML engine, so the pin window exists
/// when the event loop starts painting.
pub fn plant() -> Result<PinId, String> {
    state::with(|s| s.add(control_frame(), CTRL_AT))
}

/// One top-level window as the shim's machine-readable line describes it.
#[derive(Debug)]
pub(crate) struct Top {
    pub(crate) hwnd: u64,
    pub(crate) class: String,
    pub(crate) phys: PhysRect,
    pub(crate) visible: bool,
    /// Empty for a window with no title. Titles are matched without spaces on
    /// purpose - this is a whitespace-split field list, and `pinTopLevels` is a
    /// debugging aid whose field order is not a contract.
    pub(crate) title: String,
}

pub(crate) fn parse_levels(raw: &str) -> Vec<Top> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut hwnd = None;
        let mut class = String::new();
        let mut phys = None;
        let mut visible = false;
        let mut title = String::new();
        for field in line.split_whitespace() {
            if let Some(v) = field.strip_prefix("hwnd=0x") {
                hwnd = u64::from_str_radix(v, 16).ok();
            } else if let Some(v) = field.strip_prefix("class=") {
                class = v.to_string();
            } else if let Some(v) = field.strip_prefix("visible=") {
                visible = v == "1";
            } else if let Some(v) = field.strip_prefix("title=") {
                title = v.to_string();
            } else if let Some(v) = field.strip_prefix("phys=") {
                let n: Vec<i32> = v.split(',').filter_map(|s| s.parse().ok()).collect();
                if n.len() == 4 {
                    phys = Some(PhysRect::new(
                        n[0],
                        n[1],
                        n[2].max(1) as u32,
                        n[3].max(1) as u32,
                    ));
                }
            }
        }
        if let (Some(hwnd), Some(phys)) = (hwnd, phys) {
            out.push(Top {
                hwnd,
                class,
                phys,
                visible,
                title,
            });
        }
    }
    out
}

/// The window this process made for the control pin: the visible top-level whose
/// rectangle overlaps the patch by the most. Chosen by overlap rather than by
/// class name, because the class is `QQuickWindow` for every window here and the
/// one that matters is the one the patch is in.
fn pick_window(raw: &str, want: &PhysRect) -> Option<Top> {
    parse_levels(raw)
        .into_iter()
        .filter(|t| t.visible)
        .map(|t| (t.phys.intersection(want).map(|i| i.area()).unwrap_or(0), t))
        .max_by_key(|(area, _)| *area)
        .filter(|(area, _)| *area > 0)
        .map(|(_, t)| t)
}

/// `3072x1920 dpr=2.00` -> `(3072, 1920)`. Also what `--mask` reads the same
/// `pinScreenGrabInfo` line with, because the second readback path is this one's
/// fourth leg and both have to trust the size before they trust the pixels.
pub(crate) fn parse_size(info: &str) -> Option<(u32, u32)> {
    let head = info.split_whitespace().next()?;
    let (w, h) = head.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Read the screen, then read the same screen four other ways, and print what
/// each one saw. Runs after the event loop, while the windows still exist.
pub fn measure(id: PinId) -> (Check, String) {
    let mut rep = Report {
        out: Vec::new(),
        worst: Check::Pass,
    };
    let view = state::with(|s| s.view_data(id));
    let rect = PhysRect::new(
        view.pos_x,
        view.pos_y,
        view.size_w.max(1) as u32,
        view.size_h.max(1) as u32,
    );
    rep.out.push(format!(
        "r13 control: 品红色块 {CTRL_W}x{CTRL_H} @{},{} 窗口矩形 {rect:?}",
        CTRL_AT.x, CTRL_AT.y
    ));

    // The window this process made for the patch, found by overlap rather than by
    // name - every Qt window here is the same class.
    let levels = session::shim::top_levels();
    let own = pick_window(&levels, &rect);
    match &own {
        None => rep.row(
            "own window",
            Check::Fail,
            format!("找不到贴图自己的窗口 - shim 报告 {levels:?}"),
        ),
        Some(t) => {
            rep.row(
                "own window",
                Check::Pass,
                format!("hwnd=0x{:x} class={} phys={:?}", t.hwnd, t.class, t.phys),
            );
        }
    }

    // Whether the desktop is even obliged to show the patch here. Without this
    // the two screen legs cannot tell "a capture path is blind to our window"
    // from "another window is in front of it", and those are different problems:
    // one is R13, the other is a z-order question the product has not answered
    // yet (PRD §5.9.1's "always on top" is a user setting, not a guarantee).
    let centre = PhysPoint::new(rect.x + rect.w as i32 / 2, rect.y + rect.h as i32 / 2);
    let shown = platform_windows::proc::window_at_point(centre);
    let on_top = match (&own, shown) {
        (Some(t), Some(h)) => h == t.hwnd,
        _ => false,
    };
    let who = shown.map(|h| {
        let app = platform_windows::proc::pid_of_window(h)
            .and_then(platform_windows::proc::process_name)
            .unwrap_or_else(|| "未知进程".to_string());
        let title = platform_windows::proc::window_title(h).unwrap_or_default();
        (app, format!("「{title}」 hwnd=0x{h:x}"))
    });
    // A locked workstation puts a full-screen LockApp window, backed by LogonUI,
    // above every other window, and nothing short of unlocking moves it. Naming
    // that here is what turns `screen:gdi SKIP` into a statement about the
    // session rather than a bug report about the capture path.
    let lock = who
        .as_ref()
        .map(|(app, _)| {
            let a = app.to_ascii_lowercase();
            a.contains("lockapp") || a.contains("logonui")
        })
        .unwrap_or(false);
    let reason = if lock {
        "会话已锁屏，桌面两列在本机本次不可测"
    } else {
        "被别的窗口盖住"
    };
    if !on_top {
        rep.row(
            "z order",
            Check::Blocked,
            format!(
                "贴图中心 ({},{}) 处桌面显示的是 {}{}，不是自家贴图窗 - {reason}",
                centre.x,
                centre.y,
                who.as_ref()
                    .map(|(app, _)| app.clone())
                    .unwrap_or_else(|| "没有窗口".to_string()),
                who.as_ref()
                    .map(|(_, rest)| rest.clone())
                    .unwrap_or_default(),
            ),
        );
    } else {
        rep.row(
            "z order",
            Check::Pass,
            format!("({},{}) 处桌面上就是自家贴图窗", centre.x, centre.y),
        );
    }

    // Every leg needs the same reference pixels to be measured against, and the
    // frozen desktop is the one this project already trusts - it is the path the
    // mask UI will use. Without it there is nothing to compare, so stop.
    let frozen = match capture::freeze() {
        Err(e) => {
            rep.row(
                "screen:freeze",
                Check::Blocked,
                format!("{e} - no leg can be scored"),
            );
            return rep.finish("r13 matrix");
        }
        Ok(f) => f,
    };
    // The row name carries the backend, because a run under `--features wgc`
    // must not print the same label as a run without it: the two columns are
    // only comparable if each line says which one it came from.
    let desktop_leg = format!("screen:{}", frozen.snap.backend);
    rep.out
        .push(format!("  {}", capture::snapshot_line(&frozen.snap)));

    // Leg 1: the frozen desktop, cropped to the control. This row is the control
    // over the control: if the pin the tool itself drew is not on the screen it
    // drew, no other row means anything.
    let primary = frozen
        .snap
        .monitors
        .iter()
        .find(|m| m.info.primary)
        .map(|m| m.info.bounds)
        .unwrap_or(frozen.snap.virtual_bounds);
    let on_screen = match frozen.snap.capture(&rect) {
        Err(e) => {
            rep.row(
                desktop_leg.as_str(),
                Check::Blocked,
                format!("{e} - the patch is off the desktop"),
            );
            None
        }
        Ok(cap) => {
            let s = Stats::of(&cap.frame);
            if on_top {
                rep.row(
                    desktop_leg.as_str(),
                    Check::from(s.saw_control()),
                    format!(
                        "{} {}",
                        s.line(),
                        if s.saw_control() {
                            "桌面采集看得见自家贴图"
                        } else {
                            "窗口在最上层而桌面采集里没有它 - 这是 R13 要抓的缺陷"
                        }
                    ),
                );
            } else {
                rep.row(
                    desktop_leg.as_str(),
                    Check::Blocked,
                    format!("{} - {reason}", s.line()),
                );
            }
            Some(s)
        }
    };

    // Leg 2: Qt's own backend. The whole primary screen is grabbed with no offset
    // so that a coordinate-space disagreement shows up as a size difference in
    // the printed line rather than as a black frame.
    let bytes = session::shim::screen_grab();
    let info = session::shim::screen_grab_info();
    match parse_size(&info) {
        None => rep.row(
            "screen:qt",
            Check::Blocked,
            format!("Qt produced nothing ({info:?})"),
        ),
        Some((gw, gh)) if bytes.len() != gw as usize * gh as usize * 4 => rep.row(
            "screen:qt",
            Check::Blocked,
            format!("Qt said {gw}x{gh} but handed over {} bytes", bytes.len()),
        ),
        Some((gw, gh)) => {
            let frame = match Frame::from_rgba(gw, gh, bytes) {
                Ok(f) => f,
                Err(e) => {
                    rep.row("screen:qt", Check::Blocked, e.to_string());
                    return rep.finish("r13 matrix");
                }
            };
            // The ratio between what Qt returned and what Win32 says that screen
            // is, which is the difference between "Qt grabs device pixels" and
            // "Qt grabs logical pixels" - measured, not assumed.
            let sx = gw as f64 / primary.w.max(1) as f64;
            let sy = gh as f64 / primary.h.max(1) as f64;
            let ox = ((rect.x - primary.x) as f64 * sx).round().max(0.0) as i32;
            let oy = ((rect.y - primary.y) as f64 * sy).round().max(0.0) as i32;
            let cw = (rect.w as f64 * sx).round().max(1.0) as u32;
            let ch = (rect.h as f64 * sy).round().max(1.0) as u32;
            let crop = PhysRect::new(
                ox,
                oy,
                cw.min(gw.saturating_sub(ox as u32)),
                ch.min(gh.saturating_sub(oy as u32)),
            );
            match frame.crop(&crop) {
                Err(e) => rep.row(
                    "screen:qt",
                    Check::Fail,
                    format!(
                        "{e} - Qt returned {gw}x{gh}, 桌面物理 {primary:?}, ratio {sx:.2}x{sy:.2}"
                    ),
                ),
                Ok(part) => {
                    let s = Stats::of(&part);
                    let geometry = format!(
                        "{} (Qt {gw}x{gh} vs 物理 {}x{}, ratio {sx:.2}x{sy:.2})",
                        s.line(),
                        primary.w,
                        primary.h
                    );
                    if on_top {
                        rep.row("screen:qt", Check::from(s.saw_control()), geometry);
                    } else {
                        rep.row(
                            "screen:qt",
                            Check::Blocked,
                            format!("{geometry} - {reason}"),
                        );
                    }
                }
            }
        }
    }

    // Legs 3-5: `PrintWindow`, one flag at a time, on the window this process
    // made. A frameless QQuickWindow is the hardest case the product has: no
    // caption, per-pixel alpha, DirectComposition-backed surface.
    let mut answered: Vec<String> = Vec::new();
    if let Some(t) = &own {
        for flag in [2u32, 0, 4] {
            match platform_windows::print::print_window(t.hwnd, t.phys.w, t.phys.h, flag) {
                None => rep.note(
                    &format!("pw:flag {flag}"),
                    format!("{}x{} 无像素", t.phys.w, t.phys.h),
                ),
                Some(f) => {
                    let s = Stats::of(&f);
                    if s.saw_control() {
                        answered.push(format!("{flag}"));
                    }
                    rep.note(&format!("pw:flag {flag}"), s.line());
                }
            }
        }
        // Only a window that every flag refused, while the desktop is showing
        // it, is the failure this matrix was built to catch: a pin that is
        // behind something else still has to be able to paint itself.
        let scored = on_top && on_screen.map(|s| s.saw_control()).unwrap_or(false);
        rep.row(
            "pw:own window",
            Check::from(!answered.is_empty() || !scored),
            if answered.is_empty() {
                "三种 flag 都没有让自家贴图窗自己画出内容".to_string()
            } else {
                format!("flag {} 可以", answered.join("/"))
            },
        );

        // The ladder the product currently uses, for contrast: xcap's own window
        // path, whose answer is a mix of the rows above and possibly of a BitBlt.
        match capture::service().capture_window(t.hwnd as u32) {
            Err(e) => rep.note("xcap:window", e.to_string()),
            Ok(c) => rep.note(
                "xcap:window",
                format!("{} rect={:?}", Stats::of(&c.frame).line(), c.rect),
            ),
        }
    }

    // Foreign windows: the same `PrintWindow` flag that has to work on our own
    // pin, run against whatever else is up. The desktop column is not a ground
    // truth - an occluded window shows its occluder - so both are printed and
    // neither is scored.
    match capture::service().windows(false) {
        Err(e) => rep.note("foreign windows", e.to_string()),
        Ok(list) => {
            for w in list.iter().take(6) {
                let r = w.visible_bounds();
                let pw = platform_windows::print::print_window(u64::from(w.hwnd), r.w, r.h, 2)
                    .map(|f| Stats::of(&f).line())
                    .unwrap_or_else(|| "无像素".to_string());
                let screen = frozen
                    .snap
                    .capture(&r)
                    .map(|c| Stats::of(&c.frame).line())
                    .unwrap_or_else(|e| e.to_string());
                rep.note(
                    &format!(
                        "{} {}",
                        w.app_name,
                        if w.title.is_empty() {
                            "(无标题)"
                        } else {
                            w.title.as_str()
                        }
                    ),
                    format!("pw={pw} | 桌面={screen} | {r:?}"),
                );
            }
        }
    }

    state::with(|s| s.close(&[id]));
    rep.finish("r13 matrix")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_patch_is_mostly_the_control_colour() {
        // If the patch were ever changed to something a crop could easily lose,
        // every row of the matrix would silently start meaning "not measured".
        let s = Stats::of(&control_frame());
        assert_eq!((s.w, s.h), (CTRL_W, CTRL_H));
        assert!(s.saw_control(), "{s:?}");
        // Not uniform: the black edge has to be in there, or `flat` could not
        // tell a real capture from a path that filled one colour.
        assert!(s.flat < 1.0, "{s:?}");
        assert_eq!(control_frame().get(0, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn a_blank_frame_reads_as_flat_and_sees_nothing() {
        let black = Frame::new(8, 8).unwrap();
        let s = Stats::of(&black);
        assert!(!s.saw_control());
        assert_eq!(s.flat, 1.0);
    }

    #[test]
    fn a_top_level_line_is_read_by_field_name_not_by_position() {
        let raw = "class=QQuickWindow visible=1 hwnd=0x1234 phys=200,200,240,160\n\
                   class=Other visible=0 hwnd=0x5 phys=0,0,10,10\n";
        let tops = parse_levels(raw);
        assert_eq!(tops.len(), 2);
        assert_eq!(tops[0].hwnd, 0x1234);
        assert_eq!(tops[0].phys, PhysRect::new(200, 200, 240, 160));
        assert!(tops[0].visible);
        assert!(!tops[1].visible);
    }

    #[test]
    fn the_own_window_is_the_one_the_patch_sits_in() {
        let raw = "class=QQuickWindow visible=1 hwnd=0xaaa phys=210,210,240,160\n\
                   class=QQuickWindow visible=1 hwnd=0xbbb phys=10,10,300,300\n";
        let want = PhysRect::new(200, 200, 240, 160);
        let got = pick_window(raw, &want).expect("a window overlaps the patch");
        // `0xaaa` is the patch's own window offset by ten pixels, so it covers
        // more of the want-rect than the bigger `0xbbb` that only clips its
        // corner - which is the rule: largest overlap wins, not largest window.
        assert_eq!(got.hwnd, 0xaaa);
        assert!(pick_window("class=x visible=0 hwnd=0x1 phys=200,200,240,160", &want).is_none());
        assert!(pick_window("nonsense", &want).is_none());
    }

    #[test]
    fn the_grab_size_is_read_before_the_pixels_are_trusted() {
        assert_eq!(parse_size("3072x1920 dpr=2.00"), Some((3072, 1920)));
        assert_eq!(parse_size("null"), None);
        assert_eq!(parse_size("3072 dpr=2.00"), None);
    }
}
