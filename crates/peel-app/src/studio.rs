//! The main window: layout, global shortcuts and the computer keyboard.

use std::path::PathBuf;

use peel_document::Id;
use peel_session::{ExportSettings, PlayMode, Session};
use peel_ui::Id as W;
use peel_ui::{Align, App, KeyCode, NamedKey, Rect, Ui, WindowControl};

use crate::prefs::Prefs;
use crate::welcome::{WelcomeAction, WelcomeData};
use crate::workspace::{DockTab, Focus, Workspace};
use crate::{browser, inspector, mixer, piano_roll, playlist, rack, transport};

pub struct Studio {
    pub session: Session,
    pub ws: Workspace,
    pub prefs: Prefs,
    pub welcome: bool,
    pub show_keys: bool,
    recovered: Vec<peel_project::AutosaveEntry>,
    last_autosave: std::time::Instant,
    autosaved_revision: u64,
}

/// Seconds between autosaves of a changed project.
const AUTOSAVE_EVERY: f32 = 60.0;

/// Gap between panes.
pub const GAP: f32 = 6.0;

impl Studio {
    pub fn new(session: Session) -> Studio {
        let mut ws = Workspace::default();
        ws.selected_channel = session
            .project()
            .channels
            .iter()
            .find(|c| c.name == "Piano")
            .or(session.project().channels.first())
            .map(|c| c.id);
        ws.focus = ws.selected_channel.map(Focus::Channel);
        let prefs = Prefs::load();
        ws.octave = prefs.octave;
        if let Some(dir) = prefs.sample_dir.clone().filter(|d| d.is_dir()) {
            ws.sample_dir = Some(dir);
        }
        let recovered = if cfg!(test) {
            Vec::new()
        } else {
            peel_project::list_autosaves(&crate::prefs::autosave_dir()).unwrap_or_default()
        };
        let screenshot = std::env::var_os("PEEL_SCREENSHOT").is_some();
        let welcome = !cfg!(test) && !screenshot && (prefs.show_welcome || !recovered.is_empty());
        let revision = session.doc.revision();
        let mut studio = Studio {
            session,
            ws,
            prefs,
            welcome: welcome || std::env::var_os("PEEL_WELCOME").is_some(),
            show_keys: std::env::var_os("PEEL_KEYS").is_some(),
            recovered,
            last_autosave: std::time::Instant::now(),
            autosaved_revision: revision,
        };
        studio.apply_debug_env();
        studio
    }

    fn reset_workspace(&mut self) {
        let sample_dir = self.ws.sample_dir.clone();
        let octave = self.ws.octave;
        self.ws = Workspace::default();
        self.ws.sample_dir = sample_dir;
        self.ws.octave = octave;
        self.ws.selected_channel = self
            .session
            .project()
            .channels
            .iter()
            .find(|c| c.name == "Piano")
            .or(self.session.project().channels.first())
            .map(|c| c.id);
        self.ws.focus = self.ws.selected_channel.map(Focus::Channel);
        self.autosaved_revision = self.session.doc.revision();
    }

    fn welcome(&mut self, ui: &mut Ui) {
        let recent = self.prefs.existing_recent();
        let mut show = self.prefs.show_welcome;
        let action = crate::welcome::draw(
            ui,
            WelcomeData {
                recent: &recent,
                recovered: &self.recovered,
                show_at_startup: &mut show,
            },
        );
        if show != self.prefs.show_welcome {
            self.prefs.show_welcome = show;
            self.prefs.save();
        }
        let Some(action) = action else { return };
        self.welcome = false;
        match action {
            WelcomeAction::NewBeat => {
                self.session
                    .replace_project(peel_session::template::beat_and_melody());
                self.reset_workspace();
            }
            WelcomeAction::Empty => {
                let mut p = peel_session::template::empty();
                p.mixer.tracks[0]
                    .inserts
                    .push(peel_session::template::device("peel.fx.limiter", None));
                self.session.replace_project(p);
                self.reset_workspace();
            }
            WelcomeAction::Open => self.open(),
            WelcomeAction::OpenPath(p) => self.open_path(p),
            WelcomeAction::Recover(p) => match self.session.open(&p) {
                Ok(()) => {
                    self.session.path = None;
                    self.reset_workspace();
                    self.recovered.retain(|e| e.path != p);
                    let title = self.session.project().meta.title.clone();
                    self.session
                        .notify(format!("Recovered {title}. Save it to keep it."), false);
                }
                Err(e) => self.session.notify(format!("Could not recover: {e}"), true),
            },
            WelcomeAction::Close => {}
        }
    }

