//! The ink, checked against the pixels it is supposed to be covering.
//!
//! `--ink <ms>` is `--mask` plus a script: the mask opens on a real freeze, this
//! module drives *the same entry points a pointer uses* through seven strokes, two
//! erases and an undo, and the run ends with one `PrintWindow` read-back of each mask
//! window. That single read-back is why the strokes are laid out the way they are, and
//! why the 橡皮 rows sit outside it:
//!
//! * **stroke 1 and 2 are in different places**. If the second commit never reaches
//!   the screen - plan §3.6 constraint 8, the failure P4 measured on the spike, where
//!   a texture stored under an already-requested key simply is not re-requested - the
//!   place only stroke 2 drew stays un-inked and the row says so.
//! * **the last stroke is undone**. A repaint that only *adds* would leave that
//!   stroke's pixels behind as a ghost over the frozen desktop, which is the exact
//!   failure [`falcon_core::annotation::raster::paint`]'s reset-to-base exists to
//!   prevent. So the read-back grades its absence as loudly as the other two's
//!   presence.
//! * **the dim outside the selection is measured again**. The layer is transparent
//!   outside the hole by construction; get that wrong and the mask shows an un-dimmed
//!   copy of the desktop where the user has not selected anything - a bug the ink rows
//!   alone would pass.
//! * **one stroke is 文本, in a third ink**. Everything else the script plants is a
//!   shape it drew point by point, so its pixels are where the script put them. A text
//!   box's are not: the click is the script's and the letters are the product's, which
//!   is why the row that grades them counts an area the script declared instead of
//!   sampling a path, and prints how much of that area came back as ink.
//! * **the 橡皮 leg reads no photograph at all.** §5.7.14's two modes cannot be told
//!   apart on the screen: QML draws the frozen frame *under* the overlay, so a pixel the
//!   eraser put back onto the screenshot and a pixel it took out of the exported picture
//!   both read as the desktop. So one more line of ink is drawn and then erased twice,
//!   once with the mode off and once with it on, and those two thirds are graded out of
//!   the two bitmaps Rust owns - the incremental canvas and a whole-canvas render of the
//!   same document - against the freeze itself. The exactness half of that is a claim the
//!   `layer == export` row cannot make: it steps over every pixel whose alpha is 0, which
//!   is exactly what 擦除到透明 produces, so a canvas that dropped a pixel the export kept
//!   is invisible to it by construction. The new row grades that way round and the other
//!   one too.
//!
//! The colour tests are relational (one channel must lead the other two) rather than
//! exact, because composition and antialiasing are allowed to round a pixel; the ghost
//! test is exact against the freeze, because there the two bitmaps are the same
//! picture. Sampling positions come from [`Plan`] - the same record that produced the
//! strokes - so an expectation can never drift away from what was drawn.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};

use falcon_core::annotation::Kind;
use falcon_core::capture::ScreenSnapshot;
use falcon_core::config::FILE_NAME;
use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect};

use crate::annotate;
use crate::capture;
use crate::mask;
use crate::mask_check::{self, median, ratio_row, samples, Sides};
use crate::mask_view::shim;
use crate::r13::{parse_levels, Report, Top};
use crate::session;
use crate::state::Check;

/// The pen used by every stroke in the script. The default is 3 device pixels,
/// which is inside the antialiasing of a composited read-back; 8 is unambiguous and
/// still small against a 1536x960 selection.
const PEN: u32 = 8;

/// The two colours, chosen to be unlike each other and unlike either's background:
/// the ghost row asks for the *absence* of blue, and the ink rows for the presence
/// of green, so a stroke that landed in the wrong place fails on both counts.
const GREEN: [u8; 4] = [0, 232, 0, 255];
const BLUE: [u8; 4] = [0, 0, 232, 255];

/// How far the leading channel has to beat the other two for a pixel to be called
/// this ink. Half of the inks' own margin against a white background, so a heavily
/// antialiased edge is not counted as a hit.
const MARGIN: i32 = 60;

/// Per-channel agreement for the ghost row: two bitmaps of the same frozen desktop,
/// one through Qt's texture path and one through `PrintWindow`. Composition is
/// allowed a rounding or two; 8 is well below a visible difference and well above one.
const SAME_TOL: i32 = 8;

/// How far inside the selection 放大's copy is travelled to, in device pixels. A pen's
/// width plus antialiasing, so the samples below never sit on the copy's own frame.
const PAD: i32 = PEN as i32 + 4;

/// How far apart the samples of a stroke's band sit, in device pixels.
const BAND_STEP: i32 = 8;

/// What share of a stroke's *usable* samples have to read as expected. Antialiasing at
/// the two ends of a run, and a window whose first frame pre-dates the last publish, are
/// the honest sources of a near miss; a stroke that is absent reads far below this.
const INK_PASS: usize = 8;
const GHOST_PASS: usize = 9;

/// The fewest usable samples a band row may be graded on at all - see [`Hits::usable`].
/// Thirty is about a quarter of the smallest band this gauge plants (the freehand
/// stroke's 121 samples), and a reading taken on fewer pixels than that says more about
/// the desktop under the stroke than about the stroke. The number is a 口径 of §9.4, not
/// a tuning knob: moving it changes what a green row is evidence of.
const MIN_USABLE: usize = 30;

/// 文本's own ink: the third colour the script plants, and [`falcon_core::annotation::Style`]'s
/// default one. A third rather than a repeat of either of the other two because a row
/// that another stroke can satisfy is not a row - the green bands grade green and the
/// ghost row grades blue, so letters in either of those inks would be read by a row that
/// was not asked about them. Red is neither, and the pen the product starts with is the
/// one whose colour this row can therefore also grade.
const RED: [u8; 4] = [232, 17, 35, 255];

/// The string the sixth stroke types. ASCII on purpose: an expectation that depends on
/// which input method is loaded is not an expectation, and what this row asks - did the
/// letters the script typed reach the place the script's click chose - does not need one.
const TEXT: &str = "Falcon 2026";

/// How far inside the free band a text strip sits, in device pixels: the pen's own
/// half-width plus the antialiasing of a composited read-back, so a strip that is
/// declared clear of the other bands is clear of their *pixels* and not of their centre
/// lines.
const CLEAR: i32 = PEN as i32 / 2 + 4;

/// The strip whose ink 文本's row counts, in device pixels. A fixed size rather than a
/// fraction of the selection, because the pen's 字号 is a count of device pixels too: the
/// line this gauge types is the same height on a 960-tall selection as on a 480-tall one,
/// and a strip that shrank with the selection would stop containing it.
const TEXT_W: u32 = 240;
const TEXT_H: u32 = 40;

/// The fewest pixels of 文本 ink a strip has to show before the row calls it letters.
/// Deliberately far below any legible line at 字号 18 and above the single stray pixel a
/// seam or a neighbour's corner could leave: the row's claim is *presence*, and the count
/// it prints is the finding. A 口径 of §9.4 like [`MIN_USABLE`], not a tuning knob.
const MIN_TEXT_INK: usize = 24;

/// The diameter of 橡皮's brush when the leg erases with [`PEN`] in hand. This is
/// [`crate::annotate`]'s `link_brush` arithmetic written out here rather than trusted:
/// `width * 4` clamped to `3..=96`, and 8 * 4 sits inside the clamp, so the value is the
/// one the product will compute and not a guess at it. The leg needs the *radius* to keep
/// its samples clear of the run next door - a sample a neighbour's brush can reach is a
/// sample two rows can grade. If `link_brush` ever changes, the erase rows fail on their
/// own printed witnesses instead of passing on margins that no longer hold.
const ERASE_BRUSH: i32 = PEN as i32 * 4;

/// Half of it: how far outside an erase run's endpoints its pixels reach.
const ERASE_REACH: i32 = ERASE_BRUSH / 2;

/// The y of 橡皮's band, as a fraction of the selection: the slice between the
/// selection's own top edge and the 0.12 where 方框's top border and 折线's first point
/// begin. Why not the bottom, where the rest of the script's spare room is: the 放大 copy
/// is dragged to the selection's bottom-right corner and its size is the product's
/// (magnifier side x zoom), so no band placed in the lower half can promise not to land
/// inside a rectangle another row grades off the *screen*. 橡皮's own rows read the export
/// and would survive that overlap - the eraser's objects are planted after the copy and
/// paint over it - but the zoom row's would not, and a leg that moves its neighbour's
/// numbers is not a leg that added nothing.
const ERASE_Y: f64 = 0.06;

/// The run's x extent, as fractions of the selection, split into the three zones the
/// rows grade: a third nobody erases, a third erased with §5.7.14's mode off, a third
/// with it on.
const ERASE_X0: f64 = 0.12;
const ERASE_X1: f64 = 0.88;

/// What was planted, so the read-back can grade the same rectangles that were drawn.
struct Plan {
    /// The box's geometry, in device desktop pixels.
    rect: PhysRect,
    /// The freehand stroke's corner points.
    zig: Vec<PhysPoint>,
    /// 折线's nodes, in the order they were clicked (§5.7.5).
    poly: Vec<PhysPoint>,
    /// The arrow's two ends - the stroke that gets undone.
    arrow: [PhysPoint; 2],
    /// 放大 (§5.7.13) as Rust placed it: the source region and the copy. `None` is a
    /// gesture that never got that far, which the row says out loud rather than skipping.
    zoom: Option<(PhysRect, PhysRect)>,
    /// 文本 (§5.7.11) as the script planted it: the strip whose ink the row counts,
    /// anchored at the click. `None` is a selection too small for one line to sit in it
    /// clear of the bands the other rows grade - a refusal the row names rather than a
    /// reading it withholds. Never derived from the box the product measured: the point
    /// of the row is that the click is the script's and the letters are the product's.
    text: Option<PhysRect>,
    /// 橡皮 (§5.7.14) as the script laid it, or the reason this selection has no room
    /// for it. Both arms are read: the rows that come out of an `Ok` are the four the leg
    /// was built to print, and the `Err` says in one line why none of them is there.
    erase: Result<Erase, String>,
    /// The keys each publish produced, oldest first: four strokes and one undo.
    keys: Vec<Vec<String>>,
    /// The layer's own line at planting time, and the selection it was drawn in.
    line: String,
    hole: PhysRect,
    /// A stroke with no screen under it is a blocked row, not a silent pass.
    notes: Vec<String>,
}

static PLAN: OnceLock<Mutex<Option<Plan>>> = OnceLock::new();

fn plan() -> &'static Mutex<Option<Plan>> {
    PLAN.get_or_init(|| Mutex::new(None))
}

/// A device point to the window-local device-independent pixels `press`/`drag` take,
/// and which window that is. `None` is a point on no screen this mask covers.
fn local(slots: &[mask::Slot], at: PhysPoint) -> Option<(usize, f64, f64)> {
    let s = slots.iter().find(|s| s.bounds.contains(at))?;
    Some((
        s.index,
        (at.x - s.bounds.x) as f64 / s.scale.ratio(),
        (at.y - s.bounds.y) as f64 / s.scale.ratio(),
    ))
}

/// Press, move through every point, release - the whole of what a hand does, run
/// against [`crate::mask::MaskState`]'s own pointer entry points. Going through
/// `press`/`drag`/`release` rather than building elements is the point: this then
/// measures the *routing* - does a drawing tool take the press, does the hole stay
/// put under it - and not only the rasteriser, which has its own tests.
///
/// [`grab`] is the same motion with the button still held.
/// Press and move through every point, and *hold* the button: everything [`stroke`]
/// does before the release. 放大 needs the split, because its copy is only pending
/// until the button comes up - and the row has to grade the rectangle Rust actually
/// placed, not the one the script meant to ask for at this scale factor.
fn grab(
    m: &mut mask::MaskState,
    slots: &[mask::Slot],
    at: &[PhysPoint],
    notes: &mut Vec<String>,
) -> bool {
    let Some(first) = at.first() else {
        return false;
    };
    let Some((index, x, y)) = local(slots, *first) else {
        notes.push(format!("no screen under {:?}", first));
        return false;
    };
    m.press(index, x, y);
    for p in &at[1..] {
        // Every point of one stroke belongs to the same grab, so the index the press
        // chose is kept: a stroke that wanders onto a second screen is still dragged
        // by *this* window's pointer, and `desk_point` converts from its origin.
        if let Some((_, x, y)) = local(slots, *p) {
            m.drag(index, x, y);
        }
    }
    true
}

fn stroke(
    m: &mut mask::MaskState,
    slots: &[mask::Slot],
    at: &[PhysPoint],
    notes: &mut Vec<String>,
) {
    if grab(m, slots, at, notes) {
        m.release();
    }
}

/// The corners of a box, in the order a drag visits them: press at the top-left,
/// release at the bottom-right, with the other two corners moved through on the way.
fn box_points(rect: PhysRect) -> Vec<PhysPoint> {
    let c = rect.top_left();
    let br = PhysPoint::new(rect.right() - 1, rect.bottom() - 1);
    vec![c, PhysPoint::new(c.x, br.y), PhysPoint::new(br.x, c.y), br]
}

/// 折线 (§5.7.5) the way a hand does it: the first click places the first node, then
/// every later leg is a press-move-release that ends on *its* node - which is the
/// whole difference between this tool and the others, since none of them commits
/// anything on the release. The finisher is deliberately not here: the script calls it
/// itself, so the row can say whether the line closed or was left pending.
fn clicks(
    m: &mut mask::MaskState,
    slots: &[mask::Slot],
    nodes: &[PhysPoint],
    notes: &mut Vec<String>,
) {
    let Some(first) = nodes.first() else {
        return;
    };
    stroke(m, slots, std::slice::from_ref(first), notes);
    for w in nodes.windows(2) {
        stroke(m, slots, &along(w[0], w[1]), notes);
    }
}

/// Points along a straight run, about `BAND_STEP` device pixels apart.
fn along(a: PhysPoint, b: PhysPoint) -> Vec<PhysPoint> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let n = ((dx * dx + dy * dy) as f64).sqrt() as i32 / BAND_STEP;
    if n <= 0 {
        return vec![a, b];
    }
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            PhysPoint::new(
                (a.x as f64 + dx as f64 * t).round() as i32,
                (a.y as f64 + dy as f64 * t).round() as i32,
            )
        })
        .collect()
}

/// The band of a polyline: every segment, minus each interior point's duplicate.
/// [`rect_band`] is the same idea applied to a box's four borders.
fn path_band(points: &[PhysPoint]) -> Vec<PhysPoint> {
    let mut v = Vec::new();
    for w in points.windows(2) {
        let mut run = along(w[0], w[1]);
        if !v.is_empty() {
            run.remove(0);
        }
        v.extend(run);
    }
    v
}

/// The band of the box: its four border lines.
fn rect_band(rect: PhysRect) -> Vec<PhysPoint> {
    let (x0, y0) = (rect.x, rect.y);
    let (x1, y1) = (rect.right() - 1, rect.bottom() - 1);
    path_band(&[
        PhysPoint::new(x0, y0),
        PhysPoint::new(x1, y0),
        PhysPoint::new(x1, y1),
        PhysPoint::new(x0, y1),
        PhysPoint::new(x0, y0),
    ])
}

/// Publish what the layer has painted since the last flush, and remember the keys.
fn flush(m: &mut mask::MaskState, keys: &mut Vec<Vec<String>>) {
    keys.push(m.ink.flush());
}

/// The strip 文本's row counts: centred in the selection's one free band, the slice of
/// its height between the box's bottom border (y 0.42) and the polyline's top node
/// (y 0.50), and [`CLEAR`] inside both. Nothing else is planted there, and a strip that
/// does not fit is refused rather than allowed to paint over a stroke another row is
/// already grading - one stroke moving two rows is a reading nobody can attribute.
///
/// `None` is a selection too short or too narrow for one line of 字号 text to sit in
/// that band, which is a property of the geometry the script was handed and not of the
/// product. The row says so as a refusal; the tests give the branch a leg, because a
/// path this gauge never walks is a path this gauge has not checked.
fn text_strip(hole: PhysRect) -> Option<PhysRect> {
    let top = hole.y + (hole.h as f64 * 0.425).round() as i32 + CLEAR;
    let bottom = hole.y + (hole.h as f64 * 0.50).round() as i32 - CLEAR;
    let x = hole.x + (hole.w as f64 * 0.445).round() as i32;
    if bottom - top < TEXT_H as i32 || x + TEXT_W as i32 > hole.right() - CLEAR {
        return None;
    }
    Some(PhysRect::new(
        x,
        top + (bottom - top - TEXT_H as i32) / 2,
        TEXT_W,
        TEXT_H,
    ))
}

/// 橡皮 (§5.7.14), as the script laid it out: one line of ink across the free slice of
/// the selection, a third of it erased with the mode off, a third with it on.
struct Erase {
    /// The pencil run, end to end. Its first third is never erased, and that third is
    /// the control: without it, "the pixels are gone" is equally a reading of a stroke
    /// that never arrived.
    ink: (PhysPoint, PhysPoint),
    /// The two drags, in the order the script makes them: §5.7.14's 默认 first, then
    /// 擦除到透明. Each is a press-move-release through [`Erase::zones`]' middle.
    runs: [(PhysPoint, PhysPoint); 2],
    /// The samples the rows read, one list per zone: unerased, erased with the mode off,
    /// erased with it on. They sit on the runs' centre lines, inside them by [`CLEAR]` and
    /// outside the next run's [`ERASE_REACH`], because a sample a neighbour's brush can
    /// reach is a sample two rows can grade.
    zones: [Vec<PhysPoint>; 3],
}

