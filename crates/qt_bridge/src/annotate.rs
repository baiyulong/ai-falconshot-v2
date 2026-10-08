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

use falcon_core::annotation::model::{Dash, Document, Element, Geom, Kind, Style};
use falcon_core::annotation::undo::UndoStack;
use falcon_core::annotation::{raster, Command, Dirty};
use falcon_core::capture::ScreenSnapshot;
use falcon_core::colors::{format as format_color, ColorFormat};
use falcon_core::config::ToolStyle;
use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect};
use platform_windows::glyphs::DirectWrite;

use crate::mask::Slot;
use crate::mask_view::shim;

/// The tools the toolbar offers, in the order QML draws them. Index 0 is `None`:
/// the arrow tool, which leaves the selection alone for the pointer and is what
/// §5.7.16/§5.7.17 will hang the object-editing gestures on.
///
/// Deliberately absent, each for a missing capability rather than a missing button:
/// 编号 has its digits — [`Kind::Number`] lays out through the same paint path 文本
/// uses, and the counter is in [`Document`] — but not the interaction: §5.7.12's
/// click-to-place and 起始编号 renumbering have no QML surface and no setter on
/// [`Layer`]. 自由选择/旋转 are M4b/M4c.
///
/// 文本 (§5.7.11) is in the list, and so is its gesture: a click places the box, the
/// field that appears takes the typing, and the next click anywhere else commits it.
/// What is still missing is step 4's 字体、字号、对齐 row — the model carries all three
/// and the layer has no control that moves them.
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
    Some(Kind::Zoom),
    Some(Kind::Eraser),
];

/// §5.7.13 step 5's 放大倍数 range, in one pair of numbers shared by the knob and by
/// [`Layer::place`]: two clamps that disagree about what 800% means would light the
/// toolbar cell for a size the copy is not drawn at.
const ZOOM_MIN: u32 = 100;
const ZOOM_MAX: u32 = 800;

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

