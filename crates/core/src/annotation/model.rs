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

    /// Which objects have a string a user can be asked to retype (§5.7.16 step 3's
    /// 文字内容). 编号 carries a `text` too, but the reader that paints it prefers
    /// [`Element::number`] and only falls back to the string when there is no number,
    /// so retyping one edits a line of pixels nobody can see.
    pub fn takes_text(self) -> bool {
        self == Kind::Text
    }

    /// The tool that removes pixels instead of adding them.
    pub fn is_eraser(self) -> bool {
        self == Kind::Eraser
    }

    /// The two tools whose whole job is to alter the picture underneath them
    /// (§5.7.9 马赛克, §5.7.10 模糊). They are the only readers of
    /// [`Style::intensity`], so the toolbar needs this to know whether to show a
    /// strength knob at all.
    pub fn has_effect(self) -> bool {
        matches!(self, Kind::Mosaic | Kind::Blur)
    }

    /// The number each effect tool opens with.
    ///
    /// 马赛克 and 模糊 do not want the same value: 8 is a legible block size and a
    /// nearly invisible blur, 12 is the reverse, and the shipped config already
    /// defaults 模糊 to 12 (§5.7.1's per-tool memory means remembering them apart is
    /// the requirement, not a convenience).
    pub fn default_intensity(self) -> Option<u32> {
        match self {
            Kind::Mosaic => Some(MOSAIC_BLOCK),
            Kind::Blur => Some(BLUR_RADIUS),
            _ => None,
        }
    }

    /// What [`Style::effect_for`] would build for this kind's default number.
    pub fn default_effect(self) -> Option<Effect> {
        let v = self.default_intensity()?;
        Some(match self {
            Kind::Blur => Effect::Blur { radius: v },
            _ => Effect::Mosaic { block: v },
        })
    }

    /// The two tools whose body is laid-out letters, and therefore the only two a 描边
    /// can ring (§5.7.11). `raster` draws 编号 through 文本's path, so a ring asked of one
    /// is asked of both — and §5.7.1's per-tool memory needs this to know which rows own
    /// an outline at all, the same service [`Kind::has_effect`] gives the strength knob.
    pub fn has_outline(self) -> bool {
        matches!(self, Kind::Text | Kind::Number)
    }

    /// The things a tool is allowed to insist on when it is picked and the user has
    /// never taught it (§5.7.8's half-transparent marker, §5.7.9/§5.7.10's own effect
    /// strength).
    ///
    /// Deliberately narrow: it changes the two settings the PRD states a value for and
    /// leaves colour, width and font alone, because those are the pen the user is
    /// already holding and dropping them on a tool switch would lose their work.
    pub fn apply_defaults(self, style: &mut Style) {
        if self == Kind::Marker {
            // §5.7.8 verbatim: 默认使用半透明颜色. Set, not `min` with the current
            // alpha: a highlighter that keeps an opaque pen leaves a wall of colour
            // over the text it was meant to let through.
            style.color[3] = MARKER_ALPHA;
        }
        if let Some(effect) = self.default_effect() {
            style.effect = effect;
        }
    }

    /// Kinds whose body is a filled area, so clicking anywhere inside them
    /// selects. A plain rectangle has to be filled (§5.7.2 step 4) to count.
    pub fn interior_is_target(self) -> bool {
        matches!(
            self,
            Kind::Mosaic | Kind::Blur | Kind::Zoom | Kind::Text | Kind::Number
        )
    }

    /// The id this kind is stored under. §5.7.1's per-tool style memory has to key a
    /// config table by tool, and the tool set only exists here - so this is the one
    /// place that decides the spelling, rather than a second list somewhere that can
    /// drift out of step with the enum.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Rect => "rect",
            Kind::RoundedRect => "rounded_rect",
            Kind::Ellipse => "ellipse",
            Kind::Line => "line",
            Kind::Polyline => "polyline",
            Kind::Arrow => "arrow",
            Kind::DoubleArrow => "double_arrow",
            Kind::Pencil => "pencil",
            Kind::Marker => "marker",
            Kind::Mosaic => "mosaic",
            Kind::Blur => "blur",
            Kind::Text => "text",
            Kind::Number => "number",
            Kind::Zoom => "zoom",
            Kind::Eraser => "eraser",
        }
    }

    /// [`Kind::key`] backwards. Unknown spellings are `None`, not a default: a config
    /// file naming a tool that does not exist has to be reported, not drawn as
    /// something else.
    pub fn from_key(key: &str) -> Option<Kind> {
        KINDS.iter().copied().find(|k| k.key() == key)
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

/// §5.7.11 step 4's 字号 range, in device pixels: below the floor the letters are
/// antialiasing mush rather than text, above the ceiling one line is taller than the
/// hole it was placed in.
///
/// These live here rather than in the layer because **two** callers have to agree on
/// them: the toolbar's knob clamps what a click can set, and
/// [`crate::config::Annotation::font_size`]'s validator clamps what a hand-written
/// file can say. Before they were shared those two disagreed - the file allowed 6 and
/// 200, the pen allowed 10 and 96 - so a config that said `font_size = 6` reached a
/// pen of 10, a toolbar with no lit cell and a file that no longer described the tool
/// in hand. The numbers stay one pair because the reason a size is unreadable does not
/// change depending on who typed it.
pub const FONT_MIN: u32 = 10;
pub const FONT_MAX: u32 = 96;

/// §5.7.20 的线宽两端，也是 §5.7.11 的 描边宽度两端。
///
/// 一圈边就是一根线，所以它是同一把尺 —— 给描边另配一套上下限，就是屏幕上两枚都写着"粗细"
/// 却各自数数的格子。与 [`FONT_MIN`] 同一分工：面板的格子、档里的校验、把手的拖动都读这一对，
/// 而不是各写一份。
pub const WIDTH_MIN: u32 = 1;
pub const WIDTH_MAX: u32 = 64;

/// §5.7.3 那把圆角阶梯的两端。
///
/// 它们此前住在工具栏那一层（因为只有那一处在夹），而 step 4「通过控制点调整……圆角」那一枚
/// 把手是 core 里的算术，于是现在是**两双手**要同意同一个数：拖到头的把手若亮着一个阶梯不许
/// 的半径，下一次按"更大"就会往回跳。地板 0 是一枚直角矩形，不是第二种"没有圆角"。
pub const CORNER_MIN: u32 = 0;
pub const CORNER_MAX: u32 = 64;

/// 一只框**画得出来**的那一枚圆角有多大：笔上的那个数，被短边的一半拦下。
///
/// 半径过半之后同一条边上的两段弧没有地方落脚，`raster` 一直是这么画的；把手那一族读的必须
/// 是同一个数，否则拖到尽头的把手会亮着一个屏幕上画不出来的半径。一句式子写在一处，是因为它
/// 此前在光栅里抄过两遍，而第三遍抄进把手就是把"圆角到底多大"交给了三个主人（#281）。
pub fn drawn_corner(radius: u32, r: &PhysRect) -> u32 {
    radius.min(r.w / 2).min(r.h / 2)
}

/// §5.7.11 的 描边出厂那一档有多粗 —— 屏幕上那圈边一直这么宽，此后它是 [`Style`] 的一格、
/// 用户能改，而**没改过的那一档**仍是这个数。
pub const OUTLINE_WIDTH_PX: u32 = 2;

/// 描边**没有配色**时那一圈边是什么颜色：笔不在的那一头亮度。
///
/// 用笔自己的颜色画一圈边，等于给已经是那个颜色的字再描一遍那个颜色 —— 那是一圈边唯一不能做
/// 的事。权重是 Rec. 601 的，因为笔可以是色带上六枚芯片的任何一枚、也可以是 §5.7.22 滚轮够到
/// 的任何一枚，所以判据问的是**笔的亮度**而不是某个有名字的颜色：一支像 `0x1c1c1e` 那样的深色
/// 笔仍然拿到白圈，这才是把阈值写在笔上而不是写在颜色名上的理由。
///
/// 它收颜色而不读笔，因为「这一枚对象的边该是什么颜色」问的是**每一枚自己**的笔色。
pub fn outline_of(color: [u8; 4]) -> [u8; 4] {
    let [r, g, b, _] = color;
    let light = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    if light > 127 {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    }
}

/// §5.7.8: 马克笔「默认使用半透明颜色」. Half of 255, which is what the PRD's word
/// 半透明 means and the number the re-edit panel will show back for a marker that was
/// never touched.
pub const MARKER_ALPHA: u8 = 128;

/// §5.7.2 step 2 / §5.7.21 step 3's 透明度 floor, as a percent of full opacity.
///
/// Zero is not "fully transparent ink", it is no ink: an element that covers the
/// picture, takes the place in the layer list, holds a selection handle and paints
/// nothing is a defect the user cannot see coming. Ten percent is the point where a
/// highlight is still a highlight.
pub const ALPHA_MIN_PCT: u32 = 10;

/// §5.7.21 step 3's wheel step. The ladder is a handful of detents rather than 256
/// values because a wheel that moves one byte per notch is a wheel that cannot be
/// aimed at "a bit see-through".
pub const ALPHA_STEP_PCT: u32 = 10;

/// §5.7.2 step 4's 填充 and §5.7.11's 背景: how opaque a plate is, as a percent of the
/// pen's own alpha. Thirty is the number the layer has always used (`3/10`); it is here
/// because a fill has to be *rebuilt* when the alpha wheel moves the pen under it, and a
/// fraction copied into two files is two answers to what a filled box at 40% looks like.
pub const FILL_ALPHA_PCT: u32 = 30;

/// The rungs of the strength ladder §5.7.9's 像素化块大小 and §5.7.10's 模糊强度 share.
///
/// The ceiling is the one [`crate::config::Annotation::blur_radius`]'s validator
/// already allows a hand-written file to say (1..=96); a knob that could be dragged
/// past what the config would accept would reset itself on the next start.
pub const EFFECT_MIN: u32 = 1;
pub const EFFECT_MAX: u32 = 96;

/// 马赛克's opening block size — the same 8 [`Style::default`] carries.
pub const MOSAIC_BLOCK: u32 = 8;

/// 模糊's opening radius — the same 12 the shipped `[annotation] blur_radius` says.
pub const BLUR_RADIUS: u32 = 12;

/// A transparency percent as it reaches a [`Style::color`]. Clamped, so a wheel that
/// spun past the end and a hand-written config land on the same legal byte.
///
/// Rounded rather than truncated, so the ladder's 50% is the 128 [`MARKER_ALPHA`] is,
/// and a readout can be moved off and back onto the same rung without the ink changing.
pub fn alpha_from_pct(pct: u32) -> u8 {
    let pct = pct.clamp(ALPHA_MIN_PCT, 100);
    ((pct * 255 + 50) / 100) as u8
}

/// [`alpha_from_pct`] back out again, for the readout next to the wheel.
///
/// Round-tripped through the same clamp, so the number on screen is the number that
/// will be stored if the user moves the wheel one notch from here.
pub fn pct_from_alpha(alpha: u8) -> u32 {
    (alpha as u32 * 100 + 50) / 255
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
    /// §5.7.3 step 2「配置圆角半径」的那个数，step 4「通过控制点调整……圆角」拖动的也是它。
    /// 能有多大由 [`drawn_corner`] 与 [`CORNER_MAX`] 各拦一半 —— 阶梯与把手同一把尺。
    pub corner_radius: u32,
    /// §5.7.11 step 4: 字体、字号.
    pub font_family: String,
    pub font_size: u32,
    pub align: Align,
    /// §5.7.11 rule: 无背景或填充色.
    pub text_bg: Option<[u8; 4]>,
    /// §5.7.11 rule: 文本描边支持颜色和宽度设置 —— 这一格是那句「支持设置」里的**开关**。
    /// 关着不留一圈边，而下面那两格在它关着的时候原样留着：把描边关掉再打开不是重设一次。
    pub text_outline: bool,
    /// 那一圈边的颜色（§5.7.11 那句规则的「颜色」一半）。`None` 说的是"没人配过"，画出来
    /// 的那一圈由 [`outline_of`] 替笔色让位；配过之后这一格就是用户说的那一枚，不再问笔。
    pub outline_color: Option<[u8; 4]>,
    /// 那一圈边有多宽（§5.7.11 那句规则的「宽度」一半），走笔那把 1..=[`WIDTH_MAX`] 的尺。
    pub outline_width: u32,
    /// §5.7.9 像素化块大小 / §5.7.10 模糊强度 — one element is either, never both.
    pub effect: Effect,
    pub brush: Brush,
    pub arrow_head: ArrowHead,
    /// §5.7.13 step 5: 放大倍数 as a percent, 100 = no change.
    pub zoom_percent: u32,
    /// §5.7.13 step 5: 边框. The PRD names the setting and fixes no shape for it, so
    /// this is the one thing about the frame a user can actually be asked to decide:
    /// whether it is there. Its width stays one pixel of the picture and its colour
    /// stays the pen's, because a second number the PRD does not ask for is a second
    /// number nothing can check it against.
    pub zoom_border: bool,
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
            text_outline: false,
            outline_color: None,
            outline_width: OUTLINE_WIDTH_PX,
            effect: Effect::Mosaic { block: 8 },
            brush: Brush::default(),
            arrow_head: ArrowHead::default(),
            zoom_percent: 200,
            zoom_border: true,
            connection_line: true,
            erase_to_transparent: false,
        }
    }
}

