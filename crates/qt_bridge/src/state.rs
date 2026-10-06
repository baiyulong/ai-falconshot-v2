//! The pins that are on screen, and the one place that keeps Qt and the state
//! machine in agreement.
//!
//! `falcon_core::pin` owns the semantics and is tested on its own; this module
//! owns the *live session*: the set, the pixels behind each pin, the desktop the
//! pins are clamped to, and which pins Qt has been handed a texture for. Nothing
//! here knows QML exists.
//!
//! The one rule: a pin's picture crosses the boundary only when the pixels
//! change. Zoom and position are pure window geometry - `render_shown`
//! deliberately ignores zoom, so a wheel gesture resizes a window and re-scales a
//! texture in place without touching the encoder. That is what keeps plan rule 4
//! ("the hot path must not cross") true while the wheel is being spun.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use falcon_core::config::{AlphaBg, Pin as PinConfig};
use falcon_core::encode::{self, EncodeOptions, Format};
use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
use falcon_core::pin::{Desktop, PinId, PinItem, PinSet};

use crate::session::shim;

/// Everything a pin window binds to. Plain fields, because it crosses into a
/// CXX-Qt object one property at a time and has to be readable in a test.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PinViewData {
    /// False once the pin has left the set - the window hides itself.
    pub live: bool,
    pub pos_x: i32,
    pub pos_y: i32,
    pub size_w: i32,
    pub size_h: i32,
    pub zoom: i32,
    pub opacity: i32,
    pub smooth: bool,
    pub topmost: bool,
    pub click_through: bool,
    /// Bumped only when new pixels went out; QML appends it to the provider URL
    /// so a texture it already cached is re-requested.
    pub revision: u32,
}

#[derive(Default)]
pub struct PinState {
    pub set: PinSet,
    pub cfg: PinConfig,
    /// The source picture, in memory. §6.3's snapshot writes these out and reads
    /// them back; until the capture path exists this is the only copy.
    frames: HashMap<PinId, Frame>,
    desk: Desktop,
    pushed: HashMap<PinId, u64>,
    revisions: HashMap<PinId, u32>,
    /// What went wrong loudly enough that a run has to report it, kept so
    /// `--selftest` can assert on an empty list instead of on stdout.
    pub problems: Vec<String>,
}

impl PinState {
    fn new() -> Self {
        Self {
            desk: desktop_from_shim(),
            ..Default::default()
        }
    }

    /// A picture becomes a pin at `at`. The id comes from the set, so `next_id`
    /// and a restored session can never collide.
    pub fn add(&mut self, frame: Frame, at: PhysPoint) -> Result<PinId, String> {
        let size = PhysSize::new(frame.width, frame.height);
        let id = self
            .set
            .spawn(falcon_core::pin::ImageRef::Volatile, size, at, &self.cfg)
            .map_err(|e| e.to_string())?;
        self.frames.insert(id, frame);
        self.sync(id);
        Ok(id)
    }

