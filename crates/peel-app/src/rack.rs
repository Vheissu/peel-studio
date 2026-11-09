//! Channel Rack: one row per instrument with a step grid for the selected
//! pattern. Steps are ordinary notes, so the Piano Roll edits the same data.

use std::path::Path;

use peel_document::{Channel, Id, Note, PALETTE, Time};
use peel_session::{PlayMode, Session, template};
use peel_ui::Id as W;
use peel_ui::{Align, Color, CursorIcon, MenuItem, Rect, Ui, icons, vec2};

use crate::browser::BrowserItem;
use crate::studio::current_pattern;
use crate::util::{col, db_to_fader, fader_to_db, ink_on};
use crate::workspace::{DockTab, Focus, Workspace};

const ROW_H: f32 = 34.0;
const ROW_GAP: f32 = 4.0;
const GRAPH_H: f32 = 64.0;
const STEP: Time = Time::new_const(1, 4);

#[derive(Default)]
struct PaintState {
    active: bool,
    on: bool,
    gesture: u64,
}

/// A note that the step grid can represent: on a 16th, the channel's root
/// key and one step long.
fn is_step(n: &Note, root: u8) -> bool {
    n.key == root && n.length == STEP && n.start.rem_euclid(STEP).is_zero()
}

pub fn draw(ui: &mut Ui, r: Rect, tools: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let pid = current_pattern(s);
    let Some(pattern) = s.project().pattern(pid).cloned() else {
        return;
    };
    let steps_total = (pattern.length.as_f64() * 4.0).round().max(1.0) as usize;

    {
        // Velocity graph toggle at the left of the header tools; length and
        // swing take the rest from the right.
        let mut row = tools;
        let toggle = row.cut_left(30.0);
        row.cut_left(6.0);
        header_tools(ui, row, s, pid, &pattern);
        let b = ui.icon_button(
            W::new("rack-graph"),
            toggle,
            icons::EQUALIZER,
            ws.rack_graph,
            None,
        );
        ui.name(&b, "Velocity graph");
        ui.tooltip(&b, "Velocity graph for the selected channel");
        if b.clicked {
            ws.rack_graph = !ws.rack_graph;
        }
    }

    let body = r.shrink2(10.0, 4.0);
    let channels: Vec<Channel> = s.project().channels.clone();
    let graph_h = if ws.rack_graph {
        GRAPH_H + ROW_GAP
    } else {
        0.0
    };
    let content_h = channels.len() as f32 * (ROW_H + ROW_GAP) + ROW_H + 8.0 + graph_h;
    let mut offset = vec2(0.0, ws.rack_scroll);
    ui.scroll_area(
        W::new("rack-scroll"),
        body,
        vec2(body.w, content_h),
        &mut offset,
    );
    ws.rack_scroll = offset.y;

    // Drops onto the rack add channels (instruments and samples only).
    let accepts = |i: &BrowserItem| match i {
        BrowserItem::Device(id) | BrowserItem::Preset(id, _) => {
            peel_devices::descriptor(id).is_some_and(|d| d.kind == peel_dsp::DeviceKind::Instrument)
        }
        BrowserItem::Sample(_) => true,
        BrowserItem::Pattern(_) => false,
        BrowserItem::Plugin { instrument, .. } => *instrument,
    };
    let hovering_drop = ui.drop_hover::<BrowserItem>(body).is_some_and(accepts);
    if hovering_drop && let Some((item, _)) = ui.take_drop::<BrowserItem>(body) {
        let added = match item {
            BrowserItem::Device(type_id) => add_instrument(s, type_id),
            BrowserItem::Preset(type_id, preset) => add_instrument_preset(s, type_id, Some(preset)),
            BrowserItem::Sample(path) => add_sample_channel(s, &path),
            BrowserItem::Pattern(_) => None,
            BrowserItem::Plugin { type_id, name, .. } => add_plugin_channel(s, &type_id, &name),
        };
        if let Some(id) = added {
            select(ws, id);
        }
    }
    for path in ui.take_dropped_files(body) {
        if peel_media::is_audio_file(&path)
            && let Some(id) = add_sample_channel(s, &path)
        {
            select(ws, id);
        }
    }

    // Column layout.
    let name_w = 132.0;
    let ctrl_w = 28.0 * 2.0 + 8.0;
    let lead = 22.0 + 6.0 + name_w + 8.0 + ctrl_w + 12.0;
    let avail = (body.w - lead - 8.0).max(80.0);
    let beats = steps_total.div_ceil(4);
    let group_gap = 6.0;
    let step_gap = 3.0;
    let step_w = ((avail
        - group_gap * (beats as f32 - 1.0)
        - step_gap * (steps_total as f32 - beats as f32))
        / steps_total as f32)
        .clamp(14.0, 30.0);

    let playing_step = if s.is_playing() && matches!(s.mode(), PlayMode::Pattern(p) if p == pid) {
        let b = s.position().rem_euclid(pattern.length.as_f64());
        Some((b * 4.0).floor() as usize)
    } else {
        None
    };

    ui.scene.push_clip(body);
    let paint_id = W::new("rack-paint");
    let mut y = body.y - offset.y;
    for ch in &channels {
        let row = Rect {
            x: body.x,
            y,
            w: body.w,
            h: ROW_H,
        };
        y += ROW_H + ROW_GAP;
        let selected = ws.selected_channel == Some(ch.id);
        let graph_row = (selected && ws.rack_graph).then(|| {
            let g = Rect {
                x: body.x,
                y,
                w: body.w,
                h: GRAPH_H,
            };
            y += GRAPH_H + ROW_GAP;
            g
        });
        if row.bottom() < body.y || row.y > body.bottom() {
            continue;
        }
        if selected {
            ui.scene
                .rect(row.expand(2.0), t.radius_m, t.raised.alpha(0.55));
        }
        let mut cells = row;
        // Mute LED.
        let led_r = cells.cut_left(22.0);
        let led = ui.led(W::new(("rack-led", ch.id)), led_r, !ch.muted, t.play);
        ui.tooltip(&led, if ch.muted { "Unmute" } else { "Mute" });
        if led.clicked {
            let cid = ch.id;
            let label = if ch.muted {
                "Unmute channel"
            } else {
                "Mute channel"
            };
            s.doc.edit(label, |p| {
                if let Some(c) = p.channel_mut(cid) {
                    c.muted = !c.muted;
                }
            });
        }
        cells.cut_left(6.0);
        channel_button(ui, cells.cut_left(name_w), ch, selected, s, ws);
        cells.cut_left(8.0);
        // Volume and pan.
        let mut vol = db_to_fader(ch.gain_db);
        let vr = cells.cut_left(28.0);
        let vresp = ui.knob(
            W::new(("rack-vol", ch.id)),
            vr,
            &mut vol,
            0.8,
            col(ch.color),
            false,
        );
        ui.name(&vresp, &format!("{} volume", ch.name));
        ui.tooltip(
            &vresp,
            &format!("Volume {}", crate::util::format_db(ch.gain_db)),
        );
        if vresp.changed {
            let cid = ch.id;
            s.doc
                .edit_gesture(vresp.id.0, "Change channel volume", |p| {
                    if let Some(c) = p.channel_mut(cid) {
                        c.gain_db = fader_to_db(vol);
                    }
                });
        }
        cells.cut_left(8.0);
        let mut pan = (ch.pan + 1.0) * 0.5;
        let presp = ui.knob(
            W::new(("rack-pan", ch.id)),
            cells.cut_left(28.0),
            &mut pan,
            0.5,
            t.text_dim,
            true,
        );
        ui.name(&presp, &format!("{} pan", ch.name));
        ui.tooltip(&presp, "Pan");
        if presp.changed {
            let cid = ch.id;
            s.doc.edit_gesture(presp.id.0, "Change channel pan", |p| {
                if let Some(c) = p.channel_mut(cid) {
                    c.pan = (pan * 2.0 - 1.0).clamp(-1.0, 1.0);
                }
            });
        }
        if vresp.released || presp.released {
            s.doc.end_gesture();
        }
        cells.cut_left(12.0);

        let notes: Vec<Note> = pattern.notes_for(ch.id).copied().collect();
        let as_steps = notes.iter().all(|n| is_step(n, ch.root_key));
        let grid = Rect {
            w: (steps_total as f32 * (step_w + step_gap) + beats as f32 * (group_gap - step_gap))
                .min(cells.w),
            ..cells
        };
        if as_steps {
            step_row(
                ui,
                grid,
                ch,
                &notes,
                steps_total,
                step_w,
                step_gap,
                group_gap,
                playing_step,
                paint_id,
                s,
                pid,
            );
            if let Some(g) = graph_row {
                let grid_g = Rect {
                    y: g.y,
                    h: g.h,
                    ..grid
                };
                velocity_graph(
                    ui,
                    g,
                    grid_g,
                    ch,
                    &notes,
                    steps_total,
                    step_w,
                    step_gap,
                    group_gap,
                    s,
                    pid,
                );
            }
        } else {
            note_preview(ui, grid, ch, &notes, pattern.length, ws);
        }
    }

    // Add-channel row.
    let add_r = Rect {
        x: body.x + 28.0,
        y,
        w: name_w,
        h: ROW_H - 4.0,
    };
    let add = ui.interact(W::new("rack-add"), add_r);
    let hot = ui.animate_bool(W::new("rack-add-hot"), add.hovered);
    ui.scene
        .rect(add_r, t.radius_m, t.raised.alpha(0.3 + hot * 0.5));
    let mut ar = add_r.shrink2(10.0, 0.0);
    ui.icon(ar.cut_left(16.0), icons::PLUS, 14.0, t.text_dim);
    ar.cut_left(6.0);
    ui.text(ar, "Add", t.label, t.text_dim, Align::Start);
    if add.clicked {
        ui.open_popup(W::new("rack-add-menu"), add_r);
    }
    if add.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    ui.scene.pop_clip();

    if hovering_drop {
        ui.scene
            .outline(body.expand(2.0), t.radius_m, 2.0, t.focus.alpha(0.6));
    }

    let instruments: Vec<(&'static str, &'static str)> = peel_devices::instruments()
        .map(|d| (d.type_id, d.name))
        .collect();
    let mut items: Vec<MenuItem> = instruments.iter().map(|(_, n)| MenuItem::new(*n)).collect();
    items.push(MenuItem::Separator);
    items.push(MenuItem::new("Sampler from file…"));
    if let Some(i) = ui.menu(
        W::new("rack-add-menu"),
        vec2(add_r.x, add_r.bottom() + 4.0),
        &items,
    ) {
        if i < instruments.len() {
            if let Some(id) = add_instrument(s, instruments[i].0) {
                select(ws, id);
            }
        } else if let Some(path) =
            crate::dialogs::open(&[("Audio", &["wav", "aif", "aiff", "flac", "mp3", "ogg"])])
            && let Some(id) = add_sample_channel(s, &path)
        {
            select(ws, id);
        }
    }

    if !ui.input.down[0] {
        let st = ui.state::<PaintState>(paint_id);
        if st.active {
            st.active = false;
            s.doc.end_gesture();
        }
    }
}

