//! Which window station and desktop this process is actually on.
//!
//! This exists because a headless run is not the same as a working one: a process
//! launched from a service, a scheduled task or a remote-session agent lands on a
//! non-interactive station, where creating a window still "succeeds" - and the
//! clipboard then refuses every call with `Access is denied.`. Asking the two
//! names is how that gets told apart from a bug in this program (PRD §7.2).

#[cfg(windows)]
use windows::Win32::{
    Foundation::HANDLE,
    System::StationsAndDesktops::{
        GetProcessWindowStation, GetThreadDesktop, GetUserObjectInformationW, UOI_NAME,
    },
    System::Threading::GetCurrentThreadId,
};

/// `WinSta0\Default` reported as one line, or the names that were found instead.
#[cfg(windows)]
pub fn station_summary() -> String {
    // SAFETY: both handles come from the APIs themselves and are only read.
    unsafe {
        let station = object_name(GetProcessWindowStation().ok().map(|h| h.0));
        let desktop = object_name(GetThreadDesktop(GetCurrentThreadId()).ok().map(|h| h.0));
        format!("station={station} desktop={desktop}")
    }
}

/// The name of a window station or a desktop object: both are user objects with
/// the same information layout.
#[cfg(windows)]
fn object_name(raw: Option<*mut core::ffi::c_void>) -> String {
    let Some(ptr) = raw else {
        return "unknown".to_string();
    };
    let mut buf = [0u16; 64];
    let mut needed = 0u32;
    // SAFETY: `ptr` is a live handle from the call above, the buffer travels with
    // its own length, and `needed` is a u32 we own.
    let ok = unsafe {
        GetUserObjectInformationW(
            HANDLE(ptr),
            UOI_NAME,
            Some(buf.as_mut_ptr().cast()),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        )
    };
    if ok.is_err() {
        return "unknown".to_string();
    }
    // The API counts the terminating null in the length it reports.
    let chars = (needed as usize / 2).saturating_sub(1).min(buf.len());
    String::from_utf16_lossy(&buf[..chars])
}

/// The MVP is Windows-only; the rest of the workspace still has to compile here.
#[cfg(not(windows))]
pub fn station_summary() -> String {
    "station=n/a desktop=n/a".to_string()
}