impl Style {
    /// How far this style reaches outside the geometry, for a kind that drags a
    /// brush. Box tools only pay for the pen.
    ///
    /// 椭圆那一支（[`Shape::Ellipse`]）在这里按**长轴**说话：长轴垂直于走笔方向，而笔划往哪
    /// 走是几何的事、不是这一格的，所以四个方向一起放大成长轴那一枚数。少放的那一侧会把带子
    /// 裁掉 —— 这一格是那一枚数唯一的读者，`raster` 算覆盖用的仍是 `size` 那一枚。
    pub fn reach_for(&self, kind: Kind) -> u32 {
        let pen = self.width.div_ceil(2).max(1);
        if kind.is_stroke() {
            let half = self.brush.size.div_ceil(2);
            let across = if self.brush.shape == Shape::Ellipse {
                (half as f64 * crate::imageops::ELLIPSE_RATIO).ceil() as u32
            } else {
                half
            };
            pen.max(across).saturating_add(self.brush.feather / 2)
        } else if matches!(kind, Kind::Arrow | Kind::DoubleArrow) {
            pen.max(head_reach(self.width))
        } else {
            pen
        }
    }

    /// The one number §5.7.9's 像素化块大小 and §5.7.10's 模糊强度 are stored in: the
    /// slot [`Style::effect`] carries, whichever shape it is wearing.
    pub fn intensity(&self) -> u32 {
        match self.effect {
            Effect::Mosaic { block } => block,
            Effect::Blur { radius } => radius,
        }
    }

