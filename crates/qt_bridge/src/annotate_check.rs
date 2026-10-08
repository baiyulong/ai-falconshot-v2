//! The ink, checked against the pixels it is supposed to be covering.
//!
//! `--ink <ms>` is `--mask` plus a script: the mask opens on a real freeze, this
//! module drives *the same entry points a pointer uses* through four strokes and an
//! undo, and the run ends with one `PrintWindow` read-back of each mask window. That
//! single read-back is why the script is arranged the way it is:
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
//!
//! The colour tests are relational (one channel must lead the other two) rather than
//! exact, because composition and antialiasing are allowed to round a pixel; the ghost
//! test is exact against the freeze, because there the two bitmaps are the same
//! picture. Sampling positions come from [`Plan`] - the same record that produced the
//! strokes - so an expectation can never drift away from what was drawn.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

use falcon_core::annotation::Kind;
use falcon_core::capture::ScreenSnapshot;
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

/// How far apart the samples of a stroke's band sit, in device pixels.
const BAND_STEP: i32 = 8;

/// What share of a stroke's samples have to read as expected. Antialiasing at the
/// two ends of a run, and a window whose first frame pre-dates the last publish, are
/// the honest sources of a near miss; a stroke that is absent reads far below this.
const INK_PASS: usize = 8;
const GHOST_PASS: usize = 9;

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
fn stroke(
    m: &mut mask::MaskState,
    slots: &[mask::Slot],
    at: &[PhysPoint],
    notes: &mut Vec<String>,
) {
    let Some(first) = at.first() else { return };
    let Some((index, x, y)) = local(slots, *first) else {
        notes.push(format!("no screen under {:?}", first));
        return;
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
    m.release();
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

/// Freeze, open, draw, undo. Must run before the QML engine, for the same reason
/// [`mask_check::open`] must: the windows must not capture the screen they cover.
pub fn plant(shader: bool) -> Result<String, String> {
    mask_check::open(shader)?;
    let slots = mask::with(|m| m.slots.clone());
    let mut notes = Vec::new();

    // Every region is a fraction of the selection, so the script needs no knowledge
    // of the monitor's size or of the default hole's arithmetic. The polyline's strip
    // (y 0.48..0.60) is between the box and the arrow on purpose: the three rows that
    // look for green must not be able to satisfy each other, and the arrow's band -
    // the one graded for absence - has to stay clear of ink that is still meant to be
    // there.
    let (rect, zig, poly, arrow) = mask::with(|m| {
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
        )
    });

    let mut keys = Vec::new();
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
    *plan().lock().unwrap_or_else(PoisonError::into_inner) = Some(Plan {
        rect,
        zig,
        poly,
        arrow,
        keys: keys.clone(),
        line: line.clone(),
        hole,
        notes,
    });
    Ok(format!(
        "4 strokes + 1 undo, {} publish(es) with {} distinct key(s), hole={hole}, {line}{}",
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

/// One stroke's band, as it read. The counters are kept apart rather than collapsed
/// into a percentage, because "the stroke is not there" and "the stroke is in the
/// other box's place" and "the desktop is too dark to divide" are three different
/// findings that a single number would hide.
struct Hits {
    want: usize,
    ok: usize,
    other_ink: usize,
    plain: usize,
    dark: usize,
    off: usize,
}

impl Hits {
    /// `pass` as a tenths-of-a-whole threshold, so the row can be graded on a sample
    /// set whose size the screen decides.
    fn verdict(&self, tenths: usize) -> Check {
        if self.want == 0 {
            return Check::Blocked;
        }
        Check::from(self.ok * 10 >= self.want * tenths)
    }

    fn line(&self) -> String {
        format!(
            "{}/{} as expected, {} the other ink, {} bare desktop, {} too dark, {} off this screen",
            self.ok, self.want, self.other_ink, self.plain, self.dark, self.off
        )
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
            h.plain += 1;
            if !present {
                h.ok += 1;
            }
        }
    }
    h
}

/// The check itself, after the event loop has painted.
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
        rep.row(
            &format!("layer == export of {tag}"),
            Check::from(checked > 0 && bad.is_empty()),
            format!(
                "{checked} opaque px compared, {bare} transparent (not the overlay's business), first mismatch: {}",
                if bad.is_empty() {
                    "none".to_string()
                } else {
                    bad.join(" | ")
                }
            ),
        );

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
        mask_check::flag_column(&mut rep, win);
        rep.note(
            &format!("swaps of {tag}"),
            format!("{} frame(s), first at {:?}", slot.swaps, slot.first_swap_ms),
        );
    }

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
    rep.finish("ink check")
}
