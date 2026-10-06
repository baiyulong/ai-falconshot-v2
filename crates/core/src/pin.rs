//! The pin: what is on screen after a capture (§5.8), how it is manipulated
//! (§5.9), how several of them behave as a set (§5.10–§5.12), and how the whole
//! set survives a crash (§6.3, §7.2).
//!
//! Three rules hold the module together:
//!
//! * **No window, no screen.** A [`PinItem`] is numbers. `PinWindow.qml` binds to
//!   them and a shim pushes the window-style flags; nothing here reads a display,
//!   so every rule in §5.9 is unit-testable without one. The screen layout a pin
//!   has to respect arrives as a [`Desktop`].
//! * **Zoom is the intent, size is the fact.** `zoom` is what the user asked for,
//!   `size_phys` is what the window is. Every setter that can change one routes
//!   through [`PinItem::sync_size`], so the two can never disagree — which is what
//!   lets §5.9.3 take a typed-in size and hand back a zoom.
//! * **A pin must be able to point at pixels.** Per plan §6.3/line "R13 兜底",
//!   grabbing our own window off the screen is not something we can promise, so
//!   "copy this pin", "save this pin" and "show this pin after a restart" all read
//!   [`ImageRef`] and flatten in Rust.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{AlphaBg, Pin as PinConfig};
use crate::encode::{self, EncodeOptions, Format};
use crate::frame::{Frame, FrameError};
use crate::geometry::{self, PhysPoint, PhysRect, PhysSize};
use crate::imageops;

pub type PinId = u64;
pub type GroupId = u64;

/// §5.9.2 "合理的最小和最大缩放范围".
pub const ZOOM_MIN: u32 = 10;
pub const ZOOM_MAX: u32 = 800;
/// The wheel and `+`/`-` move between these rather than by a fixed increment: a
/// 10% step at 800% is a jump, at 10% it is invisible. 100 is on the ladder, so
/// stepping up or down from there is predictable and "reset" has a rung to land on.
pub const ZOOM_STEPS: &[u32] = &[
    10, 15, 20, 25, 33, 50, 67, 75, 90, 100, 110, 125, 150, 200, 300, 400, 500, 600, 800,
];
/// §5.9.4: the floor is the "still findable" guarantee. Below it a pin is a
/// rectangle the user can no longer click, and the requirement explicitly asks
/// for a way back — the floor is the cheapest way to keep that promise.
pub const OPACITY_MIN: u32 = 20;
pub const OPACITY_STEPS: &[u32] = &[20, 30, 40, 50, 60, 70, 80, 90, 100];
/// §5.9.1 "至少保留一部分窗口在可见区域".
pub const MIN_VISIBLE_PX: u32 = 24;
/// §5.9.10's "预设尺寸" default; the menu may pass a different box.
pub const THUMB_FIXED_DEFAULT: PhysSize = PhysSize::new(240, 240);
/// §5.9.15 checkerboard cell edge, in window pixels.
pub const CHECKER_CELL_PX: u32 = 8;
/// §5.9.14: speed is a percentage of the GIF's own frame delays.
pub const GIF_SPEED_STEPS: &[u32] = &[25, 50, 75, 100, 150, 200, 300, 400];
pub const STATE_FILE: &str = "pins.state.toml";
/// Where a pin with no file behind it gets its pixels written, so §7.2 can
/// actually restore it. Same data directory as `history.db`.
pub const CACHE_DIR: &str = "pins";
pub const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum PinError {
    #[error("a pin needs a picture with at least one pixel")]
    EmptyImage,
    #[error("{0} is not inside the picture ({1})")]
    Outside(PhysRect, PhysRect),
    #[error("the picture is {0}, this pin was made from {1}")]
    WrongSize(PhysSize, PhysSize),
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("not in thumbnail mode")]
    NoThumbnail,
    #[error("no GIF frames on this pin")]
    NoGif,
    #[error("type a width, a height, or both")]
    NoSize,
    #[error("a group needs a name")]
    EmptyGroupName,
    #[error("there is already a group called {0:?}")]
    DuplicateGroup(String),
    #[error("no such group: {0}")]
    NoSuchGroup(GroupId),
    #[error("no such pin: {0}")]
    NoSuchPin(PinId),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec: {0}")]
    Encode(#[from] crate::encode::EncodeError),
    #[error("frame: {0}")]
    Frame(#[from] FrameError),
    #[error("state file: {0}")]
    State(String),
}

/// One rung of a step ladder above or below `now`. A value between two rungs
/// snaps in the direction of the step; past the ends it stays where it is.
fn step_on(ladder: &[u32], now: u32, up: bool) -> u32 {
    let Some(edge) = (if up {
        ladder.iter().copied().find(|&v| v > now)
    } else {
        ladder.iter().copied().rev().find(|&v| v < now)
    }) else {
        return now;
    };
    if up {
        edge.max(now)
    } else {
        edge.min(now)
    }
}

pub fn clamp_zoom(pct: u32) -> u32 {
    pct.clamp(ZOOM_MIN, ZOOM_MAX)
}

/// §5.9.4 — `0` means "off", which the caller reads as fully transparent;
/// anything else keeps the pin findable.
pub fn clamp_opacity(pct: u32) -> u32 {
    pct.min(100).max(if pct == 0 { 0 } else { OPACITY_MIN })
}

/// The two limits of a clamp, ordered. A pin can be wider than a desktop.
fn ord(a: i32, b: i32) -> (i32, i32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Where the pixels are. Plan §6.3 calls this `image_ref`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageRef {
    /// Already in the history store (§6.2): `path` is relative to the data
    /// directory and `record` lets "re-pin from history" find the row again.
    History { record: i64, path: String },
    /// Pinned straight from a picture file (§5.8.3).
    File(PathBuf),
    /// Written by a snapshot for a pin that had no file of its own (§7.2).
    Cache(PathBuf),
    /// Clipboard or a fresh capture: in memory only until the next snapshot.
    Volatile,
}

/// The same thing as it appears in `pins.state.toml`. Kept separate from
/// [`ImageRef`] because a `PathBuf` has no honest TOML spelling on Windows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageTag {
    #[default]
    Volatile,
    History {
        record: i64,
        /// Relative to the data directory.
        path: String,
    },
    File {
        /// Absolute, as the user pointed at it.
        path: String,
    },
    Cache {
        path: String,
    },
}

impl ImageRef {
    fn tag(&self) -> ImageTag {
        match self {
            ImageRef::Volatile => ImageTag::Volatile,
            ImageRef::History { record, path } => ImageTag::History {
                record: *record,
                path: path.clone(),
            },
            ImageRef::File(p) => ImageTag::File {
                path: p.to_string_lossy().to_string(),
            },
            ImageRef::Cache(p) => ImageTag::Cache {
                path: p.to_string_lossy().to_string(),
            },
        }
    }

    fn from_tag(tag: &ImageTag) -> Self {
        match tag {
            ImageTag::Volatile => ImageRef::Volatile,
            ImageTag::History { record, path } => ImageRef::History {
                record: *record,
                path: path.clone(),
            },
            ImageTag::File { path } => ImageRef::File(PathBuf::from(path)),
            ImageTag::Cache { path } => ImageRef::Cache(PathBuf::from(path)),
        }
    }

    /// The file to read for these pixels, relative to the data directory. A
    /// path that is already absolute stays absolute.
    pub fn resolve(&self, dir: &Path) -> Option<PathBuf> {
        let raw = match self {
            ImageRef::Volatile => return None,
            ImageRef::History { path, .. } => PathBuf::from(path),
            ImageRef::Cache(path) | ImageRef::File(path) => path.clone(),
        };
        Some(if raw.is_absolute() {
            raw
        } else {
            dir.join(raw)
        })
    }
}

/// §5.9.10/§5.9.11 — plan §6.3 spells this `(mode, rect)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThumbnailMode {
    /// The whole picture, fitted into a preset box. `rect` is that box.
    #[default]
    Fixed,
    /// Only the region the user dragged out. `rect` is that region, in picture
    /// coordinates, and it is also the window size (§5.9.11 "以用户拖动范围确定缩略图大小").
    Free,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thumbnail {
    pub mode: ThumbnailMode,
    pub rect: PhysRect,
    /// The zoom to hand back on the second press. Size itself is derived (§5.9.2),
    /// so this is all "恢复之前尺寸" needs.
    pub prior_zoom: u32,
}

/// §5.9.14. The player is the UI's clock; this is only what it shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayState {
    pub frames: usize,
    pub frame: usize,
    /// Percent of the file's own pace (§5.9.14 "调整播放速度").
    pub speed: u32,
    pub playing: bool,
}

impl PlayState {
    pub fn new(frames: usize) -> Self {
        Self {
            frames,
            frame: 0,
            speed: 100,
            playing: frames > 1,
        }
    }

    /// §5.9.14 上一帧/下一帧. Stepping is an inspection action, so it pauses.
    pub fn step(&mut self, dir: i32) {
        if self.frames < 2 {
            return;
        }
        self.playing = false;
        let n = self.frames as i64;
        let cur = self.frame as i64;
        let next = if dir < 0 { cur - 1 } else { cur + 1 };
        self.frame = next.rem_euclid(n) as usize;
    }

    pub fn first_frame(&mut self) {
        self.frame = 0;
    }

    pub fn toggle_play(&mut self) -> bool {
        if self.frames > 1 {
            self.playing = !self.playing;
        }
        self.playing
    }

    pub fn set_speed(&mut self, pct: u32) {
        self.speed = pct.clamp(1, 1000);
    }

    pub fn speed_step(&mut self, up: bool) {
        self.speed = step_on(GIF_SPEED_STEPS, self.speed, up);
    }
}

/// §5.9.1 — the screen layout a pin is allowed to be put on. A plain list of
/// rectangles, because a state machine must not need a display server to be
/// tested. `bounds` is the union of the monitors, in physical pixels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Desktop {
    pub bounds: PhysRect,
    pub monitors: Vec<PhysRect>,
}

impl Desktop {
    pub fn new(bounds: PhysRect, monitors: Vec<PhysRect>) -> Self {
        Self { bounds, monitors }
    }

    pub fn from_monitors<'a>(rects: impl Iterator<Item = &'a PhysRect>) -> Self {
        let monitors: Vec<PhysRect> = rects.copied().collect();
        let bounds = monitors
            .iter()
            .cloned()
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default();
        Self { bounds, monitors }
    }

    /// Slide `r` back so at least [`MIN_VISIBLE_PX`] of it stays on the desktop
    /// in each axis. A rectangle that already qualifies comes back untouched,
    /// which is what makes a cross-monitor drag legal (§5.9.1).
    pub fn keep_visible(&self, r: &PhysRect) -> PhysRect {
        let d = self.bounds;
        if d.is_empty() {
            return *r;
        }
        let min = MIN_VISIBLE_PX.min(r.w).min(r.h) as i32;
        // The pin keeps `min` pixels at each edge. A window wider than the
        // desktop inverts the two limits, and `clamp` panics on an inverted
        // range, so the pair is ordered before use.
        let (lo_x, hi_x) = ord(d.x + min - r.w as i32, d.right() - min);
        let (lo_y, hi_y) = ord(d.y + min - r.h as i32, d.bottom() - min);
        r.offset(r.x.clamp(lo_x, hi_x) - r.x, r.y.clamp(lo_y, hi_y) - r.y)
    }

    pub fn monitor_at(&self, p: PhysPoint) -> Option<usize> {
        self.monitors.iter().position(|m| m.contains(p))
    }

    /// §8.5: a monitor was unplugged or the resolution changed, so a remembered
    /// position may now be off the edge. Returns true when it had to move.
    pub fn refits(&self, r: &PhysRect) -> bool {
        self.keep_visible(r) != *r
    }
}

/// §5.9.15: the transparent pixels are either not clickable, clickable but
/// invisible, or shown against a checkerboard. The pattern is decided here so
/// QML has one place to copy.
pub fn checker_is_light(x: u32, y: u32, cell: u32) -> bool {
    let cell = cell.max(1);
    (x / cell + y / cell).is_multiple_of(2)
}

/// The two squares of the board, `[r,g,b,a]`; `None` for the modes that draw no
/// board at all.
pub fn checker_colors(mode: AlphaBg) -> Option<([u8; 4], [u8; 4])> {
    match mode {
        AlphaBg::CheckerDark => Some(([0x2a, 0x2a, 0x2a, 0xff], [0x3f, 0x3f, 0x3f, 0xff])),
        AlphaBg::CheckerLight => Some(([0xd4, 0xd4, 0xd4, 0xff], [0xef, 0xef, 0xef, 0xff])),
        AlphaBg::Transparent | AlphaBg::Pseudo => None,
    }
}

/// One pin, and every piece of state §5.9 says it has.
#[derive(Clone, Debug)]
pub struct PinItem {
    pub id: PinId,
    pub image: ImageRef,
    /// The size of the bitmap `image` points at. `src_rect` always fits inside
    /// it, and "恢复原图" (§5.9.9) means putting `src_rect` back to it.
    pub source_size: PhysSize,
    /// §5.9.9 — the crop. A region of the source, never a resample.
    pub src_rect: PhysRect,
    pub pos_phys: PhysPoint,
    pub size_phys: PhysSize,
    pub zoom: u32,
    pub opacity: u32,
    /// Degrees, always a multiple of 90 (§5.9.5), normalised to 0..=270.
    pub rotation: i32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub grayscale: bool,
    pub inverted: bool,
    pub topmost: bool,
    pub click_through: bool,
    pub group_id: Option<GroupId>,
    /// §5.11 — hidden by Solo. Separate from `group_hidden` because §5.12.4 hides
    /// a whole group and the two exits must not cancel each other out.
    pub solo_excluded: bool,
    pub group_hidden: bool,
    pub alpha_bg_mode: AlphaBg,
    /// §5.9.2 "像素级 vs 平滑". `false` is nearest neighbour.
    pub smooth_zoom: bool,
    pub thumbnail: Option<Thumbnail>,
    pub gif: Option<PlayState>,
    /// §5.9.9 "支持撤销裁剪": the crops still waiting to be undone, newest last.
    pub crop_undo: Vec<PhysRect>,
}

impl PinItem {
    /// §5.8.1 — a new pin, 1:1 at the configured defaults, top-left at `at`.
    pub fn new(
        id: PinId,
        image: ImageRef,
        source: PhysSize,
        at: PhysPoint,
        cfg: &PinConfig,
    ) -> Result<Self, PinError> {
        if source.is_empty() {
            return Err(PinError::EmptyImage);
        }
        let src_rect = source.at(PhysPoint::new(0, 0));
        let mut p = Self {
            id,
            image,
            source_size: source,
            src_rect,
            pos_phys: at,
            size_phys: source,
            zoom: clamp_zoom(cfg.default_zoom),
            opacity: clamp_opacity(cfg.default_opacity),
            rotation: 0,
            flip_h: false,
            flip_v: false,
            grayscale: false,
            inverted: false,
            topmost: cfg.default_topmost,
            click_through: cfg.default_click_through,
            group_id: None,
            solo_excluded: false,
            group_hidden: false,
            alpha_bg_mode: cfg.alpha_bg,
            smooth_zoom: true,
            thumbnail: None,
            gif: None,
            crop_undo: Vec::new(),
        };
        p.sync_size();
        Ok(p)
    }