    /// Write [`Style::intensity`] **as the given kind's effect**.
    ///
    /// The kind is an argument rather than read off the style, because setting 模糊 to
    /// 20 while the style still carries a mosaic is exactly the mistake §5.7.10 says
    /// must not happen, and the toolbar only shows this knob for the two effect tools.
    /// A kind with no effect has nothing to write, so this leaves it alone.
    pub fn set_intensity(&mut self, kind: Kind, v: u32) {
        let v = v.clamp(EFFECT_MIN, EFFECT_MAX);
        if let Some(effect) = match kind {
            Kind::Blur => Some(Effect::Blur { radius: v }),
            Kind::Mosaic => Some(Effect::Mosaic { block: v }),
            _ => None,
        } {
            self.effect = effect;
        }
    }

    /// The effect to paint this element with, decided by the element's **kind**.
    ///
    /// Before this existed the rasteriser matched on [`Style::effect`] alone, so an
    /// element drawn with 模糊 while its style happened to hold a mosaic painted a
    /// mosaic — the style could not tell a blur from a mosaic because that is the
    /// kind's job. The style supplies the number, the kind supplies the shape, and a
    /// mismatch between the two stops being possible.
    pub fn effect_for(&self, kind: Kind) -> Effect {
        let v = self.intensity().clamp(EFFECT_MIN, EFFECT_MAX);
        match kind {
            Kind::Blur => Effect::Blur { radius: v },
            Kind::Mosaic => Effect::Mosaic { block: v },
            _ => self.effect,
        }
    }

