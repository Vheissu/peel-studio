//! Inspector: properties of the focused channel or mixer track, including
//! full device panels generated from parameter descriptors.

use peel_document::{DeviceInstance, Id};
use peel_dsp::{DeviceDescriptor, ParamDescriptor};
use peel_session::Session;
use peel_ui::Id as W;
use peel_ui::{Align, Color, CursorIcon, MenuItem, Rect, Ui, icons, vec2};

use crate::util::{col, db_to_fader, fader_to_db, format_db, note_name};
use crate::workspace::{Focus, Workspace};

const CELL_W: f32 = 64.0;
const CELL_H: f32 = 66.0;

#[derive(Default)]
struct WaveCache {
    asset: Option<Id>,
    cols: Vec<(f32, f32)>,
}

pub fn draw(ui: &mut Ui, r: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    ui.scene.rect(r, t.radius_l, t.panel);
    let body = r.shrink(12.0);
    match ws.focus {
        Some(Focus::Channel(id)) if s.project().channel(id).is_some() => {
            channel(ui, body, id, s, ws)
        }
        Some(Focus::Track(id)) if s.project().mixer.track(id).is_some() => {
            track(ui, body, id, s, ws)
        }
        Some(Focus::Clip(id)) if audio_clip_of(s, id).is_some() => audio_clip(ui, body, id, s),
        _ => {
            let mut c = body;
            c.cut_top(body.h * 0.35);
            ui.icon(c.cut_top(40.0), icons::SLIDERS, 28.0, t.text_faint);
            ui.text(
                c.cut_top(24.0),
                "Select a channel or mixer track",
                t.body,
                t.text_faint,
                Align::Center,
            );
        }
    }
}

fn scroll_body(ui: &mut Ui, id: W, r: Rect, content_h: f32) -> f32 {
    let mut off = vec2(0.0, *ui.state::<f32>(id));
    ui.scroll_area(id, r, vec2(r.w, content_h), &mut off);
    *ui.state::<f32>(id) = off.y;
    off.y
}

fn channel(ui: &mut Ui, r: Rect, id: Id, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let Some(ch) = s.project().channel(id).cloned() else {
        return;
    };
    let desc = peel_devices::descriptor(&ch.device.type_id);
    let mut body = r;

    // Header.
    let head = body.cut_top(44.0);
    let mut hr = head;
    let sw = hr.cut_left(6.0);
    ui.scene.rect(sw.shrink2(0.0, 4.0), 3.0, col(ch.color));
    hr.cut_left(10.0);
    let name_r = hr.cut_top(24.0);
    let name_id = W::new(("rename-channel", id));
    if ui.has_focus(name_id) {
        let mut name = ui.state::<String>(name_id).clone();
        let resp = ui.text_edit(name_id, name_r, &mut name, "Name");
        *ui.state::<String>(name_id) = name.clone();
        if resp.changed && !name.trim().is_empty() {
            let n = name.trim().to_string();
            s.doc.edit("Rename channel", |p| {
                if let Some(c) = p.channel_mut(id) {
                    c.name = n.clone();
                    let track = c.track;
                    if let Some(t) = p.mixer.track_mut(track) {
                        t.name = n;
                    }
                }
            });
        }
    } else {
        let resp = ui.interact(name_id.with("label"), name_r);
        ui.text(name_r, &ch.name, t.heading, t.text, Align::Start);
        ui.tooltip(&resp, "Double-click to rename");
        if resp.double_clicked {
            *ui.state::<String>(name_id) = ch.name.clone();
            ui.request_focus(name_id);
        }
    }
    let dev_name = desc.map_or("Missing device", |d| d.name);
    ui.text(
        hr,
        &format!(
            "{dev_name}{}",
            ch.device
                .preset
                .as_ref()
                .map(|p| format!("  ·  {p}"))
                .unwrap_or_default()
        ),
        t.caption,
        t.text_faint,
        Align::Start,
    );
    body.cut_top(10.0);

    // Channel strip row: volume, pan, routing, root key.
    let mut strip = body.cut_top(70.0);
    let mut vol = db_to_fader(ch.gain_db);
    let vcell = strip.cut_left(CELL_W);
    let v = knob_cell(
        ui,
        W::new(("insp-vol", id)),
        vcell,
        &mut vol,
        0.8,
        col(ch.color),
        false,
        "Volume",
        &format_db(ch.gain_db),
    );
    if v.changed {
        s.doc.edit_gesture(v.id.0, "Change channel volume", |p| {
            if let Some(c) = p.channel_mut(id) {
                c.gain_db = fader_to_db(vol);
            }
        });
    }
    let mut pan = (ch.pan + 1.0) * 0.5;
    let pcell = strip.cut_left(CELL_W);
    let pan_txt =
        peel_dsp::ParamDescriptor::linear(0, "", "", "", -1.0, 1.0, 0.0, peel_dsp::Unit::Pan)
            .format(ch.pan);
    let pr = knob_cell(
        ui,
        W::new(("insp-pan", id)),
        pcell,
        &mut pan,
        0.5,
        t.text_dim,
        true,
        "Pan",
        &pan_txt,
    );
    if pr.changed {
        s.doc.edit_gesture(pr.id.0, "Change channel pan", |p| {
            if let Some(c) = p.channel_mut(id) {
                c.pan = (pan * 2.0 - 1.0).clamp(-1.0, 1.0);
            }
        });
    }
    if v.released || pr.released {
        s.doc.end_gesture();
    }
    strip.cut_left(8.0);
    let mut right = strip.shrink2(0.0, 8.0);
    let route_r = right.cut_top(26.0);
    let track_name = s
        .project()
        .mixer
        .track(ch.track)
        .map_or("Master".to_string(), |t| t.name.clone());
    let rresp = ui.interact(W::new(("insp-route", id)), route_r);
    ui.scene.rect(
        route_r,
        t.radius_m,
        if rresp.hovered {
            t.control_hover
        } else {
            t.control
        },
    );
    let mut rr = route_r.shrink2(8.0, 0.0);
    ui.icon(rr.cut_left(14.0), icons::FADERS, 12.0, t.text_dim);
    rr.cut_left(4.0);
    ui.icon(rr.cut_right(12.0), icons::CARET_DOWN, 11.0, t.text_dim);
    let tn = ui.fit_text(&track_name, t.caption, rr.w);
    ui.text(rr, &tn, t.caption, t.text, Align::Start);
    ui.tooltip(&rresp, "Mixer track");
    if rresp.clicked {
        ui.open_popup(W::new(("insp-route-menu", id)), route_r);
    }
    let tracks: Vec<(Id, String)> = s
        .project()
        .mixer
        .tracks
        .iter()
        .map(|t| (t.id, t.name.clone()))
        .collect();
    let items: Vec<MenuItem> = tracks
        .iter()
        .map(|(tid, n)| MenuItem::new(n.clone()).checked(*tid == ch.track))
        .collect();
    if let Some(i) = ui.menu(
        W::new(("insp-route-menu", id)),
        vec2(route_r.x, route_r.bottom() + 4.0),
        &items,
    ) {
        let target = tracks[i].0;
        s.doc.edit("Route channel", |p| {
            if let Some(c) = p.channel_mut(id) {
                c.track = target;
            }
        });
    }
    right.cut_top(6.0);
    let key_r = right.cut_top(26.0);
    let mut key = ch.root_key as f64;
    ui.scene.rect(key_r, t.radius_m, t.control);
    let mut kr = key_r;
    let lbl = kr.cut_left(54.0);
    ui.text(
        lbl.shrink2(8.0, 0.0),
        "Step key",
        t.caption,
        t.text_faint,
        Align::Start,
    );
    let k = ui.drag_number(
        W::new(("insp-root", id)),
        kr,
        &mut key,
        0.0,
        127.0,
        0.15,
        t.readout_small,
        t.text,
        |v| note_name(v.round() as u8),
    );
    if k.changed {
        let new = key.round() as u8;
        let old = ch.root_key;
        let pid = s.selected_pattern();
        s.doc.edit_gesture(k.id.0, "Change step key", |p| {
            if let Some(c) = p.channel_mut(id) {
                c.root_key = new;
            }
            // Steps follow the key so the grid keeps showing them.
            if let Some(pt) = p.pattern_mut(pid) {
                for n in pt.notes.iter_mut().filter(|n| {
                    n.channel == id && n.key == old && n.length == peel_document::Time::new(1, 4)
                }) {
                    n.key = new;
                }
            }
        });
    }
    if k.released {
        s.doc.end_gesture();
    }
    body.cut_top(6.0);
    ui.scene.hline(body.x, body.right(), body.y, t.line_strong);
    body.cut_top(10.0);

    let Some(desc) = desc else {
        ui.text(
            body.with_h(30.0),
            &format!("Device {} is not installed", ch.device.type_id),
            t.body,
            t.warn,
            Align::Start,
        );
        return;
    };
    device_panel(ui, body, &ch.device, desc, s, ws, col(ch.color));
}

