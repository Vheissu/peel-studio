//! The user's own templates: the library catalog behind File > New from
//! template > My templates and the start screen, and the Save as template
//! dialog.
//!
//! The catalog reads the library folder at startup, after a template is
//! saved and when the start screen opens; never per frame. Each template
//! keeps its slot (and so its menu command id) for as long as the app
//! runs, so a refresh never points a command at a different template.

use std::path::{Path, PathBuf};

use peel_project::TemplateEntry;
use peel_ui::{Align, Id as W, LayerId, NamedKey, Rect, Ui, vec2};

use crate::studio::Studio;

/// Most templates the catalog tracks in one run (menu ids are reserved
/// for this many).
pub const MAX_TEMPLATES: usize = 1000;

/// The library folder: `Templates` beside the preferences. Tests and debug
/// runs use a private folder (`PEEL_TEMPLATES_DIR` picks one).
pub fn library_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PEEL_TEMPLATES_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(test)
        || std::env::var_os("PEEL_NO_PREFS").is_some()
        || std::env::var_os("PEEL_SCREENSHOT").is_some()
    {
        return std::env::temp_dir().join(format!(
            "peel-templates-{}-{}",
            std::process::id(),
            peel_document::Id::new()
        ));
    }
    crate::prefs::app_dir().join("Templates")
}

pub struct Library {
    dir: PathBuf,
    /// Templates by slot; a template removed since keeps its slot empty.
    slots: Vec<Option<TemplateEntry>>,
    /// Filled slots in name order.
    order: Vec<usize>,
}

