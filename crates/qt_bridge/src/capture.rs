//! The frozen screen, and the paths from a rectangle of it to a pin.
//!
//! One rule drives this whole module: the whole screen is captured **before** any
//! mask window exists, and every later read - the selection crop, the colour pick,
//! the magnifier, "capture again with the mask still up" - reads from that one
//! snapshot (plan §4-M1). A second `BitBlt` after the mask appears is how a
//! screenshot ends up containing the screenshot tool.
//!
//! The snapshot is cached on purpose. Freezing 3072x1920 measured 142-155 ms (P1/P5),
//! so the mask path cannot afford to re-read the screen per gesture, and the
//! magnifier would rebuild it on every mouse move.

use std::sync::{Mutex, OnceLock, PoisonError};

use falcon_core::capture::{CaptureService, ScreenSnapshot, WindowInfo, XcapCapture};
use falcon_core::geometry::{PhysPoint, PhysRect};
use falcon_core::pin::PinId;
use platform_windows::proc::{
    cursor_pos, pid_of_window, process_name, window_at_point, window_title,
};
use platform_windows::win::WinExtras;

use crate::state;

/// The capture service, with the Windows answers filled in behind `WindowExtras`.
///
/// Built per call rather than cached: it is two boxes and a pid, and every
/// enumeration has to be a fresh look at the desktop anyway - monitors get
/// unplugged and windows get closed between one question and the next.
pub fn service() -> CaptureService<WinExtras> {
    CaptureService::with_extras(Box::new(XcapCapture), Box::new(WinExtras))
}

/// A freeze, with how long it took. The timing is part of the product's promise
/// (plan P1: hot key to mask visible inside 150 ms), so it is measured here rather
/// than assumed.
pub struct Frozen {
    pub snap: ScreenSnapshot,
    pub ms: u64,
}

static FROZEN: OnceLock<Mutex<Option<ScreenSnapshot>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<ScreenSnapshot>> {
    FROZEN.get_or_init(|| Mutex::new(None))
}

/// Step 1 of every capture flow. Also replaces any previous freeze: a stale
/// snapshot is what a "capture the screen as it was two minutes ago" bug looks
/// like, so a new flow always takes a new frame (PRD §8.5 makes the same point the
/// other way round - when the resolution changes, the old frame is invalid).
pub fn freeze() -> Result<Frozen, String> {
    let started = std::time::Instant::now();
    let snap = service().freeze().map_err(|e| e.to_string())?;
    let ms = started.elapsed().as_millis() as u64;
    *slot().lock().unwrap_or_else(PoisonError::into_inner) = Some(snap.clone());
    Ok(Frozen { snap, ms })
}

