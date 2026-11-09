//! Audio input, recording and output routing in one sheet.
//!
//! Left: the input device and channels, a live level, monitoring and the
//! armed lanes with a Record button, so a vocal take is: choose the
//! microphone input, arm a lane, press Record. Right: the main output's
//! channel pair and up to four extra hardware outputs (another interface,
//! the computer's own speakers, a headphone cue) fed from the master mix or
//! any track. Everything shown is session state; nothing here opens a
//! device until the user picks one.

use peel_document::Id;
use peel_session::{
    InputChannels, InputDeviceInfo, OutputChannels, OutputDeviceInfo, OutputRoute, Session,
};
use peel_ui::Id as W;
use peel_ui::{
    Align, Color, CursorIcon, FocusKind, LayerId, MenuItem, Rect, Role, Semantics, Ui, icons, vec2,
};

use crate::instrument_editor::{Rows, section, switch};
use crate::prefs::Prefs;
use crate::recorder::Recorder;
use crate::workspace::Workspace;

/// Device lists are read when the sheet opens (and on Rescan), never per
/// frame.
#[derive(Default)]
struct Cache {
    loaded: bool,
    outputs: Vec<OutputDeviceInfo>,
    inputs: Vec<InputDeviceInfo>,
    /// Last refusal when applying outputs, shown under the list.
    error: Option<String>,
    left_scroll: f32,
    right_scroll: f32,
}

const CACHE: W = W(0x41d1_0c0a_91e2);

pub fn channel_choices_in(count: u16) -> Vec<InputChannels> {
    let mut v: Vec<InputChannels> = (0..count.max(1)).map(InputChannels::Mono).collect();
    v.extend((0..count / 2).map(|k| InputChannels::Stereo(k * 2, k * 2 + 1)));
    v
}

pub fn input_label(c: InputChannels) -> String {
    match c {
        InputChannels::Mono(a) => format!("Input {} (mono)", a + 1),
        InputChannels::Stereo(a, b) => format!("Inputs {} + {} (stereo)", a + 1, b + 1),
    }
}

/// Output channel choices for a device with `count` channels: every
/// stereo pair, then single outputs.
pub fn channel_choices_out(count: u16) -> Vec<OutputChannels> {
    let mut v: Vec<OutputChannels> = (0..count / 2)
        .map(|k| OutputChannels::Stereo(k * 2, k * 2 + 1))
        .collect();
    v.extend((0..count.max(1)).map(OutputChannels::Mono));
    v
}

pub fn output_label(c: OutputChannels) -> String {
    match c {
        OutputChannels::Mono(a) => format!("Output {} (mono)", a + 1),
        OutputChannels::Stereo(a, b) => format!("Outputs {} + {}", a + 1, b + 1),
    }
}

/// A fresh extra output: the master mix to the first device that is not
/// the main one (or the main device when it is the only one), first pair.
pub fn new_route(devices: &[OutputDeviceInfo], primary: &str, n: usize) -> Option<OutputRoute> {
    let dev = devices
        .iter()
        .find(|d| d.name != primary && d.id != primary)
        .or_else(|| devices.first())?;
    let used = devices.iter().any(|d| d.name == primary) && dev.name == primary;
    // On the main device the first pair is taken: start on the next one.
    let first = if used && dev.max_channels >= 4 { 2 } else { 0 };
    Some(OutputRoute {
        name: format!("Output {}", n + 1),
        track: None,
        device: dev.id.clone(),
        channels: if dev.max_channels >= first + 2 {
            OutputChannels::Stereo(first, first + 1)
        } else {
            OutputChannels::Mono(first.min(dev.max_channels.saturating_sub(1)))
        },
        gain_db: 0.0,
        muted: false,
    })
}

