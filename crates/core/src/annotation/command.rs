//! The commands that change a [`Document`], and the regions they dirty.
//!
//! 技术方案 §8.2 fixes the flow: the core writes the document, *then* records
//! the command. So a command here is a record of a change that already happened
//! — [`Command::apply`] is what redo runs, [`Command::invert`] is what undo runs,
//! and neither is called a second time by the interactive path.
//!
//! Both ends of every change are stored. That is what makes undo exact instead
//! of "replay everything except the last step", and it is still far smaller
//! than a snapshot of the picture: a 4K frame is 33 MB, a command is a few
//! rects.

use crate::geometry::PhysRect;

use super::model::{Document, Element, Geom, Kind, Style, Transform};

/// The region a change may have covered, in picture coordinates. The overlay
/// rasteriser asks the `QQuickImageProvider` for exactly this (M4a).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dirty {
    pub rects: Vec<PhysRect>,
}

impl Dirty {
    pub fn one(r: PhysRect) -> Self {
        Dirty { rects: vec![r] }
    }

    pub fn join(&mut self, r: PhysRect) {
        if !r.is_empty() {
            self.rects.push(r);
        }
    }

    pub fn extend(&mut self, other: &Dirty) {
        self.rects.extend(other.rects.iter().copied());
    }

    /// Trim every rect to the picture. Nothing outside the base can be painted,
    /// and a rect that leaves it entirely costs a texture upload for nothing.
    pub fn clip(&mut self, base: &PhysRect) {
        self.rects.retain_mut(|r| match r.intersection(base) {
            Some(cut) => {
                *r = cut;
                true
            }
            None => false,
        });
    }

    /// One rect covering it all: when the pieces are scattered, a single
    /// texture upload beats several.
    pub fn merged(&self) -> Option<PhysRect> {
        self.rects.iter().fold(None, |acc: Option<PhysRect>, r| {
            Some(match acc {
                Some(a) => a.union(r),
                None => *r,
            })
        })
    }
}

/// §5.7.19: everything the undo stack can hold. The variants follow 技术方案
/// §8.2's list, so `Rotate` and `Reorder` exist even though MVP exposes neither
/// handle.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// The objects as written, newest last. A single stroke is a list of one and
    /// §5.7.18's clone of a selection is a list of many, so one undo step removes
    /// the whole action either way. Re-inserting keeps each id, number and z, so
    /// a redo is byte-identical to the original.
    Add(Vec<Element>),
    /// Full snapshots, so undoing a delete gives the objects back with their z.
    /// Where they end up in the list is not significant on its own — painting
    /// order comes from [`Document::paint_order`], which sorts by z.
    Remove(Vec<Element>),
    /// Dragging an object (§5.7.17 step 4 moves a whole selection at once).
    Move(Vec<(u64, Geom, Geom)>),
    /// Dragging a control point (§5.7.2 step 3, §5.7.3 step 4).
    Resize(Vec<(u64, Geom, Geom)>),
    /// §5.7.15.
    Rotate(Vec<(u64, Transform, Transform)>),
    /// §5.7.16 step 3, and §5.7.21's colour/width change.
    Style(Vec<(u64, Style, Style)>),
    /// §5.7.16 step 3's 文字内容: the string one object carries, both ends of it.
    ///
    /// Not a fifth field of [`Style`] — a style that copied a word would make
    /// §5.7.21's eyedropper pick up someone else's sentence, and the five knobs
    /// that patch one channel of a style would have to say what they do to the text
    /// they were never asked about.
    Text(Vec<(u64, String, String)>),
    /// 技术方案 §8.2 的 ReorderLayer.
    Reorder(Vec<(u64, i32, i32)>),
    /// §5.7.12 step 5: the values changed by a renumber.
    Renumber(Vec<(u64, Option<u32>, Option<u32>)>),
    /// §5.7.12 step 5: 一次删除与它带着的重排，记成**一条**可撤销的编辑。
    ///
    /// 复合而不是给 `Remove` 焊一枚可选的重排尾巴：那两件事各自早就被单独问过（`Renumber`
    /// 是 §5.7.12 步 2 那一排的作者、不带重排的删除是 §5.7.17 步 4 现有的那一条），加尾巴
    /// 等于让每一个造删除的调用方都回答它没问的那一句。前向按顺序 `apply`、后向按**反序**
    /// `invert`，所以一次 Ctrl+Z 走回的是「重排之前、被删的那一枚还在场」——而不是先
    /// 「号回来了、中间还缺一号」、再一次才把东西本身拿回来。
    Compound(Vec<Command>),
    /// Changing the picture's bounds. Both element lists are kept because a
    /// crop drops objects that fall outside and undo has to give them back.
    Crop {
        from: PhysRect,
        to: PhysRect,
        before: Vec<Element>,
        after: Vec<Element>,
    },
}

