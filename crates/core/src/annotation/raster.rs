//! Rasterising an element list into pixels — the overlay painter behind M4a.
//!
//! 计划 M4a fixes the shape of this: a *Rust* dirty-rect rasteriser feeding a
//! `QQuickImageProvider`, so what the user sees while dragging and what
//! [`crate::encode`] writes to disk are the same bytes. Nothing here talks to Qt.
//!
//! Three rules run through the module:
//!
//! * Effects read the **base** picture, never the half-painted overlay. Two
//!   mosaics stacked do not mosaic each other, and undoing the lower one cannot
//!   leave a stale patch behind.
//! * Coverage is geometry and colour is separate. Each element becomes one plane
//!   of 0..=255 weights plus the colour that weight carries, and the transform is
//!   applied when it lands. That is what lets a feathered brush, a dash pattern
//!   and a semi-transparent colour be the same element, what lets §5.7.11 tint one
//!   glyph three ways (fill, 描边, 背景), and what lets §5.7.15 rotate 文本、矩形、
//!   椭圆、编号 and 放大区域 through a single path.
//! * Overlapping parts of *one* pen merge by taking the higher coverage, not by
//!   adding. Two capsules meeting at a pen joint would otherwise stack their
//!   alpha and leave a dark bead every few pixels on a semi-transparent marker.
//!   A *second* ink on the same element — 文本's fill over its 背景, 放大区域's
//!   frame over the enlarged copy — composites on top instead, because coverage
//!   is the rule for one stroke and not a licence to refuse to draw. See
//!   [`Plane`].

use super::model::{
    head_reach, Align, ArrowHead, Dash, Document, Element, Geom, Kind, Style, Transform,
};
use crate::frame::Frame;
use crate::frame::FrameError;
use crate::geometry::PhysRect;
use crate::imageops;
use std::f64::consts::TAU;

/// One straight stroke: a start and an end point, in picture coordinates.
type Run = ((f64, f64), (f64, f64));

/// A run of glyph coverage: one byte per pixel, 255 for fully inked.
///
/// Coverage rather than colour, because §5.7.11 sets the glyph colour, the
/// background and a stroke independently. Line advance is the block's own height,
/// so a platform that wants leading bakes it into `height` instead of asking this
/// module to invent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ink {
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
}

impl Ink {
    pub fn at(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.coverage[y as usize * self.width as usize + x as usize]
    }

    /// A solid block of the given size — enough to lay out and test against
    /// without a font.
    pub fn solid(width: u32, height: u32) -> Self {
        Ink {
            width,
            height,
            coverage: vec![255; (width as usize) * (height as usize)],
        }
    }
}

/// Whoever owns the font on this platform.
///
/// Core has no font rasteriser: `image` does not do text, and the family in
/// [`Style::font_family`] is a system font that must not be redistributed inside
/// an MSIX package. The Windows layer answers this from DirectWrite; [`NoGlyphs`]
/// is what a machine with no font story — and every test here — uses. Text with no
/// font still gets its background box, so layout, undo and export stay testable.
pub trait Glyphs {
    fn ink(&self, line: &str, style: &Style) -> Option<Ink>;
}

/// The font-free answer: nothing measures, nothing draws.
pub struct NoGlyphs;

impl Glyphs for NoGlyphs {
    fn ink(&self, _line: &str, _style: &Style) -> Option<Ink> {
        None
    }
}

/// The box `text` needs at `style`: the widest of its lines, and all of their height.
///
/// §5.7.11 step 2 places a text box from *one click*, so whoever holds that click has
/// to know how big the box will be before anything is laid out - and the only answer
/// that cannot disagree with the pixels is the one from the same leg that draws them.
/// A box measured somewhere else is a box the letters either overflow or float inside.
///
/// This is the no-wrap measure: [`layout`] wraps a paragraph down to the box, and the
/// box this returns is exactly as wide as the widest paragraph, so `wrap` can find no
/// split in it - re-joining a paragraph's words can only shorten what it measures, never
/// lengthen it. A caller that wants a *narrower* box - the edge of the selection, a
/// fixed column - has to lay the text out and measure what came back, which is a
/// different question and not answered here.
///
/// `None` when any line cannot be measured. Half a box is worse than none: it clips the
/// lines it did measure, and a caller that cannot ask the font has to guess its own.
pub fn text_extent(text: &str, style: &Style, glyphs: &dyn Glyphs) -> Option<(u32, u32)> {
    let mut width = 0u32;
    let mut height = 0u32;
    for line in text.split('\n') {
        let ink = glyphs.ink(line, style)?;
        width = width.max(ink.width);
        height = height.saturating_add(ink.height);
    }
    Some((width.max(1), height.max(1)))
}

/// Flatten the whole picture: the base, then every visible element in painting
/// order. Export and the history thumbnail call this.
pub fn render(base: &Frame, doc: &Document, glyphs: &dyn Glyphs) -> Result<Frame, FrameError> {
    let mut out = base.clone();
    paint(base, doc, &mut out, &base.bounds(), glyphs)?;
    Ok(out)
}

/// Repaint `area` of `out` in place.
///
/// The caller keeps one overlay canvas per session and uploads only what moved,
/// which is the point of the dirty rects coming back from
/// [`Command::apply`](super::Command::apply). Pixels outside `area` are left
/// exactly as they were, so `out` may be the composite of an earlier pass rather
/// than a fresh copy of `base`.
///
/// `area` itself is first put back to `base`. Without that reset the pass is only
/// additive, and an element that has been deleted, moved or rotated out of the
/// rect paints nothing there — leaving the pixels it drew last time as a ghost
/// over the screenshot. Every repaint is therefore *the picture in this rect, plus
/// whatever elements still touch it*, which is the same thing [`render`] does to
/// the whole frame.
pub fn paint(
    base: &Frame,
    doc: &Document,
    out: &mut Frame,
    area: &PhysRect,
    glyphs: &dyn Glyphs,
) -> Result<(), FrameError> {
    let Some(area) = area
        .intersection(&out.bounds())
        .and_then(|a| a.intersection(&base.bounds()))
    else {
        return Ok(());
    };
    for y in 0..area.h {
        for x in 0..area.w {
            let (px, py) = ((area.x + x as i32) as u32, (area.y + y as i32) as u32);
            out.set(px, py, base.get(px, py));
        }
    }
    for e in doc.paint_order() {
        if e.visible {
            paint_one(base, out, e, &area, glyphs)?;
        }
    }
    Ok(())
}

fn paint_one(
    base: &Frame,
    out: &mut Frame,
    e: &Element,
    area: &PhysRect,
    glyphs: &dyn Glyphs,
) -> Result<(), FrameError> {
    // The unturned box, whose centre is the pivot both the model's footprint and
    // the landing below work about.
    let pen = e.pen_box();
    let Some(local) = pen.intersection(&base.bounds()) else {
        return Ok(());
    };
    if local.is_empty() {
        return Ok(());
    }
    // Where the transform can move the layer to, tested before drawing anything.
    let reach = laid(&local, &pen, &e.transform);
    let Some(hit) = reach.intersection(area) else {
        return Ok(());
    };
    let mut plane = Plane::new(&local);
    draw(base, e, &local, &mut plane, glyphs)?;
    if plane.is_empty() {
        return Ok(());
    }
    blit(out, &plane, &local, &pen, &e.transform, hit, e)
}

/// One element's layer over its own box.
///
/// `cov` is the layer's alpha with the ink's own alpha already folded in; `rgba`
/// is the straight colour sitting under it. Two writers share a plane:
///
/// * [`Plane::put`] is the one-pen rule — overlapping capsules of the *same*
///   stroke take the higher weight instead of adding, which is what stops a
///   semi-transparent marker from growing a dark bead at every pen joint.
/// * [`Plane::over`] is for the second ink of one element: §5.7.11's 文本 has a
///   fill, a 描边 and a 背景 at once, and §5.7.13's 放大区域 puts a frame over the
///   enlarged copy. Those have to land *on top*, coverage rule or not.
struct Plane {
    local: PhysRect,
    cov: Vec<u8>,
    rgba: Vec<[u8; 4]>,
}

impl Plane {
    fn new(local: &PhysRect) -> Self {
        let n = (local.w as usize) * (local.h as usize);
        Plane {
            local: *local,
            cov: vec![0; n],
            rgba: vec![[0, 0, 0, 0]; n],
        }
    }

    fn is_empty(&self) -> bool {
        self.cov.iter().all(|v| *v == 0)
    }

