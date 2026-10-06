//! The annotated picture as a list of elements (plan §6.4, 技术方案 §8.1).
//!
//! MVP paints the overlay into a bitmap, but the model is an element list from
//! the first line of code: §5.7.16's per-object re-editing then replaces the
//! rasteriser without migrating any data. That is why nothing here stores
//! pixels.
//!
//! Coordinates are physical pixels **in picture-local space** — the origin is
//! the top-left of the captured selection — so a document is independent of
//! where on the desktop it came from, and survives being pinned or re-cropped.

use crate::geometry::{PhysPoint, PhysRect};
use crate::imageops::{Effect, Shape};

/// Which tool created the element: 矩形 §5.7.2, 圆角矩形 §5.7.3, 椭圆 §5.7.4,
/// 直线/折线 §5.7.5, 箭头 §5.7.6, 铅笔 §5.7.7, 马克笔 §5.7.8, 马赛克 §5.7.9,
/// 模糊 §5.7.10, 文本 §5.7.11, 编号 §5.7.12, 局部放大 §5.7.13, 橡皮擦 §5.7.14.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Rect,
    RoundedRect,
    Ellipse,
    Line,
    Polyline,
    Arrow,
    DoubleArrow,
    Pencil,
    Marker,
    Mosaic,
    Blur,
    Text,
    Number,
    Zoom,
    Eraser,
}

pub const KINDS: &[Kind] = &[
    Kind::Rect,
    Kind::RoundedRect,
    Kind::Ellipse,
    Kind::Line,
    Kind::Polyline,
    Kind::Arrow,
    Kind::DoubleArrow,
    Kind::Pencil,
    Kind::Marker,
    Kind::Mosaic,
    Kind::Blur,
    Kind::Text,
    Kind::Number,
    Kind::Zoom,
    Kind::Eraser,
];

impl Kind {
    /// Tools whose geometry is a dragged stroke, so they carry a brush
    /// (§5.7.9 step 2, §5.7.14 step 2) rather than a line width alone.
    pub fn is_stroke(self) -> bool {
        matches!(
            self,
            Kind::Pencil | Kind::Marker | Kind::Mosaic | Kind::Blur | Kind::Eraser
        )
    }

    /// §5.7.15 rule: straight and poly lines and arrows take their direction
    /// from the endpoints, so a rotation handle on them is a trap.
    pub fn rotates(self) -> bool {
        !matches!(self, Kind::Line | Kind::Polyline | Kind::Arrow)
    }

    /// The tool that removes pixels instead of adding them.
    pub fn is_eraser(self) -> bool {
        self == Kind::Eraser
    }

    /// Kinds whose body is a filled area, so clicking anywhere inside them
    /// selects. A plain rectangle has to be filled (§5.7.2 step 4) to count.
    pub fn interior_is_target(self) -> bool {
        matches!(
            self,
            Kind::Mosaic | Kind::Blur | Kind::Zoom | Kind::Text | Kind::Number
        )
    }
}

/// Where the element sits. Four shapes cover every tool: a box, a two-point
/// line, a freehand path, and the zoom pair of rects.
#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    Rect(PhysRect),
    Segment {
        a: PhysPoint,
        b: PhysPoint,
    },
    Path(Vec<PhysPoint>),
    /// §5.7.13: `from` is the source region of the picture, `to` where the
    /// enlarged copy is drawn.
    Zoom {
        from: PhysRect,
        to: PhysRect,
    },
}

impl Geom {
    /// Everything the element can touch, ignoring the pen. [`Element::bounds`]
    /// adds the stroke width on top of this.
    pub fn bounds(&self) -> PhysRect {
        match self {
            Geom::Rect(r) => *r,
            Geom::Segment { a, b } => PhysRect::from_points(*a, *b),
            Geom::Zoom { from, to } => from.union(to),
            Geom::Path(points) => {
                let mut iter = points.iter();
                let Some(first) = iter.next() else {
                    return PhysRect::default();
                };
                let (mut l, mut t, mut rr, mut b) = (first.x, first.y, first.x, first.y);
                for p in iter {
                    l = l.min(p.x);
                    t = t.min(p.y);
                    rr = rr.max(p.x);
                    b = b.max(p.y);
                }
                PhysRect::from_points(PhysPoint::new(l, t), PhysPoint::new(rr, b))
            }
        }
    }

