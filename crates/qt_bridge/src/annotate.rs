//! The annotation layer of one capture flow: what the user draws on top of the
//! frozen desktop, the canvas it is rastered into, and the texture Qt shows.
//!
//! M4a's first cut. Three of its parts already exist and are tested in
//! `falcon_core::annotation` - [`Document`] (the objects), [`Command`] +
//! [`UndoStack`] (the edits) and [`raster`] (the pixels). What this module adds is
//! the part that only exists inside a live mask:
//!
//! * **One canvas per screen, in that screen's own device pixels.** A `Frame`'s
//!   bounds always start at (0,0) (see [`Frame::bounds`]), so a document written in
//!   desktop pixels cannot be painted into a screen-local canvas directly -
//!   [`Layer::view_of`] is the one place the two spaces are reconciled. The
//!   arithmetic is tested at a negative desktop origin, because this machine cannot
//!   produce one and "only right on a desktop that starts at zero" is not something
//!   a mask that spans screens may assume.
//! * **Transparent outside the selection.** The dim is QML's job and the hole is
//!   Rust's, so the layer carries the picture *plus* the ink inside the hole and
//!   nothing outside it. That keeps §5.7.1's "标注不应修改原始截图" literally true:
//!   `base` is the frozen frame, never written to, and every ink pixel is recomputed
//!   from the document.
//! * **A new texture key per repaint.** The same rule the frozen frame follows in
//!   [`crate::mask`]: `Image.cache: false` only re-requests when the URL changes, so
//!   a constant key means the second stroke never reaches the screen - plan §3.6's
//!   constraint 8, which P4 measured on the spike and which is easier to hit for
//!   real here, where every stroke is a whole new layer.
//!
//! Nothing in here needs a window. [`Layer::flush`] is the only call that touches
//! Qt, and it is deliberately separate from the painting so the state machine - and
//! the byte-for-byte comparisons the gauge and the tests make - can run headless.

use std::collections::BTreeMap;
use std::time::Instant;

use falcon_core::annotation::model::{
    Align, ArrowHead, Dash, Document, Element, Geom, Kind, Style, Transform,
};
use falcon_core::annotation::undo::UndoStack;
use falcon_core::annotation::{
    alpha_command, alpha_from_pct, clamp_rotation, colour_command, font_size_command, grip_at_any,
    grip_point, intensity_command, patch_style_command, pct_from_alpha, radius_grip_command, raster,
    resize_grip_command, resize_to_rect, rotate_command, rotation_at, text_command, Command, Dirty,
    Grip, Selection, ALPHA_MIN_PCT, ALPHA_STEP_PCT, BLUR_RADIUS, EFFECT_MAX, EFFECT_MIN, FONT_MAX,
    FONT_MIN, GRIP_SLOP,
};
use falcon_core::capture::ScreenSnapshot;
use falcon_core::colors::{format as format_color, ColorFormat};
use falcon_core::config::{Annotation, ToolStyle};
use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect};
use falcon_core::imageops::Shape;
use platform_windows::glyphs::DirectWrite;

use crate::mask::Slot;
use crate::mask_view::shim;

/// The tools the toolbar offers, in the order QML draws them. Index 0 is `None`:
/// the arrow tool, which leaves the selection alone for the pointer and is what
/// §5.7.16/§5.7.17 will hang the object-editing gestures on.
///
/// Deliberately absent, each for a missing capability rather than a missing button:
///
/// 自由选择 (§5.7.17) and 旋转 (§5.7.15) are *not* on this list because they are
/// missing: they are the arrow tool's own two gestures plus the 「对象」 row's six
/// cells — [`Layer::set_marquee`] and [`Layer::step_rotation`] are two of them — and they
/// live on tool index 0 rather than on a tool of their own: 框选 is a mode of pointing at
/// objects, not a pen.
///
/// 文本 (§5.7.11) is in the list, and so is its gesture: a click places the box, the
/// field that appears takes the typing, and the next click anywhere else commits it.
/// Step 4 has its row now — 字号、对齐、背景、描边 next to the 颜色 every pen already had.
/// 字体 is the one still without a control: a family name needs a string field rather
/// than a cell, and it is the same leg whose 中文 readings §9.3 still owes.
///
/// 编号 (§5.7.12) is in the list, and so is its interaction: [`Kind::Number`] lays out
/// through the same paint path 文本 uses, the counter lives in [`Document`], and steps 2
/// and 3 have their row — 一次点击落一枚号，起始编号是那一排的两枚钮。Step 5「删除中间
/// 编号时可选择是否自动重排」是「对象」那一排删除旁边的那一枚（[`Layer::auto_renumber`]）：
/// 亮着的删除由 [`falcon_core::annotation::delete_and_renumber_command`] 记成一条
/// [`Command::Compound`]，于是一次 Ctrl+Z 退回复的是**那一次编辑**，不是它的两半。开关摆在
/// 删除旁边而不是编号那一排，理由写在 `qml/AnnotationToolbar.qml` 的那一行头上。
pub const TOOLS: &[Option<Kind>] = &[
    None,
    Some(Kind::Rect),
    Some(Kind::RoundedRect),
    Some(Kind::Ellipse),
    Some(Kind::Line),
    Some(Kind::Polyline),
    Some(Kind::Arrow),
    Some(Kind::DoubleArrow),
    Some(Kind::Pencil),
    Some(Kind::Marker),
    Some(Kind::Mosaic),
    Some(Kind::Blur),
    Some(Kind::Text),
    Some(Kind::Number),
    Some(Kind::Zoom),
    Some(Kind::Eraser),
];

/// §5.7.13 step 5's 放大倍数 range, in one pair of numbers shared by the knob and by
/// [`Layer::place`]: two clamps that disagree about what 800% means would light the
/// toolbar cell for a size the copy is not drawn at.
const ZOOM_MIN: u32 = 100;
const ZOOM_MAX: u32 = 800;

// §5.7.11's second rule asks the 描边 for a colour *and* a width, and 自本轮起那两格是**用户**
// 的数：颜色的主人是 `Style::outline_color`、宽度是 `Style::outline_width`，而没人配过时让给笔色
// 那一判住在 [`falcon_core::annotation::outline_of`]（[`Layer::set_text_outline`] 只剩开关这一
// 件事）。这一层因此不再自带一枚宽度常量 —— 一把只在开关下才有人读的宽度，正是「控制这一格的是
// 量具还是产品」会答错的地方。

/// §5.7.15 step 2「输入旋转角度」那一格有多宽。QML 只递方向，所以阶梯是这里的数 —— 与
/// 透明度那把阶梯同一分工（[`Layer::step_alpha`]）：`.qml` 里再写一把，就会有一把与这把不一致。
const ROTATE_STEP_DEG: f64 = 15.0;

/// §5.7.18 step 3 的副本落点：偏移这么多。0 是让副本原样压在那一枚下面，屏幕上看不出多了一
/// 件东西；太远则 step 4 那一挪变成一次长途搬运。16 是这个模块的数，与把手的命中容差
/// [`GRIP_SLOP`] 同一量级，所以挪开一点之后仍然抓得住。
const CLONE_STEP: i32 = 16;

/// §5.7.6 step 2「选择单向、双向或其他箭头样式」里那**其他样式**的梯子。三档的名字与序号都
/// 从这一张表来（[`Layer::arrow_head_code`] 报的 index、界面上那三枚钮亮的就是这一行），因为
/// `ArrowHead` 的枚举顺序是 core 的、钮的顺序是这个模块的，两处各排一次迟早会不一致（#281）。
/// 单向与双向不在这一行里：那是 `TOOLS` 上两枚不同的工具，不是一枚样式。
const ARROW_HEADS: [ArrowHead; 3] = [ArrowHead::Triangle, ArrowHead::Chevron, ArrowHead::Circle];

/// §5.7.8 规则「支持圆形或椭圆形笔刷」加上 §5.7.9 / §5.7.10 步骤 2 那一枚**形状**的梯子。
/// 三档与 [`ARROW_HEADS`] 同一形状：名字与序号都从这一张表来，界面上那三枚钮亮的就是这一行
/// （#281）。顺序是这一页的数 —— 圆头排在最前因为它是 [`falcon_core::imageops::Shape`] 之外
/// 唯一出厂就戴着的那一档（`Brush::default`），把新档插在中间会让上一次会话记住圆头的那位用户
/// 睁开眼看见一枚方头。
const BRUSH_SHAPES: [Shape; 3] = [Shape::Circle, Shape::Rect, Shape::Ellipse];

/// §5.7.3 step 2「配置圆角半径」的两端与一格。地板是 0：那一档圆角半径为 0 的圆角矩形就是
/// 一枚直角矩形，raster 画的正是这条式子，为它另立一个开关会得到第二种"没有圆角"。
const CORNER_MIN: u32 = 0;
const CORNER_MAX: u32 = 64;
/// 一格有多宽。与透明度那把阶梯同一分工（[`Layer::step_alpha`]）：QML 只递方向。
const CORNER_STEP: i32 = 4;

/// §5.7.12 step 2「设置起始编号」的两端与一格。地板是 1：编号是给读者看的序数，从 0 起
/// 的第一枚会在屏幕上写成一枚"0 步"。天花板 9999 只限这一把梯（读出来的那三位数住在
/// 界面的一格里），不是文档的上限 —— `Document` 的计数器本身没有顶。
const COUNTER_MIN: u32 = 1;
const COUNTER_MAX: u32 = 9999;
/// 一格一步：编号是逐个数出来的，一格跨两级就是让用户数错。
const COUNTER_STEP: i32 = 1;

/// §5.7.12 step 3「在截图上依次点击」落下的那一格有多高。盒子是**排版要用的**那只框：
/// `raster` 的 `Kind::Text | Kind::Number` 那一支把字按框宽折行，所以宽度按位数给
/// （每一位一号字宽，再加一号的空），高度给两倍字号。这不是第二把字号的尺 —— 字号仍
/// 由 [`Style::font_size`] 说，这里只回答"这一串数字要占多宽的一框"。
const NUMBER_BOX_FONTS: u32 = 2;

/// The tool a code from QML names. Anything out of range is the arrow tool rather
/// than an error: a stale number from a reloaded document must not disable input.
pub fn tool_at(code: i32) -> Option<Kind> {
    *TOOLS.get(code.max(0) as usize)?
}

/// The code for a tool, for the toolbar's checked state.
pub fn code_of(tool: Option<Kind>) -> i32 {
    TOOLS
        .iter()
        .position(|k| *k == tool)
        .unwrap_or(0)
        .try_into()
        .unwrap_or(0)
}

/// The tool row in **one** table: the kind, the name on the button, and the sentence
/// a hover owes the user (§5.21.2 第一枚「工具栏按钮应提供提示文本」).
///
/// One table rather than two lists because two lists are how 橡皮's button ends up
/// saying 画笔: `TOOLS` is the order of the codes, this is the order of `TOOLS`, and
/// index `i` of both has to name the same tool — which no binding can check and every
/// click would find out.
///
/// Each hint starts with its own button's name for the same reason a doc comment has
/// to sit next to the thing it describes: a tooltip that names a different tool is a
/// sentence about something else, and "the hint is non-empty" would let it pass.
const TOOL_TEXTS: [(Option<Kind>, &str, &str); 16] = [
    (None, "选择", "选择：挪动选区，或选中已有的标注"),
    (Some(Kind::Rect), "矩形", "矩形：拖出一枚矩形框"),
    (Some(Kind::RoundedRect), "圆角", "圆角：拖出一枚圆角的矩形"),
    (Some(Kind::Ellipse), "椭圆", "椭圆：拖出一枚椭圆"),
    (Some(Kind::Line), "直线", "直线：拖出一段直线"),
    (
        Some(Kind::Polyline),
        "折线",
        "折线：逐点连下去，双击收住这条线",
    ),
    (Some(Kind::Arrow), "箭头", "箭头：拖出一支单头的箭头"),
    (
        Some(Kind::DoubleArrow),
        "双向",
        "双向：拖出一支两头都带箭头的线",
    ),
    (Some(Kind::Pencil), "画笔", "画笔：按住拖出手写的轨迹"),
    (Some(Kind::Marker), "荧光", "荧光：拖出一条半透明的高亮"),
    (Some(Kind::Mosaic), "马赛克", "马赛克：把拖过的地方打碎"),
    (Some(Kind::Blur), "模糊", "模糊：把拖过的地方虚化"),
    (Some(Kind::Text), "文本", "文本：拖出一只框，再往里打字"),
    (
        Some(Kind::Number),
        "编号",
        "编号：点一下落一枚号，再点就是下一号",
    ),
    (Some(Kind::Zoom), "放大", "放大：先拖源区，再拖放大图落在哪"),
    (Some(Kind::Eraser), "橡皮", "橡皮：拖过的标注被擦掉"),
];

const _: () = assert!(TOOLS.len() == TOOL_TEXTS.len());

/// The toolbar's labels, in [`TOOLS`] order and pipe-separated.
///
/// Rust owns the list because a QML copy of it is a second source of truth that can
/// be renamed out of step with the enum - and index `i` of the labels has to name
/// `TOOLS[i]`, which is the one part of a toolbar that no binding can check.
///
/// The words are the PRD's own §5.7 headings, shortened to what a button holds:
/// 圆角矩形 is §5.7.3 and draws as 圆角.
pub fn tool_names() -> String {
    TOOL_TEXTS
        .iter()
        .map(|(_, name, _)| *name)
        .collect::<Vec<_>>()
        .join("|")
}

/// The same row's hover text, in [`TOOLS`] order and pipe-separated - §5.21.2 第一枚
/// 的另一半，与 [`tool_names`] 同枚数同序，所以第 `i` 句提示只能是第 `i` 枚钮的。
pub fn tool_hints() -> String {
    TOOL_TEXTS
        .iter()
        .map(|(_, _, hint)| *hint)
        .collect::<Vec<_>>()
        .join("|")
}

/// 局部放大 (§5.7.13) is the one tool whose gesture is *two* drags, so the first
/// drag's answer has to survive the release between them. That makes it 折线's shape
/// rather than every other tool's: like `poly`, this state is neither an `Element` nor
/// a draft the undo stack can see, and every path that lets the gesture go has to
/// reclaim it (§9.1's ⑦).
#[derive(Clone, Copy, Debug)]
struct Placement {
    /// The source region, as the first drag left it.
    from: PhysRect,
    /// The enlarged copy: generated from `from` at [`Layer::place`], then moved by the
    /// second drag.
    to: PhysRect,
    /// `Some` while the second drag is held - where the button went down and what `to`
    /// was then. The copy follows the pointer's *travel*: putting its origin under the
    /// cursor is §9.1's `resize`-`Body` mistake, on a much bigger object.
    held: Option<(PhysPoint, PhysRect)>,
}

/// §5.7.11's 文本 in progress: the click that placed the box, and what has been typed
/// into it since.
///
/// Like [`Placement`] this is state the undo stack cannot see, so every path that lets
/// the gesture go has to reclaim it (§9.1's ⑦). Unlike it, and unlike 折线's nodes, the
/// box owns **no pixels**: while it is in hand the letters are drawn by QML's own text
/// field, not by this layer, because both of them painting the same string at the same
/// time is a doubled glyph on the screen. That is why [`Layer::abandon_typing`] answers
/// "was there a box" where the other two answer "were pixels of it on screen", and why
/// none of the reclaiming callers fold it into their `dropped` repaint decision.
#[derive(Clone, Debug)]
struct Typing {
    /// The box's top-left, in document space: §5.7.11 step 2's click, clamped into the
    /// selection by [`Layer::doc_point`] like every other gesture's start. A box opened
    /// by [`Layer::edit_selected_text`] takes it from the object it came to retype.
    at: PhysPoint,
    /// Step 3's answer, as the field last had it. The whole string each time, not a
    /// delta: the field owns the caret, the selection and the IME, and a layer that
    /// tried to keep its own copy of an edit it did not see would disagree with it.
    text: String,
    /// Which object this box came to **retype** (§5.7.16 step 3's 文字内容), `None` for
    /// the one §5.7.11 is drawing now. The same box, the same field on the page and the
    /// same finisher either way - what this number decides is only whether the string
    /// leaves the layer as an [`Command::Add`] or as a [`falcon_core::annotation::text_command`]
    /// against an id already in the document, and whose 字号 the box is measured in
    /// ([`Layer::box_style`]).
    editing: Option<u64>,
}

/// One screen's share of the layer: its frozen pixels, the canvas on top of them,
/// and the key that canvas was last published under.
#[derive(Clone, Debug)]
pub struct Canvas {
    /// The monitor's id, which is also the slot's name - the join key.
    pub name: String,
    /// Where this screen sits, in desktop device pixels.
    pub bounds: PhysRect,
    /// The frozen frame, never written to. §5.7.1's "不修改原始截图" is this field.
    base: Frame,
    /// What is on screen: `base` plus the ink, inside the hole; transparent outside.
    layer: Frame,
    /// The key `layer` was last handed to Qt under, empty before the first time.
    published: String,
    /// The layer's version, and the version last published. They differ for exactly
    /// as long as this canvas has pixels nobody has been given yet.
    painted_rev: u32,
    published_rev: u32,
}

/// One object-selection gesture in flight (§5.7.15 step 2, §5.7.16 step 3, §5.7.17
/// steps 3-4). `Move` drags the whole selection; `Grip` drags one control point of one
/// object, and [`Grip::Rotate`] — being a control point like any other — turns it.
///
/// `Grip` carries the corner radius the element had **at the press** because
/// [`Grip::Radius`] is the one handle that edits a *style* number through a drag: the
/// preview writes that number into the document on every pointer event, so a radius
/// measured against the live number would be measured against the drag's own last
/// answer ([`radius_grip_command`] asks for the start value for exactly that reason).
/// The other grips are geometry, and geometry is what [`SelDrag::from`] already snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelMode {
    Move,
    Grip {
        id: u64,
        grip: Grip,
        corner: u32,
    },
    /// §5.7.18 steps 2-4「按住 Ctrl 拖动」：这一趟拖的是**副本**。按下那一刻副本已经摆进文档
    /// （摆在与原处同一格，`dx`/`dy` 都是 0），而 [`SelDrag::from`] 记的就是它们那一刻的几何 ——
    /// 于是拖拽与松手用的是「移动」那一套现成的机器，差别只在栈上落的是**一条添加**而不是那条
    /// 移动（[`Layer::finish_clone_drag`]）。
    ///
    /// 这一格不带 id：`SelMode` 是 `Copy` 的（框选与把手都靠它按值传），而 id 本来就在 `from` 里。
    Clone,
    /// §5.7.17 steps 2-3: 拖动矩形框, 选中范围内的一个或多个. Not an edit — the undo stack
    /// never sees "I drew a box" — so it files no [`Command`] and its own rubber band
    /// lives in [`Layer::draft`] until the button comes up.
    Marquee,
}

/// What the gesture started from. Both ends of every object it can touch are kept, in the
/// spelling [`Command`] stores: the drag is measured from the press rather than from the
/// previous mouse event, so a preview that is interrupted and resumed cannot drift, and
/// the one command filed at release has the exact "before" the undo stack needs.
#[derive(Clone, Debug)]
struct SelDrag {
    mode: SelMode,
    start: PhysPoint,
    from: Vec<(u64, Geom, Transform)>,
    /// 最后那一枚指针位置换算出来的命令。留在手势里而不是留指针坐标，是因为
    /// [`Layer::release`] 不带坐标 —— 松开的那一枚按钮只说"到了"，不说"到了哪儿"。
    /// 每一枚拖拽事件重算它（文档先按回 `from`，再从那里算），所以落在栈上的那一条
    /// 的 "before" 与 `from` 是同一份东西，一次 Ctrl+Z 拿回的正是按下那一刻。
    to: Option<Command>,
}

impl SelDrag {
    fn take(mode: SelMode, start: PhysPoint, doc: &Document, ids: &[u64]) -> SelDrag {
        SelDrag {
            mode,
            start,
            from: ids
                .iter()
                .filter_map(|id| doc.get(*id).map(|e| (*id, e.geom.clone(), e.transform)))
                .collect(),
            to: None,
        }
    }
}

/// The layer itself. It lives inside [`crate::mask::MaskState`], which owns the one
/// lock, so a hole and its ink can never be read from two different moments.
#[derive(Clone)]
pub struct Layer {
    /// The objects, in *document space*: desktop pixels with the desktop's own
    /// origin moved to (0,0), which is what a `Frame` and a `Document` both require.
    doc: Document,
    undo: UndoStack,
    /// `None` is the arrow tool: the pointer adjusts the selection, exactly as it
    /// did before any of this existed.
    tool: Option<Kind>,
    /// §5.7.16/§5.7.17's object selection: the ids the panel is describing and the
    /// gestures act on. Empty is "nothing picked", which is how every path that existed
    /// before this field reads — the pen owns the toolbar until a click picks something.
    sel: Selection,
    /// The selection gesture in flight, if the pointer is holding one.
    sel_drag: Option<SelDrag>,
    /// §5.7.17 step 1 的「进入选择模式」：为真时箭头工具在选区里的一拖**框选**对象，而不是
    /// 抓住那块框。代价说在工具栏那一枚钮上：这一档里把手与单枚选中都让开，要挪东西得先退出。
    marquee: bool,
    /// §5.7.12 step 5「删除中间编号时可选择是否自动重排」里的那一枚选择键。默认**灭**：亮着的
    /// 这一次删除会挪动屏幕上其它编号的位置，那是用户没直接落的一笔，而「只删我一枚」是这一排
    /// 钮在被发明之前一直的行为。灭与亮各有一个入口，选哪一个由 [`Layer::delete_selected`] 问。
    auto_renumber: bool,
    /// New objects take this style; each tool keeps its own last one in
    /// [`Layer::styles`], which is §5.7.1's "工具切换时，应记忆该工具最近一次使用的样式".
    style: Style,
    styles: Vec<(Kind, Style)>,
    /// What `[annotation] blur_radius` said at the last read: the number 模糊 opens with
    /// when the tool has no memory of its own. The key has existed in the file since the
    /// config did, with nothing on the other end of it, and §5.7.10's 模糊强度 is what it
    /// was written for.
    default_blur: u32,
    canvases: Vec<Canvas>,
    /// The desktop origin the document space is normalised by: `device = doc + origin`.
    origin: PhysPoint,
    /// The selection, in desktop pixels. Ink outside it is neither drawn nor kept.
    hole: PhysRect,
    /// Where the stroke being dragged started, in document space.
    anchor: Option<PhysPoint>,
    /// §5.7.4 step 3「按住约束键可创建正圆」的那只手此刻按着没有：按着的这一笔，拖出来的
    /// 框锁成正方形（椭圆于是就是正圆）。每一寸移动都由界面按当下那颗键重写一次，
    /// [`Layer::press`] 起笔时先归 false，所以不存在"键早松了、笔还锁着"的第二把状态。
    constraint: bool,
    /// §5.7.18 步 2「按住 Ctrl 拖动」的那只手此刻按着没有：界面在**每一枚按下之前**把当下
    /// `mouse.modifiers` 里那枚 Ctrl 递进来（[`Layer::hold_clone`]），[`Layer::press`] 读一次
    /// 就归 false —— 所以这一格说的是「**这一枚**按下按着 Ctrl」，不存在"上一次按着 Ctrl 的
    /// 按下把这一次没按的拖成克隆"。
    ///
    /// 它与 [`Layer::constraint`] 是分开的两格，因为两枚键说的话不同：约束键说的是**拖出来的
    /// 框锁 1:1**、每一寸移动都重写；这一枚说的是**这一趟拖的是谁**、只在按下那一刻有意义。
    /// 中途松手不把已经开始的克隆改回移动 —— 一只手不能一半拖副本、一半拖原物。
    clone_hold: bool,
    /// The freehand points so far.
    path: Vec<PhysPoint>,
    /// 折线 (§5.7.5) does not live in `path`: its nodes are added by *successive
    /// clicks*, so they have to survive the release between two of them. `poly` is
    /// the nodes committed that way, and `poly_cursor` the point the rubber segment
    /// currently runs to - which only becomes a node when the button comes up there.
    poly: Vec<PhysPoint>,
    poly_cursor: Option<PhysPoint>,
    /// 局部放大 (§5.7.13) in progress: the source region and the copy generated from it.
    zoom: Option<Placement>,
    /// 文本 (§5.7.11) in progress: the box the last click placed and the string typed
    /// into it, which QML's field is showing.
    typing: Option<Typing>,
    /// The stroke being dragged: painted, but not in the document until release.
    draft: Vec<Element>,
    /// Bumped by every repaint, and the number every published key carries.
    rev: u32,
    next_id: u64,
    pub problems: Vec<String>,
    // ---- the measurements the gauge prints rather than guesses
    /// Repaints done, the pixels the last one touched, and how long it took. The
    /// dirty-rect claim ("only what moved") is worth nothing without these beside
    /// it: a whole-layer re-render would look the same from QML.
    pub paints: u32,
    pub last_paint_ms: Option<u64>,
    pub last_paint_px: u64,
    pub full_px: u64,
    /// Publishes, and the keys the last one produced. Two commits sharing a key is
    /// constraint 8 failing, and here that is a number rather than a suspicion.
    pub flushes: u32,
    pub last_keys: Vec<String>,
    clock: Instant,
}

impl Default for Layer {
    fn default() -> Self {
        Layer {
            doc: Document::new(0, 0),
            undo: UndoStack::new(),
            tool: None,
            sel: Selection::new(),
            sel_drag: None,
            marquee: false,
            auto_renumber: false,
            style: Style::default(),
            styles: Vec::new(),
            default_blur: BLUR_RADIUS,
            canvases: Vec::new(),
            origin: PhysPoint::new(0, 0),
            hole: PhysRect::default(),
            anchor: None,
            constraint: false,
            clone_hold: false,
            path: Vec::new(),
            poly: Vec::new(),
            poly_cursor: None,
            zoom: None,
            typing: None,
            draft: Vec::new(),
            rev: 0,
            next_id: 1,
            problems: Vec::new(),
            paints: 0,
            last_paint_ms: None,
            last_paint_px: 0,
            full_px: 0,
            flushes: 0,
            last_keys: Vec::new(),
            clock: Instant::now(),
        }
    }
}

/// Desktop-space rects to one canvas's pixels: the shift every document object has
/// to take before it can be painted into a frame that starts at that screen's
/// top-left. `origin` is the desktop's own corner, `bounds` this screen's.
fn shift(origin: PhysPoint, bounds: PhysRect) -> (i32, i32) {
    (origin.x - bounds.x, origin.y - bounds.y)
}

/// A document in one canvas's coordinates, carrying these elements. Built by hand
/// rather than through [`Document::add`] because `add` re-numbers and re-stacks -
/// and a repaint must draw the objects with the z they have, or a layer that is
/// undone and redone comes out in a different order than it went in.
fn view_of(elements: &[Element], canvas: &Canvas, dx: i32, dy: i32) -> Document {
    let mut doc = Document::new(canvas.base.width, canvas.base.height);
    doc.base = canvas.base.bounds();
    doc.elements = elements
        .iter()
        .filter(|e| e.visible)
        .map(|e| {
            let mut e = e.clone();
            e.geom = e.geom.translate(dx, dy);
            e
        })
        .collect();
    doc
}

impl Layer {
    /// Start a flow from nothing but keep what the user taught us: `styles` is
    /// §5.7.1's per-tool memory, and a second capture in the same process is not a
    /// reason to forget it. Everything else in a `Layer` belongs to the flow.
    fn clear_flow(&mut self) {
        let styles = std::mem::take(&mut self.styles);
        // The config's number, not the flow's: a second capture in one process is not a
        // reason for 模糊 to forget what the file said, even before the next apply.
        let default_blur = std::mem::take(&mut self.default_blur);
        // §5.7.12 step 5 的那一枚开关同理：它记的是这个人要什么样的删除，不是这一张图上的
        // 一笔。第二张截图把它抹掉，用户上一次亮过的开关就在他没碰过的地方灭了。
        let auto_renumber = self.auto_renumber;
        *self = Layer::default();
        self.styles = styles;
        self.default_blur = default_blur;
        self.auto_renumber = auto_renumber;
    }

    /// A new flow: each slot's frozen frame becomes a base, and the document starts
    /// empty. Called from [`crate::mask::MaskState::open`], so the ink and the
    /// picture it sits on are always the same capture.
    pub fn begin(&mut self, snap: &ScreenSnapshot, slots: &[Slot]) {
        self.clear_flow();
        self.origin = PhysPoint::new(snap.virtual_bounds.x, snap.virtual_bounds.y);
        self.doc = Document::new(snap.virtual_bounds.w, snap.virtual_bounds.h);
        for slot in slots {
            let Some(m) = snap.monitors.iter().find(|m| m.info.id == slot.name) else {
                self.problems
                    .push(format!("no captured monitor is named {}", slot.name));
                continue;
            };
            let layer = match Frame::filled(m.frame.width, m.frame.height, [0, 0, 0, 0]) {
                Ok(f) => f,
                Err(e) => {
                    self.problems
                        .push(format!("{}: no canvas ({e})", slot.name));
                    continue;
                }
            };
            self.canvases.push(Canvas {
                name: slot.name.clone(),
                bounds: slot.bounds,
                base: m.frame.clone(),
                layer,
                published: String::new(),
                painted_rev: 0,
                published_rev: 0,
            });
        }
        self.full_px = self
            .canvases
            .iter()
            .map(|c| c.base.width as u64 * c.base.height as u64)
            .sum();
    }

    /// The flow is over: every texture back out of Qt's process. A 4K canvas is
    /// 24 MB, one per screen per flow.
    pub fn end(&mut self) {
        for c in &self.canvases {
            if !c.published.is_empty() {
                shim::drop_frame(&c.published);
            }
        }
        self.clear_flow();
    }

    /// The selection moved. The clip moved with it, so everything between the old
    /// clip and the new one has to be decided again - a whole-canvas repaint, which
    /// is also why [`Layer::last_paint_px`] is reported next to the count.
    ///
    /// With nothing to draw the layer is already transparent everywhere, and there is
    /// nothing to re-clip. That case is skipped rather than painted because the hole
    /// moves on every pixel of a selection *drag*, and a full-canvas pass per mouse
    /// move would put the annotation layer inside the P1 latency budget for a state
    /// in which it cannot possibly have anything to show.
    pub fn set_hole(&mut self, hole: PhysRect) {
        if hole == self.hole {
            return;
        }
        self.hole = hole;
        // A selection that went away takes the unfinished gestures with it: a
        // polyline's nodes and a 放大's source region both live in document space, so
        // leaving either pending means the *next* selection inherits a line - or a
        // magnifier - nobody is drawing any more. A half-typed 文本 box would be the
        // third of those, and the field would go on showing text measured against a
        // selection that no longer exists.
        let dropped = if hole.is_empty() {
            let line = self.abandon_polyline();
            let zoom = self.abandon_zoom();
            // Not folded into `dropped`: the box drew no pixels of its own, so there is
            // nothing here that a repaint is being asked to take back.
            self.abandon_typing();
            // The picked objects go with the picture they were picked out of, and a
            // selection of things no longer on screen would leave the panel describing
            // pixels nobody can see.
            self.sel.clear();
            self.drop_sel_drag();
            line || zoom
        } else {
            false
        };
        if self.doc.is_empty() && self.draft.is_empty() && !dropped {
            return;
        }
        self.paint(None);
    }

    /// A stroke is mid-drag, so the pointer belongs to the drawing tool rather than
    /// to the selection's eight grips.
    /// 一只手在手里：新画的那笔（`anchor`）**或对象编辑的那一趟**（`sel_drag`）。
    ///
    /// 后半是 §5.7.15/§5.7.16/§5.7.17 那三只手势在生产路径上的门闩。
    /// [`crate::mask::MaskState::drag`] 与 [`crate::mask::MaskState::release`] 都先问这一句才把
    /// 事件交给 `ink`，而箭头工具从不设 `anchor` —— 少了 `sel_drag` 这一半，一次"拖住选中对象
    /// 挪一挪"的按下会被 [`crate::mask::MaskState::press`] 认下（它看见 `press` 返回 `true` 就
    /// 把 `grab` 置空），然后拖拽与松手两枚事件谁也不转交：屏幕上什么都不动。
    pub fn dragging(&self) -> bool {
        self.anchor.is_some() || self.sel_drag.is_some()
    }

    pub fn tool(&self) -> Option<Kind> {
        self.tool
    }

    pub fn tool_code(&self) -> i32 {
        code_of(self.tool)
    }

    /// Switching tools restores the style that tool was last left with (§5.7.1), and
    /// abandons whatever stroke was half-dragged - including a polyline whose nodes
    /// were clicked one at a time and never finished.
    pub fn select_tool(&mut self, code: i32) {
        let kind = tool_at(code);
        if let Some(k) = kind {
            self.style = self.pen_for(k);
            // Picking up a pen puts the panel back on the pen — §5.7.16 step 4's way out of
            // editing an object is to go and do something else with the toolbar.
            self.sel.clear();
            self.drop_sel_drag();
        }
        self.tool = kind;
        self.anchor = None;
        self.path.clear();
        // Both unfinished gestures are dropped, and both are painted back. Written as
        // two statements rather than `a() || b()` because `||` short-circuits, and a
        // switch that cleared one preview and left the other's state in place is the
        // half-reset §9.1's ⑤ was about.
        let mut dropped = self.abandon_polyline();
        dropped |= self.abandon_zoom();
        // And the box a click placed: switching tools while it is open is the user
        // saying they did not mean to be typing. Not part of `dropped`, for the reason
        // in [`Typing`] - the field had the pixels, and this call already reloads every
        // window, which is what takes the field off the screen.
        self.abandon_typing();
        if dropped {
            self.paint_last();
        }
    }

    /// The pen a tool is picked up with: what it was last left with, or the pen in hand
    /// with the two things this tool is allowed to insist on laid over it — §5.7.8's
    /// half-transparent 荧光 and §5.7.9/§5.7.10's own strength.
    ///
    /// The defaults land here rather than in [`Kind::apply_defaults`]'s caller in
    /// [`Layer::default`] because a tool the user has taught must come back exactly as it
    /// was left, and because the pen in hand is what an *un*taught tool inherits — 直线's
    /// width, then 荧光's alpha, then nothing else.
    ///
    /// It does not call [`Layer::remember_style`]: arriving is not teaching, and a layer
    /// that stored what it only just inferred would write an entry for every tool the
    /// user ever *looked at* (§5.7.1's memory is of what they set).
    fn pen_for(&self, kind: Kind) -> Style {
        if let Some(s) = self.remembered(kind) {
            return s;
        }
        let mut style = self.style.clone();
        kind.apply_defaults(&mut style);
        if kind == Kind::Blur {
            // After `apply_defaults`, so the file's number wins over the model's, which is
            // the order every other `[annotation]` scalar is read in.
            style.set_intensity(Kind::Blur, self.default_blur);
        }
        style
    }

    pub fn color(&self) -> [u8; 4] {
        self.style.color
    }

    pub fn width(&self) -> u32 {
        self.style.width
    }

    /// The two style switches the toolbar shows as ticks. Read back rather than kept
    /// in QML, because §5.7.1's per-tool memory means a tool can come back with a
    /// dash the button has never seen.
    pub fn dashed(&self) -> bool {
        self.style.dash != Dash::Solid
    }

    pub fn filled(&self) -> bool {
        self.style.fill.is_some()
    }

    /// §5.7.13 step 5's three settings, read back rather than kept in the toolbar for
    /// the same reason [`Layer::dashed`] is: §5.7.1's per-tool memory can hand a tool
    /// back a switch no button has ever touched. `zoom_tool` is the one that says
    /// whether the row belongs on screen at all, and it is Rust's answer because the
    /// index of 放大 in [`TOOLS`] is Rust's number too.
    pub fn zoom_tool(&self) -> bool {
        self.tool == Some(Kind::Zoom)
    }

    pub fn zoom_percent(&self) -> i32 {
        self.style.zoom_percent as i32
    }

    pub fn zoom_border(&self) -> bool {
        self.style.zoom_border
    }

    pub fn connection_line(&self) -> bool {
        self.style.connection_line
    }

