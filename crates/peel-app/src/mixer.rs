//! Mixer: one strip per mixer track, master pinned on the right.

use peel_document::{Id, MixerTrack};
use peel_session::{Session, template};
use peel_ui::Id as W;
use peel_ui::{Align, Color, CursorIcon, MenuItem, Rect, Ui, icons, vec2};

use crate::browser::BrowserItem;
use crate::util::{col, db_to_fader, fader_to_db, format_db, ink_on};
use crate::workspace::{Focus, Workspace};

const STRIP_W: f32 = 86.0;
const STRIP_GAP: f32 = 6.0;
const SLOTS: usize = 4;

pub fn draw(ui: &mut Ui, r: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let mut body = r.shrink2(10.0, 0.0);
    body.cut_bottom(10.0);
    let tracks: Vec<MixerTrack> = s.project().mixer.tracks.clone();
    let Some(master) = tracks.first().cloned() else {
        return;
    };

    let master_r = body.cut_right(STRIP_W + 10.0);
    body.cut_right(10.0);
    ui.scene.vline(
        master_r.x - 6.0,
        master_r.y + 6.0,
        master_r.bottom() - 6.0,
        t.line_strong,
    );

    let content_w = (tracks.len() - 1) as f32 * (STRIP_W + STRIP_GAP) + 48.0;
    let mut off = vec2(ws.mixer_scroll, 0.0);
    if ui.pointer_in(body)
        && ui.input.scroll.x == 0.0
        && ui.input.scroll.y != 0.0
        && !ui.input.modifiers.command()
        && !ui.any_active()
    {
        // Vertical wheel scrolls strips sideways unless over a control.
        let hovering_control = false;
        if !hovering_control && !ui.input.modifiers.shift {
            ui.input.scroll = vec2(ui.input.scroll.y, 0.0);
        }
    }
    ui.scroll_area(
        W::new("mixer-scroll"),
        body,
        vec2(content_w, body.h),
        &mut off,
    );
    ws.mixer_scroll = off.x;

    ui.scene.push_clip(body);
    let mut x = body.x - off.x;
    for tr in tracks.iter().skip(1) {
        let sr = Rect {
            x,
            y: body.y,
            w: STRIP_W,
            h: body.h,
        };
        x += STRIP_W + STRIP_GAP;
        if sr.right() < body.x || sr.x > body.right() {
            continue;
        }
        strip(ui, sr, tr, false, s, ws);
    }
    // Add track.
    let add_r = Rect {
        x,
        y: body.y + 6.0,
        w: 36.0,
        h: 30.0,
    };
    let b = ui.icon_button(W::new("mixer-add"), add_r, icons::PLUS, false, None);
    ui.tooltip(&b, "Add mixer track");
    if b.clicked {
        let n = tracks.len();
        s.doc.edit("Add mixer track", |p| {
            p.mixer.tracks.push(MixerTrack {
                id: Id::new(),
                name: format!("Bus {n}"),
                color: peel_document::PALETTE[(n * 5) % peel_document::PALETTE.len()],
                gain_db: 0.0,
                pan: 0.0,
                muted: false,
                solo: false,
                inserts: vec![],
                output: None,
                sends: Vec::new(),
            })
        });
    }
    ui.scene.pop_clip();

    strip(ui, master_r.shrink2(5.0, 0.0), &master, true, s, ws);
}