fn select(ws: &mut Workspace, id: Id) {
    ws.selected_channel = Some(id);
    ws.focus = Some(Focus::Channel(id));
}

fn header_tools(
    ui: &mut Ui,
    tools: Rect,
    s: &mut Session,
    pid: Id,
    pattern: &peel_document::Pattern,
) {
    let t = ui.theme.clone();
    let mut row = tools;
    // Pattern length: shorter, the length, longer, in one well.
    let bars = (pattern.length.as_f64() / 4.0).max(0.25);
    let well = row.cut_right(28.0 * 2.0 + 56.0 + 4.0);
    ui.well(well);
    let mut len_row = well.shrink(2.0);
    let plus = ui.icon_button(
        W::new("rack-len-plus"),
        len_row.cut_right(28.0),
        icons::PLUS,
        false,
        None,
    );
    let label = if bars == 1.0 {
        "1 bar".to_string()
    } else if bars.fract() == 0.0 {
        format!("{bars:.0} bars")
    } else {
        format!("{bars} bars")
    };
    ui.text(
        len_row.cut_right(56.0),
        &label,
        t.label,
        t.text_dim,
        Align::Center,
    );
    let minus = ui.icon_button(
        W::new("rack-len-minus"),
        len_row.cut_right(28.0),
        icons::MINUS,
        false,
        None,
    );
    ui.name(&plus, "Lengthen pattern");
    ui.name(&minus, "Shorten pattern");
    ui.tooltip(&plus, "Lengthen pattern");
    ui.tooltip(&minus, "Shorten pattern");
    let mut new_len = None;
    if plus.clicked {
        new_len = Some((pattern.length + Time::beats(4)).min(Time::beats(64)));
    }
    if minus.clicked && pattern.length > Time::beats(4) {
        new_len = Some(pattern.length - Time::beats(4));
    } else if minus.clicked {
        new_len = Some(Time::beats(2).max(pattern.length.div_int(2)));
    }
    if let Some(len) = new_len {
        s.doc.edit("Change pattern length", |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                pt.length = len;
            }
        });
    }
    row.cut_right(8.0);
    // Swing: the knob, its label to the left. The label is a hover target
    // of its own (showing the amount) and goes only when there is no room
    // for it beside the velocity toggle; the knob keeps its name and value
    // for tooltips and screen readers either way.
    let mut swing = s.project().swing;
    let kr = row.cut_right(26.0);
    let k = ui.knob(W::new("rack-swing"), kr, &mut swing, 0.0, t.accent, false);
    let amount = format!("Swing {:.0}%", swing * 100.0);
    ui.name(&k, "Swing");
    ui.value_text(&k, &format!("{:.0}%", swing * 100.0));
    ui.tooltip(&k, &amount);
    if k.changed {
        s.doc
            .edit_gesture(k.id.0, "Change swing", |p| p.swing = swing);
    }
    if k.released {
        s.doc.end_gesture();
    }
    let label_w = ui.measure("Swing", t.caption).x + 2.0;
    if row.w >= label_w + 6.0 {
        row.cut_right(6.0);
        let lr = row.cut_right(label_w);
        let lresp = ui.interact(W::new("rack-swing-label"), lr);
        ui.text(lr, "Swing", t.caption, t.text_faint, Align::End);
        ui.tooltip(&lresp, &amount);
    }
}

