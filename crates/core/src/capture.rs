//! Screen/window capture and the frozen-frame model.
//!
//! The mask never enters a screenshot because the whole screen is captured
//! **before** any mask window exists, and every later crop reads from that
//! snapshot (plan §4-M1). Colour picking and the magnifier read the same
//! snapshot, which is why they are correct under mixed DPI and under the
//! dimmed overlay.

use crate::frame::{Frame, FrameError};
use crate::geometry::{PhysPoint, PhysRect, Scale};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture backend failed: {0}")]
    Backend(String),
    #[error("no monitor matches {0:?}")]
    UnknownMonitor(String),
    #[error("region {0:?} is outside every monitor")]
    OutsideAllMonitors(PhysRect),
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("nothing to capture")]
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub id: String,
    pub name: String,
    pub friendly: String,
    /// Physical pixels, in virtual-desktop coordinates.
    pub bounds: PhysRect,
    pub scale: Scale,
    pub primary: bool,
    pub builtin: bool,
}

impl MonitorInfo {
    pub fn contains(&self, p: PhysPoint) -> bool {
        self.bounds.contains(p)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub hwnd: u32,
    pub title: String,
    pub bounds: PhysRect,
    /// `DWMWA_EXTENDED_FRAME_BOUNDS`, when the platform could read it. This is
    /// the visually correct crop for a window shot (it drops the invisible
    ///收缩边 that `GetWindowRect` includes).
    pub dwm_bounds: Option<PhysRect>,
    pub app_name: String,
    /// Full executable path, filled by the platform layer when the backend
    /// cannot name it. Hotkey exclusion rules match on this (PRD §5.16.2).
    pub process_path: String,
    pub pid: u32,
    pub minimized: bool,
    pub focused: bool,
    pub z_order: usize,
    pub own_process: bool,
}

impl WindowInfo {
    /// What a window screenshot should use.
    pub fn visible_bounds(&self) -> PhysRect {
        self.dwm_bounds.unwrap_or(self.bounds)
    }
}

/// Implemented by the platform layer for things xcap does not cover: the DWM
/// bounds of one window and the UI-element hit test (PRD §5.3.3/§5.3.4).
pub trait WindowExtras: Send + Sync {
    fn dwm_bounds(&self, hwnd: u32) -> Option<PhysRect>;
    /// UIA element bounds under the cursor, innermost first (wheel cycles).
    fn element_chain(&self, p: PhysPoint) -> Vec<PhysRect>;
    fn cursor_image(&self, p: PhysPoint) -> Option<(Frame, PhysPoint)>;
    /// (executable name, full path) of the process owning `hwnd`.
    fn process_path(&self, hwnd: u32) -> Option<(String, String)>;
}

pub struct NoopExtras;

impl WindowExtras for NoopExtras {
    fn dwm_bounds(&self, _: u32) -> Option<PhysRect> {
        None
    }
    fn element_chain(&self, _: PhysPoint) -> Vec<PhysRect> {
        Vec::new()
    }
    fn cursor_image(&self, _: PhysPoint) -> Option<(Frame, PhysPoint)> {
        None
    }
    fn process_path(&self, _: u32) -> Option<(String, String)> {
        None
    }
}

/// Where pixels come from. `xcap`'s GDI path is the default; plan P5 measured
/// WGC as slower on every case that matters here.
pub trait FrameSource: Send + Sync {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;
    fn capture_monitor(&self, id: &str) -> Result<Frame, CaptureError>;
    fn capture_monitor_region(&self, id: &str, rel: &PhysRect) -> Result<Frame, CaptureError> {
        let _ = (id, rel);
        Err(CaptureError::Backend("region capture unsupported".into()))
    }
    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError>;
    fn capture_window(&self, hwnd: u32) -> Result<Frame, CaptureError>;
    fn name(&self) -> &'static str {
        "xcap-gdi"
    }
}

/// A monitor whose content is already decoded, so repeated selections never
/// re-read the screen.
#[derive(Clone, Debug)]
pub struct FrozenMonitor {
    pub info: MonitorInfo,
    pub frame: Frame,
}

