//! Window and process identity, plus the cursor.
//!
//! Two layers ask the same two questions - "whose window is this" (the clipboard's
//! §7.2 holder report and PRD §5.16.2's hotkey exclusion rules) and "where is the
//! pointer" (PRD §5.8.2's "the pin appears where the user is looking") - so they
//! live here once rather than twice.
//!
//! Handles travel as `u64`: core's model stores a window handle as `u32`, a
//! user-mode handle fits in 32 bits on Win64, and `u64` is wide enough for the
//! pointer-sized value the Win32 call actually wants.

use falcon_core::geometry::PhysPoint;

/// The process owning `hwnd`, or `None` when the handle no longer names a window.
pub fn pid_of_window(hwnd: u64) -> Option<u32> {
    #[cfg(windows)]
    {
        let mut pid = 0u32;
        // SAFETY: `hwnd` came from an enumeration of live windows and the
        // out-parameter is a plain u32 this call owns.
        let tid = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                to_hwnd(hwnd),
                Some(&mut pid),
            )
        };
        (tid != 0).then_some(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        None
    }
}

/// The window's caption, or `None` when it has none.
pub fn window_title(hwnd: u64) -> Option<String> {
    #[cfg(windows)]
    {
        let mut buf = [0u16; 256];
        // SAFETY: a fixed buffer whose capacity is handed to the API with it.
        let n = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(to_hwnd(hwnd), &mut buf)
        };
        if n <= 0 {
            return None;
        }
        let title = wide_to_string(&buf[..n as usize]);
        let title = title.trim();
        (!title.is_empty()).then(|| title.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        None
    }
}

/// `(executable name, full path)` for `pid`. The name is what a message shows;
/// the path is what exclusion rules match on (PRD §5.16.2), because two builds of
/// one app share a name and a folder does not.
///
/// A protected process refuses both the handle and the name, which is why this
/// answers `None` instead of failing: "no path" must not read as "no window".
pub fn process_paths(pid: u32) -> Option<(String, String)> {
    #[cfg(windows)]
    {
        use windows::core::PWSTR;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // SAFETY: query-limited access asks for the least permission available;
        // the buffer and its capacity are passed together and the handle is closed.
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return None;
            };
            // Longer than `MAX_PATH`: a process under a deep path still has to be
            // nameable, and `QueryFullProcessImageNameW` fails outright when the
            // buffer is too small rather than truncating.
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let named = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(handle);
            if named.is_err() {
                return None;
            }
            let full = wide_to_string(&buf[..len.min(buf.len() as u32) as usize]);
            let name = std::path::Path::new(&full)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())?;
            Some((name, full))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

/// The executable behind a pid, basename only - the full path says nothing a user
/// can act on and names a directory they may not want named.
pub fn process_name(pid: u32) -> Option<String> {
    process_paths(pid).map(|(name, _)| name)
}

/// The pointer, in physical pixels on the virtual desktop.
pub fn cursor_pos() -> Option<PhysPoint> {
    #[cfg(windows)]
    {
        let mut pt = windows::Win32::Foundation::POINT::default();
        // SAFETY: the out-parameter is a value this call owns.
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt).ok()? };
        Some(PhysPoint::new(pt.x, pt.y))
    }
    #[cfg(not(windows))]
    None
}

/// A handle value back into the pointer-sized shape Win32 expects.
#[cfg(windows)]
pub(crate) fn to_hwnd(hwnd: u64) -> windows::Win32::Foundation::HWND {
    windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)
}

/// UTF-16 up to the first NUL, which is what every one of these APIs returns.
pub fn wide_to_string(units: &[u16]) -> String {
    let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}