fn track(ui: &mut Ui, r: Rect, id: Id, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let Some(tr) = s.project().mixer.track(id).cloned() else {
        return;
    };
    let is_master = s.project().mixer.tracks[0].id == id;
    let mut body = r;
    let head = body.cut_top(44.0);
    let mut hr = head;
    let sw = hr.cut_left(6.0);
    ui.scene.rect(
        sw.shrink2(0.0, 4.0),
        3.0,
        if is_master { t.accent } else { col(tr.color) },
    );
    hr.cut_left(10.0);
    ui.text(hr.cut_top(24.0), &tr.name, t.heading, t.text, Align::Start);
    let sub = if tr.inserts.is_empty() {
        "No effects".to_string()
    } else {
        format!(
            "{} effect{}",
            tr.inserts.len(),
            if tr.inserts.len() == 1 { "" } else { "s" }
        )
    };
    ui.text(hr, &sub, t.caption, t.text_faint, Align::Start);
    body.cut_top(8.0);

    // Output routing (not for the master).
    if !is_master {
        let row = body.cut_top(28.0);
        let mut rr = row;
        ui.text(
            rr.cut_left(60.0),
            "Output",
            t.caption,
            t.text_dim,
            Align::Start,
        );
        let out_id = tr.output.unwrap_or(s.project().mixer.tracks[0].id);
        let out_name = s
            .project()
            .mixer
            .track(out_id)
            .map_or("Master".to_string(), |t| t.name.clone());
        let resp = ui.interact(W::new(("insp-output", id)), rr);
        ui.scene.rect(
            rr,
            t.radius_m,
            if resp.hovered {
                t.control_hover
            } else {
                t.control
            },
        );
        let mut inner = rr.shrink2(10.0, 0.0);
        ui.icon(inner.cut_right(14.0), icons::CARET_DOWN, 11.0, t.text_dim);
        ui.text(inner, &out_name, t.label, t.text, Align::Start);
        if resp.clicked {
            ui.open_popup(W::new(("insp-output-menu", id)), rr);
        }
        // Valid destinations: any track that does not (indirectly) feed this one.
        let p = s.project();
        let feeds_into = |candidate: peel_document::Id| -> bool {
            let mut cur = Some(candidate);
            let mut guard = 0;
            while let Some(c) = cur {
                if c == id {
                    return true;
                }
                if guard > 64 {
                    return true;
                }
                guard += 1;
                cur = p.mixer.track(c).and_then(|t| t.output).filter(|o| *o != c);
                if c == p.mixer.tracks[0].id {
                    break;
                }
            }
            false
        };
        let dests: Vec<(peel_document::Id, String)> = p
            .mixer
            .tracks
            .iter()
            .filter(|t| t.id != id && !feeds_into(t.id))
            .map(|t| (t.id, t.name.clone()))
            .collect();
        let items: Vec<MenuItem> = dests
            .iter()
            .map(|(d, n)| MenuItem::new(n.clone()).checked(*d == out_id))
            .collect();
        if let Some(i) = ui.menu(
            W::new(("insp-output-menu", id)),
            vec2(rr.x, rr.bottom() + 4.0),
            &items,
        ) {
            let dest = dests[i].0;
            let master = s.project().mixer.tracks[0].id;
            s.doc.edit("Route mixer track", |p| {
                if let Some(t) = p.mixer.track_mut(id) {
                    t.output = if dest == master { None } else { Some(dest) };
                }
            });
        }
        body.cut_top(10.0);
        sends_section(ui, &mut body, &tr, s);
    }

    if tr.inserts.is_empty() {
        let mut c = body;
        c.cut_top(40.0);
        ui.icon(c.cut_top(36.0), icons::SLIDERS, 24.0, t.text_faint);
        ui.text(
            c.cut_top(22.0),
            "Add effects from the Browser",
            t.body,
            t.text_faint,
            Align::Center,
        );
        ui.text(
            c.cut_top(20.0),
            "or a mixer slot",
            t.body,
            t.text_faint,
            Align::Center,
        );
        return;
    }
    // Stack every insert's panel in a scroll area.
    let heights: Vec<f32> = tr
        .inserts
        .iter()
        .map(|d| {
            peel_devices::descriptor(&d.type_id)
                .map_or(60.0, |desc| panel_height(desc, body.w) + 40.0)
        })
        .collect();
    let total: f32 = heights.iter().sum::<f32>() + 12.0;
    let scroll_id = W::new(("insp-track-scroll", id));
    let off = scroll_body(ui, scroll_id, body, total);
    ui.scene.push_clip(body);
    let mut y = body.y - off;
    for (dev, h) in tr.inserts.iter().zip(heights) {
        let card = Rect {
            x: body.x,
            y,
            w: body.w - 6.0,
            h: h - 8.0,
        };
        y += h;
        if card.bottom() < body.y || card.y > body.bottom() {
            continue;
        }
        let Some(desc) = peel_devices::descriptor(&dev.type_id) else {
            continue;
        };
        let mut c = card;
        let header = c.cut_top(28.0);
        let mut hr = header;
        let did = dev.id;
        let led = ui.led(
            W::new(("insp-bypass", did)),
            hr.cut_left(20.0),
            !dev.bypass,
            t.play,
        );
        ui.tooltip(&led, if dev.bypass { "Enable" } else { "Bypass" });
        if led.clicked {
            s.doc.edit(
                if dev.bypass {
                    "Enable effect"
                } else {
                    "Bypass effect"
                },
                |p| {
                    if let Some(d) = p
                        .mixer
                        .track_mut(id)
                        .and_then(|t| t.inserts.iter_mut().find(|d| d.id == did))
                    {
                        d.bypass = !d.bypass;
                    }
                },
            );
        }
        hr.cut_left(6.0);
        let x = ui.icon_button(
            W::new(("insp-remove", did)),
            hr.cut_right(26.0),
            icons::X,
            false,
            None,
        );
        ui.tooltip(&x, "Remove");
        if x.clicked {
            let name = desc.name;
            s.doc.edit(format!("Remove {name}"), |p| {
                if let Some(t) = p.mixer.track_mut(id) {
                    t.inserts.retain(|d| d.id != did);
                }
            });
        }
        ui.text(
            hr,
            desc.name,
            t.label,
            if dev.bypass { t.text_faint } else { t.text },
            Align::Start,
        );
        c.cut_top(4.0);
        device_panel(ui, c, dev, desc, s, ws, t.accent);
        ui.scene
            .hline(card.x, card.right(), card.bottom() + 3.0, t.line);
    }
    ui.scene.pop_clip();
}

