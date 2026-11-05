//! The gestures a *selected* object answers to: 控制点 (§5.7.17 改尺寸), 旋转控制点
//! or an angle (§5.7.15), and 多选 (§5.7.17 steps 2-4).
//!
//! The model already carried [`crate::annotation::Transform`] and the commands
//! [`crate::annotation::Command::Resize`] / [`crate::annotation::Command::Rotate`]
//! for it, but nothing could *produce* them: `resize_to_rect` takes a finished rect,
//! which a drag does not have — a drag has one grabbed control point and how far the
//! pointer moved. This module is the author of that step, and the reason it is in
//! `falcon-core` rather than in the mask is that every rule in it is a rule the PRD
//! states and a headless test can check: which handle an arrow owns, how short a box
//! may be dragged, which objects a rotation handle may not be put on.
//!
//! Coordinates are the same picture-local pixels [`crate::annotation::model`] uses.
//! A grip's *position* is answered in picture space — where the user sees it, after
//! the element's own turn — while the drag it produces is undone back into object
//! space before it moves the geometry, so a handle of a turned rectangle travels the
//! way the pointer does and not the way the rectangle was drawn.

use super::command::{corner_command, Command};
use super::model::{
    drawn_corner, head_reach, Document, Element, Geom, Kind, Style, Transform, CORNER_MAX,
    CORNER_MIN,
};
use crate::geometry::{resize, Handle, PhysPoint, PhysRect};

/// The smallest box a control-point drag may leave behind.
///
/// Two, not one: at one pixel the two edges a grip pulls and the edge it holds are the
/// same pixel, so the handle that just moved has nowhere to be grabbed again — and an
/// ellipse or a filled box of one pixel is a shape that cannot be hit at all
/// ([`Geom::interior`] starts asking for the curve at w ≥ 2).
pub const MIN_SIZE: u32 = 2;

/// How close the pointer has to come to a grip for the grip to take the press, in
/// picture pixels. The mask's own selection uses
/// [`crate::geometry::DEFAULT_HIT_SLOP`] for a rectangle it drew at full size; an
/// annotation is often a 3 px ring around nothing, so its handles need the same
/// generosity in absolute rather than relative terms.
pub const GRIP_SLOP: u32 = 6;

/// How far outside the box the rotation handle sits, so pressing a corner turns the
/// object rather than resizing it.
pub const ROTATE_OFFSET: i32 = 20;

/// 圆角那一枚把手离它所在那一枚角把手至少这么远。
///
/// 把手是排在框把手**前面**还是**后面**决定谁抢到这一枚按下，而 [`grip_at`] 先到先得：一枚
/// 住在 [`GRIP_SLOP`] 圈里的圆角把手永远是握不到的那一只。地板取两倍容差，为的是"角先让开、
/// 圆角再谈"这件事不必由排序去赌。代价说清楚：半径小于这一档时，把手画在地板那一格而不是画在
/// 弧上 —— 拖出来的数仍是**按下那一刻**的半径加上指针横着走过的那一段，所以读数不会跳。
pub const RADIUS_GRIP_INSET: u32 = GRIP_SLOP * 2;

/// The angle range 输入旋转角度 accepts, in degrees either way: one full turn in the
/// direction the user dragged. Anything past it is the same picture, and a number that
/// keeps growing is a number the readout cannot show.
pub const ROTATE_LIMIT: f64 = 360.0;

/// One control point of one object.
///
/// A box's eight are [`crate::geometry::Handle`]'s own — the same spelling, the same
/// corner arithmetic and the same author for the position of a corner — because the
/// mask already hit-tests a rectangle with them and a second eight-corner enum is a
/// second list to keep in step. The other three are what an *annotation* has and a
/// selection rectangle does not: the two ends of a straight or double-headed line, the
/// turn that §5.7.15 puts on everything except those lines, and the 圆角半径 of §5.7.3
/// step 4 that only a 圆角矩形 owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// One of the eight. `Handle::Body` is never used here: pressing the body of an
    /// object selects or moves it (§5.7.16 step 3), which is [`Command::Move`]'s job.
    Box(Handle),
    /// `Geom::Segment`'s `a` — the tail, and the end a plain line drag started at.
    EndA,
    /// `Geom::Segment`'s `b` — the head an arrow carries.
    EndB,
    /// §5.7.15 step 2's 旋转控制点.
    Rotate,
    /// §5.7.3 step 4's「通过控制点调整……圆角」. It moves a **style** number
    /// ([`Style::corner_radius`]) and not the geometry, which is why [`resize_grip`]
    /// refuses it and [`radius_grip_command`] is its own author.
    Radius,
}

impl Grip {
    /// The eight box grips, in [`Handle::all`]'s order.
    pub fn box_grips() -> [Grip; 8] {
        Handle::all().map(Grip::Box)
    }

    /// 圆角那一枚把手住在哪儿：沿着对角线往里走，走到**画得出来的那一枚弧**的圆心上
    /// ([`drawn_corner`]，也就是光栅真拿去画角的那个数)，再被 [`RADIUS_GRIP_INSET`] 那枚
    /// 地板托住 —— 半径为 0 时那枚圆心正是那个角，而角把手排在前面，那一格就成了永远握不到
    /// 的一格。
    fn radius_point(r: &PhysRect, radius: u32) -> PhysPoint {
        let inset = drawn_corner(radius, r).max(RADIUS_GRIP_INSET) as i32;
        PhysPoint::new(r.x + inset, r.y + inset)
    }

    /// The point on `r` this box grip owns: a corner for the four corners, the middle of
    /// the edge for the four edges. Written here rather than borrowed from
    /// [`crate::geometry::Anchor`], because `Anchor` is the corner a *typed* size keeps
    /// still and this is the place a dragged handle starts from - and `geometry::resize`
    /// reads only the coordinate its own handle owns, so the other one is free to be
    /// wherever the user can actually aim.
    pub fn box_point(r: &PhysRect, h: Handle) -> PhysPoint {
        let cx = r.x + r.w as i32 / 2;
        let cy = r.y + r.h as i32 / 2;
        let x = match h {
            Handle::Nw | Handle::W | Handle::Sw => r.x,
            Handle::Ne | Handle::E | Handle::Se => r.right(),
            _ => cx,
        };
        let y = match h {
            Handle::Nw | Handle::N | Handle::Ne => r.y,
            Handle::Sw | Handle::S | Handle::Se => r.bottom(),
            _ => cy,
        };
        PhysPoint::new(x, y)
    }

    pub fn of_box(handle: Handle) -> Option<Grip> {
        match handle {
            Handle::Body => None,
            h => Some(Grip::Box(h)),
        }
    }

    /// The spelling a log or a test names this grip by, so a failure says which handle
    /// it dragged rather than which variant of an enum it built.
    pub fn key(self) -> &'static str {
        match self {
            Grip::Box(h) => match h {
                Handle::Nw => "nw",
                Handle::N => "n",
                Handle::Ne => "ne",
                Handle::E => "e",
                Handle::Se => "se",
                Handle::S => "s",
                Handle::Sw => "sw",
                Handle::W => "w",
                Handle::Body => "body",
            },
            Grip::EndA => "end_a",
            Grip::EndB => "end_b",
            Grip::Rotate => "rotate",
            Grip::Radius => "radius",
        }
    }

    /// Which handles this object actually owns.
    ///
    /// A segment gets its two ends and no box: §5.7.15's rule 直线和箭头优先通过端点调整方向
    /// is why a line has no corner to drag — a box around a diagonal line has corners
    /// that are not on the line, and a handle that moves the *bounding box* of a stroke
    /// the user is trying to steer is a handle that does something else.
    ///
    /// The 圆角半径 handle goes **first**, ahead of the eight: it lives inside the top-left
    /// corner, and a handle the corner's own hit band swallows is a handle that can never
    /// be taken ([`grip_at`] answers with the first grip that reaches). It is offered only
    /// where there is room for it — a box too small to hold the handle outside the corner's
    /// [`GRIP_SLOP`] band has no second handle to grab, and its eight corners stay honest.
    pub fn grips_for(e: &Element) -> Vec<Grip> {
        let mut out = Vec::new();
        match &e.geom {
            Geom::Segment { .. } => {
                out.push(Grip::EndA);
                out.push(Grip::EndB);
            }
            _ => {
                if e.kind == Kind::RoundedRect
                    && e.geom.bounds().w >= RADIUS_GRIP_INSET * 2
                    && e.geom.bounds().h >= RADIUS_GRIP_INSET * 2
                {
                    out.push(Grip::Radius);
                }
                out.extend(Self::box_grips());
            }
        }
        if e.kind.rotates() {
            out.push(Grip::Rotate);
        }
        out
    }
}