    fn frame(&self) -> Result<Frame, FrameError> {
        let mut out = Frame::new(self.local.w, self.local.h)?;
        for y in 0..self.local.h {
            for x in 0..self.local.w {
                let i = self.index(x, y);
                out.set(
                    x,
                    y,
                    [
                        self.rgba[i][0],
                        self.rgba[i][1],
                        self.rgba[i][2],
                        self.cov[i],
                    ],
                );
            }
        }
        Ok(out)
    }

    fn index(&self, x: u32, y: u32) -> usize {
        y as usize * self.local.w as usize + x as usize
    }

    /// The plane slot a picture point falls in, if the plane covers it.
    fn slot(&self, x: i32, y: i32) -> Option<usize> {
        let off = self.local.intersection(&PhysRect::new(x, y, 1, 1))?;
        if off.is_empty() {
            return None;
        }
        Some(self.index((off.x - self.local.x) as u32, (off.y - self.local.y) as u32))
    }

    /// Paint one pixel of the plane with the single-pen coverage rule.
    fn put(&mut self, x: i32, y: i32, v: u8, color: [u8; 4]) {
        let a = v as u32 * color[3] as u32 / 255;
        if a == 0 {
            return;
        }
        let Some(i) = self.slot(x, y) else { return };
        if a > self.cov[i] as u32 {
            self.cov[i] = a as u8;
            self.rgba[i] = [color[0], color[1], color[2], 255];
        }
    }

    /// Composite one pixel on top of what the plane already holds.
    fn over(&mut self, x: i32, y: i32, v: u8, color: [u8; 4]) {
        let a = v as u32 * color[3] as u32 / 255;
        if a == 0 {
            return;
        }
        let Some(i) = self.slot(x, y) else { return };
        let da = self.cov[i] as u32;
        let rest = 255 - a;
        let out_a = a + da * rest / 255;
        let mut c = self.rgba[i];
        for k in 0..3 {
            // Straight over-composite: `num` and `out_a` are the same scale, so
            // the quotient is the colour. `(2*num + out_a) / (2*out_a)` rounds
            // instead of truncating toward black.
            let num = color[k] as u32 * a + c[k] as u32 * da * rest / 255;
            c[k] = ((2 * num + out_a) / (2 * out_a)).min(255) as u8;
        }
        self.rgba[i] = [c[0], c[1], c[2], 255];
        self.cov[i] = out_a.min(255) as u8;
    }

    /// Replace the plane's contents with `src`'s pixels, keeping the coverage.
    /// This is how a mosaic patch or §5.7.13's enlarged copy gets its colour:
    /// the shape is already in the plane, the pixels come from the base.
    fn tint_with(&mut self, src: &Frame, at: &PhysRect) {
        for y in 0..self.local.h {
            for x in 0..self.local.w {
                let i = self.index(x, y);
                if self.cov[i] == 0 {
                    continue;
                }
                let (ax, ay) = (self.local.x + x as i32, self.local.y + y as i32);
                let Some(hit) = at.intersection(&PhysRect::new(ax, ay, 1, 1)) else {
                    self.cov[i] = 0;
                    continue;
                };
                let p = src.get((hit.x - at.x) as u32, (hit.y - at.y) as u32);
                self.rgba[i] = [p[0], p[1], p[2], 255];
            }
        }
    }
}

/// Land the plane on the picture, undoing the element's transform on the way.
fn blit(
    out: &mut Frame,
    plane: &Plane,
    local: &PhysRect,
    pen: &PhysRect,
    tf: &Transform,
    hit: PhysRect,
    e: &Element,
) -> Result<(), FrameError> {
    let layer = plane.frame()?;
    let Some(hit) = hit.intersection(&out.bounds()) else {
        return Ok(());
    };
    let identity = tf.is_identity();
    let centre = pen.pivot();
    // §5.7.14 rule: 默认只擦除标注. An eraser's plane carries the base pixels, so
    // blending it over the overlay is the ordinary operation and reveals the
    // screenshot beneath it. 擦除到透明 is the one mode that takes pixels from the
    // picture itself, which is why the UI has to warn before it is switched on.
    let clears = e.kind.is_eraser() && e.style.erase_to_transparent;
    for dy in 0..hit.h {
        for dx in 0..hit.w {
            let (x, y) = (hit.x + dx as i32, hit.y + dy as i32);
            // `hit` came out of an intersection with the picture, so the cast is safe.
            let (px, py) = (x as u32, y as u32);
            let s = if identity {
                let (lx, ly) = (x - local.x, y - local.y);
                if lx < 0 || ly < 0 || lx as u32 >= layer.width || ly as u32 >= layer.height {
                    continue;
                }
                layer.get(lx as u32, ly as u32)
            } else {
                let (u, v) = tf.to_object(centre, x as f64 + 0.5, y as f64 + 0.5);
                match sample(&layer, u - local.x as f64, v - local.y as f64) {
                    Some(px) => px,
                    None => continue,
                }
            };
            let a = s[3] as u32;
            if a == 0 {
                continue;
            }
            let d = out.get(px, py);
            let da = d[3] as u32;
            // Straight (non-premultiplied) over-compositing. The values in a
            // `Frame` are straight, and `imageops::blend_rgba` premultiplies
            // against the destination, which is only right over an opaque
            // backdrop — the overlay is not, so the maths lives here.
            let rest = 255 - a;
            if clears {
                out.set(px, py, [d[0], d[1], d[2], (da * rest / 255) as u8]);
                continue;
            }
            let a_out = a + da * rest / 255;
            let mut rgba = d;
            for i in 0..3 {
                let num = s[i] as u32 * a + d[i] as u32 * da * rest / 255;
                rgba[i] = ((2 * num + a_out) / (2 * a_out)).min(255) as u8;
            }
            rgba[3] = a_out.min(255) as u8;
            out.set(px, py, rgba);
        }
    }
    Ok(())
}

/// Bilinear sample in premultiplied space, returned straight.
///
/// Sampling straight values across a transparent edge drags black through the
/// stroke, which is exactly what the rim of a rotated hairline would look like.
fn sample(layer: &Frame, u: f64, v: f64) -> Option<[u8; 4]> {
    let (w, h) = (layer.width as f64, layer.height as f64);
    if u < 0.0 || v < 0.0 || u >= w || v >= h {
        return None;
    }
    let (x0, y0) = (u.floor() as i32, v.floor() as i32);
    let (fx, fy) = (u - x0 as f64, v - y0 as f64);
    let (mut r, mut g, mut b, mut a) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (dy, wy) in [(0i32, 1.0 - fy), (1, fy)] {
        for (dx, wx) in [(0i32, 1.0 - fx), (1, fx)] {
            let (sx, sy) = (x0 + dx, y0 + dy);
            if sx < 0 || sy < 0 || sx >= layer.width as i32 || sy >= layer.height as i32 {
                continue;
            }
            let p = layer.get(sx as u32, sy as u32);
            let pa = p[3] as f64 / 255.0;
            let wt = wx * wy;
            r += p[0] as f64 * pa * wt;
            g += p[1] as f64 * pa * wt;
            b += p[2] as f64 * pa * wt;
            a += pa * wt;
        }
    }
    if a <= 0.0001 {
        return Some([0, 0, 0, 0]);
    }
    let round = |c: f64| -> u8 { (c / a).round().clamp(0.0, 255.0) as u8 };
    Some([
        round(r),
        round(g),
        round(b),
        (a * 255.0).round().clamp(0.0, 255.0) as u8,
    ])
}

/// The box a transformed layer can cover. The identity case keeps the common
/// path at exactly the element's own size.
fn laid(local: &PhysRect, pen: &PhysRect, tf: &Transform) -> PhysRect {
    if tf.is_identity() {
        return *local;
    }
    let centre = pen.pivot();
    let corners = [
        (local.x as f64, local.y as f64),
        (local.right() as f64, local.y as f64),
        (local.x as f64, local.bottom() as f64),
        (local.right() as f64, local.bottom() as f64),
    ];
    let (mut l, mut t, mut rr, mut bb) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in corners {
        let (rx, ry) = tf.to_picture(centre, x, y);
        l = l.min(rx);
        t = t.min(ry);
        rr = rr.max(rx);
        bb = bb.max(ry);
    }
    let (x0, y0) = (l.floor() as i32, t.floor() as i32);
    PhysRect::new(
        x0,
        y0,
        (rr.ceil() as i32 - x0).max(1) as u32,
        (bb.ceil() as i32 - y0).max(1) as u32,
    )
}

/// How far the half-thickness of this element's pen reaches, in pixels.
fn half_of(e: &Element) -> f64 {
    if e.kind.is_stroke() {
        e.style.brush.size as f64 / 2.0
    } else {
        e.style.width.max(1) as f64 / 2.0
    }
}

/// Soft-edge width: the brush feather, or the one pixel every hard edge still
/// needs to avoid a staircase.
fn feather_of(e: &Element) -> f64 {
    if e.kind.is_stroke() {
        e.style.brush.feather.max(1) as f64
    } else {
        1.0
    }
}