fn channel_button(
    ui: &mut Ui,
    r: Rect,
    ch: &Channel,
    selected: bool,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let id = W::new(("rack-name", ch.id));
    let resp = ui.interact_button(id, r, &ch.name);
    let hot = ui.animate_bool(id, resp.hovered);
    // One step above the panel; the selected channel steps up again.
    let base = if selected {
        t.control_hover
    } else {
        t.raised.mix(t.control, hot)
    };
    ui.scene.rect(r, t.radius_m, base);
    let c = col(ch.color);
    let activity = s.take_channel_activity(ch.id);
    let glow = ui.animate(W::new(("rack-act", ch.id)), activity.min(1.0), 10.0);
    ui.scene.rect_ex(
        Rect { w: 4.0, ..r },
        [t.radius_m, 0.0, 0.0, t.radius_m],
        c,
        0.0,
        Color::TRANSPARENT,
    );
    if glow > 0.01 {
        ui.scene.rect(r, t.radius_m, c.alpha(glow * 0.22));
    }
    let name = ui.fit_text(&ch.name, t.label, r.w - 18.0);
    ui.text(
        r.shrink2(12.0, 0.0),
        &name,
        t.label,
        if selected {
            t.text
        } else {
            t.text.mix(t.text_dim, 0.25)
        },
        Align::Start,
    );
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    let dev = peel_devices::descriptor(&ch.device.type_id).map_or("Missing device", |d| d.name);
    ui.tooltip(&resp, dev);
    if resp.pressed {
        select(ws, ch.id);
    }
    if resp.double_clicked {
        ws.dock = DockTab::PianoRoll;
    }
    let menu_id = W::new(("rack-menu", ch.id));
    if resp.secondary_clicked {
        select(ws, ch.id);
        ui.open_popup(menu_id, r);
    }
    let mut items = vec![
        MenuItem::new("Piano roll").shortcut("F7"),
        MenuItem::new("Instrument"),
        MenuItem::new("Rename channel"),
        MenuItem::new("Clone channel"),
        MenuItem::new("Clear notes in pattern"),
        MenuItem::Separator,
        MenuItem::Heading("Color".into()),
    ];
    let first_color = items.len();
    for (i, pc) in PALETTE.iter().enumerate() {
        items.push(
            MenuItem::new(crate::util::COLOR_NAMES[i])
                .swatch(col(*pc))
                .checked(*pc == ch.color),
        );
    }
    items.push(MenuItem::Separator);
    let delete_idx = items.len();
    items.push(MenuItem::new("Delete channel"));
    if let Some(i) = ui.menu(menu_id, vec2(r.x, r.bottom() + 4.0), &items) {
        let cid = ch.id;
        match i {
            0 => ws.dock = DockTab::PianoRoll,
            1 => crate::instrument_editor::open(s, ws, cid),
            2 => crate::rename::open(ui, W::new(("rename-channel-popup", cid)), r, &ch.name),
            3 => clone_channel(s, cid),
            4 => {
                let pid = current_pattern(s);
                s.doc.edit("Clear notes", |p| {
                    if let Some(pt) = p.pattern_mut(pid) {
                        pt.notes.retain(|n| n.channel != cid);
                    }
                });
            }
            i if i >= first_color && i < first_color + PALETTE.len() => {
                let color = PALETTE[i - first_color];
                s.doc.edit("Recolor channel", |p| {
                    if let Some(c) = p.channel_mut(cid) {
                        c.color = color;
                        let track = c.track;
                        if let Some(t) = p.mixer.track_mut(track) {
                            t.color = color;
                        }
                    }
                });
            }
            i if i == delete_idx => delete_channel(s, ws, cid),
            _ => {}
        }
    }
    if let Some(name) = crate::rename::show(ui, W::new(("rename-channel-popup", ch.id))) {
        let cid = ch.id;
        s.doc.edit("Rename channel", |p| {
            if let Some(c) = p.channel_mut(cid) {
                c.name = name.clone();
                let track = c.track;
                if let Some(t) = p.mixer.track_mut(track) {
                    t.name = name;
                }
            }
        });
    }
    // Tiny tag showing the instrument in a strong ink on selection.
    let _ = ink_on(c);
}

