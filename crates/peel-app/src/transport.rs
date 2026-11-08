//! Top bar: transport, tempo, position, pattern picker and master meter.

use peel_document::{Id, PALETTE, Pattern, Time};
use peel_session::PlayMode;
use peel_ui::Id as W;
use peel_ui::{Align, MenuItem, Rect, Ui, WindowControl, icons, vec2};

use crate::studio::Studio;
use crate::util::{col, format_position};

pub fn draw(ui: &mut Ui, r: Rect, win: &WindowControl, studio: &mut Studio) {
    let t = ui.theme.clone();
    // The whole bar moves the window; controls registered later take priority.
    let bar = ui.interact(W::new("titlebar"), r);
    if bar.pressed && !bar.double_clicked {
        win.drag_window();
    }
    if bar.double_clicked {
        win.toggle_maximized();
    }

    let mut row = r.shrink2(12.0, 9.0);
    // Narrow windows trade labels and width for room.
    let compact = r.w < 1560.0;
    let tight = r.w < 1340.0;
    row.cut_left(win.titlebar_inset());
    logo(ui, row.cut_left(30.0));
    row.cut_left(10.0);
    if !tight {
        let max_title = if compact { 110.0 } else { 180.0 };
        let title = studio.session.project().meta.title.clone();
        let tw = ui.measure(&title, t.label).x.min(max_title);
        let title_r = row.cut_left(tw + 4.0);
        let fitted = ui.fit_text(&title, t.label, max_title);
        ui.text(title_r, &fitted, t.label, t.text_dim, Align::Start);
    }
    row.cut_left(if compact { 12.0 } else { 18.0 });

    // Pattern / Song.
    let mut mode_idx = if studio.session.mode() == PlayMode::Song {
        1
    } else {
        0
    };
    if ui.segmented(
        W::new("mode"),
        row.cut_left(if compact { 136.0 } else { 150.0 }),
        &["Pattern", "Song"],
        &mut mode_idx,
    ) {
        let mode = if mode_idx == 1 {
            PlayMode::Song
        } else {
            PlayMode::Pattern(studio.current_pattern())
        };
        studio.session.set_mode(mode);
    }
    row.cut_left(10.0);

    // Play / stop / record.
    let playing = studio.session.is_playing();
    let play = ui.icon_button_fill(
        W::new("play"),
        row.cut_left(38.0),
        if playing { icons::PAUSE } else { icons::PLAY },
        playing,
        t.play,
        17.0,
    );
    ui.tooltip(&play, "Play (Space)");
    if play.clicked {
        studio.session.toggle_play();
    }
    row.cut_left(4.0);
    let stop = ui.icon_button_fill(
        W::new("stop"),
        row.cut_left(38.0),
        icons::STOP,
        false,
        t.text,
        15.0,
    );
    ui.tooltip(&stop, "Stop and return");
    if stop.clicked {
        if playing {
            studio.session.stop();
        } else {
            studio.session.seek(0.0);
        }
    }
    row.cut_left(4.0);
    let armed = studio.recorder.armed;
    let rec = ui.icon_button_fill(
        W::new("rec"),
        row.cut_left(38.0),
        icons::RECORD,
        armed,
        t.record,
        15.0,
    );
    let song = studio.session.mode() == peel_session::PlayMode::Song;
    ui.tooltip(
        &rec,
        match (armed, song) {
            (true, true) => "Recording audio onto armed lanes",
            (false, true) => "Record audio onto armed lanes",
            (true, false) => "Recording notes into the pattern",
            (false, false) => "Record notes into the pattern",
        },
    );
    if rec.clicked {
        studio.recorder.armed = !armed;
    }
    row.cut_left(12.0);

    // Position readout.
    let pos_r = row.cut_left(if tight {
        84.0
    } else if compact {
        124.0
    } else {
        132.0
    });
    ui.scene.rect(pos_r, t.radius_m, t.panel);
    let beat = studio.session.position();
    let sig = studio.session.project().time_signature;
    let pos = format_position(beat, sig);
    let mut pr = pos_r.shrink2(12.0, 0.0);
    let secs = studio
        .session
        .project()
        .tempo
        .first()
        .map_or(0.0, |s| beat * 60.0 / s.bpm);
    let time = format!("{}:{:04.1}", (secs / 60.0) as i64, secs % 60.0);
    ui.text(
        pr.cut_left(70.0),
        &pos,
        t.readout.size(18.0),
        t.text,
        Align::Start,
    );
    if !tight {
        ui.text(pr, &time, t.readout_small, t.text_faint, Align::End);
    }
    row.cut_left(8.0);

    // Tempo.
    let bpm_r = row.cut_left(96.0);
    ui.scene.rect(bpm_r, t.radius_m, t.panel);
    // With tempo changes, the tempo at the playhead: live while playing
    // (ramps included), the change in force when stopped. Edits change
    // that tempo change.
    let segs = studio.session.project().tempo.clone();
    let pos = studio.session.position().max(0.0);
    let active = segs
        .iter()
        .rposition(|x| x.start.as_f64() <= pos + 1e-9)
        .unwrap_or(0);
    let mut bpm = match segs.len() {
        0 | 1 => studio.session.project().bpm(),
        _ if studio.session.is_playing() => peel_time::TempoMap::from_segments(&segs).bpm_at(pos),
        _ => segs[active].bpm,
    };
    let mut br = bpm_r;
    let label = br.cut_right(34.0);
    let resp = ui.drag_number(
        W::new("bpm"),
        br,
        &mut bpm,
        20.0,
        400.0,
        0.2,
        t.readout.size(18.0),
        t.accent,
        |v| format!("{v:.1}"),
    );
    // Tap tempo: click the BPM label in time with the music.
    let tap = ui.interact(W::new("tap-tempo"), label);
    let tapped = ui.animate(W::new("tap-flash"), 0.0, 8.0);
    ui.text(
        label,
        "BPM",
        t.caption,
        t.text_faint.mix(t.accent, tapped),
        Align::Start,
    );
    ui.tooltip(&tap, "Tap to set the tempo");
    if tap.hovered {
        ui.set_cursor(peel_ui::CursorIcon::Pointer);
    }
    if tap.pressed {
        let now = ui.time();
        ui.animate(W::new("tap-flash"), 1.0, 1000.0);
        let taps = ui.state::<Vec<f64>>(W::new("tap-times"));
        if taps.last().is_some_and(|last| now - last > 2.0) {
            taps.clear();
        }
        taps.push(now);
        if taps.len() > 8 {
            taps.remove(0);
        }
        if taps.len() >= 3 {
            let span = taps[taps.len() - 1] - taps[0];
            let bpm = 60.0 * (taps.len() - 1) as f64 / span.max(0.01);
            if (30.0..=300.0).contains(&bpm) {
                let bpm = (bpm * 10.0).round() / 10.0;
                studio
                    .session
                    .doc
                    .edit_gesture(W::new("tap-gesture").0, "Tap tempo", |p| p.set_bpm(bpm));
            }
        }
    }
    if resp.changed {
        studio
            .session
            .doc
            .edit_gesture(W::new("bpm-gesture").0, "Change tempo", |p| {
                let v = ((bpm * 10.0).round() / 10.0).clamp(10.0, 999.0);
                if p.tempo.len() > 1 {
                    crate::playlist::tidy_tempo(&mut p.tempo);
                    if let Some(x) = p.tempo.get_mut(active) {
                        x.bpm = v;
                    }
                    crate::playlist::tidy_tempo(&mut p.tempo);
                } else {
                    p.set_bpm(v);
                }
            });
    }
    if resp.released {
        studio.session.doc.end_gesture();
    }
    ui.tooltip(&resp, "Drag or double-click to type");
    row.cut_left(8.0);

    // Time signature.
    let sig_r = row.cut_left(48.0);
    ui.scene.rect(sig_r, t.radius_m, t.panel);
    let sig_txt = format!("{}/{}", sig.numerator, sig.denominator);
    let sresp = ui.interact(W::new("sig"), sig_r);
    ui.text(
        sig_r,
        &sig_txt,
        t.readout_small.size(14.0),
        t.text_dim,
        Align::Center,
    );
    if sresp.clicked {
        ui.open_popup(W::new("sig-menu"), sig_r);
    }
    let sigs = [(4u8, 4u8), (3, 4), (6, 8), (5, 4), (7, 8), (2, 4)];
    let items: Vec<MenuItem> = sigs
        .iter()
        .map(|(n, d)| {
            MenuItem::new(format!("{n}/{d}")).checked(sig.numerator == *n && sig.denominator == *d)
        })
        .collect();
    if let Some(i) = ui.menu(
        W::new("sig-menu"),
        vec2(sig_r.x, sig_r.bottom() + 4.0),
        &items,
    ) {
        let (n, d) = sigs[i];
        studio.session.doc.edit("Change time signature", |p| {
            p.time_signature = peel_document::TimeSignature {
                numerator: n,
                denominator: d,
            };
        });
    }
    row.cut_left(6.0);

    let metro_on = studio.session.metronome();
    let m = ui.icon_button(
        W::new("metronome"),
        row.cut_left(34.0),
        icons::METRONOME,
        metro_on,
        None,
    );
    ui.tooltip(&m, "Metronome");
    if m.clicked {
        studio.session.set_metronome(!metro_on);
    }
    row.cut_left(14.0);

    pattern_picker(
        ui,
        &mut row,
        studio,
        if tight {
            120.0
        } else if compact {
            150.0
        } else {
            190.0
        },
    );

    // Right side: export, save, undo/redo, load, master meter.
    let mut right = row;
    let export = ui.interact(
        W::new("export"),
        right.cut_right(if tight { 40.0 } else { 96.0 }),
    );
    {
        let er = export.rect;
        let hot = ui.animate_bool(W::new("export-hot"), export.hovered);
        ui.scene.rect(er, t.radius_m, t.accent.lighten(hot * 0.08));
        if tight {
            ui.icon(er, icons::EXPORT, 16.0, t.text_on_accent);
        } else {
            let mut inner = er.shrink2(12.0, 0.0);
            ui.icon(inner.cut_left(16.0), icons::EXPORT, 15.0, t.text_on_accent);
            ui.text(inner, "Export", t.label, t.text_on_accent, Align::Center);
        }
        if export.hovered {
            ui.set_cursor(peel_ui::CursorIcon::Pointer);
        }
    }
    ui.tooltip(&export, "Render the arrangement to WAV (Cmd/Ctrl+E)");
    if export.clicked {
        studio.export();
    }
    export_panel(ui, export.rect, studio);
    right.cut_right(6.0);
    let save = ui.icon_button(
        W::new("save"),
        right.cut_right(34.0),
        icons::SAVE,
        false,
        None,
    );
    ui.tooltip(&save, "Save (Cmd/Ctrl+S)");
    if save.clicked {
        studio.save();
    }
    let open = ui.icon_button(
        W::new("open"),
        right.cut_right(34.0),
        icons::FOLDER_OPEN,
        false,
        None,
    );
    ui.tooltip(&open, "Open project or MIDI file");
    if open.clicked {
        studio.open();
    }
    right.cut_right(4.0);
    let redo = ui.icon_button(
        W::new("redo"),
        right.cut_right(34.0),
        icons::REDO,
        false,
        None,
    );
    let redo_label = studio
        .session
        .doc
        .redo_label()
        .map(|l| format!("Redo {l}"))
        .unwrap_or_else(|| "Nothing to redo".into());
    ui.tooltip(&redo, &redo_label);
    if redo.clicked {
        studio.session.doc.redo();
    }
    let undo = ui.icon_button(
        W::new("undo"),
        right.cut_right(34.0),
        icons::UNDO,
        false,
        None,
    );
    let undo_label = studio
        .session
        .doc
        .undo_label()
        .map(|l| format!("Undo {l}"))
        .unwrap_or_else(|| "Nothing to undo".into());
    ui.tooltip(&undo, &format!("{undo_label}  ·  right-click for history"));
    if undo.clicked {
        studio.session.doc.undo();
    }
    if undo.secondary_clicked {
        ui.open_popup(W::new("history"), undo.rect);
    }
    history_panel(ui, undo.rect, studio);
    right.cut_right(14.0);

    // Master meter + load.
    if tight {
        return;
    }
    let meter_r = right
        .cut_right(if compact { 90.0 } else { 130.0 })
        .shrink2(0.0, 7.0);
    let master = studio.session.project().mixer.tracks[0].id;
    let peaks = studio.session.take_track_meter(master);
    horizontal_meter(ui, W::new("master-meter"), meter_r, peaks);
    right.cut_right(12.0);
    let load = studio.session.cpu_load();
    let load_r = right.cut_right(if compact { 18.0 } else { 54.0 });
    let lc = if load > 0.85 {
        t.record
    } else if load > 0.6 {
        t.warn
    } else {
        t.text_faint
    };
    let lresp = ui.interact(W::new("load"), load_r);
    ui.tooltip(
        &lresp,
        &format!("Audio processing load {:.0}%", load * 100.0),
    );
    let mut lr = load_r;
    ui.icon(lr.cut_left(16.0), icons::CPU, 14.0, lc);
    if !compact {
        ui.text(
            lr,
            &format!("{:.0}%", load * 100.0),
            t.readout_small,
            lc,
            Align::End,
        );
    }
}

