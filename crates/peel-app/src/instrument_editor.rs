//! Instrument editor: a large sheet with the instrument's parameters, a
//! playable piano (mouse and computer keyboard) and the MIDI controls that
//! reach it. Plugin instruments get the same parameter panel; native
//! plugin windows are not embedded by the host.
//!
//! Notes never hang: mouse notes end when the button comes up, the pointer
//! leaves the window, the editor closes, the target changes or the window
//! loses focus. Panic ends everything on every channel.

use peel_document::Id;
use peel_session::{MidiControlTarget, MidiEvent, MidiLearnStatus, MidiMapping, Session};
use peel_ui::Id as W;
use peel_ui::{
    Align, Color, CursorIcon, FocusKind, LayerId, MenuItem, Rect, Role, Semantics, Ui, icons, vec2,
};

use crate::prefs::Prefs;
use crate::util::col;
use crate::workspace::{Focus, Workspace};

/// View state of the instrument editor. Session only.
pub struct InstState {
    /// The channel being played and edited.
    pub target: Option<Id>,
    /// Velocity of mouse notes, 0.05..=1.
    pub velocity: f32,
    /// The sustain pedal is held down by the editor's own switch.
    pub sustain: bool,
    /// Key under a pressed mouse button.
    pub mouse_key: Option<u8>,
    /// Parameter picked in the controller mapper.
    pub pick: Option<String>,
    chain_scroll: f32,
    rail_scroll: f32,
}

impl Default for InstState {
    fn default() -> Self {
        InstState {
            target: None,
            velocity: 0.8,
            sustain: false,
            mouse_key: None,
            pick: None,
            chain_scroll: 0.0,
            rail_scroll: 0.0,
        }
    }
}

impl InstState {
    pub fn is_open(&self) -> bool {
        self.target.is_some()
    }
}

/// MIDI channel used for notes played in the editor: the filter's channel,
/// or channel 1 when listening to all.
pub fn play_channel(s: &Session) -> u8 {
    s.midi_channel_filter().unwrap_or(0)
}

/// Ends every note and the sustain switch the editor holds.
pub fn release_all(s: &mut Session, ws: &mut Workspace) {
    let ch = play_channel(s);
    if let Some(k) = ws.inst.mouse_key.take() {
        s.handle_midi_event(MidiEvent::NoteOff {
            channel: ch,
            key: k,
            velocity: 0,
        });
    }
    if ws.inst.sustain {
        ws.inst.sustain = false;
        s.handle_midi_event(MidiEvent::Sustain {
            channel: ch,
            down: false,
        });
    }
}

/// Opens the editor on `channel` without changing the project. The channel
/// becomes the selected one so the computer keyboard plays it too.
pub fn open(s: &mut Session, ws: &mut Workspace, channel: Id) {
    if ws.inst.target != Some(channel) {
        release_all(s, ws);
    }
    ws.fx.close();
    ws.io_open = false;
    ws.inst.target = Some(channel);
    ws.selected_channel = Some(channel);
    ws.focus = Some(Focus::Channel(channel));
}

pub fn close(s: &mut Session, ws: &mut Workspace) {
    release_all(s, ws);
    ws.inst.target = None;
    s.cancel_midi_learn();
}

// ---- piano geometry (pure) ----

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PianoKey {
    pub key: u8,
    pub rect: Rect,
    pub black: bool,
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

/// Keys from `first` (a C) for `octaves` octaves plus the closing C, laid
/// out across `r`. Black keys come last so hit tests can prefer them.
pub fn piano_layout(r: Rect, first: u8, octaves: u8) -> Vec<PianoKey> {
    let last = (first as u16 + octaves as u16 * 12).min(127) as u8;
    let whites: Vec<u8> = (first..=last).filter(|k| !is_black(*k)).collect();
    let w = r.w / whites.len().max(1) as f32;
    let mut out = Vec::new();
    for (i, k) in whites.iter().enumerate() {
        out.push(PianoKey {
            key: *k,
            rect: Rect {
                x: r.x + i as f32 * w,
                y: r.y,
                w,
                h: r.h,
            },
            black: false,
        });
    }
    for (i, k) in whites.iter().enumerate() {
        let next = k + 1;
        if next <= last && is_black(next) {
            out.push(PianoKey {
                key: next,
                rect: Rect {
                    x: r.x + (i as f32 + 1.0) * w - w * 0.3,
                    y: r.y,
                    w: w * 0.6,
                    h: r.h * 0.62,
                },
                black: true,
            });
        }
    }
    out
}

/// The key under `p`, black keys first.
pub fn key_at(keys: &[PianoKey], p: peel_ui::Vec2) -> Option<u8> {
    keys.iter()
        .rev()
        .find(|k| k.black && k.rect.contains(p))
        .or_else(|| keys.iter().find(|k| !k.black && k.rect.contains(p)))
        .map(|k| k.key)
}

/// Letter on the computer keyboard that plays `offset` semitones above the
/// current octave's C (the Q row for the upper range).
fn typing_letter(offset: i32) -> Option<&'static str> {
    const LOWER: [&str; 12] = ["Z", "S", "X", "D", "C", "V", "G", "B", "H", "N", "J", "M"];
    const UPPER: [&str; 17] = [
        "Q", "2", "W", "3", "E", "R", "5", "T", "6", "Y", "7", "U", "I", "9", "O", "0", "P",
    ];
    match offset {
        0..=11 => Some(LOWER[offset as usize]),
        12..=28 => Some(UPPER[(offset - 12) as usize]),
        _ => None,
    }
}

