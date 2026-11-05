//! Coordinate system and selection geometry.
//!
//! Everything persisted or exchanged across the FFI boundary is in **physical
//! pixels in virtual-desktop coordinates** — the same space `xcap` reports monitor
//! and window bounds in. Device pixel ratios only exist at the display edge.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysPoint {
    pub x: i32,
    pub y: i32,
}

impl PhysPoint {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
    pub fn translate(self, dx: i32, dy: i32) -> Self {
        Self::new(self.x.saturating_add(dx), self.y.saturating_add(dy))
    }
}

impl std::fmt::Display for PhysPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{}", self.x, self.y)
    }
}

/// A size with no position attached. Pins need it because their on-screen size
/// is derived (source crop × zoom), so the two axes travel together and never
/// as a rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysSize {
    pub w: u32,
    pub h: u32,
}

impl PhysSize {
    pub const fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }

    pub fn area(self) -> u64 {
        self.w as u64 * self.h as u64
    }

    pub fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// A quarter turn swaps the axes.
    pub fn quarter_turned(self) -> Self {
        Self {
            w: self.h,
            h: self.w,
        }
    }

    /// `at` as its top-left corner.
    pub fn at(self, p: PhysPoint) -> PhysRect {
        PhysRect::new(p.x, p.y, self.w, self.h)
    }
}

impl std::fmt::Display for PhysSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.w, self.h)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl std::fmt::Display for PhysRect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{} {}x{}", self.x, self.y, self.w, self.h)
    }
}

impl PhysRect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn from_points(a: PhysPoint, b: PhysPoint) -> Self {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Self {
            x,
            y,
            w: a.x.abs_diff(b.x),
            h: a.y.abs_diff(b.y),
        }
    }

    pub fn ltrb(&self) -> (i32, i32, i32, i32) {
        (self.x, self.y, self.right(), self.bottom())
    }

    /// The same rectangle without its position.
    pub const fn size(&self) -> PhysSize {
        PhysSize {
            w: self.w,
            h: self.h,
        }
    }

    pub fn top_left(&self) -> PhysPoint {
        PhysPoint::new(self.x, self.y)
    }

    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.w as i32)
    }

    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h as i32)
    }

    pub fn center(&self) -> PhysPoint {
        PhysPoint::new(self.x + (self.w as i32) / 2, self.y + (self.h as i32) / 2)
    }

    /// The exact centre, for a rotation pivot. [`PhysRect::center`] is a pixel;
    /// half a pixel of drift between the pivot an object is drawn about and the
    /// box its repaint was told to cover is stale pixels left behind.
    pub fn pivot(&self) -> (f64, f64) {
        (
            self.x as f64 + self.w as f64 / 2.0,
            self.y as f64 + self.h as f64 / 2.0,
        )
    }

    pub fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn contains(&self, p: PhysPoint) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Self {
        Self::new(
            self.x.saturating_add(dx),
            self.y.saturating_add(dy),
            self.w,
            self.h,
        )
    }

    pub fn intersection(&self, other: &PhysRect) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x || bottom <= y {
            return None;
        }
        Some(Self::new(x, y, (right - x) as u32, (bottom - y) as u32))
    }

    /// Smallest rect covering both. Two empty rects stay empty instead of
    /// growing a phantom 1px row.
    pub fn union(&self, other: &PhysRect) -> Self {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self::new(
            x,
            y,
            (self.right().max(other.right()) - x) as u32,
            (self.bottom().max(other.bottom()) - y) as u32,
        )
    }

    /// Grow on all sides, saturating at the edges of the coordinate space.
    pub fn inflate(&self, margin: u32) -> Self {
        let m = margin as i32;
        let x = self.x.saturating_sub(m);
        let y = self.y.saturating_sub(m);
        Self {
            x,
            y,
            w: self.w.saturating_add(margin * 2),
            h: self.h.saturating_add(margin * 2),
        }
    }

    /// Shrink on all sides towards the centre. A margin that swallows the rect
    /// leaves an empty rect at its middle rather than a negative size, which is
    /// what the selection band test in `annotation` relies on.
    pub fn deflate(&self, margin: u32) -> Self {
        let m = margin.min(self.w / 2).min(self.h / 2) as i32;
        Self {
            x: self.x + m,
            y: self.y + m,
            w: self.w.saturating_sub(m as u32 * 2),
            h: self.h.saturating_sub(m as u32 * 2),
        }
    }

    /// Move `self` by the least possible amount so it fits inside `bounds`.
    /// Oversized rects are pinned to the bounds' top-left and clipped.
    pub fn fitted_into(&self, bounds: &PhysRect) -> Self {
        let mut out = *self;
        if out.w > bounds.w || out.h > bounds.h {
            out.w = out.w.min(bounds.w);
            out.h = out.h.min(bounds.h);
            out.x = bounds.x;
            out.y = bounds.y;
            return out;
        }
        if out.x < bounds.x {
            out.x = bounds.x;
        }
        if out.right() > bounds.right() {
            out.x = bounds.right() - out.w as i32;
        }
        if out.y < bounds.y {
            out.y = bounds.y;
        }
        if out.bottom() > bounds.bottom() {
            out.y = bounds.bottom() - out.h as i32;
        }
        out
    }

    /// PRD §5.2.5 / §8.5: a remembered region that no longer fits is clipped to
    /// the usable area, and the caller needs to know that happened.
    pub fn clamp_to(&self, bounds: &PhysRect) -> Option<(PhysRect, bool)> {
        let clipped = self.intersection(bounds)?;
        Some((clipped, clipped != *self))
    }
}

/// Device pixel ratio of a single monitor. `Scale(0.0)` is never valid; `of`
/// clamps so a bogus value from a driver cannot divide by zero downstream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scale(pub f64);

impl Scale {
    pub const ONE: Scale = Scale(1.0);

    pub fn of(v: f64) -> Self {
        if v.is_finite() && v > 0.0 {
            Scale(v)
        } else {
            Scale(1.0)
        }
    }

    pub fn ratio(self) -> f64 {
        self.0
    }

    pub fn phys_to_dip_i(self, v: i32) -> i32 {
        (v as f64 / self.0).round() as i32
    }

    pub fn dip_to_phys_i(self, v: i32) -> i32 {
        (v as f64 * self.0).round() as i32
    }