/// Lay 橡皮's band into the selection, or say why the selection has no room for one.
///
/// Two refusals are possible and they are different questions, so the sentence names
/// whichever one happened: the band's height is what the selection's own top edge and the
/// 0.12 where 方框 and 折线 begin leave the brush, and its width is what the three zones
/// need once each erase run has kept [`ERASE_REACH`] out of its neighbour. A row that
/// declines is a row whose reading the desktop could not have given - which is a fact
/// about the script's geometry, not about the product, and gets said as one.
fn erase_band(hole: PhysRect) -> Result<Erase, String> {
    let need = ERASE_REACH + CLEAR;
    let y = hole.y + (hole.h as f64 * ERASE_Y).round() as i32;
    let above = hole.y;
    let below = hole.y + (hole.h as f64 * 0.12).round() as i32;
    if y - above < need || below - y < need {
        return Err(format!(
            "the slice between the selection's top edge and where 方框 and 折线 begin (y 0.12) is {} px tall here, and \
             橡皮's brush needs {need} of it on each side of y {y}",
            below - above
        ));
    }

    let left = hole.x + (hole.w as f64 * ERASE_X0).round() as i32;
    let right = hole.x + (hole.w as f64 * ERASE_X1).round() as i32;
    let third = (right - left) / 3;
    let least = 2 * need + 2 * CLEAR + BAND_STEP;
    if third < least {
        return Err(format!(
            "each third of the run is {third} px wide here, and a zone needs {least} to hold a run that keeps \
             {need} out of its neighbour and samples that sit {CLEAR} inside the run"
        ));
    }

    let (z1, z2) = (left + third, left + 2 * third);
    let row = |a: i32, b: i32| along(PhysPoint::new(a, y), PhysPoint::new(b, y));
    Ok(Erase {
        ink: (PhysPoint::new(left, y), PhysPoint::new(right, y)),
        runs: [
            (PhysPoint::new(z1 + need, y), PhysPoint::new(z2 - need, y)),
            (
                PhysPoint::new(z2 + need, y),
                PhysPoint::new(right - need, y),
            ),
        ],
        zones: [
            row(left + CLEAR, z1 - CLEAR),
            row(z1 + need + CLEAR, z2 - need - CLEAR),
            row(z2 + need + CLEAR, right - need - CLEAR),
        ],
    })
}

/// Where this gauge's writes go instead of the file it read.
///
/// Under the OS temp dir, one path for the whole gauge, and never next to the script
/// that measures it: the file this run teaches pens into is a measurement artefact, and
/// an artefact that lands in the user's `%APPDATA%` is a defect that reports itself as a
/// setting. `Config::save` creates the parent directory, so this needs no `mkdir` - and
/// nothing here reads the file back, which is why the name is fixed rather than unique.
fn gauge_config() -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-ink-gauge");
    dir.push(FILE_NAME);
    dir
}

/// Freeze, open, draw, undo. Must run before the QML engine, for the same reason
/// [`mask_check::open`] must: the windows must not capture the screen they cover.
pub fn plant(shader: bool) -> Result<String, String> {
    // §10 第 33 项 ①, paid back before the leg that owed it. This script teaches the
    // pens it is about to measure - `select_tool`, `set_color`, `set_width` below - and
    // the flow it drives ends the way the product's does, through
    // [`crate::settings::write_back`]. Without the pin, every `--ink` run leaves the
    // user's own `config.toml` holding the green 矩形 and the 8 px pen it planted to
    // grade itself, and §5.7.1 promises that file the *user's* last pen.
    //
    // Only the destination moves: the read is still the user's file, so `apply=` still
    // counts the pens that user set and `write_back`'s "did anything move?" is still
    // decided against what the user has. The row in [`measure`] grades that this stayed
    // true, because a pin someone later deletes is a bug that writes itself once per
    // run and never reports.
    let pinned = crate::settings::pin_writes(&gauge_config());
    mask_check::open(shader)?;
    let slots = mask::with(|m| m.slots.clone());
    let mut notes = vec![format!("config writes pinned to {}", pinned.display())];

    // Every region is a fraction of the selection, so the script needs no knowledge
    // of the monitor's size or of the default hole's arithmetic. The polyline's strip
    // (y 0.48..0.60) is between the box and the arrow on purpose: the three rows that
    // look for green must not be able to satisfy each other, and the arrow's band -
    // the one graded for absence - has to stay clear of ink that is still meant to be
    // there. The 放大 source patch is the one band *below* the arrow for the same
    // reason: its copy paints over anything inside it. 文本 goes in the one slice of the
    // selection none of them touches - see [`text_strip`] - and in a third ink, so it can
    // satisfy none of their rows and none of them can satisfy its own. 橡皮 goes above
    // all of them, in the slice between the selection's edge and the 0.12 the box and the
    // zig start at, for the reason just given for the copy: it needs a width of its own to
    // be worth reading, and the only full-width slice left is at the top. See
    // [`erase_band`].
    let (rect, zig, poly, arrow, zsrc, strip, erase) = mask::with(|m| {
        let h = m.hole;
        let f = |fx: f64, fy: f64| {
            PhysPoint::new(
                h.x + (h.w as f64 * fx).round() as i32,
                h.y + (h.h as f64 * fy).round() as i32,
            )
        };
        (
            PhysRect::from_points(f(0.15, 0.12), f(0.40, 0.42)),
            vec![f(0.55, 0.12), f(0.85, 0.26), f(0.55, 0.40)],
            vec![
                f(0.12, 0.50),
                f(0.32, 0.58),
                f(0.52, 0.50),
                f(0.72, 0.58),
                f(0.88, 0.50),
            ],
            [f(0.15, 0.62), f(0.85, 0.76)],
            PhysRect::from_points(f(0.12, 0.84), f(0.37, 0.94)),
            text_strip(h),
            erase_band(h),
        )
    });

    let mut keys = Vec::new();
    let mut zoom: Option<(PhysRect, PhysRect)> = None;
    mask::with(|m| {
        m.ink.select_tool(annotate::code_of(Some(Kind::Rect)));
        m.ink.set_color(GREEN);
        m.ink.set_width(PEN);
        stroke(m, &slots, &box_points(rect), &mut notes);
        flush(m, &mut keys);

        m.ink.select_tool(annotate::code_of(Some(Kind::Pencil)));
        let mut path = Vec::new();
        for w in zig.windows(2) {
            let mut run = along(w[0], w[1]);
            if !path.is_empty() {
                run.remove(0);
            }
            path.extend(run);
        }
        stroke(m, &slots, &path, &mut notes);
        flush(m, &mut keys);

        // 折线, clicked node by node and then closed by the finisher rather than by a
        // release - which is the point of the row: a polyline the script forgot to
        // finish would still paint green pixels, and only the object count says it is
        // not in the document.
        m.ink.select_tool(annotate::code_of(Some(Kind::Polyline)));
        let before = m.ink.objects();
        clicks(m, &slots, &poly, &mut notes);
        let pending = m.ink.polyline_nodes();
        if pending != poly.len() {
            notes.push(format!("clicks left {pending} node(s), not {}", poly.len()));
        }
        if !m.ink.finish_polyline() {
            notes.push("the polyline had nothing to finish".to_string());
        } else if m.ink.objects() != before + 1 {
            notes.push(format!(
                "finishing made {} object(s), not {}",
                m.ink.objects(),
                before + 1
            ));
        }
        flush(m, &mut keys);

        // §5.7.13's two drags, in the order a hand makes them: the box that says *what*
        // to magnify, then a press-move that travels the copy the first one generated.
        // The copy goes to the selection's bottom-right rather than staying where
        // `place` put it, because a magnifier paints over whatever is inside it - and an
        // arrow it covers is an arrow the ghost row then reports as ink.
        m.ink.select_tool(annotate::code_of(Some(Kind::Zoom)));
        stroke(m, &slots, &box_points(zsrc), &mut notes);
        let h = m.hole;
        if let Some((src, copy)) = m.ink.zoom_pending() {
            let target = PhysPoint::new(
                (h.right() - copy.w as i32 - PAD).max(h.x + PAD),
                (h.bottom() - copy.h as i32 - PAD).max(h.y + PAD),
            );
            let a = PhysPoint::new(src.x + 4, src.y + 4);
            let b = PhysPoint::new(a.x + (target.x - copy.x), a.y + (target.y - copy.y));
            let before = m.ink.objects();
            if grab(m, &slots, &along(a, b), &mut notes) {
                // Read it *before* the release: from that moment the placement is a
                // document object, and the row grades the rectangle Rust actually put
                // there at this scale factor rather than the one the script asked for.
                zoom = m.ink.zoom_pending();
                m.release();
                if m.ink.objects() != before + 1 {
                    notes.push(format!(
                        "放大's second drag committed {} object(s)",
                        m.ink.objects().saturating_sub(before)
                    ));
                }
            }
        } else {
            notes.push("放大's first drag left no copy to place".to_string());
        }
        flush(m, &mut keys);

        // §5.7.11's three steps, in the order a hand makes them and through the same
        // entry points the pointer does: one click places the box, the string arrives the
        // way the field sends it, and the finisher files it. The release is *not* the
        // finisher - step 5 is the next press or `Enter` - and the row that grades this
        // stroke counts the area the click chose, never the area the product measured,
        // which is what keeps a box filed in the wrong place from being graded by the
        // arithmetic that put it there.
        m.ink.select_tool(annotate::code_of(Some(Kind::Text)));
        m.ink.set_color(RED);
        let before = m.ink.objects();
        if let Some(strip) = strip {
            stroke(m, &slots, &[strip.top_left()], &mut notes);
            m.ink.type_text(TEXT);
            if !m.ink.finish_typing() {
                notes.push("文本 had nothing to finish".to_string());
            } else if m.ink.objects() != before + 1 {
                notes.push(format!(
                    "finishing 文本 made {} object(s), not 1",
                    m.ink.objects().saturating_sub(before)
                ));
            }
        } else {
            notes.push(
                "the selection is too small for a text strip that clears the other bands"
                    .to_string(),
            );
        }
        flush(m, &mut keys);

        // 橡皮 (§5.7.14): the leg that reads no photograph. What the script can promise
        // is the setup only - ink down first, then one drag per mode - because a drag
        // that commits nothing is indistinguishable, in the exported frame, from an erase
        // that erased nothing. So the object count is asked here, in the one place it can
        // be asked, and the pixels are graded in `measure`.
        match &erase {
            Ok(e) => {
                m.ink.select_tool(annotate::code_of(Some(Kind::Pencil)));
                m.ink.set_color(GREEN);
                m.ink.set_width(PEN);
                stroke(m, &slots, &along(e.ink.0, e.ink.1), &mut notes);
                flush(m, &mut keys);

                m.ink.select_tool(annotate::code_of(Some(Kind::Eraser)));
                // Both halves of the pen are read back before the runs, for the reason
                // [`crate::annotate`]'s `erase_transparent` gives: §5.7.1's per-tool
                // memory can hand 橡皮 a mode and a 线宽 no button ever touched, and the
                // band's margins are measured in that 线宽's radius. Setting them is not
                // enough - saying what the tool came up with is what keeps the next
                // round's reading attributable to this leg rather than to the file.
                if m.ink.erase_transparent() {
                    notes.push("橡皮 came up 擦除到透明, not §5.7.14's 默认".to_string());
                }
                if m.ink.width() != PEN {
                    notes.push(format!("橡皮 came up 线宽 {}, not {PEN}", m.ink.width()));
                }
                m.ink.set_width(PEN);
                for (mode, run) in [(false, &e.runs[0]), (true, &e.runs[1])] {
                    let which = if mode {
                        "擦除到透明"
                    } else {
                        "只擦标注"
                    };
                    m.ink.set_erase_transparent(mode);
                    if m.ink.erase_transparent() != mode {
                        notes.push(format!(
                            "the pen refused {which}: erase_transparent reads {}",
                            m.ink.erase_transparent()
                        ));
                    }
                    let before = m.ink.objects();
                    stroke(m, &slots, &along(run.0, run.1), &mut notes);
                    let made = m.ink.objects().saturating_sub(before);
                    if made != 1 {
                        notes.push(format!("{which} committed {made} object(s), not 1"));
                    }
                    flush(m, &mut keys);
                }
            }
            Err(why) => notes.push(format!("橡皮: {why}")),
        }

        // The stroke that is about to be undone, in the other ink.
        m.ink.select_tool(annotate::code_of(Some(Kind::Arrow)));
        m.ink.set_color(BLUE);
        stroke(m, &slots, &along(arrow[0], arrow[1]), &mut notes);
        flush(m, &mut keys);

        if !m.ink.undo_step() {
            notes.push("undo had nothing to take".to_string());
        }
        flush(m, &mut keys);
        // Back to the arrow tool, so the read-back measures the mask with its
        // selection editable rather than mid-stroke.
        m.ink.select_tool(annotate::code_of(None));
    });

    let (line, hole, problems) = mask::with(|m| (m.ink_line(), m.hole, m.problems.clone()));
    let distinct: HashSet<&String> = keys.iter().flatten().collect();
    // The summary counts the strokes the script actually drove. A line that said "6
    // strokes" on a selection too narrow for 橡皮 would leave the reader to notice that
    // no erase row appeared at all, which is the difference between a declined leg and
    // one nobody asked for.
    let script = match &erase {
        Ok(_) => "7 strokes + 2 erases + 1 undo",
        Err(_) => "7 strokes + 1 undo, 橡皮 declined",
    };
    *plan().lock().unwrap_or_else(PoisonError::into_inner) = Some(Plan {
        rect,
        zig,
        poly,
        arrow,
        zoom,
        text: strip,
        erase,
        keys: keys.clone(),
        line: line.clone(),
        hole,
        notes,
    });
    Ok(format!(
        "{script}, {} publish(es) with {} distinct key(s), hole={hole}, {line}{}",
        keys.len(),
        distinct.len(),
        if problems.is_empty() {
            String::new()
        } else {
            format!(" | problems: {}", problems.join(" | "))
        }
    ))
}

/// Is this pixel this ink? The ink's own leading channel has to beat the other two by
/// `MARGIN`, so a grey background never reads as green and the other ink never reads
/// as this one. An ink with no leading channel (a grey) falls back to exactness.
fn coloured(c: [u8; 4], want: [u8; 4]) -> bool {
    let dom = (0..3).max_by_key(|i| want[*i] as i32).unwrap_or(0);
    if want[dom] < 128 {
        return (0..3).all(|i| (c[i] as i32 - want[i] as i32).abs() <= MARGIN / 4);
    }
    let best_other = (0..3)
        .filter(|i| *i != dom)
        .map(|i| c[i] as i32)
        .max()
        .unwrap_or(0);
    c[dom] as i32 - best_other >= MARGIN
}

/// Two pixels of two bitmaps of the same frozen desktop agreeing.
fn same(a: [u8; 4], b: [u8; 4]) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= SAME_TOL)
}

/// Is this pixel nearer to `a` than to `b`, over the three channels? The row's own
/// tolerances are absolute, so a read-back that is *partway* between the two candidates
/// (a copy whose coverage is not quite one, a seam, an antialiased edge) matches neither
/// and used to join the samples nobody could name. Distance says which way it is leaning
/// without deciding it is a hit: that is the next row's question, asked of the same
/// pixels.
fn nearer(c: [u8; 4], a: [u8; 4], b: [u8; 4]) -> bool {
    (0..3)
        .map(|i| (c[i] as i32 - a[i] as i32).abs())
        .sum::<i32>()
        < (0..3)
            .map(|i| (c[i] as i32 - b[i] as i32).abs())
            .sum::<i32>()
}

/// One stroke's band, as it read. The counters are kept apart rather than collapsed
/// into a percentage, because "the stroke is not there" and "the stroke is in the
/// other box's place" and "the desktop is too dark to divide" are three different
/// findings that a single number would hide.
///
/// The buckets are a *partition*: every planted sample increments `want` and then lands
/// in exactly one of the other eight counters, which is what makes the row's arithmetic
/// checkable by whoever reads it. It was not one before - a readable, in-bounds pixel
/// that matched no bucket incremented nothing at all, and the row still divided it in
/// `usable` without ever naming it. On a dark two-display desktop that unnamed bucket
/// collected 56 of a zoom copy's 211 samples, and the line as printed could not say so:
/// `144/211 … 11 the other ink … 0 bare desktop` leaves the reader to subtract and
/// wonder (the P22 run, recorded in 计划 §10 第 24 项). `unexplained` is now a number
/// the row prints, and [`Hits::examples`] prints the pixels behind the first few of
/// them, so the count is a thing the next round can go and look at.
#[derive(Default)]
struct Hits {
    want: usize,
    ok: usize,
    other_ink: usize,
    plain: usize,
    dark: usize,
    off: usize,
    /// Readable, in this bitmap, and neither candidate - see [`enlarged`]. The other
    /// kind of refusal the screen can hand back, and counted here rather than dropped
    /// from `want` so that `want` means the same word in every row: what the script
    /// planted, which is a property of the script and never of the wallpaper.
    flat: usize,
    /// Neither candidate within its own tolerance, but nearer the expected reading than
    /// the desktop under it: the ink is *partway there*. A miss, and stays one - the
    /// point of counting it separately is that "why is it only partway" and "what is that
    /// pixel doing there" are different questions for whoever reads the row.
    blended: usize,
    /// The sample the gauge cannot account for at all. Not a refusal: it is a miss whose
    /// cause the row does not know, and the difference between those two is the whole
    /// finding.
    unexplained: usize,
    /// Up to [`NAMED`] of the samples this row lost its grade on, with the colours that
    /// made it one. A count without where and what is not actionable by anyone reading
    /// the log, which is why every bucket that costs the row a point names itself here.
    examples: Vec<String>,
}

/// How many of the samples nobody can name the row keeps the pixels of. Three is what
/// it takes to tell a dim antialiased edge from a band drawn in the wrong place, and a
/// count without the pixels it came from is not actionable by anyone reading the log.
const NAMED: usize = 3;

impl Hits {
    /// The samples the screen actually offered: what was planted, minus the pixels too
    /// dark to divide, the pixels that fell off this screen and the pixels whose two
    /// candidates were the same colour. This is the denominator of the grade. Grading
    /// against `want` instead asks the ink to account for the wallpaper - a dark desktop
    /// put 148 of a polyline's 150 samples in `dark`, and the row read `0/150 FAIL` over
    /// a stroke that had not been drawn wrong once (R20).
    /// A pixel that is readable and matches none of the buckets stays in here, which is
    /// the point: an unexplained pixel is a miss, an unreadable one is not. It is now
    /// also counted in [`Hits::unexplained`], so the two ways of arriving at this number
    /// (subtracting the refusals from `want`, or adding up the readings) are the same
    /// arithmetic, and a row that says `211` also says which 211.
    fn usable(&self) -> usize {
        self.want.saturating_sub(self.dark + self.off + self.flat)
    }