pub fn draw(
    ui: &mut Ui,
    area: Rect,
    s: &mut Session,
    ws: &mut Workspace,
    prefs: &mut Prefs,
    recorder: &mut Recorder,
) {
    if !ws.io_open {
        ui.remove_state(CACHE);
        return;
    }
    let t = ui.theme.clone();
    if !ui.state::<Cache>(CACHE).loaded {
        let (outputs, inputs) = (s.audio_devices(), s.input_devices());
        let c = ui.state::<Cache>(CACHE);
        c.outputs = outputs;
        c.inputs = inputs;
        c.loaded = true;
    }
    let sheet = crate::fx_editor::sheet_rect(area);
    ui.with_layer(LayerId::ABOVE, |ui| {
        ui.scene.push_clip_absolute(area);
        ui.scene
            .rect(area, 0.0, Color::rgba(0.03, 0.04, 0.06, 0.62));
        let scrim = ui.interact(W::new("io-scrim"), area);
        ui.scene.shadow(sheet, t.radius_l, 18.0, t.shadow);
        ui.scene.rect(sheet, t.radius_l, t.panel);
        let swallow = ui.interact(W::new("io-sheet"), sheet);
        let mut body = sheet;
        let mut head = body.cut_top(t.header_h + 4.0).shrink2(14.0, 5.0);
        let close = ui.icon_button(
            W::new("io-close"),
            head.cut_right(30.0),
            icons::X,
            false,
            None,
        );
        ui.name(&close, "Close audio");
        ui.tooltip(&close, "Close (Esc)");
        let rescan = ui.button(W::new("io-rescan"), head.cut_right(80.0), "Rescan");
        ui.tooltip(&rescan, "Look for audio devices again");
        ui.text(head, "Audio", t.heading, t.text, Align::Start);
        ui.scene.hline(sheet.x, sheet.right(), body.y, t.line);
        if rescan.clicked {
            let (outputs, inputs) = (s.audio_devices(), s.input_devices());
            let c = ui.state::<Cache>(CACHE);
            c.outputs = outputs;
            c.inputs = inputs;
        }
        if close.clicked || (scrim.clicked && !swallow.hovered) {
            ws.io_open = false;
            ui.scene.pop_clip();
            return;
        }
        let body = body.shrink2(16.0, 12.0);
        let half = (body.w - 28.0) * 0.5;
        let mut left = body;
        let right = Rect {
            x: body.right() - half,
            w: half,
            ..body
        };
        left.w = half;
        ui.scene
            .vline(body.x + half + 14.0, body.y, body.bottom(), t.line);
        ui.scene.push_clip(left.expand(2.0));
        input_column(ui, left, s, ws, prefs, recorder);
        ui.scene.pop_clip();
        ui.scene.push_clip(right.expand(2.0));
        output_column(ui, right, s, prefs);
        ui.scene.pop_clip();
        ui.scene.pop_clip();
    });
}