    /// The translucent plate a box's 填充 (§5.7.2 step 4) and a text's 背景 (§5.7.11)
    /// are painted with: the pen's own colour at [`FILL_ALPHA_PCT`] of its alpha, so a
    /// filled box and a plate both stay see-through *whatever* transparency the pen was
    /// set to. One expression rather than a fraction copied into the layer, because the
    /// alpha wheel moves the pen under a fill that is already switched on and the plate
    /// has to come with it.
    pub fn translucent(color: [u8; 4]) -> [u8; 4] {
        let mut c = color;
        c[3] = (c[3] as u32 * FILL_ALPHA_PCT / 100) as u8;
        c
    }

    /// §5.7.16's 透明度 for a style that already exists: the pen's alpha from
    /// [`alpha_from_pct`]'s ladder, and a fill that is switched on rebuilt so it keeps
    /// being the same plate over the new pen. The hue, width, font and effect of the
    /// style are not touched — re-editing one channel must not reset the others.
    pub fn set_alpha_pct(&mut self, pct: u32) {
        self.color[3] = alpha_from_pct(pct);
        if self.fill.is_some() {
            self.fill = Some(Self::translucent(self.color));
        }
    }

    /// Set the 字号 inside §5.7.11 step 4's range, the same floor and ceiling the
    /// config's validator uses ([`FONT_MIN`], [`FONT_MAX`]) so a wheel and a hand-written
    /// file cannot land on two different numbers.
    pub fn set_font_size(&mut self, size: u32) {
        self.font_size = size.clamp(FONT_MIN, FONT_MAX);
    }