    /// Periodically saves changed work to the autosave area off the UI
    /// thread so a crash never loses more than a minute.
    fn autosave(&mut self, force: bool) {
        if cfg!(test) {
            return;
        }
        let rev = self.session.doc.revision();
        if rev == self.autosaved_revision || !self.session.doc.is_dirty() {
            return;
        }
        if !force && self.last_autosave.elapsed().as_secs_f32() < AUTOSAVE_EVERY {
            return;
        }
        self.last_autosave = std::time::Instant::now();
        self.autosaved_revision = rev;
        let project = self.session.project().clone();
        let source = self.session.path.clone();
        let root = crate::prefs::autosave_dir();
        let job = move || {
            let r = match &source {
                Some(src) => peel_project::autosave_from(&project, &root, src),
                None => peel_project::autosave(&project, &root),
            };
            if let Err(e) = r {
                log::warn!("autosave failed: {e}");
            }
        };
        if force {
            job();
        } else {
            std::thread::spawn(job);
        }
    }

    fn saved(&mut self, path: &std::path::Path) {
        self.prefs.add_recent(path);
        let id = self.session.project().id;
        let _ = peel_project::discard_autosave(&crate::prefs::autosave_dir(), id);
        self.autosaved_revision = self.session.doc.revision();
    }

    /// `PEEL_DOCK=rack|roll|mixer`, `PEEL_FOCUS=master` and `PEEL_PLAY=1`
    /// set up a view for screenshots and manual checks.
    fn apply_debug_env(&mut self) {
        match std::env::var("PEEL_DOCK").as_deref() {
            Ok("roll") => self.ws.dock = DockTab::PianoRoll,
            Ok("mixer") => self.ws.dock = DockTab::Mixer,
            _ => {}
        }
        if std::env::var_os("PEEL_EQ").is_some() {
            let master = self.session.project().mixer.tracks[0].id;
            let eq = peel_session::template::device("peel.fx.eq", Some("Vocal Presence"));
            self.session
                .doc
                .edit("Add EQ", |p| p.mixer.tracks[0].inserts.insert(0, eq));
            self.ws.focus = Some(Focus::Track(master));
        }
        if std::env::var_os("PEEL_EXPORT").is_some() {
            self.ws.export_panel = true;
        }
        if std::env::var_os("PEEL_SCALE").is_some() {
            self.ws.roll.scale = Some(crate::harmony::Scale { root: 9, kind: 1 });
        }
        if std::env::var("PEEL_FOCUS").as_deref() == Ok("master") {
            self.ws.focus = Some(Focus::Track(self.session.project().mixer.tracks[0].id));
        }
        if let Ok(name) = std::env::var("PEEL_PATTERN")
            && let Some(p) = self
                .session
                .project()
                .patterns
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.id)
        {
            self.session.select_pattern(p);
        }
        if std::env::var("PEEL_SONG").is_ok() {
            self.session.set_mode(PlayMode::Song);
        }
        if std::env::var("PEEL_PLAY").is_ok() {
            self.session.play();
        }
    }

    fn shortcuts(&mut self, ui: &mut Ui, win: &WindowControl) {
        if ui.text_focus() {
            return;
        }
        let input = &mut ui.input;
        if input.take_command("z", false)
            && let Some(label) = self.session.doc.undo()
        {
            self.session.notify(format!("Undo {label}"), false);
        }
        if (input.take_command("z", true) || input.take_command("y", false))
            && let Some(label) = self.session.doc.redo()
        {
            self.session.notify(format!("Redo {label}"), false);
        }
        if input.take_command("s", true) {
            self.save_as();
        } else if input.take_command("s", false) {
            self.save();
        }
        if input.take_command("o", false) {
            self.open();
        }
        if input.take_command("n", false) {
            self.session
                .replace_project(peel_session::template::beat_and_melody());
            self.ws = Workspace::default();
        }
        if input.take_command("e", false) {
            self.export();
        }
        if input.take_command("q", false) {
            win.exit();
        }
        if ui.input.take_named(NamedKey::Space) {
            self.session.toggle_play();
        }
        let mut switch = |key: NamedKey, tab: DockTab, ws: &mut Workspace| {
            if ui.input.take_named(key) {
                ws.dock = tab;
            }
        };
        switch(NamedKey::F6, DockTab::Rack, &mut self.ws);
        switch(NamedKey::F7, DockTab::PianoRoll, &mut self.ws);
        switch(NamedKey::F9, DockTab::Mixer, &mut self.ws);
        if ui.input.take_named(NamedKey::F1) {
            self.show_keys = !self.show_keys;
        }
        if ui.input.take_named(NamedKey::Escape) {
            self.session.stop_preview();
        }
        let l_pressed = ui
            .input
            .keys
            .iter_mut()
            .find(|k| !k.consumed && k.code == Some(KeyCode::KeyL) && k.modifiers.none());
        if let Some(k) = l_pressed {
            k.consumed = true;
            let next = match self.session.mode() {
                PlayMode::Song => PlayMode::Pattern(self.current_pattern()),
                PlayMode::Pattern(_) => PlayMode::Song,
            };
            self.session.set_mode(next);
        }
        self.typing_keyboard(ui);
    }

    /// Selected pattern (the pattern-mode target, or the first pattern).
    pub fn current_pattern(&self) -> Id {
        current_pattern(&self.session)
    }

    fn typing_keyboard(&mut self, ui: &mut Ui) {
        // Two rows, like a tracker: Z row is the lower octave, Q row the upper.
        const LOWER: [(KeyCode, i32); 17] = [
            (KeyCode::KeyZ, 0),
            (KeyCode::KeyS, 1),
            (KeyCode::KeyX, 2),
            (KeyCode::KeyD, 3),
            (KeyCode::KeyC, 4),
            (KeyCode::KeyV, 5),
            (KeyCode::KeyG, 6),
            (KeyCode::KeyB, 7),
            (KeyCode::KeyH, 8),
            (KeyCode::KeyN, 9),
            (KeyCode::KeyJ, 10),
            (KeyCode::KeyM, 11),
            (KeyCode::Comma, 12),
            (KeyCode::KeyL, 13),
            (KeyCode::Period, 14),
            (KeyCode::Semicolon, 15),
            (KeyCode::Slash, 16),
        ];
        const UPPER: [(KeyCode, i32); 17] = [
            (KeyCode::KeyQ, 12),
            (KeyCode::Digit2, 13),
            (KeyCode::KeyW, 14),
            (KeyCode::Digit3, 15),
            (KeyCode::KeyE, 16),
            (KeyCode::KeyR, 17),
            (KeyCode::Digit5, 18),
            (KeyCode::KeyT, 19),
            (KeyCode::Digit6, 20),
            (KeyCode::KeyY, 21),
            (KeyCode::Digit7, 22),
            (KeyCode::KeyU, 23),
            (KeyCode::KeyI, 24),
            (KeyCode::Digit9, 25),
            (KeyCode::KeyO, 26),
            (KeyCode::Digit0, 27),
            (KeyCode::KeyP, 28),
        ];
        let Some(channel) = self.ws.selected_channel else {
            return;
        };
        let base = (self.ws.octave + 1) * 12;
        let lookup = |code: KeyCode| {
            LOWER
                .iter()
                .chain(UPPER.iter())
                .find(|(c, _)| *c == code)
                .map(|(_, o)| *o)
        };
        let mut ons = Vec::new();
        for k in ui.input.keys.iter_mut() {
            if k.consumed || k.modifiers.command() || k.modifiers.ctrl || k.modifiers.alt {
                continue;
            }
            let Some(code) = k.code else { continue };
            if code == KeyCode::Minus || code == KeyCode::Equal {
                k.consumed = true;
                if !k.repeat {
                    self.ws.octave =
                        (self.ws.octave + if code == KeyCode::Equal { 1 } else { -1 }).clamp(0, 8);
                }
                continue;
            }
            // KeyL doubles as the mode switch when nothing else uses it.
            if code == KeyCode::KeyL {
                continue;
            }
            if let Some(off) = lookup(code) {
                k.consumed = true;
                if !k.repeat && !self.ws.held.contains_key(&code) {
                    let key = (base + off).clamp(0, 127) as u8;
                    ons.push((code, key));
                }
            }
        }
        for (code, key) in ons {
            self.ws.held.insert(code, (channel, key));
            self.session.live_note(channel, key, 0.8, true);
        }
        let released: Vec<KeyCode> = ui
            .input
            .keys_released
            .iter()
            .filter_map(|k| k.code)
            .collect();
        for code in released {
            if let Some((ch, key)) = self.ws.held.remove(&code) {
                self.session.live_note(ch, key, 0.0, false);
            }
        }
    }

    pub fn save(&mut self) {
        if self.session.path.is_some() {
            match self.session.save(None) {
                Ok(p) => self.saved(&p),
                Err(e) => self.session.notify(format!("Save failed: {e}"), true),
            }
        } else {
            self.save_as();
        }
    }

    pub fn save_as(&mut self) {
        let name = format!("{}.peelproj", self.session.project().meta.title);
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter("Peel project", &["peelproj"])
            .save_file()
        {
            match self.session.save(Some(path)) {
                Ok(p) => self.saved(&p),
                Err(e) => self.session.notify(format!("Save failed: {e}"), true),
            }
        }
    }

    pub fn open(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Peel project", &["peelproj"])
            .add_filter("MIDI file", &["mid", "midi"])
            .pick_file()
        else {
            return;
        };
        self.open_path(path);
    }

    pub fn open_path(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let ok = if ext == "mid" || ext == "midi" {
            crate::midi::import(&mut self.session, &path)
        } else {
            match self.session.open(&path) {
                Ok(()) => true,
                Err(e) => {
                    self.session
                        .notify(format!("Could not open project: {e}"), true);
                    false
                }
            }
        };
        if ok {
            self.welcome = false;
            self.reset_workspace();
            if ext == "peelproj" {
                self.prefs.add_recent(&path);
            }
        }
    }

    /// Opens the export panel (from the shortcut or the toolbar).
    pub fn export(&mut self) {
        self.ws.export_panel = true;
    }

    /// Renders the arrangement to WAV with the panel's settings.
    pub fn export_audio(&mut self) {
        if self.ws.export.is_some() {
            return;
        }
        let name = format!("{}.wav", self.session.project().meta.title);
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter("WAV audio", &["wav"])
            .save_file()
        else {
            return;
        };
        let settings = ExportSettings {
            path,
            sample_rate: self.ws.export_rate,
            bits: self.ws.export_bits,
            tail_seconds: self.ws.export_tail,
        };
        self.ws.export = Some(self.session.export(settings));
    }

    /// Writes the arrangement as a Standard MIDI File.
    pub fn export_midi(&mut self) {
        let name = format!("{}.mid", self.session.project().meta.title);
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter("MIDI file", &["mid"])
            .save_file()
        else {
            return;
        };
        match peel_midi::export_arrangement(
            self.session.project(),
            &peel_midi::ExportOptions::default(),
        ) {
            Ok(out) => match std::fs::write(&path, &out.bytes) {
                Ok(()) => self
                    .session
                    .notify(format!("Exported {}", path.display()), false),
                Err(e) => self
                    .session
                    .notify(format!("Could not write MIDI: {e}"), true),
            },
            Err(e) => self
                .session
                .notify(format!("Could not export MIDI: {e}"), true),
        }
    }

    fn status_bar(&mut self, ui: &mut Ui, r: Rect) {
        let t = ui.theme.clone();
        let mut row = r.shrink2(12.0, 0.0);
        if let Some(job) = &self.ws.export {
            if let Some(result) = job.take_result() {
                match result {
                    Ok(p) => self
                        .session
                        .notify(format!("Exported {}", p.display()), false),
                    Err(e) => self.session.notify(e, true),
                }
                self.ws.export = None;
            } else {
                let bar = row.cut_left(160.0).shrink2(0.0, 9.0);
                ui.scene.rect(bar, 3.0, t.control);
                ui.scene
                    .rect(bar.with_w(bar.w * job.progress()), 3.0, t.accent);
                row.cut_left(8.0);
                ui.text(
                    row.cut_left(80.0),
                    "Exporting",
                    t.caption,
                    t.text_dim,
                    Align::Start,
                );
                ui.request_repaint();
            }
        }
        if let Some(n) = self.session.notice().cloned() {
            let c = if n.error { t.record } else { t.text_dim };
            ui.text(
                row.cut_left(row.w * 0.6),
                &n.text,
                t.caption,
                c,
                Align::Start,
            );
            ui.request_repaint();
        } else if !self.session.missing_devices().is_empty() {
            let msg = format!(
                "Missing devices: {}",
                self.session.missing_devices().join(", ")
            );
            ui.text(
                row.cut_left(row.w * 0.6),
                &msg,
                t.caption,
                t.warn,
                Align::Start,
            );
        }
        let help = ui.icon_button(
            W::new("keys-help"),
            row.cut_right(24.0),
            peel_ui::icons::KEYBOARD,
            false,
            None,
        );
        ui.tooltip(&help, "Keyboard shortcuts (F1)");
        if help.clicked {
            self.show_keys = true;
        }
        row.cut_right(8.0);
        let right = format!(
            "{}  ·  {:.0} kHz  ·  octave {}",
            self.session.device_name(),
            self.session.sample_rate() / 1000.0,
            self.ws.octave
        );
        ui.text(row, &right, t.caption, t.text_faint, Align::End);
    }
}