    pub fn phys_to_dip_f(self, v: f64) -> f64 {
        v / self.0
    }

    pub fn dip_to_phys_f(self, v: f64) -> i32 {
        (v * self.0).round() as i32
    }

    pub fn rect_to_dip(self, r: &PhysRect) -> DipRect {
        DipRect {
            x: self.phys_to_dip_f(r.x as f64),
            y: self.phys_to_dip_f(r.y as f64),
            w: self.phys_to_dip_f(r.w as f64),
            h: self.phys_to_dip_f(r.h as f64),
        }
    }

    pub fn rect_to_phys(self, r: &DipRect) -> PhysRect {
        PhysRect::new(
            self.dip_to_phys_f(r.x),
            self.dip_to_phys_f(r.y),
            self.dip_to_phys_f(r.w).max(0) as u32,
            self.dip_to_phys_f(r.h).max(0) as u32,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DipRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// PRD §5.3.7 — the size readout can be shown in physical pixels or DIPs.
///
/// Persisted as `[capture] size_unit` since P164, and spelled the way [`Unit::label`] spells it:
/// `snake_case` puts `"px"` / `"dip"` in the file, which is what every other stored enum in the
/// config does (`CaptureMode`, `AlphaBg`, `Format`, `Collision`) and what a user reads on the chip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    Px,
    Dip,
}

impl Unit {
    /// The one order the product speaks: the size bar's chips, the settings gate's refusal, and the
    /// round trip all read this list rather than each keeping their own.
    pub const ALL: [Unit; 2] = [Unit::Px, Unit::Dip];

    pub fn label(self) -> &'static str {
        match self {
            Unit::Px => "px",
            Unit::Dip => "dip",
        }
    }

    /// [`Unit::label`]'s inverse, for the text typed into the settings row. Reading the same list
    /// `label` writes is what keeps a third spelling from appearing.
    pub fn from_label(text: &str) -> Option<Unit> {
        Self::ALL.iter().copied().find(|u| u.label() == text)
    }

    /// The accepted spellings in one line, for the message that refuses one.
    pub fn labels_line() -> String {
        Self::ALL
            .iter()
            .map(|u| u.label())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn show(self, phys: u32, scale: Scale) -> u32 {
        match self {
            Unit::Px => phys,
            Unit::Dip => scale.phys_to_dip_i(phys as i32).max(0) as u32,
        }
    }

    pub fn to_phys(self, shown: u32, scale: Scale) -> u32 {
        match self {
            Unit::Px => shown,
            Unit::Dip => scale.dip_to_phys_i(shown as i32).max(1) as u32,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
    Body,
}

impl Handle {
    pub fn all() -> [Handle; 8] {
        [
            Handle::Nw,
            Handle::N,
            Handle::Ne,
            Handle::E,
            Handle::Se,
            Handle::S,
            Handle::Sw,
            Handle::W,
        ]
    }
}

/// Which corner stays put when the user types an explicit width/height
/// (PRD §5.3.6 requires the anchor to be unambiguous).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

impl Anchor {
    /// The point this anchor names on `r` - the one corner that `set_size` is not
    /// allowed to move. It lives here rather than inside `set_size` because §5.3.8's
    /// whole promise is "the corner you did not drag stays put", and a promise you can
    /// only check by re-reading a constructor's arithmetic is a promise nothing tests.
    ///
    /// `right()` and `bottom()` are the exclusive edges, so a `TopRight` anchor sits at
    /// the coordinate the new rect's right edge lands on. That is the half-open
    /// convention `PhysRect` already keeps; the check is always equality of two points
    /// measured the same way.
    pub fn point(self, r: &PhysRect) -> PhysPoint {
        match self {
            Anchor::TopLeft => PhysPoint::new(r.x, r.y),
            Anchor::TopRight => PhysPoint::new(r.right(), r.y),
            Anchor::BottomLeft => PhysPoint::new(r.x, r.bottom()),
            Anchor::BottomRight => PhysPoint::new(r.right(), r.bottom()),
            Anchor::Center => r.center(),
        }
    }
}

pub const DEFAULT_HIT_SLOP: u32 = 5;

/// Hottest handle under `p`, or `Body` when inside. Checked corner-first so an
/// overlapping corner never resolves to an edge.
pub fn handle_at(r: &PhysRect, p: PhysPoint, slop: u32) -> Option<Handle> {
    let s = slop as i32;
    let in_x = |a: i32, b: i32| (a - b).abs() <= s;
    let near_left = in_x(p.x, r.x);
    let near_right = in_x(p.x, r.right());
    let near_top = in_x(p.y, r.y);
    let near_bottom = in_x(p.y, r.bottom());
    let inside = p.x > r.x + s && p.x < r.right() - s && p.y > r.y + s && p.y < r.bottom() - s;

    match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => Some(Handle::Nw),
        (_, true, true, _) => Some(Handle::Ne),
        (true, _, _, true) => Some(Handle::Sw),
        (_, true, _, true) => Some(Handle::Se),
        (_, _, true, _) => Some(Handle::N),
        (_, _, _, true) => Some(Handle::S),
        (true, _, _, _) => Some(Handle::W),
        (_, true, _, _) => Some(Handle::E),
        _ if inside || r.contains(p) => Some(Handle::Body),
        _ => None,
    }
}

/// Drag `handle` to `p`, keeping the rect inside `bounds`. Negative drags past
/// the opposite edge flip the rect rather than producing a negative size.
pub fn resize(r: &PhysRect, handle: Handle, p: PhysPoint, bounds: &PhysRect) -> PhysRect {
    if handle == Handle::Body {
        return move_by_origin(r, p, bounds);
    }
    let (l, t, rr, b) = r.ltrb();
    let (x0, y0, x1, y1) = match handle {
        Handle::Nw => (p.x, p.y, rr, b),
        Handle::N => (l, p.y, rr, b),
        Handle::Ne => (rr, p.y, p.x, b),
        Handle::E => (l, t, p.x, b),
        Handle::Se => (l, t, p.x, p.y),
        Handle::S => (l, t, rr, p.y),
        Handle::Sw => (p.x, t, rr, p.y),
        Handle::W => (p.x, t, rr, b),
        Handle::Body => unreachable!(),
    };
    let lo_x = x0.min(x1).max(bounds.x);
    let lo_y = y0.min(y1).max(bounds.y);
    let hi_x = x0.max(x1).min(bounds.right());
    let hi_y = y0.max(y1).min(bounds.bottom());
    PhysRect::new(
        lo_x,
        lo_y,
        (hi_x - lo_x).max(0) as u32,
        (hi_y - lo_y).max(0) as u32,
    )
}

/// PRD §5.3.9 — arrow keys move or resize by one pixel, with modifier steps.
pub fn nudge(r: &PhysRect, dx: i32, dy: i32, bounds: &PhysRect) -> PhysRect {
    let moved = r.offset(dx, dy);
    let fitted = moved.fitted_into(bounds);
    if fitted == *r {
        // Nothing moved: allow the size to shrink by a pixel at the offending
        // edge so a rect pinned to the bounds can still be nudged smaller.
        PhysRect::new(
            r.x.max(bounds.x),
            r.y.max(bounds.y),
            (r.right().min(bounds.right()) - r.x.max(bounds.x)) as u32,
            (r.bottom().min(bounds.bottom()) - r.y.max(bounds.y)) as u32,
        )
    } else {
        fitted
    }
}

pub fn nudge_edge(r: &PhysRect, handle: Handle, dx: i32, dy: i32, bounds: &PhysRect) -> PhysRect {
    let p = match handle {
        Handle::Nw => PhysPoint::new(r.x + dx, r.y + dy),
        Handle::N => PhysPoint::new(r.x, r.y + dy),
        Handle::Ne => PhysPoint::new(r.right() + dx, r.y + dy),
        Handle::E => PhysPoint::new(r.right() + dx, r.y),
        Handle::Se => PhysPoint::new(r.right() + dx, r.bottom() + dy),
        Handle::S => PhysPoint::new(r.x, r.bottom() + dy),
        Handle::Sw => PhysPoint::new(r.x + dx, r.bottom() + dy),
        Handle::W => PhysPoint::new(r.x + dx, r.y),
        Handle::Body => return nudge(r, dx, dy, bounds),
    };
    resize(r, handle, p, bounds)
}

/// PRD §5.3.9 第 4 步「放大镜同步显示边缘像素」里的那一颗**边缘点**。
///
/// 三个输入是同一件事的三侧：`after` 是**挪完之后**的选区，`handle` 是这一趟挪的是整块还是
/// 哪条边（与 [`nudge_edge`] 递进去的那一枚同一个数 —— 「Alt 挪哪条边」这件事因此仍然只有一
/// 个作者），`(dx, dy)` 是这一次按键**要求**的位移。
///
/// 报的是**选区自己**的那一颗像素：右/下两边取 `right() - 1` / `bottom() - 1`（`right()` 本身
/// 是第一颗**不属于**选区的像素，报它就答非所问），左/上取 `x` / `y`。另一根轴取这一条边的中点
/// （[`PhysRect::center`] 同一算式）而不是取端点，因为端点属于相邻的那两条边；一块放大镜罩在
/// 边上，边界两侧各看得见一半，这正是「正要对齐的那些边缘像素」。
///
/// `dx == dy == 0` 时把 `cursor` 原样交回：那不是一次按键，那是指针。§5.4.1「放大镜跟指针
/// 走」这件旧行为在这一臂里一字未改，鼠标那一路（`CaptureMask.qml` 的三处 hover/拖拽）至今
/// 走的是这一臂。
///
/// **读 `after`，不读 `(dx, dy)` 算出来的那一格**：[`nudge`] 与 [`nudge_edge`] 都会夹回桌面
/// （§5.3.9「不允许选区超出有效截图区域」），夹住的那一趟被挪掉的边就是夹住后的那条边；报
/// 「本来要到的那一格」就是报一颗屏幕上没有的像素 —— 而这一枚点的唯一用途就是去屏幕上取像素。
pub fn nudge_sample(
    after: &PhysRect,
    handle: Handle,
    dx: i32,
    dy: i32,
    cursor: PhysPoint,
) -> PhysPoint {
    if dx == 0 && dy == 0 {
        return cursor;
    }
    let (l, t, r, b) = after.ltrb();
    // 那两枚 `max` 兜的是空矩形：`w == 0` 时 `r - 1` 会跑到左边之左。产品路径到不了这一臂
    // （桥里 `MaskState::nudge_by` 头上有 `hole.is_empty()` 的早退），留着是为了这一手对**任何**
    // 一枚矩形都答得出一颗点，而不是答出一颗负方向的、让下面拿它去取像素。
    let (last_r, last_b) = ((r - 1).max(l), (b - 1).max(t));
    // 另一根轴上取的是这一条边的中点 —— 直接问 [`PhysRect::center`]，那一位作者，而不是在这里
    // 再写一遍 `(b - t) / 2`（那一写就成了两种"中点"，200% 屏上差的就是用户正在对的那一格）。
    let mid = after.center();
    let x = match handle {
        // 整块移动时两条竖边都动了，但「正在对齐的那一条」是前进方向的那一条 leading 边。
        Handle::Body if dx > 0 => last_r,
        Handle::Body if dx < 0 => l,
        Handle::E | Handle::Ne | Handle::Se if dx != 0 => last_r,
        Handle::W | Handle::Nw | Handle::Sw if dx != 0 => l,
        // 这一趟没挪竖边（N / S 那一档，或一次纯纵向的移动）⇒ 落在被挪的那条横边的中点上。
        _ => mid.x,
    };
    let y = match handle {
        Handle::Body if dy > 0 => last_b,
        Handle::Body if dy < 0 => t,
        Handle::N | Handle::Ne | Handle::Nw if dy != 0 => t,
        Handle::S | Handle::Se | Handle::Sw if dy != 0 => last_b,
        _ => mid.y,
    };
    PhysPoint::new(x, y)
}

/// Grow/shrink about a fixed anchor, used by the explicit size input and by
/// wheel-resize of a pin.
pub fn set_size(r: &PhysRect, w: u32, h: u32, anchor: Anchor) -> PhysRect {
    let w = w.max(1);
    let h = h.max(1);
    let p = anchor.point(r);
    // The origin is the anchor pulled back by the new size along whichever axes that
    // anchor is measured from. One match per axis rather than five pairs, so `TopRight`
    // and `BottomRight` cannot disagree about how far right the right edge is.
    let dx = match anchor {
        Anchor::TopLeft | Anchor::BottomLeft => 0,
        Anchor::TopRight | Anchor::BottomRight => -(w as i32),
        Anchor::Center => -(w as i32) / 2,
    };
    let dy = match anchor {
        Anchor::TopLeft | Anchor::TopRight => 0,
        Anchor::BottomLeft | Anchor::BottomRight => -(h as i32),
        Anchor::Center => -(h as i32) / 2,
    };
    PhysRect::new(p.x + dx, p.y + dy, w, h)
}

/// PRD §5.3.6 的第二条规则 —— "输入值超出屏幕范围时，应限制到有效区域" —— 落在尺寸那一趟上。
///
/// [`set_size`] 不管边界，因为叫它的人里有两类：§5.3.8 的比例锁只需要换尺寸，滑回桌面由它
/// 自己后面那一次 `fitted_into` 负责；钉图的滚轮缩放根本没有桌面可夹。这一枚是**打进来的那
/// 对数字**的路：用户写了 5000×4000，答案不该是"选区跑到屏幕外，下一笔拖拽再把它滑回来"。
///
/// 让路的是**对面那条边**，锚点这一侧的边一条也不动：结果取理想矩形与桌面的交集，于是
/// `TopRight` 锚住时缩短的是左边，`BottomLeft` 锚住时缩短的是上边。这不是 `fitted_into`
/// 的滑动 —— 滑动会把第一条规则钉死的那枚锚点搬走，而那正是"从哪一角扩展"这句话的反面。
pub fn set_size_within(
    r: &PhysRect,
    w: u32,
    h: u32,
    anchor: Anchor,
    bounds: &PhysRect,
) -> PhysRect {
    let q = set_size(r, w, h, anchor);
    let l = q.x.max(bounds.x);
    let t = q.y.max(bounds.y);
    let rr = q.right().min(bounds.right());
    let bb = q.bottom().min(bounds.bottom());
    // 最少留一像素，与 `set_size` 里那两枚 `.max(1)` 同一口径：夹完只剩 0 的话，"限制到有效
    // 区域"就把选区删了，而没人按过删除。
    //
    // 最后那一次 `fitted_into` 在正常路上取不到任何动作 —— 交集已经把 w/h 收到桌面以内、把
    // 左边与上边收到桌面起点，四条 if 全都落空。它管的只有那一枚 `.max(1)` 造出的窄片：锚点
    // 正好压在桌面的外沿（一张还没拖开的选区就是这种）时，"对面让路"让到对面追上了锚点，再
    // 保锚点就得把像素放到屏幕外。那一格上第二条规则赢，这一枚函数才有"结果必在桌面内"这条
    // 能被性质测试问出来的保证。
    PhysRect::new(l, t, (rr - l).max(1) as u32, (bb - t).max(1) as u32).fitted_into(bounds)
}

pub fn scale_about(r: &PhysRect, factor: f64, anchor_point: PhysPoint) -> PhysRect {
    let f = if factor.is_finite() {
        factor.clamp(0.01, 100.0)
    } else {
        1.0
    };
    let w = ((r.w as f64) * f).round().max(1.0) as u32;
    let h = ((r.h as f64) * f).round().max(1.0) as u32;
    let dx = (r.x - anchor_point.x) as f64 * f;
    let dy = (r.y - anchor_point.y) as f64 * f;
    PhysRect::new(
        anchor_point.x + dx.round() as i32,
        anchor_point.y + dy.round() as i32,
        w,
        h,
    )
}

/// PRD §5.3.8 — keep a width:height ratio while resizing. The `anchor` corner
/// stays put, which is what "adjust the size we already dragged" means.
pub fn constrain_aspect(r: &PhysRect, ratio: f64, anchor: Anchor) -> Option<PhysRect> {
    if !ratio.is_finite() || ratio <= 0.0 || r.is_empty() {
        return None;
    }
    let w = r.w.max(1);
    let h = r.h.max(1);
    let by_w = (w, ((w as f64 / ratio).round().max(1.0)) as u32);
    let by_h = (((h as f64 * ratio).round().max(1.0)) as u32, h);
    let area = r.area();
    let (nw, nh) = if (by_w.0 as u64 * by_w.1 as u64).abs_diff(area)
        <= (by_h.0 as u64 * by_h.1 as u64).abs_diff(area)
    {
        by_w
    } else {
        by_h
    };
    Some(set_size(r, nw, nh, anchor))
}

pub fn parse_ratio(spec: &str) -> Option<f64> {
    let (a, b) = spec.split_once(':')?;
    let w: f64 = a.trim().parse().ok()?;
    let h: f64 = b.trim().parse().ok()?;
    if w > 0.0 && h > 0.0 {
        Some(w / h)
    } else {
        None
    }
}

/// The corner that stays put when `handle` is dragged: the one diagonally opposite it.
///
/// Only the four corners answer. PRD §5.3.8 fixes the rule to 「调整角点时保持比例」, so an
/// edge handle says `None` and keeps moving a single dimension - which is what an edge
/// handle *is*. `Body` is not a corner either.
pub fn anchor_of_corner(handle: Handle) -> Option<Anchor> {
    Some(match handle {
        Handle::Nw => Anchor::BottomRight,
        Handle::Ne => Anchor::BottomLeft,
        Handle::Sw => Anchor::TopRight,
        Handle::Se => Anchor::TopLeft,
        _ => return None,
    })
}

/// The corner a *newly drawn* rectangle holds still: the one the pointer went down at,
/// which follows the quadrant the pointer dragged into. `from_points` normalises the two
/// points into a rect, so without this the anchor would have to be re-derived from the
/// rect's own corners - and a rect whose width is zero names two corners at once.
pub fn anchor_of_drag(from: PhysPoint, to: PhysPoint) -> Anchor {
    match (to.x >= from.x, to.y >= from.y) {
        (true, true) => Anchor::TopLeft,
        (true, false) => Anchor::BottomLeft,
        (false, true) => Anchor::TopRight,
        (false, false) => Anchor::BottomRight,
    }
}

fn move_by_origin(r: &PhysRect, p: PhysPoint, bounds: &PhysRect) -> PhysRect {
    let _ = bounds;
    PhysRect::new(p.x, p.y, r.w, r.h).fitted_into(bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn b() -> PhysRect {
        PhysRect::new(0, 0, 1000, 800)
    }

    #[test]
    fn from_points_normalizes_any_drag_direction() {
        assert_eq!(
            PhysRect::from_points(PhysPoint::new(30, 40), PhysPoint::new(10, 10)),
            PhysRect::new(10, 10, 20, 30)
        );
        assert_eq!(
            PhysRect::from_points(PhysPoint::new(10, 10), PhysPoint::new(30, 40)),
            PhysRect::new(10, 10, 20, 30)
        );
    }

    #[test]
    fn resize_keeps_the_opposite_edge() {
        let r = PhysRect::new(100, 100, 200, 100);
        assert_eq!(
            resize(&r, Handle::Se, PhysPoint::new(350, 250), &b()),
            PhysRect::new(100, 100, 250, 150)
        );
        assert_eq!(
            resize(&r, Handle::Nw, PhysPoint::new(50, 50), &b()),
            PhysRect::new(50, 50, 250, 150)
        );
    }

    #[test]
    fn resize_clamps_and_flips_past_the_opposite_edge() {
        let r = PhysRect::new(900, 700, 100, 100);
        // Clamped: the SE corner cannot leave the virtual desktop.
        let out = resize(&r, Handle::Se, PhysPoint::new(5000, 5000), &b());
        assert_eq!(out, PhysRect::new(900, 700, 100, 100));
        // Dragging NW inward shrinks, keeping the opposite (SE) edge.
        assert_eq!(
            resize(&r, Handle::Nw, PhysPoint::new(950, 760), &b()),
            PhysRect::new(950, 760, 50, 40)
        );
        // Dragging NW past the SE edge flips the rect instead of inverting it.
        let small = PhysRect::new(100, 100, 50, 50);
        assert_eq!(
            resize(&small, Handle::Nw, PhysPoint::new(200, 200), &b()),
            PhysRect::new(150, 150, 50, 50)
        );
        assert_eq!(
            resize(&small, Handle::E, PhysPoint::new(80, 120), &b()),
            PhysRect::new(80, 100, 20, 50)
        );
    }

    #[test]
    fn nudge_pins_at_the_edge() {
        let r = PhysRect::new(990, 790, 10, 10);
        assert_eq!(nudge(&r, 100, 100, &b()), r);
        assert_eq!(nudge(&r, -3, -4, &b()), PhysRect::new(987, 786, 10, 10));
    }

    // §5.3.9 第 4 步「放大镜同步显示边缘像素」—— [`nudge_sample`] 是那一颗点的唯一作者，
    // 而它是纯的：一颗点 + 一次按键的两个增量 + 一枚 handle，不碰快照、不碰 Qt、不碰锁，
    // 所以 §9.1 第十条要的「纯函数判决由 cargo test 作证」在这一格就成立。

    ///  regression 的对面：指针那一路。`dx == dy == 0` 不是按键，是 hover —— 放大镜跟着
    /// 指针走这件 §5.4.1 的旧行为必须原样交回指针那一颗，不然鼠标那条腿会在这一轮被改口。
    #[test]
    fn a_nudge_of_nothing_keeps_the_pointer_as_the_sample() {
        let r = PhysRect::new(100, 100, 200, 100);
        let cursor = PhysPoint::new(640, 480);
        for handle in [Handle::Body, Handle::Se, Handle::N] {
            assert_eq!(
                nudge_sample(&r, handle, 0, 0, cursor),
                cursor,
                "{handle:?} 那一档上，没挪动的这一趟不该改采样点"
            );
        }
    }

    /// 四枚方向键各回到**刚挪掉的那条边**上选区自己的那一颗像素。整块移动时四条边都动了，
    /// leading 的那一条才是用户正在对齐的那一条；另一根轴取这一条边的中点。
    #[test]
    fn each_arrow_re_anchors_the_magnifier_to_the_edge_that_moved() {
        let start = PhysRect::new(100, 100, 200, 100);
        let cursor = PhysPoint::new(3, 7);
        let cases = [
            // (dx, dy, 期望：刚挪掉的那条边上的那一颗)
            (1, 0, PhysPoint::new(300, 150)), // → 右边：101+200-1，不是不属于选区的 301
            (-1, 0, PhysPoint::new(99, 150)), // ← 左边
            (0, -1, PhysPoint::new(200, 99)), // ↑ 上边（x 取这一条边的中点）
            (0, 1, PhysPoint::new(200, 200)), // ↓ 下边：101+100-1
        ];
        for (dx, dy, want) in cases {
            let after = nudge(&start, dx, dy, &b());
            assert_ne!(after, start, "{dx},{dy} 这一趟根本没挪，判据就量的是空集");
            assert_eq!(
                nudge_sample(&after, Handle::Body, dx, dy, cursor),
                want,
                "{dx},{dy} 之后放大镜该看挪掉的那条边"
            );
            // 指针那一颗不许留在答案里 —— 那正是这一轮要修的缺陷。
            assert_ne!(
                nudge_sample(&after, Handle::Body, dx, dy, cursor),
                cursor,
                "{dx},{dy} 之后还报指针，就是「键盘挪了、放大镜没同步」"
            );
        }
        // 斜着挪（方向键从不发这一枚数，但这一手是全函数）：leading 的那一角。
        assert_eq!(
            nudge_sample(&nudge(&start, 1, 1, &b()), Handle::Body, 1, 1, cursor),
            PhysPoint::new(300, 200)
        );
    }

    /// 夹住的那一趟报**夹住后的**那条边，不报要求去的那一格：`(990,790,10,10)` 往右推 50，
    /// [`nudge`] 把它留在桌面里，放大镜因此该采桌面右沿上选区自己的最后一列（999），而不是
    /// 屏幕上根本不存在的那一列（1049）。
    #[test]
    fn a_nudge_clamped_at_the_desktop_edge_names_the_clamped_edge() {
        let r = PhysRect::new(990, 790, 10, 10);
        let after = nudge(&r, 50, 0, &b());
        assert_eq!(after.right(), 1000, "夹具没夹在桌面右沿上，这一格量的就不是夹住那一档");
        let p = nudge_sample(&after, Handle::Body, 50, 0, PhysPoint::new(0, 0));
        assert_eq!(p, PhysPoint::new(999, 795));
        assert_ne!(p.x, 1049, "报的是要求去的那一格，不是屏幕上真的那条边");
        // 往下推那一趟同理：夹在桌面下沿上，报的是真的最后一行（799），不是要求去的那一行（849）。
        let pushed_down = nudge(&r, 0, 50, &b());
        assert_eq!(pushed_down, r, "夹具没夹在桌面下沿上，这一格量的就不是夹住那一档");
        let q = nudge_sample(&pushed_down, Handle::Body, 0, 50, PhysPoint::new(0, 0));
        assert_eq!(q, PhysPoint::new(995, 799));
        assert_ne!(q.y, 849, "报的是要求去的那一格，不是屏幕上真的那条边");
    }

    /// Alt 那一档挪的是右下那两条边（桥里 `nudge_by` 递 [`Handle::Se`]），放大镜就只该跟着
    /// **被挪的那一条**走：只推 dx 时报右边中点、只推 dy 时报下边中点，一动未动的上边/左边
    /// 也不许冒充「刚挪过的那条边」。
    #[test]
    fn an_alt_resize_re_anchors_to_the_edge_that_grew_not_to_the_one_that_stayed() {
        let start = PhysRect::new(100, 100, 200, 100);
        let cursor = PhysPoint::new(3, 7);
        let wide = nudge_edge(&start, Handle::Se, 1, 0, &b());
        assert_eq!(
            nudge_sample(&wide, Handle::Se, 1, 0, cursor),
            PhysPoint::new(300, 150),
            "dx 那一趟该落在刚挪过的那条竖边的中点"
        );
        let tall = nudge_edge(&start, Handle::Se, 0, 1, &b());
        assert_eq!(
            nudge_sample(&tall, Handle::Se, 0, 1, cursor),
            PhysPoint::new(200, 200),
            "dy 那一趟该落在刚挪过的那条横边的中点"
        );
        assert_ne!(
            nudge_sample(&tall, Handle::Se, 0, 1, cursor).y,
            tall.y,
            "上边这一趟一动未动，放大镜不许报它"
        );
        // 只挪高度的那一趟里，左边也没动：x 取的是中点而不是那一条竖边。
        assert_eq!(nudge_sample(&tall, Handle::Se, 0, 1, cursor).x, tall.center().x);
    }

    /// 步长不改答案：Shift 那一档只是把同一条边推得远一点（§5.3.9「更细或更大步长」），
    /// 放大镜报的仍是那一条边 —— 这一格钉的是「handle 与边」这对关系，不是那枚 1/10 的数。
    #[test]
    fn a_modifier_step_names_the_same_edge_as_a_single_pixel() {
        let start = PhysRect::new(100, 100, 200, 100);
        let cursor = PhysPoint::new(3, 7);
        for (one, ten) in [(1, 10), (-1, -10)] {
            let a = nudge_sample(&nudge(&start, one, 0, &b()), Handle::Body, one, 0, cursor);
            let big = nudge_sample(&nudge(&start, ten, 0, &b()), Handle::Body, ten, 0, cursor);
            assert_eq!(
                big.x - a.x,
                ten - one,
                "十步之后那条边就该在十格之外，别的是谁都不是"
            );
            assert_eq!(a.y, big.y, "同一列的中点不该跟着步长漂");
        }
    }

    #[test]
    fn unit_round_trip_is_scale_aware() {
        let s = Scale(1.5);
        assert_eq!(Unit::Dip.show(s.dip_to_phys_i(150) as u32, s), 150);
        assert_eq!(Unit::Dip.to_phys(100, s), 150);
        assert_eq!(Unit::Px.to_phys(150, s), 150);
    }

    #[test]
    fn the_unit_labels_are_the_spellings_the_file_stores() {
        // §5.3.7 step 1: the settings row types a name and the gate reads it back. `label` writes
        // the spelling, `from_label` reads it, and `ALL` is the one order both the chips and the
        // refusal message use - so no third spelling can appear on that path.
        for unit in Unit::ALL {
            assert_eq!(Unit::from_label(unit.label()), Some(unit));
        }
        assert_eq!(Unit::labels_line(), "px, dip");
        assert_eq!(
            Unit::ALL[0],
            Unit::default(),
            "the shipped unit is the list's first"
        );
        for bad in ["PX", "Px", "pixels", "dip ", ""] {
            assert_eq!(Unit::from_label(bad), None, "{bad} is not a unit");
        }
    }

    #[test]
    fn clamp_reports_truncation() {
        let (kept, clipped) = PhysRect::new(950, 10, 100, 100).clamp_to(&b()).unwrap();
        assert!(clipped);
        assert_eq!(kept, PhysRect::new(950, 10, 50, 100));
        assert!(PhysRect::new(2000, 0, 10, 10).clamp_to(&b()).is_none());
    }

    #[test]
    fn aspect_constraint_picks_the_closer_candidate() {
        let r = PhysRect::new(0, 0, 101, 97);
        // 97x97 is 388 px off the dragged area, 101x101 is 404 off -> 97 wins.
        let c = constrain_aspect(&r, 1.0, Anchor::TopLeft).unwrap();
        assert_eq!(c, PhysRect::new(0, 0, 97, 97));
        // The anchored corner is the one that stays put (PRD §5.3.8).
        assert_eq!(
            constrain_aspect(&r, 1.0, Anchor::BottomRight).unwrap(),
            PhysRect::new(4, 0, 97, 97)
        );
        let wide = constrain_aspect(&r, 16.0 / 9.0, Anchor::TopLeft).unwrap();
        assert_eq!((wide.w, wide.h), (101, 57));
        assert_eq!(parse_ratio("16:9"), Some(16.0 / 9.0));
        assert_eq!(parse_ratio("0:9"), None);
        assert_eq!(parse_ratio("16"), None);
        assert!(constrain_aspect(&PhysRect::new(0, 0, 10, 0), 1.0, Anchor::TopLeft).is_none());
    }

    /// §5.3.8's other half: the lock is only worth anything if it holds the corner the
    /// user is *not* dragging. The pairing is asserted against `anchor_of_corner` itself,
    /// so a wrong table (`Nw` with `TopLeft`) cannot pass by keeping a corner that was
    /// never fixed.
    #[test]
    fn a_corner_handle_anchors_the_opposite_corner() {
        let r = PhysRect::new(100, 100, 40, 30);
        for (handle, a) in [
            (Handle::Nw, Anchor::BottomRight),
            (Handle::Ne, Anchor::BottomLeft),
            (Handle::Sw, Anchor::TopRight),
            (Handle::Se, Anchor::TopLeft),
        ] {
            assert_eq!(
                anchor_of_corner(handle),
                Some(a),
                "{handle:?} names the wrong corner"
            );
            let out = constrain_aspect(&r, 16.0 / 9.0, a).unwrap();
            assert_eq!(a.point(&out), a.point(&r), "{handle:?} let its anchor move");
        }
        for edge in [Handle::N, Handle::E, Handle::S, Handle::W, Handle::Body] {
            assert_eq!(anchor_of_corner(edge), None, "{edge:?} is not a corner");
        }
    }

    /// A drag that goes up-and-left from the press point keeps that press point as its
    /// corner - and the four quadrants are four different anchors, not one guess.
    #[test]
    fn a_new_drag_anchors_the_press_point() {
        let from = PhysPoint::new(100, 100);
        for to in [
            PhysPoint::new(140, 130),
            PhysPoint::new(140, 70),
            PhysPoint::new(60, 130),
            PhysPoint::new(60, 70),
        ] {
            let a = anchor_of_drag(from, to);
            assert_eq!(
                a.point(&PhysRect::from_points(from, to)),
                from,
                "{to:?} moved the press point"
            );
        }
        // A drag that has not moved yet (or only straight down) still names one corner,
        // and `constrain_aspect` is the one that declines: a zero-area rect has no ratio.
        assert_eq!(anchor_of_drag(from, from), Anchor::TopLeft);
        assert_eq!(
            anchor_of_drag(from, PhysPoint::new(100, 140)),
            Anchor::TopLeft
        );
    }

    proptest! {
        /// The claim the mask makes when it says "the corner keeps the ratio". It is
        /// stated as *which rect the lock may return* rather than as a percentage: the
        /// error of rounding one axis to integer pixels is `r²/2w` on the width-driven
        /// candidate, which is 7.6% at 41×2.5 - a tolerance written as "5%" would have
        /// been a number chosen to pass, and this round's ledger is about not doing that.
        /// The candidate disjunction says the real thing: one pixel of one axis, or the
        /// anchor did not move.
        #[test]
        fn locking_a_corner_returns_one_of_the_two_rounded_candidates(
            x in 0i32..400, y in 0i32..400, w in 1u32..400, h in 1u32..400,
            which in 0usize..4,
        ) {
            let handle = [Handle::Nw, Handle::Ne, Handle::Sw, Handle::Se][which];
            let anchor = anchor_of_corner(handle).unwrap();
            let r = PhysRect::new(x, y, w, h);
            for ratio in [1.0, 4.0 / 3.0, 16.0 / 9.0, 2.5] {
                let out = constrain_aspect(&r, ratio, anchor).unwrap();
                prop_assert_eq!(anchor.point(&out), anchor.point(&r));
                let by_w = (out.w, ((out.w as f64 / ratio).round().max(1.0)) as u32);
                let by_h = (((out.h as f64 * ratio).round().max(1.0)) as u32, out.h);
                prop_assert!(
                    (out.w, out.h) == by_w || (out.w, out.h) == by_h,
                    "{out:?} is neither candidate at {ratio:.3} ({by_w:?} / {by_h:?})"
                );
            }
        }
    }

    proptest! {
        #[test]
        fn drag_from_points_is_always_normalised(
            x0 in -4000i32..4000, y0 in -4000i32..4000,
            x1 in -4000i32..4000, y1 in -4000i32..4000,
        ) {
            let a = PhysPoint::new(x0, y0);
            let b = PhysPoint::new(x1, y1);
            let r = PhysRect::from_points(a, b);
            prop_assert_eq!(r.x, x0.min(x1));
            prop_assert_eq!(r.y, y0.min(y1));
            prop_assert_eq!(r.w, x0.abs_diff(x1));
            prop_assert_eq!(r.h, y0.abs_diff(y1));
            prop_assert_eq!(r, PhysRect::from_points(b, a));
            if !r.is_empty() {
                // Half-open: the top-left origin is inside, the far drag point
                // sits on the exclusive edge and is legitimately outside.
                prop_assert!(r.contains(PhysPoint::new(r.x, r.y)));
                prop_assert!(r.contains(PhysPoint::new(x0.max(x1) - 1, y0.max(y1) - 1)));
            }
        }

        #[test]
        fn dip_round_trip_stays_within_half_a_dip(
            v in i32::MIN / 4..i32::MAX / 4,
            pct in 50u32..400,
        ) {
            let s = Scale(pct as f64 / 100.0);
            let back = s.dip_to_phys_i(s.phys_to_dip_i(v));
            // Two roundings: one at dip resolution (0.5 dip = 0.5*scale px)
            // and one on the way back.
            let tolerance = (1.0 + s.ratio()) as i32;
            prop_assert!(
                (back - v).abs() <= tolerance,
                "{v} -> {back} at {} (tol {tolerance})",
                s.0
            );
        }

        #[test]
        fn resize_never_leaves_the_bounds(
            x in 0i32..500, y in 0i32..500, w in 1u32..400, h in 1u32..400,
            tx in -100i32..900, ty in -100i32..900,
        ) {
            let bounds = PhysRect::new(0, 0, 800, 600);
            let r = PhysRect::new(x, y, w, h).fitted_into(&bounds);
            for handle in Handle::all() {
                let out = resize(&r, handle, PhysPoint::new(tx, ty), &bounds);
                prop_assert!(bounds.contains(out.center()) || out.is_empty());
                prop_assert!(out.right() <= bounds.right() && out.bottom() <= bounds.bottom());
                prop_assert!(out.x >= bounds.x && out.y >= bounds.y);
                prop_assert!(out.w <= bounds.w && out.h <= bounds.h);
            }
        }

        #[test]
        fn anchor_set_size_preserves_the_anchor_corner(
            x in -500i32..500, y in -500i32..500, w in 1u32..300, h in 1u32..300,
            nw in 1u32..300, nh in 1u32..300,
        ) {
            let r = PhysRect::new(x, y, w, h);
            for (anchor, name, corner) in [
                (Anchor::TopLeft, "TopLeft", (r.x, r.y)),
                (Anchor::TopRight, "TopRight", (r.right(), r.y)),
                (Anchor::BottomLeft, "BottomLeft", (r.x, r.bottom())),
                (Anchor::BottomRight, "BottomRight", (r.right(), r.bottom())),
            ] {
                let s = set_size(&r, nw, nh, anchor);
                prop_assert_eq!(s.w, nw);
                prop_assert_eq!(s.h, nh);
                let got = match anchor {
                    Anchor::TopLeft => (s.x, s.y),
                    Anchor::TopRight => (s.right(), s.y),
                    Anchor::BottomLeft => (s.x, s.bottom()),
                    Anchor::BottomRight => (s.right(), s.bottom()),
                    Anchor::Center => (s.center().x, s.center().y),
                };
                prop_assert_eq!(got, corner, "{}", name);
            }
        }

        #[test]
        fn a_typed_size_within_the_screen_is_exactly_the_typed_size(
            x in 200i32..300, y in 200i32..300, w in 1u32..100, h in 1u32..100,
            nw in 1u32..100, nh in 1u32..100,
        ) {
            let desk = PhysRect::new(0, 0, 800, 600);
            let r = PhysRect::new(x, y, w, h);
            for (anchor, name) in [
                (Anchor::TopLeft, "TopLeft"),
                (Anchor::TopRight, "TopRight"),
                (Anchor::BottomLeft, "BottomLeft"),
                (Anchor::BottomRight, "BottomRight"),
                (Anchor::Center, "Center"),
            ] {
                let ideal = set_size(&r, nw, nh, anchor);
                let out = set_size_within(&r, nw, nh, anchor, &desk);
                // 理想矩形本来就在桌面里时，"限制到有效区域"一个字都不许改 —— 这一条就是
                // "用户打 37，屏幕上不许变成 38" 那句验收话的性质版本。
                let inside = ideal.x >= desk.x
                    && ideal.y >= desk.y
                    && ideal.right() <= desk.right()
                    && ideal.bottom() <= desk.bottom();
                prop_assert!(inside, "{:?} {} left the desk", anchor, name);
                prop_assert_eq!(out, ideal, "{} got moved while it fit", name);
            }
        }

        #[test]
        fn a_typed_size_never_leaves_the_screen(
            x in -200i32..1000, y in -200i32..800, w in 1u32..600, h in 1u32..600,
            nw in 1u32..2000, nh in 1u32..2000,
        ) {
            let desk = PhysRect::new(0, 0, 800, 600);
            let r = PhysRect::new(x, y, w, h);
            for (anchor, name) in [
                (Anchor::TopLeft, "TopLeft"),
                (Anchor::TopRight, "TopRight"),
                (Anchor::BottomLeft, "BottomLeft"),
                (Anchor::BottomRight, "BottomRight"),
                (Anchor::Center, "Center"),
            ] {
                let out = set_size_within(&r, nw, nh, anchor, &desk);
                prop_assert!(out.x >= desk.x && out.y >= desk.y, "{name} 跑出了桌面");
                prop_assert!(
                    out.right() <= desk.right() && out.bottom() <= desk.bottom(),
                    "{name} 跑出了桌面"
                );
                prop_assert!(out.w >= 1 && out.h >= 1, "{name} 夹没了选区");
            }
        }
    }

    #[test]
    fn a_typed_size_too_big_gives_way_at_the_far_edge_only() {
        // 一张 100×100 的选区贴在 800×600 桌面的右下角，打 1200×900：四个方向各让各的边，
        // 而锚住的那一角一步也不许走 —— §5.3.6 两条规则同时成立的那一格。
        let desk = PhysRect::new(0, 0, 800, 600);
        let sel = PhysRect::new(700, 500, 100, 100);
        assert_eq!(
            set_size_within(&sel, 1200, 900, Anchor::TopLeft, &desk),
            PhysRect::new(700, 500, 100, 100),
            "左上角锚住：让的是右与下"
        );
        assert_eq!(
            set_size_within(&sel, 1200, 900, Anchor::TopRight, &desk),
            PhysRect::new(0, 500, 800, 100),
            "右上角锚住：右边仍到 800，让的是左边"
        );
        assert_eq!(
            set_size_within(&sel, 1200, 900, Anchor::BottomLeft, &desk),
            PhysRect::new(700, 0, 100, 600),
            "左下角锚住：下边仍到 600，让的是上边"
        );
        assert_eq!(
            set_size_within(&sel, 1200, 900, Anchor::BottomRight, &desk),
            desk,
            "右下角本来就压在桌面上：整张桌面就是它能长到的地方"
        );
        // 打下来的数字本来就装得下时，一个像素都不许动 —— 上面那四臂说的是"让路"，这一臂
        // 说的是"没路可让时不让"。
        assert_eq!(
            set_size_within(&sel, 50, 40, Anchor::TopLeft, &desk),
            PhysRect::new(700, 500, 50, 40)
        );
        // 中心是唯一一枚两条边同时可让的锚点，而这张桌面关于它对称，所以让得对等。
        let full = PhysRect::new(0, 0, 800, 600);
        let c = set_size_within(&full, 1000, 200, Anchor::Center, &desk);
        assert_eq!(c, PhysRect::new(0, 200, 800, 200));
        assert_eq!(c.center(), full.center(), "中心锚住的换算把中心搬走了");
    }

    #[test]
    fn an_anchor_pinned_to_the_far_edge_keeps_one_pixel_on_screen() {
        // 还没拖开的选区是这种：零号尺寸的起点正压在桌面的外沿。往屏幕外打的尺寸夹到底，
        // 剩下的一像素留在**桌面里**，而不是挂在屏幕外——第二条规则在这一格赢过第一条。
        let desk = PhysRect::new(0, 0, 800, 600);
        let at_edge = PhysRect::new(800, 600, 0, 0);
        let out = set_size_within(&at_edge, 100, 100, Anchor::TopLeft, &desk);
        assert_eq!(out, PhysRect::new(799, 599, 1, 1));
        assert!(desk.contains(out.center()), "夹完的那一像素得在屏幕上");
    }
}