fn panel_height(desc: &DeviceDescriptor, w: f32) -> f32 {
    let per_row = ((w - 6.0) / CELL_W).floor().max(1.0) as usize;
    let mut h = 0.0;
    if !peel_devices::presets(desc.type_id).is_empty() {
        h += 36.0;
    }
    if desc.type_id == "peel.sampler" {
        h += 80.0;
    }
    if desc.type_id == "peel.fx.eq" {
        h += 140.0;
    }
    for (_, params) in groups(desc) {
        h += 22.0;
        let (wide, knobs): (Vec<&ParamDescriptor>, Vec<&ParamDescriptor>) =
            params.iter().copied().partition(|p| is_wide_choice(p));
        h += wide.len() as f32 * 34.0;
        h += knobs.len().div_ceil(per_row) as f32 * CELL_H;
        h += 6.0;
    }
    h + 8.0
}

fn groups(desc: &DeviceDescriptor) -> Vec<(&'static str, Vec<&'static ParamDescriptor>)> {
    let mut out: Vec<(&'static str, Vec<&'static ParamDescriptor>)> = Vec::new();
    for p in desc.params {
        match out.iter_mut().find(|(g, _)| *g == p.group) {
            Some((_, v)) => v.push(p),
            None => out.push((p.group, vec![p])),
        }
    }
    out
}

/// Enumerations with more than two choices get a full-width selector.
fn is_wide_choice(p: &ParamDescriptor) -> bool {
    p.is_choice() && (p.choices.len() > 2 || p.choices != ["Off", "On"])
}

