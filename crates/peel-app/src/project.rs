//! Project lifecycle: new, template, open, recover, close and quit all go
//! through [`Studio::request`], which finishes any recording, then either
//! acts at once (nothing unsaved) or holds the change behind the save
//! prompt until the user picks Save, Don't save or Cancel.
//!
//! The prompt is state on the `Studio` (`pending`), not a blocking dialog,
//! so tests drive every path with [`Studio::resolve`].

use std::path::{Path, PathBuf};

use peel_document::Project;
use peel_session::template;
use peel_ui::{Align, Id as W, LayerId, NamedKey, Rect, Ui, vec2};

use crate::studio::Studio;

/// A change that replaces or closes the current project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A blank project: no channels, notes, clips or effects.
    New,
    /// `template::templates()[i]`.
    Template(usize),
    /// The demo song.
    Demo,
    OpenPath(PathBuf),
    Recover(PathBuf),
    /// Back to the start screen over a blank project.
    Close,
    Quit,
}

/// The save prompt's answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Save,
    DontSave,
    Cancel,
}

/// The catalog index of the demo song, which the start screen and menus
/// show apart from the styles.
pub fn demo_index() -> Option<usize> {
    template::templates()
        .iter()
        .position(|(name, _)| *name == "Demo song")
}

/// Template names to offer as starting styles (the demo excluded), with
/// their catalog index.
pub fn styles() -> Vec<(usize, &'static str)> {
    let demo = demo_index();
    template::templates()
        .into_iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != demo)
        .map(|(i, (name, _))| (i, name))
        .collect()
}

fn openable(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "peelproj" | "mid" | "midi")
}

impl Studio {
    /// Asks to make `change`. Unsaved work holds it behind the save prompt;
    /// otherwise it happens now.
    pub fn request(&mut self, change: Change) {
        if self.pending.is_some() {
            return;
        }
        match &change {
            Change::Template(i) if template::templates().get(*i).is_none() => {
                self.session.notify("That template is not available", true);
                return;
            }
            Change::Demo if demo_index().is_none() => {
                self.session.notify("The demo song is not available", true);
                return;
            }
            Change::OpenPath(p) if !openable(p) => {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.session.notify(
                    format!("Can't open {name}: choose a Peel project or MIDI file"),
                    true,
                );
                return;
            }
            _ => {}
        }
        // Recorded notes and audio still buffered join the project first,
        // so they count as unsaved work and are never dropped.
        self.prepare_project_change();
        if self.session.doc.is_dirty() {
            self.pending = Some(change);
        } else {
            self.apply_change(change);
        }
    }

    /// Answers the save prompt.
    pub fn resolve(&mut self, choice: Choice) {
        let Some(change) = self.pending.take() else {
            return;
        };
        match choice {
            Choice::Cancel => {}
            Choice::Save => {
                // A cancelled Save As or a failed save keeps everything.
                if self.save() {
                    self.apply_change(change);
                }
            }
            Choice::DontSave => {
                let outgoing = self.session.project().id;
                if self.apply_change(change) {
                    self.discard_autosave(outgoing);
                }
            }
        }
    }

    /// Makes the change without asking. Returns false when it failed and
    /// the current project stayed.
    fn apply_change(&mut self, change: Change) -> bool {
        match change {
            Change::New => self.start(template::empty()),
            Change::Template(i) => match template::templates().get(i) {
                Some((_, make)) => self.start(make()),
                None => return false,
            },
            Change::Demo => match demo_index() {
                Some(i) => self.start((template::templates()[i].1)()),
                None => return false,
            },
            Change::Close => {
                self.start(template::empty());
                self.welcome = true;
            }
            Change::OpenPath(path) => return self.load_path(path),
            Change::Recover(path) => {
                self.silence();
                match self.session.open(&path) {
                    Ok(()) => {
                        self.session.path = None;
                        self.reset_workspace();
                        self.welcome = false;
                        self.recovered.retain(|e| e.path != path);
                        let title = self.session.project().meta.title.clone();
                        self.session
                            .notify(format!("Recovered {title}. Save it to keep it."), false);
                    }
                    Err(e) => {
                        self.session.notify(format!("Could not recover: {e}"), true);
                        return false;
                    }
                }
            }
            Change::Quit => {
                // Nothing left to keep: a clean exit leaves no autosave.
                self.session.doc.mark_saved();
                self.quit = true;
            }
        }
        true
    }