/// Where one grip is on the picture: the object-space point the geometry owns, then
/// the element's own turn, about the same pivot [`super::raster`] blits about
/// ([`PhysRect::pivot`] of the geometry's bounds, which is concentric with
/// [`Element::pen_box`] because an inflate is symmetric).
///
/// `None` is not an error but an answer: this object has no such handle.
pub fn grip_point(e: &Element, grip: Grip) -> Option<PhysPoint> {
    let b = e.geom.bounds();
    let obj = match grip {
        Grip::Box(h) => match &e.geom {
            Geom::Segment { .. } => return None,
            Geom::Zoom { to, .. } => Grip::box_point(to, h),
            _ => Grip::box_point(&b, h),
        },
        Grip::EndA => match &e.geom {
            Geom::Segment { a, .. } => *a,
            _ => return None,
        },
        Grip::EndB => match &e.geom {
            Geom::Segment { b, .. } => *b,
            _ => return None,
        },
        Grip::Rotate => {
            if !e.kind.rotates() {
                return None;
            }
            let n = Grip::box_point(&b, Handle::N);
            PhysPoint::new(n.x, n.y - ROTATE_OFFSET)
        }
        Grip::Radius => {
            if e.kind != Kind::RoundedRect {
                return None;
            }
            match &e.geom {
                Geom::Rect(r) => Grip::radius_point(r, e.style.corner_radius),
                // 圆角矩形没有第二种几何：一段线、一条轨迹、一对放大框都没有角可以拧。
                _ => return None,
            }
        }
    };
    if e.transform.is_identity() {
        return Some(obj);
    }
    let (x, y) = e
        .transform
        .to_picture(b.pivot(), obj.x as f64, obj.y as f64);
    Some(PhysPoint::new(x.round() as i32, y.round() as i32))
}

/// The hottest grip under `p`, corners and ends before the turn that sits outside them.
/// `None` means the press is on the body or on nothing, and is [`Document::topmost_at`]'s
/// question instead.
pub fn grip_at(e: &Element, p: PhysPoint, slop: u32) -> Option<Grip> {
    Grip::grips_for(e).into_iter().find(|g| {
        grip_point(e, *g)
            .is_some_and(|q| (q.x - p.x).abs() <= slop as i32 && (q.y - p.y).abs() <= slop as i32)
    })
}

/// The same question for a list of objects, taking the topmost object's grip: a grip
/// that is under another object's body is still that object's grip, or the last handle
/// the user grabbed could not be grabbed again.
pub fn grip_at_any(doc: &Document, ids: &[u64], p: PhysPoint, slop: u32) -> Option<(u64, Grip)> {
    doc.paint_order()
        .into_iter()
        .rev()
        .filter(|e| ids.contains(&e.id))
        .find_map(|e| grip_at(e, p, slop).map(|g| (e.id, g)))
}

/// A travel of the pointer in picture space as the object sees it: the turn undone,
/// then the flips. Written as a vector difference of [`Transform::to_object`] so the
/// one author of the angle stays [`Transform`].
fn to_object_delta(e: &Element, dx: i32, dy: i32) -> (i32, i32) {
    if e.transform.is_identity() {
        return (dx, dy);
    }
    let b = e.geom.bounds();
    let pivot = b.pivot();
    let (x, y) = e
        .transform
        .to_object(pivot, pivot.0 + dx as f64, pivot.1 + dy as f64);
    ((x - pivot.0).round() as i32, (y - pivot.1).round() as i32)
}

/// The edge a box grip pulls.
fn pulls_left(h: Handle) -> bool {
    matches!(h, Handle::Nw | Handle::W | Handle::Sw)
}

fn pulls_top(h: Handle) -> bool {
    matches!(h, Handle::Nw | Handle::N | Handle::Ne)
}

/// The other two edges a grip may be the one that moves. [`geometry::resize`] takes the
/// coordinate its handle owns from the dragged point and nothing else, so which point
/// that is stays this module's question.
fn pulls_right(h: Handle) -> bool {
    matches!(h, Handle::Ne | Handle::E | Handle::Se)
}

fn pulls_bottom(h: Handle) -> bool {
    matches!(h, Handle::Sw | Handle::S | Handle::Se)
}

/// The wall a box grip is dragged up to: the opposite edge, less [`MIN_SIZE`].
///
/// A grip pulled *through* the edge it holds still would otherwise leave a box whose
/// corners have swapped — `resize` normalises the inversion into a big box on the far
/// side, which is what a rubber-band 框选 wants and not what 改尺寸 of something already
/// drawn is. §5.7.17 step 3 asks for the smallest box that can still be grabbed, so the
/// point is stopped short of the wall and the drag simply ceases to shrink that axis.
fn wall(r: &PhysRect, h: Handle, p: PhysPoint) -> PhysPoint {
    let m = MIN_SIZE as i32;
    PhysPoint::new(
        if pulls_left(h) {
            p.x.min(r.right() - m)
        } else if pulls_right(h) {
            p.x.max(r.x + m)
        } else {
            p.x
        },
        if pulls_top(h) {
            p.y.min(r.bottom() - m)
        } else if pulls_bottom(h) {
            p.y.max(r.y + m)
        } else {
            p.y
        },
    )
}

/// The corner a box grip holds still — the point the object does not move, which for a
/// freehand path is also the point the path is scaled about.
fn opposite(h: Handle) -> Handle {
    match h {
        Handle::Nw => Handle::Se,
        Handle::N => Handle::S,
        Handle::Ne => Handle::Sw,
        Handle::E => Handle::W,
        Handle::Se => Handle::Nw,
        Handle::S => Handle::N,
        Handle::Sw => Handle::Ne,
        Handle::W => Handle::E,
        Handle::Body => Handle::Body,
    }
}

/// Pull the dragged edges back until the box is at least [`MIN_SIZE`] on each axis it
/// was asked to change, keeping the corner the grip does not own.
fn with_min(r: &PhysRect, h: Handle, min: u32, bounds: &PhysRect) -> PhysRect {
    let m = min as i32;
    let (mut l, mut t, mut rr, mut bb) = r.ltrb();
    if rr - l < m {
        if pulls_left(h) {
            // The right edge is the one this grip does not touch, so it pays: the left
            // edge comes in with it, up to the picture's own left boundary.
            l = rr.max(bounds.x + m) - m;
        } else {
            rr = l + m;
        }
    }
    if bb - t < m {
        if pulls_top(h) {
            t = bb.max(bounds.y + m) - m;
        } else {
            bb = t + m;
        }
    }
    PhysRect::new(l, t, (rr - l).max(0) as u32, (bb - t).max(0) as u32)
}

/// How short a two-point object may be dragged.
///
/// An arrow's head is drawn *along* its shaft for [`head_reach`] pixels, so a shaft
/// shorter than that puts the head past the tail and the arrow points at nothing. The
/// floor is therefore the pen's own head reach, which is why this takes the width and
/// not a constant: a thicker pen draws a bigger head and has to stop sooner. A plain
/// line has no head and stops at [`MIN_SIZE`].
pub fn segment_floor(kind: Kind, width: u32) -> f64 {
    match kind {
        Kind::Arrow | Kind::DoubleArrow => head_reach(width) as f64,
        _ => MIN_SIZE as f64,
    }
}

/// Move one endpoint and keep the shaft at least [`segment_floor`] long, sliding the
/// moved end back out along the direction it was going rather than refusing the drag.
///
/// `was` is the segment as it stood before the drag: a shaft dragged to zero length has
/// no direction of its own left to keep, and the orientation it had is the only answer
/// to "which way was this arrow pointing".
fn keep_floor(
    a: PhysPoint,
    b: PhysPoint,
    was: (PhysPoint, PhysPoint),
    moved_head: bool,
    floor: f64,
) -> (PhysPoint, PhysPoint) {
    let (dx, dy) = (b.x as f64 - a.x as f64, b.y as f64 - a.y as f64);
    let len = (dx * dx + dy * dy).sqrt();
    if len >= floor {
        return (a, b);
    }
    let (ox, oy) = (
        was.1.x as f64 - was.0.x as f64,
        was.1.y as f64 - was.0.y as f64,
    );
    let ol = (ox * ox + oy * oy).sqrt();
    let (ux, uy) = if len > 0.0 {
        (dx / len, dy / len)
    } else if ol > 0.0 {
        (ox / ol, oy / ol)
    } else {
        // A dot dragged into a dot: east is as honest a direction as any, and the floor
        // still gets respected so the head cannot land on the tail.
        (1.0, 0.0)
    };
    if moved_head {
        (
            a,
            PhysPoint::new(
                (a.x as f64 + ux * floor).round() as i32,
                (a.y as f64 + uy * floor).round() as i32,
            ),
        )
    } else {
        (
            PhysPoint::new(
                (b.x as f64 - ux * floor).round() as i32,
                (b.y as f64 - uy * floor).round() as i32,
            ),
            b,
        )
    }
}