    pub fn from_frame(
        id: PinId,
        frame: &Frame,
        at: PhysPoint,
        cfg: &PinConfig,
    ) -> Result<Self, PinError> {
        Self::new(
            id,
            ImageRef::Volatile,
            PhysSize::new(frame.width, frame.height),
            at,
            cfg,
        )
    }

    /// What part of the picture is on show: the free-thumbnail region when there
    /// is one (§5.9.11), otherwise the crop.
    pub fn visible_rect(&self) -> PhysRect {
        match self.thumbnail {
            Some(t) if t.mode == ThumbnailMode::Free => t.rect,
            _ => self.src_rect,
        }
    }

    /// The shown region in the source picture's own pixels.
    pub fn natural_size(&self) -> PhysSize {
        self.visible_rect().size()
    }

    pub fn quarter_turned(&self) -> bool {
        matches!(self.turns(), 1 | 3)
    }

    pub fn turns(&self) -> i32 {
        (self.rotation / 90).rem_euclid(4)
    }

    /// The shown region after rotation, in screen pixels at zoom 100.
    pub fn shown_size(&self) -> PhysSize {
        let n = self.natural_size();
        if self.quarter_turned() {
            n.quarter_turned()
        } else {
            n
        }
    }

    /// The size the window has to be for the current zoom.
    pub fn scaled_size(&self) -> PhysSize {
        let s = self.shown_size();
        let z = self.zoom.max(1) as f64 / 100.0;
        PhysSize::new(
            (s.w as f64 * z).round().max(1.0) as u32,
            (s.h as f64 * z).round().max(1.0) as u32,
        )
    }

    /// The one place `size_phys` is written: keep it derived or the pin lies.
    pub fn sync_size(&mut self) {
        self.size_phys = self.scaled_size();
    }

    pub fn window_rect(&self) -> PhysRect {
        self.size_phys.at(self.pos_phys)
    }

    pub fn visible(&self) -> bool {
        !self.solo_excluded && !self.group_hidden
    }

    /// §5.9.1 — absolute move, clamped so the pin cannot be lost. Returns the
    /// position actually taken, which is what the drag handler should keep.
    pub fn move_to(&mut self, at: PhysPoint, desk: &Desktop) -> PhysPoint {
        let r = desk.keep_visible(&PhysRect::new(
            at.x,
            at.y,
            self.size_phys.w,
            self.size_phys.h,
        ));
        self.pos_phys = r.top_left();
        self.pos_phys
    }

    pub fn move_by(&mut self, dx: i32, dy: i32, desk: &Desktop) -> PhysPoint {
        let at = self.pos_phys.translate(dx, dy);
        self.move_to(at, desk)
    }

    /// §5.9.2 `at` is the point that must stay under the cursor; `None` anchors
    /// on the centre, which is what `+`/`-` want.
    pub fn set_zoom_about(&mut self, pct: u32, at: Option<PhysPoint>) {
        let pct = clamp_zoom(pct);
        let before = self.zoom.max(1);
        if pct == before {
            return;
        }
        let anchor = at.unwrap_or_else(|| self.window_rect().center());
        let moved = geometry::scale_about(&self.window_rect(), pct as f64 / before as f64, anchor);
        self.zoom = pct;
        self.sync_size();
        self.pos_phys = moved.top_left();
    }

    pub fn set_zoom(&mut self, pct: u32) {
        self.set_zoom_about(pct, None);
    }

    pub fn zoom_step(&mut self, up: bool, at: Option<PhysPoint>) -> u32 {
        let next = step_on(ZOOM_STEPS, self.zoom, up);
        self.set_zoom_about(next, at);
        self.zoom
    }

    pub fn reset_zoom(&mut self) {
        self.set_zoom_about(100, None);
    }

    /// §5.9.3 — the typed numbers are a container and the ratio stays locked:
    /// the largest zoom that keeps the whole picture inside them. One axis alone
    /// is the ratio itself, since the other follows from it.
    pub fn set_size_to(&mut self, w: Option<u32>, h: Option<u32>) -> Result<(), PinError> {
        let shown = self.shown_size();
        let pct = match (w.filter(|v| *v > 0), h.filter(|v| *v > 0)) {
            (Some(w), Some(h)) => Self::zoom_for(w, shown.w).min(Self::zoom_for(h, shown.h)),
            (Some(w), None) => Self::zoom_for(w, shown.w),
            (None, Some(h)) => Self::zoom_for(h, shown.h),
            (None, None) => return Err(PinError::NoSize),
        };
        self.set_zoom(pct.round() as u32);
        Ok(())
    }

    fn zoom_for(want: u32, have: u32) -> f64 {
        want.max(1) as f64 * 100.0 / have.max(1) as f64
    }

    /// §5.9.4. `0` is allowed for one reason only: a pin that is invisible can
    /// still be found again through `reset_opacity`, which is the promise the
    /// requirement makes.
    pub fn set_opacity(&mut self, pct: u32) {
        self.opacity = clamp_opacity(pct);
    }

    pub fn opacity_step(&mut self, up: bool) -> u32 {
        self.opacity = step_on(OPACITY_STEPS, self.opacity, up);
        self.opacity
    }

    pub fn reset_opacity(&mut self) {
        self.set_opacity(100);
    }

    /// §5.9.5 — a quarter turn about the centre, so the pin does not run off to
    /// one side every time the key is pressed.
    pub fn rotate(&mut self, quarter_turns: i32) {
        let r = self.window_rect();
        let centre = r.center();
        self.rotation = (self.rotation + quarter_turns * 90).rem_euclid(360);
        self.sync_size();
        let n = self.window_rect();
        self.pos_phys = PhysPoint::new(centre.x - (n.w as i32) / 2, centre.y - (n.h as i32) / 2);
    }

    pub fn rotate_cw(&mut self) {
        self.rotate(1)
    }

    pub fn rotate_ccw(&mut self) {
        self.rotate(-1)
    }

    /// §5.9.6
    pub fn toggle_flip_h(&mut self) -> bool {
        self.flip_h = !self.flip_h;
        self.flip_h
    }

    pub fn toggle_flip_v(&mut self) -> bool {
        self.flip_v = !self.flip_v;
        self.flip_v
    }

    /// §5.9.7 — "再次执行可恢复原始颜色" is a toggle, not a second operation.
    pub fn toggle_grayscale(&mut self) -> bool {
        self.grayscale = !self.grayscale;
        self.grayscale
    }

    /// §5.9.8
    pub fn toggle_invert(&mut self) -> bool {
        self.inverted = !self.inverted;
        self.inverted
    }

    pub fn set_topmost(&mut self, on: bool) {
        self.topmost = on;
    }

    /// §5.9.13. The window style is the shim's job; the state machine only has
    /// to remember that the user asked for it, because the tray and the hotkey
    /// both read this back to turn it off again.
    pub fn set_click_through(&mut self, on: bool) -> bool {
        self.click_through = on;
        on
    }

    pub fn set_alpha_bg_mode(&mut self, mode: AlphaBg) {
        self.alpha_bg_mode = mode;
    }

    /// §5.9.9 — crop in the picture's own coordinates. Nothing is resampled, so
    /// the retained region keeps its resolution.
    pub fn crop(&mut self, region: &PhysRect) -> Result<(), PinError> {
        let whole = self.whole_rect();
        if region.is_empty() || region.intersection(&whole).is_none() {
            return Err(PinError::Outside(*region, whole));
        }
        let inside = region
            .intersection(&whole)
            .filter(|r| !r.is_empty())
            .ok_or(PinError::Outside(*region, whole))?;
        if inside == whole && !self.src_rect.is_empty() {
            // Taking the whole picture "back" through the crop tool is not a
            // crop; recording it would make undo go backwards through nothing.
            return Ok(());
        }
        self.crop_undo.push(self.src_rect);
        self.src_rect = inside;
        self.sync_size();
        Ok(())
    }

    /// §5.9.9/§5.9.11 — a rectangle the user dragged on the window, in screen
    /// pixels, mapped back through zoom, thumbnail and rotation into the
    /// picture's own coordinates.
    fn window_rect_to_source(&self, rect: &PhysRect) -> Result<PhysRect, PinError> {
        let a = self.window_to_source(rect.top_left());
        let b = self.window_to_source(PhysPoint::new(rect.right() - 1, rect.bottom() - 1));
        let (Some(a), Some(b)) = (a, b) else {
            return Err(PinError::Outside(*rect, self.window_rect()));
        };
        // `from_points` spans the two samples, and both samples are inside the
        // region being kept, so the region is one pixel wider than the span.
        let r = PhysRect::from_points(a, b);
        Ok(PhysRect::new(
            r.x,
            r.y,
            r.w.saturating_add(1),
            r.h.saturating_add(1),
        ))
    }

    pub fn crop_window(&mut self, rect: &PhysRect) -> Result<(), PinError> {
        let r = self.window_rect_to_source(rect)?;
        self.crop(&r)
    }

    pub fn undo_crop(&mut self) -> Result<(), PinError> {
        let prev = self.crop_undo.pop().ok_or(PinError::NothingToUndo)?;
        self.src_rect = prev;
        self.sync_size();
        Ok(())
    }

    /// §5.9.9 "或恢复原图".
    pub fn restore_original(&mut self) -> bool {
        let whole = self.whole_rect();
        if self.src_rect == whole && self.crop_undo.is_empty() {
            return false;
        }
        self.crop_undo.clear();
        self.src_rect = whole;
        self.sync_size();
        true
    }

    /// The uncropped extent of the picture the pin was made from.
    pub fn whole_rect(&self) -> PhysRect {
        self.source_size.at(PhysPoint::new(0, 0))
    }

    /// §5.9.10 — a second press gives the size back.
    pub fn toggle_thumbnail(&mut self, box_size: PhysSize) -> bool {
        if self.thumbnail.map(|t| t.mode) == Some(ThumbnailMode::Fixed) {
            let _ = self.exit_thumbnail();
            return false;
        }
        self.enter_fixed_thumbnail(box_size);
        true
    }

    pub fn enter_fixed_thumbnail(&mut self, box_size: PhysSize) {
        let prior_zoom = self.zoom;
        let box_ = if box_size.is_empty() {
            THUMB_FIXED_DEFAULT
        } else {
            box_size
        };
        self.thumbnail = Some(Thumbnail {
            mode: ThumbnailMode::Fixed,
            rect: box_.at(PhysPoint::new(0, 0)),
            prior_zoom,
        });
        let s = self.shown_size();
        let f = (box_.w as f64 / s.w.max(1) as f64).min(box_.h as f64 / s.h.max(1) as f64);
        self.set_zoom((f * 100.0).round() as u32);
        // A thumbnail changes the shown region, and `set_zoom` is a no-op when the
        // percentage happens to be the one already in use - so the size is
        // re-derived here rather than left to the zoom path.
        self.sync_size();
    }

    /// §5.9.11 — `region` is in picture coordinates. Pressing it again with a
    /// new region adjusts the thumbnail instead of stacking a second one.
    pub fn set_free_thumbnail(&mut self, region: &PhysRect) -> Result<(), PinError> {
        let whole = self.src_rect;
        let inside = region
            .intersection(&whole)
            .filter(|r| !r.is_empty())
            .ok_or(PinError::Outside(*region, whole))?;
        let prior_zoom = match self.thumbnail {
            Some(t) => t.prior_zoom,
            None => self.zoom,
        };
        self.thumbnail = Some(Thumbnail {
            mode: ThumbnailMode::Free,
            rect: inside,
            prior_zoom,
        });
        self.set_zoom(100);
        self.sync_size();
        Ok(())
    }

    /// §5.9.11 step 1 — the right-drag rectangle, arriving in screen pixels like
    /// a crop's.
    pub fn free_thumbnail_window(&mut self, rect: &PhysRect) -> Result<(), PinError> {
        let r = self.window_rect_to_source(rect)?;
        self.set_free_thumbnail(&r)
    }

    pub fn exit_thumbnail(&mut self) -> Result<(), PinError> {
        let t = self.thumbnail.take().ok_or(PinError::NoThumbnail)?;
        self.set_zoom(t.prior_zoom);
        Ok(())
    }

    /// §5.9.14 — a decoded GIF arrived, so the player exists now.
    pub fn set_gif(&mut self, frames: usize) -> Option<&mut PlayState> {
        if frames == 0 {
            self.gif = None;
        } else {
            self.gif = Some(PlayState::new(frames));
        }
        self.gif.as_mut()
    }

    pub fn set_smooth_zoom(&mut self, smooth: bool) {
        self.smooth_zoom = smooth;
    }

    /// §5.10.4 "重置": the way the pin is being looked at, not what it is. The
    /// crop survives because cropping is content, and the position survives
    /// because a reset is not a move — the pin grows or shrinks from its own
    /// top-left rather than sliding somewhere else.
    pub fn reset_view(&mut self, cfg: &PinConfig) {
        self.rotation = 0;
        self.flip_h = false;
        self.flip_v = false;
        self.grayscale = false;
        self.inverted = false;
        self.thumbnail = None;
        self.smooth_zoom = true;
        self.click_through = false;
        self.topmost = cfg.default_topmost;
        self.opacity = clamp_opacity(cfg.default_opacity);
        self.zoom = clamp_zoom(cfg.default_zoom);
        self.sync_size();
    }

    /// The window point back to the picture point, undoing thumbnail, zoom,
    /// rotation and flips. Flips go first because the QML `Transform` list is
    /// composed in that order: scale innermost, then rotation.
    pub fn window_to_source(&self, at: PhysPoint) -> Option<PhysPoint> {
        let win = self.window_rect();
        if !win.contains(at) {
            return None;
        }
        let (wx, wy) = (win.w as f64, win.h as f64);
        let ox = (at.x - win.x) as f64 + 0.5 - wx / 2.0;
        let oy = (at.y - win.y) as f64 + 0.5 - wy / 2.0;
        let (rx, ry) = match self.turns() {
            1 => (oy, -ox),
            2 => (-ox, -oy),
            3 => (-oy, ox),
            _ => (ox, oy),
        };
        let (cwx, cwy) = if self.quarter_turned() {
            (wy, wx)
        } else {
            (wx, wy)
        };
        let mut ux = rx + cwx / 2.0;
        let mut uy = ry + cwy / 2.0;
        if self.flip_h {
            ux = cwx - ux;
        }
        if self.flip_v {
            uy = cwy - uy;
        }
        let z = self.zoom.max(1) as f64 / 100.0;
        let n = self.natural_size();
        let v = self.visible_rect();
        let i = ((ux / z).floor() as i32).clamp(0, n.w.max(1) as i32 - 1);
        let j = ((uy / z).floor() as i32).clamp(0, n.h.max(1) as i32 - 1);
        Some(PhysPoint::new(v.x + i, v.y + j))
    }

