//! P5 dependency-capability audit (M0 spike, not production code).
//!
//! Answers, per line item of the plan's P5 row:
//!   - xcap physical-resolution capture (geometry source + captured pixels)
//!   - xcap GDI vs WGC capture path (build --no-default-features for GDI)
//!   - TGA / ICO / TIFF / GIF encode+decode in the `image` crate
//!   - DwmGetWindowAttribute rounded corners (what is actually queryable)
//!   - RealWindowFromPoint hit-testing through a transparent topmost window
//!
//! Every line is prefixed `P5|section|key=value` so the log is greppable.

use std::ffi::c_void;
use std::io::Cursor;
use std::time::{Duration, Instant};

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use windows::core::{s, w, PCSTR, PCWSTR};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWM_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND, DWMWCP_ROUNDSMALL,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, GetProcessDpiAwareness, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowRect, GWL_EXSTYLE, LWA_ALPHA, RegisterClassW, SetLayeredWindowAttributes,
    SetWindowLongPtrW, ShowWindow, SM_CXSCREEN, SM_CYSCREEN, SW_SHOWNA, WNDCLASSW, WindowFromPoint,
    WS_EX_LAYERED, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
};

const TAG: &str = "P5";

fn kv(section: &str, key: &str, value: impl std::fmt::Display) {
    println!("{TAG}|{section}|{key}={value}");
}

fn named(section: &str, name: &str, key: &str, value: impl std::fmt::Display) {
    println!("{TAG}|{section}|{name}.{key}={value}");
}

fn ms(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64() * 1000.0)
}

/// `HWND` as xcap reports it (`id() -> u32`) back into a usable handle.
fn hwnd_of(id: u32) -> HWND {
    HWND(id as *mut c_void)
}

// ---------------------------------------------------------------- environment

/// Non-spoofed OS build: `GetVersionEx` is manifest-faked, so go straight to ntdll.
fn os_build() -> String {
    unsafe {
        let Ok(ntdll) = LoadLibraryW(w!("ntdll.dll")) else {
            return "unknown".into();
        };
        let Some(raw) = GetProcAddress(ntdll, s!("RtlGetVersion")) else {
            return "unknown".into();
        };
        let f: extern "system" fn(*mut OSVERSIONINFOW) -> i32 = std::mem::transmute(raw);
        let mut v = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        if f(&mut v) != 0 {
            return "unknown".into();
        }
        format!("{}.{}.{}", v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber)
    }
}

fn audit_env() {
    kv("env", "capture_backend", if cfg!(feature = "wgc") { "wgc" } else { "gdi" });
    kv("env", "os_build", os_build());
    kv("env", "dpi_aware_before", dpi_awareness());
    // Best effort: fails with E_ACCESSDENIED when a manifest already fixed awareness.
    let set = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    kv("env", "set_pm_v2", format!("{set:?}"));
    kv("env", "dpi_aware_after", dpi_awareness());
    let dpi = unsafe { GetDpiForSystem() };
    kv("env", "system_dpi", dpi);
    kv("env", "system_scale", format!("{:.2}", dpi as f32 / 96.0));
    let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    kv("env", "sm_cxscreen", format!("{cx}x{cy}"));
}

/// 0=unaware 1=system 2=per-monitor; -1 when the query itself failed.
fn dpi_awareness() -> String {
    match unsafe { GetProcessDpiAwareness(None) } {
        Ok(v) => format!("{}({v:?})", v.0),
        Err(e) => format!("err:{e}"),
    }
}

// -------------------------------------------------------------------- monitors

fn stat(img: &RgbaImage, stride: usize) -> (u64, u64, u64, u64) {
    let (mut nonblack, mut black, mut opaque, mut total) = (0u64, 0u64, 0u64, 0u64);
    for (i, p) in img.pixels().enumerate() {
        if i % stride != 0 {
            continue;
        }
        total += 1;
        let [r, g, b, a] = p.0;
        if r == 0 && g == 0 && b == 0 {
            black += 1;
        } else {
            nonblack += 1;
        }
        if a == 255 {
            opaque += 1;
        }
    }
    (nonblack, black, opaque, total)
}

