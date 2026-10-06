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
use platform_windows::proc::cursor_pos;
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

/// One line describing a snapshot: backend, monitors, union, colour-pick probe.
pub fn snapshot_line(snap: &ScreenSnapshot) -> String {
    let centre = PhysPoint::new(
        snap.virtual_bounds.x + snap.virtual_bounds.w as i32 / 2,
        snap.virtual_bounds.y + snap.virtual_bounds.h as i32 / 2,
    );
    format!(
        "backend={} monitors={} virtual={}x{}@{},{} pixel@centre={:?}",
        snap.backend,
        snap.monitors.len(),
        snap.virtual_bounds.w,
        snap.virtual_bounds.h,
        snap.virtual_bounds.x,
        snap.virtual_bounds.y,
        snap.color_at(centre),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