#[allow(clippy::too_many_arguments)]
fn step_row(
    ui: &mut Ui,
    grid: Rect,
    ch: &Channel,
    notes: &[Note],
    steps: usize,
    step_w: f32,
    step_gap: f32,
    group_gap: f32,
    playing: Option<usize>,
    paint_id: W,
    s: &mut Session,
    pid: Id,
) {
    let t = ui.theme.clone();
    let c = col(ch.color);
    let mut x = grid.x;
    for i in 0..steps {
        if i > 0 {
            x += if i % 4 == 0 { group_gap } else { step_gap };
        }
        let cell = Rect {
            x,
            y: grid.y + 2.0,
            w: step_w,
            h: grid.h - 4.0,
        };
        x += step_w;
        if cell.x > grid.right() + 1.0 {
            break;
        }
        let pos = STEP.mul_int(i as i64);
        let note = notes.iter().find(|n| n.start == pos);
        let on = note.is_some();
        let id = W::new(("step", ch.id, i));
        let resp = ui.interact_button(id, cell, &format!("{} step {}", ch.name, i + 1));
        let beat_odd = (i / 4) % 2 == 1;
        let off_fill = if beat_odd {
            t.raised.lighten(0.035)
        } else {
            t.raised
        };
        let hot = resp.hovered as i32 as f32;
        if on {
            let vel = note.map_or(1.0, |n| n.velocity);
            let fill = c.mix(t.raised, (1.0 - vel) * 0.55);
            ui.scene.rect(cell, t.radius_s, fill.lighten(hot * 0.08));
            ui.scene.rect(
                Rect {
                    h: 2.0,
                    ..cell.shrink2(3.0, 0.0)
                }
                .translate(vec2(0.0, 3.0)),
                1.0,
                Color::WHITE.alpha(0.18),
            );
        } else {
            ui.scene
                .rect(cell, t.radius_s, off_fill.lighten(hot * 0.05));
        }
        if playing == Some(i) {
            ui.scene.rect(
                cell,
                t.radius_s,
                Color::WHITE.alpha(if on { 0.28 } else { 0.10 }),
            );
        }
        if resp.hovered {
            ui.set_cursor(CursorIcon::Pointer);
        }
        // Paint: the first step touched decides on/off for the whole drag.
        let pointer_inside = ui.pointer().is_some_and(|p| cell.contains(p));
        let (paint_active, paint_on, gesture) = {
            let st = ui.state::<PaintState>(paint_id);
            (st.active, st.on, st.gesture)
        };
        if resp.pressed {
            let want = !on;
            let gesture = id.0 ^ 0x5eed;
            {
                let st = ui.state::<PaintState>(paint_id);
                st.active = true;
                st.on = want;
                st.gesture = gesture;
            }
            set_step(s, pid, ch, pos, want, gesture);
            if want {
                s.live_note(ch.id, ch.root_key, 0.8, true);
                s.live_note(ch.id, ch.root_key, 0.0, false);
            }
        } else if paint_active && ui.input.down[0] && pointer_inside && on != paint_on {
            set_step(s, pid, ch, pos, paint_on, gesture);
        }
        // Keyboard or assistive activation (no mouse press behind it).
        if resp.clicked && !paint_active && !resp.pressed {
            set_step(s, pid, ch, pos, !on, id.0 ^ 0x6b);
            s.doc.end_gesture();
        }
        if resp.secondary_clicked && on {
            set_step(s, pid, ch, pos, false, id.0 ^ 0xdead);
            s.doc.end_gesture();
        }
        // Wheel over a lit step adjusts its velocity.
        if resp.hovered && on && ui.input.scroll.y != 0.0 && ui.input.modifiers.alt {
            let dv = ui.input.scroll.y / 400.0;
            ui.input.scroll.y = 0.0;
            let cid = ch.id;
            s.doc.edit_gesture(id.0 ^ 0x7e1, "Change velocity", |p| {
                if let Some(pt) = p.pattern_mut(pid)
                    && let Some(n) = pt
                        .notes
                        .iter_mut()
                        .find(|n| n.channel == cid && n.start == pos)
                {
                    n.velocity = (n.velocity + dv).clamp(0.05, 1.0);
                }
            });
        }
    }
}

