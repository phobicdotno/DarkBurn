//! The editing session: the document, what is selected, undo and redo, the current layer and
//! whether anything changed since the last save.
//!
//! Undo keeps whole snapshots of the document. Shapes are shared between snapshots, so a step
//! costs one pointer per shape plus the layer table, and history is unlimited.

use crate::clipboard::Clipboard;
use crate::document::Document;
use crate::shape::{Shape, ShapeId};

/// The selected shapes, in the order they were selected (the last is the reference for align).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    ids: Vec<ShapeId>,
}

impl Selection {
    pub fn new() -> Selection {
        Selection::default()
    }

    pub fn ids(&self) -> &[ShapeId] {
        &self.ids
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn contains(&self, id: ShapeId) -> bool {
        self.ids.contains(&id)
    }

    pub fn clear(&mut self) {
        self.ids.clear();
    }

    /// Select exactly `ids`.
    pub fn set(&mut self, ids: impl IntoIterator<Item = ShapeId>) {
        self.ids.clear();
        self.extend(ids);
    }

    /// Add `id` (moved to the end if already selected).
    pub fn add(&mut self, id: ShapeId) {
        self.ids.retain(|i| *i != id);
        self.ids.push(id);
    }

    pub fn extend(&mut self, ids: impl IntoIterator<Item = ShapeId>) {
        for id in ids {
            self.add(id);
        }
    }

    pub fn remove(&mut self, id: ShapeId) {
        self.ids.retain(|i| *i != id);
    }

    /// Add `id` when not selected, else remove it (Shift+click).
    pub fn toggle(&mut self, id: ShapeId) {
        if self.contains(id) { self.remove(id) } else { self.add(id) }
    }

    /// The most recently selected shape.
    pub fn last(&self) -> Option<ShapeId> {
        self.ids.last().copied()
    }

    /// Drop shapes that can no longer be clicked: locked, or with every layer hidden. Without
    /// this a shape on a layer that was just hidden stays selected, unseen, and the next Delete
    /// or arrow key acts on it.
    pub fn retain_selectable(&mut self, doc: &Document) {
        self.ids.retain(|id| doc.get(*id).is_some_and(|s| doc.is_selectable(s)));
    }

    /// Drop ids that are no longer in `doc`.
    pub fn retain_existing(&mut self, doc: &Document) {
        self.ids.retain(|id| doc.get(*id).is_some());
    }
}

/// One undo step: the state before a change.
#[derive(Clone, Debug)]
struct Snapshot {
    doc: Document,
    selection: Selection,
    label: String,
    /// The editor's revision when this state was current.
    revision: u64,
    /// A number the app gave the step (0: none), to find what it keeps outside the document
    /// for it (see [`Editor::checkpoint_tagged`]).
    tag: u64,
}

/// Redo steps taken out of the history with [`History::take_redo`], to be put back with
/// [`History::put_back_redo`] when the change that would have cleared them is taken back.
#[derive(Clone, Debug, Default)]
pub struct RedoSteps(Vec<Snapshot>);

impl RedoSteps {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Unlimited undo and redo.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What Undo would undo.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    /// The tag of the step Undo would undo (0: none).
    pub fn undo_tag(&self) -> u64 {
        self.undo.last().map_or(0, |s| s.tag)
    }

    /// The tag of the step Redo would redo (0: none).
    pub fn redo_tag(&self) -> u64 {
        self.redo.last().map_or(0, |s| s.tag)
    }

    pub fn len(&self) -> usize {
        self.undo.len()
    }

    /// Labels of the undo steps, the next one to undo first.
    pub fn undo_labels(&self) -> Vec<&str> {
        self.undo.iter().rev().map(|s| s.label.as_str()).collect()
    }