fn logo(ui: &mut Ui, r: Rect) {
    // A cut orange with a curl of peel lifting away.
    let t = ui.theme.clone();
    let c = r.center();
    ui.scene.circle(c, 12.0, t.accent);
    ui.scene.circle(c, 9.2, t.accent.darken(0.18));
    for k in 0..4 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let d = vec2(a.sin(), -a.cos()) * 8.6;
        ui.scene.line(c - d, c + d, 1.2, t.accent);
    }
    ui.scene.arc(c, 12.0, 2.8, 0.35, 1.25, t.bg);
    ui.scene.arc(
        c + vec2(9.4, -8.8),
        3.8,
        2.0,
        3.5,
        2.5,
        t.accent.lighten(0.15),
    );
}

fn horizontal_meter(ui: &mut Ui, id: W, r: Rect, peaks: (f32, f32)) {
    // Reuse the vertical meter ballistics by drawing two thin horizontal bars.
    let t = ui.theme.clone();
    let st = ui.state::<[f32; 2]>(id);
    let dt = 1.0 / 60.0;
    for (i, p) in [peaks.0, peaks.1].into_iter().enumerate() {
        let v = peel_ui::meter_pos(p);
        st[i] = if v >= st[i] {
            v
        } else {
            (st[i] - dt * 0.35).max(v)
        };
    }
    let levels = *st;
    if levels[0] > 0.0 || levels[1] > 0.0 {
        ui.request_repaint();
    }
    let rows = r.rows(2, 3.0);
    for (i, row) in rows.iter().enumerate() {
        ui.scene.rect(*row, 2.0, t.panel);
        let w = row.w * levels[i];
        if w > 0.5 {
            let fill = row.with_w(w);
            let amber = peel_ui::meter_pos(0.5);
            let red = peel_ui::meter_pos(1.0);
            ui.scene.push_clip(fill);
            ui.scene.rect(
                Rect {
                    w: row.w * amber,
                    ..*row
                },
                2.0,
                t.play,
            );
            ui.scene.rect(
                Rect {
                    x: row.x + row.w * amber,
                    w: row.w * (red - amber),
                    ..*row
                },
                0.0,
                t.accent,
            );
            ui.scene.rect(
                Rect {
                    x: row.x + row.w * red,
                    w: row.w * (1.0 - red),
                    ..*row
                },
                2.0,
                t.record,
            );
            ui.scene.pop_clip();
        }
    }
    // 0 dB tick.
    let x = r.x + r.w * peel_ui::meter_pos(1.0);
    ui.scene
        .vline(x, r.y - 2.0, r.bottom() + 2.0, t.text_faint.alpha(0.6));
}