// ---- drawing ----

pub fn draw(ui: &mut Ui, area: Rect, s: &mut Session, ws: &mut Workspace, prefs: &mut Prefs) {
    let Some(target) = ws.inst.target else {
        return;
    };
    let Some(ch) = s.project().channel(target).cloned() else {
        close(s, ws);
        return;
    };
    let t = ui.theme.clone();
    let sheet = crate::fx_editor::sheet_rect(area);
    ui.with_layer(LayerId::ABOVE, |ui| {
        ui.scene.push_clip_absolute(area);
        ui.scene.rect(area, 0.0, Color::rgba(0.03, 0.04, 0.06, 0.62));
        let scrim = ui.interact(W::new("inst-scrim"), area);
        ui.scene.shadow(sheet, t.radius_l, 18.0, t.shadow);
        ui.scene.rect(sheet, t.radius_l, t.panel);
        let swallow = ui.interact(W::new("inst-sheet"), sheet);
        if scrim.clicked && !swallow.hovered {
            close(s, ws);
            ui.scene.pop_clip();
            return;
        }
        let mut body = sheet;
        let head = body.cut_top(t.header_h + 4.0);
        if header(ui, head, &ch, s, ws) {
            ui.scene.pop_clip();
            return;
        }
        ui.scene.hline(sheet.x, sheet.right(), body.y, t.line);
        let mut body = body.shrink2(14.0, 12.0);
        let piano_h = if body.h > 430.0 { 150.0 } else { 118.0 };
        let piano_r = body.bottom_part(piano_h);
        body.cut_bottom(piano_h + 12.0);
        let mut rail = body.cut_right(308.0);
        body.cut_right(16.0);
        ui.scene.push_clip(body.expand(2.0));
        params(ui, body, &ch, s, ws);
        ui.scene.pop_clip();
        ui.scene.push_clip(rail.expand(2.0));
        rail_content(ui, &mut rail, &ch, s, ws, prefs);
        ui.scene.pop_clip();
        piano(ui, piano_r, &ch, s, ws);
        ui.scene.pop_clip();
    });
    // Safety net: a mouse note never outlives the button.
    if ws.inst.mouse_key.is_some() && !ui.input.down[0] {
        release_mouse_note(s, ws);
    }
    if ui.input.focus_lost {
        release_all(s, ws);
    }
}

fn release_mouse_note(s: &mut Session, ws: &mut Workspace) {
    if let Some(k) = ws.inst.mouse_key.take() {
        s.handle_midi_event(MidiEvent::NoteOff {
            channel: play_channel(s),
            key: k,
            velocity: 0,
        });
    }
}

trait BottomPart {
    fn bottom_part(&self, h: f32) -> Rect;
}

impl BottomPart for Rect {
    fn bottom_part(&self, h: f32) -> Rect {
        Rect {
            y: self.bottom() - h,
            h,
            ..*self
        }
    }
}

