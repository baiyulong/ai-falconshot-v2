//! The capture mask's live state: which screens it covers, the one frozen frame
//! each is showing, and where the hole is.
//!
//! Two rules decide the shape of this module.
//!
//! The first is [`crate::capture`]'s: the screen is read **once**, before any mask
//! window exists, and every later question - the crop, the colour pick, the
//! magnifier - is answered from that snapshot. So `open` takes the frames it
//! publishes rather than taking a capture of its own, and a flow that needs a
//! fresh picture calls `capture::freeze` first.
//!
//! The second is that a mask is *not* a pin. A pin is a picture the user keeps,
//! addressed by a `PinId`; a mask is a transient full-screen texture addressed by
//! a key, and there is one per screen rather than one per picture. That is why the
//! provider gets a second store keyed by string (`pinIdOf` reads anything it
//! cannot parse as -1, so negative ids are not addressable and a mask has no id at
//! all), and why the slots live here instead of in [`crate::state`].
//!
//! Geometry arrives in two spaces on purpose: the hole is in physical desktop
//! pixels, because that is the space the selection and the crop are decided in,
//! and each slot converts its part of it into the window-local
//! device-independent pixels QML binds to. On a 200% screen the conversion is
//! where a mask that looks right and crops wrong gets caught.

use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use falcon_core::capture::ScreenSnapshot;
use falcon_core::encode;
use falcon_core::geometry::{
    handle_at, nudge, nudge_edge, resize, Handle, PhysPoint, PhysRect, Scale, DEFAULT_HIT_SLOP,
};

use crate::annotate;
use crate::capture;
use crate::mask_view::shim;

/// How far the desktop is pushed toward black outside the selection. One number,
/// because QML draws it, the shader multiplies by it and `--mask` asserts a
/// readback against `1 - DIM` - three places that have to disagree by design.
pub const DIM: f64 = 0.6;

/// A screen the mask covers.
#[derive(Clone, Debug)]
pub struct Slot {
    /// Index into Qt's screen list, which is what the QML `Instantiator` repeats.
    pub index: usize,
    /// The capture layer's name for the same monitor, for the report to line up.
    pub name: String,
    /// Provider key, `name-revision`.
    pub key: String,
    /// Physical bounds, from the snapshot - the space the hole lives in.
    pub bounds: PhysRect,
    /// The same screen in device-independent pixels, from Qt - the space a
    /// `Window`'s x/y/width/height live in.
    pub geom: PhysRect,
    pub scale: Scale,
    pub revision: u32,
    pub swaps: u32,
    /// Milliseconds from `open` to this window's first presented frame, which is
    /// where the GPU warm-up the spike measured at 62-117 ms shows up. `None`
    /// until a frame has been presented.
    pub first_swap_ms: Option<u64>,
    /// `ShaderEffect.status`, once QML has reported it. `None` means the item was
    /// never constructed, which is a different fact from `Error`.
    pub shader_status: Option<i32>,
}

/// What one mask window binds to. Plain fields so a test can read it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaskViewData {
    /// False once the mask is closed - the window hides itself.
    pub live: bool,
    /// The user is looking at a mask right now.
    pub shown: bool,
    pub geom: PhysRect,
    /// The selection, in window-local DIP. Empty when the hole is on another
    /// screen, which is how a two-monitor mask draws nothing rather than a band.
    pub hole: PhysRect,
    pub scale: Scale,
    /// §5.7.11's 文本 field: the box the layer is holding a string in, in window-local
    /// DIP, and whether there is one at all. QML owns the field and Rust owns the box,
    /// because the number that sizes it comes from the font leg - and a field whose
    /// width is QML's guess about somebody else's text is how a typed line lands outside
    /// the box it was measured in.
    pub typing: PhysRect,
    pub typing_live: bool,
    /// The pen's 字号 in DIP, for the same reason and by the same division.
    pub font_dip: u32,
    pub revision: u32,
    pub swaps: u32,
    pub first_swap_ms: Option<u64>,
    pub shader_status: Option<i32>,
}

/// What the pointer took hold of, in physical desktop pixels.
#[derive(Clone, Copy, Debug)]
pub struct Grab {
    /// `None` is not "nothing held" - it is "a rectangle starts here", which is the
    /// only grip that can create a selection. `Some(Body)` moves the whole rect,
    /// and the other eight resize it (§5.3.1's "拖动边缘或角点调整选区").
    pub handle: Option<Handle>,
    /// The press point: the fixed corner of a new rectangle, and the reference for
    /// a move.
    pub anchor: PhysPoint,
    /// The rect the grab began from. Resizing from `start` rather than from the
    /// current hole is what stops a long drag from accumulating rounding.
    pub start: PhysRect,
}

#[derive(Default)]
pub struct MaskState {
    pub slots: Vec<Slot>,
    /// The selection in physical desktop pixels.
    pub hole: PhysRect,
    /// §5.7's annotations, drawn on top of this flow's frozen frames. Its base is
    /// the same snapshot the slots' textures came from, so ink and picture cannot
    /// be describing two different captures.
    pub ink: annotate::Layer,
    /// The pointer's grip since the last `press`, or `None` when it is not held.
    /// Kept here rather than in QML because the arithmetic it feeds - which edge
    /// moved, and how far - is the part that can be tested without a window.
    pub grab: Option<Grab>,
    /// True = one ShaderEffect pass; false = four dim `Rectangle`s, which is also
    /// what the `QT_QUICK_BACKEND=software` fallback has to be able to draw.
    pub shader: bool,
    pub shown: bool,
    /// Set at the end of `open`, so the swap timings mean "since the mask was
    /// asked for", not "since the process started".
    started: Option<Instant>,
    /// The whole desktop as the snapshot saw it, kept for §5.2.5's clipping and
    /// for `desktop()` - a fold over the slots would count the origin as desktop
    /// whenever the monitors do not start there.
    pub virtual_bounds: PhysRect,
    /// Pixels handed to Qt, and how long that took - the part of the 150 ms
    /// promise that is this module's to keep.
    pub pixels_ms: Option<u64>,
    /// What the last delivery attempt said, in the user's own language, and `""`
    /// when the flow has nothing to report.
    ///
    /// It lives in Rust rather than in a return value for the same reason the hole
    /// does: two mask windows read this one state, so a sentence only the clicked
    /// window knew is a sentence the other screen never shows. It is cleared by
    /// `open`, so a fresh capture cannot start out wearing the last one's answer.
    pub message: String,
    /// §5.5.5's export knobs, taken at `open` for the same reason the pens are: the
    /// settings file is read while the mask is being put up, not in front of a user
    /// who has already waited for a save. `format` is what a name typed without an
    /// extension becomes, and `quality` is what JPG is written at.
    ///
    /// `Default` is `{Png, 92, true}`, which is what a first run reads out of the file
    /// anyway - so a test that never calls `open` saves at the same numbers a user
    /// with no `config.toml` does, rather than at quality 0.
    pub export: encode::EncodeOptions,
    /// Bumped by every `open`, so a second capture in the same process gets keys
    /// nobody has asked for yet. A constant revision would reuse the previous
    /// frame's key, and `Image.cache: false` only re-requests when the URL changes:
    /// a "re-capture" would then show the old freeze, which is exactly the bug
    /// §5.2.4 cannot survive.
    next_revision: u32,
    pub problems: Vec<String>,
}

