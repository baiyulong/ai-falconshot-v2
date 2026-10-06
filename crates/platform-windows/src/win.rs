//! The Windows side of [`falcon_core::capture::WindowExtras`].
//!
//! `xcap` enumerates windows and grabs pixels, but it does not know the DWM's idea
//! of a window's visible rectangle, cannot name the process behind a handle on
//! every path, and has no UI Automation client. Those four gaps are this trait, and
//! this module is where the two that decide *what a user sees* are filled in.

use falcon_core::capture::WindowExtras;
use falcon_core::geometry::{PhysPoint, PhysRect};

pub struct WinExtras;

#[cfg(windows)]
impl WindowExtras for WinExtras {
    /// `DWMWA_EXTENDED_FRAME_BOUNDS`, in physical pixels.
    ///
    /// `GetWindowRect` - what `xcap` reports - includes the invisible resize border
    /// the DWM draws outside the content, so a window shot cropped from it carries a
    /// frame of shadow and desktop that the user never saw as part of the window.
    fn dwm_bounds(&self, hwnd: u32) -> Option<PhysRect> {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};

        let mut rect = RECT::default();
        // SAFETY: the out-parameter and its size are passed together, and the
        // handle names a window from this same enumeration.
        let ok = unsafe {
            DwmGetWindowAttribute(
                crate::proc::to_hwnd(u64::from(hwnd)),
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut RECT as *mut core::ffi::c_void,
                std::mem::size_of::<RECT>() as u32,
            )
        };
        if ok.is_err() {
            return None;
        }
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some(PhysRect::new(rect.left, rect.top, w as u32, h as u32))
    }

    /// PRD §5.3.4 - the wheel cycling through the element chain under the cursor.
    ///
    /// Not answered yet, and deliberately not guessed at: a wrong rect here would
    /// silently select the wrong region, which is worse than no element snapping.
    /// The UIA client (`IUIAutomation::ElementFromPoint`, then walking parents) is
    /// the work item that fills this in.
    fn element_chain(&self, _: PhysPoint) -> Vec<PhysRect> {
        Vec::new()
    }

    /// The system cursor as a small frame, for "capture with cursor".
    ///
    /// Empty for the same reason as above: drawing nothing is honest, drawing the
    /// wrong arrow is not.
    fn cursor_image(&self, _: PhysPoint) -> Option<(falcon_core::frame::Frame, PhysPoint)> {
        None
    }

    /// `(executable name, full path)` of the process owning `hwnd`.
    fn process_path(&self, hwnd: u32) -> Option<(String, String)> {
        crate::proc::process_paths(crate::proc::pid_of_window(u64::from(hwnd))?)
    }
}

#[cfg(not(windows))]
impl WindowExtras for WinExtras {
    fn dwm_bounds(&self, _: u32) -> Option<PhysRect> {
        None
    }

    fn element_chain(&self, _: PhysPoint) -> Vec<PhysRect> {
        Vec::new()
    }

    fn cursor_image(&self, _: PhysPoint) -> Option<(falcon_core::frame::Frame, PhysPoint)> {
        None
    }

    fn process_path(&self, _: u32) -> Option<(String, String)> {
        None
    }
}