/// 12x12 block stats at the four corners. For a DWM-rounded window the outside-corner
/// pixels are the tell: transparent (alpha<255), opaque black, or real content.
fn corners(img: &RgbaImage) -> String {
    let n = 12u32;
    let (w, h) = (img.width(), img.height());
    if w < 2 * n || h < 2 * n {
        return "too_small".into();
    }
    let spots = [
        ("tl", 0u32, 0u32),
        ("tr", w - n, 0),
        ("bl", 0, h - n),
        ("br", w - n, h - n),
    ];
    let mut out = String::new();
    for (tag, x0, y0) in spots {
        let (mut opaque, mut black, mut content, mut total) = (0u32, 0u32, 0u32, 0u32);
        for y in y0..y0 + n {
            for x in x0..x0 + n {
                let p = img.get_pixel(x, y).0;
                total += 1;
                if p[3] == 255 {
                    opaque += 1;
                }
                if p[0] == 0 && p[1] == 0 && p[2] == 0 {
                    black += 1;
                } else {
                    content += 1;
                }
            }
        }
        out.push_str(&format!("{tag}:op{opaque}/bl{black}/ct{content}/{total} "));
    }
    out.trim_end().to_string()
}

fn audit_monitors() {
    let t0 = Instant::now();
    let monitors = match xcap::Monitor::all() {
        Ok(m) => m,
        Err(e) => {
            kv("monitor", "error", e);
            return;
        }
    };
    kv("monitor", "count", monitors.len());
    kv("monitor", "enumerate_ms", ms(t0.elapsed()));

    for (idx, m) in monitors.iter().enumerate() {
        let id = format!("M{idx}");
        named("monitor", &id, "device", m.name().unwrap_or_default());
        named("monitor", &id, "friendly", m.friendly_name().unwrap_or_default());
        named(
            "monitor",
            &id,
            "geom",
            format!(
                "x={} y={} w={} h={}",
                m.x().unwrap_or(-1),
                m.y().unwrap_or(-1),
                m.width().unwrap_or(0),
                m.height().unwrap_or(0)
            ),
        );
        named("monitor", &id, "primary", m.is_primary().unwrap_or(false));
        named("monitor", &id, "builtin", m.is_builtin().unwrap_or(false));
        named("monitor", &id, "rotation", m.rotation().unwrap_or(-1.0));
        named("monitor", &id, "frequency", m.frequency().unwrap_or(-1.0));
        match m.scale_factor() {
            Ok(v) => named("monitor", &id, "scale_factor", format!("{v:.3}")),
            Err(e) => named("monitor", &id, "scale_factor", format!("err:{e}")),
        }

        let t_first = Instant::now();
        match m.capture_image() {
            Ok(img) => {
                let first = ms(t_first.elapsed());
                let (nb, blk, opq, tot) = stat(&img, 997);
                named(
                    "monitor",
                    &id,
                    "capture",
                    format!("{}x{} first={first}ms", img.width(), img.height()),
                );
                named(
                    "monitor",
                    &id,
                    "matches_geom",
                    img.width() == m.width().unwrap_or(0) && img.height() == m.height().unwrap_or(0),
                );
                named(
                    "monitor",
                    &id,
                    "px",
                    format!("nonblack={nb}/{} black={blk} opaque={opq}/{tot}", tot)
                );
                let mut times = Vec::new();
                for _ in 0..4 {
                    let t = Instant::now();
                    let _ = m.capture_image();
                    times.push(t.elapsed());
                }
                times.sort_by(|a, b| a.cmp(b));
                named(
                    "monitor",
                    &id,
                    "capture_x4",
                    format!("min={} med={} max={}", ms(times[0]), ms(times[1]), ms(times[3]))
                );
                let rw = m.width().unwrap_or(512).min(512);
                let rh = m.height().unwrap_or(512).min(512);
                let t = Instant::now();
                match m.capture_region(0, 0, rw, rh) {
                    Ok(r) => named(
                        "monitor",
                        &id,
                        "region_512",
                        format!("{}x{} {}ms", r.width(), r.height(), ms(t.elapsed()))
                    ),
                    Err(e) => named("monitor", &id, "region_512", format!("err:{e}")),
                }
            }
            Err(e) => named("monitor", &id, "capture", format!("err:{e}")),
        }
    }
}

// -------------------------------------------------------------------- windows