fn pattern_picker(ui: &mut Ui, row: &mut Rect, studio: &mut Studio, width: f32) {
    let t = ui.theme.clone();
    let pid = studio.current_pattern();
    let Some(pat) = studio.session.project().pattern(pid).cloned() else {
        return;
    };
    let r = row.cut_left(width);
    let id = W::new("pattern-picker");
    let resp = ui.interact(id, r);
    let hot = ui.animate_bool(id, resp.hovered);
    ui.scene.rect(r, t.radius_m, t.panel.mix(t.raised, hot));
    let mut inner = r.shrink2(10.0, 0.0);
    let sw = inner.cut_left(12.0);
    ui.scene.rect(
        Rect::from_center(sw.center(), 12.0, 12.0),
        3.0,
        col(pat.color),
    );
    inner.cut_left(8.0);
    let caret = inner.cut_right(16.0);
    ui.icon(caret, icons::CARET_DOWN, 13.0, t.text_dim);
    let name = ui.fit_text(&pat.name, t.label, inner.w);
    ui.text(inner, &name, t.label, t.text, Align::Start);
    ui.tooltip(&resp, "Pattern being edited");
    if resp.clicked {
        ui.open_popup(W::new("pattern-menu"), r);
    }
    if resp.hovered {
        ui.set_cursor(peel_ui::CursorIcon::Pointer);
    }
    let patterns: Vec<(Id, String, peel_document::Color)> = studio
        .session
        .project()
        .patterns
        .iter()
        .map(|p| (p.id, p.name.clone(), p.color))
        .collect();
    let mut items: Vec<MenuItem> = patterns
        .iter()
        .map(|(id, name, c)| {
            MenuItem::new(name.clone())
                .swatch(col(*c))
                .checked(*id == pid)
        })
        .collect();
    items.push(MenuItem::Separator);
    items.push(MenuItem::new("New pattern"));
    items.push(MenuItem::new("Duplicate pattern"));
    items.push(MenuItem::new("Rename pattern"));
    items.push(MenuItem::new("Delete pattern").enabled(patterns.len() > 1));
    if let Some(i) = ui.menu(W::new("pattern-menu"), vec2(r.x, r.bottom() + 4.0), &items) {
        let n = patterns.len();
        if i < n {
            studio.session.select_pattern(patterns[i].0);
        } else if i == n + 1 {
            let id = new_pattern(studio, None);
            studio.session.select_pattern(id);
        } else if i == n + 2 {
            let id = new_pattern(studio, Some(&pat));
            studio.session.select_pattern(id);
        } else if i == n + 3 {
            crate::rename::open(ui, W::new("rename-pattern"), r, &pat.name);
        } else if i == n + 4 {
            delete_pattern(studio, pid);
        }
    }
    if let Some(name) = crate::rename::show(ui, W::new("rename-pattern")) {
        studio.session.doc.edit("Rename pattern", |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                pt.name = name;
            }
        });
    }
    row.cut_left(8.0);
    let add = ui.icon_button(
        W::new("pattern-add"),
        row.cut_left(30.0),
        icons::PLUS,
        false,
        None,
    );
    ui.tooltip(&add, "New pattern");
    if add.clicked {
        let id = new_pattern(studio, None);
        studio.session.select_pattern(id);
    }
}

