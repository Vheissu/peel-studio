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
    row.cut_left(win.titlebar_inset());
    logo(ui, row.cut_left(30.0));
    row.cut_left(10.0);
    let title = studio.session.project().meta.title.clone();
    let tw = ui.measure(&title, t.label).x.min(180.0);
    let title_r = row.cut_left(tw + 4.0);
    let fitted = ui.fit_text(&title, t.label, 180.0);
    ui.text(title_r, &fitted, t.label, t.text_dim, Align::Start);
    row.cut_left(18.0);

    // Pattern / Song.
    let mut mode_idx = if studio.session.mode() == PlayMode::Song {
        1
    } else {
        0
    };
    if ui.segmented(
        W::new("mode"),
        row.cut_left(150.0),
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
    let rec = ui.icon_button_fill(
        W::new("rec"),
        row.cut_left(38.0),
        icons::RECORD,
        false,
        t.record,
        15.0,
    );
    ui.tooltip(&rec, "Recording arrives with audio input support");
    row.cut_left(12.0);

    // Position readout.
    let pos_r = row.cut_left(132.0);
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
    ui.text(pr, &time, t.readout_small, t.text_faint, Align::End);
    row.cut_left(8.0);

    // Tempo.
    let bpm_r = row.cut_left(96.0);
    ui.scene.rect(bpm_r, t.radius_m, t.panel);
    let mut bpm = studio.session.project().bpm();
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
    ui.text(label, "BPM", t.caption, t.text_faint, Align::Start);
    if resp.changed {
        studio
            .session
            .doc
            .edit_gesture(W::new("bpm-gesture").0, "Change tempo", |p| {
                p.set_bpm((bpm * 10.0).round() / 10.0)
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

    pattern_picker(ui, &mut row, studio);

    // Right side: export, save, undo/redo, load, master meter.
    let mut right = row;
    let export = ui.interact(W::new("export"), right.cut_right(96.0));
    {
        let er = export.rect;
        let hot = ui.animate_bool(W::new("export-hot"), export.hovered);
        ui.scene.rect(er, t.radius_m, t.accent.lighten(hot * 0.08));
        let mut inner = er.shrink2(12.0, 0.0);
        ui.icon(inner.cut_left(16.0), icons::EXPORT, 15.0, t.text_on_accent);
        ui.text(inner, "Export", t.label, t.text_on_accent, Align::Center);
        if export.hovered {
            ui.set_cursor(peel_ui::CursorIcon::Pointer);
        }
    }
    ui.tooltip(&export, "Render the arrangement to WAV (Cmd/Ctrl+E)");
    if export.clicked {
        studio.export();
    }
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
    ui.tooltip(&undo, &undo_label);
    if undo.clicked {
        studio.session.doc.undo();
    }
    right.cut_right(14.0);

    // Master meter + load.
    let meter_r = right.cut_right(130.0).shrink2(0.0, 7.0);
    let master = studio.session.project().mixer.tracks[0].id;
    let peaks = studio.session.take_track_meter(master);
    horizontal_meter(ui, W::new("master-meter"), meter_r, peaks);
    right.cut_right(12.0);
    let load = studio.session.cpu_load();
    let load_r = right.cut_right(54.0);
    let lc = if load > 0.85 {
        t.record
    } else if load > 0.6 {
        t.warn
    } else {
        t.text_faint
    };
    let lresp = ui.interact(W::new("load"), load_r);
    ui.tooltip(&lresp, "Audio processing load");
    let mut lr = load_r;
    ui.icon(lr.cut_left(16.0), icons::CPU, 14.0, lc);
    ui.text(
        lr,
        &format!("{:.0}%", load * 100.0),
        t.readout_small,
        lc,
        Align::End,
    );
}

fn logo(ui: &mut Ui, r: Rect) {
    // A citrus disc with a curl of peel lifting away.
    let t = ui.theme.clone();
    let c = r.center();
    ui.scene.circle(c, 12.0, t.accent);
    ui.scene.circle(c, 8.5, t.accent.darken(0.18));
    ui.scene.arc(c, 12.0, 3.0, 0.35, 1.25, t.bg);
    ui.scene.arc(
        c + vec2(9.0, -8.0),
        4.0,
        2.2,
        3.4,
        2.6,
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

fn pattern_picker(ui: &mut Ui, row: &mut Rect, studio: &mut Studio) {
    let t = ui.theme.clone();
    let pid = studio.current_pattern();
    let Some(pat) = studio.session.project().pattern(pid).cloned() else {
        return;
    };
    let r = row.cut_left(190.0);
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
            delete_pattern(studio, pid);
        }
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