fn audit_windows() {
    let t0 = Instant::now();
    let winlist = match xcap::Window::all() {
        Ok(w) => w,
        Err(e) => {
            kv("window", "error", e);
            return;
        }
    };
    kv("window", "count", winlist.len());
    kv("window", "enumerate_ms", ms(t0.elapsed()));

    let t = Instant::now();
    for w in winlist.iter().take(3) {
        let _ = w.z();
    }
    kv("window", "z_per_window_ms", format!("{:.2}", t.elapsed().as_secs_f64() * 1000.0 / 3.0f64));

    let mut probed = 0usize;
    for w in winlist.iter() {
        let title = w.title().unwrap_or_default();
        if title.is_empty() || w.is_minimized().unwrap_or(true) {
            continue;
        }
        probed += 1;
        if probed > 6 {
            break;
        }
        let id = format!("W{probed}");
        named("window", &id, "title", title.replace('\n', " "));
        named(
            "window",
            &id,
            "geom",
            format!(
                "x={} y={} w={} h={}",
                w.x().unwrap_or(0),
                w.y().unwrap_or(0),
                w.width().unwrap_or(0),
                w.height().unwrap_or(0)
            ),
        );
        let id32 = w.id().unwrap_or(0);
        named("window", &id, "hwnd_from_id", format!("{id32:#08x}"));

        let hwnd = hwnd_of(id32);
        let mut rect = RECT::default();
        let gwr = unsafe { GetWindowRect(hwnd, &mut rect) };
        named(
            "window",
            &id,
            "getwindowrect",
            format!(
                "{}x{}@{},{} ok={}",
                rect.right - rect.left,
                rect.bottom - rect.top,
                rect.left,
                rect.top,
                gwr.is_ok()
            ),
        );

        let t = Instant::now();
        match w.capture_image() {
            Ok(img) => {
                let (nb, blk, opq, tot) = stat(&img, 499);
                named(
                    "window",
                    &id,
                    "capture",
                    format!("{}x{} {}ms", img.width(), img.height(), ms(t.elapsed()))
                );
                named(
                    "window",
                    &id,
                    "px",
                    format!("nonblack={nb}/{tot} black={blk} opaque={opq}/{tot}")
                );
                named("window", &id, "corners", corners(&img));
            }
            Err(e) => named("window", &id, "capture", format!("err:{e}")),
        }
    }
}

// ---------------------------------------------------------------- DWM corners

fn corner_name(p: DWM_WINDOW_CORNER_PREFERENCE) -> &'static str {
    if p == DWMWCP_DONOTROUND {
        "DontRound"
    } else if p == DWMWCP_ROUND {
        "Round"
    } else if p == DWMWCP_ROUNDSMALL {
        "RoundSmall"
    } else {
        "Default"
    }
}

fn audit_dwm() {
    let winlist = xcap::Window::all().unwrap_or_default();
    let mut probed = 0usize;
    let mut ok = 0usize;
    for w in winlist.iter() {
        let title = w.title().unwrap_or_default();
        if title.is_empty() || w.is_minimized().unwrap_or(true) {
            continue;
        }
        probed += 1;
        if probed > 10 {
            break;
        }
        let hwnd = hwnd_of(w.id().unwrap_or(0));
        let mut pref = DWM_WINDOW_CORNER_PREFERENCE(0);
        let res = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &mut pref as *mut _ as *mut c_void,
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            )
        };
        if res.is_ok() {
            ok += 1;
        }
        let mut bounds = RECT::default();
        let bres = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut bounds as *mut _ as *mut c_void,
                std::mem::size_of::<RECT>() as u32,
            )
        };
        named(
            "dwm",
            &format!("W{probed}"),
            "attr",
            format!(
                "corner={} corner_ok={} radius_exposed=no | extbounds_ok={} {}x{}",
                corner_name(pref),
                res.is_ok(),
                bres.is_ok(),
                bounds.right - bounds.left,
                bounds.bottom - bounds.top
            ),
        );
    }
    kv("dwm", "corner_attr_readable", format!("{ok}/{probed}"));
    kv(
        "dwm",
        "corner_radius_api",
        "none: DWMWA_WINDOW_CORNER_PREFERENCE is a 4-value enum (Default/DontRound/Round/RoundSmall), no pixel radius is exposed"
    );
}

// ------------------------------------------------------------------ hit testing

unsafe extern "system" fn audit_wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(h, msg, wp, lp)
}

fn ascii(name: &str) -> Vec<u8> {
    let mut v = name.as_bytes().to_vec();
    v.push(0);
    v
}