/// §5.7.17 step 3's 改尺寸: the geometry one grip produces when the pointer has travelled
/// `dx`, `dy` since the grip was taken.
///
/// `bounds` is the picture: the same rule that keeps a stroke inside the selection while
/// it is drawn ([`Layer::doc_point`] in the bridge) applies to a drag of something
/// already drawn, and ink outside the hole leaves the export anyway.
///
/// `None` says the object has no such handle — a box grip on a line, an end on a
/// rectangle, a resize on the turn handle or on the 圆角半径 handle, whose number lives in
/// the style and not in the geometry ([`radius_grip_command`] is that one's author).
pub fn resize_grip(e: &Element, grip: Grip, dx: i32, dy: i32, bounds: &PhysRect) -> Option<Geom> {
    let (odx, ody) = to_object_delta(e, dx, dy);
    match (grip, &e.geom) {
        (Grip::Rotate, _) | (Grip::Radius, _) => None,
        (Grip::Box(h), Geom::Segment { .. }) if h != Handle::Body => None,
        (Grip::EndA, _) | (Grip::EndB, _) if !matches!(e.geom, Geom::Segment { .. }) => None,
        (Grip::Box(h), Geom::Rect(r)) | (Grip::Box(h), Geom::Zoom { to: r, .. }) => {
            let next = resize(
                r,
                h,
                wall(r, h, Grip::box_point(r, h).translate(odx, ody)),
                bounds,
            );
            Some(match &e.geom {
                Geom::Zoom { from, .. } => Geom::Zoom {
                    from: *from,
                    to: with_min(&next, h, MIN_SIZE, bounds),
                },
                _ => Geom::Rect(with_min(&next, h, MIN_SIZE, bounds)),
            })
        }
        (Grip::EndA, Geom::Segment { a: a0, b: b0 })
        | (Grip::EndB, Geom::Segment { a: a0, b: b0 }) => {
            let moved = matches!(grip, Grip::EndB);
            let (na, nb) = if moved {
                (
                    *a0,
                    PhysPoint::new(
                        b0.x.saturating_add(odx).clamp(bounds.x, bounds.right() - 1),
                        b0.y.saturating_add(ody)
                            .clamp(bounds.y, bounds.bottom() - 1),
                    ),
                )
            } else {
                (
                    PhysPoint::new(
                        a0.x.saturating_add(odx).clamp(bounds.x, bounds.right() - 1),
                        a0.y.saturating_add(ody)
                            .clamp(bounds.y, bounds.bottom() - 1),
                    ),
                    *b0,
                )
            };
            let (a, b) = keep_floor(
                na,
                nb,
                (*a0, *b0),
                moved,
                segment_floor(e.kind, e.style.width),
            );
            Some(Geom::Segment { a, b })
        }
        (Grip::Box(h), Geom::Path(points)) => {
            let old = e.geom.bounds();
            let next = with_min(
                &resize(
                    &old,
                    h,
                    wall(&old, h, Grip::box_point(&old, h).translate(odx, ody)),
                    bounds,
                ),
                h,
                MIN_SIZE,
                bounds,
            );
            let anchor = Grip::box_point(&old, opposite(h));
            // An axis with no extent cannot be scaled — dividing by the width of a
            // vertical scribble is how a drag along it would fling every point to
            // infinity — so that axis travels with the grip. By the pointer's own travel,
            // not by how far the box's left edge came: every point of a flat scribble sits
            // *on* the edge the user grabbed, and `resize` reads a drag through that edge
            // as the opposite edge moving, which leaves `next.x` exactly where it was.
            let slide_x = if pulls_left(h) || pulls_right(h) {
                odx
            } else {
                0
            };
            let slide_y = if pulls_top(h) || pulls_bottom(h) {
                ody
            } else {
                0
            };
            let sx = if old.w == 0 {
                1.0
            } else {
                next.w as f64 / old.w as f64
            };
            let sy = if old.h == 0 {
                1.0
            } else {
                next.h as f64 / old.h as f64
            };
            Some(Geom::Path(
                points
                    .iter()
                    .map(|p| {
                        PhysPoint::new(
                            if old.w == 0 {
                                p.x + slide_x
                            } else {
                                anchor.x + ((p.x - anchor.x) as f64 * sx).round() as i32
                            },
                            if old.h == 0 {
                                p.y + slide_y
                            } else {
                                anchor.y + ((p.y - anchor.y) as f64 * sy).round() as i32
                            },
                        )
                    })
                    .collect(),
            ))
        }
        _ => None,
    }
}

/// The command one control-point drag files: a single [`Command::Resize`] over the one
/// object, so one Ctrl+Z gives the old geometry back exactly.
///
/// A drag that lands where the object already is files nothing — the same rule that
/// keeps a click which drew nothing out of the undo stack (§5.7.19).
pub fn resize_grip_command(
    doc: &Document,
    id: u64,
    grip: Grip,
    dx: i32,
    dy: i32,
) -> Option<Command> {
    if !doc.can_edit(id) {
        return None;
    }
    let e = doc.get(id)?;
    let to = resize_grip(e, grip, dx, dy, &doc.base)?;
    if to == e.geom {
        return None;
    }
    super::command::resize_command(doc, &[(id, to)])
}

// A multi-object resize is deliberately not offered here: §5.7.17 step 4 gives a marquee
// 移动、复制、删除或样式调整 and never a shared control point, and scaling a group would have
// to decide what happens to a pen width and a font size — two settings the PRD puts in
// the *style*, not the geometry. [`Selection::bounds`] is still the box the UI draws.

/// §5.7.3 step 4's「通过控制点调整……圆角」: the radius a drag of that handle asks for,
/// filed as the one [`Command::Style`] the ladder's own cells file — so §5.7.19's undo
/// coverage does not have to learn a second kind of step to cover this one.
///
/// `start_radius` is the number the element held **when the handle was taken** and not the
/// one it holds now: the handle's *position* is floored at [`RADIUS_GRIP_INSET`] so the
/// corner grips stay reachable, and a drag measured against the live number would jump the
/// moment the number passes that floor. The travel is read on **one axis** — the handle
/// sits on the box's diagonal, so a pointer that wanders vertically while pulling outward
/// is not asked to mean something as well. (The picture-space vector is still undone
/// through the element's turn first, which is the only way a handle of a rotated box
/// travels the way the pointer does.)
///
/// The clamp is the ladder's own ([`Style::set_corner_radius`], i.e.
/// [`CORNER_MIN`]..=[`CORNER_MAX`]) narrowed to what this box can draw — [`drawn_corner`]
/// of [`CORNER_MAX`], the same expression the renderer and the handle's position use. A
/// ceiling that ignores `start_radius` would take a number away on a drag that only ever
/// pulled outward, which is the one direction the picture cannot argue about.
pub fn radius_grip_command(
    doc: &Document,
    id: u64,
    start_radius: u32,
    dx: i32,
    dy: i32,
) -> Option<Command> {
    if !doc.can_edit(id) {
        return None;
    }
    let e = doc.get(id)?;
    // Only the tool §5.7.3 step 4 names has a corner to round: a drag that somehow lands
    // on another object's handle changes nothing rather than writing a number the renderer
    // of that tool would ignore.
    if e.kind != Kind::RoundedRect {
        return None;
    }
    let (odx, _) = to_object_delta(e, dx, dy);
    // 把手那一根轴上没走 = 这一拖没有要说的数。为什么不交给下面那道夹子去判等：`start_radius`
    // 是**握住把手那一刻**的那一枚数，而窗上那一枚可能已经在此期间被别的手改过（拖一次、再按
    // 同一处原封不动就是这一臂），于是「零行程」会被读成「改回握住时那一枚」—— 那是页上看不
    // 出来的第三件事，也是白占一格撤销栈的第三件事（#453：两种失败保持分开）。
    if odx == 0 {
        return None;
    }
    let ceiling = drawn_corner(CORNER_MAX, &e.geom.bounds()).max(start_radius);
    let asked = i64::from(start_radius) + i64::from(odx);
    corner_command(doc, &[id], asked.clamp(i64::from(CORNER_MIN), i64::from(ceiling)) as u32)
}

// A 圆角 handle is offered to one object at a time for the same reason a resize is: §5.7.17
// step 4 gives a marquee 样式调整 through the ladder row, which settles on **one** number,
// while this handle is about the shape of the box under the pointer.

/// The angle a drag of the turn handle asks for: degrees clockwise from the object's
/// upright, measured about the same pivot the renderer turns about.
///
/// North of the centre is 0°, due east is 90° — the direction the handle is *pointed*,
/// not the distance travelled, so a drag that starts anywhere on the circle and ends
/// due east leaves the object at 90° whatever angle it began at. That is what makes the
/// same number usable for §5.7.15 step 2's 输入旋转角度.
pub fn rotation_at(rect: &PhysRect, p: PhysPoint) -> f64 {
    let (cx, cy) = rect.pivot();
    let (dx, dy) = (p.x as f64 + 0.5 - cx, p.y as f64 + 0.5 - cy);
    dx.atan2(-dy).to_degrees()
}

/// [`ROTATE_LIMIT`]'s two ends, applied to a number a field or a drag produced.
pub fn clamp_rotation(degrees: f64) -> f64 {
    degrees.clamp(-ROTATE_LIMIT, ROTATE_LIMIT)
}

