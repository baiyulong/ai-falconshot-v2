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
use std::time::Instant;

use falcon_core::config::{AlphaBg, Pin as PinConfig};
use falcon_core::encode::{self, EncodeOptions, Format};
use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
use falcon_core::pin::{Desktop, PinId, PinItem, PinSet};

use crate::capture;
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
    /// The picture as the pin currently *shows* it - cropped, turned, effected.
    /// This is what §5.9.16's copy and §5.9.17's save hand over, and it is the
    /// "one source bitmap per pin" the plan asks for as the fallback against a
    /// screen-capture path that cannot see our own window (R13).
    rendered: HashMap<PinId, Frame>,
    desk: Desktop,
    pushed: HashMap<PinId, u64>,
    revisions: HashMap<PinId, u32>,
    /// What went wrong loudly enough that a run has to report it, kept so
    /// `--selftest` can assert on an empty list instead of on stdout.
    pub problems: Vec<String>,
    /// PRD 447's reminder, said by [`Self::remind_before_save`] and by
    /// [`Self::save_image`]'s first rung - always the sentence about a write that has
    /// not happened yet, so it never carries the result of one. The mask's own copy of
    /// this slot is `MaskState::pre_notice`; the two are the same string by
    /// construction because both come out of [`crate::save::reminder`].
    pub pre_notice: String,
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
        self.rendered.insert(id, shown);
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
            self.rendered.remove(id);
            self.pushed.remove(id);
            self.revisions.remove(id);
            shim::drop_frame(*id);
        }
        n
    }

    /// Hands the pin's current picture to one of the output paths. Taking the
    /// frame by reference rather than cloning it is the point: a 4K pin is 33 MB
    /// and a copy gesture should not pay for it twice.
    ///
    /// Reachable from the gauges since §10 第 43 项 ①: a row that has to say whether the
    /// picture a pin shows is the picture a file holds has to read the frame the pin was
    /// *handed*, and there is no other way to ask without copying megabytes into the row.
    pub(crate) fn with_rendered<R>(&self, id: PinId, f: impl FnOnce(&Frame) -> R) -> Option<R> {
        self.rendered.get(&id).map(f)
    }

    /// §5.9.16 - the pin's own pixels onto the clipboard.
    pub fn copy_image(&self, id: PinId) -> Result<usize, String> {
        let Some(done) = self.with_rendered(id, |frame| {
            platform_windows::clip::write_image(frame)
                .map(|_| (frame.width as usize) * (frame.height as usize))
                .map_err(|e| e.to_string())
        }) else {
            return Err(not_drawn(id));
        };
        done
    }

    /// [`Self::remind_before_save`]'s wording without the assignment: what this encoder is
    /// about to cost this pin, in the future tense PRD 447 asks for.
    ///
    /// The empty string is one answer with three comings - a format that carries alpha, a
    /// pin that was never drawn, and a name that decides no format - which is why it reads
    /// as "this door loses no transparency" and never as "no reminder was due". The
    /// sentence itself has one owner ([`crate::save::reminder`]) and reaches the mask's
    /// three doors unchanged, because only the subject differs: 贴图, not 选区.
    fn remind_with(&self, id: PinId, path: &str, chosen: &str, export: &EncodeOptions) -> String {
        self.with_rendered(id, |frame| {
            crate::save::reminder(frame, path, chosen, export, "贴图").unwrap_or_default()
        })
        .unwrap_or_default()
    }

    /// [`Self::save_image`]'s first rung on its own, so the order is *callable*: a gauge
    /// can ask what this door said at the moment the file it warns about was not on disk
    /// yet. It writes nothing, and the write that follows it is the product's own
    /// [`Self::save_image`] - a reminder is not a confirmation.
    pub fn remind_before_save(&mut self, id: PinId, path: &str, chosen: &str) -> String {
        self.pre_notice = self.remind_with(id, path, chosen, &crate::settings::export());
        self.pre_notice.clone()
    }

    /// §5.9.17 - the pin out to a file, through the same owner the mask's 另存为 uses
    /// ([`crate::save`]), so the two doors read `[output] jpeg_quality`, answer in the
    /// same Chinese sentence and name the pixels a format without alpha threw away.
    /// The extension picks the format, so `core::encode`'s ICO-downscale and
    /// JPG-flatten rules apply on the way; a name without one takes the format the
    /// dialog was showing (`chosen`), and only when that offers nothing the configured
    /// one - which is the half P35 fixed, widened by §10 第 40 项 ② because PRD §5.5.5
    /// has the user choosing 格式 in that dialog too.
    ///
    /// The first rung is [`Self::remind_before_save`]'s, in the same order and for the same
    /// reason as the mask's `save_out`: PRD 447 says 背景**将**被填充, and
    /// [`crate::save::write`] turns that into 已填成白色 the moment it returns. §10 第 46 项
    /// ③ registered this door as the one that only had the after half; this is the before
    /// half, and the write still happens.
    pub fn save_image(&mut self, id: PinId, path: &str, chosen: &str) -> Result<String, String> {
        let export = crate::settings::export();
        self.pre_notice = self.remind_with(id, path, chosen, &export);
        let Some(done) = self.with_rendered(id, |frame| {
            match crate::save::write(frame, path, &export, "贴图", chosen) {
                Ok(said) => Ok(said),
                Err(crate::save::Refusal::BadName(e)) => Err(e),
                Err(crate::save::Refusal::Write(e)) => {
                    Err(format!("{e}。贴图还在，可以再保存一次。"))
                }
            }
        }) else {
            return Err(not_drawn(id));
        };
        done
    }

    /// §5.8.3 / §10 第 43 项 ① - a file that is already on disk becomes a pin.
    ///
    /// Until P43 the only way a picture living in a file could reach the screen was the
    /// clipboard: [`Self::add_from_clipboard`] took a `Files` payload and decoded its
    /// first entry (`encode::decode_file(first)`). That made 存完再钉图 an accident - the
    /// user had to copy the file, and it made §5.8.3's own "选择图片文件 → 系统读取图片并
    /// 创建贴图" unreachable without a trip through the shell.
    ///
    /// Reading the file is the whole of the difference from [`Self::add`], and the reason
    /// to do it here rather than at each door is the second half of PRD line 910: a
    /// damaged or unsupported file "应显示错误信息". Spelled once, both doors answer the
    /// same way, and neither one gets to forward `EncodeError`'s English.
    pub fn add_from_file(&mut self, path: &str, at: PhysPoint) -> Result<PinId, String> {
        let frame = match encode::decode_file(std::path::Path::new(path)) {
            Ok(f) => f,
            Err(e) => return Err(read_refusal(path, &e)),
        };
        self.add(frame, at)
    }

    /// §5.8.2 - whatever the user copied becomes a pin. Deciding *what it was* is
    /// `core::clip::classify`'s job, already tested; this only moves bytes.
    pub fn add_from_clipboard(&mut self, at: PhysPoint) -> Result<PinId, String> {
        let contents = platform_windows::clip::read().map_err(|e| e.to_string())?;
        if contents.is_empty() {
            return Err("剪贴板是空的".into());
        }
        let payload = {
            let facts = contents.facts();
            falcon_core::clip::classify(&facts).map_err(|e| e.to_string())?
        };
        let frame = match payload {
            (falcon_core::clip::ClipKind::Image, falcon_core::clip::ClipPayload::Image(f)) => f,
            (falcon_core::clip::ClipKind::Files, falcon_core::clip::ClipPayload::Files(paths)) => {
                let first = paths.first().ok_or("剪贴板里的文件列表是空的")?;
                encode::decode_file(first).map_err(|e| e.to_string())?
            }
            (falcon_core::clip::ClipKind::Colour, falcon_core::clip::ClipPayload::Colour(rgba)) => {
                // §5.12: a colour swatch is a pin of one pixel, which the text
                // card's renderer will draw properly in Phase 2.
                Frame::filled(64, 64, rgba).map_err(|e| e.to_string())?
            }
            (_, falcon_core::clip::ClipPayload::Empty) => Err("剪贴板没有可贴图的内容")?,
            (kind, _) => Err(format!("{kind:?} 贴图还未实现（Phase 2）"))?,
        };
        self.add(frame, at)
    }

    fn note(&mut self, message: String) {
        self.problems.push(message);
    }
}