/// Creates a new pattern (optionally duplicating `from`). Returns its id.
pub fn new_pattern(studio: &mut Studio, from: Option<&Pattern>) -> Id {
    let n = studio.session.project().patterns.len();
    let id = Id::new();
    let pattern = match from {
        Some(src) => {
            let mut p = src.clone();
            p.id = id;
            p.name = format!("{} copy", src.name);
            for note in &mut p.notes {
                note.id = Id::new();
            }
            p
        }
        None => Pattern {
            id,
            name: format!("Pattern {}", n + 1),
            color: PALETTE[(n * 5 + 3) % PALETTE.len()],
            length: Time::beats(4),
            notes: vec![],
        },
    };
    let label = if from.is_some() {
        "Duplicate pattern"
    } else {
        "New pattern"
    };
    studio
        .session
        .doc
        .edit(label, |p| p.patterns.push(std::sync::Arc::new(pattern)));
    id
}

fn delete_pattern(studio: &mut Studio, pid: Id) {
    studio.session.doc.edit("Delete pattern", |p| {
        p.patterns.retain(|x| x.id != pid);
        for a in p.arrangements.iter_mut() {
            let a = std::sync::Arc::make_mut(a);
            a.clips.retain(|c| !matches!(c.content, peel_document::ClipContent::Pattern { pattern } if pattern == pid));
        }
    });
    let first = studio.session.project().patterns.first().map(|p| p.id);
    if let Some(f) = first {
        studio.session.select_pattern(f);
    }
}