/// §5.7.15: turn the selected objects to an absolute angle about their own centres.
///
/// *Each object about its own centre*, which is what [`Transform`] already means here
/// (`to_picture` turns about the rect it is given): a marquee of three boxes that were
/// never lined up does not become one fan.
///
/// Objects whose kind does not rotate — 直线、折线、箭头 — are left alone rather than
/// turned anyway, and the whole selection being of that kind produces no command at all.
pub fn rotate_command(doc: &Document, ids: &[u64], degrees: f64) -> Option<Command> {
    let degrees = clamp_rotation(degrees);
    let mut pairs = Vec::new();
    for id in ids {
        let Some(e) = doc.get(*id) else { continue };
        if !doc.can_edit(*id) || !e.kind.rotates() {
            continue;
        }
        let to = Transform {
            rotation: degrees,
            ..e.transform
        };
        if to != e.transform {
            pairs.push((*id, e.transform, to));
        }
    }
    if pairs.is_empty() {
        return None;
    }
    Some(Command::Rotate(pairs))
}

/// The turn one drag of the handle produces, from the object's own box and the point the
/// pointer came up at.
pub fn rotate_at_command(doc: &Document, id: u64, pointer: PhysPoint) -> Option<Command> {
    let e = doc.get(id)?;
    let degrees = rotation_at(&e.geom.bounds(), pointer);
    rotate_command(doc, &[id], degrees)
}

/// §5.7.18 step 1's 选中一个或多个: the set of objects the edits are applied to.
///
/// A set of ids rather than a list of elements, because the document is the author of
/// what an object *is* — a selection that held copies would still be describing the
/// geometry the user moved five seconds ago. Ids are unique ([`Document::get`]) and
/// stable, and an id whose object has gone is simply not editable, which is what
/// [`Document::can_edit`] already says about an unknown one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    ids: Vec<u64>,
}

impl Selection {
    pub fn new() -> Self {
        Selection::default()
    }

    /// One object, as §5.7.16 step 2's single click leaves it.
    pub fn one(id: u64) -> Self {
        Selection { ids: vec![id] }
    }

    /// A list, deduplicated, in the order given: a marquee's result comes out of
    /// [`Document::in_region`] already in paint order, and re-sorting it here would put
    /// the clone's ids in a different order than the copy was made in.
    pub fn of(ids: &[u64]) -> Self {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        Selection { ids: out }
    }

    pub fn ids(&self) -> &[u64] {
        &self.ids
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn contains(&self, id: u64) -> bool {
        self.ids.contains(&id)
    }

    pub fn clear(&mut self) {
        self.ids.clear();
    }

    /// Replace the selection with one object.
    pub fn select(&mut self, id: u64) {
        self.ids = vec![id];
    }

    /// §5.7.17 step 3's additive selection: keep what is already held and take one more.
    pub fn add(&mut self, id: u64) {
        if !self.contains(id) {
            self.ids.push(id);
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.ids.retain(|i| *i != id);
    }

    /// Add it if it is not in, drop it if it is — the click-with-a-modifier behaviour.
    pub fn toggle(&mut self, id: u64) {
        if self.contains(id) {
            self.remove(id);
        } else {
            self.add(id);
        }
    }

    /// §5.7.16 step 2: the object under one click, topmost first. Empty when the click
    /// landed on nothing, which is also §5.7.16 step 4's 点击空白区域完成编辑.
    pub fn at(doc: &Document, p: PhysPoint) -> Self {
        match doc.topmost_at(p) {
            Some(id) => Selection::one(id),
            None => Selection::new(),
        }
    }

    /// §5.7.17 steps 2-3: everything a dragged rectangle touches. Locked objects are in
    /// the list — being selected is not being edited — and [`Document::can_edit`] is what
    /// keeps them from moving when the group is dragged.
    pub fn marquee(doc: &Document, r: &PhysRect) -> Self {
        Selection::of(&doc.in_region(r))
    }

    /// The box the selection occupies on the picture, or `None` when nothing in it is
    /// there any more. This is the rectangle the handles are drawn on and the region a
    /// group drag repaints.
    pub fn bounds(&self, doc: &Document) -> Option<PhysRect> {
        let mut acc: Option<PhysRect> = None;
        for id in &self.ids {
            if let Some(e) = doc.get(*id) {
                let b = e.bounds();
                acc = Some(match acc {
                    Some(a) => a.union(&b),
                    None => b,
                });
            }
        }
        acc
    }

    /// The object the panel shows its values for: the **topmost** thing in the
    /// selection (§5.7.16 step 2's 单击已有标注对象 picks exactly that one), not the first
    /// id — a marquee comes out of [`Document::in_region`] in paint order, and showing the
    /// colour of the box underneath the one the user can see would be a panel that
    /// describes something else.
    pub fn topmost<'a>(&self, doc: &'a Document) -> Option<&'a Element> {
        doc.paint_order()
            .into_iter()
            .rev()
            .find(|e| self.contains(e.id))
    }

    /// §5.7.17 step 4's 移动, one command for the whole group: locked objects are skipped
    /// by [`Command`] builder, and one Ctrl+Z takes the whole move back.
    pub fn move_by(&self, doc: &Document, dx: i32, dy: i32) -> Option<Command> {
        super::command::move_command(doc, self.ids(), dx, dy)
    }

    /// §5.7.15 for a group.
    pub fn rotate(&self, doc: &Document, degrees: f64) -> Option<Command> {
        rotate_command(doc, self.ids(), degrees)
    }

    /// §5.7.17 step 4's 删除.
    pub fn delete(&self, doc: &Document) -> Option<Command> {
        super::command::remove_command(doc, self.ids())
    }

    /// §5.7.12 step 5 那一枚「是否自动重排」亮着时的删除：删掉这串，同时把它们腾出来的空档
    /// 补上，两件事记成**一条**命令（[`super::command::delete_and_renumber_command`]），于是
    /// 一次 Ctrl+Z 撤的是那一次编辑、不是它的两半。与 [`Selection::delete`] 分成两个入口而不
    /// 是给后者加一枚参数：那一个问的是「删除」本身，多的这一句问的是编号那一排，而回答它的
    /// 是开关亮不亮——由调用方挑入口，比让每一位调用方都替编号那一排表态少写一份。
    pub fn delete_renumbering(&self, doc: &Document) -> Option<Command> {
        super::command::delete_and_renumber_command(doc, self.ids())
    }

    /// §5.7.16 step 3's 样式调整, one command over the whole group. `None` when the patch
    /// would change nothing, which is what keeps a wheel that spun onto the value already
    /// shown from becoming an undo step.
    pub fn restyle<F>(&self, doc: &Document, patch: F) -> Option<Command>
    where
        F: Fn(&Element, &mut Style),
    {
        super::command::patch_style_command(doc, self.ids(), patch)
    }

