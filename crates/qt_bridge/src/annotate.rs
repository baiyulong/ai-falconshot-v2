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
use falcon_core::annotation::raster::NoGlyphs;
use falcon_core::annotation::undo::UndoStack;
use falcon_core::annotation::{raster, Command, Dirty};
use falcon_core::capture::ScreenSnapshot;
use falcon_core::colors::{format as format_color, ColorFormat};
use falcon_core::config::ToolStyle;
use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect};

use crate::mask::Slot;
use crate::mask_view::shim;

/// The tools the toolbar offers, in the order QML draws them. Index 0 is `None`:
/// the arrow tool, which leaves the selection alone for the pointer and is what
/// §5.7.16/§5.7.17 will hang the object-editing gestures on.
///
/// Deliberately absent, each for a missing capability rather than a missing button:
/// 文本/编号 need the font bridge (§5.7.11/§5.7.12 - [`NoGlyphs`] would draw a
/// background box and call it text), 折线 needs multi-click geometry, 局部放大 needs
/// two drags, and 自由选择/旋转 are M4b/M4c.
pub const TOOLS: &[Option<Kind>] = &[
    None,
    Some(Kind::Rect),
    Some(Kind::RoundedRect),
    Some(Kind::Ellipse),
    Some(Kind::Line),
    Some(Kind::Arrow),
    Some(Kind::DoubleArrow),
    Some(Kind::Pencil),
    Some(Kind::Marker),
    Some(Kind::Mosaic),
    Some(Kind::Blur),
    Some(Kind::Eraser),
];

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
        if self.doc.is_empty() && self.draft.is_empty() {
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
    /// abandons whatever stroke was half-dragged.
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
        if !self.draft.is_empty() {
            self.draft.clear();
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

    pub fn set_color(&mut self, rgba: [u8; 4]) {
        self.style.color = rgba;
        self.remember_style();
    }

    /// Line width and brush diameter move together: §5.7.20's 多级画笔粗细 is one
    /// number the user sets, and a 2 px pencil whose brush is still 16 px wide is a
    /// tool that ignores the knob.
    pub fn set_width(&mut self, width: u32) {
        self.style.width = width.clamp(1, 64);
        Self::link_brush(&mut self.style);
        self.remember_style();
    }

    pub fn set_dashed(&mut self, dashed: bool) {
        self.style.dash = if dashed {
            Dash::Dash(self.style.width * 3, self.style.width * 2)
        } else {
            Dash::Solid
        };
        self.remember_style();
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

    /// 全清: every object out, through one command so it is undoable as one step.
    pub fn clear_ink(&mut self) -> bool {
        let all: Vec<Element> = self.doc.elements.clone();
        if all.is_empty() {
            return false;
        }
        self.apply(Command::Remove(all));
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
        raster::render(&c.base, &view_of(&self.doc.elements, c, dx, dy), &NoGlyphs).ok()
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
        let hole = self.hole_doc();
        let next_rev = self.rev + 1;
        let shifts: Vec<(i32, i32)> = self
            .canvases
            .iter()
            .map(|c| shift(origin, c.bounds))
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
            if let Err(e) = raster::paint(&canvas.base, &doc, &mut canvas.layer, &area, &NoGlyphs) {
                problems.push(format!("repaint of {} failed: {e}", canvas.name));
            }
            // Outside the selection the layer has to show *nothing*, or an undimmed
            // copy of the desktop replaces the dim that QML just drew under it.
            let Some(clip) = hole.intersection(&area) else {
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
}