/// What went wrong with a file, said the way the user reads it.
///
/// PRD line 910 asks for a message on both shapes, and they are different answers: one says
/// the file is broken, the other says this program does not read that kind of file. The
/// decoder cannot tell them apart - an unrecognised extension and a damaged file both
/// arrive as `Codec`, because `encode::decode_file` hints the format from the extension and
/// falls back to guessing from the bytes - so the name is asked as well as the error.
/// The file's own path is the one piece that travels through as typed: it is what the user
/// can find in the dialog again.
fn read_refusal(path: &str, e: &encode::EncodeError) -> String {
    use encode::EncodeError as E;
    let readable = encode::IMPORT_FORMATS
        .iter()
        .map(|f| f.ext())
        .collect::<Vec<_>>()
        .join("、");
    let named = std::path::Path::new(path)
        .extension()
        .and_then(|x| x.to_str())
        .and_then(encode::Format::from_ext);
    match e {
        E::Io(_) => format!("{path} 打不开：文件读不进来（可能已被移走、正被占用，或没有权限）"),
        E::TooLarge(w, h) => format!("{path} 太大读不进来（{w}×{h} 像素）"),
        E::Codec(_) | E::Unsupported(_) => match named {
            None => format!("{path} 不是本程序会读的图片：用 {readable}。"),
            Some(_) => format!("{path} 读不出图片：文件可能已损坏。"),
        },
        E::Frame(_) | E::TransparencyLost(_) => format!("{path} 钉不上：这张图拼不出像素"),
    }
}