    /// The fields that decide the pixels. Position, zoom and opacity are absent
    /// on purpose: they decide the window.
    fn content_key(p: &PinItem) -> u64 {
        let region = p.visible_rect();
        let parts: [i64; 10] = [
            region.x as i64,
            region.y as i64,
            region.w as i64,
            region.h as i64,
            p.rotation as i64,
            p.flip_h as i64,
            p.flip_v as i64,
            p.grayscale as i64,
            p.inverted as i64,
            p.alpha_bg_mode as i64,
        ];
        parts.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, v| {
            (h ^ (v as u64)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    /// Render, encode and hand Qt the result - but only if the pixels moved.
    /// Returns whether a new texture went out.
    pub fn sync(&mut self, id: PinId) -> bool {
        let key = match self.set.get(id) {
            Some(p) => Self::content_key(p),
            None => {
                self.pushed.remove(&id);
                self.revisions.remove(&id);
                return false;
            }
        };
        if self.pushed.get(&id) == Some(&key) {
            return false;
        }

        let mut shown: Option<Result<Frame, String>> = None;
        if let (Some(p), Some(src)) = (self.set.get(id), self.frames.get(&id)) {
            shown = Some(
                p.render_shown(src)
                    .map(|f| p.paint_alpha_bg(f))
                    .map_err(|e| e.to_string()),
            );
        }
        let shown = match shown {
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                self.note(format!("pin {id}: render failed: {e}"));
                return false;
            }
            None => {
                self.note(format!("pin {id} has no pixels to draw from"));
                return false;
            }
        };
        let opt = EncodeOptions {
            format: Format::Png,
            ..Default::default()
        };
        let png = match encode::encode(&shown, &opt) {
            Ok(b) => b,
            Err(e) => {
                self.note(format!("pin {id}: encode failed: {e}"));
                return false;
            }
        };

        shim::store_frame(id, &png);
        self.pushed.insert(id, key);
        *self.revisions.entry(id).or_insert(0) += 1;
        true
    }

    /// Apply a view or content change to one pin, then push whatever it produced.
    /// One gesture applied to one pin. `None` when the pin is not on screen,
    /// which is a closed window's leftover click rather than an error.
    pub fn edit<R>(
        &mut self,
        id: PinId,
        f: impl FnOnce(&mut PinItem, &PinConfig, &Desktop) -> R,
    ) -> Option<R> {
        let cfg = self.cfg.clone();
        let desk = self.desk.clone();
        let done = self.set.get_mut(id).map(|p| f(p, &cfg, &desk));
        self.sync(id);
        done
    }

    pub fn view_data(&self, id: PinId) -> PinViewData {
        let Some(p) = self.set.get(id) else {
            return PinViewData {
                live: false,
                ..Default::default()
            };
        };
        PinViewData {
            live: true,
            pos_x: p.pos_phys.x,
            pos_y: p.pos_phys.y,
            size_w: p.size_phys.w as i32,
            size_h: p.size_phys.h as i32,
            zoom: p.zoom as i32,
            opacity: p.opacity as i32,
            smooth: p.smooth_zoom,
            topmost: p.topmost,
            click_through: p.click_through,
            revision: self.revisions.get(&id).copied().unwrap_or(0),
        }
    }

    pub fn ids(&self) -> Vec<PinId> {
        self.set.ids()
    }

    pub fn id_at(&self, index: usize) -> PinId {
        self.set.ids().get(index).copied().unwrap_or(0)
    }

    /// §5.9.18 - off the set and off the screen, and Qt is told to forget the
    /// texture rather than hold one for a pin that is gone.
    pub fn close(&mut self, ids: &[PinId]) -> usize {
        let n = self.set.close(ids);
        for id in ids {
            self.frames.remove(id);
            self.pushed.remove(id);
            self.revisions.remove(id);
            shim::drop_frame(*id);
        }
        n
    }

    fn note(&mut self, message: String) {
        self.problems.push(message);
    }
}

// ---------------------------------------------------------------- the session

static STATE: OnceLock<Mutex<PinState>> = OnceLock::new();

pub fn state() -> &'static Mutex<PinState> {
    STATE.get_or_init(|| Mutex::new(PinState::new()))
}

/// Run one thing against the live set. Every caller is on the GUI thread, so the
/// lock is a formality - but a panic must not take the app down, which is why
/// `into_inner` is used rather than `unwrap`.
pub fn with<R>(f: impl FnOnce(&mut PinState) -> R) -> R {
    let mut guard = state().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// The screen layout, as Qt sees it. Multi-monitor enumeration properly belongs
/// to `platform-windows` (§6.1) - until that lands, the union of Qt's screens is
/// the desktop and the primary is the only monitor a pin can be clamped against.
fn desktop_from_shim() -> Desktop {
    let (dx, dy, dw, dh) = shim::desktop_bounds();
    let (px, py, pw, ph) = shim::primary_bounds();
    Desktop::new(
        PhysRect::new(dx, dy, dw.max(1) as u32, dh.max(1) as u32),
        vec![PhysRect::new(px, py, pw.max(1) as u32, ph.max(1) as u32)],
    )
}

pub fn desktop_summary() -> String {
    let (dx, dy, dw, dh) = shim::desktop_bounds();
    let (px, py, pw, ph) = shim::primary_bounds();
    format!(
        "desktop={dx},{dy} {dw}x{dh} primary={px},{py} {pw}x{ph} dpr={:.2}",
        shim::device_pixel_ratio()
    )
}

pub fn window_report() -> String {
    shim::window_report()
}

/// Qt's own complaints - the QML compiler's errors among them - kept where a
/// headless run can print them.
pub fn install_message_capture() {
    shim::install_message_capture();
}

pub fn messages() -> String {
    shim::messages()
}

/// Set by `main.qml` when its root finishes constructing. The probe reads it,
/// because `topLevelWindows=0` has two very different causes: the document never
/// loaded, or it loaded and made no windows.
static QML_LOADED: AtomicBool = AtomicBool::new(false);

pub fn mark_qml_loaded() {
    QML_LOADED.store(true, Ordering::Relaxed);
}

pub fn qml_loaded() -> bool {
    QML_LOADED.load(Ordering::Relaxed)
}

pub fn quit_after(ms: i32) {
    shim::quit_after(ms);
}

/// §5.9.1: a new pin arrives roughly where the user is looking, and the desktop
/// it is clamped against is Qt's own geometry rather than a guess.
pub fn add_demo(width: u32, height: u32) -> Result<PinId, String> {
    let (_x, _y, pw, ph) = shim::primary_bounds();
    let at = PhysPoint::new(
        pw.saturating_sub(width as i32) / 2,
        ph.saturating_sub(height as i32) / 2,
    );
    with(|s| s.add(demo_frame(width, height), at))
}

/// A picture to pin when nothing else has produced one yet: a checkerboard with a
/// transparent margin, so §5.9's alpha promise is visible before the capture
/// service exists.
pub fn demo_frame(width: u32, height: u32) -> Frame {
    let w = width.clamp(4, 4096);
    let h = height.clamp(4, 4096);
    let mut f = Frame::filled(w, h, [0x11, 0x22, 0x33, 0xff]).expect("frame");
    let cell = (w.min(h) / 6).max(1);
    for y in 0..h {
        for x in 0..w {
            if x < 2 || y < 2 || x + 2 >= w || y + 2 >= h {
                f.set(x, y, [0, 0, 0, 0]);
            } else if (x / cell + y / cell).is_multiple_of(2) {
                f.set(x, y, [0xe8, 0x6c, 0x2c, 0xff]);
            }
        }
    }
    f
}

/// Fully transparent: whatever the window shows is the alpha background, so a
/// board that was not painted cannot hide behind a picture.
fn transparent_frame(width: u32, height: u32) -> Frame {
    Frame::filled(width, height, [0x00, 0x00, 0x00, 0x00]).expect("frame")
}

// ------------------------------------------------------------------- selftest

/// The render → encode → store → decode round trip, checked without showing a
/// window. Every step is one the GUI path uses for real, so this fails when the
/// pixel pipeline is broken rather than only when a binding is missing.
pub fn selftest() -> (bool, String) {
    let mut out = Vec::new();
    let mut ok = true;

    {
        let mut check = |label: &str, passed: bool, detail: String| {
            ok &= passed;
            out.push(format!(
                "{label:<34} {:<4} {detail}",
                if passed { "PASS" } else { "FAIL" }
            ));
        };

        check(
            "desktop geometry is real",
            !desktop_summary().is_empty(),
            desktop_summary(),
        );

        let src = demo_frame(64, 32);
        let id = match with(|s| s.add(src.clone(), PhysPoint::new(10, 10))) {
            Ok(id) => id,
            Err(e) => return (false, format!("could not add a pin: {e}")),
        };
        check(
            "first frame decodes",
            !shim::self_check(id).contains("null=1"),
            shim::self_check(id),
        );

        let after_add = with(|s| s.view_data(id));
        check(
            "adding pushed one texture",
            after_add.revision == 1,
            format!("revision={}", after_add.revision),
        );
        check(
            "the transparent margin survives",
            shim::pixel(id, 0, 0) == "0,0,0,0",
            format!("(0,0)={}", shim::pixel(id, 0, 0)),
        );

        // The whole reason the state machine is authoritative: zoom is geometry.
        let _ = with(|s| s.edit(id, |p, _, _| p.set_zoom(300)));
        let after_zoom = with(|s| s.view_data(id));
        check(
            "zoom re-encodes nothing",
            after_zoom.revision == after_add.revision && after_zoom.size_w == 192,
            format!(
                "revision={} window={}x{} at {},{}",
                after_zoom.revision,
                after_zoom.size_w,
                after_zoom.size_h,
                after_zoom.pos_x,
                after_zoom.pos_y
            ),
        );

        // Rotation swaps the shown rectangle, so the bitmap has to change.
        let _ = with(|s| s.edit(id, |p, _, _| p.rotate_cw()));
        check(
            "rotation re-encodes",
            shim::self_check(id).contains("w=32 h=64"),
            format!(
                "{} revision={}",
                shim::self_check(id),
                with(|s| s.view_data(id).revision)
            ),
        );

        // Effects are content, so they cross too - and the decode must hold.
        let _ = with(|s| {
            s.edit(id, |p, _, _| {
                p.toggle_grayscale();
                p.toggle_invert();
            })
        });
        check(
            "grayscale + invert decodes",
            !shim::self_check(id).contains("null=1"),
            shim::self_check(id),
        );

        // §5.9.15: the board is baked into the texture, not left for QML to invent.
        with(|s| s.cfg.alpha_bg = AlphaBg::CheckerLight);
        let board = match with(|s| s.add(transparent_frame(16, 1), PhysPoint::new(20, 20))) {
            Ok(id) => id,
            Err(e) => return (false, format!("could not add the board pin: {e}")),
        };
        with(|s| s.cfg.alpha_bg = AlphaBg::Transparent);
        check(
            "the board paints see-through",
            shim::pixel(board, 0, 0) == "212,212,212,255"
                && shim::pixel(board, 8, 0) == "239,239,239,255",
            format!(
                "(0,0)={} (8,0)={}",
                shim::pixel(board, 0, 0),
                shim::pixel(board, 8, 0)
            ),
        );

        // A crop is content; the texture shrinks with it.
        let cropped = with(|s| s.edit(board, |p, _, _| p.crop(&PhysRect::new(0, 0, 4, 1))));
        check(
            "crop re-encodes smaller",
            matches!(cropped, Some(Ok(()))) && shim::self_check(board).contains("w=4 h=1"),
            shim::self_check(board),
        );

        // §5.9.11: what the window shows is not what the picture is.
        let thumb = with(|s| {
            let _ = s.edit(board, |p, _, _| {
                p.set_free_thumbnail(&PhysRect::new(2, 0, 1, 1))
            });
            s.view_data(board)
        });
        check(
            "a free thumbnail shows its region",
            shim::self_check(board).contains("w=1 h=1") && thumb.size_w == 1,
            format!(
                "{} window={}x{}",
                shim::self_check(board),
                thumb.size_w,
                thumb.size_h
            ),
        );

        // Closing takes the pin and its texture out together.
        let closed = with(|s| s.close(&[id, board]));
        check(
            "closing drops the pixels",
            closed == 2 && shim::self_check(id).contains("null=1"),
            format!("closed={closed} {}", shim::self_check(id)),
        );

        let problems = with(|s| s.problems.clone());
        check(
            "no problems were noted",
            problems.is_empty(),
            problems.join(" | "),
        );

        out.push(format!("selftest: {}", if ok { "PASS" } else { "FAIL" }));
    }

    (ok, out.join("\n"))
}