    /// The verdict, on a denominator the screen decides. `tenths` is the pass share in
    /// tenths-of-a-whole. A band the screen refused to show is `BLOCKED` rather than
    /// `FAIL`: "this desktop cannot be graded" and "this stroke was drawn wrong" are not
    /// the same finding, and exit 3 is the only way the run says which.
    fn verdict(&self, tenths: usize) -> Check {
        let usable = self.usable();
        if usable < MIN_USABLE {
            return Check::Blocked;
        }
        Check::from(self.ok * 10 >= usable * tenths)
    }

    fn line(&self) -> String {
        let named = if self.examples.is_empty() {
            String::new()
        } else {
            format!("; first {}", self.examples.join(" | "))
        };
        format!(
            "{}/{} as expected of {} planted, {} the other ink, {} bare desktop, {} partway there, {} nobody can name, {} too dark, {} off this screen, {} the desktop cannot distinguish{}",
            self.ok,
            self.usable(),
            self.want,
            self.other_ink,
            self.plain,
            self.blended,
            self.unexplained,
            self.dark,
            self.off,
            self.flat,
            named
        )
    }

    /// One more sample the row could not match, sorted by which candidate it leans
    /// toward and recorded with the three colours that made it one. Both halves in one
    /// call so a count and its witness cannot drift apart.
    fn name_one(&mut self, at: PhysPoint, got: [u8; 4], want: [u8; 4], desk: [u8; 4]) {
        if nearer(got, want, desk) {
            self.blended += 1;
        } else {
            self.unexplained += 1;
        }
        if self.examples.len() < NAMED {
            self.examples.push(format!(
                "{},{} read={:?} not ink={:?} nor desktop={:?}",
                at.x, at.y, got, want, desk
            ));
        }
    }
}

/// Grade one stroke's band against the read-back bitmap. `cover` is what the bitmap is
/// a photograph of, in device pixels; the arithmetic from a desk point to a *bitmap*
/// pixel is spelled out, because a bitmap that came back at the wrong resolution must
/// sample the same desk points rather than a scaled guess at them - the same rule
/// [`mask_check::samples`] runs on.
///
/// `present` says which way round the row is graded: the strokes that are still in the
/// document are wanted, and the undone one is wanted *absent* - which for the ghost
/// means the pixel has to be the frozen desktop again, not merely "not blue".
fn band(
    cover: PhysRect,
    got: &Frame,
    snap: &ScreenSnapshot,
    points: &[PhysPoint],
    want: [u8; 4],
    present: bool,
) -> Hits {
    let mut h = Hits {
        want: 0,
        ok: 0,
        other_ink: 0,
        plain: 0,
        dark: 0,
        off: 0,
        flat: 0,
        blended: 0,
        unexplained: 0,
        examples: Vec::new(),
    };
    let scale_x = cover.w.max(1) as f64 / got.width.max(1) as f64;
    let scale_y = cover.h.max(1) as f64 / got.height.max(1) as f64;
    let other = if want == GREEN { BLUE } else { GREEN };
    for at in points {
        h.want += 1;
        let Some(src) = snap.color_at(*at) else {
            h.off += 1;
            continue;
        };
        if (src[0].max(src[1]).max(src[2]) as u32) < mask_check::MIN_SOURCE {
            h.dark += 1;
            continue;
        }
        let px = ((at.x - cover.x) as f64 / scale_x).round() as i32;
        let py = ((at.y - cover.y) as f64 / scale_y).round() as i32;
        if px < 0 || py < 0 || px >= got.width as i32 || py >= got.height as i32 {
            h.off += 1;
            continue;
        }
        // Exactly one bucket per sample, including the last: a pixel the row cannot
        // explain is a sample it read, and `usable` divides by it whether it is named
        // or not. Naming it is what keeps the line honest.
        let c = got.get(px as u32, py as u32);
        if coloured(c, want) {
            if present {
                h.ok += 1;
            } else {
                h.other_ink += 1;
            }
        } else if coloured(c, other) {
            h.other_ink += 1;
        } else if same(c, src) {
            // The undone stroke's pixels have to be the frozen desktop again, so for
            // that row *this is* the expected reading and it belongs to `ok` alone;
            // counting it in `plain` as well made the buckets overlap and the line's
            // numbers no longer added up to `usable`.
            if present {
                h.plain += 1;
            } else {
                h.ok += 1;
            }
        } else {
            h.name_one(*at, c, want, src);
        }
    }
    h
}

/// One declared area's worth of ink, counted pixel by pixel instead of sampled along a
/// path. The band rows can ask "is the stroke *here*", because the script drew it there;
/// a text box's letters are where the product put them, and the gauge does not get to
/// ask where without repeating the measurement it is meant to be checking. So this row's
/// denominator is a strip the script chose and its reading is a count.
///
/// The buckets partition [`Footprint::planted`] exactly as [`Hits`]' do, for the same
/// reason: a line whose numbers do not add up to the area it asked about is a line that
/// dropped pixels on the way.
#[derive(Default)]
struct Footprint {
    /// Every device pixel of the strip the script declared.
    planted: usize,
    /// This pen's colour, on a pixel the frozen frame was not that colour: the letters.
    ink: usize,
    /// Either of the other two inks, which means the strip is not clear of a neighbour's
    /// stroke - a fact about the script's layout, and printed as one.
    other_ink: usize,
    /// The frozen desktop, unchanged: the space between the letters.
    plain: usize,
    /// Read the pixel, matched none of the above.
    unexplained: usize,
    /// Refusal: the pixel the screen shows is too dark to divide.
    dark: usize,
    /// Refusal: the pixel is on no canvas this window photographs.
    off: usize,
    /// Refusal: the desktop under that pixel was *already* this colour, so the colour
    /// cannot be credited to the ink. [`enlarged`] calls the same refusal `flat`, for
    /// the same reason - a wallpaper that supplies the answer is not evidence.
    preexisting: usize,
    /// The first credited pixel, so the count is a thing the next round can look at.
    first: Option<PhysPoint>,
    examples: Vec<String>,
}

impl Footprint {
    /// The pixels the screen offered: the strip, minus the refusals. Grading against
    /// `planted` instead would ask the ink to account for the wallpaper.
    fn usable(&self) -> usize {
        self.planted
            .saturating_sub(self.dark + self.off + self.preexisting)
    }

    /// Presence, not a share: almost all of a text strip is the space between letters,
    /// so the ratio the band rows grade is meaningless here and a floor on the count is
    /// not. A strip the screen refused to show is `Blocked`, and a strip that shows none
    /// of this pen's colour - or shows somebody else's - is a `Fail`.
    fn verdict(&self) -> Check {
        if self.usable() < MIN_USABLE {
            return Check::Blocked;
        }
        Check::from(self.ink >= MIN_TEXT_INK && self.other_ink == 0)
    }

    fn line(&self) -> String {
        let named = if self.examples.is_empty() {
            String::new()
        } else {
            format!("; first {}", self.examples.join(" | "))
        };
        let first = match self.first {
            Some(p) => format!("{},{}", p.x, p.y),
            None => "-".to_string(),
        };
        format!(
            "{} inked px of {} usable of {} planted, {} the other ink, {} bare desktop, {} nobody can name, {} already this colour in the freeze, {} too dark, {} off this canvas, first ink at {first}{named}",
            self.ink,
            self.usable(),
            self.planted,
            self.other_ink,
            self.plain,
            self.unexplained,
            self.preexisting,
            self.dark,
            self.off,
        )
    }
}

/// Count the 文本 ink inside `strip`, in the same desk-to-bitmap arithmetic [`band`]
/// spells out, so a read-back that came back at another resolution is still sampled at
/// the pixels the strip names.
///
/// The `dark` refusal is keyed to the pixel the *screen* shows, not to the pixel the
/// freeze shows under it - the opposite of what [`band`] does, and deliberately: this
/// row's question is "is the picture reaching the screen this pen's colour", so which of
/// its answers the desktop can decide has to be a property of that picture. Keying it to
/// the freeze is how the R20 run read `0/150 FAIL` over a stroke that had not been drawn
/// wrong once, on nothing but a dark wallpaper.
fn footprint(cover: PhysRect, got: &Frame, snap: &ScreenSnapshot, strip: PhysRect) -> Footprint {
    let mut f = Footprint::default();
    let scale_x = cover.w.max(1) as f64 / got.width.max(1) as f64;
    let scale_y = cover.h.max(1) as f64 / got.height.max(1) as f64;
    for y in strip.y..strip.bottom() {
        for x in strip.x..strip.right() {
            f.planted += 1;
            let at = PhysPoint::new(x, y);
            if !cover.contains(at) {
                f.off += 1;
                continue;
            }
            let Some(src) = snap.color_at(at) else {
                f.off += 1;
                continue;
            };
            let px = ((x - cover.x) as f64 / scale_x).round() as i32;
            let py = ((y - cover.y) as f64 / scale_y).round() as i32;
            if px < 0 || py < 0 || px >= got.width as i32 || py >= got.height as i32 {
                f.off += 1;
                continue;
            }
            let c = got.get(px as u32, py as u32);
            if (c[0].max(c[1]).max(c[2]) as u32) < mask_check::MIN_SOURCE {
                f.dark += 1;
                continue;
            }
            if coloured(c, RED) {
                if same(c, src) {
                    f.preexisting += 1;
                } else {
                    f.ink += 1;
                    if f.first.is_none() {
                        f.first = Some(at);
                    }
                }
                continue;
            }
            if coloured(c, GREEN) || coloured(c, BLUE) {
                f.other_ink += 1;
                continue;
            }
            if same(c, src) {
                f.plain += 1;
                continue;
            }
            f.unexplained += 1;
            if f.examples.len() < NAMED {
                f.examples.push(format!(
                    "{},{} read={:?} not ink={:?} nor desktop={:?}",
                    at.x, at.y, c, RED, src
                ));
            }
        }
    }
    f
}

/// Per-channel agreement for the 放大 row, which is deliberately looser than
/// [`SAME_TOL`]. Before P24 the looseness bought the difference between a nearest
/// expectation and a resampled copy; it now buys something narrower - the copy is
/// graded against the resampling itself, so what is left to forgive is the read-back
/// path (composition, the window's own device scale) rounding a pixel. What the row
/// claims is *this picture is here*, not these bytes match, and the samples it grades
/// are the ones whose two candidates differ by more than this number, so the claim
/// cannot be satisfied by tolerance alone.
const ENLARGE_TOL: i32 = 24;

/// The magnifier's own output for a source rectangle, built the way `raster` builds the
/// copy it lays down: take the source out of the monitor's frozen frame, clipping it to
/// that frame first, and `resized(to.w, to.h, smooth)`.
///
/// This is what [`enlarged`] grades against now. The row used to expect the *source
/// pixel* rounded onto the destination grid, and the product does not produce that:
/// `Kind::Zoom` resamples with a filter that rings at hard edges, so the two answers
/// differ at every edge in the copied picture. On a flat wallpaper they are the same
/// colour and nobody could tell - which is exactly why the P23 run read `98.3%` of the
/// usable samples on one desktop and `73.1%` on a textured one, with the same script,
/// the same 2,185 lattice points and the same buckets.
///
/// What the row stops testing is the *choice of filter*: PRD §5.7.13 has no rules block,
/// so nothing in the requirement lets this gauge call one resampling wrong. What it
/// still tests is *which picture reached the screen*, and that keeps its teeth - a copy
/// made from the wrong rectangle, or laid down at the wrong offset, moves this frame
/// away from the pixels and reads as a miss
/// (`the_row_is_sensitive_to_which_rectangle_the_copy_was_made_from`).
fn magnified(snap: &ScreenSnapshot, from: &PhysRect, w: u32, h: u32) -> Option<Frame> {
    let m = snap
        .monitors
        .iter()
        .find(|m| m.info.contains(from.top_left()))?;
    let local = PhysRect::new(
        from.x - m.info.bounds.x,
        from.y - m.info.bounds.y,
        from.w,
        from.h,
    );
    let src = local.intersection(&m.frame.bounds())?;
    m.frame.crop(&src).ok()?.resized(w, h, true).ok()
}

/// 放大 (§5.7.13) graded against the magnifier's picture rather than a colour.
///
/// Its frames are one device pixel wide - `raster`'s own comment on step 5 - so a colour
/// row over them measures antialiasing, and a copy that never landed leaves the desktop
/// behind, which no colour row can see at all. So each sample asks which of two desktop
/// points the read-back matches: the point it stands on, or the point of the source
/// region as the magnifier renders it at this scale.
///
/// The second return value is how many of the graded samples the *old* nearest
/// expectation would also have called the copy: it is how much of the row's arithmetic
/// the filter actually moves, and it is printed beside the row rather than folded into
/// it - the buckets, their identities and the threshold are unchanged.
///
/// A sample whose two candidates are the same colour cannot answer that - a flat
/// wallpaper magnifies to itself. It goes into [`Hits::flat`] and out of the
/// denominator, which is the same arithmetic the row did when it subtracted these from
/// `want`, but keeps `want` meaning *what the script planted*: the P22 run read
/// `2172 planted` on a dark desktop against `485 planted` on a bright one for one
/// unchanged script, because the number printed as "planted" was really a property of
/// the wallpaper. A refusal therefore lands in a named bucket exactly like the two
/// refusals above, and an untestable desktop still cannot read as a pass.
///
/// The refusals are keyed to the magnified value, not the source pixel, for the same
/// reason the verdict is: the question this row asks is about the picture that reaches
/// the screen, so "too dark to tell" and "magnifies to itself" have to be properties of
/// that picture.
fn enlarged(
    cover: PhysRect,
    got: &Frame,
    snap: &ScreenSnapshot,
    mag: &Frame,
    from: PhysRect,
    to: PhysRect,
    hole: PhysRect,
) -> (Hits, usize) {
    let mut h = Hits {
        want: 0,
        ok: 0,
        other_ink: 0,
        plain: 0,
        dark: 0,
        off: 0,
        flat: 0,
        blended: 0,
        unexplained: 0,
        examples: Vec::new(),
    };
    let mut nearest_ok = 0usize;
    let close =
        |a: [u8; 4], b: [u8; 4]| (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= ENLARGE_TOL);
    // Outside the selection the mask shows the dim, not the ink: the copy is clipped
    // there by §9.1's ③, and a sample past the edge would grade the wallpaper.
    let Some(area) = to.intersection(&hole) else {
        return (h, nearest_ok);
    };
    let inset = PEN / 2 + 2;
    let inner = PhysRect::new(
        area.x + inset as i32,
        area.y + inset as i32,
        area.w.saturating_sub(inset * 2),
        area.h.saturating_sub(inset * 2),
    );
    if inner.is_empty() {
        return (h, nearest_ok);
    }
    let scale_x = cover.w.max(1) as f64 / got.width.max(1) as f64;
    let scale_y = cover.h.max(1) as f64 / got.height.max(1) as f64;
    for y in (inner.y..inner.bottom()).step_by(BAND_STEP as usize) {
        for x in (inner.x..inner.right()).step_by(BAND_STEP as usize) {
            let here = PhysPoint::new(x, y);
            h.want += 1;
            let Some(plain) = snap.color_at(here) else {
                h.off += 1;
                continue;
            };
            // The pixel the user should be looking at: the copy's own grid, which is
            // `to`'s top-left plus this sample's offset into it.
            let (mx, my) = ((x - to.x) as u32, (y - to.y) as u32);
            if mx >= mag.width || my >= mag.height {
                h.off += 1;
                continue;
            }
            let want = mag.get(mx, my);
            if (want[0].max(want[1]).max(want[2]) as u32) < mask_check::MIN_SOURCE {
                h.dark += 1;
                continue;
            }
            if close(want, plain) {
                h.flat += 1;
                continue;
            }
            let px = ((x - cover.x) as f64 / scale_x).round() as i32;
            let py = ((y - cover.y) as f64 / scale_y).round() as i32;
            if px < 0 || py < 0 || px >= got.width as i32 || py >= got.height as i32 {
                h.off += 1;
                continue;
            }
            let c = got.get(px as u32, py as u32);
            // The old expectation, kept as a printed number rather than as a bucket:
            // how often nearest and the filter disagree here is the size of the thing
            // P24 found, and it must not become a second way of passing.
            let near = PhysPoint::new(
                from.x + ((x - to.x) as f64 * from.w as f64 / to.w.max(1) as f64).floor() as i32,
                from.y + ((y - to.y) as f64 * from.h as f64 / to.h.max(1) as f64).floor() as i32,
            );
            if snap.color_at(near).is_some_and(|n| close(c, n)) {
                nearest_ok += 1;
            }
            if close(c, want) {
                h.ok += 1;
            } else if close(c, plain) {
                h.plain += 1;
            } else if coloured(c, GREEN) || coloured(c, BLUE) {
                h.other_ink += 1;
            } else {
                h.name_one(PhysPoint::new(x, y), c, want, plain);
            }
        }
    }
    (h, nearest_ok)
}

/// The check itself, after the event loop has painted.
/// Which of 橡皮's three zones a set of samples belongs to. Each is graded against a
/// different expectation, and only two of them are about colour:
///
/// * [`Zone::Ink`] - the third nobody erased, and the control. Without it "the pixels are
///   gone" is equally a reading of ink that never arrived, which is the confusion
///   [`crate::annotate`]'s §5.7.14 test refuses on purpose.
/// * [`Zone::Base`] - erased with §5.7.14's 默认. The exported picture has to be the
///   frozen frame at that pixel, byte for byte: 只擦标注 hands the screenshot back.
/// * [`Zone::Void`] - erased with the mode on. The pixel has left the picture, so its
///   alpha is 0 whatever its rgb happens to be - which is why this expectation is a
///   number and not a colour. Clearing a pixel is not a colour operation, and the rgb
///   the product leaves behind is the ink's.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Zone {
    Ink,
    Base,
    Void,
}