/// What both output doors say about a pin that has no picture yet.
///
/// One sentence rather than two copies because it is a promise to the user, not a
/// log line: [`crate::session`] wraps it under `复制失败：` and `保存失败：`, and an
/// English clause inside either prefix is what §10 第 37 项 ① measured for the first
/// time in P37. It names 贴图 so the wrapping reads as one Chinese sentence, and the
/// `--pin` gauge judges exactly those two properties (contains 贴图, no English).
fn not_drawn(id: PinId) -> String {
    format!("贴图 {id} 还没有画过，没有可输出的像素")
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
        "desktop={dx},{dy} {dw}x{dh} primary={px},{py} {pw}x{ph} dpr={:.2} {}",
        shim::device_pixel_ratio(),
        platform_windows::desktop::station_summary()
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
    stamp("qml");
}

pub fn qml_loaded() -> bool {
    QML_LOADED.load(Ordering::Relaxed)
}

/// Cold-path stamps: milliseconds since the first one, per named stage.
///
/// `--mask` prints one `cold of` figure and §10-14 ② has to reduce it, but a single
/// number cannot say which part a pre-warm can even reach. Creating the QML engine
/// and compiling the document is work a resident process has already finished before
/// any hot key is pressed; the freeze, the texture hand-off and the first painted
/// frame have not. Stamping both ends of each stage is what turns "1078 ms, reduce
/// it" into "these 700 ms disappear by itself, these 378 ms are the ones to engineer".
static CLOCK: OnceLock<Instant> = OnceLock::new();
static STAMPS: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());

/// Record `name` once, at its elapsed time. A second call with the same name is
/// ignored on purpose: these mark the *first* time a stage happened, which is the
/// only thing a cold path has.
pub fn stamp(name: &'static str) {
    let clock = CLOCK.get_or_init(Instant::now);
    let ms = clock.elapsed().as_millis() as u64;
    let mut v = STAMPS.lock().unwrap_or_else(PoisonError::into_inner);
    if !v.iter().any(|(n, _)| *n == name) {
        v.push((name, ms));
    }
}

/// Milliseconds between two stamps, or `None` when either is missing. Missing is
/// never reported as `0`: a stage that did not run must not read as one that was free.
pub fn between(a: &str, b: &str) -> Option<u64> {
    let v = STAMPS.lock().unwrap_or_else(PoisonError::into_inner);
    let at = |n: &str| v.iter().find(|(x, _)| *x == n).map(|(_, ms)| *ms);
    Some((at(b)? as i64 - at(a)? as i64).max(0) as u64)
}