fn device_panel(
    ui: &mut Ui,
    r: Rect,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    s: &mut Session,
    ws: &mut Workspace,
    accent: Color,
) {
    let t = ui.theme.clone();
    let mut body = r;
    // Presets.
    let presets = peel_devices::presets(desc.type_id);
    if !presets.is_empty() {
        let pr = body.cut_top(28.0);
        let id = W::new(("preset", dev.id));
        let resp = ui.interact(id, pr);
        ui.scene.rect(
            pr,
            t.radius_m,
            if resp.hovered {
                t.control_hover
            } else {
                t.control
            },
        );
        let mut inner = pr.shrink2(10.0, 0.0);
        ui.icon(inner.cut_left(16.0), icons::STAR, 12.0, t.accent);
        inner.cut_left(4.0);
        ui.icon(inner.cut_right(14.0), icons::CARET_DOWN, 11.0, t.text_dim);
        let label = dev.preset.clone().unwrap_or_else(|| "Presets".into());
        ui.text(inner, &label, t.label, t.text, Align::Start);
        if resp.clicked {
            ui.open_popup(id.with("menu"), pr);
        }
        if resp.hovered {
            ui.set_cursor(CursorIcon::Pointer);
        }
        let items: Vec<MenuItem> = presets
            .iter()
            .map(|p| MenuItem::new(p.name).checked(dev.preset.as_deref() == Some(p.name)))
            .collect();
        if let Some(i) = ui.menu(id.with("menu"), vec2(pr.x, pr.bottom() + 4.0), &items) {
            let preset = presets[i];
            let did = dev.id;
            s.doc.edit(format!("Load preset {}", preset.name), |p| {
                if let Some(d) = peel_session::find_device_mut(p, did) {
                    for pd in desc.params {
                        d.params.insert(pd.key.to_string(), pd.default);
                    }
                    for (k, v) in preset.params {
                        d.params.insert((*k).to_string(), *v);
                    }
                    d.preset = Some(preset.name.to_string());
                }
            });
        }
        body.cut_top(8.0);
    }
    if desc.type_id == "peel.sampler" {
        sample_view(ui, &mut body, dev, s);
    }
    if desc.type_id == "peel.fx.eq" {
        eq_view(ui, &mut body, dev, desc, s);
    }

    let mut content_h = panel_height(desc, body.w);
    if !presets.is_empty() {
        content_h -= 36.0;
    }
    if desc.type_id == "peel.sampler" {
        content_h -= 80.0;
    }
    if desc.type_id == "peel.fx.eq" {
        content_h -= 140.0;
    }
    let scroll_id = W::new(("insp-dev-scroll", dev.id));
    let off = if content_h > body.h {
        scroll_body(ui, scroll_id, body, content_h)
    } else {
        0.0
    };
    ui.scene.push_clip(body);
    let per_row = ((body.w - 6.0) / CELL_W).floor().max(1.0) as usize;
    let mut y = body.y - off;
    for (group, params) in groups(desc) {
        ui.text(
            Rect {
                x: body.x,
                y,
                w: body.w,
                h: 20.0,
            },
            group,
            t.caption,
            t.text_faint,
            Align::Start,
        );
        y += 22.0;
        let (wide, knobs): (Vec<&ParamDescriptor>, Vec<&ParamDescriptor>) =
            params.into_iter().partition(|p| is_wide_choice(p));
        for p in wide {
            let row = Rect {
                x: body.x,
                y,
                w: body.w - 6.0,
                h: 28.0,
            };
            y += 34.0;
            let value = dev.params.get(p.key).copied().unwrap_or(p.default);
            let mut sel = value.round().max(0.0) as usize;
            let short: Vec<&str> = p.choices.to_vec();
            // Segmented when every label fits its cell, otherwise a menu.
            let cell_w = (row.w - 56.0 - 4.0) / p.choices.len().max(1) as f32;
            let fits = p
                .choices
                .iter()
                .all(|c| ui.measure(c, t.label).x + 10.0 <= cell_w);
            if p.choices.len() <= 5 && fits {
                let mut rr = row;
                let lbl = rr.cut_left(56.0);
                ui.text(lbl, p.name, t.caption, t.text_dim, Align::Start);
                if ui.segmented(W::new(("choice", dev.id, p.id)), rr, &short, &mut sel) {
                    s.set_param(
                        dev.id,
                        p.key,
                        sel as f32,
                        W::new(("choice-g", dev.id, p.id, sel)).0,
                    );
                    s.doc.end_gesture();
                }
            } else {
                let id = W::new(("choice-menu", dev.id, p.id));
                let mut rr = row;
                let lbl = rr.cut_left(56.0);
                ui.text(lbl, p.name, t.caption, t.text_dim, Align::Start);
                let resp = ui.interact(id, rr);
                ui.scene.rect(
                    rr,
                    t.radius_m,
                    if resp.hovered {
                        t.control_hover
                    } else {
                        t.control
                    },
                );
                let mut inner = rr.shrink2(10.0, 0.0);
                ui.icon(inner.cut_right(14.0), icons::CARET_DOWN, 11.0, t.text_dim);
                ui.text(
                    inner,
                    &s.param_text(dev.id, p, value),
                    t.label,
                    t.text,
                    Align::Start,
                );
                if resp.clicked {
                    ui.open_popup(id.with("m"), rr);
                }
                let items: Vec<MenuItem> = p
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| MenuItem::new(*c).checked(i == sel))
                    .collect();
                if let Some(i) = ui.menu(id.with("m"), vec2(rr.x, rr.bottom() + 4.0), &items) {
                    s.set_param(dev.id, p.key, i as f32, id.0 ^ i as u64);
                    s.doc.end_gesture();
                }
            }
        }
        for chunk in knobs.chunks(per_row) {
            for (i, p) in chunk.iter().enumerate() {
                let cell = Rect {
                    x: body.x + i as f32 * CELL_W,
                    y,
                    w: CELL_W,
                    h: CELL_H,
                };
                let value = dev.params.get(p.key).copied().unwrap_or(p.default);
                if p.is_choice() {
                    // Two-state switch.
                    let on = value >= 0.5;
                    let mut cr = cell;
                    let led_r = cr.cut_top(40.0);
                    let resp = ui.led(
                        W::new(("toggle", dev.id, p.id)),
                        Rect::from_center(led_r.center(), 22.0, 22.0),
                        on,
                        accent,
                    );
                    ui.text(
                        cr.cut_top(16.0),
                        p.name,
                        t.caption,
                        t.text_dim,
                        Align::Center,
                    );
                    ui.tooltip(&resp, &s.param_text(dev.id, p, value));
                    if resp.clicked {
                        s.set_param(
                            dev.id,
                            p.key,
                            if on { 0.0 } else { 1.0 },
                            W::new(("toggle-g", dev.id, p.id, on)).0,
                        );
                        s.doc.end_gesture();
                    }
                } else {
                    let mut norm = p.to_normalized(value);
                    let default = p.to_normalized(p.default);
                    let bipolar = p.min < 0.0 && p.max > 0.0;
                    let resp = knob_cell(
                        ui,
                        W::new(("param", dev.id, p.id)),
                        cell,
                        &mut norm,
                        default,
                        accent,
                        bipolar,
                        p.name,
                        &s.param_text(dev.id, p, value),
                    );
                    let menu = W::new(("param-menu", dev.id, p.id));
                    if resp.secondary_clicked {
                        ui.open_popup(menu, resp.rect);
                    }
                    let items = [
                        MenuItem::new("Create automation clip"),
                        MenuItem::new("Reset to default"),
                    ];
                    match ui.menu(menu, vec2(resp.rect.x, resp.rect.bottom()), &items) {
                        Some(0) => {
                            let target = peel_document::AutomationTarget::Param {
                                device: dev.id,
                                param: p.key.to_string(),
                            };
                            crate::playlist::create_automation(
                                s,
                                ws,
                                target,
                                p.to_normalized(value),
                            );
                        }
                        Some(1) => {
                            s.set_param(dev.id, p.key, p.default, menu.0);
                            s.doc.end_gesture();
                        }
                        _ => {}
                    }
                    if resp.changed {
                        let plain = p.from_normalized(norm);
                        s.set_param(dev.id, p.key, plain, resp.id.0);
                    }
                    if resp.released {
                        s.doc.end_gesture();
                    }
                }
            }
            y += CELL_H;
        }
        y += 6.0;
    }
    ui.scene.pop_clip();
}