impl MaskState {
    /// Freeze → publish → one slot per screen. Returns the slot count.
    ///
    /// A screen Qt lists that the snapshot does not have is a `problem` and no
    /// slot, not a silent skip: a mask that covers two of three monitors is the
    /// kind of bug that otherwise gets filed against the capture path.
    pub fn open(&mut self, snap: &ScreenSnapshot, shader: bool) -> Result<usize, String> {
        if snap.monitors.is_empty() {
            return Err("快照里没有任何显示器".into());
        }
        self.shader = shader;
        self.hole = Self::default_hole(snap);
        self.pixels_ms = None;
        self.message.clear();

        let started = Instant::now();
        let screens = shim::screens();
        if screens.is_empty() {
            return Err("Qt 没有报告任何屏幕".into());
        }

        let mut used = vec![false; snap.monitors.len()];
        let mut slots = Vec::new();
        let revision = self.take_revision();
        for (index, screen) in screens.iter().enumerate() {
            let Some(frozen) = Self::match_monitor(snap, screen, &mut used, index) else {
                self.problems.push(format!(
                    "screen {} {}x{} dpr={:.2} has no captured monitor",
                    screen.name, screen.w, screen.h, screen.dpr
                ));
                continue;
            };
            let info = &snap.monitors[frozen].info;
            let frame = &snap.monitors[frozen].frame;
            let key = Self::frame_key(&info.id, revision);
            shim::store_raw(&key, &frame.pixels, frame.width, frame.height);
            slots.push(Slot {
                index,
                name: info.id.clone(),
                key,
                bounds: info.bounds,
                geom: PhysRect::new(
                    screen.x,
                    screen.y,
                    screen.w.max(1) as u32,
                    screen.h.max(1) as u32,
                ),
                scale: Scale::of(screen.dpr),
                revision,
                swaps: 0,
                first_swap_ms: None,
                shader_status: None,
            });
        }
        if slots.is_empty() {
            return Err("Qt 的屏幕和快照的显示器配不上对".into());
        }

        self.slots = slots;
        // The annotation layer takes its base from the same snapshot, after the
        // slots exist: one flow, one freeze, and the ink's clip is this flow's hole.
        self.ink.begin(snap, &self.slots);
        // §5.7.1's per-tool memory arrives from the settings file here, after the
        // flow has been cleared: `begin` resets the layer, so pens handed over before
        // it would be thrown away by the reset. This is the one place the hot key
        // touches the file, and it costs what `[settings] apply=` reports.
        crate::settings::apply_to(&mut self.ink);
        self.export = crate::settings::export();
        self.ink.set_hole(self.hole);
        self.virtual_bounds = snap.virtual_bounds;
        self.shown = true;
        self.pixels_ms = Some(started.elapsed().as_millis() as u64);
        self.started = Some(Instant::now());
        crate::state::stamp("opened");
        Ok(self.slots.len())
    }

    /// Claim the next frame revision, one-based and never repeated.
    ///
    /// Split out of `open` because the rest of `open` needs Qt (the screen list and
    /// the frame store), while this is the part whose mistake is invisible: with a
    /// constant revision a re-capture publishes the same provider key as the capture
    /// before it, the `Image`'s URL does not change, and the overlay keeps showing
    /// the old freeze while reporting a new one.
    fn take_revision(&mut self) -> u32 {
        let revision = self.next_revision.max(1);
        self.next_revision = revision + 1;
        revision
    }

    /// The provider key for one screen's frame at one revision.
    fn frame_key(name: &str, revision: u32) -> String {
        format!("{name}-{revision}")
    }

    /// The selection a mask opens with: the middle half of the primary monitor,
    /// or of the whole desktop if the enumeration names no primary. A fixed shape
    /// on purpose - the hole the user drags is M3's, and until then the only
    /// honest test of a hole is one whose size is known in advance.
    fn default_hole(snap: &ScreenSnapshot) -> PhysRect {
        let b = snap
            .monitors
            .iter()
            .find(|m| m.info.primary)
            .map(|m| m.info.bounds)
            .unwrap_or(snap.virtual_bounds);
        PhysRect::new(b.x + b.w as i32 / 4, b.y + b.h as i32 / 4, b.w / 2, b.h / 2)
    }

    /// Qt's screen `i` to a snapshot monitor, by physical size.
    ///
    /// Size rather than name because the two enumerations do not speak the same
    /// names (`DISPLAY1` against `m0-display1`) and index order is not promised by
    /// either. At mixed scale, DIP times one screen's dpr is that screen's physical
    /// size, which is enough to pair them; two identical monitors at the same scale
    /// fall back to index order, and a genuine mismatch is reported, not guessed at.
    fn match_monitor(
        snap: &ScreenSnapshot,
        screen: &shim::Screen,
        used: &mut [bool],
        index: usize,
    ) -> Option<usize> {
        let near = |a: i32, b: i32| (a - b).abs() <= 1;
        let phys = |dip: i32, ratio: f64| (dip as f64 * ratio).round() as i32;
        let mut candidates: Vec<usize> = Vec::new();
        for (i, m) in snap.monitors.iter().enumerate() {
            if used[i] {
                continue;
            }
            if near(phys(screen.w, screen.dpr), m.info.bounds.w as i32)
                && near(phys(screen.h, screen.dpr), m.info.bounds.h as i32)
            {
                candidates.push(i);
            }
        }
        if candidates.is_empty() {
            return None;
        }
        let chosen = candidates
            .iter()
            .copied()
            .find(|i| *i == index)
            .unwrap_or(candidates[0]);
        used[chosen] = true;
        Some(chosen)
    }

    /// The mask is over: take the textures out of Qt's process, all of them, since
    /// a 4K desktop is 24 MB per screen and a leak here is a leak the user sees.
    pub fn close(&mut self) {
        for slot in &self.slots {
            shim::drop_frame(&slot.key);
        }
        self.slots.clear();
        self.grab = None;
        self.shown = false;
        // The ink's canvases are the same size as the frames they sit on, so a flow
        // that ended without this leaked two 24 MB textures per screen, not one.
        self.ink.end();
        // What this flow taught the pens goes back to the settings file now, not at
        // exit: a user who signs out without the process ever running its teardown
        // still gets the pen they set last time. `end` keeps `styles` and drops
        // everything else, so reading the layer here is reading the answer, not the
        // draft.
        crate::settings::write_back(&self.ink);
    }

    /// The selection, in physical desktop pixels. Each slot re-reads its part of
    /// it on the next `reload`; making every window redraw without waiting for its
    /// own reload is the M3 selection flow's, not this one's.
    ///
    /// The annotation layer is told as well: its clip is this rect, and an ink pixel
    /// the user has just dragged out of the selection must stop being on screen.
    pub fn set_hole(&mut self, hole: PhysRect) {
        self.hole = hole;
        self.ink.set_hole(hole);
    }

    pub fn set_shader(&mut self, shader: bool) {
        self.shader = shader;
    }

    // ------------------------------------------------------------ selection
    //
    // The pointer arrives in window-local device-independent pixels because that is
    // what Qt Quick reports, and every answer here is in physical desktop pixels
    // because that is the space the crop happens in. One conversion, in
    // [`MaskState::desk_point`], is the whole boundary.

    /// A press inside an existing selection moves it rather than starting a new
    /// one; a press outside draws. PRD §5.3.1 only says a drag makes a rectangle
    /// and the edges adjust it, so this is this project's decision, and the reason
    /// is that a mis-drawn selection is cheaper to move than to re-draw.
    ///
    /// A *drawing* tool takes the press first, though - §5.7.1 step 4's 在选区内绘制
    /// is about the pen, not the selection - and then nothing of the selection is
    /// held: `grab` stays `None` so a later `drag` cannot move the hole out from
    /// under the stroke. Getting the selection's handles back is the arrow tool's
    /// (code 0 in [`crate::annotate::TOOLS`]), which is the same reason a real
    /// screenshot toolbar has one.
    pub fn press(&mut self, index: usize, x: f64, y: f64) -> Option<Handle> {
        let p = self.desk_point(index, x, y)?;
        if self.ink.press(p) {
            self.grab = None;
            return None;
        }
        let slop = self.hit_slop(index);
        let hit = if self.hole.is_empty() {
            None
        } else {
            handle_at(&self.hole, p, slop)
        };
        self.grab = Some(match hit {
            Some(h) => Grab {
                handle: Some(h),
                anchor: p,
                start: self.hole,
            },
            None => Grab {
                handle: None,
                anchor: p,
                start: PhysRect::default(),
            },
        });
        if hit.is_none() {
            // Nothing is drawn yet: a hole of zero size at the anchor reads as "no
            // selection" to the dim, so the desktop stays fully dark until the
            // pointer actually moves.
            self.set_hole(PhysRect::new(p.x, p.y, 0, 0));
        }
        hit
    }

