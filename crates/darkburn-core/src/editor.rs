//! The editing session: the document, what is selected, undo and redo, the current layer and
//! whether anything changed since the last save.
//!
//! Undo keeps whole snapshots of the document. Shapes are shared between snapshots, so a step
//! costs one pointer per shape plus the layer table, and history is unlimited.

use crate::clipboard::Clipboard;
use crate::document::Document;
use crate::shape::ShapeId;

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
}

/// The editing session for one open project.
#[derive(Clone, Debug, Default)]
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
}

impl Editor {
    pub fn new() -> Editor {
        Editor::default()
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
    }

    /// Note a change that is not an undo step (selection, view).
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    /// Record the state before a change called `label`. Call it, then change `doc`.
    pub fn checkpoint(&mut self, label: &str) {
        self.history.undo.push(Snapshot {
            doc: self.doc.clone(),
            selection: self.selection.clone(),
            label: label.to_string(),
            revision: self.revision,
        });
        self.history.redo.clear();
        self.revision += 1;
    }

    /// Make an undoable change: `f` changes the document and selection; nothing is recorded
    /// when it returns false.
    pub fn edit(&mut self, label: &str, f: impl FnOnce(&mut Document, &mut Selection) -> bool) -> bool {
        let before = Snapshot { doc: self.doc.clone(), selection: self.selection.clone(), label: label.to_string(), revision: self.revision };
        let changed = f(&mut self.doc, &mut self.selection);
        if changed {
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

    /// Forget the last checkpoint when the change it guarded did nothing (a click without a drag).
    pub fn drop_checkpoint(&mut self) {
        if let Some(s) = self.history.undo.pop() {
            self.restore_doc(s.doc);
            self.selection = s.selection;
            // Back at the state of the checkpoint: still saved if it was saved then.
            let was_saved = s.revision == self.saved_revision;
            self.revision += 1;
            if was_saved {
                self.saved_revision = self.revision;
            }
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
        let now = Snapshot {
            doc: self.restore_doc(s.doc),
            selection: std::mem::replace(&mut self.selection, s.selection),
            label: s.label,
            revision: self.revision,
        };
        self.history.redo.push(now);
        self.revision += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.history.redo.pop() else { return false };
        let now = Snapshot {
            doc: self.restore_doc(s.doc),
            selection: std::mem::replace(&mut self.selection, s.selection),
            label: s.label,
            revision: self.revision,
        };
        self.history.undo.push(now);
        self.revision += 1;
        true
    }

    /// The selected ids that still exist, in selection order.
    pub fn selected(&self) -> Vec<ShapeId> {
        self.selection.ids().iter().copied().filter(|id| self.doc.get(*id).is_some()).collect()
    }
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
}