/// `inside` is how far the point sits inside the painted edge, measured across
/// `ramp` pixels.
fn ramp(inside: f64, r: f64) -> u8 {
    let t = (inside / r.max(1.0) + 0.5).clamp(0.0, 1.0);
    if t >= 1.0 {
        255
    } else if t <= 0.0 {
        0
    } else {
        (t * 255.0 + 0.5) as u8
    }
}

fn dist_to_seg(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let (px, py) = (p.0 - a.0, p.1 - a.1);
    let len2 = vx * vx + vy * vy;
    if len2 == 0.0 {
        return px.hypot(py);
    }
    let t = ((px * vx + py * vy) / len2).clamp(0.0, 1.0);
    (px - vx * t).hypot(py - vy * t)
}

/// Signed distance to a box shape's boundary, positive inside. Analytic, so a
/// full-picture mosaic or a big red rectangle stays one multiply per pixel.
fn box_sd(r: &PhysRect, kind: Kind, radius: u32, p: (f64, f64)) -> f64 {
    let (x, y) = (p.0, p.1);
    match kind {
        Kind::Ellipse => {
            let (rx, ry) = ((r.w as f64 / 2.0).max(0.5), (r.h as f64 / 2.0).max(0.5));
            let (dx, dy) = (x - (r.x as f64 + rx), y - (r.y as f64 + ry));
            (1.0 - ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt()) * rx.min(ry)
        }
        Kind::RoundedRect => {
            let rad = radius.min(r.w / 2).min(r.h / 2) as f64;
            if rad < 0.5 {
                return box_sd(r, Kind::Rect, 0, p);
            }
            // Standard rounded-box SDF: shrink the box by the radius, measure the
            // corner vector, then put the radius back.
            let hw = r.w as f64 / 2.0 - rad;
            let hh = r.h as f64 / 2.0 - rad;
            let (cx, cy) = (r.x as f64 + r.w as f64 / 2.0, r.y as f64 + r.h as f64 / 2.0);
            let q = ((x - cx).abs() - hw, (y - cy).abs() - hh);
            let outside = (q.0.max(0.0).powi(2) + q.1.max(0.0).powi(2)).sqrt();
            let inside = q.0.max(q.1).min(0.0);
            outside + inside - rad
        }
        _ => {
            let ix = (r.right() as f64 - x).min(x - r.x as f64);
            let iy = (r.bottom() as f64 - y).min(y - r.y as f64);
            ix.min(iy)
        }
    }
}

/// The outline polygon of a box shape, for the cases that need to walk an edge
/// rather than test a point: a dashed outline.
fn box_poly(r: &PhysRect, kind: Kind, radius: u32) -> Vec<(f64, f64)> {
    let (x, y) = (r.x as f64, r.y as f64);
    let (w, h) = (r.w as f64, r.h as f64);
    if kind == Kind::Ellipse {
        let (rx, ry) = ((w / 2.0).max(0.5), (h / 2.0).max(0.5));
        let n = 96usize;
        return (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * TAU;
                (x + rx + t.cos() * rx, y + ry + t.sin() * ry)
            })
            .collect();
    }
    let rad = if kind == Kind::RoundedRect {
        radius.min(r.w / 2).min(r.h / 2) as f64
    } else {
        0.0
    };
    if rad < 0.5 {
        return vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    }
    let arcs = [
        (x + w - rad, y + rad, 270.0f64, 360.0f64),
        (x + w - rad, y + h - rad, 0.0, 90.0),
        (x + rad, y + h - rad, 90.0, 180.0),
        (x + rad, y + rad, 180.0, 270.0),
    ];
    let steps = ((rad * 0.5).ceil() as usize).clamp(3, 16);
    let mut out = Vec::with_capacity(arcs.len() * steps);
    for (ccx, ccy, a0, a1) in arcs {
        for s in 0..steps {
            let deg = a0 + (a1 - a0) * s as f64 / steps as f64;
            let t = deg.to_radians();
            out.push((ccx + t.cos() * rad, ccy + t.sin() * rad));
        }
    }
    out
}

/// The polyline a stroked element follows, in picture space.
fn path_pts(g: &Geom) -> Vec<(f64, f64)> {
    match g {
        Geom::Segment { a, b } => vec![(a.x as f64, a.y as f64), (b.x as f64, b.y as f64)],
        Geom::Path(points) => points.iter().map(|p| (p.x as f64, p.y as f64)).collect(),
        Geom::Rect(_) | Geom::Zoom { .. } => Vec::new(),
    }
}

/// A dash pattern as on/off lengths in pixels.
fn dash_len(dash: Dash, width: f64) -> Option<(f64, f64)> {
    match dash {
        Dash::Solid => None,
        Dash::Dash(on, off) => Some((on.max(1) as f64, off.max(1) as f64)),
        // A dot has to read as a dot, so the gap sits a pen width behind it.
        Dash::Dot => Some((width.max(1.0), width.max(1.0) * 2.0)),
    }
}

/// Split a polyline into the runs a dash pattern paints, so a dash is just a
/// shorter capsule and its ends stay round.
fn runs(pts: &[(f64, f64)], closed: bool, dash: Dash, width: f64) -> Vec<Run> {
    if pts.len() < 2 {
        return pts.iter().map(|p| (*p, *p)).collect();
    }
    let mut segs: Vec<Run> = pts.windows(2).map(|w| (w[0], w[1])).collect();
    if closed {
        segs.push((*pts.last().unwrap(), *pts.first().unwrap()));
    }
    let lens: Vec<f64> = segs
        .iter()
        .map(|(a, b)| (b.0 - a.0).hypot(b.1 - a.1))
        .collect();
    let total: f64 = lens.iter().sum();
    if total == 0.0 {
        return vec![(segs[0].0, segs[0].1)];
    }
    let mut starts = vec![0.0f64; segs.len()];
    for i in 1..segs.len() {
        starts[i] = starts[i - 1] + lens[i - 1];
    }
    let at = |d: f64| -> (f64, f64) {
        let d = d.clamp(0.0, total);
        let i = starts.partition_point(|s| *s <= d).saturating_sub(1);
        let (a, b) = segs[i];
        let l = lens[i].max(1e-6);
        let t = ((d - starts[i]) / l).clamp(0.0, 1.0);
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    };
    let Some((on, off)) = dash_len(dash, width) else {
        return segs;
    };
    let cycle = on + off;
    let mut out = Vec::new();
    let mut d = 0.0;
    while d < total {
        let stop = (d + on).min(total);
        if stop > d {
            out.push((at(d), at(stop)));
        }
        d += cycle;
    }
    if out.is_empty() {
        out.push((at(0.0), at(0.0)));
    }
    out
}

/// Paint a stroked set of runs into the plane.
fn paint_stroke(plane: &mut Plane, stroke: &[Run], half: f64, feather: f64, color: [u8; 4]) {
    stroke_into(plane, stroke, half, feather, color, false);
}

/// The same stroke landing on top of what the plane already holds. A frame drawn
/// over §5.7.13's enlarged copy is a second ink, not a second pass of one pen.
fn stroke_on_top(plane: &mut Plane, stroke: &[Run], half: f64, feather: f64, color: [u8; 4]) {
    stroke_into(plane, stroke, half, feather, color, true);
}

fn stroke_into(
    plane: &mut Plane,
    stroke: &[Run],
    half: f64,
    feather: f64,
    color: [u8; 4],
    top: bool,
) {
    let local = plane.local;
    for ly in 0..local.h {
        for lx in 0..local.w {
            let p = (
                local.x as f64 + lx as f64 + 0.5,
                local.y as f64 + ly as f64 + 0.5,
            );
            let mut best = f64::INFINITY;
            for (a, b) in stroke {
                let d = dist_to_seg(p, *a, *b);
                if d < best {
                    best = d;
                }
            }
            let v = ramp(half - best, feather);
            let (x, y) = (local.x + lx as i32, local.y + ly as i32);
            if top {
                plane.over(x, y, v, color);
            } else {
                plane.put(x, y, v, color);
            }
        }
    }
}

/// Paint the inside of a box shape: analytic for the solid case.
fn paint_box_fill(plane: &mut Plane, r: &PhysRect, kind: Kind, radius: u32, color: [u8; 4]) {
    let local = plane.local;
    let feather = 1.0;
    for ly in 0..local.h {
        for lx in 0..local.w {
            let p = (
                local.x as f64 + lx as f64 + 0.5,
                local.y as f64 + ly as f64 + 0.5,
            );
            let sd = box_sd(r, kind, radius, p);
            let v = ramp(sd, feather);
            plane.put(local.x + lx as i32, local.y + ly as i32, v, color);
        }
    }
}