    /// Labels of the redo steps, the next one to redo first.
    pub fn redo_labels(&self) -> Vec<&str> {
        self.redo.iter().rev().map(|s| s.label.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Take the redo steps out, before a change that may be taken back as if it never came
    /// (a change clears them).
    pub fn take_redo(&mut self) -> RedoSteps {
        RedoSteps(std::mem::take(&mut self.redo))
    }

    /// Put redo steps from [`History::take_redo`] back, in place of any there are now.
    pub fn put_back_redo(&mut self, steps: RedoSteps) {
        self.redo = steps.0;
    }
}

/// How [`Editor::end_checkpoint`] settled a gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureEnd {
    /// The change stays, as one undo step.
    Kept,
    /// The document is as it was: no undo step.
    Unchanged,
    /// A shape went out of reach: the document is back as it was before the gesture.
    Refused,
}

/// The gesture opened by the last [`Editor::checkpoint`], kept until it ends.
#[derive(Clone, Debug)]
struct Gesture {
    /// The state when it began (the checkpoint's own copy may be followed by other steps).
    start: Snapshot,
    /// How many undo steps there were before its checkpoint.
    depth: usize,
    /// The document as saved during the gesture, if it was.
    saved_doc: Option<Document>,
}

/// The editing session for one open project.
#[derive(Clone, Debug)]
pub struct Editor {
    pub doc: Document,
    pub selection: Selection,
    pub history: History,
    pub clipboard: Clipboard,
    /// The layer new shapes go on (the cut palette's choice).
    pub current_layer: usize,
    /// Bumped on every change; renderers and auto-save compare it.
    revision: u64,
    saved_revision: u64,
    /// Which project this is: see [`Editor::project`].
    project: u64,
    /// The gesture that is open, see [`Editor::end_checkpoint`].
    gesture: Option<Gesture>,
}

impl Default for Editor {
    fn default() -> Editor {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Editor {
            doc: Document::default(),
            selection: Selection::default(),
            history: History::default(),
            clipboard: Clipboard::default(),
            current_layer: 0,
            revision: 0,
            saved_revision: 0,
            project: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            gesture: None,
        }
    }
}

impl Editor {
    pub fn new() -> Editor {
        Editor::default()
    }

    /// A number of its own for every editor made (a new or an opened project); a copy keeps it.
    /// Something that holds on to the project between frames (a window's draft) compares it to
    /// learn that another project took this one's place.
    pub fn project(&self) -> u64 {
        self.project
    }