/// `name=ms` for every stamp taken so far, in the order they were taken.
pub fn stamp_line() -> String {
    let v = STAMPS.lock().unwrap_or_else(PoisonError::into_inner);
    v.iter()
        .map(|(n, ms)| format!("{n}={ms}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn quit_after(ms: i32) {
    shim::quit_after(ms);
}

/// The `--mask` gauge's second and later rounds.
///
/// `main.rs` reads the command line and writes this before the QML engine exists;
/// `Session` pulls it into qproperties from `Component.onCompleted`, because the
/// qobject is created by QML and Rust has no handle to push through beforehand.
/// Only a QML `Timer` can act once the event loop is running, and the question the
/// rounds answer - what a hot key really costs - cannot be asked before then.
#[derive(Default)]
pub struct WarmPlan {
    /// Rounds still owed. Counts down as they run, so the timer stops itself.
    pub left: i32,
    pub step_ms: i32,
    pub shader: bool,
    pub rounds: Vec<String>,
}

static WARM: OnceLock<Mutex<WarmPlan>> = OnceLock::new();

pub fn warm<R>(f: impl FnOnce(&mut WarmPlan) -> R) -> R {
    let mut guard = WARM
        .get_or_init(|| Mutex::new(WarmPlan::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// Where a new pin goes: beside the pointer, because that is where the user was
/// looking when they pressed the key, stepped aside 8 px so the picture does not
/// hide under the arrow. The state machine then clamps it to the desktop, which is
/// what keeps a paste near a screen edge partly visible rather than off-screen.
///
/// The centre of the primary screen is the fallback, and only for a build that has
/// no pointer to ask (a non-Windows compile of this workspace).
fn next_drop_point() -> PhysPoint {
    if let Some(p) = platform_windows::proc::cursor_pos() {
        return PhysPoint::new(p.x + 8, p.y + 8);
    }
    let (x, y, w, h) = shim::primary_bounds();
    let step = with(|s| s.ids().len()).min(24) as i32;
    PhysPoint::new(x + w / 2 + step * 16, y + h / 2 + step * 16)
}

/// §5.8.2 - the clipboard becomes a pin. Shared by the QML menu path and by
/// `--paste`, which exists so this can be verified on a real machine today rather
/// than only once the global hotkey lands in M5.
pub fn paste_from_clipboard() -> Result<PinId, String> {
    with(|s| s.add_from_clipboard(next_drop_point()))
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

        // Two questions in one row, because answering only the first still shows
        // nobody a pin: is the desktop measurable, and is it the desktop the user
        // is looking at. A process launched from a service or a scheduled task
        // measures a perfect geometry on a station nobody can see.
        let (_, _, dw, dh) = shim::desktop_bounds();
        check(
            "desktop geometry is real",
            dw > 0 && dh > 0 && desktop_summary().contains("station=WinSta0"),
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

/// The output chain, for real: a pin onto the clipboard, the clipboard back in,
/// and a pin out to a file and read again.
///
/// Three answers, not two: another app may be holding the clipboard, and that is
/// reported as `BLOCKED` with the holder named rather than as a row of failures.
///
/// This is kept apart from [`selftest`] because it *takes the user's clipboard
/// over*. What was on it is read first and put back at the end, which is the best
/// that can be promised: a clipboard can carry formats this app does not model,
/// and those would be lost. `--selftest --clipboard`.
/// A row of the clipboard check has three answers, and they are not two.
///
/// Ordering matters as much as the names: `Fail` dominates `Blocked`, so a run
/// that is both cannot report only the excuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    Pass,
    Blocked,
    Fail,
}

impl Check {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Check::Pass => "PASS",
            Check::Blocked => "SKIP",
            Check::Fail => "FAIL",
        }
    }
}

impl From<bool> for Check {
    fn from(passed: bool) -> Self {
        if passed {
            Check::Pass
        } else {
            Check::Fail
        }
    }
}

/// Put back what the user had, as far as this app can represent it, and say how far
/// that was.
///
/// Two gauges owe the machine this now: `--selftest --clipboard` writes a test picture,
/// and `--ink`'s 复制 leg goes through the product's own clipboard call, which writes
/// the screenshot. Both take the user's contents away in order to measure something,
/// and the debt is paid here rather than in two copies of the same four branches -
/// the copy that forgets the `html or a file list` case is the one that silently eats
/// what was in front of the user, and the copy that forgets *empty* leaves its own test
/// picture on a clipboard the user had cleared.
pub fn put_back(before: &Option<platform_windows::clip::Contents>) -> Result<String, String> {
    match before {
        // `read()` answering `None` is not an empty clipboard: it is a clipboard that
        // would not open, and whatever was on it went with the write that followed. The
        // leg that reports that as "it was empty" is a leg claiming to have restored
        // what it never read.
        None => Err("it would not open for reading, so what was on it is not knowable".to_string()),
        Some(c) if c.is_empty() => platform_windows::clip::clear()
            .map(|_| "it was empty, and is empty again".to_string())
            .map_err(|e| e.to_string()),
        Some(c) => {
            if let Some(bytes) = &c.image_bytes {
                match encode::decode(bytes) {
                    Ok(frame) => platform_windows::clip::write_image(&frame)
                        .map(|_| format!("image {}x{}", frame.width, frame.height))
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                }
            } else if let Some(t) = &c.text {
                platform_windows::clip::write_text(t)
                    .map(|_| format!("text ({} chars)", t.chars().count()))
                    .map_err(|e| e.to_string())
            } else {
                Ok("html or a file list, which this check cannot put back".to_string())
            }
        }
    }
}

pub fn clipboard_selftest() -> (Check, String) {
    // Ask first, because the clipboard can belong to somebody else. When it does,
    // the honest answer is who holds it - not a row of failures that reads as if
    // this program's clipboard code were broken.
    let blocked = platform_windows::clip::usable()
        .err()
        .map(|e| e.to_string());
    let before = platform_windows::clip::read().ok();
    let mut out = Vec::new();
    let mut worst = if blocked.is_some() {
        Check::Blocked
    } else {
        Check::Pass
    };
    let mut check = |label: &str, verdict: Check, detail: String| {
        if verdict > worst {
            worst = verdict;
        }
        out.push(format!("{label:<34} {:<4} {detail}", verdict.tag()));
    };

    // Semi-transparent on purpose: whether alpha survives the trip is the whole
    // question this check exists to answer, and an opaque test picture would
    // pass by accident.
    let src = match Frame::from_rgba(
        4,
        2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 0, 255, //
            0, 255, 255, 0, 255, 0, 255, 32, 12, 34, 56, 78, 200, 100, 50, 255,
        ],
    ) {
        Ok(f) => f,
        Err(e) => return (Check::Fail, format!("test frame is not a frame: {e}")),
    };
    let id = match with(|s| s.add(src.clone(), PhysPoint::new(30, 30))) {
        Ok(id) => id,
        Err(e) => return (Check::Fail, format!("could not add a pin: {e}")),
    };

    let copied = with(|s| s.copy_image(id));
    match &blocked {
        Some(who) => check(
            "a pin copied itself",
            Check::Blocked,
            format!("clipboard held by {who}"),
        ),
        None => check(
            "a pin copied itself",
            Check::from(copied.is_ok()),
            copied
                .clone()
                .map(|n| format!("{n} pixels"))
                .unwrap_or_else(|e| e),
        ),
    }

    let formats = platform_windows::clip::formats();
    match (&blocked, &formats) {
        (Some(who), _) => check(
            "the clipboard carries a DIB",
            Check::Blocked,
            who.to_string(),
        ),
        (None, Err(e)) => check("the clipboard carries a DIB", Check::Fail, e.to_string()),
        (None, Ok(list)) => check(
            "the clipboard carries a DIB",
            Check::from(list.iter().any(|f| f == "#8")),
            list.join(","),
        ),
    }

    // Read it back through the same door §5.8.2 uses.
    let back = match platform_windows::clip::read() {
        Ok(contents) => {
            let facts = contents.facts();
            falcon_core::clip::classify(&facts)
                .map(|(_kind, payload)| match payload {
                    falcon_core::clip::ClipPayload::Image(f) => Some(f),
                    other => {
                        eprintln!("[clipboard] classified as {other:?}");
                        None
                    }
                })
                .map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    };
    match &blocked {
        Some(who) => check(
            "paste-back is the same pixels",
            Check::Blocked,
            who.to_string(),
        ),
        None => {
            let round = matches!(&back, Ok(Some(frame)) if *frame == src);
            check(
                "paste-back is the same pixels",
                Check::from(round),
                match back {
                    Ok(Some(frame)) => {
                        format!("{}x{} {:?}", frame.width, frame.height, frame.pixels)
                    }
                    Ok(None) => "not an image payload".to_string(),
                    Err(e) => e,
                },
            );
        }
    }

    let file = std::env::temp_dir().join("falconshot-clipcheck.png");
    let saved = with(|s| s.save_image(id, &file.to_string_lossy(), ""));
    let again = saved
        .as_ref()
        .ok()
        .and_then(|_| encode::decode_file(&file).ok());
    check(
        "a pin saved and re-read",
        Check::from(again.as_ref() == Some(&src)),
        format!(
            "{} -> {:?}",
            saved.unwrap_or_default(),
            again.map(|f| f.pixels).unwrap_or_default()
        ),
    );

    let closed = with(|s| s.close(&[id]));
    check(
        "the check cleaned up after itself",
        Check::from(closed == 1 && file.exists()),
        format!("closed={closed} file kept for inspection"),
    );
    let _ = std::fs::remove_file(&file);

    // Whatever the user had on the clipboard goes back, as far as this app can
    // represent it - and the check says which far that was.
    match &blocked {
        Some(who) => check("the clipboard came back", Check::Blocked, who.to_string()),
        None => {
            let restored = put_back(&before);
            check(
                "the clipboard came back",
                Check::from(restored.is_ok()),
                restored.unwrap_or_else(|e| e),
            );
        }
    }

    out.push(format!(
        "clipboard selftest: {}",
        match worst {
            Check::Pass => "PASS",
            Check::Blocked => "BLOCKED",
            Check::Fail => "FAIL",
        }
    ));
    (worst, out.join("\n"))
}

/// The capture layer checked against the real desktop: enumerate, freeze, crop,
/// pick a colour, look at a window, pin a region. `--selftest --capture`.
///
/// Every row asserts a *relationship* rather than a fixed value, because what
/// happens to be on this screen is not knowable in advance - and a relationship
/// that holds for any content (a frame exactly the size of the monitor it came
/// from) is precisely what a stretched or mis-cropped capture breaks.
///
/// `BLOCKED` is for the machine refusing to be looked at - a capture backend that
/// will not start is an environment fact, not a failing test.
pub fn capture_selftest() -> (Check, String) {
    let mut out = Vec::new();
    let mut worst = Check::Pass;
    let mut check = |label: &str, verdict: Check, detail: String| {
        if verdict > worst {
            worst = verdict;
        }
        out.push(format!("{label:<34} {:<4} {detail}", verdict.tag()));
    };

    let listed = capture::service().monitors();
    match &listed {
        Err(e) => check("monitor enumeration", Check::Blocked, e.to_string()),
        Ok(list) => {
            let primaries = list.iter().filter(|m| m.primary).count();
            let line = list
                .iter()
                .map(|m| {
                    format!(
                        "{} {}x{}@{},{} x{:.2}{}",
                        m.id,
                        m.bounds.w,
                        m.bounds.h,
                        m.bounds.x,
                        m.bounds.y,
                        m.scale.ratio(),
                        if m.primary { "*" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ");
            // Two primaries is the enumeration lying about the desktop, and it
            // decides which scale a DIP readout uses (PRD §5.3.7).
            check(
                "monitor enumeration",
                Check::from(!list.is_empty() && primaries == 1),
                format!("{} monitor(s), {primaries} primary: {line}", list.len()),
            );
        }
    }

    let frozen = capture::freeze();
    match &frozen {
        Err(e) => check(
            "freeze the whole screen",
            Check::Blocked,
            format!("{e} - every row below needs a frame"),
        ),
        Ok(f) => {
            // The one shape a later crop depends on: `Frame::crop` works in the
            // frame's own pixels, so a frame smaller than its monitor turns "the
            // right-hand half" into a stretched copy instead of a crop (PRD §4.3).
            let bad: Vec<String> = f
                .snap
                .monitors
                .iter()
                .filter(|m| m.frame.width != m.info.bounds.w || m.frame.height != m.info.bounds.h)
                .map(|m| {
                    format!(
                        "{} frame {}x{} vs bounds {}x{}",
                        m.info.id, m.frame.width, m.frame.height, m.info.bounds.w, m.info.bounds.h
                    )
                })
                .collect();
            check(
                "freeze the whole screen",
                Check::from(bad.is_empty()),
                if bad.is_empty() {
                    format!("{} ms, {}", f.ms, capture::snapshot_line(&f.snap))
                } else {
                    bad.join(" | ")
                },
            );
            // P1/P5 measured 142-155 ms for one 3072x1920 BitBlt; the 150 ms line is
            // for hot key to *mask visible*, so a freeze alone gets the benefit of
            // the doubt up to a ceiling that still means something is wrong.
            check(
                "freeze is not pathological",
                Check::from(f.ms <= 400),
                format!("{} ms", f.ms),
            );
        }
    }
    let Some(snap) = frozen.as_ref().ok().map(|f| f.snap.clone()) else {
        out.push(format!(
            "capture selftest: {}",
            match worst {
                Check::Pass => "PASS",
                Check::Blocked => "BLOCKED",
                Check::Fail => "FAIL",
            }
        ));
        return (worst, out.join("\n"));
    };

    // Indexing is safe here and nowhere else: `freeze` answers `Empty` rather than
    // an snapshot with no monitors, and `snap` came from a successful freeze.
    let primary = snap
        .monitors
        .iter()
        .find(|m| m.info.primary)
        .unwrap_or(&snap.monitors[0]);
    let pb = primary.info.bounds;
    let centre = PhysPoint::new(pb.x + pb.w as i32 / 2, pb.y + pb.h as i32 / 2);

    // Win32 answers in pixels, Qt answers in device-independent pixels. Both are
    // right, and a pin's position is handed to Qt as physical - so the two have to
    // be the same desktop divided by the scale factor, or one of them is describing
    // a screen that is not there.
    {
        let (qx, qy, qw, qh) = shim::primary_bounds();
        let s = primary.info.scale;
        let close = (pb.w as f64 / s.ratio() - qw as f64).abs() <= 2.0
            && (pb.h as f64 / s.ratio() - qh as f64).abs() <= 2.0
            && s.phys_to_dip_i(pb.x) == qx
            && s.phys_to_dip_i(pb.y) == qy;
        check(
            "Win32 and Qt agree on geometry",
            Check::from(close),
            format!(
                "win32={}x{}@{},{} scale={:.2} -> dip {}x{}@{},{} qt={qw}x{qh}@{qx},{qy}",
                pb.w,
                pb.h,
                pb.x,
                pb.y,
                s.ratio(),
                s.phys_to_dip_i(pb.w as i32),
                s.phys_to_dip_i(pb.h as i32),
                s.phys_to_dip_i(pb.x),
                s.phys_to_dip_i(pb.y),
            ),
        );
    }

    // The colour picker and the magnifier both read the frozen frame, which is the
    // only reason they are correct under a dimmed mask (PRD §5.4.2).
    {
        let picked = snap.color_at(centre);
        let direct = primary
            .frame
            .get((centre.x - pb.x) as u32, (centre.y - pb.y) as u32);
        check(
            "colour pick reads the frame",
            Check::from(picked == Some(direct)),
            format!("{picked:?} vs {direct:?}"),
        );
    }

    {
        let mag = snap.magnifier(centre, 8, 4);
        check(
            "magnifier is a hard-edged zoom",
            Check::from(
                mag.as_ref()
                    .is_ok_and(|f| f.width == (8 * 2 + 1) * 4 && f.height == (8 * 2 + 1) * 4),
            ),
            match &mag {
                Ok(f) => format!("{}x{} for radius 8 zoom 4", f.width, f.height),
                Err(e) => e.to_string(),
            },
        );
    }

    // Asking for more desktop than exists must come back with what exists, and say
    // so (PRD §5.2.5/§8.5) - never an error and never a padded edge.
    {
        let want = PhysRect::new(pb.x + pb.w as i32 - 60, pb.y, 200, 80);
        let got = snap.capture(&want);
        let right = snap.virtual_bounds.x + snap.virtual_bounds.w as i32;
        check(
            "a rect off the edge is clipped",
            Check::from(
                got.as_ref()
                    .is_ok_and(|c| c.clipped && c.rect.x + c.rect.w as i32 <= right),
            ),
            match &got {
                Ok(c) => format!(
                    "asked 200 wide, got {}x{} at {},{} clipped={}",
                    c.rect.w, c.rect.h, c.rect.x, c.rect.y, c.clipped
                ),
                Err(e) => e.to_string(),
            },
        );
    }

    // Two monitors side by side: one image, no stretch, both named (PRD §5.3.10).
    {
        let pair = snap.monitors.iter().find_map(|a| {
            snap.monitors
                .iter()
                .find(|b| {
                    b.info.id != a.info.id
                        && b.info.bounds.x == a.info.bounds.x + a.info.bounds.w as i32
                })
                .map(|b| (a, b))
        });
        match pair {
            None => check(
                "a straddle stitches two frames",
                Check::Blocked,
                "no two monitors are adjacent".to_string(),
            ),
            Some((a, b)) => {
                let seam = a.info.bounds.x + a.info.bounds.w as i32;
                let got = snap.capture(&PhysRect::new(seam - 20, a.info.bounds.y + 10, 40, 20));
                check(
                    "a straddle stitches two frames",
                    Check::from(
                        got.as_ref()
                            .is_ok_and(|c| c.monitors.len() == 2 && c.frame.width == 40),
                    ),
                    match &got {
                        Ok(c) => format!(
                            "{}x{} from [{}]",
                            c.frame.width,
                            c.frame.height,
                            c.monitors.join(",")
                        ),
                        Err(e) => e.to_string(),
                    },
                );
                let _ = b;
            }
        }
    }

    {
        let wins = capture::service().windows(false);
        match wins {
            Err(e) => check("window enumeration", Check::Blocked, e.to_string()),
            Ok(list) => {
                let dwmed = list.iter().filter(|w| w.dwm_bounds.is_some()).count();
                // A window shot is cropped by the DWM rect, not by `GetWindowRect`,
                // which carries the invisible resize border along with it. Zero here
                // means the platform layer is answering nothing.
                check(
                    "window enumeration",
                    Check::from(!list.is_empty() && dwmed > 0),
                    format!(
                        "{} window(s), {dwmed} with DWM bounds; top {:?}",
                        list.len(),
                        list.first().map(|w| (w.title.clone(), w.visible_bounds())),
                    ),
                );
            }
        }
    }

    // End to end, on pixels that exist: a region of the frozen screen becomes a pin
    // of exactly that size, and the pin leaves no residue behind.
    {
        let want = PhysRect::new(pb.x + 40, pb.y + 40, 48, 32);
        let got = capture::pin_rect(&want);
        let seen = got
            .as_ref()
            .ok()
            .and_then(|id| with(|s| s.with_rendered(*id, |f| (f.width, f.height))))
            .unwrap_or((0, 0));
        let closed = match &got {
            Ok(id) => with(|s| s.close(&[*id])),
            Err(_) => 0,
        };
        check(
            "a frozen region becomes a pin",
            Check::from(got.is_ok() && seen == (48, 32) && closed == 1),
            format!(
                "{} -> {}x{} closed={}",
                match &got {
                    Ok(id) => format!("pin {id}"),
                    Err(e) => e.clone(),
                },
                seen.0,
                seen.1,
                closed
            ),
        );
    }

    // The other input path: whatever the pointer is over becomes a picture. Blocked
    // rather than failed when the pointer rests on something that cannot be grabbed
    // (the desktop itself, or a window another process owns).
    {
        match capture::window_under_cursor() {
            None => check(
                "the window under the cursor",
                Check::Blocked,
                format!(
                    "no enumerable window at {:?}",
                    platform_windows::proc::cursor_pos()
                ),
            ),
            Some(w) => {
                let got = capture::pin_window(w.hwnd);
                let seen = got
                    .as_ref()
                    .ok()
                    .and_then(|id| with(|s| s.with_rendered(*id, |f| (f.width, f.height))))
                    .unwrap_or((0, 0));
                let closed = match &got {
                    Ok(id) => with(|s| s.close(&[*id])),
                    Err(_) => 0,
                };
                check(
                    "the window under the cursor",
                    Check::from(got.is_ok() && seen.0 > 0 && seen.1 > 0 && closed == 1),
                    format!(
                        "{} ({}x{}) -> {}x{} closed={}",
                        if w.title.is_empty() {
                            w.app_name.clone()
                        } else {
                            w.title.clone()
                        },
                        w.visible_bounds().w,
                        w.visible_bounds().h,
                        seen.0,
                        seen.1,
                        closed
                    ),
                );
            }
        }
    }

    capture::clear();
    out.push(format!("capture summary: {}", capture::monitors_line()));
    out.push(format!(
        "capture selftest: {}",
        match worst {
            Check::Pass => "PASS",
            Check::Blocked => "BLOCKED",
            Check::Fail => "FAIL",
        }
    ));
    (worst, out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::{not_drawn, read_refusal, Frame, PinState};
    use falcon_core::encode::EncodeError;

    /// PRD 910's two shapes are one error to the decoder - both a damaged file and an
    /// unrecognised extension arrive as `Codec`, because an unknown suffix only means the
    /// format had to be guessed from the bytes - so the split lives in this file and is
    /// decided by the name. A refusal that says the wrong half is a user told to go look
    /// for a broken file that is fine, which is why the two arms are graded against each
    /// other and not only against English.
    #[test]
    fn a_file_that_will_not_decode_is_named_by_what_the_name_says() {
        let damaged = read_refusal(
            "C:\\temp\\broken.png",
            &EncodeError::Codec("could not determine image size".into()),
        );
        let unsupported = read_refusal(
            "C:\\temp\\notes.qqq",
            &EncodeError::Codec("Image format not recognized".into()),
        );
        for (said, word, other, name) in [
            (&damaged, "损坏", "不是本程序会读的图片", "broken.png"),
            (&unsupported, "不是本程序会读的图片", "损坏", "notes.qqq"),
        ] {
            assert!(said.contains(word) && said.contains(name), "{said}");
            assert!(!said.contains(other), "{said} answers two questions");
            for english in [
                "io: ",
                "codec failed",
                "unsupported format",
                "not recognized",
            ] {
                assert!(!said.contains(english), "{said} forwards {english:?}");
            }
        }
    }

    /// §10 第 37 项 ①. `session` prints this under `复制失败：` and `保存失败：`, so for
    /// as long as it was English the line a user read was one Chinese prefix wrapped
    /// around a sentence they could not use. The `--pin` gauge is the acceptance; this
    /// is the guard for the runs with no screen attached, and it is written against the
    /// whole sentence because a prefix is not allowed to be the only Chinese part of a
    /// Chinese interface.
    #[test]
    fn an_undrawn_pin_answers_without_an_english_word() {
        let said = not_drawn(7);
        assert!(said.contains("贴图") && said.contains('7'), "{said}");
        assert!(
            !said.chars().any(|c| c.is_ascii_alphabetic()),
            "the sentence still carries Latin letters: {said}"
        );
    }

    /// A file that is simply not there is the third shape line 910 covers, and the one
    /// this function cannot get from a name: the extension is a real one, so only the
    /// operating system's own answer says why.
    #[test]
    fn a_file_that_is_not_there_names_the_file() {
        let said = read_refusal(
            "C:\\temp\\gone.png",
            &EncodeError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)),
        );
        assert!(
            said.contains("gone.png") && said.contains("打不开"),
            "{said}"
        );
        assert!(!said.contains("io: "), "{said}");
    }

    /// A directory under the temp root that no earlier leg of this test left behind.
    /// Same shape as `mask::tests::sandbox`, and only there to keep two doors' files from
    /// reading each other's leftovers.
    fn sandbox(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-pin-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    /// §10 第 47 项 ②: the pin door's half of what 第 46 项 ① proved of the mask door.
    /// PRD 447 says the user **应提醒**背景**将**被填充, so the judgment is about
    /// *order*: at the moment the sentence is spoken, the file must not exist yet, and the
    /// sentence itself must not create it. The second half is that the two sentences
    /// differ only in tense - one user, one door, no two ways of saying it.
    ///
    /// This does not go through [`PinState::add`]: `sync` hands the texture to Qt's image
    /// provider and there is no `QGuiApplication` under `cargo test`. The frame is placed
    /// straight into `rendered`, which *is* "how this pin currently shows itself" and is
    /// the one slot both the reminder and the save read - so the ordering under test is
    /// the ordering the product has, while the pixels→texture leg stays the gauge's
    /// (`pin saved png`, which compares the pin's bytes against the mask's).
    #[test]
    fn the_pin_door_says_it_before_the_file_exists() {
        let dir = sandbox("pre-notice");
        let mut s = PinState::default();
        s.rendered.insert(1, Frame::new(8, 4).expect("frame"));
        let total = 8 * 4;

        let png = dir.join("pin.png");
        assert!(
            s.remind_before_save(1, &png.to_string_lossy(), "")
                .is_empty(),
            "PNG was told it loses pixels"
        );
        assert_eq!(s.pre_notice, "");
        assert!(!png.exists(), "a reminder wrote a file");

        let jpg = dir.join("pin.jpg");
        let pre = s.remind_before_save(1, &jpg.to_string_lossy(), "");
        assert!(pre.contains(&format!("贴图里有 {total} px 透明")), "{pre}");
        assert!(pre.contains("将被填成白色"), "{pre}");
        assert!(pre.contains("存成 PNG"), "{pre}");
        assert!(!jpg.exists(), "the reminder itself wrote {jpg:?}");
        assert_eq!(s.pre_notice, pre, "窗口那一格与返回值不是同一句");

        let said = s.save_image(1, &jpg.to_string_lossy(), "");
        assert!(
            jpg.exists(),
            "the write did not follow the notice: {said:?}"
        );
        let post = pre.replace("将被填成白色", "已填成白色");
        let said = said.expect("the notice was followed by a refusal");
        assert!(
            said.ends_with(&post),
            "两句话差的不止时态：前 {pre:?} 后 {said:?}"
        );

        // A name this app cannot give a format answers nothing, and takes the earlier
        // sentence off the window instead of leaving a JPG warning about a PNG.
        let odd = dir.join("pin.qqq");
        assert!(s
            .remind_before_save(1, &odd.to_string_lossy(), "")
            .is_empty());
        assert_eq!(
            s.pre_notice, "",
            "a refused name kept a stale notice on the window"
        );
        assert!(!odd.exists());

        // The same holds for a pin that never drew: the refusal is the window's answer,
        // not last write's.
        s.pre_notice = "a notice from somewhere else".to_string();
        assert!(s
            .remind_before_save(99, &jpg.to_string_lossy(), "")
            .is_empty());
        assert_eq!(
            s.pre_notice, "",
            "an undrawn pin kept someone else's notice"
        );
        assert!(s.save_image(99, &jpg.to_string_lossy(), "").is_err());
        assert_eq!(s.pre_notice, "", "a refused save left a notice behind");
    }
}