impl Zone {
    /// The row's name in the report.
    fn name(self) -> &'static str {
        match self {
            Zone::Ink => "erase ink",
            Zone::Base => "erase 只擦标注",
            Zone::Void => "erase 擦到透明",
        }
    }

    /// What the row says it asked for, so the numerator of its fraction is not a mystery.
    fn asks(self) -> &'static str {
        match self {
            Zone::Ink => "the unerased third (ink expected)",
            Zone::Base => "只擦标注 (the screenshot expected)",
            Zone::Void => "擦除到透明 (no pixel expected)",
        }
    }

    /// The counter this zone's verdict is read out of.
    fn of(self, h: &Erased) -> usize {
        match self {
            Zone::Ink => h.ink,
            Zone::Base => h.desk,
            Zone::Void => h.void,
        }
    }

    /// Can this claim be told apart from its opposite on this desktop? Where the frozen
    /// frame is already this pen's colour, "the ink is there" and "the ink was erased"
    /// are the same reading, and the row may not claim either. [`Zone::Void`] needs no
    /// such floor: the freeze is opaque by construction (§3.5① ⑦), so an alpha of 0 is
    /// a thing no wallpaper leaves behind by accident.
    fn creditable(self, base: [u8; 4]) -> bool {
        self == Zone::Void || !coloured(base, GREEN)
    }
}

/// One erase zone, as the exported frame read it. The buckets partition [`Erased::want`]
/// exactly as [`Hits`]' do and for the same reason: a line whose numbers do not add up to
/// what was planted is a line that dropped pixels on the way to the verdict.
#[derive(Default)]
struct Erased {
    want: usize,
    ink: usize,
    desk: usize,
    void: usize,
    neither: usize,
    /// The sample the desktop cannot answer: this pen's colour is already what the freeze
    /// shows there. A refusal, and named as one - it is not a miss.
    uncredited: usize,
    off: usize,
    /// Up to [`NAMED`] samples that came back as something other than what the zone asks,
    /// with both frames and the freeze beside them.
    examples: Vec<String>,
}

impl Erased {
    /// The samples this zone can be graded on at all: planted, minus the pixels off this
    /// canvas and the pixels whose desktop colour is this ink. There is no "too dark" term
    /// because nothing is divided - see [`erase_read`].
    fn usable(&self) -> usize {
        self.want.saturating_sub(self.off + self.uncredited)
    }

    /// A subtraction is not a thing that can be "mostly right", and nothing in this row
    /// goes through a compositor: the export is a bitmap this process made. So the bar is
    /// every graded sample, and a zone that cannot reach [`MIN_USABLE`] graded samples
    /// declines out loud instead of passing on one lucky pixel.
    fn verdict(&self, zone: Zone) -> Check {
        let usable = self.usable();
        if usable < MIN_USABLE {
            return Check::Blocked;
        }
        Check::from(zone.of(self) == usable)
    }

    fn line(&self, zone: Zone) -> String {
        let named = if self.examples.is_empty() {
            String::new()
        } else {
            format!("; first {}", self.examples.join(" | "))
        };
        format!(
            "{}/{} as {} of {} planted, {} this ink, {} the frozen frame, {} transparent, {} neither, \
             {} the desktop is already this ink, {} off this canvas{named}",
            zone.of(self),
            self.usable(),
            zone.asks(),
            self.want,
            self.ink,
            self.desk,
            self.void,
            self.neither,
            self.uncredited,
            self.off,
        )
    }
}

/// What one graded pixel actually read as. A four-way name rather than four counters, so
/// [`Zone::expected`] can say what a row wanted and the loop can say what it got, in the
/// same sentence, without a reader matching counters by eye.
#[derive(Copy, Clone, PartialEq, Debug)]
enum Read {
    Ink,
    Desk,
    Void,
    Neither,
}

impl Zone {
    /// The reading this zone is asking the export for.
    fn expected(self) -> Read {
        match self {
            Zone::Ink => Read::Ink,
            Zone::Base => Read::Desk,
            Zone::Void => Read::Void,
        }
    }
}

/// Read one erase zone out of the **exported** frame.
///
/// No photograph is involved, and that is the whole point of the leg: on the screen the
/// two modes are the same picture, because QML draws the frozen frame under an overlay
/// whose cleared pixels and restored pixels both show the desktop. What differs is what
/// leaves the mask, so that is what gets read. Nor is anything divided: the freeze and
/// the export are two bitmaps this process owns, which is why this row has no `dark`
/// bucket where [`band`] has one, and why its pass bar is every sample.
///
/// `canvas` is this screen's bounds in desktop pixels: the export is canvas-local, and a
/// desktop point becomes one of its pixels by the subtraction `layer == export`'s own walk
/// uses.
fn erase_read(
    zone: Zone,
    at: &[PhysPoint],
    canvas: PhysRect,
    frozen: &ScreenSnapshot,
    export: &Frame,
) -> Erased {
    let mut h = Erased::default();
    for at in at {
        h.want += 1;
        let (lx, ly) = (at.x - canvas.x, at.y - canvas.y);
        if lx < 0 || ly < 0 || (lx as u32) >= export.width || (ly as u32) >= export.height {
            h.off += 1;
            continue;
        }
        let Some(base) = frozen.color_at(*at) else {
            h.off += 1;
            continue;
        };
        if !zone.creditable(base) {
            h.uncredited += 1;
            continue;
        }
        let c = export.get(lx as u32, ly as u32);
        // Exactly one bucket per graded sample. Transparent first: a pixel that left the
        // picture is not this ink, even though the rgb it keeps is the ink's own.
        let read = if c[3] == 0 {
            h.void += 1;
            Read::Void
        } else if coloured(c, GREEN) {
            h.ink += 1;
            Read::Ink
        } else if c == base {
            h.desk += 1;
            Read::Desk
        } else {
            h.neither += 1;
            Read::Neither
        };
        if read != zone.expected() && h.examples.len() < NAMED {
            h.examples.push(format!(
                "{},{} export={:?} over desktop={:?}",
                at.x, at.y, c, base
            ));
        }
    }
    h
}

/// The incremental canvas against a whole-canvas render of the same document, at the
/// pixels 橡皮 touched.
///
/// This is not the `layer == export` row again: that row steps over every pixel whose
/// alpha is 0, because the overlay is only obliged to know the areas it was asked to
/// repaint - and 擦除到透明's entire output is alpha 0. So the one zone that row cannot
/// speak about is graded here, in both directions: a canvas that kept a pixel the export
/// dropped, and a canvas that dropped one the export kept, are both a subtract mode
/// getting the dirty rects wrong, and the first of them is what the user would see as a
/// smear on the mask.
#[derive(Default)]
struct Synced {
    want: usize,
    ok: usize,
    off: usize,
    bad: Vec<String>,
}

impl Synced {
    fn usable(&self) -> usize {
        self.want.saturating_sub(self.off)
    }

    fn verdict(&self) -> Check {
        let usable = self.usable();
        if usable < MIN_USABLE {
            return Check::Blocked;
        }
        Check::from(self.ok == usable)
    }

    fn line(&self) -> String {
        format!(
            "{}/{} of {} planted pixels agree, {} off this canvas, first mismatch: {}",
            self.ok,
            self.usable(),
            self.want,
            self.off,
            if self.bad.is_empty() {
                "none".to_string()
            } else {
                self.bad.join(" | ")
            }
        )
    }
}

fn sync_read(at: &[PhysPoint], canvas: PhysRect, layer: &Frame, export: &Frame) -> Synced {
    let mut s = Synced::default();
    for at in at {
        s.want += 1;
        let (lx, ly) = (at.x - canvas.x, at.y - canvas.y);
        if lx < 0
            || ly < 0
            || (lx as u32) >= layer.width
            || (ly as u32) >= layer.height
            || (lx as u32) >= export.width
            || (ly as u32) >= export.height
        {
            s.off += 1;
            continue;
        }
        let (a, b) = (
            layer.get(lx as u32, ly as u32),
            export.get(lx as u32, ly as u32),
        );
        if a == b {
            s.ok += 1;
        } else if s.bad.len() < NAMED {
            s.bad
                .push(format!("{},{}: layer={a:?} export={b:?}", at.x, at.y));
        }
    }
    s
}

/// 橡皮's four rows, for one canvas. They run **before** anything that needs the
/// `PrintWindow` bitmap, and that order is the finding: a desktop that refuses to be
/// photographed costs this leg nothing, because it never asked the desktop.
fn erase_rows(rep: &mut Report, slot: &mask::Slot, frozen: Option<&ScreenSnapshot>, p: &Plan) {
    let tag = slot.name.clone();
    let e = match &p.erase {
        Ok(e) => e,
        Err(why) => {
            rep.row(&format!("erase of {tag}"), Check::Blocked, why.clone());
            return;
        }
    };
    let Some(frozen) = frozen else {
        rep.row(
            &format!("erase of {tag}"),
            Check::Blocked,
            "the freeze is gone, so neither the desktop nor an erased pixel can be told from nothing"
                .to_string(),
        );
        return;
    };
    let read = {
        // Which canvas the band sits on is geometry, never a pixel count: a second screen
        // the selection never enters has no erase samples and no exported frame to grade
        // them in, and grading its emptiness as a failure would put a spurious row into
        // every run for reasons of desktop layout. Same rule as `layer == export`.
        let band = PhysRect::new(
            e.ink.0.x - ERASE_REACH,
            e.ink.0.y - ERASE_REACH,
            ((e.ink.1.x - e.ink.0.x) + 2 * ERASE_REACH) as u32,
            (2 * ERASE_REACH) as u32,
        );
        match band.intersection(&slot.bounds) {
            None => {
                rep.row(
                    &format!("erase of {tag}"),
                    Check::Blocked,
                    format!(
                        "the 橡皮 band {}..{} does not reach this canvas, so there is nothing here it erased to read",
                        e.ink.0.x, e.ink.1.x
                    ),
                );
                return;
            }
            Some(_) => mask::with(|m| {
                let (Some(export), Some(layer)) =
                    (m.ink.flatten(&slot.name), m.ink.layer_of(&slot.name))
                else {
                    return None;
                };
                let zones = [
                    erase_read(Zone::Ink, &e.zones[0], slot.bounds, frozen, &export),
                    erase_read(Zone::Base, &e.zones[1], slot.bounds, frozen, &export),
                    erase_read(Zone::Void, &e.zones[2], slot.bounds, frozen, &export),
                ];
                let all: Vec<PhysPoint> = e.zones.iter().flatten().cloned().collect();
                Some((zones, sync_read(&all, slot.bounds, layer, &export)))
            }),
        }
    };
    let Some((zones, synced)) = read else {
        rep.row(
            &format!("erase of {tag}"),
            Check::Blocked,
            format!("no canvas by the name {tag}, so there is no exported frame to read"),
        );
        return;
    };
    for (zone, h) in [
        (Zone::Ink, &zones[0]),
        (Zone::Base, &zones[1]),
        (Zone::Void, &zones[2]),
    ] {
        rep.row(
            &format!("{} of {tag}", zone.name()),
            h.verdict(zone),
            h.line(zone),
        );
    }
    rep.row(
        &format!("erase layer == export of {tag}"),
        synced.verdict(),
        synced.line(),
    );
    // The geometry the rows walked, in the same run as the readings: a reader who wants
    // to know whether a zone's brush reached the neighbour's samples can answer it from
    // this line and nothing else.
    rep.note(
        &format!("erase band of {tag}"),
        format!(
            "y {} over x {}..{}: unerased from {}, 只擦标注 {}..{}, 擦除到透明 {}..{}; brush {ERASE_BRUSH} px, \
             {}/{wanted} samples off this canvas",
            e.ink.0.y,
            e.ink.0.x,
            e.ink.1.x,
            e.zones[0].first().map_or(0, |q| q.x),
            e.zones[1].first().map_or(0, |q| q.x),
            e.zones[1].last().map_or(0, |q| q.x),
            e.zones[2].first().map_or(0, |q| q.x),
            e.zones[2].last().map_or(0, |q| q.x),
            zones.iter().map(|z| z.off).sum::<usize>(),
            wanted = zones.iter().map(|z| z.want).sum::<usize>(),
        ),
    );
}

/// The stitched picture, compared against the render it was cut from.
///
/// [`crate::annotate::Layer::deliver`] calls [`crate::annotate::Layer::flatten`], crops
/// the result and pastes it at an offset, so on paper this row compares a frame with
/// itself and could be skipped. It is here because every one of those three steps is a
/// coordinate-space change and both of the bugs this project has had in §9.1 ① were
/// space mix-ups: a crop taken in desktop space instead of canvas space, a paste at the
/// hole's desktop corner instead of its own, a `shift` applied once per screen instead of
/// once per canvas. Each of them leaves an *export* that is correct and a *delivery* that
/// is not, which is exactly the pairing no other row can see - `layer == export` never
/// looks at the hole, and the on-screen rows never look at the frame that would leave the
/// process.
///
/// Exact, not tolerant: the two bitmaps are made by this process, from one document, and
/// nothing composites them. A rounding here is a wrong pixel in the user's file.
fn stitched(
    shipped: &Frame,
    export: &Frame,
    part: PhysRect,
    hole: PhysRect,
    canvas: PhysRect,
) -> Synced {
    let mut s = Synced::default();
    for y in part.y..part.bottom() {
        for x in part.x..part.right() {
            s.want += 1;
            let (gx, gy) = (x - hole.x, y - hole.y);
            let (lx, ly) = (x - canvas.x, y - canvas.y);
            if gx < 0
                || gy < 0
                || lx < 0
                || ly < 0
                || (gx as u32) >= shipped.width
                || (gy as u32) >= shipped.height
                || (lx as u32) >= export.width
                || (ly as u32) >= export.height
            {
                s.off += 1;
                continue;
            }
            let (a, b) = (
                shipped.get(gx as u32, gy as u32),
                export.get(lx as u32, ly as u32),
            );
            if a == b {
                s.ok += 1;
            } else if s.bad.len() < NAMED {
                s.bad.push(format!(
                    "{x},{y}: shipped={a:?} (from {gx},{gy}) export={b:?} (from {lx},{ly})"
                ));
            }
        }
    }
    s
}

/// Share of the graded samples row B has to agree on, in tenths. Deliberately not the
/// exactness row A grades: this is one Rust-made frame read back through Qt's texture
/// path, a compositor and `PrintWindow`, so antialiasing and a rounding are the expected
/// residue. `blended` absorbs a pixel that is nearer the shipped picture than the desktop
/// under it, which is the residue's own signature - and 9/10 is high enough that an
/// overlay which simply never repainted (the desktop showing through everywhere) still
/// fails.
const SHIP_PASS: usize = 9;

/// What the screen shows inside the selection, against the picture the product says it
/// will deliver.
///
/// Every other read-back row in this gauge compares the screen with a *stroke* - a green
/// band, a text strip, a magnified patch - which means the shipped frame itself has never
/// been on screen in any test until now. This row asks the whole-picture question on the
/// same ~2000-sample grid `--mask` uses, in the same desk-to-bitmap arithmetic, over the
/// part of the selection this window covers and [`mask_check::BORDER_DIP`] away from the
/// selection's own edge (that ring is the dim's boundary and the white frame, not a
/// measurement).
///
/// The `flat` refusal is what makes this row more than a second copy of row A: over most
/// of the picture the shipped pixel *is* the frozen desktop - only the ink differs - so a
/// sample where the two candidates are the same colour cannot distinguish a correct
/// delivery from an overlay that paints nothing, and is refused rather than credited.
/// The samples that survive are the ink's own pixels, which is the part of the delivery
/// only a compositor can lose.
fn shipped_on_screen(
    cover: PhysRect,
    got: &Frame,
    shipped: &Frame,
    snap: &ScreenSnapshot,
    hole: &PhysRect,
    part: PhysRect,
    dpr: f64,
) -> Hits {
    let mut h = Hits::default();
    let scale_x = cover.w.max(1) as f64 / got.width.max(1) as f64;
    let scale_y = cover.h.max(1) as f64 / got.height.max(1) as f64;
    let border = (mask_check::BORDER_DIP * dpr / scale_x.max(1.0)) as i32;
    // The same sample budget as `--mask`, spent inside this rectangle rather than across
    // the whole window: a grid laid over the window would grade a selection-sized picture
    // at a fraction of its own resolution, and a small selection would then be a row the
    // screen refused.
    let step = ((part.w as f64 * part.h as f64 / 2000.0).sqrt().ceil() as i32).max(1);
    for y in (part.y..part.bottom()).step_by(step as usize) {
        for x in (part.x..part.right()).step_by(step as usize) {
            let at = PhysPoint::new(x, y);
            let dx = (at.x - hole.x).min(hole.right() - 1 - at.x).abs();
            let dy = (at.y - hole.y).min(hole.bottom() - 1 - at.y).abs();
            if dx <= border || dy <= border {
                // Not a planted sample at all, so it joins none of the buckets: the ring is
                // where the dim's edge and the selection's own frame live, and `--mask`
                // refuses it for the same reason.
                continue;
            }
            h.want += 1;
            let bx = ((at.x - cover.x) as f64 / scale_x).round() as i32;
            let by = ((at.y - cover.y) as f64 / scale_y).round() as i32;
            let (sx, sy) = (at.x - hole.x, at.y - hole.y);
            if bx < 0
                || by < 0
                || (bx as u32) >= got.width
                || (by as u32) >= got.height
                || (sx as u32) >= shipped.width
                || (sy as u32) >= shipped.height
            {
                h.off += 1;
                continue;
            }
            // On no desktop this run froze: the sample has no second candidate to be
            // refused against and no first one to be graded on.
            let Some(src) = snap.color_at(at) else {
                h.off += 1;
                continue;
            };
            let (seen, want) = (
                got.get(bx as u32, by as u32),
                shipped.get(sx as u32, sy as u32),
            );
            if want[3] == 0 {
                // The delivery carries no colour here at all: it is a pixel the 擦除到透明
                //档 erased, and what the screen must show through it is whatever is under
                // the ink. Refused rather than blamed - an RGB comparison cannot agree with
                // "nothing", and crediting it would credit a document that ships no ink.
                // The band itself is graded by 橡皮's own rows and by the exact stitch
                // above, where alpha is compared in full.
                h.flat += 1;
                if h.examples.len() < NAMED {
                    h.examples.push(format!(
                        "{},{}: shipped alpha 0 ({want:?}), screen={seen:?} freeze={src:?}",
                        at.x, at.y
                    ));
                }
                continue;
            }
            if (want[0].max(want[1]).max(want[2]) as u32) < mask_check::MIN_SOURCE {
                // The expectation is black, so nothing the screen shows here can be
                // divided: black times anything is black (`--mask`'s `MIN_SOURCE` rule).
                h.dark += 1;
                continue;
            }
            if same(want, src) {
                // The delivery and the desktop are the same colour at this pixel, which is
                // most of the picture: only the ink differs. Such a sample cannot tell a
                // correct delivery from no delivery, so it is refused - `enlarged`'s `flat`
                // - and never credited. Without this rung the row would pass on a desktop
                // whose overlay painted nothing at all.
                h.flat += 1;
                continue;
            }
            if same(seen, want) {
                h.ok += 1;
                continue;
            }
            if same(seen, src) {
                h.plain += 1;
                // The distance to the selection's own frame travels with the sample. That
                // frame is decoration no delivery contains, so a miss sitting on it and a
                // miss in the middle of the picture are different findings - and a reader
                // has to be able to tell them apart without re-running the gauge.
                if h.examples.len() < NAMED {
                    h.examples.push(format!(
                        "{},{}: edge {dx},{dy} screen={seen:?} shipped={want:?} freeze={src:?}",
                        at.x, at.y
                    ));
                }
                continue;
            }
            if nearer(seen, want, src) {
                h.blended += 1;
                continue;
            }
            h.unexplained += 1;
            if h.examples.len() < NAMED {
                h.examples.push(format!(
                    "{},{}: screen={seen:?} shipped={want:?} freeze={src:?}",
                    at.x, at.y
                ));
            }
        }
    }
    h
}