    /// An editor on an opened document, unmodified.
    pub fn with_document(doc: Document) -> Editor {
        Editor { doc, ..Editor::default() }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Changed since it was last saved or opened.
    pub fn is_modified(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
        if let Some(g) = &mut self.gesture {
            g.saved_doc = Some(self.doc.clone());
        }
    }

    /// Note a change that is not an undo step (selection, view).
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    /// Record the state before a change called `label`. Call it, then change `doc`.
    pub fn checkpoint(&mut self, label: &str) {
        self.checkpoint_tagged(label, 0);
    }

    /// [`Editor::checkpoint`] with a `tag` the app can read back with [`History::undo_tag`] and
    /// [`History::redo_tag`] before it undoes or redoes the step, for a change it keeps outside
    /// the document (a cut library entry). The step is recorded even if the document stays the same.
    pub fn checkpoint_tagged(&mut self, label: &str, tag: u64) {
        let start = Snapshot { doc: self.doc.clone(), selection: self.selection.clone(), label: label.to_string(), revision: self.revision, tag };
        self.gesture = Some(Gesture { start: start.clone(), depth: self.history.undo.len(), saved_doc: None });
        self.history.undo.push(start);
        self.history.redo.clear();
        self.revision += 1;
    }

    /// Make an undoable change: `f` changes the document and selection. A change that leaves a
    /// shape further out than [`MAX_COORD`], or with a position or size that is not a number,
    /// or nested deeper than [`MAX_GROUP_DEPTH`], is not made at all (false comes back, as if `f` had declined). Nothing is recorded
    /// when it returns false, nor when it says it changed something and the document is the one
    /// it was (the selection `f` made is kept then, and what could be redone still can).
    pub fn edit(&mut self, label: &str, f: impl FnOnce(&mut Document, &mut Selection) -> bool) -> bool {
        let before = Snapshot { doc: self.doc.clone(), selection: self.selection.clone(), label: label.to_string(), revision: self.revision, tag: 0 };
        let changed = f(&mut self.doc, &mut self.selection) && within_reach(&self.doc, &before.doc);
        if changed && self.doc == before.doc {
            // Nothing to undo: no step, and the document is no more "changed" than it was.
        } else if changed {
            self.history.undo.push(before);
            self.history.redo.clear();
            self.revision += 1;
        } else {
            self.doc = before.doc;
            self.selection = before.selection;
        }
        self.selection.retain_existing(&self.doc);
        changed
    }

    /// End a gesture that began with [`Editor::checkpoint`] (a drag on the canvas) with the
    /// guards of [`Editor::edit`]: a gesture that left the document as it was when it began
    /// leaves no undo step (the selection stays as it is now), and one that put a shape out of
    /// reach (see [`MAX_COORD`] and [`MAX_GROUP_DEPTH`]) is taken back whole, with every step
    /// made during it. Both are judged against the document as it was when the gesture began,
    /// so a change made by another command during the gesture (a shortcut, the API) cannot
    /// slip a shape out of reach past it. However it ends, the project counts as saved when
    /// the document is the one saved before or during the gesture.
    pub fn end_checkpoint(&mut self) -> GestureEnd {
        let Some(g) = self.gesture.take() else { return GestureEnd::Kept };
        let end = if self.doc == g.start.doc {
            // The checkpoint and any step made during the gesture undo nothing now.
            self.history.undo.truncate(g.depth);
            self.step_to(false);
            GestureEnd::Unchanged
        } else if !within_reach(&self.doc, &g.start.doc) {
            // Take back the gesture and every step made during it. Redo was empty when it
            // began, so what is there now came from an undo during it and may be out of reach.
            self.history.undo.truncate(g.depth);
            self.history.redo.clear();
            self.restore_doc(g.start.doc.clone());
            self.selection = g.start.selection.clone();
            self.selection.retain_existing(&self.doc);
            self.step_to(false);
            GestureEnd::Refused
        } else {
            GestureEnd::Kept
        };
        // An undo during the gesture put the document of that moment on the redo history,
        // unchecked: redo must not bring back what the reach guard refuses.
        if self.history.redo.iter().any(|s| !within_reach(&s.doc, &g.start.doc)) {
            self.history.redo.clear();
        }
        if self.ends_saved(&g) {
            self.saved_revision = self.revision;
        }
        end
    }

    /// Whether the open document, at the end of gesture `g`, is the one last saved: the one
    /// from when `g` began if it was saved then, or one saved during `g`.
    fn ends_saved(&self, g: &Gesture) -> bool {
        (self.doc == g.start.doc && self.is_saved_state(&g.start)) || self.saved_during(Some(g))
    }

    /// Forget the last checkpoint when the change it guarded did nothing (a click without a drag).
    pub fn drop_checkpoint(&mut self) {
        let gesture = self.gesture.take();
        if let Some(s) = self.history.undo.pop() {
            // Back at the state of the checkpoint: still saved if it was saved then.
            let was_saved = self.is_saved_state(&s);
            self.restore_doc(s.doc);
            self.selection = s.selection;
            let was_saved = was_saved || self.saved_during(gesture.as_ref());
            self.step_to(was_saved);
        }
    }

    /// Whether the open document is the one saved while `gesture` was open (a save during a
    /// drag moves the saved revision past the checkpoint's, so the revision alone cannot tell).
    fn saved_during(&self, gesture: Option<&Gesture>) -> bool {
        gesture.and_then(|g| g.saved_doc.as_ref()).is_some_and(|d| *d == self.doc)
    }

    /// Whether `s` holds the document as it was last saved or opened. The notes are not part of
    /// an undo step, so they must still be the ones that were saved.
    fn is_saved_state(&self, s: &Snapshot) -> bool {
        s.revision == self.saved_revision && s.doc.notes == self.doc.notes && s.doc.show_notes_on_load == self.doc.show_notes_on_load
    }

    /// Move the revision on after a step through the history; `saved` says the document is now
    /// the one that was saved.
    fn step_to(&mut self, saved: bool) {
        self.revision += 1;
        if saved {
            self.saved_revision = self.revision;
        }
    }

    /// Put the document of an undo step back. The notes are typed outside the undo history (they
    /// are not undo steps), so the ones in the open document stay.
    fn restore_doc(&mut self, mut doc: Document) -> Document {
        doc.notes = std::mem::take(&mut self.doc.notes);
        doc.show_notes_on_load = self.doc.show_notes_on_load;
        let mut old = std::mem::replace(&mut self.doc, doc);
        old.notes = self.doc.notes.clone();
        old
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.history.undo.pop() else { return false };
        let saved = self.is_saved_state(&s);
        let now = Snapshot {
            doc: self.restore_doc(s.doc),
            selection: std::mem::replace(&mut self.selection, s.selection),
            label: s.label,
            revision: self.revision,
            tag: s.tag,
        };
        self.history.redo.push(now);
        self.step_to(saved);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.history.redo.pop() else { return false };
        let saved = self.is_saved_state(&s);
        let now = Snapshot {
            doc: self.restore_doc(s.doc),
            selection: std::mem::replace(&mut self.selection, s.selection),
            label: s.label,
            revision: self.revision,
            tag: s.tag,
        };
        self.history.undo.push(now);
        self.step_to(saved);
        true
    }

    /// The selected ids that still exist, in selection order.
    pub fn selected(&self) -> Vec<ShapeId> {
        self.selection.ids().iter().copied().filter(|id| self.doc.get(*id).is_some()).collect()
    }
}

/// The furthest from the origin a shape may reach, mm: a thousand kilometres, far beyond any
/// machine and well inside the range where the arithmetic still holds. A typing slip or a
/// script asking for 1e300 is refused instead of putting "infinity" into the job.
pub const MAX_COORD: f64 = 1e9;

/// How many groups (or array originals) deep a shape may sit, counted from the top of the
/// document. A project file holds about 170 levels before its reader refuses it; this leaves
/// room to spare. Grouping, pasting or arraying that would go deeper is refused.
pub const MAX_GROUP_DEPTH: usize = 100;

/// Whether `ids` can be grouped without putting a shape deeper than [`MAX_GROUP_DEPTH`].
pub fn can_group(doc: &Document, ids: &[ShapeId]) -> bool {
    ids.iter().filter_map(|id| doc.get(*id)).all(|s| s.group_depth() < MAX_GROUP_DEPTH)
}

/// Whether every shape that `doc` has and `before` had not (new or changed) lies within
/// [`MAX_COORD`] and is nested no deeper than [`MAX_GROUP_DEPTH`] (or than the deepest shape
/// `before` had, so a document that came in nested deeper can still be ungrouped). Shapes that
/// were already there are left alone, so a document that came in with something far out can
/// still be worked on.
fn within_reach(doc: &Document, before: &Document) -> bool {
    doc.shapes.iter().enumerate().all(|(i, s)| {
        if before.shapes.get(i).is_some_and(|b| std::sync::Arc::ptr_eq(b, s)) {
            return true;
        }
        let b = s.bbox();
        s.transform.as_coeffs().iter().chain(&[b.x0, b.y0, b.x1, b.y1]).all(|c| c.is_finite() && c.abs() <= MAX_COORD)
            && (s.group_depth() <= MAX_GROUP_DEPTH || s.group_depth() <= before.iter().map(Shape::group_depth).max().unwrap_or(0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::Shape;
    use kurbo::Rect;

    fn square(x: f64) -> Shape {
        Shape::rect(Rect::new(x, 0.0, x + 10.0, 10.0), 0.0, 0)
    }

    #[test]
    fn every_new_or_opened_project_is_another_project() {
        let a = Editor::new();
        let b = Editor::with_document(a.doc.clone());
        let mut c = a.clone();
        c.touch();
        assert_ne!(a.project(), b.project());
        assert_ne!(a.project(), Editor::default().project());
        assert_eq!(a.project(), c.project(), "a copy, changed or not, is the same project");
    }

    #[test]
    fn a_change_that_changes_nothing_is_not_an_undo_step() {
        // Found by the monkey: commands that report success without changing anything (a 1 x 1
        // grid array, "move to the lower left" when already there) each left an undo step that
        // did nothing and marked the document as changed.
        let mut e = Editor::new();
        let id = e.doc.add(square(0.0));
        e.edit("Move", |d, _| {
            d.get_mut(id).unwrap().transform = kurbo::Affine::translate((5.0, 0.0)) * d.get(id).unwrap().transform;
            true
        });
        e.undo();
        e.mark_saved();
        let steps = e.history.len();
        let ok = e.edit("Nothing", |_, sel| {
            sel.set([id]);
            true
        });
        assert!(ok, "the caller still hears that its change went through");
        assert_eq!(e.history.len(), steps, "no undo step");
        assert!(!e.is_modified(), "the document is as it was saved");
        assert_eq!(e.selection.ids(), &[id], "the selection the change made is kept");
        assert!(e.redo(), "and what could be redone still can");
    }

    /// A change that is taken back with drop_checkpoint can leave redo as it was.
    #[test]
    fn redo_steps_can_be_kept_across_a_change_that_is_taken_back() {
        let mut e = Editor::new();
        e.edit("Add", |d, _| {
            d.add(square(0.0));
            true
        });
        e.undo();
        assert_eq!(e.history.redo_label(), Some("Add"));
        let kept = e.history.take_redo();
        assert_eq!(kept.len(), 1);
        e.edit("Picture", |d, _| {
            d.add(square(5.0));
            true
        });
        assert!(!e.history.can_redo());
        e.drop_checkpoint();
        e.history.put_back_redo(kept);
        assert_eq!(e.doc.len(), 0);
        assert!(e.redo(), "redo brings the first change back");
        assert_eq!(e.doc.len(), 1);
    }

    /// A drag (checkpoint, live changes) that ends where it started leaves no undo step and
    /// the document unmodified; before `end_checkpoint` it left a step that did nothing.
    #[test]
    fn a_gesture_that_ends_where_it_started_leaves_no_step() {
        let mut e = Editor::new();
        let id = e.doc.add(square(0.0));
        e.mark_saved();
        let t = e.doc.get(id).unwrap().transform;
        e.checkpoint("Move");
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((30.0, 0.0)) * t;
        e.touch();
        e.doc.get_mut(id).unwrap().transform = t;
        e.touch();
        assert_eq!(e.history.len(), 1, "the checkpoint alone is an undo step that does nothing");
        assert_eq!(e.end_checkpoint(), GestureEnd::Unchanged);
        assert_eq!(e.history.len(), 0);
        assert!(!e.is_modified(), "back where it was saved");
        // A real move stays.
        e.checkpoint("Move");
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((5.0, 0.0));
        assert_eq!(e.end_checkpoint(), GestureEnd::Kept);
        assert_eq!(e.history.len(), 1);
        assert!(e.is_modified());
    }

    /// A change made by another command during a drag (a shortcut, the API) is checked against
    /// the document from before the drag when the drag ends, so it cannot slip a shape out of reach.
    #[test]
    fn a_change_during_a_gesture_cannot_slip_a_shape_out_of_reach() {
        let mut e = Editor::new();
        let id = e.doc.add(square(0.0));
        e.selection.set([id]);
        e.mark_saved();
        let before = e.doc.clone();
        e.checkpoint("Move");
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((1e13, 0.0));
        e.touch();
        // A duplicate during the drag records a step that already holds the far shape.
        assert!(e.edit("Duplicate", |d, s| {
            let copy = d.add(square(10.0));
            s.set([copy]);
            true
        }));
        assert_eq!(e.end_checkpoint(), GestureEnd::Refused);
        assert_eq!(e.doc, before);
        assert_eq!(e.history.len(), 0);
        assert_eq!(e.selected(), vec![id]);
        assert!(!e.is_modified(), "back at the saved document");
    }

    /// A save during a drag that then ends where it began leaves the project saved.
    #[test]
    fn a_save_during_a_gesture_that_ends_unchanged_stays_saved() {
        let mut e = Editor::new();
        let id = e.doc.add(square(0.0));
        let t = e.doc.get(id).unwrap().transform;
        e.checkpoint("Move");
        e.mark_saved();
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((30.0, 0.0)) * t;
        e.touch();
        e.doc.get_mut(id).unwrap().transform = t;
        e.touch();
        assert_eq!(e.end_checkpoint(), GestureEnd::Unchanged);
        assert!(!e.is_modified(), "the document is the one that was saved");
        // Saved while moved away, then back: not the saved document.
        e.checkpoint("Move");
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((30.0, 0.0)) * t;
        e.touch();
        e.mark_saved();
        e.doc.get_mut(id).unwrap().transform = t;
        e.touch();
        assert_eq!(e.end_checkpoint(), GestureEnd::Unchanged);
        assert!(e.is_modified());
    }

    /// A step recorded during a drag that then ends where it began goes with the checkpoint,
    /// and the project is still the saved one.
    #[test]
    fn a_gesture_back_at_its_start_after_an_edit_leaves_no_step() {
        let mut e = Editor::new();
        let id = e.doc.add(square(0.0));
        e.mark_saved();
        let t = e.doc.get(id).unwrap().transform;
        e.checkpoint("Move");
        assert!(e.edit("Nudge", |d, _| {
            d.get_mut(id).unwrap().transform = kurbo::Affine::translate((30.0, 0.0)) * t;
            true
        }));
        assert_eq!(e.history.len(), 2);
        e.doc.get_mut(id).unwrap().transform = t;
        e.touch();
        assert_eq!(e.end_checkpoint(), GestureEnd::Unchanged);
        assert_eq!(e.history.len(), 0, "no step that does nothing");
        assert!(!e.is_modified(), "back at the saved document");
    }

    /// An undo during a drag puts the dragged document on the redo history; when the drag is
    /// refused (or ends where it began), redo cannot bring a shape out of reach back.
    #[test]
    fn redo_cannot_bring_back_a_refused_gesture() {
        let mut e = Editor::new();
        let id = e.doc.add(square(0.0));
        let before = e.doc.clone();
        e.checkpoint("Move");
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((1e13, 0.0));
        assert!(e.undo());
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((2e13, 0.0));
        assert_eq!(e.end_checkpoint(), GestureEnd::Refused);
        assert_eq!(e.doc, before);
        assert!(!e.redo(), "nothing out of reach to redo");
        assert_eq!(e.doc, before);
        // Undone mid-drag and left there: the same.
        e.checkpoint("Move");
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((1e13, 0.0));
        assert!(e.undo());
        assert_eq!(e.end_checkpoint(), GestureEnd::Unchanged);
        assert!(!e.redo());
        assert_eq!(e.doc, before);
    }

    /// A save during a drag that ends on the saved document leaves the project saved.
    #[test]
    fn a_kept_gesture_on_the_document_saved_during_it_is_saved() {
        let mut e = Editor::new();
        let id = e.doc.add(square(0.0));
        let t = e.doc.get(id).unwrap().transform;
        e.checkpoint("Move");
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((30.0, 0.0)) * t;
        e.touch();
        e.mark_saved();
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((40.0, 0.0)) * t;
        e.touch();
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((30.0, 0.0)) * t;
        e.touch();
        assert_eq!(e.end_checkpoint(), GestureEnd::Kept);
        assert_eq!(e.history.len(), 1);
        assert!(!e.is_modified(), "the document is the one saved during the drag");
    }

    /// A drag that puts a shape out of reach is taken back, like the same change through `edit`.
    #[test]
    fn a_gesture_that_puts_a_shape_out_of_reach_is_taken_back() {
        let mut e = Editor::new();
        let id = e.doc.add(square(0.0));
        e.selection.set([id]);
        let before = e.doc.clone();
        for far in [1e13, f64::NAN, f64::INFINITY] {
            e.checkpoint("Resize");
            e.doc.get_mut(id).unwrap().transform = kurbo::Affine::scale(far);
            assert_eq!(e.end_checkpoint(), GestureEnd::Refused, "{far} was let through");
            assert_eq!(e.doc, before);
            assert_eq!(e.history.len(), 0);
            assert_eq!(e.selected(), vec![id]);
        }
    }

    #[test]
    fn a_change_that_puts_a_shape_out_of_this_world_is_refused() {
        // Found by the monkey (seeds 3004, 3007, 3010): a shape placed at -1.7e308 mm had a
        // bounding box reaching to infinity, and the job got "G0 X-inf".
        let mut e = Editor::new();
        let id = e.doc.add(square(0.0));
        let before = e.doc.clone();
        for far in [1.7e308, -1e300, 1e13, f64::NAN, f64::INFINITY] {
            let ok = e.edit("Move", |d, _| {
                d.get_mut(id).unwrap().transform = kurbo::Affine::translate((far, 0.0));
                true
            });
            assert!(!ok, "{far} was let through");
            assert_eq!(e.doc, before);
            assert_eq!(e.history.len(), 0);
        }
        let ok = e.edit("Size", |d, _| {
            d.get_mut(id).unwrap().kind = crate::shape::ShapeKind::Rect { width: 1e300, height: 5.0, radius: 0.0 };
            true
        });
        assert!(!ok, "a size of 1e300 was let through");
        assert_eq!(e.doc, before);
        // Anything a machine could ever reach is fine, and so is a shape that was already far
        // out when the document was opened: other changes still go through.
        assert!(e.edit("Move", |d, _| {
            d.get_mut(id).unwrap().transform = kurbo::Affine::translate((5e6, -5e6));
            true
        }));
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((1e300, 0.0));
        let other = e.doc.add(square(20.0));
        assert!(e.edit("Move", |d, _| {
            d.get_mut(other).unwrap().transform = kurbo::Affine::translate((1.0, 0.0));
            true
        }));
    }

    #[test]
    fn selection_keeps_order_and_toggles() {
        let mut s = Selection::new();
        s.set([ShapeId(1), ShapeId(2)]);
        s.add(ShapeId(1));
        assert_eq!(s.ids(), &[ShapeId(2), ShapeId(1)]);
        assert_eq!(s.last(), Some(ShapeId(1)));
        s.toggle(ShapeId(2));
        assert_eq!(s.ids(), &[ShapeId(1)]);
        s.toggle(ShapeId(3));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn undo_and_redo_restore_document_and_selection() {
        let mut e = Editor::new();
        assert!(!e.is_modified());
        let mut first = ShapeId(0);
        e.edit("Add", |d, sel| {
            first = d.add(square(0.0));
            sel.set([first]);
            true
        });
        e.edit("Add", |d, sel| {
            let id = d.add(square(20.0));
            sel.set([id]);
            true
        });
        assert!(e.is_modified());
        assert_eq!(e.doc.len(), 2);
        assert_eq!(e.history.undo_label(), Some("Add"));
        assert!(e.undo());
        assert_eq!(e.doc.len(), 1);
        assert_eq!(e.selected(), vec![first]);
        assert!(e.undo());
        assert!(e.doc.is_empty());
        assert!(!e.undo());
        assert!(e.redo());
        assert!(e.redo());
        assert_eq!(e.doc.len(), 2);
        assert!(!e.redo());
        // A new change after undo drops the redo steps.
        e.undo();
        e.edit("Move", |d, _| {
            d.get_mut(first).unwrap().transform = kurbo::Affine::translate((1.0, 0.0));
            true
        });
        assert!(!e.history.can_redo());
    }

    #[test]
    fn unchanged_edits_leave_no_step_and_saving_clears_modified() {
        let mut e = Editor::new();
        assert!(!e.edit("Nothing", |_, _| false));
        assert!(!e.history.can_undo());
        e.edit("Add", |d, _| {
            d.add(square(0.0));
            true
        });
        e.mark_saved();
        assert!(!e.is_modified());
        e.checkpoint("Drag");
        e.drop_checkpoint();
        assert_eq!(e.history.len(), 1);
    }

    #[test]
    fn history_is_unlimited_and_cheap() {
        let mut e = Editor::new();
        for i in 0..2000 {
            e.edit("Add", |d, _| {
                d.add(square(i as f64));
                true
            });
        }
        assert_eq!(e.history.len(), 2000);
        while e.undo() {}
        assert!(e.doc.is_empty());
    }

    /// Notes are typed outside the undo history, so stepping through it must not touch them.
    #[test]
    fn undo_and_redo_leave_the_project_notes_alone() {
        let mut e = Editor::new();
        e.edit("Add", |d, _| {
            d.add(square(0.0));
            true
        });
        e.doc.notes = "3 mm birch, two passes".into();
        e.doc.show_notes_on_load = true;
        e.touch();
        e.edit("Add", |d, _| {
            d.add(square(20.0));
            true
        });
        assert!(e.undo() && e.undo());
        assert!(e.doc.is_empty());
        assert_eq!(e.doc.notes, "3 mm birch, two passes", "undoing shapes does not wipe the notes");
        assert!(e.doc.show_notes_on_load);
        e.doc.notes.push_str(" (checked)");
        e.touch();
        assert!(e.redo() && e.redo());
        assert_eq!(e.doc.len(), 2);
        assert_eq!(e.doc.notes, "3 mm birch, two passes (checked)", "redo does not bring old notes back");
        // A dropped checkpoint (a click that moved nothing) does not either.
        e.checkpoint("Drag");
        e.doc.notes = "new".into();
        e.drop_checkpoint();
        assert_eq!(e.doc.notes, "new");
    }

    /// A drag that is cancelled or ends where it began changes nothing: a saved project stays saved.
    #[test]
    fn a_dropped_checkpoint_does_not_mark_a_saved_project_as_changed() {
        let mut e = Editor::new();
        e.edit("Add", |d, _| {
            d.add(square(0.0));
            true
        });
        e.mark_saved();
        let id = e.doc.iter().next().unwrap().id;
        let before = e.revision();
        e.checkpoint("Move");
        e.doc.get_mut(id).unwrap().transform = kurbo::Affine::translate((5.0, 0.0));
        e.touch();
        e.drop_checkpoint();
        assert!(!e.is_modified(), "nothing changed since the save");
        assert!(e.revision() > before, "but the canvas is told to draw again");
        // With unsaved changes before the drag it stays changed.
        e.edit("Add", |d, _| {
            d.add(square(20.0));
            true
        });
        e.checkpoint("Move");
        e.drop_checkpoint();
        assert!(e.is_modified());
    }

    /// Undo back to what was saved (and redo forward to it) is the saved project again.
    #[test]
    fn undo_and_redo_back_to_the_saved_state_are_not_unsaved_changes() {
        let mut e = Editor::new();
        e.edit("Add", |d, _| {
            d.add(square(0.0));
            true
        });
        e.mark_saved();
        e.edit("Add", |d, _| {
            d.add(square(20.0));
            true
        });
        e.edit("Add", |d, _| {
            d.add(square(40.0));
            true
        });
        assert!(e.undo());
        assert!(e.is_modified(), "one step short of the saved state");
        assert!(e.undo());
        assert!(!e.is_modified(), "undo back to the saved state");
        assert!(e.undo());
        assert!(e.is_modified(), "undo past the saved state");
        assert!(e.redo());
        assert!(!e.is_modified(), "redo forward to the saved state");
        assert!(e.redo() && e.redo());
        assert!(e.is_modified());
        assert!(e.undo() && e.undo());
        assert!(!e.is_modified(), "and back again");
        // Notes are not undo steps: with other notes than the saved ones it is not the saved project.
        e.edit("Add", |d, _| {
            d.add(square(60.0));
            true
        });
        e.doc.notes = "changed".into();
        e.touch();
        assert!(e.undo());
        assert!(e.is_modified(), "the notes differ from the saved ones");
        // A new change after an undo past the save can never be undone into the saved state.
        let mut e = Editor::new();
        e.edit("Add", |d, _| {
            d.add(square(0.0));
            true
        });
        e.mark_saved();
        assert!(e.undo());
        e.edit("Add", |d, _| {
            d.add(square(5.0));
            true
        });
        assert!(e.undo());
        assert!(e.is_modified(), "the empty document was never saved");
        assert!(e.redo());
        assert!(e.is_modified());
    }

    /// A shape wrapped in `levels` groups.
    fn nested(levels: usize) -> Shape {
        let mut s = square(0.0);
        for _ in 0..levels {
            s = Shape::new(crate::shape::ShapeKind::Group { children: vec![s] }, kurbo::Affine::IDENTITY, 0);
        }
        s
    }

    #[test]
    fn grouping_past_the_depth_cap_is_refused() {
        let mut e = Editor::new();
        let mut ids = Vec::new();
        assert!(e.edit("Add", |d, _| {
            ids = vec![d.add(nested(MAX_GROUP_DEPTH - 1)), d.add(square(50.0))];
            true
        }));
        assert!(can_group(&e.doc, &ids));
        assert!(e.edit("Group", |d, _| crate::arrange::group(d, &ids).is_some()), "up to the cap is fine");
        let top = e.doc.iter().next().unwrap().id;
        assert_eq!(e.doc.get(top).unwrap().group_depth(), MAX_GROUP_DEPTH);
        let mut pair = Vec::new();
        assert!(e.edit("Add", |d, _| {
            pair = vec![top, d.add(square(80.0))];
            true
        }));
        assert!(!can_group(&e.doc, &pair));
        let before = e.doc.clone();
        assert!(!e.edit("Group", |d, _| crate::arrange::group(d, &pair).is_some()), "one more level is refused");
        assert_eq!(e.doc, before, "nothing changed");
        assert!(!e.edit("Paste", |d, _| {
            d.add(nested(MAX_GROUP_DEPTH + 1));
            true
        }));
        assert_eq!(e.doc, before, "a pasted shape nested too deep is refused too");
    }
}
