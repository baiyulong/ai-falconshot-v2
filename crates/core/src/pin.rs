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

use crate::annotation::raster::Glyphs;
use crate::card::CardError;
use crate::colors;
use crate::config::{AlphaBg, Card, Pin as PinConfig};
use crate::encode::{self, EncodeOptions, Format};
use crate::frame::{Frame, FrameError};
use crate::geometry::{self, PhysPoint, PhysRect, PhysSize, Scale};
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
/// §5.9.10's "预设尺寸" fallback box: used when a `[pin]` config reaches
/// [`thumb_preset_box`] with a zero edge, and by callers (tests, the menu) that
/// pass an explicit box. The user's own preset lives in
/// [`crate::config::Pin::thumb_fixed_width`] / [`crate::config::Pin::thumb_fixed_height`],
/// whose `Default` is spelled from this constant so 240 has one author.
pub const THUMB_FIXED_DEFAULT: PhysSize = PhysSize::new(240, 240);
/// §5.9.15 checkerboard cell edge, in window pixels.
pub const CHECKER_CELL_PX: u32 = 8;
/// §5.9.14: speed is a percentage of the GIF's own frame delays.
pub const GIF_SPEED_STEPS: &[u32] = &[25, 50, 75, 100, 150, 200, 300, 400];
/// A frame whose delay field says nothing is a broken file, not an instruction
/// to spin: this is how long a viewer holds it.
pub const GIF_DEFAULT_FRAME_DELAY_MS: u32 = 100;
/// The same file read the other way - no frame may be shown for less than this,
/// however the user's speed multiplies it.
pub const GIF_MIN_FRAME_DELAY_MS: u32 = 10;
/// §5.9.14 / §7.1: how often the pin window asks its clip's clock whether a frame
/// is over, while somebody can actually see the window. This is the number
/// `PinWindow.qml`'s `gifClock` has carried since §5.9.14 - 20 ms, a polling rate
/// and not a frame delay - and it lives here for one reason: the throttled
/// cadence below has to be spelled next to it, by the same author, so the pair
/// cannot drift into two clocks (plan rule 281).
pub const GIF_POLL_MS: u32 = 20;
/// §7.1「GIF 不可见或最小化时，可降低刷新频率以节省资源」: the floor of the
/// cadence a pin window that nobody can see asks at. One second, i.e. twenty
/// times fewer wakeups than the visible arm, and - see [`gif_interval`] - still
/// a running clock rather than a freeze.
pub const GIF_HIDDEN_POLL_MS: u32 = 1_000;
/// The same question's ceiling. A throttled interval that followed a broken file's
/// own delay all the way to `u32::MAX` would never fire again, and "nobody can see
/// it" would have become "it never came back" - which §433 forbids silently.
pub const GIF_HIDDEN_POLL_MAX_MS: u32 = 10_000;

/// §7.1's cadence decision, as pure arithmetic.
///
/// `visible` is not something this file can read - a [`PinItem`] is numbers and
/// has no window - and the window's own reading is `PinWindow.qml`'s `gifWatched`
/// (`win.visible` **and** `win.visibility !== Window.Minimized / Window.Hidden`).
/// Both terms are needed and that was measured rather than remembered: Qt 6.10's
/// `QWindow::isVisible()` answers `d->visible`, the state the *application* asked
/// for, and `QWindowPrivate::updateVisibility()` only writes `Minimized` into
/// `visibility`, so a minimized window still answers `visible == true`.
///
/// `frame_delay_ms` is the per-frame pace this file already answers with
/// [`PlayState::delay_ms`], read through [`PlayState::frame_delay_ms`]. It is used
/// here as a *ruler for the hidden arm only*: a clip whose frame is on screen for
/// three seconds does not need to be asked five times per second even when nobody
/// sees it. It deliberately plays no part in the visible arm - when the frame
/// changes is [`PlayState::tick`]'s answer and nothing else, and a second place
/// that multiplies a delay is a second frame clock (plan rule 281).
///
/// The window runs the floor of the hidden arm (`gif_interval(false,
/// GIF_HIDDEN_POLL_MS)`, which is `GIF_HIDDEN_POLL_MS` itself) because
/// `PinViewData` carries no per-frame delay for QML to read; following a slow
/// clip's own pace is the part that still needs a reader, and it is stated here
/// rather than pretended away (§433).
pub fn gif_interval(visible: bool, frame_delay_ms: u32) -> u32 {
    let hidden = frame_delay_ms.clamp(GIF_HIDDEN_POLL_MS, GIF_HIDDEN_POLL_MAX_MS);
    (if visible { GIF_POLL_MS } else { hidden }).clamp(1, GIF_HIDDEN_POLL_MAX_MS)
}

pub const STATE_FILE: &str = "pins.state.toml";
/// Where a pin with no file behind it gets its pixels written, so §7.2 can
/// actually restore it. Same data directory as `history.db`.
pub const CACHE_DIR: &str = "pins";
pub const SNAPSHOT_VERSION: u32 = 1;
/// §7.2's second door, next to the snapshot rather than inside it: one file that
/// exists only *while a run is live*. The snapshot can only ever say what the
/// desktop looked like; whether the run that wrote it finished or died is a fact
/// about the run, and lives here. See [`open_session`].
pub const SESSION_FILE: &str = "pins.session";

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
    // 下面六臂是 §5.9.3「输入宽度、高度或缩放百分比」与 §5.9.4「或在菜单中输入透明度」那两格
    // 对话框说的话，所以字面是中文：这一句直接进页上那一格，不是日志、不是量规读数（本仓的
    // 对话框句子向来只有一个作者，见 `qt_bridge::capture::size_guard` 那一枚）。上面 `NoSize`
    // 留成英文，因为它说的是「程序内调用者一个数都没给」这件事，而调用者是 Rust 不是人。
    #[error("「{0}」这一格要的是整数，现在写的是「{1}」")]
    NotANumber(String, String),
    #[error("「{0}」要的是大于 0 的数，现在写的是 {1}")]
    NotAboveZero(String, i64),
    #[error("「{0}」要的是 {1}～{2} 之间的整数，现在写的是 {3}")]
    OutOfRange(String, i64, i64, i64),
    #[error("「{0}」这一格空着：填一个数才改得动")]
    EmptyCell(String),
    #[error("宽度、高度与缩放百分比说的是同一件事，请只填一种（现在两边都有字）")]
    BothSizeAndPercent,
    #[error("尺寸与缩放那一格至少填一格：宽度、高度、缩放百分比，填一种就够")]
    NothingTyped,
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

/// 对话框里一格的文本 → 一枚可选的整数。空着是 `None`（**那一格没填**，不是 0 —— 把没填读成 0
/// 就成了替用户输入），不是整数是指名到**那一格**的拒绝。§5.3.5 在
/// `qt_bridge::capture::size_guard` 立的是同一条口径：拒，不悄悄改成合法值。
fn cell_number(cell: &str, raw: &str) -> Result<Option<i64>, PinError> {
    let typed = raw.trim();
    if typed.is_empty() {
        return Ok(None);
    }
    typed
        .parse::<i64>()
        .map(Some)
        .map_err(|_| PinError::NotANumber(cell.to_string(), typed.to_string()))
}

/// 宽、高那一格比百分比多两条判据：**要大于 0**（§5.3.5「宽度和高度必须大于 0」那一句在这一格
/// 的同一位作者手里，0 与负数都不是一块窗装得下的尺寸），以及**大得没边也非法**（一格写着
/// 2^40 不是一枚尺寸，把它绕回 `u32` 再落下去就是说了一句用户没说的话）。
fn size_cell(label: &str, v: Option<i64>) -> Result<Option<u32>, PinError> {
    let Some(v) = v else {
        return Ok(None);
    };
    if v <= 0 {
        return Err(PinError::NotAboveZero(label.to_string(), v));
    }
    u32::try_from(v)
        .map(Some)
        .map_err(|_| PinError::OutOfRange(label.to_string(), 1, i64::from(u32::MAX), v))
}

/// §5.9.3 步骤 3「输入宽度、高度或缩放百分比」的全部判据，一个作者。
///
/// * 三格里至少一格有字，否则 [`PinError::NothingTyped`]；
/// * 宽/高与百分比**不混着填**（同一件事的两种说法一起填就是一次没说清），
///   [`PinError::BothSizeAndPercent`]；
/// * 每一格都得是整数（[`cell_number`]）、宽高都得大于 0（[`PinError::NotAboveZero`]）；
/// * 等比锁定那一式在 [`PinItem::locked_zoom`]，本档只此一处；
/// * **上下界**是 §5.9.2 那一族钉下的 [`ZOOM_MIN`]..=[`ZOOM_MAX`]：算出来的比例（或直接打出来
///   的百分比）跑到界外是**拒**，不是钳。钳是滚轮那一路该做的事（阶梯自己走不出界），而用户
///   亲手打下的 5000% 被悄悄改成 800% 说的是「你打的数我听成了另一个数」。界外那句
///   ([`PinError::OutOfRange`]) 把界本身报出来，页上不必再抄一份。
///
/// `shown` 是这一刻窗上那块的自然尺寸（[`PinItem::shown_size`]）：宽高是相对它读的，所以
/// 「锁定比例」锁的是**图**的比例，与窗此刻摆在多大无关。
pub fn size_zoom_request(
    width: &str,
    height: &str,
    percent: &str,
    shown: PhysSize,
) -> Result<u32, PinError> {
    let w = cell_number("宽度", width)?;
    let h = cell_number("高度", height)?;
    let p = cell_number("缩放百分比", percent)?;
    if p.is_some() && (w.is_some() || h.is_some()) {
        return Err(PinError::BothSizeAndPercent);
    }
    if let Some(p) = p {
        if !(ZOOM_MIN as i64..=ZOOM_MAX as i64).contains(&p) {
            return Err(PinError::OutOfRange(
                "缩放百分比".to_string(),
                ZOOM_MIN as i64,
                ZOOM_MAX as i64,
                p,
            ));
        }
        return Ok(p as u32);
    }
    let size = (size_cell("宽度", w)?, size_cell("高度", h)?);
    let pct = PinItem::locked_zoom(size.0, size.1, shown).ok_or(PinError::NothingTyped)?;
    // `locked_zoom` 的分子是 `u32`、分母至少 1，所以这里最大也就 4.29e11 量级，`round` 之后
    // 落进 `i64` 绰绰有余；越界那一句要在**报数**之前判，不能让 0 或一枚回绕的数冒充成功。
    let want = pct.round() as i64;
    if !(i64::from(ZOOM_MIN)..=i64::from(ZOOM_MAX)).contains(&want) {
        return Err(PinError::OutOfRange(
            "缩放百分比".to_string(),
            i64::from(ZOOM_MIN),
            i64::from(ZOOM_MAX),
            want,
        ));
    }
    Ok(want as u32)
}

/// §5.9.4 步骤 2「或在菜单中输入透明度」那一格的判据。
///
/// 界是 [`OPACITY_MIN`]..=100，而**下界连 0 一起拒**。规则第一句「透明度过低时仍应保留找回窗口的
/// 方式」要说得出口，得先有一只门能改得动这扇窗；而 0% 那一档窗上什么也画不出来，本窗给它的两只
/// 回头手（菜单里「不透明度 100%」与「重置视图」）都要先把这扇窗点着 —— 一扇完全不画的窗在
/// Windows 上究竟还接不接得到鼠标，本仓**没有量具测过**（`hit()` 判的是状态机里那块像素在不在图里，
/// 原生那层命中测试不在本档的射程内），所以这里不许把「还点得着」当成已成立的前提。这一扇门因此
/// 到不了 0：走得到的最低一档 [`OPACITY_MIN`] 仍然看得见、仍然吃得下点击，那句话在**这一扇门上**就
/// 是成立的。
///
/// [`clamp_opacity`] 手里 0 仍然是合法的（那是「程序内的调用者知道自己做了什么」那一格，
/// `reset_opacity` 是它写下的回头路），而窗上的阶梯 [`OPACITY_STEPS`] 最低也是 [`OPACITY_MIN`]：
/// 「页面上没有任何一格能把一扇贴图调成完全看不见」由这三处共同成立，测试
/// `the_floor_is_the_way_back_on_every_door` 逐臂判它，0% 那一臂只判状态机那一头。
pub fn typed_opacity(raw: &str) -> Result<u32, PinError> {
    let typed = raw.trim();
    if typed.is_empty() {
        return Err(PinError::EmptyCell("透明度".to_string()));
    }
    let want = typed
        .parse::<i64>()
        .map_err(|_| PinError::NotANumber("透明度".to_string(), typed.to_string()))?;
    if !(i64::from(OPACITY_MIN)..=100).contains(&want) {
        return Err(PinError::OutOfRange(
            "透明度".to_string(),
            i64::from(OPACITY_MIN),
            100,
            want,
        ));
    }
    Ok(want as u32)
}

/// §5.9.10 "预设尺寸": the box a fixed thumbnail is fitted into, spelled from the
/// user's `[pin]` config. This is the **only** place that turns the two
/// `thumb_fixed_*` keys into a [`PhysSize`] — the equal-ratio fit that then turns
/// the box into a shown size has its own single author,
/// [`PinItem::enter_fixed_thumbnail`] — so neither the bridge nor QML recomputes
/// the geometry. A zero on either edge (an un-validated config, or a caller that
/// never went through `Config::validate`) falls back to
/// [`THUMB_FIXED_DEFAULT`] rather than handing `enter_fixed_thumbnail` an empty
/// box, keeping the two fallbacks in agreement.
pub fn thumb_preset_box(cfg: &PinConfig) -> PhysSize {
    let box_ = PhysSize::new(cfg.thumb_fixed_width, cfg.thumb_fixed_height);
    if box_.is_empty() {
        THUMB_FIXED_DEFAULT
    } else {
        box_
    }
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
    /// Milliseconds already spent on the frame that is on screen. The one field
    /// with `#[serde(default)]`: a session saved before this clock existed has
    /// no such number, and §7.2 must still read it.
    #[serde(default)]
    pub elapsed_ms: u32,
}

impl PlayState {
    pub fn new(frames: usize) -> Self {
        Self {
            frames,
            frame: 0,
            speed: 100,
            playing: frames > 1,
            elapsed_ms: 0,
        }
    }

    /// §5.9.14 上一帧/下一帧. Stepping is an inspection action, so it pauses -
    /// and the half-spent frame it interrupts is spent, not carried over to the
    /// frame being looked at.
    pub fn step(&mut self, dir: i32) {
        if self.frames < 2 {
            return;
        }
        self.playing = false;
        self.elapsed_ms = 0;
        let n = self.frames as i64;
        let cur = self.frame as i64;
        let next = if dir < 0 { cur - 1 } else { cur + 1 };
        self.frame = next.rem_euclid(n) as usize;
    }

    pub fn first_frame(&mut self) {
        self.frame = 0;
        self.elapsed_ms = 0;
    }

    /// Resuming starts the wait again from now: the minute a paused pin sat
    /// there is not a minute of frames it owes.
    pub fn toggle_play(&mut self) -> bool {
        if self.frames > 1 {
            self.playing = !self.playing;
        }
        self.elapsed_ms = 0;
        self.playing
    }

    pub fn set_speed(&mut self, pct: u32) {
        self.speed = pct.clamp(1, 1000);
        self.elapsed_ms = 0;
    }

    pub fn speed_step(&mut self, up: bool) {
        self.speed = step_on(GIF_SPEED_STEPS, self.speed, up);
        self.elapsed_ms = 0;
    }

    /// §5.9.14: the frame moves with time. This is the whole clock, and it has
    /// no thread of its own because a [`PinItem`] is numbers and the window's
    /// timer is the thing that knows how long a frame has been up. `delays` is
    /// the decoded file's own per-frame pace, which lives with the picture
    /// rather than in here: this struct is what a restored session writes, and a
    /// frame list would not survive re-pasting a file.
    ///
    /// Returns whether the frame on screen moved. Paused, nothing accumulates.
    /// One tick may advance at most one lap of the clip, so a delta that arrives
    /// late - a window that was hidden, a machine that slept - is spent rather
    /// than owed.
    pub fn tick(&mut self, delta_ms: u32, delays: &[u32]) -> bool {
        if !self.playing || self.frames < 2 {
            self.elapsed_ms = 0;
            return false;
        }
        let mut spent = self.elapsed_ms.saturating_add(delta_ms);
        let mut moved = false;
        for _ in 0..self.frames {
            let due = self.delay_ms(delays);
            if spent < due {
                break;
            }
            spent -= due;
            self.frame = (self.frame + 1) % self.frames;
            moved = true;
        }
        self.elapsed_ms = if moved && spent >= self.delay_ms(delays) {
            0
        } else {
            spent
        };
        moved
    }

    /// The file's pace for the frame on screen, scaled by the user's speed and
    /// floored: a delay of 0 is a broken file, and 400% of a 1 ms frame is not a
    /// request to redraw forever.
    fn delay_ms(&self, delays: &[u32]) -> u32 {
        let base = match delays.get(self.frame) {
            Some(&d) if d >= GIF_MIN_FRAME_DELAY_MS => d,
            _ => GIF_DEFAULT_FRAME_DELAY_MS,
        };
        (u64::from(base) * 100 / u64::from(self.speed.max(1))).clamp(1, u64::from(u32::MAX)) as u32
    }

    /// §7.1: the one reader [`gif_interval`] is allowed to be given for the frame
    /// that is on screen. Exposed as a *read* of [`Self::delay_ms`] rather than as
    /// a second calculation so that "how long does this frame last" keeps exactly
    /// one author; a caller that needs the cadence asks the player, never the file.
    pub fn frame_delay_ms(&self, delays: &[u32]) -> u32 {
        self.delay_ms(delays)
    }
}

