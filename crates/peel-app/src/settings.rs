//! Audio and MIDI settings panel.

use peel_session::{PortSelection, Session};
use peel_ui::{Align, Id as W, MenuItem, Rect, Ui, icons, vec2};

use crate::prefs::Prefs;

const BUFFERS: [u32; 5] = [64, 128, 256, 512, 1024];

/// Draws the panel when open. `anchor` is the button that opened it.
pub fn panel(ui: &mut Ui, s: &mut Session, prefs: &mut Prefs) {
    let id = W::new("settings-panel");
    if !ui.is_popup_open(id) {
        return;
    }
    let t = ui.theme.clone();
    let screen = ui.screen();
    let w = 380.0;
    let r = Rect {
        x: screen.right() - w - 10.0,
        y: screen.bottom() - 24.0 - 292.0,
        w,
        h: 284.0,
    };
    let devices = s.audio_devices();
    let inputs = s.midi_available_inputs();
    let mut restart: Option<(Option<String>, Option<u32>)> = None;
    let mut midi_choice: Option<PortSelection> = None;
    ui.popup_panel(id, r, |ui, r| {
        let mut body = r.shrink(18.0);
        ui.text(
            body.cut_top(22.0),
            "Audio and MIDI",
            t.heading,
            t.text,
            Align::Start,
        );
        body.cut_top(12.0);

        // Output device.
        let mut row = body.cut_top(30.0);
        ui.text(
            row.cut_left(70.0),
            "Output",
            t.caption,
            t.text_dim,
            Align::Start,
        );
        let current = s.audio_device_name().to_string();
        let dev_btn = ui.interact(W::new("settings-device"), row);
        ui.scene.rect(
            row,
            t.radius_m,
            if dev_btn.hovered {
                t.control_hover
            } else {
                t.control
            },
        );
        let mut inner = row.shrink2(10.0, 0.0);
        ui.icon(inner.cut_right(14.0), icons::CARET_DOWN, 11.0, t.text_dim);
        let name = ui.fit_text(&current, t.label, inner.w);
        ui.text(inner, &name, t.label, t.text, Align::Start);
        if dev_btn.clicked {
            ui.open_popup(W::new("settings-device-menu"), row);
        }
        body.cut_top(10.0);

        // Buffer size.
        let mut row = body.cut_top(30.0);
        ui.text(
            row.cut_left(70.0),
            "Buffer",
            t.caption,
            t.text_dim,
            Align::Start,
        );
        let labels: Vec<String> = BUFFERS.iter().map(|b| b.to_string()).collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut sel = BUFFERS
            .iter()
            .position(|b| *b == s.buffer_frames())
            .unwrap_or(2);
        if ui.segmented(W::new("settings-buffer"), row, &label_refs, &mut sel) {
            restart = Some((prefs.audio_device.clone(), Some(BUFFERS[sel])));
        }
        body.cut_top(8.0);
        let ms = s.buffer_frames() as f32 / s.sample_rate().max(1.0) * 1000.0;
        let info = format!(
            "{:.1} kHz  ·  {} frames  ·  {:.1} ms",
            s.sample_rate() / 1000.0,
            s.buffer_frames(),
            ms
        );
        let mut info_r = body.cut_top(18.0);
        info_r.cut_left(70.0);
        ui.text(info_r, &info, t.caption, t.text_faint, Align::Start);
        if let Some(w) = s.audio_warning() {
            let mut wr = body.cut_top(18.0);
            wr.cut_left(70.0);
            let w = ui.fit_text(w, t.caption, wr.w);
            ui.text(wr, &w, t.caption, t.warn, Align::Start);
        }
        body.cut_top(16.0);
        ui.scene.hline(body.x, body.right(), body.y, t.line_strong);
        body.cut_top(14.0);

        // MIDI input.
        let mut row = body.cut_top(30.0);
        ui.text(
            row.cut_left(70.0),
            "MIDI in",
            t.caption,
            t.text_dim,
            Align::Start,
        );
        let midi_label = if !s.midi_enabled() {
            "Off".to_string()
        } else {
            let connected = s.midi_inputs();
            if connected.is_empty() {
                "No devices".into()
            } else {
                connected.join(", ")
            }
        };
        let midi_btn = ui.interact(W::new("settings-midi"), row);
        ui.scene.rect(
            row,
            t.radius_m,
            if midi_btn.hovered {
                t.control_hover
            } else {
                t.control
            },
        );
        let mut inner = row.shrink2(10.0, 0.0);
        ui.icon(inner.cut_right(14.0), icons::CARET_DOWN, 11.0, t.text_dim);
        let ml = ui.fit_text(&midi_label, t.label, inner.w);
        ui.text(inner, &ml, t.label, t.text, Align::Start);
        if midi_btn.clicked {
            ui.open_popup(W::new("settings-midi-menu"), row);
        }
        body.cut_top(10.0);
        let mut hint = body.cut_top(36.0);
        hint.cut_left(70.0);
        ui.text_wrapped(
            vec2(hint.x, hint.y),
            hint.w,
            "Notes play the selected channel. The sustain pedal works on every instrument.",
            t.caption,
            t.text_faint,
        );
    });

    // Sub-menus draw on top of the panel (they replace it as the open popup,
    // so reopen the panel afterwards).
    if ui.is_popup_open(W::new("settings-device-menu")) {
        let anchor = ui.popup_rect().unwrap_or_default();
        let mut items = vec![MenuItem::new("System default").checked(prefs.audio_device.is_none())];
        items.extend(devices.iter().map(|d| {
            MenuItem::new(d.name.clone())
                .checked(prefs.audio_device.as_deref() == Some(d.name.as_str()))
        }));
        if let Some(i) = ui.menu(
            W::new("settings-device-menu"),
            vec2(anchor.x, anchor.bottom() + 4.0),
            &items,
        ) {
            let device = if i == 0 {
                None
            } else {
                Some(devices[i - 1].name.clone())
            };
            restart = Some((device, prefs.buffer_frames));
        }
        if !ui.is_popup_open(W::new("settings-device-menu")) {
            ui.open_popup(id, r);
        }
    }
    if ui.is_popup_open(W::new("settings-midi-menu")) {
        let anchor = ui.popup_rect().unwrap_or_default();
        let mut items = vec![
            MenuItem::new("All inputs"),
            MenuItem::new("Off").checked(!s.midi_enabled()),
        ];
        items.push(MenuItem::Separator);
        items.extend(inputs.iter().map(|p| MenuItem::new(p.name.clone())));
        if let Some(i) = ui.menu(
            W::new("settings-midi-menu"),
            vec2(anchor.x, anchor.bottom() + 4.0),
            &items,
        ) {
            midi_choice = Some(match i {
                0 => PortSelection::All,
                1 => PortSelection::Nothing,
                n => PortSelection::Only(inputs[n - 3].id.clone()),
            });
        }
        if !ui.is_popup_open(W::new("settings-midi-menu")) {
            ui.open_popup(id, r);
        }
    }

    if let Some((device, buffer)) = restart {
        match s.restart_audio(device.clone(), buffer) {
            Ok(()) => {
                prefs.audio_device = device;
                prefs.buffer_frames = buffer;
                prefs.save();
                let name = s.audio_device_name().to_string();
                s.notify(format!("Audio: {name}"), false);
            }
            Err(e) => s.notify(format!("Could not start audio: {e}"), true),
        }
    }
    if let Some(sel) = midi_choice {
        let off = matches!(sel, PortSelection::Nothing);
        s.set_midi_enabled(!off);
        if !off {
            s.select_midi_input(sel);
        }
    }
}