/// §5.5.2's 复制, walked on the machine's own clipboard.
///
/// The leg reads the user's contents before it writes anything and puts them back after,
/// through [`crate::state::put_back`] - the same debt [`--selftest --clipboard`](crate::state)
/// pays, in the same four branches, including the one that means "the clipboard was empty"
/// and so has to make it empty again rather than leave the screenshot behind. A clipboard
/// that will not open for *reading* is refused rather than written to: taking contents
/// this gauge cannot restore is not a measurement anyone is entitled to make on someone
/// else's desktop. The same rule covers a clipboard that holds RTF or a file list -
/// readable, but not something this app can put back.
///
/// Nothing here is compared against a stroke or a band. The claim is narrower and harder
/// than the on-screen rows: the bytes that came back out of the clipboard are the bytes
/// [`crate::annotate::Layer::deliver`] handed over, at the size of the selection.
fn clipboard_rows(rep: &mut Report, shipped: Option<&Frame>) {
    // The reason travels with the refusal: "would not open" is only actionable once the
    // row says who is holding it, which is the same thing `--selftest --clipboard`
    // learned to print instead of a row of failures that reads as a broken app.
    let before = match platform_windows::clip::read() {
        Ok(c) => c,
        Err(e) => {
            rep.row(
                "clipboard copy",
                Check::Blocked,
                format!(
                    "the clipboard would not open for reading ({e}), and this leg will not \
                     write what it cannot put back"
                ),
            );
            return;
        }
    };
    let formats = platform_windows::clip::formats().unwrap_or_default();
    rep.note(
        "clipboard before",
        format!(
            "{} bytes of image, {} chars of text, {} format(s): {}",
            before.image_bytes.as_ref().map_or(0, |b| b.len()),
            before
                .text
                .as_ref()
                .map(|t| t.chars().count())
                .unwrap_or_default(),
            formats.len(),
            formats.join(",")
        ),
    );
    // Counts and format names only, never the contents: a log that quoted what the user
    // had copied would be a second copy of the user's clipboard.
    if !before.is_empty() && before.image_bytes.is_none() && before.text.is_none() {
        rep.row(
            "clipboard copy",
            Check::Blocked,
            format!(
                "the clipboard holds {} , which this app cannot put back after writing over \
                 it, so nothing was written",
                formats.join(",")
            ),
        );
        return;
    }
    if let Err(e) = platform_windows::clip::usable() {
        // The product's own first rung, on the machine's own answer: PRD §7.2 asks for the
        // holder's name, and `ClipError::Locked` displays the holder's name.
        rep.row(
            "clipboard copy",
            Check::Blocked,
            format!("{e} - nothing was written, so nothing had to be put back"),
        );
        return;
    }
    let done = mask::with(|m| m.copy_out());
    let said = mask::with(|m| m.message.clone());
    rep.note("clipboard sentence", said.clone());
    // Read back *before* anything is put away. The restore is itself a write, and every
    // write opens with `EmptyClipboard`, so grading afterwards would grade the user's own
    // text against the selection - which is exactly the reading this leg produced on its
    // first walk of a real clipboard: `copy_out=true`, the product said 已复制, and 0 px
    // of picture came back because the picture had already been replaced.
    let formats_after = platform_windows::clip::formats().unwrap_or_default();
    let came_back = platform_windows::clip::read()
        .ok()
        .and_then(|c| c.image_bytes);
    rep.note(
        "clipboard after",
        format!(
            "{} bytes of image, {} format(s): {}",
            came_back.as_ref().map_or(0, |b| b.len()),
            formats_after.len(),
            formats_after.join(",")
        ),
    );
    let read_back = came_back.map(|bytes| falcon_core::encode::decode(&bytes));
    // Restored whether or not the copy succeeded: `write_formats` empties the clipboard
    // before it publishes, so a write that failed half-way had already taken the user's
    // contents with it. Where nothing was taken - either pre-clipboard rung - emptying an
    // already empty clipboard is a no-op with the same name.
    let back = crate::state::put_back(&Some(before));
    let (mut ok, mut total, mut bad, mut whole, mut gone) =
        (0usize, 0usize, Vec::new(), false, 0usize);
    if let (Some(Ok(frame)), Some(want)) = (&read_back, shipped) {
        total = frame.width as usize * frame.height as usize;
        if frame.width == want.width && frame.height == want.height {
            whole = true;
            for y in 0..frame.height {
                for x in 0..frame.width {
                    // Three channels, not four: `CF_DIB` carries no alpha, so what a paste
                    // target reads is the RGB the DIB holds, and the alpha the PNG path
                    // re-adds is 255 everywhere. That is not a harmless loss - a pixel the
                    // 擦除到透明 档 erased has no colour to lose, but the RGB underneath it
                    // is the ink's own, and it is what arrives. Counted here and quoted in
                    // the row, so a full-mark comparison does not read as though
                    // transparency survived the trip.
                    let (a, b) = (frame.get(x, y), want.get(x, y));
                    if b[3] == 0 {
                        gone += 1;
                    }
                    if same(a, b) {
                        ok += 1;
                    } else if bad.len() < NAMED {
                        bad.push(format!("{x},{y}: clipboard={a:?} shipped={b:?}"));
                    }
                }
            }
        }
    }
    let verdict = match (&read_back, shipped) {
        (Some(Ok(_)), Some(_)) => Check::from(done && whole && bad.is_empty()),
        // The product said 已复制 and there is no picture to read, or the bytes it put
        // there are not a picture this app can decode: either way the leg's claim died at
        // this rung and not at the comparison above.
        _ => Check::Fail,
    };
    rep.row(
        "clipboard copy",
        verdict,
        format!(
            "copy_out={done}, {} of {total} px of the selection came back as shipped, size \
             matches={whole}, first mismatch: {}; the product's own sentence was {said:?}{}",
            ok,
            if bad.is_empty() {
                "none".to_string()
            } else {
                bad.join(" | ")
            },
            if total == 0 {
                // The count alone does not say whether the write missed, the read missed,
                // or the bytes are a picture this decoder rejects; the format list is the
                // one reading that separates those three.
                format!(
                    ", and the clipboard held {} at the moment it was read back",
                    formats_after.join(",")
                )
            } else if gone > 0 {
                format!(
                    ", of which {gone} px are transparent in the document and reached the \
                     clipboard as the colour under them"
                )
            } else {
                String::new()
            }
        ),
    );
    rep.row(
        "clipboard restored",
        Check::from(back.is_ok()),
        back.unwrap_or_else(|e| e),
    );
}

pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
    let Some(p) = plan().lock().unwrap_or_else(PoisonError::into_inner).take() else {
        return (
            Check::Fail,
            "ink check: FAIL - nothing was planted, so there is no script to read back".to_string(),
        );
    };
    rep.note(
        "planted",
        format!(
            "{}{}",
            p.line,
            if p.notes.is_empty() {
                String::new()
            } else {
                format!(" | {}", p.notes.join(" | "))
            }
        ),
    );

    let snap = capture::current();
    let levels = parse_levels(&session::shim::top_levels());
    let masks: Vec<&Top> = levels
        .iter()
        .filter(|t| t.title.starts_with(mask_check::TITLE))
        .collect();
    let slots = mask::with(|m| m.slots.clone());

    // The picture the product would hand over right now, taken once for the whole run:
    // [`crate::annotate::Layer::deliver`] re-stitches from the per-screen renders on
    // every call, and a delivery row that called it per screen could otherwise be
    // grading two frames that differ from each other.
    let shipped = mask::with(|m| m.ink.deliver());
    let uncovered = mask::with(|m| m.ink.uncovered());
    rep.note(
        "shipped",
        match &shipped {
            Some(f) => format!(
                "{}×{} frame, {uncovered} px of {:?} supplied by no canvas",
                f.width, f.height, p.hole
            ),
            None => "the selection stitched no frame at all".to_string(),
        },
    );

    // Every key the flow handed Qt: one frozen frame per screen, and one ink texture
    // per publish. The release row checks all of them, because a leak here is a
    // desktop-sized leak per stroke.
    let frame_keys: Vec<String> = slots.iter().map(|s| s.key.clone()).collect();
    let ink_keys: Vec<String> = p.keys.iter().flatten().cloned().collect();
    let distinct: HashSet<&String> = ink_keys.iter().collect();
    rep.row(
        "keys distinct",
        Check::from(!ink_keys.is_empty() && distinct.len() == ink_keys.len()),
        format!(
            "{} publish(es), {} distinct: {}",
            ink_keys.len(),
            distinct.len(),
            ink_keys.join(",")
        ),
    );

    for slot in &slots {
        let tag = slot.name.clone();
        // 橡皮 first, and it stays first even when everything below it declines: the leg
        // reads two bitmaps this process made, so a desktop that refuses `PrintWindow`
        // still gets its two erase modes graded here.
        erase_rows(&mut rep, slot, snap.as_ref(), &p);
        let want_title = format!("{}{}", mask_check::TITLE, slot.name);
        let Some(win) = masks.iter().find(|t| t.title == want_title) else {
            rep.row(
                &format!("window {tag}"),
                Check::Fail,
                format!("no top-level titled {want_title}"),
            );
            continue;
        };
        let Some(got) = platform_windows::print::print_window(win.hwnd, win.phys.w, win.phys.h, 2)
        else {
            for row in [
                "ink box",
                "ink stroke",
                "ink polyline",
                "ink text",
                "zoom copy",
                "no ghost",
                "dim",
                "hole",
            ] {
                rep.row(
                    &format!("{row} of {tag}"),
                    Check::Blocked,
                    "PrintWindow(flag 2) produced no bitmap for this window".to_string(),
                );
            }
            continue;
        };
        let Some(frozen) = &snap else {
            rep.row(
                &format!("ink of {tag}"),
                Check::Blocked,
                "the freeze is gone; nothing to compare against".to_string(),
            );
            continue;
        };

        // Which dim path is under the ink. A shader that did not compile dims the
        // layer along with the desktop, and the ink rows below would then read as
        // strokes that never landed - so the path is written down beside them.
        rep.note(
            &format!("dim path {tag}"),
            match (mask::with(|m| m.shader), slot.shader_status) {
                (false, _) => "rectangles (chosen)",
                (true, Some(0)) => "shader",
                (true, _) => "rectangles (fallback)",
            }
            .to_string(),
        );
        rep.note(
            &format!("overlay of {tag}"),
            mask::with(|m| m.ink.overlay_key(&slot.name)),
        );

        // The dirty-rect claim, checked byte for byte rather than by ratio:
        // `layer_of` is what the incremental `paint` produced and `flatten` is a
        // whole-canvas `raster::render` of the same document.
        //
        // Wherever the layer has a pixel to show, it must be the composited picture -
        // a too-narrow repaint, a stale rect or an undone stroke's ghost all surface
        // here as a count and a first coordinate, in Rust, before any of them can be
        // blamed on composition or on `PrintWindow`. Where it is transparent the
        // comparison does not apply: the overlay only knows the areas it was asked to
        // repaint, and QML draws the frozen frame underneath the rest of it.
        let (checked, bare, bad) = mask::with(|m| {
            let (Some(export), Some(layer)) =
                (m.ink.flatten(&slot.name), m.ink.layer_of(&slot.name))
            else {
                return (0, 0, vec!["no canvas by that name".to_string()]);
            };
            let mut n = 0usize;
            let mut bare = 0usize;
            let mut bad: Vec<String> = Vec::new();
            for y in p.hole.y..p.hole.bottom() {
                for x in p.hole.x..p.hole.right() {
                    let (lx, ly) = (x - slot.bounds.x, y - slot.bounds.y);
                    if lx < 0 || ly < 0 || (lx as u32) >= layer.width || (ly as u32) >= layer.height
                    {
                        continue;
                    }
                    let a = layer.get(lx as u32, ly as u32);
                    if a[3] == 0 {
                        bare += 1;
                        continue;
                    }
                    n += 1;
                    let b = export.get(lx as u32, ly as u32);
                    if a != b && bad.len() < 3 {
                        bad.push(format!("{x},{y}: layer={a:?} export={b:?}"));
                    }
                }
            }
            (n, bare, bad)
        });
        // Which way round this comparison applies is decided by the *geometry*, never by
        // `checked`: a canvas the selection never enters legitimately has no opaque
        // pixels, and grading its emptiness as a failure puts a second screen into every
        // run for reasons of desktop layout. Keying the decline on `checked == 0` instead
        // would have swallowed the defect P22 found here, where the hole *was* on this
        // canvas and the clip was measured in the wrong space, leaving the whole layer
        // transparent. A canvas with a hole and nothing painted is the finding; a canvas
        // without a hole is not a canvas this row can speak about.
        let painted_here = p.hole.intersection(&slot.bounds);
        if let Some(part) = painted_here {
            rep.row(
                &format!("layer == export of {tag}"),
                Check::from(checked > 0 && bad.is_empty()),
                format!(
                    "{checked} opaque px compared, {bare} transparent (not the overlay's business), {} of the hole on this canvas, first mismatch: {}",
                    part.area(),
                    if bad.is_empty() {
                        "none".to_string()
                    } else {
                        bad.join(" | ")
                    }
                ),
            );
        } else {
            rep.row(
                &format!("layer == export of {tag}"),
                Check::Blocked,
                format!(
                    "the hole reaches none of this canvas, so the overlay shows nothing here to compare ({} transparent)",
                    bare
                ),
            );
        }

        // What leaves the process, pixel for pixel, against the render it was cut from.
        // The two bitmaps are both made here, so the comparison is exact - see
        // [`stitched`] for why an exactness test of a frame built out of itself is the
        // only place a stitch's own arithmetic can be caught.
        let export = mask::with(|m| m.ink.flatten(&slot.name));
        match (p.hole.intersection(&slot.bounds), &shipped, export) {
            (Some(part), Some(frame), Some(export)) => {
                let s = stitched(frame, &export, part, p.hole, slot.bounds);
                rep.row(
                    &format!("shipped == export of {tag}"),
                    s.verdict(),
                    s.line(),
                );
            }
            (None, _, _) => rep.row(
                &format!("shipped == export of {tag}"),
                Check::Blocked,
                "the hole reaches none of this canvas, so this screen supplies none of the \
                 picture"
                    .to_string(),
            ),
            (Some(_), None, _) => rep.row(
                &format!("shipped == export of {tag}"),
                Check::Fail,
                "the selection stitched no frame at all".to_string(),
            ),
            (Some(_), Some(_), None) => rep.row(
                &format!("shipped == export of {tag}"),
                Check::Fail,
                "the canvas the hole is on exported no frame, so the stitch had nothing to \
                 paste"
                    .to_string(),
            ),
        }

        for (label, points, want, tenths) in [
            ("ink box", rect_band(p.rect), GREEN, INK_PASS),
            ("ink stroke", path_band(&p.zig), GREEN, INK_PASS),
            ("ink polyline", path_band(&p.poly), GREEN, INK_PASS),
        ] {
            let hits = band(win.phys, &got, frozen, &points, want, true);
            rep.row(
                &format!("{label} of {tag}"),
                hits.verdict(tenths),
                hits.line(),
            );
        }

        // 文本 (§5.7.11), counted over an area rather than sampled along a path: the
        // click is the script's and the letters are the product's, so the strip is the
        // one thing the two can agree on without the row grading its own arithmetic.
        // This is also the desktop control P26's placement fix never had - a box filed
        // anywhere other than under the click that opened it is a strip that reads back
        // as the frozen desktop, and no headless test can tell those two apart because
        // both of them ask `box_at` the same question and take the same answer.
        match p.text {
            Some(strip) => {
                let fp = footprint(win.phys, &got, frozen, strip);
                rep.row(&format!("ink text of {tag}"), fp.verdict(), fp.line());
                rep.note(
                    &format!("text strip of {tag}"),
                    format!("{strip:?} typed {TEXT:?}"),
                );
            }
            None => rep.row(
                &format!("ink text of {tag}"),
                Check::Blocked,
                "the selection is too small for a text strip that clears the bands the \
                 other rows grade, so none of this pen's colour was planted to count"
                    .to_string(),
            ),
        }

        // 放大 (§5.7.13), graded against the picture rather than the pen: see
        // [`enlarged`]. A copy that never landed shows the desktop, and the only other
        // thing that can leave the desktop showing is an overlay key that was stored
        // under a name Qt had already asked for - so this row is the second reading of
        // §3.6 constraint 8, on the stroke nobody can grade by colour.
        match p.zoom {
            Some((src, copy)) => {
                let built = magnified(frozen, &src, copy.w, copy.h);
                let (hits, nearest_ok) = match &built {
                    Some(mag) => enlarged(win.phys, &got, frozen, mag, src, copy, p.hole),
                    // The expectation is the one thing this row cannot do without: if
                    // no captured monitor covers the source rectangle, there is no
                    // magnifier output to compare against, and that is a script problem
                    // rather than the desktop declining to answer.
                    None => (Hits::default(), 0),
                };
                let verdict = if built.is_some() {
                    hits.verdict(INK_PASS)
                } else {
                    Check::Fail
                };
                let detail = if built.is_some() {
                    hits.line()
                } else {
                    format!(
                        "the source rect {src:?} is on no captured monitor, so there is no \
                         magnifier output to grade the copy against"
                    )
                };
                rep.row(&format!("zoom copy of {tag}"), verdict, detail);
                rep.note(
                    &format!("zoom expectation of {tag}"),
                    format!(
                        "the magnifier's own resampled frame; nearest would have graded {} \
                         of the graded samples the same way",
                        nearest_ok
                    ),
                );
            }
            None => rep.row(
                &format!("zoom copy of {tag}"),
                Check::Fail,
                "放大 left no copy in the document; see the planted notes".to_string(),
            ),
        }

        // The undone stroke: its pixels have to be the frozen desktop again. `other_ink`
        // in this row is the ghost itself, counted where it was drawn.
        let ghost = band(
            win.phys,
            &got,
            frozen,
            &along(p.arrow[0], p.arrow[1]),
            BLUE,
            false,
        );
        rep.row(
            &format!("no ghost of {tag}"),
            ghost.verdict(GHOST_PASS),
            format!("{}, and that ink is the ghost", ghost.line()),
        );

        // The dim, which the layer must not have disturbed outside the hole, and the
        // un-inked desktop inside it.
        let sides: Sides = samples(win.phys, &got, frozen, &p.hole, slot.scale.ratio());
        for (label, want, values) in [
            ("dim", 1.0 - mask::DIM, &sides.outside),
            ("hole", 1.0, &sides.inside),
        ] {
            let (verdict, detail) = ratio_row(values.len(), median(values), want, &sides);
            rep.row(&format!("{label} of {tag} (ink up)"), verdict, detail);
        }
        // The delivery as the desktop shows it. Every row above this one grades a stroke
        // against the screen; none of them has ever graded the *picture* against it, and
        // the picture is what §5.5.2 hands to another application.
        match (shipped.as_ref(), p.hole.intersection(&slot.bounds)) {
            (Some(frame), Some(part)) => {
                let hits = shipped_on_screen(
                    win.phys,
                    &got,
                    frame,
                    frozen,
                    &p.hole,
                    part,
                    slot.scale.ratio(),
                );
                rep.row(
                    &format!("shipped on screen of {tag}"),
                    hits.verdict(SHIP_PASS),
                    hits.line(),
                );
            }
            (None, _) => rep.row(
                &format!("shipped on screen of {tag}"),
                Check::Fail,
                "the selection stitched no frame, so there is no picture this screen could \
                 be showing"
                    .to_string(),
            ),
            (_, None) => rep.row(
                &format!("shipped on screen of {tag}"),
                Check::Blocked,
                "the hole reaches none of this canvas, so this window photographs none of \
                 the delivery"
                    .to_string(),
            ),
        }
        mask_check::flag_column(&mut rep, win);
        rep.note(
            &format!("swaps of {tag}"),
            format!("{} frame(s), first at {:?}", slot.swaps, slot.first_swap_ms),
        );
    }

    // The selection's own completeness, which is not either delivery row's question: a
    // hole that left the captured desktop ships *something* for every pixel it covers and
    // nothing for the rest, and both of the comparisons above are right about the part
    // they can see. A drag can leave the desktop on a machine with one monitor, so this is
    // a row and not an assumption.
    rep.row(
        "shipped covers the hole",
        Check::from(uncovered == 0 && shipped.is_some()),
        format!(
            "{uncovered} of {} px of {:?} came from no canvas{}",
            p.hole.area(),
            p.hole,
            if shipped.is_some() {
                String::new()
            } else {
                ", and no frame was stitched at all".to_string()
            }
        ),
    );

    // The one leg of this gauge that touches something outside the process. It runs last
    // among the delivery rows and before the mask comes down, because it is the leg that
    // needs the selection, the ink and the freeze all still standing.
    clipboard_rows(&mut rep, shipped.as_ref());

    let (mask_problems, ink_problems) =
        mask::with(|m| (m.problems.clone(), m.ink.problems.clone()));
    let all: Vec<String> = mask_problems.into_iter().chain(ink_problems).collect();
    rep.row("no problems", Check::from(all.is_empty()), all.join(" | "));
    mask_check::qml_row(&mut rep);
    rep.note("ink state", mask::with(|m| m.ink_line()));

    // Down, and everything out of Qt's process. An ink texture is the same size as the
    // screen it covers, and publishing one per repaint means a flow that ends without
    // releasing them leaks a desktop per *stroke*.
    let mut keys = frame_keys;
    keys.extend(ink_keys);
    mask::with(|m| m.close());
    let left: Vec<&String> = keys
        .iter()
        .filter(|k| !shim::self_check(k).contains("null=1"))
        .collect();
    rep.row(
        "textures released",
        Check::from(left.is_empty()),
        format!(
            "{} of {} still in Qt's process after close",
            left.len(),
            keys.len()
        ),
    );
    // §10 第 33 项 ①, graded rather than assumed. This script teaches the pens it measures
    // and the flow it drives ends the way the product's does, so the run writes a settings
    // file - and §5.7.1 promises that file to the *user*, whose last pen it is supposed to
    // be. [`plant`] pins the writes into the temp dir; a pin somebody later deletes is a
    // defect that runs once per measurement and never says so, which is what this row is
    // for. Read after `close`, because `close` is what writes.
    let (saves, read_from, write_to) =
        crate::settings::with(|s| (s.saves, s.read_from.clone(), s.write_to.clone()));
    let pinned = write_to != read_from && write_to.starts_with(std::env::temp_dir());
    rep.row(
        "config writes pinned",
        match (saves, pinned) {
            (0, _) => Check::Blocked,
            (_, true) => Check::Pass,
            (_, false) => Check::Fail,
        },
        format!(
            "{saves} write(s): read from {}, wrote to {}{}",
            read_from.display(),
            write_to.display(),
            match (saves, pinned) {
                (0, _) => " - nothing moved, so the pin was never exercised".to_string(),
                (_, true) => String::new(),
                (_, false) => " - which is the file the user sets their pens in".to_string(),
            }
        ),
    );
    rep.finish("ink check")
}