    /// Set the 圆角半径 inside §5.7.3's ladder ([`CORNER_MIN`]..=[`CORNER_MAX`]).
    ///
    /// One method for both hands that ask: the toolbar's cell and the control point of
    /// §5.7.3 step 4. The box's own half-side is [`drawn_corner`]'s business, and the drag
    /// stops there before it ever reaches here — a handle that could be pulled to a radius
    /// the ladder would refuse is a readout that jumps back on the next click of 更大.
    pub fn set_corner_radius(&mut self, radius: u32) {
        self.corner_radius = radius.clamp(CORNER_MIN, CORNER_MAX);
    }

    /// §5.7.11's 描边宽度, on the pen's own ladder ([`WIDTH_MIN`]..=[`WIDTH_MAX`]): a ring
    /// is a line, so 粗细 means one number whether it is the box's edge or the letters' ring.
    pub fn set_outline_width(&mut self, width: u32) {
        self.outline_width = width.clamp(WIDTH_MIN, WIDTH_MAX);
    }

    /// §5.7.11's 描边颜色 as three hue bytes, with the ring's own alpha left where it was —
    /// the same split [`crate::annotation::colour_command`] makes for the pen, because the
    /// chip that carries a hue carries no alpha.
    pub fn set_outline_colour(&mut self, rgb: [u8; 3]) {
        let alpha = self.outline_for().0[3];
        self.outline_color = Some([rgb[0], rgb[1], rgb[2], alpha]);
    }