impl Command {
    /// Forward: put the document into this command's "after" state and report
    /// what to repaint.
    pub fn apply(&self, doc: &mut Document) -> Dirty {
        let mut dirty = Dirty::default();
        match self {
            Command::Add(list) => {
                for e in list {
                    doc.insert(e.clone());
                    dirty.join(e.bounds());
                }
            }
            Command::Remove(list) => {
                for e in list {
                    dirty.join(e.bounds());
                    doc.take(e.id);
                }
            }
            Command::Move(pairs) | Command::Resize(pairs) => {
                for (id, from, to) in pairs {
                    dirty.join(bounded(doc, *id, from));
                    if let Some(e) = doc.get_mut(*id) {
                        e.geom = to.clone();
                    }
                    dirty.join(bounded(doc, *id, to));
                }
            }
            Command::Rotate(pairs) => {
                for (id, _from, to) in pairs {
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.transform = *to;
                    }
                    dirty.join(whole(doc, *id));
                }
            }
            Command::Style(pairs) => {
                for (id, _from, to) in pairs {
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.style = to.clone();
                    }
                    dirty.join(whole(doc, *id));
                }
            }
            Command::Text(pairs) => {
                for (id, _from, to) in pairs {
                    // One join, not the pair 样式 and 重排 make: this variant moves no
                    // geometry and changes no reach, so the box before the write and the
                    // box after it are the same rect, and both the old letters and the
                    // new ones are clipped to it.
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.text = to.clone();
                    }
                }
            }
            Command::Reorder(pairs) => {
                for (id, _from, to) in pairs {
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.z = *to;
                    }
                }
            }
            Command::Renumber(pairs) => {
                for (id, _from, to) in pairs {
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.number = *to;
                    }
                    // 9 → 10 is a wider label, so the new footprint must be in too.
                    dirty.join(whole(doc, *id));
                }
            }
            Command::Compound(parts) => {
                for cmd in parts {
                    dirty.extend(&cmd.apply(doc));
                }
            }
            Command::Crop { to, after, .. } => {
                dirty.join(doc.base);
                doc.base = *to;
                doc.elements = after.clone();
                dirty.join(*to);
            }
        }
        dirty.clip(&doc.base);
        dirty
    }

    /// Backward: the exact state before this command ran.
    pub fn invert(&self, doc: &mut Document) -> Dirty {
        let mut dirty = Dirty::default();
        match self {
            Command::Add(list) => {
                for e in list {
                    dirty.join(e.bounds());
                    doc.take(e.id);
                }
            }
            Command::Remove(list) => {
                for e in list {
                    doc.insert(e.clone());
                    dirty.join(e.bounds());
                }
            }
            Command::Move(pairs) | Command::Resize(pairs) => {
                for (id, from, to) in pairs {
                    dirty.join(bounded(doc, *id, to));
                    if let Some(e) = doc.get_mut(*id) {
                        e.geom = from.clone();
                    }
                    dirty.join(bounded(doc, *id, from));
                }
            }
            Command::Rotate(pairs) => {
                for (id, from, _to) in pairs {
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.transform = *from;
                    }
                    dirty.join(whole(doc, *id));
                }
            }
            Command::Style(pairs) => {
                for (id, from, _to) in pairs {
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.style = from.clone();
                    }
                    dirty.join(whole(doc, *id));
                }
            }
            Command::Text(pairs) => {
                for (id, from, _to) in pairs {
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.text = from.clone();
                    }
                }
            }
            Command::Reorder(pairs) => {
                for (id, from, _to) in pairs {
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.z = *from;
                    }
                }
            }
            Command::Renumber(pairs) => {
                for (id, from, _to) in pairs {
                    dirty.join(whole(doc, *id));
                    if let Some(e) = doc.get_mut(*id) {
                        e.number = *from;
                    }
                    dirty.join(whole(doc, *id));
                }
            }
            Command::Compound(parts) => {
                // 反着走：先把号退回去（被删的那几枚还不在场，`Renumber` 的 pair 本来也
                // 不含它们），再把东西本身请回来。倒着来的这一趟与正着来的那一趟是同一份
                // 状态描述的两端，所以一次 undo 就到位，不需要第二趟。
                for cmd in parts.iter().rev() {
                    dirty.extend(&cmd.invert(doc));
                }
            }
            Command::Crop { from, before, .. } => {
                dirty.join(doc.base);
                doc.base = *from;
                doc.elements = before.clone();
                dirty.join(*from);
            }
        }
        dirty.clip(&doc.base);
        dirty
    }

    /// A conservative region covering both ends, for scheduling a repaint
    /// before the change lands. Clipped to the picture, which is all the overlay
    /// can ever show.
    pub fn bounds(&self, doc: &Document) -> Option<PhysRect> {
        let mut dirty = Dirty::default();
        match self {
            Command::Add(list) | Command::Remove(list) => {
                for e in list {
                    dirty.join(e.bounds());
                }
            }
            Command::Move(pairs) | Command::Resize(pairs) => {
                for (id, from, to) in pairs {
                    dirty.join(bounded(doc, *id, from));
                    dirty.join(bounded(doc, *id, to));
                }
            }
            Command::Rotate(pairs) => {
                for id in ids_of(pairs) {
                    dirty.join(whole(doc, id));
                }
            }
            Command::Style(pairs) => {
                for id in ids_of(pairs) {
                    dirty.join(whole(doc, id));
                }
            }
            Command::Text(pairs) => {
                for id in ids_of(pairs) {
                    dirty.join(whole(doc, id));
                }
            }
            Command::Reorder(pairs) => {
                for id in ids_of(pairs) {
                    dirty.join(whole(doc, id));
                }
            }
            Command::Renumber(pairs) => {
                for id in ids_of(pairs) {
                    dirty.join(whole(doc, id));
                }
            }
            Command::Compound(parts) => {
                // 每一枚子命令自己那一份保守区域，一起收进同一次重绘：复合命令是一次编辑，
                // 屏上不该分两趟刷。
                for cmd in parts {
                    if let Some(b) = cmd.bounds(doc) {
                        dirty.join(b);
                    }
                }
            }
            Command::Crop { from, to, .. } => {
                dirty.join(*from);
                dirty.join(*to);
            }
        }
        dirty.merged().and_then(|r| r.intersection(&doc.base))
    }

    /// §5.7.16's consecutive tweaks are the one mergeable pair (plan M4a: a 400
    /// ms window collapses them into a single undo step).
    pub fn is_style(&self) -> bool {
        matches!(self, Command::Style(_))
    }
}

/// Two style edits over the same set of objects are one change: keep the oldest
/// "from" and the newest "to". Anything else — a different selection, a
/// non-style command — is not mergeable and stays two steps.
pub fn merge_styles(older: &Command, newer: &Command) -> Option<Command> {
    let (Command::Style(a), Command::Style(b)) = (older, newer) else {
        return None;
    };
    let mut ids: Vec<u64> = a.iter().map(|(id, _, _)| *id).collect();
    let mut b_ids: Vec<u64> = b.iter().map(|(id, _, _)| *id).collect();
    ids.sort_unstable();
    b_ids.sort_unstable();
    if ids != b_ids || ids.is_empty() {
        return None;
    }
    let merged = a
        .iter()
        .filter_map(|(id, from, _)| {
            b.iter()
                .find(|(bid, _, _)| bid == id)
                .map(|(_, _, to)| (*id, from.clone(), to.clone()))
        })
        .collect();
    Some(Command::Style(merged))
}

/// The element's whole footprint as it stands, for changes that do not move the
/// geometry but can change its reach — a thicker pen, a wider brush.
fn whole(doc: &Document, id: u64) -> PhysRect {
    doc.get(id)
        .map(|e| e.bounds())
        .unwrap_or_else(|| PhysRect::new(0, 0, 1, 1))
}

fn bounded(doc: &Document, id: u64, g: &Geom) -> PhysRect {
    let slop = doc.get(id).map(|e| e.reach()).unwrap_or(2);
    g.bounds().inflate(slop)
}

/// Every `(id, from, to)` triple starts with the id, whatever the two ends are.
fn ids_of<T>(pairs: &[(u64, T, T)]) -> impl Iterator<Item = u64> + '_ {
    pairs.iter().map(|(id, _, _)| *id)
}