    pub fn translate(&self, dx: i32, dy: i32) -> Geom {
        match self {
            Geom::Rect(r) => Geom::Rect(r.offset(dx, dy)),
            Geom::Segment { a, b } => Geom::Segment {
                a: a.translate(dx, dy),
                b: b.translate(dx, dy),
            },
            Geom::Path(points) => Geom::Path(points.iter().map(|p| p.translate(dx, dy)).collect()),
            Geom::Zoom { from, to } => Geom::Zoom {
                from: from.offset(dx, dy),
                to: to.offset(dx, dy),
            },
        }
    }

    /// Is `p` within `slop` pixels of the drawn edge? `slop` comes from
    /// [`Style::reach`], which is already about half the pen, so this is the
    /// band the tool actually painted.
    pub fn near(&self, p: PhysPoint, slop: u32) -> bool {
        match self {
            Geom::Rect(r) => {
                r.inflate(slop).contains(p) && !r.deflate(slop.saturating_add(1)).contains(p)
            }
            Geom::Segment { a, b } => distance_to_segment(p, *a, *b) <= slop as f64,
            Geom::Path(points) => {
                if points.len() < 2 {
                    return points
                        .first()
                        .map(|f| {
                            PhysRect::from_points(*f, f.translate(1, 1))
                                .inflate(slop)
                                .contains(p)
                        })
                        .unwrap_or(false);
                }
                points
                    .windows(2)
                    .any(|w| distance_to_segment(p, w[0], w[1]) <= slop as f64)
            }
            // The zoom copy is a solid rectangle of pixels, and the source
            // region is outlined by the connector (§5.7.13 step 5).
            Geom::Zoom { from, to } => {
                to.inflate(slop).contains(p) || from.inflate(slop).contains(p)
            }
        }
    }

    /// The area a fill or an opaque body covers, for the selection hit test.
    pub fn interior(&self, kind: Kind, p: PhysPoint) -> bool {
        match self {
            Geom::Rect(r) | Geom::Zoom { to: r, .. } => match kind {
                Kind::Ellipse => {
                    if r.w < 2 || r.h < 2 {
                        return r.contains(p);
                    }
                    let nx =
                        (p.x as f64 + 0.5 - (r.x as f64 + r.w as f64 / 2.0)) / (r.w as f64 / 2.0);
                    let ny =
                        (p.y as f64 + 0.5 - (r.y as f64 + r.h as f64 / 2.0)) / (r.h as f64 / 2.0);
                    nx * nx + ny * ny <= 1.0
                }
                _ => r.contains(p),
            },
            Geom::Segment { .. } | Geom::Path { .. } => false,
        }
    }
}

/// §5.7.2 step 2 and §5.7.4: 实线 or 虚线.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dash {
    #[default]
    Solid,
    /// On/off lengths in physical pixels.
    Dash(u32, u32),
    Dot,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// §5.7.6 step 2: 单向、双向或其他箭头样式.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArrowHead {
    #[default]
    Triangle,
    Chevron,
    Circle,
}

/// §5.7.7/§5.7.8/§5.7.9/§5.7.14 all carry a brush: size, shape and a soft edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brush {
    pub size: u32,
    /// §5.7.8 rule: 圆形或椭圆形 — an ellipse falls out of a non-square mask.
    pub shape: Shape,
    /// 0 is a hard edge; the marker wants a soft one.
    pub feather: u32,
}

impl Default for Brush {
    fn default() -> Self {
        Brush {
            size: 16,
            shape: Shape::Circle,
            feather: 0,
        }
    }
}