#[cfg(test)]
mod tests {
    use super::*;
    use falcon_core::capture::{FrozenMonitor, MonitorInfo};
    use falcon_core::geometry::Scale;

    /// A band that read on the dark desktop of R20's third run: 16 of 165 samples
    /// visible, and every one of those was the expected ink.
    fn r20() -> Hits {
        Hits {
            want: 165,
            ok: 16,
            other_ink: 0,
            plain: 0,
            dark: 149,
            off: 0,
            flat: 0,
            blended: 0,
            unexplained: 0,
            examples: Vec::new(),
        }
    }

    #[test]
    fn a_desktop_too_dark_to_read_blocks_the_row_instead_of_failing_it() {
        // The whole point of the change: this row was `FAIL 16/165`, and it read exactly
        // like a stroke that had been drawn wrong. Nothing about the ink is being
        // asserted here - the screen refused 149 of the 165 samples.
        assert_eq!(r20().usable(), 16);
        assert_eq!(r20().verdict(INK_PASS), Check::Blocked);
        // The same band on a desktop that does show its pixels grades normally, so
        // BLOCKED is not this row's way of always declining.
        let shown = Hits {
            ok: 149,
            plain: 16,
            dark: 0,
            ..r20()
        };
        assert_eq!(shown.usable(), 165);
        assert_eq!(shown.verdict(INK_PASS), Check::Pass);
    }

    #[test]
    fn a_band_that_can_be_read_and_does_not_match_still_fails() {
        // BLOCKED must not become where a real defect goes to hide: 40 samples the
        // desktop refused, 125 it showed, and not one of those is the stroke.
        let missing = Hits {
            want: 165,
            ok: 0,
            other_ink: 0,
            plain: 125,
            dark: 40,
            off: 0,
            flat: 0,
            blended: 0,
            unexplained: 0,
            examples: Vec::new(),
        };
        assert_eq!(missing.usable(), 125);
        assert_eq!(missing.verdict(INK_PASS), Check::Fail);
    }

    #[test]
    fn the_row_names_the_sample_it_divided_by_and_the_one_it_was_planted() {
        // Both numbers, or the row cannot be read as a measurement of the ink rather
        // than of the desktop - and now every bucket between them is named too, so the
        // reader can add the line up instead of subtracting from it.
        assert_eq!(
            r20().line(),
            "16/16 as expected of 165 planted, 0 the other ink, 0 bare desktop, 0 partway there, 0 nobody can name, 149 too dark, 0 off this screen, 0 the desktop cannot distinguish"
        );
    }

    /// A one-monitor desktop of one colour, 200x200 device pixels at scale 1, so a
    /// reading is a fact about the bitmaps handed to the gauge rather than about the
    /// machine the test runs on. The monitor and its frame have to agree or
    /// [`ScreenSnapshot::color_at`] answers `None` and every sample lands in `off`.
    fn desk(rgba: [u8; 4]) -> ScreenSnapshot {
        desk_sized(200, 200, rgba)
    }

    /// [`desk`] at a size the caller picks. 橡皮's zones need it: a run is sampled every
    /// [`BAND_STEP`] pixels, so a 200 px canvas hands a zone 25 samples and [`MIN_USABLE`]
    /// is 30 - the desk every other row uses is too small to be able to pass.
    fn desk_sized(w: u32, h: u32, rgba: [u8; 4]) -> ScreenSnapshot {
        let mut snap = ScreenSnapshot {
            monitors: vec![FrozenMonitor {
                info: MonitorInfo {
                    id: "m0".into(),
                    name: "display1".into(),
                    friendly: "display1".into(),
                    bounds: PhysRect::new(0, 0, w, h),
                    scale: Scale::ONE,
                    primary: true,
                    builtin: false,
                },
                frame: Frame::filled(w, h, rgba).expect("frame"),
            }],
            virtual_bounds: PhysRect::new(0, 0, w, h),
            ..Default::default()
        };
        // The patch the `dark` bucket needs: a source too dim to divide, in the corner
        // rows of samples never visit.
        snap.monitors[0]
            .frame
            .fill_rect(&PhysRect::new(0, h as i32 - 10, w, 10), [4, 4, 4, 255]);
        snap
    }

    /// A pixel bright enough to read and past both candidates' tolerances. Against the
    /// green ink the colour rows expect it is nearer the grey desktop, so those rows call
    /// it *unexplained*; against the red source the 放大 row compares it is nearer the
    /// source, so that row calls it *partway*. One pixel, and the two readings are exactly
    /// the difference these buckets exist to make.
    const ORPHAN: [u8; 4] = [255, 0, 255, 255];
    /// A pixel a little off the desktop and past its tolerance, leaning toward nothing:
    /// unexplained for either row.
    const ODD: [u8; 4] = [160, 200, 200, 255];
    const GREY: [u8; 4] = [200, 200, 200, 255];

    /// Read-back of `desk`'s own geometry, with the given pixels written at the points
    /// the row will sample.
    fn read_back(painted: &[(PhysPoint, [u8; 4])]) -> Frame {
        let mut got = Frame::filled(200, 200, [0, 0, 0, 0]).expect("frame");
        for (at, c) in painted {
            got.set(at.x as u32, at.y as u32, *c);
        }
        got
    }

    #[test]
    fn every_sample_the_row_reads_lands_in_exactly_one_bucket() {
        // The partition is the claim, and it is now checkable instead of implied: 31
        // hits, one of the other ink, one bare desktop, one nothing-can-name, one too
        // dark and one off this screen - 36 planted, and the eight counters add to it.
        // None of them is *partway*, and the term is still in the sum: an identity that
        // leaves a bucket out is a claim about seven of the eight, not about the partition.
        let snap = desk(GREY);
        let cover = PhysRect::new(0, 0, 200, 200);
        let mut points: Vec<PhysPoint> = (0..31).map(|i| PhysPoint::new(i, 0)).collect();
        points.push(PhysPoint::new(31, 0));
        points.push(PhysPoint::new(32, 0));
        points.push(PhysPoint::new(33, 0));
        points.push(PhysPoint::new(4, 195));
        points.push(PhysPoint::new(300, 300));
        let painted: Vec<(PhysPoint, [u8; 4])> = (0..31)
            .map(|i| (PhysPoint::new(i, 0), GREEN))
            .chain([
                (PhysPoint::new(31, 0), BLUE),
                (PhysPoint::new(32, 0), GREY),
                (PhysPoint::new(33, 0), ORPHAN),
                (PhysPoint::new(4, 195), GREY),
            ])
            .collect();
        let got = read_back(&painted);
        let h = band(cover, &got, &snap, &points, GREEN, true);
        assert_eq!(
            (
                h.want,
                h.ok,
                h.other_ink,
                h.plain,
                h.blended,
                h.unexplained,
                h.dark,
                h.off
            ),
            (36, 31, 1, 1, 0, 1, 1, 1)
        );
        assert_eq!(
            h.ok + h.other_ink + h.plain + h.blended + h.unexplained + h.dark + h.off + h.flat,
            h.want
        );
        // 36 planted, 2 refused, and the line's own numbers add up to the 34 it divides
        // by - which is what the P22 row could not show.
        assert_eq!(h.usable(), 34);
        assert_eq!(
            h.ok + h.other_ink + h.plain + h.blended + h.unexplained,
            h.usable()
        );
    }

    #[test]
    fn the_undone_stroke_counts_a_pixel_once_not_twice() {
        // The ghost row grades the *absence* of blue, so a pixel back to the desktop is
        // the expected reading. It used to be counted as both `plain` and `ok`, which
        // made the buckets overlap and the printed numbers add past the denominator.
        // The fourth pixel is `ODD` rather than `ORPHAN` because the row it is graded
        // against wants blue *absent*: ORPHAN leans toward the ink and would land in
        // `blended`, and this row's claim is about `plain` and `ok` only.
        let snap = desk(GREY);
        let cover = PhysRect::new(0, 0, 200, 200);
        let points = [
            PhysPoint::new(0, 0),
            PhysPoint::new(1, 0),
            PhysPoint::new(2, 0),
            PhysPoint::new(3, 0),
        ];
        let got = read_back(&[
            (PhysPoint::new(0, 0), BLUE),
            (PhysPoint::new(1, 0), GREEN),
            (PhysPoint::new(2, 0), GREY),
            (PhysPoint::new(3, 0), ODD),
        ]);
        let h = band(cover, &got, &snap, &points, BLUE, false);
        assert_eq!(
            (h.want, h.ok, h.other_ink, h.plain, h.blended, h.unexplained),
            (4, 1, 2, 0, 0, 1)
        );
    }

    #[test]
    fn a_sample_nobody_can_name_is_still_counted_and_still_a_miss() {
        // This is the 56. A whole band of them reads FAIL, exactly as it did when the
        // row subtracted them silently - the difference is only that the row now says
        // which 40, so a defect cannot hide in the arithmetic.
        let snap = desk(GREY);
        let cover = PhysRect::new(0, 0, 200, 200);
        let points: Vec<PhysPoint> = (0..40).map(|i| PhysPoint::new(i, 5)).collect();
        let painted: Vec<(PhysPoint, [u8; 4])> = points.iter().map(|p| (*p, ORPHAN)).collect();
        let got = read_back(&painted);
        let h = band(cover, &got, &snap, &points, GREEN, true);
        assert_eq!((h.want, h.ok, h.blended, h.unexplained), (40, 0, 0, 40));
        assert_eq!(h.usable(), 40);
        assert_eq!(h.verdict(INK_PASS), Check::Fail);
        assert!(h.line().contains("40 nobody can name"));
        assert!(h.line().contains("0 partway there"));
        // And the count comes with the pixels it counted: three of them is enough to
        // tell where to look, and the row prints them.
        assert_eq!(h.examples.len(), NAMED);
        assert!(h.line().contains("; first "));
    }

    /// 放大's geometry for the two rows below: a 40x40 source magnified into an 80x80
    /// copy, a hole that covers both, and a read-back the same size as the window.
    const FROM: PhysRect = PhysRect::new(0, 0, 40, 40);
    const COPY: PhysRect = PhysRect::new(60, 60, 80, 80);

    fn zoom_read(inside_copy: [u8; 4]) -> Frame {
        let mut got = Frame::filled(200, 200, GREY).expect("frame");
        got.fill_rect(&COPY, inside_copy);
        got
    }

    /// The row's own entry point: the expectation is the magnifier's output built from
    /// the same frozen desktop the product reads, so a test that means "the copy was
    /// made from somewhere else" says so by passing a different `from`.
    fn zoom_row(snap: &ScreenSnapshot, got: &Frame, from: PhysRect, to: PhysRect) -> (Hits, usize) {
        let mag = magnified(snap, &from, to.w, to.h).expect("magnifier output");
        enlarged(
            PhysRect::new(0, 0, 200, 200),
            got,
            snap,
            &mag,
            from,
            to,
            PhysRect::new(0, 0, 200, 200),
        )
    }

    /// A desktop whose copied region is **textured per pixel**, which is the only shape
    /// that exercises what P24 is about. The first fixture for these rows was one flat
    /// colour with a single hard edge, and it measured nothing: at every one of the 81
    /// lattice points the filter's answer and nearest's answer came out within
    /// [`ENLARGE_TOL`] of each other (`nearest_ok=81`), and a copy shifted four device
    /// pixels still passed 72 of 81. Structure at the scale of a pixel is what makes a
    /// resampling observable - which is also why the real run read 98.3% on a flat
    /// wallpaper and 73.1% on a textured one.
    fn textured_desk() -> ScreenSnapshot {
        let mut snap = desk(GREY);
        for y in 0..40u32 {
            for x in 0..40u32 {
                let i = (x * 7 + y * 13) % 4;
                let c = [
                    60 + (i * 55) as u8,
                    60 + ((3 - i) * 55) as u8,
                    60 + (((x + y) % 3) * 60) as u8,
                    255,
                ];
                snap.monitors[0].frame.set(x, y, c);
            }
        }
        snap
    }