fn set_step(s: &mut Session, pid: Id, ch: &Channel, pos: Time, on: bool, gesture: u64) {
    let cid = ch.id;
    let key = ch.root_key;
    let label = if on { "Add steps" } else { "Remove steps" };
    s.doc.edit_gesture(gesture, label, |p| {
        if let Some(pt) = p.pattern_mut(pid) {
            let exists = pt.notes.iter().any(|n| n.channel == cid && n.start == pos);
            if on && !exists {
                pt.notes.push(Note::new(cid, pos, STEP, key, 0.8));
                pt.sort_notes();
            } else if !on {
                pt.notes.retain(|n| !(n.channel == cid && n.start == pos));
            }
        }
    });
}

fn note_preview(
    ui: &mut Ui,
    grid: Rect,
    ch: &Channel,
    notes: &[Note],
    length: Time,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let r = grid.shrink2(0.0, 2.0);
    let id = W::new(("rack-preview", ch.id));
    let resp = ui.interact(id, r);
    let hot = ui.animate_bool(id, resp.hovered);
    ui.scene.rect(r, t.radius_s, t.raised.lighten(hot * 0.03));
    let (lo, hi) = notes
        .iter()
        .fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
    let span = (hi as f32 - lo as f32 + 1.0).max(8.0);
    let len = length.as_f64().max(0.25) as f32;
    let c = col(ch.color);
    ui.scene.push_clip(r);
    for beat in 1..(len.ceil() as i32) {
        let x = r.x + r.w * beat as f32 / len;
        ui.scene.vline(
            x,
            r.y + 3.0,
            r.bottom() - 3.0,
            if beat % 4 == 0 {
                t.grid_bar
            } else {
                t.grid_beat
            },
        );
    }
    for n in notes {
        let x0 = r.x + r.w * n.start.as_f64() as f32 / len;
        let w = (r.w * n.length.as_f64() as f32 / len).max(2.0);
        let y = r.bottom() - 4.0 - (r.h - 10.0) * ((n.key - lo) as f32 + 0.5) / span;
        ui.scene.rect(
            Rect {
                x: x0,
                y: y - 1.5,
                w,
                h: 3.0,
            },
            1.5,
            c,
        );
    }
    ui.scene.pop_clip();
    ui.tooltip(&resp, "Open in the piano roll");
    if resp.clicked {
        ws.selected_channel = Some(ch.id);
        ws.focus = Some(Focus::Channel(ch.id));
        ws.dock = DockTab::PianoRoll;
    }
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
}