    /// §5.7.18 step 3's 副本: the command's own [`Command::Add`] list carries the fresh
    /// ids, so the caller can move the copies next (step 4) without this type having to
    /// guess what they were called. Applying the command is what files them — asking for
    /// the copies leaves the document as it was.
    pub fn clone_by(&self, doc: &Document, dx: i32, dy: i32) -> Option<Command> {
        super::command::clone_command(doc, self.ids(), dx, dy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::command::{merge_styles, remove_command};
    use crate::annotation::model::{alpha_from_pct, Brush, Style, MARKER_ALPHA};
    use crate::imageops::Effect;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> PhysRect {
        PhysRect::new(x, y, w, h)
    }

    fn element(kind: Kind, geom: Geom, style: Style) -> Element {
        Element::new(1, kind, geom, style)
    }

    /// A 2 px pen, so the reach inflation in the bounds tests is a known number.
    fn pen() -> Style {
        Style {
            width: 2,
            ..Style::default()
        }
    }

    #[test]
    fn a_box_grip_follows_the_pointer_and_holds_the_opposite_corner() {
        let e = element(Kind::Rect, Geom::Rect(rect(10, 10, 20, 20)), pen());
        let base = rect(0, 0, 100, 100);
        // §5.7.17 step 3: dragging the south-east corner moves only that corner.
        assert_eq!(
            resize_grip(&e, Grip::Box(Handle::Se), 5, 7, &base),
            Some(Geom::Rect(rect(10, 10, 25, 27)))
        );
        // The north-west corner moves both edges it owns and leaves the other two.
        assert_eq!(
            resize_grip(&e, Grip::Box(Handle::Nw), -3, -2, &base),
            Some(Geom::Rect(rect(7, 8, 23, 22)))
        );
        // A single edge changes one dimension only.
        assert_eq!(
            resize_grip(&e, Grip::Box(Handle::E), 4, 40, &base),
            Some(Geom::Rect(rect(10, 10, 24, 20))),
            "the 直线 an edge grip owns is the one it drags"
        );
        // An ellipse and a text box take the same eight, because their geometry is a box.
        for kind in [Kind::Ellipse, Kind::Text, Kind::Number, Kind::RoundedRect] {
            let shaped = element(kind, Geom::Rect(rect(10, 10, 20, 20)), pen());
            assert_eq!(
                resize_grip(&shaped, Grip::Box(Handle::Se), 1, 1, &base),
                Some(Geom::Rect(rect(10, 10, 21, 21))),
                "{kind:?} resizes through the box its tool drew"
            );
        }
    }

    #[test]
    fn a_grip_dragged_through_the_box_stops_at_the_smallest_it_may_be() {
        let e = element(Kind::Rect, Geom::Rect(rect(10, 10, 20, 20)), pen());
        let base = rect(0, 0, 100, 100);
        // South-east dragged a hundred pixels up-and-left: the north-west corner stays,
        // and the box cannot go below MIN_SIZE on either axis.
        assert_eq!(
            resize_grip(&e, Grip::Box(Handle::Se), -100, -100, &base),
            Some(Geom::Rect(rect(10, 10, MIN_SIZE, MIN_SIZE))),
            "a box that collapses to nothing cannot be grabbed again"
        );
        // The other way round: the north-west corner is pulled *through* the south-east,
        // and what holds is the corner this grip does not own - the box stops on that
        // side of it instead of turning inside out and growing the other way.
        assert_eq!(
            resize_grip(&e, Grip::Box(Handle::Nw), 100, 100, &base),
            Some(Geom::Rect(rect(28, 28, MIN_SIZE, MIN_SIZE)))
        );
        // A one-pixel nudge is still a nudge.
        assert_eq!(
            resize_grip(&e, Grip::Box(Handle::E), 1, 0, &base),
            Some(Geom::Rect(rect(10, 10, 21, 20)))
        );
    }

    #[test]
    fn the_picture_edge_is_where_a_drag_stops() {
        let e = element(Kind::Rect, Geom::Rect(rect(90, 90, 20, 20)), pen());
        let base = rect(0, 0, 100, 100);
        let Some(Geom::Rect(r)) = resize_grip(&e, Grip::Box(Handle::Se), 50, 50, &base) else {
            panic!("a corner of a box always resizes");
        };
        assert_eq!(r.right(), 100);
        assert_eq!(r.bottom(), 100);
    }

    #[test]
    fn a_line_is_steeried_by_its_ends_and_an_arrow_keeps_room_for_its_head() {
        let line = element(
            Kind::Line,
            Geom::Segment {
                a: PhysPoint::new(10, 10),
                b: PhysPoint::new(60, 10),
            },
            pen(),
        );
        let base = rect(0, 0, 100, 100);
        assert_eq!(
            resize_grip(&line, Grip::EndB, 5, 3, &base),
            Some(Geom::Segment {
                a: PhysPoint::new(10, 10),
                b: PhysPoint::new(65, 13)
            })
        );
        assert_eq!(
            resize_grip(&line, Grip::EndA, -2, 0, &base),
            Some(Geom::Segment {
                a: PhysPoint::new(8, 10),
                b: PhysPoint::new(60, 10)
            })
        );
        // A box grip on a line is the wrong question, and an end on a box is too.
        assert_eq!(resize_grip(&line, Grip::Box(Handle::Se), 4, 4, &base), None);
        assert_eq!(resize_grip(&line, Grip::Rotate, 4, 4, &base), None);
        assert_eq!(
            resize_grip(
                &element(Kind::Rect, Geom::Rect(rect(0, 0, 4, 4)), pen()),
                Grip::EndB,
                1,
                1,
                &base
            ),
            None
        );
        // The head of an arrow is drawn along the shaft for `head_reach(width)` pixels, so
        // dragging the head onto the tail stops with the head exactly long enough: pulled
        // to 15 it is 5 px from the tail, and the answer is 6, on the side it came from.
        let arrow = element(
            Kind::Arrow,
            Geom::Segment {
                a: PhysPoint::new(10, 10),
                b: PhysPoint::new(60, 10),
            },
            pen(),
        );
        let floor = segment_floor(Kind::Arrow, 2);
        assert_eq!(floor, head_reach(2) as f64, "the floor is the head");
        let Some(Geom::Segment { a, b }) = resize_grip(&arrow, Grip::EndB, -45, 0, &base) else {
            panic!("an arrow head always drags");
        };
        assert_eq!((a.x, a.y), (10, 10), "the tail did not move");
        assert_eq!((b.x - a.x) as f64, floor, "the shaft is exactly head reach");
        // A plain line has no head, and stops at the smallest grabbable length instead.
        assert_eq!(segment_floor(Kind::Line, 2), MIN_SIZE as f64);
        let Some(Geom::Segment { b, .. }) = resize_grip(&line, Grip::EndB, -49, 0, &base) else {
            panic!();
        };
        assert_eq!(b.x - 10, MIN_SIZE as i32);
        // A fatter pen draws a bigger head, and so stops sooner: the floor is the
        // element's own width, not a constant.
        assert!(segment_floor(Kind::Arrow, 20) > segment_floor(Kind::Arrow, 2));
    }

    #[test]
    fn a_path_grip_scales_every_point_about_the_corner_that_stays() {
        let scribble = element(
            Kind::Pencil,
            Geom::Path(vec![
                PhysPoint::new(10, 10),
                PhysPoint::new(20, 15),
                PhysPoint::new(15, 20),
            ]),
            pen(),
        );
        let base = rect(0, 0, 100, 100);
        // The box is 10 by 10; pulling south-east out to 20 by 20 doubles every point
        // about the north-west corner, which is the one this grip does not own.
        let Some(Geom::Path(points)) = resize_grip(&scribble, Grip::Box(Handle::Se), 10, 10, &base)
        else {
            panic!("a scribble has corners");
        };
        assert_eq!(
            points,
            vec![
                PhysPoint::new(10, 10),
                PhysPoint::new(30, 20),
                PhysPoint::new(20, 30)
            ]
        );
        assert_eq!(scribble.geom.bounds(), rect(10, 10, 10, 10));
        assert_eq!(Geom::Path(points.clone()).bounds(), rect(10, 10, 20, 20));
        // An axis with no extent cannot be scaled — that is a division by zero dressed as
        // a resize — so it travels with the edge the user dragged instead.
        let bar = element(
            Kind::Pencil,
            Geom::Path(vec![PhysPoint::new(10, 10), PhysPoint::new(10, 30)]),
            pen(),
        );
        let Some(Geom::Path(points)) = resize_grip(&bar, Grip::Box(Handle::W), 5, 0, &base) else {
            panic!();
        };
        assert_eq!(
            points,
            vec![PhysPoint::new(15, 10), PhysPoint::new(15, 30)],
            "the vertical stroke slid right with its left edge"
        );
        // §5.7.15's rule, in the other direction: a polyline is steered by its ends, so
        // the freehand scale path is what a 铅笔 and a 马克笔 and a 橡皮 all share.
        for kind in [Kind::Marker, Kind::Eraser, Kind::Mosaic] {
            let stroked = element(
                kind,
                Geom::Path(vec![PhysPoint::new(10, 10), PhysPoint::new(20, 10)]),
                pen(),
            );
            let Some(Geom::Path(points)) =
                resize_grip(&stroked, Grip::Box(Handle::E), 10, 0, &base)
            else {
                panic!("{kind:?} keeps its corners");
            };
            assert_eq!(points[1].x, 30, "{kind:?} stretches with the grip");
            assert_eq!(points[0].x, 10, "{kind:?} keeps the edge it did not grab");
        }
    }

    #[test]
    fn a_magnifier_resizes_its_copy_and_leaves_its_source_alone() {
        let zoom = element(
            Kind::Zoom,
            Geom::Zoom {
                from: rect(10, 10, 20, 20),
                to: rect(60, 60, 40, 40),
            },
            pen(),
        );
        let base = rect(0, 0, 200, 200);
        let Some(Geom::Zoom { from, to }) = resize_grip(&zoom, Grip::Box(Handle::Se), 10, 0, &base)
        else {
            panic!("放大区域 has corners");
        };
        assert_eq!(
            from,
            rect(10, 10, 20, 20),
            "the source region is what was sampled"
        );
        assert_eq!(to, rect(60, 60, 50, 40), "the copy is what got bigger");
    }

    #[test]
    fn a_grip_is_where_the_object_is_seen_not_where_it_was_drawn() {
        let mut e = element(Kind::Rect, Geom::Rect(rect(10, 10, 20, 4)), pen());
        assert_eq!(
            grip_point(&e, Grip::Box(Handle::Nw)),
            Some(PhysPoint::new(10, 10)),
            "unturned, the corner is the corner"
        );
        e.transform.rotation = 90.0;
        // Turned a quarter about the centre (20,12), the north-west corner lands to the
        // north-east of it — and that is the point the user has to be able to find.
        assert_eq!(
            grip_point(&e, Grip::Box(Handle::Nw)),
            Some(PhysPoint::new(22, 2))
        );
        assert_eq!(
            grip_at(&e, PhysPoint::new(22, 2), GRIP_SLOP),
            Some(Grip::Box(Handle::Nw)),
            "the turned corner is picked where it is drawn"
        );
        assert_eq!(
            grip_at(&e, PhysPoint::new(10, 10), GRIP_SLOP),
            None,
            "and not where it used to be"
        );
        // The drag itself is undone back into the object: the east edge of this box is
        // drawn at the *bottom* of the turned one, so pulling it 5 px down lengthens the
        // box along its own width - the axis that edge owns - and not along its height.
        let base = rect(0, 0, 100, 100);
        assert_eq!(
            resize_grip(&e, Grip::Box(Handle::E), 0, 5, &base),
            Some(Geom::Rect(rect(10, 10, 25, 4)))
        );
        assert_eq!(
            resize_grip(&e, Grip::Box(Handle::E), 0, -5, &base),
            Some(Geom::Rect(rect(10, 10, 15, 4))),
            "and the same handle pulled up shortens the same axis, about the edge it holds"
        );
        // The turn handle is only offered to what can turn, and sits outside the box.
        assert!(Grip::grips_for(&e).contains(&Grip::Rotate));
        let turned = element(Kind::Rect, Geom::Rect(rect(10, 10, 20, 20)), pen());
        assert_eq!(
            grip_point(&turned, Grip::Rotate),
            Some(PhysPoint::new(20, -10)),
            "ROTATE_OFFSET above the top edge"
        );
        let line = element(
            Kind::Line,
            Geom::Segment {
                a: PhysPoint::new(0, 0),
                b: PhysPoint::new(10, 0),
            },
            pen(),
        );
        assert_eq!(
            Grip::grips_for(&line),
            vec![Grip::EndA, Grip::EndB],
            "§5.7.15 rule: 直线和箭头优先通过端点调整方向"
        );
        assert_eq!(grip_point(&line, Grip::Rotate), None);
    }

    #[test]
    fn the_topmost_object_s_grip_wins_a_press_two_of_them_share() {
        let mut doc = Document::new(100, 100);
        let under = doc
            .add(Kind::Rect, Geom::Rect(rect(0, 0, 20, 20)), pen())
            .id;
        // Sitting on the lower box's south-east corner, so (20, 20) is both that box's
        // 东南 and this one's 西北: the press belongs to one of the two, and it is the
        // object the user can see that gets it.
        let over = doc
            .add(Kind::Rect, Geom::Rect(rect(20, 20, 20, 20)), pen())
            .id;
        let ids = [under, over];
        assert_eq!(
            grip_at_any(&doc, &ids, PhysPoint::new(40, 40), GRIP_SLOP),
            Some((over, Grip::Box(Handle::Se))),
            "a corner only the top box has is the top box's"
        );
        assert_eq!(
            grip_at_any(&doc, &ids, PhysPoint::new(20, 20), GRIP_SLOP),
            Some((over, Grip::Box(Handle::Nw))),
            "the corner both boxes have belongs to the one on top"
        );
        assert_eq!(
            grip_at_any(&doc, &ids, PhysPoint::new(0, 0), GRIP_SLOP),
            Some((under, Grip::Box(Handle::Nw))),
            "and a grip only the bottom box has still answers, selected list and all"
        );
        assert_eq!(
            grip_at_any(&doc, &ids, PhysPoint::new(70, 70), GRIP_SLOP),
            None
        );
        // A press inside the selection list but on nothing is the body's question, not
        // the grips': that is how §5.7.16 step 3's 移动 gets to run after this answered.
        assert_eq!(
            grip_at_any(&doc, &[under], PhysPoint::new(70, 70), GRIP_SLOP),
            None
        );
    }

    #[test]
    fn turning_an_object_is_one_step_and_leaves_what_cannot_turn() {
        let mut doc = Document::new(100, 100);
        let r = doc
            .add(Kind::Rect, Geom::Rect(rect(10, 10, 20, 20)), pen())
            .id;
        let t = doc
            .add(Kind::Text, Geom::Rect(rect(40, 40, 20, 10)), pen())
            .id;
        let line = doc
            .add(
                Kind::Line,
                Geom::Segment {
                    a: PhysPoint::new(0, 50),
                    b: PhysPoint::new(30, 50),
                },
                pen(),
            )
            .id;
        let before = doc.clone();
        let cmd = rotate_command(&doc, &[r, t, line], 30.0).expect("two of three turn");
        cmd.apply(&mut doc);
        assert_eq!(doc.get(r).unwrap().transform.rotation, 30.0);
        assert_eq!(doc.get(t).unwrap().transform.rotation, 30.0);
        assert!(
            doc.get(line).unwrap().transform.is_identity(),
            "§5.7.15: 直线 takes its direction from its endpoints, so a rotation handle \
             on it is a trap"
        );
        cmd.invert(&mut doc);
        assert_eq!(doc, before, "one Ctrl+Z undoes the whole turn");
        // The same angle again is not a change, and a change that is not one is not an
        // undo step.
        assert!(rotate_command(&doc, &[line], 45.0).is_none());
        let cmd = rotate_command(&doc, &[r], 45.0).unwrap();
        cmd.apply(&mut doc);
        assert!(rotate_command(&doc, &[r], 45.0).is_none());
        // The limit is one turn either way, so the readout can always say it.
        assert_eq!(clamp_rotation(1000.0), ROTATE_LIMIT);
        assert_eq!(clamp_rotation(-1000.0), -ROTATE_LIMIT);
        let cmd = rotate_command(&doc, &[t], 1000.0).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(t).unwrap().transform.rotation, ROTATE_LIMIT);
        // A flip the object already carries survives an angle change: the turn handle
        // turns, it does not un-mirror.
        doc.get_mut(r).unwrap().transform.flip_h = true;
        let cmd = rotate_command(&doc, &[r], 12.0).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(
            doc.get(r).unwrap().transform,
            Transform {
                rotation: 12.0,
                flip_h: true,
                flip_v: false
            }
        );
    }

    #[test]
    fn the_angle_is_the_direction_the_handle_points() {
        let box_ = rect(0, 0, 20, 20);
        // North of the centre is upright: 0°, and due east is a quarter turn clockwise,
        // which is the direction `Transform::to_picture` turns in.
        assert!(rotation_at(&box_, PhysPoint::new(10, 0)).abs() < 4.0);
        assert!((rotation_at(&box_, PhysPoint::new(30, 10)) - 90.0).abs() < 4.0);
        assert!((rotation_at(&box_, PhysPoint::new(10, 30)).abs() - 180.0).abs() < 4.0);
        assert!((rotation_at(&box_, PhysPoint::new(-10, 10)).abs() - 90.0).abs() < 4.0);
    }

    #[test]
    fn a_selection_is_ids_a_box_and_one_group_move() {
        let mut doc = Document::new(100, 100);
        let a = doc
            .add(Kind::Rect, Geom::Rect(rect(10, 10, 20, 20)), pen())
            .id;
        let b = doc
            .add(Kind::Ellipse, Geom::Rect(rect(60, 60, 20, 20)), pen())
            .id;
        let c = doc
            .add(
                Kind::Line,
                Geom::Segment {
                    a: PhysPoint::new(0, 0),
                    b: PhysPoint::new(5, 0),
                },
                pen(),
            )
            .id;
        // §5.7.17 steps 2-3: the marquee takes what it touches, in paint order.
        let sel = Selection::marquee(&doc, &rect(0, 0, 100, 100));
        assert_eq!(sel.ids(), &[a, b, c]);
        assert_eq!(sel.len(), 3);
        let partial = Selection::marquee(&doc, &rect(50, 50, 40, 40));
        assert_eq!(partial.ids(), vec![b]);
        assert!(Selection::marquee(&doc, &rect(96, 0, 3, 3)).is_empty());

        // The box is the union of what is there, pen reach included (a 2 px pen is 1).
        assert_eq!(
            sel.bounds(&doc),
            Some(rect(-1, -1, 82, 82)),
            "a, b and the line at the origin"
        );
        assert!(Selection::of(&[999]).bounds(&doc).is_none());

        // One command, three objects, one undo.
        let before = doc.clone();
        let cmd = sel.move_by(&doc, 4, 6).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(a).unwrap().geom, Geom::Rect(rect(14, 16, 20, 20)));
        assert_eq!(doc.get(b).unwrap().geom, Geom::Rect(rect(64, 66, 20, 20)));
        assert_eq!(
            doc.get(c).unwrap().geom,
            Geom::Segment {
                a: PhysPoint::new(4, 6),
                b: PhysPoint::new(9, 6)
            }
        );
        cmd.invert(&mut doc);
        assert_eq!(doc, before);

        // Selection bookkeeping: dedup, toggle, and the object the panel reads.
        let mut s = Selection::of(&[a, a, b]);
        assert_eq!(s.ids(), &[a, b]);
        s.toggle(c);
        assert_eq!(s.len(), 3);
        s.toggle(c);
        assert_eq!(s.ids(), &[a, b]);
        s.select(c);
        assert_eq!(s.ids(), &[c]);
        assert!(s.contains(c) && !s.contains(a));
        s.clear();
        assert!(s.is_empty());
        // The topmost of a marquee is the one the panel shows its values for.
        let s = Selection::marquee(&doc, &rect(0, 0, 100, 100));
        assert_eq!(s.topmost(&doc).map(|e| e.id), Some(c));
        doc.get_mut(c).unwrap().visible = false;
        assert_eq!(
            s.topmost(&doc).map(|e| e.id),
            Some(b),
            "a hidden object is not the thing on screen"
        );
    }