/// Everything a tool can be set to. One flat struct rather than a per-kind
/// style enum, because §5.7.16 lets one edit change several fields at once and
/// [`crate::annotation::Command::Style`] has to snapshot the whole thing to be
/// undoable. Fields that do not apply to a kind are simply unused.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// Pen colour with its own alpha (§5.7.2 step 2 透明度).
    pub color: [u8; 4],
    /// §5.7.2 step 4: 可选择是否填充内部区域.
    pub fill: Option<[u8; 4]>,
    /// Line width in physical pixels (§5.7.20).
    pub width: u32,
    pub dash: Dash,
    /// §5.7.3 step 2.
    pub corner_radius: u32,
    /// §5.7.11 step 4: 字体、字号.
    pub font_family: String,
    pub font_size: u32,
    pub align: Align,
    /// §5.7.11 rule: 无背景或填充色.
    pub text_bg: Option<[u8; 4]>,
    /// §5.7.11 rule: 文本描边支持颜色和宽度.
    pub text_outline: Option<([u8; 4], u32)>,
    /// §5.7.9 像素化块大小 / §5.7.10 模糊强度 — one element is either, never both.
    pub effect: Effect,
    pub brush: Brush,
    pub arrow_head: ArrowHead,
    /// §5.7.13 step 5: 放大倍数 as a percent, 100 = no change.
    pub zoom_percent: u32,
    /// §5.7.13 step 5: 连接线.
    pub connection_line: bool,
    /// §5.7.14 rule: 擦除到透明. Off by default, and the UI has to explain the
    /// consequence before it can be turned on.
    pub erase_to_transparent: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            color: [232, 17, 35, 255],
            fill: None,
            width: 3,
            dash: Dash::default(),
            corner_radius: 8,
            font_family: "Microsoft YaHei UI".into(),
            font_size: 18,
            align: Align::default(),
            text_bg: None,
            text_outline: None,
            effect: Effect::Mosaic { block: 8 },
            brush: Brush::default(),
            arrow_head: ArrowHead::default(),
            zoom_percent: 200,
            connection_line: true,
            erase_to_transparent: false,
        }
    }
}

impl Style {
    /// How far this style reaches outside the geometry, for a kind that drags a
    /// brush. Box tools only pay for the pen.
    pub fn reach_for(&self, kind: Kind) -> u32 {
        let pen = self.width.div_ceil(2).max(1);
        if kind.is_stroke() {
            pen.max(self.brush.size.div_ceil(2))
                .saturating_add(self.brush.feather / 2)
        } else if matches!(kind, Kind::Arrow | Kind::DoubleArrow) {
            pen.max(head_reach(self.width))
        } else {
            pen
        }
    }
}

/// How far an arrow head sticks out past the line it caps.
///
/// The head is drawn by the rasteriser and paid for here, in one number, so the
/// repaint box of an arrow always contains the head it is about to draw. An
/// arrow whose head was clipped by its own bounds would leave the cropped stub
/// behind when it was undone.
pub fn head_reach(width: u32) -> u32 {
    width.saturating_mul(2).max(6)
}