/// One monitor of [`Desktop`]: its rect in physical pixels, plus the ratio a
/// window placed on it needs to be sized in QML. The ratio lives here rather
/// than being read from a `Screen` attached object because the latter depends on
/// which screen a window ended up on, which depends on the window's size, which
/// depends on the ratio — a cycle. Sourced from the enumeration instead, the
/// chain is one-directional and stays physical until QML (§5.9.1, plan §6.1
/// "几何只用物理像素").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    pub rect: PhysRect,
    pub dpr: Scale,
}

impl Monitor {
    /// `Scale::of` re-clamps, so a driver's zero or NaN cannot reach a division.
    pub fn new(rect: PhysRect, dpr: Scale) -> Self {
        Self {
            rect,
            dpr: Scale::of(dpr.0),
        }
    }

    pub fn contains(&self, p: PhysPoint) -> bool {
        self.rect.contains(p)
    }
}

/// §5.9.1 — the screen layout a pin is allowed to be put on. A plain list of
/// rectangles, because a state machine must not need a display server to be
/// tested. `bounds` is the union of the monitors, in physical pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Desktop {
    pub bounds: PhysRect,
    /// Enumeration order. Index 0 is the primary by the convention the capture
    /// service already keeps (`CaptureService::monitors` sorts primary first),
    /// which is what lets a dead-space query resolve without a second field.
    pub monitors: Vec<Monitor>,
}

impl Desktop {
    pub fn new(bounds: PhysRect, monitors: Vec<Monitor>) -> Self {
        Self { bounds, monitors }
    }

    pub fn from_monitors(monitors: Vec<Monitor>) -> Self {
        let bounds = monitors
            .iter()
            .map(|m| m.rect)
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default();
        Self { bounds, monitors }
    }

    /// Slide `r` back so at least [`MIN_VISIBLE_PX`] of it stays inside `d`, the
    /// same arithmetic whether `d` is the union (§5.9.1, what a drag is clamped
    /// against) or one monitor (what §8.5's 主显示器 asks for). A rectangle that
    /// already qualifies comes back untouched, which is what makes a cross-monitor
    /// drag legal.
    fn slide_into(r: &PhysRect, d: PhysRect) -> PhysRect {
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

    pub fn keep_visible(&self, r: &PhysRect) -> PhysRect {
        Self::slide_into(r, self.bounds)
    }

    /// Is `r` on a screen at all — at least [`MIN_VISIBLE_PX`] of it inside **one**
    /// enumerated monitor? The union cannot answer this: three staggered screens
    /// make a box with a hole in it, and a rectangle parked in the hole is inside
    /// the box while nothing displays it.
    pub fn covered(&self, r: &PhysRect) -> bool {
        let min = MIN_VISIBLE_PX.min(r.w).min(r.h);
        self.monitors.iter().any(|m| {
            r.intersection(&m.rect)
                .is_some_and(|x| x.w >= min && x.h >= min)
        })
    }

    /// §8.5 第一句的后半 —— 「将**不可见**贴图移动到**主显示器**可见区域」。`r` is
    /// answered by [`Desktop::keep_visible`] first, so this only asks the question
    /// the union cannot: nothing here moves a pin that some monitor is showing,
    /// even one half of which hangs off the edge.
    ///
    /// `monitors[0]` is the primary by the convention the enumeration keeps; with
    /// no table at all the rect comes back untouched, because "where" has no answer
    /// that a caller did not already have.
    pub fn rescue_onto_primary(&self, r: &PhysRect) -> PhysRect {
        if self.covered(r) {
            return *r;
        }
        match self.monitors.first() {
            Some(m) => Self::slide_into(r, m.rect),
            None => *r,
        }
    }

    pub fn monitor_at(&self, p: PhysPoint) -> Option<usize> {
        self.monitors.iter().position(|m| m.contains(p))
    }

    /// The ratio a window placed at `p` is sized with. A point on no monitor —
    /// the dead space between two staggered screens, which §5.9.1 lets a pin be
    /// dragged through — still has to answer, because a pin with no ratio has no
    /// size: the primary's ratio is used, and `Scale::ONE` only when there is no
    /// table to consult at all.
    pub fn dpr_at(&self, p: PhysPoint) -> Scale {
        match self.monitor_at(p) {
            Some(i) => self.monitors[i].dpr,
            None => self.monitors.first().map(|m| m.dpr).unwrap_or(Scale::ONE),
        }
    }

    /// §8.5: a monitor was unplugged or the resolution changed, so a remembered
    /// position may now be off the edge, or inside the union but on no screen.
    /// Answers the same question [`PinItem::refit`] acts on.
    pub fn refits(&self, r: &PhysRect) -> bool {
        self.rescue_onto_primary(&self.keep_visible(r)) != *r
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

/// §5.8.5 步骤 4 那一格栅窗交回来的四格，与 §5.8.6 步骤 4 那一格颜色值。
///
/// `None` = 用户这一格没填 ⇒ 这一枚卡片在这一格上仍跟着 `[card]`。步骤 4 要的是「只改这一
/// 枚」而不是「把这一枚钉死在按下那一刻的四枚数上」：没填的格跟着档走，填过的格归这一枚。
/// 空格换成 `None` 是桥那一只手做的（它才知道哪一格是页上的哪一格），颜色那一扇门例外 ——
/// 它只有一格，空着就是「没说要改成什么」，那一串照样进来得让闸拒它（见
/// [`CardPin::merged`] 那一段）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardEdit {
    pub width: Option<u32>,
    pub font_size: Option<u32>,
    pub font_family: Option<String>,
    pub background: Option<String>,
}

impl CardEdit {
    /// 四格全空：那一格栅窗没说话。判在这儿而不是在桥或页上判，是因为「填没填」是这四格的
    /// 事实而不是哪一扇门的事实 —— 两处各判一遍就是两处对同一次按下说不同的话（#281）。
    pub fn is_empty(&self) -> bool {
        self.width.is_none()
            && self.font_size.is_none()
            && self.font_family.is_none()
            && self.background.is_none()
    }
}

/// 一次卡片重排交回的两样读数。
#[derive(Clone, Debug)]
pub struct CardEditReport {
    /// 重排出来的那张新图。调用方（`qt_bridge::state`）是像素的唯一持有者，所以图由这里交回
    /// 而不是由这里存下；几何那一半已经在 [`PinItem::edit_card`] 里搬到贴图身上了。
    pub frame: Frame,
    /// **用了的那张卡**：状态栏念的是这四枚读数而不是用户刚打的那四串字，因为闸可能已经夹过
    /// 其中一格，而念回去一个被夹掉的数就是念一句没发生过的话（#433/#455）。
    pub card: Card,
    /// 档的闸自己那句「这一格越界了、夹成这一枚数」。空 = 填进来的每一格都在界内。
    pub clamped: Vec<String>,
}

/// §5.8.5 步骤 4 与 §5.8.6 步骤 4–5 —— 一枚**卡片**贴图记得自己当初被要求说什么，以及它
/// 自己被调过的那几格。
///
/// 为什么要有这一格：贴图那一手把一段话排成卡片之后，窗上剩下的只是一张位图 —— 那句话、
/// 那四枚旋钮各自是谁，都回不去了。于是「调整这一枚卡片的宽度」在当时只有一种做法：改
/// `[card]` 再贴一次，而那改的是**每一张将来的**卡片、旧的那张一动不动。§5.8.5 步骤 4 与
/// §5.8.6 步骤 4 要的都是后者，所以这一格身份住在贴图身上。
///
/// 四枚旋钮是 `Option`：一张非卡片的贴图在这里是 `None`（整格不占钱），而一张刚贴进来的
/// 卡片四枚也全是 `None` —— 它说的就是 `[card]` 那一刻的话。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardPin {
    /// 卡片上的那些话。§5.8.6 的那一张写的就是它代表的那枚颜色值（[`crate::card::colour_text`]）。
    pub text: String,
    /// §5.8.6：`Some` = 这是一张**颜色**卡片 —— 色块就是这枚颜色，上面那一行就是它的值。
    /// 两者由同一次编辑写出（[`CardPin::merged`]），所以「块是 #A、行写着 #B」这一族在这一层
    /// 组不出来，而不是靠页上把两格填同步。alpha 走的是那一行：色块是不透明的那一枚。
    pub colour: Option<[u8; 4]>,
    /// §5.8.5 步骤 4 的四枚旋钮，只有被**这一枚**贴图调过的才在这里。
    pub width: Option<u32>,
    pub font_size: Option<u32>,
    pub font_family: Option<String>,
    pub background: Option<String>,
}

impl CardPin {
    /// 纯文本 / HTML 排成的那一张：话记住了，四枚旋钮一枚也没动过。
    pub fn of_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    /// §5.8.6 步骤 3 的那一张：色块是那枚颜色，那一行是它的值。
    pub fn of_colour(rgba: [u8; 4]) -> Self {
        Self {
            text: crate::card::colour_text(rgba),
            colour: Some(rgba),
            ..Default::default()
        }
    }

    /// 这一枚卡片**此刻**排版用的那张 `Card`：`[card]` 打底，自己被调过的那几格盖上去。
    ///
    /// 页上那格栅窗的初值读的就是这一枚，而不是 `[card]`：打开 A 卡片的窗看到 A 此刻的宽度，
    /// 看到 B 的数就是这一手在改 B（§5.8.5 步骤 4 的「调整这一枚」正对着这一句）。
    pub fn card_of(&self, base: &Card) -> Card {
        let mut card = base.clone();
        if let Some(w) = self.width {
            card.width = w;
        }
        if let Some(f) = self.font_size {
            card.font_size = f;
        }
        if let Some(f) = &self.font_family {
            card.font_family = f.clone();
        }
        if let Some(bg) = &self.background {
            card.background = bg.clone();
        }
        if let Some(rgba) = self.colour {
            // 颜色卡片的背景**就是**那枚颜色，它不与「背景颜色被单独调过」同时为真：一块 #A
            // 的色块配一行 #B 正是 §5.8.6 步骤 5 那句「同步更新」的反面。
            crate::card::colour_card(&card, rgba)
        } else {
            card
        }
    }

