//! The undo/redo stack over [`Command`]s.
//!
//! 计划 §6.4 fixes both numbers: 200 steps, and a style edit landing within 400
//! ms of the previous step merges into it. §5.7.19 supplies the rule behind the
//! first number — when the stack is full the *oldest* step goes, so the work the
//! user is in the middle of is never the part that is lost.
//!
//! The stack never applies anything. By the time [`UndoStack::record`] runs the
//! document is already written: a live stroke goes through `Document::add` as it
//! is drawn, and a change whose "before" state has to be read first goes through
//! the builders in [`super::command`] followed by `Command::apply`. Recording is
//! only filing the command away.

use super::command::{merge_styles, Command, Dirty};
use super::model::Document;

/// One step. The dirty region is not stored: it is recomputed from the command
/// at the moment the step is reversed, which is the only time it means something
/// — a merged step has to dirty what all of its pieces touched, and both ends of
/// every change are already inside the command.
#[derive(Clone, Debug)]
struct Step {
    cmd: Command,
    at_ms: u64,
}

/// Undo history for one annotation session.
#[derive(Clone, Debug)]
pub struct UndoStack {
    done: Vec<Step>,
    redo: Vec<Step>,
    cap: usize,
    merge_ms: u64,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoStack {
    /// 计划 §6.4: 200 steps.
    pub const CAP: usize = 200;
    /// 计划 §6.4: 400 ms of style merging.
    pub const MERGE_MS: u64 = 400;

    pub fn new() -> Self {
        Self::with_limits(Self::CAP, Self::MERGE_MS)
    }

    /// The same machine with different numbers, for the cap and window tests.
    pub fn with_limits(cap: usize, merge_ms: u64) -> Self {
        UndoStack {
            done: Vec::new(),
            redo: Vec::new(),
            cap: cap.max(1),
            merge_ms,
        }
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn merge_window(&self) -> u64 {
        self.merge_ms
    }

    /// File a command that has already been written to the document.
    ///
    /// Two style commands over the same objects inside the window become one
    /// step, keeping the oldest "from" so a single undo lands where the user
    /// started. The window slides: a wheel held down over the same selection
    /// stays one step, which is what §5.7.20 (scroll to fine-tune width) and
    /// §5.7.21 (scroll to adjust opacity) ask for — nobody wants forty undos to
    /// get one width back.
    ///
    /// `false` means the command merged into the step above, so the undo depth
    /// did not grow; the view uses that for the label on the Undo button.
    pub fn record(&mut self, cmd: Command, now_ms: u64) -> bool {
        self.redo.clear();
        if cmd.is_style() {
            if let Some(last) = self.done.last_mut() {
                if now_ms.saturating_sub(last.at_ms) <= self.merge_ms {
                    if let Some(merged) = merge_styles(&last.cmd, &cmd) {
                        last.cmd = merged;
                        last.at_ms = now_ms;
                        return false;
                    }
                }
            }
        }
        self.done.push(Step { cmd, at_ms: now_ms });
        if self.done.len() > self.cap {
            self.done.remove(0);
        }
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// How far back the user can still get. Past [`UndoStack::cap`] this
    /// saturates, which is the honest answer: that is all the undo there is.
    pub fn depth(&self) -> usize {
        self.done.len()
    }

    /// §5.7.19 step 2: reverse the newest step and report what to repaint.
    /// `None` means there was nothing to undo and the document is untouched.
    pub fn undo(&mut self, doc: &mut Document) -> Option<Dirty> {
        let step = self.done.pop()?;
        let dirty = step.cmd.invert(doc);
        self.redo.push(step);
        Some(dirty)
    }

    /// §5.7.19 step 3: replay the newest undone step. A redo can never overflow
    /// `done`, because every step on `redo` came off it.
    pub fn redo(&mut self, doc: &mut Document) -> Option<Dirty> {
        let step = self.redo.pop()?;
        let dirty = step.cmd.apply(doc);
        self.done.push(step);
        Some(dirty)
    }

    /// A fresh capture starts a fresh session; the old history is unreachable.
    pub fn clear(&mut self) {
        self.done.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::command::{clone_command, crop_command, move_command, style_command};
    use crate::annotation::model::{Brush, Element, Geom, Kind, Style, KINDS};
    use crate::geometry::{PhysPoint, PhysRect};
    use crate::imageops::Shape;
    use proptest::prelude::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Geom {
        Geom::Rect(PhysRect::new(x, y, w, h))
    }

    fn path(points: &[(i32, i32)]) -> Geom {
        Geom::Path(points.iter().map(|(x, y)| PhysPoint::new(*x, *y)).collect())
    }

    fn pen(width: u32) -> Style {
        Style {
            width,
            ..Style::default()
        }
    }

    /// The interactive shape of a new stroke: written first, then filed.
    fn stroke(doc: &mut Document, stack: &mut UndoStack, t: u64) -> u64 {
        let e = doc.add(Kind::Rect, rect(10, 10, 20, 20), Style::default());
        stack.record(Command::Add(vec![e.clone()]), t);
        e.id
    }

    #[test]
    fn undo_then_redo_returns_to_the_same_document() {
        let mut doc = Document::new(100, 100);
        let mut stack = UndoStack::new();
        stroke(&mut doc, &mut stack, 1_000);
        let after = doc.clone();

        let dirty = stack.undo(&mut doc).unwrap();
        assert_eq!(doc.len(), 0);
        // The box an undo has to repaint is the element plus its pen.
        assert_eq!(dirty.merged(), Some(PhysRect::new(8, 8, 24, 24)));
        assert!(stack.can_redo());
        stack.redo(&mut doc).unwrap();
        assert_eq!(doc, after);
        assert!(!stack.can_redo());
    }

    #[test]
    fn undo_past_the_start_and_redo_past_the_end_leave_the_document_alone() {
        let mut doc = Document::new(100, 100);
        let mut stack = UndoStack::new();
        assert!(stack.undo(&mut doc).is_none());
        assert!(stack.redo(&mut doc).is_none());
        stroke(&mut doc, &mut stack, 1_000);
        assert!(stack.undo(&mut doc).is_some());
        assert!(stack.undo(&mut doc).is_none());
        assert_eq!(doc.len(), 0);
    }

    #[test]
    fn a_new_command_forgets_the_redo_branch() {
        let mut doc = Document::new(100, 100);
        let mut stack = UndoStack::new();
        stroke(&mut doc, &mut stack, 1_000);
        stack.undo(&mut doc);
        assert!(stack.can_redo());
        stroke(&mut doc, &mut stack, 2_000);
        assert!(!stack.can_redo());
        // One step left: the first stroke was undone out of existence, the
        // second is the only thing still reachable backwards.
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn style_edits_inside_the_window_are_one_step_back_to_where_you_started() {
        let mut doc = Document::new(100, 100);
        let mut stack = UndoStack::new();
        let id = stroke(&mut doc, &mut stack, 1_000);

        for (t, w) in [(1_100u64, 4u32), (1_200, 8), (1_300, 16)] {
            let cmd = style_command(&doc, &[id], &pen(w)).unwrap();
            cmd.apply(&mut doc);
            stack.record(cmd, t);
        }
        // Three scrolls, one undo — and it goes back to the width the stroke was
        // drawn with, not to the first intermediate value.
        assert_eq!(stack.depth(), 2);
        stack.undo(&mut doc);
        assert_eq!(doc.get(id).unwrap().style.width, Style::default().width);
        assert_eq!(stack.depth(), 1);
        stack.redo(&mut doc);
        assert_eq!(doc.get(id).unwrap().style.width, 16);
    }

    #[test]
    fn style_edits_outside_the_window_stay_separate_steps() {
        let mut doc = Document::new(100, 100);
        let mut stack = UndoStack::new();
        let id = stroke(&mut doc, &mut stack, 1_000);
        for t in [1_500u64, 2_000] {
            let cmd = style_command(&doc, &[id], &pen(5)).unwrap();
            cmd.apply(&mut doc);
            stack.record(cmd, t);
        }
        assert_eq!(stack.depth(), 3);
    }

    #[test]
    fn only_style_merges_and_only_over_the_same_objects() {
        let mut doc = Document::new(100, 100);
        let mut stack = UndoStack::new();
        let a = stroke(&mut doc, &mut stack, 1_000);
        let b = doc.add(Kind::Ellipse, rect(50, 50, 20, 20), Style::default());
        stack.record(Command::Add(vec![b.clone()]), 1_050);
        assert_eq!(stack.depth(), 2);

        // Same window, but the step below is an Add: two steps stay two steps.
        let cmd = style_command(&doc, &[a], &pen(9)).unwrap();
        cmd.apply(&mut doc);
        stack.record(cmd, 1_100);
        assert_eq!(stack.depth(), 3);

        // And a style over a different object never folds into the one below.
        let cmd = style_command(&doc, &[b.id], &pen(3)).unwrap();
        cmd.apply(&mut doc);
        stack.record(cmd, 1_150);
        let cmd = style_command(&doc, &[a], &pen(4)).unwrap();
        cmd.apply(&mut doc);
        stack.record(cmd, 1_200);
        assert_eq!(stack.depth(), 5);
    }

    #[test]
    fn a_full_stack_drops_the_oldest_and_keeps_the_newest() {
        // §5.7.19 规则: 撤销栈达到上限时，应优先保留最近操作.
        let mut doc = Document::new(400, 400);
        let mut stack = UndoStack::with_limits(3, 400);
        let mut ids = Vec::new();
        for (i, t) in (0..5u64).enumerate() {
            let e = doc.add(Kind::Rect, rect(i as i32 * 10, 0, 4, 4), Style::default());
            ids.push(e.id);
            stack.record(Command::Add(vec![e]), t * 1_000);
        }
        assert_eq!(stack.depth(), 3);
        for _ in 0..4 {
            stack.undo(&mut doc);
        }
        // The two steps that fell off the cap cannot be undone, so the first two
        // elements are the ones still on the canvas — losing the oldest work is
        // what "优先保留最近操作" means, not losing the newest.
        assert_eq!(
            doc.elements.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![ids[0], ids[1]]
        );
        assert_eq!(stack.depth(), 0);
        assert!(stack.undo(&mut doc).is_none());
    }

    #[test]
    fn a_crop_is_undone_whole_and_its_dirty_rects_stay_in_the_picture() {
        let mut doc = Document::new(100, 100);
        let mut stack = UndoStack::new();
        let id = stroke(&mut doc, &mut stack, 1_000);

        let cmd = move_command(&doc, &[id], 30, 0).unwrap();
        let d = cmd.apply(&mut doc);
        assert!(d.merged().unwrap().contains(PhysPoint::new(40, 20)));
        stack.record(cmd, 2_000);

        let cmd = crop_command(&doc, &PhysRect::new(0, 0, 30, 30)).unwrap();
        let d = cmd.apply(&mut doc);
        for r in &d.rects {
            assert_eq!(*r, r.intersection(&doc.base).unwrap());
        }
        stack.record(cmd, 3_000);
        stack.undo(&mut doc);
        assert_eq!(doc.base_size(), (100, 100));
        assert_eq!(doc.get(id).unwrap().geom, rect(40, 10, 20, 20));
    }

    #[test]
    fn a_wider_brush_widens_the_region_both_directions_report() {
        let mut doc = Document::new(100, 100);
        let mut stack = UndoStack::new();
        let e = doc.add(Kind::Pencil, path(&[(10, 10), (30, 30)]), Style::default());
        let id = e.id;
        stack.record(Command::Add(vec![e]), 1_000);

        let to = Style {
            brush: Brush {
                size: 40,
                shape: Shape::Circle,
                feather: 0,
            },
            ..Style::default()
        };
        let cmd = style_command(&doc, &[id], &to).unwrap();
        let r = cmd.apply(&mut doc).merged().unwrap();
        // The stroke spans (10,10)..(30,30) and a 40px brush reaches 20 past it,
        // so the repaint box runs from the picture corner to (50,50). The 36x36
        // box of the old 16px brush would have left the new edge unpainted.
        assert_eq!(r, PhysRect::new(0, 0, 50, 50));
        stack.record(cmd, 1_100);
        assert_eq!(stack.undo(&mut doc).unwrap().merged(), Some(r));
    }

    #[test]
    fn clear_forgets_everything_in_both_directions() {
        let mut doc = Document::new(100, 100);
        let mut stack = UndoStack::new();
        stroke(&mut doc, &mut stack, 1_000);
        stack.undo(&mut doc);
        stack.clear();
        assert!(!stack.can_undo() && !stack.can_redo());
        assert_eq!(doc.len(), 0);
    }

    #[derive(Clone, Debug)]
    enum Op {
        Add(u8, i32, i32, u32, u32),
        Move(u8, i32, i32),
        Style(u8, u32),
        Clone(u8, i32, i32),
        Renumber(u32),
        Crop(u32, u32),
        Undo,
        Redo,
    }

    impl Op {
        /// An id to act on, taken from what is actually in the document.
        fn pick(doc: &Document, i: u8) -> Option<u64> {
            if doc.elements.is_empty() {
                return None;
            }
            Some(doc.elements[i as usize % doc.elements.len()].id)
        }
    }

    fn arb_op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u8..16, 0..400i32, 0..400i32, 1u32..50, 1u32..50)
                .prop_map(|(k, x, y, w, h)| Op::Add(k, x, y, w, h)),
            (0u8..16, -40..40i32, -40..40i32).prop_map(|(i, dx, dy)| Op::Move(i, dx, dy)),
            (0u8..16, 1u32..24).prop_map(|(i, w)| Op::Style(i, w)),
            (0u8..16, -30..30i32, -30..30i32).prop_map(|(i, dx, dy)| Op::Clone(i, dx, dy)),
            (1u32..30).prop_map(Op::Renumber),
            (1u32..200, 1u32..200).prop_map(|(w, h)| Op::Crop(w, h)),
            Just(Op::Undo),
            Just(Op::Redo),
        ]
    }

    /// What a step ends on: the picture and its objects.
    type State = (PhysRect, Vec<Element>);

    /// The document plus one snapshot per reachable step. The two counters are
    /// left out of the snapshot on purpose: ids and numbers are never reused, so
    /// undoing an action must not rewind them, and they are checked separately.
    struct Model {
        doc: Document,
        snaps: Vec<State>,
        cursor: usize,
        max_counter: u32,
    }

    impl Model {
        fn new(size: u32) -> Self {
            let doc = Document::new(size, size);
            let max_counter = doc.next_counter;
            let snaps = vec![Model::state(&doc)];
            Model {
                doc,
                snaps,
                cursor: 0,
                max_counter,
            }
        }

        fn state(doc: &Document) -> State {
            (doc.base, doc.elements.clone())
        }

        /// File the state after an action. A merged step does not move the
        /// cursor: it rewrites the picture that its own step ends on.
        fn commit(&mut self, grew: bool) {
            let now = Model::state(&self.doc);
            self.snaps.truncate(self.cursor + 1);
            if grew {
                self.snaps.push(now);
                self.cursor += 1;
            } else {
                self.snaps[self.cursor] = now;
            }
        }
    }

    // The oracle is a snapshot of the whole object list per step, so exact
    // inversion, id reuse, painting order and the number counter all have to
    // agree with "the document holds what it held when this step was current".
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]
        #[test]
        fn replay_matches_snapshots_at_every_depth(ops in prop::collection::vec(arb_op(), 1..40)) {
            let mut m = Model::new(200);
            let mut stack = UndoStack::with_limits(6, 400);
            let mut t = 0u64;

            for op in ops {
                t += 150;
                let mut recorded = false;
                let mut grew = true;
                match op {
                    Op::Add(k, x, y, w, h) => {
                        let kind = KINDS[k as usize % KINDS.len()];
                        let x = x.rem_euclid(m.doc.base.w.max(1) as i32);
                        let y = y.rem_euclid(m.doc.base.h.max(1) as i32);
                        let e = m.doc.add(kind, rect(x, y, w, h), Style::default());
                        grew = stack.record(Command::Add(vec![e]), t);
                        recorded = true;
                    }
                    Op::Move(i, dx, dy) => {
                        if let Some(id) = Op::pick(&m.doc, i) {
                            if let Some(cmd) = move_command(&m.doc, &[id], dx, dy) {
                                cmd.apply(&mut m.doc);
                                grew = stack.record(cmd, t);
                                recorded = true;
                            }
                        }
                    }
                    Op::Style(i, w) => {
                        if let Some(id) = Op::pick(&m.doc, i) {
                            if let Some(cmd) = style_command(&m.doc, &[id], &pen(w)) {
                                cmd.apply(&mut m.doc);
                                grew = stack.record(cmd, t);
                                recorded = true;
                            }
                        }
                    }
                    Op::Clone(i, dx, dy) => {
                        if let Some(id) = Op::pick(&m.doc, i) {
                            // A copy mutates the document as it is made, so the
                            // builder takes the document by value like the live
                            // Ctrl-drag gesture does.
                            if let Some(cmd) = clone_command(&mut m.doc, &[id], dx, dy) {
                                grew = stack.record(cmd, t);
                                recorded = true;
                            }
                        }
                    }
                    Op::Renumber(start) => {
                        // `renumber` writes and reports in one call; the pairs it
                        // leaves out are the elements already holding their value.
                        let pairs = m.doc.renumber(start);
                        if !pairs.is_empty() {
                            grew = stack.record(Command::Renumber(pairs), t);
                            recorded = true;
                        }
                    }
                    Op::Crop(w, h) => {
                        let from = m.doc.base;
                        let to = PhysRect::new(0, 0, w.min(from.w), h.min(from.h));
                        if let Some(cmd) = crop_command(&m.doc, &to) {
                            cmd.apply(&mut m.doc);
                            grew = stack.record(cmd, t);
                            recorded = true;
                        }
                    }
                    Op::Undo => {
                        let had = stack.can_undo();
                        let got = stack.undo(&mut m.doc).is_some();
                        prop_assert_eq!(had, got);
                        if got {
                            m.cursor -= 1;
                        }
                    }
                    Op::Redo => {
                        let had = stack.can_redo();
                        let got = stack.redo(&mut m.doc).is_some();
                        prop_assert_eq!(had, got);
                        if got {
                            m.cursor += 1;
                        }
                    }
                }
                if recorded {
                    m.commit(grew);
                }
                prop_assert!(m.cursor < m.snaps.len());
                prop_assert!(
                    m.doc.next_counter >= m.max_counter,
                    "the 起始编号 counter rewound from {} to {}",
                    m.max_counter,
                    m.doc.next_counter
                );
                m.max_counter = m.doc.next_counter;
                let now = Model::state(&m.doc);
                prop_assert!(
                    now == m.snaps[m.cursor],
                    "at depth {} of {} steps the objects left their snapshot",
                    m.cursor,
                    stack.depth()
                );
            }
        }
    }
}