/// A knob with its name below; shows the value while hovered or dragged.
#[allow(clippy::too_many_arguments)]
fn knob_cell(
    ui: &mut Ui,
    id: W,
    cell: Rect,
    norm: &mut f32,
    default: f32,
    color: Color,
    bipolar: bool,
    name: &str,
    value: &str,
) -> peel_ui::Response {
    let t = ui.theme.clone();
    let mut c = cell;
    let kr = c.cut_top(44.0);
    let knob = Rect::from_center(kr.center(), 40.0, 40.0);
    let resp = ui.knob(id, knob, norm, default, color, bipolar);
    ui.name(&resp, name);
    ui.value_text(&resp, value);
    let label_r = c.cut_top(18.0);
    if resp.hovered || resp.active {
        ui.text(label_r, value, t.readout_small, t.text, Align::Center);
    } else {
        let n = ui.fit_text(name, t.caption, label_r.w - 2.0);
        ui.text(label_r, &n, t.caption, t.text_dim, Align::Center);
    }
    resp
}

fn sample_view(ui: &mut Ui, body: &mut Rect, dev: &DeviceInstance, s: &mut Session) {
    let t = ui.theme.clone();
    let r = body.cut_top(72.0);
    body.cut_top(8.0);
    ui.scene.rect(r, t.radius_m, t.bg);
    let Some(asset_id) = dev.sample else {
        ui.text(
            r,
            "Drop an audio file on the Channel Rack",
            t.caption,
            t.text_faint,
            Align::Center,
        );
        return;
    };
    let name = s
        .project()
        .asset(asset_id)
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let Some(buf) = s.sample(asset_id) else {
        ui.text(r, "Loading…", t.caption, t.text_faint, Align::Center);
        ui.request_repaint();
        return;
    };
    let cache_id = W::new(("wave", asset_id));
    let w = r.w.max(1.0) as usize;
    if ui.state::<WaveCache>(cache_id).asset != Some(asset_id)
        || ui.state::<WaveCache>(cache_id).cols.len() != w
    {
        let data = buf.left();
        let per = (data.len() / w.max(1)).max(1);
        let cols: Vec<(f32, f32)> = (0..w)
            .map(|i| {
                let s0 = (i * per).min(data.len());
                let s1 = ((i + 1) * per).min(data.len());
                data[s0..s1]
                    .iter()
                    .fold((0.0f32, 0.0f32), |(lo, hi), &x| (lo.min(x), hi.max(x)))
            })
            .collect();
        let c = ui.state::<WaveCache>(cache_id);
        c.asset = Some(asset_id);
        c.cols = cols;
    }
    let cols = ui.state::<WaveCache>(cache_id).cols.clone();
    let mid = r.center().y;
    let amp = r.h * 0.42;
    for (i, (lo, hi)) in cols.iter().enumerate() {
        let x = r.x + i as f32;
        let y0 = mid - hi * amp;
        let y1 = mid - lo * amp;
        ui.scene.rect(
            Rect {
                x,
                y: y0,
                w: 1.0,
                h: (y1 - y0).max(1.0),
            },
            0.0,
            t.accent.alpha(0.85),
        );
    }
    let label = format!("{name}  ·  {:.2} s", buf.duration_seconds());
    ui.text(
        Rect {
            x: r.x + 8.0,
            y: r.y + 4.0,
            w: r.w - 16.0,
            h: 16.0,
        },
        &label,
        t.caption,
        t.text_dim,
        Align::Start,
    );
    // Start marker: drag to choose where playback begins.
    let start = dev
        .params
        .get("sample.start")
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let x = r.x + r.w * start;
    if start > 0.0 {
        ui.scene
            .rect(Rect { w: x - r.x, ..r }, t.radius_m, t.bg.alpha(0.6));
    }
    let handle = Rect {
        x: x - 6.0,
        w: 12.0,
        ..r
    };
    let id = W::new(("sample-start", dev.id));
    let resp = ui.interact(id, handle);
    ui.name(&resp, "Sample start");
    let hot = resp.hovered || resp.active;
    ui.scene.rect(
        Rect {
            x: x - 1.0,
            w: 2.0,
            ..r
        },
        1.0,
        if hot { t.text } else { t.accent },
    );
    ui.scene.triangle(
        Rect {
            x: x - 5.0,
            y: r.y,
            w: 10.0,
            h: 7.0,
        },
        peel_ui::Dir::Down,
        0.5,
        if hot { t.text } else { t.accent },
    );
    if hot {
        ui.set_cursor(peel_ui::CursorIcon::EwResize);
    }
    ui.tooltip(&resp, &format!("Start {:.0}%", start * 100.0));
    if resp.dragging
        && let Some(p) = ui.pointer()
    {
        let v = ((p.x - r.x) / r.w).clamp(0.0, 0.99);
        s.set_param(dev.id, "sample.start", v, id.0);
    }
    if resp.released {
        s.doc.end_gesture();
    }
}

const EQ_DB: f32 = 18.0;
const BAND_COLORS: [u32; 4] = [0xE8735A, 0xF2B66D, 0x5FBF9A, 0x6E9FE0];