    /// Replaces the project with `project`: a clean document with no path
    /// and no undo history, the workspace reset (layout prefs kept).
    fn start(&mut self, project: Project) {
        self.silence();
        self.session.replace_project(project);
        self.reset_workspace();
        self.welcome = false;
    }

    /// Stops everything sounding before the project goes: playback,
    /// previews, held computer keys and any expression in flight.
    pub(crate) fn silence(&mut self) {
        self.release_held();
        crate::instrument_editor::release_all(&mut self.session, &mut self.ws);
        if self.session.performing() {
            self.session.stop_performance();
        }
        self.session.stop_preview();
        self.session.stop();
        self.session.panic();
    }

    fn discard_autosave(&self, id: peel_document::Id) {
        if cfg!(test) || std::env::var_os("PEEL_NO_PREFS").is_some() {
            return;
        }
        let _ = peel_project::discard_autosave(&crate::prefs::autosave_dir(), id);
    }

    /// Draws the save prompt while a change waits on it.
    pub(crate) fn save_prompt(&mut self, ui: &mut Ui) {
        if self.pending.is_none() {
            return;
        }
        let t = ui.theme.clone();
        let screen = ui.screen();
        let title = self.session.project().meta.title.clone();
        let mut choice = None;
        ui.with_layer(LayerId::POPUP, |ui| {
            ui.scene.push_clip_absolute(Rect::EVERYTHING);
            ui.interact(W::new("save-prompt-backdrop"), screen);
            ui.scene.rect(screen, 0.0, t.bg.alpha(0.6));
            let w = 420.0f32.min(screen.w - 40.0);
            let panel = Rect::from_center(screen.center(), w, 150.0);
            ui.interact(W::new("save-prompt"), panel);
            ui.scene
                .shadow(panel.translate(vec2(0.0, 8.0)), t.radius_l, 22.0, t.shadow);
            ui.scene
                .rect_stroke(panel, t.radius_l, t.raised, 1.0, t.line_strong);
            let mut body = panel.shrink(20.0);
            let heading = format!("Save changes to {title}?");
            let heading = ui.fit_text(&heading, t.heading, body.w);
            ui.text(
                body.cut_top(22.0),
                &heading,
                t.heading,
                t.text,
                Align::Start,
            );
            body.cut_top(6.0);
            ui.text(
                body.cut_top(20.0),
                "Changes you don't save are lost.",
                t.body,
                t.text_dim,
                Align::Start,
            );
            let mut row = body.cut_bottom(30.0);
            let save = row.cut_right(84.0);
            row.cut_right(8.0);
            let cancel = row.cut_right(84.0);
            let dont = row.cut_left(104.0);
            if ui
                .button_primary(W::new("save-prompt-save"), save, "Save")
                .clicked
            {
                choice = Some(Choice::Save);
            }
            if ui
                .button(W::new("save-prompt-cancel"), cancel, "Cancel")
                .clicked
            {
                choice = Some(Choice::Cancel);
            }
            if ui
                .button(W::new("save-prompt-discard"), dont, "Don't save")
                .clicked
            {
                choice = Some(Choice::DontSave);
            }
            ui.scene.pop_clip();
        });
        if let Some(c) = choice {
            self.resolve(c);
        }
        self.prompt_keys(ui);
    }

    /// Handle the modal's keys before any editor can consume them.
    pub(crate) fn prompt_keys(&mut self, ui: &mut Ui) {
        if self.pending.is_none() {
            return;
        }
        let mut choice = None;
        if ui.input.take_named(NamedKey::Escape) {
            choice = Some(Choice::Cancel);
        } else if ui.input.take_named(NamedKey::Enter) {
            choice = Some(Choice::Save);
        } else if ui.input.take_command("d", false) {
            choice = Some(Choice::DontSave);
        }
        // The prompt owns the keyboard: nothing reaches the typing keys.
        for k in ui.input.keys.iter_mut() {
            k.consumed = true;
        }
        ui.input.text.clear();
        if let Some(c) = choice {
            self.resolve(c);
        }
    }
}