/// Read the "from" out of the document and compute the "to" for each editable
/// object in a selection. An unknown id aborts the whole command — a stale
/// selection means the caller is about to describe a change to something that is
/// no longer there. Nothing editable produces no command at all.
fn edit_pairs<T, F, G>(doc: &Document, ids: &[u64], from: F, to: G) -> Option<Vec<(u64, T, T)>>
where
    T: Clone,
    F: Fn(&Element) -> T,
    G: Fn(&Element) -> T,
{
    let mut pairs = Vec::new();
    for id in ids {
        let e = doc.get(*id)?;
        if doc.can_edit(*id) {
            pairs.push((*id, from(e), to(e)));
        }
    }
    if pairs.is_empty() {
        return None;
    }
    Some(pairs)
}

/// Build the crop command. Objects are not cut, they move with the origin: what
/// hangs off the new edge simply is not painted, and what falls entirely outside
/// is dropped (and comes back on undo).
pub fn crop_command(doc: &Document, to: &PhysRect) -> Option<Command> {
    let kept = to.intersection(&doc.base)?;
    if kept.is_empty() {
        return None;
    }
    let after: Vec<Element> = doc
        .elements
        .iter()
        .filter(|e| kept.intersection(&e.bounds()).is_some())
        .map(|e| {
            let mut c = e.clone();
            c.geom = e.geom.translate(-kept.x, -kept.y);
            c
        })
        .collect();
    Some(Command::Crop {
        from: doc.base,
        to: kept,
        before: doc.elements.clone(),
        after,
    })
}

/// Translate several objects as one undoable step (§5.7.17 step 4). Locked
/// objects are skipped: selection is not permission to edit.
pub fn move_command(doc: &Document, ids: &[u64], dx: i32, dy: i32) -> Option<Command> {
    let pairs = edit_pairs(doc, ids, |e| e.geom.clone(), |e| e.geom.translate(dx, dy))?;
    Some(Command::Move(pairs))
}

/// The same, for control-point drags: one target geometry per id (§5.7.3 step
/// 4 adjusts the size and the corner radius together).
pub fn resize_command(doc: &Document, targets: &[(u64, Geom)]) -> Option<Command> {
    let mut pairs = Vec::new();
    for (id, to) in targets {
        let e = doc.get(*id)?;
        if !doc.can_edit(*id) {
            continue;
        }
        pairs.push((*id, e.geom.clone(), to.clone()));
    }
    if pairs.is_empty() {
        return None;
    }
    Some(Command::Resize(pairs))
}

/// Style the whole selection in one step (§5.7.17 step 4).
pub fn style_command(doc: &Document, ids: &[u64], to: &Style) -> Option<Command> {
    let pairs = edit_pairs(doc, ids, |e| e.style.clone(), |_| to.clone())?;
    Some(Command::Style(pairs))
}

/// §5.7.16 step 3's 修改颜色、线宽、透明度: one **channel** of the style, over a selection,
/// as the one [`Command::Style`] the undo stack needs.
///
/// [`style_command`] replaces an entire style, which is right for "make these objects use
/// this pen" and wrong for "make these objects 50% transparent": a marquee of a red 3 px
/// box and an 18 pt text would leave with both wearing whichever style the panel was
/// holding, losing a width and a font nobody asked to change. A patch runs against each
/// object's *own* style, so the field the knob owns moves and the fields it does not stay.
///
/// Every pair is kept even when it changes nothing, because [`merge_styles`] only merges
/// two steps whose id sets are equal — dropping the quiet objects here would make the
/// second notch of one wheel drag a second undo step. What is dropped instead is the
/// whole command when *nothing* moved: a wheel spun onto the value the panel is already
/// showing is not something the user can be asked to undo.
pub fn patch_style_command<F>(doc: &Document, ids: &[u64], patch: F) -> Option<Command>
where
    F: Fn(&Element, &mut Style),
{
    let pairs = edit_pairs(
        doc,
        ids,
        |e| e.style.clone(),
        |e| {
            let mut s = e.style.clone();
            patch(e, &mut s);
            s
        },
    )?;
    if pairs.iter().all(|(_, from, to)| from == to) {
        return None;
    }
    Some(Command::Style(pairs))
}

/// §5.7.16's 透明度 for the selected objects, off the ladder in [`alpha_from_pct`] and
/// through [`Style::set_alpha_pct`] — so a re-edit and a pen change cannot disagree about
/// what 50% means, and a filled box keeps the plate it was filled with.
pub fn alpha_command(doc: &Document, ids: &[u64], percent: u32) -> Option<Command> {
    patch_style_command(doc, ids, move |_e, s| s.set_alpha_pct(percent))
}

/// §5.7.16's 颜色: the three hue bytes, with every object's own alpha left where it is.
/// The toolbar's colour band carries no alpha (its wheel is the fourth byte's author), so
/// picking a chip must not silently set the whole selection opaque.
pub fn colour_command(doc: &Document, ids: &[u64], rgb: [u8; 3]) -> Option<Command> {
    patch_style_command(doc, ids, move |_e, s| {
        let a = s.color[3];
        s.color = [rgb[0], rgb[1], rgb[2], a];
        if s.fill.is_some() {
            s.fill = Some(Style::translucent(s.color));
        }
    })
}

/// §5.7.16's 字号, clamped by the one pair of numbers the config validates against.
pub fn font_size_command(doc: &Document, ids: &[u64], size: u32) -> Option<Command> {
    patch_style_command(doc, ids, move |_e, s| s.set_font_size(size))
}

/// §5.7.16's 线宽. No clamp here: the ladder of 多级画笔粗细 is the layer's and it already
/// clamps what a cell can set ([`crate::annotation::command`] has no business being a
/// second one). The brush follows the pen through [`Style::link_brush`]'s rule, which the
/// caller owns; a width re-edit that left a 16 px brush on a 2 px pencil would be the
/// knob-that-ignores-the-tool defect in the other direction.
pub fn width_command(doc: &Document, ids: &[u64], width: u32) -> Option<Command> {
    patch_style_command(doc, ids, move |_e, s| s.width = width)
}

/// §5.7.9's 块大小 / §5.7.10's 模糊强度 for the selected objects, written **as each
/// object's own effect**: a marquee of a mosaic and a blur given the number 20 leaves the
/// mosaic at a 20 px block and the blur at a 20 px radius, because the kind decides the
/// shape ([`Style::set_intensity`]) and only the number is shared.
pub fn intensity_command(doc: &Document, ids: &[u64], value: u32) -> Option<Command> {
    patch_style_command(doc, ids, move |e, s| s.set_intensity(e.kind, value))
}