    /// §5.7.2 step 2's 颜色, which is also §5.7.16 step 3's 修改颜色 for whatever is picked.
    pub fn set_color(&mut self, rgba: [u8; 4]) {
        self.style.color = rgba;
        self.remember_style();
        let cmd = colour_command(&self.doc, self.sel.ids(), [rgba[0], rgba[1], rgba[2]]);
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// §5.7.2 step 2 / §5.7.21 step 3's 透明度, as the percent the toolbar reads out.
    ///
    /// Read back off the pen rather than kept beside it for the same reason
    /// [`Layer::dashed`] is: the alpha is not a second setting but the fourth byte of the
    /// colour the chip already shows, and §5.7.1's memory can hand a tool back a
    /// transparency no button has ever touched.
    pub fn alpha_percent(&self) -> i32 {
        pct_from_alpha(self.style.color[3]) as i32
    }

    pub fn set_alpha_percent(&mut self, percent: u32) {
        let alpha = alpha_from_pct(percent);
        // 组上那一趟先问：选中框自己的 alpha 与笔的不必相同，所以"笔已经在这格上"不等于
        // "这一句没有可做的改动"。core 那一边把没动的对象算成 `None`，两半各自成立。
        let cmd = alpha_command(&self.doc, self.sel.ids(), percent);
        if self.style.color[3] != alpha {
            self.style.color[3] = alpha;
            // A fill switched on is a fraction of the pen it was switched on with, so the
            // pen's transparency moving under it moves it too: a box that stayed at the old
            // ratio is the knob-that-lies shape [`Layer::set_width`] was written against.
            if self.style.fill.is_some() {
                self.style.fill = Some(Self::fill_colour(self.style.color));
            }
            self.remember_style();
        }
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// The wheel over the colour chip (§5.7.21 step 3): one rung of the ladder per notch,
    /// from wherever the pen stands. QML passes the direction and nothing else, because
    /// the rung size, the floor and the ceiling are Rust's numbers and a second ladder in
    /// a `.qml` file is a second ladder that can disagree with this one.
    pub fn step_alpha(&mut self, notches: i32) {
        let rung = ALPHA_STEP_PCT as i32;
        let percent = (self.alpha_percent() + notches.saturating_mul(rung))
            .clamp(ALPHA_MIN_PCT as i32, 100);
        self.set_alpha_percent(percent.max(0) as u32);
    }

    /// Line width and brush diameter move together: §5.7.20's 多级画笔粗细 is one
    /// number the user sets, and a 2 px pencil whose brush is still 16 px wide is a
    /// tool that ignores the knob. The picked group takes the same two through
    /// [`Layer::restyle`], which is §5.7.16 step 3's 修改线宽.
    pub fn set_width(&mut self, width: u32) {
        let width = width.clamp(1, 64);
        self.style.width = width;
        Self::link_brush(&mut self.style);
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| {
            s.width = width;
            Self::link_brush(s);
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    pub fn set_dashed(&mut self, dashed: bool) {
        self.style.dash = if dashed {
            Dash::Dash(self.style.width * 3, self.style.width * 2)
        } else {
            Dash::Solid
        };
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |e, s| {
            // 虚线的两段是**那一枚对象自己的**线宽三倍与两倍，与笔上同一条式子 —— 一组的
            // 线宽互不相同时，把它们全拧成笔的 dash 会让改实线变成改粗细。
            let w = e.style.width;
            s.dash = if dashed {
                Dash::Dash(w * 3, w * 2)
            } else {
                Dash::Solid
            };
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// §5.7.2 step 4's 填充, at a third of the pen's alpha so a filled box still
    /// shows the picture under it.
    pub fn set_filled(&mut self, on: bool) {
        self.style.fill = on.then(|| Self::fill_colour(self.style.color));
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| {
            s.fill = on.then(|| Self::fill_colour(s.color));
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// The fill's colour, which is the pen's with [`Layer::set_filled`]'s fraction on its
    /// alpha. One expression rather than two because [`Layer::set_alpha_percent`] has to
    /// rebuild a fill that is already switched on, and a second copy of "a third" is a
    /// second answer to what the box looks like at 40%.
    fn fill_colour(color: [u8; 4]) -> [u8; 4] {
        let mut c = color;
        c[3] = (c[3] as u32 * 3 / 10) as u8;
        c
    }

    /// Whether the tool in hand has a strength to set at all: §5.7.9's 块大小 and
    /// §5.7.10's 模糊强度, and no other tool. Rust answers it because the toolbar knowing
    /// which index 马赛克 sits at is a second copy of [`TOOLS`].
    pub fn effect_tool(&self) -> bool {
        self.tool.is_some_and(Kind::has_effect)
    }

    /// The row's label, in the tool's own word: the same number means a block size in one
    /// tool and a blur radius in the other, and "强度" on 马赛克's row would be a sentence
    /// about a different setting.
    pub fn effect_caption(&self) -> String {
        match self.tool {
            Some(Kind::Blur) => "模糊强度".into(),
            Some(Kind::Mosaic) => "块大小".into(),
            _ => String::new(),
        }
    }

    /// §5.7.9's 像素化块大小 / §5.7.10's 模糊强度: one slot, two names, and
    /// [`Layer::effect_caption`] says which one is being shown.
    pub fn effect_intensity(&self) -> i32 {
        self.style.intensity() as i32
    }

    /// Set the strength **as the picked tool's effect**. The kind is taken from the layer
    /// rather than from the style, which is the whole of §5.7.10's defect: a number typed
    /// while 模糊 was picked used to sit in a style that still wore a mosaic, and the
    /// picture came out blocky.
    pub fn set_effect_intensity(&mut self, value: u32) {
        let Some(kind) = self.tool.filter(|k| k.has_effect()) else {
            return;
        };
        self.style.set_intensity(kind, value);
        self.remember_style();
        // 组里每一枚按**它自己的那种**效果吃这个数（`intensity_command` 问的是元素的
        // kind）：一框马赛克配一枚模糊拿到 20，得到的是 20 px 的块与 20 px 的半径。
        let cmd = intensity_command(&self.doc, self.sel.ids(), value);
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// 手里这支笔是不是**拖一支笔**的工具（§5.7.8/§5.7.9/§5.7.10 步骤 2 那一行的门）：只有
    /// 这类工具戴得起 [`BRUSH_SHAPES`] 那一排，界面上那一排也就只在这一枚门下亮。
    pub fn brush_tool(&self) -> bool {
        self.tool.is_some_and(Kind::is_stroke)
    }

    /// 此刻手里那一支笔在 [`BRUSH_SHAPES`] 里排第几：界面上那三枚钮按它决定哪一枚亮，与
    /// [`Layer::arrow_head_code`] 同一形状（序号过桥、名字留在界面那一排）。
    pub fn brush_shape_code(&self) -> i32 {
        BRUSH_SHAPES
            .iter()
            .position(|s| *s == self.style.brush.shape)
            .unwrap_or(0) as i32
    }

    /// §5.7.10 step 2's 形状. Only the tools that drag a brush own one, and the rasteriser
    /// draws all three for every one of them, so this is not a control nothing reads. 序号在
    /// 梯子之外就**不动笔**：三档是 [`BRUSH_SHAPES`] 的数也是界面那三枚钮的数，一枚第四档悄悄
    /// 变成圆头会是一枚说自己改了而没改的钮（[`Layer::set_align`] 立的正是这一条）。
    pub fn set_brush_shape(&mut self, code: i32) {
        let Some(&shape) = usize::try_from(code)
            .ok()
            .and_then(|i| BRUSH_SHAPES.get(i))
        else {
            return;
        };
        if !self.brush_tool() {
            return;
        }
        self.style.brush.shape = shape;
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| {
            s.brush.shape = shape;
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// 手里那支笔是不是箭头（单向或双向都算）：§5.7.6 那一行只在这两枚工具下亮，理由与
    /// [`Layer::brush_tool`] 同一句 —— 工位在 `TOOLS` 里排第几是 Rust 的数，QML 自己比索引
    /// 就是第二把会各自漂移的梯。
    pub fn arrow_tool(&self) -> bool {
        matches!(self.tool, Some(Kind::Arrow) | Some(Kind::DoubleArrow))
    }

    /// 此刻这一档在 [`ARROW_HEADS`] 里排第几：界面上那三枚钮按它决定哪一枚亮，与
    /// [`Layer::align_code`] 同一形状（序号过桥、名字留在界面那一排）。
    pub fn arrow_head_code(&self) -> i32 {
        ARROW_HEADS
            .iter()
            .position(|h| *h == self.style.arrow_head)
            .unwrap_or(0) as i32
    }

    /// §5.7.6 step 2 的另一半「其他箭头样式」。递来的序号在梯子之外就**不动笔**：三档是
    /// [`ARROW_HEADS`] 的数也是界面那三枚钮的数，一枚第四档悄悄变成三角会是一枚说自己改了
    /// 而没改的钮（[`Layer::set_align`] 立的正是这一条）。
    pub fn set_arrow_head(&mut self, code: i32) {
        let Some(&head) = usize::try_from(code).ok().and_then(|i| ARROW_HEADS.get(i)) else {
            return;
        };
        self.style.arrow_head = head;
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| s.arrow_head = head);
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// 手里那支笔是不是圆角矩形：§5.7.3 那一行的门，同 [`Layer::arrow_tool`] 的理由。
    pub fn rounded_tool(&self) -> bool {
        self.tool == Some(Kind::RoundedRect)
    }

    /// §5.7.3 step 2 的那一个数，读回来而不是界面自己记：§5.7.1 的每工具记忆可能交回一枚
    /// 这一排从没按过的半径。
    pub fn corner_radius(&self) -> i32 {
        self.style.corner_radius as i32
    }

    /// §5.7.3 step 2「配置圆角半径」。夹在两端之内由 [`Layer::step_corner_radius`] 那把阶梯
    /// 交给它，落进文档的那一条命令与别的样式钮同一条腿（选中着改是 §5.7.16 step 3 的
    /// 「改样式」，一次 Ctrl+Z 拿回整组）。
    pub fn set_corner_radius(&mut self, radius: u32) {
        let radius = radius.clamp(CORNER_MIN, CORNER_MAX);
        self.style.corner_radius = radius;
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| {
            s.corner_radius = radius;
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// 圆角的那把阶梯：QML 递方向，格宽与两个端点是这里的数（[`Layer::step_alpha`] 同一分工）。
    pub fn step_corner_radius(&mut self, notches: i32) {
        let next = self.corner_radius() + notches.saturating_mul(CORNER_STEP);
        self.set_corner_radius(next.max(CORNER_MIN as i32) as u32);
    }

    /// 手里那支笔是不是编号：§5.7.12 那一排的门，同 [`Layer::arrow_tool`] 的理由。
    pub fn number_tool(&self) -> bool {
        self.tool == Some(Kind::Number)
    }

    /// 下一枚号是几（§5.7.12 step 2 读回来的就是这一格）。它住在 [`Document`] 的计数器上，
    /// 这里不另存一份：删除与撤销都不把计数器往回拖，跟着文档走才对得上屏幕上那些号。
    pub fn counter_start(&self) -> i32 {
        self.doc.next_counter as i32
    }

    /// §5.7.12 step 2「设置起始编号」。这一改**不进撤销栈**：它说的是下一枚号从几起，不是
    /// 屏幕上已有的一枚 —— 记成一条编辑，Ctrl+Z 就退回到一个画面上一模一样的时刻
    /// （§5.7.19 那条"应覆盖绘制、移动、删除、样式修改"列的都是看得见的改动）。
    pub fn set_counter_start(&mut self, value: u32) {
        self.doc.set_counter_start(value.clamp(COUNTER_MIN, COUNTER_MAX));
        self.refresh_preview();
    }

    /// 起始编号的那把阶梯：QML 只递方向（[`Layer::step_corner_radius`] 同一分工）。
    pub fn step_counter_start(&mut self, notches: i32) {
        let next = self.counter_start() + notches.saturating_mul(COUNTER_STEP);
        self.set_counter_start(next.max(COUNTER_MIN as i32) as u32);
    }

    /// §5.7.12 step 5 那一枚「是否自动重排」：删掉中间一号之后，后面的号要不要自己往前补上
    /// 那个空档。界面上亮不亮读的就是这一格，不是界面自己另记一枚 bool（[`Layer::marquee_mode`]
    /// 同一口径）—— 开关只有一个作者。
    pub fn auto_renumber(&self) -> bool {
        self.auto_renumber
    }

    /// 开关**不进撤销栈**：它定的是下一次删除该怎么记，不是屏幕上已有的一枚 ——
    /// [`Layer::set_counter_start`] 头顶那句「记成一条编辑，Ctrl+Z 就退回到一个画面上一模一样
    /// 的时刻」在这里同一字面成立。
    pub fn set_auto_renumber(&mut self, on: bool) {
        self.auto_renumber = on;
    }

    /// §5.7.12 step 3 的一次点击：以那一点摆一只装得下这一号的盒子，放进 `draft`，松手由
    /// [`Layer::release`] 的通用那一段收进文档（于是一枚 Ctrl+Z 退的就是这一枚号）。号是
    /// 此刻从计数器取的，提交时 `Document::insert` 把它抬到这一枚之后 —— core 定的那一条。
    fn draft_number(&mut self, at: PhysPoint) {
        let font = self.style.font_size.max(1);
        let value = self.doc.next_counter;
        let digits = value.to_string().chars().count() as u32;
        let (w, h) = (font * (digits + 1), font * NUMBER_BOX_FONTS);
        let r = PhysRect::new(at.x - (w / 2) as i32, at.y - (h / 2) as i32, w, h);
        let mut e = Element::new(self.next_id, Kind::Number, Geom::Rect(r), self.style.clone());
        e.number = Some(value);
        e.z = self.doc.elements.iter().map(|x| x.z).max().unwrap_or(0) + 1;
        // 上一格预览过的地方要一起重画：这一枚盒子是**被挪动**的，新框并不包住旧框，而
        // [`Layer::paint`] 只重画它拿到的那一片 —— 只报新框就把旧框的那一枚数字留在屏上
        // 成了影。拖框的那几枚工具不会撞上这一条，它们的框是从锚点长出去的。
        let ghost = self.draft.first().map(|e| e.bounds());
        self.draft = vec![e];
        let mut area = self.draft[0].bounds();
        if let Some(g) = ghost {
            area = area.union(&g);
        }
        self.paint(Some(&area));
    }

    /// §5.7.13 step 5's 放大倍数. A copy that is still in hand is regenerated rather
    /// than left at the old scale, keeping the corner the second drag put it at: a knob
    /// the pending object ignores is the tool-that-ignores-the-knob shape [`Layer::set_width`]
    /// was written against. An object that already committed is *not* resized - step 5
    /// sets the pen, and §5.7.17's per-object editing is a different feature with a
    /// different undo command.
    pub fn set_zoom_percent(&mut self, percent: u32) {
        self.style.zoom_percent = percent.clamp(ZOOM_MIN, ZOOM_MAX);
        self.remember_style();
        if let Some(z) = self.zoom {
            let grown = self.place(z.from);
            let next = Placement {
                from: z.from,
                to: PhysRect::new(z.to.x, z.to.y, grown.w, grown.h),
                held: z.held,
            };
            self.zoom = Some(next);
        }
        self.refresh_preview();
    }

    /// §5.7.13 step 5's 边框 and 连接线. Both are read by the rasteriser off the
    /// element's own style, so a pending copy that is not repainted is previewing the
    /// answer to the previous click.
    pub fn set_zoom_border(&mut self, on: bool) {
        self.style.zoom_border = on;
        self.remember_style();
        self.refresh_preview();
    }

    pub fn set_connection_line(&mut self, on: bool) {
        self.style.connection_line = on;
        self.remember_style();
        self.refresh_preview();
    }

    /// Is the tool in hand 文本? The toolbar collapses step 4's row off this one bool
    /// rather than comparing against a tool index it would have to keep in step with
    /// [`TOOLS`] by itself, for the reason [`Layer::zoom_tool`] gives.
    pub fn text_tool(&self) -> bool {
        self.tool == Some(Kind::Text)
    }

    /// §5.7.11 step 4's 对齐 as the number QML carries: `0` 左, `1` 中, `2` 右. The
    /// ladder is Rust's, the same argument [`Layer::tool_code`] makes for the tools.
    pub fn align_code(&self) -> i32 {
        match self.style.align {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
        }
    }

    pub fn text_bg_on(&self) -> bool {
        self.style.text_bg.is_some()
    }

    pub fn text_outline_on(&self) -> bool {
        self.style.text_outline
    }

    /// §5.7.11 step 4's 字号. The one pen field with a life *during* the gesture:
    /// [`Layer::typing_rect`] measures through [`Layer::box_at`] against the live style,
    /// so a box already open takes the new size with it instead of keeping whatever the
    /// placing click happened to start with. Every other knob can wait for the next
    /// object; a field the user is looking at cannot.
    pub fn set_font_size(&mut self, size: u32) {
        self.style.font_size = size.clamp(FONT_MIN, FONT_MAX);
        self.remember_style();
        let cmd = font_size_command(&self.doc, self.sel.ids(), size);
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// §5.7.11 step 4's 对齐. A code off the end of the ladder leaves the pen alone
    /// rather than guessing which cell the caller meant: three is [`Layer::align_code`]'s
    /// number as well as the toolbar's, and a fourth that silently became 左 would be a
    /// knob that lies about what it set.
    pub fn set_align(&mut self, code: i32) {
        let align = match code {
            0 => Align::Left,
            1 => Align::Center,
            2 => Align::Right,
            _ => return,
        };
        self.style.align = align;
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| s.align = align);
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// §5.7.11 step 4's 字体. A family is a name the user types rather than a rung on a
    /// ladder, and the one shape that is not a name is the blank one: clearing the field and
    /// pressing Enter is not a request to draw with no font, so the pen is left alone - the
    /// same answer [`Layer::set_align`] gives a code off the end of the ladder, for the same
    /// reason (a knob that silently became 微软雅黑 would be saying something it did not do).
    /// Trimmed on the way in because that is the form the field's own readout shows.
    pub fn set_font_family(&mut self, family: &str) {
        let name = family.trim();
        if name.is_empty() {
            return;
        }
        self.style.font_family = name.to_string();
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| {
            s.font_family = name.to_string();
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// §5.7.11's first rule: 无背景或填充色. The plate is the pen at a third of its alpha,
    /// the same translucent fill [`Layer::set_filled`] puts inside a box - so the letters
    /// stay the densest thing inside their own background.
    pub fn set_text_bg(&mut self, on: bool) {
        self.style.text_bg = on.then(|| Self::fill_colour(self.style.color));
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| {
            // 每一枚对象自己的笔色吃自己那一份三分之一，与 [`Layer::set_filled`] 同一条式子。
            s.text_bg = on.then(|| Self::fill_colour(s.color));
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// §5.7.11's second rule, one switch. 那句规则要的是「颜色和宽度」，而那两格自本轮起各有自己
    /// 的手（[`Layer::set_outline_colour`] / [`Layer::set_outline_width`]），开关不许替它们改数：
    /// 把描边关掉再打开不是重设一次，这一条与 core 那格 `Style::text_outline` 头顶说的是同一件事。
    pub fn set_text_outline(&mut self, on: bool) {
        self.style.text_outline = on;
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| {
            s.text_outline = on;
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// 屏幕上那一圈边**现在是什么颜色**。交回的是 `Style::outline_for` 的答案，所以「没人配过、
    /// 让位给笔色」与「用户配过」这两臂在工具栏里读起来是同一枚颜色 —— 面板再判一次让位，就是
    /// 给同一圈边造第二个作者，而渲染那一层与面板会各答一枚。
    pub fn outline_colour(&self) -> [u8; 4] {
        self.style.outline_for().0
    }

    /// 那一圈边有多宽，同上一枚：读的是渲染那一层要用的那一个数。
    pub fn outline_width(&self) -> u32 {
        self.style.outline_for().1
    }

    /// §5.7.11 那句规则的「颜色」那一半。收三枚而不是四枚：色块不带 alpha，而那一圈边的 alpha 是
    /// 它自己的 —— 这一条分工与 [`falcon_core::annotation::colour_command`] 对笔色做的那件相同。
    pub fn set_outline_colour(&mut self, rgb: [u8; 3]) {
        self.style.set_outline_colour(rgb);
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| {
            // 每一枚对象自己那一格的 alpha 由 core 那一位保着，这里递的是色块上那三枚。
            s.set_outline_colour(rgb);
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// 「宽度」那一半。阶梯住在 core（`Style::set_outline_width` 走笔那把 `WIDTH_MIN..=WIDTH_MAX`
    /// ——一圈边就是一条线），这里只递数，不再夹一遍。
    pub fn set_outline_width(&mut self, width: u32) {
        self.style.set_outline_width(width);
        self.remember_style();
        let cmd = patch_style_command(&self.doc, self.sel.ids(), move |_e, s| {
            s.set_outline_width(width);
        });
        self.apply_edit(cmd);
        self.refresh_preview();
    }

    /// Is the tool in hand 橡皮? The row that offers §5.7.14's mode collapses off this
    /// one bool, for the reason [`Layer::zoom_tool`] gives: the index of 橡皮 in
    /// [`TOOLS`] is Rust's number, and a second copy of it in QML is a second ladder.
    pub fn eraser_tool(&self) -> bool {
        self.tool == Some(Kind::Eraser)
    }

    /// §5.7.14's 擦除到透明, read back rather than kept in the toolbar for the same
    /// reason [`Layer::dashed`] is: §5.7.1's per-tool memory can hand the eraser back a
    /// mode no button has ever touched.
    pub fn erase_transparent(&self) -> bool {
        self.style.erase_to_transparent
    }

    /// §5.7.14's second rule. One bool of the pen, but what it moves is the *exported*
    /// picture: with it off the eraser's plane carries the frozen screenshot back over
    /// the ink, so the annotation goes and the pixels under it stay; with it on the
    /// pixels leave the picture altogether.
    ///
    /// The live preview cannot show that difference, and the toolbar's 提示 is not
    /// decoration because of it: QML draws the same frozen frame *under* the ink layer,
    /// so a cleared pixel and a restored pixel both read as the desktop on screen. What
    /// leaves the mask is the frame with the hole in it, and the encoder's own answer
    /// covers both halves - a PNG keeps it, an alpha-less format fills it with white
    /// ([`falcon_core::encode`]'s `flatten_on_lossy`, on by default).
    pub fn set_erase_transparent(&mut self, on: bool) {
        self.style.erase_to_transparent = on;
        self.remember_style();
        self.refresh_preview();
    }

    /// Catch a pending preview up with the pen. Only the two node-style gestures have a
    /// life of their own between style calls: a stroke mid-drag cannot meet a toolbar
    /// click, because the pointer that draws it is the pointer the toolbar would need.
    fn refresh_preview(&mut self) {
        match self.draft.first().map(|e| e.kind) {
            Some(Kind::Zoom) => self.draft_zoom(),
            Some(Kind::Polyline) => self.draft_polyline(),
            _ => {}
        }
    }

    /// §5.7.16 step 3 的「改样式」与 §5.7.17 step 4 的「样式调整」是同一条腿：面板上那枚旋钮
    /// 动一下，除了改笔，还要落到**此刻选中的那些对象**上，一次 Ctrl+Z 拿回整组。
    ///
    /// 每一处调用交来的都是 core 那一位作者算好的 `Command`（`colour_command` / `alpha_command`
    /// / `font_size_command` / `intensity_command` / `patch_style_command`），所以本函数既不判
    /// 空也不判等：没选中东西、锁着、或改完与改前一模一样，core 一律交回 `None`，这里就一个字
    /// 也不做 —— 笔照改，因为那是下一笔的形状，不是一次可撤销的编辑（§5.7.19）。
    fn apply_edit(&mut self, cmd: Option<Command>) -> bool {
        match cmd {
            Some(cmd) => {
                self.apply(cmd);
                true
            }
            None => false,
        }
    }

    /// 把那一串里已经不在场的 id 摘掉。撤销、重做与全清都会让对象消失，而留下的读数是一枚
    /// 亮着的钮：`selected_count` 报「2 枚」，面板就占着高度、删除与复制都显得能按，而
    /// core 那几位命令作者对陌生 id 的答案是**整条不建** —— 按下去什么也不发生。摘掉之后
    /// 那一行自己收起来，钮与它做不了的事一起不在屏幕上。
    fn prune_selection(&mut self) {
        let kept: Vec<u64> = self
            .sel
            .ids()
            .iter()
            .copied()
            .filter(|id| self.doc.get(*id).is_some())
            .collect();
        self.sel = Selection::of(&kept);
    }

    /// Is the preview already exactly what the state asks for - geometry *and* pen?
    ///
    /// A draft snapshots the style it was built with, so a guard on the geometry alone
    /// lets a switch flipped while a gesture is in hand leave the old pen on the screen.
    /// The user then sees one thing and commits another, which is the tool-that-ignores-
    /// the-knob shape [`Layer::set_width`] was written against, in the other direction:
    /// the knob was obeyed, by the object that has not been drawn yet.
    fn draft_matches(&self, wanted: Option<&Geom>) -> bool {
        match (self.draft.first(), wanted) {
            (Some(e), Some(g)) => &e.geom == g && e.style == self.style,
            (None, None) => true,
            _ => false,
        }
    }

    fn remember_style(&mut self) {
        let Some(k) = self.tool else { return };
        self.store(k, self.style.clone());
    }

    /// The pen this tool was last left with, if the user ever taught it one.
    fn remembered(&self, kind: Kind) -> Option<Style> {
        self.styles
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, s)| s.clone())
    }

    fn store(&mut self, kind: Kind, style: Style) {
        match self.styles.iter_mut().find(|(k, _)| *k == kind) {
            Some(slot) => slot.1 = style,
            None => self.styles.push((kind, style)),
        }
    }

    /// §5.7.20's one粗细 knob: the brush follows the pen wherever the width is set, so
    /// that a remembered width and a typed one cannot leave a tool with a 2 px pen and
    /// a 16 px brush.
    fn link_brush(style: &mut Style) {
        style.brush.size = style.width.saturating_mul(4).clamp(3, 96);
    }

    /// §5.20's `[annotation]` scalars, into the pen a tool starts with. This is the key
    /// the file has always been able to carry and nothing has ever read: until this call
    /// existed, the only `[annotation]` field any product module touched was `tool_style`,
    /// so a hand-edited `stroke_color`/`stroke_width`/`font_size`/`font_family` parsed,
    /// validated, survived a round trip through the file and changed nothing on screen.
    ///
    /// It moves the *base* pen only, and [`Layer::apply_tool_styles`] runs after it,
    /// which is the order §5.7.1's fallback needs: a tool the user has taught keeps its
    /// own colour, and a tool the table says nothing about inherits this one. Both halves
    /// are the same sentence - the file's `[annotation]` row is the default *per tool*, not
    /// a global that overwrites what was learned.
    ///
    /// Unlike every other setter here this one does not call [`Layer::remember_style`]:
    /// reading a file is not the user teaching a tool, and a config that seeded the pen
    /// would otherwise write its own defaults straight back into `tool_style`, inventing
    /// an entry for whichever tool happened to be selected at the first save.
    ///
    /// There is no refusal path here for the same reason there is none in [`Layer`]'s
    /// callers: `crate::config` validates on load with this very parser
    /// ([`falcon_core::colors::parse`]) and rewrites what it cannot read, so a colour that
    /// reaches this function is a colour it can read. The two `clamps` here are the pen's
    /// own floors, not repairs.
    pub fn apply_annotation_defaults(&mut self, ann: &Annotation) {
        if let Some(rgba) = falcon_core::colors::parse(&ann.stroke_color) {
            self.style.color = rgba;
        }
        self.style.width = ann.stroke_width.clamp(1, 64);
        Self::link_brush(&mut self.style);
        self.style.font_size = ann.font_size.clamp(FONT_MIN, FONT_MAX);
        // The number 模糊 opens with when it has no memory. The key has been in the file
        // as long as the file has been and has had no reader at all; it is 马赛克's block
        // size *or* 模糊's radius depending on the tool, which is why it can only be one
        // tool's default and why the per-tool `intensity` row beats it whenever both are
        // present.
        self.default_blur = ann.blur_radius.clamp(EFFECT_MIN, EFFECT_MAX);
        // An empty family is the file saying nothing, and `default_tool` already uses
        // that spelling. Writing it into the pen would hand DirectWrite an empty
        // family name, which is a different question from an unknown one.
        if !ann.font_family.trim().is_empty() {
            self.style.font_family = ann.font_family.clone();
        }
    }

    /// The restart half of §5.7.1's memory (PRD §9.2 样式记忆重启后仍然有效): the
    /// `[annotation.tool_style]` table goes back into the layer. The layer parses no
    /// file - this is the seam the config hands its readings over.
    ///
    /// A tool the table says nothing about is left alone, and a field it leaves out is
    /// left alone rather than zeroed: `width = 5` with no `color` is a user who changed
    /// the rect's line width, not one who also reset its colour.
    ///
    /// [`crate::settings`] is the caller, and it is the only part that touches a file.
    pub fn apply_tool_styles(&mut self, remembered: &BTreeMap<String, ToolStyle>) {
        for (key, saved) in remembered {
            let Some(kind) = Kind::from_key(key) else {
                self.problems.push(format!(
                    "tool style for {key:?}: no such tool, the config has to be edited by hand"
                ));
                continue;
            };
            let mut style = self.remembered(kind).unwrap_or_else(|| self.style.clone());
            if let Some(text) = &saved.color {
                match falcon_core::colors::parse(text) {
                    Some(rgba) => style.color = rgba,
                    None => self
                        .problems
                        .push(format!("tool style for {key:?}: {text} is not a colour")),
                }
            }
            if let Some(width) = saved.width {
                style.width = width.clamp(1, 64);
                Self::link_brush(&mut style);
            }
            if let Some(intensity) = saved.intensity {
                // Written *as this row's tool's effect*, so a radius read out of the blur
                // row cannot land on a mosaic's block size and vice versa (§5.7.1).
                style.set_intensity(kind, intensity);
            }
            self.store(kind, style);
        }
        // The selected tool shows its pen in the toolbar, so an apply that moved it has
        // to move the readback with it - the same rule `select_tool` follows.
        if let Some(k) = self.tool {
            if let Some(s) = self.remembered(k) {
                self.style = s;
            }
        }
    }

    /// Everything the layer has learned, in the spelling the config stores it under.
    /// Colours leave as `#RRGGBBAA`, because the alpha is part of the pen and a
    /// six-digit hex cannot carry it.
    pub fn tool_styles(&self) -> BTreeMap<String, ToolStyle> {
        self.styles
            .iter()
            .map(|(kind, style)| {
                (
                    kind.key().to_string(),
                    ToolStyle {
                        color: Some(format_color(style.color, ColorFormat::HexRgba)),
                        width: Some(style.width),
                        // Only the two tools that own a strength have one to write. Putting
                        // the number every pen happens to carry into 矩形's row would
                        // describe a setting that row has no reader for, and the file's own
                        // validator reports it as forgotten on the next load.
                        intensity: kind.has_effect().then(|| style.intensity()),
                    },
                )
            })
            .collect()
    }

    /// The selection in document space.
    ///
    /// `hole` is stored in physical desktop pixels because that is the space the
    /// crop is measured in, while everything here asks about it as a document
    /// question. On a desktop whose leftmost screen starts at a negative x the two
    /// are not the same numbers, and a clip left in desktop pixels would hide the
    /// ink on one screen and draw it on the other.
    fn hole_doc(&self) -> PhysRect {
        self.hole.offset(-self.origin.x, -self.origin.y)
    }

    /// A desktop point in document space, clamped into the selection: §5.7.1 step 4
    /// 是"在选区内绘制", and ink outside the box is cropped at export anyway, so a
    /// stroke that wanders off comes back as one that stops at the edge.
    fn doc_point(&self, device: PhysPoint) -> PhysPoint {
        let p = PhysPoint::new(device.x - self.origin.x, device.y - self.origin.y);
        let hole = self.hole_doc();
        if hole.is_empty() {
            return p;
        }
        PhysPoint::new(
            p.x.clamp(hole.x, hole.right() - 1),
            p.y.clamp(hole.y, hole.bottom() - 1),
        )
    }

    /// §5.7.16 step 2 与 §5.7.17 steps 2-4 在箭头工具下的那一次按下。三臂的**次序就是判据**：
    ///
    /// ① 已选中那些对象的控制点（[`grip_at_any`]）先问 —— 一枚把手与它自己那条边线压在同一个
    ///    像素上，后问就永远握不到把手。多选没有共享把手：这一枚抓的是**那一枚对象自己的**
    ///    （§5.7.17 第四步给一组的只有移动、复制、删除与样式）。
    /// ② [`Document::topmost_at`] 那一枚对象：点到谁就选中谁，而**已经在这串里**的那一枚不换 ——
    ///    拖一串里的一员是挪动整组，不是把组缩成一枚。锁着的在这一步之外，那是
    ///    [`Document::topmost_at`] 自己的口径（「a locked object ignores clicks」）。
    ///    `ctrl` 为真时这一臂拖的是**副本**（§5.7.18 步 2「按住 Ctrl 拖动」），见
    ///    [`Layer::clone_press`]。
    /// ③ 空白：§5.16 第四步「点击空白区域完成编辑」就是这一格 —— 清掉选中，然后**把这枚按下交回
    ///    选区那八枚把手**（返回 `false`）。屏幕上此时没有选中的对象，指针该管的还是那块框。
    ///
    /// 返回值同 [`Layer::press`] 头上那句：`true` = 画图的那只手没有吃掉这枚按下，而是对象编辑
    /// 吃了它，调用者不许再把它读成别的事。
    fn select_press(&mut self, device: PhysPoint, ctrl: bool) -> bool {
        if self.hole.is_empty() {
            return false;
        }
        let p = self.doc_point(device);
        if self.typing_editing() {
            // §5.7.16 步 4「点击空白区域完成编辑」在改字这一臂上的那一次按下：字先交回文档
            // （没改动就什么也不交，见 [`Layer::commit_typing`]），然后**这一枚按下照常往下
            // 走** —— 把手、单枚选中与空白那三臂都还在原地等它，而"改完就走"不该以吃掉这一次
            // 点击为代价。落在框里的那一枚到不了这里：页上那一枚 `TextEdit` 是指针底下的
            // item，它自己把点击拿走挪光标（与 §5.7.11 步 5 在 [`Layer::press`] 同一判）。
            self.commit_typing();
        }
        if self.marquee {
            // §5.7.17 step 1 的那一档：一滚到底只问框选，把手与单枚选中都让开 —— 同一枚按下
            // 既是"抓住这枚把手"又是"框住这一片"的话，两件事会同时发生而谁也不报错。
            let drag = SelDrag::take(SelMode::Marquee, p, &self.doc, &[]);
            self.sel_drag = Some(drag);
            return true;
        }
        if let Some((id, grip)) = grip_at_any(&self.doc, self.sel.ids(), p, GRIP_SLOP) {
            // 圆角那一枚把手要的是**握住那一刻**的那一枚数（[`SelMode::Grip`] 头上那句），
            // 其余把手读到的这一格用不上，而读一次比让每一枚都带一份 style 快照便宜。
            //
            // `ctrl` 在这一臂不做的事：握住把手改的是那一枚的尺寸，把「按住 Ctrl 拖动」读成
            // 「拖把手时顺便复制一枚」会让 §5.7.3 的改尺寸一次交出两件事，而那一拖只有一句话。
            let corner = self.doc.get(id).map_or(CORNER_MIN, |e| e.style.corner_radius);
            let drag = SelDrag::take(SelMode::Grip { id, grip, corner }, p, &self.doc, &[id]);
            self.sel_drag = Some(drag);
            return true;
        }
        if let Some(id) = self.doc.topmost_at(p) {
            if !self.sel.contains(id) {
                self.sel = Selection::one(id);
            }
            // `ctrl` 为真 ⇒ 这一趟拖的是副本（§5.7.18 步 2「按住 Ctrl 拖动」）。core 那一位
            // 作者建不出副本时**照旧走移动**：那一拖本来就要挪东西，只是没多出一枚，而"克隆
            // 没成"与"没按 Ctrl"在屏幕上该是同一张脸。
            let cloned = if ctrl { self.clone_press(p) } else { None };
            let drag = cloned.unwrap_or_else(|| {
                SelDrag::take(SelMode::Move, p, &self.doc, self.sel.ids())
            });
            self.sel_drag = Some(drag);
            return true;
        }
        self.sel.clear();
        self.drop_sel_drag();
        false
    }

    /// §5.7.18 步 2「按住 Ctrl 拖动」的起手：把这一串**复制**进文档，摆在与原处同一格
    /// （`dx`/`dy` 都取 0 —— 副本跟着拖多远走多远，而按下那一点此刻正压着副本，起手的手感与
    /// 移动一致），副本的 id 由这一趟手势自己带着（[`SelDrag::from`]）。
    ///
    /// 两件刻意不做的事。其一：**不换** [`Layer::sel`]，整个手势里它说的还是原物 —— 副本此刻
    /// 是预览，与橡皮筋同一格（§5.7.17「框了一下」不进撤销栈、也就不该改面板在描述谁），于是
    /// 半路丢掉、拖回原处这些没有交出去的路径不会留下一串已经不在场的 id。真正接手副本的是
    /// [`Layer::finish_clone_drag`]，落栈的那一次才把选中换成它们（与 [`Layer::clone_selected`]
    /// 「副本接着成为这一串」同一口径）。其二：副本进文档而**不进栈** —— 栈上那一枚要等松手
    /// 才知道拖到了哪儿，而一次 Ctrl+Z 该收回的是整趟手势。
    ///
    /// 新 id 与 z 由 [`falcon_core::annotation::model::Document::clone_elements`] 定，桥里不猜
    /// 它们叫什么；与工具栏那一枚克隆钮的差别只在副本落在原地偏一格、还是落在拖到的那一格。
    fn clone_press(&mut self, start: PhysPoint) -> Option<SelDrag> {
        let copies = match self.sel.clone_by(&self.doc, 0, 0) {
            Some(Command::Add(list)) => list,
            _ => return None,
        };
        let ids: Vec<u64> = copies.iter().map(|e| e.id).collect();
        Command::Add(copies).apply(&mut self.doc);
        Some(SelDrag::take(SelMode::Clone, start, &self.doc, &ids))
    }

    /// 收回手里那趟对象编辑。克隆那一臂另有一件事要收：预览的副本已经进了文档、却还没落过栈，
    /// 半路丢掉时必须把它们摘回去 —— 否则屏幕上多出一枚没有来历的对象，撤销收不走，而用户从没
    /// 要求过它（§9.1 的⑦「一次没做完的手势不该多出一枚对象」在这一臂上的读法）。
    ///
    /// 四个入口都走这一手，而不是各自写 `sel_drag = None`：换工具、开框选、选区被清空与点到
    /// 空白。「收回这一趟」是一件事，写四遍就会有两遍漏掉副本。
    fn drop_sel_drag(&mut self) {
        let Some(drag) = self.sel_drag.take() else {
            return;
        };
        if drag.mode != SelMode::Clone {
            return;
        }
        let area = self.sel_area(&drag.from);
        self.take_clone_preview(&drag);
        if let Some(area) = area {
            self.paint(Some(&area));
        }
    }

    /// 把预览态的副本从文档上摘下，并按**此刻**的几何交回它们（拖到哪儿就是哪儿）。
    /// `Document::take` 是 `pub(crate)`，所以桥里摘对象只经由 core 那一位作者 ——
    /// [`Command::Remove`] 的 `apply` —— 不留第二套摘法。
    fn take_clone_preview(&mut self, drag: &SelDrag) -> Vec<Element> {
        let ids: Vec<u64> = drag.from.iter().map(|(id, _, _)| *id).collect();
        let copies: Vec<Element> = ids
            .iter()
            .filter_map(|id| self.doc.get(*id).cloned())
            .collect();
        Command::Remove(copies.clone()).apply(&mut self.doc);
        copies
    }

    /// §5.7.18 那一趟拖拽的收尾：**一条**命令落在栈上。
    ///
    /// 文档里此刻是预览态的副本，而它们从没落过栈，所以先按此刻的几何收一份、从文档上摘下，
    /// 再把同一份几何落成一条 [`Command::Add`]：栈上这一条的 "before" 是「没有副本」，于是
    /// 一次 Ctrl+Z 收回的是整批副本，而不是把它们摞回原处、再留下一枚没人认领的添加。
    ///
    /// 中间一次也没拖（`to` 还是 `None`）⇒ 摘下之后什么都不落，与 [`Layer::select_release`]
    /// 头上那句「按下去又松开、中间一次也没拖就不进撤销栈」同一读法；副本跟着预览一起走，
    /// 所以一次「按着 Ctrl 单击」不在屏幕上留下第二枚对象，而选中还是原物（步 3 没发生，
    /// 步 4 也就没有「这一串」可换）。
    fn finish_clone_drag(&mut self, drag: &SelDrag) -> bool {
        let area = self.sel_area(&drag.from);
        let copies = self.take_clone_preview(drag);
        if drag.to.is_none() {
            if let Some(area) = area {
                self.paint(Some(&area));
            }
            return false;
        }
        // 交出去了，副本才成为这一串 —— 步 4「用户移动副本到目标位置」的下一手还要接着挪
        // 它们，而 [`Layer::clone_selected`] 立的就是这一条（工具栏那一枚克隆钮亮着的也是它）。
        self.sel = Selection::of(&copies.iter().map(|e| e.id).collect::<Vec<u64>>());
        self.apply(Command::Add(copies));
        true
    }

    /// 把这一枚指针位置换算成一条命令，**不**自己落任何东西。三臂各问 core 里唯一的那一位作者：
    /// 整组挪动问 [`Selection::move_by`]、一枚的把手问 [`resize_grip_command`]、而
    /// [`Grip::Rotate`] 那一枚问 [`rotate_command`]、[`Grip::Radius`] 那一枚问
    /// [`radius_grip_command`] —— `resize_grip` 对后两枚都是 `None`，因为转动改的是 `transform`、
    /// 圆角改的是 `style`，都不是几何；把其中任何一枚算成一次改尺寸，会得到一个既不转也不圆、
    /// 也许根本不大的框。
    ///
    /// 问的是**已经按回 `from` 的那份文档**：dx/dy 从按下量起而不是从上一枚事件累加，所以一趟
    /// 被打断又接上的拖拽不会漂，而栈上那一条的 "before" 与 `from` 是同一份东西。圆角那一枚
    /// 另带 `corner`，因为几何按回去了而 `style` 没有 —— 预览写的正是那一格。
    fn sel_command(&self, drag: &SelDrag, p: PhysPoint) -> Option<Command> {
        let (dx, dy) = (p.x - drag.start.x, p.y - drag.start.y);
        match drag.mode {
            // 框选那一趟算不出命令，也不该算出：它改的是「哪些对象被选中」，不是文档，而撤销栈
            // 上出现"我框了一下"就是把选区当成了编辑。生产路径上到不了这一臂（[`Layer::select_drag`]
            // 先走它自己的橡皮筋分支），留在这里是因为"框选也是一次拖拽"这句话该由类型说。
            SelMode::Marquee => None,
            SelMode::Move => self.sel.move_by(&self.doc, dx, dy),
            // 副本那一臂问的还是同一位作者（[`Selection::move_by`]），只是这一串的 id 由这一趟
            // 手势自己带着：`self.sel` 在整个手势里说的是**原物**，副本还是预览（见
            // [`Layer::clone_press`] 头上那句"不换选中"）。
            SelMode::Clone => {
                let ids: Vec<u64> = drag.from.iter().map(|(id, _, _)| *id).collect();
                Selection::of(&ids).move_by(&self.doc, dx, dy)
            }
            SelMode::Grip { id, grip, corner } => match grip {
                Grip::Rotate => {
                    let bounds = self.doc.get(id)?.geom.bounds();
                    rotate_command(&self.doc, &[id], rotation_at(&bounds, p))
                }
                Grip::Radius => radius_grip_command(&self.doc, id, corner, dx, dy),
                _ => resize_grip_command(&self.doc, id, grip, dx, dy),
            },
        }
    }

    /// 文档按回按下那一刻的样子。预览每一枚事件先走这一步、命令落账最后也走这一步，
    /// 所以「此刻屏幕上是什么」与「撤销栈上那一条的 before 是什么」始终是同一份东西。
    fn sel_restore(&mut self, from: &[(u64, Geom, Transform)]) {
        for (id, geom, transform) in from {
            if let Some(e) = self.doc.get_mut(*id) {
                e.geom = geom.clone();
                e.transform = *transform;
            }
        }
    }

    /// [`SelDrag::from`] 按回的是几何与转向，而圆角那一枚把手预览写的是 `style` —— 于是那一枚
    /// 数在这里单独按回**握住把手那一刻**的那一份（[`SelMode::Grip`] 头上那句，与
    /// [`falcon_core::annotation::radius_grip_command`] 要的 `start_radius` 是同一枚数）。
    /// 不按回会得到什么样的错：第二枚指针事件算出的那条命令，"before" 会是第一枚事件留下的那
    /// 一枚半径，一次 Ctrl+Z 退回的是一拖拖到一半的那个角。别的把手不写 `style`，对它们这一句
    /// 原样返回。
    fn sel_restore_corner(&mut self, mode: SelMode) {
        let SelMode::Grip {
            id,
            grip: Grip::Radius,
            corner,
        } = mode
        else {
            return;
        };
        if let Some(e) = self.doc.get_mut(id) {
            e.style.corner_radius = corner;
        }
    }

    /// 这一趟手势碰过的区域：按下那一刻那些框的足迹，并上此刻那些框的足迹。预览只重画这一块
    /// —— 与 [`Layer::paint`] 头上那条「only what moved」同一口径，一趟拖拽不该把整层重渲染一遍。
    fn sel_area(&self, from: &[(u64, Geom, Transform)]) -> Option<PhysRect> {
        let mut acc: Option<PhysRect> = None;
        {
            let mut grow = |r: PhysRect| {
                acc = Some(match acc {
                    Some(a) => a.union(&r),
                    None => r,
                });
            };
            for (id, geom, transform) in from {
                grow(transform.footprint(&geom.bounds()));
                if let Some(e) = self.doc.get(*id) {
                    grow(e.bounds());
                }
            }
        }
        acc
    }

    /// 拖拽中的对象编辑：把文档按回按下那一刻，再落上这一枚指针位置算出来的那条命令，**不落栈**。
    /// 一趟拖拽是一次编辑而不是几十次，所以栈上那一枚只在 [`Layer::select_release`] 落。
    ///
    /// 算不出命令（转回了它自己的角度、把手拉到 2×2 的地板之内）也要重画：上面那一步已经把文档
    /// 按回去了，而屏幕上还留着上一枚事件的预览，不画就等于把一件已经不做了的事留在画布上。
    fn select_drag(&mut self, device: PhysPoint) {
        let Some(drag) = self.sel_drag.clone() else {
            return;
        };
        if drag.mode == SelMode::Marquee {
            self.marquee_band(drag.start, self.doc_point(device));
            return;
        }
        self.sel_restore(&drag.from);
        self.sel_restore_corner(drag.mode);
        let mut drag = drag;
        drag.to = self.sel_command(&drag, self.doc_point(device));
        if let Some(cmd) = drag.to.as_ref() {
            cmd.apply(&mut self.doc);
        }
        let area = self.sel_area(&drag.from);
        self.sel_drag = Some(drag);
        if let Some(area) = area {
            self.paint(Some(&area));
        }
    }

    /// 松手：把文档按回按下那一刻，再把最后那一枚指针位置的那条命令**落进撤销栈**（§5.7.19 的
    /// 「移动/改尺寸/旋转都是一条可撤销的编辑」）。按下去又松开、中间一次也没拖 ⇒ `from` 与
    /// 此刻一模一样，[`sel_command`] 交回 `None`，栈上因此什么都不落 —— 与「一枚没画出东西的
    /// 点击不进撤销栈」是同一句规矩（§5.7.19）。
    ///
    /// 克隆那一臂是这一句的例外里的例外：它落栈的不是这条移动，而是把拖到的那几枚副本**添加**
    /// 进去（[`Layer::finish_clone_drag`]），因为「拖出一堆副本」的 before 是「没有副本」。
    fn select_release(&mut self) -> bool {
        let Some(drag) = self.sel_drag.take() else {
            return false;
        };
        if drag.mode == SelMode::Marquee {
            return self.finish_marquee();
        }
        if drag.mode == SelMode::Clone {
            return self.finish_clone_drag(&drag);
        }
        self.sel_restore(&drag.from);
        let Some(cmd) = drag.to else {
            return false;
        };
        let area = self.sel_area(&drag.from);
        self.apply(cmd);
        if let Some(area) = area {
            // `apply` 已经按命令自己的 dirty 重画过一次；这里补的是**预览碰过而命令没碰**的那一块
            // （把手拉到地板之内被 clamp 回原样那一族），不补就会留下一块没人负责的预览像素。
            self.paint(Some(&area));
        }
        true
    }

    /// 橡皮筋：画在 `draft` 上 —— 与手里那笔同一块地方，因为「框了一下」不是编辑，撤销栈上
    /// 不该有它。不灌满：框住的那片图正是这一手要看见的东西。
    fn marquee_band(&mut self, from: PhysPoint, to: PhysPoint) {
        let rect = PhysRect::from_points(from, to);
        let mut style = self.style.clone();
        style.fill = None;
        let band = Element::new(self.next_id, Kind::Rect, Geom::Rect(rect), style);
        let area = match self.draft.first() {
            Some(old) => old.bounds().union(&band.bounds()),
            None => band.bounds(),
        };
        self.draft = vec![band];
        self.paint(Some(&area));
    }

    /// 松手：把橡皮筋换成选中。框里的每一枚都进这串，**包括锁着的** —— 被选中不等于可编辑，
    /// 那是 [`falcon_core::annotation::model::Document::can_edit`] 的活儿，而框选这一步本来
    /// 就没改动任何东西。
    ///
    /// 两个矩形不是一件事，所以各拿各的：进那一串的是**拖出来的那一条几何**（框住谁由这条边
    /// 决定，而不是由这条边的笔刷外沿决定），而重画的是它**连着笔锋的那一圈** —— 橡皮筋是
    /// 一支 3 px 的笔画出来的，只回几何那圈就把最外两像素留在层上，而下一次重画只覆盖它
    /// 自己那块脏区，谁也不会来收这笔迹。
    fn finish_marquee(&mut self) -> bool {
        let Some(band) = self.draft.first().cloned() else {
            // 按下去又松开，中间一次也没拖：谁也没框住，这串保持原样。
            return true;
        };
        self.draft.clear();
        self.sel = Selection::marquee(&self.doc, &band.geom.bounds());
        self.paint(Some(&band.bounds()));
        true
    }

    /// 框选那一档开着没有：工具栏那枚钮亮不亮，问的是这一句而不是界面自己记的一枚 bool。
    pub fn marquee_mode(&self) -> bool {
        self.marquee
    }

    /// §5.7.17 step 1 的开关。顺手收回那趟手势： mode 换了还让它继续，会让"挪这串"的松手
    /// 落到框选的收尾上去（或反过来），而这两件事在屏幕上差一整块选区。
    pub fn set_marquee(&mut self, on: bool) {
        self.marquee = on;
        self.drop_sel_drag();
    }

    /// 这一串里有几枚。界面上那一行按它决定自己占不占高度 —— 「什么算选中了东西」与"数得出
    /// 几枚"都是 Rust 的数，抄进 `.qml` 就是第二份（同 [`Layer::tool_code`] 的理由）。
    pub fn selected_count(&self) -> usize {
        self.sel.len()
    }

    /// §5.7.15 step 3「系统实时显示旋转结果」的那一枚读数：这串里**最上面**那一枚的角度，
    /// 与 [`Selection::topmost`] 给面板报样式同一口径。没选中东西 ⇒ 0。
    pub fn rotation_degrees(&self) -> i32 {
        self.sel
            .topmost(&self.doc)
            .map_or(0, |e| e.transform.rotation.round() as i32)
    }

    /// §5.7.3 step 4 的那一枚圆角把手，在**文档坐标**里的位置；`None` 是「此刻屏幕上没有这一枚
    /// 把手可画」。三位作者各管一段，谁也不写第二遍：有没有这一枚问
    /// [`Grip::grips_for`]（与 [`grip_at_any`] 抢不到把手时判的是同一张名单，画在页上的点因此
    /// 不会是握不到的那一个），住在哪儿问 [`grip_point`]（含那一枚对象自己的转向），而
    /// 文档坐标 → 本窗 DIP 那一步在 [`crate::mask`] 里与文本框同一条腿。
    ///
    /// 只在串里**恰好一枚**对象时才递：两枚 圆角矩形 都在场时页上只能画一个点，那一个点说的是
    /// 「这一只框的角」而屏幕上没有第二条路能讲清是哪一只（#453 两种失败保持分开 —— 这里选的
    /// 是「不讲」而不是「讲错」）。
    pub fn radius_handle(&self) -> Option<PhysPoint> {
        let [id] = self.sel.ids() else {
            return None;
        };
        let e = self.doc.get(*id)?;
        if !Grip::grips_for(e).contains(&Grip::Radius) {
            return None;
        }
        grip_point(e, Grip::Radius)
    }

    /// §5.7.15 step 2 的后一半「或输入旋转角度」：QML 只递方向，格的宽窄是这里的
    /// [`ROTATE_STEP_DEG`]，与 [`Layer::step_alpha`] 同一分工。起点取面板所报那一枚的当前
    /// 读数，落点是一个**绝对**角：[`rotate_command`] 把整组转到同一角度上、每一枚绕它自己的
    /// 中心，所以一组没排齐的框不会转成一把扇子。不转的对象（直线、折线、箭头）由那一位
    /// 作者按 `Kind::rotates` 筛掉，因此一组里全是它们时这一句返回 `false`。
    pub fn step_rotation(&mut self, notches: i32) -> bool {
        let Some(base) = self
            .sel
            .topmost(&self.doc)
            .map(|e| e.transform.rotation)
        else {
            return false;
        };
        let degrees = clamp_rotation(base + f64::from(notches) * ROTATE_STEP_DEG);
        let cmd = self.sel.rotate(&self.doc, degrees);
        self.apply_edit(cmd)
    }

    /// §5.7.17 step 4 的「删除」：整组一条命令，一次 Ctrl+Z 把它们原样拿回（`Command::Remove`
    /// 存的是整份快照，z、编号与样式都在里面）。删完必须清空这串：留下的是一串已经不在场的
    /// id，而 core 那几位 builder 对陌生 id 的答案是**整条不建**，于是下一次样式改动会悄悄
    /// 不落地。
    ///
    /// 「自动重排」亮着时换的是**建命令的那一位**，不是这一趟的收尾：`delete_renumbering`
    /// 把腾出来的空档补上，两件事记成一条 [`Command::Compound`]，所以判据仍是「一次 Ctrl+Z」
    /// —— 退回复的是那一次编辑，而不是它的两半。
    pub fn delete_selected(&mut self) -> bool {
        let cmd = if self.auto_renumber {
            self.sel.delete_renumbering(&self.doc)
        } else {
            self.sel.delete(&self.doc)
        };
        let done = self.apply_edit(cmd);
        if done {
            self.sel.clear();
        }
        done
    }

    /// §5.7.18 steps 2-4 的「复制」：整组一份副本，落在原地偏 [`CLONE_STEP`] 的地方，而
    /// **副本接着成为这一串** —— 第 4 步「用户移动副本到目标位置」要握的是刚生成的那几枚，
    /// 不是留在原处的那几枚。新 id 由命令自己带着（`Command::Add`），所以这里不猜它们叫什么；
    /// 一次 Ctrl+Z 收回的是整批副本而不是一枚。
    pub fn clone_selected(&mut self) -> bool {
        let copies = match self.sel.clone_by(&self.doc, CLONE_STEP, CLONE_STEP) {
            Some(Command::Add(list)) => list,
            _ => return false,
        };
        let ids: Vec<u64> = copies.iter().map(|e| e.id).collect();
        let done = self.apply_edit(Some(Command::Add(copies)));
        if done {
            self.sel = Selection::of(&ids);
        }
        done
    }

    /// 约束键此刻按着没有，由界面按**当下这一枚鼠标事件**递进来（§5.7.4 step 3）。只有拖
    /// 框的那三枚工具（矩形、圆角、椭圆）读它：笔是逐点记的，线段有自己的另一条规矩
    /// （§5.7.6 step 4 说的是端点，不是角度）。
    pub fn set_constraint(&mut self, on: bool) {
        self.constraint = on;
    }

    /// §5.7.18 step 2「按住 Ctrl 拖动」的那只手：界面把**当下这一枚按下事件**里那枚 Ctrl 按着
    /// 没有递进来，于是箭头工具在选区里的那一拖拖的是副本而不是原物。与
    /// [`Layer::set_constraint`] 同一分工（页上不自己判「这一趟拖的是谁」，抄进 `.qml` 就是
    /// 第二份），差别在读它的时刻：约束键每一寸移动都重写、这一枚只在下一枚按下有效 ——
    /// [`Layer::press`] 读一次即归 false（[`Layer::clone_hold`] 头上那句）。
    ///
    /// 这一格不 publishes 任何东西，所以后面不跟 `reload`，与 `holdConstraint` 同一待遇。
    pub fn hold_clone(&mut self, on: bool) {
        self.clone_hold = on;
    }

    /// `true` when a drawing tool took the press, so the caller must not also read
    /// it as a grip on the selection.
    pub fn press(&mut self, device: PhysPoint) -> bool {
        // 一笔从**没按着约束键**开始：上一笔松手时那颗键还按着，不算这一笔按下的理由。
        // 界面每一寸移动都重写这一格（[`Layer::set_constraint`]），所以这里清零不会把
        // "按住 Shift 拖第二笔"拖成自由比例。
        self.constraint = false;
        // 克隆那一枚修饰键在**这一枚按下**上用掉：读一次即归 false，所以下面拿到的是"这一枚
        // 按下按着 Ctrl 吗"，不是"上一次按下按着吗"（[`Layer::clone_hold`] 头上那句）。手里
        // 有笔时这一格也一样被用掉 —— 笔不认 Ctrl，但它不能把它留给下一枚按下。
        let ctrl = std::mem::replace(&mut self.clone_hold, false);
        if self.tool.is_none() {
            // The arrow tool is the object-editing pointer: §5.7.15's turn, §5.7.16's
            // 单击已有标注对象 and §5.7.17's 移动 all live in [`Layer::select_press`].
            return self.select_press(device, ctrl);
        }
        let Some(kind) = self.tool else {
            return false;
        };
        if self.hole.is_empty() {
            return false;
        }
        // 拿起笔开始画新的一枚，就是"上一枚不再是我正在编辑的那一枚"：§5.7.16 步 4 的完成
        // 编辑除点空白之外还有这一条路。不清的话，用户写第二行字时字号旋钮会把第一行一起
        // 拧粗 —— 而那一枚旋钮此刻报的是笔的读数，不是任何一枚对象的。
        self.sel.clear();
        let p = self.doc_point(device);
        self.anchor = Some(p);
        if kind == Kind::Polyline {
            // The press only *starts* a node. §5.7.5 adds them one click at a time,
            // and a click that was dragged steers the segment, so the point that
            // becomes a node is the one the button comes up at - not the one it went
            // down at, which is already a node or the tail of the previous segment.
            self.poly_cursor = Some(p);
            self.draft_polyline();
            return true;
        }
        if kind == Kind::Text {
            // §5.7.11 steps 2 and 5 are both this one event: the press that opens a box
            // and the press that closes the one already open. A press *inside* an open
            // box does not reach here at all - the field is the item under the pointer,
            // and it takes that click to move its own caret - so anything that does
            // reach this arm is step 5's "点击空白区域".
            //
            // The press that commits does not also place the next box. It could, and it
            // would save a click for someone writing a list, but it would also leave an
            // empty field under the pointer after every finish, and dismissing that
            // costs a click nobody meant to spend.
            if self.typing.is_some() {
                self.commit_typing();
            } else {
                self.typing = Some(Typing {
                    at: p,
                    text: String::new(),
                    editing: None,
                });
            }
            return true;
        }
        if kind == Kind::Number {
            // §5.7.12 step 3 是**一次点击**：按下就把这一号的盒子摆好，所以一次"按下又立刻
            // 松手"也是一枚编号，不是通用那一段眼里"什么都没拖出来"的一次空手。
            self.draft_number(p);
            return true;
        }
        if kind == Kind::Zoom {
            // The second drag of §5.7.13 holds the copy the first one generated, and
            // only `held` changes: `draft` still carries the preview, and clearing it
            // here would take pixels off the screen that nothing repaints until the
            // pointer moves. With no placement yet this is the first drag, whose rect is
            // built by [`Layer::drag`] from this anchor.
            if let Some(z) = self.zoom.as_mut() {
                let start = z.to;
                z.held = Some((p, start));
            }
            return true;
        }
        self.path = if kind.is_stroke() {
            vec![p]
        } else {
            Vec::new()
        };
        self.draft = Vec::new();
        true
    }

    pub fn drag(&mut self, device: PhysPoint) {
        if self.tool.is_none() {
            self.select_drag(device);
            return;
        }
        let (Some(kind), Some(anchor)) = (self.tool, self.anchor) else {
            return;
        };
        let p = self.doc_point(device);
        if kind == Kind::Polyline {
            self.poly_cursor = Some(p);
            self.draft_polyline();
            return;
        }
        if kind == Kind::Text {
            // A box is *placed*, not drawn: §5.7.11 step 2 is one click. Without this
            // early return the code below builds a `Geom::Rect` from the anchor to the
            // pointer, which for a tool that never previews anything is a drag that
            // moves nothing and a release that has to decide whether to keep it.
            return;
        }
        if kind == Kind::Number {
            // 拖出去的这一段只是把盒子挪到指针这儿 —— 点击落点的那一笔仍然算一次点击
            // （§5.7.12 step 3），按着挪一挪则是把这一枚号摆正，松手落的是当前这一点。
            self.draft_number(p);
            return;
        }
        if kind == Kind::Zoom {
            self.zoom_drag(anchor, p);
            return;
        }
        let geom = if kind.is_stroke() {
            if self.path.last().is_some_and(|last| *last == p) {
                return;
            }
            self.path.push(p);
            Geom::Path(self.path.clone())
        } else if matches!(kind, Kind::Line | Kind::Arrow | Kind::DoubleArrow) {
            Geom::Segment { a: anchor, b: p }
        } else {
            // §5.7.4 step 3「按住约束键可创建正圆」：按着的那一段，把拖到的那一点搬到与
            // 锚点**边长相等**的那个角上 —— 取两边里长的那条为准，所以越拖越斜是越接近
            // 1:1，而不是缩成一粒。椭圆于是就是正圆，矩形与圆角于是就是正方形。
            let to = if self.constraint {
                let side = (p.x - anchor.x).abs().max((p.y - anchor.y).abs());
                PhysPoint::new(
                    anchor.x + if p.x < anchor.x { -side } else { side },
                    anchor.y + if p.y < anchor.y { -side } else { side },
                )
            } else {
                p
            };
            Geom::Rect(PhysRect::from_points(anchor, to))
        };
        let mut e = Element::new(self.next_id, kind, geom, self.style.clone());
        e.z = self.doc.elements.iter().map(|x| x.z).max().unwrap_or(0) + 1;
        self.draft = vec![e];
        let area = self.draft[0].bounds();
        self.paint(Some(&area));
    }

    /// The stroke is finished: it joins the document through a [`Command::Add`], so
    /// undo removes exactly what the drag added. `false` is a click that drew
    /// nothing - a dot of a box, or one point of a path - which must not become an
    /// undoable nothing.
    pub fn release(&mut self) -> bool {
        self.anchor = None;
        if self.tool.is_none() {
            return self.select_release();
        }
        if self.tool == Some(Kind::Polyline) {
            // Not the end of the object, only of this node: the point came up here,
            // so it joins the polyline, and the rubber segment goes with the button.
            if let Some(c) = self.poly_cursor.take() {
                if self.poly.last() != Some(&c) {
                    self.poly.push(c);
                }
            }
            self.draft_polyline();
            return false;
        }
        if self.tool == Some(Kind::Text) {
            // The box is finished by the *next* press, by `Enter` or by 双击, never by
            // this release: the click that opened it is the same click whose button is
            // coming up now, and committing here would end the gesture before step 3
            // has had a single keystroke. Falling through is worse still - the code
            // below takes whatever is in `draft` and offers it to the document, and a
            // text box has no business in a slot that belongs to the previews.
            return false;
        }
        if self.tool == Some(Kind::Zoom) {
            // §5.7.13's two drags end differently. The first release *places* the copy
            // - step 3 is generated, not typed - and puts nothing in the document; the
            // second one ends the gesture, and the two rectangles become the one object
            // a single Ctrl+Z takes back.
            let Some(z) = self.zoom else {
                return false;
            };
            return if z.held.is_none() {
                false
            } else {
                self.commit_zoom()
            };
        }
        self.path.clear();
        let mut draft = std::mem::take(&mut self.draft);
        let Some(e) = draft.pop() else { return false };
        if e.bounds().is_empty() || matches!(&e.geom, Geom::Path(p) if p.len() < 2) {
            self.paint(None);
            return false;
        }
        self.next_id += 1;
        self.apply(Command::Add(vec![e]));
        true
    }

    /// §5.7.5's two finishers - 双击 and `Enter` - which are the same event as far as
    /// the layer is concerned. The nodes become one object through [`Command::Add`],
    /// so a single Ctrl+Z takes the whole polyline rather than its last leg.
    ///
    /// `false` means "there was no line here", which is what lets the mask's `Enter`
    /// fall through to 完成选区 instead of eating the keystroke mid-drawing.
    pub fn finish_polyline(&mut self) -> bool {
        self.poly_cursor = None;
        let nodes = std::mem::take(&mut self.poly);
        if nodes.len() < 2 {
            if self.abandon_polyline() {
                self.paint_last();
            }
            return false;
        }
        let mut e = Element::new(
            self.next_id,
            Kind::Polyline,
            Geom::Path(nodes),
            self.style.clone(),
        );
        e.z = self.doc.elements.iter().map(|x| x.z).max().unwrap_or(0) + 1;
        self.next_id += 1;
        self.draft.clear();
        self.apply(Command::Add(vec![e]));
        true
    }

    /// How many nodes the user has clicked so far, and how many the finisher is
    /// holding: the mask's `Enter` routes to [`Layer::finish_polyline`] first, and
    /// that call's own `false` is the answer to "was there a line here?" - so this is
    /// for the gauge and the tests to say *which* click they are reporting, not for a
    /// second ladder nobody has to agree with the first.
    pub fn polyline_nodes(&self) -> usize {
        self.poly.len()
    }

    // ------------------------------------------------------------ 局部放大

    /// Which of §5.7.13's two drags the pointer is in, decided by whether a placement
    /// already exists - the difference being that the first drag *draws* the source
    /// region and the second one *moves* the copy generated from it.
    fn zoom_drag(&mut self, anchor: PhysPoint, p: PhysPoint) {
        // Read out first: matching on `self.zoom` would hold it borrowed through the arm
        // that assigns to it.
        let pending = self.zoom;
        let next = match pending {
            Some(Placement {
                from,
                held: Some((a, start)),
                ..
            }) => Placement {
                from,
                // Travel, not a jump: the source region keeps its place, and so does
                // the copy's own size - only its corner moves.
                to: start.offset(p.x - a.x, p.y - a.y),
                held: Some((a, start)),
            },
            _ => {
                let from = PhysRect::from_points(anchor, p);
                if from.is_empty() {
                    // Nothing to magnify yet, so nothing to show: taking the preview back
                    // rather than framing a 1x1 copy of the desktop is also what makes a
                    // click with this tool a click that drew nothing.
                    self.zoom = None;
                    self.draft_zoom();
                    return;
                }
                let to = self.place(from);
                Placement {
                    from,
                    to,
                    held: None,
                }
            }
        };
        self.zoom = Some(next);
        self.draft_zoom();
    }

    /// Where the copy goes when the user has not said otherwise (§5.7.13 step 3):
    /// `zoom_percent` of the source's own size, its corner a pen's width beyond the
    /// source's bottom-right so the copy does not sit glued on top of what it magnifies.
    ///
    /// Each side is capped at the selection's own length. Anything wider is clipped away
    /// by the ink rule §9.1's ③ states for every tool, and without the cap an 8x of a 4K
    /// source is a 1.5 GB allocation inside the rasteriser's `resized` - a number this
    /// program should not discover on a user's machine.
    fn place(&self, from: PhysRect) -> PhysRect {
        let pct = self.style.zoom_percent.clamp(ZOOM_MIN, ZOOM_MAX) as u64;
        let hole = self.hole_doc();
        let side = |src: u32, cap: u32| {
            (src as u64 * pct / 100)
                .clamp(1, cap.max(1) as u64)
                .try_into()
                .unwrap_or(1)
        };
        let gap = self.style.width.max(4) as i32;
        PhysRect::new(
            from.right() + gap,
            from.bottom() + gap,
            side(from.w, hole.w),
            side(from.h, hole.h),
        )
    }

    /// The pending 放大 as a draft: the source region, the copy and the connectors
    /// between them are one element, because step 4 drags *the copy* and a gesture that
    /// moved two objects would leave the two of them disagreeing about the same
    /// magnifier. The same no-op rule as [`Layer::draft_polyline`] applies - an unchanged
    /// preview must not buy a new key, or the gauge charges the flow for a stroke that
    /// changed nothing - and "unchanged" now covers the pen as well as the geometry, see
    /// [`Layer::draft_matches`].
    fn draft_zoom(&mut self) {
        let wanted = self.zoom.map(|z| Geom::Zoom {
            from: z.from,
            to: z.to,
        });
        if self.draft_matches(wanted.as_ref()) {
            return;
        }
        let was = self.draft.first().map(|e| e.bounds());
        self.draft = match wanted {
            Some(geom) => {
                let mut e = Element::new(self.next_id, Kind::Zoom, geom, self.style.clone());
                e.z = self.doc.elements.iter().map(|x| x.z).max().unwrap_or(0) + 1;
                vec![e]
            }
            None => Vec::new(),
        };
        let now = self.draft.first().map(|e| e.bounds());
        let area = match (was, now) {
            (Some(a), Some(b)) => Some(a.union(&b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        if let Some(area) = area {
            self.paint(Some(&area));
        }
    }

    /// The gesture is over: the copy joins the document as one [`Command::Add`], so undo
    /// takes both rectangles back with it.
    fn commit_zoom(&mut self) -> bool {
        let Some(z) = self.zoom.take() else {
            return false;
        };
        if z.from.is_empty() || z.to.is_empty() {
            self.draft_zoom();
            return false;
        }
        let mut e = Element::new(
            self.next_id,
            Kind::Zoom,
            Geom::Zoom {
                from: z.from,
                to: z.to,
            },
            self.style.clone(),
        );
        e.z = self.doc.elements.iter().map(|x| x.z).max().unwrap_or(0) + 1;
        self.next_id += 1;
        self.draft.clear();
        self.apply(Command::Add(vec![e]));
        true
    }

    /// `Enter` and the double-click accept the copy where it stands, for a user who drew
    /// the source region and wants the default placement rather than a second drag. The
    /// order against 折线 is [`crate::mask::MaskState::finish_ink`]'s, not this module's
    /// and not QML's, so the ladder stays in one place.
    pub fn finish_zoom(&mut self) -> bool {
        if self.tool != Some(Kind::Zoom) || self.zoom.is_none() {
            return false;
        }
        self.commit_zoom()
    }

    /// The placement the first drag left behind, as the two rectangles it is made of -
    /// for the gauge and the tests to say *which* drag they are reporting, the same role
    /// [`Layer::polyline_nodes`] plays for 折线, and deliberately not a `bool` any second
    /// ladder could disagree with the first about.
    ///
    /// Returned in *desktop* pixels: the internals of this layer are document space, but
    /// both callers compare against [`crate::mask::MaskState::hole`], which is the
    /// desktop because that is what a window is positioned in.
    pub fn zoom_pending(&self) -> Option<(PhysRect, PhysRect)> {
        self.zoom.map(|z| {
            (
                z.from.offset(self.origin.x, self.origin.y),
                z.to.offset(self.origin.x, self.origin.y),
            )
        })
    }

    /// [`Layer::zoom_pending`] in the shape `ink_line` prints, with the held flag the
    /// second drag sets - so the one line says both which gesture is in progress and
    /// whether the copy has moved yet.
    pub fn zoom_line(&self) -> String {
        let Some(z) = self.zoom else {
            return "-".to_string();
        };
        let (dx, dy) = (self.origin.x, self.origin.y);
        format!(
            "{:?}=>{:?}{}",
            z.from.offset(dx, dy),
            z.to.offset(dx, dy),
            if z.held.is_some() { " held" } else { "" }
        )
    }

    /// Drop a pending 放大, preview and all. `true` is "pixels of it were on screen", so
    /// each caller knows it owes a repaint: the same debt [`Layer::abandon_polyline`]
    /// answers for, for the same reason - the copy never entered the document, so neither
    /// the undo stack nor `Command::Remove` can see the pixels it drew.
    fn abandon_zoom(&mut self) -> bool {
        let had = self.draft.first().is_some_and(|e| e.kind == Kind::Zoom);
        self.zoom = None;
        if had {
            self.draft.clear();
        }
        had
    }

    /// The open box, in *desktop* pixels - the same space [`Layer::zoom_pending`]
    /// answers in, for the same reason: the caller is [`crate::mask`], and a window is
    /// positioned in the desktop. `None` when nothing is being typed.
    pub fn typing_rect(&self) -> Option<PhysRect> {
        self.typing.as_ref().map(|t| {
            self.box_at(t.at, &t.text, &DirectWrite)
                .offset(self.origin.x, self.origin.y)
        })
    }

    /// §5.7.11 step 4's 字号, in device pixels. The field needs it in its own units, and
    /// the conversion is the screen's - so this hands over the raw number and
    /// [`crate::mask::MaskState::view_data`] divides it, the way it divides the box.
    ///
    /// This is the **pen's** number, which is what the toolbar's cell lights. The field
    /// inside the open box reads [`Layer::typing_font_px`] instead: while a retype is in
    /// hand the two are not the same number, and a field sized by the pen inside a box
    /// measured by the object is letters overflowing the box they sit in.
    pub fn font_px(&self) -> u32 {
        self.style.font_size
    }

    /// §5.7.11 step 4's 字体. Read out as well as in, because the pen is the only place
    /// the answer exists: a config key that sets a family nothing can report back is a
    /// setting whose effect can only be guessed at, which is the shape this round was
    /// written to close. The pen's own, for the same reason and with the same split as
    /// [`Layer::font_px`]'s - the field's family is [`Layer::typing_font_family`].
    pub fn font_family(&self) -> &str {
        &self.style.font_family
    }

    /// The 字号 the open box was measured in, in device pixels: the object's while a
    /// retype is in hand, the pen's for a box being written now ([`Layer::box_style`]).
    /// While no box is open this is [`Layer::font_px`] byte for byte, which is why one
    /// number has always been enough until now.
    pub fn typing_font_px(&self) -> u32 {
        self.box_style().font_size
    }

    /// The family half of [`Layer::typing_font_px`]'s answer, asked and answered together
    /// because the box is measured with **both**: a field that gets the right size in the
    /// wrong family is still a field whose letters do not fit the box.
    pub fn typing_font_family(&self) -> &str {
        &self.box_style().font_family
    }

    /// The colour half of [`Layer::typing_font_px`]'s answer, same leg and same rule: the
    /// object's own while a retype is in hand, the pen's for a box being written now.
    ///
    /// 页上那一串字过去绑的是 `pen_rgb`。落新框时那一格是诚实的（新对象拿的正是画笔这一枚），
    /// 改字时不诚实：提交上去的那一枚保留它自己的颜色，于是字段里的字在框关掉那一刻换一次色
    /// —— 而用户刚刚看着自己编辑的那一串就是旧的那一枚颜色（§5.7.16 步 3）。字号与字体名
    /// 换成"框的作者"之后，颜色是分岔里剩下的最后一枚。
    ///
    /// 第四枚字节（alpha）在这一格里跟着 `Style.color` 一起交出，但桥那一头递的是
    /// `0xRRGGBB`、页上解出来按 1.0 铺 —— 与 `pen_rgb` 同规，本轮没改那一条（写在这里而不是
    /// 让它没有对面）。
    pub fn typing_colour(&self) -> [u8; 4] {
        self.box_style().color
    }

    /// The style the open box is measured and drawn in: the object's own while a retype
    /// is in hand ([`Layer::edit_selected_text`]), the pen's for a box being written now.
    ///
    /// One author, because the things that ask are [`Layer::box_at`] (and through it
    /// [`Layer::typing_rect`] and [`Layer::commit_typing`]), the field's two size
    /// parameters ([`Layer::typing_font_px`], [`Layer::typing_font_family`]) and the
    /// string the field opens with ([`Layer::typing_text`]). A second one of them reading
    /// the pen directly is a box whose letters do not fit it.
    ///
    /// A retype whose object has gone - deleted from the panel while the field had the
    /// keyboard - falls back to the pen rather than guessing at a style nobody can show:
    /// that box is one keystroke from filing nothing, which is
    /// [`falcon_core::annotation::text_command`]'s answer to an unknown id.
    fn box_style(&self) -> &Style {
        match &self.typing {
            Some(t) => t
                .editing
                .and_then(|id| self.doc.get(id))
                .map_or(&self.style, |e| &e.style),
            None => &self.style,
        }
    }

    /// The string the open box holds, `""` when there is no box. The field reads it once,
    /// at the moment it becomes visible, which is the seed it was opened with: §5.7.11's
    /// new box starts empty and §5.7.16 步 3's retype starts with the object's own
    /// sentence. Not a second copy of the string - the layer holds the only one, and the
    /// field pushes every change back through [`Layer::type_text`].
    pub fn typing_text(&self) -> &str {
        self.typing.as_ref().map_or("", |t| &t.text)
    }

    /// Is the open box a retype of something already in the document? [`Layer::select_press`]
    /// needs this to file it on the click that ends the edit (§5.7.16 步 4).
    fn typing_editing(&self) -> bool {
        self.typing.as_ref().is_some_and(|t| t.editing.is_some())
    }

    /// The box a string lands in at a click, in document space: the point for a corner,
    /// the font for the two sides.
    ///
    /// The measure comes from [`raster::text_extent`], which asks the same leg that will
    /// draw the letters, so the box cannot disagree with them. When the leg cannot
    /// answer there is nothing to draw either - the number only has to be *stable*, one
    /// em tall per line and half an em per character, because a box that resized on
    /// every keystroke would move the field under the user's caret for no reason. The
    /// leg arrives as an argument so a test can stand in the second case; every call
    /// site in this module hands over the real one.
    ///
    /// Which 字号 and 字体 are asked for is [`Layer::box_style`]'s answer, not this
    /// function's: a box being written now is measured with the pen, a retype with the
    /// object's own style.
    fn box_at(&self, at: PhysPoint, text: &str, glyphs: &dyn raster::Glyphs) -> PhysRect {
        let style = self.box_style();
        let size = raster::text_extent(text, style, glyphs).unwrap_or_else(|| {
            let em = style.font_size.max(1);
            // The *widest line*, not the whole string: a newline must not grow the box
            // sideways. `text_extent` gets this right by construction and this branch is
            // the one place that could get it wrong without anyone seeing it, because
            // the box is drawn by the field rather than by the layer.
            let mut lines = 0u32;
            let mut chars = 0u32;
            for line in text.split('\n') {
                lines += 1;
                chars = chars.max(line.chars().count() as u32);
            }
            (
                chars.saturating_mul(em / 2).max(1),
                lines.saturating_mul(em),
            )
        });
        PhysRect::new(at.x, at.y, size.0, size.1)
    }

    /// §5.7.11 step 3: the field's whole current string, pushed on every change.
    ///
    /// A call with no box in hand is a silent no-op rather than an error: the bridge
    /// cannot tell which window's field is the live one, and a stale signal from a field
    /// that a reload just took off the screen must not invent a box at the last click.
    pub fn type_text(&mut self, text: &str) {
        let Some(t) = self.typing.as_mut() else {
            return;
        };
        t.text = text.to_string();
    }

    /// §5.7.16 步 3 的「文字内容」的门：把选中的那一枚 文本 摊开成一只框 —— 与 §5.7.11
    /// 步 2 那一只是**同一只**（同一个 [`Typing`]、页上同一枚 `TextEdit`、同一个
    /// [`Layer::finish_typing`] 收尾、Esc 阶梯上的同一格 [`Layer::abandon_typing`]），差别
    /// 只在框里预先放着原有的那串字（[`Layer::typing_text`]），并且记着它是来改谁的。
    ///
    /// `false` 的那几样各是各的：手里是笔而不是箭头（步 1 说的是「选择"选择"工具」，而笔
    /// 底下点哪都先落**新的一枚**，改不动已经在那儿的那一串）、框已经开着（不许叠第二只 ——
    /// [`Layer::typing_text`] 只有一份，前一只的字还挂在屏幕上）、串里不恰好一枚（多选没有
    /// "这一枚的字"，页上也只有一只框），以及 core 那两位作者的话：这一枚不接字
    /// （[`Kind::takes_text`]，编号画的是号）与锁着（[`Document::can_edit`]，选中不等于许可）。
    /// 后两问这里不重判第二遍，只是**在开门之前**问一遍 —— 门开了、框也亮了，到提交时才说
    /// 「改不了」，屏幕上就没有任何一句话会说这件事（#494）。
    ///
    /// 开门不改文档，所以撤销栈上不多东西；改到一半换工具或按 Esc，收走的只是那只框
    /// （§9.1 的⑦），原来那串字一个像素也没动过。
    pub fn edit_selected_text(&mut self) -> bool {
        if self.tool.is_some() || self.typing.is_some() {
            return false;
        }
        let [id] = self.sel.ids() else {
            return false;
        };
        let id = *id;
        let Some(e) = self.doc.get(id) else {
            return false;
        };
        if !e.kind.takes_text() || !self.doc.can_edit(id) {
            return false;
        }
        let at = e.geom.bounds();
        let text = e.text.clone();
        self.typing = Some(Typing {
            at: PhysPoint::new(at.x, at.y),
            text,
            editing: Some(id),
        });
        true
    }

    /// Take the box out of hand and put its text in the document, so one Ctrl+Z takes the
    /// whole line back. `false` is a box with nothing in it: [`Layer::release`] refuses to
    /// make an undoable nothing out of a click that drew a dot, and a keystroke history
    /// with a blank entry in it is the same mistake wearing a different hat. The box closes
    /// either way - the `take` is the first line here, which is what lets the mask's `Enter`
    /// fall through to 完成选区 without leaving a field on the screen.
    ///
    /// 提交的那一枚同时成为**当前对象**：§5.7.11 步 5 与 §5.7.16 步 3 是同一段——用户敲完
    /// 回车，紧接着就要动那一行的字号、字体、背景与描边，而文本工具此刻还亮着、那一排五枚
    /// 旋钮还在屏幕上。没有这一句，那一排在选到东西的整段时间里都是空转的（`sel` 一起画笔就被
    /// 清空，屏幕上再没有第二条路能既有选中的文本、又让那五枚可见）。
    ///
    /// A retype files the other kind of change: the string through [`text_command`], and the
    /// box through [`resize_to_rect`] **only when the new string needs more room than the
    /// object has** - never less. Shrinking to the measure would take back the width
    /// §5.7.11's 调整大小 gave that box by hand, and a shortened line would move the frame
    /// out from under the letters still standing in it. The two halves go into one
    /// [`Command::Compound`] so one Ctrl+Z ends the whole edit (the same split
    /// [`falcon_core::annotation::delete_and_renumber_command`] makes); a retype that leaves
    /// the box as wide as it is files the string alone, because then there is only one thing
    /// to undo.
    fn commit_typing(&mut self) -> bool {
        let Some(t) = self.typing.take() else {
            return false;
        };
        if t.text.trim().is_empty() {
            return false;
        }
        let box_ = self.box_at(t.at, &t.text, &DirectWrite);
        let Some(id) = t.editing else {
            let mut e = Element::new(
                self.next_id,
                Kind::Text,
                Geom::Rect(box_),
                self.style.clone(),
            );
            e.text = t.text;
            e.z = self.doc.elements.iter().map(|x| x.z).max().unwrap_or(0) + 1;
            self.next_id += 1;
            let id = e.id;
            self.apply(Command::Add(vec![e]));
            self.sel = Selection::one(id);
            return true;
        };
        let old = self.doc.get(id).map(|e| e.geom.bounds());
        let Some(text) = text_command(&self.doc, id, &t.text) else {
            return false;
        };
        let Some(old) = old else {
            return false;
        };
        let grown = PhysRect::new(old.x, old.y, box_.w.max(old.w), box_.h.max(old.h));
        // `resize_to_rect` 对「锁着」与「已经不在了」交回 `None`，而 [`text_command`] 刚刚把
        // 这两样各拒过一遍（同一位作者的两位判官），所以那一只盒子必然建得出；`_` 那一臂
        // 因此是「字照样交，盒子没变」而不是「悄悄丢一半」。
        let cmd = match resize_to_rect(&self.doc, id, grown) {
            Some(resize) if grown != old => Command::Compound(vec![text, resize]),
            _ => text,
        };
        self.apply(cmd);
        true
    }

    /// `Enter` and 双击 accept the box where it stands (§5.7.11 has no third finisher,
    /// and step 5's click-away is in [`Layer::press`]). `false` means "there was no text
    /// here", which is what lets the mask's `Enter` fall through to 完成选区 instead of
    /// eating the keystroke. The order against 折线 and 放大 is
    /// [`crate::mask::MaskState::finish_ink`]'s, not this module's and not QML's.
    ///
    /// A retype's box answers the same two keystrokes with the pen *not* in hand - the
    /// arrow tool is what selected the object in the first place (§5.7.16 steps 1-2), so
    /// the pen is no part of this guard for that one case, and the box itself carries the
    /// answer ([`Typing::editing`]).
    pub fn finish_typing(&mut self) -> bool {
        let Some(t) = &self.typing else {
            return false;
        };
        if t.editing.is_none() && self.tool != Some(Kind::Text) {
            return false;
        }
        self.commit_typing()
    }

    /// Drop a half-typed box, text and all. `true` is "there was a box", which is what
    /// tells the Esc handler the keystroke was spent - and deliberately not the
    /// "pixels of it were on screen" that [`Layer::abandon_polyline`] answers, because
    /// the field had those pixels and this layer never had them to give back.
    pub fn abandon_typing(&mut self) -> bool {
        self.typing.take().is_some()
    }

    /// [`Layer::typing_rect`] in the shape `ink_line` prints, with the string quoted so
    /// a space or a pipe in what the user typed cannot break the line into fields it
    /// does not have. `-` for no box, the same spelling 放大's half of the line uses.
    pub fn typing_line(&self) -> String {
        let Some(t) = &self.typing else {
            return "-".to_string();
        };
        let box_ = self
            .box_at(t.at, &t.text, &DirectWrite)
            .offset(self.origin.x, self.origin.y);
        format!("{box_:?} {:?}", t.text)
    }

    /// The pending polyline plus the point the pointer is at: one element, painted as
    /// a draft and never in the document. The two halves are drawn together because a
    /// polyline that shows only its committed nodes hides the segment being aimed.
    fn draft_polyline(&mut self) {
        let mut pts = self.poly.clone();
        if let Some(c) = self.poly_cursor {
            if pts.last() != Some(&c) {
                pts.push(c);
            }
        }
        let wanted = if pts.len() >= 2 {
            Some(Geom::Path(pts))
        } else {
            None
        };
        if self.draft_matches(wanted.as_ref()) {
            // The release after a click that was not dragged asks for exactly the
            // preview the press already painted, and repainting the same pixels under a
            // new key would charge the flow for a stroke that changed nothing.
            return;
        }
        let was = self.draft.first().map(|e| e.bounds());
        self.draft = match wanted {
            Some(geom) => {
                let mut e = Element::new(self.next_id, Kind::Polyline, geom, self.style.clone());
                e.z = self.doc.elements.iter().map(|x| x.z).max().unwrap_or(0) + 1;
                vec![e]
            }
            None => Vec::new(),
        };
        let now = self.draft.first().map(|e| e.bounds());
        let area = match (was, now) {
            (Some(a), Some(b)) => Some(a.union(&b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        if let Some(area) = area {
            self.paint(Some(&area));
        }
    }

    /// Drop a half-drawn polyline, preview and all. `true` is "pixels of it were on
    /// screen", so the caller knows it owes a repaint: the preview is in the layer but
    /// in nobody's document, and nothing else can take those pixels back.
    ///
    /// Only a draft that *is* a polyline. 放大 joins the same single `draft` slot, and an
    /// abandon that cleared the other gesture's pixels as a side effect would repaint them
    /// away while leaving its state in place - which is the half-reset shape again, one
    /// field further along.
    fn abandon_polyline(&mut self) -> bool {
        let had = self.draft.first().is_some_and(|e| e.kind == Kind::Polyline);
        self.poly.clear();
        self.poly_cursor = None;
        if had {
            self.draft.clear();
        }
        had
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// Ctrl+Z. The stack answers with the rects to repaint, which is the whole point
    /// of routing deletion through [`Command::Remove`] instead of dropping elements
    /// quietly: the pixels they drew have to be put back.
    pub fn undo_step(&mut self) -> bool {
        match self.undo.undo(&mut self.doc) {
            Some(dirty) => {
                self.repaint(&dirty);
                self.prune_selection();
                true
            }
            None => false,
        }
    }

    pub fn redo_step(&mut self) -> bool {
        match self.undo.redo(&mut self.doc) {
            Some(dirty) => {
                self.repaint(&dirty);
                self.prune_selection();
                true
            }
            None => false,
        }
    }

    /// 全清: every object out, through one command so it is undoable as one step, and
    /// the half-drawn gestures out with them - a polyline's preview and a 放大's pending
    /// copy are both on the screen, and a 全清 that left either behind would be the one
    /// gesture that fails to clear.
    pub fn clear_ink(&mut self) -> bool {
        let all: Vec<Element> = self.doc.elements.clone();
        let mut dropped = self.abandon_polyline();
        dropped |= self.abandon_zoom();
        self.abandon_typing();
        if all.is_empty() {
            if dropped {
                self.paint_last();
            }
            return false;
        }
        self.apply(Command::Remove(all));
        // 全清把每一枚都带走了，那一串自然也就没有成员可留。
        self.prune_selection();
        // The previews are not in `all`, so none of the removal's dirty rects cover the
        // pixels they drew. An abandon that happens alongside a real removal therefore
        // has to be paid for with a full repaint rather than trusted to the command's
        // own rects - the case P19's ⑦ named but did not have a test reach, because its
        // 全清 runs had an empty document.
        if dropped {
            self.paint_last();
        }
        true
    }

    pub fn objects(&self) -> usize {
        self.doc.len()
    }

    /// Flatten one screen for export: its frozen base with every object on it. The
    /// mask keeps drawing its own dim; this is the picture the crop takes, and it is
    /// the only place the whole document is re-rendered in this module.
    pub fn flatten(&self, name: &str) -> Option<Frame> {
        let c = self.canvases.iter().find(|c| c.name == name)?;
        let (dx, dy) = shift(self.origin, c.bounds);
        raster::render(
            &c.base,
            &view_of(&self.doc.elements, c, dx, dy),
            &DirectWrite,
        )
        .ok()
    }

    /// §5.5's 成图: the picture the selection takes, stitched from every screen that
    /// has a share of it.
    ///
    /// Two spaces meet in the four lines below and both of them have broken this
    /// module before (§9.1 ①): `hole` is desktop pixels, while a `flatten` is one
    /// screen's own pixels from its own corner. So the cut is an intersection taken in
    /// desktop space and *then* shifted into the screen's, which is what lets a hole
    /// that straddles a seam come out of two canvases instead of one stretched one.
    ///
    /// It reads `flatten` and not [`Layer::layer_of`] on purpose: the incremental
    /// canvas only knows the rectangles it was asked to repaint, and what leaves the
    /// process has to be the whole document as it now stands.
    pub fn deliver(&self) -> Option<Frame> {
        if self.hole.is_empty() || self.canvases.is_empty() {
            return None;
        }
        let mut out = Frame::new(self.hole.w, self.hole.h).ok()?;
        for c in &self.canvases {
            let Some(part) = self.hole.intersection(&c.bounds) else {
                continue;
            };
            let full = self.flatten(&c.name)?;
            let cut = full
                .crop(&PhysRect::new(
                    part.x - c.bounds.x,
                    part.y - c.bounds.y,
                    part.w,
                    part.h,
                ))
                .ok()?;
            out.paste(
                &cut,
                PhysPoint::new(part.x - self.hole.x, part.y - self.hole.y),
            )?;
        }
        Some(out)
    }

    /// How much of the selection no canvas covers: the pixels [`Layer::deliver`]
    /// would hand out as untouched black. `0` is the normal answer, and a non-zero one
    /// is a hole that left the captured desktop - the row that ships the picture prints
    /// this rather than assuming the frame is whole.
    ///
    /// Screens tile a desktop without overlapping it, so the shares add up. A mirrored
    /// desktop would break that assumption by counting one area twice, which is why this
    /// saturates instead of subtracting: the worst case then reads as "fully covered",
    /// not as an underflow.
    pub fn uncovered(&self) -> u64 {
        if self.hole.is_empty() {
            return 0;
        }
        let covered: u64 = self
            .canvases
            .iter()
            .filter_map(|c| self.hole.intersection(&c.bounds))
            .map(|part| part.area())
            .sum();
        self.hole.area().saturating_sub(covered)
    }

    /// A whole-canvas repaint, for the cases where the document changed in a way no
    /// dirty rect describes (an abandoned draft).
    fn paint_last(&mut self) {
        self.paint(None);
    }

    fn apply(&mut self, cmd: Command) -> Dirty {
        let dirty = cmd.apply(&mut self.doc);
        let now = self.clock.elapsed().as_millis() as u64;
        self.undo.record(cmd, now);
        self.repaint(&dirty);
        dirty
    }

    fn repaint(&mut self, dirty: &Dirty) {
        let Some(area) = dirty.merged() else {
            // Nothing moved, so nothing may be repainted - and nothing may be
            // published, which is what this returns early of.
            return;
        };
        self.paint(Some(&area));
    }

    /// Repaint `area` (document space) - or all of it, when `None` - into every
    /// canvas, and mark those canvases as needing publication.
    ///
    /// The area is only ever *widened* here: the reach of the pen that draws the
    /// objects in it is added on top, because a 4 px line with a 12 px stroke has to
    /// repaint 4 px more on every side than its geometry says. `Command`'s dirty
    /// rects already include the reach of what they changed; this is the belt for the
    /// draft, whose element is not in the document yet.
    fn paint(&mut self, area: Option<&PhysRect>) {
        if self.canvases.is_empty() {
            return;
        }
        let started = Instant::now();
        let mut elements = self.doc.elements.clone();
        elements.extend(self.draft.iter().cloned());
        let origin = self.origin;
        let next_rev = self.rev + 1;
        let shifts: Vec<(i32, i32)> = self
            .canvases
            .iter()
            .map(|c| shift(origin, c.bounds))
            .collect();
        // The selection in each canvas's *own* pixels, because the clip below is
        // compared against `area`, which is canvas-local. A document-space rect would
        // only answer for the one screen that sits at the desktop's corner; every other
        // screen gets an offset, and a clip offset from the thing it clips throws that
        // screen's ink away - the ink rule §9.1 ③ applied to the wrong pixels, so
        // nothing is drawn where the user is drawing.
        let holes: Vec<PhysRect> = self
            .canvases
            .iter()
            .map(|c| self.hole.offset(-c.bounds.x, -c.bounds.y))
            .collect();
        let mut touched = 0u64;
        let mut problems = Vec::new();
        for (i, canvas) in self.canvases.iter_mut().enumerate() {
            let (dx, dy) = shifts[i];
            let bounds = canvas.layer.bounds();
            let local = match area {
                Some(a) => {
                    let shifted = PhysRect::new(a.x + dx, a.y + dy, a.w, a.h);
                    match shifted.intersection(&bounds) {
                        Some(hit) => hit,
                        None => continue,
                    }
                }
                None => bounds,
            };
            // The areas handed to this function are already `Element::bounds()`,
            // which is the geometry *inflated by the pen's reach* - so a 4 px line
            // with a 12 px stroke asks for 4 px more on every side than its geometry
            // says, and adding the reach a second time here would only make the
            // measured `last_paint_px` a lie about the cost.
            let area = local;
            let doc = view_of(&elements, canvas, dx, dy);
            if let Err(e) =
                raster::paint(&canvas.base, &doc, &mut canvas.layer, &area, &DirectWrite)
            {
                problems.push(format!("repaint of {} failed: {e}", canvas.name));
            }
            // Outside the selection the layer has to show *nothing*, or an undimmed
            // copy of the desktop replaces the dim that QML just drew under it.
            let Some(clip) = holes[i].intersection(&area) else {
                // Whole area is outside the hole: all of it becomes transparent.
                for y in area.y..area.bottom() {
                    for x in area.x..area.right() {
                        canvas.layer.set(x as u32, y as u32, [0, 0, 0, 0]);
                    }
                }
                canvas.painted_rev = next_rev;
                touched += area.area();
                continue;
            };
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    if !clip.contains(PhysPoint::new(x, y)) {
                        canvas.layer.set(x as u32, y as u32, [0, 0, 0, 0]);
                    }
                }
            }
            touched += area.area();
            canvas.painted_rev = next_rev;
        }
        self.rev = next_rev;
        self.paints += 1;
        self.last_paint_px = touched;
        self.last_paint_ms = Some(started.elapsed().as_millis() as u64);
        self.problems.extend(problems);
    }

    /// The layer's current key for one canvas, or empty when it has nothing to show.
    pub fn overlay_key(&self, name: &str) -> String {
        self.canvases
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.published.clone())
            .unwrap_or_default()
    }

    /// One canvas's layer, for the gauge and the tests to read bytes out of.
    pub fn layer_of(&self, name: &str) -> Option<&Frame> {
        self.canvases
            .iter()
            .find(|c| c.name == name)
            .map(|c| &c.layer)
    }

    /// Hand every canvas repainted since the last flush to Qt under a new key, then
    /// let the old key go. `reload` calls this, so the GUI thread is the one that
    /// publishes.
    ///
    /// The order is the release-reload handshake of §3.6 constraint 8: the new key
    /// is stored *before* the old one is dropped, so a window that has not re-read
    /// its URL yet still finds a texture, and one that has already re-read it finds
    /// the new pixels. Dropping first would make the second stroke's request answer
    /// with the provider's 1x1 transparent placeholder - a stroke that is in the
    /// document, in the canvas and nowhere on screen.
    pub fn flush(&mut self) -> Vec<String> {
        if self
            .canvases
            .iter()
            .all(|c| c.painted_rev == c.published_rev)
        {
            return Vec::new();
        }
        let rev = self.rev;
        let mut keys = Vec::new();
        let mut drops = Vec::new();
        for c in &mut self.canvases {
            if c.painted_rev == c.published_rev {
                continue;
            }
            let key = format!("{}-ink-{rev}", c.name);
            shim::store_raw(&key, &c.layer.pixels, c.layer.width, c.layer.height);
            let previous = std::mem::replace(&mut c.published, key.clone());
            c.published_rev = c.painted_rev;
            keys.push(key);
            if !previous.is_empty() {
                drops.push(previous);
            }
        }
        for old in drops {
            shim::drop_frame(&old);
        }
        self.flushes += 1;
        self.last_keys = keys.clone();
        keys
    }

    /// Repaints, the pixels the last one touched against the whole, and the publish
    /// count. This is the line that makes "增量" a claim rather than a name.
    pub fn paint_line(&self) -> String {
        format!(
            "paints={} last={}px of {}px ({:.1}%) {}ms objects={} depth={} flushes={} keys={}",
            self.paints,
            self.last_paint_px,
            self.full_px,
            if self.full_px == 0 {
                0.0
            } else {
                self.last_paint_px as f64 * 100.0 / self.full_px as f64
            },
            self.last_paint_ms.unwrap_or(0),
            self.doc.len(),
            self.undo.depth(),
            self.flushes,
            self.last_keys.join(",")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use falcon_core::annotation::{MARKER_ALPHA, OUTLINE_WIDTH_PX, ROTATE_LIMIT};
    use falcon_core::config::Config;
    use falcon_core::imageops::Effect;

    /// A layer over fake screens, with no freeze and no Qt: [`Layer::flush`] is the
    /// only method in this module that touches the image provider, and nothing here
    /// calls it. The desktop origin is derived from the screens the same way
    /// [`Layer::begin`] derives it, so a test can put the desktop anywhere it likes -
    /// including to the left of zero, which no machine in this house has.
    fn screens(list: &[(&str, PhysRect)], grey: [u8; 4]) -> Layer {
        let mut l = Layer::default();
        let mut min = PhysPoint::new(i32::MAX, i32::MAX);
        let mut max = PhysPoint::new(0, 0);
        for (name, bounds) in list {
            min.x = min.x.min(bounds.x);
            min.y = min.y.min(bounds.y);
            max.x = max.x.max(bounds.right());
            max.y = max.y.max(bounds.bottom());
            l.canvases.push(Canvas {
                name: (*name).to_string(),
                bounds: *bounds,
                base: Frame::filled(bounds.w, bounds.h, grey).unwrap(),
                layer: Frame::filled(bounds.w, bounds.h, [0, 0, 0, 0]).unwrap(),
                published: String::new(),
                painted_rev: 0,
                published_rev: 0,
            });
        }
        l.origin = min;
        l.doc = Document::new((max.x - min.x) as u32, (max.y - min.y) as u32);
        l.full_px = list.iter().map(|(_, b)| b.area()).sum();
        l
    }

    /// Every opaque pixel of the incremental layer, against a whole-canvas re-render
    /// of the same document. `None` means the canvas is missing; a `Some(0)` is the
    /// case where nothing has been painted yet and there is nothing to compare.
    fn mismatched(l: &Layer, name: &str) -> Option<Vec<String>> {
        let export = l.flatten(name)?;
        let layer = l.layer_of(name)?;
        let mut bad = Vec::new();
        for cy in 0..layer.height {
            for cx in 0..layer.width {
                let a = layer.get(cx, cy);
                if a[3] == 0 {
                    // The overlay only knows the areas it was asked to repaint; QML
                    // draws the frozen frame under the rest.
                    continue;
                }
                let b = export.get(cx, cy);
                if a != b && bad.len() < 5 {
                    bad.push(format!("{name} {cx},{cy}: layer={a:?} render={b:?}"));
                }
            }
        }
        Some(bad)
    }

    #[test]
    fn a_negative_desktop_origin_lands_on_the_right_screen() {
        // Two 640x320 screens side by side, the pair starting at x = -1280. The box is
        // drawn on the left one, and the test is that the *other* canvas stays empty -
        // a sign error in `shift` would put the ink on the wrong monitor and still
        // look right on a desktop that starts at zero.
        let left = PhysRect::new(-1280, 0, 640, 320);
        let right = PhysRect::new(-640, 0, 640, 320);
        let grey = [90, 90, 90, 255];
        let mut l = screens(&[("left", left), ("right", right)], grey);
        l.set_hole(PhysRect::new(-1280, 0, 1280, 320));
        l.set_color([255, 0, 0, 255]);
        l.select_tool(code_of(Some(Kind::Rect)));

        // 100,100 to 200,200 of the left screen, in desktop pixels.
        assert!(l.press(PhysPoint::new(-1180, 100)));
        l.drag(PhysPoint::new(-1080, 200));
        assert!(l.release());
        assert_eq!(l.objects(), 1);

        // Local (100,100) is the box's own border: opaque, and red-dominant.
        let on = l.layer_of("left").unwrap().get(100, 100);
        assert_eq!(on[3], 255, "the box never reached the canvas: {on:?}");
        assert!(
            on[0] as i32 - on[1].max(on[2]) as i32 > 40,
            "the wrong colour drew: {on:?}"
        );
        // And it is the *whole* box: (199,100) is the other end of its top edge, so a
        // clip measured in the wrong space - which is what drew this rect at x = -1 -
        // cannot pass by landing one corner in the right place.
        let far = l.layer_of("left").unwrap().get(199, 100);
        assert_eq!(far[3], 255, "the box arrived in the wrong place: {far:?}");
        // The same pixel of the right canvas is the same desktop x=-540 - nowhere
        // near the box, and never painted.
        assert_eq!(l.layer_of("right").unwrap().get(100, 100), [0, 0, 0, 0]);
        for name in ["left", "right"] {
            assert_eq!(
                mismatched(&l, name).unwrap(),
                Vec::<String>::new(),
                "{name}"
            );
        }
    }

    #[test]
    fn the_screen_that_is_not_at_the_desktops_corner_keeps_its_ink() {
        // The pair above starts at x = -1280, so the *right* screen is the one whose own
        // corner is not the desktop's: `shift` hands it a non-zero offset, and the clip
        // that keeps ink inside the selection is compared against canvas-local pixels.
        // Measured, not inferred - this house got the hardware on 2026-10-08 (a second
        // monitor above and left of the primary, selection on the primary) and `--ink`
        // then read `0 opaque px compared` across the whole hole: the layer drew nothing
        // on the very screen the user was annotating, while the saved picture had the ink.
        let left = PhysRect::new(-1280, 0, 640, 320);
        let right = PhysRect::new(-640, 0, 640, 320);
        let grey = [90, 90, 90, 255];
        let mut l = screens(&[("left", left), ("right", right)], grey);
        l.set_hole(right);
        l.set_color([255, 0, 0, 255]);
        l.select_tool(code_of(Some(Kind::Rect)));

        // Desktop (-600,40) is local (40,40) of the right screen.
        assert!(l.press(PhysPoint::new(-600, 40)));
        l.drag(PhysPoint::new(-500, 140));
        assert!(l.release());
        assert_eq!(l.objects(), 1);

        let on = l.layer_of("right").unwrap().get(40, 40);
        assert_eq!(
            on[3], 255,
            "the selection's own screen drew nothing: {on:?}"
        );
        // The failure this pins is "what you save is not what you saw", so the export -
        // the picture the crop takes - has to have the same pixel, not merely the layer.
        assert_eq!(on, l.flatten("right").unwrap().get(40, 40));
        assert_eq!(
            mismatched(&l, "right").unwrap(),
            Vec::<String>::new(),
            "the layer is not the document"
        );
    }

    #[test]
    fn incremental_repaint_equals_a_full_render() {
        // Three different pens, an undo and a redo, and the layer still has to be the
        // picture `raster::render` produces from the same document - which is the one
        // claim a dirty-rect painter can quietly stop honouring.
        let one = PhysRect::new(0, 0, 320, 200);
        let mut l = screens(&[("m0", one)], [40, 60, 80, 255]);
        l.set_hole(one);
        l.set_width(6);

        l.select_tool(code_of(Some(Kind::Rect)));
        l.press(PhysPoint::new(20, 20));
        l.drag(PhysPoint::new(140, 90));
        assert!(l.release());

        l.select_tool(code_of(Some(Kind::Pencil)));
        l.press(PhysPoint::new(30, 150));
        for x in 30..200 {
            l.drag(PhysPoint::new(x, 150 + (x % 17)));
        }
        assert!(l.release());

        l.select_tool(code_of(Some(Kind::Arrow)));
        l.press(PhysPoint::new(200, 30));
        l.drag(PhysPoint::new(300, 170));
        assert!(l.release());

        assert_eq!(l.objects(), 3);
        assert_eq!(mismatched(&l, "m0").unwrap(), Vec::<String>::new());
        assert!(l.undo_step());
        assert_eq!(l.objects(), 2);
        assert_eq!(mismatched(&l, "m0").unwrap(), Vec::<String>::new());
        assert!(l.redo_step());
        assert_eq!(l.objects(), 3);
        assert_eq!(mismatched(&l, "m0").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn undo_puts_the_desktop_back() {
        // Not "the object count went down": the pixels. A ghost is a layer that is
        // correct as a document and wrong as a picture.
        let one = PhysRect::new(0, 0, 200, 200);
        let grey = [70, 70, 70, 255];
        let mut l = screens(&[("m0", one)], grey);
        l.set_hole(one);
        l.set_color([0, 200, 0, 255]);
        l.select_tool(code_of(Some(Kind::Rect)));
        l.press(PhysPoint::new(50, 50));
        l.drag(PhysPoint::new(120, 120));
        assert!(l.release());

        let inked = l.layer_of("m0").unwrap().get(50, 50);
        assert_ne!(inked, grey, "the box did not draw: {inked:?}");
        assert!(l.undo_step());
        assert_eq!(
            l.layer_of("m0").unwrap().get(50, 50),
            grey,
            "undo left a ghost behind"
        );
        // Inside the selection the layer is the composited picture, so it goes back to
        // the frozen grey rather than to transparent - and the export agrees with it.
        assert_eq!(mismatched(&l, "m0").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn ink_stops_at_the_selection_edge() {
        // A drag that starts inside and ends outside must stop at the edge, and the
        // pixels outside it must stay empty: the dim is QML's, and an ink pixel that
        // wanders out of the hole is an un-dimmed piece of desktop on screen.
        let one = PhysRect::new(0, 0, 400, 400);
        let hole = PhysRect::new(100, 100, 100, 100);
        let mut l = screens(&[("m0", one)], [20, 20, 20, 255]);
        l.set_hole(hole);
        l.select_tool(code_of(Some(Kind::Rect)));
        l.press(PhysPoint::new(120, 120));
        l.drag(PhysPoint::new(300, 300));
        assert!(l.release());

        // Clamped to the hole's bottom-right corner, so the border is on it and none of
        // the pixels beyond it are set.
        assert_ne!(l.layer_of("m0").unwrap().get(199, 199), [0, 0, 0, 0]);
        for y in 200..400 {
            for x in (200..400).step_by(7) {
                assert_eq!(l.layer_of("m0").unwrap().get(x, y), [0, 0, 0, 0]);
            }
        }
        for x in 0..100 {
            assert_eq!(l.layer_of("m0").unwrap().get(x, 150), [0, 0, 0, 0]);
        }
        assert_eq!(mismatched(&l, "m0").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn every_tool_keeps_its_own_pen() {
        // §5.7.1: 工具切换时记忆最近一次使用的样式. The memory is the reason the
        // toolbar reads its ticks back off the layer instead of holding them.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 64, 64))], [0, 0, 0, 255]);
        let rect = code_of(Some(Kind::Rect));
        let pencil = code_of(Some(Kind::Pencil));

        l.select_tool(rect);
        l.set_width(9);
        l.set_dashed(true);
        l.set_filled(true);
        assert!(l.dashed());
        assert!(l.filled());
        assert_eq!(l.width(), 9);

        l.select_tool(pencil);
        // A tool nobody has used yet starts from the pen in hand - including its dash
        // and its fill, which is what "remembers" has to mean to be predictable.
        assert_eq!(l.width(), 9);
        assert!(l.dashed());
        l.set_width(20);
        assert_eq!(l.style.brush.size, 80, "the brush did not follow the width");
        l.set_dashed(false);
        l.set_filled(false);

        l.select_tool(rect);
        assert_eq!(l.width(), 9);
        assert!(l.dashed());
        assert!(l.filled());
    }

    #[test]
    fn a_fresh_highlighter_is_half_transparent_and_arriving_is_not_teaching() {
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        let rect = code_of(Some(Kind::Rect));
        let marker = code_of(Some(Kind::Marker));

        l.select_tool(rect);
        l.set_color([255, 0, 0, 255]);
        l.select_tool(marker);
        assert_eq!(
            l.color(),
            [255, 0, 0, MARKER_ALPHA],
            "§5.7.8: 默认使用半透明颜色"
        );
        assert_eq!(l.alpha_percent(), 50);
        assert_eq!(l.style.width, 3, "the pen the user was holding is still the pen");
        // §5.7.1's memory is of what the user set. Picking a tool up is not setting it,
        // so the pen the layer just inferred for itself must not become a table row —
        // while the rect the user explicitly coloured stays remembered, because that
        // one *was* a setting and forgetting it would be the other kind of surprise.
        let rows = l.tool_styles();
        assert!(
            !rows.contains_key("marker"),
            "arriving at 荧光笔 is not teaching it: {rows:?}"
        );
        assert_eq!(rows["rect"].color.as_deref(), Some("#FF0000FF"));
        assert_eq!(rows["rect"].width, Some(3));

        l.select_tool(rect);
        assert_eq!(l.color(), [255, 0, 0, 255], "the rect kept its opaque red");
    }

    #[test]
    fn the_wheel_over_the_chip_moves_the_pen_and_the_file_carries_it() {
        // §5.7.21 step 3 and §9.2 on one line: the input, the pen it moves, the row it is
        // written under, and a layer that has never seen the tool reading the row back.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.select_tool(code_of(Some(Kind::Rect)));
        l.set_color([255, 0, 0, 255]);

        l.step_alpha(-1);
        assert_eq!(l.alpha_percent(), 90);
        assert_eq!(i32::from(l.color()[3]), alpha_from_pct(90) as i32);
        l.step_alpha(-5);
        assert_eq!(l.alpha_percent(), 40, "one rung per notch, not one byte");
        l.step_alpha(-10);
        assert_eq!(
            l.alpha_percent(),
            ALPHA_MIN_PCT as i32,
            "the floor is a pale pen, not ink nobody can see"
        );
        l.step_alpha(100);
        assert_eq!(l.alpha_percent(), 100);

        // 填充 follows the pen instead of staying where the switch was flipped.
        l.set_alpha_percent(50);
        l.set_filled(true);
        assert_eq!(i32::from(l.style.fill.unwrap()[3]), 128 * 3 / 10);
        l.set_alpha_percent(100);
        assert_eq!(
            i32::from(l.style.fill.unwrap()[3]),
            255 * 3 / 10,
            "the fill moved with the transparency it was made from"
        );

        let mut cfg = Config::default();
        cfg.annotation.tool_style = l.tool_styles();
        assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());
        assert_eq!(
            cfg.annotation.tool_style["rect"].color.as_deref(),
            Some("#FF0000FF"),
            "the alpha leaves in the colour, which is the row the file already has"
        );

        let mut fresh = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        fresh.apply_tool_styles(&cfg.annotation.tool_style);
        fresh.select_tool(code_of(Some(Kind::Rect)));
        assert_eq!(fresh.color(), [255, 0, 0, 255], "§9.2: it comes back");
        fresh.select_tool(code_of(Some(Kind::Marker)));
        assert_eq!(
            fresh.alpha_percent(),
            50,
            "and a tool the file says nothing about still opens half transparent"
        );
    }

    #[test]
    fn a_strength_one_tool_learns_does_not_walk_onto_the_other() {
        // §5.7.1's "记忆该工具最近一次使用的样式" and §5.7.10's defect, which are the
        // same sentence: 马赛克 and 模糊 put their number in one slot, so the only thing
        // that keeps a tuned radius off a mosaic is remembering them apart.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        let blur = code_of(Some(Kind::Blur));
        let mosaic = code_of(Some(Kind::Mosaic));

        l.select_tool(blur);
        assert!(l.effect_tool());
        assert_eq!(l.effect_caption(), "模糊强度");
        assert_eq!(l.effect_intensity(), 12, "it opens on the file's own number");
        l.set_effect_intensity(30);
        assert_eq!(l.style.effect, Effect::Blur { radius: 30 }, "…as a radius");

        l.select_tool(mosaic);
        assert_eq!(l.effect_caption(), "块大小");
        assert_eq!(
            l.effect_intensity(),
            8,
            "30 did not follow it out of 模糊: the row is the tool's, not the pen's"
        );
        assert_eq!(l.style.effect, Effect::Mosaic { block: 8 });
        l.set_effect_intensity(16);
        assert_eq!(l.style.effect, Effect::Mosaic { block: 16 });

        l.select_tool(blur);
        assert_eq!(l.effect_intensity(), 30, "the blur's own lesson came back");

        let table = l.tool_styles();
        assert_eq!(table["blur"].intensity, Some(30));
        assert_eq!(table["mosaic"].intensity, Some(16));

        // A tool with no strength is offered no row and sets none, so the file never
        // gains a number nothing reads.
        l.select_tool(code_of(Some(Kind::Rect)));
        assert!(!l.effect_tool());
        assert_eq!(l.effect_caption(), "");
        l.set_effect_intensity(50);
        assert!(
            l.tool_styles().get("rect").is_none(),
            "a call no row can make taught the rect a pen"
        );
    }

    #[test]
    fn what_the_blur_tool_draws_is_an_element_the_blur_made() {
        // The chain end to end: the toolbar's pen, the element the gesture commits, and
        // the picture the rasteriser turns out. Before this, 模糊 left every element
        // wearing a mosaic and the screen came out blocky.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.set_hole(PhysRect::new(0, 0, 100, 100));
        l.select_tool(code_of(Some(Kind::Blur)));
        assert!(l.press(PhysPoint::new(20, 20)));
        l.drag(PhysPoint::new(60, 60));
        assert!(l.release());
        let e = &l.doc.elements[0];
        assert_eq!(e.kind, Kind::Blur);
        assert_eq!(e.style.effect, Effect::Blur { radius: 12 });
        // §5.7.10 step 2's other two knobs are the pen's own: the width the toolbar sets
        // is the brush these tools drag, so 笔刷大小 needs no second control.
        l.set_width(9);
        assert_eq!(l.style.brush.size, 36, "the brush followed the pen");
        assert!(
            matches!(l.doc.elements[0].style.effect, Effect::Blur { .. }),
            "the committed object kept the effect it was drawn with: {:?}",
            l.doc.elements[0].style.effect
        );
    }

    #[test]
    fn the_blur_opens_on_the_number_the_file_gave_it() {
        // `[annotation] blur_radius` has been in the file since the file was and has never
        // had a reader. Wiring it as 模糊's *fallback* - not as a global that overwrites
        // what the user taught - is the order §5.7.1's memory is written in.
        let ann = annotated(|a| a.blur_radius = 24);
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.apply_annotation_defaults(&ann);
        l.select_tool(code_of(Some(Kind::Blur)));
        assert_eq!(
            l.style.effect,
            Effect::Blur { radius: 24 },
            "the row the file wrote reaches the tool it names"
        );
        l.select_tool(code_of(Some(Kind::Mosaic)));
        assert_eq!(
            l.style.effect,
            Effect::Mosaic { block: 8 },
            "one key cannot be both tools' number, so it is only 模糊's"
        );

        l.select_tool(code_of(Some(Kind::Blur)));
        l.set_effect_intensity(9);
        let mut fresh = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        fresh.apply_annotation_defaults(&ann);
        fresh.apply_tool_styles(&l.tool_styles());
        fresh.select_tool(code_of(Some(Kind::Blur)));
        assert_eq!(fresh.effect_intensity(), 9, "the lesson outranks the row");
    }

    #[test]
    fn the_shape_of_the_brush_belongs_to_the_tool_that_picked_it() {
        // §5.7.10 step 2's 形状 与 §5.7.8 规则那句「支持圆形或椭圆形笔刷」：raster 三档都画，
        // 所以这一枚钮有东西可挪 —— 而它挪的是**当下拖笔的那支工具**自己的一档。
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        let pencil = code_of(Some(Kind::Pencil));
        l.select_tool(pencil);
        assert!(l.brush_tool());
        assert_eq!(l.brush_shape_code(), 0, "圆头是出厂那一档");
        l.set_brush_shape(1);
        assert_eq!(l.style.brush.shape, Shape::Rect);
        assert_eq!(l.brush_shape_code(), 1, "and the readback is the pen, not the click");
        // 第三档：§5.7.8 那点名的椭圆。它过的是同一座桥、同一张表，所以这里量的是「第三枚序号
        // 落到了第三枚笔头」，而不是又一趟笔刷形状的记忆。
        l.set_brush_shape(2);
        assert_eq!(l.style.brush.shape, Shape::Ellipse);
        assert_eq!(l.brush_shape_code(), 2);
        // 出格的那一枚**不动笔**：三档是 `BRUSH_SHAPES` 的数也是界面上那三枚钮的数，一枚第四档
        // 悄悄画成椭圆是一枚说自己改了而没改的钮。
        l.set_brush_shape(3);
        assert_eq!(l.brush_shape_code(), 2, "第四枚序号没有第四支笔");
        l.set_brush_shape(-1);
        assert_eq!(l.brush_shape_code(), 2, "负数同样不落笔");

        l.select_tool(code_of(Some(Kind::Rect)));
        assert!(!l.brush_tool(), "矩形 drags no brush, so the row is off");
        l.set_brush_shape(0);
        assert!(
            l.tool_styles().get("rect").is_none(),
            "and the call taught it nothing"
        );
        l.select_tool(pencil);
        assert_eq!(l.style.brush.shape, Shape::Ellipse, "the pencil kept its own");
    }

    #[test]
    fn a_click_that_drew_nothing_is_not_undoable() {
        // A box with no size and a path with one point are both "the user misclicked".
        // Turning either into an undo step would make Ctrl+Z look like it did nothing -
        // which is exactly when a user stops trusting it.
        let one = PhysRect::new(0, 0, 100, 100);
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);

        l.select_tool(code_of(Some(Kind::Rect)));
        assert!(l.press(PhysPoint::new(40, 40)));
        assert!(!l.release(), "a dot of a box drew nothing");
        assert_eq!(l.objects(), 0);
        assert!(!l.can_undo());

        l.select_tool(code_of(Some(Kind::Pencil)));
        assert!(l.press(PhysPoint::new(50, 50)));
        assert!(!l.release(), "a one-point path drew nothing");
        assert_eq!(l.objects(), 0);
        assert!(!l.can_undo());
        // And the abandoned drafts left no pixels either.
        assert_eq!(mismatched(&l, "m0").unwrap(), Vec::<String>::new());
        assert_eq!(l.layer_of("m0").unwrap().get(50, 50), [0, 0, 0, 0]);
    }

    #[test]
    fn the_arrow_tool_hands_the_pointer_back() {
        // The routing rule the mask depends on: a drawing tool takes the press, and
        // `TOOLS[0]` - no tool at all - does not, so the selection's grips still work.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.set_hole(PhysRect::new(0, 0, 100, 100));
        assert!(
            !l.press(PhysPoint::new(10, 10)),
            "the arrow tool must not draw"
        );
        assert!(!l.dragging());

        l.select_tool(code_of(Some(Kind::Rect)));
        assert!(l.press(PhysPoint::new(10, 10)));
        assert!(l.dragging(), "a stroke in progress owns the pointer");
        l.drag(PhysPoint::new(30, 30));
        assert!(l.release());
        assert!(!l.dragging());
    }

    /// §5.7.16/§5.7.17 的现场：箭头工具、一整块选区，和三枚已经在那儿的东西。
    ///
    /// 对象是直加进文档的，不走 `press`/`drag`/`release` 那一趟 —— 这一档要量的是"选中
    /// 之后还能不能改"，画笔那一段上面已经有人量过了；而 `undo.depth()` 从 0 起算才说得出
    /// "整组一次改动就是一条"。第三枚排在 150 那一头，框选那条橡皮筋碰不到它，它就是
    /// "只有选中的那些跟着变" 的对照组。
    fn three_objects() -> Layer {
        let one = PhysRect::new(0, 0, 200, 200);
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);
        for (kind, x) in [(Kind::Rect, 10), (Kind::Ellipse, 70), (Kind::Rect, 150)] {
            l.doc.add(
                kind,
                Geom::Rect(PhysRect::new(x, 10, 40, 30)),
                Style::default(),
            );
        }
        l.paint(None);
        l
    }

    /// 框住前两枚，然后退出那一档：留着亮着的框选，后面那些"抓住这枚把手"的手势会一路
    /// 撞上橡皮筋。返回的就是这串里的那两枚。
    fn marquee_the_pair(l: &mut Layer) -> [u64; 2] {
        l.set_marquee(true);
        assert!(
            l.press(PhysPoint::new(5, 5)),
            "框选一档里，落点在哪都是这一档吃掉那枚按下"
        );
        l.drag(PhysPoint::new(120, 60));
        assert!(l.release());
        l.set_marquee(false);
        let ids = l.sel.ids().to_vec();
        assert_eq!(ids.len(), 2, "框住的两枚进串，框外那枚不进");
        [ids[0], ids[1]]
    }

    #[test]
    fn a_marquee_is_a_selection_and_not_an_edit() {
        // §5.7.17 步 2-3 在屏幕上第一次能按：一拖框住一片，松手就选中，而撤销栈上一个字
        // 都不多 —— "我框了一下"不是编辑，Ctrl+Z 该退的是上一笔。
        let mut l = three_objects();
        l.set_marquee(true);
        assert!(l.marquee_mode());
        assert!(l.press(PhysPoint::new(5, 5)));
        assert!(l.dragging(), "橡皮筋也是一趟在手里的拖拽，指针得归它");
        l.drag(PhysPoint::new(120, 60));
        assert_eq!(l.selected_count(), 0, "松手以前只有橡皮筋，还没选中东西");
        assert_eq!(l.draft.len(), 1, "橡皮筋借画笔那一格画，因为它不是一笔");
        assert!(l.release());
        assert_eq!(l.selected_count(), 2);
        assert!(l.draft.is_empty(), "松手时橡皮筋该收走，不该留下一框没人负责的边");
        assert_eq!(l.undo.depth(), 0, "框选不落命令");
        assert!(l.marquee_mode(), "那一枚钮是一档，不是一瞬间");

        // 按下去又松开、中间一次也没拖：谁也没框住，这一串保持原样。换成"空的串"就是
        // 一次单击把面板上正在描述的那组弄丢了。
        let before = l.sel.ids().to_vec();
        assert!(l.press(PhysPoint::new(5, 5)));
        assert!(l.release());
        assert_eq!(l.sel.ids(), before.as_slice(), "没框住任何东西的一按不换这一串");
    }

    #[test]
    fn the_knob_moves_the_whole_group_and_one_undo_takes_the_group_back() {
        // §5.7.16 步 3 与 §5.7.17 步 4 是同一条腿的两半：面板上那枚旋钮除了改笔，还要落到
        // 选中的那些对象上，而整组一次改动是一条可撤销的编辑、不是一组。
        let mut l = three_objects();
        let [a, b] = marquee_the_pair(&mut l);
        let third = l.doc.elements.last().unwrap().id;
        let before = l.doc.get(a).unwrap().style.clone();
        assert_eq!(l.undo.depth(), 0, "框选那趟没落命令");

        l.set_color([0, 120, 255, 255]);
        assert_eq!(l.undo.depth(), 1, "整组一次样式改动是一条，不是一组");
        for id in [a, b] {
            assert_eq!(l.doc.get(id).unwrap().style.color, [0, 120, 255, 255]);
        }
        assert_eq!(
            l.doc.get(third).unwrap().style.color, before.color,
            "没选中的那枚不该跟着换色"
        );
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "改色后的增量重画要等于整层重渲染"
        );

        // 再拧一次同一枚：笔已经在这一格上、整组也已经在，栈上不该多一条。
        l.set_color([0, 120, 255, 255]);
        assert_eq!(l.undo.depth(), 1, "拧到它已经显示的那一格不是编辑");

        assert!(l.undo_step());
        assert_eq!(l.doc.get(a).unwrap().style, before, "一次 Ctrl+Z 拿回整组");
        assert_eq!(l.doc.get(b).unwrap().style, before);
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "撤销后的增量重画也要等于整层重渲染"
        );
    }

    #[test]
    fn a_run_of_knobs_over_the_same_group_is_one_step_back() {
        // §5.7.20/§5.7.21 的"连着拧"那条合并窗口，套到选中的组上同样是一条：几枚旋钮挨着
        // 按下去，一次 Ctrl+Z 要回到它们都没动的那一刻，否则用户得按同样多的次数退回去。
        let mut l = three_objects();
        let [a, b] = marquee_the_pair(&mut l);
        let third = l.doc.elements.last().unwrap().id;
        let before = l.doc.get(a).unwrap().style.clone();

        l.set_color([0, 120, 255, 255]);
        l.set_width(9);
        l.set_alpha_percent(40);
        l.set_dashed(true);
        l.set_filled(true);
        l.set_font_size(24);
        l.set_align(1);
        l.set_text_bg(true);
        l.set_text_outline(true);
        // §5.7.11 那句规则的「颜色和宽度」两半：开关只管画不画，这两枚管那一圈边是什么色、
        // 多粗。它们与前面那几枚挨着按下去，一次 Ctrl+Z 要回到它们都没动的那一刻。
        l.set_outline_colour([0, 0, 255]);
        l.set_outline_width(6);

        assert_eq!(l.undo.depth(), 1, "同一组的连着改，栈上只有一条");
        for id in [a, b] {
            let s = &l.doc.get(id).unwrap().style;
            assert_eq!(pct_from_alpha(s.color[3]), 40, "透明度落在自己那一枚上");
            assert_eq!(s.width, 9);
            assert_eq!(s.brush.size, 36, "§5.7.20 的那一支笔刷跟着笔走，对象上同一条规矩");
            assert_eq!(s.dash, Dash::Dash(27, 18), "虚线两段是**那一枚对象自己的**线宽三倍与两倍");
            assert!(s.fill.is_some(), "填充");
            assert_eq!(s.font_size, 24, "字号");
            assert_eq!(s.align, Align::Center, "对齐");
            assert!(s.text_bg.is_some(), "背景");
            assert!(s.text_outline, "描边");
            // 那一圈边自己的 alpha 不被笔的 40% 抢走（`Style::set_outline_colour` 那句分工）。
            assert_eq!(s.outline_color, Some([0, 0, 255, 255]), "描边颜色");
            assert_eq!(s.outline_width, 6, "描边宽度");
        }
        assert_eq!(
            l.doc.get(third).unwrap().style, before,
            "框外那枚一样也没被动过"
        );

        assert!(l.undo_step());
        assert_eq!(l.doc.get(a).unwrap().style, before, "一条退回到十一枚旋钮都没动");
        assert_eq!(l.doc.get(b).unwrap().style, before);
    }

    #[test]
    fn deleting_the_group_is_one_step_and_the_gone_ids_stop_answering() {
        // §5.7.17 步 4 的删除，与 §5.7.19 的"删除是一条可撤销的编辑"。删完必须清空那一串：
        // core 那几位命令作者对**陌生 id** 的答案是整条不建，留着旧 id 的面板从此悄悄不做
        // 任何事，而那正是"按钮亮着而没有后端"那一族的另一种长相。
        let mut l = three_objects();
        let [a, b] = marquee_the_pair(&mut l);
        let third = l.doc.elements.last().unwrap().id;
        assert!(l.delete_selected());
        assert_eq!(l.objects(), 1, "框外那枚留下，框住的两枚一起走");
        assert_eq!(l.selected_count(), 0, "已经不在场的 id 不该还报『被选中』");
        assert!(!l.delete_selected(), "空的那一串无事可做");

        let depth = l.undo.depth();
        l.set_color([0, 0, 0, 255]);
        assert_eq!(l.undo.depth(), depth, "一串空成员改不出命令，也就不该多一条");

        assert!(l.undo_step());
        assert_eq!(l.objects(), 3, "一次 Ctrl+Z 拿回整组，连样式与次序一起");
        let order: Vec<u64> = l.doc.paint_order().into_iter().map(|e| e.id).collect();
        assert_eq!(order, vec![a, b, third], "快照按文档自己的次序存的，撤销就按那次序还回来");
        assert_eq!(l.doc.get(a).unwrap().style, Style::default());
    }

    #[test]
    fn a_clone_lands_beside_the_original_and_becomes_the_selection() {
        // §5.7.18 步 2-4：面板那枚复制钮把选中的每一枚各留一份，偏 CLONE_STEP 落在原地旁边
        // （叠在正上方，用户会以为这一枚钮没生效），而**副本接着成为这一串** —— 第 4 步
        // "用户移动副本到目标位置"要握的是刚生成的那几枚，不是留在原处的那几枚。
        let mut l = three_objects();
        // 按在**画出来的那一条边**上：没灌满的矩形里，中间那片不是它的目标（
        // [`falcon_core::annotation::model::Element::hit`] 问的是 `near`），点正中间是"什么
        // 也没点着"，而那正是 §5.7.16 步 4 的完成编辑。
        assert!(l.press(PhysPoint::new(10, 20)), "箭头工具点到谁就选中谁");
        assert!(!l.marquee_mode(), "单击是选中一枚，不该顺手开框选那一档");
        assert!(!l.release(), "按下去一次也没拖 ⇒ 栈上没有『我点了一下』");
        assert_eq!(l.undo.depth(), 0);

        let original = l.sel.ids()[0];
        let from = l.doc.get(original).unwrap().geom.bounds();
        assert!(l.clone_selected());
        assert_eq!(l.objects(), 4);
        assert_eq!(l.selected_count(), 1, "选中的是那一份副本");
        let copy = l.sel.ids()[0];
        assert_ne!(copy, original, "新 id 由命令自己带着，这里不猜它们叫什么");
        let to = l.doc.get(copy).unwrap().geom.bounds();
        assert_eq!((to.x, to.y), (from.x + CLONE_STEP, from.y + CLONE_STEP));
        assert_eq!(
            l.doc.get(copy).unwrap().kind,
            l.doc.get(original).unwrap().kind,
            "副本是那枚对象的样子，不是笔的样子"
        );

        assert!(l.undo_step(), "一次 Ctrl+Z 收回整批副本，而不是逐枚");
        assert_eq!(l.objects(), 3);
        assert!(l.doc.get(copy).is_none());
        assert_eq!(l.selected_count(), 0, "副本走了，那一串里也就没有它");
    }

    /// §5.7.18 步 2 的第二扇门（第一扇是上面那一枚克隆钮）：按住 Ctrl 拖。
    ///
    /// 按的那一点取 **(20,10)** 而不是 40×30 那只框的左边缘中点：八枚把手住在 GRIP_SLOP
    /// 那一圈里，`(10,25)` 那一枚离 `(10,20)` 只有五个像素，按下去先撞上把手那一臂，
    /// 量的就不是克隆了。
    #[test]
    fn a_ctrl_drag_lands_a_copy_where_the_pointer_stops_and_files_one_step() {
        let mut l = three_objects();
        l.hold_clone(true);
        assert!(l.press(PhysPoint::new(20, 10)), "按着 Ctrl 点到谁就克隆谁");
        let original = l.sel.ids()[0];
        let from = l.doc.get(original).unwrap().geom.bounds();
        assert_eq!(l.objects(), 4, "副本在按下那一刻就摆进了文档（摆在与原处同一格）");
        assert_eq!(
            l.doc.get(original).unwrap().geom.bounds(),
            from,
            "原物不该跟着被拖走"
        );
        assert_eq!(
            l.sel.ids(),
            [original],
            "手势里选中说的还是原物：副本此刻是预览"
        );

        l.drag(PhysPoint::new(40, 30));
        l.drag(PhysPoint::new(60, 50));
        assert_eq!(l.objects(), 4, "预览不落新的副本：一趟手势只有一份");
        assert!(l.release());

        assert_eq!(l.undo.depth(), 1, "整趟 Ctrl 拖在栈上是一条，不是添加加移动");
        assert_eq!(l.objects(), 4);
        let copy = l.sel.ids()[0];
        assert_ne!(copy, original, "步 4「用户移动副本到目标位置」接着要握的是副本");
        let to = l.doc.get(copy).unwrap().geom.bounds();
        assert_eq!((to.x, to.y), (from.x + 40, from.y + 40), "副本停在指针拖到的那一格");
        assert_eq!(l.doc.get(original).unwrap().geom.bounds(), from, "松手时原物还不动");
        assert_eq!(
            l.doc.get(copy).unwrap().kind,
            l.doc.get(original).unwrap().kind,
            "副本是那枚对象的样子，不是笔的样子"
        );
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "预览走过的每一块都要有人收：原处与落点都该是重渲染出来的样子"
        );

        assert!(l.undo_step());
        assert_eq!(l.objects(), 3, "一次 Ctrl+Z 收回整批副本");
        assert!(l.doc.get(copy).is_none());
        assert_eq!(l.doc.get(original).unwrap().geom.bounds(), from);
        assert!(l.redo_step());
        assert_eq!(
            l.doc.get(copy).unwrap().geom.bounds(),
            to,
            "重做落回的是拖到的那一格，不是原处"
        );
    }

    #[test]
    fn a_ctrl_click_that_never_dragged_files_nothing_and_leaves_no_second_object() {
        let mut l = three_objects();
        l.hold_clone(true);
        assert!(l.press(PhysPoint::new(20, 10)));
        let original = l.sel.ids()[0];
        assert_eq!(l.objects(), 4, "按下那一刻副本还是摆进了文档，它是这一趟的预览");
        assert!(!l.release(), "一次也没拖 ⇒ 这一趟什么也没交");
        assert_eq!(
            l.objects(),
            3,
            "副本跟着预览一起走：一次 Ctrl+单击不在屏幕上留下第二枚对象"
        );
        assert_eq!(l.undo.depth(), 0, "栈上也不留一条『我按了一下』");
        assert_eq!(l.sel.ids(), [original], "没交出去的克隆不该把选中换走");
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "摘掉的副本画过的那一块要有人收回去"
        );

        // 那一枚修饰键已经被上面那枚按下**用掉**了：同一枚对象再拖一次、没再递 Ctrl，
        // 走的就是移动原物那一臂。「上一次按着 Ctrl」不是这一次按下的理由。
        let from = l.doc.get(original).unwrap().geom.bounds();
        assert!(l.press(PhysPoint::new(20, 10)));
        l.drag(PhysPoint::new(40, 30));
        assert!(l.release());
        assert_eq!(l.objects(), 3, "没按 Ctrl 的那一拖不多出第二枚");
        assert_eq!(
            l.doc.get(original).unwrap().geom.bounds(),
            PhysRect::new(from.x + 20, from.y + 20, from.w, from.h),
            "那一拖挪的是原物"
        );
    }

    #[test]
    fn a_ctrl_drag_over_a_group_clones_every_member_as_one_step() {
        let mut l = three_objects();
        let [a, b] = marquee_the_pair(&mut l);
        let a_from = l.doc.get(a).unwrap().geom.bounds();
        let b_from = l.doc.get(b).unwrap().geom.bounds();

        l.hold_clone(true);
        assert!(
            l.press(PhysPoint::new(20, 10)),
            "拖一串里的一员是克隆整组，不是把组缩成一枚"
        );
        assert_eq!(l.objects(), 5, "串里的每一枚各留一份");
        l.drag(PhysPoint::new(30, 20));
        assert!(l.release());
        assert_eq!(l.undo.depth(), 1, "整组一趟克隆是一条命令，不是两枚");
        assert_eq!(l.selected_count(), 2, "两枚副本一起成为这一串");
        assert_eq!(l.doc.get(a).unwrap().geom.bounds(), a_from, "两枚原物都不动");
        assert_eq!(l.doc.get(b).unwrap().geom.bounds(), b_from);
        let copies: Vec<(i32, i32)> = l
            .sel
            .ids()
            .iter()
            .map(|id| {
                let r = l.doc.get(*id).unwrap().geom.bounds();
                (r.x, r.y)
            })
            .collect();
        assert_eq!(
            copies,
            vec![(a_from.x + 10, a_from.y + 10), (b_from.x + 10, b_from.y + 10)],
            "整组走的是同一个位移：两枚副本之间的相对位置一格没变"
        );

        assert!(l.undo_step());
        assert_eq!(l.objects(), 3, "一次 Ctrl+Z 收回整批，而不是先收一枚");
    }

    #[test]
    fn ctrl_on_a_grip_resizes_the_one_it_grabbed_and_invents_no_copy() {
        // 「按住 Ctrl 拖动」说的是**拖这一串**，不是「拖把手时顺便复制一枚」：握住把手那一臂
        // 在克隆那一臂之前问，屏幕上发生的还是 §5.7.3 的改尺寸。两件事一次交出，用户按的是
        // 一枚键而看见的是两件改动。
        let mut l = three_objects();
        assert!(l.press(PhysPoint::new(20, 10)));
        let id = l.sel.ids()[0];
        let from = l.doc.get(id).unwrap().geom.bounds();
        assert!(!l.release());

        l.hold_clone(true);
        let at = PhysPoint::new(50, 40); // 右下那一枚角把手
        assert!(l.press(at), "握住把手那一臂先问");
        l.drag(PhysPoint::new(70, 60));
        assert!(l.release());
        assert_eq!(l.objects(), 3, "按着 Ctrl 拖把手不多出一枚");
        assert_eq!(l.undo.depth(), 1, "那一趟改尺寸本身是一条");
        let r = l.doc.get(id).unwrap().geom.bounds();
        assert_eq!((r.x, r.y, r.w, r.h), (from.x, from.y, from.w + 20, from.h + 20));
    }

    #[test]
    fn dropping_a_clone_gesture_before_it_lands_takes_the_copies_back() {
        // 半路丢掉的一趟（换工具、开框选、选区被清空、点到空白）不许在文档里留下一枚没有来历
        // 的对象：它不在撤销栈上，Ctrl+Z 收不走，而用户从没要求过它。这里量开框选那一扇。
        let mut l = three_objects();
        assert!(l.press(PhysPoint::new(20, 10)));
        let original = l.sel.ids()[0];
        assert!(!l.release());

        l.hold_clone(true);
        assert!(l.press(PhysPoint::new(20, 10)));
        assert_eq!(l.objects(), 4);
        l.drag(PhysPoint::new(40, 30));
        assert_eq!(l.objects(), 4, "拖过之后副本还在文档里等着被交出去");

        l.set_marquee(true);
        assert_eq!(l.objects(), 3, "丢掉这一趟 = 连预览的副本一起丢掉");
        assert!(l.doc.get(original).is_some(), "摘掉的是副本，不是原物");
        assert_eq!(l.undo.depth(), 0, "没交出去的一趟不落栈");
        assert_eq!(l.sel.ids(), [original], "这一串说的还是原物");
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "副本拖过的那一块没人负责就会留像素"
        );
    }

    #[test]
    fn the_rotation_cell_turns_the_group_by_its_own_rung() {
        // §5.7.15 步 2 的后半边「或输入旋转角度」：QML 只递方向，格子的宽窄
        // （[`ROTATE_STEP_DEG`]）与两端（`clamp_rotation`）都是这里的数。读数取**最上面**
        // 那一枚，与面板报样式同一口径 —— 一组没对齐的角度里报底下那枚，就是描述一件屏幕
        // 上看不见的东西。
        let mut l = three_objects();
        let [a, b] = marquee_the_pair(&mut l);
        assert_eq!(l.rotation_degrees(), 0, "没转过的组读 0");
        assert!(l.step_rotation(1));
        assert_eq!(
            l.rotation_degrees(),
            ROTATE_STEP_DEG as i32,
            "一格是 Rust 的那一枚数，不是 QML 抄的"
        );
        for id in [a, b] {
            assert_ne!(
                l.doc.get(id).unwrap().transform.rotation,
                0.0,
                "整组都转了，不只是被读的那一枚"
            );
        }
        assert!(l.step_rotation(40));
        assert_eq!(
            l.rotation_degrees(),
            ROTATE_LIMIT as i32,
            "刻度盘的另一端由 clamp 守着，而不是转圈"
        );
        assert!(l.step_rotation(-200));
        assert_eq!(l.rotation_degrees(), -(ROTATE_LIMIT as i32));

        assert!(l.undo_step());
        assert_eq!(l.rotation_degrees(), ROTATE_LIMIT as i32, "一次 Ctrl+Z 退一格");
        assert!(!l.step_rotation(0), "转回它自己在的那一格不是编辑");
        assert_eq!(l.undo.depth(), 2, "转向是三条：样式那条合并窗口管不到它们");
    }

    #[test]
    fn a_line_takes_its_direction_from_its_endpoints_not_the_rotation_cell() {
        // §5.7.15 的规则原文：直线和箭头优先通过端点调整方向。框选照样把它们选进来（选中
        // 不是编辑），但那一枚转动的钮对一整组只会转的都没有时答案是 false，并且栈上不留
        // "我按了一下"。
        let one = PhysRect::new(0, 0, 200, 200);
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);
        let line = l.doc.add(
            Kind::Line,
            Geom::Segment {
                a: PhysPoint::new(20, 60),
                b: PhysPoint::new(120, 20),
            },
            Style::default(),
        );
        l.paint(None);

        l.set_marquee(true);
        assert!(l.press(PhysPoint::new(10, 10)));
        l.drag(PhysPoint::new(130, 70));
        assert!(l.release());
        l.set_marquee(false);
        assert_eq!(l.selected_count(), 1, "框选照样选中直线：被选中不等于可编辑");

        let depth = l.undo.depth();
        assert!(!l.step_rotation(1), "转不动的一组不该落下命令");
        assert_eq!(l.undo.depth(), depth);
        assert_eq!(l.doc.get(line.id).unwrap().transform.rotation, 0.0);
    }

    #[test]
    fn the_text_the_field_just_handed_over_is_what_the_text_row_moves() {
        // §5.7.11 步 5 与 §5.7.16 步 3 是同一段：回车之后紧接着要动那一行的字号、字体、
        // 对齐、背景与描边。那一排只在文本工具亮着时出现，而拿起任何一支笔都会把选中清空 ——
        // 没有"提交的那一枚就是当前对象"这一句，那五枚在屏幕上永远没有能落的东西。
        let mut l = typed("第一行");
        assert_eq!(l.selected_count(), 1, "刚提交的那一枚该是当前对象");
        let one = l.sel.ids()[0];

        // 那一枚窗口本身（400 ms）在 core 有自己的格子，那里把时间戳**递进去**，所以量的时候不必
        // 赌这台机器当时排到几号线程。这一档判的是「同一枚对象的连着几枚旋钮合进一条」，而一条满载的
        // 并行跑能让两次旋钮之间跨过四百毫秒 —— 那时候红的是时钟，不是那一合（#261：判据不许落在
        // 读数之外的那件事上）。所以这里先钉一次「产品起手的窗口就是 core 那一枚」，再把窗口递成
        // 「整趟都算同一次」，让那五枚旋钮只归因于它们自己。
        assert_eq!(
            l.undo.merge_window(),
            UndoStack::MERGE_MS,
            "本档改窗口之前，产品那一趟用的是 core 那一个作者的那个数"
        );
        l.undo = UndoStack::with_limits(UndoStack::CAP, u64::MAX);

        let depth = l.undo.depth();
        l.set_font_size(30);
        assert_eq!(l.undo.depth(), depth + 1);
        assert_eq!(l.doc.get(one).unwrap().style.font_size, 30);
        l.set_align(2);
        assert_eq!(l.doc.get(one).unwrap().style.align, Align::Right);
        // 字体那一枚是那一排唯一**一串字**，也就是那一排唯一没有芯片可点的一格；它落不落
        // 在同一枚刚提交的文本上，只能这样问一次。
        l.set_font_family("Consolas");
        assert_eq!(l.doc.get(one).unwrap().style.font_family, "Consolas");
        l.set_text_bg(true);
        assert!(l.doc.get(one).unwrap().style.text_bg.is_some());
        l.set_color([9, 9, 200, 255]);
        assert_eq!(l.doc.get(one).unwrap().style.color, [9, 9, 200, 255]);
        assert_eq!(l.undo.depth(), depth + 1, "同一枚对象的连着五枚旋钮还是一条");

        // 再点一下放第二行：上一枚不再是当前对象，旋钮从此只改笔。
        assert!(l.press(PhysPoint::new(140, 90)));
        assert_eq!(l.selected_count(), 0, "开始写第二行就不该再改着第一行");
        l.set_font_size(12);
        assert_eq!(
            l.doc.get(one).unwrap().style.font_size,
            30,
            "第二行的字号不该回头把第一行也拧小"
        );
        assert_eq!(l.undo.depth(), depth + 1);
    }

    #[test]
    fn what_no_longer_exists_leaves_the_panel() {
        // 撤销、重做与全清都会让对象消失。留着它们的读数是一枚亮着而不做事的钮：
        // `picked_count` 报「2 枚」，那一行就占着高度，而 core 对陌生 id 的答案是整条不建。
        let mut l = three_objects();
        marquee_the_pair(&mut l);
        assert_eq!(l.selected_count(), 2);
        assert!(l.clear_ink());
        assert_eq!(l.objects(), 0);
        assert_eq!(l.selected_count(), 0, "全清之后没有东西还算被选中");
        assert!(l.undo_step(), "全清本身是一条可撤销的编辑");
        assert_eq!(l.objects(), 3, "三枚都回来了");
        assert_eq!(l.selected_count(), 0, "回来不等于重新选中");
    }

    #[test]
    fn the_verbs_of_undo_redo_go_back_and_come_forward_in_order() {
        // §5.7.19 那一串动词，上面每一档各有人量过自己那一格的 undo；这一档把它们排成一串
        // 连续的动作，量的是深浅与画面**两路**：Ctrl+Z 一路退到底、Ctrl+Y 一路回到底。重做
        // 那一路是这一档的新东西 —— 产品里它就是 `CaptureMask.qml` 的 Ctrl+Y 与 Ctrl+Shift+Z，
        // 而 #5 那批判据只走到退回去为止，退回去之后 `redo_step` 画得对不对没人看过。
        let one = PhysRect::new(0, 0, 200, 200);
        let mut l = screens(&[("m0", one)], [70, 70, 70, 255]);
        l.set_hole(one);
        assert_eq!(
            l.undo.cap(),
            UndoStack::CAP,
            "遮罩这一场用的就是计划 §6.4 那一只 200 步的栈——「达到上限时优先保留最近操作」由它作证"
        );

        // ① 绘制。
        l.select_tool(code_of(Some(Kind::Rect)));
        l.press(PhysPoint::new(20, 20));
        l.drag(PhysPoint::new(140, 90));
        assert!(l.release());
        let id = l.doc.elements[0].id;

        // ② 移动：换到箭头那一档，单击它自己画出来的那条边（§5.7.16 步 2），拖一把再松手。
        l.select_tool(code_of(None));
        assert!(l.press(PhysPoint::new(20, 40)));
        assert_eq!(l.selected_count(), 1, "单击落在对象上就是选中它");
        l.drag(PhysPoint::new(50, 60));
        assert!(l.release());
        assert_eq!(l.undo.depth(), 2, "一整趟拖拽落一条，不是每一枚指针事件一条");

        // ③ 样式修改：选中着改颜色。
        l.set_color([0, 200, 0, 255]);
        assert_eq!(l.undo.depth(), 3);

        // ④ 删除：单删这一枚（不是全清）。
        assert!(l.delete_selected());
        assert_eq!(l.undo.depth(), 4);
        assert_eq!(l.objects(), 0);
        assert!(!l.can_redo(), "一次也没退回去过，重做那一路是空的");

        // 退到底：四站各退一枚，屏幕上始终是同一份文档的那张图。
        for expect in [1, 1, 1, 0] {
            assert!(l.undo_step());
            assert_eq!(l.objects(), expect);
            assert_eq!(mismatched(&l, "m0").unwrap(), Vec::<String>::new());
        }
        assert!(!l.can_undo());

        // 回到底：同一串命令按同一顺序重放，每一站各是自己那一个动词的样子。
        assert!(l.redo_step(), "① 绘制回来");
        assert_eq!(
            l.doc.get(id).unwrap().geom.bounds(),
            PhysRect::new(20, 20, 120, 70),
            "第一站是原样那一枚，还没挪过"
        );
        assert!(l.redo_step(), "② 移动回来");
        assert_eq!(l.doc.get(id).unwrap().geom.bounds(), PhysRect::new(50, 40, 120, 70));
        assert!(l.redo_step(), "③ 样式回来");
        assert_eq!(l.doc.get(id).unwrap().style.color, [0, 200, 0, 255]);
        assert_eq!(mismatched(&l, "m0").unwrap(), Vec::<String>::new());
        assert!(l.redo_step(), "④ 删除回来");
        assert_eq!(l.objects(), 0);
        assert!(!l.can_redo(), "回到顶就再没有可重做的");
        assert_eq!(mismatched(&l, "m0").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn moving_the_region_is_not_a_crop_someone_has_to_undo() {
        // §5.7.19「应覆盖绘制、移动、删除、样式修改和裁剪」里那个**裁剪**在产品落在哪一层，
        // 这一档把它说清 —— 因为这一格正是 `crop_command` 那位作者在仓库里零生产调用者的地方。
        //
        // 遮罩里用户能改的是**选区**本身（方向键带 `resize_edge`、拖框、§5.14.3 的回到原选区），
        // 而选区改动一件也不动文档：洞外的墨还留在 `doc` 里，把框拖回去它就重新出现，栈上也是
        // 一条都不多——「我改了框」不是编辑，与 [`Layer::finish_marquee`] 那句同一个理由。把
        // [`Layer::set_hole`] 接成一次 Crop 反倒会把这族非破坏性的改动变成一次删除，用户每挪一
        // 下框都得赌一把有没有 Ctrl+Z，而 §5.7.1 的「标注不应修改原始截图，直到用户确认导出」
        // 要的正是前者；真正交出去的那一刀在 [`Layer::deliver`]，它裁的是**帧**不是文档。
        //
        // 需要一次撤销拿回的那一刀是 §5.9.9 那张图：`Pin::crop` 把旧 `src_rect` 推进
        // `crop_undo`，`Pin::undo_crop` 与 `restore_original` 在 `pin_view.rs` 里接到
        // `PinWindow.qml` 菜单上那两行。文档级的那一位作者因此留在那里不接：命令名册是技术
        // 方案 §8.2 的那一张（`command.rs` 头上「Rotate 与 Reorder 也在，尽管 MVP 不放钮」
        // 已经立过同一条口径），它由 core 在 `undo.rs` 里那一档 crop 的整体往返与那台
        // proptest 量着，不是没人管的代码。
        let one = PhysRect::new(0, 0, 200, 200);
        let grey = [70, 70, 70, 255];
        let mut l = screens(&[("m0", one)], grey);
        l.set_hole(one);
        l.set_color([0, 200, 0, 255]);
        l.select_tool(code_of(Some(Kind::Rect)));
        l.press(PhysPoint::new(20, 20));
        l.drag(PhysPoint::new(140, 90));
        assert!(l.release());
        assert_eq!(l.undo.depth(), 1);
        assert_ne!(
            l.layer_of("m0").unwrap().get(20, 50),
            grey,
            "那条边该在屏上"
        );

        // 把框缩到左上角那一小块：墨从屏上没了，却没有从文档里没了。
        l.set_hole(PhysRect::new(0, 0, 10, 10));
        assert_eq!(l.objects(), 1, "缩框不删东西");
        assert_eq!(
            l.undo.depth(),
            1,
            "「我改了框」不落命令，Ctrl+Z 该退的还是那一笔"
        );
        assert_eq!(
            l.layer_of("m0").unwrap().get(20, 50),
            [0, 0, 0, 0],
            "洞外不留墨"
        );
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "缩框之后的增量层还是同一张图"
        );

        // 拖回去：那一笔完好如初，没有人替它做过决定。
        l.set_hole(one);
        assert_ne!(
            l.layer_of("m0").unwrap().get(20, 50),
            grey,
            "框回来了，墨也该跟着回来"
        );
        assert_eq!(l.undo.depth(), 1);
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "拖回去那一趟补的全canvas重画也不能留下一块旧像素"
        );
    }

    #[test]
    fn the_arrow_head_rung_lights_the_pen_and_the_arrow_it_draws() {
        // §5.7.6 步骤 2「选择单向、双向或其他箭头样式」里的那一半**其他样式**：三档第一次在
        // 屏幕上能按。单向与双向不在这一行里 —— 那是 `TOOLS` 上两枚工具，样式才是这一格。
        let one = PhysRect::new(0, 0, 200, 200);
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);
        l.select_tool(code_of(Some(Kind::Arrow)));
        assert!(l.arrow_tool(), "手里是箭头，那一行才占高度");
        assert_eq!(l.arrow_head_code(), 0, "出厂那一档是实心三角");

        l.set_arrow_head(2);
        assert_eq!(l.arrow_head_code(), 2);
        l.set_arrow_head(7);
        assert_eq!(l.arrow_head_code(), 2, "第四档不存在，笔不该被猜走");
        l.set_arrow_head(-1);
        assert_eq!(l.arrow_head_code(), 2, "负号也不是第四档");

        l.set_arrow_head(1);
        l.press(PhysPoint::new(20, 20));
        l.drag(PhysPoint::new(120, 80));
        assert!(l.release());
        let id = l.doc.elements[0].id;
        assert_eq!(l.doc.get(id).unwrap().style.arrow_head, ArrowHead::Chevron);

        l.select_tool(code_of(None));
        assert!(l.press(PhysPoint::new(70, 50)), "按在线段中点上就是选中它");
        assert_eq!(l.selected_count(), 1);
        let depth = l.undo.depth();
        l.set_arrow_head(0);
        assert_eq!(l.doc.get(id).unwrap().style.arrow_head, ArrowHead::Triangle);
        assert_eq!(l.undo.depth(), depth + 1, "样式改动是一条可撤销的编辑");
        assert!(l.undo_step());
        assert_eq!(
            l.doc.get(id).unwrap().style.arrow_head,
            ArrowHead::Chevron,
            "Ctrl+Z 退的是那一档，不是整枚箭头"
        );

        l.select_tool(code_of(Some(Kind::Rect)));
        l.select_tool(code_of(Some(Kind::Arrow)));
        assert_eq!(l.arrow_head_code(), 1, "箭头忘了它被教过的那一档");
    }

    #[test]
    fn the_corner_rung_moves_the_rounded_pen_between_two_ends() {
        // §5.7.3 步骤 2「配置圆角半径」：一格有多宽、地板与天花板各是多少都住在 `annotate`，
        // QML 那一排只递方向 —— 与透明度那把阶梯同一分工。
        let one = PhysRect::new(0, 0, 200, 200);
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);
        l.select_tool(code_of(Some(Kind::RoundedRect)));
        assert!(l.rounded_tool());
        assert_eq!(l.corner_radius(), 8, "出厂那一档读回来的是笔的，不是界面自己记的");

        l.step_corner_radius(1);
        assert_eq!(l.corner_radius(), 12);
        l.step_corner_radius(-1);
        assert_eq!(l.corner_radius(), 8, "一格下去就是原来那一格");

        for _ in 0..40 {
            l.step_corner_radius(1);
        }
        assert_eq!(l.corner_radius(), 64, "天花板：圆角不会长过矩形本身");
        for _ in 0..40 {
            l.step_corner_radius(-1);
        }
        assert_eq!(l.corner_radius(), 0, "地板那一档是直角，raster 画的正是这条式子");
        assert_eq!(
            l.undo.depth(),
            0,
            "笔上的一档不是编辑：那是下一笔的形状（§5.7.19）"
        );

        l.doc.add(
            Kind::RoundedRect,
            Geom::Rect(PhysRect::new(10, 10, 60, 40)),
            Style::default(),
        );
        l.paint(None);
        let id = l.doc.elements[0].id;
        l.select_tool(code_of(None));
        assert!(l.press(PhysPoint::new(10, 30)), "按在画出来的那条边上就是选中它");
        l.set_corner_radius(20);
        assert_eq!(l.doc.get(id).unwrap().style.corner_radius, 20);
        assert_eq!(l.undo.depth(), 1);
        assert!(l.undo_step());
        assert_eq!(
            l.doc.get(id).unwrap().style.corner_radius,
            8,
            "退回来的是这枚对象自己的出厂档"
        );
    }

    #[test]
    fn the_round_handle_drags_the_corner_and_one_undo_takes_the_corner_back() {
        // §5.7.3 步骤 4「通过控制点调整……圆角」的第一趟真手势：箭头工具下握住那一枚把手、拖、
        // 松手。改的是 `style.corner_radius` 而不是那一框的尺寸 —— `resize_grip` 对这一枚的答案
        // 是 `None`，所以这一趟走的是 core 里另一那位作者 `radius_grip_command`。
        let one = PhysRect::new(0, 0, 300, 300);
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);
        let mut style = Style::default();
        style.corner_radius = 12;
        l.doc.add(
            Kind::RoundedRect,
            Geom::Rect(PhysRect::new(20, 20, 200, 120)),
            style,
        );
        l.paint(None);
        let id = l.doc.elements[0].id;
        let geom_before = l.doc.get(id).unwrap().geom.clone();

        l.select_tool(code_of(None));
        assert!(
            l.press(PhysPoint::new(120, 20)),
            "按在画出来的那条边上就是选中它"
        );
        l.release();
        assert_eq!(l.selected_count(), 1);
        assert_eq!(l.undo.depth(), 0, "单击选中不是编辑");

        let at = l.radius_handle().expect("框里够大的 圆角矩形 该有那一枚把手");
        // 拖两枚事件而不是一枚：栈上那一条的 "before" 必须是**握住把手那一刻**的那一枚数。
        // 预览写的是 `style`，而 [`SelDrag::from`] 那份快照只按回几何与转向 —— 少一步按回，
        // 第二条事件算出的命令就把第一条的预览当成了原始值，一次 Ctrl+Z 退回一拖拖到一半的角。
        assert!(l.press(at), "那一枚把手该握得住");
        l.drag(PhysPoint::new(at.x + 20, at.y));
        l.drag(PhysPoint::new(at.x + 30, at.y));
        assert!(l.release());
        let dragged = l.doc.get(id).unwrap();
        assert_eq!(dragged.style.corner_radius, 42, "12 + 沿把手那根轴走的 30");
        assert_eq!(dragged.geom, geom_before, "拧角的那一趟把框本身也拖大了");
        assert_eq!(l.undo.depth(), 1, "一趟把手是一条编辑，不是一串预览");

        assert!(l.undo_step());
        assert_eq!(
            l.doc.get(id).unwrap().style.corner_radius,
            12,
            "Ctrl+Z 退回的不是握住把手那一刻的那一枚角"
        );
        assert!(l.redo_step());
        assert_eq!(l.doc.get(id).unwrap().style.corner_radius, 42);

        // 那一枚点只在**恰好一枚**对象时才递：两枚都在串里时页上只能画一个点，而那一个点讲不
        // 清说的是哪一只框 —— 这里选「不讲」而不是「讲错」。谁有把手、谁没有，是
        // [`Grip::grips_for`] 在 core 里已经答过的那一问，这一档只量递不递。
        let mut other = Style::default();
        other.corner_radius = 30;
        l.doc.add(
            Kind::RoundedRect,
            Geom::Rect(PhysRect::new(210, 20, 60, 60)),
            other,
        );
        l.paint(None);
        let second = l.doc.elements[1].id;
        l.sel = Selection::of(&[id, second]);
        assert_eq!(l.radius_handle(), None, "多选递出了一枚有主的把手");
        l.sel = Selection::of(&[second]);
        assert!(
            l.radius_handle().is_some(),
            "串里只剩那一枚 圆角矩形 时把手没递回来"
        );
    }

    #[test]
    fn the_constraint_key_squares_the_box_being_dragged() {
        // §5.7.4 步骤 3「按住约束键可创建正圆」：按着的那一段，拖出来的框是 1:1。那一枚键
        // 由界面在**当下这一枚鼠标事件**里递进来（`holdConstraint`），不是这里存的一把门，
        // 所以"按着就锁、松开就自由、起笔一律不锁"三件事各量一次。
        let one = PhysRect::new(0, 0, 300, 300);
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);
        l.select_tool(code_of(Some(Kind::Ellipse)));
        // 量的是拖出来的那一框，不是它的外接矩形：`bounds()` 会把描边的一半算进去，
        // 那是 raster 那一头的数，与这一档要说的"锁不锁"无关。
        let dragged = |l: &Layer, i: usize| match &l.doc.elements[i].geom {
            Geom::Rect(r) => *r,
            other => panic!("拖框的那三枚工具画的该是一框，不是 {other:?}"),
        };

        l.press(PhysPoint::new(20, 30));
        l.drag(PhysPoint::new(120, 90));
        assert!(l.release());
        assert_eq!(dragged(&l, 0), PhysRect::new(20, 30, 100, 60), "不按键时拖到哪儿就是哪儿");

        l.press(PhysPoint::new(20, 30));
        l.set_constraint(true);
        l.drag(PhysPoint::new(120, 90));
        assert!(l.release());
        assert_eq!(
            dragged(&l, 1),
            PhysRect::new(20, 30, 100, 100),
            "以长的那一边为准：椭圆于是就是正圆"
        );

        // 往左上拖同样锁得住：符号跟着指针走，锁不是只会在锚点的右下方拼正方形。
        l.press(PhysPoint::new(150, 150));
        l.set_constraint(true);
        l.drag(PhysPoint::new(50, 120));
        assert!(l.release());
        assert_eq!(dragged(&l, 2), PhysRect::new(50, 50, 100, 100));

        // 起笔自己归 false：上一笔松手时那颗键还按着，不算这一笔按下了的理由。
        l.press(PhysPoint::new(10, 10));
        l.drag(PhysPoint::new(60, 30));
        assert!(l.release());
        assert_eq!(dragged(&l, 3), PhysRect::new(10, 10, 50, 20), "新的一笔从不锁起");

        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "屏上那四枚不是文档里那四枚"
        );
    }

    #[test]
    fn one_click_lays_one_number_and_the_counter_walks_on() {
        // §5.7.12 步骤 3 与 4「在截图上依次点击 / 系统自动生成连续编号」：一次点击落一枚号。
        let one = PhysRect::new(0, 0, 300, 300);
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);
        l.select_tool(code_of(Some(Kind::Number)));
        assert!(l.number_tool(), "手里是编号，那一行才占高度");
        assert_eq!(l.counter_start(), 1, "出厂那一枚是 1，不是 0");

        // 一次"按下又立刻松手"，中间一个 drag 也没有：仍然要落一枚 —— 那是 step 3 说的
        // 一次点击，不该被"什么都没拖出来"那一格判成空手。
        assert!(l.press(PhysPoint::new(30, 30)));
        assert!(l.release());
        assert_eq!(l.objects(), 1);
        assert_eq!(l.doc.elements[0].number, Some(1));
        let first = match &l.doc.elements[0].geom {
            Geom::Rect(r) => *r,
            other => panic!("编号占的该是一框，不是 {other:?}"),
        };

        // 按着挪一挪：盒子跟到指针这儿，落的还是同一枚号。
        l.press(PhysPoint::new(90, 90));
        l.drag(PhysPoint::new(110, 110));
        assert!(l.release());
        assert_eq!(l.doc.elements[1].number, Some(2));
        assert_eq!(l.counter_start(), 3, "下一枚是 3：计数器跟着走");
        let second = match &l.doc.elements[1].geom {
            Geom::Rect(r) => *r,
            other => panic!("编号占的该是一框，不是 {other:?}"),
        };
        assert_eq!(second.x + (second.w as i32 / 2), 110, "落点在指针那一点上");

        // 撤销退的是那一枚号，不退计数器（core 在 `insert` 那一条上定的规矩：往回拖会得到
        // 一枚屏幕上已有第二个 2 的号）。
        let depth = l.undo.depth();
        assert!(l.undo_step());
        assert_eq!(l.objects(), 1);
        assert_eq!(l.undo.depth(), depth - 1);
        assert_eq!(l.counter_start(), 3, "退回去的那一枚不占号");

        // 起始编号那一排：一格一步，地板是 1。
        l.step_counter_start(5);
        assert_eq!(l.counter_start(), 8);
        for _ in 0..40 {
            l.step_counter_start(-1);
        }
        assert_eq!(l.counter_start(), 1, "地板之下没有第 0 枚");
        assert_eq!(l.undo.depth(), depth - 1, "起始编号不是一条可撤销的编辑");

        // 盒子按位数给宽：字号还是唯一那把尺，多出来的宽度是"这一串数字要占几格"。上面
        // 撤销掉了一枚，所以这一枚排在 index 1 上。
        l.set_counter_start(10);
        l.press(PhysPoint::new(200, 200));
        assert!(l.release());
        assert_eq!(l.objects(), 2);
        let wide = match &l.doc.elements[1].geom {
            Geom::Rect(r) => *r,
            other => panic!("编号占的该是一框，不是 {other:?}"),
        };
        assert!(wide.w > first.w, "两位数的一框比一位数窄，字就被折进行里了");
        assert_eq!(wide.h, first.h, "位数不改行高：那是字号的事");
        assert_eq!(l.doc.elements[1].number, Some(10));

        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "屏上那三枚不是文档里那三枚"
        );
    }

    #[test]
    fn a_bright_switch_makes_one_undo_take_back_the_delete_and_the_renumber() {
        // §5.7.12 步骤 5「删除中间编号时可选择是否自动重排」。判据是**一次** Ctrl+Z：被删的那
        // 一枚回到场上、被挪走的那几枚回到原位，两件事一起来 —— 分成两半的撤销会留下一个「号
        // 回来了、中间还缺一号」的时刻，那不是用户按过的那一格。
        let one = PhysRect::new(0, 0, 300, 300);
        let worn = |l: &Layer| -> Vec<Option<u32>> {
            l.doc.elements.iter().map(|e| e.number).collect()
        };
        let three = |l: &mut Layer| {
            for x in [30, 90, 150] {
                assert!(l.press(PhysPoint::new(x, x)));
                assert!(l.release());
            }
        };

        // 灭着那一臂是反证：这一档不亮的时候，屏幕上留着一个空档，而谁也没去动别人的号。
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);
        l.select_tool(code_of(Some(Kind::Number)));
        assert!(!l.auto_renumber(), "默认灭着：亮起来的那一次会挪动别人身上的号");
        three(&mut l);
        assert_eq!(worn(&l), vec![Some(1), Some(2), Some(3)]);
        l.select_tool(code_of(None));
        l.press(PhysPoint::new(90, 90));
        assert_eq!(l.selected_count(), 1, "单击落在中间那一枚上");
        assert!(l.delete_selected());
        assert_eq!(worn(&l), vec![Some(1), Some(3)], "灭着 ⇒ 空档留着");
        assert!(l.undo_step());
        assert_eq!(worn(&l), vec![Some(1), Some(2), Some(3)]);

        // 亮着那一臂：同一次点击，栈上多的是**一条**记录。
        let mut l = screens(&[("m0", one)], [0, 0, 0, 255]);
        l.set_hole(one);
        l.select_tool(code_of(Some(Kind::Number)));
        l.set_auto_renumber(true);
        assert!(l.auto_renumber());
        three(&mut l);
        l.select_tool(code_of(None));
        l.press(PhysPoint::new(90, 90));
        let depth = l.undo.depth();
        assert!(l.delete_selected());
        assert_eq!(l.undo.depth(), depth + 1, "删除与重排在栈上是一条，不是两条");
        assert_eq!(worn(&l), vec![Some(1), Some(2)], "3 号补上前面的空档");
        assert_eq!(l.counter_start(), 4, "重排不碰计数器：下一枚仍是没发出去的那个号");
        assert!(l.undo_step());
        assert_eq!(
            worn(&l),
            vec![Some(1), Some(2), Some(3)],
            "一次 Ctrl+Z 把删除与重排一起退回复"
        );
    }

    #[test]
    fn a_second_flow_still_remembers_the_tool_styles() {
        // §5.7.1's memory is per tool, and §9.2 asks it to survive a restart. The
        // restart half is the config's job; this is the half that is not: a second
        // capture *inside one process* must not forget what the first one learned.
        // `begin` and `end` both did `*self = Layer::default()`, which threw `styles`
        // away together with the canvases - so the second flow of a session opened
        // with the factory colour and the factory 3 px line.
        //
        // Driven through `end` because `begin` needs a `ScreenSnapshot`, which needs a
        // real capture; the two shared one reset, so this is the same line of code.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        let rect = code_of(Some(Kind::Rect));
        let ellipse = code_of(Some(Kind::Ellipse));
        l.select_tool(rect);
        l.set_color([1, 2, 3, 255]);
        l.set_width(7);
        l.select_tool(ellipse);
        l.set_color([9, 8, 7, 255]);
        l.set_auto_renumber(true);

        l.end();
        assert!(l.canvases.is_empty(), "the flow did not end");
        assert_eq!(l.objects(), 0);
        assert!(
            l.auto_renumber(),
            "开关记的是这个人要什么样的删除，不是这一张图上的一笔"
        );

        l.select_tool(rect);
        assert_eq!(l.color(), [1, 2, 3, 255], "the rect forgot its colour");
        assert_eq!(l.width(), 7, "the rect forgot its width");
        l.select_tool(ellipse);
        assert_eq!(l.color(), [9, 8, 7, 255], "the ellipse forgot its colour");
    }

    #[test]
    fn a_restarted_layer_reads_the_pens_the_file_remembered() {
        // PRD §9.2's restart is the config's job; this is the seam it hands the readings
        // over. A row of the table that cannot be honoured is said out loud rather than
        // quietly ignored, because the user has no other way to find out.
        let mut remembered = BTreeMap::new();
        remembered.insert(
            "rect".into(),
            ToolStyle {
                color: Some("#123456EF".into()),
                width: Some(9),
                intensity: None,
            },
        );
        remembered.insert(
            "arrow".into(),
            ToolStyle {
                color: None,
                width: Some(21),
                intensity: None,
            },
        );
        remembered.insert(
            "ellipse".into(),
            ToolStyle {
                color: Some("crimson".into()),
                width: None,
                intensity: None,
            },
        );
        remembered.insert(
            "rec".into(),
            ToolStyle {
                color: Some("#FFB900FF".into()),
                width: Some(4),
                intensity: None,
            },
        );

        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.select_tool(code_of(Some(Kind::Rect)));
        l.apply_tool_styles(&remembered);

        assert_eq!(
            l.color(),
            [0x12, 0x34, 0x56, 0xEF],
            "the tool on screen reads the pen the file gave it"
        );
        assert_eq!(l.width(), 9);
        l.select_tool(code_of(Some(Kind::Arrow)));
        assert_eq!(l.width(), 21);
        assert_eq!(
            l.color(),
            [232, 17, 35, 255],
            "a width alone is not a colour reset"
        );
        l.select_tool(code_of(Some(Kind::Ellipse)));
        assert_eq!(
            l.color(),
            [232, 17, 35, 255],
            "an unreadable colour teaches the tool nothing"
        );

        assert_eq!(l.problems.len(), 2, "{:?}", l.problems);
        assert!(
            l.problems.iter().any(|p| p.contains("\"rec\"")),
            "{:?}",
            l.problems
        );
        assert!(
            l.problems.iter().any(|p| p.contains("crimson")),
            "{:?}",
            l.problems
        );
    }

    #[test]
    fn what_the_layer_learns_comes_back_out_of_the_config() {
        // Both halves of §9.2 in one line: teach the layer, put its table through the
        // config's own validation, and read it into a layer that has never seen these
        // tools. No file is involved - the disk half waits for M5's config owner.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.select_tool(code_of(Some(Kind::Rect)));
        l.set_color([1, 2, 3, 240]);
        l.set_width(7);
        l.select_tool(code_of(Some(Kind::Marker)));
        l.set_color([9, 8, 7, 255]);
        l.set_width(40);

        let table = l.tool_styles();
        assert_eq!(table.len(), 2);
        assert_eq!(table["rect"].color.as_deref(), Some("#010203F0"));
        assert_eq!(table["rect"].width, Some(7));
        assert_eq!(table["marker"].color.as_deref(), Some("#090807FF"));

        let mut cfg = Config::default();
        cfg.annotation.tool_style = table;
        assert!(cfg.validate().is_empty(), "{:?}", cfg.annotation.tool_style);

        let mut fresh = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        fresh.apply_tool_styles(&cfg.annotation.tool_style);
        assert!(fresh.problems.is_empty(), "{:?}", fresh.problems);
        fresh.select_tool(code_of(Some(Kind::Rect)));
        assert_eq!(fresh.color(), [1, 2, 3, 240]);
        assert_eq!(fresh.width(), 7);
        fresh.select_tool(code_of(Some(Kind::Marker)));
        assert_eq!(fresh.color(), [9, 8, 7, 255]);
        assert_eq!(fresh.width(), 40);
    }

    /// One 折线 node, clicked rather than dragged: press, move to the same point, let
    /// go. §5.7.5's "依次点击添加节点" is a press and a release with nothing in between,
    /// and that is exactly the sequence no other tool in this module survives - the
    /// others commit on the release, this one must not.
    fn click(l: &mut Layer, at: (i32, i32)) {
        let p = PhysPoint::new(at.0, at.1);
        assert!(l.press(p), "the polyline tool did not take the click");
        l.drag(p);
        assert!(!l.release(), "a node is not a finished object");
    }

    /// A layer with the polyline tool in its hand and a grey screen to draw on.
    fn poly_layer() -> Layer {
        let one = PhysRect::new(0, 0, 200, 120);
        let mut l = screens(&[("m0", one)], [70, 70, 70, 255]);
        l.set_hole(one);
        l.set_color([0, 200, 0, 255]);
        l.set_width(6);
        l.select_tool(code_of(Some(Kind::Polyline)));
        l
    }

    /// How many pixels of one canvas are this layer's green. Stated as a count rather
    /// than as "the pixel at x,y is grey again", because a canvas nobody ever painted
    /// is transparent - which is the overlay saying "show the frame underneath", not a
    /// leftover stroke, and a point test cannot tell the two apart.
    fn inked(l: &Layer, name: &str) -> usize {
        let f = l.layer_of(name).unwrap();
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| f.get(x, y)))
            .filter(|p| p[3] != 0 && p[1] as i32 - p[0].max(p[2]) as i32 > 40)
            .count()
    }

    #[test]
    fn a_polyline_is_one_object_whatever_the_click_count() {
        let mut l = poly_layer();

        click(&mut l, (20, 20));
        assert_eq!(l.polyline_nodes(), 1);
        assert_eq!(l.objects(), 0, "the first click committed a line");

        click(&mut l, (100, 90));
        assert_eq!(l.polyline_nodes(), 2);
        // (60,55) is the midpoint of the only segment so far: a click that put nothing
        // on the canvas is a tool the user is aiming blind.
        let mid = l.layer_of("m0").unwrap().get(60, 55);
        assert_eq!(mid[3], 255, "the segment never reached the canvas: {mid:?}");
        assert!(
            mid[1] as i32 - mid[0].max(mid[2]) as i32 > 40,
            "the wrong ink drew: {mid:?}"
        );

        click(&mut l, (180, 20));
        assert_eq!(l.objects(), 0, "the finisher had not been called yet");
        assert!(l.finish_polyline(), "two clicks had nothing to finish");
        assert_eq!(l.objects(), 1);
        assert_eq!(l.polyline_nodes(), 0);
        assert!(!l.finish_polyline(), "an empty line is a second undo step");
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "the layer is not the document"
        );

        // One Ctrl+Z takes the whole polyline. Undoing only its last leg would leave a
        // line on screen that no keystroke can account for.
        assert!(l.undo_step());
        assert_eq!(l.objects(), 0);
        assert_eq!(
            l.layer_of("m0").unwrap().get(60, 55),
            [70, 70, 70, 255],
            "undo left a segment behind"
        );
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "the undo did not repaint"
        );
    }

    #[test]
    fn switching_tools_takes_an_unfinished_line_back_off_the_screen() {
        // The tool switch already abandoned half-dragged strokes; 折线 is the first tool
        // whose preview sits on the screen *between* strokes, so the pixels have to be
        // taken back with the nodes or the rect the user picks up next is drawn over a
        // line they let go of two clicks ago.
        let mut l = poly_layer();
        click(&mut l, (20, 20));
        click(&mut l, (100, 90));
        assert!(l.polyline_nodes() >= 2, "the clicks left no line waiting");

        l.select_tool(code_of(Some(Kind::Rect)));
        assert_eq!(l.polyline_nodes(), 0, "the switch kept the line in hand");
        assert_eq!(l.objects(), 0);
        assert_eq!(
            l.layer_of("m0").unwrap().get(60, 55),
            [70, 70, 70, 255],
            "the abandoned preview stayed on the canvas"
        );
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "an empty document drew pixels"
        );
    }

    #[test]
    fn a_cleared_selection_does_not_inherit_the_abandoned_line() {
        // 全清 and the first Esc both mean "nothing of mine is on this screen". The
        // preview is not in the document, so neither gesture can see it through
        // `Command::Remove` - and a pending polyline that survives an un-select comes
        // back the moment the user drags a new box.
        let mut l = poly_layer();
        click(&mut l, (20, 20));
        click(&mut l, (100, 90));
        assert!(
            !l.clear_ink(),
            "an empty document is not something 全清 removed"
        );
        assert_eq!(l.polyline_nodes(), 0, "全清 kept the line in hand");
        assert_eq!(l.layer_of("m0").unwrap().get(60, 55), [70, 70, 70, 255]);

        click(&mut l, (20, 20));
        click(&mut l, (100, 90));
        l.set_hole(PhysRect::default());
        assert_eq!(l.polyline_nodes(), 0, "un-selecting kept the line in hand");
        assert_eq!(l.layer_of("m0").unwrap().get(60, 55), [0, 0, 0, 0]);
        l.set_hole(PhysRect::new(0, 0, 200, 120));
        assert_eq!(
            inked(&l, "m0"),
            0,
            "the next selection inherited the line nobody was drawing"
        );
    }

    #[test]
    fn one_click_is_not_a_line() {
        let mut l = poly_layer();
        click(&mut l, (20, 20));
        click(&mut l, (20, 20));
        assert_eq!(l.polyline_nodes(), 1, "the same point clicked twice");
        assert!(!l.finish_polyline());
        assert_eq!(l.objects(), 0);
        assert!(
            !l.can_undo(),
            "a click that drew nothing became an undo step"
        );
        assert_eq!(inked(&l, "m0"), 0, "a single node drew a something");
    }

    // ------------------------------------------------------------ 局部放大 §5.7.13

    /// A layer with 放大 in hand on a screen big enough for a 2x copy to have somewhere
    /// to go. The base is one flat grey on purpose: over a desktop of a single colour the
    /// *only* pixels a magnifier adds are its frames and its connectors, so a non-zero
    /// [`inked`] here is the ink and never the copy's contents - and a non-zero count in
    /// the gauge, where the freeze is a real wallpaper, is the copy landing.
    fn zoom_layer() -> Layer {
        let one = PhysRect::new(0, 0, 400, 300);
        let mut l = screens(&[("m0", one)], [70, 70, 70, 255]);
        l.set_hole(one);
        l.set_color([0, 200, 0, 255]);
        l.set_width(6);
        l.select_tool(code_of(Some(Kind::Zoom)));
        l
    }

    /// §5.7.13's first drag. Its `false` return is part of the assertion: the source
    /// region is not the finished object, and the copy generated from it is still waiting
    /// to be placed - the shape 折线 established, not an exception invented for one tool.
    fn magnify(l: &mut Layer, a: (i32, i32), b: (i32, i32)) {
        let a = PhysPoint::new(a.0, a.1);
        let b = PhysPoint::new(b.0, b.1);
        assert!(l.press(a), "放大 would not take the drag");
        l.drag(b);
        assert!(!l.release(), "the first drag committed the magnifier");
    }

    #[test]
    fn two_drags_make_one_magnifier() {
        let mut l = zoom_layer();
        magnify(&mut l, (20, 20), (60, 60));
        let (from, placed) = l
            .zoom_pending()
            .expect("the first drag left no copy to place");
        assert_eq!(from, PhysRect::new(20, 20, 40, 40));
        assert_eq!(
            l.objects(),
            0,
            "the copy was committed before it was placed"
        );
        assert!(inked(&l, "m0") > 0, "the preview never reached the screen");

        // The second drag holds the copy from anywhere, and moves it by exactly the
        // pointer's travel. Re-anchoring the copy's corner to the press point is §9.1's
        // `resize`-`Body` mistake, on an object forty times the size of a grip.
        assert!(
            l.press(PhysPoint::new(10, 10)),
            "放大 let go of the second drag"
        );
        l.drag(PhysPoint::new(30, 45));
        let (still_from, moved) = l.zoom_pending().expect("the second drag lost the copy");
        assert_eq!(
            moved,
            placed.offset(20, 35),
            "the copy jumped instead of travelling"
        );
        assert_eq!(still_from, from, "the source region moved with the copy");

        assert!(l.release(), "the second drag did not finish the object");
        assert_eq!(l.objects(), 1, "two drags made more than one object");
        assert!(l.zoom_pending().is_none(), "the gesture stayed in hand");
        let Geom::Zoom {
            from: committed_from,
            to: committed_to,
        } = l.doc.elements[0].geom
        else {
            panic!("放大 committed a {:?}", l.doc.elements[0].kind);
        };
        assert_eq!(
            (committed_from, committed_to),
            (from, moved),
            "what joined the document is not what was on the screen"
        );
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "the layer is not the document"
        );

        // One Ctrl+Z takes both rectangles back. An undo that removed the copy and left
        // the source frame would leave a magnifier with nothing magnified in it.
        assert!(l.undo_step());
        assert_eq!(l.objects(), 0);
        assert_eq!(inked(&l, "m0"), 0, "undo left the frame behind");
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "the undo did not repaint"
        );
    }

    #[test]
    fn the_copy_is_generated_not_typed_but_capped_by_the_selection() {
        // §5.7.13 step 3 is arithmetic the user has to be able to predict, whatever the
        // step 5 cell is set to: `zoom_percent` of the source's own size, its corner a
        // pen's width beyond the source's bottom-right so the copy does not sit glued on
        // top of what it magnifies.
        let mut l = zoom_layer();
        magnify(&mut l, (20, 20), (60, 60));
        let (from, to) = l.zoom_pending().unwrap();
        assert_eq!((to.w, to.h), (80, 80), "200% of a 40x40 source");
        assert_eq!(
            (to.x - from.right(), to.y - from.bottom()),
            (6, 6),
            "the copy sits a pen's width beyond its source, not on top of it"
        );

        // The percent is clamped, and each side is then capped by the selection it is
        // clipped to: without the cap, an 8x of a 4K source is a 1.5 GB allocation inside
        // the rasteriser's `resized`, which is not a number this program should discover
        // on a user's machine.
        let mut l = zoom_layer();
        l.style.zoom_percent = 2000;
        magnify(&mut l, (20, 20), (60, 60));
        let (_, to) = l.zoom_pending().unwrap();
        assert_eq!(to.w, 320, "800% of 40, not 2000% of it");
        assert_eq!(
            to.h, 300,
            "the copy outgrew the selection it cannot draw outside"
        );
    }

    #[test]
    fn a_drag_that_magnified_nothing_is_not_undoable() {
        let mut l = zoom_layer();
        let p = PhysPoint::new(30, 30);
        assert!(l.press(p), "放大 would not take the click");
        l.drag(p);
        assert!(!l.release(), "a click with 放大 became an undo step");
        assert!(l.zoom_pending().is_none(), "a 0x0 source is a placement");
        assert!(!l.can_undo(), "nothing became undoable");
        assert_eq!(l.objects(), 0);
        assert_eq!(inked(&l, "m0"), 0, "an empty magnifier drew a something");

        // And the tool is not wedged: the next real drag works after the dead click.
        magnify(&mut l, (20, 20), (60, 60));
        assert!(
            l.zoom_pending().is_some(),
            "the click left 放大 unable to start again"
        );
    }

    #[test]
    fn every_way_out_of_a_pending_copy_takes_its_pixels_back() {
        // §9.1's ⑦ is a rule about *every* release path, and 放大 is the second tool with
        // a preview that lives on the screen between gestures. The judgement is
        // `inked() == 0`, not "the placement is gone": a preview Rust has forgotten but
        // never repainted away is still what the user is looking at.
        let mut l = zoom_layer();
        magnify(&mut l, (20, 20), (120, 100));
        assert!(
            inked(&l, "m0") > 0,
            "the tool switch has nothing to take back"
        );
        l.select_tool(code_of(Some(Kind::Rect)));
        assert!(
            l.zoom_pending().is_none(),
            "the switch kept the copy in hand"
        );
        assert_eq!(
            inked(&l, "m0"),
            0,
            "the tool switch left the magnifier on screen"
        );

        // 全清 with a document *and* a pending copy, planted where the committed object
        // cannot reach: the removal's dirty rects cover the object's own bounds only, so
        // the preview's pixels survive it unless the abandon is paid for with a repaint.
        // This is the run that fails without [`Layer::clear_ink`]'s `paint_last`, and it
        // is why the two are kept apart rather than one drawn over the other - P19 named
        // the rule, and its 全清 runs had an empty document, so nothing reached it.
        l.select_tool(code_of(Some(Kind::Zoom)));
        magnify(&mut l, (20, 20), (60, 40));
        assert!(
            l.press(PhysPoint::new(30, 30)),
            "放大 let go of the second drag"
        );
        l.drag(PhysPoint::new(40, 40));
        assert!(l.release(), "the first magnifier never committed");
        magnify(&mut l, (250, 150), (320, 200));
        assert_eq!(l.objects(), 1);
        assert!(
            inked(&l, "m0") > 0,
            "the pending copy never reached the screen"
        );
        assert!(l.clear_ink(), "全清 found nothing to remove");
        assert_eq!(l.objects(), 0);
        assert!(l.zoom_pending().is_none(), "全清 kept the copy in hand");
        assert_eq!(inked(&l, "m0"), 0, "全清 left the pending copy on screen");

        // Un-selecting, then selecting again: the new box must not inherit the old copy.
        magnify(&mut l, (20, 20), (100, 80));
        assert!(inked(&l, "m0") > 0);
        l.set_hole(PhysRect::default());
        assert!(
            l.zoom_pending().is_none(),
            "un-selecting kept the copy in hand"
        );
        l.set_hole(PhysRect::new(0, 0, 400, 300));
        assert_eq!(
            inked(&l, "m0"),
            0,
            "the next selection inherited the copy nobody was placing"
        );
    }

    #[test]
    fn enter_accepts_the_copy_where_the_default_put_it() {
        // A user who drew the source region and wants nothing to do with step 4 still has
        // to be able to get the magnifier: 双击 and `Enter` take the copy as placed.
        let mut l = zoom_layer();
        magnify(&mut l, (20, 20), (60, 60));

        // The ladder's other rung must not steal the keystroke it has nothing for, and
        // must not clear a preview it does not own: 折线's abandon used to wipe whatever
        // sat in `draft`, which was a second tool's pixels.
        assert!(
            !l.finish_polyline(),
            "折线 finished a line that was never clicked"
        );
        assert!(
            l.zoom_pending().is_some(),
            "an unrelated keystroke lost the copy"
        );
        assert!(
            inked(&l, "m0") > 0,
            "the keystroke took the preview off the screen"
        );

        assert!(l.finish_zoom(), "Enter declined a copy that was here");
        assert_eq!(l.objects(), 1);
        assert!(l.zoom_pending().is_none());
        assert!(
            !l.finish_zoom(),
            "an empty gesture became a second undo step"
        );
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "the layer is not the document"
        );

        // With another tool in hand the copy is not here to finish, whatever `Enter` is
        // otherwise asked to do - which is how the keystroke falls through to 完成选区.
        let mut l = zoom_layer();
        magnify(&mut l, (20, 20), (60, 60));
        l.select_tool(code_of(Some(Kind::Rect)));
        assert!(
            !l.finish_zoom(),
            "a tool that is not 放大 owns a pending copy"
        );
        let mut l = zoom_layer();
        assert!(!l.finish_zoom(), "a copy that was never drawn finished");
    }

    #[test]
    fn every_tool_button_names_the_tool_at_its_index() {
        // 放大 joins [`TOOLS`] *before* 橡皮, so every code from its index up shifts by
        // one. The toolbar draws its labels from Rust in this order and sends the index
        // back as the code, so a list that is out of step with the enum is a button that
        // picks up the eraser where the user pointed at the magnifier.
        let names = tool_names();
        let labels: Vec<&str> = names.split('|').collect();
        assert_eq!(
            labels.len(),
            TOOLS.len(),
            "a button with no tool behind it, or a tool with no button"
        );
        assert!(
            labels.iter().all(|s| !s.is_empty()),
            "an unnamed button: {labels:?}"
        );
        let mut seen: Vec<Option<Kind>> = Vec::new();
        for (i, kind) in TOOLS.iter().copied().enumerate() {
            assert!(!seen.contains(&kind), "{kind:?} has two buttons");
            seen.push(kind);
            assert_eq!(
                code_of(kind),
                i as i32,
                "{kind:?} does not point back at {i}"
            );
        }
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 40, 40))], [70, 70, 70, 255]);
        for (i, kind) in TOOLS.iter().copied().enumerate() {
            l.select_tool(i as i32);
            assert_eq!(
                l.tool(),
                kind,
                "button {i} ({}) chose {:?}",
                labels[i],
                l.tool()
            );
        }
    }

    #[test]
    fn every_hint_sits_on_the_button_it_names() {
        // §5.21.2 第一枚判的是「这句提示说的是不是这一枚钮」。非空不够：一句写歪了
        // 名字的提示照样填满那一格。所以判定钉在同一张表的三列上 —— 第 `i` 行的
        // kind 必须是 `TOOLS[i]`、句子必须以第 `i` 行的名字起头，而且除了名字还得
        // 说出点它之后会发生什么。
        let names_s = tool_names();
        let hints_s = tool_hints();
        let names: Vec<&str> = names_s.split('|').collect();
        let hints: Vec<&str> = hints_s.split('|').collect();
        assert_eq!(
            hints.len(),
            TOOLS.len(),
            "a button with no sentence over it, or a sentence with no button"
        );
        for (i, (kind, name, hint)) in TOOL_TEXTS.iter().enumerate() {
            assert_eq!(*kind, TOOLS[i], "row {i} is not the tool drawn there");
            assert_eq!(names[i], *name, "row {i} renamed its own button");
            assert!(
                hint.starts_with(&format!("{name}：")),
                "button {i} is named {name} but its hint says {hint}"
            );
            assert!(
                hint.chars().count() > name.chars().count() + 1,
                "the hint on {name} repeats the name and says nothing else"
            );
            assert!(!hint.contains('|'), "the hint on {name} splits itself");
        }
    }

    #[test]
    fn no_two_buttons_share_a_hint_sentence() {
        // Two cells with the same sentence is one cell that was never read: 撤销 and
        // 重做 sit next to each other, and a shared string is how a copy-paste of the
        // first one survives the review.
        let hints_s = tool_hints();
        let hints: Vec<&str> = hints_s.split('|').collect();
        let mut seen: Vec<&str> = Vec::new();
        for hint in hints {
            assert!(!seen.contains(&hint), "{hint} is on two buttons");
            seen.push(hint);
        }
    }

    // -------------------------------------------------- §5.7.13 step 5 的三件控制

    #[test]
    fn the_magnification_knob_regrows_the_copy_in_hand() {
        let mut l = zoom_layer();
        magnify(&mut l, (20, 20), (60, 60));
        let (from, to) = l.zoom_pending().unwrap();
        assert_eq!((to.w, to.h), (80, 80), "the copy starts at the tool's 200%");

        l.set_zoom_percent(400);
        let (still_from, grown) = l.zoom_pending().expect("the knob lost the copy");
        assert_eq!((grown.w, grown.h), (160, 160), "400% of a 40x40 source");
        assert_eq!(
            (grown.x, grown.y),
            (to.x, to.y),
            "the copy moved when only its size was asked for"
        );
        assert_eq!(still_from, from, "the knob resized the source region");
        assert_eq!(
            l.zoom_percent(),
            400,
            "the knob does not read back what it set"
        );

        // The knob cannot be taken outside the range `place` enforces, and says so in the
        // same number it set: a cell lit for 800% over a copy drawn at 2000% is the two
        // clamps disagreeing, which is why there is one pair of numbers here.
        l.set_zoom_percent(20);
        assert_eq!(
            l.zoom_percent(),
            100,
            "a magnifier smaller than its own source"
        );
        let (_, small) = l.zoom_pending().unwrap();
        assert_eq!((small.w, small.h), (40, 40), "100% is no change");
        l.set_zoom_percent(9000);
        assert_eq!(
            l.zoom_percent(),
            800,
            "and one the rasteriser would not survive"
        );
        let (_, huge) = l.zoom_pending().unwrap();
        assert_eq!(
            (huge.w, huge.h),
            (320, 300),
            "800%, then the selection's cap"
        );

        // An object that already committed keeps the size it was placed at. Step 5 sets
        // the pen; §5.7.17's per-object editing is a different command in the undo stack,
        // not a side effect of a toolbar click.
        let mut l = zoom_layer();
        magnify(&mut l, (20, 20), (60, 60));
        assert!(l.finish_zoom(), "the copy never committed");
        l.set_zoom_percent(800);
        let Geom::Zoom { to, .. } = l.doc.elements[0].geom else {
            panic!("放大 committed a {:?}", l.doc.elements[0].kind);
        };
        assert_eq!(
            (to.w, to.h),
            (80, 80),
            "the knob resized an object that was already placed"
        );
        assert!(l.undo_step(), "the commit was not undoable");
        assert_eq!(l.objects(), 0);
        assert!(
            !l.undo_step(),
            "one toolbar click became more than one undo step"
        );
    }

    #[test]
    fn the_two_switches_land_in_the_object_that_commits() {
        let mut l = zoom_layer();
        assert!(
            l.zoom_border(),
            "§5.7.13: 边框 on until the user says otherwise"
        );
        assert!(l.connection_line(), "and 连接线 the same");
        l.set_zoom_border(false);
        l.set_connection_line(false);
        assert!(!l.zoom_border(), "the switch does not read back");
        assert!(!l.connection_line(), "the switch does not read back");

        magnify(&mut l, (20, 20), (60, 60));
        assert!(l.finish_zoom(), "the switches never reached a commit");
        let e = &l.doc.elements[0];
        assert!(
            !e.style.zoom_border,
            "边框 is a cell the rasteriser never sees"
        );
        assert!(
            !e.style.connection_line,
            "连接线 is a cell the rasteriser never sees"
        );
        assert_eq!(
            e.style.zoom_percent, 200,
            "the third setting did not travel with the other two"
        );
    }

    #[test]
    fn a_switch_flipped_while_a_copy_waits_takes_the_old_pen_off_the_screen() {
        // A draft snapshots the style it was built with. Guarded on geometry alone, a
        // knob flipped while 放大 is in hand leaves the *previous* pen on the screen and
        // only agrees with the user when the object commits - a preview that lies about
        // what it will become. The flat grey base makes this measurable in one pixel: the
        // only ink a magnifier adds here is its frames and its connectors, in the pen's
        // colour.
        let mut l = zoom_layer();
        magnify(&mut l, (20, 20), (60, 60));
        let green = l.layer_of("m0").unwrap().get(40, 20);
        assert_ne!(
            green,
            [70, 70, 70, 255],
            "the source frame is not on the canvas"
        );
        assert!(
            green[1] as i32 - green[0].max(green[2]) as i32 > 40,
            "the preview is not the green pen: {green:?}"
        );
        let both = inked(&l, "m0");

        // The two switches first, while the pen is still the colour [`inked`] counts.
        // Each one gets its own number: the connector's pixels go and the frame's stay.
        l.set_connection_line(false);
        let framed = inked(&l, "m0");
        assert!(framed < both, "连接线 off left its pixels on screen");
        assert!(framed > 0, "…and it took the 边框 with it");
        l.set_zoom_border(false);
        assert_eq!(inked(&l, "m0"), 0, "边框 off left its pixels on screen");

        // Then the pen, over a preview that is visible again - the switch this test is
        // really about, because the geometry did not move and a guard on geometry alone
        // would have stopped here without repainting anything.
        l.set_zoom_border(true);
        l.set_color([0, 0, 255, 255]);
        let blue = l.layer_of("m0").unwrap().get(40, 20);
        assert!(
            blue[2] as i32 - blue[0].max(blue[1]) as i32 > 40,
            "the pen switch left the old colour on the preview: {blue:?}"
        );

        // And what the user goes on to commit is what the switches and the pen said:
        // `flatten` is the *committed* document's export, so a gesture still in hand has
        // no honest comparison to make until it is one.
        l.set_connection_line(true);
        assert!(l.finish_zoom(), "the switches never reached a commit");
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "the layer is not the document"
        );
    }

    #[test]
    fn the_three_settings_travel_with_the_tool_that_learned_them() {
        // §5.7.1's memory is one whole-`Style` snapshot per tool, so a field with a cell
        // of its own is remembered exactly like a colour is - and a magnifier that came
        // back at 200% after the user set 400% would be a tool that forgets on its own.
        let mut l = zoom_layer();
        l.set_zoom_percent(400);
        l.set_zoom_border(false);
        l.set_connection_line(false);

        l.select_tool(code_of(Some(Kind::Rect)));
        l.set_color([0, 0, 255, 255]);

        l.select_tool(code_of(Some(Kind::Zoom)));
        assert_eq!(
            l.zoom_percent(),
            400,
            "放大 came back without its magnification"
        );
        assert!(!l.zoom_border(), "放大 came back with its 边框 on");
        assert!(!l.connection_line(), "放大 came back with its 连接线 on");
        assert_eq!(
            l.color(),
            [0, 200, 0, 255],
            "the magnifier came back with the rectangle's pen"
        );
    }

    #[test]
    fn the_magnifier_knobs_are_offered_to_the_magnifier_only() {
        // The toolbar hides the row off one Rust bool rather than a comparison against a
        // tool index it would have to keep in step with [`TOOLS`] by itself - the same
        // reason `AnnotationToolbar.qml` takes its labels from `tool_names()`.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 40, 40))], [70, 70, 70, 255]);
        for (code, want) in [
            (code_of(None), false),
            (code_of(Some(Kind::Rect)), false),
            (code_of(Some(Kind::Polyline)), false),
            (code_of(Some(Kind::Zoom)), true),
            (code_of(Some(Kind::Eraser)), false),
        ] {
            l.select_tool(code);
            assert_eq!(l.zoom_tool(), want, "the row moved with button {code}");
        }
    }

    /// §5.7.11's letters through the product's own paths rather than core's: the
    /// incremental layer QML shows and the export the crop takes. The control is the
    /// same document painted through [`NoGlyphs`], which is what these two call sites
    /// did until the font bridge landed - it lays down no box here (背景 is off) and
    /// therefore nothing at all, so the count of pixels that left the desktop grey is
    /// the difference the bridge makes.
    #[cfg(windows)]
    #[test]
    fn a_text_element_lands_letters_on_the_layer_and_the_export() {
        use falcon_core::annotation::raster::NoGlyphs;

        let bounds = PhysRect::new(0, 0, 400, 200);
        let grey = [90, 90, 90, 255];
        let mut l = screens(&[("m0", bounds)], grey);
        l.set_hole(bounds);

        let area = PhysRect::new(10, 20, 220, 80);
        let style = Style {
            color: [255, 0, 0, 255],
            font_size: 24,
            ..Default::default()
        };
        let mut e = Element::new(1, Kind::Text, Geom::Rect(area), style);
        e.text = "局部放大 Annotation".into();
        l.apply(Command::Add(vec![e]));

        // Pixels the desktop is not, which is the only way to see letters through a
        // paint path that resets its own area to the base before drawing on it.
        let inked = |f: &Frame| -> usize {
            let mut n = 0;
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    if f.get(x as u32, y as u32) != grey {
                        n += 1;
                    }
                }
            }
            n
        };

        let layer = l.layer_of("m0").unwrap();
        let on = inked(layer);
        assert!(on > 200, "文本 drew {on} pixels that are not the desktop");
        let export = l.flatten("m0").unwrap();
        assert_eq!(on, inked(&export), "the export counted a different picture");
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "the layer is not the document"
        );
        let solid = (area.w * area.h) as usize;
        assert!(on * 3 < solid, "the box came back filled: {on} of {solid}");
        let pen = (area.y..area.bottom()).any(|y| {
            (area.x..area.right()).any(|x| {
                let p = export.get(x as u32, y as u32);
                p[0] as i32 - p[1].max(p[2]) as i32 > 40
            })
        });
        assert!(
            pen,
            "no pixel carries the pen's colour, so something else drew"
        );