fn level_bars(ui: &mut Ui, r: Rect, peaks: (f32, f32), live: bool) {
    let t = ui.theme.clone();
    ui.scene.rect(r, 3.0, t.well);
    for (k, v) in [peaks.0, peaks.1].into_iter().enumerate() {
        let lane = Rect {
            x: r.x + 2.0,
            y: r.y + 2.0 + k as f32 * (r.h - 4.0) * 0.5,
            w: r.w - 4.0,
            h: (r.h - 4.0) * 0.5 - 1.0,
        };
        let f = if live {
            ((20.0 * v.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0)
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
        if f > 0.0 {
            ui.scene.rect(lane.with_w(lane.w * f), 1.5, c);
        }
    }
}

fn labelled(ui: &mut Ui, rows: &mut Rows, h: f32, label: &str) -> Rect {
    let t = ui.theme.clone();
    let mut row = rows.row(h);
    ui.text(
        row.cut_left(78.0),
        label,
        t.caption,
        t.text_faint,
        Align::Start,
    );
    row
}

fn input_column(
    ui: &mut Ui,
    r: Rect,
    s: &mut Session,
    ws: &mut Workspace,
    prefs: &mut Prefs,
    recorder: &mut Recorder,
) {
    let t = ui.theme.clone();
    let lanes = s.project().arrangement().tracks.clone();
    let content_h = 640.0;
    let mut off = vec2(0.0, ui.state::<Cache>(CACHE).left_scroll);
    let scroll = content_h > r.h;
    if scroll {
        ui.scroll_area(W::new("io-left"), r, vec2(r.w, content_h), &mut off);
    } else {
        off.y = 0.0;
    }
    ui.state::<Cache>(CACHE).left_scroll = off.y;
    let mut rows = Rows {
        x: r.x,
        w: r.w - if scroll { 10.0 } else { 0.0 },
        y: r.y - off.y,
    };

    section(ui, &mut rows, "Input");
    rows.gap(6.0);
    // Device.
    let row = labelled(ui, &mut rows, 30.0, "Device");
    let open = s.input_is_open();
    let label = match (open, s.input_info()) {
        (true, Some(info)) => format!(
            "{}  ·  {} kHz",
            info.device_name,
            info.sample_rate as f32 / 1000.0
        ),
        _ => match s.requested_input_device() {
            Some(d) => format!("{d} (closed)"),
            None => "Closed".into(),
        },
    };
    let id = W::new("io-input");
    let resp = ui.dropdown(id, row, &label, "Input device");
    if resp.clicked {
        ui.open_popup(id.with("menu"), row);
    }
    let inputs = ui.state::<Cache>(CACHE).inputs.clone();
    let mut items = vec![
        MenuItem::new("System default").checked(open && s.requested_input_device().is_none()),
        MenuItem::new("Closed").checked(!open),
        MenuItem::Separator,
    ];
    items.extend(inputs.iter().map(|d| {
        MenuItem::new(format!("{}  ({} ch)", d.name, d.max_channels))
            .checked(open && s.requested_input_device() == Some(d.name.as_str()))
    }));
    if let Some(i) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
        let device = match i {
            0 => Some(None),
            1 => None,
            2 => return,
            n => Some(Some(inputs[n - 3].name.clone())),
        };
        match device {
            None => s.close_input(),
            Some(device) => match s.set_input_device(device.clone()) {
                Ok(()) => {
                    prefs.input_device = device;
                    prefs.save();
                }
                Err(e) => s.notify(e, true),
            },
        }
    }
    rows.gap(6.0);
    // Channels.
    let row = labelled(ui, &mut rows, 30.0, "Channels");
    let count = s.input_info().map_or(2, |i| i.channels);
    let cur = s.input_channels();
    let id = W::new("io-channels");
    let resp = ui.dropdown(id, row, &input_label(cur), "Input channels");
    if resp.clicked {
        ui.open_popup(id.with("menu"), row);
    }
    let choices = channel_choices_in(count);
    let items: Vec<MenuItem> = choices
        .iter()
        .map(|c| MenuItem::new(input_label(*c)).checked(*c == cur))
        .collect();
    if let Some(i) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
        s.set_input_channels(choices[i]);
        prefs.set_input_channels(choices[i]);
        prefs.save();
    }
    rows.gap(6.0);
    // Level.
    let row = labelled(ui, &mut rows, 26.0, "Level");
    let (l, rr) = ws.input_meter;
    let peak = l.max(rr);
    let mut bars = row;
    let figure = bars.cut_right(60.0);
    ui.text(
        figure,
        &if open {
            crate::util::peak_text(peak)
        } else {
            "-inf".into()
        },
        t.readout_small,
        if peak >= 1.0 { t.record } else { t.text_dim },
        Align::End,
    );
    level_bars(ui, bars.shrink2(0.0, 3.0), ws.input_meter, open);
    rows.gap(4.0);
    for (text, c) in [(s.input_error(), t.record), (s.input_warning(), t.warn)] {
        if let Some(text) = text {
            let used = ui.text_wrapped(
                vec2(rows.x + 78.0, rows.y),
                rows.w - 78.0,
                &text,
                t.caption,
                c,
            );
            rows.gap(used + 4.0);
        }
    }
    rows.gap(6.0);
    // Monitoring.
    let mon = s.monitor_status();
    let mut row = labelled(ui, &mut rows, 30.0, "Monitor");
    let sw = row.cut_left(86.0);
    if switch(
        ui,
        W::new("io-monitor"),
        sw,
        if mon.on { "On" } else { "Off" },
        "Hear the input through the output. Use headphones.",
        mon.on,
        t.accent,
    ) {
        s.set_monitoring(!mon.on, mon.gain);
    }
    row.cut_left(10.0);
    let mut norm = (mon.gain / 2.0).clamp(0.0, 1.0);
    let sl = ui.hslider(W::new("io-monitor-gain"), row, &mut norm, 0.5, t.accent);
    ui.name(&sl, "Monitor level");
    if sl.changed {
        s.set_monitoring(mon.on, norm * 2.0);
    }
    // Only a running monitor has something to report; the feedback caution
    // is there on request.
    if mon.on && mon.available {
        let note = format!(
            "{:.0} ms through{}",
            mon.latency_ms,
            if mon.underruns + mon.dropped_frames > 0 {
                format!(", {} dropouts", mon.underruns + mon.dropped_frames)
            } else {
                String::new()
            }
        );
        let used = ui.text_wrapped(
            vec2(rows.x + 78.0, rows.y + 4.0),
            rows.w - 78.0,
            &note,
            t.caption,
            t.text_faint,
        );
        rows.gap(used + 14.0);
    } else if mon.on {
        let why = mon
            .unavailable_reason
            .clone()
            .unwrap_or_else(|| "Waiting for the input".into());
        let used = ui.text_wrapped(
            vec2(rows.x + 78.0, rows.y + 4.0),
            rows.w - 78.0,
            &why,
            t.caption,
            t.warn,
        );
        rows.gap(used + 14.0);
    } else {
        let used = crate::util::hint(
            ui,
            W::new("io-monitor-hint"),
            Rect {
                x: rows.x + 78.0,
                y: rows.y + 4.0,
                w: rows.w - 78.0,
                h: 40.0,
            },
            "Use headphones: monitoring through speakers can feed back.",
        );
        rows.gap(used + 14.0);
    }

    // Recording: pick the lane, then Record. A vocal take is three
    // steps: choose the input above, choose a lane, press Record.
    section(ui, &mut rows, "Record");
    rows.gap(6.0);
    let armed = s.armed_lanes();
    let armed_count = armed.len();
    let row = labelled(ui, &mut rows, 30.0, "Lane");
    let lane_label = match armed.as_slice() {
        [] => "Choose a lane".to_string(),
        [one] => lanes
            .iter()
            .find(|l| l.id == *one)
            .map_or("Lane".to_string(), |l| l.name.clone()),
        many => format!("{} lanes", many.len()),
    };
    let id = W::new("io-lane");
    let resp = ui.dropdown(id, row, &lane_label, "Lane to record onto");
    ui.tooltip(&resp, "Armed lanes get the new recording as a clip");
    if resp.clicked {
        ui.open_popup(id.with("menu"), row);
    }
    let mut items: Vec<MenuItem> = lanes
        .iter()
        .map(|l| MenuItem::new(l.name.clone()).checked(s.is_lane_armed(l.id)))
        .collect();
    items.push(MenuItem::Separator);
    items.push(MenuItem::new("New lane"));
    if let Some(i) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
        if i < lanes.len() {
            let on = s.is_lane_armed(lanes[i].id);
            s.arm_lane(lanes[i].id, !on);
        } else if i == lanes.len() + 1 {
            let new = peel_document::Track {
                id: Id::new(),
                name: format!("Recording {}", lanes.len() + 1),
                color: None,
                muted: false,
            };
            let nid = new.id;
            s.doc
                .edit("Add lane", |p| p.arrangement_mut().tracks.push(new));
            s.arm_lane(nid, true);
        }
    }
    rows.gap(8.0);
    // Record and Stop.
    let mut row = rows.row(40.0);
    let recording = s.is_recording();
    let blocker = if recorder.armed && s.is_playing() {
        None
    } else {
        s.record_blocker()
    };
    if recording || (recorder.armed && s.is_playing()) {
        let stop = ui.button_primary(W::new("io-stop"), row.cut_left(120.0), "Stop");
        if stop.clicked {
            recorder.armed = false;
            s.stop();
        }
    } else {
        let rec_r = row.cut_left(120.0);
        let ok = blocker.is_none();
        let resp = ui.interact(W::new("io-record"), rec_r);
        ui.scene.rect(
            rec_r,
            t.radius_m,
            if !ok {
                t.control.alpha(0.5)
            } else if resp.hovered {
                t.record.lighten(0.1)
            } else {
                t.record
            },
        );
        let ink = if ok {
            crate::util::ink_on(t.record)
        } else {
            t.text_faint
        };
        ui.icon(Rect { w: 36.0, ..rec_r }, icons::RECORD, 16.0, ink);
        ui.text(
            Rect {
                x: rec_r.x + 32.0,
                w: rec_r.w - 32.0,
                ..rec_r
            },
            "Record",
            t.label,
            ink,
            Align::Start,
        );
        if resp.hovered && ok {
            ui.set_cursor(CursorIcon::Pointer);
        }
        ui.name(&resp, "Record");
        if let Some(why) = &blocker {
            ui.tooltip(&resp, why);
        }
        if resp.clicked && ok {
            recorder.armed = true;
            if !s.is_playing() {
                s.play();
            }
        }
    }
    row.cut_left(12.0);
    let status_text = match s.recording_status() {
        Some(st) if st.waiting_for_transport => "Ready. Waiting for playback to start.".to_string(),
        Some(st) => format!(
            "Recording {}:{:02}{}",
            st.seconds as u64 / 60,
            st.seconds as u64 % 60,
            if st.dropped_frames > 0 {
                format!("  ·  {} frames dropped", st.dropped_frames)
            } else {
                String::new()
            }
        ),
        None => format!(
            "{armed_count} lane{} armed",
            if armed_count == 1 { "" } else { "s" }
        ),
    };
    ui.text(
        row,
        &status_text,
        t.label,
        if recording { t.record } else { t.text_dim },
        Align::Start,
    );
    rows.gap(6.0);
    if let Some(why) = blocker {
        let used = ui.text_wrapped(vec2(rows.x, rows.y), rows.w, &why, t.caption, t.text_faint);
        rows.gap(used + 6.0);
    }
    if let Some(e) = s.recording_status().and_then(|st| st.error) {
        let used = ui.text_wrapped(vec2(rows.x, rows.y), rows.w, &e, t.caption, t.record);
        rows.gap(used + 6.0);
    }
    rows.gap(6.0);
    // Latency compensation.
    let row = labelled(ui, &mut rows, 28.0, "Offset");
    let lat = s.input_latency();
    let mut v = f64::from(s.record_offset_ms());
    let resp = ui.drag_number(
        W::new("io-offset"),
        Rect { w: 86.0, ..row }.shrink2(0.0, 2.0),
        &mut v,
        -1000.0,
        1000.0,
        0.25,
        t.readout_small,
        t.text,
        |v| format!("{v:+.1} ms"),
    );
    ui.name(&resp, "Recording offset in milliseconds");
    if resp.changed {
        s.set_record_offset_ms(v as f32);
    }
    ui.text(
        Rect {
            x: row.x + 96.0,
            w: row.w - 96.0,
            ..row
        },
        &format!("on top of {:.1} ms measured", lat.round_trip_s() * 1000.0),
        t.caption,
        t.text_faint,
        Align::Start,
    );
    rows.gap(8.0);
    // Where takes go: the folder's name, the full path on hover.
    let dir = s.recordings_dir();
    let name = dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut row = labelled(ui, &mut rows, 28.0, "Folder");
    let choose_r = row.cut_right(84.0);
    row.cut_right(6.0);
    let busy = s.is_recording();
    let choose = ui.interact_node(
        W::new("io-folder"),
        choose_r,
        FocusKind::Activate,
        Semantics::new(Role::Button)
            .label("Choose recordings folder")
            .disabled(busy),
    );
    ui.scene.rect(
        choose_r,
        t.radius_m,
        if busy {
            t.control.alpha(0.4)
        } else if choose.hovered {
            t.control_hover
        } else {
            t.control
        },
    );
    ui.text(
        choose_r,
        "Choose",
        t.label,
        if busy { t.text_faint } else { t.text },
        Align::Center,
    );
    ui.focus_ring(W::new("io-folder"), choose_r, t.radius_m);
    ui.tooltip(
        &choose,
        if busy {
            "Not while recording"
        } else {
            "Choose where takes are saved"
        },
    );
    if choose.clicked
        && !busy
        && let Some(picked) = crate::dialogs::folder(&dir)
    {
        s.set_recordings_dir(Some(picked.clone()));
        prefs.recordings_dir = Some(picked);
        prefs.save();
    }
    let shown = ui.fit_text(&name, t.label, row.w);
    let path_hit = ui.interact(W::new("io-folder-path"), row);
    ui.text(row, &shown, t.label, t.text_dim, Align::Start);
    ui.tooltip(&path_hit, &dir.display().to_string());
}