impl Library {
    /// The catalog of `dir`, read now.
    pub fn open(dir: PathBuf) -> Library {
        let mut lib = Library {
            dir,
            slots: Vec::new(),
            order: Vec::new(),
        };
        lib.refresh();
        lib
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads the folder again. Known templates keep their slots; new ones
    /// take fresh slots, reusing empty ones only when none are left.
    pub fn refresh(&mut self) {
        let found = peel_project::list_templates(&self.dir);
        let mut seen = vec![false; self.slots.len()];
        let mut fresh = Vec::new();
        for entry in found {
            match self
                .slots
                .iter()
                .position(|s| s.as_ref().is_some_and(|e| e.path == entry.path))
            {
                Some(i) => {
                    seen[i] = true;
                    self.slots[i] = Some(entry);
                }
                None => fresh.push(entry),
            }
        }
        for (i, seen) in seen.into_iter().enumerate() {
            if !seen {
                self.slots[i] = None;
            }
        }
        for entry in fresh {
            if self.slots.len() < MAX_TEMPLATES {
                self.slots.push(Some(entry));
            } else if let Some(free) = self.slots.iter().position(Option::is_none) {
                self.slots[free] = Some(entry);
            } else {
                log::warn!("template library: more than {MAX_TEMPLATES} templates, some hidden");
                break;
            }
        }
        self.order = (0..self.slots.len())
            .filter(|&i| self.slots[i].is_some())
            .collect();
        self.order.sort_by_key(|&i| {
            let name = &self.slots[i].as_ref().expect("filled slot").name;
            (name.to_lowercase(), name.clone())
        });
    }

    /// `(slot, template)` in name order.
    pub fn entries(&self) -> impl Iterator<Item = (usize, &TemplateEntry)> {
        self.order
            .iter()
            .filter_map(|&i| self.slots[i].as_ref().map(|e| (i, e)))
    }

    pub fn get(&self, slot: usize) -> Option<&TemplateEntry> {
        self.slots.get(slot).and_then(Option::as_ref)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// The Save as template dialog while it is open.
#[derive(Clone, Debug, Default)]
pub struct SaveDialog {
    pub name: String,
    pub error: Option<String>,
    /// Keys and text held back from the editors this frame, for the field.
    held: Option<(Vec<peel_ui::KeyPress>, String)>,
    opened: bool,
}

pub(crate) const FIELD: W = W(0x7e3a_17e5_0f1e);

impl Studio {
    /// Opens the Save as template dialog, named after the project.
    pub fn open_save_template(&mut self) {
        if self.pending.is_some() || self.template_dialog.is_some() {
            return;
        }
        let title = self.session.project().meta.title.trim().to_string();
        self.template_dialog = Some(SaveDialog {
            name: if title.is_empty() {
                "Untitled".into()
            } else {
                title
            },
            ..SaveDialog::default()
        });
    }

    /// Saves the project as the template `name`. The project itself, its
    /// file, title, history and saved state stay as they are; a take still
    /// buffered joins the project first so the template has it too.
    pub fn save_template(&mut self, name: &str) -> Result<PathBuf, String> {
        if self.session.is_recording() {
            return Err("Stop recording first".into());
        }
        self.recorder.finish_capture(&mut self.session);
        let dir = self.templates.dir().to_path_buf();
        let result = self.session.save_template(&dir, name);
        self.templates.refresh();
        match result {
            Ok(path) => {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.session.notify(format!("Saved template {name}"), false);
                Ok(path)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// While the dialog is open it owns the keyboard: keys and typed text
    /// wait for its field, and nothing reaches the editors.
    pub(crate) fn hold_dialog_keys(&mut self, ui: &mut Ui) {
        if let Some(d) = &mut self.template_dialog {
            let keys = std::mem::take(&mut ui.input.keys);
            let text = std::mem::take(&mut ui.input.text);
            d.held = Some((keys, text));
        }
    }

    /// Draws the Save as template dialog.
    pub(crate) fn save_template_dialog(&mut self, ui: &mut Ui) {
        let Some(mut d) = self.template_dialog.take() else {
            return;
        };
        if let Some((keys, text)) = d.held.take() {
            ui.input.keys = keys;
            ui.input.text = text;
        }
        if !d.opened {
            d.opened = true;
            ui.close_popup();
            ui.focus_text_edit(FIELD, true);
        }
        // Only Enter or Save submits. A text edit also reports changes
        // on Tab, focus loss and accessibility SetValue, which must not
        // save a template (especially while clicking Cancel).
        let escape = take_key(ui, NamedKey::Escape);
        let enter = take_key(ui, NamedKey::Enter);
        let t = ui.theme.clone();
        let screen = ui.screen();
        let mut save = enter;
        let mut cancel = escape;
        ui.with_layer(LayerId::POPUP, |ui| {
            ui.scene.push_clip_absolute(Rect::EVERYTHING);
            ui.interact(W::new("template-backdrop"), screen);
            ui.scene.rect(screen, 0.0, t.bg.alpha(0.6));
            let w = 400.0f32.min(screen.w - 40.0);
            let panel = Rect::from_center(screen.center(), w, 176.0);
            ui.interact(W::new("template-dialog"), panel);
            ui.scene
                .shadow(panel.translate(vec2(0.0, 8.0)), t.radius_l, 22.0, t.shadow);
            ui.scene
                .rect_stroke(panel, t.radius_l, t.raised, 1.0, t.line_strong);
            {
                let mut body = panel.shrink(20.0);
                ui.describe_label(
                    W::new("template-title"),
                    Rect { h: 22.0, ..body },
                    "Save as template",
                );
                ui.text(
                    body.cut_top(22.0),
                    "Save as template",
                    t.heading,
                    t.text,
                    Align::Start,
                );
                body.cut_top(12.0);
                let field = body.cut_top(30.0);
                ui.text_edit(FIELD, field, &mut d.name, "Template name");
                body.cut_top(6.0);
                let line = body.cut_top(18.0);
                match &d.error {
                    Some(e) => {
                        let e = ui.fit_text(e, t.caption, line.w);
                        ui.text(line, &e, t.caption, t.record, Align::Start);
                        ui.describe_label(W::new("template-error"), line, &e);
                    }
                    None => ui.text(
                        line,
                        "Find it in New from template.",
                        t.caption,
                        t.text_faint,
                        Align::Start,
                    ),
                }
                let mut row = body.cut_bottom(30.0);
                let save_r = row.cut_right(84.0);
                row.cut_right(8.0);
                let cancel_r = row.cut_right(84.0);
                if ui
                    .button(W::new("template-cancel"), cancel_r, "Cancel")
                    .clicked
                {
                    cancel = true;
                }
                if ui
                    .button_primary(W::new("template-save"), save_r, "Save")
                    .clicked
                {
                    save = true;
                }
            }
            ui.scene.pop_clip();
        });
        // Nothing typed here reaches the editors or the typing keys.
        for k in ui.input.keys.iter_mut() {
            k.consumed = true;
        }
        ui.input.text.clear();
        if cancel {
            ui.clear_focus();
            return;
        }
        if save {
            match self.save_template(&d.name) {
                Ok(_) => {
                    ui.clear_focus();
                    return;
                }
                Err(e) => {
                    d.error = Some(e);
                    ui.focus_text_edit(FIELD, true);
                }
            }
        }
        self.template_dialog = Some(d);
    }
}

fn take_key(ui: &mut Ui, key: NamedKey) -> bool {
    let mut hit = false;
    for k in ui.input.keys.iter_mut() {
        if !k.consumed && k.key == peel_ui::Key::Named(key) {
            k.consumed = true;
            hit = true;
        }
    }
    hit
}

/// The start screen's My templates list: a popup under `anchor` with one
/// row per template, scrolling when there are many. Returns the template
/// picked.
pub fn picker(ui: &mut Ui, anchor: Rect, lib: &Library) -> Option<PathBuf> {
    let id = picker_id();
    if !ui.is_popup_open(id) {
        return None;
    }
    let t = ui.theme.clone();
    let screen = ui.screen();
    let row_h = 30.0;
    let content_h = lib.len() as f32 * row_h;
    let w = 260.0;
    // Below the button when the list fits there, else on the roomier side.
    let want = (content_h + 12.0).min(row_h * 8.0 + 12.0);
    let below = screen.bottom() - anchor.bottom() - 12.0;
    let above = anchor.y - screen.y - 12.0;
    let down = below >= want || below >= above;
    let h = want.min(if down { below } else { above }).max(row_h + 12.0);
    let r = Rect {
        x: anchor.x.min(screen.right() - w - 8.0).max(screen.x + 8.0),
        y: if down {
            anchor.bottom() + 4.0
        } else {
            anchor.y - 4.0 - h
        },
        w,
        h,
    };
    let mut picked = None;
    ui.popup_panel(id, r, |ui, r| {
        let view = r.shrink(6.0);
        let mut offset = ui.state::<peel_ui::Vec2>(id.with("scroll")).to_owned();
        ui.scroll_area(id.with("list"), view, vec2(view.w, content_h), &mut offset);
        *ui.state::<peel_ui::Vec2>(id.with("scroll")) = offset;
        ui.scene.push_clip(view);
        for (n, (slot, entry)) in lib.entries().enumerate() {
            let row = Rect {
                x: view.x,
                y: view.y + n as f32 * row_h - offset.y,
                w: view.w - 8.0,
                h: row_h - 2.0,
            };
            if row.bottom() < view.y || row.y > view.bottom() {
                continue;
            }
            let rid = W::new(("welcome-user-template", slot));
            let resp = ui.interact_button(rid, row, &entry.name);
            let hot = ui.animate_bool(rid, resp.hovered);
            if hot > 0.0 {
                ui.scene.rect(row, t.radius_m, t.control_hover.alpha(hot));
            }
            let label = ui.fit_text(&entry.name, t.label, row.w - 20.0);
            ui.text(
                row.shrink2(10.0, 0.0),
                &label,
                t.label,
                t.text,
                Align::Start,
            );
            if resp.hovered {
                ui.set_cursor(peel_ui::CursorIcon::Pointer);
            }
            ui.focus_ring(rid, row, t.radius_m);
            if resp.clicked {
                picked = Some(entry.path.clone());
            }
        }
        ui.scene.pop_clip();
    });
    if picked.is_some() {
        ui.close_popup();
    }
    picked
}

/// Opens the My templates list at the top.
pub fn open_picker(ui: &mut Ui, anchor: Rect) {
    let id = picker_id();
    *ui.state::<peel_ui::Vec2>(id.with("scroll")) = peel_ui::Vec2::ZERO;
    ui.open_popup(id, anchor);
}

pub fn picker_id() -> W {
    W::new("welcome-my-templates-list")
}