fn audit_hittest() {
    unsafe {
        if let Ok(user32) = LoadLibraryW(w!("user32.dll")) {
            for name in [
                "RealWindowFromPoint",
                "RealChildWindowFromPoint",
                "WindowFromPoint",
                "GetAncestor",
            ] {
                let bytes = ascii(name);
                let p = GetProcAddress(user32, PCSTR(bytes.as_ptr()));
                named("hittest", "export", name, if p.is_some() { "present" } else { "absent" });
            }
        }
    }
    kv(
        "hittest",
        "windows_crate_binding",
        "windows 0.62.2 does not export RealWindowFromPoint (undocumented, arity unverifiable)"
    );

    // Behavioural test: does WindowFromPoint skip a topmost translucent+transparent overlay?
    let class: PCWSTR = w!("P5AuditOverlay");
    let registered = unsafe {
        let hmodule = GetModuleHandleW(None).map_err(|e| e.to_string()).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(audit_wndproc),
            hInstance: HINSTANCE(hmodule.0),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc)
    };
    if registered == 0 {
        kv("hittest", "register_class", "failed");
        return;
    }

    let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let (ox, oy, ow, oh) = (cx / 2 - 200, cy / 2 - 150, 400i32, 300i32);
    let center = POINT {
        x: ox + ow / 2,
        y: oy + oh / 2,
    };

    let created = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST,
            class,
            w!("p5 overlay"),
            WS_POPUP | WS_VISIBLE,
            ox,
            oy,
            ow,
            oh,
            None,
            None,
            None,
            None,
        )
    };
    let hwnd = match created {
        Ok(h) => h,
        Err(e) => {
            kv("hittest", "create_window", format!("err:{e}"));
            return;
        }
    };
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 120, LWA_ALPHA);
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
    std::thread::sleep(Duration::from_millis(150));

    let probe = |label: &str| {
        let got = unsafe { WindowFromPoint(center) };
        let mut r = RECT::default();
        let _ = unsafe { GetWindowRect(got, &mut r) };
        kv(
            "hittest",
            label,
            format!(
                "hit_ours={} hwnd={:?} rect={}x{}@{},{}",
                got == hwnd,
                got,
                r.right - r.left,
                r.bottom - r.top,
                r.left,
                r.top
            ),
        );
    };

    probe("windowfrompoint_no_transparent_ex");
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_TRANSPARENT.0 as isize);
    }
    std::thread::sleep(Duration::from_millis(80));
    probe("windowfrompoint_with_transparent_ex");

    // The documented fallback if transparent-skip is not to be trusted: walk the z-order
    // by hand and test DWM-bounds containment.
    let t = Instant::now();
    let mut under: Option<(String, usize)> = None;
    let mut scanned = 0usize;
    for (z, w) in xcap::Window::all().unwrap_or_default().iter().enumerate() {
        scanned = z + 1;
        let title = w.title().unwrap_or_default();
        if title.is_empty() || w.is_minimized().unwrap_or(true) {
            continue;
        }
        let (x, y) = (w.x().unwrap_or(0), w.y().unwrap_or(0));
        let (ww, hh) = (w.width().unwrap_or(0) as i32, w.height().unwrap_or(0) as i32);
        if center.x >= x && center.x < x + ww && center.y >= y && center.y < y + hh {
            under = Some((title, z));
            break;
        }
    }
    kv(
        "hittest",
        "zorder_walk",
        format!(
            "{} ms after {scanned} windows -> {under:?}",
            ms(t.elapsed())
        ),
    );

    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

// --------------------------------------------------------------------- codecs

fn swatch(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        Rgba([
            (x * 7 % 256) as u8,
            (y * 5 % 256) as u8,
            ((x + y) * 3 % 256) as u8,
            if (x + y) % 3 == 0 { 90 } else { 255 },
        ])
    })
}

fn audit_codecs() {
    let fmts: [(ImageFormat, &str); 14] = [
        (ImageFormat::Png, "png"),
        (ImageFormat::Jpeg, "jpeg"),
        (ImageFormat::Bmp, "bmp"),
        (ImageFormat::Tga, "tga"),
        (ImageFormat::Ico, "ico"),
        (ImageFormat::Tiff, "tiff"),
        (ImageFormat::Gif, "gif"),
        (ImageFormat::WebP, "webp"),
        (ImageFormat::Qoi, "qoi"),
        (ImageFormat::Pnm, "pnm"),
        (ImageFormat::Dds, "dds"),
        (ImageFormat::Hdr, "hdr"),
        (ImageFormat::OpenExr, "exr"),
        (ImageFormat::Farbfeld, "ffdf"),
    ];

    for (w, h) in [(64u32, 64u32), (300u32, 200u32), (256u32, 256u32)] {
        let img = swatch(w, h);
        let id = format!("{w}x{h}");
        for (f, name) in fmts {
            let mut buf = Cursor::new(Vec::new());
            let enc = DynamicImage::ImageRgba8(img.clone()).write_to(&mut buf, f);
            match enc {
                Ok(()) => {
                    let bytes = buf.into_inner();
                    let dec = match image::load_from_memory_with_format(&bytes, f) {
                        Ok(d) => format!("{}x{}", d.width(), d.height()),
                        Err(e) => format!("decode_err:{e}"),
                    };
                    named("codec", &id, name, format!("enc={}B decode={}", bytes.len(), dec));
                }
                Err(e) => named("codec", &id, name, format!("enc_err:{e}")),
            }
        }
    }
}

fn main() {
    println!("{TAG}|begin|");
    audit_env();
    audit_monitors();
    audit_windows();
    audit_dwm();
    audit_hittest();
    audit_codecs();
    println!("{TAG}|end|");
}