/// §5.7.3's 圆角半径 for the selected objects, clamped by [`Style::set_corner_radius`]'s
/// one ladder.
///
/// It lives here rather than at each caller because §5.7.3 step 4 has **two** hands that
/// ask for it: the toolbar's 更大/更小 cells and the control point dragged on the picture
/// ([`crate::annotation::select::radius_grip_command`]). A radius the round handle sets is
/// the same one undoable [`Command::Style`] the cells file, which is what §5.7.19 asks of
/// an edit and what a half command — geometry without the corner — would quietly lose.
pub fn corner_command(doc: &Document, ids: &[u64], radius: u32) -> Option<Command> {
    patch_style_command(doc, ids, move |_e, s| s.set_corner_radius(radius))
}

/// §5.7.16 step 3's 文字内容: one object's string, both ends of it.
///
/// A channel of its own rather than a sixth field of [`Style`]: the five re-edit knobs
/// above patch one *style* channel each and would otherwise have to answer what they do
/// to a word they were never asked about, and §5.7.21's 吸管 copies a style — with the
/// string in there, picking up a colour would pick up someone else's sentence.
///
/// The box that follows the string is not this command's: the measure needs the leg that
/// draws the glyphs, which the caller holds, so the two halves arrive as one
/// [`Command::Compound`] and stay one undo step (the same split
/// [`delete_and_renumber_command`] makes between the delete and the renumber).
pub fn text_command(doc: &Document, id: u64, to: &str) -> Option<Command> {
    let e = doc.get(id)?;
    if !e.kind.takes_text() || !doc.can_edit(id) || e.text == to {
        return None;
    }
    Some(Command::Text(vec![(id, e.text.clone(), to.to_string())]))
}

/// §5.7.13 step 5 and §5.7.3 step 4 want the size change expressed as a rect,
/// so this is the one resize constructor that takes a rect instead of a Geom.
pub fn resize_to_rect(doc: &Document, id: u64, to: PhysRect) -> Option<Command> {
    resize_command(doc, &[(id, Geom::Rect(to))])
}

/// §5.7.18 step 2: Ctrl+drag, or copy and paste. The copies are numbered here, so the
/// command that comes back already describes the finished change — and one undo removes
/// the whole clone rather than one copy at a time.
///
/// Like every other builder in this module it *describes* the step and writes nothing:
/// `Command::Add` is what puts elements in the document, which is the difference between
/// a clone landing once and landing twice.
pub fn clone_command(doc: &Document, ids: &[u64], dx: i32, dy: i32) -> Option<Command> {
    let copies = doc.clone_elements(ids, dx, dy);
    if copies.is_empty() {
        return None;
    }
    Some(Command::Add(copies))
}

/// Delete the selection (§5.7.17 step 4, and the Eraser's object-level erase).
/// The snapshots keep the document's own order, so an undo gives the objects back
/// exactly as they were listed.
pub fn remove_command(doc: &Document, ids: &[u64]) -> Option<Command> {
    let list: Vec<Element> = doc
        .elements
        .iter()
        .filter(|e| ids.contains(&e.id))
        .cloned()
        .collect();
    if list.is_empty() {
        return None;
    }
    Some(Command::Remove(list))
}