    /// The pointer moved while held. A new rectangle is normalised from the anchor,
    /// so dragging up-and-left works as well as down-and-right.
    ///
    /// A stroke mid-drag is not a selection drag: the drawing tool already took the
    /// press, so the point goes to the ink and the hole is left alone.
    pub fn drag(&mut self, index: usize, x: f64, y: f64) {
        let Some(p) = self.desk_point(index, x, y) else {
            return;
        };
        if self.ink.dragging() {
            self.ink.drag(p);
            return;
        }
        let Some(g) = self.grab else { return };
        let bounds = self.desktop();
        self.set_hole(match g.handle {
            None => PhysRect::from_points(g.anchor, p),
            // Not `resize`'s `Body` branch: that puts the rectangle's origin under
            // the pointer, which is right for a tool you click to place and wrong
            // for a selection you grabbed in the middle - the rect would jump by
            // half its size on the first pixel of the drag.
            Some(Handle::Body) => g
                .start
                .offset(p.x - g.anchor.x, p.y - g.anchor.y)
                .fitted_into(&bounds),
            Some(h) => resize(&g.start, h, p, &bounds),
        });
    }

    /// The pointer let go: the selection's grab is dropped, and a half-drawn stroke
    /// becomes an object (or is discarded) in the layer.
    pub fn release(&mut self) {
        self.grab = None;
        if self.ink.dragging() {
            self.ink.release();
        }
    }

    /// A point clamped into the desktop, in physical pixels.
    fn desk_point(&self, index: usize, x: f64, y: f64) -> Option<PhysPoint> {
        let slot = self.slot(index)?;
        let r = slot.scale.ratio();
        let p = PhysPoint::new(
            slot.bounds.x + (x * r).round() as i32,
            slot.bounds.y + (y * r).round() as i32,
        );
        let d = self.desktop();
        Some(PhysPoint::new(
            p.x.clamp(d.x, d.right()),
            p.y.clamp(d.y, d.bottom()),
        ))
    }

    /// The grab radius is a distance a finger travels, so it is stated in
    /// device-independent pixels and this screen's scale turns it into physical
    /// ones. Five DIP is `falcon_core`'s `DEFAULT_HIT_SLOP`, the same number the
    /// annotation handles use.
    fn hit_slop(&self, index: usize) -> u32 {
        self.slot(index)
            .map(|s| s.scale.dip_to_phys_i(DEFAULT_HIT_SLOP as i32).max(1) as u32)
            .unwrap_or(DEFAULT_HIT_SLOP)
    }

    /// Arrow keys (§5.3.9). The step is this project's: one physical pixel, ten with
    /// a modifier, and `resize_edge` moves the bottom-right edge instead of the
    /// whole rect. Both helpers clamp to the desktop, which is the rule the PRD
    /// does fix - "不允许选区超出有效截图区域".
    pub fn nudge_by(&mut self, dx: i32, dy: i32, resize_edge: bool) {
        if self.hole.is_empty() {
            return;
        }
        let bounds = self.desktop();
        self.set_hole(if resize_edge {
            nudge_edge(&self.hole, Handle::Se, dx, dy, &bounds)
        } else {
            nudge(&self.hole, dx, dy, &bounds)
        });
    }

    /// Esc. `true` means this press consumed itself - on a half-typed 文本 box or on the
    /// selection;
    /// `false` means there was nothing to clear, so the caller cancels the capture.
    /// Two windows both receiving one Esc press would then clear-and-cancel in a
    /// single keystroke, which is why the ladder is a return value and not a
    /// property QML reads.
    pub fn escape(&mut self) -> bool {
        // §5.7.11's box is the first rung, because it is the only thing in this ladder
        // that is not in any history yet: the typed string is not an object, so a rung
        // that un-selected instead would take the letters with it - and the selection
        // one rung down clears the box anyway, through `Layer::set_hole`.
        if self.ink.abandon_typing() {
            return true;
        }
        if self.hole.is_empty() {
            return false;
        }
        self.grab = None;
        self.set_hole(PhysRect::default());
        true
    }

    /// Enter or a double-click. `None` when there is no selection to confirm - the
    /// mask stays up rather than inventing a crop the user never drew.
    pub fn commit(&mut self) -> Option<PhysRect> {
        self.grab = None;
        (!self.hole.is_empty()).then_some(self.hole)
    }

    pub fn has_selection(&self) -> bool {
        !self.hole.is_empty()
    }

    /// §5.5.2's 复制: the annotated selection onto the clipboard, with a sentence
    /// beside it either way.
    ///
    /// Three rungs, in this order, because each costs what the one above it says is
    /// worth paying. Nothing selected is the user's own doing and needs no probe. The
    /// probe (`clip::usable`) runs before any pixel is moved, because a clipboard
    /// another process owns is an environment fact the user can act on - and PRD §7.2
    /// asks for the holder's name rather than "Access is denied". Only then does the
    /// write happen, through [`crate::annotate::Layer::deliver`], which is the one
    /// place a selection is stitched: what lands on the clipboard here is byte-for-byte
    /// what §5.5.4's 保存 will write to a file.
    ///
    /// This does **not** end the flow. The PRD stops at "系统将图片写入剪贴板"
    /// (§5.5.2 step 3), and closing has a cost this project is not entitled to pay on
    /// the user's behalf: the freeze, the ink and the message all go with the mask, so
    /// a copy that closed the overlay would take its own answer with it. Ending the
    /// capture is 取消's and, from M4b, 完成的.
    pub fn copy_out(&mut self) -> bool {
        if !self.has_selection() {
            self.message = "还没有选区：先把要复制的范围拖出来。".to_string();
            return false;
        }
        let Some(frame) = self.ink.deliver() else {
            self.message = "选区的画面拼不出来：截图还在，可以再复制一次。".to_string();
            return false;
        };
        if let Err(e) = platform_windows::clip::usable() {
            self.message = format!("{e}。截图还在，可以先关掉占用它的程序再复制。");
            return false;
        }
        match platform_windows::clip::write_image(&frame) {
            Ok(()) => {
                self.message = format!("已复制 {}×{} 像素。", frame.width, frame.height);
                true
            }
            Err(e) => {
                self.message = format!("{e}。截图还在，可以再复制一次。");
                false
            }
        }
    }