#[derive(Clone, Debug, Default)]
pub struct ScreenSnapshot {
    pub monitors: Vec<FrozenMonitor>,
    pub virtual_bounds: PhysRect,
    /// Wall-clock ms since epoch, for history rows.
    pub taken_at_ms: i64,
    pub backend: &'static str,
}

#[derive(Clone, Debug)]
pub struct Captured {
    pub frame: Frame,
    /// Physical rect the result really covers after clipping to the desktop.
    pub rect: PhysRect,
    pub monitors: Vec<String>,
    /// Set when the requested rect did not fit and was clipped (PRD §5.2.5/§8.5).
    pub clipped: bool,
}

impl ScreenSnapshot {
    pub fn monitor_at(&self, p: PhysPoint) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|m| m.info.contains(p))
            .map(|m| &m.info)
    }

    /// Primary wins ties; used to pick which scale a mixed-DIP readout shows.
    pub fn monitor_for_rect(&self, r: &PhysRect) -> Option<&MonitorInfo> {
        let mut best: Option<(u64, &MonitorInfo)> = None;
        for m in &self.monitors {
            let Some(hit) = m.info.bounds.intersection(r) else {
                continue;
            };
            let a = hit.area();
            let better = match best {
                None => true,
                Some((ba, bi)) => a > ba || (a == ba && m.info.primary && !bi.primary),
            };
            if better {
                best = Some((a, &m.info));
            }
        }
        best.map(|(_, m)| m)
    }

    pub fn scale_at(&self, p: PhysPoint) -> Scale {
        self.monitor_at(p).map(|m| m.scale).unwrap_or(Scale::ONE)
    }

    /// Colour of the raw screen pixel (PRD §5.4.2: unaffected by the overlay).
    pub fn color_at(&self, p: PhysPoint) -> Option<[u8; 4]> {
        let m = self.monitors.iter().find(|m| m.info.contains(p))?;
        let x = (p.x - m.info.bounds.x) as u32;
        let y = (p.y - m.info.bounds.y) as u32;
        if x >= m.frame.width || y >= m.frame.height {
            return None;
        }
        Some(m.frame.get(x, y))
    }

    /// Magnifier content: a square neighbourhood around `p` scaled up with hard
    /// edges, in destination coordinates relative to `p`.
    /// `radius` is in *screen* pixels around `p`; `zoom` is the magnification
    /// factor. The output is therefore `(2*radius+1) * zoom` a side, so a hard
    /// edge landing between two source pixels stays a hard edge in the output
    /// (PRD §5.4.1 — the magnifier is used to align a selection to a pixel).
    pub fn magnifier(&self, p: PhysPoint, radius: u32, zoom: u32) -> Result<Frame, CaptureError> {
        let radius = radius.max(1);
        let zoom = zoom.clamp(1, 64);
        let src_side = radius * 2 + 1;
        let mut out = Frame::filled(src_side * zoom, src_side * zoom, [0, 0, 0, 255])?;
        for dy in 0..src_side {
            for dx in 0..src_side {
                let sp = PhysPoint::new(
                    p.x - radius as i32 + dx as i32,
                    p.y - radius as i32 + dy as i32,
                );
                if let Some(c) = self.color_at(sp) {
                    out.fill_rect(
                        &PhysRect::new((dx * zoom) as i32, (dy * zoom) as i32, zoom, zoom),
                        c,
                    );
                }
            }
        }
        Ok(out)
    }

    /// Cross-monitor capture of an arbitrary rect: one output image, stitched
    /// in physical space (PRD §5.3.10 / §4.3).
    pub fn capture(&self, want: &PhysRect) -> Result<Captured, CaptureError> {
        if want.is_empty() {
            return Err(CaptureError::Empty);
        }
        let (rect, clipped) = match want.clamp_to(&self.virtual_bounds) {
            Some(v) => v,
            None => return Err(CaptureError::OutsideAllMonitors(*want)),
        };
        let mut out = Frame::new(rect.w, rect.h)?;
        // Areas covered by no monitor at all stay transparent; PRD §5.3.10
        // forbids stretches, not gaps, but we report them.
        let mut used = Vec::new();
        for m in &self.monitors {
            let Some(hit) = rect.intersection(&m.info.bounds) else {
                continue;
            };
            let src = PhysRect::new(
                hit.x - m.info.bounds.x,
                hit.y - m.info.bounds.y,
                hit.w,
                hit.h,
            );
            let piece = m.frame.crop(&src)?;
            out.paste(&piece, PhysPoint::new(hit.x - rect.x, hit.y - rect.y));
            used.push(m.info.id.clone());
        }
        if used.is_empty() {
            return Err(CaptureError::OutsideAllMonitors(*want));
        }
        Ok(Captured {
            frame: out,
            rect,
            monitors: used,
            clipped,
        })
    }
}