fn header(
    ui: &mut Ui,
    head: Rect,
    ch: &peel_document::Channel,
    s: &mut Session,
    ws: &mut Workspace,
) -> bool {
    let t = ui.theme.clone();
    let mut row = head.shrink2(12.0, 5.0);
    let close_btn = ui.icon_button(W::new("inst-close"), row.cut_right(30.0), icons::X, false, None);
    ui.name(&close_btn, "Close instrument");
    ui.tooltip(&close_btn, "Close (Esc)");
    row.cut_right(6.0);
    let panic = ui.button(W::new("inst-panic"), row.cut_right(64.0), "Panic");
    ui.tooltip(&panic, "Silence every note on every channel");
    row.cut_right(12.0);
    let swatch = row.cut_left(4.0);
    ui.scene.rect(swatch, 2.0, col(ch.color));
    row.cut_left(8.0);
    let title_w = (ui.measure(&ch.name, t.heading).x + 12.0).min(row.w * 0.28);
    let title = row.cut_left(title_w);
    let shown = ui.fit_text(&ch.name, t.heading, title.w);
    ui.text(title, &shown, t.heading, t.text, Align::Start);
    row.cut_left(8.0);
    ui.divider_v(row.cut_left(1.0));
    row.cut_left(8.0);

    // Every channel as a chip: switching never closes the editor.
    let chips: Vec<(Id, String, Color)> = s
        .project()
        .channels
        .iter()
        .map(|c| (c.id, c.name.clone(), col(c.color)))
        .collect();
    let widths: Vec<f32> = chips
        .iter()
        .map(|(_, n, _)| ui.measure(n, t.label).x + 32.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0 * chips.len() as f32;
    let mut off = vec2(ws.inst.chain_scroll, 0.0);
    if total > row.w {
        ui.scroll_area(W::new("inst-chain"), row, vec2(total, row.h), &mut off);
    } else {
        off.x = 0.0;
    }
    ws.inst.chain_scroll = off.x;
    ui.scene.push_clip(row);
    let mut x = row.x - off.x;
    let mut switch = None;
    for ((id, n, c), w) in chips.iter().zip(widths) {
        let cell = Rect {
            x,
            y: row.y,
            w,
            h: row.h - if total > row.w { 4.0 } else { 0.0 },
        };
        x += w + 4.0;
        if cell.right() < row.x || cell.x > row.right() {
            continue;
        }
        let on = *id == ch.id;
        let cid = W::new(("inst-chip", *id));
        let resp = ui.interact_node(
            cid,
            cell,
            FocusKind::Activate,
            Semantics::new(Role::Tab).label(n.clone()).selected(on),
        );
        ui.scene.rect(
            cell,
            t.radius_m,
            if on {
                t.raised
            } else if resp.hovered {
                t.control.alpha(0.6)
            } else {
                t.well
            },
        );
        let mut inner = cell.shrink2(8.0, 0.0);
        let dot = inner.cut_left(10.0);
        ui.scene.circle(dot.center(), 3.5, *c);
        inner.cut_left(4.0);
        ui.text(inner, n, t.label, if on { t.text } else { t.text_dim }, Align::Start);
        if on {
            ui.scene.rect(
                Rect {
                    x: cell.x + 6.0,
                    y: cell.bottom() - 2.0,
                    w: cell.w - 12.0,
                    h: 2.0,
                },
                1.0,
                t.accent,
            );
        }
        ui.focus_ring(cid, cell, t.radius_m);
        if resp.hovered {
            ui.set_cursor(CursorIcon::Pointer);
        }
        if resp.clicked && !on {
            switch = Some(*id);
        }
    }
    ui.scene.pop_clip();
    if let Some(id) = switch {
        open(s, ws, id);
    }
    if panic.clicked {
        release_all(s, ws);
        s.panic();
        s.notify("Silenced every note", false);
    }
    if close_btn.clicked {
        close(s, ws);
        return true;
    }
    false
}

fn params(ui: &mut Ui, body: Rect, ch: &peel_document::Channel, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let Some(desc) = peel_devices::descriptor(&ch.device.type_id) else {
        ui.text(
            body.with_h(24.0),
            "This instrument is not available",
            t.body,
            t.text_faint,
            Align::Start,
        );
        return;
    };
    let mut c = body;
    let mut top = c.cut_top(26.0);
    let is_plugin = ch.device.type_id.starts_with("clap:");
    if is_plugin {
        ui.text(
            top.cut_right(300.0),
            "Plugin windows are not embedded. Parameters are below.",
            t.caption,
            t.text_faint,
            Align::End,
        );
    }
    ui.text(top, desc.name, t.heading, t.text, Align::Start);
    c.cut_top(8.0);
    crate::inspector::device_panel(ui, c, &ch.device, desc, s, ws, col(ch.color));
}

// ---- piano ----

fn piano(ui: &mut Ui, r: Rect, ch: &peel_document::Channel, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let base = ((ws.octave + 1) * 12).clamp(0, 108) as u8;
    let octaves = if r.w >= 900.0 { 4 } else { 3 };
    let keys = piano_layout(r, base, octaves);
    let id = W::new("inst-piano");
    let resp = ui.interact_node(
        id,
        r,
        FocusKind::Activate,
        Semantics::new(Role::Group)
            .label("Piano")
            .description("Click and drag to play. The computer keyboard plays too: Z to M, Q to P."),
    );
    let hover = ui.pointer().and_then(|p| key_at(&keys, p));
    // Mouse: press plays, dragging across keys glides, release stops.
    let down = ui.input.down[0];
    let ch_midi = play_channel(s);
    let vel = (ws.inst.velocity * 127.0).round().clamp(1.0, 127.0) as u8;
    if resp.pressed {
        if let Some(k) = hover {
            ws.inst.mouse_key = Some(k);
            s.handle_midi_event(MidiEvent::NoteOn {
                channel: ch_midi,
                key: k,
                velocity: vel,
            });
        }
    } else if ui.is_active(id) && down && hover != ws.inst.mouse_key {
        release_mouse_note(s, ws);
        if let Some(k) = hover {
            ws.inst.mouse_key = Some(k);
            s.handle_midi_event(MidiEvent::NoteOn {
                channel: ch_midi,
                key: k,
                velocity: vel,
            });
        }
    }
    if ws.inst.mouse_key.is_some() && !down {
        release_mouse_note(s, ws);
    }
    if resp.hovered || resp.active {
        ui.set_cursor(CursorIcon::Pointer);
    }
    let held: Vec<u8> = ws
        .held
        .values()
        .filter(|(c, _)| *c == ch.id)
        .map(|(_, k)| *k)
        .collect();
    let lit = |k: u8| ws.inst.mouse_key == Some(k) || held.contains(&k);
    let accent = col(ch.color);
    for k in keys.iter().filter(|k| !k.black) {
        let on = lit(k.key);
        let hot = hover == Some(k.key) && !on;
        ui.scene.rect(
            k.rect.shrink(0.5),
            2.0,
            if on {
                accent
            } else if hot {
                t.key_white.mix(t.accent, 0.18)
            } else {
                t.key_white
            },
        );
        let mut label = String::new();
        if k.key % 12 == 0 {
            label = crate::util::note_name(k.key);
        }
        let off = k.key as i32 - base as i32;
        if (0..=28).contains(&off)
            && let Some(l) = typing_letter(off)
        {
            // Computer-keyboard letter near the top of the key.
            ui.text(
                Rect {
                    y: k.rect.y + 6.0,
                    h: 14.0,
                    ..k.rect
                },
                l,
                t.caption.size(10.5),
                if on { crate::util::ink_on(accent) } else { Color::hex(0x6B7584) },
                Align::Center,
            );
        }
        if !label.is_empty() {
            ui.text(
                Rect {
                    y: k.rect.bottom() - 20.0,
                    h: 16.0,
                    ..k.rect
                },
                &label,
                t.caption,
                if on { crate::util::ink_on(accent) } else { Color::hex(0x4A5362) },
                Align::Center,
            );
        }
    }
    for k in keys.iter().filter(|k| k.black) {
        let on = lit(k.key);
        ui.scene.rect(
            k.rect,
            2.0,
            if on {
                accent
            } else if hover == Some(k.key) {
                t.key_black.lighten(0.15)
            } else {
                t.key_black
            },
        );
        let off = k.key as i32 - base as i32;
        if (0..=28).contains(&off)
            && let Some(l) = typing_letter(off)
        {
            ui.text(
                Rect {
                    y: k.rect.bottom() - 20.0,
                    h: 14.0,
                    ..k.rect
                },
                l,
                t.caption.size(10.0),
                if on { crate::util::ink_on(accent) } else { Color::hex(0x8B95A5) },
                Align::Center,
            );
        }
    }
    ui.focus_ring(id, r, 4.0);
}

// ---- right rail ----

struct Rows {
    x: f32,
    w: f32,
    y: f32,
}

impl Rows {
    fn row(&mut self, h: f32) -> Rect {
        let r = Rect {
            x: self.x,
            y: self.y,
            w: self.w,
            h,
        };
        self.y += h;
        r
    }
    fn gap(&mut self, h: f32) {
        self.y += h;
    }
}

fn section(ui: &mut Ui, rows: &mut Rows, name: &str) {
    let t = ui.theme.clone();
    let r = rows.row(22.0);
    ui.scene.hline(r.x, r.right(), r.y, t.line);
    ui.text(
        Rect { y: r.y + 4.0, h: r.h - 4.0, ..r },
        name,
        t.label,
        t.text_dim,
        Align::Start,
    );
}

fn switch(ui: &mut Ui, id: W, r: Rect, label: &str, tip: &str, on: bool, color: Color) -> bool {
    let t = ui.theme.clone();
    let resp = ui.interact_node(
        id,
        r,
        FocusKind::Activate,
        Semantics::new(Role::Button).label(label).toggled(on),
    );
    ui.scene.rect(
        r,
        t.radius_s,
        if on {
            color
        } else if resp.hovered {
            t.control_hover
        } else {
            t.control
        },
    );
    ui.text(
        r,
        label,
        t.label,
        if on { crate::util::ink_on(color) } else { t.text_dim },
        Align::Center,
    );
    ui.focus_ring(id, r, t.radius_s);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    ui.tooltip(&resp, tip);
    resp.clicked
}

fn rail_content(
    ui: &mut Ui,
    rail: &mut Rect,
    ch: &peel_document::Channel,
    s: &mut Session,
    ws: &mut Workspace,
    prefs: &mut Prefs,
) {
    let t = ui.theme.clone();
    let mine = mappings_for(s, ch);
    let content_h = 330.0 + 34.0 * mine.len() as f32 + 200.0;
    let mut off = vec2(0.0, ws.inst.rail_scroll);
    if content_h > rail.h {
        ui.scroll_area(W::new("inst-rail"), *rail, vec2(rail.w, content_h), &mut off);
    } else {
        off.y = 0.0;
    }
    ws.inst.rail_scroll = off.y;
    let mut rows = Rows {
        x: rail.x,
        w: rail.w - if content_h > rail.h { 10.0 } else { 0.0 },
        y: rail.y - off.y,
    };

    section(ui, &mut rows, "Play");
    rows.gap(6.0);
    // Velocity.
    let mut row = rows.row(28.0);
    ui.text(row.cut_left(70.0), "Velocity", t.caption, t.text_faint, Align::Start);
    let val = row.cut_right(34.0);
    ui.text(
        val,
        &format!("{}", (ws.inst.velocity * 127.0).round() as i32),
        t.readout_small,
        t.text,
        Align::End,
    );
    let mut norm = ws.inst.velocity;
    let sl = ui.hslider(W::new("inst-velocity"), row, &mut norm, 0.8, col(ch.color));
    ui.name(&sl, "Velocity of mouse notes");
    ws.inst.velocity = norm.clamp(0.05, 1.0);
    rows.gap(4.0);
    // Octave.
    let mut row = rows.row(28.0);
    ui.text(row.cut_left(70.0), "Octave", t.caption, t.text_faint, Align::Start);
    let minus = ui.icon_button(W::new("inst-oct-down"), row.cut_left(30.0), icons::MINUS, false, None);
    ui.name(&minus, "Octave down");
    let label = format!("C{}", ws.octave);
    ui.text(row.cut_left(50.0), &label, t.readout_small, t.text, Align::Center);
    let plus = ui.icon_button(W::new("inst-oct-up"), row.cut_left(30.0), icons::PLUS, false, None);
    ui.name(&plus, "Octave up");
    if minus.clicked {
        ws.octave = (ws.octave - 1).max(0);
    }
    if plus.clicked {
        ws.octave = (ws.octave + 1).min(8);
    }
    rows.gap(4.0);
    // Computer keyboard and sustain.
    let mut row = rows.row(28.0);
    let half = (row.w - 6.0) * 0.5;
    let kb = row.cut_left(half);
    row.cut_left(6.0);
    if switch(
        ui,
        W::new("inst-keys"),
        kb,
        "Keyboard",
        "Play with the computer keyboard: Z to M, Q to P. - and = change octave.",
        ws.keys_enabled,
        t.accent,
    ) {
        ws.keys_enabled = !ws.keys_enabled;
    }
    if switch(
        ui,
        W::new("inst-sustain"),
        row,
        "Sustain",
        "Hold the sustain pedal for notes you play here",
        ws.inst.sustain,
        t.focus,
    ) {
        ws.inst.sustain = !ws.inst.sustain;
        s.handle_midi_event(MidiEvent::Sustain {
            channel: play_channel(s),
            down: ws.inst.sustain,
        });
    }
    rows.gap(10.0);

    section(ui, &mut rows, "MIDI");
    rows.gap(6.0);
    midi_status(ui, &mut rows, s, ws, prefs);
    rows.gap(6.0);
    mapper(ui, &mut rows, ch, &mine, s, ws, prefs);
}

/// Mappings (with their index in the session list) that act on this
/// instrument: its parameters, its gain and pan.
fn mappings_for(s: &Session, ch: &peel_document::Channel) -> Vec<(usize, MidiMapping)> {
    s.midi_control_mappings()
        .iter()
        .enumerate()
        .filter(|(_, m)| match &m.target {
            MidiControlTarget::DeviceParam { device, .. } => *device == ch.device.id,
            MidiControlTarget::ChannelGain { channel } | MidiControlTarget::ChannelPan { channel } => {
                *channel == ch.id
            }
            _ => false,
        })
        .map(|(i, m)| (i, m.clone()))
        .collect()
}

fn midi_status(ui: &mut Ui, rows: &mut Rows, s: &mut Session, ws: &mut Workspace, prefs: &mut Prefs) {
    let t = ui.theme.clone();
    let _ = ws;
    // Activity light and what is listening.
    let activity = s.midi_activity();
    let dt = ui.dt() as f64;
    let lit = {
        let st = ui.state::<(u64, f64)>(W::new("inst-midi-activity"));
        if st.0 != activity {
            *st = (activity, 0.0);
        }
        st.1 += dt;
        st.1 < 0.2 && activity > 0
    };
    if lit {
        ui.request_repaint();
    }
    let mut row = rows.row(24.0);
    let dot = row.cut_left(18.0);
    ui.scene
        .circle(dot.center(), 4.0, if lit { t.play } else { t.control_hover });
    let ports = s.midi_inputs();
    let text = if !s.midi_enabled() {
        "MIDI input off".to_string()
    } else if ports.is_empty() {
        "No MIDI device connected".to_string()
    } else if ports.len() == 1 {
        ports[0].clone()
    } else {
        format!("{} MIDI devices", ports.len())
    };
    let ports_btn = ui.button(W::new("inst-midi-ports"), row.cut_right(60.0), "Ports");
    ui.tooltip(&ports_btn, "Choose MIDI input devices");
    row.cut_right(6.0);
    let shown = ui.fit_text(&text, t.caption, row.w);
    ui.text(row, &shown, t.caption, t.text_dim, Align::Start);
    if ports_btn.clicked {
        ui.open_popup(W::new("settings-panel"), ports_btn.rect);
    }
    rows.gap(4.0);
    // Channel filter.
    let mut row = rows.row(28.0);
    ui.text(row.cut_left(70.0), "Channel", t.caption, t.text_faint, Align::Start);
    let cur = s.midi_channel_filter();
    let label = cur.map_or("All".to_string(), |c| format!("Channel {}", c + 1));
    let id = W::new("inst-midi-channel");
    let resp = ui.dropdown(id, row, &label, "MIDI channel");
    if resp.clicked {
        ui.open_popup(id.with("menu"), row);
    }
    let mut items = vec![MenuItem::new("All channels").checked(cur.is_none())];
    items.extend((0..16u8).map(|c| MenuItem::new(format!("Channel {}", c + 1)).checked(cur == Some(c))));
    if let Some(i) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
        let pick = if i == 0 { None } else { Some((i - 1) as u8) };
        match s.set_midi_channel_filter(pick) {
            Ok(()) => {
                prefs.midi_channel = pick;
                prefs.save();
            }
            Err(e) => s.notify(e, true),
        }
    }
    rows.gap(2.0);
    let note = rows.row(32.0);
    ui.text_wrapped(
        note.min(),
        note.w,
        "Notes, sustain pedal and controller (CC) knobs. Pitch bend and aftertouch are not used.",
        t.caption.size(10.5),
        t.text_faint,
    );
}