/// Interactive EQ curve: drag a band's handle to set its frequency and
/// gain, scroll over it to change Q, double-click to switch it on or off.
fn eq_view(
    ui: &mut Ui,
    body: &mut Rect,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    s: &mut Session,
) {
    let t = ui.theme.clone();
    let r = body.cut_top(132.0);
    body.cut_top(8.0);
    ui.scene.rect(r, t.radius_m, t.bg);
    let plot = r.shrink2(8.0, 10.0);
    let (f_lo, f_hi) = (20.0f32, 20_000.0f32);
    let x_of = |f: f32| plot.x + plot.w * (f / f_lo).ln() / (f_hi / f_lo).ln();
    let f_of = |x: f32| f_lo * (f_hi / f_lo).powf(((x - plot.x) / plot.w).clamp(0.0, 1.0));
    let y_of = |db: f32| plot.center().y - db.clamp(-EQ_DB, EQ_DB) / EQ_DB * plot.h * 0.5;
    let db_of = |y: f32| ((plot.center().y - y) / (plot.h * 0.5) * EQ_DB).clamp(-EQ_DB, EQ_DB);
    // Grid: decades and the 0 dB line.
    for f in [100.0, 1000.0, 10_000.0] {
        ui.scene.vline(x_of(f), plot.y, plot.bottom(), t.grid_beat);
    }
    for f in [50.0, 200.0, 500.0, 2000.0, 5000.0] {
        ui.scene.vline(x_of(f), plot.y, plot.bottom(), t.grid_step);
    }
    ui.scene.hline(plot.x, plot.right(), y_of(0.0), t.grid_bar);
    ui.scene
        .hline(plot.x, plot.right(), y_of(12.0), t.grid_step);
    ui.scene
        .hline(plot.x, plot.right(), y_of(-12.0), t.grid_step);
    for (f, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        ui.text_at(
            vec2(x_of(f) + 3.0, plot.bottom() - 14.0),
            label,
            t.caption.size(10.0),
            t.text_faint,
        );
    }
    let params: Vec<f32> = desc
        .params
        .iter()
        .map(|p| dev.params.get(p.key).copied().unwrap_or(p.default))
        .collect();
    let sr = s.sample_rate().max(8000.0);
    let n = (plot.w / 3.0) as usize;
    let pts: Vec<peel_ui::Vec2> = (0..=n)
        .map(|i| {
            let x = plot.x + plot.w * i as f32 / n as f32;
            vec2(
                x,
                y_of(peel_effects::eq::eq_response_db(&params, sr, f_of(x))),
            )
        })
        .collect();
    ui.scene.push_clip(r);
    for w in pts.windows(2) {
        let y0 = y_of(0.0);
        let top = w[0].y.min(y0);
        let h = (w[0].y - y0).abs();
        ui.scene.rect(
            Rect {
                x: w[0].x,
                y: top,
                w: (w[1].x - w[0].x).max(1.0),
                h,
            },
            0.0,
            t.accent.alpha(0.08),
        );
    }
    ui.scene.polyline(&pts, 2.0, t.accent);
    // Band handles.
    for band in 1..=4u32 {
        let key = |k: &str| format!("band{band}.{k}");
        let get = |k: &str| {
            let k = key(k);
            dev.params
                .get(&k)
                .copied()
                .or_else(|| desc.param_by_key(&k).map(|p| p.default))
                .unwrap_or(0.0)
        };
        let (freq, gain, q, on) = (get("freq"), get("gain"), get("q"), get("enable") >= 0.5);
        let c = peel_ui::Color::hex(BAND_COLORS[band as usize - 1]);
        let center = vec2(x_of(freq.clamp(f_lo, f_hi)), y_of(gain));
        let id = W::new(("eq-band", dev.id, band));
        let resp = ui.interact(id, Rect::from_center(center, 18.0, 18.0));
        let hot = resp.hovered || resp.active;
        if on {
            ui.scene
                .circle_stroke(center, if hot { 7.5 } else { 6.0 }, c, 1.5, t.bg);
        } else {
            ui.scene.circle_stroke(center, 5.5, t.bg, 1.5, c.alpha(0.7));
        }
        ui.text(
            Rect::from_center(center, 12.0, 12.0),
            &band.to_string(),
            t.caption.size(9.5),
            if on { t.text_on_accent } else { c },
            Align::Center,
        );
        if hot {
            ui.set_cursor(peel_ui::CursorIcon::Grab);
            ui.tooltip(
                &resp,
                &format!("{:.0} Hz  {:+.1} dB  Q {:.2}", freq, gain, q),
            );
        }
        if resp.double_clicked {
            s.set_param(
                dev.id,
                &key("enable"),
                if on { 0.0 } else { 1.0 },
                id.0 ^ 0xe0,
            );
            s.doc.end_gesture();
        } else if resp.dragging
            && let Some(p) = ui.pointer()
        {
            if !on {
                s.set_param(dev.id, &key("enable"), 1.0, id.0);
            }
            s.set_param(dev.id, &key("freq"), f_of(p.x).round(), id.0);
            s.set_param(
                dev.id,
                &key("gain"),
                (db_of(p.y) * 10.0).round() / 10.0,
                id.0,
            );
        }
        if resp.released {
            s.doc.end_gesture();
        }
        if resp.hovered && ui.input.scroll.y != 0.0 {
            let nq = (q * (1.0 + ui.input.scroll.y * 0.004)).clamp(0.1, 18.0);
            ui.input.scroll.y = 0.0;
            s.set_param(dev.id, &key("q"), nq, id.0 ^ 0x51);
        }
    }
    ui.scene.pop_clip();
}