    /// The read-back the product would produce for `mag` laid down at `to`: the window
    /// showing the desktop everywhere except the copy, and the copy's own pixels there.
    fn laid_down(mag: &Frame, to: PhysRect) -> Frame {
        let mut got = Frame::filled(200, 200, GREY).expect("frame");
        for y in 0..mag.height {
            for x in 0..mag.width {
                let (dx, dy) = (to.x + x as i32, to.y + y as i32);
                if dx >= 0 && dy >= 0 && (dx as u32) < got.width && (dy as u32) < got.height {
                    got.set(dx as u32, dy as u32, mag.get(x, y));
                }
            }
        }
        got
    }

    #[test]
    fn the_row_grades_the_copy_against_the_magnifier_and_not_against_nearest() {
        // The claim P24 makes, in both directions at once. The copy is laid down exactly
        // as `raster` lays it - the source cropped out of the frozen frame and resampled
        // with the filter - and the row calls every one of its samples the expected
        // reading. And the pass is load-bearing: nearest disagrees with the filter at
        // most of these points, so the row is not passing because the two answers match.
        let snap = textured_desk();
        let mag = magnified(&snap, &FROM, COPY.w, COPY.h).expect("magnifier output");
        let got = laid_down(&mag, COPY);
        let (h, nearest_ok) = zoom_row(&snap, &got, FROM, COPY);
        assert_eq!(h.ok, h.usable());
        assert_eq!(h.verdict(INK_PASS), Check::Pass);
        assert!(
            nearest_ok * 2 < h.usable(),
            "the fixture has no texture in it: nearest_ok={nearest_ok} usable={}",
            h.usable()
        );
    }

    #[test]
    fn a_copy_made_with_the_other_filter_is_not_called_the_copy() {
        // The teeth the row keeps, and the reason grading against the magnifier's own
        // frame is not just agreeing with the implementation: a copy made by nearest
        // neighbour - the same pixels, the same rectangle, the same place - is a
        // different picture at this scale, and this row says so. Changing the filter is
        // therefore a decision the plan has to take on purpose, not one that goes by
        // unnoticed.
        let snap = textured_desk();
        let nearest = snap.monitors[0]
            .frame
            .crop(&FROM)
            .expect("crop")
            .nearest_resized(COPY.w, COPY.h);
        let got = laid_down(&nearest, COPY);
        let (h, _) = zoom_row(&snap, &got, FROM, COPY);
        assert_eq!(h.usable(), 81);
        assert!(
            h.ok * 2 < h.usable(),
            "a nearest-neighbour copy read as the magnifier's own: {}/{}",
            h.ok,
            h.usable()
        );
        assert_eq!(h.verdict(INK_PASS), Check::Fail);
    }

    #[test]
    fn the_row_is_sensitive_to_which_rectangle_the_copy_was_made_from() {
        // Sensitivity is the other half that must survive the change. Same desktop, same
        // pixels on screen, but the expectation is built from a source rectangle one
        // device pixel away: at this texture that is a different picture, and the row
        // cannot grade its way through it.
        let snap = textured_desk();
        let mag = magnified(&snap, &FROM, COPY.w, COPY.h).expect("magnifier output");
        let got = laid_down(&mag, COPY);
        let (h, _) = zoom_row(&snap, &got, FROM.offset(1, 0), COPY);
        assert_eq!(h.want, 81);
        assert!(
            h.ok * 2 < h.usable(),
            "a copy made from the wrong rectangle still reads as a clean pass: {}/{}",
            h.ok,
            h.usable()
        );
        assert_eq!(h.verdict(INK_PASS), Check::Fail);
    }

    #[test]
    fn a_copy_laid_down_where_the_document_does_not_put_it_is_not_the_copy() {
        // The last axis: the picture is right, the place is wrong. The row asks about the
        // rectangle the document claims, so it reads the copy's neighbours there and
        // finds neither the magnifier's picture nor the desktop.
        let snap = textured_desk();
        let mag = magnified(&snap, &FROM, COPY.w, COPY.h).expect("magnifier output");
        let got = laid_down(&mag, COPY);
        let (h, _) = zoom_row(&snap, &got, FROM, COPY.offset(4, 0));
        assert_eq!(h.want, 81);
        assert!(
            h.ok * 2 < h.usable(),
            "a copy four device pixels off the document's rectangle still passed: {}/{}",
            h.ok,
            h.usable()
        );
        assert_eq!(h.verdict(INK_PASS), Check::Fail);
    }

    #[test]
    fn a_source_on_no_captured_monitor_gives_the_row_no_expectation() {
        // Not a refusal the desktop hands back and not a pass: there is no magnifier
        // output at all, so the row cannot ask its question and the caller says so.
        let snap = desk(GREY);
        assert!(magnified(&snap, &PhysRect::new(300, 300, 40, 40), COPY.w, COPY.h).is_none());
    }

    #[test]
    fn planted_is_what_the_script_planted_not_what_the_wallpaper_shows() {
        // 9 x 9 = 81 lattice points inside the copy either way. On a desktop that cannot
        // distinguish them - source and destination the same colour - the old code
        // subtracted them from `want` and printed `0 planted`, which is how the P22 run
        // read 485 on a bright wallpaper and 2,172 on a dark one for an unchanged
        // script. The refusals move out of `planted` and into a named bucket; `planted`
        // stops moving at all.
        let flat = desk(GREY);
        let (h, _) = zoom_row(&flat, &zoom_read(GREY), FROM, COPY);
        assert_eq!((h.want, h.flat, h.usable()), (81, 81, 0));
        assert_eq!(h.verdict(INK_PASS), Check::Blocked);
    }

    #[test]
    fn a_copy_that_never_landed_still_fails_on_the_same_sample_count() {
        // The other half of the same change: a distinguishable desktop gives the row the
        // identical denominator, and a copy showing the desktop instead of the source is
        // a FAIL with every one of its samples named as the bare desktop. The source here
        // is one flat colour, which is also why these four readings say the same thing
        // under either expectation - the filter has no edge to ring at. See
        // `the_row_grades_the_copy_against_the_magnifier_and_not_against_nearest` for the
        // fixture where the two answers part company.
        let mut snap = desk(GREY);
        snap.monitors[0].frame.fill_rect(&FROM, [250, 20, 20, 255]);
        let (h, _) = zoom_row(&snap, &zoom_read(GREY), FROM, COPY);
        assert_eq!(
            (h.want, h.flat, h.usable(), h.ok, h.plain),
            (81, 0, 81, 0, 81)
        );
        assert_eq!(h.verdict(INK_PASS), Check::Fail);

        // And the same row with the copy actually there reads as expected, so the
        // buckets above are not this row's way of always declining.
        let (shown, _) = zoom_row(&snap, &zoom_read([250, 20, 20, 255]), FROM, COPY);
        assert_eq!((shown.ok, shown.usable()), (81, 81));
        assert_eq!(shown.verdict(INK_PASS), Check::Pass);

        // The third reading is the one the P22 run produced 56 of: a copy that is
        // neither the source nor the desktop under it. It used to be arithmetic the
        // reader had to do; now it is a count, with three pixels printed to go and look
        // at. ORPHAN is the measured shape of those 56 - the read-back leans toward the
        // copy, at about three quarters coverage - so the row calls it *partway*, which
        // is a different question from the fourth reading below.
        let (h, _) = zoom_row(&snap, &zoom_read(ORPHAN), FROM, COPY);
        assert_eq!(
            (h.ok, h.plain, h.blended, h.unexplained, h.examples.len()),
            (0, 0, 81, 0, NAMED)
        );
        assert_eq!(h.usable(), 81);
        assert_eq!(h.verdict(INK_PASS), Check::Fail);

        // And a pixel that leans toward neither candidate stays in `unexplained`: the
        // two buckets are the difference P23 exists to make, so both have to be reachable.
        let (h, _) = zoom_row(&snap, &zoom_read(ODD), FROM, COPY);
        assert_eq!((h.blended, h.unexplained, h.usable()), (0, 81, 81));
        assert_eq!(h.verdict(INK_PASS), Check::Fail);
    }

    /// The strip 文本's rows are graded on: 20x10 device pixels of the test desk, so the
    /// area asked about is 200 and every bucket can be named in the assertion. A strip
    /// this gauge can hold in one hand, because the arithmetic being tested is the
    /// counting; the placement is the test below that, and is a separate claim.
    fn strip() -> PhysRect {
        PhysRect::new(10, 10, 20, 10)
    }

    /// A read-back of the whole test desk, with the first `n` pixels of `s` - in the
    /// order the row walks it - written in `c`.
    fn strip_read(s: PhysRect, c: [u8; 4], n: usize) -> Frame {
        let mut got = Frame::filled(200, 200, GREY).expect("frame");
        let mut painted = 0usize;
        for y in s.y..s.bottom() {
            for x in s.x..s.right() {
                if painted >= n {
                    return got;
                }
                got.set(x as u32, y as u32, c);
                painted += 1;
            }
        }
        got
    }

    #[test]
    fn the_row_counts_the_letters_and_names_the_space_between_them() {
        let snap = desk(GREY);
        let fp = footprint(
            PhysRect::new(0, 0, 200, 200),
            &strip_read(strip(), RED, 40),
            &snap,
            strip(),
        );
        assert_eq!(
            (fp.planted, fp.ink, fp.plain, fp.usable()),
            (200, 40, 160, 200)
        );
        assert_eq!(fp.first, Some(PhysPoint::new(10, 10)));
        assert_eq!(fp.verdict(), Check::Pass);
        // The printed line, in full, because the count is the deliverable: 40 is the
        // number this row exists to say out loud, and the buckets are what makes it
        // readable as *letters* rather than as "40 of something".
        assert_eq!(
            fp.line(),
            "40 inked px of 200 usable of 200 planted, 0 the other ink, 160 bare desktop, 0 nobody can name, 0 already this colour in the freeze, 0 too dark, 0 off this canvas, first ink at 10,10"
        );
    }

    #[test]
    fn a_box_that_never_reached_the_screen_is_a_fail_and_not_an_empty_pass() {
        // The control the count needs, and the defect class it is aimed at: a text object
        // that is filed in the document and painted into no pixels at all. Two hundred
        // plain desktop pixels is what that looks like from here, and it is also what a
        // strip that was never planted looks like - which is why [`plant`] types the
        // string through the same entry points a keyboard does and the row prints whose
        // ink it found.
        let snap = desk(GREY);
        let fp = footprint(
            PhysRect::new(0, 0, 200, 200),
            &Frame::filled(200, 200, GREY).expect("frame"),
            &snap,
            strip(),
        );
        assert_eq!((fp.ink, fp.plain, fp.usable()), (0, 200, 200));
        assert_eq!(fp.verdict(), Check::Fail);
        assert_eq!(fp.first, None);
    }

    #[test]
    fn the_floor_is_a_number_whose_teeth_the_row_can_be_shown() {
        // 23 and 24 of the same thing, one pixel apart: a count with no floor passes on
        // an accident, and a floor nobody tests is a number nobody read.
        let snap = desk(GREY);
        let cover = PhysRect::new(0, 0, 200, 200);
        let one_short = footprint(
            cover,
            &strip_read(strip(), RED, MIN_TEXT_INK - 1),
            &snap,
            strip(),
        );
        let on_it = footprint(
            cover,
            &strip_read(strip(), RED, MIN_TEXT_INK),
            &snap,
            strip(),
        );
        assert_eq!((one_short.ink, one_short.verdict()), (23, Check::Fail));
        assert_eq!((on_it.ink, on_it.verdict()), (24, Check::Pass));
    }

    #[test]
    fn neither_ink_can_satisfy_the_other_s_row() {
        // Both directions, because the third ink exists for exactly this claim. Letters
        // planted in the polyline's colour are not this pen's colour, and the row says
        // whose they are instead of calling them nothing.
        let snap = desk(GREY);
        let cover = PhysRect::new(0, 0, 200, 200);
        let green = footprint(cover, &strip_read(strip(), GREEN, 200), &snap, strip());
        assert_eq!((green.ink, green.other_ink, green.usable()), (0, 200, 200));
        assert_eq!(green.verdict(), Check::Fail);
        // And the way round: a band walked entirely through this stroke's ink, graded by a
        // row that wants green, reads not one expected sample.
        let points: Vec<PhysPoint> = (0..40).map(|i| PhysPoint::new(10 + i, 10)).collect();
        let hits = band(
            cover,
            &strip_read(strip(), RED, 40),
            &snap,
            &points,
            GREEN,
            true,
        );
        assert_eq!((hits.ok, hits.usable()), (0, 40));
    }

    #[test]
    fn a_wallpaper_that_is_already_this_colour_cannot_be_credited() {
        // The refusal [`enlarged`] calls `flat`, in the row that needs it most: red text
        // on a red desktop is a claim about the desktop, and a count that grew from 0 to
        // 200 because the wallpaper changed would be the P22 finding all over again.
        let snap = desk(RED);
        let fp = footprint(
            PhysRect::new(0, 0, 200, 200),
            &Frame::filled(200, 200, RED).expect("frame"),
            &snap,
            strip(),
        );
        assert_eq!((fp.ink, fp.preexisting, fp.usable()), (0, 200, 0));
        assert_eq!(fp.verdict(), Check::Blocked);
    }

    #[test]
    fn every_pixel_the_row_walks_lands_in_exactly_one_bucket() {
        // A strip that runs off the window the bitmap photographs, so `off` is reachable,
        // and one pixel of every other kind: the identity is the claim, and `plain` is
        // what is left when the named six are subtracted.
        let snap = desk(GREY);
        let mut got = Frame::filled(200, 200, GREY).expect("frame");
        got.set(195, 5, RED);
        got.set(196, 5, GREEN);
        got.set(197, 5, [10, 10, 10, 255]);
        got.set(198, 5, ORPHAN);
        let fp = footprint(
            PhysRect::new(0, 0, 200, 200),
            &got,
            &snap,
            PhysRect::new(195, 5, 10, 10),
        );
        assert_eq!(fp.planted, 100);
        assert_eq!(
            (fp.ink, fp.other_ink, fp.dark, fp.unexplained),
            (1, 1, 1, 1)
        );
        assert_eq!(fp.off, 50);
        assert_eq!(fp.plain, 46);
        assert_eq!(
            fp.ink + fp.other_ink + fp.plain + fp.unexplained + fp.dark + fp.off + fp.preexisting,
            fp.planted
        );
    }

    #[test]
    fn the_strip_is_laid_where_the_selection_is_free_and_refused_where_it_is_not() {
        // The band this gauge actually walks, in the selection the P26 run was handed:
        // inside the free band, clear of the polyline's reach below and of the box's
        // right border to the left.
        let s = text_strip(PhysRect::new(768, 480, 1536, 960)).expect("strip");
        assert_eq!(s, PhysRect::new(1452, 904, TEXT_W, TEXT_H));
        assert!(s.y >= 480 + 408 && s.bottom() <= 480 + 480 - CLEAR);
        assert!(s.x > 768 + 614 + CLEAR);
        // The two refusals. Neither is reachable on this machine, and a branch no run
        // walks is a branch that has to be walked here or not at all (§9.1 第十条): half
        // the height has no room for one line between the bands, and a third of the width
        // has none between the strip and the selection's own edge.
        assert_eq!(text_strip(PhysRect::new(0, 0, 1536, 480)), None);
        assert_eq!(text_strip(PhysRect::new(0, 0, 400, 960)), None);
    }

    /// 橡皮's band on the selection the real runs are handed.
    fn band_on(hole: PhysRect) -> Erase {
        match erase_band(hole) {
            Ok(e) => e,
            Err(why) => panic!("no band on {hole:?}: {why}"),
        }
    }

    #[test]
    fn the_erase_band_keeps_each_zone_inside_the_run_that_erased_it_and_out_of_the_next() {
        let hole = PhysRect::new(768, 480, 1536, 960);
        let e = band_on(hole);
        // One line, inside the selection, and long enough on all three zones that no row
        // has to fall back on `Blocked` for want of pixels on a 1536x960 crop.
        for z in &e.zones {
            assert!(!z.is_empty());
            assert!(z.iter().all(|p| p.y == e.ink.0.y && hole.contains(*p)));
            assert!(
                z.len() >= MIN_USABLE,
                "a zone of {} samples cannot be graded",
                z.len()
            );
        }
        // The control has to be out of the first drag's reach by more than one step, or
        // the row that says "the ink is still there" is reading a pixel the eraser was
        // allowed to eat - and then the two modes agree for the wrong reason.
        let gap = e.runs[0].0.x - e.zones[0].last().unwrap().x;
        assert!(
            gap > ERASE_REACH + BAND_STEP,
            "the first drag comes within {gap} px of the control's last sample"
        );
        // Each erased zone sits inside its own run by `CLEAR`, so the drag's endpoints -
        // where a round brush covers the most ground - are not what the row counts.
        for (run, zone) in e.runs.iter().zip(&e.zones[1..]) {
            assert!(zone.first().unwrap().x >= run.0.x + CLEAR);
            assert!(zone.last().unwrap().x <= run.1.x - CLEAR);
        }
        // ...and out of its neighbour's, so one stroke cannot move two readings.
        let gap = e.zones[2].first().unwrap().x - e.runs[0].1.x;
        assert!(
            gap > ERASE_REACH + BAND_STEP,
            "the second drag reaches back {gap} px into the third zone"
        );
    }

    #[test]
    fn a_selection_too_flat_or_too_narrow_refuses_the_band_and_says_which() {
        // Neither arm is reachable on this machine's 1536x960 selection (58 px of slice
        // against the 24 the brush needs, 389 px of third against the 72 a zone needs), so
        // they are walked here or never (§9.1 第十条). They are two different questions and
        // each sentence names the measurement that failed: the slice the selection's edge
        // and the box leave the brush, and the width three zones need once each run keeps
        // its reach out of its neighbour.
        let flat = erase_band(PhysRect::new(0, 0, 1536, 320)).err().unwrap();
        let thin = erase_band(PhysRect::new(0, 0, 260, 960)).err().unwrap();
        assert!(
            flat.contains("the slice between the selection's top edge"),
            "{flat}"
        );
        assert!(thin.contains("each third of the run"), "{thin}");
        assert_ne!(flat, thin);
    }