fn mapper(
    ui: &mut Ui,
    rows: &mut Rows,
    ch: &peel_document::Channel,
    mine: &[(usize, MidiMapping)],
    s: &mut Session,
    ws: &mut Workspace,
    prefs: &mut Prefs,
) {
    let t = ui.theme.clone();
    section(ui, rows, "Controllers");
    rows.gap(6.0);
    let desc = peel_devices::descriptor(&ch.device.type_id);
    // Existing mappings with live value feedback.
    if mine.is_empty() {
        let r = rows.row(20.0);
        ui.text(r, "No controllers mapped", t.caption, t.text_faint, Align::Start);
    }
    let mut remove = None;
    for (index, m) in mine {
        let mut row = rows.row(30.0);
        let x = ui.icon_button(
            W::new(("inst-unmap", *index)),
            row.cut_right(28.0).shrink2(0.0, 2.0),
            icons::X,
            false,
            None,
        );
        ui.name(&x, "Unmap");
        ui.tooltip(&x, "Unmap this controller");
        if x.clicked {
            remove = Some(*index);
        }
        let (name, value) = match &m.target {
            MidiControlTarget::DeviceParam { key, .. } => desc
                .and_then(|d| d.param_by_key(key))
                .map(|p| {
                    let v = ch.device.params.get(p.key).copied().unwrap_or(p.default);
                    (p.name.to_string(), s.param_text(ch.device.id, p, v))
                })
                .unwrap_or_else(|| (key.clone(), String::new())),
            MidiControlTarget::ChannelGain { .. } => ("Volume".to_string(), crate::util::format_db(ch.gain_db)),
            MidiControlTarget::ChannelPan { .. } => ("Pan".to_string(), crate::util::format_pan(ch.pan)),
            _ => (String::new(), String::new()),
        };
        let cc = format!(
            "CC {}{}",
            m.controller,
            m.channel.map_or(String::new(), |c| format!(" ch {}", c + 1))
        );
        ui.text(row.cut_left(64.0), &cc, t.readout_small, t.text, Align::Start);
        ui.text(row.cut_right(54.0), &value, t.readout_small, t.text_dim, Align::End);
        let shown = ui.fit_text(&name, t.caption, row.w);
        ui.text(row, &shown, t.caption, t.text_dim, Align::Start);
    }
    if let Some(i) = remove {
        // Forget it in preferences too: drop the saved copy first.
        if let Some(m) = s.midi_control_mappings().get(i).cloned() {
            prefs.midi_mappings.retain(|x| *x != m);
        }
        s.remove_midi_control_mapping(i);
        sync_prefs(s, prefs);
    }
    rows.gap(6.0);

    // Learn: pick a parameter, press Learn, move a control.
    let params: Vec<&'static peel_dsp::ParamDescriptor> = desc
        .map(|d| d.params.iter().filter(|p| p.automatable).collect())
        .unwrap_or_default();
    let mut row = rows.row(28.0);
    let pick_label = ws
        .inst
        .pick
        .as_deref()
        .and_then(|k| params.iter().find(|p| p.key == k))
        .map_or("Parameter".to_string(), |p| p.name.to_string());
    let learn_r = row.cut_right(70.0);
    row.cut_right(6.0);
    let id = W::new("inst-pick");
    let resp = ui.dropdown(id, row, &pick_label, "Parameter to control");
    if resp.clicked {
        ui.open_popup(id.with("menu"), row);
    }
    let items: Vec<MenuItem> = params
        .iter()
        .map(|p| MenuItem::new(p.name).checked(ws.inst.pick.as_deref() == Some(p.key)))
        .collect();
    if let Some(i) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
        ws.inst.pick = Some(params[i].key.to_string());
    }
    let waiting = matches!(s.midi_learn_status(), MidiLearnStatus::Waiting { .. });
    let target = ws.inst.pick.clone().map(|key| MidiControlTarget::DeviceParam {
        device: ch.device.id,
        key,
    });
    if waiting {
        if ui.button(W::new("inst-learn-cancel"), learn_r, "Cancel").clicked {
            s.cancel_midi_learn();
        }
    } else {
        let can = target.is_some();
        let b = ui.button(W::new("inst-learn"), learn_r, "Learn");
        ui.tooltip(
            &b,
            if can {
                "Then move a knob or fader on your controller"
            } else {
                "Choose a parameter first"
            },
        );
        if b.clicked
            && let Some(target) = target
            && let Err(e) = s.begin_midi_learn(target)
        {
            s.notify(e, true);
        }
    }
    rows.gap(4.0);
    let status = rows.row(36.0);
    let text = match s.midi_learn_status() {
        MidiLearnStatus::Idle => String::new(),
        MidiLearnStatus::Waiting { .. } => "Move a control on your MIDI device".to_string(),
        MidiLearnStatus::Learned { mapping } => format!(
            "Mapped CC {} on channel {}",
            mapping.controller,
            mapping.channel.map_or("any".to_string(), |c| (c + 1).to_string())
        ),
        MidiLearnStatus::Error { message } => message.clone(),
    };
    let err = matches!(s.midi_learn_status(), MidiLearnStatus::Error { .. });
    if !text.is_empty() {
        ui.text_wrapped(status.min(), status.w, &text, t.caption, if err { t.record } else { t.text_dim });
    }
    if matches!(s.midi_learn_status(), MidiLearnStatus::Waiting { .. }) {
        ui.request_repaint();
    }
    if let Some(e) = s.midi_control_error() {
        let r = rows.row(32.0);
        ui.text_wrapped(r.min(), r.w, e, t.caption, t.warn);
    }
    // A finished Learn is remembered in preferences (once).
    if matches!(s.midi_learn_status(), MidiLearnStatus::Learned { .. }) {
        sync_prefs(s, prefs);
    }
}