/// The frame the current flow is working from, if there is one.
pub fn current() -> Option<ScreenSnapshot> {
    slot()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The flow is over; the pixels go away with it.
pub fn clear() {
    *slot().lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Freeze if nothing is frozen yet, otherwise reuse the frame the user is looking at.
fn working_frame() -> Result<ScreenSnapshot, String> {
    match current() {
        Some(snap) => Ok(snap),
        None => freeze().map(|f| f.snap),
    }
}

/// §5.3's output for one rectangle: stitched across monitors, clipped to the
/// desktop, and pinned where it was on screen.
pub fn pin_rect(want: &PhysRect) -> Result<PinId, String> {
    let snap = working_frame()?;
    let cap = snap.capture(want).map_err(|e| e.to_string())?;
    state::with(|s| s.add(cap.frame, PhysPoint::new(cap.rect.x, cap.rect.y)))
}

/// §5.3.2 - the window under a point, as the user sees it: topmost in z-order.
pub fn window_at(p: PhysPoint) -> Option<WindowInfo> {
    service().window_at(p)
}

/// §5.3.3 - the window the pointer is over right now.
pub fn window_under_cursor() -> Option<WindowInfo> {
    window_at(cursor_pos()?)
}

/// The middle of the primary monitor, in physical pixels - what `--snap` grabs
/// when it is not told a rectangle. The point of the flag is to prove that real
/// screen pixels reach a real window, not to stand in for a selection UI that does
/// not exist yet.
pub fn centre_rect(width: u32, height: u32) -> Result<PhysRect, String> {
    let snap = working_frame()?;
    let m = snap
        .monitors
        .iter()
        .find(|m| m.info.primary)
        .or_else(|| snap.monitors.first())
        .ok_or("没有可截取的显示器")?;
    let b = m.info.bounds;
    Ok(PhysRect::new(
        b.x + (b.w as i32 - width as i32) / 2,
        b.y + (b.h as i32 - height as i32) / 2,
        width,
        height,
    ))
}

/// `--snap`: freeze, take the middle of the primary monitor, pin it. Returns the
/// new pin and the sentence describing the frame it came from.
pub fn snap_centre(width: u32, height: u32) -> Result<(PinId, String), String> {
    let f = freeze()?;
    let want = centre_rect(width, height)?;
    let id = pin_rect(&want)?;
    Ok((
        id,
        format!("{} ms, {:?} from {}", f.ms, want, snapshot_line(&f.snap)),
    ))
}

/// A rectangle in the shape `--rect x,y,w,h` carries.
pub fn rect_arg(raw: &str) -> Option<PhysRect> {
    let n: Vec<i32> = raw
        .split(',')
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().parse::<i32>().ok())
        .collect::<Option<Vec<_>>>()?;
    if n.len() != 4 {
        return None;
    }
    Some(PhysRect::new(
        n[0],
        n[1],
        n[2].max(1) as u32,
        n[3].max(1) as u32,
    ))
}

/// A whole window as its own picture. This path does not use the freeze: a window
/// shot asks the DWM for that window's content, which is what makes it survive
/// being behind another window - and what makes its size drift (P5), hence the
/// re-crop inside `capture_window`.
pub fn pin_window(hwnd: u32) -> Result<PinId, String> {
    let cap = service().capture_window(hwnd).map_err(|e| e.to_string())?;
    state::with(|s| s.add(cap.frame, PhysPoint::new(cap.rect.x, cap.rect.y)))
}

/// `monitor id  WxH @x,y scale` per line, plus the union. Printed by `--capture`
/// and asserted on by the selftest, because "the enumeration is wrong" and "the
/// enumeration is right but the crop is wrong" need different fixes.
pub fn monitors_line() -> String {
    match service().monitors() {
        Ok(ms) => ms
            .iter()
            .map(|m| {
                format!(
                    "{} {}x{}@{},{} scale={:.2}{}",
                    m.id,
                    m.bounds.w,
                    m.bounds.h,
                    m.bounds.x,
                    m.bounds.y,
                    m.scale.ratio(),
                    if m.primary { " primary" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(" | "),
        Err(e) => format!("error: {e}"),
    }
}

/// The point a screen-reading row takes as the desktop's own: the centre of the virtual
/// bounds.
///
/// §10 第 42 项 ② made this a reading a row quotes rather than a detail of one log line:
/// a `readable` row has to say *what the desktop was* when it declined to answer, and the
/// answer it prints has to be the same coordinate [`snapshot_line`] prints, or the two
/// sentences describe two different screens.
pub fn centre_point(snap: &ScreenSnapshot) -> PhysPoint {
    PhysPoint::new(
        snap.virtual_bounds.x + snap.virtual_bounds.w as i32 / 2,
        snap.virtual_bounds.y + snap.virtual_bounds.h as i32 / 2,
    )
}

/// What the desktop is at [`centre_point`], already formatted, so a row that quotes it and
/// the line that prints it cannot disagree about how the reading is written.
pub fn centre_pixel(snap: &ScreenSnapshot) -> String {
    format!("{:?}", snap.color_at(centre_point(snap)))
}

/// One line describing a snapshot: backend, monitors, union, colour-pick probe.
pub fn snapshot_line(snap: &ScreenSnapshot) -> String {
    format!(
        "backend={} monitors={} virtual={}x{}@{},{} pixel@centre={}",
        snap.backend,
        snap.monitors.len(),
        snap.virtual_bounds.w,
        snap.virtual_bounds.h,
        snap.virtual_bounds.x,
        snap.virtual_bounds.y,
        centre_pixel(snap),
    )
}

/// Whether a process name belongs to the workstation lock.
///
/// A locked session puts a full-screen `LockApp` window, backed by `LogonUI`, above every
/// other window and nothing short of unlocking moves it. Naming that is what turns a
/// screen-reading row's SKIP into a statement about the session rather than a bug report
/// about the capture path - which since §10 第 41 项 ⑤ is *not* an excuse to explain a
/// row away by it: P40 read better ink on the locked run than on the unlocked one, so the
/// lock is one input parameter of a row, not a verdict over a round.
pub(crate) fn is_lock(app: &str) -> bool {
    let a = app.to_ascii_lowercase();
    a.contains("lockapp") || a.contains("logonui")
}

/// What the desktop is showing at its own centre right now.
///
/// §10 第 41 项 ④: `--r13` has been able to say 会话已锁屏 since P39 and the other three
/// gauges said nothing at all, so a run of `--ink` alone left no scene behind it and the
/// only witness was a `tasklist` in a batch file - which was broken (`//FI`, the Git-Bash
/// spelling) until this round. The reading lives here, next to the capture paths, because
/// it is an input of every row that reads the screen and not a property of one report.
///
/// No pixels are grabbed: the point is the primary monitor's centre and the answer is which
/// window the shell puts there. A `--pin` run does not look at the desktop (§10 第 41 项 ⑧)
/// and still gets to say what was there.
///
/// [`Scene::hwnd`] is the reading and not a word in the sentence: a row that graded "the
/// line does not begin with 没有" would be grading this file's own phrasing, which is the
/// shape §10 第 41 项 ⑥ refuses - 判接线的格，对面必须是操作系统.
pub struct Scene {
    pub line: String,
    pub hwnd: Option<u64>,
}

pub fn desktop_scene() -> Scene {
    let refused = |why: String| Scene {
        line: format!("没有桌面读数：{why}"),
        hwnd: None,
    };
    let monitors = match service().monitors() {
        Ok(m) => m,
        Err(e) => return refused(format!("monitors() 报 {e}")),
    };
    let Some(m) = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
    else {
        return refused(format!("枚举到 {} 个显示器", monitors.len()));
    };
    let centre = PhysPoint::new(
        m.bounds.x + m.bounds.w as i32 / 2,
        m.bounds.y + m.bounds.h as i32 / 2,
    );
    match window_at_point(centre) {
        None => refused(format!("({},{}) 处桌面没有任何窗口", centre.x, centre.y)),
        Some(hwnd) => {
            let app = pid_of_window(hwnd)
                .and_then(process_name)
                .unwrap_or_else(|| "未知进程".to_string());
            let title = window_title(hwnd).unwrap_or_default();
            Scene {
                line: format!(
                    "({},{}) 上是 {app}「{title}」 hwnd=0x{hwnd:x} 会话锁屏={}",
                    centre.x,
                    centre.y,
                    is_lock(&app)
                ),
                hwnd: Some(hwnd),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §10 第 41 项 ④: the rule that turns a screen row's refusal into a statement about the
    /// session is two substrings, and until this round those two substrings lived inside one
    /// report's `map(|(app, _)| …)` closure - so the other three gauges could not name the
    /// same thing without writing the rule a second time. A name a test can hit is what keeps
    /// them honest, and the case list is the shell's own spelling, not this file's: the name
    /// comes back from `QueryFullProcessImageNameW` in whatever case the exe's own directory
    /// entry has.
    #[test]
    fn a_lock_is_named_by_the_process_and_not_by_a_pixel() {
        assert!(is_lock("LockApp.exe"));
        assert!(is_lock("LOCKAPP.EXE"));
        assert!(is_lock("LogonUI.exe"));
        assert!(!is_lock("falconshot-desktop.exe"));
        assert!(!is_lock("explorer.exe"));
        // The desktop's own window, which is what a normal unlocked session answers here.
        assert!(!is_lock("Progman"));
        assert!(!is_lock(""));
    }

    #[test]
    fn a_rect_argument_is_four_numbers_and_nothing_else() {
        assert_eq!(
            rect_arg("10,20,300,400"),
            Some(PhysRect::new(10, 20, 300, 400))
        );
        // Spaces are typed by humans; three numbers is a typo, and a typo that
        // silently becomes a rect is worse than a rect that is refused.
        assert_eq!(
            rect_arg(" 10 , 20 , 300 , 400 "),
            Some(PhysRect::new(10, 20, 300, 400))
        );
        assert_eq!(rect_arg("10,20,300"), None);
        assert_eq!(rect_arg("10,20,300,abc"), None);
        assert_eq!(rect_arg(""), None);
    }

    #[test]
    fn a_zero_sized_rect_becomes_one_pixel_rather_than_nothing() {
        // An empty rect is refused by `ScreenSnapshot::capture` with `Empty`, so a
        // `--rect 0,0,0,0` would read as "the desktop is broken". Clamping to one
        // pixel keeps the diagnostic about the region the user named.
        assert_eq!(rect_arg("5,6,0,-40"), Some(PhysRect::new(5, 6, 1, 1)));
    }
}