    #[test]
    fn the_two_modes_differ_only_in_the_export_and_the_rows_read_the_export() {
        let snap = desk_sized(800, 200, GREY);
        let canvas = PhysRect::new(0, 0, 800, 200);
        let at: Vec<PhysPoint> = along(PhysPoint::new(0, 50), PhysPoint::new(360, 50));
        let n = at.len();

        // The three pictures the export can be, over one grey freeze: ink left alone,
        // the screenshot handed back, and the pixel taken out of the picture with the
        // ink's rgb still on it.
        let paint = |c: [u8; 4]| {
            let mut f = Frame::filled(800, 200, GREY).expect("frame");
            for p in &at {
                f.set(p.x as u32, p.y as u32, c);
            }
            f
        };
        let kept = paint(GREEN);
        let restored = Frame::filled(800, 200, GREY).expect("frame");
        let cleared = paint([0, 232, 0, 0]);

        assert_eq!(
            erase_read(Zone::Ink, &at, canvas, &snap, &kept).verdict(Zone::Ink),
            Check::Pass
        );
        let off = erase_read(Zone::Base, &at, canvas, &snap, &restored);
        assert_eq!((off.desk, off.usable()), (n, n));
        assert_eq!(off.verdict(Zone::Base), Check::Pass);
        let on = erase_read(Zone::Void, &at, canvas, &snap, &cleared);
        assert_eq!((on.void, on.usable()), (n, n));
        assert_eq!(on.verdict(Zone::Void), Check::Pass);

        // Each mode graded by the other's row: a 擦除到透明 pixel read as 只擦标注 is the
        // defect this leg exists to catch - both of them show the desktop on the mask, and
        // nothing but the alpha tells them apart. A control the eraser ate is a Fail too,
        // never a Pass handed out by the two thirds that did get erased.
        assert_eq!(
            erase_read(Zone::Base, &at, canvas, &snap, &cleared).verdict(Zone::Base),
            Check::Fail
        );
        assert_eq!(
            erase_read(Zone::Void, &at, canvas, &snap, &restored).verdict(Zone::Void),
            Check::Fail
        );
        assert_eq!(
            erase_read(Zone::Ink, &at, canvas, &snap, &restored).verdict(Zone::Ink),
            Check::Fail
        );
    }

    #[test]
    fn every_erase_sample_lands_in_one_bucket_and_a_desktop_that_already_wore_the_ink_is_a_refusal()
    {
        let mut snap = desk_sized(800, 200, GREY);
        // One pixel of the freeze is already this pen's green. There, "the ink is still
        // there" and "the ink was erased" are the same reading, so the row may not claim
        // either and the sample is named as a refusal rather than counted as a miss.
        snap.monitors[0].frame.set(360, 50, GREEN);
        let canvas = PhysRect::new(0, 0, 800, 200);
        let run = along(PhysPoint::new(0, 50), PhysPoint::new(360, 50));
        let mut at = run.clone();
        at.push(PhysPoint::new(900, 50)); // a point on no screen at all
        let mut export = Frame::filled(800, 200, GREY).expect("frame");
        for (i, p) in at.iter().enumerate().take(45) {
            let c = match i {
                0..=29 => GREEN,
                30..=39 => GREY,
                40..=42 => [0, 232, 0, 0],
                _ => ORPHAN,
            };
            export.set(p.x as u32, p.y as u32, c);
        }
        let h = erase_read(Zone::Ink, &at, canvas, &snap, &export);
        assert_eq!(
            (
                h.want,
                h.ink,
                h.desk,
                h.void,
                h.neither,
                h.uncredited,
                h.off
            ),
            (47, 30, 10, 3, 2, 1, 1)
        );
        assert_eq!(
            h.want,
            h.ink + h.desk + h.void + h.neither + h.uncredited + h.off
        );
        assert_eq!(h.usable(), 45);
        assert_eq!(h.verdict(Zone::Ink), Check::Fail);
        assert_eq!(
            h.examples.len(),
            NAMED,
            "the witnesses are the row's evidence"
        );

        // The refusal taken all the way: a wallpaper in this pen's green gives the colour
        // rows nothing, and `Blocked` is the honest answer - not the Pass that counting
        // the ink alone would have handed out on a desktop that never needed any.
        let wall = desk_sized(800, 200, GREEN);
        let all = erase_read(Zone::Ink, &at, canvas, &wall, &paint_green(&run));
        assert_eq!((all.uncredited, all.off), (46, 1));
        assert_eq!(all.usable(), 0);
        assert_eq!(all.verdict(Zone::Ink), Check::Blocked);
        // 擦除到透明 is the one claim no wallpaper can refuse: an alpha of 0 is not a
        // thing a desktop leaves behind by accident, so this zone needs no floor.
        let void_on_green = erase_read(Zone::Void, &at, canvas, &wall, &cleared(&run));
        assert_eq!(void_on_green.uncredited, 0);
        assert_eq!(void_on_green.verdict(Zone::Void), Check::Pass);
    }

    fn paint_green(at: &[PhysPoint]) -> Frame {
        let mut f = Frame::filled(800, 200, GREEN).expect("frame");
        for p in at {
            f.set(p.x as u32, p.y as u32, GREEN);
        }
        f
    }

    fn cleared(at: &[PhysPoint]) -> Frame {
        let mut f = Frame::filled(800, 200, GREY).expect("frame");
        for p in at {
            f.set(p.x as u32, p.y as u32, [0, 232, 0, 0]);
        }
        f
    }

    #[test]
    fn the_alpha_the_layer_row_steps_over_is_graded_in_both_directions() {
        let canvas = PhysRect::new(0, 0, 800, 200);
        let at: Vec<PhysPoint> = along(PhysPoint::new(0, 50), PhysPoint::new(360, 50));
        let n = at.len();
        let export = cleared(&at);
        let layer = cleared(&at);

        // The claim, first: the incremental canvas and a whole-canvas render of the same
        // document agree byte for byte, alpha 0 included.
        let both = sync_read(&at, canvas, &layer, &export);
        assert_eq!((both.ok, both.usable()), (n, n));
        assert_eq!(both.verdict(), Check::Pass);

        // One way to be wrong: the canvas still holds an opaque pixel where the export has
        // none. That is a smear on the mask the saved file does not have.
        let mut kept = layer.clone();
        for p in at.iter().take(3) {
            kept.set(p.x as u32, p.y as u32, GREEN);
        }
        let smear = sync_read(&at, canvas, &kept, &export);
        assert_eq!(smear.verdict(), Check::Fail);
        assert_eq!(smear.bad.len(), NAMED);

        // The other way, and this is the one `layer == export` cannot see: the canvas
        // dropped a pixel the export kept, so its own alpha is 0 - and that row steps over
        // every pixel whose alpha is 0, by construction. The control below is that row's
        // own walk over this zone: it compares none of the samples and reports nothing,
        // while this one names three.
        let mut dropped = layer.clone();
        for p in at.iter().take(3) {
            dropped.set(p.x as u32, p.y as u32, [0, 0, 0, 0]);
        }
        let lost = sync_read(&at, canvas, &dropped, &export);
        assert_eq!(lost.verdict(), Check::Fail);
        let (compared, mismatched) = at.iter().fold((0usize, 0usize), |(c, m), p| {
            let a = dropped.get(p.x as u32, p.y as u32);
            if a[3] == 0 {
                (c, m)
            } else {
                (
                    c + 1,
                    if a != export.get(p.x as u32, p.y as u32) {
                        m + 1
                    } else {
                        m
                    },
                )
            }
        });
        assert_eq!(
            (compared, mismatched),
            (0, 0),
            "the row this one replaces sees no disagreement here"
        );
    }

    #[test]
    fn a_sync_row_whose_pixels_are_on_another_screen_declines() {
        // An empty partition is not a pass: two frames of one grey and zero samples in
        // common would otherwise agree by having nothing to say.
        let layer = Frame::filled(200, 200, GREY).expect("frame");
        let export = Frame::filled(200, 200, GREY).expect("frame");
        let at: Vec<PhysPoint> = (0..40).map(|i| PhysPoint::new(1000 + i * 8, 50)).collect();
        let s = sync_read(&at, PhysRect::new(0, 0, 200, 200), &layer, &export);
        assert_eq!((s.want, s.off, s.ok), (at.len(), at.len(), 0));
        assert_eq!(s.usable(), 0);
        assert_eq!(s.verdict(), Check::Blocked);
    }

    #[test]
    fn the_erase_rows_name_every_bucket_between_the_planted_and_the_graded() {
        // Both fractions, in the same order the counters are kept in, or the lines cannot
        // be added up by the reader who has to decide whether a run measured the ink or
        // the desktop. `Synced` gets the same treatment: the number it skipped is the
        // number that makes its `ok` mean something.
        let h = Erased {
            want: 47,
            ink: 30,
            desk: 10,
            void: 3,
            neither: 2,
            uncredited: 1,
            off: 1,
            examples: vec![
                "16,50 export=[255, 0, 255, 255] over desktop=[200, 200, 200, 255]".to_string(),
            ],
        };
        assert_eq!(
            h.line(Zone::Ink),
            "30/45 as the unerased third (ink expected) of 47 planted, 30 this ink, 10 the frozen frame, \
             3 transparent, 2 neither, 1 the desktop is already this ink, 1 off this canvas; \
             first 16,50 export=[255, 0, 255, 255] over desktop=[200, 200, 200, 255]"
        );
        assert_eq!(
            Synced {
                want: 46,
                ok: 43,
                off: 3,
                bad: vec!["16,50: layer=[0, 232, 0, 255] export=[0, 232, 0, 0]".to_string()],
            }
            .line(),
            "43/43 of 46 planted pixels agree, 3 off this canvas, first mismatch: \
             16,50: layer=[0, 232, 0, 255] export=[0, 232, 0, 0]"
        );
    }

    /// One screen's contribution to a delivery, cut and pasted the way
    /// [`crate::annotate::Layer::deliver`] does it, so the tests below plant a stitch
    /// rather than a whole flow.
    fn stitch(export: &Frame, part: PhysRect, hole: PhysRect, canvas: PhysRect, dx: i32) -> Frame {
        let cut = export
            .crop(&PhysRect::new(
                part.x - canvas.x,
                part.y - canvas.y,
                part.w,
                part.h,
            ))
            .expect("crop this screen's share");
        let mut out = Frame::new(hole.w, hole.h).expect("frame");
        out.paste(&cut, PhysPoint::new(part.x - hole.x + dx, part.y - hole.y))
            .expect("paste it at its own corner");
        out
    }

    #[test]
    fn a_stitch_one_pixel_out_of_place_is_invisible_to_every_row_above_it() {
        let (hole, canvas) = (
            PhysRect::new(20, 30, 100, 60),
            PhysRect::new(0, 0, 200, 200),
        );
        let part = hole
            .intersection(&canvas)
            .expect("the hole is on this canvas");
        let mut export = Frame::filled(200, 200, GREY).expect("frame");
        export.fill_rect(&PhysRect::new(40, 50, 20, 10), GREEN);

        let right = stitch(&export, part, hole, canvas, 0);
        let s = stitched(&right, &export, part, hole, canvas);
        assert_eq!(
            (s.ok, s.usable(), s.want),
            (
                part.area() as usize,
                part.area() as usize,
                part.area() as usize
            ),
            "the row's own arithmetic: {} added up to {}",
            s.line(),
            part.area()
        );
        assert_eq!(s.verdict(), Check::Pass);

        // One pixel of the wrong space, which is what a `shift` applied twice - or not at
        // all - leaves behind, and what P22's clip bug looked like from the outside: a
        // correct render shipped to the wrong place.
        let off_by_one = stitch(&export, part, hole, canvas, 1);
        let bad = stitched(&off_by_one, &export, part, hole, canvas);
        assert_eq!(bad.verdict(), Check::Fail);
        assert_eq!(bad.bad.len(), NAMED, "{}", bad.line());

        // The control, and the reason this row exists: the canvas and a whole-canvas render
        // of the *same document* still agree byte for byte, because the defect is in the
        // frame that leaves the process and `layer == export` never looks at that one.
        let at: Vec<PhysPoint> = (0..40).map(|i| PhysPoint::new(20 + i * 2, 55)).collect();
        assert_eq!(
            sync_read(&at, canvas, &export, &export).verdict(),
            Check::Pass,
            "a row that compares two renders of one canvas cannot see a wrong stitch"
        );

        // And a shipped frame that reaches almost none of the share it is compared with is
        // a refusal, not the agreement of the one pixel it does reach: `usable` is where
        // that is decided, before any count of agreements gets a chance to look like a
        // pass.
        let stub = Frame::new(1, 1).expect("frame");
        let none = stitched(&stub, &export, part, hole, canvas);
        assert_eq!(
            (none.off, none.usable(), none.verdict()),
            (part.area() as usize - 1, 1, Check::Blocked),
            "{}",
            none.line()
        );
    }

    #[test]
    fn the_picture_on_the_screen_is_graded_against_the_delivery_and_not_against_itself() {
        let snap = desk_sized(400, 400, GREY);
        let (cover, hole) = (
            PhysRect::new(0, 0, 400, 400),
            PhysRect::new(50, 50, 300, 300),
        );
        let grey = Frame::filled(300, 300, GREY).expect("frame");
        let green = Frame::filled(300, 300, GREEN).expect("frame");
        let desktop = Frame::filled(400, 400, GREY).expect("frame");
        let ink = |seen: &Frame| shipped_on_screen(cover, seen, &green, &snap, &hole, hole, 1.0);

        // Delivered: the window shows the picture the product says it will ship.
        let mut shown = desktop.clone();
        shown.fill_rect(&hole, GREEN);
        let h = ink(&shown);
        assert!(h.usable() >= MIN_USABLE, "{}", h.line());
        assert_eq!(h.verdict(SHIP_PASS), Check::Pass);

        // The overlay painted nothing, so the screen is the desktop and every sample this
        // row can grade is a sample of the ink missing.
        let h = ink(&desktop);
        assert_eq!((h.ok, h.verdict(SHIP_PASS)), (0, Check::Fail));
        assert!(h.plain >= MIN_USABLE, "{}", h.line());

        // Both candidates the same colour, which is most of a real selection: refused, and
        // a refusal that leaves nothing to grade is not a pass. Without this rung the row
        // would read 2000/2000 on a desktop whose mask had never painted.
        let h = shipped_on_screen(cover, &desktop, &grey, &snap, &hole, hole, 1.0);
        assert_eq!(
            (h.flat, h.usable(), h.verdict(SHIP_PASS)),
            (h.want, 0, Check::Blocked)
        );

        // A black expectation divides nothing, whatever the screen shows.
        let black = Frame::filled(300, 300, [4, 4, 4, 255]).expect("frame");
        let h = shipped_on_screen(cover, &desktop, &black, &snap, &hole, hole, 1.0);
        assert!(h.dark > 0 && h.usable() == 0, "{}", h.line());
        assert_eq!(h.verdict(SHIP_PASS), Check::Blocked);

        // Partway to the ink is still a miss, and leans a named way; a pixel that is
        // neither candidate is a different finding and cannot borrow the first one's pass.
        let mut near = desktop.clone();
        near.fill_rect(&hole, [50, 225, 50, 255]);
        let h = ink(&near);
        assert_eq!(
            (h.ok, h.blended > 0, h.verdict(SHIP_PASS)),
            (0, true, Check::Fail)
        );
        let mut orphan = desktop.clone();
        orphan.fill_rect(&hole, ORPHAN);
        let h = ink(&orphan);
        assert!(h.unexplained >= MIN_USABLE, "{}", h.line());
        assert_eq!(h.verdict(SHIP_PASS), Check::Fail);

        // A window on a piece of desktop this run never froze answers nothing.
        let far = PhysRect::new(450, 50, 300, 300);
        let h = shipped_on_screen(
            PhysRect::new(400, 0, 400, 400),
            &desktop,
            &green,
            &snap,
            &far,
            far,
            1.0,
        );
        assert_eq!(
            (h.off, h.usable(), h.verdict(SHIP_PASS)),
            (h.want, 0, Check::Blocked)
        );
    }

    /// A pixel the 擦除到透明 档 erased reaches the delivery as the ink's colour under an
    /// alpha of 0, and the screen has nothing to show but what is underneath. An RGB
    /// comparison there is not a failed grade - it is a question that has no answer - and
    /// the first live walk of [`shipped_on_screen`] proved that by losing exactly its
    /// whole 橡皮 band to `bare desktop`. Refused, and refused in a way that cannot be
    /// used to hide the pixels next to it.
    #[test]
    fn a_transparent_pixel_of_the_delivery_is_refused_and_never_blamed() {
        let snap = desk_sized(400, 400, GREY);
        let hole = PhysRect::new(50, 50, 300, 300);
        let cover = PhysRect::new(0, 0, 400, 400);
        let desktop = Frame::filled(400, 400, GREY).expect("frame");
        let green = Frame::filled(300, 300, GREEN).expect("frame");
        let erased_pixel = [GREEN[0], GREEN[1], GREEN[2], 0u8];
        let erased = Frame::filled(300, 300, erased_pixel).expect("frame");

        // Nothing but refusals: the band is the whole picture here, and no bucket that
        // costs the row a point gets a sample.
        let h = shipped_on_screen(cover, &desktop, &erased, &snap, &hole, hole, 1.0);
        assert_eq!(
            (h.plain, h.blended, h.unexplained, h.usable()),
            (0, 0, 0, 0),
            "{}",
            h.line()
        );
        assert_eq!((h.flat, h.verdict(SHIP_PASS)), (h.want, Check::Blocked));

        // And the refusal cannot eat a pixel it has no business refusing: half of the
        // document erased, half still ink, and the inked half is graded on its own merits
        // rather than carried by the refusal next to it.
        let mut mixed = green.clone();
        mixed.fill_rect(&PhysRect::new(0, 0, 150, 300), erased_pixel);
        let mut half = desktop.clone();
        half.fill_rect(&PhysRect::new(200, 50, 150, 300), GREEN);
        let h = shipped_on_screen(cover, &half, &mixed, &snap, &hole, hole, 1.0);
        assert!(h.usable() >= MIN_USABLE, "{}", h.line());
        assert_eq!(
            (
                h.plain,
                h.unexplained,
                h.ok == h.usable(),
                h.verdict(SHIP_PASS)
            ),
            (0, 0, true, Check::Pass),
            "{}",
            h.line()
        );
    }
}