/// Stores the session's mappings in preferences, keeping the saved ones
/// that point at things this project does not have.
pub fn sync_prefs(s: &Session, prefs: &mut Prefs) {
    let p = s.project();
    let exists = |t: &MidiControlTarget| match t {
        MidiControlTarget::DeviceParam { device, .. } => peel_session::find_device(p, *device).is_some(),
        MidiControlTarget::TrackGain { track } | MidiControlTarget::TrackPan { track } => {
            p.mixer.track(*track).is_some()
        }
        MidiControlTarget::ChannelGain { channel } | MidiControlTarget::ChannelPan { channel } => {
            p.channel(*channel).is_some()
        }
    };
    let merged = merged_mappings(&prefs.midi_mappings, s.midi_control_mappings(), exists);
    if merged != prefs.midi_mappings {
        prefs.midi_mappings = merged;
        prefs.save();
    }
}

/// Mappings to save: everything active now, plus saved ones whose target
/// belongs to some other project.
pub fn merged_mappings(
    saved: &[MidiMapping],
    current: &[MidiMapping],
    exists: impl Fn(&MidiControlTarget) -> bool,
) -> Vec<MidiMapping> {
    let mut out: Vec<MidiMapping> = saved
        .iter()
        .filter(|m| !exists(&m.target) && !current.contains(m))
        .cloned()
        .collect();
    out.extend(current.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect { x: 0.0, y: 0.0, w: 840.0, h: 120.0 }
    }

    #[test]
    fn four_octaves_have_29_white_keys_and_black_keys_on_top() {
        let keys = piano_layout(area(), 48, 4);
        assert_eq!(keys.iter().filter(|k| !k.black).count(), 29);
        assert_eq!(keys.iter().filter(|k| k.black).count(), 20);
        assert_eq!(keys.iter().filter(|k| !k.black).map(|k| k.key).min(), Some(48));
        assert_eq!(keys.iter().map(|k| k.key).max(), Some(96));
        let first_black = keys.iter().position(|k| k.black).unwrap();
        assert!(keys[first_black..].iter().all(|k| k.black), "black keys are drawn last");
    }

    #[test]
    fn hit_tests_prefer_black_keys_and_cover_the_whole_board() {
        let keys = piano_layout(area(), 60, 3);
        let c_sharp = keys.iter().find(|k| k.key == 61).unwrap();
        assert_eq!(key_at(&keys, c_sharp.rect.center()), Some(61));
        // Below the black key's reach the white key answers.
        let low = vec2(c_sharp.rect.center().x, 110.0);
        assert!(matches!(key_at(&keys, low), Some(60 | 62)));
        assert_eq!(key_at(&keys, vec2(-5.0, 10.0)), None);
        // Every point of the board maps to some key.
        for i in 0..40 {
            let p = vec2(1.0 + i as f32 * 20.9, 60.0);
            assert!(key_at(&keys, p).is_some(), "{p:?}");
        }
    }

    #[test]
    fn typing_letters_follow_the_computer_keyboard_rows() {
        assert_eq!(typing_letter(0), Some("Z"));
        assert_eq!(typing_letter(11), Some("M"));
        assert_eq!(typing_letter(12), Some("Q"));
        assert_eq!(typing_letter(28), Some("P"));
        assert_eq!(typing_letter(29), None);
    }

    #[test]
    fn saved_mappings_for_other_projects_survive_a_sync() {
        let m = |cc: u8, device: Id| MidiMapping {
            controller: cc,
            channel: None,
            target: MidiControlTarget::DeviceParam { device, key: "x".into() },
            min: 0.0,
            max: 1.0,
        };
        let (here, elsewhere) = (Id::new(), Id::new());
        let saved = vec![m(1, here), m(2, elsewhere)];
        let current = vec![m(1, here), m(3, here)];
        let merged = merged_mappings(&saved, &current, |t| {
            matches!(t, MidiControlTarget::DeviceParam { device, .. } if *device == here)
        });
        let ccs: Vec<u8> = merged.iter().map(|m| m.controller).collect();
        assert_eq!(ccs, vec![2, 1, 3]);
    }

    #[test]
    fn the_range_stops_at_the_top_of_midi() {
        let keys = piano_layout(area(), 108, 4);
        assert!(keys.iter().all(|k| k.key <= 127));
    }
}