/// §5.7.15. Carried from the first commit even though MVP does not expose the
/// handle: the pin snapshot and the Phase 2 renderer both need it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transform {
    /// Degrees, clockwise, unnormalised so a drag reverses exactly.
    pub rotation: f64,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl Transform {
    pub fn is_identity(&self) -> bool {
        self.rotation == 0.0 && !self.flip_h && !self.flip_v
    }

    /// Object space to picture space about `pivot`: flip, then rotate.
    pub fn to_picture(&self, pivot: (f64, f64), x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (
            if self.flip_h {
                pivot.0 - x
            } else {
                x - pivot.0
            },
            if self.flip_v {
                pivot.1 - y
            } else {
                y - pivot.1
            },
        );
        let rad = self.rotation.to_radians();
        let (cos, sin) = (rad.cos(), rad.sin());
        (dx * cos - dy * sin + pivot.0, dx * sin + dy * cos + pivot.1)
    }

    /// Picture space back to the untransformed object: undo the rotation, then
    /// the flip.
    pub fn to_object(&self, pivot: (f64, f64), x: f64, y: f64) -> (f64, f64) {
        let rad = -self.rotation.to_radians();
        let (cos, sin) = (rad.cos(), rad.sin());
        let (dx, dy) = (x - pivot.0, y - pivot.1);
        let (mut ux, mut uy) = (dx * cos - dy * sin, dx * sin + dy * cos);
        if self.flip_h {
            ux = -ux;
        }
        if self.flip_v {
            uy = -uy;
        }
        (ux + pivot.0, uy + pivot.1)
    }

    /// A picture point back into the untransformed object of `r`.
    /// [`Element::hit`] uses this so a turned object is picked where it is *seen*,
    /// not where it was drawn, and the rasteriser uses the same pivot.
    pub fn invert_point(&self, r: &PhysRect, p: PhysPoint) -> PhysPoint {
        if self.is_identity() {
            return p;
        }
        let (x, y) = self.to_object(r.pivot(), p.x as f64, p.y as f64);
        PhysPoint::new(x.round() as i32, y.round() as i32)
    }

    /// The box the picture can see of a rect after this transform. Rotating about
    /// the centre keeps the centre, so the footprint is concentric with the rect
    /// it came from and an undo can find what the turned object covered.
    pub fn footprint(&self, r: &PhysRect) -> PhysRect {
        if self.is_identity() {
            return *r;
        }
        let pivot = r.pivot();
        let corners = [
            (r.x as f64, r.y as f64),
            (r.right() as f64, r.y as f64),
            (r.x as f64, r.bottom() as f64),
            (r.right() as f64, r.bottom() as f64),
        ];
        let (mut l, mut t, mut rr, mut bb) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in corners {
            let (rx, ry) = self.to_picture(pivot, x, y);
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
}

/// One annotated object.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub id: u64,
    pub kind: Kind,
    pub geom: Geom,
    pub style: Style,
    /// Painting order; ties break on `id` so the order stays deterministic.
    pub z: i32,
    pub visible: bool,
    /// §5.7.17: a locked object ignores clicks and cannot be edited.
    pub locked: bool,
    pub transform: Transform,
    /// §5.7.11 step 3.
    pub text: String,
    /// §5.7.12: the value shown, for numbering only.
    pub number: Option<u32>,
}

impl Element {
    pub fn new(id: u64, kind: Kind, geom: Geom, style: Style) -> Self {
        Element {
            id,
            kind,
            geom,
            z: 0,
            visible: true,
            locked: false,
            transform: Transform::default(),
            text: String::new(),
            number: None,
            style,
        }
    }

    pub fn reach(&self) -> u32 {
        self.style.reach_for(self.kind)
    }

    /// The object's own box before it is turned: geometry plus pen reach. This is
    /// the box whose centre the transform turns about, so the rasteriser and
    /// [`Element::bounds`] have to agree on it to the pixel.
    pub fn pen_box(&self) -> PhysRect {
        self.geom.bounds().inflate(self.reach())
    }

    /// The region a repaint has to cover: geometry, pen reach and, for something
    /// that has been turned, the box it lands in afterwards. §5.7.15 rotates 文本、
    /// 矩形、椭圆、编号 and 放大区域, and a repaint box that only knew the
    /// unrotated rect would leave the turned corners drawn and stale.
    pub fn bounds(&self) -> PhysRect {
        self.transform.footprint(&self.pen_box())
    }

    /// §5.7.16 step 2: one click picks the object under it.
    pub fn hit(&self, p: PhysPoint) -> bool {
        if !self.visible {
            return false;
        }
        let p = self.transform.invert_point(&self.geom.bounds(), p);
        let solid = self.kind.interior_is_target() || self.style.fill.is_some();
        (solid && self.geom.interior(self.kind, p)) || self.geom.near(p, self.reach())
    }
}

/// The picture being annotated plus its objects.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// The base bitmap's bounds, origin at (0,0). Plan §6.4 calls this
    /// `base_size`; a rect is the same information with the geometry helpers
    /// attached, which makes a crop just a different rect.
    pub base: PhysRect,
    pub elements: Vec<Element>,
    /// §5.7.12 step 2: 起始编号.
    pub next_counter: u32,
    next_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        Document::new(0, 0)
    }
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        Document {
            base: PhysRect::new(0, 0, width, height),
            elements: Vec::new(),
            next_counter: 1,
            next_id: 1,
        }
    }

    pub fn base_size(&self) -> (u32, u32) {
        (self.base.w, self.base.h)
    }

    pub fn set_counter_start(&mut self, start: u32) {
        self.next_counter = start;
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// The smallest rect covering every drawing, clipped to nothing when there
    /// is nothing to draw. This is the whole-layer dirty rect.
    pub fn dirty(&self) -> Option<PhysRect> {
        let mut acc: Option<PhysRect> = None;
        for e in self.elements.iter().filter(|e| e.visible) {
            let b = e.bounds();
            acc = Some(match acc {
                Some(a) => a.union(&b),
                None => b,
            });
        }
        acc.and_then(|r| r.intersection(&self.base))
    }

    /// Paint order, ascending. Stable, so equal `z` keeps insertion order.
    pub fn paint_order(&self) -> Vec<&Element> {
        let mut v: Vec<&Element> = self.elements.iter().filter(|e| e.visible).collect();
        v.sort_by(|a, b| a.z.cmp(&b.z).then(a.id.cmp(&b.id)));
        v
    }

    pub fn get(&self, id: u64) -> Option<&Element> {
        self.elements.iter().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Element> {
        self.elements.iter_mut().find(|e| e.id == id)
    }

    /// §5.7.17: a locked object can be selected but not edited, so every
    /// mutation goes through this gate.
    pub fn can_edit(&self, id: u64) -> bool {
        self.get(id).is_some_and(|e| !e.locked)
    }

    fn next_z(&self) -> i32 {
        self.elements.iter().map(|e| e.z).max().unwrap_or(0) + 1
    }

    /// Insert keeping the element's own id and number — used by
    /// [`Command::Add`] and by undo, so a redo restores the exact element
    /// rather than a re-numbered copy of it.
    pub(crate) fn insert(&mut self, e: Element) {
        self.next_id = self.next_id.max(e.id + 1);
        if let Some(n) = e.number {
            self.next_counter = self.next_counter.max(n + 1);
        }
        self.elements.push(e);
    }

    pub(crate) fn take(&mut self, id: u64) -> Option<Element> {
        let at = self.elements.iter().position(|e| e.id == id)?;
        Some(self.elements.remove(at))
    }

    /// A new object from the current tool state. Numbers take the next value in
    /// sequence (§5.7.12 step 4).
    pub fn add(&mut self, kind: Kind, geom: Geom, style: Style) -> Element {
        let id = self.next_id;
        self.next_id += 1;
        let mut e = Element::new(id, kind, geom, style);
        e.z = self.next_z();
        if kind == Kind::Number {
            e.number = Some(self.next_counter);
            self.next_counter += 1;
        }
        self.elements.push(e.clone());
        e
    }

    /// §5.7.17 step 3: every object the marquee touches. Selection is not
    /// mutation, so locked objects are listed too and [`Document::can_edit`]
    /// decides what may be done to them.
    pub fn in_region(&self, r: &PhysRect) -> Vec<u64> {
        self.paint_order()
            .into_iter()
            .filter(|e| r.intersection(&e.bounds()).is_some())
            .map(|e| e.id)
            .collect()
    }

    /// §5.7.16 step 2: the topmost editable object under the point.
    pub fn topmost_at(&self, p: PhysPoint) -> Option<u64> {
        self.paint_order()
            .into_iter()
            .rev()
            .filter(|e| !e.locked)
            .find(|e| e.hit(p))
            .map(|e| e.id)
    }

    /// §5.7.12 step 5: after a middle number is deleted, the rest can be
    /// renumbered from `start` in creation order. Returns what actually moved,
    /// so a renumber that changes nothing produces no command to record.
    pub fn renumber(&mut self, start: u32) -> Vec<(u64, Option<u32>, Option<u32>)> {
        let mut ids: Vec<u64> = self
            .elements
            .iter()
            .filter(|e| e.kind == Kind::Number)
            .map(|e| e.id)
            .collect();
        ids.sort_unstable();
        let mut pairs = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let to = start + i as u32;
            if let Some(e) = self.get_mut(*id) {
                if e.number != Some(to) {
                    pairs.push((*id, e.number, Some(to)));
                    e.number = Some(to);
                }
            }
        }
        // The counter never rewinds, same as an id: undoing a renumber must not
        // make the next click produce a number that already exists on screen.
        self.next_counter = self.next_counter.max(start + ids.len() as u32);
        pairs
    }

    /// §5.7.18 step 3: copies with fresh ids, shifted, above the originals. A
    /// cloned number keeps its value — renumbering is a separate choice.
    pub fn clone_elements(&mut self, ids: &[u64], dx: i32, dy: i32) -> Vec<u64> {
        let mut copies: Vec<Element> = Vec::new();
        for id in ids {
            let Some(mut c) = self.get(*id).cloned() else {
                continue;
            };
            let from = c.geom.clone();
            c.id = self.next_id;
            self.next_id += 1;
            c.z = self.next_z();
            c.geom = from.translate(dx, dy);
            copies.push(c);
        }
        let out = copies.iter().map(|e| e.id).collect();
        self.elements.extend(copies);
        out
    }
}