    /// The ring to draw with, for an element whose [`Style::text_outline`] is switched on:
    /// the colour the user set or [`outline_of`]'s when nobody has, and a width of at least
    /// one pixel — a zero-pixel ring is a lit switch that paints nothing.
    ///
    /// Resolved here rather than at each call site, because the renderer and the toolbar's
    /// readout have to answer "what colour is that ring" with the same number: while the
    /// switch was the only control, the colour existed only in the layer that drew it, and
    /// a panel that showed it would have been showing a second derivation.
    pub fn outline_for(&self) -> ([u8; 4], u32) {
        (
            self.outline_color.unwrap_or_else(|| outline_of(self.color)),
            self.outline_width.max(1),
        )
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
    ///
    /// 落回的位置按 id 找，不是接在末尾：这张表按构造是 id 升序（`add` 与 `clone_elements`
    /// 发的都是当时最大的号，`Crop` 是保序过滤），末尾那一格只有在**撤销一次删除**时才会说错话
    /// —— 删掉中间那一枚再 undo，接在末尾就把 id 序打乱了，而 `remove_command` 头顶那句
    /// 「undo 把东西交回它被记下的那个样子」认的正是原位。渲染不读这个位次（[`Document::paint_order`]
    /// 自己按 (z, id) 排），所以这一格只关乎那张表本身的读法。
    pub(crate) fn insert(&mut self, e: Element) {
        self.next_id = self.next_id.max(e.id + 1);
        if let Some(n) = e.number {
            self.next_counter = self.next_counter.max(n + 1);
        }
        let at = self.elements.partition_point(|x| x.id < e.id);
        self.elements.insert(at, e);
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

    /// 屏幕上第一枚**还戴着的**号：id 最小的那一枚编号自己戴的那个值。取 id 最小而不是号
    /// 最小，是因为 [`Self::renumber`] 按 id 序铺号 —— 「起点」要说的是它铺开的第一格落在
    /// 谁身上，两者不是同一问。场上没有编号 ⇒ `None`。
    pub fn first_number(&self) -> Option<u32> {
        self.elements
            .iter()
            .filter(|e| e.kind == Kind::Number)
            .min_by_key(|e| e.id)
            .and_then(|e| e.number)
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

    /// §5.7.18 step 3: the copies a clone would be, with fresh ids, shifted, above the
    /// originals. A cloned number keeps its value — renumbering is a separate choice.
    ///
    /// These are *produced*, not inserted: [`super::command::Command::Add`] is what puts
    /// elements in the document, and a builder that also wrote them would hand the same
    /// copy over twice — once here and once when the caller applied the command it made.
    /// The ids are drawn from the counter as if they were already handed out, so a list of
    /// them is self-numbered; [`Document::insert`] then raises the counter to match.
    pub fn clone_elements(&self, ids: &[u64], dx: i32, dy: i32) -> Vec<Element> {
        let mut copies: Vec<Element> = Vec::new();
        for id in ids {
            let Some(mut c) = self.get(*id).cloned() else {
                continue;
            };
            let from = c.geom.clone();
            let n = copies.len() as u64;
            c.id = self.next_id + n;
            c.z = self.next_z() + n as i32;
            c.geom = from.translate(dx, dy);
            copies.push(c);
        }
        copies
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
    fn the_first_number_on_screen_is_the_lowest_id_s_value() {
        // 「起点」那一问的证词：id 最小的那一枚戴 9、下一枚戴 3，屏幕上第一枚是 9 不是 3。
        // 取号最小会把 `renumber` 铺开的方向（id 序）与它落下的第一格混成两把尺。
        let mut doc = Document::new(100, 100);
        for x in [0, 30] {
            doc.add(Kind::Number, r(x, 10, 4, 4), Style::default());
        }
        doc.get_mut(1).unwrap().number = Some(9);
        doc.get_mut(2).unwrap().number = Some(3);
        assert_eq!(doc.first_number(), Some(9));
        doc.take(1);
        doc.take(2);
        assert_eq!(doc.first_number(), None, "场上没有编号");
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
        assert_eq!(copies.iter().map(|e| e.id).collect::<Vec<_>>(), vec![2]);
        let copy = copies[0].clone();
        assert_eq!(copy.geom, r(40, 10, 20, 20));
        assert!(copy.z > doc.get(id).unwrap().z);
        // Producing the copies is not filing them: writing them to the document is
        // `Command::Add`'s step, and a builder that wrote them too would hand the caller
        // the same copy twice - once here and once when it applied what this returned.
        assert_eq!(doc.len(), 1);
        doc.insert(copy);
        assert_eq!(doc.len(), 2);
        assert_eq!(doc.get(2).unwrap().geom, r(40, 10, 20, 20));
        assert_eq!(
            doc.add(Kind::Rect, r(0, 0, 4, 4), Style::default()).id,
            3,
            "insert raises the id counter to whatever the copies were called"
        );
        let n = doc.add(Kind::Number, r(20, 20, 4, 4), Style::default()).id;
        let c = doc.clone_elements(&[n], 1, 0)[0].clone();
        assert_eq!(c.number, doc.get(n).unwrap().number);
        assert_ne!(c.id, n);
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
    fn every_tool_has_one_spelling_that_survives_a_round_trip() {
        // `[tool_style]` keys config by this string, so a duplicate would silently
        // give two tools the same remembered colour.
        let mut seen = std::collections::HashSet::new();
        for kind in KINDS {
            assert!(seen.insert(kind.key()), "{} shares a key", kind.key());
            assert_eq!(Kind::from_key(kind.key()), Some(*kind));
        }
        assert_eq!(seen.len(), KINDS.len(), "KINDS itself repeats");
        assert_eq!(Kind::from_key("Rect"), None, "spellings are exact");
        assert_eq!(Kind::from_key(""), None);
        assert_eq!(Kind::from_key("no_such_tool"), None);
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

    #[test]
    fn a_fresh_marker_is_half_transparent_and_a_fresh_rect_is_not() {
        let mut style = Style::default();
        Kind::Marker.apply_defaults(&mut style);
        assert_eq!(style.color[3], MARKER_ALPHA, "§5.7.8 默认使用半透明颜色");
        assert_eq!(
            style.color[0],
            Style::default().color[0],
            "the highlighter keeps the pen's colour, only its alpha changes"
        );

        let mut style = Style::default();
        Kind::Rect.apply_defaults(&mut style);
        assert_eq!(style.color[3], 255, "a rectangle is not a highlighter");
        // Everything apply_defaults does not have a PRD word for is left alone, because
        // the pen in hand is the user's, not the tool's.
        assert_eq!(style.width, Style::default().width);
        assert_eq!(style.font_size, Style::default().font_size);
    }

    #[test]
    fn the_two_effect_tools_open_with_their_own_numbers() {
        let mut mosaic = Style::default();
        Kind::Mosaic.apply_defaults(&mut mosaic);
        assert_eq!(
            mosaic.effect,
            Effect::Mosaic {
                block: MOSAIC_BLOCK
            }
        );
        let mut blur = Style::default();
        Kind::Blur.apply_defaults(&mut blur);
        assert_eq!(
            blur.effect,
            Effect::Blur {
                radius: BLUR_RADIUS
            }
        );
        // §5.7.1's per-tool memory is only real if the two shapes stay apart: one
        // number slot shared between a block size and a radius is how 模糊 came to draw
        // a mosaic.
        assert_ne!(mosaic.effect, blur.effect);
        assert!(Kind::Mosaic.has_effect());
        assert!(Kind::Blur.has_effect());
        assert!(!Kind::Rect.has_effect());
        assert_eq!(Kind::Rect.default_intensity(), None);
        assert_eq!(Kind::Rect.default_effect(), None);
        // The number 模糊 opens with is the one the shipped config says, from one place.
        assert_eq!(
            BLUR_RADIUS,
            crate::config::Annotation::default().blur_radius
        );
    }

    #[test]
    fn the_kind_picks_the_effect_and_the_style_only_the_number() {
        // §5.7.10: an element drawn with 模糊 while its style happened to carry a mosaic
        // used to paint a mosaic, because nothing asked the element which tool made it.
        let style = Style {
            effect: Effect::Mosaic { block: 20 },
            ..Style::default()
        };
        assert_eq!(style.effect_for(Kind::Blur), Effect::Blur { radius: 20 });
        assert_eq!(style.effect_for(Kind::Mosaic), Effect::Mosaic { block: 20 });

        let style = Style {
            effect: Effect::Blur { radius: 7 },
            ..Style::default()
        };
        assert_eq!(style.effect_for(Kind::Mosaic), Effect::Mosaic { block: 7 });
        assert_eq!(style.effect_for(Kind::Blur), Effect::Blur { radius: 7 });
        // A tool with no effect is not given one by the lookup.
        assert_eq!(style.effect_for(Kind::Rect), Effect::Blur { radius: 7 });
        // The number is clamped on the way out, so a hand-written file cannot ask for a
        // radius the toolbar would then show as a different value.
        let style = Style {
            effect: Effect::Mosaic { block: 500 },
            ..Style::default()
        };
        assert_eq!(
            style.effect_for(Kind::Blur),
            Effect::Blur { radius: EFFECT_MAX }
        );
    }

    #[test]
    fn a_strength_written_for_one_tool_does_not_move_the_other() {
        let mut style = Style::default();
        style.set_intensity(Kind::Blur, 20);
        assert_eq!(style.effect, Effect::Blur { radius: 20 });
        assert_eq!(style.intensity(), 20);
        style.set_intensity(Kind::Mosaic, 4);
        assert_eq!(style.effect, Effect::Mosaic { block: 4 });
        assert_eq!(style.intensity(), 4);

        style.set_intensity(Kind::Mosaic, 0);
        assert_eq!(style.intensity(), EFFECT_MIN);
        style.set_intensity(Kind::Blur, 1000);
        assert_eq!(style.intensity(), EFFECT_MAX);
        // A tool with no strength knob cannot be handed one, and must not inherit the
        // number the toolbar was showing when it was switched away from.
        style.set_intensity(Kind::Rect, 50);
        assert_eq!(style.effect, Effect::Blur { radius: EFFECT_MAX });
    }

    #[test]
    fn re_editing_one_channel_leaves_the_others_alone() {
        // §5.7.16 step 3: 修改…颜色、线宽、透明度 — one at a time, on a style that already
        // exists. Each of these three is allowed to move exactly one field.
        let mut style = Style {
            color: [200, 30, 40, 255],
            width: 9,
            font_size: 40,
            fill: Some(Style::translucent([200, 30, 40, 255])),
            ..Style::default()
        };
        let before = style.clone();
        style.set_alpha_pct(50);
        assert_eq!(style.color[3], MARKER_ALPHA, "§5.7.8's 半透明 is 50%");
        assert_eq!(
            style.color[..3],
            before.color[..3],
            "the wheel moves the alpha, not the hue"
        );
        assert_eq!(style.width, 9);
        assert_eq!(style.font_size, 40);
        let fill = style.fill.unwrap();
        assert_eq!(fill[..3], before.color[..3], "the plate keeps its colour");
        assert_eq!(
            fill[3],
            (MARKER_ALPHA as u32 * FILL_ALPHA_PCT / 100) as u8,
            "a fill that was a third of the pen is still a third of the new pen"
        );

        style.set_font_size(4);
        assert_eq!(
            style.font_size, FONT_MIN,
            "below the floor the letters are mush"
        );
        style.set_font_size(500);
        assert_eq!(style.font_size, FONT_MAX);
        assert_eq!(style.width, 9, "the 字号 knob does not move the 线宽");

        let mut no_fill = style.clone();
        no_fill.fill = None;
        no_fill.set_alpha_pct(100);
        assert_eq!(no_fill.color[3], 255);
        assert!(
            no_fill.fill.is_none(),
            "a wheel over an unfilled box does not fill it"
        );
    }

    #[test]
    fn a_plate_is_the_pen_at_the_plate_s_own_fraction() {
        assert_eq!(
            Style::translucent([7, 8, 9, 255])[3],
            76,
            "30% of a full pen, which is what the layer has always drawn"
        );
        let c = Style::translucent([7, 8, 9, 100]);
        assert_eq!(c[3], 30);
        assert_eq!(&c[..3], &[7, 8, 9][..], "the plate is the pen's colour");
    }

    #[test]
    fn the_transparency_ladder_never_reaches_an_invisible_pen() {
        assert_eq!(alpha_from_pct(0), alpha_from_pct(ALPHA_MIN_PCT));
        assert!(
            alpha_from_pct(0) > 0,
            "alpha 0 is no ink at all: an element that covers the picture and paints \
             nothing is a bug the user cannot see"
        );
        assert_eq!(alpha_from_pct(150), 255);
        assert_eq!(alpha_from_pct(50), MARKER_ALPHA);
        assert_eq!(pct_from_alpha(255), 100);
        assert_eq!(pct_from_alpha(MARKER_ALPHA), 50, "§5.7.8 reads back as 50%");
        for pct in (ALPHA_MIN_PCT..=100).step_by(ALPHA_STEP_PCT as usize) {
            assert_eq!(
                pct_from_alpha(alpha_from_pct(pct)),
                pct,
                "{pct}% has to read back as {pct}%"
            );
        }
    }

    #[test]
    fn the_corner_ladder_and_the_box_it_is_drawn_on_share_one_number() {
        // The ladder's two ends are one pair and the box's half-side the other stop; both
        // hands (§5.7.3 step 2's cells and step 4's control point) read the same numbers.
        let mut style = Style::default();
        style.set_corner_radius(CORNER_MAX + 500);
        assert_eq!(style.corner_radius, CORNER_MAX, "天花板之上没有第二个半径");
        style.set_corner_radius(u32::MAX);
        assert_eq!(style.corner_radius, CORNER_MAX);
        style.set_corner_radius(0);
        assert_eq!(style.corner_radius, CORNER_MIN, "地板那一档是直角，不是第二种「没有圆角」");
        assert_eq!(
            drawn_corner(CORNER_MAX, &PhysRect::new(0, 0, 20, 200)),
            10,
            "一枚 20 px 宽的框画不出 64 px 的圆角：短边的一半才是它画得出来的那一档"
        );
        assert_eq!(drawn_corner(4, &PhysRect::new(0, 0, 20, 20)), 4);
        assert_eq!(
            drawn_corner(CORNER_MAX, &PhysRect::new(0, 0, 300, 300)),
            CORNER_MAX,
            "大到装得下整把阶梯时，拦它的是阶梯不是框"
        );
    }

    #[test]
    fn a_ring_with_no_colour_of_its_own_steps_aside_for_the_pen() {
        let mut style = Style::default();
        assert!(!style.text_outline, "出厂不开描边：一圈边是用户要的那一手，不是默认画面");
        assert_eq!(style.outline_width, OUTLINE_WIDTH_PX, "没配过的那一档仍是今天这个数");
        // 深色笔 ⇒ 白圈，浅色笔 ⇒ 黑圈：这一半是「用笔自己的颜色描一圈等于没描」那句老规矩。
        assert_eq!(style.outline_for().0, [255, 255, 255, 255]);
        style.color = [255, 255, 128, 255];
        assert_eq!(style.outline_for().0, [0, 0, 0, 255], "浅色笔拿到一枚浅色圈");
        // 配过颜色之后就不问笔了 —— 那一格现在是用户的，不是这一族的读数。
        style.set_outline_colour([10, 20, 30]);
        assert_eq!(style.outline_color, Some([10, 20, 30, 255]));
        assert_eq!(style.outline_for().0, [10, 20, 30, 255]);
        style.color = [1, 2, 3, 255];
        assert_eq!(style.outline_for().0, [10, 20, 30, 255], "换了笔色不该把配好的圈抢回去");
        // 宽度走笔那把尺，一圈边为零像素的"开关亮着"是不许存在的。
        style.set_outline_width(0);
        assert_eq!(style.outline_width, WIDTH_MIN, "地板之上没有第二把粗细");
        style.set_outline_width(WIDTH_MAX + 9);
        assert_eq!(style.outline_width, WIDTH_MAX);
        style.outline_width = 0;
        assert_eq!(style.outline_for().1, 1, "画出来的一圈至少一个像素");
        assert!(Kind::Text.has_outline() && Kind::Number.has_outline());
        assert!(!Kind::Rect.has_outline(), "描边是字的那一手，矩形没有它");
    }
}
