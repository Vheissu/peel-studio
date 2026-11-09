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
    pub recorder: crate::recorder::Recorder,
    recovered: Vec<peel_project::AutosaveEntry>,
    /// `PEEL_PLUGIN` debug insert still waiting for the plugin scan.
    debug_plugin: Option<String>,
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
            recorder: Default::default(),
            recovered,
            debug_plugin: std::env::var("PEEL_PLUGIN").ok(),
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
            WelcomeAction::Template(i) => {
                let make = peel_session::template::templates()
                    .get(i)
                    .map_or(peel_session::template::beat_and_melody as fn() -> _, |t| {
                        t.1
                    });
                self.session.replace_project(make());
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
        // Tests, screenshots and other debug runs leave the user's data alone.
        if cfg!(test)
            || std::env::var_os("PEEL_NO_PREFS").is_some()
            || std::env::var_os("PEEL_SCREENSHOT").is_some()
        {
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
        self.session.capture_plugin_states();
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
        if let Some(path) = std::env::var_os("PEEL_AUDIO") {
            let lane = self.session.project().arrangement().tracks[3].id;
            if let Err(e) = self.session.add_audio_clip(
                lane,
                peel_document::Time::ZERO,
                std::path::Path::new(&path),
            ) {
                log::error!("PEEL_AUDIO: {e}");
            }
            if let Some(c) = self
                .session
                .project()
                .arrangement()
                .clips
                .iter()
                .find(|c| c.track == lane)
            {
                self.ws.focus = Some(Focus::Clip(c.id));
            }
        }
        if let Some(path) = std::env::var_os("PEEL_SAMPLE")
            && let Some(id) =
                crate::rack::add_sample_channel(&mut self.session, std::path::Path::new(&path))
        {
            self.ws.selected_channel = Some(id);
            self.ws.focus = Some(Focus::Channel(id));
            if let Some(dev) = self.session.project().channel(id).map(|c| c.device.id) {
                self.session.set_param(dev, "sample.start", 0.18, 1);
                self.session.doc.end_gesture();
            }
        }
        if std::env::var_os("PEEL_SIDECHAIN").is_some() {
            let p = self.session.project();
            let kick = p
                .channels
                .iter()
                .find(|c| c.name == "Kick")
                .map(|c| c.track);
            let synth = p
                .channels
                .iter()
                .find(|c| c.name == "Synth")
                .map(|c| c.track);
            if let (Some(kick), Some(synth)) = (kick, synth) {
                let mut comp =
                    peel_session::template::device("peel.fx.compressor", Some("Sidechain Duck"));
                comp.sidechain = Some(kick);
                comp.preset = Some("Sidechain Duck".into());
                self.session.doc.edit("Sidechain", |p| {
                    if let Some(t) = p.mixer.track_mut(synth) {
                        t.inserts.insert(0, comp);
                    }
                });
                self.ws.focus = Some(Focus::Track(synth));
                self.ws.dock = DockTab::Mixer;
            }
        }
        if std::env::var_os("PEEL_TEMPO").is_some() {
            self.ws.playlist.show_tempo = true;
            self.session.doc.edit("Tempo", |p| {
                use peel_document::Time;
                p.tempo = vec![
                    peel_time::TempoSegment {
                        start: Time::ZERO,
                        bpm: 118.0,
                        ramp_to: Some(140.0),
                    },
                    peel_time::TempoSegment {
                        start: Time::beats(16),
                        bpm: 140.0,
                        ramp_to: None,
                    },
                    peel_time::TempoSegment {
                        start: Time::beats(24),
                        bpm: 104.0,
                        ramp_to: None,
                    },
                ];
            });
        }
        if std::env::var_os("PEEL_ARM").is_some() {
            for k in [3, 4] {
                let lane = self.session.project().arrangement().tracks[k].id;
                self.session.arm_lane(lane, true);
            }
        }
        if std::env::var_os("PEEL_GRAPH").is_some() {
            self.ws.rack_graph = true;
        }
        if let Ok(name) = std::env::var("PEEL_CHANNEL")
            && let Some(c) = self
                .session
                .project()
                .channels
                .iter()
                .find(|c| c.name == name)
                .map(|c| c.id)
        {
            self.ws.selected_channel = Some(c);
            self.ws.focus = Some(Focus::Channel(c));
        }
        if std::env::var_os("PEEL_AUTOMATION").is_some() {
            use peel_document::{AutomationPoint, AutomationTarget, ClipContent, Time};
            let master = self.session.project().mixer.tracks[0].id;
            let synth = self
                .session
                .project()
                .mixer
                .tracks
                .iter()
                .find(|t| t.name == "Synth")
                .map(|t| t.id)
                .unwrap_or(master);
            let fade = crate::playlist::create_automation(
                &mut self.session,
                &mut self.ws,
                AutomationTarget::TrackGain { track: master },
                0.8,
            );
            let pan = crate::playlist::create_automation(
                &mut self.session,
                &mut self.ws,
                AutomationTarget::TrackPan { track: synth },
                0.5,
            );
            let pt = |b: i64, v: f32| AutomationPoint {
                position: Time::beats(b),
                value: v,
                tension: 0.0,
            };
            self.session.doc.edit("Shape automation", |p| {
                for c in p.arrangement_mut().clips.iter_mut() {
                    if let ClipContent::Automation { points, .. } = &mut c.content {
                        if c.id == fade {
                            *points = vec![pt(0, 0.8), pt(20, 0.8), pt(30, 0.15), pt(32, 0.0)];
                            points[1].tension = -0.6;
                        } else if c.id == pan {
                            *points = (0..=8)
                                .map(|i| pt(i * 4, if i % 2 == 0 { 0.15 } else { 0.85 }))
                                .collect();
                            for (i, x) in points.iter_mut().enumerate() {
                                x.tension = if i % 2 == 0 { 0.5 } else { -0.5 };
                            }
                        }
                    }
                }
            });
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
            let beat = self.session.position();
            self.recorder
                .note(&mut self.session, channel, key, 0.8, true, beat);
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
                let beat = self.session.position();
                self.recorder
                    .note(&mut self.session, ch, key, 0.0, false, beat);
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
        if let Some(path) = crate::dialogs::save(&name, "Peel project", &["peelproj"]) {
            match self.session.save(Some(path)) {
                Ok(p) => self.saved(&p),
                Err(e) => self.session.notify(format!("Save failed: {e}"), true),
            }
        }
    }

    pub fn open(&mut self) {
        let Some(path) = crate::dialogs::open(&[
            ("Peel project", &["peelproj"]),
            ("MIDI file", &["mid", "midi"]),
        ]) else {
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
        let Some(path) = crate::dialogs::save(&name, "WAV audio", &["wav"]) else {
            return;
        };
        let settings = ExportSettings {
            path,
            sample_rate: self.ws.export_rate,
            bits: self.ws.export_bits,
            tail_seconds: self.ws.export_tail,
        };
        self.ws.export = Some(if self.ws.export_stems {
            self.session.export_stems(settings)
        } else {
            self.session.export(settings)
        });
    }

    /// Writes the arrangement as a Standard MIDI File.
    pub fn export_midi(&mut self) {
        let name = format!("{}.mid", self.session.project().meta.title);
        let Some(path) = crate::dialogs::save(&name, "MIDI file", &["mid"]) else {
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
        if let Some(f) = self.session.render_progress() {
            let bar = row.cut_left(120.0).shrink2(0.0, 9.0);
            ui.scene.rect(bar, 3.0, t.control);
            ui.scene.rect(bar.with_w(bar.w * f), 3.0, t.accent);
            row.cut_left(8.0);
            ui.text(
                row.cut_left(80.0),
                "Rendering",
                t.caption,
                t.text_dim,
                Align::Start,
            );
        }
        if let Some(rec) = self.session.recording_status() {
            let mut cell = row.cut_left(118.0);
            let pulse = 0.6 + 0.4 * (ui.time() as f32 * 4.0).sin().abs();
            ui.scene.circle(
                peel_ui::vec2(cell.x + 5.0, cell.center().y),
                4.5,
                t.record.alpha(pulse),
            );
            cell.cut_left(16.0);
            let secs = rec.seconds as u64;
            let label = if rec.waiting_for_transport {
                "REC ready".to_string()
            } else {
                format!("REC {}:{:02}", secs / 60, secs % 60)
            };
            ui.text(cell, &label, t.caption, t.record, Align::Start);
            if let Some(e) = rec.error.as_deref() {
                let er = row.cut_left(320.0);
                let e = ui.fit_text(e, t.caption, er.w);
                ui.text(er, &e, t.caption, t.warn, Align::Start);
            }
            row.cut_left(8.0);
        }
        if let Some(job) = &self.ws.export {
            job.idle();
            if let Some(result) = job.take_result() {
                match result {
                    Ok(p) if p.is_dir() => self
                        .session
                        .notify(format!("Exported stems to {}", p.display()), false),
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
        let gear = ui.icon_button(
            W::new("settings-open"),
            row.cut_right(24.0),
            peel_ui::icons::GEAR,
            ui.is_popup_open(W::new("settings-panel")),
            None,
        );
        ui.tooltip(&gear, "Audio and MIDI");
        if gear.clicked {
            ui.open_popup(W::new("settings-panel"), gear.rect);
        }
        // MIDI activity light.
        let activity = self.session.midi_activity();
        let dt = ui.dt() as f64;
        let lit = {
            let st = ui.state::<(u64, f64)>(W::new("midi-activity"));
            if st.0 != activity {
                *st = (activity, 0.0);
            }
            st.1 += dt;
            st.1 < 0.15 && activity > 0
        };
        let dot = row.cut_right(14.0);
        let t2 = ui.theme.clone();
        ui.scene.circle(
            dot.center(),
            3.0,
            if lit { t2.play } else { t2.control_hover },
        );
        if lit {
            ui.request_repaint();
        }
        row.cut_right(4.0);
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
        // Audio health: a lost or failing device gets a visible restart.
        if !self.session.audio_is_healthy() {
            let r = row.cut_right(118.0);
            let resp = ui.interact_button(W::new("audio-restart"), r, "Restart audio");
            ui.scene.rect(
                r.shrink2(0.0, 2.0),
                t.radius_s,
                t.record.alpha(if resp.hovered { 0.3 } else { 0.18 }),
            );
            let mut inner = r.shrink2(8.0, 0.0);
            ui.icon(
                inner.cut_left(14.0),
                peel_ui::icons::LIGHTNING,
                12.0,
                t.record,
            );
            inner.cut_left(4.0);
            ui.text(inner, "Restart audio", t.caption, t.record, Align::Start);
            let why = self
                .session
                .audio_warning()
                .unwrap_or("The audio device stopped")
                .to_string();
            ui.tooltip(&resp, &why);
            if resp.clicked {
                let device = self.prefs.audio_device.clone();
                if let Err(e) = self.session.restart_audio(device, self.prefs.buffer_frames) {
                    self.session
                        .notify(format!("Could not start audio: {e}"), true);
                }
            }
            row.cut_right(8.0);
        }
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
        let frame_start = std::time::Instant::now();
        self.update_inner(ui, win);
        if std::env::var_os("PEEL_FRAME_STATS").is_some() {
            let us = frame_start.elapsed().as_micros() as u64;
            let shapes = ui.scene.shape_count();
            let st = ui.state::<(u64, u64, u64)>(W::new("frame-stats"));
            st.0 += 1;
            st.1 += us;
            st.2 = st.2.max(us);
            if st.0 == 120 {
                log::info!(
                    "UI build: avg {} us, max {} us, {} shapes",
                    st.1 / 120,
                    st.2,
                    shapes
                );
                *st = (0, 0, 0);
            }
        }
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

impl Studio {
    fn update_inner(&mut self, ui: &mut Ui, win: &WindowControl) {
        self.session.tick();
        // Input meter: peak hold with a fall of about 20 dB a second.
        let (l, r) = self.session.input_level();
        let fall = 0.1f32.powf(ui.dt().clamp(0.0, 0.1));
        let m = &mut self.ws.input_meter;
        *m = ((m.0 * fall).max(l), (m.1 * fall).max(r));
        if let Some(type_id) = &self.debug_plugin
            && let Some(name) = self
                .session
                .plugins()
                .iter()
                .find(|p| peel_session::plugin_type_id(p) == *type_id)
                .map(|p| p.name.clone())
        {
            let master = self.session.project().mixer.tracks[0].id;
            crate::mixer::add_plugin_insert(&mut self.session, master, type_id, &name);
            self.ws.focus = Some(Focus::Track(master));
            self.ws.browser_tab = crate::workspace::BrowserTab::Effects;
            self.debug_plugin = None;
        }
        self.session.set_live_target(self.ws.selected_channel);
        self.session.poll_midi();
        self.recorder.frame(&mut self.session);
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

        if std::env::var_os("PEEL_SETTINGS").is_some()
            && !ui.is_popup_open(W::new("settings-panel"))
            && ui.time() < 0.5
        {
            ui.open_popup(W::new("settings-panel"), Rect::ZERO);
        }
        crate::settings::panel(ui, &mut self.session, &mut self.prefs, self.ws.input_meter);
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
        let resp = ui.interact_button(id, cell, label);
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