pub struct CaptureService<E: WindowExtras + ?Sized> {
    source: Box<dyn FrameSource>,
    pub extras: Box<E>,
    pub self_pid: u32,
}

impl CaptureService<NoopExtras> {
    pub fn new(source: Box<dyn FrameSource>) -> Self {
        Self {
            source,
            extras: Box::new(NoopExtras),
            self_pid: std::process::id(),
        }
    }
}

impl<E: WindowExtras + ?Sized> CaptureService<E> {
    pub fn with_extras(source: Box<dyn FrameSource>, extras: Box<E>) -> Self {
        Self {
            source,
            extras,
            self_pid: std::process::id(),
        }
    }

    pub fn source(&self) -> &dyn FrameSource {
        &*self.source
    }

    pub fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        let mut v = self.source.monitors()?;
        v.sort_by(|a, b| {
            (!a.primary)
                .cmp(&!b.primary)
                .then(a.bounds.x.cmp(&b.bounds.x))
                .then(a.bounds.y.cmp(&b.bounds.y))
        });
        Ok(v)
    }

    pub fn virtual_bounds(&self) -> Result<PhysRect, CaptureError> {
        let ms = self.monitors()?;
        ms.iter()
            .map(|m| m.bounds)
            .reduce(|a, b| a.union(&b))
            .ok_or(CaptureError::Empty)
    }

    /// Step 1 of every capture flow, run before the mask window is created.
    pub fn freeze(&self) -> Result<ScreenSnapshot, CaptureError> {
        let monitors = self.monitors()?;
        let mut frozen = Vec::with_capacity(monitors.len());
        let mut virtual_bounds = PhysRect::default();
        let mut backend = "xcap-gdi";
        for info in monitors {
            backend = self.source.name();
            let frame = match self.source.capture_monitor(&info.id) {
                Ok(f) => f,
                // A monitor that refuses to be captured must not blank the
                // whole snapshot (PRD §7.2).
                Err(e) => {
                    tracing::warn!(monitor = %info.id, error = %e, "monitor capture failed");
                    Frame::new(info.bounds.w, info.bounds.h)?
                }
            };
            virtual_bounds = if virtual_bounds.is_empty() {
                info.bounds
            } else {
                virtual_bounds.union(&info.bounds)
            };
            frozen.push(FrozenMonitor { info, frame });
        }
        if frozen.is_empty() {
            return Err(CaptureError::Empty);
        }
        Ok(ScreenSnapshot {
            monitors: frozen,
            virtual_bounds,
            taken_at_ms: now_ms(),
            backend,
        })
    }

    /// Freeze + crop in one call: used by repeat-last-region, delayed capture,
    /// the CLI and any path with no interactive selection.
    pub fn capture_rect(&self, want: &PhysRect) -> Result<Captured, CaptureError> {
        self.freeze()?.capture(want)
    }

    pub fn capture_window(&self, hwnd: u32) -> Result<Captured, CaptureError> {
        let frame = self.source.capture_window(hwnd)?;
        let info = self.windows(true)?.into_iter().find(|w| w.hwnd == hwnd);
        let want = info
            .as_ref()
            .map(|w| w.visible_bounds())
            .unwrap_or_else(|| frame.bounds());
        let monitors = self
            .monitors()?
            .iter()
            .filter(|m| m.bounds.intersection(&want).is_some())
            .map(|m| m.id.clone())
            .collect();
        // P5: `PrintWindow`-backed shots drift in size, so never assume the
        // decoded frame matches the geometry we asked for.
        let cropped = if frame.width == want.w && frame.height == want.h {
            frame
        } else {
            let keep = PhysRect::new(0, 0, want.w.min(frame.width), want.h.min(frame.height));
            frame.crop(&keep)?
        };
        Ok(Captured {
            frame: cropped,
            rect: want,
            monitors,
            clipped: false,
        })
    }

    /// z-order top-down, with invisible / minimised / (optionally) our own
    /// windows removed, so hovering picks what the user sees (PRD §5.3.2).
    pub fn windows(&self, include_own_process: bool) -> Result<Vec<WindowInfo>, CaptureError> {
        let mut ws = self.source.windows()?;
        ws.retain(|w| !w.minimized && !w.bounds.is_empty());
        if !include_own_process {
            ws.retain(|w| !w.own_process);
        }
        ws.sort_by_key(|w| w.z_order);
        for w in &mut ws {
            if w.dwm_bounds.is_none() {
                w.dwm_bounds = self.extras.dwm_bounds(w.hwnd);
            }
            if w.process_path.is_empty() {
                if let Some((_, path)) = self.extras.process_path(w.hwnd) {
                    w.process_path = path;
                }
            }
        }
        Ok(ws)
    }

    /// The window the cursor points at most directly: topmost in z-order whose
    /// visible bounds contain the point.
    pub fn window_at(&self, p: PhysPoint) -> Option<WindowInfo> {
        let ws = self.windows(false).ok()?;
        // `xcap` enumerates top-down (measured on this desktop: the IME overlay
        // is index 0, the taskbar 1, the focused window 2), so the first hit is
        // the window the user actually sees (PRD §5.3.2).
        ws.iter().find(|w| w.visible_bounds().contains(p)).cloned()
    }

    /// PRD §5.3.4: the wheel cycles through the element chain under the cursor.
    pub fn element_chain(&self, p: PhysPoint) -> Vec<PhysRect> {
        self.extras.element_chain(p)
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

// ---------------------------------------------------------------- xcap backend

pub struct XcapCapture;

fn monitor_id(index: usize, name: &str) -> String {
    let slug: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    format!("m{index}-{slug}")
}

impl FrameSource for XcapCapture {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        let ms = xcap::Monitor::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        Ok(ms
            .iter()
            .enumerate()
            .map(|(i, m)| MonitorInfo {
                id: monitor_id(i, &m.name().unwrap_or_default()),
                name: m.name().unwrap_or_default(),
                friendly: m.friendly_name().unwrap_or_default(),
                bounds: PhysRect::new(
                    m.x().unwrap_or(0),
                    m.y().unwrap_or(0),
                    m.width().unwrap_or(0),
                    m.height().unwrap_or(0),
                ),
                scale: Scale::of(f64::from(m.scale_factor().unwrap_or(1.0))),
                primary: m.is_primary().unwrap_or(false),
                builtin: m.is_builtin().unwrap_or(false),
            })
            .collect())
    }

    fn capture_monitor(&self, id: &str) -> Result<Frame, CaptureError> {
        let m = self.monitor(id)?;
        let img = m
            .capture_image()
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        Ok(Frame::from_image(img))
    }

    fn capture_monitor_region(&self, id: &str, rel: &PhysRect) -> Result<Frame, CaptureError> {
        let m = self.monitor(id)?;
        let img = m
            .capture_region(rel.x.max(0) as u32, rel.y.max(0) as u32, rel.w, rel.h)
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        Ok(Frame::from_image(img))
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        let ws = xcap::Window::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        let me = std::process::id();
        Ok(ws
            .iter()
            .enumerate()
            .map(|(z, w)| {
                let pid = w.pid().unwrap_or(0);
                WindowInfo {
                    hwnd: w.id().unwrap_or(0),
                    title: w.title().unwrap_or_default(),
                    bounds: PhysRect::new(
                        w.x().unwrap_or(0),
                        w.y().unwrap_or(0),
                        w.width().unwrap_or(0),
                        w.height().unwrap_or(0),
                    ),
                    dwm_bounds: None,
                    app_name: w.app_name().unwrap_or_default(),
                    process_path: String::new(),
                    pid,
                    minimized: w.is_minimized().unwrap_or(false),
                    focused: w.is_focused().unwrap_or(false),
                    z_order: z,
                    own_process: pid == me,
                }
            })
            .collect())
    }

    fn capture_window(&self, hwnd: u32) -> Result<Frame, CaptureError> {
        let ws = xcap::Window::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        let w = ws
            .iter()
            .find(|w| w.id().unwrap_or(0) == hwnd)
            .ok_or(CaptureError::Backend(format!("no window {hwnd:#08x}")))?;
        let img = w
            .capture_image()
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        Ok(Frame::from_image(img))
    }

    fn name(&self) -> &'static str {
        if cfg!(feature = "wgc") {
            "xcap-wgc"
        } else {
            "xcap-gdi"
        }
    }
}