fn output_column(ui: &mut Ui, r: Rect, s: &mut Session, prefs: &mut Prefs) {
    let t = ui.theme.clone();
    let routes: Vec<OutputRoute> = s.output_routes().to_vec();
    let content_h = 330.0 + 140.0 * routes.len() as f32;
    let mut off = vec2(0.0, ui.state::<Cache>(CACHE).right_scroll);
    let scroll = content_h > r.h;
    if scroll {
        ui.scroll_area(W::new("io-right"), r, vec2(r.w, content_h), &mut off);
    } else {
        off.y = 0.0;
    }
    ui.state::<Cache>(CACHE).right_scroll = off.y;
    let mut rows = Rows {
        x: r.x,
        w: r.w - if scroll { 10.0 } else { 0.0 },
        y: r.y - off.y,
    };
    let devices = ui.state::<Cache>(CACHE).outputs.clone();

    section(ui, &mut rows, "Main output");
    rows.gap(6.0);
    let mut row = labelled(ui, &mut rows, 28.0, "Device");
    let settings = ui.button(W::new("io-main-settings"), row.cut_right(84.0), "Settings");
    ui.tooltip(&settings, "Change the main device and buffer size");
    row.cut_right(6.0);
    let shown = ui.fit_text(s.audio_device_name(), t.label, row.w);
    ui.text(row, &shown, t.label, t.text, Align::Start);
    if settings.clicked {
        ui.open_popup(W::new("settings-panel"), settings.rect);
    }
    rows.gap(4.0);
    let row = labelled(ui, &mut rows, 30.0, "Channels");
    let count = s.primary_output_channel_count();
    let cur = s.primary_output_channels();
    let id = W::new("io-main-channels");
    let resp = ui.dropdown(id, row, &output_label(cur), "Main output channels");
    if resp.clicked {
        ui.open_popup(id.with("menu"), row);
    }
    let choices = channel_choices_out(count);
    let items: Vec<MenuItem> = choices
        .iter()
        .map(|c| MenuItem::new(output_label(*c)).checked(*c == cur))
        .collect();
    if let Some(i) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
        match s.set_primary_output_channels(choices[i]) {
            Ok(()) => {
                prefs.primary_output = Some(choices[i]);
                prefs.save();
            }
            Err(e) => s.notify(e, true),
        }
    }
    rows.gap(14.0);

    section(ui, &mut rows, "Extra outputs");
    rows.gap(6.0);
    let status = s.output_route_status();
    let tracks: Vec<(Option<Id>, String)> = std::iter::once((None, "Master mix".to_string()))
        .chain(
            s.project()
                .mixer
                .tracks
                .iter()
                .skip(1)
                .map(|t| (Some(t.id), t.name.clone())),
        )
        .collect();
    let mut new_routes: Option<Vec<OutputRoute>> = None;
    let mut dirty = routes.clone();
    if routes.is_empty() {
        let r = rows.row(20.0);
        ui.text(r, "No extra outputs", t.caption, t.text_faint, Align::Start);
    }
    for (i, route) in routes.iter().enumerate() {
        ui.scene.hline(rows.x, rows.x + rows.w, rows.y, t.line);
        rows.gap(8.0);
        let mut head = rows.row(24.0);
        let healthy = status.get(i).is_none_or(|st| st.healthy);
        let x = ui.icon_button(
            W::new(("io-route-remove", i)),
            head.cut_right(26.0),
            icons::X,
            false,
            None,
        );
        ui.name(&x, &format!("Remove {}", route.name));
        ui.tooltip(&x, "Remove this output");
        let m = switch(
            ui,
            W::new(("io-route-mute", i)),
            head.cut_right(60.0).shrink2(0.0, 1.0),
            "Mute",
            &format!("Mute {}", route.name),
            route.muted,
            t.warn,
        );
        head.cut_right(6.0);
        let dot = head.cut_left(16.0);
        ui.scene
            .circle(dot.center(), 4.0, if healthy { t.play } else { t.record });
        let shown = ui.fit_text(&route.name, t.label, head.w);
        ui.text(head, &shown, t.label, t.text, Align::Start);
        if x.clicked {
            dirty.remove(i);
            new_routes = Some(dirty.clone());
            break;
        }
        if m {
            dirty[i].muted = !route.muted;
            new_routes = Some(dirty.clone());
        }
        rows.gap(4.0);
        // Source.
        let row = labelled(ui, &mut rows, 28.0, "Source");
        let src_name = tracks
            .iter()
            .find(|(id, _)| *id == route.track)
            .map_or("Missing track".to_string(), |(_, n)| n.clone());
        let id = W::new(("io-route-source", i));
        let resp = ui.dropdown(id, row, &src_name, "Output source");
        if resp.clicked {
            ui.open_popup(id.with("menu"), row);
        }
        let items: Vec<MenuItem> = tracks
            .iter()
            .map(|(tid, n)| MenuItem::new(n.clone()).checked(*tid == route.track))
            .collect();
        if let Some(k) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
            dirty[i].track = tracks[k].0;
            new_routes = Some(dirty.clone());
        }
        rows.gap(4.0);
        // Device.
        let row = labelled(ui, &mut rows, 28.0, "Device");
        let dev_name = devices
            .iter()
            .find(|d| d.id == route.device || d.name == route.device)
            .map_or(route.device.clone(), |d| d.name.clone());
        let id = W::new(("io-route-device", i));
        let resp = ui.dropdown(id, row, &dev_name, "Output device");
        if resp.clicked {
            ui.open_popup(id.with("menu"), row);
        }
        let items: Vec<MenuItem> = devices
            .iter()
            .map(|d| {
                MenuItem::new(format!(
                    "{}{}",
                    d.name,
                    if d.is_default { "  (default)" } else { "" }
                ))
                .checked(d.id == route.device || d.name == route.device)
            })
            .collect();
        if let Some(k) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
            let d = &devices[k];
            dirty[i].device = d.id.clone();
            // Keep the channels valid for the new device.
            let max = d.max_channels.max(1);
            if !channel_choices_out(max).contains(&dirty[i].channels) {
                dirty[i].channels = if max >= 2 {
                    OutputChannels::Stereo(0, 1)
                } else {
                    OutputChannels::Mono(0)
                };
            }
            new_routes = Some(dirty.clone());
        }
        rows.gap(4.0);
        // Channels and gain.
        let mut row = labelled(ui, &mut rows, 28.0, "Channels");
        let gain_r = row.cut_right(76.0);
        row.cut_right(6.0);
        let max = devices
            .iter()
            .find(|d| d.id == route.device || d.name == route.device)
            .map_or(2, |d| d.max_channels);
        let id = W::new(("io-route-channels", i));
        let resp = ui.dropdown(id, row, &output_label(route.channels), "Output channels");
        if resp.clicked {
            ui.open_popup(id.with("menu"), row);
        }
        let choices = channel_choices_out(max);
        let items: Vec<MenuItem> = choices
            .iter()
            .map(|c| MenuItem::new(output_label(*c)).checked(*c == route.channels))
            .collect();
        if let Some(k) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
            dirty[i].channels = choices[k];
            new_routes = Some(dirty.clone());
        }
        let mut g = f64::from(route.gain_db);
        let gresp = ui.drag_number(
            W::new(("io-route-gain", i)),
            gain_r.shrink2(0.0, 2.0),
            &mut g,
            -90.0,
            12.0,
            0.25,
            t.readout_small,
            t.text,
            |v| format!("{v:+.1} dB"),
        );
        ui.name(&gresp, &format!("{} level", route.name));
        if gresp.changed {
            dirty[i].gain_db = g as f32;
            new_routes = Some(dirty.clone());
        }
        // Status.
        if let Some(st) = status.get(i) {
            let text = if st.healthy {
                format!(
                    "{} kHz  ·  {:.0} ms  ·  {:+.0} ppm drift{}",
                    st.sample_rate as f32 / 1000.0,
                    st.latency_ms,
                    st.clock_correction_ppm,
                    if st.underruns + st.overruns > 0 {
                        format!("  ·  {} dropouts", st.underruns + st.overruns)
                    } else {
                        String::new()
                    }
                )
            } else {
                "Not running. The device may be unplugged: pick it again to retry.".to_string()
            };
            let used = ui.text_wrapped(
                vec2(rows.x + 78.0, rows.y + 4.0),
                rows.w - 78.0,
                &text,
                t.caption,
                if st.healthy { t.text_faint } else { t.record },
            );
            rows.gap(used + 10.0);
        }
    }
    rows.gap(8.0);
    let mut row = rows.row(30.0);
    let can_add = routes.len() < 4 && !devices.is_empty();
    let add = ui.button(W::new("io-route-add"), row.cut_left(130.0), "Add output");
    ui.tooltip(
        &add,
        if routes.len() >= 4 {
            "Four extra outputs is the most"
        } else if devices.is_empty() {
            "No output devices found"
        } else {
            "Play the master mix or a track to another device or channel pair"
        },
    );
    if add.clicked
        && can_add
        && let Some(r) = new_route(&devices, s.audio_device_name(), routes.len())
    {
        dirty.push(r);
        new_routes = Some(dirty.clone());
    }
    rows.gap(8.0);
    if let Some(e) = ui.state::<Cache>(CACHE).error.clone() {
        let used = ui.text_wrapped(vec2(rows.x, rows.y), rows.w, &e, t.caption, t.record);
        rows.gap(used + 6.0);
    }
    let used = crate::util::hint(
        ui,
        W::new("io-clock-hint"),
        Rect {
            x: rows.x,
            y: rows.y,
            w: rows.w,
            h: 60.0,
        },
        "Each extra output runs on its own clock, drift-corrected: good for cues and headphones, not sample-locked to the main output. Use an aggregate device for that.",
    );
    rows.gap(used);

    if let Some(new) = new_routes {
        match s.configure_output_routes(new.clone()) {
            Ok(()) => {
                prefs.output_routes = new;
                prefs.save();
                ui.state::<Cache>(CACHE).error = None;
            }
            Err(e) => ui.state::<Cache>(CACHE).error = Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, ch: u16, default: bool) -> OutputDeviceInfo {
        OutputDeviceInfo {
            name: name.into(),
            id: format!("id-{name}"),
            is_default: default,
            max_channels: ch,
            sample_rate_ranges: vec![(44_100, 96_000)],
            sample_rates: vec![44_100, 48_000],
            default_sample_rate: Some(48_000),
            buffer_frames: None,
        }
    }

    #[test]
    fn output_choices_list_pairs_then_single_outputs() {
        let c = channel_choices_out(4);
        assert_eq!(c[0], OutputChannels::Stereo(0, 1));
        assert_eq!(c[1], OutputChannels::Stereo(2, 3));
        assert_eq!(c.len(), 2 + 4);
        assert_eq!(channel_choices_out(1), vec![OutputChannels::Mono(0)]);
        assert_eq!(output_label(OutputChannels::Stereo(2, 3)), "Outputs 3 + 4");
    }

    #[test]
    fn a_new_extra_output_prefers_another_device() {
        let devices = vec![dev("MacBook Speakers", 2, true), dev("Interface", 8, false)];
        let r = new_route(&devices, "MacBook Speakers", 0).unwrap();
        assert_eq!(r.device, "id-Interface");
        assert_eq!(r.channels, OutputChannels::Stereo(0, 1));
        assert_eq!(r.track, None);
    }

    #[test]
    fn with_one_device_the_extra_output_takes_a_free_pair() {
        let devices = vec![dev("Interface", 8, true)];
        let r = new_route(&devices, "Interface", 0).unwrap();
        assert_eq!(
            r.channels,
            OutputChannels::Stereo(2, 3),
            "the main pair stays free"
        );
        let small = vec![dev("Speakers", 2, true)];
        let r = new_route(&small, "Speakers", 0).unwrap();
        assert_eq!(r.channels, OutputChannels::Stereo(0, 1));
        assert!(new_route(&[], "x", 0).is_none());
    }

    #[test]
    fn input_labels_name_mono_and_stereo_choices() {
        assert_eq!(input_label(InputChannels::Mono(0)), "Input 1 (mono)");
        assert_eq!(
            input_label(InputChannels::Stereo(2, 3)),
            "Inputs 3 + 4 (stereo)"
        );
        assert_eq!(channel_choices_in(4).len(), 4 + 2);
    }
}