/// Post-fader sends of a mixer track: one knob per send plus an add menu.
fn sends_section(ui: &mut Ui, body: &mut Rect, tr: &peel_document::MixerTrack, s: &mut Session) {
    let t = ui.theme.clone();
    let id = tr.id;
    let mut head = body.cut_top(24.0);
    ui.text(
        head.cut_left(60.0),
        "Sends",
        t.caption,
        t.text_dim,
        Align::Start,
    );
    let add = ui.icon_button(
        W::new(("send-add", id)),
        head.cut_right(24.0),
        icons::PLUS,
        false,
        None,
    );
    ui.tooltip(&add, "Send to a bus");
    if add.clicked {
        ui.open_popup(W::new(("send-menu", id)), add.rect);
    }
    // Valid targets: not this track, not already sent to, and not anything
    // that feeds into this track (that would make a loop).
    let p = s.project();
    let targets: Vec<(peel_document::Id, String)> = p
        .mixer
        .tracks
        .iter()
        .skip(1)
        .filter(|x| x.id != id && !tr.sends.iter().any(|sd| sd.target == x.id))
        .filter(|x| !reaches(p, x.id, id))
        .map(|x| (x.id, x.name.clone()))
        .collect();
    let items: Vec<MenuItem> = if targets.is_empty() {
        vec![MenuItem::new("Add a mixer track first").enabled(false)]
    } else {
        targets
            .iter()
            .map(|(_, n)| MenuItem::new(n.clone()))
            .collect()
    };
    if let Some(i) = ui.menu(
        W::new(("send-menu", id)),
        vec2(add.rect.x - 120.0, add.rect.bottom() + 4.0),
        &items,
    ) && let Some((target, name)) = targets.get(i).cloned()
    {
        s.doc.edit(format!("Send to {name}"), |p| {
            if let Some(x) = p.mixer.track_mut(id) {
                x.sends.push(peel_document::MixerSend {
                    target,
                    gain_db: -6.0,
                });
            }
        });
    }
    if tr.sends.is_empty() {
        let hint = body.cut_top(20.0);
        ui.text(hint, "None", t.caption, t.text_faint, Align::Start);
        body.cut_top(8.0);
        return;
    }
    let names: Vec<String> = tr
        .sends
        .iter()
        .map(|sd| {
            s.project()
                .mixer
                .track(sd.target)
                .map_or("Missing".into(), |x| x.name.clone())
        })
        .collect();
    for (i, sd) in tr.sends.iter().enumerate() {
        let mut row = body.cut_top(40.0);
        let knob_r = row.cut_left(40.0);
        let mut norm = peel_document::db_to_fader(sd.gain_db);
        let k = ui.knob(
            W::new(("send", id, i)),
            Rect::from_center(knob_r.center(), 34.0, 34.0),
            &mut norm,
            peel_document::db_to_fader(-6.0),
            t.accent,
            false,
        );
        ui.tooltip(&k, &crate::util::format_db(sd.gain_db));
        if k.changed {
            let db = peel_document::fader_to_db(norm);
            s.doc.edit_gesture(k.id.0, "Change send level", |p| {
                if let Some(x) = p.mixer.track_mut(id)
                    && let Some(send) = x.sends.get_mut(i)
                {
                    send.gain_db = db;
                }
            });
        }
        if k.released {
            s.doc.end_gesture();
        }
        row.cut_left(8.0);
        let x = ui.icon_button(
            W::new(("send-remove", id, i)),
            row.cut_right(24.0).shrink2(0.0, 8.0),
            icons::X,
            false,
            None,
        );
        if x.clicked {
            s.doc.edit("Remove send", |p| {
                if let Some(t) = p.mixer.track_mut(id)
                    && i < t.sends.len()
                {
                    t.sends.remove(i);
                }
            });
        }
        let label = format!("{}  {}", names[i], crate::util::format_db(sd.gain_db));
        ui.text(row, &label, t.label, t.text, Align::Start);
    }
    body.cut_top(8.0);
}

/// True if audio from `from` can reach `to` through outputs and sends.
pub(crate) fn reaches(
    p: &peel_document::Project,
    from: peel_document::Id,
    to: peel_document::Id,
) -> bool {
    let master = p.mixer.tracks[0].id;
    let mut stack = vec![from];
    let mut seen = std::collections::HashSet::new();
    while let Some(c) = stack.pop() {
        if c == to {
            return true;
        }
        if !seen.insert(c) || c == master {
            continue;
        }
        if let Some(t) = p.mixer.track(c) {
            stack.push(t.output.unwrap_or(master));
            stack.extend(t.sends.iter().map(|x| x.target));
        }
    }
    false
}

fn audio_clip_of(s: &Session, id: Id) -> Option<peel_document::Clip> {
    s.project()
        .arrangement()
        .clips
        .iter()
        .find(|c| c.id == id && matches!(c.content, peel_document::ClipContent::Audio { .. }))
        .cloned()
}

/// Gain range of the clip gain knob, in dB.
const CLIP_GAIN: (f32, f32) = (-24.0, 12.0);

/// Changes an audio clip's settings as one undo step (or gesture).
fn edit_audio_clip(
    s: &mut Session,
    id: Id,
    label: &str,
    gesture: Option<u64>,
    f: impl FnOnce(&mut f32, &mut bool, &mut Id),
) {
    let apply = |p: &mut peel_document::Project| {
        if let Some(c) = p.arrangement_mut().clips.iter_mut().find(|c| c.id == id)
            && let peel_document::ClipContent::Audio {
                gain_db,
                reversed,
                track,
                ..
            } = &mut c.content
        {
            f(gain_db, reversed, track);
        }
    };
    match gesture {
        Some(g) => s.doc.edit_gesture(g, label, apply),
        None => s.doc.edit(label, apply),
    }
}