/// The two ends an arrow carries: §5.7.6 step 2 单向 or 双向.
fn arrow_ends(e: &Element) -> Vec<Run> {
    let pts = path_pts(&e.geom);
    if pts.len() < 2 {
        return Vec::new();
    }
    let head = |tip: (f64, f64), from: (f64, f64)| (from, tip);
    let last = *pts.last().unwrap();
    let prev = pts[pts.len() - 2];
    let first = pts[0];
    let second = pts[1];
    let mut out = vec![head(last, prev)];
    if e.kind == Kind::DoubleArrow {
        out.push(head(first, second));
    }
    out
}

/// Paint one arrow head, sized from the pen so a thicker line gets a bigger
/// chevron and the head never looks pasted on.
fn paint_arrow_head(plane: &mut Plane, from: (f64, f64), tip: (f64, f64), e: &Element) {
    let half = half_of(e);
    let feather = feather_of(e);
    let color = e.style.color;
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = dx.hypot(dy);
    if len < 1e-6 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let reach = head_reach(e.style.width.max(1)) as f64;
    match e.style.arrow_head {
        ArrowHead::Triangle => {
            let back = (tip.0 - ux * reach, tip.1 - uy * reach);
            let spread = reach * 0.55;
            let poly = [
                tip,
                (back.0 + px * spread, back.1 + py * spread),
                (back.0 - px * spread, back.1 - py * spread),
            ];
            fill_poly(plane, &poly, feather, color);
        }
        ArrowHead::Chevron => {
            let back = (tip.0 - ux * reach, tip.1 - uy * reach);
            let spread = reach * 0.5;
            let stroke = [
                (back.0 + px * spread, back.1 + py * spread),
                tip,
                (back.0 - px * spread, back.1 - py * spread),
            ];
            let segs: Vec<Run> = stroke.windows(2).map(|w| (w[0], w[1])).collect();
            paint_stroke(plane, &segs, half, feather, color);
        }
        ArrowHead::Circle => {
            let rad = reach * 0.45;
            let local = plane.local;
            for ly in 0..local.h {
                for lx in 0..local.w {
                    let p = (
                        local.x as f64 + lx as f64 + 0.5,
                        local.y as f64 + ly as f64 + 0.5,
                    );
                    let d = ((p.0 - tip.0).powi(2) + (p.1 - tip.1).powi(2)).sqrt() - rad;
                    let v = ramp(half - d.abs(), feather);
                    plane.put(local.x + lx as i32, local.y + ly as i32, v, color);
                }
            }
        }
    }
}

fn fill_poly(plane: &mut Plane, poly: &[(f64, f64)], feather: f64, color: [u8; 4]) {
    let local = plane.local;
    for ly in 0..local.h {
        for lx in 0..local.w {
            let p = (
                local.x as f64 + lx as f64 + 0.5,
                local.y as f64 + ly as f64 + 0.5,
            );
            let mut d = f64::INFINITY;
            for i in 0..poly.len() {
                let j = (i + 1) % poly.len();
                d = d.min(dist_to_seg(p, poly[i], poly[j]));
            }
            if !in_poly(p, poly) {
                d = -d;
            }
            let v = ramp(d, feather);
            plane.put(local.x + lx as i32, local.y + ly as i32, v, color);
        }
    }
}

