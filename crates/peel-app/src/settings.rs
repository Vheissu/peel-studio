//! Audio and MIDI settings panel.

use peel_session::{InputChannels, InputDeviceInfo, OutputDeviceInfo, PortSelection, Session};
use peel_ui::{Align, Id as W, MenuItem, Rect, Ui, icons, vec2};

use crate::prefs::Prefs;

const BUFFERS: [u32; 5] = [64, 128, 256, 512, 1024];

/// Device lists, refreshed when a device menu opens (listing devices can
/// take milliseconds, too slow for every frame).
#[derive(Default)]
struct Devices {
    outputs: Vec<OutputDeviceInfo>,
    inputs: Vec<InputDeviceInfo>,
}

/// Label for a channel selection (device channels are zero-based).
fn channels_label(c: InputChannels) -> String {
    match c {
        InputChannels::Mono(a) => format!("Input {}", a + 1),
        InputChannels::Stereo(a, b) => format!("Inputs {} + {}", a + 1, b + 1),
    }
}

/// Every mono channel, then adjacent stereo pairs.
fn channel_choices(count: u16) -> Vec<InputChannels> {
    let mut v: Vec<InputChannels> = (0..count.max(1)).map(InputChannels::Mono).collect();
    v.extend((0..count / 2).map(|k| InputChannels::Stereo(k * 2, k * 2 + 1)));
    v
}

/// A dropdown-looking button.
fn dropdown(ui: &mut Ui, id: W, row: Rect, label: &str) -> bool {
    let t = ui.theme.clone();
    let b = ui.interact_button(id, row, label);
    ui.scene.rect(
        row,
        t.radius_m,
        if b.hovered {
            t.control_hover
        } else {
            t.control
        },
    );
    let mut inner = row.shrink2(10.0, 0.0);
    ui.icon(inner.cut_right(14.0), icons::CARET_DOWN, 11.0, t.text_dim);
    let shown = ui.fit_text(label, t.label, inner.w);
    ui.text(inner, &shown, t.label, t.text, Align::Start);
    b.clicked
}