fn strip(
    ui: &mut Ui,
    r: Rect,
    tr: &MixerTrack,
    is_master: bool,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let c = col(tr.color);
    let selected = ws.focus == Some(Focus::Track(tr.id));
    let bg_id = W::new(("strip-bg", tr.id));
    let bg = ui.interact(bg_id, r);
    if bg.pressed {
        ws.focus = Some(Focus::Track(tr.id));
    }
    ui.scene
        .rect(r, t.radius_m, if selected { t.raised } else { t.panel_alt });
    // Effect drops.
    let is_effect = |i: &BrowserItem| match i {
        BrowserItem::Device(id) | BrowserItem::Preset(id, _) => {
            peel_devices::descriptor(id).is_some_and(|d| d.kind == peel_dsp::DeviceKind::Effect)
        }
        _ => false,
    };
    if ui.drop_hover::<BrowserItem>(r).is_some_and(is_effect) {
        ui.scene.outline(r, t.radius_m, 2.0, t.focus.alpha(0.7));
        if let Some((item, _)) = ui.take_drop::<BrowserItem>(r) {
            match item {
                BrowserItem::Device(id) => add_insert(s, tr.id, id),
                BrowserItem::Preset(id, preset) => add_insert_preset(s, tr.id, id, Some(preset)),
                _ => {}
            }
            ws.focus = Some(Focus::Track(tr.id));
        }
    }

    let mut col_r = r.shrink2(6.0, 6.0);
    // Cap and name.
    let cap = col_r.cut_top(4.0);
    ui.scene
        .rect(cap, 2.0, if is_master { t.accent } else { c });
    col_r.cut_top(4.0);
    let name_r = col_r.cut_top(20.0);
    let name = ui.fit_text(&tr.name, t.label, name_r.w);
    ui.text(
        name_r,
        &name,
        t.label,
        if selected {
            t.text
        } else {
            t.text.mix(t.text_dim, 0.3)
        },
        Align::Center,
    );
    if !is_master {
        strip_menu(ui, name_r, tr, s, ws);
    }
    col_r.cut_top(4.0);

    // Insert slots.
    for i in 0..SLOTS {
        let slot_r = col_r.cut_top(20.0);
        col_r.cut_top(2.0);
        let id = W::new(("insert", tr.id, i));
        match tr.inserts.get(i) {
            Some(dev) => {
                let resp = ui.interact(id, slot_r);
                let name = peel_devices::descriptor(&dev.type_id).map_or("Missing", |d| d.name);
                let fill = if resp.hovered {
                    t.control_hover
                } else {
                    t.control
                };
                ui.scene.rect(slot_r, t.radius_s, fill);
                let dot = Rect {
                    x: slot_r.x + 6.0,
                    w: 6.0,
                    ..slot_r
                };
                ui.scene.circle(
                    dot.center(),
                    3.0,
                    if dev.bypass { t.text_faint } else { t.play },
                );
                ui.text(
                    Rect {
                        x: slot_r.x + 16.0,
                        w: slot_r.w - 18.0,
                        ..slot_r
                    },
                    name,
                    t.caption,
                    if dev.bypass { t.text_faint } else { t.text },
                    Align::Start,
                );
                if resp.clicked {
                    ws.focus = Some(Focus::Track(tr.id));
                }
                if resp.hovered {
                    ui.set_cursor(CursorIcon::Pointer);
                }
                let menu = W::new(("insert-menu", tr.id, i));
                if resp.secondary_clicked {
                    ui.open_popup(menu, slot_r);
                }
                let items = [
                    MenuItem::new(if dev.bypass { "Enable" } else { "Bypass" }),
                    MenuItem::new("Move up").enabled(i > 0),
                    MenuItem::new("Move down").enabled(i + 1 < tr.inserts.len()),
                    MenuItem::Separator,
                    MenuItem::new("Remove"),
                ];
                if let Some(choice) = ui.menu(menu, vec2(slot_r.x, slot_r.bottom() + 2.0), &items) {
                    let tid = tr.id;
                    let did = dev.id;
                    match choice {
                        0 => s.doc.edit(
                            if dev.bypass {
                                "Enable effect"
                            } else {
                                "Bypass effect"
                            },
                            |p| {
                                if let Some(d) = p
                                    .mixer
                                    .track_mut(tid)
                                    .and_then(|t| t.inserts.iter_mut().find(|d| d.id == did))
                                {
                                    d.bypass = !d.bypass;
                                }
                            },
                        ),
                        1 => s.doc.edit("Move effect", |p| {
                            if let Some(t) = p.mixer.track_mut(tid) {
                                t.inserts.swap(i, i - 1);
                            }
                        }),
                        2 => s.doc.edit("Move effect", |p| {
                            if let Some(t) = p.mixer.track_mut(tid) {
                                t.inserts.swap(i, i + 1);
                            }
                        }),
                        4 => s.doc.edit(format!("Remove {name}"), |p| {
                            if let Some(t) = p.mixer.track_mut(tid) {
                                t.inserts.retain(|d| d.id != did);
                            }
                        }),
                        _ => {}
                    }
                }
            }
            None => {
                let resp = ui.interact(id, slot_r);
                let hot = ui.animate_bool(id, resp.hovered);
                ui.scene
                    .rect(slot_r, t.radius_s, t.control.alpha(0.25 + hot * 0.5));
                if i == tr.inserts.len() {
                    ui.icon(slot_r, icons::PLUS, 11.0, t.text_faint.mix(t.text, hot));
                    if resp.clicked {
                        ui.open_popup(W::new(("add-fx", tr.id)), slot_r);
                    }
                    if resp.hovered {
                        ui.set_cursor(CursorIcon::Pointer);
                    }
                    ui.tooltip(&resp, "Add effect");
                }
                let effects: Vec<(&'static str, &'static str)> = peel_devices::effects()
                    .map(|d| (d.type_id, d.name))
                    .collect();
                if i == tr.inserts.len() {
                    let items: Vec<MenuItem> =
                        effects.iter().map(|(_, n)| MenuItem::new(*n)).collect();
                    if let Some(choice) = ui.menu(
                        W::new(("add-fx", tr.id)),
                        vec2(slot_r.x, slot_r.bottom() + 2.0),
                        &items,
                    ) {
                        add_insert(s, tr.id, effects[choice].0);
                        ws.focus = Some(Focus::Track(tr.id));
                    }
                }
            }
        }
    }
    col_r.cut_top(6.0);

    // Pan.
    let pan_r = col_r.cut_top(34.0);
    let mut pan = (tr.pan + 1.0) * 0.5;
    let k = ui.knob(
        W::new(("strip-pan", tr.id)),
        Rect::from_center(pan_r.center(), 32.0, 32.0),
        &mut pan,
        0.5,
        t.text_dim,
        true,
    );
    ui.name(&k, &format!("{} pan", tr.name));
    automate_menu(
        ui,
        s,
        ws,
        k,
        peel_document::AutomationTarget::TrackPan { track: tr.id },
        (tr.pan + 1.0) * 0.5,
        "Automate pan",
    );
    ui.tooltip(
        &k,
        &peel_dsp::ParamDescriptor::linear(0, "", "", "", -1.0, 1.0, 0.0, peel_dsp::Unit::Pan)
            .format(tr.pan),
    );
    if k.changed {
        let tid = tr.id;
        s.doc.edit_gesture(k.id.0, "Change pan", |p| {
            if let Some(t) = p.mixer.track_mut(tid) {
                t.pan = (pan * 2.0 - 1.0).clamp(-1.0, 1.0);
            }
        });
    }
    if k.released {
        s.doc.end_gesture();
    }
    col_r.cut_top(4.0);

    // Mute / solo.
    let mut ms = col_r.cut_bottom(24.0);
    let half = (ms.w - 4.0) / 2.0;
    let mute_r = ms.cut_left(half);
    ms.cut_left(4.0);
    let solo_r = ms;
    if toggle(ui, W::new(("mute", tr.id)), mute_r, "M", tr.muted, t.warn) {
        let tid = tr.id;
        s.doc.edit(
            if tr.muted {
                "Unmute track"
            } else {
                "Mute track"
            },
            |p| {
                if let Some(t) = p.mixer.track_mut(tid) {
                    t.muted = !t.muted;
                }
            },
        );
    }
    if !is_master && toggle(ui, W::new(("solo", tr.id)), solo_r, "S", tr.solo, t.focus) {
        let tid = tr.id;
        s.doc.edit(
            if tr.solo {
                "Unsolo track"
            } else {
                "Solo track"
            },
            |p| {
                if let Some(t) = p.mixer.track_mut(tid) {
                    t.solo = !t.solo;
                }
            },
        );
    }
    col_r.cut_bottom(4.0);
    let db_r = col_r.cut_bottom(18.0);
    ui.text(
        db_r,
        &format_db(tr.gain_db),
        t.readout_small,
        t.text_dim,
        Align::Center,
    );
    col_r.cut_bottom(2.0);
    if let Some(out) = tr.output.filter(|_| !is_master)
        && let Some(dest) = s.project().mixer.track(out)
    {
        let route_r = col_r.cut_bottom(16.0);
        let label = ui.fit_text(&format!("→ {}", dest.name), t.caption, route_r.w);
        ui.text(route_r, &label, t.caption, col(dest.color), Align::Center);
    }

    // Fader and meter.
    let mut fm = col_r;
    let meter_r = fm.cut_right(12.0);
    fm.cut_right(4.0);
    let mut pos = db_to_fader(tr.gain_db);
    let f = ui.fader(
        W::new(("fader", tr.id)),
        fm,
        &mut pos,
        0.8,
        if is_master { t.accent } else { c },
    );
    if f.changed {
        let tid = tr.id;
        let db = fader_to_db(pos);
        let db = if (db).abs() < 0.25 {
            0.0
        } else {
            (db * 10.0).round() / 10.0
        };
        s.doc.edit_gesture(f.id.0, "Change volume", |p| {
            if let Some(t) = p.mixer.track_mut(tid) {
                t.gain_db = db;
            }
        });
    }
    if f.released {
        s.doc.end_gesture();
    }
    ui.name(&f, &format!("{} volume", tr.name));
    ui.value_text(&f, &format_db(tr.gain_db));
    automate_menu(
        ui,
        s,
        ws,
        f,
        peel_document::AutomationTarget::TrackGain { track: tr.id },
        db_to_fader(tr.gain_db),
        "Automate volume",
    );
    let peaks = s.take_track_meter(tr.id);
    ui.meter(W::new(("meter", tr.id)), meter_r, peaks);
    let _ = ink_on(c);
}