fn audio_clip(ui: &mut Ui, r: Rect, id: Id, s: &mut Session) {
    let t = ui.theme.clone();
    let Some(clip) = audio_clip_of(s, id) else {
        return;
    };
    let peel_document::ClipContent::Audio {
        asset,
        gain_db,
        reversed,
        track,
        ..
    } = clip.content.clone()
    else {
        return;
    };
    let name = s
        .project()
        .asset(asset)
        .map_or("Audio".to_string(), |a| a.name.clone());
    let tempo = peel_time::TempoMap::from_segments(&s.project().tempo);
    let bpm = tempo.bpm_at(clip.start.as_f64());
    let secs = clip.length.as_f64() * 60.0 / bpm;
    let mut body = r;

    // Header.
    let head = body.cut_top(44.0);
    let mut hr = head;
    let sw = hr.cut_left(6.0);
    ui.scene.rect(sw.shrink2(0.0, 4.0), 3.0, t.text_dim);
    hr.cut_left(10.0);
    let shown = ui.fit_text(&name, t.heading, hr.w);
    ui.text(hr.cut_top(24.0), &shown, t.heading, t.text, Align::Start);
    ui.text(
        hr,
        &format!("Audio clip  ·  {secs:.2} s"),
        t.caption,
        t.text_faint,
        Align::Start,
    );
    body.cut_top(10.0);

    // Whole file, with the part the clip plays lit.
    let wave = body.cut_top(72.0);
    body.cut_top(12.0);
    ui.scene.rect(wave, t.radius_m, t.bg);
    let total = s.asset_duration_seconds(asset).unwrap_or(0.0);
    let src_start = peel_session::clip_source_start_seconds(&tempo, &clip);
    if let Some(peaks) = s.audio_peaks(asset)
        && total > 0.0
    {
        let inner = wave.shrink2(6.0, 6.0);
        let cols = inner.w.max(1.0) as usize;
        let frames = peaks.frames();
        let (lit0, lit1) = (
            (src_start / total) as f32,
            ((src_start + secs) / total) as f32,
        );
        for (i, (lo, hi)) in peaks.query(0, frames, cols).enumerate() {
            let x = inner.x + i as f32;
            let f = i as f32 / cols as f32;
            let c = if f >= lit0 && f <= lit1 {
                t.accent
            } else {
                t.text_faint.alpha(0.5)
            };
            let mid = inner.center().y;
            let (y0, y1) = (
                mid - hi.max(0.0) * inner.h * 0.5,
                mid - lo.min(0.0) * inner.h * 0.5,
            );
            ui.scene.rect(
                Rect {
                    x,
                    y: y0,
                    w: 1.0,
                    h: (y1 - y0).max(1.0),
                },
                0.0,
                c,
            );
        }
        let lit = Rect {
            x: inner.x + inner.w * lit0,
            w: inner.w * (lit1 - lit0).max(0.0),
            ..wave
        };
        ui.scene.outline(lit, 2.0, 1.0, t.accent.alpha(0.5));
    } else {
        ui.text(wave, "Loading", t.caption, t.text_faint, Align::Center);
    }

    // Gain, reverse, normalize.
    let row = body.cut_top(CELL_H);
    body.cut_top(8.0);
    let cells = row.columns(3, 0.0);
    let mut norm = (gain_db - CLIP_GAIN.0) / (CLIP_GAIN.1 - CLIP_GAIN.0);
    let default = -CLIP_GAIN.0 / (CLIP_GAIN.1 - CLIP_GAIN.0);
    let gid = W::new(("clip-gain", id));
    let k = knob_cell(
        ui,
        gid,
        cells[0],
        &mut norm,
        default,
        t.accent,
        false,
        "Gain",
        &crate::util::format_db(gain_db),
    );
    if k.changed {
        let db = (CLIP_GAIN.0 + norm * (CLIP_GAIN.1 - CLIP_GAIN.0)).clamp(CLIP_GAIN.0, CLIP_GAIN.1);
        edit_audio_clip(s, id, "Clip gain", Some(gid.0), |g, _, _| *g = db);
    }
    if k.released {
        s.doc.end_gesture();
    }
    let mut cr = cells[1];
    let led_r = cr.cut_top(44.0);
    let led = ui.led(
        W::new(("clip-rev", id)),
        Rect::from_center(led_r.center(), 22.0, 22.0),
        reversed,
        t.accent,
    );
    ui.name(&led, "Reverse");
    ui.text(
        cr.cut_top(16.0),
        "Reverse",
        t.caption,
        t.text_dim,
        Align::Center,
    );
    if led.clicked {
        edit_audio_clip(
            s,
            id,
            if reversed {
                "Play forwards"
            } else {
                "Reverse clip"
            },
            None,
            |_, rv, _| *rv = !*rv,
        );
    }
    let mut cr = cells[2];
    let btn_r = Rect::from_center(cr.cut_top(44.0).center(), 30.0, 30.0);
    let nb = ui.icon_button(
        W::new(("clip-normalize", id)),
        btn_r,
        icons::LIGHTNING,
        false,
        None,
    );
    ui.text(
        cr.cut_top(16.0),
        "Normalize",
        t.caption,
        t.text_dim,
        Align::Center,
    );
    ui.tooltip(
        &nb,
        "Set the gain so the loudest peak in the clip reaches 0 dB",
    );
    if nb.clicked
        && let Some(buf) = s.sample(asset)
    {
        let rate = f64::from(buf.sample_rate);
        let a = (src_start * rate) as usize;
        let b = ((src_start + secs) * rate) as usize;
        let peak = buf
            .channels
            .iter()
            .flat_map(|d| {
                let (a, b) = (a.min(d.len()), b.min(d.len()));
                d[a..b].iter().map(|x| x.abs())
            })
            .fold(0.0f32, f32::max);
        if peak > 1e-5 {
            let db = (-peel_dsp::util::gain_to_db(peak)).clamp(CLIP_GAIN.0, CLIP_GAIN.1);
            edit_audio_clip(s, id, "Normalize clip", None, |g, _, _| *g = db);
        } else {
            s.notify("The clip is silent", true);
        }
    }

    // Mixer output.
    let mut row = body.cut_top(28.0);
    ui.text(
        row.cut_left(60.0),
        "Output",
        t.caption,
        t.text_dim,
        Align::Start,
    );
    let tracks: Vec<(Id, String)> = s
        .project()
        .mixer
        .tracks
        .iter()
        .map(|m| (m.id, m.name.clone()))
        .collect();
    let current = tracks
        .iter()
        .find(|(tid, _)| *tid == track)
        .map_or("Master".to_string(), |x| x.1.clone());
    let oid = W::new(("clip-output", id));
    let ob = ui.interact_button(oid, row, &current);
    ui.scene.rect(
        row,
        t.radius_m,
        if ob.hovered {
            t.control_hover
        } else {
            t.control
        },
    );
    let mut inner = row.shrink2(10.0, 0.0);
    ui.icon(inner.cut_right(14.0), icons::CARET_DOWN, 11.0, t.text_dim);
    ui.text(inner, &current, t.label, t.text, Align::Start);
    if ob.clicked {
        ui.open_popup(oid.with("menu"), row);
    }
    let items: Vec<MenuItem> = tracks
        .iter()
        .map(|(tid, n)| MenuItem::new(n.clone()).checked(*tid == track))
        .collect();
    if let Some(i) = ui.menu(oid.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
        let to = tracks[i].0;
        edit_audio_clip(s, id, "Route clip", None, |_, _, tr| *tr = to);
    }
}