// ---- channel operations ----

/// Adds a native instrument channel routed to a new mixer track.
pub fn add_instrument(s: &mut Session, type_id: &str) -> Option<Id> {
    add_instrument_preset(s, type_id, None)
}

/// Adds an instrument channel with a factory preset applied.
pub fn add_instrument_preset(s: &mut Session, type_id: &str, preset: Option<&str>) -> Option<Id> {
    if crate::util::mixer_full(s) {
        return None;
    }
    let desc = peel_devices::descriptor(type_id)?;
    let dev = template::device(type_id, preset);
    let name = preset
        .filter(|p| {
            !peel_devices::presets(type_id)
                .first()
                .is_some_and(|f| f.name == *p)
        })
        .unwrap_or(desc.name)
        .to_string();
    let mut new_id = None;
    s.doc.edit(format!("Add {name}"), |p| {
        let n = p.channels.len();
        new_id = Some(template::add_channel(p, &name, n * 5 + 2, dev, 60));
    });
    new_id
}

/// Adds a channel playing an installed plugin instrument.
pub fn add_plugin_channel(s: &mut Session, type_id: &str, name: &str) -> Option<Id> {
    if crate::util::mixer_full(s) {
        return None;
    }
    let dev = peel_document::DeviceInstance::new(type_id, 1);
    let mut new_id = None;
    s.doc.edit(format!("Add {name}"), |p| {
        let n = p.channels.len();
        new_id = Some(template::add_channel(p, name, n * 5 + 2, dev, 60));
    });
    new_id
}

