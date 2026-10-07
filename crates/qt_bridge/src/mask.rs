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
use falcon_core::geometry::{PhysRect, Scale};

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
    pub revision: u32,
    pub swaps: u32,
    pub first_swap_ms: Option<u64>,
    pub shader_status: Option<i32>,
}

#[derive(Default)]
pub struct MaskState {
    pub slots: Vec<Slot>,
    /// The selection in physical desktop pixels.
    pub hole: PhysRect,
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

        let started = Instant::now();
        let screens = shim::screens();
        if screens.is_empty() {
            return Err("Qt 没有报告任何屏幕".into());
        }

        let mut used = vec![false; snap.monitors.len()];
        let mut slots = Vec::new();
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
            let key = format!("{}-1", info.id);
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
                revision: 1,
                swaps: 0,
                first_swap_ms: None,
                shader_status: None,
            });
        }
        if slots.is_empty() {
            return Err("Qt 的屏幕和快照的显示器配不上对".into());
        }

        self.slots = slots;
        self.virtual_bounds = snap.virtual_bounds;
        self.shown = true;
        self.pixels_ms = Some(started.elapsed().as_millis() as u64);
        self.started = Some(Instant::now());
        Ok(self.slots.len())
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
        self.shown = false;
    }

    /// The selection, in physical desktop pixels. Each slot re-reads its part of
    /// it on the next `reload`; making every window redraw without waiting for its
    /// own reload is the M3 selection flow's, not this one's.
    pub fn set_hole(&mut self, hole: PhysRect) {
        self.hole = hole;
    }

    pub fn set_shader(&mut self, shader: bool) {
        self.shader = shader;
    }

    pub fn slot(&self, index: usize) -> Option<&Slot> {
        self.slots.iter().find(|s| s.index == index)
    }

    /// The hole as this window sees it: clipped to the screen, moved to the
    /// screen's origin, divided by its scale.
    fn hole_for(&self, slot: &Slot) -> PhysRect {
        let Some(local) = self.hole.intersection(&slot.bounds) else {
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
        MaskViewData {
            live: true,
            shown: self.shown,
            geom: slot.geom,
            hole: self.hole_for(slot),
            scale: slot.scale,
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
}