/// Draws the panel when open. `meter` is the input peak (left, right).
pub fn panel(ui: &mut Ui, s: &mut Session, prefs: &mut Prefs, meter: (f32, f32)) {
    let id = W::new("settings-panel");
    if !ui.is_popup_open(id) {
        return;
    }
    let t = ui.theme.clone();
    let screen = ui.screen();
    let w = 380.0;
    let r = Rect {
        x: screen.right() - w - 10.0,
        y: 0.0,
        w,
        h: 0.0,
    };
    // Height follows the warnings that are showing.
    let extra = if s.input_warning().is_some() {
        36.0
    } else {
        0.0
    } + if s.audio_warning().is_some() {
        18.0
    } else {
        0.0
    };
    let r = Rect {
        h: 400.0 + extra,
        y: screen.bottom() - 32.0 - 400.0 - extra,
        ..r
    };
    let cache = W::new("settings-devices");
    let devices = std::mem::take(&mut ui.state::<Devices>(cache).outputs);
    let input_devices = std::mem::take(&mut ui.state::<Devices>(cache).inputs);
    let mut refresh_outputs = false;
    let mut refresh_inputs = false;
    let mut input_choice: Option<Option<Option<String>>> = None;
    let mut channel_choice: Option<InputChannels> = None;
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
            refresh_outputs = true;
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

        // Audio input.
        let mut row = body.cut_top(30.0);
        ui.text(
            row.cut_left(70.0),
            "Input",
            t.caption,
            t.text_dim,
            Align::Start,
        );
        let open = s.input_is_open();
        let input_label = match (open, s.input_info()) {
            (true, Some(info)) => info.device_name,
            _ => match s.requested_input_device() {
                Some(d) => format!("{d} (closed)"),
                None => "Closed".into(),
            },
        };
        if dropdown(ui, W::new("settings-input"), row, &input_label) {
            refresh_inputs = true;
            ui.open_popup(W::new("settings-input-menu"), row);
        }
        body.cut_top(10.0);
        let mut row = body.cut_top(30.0);
        ui.text(
            row.cut_left(70.0),
            "Channels",
            t.caption,
            t.text_dim,
            Align::Start,
        );
        let mut chan_r = row.cut_left(row.w * 0.5);
        chan_r.w -= 8.0;
        if dropdown(
            ui,
            W::new("settings-channels"),
            chan_r,
            &channels_label(s.input_channels()),
        ) {
            ui.open_popup(W::new("settings-channels-menu"), chan_r);
        }
        // Level meter (left over right).
        let m = row.shrink2(0.0, 7.0);
        ui.scene.rect(m, 3.0, t.bg);
        for (k, v) in [meter.0, meter.1].into_iter().enumerate() {
            let lane = Rect {
                y: m.y + 2.0 + k as f32 * (m.h - 4.0) / 2.0,
                h: (m.h - 4.0) / 2.0 - 1.0,
                x: m.x + 2.0,
                w: m.w - 4.0,
            };
            let level = if open {
                (peel_dsp::util::gain_to_db(v.max(1e-6)) + 60.0) / 60.0
            } else {
                0.0
            };
            let c = if v >= 1.0 {
                t.record
            } else if v > 0.5 {
                t.warn
            } else {
                t.play
            };
            ui.scene
                .rect(lane.with_w(lane.w * level.clamp(0.0, 1.0)), 1.5, c);
        }
        body.cut_top(10.0);
        let mut row = body.cut_top(30.0);
        ui.text(
            row.cut_left(70.0),
            "Monitor",
            t.caption,
            t.text_dim,
            Align::Start,
        );
        let mon = s.monitor_status();
        let led_r = row.cut_left(30.0);
        let led = ui.led(
            W::new("settings-monitor"),
            Rect::from_center(led_r.center(), 20.0, 20.0),
            mon.on,
            t.accent,
        );
        ui.name(&led, "Monitor input");
        ui.tooltip(
            &led,
            mon.unavailable_reason
                .as_deref()
                .unwrap_or("Hear the input through the output. Use headphones."),
        );
        if led.clicked {
            s.set_monitoring(!mon.on, mon.gain);
        }
        row.cut_left(6.0);
        let mon_text = if mon.on && mon.available {
            format!("{:.0} ms through", mon.latency_ms)
        } else if mon.on {
            "Waiting for the input".into()
        } else {
            "Off".into()
        };
        ui.text(row, &mon_text, t.caption, t.text_faint, Align::Start);
        if let Some(w) = s.input_warning() {
            body.cut_top(4.0);
            let mut wr = body.cut_top(32.0);
            wr.cut_left(70.0);
            ui.text_wrapped(vec2(wr.x, wr.y), wr.w, &w, t.caption, t.warn);
        }
        body.cut_top(14.0);
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
    if ui.is_popup_open(W::new("settings-input-menu")) {
        let anchor = ui.popup_rect().unwrap_or_default();
        let open = s.input_is_open();
        let mut items = vec![
            MenuItem::new("Closed").checked(!open),
            MenuItem::new("System default").checked(open && s.requested_input_device().is_none()),
            MenuItem::Separator,
        ];
        items.extend(input_devices.iter().map(|d| {
            MenuItem::new(d.name.clone())
                .checked(open && s.requested_input_device() == Some(d.name.as_str()))
        }));
        if let Some(i) = ui.menu(
            W::new("settings-input-menu"),
            vec2(anchor.x, anchor.bottom() + 4.0),
            &items,
        ) {
            input_choice = Some(match i {
                0 => None,
                1 => Some(None),
                n => Some(Some(input_devices[n - 3].name.clone())),
            });
        }
        if !ui.is_popup_open(W::new("settings-input-menu")) {
            ui.open_popup(id, r);
        }
    }
    if ui.is_popup_open(W::new("settings-channels-menu")) {
        let anchor = ui.popup_rect().unwrap_or_default();
        let count = s.input_info().map_or(2, |i| i.channels);
        let choices = channel_choices(count);
        let current = s.input_channels();
        let items: Vec<MenuItem> = choices
            .iter()
            .map(|c| MenuItem::new(channels_label(*c)).checked(*c == current))
            .collect();
        if let Some(i) = ui.menu(
            W::new("settings-channels-menu"),
            vec2(anchor.x, anchor.bottom() + 4.0),
            &items,
        ) {
            channel_choice = Some(choices[i]);
        }
        if !ui.is_popup_open(W::new("settings-channels-menu")) {
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
    if let Some(choice) = input_choice {
        match choice {
            None => s.close_input(),
            Some(device) => {
                if s.set_input_device(device.clone()).is_ok() {
                    prefs.input_device = device;
                    prefs.save();
                }
            }
        }
    }
    if let Some(c) = channel_choice {
        s.set_input_channels(c);
        prefs.set_input_channels(c);
        prefs.save();
    }
    {
        let st = ui.state::<Devices>(cache);
        st.outputs = devices;
        st.inputs = input_devices;
    }
    if refresh_outputs {
        ui.state::<Devices>(cache).outputs = s.audio_devices();
    }
    if refresh_inputs {
        ui.state::<Devices>(cache).inputs = s.input_devices();
    }
    if let Some(sel) = midi_choice {
        let off = matches!(sel, PortSelection::Nothing);
        s.set_midi_enabled(!off);
        if !off {
            s.select_midi_input(sel);
        }
    }
}