fn export_panel(ui: &mut Ui, anchor: Rect, studio: &mut Studio) {
    let id = W::new("export-panel");
    if studio.ws.export_panel {
        studio.ws.export_panel = false;
        ui.open_popup(id, anchor);
    }
    if !ui.is_popup_open(id) {
        return;
    }
    let t = ui.theme.clone();
    let screen = ui.screen();
    let w = 300.0;
    let r = Rect {
        x: (anchor.right() - w).min(screen.right() - w - 8.0),
        y: anchor.bottom() + 6.0,
        w,
        h: 250.0,
    };
    let mut action = 0;
    ui.popup_panel(id, r, |ui, r| {
        let mut body = r.shrink(16.0);
        ui.text(
            body.cut_top(22.0),
            "Export",
            t.heading,
            t.text,
            Align::Start,
        );
        body.cut_top(10.0);
        let row = |body: &mut Rect| {
            let mut r = body.cut_top(28.0);
            body.cut_top(8.0);
            let label = r.cut_left(64.0);
            (label, r)
        };
        let (l, c) = row(&mut body);
        ui.text(l, "Format", t.caption, t.text_dim, Align::Start);
        let bits = [16u16, 24, 32];
        let mut i = bits
            .iter()
            .position(|b| *b == studio.ws.export_bits)
            .unwrap_or(1);
        if ui.segmented(
            W::new("export-bits"),
            c,
            &["16-bit", "24-bit", "32 float"],
            &mut i,
        ) {
            studio.ws.export_bits = bits[i];
        }
        let (l, c) = row(&mut body);
        ui.text(l, "Rate", t.caption, t.text_dim, Align::Start);
        let rates = [44_100u32, 48_000, 96_000];
        let mut i = rates
            .iter()
            .position(|x| *x == studio.ws.export_rate)
            .unwrap_or(1);
        if ui.segmented(
            W::new("export-rate"),
            c,
            &["44.1 kHz", "48 kHz", "96 kHz"],
            &mut i,
        ) {
            studio.ws.export_rate = rates[i];
        }
        let (l, c) = row(&mut body);
        ui.text(l, "Tail", t.caption, t.text_dim, Align::Start);
        let tails = [0.0f32, 3.0, 8.0];
        let mut i = tails
            .iter()
            .position(|x| *x == studio.ws.export_tail)
            .unwrap_or(1);
        if ui.segmented(W::new("export-tail"), c, &["None", "3 s", "8 s"], &mut i) {
            studio.ws.export_tail = tails[i];
        }
        let (l, c) = row(&mut body);
        ui.text(l, "Files", t.caption, t.text_dim, Align::Start);
        let mut i = usize::from(studio.ws.export_stems);
        if ui.segmented(W::new("export-stems"), c, &["Mix", "Stems"], &mut i) {
            studio.ws.export_stems = i == 1;
        }
        body.cut_top(6.0);
        let mut buttons = body.cut_top(34.0);
        let midi = buttons.cut_left(100.0);
        buttons.cut_left(8.0);
        let label = if studio.ws.export_stems {
            "Export stems"
        } else {
            "Export WAV"
        };
        if ui
            .button_primary(W::new("export-audio"), buttons, label)
            .clicked
        {
            action = 1;
        }
        if ui.button(W::new("export-midi"), midi, "MIDI").clicked {
            action = 2;
        }
    });
    match action {
        1 => {
            ui.close_popup();
            studio.export_audio();
        }
        2 => {
            ui.close_popup();
            studio.export_midi();
        }
        _ => {}
    }
}