fn toggle(ui: &mut Ui, id: W, r: Rect, label: &str, on: bool, color: Color) -> bool {
    let t = ui.theme.clone();
    let resp = ui.interact(id, r);
    let hot = ui.animate_bool(id, resp.hovered);
    let fill = if on {
        color
    } else {
        t.control.mix(t.control_hover, hot)
    };
    ui.scene.rect(r, t.radius_s, fill);
    ui.text(
        r,
        label,
        t.label,
        if on { ink_on(color) } else { t.text_dim },
        Align::Center,
    );
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    resp.clicked
}

pub fn add_insert(s: &mut Session, track: Id, type_id: &str) {
    add_insert_preset(s, track, type_id, None);
}

/// Adds an insert effect with a factory preset applied.
pub fn add_insert_preset(s: &mut Session, track: Id, type_id: &str, preset: Option<&str>) {
    let name = peel_devices::descriptor(type_id)
        .map_or("effect", |d| d.name)
        .to_string();
    let dev = template::device(type_id, preset);
    s.doc.edit(format!("Add {name}"), |p| {
        if let Some(t) = p.mixer.track_mut(track) {
            t.inserts.push(dev);
        }
    });
}

/// Right-click menu on a mixer control offering an automation clip.
fn automate_menu(
    ui: &mut Ui,
    s: &mut Session,
    ws: &mut Workspace,
    resp: peel_ui::Response,
    target: peel_document::AutomationTarget,
    value: f32,
    label: &str,
) {
    let menu = resp.id.with("automate");
    if resp.secondary_clicked {
        ui.open_popup(menu, resp.rect);
    }
    if ui.menu(
        menu,
        vec2(resp.rect.x, resp.rect.bottom()),
        &[MenuItem::new(label)],
    ) == Some(0)
    {
        crate::playlist::create_automation(s, ws, target, value);
    }
}