/// Adds a sampler channel playing `path`.
pub fn add_sample_channel(s: &mut Session, path: &Path) -> Option<Id> {
    if crate::util::mixer_full(s) {
        return None;
    }
    let asset = match s.reference_audio_file(path) {
        Ok(a) => a,
        Err(e) => {
            s.notify(format!("Could not read {}: {e}", path.display()), true);
            return None;
        }
    };
    let mut dev = template::device("peel.sampler", None);
    dev.sample = Some(asset.id);
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Sample".into());
    let mut new_id = None;
    s.doc.edit(format!("Add {name}"), |p| {
        if p.asset(asset.id).is_none() {
            p.assets.push(asset.clone());
        }
        let n = p.channels.len();
        new_id = Some(template::add_channel(p, &name, n * 5 + 2, dev, 60));
    });
    new_id
}

fn clone_channel(s: &mut Session, cid: Id) {
    if crate::util::mixer_full(s) {
        return;
    }
    let Some(src) = s.project().channel(cid).cloned() else {
        return;
    };
    s.doc.edit(format!("Clone {}", src.name), |p| {
        let mut dev = src.device.clone();
        dev.id = Id::new();
        let n = p.channels.len();
        let id = template::add_channel(p, &format!("{} 2", src.name), n * 5 + 2, dev, src.root_key);
        if let Some(c) = p.channel_mut(id) {
            c.color = src.color;
            c.gain_db = src.gain_db;
            c.pan = src.pan;
        }
    });
}