/// Undo history: every step in musical terms; click one to return to it.
fn history_panel(ui: &mut Ui, anchor: Rect, studio: &mut Studio) {
    let id = W::new("history");
    if !ui.is_popup_open(id) {
        return;
    }
    let t = ui.theme.clone();
    let undo: Vec<String> = studio
        .session
        .doc
        .history_labels()
        .map(str::to_string)
        .collect();
    let redo: Vec<String> = studio
        .session
        .doc
        .redo_labels()
        .map(str::to_string)
        .collect();
    let row_h = 26.0;
    let shown_undo = undo.len().min(16);
    let shown_redo = redo.len().min(6);
    let h = 52.0 + (shown_undo + shown_redo + 1) as f32 * row_h;
    let r = Rect {
        x: (anchor.right() - 280.0).max(8.0),
        y: anchor.bottom() + 6.0,
        w: 280.0,
        h,
    };
    let mut jump: Option<i64> = None;
    ui.popup_panel(id, r, |ui, r| {
        let mut body = r.shrink(12.0);
        ui.text(
            body.cut_top(24.0),
            "History",
            t.heading,
            t.text,
            Align::Start,
        );
        body.cut_top(6.0);
        // Redo entries (greyed), newest at the top, then the current state,
        // then undo entries from most recent.
        for (i, label) in redo.iter().rev().take(shown_redo).enumerate().rev() {
            let row = body.cut_top(row_h);
            let resp = ui.interact(W::new(("history-redo", i)), row);
            if resp.hovered {
                ui.scene.rect(row, t.radius_s, t.control_hover);
            }
            ui.text(
                row.shrink2(10.0, 0.0),
                label,
                t.body,
                t.text_faint,
                Align::Start,
            );
            if resp.clicked {
                jump = Some(-(redo.len() as i64 - (redo.len() - 1 - i) as i64));
            }
        }
        let now = body.cut_top(row_h);
        ui.scene.rect(now, t.radius_s, t.accent.alpha(0.14));
        ui.text(
            now.shrink2(10.0, 0.0),
            "Now",
            t.label,
            t.accent,
            Align::Start,
        );
        for (i, label) in undo.iter().rev().take(shown_undo).enumerate() {
            let row = body.cut_top(row_h);
            let resp = ui.interact(W::new(("history-undo", i)), row);
            if resp.hovered {
                ui.scene.rect(row, t.radius_s, t.control_hover);
            }
            ui.text(row.shrink2(10.0, 0.0), label, t.body, t.text, Align::Start);
            if resp.clicked {
                jump = Some(i as i64 + 1);
            }
        }
    });
    if let Some(steps) = jump {
        // Positive: undo that many steps; negative: redo.
        if steps > 0 {
            for _ in 0..steps {
                studio.session.doc.undo();
            }
        } else {
            for _ in 0..(-steps) {
                studio.session.doc.redo();
            }
        }
        ui.close_popup();
    }
}