    /// §5.8.5 步骤 4 的那一手：并上刚填的那几格，让档的闸自己夹一遍，并把闸那句抱怨留给状态栏。
    /// 交回的是**新的身份**（`self` 不动 —— 被拒那一路 `edit_card` 就不提交，这一枚贴图一处
    /// 也不改）。
    fn merged(&self, base: &Card, edit: &CardEdit) -> Result<(Self, Vec<String>), CardError> {
        let mut want = self.clone();
        if let Some(w) = edit.width {
            want.width = Some(w);
        }
        if let Some(f) = edit.font_size {
            want.font_size = Some(f);
        }
        if let Some(f) = &edit.font_family {
            want.font_family = Some(f.clone());
        }
        if let Some(bg) = &edit.background {
            if want.colour.is_some() {
                // §5.8.6 步骤 4：颜色卡片上这一格**就是**那一格「颜色值」，读它的也正是
                // [`crate::colors::parse`] —— 读不出来就拒，命名刚打的那串，既不留在旧色上也不
                // 替用户想一枚。
                let rgba = colors::parse(bg).ok_or_else(|| CardError::Background(bg.clone()))?;
                want.colour = Some(rgba);
                // 步骤 5：色块与那一行是同一次编辑的两半。
                want.text = crate::card::colour_text(rgba);
                want.background = None;
            } else {
                want.background = Some(bg.clone());
            }
        }
        let candidate = want.card_of(base);
        // 颜色的那一半先问：闸会把读不出来的背景**换成出厂那一枚**（那是档的规矩 —— 一份手写的
        // 档总得开得了），而不是一次按下的规矩。用户刚打的串要被命名、要被拒，不能被换掉后
        // 装作是他自己填了白色。
        crate::card::background(&candidate)?;
        let (gated, said) = crate::card::gate(&candidate);
        // 只有**被夹过的那一格**才落成这一枚的 override：没夹的格仍跟着 `[card]`，于是「这格
        // 我没填」的那一枚数在用户改了档之后的下一手仍然跟着档走。
        if gated.width != candidate.width {
            want.width = Some(gated.width);
        }
        if gated.font_size != candidate.font_size {
            want.font_size = Some(gated.font_size);
        }
        if gated.background != candidate.background {
            want.background = Some(gated.background);
        }
        Ok((want, said))
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
    /// §5.5.8 步骤 3「系统按原截图坐标重新读取屏幕内容」里的那一枚**原截图坐标**: the point
    /// of the desktop this pin's source bitmap was read off.
    ///
    /// Only the legs whose pixels *are* a screen read set it — in this build that is the
    /// four presenters in `qt_bridge::capture` (`--rect`, `--window`, 活动窗口, 重复上次区域),
    /// all of which go through the bridge's `add_captured`. It stays the
    /// top-left of the rectangle that was read even after the window has been dragged
    /// somewhere else, because "where this picture came from" is a fact about the picture
    /// and not about where the window happens to sit — a refresh that followed
    /// [`PinItem::pos_phys`] would re-read whatever is under the pin now, which is a
    /// different requirement's answer. `None` is every other origin: a file, the clipboard,
    /// a text card, a restored session. Those pins have no coordinates to re-read, and
    /// saying so is [`crate::refresh::plan`]'s first arm rather than a silent no-op.
    ///
    /// The size is deliberately not stored here: [`PinItem::source_size`] is the shape of
    /// the same bitmap and nothing shrinks or grows it afterwards (§5.9.9 crops *inside*
    /// it), so a second copy would be two authorities on one fact — see
    /// [`PinItem::captured_rect`].
    pub captured_at: Option<PhysPoint>,
    /// §5.8.5 步骤 4 / §5.8.6 步骤 4–5：这一段话被排成卡片时的身份，与**这一枚**被调过的那几格。
    /// `None` = 这一枚贴图不是卡片排出来的（一次屏幕读取、一个文件、剪贴板里的一张图片），它没有
    /// 「当初被要求说什么」可记 —— 所以那一格栅窗对它也没有落点，`edit_card` 交回 `None`。
    pub card: Option<CardPin>,
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
            // A pin is not assumed to have come off the screen. The one writer is
            // `qt_bridge::state::PinState::add_captured`, and it writes it only for the
            // pixels a `capture` leg just handed over.
            captured_at: None,
            // 同上：一张新图不是一个「说法」。卡片那三臂（纯文本、HTML、颜色值）在
            // `qt_bridge::state` 里走 [`PinItem::edit_card`] 旁边那一只手写上这一格，
            // 别的来历在这一枚上永远是 `None`。
            card: None,
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

    /// §5.8.4 方式二「更新当前贴图」与 §7.2 崩溃恢复**共用**的那一步：把这一枚的来源换成另一张
    /// 图的形状，并让所有按**旧图**像素记着的坐标跟着装得进新图。
    ///
    /// 为什么这是一只手而不是两处 `if`：换来源的时刻有两个 —— 恢复时盘上那份文件已经被换成另一张
    /// 图（[`PinSet::read_state`] 里那句注释说的就是它），与用户拖进来一张新的（`drop` 那一支）。
    /// 两处要挡的坏是同一件：[`Self::src_rect`] 与自由缩略图那一格记的都是旧图的像素，新图更小就
    /// 会越界，而越界的现场是「这扇窗一开就白屏」或解码时的一枚 panic。规则只有字段上那一句 ——
    /// `src_rect` 永远装得进 `source_size`（见 [`Self::visible_rect`] 头顶）—— 所以它只该有一个
    /// 执行处：写第二份的那一天，两份会在「新图比旧图小多少」这一格上不一致。
    ///
    /// 交回 `Ok(true)` = 形状确实变了。调用方拿它决定其余那些**跟着形状走**、记着旧图像素的
    /// 那几格要不要一并收走（裁剪撤销栈、GIF 播放位：那些记的是旧图的坐标，来源一换就没了落点）。
    /// `Err` 只有一种：那张新图一枚像素也没有，来源不可能是空的 —— 与 [`Self::new`] 拒的是同一枚
    /// [`PinError::EmptyImage`]，所以「空图」这一判在恢复与拖入两条路上是**同一条**规则（话怎么说
    /// 由调用方按它那一侧的规矩翻：桥里给用户的句子向来不直接 forwarding 这一族的英文原句）。
    pub fn adopt_source(&mut self, source: PhysSize) -> Result<bool, PinError> {
        if source.is_empty() {
            return Err(PinError::EmptyImage);
        }
        if source == self.source_size {
            return Ok(false);
        }
        self.source_size = source;
        let whole = source.at(PhysPoint::new(0, 0));
        self.src_rect = self
            .src_rect
            .intersection(&whole)
            .filter(|r| !r.is_empty())
            .unwrap_or(whole);
        // 自由缩略图那一格装的是**同一套**像素（[`Self::visible_rect`] 在 Free 那一支交回它的
        // `rect`），所以它也得落在新的那一刀里 —— 与 [`Self::set_free_thumbnail`] 那句
        // 「clipped to what the crop kept」是同一条规矩，这里只是来源变了而不是刀变了。
        if let Some(t) = self.thumbnail {
            if t.mode == ThumbnailMode::Free {
                self.thumbnail = t
                    .rect
                    .intersection(&self.src_rect)
                    .filter(|r| !r.is_empty())
                    .map(|rect| Thumbnail { rect, ..t });
            }
        }
        // 撤销栈里记的是**旧图**的那几刀（[`Self::undo_crop`] 交回的就是其中一格）。换过来源还把
        // 它们留着，「撤销裁剪」按下之后恢复的是另一张图上的一格 —— 那一格在新图里没有落点，用户
        // 看到的是一枚凭空变了尺寸的裁剪。来源一换，那些刀就不再属于这一枚贴图的来历，栈在这里
        // 清空。留着比清空更坏的那一层理由是 PRD §5.8.4 那句「不得用低质量占位图静默替代」的同族：
        // 拿旧图的一格冒充新图的内容。
        self.crop_undo.clear();
        Ok(true)
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

    /// §5.5.8: the piece of the desktop [`PinItem::source_size`] was read off, or `None`
    /// when these pixels never came off a screen. Derived rather than stored because the
    /// rectangle is two facts the pin already keeps — where it was captured
    /// ([`PinItem::captured_at`]) and how big those pixels are — and a stored copy of the
    /// second one is a thing that can disagree with the bitmap it describes.
    ///
    /// It ignores [`PinItem::pos_phys`] by design: a dragged pin still came from the
    /// rectangle it was made in.
    pub fn captured_rect(&self) -> Option<PhysRect> {
        self.captured_at.map(|at| self.source_size.at(at))
    }

    /// §5.8.5 步骤 4「用户可调整卡片宽度、字号、字体和背景颜色」与 §5.8.6 步骤 4–5 那一手，
    /// 落点都在**这一枚**贴图身上：把刚填的那几格并到它自己的身份上，再按它记得的那句话重排。
    ///
    /// 三件事这一手管着，桥不另发明第二份：
    /// * 身份的合并与档的闸（[`CardPin::merged`] —— 界只有那一处作者，越界那句抱怨原样交回）；
    /// * 重排失败那一路**一处也不动**：那一串不是颜色、或排出来装不进一张位图，都还留着旧身份
    ///   与旧那张图，窗不会停在「说法是新的、画是旧的」那种中间地上；
    /// * 成功之后把新位图的形状搬到窗上（[`PinItem::adopt_card`]）。
    ///
    /// `None` = 这一枚贴图不是卡片排出来的。那不是错误而是「没有这一问」，与
    /// [`PinItem::gif`] 那五只手对一张静图答的同样是 `None`；那句话由门说
    /// （`qt_bridge::state::PinState::edit_card`），因为字面从来住在门上（§8.1）。
    pub fn edit_card(
        &mut self,
        base: &Card,
        edit: &CardEdit,
        glyphs: &dyn Glyphs,
    ) -> Option<Result<CardEditReport, CardError>> {
        let current = self.card.as_ref()?;
        let (want, clamped) = match current.merged(base, edit) {
            Ok(merged) => merged,
            Err(why) => return Some(Err(why)),
        };
        let card = want.card_of(base);
        let frame = match crate::card::render(&want.text, &card, glyphs) {
            Ok(frame) => frame,
            Err(why) => return Some(Err(why)),
        };
        self.card = Some(want);
        self.adopt_card(&frame);
        Some(Ok(CardEditReport {
            frame,
            card,
            clamped,
        }))
    }

    /// 重排交回的是**另一张位图**：指进旧排版的那几格在这一张上没有对应的位置 —— 裁剪框与它的
    /// 撤销栈是上一版排版的坐标，自由缩略图那块也是。留着它们，窗就压在一块已经不存在的内容上，
    /// 而「撤销裁剪」（§5.9.9）会交回一个谁也看不见的框。
    ///
    /// 与 [`PinSet::read_state`] 里「文件被换成了另一张图」那一臂同式（`source_size` 跟着新图走、
    /// `src_rect` 落回装得下的那一块），只是这里不必求交集：重排本来就是整张重来。
    fn adopt_card(&mut self, frame: &Frame) {
        self.source_size = PhysSize::new(frame.width, frame.height);
        self.src_rect = self.source_size.at(PhysPoint::new(0, 0));
        self.crop_undo.clear();
        self.thumbnail = None;
        self.sync_size();
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

    /// §8.5 — this pin against the layout that is there *now*: the union clamp
    /// first, through the same hand a drag goes through, and then the question the
    /// union cannot ask. 拔掉一块屏，剩下的两块上下错开时并集里是有死角的，而停在
    /// 死角里的贴图没有任何一台显示器在放它 —— 那一句要的是「搬到主显示器」，不是
    /// 「搬到盒子里」。Answers whether the position moved; only the position moves.
    pub fn refit(&mut self, desk: &Desktop) -> bool {
        let before = self.pos_phys;
        self.move_to(before, desk);
        self.pos_phys = desk.rescue_onto_primary(&self.window_rect()).top_left();
        self.pos_phys != before
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
    /// is the other's ratio, since the ratio decides it. The formula itself is
    /// [`Self::locked_zoom`], written once, which the dialog door below also calls.
    pub fn set_size_to(&mut self, w: Option<u32>, h: Option<u32>) -> Result<(), PinError> {
        let shown = self.shown_size();
        let pct = Self::locked_zoom(w.filter(|v| *v > 0), h.filter(|v| *v > 0), shown)
            .ok_or(PinError::NoSize)?;
        self.set_zoom(pct.round() as u32);
        Ok(())
    }

    /// §5.9.3 步骤 3/4 在菜单那一格上的门：三格文本（宽度、高度、缩放百分比）→ 一次真的落地。
    /// 页上一处算术也没有，桥只搬运，**等比锁定、上下界、非法输入三判全在这一个作者手里**
    /// ([`size_zoom_request`])，拒绝的那句话也是这里的（#281：一件事只有一个作者）。
    ///
    /// 交回的是**实际落在**的那一枚缩放（[`Self::set_zoom`] 之后 `self.zoom` 的本体），所以
    /// 按下之后页上读「现在是几 %」不必自己算，问这一只手要就行。
    pub fn set_size_from_text(
        &mut self,
        width: &str,
        height: &str,
        percent: &str,
    ) -> Result<u32, PinError> {
        let pct = size_zoom_request(width, height, percent, self.shown_size())?;
        self.set_zoom(pct);
        Ok(self.zoom)
    }

    fn zoom_for(want: u32, have: u32) -> f64 {
        want.max(1) as f64 * 100.0 / have.max(1) as f64
    }

    /// 等比锁定那一式，本档**唯一**一处：从 `shown`（此刻窗上那块的自然尺寸，
    /// [`Self::shown_size`]）出发，把「要装进宽 `w`、高 `h` 这一块里」读成一枚缩放。
    /// 两格都填取**更小**的那一枚，因为整张图都得装得下；只填一格就是那一格自己的比例，
    /// 另一格由锁定比例跟着走 —— 这正是 PRD 第 1006 行「系统按锁定比例设置更新结果」要的
    /// 那句话。两格都没填是 `None`，怎么说不归这里（[`PinItem::set_size_to`] 与
    /// [`size_zoom_request`] 各自的对面不是同一个人）。
    fn locked_zoom(w: Option<u32>, h: Option<u32>, shown: PhysSize) -> Option<f64> {
        let wf = w.map(|v| Self::zoom_for(v, shown.w));
        let hf = h.map(|v| Self::zoom_for(v, shown.h));
        match (wf, hf) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
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

    /// Put the rect back on the point it was standing around. Every hand that
    /// changes the *size* of a pin through a view change (`rotate` swaps w/h,
    /// `reset_view` returns the zoom to the default) owes the user this: the
    /// middle stays, the edges grow. Written once because two spellings of it
    /// drift — P134 found the second one missing.
    fn restick_about(&mut self, centre: PhysPoint) {
        let n = self.window_rect();
        self.pos_phys = PhysPoint::new(centre.x - (n.w as i32) / 2, centre.y - (n.h as i32) / 2);
    }

    /// §5.9.5 — a quarter turn about the centre, so the pin does not run off to
    /// one side every time the key is pressed.
    pub fn rotate(&mut self, quarter_turns: i32) {
        let r = self.window_rect();
        let centre = r.center();
        self.rotation = (self.rotation + quarter_turns * 90).rem_euclid(360);
        self.sync_size();
        self.restick_about(centre);
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

    /// §5.9.10 steps 1–3 driven by config: the box a fixed thumbnail is fitted
    /// into is the preset the user set in `[pin]`, read here rather than at the
    /// call site so the menu cannot pass an un-configured size.
    pub fn toggle_thumbnail_preset(&mut self, cfg: &PinConfig) -> bool {
        self.toggle_thumbnail(thumb_preset_box(cfg))
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
        // §5.9.10's one author of the fit. A thumbnail is 等比, not 定宽定高: the
        // box is an outer bound and the whole picture keeps its aspect ratio, so
        // `f` is the smaller of the two edges' ratios and both axes use it. QML
        // reads the resulting `size_phys` back and never runs this arithmetic
        // itself — a second copy there would be a second answer to "how big".
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
    /// because a reset is not a move — which for a size change means the **centre**
    /// stays where it was, the same rule [`Self::rotate`] and [`Self::reset_zoom`]
    /// follow. Holding the top-left instead was P134's finding: `zoomStep` shrinks
    /// about the centre, so `-` then 「重置视图」left the picture 16 px right and
    /// 10 px down from where the user had it, and the gauge's own round trip
    /// (`rst-lone-turned` → `reset_pin`) opened a window up to another row's
    /// geometry check.
    pub fn reset_view(&mut self, cfg: &PinConfig) {
        let centre = self.window_rect().center();
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
        self.restick_about(centre);
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

    /// §5.16.1 `pin_edit`「让**选中的贴图**进入标注编辑」的前半段：选中的那些枚在桌面上
    /// 合起来占的那一块。遮罩那一手要的洞是一枚 `PhysRect`，而在没有桌面的地方能答对的
    /// 只有这一半。
    ///
    /// 每一枚只问 [`PinItem::window_rect`]（pin.rs:784，`size_phys.at(pos_phys)`），这里
    /// **不再第二次**把 `pos_phys` 和 `size_phys` 拼起来：`size_phys` 不是用户写的数，而是
    /// [`PinItem::sync_size`]（pin.rs:780）从 [`PinItem::scaled_size`] 推下来的**实际**窗尺寸，
    /// 缩放（`zoom / 100`）、旋转（[`PinItem::shown_size`] 在 90°/270° 走
    /// [`PhysSize::quarter_turned`] 换边，pin.rs:760）、自由缩略图（[`PinItem::visible_rect`]
    /// 交回 `thumbnail.rect` 而不是 `src_rect`，pin.rs:739）三条改尺寸的路尽头都写着同一句
    /// `sync_size()`（pin.rs:833、933、1095、1116）。此刻两种写法答案相同，分叉要等到有人
    /// 往这三条里加第四条 —— 「这枚贴图在桌面上是哪一块」自此只有一个作者。
    ///
    /// **藏起来的那些枚不参与**，选中的全被藏起来时是 `None`，与「什么都没选中」同一句回答：
    /// [`PinItem::visible`]（pin.rs:800）为假就是 `solo_excluded` 或 `group_hidden`，而用户
    /// 看不见的一枚不可能被这张冻结拍到，让它撑大洞等于在照片上圈进一块根本没有它的地面。
    /// core 只问这一句：桥侧 `qt_bridge/src/state.rs` 的 `view_data` 里那一枚 `live` 还叠着
    /// 「被对话框临时压住」，那问的是此刻有没有人在盖它，不是用户自己有没有把它收起来。
    ///
    /// **跨屏不做特殊处理**：两枚坐在不同显示器上，并集就是一枚跨过中间那段死区的
    /// [`PhysRect`]，洞跟着跨过去 —— 遮罩与 [`PhysRect::fitted_into`]（geometry.rs:219）本来
    /// 就住在这一枚枚矩形里。这里既不裁成两块（那就不再是**一枚**洞），也不替谁让路。
    pub fn selected_bounds(&self) -> Option<PhysRect> {
        self.items
            .iter()
            .filter(|p| self.selection.contains(&p.id) && p.visible())
            .map(PinItem::window_rect)
            .reduce(|acc, r| acc.union(&r))
    }

    /// 这一枚堆顶的主人只有一位：[`Self::click_at`]（`cycle_on_click` 那一手，点着谁谁
    /// 站到最上）。§5.9.12「始终置顶」是每枚窗自己的 topmost 标志，由窗那侧落，与这里
    /// 的堆序无关；批量菜单（§5.10.4）那三只手是重置/关闭/销毁，没有「置于顶层」这一枚。
    pub fn raise(&mut self, id: PinId) -> bool {
        let Some(i) = self.items.iter().position(|p| p.id == id) else {
            return false;
        };
        let item = self.items.remove(i);
        self.items.push(item);
        true
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

    /// §5.10.3 — one rung of [`OPACITY_STEPS`], taken by each selected pin from
    /// **its own** current opacity. This is not `set_selection_opacity` with a
    /// value picked off the first pin: two pins at 30% and 80% that both get
    /// "more transparent" go to 20% and 70%, and flattening them onto one number
    /// would silently restyle the second one. Same rule as §5.10.2's 相同位移 — the
    /// batch shares the *adjustment*, never the resulting position or opacity.
    pub fn step_selection_opacity(&mut self, up: bool) -> usize {
        let ids = self.selected_ids();
        let mut n = 0;
        for id in ids {
            if let Some(p) = self.get_mut(id) {
                p.opacity_step(up);
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

    /// §8.5 — after a screen change: pull every pin back onto a screen.
    pub fn refit_all(&mut self, desk: &Desktop) -> usize {
        let mut n = 0;
        for p in &mut self.items {
            if p.refit(desk) {
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
    /// §5.8.5 步骤 4 / §5.8.6 步骤 4–5：这一枚卡片被要求说什么、它自己被调过的那几格。
    ///
    /// 这一格必须在档里：§7.2 的恢复交回的是**像素**，而卡片的重排要的是那句话 —— 快照记不住
    /// 它，恢复之后的那一枚就再也回不到「按宽度重排一次」那一手，卡片从此变成一张平图（页上那
    /// 四格也读不到初值）。TOML 没有 null，所以「这一枚不是卡片」写的就是**没有这一格**。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<CardPin>,
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
            card: pin.card.clone(),
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
            // `PersistedPin` has no `captured_at` row on purpose: §5.5.8's coordinates are
            // a claim about *this* desktop, and a pin read back off a file in a later
            // process has no screen read behind it to point at.
            captured_at: None,
            // 卡片那一格则**有**自己的行：那句话与那几枚 override 不是关于这块桌面的说法，而是
            // 关于这一段被排成图的文字，恢复之后还要靠它重排（见 `PersistedPin::card` 那封信）。
            card: self.card.clone(),
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
    /// Which pins those two counts name. The counts answer "how many", these
    /// answer "which one do I go look at" - and §7.2's promise that one bad
    /// picture changes nothing else is only checkable per id.
    pub missing_ids: Vec<PinId>,
    pub corrupt_ids: Vec<PinId>,
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
                out.missing_ids.push(p.id);
                continue;
            };
            if !path.is_file() {
                out.dropped_missing += 1;
                out.missing_ids.push(p.id);
                continue;
            }
            let frame = match encode::decode_file(&path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("pin {} would not decode: {e}", item.id);
                    out.dropped_corrupt += 1;
                    out.corrupt_ids.push(item.id);
                    continue;
                }
            };
            // The file may have been replaced by a different picture since.
            let mut item = item;
            if let Err(e) = item.adopt_source(PhysSize::new(frame.width, frame.height)) {
                // 一份没有像素的文件与一份译不出来的文件是同一族坏：这一枚没有可以画的来源，
                // 说的也就是同一句话（同一枚 [`PinError::EmptyImage`]，与 [`PinItem::new`] 拒的
                // 那一枚同一只）。
                tracing::warn!("pin {} has no pixels to restore: {e}", item.id);
                out.dropped_corrupt += 1;
                out.corrupt_ids.push(item.id);
                continue;
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

// ------------------------------------------------------------------ §7.2 sessions

fn session_path(dir: &Path) -> PathBuf {
    dir.join(SESSION_FILE)
}

/// What the startup check found about the run *before* this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrevRun {
    /// No stamp: a first run in this directory, or the previous run reached the
    /// clean-exit door [`finish_clean`].
    Clean,
    /// The stamp was still there, so the run that wrote it never finished -
    /// crash, kill, power loss. This is the only case §7.2 restores for.
    Crashed,
}

/// Lay down this run's stamp and report what the previous one says.
///
/// The file exists *while a run is live* and holds `pid` plus the start stamp -
/// not for liveness (this module asks no OS), but so a person reading the data
/// directory after a crash can tell which run died. The stamp is written through
/// [`write_atomic`] for the same reason the snapshot is: a power loss that eats
/// the stamp would silently disable recovery, and §7.2 promises it.
pub fn open_session(dir: &Path, pid: u32, now_ms: i64) -> Result<PrevRun, PinError> {
    let path = session_path(dir);
    let prev = if path.is_file() {
        PrevRun::Crashed
    } else {
        PrevRun::Clean
    };
    write_atomic(
        &path,
        format!("pid = {pid}\nstarted_at = {now_ms}\n").as_bytes(),
    )?;
    Ok(prev)
}

/// Take this run's stamp back. Public because a caller that only wants to close
/// the session without touching the snapshot (an aborted gauge run, say) should
/// ask for exactly that.
pub fn close_session(dir: &Path) -> Result<(), PinError> {
    match std::fs::remove_file(session_path(dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PinError::Io(e)),
    }
}

/// What this run should do first, decided in one place so no caller can hold a
/// second opinion on §7.2.
#[derive(Debug)]
pub struct Startup {
    pub prev: PrevRun,
    /// The pins to rebuild. Always `None` after a run that ended cleanly - which
    /// is what keeps "用户关得掉的贴图，下次启动不再回来" true even when the
    /// snapshot on disk still lists them.
    pub restore: Option<Restored>,
}

/// The startup door: [`open_session`], then read the snapshot back **only if the
/// previous run was interrupted**. A clean stamp pair means the pins on disk are
/// the pins the user dismissed, and both the state file and its pixel cache go
/// away with them (§5.9.16/17: the snapshot's pixels must not outlive their pins).
pub fn plan_startup(dir: &Path, pid: u32, now_ms: i64) -> Result<Startup, PinError> {
    let prev = open_session(dir, pid, now_ms)?;
    let restore = match prev {
        PrevRun::Crashed => PinSet::read_state(dir)?,
        PrevRun::Clean => {
            PinSet::clear_state(dir)?;
            None
        }
    };
    Ok(Startup { prev, restore })
}

/// The clean-exit door: this run ended as the user meant it to end. Close the
/// session so the next start restores nothing, and drop the snapshot with its
/// cache so nothing of this session outlives it.
pub fn finish_clean(dir: &Path) -> Result<(), PinError> {
    close_session(dir)?;
    PinSet::clear_state(dir)
}

// ------------------------------------------------- §5.8.3 从图片文件创建贴图那一批

/// 一次「打开图片并贴图」最多收下几枚文件。
///
/// 这一枚上界不是为了快：每一枚成了图的文件就是一扇窗、一份纹理、名册里的一格，而 §5.8.3
/// 第 2 步那一句「选择图片文件」在一台资源管理器里是可以全选一整 folder 的 —— 三千枚。把
/// 前 [`FILE_PIN_MAX`] 枚贴出来、其余**逐枚说一句**，是这一节唯一两种坏里较轻的那一种；
/// 静默丢下后两千九百枚是较重的那一种（#433「不许静默」）。
pub const FILE_PIN_MAX: usize = 32;

/// 「这一批太大了」那一句的**唯一**作者。
///
/// 为什么要把它单独立出来：这一枚上界有两批读者 —— §5.8.3 那一批挑出来的文件，与 §5.8.4 那一
/// 批拖进贴图窗口的格子（文件与网页地址混在一起）。两批要挡的是同一件事（`FILE_PIN_MAX` 之后
/// 的一枚连读都不读），差别只在主语。写成 `what` 一枚参数而不是两处各拼一遍，是因为「一次最多
/// 几枚」这枚数将来只会改一处，而那句「再挑少一点」的口径不该在两批之间漂移（#281）。
pub fn too_many_why(shown: &str, what: &str) -> String {
    format!("{shown} 没有{what}：一次最多贴 {FILE_PIN_MAX} 枚，这一批超出了——再挑少一点。")
}

/// 一次点选多枚时，相邻两枚落位点相差的物理像素。
///
/// 这个数不是新发明的：它是 [`crate`] 之外那一只 `next_drop_point()` 在「这台机器问不到指针」
/// 的分支里已经用着的同一枚 `step * 16`（`qt_bridge/src/state.rs`）。两枚都写 16 而只有一个人
/// 拥有它，就是 #281 等着的那次分家，所以这里把它立成一名，让**每一批**都从这一处读。
pub const CASCADE_STEP_PX: i32 = 16;

/// 那一句屏幕上飘的话里，最多原样引几枚坏文件的说法。
///
/// 每一枚坏文件的说法都带它自己的完整路径，而浮窗那一格读的是**一句话**：三枚之后它就不像
/// 一句提示、像一段日志。多出来的那些由调用方逐行记进自己的历史（`hotkey.rs` 的 `note`），
/// 这一枚常量管的只是屏幕上那一行。
pub const FILE_PIN_SAID_MAX: usize = 3;

/// §5.8.3「格式要求」那一列的七种，写成对话框认的过滤器模式。
///
/// 唯一的作者是 [`encode::IMPORT_FORMATS`]：这一枚函数不列清单，它把那七枚各自 `ext()`
/// 拼起来。于是 §5.8.3 那一列将来加一种时，需要改的地方只有一处，而这一枚函数、托盘那一项
/// 背后的过滤器、以及「本程序会读哪几种」那句话（`state.rs` 的 `read_refusal`）跟着一起改口。
///
/// 交回的是 `*.png;*.jpg;…` 这一串（Windows 那一张对话框的分号分隔法），不是七个元素。
pub fn picture_filter_patterns() -> String {
    encode::IMPORT_FORMATS
        .iter()
        .map(|f| format!("*.{}", f.ext()))
        .collect::<Vec<_>>()
        .join(";")
}

/// 一次点选多枚时第 `index` 枚该站在哪里。
///
/// `base` 是**既有**的落位规则交回的那一枚点（贴图落在指针旁边，见 `next_drop_point()`），
/// 本函数只加一个阶梯。不加这个阶梯的话，一次选三枚得到的是三扇**完全重叠**的窗：名册里确实
/// 是三枚、`windows()` 也确实是 3，而屏幕上是一扇 —— 那是「数量归名册管」这一条最容易糊过去
/// 的坏法，所以这里量的是坐标而不是数目。
///
/// 阶梯往右下走，且一路 `saturating_add`：`i32::MAX` 附近的一枚点不该把这一步加成一次绕回
/// 屏幕另一头的回卷。
pub fn cascade_at(base: PhysPoint, index: usize) -> PhysPoint {
    let step = (index as i64).saturating_mul(CASCADE_STEP_PX as i64);
    let add = |v: i32| -> i32 { (v as i64 + step).clamp(i32::MIN as i64, i32::MAX as i64) as i32 };
    PhysPoint::new(add(base.x), add(base.y))
}

/// 一批里**没有**成图的那一枚：路径，和给它的那一句说法。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePinFailure {
    /// 用户看得见的那串名字，原样留着（它在对话框里就是这么写的）。
    pub path: String,
    /// 这一枚自己的那句话。作者不在本模块 —— 见 [`read_picture_batch`] 的 `read`。
    pub why: String,
}

/// 一次「打开图片并贴图」交回的账。
///
/// 三格各是两种坏里的一种与好里的一种，不许混：**成了几枚**、**读了却没读出来的**、
/// **多到不收的**。第二格与第三格在屏幕上都是「没有贴出来」，但它们是两件事（一枚是坏档或
/// 本程序不认的格式，一枚是这一批压根排不进上界），混成一格就说不出「再挑少一点」这句实话。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilePinBatch {
    /// 成了图的那几枚：`(路径, 名册 id)`。**窗的数目就读在这一格的长度上**，不读在任何一句
    /// 散文里 —— 一次选三枚、三枚都好，这一格就是三条。
    pub pinned: Vec<(String, PinId)>,
    /// 递进去、读不出来的那几枚，各带一句说法。
    pub failed: Vec<FilePinFailure>,
    /// 超出 [`FILE_PIN_MAX`] 而被丢下的那几枚，各带一句说法。
    pub declined: Vec<FilePinFailure>,
}

impl FilePinBatch {
    /// 这一次贴出了几扇窗。
    pub fn windows(&self) -> usize {
        self.pinned.len()
    }

    /// 这一次有几枚没有成。
    pub fn trouble(&self) -> usize {
        self.failed.len() + self.declined.len()
    }

    /// 屏幕上那一句话。
    ///
    /// 它是这一批唯一的读数出口，所以三种情形都得有自己的说法而不是共用一句：一枚也没挑
    /// （用户按了取消）、全成了、成了几枚又坏了几枚。第二句里那些原因出自被递进来的那一只
    /// `read`，本模块不替它们改写一个字（#281）。
    pub fn said(&self) -> String {
        let ok = self.pinned.len();
        let bad = self.trouble();
        if ok == 0 && bad == 0 {
            return "没有选中文件，一张也没有贴。".to_string();
        }
        let mut line = if ok == 0 {
            format!("{bad} 枚都没有贴出来")
        } else if bad == 0 {
            format!("已贴出 {ok} 张贴图")
        } else {
            format!("已贴出 {ok} 张贴图，另有 {bad} 枚没有贴出来")
        };
        let quotes: Vec<&str> = self
            .failed
            .iter()
            .chain(self.declined.iter())
            .take(FILE_PIN_SAID_MAX)
            .map(|f| f.why.as_str())
            .collect();
        if !quotes.is_empty() {
            line.push('：');
            line.push_str(&quotes.join("；"));
            let rest = bad.saturating_sub(quotes.len());
            if rest > 0 {
                line.push_str(&format!("…其余 {rest} 枚见运行历史"));
            }
        }
        line
    }
}

/// §5.8.3 第 3 步那一「批」的账本，也是这一节最后那一句（「文件损坏或格式不支持时应显示
/// 错误信息」）与 §7.2（「单张图片解码失败不得导致整个应用退出」）共用的那一手。
///
/// 为什么这一枚函数在 core 而不是在桥里：它判的三件事 —— **收几枚**、**坏的那几枚怎么归账**、
/// **一次几枚** —— 全都不是「怎么读字节」，而那正是本仓反复出现的那一族缺陷（逻辑齐备、生产
/// 路径零调用）最容易藏身的形状：把它放在桥里，它就只会在一扇真对话框后面被执行一次，而那一
/// 次没有人判过它。放在这里，「好/坏/格式不支持」一批递进来是跑得动的单测。
///
/// `read` 是「把一枚路径读成贴图」这一手，由调用方递进来：桥递的是真的读盘 + 建窗，单测递的
/// 是一句假说法。**这一枚函数不开对话框、不碰 Qt、也不读一个字节**，所以它能在没有屏幕的
/// 机器上被问一遍。
///
/// 一条硬规则，写在这里而不是写在注释里等有人去读：`read` 交回 `Err` 时本函数**只记那一句，
/// 然后接着读下一枚**。§7.2 那一半与 §8.1 那一半要的都是这一条 —— 一枚坏档不该吃掉它后面
/// 那几枚好档。
///
/// 递给 `read` 的第二枚参数是这一枚在本批里的**序号**（从 0 起），它存在的理由只有一个：
/// 一次挑三枚时落位点要按 [`cascade_at`] 排开，而那个阶梯认的是「第几枚挑的」而不是「第几枚
/// 成功了」—— 坏掉的那一枚也占一格阶梯，于是用户看到的间距与他挑的顺序一致，不会因中间一枚
/// 读失败而把后面几枚往前挤成重叠。本模块自己不用这个序号。
pub fn read_picture_batch(
    paths: &[PathBuf],
    mut read: impl FnMut(&Path, usize) -> Result<PinId, String>,
) -> FilePinBatch {
    let mut out = FilePinBatch::default();
    for (index, path) in paths.iter().enumerate() {
        let shown = path.display().to_string();
        if index >= FILE_PIN_MAX {
            // 被上界挡下的那些一枚也不读：读进来也只是先建出一扇窗、再当着用户把它收回，
            // 而这一格要的答复是「这批太大，再挑少一点」。
            out.declined.push(FilePinFailure {
                why: too_many_why(&shown, "贴出来"),
                path: shown,
            });
            continue;
        }
        match read(path, index) {
            Ok(id) => out.pinned.push((shown, id)),
            Err(why) => out.failed.push(FilePinFailure { path: shown, why }),
        }
    }
    out
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
        Desktop::new(
            PhysRect::new(0, 0, w, h),
            vec![Monitor::new(PhysRect::new(0, 0, w, h), Scale::ONE)],
        )
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

    // ---------------------------------------------------- §5.8.5 步骤 4 / §5.8.6 步骤 4–5

    /// 没有字体可问的那一位提供者（[`crate::annotation::raster`] 在同一种情况下只画背景）。
    /// 这一族判据全部用它：本档要证的是**尺寸与颜色**跟着刚填的那几格走，而那两样不需要有
    /// 人量过一行字；发明一行高度反而会把「下一次真机上那一眼看到的行数」写成这里的读数。
    const NO_FONT: crate::annotation::raster::NoGlyphs = crate::annotation::raster::NoGlyphs;

    /// 一张被贴进来的卡片：贴图本身，与它那一刻的像素（快照那一手要的是后者）。
    fn card_pin_frame(id: PinId, card: CardPin, base: &Card) -> (PinItem, Frame) {
        let frame = crate::card::render(&card.text, &card.card_of(base), &NO_FONT).expect("card");
        let mut p = PinItem::from_frame(id, &frame, PhysPoint::new(0, 0), &cfg()).expect("pin");
        p.card = Some(card);
        (p, frame)
    }

    fn card_pin(id: PinId, card: CardPin, base: &Card) -> PinItem {
        card_pin_frame(id, card, base).0
    }

    #[test]
    fn a_pin_that_is_not_a_card_has_no_card_to_re_layout() {
        let mut p = pin(40, 20);
        assert_eq!(p.card, None, "一张普通贴图不该带着一份卡片身份");
        // `None` 而不是一枚错误：这一枚压根没有「当初被要求说什么」可改，而那句「这不是卡片」
        // 由门说（`qt_bridge::state::PinState::edit_card`），字面从来住在门上（§8.1）。
        let edit = CardEdit {
            width: Some(700),
            ..Default::default()
        };
        assert!(p
            .edit_card(&Card::default(), &edit, &NO_FONT)
            .is_none());
        assert_eq!(p.source_size, PhysSize::new(40, 20), "答「没有这一问」的那一手把窗也改了");
    }

    /// §5.8.5 步骤 4 的那一句「用户可调整卡片宽度、字号、字体和背景颜色」，在这一枚贴图身上
    /// 判三件事：这一枚跟着走、**另一枚**一动不动、`[card]` 那一份档也被原样留着（改的是这一张
    /// 而不是「每一张将来的卡片」—— 后者是设置页那一扇门，早就在场）。
    #[test]
    fn adjusting_one_card_leaves_the_other_card_and_the_file_alone() {
        let base = Card::default();
        let mut a = card_pin(1, CardPin::of_text("第一张卡片"), &base);
        let b = card_pin(2, CardPin::of_text("第二张卡片"), &base);
        assert_eq!(a.source_size, b.source_size, "两张卡当初排的是同一枚宽度");

        let report = a
            .edit_card(
                &base,
                &CardEdit {
                    width: Some(700),
                    font_family: Some("Arial".into()),
                    ..Default::default()
                },
                &NO_FONT,
            )
            .expect("A 是卡片")
            .expect("A 收下了这一手");
        assert_eq!(report.frame.width, 700, "A 没按刚填的宽度重排：{:#?}", report.card);
        assert_eq!(report.card.font_family, "Arial", "字体那一格没落到这张卡上：{:#?}", report.card);
        assert_eq!(a.source_size.w, 700, "A 的位图没跟着换：{:#?}", a.source_size);
        assert_eq!(a.size_phys.w, 700, "A 的窗没跟着换：{:#?}", a.size_phys);
        assert_eq!(
            a.card.as_ref().expect("A 的身份").text,
            "第一张卡片",
            "重排要按**记得的那句话**再来一遍，而不是按空话"
        );

        assert_eq!(b.source_size.w, base.width, "B 的宽度跟着 A 的那一手变了");
        assert_eq!(
            b.card.as_ref().expect("B 的身份").width,
            None,
            "B 身上多出一枚只有 A 才填过的 override"
        );
        assert_eq!(base, Card::default(), "改一张卡片把 [card] 也改写了");
    }

    /// 没填的那一格仍跟着 `[card]`：这一步判的是 `Option` 那层的理由本身 —— 卡片不是被钉死在
    /// 按下那一刻的四枚数上，而是「填过的归这一枚、没填的归档管」。
    #[test]
    fn the_knob_that_was_left_blank_keeps_following_the_file() {
        let base = Card::default();
        let mut p = card_pin(1, CardPin::of_text("一句话"), &base);
        p.edit_card(
            &base,
            &CardEdit {
                width: Some(700),
                ..Default::default()
            },
            &NO_FONT,
        )
        .expect("卡片")
        .expect("收下");

        let remembered = p.card.as_ref().expect("身份");
        assert_eq!(remembered.width, Some(700), "填过的那一格该留在这一枚身上");
        assert_eq!(remembered.font_size, None, "没填的那一格不该被落成 override");
        assert_eq!(remembered.font_family, None, "同上：字体");
        assert_eq!(remembered.background, None, "同上：背景");

        // 档把字号改了 ⇒ 这一枚的下一次重排跟着档走，而宽度仍是它自己那枚 700。
        let moved_file = Card {
            font_size: 40,
            ..Card::default()
        };
        let report = p
            .edit_card(&moved_file, &CardEdit::default(), &NO_FONT)
            .expect("卡片")
            .expect("重排");
        assert_eq!(
            (report.frame.width, report.frame.height),
            (700, 40),
            "宽度跟着这一枚、字号跟着档：那两枚数该同时成立"
        );
    }

    /// 越界的那一格由**档的那道闸**夹，并把它自己那句抱怨交回状态栏：界不在这里抄第二份
    /// （#281），而「悄悄夹掉」也不干 —— 用户打的是 1 000 000，读回来的是 4096 与那一句为什么。
    #[test]
    fn a_card_knob_out_of_range_comes_back_clamped_with_the_gate_saying_so() {
        let base = Card::default();
        let mut p = card_pin(1, CardPin::of_text("一句话"), &base);
        let report = p
            .edit_card(
                &base,
                &CardEdit {
                    width: Some(1_000_000),
                    font_size: Some(500),
                    ..Default::default()
                },
                &NO_FONT,
            )
            .expect("卡片")
            .expect("闸门接下了这一手");
        assert_eq!(
            (report.card.width, report.card.font_size),
            (4096, crate::annotation::model::FONT_MAX),
            "越界的两枚没按档的那两枚界夹回来：{:#?}",
            report.card
        );
        assert_eq!(
            report.frame.width, report.card.width,
            "窗上那张图用的不是夹过的那一枚数"
        );
        assert_eq!(
            report.clamped.len(),
            2,
            "闸没有逐格说它夹了什么：{:#?}",
            report.clamped
        );
        for needle in ["card.width", "card.font_size"] {
            assert!(
                report.clamped.iter().any(|w| w.contains(needle)),
                "状态栏里读不到 {needle} 那一句：{:#?}",
                report.clamped
            );
        }
        // 落成 override 的只有**被夹过**的那两格；用户没填、也没被夹的那两格仍跟着 `[card]`。
        let remembered = p.card.as_ref().expect("身份");
        assert_eq!(remembered.width, Some(4096), "夹过的宽度没记下来");
        assert_eq!(remembered.font_family, None, "没人填过的一格被写进了身份");
    }

    /// §5.8.6 步骤 4「修改颜色值」与步骤 5「卡片颜色和显示文本同步更新」：一次编辑写下的是
    /// **两半**，色块与那一行，而它们中间没有让用户自己对账的那一步。
    #[test]
    fn a_colour_card_moves_its_swatch_and_its_value_line_together() {
        let base = Card::default();
        let red = [0xe8, 0x11, 0x23, 0xff];
        let mut p = card_pin(1, CardPin::of_colour(red), &base);
        assert_eq!(p.card.as_ref().expect("身份").text, "#E81123");

        let report = p
            .edit_card(
                &base,
                &CardEdit {
                    background: Some("#00ff00".into()),
                    ..Default::default()
                },
                &NO_FONT,
            )
            .expect("颜色卡片也是卡片")
            .expect("这一串是颜色");
        let green = [0x00, 0xff, 0x00, 0xff];
        assert_eq!(report.frame.get(0, 0), green, "色块没换成刚填的那枚颜色");
        let remembered = p.card.as_ref().expect("身份");
        assert_eq!(remembered.colour, Some(green), "身份里那枚颜色还是旧的");
        assert_eq!(remembered.text, "#00FF00", "那一行没跟着色块走：{}", remembered.text);
        assert_eq!(
            remembered.background, None,
            "颜色卡片的背景由那枚颜色写着，不该另存一枚 override 与它争"
        );
        assert_eq!(
            report.card.background, "#00FF00",
            "重排用的那张卡的背景不是新色：{:#?}",
            report.card
        );
    }

    /// 带 alpha 的那一枚：色块是不透明的写法、那一行写满八位 —— 两半各按自己的规矩，而规矩只有
    /// [`crate::card::colour_text`] / [`crate::card::colour_card`] 两名作者。
    #[test]
    fn a_half_transparent_colour_writes_its_alpha_on_the_line_not_the_swatch() {
        let base = Card::default();
        let rgba = [0xab, 0xcd, 0xef, 0x80];
        let p = card_pin(1, CardPin::of_colour(rgba), &base);
        assert_eq!(p.card.as_ref().expect("身份").text, "#ABCDEF80");
        assert_eq!(
            p.card.as_ref().expect("身份").card_of(&base).background,
            "#ABCDEF",
            "色块那一格写上了 alpha，就成了谁也对不上的第三串字"
        );
    }

    /// §5.8.6 步骤 4 的拒：读不出来的那一串要被**点名**交回，这一枚卡片一处也不动 —— 既不悄悄
    /// 留着旧色当成功，也不替用户想一枚颜色。
    #[test]
    fn a_colour_value_nobody_can_read_is_refused_with_the_string_it_was_given() {
        let base = Card::default();
        let mut typed = "papercolor".to_string();
        for raw in ["papercolor", ""] {
            let mut p = card_pin(1, CardPin::of_colour([0x12, 0x34, 0x56, 0xff]), &base);
            let before_size = p.size_phys;
            let before_text = p.card.as_ref().expect("身份").text.clone();
            typed = raw.to_string();
            let why = p
                .edit_card(
                    &base,
                    &CardEdit {
                        background: Some(raw.to_string()),
                        ..Default::default()
                    },
                    &NO_FONT,
                )
                .expect("颜色卡片也是卡片")
                .expect_err("读不出来的那一串必须被拒");
            assert!(
                matches!(&why, CardError::Background(s) if *s == typed),
                "拒语没有点名刚打的那串 {raw}：{why:?}"
            );
            let after = p.card.as_ref().expect("身份");
            assert_eq!(after.text, before_text, "拒了，那一行却被换掉了：{}", after.text);
            assert_eq!(p.size_phys, before_size, "被拒的这一手把窗也改了");
        }
    }

    /// 同一枚拒语在**纯文本**卡片上也成立：背景那一格读不出来时，闸会把档里那枚出厂白色填进去
    /// （那是一份手写的档要开得了的规矩），而这不是一次按下的规矩 —— 所以颜色那一半先问
    /// [`crate::card::background`]，问不出就直接拒。
    #[test]
    fn a_text_card_refuses_an_unreadable_background_instead_of_getting_white() {
        let base = Card::default();
        let mut p = card_pin(1, CardPin::of_text("一段话"), &base);
        let why = p
            .edit_card(
                &base,
                &CardEdit {
                    background: Some("papercolor".into()),
                    ..Default::default()
                },
                &NO_FONT,
            )
            .expect("卡片")
            .expect_err("那一串不是颜色");
        assert!(
            matches!(&why, CardError::Background(s) if s == "papercolor"),
            "{why:?}"
        );
        let remembered = p.card.as_ref().expect("身份");
        assert_eq!(remembered.background, None, "被拒的那串落成了这一枚的背景");
        assert_eq!(
            remembered.colour, None,
            "纯文本卡片不该因为填过背景那一格就变成颜色卡片"
        );
    }

    /// 颜色卡片身上那枚**手写的**背景 override 争不过它自己的颜色：色块与那一行是一件事的两半，
    /// 组出「块是 #A、行写着 #B」的那一扇窗就是 §5.8.6 步骤 5 的反面。
    #[test]
    fn a_colour_card_has_one_colour_rather_than_two_claims() {
        let split = CardPin {
            text: "#E81123".into(),
            colour: Some([0xe8, 0x11, 0x23, 0xff]),
            background: Some("#0000FF".into()),
            ..Default::default()
        };
        let effective = split.card_of(&Card::default());
        assert_eq!(
            effective.background, "#E81123",
            "背景那一格的 override 顶掉了这枚颜色本身：{effective:?}"
        );
    }

    /// 重排交回的是另一张位图：指进上一版排版的那几格（裁剪框、它的撤销栈、自由缩略图那块）在
    /// 这一张上没有对应的位置，留着就是让窗压在一块已经不存在的内容上（§5.9.9 的撤销要撤销得动）。
    #[test]
    fn a_re_laid_out_card_drops_the_regions_that_pointed_into_the_old_layout() {
        let base = Card::default();
        let mut p = card_pin(1, CardPin::of_text("一句话"), &base);
        let first = p.source_size;
        p.crop(&PhysRect::new(0, 0, first.w / 2, first.h / 2))
            .expect("crop");
        p.set_free_thumbnail(&PhysRect::new(0, 0, 4, 4)).expect("thumb");
        p.set_zoom(200);
        let before = (p.src_rect, p.crop_undo.len());

        let report = p
            .edit_card(
                &base,
                &CardEdit {
                    width: Some(700),
                    ..Default::default()
                },
                &NO_FONT,
            )
            .expect("卡片")
            .expect("收下");
        assert_eq!(
            p.src_rect,
            p.source_size.at(PhysPoint::new(0, 0)),
            "重排之后窗还压在上一版排版的那一块上：{before:?}"
        );
        assert!(
            p.crop_undo.is_empty(),
            "撤销栈里留着谁也看不见的框：{:#?}",
            p.crop_undo
        );
        assert_eq!(p.thumbnail, None, "自由缩略图那块是新图上没有的坐标");
        assert_eq!(
            p.source_size,
            PhysSize::new(report.frame.width, report.frame.height),
            "位图的形状与身份对不上"
        );
        assert_eq!(
            p.size_phys,
            PhysSize::new(report.frame.width * 2, report.frame.height * 2),
            "200% 缩放该跟着新图幅走：{:#?}",
            p.size_phys
        );
    }

    /// 一张非卡片的贴图在这里花的钱就是 `Option` 那一枚指针：`None`，而快照也不为它多写一行
    /// （§7.2 那一份档的形状是它的文档，见 [`PersistedPin::card`]）。
    ///
    /// 尺子两头都问一遍（#269）：只问「普通贴图那一行不在」的判据在尺子读不出「在」的时候照样
    /// 是绿的，所以同一趟里再钉一枚卡片进去、读回它那一行**确实在**。
    #[test]
    fn a_plain_pin_pays_nothing_for_the_card_identity_and_writes_no_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = PinSet::default();
        s.add(pin_at(1, 30, 20, PhysPoint::new(0, 0)));
        s.write_state(dir.path(), &mut no_frames, 1).expect("write");
        let raw = std::fs::read_to_string(dir.path().join(STATE_FILE)).expect("read");
        assert!(
            !raw.contains("card =") && !raw.contains("[pins.card]"),
            "一张普通贴图被写进了一行卡片身份：{raw}"
        );

        let base = Card::default();
        let (words, frame) = card_pin_frame(2, CardPin::of_text("一句话"), &base);
        let mut s = PinSet::default();
        s.add(words);
        let mut grab = |_id: PinId| -> Option<Frame> { Some(frame.clone()) };
        s.write_state(dir.path(), &mut grab, 1).expect("write");
        let raw = std::fs::read_to_string(dir.path().join(STATE_FILE)).expect("read");
        assert!(
            raw.contains("card =") || raw.contains("[pins.card]"),
            "这一枚是卡片，而档里读不出它那一行——上面那一问因此是空集上的真：{raw}"
        );
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
        let a = Monitor::new(PhysRect::new(0, 0, 1000, 600), Scale::ONE);
        let b = Monitor::new(PhysRect::new(1000, 0, 1000, 600), Scale::of(2.0));
        let desk = Desktop::from_monitors(vec![a, b]);
        assert_eq!(desk.bounds, PhysRect::new(0, 0, 2000, 600));
        assert_eq!(desk.monitor_at(PhysPoint::new(1500, 10)), Some(1));
        assert_eq!(desk.monitor_at(PhysPoint::new(10, 10)), Some(0));
        assert_eq!(desk.monitor_at(PhysPoint::new(3000, 10)), None);
    }

    // The window's `dpr` is answered from this table, so a per-screen ratio has
    // to survive the trip and a point on no screen has to answer anyway: an
    // unknown ratio is an unsizable pin (plan §6.1 "几何只用物理像素").
    #[test]
    fn the_ratio_is_the_one_of_the_monitor_under_the_point() {
        let desk = Desktop::from_monitors(vec![
            Monitor::new(PhysRect::new(0, 0, 1000, 600), Scale::ONE),
            Monitor::new(PhysRect::new(1000, 0, 1000, 600), Scale::of(2.0)),
        ]);
        assert_eq!(desk.dpr_at(PhysPoint::new(500, 300)).ratio(), 1.0);
        assert_eq!(desk.dpr_at(PhysPoint::new(1500, 300)).ratio(), 2.0);
    }

    #[test]
    fn the_dead_space_inside_a_staggered_desktop_still_answers_a_ratio() {
        let desk = Desktop::from_monitors(vec![
            Monitor::new(PhysRect::new(0, 0, 1000, 600), Scale::ONE),
            Monitor::new(PhysRect::new(1000, 200, 1000, 600), Scale::of(2.0)),
        ]);
        assert_eq!(desk.bounds, PhysRect::new(0, 0, 2000, 800));
        let dead = PhysPoint::new(1500, 100);
        assert_eq!(desk.monitor_at(dead), None);
        assert_eq!(desk.dpr_at(dead), Scale::ONE);
    }

    #[test]
    fn a_pin_the_unions_dead_space_left_behind_comes_back_onto_the_primary() {
        // 三块屏里中间那一块被拔掉：剩下的两块上下错开，并集仍是一只完整的盒子，
        // 而盒子里有一条谁也没有在放的横带。
        let desk = Desktop::from_monitors(vec![
            Monitor::new(PhysRect::new(0, 0, 1000, 600), Scale::ONE),
            Monitor::new(PhysRect::new(1000, 200, 1000, 600), Scale::of(2.0)),
        ]);
        assert_eq!(desk.bounds, PhysRect::new(0, 0, 2000, 800));

        // 100x80 at (1500, 100)：在盒子里，不在任何一块屏上 —— 并集那一夹对它一个字
        // 也不说，所以「搬到主显示器」不能由那一夹代答。
        let dead = PhysRect::new(1500, 100, 100, 80);
        assert_eq!(desk.keep_visible(&dead), dead, "并集那一夹动了盒子内的东西");
        assert!(!desk.covered(&dead), "这条横带没有一台显示器在放");
        assert!(desk.refits(&dead), "问的那一句答不动，搬的那一只手却搬");
        let home = desk.rescue_onto_primary(&dead);
        assert_eq!(home, PhysRect::new(976, 100, 100, 80));
        assert_eq!(home.size(), dead.size(), "搬动顺手把图也改了");
        assert!(desk.covered(&home), "搬到主显示器却仍没落在它的可见区里");

        // 正在被一块屏显示的那一枚一步也不动：整枚在第二块上的、以及一半压在第二块
        // 上的（第一块只给它 10 px，不到 `MIN_VISIBLE_PX` 那一档也不算没人放）都留在原地。
        for shown in [
            PhysRect::new(1050, 300, 100, 80),
            PhysRect::new(990, 300, 100, 80),
        ] {
            assert!(desk.covered(&shown), "{shown:?} 此刻就在一块屏上");
            assert_eq!(desk.rescue_onto_primary(&shown), shown);
        }

        // 没有表就没有「哪一块是主屏」可答：交回原样，而不是替用户猜一个坐标。
        assert_eq!(Desktop::default().rescue_onto_primary(&dead), dead);
        assert!(!Desktop::default().refits(&dead));

        // 场上那一只手：`refit_all` 走的是同一条规则，一枚搬、另一枚留在原地。
        let mut s = pile(2, 100, 80);
        s.get_mut(1).expect("pin").pos_phys = PhysPoint::new(1500, 100);
        s.get_mut(2).expect("pin").pos_phys = PhysPoint::new(1050, 300);
        assert_eq!(s.refit_all(&desk), 1, "只有没人放的那一枚该被搬");
        assert_eq!(s.get(1).expect("pin").pos_phys, PhysPoint::new(976, 100));
        assert_eq!(s.get(2).expect("pin").pos_phys, PhysPoint::new(1050, 300));
    }

    #[test]
    fn a_zero_or_absent_ratio_cannot_reach_a_division() {
        let desk = Desktop::from_monitors(vec![Monitor::new(
            PhysRect::new(0, 0, 1000, 600),
            Scale(0.0),
        )]);
        assert_eq!(desk.dpr_at(PhysPoint::new(10, 10)), Scale::ONE);
        assert_eq!(Desktop::default().dpr_at(PhysPoint::new(0, 0)), Scale::ONE);
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

    /// §5.9.3 步骤 3/4 在菜单那一格上的**等比**：三格里填哪一格都走同一式子（宽或高单独填，
    /// 另一格由锁定比例跟着走；两格都填就是装得下整张图的那一枚），而「填百分比」这一支就是
    /// 直接给那一枚。判据的作者只有 [`size_zoom_request`] 一处，页与桥都不再算第二遍。
    #[test]
    fn the_size_dialog_locks_the_ratio_from_whichever_cell_was_typed() {
        let mut p = pin(400, 200);
        // 只填宽度：高跟着比例走。
        assert_eq!(p.set_size_from_text("200", "", "").expect("w"), 50);
        assert_eq!(p.size_phys, PhysSize::new(200, 100));
        // 只填高度：宽跟着比例走。
        assert_eq!(p.set_size_from_text("", "300", "").expect("h"), 150);
        assert_eq!(p.size_phys, PhysSize::new(600, 300));
        // 两格都填是一块**容器**：整张图都得装得下，所以取更小那一枚比例，与上面那枚 Option
        // 门 (`set_size_to`) 同一式子 —— 两扇门在这里合成一只作者。
        assert_eq!(p.set_size_from_text("100", "100", "").expect("box"), 25);
        assert_eq!(p.size_phys, PhysSize::new(100, 50));
        // 直接打百分比。
        assert_eq!(p.set_size_from_text("", "", "150").expect("pct"), 150);
        assert_eq!(p.size_phys, PhysSize::new(600, 300));
        // 空格两侧都算「没填」，前后字面的空白也不算字。
        assert_eq!(
            p.set_size_from_text(" 400 ", "", "  ").expect("padded"),
            100
        );
    }

    /// §5.9.3 那一格的非法输入与上下界：**拒，不猜**（§5.3.5 的 `size_guard` 立的同一口径 ——
    /// 不钳成 1×1、也不把用户没填的那一格替他填上），而且拒绝之后窗**一处也不动**，不会留下
    /// 「改了一半」的尺寸。界就是 §5.9.2 那两枚常数，句子里报的就是它们，页上没有第二份界。
    #[test]
    fn the_size_dialog_refuses_what_was_not_typed_and_changes_nothing() {
        for (cells, want) in [
            ((" ", "", ""), "三格都空着"),
            (("abc", "", ""), "宽那一格不是数"),
            (("", "12.5", ""), "高那一格不是数"),
            (("0", "", ""), "0 宽"),
            (("", "-5", ""), "负高"),
            (("", "", "0"), "百分比 0"),
            (("", "", "5"), "百分比低于下界（界内是 10..=800）"),
            (("", "", "1000"), "百分比出上界"),
            (("4000", "2000", ""), "按宽算要 1000%，出界"),
            (("4", "", ""), "按宽算要 1%，出界"),
            (("200", "", "150"), "宽高与百分比混着填"),
            (("", "", "99999999999999999999"), "大到读不出来的百分比"),
        ] {
            let mut p = pin(400, 200);
            let before = (p.zoom, p.size_phys);
            let got = p.set_size_from_text(cells.0, cells.1, cells.2);
            assert!(got.is_err(), "{cells:?}（{want}）竟然过了");
            assert_eq!((p.zoom, p.size_phys), before, "{cells:?} 被拒却动了窗");
        }
        // 界那句话里得把界本身报出来，页上才不必抄第二份。
        let said = size_zoom_request("4000", "", "", PhysSize::new(400, 200))
            .err()
            .unwrap()
            .to_string();
        assert!(
            said.contains("10") && said.contains("800") && said.contains("1000"),
            "{said}"
        );
        // 界内但**不站在滚轮阶梯上**的一枚（101%）照样吃得下：阶梯是滚轮那一手的事，
        // 输入这一扇门的界只有 §5.9.2 那两枚常数，页上不许替它再加一条「只能打在梯子上」。
        assert_eq!(
            size_zoom_request("", "", "101", PhysSize::new(400, 200))
                .expect("off the ladder but inside the bounds"),
            101
        );
        let said = size_zoom_request("abc", "", "", PhysSize::new(400, 200))
            .err()
            .unwrap()
            .to_string();
        assert!(said.contains("宽度") && said.contains("abc"), "{said}");
        // 大到绕不回 `u32` 的一格也不许落地。
        let big = size_zoom_request("4294967296", "", "", PhysSize::new(400, 200)).err();
        assert!(
            matches!(big, Some(PinError::OutOfRange(_, _, _, _))),
            "{big:?}"
        );
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

    /// §5.9.4 规则第一句「透明度过低时仍应保留找回窗口的方式」逐臂判一遍：窗上的**阶梯**到不了
    /// 地板以下（[`OPACITY_STEPS`] 最低一级就是 [`OPACITY_MIN`]），**钳**那一臂把 1..=19 抬回地板，
    /// 而这一轮新加的**菜单输入**那一扇门 [`typed_opacity`] 连 0 一起拒 —— 三条门都到不了
    /// 「完全看不见」那一档，那句话因此在页上成立。0 只有程序内的调用者给得出来（上面那一枚测试
    /// 判的就是 `[clamp_opacity]` 手里它仍合法），那是另一件事。
    ///
    /// 另外两句在本档能判的只有状态机这一头：透明度**不**进 [`PinItem::hit`] 的判据，所以一扇调到
    /// 地板的窗在状态机里照样认得出鼠标；原生命中测试在 0% 那一档还认不认（那才是「找不找得回」
    /// 的真身）本仓没有量具测过，写在 [`typed_opacity`] 的信里，不在这里冒充判过。规则第二句
    /// 「应允许一键恢复 100% 不透明」是 [`PinItem::reset_opacity`] 那一臂。
    #[test]
    fn the_floor_is_the_way_back_on_every_door() {
        assert_eq!(OPACITY_STEPS.iter().min(), Some(&OPACITY_MIN));
        let mut p = pin(10, 10);
        for _ in 0..30 {
            p.opacity_step(false);
        }
        assert_eq!(p.opacity, OPACITY_MIN, "阶梯走不到地板以下");

        for v in 1..OPACITY_MIN {
            assert_eq!(clamp_opacity(v), OPACITY_MIN, "{v} 被抬回地板");
        }
        assert_eq!(clamp_opacity(200), 100);

        for raw in ["0", "1", "19", "101", "-5", "abc", "", "   ", "55.5", "5 0"] {
            assert!(typed_opacity(raw).is_err(), "{raw:?} 竟然过了");
        }
        assert_eq!(typed_opacity("20").expect("floor"), OPACITY_MIN);
        assert_eq!(typed_opacity("100").expect("top"), 100);
        assert_eq!(typed_opacity(" 55 ").expect("trimmed"), 55);
        let said = typed_opacity("5").err().unwrap().to_string();
        assert!(said.contains("20") && said.contains("100"), "{said}");

        // 地板那一档仍然吃得下点击；0 那一档也还认得鼠标（状态机这一头），只是页上没有一扇门
        // 走得到那里。
        let mut low = pin(20, 10);
        low.set_opacity(typed_opacity("20").unwrap());
        assert!(low.hit(PhysPoint::new(5, 5), |_| 255));
        low.set_opacity(0);
        assert!(
            low.hit(PhysPoint::new(5, 5), |_| 255),
            "透明度不是 `hit` 的判据，状态机不许因此认不出这一块"
        );
        low.reset_opacity();
        assert_eq!(low.opacity, 100, "一键回到 100% 不透明");
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
    fn a_crop_taken_from_the_window_keeps_the_source_pixels_not_the_screen_result() {
        // §5.9.9「裁剪操作不应降低保留区域的原始分辨率」——这一句在这里是可量的，不是说法：
        // 保留下来的那张图，像素数应该等于源图上被留下的那一块。若裁剪是从屏幕结果上切的，
        // 50% 时拖 25×12 就只剩 25×12，源图那一块的 49×23 永远拿不回来。
        let src = solid(200, 100, [10, 20, 30, 255]);
        let mut p = pin(200, 100);
        p.set_zoom(50);
        assert_eq!(
            p.size_phys,
            PhysSize::new(100, 50),
            "屏幕上的窗比源图小一半"
        );

        let w = p.window_rect();
        let dragged = PhysRect::new(w.x, w.y, w.w / 4, w.h / 4);
        p.crop_window(&dragged)
            .expect("the window drag becomes a crop");

        let kept = p.render(&src).expect("render");
        assert_eq!(
            (kept.width, kept.height),
            (p.src_rect.w, p.src_rect.h),
            "渲染出来的就是保留区域本身，中间没有第二次采样"
        );
        assert!(
            kept.width >= dragged.w && kept.height >= dragged.h,
            "缩小看的时候，保留的源像素不该比屏幕上那块还少：{}×{} vs 屏幕 {}×{}",
            kept.width,
            kept.height,
            dragged.w,
            dragged.h
        );
        // 保留的尺寸落在「屏幕那块 ÷ 缩放」±1 之内。不是恰好等于：`window_to_source` 取的是
        // 像素中心（+0.5），低缩放时这一次取样会偏出约一个源像素。量到的就是这个，写这个：
        // 屏幕 25×12 在 50% 下留下 49×23（理想 50×24），保留的仍是源图上那一块的原始像素。
        assert_eq!(
            (kept.width, kept.height),
            (49, 23),
            "量的就是这一枚，不是理想算术"
        );
        for (have, want, axis) in [
            (kept.width, dragged.w * 2, "宽"),
            (kept.height, dragged.h * 2, "高"),
        ] {
            assert!(
                (i64::from(have) - i64::from(want)).abs() <= 1,
                "{axis}：保留 {have}，屏幕 {want} 像素放大回源图应当是 {want}±1"
            );
        }

        // 反过来那一头也成立：300% 时屏幕上的一大块只对应源里的一小块，切完仍是源里的原始
        // 像素，不会因为屏幕变大而多出并不存在的细节。
        let mut p = pin(200, 100);
        p.set_zoom(300);
        assert_eq!(p.size_phys, PhysSize::new(600, 300));
        let w = p.window_rect();
        p.crop_window(&PhysRect::new(w.x, w.y, w.w / 6, w.h / 6))
            .expect("the zoomed-in drag becomes a crop");
        let kept = p.render(&src).expect("render");
        assert_eq!(
            (kept.width, kept.height),
            (p.src_rect.w, p.src_rect.h),
            "放大之后再裁，也还是从源图上切"
        );
        assert!(
            kept.width <= 100 && kept.height <= 50,
            "源图只有 200×100，切出来的不可能更多：{}×{}",
            kept.width,
            kept.height
        );
        // 量到的这一枚：屏幕 100×50 在 300% 下只对应源里 34×17（理想 33.3×16.7）。
        // 屏幕上多出来的那一倍细节源图里本来没有，裁剪不会替它造出来。
        assert_eq!(
            (kept.width, kept.height),
            (34, 17),
            "放大之后切的就是源里这一小块"
        );

        // 转过 90° 的窗上拖动，切的同样是源图那一块，而不是被旋转过的屏幕结果。
        let mut p = pin(200, 100);
        p.set_zoom(50);
        p.rotate_cw();
        let w = p.window_rect();
        p.crop_window(&PhysRect::new(w.x, w.y, w.w / 2, w.h / 2))
            .expect("a crop on a turned window");
        let kept = p.render(&src).expect("render");
        // `render` 在保留区域之上还会再转 90°，所以这里比的是两条边，不是一一对应。
        let kept_sides = {
            let mut v = [kept.width, kept.height];
            v.sort_unstable();
            v
        };
        let rect_sides = {
            let mut v = [p.src_rect.w, p.src_rect.h];
            v.sort_unstable();
            v
        };
        assert_eq!(
            kept_sides, rect_sides,
            "保留区域的原始分辨率不因旋转而重采样"
        );
        // 量到的这一枚：源坐标里留下 99×49，渲染再转回去就是 49×99。
        assert_eq!(rect_sides, [49, 99], "转过的窗上切的还是源图那一块");
        assert!(
            kept_sides[0] <= 100 && kept_sides[1] <= 200,
            "保留区域始终在源图的坐标系里，不可能超出源图：{kept_sides:?}"
        );
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
    fn the_preset_box_is_read_from_the_pin_config() {
        // The two `[pin]` keys are the whole story: a set box is handed back
        // verbatim, and the number 240 has one author (the constant), so the
        // default config spells out that same box rather than a second literal.
        let mut c = cfg();
        c.thumb_fixed_width = 160;
        c.thumb_fixed_height = 90;
        assert_eq!(thumb_preset_box(&c), PhysSize::new(160, 90));

        assert_eq!(thumb_preset_box(&cfg()), THUMB_FIXED_DEFAULT);
        assert_eq!(cfg().thumb_fixed_width, THUMB_FIXED_DEFAULT.w);
        assert_eq!(cfg().thumb_fixed_height, THUMB_FIXED_DEFAULT.h);

        // A zero edge — a config that skipped `validate` — is not a silent empty
        // box: it falls back to the same default `enter_fixed_thumbnail` uses.
        let mut broken = cfg();
        broken.thumb_fixed_height = 0;
        assert_eq!(thumb_preset_box(&broken), THUMB_FIXED_DEFAULT);
    }

    #[test]
    fn a_configured_preset_thumbnail_takes_the_preset_size_and_gives_the_size_back() {
        // §5.9.10 steps 2 and 3, driven by config rather than the constant. A
        // 400×200 picture fitted 等比 into a 100×100 preset box is 100×50; the
        // same press a second time returns the pin to its original 400×200. That
        // the value is 100×50 and not the default's 240×120 is the proof the
        // preset was read, not hardcoded.
        let mut c = cfg();
        c.thumb_fixed_width = 100;
        c.thumb_fixed_height = 100;
        let mut p = pin(400, 200);
        let before = p.size_phys;

        assert!(p.toggle_thumbnail_preset(&c));
        assert_eq!(p.thumbnail.map(|t| t.mode), Some(ThumbnailMode::Fixed));
        assert_eq!(
            p.size_phys,
            PhysSize::new(100, 50),
            "the middle size is the preset, fitted 等比"
        );

        assert!(!p.toggle_thumbnail_preset(&c));
        assert_eq!(p.thumbnail, None);
        assert_eq!(p.size_phys, before, "the size the shortcut gave back");
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

    #[test]
    fn the_clock_moves_only_when_a_frame_has_run_out_of_its_own_time() {
        let mut p = pin(10, 10);
        let delays = [100, 200, 50];
        let ps = p.set_gif(3).expect("gif");
        assert!(ps.playing);
        assert!(!ps.tick(50, &delays));
        assert_eq!(ps.frame, 0);
        assert!(ps.tick(60, &delays));
        assert_eq!(ps.frame, 1);
        assert_eq!(ps.elapsed_ms, 10);
        assert!(!ps.tick(100, &delays));
        assert_eq!(ps.frame, 1);
        assert!(ps.tick(100, &delays));
        assert_eq!(ps.frame, 2);
        assert_eq!(ps.elapsed_ms, 10);
        assert!(ps.tick(40, &delays));
        assert_eq!(ps.frame, 0);
        assert_eq!(ps.elapsed_ms, 0);
    }

    #[test]
    fn the_file_s_own_pace_is_scaled_by_the_speed_the_user_chose() {
        let mut p = pin(10, 10);
        let delays = [100, 100];
        let ps = p.set_gif(2).expect("gif");
        ps.set_speed(200);
        assert!(ps.tick(50, &delays));
        assert_eq!(ps.frame, 1);
        ps.set_speed(50);
        assert!(!ps.tick(190, &delays));
        assert_eq!(ps.frame, 1);
        assert!(ps.tick(10, &delays));
        assert_eq!(ps.frame, 0);
    }

    #[test]
    fn a_paused_clock_does_not_bank_the_time_it_was_stopped_for() {
        let mut p = pin(10, 10);
        let delays = [100, 100, 100, 100];
        let ps = p.set_gif(4).expect("gif");
        ps.tick(50, &delays);
        assert!(!ps.toggle_play());
        assert!(!ps.tick(10_000, &delays));
        assert_eq!(ps.frame, 0);
        assert_eq!(ps.elapsed_ms, 0);
        assert!(ps.toggle_play());
        assert!(!ps.tick(50, &delays));
        assert!(ps.tick(50, &delays));
        assert_eq!(ps.frame, 1);
    }

    #[test]
    fn one_tick_cannot_run_the_clock_around_the_clip() {
        let mut p = pin(10, 10);
        let delays = [10, 10, 10];
        let ps = p.set_gif(3).expect("gif");
        assert!(ps.tick(100_000, &delays));
        assert!(ps.frame < 3);
        assert_eq!(ps.elapsed_ms, 0);
        // A file that declares no delay at all is a broken file, not a spin loop.
        let ps = p.set_gif(3).expect("gif");
        assert!(!ps.tick(90, &[0, 0, 0]));
        assert_eq!(ps.frame, 0);
        assert!(ps.tick(10, &[0, 0, 0]));
        assert_eq!(ps.frame, 1);
        assert_eq!(ps.elapsed_ms, 0);
    }

    #[test]
    fn stepping_by_hand_spends_the_partial_frame_the_clock_had_banked() {
        let mut p = pin(10, 10);
        let delays = [100; 4];
        let ps = p.set_gif(4).expect("gif");
        assert!(!ps.tick(70, &delays));
        assert_eq!(ps.elapsed_ms, 70);
        ps.step(1);
        assert_eq!(ps.elapsed_ms, 0);
        assert!(ps.toggle_play());
        assert!(!ps.tick(30, &delays));
        assert!(ps.tick(70, &delays));
        assert_eq!(ps.frame, 2);
        ps.tick(50, &delays);
        assert_eq!(ps.elapsed_ms, 50);
        ps.first_frame();
        assert_eq!(ps.frame, 0);
        assert_eq!(ps.elapsed_ms, 0);
        assert!(!ps.tick(50, &delays));
    }

    // --------------------------------------------------------------- §7.1 (GIF 降频)

    /// The watched arm is a regression guard, not a new promise: a pin somebody can
    /// see is asked at the polling rate §5.9.14 has always used, whatever the clip's
    /// own pace says. A visible arm that moved with `frame_delay_ms` would mean the
    /// gate had grown a second frame clock (plan rule 281).
    #[test]
    fn a_watched_pin_is_asked_at_the_cadence_it_was_always_asked_at() {
        for delay in [
            0,
            1,
            GIF_MIN_FRAME_DELAY_MS,
            100,
            GIF_DEFAULT_FRAME_DELAY_MS,
            3_000,
            u32::MAX,
        ] {
            assert_eq!(
                gif_interval(true, delay),
                GIF_POLL_MS,
                "看得见那一臂不许跟着 delay 走：{delay}"
            );
        }
        assert_eq!(GIF_POLL_MS, 20, "§5.9.14 起窗那一侧问的就是这一档");
    }

    /// §7.1's own row: 看不见 ⇒ 降到 [`GIF_HIDDEN_POLL_MS`]，二十倍少的 wakeup；而一帧
    /// 本来就挂得比一秒久的片子跟着文件自己的步子走，不必一秒敲五次。
    #[test]
    fn a_pin_nobody_can_see_is_asked_ten_times_less_often() {
        assert_eq!(gif_interval(false, 0), GIF_HIDDEN_POLL_MS);
        assert_eq!(gif_interval(false, 100), GIF_HIDDEN_POLL_MS);
        assert_eq!(gif_interval(false, GIF_HIDDEN_POLL_MS), GIF_HIDDEN_POLL_MS);
        assert_eq!(gif_interval(false, 3_000), 3_000);
        for delay in [0, 100, 1_000, 3_000, 9_999] {
            assert!(
                gif_interval(false, delay) > gif_interval(true, delay),
                "{delay} 那一档没有降下来：{} vs {}",
                gif_interval(true, delay),
                gif_interval(false, delay)
            );
        }
    }

    /// Nothing this gate hands a `Timer` may be zero - a 0 ms interval is a loop that
    /// never sleeps - and nothing may be so long that a pin nobody sees never wakes up
    /// again (§433 不许静默: 降频不能降成「从此不再动」). Both the broken file's
    /// `u32::MAX` and the clip with no second frame at all land inside the bounds.
    #[test]
    fn the_gate_never_answers_zero_and_never_answers_forever() {
        for visible in [true, false] {
            for delay in [0, 1, u32::MAX / 2, u32::MAX] {
                let got = gif_interval(visible, delay);
                assert!(
                    (1..=GIF_HIDDEN_POLL_MAX_MS).contains(&got),
                    "{visible} / {delay} 答了 {got}"
                );
            }
        }
        let mut p = pin(10, 10);
        // A one-frame clip: the player still answers a real pace for the frame it is
        // showing, and it never plays and never banks a wait.
        let one = p.set_gif(1).expect("one frame");
        assert_eq!(one.frame_delay_ms(&[]), GIF_DEFAULT_FRAME_DELAY_MS);
        assert_eq!(
            gif_interval(false, one.frame_delay_ms(&[])),
            GIF_HIDDEN_POLL_MS
        );
        assert!(!one.playing);
        assert!(!one.tick(GIF_POLL_MS, &[]));
        assert_eq!(one.elapsed_ms, 0);
        // A file whose delay list says nothing is a broken file, not an infinite pace.
        let three = p.set_gif(3).expect("three frames");
        assert!(three.frame_delay_ms(&[0, 0, 0]) >= GIF_MIN_FRAME_DELAY_MS);
        assert!(gif_interval(false, three.frame_delay_ms(&[0, 0, 0])) <= GIF_HIDDEN_POLL_MAX_MS);
    }

    /// 回来时站在哪一帧，这一读在这里说死：**看不见的那一段是花掉了、不是欠着了**
    /// ([`PlayState::tick`] 一次至多绕片子一圈)，所以一扇窗从最小化里回来时既不跳回开头、
    /// 也不一次补掉好几圈，它继续走的是自己走开那一刻站着的那一帧。慢片子（一圈比一秒长）
    /// 在看不见的那一段按自己的步子一帧一帧走，那是同一道判据的另一半。
    #[test]
    fn the_throttled_wait_is_spent_rather_than_owed_when_the_pin_comes_back() {
        let mut p = pin(10, 10);
        let delays = [100, 100, 100, 100];
        let ps = p.set_gif(4).expect("gif");
        for _ in 0..10 {
            let asked = gif_interval(true, ps.frame_delay_ms(&delays));
            ps.tick(asked, &delays);
        }
        assert_eq!(ps.frame, 2, "看得见时 200 ms 就是两帧");
        assert_eq!(ps.elapsed_ms, 0);

        let hidden = gif_interval(false, ps.frame_delay_ms(&delays));
        assert_eq!(hidden, GIF_HIDDEN_POLL_MS);
        // One second of asking is ten frames of this clip and it has four: one lap at
        // most, so the index lands back on the frame it was already on.
        for _ in 0..3 {
            assert!(ps.tick(hidden, &delays), "看不见不等于停钟");
        }
        assert_eq!(ps.frame, 2, "看不见的那一段不该把片子挪走，更不该一次挪两圈");
        assert_eq!(ps.elapsed_ms, 0);

        for _ in 0..5 {
            let asked = gif_interval(true, ps.frame_delay_ms(&delays));
            ps.tick(asked, &delays);
        }
        assert_eq!(ps.frame, 3, "回到屏幕上是从原来那一帧接着走，不是爆发");

        let slow = [3_000, 3_000, 3_000];
        let ps = p.set_gif(3).expect("slow gif");
        let asked = gif_interval(false, ps.frame_delay_ms(&slow));
        assert_eq!(asked, 3_000);
        assert!(ps.tick(asked, &slow));
        assert_eq!(ps.frame, 1, "一帧三秒的片子被问一次就该走一帧");
        assert_eq!(ps.elapsed_ms, 0);
    }

    /// 接线格，不是判据格：§7.1 的可见性判断必须长在窗那一枚 `gifClock` 上，而不是只长
    /// 成 core 的一枚函数。这一格 `include_str!` 读 `crates/qt_bridge/qml/PinWindow.qml`
    /// 本档，并且**只读代码行**——一句写在注释里的 `interval: win.gifWatched …` 不算接线
    /// (§433)。
    ///
    /// 尺子为什么在本档而不是 `qt_bridge::pin_check.rs`：全仓那一把剥注释的尺子是
    /// `qt_bridge/src/hint_check.rs::qml_code`，`pub(crate)`，core 够不着；搬到那边去的
    /// 一份 diff 随本轮报告交出，本档因此只做「整行是代码、且含这一串」这件最小的事。
    #[test]
    fn the_pin_qml_gif_gate_really_reads_the_window_s_own_visibility() {
        let qml = include_str!("../../qt_bridge/qml/PinWindow.qml");
        let rows: Vec<String> = qml
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && !l.starts_with('*'))
            .map(str::to_string)
            .collect();
        let coded = |needle: &str| rows.iter().filter(|l| l.contains(needle)).count();

        assert_eq!(
            coded("id: gifClock"),
            1,
            "贴图的 gif 帧钟只该有一枚 `gifClock`"
        );
        assert_eq!(
            coded("interval: win.gifWatched ? win.gifPollMs : win.gifHiddenPollMs"),
            1,
            "`gifClock` 的 `interval` 不再问可见性那一格，§7.1 的接线就断了"
        );
        assert_eq!(
            coded("readonly property bool gifWatched: win.visible"),
            1,
            "读数本体不在了"
        );
        assert_eq!(
            coded("&& win.visibility !== Window.Minimized"),
            1,
            "最小化那一半没有读数——Qt 的 `visible` 在最小化时仍是 true"
        );
        assert_eq!(
            coded("&& win.visibility !== Window.Hidden"),
            1,
            "隐藏那一半没有读数"
        );
        assert_eq!(
            coded(&format!("readonly property int gifPollMs: {GIF_POLL_MS}")),
            1,
            "窗上抄的看得见那一档与 core 的 `GIF_POLL_MS` 已经不是同一个数"
        );
        assert_eq!(
            coded(&format!(
                "readonly property int gifHiddenPollMs: {GIF_HIDDEN_POLL_MS}"
            )),
            1,
            "窗上抄的看不见那一档与 core 的 `GIF_HIDDEN_POLL_MS` 已经不是同一个数"
        );
        // 交给 Rust 的那一段永远是窗真正等到的那一段：判据没有第二条路去乘 delay。
        assert_eq!(
            coded("onTriggered: win.view.gifTick(gifClock.interval)"),
            1,
            "帧钟的落点不再读自己等到的那一段"
        );
        assert_eq!(
            coded("win.view.gifTick("),
            1,
            "帧钟只该有一处落点，两处就是两把钟"
        );
        // 探针当场读过回来的两枚错写法，一次也不许出现（`visibleDepth` 静默 undefined、
        // `QWindow.Minimized` ReferenceError）。
        assert_eq!(
            coded("win.visibleDepth"),
            0,
            "6.10.1 的 Window 类型没有 `visibleDepth` 这一格，读它等于永远判成没人看"
        );
        assert_eq!(
            coded("QWindow.Minimized"),
            0,
            "这档 QML 里 visibility 的枚举只认 `Window.` 前缀"
        );
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
        let stood_around = p.window_rect().center();
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
        assert_eq!(p.window_rect().center(), stood_around);
        assert_eq!(p.size_phys, PhysSize::new(100, 50));
        // 圆心那句话不是从「尺寸没变」里白来的：40x40 → 100x50 那一手必须先把左上角挪走，
        // 否则上面那问是一枚空尺（#433），而 P134 之前红的就是它没挪。
        assert_ne!(p.pos_phys, where_it_stood);
    }

    /// P134 的落空：改尺寸的两只手对「锚在哪儿」不许各说各话。`zoomStep` 照圆心缩，
    /// 「重置视图」照左上角长，用户按 `-` 再按重置，贴图就自己走了 (+16,+10)；真机那一趟
    /// 是 `rst-lone-turned` → `reset_pin` 把这 16x10 留在了场上，接着把别家的
    /// `pin window` / `pin drag undo` / `pin pile *` 四枚行一起弄红。
    #[test]
    fn a_zoom_step_and_a_reset_leave_the_pin_exactly_where_it_stood() {
        let mut p = pin(320, 200);
        let stood = p.pos_phys;
        let size = p.size_phys;
        p.zoom_step(false, None);
        assert_eq!(p.zoom, 90);
        assert_ne!(
            p.size_phys, size,
            "那一手得真把尺寸改小，不然下面那问没有证人"
        );
        assert_ne!(p.pos_phys, stood, "缩是照着圆心挪的，左上角不动才是旧的错");
        p.reset_view(&cfg());
        assert_eq!(p.pos_phys, stood);
        assert_eq!(p.size_phys, size);
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
        s.set_selection(&[1, 2]);
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
    fn a_batch_opacity_step_keeps_each_pin_on_its_own_rung() {
        let mut s = pile(3, 10, 10);
        s.get_mut(1).expect("pin").set_opacity(30);
        s.get_mut(3).expect("pin").set_opacity(80);
        s.set_selection(&[1, 3]);
        assert_eq!(s.step_selection_opacity(false), 2);
        assert_eq!(s.get(1).expect("pin").opacity, 20);
        assert_eq!(s.get(3).expect("pin").opacity, 70);
        assert_eq!(s.get(2).expect("pin").opacity, 100);
        assert_eq!(s.step_selection_opacity(true), 2);
        assert_eq!(s.get(1).expect("pin").opacity, 30);
        assert_eq!(s.get(3).expect("pin").opacity, 80);
    }

    #[test]
    fn raising_moves_one_pin_to_the_top_of_the_pile() {
        let mut s = pile(3, 10, 10);
        assert!(s.raise(1));
        assert_eq!(s.ids(), vec![2, 3, 1]);
        assert!(s.raise(3));
        assert_eq!(s.ids(), vec![2, 1, 3], "再抬一次，被抬的那一枚站到最上");
        assert!(!s.raise(9));
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

    // -------------------------------------------------------- §5.16.1 pin_edit

    #[test]
    fn nothing_selected_leaves_no_rect_to_hole_out() {
        let s = pile(2, 100, 100);
        assert_eq!(s.selected_bounds(), None, "空集里没有一枚洞");
        let empty = PinSet::default();
        assert_eq!(empty.selected_bounds(), None);
    }

    #[test]
    fn one_selected_pin_gives_exactly_the_window_it_sits_in() {
        let mut s = PinSet::default();
        s.add(pin_at(1, 120, 80, PhysPoint::new(10, 20)));
        s.set_selection(&[1]);
        assert_eq!(s.selected_bounds(), Some(PhysRect::new(10, 20, 120, 80)));
    }

    #[test]
    fn two_selected_pins_are_one_union_across_the_dead_space() {
        // Primary at (0,0) 1920x1080, a second monitor bolted to its left edge;
        // the two pins face each other from the far ends of the two screens.
        let mut s = PinSet::default();
        s.add(pin_at(1, 100, 100, PhysPoint::new(900, 0)));
        s.add(pin_at(2, 100, 100, PhysPoint::new(-500, 200)));
        s.set_selection(&[1, 2]);
        assert_eq!(s.selected_bounds(), Some(PhysRect::new(-500, 0, 1500, 300)));
        // The hole is one rect, not two: the strip of nothing between the
        // screens comes with it, which is what `pin_edit` promises to live with.
        assert!(s
            .selected_bounds()
            .expect("rect")
            .contains(PhysPoint::new(-1, 50)));
    }

    #[test]
    fn a_zoomed_pin_contributes_the_zoomed_rect_not_the_picture() {
        let mut s = PinSet::default();
        let mut p = pin_at(1, 120, 80, PhysPoint::new(10, 20));
        p.set_zoom(200);
        s.add(p);
        s.set_selection(&[1]);
        // Doubled about its own centre (70,60), so the top-left runs off to
        // (-50,-20): the numbers come from the rule, not from the call.
        assert_eq!(s.selected_bounds(), Some(PhysRect::new(-50, -20, 240, 160)));
    }

    #[test]
    fn a_turned_pin_contributes_the_rect_it_covers_after_the_swap() {
        let mut s = PinSet::default();
        let mut p = pin_at(1, 120, 80, PhysPoint::new(10, 20));
        p.rotate_cw();
        s.add(p);
        s.set_selection(&[1]);
        // 90° swaps the axes and the pin re-sticks about (70,60): a reader that
        // spelled `size_phys` off the picture instead of the window would put
        // 120x80 here and cut the bottom 40px of the pin out of the hole.
        assert_eq!(s.selected_bounds(), Some(PhysRect::new(30, 0, 80, 120)));
        assert_ne!(
            s.selected_bounds().expect("rect").size(),
            PhysSize::new(120, 80),
            "转过的窗盖住的不是竖着的那一块"
        );
    }

    #[test]
    fn a_free_thumbnail_narrows_the_hole_to_what_is_on_show() {
        let mut s = PinSet::default();
        let mut p = pin_at(1, 120, 80, PhysPoint::new(10, 20));
        p.set_free_thumbnail(&PhysRect::new(20, 10, 60, 40))
            .expect("inside");
        s.add(p);
        s.set_selection(&[1]);
        assert_eq!(s.selected_bounds(), Some(PhysRect::new(10, 20, 60, 40)));
        assert_ne!(
            s.selected_bounds(),
            Some(PhysRect::new(10, 20, 120, 80)),
            "缩略图只把看得见的部分铺在桌面上"
        );
    }

    #[test]
    fn a_fixed_thumbnail_contributes_the_box_it_was_fitted_into() {
        let mut s = PinSet::default();
        let mut p = pin_at(1, 120, 80, PhysPoint::new(10, 20));
        p.enter_fixed_thumbnail(PhysSize::new(60, 60));
        s.add(p);
        s.set_selection(&[1]);
        // 等比 fit into 60x60 is 50%, so the window is 60x40 re-centred on (70,60).
        assert_eq!(s.selected_bounds(), Some(PhysRect::new(40, 40, 60, 40)));
    }

    #[test]
    fn a_selected_but_hidden_pin_does_not_widen_the_hole() {
        let mut s = PinSet::default();
        s.add(pin_at(1, 100, 100, PhysPoint::new(0, 0)));
        s.add(pin_at(2, 100, 100, PhysPoint::new(400, 0)));
        s.set_selection(&[1, 2]);
        assert_eq!(s.selected_bounds(), Some(PhysRect::new(0, 0, 500, 100)));

        s.get_mut(2).expect("pin").solo_excluded = true;
        assert_eq!(
            s.selected_bounds(),
            Some(PhysRect::new(0, 0, 100, 100)),
            "Solo 收起来的那一枚不能被冻结拍到，也不该把洞撑大"
        );
        s.get_mut(1).expect("pin").group_hidden = true;
        assert_eq!(
            s.selected_bounds(),
            None,
            "选中的两枚都看不见，就是没有一块可拍"
        );
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
        assert_eq!(
            q.card, p.card,
            "一张从文件钉上来的贴图被读成了一张卡片：{:?} vs {:?}",
            q.card, p.card
        );
    }

    /// §5.8.5 步骤 4 / §5.8.6 步骤 4–5 在 §7.2 那一头上的债：快照只带像素的话，恢复回来的那一枚
    /// 就再也回不到「按这一枚的宽度重排一次」—— 卡片从此退化成一张平图，而页上那四格连初值也
    /// 读不到（读的是 [`CardPin::card_of`]）。这一格判的是那句「记得说什么」真的过了一趟档。
    #[test]
    fn a_card_pin_comes_back_still_knowing_what_it_was_asked_to_say() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = Card::default();

        let mut words = card_pin(4, CardPin::of_text("这段字要留在档里"), &base);
        words
            .edit_card(
                &base,
                &CardEdit {
                    width: Some(700),
                    font_family: Some("Arial".into()),
                    background: Some("#102030".into()),
                    ..Default::default()
                },
                &NO_FONT,
            )
            .expect("卡片")
            .expect("收下");
        let colour = card_pin(5, CardPin::of_colour([0xab, 0xcd, 0xef, 0x80]), &base);
        let (words_frame, colour_frame) = (
            crate::card::render(
                &words.card.as_ref().expect("身份").text,
                &words.card.as_ref().expect("身份").card_of(&base),
                &NO_FONT,
            )
            .expect("重排"),
            crate::card::render(
                &colour.card.as_ref().expect("身份").text,
                &colour.card.as_ref().expect("身份").card_of(&base),
                &NO_FONT,
            )
            .expect("重排"),
        );

        let mut s = PinSet::default();
        s.add(words.clone());
        s.add(colour.clone());
        let mut grab = |id: PinId| -> Option<Frame> {
            match id {
                4 => Some(words_frame.clone()),
                5 => Some(colour_frame.clone()),
                _ => None,
            }
        };
        let report = s.write_state(dir.path(), &mut grab, 1).expect("write");
        assert_eq!(report.frames_written, 2, "两枚卡片的像素都该落盘：{report:?}");

        let raw = std::fs::read_to_string(dir.path().join(STATE_FILE)).expect("read");
        assert!(
            raw.contains("这段字要留在档里"),
            "那句话没进到档里，恢复后就没得重排：{raw}"
        );
        assert!(
            raw.contains("#ABCDEF80"),
            "颜色卡片那一行没进到档里：{raw}"
        );

        let back = PinSet::read_state(dir.path())
            .expect("read")
            .expect("a file");
        assert_eq!(back.set.len(), 2);
        let a = back.set.get(4).expect("pin 4");
        assert_eq!(
            a.card,
            words.card,
            "文本卡片的那份身份过了趟档就变了：{:?}",
            a.card
        );
        let b = back.set.get(5).expect("pin 5");
        assert_eq!(b.card, colour.card, "颜色卡片的那份身份没回来：{:?}", b.card);
        // 回来的那一枚还**排得动**：恢复之后按它自己的宽度再来一遍，窗跟着走。
        let again = b
            .clone()
            .edit_card(
                &base,
                &CardEdit {
                    width: Some(900),
                    ..Default::default()
                },
                &NO_FONT,
            )
            .expect("恢复回来的还是卡片")
            .expect("收下");
        assert_eq!(again.frame.width, 900, "恢复后的那一枚按这一枚的宽度排不出去");
        assert_eq!(
            b.card.as_ref().expect("身份").text,
            "#ABCDEF80",
            "重排要用的还是那句话，不是空的一行"
        );
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

    /// [`PinItem::adopt_source`] 是「换来源」那两趟唯一执行处（§7.2 恢复 与 §5.8.4 拖进来那一张）。
    /// 这一格钉的是它**换了什么、没换什么**：形状与那一刀跟着走、自由缩略图那一格（它记的是同一套
    /// 源图像素，见 [`PinItem::visible_rect`]）跟着夹、记着旧图的那几刀清空；而**形状没变的那一趟
    /// 一枚字都不许动** —— 恢复现场每天走的就是这一手，它要是把没换过来源的贴图也改坏了，用户的
    /// 现场是「关一下再开，那枚裁剪就变小了」。
    #[test]
    fn adopting_a_source_moves_only_what_measured_the_old_picture() {
        let mut p = PinItem::new(
            1,
            ImageRef::Volatile,
            PhysSize::new(200, 100),
            PhysPoint::new(0, 0),
            &cfg(),
        )
        .expect("pin");
        p.crop(&PhysRect::new(10, 10, 100, 60)).expect("crop");
        p.set_free_thumbnail(&PhysRect::new(20, 20, 40, 30))
            .expect("thumb");
        assert_eq!(p.crop_undo.len(), 1, "这一刀之前是整张图");

        // 同一枚形状：答「没换」，而现场一字未动。
        assert!(!p.adopt_source(PhysSize::new(200, 100)).expect("same"));
        assert_eq!(p.src_rect, PhysRect::new(10, 10, 100, 60));
        assert_eq!(p.crop_undo.len(), 1, "形状没变却清了栈");

        // 换小：那一刀夹进新图，缩略图跟着夹进那一刀，栈清空。
        assert!(p.adopt_source(PhysSize::new(50, 40)).expect("adopt"));
        assert_eq!(p.source_size, PhysSize::new(50, 40));
        assert_eq!(p.src_rect, PhysRect::new(10, 10, 40, 30));
        assert!(
            p.crop_undo.is_empty(),
            "旧图的那几刀还留着：撤销会交回另一张图上的一格"
        );
        let t = p.thumbnail.expect("缩略图那一格还在，只是被夹小了");
        assert_eq!(t.rect, PhysRect::new(20, 20, 30, 20), "{t:?}");
        assert!(
            p.visible_rect()
                .intersection(&p.source_size.at(PhysPoint::new(0, 0)))
                .is_some_and(|r| r == p.visible_rect()),
            "换过来源之后，屏幕上那一格还是装得进新图"
        );

        // 一枚像素也没有的来源不是「一次没换成功的取回」，而是拒 —— 且被拒的那一趟不许留下半改
        // 好的现场（那正是「屏幕上还是上一张、档里已经是下一张」那一族的形状）。
        assert!(matches!(
            p.adopt_source(PhysSize::new(0, 10)),
            Err(PinError::EmptyImage)
        ));
        assert_eq!(p.source_size, PhysSize::new(50, 40), "被拒的那一趟改坏了现场");
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

    // ------------------------------------------------------------------ §7.2

    /// §7.2's one sentence, decided by the *pair* of doors rather than by the
    /// snapshot: only a run that never reached the clean exit gets its pins back,
    /// and a run that did must leave nothing for the next one - otherwise the
    /// user can never close a pin for good.
    #[test]
    fn only_an_interrupted_run_gets_its_pins_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let frame = solid(4, 3, [7, 8, 9, 255]);
        let mut s = PinSet::default();
        s.add(PinItem::from_frame(1, &frame, PhysPoint::new(0, 0), &cfg()).expect("pin"));
        let mut grab = |_id: PinId| -> Option<Frame> { Some(frame.clone()) };
        s.write_state(dir.path(), &mut grab, 1).expect("write");

        // Run A starts... and never finishes: the stamp stays, so B reads A back.
        assert_eq!(
            open_session(dir.path(), 111, 10).expect("open A"),
            PrevRun::Clean
        );
        let plan = plan_startup(dir.path(), 222, 20).expect("plan B");
        assert_eq!(plan.prev, PrevRun::Crashed, "A's stamp was still there");
        let back = plan.restore.expect("B rebuilds A's pins");
        assert_eq!(back.set.len(), 1);
        assert_eq!(back.frames[0].1, frame);

        // B ends the way the user meant it to end.
        finish_clean(dir.path()).expect("finish B");
        assert!(
            !dir.path().join(SESSION_FILE).is_file(),
            "the stamp is B's own"
        );
        assert!(!PinSet::has_state(dir.path()), "B's snapshot outlived B");

        // C, on the same directory, gets nothing - the pins B closed stay closed.
        let plan = plan_startup(dir.path(), 333, 30).expect("plan C");
        assert_eq!(plan.prev, PrevRun::Clean);
        assert!(plan.restore.is_none());
    }

    /// §7.2's first line and §8.2's rule, at the one place the rule can fail: a
    /// restore list where one picture is broken. The bad id is *named* (not just
    /// counted) so the caller can say which pin owes the user an explanation, and
    /// the pins around it travel untouched.
    #[test]
    fn one_undecodable_picture_does_not_take_the_restore_down() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f1 = solid(3, 2, [1, 1, 1, 255]);
        let f2 = solid(4, 2, [2, 2, 2, 255]);
        let f3 = solid(5, 2, [3, 3, 3, 255]);
        let mut s = PinSet::default();
        for (id, f) in [(1, &f1), (2, &f2), (3, &f3)] {
            s.add(PinItem::from_frame(id, f, PhysPoint::new(0, 0), &cfg()).expect("pin"));
        }
        let mut grab = |id: PinId| -> Option<Frame> {
            match id {
                1 => Some(f1.clone()),
                2 => Some(f2.clone()),
                3 => Some(f3.clone()),
                _ => None,
            }
        };
        s.write_state(dir.path(), &mut grab, 1).expect("write");

        // Between the write and the crash: 2's bytes rot, 3's file goes missing -
        // the two honest answers are different, and both must be said.
        std::fs::write(dir.path().join(CACHE_DIR).join("2.png"), b"not a png").expect("rot");
        std::fs::remove_file(dir.path().join(CACHE_DIR).join("3.png")).expect("rm");

        open_session(dir.path(), 1, 1).expect("stamp");
        let plan = plan_startup(dir.path(), 2, 2).expect("plan");
        let back = plan.restore.expect("a crashed run reads back");
        assert_eq!(back.set.len(), 1, "pin 1 comes back: {back:?}");
        assert!(back.set.get(1).is_some());
        assert_eq!(back.frames.len(), 1);
        assert_eq!(back.frames[0].1, f1);
        assert_eq!(back.dropped_corrupt, 1);
        assert_eq!(back.corrupt_ids, vec![2]);
        assert_eq!(back.dropped_missing, 1);
        assert_eq!(back.missing_ids, vec![3]);
    }

    /// The stamp's own durability: `finish_clean` tolerates a directory where
    /// nothing was ever written (a first crash-less run), and `open_session` on
    /// a half-written garbage stamp still reads as Crashed - its *bytes* are not
    /// the judgment, its *existence* is.
    #[test]
    fn the_clean_door_is_idempotent_and_the_stamp_needs_parsing() {
        let dir = tempfile::tempdir().expect("tempdir");
        finish_clean(dir.path()).expect("nothing to finish");
        finish_clean(dir.path()).expect("still nothing");

        std::fs::write(dir.path().join(SESSION_FILE), b"half a sentence").expect("plant");
        assert_eq!(
            open_session(dir.path(), 9, 9).expect("open"),
            PrevRun::Crashed
        );
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

    // ------------------------------------------------------------ §5.8.3 那一批文件

    /// 一次点选的三枚好文件，交回的必须是**三条**记录。
    ///
    /// 判的是数目而不是措辞：「一次选 3 个文件应得 3 扇窗」这一条在散文里写三遍也不成立，
    /// 它成立的地方是名册的长度。
    #[test]
    fn three_picked_files_are_three_entries_in_the_roster() {
        let paths: Vec<PathBuf> = ["a.png", "b.jpg", "c.gif"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let mut asked = Vec::new();
        let batch = read_picture_batch(&paths, |p, _index| {
            asked.push(p.to_path_buf());
            Ok(asked.len() as PinId - 1)
        });
        assert_eq!(batch.windows(), 3, "三枚都得成为三格，而不是一格加两句散文");
        assert_eq!(batch.trouble(), 0);
        assert_eq!(
            batch.pinned.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "名册给的 id 要一枚不落、按挑的顺序记下"
        );
        assert_eq!(asked, paths, "每一枚都要真递进读的那一只手，不许提前收工");
    }

    /// §5.8.3 最后一句 + §7.2 的第一句，同一条判据的两半：**一枚坏的吃掉不了后面那几枚**，
    /// 而每一枚坏的各有一句自己的说法。
    #[test]
    fn one_bad_file_costs_its_own_sentence_and_not_the_ones_behind_it() {
        let paths: Vec<PathBuf> = ["good.png", "damaged.png", "notes.txt", "last.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let batch = read_picture_batch(&paths, |p, _| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with("good") || name.starts_with("last") {
                Ok(name.len() as PinId)
            } else if name.starts_with("damaged") {
                Err(format!("{name} 读不出图片：文件可能已损坏。"))
            } else {
                Err(format!("{name} 不是本程序会读的图片。"))
            }
        });

        // 好的那几枚出来了 —— 包括排在坏的那两枚**后面**的那一枚。
        assert_eq!(
            batch
                .pinned
                .iter()
                .map(|(p, _)| p.as_str())
                .collect::<Vec<_>>(),
            vec!["good.png", "last.png"],
            "坏档后面那枚好的必须照样贴出来，这正是 §7.2 那一半"
        );
        // 坏的那两枚各有一句，而且各说各的：两句不许撞成一句、也不许只报第一句就收工。
        assert_eq!(batch.failed.len(), 2);
        let said: Vec<&str> = batch.failed.iter().map(|f| f.why.as_str()).collect();
        assert_eq!(
            said,
            vec![
                "damaged.png 读不出图片：文件可能已损坏。",
                "notes.txt 不是本程序会读的图片。"
            ],
            "两种坏要说两种话，而原话一个字不许被本模块改写"
        );
        assert_eq!(
            batch
                .failed
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["damaged.png", "notes.txt"],
            "每枚坏的得留下自己的名字，用户才知道去机器上看哪一份"
        );
        assert_ne!(said[0], said[1], "「损坏」与「格式不支持」不许混成同一句");
    }

    /// 真解码器也得站在这一条上过一次：一份好 PNG、一份**截了半截**的 PNG、一份纯文本。
    ///
    /// 上面那一格判的是账本的形状，这一格判的是「这一批东西真递进 `encode::decode_file`
    /// 会怎么样」。两格缺一不可 —— 只有前者，账本可以对着一个永远不会出现的错误分布成立。
    #[test]
    fn the_real_decoder_sorts_one_good_one_damaged_and_one_unreadable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let whole = Frame::filled(8, 6, [7, 130, 200, 255]).expect("frame");
        let mut opt = EncodeOptions::default();
        opt.format = Format::Png;

        let good = dir.path().join("good.png");
        encode::save(&whole, &good, &opt).expect("write png");
        let intact = std::fs::read(&good).expect("read back");

        // 同一份 PNG，留下文件头那几枚字节：格式认得出来，像素读不回来 —— 那是「损坏」。
        let damaged = dir.path().join("damaged.png");
        std::fs::write(&damaged, &intact[..8]).expect("truncate");
        // 一份压根不是图片的东西：连格式都认不出来 —— 那是「格式不支持」。
        let foreign = dir.path().join("notes.txt");
        std::fs::write(&foreign, b"not a picture at all").expect("write text");

        let paths = vec![good.clone(), damaged.clone(), foreign.clone()];
        let batch = read_picture_batch(&paths, |p, _| {
            encode::decode_file(p)
                .map(|f| f.width as PinId)
                .map_err(|e| e.to_string())
        });

        assert_eq!(
            batch.pinned,
            vec![(good.display().to_string(), 8)],
            "好的那一枚必须出来，而且带回来的就是它自己那 8 像素宽"
        );
        assert_eq!(batch.failed.len(), 2, "两枚坏的都要留下一句说法");
        assert_eq!(batch.windows(), 1);

        // 桥那一只手（`state.rs` 的 `read_refusal`）分这两种坏靠的就是扩展名读不读得出格式，
        // 所以这里把判据本身钉住：这两份夹具必须在它那一侧落成**两种**答案。
        let ext_of = |p: &Path| {
            p.extension()
                .and_then(|x| x.to_str())
                .and_then(encode::Format::from_ext)
        };
        assert_eq!(
            ext_of(&damaged),
            Some(Format::Png),
            "损坏那枚：名字说是 PNG"
        );
        assert_eq!(ext_of(&foreign), None, "不支持那枚：名字不说是任何一种图片");
    }

    /// 超出上界的那几枚**必须出声**：不读、不建窗，也不许静默丢下（#433）。
    #[test]
    fn the_files_beyond_the_cap_are_declined_out_loud() {
        let count = FILE_PIN_MAX + 2;
        let paths: Vec<PathBuf> = (0..count)
            .map(|i| PathBuf::from(format!("{i}.png")))
            .collect();
        let mut read = 0usize;
        let batch = read_picture_batch(&paths, |_, _| {
            read += 1;
            Ok(read as PinId)
        });
        assert_eq!(batch.windows(), FILE_PIN_MAX, "上界以内的全成");
        assert_eq!(read, FILE_PIN_MAX, "越界的那几枚一枚也不许读进来");
        assert_eq!(batch.declined.len(), 2);
        assert_eq!(
            batch.failed.len(),
            0,
            "被上界挡下不是「读不出来」，两格不许混"
        );
        for (i, off) in batch.declined.iter().enumerate() {
            assert!(off.why.contains("一次最多贴"), "{off:?}");
            assert!(off.path.ends_with(&format!("{}.png", FILE_PIN_MAX + i)));
        }
        let line = batch.said();
        assert!(
            line.contains(&format!("已贴出 {FILE_PIN_MAX} 张贴图")),
            "{line}"
        );
        assert!(line.contains("另有 2 枚没有贴出来"), "{line}");
    }

    /// 取消那一手：一挑选不出来，屏幕上也得有一句，而不是「什么都没发生」。
    #[test]
    fn an_empty_batch_still_answers_with_a_sentence() {
        let batch = read_picture_batch(&[], |_, _| Ok(0));
        assert_eq!(batch, FilePinBatch::default());
        assert_eq!(batch.windows(), 0);
        assert_eq!(batch.trouble(), 0);
        assert_eq!(batch.said(), "没有选中文件，一张也没有贴。");
    }

    /// 屏幕上那一行只引前几枚的原因，多出来的报个数而不假装不存在。
    #[test]
    fn the_floating_line_counts_every_failure_and_quotes_only_the_first_few() {
        let paths: Vec<PathBuf> = (0..5).map(|i| PathBuf::from(format!("b{i}.png"))).collect();
        let batch = read_picture_batch(&paths, |_, _| Err("读不出图片".to_string()));
        assert_eq!(batch.failed.len(), 5);
        let line = batch.said();
        assert!(line.starts_with("5 枚都没有贴出来"), "{line}");
        assert_eq!(
            line.matches("读不出图片").count(),
            FILE_PIN_SAID_MAX,
            "屏幕上那一行引 {FILE_PIN_SAID_MAX} 句就该收住"
        );
        assert!(line.contains("其余 2 枚见运行历史"), "{line}");
    }

    /// 一次点选三枚得到的是三扇**位置各不同**的窗。
    ///
    /// 这一格盯着的是 `windows() == 3` 唯一冒充得过去的那条缝：三枚全落在同一个点上，名册
    /// 确实有三格、屏幕却只有一扇。阶梯用的是既有那一只 `next_drop_point()` fallback 已经在
    /// 用的同一枚 16，不是这里新编的数。
    #[test]
    fn three_files_land_on_three_different_points() {
        let base = PhysPoint::new(100, 200);
        let spots: Vec<PhysPoint> = (0..3).map(|i| cascade_at(base, i)).collect();
        assert_eq!(spots[0], base, "第一枚就站在既有规则交回的那个点上");
        assert_eq!(spots[1], PhysPoint::new(116, 216));
        assert_eq!(spots[2], PhysPoint::new(132, 232));
        let mut unique = spots.clone();
        unique.sort_by_key(|p| (p.x, p.y));
        unique.dedup();
        assert_eq!(
            unique.len(),
            3,
            "三枚落位两两不同，否则三扇窗在屏幕上是扇形的"
        );

        // 阶梯走到 i32 那一头不许回卷成屏幕上另一头的一枚点。
        let edge = cascade_at(PhysPoint::new(i32::MAX - 8, i32::MAX - 8), 10);
        assert_eq!(edge, PhysPoint::new(i32::MAX, i32::MAX));
    }

    /// 阶梯认的是**挑的顺序**，不是**成功的顺序**：中间坏掉的那一枚也占一格。
    ///
    /// 这一格盯的是 `read` 收到的第二枚参数。桥那边把它直接喂给 [`cascade_at`]，所以序号若
    /// 在这里被改成「已成功了几枚」，屏幕上就会出现一种很难看出的坏：第三枚挑的文件落到第二
    /// 格阶梯上，与它前面那枚挤成部分重叠，而名册与 `windows()` 全都还是对的。数目对、位置错
    /// 是这一族最难由「几扇窗」这类断言抓到的，所以这里断言的是序号本身。
    #[test]
    fn the_step_follows_the_pick_order_not_the_successes() {
        let paths: Vec<PathBuf> = ["a.png", "broken.png", "c.png", "d.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let mut seen: Vec<usize> = Vec::new();
        let base = PhysPoint::new(0, 0);
        let mut spots: Vec<PhysPoint> = Vec::new();
        let batch = read_picture_batch(&paths, |p, index| {
            seen.push(index);
            if p.file_name().unwrap().to_string_lossy() == "broken.png" {
                return Err("读不出图片：文件可能已损坏。".to_string());
            }
            spots.push(cascade_at(base, index));
            Ok(index as PinId)
        });
        assert_eq!(seen, vec![0, 1, 2, 3], "四枚都得问一遍，序号按挑的排");
        assert_eq!(batch.windows(), 3);
        assert_eq!(
            spots,
            vec![
                base,
                PhysPoint::new(2 * CASCADE_STEP_PX, 2 * CASCADE_STEP_PX),
                PhysPoint::new(3 * CASCADE_STEP_PX, 3 * CASCADE_STEP_PX)
            ],
            "坏掉的那枚占住它自己那一格，后面两枚不许往前挤"
        );
    }

    /// 「支持 PNG、JPG、BMP、TGA、ICO、TIFF 和 GIF」这一句在对话框那一头就是这一串过滤器。
    ///
    /// 判的是它与 §5.8.3 那一列**不多不少**：少一种是托盘上开不出来的功能，一种是这一节没有
    /// 许过的承诺。写死的七枚在这里是量具，不是散文 —— 它读的还是 `encode::IMPORT_FORMATS`
    /// 那唯一的一份名单。
    #[test]
    fn the_dialog_filter_is_the_seven_formats_prd_names_and_nothing_else() {
        let filter = picture_filter_patterns();
        assert_eq!(
            filter, "*.png;*.jpg;*.bmp;*.tga;*.ico;*.tiff;*.gif",
            "七种、按 IMPORT_FORMATS 的顺序、分号分隔"
        );
        let listed: Vec<&str> = filter.split(';').collect();
        assert_eq!(listed.len(), encode::IMPORT_FORMATS.len());
        for fmt in encode::IMPORT_FORMATS {
            assert!(
                listed.contains(&format!("*.{}", fmt.ext()).as_str()),
                "{fmt:?} 在名单里却不在过滤器里"
            );
        }
        assert!(!filter.contains("webp"), "§5.8.3 那一列没有 WebP");
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
