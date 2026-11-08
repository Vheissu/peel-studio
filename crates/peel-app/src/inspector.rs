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

fn channel(ui: &mut Ui, r: Rect, id: Id, s: &mut Session, _ws: &mut Workspace) {
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
    device_panel(ui, body, &ch.device, desc, s, col(ch.color));
}

fn track(ui: &mut Ui, r: Rect, id: Id, s: &mut Session, _ws: &mut Workspace) {
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
        device_panel(ui, c, dev, desc, s, t.accent);
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
    p.is_choice() && p.choices.len() > 2
}

fn device_panel(
    ui: &mut Ui,
    r: Rect,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    s: &mut Session,
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

    let mut content_h = panel_height(desc, body.w);
    if !presets.is_empty() {
        content_h -= 36.0;
    }
    if desc.type_id == "peel.sampler" {
        content_h -= 80.0;
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
            if p.choices.len() <= 5 {
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
                ui.text(inner, &p.format(value), t.label, t.text, Align::Start);
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
                    ui.tooltip(&resp, &p.format(value));
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
                        &p.format(value),
                    );
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
}