    /// §5.5.5's 另存为: the annotated selection to a file, in the format the name the
    /// user typed says, through the dialog step 2 of that section asks for.
    ///
    /// The first three rungs are [`Self::copy_out`]'s, in the same order and for the
    /// same reasons - nothing selected is the user's own doing and needs no probe, and
    /// a selection that cannot be stitched is a refusal that has to be said *before*
    /// anything is written, not a file that turns out to be nothing. What is new is
    /// only what happens to the bytes: [`falcon_core::encode`] rather than
    /// `QImage::save` - P5's rule for every byte this app puts in a file - and
    /// `encode::save`'s own tmp+rename, because half a PNG on disk is worse than a
    /// refusal (PRD §8.3). The frame that goes in is the one `deliver()` stitched, so
    /// what 复制 put on the clipboard and what 保存 put on disk are the same picture
    /// rather than two answers to the same selection.
    ///
    /// A name without an extension is not a refusal: the dialog was showing the filter
    /// for PNG while the user typed `foo`, so `foo` means §5.5.5's configured format,
    /// and the file that appears is `foo.png`.
    ///
    /// The fourth sentence is the reason this round exists. Three rows of the PRD ask
    /// for it - §5.5.5 line 506 ("包含透明背景时，应推荐 PNG"), §5.4.5's line 447
    /// ("不支持透明度的输出格式应提醒用户背景将被填充") and §5.5.6's line 521 - and all
    /// three are true of the same write, because `flatten_on_lossy` paints those pixels
    /// white and the file is then not the picture the user was looking at. The count
    /// travels with the sentence: "有透明区域" alone is boilerplate, and P33 measured
    /// what one selection's eraser actually costs (`11684 px`), so the number is the
    /// difference between a prompt and a shrug.
    ///
    /// The write still happens. The extension was the user's own choice in a file
    /// dialog, this mask has one message line and no modal, and refusing a legitimate
    /// format is a worse reading of "应提醒" than doing the thing and naming what it
    /// cost. A pre-write confirm rung - the other way to read those lines - is a
    /// decision this round records rather than makes.
    ///
    /// Like 复制, this does not end the flow: the freeze, the ink and the sentence all
    /// go with the mask, and a save that closed the overlay would take the picture the
    /// user may still want to copy with it.
    pub fn save_out(&mut self, path: &str) -> bool {
        if !self.has_selection() {
            self.message = "还没有选区：先把要保存的范围拖出来。".to_string();
            return false;
        }
        let Some(frame) = self.ink.deliver() else {
            self.message = "选区的画面拼不出来：截图还在，可以再保存一次。".to_string();
            return false;
        };
        let typed = std::path::Path::new(path);
        let (target, format) = match typed.extension() {
            Some(ext) => (
                typed.to_path_buf(),
                encode::Format::from_ext(&ext.to_string_lossy()),
            ),
            None => {
                let mut named = typed.as_os_str().to_os_string();
                named.push(".");
                named.push(self.export.format.ext());
                (std::path::PathBuf::from(named), Some(self.export.format))
            }
        };
        let Some(format) = format else {
            self.message = format!("{path} 不是本程序会写的格式：用 .png、.jpg 或 .bmp。");
            return false;
        };
        // Counted before the write, from the frame that is about to be encoded: asking
        // the file afterwards would be asking a JPG whether it kept an alpha channel.
        let filled = if format.supports_alpha() || !frame.has_transparency() {
            0
        } else {
            frame
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[3] != 255)
                .count()
        };
        let opt = encode::EncodeOptions {
            format,
            ..self.export
        };
        match encode::save(&frame, &target, &opt) {
            Ok(()) => {
                // The selection's size, not the file's: `encode` rescales an ICO down to
                // 256 before it writes, and a success sentence that quoted the frame
                // would be quoting a picture the file is not. What the user selected is
                // true of every format, and is the number they recognise.
                self.message = format!(
                    "已保存到 {}（选区 {}×{} 像素）。{}",
                    target.display(),
                    frame.width,
                    frame.height,
                    if filled == 0 {
                        String::new()
                    } else {
                        format!(
                            "选区里有 {filled} px 透明，{} 存不下，已填成白色；要保留透明请存成 PNG。",
                            format.label()
                        )
                    }
                );
                true
            }
            Err(e) => {
                self.message = format!("{e}。截图还在，可以再保存一次。");
                false
            }
        }
    }

    /// §5.7.11's 文本, §5.7.5's 折线 and §5.7.13's 放大 finishers, in that order, in
    /// this one place: 双击 and `Enter` mean four things to this mask (close a box, close
    /// a line, place a copy, crop the picture) and only the layer knows which is pending.
    /// `true` is "the keystroke closed something of mine", which is what tells the caller
    /// the crop is not this press's - and a ladder spelled in QML is two ladders.
    ///
    /// 文本 goes first because it is the one holding the keyboard: while a box is open
    /// `Enter` is *its* key, and a ladder that asked 折线 first would still be right (the
    /// two gestures cannot both be pending, `select_tool` clears them) but would be a
    /// ladder whose order nobody could defend.
    pub fn finish_ink(&mut self) -> bool {
        if self.ink.finish_typing() {
            return true;
        }
        if self.ink.finish_polyline() {
            return true;
        }
        self.ink.finish_zoom()
    }

    /// The live grip as a stable number, so the cursor shape and the hit test cannot
    /// drift apart: 0 = nothing held, 1 = drawing a new rectangle, 2..9 in
    /// `Handle::all()` order (Nw, N, Ne, E, Se, S, Sw, W), 10 = the whole rectangle.
    ///
    /// A stroke being dragged answers `1` as well, because that is the number QML
    /// turns into a crosshair - and a pen *is* drawing at the pointer, whatever the
    /// selection is doing underneath.
    pub fn grip_code(&self) -> i32 {
        if self.ink.dragging() {
            return 1;
        }
        let Some(g) = self.grab.as_ref() else {
            return 0;
        };
        Self::code_of(g.handle)
    }

    /// Which grip a point *would* take, without taking it. QML asks this on hover,
    /// for the cursor shape, and it has to be the same arithmetic `press` runs -
    /// a pointer that lies about what it is about to grab is how a resize quietly
    /// becomes a re-draw.
    pub fn hit_test(&self, index: usize, x: f64, y: f64) -> i32 {
        if self.ink.tool().is_some() {
            return 1;
        }
        let Some(p) = self.desk_point(index, x, y) else {
            return 0;
        };
        if self.hole.is_empty() {
            return Self::code_of(None);
        }
        Self::code_of(handle_at(&self.hole, p, self.hit_slop(index)))
    }

    fn code_of(handle: Option<Handle>) -> i32 {
        match handle {
            None => 1,
            Some(Handle::Body) => 10,
            Some(h) => 2 + Handle::all().iter().position(|x| *x == h).unwrap_or(0) as i32,
        }
    }

    pub fn slot(&self, index: usize) -> Option<&Slot> {
        self.slots.iter().find(|s| s.index == index)
    }

    /// The hole as this window sees it.
    fn hole_for(&self, slot: &Slot) -> PhysRect {
        self.local_dip(slot, &self.hole)
    }

    /// A desktop rect as one window sees it: clipped to the screen, moved to the
    /// screen's origin, divided by its scale.
    ///
    /// The one place that turns a desktop number into a number a mask window can place
    /// an item at. The selection goes through it and so does §5.7.11's text box, because
    /// a second copy of those three steps is a second answer, and the two of them
    /// rounding differently is a field that sits beside its own box.
    fn local_dip(&self, slot: &Slot, desk: &PhysRect) -> PhysRect {
        let Some(local) = desk.intersection(&slot.bounds) else {
            return PhysRect::default();
        };
        PhysRect::new(
            slot.scale.phys_to_dip_i(local.x - slot.bounds.x),
            slot.scale.phys_to_dip_i(local.y - slot.bounds.y),
            slot.scale.phys_to_dip_i(local.w as i32) as u32,
            slot.scale.phys_to_dip_i(local.h as i32) as u32,
        )
    }

    pub fn view_data(&self, index: usize) -> MaskViewData {
        let Some(slot) = self.slot(index) else {
            return MaskViewData::default();
        };
        let typing = self
            .ink
            .typing_rect()
            // The field belongs to the window whose click opened it, and only one window
            // can hold a focus: a box that straddles a screen seam shows its clipped
            // piece on both screens, but the caret goes to the one that was clicked.
            .filter(|r| slot.bounds.contains(PhysPoint::new(r.x, r.y)))
            .map(|r| self.local_dip(slot, &r));
        let typing_live = typing.is_some();
        MaskViewData {
            live: true,
            shown: self.shown,
            geom: slot.geom,
            hole: self.hole_for(slot),
            scale: slot.scale,
            typing: typing.unwrap_or_default(),
            typing_live,
            font_dip: slot.scale.phys_to_dip_i(self.ink.font_px() as i32).max(1) as u32,
            revision: slot.revision,
            swaps: slot.swaps,
            first_swap_ms: slot.first_swap_ms,
            shader_status: slot.shader_status,
        }
    }

    /// One presented frame. Only the first is timed, and the clock starts at
    /// `open`, so this is the number the pre-warm work is measured against.
    pub fn note_swap(&mut self, index: usize) {
        let elapsed = self.started.map(|t| t.elapsed().as_millis() as u64);
        if let Some(slot) = self.slot_mut(index) {
            slot.swaps += 1;
            if slot.first_swap_ms.is_none() {
                slot.first_swap_ms = elapsed;
                // The process's very first presented frame, whenever it lands. With two
                // screens this is the earlier of the two, which is what a user sees; the
                // per-slot `first_swap_ms` beside it stays exact per window.
                crate::state::stamp("swap");
            }
        }
    }

    pub fn note_shader_status(&mut self, index: usize, status: i32) {
        if let Some(slot) = self.slot_mut(index) {
            slot.shader_status = Some(status);
        }
    }

    fn slot_mut(&mut self, index: usize) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|s| s.index == index)
    }

    /// The desktop the hole is clipped against, as the snapshot described it.
    pub fn desktop(&self) -> PhysRect {
        self.virtual_bounds
    }

    /// One line per slot, for `--mask` to print and assert against.
    pub fn line(&self) -> String {
        let parts: Vec<String> = self
            .slots
            .iter()
            .map(|s| {
                format!(
                    "{} dpr={:.2} dip={}x{}@{},{} hole={:?} swaps={} first={:?} shader={:?}",
                    s.name,
                    s.scale.ratio(),
                    s.geom.w,
                    s.geom.h,
                    s.geom.x,
                    s.geom.y,
                    self.hole_for(s),
                    s.swaps,
                    s.first_swap_ms,
                    s.shader_status
                )
            })
            .collect();
        parts.join(" | ")
    }

    /// The annotation layer's own line, for `--ink` and for the toolbar's tooltip:
    /// which tool holds the pointer, how many objects are in the document, how many
    /// nodes a polyline has been clicked but not finished with, what copy 放大 is
    /// holding before its second drag commits it, what 文本 box is waiting for its
    /// click-away, and what the last repaint cost in pixels and milliseconds. "增量栅格"
    /// is only a claim once the number of pixels it moved is next to it.
    pub fn ink_line(&self) -> String {
        format!(
            "tool={:?} hole={:?} poly={} text={} zoom={} {}",
            self.ink.tool(),
            self.hole,
            self.ink.polyline_nodes(),
            self.ink.typing_line(),
            self.ink.zoom_line(),
            self.ink.paint_line()
        )
    }
}