/// §5.7.12 step 5: 删掉中间一枚编号，并把「后续编号自动重排」与那一次删除记成**一条**命令。
///
/// 起点不在这里挑：它问的是 [`Document::first_number`] —— 删过之后屏幕上第一枚还戴着的号，
/// 不是常量 1（起点若取常量，用户把起始编号设到 7 之后删一枚，整排号会被拉回 1，那是这一排
/// 钮从没被按过的那一句话）。重排在一份 `trial` 上算，理由与 [`clone_command`] 头顶那句相同：
/// 本模块的作者只**描述**这一步、不落笔，落笔的是调用它的 `Command::apply`。
///
/// 四臂各有其词：串里没有在场的 id ⇒ `None`（无事可做，也就不该多一条栈）；这一趟删的**不是
/// 编号** ⇒ 只交回那条 `Remove` —— PRD 那句是「删除中间**编号**时可选择是否自动重排」，删一枚
/// 矩形不该把上一次留着的那串空档填上，那是另一次删除或另一枚钮的事；场上没有编号、或留着的
/// 那几枚本来就顺 ⇒ 同样只交回 `Remove`，一趟没改动的重排不配第二条记录；两件都改了
/// ⇒ [`Command::Compound`]。
///
/// 计数器不归这一趟走：`renumber` 在 `trial` 上顶高的那一格不回灌真档，于是删完再重排之后，
/// 下一枚号仍是「这台机器还没发出去的那一枚」。这不是遗漏——它与 [`Document::renumber`] 头顶
/// 那句「计数器永不回退，与 id 同一规矩」是同一条口径，重排改的是**屏幕上已有**的号。
pub fn delete_and_renumber_command(doc: &Document, ids: &[u64]) -> Option<Command> {
    let removed: Vec<Element> = doc
        .elements
        .iter()
        .filter(|e| ids.contains(&e.id))
        .cloned()
        .collect();
    if removed.is_empty() {
        return None;
    }
    if !removed.iter().any(|e| e.kind == Kind::Number) {
        return Some(Command::Remove(removed));
    }
    let mut trial = doc.clone();
    for e in &removed {
        trial.take(e.id);
    }
    let delete = Command::Remove(removed);
    let Some(start) = trial.first_number() else {
        return Some(delete);
    };
    let pairs = trial.renumber(start);
    if pairs.is_empty() {
        return Some(delete);
    }
    Some(Command::Compound(vec![delete, Command::Renumber(pairs)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::model::{
        alpha_from_pct, Brush, Kind, CORNER_MAX, EFFECT_MIN, FONT_MAX, MARKER_ALPHA,
    };
    use crate::geometry::PhysPoint;
    use crate::imageops::{Effect, Shape};

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Geom {
        Geom::Rect(PhysRect::new(x, y, w, h))
    }

    fn doc_with_two() -> (Document, Vec<u64>) {
        let mut doc = Document::new(100, 100);
        let a = doc.add(Kind::Rect, rect(10, 10, 20, 20), Style::default());
        let b = doc.add(Kind::Ellipse, rect(50, 50, 20, 20), Style::default());
        (doc, vec![a.id, b.id])
    }

    #[test]
    fn add_is_recorded_not_replayed_and_redo_restores_it_exactly() {
        let (mut doc, _) = doc_with_two();
        let before = doc.clone();
        // The interactive path writes the document and records the element.
        let written = doc.add(Kind::Text, rect(0, 0, 8, 8), Style::default());
        let cmd = Command::Add(vec![written]);
        assert_eq!(doc.len(), 3);
        // Undo takes it away again...
        cmd.invert(&mut doc);
        assert_eq!(doc.elements, before.elements);
        // ...and redo puts the same element back, id, z and all.
        cmd.apply(&mut doc);
        assert_eq!(doc.get(3).unwrap().kind, Kind::Text);
        assert_eq!(doc.get(3).unwrap().z, 3);
        cmd.invert(&mut doc);
        assert_eq!(doc.elements, before.elements);
        // Undoing an Add does not hand its id back: a recycled id would make an
        // older Style step in the stack point at something else entirely.
        assert_eq!(
            doc.add(Kind::Rect, rect(0, 0, 1, 1), Style::default()).id,
            4
        );
    }

    #[test]
    fn remove_snapshots_restore_place_number_and_order() {
        let (mut doc, _) = doc_with_two();
        doc.add(Kind::Number, rect(5, 5, 4, 4), Style::default());
        doc.add(Kind::Number, rect(70, 5, 4, 4), Style::default());
        let before = doc.clone();
        let removed: Vec<Element> = doc
            .elements
            .iter()
            .filter(|e| e.kind == Kind::Number)
            .cloned()
            .collect();
        let cmd = Command::Remove(removed);
        cmd.apply(&mut doc);
        assert_eq!(doc.len(), 2);
        cmd.invert(&mut doc);
        assert_eq!(doc, before);
        // A redo of a delete that is followed by new drawing keeps the ids: the
        // numbers are gone from the document but `next_id` never rewinds.
        cmd.apply(&mut doc);
        doc.add(Kind::Rect, rect(1, 1, 2, 2), Style::default());
        assert_eq!(doc.elements.last().unwrap().id, 5);
        assert!(doc.get(3).is_none() && doc.get(4).is_none());
    }

    #[test]
    fn a_move_carries_the_whole_geometry_and_reverses_exactly() {
        let (mut doc, ids) = doc_with_two();
        let before = doc.clone();
        let cmd = move_command(&doc, &ids, 7, -4).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(1).unwrap().geom, rect(17, 6, 20, 20));
        assert_eq!(doc.get(2).unwrap().geom, rect(57, 46, 20, 20));
        cmd.invert(&mut doc);
        assert_eq!(doc, before);
    }

    #[test]
    fn a_locked_object_is_not_moved_by_a_selection_drag() {
        let (mut doc, ids) = doc_with_two();
        doc.get_mut(2).unwrap().locked = true;
        let cmd = move_command(&doc, &ids, 5, 5).unwrap();
        match &cmd {
            Command::Move(pairs) => assert_eq!(pairs.len(), 1),
            other => panic!("{other:?}"),
        }
        cmd.apply(&mut doc);
        assert_eq!(doc.get(2).unwrap().geom, rect(50, 50, 20, 20));
        // A selection of nothing editable produces no command at all.
        doc.get_mut(1).unwrap().locked = true;
        assert!(move_command(&doc, &ids, 1, 1).is_none());
    }

    #[test]
    fn a_style_change_is_undoable_and_reports_only_the_touched_objects() {
        let (mut doc, ids) = doc_with_two();
        let to = Style {
            width: 9,
            ..Style::default()
        };
        let cmd = style_command(&doc, &ids, &to).unwrap();
        let dirty = cmd.apply(&mut doc);
        assert_eq!(doc.get(2).unwrap().style.width, 9);
        assert!(dirty.merged().unwrap().contains(PhysPoint::new(10, 10)));
        assert!(dirty.merged().unwrap().contains(PhysPoint::new(69, 69)));
        cmd.invert(&mut doc);
        assert_eq!(doc.get(1).unwrap().style.width, 3);
        assert_eq!(doc, {
            let (fresh, _) = doc_with_two();
            fresh
        });
    }

    #[test]
    fn consecutive_style_tweaks_of_the_same_objects_merge_into_one_step() {
        let (mut doc, ids) = doc_with_two();
        let before = doc.clone();
        let a = Style {
            width: 5,
            ..Style::default()
        };
        let b = Style {
            width: 9,
            color: [0, 0, 255, 255],
            ..Style::default()
        };
        let first = style_command(&doc, &ids, &a).unwrap();
        first.apply(&mut doc);
        let second = style_command(&doc, &ids, &b).unwrap();
        let merged = merge_styles(&first, &second).expect("same id set");
        second.apply(&mut doc);
        // One undo of the merged command lands on the original style, not on a.
        merged.invert(&mut doc);
        assert_eq!(doc, before);
        // A different selection does not merge.
        let one = style_command(&doc, &[1], &a).unwrap();
        let two = style_command(&doc, &[2], &a).unwrap();
        assert!(merge_styles(&one, &two).is_none());
        assert!(merge_styles(&Command::Add(vec![doc.get(1).unwrap().clone()]), &two).is_none());
    }

    #[test]
    fn rotate_reorder_and_renumber_round_trip() {
        let (mut doc, _) = doc_with_two();
        let before = doc.clone();
        let tf = Transform {
            rotation: 30.0,
            flip_h: true,
            flip_v: false,
        };
        let cmd = Command::Rotate(vec![(1, Transform::default(), tf)]);
        cmd.apply(&mut doc);
        assert_eq!(doc.get(1).unwrap().transform, tf);
        cmd.invert(&mut doc);
        assert_eq!(doc, before);

        let cmd = Command::Reorder(vec![(1, 1, 50), (2, 2, 40)]);
        cmd.apply(&mut doc);
        assert_eq!(
            doc.paint_order().iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![2, 1]
        );
        cmd.invert(&mut doc);
        assert_eq!(doc, before);

        // Renumber goes through the document writer, which returns the pairs.
        for x in [0, 30, 60] {
            doc.add(Kind::Number, rect(x, 80, 4, 4), Style::default());
        }
        let pairs = doc.renumber(9);
        let cmd = Command::Renumber(pairs);
        assert_eq!(
            doc.elements
                .iter()
                .filter(|e| e.kind == Kind::Number)
                .map(|e| e.number)
                .collect::<Vec<_>>(),
            vec![Some(9), Some(10), Some(11)]
        );
        cmd.invert(&mut doc);
        assert_eq!(
            doc.elements
                .iter()
                .filter(|e| e.kind == Kind::Number)
                .map(|e| e.number)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3)]
        );
        // Undoing a renumber does not rewind the counter: the next click must not
        // hand out a number that still has an element of its own in the redo stack.
        assert_eq!(doc.next_counter, 12);
        cmd.apply(&mut doc);
        assert_eq!(doc.get(5).unwrap().number, Some(11));
    }

    /// 屏上的读数：paint order 里每一枚的 (id, 号)。撤销一条删除把元素交回列表**末尾**
    /// （`Document::insert` 认 z、不认位次），所以「走回原样」只能比这个序列，不能比
    /// `elements` 那一条。
    fn worn(doc: &Document) -> Vec<(u64, Option<u32>)> {
        doc.paint_order().iter().map(|e| (e.id, e.number)).collect()
    }

    fn three_numbers() -> Document {
        let mut doc = Document::new(100, 100);
        for x in [0, 30, 60] {
            doc.add(Kind::Number, rect(x, 10, 4, 4), Style::default());
        }
        doc
    }

    #[test]
    fn deleting_a_middle_number_undoes_the_renumber_with_the_delete() {
        // §5.7.12 步 5 那一枚「是否自动重排」的判据：一次删除带着重排 = 栈上**一条**记录，
        // 一次 Ctrl+Z 把被删的那一枚和被挪走的号一起请回来。
        let mut doc = three_numbers();
        let before = doc.clone();

        let cmd = delete_and_renumber_command(&doc, &[2]).expect("id 2 在场");
        assert!(
            matches!(cmd, Command::Compound(_)),
            "删除与它带着的重排是一条命令，不是两条"
        );
        assert!(
            cmd.apply(&mut doc).merged().is_some(),
            "两件事都在那一次重绘里"
        );
        assert_eq!(worn(&doc), vec![(1, Some(1)), (3, Some(2))], "屏幕不留空档");
        assert_eq!(
            doc.next_counter, 4,
            "重排不碰计数器：下一枚仍是没发出去的那个号"
        );

        cmd.invert(&mut doc);
        assert_eq!(doc, before, "一次 undo 走回原样，连那张表的位次也一样");
    }

    #[test]
    fn undoing_a_delete_gives_the_objects_back_where_they_were_listed() {
        // `remove_command` 头顶那句「snapshots keep the document's own order」的证词：撤销
        // 中间那几枚的删除，不能把它们接成列表的最后一名。不带重排的那一条是 §5.7.17 步 4
        // 现有的那一条，与上面那条复合命令共用同一个落点。
        let mut doc = three_numbers();
        doc.add(Kind::Rect, rect(80, 10, 4, 4), Style::default());
        let before = doc.clone();
        let cmd = remove_command(&doc, &[2, 4]).expect("both are on the board");
        cmd.apply(&mut doc);
        assert_eq!(
            doc.elements.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        cmd.invert(&mut doc);
        assert_eq!(doc, before);
    }

    #[test]
    fn a_renumber_that_changes_nothing_records_only_the_delete() {
        // 删的是末尾那一枚：留着的那几枚本来就顺，一趟没改动的重排不配第二条记录。
        let mut doc = three_numbers();
        let cmd = delete_and_renumber_command(&doc, &[3]).expect("id 3 在场");
        assert!(matches!(cmd, Command::Remove(_)), "这一趟只有删除这一件事");
        cmd.apply(&mut doc);
        assert_eq!(worn(&doc), vec![(1, Some(1)), (2, Some(2))]);
        cmd.invert(&mut doc);
        assert_eq!(worn(&doc), vec![(1, Some(1)), (2, Some(2)), (3, Some(3))]);

        // 串里没有在场的 id ⇒ 一条栈都不该多。
        assert!(delete_and_renumber_command(&doc, &[9]).is_none());
    }

    #[test]
    fn a_deletion_that_removed_no_number_touches_no_numbers() {
        // 上一趟删除留着的那个空档（场上只剩 1 与 3）不是这一趟的理由：PRD 那句问的是
        // 「删除中间**编号**时」，删一枚矩形就把号补齐，是替另一枚从没按过的钮落笔。
        let mut doc = three_numbers();
        doc.take(2);
        doc.add(Kind::Rect, rect(80, 10, 4, 4), Style::default());
        let cmd = delete_and_renumber_command(&doc, &[4]).expect("那枚矩形在场");
        assert!(matches!(cmd, Command::Remove(_)), "这一趟删的不是编号");
        cmd.apply(&mut doc);
        assert_eq!(
            doc.elements
                .iter()
                .filter(|e| e.kind == Kind::Number)
                .map(|e| e.number)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(3)],
            "那个空档仍是上一次那道伤口，不是这一次的"
        );
    }

    #[test]
    fn the_renumber_starts_at_the_first_number_on_screen_not_at_one() {
        // 起始编号设到 7 之后删中间一枚：整排号要留在 7 那一头。回到常量 1 是这一排钮从没
        // 被按过的那一句话。
        let mut doc = Document::new(100, 100);
        doc.set_counter_start(7);
        for x in [0, 30, 60] {
            doc.add(Kind::Number, rect(x, 10, 4, 4), Style::default());
        }
        assert_eq!(doc.first_number(), Some(7));

        let cmd = delete_and_renumber_command(&doc, &[2]).expect("中间那一枚");
        cmd.apply(&mut doc);
        assert_eq!(worn(&doc), vec![(1, Some(7)), (3, Some(8))]);
    }

    #[test]
    fn a_crop_moves_the_origin_and_keeps_only_what_it_touched() {
        let (mut doc, _) = doc_with_two();
        doc.add(Kind::Rect, rect(90, 90, 6, 6), Style::default());
        let before = doc.clone();
        let cmd = crop_command(&doc, &PhysRect::new(5, 5, 60, 60)).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.base, PhysRect::new(5, 5, 60, 60));
        assert_eq!(doc.get(1).unwrap().geom, rect(5, 5, 20, 20));
        assert_eq!(doc.len(), 2);
        cmd.invert(&mut doc);
        assert_eq!(doc, before);
        // A request entirely outside the picture is refused.
        assert!(crop_command(&doc, &PhysRect::new(500, 500, 10, 10)).is_none());
        // And a crop of nothing at all is not a command.
        let empty = Document::new(10, 10);
        assert!(crop_command(&empty, &PhysRect::new(0, 0, 100, 100)).is_some());
        assert_eq!(empty.base, PhysRect::new(0, 0, 10, 10));
    }

    #[test]
    fn dirty_rects_are_inflated_by_the_pen_and_cover_both_positions() {
        let (mut doc, _) = doc_with_two();
        let fat = Style {
            brush: Brush {
                size: 40,
                shape: Shape::Rect,
                feather: 0,
            },
            effect: Effect::Mosaic { block: 4 },
            ..Style::default()
        };
        doc.add(Kind::Mosaic, rect(0, 0, 10, 10), fat);
        let id = 3;
        let cmd = move_command(&doc, &[id], 3, 3).unwrap();
        let dirty = cmd.apply(&mut doc);
        // A 40 px brush reaches 20 past the box, so the two positions together
        // span -20..33 on each axis. A repaint cannot start outside the picture,
        // which is what clipping the dirty rects to the base is for.
        assert_eq!(dirty.merged(), Some(PhysRect::new(0, 0, 33, 33)));
        assert_eq!(doc.get(id).unwrap().geom, rect(3, 3, 10, 10));
        // The repaint region an unknown object yields is a point, not a panic.
        assert_eq!(whole(&doc, 999), PhysRect::new(0, 0, 1, 1));
    }

    #[test]
    fn command_bounds_are_clipped_to_the_picture() {
        let (mut doc, ids) = doc_with_two();
        let cmd = move_command(&doc, &ids, 30, 0).unwrap();
        assert_eq!(cmd.bounds(&doc), Some(PhysRect::new(8, 8, 92, 64)));
        assert_eq!(doc.get(1).unwrap().geom, rect(10, 10, 20, 20));
        doc.base = PhysRect::new(0, 0, 40, 40);
        assert_eq!(cmd.bounds(&doc), Some(PhysRect::new(8, 8, 32, 32)));
    }

    #[test]
    fn re_editing_a_selection_moves_one_channel_and_undoes_as_one_step() {
        // §5.7.16 step 3 on two objects that were never the same: a 3 px red box and an
        // 18 pt text. The alpha wheel over them both must land both on 50% and leave the
        // width, the font and the hue each object already had.
        let (mut doc, ids) = doc_with_two();
        doc.get_mut(2).unwrap().style.font_size = 18;
        doc.get_mut(1).unwrap().style.color = [232, 17, 35, 255];
        doc.get_mut(2).unwrap().style.color = [0, 120, 200, 255];
        let before = doc.clone();
        let cmd = alpha_command(&doc, &ids, 50).expect("both change");
        let dirty = cmd.apply(&mut doc);
        assert_eq!(doc.get(1).unwrap().style.color[3], MARKER_ALPHA);
        assert_eq!(doc.get(2).unwrap().style.color[3], MARKER_ALPHA);
        assert_eq!(doc.get(1).unwrap().style.color[..3], [232, 17, 35]);
        assert_eq!(doc.get(2).unwrap().style.color[..3], [0, 120, 200]);
        assert_eq!(
            doc.get(1).unwrap().style.width,
            3,
            "the wheel is not a pen width"
        );
        assert_eq!(doc.get(2).unwrap().style.font_size, 18);
        assert!(dirty.merged().unwrap().contains(PhysPoint::new(10, 10)));
        // One undo of the one command, and both objects are back exactly.
        cmd.invert(&mut doc);
        assert_eq!(doc, before);
        // And the merged-with-a-second-notch case: still one step, still to the start.
        let first = alpha_command(&doc, &ids, 40).unwrap();
        first.apply(&mut doc);
        let second = alpha_command(&doc, &ids, 30).unwrap();
        second.apply(&mut doc);
        let merged = merge_styles(&first, &second).expect("the same two objects");
        merged.invert(&mut doc);
        assert_eq!(doc, before, "one Ctrl+Z for the whole wheel drag");
    }

    #[test]
    fn a_channel_that_is_already_there_is_no_undo_step() {
        let (mut doc, ids) = doc_with_two();
        doc.get_mut(1).unwrap().style.color[3] = alpha_from_pct(50);
        doc.get_mut(2).unwrap().style.color[3] = alpha_from_pct(50);
        assert!(
            alpha_command(&doc, &ids, 50).is_none(),
            "spinning the wheel onto the number the panel is showing files nothing"
        );
        // A selection where only one object moves still files one command over both, so
        // the two steps a wheel drag produces keep merging into one.
        doc.get_mut(2).unwrap().style.color[3] = 255;
        match alpha_command(&doc, &ids, 50).unwrap() {
            Command::Style(pairs) => assert_eq!(pairs.len(), 2),
            other => panic!("{other:?}"),
        }
        // A locked object is not edited, and a selection of only that is no command.
        let (mut doc2, ids2) = doc_with_two();
        doc2.get_mut(1).unwrap().locked = true;
        doc2.get_mut(2).unwrap().locked = true;
        assert!(alpha_command(&doc2, &ids2, 20).is_none());
        assert!(colour_command(&doc2, &ids2, [1, 2, 3]).is_none());
    }

    #[test]
    fn the_strength_of_a_mixed_selection_is_written_by_each_object_s_own_kind() {
        // §5.7.9 and §5.7.10 out of one marquee: the number is shared, the shape is not.
        let mut doc = Document::new(100, 100);
        let m = doc
            .add(
                Kind::Mosaic,
                rect(0, 0, 10, 10),
                Style {
                    effect: Effect::Mosaic { block: 8 },
                    ..Style::default()
                },
            )
            .id;
        let b = doc
            .add(
                Kind::Blur,
                rect(20, 20, 10, 10),
                Style {
                    effect: Effect::Blur { radius: 12 },
                    ..Style::default()
                },
            )
            .id;
        let cmd = intensity_command(&doc, &[m, b], 20).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(
            doc.get(m).unwrap().style.effect,
            Effect::Mosaic { block: 20 }
        );
        assert_eq!(
            doc.get(b).unwrap().style.effect,
            Effect::Blur { radius: 20 }
        );
        // Clamped by the ladder: a 0 asks for the smallest rung, and each kind keeps its
        // own shape while doing it.
        let cmd = intensity_command(&doc, &[m, b], 0).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(
            doc.get(m).unwrap().style.effect,
            Effect::Mosaic { block: EFFECT_MIN }
        );
        assert_eq!(
            doc.get(b).unwrap().style.effect,
            Effect::Blur { radius: EFFECT_MIN }
        );
        // A width and a font re-edit, on the same two objects, one channel each.
        let cmd = width_command(&doc, &[m, b], 2).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(m).unwrap().style.width, 2);
        assert_eq!(
            doc.get(b).unwrap().style.effect,
            Effect::Blur { radius: EFFECT_MIN },
            "the pen width does not reach into the effect slot"
        );
        let cmd = font_size_command(&doc, &[m, b], 500).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(m).unwrap().style.font_size, FONT_MAX);
    }

    #[test]
    fn resize_needs_a_target_for_every_id_and_refuses_a_locked_one() {
        let (mut doc, _) = doc_with_two();
        assert!(resize_command(&doc, &[]).is_none());
        assert!(resize_command(&doc, &[(7, rect(0, 0, 1, 1))]).is_none());
        doc.get_mut(2).unwrap().locked = true;
        let cmd = resize_to_rect(&doc, 2, PhysRect::new(0, 0, 5, 5));
        assert!(cmd.is_none());
        let cmd = resize_to_rect(&doc, 1, PhysRect::new(11, 11, 18, 18)).unwrap();
        cmd.apply(&mut doc);
        assert_eq!(doc.get(1).unwrap().geom, rect(11, 11, 18, 18));
        cmd.invert(&mut doc);
        assert_eq!(doc.get(1).unwrap().geom, rect(10, 10, 20, 20));
    }

    #[test]
    fn a_corner_edit_is_one_undoable_step_and_a_locked_object_refuses_it() {
        // §5.7.3 step 4 的两只手 —— 面板那一格与图上那一枚把手 —— 交回的是同一位作者建的
        // 一条 [`Command::Style`]，所以一次 Ctrl+Z 退回来的是"那一次改半径"，不是它的两半。
        let (mut doc, ids) = doc_with_two();
        let before = doc.clone();
        let cmd = corner_command(&doc, &ids, CORNER_MAX + 40).unwrap();
        cmd.apply(&mut doc);
        for id in &ids {
            assert_eq!(
                doc.get(*id).unwrap().style.corner_radius,
                CORNER_MAX,
                "阶梯的那一端就是把手拖到尽头的那一端"
            );
        }
        cmd.invert(&mut doc);
        assert_eq!(doc, before, "一次撤销要回到半径还没动的那一刻");
        // 已经站在这一档上 ⇒ 不建条：没改动任何东西的一次按下不进撤销栈（§5.7.19）。
        assert!(
            corner_command(&doc, &ids, Style::default().corner_radius).is_none(),
            "把半径改成它已经是的那个数，栈上不该多一条"
        );
        doc.get_mut(2).unwrap().locked = true;
        match corner_command(&doc, &ids, 20).unwrap() {
            Command::Style(pairs) => assert_eq!(pairs.len(), 1, "锁着的那一枚不该被半径带走"),
            other => panic!("{other:?}"),
        }
    }

    /// §5.7.16 步 3 的「文字内容」自此有读者：`Command::Text` 记下改前改后两串，一次
    /// `invert` 回到原字；没换字、锁着的、已经不在了的那一枚，以及「屏幕上看不见的字」
    /// （编号）都不建条。
    #[test]
    fn retyping_an_object_is_one_undoable_step_and_a_locked_one_refuses_it() {
        let mut doc = Document::new(100, 100);
        let a = doc.add(Kind::Text, rect(10, 10, 40, 12), Style::default());
        doc.get_mut(a.id).unwrap().text = "先写的那一句".into();
        let before = doc.clone();

        let cmd = text_command(&doc, a.id, "改过的那一句").unwrap();
        let dirty = cmd.apply(&mut doc);
        assert_eq!(doc.get(a.id).unwrap().text, "改过的那一句");
        assert_eq!(dirty.rects.len(), 1, "改字只动那一枚对象，重绘不该铺满整张图");
        cmd.invert(&mut doc);
        assert_eq!(doc, before, "一次撤销要回到字还没改的那一刻");

        // 交回同一串 ⇒ 栈上不多一条：没改动任何东西的一次按下不进撤销栈（§5.7.19）。
        assert!(text_command(&doc, a.id, "先写的那一句").is_none());
        // 锁着 ⇒ 选中不等于许可，这一判与其余五臂同一位作者（`Document::can_edit`）。
        doc.get_mut(a.id).unwrap().locked = true;
        assert!(text_command(&doc, a.id, "第三种").is_none());
        doc.get_mut(a.id).unwrap().locked = false;
        // 已经不在了的那一枚：整条不建，与其余 builder 对陌生 id 的答案一致。
        assert!(text_command(&doc, a.id + 9, "改给不存在的人").is_none());
        // 编号也带一串字，但画上去的是号（`raster` 那一处 match 先读 `number`），就地改
        // 它等于改一行屏幕上看不见的字 —— [`Kind::takes_text`] 是这一判的唯一作者。
        let n = doc.add(Kind::Number, rect(60, 60, 8, 8), Style::default());
        assert!(text_command(&doc, n.id, "改成字").is_none());
    }

    /// 字变长了，盒子要跟着变：那两半拼成一条 [`Command::Compound`]，所以一次 Ctrl+Z 退
    /// 回的是**那一次改字**，不是它的两半（与 [`delete_and_renumber_command`] 把删除与
    /// 重排合成一条同一口径）。
    #[test]
    fn a_retype_with_a_bigger_box_undoes_both_halves_at_once() {
        let mut doc = Document::new(100, 100);
        let a = doc.add(Kind::Text, rect(10, 10, 40, 12), Style::default());
        doc.get_mut(a.id).unwrap().text = "短".into();
        let before = doc.clone();

        let cmd = Command::Compound(vec![
            text_command(&doc, a.id, "长到要换个盒子的那一句").unwrap(),
            resize_to_rect(&doc, a.id, PhysRect::new(10, 10, 90, 12)).unwrap(),
        ]);
        cmd.apply(&mut doc);
        assert_eq!(doc.get(a.id).unwrap().text, "长到要换个盒子的那一句");
        assert_eq!(doc.get(a.id).unwrap().geom, rect(10, 10, 90, 12));
        cmd.invert(&mut doc);
        assert_eq!(doc, before, "一次撤销要同时退掉字与盒子");

        let wide = Command::Compound(vec![
            text_command(&doc, a.id, "再来一次").unwrap(),
            resize_to_rect(&doc, a.id, PhysRect::new(10, 10, 90, 12)).unwrap(),
        ]);
        let b = wide.bounds(&doc).expect("复合命令报不出可重绘的区域");
        assert!(b.w >= 90 && b.h >= 12, "两半的区域没合到一次刷里：{b:?}");
    }
}