/// The toolbar's labels, in [`TOOLS`]' order and pipe-separated.
///
/// Rust owns the list because a QML copy of it is a second source of truth that can
/// be renamed out of step with the enum - and index `i` of the labels has to name
/// `TOOLS[i]`, which is the one part of a toolbar that no binding can check.
///
/// The words are the PRD's own §5.7 headings, shortened to what a button holds:
/// 圆角矩形 is §5.7.3 and draws as 圆角.
pub fn tool_names() -> String {
    TOOLS
        .iter()
        .map(|k| match k {
            None => "选择",
            Some(Kind::Rect) => "矩形",
            Some(Kind::RoundedRect) => "圆角",
            Some(Kind::Ellipse) => "椭圆",
            Some(Kind::Line) => "直线",
            Some(Kind::Arrow) => "箭头",
            Some(Kind::DoubleArrow) => "双向",
            Some(Kind::Pencil) => "画笔",
            Some(Kind::Marker) => "荧光",
            Some(Kind::Mosaic) => "马赛克",
            Some(Kind::Blur) => "模糊",
            Some(Kind::Text) => "文本",
            Some(Kind::Number) => "编号",
            Some(Kind::Polyline) => "折线",
            Some(Kind::Zoom) => "放大",
            Some(Kind::Eraser) => "橡皮",
        })
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
    /// selection by [`Layer::doc_point`] like every other gesture's start.
    at: PhysPoint,
    /// Step 3's answer, as the field last had it. The whole string each time, not a
    /// delta: the field owns the caret, the selection and the IME, and a layer that
    /// tried to keep its own copy of an edit it did not see would disagree with it.
    text: String,
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
    /// New objects take this style; each tool keeps its own last one in
    /// [`Layer::styles`], which is §5.7.1's "工具切换时，应记忆该工具最近一次使用的样式".
    style: Style,
    styles: Vec<(Kind, Style)>,
    canvases: Vec<Canvas>,
    /// The desktop origin the document space is normalised by: `device = doc + origin`.
    origin: PhysPoint,
    /// The selection, in desktop pixels. Ink outside it is neither drawn nor kept.
    hole: PhysRect,
    /// Where the stroke being dragged started, in document space.
    anchor: Option<PhysPoint>,
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
            style: Style::default(),
            styles: Vec::new(),
            canvases: Vec::new(),
            origin: PhysPoint::new(0, 0),
            hole: PhysRect::default(),
            anchor: None,
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
        *self = Layer::default();
        self.styles = styles;
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
    pub fn dragging(&self) -> bool {
        self.anchor.is_some()
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
            if let Some(s) = self.remembered(k) {
                self.style = s;
            }
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

    pub fn set_color(&mut self, rgba: [u8; 4]) {
        self.style.color = rgba;
        self.remember_style();
        self.refresh_preview();
    }

    /// Line width and brush diameter move together: §5.7.20's 多级画笔粗细 is one
    /// number the user sets, and a 2 px pencil whose brush is still 16 px wide is a
    /// tool that ignores the knob.
    pub fn set_width(&mut self, width: u32) {
        self.style.width = width.clamp(1, 64);
        Self::link_brush(&mut self.style);
        self.remember_style();
        self.refresh_preview();
    }

    pub fn set_dashed(&mut self, dashed: bool) {
        self.style.dash = if dashed {
            Dash::Dash(self.style.width * 3, self.style.width * 2)
        } else {
            Dash::Solid
        };
        self.remember_style();
        self.refresh_preview();
    }

    /// §5.7.2 step 4's 填充, at a third of the pen's alpha so a filled box still
    /// shows the picture under it.
    pub fn set_filled(&mut self, on: bool) {
        self.style.fill = on.then(|| {
            let mut c = self.style.color;
            c[3] = (c[3] as u32 * 3 / 10) as u8;
            c
        });
        self.remember_style();
        self.refresh_preview();
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

    /// `true` when a drawing tool took the press, so the caller must not also read
    /// it as a grip on the selection.
    pub fn press(&mut self, device: PhysPoint) -> bool {
        let Some(kind) = self.tool else {
            return false;
        };
        if self.hole.is_empty() {
            return false;
        }
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
                });
            }
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
            Geom::Rect(PhysRect::from_points(anchor, p))
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
    pub fn font_px(&self) -> u32 {
        self.style.font_size
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
    fn box_at(&self, at: PhysPoint, text: &str, glyphs: &dyn raster::Glyphs) -> PhysRect {
        let size = raster::text_extent(text, &self.style, glyphs).unwrap_or_else(|| {
            let em = self.style.font_size.max(1);
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

    /// Take the box out of hand and put its text in the document as one
    /// [`Command::Add`], so one Ctrl+Z takes the whole line back. `false` is a box with
    /// nothing in it: [`Layer::release`] refuses to make an undoable nothing out of a
    /// click that drew a dot, and a keystroke history with a blank entry in it is the
    /// same mistake wearing a different hat.
    fn commit_typing(&mut self) -> bool {
        let Some(t) = self.typing.take() else {
            return false;
        };
        if t.text.trim().is_empty() {
            return false;
        }
        let geom = Geom::Rect(self.box_at(t.at, &t.text, &DirectWrite));
        let mut e = Element::new(self.next_id, Kind::Text, geom, self.style.clone());
        e.text = t.text;
        e.z = self.doc.elements.iter().map(|x| x.z).max().unwrap_or(0) + 1;
        self.next_id += 1;
        self.apply(Command::Add(vec![e]));
        true
    }

    /// `Enter` and 双击 accept the box where it stands (§5.7.11 has no third finisher,
    /// and step 5's click-away is in [`Layer::press`]). `false` means "there was no text
    /// here", which is what lets the mask's `Enter` fall through to 完成选区 instead of
    /// eating the keystroke. The order against 折线 and 放大 is
    /// [`crate::mask::MaskState::finish_ink`]'s, not this module's and not QML's.
    pub fn finish_typing(&mut self) -> bool {
        if self.tool != Some(Kind::Text) || self.typing.is_none() {
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
                true
            }
            None => false,
        }
    }

    pub fn redo_step(&mut self) -> bool {
        match self.undo.redo(&mut self.doc) {
            Some(dirty) => {
                self.repaint(&dirty);
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
    use falcon_core::config::Config;

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

        l.end();
        assert!(l.canvases.is_empty(), "the flow did not end");
        assert_eq!(l.objects(), 0);

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
            },
        );
        remembered.insert(
            "arrow".into(),
            ToolStyle {
                color: None,
                width: Some(21),
            },
        );
        remembered.insert(
            "ellipse".into(),
            ToolStyle {
                color: Some("crimson".into()),
                width: None,
            },
        );
        remembered.insert(
            "rec".into(),
            ToolStyle {
                color: Some("#FFB900FF".into()),
                width: Some(4),
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
}