/// Distance from a pixel centre to the segment `a`—`b`, in pixels.
fn distance_to_segment(p: PhysPoint, a: PhysPoint, b: PhysPoint) -> f64 {
    let (px, py) = (p.x as f64 + 0.5, p.y as f64 + 0.5);
    let (ax, ay) = (a.x as f64 + 0.5, a.y as f64 + 0.5);
    let (bx, by) = (b.x as f64 + 0.5, b.y as f64 + 0.5);
    let (vx, vy) = (bx - ax, by - ay);
    let len2 = vx * vx + vy * vy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * vx + (py - ay) * vy) / len2).clamp(0.0, 1.0)
    };
    ((px - (ax + t * vx)).powi(2) + (py - (ay + t * vy)).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: u32, h: u32) -> Geom {
        Geom::Rect(PhysRect::new(x, y, w, h))
    }

    #[test]
    fn ids_and_numbers_are_handed_out_in_sequence() {
        let mut doc = Document::new(100, 100);
        let a = doc.add(Kind::Rect, r(0, 0, 10, 10), Style::default());
        let b = doc.add(Kind::Number, r(20, 20, 10, 10), Style::default());
        let c = doc.add(Kind::Number, r(40, 40, 10, 10), Style::default());
        assert_eq!((a.id, b.id, c.id), (1, 2, 3));
        assert_eq!((b.number, c.number), (Some(1), Some(2)));
        doc.set_counter_start(7);
        assert_eq!(
            doc.add(Kind::Number, r(0, 0, 4, 4), Style::default())
                .number,
            Some(7)
        );
    }

    #[test]
    fn renumbering_follows_creation_order_not_position() {
        let mut doc = Document::new(100, 100);
        for x in [0, 60, 30] {
            doc.add(Kind::Number, r(x, 0, 8, 8), Style::default());
        }
        let third = doc.take(3).unwrap();
        assert_eq!(third.number, Some(3));
        let pairs = doc.renumber(1);
        assert!(pairs.is_empty(), "already 1 and 2: nothing to record");
        assert_eq!(
            doc.elements.iter().map(|e| e.number).collect::<Vec<_>>(),
            vec![Some(1), Some(2)]
        );
        // Renumbering down to 1 and 2 does not pull the counter back: the next
        // click still has to produce a number nothing else is wearing.
        assert_eq!(doc.next_counter, 4);
        let pairs = doc.renumber(10);
        assert_eq!(pairs, vec![(1, Some(1), Some(10)), (2, Some(2), Some(11)),]);
        assert_eq!(doc.next_counter, 12);
        // Undoing it must not hand the same number out twice.
        doc.renumber(1);
        assert_eq!(doc.next_counter, 12);
    }

    #[test]
    fn painting_order_is_by_z_then_id() {
        let mut doc = Document::new(50, 50);
        let a = doc.add(Kind::Rect, r(0, 0, 8, 8), Style::default());
        let b = doc.add(Kind::Rect, r(0, 0, 8, 8), Style::default());
        let ids: Vec<u64> = doc.paint_order().iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![a.id, b.id]);
        doc.get_mut(a.id).unwrap().z = 9;
        let ids: Vec<u64> = doc.paint_order().iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![b.id, a.id]);
        doc.get_mut(a.id).unwrap().visible = false;
        let ids: Vec<u64> = doc.paint_order().iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![b.id]);
    }

    #[test]
    fn a_hollow_rect_is_picked_on_its_edge_only() {
        let mut doc = Document::new(100, 100);
        let style = Style {
            width: 4,
            ..Style::default()
        };
        let id = doc.add(Kind::Rect, r(10, 10, 40, 30), style).id;
        // reach of a 4 px pen is 2, so the ring spans 8..52 by 8..42.
        assert_eq!(doc.topmost_at(PhysPoint::new(10, 25)), Some(id));
        assert_eq!(doc.topmost_at(PhysPoint::new(9, 25)), Some(id));
        assert_eq!(doc.topmost_at(PhysPoint::new(6, 25)), None);
        // The middle of a hollow box is not a hit …
        assert_eq!(doc.topmost_at(PhysPoint::new(30, 25)), None);
        // … and becomes one the moment it is filled (§5.7.2 step 4).
        doc.get_mut(id).unwrap().style.fill = Some([0, 0, 0, 128]);
        assert_eq!(doc.topmost_at(PhysPoint::new(30, 25)), Some(id));
    }

    #[test]
    fn an_ellipse_uses_the_curve_for_its_fill_and_the_box_for_its_edge() {
        let mut doc = Document::new(100, 100);
        let style = Style {
            width: 2,
            ..Style::default()
        };
        let id = doc.add(Kind::Ellipse, r(10, 10, 40, 20), style).id;
        // Outside the box is never a hit; the box band is the selection edge.
        assert_eq!(doc.topmost_at(PhysPoint::new(5, 20)), None);
        assert_eq!(doc.topmost_at(PhysPoint::new(10, 20)), Some(id));
        doc.get_mut(id).unwrap().style.fill = Some([0, 0, 0, 255]);
        // The flat shoulder of the ellipse: inside the box, outside the curve,
        // and beyond the pen band — so the fill genuinely does not reach it.
        assert_eq!(doc.topmost_at(PhysPoint::new(12, 12)), None);
        assert_eq!(doc.topmost_at(PhysPoint::new(30, 20)), Some(id));
    }

    #[test]
    fn a_locked_object_is_click_through_but_still_selectable() {
        let mut doc = Document::new(100, 100);
        let id = doc.add(Kind::Rect, r(10, 10, 20, 20), Style::default()).id;
        doc.get_mut(id).unwrap().locked = true;
        assert_eq!(doc.topmost_at(PhysPoint::new(10, 20)), None);
        assert_eq!(doc.in_region(&PhysRect::new(0, 0, 100, 100)), vec![id]);
        assert!(!doc.can_edit(id));
        let other = doc.add(Kind::Rect, r(1, 1, 2, 2), Style::default()).id;
        assert!(doc.can_edit(other));
        assert!(!doc.can_edit(999));
        assert!(doc.in_region(&PhysRect::new(60, 60, 10, 10)).is_empty());
    }

    #[test]
    fn a_stroke_bounds_includes_the_brush_and_feather() {
        let style = Style {
            brush: Brush {
                size: 10,
                shape: Shape::Circle,
                feather: 4,
            },
            ..Style::default()
        };
        let e = Element::new(
            1,
            Kind::Pencil,
            Geom::Path(vec![PhysPoint::new(10, 10), PhysPoint::new(20, 10)]),
            style,
        );
        // max(pen 2, brush 5) + feather/2 = 7.
        assert_eq!(e.reach(), 7);
        assert_eq!(e.bounds(), PhysRect::new(3, 3, 24, 14));
        assert!(e.hit(PhysPoint::new(15, 4)));
        assert!(!e.hit(PhysPoint::new(15, 2)));
        // A box tool with the same style ignores the brush entirely.
        let box_e = Element::new(2, Kind::Rect, r(0, 0, 4, 4), e.style.clone());
        assert_eq!(box_e.reach(), 2);
    }

    #[test]
    fn a_single_dot_stroke_is_still_clickable() {
        let mut doc = Document::new(50, 50);
        let style = Style {
            width: 6,
            brush: Brush {
                size: 6,
                shape: Shape::Circle,
                feather: 0,
            },
            ..Style::default()
        };
        let id = doc
            .add(
                Kind::Pencil,
                Geom::Path(vec![PhysPoint::new(20, 20)]),
                style,
            )
            .id;
        assert_eq!(
            doc.get(id).unwrap().geom.bounds(),
            PhysRect::new(20, 20, 0, 0)
        );
        assert_eq!(doc.topmost_at(PhysPoint::new(22, 21)), Some(id));
        assert_eq!(doc.topmost_at(PhysPoint::new(30, 30)), None);
    }

    #[test]
    fn the_dirty_rect_covers_everything_and_clips_to_the_picture() {
        let mut doc = Document::new(40, 40);
        assert_eq!(doc.dirty(), None);
        doc.add(Kind::Rect, r(5, 5, 10, 10), Style::default());
        assert_eq!(doc.dirty(), Some(PhysRect::new(3, 3, 14, 14)));
        let id = doc.add(Kind::Rect, r(30, 30, 20, 20), Style::default()).id;
        // The second box hangs off the picture, so the union is clipped to it.
        assert_eq!(doc.dirty(), Some(PhysRect::new(3, 3, 37, 37)));
        doc.get_mut(id).unwrap().visible = false;
        assert_eq!(doc.dirty(), Some(PhysRect::new(3, 3, 14, 14)));
    }

    #[test]
    fn cloning_shifts_reids_and_puts_the_copy_on_top() {
        let mut doc = Document::new(100, 100);
        let id = doc.add(Kind::Rect, r(10, 10, 20, 20), Style::default()).id;
        let copies = doc.clone_elements(&[id], 30, 0);
        assert_eq!(copies, vec![2]);
        let copy = doc.get(2).unwrap();
        assert_eq!(copy.geom, r(40, 10, 20, 20));
        assert!(copy.z > doc.get(id).unwrap().z);
        assert_eq!(doc.len(), 2);
        let n = doc.add(Kind::Number, r(0, 0, 4, 4), Style::default()).id;
        let c = doc.clone_elements(&[n], 1, 0)[0];
        assert_eq!(doc.get(c).unwrap().number, doc.get(n).unwrap().number);
        assert_ne!(c, n);
        // An unknown id clones to nothing rather than cloning the wrong thing.
        assert!(doc.clone_elements(&[999], 1, 1).is_empty());
    }

    #[test]
    fn translating_a_path_moves_every_point() {
        let g = Geom::Path(vec![PhysPoint::new(1, 1), PhysPoint::new(3, 5)]);
        assert_eq!(
            g.translate(2, -1),
            Geom::Path(vec![PhysPoint::new(3, 0), PhysPoint::new(5, 4)])
        );
        assert_eq!(g.bounds(), PhysRect::new(1, 1, 2, 4));
        assert_eq!(Geom::Path(vec![]).bounds(), PhysRect::default());
    }

    #[test]
    fn a_zoom_element_carries_both_regions_and_hits_either() {
        let mut doc = Document::new(200, 200);
        let g = Geom::Zoom {
            from: PhysRect::new(10, 10, 20, 20),
            to: PhysRect::new(100, 100, 40, 40),
        };
        let id = doc.add(Kind::Zoom, g, Style::default()).id;
        assert_eq!(
            doc.get(id).unwrap().geom.bounds(),
            PhysRect::new(10, 10, 130, 130)
        );
        assert_eq!(doc.topmost_at(PhysPoint::new(15, 15)), Some(id));
        assert_eq!(doc.topmost_at(PhysPoint::new(110, 110)), Some(id));
        assert_eq!(doc.topmost_at(PhysPoint::new(70, 70)), None);
    }

    #[test]
    fn kinds_decide_what_a_handle_can_do() {
        assert!(!Kind::Line.rotates());
        assert!(!Kind::Polyline.rotates());
        assert!(!Kind::Arrow.rotates());
        assert!(Kind::Rect.rotates());
        assert!(Kind::Text.rotates());
        assert!(Kind::Number.rotates());
        assert!(Kind::Zoom.rotates());
        assert!(Kind::Pencil.is_stroke());
        assert!(Kind::Eraser.is_stroke());
        assert!(Kind::Eraser.is_eraser());
        assert!(!Kind::Marker.is_eraser());
        assert!(!Kind::Text.is_stroke());
        assert!(Kind::Mosaic.interior_is_target());
        assert!(!Kind::Arrow.interior_is_target());
        assert!(Transform::default().is_identity());
    }

    #[test]
    fn a_segment_is_picked_near_the_line_not_its_bounding_box() {
        let mut doc = Document::new(100, 100);
        let style = Style {
            width: 2,
            ..Style::default()
        };
        let id = doc
            .add(
                Kind::Line,
                Geom::Segment {
                    a: PhysPoint::new(10, 10),
                    b: PhysPoint::new(90, 10),
                },
                style,
            )
            .id;
        assert_eq!(doc.topmost_at(PhysPoint::new(50, 11)), Some(id));
        // The box of a horizontal line is tall enough to swallow this point if
        // the hit test is lazy.
        assert_eq!(doc.topmost_at(PhysPoint::new(50, 60)), None);
        assert_eq!(doc.topmost_at(PhysPoint::new(95, 10)), None);
    }
}