/// Even-odd test on the pixel centre.
fn in_poly(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    for i in 0..poly.len() {
        let (x0, y0) = poly[i];
        let (x1, y1) = poly[(i + 1) % poly.len()];
        if (y0 > p.1) != (y1 > p.1) {
            let x = x0 + (p.1 - y0) * (x1 - x0) / (y1 - y0);
            if p.0 < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Draw one element into its plane.
fn draw(
    base: &Frame,
    e: &Element,
    local: &PhysRect,
    plane: &mut Plane,
    glyphs: &dyn Glyphs,
) -> Result<(), FrameError> {
    let half = half_of(e);
    let feather = feather_of(e);
    let box_r = match &e.geom {
        Geom::Rect(r) => Some(*r),
        Geom::Zoom { to, .. } => Some(*to),
        _ => None,
    };

    match e.kind {
        Kind::Rect | Kind::RoundedRect | Kind::Ellipse => {
            let Some(r) = box_r else { return Ok(()) };
            if e.style.width > 0 {
                if dash_len(e.style.dash, e.style.width as f64).is_some() {
                    let poly = box_poly(&r, e.kind, e.style.corner_radius);
                    let segs = runs(&poly, true, e.style.dash, e.style.width as f64);
                    paint_stroke(plane, &segs, half, feather, e.style.color);
                } else {
                    // Analytic: |distance to the boundary| inside the pen band.
                    paint_box_band(
                        plane,
                        &r,
                        e.kind,
                        e.style.corner_radius,
                        half,
                        feather,
                        e.style.color,
                    );
                }
            }
            if let Some(fill) = e.style.fill {
                paint_box_fill(plane, &r, e.kind, e.style.corner_radius, fill);
            }
        }
        Kind::Line | Kind::Polyline | Kind::Arrow | Kind::DoubleArrow => {
            let pts = path_pts(&e.geom);
            let segs = runs(&pts, false, e.style.dash, e.style.width as f64);
            paint_stroke(plane, &segs, half, feather, e.style.color);
            if matches!(e.kind, Kind::Arrow | Kind::DoubleArrow) {
                for (from, tip) in arrow_ends(e) {
                    paint_arrow_head(plane, from, tip, e);
                }
            }
        }
        Kind::Pencil | Kind::Marker => {
            let pts = path_pts(&e.geom);
            let segs = runs(&pts, false, Dash::Solid, e.style.brush.size as f64);
            paint_stroke(plane, &segs, half, feather, e.style.color);
        }
        Kind::Mosaic | Kind::Blur => {
            let crop = base.crop(local)?;
            let patch = match e.style.effect {
                imageops::Effect::Mosaic { block } => imageops::mosaic(&crop, block)?,
                imageops::Effect::Blur { radius } => imageops::blur(&crop, radius)?,
            };
            match box_r {
                Some(r) => paint_box_fill(plane, &r, e.kind, e.style.corner_radius, [0, 0, 0, 255]),
                None => {
                    let pts = path_pts(&e.geom);
                    let segs = runs(&pts, false, Dash::Solid, e.style.brush.size as f64);
                    paint_stroke(plane, &segs, half, feather, [0, 0, 0, 255]);
                }
            }
            plane.tint_with(&patch, local);
        }
        Kind::Eraser => {
            match box_r {
                Some(r) => paint_box_fill(plane, &r, Kind::Rect, 0, [0, 0, 0, 255]),
                None => {
                    let pts = path_pts(&e.geom);
                    let segs = runs(&pts, false, Dash::Solid, e.style.brush.size as f64);
                    paint_stroke(plane, &segs, half, feather, [0, 0, 0, 255]);
                }
            }
            if !e.style.erase_to_transparent {
                let crop = base.crop(local)?;
                plane.tint_with(&crop, local);
            }
        }
        Kind::Zoom => {
            let Geom::Zoom { from, to } = e.geom else {
                return Ok(());
            };
            let Some(src) = from.intersection(&base.bounds()) else {
                return Ok(());
            };
            if src.is_empty() {
                return Ok(());
            }
            paint_box_fill(plane, &to, Kind::Rect, 0, [0, 0, 0, 255]);
            let copy = base.crop(&src)?.resized(to.w, to.h, true)?;
            plane.tint_with(&copy, &to);
            // §5.7.13 step 5: 设置放大倍数、边框和连接线. The frame is one pixel
            // at the picture's own scale, not the pen width, and 边框 decides whether
            // it is drawn at all - around the copy and around the source, because the
            // two boxes are one object and a frame on only one of them reads as a
            // mistake about which rectangle is the magnifier.
            let mut segs = Vec::new();
            if e.style.zoom_border {
                segs.extend(runs(&box_poly(&to, Kind::Rect, 0), true, Dash::Solid, 1.0));
                segs.extend(runs(
                    &box_poly(&from, Kind::Rect, 0),
                    true,
                    Dash::Solid,
                    1.0,
                ));
            }
            if e.style.connection_line {
                segs.extend(connectors(&from, &to));
            }
            stroke_on_top(plane, &segs, 0.5, 1.0, e.style.color);
        }
        Kind::Text | Kind::Number => {
            let Some(r) = box_r else { return Ok(()) };
            let text = match e.kind {
                Kind::Number => e
                    .number
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| e.text.clone()),
                _ => e.text.clone(),
            };
            if text.is_empty() {
                return Ok(());
            }
            let lines = layout(&r, &text, &e.style, glyphs);
            if let Some(bg) = e.style.text_bg {
                // The PRD switches a background on and off but fixes no inset, so
                // the box hugs the laid-out glyphs plus a quarter of the font.
                let area = text_box(&lines, &r, &e.style);
                paint_box_fill(
                    plane,
                    &area,
                    Kind::RoundedRect,
                    e.style.corner_radius / 2,
                    bg,
                );
            }
            if lines.is_empty() {
                return Ok(());
            }
            // Glyph coverage lands in the plane's own buffer, which is indexed
            // relative to `local` — the layout is in picture coordinates.
            let mut cov = vec![0u8; plane.cov.len()];
            let w = local.w as usize;
            for (ink, ox, oy) in &lines {
                let (ax, ay) = (*ox as i32 - local.x, *oy as i32 - local.y);
                for iy in 0..ink.height as i32 {
                    let y = ay + iy;
                    if y >= local.h as i32 {
                        break;
                    }
                    if y < 0 {
                        continue;
                    }
                    for ix in 0..ink.width as i32 {
                        let x = ax + ix;
                        if x >= local.w as i32 {
                            break;
                        }
                        if x < 0 {
                            continue;
                        }
                        let i = y as usize * w + x as usize;
                        let v = ink.at(ix as u32, iy as u32);
                        if v > cov[i] {
                            cov[i] = v;
                        }
                    }
                }
            }
            if let Some((color, width)) = e.style.text_outline {
                let ring = dilate(&cov, local.w, local.h, width.max(1).div_ceil(2));
                for i in 0..cov.len() {
                    let v = ring[i].saturating_sub(cov[i]);
                    if v > 0 {
                        let (x, y) = split(i, local.w as usize);
                        plane.over(local.x + x as i32, local.y + y as i32, v, color);
                    }
                }
            }
            for (i, &v) in cov.iter().enumerate() {
                if v > 0 {
                    let (x, y) = split(i, local.w as usize);
                    plane.over(local.x + x as i32, local.y + y as i32, v, e.style.color);
                }
            }
        }
    }
    Ok(())
}

fn split(i: usize, w: usize) -> (u32, u32) {
    ((i % w) as u32, (i / w) as u32)
}

/// Band around a box boundary: inside the pen, outside nothing.
fn paint_box_band(
    plane: &mut Plane,
    r: &PhysRect,
    kind: Kind,
    radius: u32,
    half: f64,
    feather: f64,
    color: [u8; 4],
) {
    let local = plane.local;
    for ly in 0..local.h {
        for lx in 0..local.w {
            let p = (
                local.x as f64 + lx as f64 + 0.5,
                local.y as f64 + ly as f64 + 0.5,
            );
            let sd = box_sd(r, kind, radius, p);
            let v = ramp(half - sd.abs(), feather);
            plane.put(local.x + lx as i32, local.y + ly as i32, v, color);
        }
    }
}

/// Corner-to-corner lines from the source region to the enlarged copy.
fn connectors(from: &PhysRect, to: &PhysRect) -> Vec<Run> {
    let a = [
        (from.x as f64, from.y as f64),
        (from.right() as f64, from.y as f64),
        (from.x as f64, from.bottom() as f64),
        (from.right() as f64, from.bottom() as f64),
    ];
    let b = [
        (to.x as f64, to.y as f64),
        (to.right() as f64, to.y as f64),
        (to.x as f64, to.bottom() as f64),
        (to.right() as f64, to.bottom() as f64),
    ];
    a.iter().zip(b.iter()).map(|(p, q)| (*p, *q)).collect()
}

/// Glyph lines in picture space: `(ink, x, y)`, wrapped to the box.
fn layout(r: &PhysRect, text: &str, style: &Style, glyphs: &dyn Glyphs) -> Vec<(Ink, u32, u32)> {
    let mut out = Vec::new();
    let mut y = r.y;
    for para in text.split('\n') {
        for seg in wrap(para, r.w, style, glyphs) {
            let Some(ink) = glyphs.ink(&seg, style) else {
                continue;
            };
            let avail = r.w as i64 - ink.width as i64;
            let height = ink.height as i32;
            let x = match style.align {
                Align::Left => r.x,
                Align::Center => r.x + (avail / 2) as i32,
                Align::Right => r.x + avail as i32,
            };
            if y < 0 || x < 0 {
                continue;
            }
            out.push((ink, x as u32, y as u32));
            y += height;
        }
    }
    out
}

/// Break a paragraph so no line runs past the box. With no font there is nothing
/// to measure, so a paragraph stays one line.
fn wrap(para: &str, width: u32, style: &Style, glyphs: &dyn Glyphs) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in para.split_whitespace() {
        let trial = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        match glyphs.ink(&trial, style) {
            None => cur = trial,
            Some(ink) if ink.width <= width || cur.is_empty() => cur = trial,
            Some(_) => {
                out.push(std::mem::take(&mut cur));
                cur = word.to_string();
            }
        }
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

/// The rectangle the laid text covers.
fn text_box(lines: &[(Ink, u32, u32)], r: &PhysRect, style: &Style) -> PhysRect {
    if lines.is_empty() {
        return *r;
    }
    let left = lines.iter().map(|(_, x, _)| *x as i32).min().unwrap_or(r.x);
    let top = lines.iter().map(|(_, _, y)| *y as i32).min().unwrap_or(r.y);
    let right = lines
        .iter()
        .map(|(ink, x, _)| *x as i32 + ink.width as i32)
        .max()
        .unwrap_or(r.right());
    let bottom = lines
        .iter()
        .map(|(ink, _, y)| *y as i32 + ink.height as i32)
        .max()
        .unwrap_or(r.bottom());
    let pad = (style.font_size / 4).min(64) as i32;
    let l = (left - pad).max(r.x);
    let t = (top - pad).max(r.y);
    let rr = (right + pad).min(r.right());
    let bb = (bottom + pad).min(r.bottom());
    PhysRect::new(l, t, (rr - l).max(1) as u32, (bb - t).max(1) as u32)
}

/// Grow a coverage plane by `by` pixels: §5.7.11's 文本描边 is the glyph shape
/// with a ring around it, and the ring is drawn under the glyph.
fn dilate(cov: &[u8], width: u32, height: u32, by: u32) -> Vec<u8> {
    if by == 0 {
        return cov.to_vec();
    }
    let horiz = shift_max(cov, width, height, by, true);
    shift_max(&horiz, width, height, by, false)
}

fn shift_max(cov: &[u8], width: u32, height: u32, by: u32, along_x: bool) -> Vec<u8> {
    let mut out = vec![0u8; cov.len()];
    let (major, minor) = if along_x {
        (height, width)
    } else {
        (width, height)
    };
    let idx = |a: u32, b: u32| -> usize {
        if along_x {
            a as usize * width as usize + b as usize
        } else {
            b as usize * width as usize + a as usize
        }
    };
    for a in 0..major {
        for b in 0..minor {
            let mut best = 0u8;
            for k in 0..=(by * 2) {
                let s = b as i64 + k as i64 - by as i64;
                if s < 0 || s >= minor as i64 {
                    continue;
                }
                best = best.max(cov[idx(a, s as u32)]);
            }
            out[idx(a, b)] = best;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::{Brush, Command, KINDS};
    use crate::geometry::PhysPoint;
    use crate::imageops::Effect;
    use proptest::prelude::*;
    use std::cell::RefCell;

    const BASE: [u8; 4] = [10, 20, 30, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    fn base() -> Frame {
        Frame::filled(64, 48, BASE).unwrap()
    }

    fn doc1(kind: Kind, geom: Geom, style: Style) -> Document {
        let mut doc = Document::new(64, 48);
        doc.add(kind, geom, style);
        doc
    }

    fn render1(kind: Kind, geom: Geom, style: Style) -> Frame {
        render(&base(), &doc1(kind, geom, style), &NoGlyphs).unwrap()
    }

    /// A font that measures a character as a square of the font size: enough to
    /// test the §5.7.11 layout and ink order without a font on the machine.
    #[derive(Default)]
    struct Blocks {
        asked: RefCell<Vec<String>>,
    }

    impl Glyphs for Blocks {
        fn ink(&self, line: &str, style: &Style) -> Option<Ink> {
            self.asked.borrow_mut().push(line.to_string());
            let n = line.chars().count() as u32;
            Some(Ink::solid(n * style.font_size, style.font_size))
        }
    }

    #[test]
    fn an_empty_document_renders_the_base_back() {
        let doc = Document::new(64, 48);
        assert_eq!(render(&base(), &doc, &NoGlyphs).unwrap(), base());
    }

    #[test]
    fn a_line_width_paints_exactly_its_own_band() {
        // half = 2, feather = 1: |sd| <= 1.5 is fully inked, |sd| >= 2.5 is not at all.
        let style = Style {
            color: RED,
            width: 4,
            ..Style::default()
        };
        let out = render1(Kind::Rect, Geom::Rect(PhysRect::new(8, 8, 16, 12)), style);
        for y in 6..=9 {
            assert_eq!(out.get(16, y), RED, "row {y} of the top edge");
        }
        assert_eq!(out.get(16, 5), BASE, "one pixel past the pen");
        assert_eq!(out.get(16, 10), BASE, "one pixel inside the band");
        assert_eq!(out.get(16, 14), BASE, "the interior is not filled");
    }

    #[test]
    fn a_pen_joint_does_not_stack_its_own_alpha() {
        let style = Style {
            color: [255, 0, 0, 100],
            brush: Brush {
                size: 8,
                ..Brush::default()
            },
            ..Style::default()
        };
        let out = render1(
            Kind::Marker,
            Geom::Path(vec![
                PhysPoint::new(10, 20),
                PhysPoint::new(30, 20),
                PhysPoint::new(30, 10),
            ]),
            style,
        );
        let shaft = out.get(20, 20);
        // One 40%-opaque pen over the picture: alpha 255, colour blended, and no
        // channel pinned to its maximum.
        assert_eq!(
            shaft,
            [106, 12, 18, 255],
            "a translucent marker stays translucent"
        );
        assert_eq!(
            out.get(30, 20),
            shaft,
            "the joint must not be a second pass of the same pen"
        );
    }

    #[test]
    fn an_effect_reads_the_base_picture_not_the_overlay_under_it() {
        let mut doc = Document::new(64, 48);
        doc.add(
            Kind::Rect,
            Geom::Rect(PhysRect::new(16, 16, 16, 16)),
            Style {
                color: RED,
                fill: Some(RED),
                width: 0,
                ..Style::default()
            },
        );
        doc.add(
            Kind::Mosaic,
            Geom::Rect(PhysRect::new(16, 16, 8, 8)),
            Style {
                effect: Effect::Mosaic { block: 8 },
                ..Style::default()
            },
        );
        let out = render(&base(), &doc, &NoGlyphs).unwrap();
        // The base is uniform, so a mosaic of it is the base. Any red here would be
        // the effect reading the half-painted overlay instead of the screenshot.
        assert_eq!(out.get(18, 18), BASE, "mosaic over the red fill");
        assert_eq!(out.get(30, 30), RED, "the fill the mosaic did not cover");
    }

    fn erase_doc(transparent: bool) -> Document {
        let mut doc = Document::new(64, 48);
        doc.add(
            Kind::Rect,
            Geom::Rect(PhysRect::new(10, 10, 20, 20)),
            Style {
                color: RED,
                fill: Some(RED),
                width: 0,
                ..Style::default()
            },
        );
        doc.add(
            Kind::Eraser,
            Geom::Rect(PhysRect::new(14, 14, 8, 8)),
            Style {
                erase_to_transparent: transparent,
                ..Style::default()
            },
        );
        doc
    }

    #[test]
    fn the_default_eraser_reveals_the_screenshot() {
        let out = render(&base(), &erase_doc(false), &NoGlyphs).unwrap();
        assert_eq!(out.get(16, 16), BASE, "§5.7.14: 默认只擦除标注");
        assert_eq!(out.get(11, 11), RED, "outside the box the ink stays");
    }

    #[test]
    fn erasing_to_transparent_takes_the_picture_pixels_too() {
        let out = render(&base(), &erase_doc(true), &NoGlyphs).unwrap();
        assert_eq!(
            out.get(16, 16)[3],
            0,
            "§5.7.14: 擦除到透明 removes the base"
        );
        assert_eq!(out.get(11, 11)[3], 255, "and only where the eraser went");
    }

    #[test]
    fn repainting_each_elements_bounds_reproduces_the_full_render() {
        let mut doc = Document::new(64, 48);
        doc.add(
            Kind::Rect,
            Geom::Rect(PhysRect::new(6, 6, 20, 14)),
            Style {
                color: RED,
                width: 4,
                fill: Some([0, 0, 255, 128]),
                ..Style::default()
            },
        );
        doc.add(
            Kind::Arrow,
            Geom::Segment {
                a: PhysPoint::new(40, 40),
                b: PhysPoint::new(10, 12),
            },
            Style {
                color: BLUE,
                width: 5,
                ..Style::default()
            },
        );
        doc.add(
            Kind::Marker,
            Geom::Path(vec![
                PhysPoint::new(44, 8),
                PhysPoint::new(52, 20),
                PhysPoint::new(30, 30),
            ]),
            Style {
                color: [255, 255, 0, 90],
                brush: Brush {
                    size: 10,
                    feather: 3,
                    ..Brush::default()
                },
                ..Style::default()
            },
        );
        doc.add(
            Kind::Text,
            Geom::Rect(PhysRect::new(12, 30, 24, 12)),
            Style {
                color: RED,
                text_bg: Some([0, 255, 255, 200]),
                corner_radius: 4,
                ..Style::default()
            },
        );
        doc.add(
            Kind::Blur,
            Geom::Rect(PhysRect::new(2, 34, 10, 10)),
            Style {
                effect: Effect::Blur { radius: 2 },
                ..Style::default()
            },
        );
        doc.add(
            Kind::Zoom,
            Geom::Zoom {
                from: PhysRect::new(2, 2, 8, 8),
                to: PhysRect::new(20, 20, 24, 24),
            },
            Style {
                color: GREEN,
                connection_line: true,
                ..Style::default()
            },
        );
        let line = doc.add(
            Kind::Line,
            Geom::Segment {
                a: PhysPoint::new(8, 44),
                b: PhysPoint::new(58, 44),
            },
            Style {
                color: BLUE,
                width: 3,
                dash: Dash::Dash(2, 12),
                ..Style::default()
            },
        );
        doc.get_mut(line.id).unwrap().transform = Transform {
            rotation: 30.0,
            flip_h: true,
            flip_v: false,
        };
        let ellipse = doc.add(
            Kind::Ellipse,
            Geom::Rect(PhysRect::new(44, 2, 14, 8)),
            Style {
                color: RED,
                fill: Some(RED),
                width: 0,
                ..Style::default()
            },
        );
        doc.get_mut(ellipse.id).unwrap().transform = Transform {
            rotation: -45.0,
            ..Transform::default()
        };

        let full = render(&base(), &doc, &NoGlyphs).unwrap();
        let mut out = base();
        for e in doc.paint_order() {
            let b = e.bounds();
            paint(&base(), &doc, &mut out, &b, &NoGlyphs).unwrap();
        }
        assert_eq!(
            out, full,
            "no element may draw outside the bounds it reports"
        );
    }

    #[test]
    fn the_dirty_rects_a_rotate_reports_are_enough_to_undo_it() {
        let style = Style {
            color: RED,
            fill: Some(RED),
            width: 0,
            ..Style::default()
        };
        let mut doc = doc1(Kind::Rect, Geom::Rect(PhysRect::new(10, 20, 20, 10)), style);
        let tf = Transform {
            rotation: 90.0,
            ..Transform::default()
        };
        let cmd = Command::Rotate(vec![(1, Transform::default(), tf)]);
        let flat = render(&base(), &doc, &NoGlyphs).unwrap();
        let dirty = cmd.apply(&mut doc);
        let turned = render(&base(), &doc, &NoGlyphs).unwrap();
        assert_ne!(flat, turned, "the rotation should have changed something");

        let mut out = flat.clone();
        for r in &dirty.rects {
            paint(&base(), &doc, &mut out, r, &NoGlyphs).unwrap();
        }
        assert_eq!(out, turned, "repainting the reported rects missed a corner");

        let dirty = cmd.invert(&mut doc);
        let mut out = turned.clone();
        for r in &dirty.rects {
            paint(&base(), &doc, &mut out, r, &NoGlyphs).unwrap();
        }
        assert_eq!(out, flat, "undo left the turned bar behind");
    }

    #[test]
    fn a_repainted_rect_returns_to_the_picture_when_nothing_draws_there() {
        let style = Style {
            color: RED,
            fill: Some(RED),
            width: 0,
            ..Style::default()
        };
        let doc = doc1(Kind::Rect, Geom::Rect(PhysRect::new(10, 10, 20, 20)), style);
        let mut out = render(&base(), &doc, &NoGlyphs).unwrap();
        let empty = Document::new(64, 48);
        paint(
            &base(),
            &empty,
            &mut out,
            &PhysRect::new(8, 8, 24, 24),
            &NoGlyphs,
        )
        .unwrap();
        assert_eq!(out, base(), "a deleted element must not keep its pixels");
    }

    #[test]
    fn a_transformed_element_is_drawn_where_it_is_turned() {
        let style = Style {
            color: RED,
            fill: Some(RED),
            width: 0,
            ..Style::default()
        };
        let mut doc = doc1(Kind::Rect, Geom::Rect(PhysRect::new(10, 20, 20, 10)), style);
        doc.get_mut(1).unwrap().transform = Transform {
            rotation: 90.0,
            ..Transform::default()
        };
        let out = render(&base(), &doc, &NoGlyphs).unwrap();
        assert_eq!(out.get(20, 20), RED, "the bar now runs vertically");
        assert_eq!(out.get(20, 30), RED);
        assert_eq!(
            out.get(12, 25),
            BASE,
            "and nothing is left where it lay flat"
        );
    }

    #[test]
    fn a_flip_moves_the_stroke_to_the_mirrored_side() {
        let seg = Geom::Segment {
            a: PhysPoint::new(10, 10),
            b: PhysPoint::new(30, 30),
        };
        let style = Style {
            color: RED,
            ..Style::default()
        };
        assert_eq!(
            render1(Kind::Line, seg.clone(), style.clone()).get(15, 15),
            RED
        );

        let mut doc = doc1(Kind::Line, seg, style);
        doc.get_mut(1).unwrap().transform = Transform {
            flip_v: true,
            ..Transform::default()
        };
        let out = render(&base(), &doc, &NoGlyphs).unwrap();
        assert_eq!(out.get(15, 15), BASE, "the unflipped diagonal is gone");
        // The pivot is (20,20), so the line lands on x + y = 40; (14,25) is the
        // pixel whose centre maps back onto it exactly.
        assert_eq!(out.get(14, 25), RED, "x + y = 40");
    }

    #[test]
    fn text_glyphs_land_on_top_of_their_own_background() {
        let style = Style {
            color: RED,
            text_bg: Some(BLUE),
            corner_radius: 0,
            font_size: 18,
            ..Style::default()
        };
        let mut doc = doc1(Kind::Text, Geom::Rect(PhysRect::new(10, 10, 40, 22)), style);
        doc.get_mut(1).unwrap().text = "ab".into();
        let blocks = Blocks::default();
        let out = render(&base(), &doc, &blocks).unwrap();
        // §5.7.11: 背景 and glyph are separate inks, so the coverage rule that
        // merges one pen must not let the background swallow the glyph.
        assert_eq!(out.get(20, 20), RED, "the glyph over its background");
        assert_eq!(
            out.get(48, 20),
            BLUE,
            "the background the glyph does not reach"
        );
        assert!(blocks.asked.borrow().iter().any(|l| l == "ab"));
    }

    #[test]
    fn text_still_gets_its_background_when_there_is_no_font() {
        let style = Style {
            color: RED,
            text_bg: Some(BLUE),
            corner_radius: 0,
            font_size: 18,
            ..Style::default()
        };
        let mut doc = doc1(Kind::Text, Geom::Rect(PhysRect::new(10, 10, 40, 22)), style);
        doc.get_mut(1).unwrap().text = "ab".into();
        let out = render(&base(), &doc, &NoGlyphs).unwrap();
        assert_eq!(out.get(20, 20), BLUE);
        assert_eq!(out.get(48, 20), BLUE);
        assert_eq!(
            out.get(4, 4),
            BASE,
            "the box is no bigger than the geometry"
        );
    }

    #[test]
    fn the_extent_is_the_widest_line_and_all_of_their_height() {
        // `Blocks` measures a character as a square of the font size, so the expected
        // numbers come from the test's own font rather than from a second copy of the
        // arithmetic in `text_extent`.
        let style = Style {
            font_size: 18,
            ..Style::default()
        };
        assert_eq!(
            text_extent("ab", &style, &Blocks::default()),
            Some((36, 18)),
            "one line"
        );
        assert_eq!(
            text_extent("a\nbcd", &style, &Blocks::default()),
            Some((54, 36)),
            "the widest of the two lines (three blocks) and both of them tall"
        );
        assert_eq!(
            text_extent("", &style, &Blocks::default()),
            Some((1, 18)),
            "an empty box still has a side, or it is not a box"
        );
        assert_eq!(
            text_extent("ab", &style, &NoGlyphs),
            None,
            "a font that cannot answer is not reported as zero"
        );
    }

    #[test]
    fn a_box_measured_from_the_extent_needs_no_wrap() {
        // The claim in `text_extent`'s own comment: give `layout` the box it returns and
        // every paragraph stays one line. If `wrap` ever split one, the text would be
        // taller than the box measured for it, and the lines below would be clipped by
        // the very paint call that laid them out.
        let style = Style {
            font_size: 18,
            ..Style::default()
        };
        let text = "ab\ncd ef\nghi";
        let (w, h) = text_extent(text, &style, &Blocks::default()).unwrap();
        let lines = layout(&PhysRect::new(0, 0, w, h), text, &style, &Blocks::default());
        assert_eq!(lines.len(), 3, "one line per paragraph, none wrapped");
        let tall: i64 = lines.iter().map(|(ink, _, _)| ink.height as i64).sum();
        assert_eq!(tall, h as i64, "the box is exactly as tall as its lines");
    }

    #[test]
    fn a_number_lays_out_the_number_it_was_given() {
        let mut doc = Document::new(64, 48);
        let style = Style {
            color: RED,
            font_size: 18,
            corner_radius: 0,
            ..Style::default()
        };
        let a = doc.add(
            Kind::Number,
            Geom::Rect(PhysRect::new(10, 10, 40, 22)),
            style.clone(),
        );
        let b = doc.add(
            Kind::Number,
            Geom::Rect(PhysRect::new(10, 30, 40, 12)),
            style,
        );
        assert_eq!(
            (a.number, b.number),
            (Some(1), Some(2)),
            "§5.7.12 起始编号 counts up"
        );

        let blocks = Blocks::default();
        let out = render(&base(), &doc, &blocks).unwrap();
        assert_eq!(out.get(20, 20), RED, "one 18px square of ink");
        assert_eq!(out.get(40, 20), BASE, "and not wider than that");
        assert!(blocks.asked.borrow().iter().any(|l| l == "1"));
    }

    #[test]
    fn invisible_elements_are_not_painted_and_z_decides_the_overlap() {
        let mut doc = Document::new(64, 48);
        let fill = |c: [u8; 4]| Style {
            color: c,
            fill: Some(c),
            width: 0,
            ..Style::default()
        };
        let red = doc.add(
            Kind::Rect,
            Geom::Rect(PhysRect::new(10, 10, 20, 20)),
            fill(RED),
        );
        let blue = doc.add(
            Kind::Rect,
            Geom::Rect(PhysRect::new(10, 10, 20, 20)),
            fill(BLUE),
        );
        let at = |doc: &Document| render(&base(), doc, &NoGlyphs).unwrap().get(20, 20);
        assert_eq!(at(&doc), BLUE, "added later is painted later");
        doc.get_mut(blue.id).unwrap().visible = false;
        assert_eq!(at(&doc), RED, "an invisible element is not painted");
        doc.get_mut(red.id).unwrap().visible = false;
        assert_eq!(
            render(&base(), &doc, &NoGlyphs).unwrap(),
            base(),
            "nothing visible is nothing drawn"
        );
        doc.get_mut(red.id).unwrap().visible = true;
        doc.get_mut(blue.id).unwrap().visible = true;
        doc.get_mut(red.id).unwrap().z = 5;
        assert_eq!(at(&doc), RED, "z wins over insertion order");
    }

    #[test]
    fn a_dash_leaves_gaps_and_a_solid_line_does_not() {
        let seg = Geom::Segment {
            a: PhysPoint::new(10, 20),
            b: PhysPoint::new(60, 20),
        };
        let style = Style {
            color: RED,
            ..Style::default()
        };
        let solid = render1(Kind::Line, seg.clone(), style.clone());
        assert_eq!(solid.get(20, 20), RED);
        assert_eq!(solid.get(50, 20), RED);
        let dashed = render1(
            Kind::Line,
            seg,
            Style {
                dash: Dash::Dash(2, 12),
                ..style
            },
        );
        assert_eq!(dashed.get(11, 20), RED, "the first dash");
        assert_eq!(dashed.get(20, 20), BASE, "the gap after it");
    }

    /// Half a picture with a marked-up corner, for the effects that copy pixels.
    fn marked() -> Frame {
        let mut picture = Frame::filled(64, 48, GREEN).unwrap();
        for y in 2..10 {
            for x in 2..10 {
                picture.set(x, y, BLUE);
            }
        }
        picture
    }

    fn zoom_doc(connection_line: bool) -> Document {
        zoom_doc_with(connection_line, true)
    }

    fn zoom_doc_with(connection_line: bool, zoom_border: bool) -> Document {
        doc1(
            Kind::Zoom,
            Geom::Zoom {
                from: PhysRect::new(2, 2, 8, 8),
                to: PhysRect::new(20, 20, 24, 24),
            },
            Style {
                color: RED,
                connection_line,
                zoom_border,
                ..Style::default()
            },
        )
    }

    #[test]
    fn a_zoom_enlarges_the_copy_and_frames_it() {
        let out = render(&marked(), &zoom_doc(false), &NoGlyphs).unwrap();
        assert_eq!(
            out.get(30, 30),
            BLUE,
            "§5.7.13: the copy is the source, enlarged"
        );
        let frame = out.get(30, 20);
        assert_ne!(frame, BLUE, "and the frame goes over it");
        assert!(
            frame[0] > 0 && frame[0] < 255,
            "the frame is a blend of the pen and the copy, not a channel: {frame:?}"
        );
    }

    #[test]
    fn the_connection_line_is_what_the_toggle_is_for() {
        let plain = render(&marked(), &zoom_doc(false), &NoGlyphs).unwrap();
        let lined = render(&marked(), &zoom_doc(true), &NoGlyphs).unwrap();
        // (11,11) sits between the two boxes, on the corner-to-corner line.
        assert_eq!(plain.get(11, 11), GREEN, "no connector, nothing drawn");
        assert_ne!(lined.get(11, 11), GREEN, "§5.7.13 step 5: 连接线");
    }

    #[test]
    fn the_border_is_what_its_toggle_is_for() {
        let framed = render(&marked(), &zoom_doc_with(false, true), &NoGlyphs).unwrap();
        let bare = render(&marked(), &zoom_doc_with(false, false), &NoGlyphs).unwrap();
        // (30,20) is the copy's own top edge and (2,2) the source's corner: the frame
        // lies on both while it is on, and both are plain picture once it is off.
        assert_ne!(framed.get(30, 20), BLUE, "§5.7.13 step 5: 边框");
        assert_ne!(
            framed.get(2, 2),
            BLUE,
            "and it frames the source, not only the copy"
        );
        assert_eq!(
            bare.get(30, 20),
            BLUE,
            "off means the copy is all that is left"
        );
        assert_eq!(bare.get(2, 2), BLUE);
        // The switch is a border and not the object it borders: read the middle of the
        // copy too, or a version that erased the whole element would pass this test.
        assert_eq!(
            bare.get(30, 30),
            BLUE,
            "the enlarged picture is still the picture"
        );
        let lined = render(&marked(), &zoom_doc_with(true, false), &NoGlyphs).unwrap();
        assert_ne!(
            lined.get(11, 11),
            GREEN,
            "边框 off does not take the 连接线 with it"
        );
    }

    #[test]
    fn a_wholly_outside_element_costs_nothing() {
        let out = render1(
            Kind::Rect,
            Geom::Rect(PhysRect::new(100, 100, 10, 10)),
            Style {
                color: RED,
                fill: Some(RED),
                ..Style::default()
            },
        );
        assert_eq!(out, base());
    }

    fn arb_rect() -> impl Strategy<Value = PhysRect> {
        (0i32..20, 0i32..20, 1u32..12, 1u32..12).prop_map(|(x, y, w, h)| PhysRect::new(x, y, w, h))
    }

    fn arb_point() -> impl Strategy<Value = PhysPoint> {
        (0i32..20, 0i32..20).prop_map(|(x, y)| PhysPoint::new(x, y))
    }

    /// One element in a 24x24 picture: every kind, a geometry it can use, a
    /// transform and one of the styles that tool can be set to.
    fn arb_element() -> impl Strategy<Value = (Kind, Geom, Transform, u8)> {
        (
            prop::sample::select(Vec::from(KINDS)),
            (
                arb_rect(),
                arb_rect(),
                prop::collection::vec(arb_point(), 1..6),
            ),
            0u8..8,
            (any::<bool>(), any::<bool>()),
        )
            .prop_map(|(kind, (r1, r2, pts), turn, (flip_h, flip_v))| {
                let geom = match kind {
                    Kind::Zoom => Geom::Zoom { from: r1, to: r2 },
                    Kind::Line | Kind::Arrow | Kind::DoubleArrow => Geom::Segment {
                        a: pts[0],
                        b: pts[pts.len() - 1],
                    },
                    Kind::Rect | Kind::RoundedRect | Kind::Ellipse | Kind::Text | Kind::Number => {
                        Geom::Rect(r1)
                    }
                    _ if turn.is_multiple_of(2) => Geom::Rect(r1),
                    _ => Geom::Path(pts),
                };
                let rotation = [0.0, 15.0, -30.0, 45.0, 90.0, 180.0, 270.0, 120.0][turn as usize];
                (
                    kind,
                    geom,
                    Transform {
                        rotation,
                        flip_h,
                        flip_v,
                    },
                    turn,
                )
            })
    }

    fn style_for(kind: Kind, v: u8) -> Style {
        let mut s = Style {
            color: RED,
            ..Style::default()
        };
        match kind {
            Kind::Rect | Kind::RoundedRect | Kind::Ellipse => {
                s.width = match v % 3 {
                    0 => 0,
                    1 => 4,
                    _ => 9,
                };
                s.fill = if v.is_multiple_of(2) {
                    Some([0, 0, 255, 128])
                } else {
                    None
                };
                s.dash = match v % 4 {
                    0 => Dash::Solid,
                    1 => Dash::Dash(2, 6),
                    2 => Dash::Dot,
                    _ => Dash::Solid,
                };
            }
            Kind::Line | Kind::Polyline | Kind::Arrow | Kind::DoubleArrow => {
                s.width = match v % 3 {
                    0 => 1,
                    1 => 5,
                    _ => 12,
                };
                s.dash = if v.is_multiple_of(2) {
                    Dash::Solid
                } else {
                    Dash::Dash(2, 8)
                };
                s.arrow_head = match v % 3 {
                    0 => ArrowHead::Triangle,
                    1 => ArrowHead::Chevron,
                    _ => ArrowHead::Circle,
                };
            }
            Kind::Pencil | Kind::Marker => {
                s.color = [255, 0, 0, 120];
                s.brush = Brush {
                    size: match v % 3 {
                        0 => 4,
                        1 => 10,
                        _ => 2,
                    },
                    feather: u32::from(v % 3) * 2,
                    ..Brush::default()
                };
            }
            Kind::Mosaic | Kind::Blur => {
                s.effect = if v.is_multiple_of(2) {
                    Effect::Mosaic { block: 4 }
                } else {
                    Effect::Blur { radius: 2 }
                };
                s.brush = Brush {
                    size: 6,
                    ..Brush::default()
                };
            }
            Kind::Text | Kind::Number => {
                s.corner_radius = 0;
                s.text_bg = if v.is_multiple_of(2) {
                    Some([0, 255, 255, 200])
                } else {
                    None
                };
                s.text_outline = if v.is_multiple_of(3) {
                    Some((GREEN, 2))
                } else {
                    None
                };
            }
            Kind::Zoom => {
                s.connection_line = v.is_multiple_of(2);
                s.zoom_border = !v.is_multiple_of(3);
            }
            Kind::Eraser => {
                s.erase_to_transparent = v.is_multiple_of(2);
                s.brush = Brush {
                    size: 6,
                    ..Brush::default()
                };
            }
        }
        s
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// The one promise the rasteriser makes to the dirty-rect protocol: an
        /// element only ever touches the rectangle [`Element::bounds`] reports.
        #[test]
        fn nothing_draws_outside_the_bounds_it_reports((kind, geom, transform, v) in arb_element()) {
            let size = 24u32;
            let base = Frame::filled(size, size, BASE).unwrap();
            let mut doc = Document::new(size, size);
            let mut style = style_for(kind, v);
            if kind == Kind::Number {
                style.font_size = 6;
            }
            let id = doc.add(kind, geom, style).id;
            if let Some(e) = doc.get_mut(id) {
                e.transform = transform;
                if kind == Kind::Text {
                    e.text = "hi there".into();
                }
            }
            let out = render(&base, &doc, &NoGlyphs).unwrap();
            prop_assert_eq!(out.width, size, "the picture is never resized");
            let b = doc.get(id).unwrap().bounds();
            for y in 0..size {
                for x in 0..size {
                    if b.contains(PhysPoint::new(x as i32, y as i32)) {
                        continue;
                    }
                    prop_assert_eq!(
                        out.get(x, y),
                        BASE,
                        "{:?} at {:?} painted outside its own bounds",
                        kind,
                        b
                    );
                }
            }
        }
    }
}