impl XcapCapture {
    fn monitor(&self, id: &str) -> Result<xcap::Monitor, CaptureError> {
        let ms = xcap::Monitor::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        ms.into_iter()
            .enumerate()
            .find(|(i, m)| monitor_id(*i, &m.name().unwrap_or_default()) == id)
            .map(|(_, m)| m)
            .ok_or_else(|| CaptureError::UnknownMonitor(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use crate::geometry::Scale;

    fn mon(id: &str, x: i32, w: u32, scale: f64) -> MonitorInfo {
        MonitorInfo {
            id: id.into(),
            name: id.into(),
            friendly: id.into(),
            bounds: PhysRect::new(x, 0, w, 100),
            scale: Scale::of(scale),
            primary: x == 0,
            builtin: false,
        }
    }

    fn snap_two() -> ScreenSnapshot {
        let mut left = Frame::new(200, 100).unwrap();
        left.fill_rect(&PhysRect::new(0, 0, 200, 100), [255, 0, 0, 255]);
        let right = Frame::filled(100, 100, [0, 0, 255, 255]).unwrap();
        ScreenSnapshot {
            monitors: vec![
                FrozenMonitor {
                    info: mon("a", 0, 200, 1.0),
                    frame: left,
                },
                FrozenMonitor {
                    info: mon("b", 200, 100, 2.0),
                    frame: right,
                },
            ],
            virtual_bounds: PhysRect::new(0, 0, 300, 100),
            taken_at_ms: 0,
            backend: "test",
        }
    }

    #[test]
    fn stitch_spans_two_monitors_without_stretch() {
        let s = snap_two();
        let c = s.capture(&PhysRect::new(180, 10, 40, 20)).unwrap();
        assert_eq!((c.frame.width, c.frame.height), (40, 20));
        assert_eq!(c.frame.get(0, 0), [255, 0, 0, 255]);
        assert_eq!(c.frame.get(20, 0), [0, 0, 255, 255]);
        assert_eq!(c.monitors, vec!["a".to_string(), "b".to_string()]);
        assert!(!c.clipped);
    }

    #[test]
    fn out_of_range_is_clipped_and_reported() {
        let s = snap_two();
        let c = s.capture(&PhysRect::new(290, 95, 50, 50)).unwrap();
        assert_eq!(c.rect, PhysRect::new(290, 95, 10, 5));
        assert!(c.clipped);
        assert!(s.capture(&PhysRect::new(1000, 0, 10, 10)).is_err());
        assert!(s.capture(&PhysRect::new(0, 0, 0, 10)).is_err());
    }

    #[test]
    fn colour_and_scale_follow_the_monitor() {
        let s = snap_two();
        assert_eq!(s.color_at(PhysPoint::new(250, 50)), Some([0, 0, 255, 255]));
        assert_eq!(s.scale_at(PhysPoint::new(250, 50)), Scale::of(2.0));
        assert_eq!(s.scale_at(PhysPoint::new(50, 50)), Scale::ONE);
        assert_eq!(s.color_at(PhysPoint::new(999, 0)), None);
        assert_eq!(
            s.monitor_for_rect(&PhysRect::new(190, 0, 40, 40))
                .unwrap()
                .id,
            "b"
        );
    }

    #[test]
    fn magnifier_zooms_with_hard_edges() {
        let s = snap_two();
        // Straddle the seam: monitor a (red) ends at x=200, b (blue) starts there.
        let f = s.magnifier(PhysPoint::new(200, 5), 2, 3).unwrap();
        assert_eq!((f.width, f.height), (15, 15));
        assert_eq!(f.get(0, 6), [255, 0, 0, 255]);
        assert_eq!(f.get(5, 6), [255, 0, 0, 255]);
        assert_eq!(f.get(6, 6), [0, 0, 255, 255]);
        assert_eq!(f.get(14, 6), [0, 0, 255, 255]);
        // Off-snapshot source pixels keep the frame filler instead of reading
        // another monitor's data.
        let edge = s.magnifier(PhysPoint::new(0, 0), 2, 2).unwrap();
        assert_eq!((edge.width, edge.height), (10, 10));
        assert_eq!(edge.get(0, 0), [0, 0, 0, 255]);
        assert_eq!(edge.get(2, 2), [0, 0, 0, 255]);
        assert_eq!(edge.get(4, 4), [255, 0, 0, 255]);
        assert_eq!(edge.get(9, 9), [255, 0, 0, 255]);
    }

    /// Real hardware, so the assumptions above get checked against a desktop
    /// instead of a fixture. Run with `cargo test -p falcon-core -- --ignored`.
    #[test]
    #[ignore]
    fn freezes_the_actual_desktop() {
        use crate::capture::{CaptureService, XcapCapture};
        use std::time::Instant;

        let svc = CaptureService::new(Box::new(XcapCapture));
        let ms = svc.monitors().unwrap();
        println!("backend = {}", svc.source.name());
        for m in &ms {
            println!(
                "monitor {} {:?} bounds={} scale={:?} primary={} builtin={}",
                m.id, m.name, m.bounds, m.scale, m.primary, m.builtin
            );
        }
        assert!(!ms.is_empty(), "no monitors enumerated");

        let t = Instant::now();
        let snap = svc.freeze().unwrap();
        let freeze_ms = t.elapsed().as_millis();
        println!(
            "freeze: {} ms, virtual {:?}, {} frame(s), backend {}",
            freeze_ms,
            snap.virtual_bounds,
            snap.monitors.len(),
            snap.backend
        );
        for fm in &snap.monitors {
            assert_eq!(fm.frame.width, fm.info.bounds.w, "{} width", fm.info.id);
            assert_eq!(fm.frame.height, fm.info.bounds.h, "{} height", fm.info.id);
        }

        let t = Instant::now();
        let want = PhysRect::new(
            snap.virtual_bounds.x,
            snap.virtual_bounds.y,
            512.min(snap.virtual_bounds.w),
            512.min(snap.virtual_bounds.h),
        );
        let c = snap.capture(&want).unwrap();
        println!("crop {want:?}: {} ms", t.elapsed().as_millis());
        assert_eq!(c.frame.width, want.w);
        assert_eq!(c.frame.height, want.h);

        let t = Instant::now();
        let ws = svc.windows(true).unwrap();
        let enum_ms = t.elapsed().as_millis();
        println!("windows: {} in {} ms", ws.len(), enum_ms);
        for w in ws.iter().take(12) {
            println!(
                "  z={:<2} hwnd={:#08x} focus={} bounds={:?} vis={:?} {:?}",
                w.z_order,
                w.hwnd,
                w.focused,
                w.bounds,
                w.visible_bounds(),
                w.app_name
            );
        }
        let focused = ws.iter().position(|w| w.focused);
        println!("focused window sits at enumeration index {focused:?}");

        let probe = PhysPoint::new(snap.virtual_bounds.x + 40, snap.virtual_bounds.y + 40);
        println!(
            "window_at({probe:?}) = {:?}",
            svc.window_at(probe).map(|w| (w.hwnd, w.app_name, w.bounds))
        );

        let png =
            crate::encode::encode(&c.frame, &crate::encode::EncodeOptions::default()).unwrap();
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.qoder/scratch/m1-freeze.png");
        crate::encode::save(&c.frame, &out, &crate::encode::EncodeOptions::default()).unwrap();
        println!(
            "wrote {} ({} bytes of PNG, has_transparency={})",
            out.display(),
            png.len(),
            c.frame.has_transparency()
        );
    }
}
