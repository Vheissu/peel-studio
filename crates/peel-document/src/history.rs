//! Transactional editing with checkpoint-based undo.
//!
//! Every edit runs as a named transaction against the live [`Project`]. The
//! state before the transaction is kept as a checkpoint; because heavy
//! collections are `Arc`-shared, a checkpoint costs a shallow copy plus the
//! objects the transaction actually changed. A drag or knob gesture coalesces
//! into one history entry.

use crate::Project;
use crate::validate::{Issue, validate};

const DEFAULT_LIMIT: usize = 500;

#[derive(Clone, Debug)]
struct Checkpoint {
    label: String,
    before: Project,
}

/// The editable document plus its undo history.
#[derive(Clone, Debug)]
pub struct Document {
    project: Project,
    undo: Vec<Checkpoint>,
    redo: Vec<Checkpoint>,
    limit: usize,
    gesture: Option<u64>,
    saved_revision: u64,
}

impl Document {
    pub fn new(project: Project) -> Document {
        let saved_revision = project.revision;
        Document {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            limit: DEFAULT_LIMIT,
            gesture: None,
            saved_revision,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn revision(&self) -> u64 {
        self.project.revision
    }

    /// True when the document changed since it was last marked saved.
    pub fn is_dirty(&self) -> bool {
        self.project.revision != self.saved_revision
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.project.revision;
    }

    /// Runs `f` as one undoable transaction labelled in musical terms
    /// ("Move 8 notes"). Ends any open gesture.
    pub fn edit<R>(&mut self, label: impl Into<String>, f: impl FnOnce(&mut Project) -> R) -> R {
        self.gesture = None;
        self.push_checkpoint(label.into());
        self.apply(f)
    }

    /// Runs `f` as part of a continuous gesture (drag, knob turn). The first
    /// call with a new `gesture` key records a checkpoint; later calls with
    /// the same key extend that history entry.
    pub fn edit_gesture<R>(
        &mut self,
        gesture: u64,
        label: impl Into<String>,
        f: impl FnOnce(&mut Project) -> R,
    ) -> R {
        if self.gesture != Some(gesture) {
            self.gesture = Some(gesture);
            self.push_checkpoint(label.into());
        }
        self.apply(f)
    }

    /// Changes data that is not part of the musical history (for example
    /// plugin state captured on save): no undo entry, no revision change.
    pub fn patch(&mut self, f: impl FnOnce(&mut Project)) {
        f(&mut self.project);
    }

    /// Renames the most recent undo step (a previewed tool whose choice
    /// changed while it was shaped).
    pub fn relabel_last(&mut self, label: impl Into<String>) {
        if let Some(last) = self.undo.last_mut() {
            last.label = label.into();
        }
    }

    /// Closes the current gesture so the next edit starts a new entry.
    pub fn end_gesture(&mut self) {
        self.gesture = None;
    }

    /// Replaces the project without history (used after loading).
    pub fn replace(&mut self, project: Project) {
        self.project = project;
        self.undo.clear();
        self.redo.clear();
        self.gesture = None;
        self.saved_revision = self.project.revision;
    }

    fn push_checkpoint(&mut self, label: String) {
        self.redo.clear();
        self.undo.push(Checkpoint {
            label,
            before: self.project.clone(),
        });
        if self.undo.len() > self.limit {
            let excess = self.undo.len() - self.limit;
            self.undo.drain(..excess);
        }
    }

    fn apply<R>(&mut self, f: impl FnOnce(&mut Project) -> R) -> R {
        let result = f(&mut self.project);
        self.project.revision += 1;
        debug_assert!(
            validate(&self.project).iter().all(|i| !i.is_error()),
            "transaction produced an invalid project: {:?}",
            validate(&self.project)
        );
        result
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|c| c.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|c| c.label.as_str())
    }

    /// Labels of the redo stack, next redo last.
    pub fn redo_labels(&self) -> impl Iterator<Item = &str> {
        self.redo.iter().map(|c| c.label.as_str())
    }

    /// Labels of the undo stack, most recent last.
    pub fn history_labels(&self) -> impl Iterator<Item = &str> {
        self.undo.iter().map(|c| c.label.as_str())
    }

    /// Restores the state before the last transaction. Returns its label.
    pub fn undo(&mut self) -> Option<String> {
        self.gesture = None;
        let cp = self.undo.pop()?;
        let revision = self.project.revision + 1;
        let after = std::mem::replace(&mut self.project, cp.before);
        self.project.revision = revision;
        self.redo.push(Checkpoint {
            label: cp.label.clone(),
            before: after,
        });
        Some(cp.label)
    }

    /// Re-applies the last undone transaction. Returns its label.
    pub fn redo(&mut self) -> Option<String> {
        self.gesture = None;
        let cp = self.redo.pop()?;
        let revision = self.project.revision + 1;
        let before = std::mem::replace(&mut self.project, cp.before);
        self.project.revision = revision;
        self.undo.push(Checkpoint {
            label: cp.label.clone(),
            before,
        });
        Some(cp.label)
    }

    /// Integrity report for the current project.
    pub fn validate(&self) -> Vec<Issue> {
        validate(&self.project)
    }
}
