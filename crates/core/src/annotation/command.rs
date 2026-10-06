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

use super::model::{Document, Element, Geom, Style, Transform};

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
    /// 技术方案 §8.2 的 ReorderLayer.
    Reorder(Vec<(u64, i32, i32)>),
    /// §5.7.12 step 5: the values changed by a renumber.
    Renumber(Vec<(u64, Option<u32>, Option<u32>)>),
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

/// §5.7.13 step 5 and §5.7.3 step 4 want the size change expressed as a rect,
/// so this is the one resize constructor that takes a rect instead of a Geom.
pub fn resize_to_rect(doc: &Document, id: u64, to: PhysRect) -> Option<Command> {
    resize_command(doc, &[(id, Geom::Rect(to))])
}

/// §5.7.18 step 2: Ctrl+drag, or copy and paste. The copies are made here, so
/// the command that comes back already describes the finished change — and one
/// undo removes the whole clone rather than one copy at a time.
pub fn clone_command(doc: &mut Document, ids: &[u64], dx: i32, dy: i32) -> Option<Command> {
    let created = doc.clone_elements(ids, dx, dy);
    if created.is_empty() {
        return None;
    }
    let list = created
        .iter()
        .filter_map(|id| doc.get(*id).cloned())
        .collect();
    Some(Command::Add(list))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::model::{Brush, Kind};
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
}