        let c = &l.canvases[0];
        let (dx, dy) = shift(l.origin, c.bounds);
        let view = view_of(&l.doc.elements, c, dx, dy);
        let mut bare = c.base.clone();
        raster::paint(&c.base, &view, &mut bare, &c.bounds, &NoGlyphs).unwrap();
        assert_eq!(
            inked(&bare),
            0,
            "the leg that answers no glyphs drew letters anyway"
        );
    }

    // ------------------------------------------------------------ 文本 (§5.7.11)

    /// A layer with the 文本 tool in its hand. Its screen is deliberately *not* at the
    /// desktop's corner: the box QML positions lives in desktop pixels and the box the
    /// document files lives in document space, and on a screen at (0,0) those are the
    /// same numbers, so a lost origin would pass every test below.
    fn text_layer() -> Layer {
        let one = PhysRect::new(100, 50, 200, 120);
        let mut l = screens(&[("m0", one)], [70, 70, 70, 255]);
        l.set_hole(one);
        l.set_color([0, 200, 0, 255]);
        l.select_tool(code_of(Some(Kind::Text)));
        l
    }

    /// Pixels the incremental layer has an opinion about - any of them, not just this
    /// pen's green. While a box is open the answer has to be none of them: the field is
    /// what shows the letters, and a layer that drew them too would put a second glyph
    /// under the first.
    fn touched(l: &Layer, name: &str) -> usize {
        let f = l.layer_of(name).unwrap();
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| f.get(x, y)))
            .filter(|p| p[3] != 0)
            .count()
    }

    #[test]
    fn a_click_places_a_box_and_a_click_away_files_it() {
        let mut l = text_layer();

        // Steps 2 and 3, in the order the PRD gives them. Desktop (130,80) is document
        // (30,30) on this screen.
        assert!(
            l.press(PhysPoint::new(130, 80)),
            "文本 did not take the click"
        );
        let live = l.typing_rect().expect("the placing click opened no box");
        assert_eq!(
            (live.x, live.y),
            (130, 80),
            "the box is not under the pointer"
        );
        assert_eq!(l.objects(), 0, "the placing click also committed an object");
        assert_eq!(
            touched(&l, "m0"),
            0,
            "the layer drew the box that belongs to the field"
        );

        // The button coming up is not step 5: it is the same click that opened the box,
        // and committing here would end the gesture before one keystroke.
        l.drag(PhysPoint::new(160, 110));
        assert!(
            !l.release(),
            "the release filed a box nobody has typed into"
        );
        assert_eq!(
            l.typing_rect(),
            Some(live),
            "the drag or the release moved the box"
        );

        l.type_text("hi");
        let typed = l.typing_rect().expect("typing closed the box");
        assert!(typed.w > live.w, "the box did not grow with the text");
        assert_eq!((typed.x, typed.y), (130, 80), "typing moved the box");

        // Step 5: a press anywhere else. It files the box and does not place the next one.
        assert!(
            l.press(PhysPoint::new(260, 150)),
            "the finishing click was not taken"
        );
        assert!(!l.release());
        assert!(
            l.typing_rect().is_none(),
            "the finishing click placed another box"
        );
        assert_eq!(l.objects(), 1, "the click away did not file the text");

        let e = &l.doc.elements[0];
        assert_eq!(e.kind, Kind::Text);
        assert_eq!(e.text, "hi");
        let Geom::Rect(filed) = e.geom else {
            panic!("文本 filed {:?}, not a rect", e.geom);
        };
        // The one number the two paths share: what the field was positioned by is what
        // the document kept, minus the origin - so a measure taken twice, or a box
        // re-derived at commit, cannot quietly disagree with the caret the user was at.
        assert_eq!(
            filed,
            PhysRect::new(30, 30, typed.w, typed.h),
            "the document filed a different box than the field sat in"
        );

        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "the layer is not the document"
        );
        assert!(inked(&l, "m0") > 0, "the committed text drew no pixels");
        assert!(
            l.last_paint_px < l.full_px,
            "one 文本 commit repainted the whole desktop: {} of {}",
            l.last_paint_px,
            l.full_px
        );

        // One screen at the desktop's corner, so `shift` is (0,0) and document
        // coordinates are this canvas's own. Three pixels of slack: two is the reach the
        // pen adds to the geometry (`bounds()` inflates by it, and the whole inflated
        // area comes back opaque base), and one more covers the fringe of an
        // antialiased edge.
        let grown = PhysRect::new(filed.x - 3, filed.y - 3, filed.w + 6, filed.h + 6);
        let f = l.layer_of("m0").unwrap();
        let stray = (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| (x, y)))
            .filter(|(x, y)| {
                f.get(*x, *y)[3] != 0 && !grown.contains(PhysPoint::new(*x as i32, *y as i32))
            })
            .count();
        assert_eq!(
            stray, 0,
            "letters landed outside the box they were measured for"
        );

        assert!(l.can_undo());
        assert!(l.undo_step(), "Ctrl+Z did not take the text back");
        assert_eq!(l.objects(), 0);
        assert_eq!(
            inked(&l, "m0"),
            0,
            "the undo left the letters on the screen"
        );
    }

    /// §5.7.11 规则三「文本框应支持换行」在这一层的那一读。
    ///
    /// 三趟读的是**同一枚开着的框**,所以两个数都不是新造的:`wide` 是最长那单独一行,
    /// `narrow` 是短的那单独一行,`two` 是把它们写上下两趟。于是「换行只往下长、不往两边
    /// 长」这句话有证人而不靠 [`raster::text_extent`] 的实现自己说自己对:锚点不动,宽就是
    /// 最宽那一行(与另一行在前还是在后无关,所以 `flipped` 也读一次),高是那两行各自一行
    /// 高度之和。
    ///
    /// 量到框变高了不等于支持:那是 measure 那一半。第二趟数的是**一行下沿以下**真有这笔
    /// 的绿,才是 paint 那一半也落了笔。`NoGlyphs` 什么都不画,这一读就没有读者,所以整条
    /// gate 在 windows 上——落这一笔的那一只手在 `qml/CaptureMask.qml` 的两枚键上:Shift+Enter
    /// 在光标处插一行、裸 Enter 收尾,不在这里。
    #[test]
    #[cfg(windows)]
    fn a_newline_grows_the_box_downward_and_draws_the_second_line() {
        let mut l = text_layer();
        assert!(
            l.press(PhysPoint::new(130, 80)),
            "文本 did not take the click"
        );

        // The field's whole string arrives wholesale on every keystroke, so one open box
        // can be measured with any probe put in it - which is what keeps the two
        // one-line readings the same two the user would have had on the way to the third.
        let measure = |l: &mut Layer, s: &str| -> PhysRect {
            l.type_text(s);
            l.typing_rect().expect("typing closed the box")
        };

        let wide = measure(&mut l, "cdef");
        let narrow = measure(&mut l, "ab");
        let two = measure(&mut l, "ab\ncdef");
        let flipped = measure(&mut l, "cdef\nab");

        assert_eq!(
            (two.x, two.y),
            (narrow.x, narrow.y),
            "the newline moved the anchor instead of growing down from it"
        );
        assert_eq!(
            two.w, wide.w,
            "the second line grew the box sideways: {} against {}",
            two.w,
            wide.w
        );
        assert!(
            two.w > narrow.w,
            "a two-line box is no wider than the short line it also contains"
        );
        assert_eq!(
            two.h,
            wide.h + narrow.h,
            "each line should cost its own height, so two lines cost {} and the box says {}",
            wide.h + narrow.h,
            two.h
        );
        assert_eq!(
            flipped, two,
            "which line comes first changed the measure - the box is not the widest line"
        );

        // Back to the order the fold is read in: the first line of what gets committed has
        // to be the line whose height was measured, or the band below it is guesswork.
        let two = measure(&mut l, "ab\ncdef");
        assert!(l.finish_typing(), "Enter filed nothing");
        let e = &l.doc.elements[0];
        assert_eq!(e.text, "ab\ncdef", "commit took the newline out");
        let Geom::Rect(filed) = e.geom else {
            panic!("文本 filed {:?}, not a rect", e.geom);
        };
        assert_eq!(
            (filed.w, filed.h),
            (two.w, two.h),
            "the document filed a different box than the field sat in"
        );

        // The fold is where a one-line box would have ended; anything this layer has an
        // opinion about below it is a letter of the second line.
        let f = l.layer_of("m0").unwrap();
        let from = (filed.y + narrow.h as i32).max(0) as u32;
        let to = (filed.bottom()).max(0) as u32;
        let (from, to) = (from.min(f.height), to.min(f.height));
        let second_line = (from..to)
            .flat_map(|y| (0..f.width).map(move |x| f.get(x, y)))
            .filter(|p| p[3] != 0 && p[1] as i32 - p[0].max(p[2]) as i32 > 40)
            .count();
        assert!(
            second_line > 0,
            "the box is two lines tall and the second one drew no pixel of this pen"
        );
    }

    #[test]
    fn a_box_with_nothing_in_it_is_not_an_undoable_nothing() {
        let mut l = text_layer();

        // Step 5 on a box no one has typed into: the box goes, and nothing else happens.
        assert!(l.press(PhysPoint::new(130, 80)));
        assert!(l.press(PhysPoint::new(150, 90)));
        assert_eq!(l.objects(), 0, "an empty click-away filed a text object");
        assert!(!l.can_undo(), "the empty box is in the keystroke history");
        assert!(l.typing_rect().is_none(), "the empty box stayed open");
        assert_eq!(touched(&l, "m0"), 0);

        // Whitespace is the same nothing, seen by `trim` rather than by length.
        assert!(l.press(PhysPoint::new(130, 80)));
        l.type_text("   ");
        assert!(!l.finish_typing(), "Enter accepted three spaces");
        assert_eq!(l.objects(), 0);

        // And each discard leaves the layer ready for the next box, which is the only
        // way out of the state the two blocks above just entered.
        assert!(l.press(PhysPoint::new(130, 80)));
        assert_eq!(
            l.typing_rect().map(|r| (r.x, r.y)),
            Some((130, 80)),
            "a box after a discard did not open where it was clicked"
        );
        l.type_text("go");
        assert!(l.press(PhysPoint::new(150, 90)));
        assert_eq!(l.objects(), 1, "a box after two discards was never filed");
    }

    #[test]
    fn every_way_out_of_an_open_box_takes_the_field_back() {
        // §9.1's ⑦ again, for the third gesture with state the undo stack cannot see.
        // The judgement is "the box is gone and nothing was filed" rather than the
        // `inked() == 0` the other two reclaims are judged by, because the box owns no
        // pixels to take back - the difference [`Layer::abandon_typing`] documents.
        for way in 0..5 {
            let mut l = text_layer();
            assert!(l.press(PhysPoint::new(130, 80)));
            l.type_text("hi");
            match way {
                0 => l.select_tool(code_of(None)),
                1 => l.select_tool(code_of(Some(Kind::Rect))),
                2 => l.set_hole(PhysRect::default()),
                3 => {
                    l.clear_ink();
                }
                _ => {
                    l.abandon_typing();
                }
            }
            assert!(l.typing_rect().is_none(), "path {way} kept the box in hand");
            assert_eq!(l.objects(), 0, "path {way} filed a box the user abandoned");
            assert_eq!(
                touched(&l, "m0"),
                0,
                "path {way} painted a box that owns no pixels"
            );
        }
    }

    #[test]
    fn the_box_is_the_size_the_letters_were_measured_for() {
        use falcon_core::annotation::raster::NoGlyphs;

        let l = text_layer();
        let at = PhysPoint::new(30, 30);

        // What the click fixes: one corner is the click, whatever the measure says.
        let one = l.box_at(at, "ab", &DirectWrite);
        assert_eq!((one.x, one.y), (30, 30));
        assert!(one.w >= 1 && one.h >= 1, "the box collapsed to nothing");

        let more = l.box_at(at, "ab cd ef", &DirectWrite);
        assert!(more.w > one.w, "the box did not grow with the string");
        assert_eq!(more.h, one.h, "a longer line changed the line height");

        let two = l.box_at(at, "ab\nab", &DirectWrite);
        assert!(two.h > one.h, "a second line did not make the box taller");
        assert_eq!(two.w, one.w, "the box is not the widest of its lines");

        // With no leg to answer, the box must be *stable* instead: the same size for the
        // same character count, so the field never jumps under the caret because a period
        // happens to be narrower than an m.
        let dots = l.box_at(at, "....", &NoGlyphs);
        assert!(dots.w >= 1 && dots.h >= 1, "the fallback box collapsed");
        assert_eq!(
            dots,
            l.box_at(at, "aaaa", &NoGlyphs),
            "the fallback measured the glyphs it cannot see"
        );
        let twice = l.box_at(at, "....\n....", &NoGlyphs);
        assert_eq!(twice.w, dots.w, "the fallback grew sideways on a newline");
        assert_eq!(twice.h, dots.h * 2, "the fallback is not one em per line");

        // The other branch, proven rather than assumed: a real measure cannot give these
        // two the same width, so if the estimate were still answering this would fail.
        #[cfg(windows)]
        {
            let wide = l.box_at(at, "MMMM", &DirectWrite);
            let thin = l.box_at(at, "iiii", &DirectWrite);
            assert_ne!(
                wide.w, thin.w,
                "the box is the fixed half-em estimate, not the font's own measure"
            );
        }
    }

    #[test]
    fn enter_accepts_the_box_where_the_click_left_it() {
        let mut l = text_layer();
        assert!(
            !l.finish_typing(),
            "Enter with no box open has to reach 完成选区"
        );

        assert!(l.press(PhysPoint::new(130, 80)));
        let live = l.typing_rect().unwrap();
        assert!(
            !l.finish_typing(),
            "Enter accepted a box with nothing typed into it"
        );
        assert_eq!(l.objects(), 0);

        assert!(
            l.press(PhysPoint::new(130, 80)),
            "the discard closed the flow"
        );
        l.type_text("ok");
        let typed = l.typing_rect().unwrap();
        assert!(l.finish_typing(), "Enter did not accept the open box");
        assert_eq!(l.objects(), 1);
        assert!(l.typing_rect().is_none(), "Enter left the field open");
        assert!(
            !l.finish_typing(),
            "a second Enter committed the same box twice"
        );

        let Geom::Rect(filed) = l.doc.elements[0].geom else {
            panic!("文本 filed {:?}, not a rect", l.doc.elements[0].geom)
        };
        assert_eq!(
            filed,
            PhysRect::new(live.x - l.origin.x, live.y - l.origin.y, typed.w, typed.h),
            "Enter filed a box the field was not sitting in"
        );
        assert!(
            inked(&l, "m0") > 0,
            "the layer never drew the text it filed"
        );
        assert_eq!(
            mismatched(&l, "m0").unwrap(),
            Vec::<String>::new(),
            "the layer is not the document"
        );
    }

    // ------------------------------------------------------ 文本样式 (§5.7.11 step 4)

    /// The desktop `Layer`-level 文本样式 tests paint on. Dark enough that a white 描边
    /// and a translucent plate are each separable from it, and from each other.
    const PLATE_BASE: [u8; 4] = [70, 70, 70, 255];

    /// The pen these tests write with: green, so a pixel's `g - max(r, b)` says how much
    /// of it is this pen and how much is only its own antialiasing.
    const PLATE_PEN: [u8; 4] = [0, 200, 0, 255];

    /// Count the pixels of the whole canvas that `area` cannot hold: `f` is read over the
    /// canvas, which is what [`Layer::flush`] would hand to Qt, so a plate that grew past
    /// its own box is counted rather than missed.
    #[cfg(windows)]
    fn pixels(l: &Layer, name: &str, mut keep: impl FnMut([u8; 4]) -> bool) -> usize {
        let f = l.layer_of(name).unwrap();
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| f.get(x, y)))
            .filter(|p| *p != PLATE_BASE && keep(*p))
            .count()
    }

    /// Pixels that are *exactly* the pen. A letter pixel at full coverage is the pen
    /// whatever sits under it ([`raster`]'s over-composite lands on the source colour at
    /// `v = 255`), so this is the measure that can say "the plate is under the letters" and
    /// mean it as an equality rather than as a tolerance.
    #[cfg(windows)]
    fn letters(l: &Layer) -> usize {
        pixels(l, "m0", |p| p == PLATE_PEN)
    }

    /// Pixels carrying the pen's hue but not its density - the plate, and the fringe every
    /// glyph edge already has with 背景 off. Which is why the test below compares two runs
    /// instead of naming a number.
    #[cfg(windows)]
    fn faint(l: &Layer) -> usize {
        pixels(l, "m0", |p| {
            let green = p[1] as i32 - p[0].max(p[2]) as i32;
            green > 20 && green <= 150
        })
    }

    /// Pixels that are neither the pen nor the desktop: the 描边, whose colour
    /// [`Layer::outline_of`] picks off the lightness scale the pen is not on.
    #[cfg(windows)]
    fn ring(l: &Layer) -> usize {
        pixels(l, "m0", |p| {
            let lo = p[0].min(p[1]).min(p[2]);
            let hi = p[0].max(p[1]).max(p[2]);
            lo > 140 && hi - lo < 30
        })
    }

    /// Type a word through the product's own gesture - click, keystrokes, Enter - and hand
    /// back the layer that has filed it. The row of step 4's knobs is what the caller moves.
    #[cfg(windows)]
    fn typed(text: &str) -> Layer {
        let mut l = text_layer();
        assert!(
            l.press(PhysPoint::new(130, 80)),
            "文本 did not take the click"
        );
        l.type_text(text);
        assert!(l.finish_typing(), "Enter filed nothing");
        l
    }

    #[test]
    fn the_text_knobs_are_offered_to_the_text_tool_only() {
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 40, 40))], PLATE_BASE);
        for (code, want) in [
            (code_of(None), false),
            (code_of(Some(Kind::Rect)), false),
            (code_of(Some(Kind::Zoom)), false),
            (code_of(Some(Kind::Text)), true),
            (code_of(Some(Kind::Eraser)), false),
        ] {
            l.select_tool(code);
            assert_eq!(l.text_tool(), want, "the row moved with button {code}");
        }
    }

    #[test]
    fn the_erase_mode_is_offered_to_the_eraser_only() {
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 40, 40))], PLATE_BASE);
        for (code, want) in [
            (code_of(None), false),
            (code_of(Some(Kind::Rect)), false),
            (code_of(Some(Kind::Pencil)), false),
            (code_of(Some(Kind::Eraser)), true),
        ] {
            l.select_tool(code);
            assert_eq!(l.eraser_tool(), want, "the row moved with button {code}");
        }
    }

    #[test]
    fn the_default_eraser_puts_the_picture_back_and_the_transparent_one_takes_it() {
        // One red band down the middle of a flat grey desktop, then the same 橡皮 path
        // over it twice - once as the pen comes by default, once with §5.7.14's mode on.
        // The readings are the fixture's own numbers: [90,90,90,255] is what the frozen
        // frame was filled with, and `0` is the alpha a pixel that no longer contributes
        // has. Neither is read back out of the code being graded.
        const GREY: [u8; 4] = [90, 90, 90, 255];
        const RED: [u8; 4] = [232, 17, 35, 255];
        let one = PhysRect::new(0, 0, 320, 120);
        let mut export = [GREY; 2];

        for (leg, transparent) in [false, true].into_iter().enumerate() {
            let mut l = screens(&[("m0", one)], GREY);
            l.set_hole(one);
            l.select_tool(code_of(Some(Kind::Pencil)));
            l.set_color(RED);
            l.set_width(12);
            band(&mut l);
            let inked = l.layer_of("m0").unwrap().get(160, 60);
            assert!(
                inked[0] as i32 - inked[1].max(inked[2]) as i32 > 40,
                "the ink never arrived (transparent={transparent}): {inked:?}"
            );

            l.select_tool(code_of(Some(Kind::Eraser)));
            assert!(
                !l.erase_transparent(),
                "the mode is off until someone asks for it"
            );
            l.set_erase_transparent(transparent);
            assert_eq!(l.erase_transparent(), transparent);
            band(&mut l);

            let erased = l.layer_of("m0").unwrap().get(160, 60);
            export[leg] = l.flatten("m0").unwrap().get(160, 60);
            if transparent {
                assert_eq!(
                    erased[3], 0,
                    "the overlay kept pixels it was told to clear: {erased:?}"
                );
                // The rgb is the ink that was there - invisible at alpha 0, and left as
                // it is because clearing a pixel is not a colour operation.
                assert_eq!(
                    export[leg][3], 0,
                    "the exported picture is not transparent: {:?}",
                    export[leg]
                );
                // Through the encoder's no-alpha branch, which is what a JPG save of the
                // same frame takes: the hole becomes the colour `encode` flattens on and
                // the ink that was cleared does not come back. Measured, because a reader
                // cannot check that sentence on the screen - it is in a file's bytes.
                let jpg = falcon_core::encode::encode(
                    &l.flatten("m0").unwrap(),
                    &falcon_core::encode::EncodeOptions {
                        format: falcon_core::encode::Format::Jpg,
                        ..Default::default()
                    },
                )
                .unwrap();
                let back = falcon_core::encode::decode(&jpg).unwrap().get(160, 60);
                assert!(
                    back[0] >= 200 && back[1] >= 200 && back[2] >= 200,
                    "the JPG leg is not the fill colour: {back:?}"
                );
                assert!(
                    back[0] as i32 - back[1].max(back[2]) as i32 <= 40,
                    "the JPG leg came back as the erased ink: {back:?}"
                );
            } else {
                assert_eq!(erased, GREY, "默认 erased the screenshot too: {erased:?}");
                assert_eq!(export[leg], GREY, "the export lost the screenshot");
            }
            // And the incremental canvas still agrees with a whole-canvas render, which
            // is the claim a mode that *subtracts* is exactly the shape to break.
            assert_eq!(
                mismatched(&l, "m0").unwrap(),
                Vec::<String>::new(),
                "transparent={transparent}"
            );
        }

        // One geometry, one bool, and the two exported pixels that came out of it: the
        // difference between these two *is* §5.7.14's rule, and on the live mask it is
        // invisible either way - which is why the toolbar says it in words.
        assert_ne!(export[0], export[1], "both modes exported the same pixel");
    }

    #[test]
    fn the_eraser_keeps_its_own_answer_when_another_tool_moves_the_same_field() {
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 40, 40))], [90, 90, 90, 255]);
        l.select_tool(code_of(Some(Kind::Eraser)));
        assert!(!l.erase_transparent(), "§5.7.14's 默认 is 只擦标注");

        l.set_erase_transparent(true);
        l.select_tool(code_of(Some(Kind::Rect)));
        // The rect's pen is the eraser's, because `select_tool` deliberately has no else
        // branch (§5.7.1 only requires remembering what a tool was last left with) - so
        // this reads true even though the row is hidden for the rect. The field is one
        // bool of the pen, and nothing else in the product reads it for a rect.
        assert!(
            l.erase_transparent(),
            "an untaught tool inherits the pen in hand"
        );
        assert!(!l.eraser_tool(), "the row is not the rect's");
        l.set_erase_transparent(false);

        l.select_tool(code_of(Some(Kind::Eraser)));
        assert!(
            l.erase_transparent(),
            "the eraser came back with what it was left with, not the rect's answer"
        );
    }

    /// The middle band both legs of the two-mode test draw: 40 to 280 along y = 60, in
    /// one grab. The same geometry is the whole point - the only thing that may differ
    /// between the two readings is the pen's mode.
    fn band(l: &mut Layer) {
        assert!(l.press(PhysPoint::new(40, 60)));
        for x in 40..=280 {
            l.drag(PhysPoint::new(x, 60));
        }
        assert!(l.release());
    }

    #[test]
    fn a_font_size_moves_the_field_the_user_is_typing_into() {
        // The one knob of the four that has to bite *before* the object exists: the box
        // open on the screen is measured off the live style, so a field that kept the size
        // it was clicked at is a control that only works on the next one.
        let mut l = text_layer();
        assert!(l.press(PhysPoint::new(130, 80)));
        l.type_text("Falcon");
        let small = l.typing_rect().expect("typing closed the box");

        l.set_font_size(60);
        assert_eq!(l.font_px(), 60);
        let big = l.typing_rect().expect("the size change closed the box");
        assert_eq!(
            (big.x, big.y),
            (small.x, small.y),
            "a bigger font moved the box instead of growing it"
        );
        assert!(big.h > small.h && big.w > small.w, "{small:?} => {big:?}");

        // The ends of the ladder are Rust's, not the toolbar's: 0 would divide nothing and
        // 4,000 would draw one line taller than the hole.
        l.set_font_size(0);
        assert_eq!(l.font_px(), FONT_MIN, "the floor is not a clamp");
        l.set_font_size(4000);
        assert_eq!(l.font_px(), FONT_MAX, "the ceiling is not a clamp");
    }

    #[test]
    fn an_align_code_off_the_ladder_leaves_the_pen_alone() {
        let mut l = text_layer();
        l.set_align(1);
        assert_eq!(l.align_code(), 1, "居中 did not land");
        l.set_align(7);
        assert_eq!(
            l.align_code(),
            1,
            "a code past the ladder overwrote the pen the user had set"
        );
        l.set_align(-1);
        assert_eq!(l.align_code(), 1, "a negative code overwrote it too");
    }

    #[test]
    fn a_typed_family_lands_on_the_pen_and_on_what_is_selected() {
        // §5.7.11 step 4's 字体 as the row's one string field. A name is no rung on a
        // ladder, so this arm can say three things: the pen took the name, the selected
        // group moved with it in one step, and a box the user cleaned is 没改 rather
        // than 改成默认.
        let mut l = three_objects();
        let [a, b] = marquee_the_pair(&mut l);
        let third = l.doc.elements.last().unwrap().id;

        l.set_font_family("  Consolas  ");
        assert_eq!(
            l.font_family(),
            "Consolas",
            "trim 归 Rust：页上不许夹第二遍，那它就得夹得动"
        );
        assert_eq!(l.doc.get(a).unwrap().style.font_family, "Consolas");
        assert_eq!(l.doc.get(b).unwrap().style.font_family, "Consolas");
        assert_eq!(
            l.doc.get(third).unwrap().style.font_family,
            Style::default().font_family,
            "框外那一枚跟着换了字体"
        );
        assert_eq!(l.undo.depth(), 1, "一组一次改动是一条，不是一组");

        // 空白不是一枚名字。把那一格清干净交回来，说的是「我不动这一支笔」，而不是「换回
        // 默认那枚」—— 后者会让一次 no-op 变成一次改写，而用户原本那枚字体被谁换掉了，
        // 屏幕上没有任何一句话会说。与 [`Layer::set_align`] 那枚 off-the-ladder 同一规。
        let before = l.doc.get(a).unwrap().style.clone();
        l.set_font_family("   ");
        assert_eq!(l.font_family(), "Consolas", "一句空白把笔改了");
        assert_eq!(l.doc.get(a).unwrap().style, before, "一句空白改动了选中的那一枚");
        assert_eq!(l.undo.depth(), 1, "什么都没改就不该往撤销栈上放一条");
    }

    #[test]
    fn the_five_knobs_travel_with_the_tool_that_learned_them() {
        let mut l = text_layer();
        l.set_font_size(40);
        l.set_font_family("Consolas");
        l.set_align(2);
        l.set_text_bg(true);
        l.set_text_outline(true);

        l.select_tool(code_of(Some(Kind::Rect)));
        l.set_color([0, 0, 255, 255]);

        l.select_tool(code_of(Some(Kind::Text)));
        assert_eq!(l.font_px(), 40, "文本 came back without its 字号");
        assert_eq!(l.font_family(), "Consolas", "文本 came back without its 字体");
        assert_eq!(l.align_code(), 2, "文本 came back without its 对齐");
        assert!(l.text_bg_on(), "文本 came back with its 背景 off");
        assert!(l.text_outline_on(), "文本 came back with its 描边 off");
        assert_eq!(
            l.color(),
            PLATE_PEN,
            "the text pen came back with the rectangle's colour"
        );
    }

    /// 对齐 through the document rather than the gesture: [`Layer::box_at`] sizes the box
    /// to the line, so a box the user typed into has nothing to align *inside* until
    /// §5.7.11's rule 3 (缩放) gives something a wider box. The rect here is that box, and
    /// every step after it - [`Layer::apply`], the incremental layer, the same glyph leg -
    /// is the product's.
    #[cfg(windows)]
    #[test]
    fn alignment_moves_the_line_within_a_box_wider_than_the_line() {
        let area = PhysRect::new(10, 10, 300, 60);
        let edge = |align: Align| -> (u32, u32) {
            let style = Style {
                color: PLATE_PEN,
                font_size: 24,
                align,
                ..Default::default()
            };
            let mut l = screens(&[("m0", PhysRect::new(0, 0, 400, 200))], PLATE_BASE);
            l.set_hole(PhysRect::new(0, 0, 400, 200));
            let mut e = Element::new(1, Kind::Text, Geom::Rect(area), style);
            e.text = "Falcon".into();
            l.apply(Command::Add(vec![e]));
            let f = l.layer_of("m0").unwrap();
            let cols: Vec<u32> = (0..f.width)
                .filter(|x| {
                    (0..f.height).any(|y| {
                        let p = f.get(*x, y);
                        p[1] as i32 - p[0].max(p[2]) as i32 > 150
                    })
                })
                .collect();
            assert!(!cols.is_empty(), "{align:?} drew no letters at all");
            (*cols.first().unwrap(), *cols.last().unwrap())
        };

        let (left, centre, right) = (edge(Align::Left), edge(Align::Center), edge(Align::Right));
        assert!(
            left.0 < centre.0 && centre.0 < right.0,
            "the three cells did not move the line: left {left:?}, centre {centre:?}, right {right:?}"
        );
        assert!(
            left.1 < centre.1 && centre.1 < right.1,
            "the line's far edge did not follow: left {left:?}, centre {centre:?}, right {right:?}"
        );
        // The control the ordering needs: the middle is not merely *different*, it is the
        // same picture every time, so a measure that could not tell 左 from 右 would have
        // failed the two assertions above instead of passing them.
        assert_eq!(
            edge(Align::Left),
            left,
            "the same cell painted twice, twice"
        );
        assert_eq!(
            edge(Align::default()),
            left,
            "the model's own default is not the row's first cell"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_background_lays_a_plate_and_leaves_the_letters_alone() {
        let off = typed("Falcon 2026");
        let mut l = text_layer();
        assert!(l.press(PhysPoint::new(130, 80)));
        l.type_text("Falcon 2026");
        l.set_text_bg(true);
        assert!(l.finish_typing());
        let on = &l;

        assert!(letters(on) > 20, "the letters vanished behind the plate");
        assert_eq!(
            letters(on),
            letters(&off),
            "the plate moved a letter pixel off the pen's own colour"
        );
        let (plate_off, plate_on) = (faint(&off), faint(on));
        assert!(
            plate_on > plate_off + 40,
            "no plate: {plate_off} faint pixels with 背景 off, {plate_on} with it on"
        );

        // And the plate is the translucent one §5.7.2's 填充 already uses, not the pen at
        // full strength - the reading that says the letters are still the densest thing
        // inside their own background.
        let bg = l.doc.elements[0]
            .style
            .text_bg
            .expect("the switch filed nothing");
        assert_eq!(bg, [0, 200, 0, 76], "the plate is not the pen at 3/10");

        let mut back = text_layer();
        assert!(back.press(PhysPoint::new(130, 80)));
        back.type_text("Falcon 2026");
        back.set_text_bg(true);
        back.set_text_bg(false);
        assert!(back.finish_typing());
        assert_eq!(
            faint(&back),
            plate_off,
            "the switch's off leg left a plate behind"
        );
    }

    #[cfg(windows)]
    #[test]
    fn an_outline_rings_the_letters_it_does_not_cover() {
        let off = typed("Falcon 2026");
        let mut l = text_layer();
        assert!(l.press(PhysPoint::new(130, 80)));
        l.type_text("Falcon 2026");
        l.set_text_outline(true);
        assert!(l.finish_typing());

        assert_eq!(
            ring(&off),
            0,
            "the ring was in the picture before the switch"
        );
        let on = ring(&l);
        assert!(
            on > 20,
            "the switch laid {on} ring pixels, which is nothing"
        );
        assert_eq!(
            letters(&l),
            letters(&off),
            "the ring painted over the letters"
        );

        let style = &l.doc.elements[0].style;
        assert!(style.text_outline, "开关按下了而那一格还关着");
        let (colour, width) = style.outline_for();
        assert_eq!(width, OUTLINE_WIDTH_PX, "the width is not the one set");
        assert_ne!(colour, PLATE_PEN, "a ring in the pen's own colour");
        assert_eq!(
            colour,
            [255, 255, 255, 255],
            "the dark pen got the dark ring"
        );

        // The other side of the same rule, which is the whole reason the colour is derived
        // rather than fixed: a light pen must not be ringed in the colour it is nearly
        // already made of.
        let mut bright = text_layer();
        bright.set_color([255, 255, 128, 255]);
        bright.set_text_outline(true);
        assert_eq!(
            bright.style.outline_for().0,
            [0, 0, 0, 255],
            "a light pen got a light ring"
        );
    }

    /// A config as a user would leave it: typed by hand, then put through the file's own
    /// validation, because `[annotation]` values only ever reach a layer along that road.
    fn annotated(edit: impl FnOnce(&mut Annotation)) -> Annotation {
        let mut cfg = Config::default();
        edit(&mut cfg.annotation);
        let warnings = cfg.validate();
        assert!(warnings.is_empty(), "{warnings:?}");
        cfg.annotation
    }

    #[test]
    fn a_stroke_colour_written_by_hand_reaches_the_pen() {
        let ann = annotated(|a| a.stroke_color = "#00FF00FF".into());
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.apply_annotation_defaults(&ann);
        assert_eq!(
            l.color(),
            [0, 255, 0, 255],
            "the file's colour never arrived"
        );
        assert!(l.problems.is_empty(), "{:?}", l.problems);
    }

    #[test]
    fn a_stroke_width_written_by_hand_moves_the_brush_with_it() {
        // The brush half is the point of the test rather than a detail: §5.7.20's rule is
        // that one number sets both, so a file that moved the pen and left the pencil's
        // brush at the model's 16 would be the tool-that-ignores-the-knob shape, arriving
        // through the config instead of through the toolbar.
        let ann = annotated(|a| a.stroke_width = 9);
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.apply_annotation_defaults(&ann);
        assert_eq!(l.width(), 9);
        assert_eq!(
            l.style.brush.size, 36,
            "the pen took the file's width, the brush did not"
        );
    }

    #[test]
    fn a_font_size_and_family_written_by_hand_reach_the_letters() {
        let ann = annotated(|a| {
            a.font_size = 28;
            a.font_family = "Consolas".into();
        });
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.set_hole(PhysRect::new(0, 0, 100, 100));
        l.apply_annotation_defaults(&ann);
        assert_eq!(l.font_px(), 28);
        assert_eq!(l.font_family(), "Consolas");

        // A pen that reaches the field but not the object would show the right size in the
        // toolbar and export another, so the committed element is read as well as the layer.
        l.select_tool(code_of(Some(Kind::Text)));
        assert!(l.press(PhysPoint::new(10, 10)), "the click placed nothing");
        l.type_text("Falcon");
        assert!(l.finish_typing());
        let s = &l.doc.elements[0].style;
        assert_eq!(s.font_size, 28, "the object kept the model's size");
        assert_eq!(
            s.font_family, "Consolas",
            "the object kept the model's family"
        );
    }

    #[test]
    fn the_files_font_range_is_the_pens_font_range() {
        // The defect this round found on the way in: the validator let the file say 6 and
        // 200 while the pen only ever allowed 10 and 96, and both numbers were in the tree
        // at once. So the row below compares the two halves to *each other* rather than to
        // a literal - a test that pinned the pen to 10 alone would still have passed with
        // the file reading 6, and the file is the thing the user re-opens.
        for (written, wanted, said) in [(6, FONT_MIN, true), (200, FONT_MAX, true), (12, 12, false)]
        {
            let mut cfg = Config::default();
            cfg.annotation.font_size = written;
            let warnings = cfg.validate();
            assert_eq!(
                warnings.iter().any(|w| w.contains("annotation.font_size")),
                said,
                "font_size = {written}: {:?}",
                warnings
            );
            let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
            l.apply_annotation_defaults(&cfg.annotation);
            assert_eq!(
                (cfg.annotation.font_size, l.font_px()),
                (wanted, wanted),
                "font_size = {written}: the file and the pen do not agree"
            );
        }
    }

    #[test]
    fn defaults_that_were_never_edited_leave_the_pen_where_it_was() {
        // The control for the whole round: wiring a key means a config *default* becomes a
        // product behaviour, so an untouched file must hand over the pen the model was
        // already using. Anything else would have moved every user's default without a
        // single test in the tree turning red - the §9.1 第九条 shape, in the other direction.
        let ann = annotated(|_| {});
        let d = Style::default();
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.apply_annotation_defaults(&ann);
        assert_eq!(l.color(), d.color, "the default row reset the pen colour");
        assert_eq!(l.width(), d.width, "the default row reset the pen width");
        assert_eq!(l.font_px(), d.font_size, "the default row reset the size");
        assert_eq!(
            l.font_family(),
            d.font_family,
            "the default row reset the family - [annotation] font_family has to name the
             same word the model does, or the wired key changes the 中文 default"
        );
        assert!(l.problems.is_empty(), "{:?}", l.problems);
        // The one field that does move, and it moves on purpose: width 3 was already a
        // number the toolbar could set, and setting it there links the brush to 12. Leaving
        // the file at 16 would mean the same pen through two doors.
        assert_eq!(
            l.style.brush.size,
            3u32.saturating_mul(4),
            "the brush is the model's 16 while the pen is the file's 3"
        );
    }

    #[test]
    fn a_tool_the_user_taught_outranks_the_default_row() {
        // §5.7.1's sentence is "a tool with no entry takes [annotation]'s values", which is
        // only true in this order. Reversed - tool styles first, defaults second - the
        // default row overwrites every taught pen at each start and the memory does not
        // survive one restart, with all of P18's tests still green because none of them
        // ever reads the `[annotation]` scalars.
        let mut cfg = Config::default();
        cfg.annotation.stroke_color = "#00FF00FF".into();
        cfg.annotation.stroke_width = 5;
        cfg.annotation.tool_style = BTreeMap::from([(
            "rect".into(),
            ToolStyle {
                color: Some("#0000FFFF".into()),
                width: Some(2),
                intensity: None,
            },
        )]);
        assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());

        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.apply_annotation_defaults(&cfg.annotation);
        l.apply_tool_styles(&cfg.annotation.tool_style);

        // Untaught first, so the base is the row the file wrote rather than a pen some
        // other tool left behind - which is the case §5.7.1's fallback is about.
        l.select_tool(code_of(Some(Kind::Arrow)));
        assert_eq!(l.color(), [0, 255, 0, 255], "an untaught tool took nothing");
        assert_eq!(l.width(), 5);
        l.select_tool(code_of(Some(Kind::Rect)));
        assert_eq!(
            l.color(),
            [0, 0, 255, 255],
            "the default row overwrote the lesson"
        );
        assert_eq!(l.width(), 2);
    }

    #[test]
    fn reading_the_file_is_not_the_user_teaching_a_tool() {
        // The defaults setter must not call `remember_style`, and the cost of doing it
        // anyway is invisible in one direction only: the layer would hand `write_back` a
        // table with one invented entry for whichever tool happened to be selected, so the
        // first screenshot after a config edit rewrites the file with a pen nobody set.
        let ann = annotated(|a| a.stroke_color = "#00FF00FF".into());
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.apply_annotation_defaults(&ann);
        l.select_tool(code_of(Some(Kind::Rect)));
        assert!(l.tool_styles().is_empty(), "{:?}", l.tool_styles());

        l.set_width(4);
        let table = l.tool_styles();
        assert_eq!(table.len(), 1, "the pen the user set was not remembered");
        assert_eq!(table["rect"].width, Some(4));
    }

    #[test]
    fn an_empty_family_is_the_file_saying_nothing() {
        // `default_tool` already spells "unset" as an empty string, and the same spelling
        // has to hold here or the pen ends up asking DirectWrite for the empty family -
        // which is not the same question as an unknown one, and nothing in the tree would
        // have noticed the difference.
        let ann = annotated(|a| a.font_family = "   ".into());
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [0, 0, 0, 255]);
        l.apply_annotation_defaults(&ann);
        assert_eq!(l.font_family(), Style::default().font_family);
        assert!(l.problems.is_empty(), "{:?}", l.problems);
    }

    /// The colour question the delivery rows ask: the pen is a hue, not a byte, so a
    /// channel that leads the other two by 40 is what "this pixel is the box" means here.
    fn leads(px: [u8; 4], hot: usize, cold: [usize; 2]) -> bool {
        px[3] == 255 && px[hot] as i32 - px[cold[0]].max(px[cold[1]]) as i32 > 40
    }

    #[test]
    fn the_picture_that_leaves_is_the_selection_not_the_screen() {
        // A hole that starts at (50,40) of a 320x200 screen. The frame that ships has
        // to be the hole's own 120x80 and not the canvas, and its (0,0) has to be the
        // desktop pixel the hole's corner sits on - which is the one place a missing
        // shift shows up as a colour rather than a size.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 320, 200))], [90, 90, 90, 255]);
        l.set_hole(PhysRect::new(50, 40, 120, 80));
        l.set_color([255, 0, 0, 255]);
        l.select_tool(code_of(Some(Kind::Rect)));
        assert!(l.press(PhysPoint::new(70, 60)));
        l.drag(PhysPoint::new(110, 90));
        assert!(l.release());

        let out = l.deliver().expect("a hole inside one screen has a picture");
        assert_eq!(
            (out.width, out.height),
            (120, 80),
            "the crop is not the hole"
        );
        assert_eq!(l.uncovered(), 0, "a hole on one screen covers itself");

        // The box's own corner, in the shipped frame: desktop (70,60) is hole-local
        // (20,20).
        assert!(
            leads(out.get(20, 20), 0, [1, 2]),
            "the ink did not travel: {:?}",
            out.get(20, 20)
        );
        // And the corner of the frame is the desktop pixel under the hole's corner, not
        // the canvas's - the shift the export already had to learn (§9.1 ①).
        let export = l.flatten("m0").unwrap();
        assert_eq!(out.get(0, 0), export.get(50, 40));
        assert_eq!(out.get(119, 79), export.get(169, 119));
    }

    #[test]
    fn a_selection_across_the_seam_is_two_screens_and_not_one_stretch() {
        // The pair the P22 bug lived on: two 640x320 screens whose desktop starts at
        // x = -1280. The hole straddles their seam, and each screen carries a box of a
        // different colour next to it. Stitching one canvas and stretching it to the
        // hole's width cannot pass this, because the two sides ask for different hues
        // and the column that carries the seam has to come from the right screen.
        let left = PhysRect::new(-1280, 0, 640, 320);
        let right = PhysRect::new(-640, 0, 640, 320);
        let mut l = screens(&[("left", left), ("right", right)], [90, 90, 90, 255]);
        let hole = PhysRect::new(-1000, 50, 900, 200);
        l.set_hole(hole);

        // Green ends one pixel before the seam, red starts one pixel after it.
        l.set_color([0, 255, 0, 255]);
        l.select_tool(code_of(Some(Kind::Rect)));
        assert!(l.press(PhysPoint::new(-700, 80)));
        l.drag(PhysPoint::new(-641, 120));
        assert!(l.release());
        l.set_color([255, 0, 0, 255]);
        assert!(l.press(PhysPoint::new(-639, 80)));
        l.drag(PhysPoint::new(-580, 120));
        assert!(l.release());
        assert_eq!(l.objects(), 2);

        let out = l.deliver().expect("a hole over two screens has a picture");
        assert_eq!((out.width, out.height), (900, 200));
        assert_eq!(l.uncovered(), 0, "the seam is covered twice over");

        // Desktop -700 (green box's own corner) is hole-local 300 and desktop -639
        // (red's) is 361 - two different screens, two different hues, both inside one
        // 900-wide frame. A single stretched canvas cannot produce that pairing.
        assert!(
            leads(out.get(300, 30), 1, [0, 2]),
            "the left screen's share is not green: {:?}",
            out.get(300, 30)
        );
        let left_export = l.flatten("left").unwrap();
        assert_eq!(
            out.get(0, 0),
            left_export.get(280, 50),
            "the frame starts on the wrong column of the wrong screen"
        );
        let right_export = l.flatten("right").unwrap();
        assert_eq!(
            out.get(360, 30),
            right_export.get(0, 80),
            "the seam column is not the right screen's own first column"
        );
        assert!(
            leads(out.get(361, 30), 0, [1, 2]),
            "the right screen's share is not red: {:?}",
            out.get(361, 30)
        );
    }

    #[test]
    fn the_part_of_the_selection_no_screen_covers_is_counted_not_assumed() {
        // A 100x100 hole on a 200x100 canvas has the right half of it on no screen at
        // all. The mask clamps a selection to the desktop, so this is not where a user
        // lands - it is where the number the delivery row prints comes from, and the
        // row has to be able to say "half of this picture is nothing" rather than ship
        // it silently.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 200, 100))], [90, 90, 90, 255]);
        l.set_hole(PhysRect::new(150, 50, 100, 100));
        assert_eq!(l.uncovered(), 100 * 100 - 50 * 50);

        let out = l.deliver().expect("a partial hole still has a frame");
        assert_eq!((out.width, out.height), (100, 100));
        // The covered corner is the screen's own pixel, and the uncovered one is what
        // an empty frame is: nothing painted.
        assert_eq!(out.get(0, 0), [90, 90, 90, 255]);
        assert_eq!(out.get(60, 60), [0, 0, 0, 0]);
    }

    #[test]
    fn nothing_selected_ships_nothing() {
        // `deliver` on an empty hole is `None`, not a 0x0 frame: the caller answers the
        // user with a sentence, and a zero-sized bitmap would be a clipboard write that
        // succeeds at nothing.
        let mut l = screens(&[("m0", PhysRect::new(0, 0, 100, 100))], [90, 90, 90, 255]);
        assert!(l.deliver().is_none());
        assert_eq!(l.uncovered(), 0);
        l.set_hole(PhysRect::new(10, 10, 20, 20));
        assert!(l.deliver().is_some());
    }

    /// §5.7.16 步 1-2 到位的那一格：文档里已经有一枚写着什么的对象，手里是「选择」，而那一枚
    /// 在串里。
    ///
    /// 这一格走的是产品自己的两条腿而不是一条新路：[`Layer::commit_typing`] 提交的那一枚就是
    /// 当前对象（§5.7.11 步 5 与 §5.7.16 步 3 是同一段），而放下笔拿起箭头**不动**选中 ——
    /// [`Layer::select_tool`] 只在拿起某一**支笔**时把面板交还画笔。于是「敲完回车紧接着改
    /// 这几个字」与「先点它一下再改」到的是同一个状态，屏幕上也就是这两个入口。
    fn retype_of(text: &str) -> Layer {
        let mut l = typed(text);
        l.select_tool(code_of(None));
        assert_eq!(l.selected_count(), 1, "放下笔把选中丢了");
        assert_eq!(l.objects(), 1);
        l
    }

    #[test]
    fn f2_on_a_selected_text_object_reopens_the_field_on_its_own_letters() {
        let mut l = retype_of("hello");
        let id = l.sel.ids()[0];

        assert!(l.edit_selected_text(), "选中的文字标注上，这一枚键没开门");
        // 框里预先放着的是对象自己那句话（页上只读一次，见 `CaptureMask.qml` 的
        // `onVisibleChanged`），而框开在它自己那一枚的角上而不是新开一处空白。
        assert_eq!(l.typing_text(), "hello", "框里没有对象自己那句话");
        assert_eq!(
            l.typing_rect().map(|r| (r.x, r.y)),
            Some((130, 80)),
            "改字的那只框没有落在对象自己的角上"
        );
        // 开门不写文档：撤销栈上不多一件事，屏幕上那一串还是原来那一串。
        assert_eq!(l.undo.depth(), 1, "开一只框就在撤销栈上记了一件事");
        assert_eq!(l.doc.get(id).unwrap().text, "hello");

        l.type_text("hello world");
        assert!(l.finish_typing(), "改完回车没有交出去");
        assert_eq!(l.doc.get(id).unwrap().text, "hello world");
        assert_eq!(l.objects(), 1, "改字又落了一枚新对象");
        assert!(l.typing_rect().is_none(), "提交之后那只框还亮着");

        // 一次 Ctrl+Z 退回来的是"那一次改字"，不是它的两半。
        assert!(l.undo_step());
        assert_eq!(l.doc.get(id).unwrap().text, "hello", "撤销没有把字退回去");
        assert!(l.redo_step());
        assert_eq!(l.doc.get(id).unwrap().text, "hello world");
    }

    #[test]
    fn the_retype_door_refuses_each_of_the_five_it_cannot_answer() {
        // 五支拒门各有各的拦手，这里一支一支分开数（#453）：合成一句"开不了"就说不出哪一次
        // 是哪一种，而屏幕上那句话（[`crate::mask::MaskState::edit_text`]）要说的是四样事。
        // ① 手里是笔而不是箭头 —— 笔底下点哪都先落**新的一枚**。
        let mut pen = typed("hi");
        assert_eq!(pen.tool(), Some(Kind::Text));
        assert!(!pen.edit_selected_text(), "一支笔开不了改字的框");

        // ② 串里不恰好一枚：空的。
        let mut none = text_layer();
        none.select_tool(code_of(None));
        assert_eq!(none.selected_count(), 0);
        assert!(!none.edit_selected_text(), "什么都没选，没有「这一枚的字」");

        // ②' 多选。
        let mut many = typed("one");
        assert!(many.press(PhysPoint::new(220, 140)), "文本没在第二处落下新框");
        many.type_text("two");
        assert!(many.finish_typing());
        many.select_tool(code_of(None));
        many.set_marquee(true);
        assert!(many.press(PhysPoint::new(110, 60)));
        many.drag(PhysPoint::new(290, 165));
        assert!(many.release());
        many.set_marquee(false);
        assert_eq!(many.selected_count(), 2, "两枚都该在这串里");
        assert!(!many.edit_selected_text(), "多选没有「这一枚的字」");

        // ③ 框已经开着：不许叠第二只 —— [`Layer::typing_text`] 只有一份。
        let mut twice = retype_of("hi");
        assert!(twice.edit_selected_text());
        assert!(!twice.edit_selected_text(), "第二只框叠在第一只上");

        // ④ 编号：它也带 `text`，但画的是 [`Element::number`]，重打那串改的是没人看得见的一行
        // 像素 —— core 那一位作者的 [`Kind::takes_text`]。
        let mut numbered = screens(
            &[("m0", PhysRect::new(0, 0, 120, 80))],
            [70, 70, 70, 255],
        );
        numbered.set_hole(PhysRect::new(0, 0, 120, 80));
        numbered.doc.add(
            Kind::Number,
            Geom::Rect(PhysRect::new(20, 20, 24, 24)),
            Style::default(),
        );
        numbered.select_tool(code_of(None));
        assert!(numbered.press(PhysPoint::new(30, 30)));
        assert_eq!(numbered.selected_count(), 1);
        assert!(!numbered.edit_selected_text(), "编号的字不在这一格里改");

        // ⑤ 锁着的：选中不等于许可（[`Document::can_edit`]），而且这一问在**开门之前**。
        let mut locked = retype_of("hi");
        let id = locked.sel.ids()[0];
        locked.doc.get_mut(id).unwrap().locked = true;
        assert!(!locked.edit_selected_text(), "锁着的该在门这一侧就拒");
        assert!(locked.typing_rect().is_none(), "拒了却还是把框亮给了用户");

        // 每一支拒门都留在"什么都没发生"那一格：没有框，没有新命令，屏幕上那一串还是那一串。
        for (name, l) in [
            ("笔", &pen),
            ("空", &none),
            ("多选", &many),
            ("编号", &numbered),
            ("锁", &locked),
        ] {
            assert!(l.typing_rect().is_none(), "{name} 那一支拒门把框亮给了用户");
        }
        assert!(!none.can_undo(), "拒门上了撤销栈");
        assert_eq!(locked.doc.get(id).unwrap().text, "hi", "拒门改写了字");
    }

    #[test]
    fn the_retype_box_is_measured_in_the_objects_own_font_not_the_pens() {
        // 框的宽高与字号、字体三格读的是同一只框（[`Layer::box_style`]），工具栏那一排读的
        // 是画笔 —— 两位作者各报各的（#281）。这一格把它们岔开：对象自己那一族、两倍字号。
        let mut l = retype_of("hi");
        let id = l.sel.ids()[0];
        let pen_font = l.font_px();
        l.doc.get_mut(id).unwrap().style.font_size = pen_font * 2;
        l.doc.get_mut(id).unwrap().style.font_family = "Consolas".into();

        assert_eq!(l.font_px(), pen_font, "画笔那一格跟着对象跑了");
        assert_ne!(l.font_family(), "Consolas", "工具栏上亮着的字体名换人了");
        assert!(l.edit_selected_text());
        assert_eq!(l.typing_font_px(), pen_font * 2, "框没有按对象自己的字号量");
        assert_eq!(l.typing_font_family(), "Consolas", "框没有按对象自己的字体量");
        // 撑出来的那一格比同一串字按画笔字号量出来的高 —— 页上那枚字段绑的就是这两格，
        // 而 [`crate::mask::MaskState::view_data`] 除的是同一份样本。
        let tall = l.typing_rect().unwrap();
        assert!(
            tall.h > pen_font,
            "按 {} 量的框没有比按 {} 量的高",
            pen_font * 2,
            pen_font
        );
        l.abandon_typing();

        // 没有框的时候这两格与画笔一字不差：屏幕上没有第二只框要它们，而那一格除法在
        // 无框时也要有个数可除（[`crate::mask::MaskState::view_data`] 不问"有没有框"）。
        assert_eq!(l.typing_font_px(), pen_font);
        assert_eq!(l.typing_font_family(), l.font_family());
    }

    /// 同一只框的第三格读数：颜色。页上那一行 `color:` 过去绑的是 `pen_rgb`，于是改字那一只
    /// 框里 preview 的是画笔的颜色，而提交上去的那一枚保留对象自己的 —— 字在框关掉那一刻换
    /// 一次色。这一格把两支岔成两种颜色（只改字号的那一枚证人若同时带上色，量的就不是分岔）。
    #[test]
    fn the_retype_field_shows_the_objects_own_colour_not_the_pens() {
        let mut l = retype_of("hi");
        let id = l.sel.ids()[0];
        let pen = l.color();
        // 一定与画笔不同：翻一枚字节，两支色就岔开了（红是默认的笔色，写死一枚"蓝色"有可能
        // 正好撞上，那这一格量的就是"两边都一样"那一支空臂）。
        let mut other = pen;
        other[0] ^= 0xff;
        l.doc.get_mut(id).unwrap().style.color = other;

        assert_ne!(pen, other, "两支色一样，这一格什么也没量");
        assert_eq!(l.color(), pen, "画笔那一格跟着对象跑了");

        // 还没有框：这一格与画笔一字不差，也就是 §5.7.11 落新框那一只读的那一枚 —— 桥那一头的
        // 打包不问"有没有框"，所以无框时必须有个数可报（[`Layer::box_style`] 同一份答案）。
        assert_eq!(l.typing_colour(), pen, "落新框那一只的颜色不该是对象的那一枚");

        assert!(l.edit_selected_text());
        assert_eq!(l.typing_colour(), other, "字段没有按对象自己的那一枚色");

        // 提交上去的那一枚还是对象自己那一枚 —— 这是"换色"这件事的另一半：字段与落下的那一枚
        // 过去读的不是同一格，屏幕上才会闪那一次。`type_text` 是**整串替换**，所以这里递的是
        // 与框里那句话不同的另一串：一模一样的字交不回命令（core 那一位作者的 `text_command`
        // 「没改动就什么也不交」），那一次断言量的就是没提交。
        l.type_text("hi!");
        assert!(l.finish_typing(), "文本的回车没交出去");
        assert_eq!(
            l.doc.get(id).unwrap().style.color,
            other,
            "改字顺手把对象自己的那一枚色改了"
        );
    }

    #[test]
    fn a_retype_grows_the_frame_for_longer_letters_and_never_shrinks_it() {
        let mut l = retype_of("hi");
        let id = l.sel.ids()[0];
        let small = l.doc.get(id).unwrap().geom.bounds();

        assert!(l.edit_selected_text());
        l.type_text("a far longer line than the two letters that were here");
        assert!(l.finish_typing());
        let wide = l.doc.get(id).unwrap().geom.bounds();
        assert!(wide.w > small.w, "长的那串没有把盒子撑开");
        assert_eq!(
            (wide.x, wide.y),
            (small.x, small.y),
            "撑开的是右下角：那一枚对象没有跑掉"
        );
        // 撑开的那一半与改字的那一半在一条命令里，所以一次 Ctrl+Z 两件一起退。
        assert!(l.undo_step());
        assert_eq!(l.doc.get(id).unwrap().text, "hi");
        assert_eq!(l.doc.get(id).unwrap().geom.bounds(), small, "盒子没跟着退");

        // 反向的那一支：改短了**不**缩回去 —— 那条边可能是 §5.7.11 步 4 手工拉宽的，
        // 收回量出来的宽度会把框从还站在里面的字上挪走。
        assert!(l.redo_step());
        assert_eq!(l.doc.get(id).unwrap().geom.bounds(), wide);
        assert!(l.edit_selected_text());
        l.type_text("no");
        assert!(l.finish_typing());
        assert_eq!(l.doc.get(id).unwrap().text, "no");
        assert_eq!(
            l.doc.get(id).unwrap().geom.bounds(),
            wide,
            "短的那串收回了别人手工拉过的那条边"
        );
    }

    #[test]
    fn a_blank_or_unchanged_retype_files_nothing_and_still_closes() {
        // 与 §5.7.11 那一枚空框同一契约（[`Layer::commit_typing`]）：交不出东西不是错误，
        // 但框要关 —— 而"框里没字所以什么也没交"与"悄悄丢了一半"是两件事，这里分开数。
        let mut l = retype_of("hi");
        let id = l.sel.ids()[0];
        // 栈上此刻有的是"那一次落下这一枚对象"，一位；后面三支拒门都不许再添。
        assert_eq!(l.undo.depth(), 1, "落子那一步就不止一件事");

        assert!(l.edit_selected_text());
        l.type_text("   ");
        assert!(!l.finish_typing(), "三个空格被当成了一次改字");
        assert!(l.typing_rect().is_none(), "拒收之后框还亮着");
        assert_eq!(l.doc.get(id).unwrap().text, "hi", "空格把字盖掉了");
        assert_eq!(l.undo.depth(), 1, "一次没交出去的改字上了栈");

        // 一个字没改：[`falcon_core::annotation::text_command`] 对同一串的答案是不建条。
        assert!(l.edit_selected_text());
        l.type_text("hi");
        assert!(!l.finish_typing(), "没改动也记了一笔");
        assert!(l.typing_rect().is_none());
        assert_eq!(l.undo.depth(), 1);
        assert_eq!(l.doc.get(id).unwrap().text, "hi");

        // 按 Esc 或换工具：收走的只是那只框，原来那一串一个像素也没动过（§9.1 的⑦）。
        assert!(l.edit_selected_text());
        l.type_text("gone?");
        assert!(l.abandon_typing(), "Esc 没吃掉这一枚键");
        assert_eq!(l.doc.get(id).unwrap().text, "hi");
        assert_eq!(l.undo.depth(), 1, "半路丢掉的改字上了栈");
        // 而这一枚对象此刻还在串里：下一次 F2 照样开门。
        assert_eq!(l.selected_count(), 1);
        assert!(l.edit_selected_text());
        assert_eq!(l.typing_text(), "hi");
    }

    #[test]
    fn a_click_elsewhere_hands_the_retype_back_and_is_not_eaten() {
        // §5.7.16 步 4「点击空白区域完成编辑」在改字这一臂上：字先交回文档，然后**这一枚按下
        // 照常往下走** —— 落在把手、单枚选中与空白那三臂之一的点击不该因为框刚关了就少一次。
        let mut l = retype_of("hi");
        let id = l.sel.ids()[0];
        assert!(l.edit_selected_text());
        l.type_text("edited");

        // 落在框里的那一枚到不了这里（页上那枚 `TextEdit` 是指针底下的 item），所以这里
        // 按的是框外：那里此刻没有对象，走到的是空白那一臂。
        assert!(!l.press(PhysPoint::new(250, 150)), "空白那一臂该把按下交回选区");
        assert_eq!(l.doc.get(id).unwrap().text, "edited", "点击空白没有先交字");
        assert!(l.typing_rect().is_none());
        assert_eq!(l.selected_count(), 0, "空白那一臂清了选中");
        // 栈上两件：落子那一次，与这一次改字 —— 一支拒门没上添东西，交出去的那一支添了一件。
        assert_eq!(l.undo.depth(), 2, "点击空白那一次改字没有上栈");
        assert!(l.undo_step());
        assert_eq!(l.doc.get(id).unwrap().text, "hi");
    }
}