    #[test]
    fn the_click_selects_the_object_under_it_and_a_blank_click_selects_nothing() {
        let mut doc = Document::new(100, 100);
        let a = doc
            .add(Kind::Rect, Geom::Rect(rect(10, 10, 20, 20)), pen())
            .id;
        doc.get_mut(a).unwrap().style.fill = Some([0, 0, 0, 128]);
        assert_eq!(Selection::at(&doc, PhysPoint::new(20, 20)).ids(), [a]);
        assert!(
            Selection::at(&doc, PhysPoint::new(70, 70)).is_empty(),
            "§5.7.16 step 4's 点击空白区域"
        );
    }

    #[test]
    fn cloning_a_selection_copies_every_object_as_one_step() {
        let mut doc = Document::new(100, 100);
        let a = doc
            .add(Kind::Rect, Geom::Rect(rect(10, 10, 20, 20)), pen())
            .id;
        let b = doc
            .add(Kind::Ellipse, Geom::Rect(rect(50, 50, 10, 10)), pen())
            .id;
        let sel = Selection::of(&[a, b]);
        let before = doc.clone();
        let cmd = sel.clone_by(&doc, 5, 5).expect("§5.7.18 step 3");
        cmd.apply(&mut doc);
        assert_eq!(doc.len(), 4);
        let copies: Vec<u64> = match &cmd {
            Command::Add(list) => list.iter().map(|e| e.id).collect(),
            other => panic!("{other:?}"),
        };
        assert_eq!(copies.len(), 2);
        for id in &copies {
            assert_ne!(*id, a);
            assert_ne!(*id, b);
        }
        assert_eq!(
            doc.get(copies[0]).unwrap().geom,
            Geom::Rect(rect(15, 15, 20, 20))
        );
        assert_eq!(
            doc.get(copies[1]).unwrap().geom,
            Geom::Rect(rect(55, 55, 10, 10))
        );
        // The copies are the selection now, and one Ctrl+Z takes both of them back.
        let copy_sel = Selection::of(&copies);
        assert_eq!(copy_sel.topmost(&doc).map(|e| e.id), Some(copies[1]));
        cmd.invert(&mut doc);
        // The objects come back exactly; the *counter* does not, which is the same rule
        // §5.7.12 step 5 states for a number - undoing must not put an id that was already
        // written into a file back into circulation.
        assert_eq!(
            doc.elements, before.elements,
            "one Ctrl+Z takes both of them back"
        );
        // A locked object is still cloned (§5.7.18 says 选中一个或多个, and locking one
        // forbids editing *it*, not making a copy of the selection it is in).
        doc.get_mut(a).unwrap().locked = true;
        let cmd = Selection::of(&[a]).clone_by(&doc, 1, 0).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.len(), 3);
        // Deleting the selection is its own command, and undoing it gives both back.
        let cmd = sel.delete(&doc).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(
            doc.elements
                .iter()
                .filter(|e| e.id == a || e.id == b)
                .count(),
            0
        );
        cmd.invert(&mut doc);
        assert!(doc.get(a).is_some() && doc.get(b).is_some());
        // An empty selection produces no command for any of the four edits.
        let none = Selection::new();
        assert!(none.move_by(&doc, 1, 1).is_none());
        assert!(none.rotate(&doc, 10.0).is_none());
        assert!(none.clone_by(&doc, 1, 1).is_none());
        assert!(none.delete(&doc).is_none());
        assert!(none.restyle(&doc, |_e, s| s.width = 40).is_none());
        assert!(remove_command(&doc, &[999]).is_none());
        // Which is what the next object's id says: nothing that a copy of this clone was
        // ever called is back in the box.
        let fresh = doc.add(Kind::Rect, Geom::Rect(rect(0, 0, 4, 4)), pen()).id;
        assert!(
            !copies.contains(&fresh),
            "{fresh} was already handed to a copy"
        );
    }

    #[test]
    fn a_group_style_patch_keeps_each_object_s_own_other_channels() {
        // §5.7.17 step 4's 样式调整 over a marquee, and §5.7.19's one step for it.
        let mut doc = Document::new(100, 100);
        let a = doc
            .add(
                Kind::Rect,
                Geom::Rect(rect(10, 10, 20, 20)),
                Style {
                    width: 2,
                    fill: Some(Style::translucent([232, 17, 35, 255])),
                    effect: Effect::Mosaic { block: 8 },
                    ..Style::default()
                },
            )
            .id;
        let b = doc
            .add(
                Kind::Pencil,
                Geom::Path(vec![PhysPoint::new(0, 0), PhysPoint::new(9, 9)]),
                Style {
                    width: 9,
                    brush: Brush {
                        size: 36,
                        ..Brush::default()
                    },
                    effect: Effect::Blur { radius: 12 },
                    ..Style::default()
                },
            )
            .id;
        let sel = Selection::of(&[a, b]);
        let before = doc.clone();
        let cmd = sel
            .restyle(&doc, |_e, s| s.color[3] = alpha_from_pct(50))
            .unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(a).unwrap().style.color[3], MARKER_ALPHA);
        assert_eq!(doc.get(b).unwrap().style.color[3], MARKER_ALPHA);
        assert_eq!(doc.get(a).unwrap().style.width, 2);
        assert_eq!(doc.get(b).unwrap().style.width, 9);
        assert_eq!(doc.get(b).unwrap().style.brush.size, 36);
        cmd.invert(&mut doc);
        assert_eq!(doc, before);
        // Two patches in a row still merge into one step, because the id sets match even
        // when one object's own value was already the one asked for.
        let first = sel
            .restyle(&doc, |_e, s| s.color[3] = alpha_from_pct(80))
            .unwrap();
        first.apply(&mut doc);
        let second = sel
            .restyle(&doc, |_e, s| s.color[3] = alpha_from_pct(60))
            .unwrap();
        second.apply(&mut doc);
        let merged = merge_styles(&first, &second).expect("same selection");
        merged.invert(&mut doc);
        assert_eq!(doc, before, "one Ctrl+Z for the whole wheel drag");
    }

    #[test]
    fn a_grip_drag_files_one_resize_and_undoes_to_the_geometry_it_started_from() {
        let mut doc = Document::new(100, 100);
        let id = doc
            .add(Kind::Ellipse, Geom::Rect(rect(10, 10, 30, 20)), pen())
            .id;
        let before = doc.clone();
        let cmd = resize_grip_command(&doc, id, Grip::Box(Handle::E), 6, 0).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(id).unwrap().geom, Geom::Rect(rect(10, 10, 36, 20)));
        cmd.invert(&mut doc);
        assert_eq!(doc, before);
        // Nothing moved, nothing recorded; and a locked or unknown object is not resized.
        assert!(resize_grip_command(&doc, id, Grip::Box(Handle::E), 0, 0).is_none());
        doc.get_mut(id).unwrap().locked = true;
        assert!(resize_grip_command(&doc, id, Grip::Box(Handle::E), 6, 0).is_none());
        assert!(resize_grip_command(&doc, 999, Grip::Box(Handle::E), 6, 0).is_none());
    }

    /// A 圆角矩形 of 100×60 at (10,10) — big enough for the handle to have room outside
    /// every corner band.
    fn rounded(radius: u32) -> Element {
        element(
            Kind::RoundedRect,
            Geom::Rect(rect(10, 10, 100, 60)),
            Style {
                width: 2,
                corner_radius: radius,
                ..Style::default()
            },
        )
    }

    #[test]
    fn the_radius_handle_sits_on_the_arc_the_raster_actually_draws() {
        let e = rounded(12);
        // The arc's centre is inset by the radius it is drawn with, so at 12 the handle is
        // one box-corner-diagonal of 12 px in: exactly where the curve starts to bend.
        assert_eq!(
            grip_point(&e, Grip::Radius),
            Some(PhysPoint::new(22, 22)),
            "把手住在弧的圆心上，不是住在框的角上"
        );
        // A radius of zero has no arc to sit on, and the corner it would share with the
        // 直角 is the corner grip's own square: the floor keeps the handle out of that
        // corner's hit band, so the press the user aims at 圆角 lands on 圆角.
        assert_eq!(
            grip_point(&rounded(0), Grip::Radius),
            Some(PhysPoint::new(10 + RADIUS_GRIP_INSET as i32, 10 + RADIUS_GRIP_INSET as i32))
        );
        // Past half the short side there is no bigger arc to draw either, so the handle
        // stops where the picture stops — the same number [`drawn_corner`] answers with.
        assert_eq!(
            grip_point(&rounded(64), Grip::Radius),
            Some(PhysPoint::new(40, 40)),
            "半径超过半短边时把手不跟着那个画不出来的数跑"
        );
        // Only this tool owns it, and only in the geometry that has corners.
        assert_eq!(grip_point(&rounded(12), Grip::Radius).is_some(), true);
        assert_eq!(
            grip_point(
                &element(Kind::Rect, Geom::Rect(rect(10, 10, 100, 60)), pen()),
                Grip::Radius
            ),
            None,
            "矩形那一种工具没有这一枚把手，尽管它也存着 corner_radius"
        );
        let mut line = rounded(12);
        line.kind = Kind::Line;
        line.geom = Geom::Segment {
            a: PhysPoint::new(10, 10),
            b: PhysPoint::new(60, 60),
        };
        assert_eq!(grip_point(&line, Grip::Radius), None);
    }

    #[test]
    fn the_radius_handle_is_offered_only_where_the_corners_can_still_be_taken() {
        let e = rounded(12);
        let grips = Grip::grips_for(&e);
        assert_eq!(
            grips.first(),
            Some(&Grip::Radius),
            "抢在八枚角把手之前：先到先得那一枚才是握得到的那一枚"
        );
        assert_eq!(grips.len(), 10, "九枚已有的把手加上这一枚新把手");
        // Where the press falls in both bands, the 圆角 handle wins; on the corner itself,
        // the corner stays the corner.
        let inside = PhysPoint::new(22, 22);
        assert_eq!(grip_at(&e, inside, GRIP_SLOP), Some(Grip::Radius));
        assert_eq!(
            grip_at(&e, PhysPoint::new(10, 10), GRIP_SLOP),
            Some(Grip::Box(Handle::Nw))
        );
        // The turn handle is still the outermost of them all.
        assert_eq!(
            grip_at(&e, PhysPoint::new(60, -10), GRIP_SLOP),
            Some(Grip::Rotate)
        );
        // A box too small to hold the handle clear of its corner band has no second handle:
        // its eight corners are not asked to give one up.
        let tight = element(
            Kind::RoundedRect,
            Geom::Rect(rect(10, 10, RADIUS_GRIP_INSET * 2 - 1, 100)),
            Style {
                corner_radius: 4,
                ..Style::default()
            },
        );
        assert!(!Grip::grips_for(&tight).contains(&Grip::Radius));
        assert_eq!(grip_at(&tight, PhysPoint::new(14, 14), GRIP_SLOP), Some(Grip::Box(Handle::Nw)));
        // A line, which owns two ends and no box, owns no radius either.
        assert!(!Grip::grips_for(&line_only()).contains(&Grip::Radius));
    }

    fn line_only() -> Element {
        element(
            Kind::Line,
            Geom::Segment {
                a: PhysPoint::new(10, 10),
                b: PhysPoint::new(60, 60),
            },
            pen(),
        )
    }

    #[test]
    fn a_radius_drag_moves_one_axis_and_clamps_the_way_the_ladder_clamps() {
        let mut doc = Document::new(200, 200);
        let id = doc
            .add(
                Kind::RoundedRect,
                Geom::Rect(rect(10, 10, 100, 60)),
                Style {
                    width: 2,
                    corner_radius: 12,
                    ..Style::default()
                },
            )
            .id;
        // Outward along x, measured from the radius the handle was taken at: 8 px more arc.
        let cmd = radius_grip_command(&doc, id, 12, 8, 0).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(id).unwrap().style.corner_radius, 20);
        // The other axis is not a second dial: the handle is on the diagonal, and a pointer
        // that wanders down while it pulls right is asked to mean one thing.
        cmd.invert(&mut doc);
        assert_eq!(doc.get(id).unwrap().style.corner_radius, 12);
        assert!(radius_grip_command(&doc, id, 12, 0, 40).is_none());
        // Inward, and the floor is the ladder's own floor: a直角 and nothing between.
        let square = radius_grip_command(&doc, id, 12, -50, 0).unwrap();
        square.apply(&mut doc);
        assert_eq!(doc.get(id).unwrap().style.corner_radius, CORNER_MIN);
        square.invert(&mut doc);
        // Outward past the ladder's ceiling still lands on the ladder's ceiling, and past
        // the box's own half-short-side lands on *that*: 100×60 can draw a 30 px arc and no
        // more, so a number above it is a number no picture shows.
        let big = radius_grip_command(&doc, id, 12, 500, 0).unwrap();
        big.apply(&mut doc);
        assert_eq!(
            doc.get(id).unwrap().style.corner_radius,
            drawn_corner(CORNER_MAX, &rect(10, 10, 100, 60)),
            "拖出来的数停在画得出来的那一档"
        );
        // A box whose own short side is wide enough to draw the ladder's top reaches the
        // ladder's ceiling instead, so the two clamps are an AND and not a second ruler:
        // 100×60 above stops at 30 (half the short side), 160×160 here stops at 64 (the ladder).
        let mut tall = Document::new(200, 200);
        let tall_id = tall
            .add(
                Kind::RoundedRect,
                Geom::Rect(rect(10, 10, 160, 160)),
                Style {
                    width: 2,
                    corner_radius: 8,
                    ..Style::default()
                },
            )
            .id;
        let to_the_top = radius_grip_command(&tall, tall_id, 8, 300, 0).unwrap();
        to_the_top.apply(&mut tall);
        assert_eq!(tall.get(tall_id).unwrap().style.corner_radius, CORNER_MAX);
    }

    #[test]
    fn a_handle_pulled_out_never_takes_a_number_the_ladder_already_gave() {
        // §5.7.3's row can put 64 on a box whose short side only draws 15. Grabbing the
        // handle there and pulling *outward* must not shrink the stored radius behind the
        // user's back; pulling inward is free to bring it down.
        let mut doc = Document::new(200, 200);
        let id = doc
            .add(
                Kind::RoundedRect,
                Geom::Rect(rect(10, 10, 100, 30)),
                Style {
                    width: 2,
                    corner_radius: 64,
                    ..Style::default()
                },
            )
            .id;
        assert_eq!(drawn_corner(64, &rect(10, 10, 100, 30)), 15);
        assert!(
            radius_grip_command(&doc, id, 64, 20, 0).is_none(),
            "一把只往外的拖不动画不出来的那一档，也就不该留下一条撤销"
        );
        let down = radius_grip_command(&doc, id, 64, -20, 0).unwrap();
        down.apply(&mut doc);
        assert_eq!(doc.get(id).unwrap().style.corner_radius, 44);
    }

    #[test]
    fn a_radius_drag_is_one_undoable_step_and_a_locked_object_refuses_it() {
        let mut doc = Document::new(200, 200);
        let id = doc
            .add(
                Kind::RoundedRect,
                Geom::Rect(rect(10, 10, 100, 60)),
                Style {
                    width: 2,
                    corner_radius: 12,
                    ..Style::default()
                },
            )
            .id;
        let before = doc.clone();
        let cmd = radius_grip_command(&doc, id, 12, 6, 0).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(id).unwrap().style.corner_radius, 18);
        // The geometry it did not touch stays untouched, which is the difference between
        // this handle and the corner it lives next to.
        assert_eq!(doc.get(id).unwrap().geom, Geom::Rect(rect(10, 10, 100, 60)));
        cmd.invert(&mut doc);
        assert_eq!(doc, before, "一次 Ctrl+Z 撤的是那一次拖，不是它的两半");
        // A press that goes nowhere files nothing, so it cannot become an undo step.
        assert!(radius_grip_command(&doc, id, 12, 0, 0).is_none());
        // Locked, unknown, or not the tool that has corners.
        doc.get_mut(id).unwrap().locked = true;
        assert!(radius_grip_command(&doc, id, 12, 6, 0).is_none());
        assert!(radius_grip_command(&doc, 999, 12, 6, 0).is_none());
        doc.get_mut(id).unwrap().locked = false;
        doc.get_mut(id).unwrap().kind = Kind::Rect;
        assert!(radius_grip_command(&doc, id, 12, 6, 0).is_none());
    }

    #[test]
    fn a_turned_box_moves_its_radius_the_way_the_pointer_moves_it() {
        // Rotated a quarter turn, the handle's own axis is the picture's vertical: a drag
        // down pulls the arc out and a drag right does nothing, which is what makes the
        // handle follow the pointer instead of following the box that was written down.
        let mut doc = Document::new(200, 200);
        let id = {
            let el = doc.add(
                Kind::RoundedRect,
                Geom::Rect(rect(60, 60, 100, 60)),
                Style {
                    width: 2,
                    corner_radius: 12,
                    ..Style::default()
                },
            );
            el.id
        };
        doc.get_mut(id).unwrap().transform.rotation = 90.0;
        let cmd = radius_grip_command(&doc, id, 12, 0, 8).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(id).unwrap().style.corner_radius, 20);
        assert!(radius_grip_command(&doc, id, 12, 8, 0).is_none());
    }
}