pub fn delete_channel(s: &mut Session, ws: &mut Workspace, cid: Id) {
    let Some(ch) = s.project().channel(cid).cloned() else {
        return;
    };
    s.doc.edit(format!("Delete {}", ch.name), |p| {
        p.channels.retain(|c| c.id != cid);
        for pt in p.patterns.iter_mut() {
            if pt.uses_channel(cid) {
                std::sync::Arc::make_mut(pt)
                    .notes
                    .retain(|n| n.channel != cid);
            }
        }
        // Remove the channel's private mixer track if nothing else uses it.
        let used = p.channels.iter().any(|c| c.track == ch.track);
        if !used && p.mixer.tracks.first().is_some_and(|m| m.id != ch.track) {
            p.mixer.tracks.retain(|t| t.id != ch.track);
            for t in p.mixer.tracks.iter_mut() {
                if t.output == Some(ch.track) {
                    t.output = None;
                }
            }
        }
    });
    if ws.selected_channel == Some(cid) {
        ws.selected_channel = s.project().channels.first().map(|c| c.id);
        ws.focus = ws.selected_channel.map(Focus::Channel);
    }
}

/// Velocity bars under the selected channel's steps; drag across to draw.
#[allow(clippy::too_many_arguments)]
fn velocity_graph(
    ui: &mut Ui,
    row: Rect,
    grid: Rect,
    ch: &Channel,
    notes: &[Note],
    steps: usize,
    step_w: f32,
    step_gap: f32,
    group_gap: f32,
    s: &mut Session,
    pid: Id,
) {
    let t = ui.theme.clone();
    let c = col(ch.color);
    ui.scene.rect(row, t.radius_m, t.bg.mix(t.panel, 0.5));
    ui.text(
        Rect {
            x: row.x + 28.0,
            w: 132.0,
            ..row
        },
        "Velocity",
        t.caption,
        t.text_faint,
        Align::Start,
    );
    let inner = grid.shrink2(0.0, 6.0);
    let id = W::new(("rack-velocity", ch.id));
    let resp = ui.interact(id, inner);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Crosshair);
    }
    let mut x = grid.x;
    let mut cells = Vec::with_capacity(steps);
    for i in 0..steps {
        if i > 0 {
            x += if i % 4 == 0 { group_gap } else { step_gap };
        }
        cells.push(Rect {
            x,
            y: inner.y,
            w: step_w,
            h: inner.h,
        });
        x += step_w;
    }
    for (i, cell) in cells.iter().enumerate() {
        let pos = STEP.mul_int(i as i64);
        ui.scene.rect(*cell, 2.0, t.raised.alpha(0.5));
        if let Some(n) = notes.iter().find(|n| n.start == pos) {
            let h = cell.h * n.velocity;
            ui.scene.rect(
                Rect {
                    y: cell.bottom() - h,
                    h,
                    ..*cell
                },
                2.0,
                c.alpha(0.85),
            );
        }
    }
    if (resp.pressed || resp.dragging)
        && let Some(p) = ui.pointer()
        && let Some(i) = cells
            .iter()
            .position(|r| p.x >= r.x - step_gap * 0.5 && p.x < r.right() + step_gap * 0.5)
    {
        let pos = STEP.mul_int(i as i64);
        if notes.iter().any(|n| n.start == pos) {
            let vel = ((inner.bottom() - p.y) / inner.h).clamp(0.05, 1.0);
            let cid = ch.id;
            s.doc.edit_gesture(id.0, "Change velocity", |p| {
                if let Some(pt) = p.pattern_mut(pid)
                    && let Some(n) = pt
                        .notes
                        .iter_mut()
                        .find(|n| n.channel == cid && n.start == pos)
                {
                    n.velocity = vel;
                }
            });
        }
    }
    if resp.released {
        s.doc.end_gesture();
    }
}