// ------------------------------------------------------------------- session

static MASK: OnceLock<Mutex<MaskState>> = OnceLock::new();

fn store() -> &'static Mutex<MaskState> {
    MASK.get_or_init(|| Mutex::new(MaskState::default()))
}

pub fn with<R>(f: impl FnOnce(&mut MaskState) -> R) -> R {
    let mut guard = store().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// A mask that opened: the frame it is showing, what freezing it cost, and how
/// many screens it covers.
pub struct Opened {
    pub snap: ScreenSnapshot,
    pub ms: u64,
    pub slots: usize,
}

/// The flow the hotkey will run: freeze, then cover every screen with its frame.
/// Returns the snapshot alongside the slot count, because the `--mask` report has
/// to compare what the mask shows against the same pixels the crop will take.
pub fn open_from_freeze(shader: bool) -> Result<Opened, String> {
    let frozen = capture::freeze()?;
    let slots = with(|m| m.open(&frozen.snap, shader))?;
    Ok(Opened {
        snap: frozen.snap,
        ms: frozen.ms,
        slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask_view::shim;
    use falcon_core::capture::{FrozenMonitor, MonitorInfo};
    use falcon_core::frame::Frame;

    fn monitor(
        id: &str,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        scale: f64,
        primary: bool,
    ) -> FrozenMonitor {
        FrozenMonitor {
            info: MonitorInfo {
                id: id.into(),
                name: id.into(),
                friendly: id.into(),
                bounds: PhysRect::new(x, y, w, h),
                scale: Scale::of(scale),
                primary,
                builtin: false,
            },
            frame: Frame::new(w, h).expect("frame"),
        }
    }

    fn snapshot(monitors: Vec<FrozenMonitor>) -> ScreenSnapshot {
        let virtual_bounds = monitors.iter().fold(PhysRect::default(), |acc, m| {
            if acc.is_empty() {
                m.info.bounds
            } else {
                acc.union(&m.info.bounds)
            }
        });
        ScreenSnapshot {
            monitors,
            virtual_bounds,
            ..Default::default()
        }
    }

    fn state_with(slots: Vec<Slot>, hole: PhysRect) -> MaskState {
        MaskState {
            slots,
            hole,
            ..Default::default()
        }
    }

    fn slot(index: usize, bounds: PhysRect, scale: f64) -> Slot {
        Slot {
            index,
            name: format!("m{index}"),
            key: format!("m{index}-1"),
            bounds,
            geom: PhysRect::new(
                bounds.x / scale as i32,
                bounds.y / scale as i32,
                (bounds.w as f64 / scale).round() as u32,
                (bounds.h as f64 / scale).round() as u32,
            ),
            scale: Scale::of(scale),
            revision: 1,
            swaps: 0,
            first_swap_ms: None,
            shader_status: None,
        }
    }

    /// The conversion a stretched-or-mis-cropped mask breaks: the hole arrives in
    /// desktop pixels and has to leave as this window's own, divided by this
    /// window's scale.
    #[test]
    fn a_hole_becomes_window_local_pixels() {
        let bounds = PhysRect::new(0, 0, 3072, 1920);
        let m = state_with(
            vec![slot(0, bounds, 2.0)],
            PhysRect::new(768, 480, 1536, 960),
        );
        assert_eq!(
            m.hole_for(m.slot(0).unwrap()),
            PhysRect::new(384, 240, 768, 480)
        );
    }

    /// Two screens, a hole on one of them: the other draws *nothing*, not a band
    /// at its edge - which is what an unclipped hole looks like on a second
    /// monitor to the right.
    #[test]
    fn a_hole_only_reaches_the_screen_it_is_on() {
        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 1.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let m = state_with(
            vec![left.clone(), right.clone()],
            PhysRect::new(2100, 100, 400, 300),
        );
        assert_eq!(m.hole_for(m.slot(0).unwrap()), PhysRect::default());
        assert_eq!(m.hole_for(&right), PhysRect::new(180, 100, 400, 300));
    }

    /// The seam case, and the reason the clip happens before the division: half
    /// of the selection is on each screen, and each window gets its own half.
    #[test]
    fn a_hole_across_the_seam_is_split_not_stretched() {
        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 1.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let m = state_with(
            vec![left.clone(), right.clone()],
            PhysRect::new(1900, 10, 40, 100),
        );
        assert_eq!(m.hole_for(&left), PhysRect::new(1900, 10, 20, 100));
        assert_eq!(m.hole_for(&right), PhysRect::new(0, 10, 20, 100));
    }

    /// `--mask` asserts against this shape, so it is decided here: the middle half
    /// of the *primary*, never of the whole desktop - a two-monitor desktop whose
    /// primary is the right-hand one must not open a hole over the other screen.
    #[test]
    fn the_default_hole_is_the_middle_half_of_the_primary() {
        let snap = snapshot(vec![
            monitor("m0", 0, 0, 1920, 1080, 1.0, false),
            monitor("m1", 1920, 0, 1920, 1080, 1.0, true),
        ]);
        assert_eq!(
            MaskState::default_hole(&snap),
            PhysRect::new(2400, 270, 960, 540)
        );
    }

    /// The pairing has to survive mixed scale, because that is the only reason a
    /// mask window and its frame could disagree about size: Qt answers in DIP, the
    /// snapshot in device pixels, and neither names the other's monitor.
    #[test]
    fn a_screen_is_matched_by_its_physical_size() {
        let snap = snapshot(vec![
            monitor("m0", 0, 0, 3072, 1920, 2.0, true),
            monitor("m1", 3072, 0, 1920, 1080, 1.0, false),
        ]);
        let screen = shim::Screen {
            name: "DISPLAY1".into(),
            x: 1536,
            y: 0,
            w: 1920,
            h: 1080,
            dpr: 1.0,
        };
        let mut used = vec![false; snap.monitors.len()];
        assert_eq!(
            MaskState::match_monitor(&snap, &screen, &mut used, 1),
            Some(1)
        );
        let mut used = vec![false; snap.monitors.len()];
        let hidpi = shim::Screen {
            name: "DISPLAY2".into(),
            x: 0,
            y: 0,
            w: 1536,
            h: 960,
            dpr: 2.0,
        };
        assert_eq!(
            MaskState::match_monitor(&snap, &hidpi, &mut used, 0),
            Some(0)
        );
        // A screen the capture path never saw stays unmatched, and the caller turns
        // that into a problem row rather than a mask covering the wrong pixels.
        let mut used = vec![false; snap.monitors.len()];
        let alien = shim::Screen {
            name: "DISPLAY3".into(),
            x: 0,
            y: 0,
            w: 800,
            h: 600,
            dpr: 1.0,
        };
        assert_eq!(MaskState::match_monitor(&snap, &alien, &mut used, 2), None);
    }

    // ------------------------------------------------------------ selection

    /// The same state a drag runs against, with the one field `state_with` leaves
    /// empty: `desktop()` reads `virtual_bounds`, and an empty desktop would clamp
    /// every point to the origin and make each of these tests pass for the wrong
    /// reason.
    fn selectable(slots: Vec<Slot>, hole: PhysRect, desktop: PhysRect) -> MaskState {
        MaskState {
            slots,
            hole,
            virtual_bounds: desktop,
            ..Default::default()
        }
    }

    /// One 3072x1920 screen at 200%, which is the machine this runs on: a press at
    /// DIP `(100, 50)` is physical `(200, 100)`, so every expectation below states
    /// the scale conversion once instead of trusting a helper to hide it.
    fn one_screen() -> (Vec<Slot>, PhysRect) {
        let bounds = PhysRect::new(0, 0, 3072, 1920);
        (vec![slot(0, bounds, 2.0)], bounds)
    }

    /// §5.3.1 steps 2-3: hold, move, release, and the rectangle is there - in
    /// physical pixels, which is the space the crop is taken in afterwards.
    #[test]
    fn a_drag_from_nothing_draws_a_selection() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert_eq!(m.press(0, 100.0, 50.0), None);
        assert_eq!(m.grip_code(), 1, "the grip is a new rectangle");
        assert_eq!(m.hole, PhysRect::new(200, 100, 0, 0));
        m.drag(0, 300.0, 200.0);
        assert_eq!(m.hole, PhysRect::new(200, 100, 400, 300));
        m.release();
        assert!(m.has_selection());
        assert_eq!(m.grip_code(), 0, "nothing is held after the release");
    }

    /// The same drag in the other direction, because "from the anchor" is only
    /// honest if up-and-left works without producing a negative size.
    #[test]
    fn a_drag_up_and_left_normalises_instead_of_going_negative() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        m.press(0, 300.0, 200.0);
        m.drag(0, 100.0, 50.0);
        assert_eq!(m.hole, PhysRect::new(200, 100, 400, 300));
    }

    /// §5.3.1 step 4: a corner drags the two edges it owns and leaves the opposite
    /// corner exactly where it was.
    #[test]
    fn a_corner_drag_resizes_and_keeps_the_opposite_corner() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        // The bottom-right corner is physical (1200, 1000) = DIP (600, 500).
        assert!(matches!(m.press(0, 600.0, 500.0), Some(Handle::Se)));
        m.drag(0, 700.0, 600.0);
        assert_eq!(m.hole, PhysRect::new(400, 400, 1000, 800));
        assert_eq!(m.hole.top_left(), PhysPoint::new(400, 400));
    }

    /// The same corner pushed past the edge of the screen. §5.3.9's rule is that a
    /// selection may not leave the valid area, and the crop would otherwise ask the
    /// frozen frame for pixels that do not exist.
    #[test]
    fn a_drag_off_the_screen_stops_at_its_edge() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        m.press(0, 600.0, 500.0);
        m.drag(0, 2000.0, 1200.0);
        assert_eq!(m.hole, PhysRect::new(400, 400, 2672, 1520));
        assert_eq!(m.hole.right(), 3072);
        assert_eq!(m.hole.bottom(), 1920);
    }

    /// Grabbing the middle moves the selection by the pointer's travel. It must not
    /// put the rectangle's origin under the cursor: that jump is what `resize`'s own
    /// `Body` branch does, and it is right for a tool you place with a click and
    /// wrong for something you already have hold of.
    #[test]
    fn a_drag_inside_moves_without_jumping() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        assert!(matches!(m.press(0, 300.0, 300.0), Some(Handle::Body)));
        m.drag(0, 350.0, 320.0);
        assert_eq!(m.hole, PhysRect::new(500, 440, 800, 600));
    }

    /// The grab radius is a distance a finger travels, so it is stated in DIP and
    /// this screen's scale turns it into pixels: at 200% a press 8 physical pixels
    /// off the edge is 4 DIP off it, inside `DEFAULT_HIT_SLOP`, and grabs the edge
    /// instead of starting a new rectangle on top of the one already there.
    #[test]
    fn the_hit_test_reaches_five_dip_in_whatever_units_the_screen_uses() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(200, 100, 800, 600), desk);
        assert!(matches!(m.press(0, 96.0, 100.0), Some(Handle::W)));
    }

    /// §5.3.10: a drag that leaves its own monitor keeps extending the selection,
    /// so the clamp is against the desktop rather than the window. Two screens at
    /// 100%, so the arithmetic is the seam itself and not a scale factor.
    #[test]
    fn a_drag_can_cross_the_seam_between_two_screens() {
        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 1.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let mut m = selectable(
            vec![left, right],
            PhysRect::default(),
            PhysRect::new(0, 0, 3840, 1080),
        );
        m.press(0, 1900.0, 100.0);
        m.drag(0, 2100.0, 300.0);
        assert_eq!(m.hole, PhysRect::new(1900, 100, 200, 200));
        // And each window still sees only its own half of it.
        assert_eq!(m.view_data(0).hole, PhysRect::new(1900, 100, 20, 200));
        assert_eq!(m.view_data(1).hole, PhysRect::new(0, 100, 180, 200));
    }

    /// The Esc ladder, as a return value rather than a property: two mask windows
    /// can both receive one keystroke, and if each read "is there a selection"
    /// instead of being told, the first would clear it and the second would cancel
    /// the capture in the same press.
    #[test]
    fn escape_clears_the_selection_before_it_asks_to_cancel() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(200, 100, 800, 600), desk);
        assert!(m.escape());
        assert!(!m.has_selection());
        assert!(!m.escape(), "the second Esc is the one that ends the flow");
    }

    /// §5.7.11's box as two windows see it. The judgement is `typing_live` and not
    /// `typing`: a clipped rect on both screens is two text fields, and two fields
    /// competing for the keyboard is a caret that moves when the other one is typed
    /// into. The selection already splits at the seam; the field is the one thing in
    /// this picture that must not.
    #[test]
    fn a_text_box_belongs_to_the_window_that_was_clicked() {
        use falcon_core::annotation::model::Kind;

        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 1.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let mut m = selectable(
            vec![left, right],
            PhysRect::default(),
            PhysRect::new(0, 0, 3840, 1080),
        );
        // Through `set_hole`, not the struct literal: the layer keeps its own copy of
        // the selection to clamp gestures into, and this harness has to keep them
        // together the way `open` does.
        m.set_hole(PhysRect::new(1900, 10, 40, 100));
        m.ink.select_tool(annotate::code_of(Some(Kind::Text)));
        assert!(
            m.press(0, 1910.0, 20.0).is_none(),
            "the pen did not take the press"
        );
        m.release();
        m.ink.type_text("a string long enough to run over the seam");

        let desk = m.ink.typing_rect().expect("the click opened no box");
        assert!(desk.right() > 1920, "the box does not straddle: {desk:?}");
        let on_left = m.view_data(0);
        let on_right = m.view_data(1);
        assert!(on_left.typing_live, "the clicked window has no field");
        assert!(!on_right.typing_live, "the other window was handed one too");
        // The clicked window still shows only its own piece of the box, because the
        // field is a window's child and cannot reach past its edge.
        assert_eq!((on_left.typing.x, on_left.typing.y), (1910, 20));
        assert_eq!(
            on_left.typing.w, 10,
            "the box was not clipped to the screen"
        );
        assert_eq!(
            on_right.typing,
            PhysRect::default(),
            "the other window drew a box"
        );
    }

    /// The same box on the 200% screen this runs on: the field QML places lives in
    /// device-independent pixels, and so does the 字号 it has to match. `--mask`
    /// measures the conversion for the selection; this is the same three steps for the
    /// one item that arrives at a *click* rather than from a drag.
    #[test]
    fn a_text_box_is_placed_and_sized_in_the_windows_own_units() {
        use falcon_core::annotation::model::Kind;

        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        m.set_hole(PhysRect::new(200, 100, 800, 600));
        m.ink.select_tool(annotate::code_of(Some(Kind::Text)));
        // DIP (150,70) is physical (300,140) on this screen.
        m.press(0, 150.0, 70.0);
        m.release();
        m.ink.type_text("hi");
        let box_ = m.ink.typing_rect().unwrap();
        assert_eq!(
            (box_.x, box_.y),
            (300, 140),
            "the box is not in device pixels"
        );

        let v = m.view_data(0);
        assert!(v.typing_live);
        assert_eq!(
            (v.typing.x, v.typing.y),
            (150, 70),
            "the field is not at the click"
        );
        assert_eq!(v.font_dip, 9, "the 字号 did not divide by the scale");
        // `phys_to_dip_i` rounds, so the doubled number can be one off the measure.
        assert!(
            (v.typing.w as i32 * 2 - box_.w as i32).abs() <= 1,
            "the field is {} DIP wide for a {} px box",
            v.typing.w,
            box_.w
        );

        // And the box is the ladder's first rung: the typed string is in no history, so
        // a first Esc that un-selected instead would take the letters with it and leave
        // the selection standing.
        assert!(m.escape(), "the first Esc did not close the box");
        assert!(!m.view_data(0).typing_live, "the box survived Esc");
        assert_eq!(
            m.hole,
            PhysRect::new(200, 100, 800, 600),
            "the first Esc moved the selection"
        );
        assert!(m.escape(), "the second Esc did not clear the selection");
        assert!(!m.has_selection());
        assert!(!m.escape(), "the third Esc is the one that ends the flow");
    }

    /// Enter with nothing drawn is not a crop the user never drew.
    #[test]
    fn commit_only_returns_a_rect_that_exists() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert_eq!(m.commit(), None);
        m.set_hole(PhysRect::new(10, 20, 30, 40));
        assert_eq!(m.commit(), Some(PhysRect::new(10, 20, 30, 40)));
    }

    /// The two rungs of 复制 that answer *before* the clipboard is touched, and the
    /// sentences that say which one answered.
    ///
    /// This is the headless half of §9.1 第十条: the gauge's own copy row walks the
    /// real clipboard and then puts the user's old contents back, so the refusals
    /// underneath the write - no selection, no picture - are branches it can only
    /// reach by accident. Here they are the whole test, and neither of them may open
    /// the clipboard: a test that wrote to the user's clipboard to prove it can
    /// decline would be a bad trade.
    #[test]
    fn copying_answers_with_a_sentence_before_it_opens_the_clipboard() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(!m.copy_out());
        assert!(
            m.message.contains("还没有选区"),
            "an empty hole said {:?}",
            m.message
        );

        // A hole the user drew, on a layer that was never given frames: the answer is
        // a different sentence, which is what tells the gauge's row - and a user -
        // that this is the stitching, not the selection, that came up empty.
        m.set_hole(PhysRect::new(10, 20, 30, 40));
        assert!(!m.copy_out());
        assert!(
            m.message.contains("拼不出来"),
            "a hole with no frames said {:?}",
            m.message
        );
    }

    /// A directory under the OS temp dir, emptied first: a file the previous run of
    /// this test left behind would grade this one's write without being written by it.
    fn sandbox(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-save-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    /// A mask state whose layer can really stitch a frame. [`annotate::Layer::begin`]
    /// is the only way a canvas exists without Qt, and nothing in it reaches the image
    /// provider. The freeze is `Frame::new` - all zero, so every pixel of the selection
    /// is alpha 0, which is the one condition the transparency prompt is about and does
    /// not need an eraser, a pen or a screen to arrange.
    fn savable(hole: PhysRect) -> MaskState {
        let bounds = PhysRect::new(0, 0, 640, 480);
        let snap = snapshot(vec![monitor("m0", 0, 0, 640, 480, 1.0, true)]);
        let mut m = state_with(vec![slot(0, bounds, 1.0)], PhysRect::default());
        m.ink.begin(&snap, &m.slots);
        m.virtual_bounds = bounds;
        m.set_hole(hole);
        m
    }

    /// The rungs of 另存为 that answer before a byte is written, and the refusal that
    /// does not leave a file behind.
    ///
    /// The headless half of §9.1 第十条 again: the gauge's save legs walk a real file on
    /// a real desktop, so "nothing selected", "nothing stitched" and "a name this app
    /// cannot write" are branches it reaches only by accident. Here they are the whole
    /// test. The `!exists()` assertions are what makes it worth having - a refusal that
    /// wrote an empty or half file would still return `false` and still read as a
    /// refusal, while leaving the user a picture that is not a picture.
    #[test]
    fn saving_refuses_without_writing_anything() {
        let dir = sandbox("refuse");
        let png = dir.join("selection.png");

        let mut m = state_with(
            vec![slot(0, PhysRect::new(0, 0, 640, 480), 1.0)],
            PhysRect::default(),
        );
        assert!(!m.save_out(&png.to_string_lossy()));
        assert!(
            m.message.contains("还没有选区"),
            "an empty hole said {:?}",
            m.message
        );
        assert!(!png.exists(), "a refusal wrote a file");

        // A hole the user drew, on a layer that was never given frames.
        m.hole = PhysRect::new(10, 20, 100, 80);
        assert!(!m.save_out(&png.to_string_lossy()));
        assert!(
            m.message.contains("拼不出来"),
            "a hole with no frames said {:?}",
            m.message
        );
        assert!(!png.exists(), "the second refusal wrote a file");

        // A name that is not a format, on a state that could have written it: refused
        // after the frame exists, which is the order that lets the sentence name the
        // formats rather than the state.
        let mut m = savable(PhysRect::new(10, 20, 100, 80));
        let odd = dir.join("selection.qqq");
        assert!(!m.save_out(&odd.to_string_lossy()));
        assert!(
            m.message.contains("不是本程序会写的格式"),
            "an unknown extension said {:?}",
            m.message
        );
        assert!(!odd.exists(), "the refused name appeared on disk");
        assert!(
            !dir.join("selection.qqq.tmp").exists(),
            "a tmp file was left"
        );
    }

    /// PRD 506 ("包含透明背景时，应推荐 PNG") and 447 ("不支持透明度的输出格式应提醒用户
    /// 背景将被填充"), read against the same selection: PNG keeps what the document has,
    /// JPG does not - and says so, with the number of pixels it threw away.
    #[test]
    fn saving_says_what_a_format_without_alpha_threw_away() {
        let dir = sandbox("transparency");
        let hole = PhysRect::new(10, 20, 100, 80);
        let mut m = savable(hole);
        let shipped = m
            .ink
            .deliver()
            .expect("a hole inside one canvas has a picture");
        let total = (hole.w * hole.h) as usize;

        let png = dir.join("selection.png");
        assert!(m.save_out(&png.to_string_lossy()), "{:?}", m.message);
        assert!(m.message.contains("已保存到"), "{:?}", m.message);
        assert!(
            !m.message.contains("透明"),
            "PNG was told it lost transparency: {:?}",
            m.message
        );
        let back = encode::decode(&std::fs::read(&png).expect("the png is there")).expect("png");
        assert_eq!(back.width, shipped.width);
        assert_eq!(back.height, shipped.height);
        assert_eq!(
            back.pixels, shipped.pixels,
            "PNG did not keep the alpha the document had"
        );

        let jpg = dir.join("selection.jpg");
        assert!(m.save_out(&jpg.to_string_lossy()), "{:?}", m.message);
        assert!(
            m.message.contains(&format!("{total} px 透明")),
            "the prompt did not name the size of what it filled: {:?}",
            m.message
        );
        assert!(m.message.contains("填成白色"), "{:?}", m.message);
        assert!(m.message.contains("PNG"), "{:?}", m.message);
        let back = encode::decode(&std::fs::read(&jpg).expect("the jpg is there")).expect("jpg");
        assert!(
            back.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
            "JPG kept an alpha channel, so the sentence lied"
        );
    }

    /// §5.5.5's dialog is showing a format while the user types a bare name: the
    /// configured format wins, and the file that appears is the one the sentence names.
    ///
    /// BMP is the format under test, not JPG, because the prompt must key off "this
    /// format has no alpha" rather than off the one format the PRD happens to name.
    #[test]
    fn a_bare_name_becomes_the_configured_format() {
        let dir = sandbox("bare");
        let mut m = savable(PhysRect::new(10, 20, 100, 80));
        m.export.format = encode::Format::Bmp;
        let bare = dir.join("selection");
        assert!(m.save_out(&bare.to_string_lossy()), "{:?}", m.message);
        let written = dir.join("selection.bmp");
        assert!(written.exists(), "the bare name did not become {written:?}");
        assert!(
            !bare.exists(),
            "the name without an extension was written too"
        );
        assert!(
            m.message.contains("selection.bmp"),
            "the sentence named {:?}",
            m.message
        );
        assert!(m.message.contains("透明"), "{:?}", m.message);
    }

    /// §5.3.9: arrows, with the step and the modifier this project chose (the PRD
    /// fixes only "finer or larger steps" and "never leave the valid area").
    #[test]
    fn arrows_move_and_a_modifier_resizes() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        m.nudge_by(1, 0, false);
        assert_eq!(m.hole, PhysRect::new(401, 400, 800, 600));
        m.nudge_by(0, -10, true);
        assert_eq!(m.hole, PhysRect::new(401, 400, 800, 590));
        // Pinned to the right edge, a further push right cannot take it out.
        m.set_hole(PhysRect::new(2272, 100, 800, 600));
        m.nudge_by(50, 0, false);
        assert_eq!(m.hole.right(), 3072);
    }

    /// A grip code is what QML turns into a cursor, so the numbering is an
    /// interface: eight edges and corners, the body, a new rectangle, nothing.
    #[test]
    fn the_grip_codes_name_every_handle_once() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        assert_eq!(m.grip_code(), 0);
        // The selection is physical `(400,400,800,600)`; at 200% the pointer lives
        // in `(200,200)-(600,500)`, so every coordinate below is the rect's edge
        // divided by the scale, not the rect's edge.
        let mut codes = Vec::new();
        for (x, y) in [
            (200.0, 200.0), // Nw
            (400.0, 200.0), // N
            (600.0, 200.0), // Ne
            (600.0, 350.0), // E
            (600.0, 500.0), // Se
            (400.0, 500.0), // S
            (200.0, 500.0), // Sw
            (200.0, 350.0), // W
        ] {
            m.press(0, x, y);
            codes.push(m.grip_code());
            m.release();
        }
        assert_eq!(codes, vec![2, 3, 4, 5, 6, 7, 8, 9]);
        assert!(!codes.contains(&0) && !codes.contains(&1));
        m.press(0, 400.0, 350.0);
        assert_eq!(m.grip_code(), 10, "the body");
        m.release();
        m.press(0, 1400.0, 900.0);
        assert_eq!(m.grip_code(), 1, "a new rectangle");
    }

    /// The cursor is drawn from a hover, so the hover answer has to be the press
    /// answer - and it must reach that conclusion without moving anything.
    #[test]
    fn the_pointer_is_asked_before_it_presses() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        let before = m.hole;
        assert_eq!(m.hit_test(0, 200.0, 200.0), 2, "the top-left corner");
        assert_eq!(m.hit_test(0, 400.0, 350.0), 10, "the body");
        assert_eq!(m.hit_test(0, 1400.0, 900.0), 1, "a fresh rectangle");
        assert_eq!(
            m.hit_test(0, 200.0, 205.0),
            2,
            "still the corner, 5 DIP down"
        );
        assert_eq!(m.hole, before, "and nothing moved");

        // With the grip named, the hover and the press must agree everywhere.
        for (x, y) in [
            (200.0, 200.0),
            (400.0, 200.0),
            (600.0, 200.0),
            (600.0, 350.0),
            (600.0, 500.0),
            (400.0, 500.0),
            (200.0, 500.0),
            (200.0, 350.0),
            (1400.0, 900.0),
        ] {
            let hover = m.hit_test(0, x, y);
            m.press(0, x, y);
            assert_eq!(m.grip_code(), hover, "hover and press at ({x}, {y})");
            m.release();
        }

        // No selection yet: every point of the screen is "start drawing", which is
        // what keeps the cursor a crosshair instead of an arrow on a fresh mask.
        m.set_hole(PhysRect::default());
        assert_eq!(m.hit_test(0, 700.0, 400.0), 1);
    }

    /// Two captures in one process must not share a frame key.
    ///
    /// The failure this guards is invisible on screen: `Image.cache: false` only
    /// re-requests when the URL changes, so a repeated key means the second freeze
    /// is stored under a name Qt has already cached a picture for, and the overlay
    /// reports a new frame while showing the old one.
    #[test]
    fn a_second_capture_asks_for_a_key_nobody_has_used() {
        let mut m = MaskState::default();
        let first = m.take_revision();
        let second = m.take_revision();
        assert_eq!(first, 1, "revisions are one-based, 0 is no frame at all");
        assert_ne!(first, second, "and never repeated");
        assert_ne!(
            MaskState::frame_key("m0-display1", first),
            MaskState::frame_key("m0-display1", second),
            "so the provider URL changes with the capture"
        );
    }
}