pub fn current_pattern(session: &Session) -> Id {
    session.selected_pattern()
}

impl App for Studio {
    fn update(&mut self, ui: &mut Ui, win: &WindowControl) {
        self.session.tick();
        if !self.welcome {
            self.shortcuts(ui, win);
        }
        self.autosave(false);
        let t = ui.theme.clone();
        let screen = ui.screen();
        ui.scene.rect(screen, 0.0, t.bg);

        // Files dropped from the OS anywhere else open as projects/MIDI.
        let mut area = screen;
        let top = area.cut_top(52.0);
        let status = area.cut_bottom(24.0);
        area = area.shrink2(GAP, 0.0);

        transport::draw(ui, top, win, self);

        let browser_r = area.cut_left(self.ws.browser_w);
        let d = ui.splitter(W::new("split-browser"), area.cut_left(GAP), true);
        self.ws.browser_w = (self.ws.browser_w + d).clamp(180.0, 420.0);
        let inspector_r = area.cut_right(self.ws.inspector_w);
        let d = ui.splitter(W::new("split-inspector"), area.cut_right(GAP), true);
        self.ws.inspector_w = (self.ws.inspector_w - d).clamp(240.0, 480.0);
        let max_dock = (area.h - 160.0).max(180.0);
        self.ws.dock_h = self.ws.dock_h.clamp(180.0, max_dock);
        let dock_r = area.cut_bottom(self.ws.dock_h);
        let d = ui.splitter(W::new("split-dock"), area.cut_bottom(GAP), false);
        self.ws.dock_h = (self.ws.dock_h - d).clamp(180.0, max_dock);
        let playlist_r = area;

        browser::draw(ui, browser_r, &mut self.session, &mut self.ws);
        playlist::draw(ui, playlist_r, &mut self.session, &mut self.ws);
        dock(ui, dock_r, self);
        inspector::draw(ui, inspector_r, &mut self.session, &mut self.ws);
        self.status_bar(ui, status);

        let leftovers = std::mem::take(&mut ui.input.dropped_files);
        for path in leftovers {
            self.open_path(path);
        }

        if self.welcome {
            self.welcome(ui);
        } else if self.show_keys && crate::keys::overlay(ui) {
            self.show_keys = false;
        }
        if self.session.is_playing() {
            ui.request_repaint();
        }
        let dirty = if self.session.doc.is_dirty() {
            " •"
        } else {
            ""
        };
        win.set_title(&format!(
            "{}{} — Peel Studio",
            self.session.project().meta.title,
            dirty
        ));
    }