    /// §5.9.15 — whether a point of the window takes the click. `alpha_at`
    /// reads the picture, which lives with the window layer, not here.
    pub fn hit(&self, at: PhysPoint, alpha_at: impl FnOnce(PhysPoint) -> u8) -> bool {
        let source = match self.window_to_source(at) {
            Some(p) => p,
            None => return false,
        };
        match self.alpha_bg_mode {
            AlphaBg::Transparent => alpha_at(source) > 0,
            // Pseudo promises the whole window is clickable; a checkerboard is
            // drawn behind the picture, so there is no hole to click through
            // either.
            AlphaBg::Pseudo | AlphaBg::CheckerDark | AlphaBg::CheckerLight => true,
        }
    }

    /// §5.9.16 — the picture as it now stands. Two things are deliberately left
    /// out: `zoom` (copying a pin at 300% would break §5.9.9's promise that the
    /// retained region keeps its own resolution) and the thumbnail modes, which
    /// are a way of looking at the picture rather than a change to it.
    pub fn render(&self, src: &Frame) -> Result<Frame, PinError> {
        let have = PhysSize::new(src.width, src.height);
        if have != self.source_size {
            return Err(PinError::WrongSize(have, self.source_size));
        }
        if self.src_rect.is_empty() {
            return Err(PinError::EmptyImage);
        }
        let mut out = src.crop(&self.src_rect)?;
        if self.flip_h {
            out = out.flipped_horizontal();
        }
        if self.flip_v {
            out = out.flipped_vertical();
        }
        for _ in 0..self.turns() {
            out = out.rotated_90_cw();
        }
        if self.grayscale {
            out = imageops::grayscale(&out);
        }
        if self.inverted {
            out = imageops::invert(&out);
        }
        Ok(out)
    }

    /// What the window shows, which is not always what the picture is: a free
    /// thumbnail (§5.9.11) narrows the shown region without touching the crop,
    /// and `render` deliberately ignores it. Export keeps using [`render`], the
    /// screen has to use this.
    pub fn render_shown(&self, src: &Frame) -> Result<Frame, PinError> {
        let region = self.visible_rect();
        if region == self.src_rect {
            return self.render(src);
        }
        let mut looked = self.clone();
        looked.src_rect = region;
        looked.crop_undo.clear();
        looked.render(src)
    }

    /// §5.9.15 — the board behind a transparent pin. A display step, not a
    /// content step: an export keeps its alpha, only the window needs pixels to
    /// look at where the picture is see-through.
    pub fn paint_alpha_bg(&self, mut out: Frame) -> Frame {
        let Some((light, dark)) = checker_colors(self.alpha_bg_mode) else {
            return out;
        };
        for y in 0..out.height {
            for x in 0..out.width {
                if out.get(x, y)[3] == 0 {
                    let c = if checker_is_light(x, y, CHECKER_CELL_PX) {
                        light
                    } else {
                        dark
                    };
                    out.set(x, y, c);
                }
            }
        }
        out
    }
}

/// §5.12.3 — deleting a group that still has pins in it has to answer "what
/// about the pictures?" (PRD: "应询问贴图如何处理"). The state machine is not
/// allowed to guess, so the caller has to say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupFate {
    /// The pins stay on screen and go back to ungrouped.
    #[default]
    Ungroup,
    /// The pins go with the group.
    Destroy,
}

/// §5.12.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
}

/// §5.11. `saved` is what the hidden pins looked like before Solo, which is the
/// whole reason exiting is not simply "show everything".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Solo {
    pub kept: Vec<PinId>,
    pub saved: Vec<SavedPin>,
}

/// A remembered opacity, kept as a struct rather than a tuple so the TOML file
/// says what it means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPin {
    pub id: PinId,
    pub opacity: u32,
}

/// A pin taken out of the set, with the z slot it left, so §5.9.18's undo can
/// put it back where it was instead of on top of everything.
#[derive(Clone, Debug)]
pub struct PinSlot {
    pub index: usize,
    pub item: PinItem,
}

#[derive(Clone, Debug, Default)]
pub struct PinSet {
    /// z order, bottom first: the last entry is the one a click finds.
    items: Vec<PinItem>,
    next_id: PinId,
    next_group: GroupId,
    selection: BTreeSet<PinId>,
    groups: Vec<Group>,
    solo: Option<Solo>,
}

impl PinSet {
    /// §5.8.1 — the newest pin is on top.
    pub fn spawn(
        &mut self,
        image: ImageRef,
        source: PhysSize,
        at: PhysPoint,
        cfg: &PinConfig,
    ) -> Result<PinId, PinError> {
        let id = self.next_id;
        let item = PinItem::new(id, image, source, at, cfg)?;
        self.next_id = id + 1;
        self.items.push(item);
        Ok(id)
    }

    pub fn add(&mut self, item: PinItem) -> PinId {
        let id = item.id;
        self.next_id = self.next_id.max(id + 1);
        self.items.push(item);
        id
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[PinItem] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut [PinItem] {
        &mut self.items
    }

    pub fn get(&self, id: PinId) -> Option<&PinItem> {
        self.items.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: PinId) -> Option<&mut PinItem> {
        self.items.iter_mut().find(|p| p.id == id)
    }

    pub fn z_order(&self) -> impl Iterator<Item = PinId> + '_ {
        self.items.iter().rev().map(|p| p.id)
    }

    pub fn ids(&self) -> Vec<PinId> {
        self.items.iter().map(|p| p.id).collect()
    }

    /// Topmost first, so the caller can walk the pile.
    pub fn stack_at(
        &self,
        at: PhysPoint,
        accepts: &dyn Fn(&PinItem, PhysPoint) -> bool,
    ) -> Vec<PinId> {
        self.items
            .iter()
            .filter(|p| p.visible() && accepts(p, at))
            .rev()
            .map(|p| p.id)
            .collect()
    }

    /// A click that lands on nothing clears the selection (§5.10.1 "点击空白区域").
    /// `cycle` is §5.10.4's `cycle_on_click`: instead of grabbing the top pin of
    /// a pile every time, take the one buried under it and bring it up.
    pub fn click_at(
        &mut self,
        at: PhysPoint,
        cycle: bool,
        accepts: &dyn Fn(&PinItem, PhysPoint) -> bool,
    ) -> Option<PinId> {
        let stack = self.stack_at(at, accepts);
        if stack.is_empty() {
            self.clear_selection();
            return None;
        }
        let id = if cycle && stack.len() > 1 {
            *stack.last().unwrap_or(&stack[0])
        } else {
            stack[0]
        };
        self.raise(id);
        // The id came out of `stack_at`, so `select` cannot fail on it.
        let _ = self.select(id, false);
        Some(id)
    }

    /// §5.10.1 — plain click selects one, Ctrl+click adds or takes away.
    pub fn select(&mut self, id: PinId, add: bool) -> Result<(), PinError> {
        if self.get(id).is_none() {
            return Err(PinError::NoSuchPin(id));
        }
        if add {
            if !self.selection.remove(&id) {
                self.selection.insert(id);
            }
        } else {
            self.selection.clear();
            self.selection.insert(id);
        }
        Ok(())
    }

    pub fn select_all(&mut self) {
        self.selection = self.ids().into_iter().collect();
    }