/// Right-click a strip's name: rename, recolor or delete the track.
fn strip_menu(ui: &mut Ui, name_r: Rect, tr: &MixerTrack, s: &mut Session, ws: &mut Workspace) {
    let resp = ui.interact(W::new(("strip-name", tr.id)), name_r);
    let menu = W::new(("strip-menu", tr.id));
    if resp.secondary_clicked {
        ui.open_popup(menu, name_r);
    }
    if resp.double_clicked {
        crate::rename::open(ui, W::new(("strip-rename", tr.id)), name_r, &tr.name);
    }
    let mut items = vec![
        MenuItem::new("Rename"),
        MenuItem::Separator,
        MenuItem::Heading("Color".into()),
    ];
    let first_color = items.len();
    for (c, name) in peel_document::PALETTE.iter().zip(crate::util::COLOR_NAMES) {
        items.push(MenuItem::new(name).swatch(col(*c)).checked(*c == tr.color));
    }
    items.push(MenuItem::Separator);
    let delete = items.len();
    items.push(MenuItem::new("Delete track"));
    let tid = tr.id;
    match ui.menu(menu, vec2(name_r.x, name_r.bottom() + 4.0), &items) {
        Some(0) => crate::rename::open(ui, W::new(("strip-rename", tid)), name_r, &tr.name),
        Some(i) if i >= first_color && i < first_color + peel_document::PALETTE.len() => {
            let color = peel_document::PALETTE[i - first_color];
            s.doc.edit("Recolor mixer track", |p| {
                if let Some(t) = p.mixer.track_mut(tid) {
                    t.color = color;
                }
            });
        }
        Some(i) if i == delete => {
            let name = tr.name.clone();
            s.doc.edit(format!("Delete {name}"), |p| {
                let master = p.mixer.tracks[0].id;
                p.mixer.tracks.retain(|t| t.id != tid);
                for t in p.mixer.tracks.iter_mut() {
                    if t.output == Some(tid) {
                        t.output = None;
                    }
                    t.sends.retain(|sd| sd.target != tid);
                }
                for c in p.channels.iter_mut() {
                    if c.track == tid {
                        c.track = master;
                    }
                }
                for a in p.arrangements.iter_mut() {
                    let a = std::sync::Arc::make_mut(a);
                    for c in a.clips.iter_mut() {
                        if let peel_document::ClipContent::Audio { track, .. } = &mut c.content
                            && *track == tid
                        {
                            *track = master;
                        }
                    }
                    // Automation of the deleted track has nothing to drive.
                    a.clips.retain(|c| !matches!(&c.content, peel_document::ClipContent::Automation { target: peel_document::AutomationTarget::TrackGain { track } | peel_document::AutomationTarget::TrackPan { track }, .. } if *track == tid));
                }
            });
            if ws.focus == Some(Focus::Track(tid)) {
                ws.focus = None;
            }
        }
        _ => {}
    }
    if let Some(name) = crate::rename::show(ui, W::new(("strip-rename", tid))) {
        s.doc.edit("Rename mixer track", |p| {
            if let Some(t) = p.mixer.track_mut(tid) {
                t.name = name;
            }
        });
    }
}