    fn close_requested(&mut self) -> bool {
        true
    }

    fn shutdown(&mut self) {
        self.session.panic();
        // Unsaved work stays recoverable; a clean session leaves nothing.
        if self.session.doc.is_dirty() {
            self.autosave(true);
        } else if !cfg!(test) {
            let _ = peel_project::discard_autosave(
                &crate::prefs::autosave_dir(),
                self.session.project().id,
            );
        }
        self.prefs.octave = self.ws.octave;
        self.prefs.sample_dir = self.ws.sample_dir.clone();
        self.prefs.save();
    }
}

fn dock(ui: &mut Ui, r: Rect, studio: &mut Studio) {
    let t = ui.theme.clone();
    ui.scene.rect(r, t.radius_l, t.panel);
    let mut body = r;
    let mut tabs = body.cut_top(38.0).shrink2(8.0, 6.0);
    let items = [
        (DockTab::Rack, peel_ui::icons::ROWS, "Channels"),
        (DockTab::PianoRoll, peel_ui::icons::PIANO_KEYS, "Piano roll"),
        (DockTab::Mixer, peel_ui::icons::FADERS, "Mixer"),
    ];
    for (tab, icon, label) in items {
        // Only the active tab spells out its name.
        let active = studio.ws.dock == tab;
        let w = if active {
            ui.measure(label, t.label).x + 52.0
        } else {
            38.0
        };
        let cell = tabs.cut_left(w);
        tabs.cut_left(4.0);
        let id = W::new(("dock-tab", label));
        let resp = ui.interact(id, cell);
        let on = studio.ws.dock == tab;
        let hot = ui.animate_bool(id, resp.hovered);
        if on {
            ui.scene.rect(cell, t.radius_m, t.raised);
        } else if hot > 0.0 {
            ui.scene.rect(cell, t.radius_m, t.raised.alpha(hot * 0.5));
        }
        let c = if on { t.text } else { t.text_dim };
        if active {
            let mut inner = cell.shrink2(10.0, 0.0);
            ui.icon(inner.cut_left(18.0), icon, 15.0, t.accent);
            inner.cut_left(6.0);
            ui.text(inner, label, t.label, c, Align::Start);
        } else {
            ui.icon(cell, icon, 15.0, c);
            let shortcut = match tab {
                DockTab::Rack => "F6",
                DockTab::PianoRoll => "F7",
                DockTab::Mixer => "F9",
            };
            ui.tooltip(&resp, &format!("{label} ({shortcut})"));
        }
        if resp.clicked {
            studio.ws.dock = tab;
        }
        if resp.hovered {
            ui.set_cursor(peel_ui::CursorIcon::Pointer);
        }
    }
    let tools = tabs;
    let content = body.shrink2(0.0, 0.0);
    match studio.ws.dock {
        DockTab::Rack => rack::draw(ui, content, tools, &mut studio.session, &mut studio.ws),
        DockTab::PianoRoll => {
            piano_roll::draw(ui, content, tools, &mut studio.session, &mut studio.ws)
        }
        DockTab::Mixer => mixer::draw(ui, content, &mut studio.session, &mut studio.ws),
    }
}