    pub fn set_selection(&mut self, ids: &[PinId]) {
        self.selection = ids.iter().copied().collect();
        // Selecting a pin that is not there is a stale menu, not a bug worth a
        // panic; silently dropping it keeps the selection honest.
        self.selection
            .retain(|id| self.items.iter().any(|p| p.id == *id));
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// §5.12.4 "切换" — a group whose pins are all shown becomes hidden and back.
    pub fn toggle_selection(&mut self, id: PinId) -> bool {
        if self.selection.remove(&id) {
            false
        } else {
            self.selection.insert(id);
            true
        }
    }

    pub fn is_selected(&self, id: PinId) -> bool {
        self.selection.contains(&id)
    }

    pub fn selection(&self) -> &BTreeSet<PinId> {
        &self.selection
    }

    pub fn selected_ids(&self) -> Vec<PinId> {
        self.selection.iter().copied().collect()
    }

    pub fn selection_count(&self) -> usize {
        self.selection.len()
    }

    /// §5.9.12: the flags are per pin, and a batch menu only has to set them.
    pub fn raise(&mut self, id: PinId) -> bool {
        let Some(i) = self.items.iter().position(|p| p.id == id) else {
            return false;
        };
        let item = self.items.remove(i);
        self.items.push(item);
        true
    }

    /// Used by the tray list's "置底".
    pub fn lower(&mut self, id: PinId) -> bool {
        let Some(i) = self.items.iter().position(|p| p.id == id) else {
            return false;
        };
        let item = self.items.remove(i);
        self.items.insert(0, item);
        true
    }

    /// §5.10.2 — the selection goes to the top as a block, keeping the relative
    /// order it already had. Raising from the bottom is what does it: each raise
    /// puts one pin above everything, so the last one raised ends up on top.
    pub fn raise_selection(&mut self) -> usize {
        let ids: Vec<PinId> = self
            .items
            .iter()
            .filter(|p| self.selection.contains(&p.id))
            .map(|p| p.id)
            .collect();
        let mut n = 0;
        for id in ids {
            if self.raise(id) {
                n += 1;
            }
        }
        n
    }

    /// §5.10.2 — one displacement, applied to each selected pin, each clamped on
    /// its own: a batch move across a monitor edge must not fling the second pin
    /// off the screen just because the first one had room.
    pub fn translate_selection(&mut self, dx: i32, dy: i32, desk: &Desktop) -> usize {
        let ids = self.selected_ids();
        let mut n = 0;
        for id in ids {
            if let Some(p) = self.get_mut(id) {
                p.move_by(dx, dy, desk);
                n += 1;
            }
        }
        n
    }

    /// §5.10.3.
    pub fn set_selection_opacity(&mut self, pct: u32) -> usize {
        let ids = self.selected_ids();
        let mut n = 0;
        for id in ids {
            if let Some(p) = self.get_mut(id) {
                p.set_opacity(pct);
                n += 1;
            }
        }
        n
    }

    /// §5.10.4 "重置".
    pub fn reset_selection(&mut self, cfg: &PinConfig) -> usize {
        let ids = self.selected_ids();
        let mut n = 0;
        for id in ids {
            if let Some(p) = self.get_mut(id) {
                p.reset_view(cfg);
                n += 1;
            }
        }
        n
    }

    /// §5.10.4 "关闭" — off the screen. Where the picture came from is untouched,
    /// so history can pin it again (§5.14.2).
    pub fn close(&mut self, ids: &[PinId]) -> usize {
        let before = self.items.len();
        self.retain(|p| !ids.contains(&p.id));
        before - self.items.len()
    }

    /// §5.9.18 "销毁" — out of the set, and the only way back is what this
    /// returns. A dialog can print `ids.len()` for the "影响数量" rule and hand
    /// the vector to [`PinSet::restore`] for the undo.
    ///
    /// The slots come back in z order, lowest first, so an undo rebuilds the
    /// pile rather than dumping the pins on top of it.
    pub fn destroy(&mut self, ids: &[PinId]) -> Vec<PinSlot> {
        let taken: Vec<PinSlot> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, p)| ids.contains(&p.id))
            .map(|(index, p)| PinSlot {
                index,
                item: p.clone(),
            })
            .collect();
        self.retain(|p| !ids.contains(&p.id));
        taken
    }

    /// The undo of [`PinSet::destroy`], each pin back in the slot it left.
    pub fn restore(&mut self, slots: Vec<PinSlot>) -> usize {
        let mut n = 0;
        for slot in slots {
            if self.get(slot.item.id).is_some() {
                continue;
            }
            let index = slot.index.min(self.items.len());
            self.next_id = self.next_id.max(slot.item.id + 1);
            self.items.insert(index, slot.item);
            n += 1;
        }
        n
    }

    pub fn destroy_count(&self) -> usize {
        self.selection_count()
    }

    fn retain(&mut self, keep: impl Fn(&PinItem) -> bool) {
        let removed: Vec<PinItem> = self.items.iter().filter(|p| !keep(p)).cloned().collect();
        self.items.retain(keep);
        self.after_removal(&removed);
    }

    fn after_removal(&mut self, removed: &[PinItem]) {
        let gone: BTreeSet<PinId> = removed.iter().map(|p| p.id).collect();
        self.selection = self.selection.difference(&gone).copied().collect();
        let Some(solo) = self.solo.clone() else {
            return;
        };
        let live = |id: &PinId| self.items.iter().any(|p| p.id == *id);
        let kept: Vec<PinId> = solo.kept.iter().filter(|id| live(id)).copied().collect();
        let saved: Vec<SavedPin> = solo.saved.iter().filter(|s| live(&s.id)).cloned().collect();
        if kept.is_empty() {
            // Solo over. What it was holding back comes back, rather than being
            // left dimmed by a session nobody can exit any more.
            self.solo = None;
            for s in &saved {
                if let Some(p) = self.get_mut(s.id) {
                    p.solo_excluded = false;
                    p.opacity = s.opacity;
                }
            }
        } else {
            self.solo = Some(Solo { kept, saved });
        }
    }

    /// §5.11 — Solo. Returns how many pins were put out of the way. `0` with no
    /// session means "nothing was selected", which is the case the requirement
    /// does not cover and the UI should refuse instead of dimming the screen.
    pub fn enter_solo(&mut self, cfg: &PinConfig) -> usize {
        if self.solo.is_some() || self.selection.is_empty() {
            return 0;
        }
        let kept = self.selected_ids();
        let dim = if cfg.solo_dim_opacity > 0 {
            Some(cfg.solo_dim_opacity.min(100))
        } else {
            None
        };
        let mut saved = Vec::new();
        let mut n = 0;
        for p in &mut self.items {
            if kept.contains(&p.id) {
                continue;
            }
            saved.push(SavedPin {
                id: p.id,
                opacity: p.opacity,
            });
            match dim {
                Some(o) => p.set_opacity(o),
                None => p.solo_excluded = true,
            }
            n += 1;
        }
        self.solo = Some(Solo { kept, saved });
        n
    }

    /// §5.11 "退出后应恢复原状态，而不是无条件显示全部" — only the pins Solo put
    /// away are touched, and they get their own opacity back.
    pub fn exit_solo(&mut self) -> usize {
        let Some(solo) = self.solo.take() else {
            return 0;
        };
        let mut n = 0;
        for s in &solo.saved {
            if let Some(p) = self.get_mut(s.id) {
                p.solo_excluded = false;
                p.opacity = s.opacity;
                n += 1;
            }
        }
        for p in &mut self.items {
            if solo.kept.contains(&p.id) {
                p.solo_excluded = false;
            }
        }
        n
    }

    pub fn solo_active(&self) -> bool {
        self.solo.is_some()
    }

    pub fn solo_keeps(&self) -> &[PinId] {
        self.solo.as_ref().map(|s| s.kept.as_slice()).unwrap_or(&[])
    }

    /// §5.11 "再次执行 Solo 或退出命令" — the same gesture both ways.
    pub fn toggle_solo(&mut self, cfg: &PinConfig) -> bool {
        if self.solo_active() {
            self.exit_solo();
            false
        } else {
            self.enter_solo(cfg) > 0
        }
    }

    /// §5.12.1. Names are trimmed, cannot be empty, and cannot collide —
    /// identical or, for ASCII names, differing only in case (§5.12.3 asks for
    /// the duplication rule to be defined somewhere; this is that somewhere).
    pub fn create_group(&mut self, name: &str) -> Result<GroupId, PinError> {
        let name = Self::group_name(name)?;
        if self.name_taken(&name, None) {
            return Err(PinError::DuplicateGroup(name));
        }
        let id = self.next_group;
        self.next_group += 1;
        self.groups.push(Group { id, name });
        Ok(id)
    }

    fn name_taken(&self, name: &str, except: Option<GroupId>) -> bool {
        self.groups
            .iter()
            .any(|g| Some(g.id) != except && (g.name == name || g.name.eq_ignore_ascii_case(name)))
    }

    fn group_name(raw: &str) -> Result<String, PinError> {
        let name = raw.trim().to_string();
        if name.is_empty() {
            return Err(PinError::EmptyGroupName);
        }
        Ok(name)
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// §5.12.3 "重命名".
    pub fn rename_group(&mut self, id: GroupId, name: &str) -> Result<(), PinError> {
        let name = Self::group_name(name)?;
        if self.name_taken(&name, Some(id)) {
            return Err(PinError::DuplicateGroup(name));
        }
        let g = self.group_mut(id)?;
        g.name = name;
        Ok(())
    }

    fn group_mut(&mut self, id: GroupId) -> Result<&mut Group, PinError> {
        self.groups
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or(PinError::NoSuchGroup(id))
    }

    /// §5.12.3 "删除" — and the answer to "贴图怎么办" comes from the caller.
    /// Returns how many pins were affected, which is the number a confirmation
    /// dialog should print.
    pub fn delete_group(&mut self, id: GroupId, fate: GroupFate) -> Result<usize, PinError> {
        self.group_mut(id)?;
        let pins = self.pins_in_group(id);
        match fate {
            GroupFate::Ungroup => {
                self.assign_group(&pins, None)?;
            }
            GroupFate::Destroy => {
                self.destroy(&pins);
            }
        }
        self.groups.retain(|g| g.id != id);
        Ok(pins.len())
    }

    /// §5.12.2. `None` means "no group", which is where a pin starts and where
    /// it goes when its group is deleted.
    pub fn assign_group(
        &mut self,
        ids: &[PinId],
        group: Option<GroupId>,
    ) -> Result<usize, PinError> {
        if let Some(g) = group {
            self.group_mut(g)?;
        }
        let mut n = 0;
        for p in &mut self.items {
            if ids.contains(&p.id) {
                p.group_id = group;
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn assign_selection_to_group(&mut self, group: Option<GroupId>) -> Result<usize, PinError> {
        let ids = self.selected_ids();
        self.assign_group(&ids, group)
    }

    pub fn pins_in_group(&self, id: GroupId) -> Vec<PinId> {
        self.items
            .iter()
            .filter(|p| p.group_id == Some(id))
            .map(|p| p.id)
            .collect()
    }

    /// §5.12.3 "拖动排序": the order of `groups` *is* the quick-switch order, so
    /// moving an entry is the whole feature.
    pub fn move_group(&mut self, id: GroupId, delta: i32) -> Result<(), PinError> {
        let from = self
            .groups
            .iter()
            .position(|g| g.id == id)
            .ok_or(PinError::NoSuchGroup(id))?;
        let to = if delta < 0 {
            from.saturating_sub((-delta) as usize)
        } else {
            (from + delta as usize).min(self.groups.len() - 1)
        };
        if to != from {
            let g = self.groups.remove(from);
            self.groups.insert(to, g);
        }
        Ok(())
    }

    /// §5.12.4 — the whole group at once. Hidden through its own flag so exiting
    /// Solo cannot accidentally reveal it again.
    pub fn set_group_shown(&mut self, id: GroupId, shown: bool) -> Result<usize, PinError> {
        self.group_mut(id)?;
        let mut n = 0;
        for p in &mut self.items {
            if p.group_id == Some(id) {
                p.group_hidden = !shown;
                n += 1;
            }
        }
        Ok(n)
    }

    /// True when every pin in the group is hidden; an empty group counts as shown,
    /// because there is nothing to have put away.
    pub fn group_shown(&self, id: GroupId) -> bool {
        let pins: Vec<&PinItem> = self
            .items
            .iter()
            .filter(|p| p.group_id == Some(id))
            .collect();
        !pins.iter().any(|p| p.group_hidden)
    }

    pub fn toggle_group_shown(&mut self, id: GroupId) -> Result<bool, PinError> {
        let shown = self.group_shown(id);
        self.set_group_shown(id, !shown)?;
        Ok(!shown)
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub fn group_ids(&self) -> Vec<GroupId> {
        self.groups.iter().map(|g| g.id).collect()
    }

    /// §8.5 — after a screen change: pull every pin back onto the desktop.
    pub fn refit_all(&mut self, desk: &Desktop) -> usize {
        let mut n = 0;
        for p in &mut self.items {
            let r = desk.keep_visible(&p.window_rect());
            if r.top_left() != p.pos_phys {
                p.pos_phys = r.top_left();
                n += 1;
            }
        }
        n
    }

    pub fn next_id(&self) -> PinId {
        self.next_id
    }

    pub fn next_group_id(&self) -> GroupId {
        self.next_group
    }
}

/// `pins.state.toml` — the §6.3 schema, field for field, so the file is the
/// documentation. `Option`s carry `skip_serializing_if` because TOML has no
/// null: an absent key is how "no group", "no thumbnail" is written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedPin {
    pub id: PinId,
    pub image_ref: ImageTag,
    pub source_size: PhysSize,
    pub src_rect: PhysRect,
    pub pos_phys: PhysPoint,
    pub size_phys: PhysSize,
    pub zoom: u32,
    pub opacity: u32,
    pub rotation: i32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub grayscale: bool,
    pub inverted: bool,
    pub topmost: bool,
    pub click_through: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<GroupId>,
    pub solo_excluded: bool,
    pub group_hidden: bool,
    pub alpha_bg_mode: AlphaBg,
    pub smooth_zoom: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<Thumbnail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gif: Option<PlayState>,
    pub crop_undo: Vec<PhysRect>,
}

impl PersistedPin {
    fn of(pin: &PinItem, image_ref: ImageTag) -> Self {
        Self {
            id: pin.id,
            image_ref,
            source_size: pin.source_size,
            src_rect: pin.src_rect,
            pos_phys: pin.pos_phys,
            size_phys: pin.size_phys,
            zoom: pin.zoom,
            opacity: pin.opacity,
            rotation: pin.rotation,
            flip_h: pin.flip_h,
            flip_v: pin.flip_v,
            grayscale: pin.grayscale,
            inverted: pin.inverted,
            topmost: pin.topmost,
            click_through: pin.click_through,
            group_id: pin.group_id,
            solo_excluded: pin.solo_excluded,
            group_hidden: pin.group_hidden,
            alpha_bg_mode: pin.alpha_bg_mode,
            smooth_zoom: pin.smooth_zoom,
            thumbnail: pin.thumbnail,
            gif: pin.gif,
            crop_undo: pin.crop_undo.clone(),
        }
    }

    fn to_item(&self) -> PinItem {
        PinItem {
            id: self.id,
            image: ImageRef::from_tag(&self.image_ref),
            source_size: self.source_size,
            src_rect: self.src_rect,
            pos_phys: self.pos_phys,
            size_phys: self.size_phys,
            zoom: self.zoom,
            opacity: self.opacity,
            rotation: self.rotation,
            flip_h: self.flip_h,
            flip_v: self.flip_v,
            grayscale: self.grayscale,
            inverted: self.inverted,
            topmost: self.topmost,
            click_through: self.click_through,
            group_id: self.group_id,
            solo_excluded: self.solo_excluded,
            group_hidden: self.group_hidden,
            alpha_bg_mode: self.alpha_bg_mode,
            smooth_zoom: self.smooth_zoom,
            thumbnail: self.thumbnail,
            gif: self.gif,
            crop_undo: self.crop_undo.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateFile {
    pub version: u32,
    pub saved_at: i64,
    pub next_id: PinId,
    pub next_group: GroupId,
    pub selection: Vec<PinId>,
    pub groups: Vec<Group>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solo: Option<Solo>,
    pub pins: Vec<PersistedPin>,
}

/// What one snapshot pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotReport {
    pub pins: usize,
    /// Pins that had no file behind them and were therefore written to `pins/`.
    pub frames_written: usize,
    /// Pins that could not be written; they will not come back after a crash.
    pub frames_failed: Vec<PinId>,
}

/// What came back from a snapshot, and why anything did not.
#[derive(Clone, Debug, Default)]
pub struct Restored {
    pub set: PinSet,
    /// The pixels each restored pin needs, in the order the pins were saved.
    pub frames: Vec<(PinId, Frame)>,
    /// Pointed at a file that is gone, or was only ever in memory.
    pub dropped_missing: usize,
    /// The file was there but would not decode (§7.2 must not exit over this).
    pub dropped_corrupt: usize,
    pub version: u32,
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join(STATE_FILE)
}

/// tmp → fsync → rename, the same discipline plan §6.1 sets for `config.toml`:
/// a snapshot written by a half-finished save is worse than no snapshot, because
/// §7.2 would promise a restore it cannot deliver.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), PinError> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    f.as_file_mut().write_all(bytes)?;
    f.commit()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

impl PinSet {
    /// §6.3 "每 5 秒与退出/崩溃时快照". `frames` hands back the live bitmap for
    /// pins that have no file of their own, which is exactly the case §7.2 would
    /// otherwise lose.
    pub fn write_state(
        &self,
        dir: &Path,
        frames: &mut dyn FnMut(PinId) -> Option<Frame>,
        now_ms: i64,
    ) -> Result<SnapshotReport, PinError> {
        let mut pins = Vec::with_capacity(self.items.len());
        let mut report = SnapshotReport::default();
        for pin in &self.items {
            let tag = match &pin.image {
                ImageRef::Volatile => match frames(pin.id) {
                    Some(frame) => {
                        let rel = format!("{}/{}.png", CACHE_DIR, pin.id);
                        let path = dir.join(&rel);
                        let opt = EncodeOptions {
                            format: Format::Png,
                            quality: 92,
                            flatten_on_lossy: false,
                        };
                        match encode::save(&frame, &path, &opt) {
                            Ok(()) => {
                                report.frames_written += 1;
                                ImageTag::Cache { path: rel }
                            }
                            Err(e) => {
                                tracing::warn!("pin {} cache write failed: {e}", pin.id);
                                report.frames_failed.push(pin.id);
                                ImageTag::Volatile
                            }
                        }
                    }
                    None => {
                        report.frames_failed.push(pin.id);
                        ImageTag::Volatile
                    }
                },
                other => other.tag(),
            };
            pins.push(PersistedPin::of(pin, tag));
        }
        report.pins = pins.len();
        let file = StateFile {
            version: SNAPSHOT_VERSION,
            saved_at: now_ms,
            next_id: self.next_id,
            next_group: self.next_group,
            selection: self.selected_ids(),
            groups: self.groups.clone(),
            solo: self.solo.clone(),
            pins,
        };
        let text =
            toml_edit::ser::to_string_pretty(&file).map_err(|e| PinError::State(e.to_string()))?;
        write_atomic(&state_path(dir), text.as_bytes())?;
        let keep: BTreeSet<PinId> = self.items.iter().map(|p| p.id).collect();
        prune_cache(dir, &keep)?;
        Ok(report)
    }

    /// None when there is no snapshot — a first run, or a clean start after the
    /// user closed everything.
    pub fn read_state(dir: &Path) -> Result<Option<Restored>, PinError> {
        let path = state_path(dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        let file: StateFile =
            toml_edit::de::from_str(&text).map_err(|e| PinError::State(e.to_string()))?;
        if file.version > SNAPSHOT_VERSION {
            // Same rule as the history database: a file from a newer build is
            // refused, never rewritten over.
            return Err(PinError::State(format!(
                "state file is version {}, this build writes {}",
                file.version, SNAPSHOT_VERSION
            )));
        }
        let mut set = PinSet {
            next_id: file.next_id,
            next_group: file.next_group,
            groups: file.groups.clone(),
            ..PinSet::default()
        };
        set.selection = file.selection.iter().copied().collect();
        set.solo = file.solo.clone();
        let mut out = Restored {
            version: file.version,
            ..Restored::default()
        };
        for p in &file.pins {
            let item = p.to_item();
            let Some(path) = item.image.resolve(dir) else {
                out.dropped_missing += 1;
                continue;
            };
            if !path.is_file() {
                out.dropped_missing += 1;
                continue;
            }
            let frame = match encode::decode_file(&path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("pin {} would not decode: {e}", item.id);
                    out.dropped_corrupt += 1;
                    continue;
                }
            };
            // The file may have been replaced by a different picture since.
            let mut item = item;
            if frame.width != item.source_size.w || frame.height != item.source_size.h {
                item.source_size = PhysSize::new(frame.width, frame.height);
                let whole = item.source_size.at(PhysPoint::new(0, 0));
                item.src_rect = item
                    .src_rect
                    .intersection(&whole)
                    .filter(|r| !r.is_empty())
                    .unwrap_or(whole);
            }
            if item.thumbnail.map(|t| t.rect.is_empty()).unwrap_or(false) {
                item.thumbnail = None;
            }
            let id = item.id;
            set.next_id = set.next_id.max(id + 1);
            set.items.push(item);
            out.frames.push((id, frame));
        }
        set.after_removal(&[]);
        set.selection
            .retain(|id| set.items.iter().any(|p| p.id == *id));
        out.set = set;
        Ok(Some(out))
    }

    pub fn has_state(dir: &Path) -> bool {
        state_path(dir).is_file()
    }

    /// Drop the snapshot: used when the user closes every pin and does not want
    /// them back on the next start.
    pub fn clear_state(dir: &Path) -> Result<(), PinError> {
        match std::fs::remove_file(state_path(dir)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(PinError::Io(e)),
        }
        .and_then(|()| prune_cache(dir, &BTreeSet::new()).map(|_| ()))
    }
}

/// §5.9.16/§5.9.17 — the snapshot's own pixels must not outlive the pins they
/// belong to. Only files named `<digits>.png` are ever removed, so anything a
/// user put in the folder by hand stays.
pub fn prune_cache(dir: &Path, keep: &BTreeSet<PinId>) -> Result<usize, PinError> {
    let entries = match std::fs::read_dir(dir.join(CACHE_DIR)) {
        Ok(e) => e,
        Err(_) => return Ok(0),
    };
    let mut gone = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let png = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("png"))
            .unwrap_or(false);
        let Ok(id) = stem.parse::<PinId>() else {
            continue;
        };
        if !png || keep.contains(&id) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => gone += 1,
            Err(e) => tracing::warn!("stale pin cache {}: {e}", path.display()),
        }
    }
    Ok(gone)
}

/// §6.3's cadence, decided without a clock of its own so the caller can drive it
/// from a timer: is a snapshot due at `now_ms`, given the last one and the
/// configured interval? `seconds == 0` disables it, as `[pin] state_save_seconds`
/// promises.
pub fn snapshot_due(last_saved_ms: i64, now_ms: i64, seconds: u32) -> bool {
    if seconds == 0 {
        return false;
    }
    if last_saved_ms == 0 || now_ms < last_saved_ms {
        return true;
    }
    now_ms - last_saved_ms >= seconds as i64 * 1000
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn cfg() -> PinConfig {
        PinConfig::default()
    }

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Frame {
        Frame::filled(w, h, rgba).expect("frame")
    }

    fn screen(w: u32, h: u32) -> Desktop {
        Desktop::new(PhysRect::new(0, 0, w, h), vec![PhysRect::new(0, 0, w, h)])
    }

    /// A 1:1 pin with no file behind it, its top-left at the origin.
    fn pin(w: u32, h: u32) -> PinItem {
        pin_at(7, w, h, PhysPoint::new(0, 0))
    }

    fn pin_at(id: PinId, w: u32, h: u32, at: PhysPoint) -> PinItem {
        PinItem::from_frame(id, &solid(w, h, [10, 20, 30, 255]), at, &cfg()).expect("pin")
    }

    /// `n` pins side by side, ids 1..=n, bottom of the pile first.
    fn pile(n: u64, w: u32, h: u32) -> PinSet {
        let mut s = PinSet::default();
        for i in 0..n {
            s.add(pin_at(i + 1, w, h, PhysPoint::new(0, 0)));
        }
        s
    }

    fn opaque(_: PhysPoint) -> u8 {
        255
    }

    fn body(p: &PinItem, at: PhysPoint) -> bool {
        p.hit(at, opaque)
    }

    fn no_frames(_id: PinId) -> Option<Frame> {
        None
    }

    // ---------------------------------------------------------------- §5.8.1

    #[test]
    fn a_new_pin_is_one_to_one_at_the_configured_defaults() {
        let p = pin(120, 80);
        assert_eq!(p.zoom, 100);
        assert_eq!(p.opacity, 100);
        assert_eq!(p.size_phys, PhysSize::new(120, 80));
        assert_eq!(p.src_rect, PhysRect::new(0, 0, 120, 80));
        assert_eq!(p.pos_phys, PhysPoint::new(0, 0));
        assert!(p.topmost);
        assert!(!p.click_through);
        assert!(p.smooth_zoom);
        assert_eq!(p.alpha_bg_mode, AlphaBg::Transparent);
        assert_eq!(p.image, ImageRef::Volatile);
        assert!(p.visible());
        assert!(p.crop_undo.is_empty());
    }

    #[test]
    fn a_pin_needs_at_least_one_pixel() {
        let e = PinItem::new(
            1,
            ImageRef::Volatile,
            PhysSize::new(0, 10),
            PhysPoint::new(0, 0),
            &cfg(),
        );
        assert!(matches!(e, Err(PinError::EmptyImage)));
    }

    #[test]
    fn the_newest_pin_is_the_one_a_click_finds() {
        let s = pile(3, 20, 20);
        assert_eq!(s.ids(), vec![1, 2, 3]);
        assert_eq!(s.z_order().collect::<Vec<_>>(), vec![3, 2, 1]);
    }

    // ---------------------------------------------------------------- §5.9.1

    #[test]
    fn a_pin_dragged_off_the_edge_is_pulled_back_by_the_minimum() {
        let desk = screen(1000, 600);
        let mut p = pin_at(1, 100, 100, PhysPoint::new(0, 0));
        let taken = p.move_to(PhysPoint::new(-500, -500), &desk);
        assert_eq!(taken, PhysPoint::new(-76, -76));
        assert_eq!(p.pos_phys, taken);
        let r = desk.keep_visible(&p.window_rect());
        assert_eq!(r.right() - desk.bounds.x, MIN_VISIBLE_PX as i32);
        assert_eq!(r.bottom() - desk.bounds.y, MIN_VISIBLE_PX as i32);
    }

    #[test]
    fn a_rect_that_is_already_qualified_is_not_moved_at_all() {
        let desk = screen(1000, 600);
        let r = PhysRect::new(50, 50, 100, 100);
        assert_eq!(desk.keep_visible(&r), r);
        assert!(!desk.refits(&r));
    }

    #[test]
    fn a_window_wider_than_the_desktop_does_not_panic_the_clamp() {
        let desk = screen(100, 100);
        let r = PhysRect::new(-5000, -5000, 9000, 9000);
        let kept = desk.keep_visible(&r);
        assert_eq!(kept.w, 9000);
        assert_eq!(kept.size(), r.size());
    }

    #[test]
    fn a_pin_off_the_desktop_after_a_resolution_change_comes_back() {
        let mut s = pile(2, 100, 100);
        s.get_mut(2).expect("pin").pos_phys = PhysPoint::new(5000, 5000);
        assert_eq!(s.refit_all(&screen(1000, 600)), 1);
        assert_eq!(s.get(2).expect("pin").pos_phys, PhysPoint::new(976, 576));
        assert_eq!(s.get(1).expect("pin").pos_phys, PhysPoint::new(0, 0));
    }

    #[test]
    fn the_monitor_under_a_point_is_the_one_containing_it() {
        let a = PhysRect::new(0, 0, 1000, 600);
        let b = PhysRect::new(1000, 0, 1000, 600);
        let desk = Desktop::from_monitors([a, b].iter());
        assert_eq!(desk.bounds, PhysRect::new(0, 0, 2000, 600));
        assert_eq!(desk.monitor_at(PhysPoint::new(1500, 10)), Some(1));
        assert_eq!(desk.monitor_at(PhysPoint::new(10, 10)), Some(0));
        assert_eq!(desk.monitor_at(PhysPoint::new(3000, 10)), None);
    }

    // ---------------------------------------------------------------- §5.9.2

    #[test]
    fn zooming_at_the_cursor_keeps_that_pixel_under_it() {
        let mut p = pin(100, 50);
        let cursor = PhysPoint::new(20, 10);
        p.set_zoom_about(200, Some(cursor));
        assert_eq!(p.zoom, 200);
        assert_eq!(p.size_phys, PhysSize::new(200, 100));
        assert_eq!(p.pos_phys, PhysPoint::new(-20, -10));
        assert_eq!(p.window_to_source(cursor), Some(cursor));
    }

    #[test]
    fn zoom_is_clamped_at_both_ends_of_the_ladder() {
        assert_eq!(clamp_zoom(5), ZOOM_MIN);
        assert_eq!(clamp_zoom(5000), ZOOM_MAX);
        assert_eq!(clamp_zoom(100), 100);
        let mut p = pin(10, 10);
        p.set_zoom(1);
        assert_eq!(p.zoom, ZOOM_MIN);
        p.set_zoom(9999);
        assert_eq!(p.zoom, ZOOM_MAX);
    }

    #[test]
    fn the_wheel_walks_the_ladder_and_stops_at_the_ends() {
        let mut p = pin(10, 10);
        assert_eq!(p.zoom_step(true, None), 110);
        assert_eq!(p.zoom_step(true, None), 125);
        assert_eq!(p.zoom_step(false, None), 110);
        p.set_zoom(ZOOM_MAX);
        assert_eq!(p.zoom_step(true, None), ZOOM_MAX);
        p.set_zoom(ZOOM_MIN);
        assert_eq!(p.zoom_step(false, None), ZOOM_MIN);
    }

    #[test]
    fn reset_zoom_anchors_on_the_centre_so_the_pin_does_not_teleport() {
        let mut p = pin(200, 100);
        let before = p.window_rect().center();
        p.set_zoom(50);
        assert_eq!(p.size_phys, PhysSize::new(100, 50));
        assert_eq!(p.window_rect().center(), before);
        p.reset_zoom();
        assert_eq!(p.pos_phys, PhysPoint::new(0, 0));
        assert_eq!(p.size_phys, PhysSize::new(200, 100));
    }

    #[test]
    fn the_size_the_window_is_told_matches_the_size_the_window_has() {
        let mut p = pin(333, 77);
        for z in [ZOOM_MIN, 33, 67, 100, 137, 300, ZOOM_MAX] {
            p.set_zoom(z);
            assert_eq!(p.size_phys, p.scaled_size(), "zoom {z}");
            assert_eq!(p.window_rect().size(), p.size_phys);
        }
    }

    // ---------------------------------------------------------------- §5.9.3

    #[test]
    fn typing_a_size_is_a_container_and_the_ratio_stays_locked() {
        let mut p = pin(400, 200);
        p.set_size_to(Some(100), Some(100)).expect("size");
        assert_eq!(p.zoom, 25);
        assert_eq!(p.size_phys, PhysSize::new(100, 50));

        let mut q = pin(400, 200);
        q.set_size_to(Some(200), None).expect("size");
        assert_eq!(q.zoom, 50);
        assert_eq!(q.size_phys, PhysSize::new(200, 100));

        let mut r = pin(400, 200);
        assert!(matches!(r.set_size_to(None, None), Err(PinError::NoSize)));
    }

    // ---------------------------------------------------------------- §5.9.4

    #[test]
    fn opacity_has_a_floor_but_zero_means_deliberately_invisible() {
        assert_eq!(clamp_opacity(5), OPACITY_MIN);
        assert_eq!(clamp_opacity(0), 0);
        assert_eq!(clamp_opacity(150), 100);
        let mut p = pin(10, 10);
        p.set_opacity(5);
        assert_eq!(p.opacity, OPACITY_MIN);
        p.set_opacity(0);
        assert_eq!(p.opacity, 0);
        p.reset_opacity();
        assert_eq!(p.opacity, 100);
        assert_eq!(p.opacity_step(false), 90);
        assert_eq!(p.opacity_step(true), 100);
        assert_eq!(p.opacity_step(true), 100);
    }

    // ---------------------------------------------------------------- §5.9.5

    #[test]
    fn a_quarter_turn_swaps_the_axes_and_stays_centred() {
        let mut p = pin(200, 100);
        let centre = p.window_rect().center();
        p.rotate_cw();
        assert_eq!(p.rotation, 90);
        assert_eq!(p.size_phys, PhysSize::new(100, 200));
        assert_eq!(p.pos_phys, PhysPoint::new(50, -50));
        assert_eq!(p.window_rect().center(), centre);
        for _ in 0..3 {
            p.rotate_cw();
        }
        assert_eq!(p.rotation, 0);
        assert_eq!(p.pos_phys, PhysPoint::new(0, 0));
        assert_eq!(p.size_phys, PhysSize::new(200, 100));
    }

    #[test]
    fn counter_clockwise_is_the_inverse_of_clockwise() {
        let mut p = pin(200, 100);
        p.rotate_ccw();
        assert_eq!(p.rotation, 270);
        assert!(p.quarter_turned());
        p.rotate_cw();
        assert_eq!(p.rotation, 0);
        assert_eq!(p.window_rect().center(), PhysPoint::new(100, 50));
    }

    // ---------------------------------------------------------------- §5.9.6

    #[test]
    fn the_flips_are_toggles() {
        let mut p = pin(10, 10);
        assert!(p.toggle_flip_h());
        assert!(!p.toggle_flip_h());
        assert!(p.toggle_flip_v());
        assert!(p.flip_v);
    }

    // -------------------------------------------------------------- §5.9.7/8

    #[test]
    fn grayscale_and_invert_are_toggles_too() {
        let mut p = pin(10, 10);
        assert!(p.toggle_grayscale());
        assert!(!p.toggle_grayscale());
        assert!(p.toggle_invert());
        assert!(p.inverted);
    }

    #[test]
    fn the_effects_land_on_the_pixels_in_the_order_the_window_shows_them() {
        let src = solid(4, 2, [10, 20, 30, 255]);
        let mut p = pin_at(1, 4, 2, PhysPoint::new(0, 0));
        let plain = p.render(&src).expect("render");
        assert_eq!(plain.get(0, 0), [10, 20, 30, 255]);

        p.toggle_grayscale();
        let gray = p.render(&src).expect("render");
        // Rec. 601 luma of (10,20,30), with the alpha untouched.
        assert_eq!(gray.get(3, 1), [18, 18, 18, 255]);

        p.toggle_invert();
        let neg = p.render(&src).expect("render");
        // Invert comes after grayscale, the same order the window composes them.
        assert_eq!(neg.get(0, 0), [237, 237, 237, 255]);
    }

    // ---------------------------------------------------------------- §5.9.9

    #[test]
    fn cropping_keeps_the_resolution_of_what_it_keeps() {
        let mut p = pin(200, 100);
        p.crop(&PhysRect::new(50, 50, 100, 60)).expect("crop");
        // Clipped to the picture: only 50 of the 60 rows asked for exist.
        assert_eq!(p.src_rect, PhysRect::new(50, 50, 100, 50));
        assert_eq!(p.size_phys, PhysSize::new(100, 50));
        assert_eq!(p.crop_undo, vec![PhysRect::new(0, 0, 200, 100)]);
        p.undo_crop().expect("undo");
        assert_eq!(p.src_rect, PhysRect::new(0, 0, 200, 100));
        assert!(matches!(p.undo_crop(), Err(PinError::NothingToUndo)));
    }

    #[test]
    fn taking_the_whole_picture_back_is_not_a_crop() {
        let mut p = pin(200, 100);
        p.crop(&p.whole_rect()).expect("crop");
        assert!(p.crop_undo.is_empty());
        assert_eq!(p.src_rect, PhysRect::new(0, 0, 200, 100));
    }

    #[test]
    fn a_crop_outside_the_picture_is_refused() {
        let mut p = pin(200, 100);
        assert!(matches!(
            p.crop(&PhysRect::new(300, 0, 10, 10)),
            Err(PinError::Outside(_, _))
        ));
        assert!(matches!(
            p.crop(&PhysRect::new(10, 10, 0, 0)),
            Err(PinError::Outside(_, _))
        ));
        assert_eq!(p.src_rect, PhysRect::new(0, 0, 200, 100));
    }

    #[test]
    fn a_crop_outside_the_crop_is_clipped_rather_than_rejected() {
        let mut p = pin(200, 100);
        p.crop(&PhysRect::new(100, 50, 200, 200)).expect("crop");
        assert_eq!(p.src_rect, PhysRect::new(100, 50, 100, 50));
    }

    #[test]
    fn dragging_a_crop_on_the_window_maps_through_the_zoom() {
        let mut p = pin(100, 50);
        p.set_zoom(200);
        assert_eq!(p.size_phys, PhysSize::new(200, 100));
        let w = p.window_rect();
        p.crop_window(&PhysRect::new(w.x, w.y, w.w / 2, w.h / 2))
            .expect("crop");
        assert_eq!(p.src_rect, PhysRect::new(0, 0, 50, 25));
        p.undo_crop().expect("undo");
        let w = p.window_rect();
        p.crop_window(&w).expect("full window crop");
        // Asking for the whole window is asking for the whole picture.
        assert_eq!(p.src_rect, PhysRect::new(0, 0, 100, 50));
    }

    #[test]
    fn restoring_the_original_forgets_the_whole_undo_stack() {
        let mut p = pin(200, 100);
        p.crop(&PhysRect::new(0, 0, 100, 50)).expect("crop");
        p.crop(&PhysRect::new(20, 10, 50, 30)).expect("crop");
        assert_eq!(p.crop_undo.len(), 2);
        assert!(p.restore_original());
        assert_eq!(p.src_rect, PhysRect::new(0, 0, 200, 100));
        assert!(p.crop_undo.is_empty());
        assert!(!p.restore_original());
    }

    // -------------------------------------------------------- §5.9.10/11

    #[test]
    fn a_fixed_thumbnail_fits_the_box_and_a_second_press_gives_the_size_back() {
        let mut p = pin(400, 200);
        assert!(p.toggle_thumbnail(PhysSize::new(100, 100)));
        assert_eq!(p.zoom, 25);
        assert_eq!(p.size_phys, PhysSize::new(100, 50));
        assert_eq!(p.thumbnail.map(|t| t.mode), Some(ThumbnailMode::Fixed));

        p.set_zoom(60);
        assert!(!p.toggle_thumbnail(PhysSize::new(100, 100)));
        assert_eq!(p.zoom, 100);
        assert_eq!(p.size_phys, PhysSize::new(400, 200));
        assert_eq!(p.thumbnail, None);
    }

    #[test]
    fn an_empty_box_falls_back_to_the_preset_size() {
        let mut p = pin(480, 240);
        p.enter_fixed_thumbnail(PhysSize::default());
        let t = p.thumbnail.expect("thumb");
        assert_eq!(t.rect.size(), THUMB_FIXED_DEFAULT);
    }

    #[test]
    fn leaving_a_thumbnail_that_was_never_entered_is_an_error() {
        let mut p = pin(10, 10);
        assert!(matches!(p.exit_thumbnail(), Err(PinError::NoThumbnail)));
    }

    #[test]
    fn a_free_thumbnail_keeps_the_size_it_replaced() {
        let mut p = pin(400, 200);
        p.set_zoom(150);
        let before = p.zoom;
        p.set_free_thumbnail(&PhysRect::new(100, 100, 50, 50))
            .expect("thumb");
        assert_eq!(p.zoom, 100);
        assert_eq!(p.size_phys, PhysSize::new(50, 50));
        assert_eq!(p.visible_rect(), PhysRect::new(100, 100, 50, 50));

        // Adjusting the region must not overwrite the size being remembered.
        p.set_free_thumbnail(&PhysRect::new(0, 0, 20, 20))
            .expect("thumb");
        assert_eq!(p.thumbnail.expect("t").prior_zoom, before);
        p.exit_thumbnail().expect("exit");
        assert_eq!(p.zoom, 150);
    }

    #[test]
    fn a_free_thumbnail_is_clipped_to_what_the_crop_kept() {
        let mut p = pin(400, 200);
        p.crop(&PhysRect::new(100, 0, 300, 200)).expect("crop");
        p.set_free_thumbnail(&PhysRect::new(50, 0, 100, 100))
            .expect("thumb");
        assert_eq!(p.visible_rect(), PhysRect::new(100, 0, 50, 100));
        assert!(matches!(
            p.set_free_thumbnail(&PhysRect::new(0, 0, 10, 10)),
            Err(PinError::Outside(_, _))
        ));
    }

    #[test]
    fn the_window_renders_the_thumbnail_region_while_export_keeps_the_crop() {
        let mut src = solid(4, 2, [0, 0, 0, 255]);
        src.set(1, 0, [11, 0, 0, 255]);
        src.set(3, 1, [0, 0, 22, 255]);
        let mut p = PinItem::from_frame(1, &src, PhysPoint::new(0, 0), &cfg()).expect("pin");

        p.crop(&PhysRect::new(1, 0, 3, 2)).expect("crop");
        // With nothing looking at the pin, the two are the same picture.
        assert_eq!(
            p.render_shown(&src).expect("shown").get(0, 0),
            p.render(&src).expect("export").get(0, 0)
        );

        p.set_free_thumbnail(&PhysRect::new(3, 1, 1, 1))
            .expect("thumb");
        let shown = p.render_shown(&src).expect("shown");
        assert_eq!((shown.width, shown.height), (1, 1));
        assert_eq!(shown.get(0, 0), [0, 0, 22, 255]);

        // §5.9.16: the thumbnail is a way of looking, so the copy keeps the crop.
        let export = p.render(&src).expect("export");
        assert_eq!((export.width, export.height), (3, 2));
        assert_eq!(export.get(2, 1), [0, 0, 22, 255]);

        // And rotation still applies to what the window shows.
        p.rotate_cw();
        let turned = p.render_shown(&src).expect("shown");
        assert_eq!((turned.width, turned.height), (1, 1));
    }

    #[test]
    fn the_alpha_board_is_painted_only_where_the_picture_is_see_through() {
        let mut src = solid(16, 1, [7, 7, 7, 255]);
        src.set(0, 0, [0, 0, 0, 0]);
        src.set(8, 0, [0, 0, 0, 0]);
        let mut p = PinItem::from_frame(1, &src, PhysPoint::new(0, 0), &cfg()).expect("pin");
        p.alpha_bg_mode = AlphaBg::CheckerLight;
        let board = p.paint_alpha_bg(p.render(&src).expect("render"));
        // CHECKER_CELL_PX is 8, so a 16-wide strip covers both squares.
        assert_eq!(board.get(0, 0), [0xd4, 0xd4, 0xd4, 0xff]);
        assert_eq!(board.get(8, 0), [0xef, 0xef, 0xef, 0xff]);
        assert_eq!(board.get(1, 0), [7, 7, 7, 255]);

        // The modes that draw no board leave the alpha alone - and export never
        // asks for the board in the first place.
        p.alpha_bg_mode = AlphaBg::Transparent;
        let plain = p.paint_alpha_bg(p.render(&src).expect("render"));
        assert_eq!(plain.get(0, 0), [0, 0, 0, 0]);
    }

    // --------------------------------------------------------------- §5.9.11

    #[test]
    fn a_right_drag_on_the_window_lays_out_a_free_thumbnail() {
        let mut p = pin(400, 200);
        p.set_zoom(200);
        // Zoom anchors the centre, so the window is 800x400 hanging around the
        // old one. The drag arrives in screen pixels, which is why the mapping
        // has to go through `window_to_source` rather than reading the numbers.
        let win = p.window_rect();
        assert_eq!(win, PhysRect::new(-200, -100, 800, 400));
        p.free_thumbnail_window(&PhysRect::new(win.x, win.y, win.w / 2, win.h / 2))
            .expect("thumb");
        assert_eq!(p.visible_rect(), PhysRect::new(0, 0, 200, 100));
        assert_eq!(p.zoom, 100);
        // A thumbnail narrows what is shown, never the crop.
        assert_eq!(p.src_rect, PhysRect::new(0, 0, 400, 200));

        // Adjusting it reads the region the new drag covers, not the whole pin.
        let win = p.window_rect();
        p.free_thumbnail_window(&PhysRect::new(win.x, win.y, win.w / 2, win.h / 2))
            .expect("adjust");
        assert_eq!(p.visible_rect(), PhysRect::new(0, 0, 100, 50));

        // And a drag that leaves the window is refused rather than clamped.
        assert!(matches!(
            p.free_thumbnail_window(&PhysRect::new(win.x, win.y, win.w + 10, win.h)),
            Err(PinError::Outside(_, _))
        ));

        // From 100% the percentage does not move but the region does, and the
        // window is sized by the region.
        let mut q = pin(400, 200);
        q.free_thumbnail_window(&PhysRect::new(0, 0, 200, 100))
            .expect("thumb");
        assert_eq!(q.zoom, 100);
        assert_eq!(q.size_phys, PhysSize::new(200, 100));
    }

    // --------------------------------------------------------------- §5.9.12

    #[test]
    fn the_top_flag_is_per_pin_and_a_batch_can_set_it() {
        let mut s = pile(3, 10, 10);
        s.get_mut(2).expect("pin").set_topmost(false);
        assert!(!s.get(2).expect("pin").topmost);
        for id in s.ids() {
            s.get_mut(id).expect("pin").set_topmost(true);
        }
        assert!(s.items().iter().all(|p| p.topmost));
    }

    // --------------------------------------------------------------- §5.9.13

    #[test]
    fn click_through_is_remembered_because_the_tray_has_to_turn_it_back_off() {
        let mut p = pin(10, 10);
        assert!(p.set_click_through(true));
        assert!(p.click_through);
        assert!(!p.set_click_through(false));
    }

    // --------------------------------------------------------------- §5.9.14

    #[test]
    fn the_frame_stepper_wraps_and_stepping_pauses_playback() {
        let mut p = pin(10, 10);
        let ps = p.set_gif(5).expect("gif");
        assert!(ps.playing);
        ps.step(1);
        assert_eq!(ps.frame, 1);
        assert!(!ps.playing);
        ps.step(-1);
        ps.step(-1);
        assert_eq!(ps.frame, 4);
        ps.first_frame();
        assert_eq!(ps.frame, 0);
        p.set_gif(0);
        assert_eq!(p.gif, None);
    }

    #[test]
    fn a_still_has_no_frame_to_play() {
        let mut p = pin(10, 10);
        let ps = p.set_gif(1).expect("gif");
        assert!(!ps.toggle_play());
        ps.step(1);
        assert_eq!(ps.frame, 0);
        ps.set_speed(0);
        assert_eq!(ps.speed, 1);
        ps.set_speed(9999);
        assert_eq!(ps.speed, 1000);
        ps.set_speed(100);
        ps.speed_step(true);
        assert_eq!(ps.speed, 150);
    }

    // --------------------------------------------------------------- §5.9.15

    #[test]
    fn a_transparent_pixel_lets_the_click_go_through_and_a_board_does_not() {
        let mut p = pin(4, 4);
        let hole = |s: PhysPoint| if s.x == 0 && s.y == 0 { 0 } else { 255 };
        assert!(!p.hit(PhysPoint::new(0, 0), hole));
        assert!(p.hit(PhysPoint::new(1, 0), hole));

        p.set_alpha_bg_mode(AlphaBg::Pseudo);
        assert!(p.hit(PhysPoint::new(0, 0), hole));
        p.set_alpha_bg_mode(AlphaBg::CheckerLight);
        assert!(p.hit(PhysPoint::new(0, 0), hole));
    }

    #[test]
    fn a_point_outside_the_window_is_not_a_hit_at_all() {
        let p = pin(10, 10);
        assert_eq!(p.window_to_source(PhysPoint::new(500, 500)), None);
        assert!(!p.hit(PhysPoint::new(500, 500), opaque));
    }

    #[test]
    fn a_hit_on_a_rotated_pin_reads_the_rotated_picture() {
        let mut p = pin(100, 40);
        p.rotate_cw();
        assert!(p.toggle_flip_h());
        let w = p.window_rect();
        let top_left = PhysPoint::new(w.x, w.y);
        let bottom_right = PhysPoint::new(w.right() - 1, w.bottom() - 1);
        assert_eq!(p.window_to_source(top_left), Some(PhysPoint::new(99, 39)));
        assert_eq!(p.window_to_source(bottom_right), Some(PhysPoint::new(0, 0)));
    }

    #[test]
    fn every_point_of_the_window_maps_into_the_picture() {
        let mut p = pin(23, 17);
        p.set_zoom(37);
        p.rotate_cw();
        p.toggle_flip_v();
        let w = p.window_rect();
        let v = p.visible_rect();
        for y in 0..w.h {
            for x in 0..w.w {
                let s = p
                    .window_to_source(PhysPoint::new(w.x + x as i32, w.y + y as i32))
                    .expect("inside");
                assert!(
                    s.x >= v.x && s.x < v.right() && s.y >= v.y && s.y < v.bottom(),
                    "{s} outside {v}"
                );
            }
        }
    }

    #[test]
    fn the_checkerboard_alternates_on_a_cell_grid() {
        assert!(checker_is_light(0, 0, CHECKER_CELL_PX));
        assert!(!checker_is_light(CHECKER_CELL_PX, 0, CHECKER_CELL_PX));
        assert!(!checker_is_light(0, CHECKER_CELL_PX, CHECKER_CELL_PX));
        assert!(checker_is_light(
            CHECKER_CELL_PX,
            CHECKER_CELL_PX,
            CHECKER_CELL_PX
        ));
        assert!(checker_is_light(7, 7, CHECKER_CELL_PX));
        assert!(checker_is_light(0, 0, 0));
    }

    #[test]
    fn only_the_board_modes_get_board_colours() {
        assert_eq!(checker_colors(AlphaBg::Transparent), None);
        assert_eq!(checker_colors(AlphaBg::Pseudo), None);
        let dark = checker_colors(AlphaBg::CheckerDark).expect("dark");
        let light = checker_colors(AlphaBg::CheckerLight).expect("light");
        assert!(dark.0[0] < light.0[0]);
    }

    // --------------------------------------------------------------- §5.9.16

    #[test]
    fn the_copy_is_the_picture_not_the_zoom() {
        let src = solid(200, 100, [1, 2, 3, 255]);
        let mut p = pin_at(1, 200, 100, PhysPoint::new(0, 0));
        p.set_zoom(800);
        let out = p.render(&src).expect("render");
        assert_eq!(out.width, 200);
        assert_eq!(out.height, 100);

        p.enter_fixed_thumbnail(PhysSize::new(50, 50));
        assert_eq!(p.render(&src).expect("render").width, 200);
        p.set_free_thumbnail(&PhysRect::new(10, 10, 20, 20))
            .expect("free");
        assert_eq!(p.render(&src).expect("render").width, 200);
    }

    #[test]
    fn the_copy_is_the_crop_and_nothing_else() {
        let mut src = solid(4, 1, [0, 0, 0, 255]);
        src.set(0, 0, [255, 0, 0, 255]);
        src.set(1, 0, [0, 255, 0, 255]);
        src.set(2, 0, [0, 0, 255, 255]);
        src.set(3, 0, [255, 255, 0, 255]);
        let mut p = pin_at(1, 4, 1, PhysPoint::new(0, 0));
        p.crop(&PhysRect::new(1, 0, 2, 1)).expect("crop");
        let out = p.render(&src).expect("render");
        assert_eq!(out.width, 2);
        assert_eq!(out.get(0, 0), [0, 255, 0, 255]);
        assert_eq!(out.get(1, 0), [0, 0, 255, 255]);

        p.toggle_flip_h();
        let flipped = p.render(&src).expect("render");
        assert_eq!(flipped.get(0, 0), [0, 0, 255, 255]);
        assert_eq!(flipped.get(1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn a_rotation_quarter_turns_the_copy() {
        let mut src = solid(2, 1, [0, 0, 0, 255]);
        src.set(0, 0, [1, 1, 1, 255]);
        src.set(1, 0, [2, 2, 2, 255]);
        let mut p = pin_at(1, 2, 1, PhysPoint::new(0, 0));
        p.rotate_cw();
        let out = p.render(&src).expect("render");
        assert_eq!(out.width, 1);
        assert_eq!(out.height, 2);
        assert_eq!(out.get(0, 0), [1, 1, 1, 255]);
        assert_eq!(out.get(0, 1), [2, 2, 2, 255]);
    }

    #[test]
    fn the_copy_refuses_a_picture_that_is_not_the_one_the_pin_was_made_from() {
        let p = pin(200, 100);
        let wrong = solid(50, 50, [0, 0, 0, 255]);
        assert!(matches!(
            p.render(&wrong),
            Err(PinError::WrongSize(a, b))
                if a == PhysSize::new(50, 50) && b == PhysSize::new(200, 100)
        ));
    }

    // --------------------------------------------------------------- §5.9.17

    #[test]
    fn reset_puts_the_view_back_without_touching_the_content() {
        let mut p = pin(200, 100);
        p.crop(&PhysRect::new(20, 10, 100, 50)).expect("crop");
        p.rotate_cw();
        p.toggle_flip_h();
        p.toggle_grayscale();
        p.set_opacity(30);
        p.set_click_through(true);
        p.enter_fixed_thumbnail(PhysSize::new(40, 40));
        let where_it_stood = p.pos_phys;
        p.reset_view(&cfg());
        assert_eq!(p.rotation, 0);
        assert!(!p.flip_h);
        assert!(!p.grayscale);
        assert_eq!(p.thumbnail, None);
        assert!(!p.click_through);
        assert_eq!(p.opacity, 100);
        assert!(p.smooth_zoom);
        assert_eq!(p.zoom, 100);
        assert_eq!(p.src_rect, PhysRect::new(20, 10, 100, 50));
        assert_eq!(p.pos_phys, where_it_stood);
        assert_eq!(p.size_phys, PhysSize::new(100, 50));
    }

    // --------------------------------------------------------------- §5.9.18

    #[test]
    fn destroying_and_restoring_returns_the_pile_exactly_as_it_was() {
        let mut s = pile(4, 10, 10);
        s.select(3, false).expect("select");
        let before = s.ids();
        let taken = s.destroy(&[3]);
        assert_eq!(taken.len(), 1);
        assert_eq!(s.ids(), vec![1, 2, 4]);
        assert_eq!(s.selection_count(), 0);
        assert_eq!(s.restore(taken), 1);
        assert_eq!(s.ids(), before);
        assert_eq!(s.restore(Vec::new()), 0);
    }

    #[test]
    fn destroying_a_pin_leaves_the_group_it_belonged_to_alone() {
        let mut s = pile(2, 10, 10);
        let g = s.create_group("g").expect("group");
        s.assign_group(&[1, 2], Some(g)).expect("assign");
        s.destroy(&[1]);
        assert_eq!(s.pins_in_group(g), vec![2]);
        assert_eq!(s.destroy(&[2]).len(), 1);
        assert!(s.pins_in_group(g).is_empty());
    }

    // ---------------------------------------------------------------- §5.10

    #[test]
    fn clicking_a_pile_can_cycle_to_the_pin_buried_under_it() {
        let mut s = pile(3, 100, 100);
        assert_eq!(s.click_at(PhysPoint::new(10, 10), false, &body), Some(3));
        assert_eq!(s.click_at(PhysPoint::new(10, 10), true, &body), Some(1));
        assert!(s.is_selected(1));
        assert_eq!(s.ids(), vec![2, 3, 1]);
    }

    #[test]
    fn clicking_nothing_clears_the_selection() {
        let mut s = pile(2, 20, 20);
        s.select_all();
        assert_eq!(s.selection_count(), 2);
        assert_eq!(s.click_at(PhysPoint::new(900, 900), false, &body), None);
        assert_eq!(s.selection_count(), 0);
    }

    #[test]
    fn ctrl_click_adds_and_takes_away() {
        let mut s = pile(2, 20, 20);
        s.select(1, false).expect("one");
        s.select(2, true).expect("two");
        assert_eq!(s.selected_ids(), vec![1, 2]);
        s.select(2, true).expect("toggle off");
        assert_eq!(s.selected_ids(), vec![1]);
        assert!(matches!(s.select(99, false), Err(PinError::NoSuchPin(99))));
    }

    #[test]
    fn hidden_pins_are_not_in_the_click_stack() {
        let mut s = pile(2, 100, 100);
        s.get_mut(2).expect("pin").solo_excluded = true;
        assert_eq!(s.stack_at(PhysPoint::new(10, 10), &body), vec![1]);
        assert!(!s.get(2).expect("pin").visible());
    }

    #[test]
    fn a_batch_move_clamps_each_pin_on_its_own() {
        let desk = screen(1000, 600);
        let mut s = PinSet::default();
        s.add(pin_at(1, 100, 100, PhysPoint::new(900, 100)));
        s.add(pin_at(2, 100, 100, PhysPoint::new(0, 100)));
        s.set_selection(&[1, 2]);
        assert_eq!(s.translate_selection(200, 0, &desk), 2);
        assert_eq!(s.get(1).expect("pin").pos_phys.x, 976);
        assert_eq!(s.get(2).expect("pin").pos_phys.x, 200);
    }

    #[test]
    fn a_batch_opacity_change_only_touches_the_selection() {
        let mut s = pile(3, 10, 10);
        s.set_selection(&[1, 3]);
        assert_eq!(s.set_selection_opacity(50), 2);
        assert_eq!(s.get(1).expect("pin").opacity, 50);
        assert_eq!(s.get(2).expect("pin").opacity, 100);
        assert_eq!(s.get(3).expect("pin").opacity, 50);
        assert_eq!(s.reset_selection(&cfg()), 2);
    }

    #[test]
    fn raising_and_lowering_move_one_pin_within_the_pile() {
        let mut s = pile(3, 10, 10);
        assert!(s.raise(1));
        assert_eq!(s.ids(), vec![2, 3, 1]);
        assert!(s.lower(1));
        assert_eq!(s.ids(), vec![1, 2, 3]);
        assert!(!s.raise(9));
    }

    #[test]
    fn a_batch_raise_moves_the_block_up_without_reordering_it() {
        let mut s = pile(3, 10, 10);
        s.get_mut(1).expect("pin").id = 9;
        // A pile whose ids do not run in z order, so the assertion below can
        // only hold if the raise keeps the pins in their own order.
        assert_eq!(s.ids(), vec![9, 2, 3]);
        s.set_selection(&[9, 3]);
        assert_eq!(s.raise_selection(), 2);
        assert_eq!(s.ids(), vec![2, 9, 3]);
        assert_eq!(s.z_order().collect::<Vec<_>>(), vec![3, 9, 2]);
    }

    #[test]
    fn closing_a_pin_leaves_the_picture_behind() {
        let mut s = pile(3, 10, 10);
        assert_eq!(s.close(&[2]), 1);
        assert_eq!(s.ids(), vec![1, 3]);
        assert_eq!(s.close(&[2]), 0);
    }

    #[test]
    fn the_dialog_counts_what_the_gesture_would_touch() {
        let mut s = pile(4, 10, 10);
        s.set_selection(&[1, 2, 3]);
        assert_eq!(s.destroy_count(), 3);
    }

    // ---------------------------------------------------------------- §5.11

    #[test]
    fn solo_dims_the_rest_and_puts_their_own_opacity_back_on_exit() {
        let mut s = pile(3, 10, 10);
        s.get_mut(2).expect("pin").set_opacity(40);
        s.select(1, false).expect("select");
        assert_eq!(s.enter_solo(&cfg()), 2);
        assert!(s.solo_active());
        assert_eq!(s.solo_keeps(), &[1]);
        assert_eq!(s.get(2).expect("pin").opacity, cfg().solo_dim_opacity);
        assert_eq!(s.get(3).expect("pin").opacity, cfg().solo_dim_opacity);
        assert_eq!(s.exit_solo(), 2);
        assert!(!s.solo_active());
        assert_eq!(s.get(2).expect("pin").opacity, 40);
        assert_eq!(s.get(3).expect("pin").opacity, 100);
    }

    #[test]
    fn solo_hides_instead_of_dimming_when_dimming_is_off() {
        let mut c = cfg();
        c.solo_dim_opacity = 0;
        let mut s = pile(2, 10, 10);
        s.select(1, false).expect("select");
        assert_eq!(s.enter_solo(&c), 1);
        assert!(s.get(2).expect("pin").solo_excluded);
        assert_eq!(s.get(2).expect("pin").opacity, 100);
        s.exit_solo();
        assert!(!s.get(2).expect("pin").solo_excluded);
    }

    #[test]
    fn solo_with_nothing_selected_does_nothing() {
        let mut s = pile(2, 10, 10);
        assert_eq!(s.enter_solo(&cfg()), 0);
        assert!(!s.solo_active());
        assert!(!s.toggle_solo(&cfg()));
    }

    #[test]
    fn toggling_solo_is_the_same_gesture_both_ways() {
        let mut s = pile(2, 10, 10);
        s.select(1, false).expect("select");
        assert!(s.toggle_solo(&cfg()));
        assert!(!s.toggle_solo(&cfg()));
        assert_eq!(s.get(2).expect("pin").opacity, 100);
    }

    #[test]
    fn destroying_the_last_solo_pin_ends_the_session() {
        let mut c = cfg();
        c.solo_dim_opacity = 0;
        let mut s = pile(3, 10, 10);
        s.select(1, false).expect("select");
        s.enter_solo(&c);
        s.close(&[1]);
        assert!(!s.solo_active());
        assert!(!s.get(2).expect("pin").solo_excluded);
        assert!(!s.get(3).expect("pin").solo_excluded);
    }

    #[test]
    fn closing_a_dimmed_pin_keeps_the_session_pointing_at_what_is_left() {
        let mut s = pile(3, 10, 10);
        s.set_selection(&[1, 2]);
        s.enter_solo(&cfg());
        s.close(&[2]);
        assert_eq!(s.solo_keeps(), &[1]);
        assert_eq!(s.get(3).expect("pin").opacity, cfg().solo_dim_opacity);
        s.exit_solo();
        assert_eq!(s.get(3).expect("pin").opacity, 100);
    }

    // ---------------------------------------------------------------- §5.12

    #[test]
    fn group_names_are_trimmed_and_unique_ignoring_case() {
        let mut s = pile(1, 10, 10);
        let g = s.create_group(" 参考 ").expect("group");
        assert_eq!(s.group(g).expect("group").name, "参考");
        assert!(matches!(
            s.create_group("参考"),
            Err(PinError::DuplicateGroup(_))
        ));
        assert!(matches!(
            s.create_group("   "),
            Err(PinError::EmptyGroupName)
        ));
        s.create_group("Ref").expect("second");
        assert!(matches!(
            s.rename_group(g, "ref"),
            Err(PinError::DuplicateGroup(_))
        ));
        s.rename_group(g, "其它").expect("rename");
        assert_eq!(s.group(g).expect("group").name, "其它");
        assert!(matches!(
            s.rename_group(99, "x"),
            Err(PinError::NoSuchGroup(99))
        ));
    }

    #[test]
    fn assigning_and_unassigning_count_the_pins_they_moved() {
        let mut s = pile(3, 10, 10);
        let g = s.create_group("g").expect("group");
        s.set_selection(&[1, 2]);
        assert_eq!(s.assign_selection_to_group(Some(g)).expect("assign"), 2);
        assert_eq!(s.pins_in_group(g), vec![1, 2]);
        assert_eq!(s.get(3).expect("pin").group_id, None);
        assert_eq!(s.assign_group(&[1, 2, 9], None).expect("unassign"), 2);
        assert!(matches!(
            s.assign_group(&[1], Some(42)),
            Err(PinError::NoSuchGroup(42))
        ));
    }

    #[test]
    fn deleting_a_group_asks_the_caller_what_happens_to_the_pins() {
        let mut s = pile(2, 10, 10);
        let g = s.create_group("g").expect("group");
        s.assign_group(&[1, 2], Some(g)).expect("assign");
        assert_eq!(s.delete_group(g, GroupFate::Ungroup).expect("delete"), 2);
        assert_eq!(s.len(), 2);
        assert_eq!(s.get(1).expect("pin").group_id, None);
        assert!(s.groups().is_empty());

        let h = s.create_group("h").expect("group");
        s.assign_group(&[1, 2], Some(h)).expect("assign");
        assert_eq!(s.delete_group(h, GroupFate::Destroy).expect("delete"), 2);
        assert!(s.is_empty());
        assert!(matches!(
            s.delete_group(h, GroupFate::Ungroup),
            Err(PinError::NoSuchGroup(_))
        ));
    }

    #[test]
    fn the_group_list_order_is_the_quick_switch_order() {
        let mut s = pile(1, 10, 10);
        let a = s.create_group("a").expect("a");
        let b = s.create_group("b").expect("b");
        let c = s.create_group("c").expect("c");
        assert_eq!(s.group_ids(), vec![a, b, c]);
        s.move_group(c, -2).expect("move");
        assert_eq!(s.group_ids(), vec![c, a, b]);
        s.move_group(c, 99).expect("move");
        assert_eq!(s.group_ids(), vec![a, b, c]);
        assert!(matches!(
            s.move_group(77, 1),
            Err(PinError::NoSuchGroup(77))
        ));
    }

    #[test]
    fn hiding_a_group_and_running_solo_do_not_step_on_each_other() {
        let mut s = pile(3, 10, 10);
        let g = s.create_group("g").expect("group");
        s.assign_group(&[1, 2], Some(g)).expect("assign");
        assert_eq!(s.set_group_shown(g, false).expect("hide"), 2);
        assert!(!s.group_shown(g));
        assert!(!s.get(1).expect("pin").visible());
        assert!(s.get(3).expect("pin").visible());

        s.select(3, false).expect("select");
        s.enter_solo(&cfg());
        s.exit_solo();
        assert!(!s.group_shown(g));
        assert!(!s.get(1).expect("pin").visible());
        assert!(!s.get(1).expect("pin").solo_excluded);

        assert!(s.toggle_group_shown(g).expect("show"));
        assert!(s.get(1).expect("pin").visible());
        assert!(matches!(
            s.set_group_shown(9, true),
            Err(PinError::NoSuchGroup(9))
        ));
    }

    #[test]
    fn a_hidden_group_is_out_of_the_click_stack_but_still_on_the_set() {
        let mut s = pile(2, 100, 100);
        let g = s.create_group("g").expect("group");
        s.assign_group(&[2], Some(g)).expect("assign");
        s.set_group_shown(g, false).expect("hide");
        assert_eq!(s.stack_at(PhysPoint::new(10, 10), &body), vec![1]);
        assert_eq!(s.len(), 2);
        assert!(s.group_shown(9));
    }

    #[test]
    fn ids_are_never_reused_after_a_destroy() {
        let mut s = pile(2, 10, 10);
        assert_eq!(s.next_id(), 3);
        s.destroy(&[2]);
        assert_eq!(s.next_id(), 3);
        let id = s
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(4, 4),
                PhysPoint::new(0, 0),
                &cfg(),
            )
            .expect("spawn");
        assert_eq!(id, 3);
        assert_eq!(s.next_id(), 4);
    }

    // ---------------------------------------------------------------- §6.3

    #[test]
    fn a_snapshot_writes_back_everything_that_matters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pic = dir.path().join("orig.png");
        encode::save(
            &solid(120, 80, [10, 20, 30, 255]),
            &pic,
            &EncodeOptions::default(),
        )
        .expect("save");

        let mut p = PinItem::new(
            3,
            ImageRef::File(pic.clone()),
            PhysSize::new(120, 80),
            PhysPoint::new(40, 50),
            &cfg(),
        )
        .expect("pin");
        p.crop(&PhysRect::new(10, 10, 60, 40)).expect("crop");
        p.rotate_cw();
        p.toggle_grayscale();
        p.toggle_flip_v();
        p.set_zoom(133);
        p.set_opacity(60);
        p.set_click_through(true);
        p.set_topmost(false);
        p.set_alpha_bg_mode(AlphaBg::CheckerDark);
        p.set_smooth_zoom(false);
        p.set_free_thumbnail(&PhysRect::new(10, 10, 30, 20))
            .expect("thumb");
        p.set_gif(5).expect("gif");

        let mut s = PinSet::default();
        s.add(p.clone());
        let g = s.create_group("参考").expect("group");
        s.assign_group(&[3], Some(g)).expect("assign");
        s.select(3, false).expect("select");

        let report = s
            .write_state(dir.path(), &mut no_frames, 1_700_000_000_000)
            .expect("write");
        assert_eq!(report.pins, 1);
        assert_eq!(report.frames_written, 0);
        assert!(report.frames_failed.is_empty());
        assert!(PinSet::has_state(dir.path()));

        let back = PinSet::read_state(dir.path())
            .expect("read")
            .expect("a file");
        assert_eq!(back.version, SNAPSHOT_VERSION);
        assert_eq!(back.dropped_missing, 0);
        assert_eq!(back.dropped_corrupt, 0);
        assert_eq!(back.frames.len(), 1);
        assert_eq!(back.set.len(), 1);
        assert_eq!(back.set.next_id(), s.next_id());
        assert_eq!(back.set.group_ids(), vec![g]);
        assert_eq!(back.set.group(g).expect("g").name, "参考");
        assert_eq!(back.set.selected_ids(), vec![3]);

        let q = back.set.get(3).expect("pin 3");
        assert_eq!(q.image, ImageRef::File(pic));
        assert_eq!(q.source_size, p.source_size);
        assert_eq!(q.src_rect, p.src_rect);
        assert_eq!(q.pos_phys, p.pos_phys);
        assert_eq!(q.size_phys, p.size_phys);
        assert_eq!(q.zoom, p.zoom);
        assert_eq!(q.opacity, p.opacity);
        assert_eq!(q.rotation, p.rotation);
        assert_eq!(q.flip_h, p.flip_h);
        assert_eq!(q.flip_v, p.flip_v);
        assert_eq!(q.grayscale, p.grayscale);
        assert_eq!(q.inverted, p.inverted);
        assert_eq!(q.topmost, p.topmost);
        assert_eq!(q.click_through, p.click_through);
        // The group was assigned on the set after `p` was cloned into it.
        assert_eq!(q.group_id, Some(g));
        assert_eq!(q.alpha_bg_mode, p.alpha_bg_mode);
        assert_eq!(q.smooth_zoom, p.smooth_zoom);
        assert_eq!(q.thumbnail, p.thumbnail);
        assert_eq!(q.gif, p.gif);
        assert_eq!(q.crop_undo, p.crop_undo);
    }

    #[test]
    fn a_pin_with_no_file_of_its_own_gets_its_pixels_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let frame = solid(4, 3, [7, 8, 9, 255]);
        let mut s = PinSet::default();
        s.add(PinItem::from_frame(9, &frame, PhysPoint::new(10, 10), &cfg()).expect("pin"));

        let mut grab = |id: PinId| -> Option<Frame> {
            if id == 9 {
                Some(frame.clone())
            } else {
                None
            }
        };
        let report = s.write_state(dir.path(), &mut grab, 1).expect("write");
        assert_eq!(report.frames_written, 1);
        assert!(dir.path().join(CACHE_DIR).join("9.png").is_file());

        let back = PinSet::read_state(dir.path())
            .expect("read")
            .expect("a file");
        assert_eq!(back.frames.len(), 1);
        assert_eq!(back.frames[0].1, frame);
        assert_eq!(
            back.set.get(9).expect("pin").image,
            ImageRef::Cache(PathBuf::from(format!("{CACHE_DIR}/9.png")))
        );
    }

    #[test]
    fn a_volatile_pin_with_nothing_to_write_is_dropped_rather_than_restored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = PinSet::default();
        s.add(pin(8, 8));
        let report = s.write_state(dir.path(), &mut no_frames, 1).expect("write");
        assert_eq!(report.frames_written, 0);
        assert_eq!(report.frames_failed, vec![7]);

        let back = PinSet::read_state(dir.path())
            .expect("read")
            .expect("a file");
        assert_eq!(back.dropped_missing, 1);
        assert!(back.set.is_empty());
    }

    #[test]
    fn a_picture_that_went_away_since_the_last_save_is_counted_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = PinSet::default();
        s.add(
            PinItem::new(
                1,
                ImageRef::History {
                    record: 5,
                    path: "history/5.png".to_string(),
                },
                PhysSize::new(20, 20),
                PhysPoint::new(0, 0),
                &cfg(),
            )
            .expect("pin"),
        );
        s.write_state(dir.path(), &mut no_frames, 1).expect("write");
        let back = PinSet::read_state(dir.path())
            .expect("read")
            .expect("a file");
        assert_eq!(back.dropped_missing, 1);
        assert!(back.set.is_empty());
    }

    #[test]
    fn a_picture_that_will_not_decode_does_not_take_the_process_down() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pic = dir.path().join("junk.png");
        let mut s = PinSet::default();
        s.add(
            PinItem::new(
                1,
                ImageRef::File(pic.clone()),
                PhysSize::new(20, 20),
                PhysPoint::new(0, 0),
                &cfg(),
            )
            .expect("pin"),
        );
        s.write_state(dir.path(), &mut no_frames, 1).expect("write");
        std::fs::write(&pic, b"not a png at all").expect("junk");

        let back = PinSet::read_state(dir.path())
            .expect("read")
            .expect("a file");
        assert_eq!(back.dropped_corrupt, 1);
        assert!(back.set.is_empty());
    }

    #[test]
    fn a_picture_replaced_by_a_smaller_one_shrinks_the_remembered_crop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pic = dir.path().join("swap.png");
        encode::save(
            &solid(60, 40, [1, 1, 1, 255]),
            &pic,
            &EncodeOptions::default(),
        )
        .expect("save");
        let mut p = PinItem::new(
            1,
            ImageRef::File(pic.clone()),
            PhysSize::new(120, 80),
            PhysPoint::new(0, 0),
            &cfg(),
        )
        .expect("pin");
        p.crop(&PhysRect::new(10, 10, 100, 60)).expect("crop");
        let mut s = PinSet::default();
        s.add(p);
        s.write_state(dir.path(), &mut no_frames, 1).expect("write");

        let back = PinSet::read_state(dir.path())
            .expect("read")
            .expect("a file");
        let q = back.set.get(1).expect("pin");
        assert_eq!(q.source_size, PhysSize::new(60, 40));
        assert_eq!(q.src_rect, PhysRect::new(10, 10, 50, 30));
    }

    #[test]
    fn an_empty_thumbnail_region_is_dropped_on_the_way_back_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pic = dir.path().join("t.png");
        encode::save(
            &solid(20, 20, [1, 1, 1, 255]),
            &pic,
            &EncodeOptions::default(),
        )
        .expect("save");
        let mut p = PinItem::new(
            1,
            ImageRef::File(pic),
            PhysSize::new(20, 20),
            PhysPoint::new(0, 0),
            &cfg(),
        )
        .expect("pin");
        p.thumbnail = Some(Thumbnail {
            mode: ThumbnailMode::Free,
            rect: PhysRect::new(0, 0, 0, 0),
            prior_zoom: 100,
        });
        let mut s = PinSet::default();
        s.add(p);
        s.write_state(dir.path(), &mut no_frames, 1).expect("write");
        let back = PinSet::read_state(dir.path())
            .expect("read")
            .expect("a file");
        assert_eq!(back.set.get(1).expect("pin").thumbnail, None);
    }

    #[test]
    fn no_state_file_is_not_an_error_and_clearing_it_leaves_no_pixels_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!PinSet::has_state(dir.path()));
        assert!(PinSet::read_state(dir.path()).expect("read").is_none());
        PinSet::clear_state(dir.path()).expect("clear");

        let frame = solid(2, 2, [1, 2, 3, 255]);
        let mut s = PinSet::default();
        s.add(PinItem::from_frame(1, &frame, PhysPoint::new(0, 0), &cfg()).expect("pin"));
        let mut grab = |_id: PinId| -> Option<Frame> { Some(frame.clone()) };
        s.write_state(dir.path(), &mut grab, 1).expect("write");
        assert!(dir.path().join(CACHE_DIR).join("1.png").is_file());
        PinSet::clear_state(dir.path()).expect("clear");
        assert!(!PinSet::has_state(dir.path()));
        assert!(!dir.path().join(CACHE_DIR).join("1.png").is_file());
    }

    #[test]
    fn a_state_file_from_a_newer_build_is_refused_not_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = pile(1, 10, 10);
        s.write_state(dir.path(), &mut no_frames, 1).expect("write");
        let path = dir.path().join(STATE_FILE);
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("version = 1"), "{text}");
        let bumped = text.replace("version = 1", "version = 99");
        std::fs::write(&path, &bumped).expect("bump");
        assert!(PinSet::read_state(dir.path()).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("still there"), bumped);
    }

    #[test]
    fn a_selection_that_survives_a_restart_only_names_live_pins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = StateFile {
            version: SNAPSHOT_VERSION,
            saved_at: 1,
            next_id: 7,
            next_group: 2,
            selection: vec![1, 2],
            groups: vec![Group {
                id: 2,
                name: "gone".to_string(),
            }],
            solo: Some(Solo {
                kept: vec![1],
                saved: vec![SavedPin { id: 2, opacity: 60 }],
            }),
            pins: Vec::new(),
        };
        let text = toml_edit::ser::to_string_pretty(&file).expect("serialize");
        std::fs::write(dir.path().join(STATE_FILE), text).expect("write");

        let back = PinSet::read_state(dir.path())
            .expect("read")
            .expect("a file");
        assert!(back.set.is_empty());
        assert!(back.set.selected_ids().is_empty());
        assert!(!back.set.solo_active());
        assert_eq!(back.set.next_id(), 7);
        assert_eq!(back.set.group_ids(), vec![2]);
    }

    #[test]
    fn the_cache_only_ever_holds_snapshots_of_pins_still_on_screen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = dir.path().join(CACHE_DIR);
        std::fs::create_dir_all(&cache).expect("mkdir");
        for name in ["1.png", "9.png", "notes.txt", "10.jpg"] {
            std::fs::write(cache.join(name), b"x").expect("write");
        }
        let keep: BTreeSet<PinId> = [1].into_iter().collect();
        assert_eq!(prune_cache(dir.path(), &keep).expect("prune"), 1);
        assert!(cache.join("1.png").is_file());
        assert!(!cache.join("9.png").is_file());
        assert!(cache.join("notes.txt").is_file());
        assert!(cache.join("10.jpg").is_file());
        assert_eq!(prune_cache(dir.path(), &BTreeSet::new()).expect("prune"), 1);
        assert!(!cache.join("1.png").is_file());
        assert_eq!(
            prune_cache(&dir.path().join("nowhere"), &BTreeSet::new()).expect("gone"),
            0
        );
    }

    #[test]
    fn the_snapshot_cadence_is_the_configured_one() {
        assert!(snapshot_due(0, 1_000, 5));
        assert!(!snapshot_due(10_000, 14_000, 5));
        assert!(snapshot_due(10_000, 15_000, 5));
        assert!(snapshot_due(10_000, 9_000, 5));
        assert!(!snapshot_due(0, 1_000, 0));
    }

    // ------------------------------------------------------------------ misc

    #[test]
    fn a_pin_lying_about_its_own_size_is_corrected_by_the_only_writer() {
        let mut p = pin(100, 50);
        p.size_phys = PhysSize::new(1, 1);
        p.sync_size();
        assert_eq!(p.size_phys, PhysSize::new(100, 50));
        p.zoom = 200;
        p.sync_size();
        assert_eq!(p.size_phys, PhysSize::new(200, 100));
    }

    #[test]
    fn the_two_thumbnail_axes_travel_together() {
        let mut p = pin(100, 100);
        p.crop(&PhysRect::new(0, 0, 50, 100)).expect("crop");
        p.rotate_cw();
        assert_eq!(p.natural_size(), PhysSize::new(50, 100));
        assert_eq!(p.shown_size(), PhysSize::new(100, 50));
        assert_eq!(p.scaled_size(), PhysSize::new(100, 50));
        assert_eq!(p.size_phys, p.scaled_size());
    }

    proptest! {
        #[test]
        fn a_pin_always_comes_back_with_some_of_it_visible(
            dw in 100u32..4000u32,
            dh in 100u32..4000u32,
            w in 1u32..5000u32,
            h in 1u32..5000u32,
            x in (-6000i32..6000i32),
            y in (-6000i32..6000i32),
        ) {
            let desk = screen(dw, dh);
            let r = PhysRect::new(x, y, w, h);
            let kept = desk.keep_visible(&r);
            prop_assert_eq!(kept.size(), r.size());
            let want = MIN_VISIBLE_PX.min(w).min(h) as i32;
            let b = desk.bounds;
            let over_x = kept.right().min(b.right()) - kept.x.max(b.x);
            let over_y = kept.bottom().min(b.bottom()) - kept.y.max(b.y);
            prop_assert!(over_x >= want, "x overlap {over_x} < {want} for {r}");
            prop_assert!(over_y >= want, "y overlap {over_y} < {want} for {r}");
            prop_assert_eq!(desk.keep_visible(&kept), kept);
        }

        #[test]
        fn any_zoom_leaves_the_window_and_the_size_in_agreement(
            sw in 1u32..600u32,
            sh in 1u32..600u32,
            zoom in 1u32..2000u32,
            turns in 0i32..4,
        ) {
            let mut p = pin(sw, sh);
            p.rotate(turns);
            p.set_zoom(zoom);
            prop_assert_eq!(p.size_phys, p.scaled_size());
            prop_assert!(p.size_phys.w >= 1);
            prop_assert!(p.size_phys.h >= 1);
            prop_assert_eq!(p.zoom, clamp_zoom(zoom));
            let w = p.window_rect();
            prop_assert_eq!(w.size(), p.size_phys);
            let corner = PhysPoint::new(w.x + w.w as i32 - 1, w.y + w.h as i32 - 1);
            prop_assert!(p.window_to_source(corner).is_some());
        }
    }
}
