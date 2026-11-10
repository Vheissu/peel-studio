//! Mixer: one strip per mixer track, master pinned on the right.
//!
//! Strips come in three sizes (Compact, Expanded, Full). Each size decides
//! which rows a strip has; the dock height decides how many insert and
//! send rows are visible, so the fader never shrinks below a usable
//! length. Everything shown is real engine state: gain, pan, mute, solo,
//! inserts (all of them, scrolled), sends and output routing.

use peel_document::{Id, MixerTrack};
use peel_session::{Session, template};
use peel_ui::Id as W;
use peel_ui::{
    Align, Color, CursorIcon, FocusKind, MenuItem, Rect, Role, Semantics, Ui, icons, meter_pos,
    vec2,
};

use crate::browser::BrowserItem;
use crate::util::{
    col, db_to_fader, fader_to_db, format_db, format_pan, ink_on, peak_text, track_peaks,
};
use crate::workspace::{Focus, MixerSize, Workspace};

/// Strips sit side by side; the dock shows through as a 1 px seam.
const STRIP_GAP: f32 = 1.0;
const ROW: f32 = 22.0;
const SEND_ROW: f32 = 20.0;
/// Shortest fader the layout aims to keep.
const FADER_MIN: f32 = 96.0;
const MASTER_TYPE: &str = "peel.fx.mastering";

/// What one strip shows at a given size and height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub width: f32,
    pub meter_w: f32,
    pub scale: bool,
    pub route: bool,
    pub pan_text: bool,
    pub numeric: bool,
    pub sends: SendsRow,
    pub insert_rows: usize,
    pub send_rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendsRow {
    None,
    /// One button that opens the send menu.
    Summary,
    /// A list with levels.
    List,
}

/// Rows a strip can afford in `height` pixels.
pub fn plan(size: MixerSize, height: f32, master: bool) -> Plan {
    let (width, meter_w, scale, route, pan_text, numeric, sends) = match size {
        MixerSize::Compact => (64.0, 14.0, false, master, false, false, SendsRow::None),
        MixerSize::Expanded => (92.0, 18.0, true, true, true, true, SendsRow::Summary),
        MixerSize::Full => (124.0, 22.0, true, true, true, true, SendsRow::List),
    };
    let sends = if master { SendsRow::None } else { sends };
    // Cap, name, pan, buttons, level readouts and gaps.
    let base = 34.0 + 4.0 + 38.0 + 28.0 + 22.0 + 18.0 + 6.0;
    let layout = |sends: SendsRow, pan_text: bool, insert_rows: usize, send_rows: usize| {
        let mut fixed = base + insert_rows as f32 * ROW + send_rows as f32 * SEND_ROW;
        if pan_text {
            fixed += 14.0;
        }
        if route {
            fixed += ROW + 4.0;
        }
        match sends {
            SendsRow::None => {}
            SendsRow::Summary => fixed += ROW + 4.0,
            SendsRow::List => fixed += 24.0 + 4.0,
        }
        fixed
    };
    // When the minimum layout leaves the fader too short, give up the
    // send list, then the pan legend, then an insert row, in that order.
    let (mut sends, mut pan_text, mut insert_rows) = (sends, pan_text, 2usize);
    let mut send_rows = usize::from(sends == SendsRow::List);
    let short = |fixed: f32| height - fixed < FADER_MIN;
    if short(layout(sends, pan_text, insert_rows, send_rows)) && sends == SendsRow::List {
        sends = SendsRow::Summary;
        send_rows = 0;
    }
    if short(layout(sends, pan_text, insert_rows, send_rows)) {
        pan_text = false;
    }
    if short(layout(sends, pan_text, insert_rows, send_rows)) {
        insert_rows = 1;
    }
    let mut spare = height - layout(sends, pan_text, insert_rows, send_rows) - FADER_MIN;
    loop {
        if spare >= ROW && insert_rows < 6 {
            insert_rows += 1;
            spare -= ROW;
        } else if sends == SendsRow::List && spare >= SEND_ROW && send_rows < 3 {
            send_rows += 1;
            spare -= SEND_ROW;
        } else {
            break;
        }
    }
    Plan {
        width,
        meter_w,
        scale,
        route,
        pan_text,
        numeric,
        sends,
        insert_rows,
        send_rows,
    }
}

/// Peak hold shown as a number until clicked; latches a clip at 0 dBFS.
#[derive(Default)]
pub(crate) struct PeakHold {
    pub max: f32,
    pub clipped: bool,
}

/// Header tools: strip size, clear peaks and a toggle that gives the
/// mixer the whole window.
pub fn toolbar(ui: &mut Ui, tools: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let mut row = tools;
    row.cut_left(8.0);
    ui.divider_v(row.cut_left(1.0));
    row.cut_left(8.0);
    let full = ["Compact", "Expanded", "Full"];
    let short = ["S", "M", "L"];
    let labels: &[&str] = if row.w >= 300.0 { &full } else { &short };
    let seg = row.cut_left(if row.w >= 300.0 { 216.0 } else { 90.0 });
    let mut sel = MixerSize::ALL
        .iter()
        .position(|m| *m == ws.mixer_size)
        .unwrap_or(1);
    if ui.segmented(W::new("mixer-size"), seg, labels, &mut sel) {
        ws.mixer_size = MixerSize::ALL[sel];
    }
    row.cut_left(8.0);
    let clear = ui.icon_button(
        W::new("mixer-clear-peaks"),
        row.cut_left(28.0),
        icons::X,
        false,
        None,
    );
    ui.name(&clear, "Clear peaks");
    ui.tooltip(&clear, "Clear peak holds and clip lamps");
    if clear.clicked {
        let ids: Vec<Id> = s.project().mixer.tracks.iter().map(|t| t.id).collect();
        for id in ids {
            *ui.state::<PeakHold>(W::new(("peak-hold", id))) = PeakHold::default();
        }
    }
    let maxed = ws.mixer_restore_h.is_some();
    let fill = ui.icon_button(
        W::new("mixer-fill"),
        row.cut_left(28.0),
        if maxed {
            icons::CARET_DOWN
        } else {
            icons::CARET_UP
        },
        maxed,
        None,
    );
    ui.name(
        &fill,
        if maxed {
            "Restore mixer"
        } else {
            "Expand mixer"
        },
    );
    ui.tooltip(
        &fill,
        if maxed {
            "Restore the mixer to the dock"
        } else {
            "Expand the mixer over the whole workspace"
        },
    );
    if fill.clicked {
        // The playlist steps aside while the mixer is expanded; the dock
        // height is left as the user set it.
        ws.mixer_restore_h = if maxed { None } else { Some(ws.dock_h) };
    }
    let _ = t;
}

pub fn draw(ui: &mut Ui, r: Rect, tools: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    toolbar(ui, tools, s, ws);
    let mut body = r.shrink2(4.0, 0.0);
    body.cut_top(2.0);
    body.cut_bottom(4.0);
    let tracks: Vec<MixerTrack> = s.project().mixer.tracks.clone();
    let Some(master) = tracks.first().cloned() else {
        return;
    };
    let strips_h = (body.h - 8.0).max(80.0);
    let mut normal = plan(ws.mixer_size, strips_h, false);
    let mut master_plan = plan(ws.mixer_size, strips_h, true);
    // Spare height goes to the fader, not to empty insert rows: show only
    // as many rows as the fullest chain (plus the add row) needs.
    let rows_needed = tracks.iter().map(|t| t.inserts.len()).max().unwrap_or(0) + 1;
    normal.insert_rows = normal.insert_rows.min(rows_needed.max(2));
    master_plan.insert_rows = master_plan.insert_rows.min(rows_needed.max(2));

    let master_r = body.cut_right(master_plan.width + 6.0);
    body.cut_right(4.0);
    ui.scene.vline(
        master_r.x - 2.0,
        master_r.y,
        master_r.bottom(),
        t.line_strong,
    );

    let content_w = (tracks.len() - 1) as f32 * (normal.width + STRIP_GAP) + 48.0;
    let mut off = vec2(ws.mixer_scroll, 0.0);

    ui.scene.push_clip(body);
    let mut x = body.x - off.x;
    for tr in tracks.iter().skip(1) {
        let sr = Rect {
            x,
            y: body.y,
            w: normal.width,
            h: strips_h,
        };
        x += normal.width + STRIP_GAP;
        if sr.right() < body.x || sr.x > body.right() {
            continue;
        }
        strip(ui, sr, tr, false, normal, s, ws);
    }
    let add_r = Rect {
        x,
        y: body.y + 6.0,
        w: 36.0,
        h: 30.0,
    };
    let b = ui.icon_button(W::new("mixer-add"), add_r, icons::PLUS, false, None);
    ui.name(&b, "Add mixer track");
    ui.tooltip(&b, "Add mixer track");
    if b.clicked && !crate::util::mixer_full(s) {
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

    // Strips took any wheel they own (insert lists); what is left scrolls
    // the strips sideways.
    if ui.pointer_in(body)
        && ui.input.scroll.x == 0.0
        && ui.input.scroll.y != 0.0
        && !ui.input.modifiers.command()
        && !ui.any_active()
        && !ui.input.modifiers.shift
    {
        ui.input.scroll = vec2(ui.input.scroll.y, 0.0);
    }
    ui.scroll_area(
        W::new("mixer-scroll"),
        body,
        vec2(content_w, body.h),
        &mut off,
    );
    ws.mixer_scroll = off.x;

    strip(
        ui,
        master_r.shrink2(3.0, 0.0).with_h(strips_h),
        &master,
        true,
        master_plan,
        s,
        ws,
    );
}

fn strip(
    ui: &mut Ui,
    r: Rect,
    tr: &MixerTrack,
    is_master: bool,
    plan: Plan,
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
        .rect(r, 0.0, if selected { t.raised } else { t.panel_alt });
    // Effect drops.
    let is_effect = |i: &BrowserItem| match i {
        BrowserItem::Device(id) | BrowserItem::Preset(id, _) => {
            peel_devices::descriptor(id).is_some_and(|d| d.kind == peel_dsp::DeviceKind::Effect)
        }
        BrowserItem::Plugin { instrument, .. } => !*instrument,
        _ => false,
    };
    if ui.drop_hover::<BrowserItem>(r).is_some_and(is_effect) {
        ui.scene.outline(r, 0.0, 2.0, t.focus.alpha(0.7));
        if let Some((item, _)) = ui.take_drop::<BrowserItem>(r) {
            match item {
                BrowserItem::Device(id) => add_insert(s, tr.id, id),
                BrowserItem::Preset(id, preset) => add_insert_preset(s, tr.id, id, Some(preset)),
                BrowserItem::Plugin { type_id, name, .. } => {
                    add_plugin_insert(s, tr.id, &type_id, &name)
                }
                _ => {}
            }
            ws.focus = Some(Focus::Track(tr.id));
        }
    }

    let mut col_r = r.shrink2(6.0, 6.0);
    // The name sits on a plate in the channel color, like the scribble
    // strip of a console.
    col_r.cut_top(1.0);
    let name_r = col_r.cut_top(22.0);
    let plate = if is_master { t.accent } else { c };
    let plate = if selected {
        plate
    } else {
        t.panel_alt.mix(plate, 0.82)
    };
    ui.scene.rect(name_r, t.radius_s, plate);
    let name = ui.fit_text(&tr.name, t.label, name_r.w - 8.0);
    ui.text(name_r, &name, t.label, ink_on(plate), Align::Center);
    if !is_master {
        strip_menu(ui, name_r, tr, s, ws);
    } else {
        let hit = ui.interact(W::new(("strip-name", tr.id)), name_r);
        ui.tooltip(&hit, "Master");
    }
    col_r.cut_top(6.0);

    inserts(ui, &mut col_r, tr, plan, s, ws);
    col_r.cut_top(4.0);

    // Routing.
    if plan.route {
        let row = col_r.cut_top(ROW);
        col_r.cut_top(4.0);
        if is_master {
            mastering_button(ui, row, tr, plan, s, ws);
        } else {
            output_menu(ui, row, tr, s);
        }
    }
    match plan.sends {
        SendsRow::None => {}
        SendsRow::Summary => {
            let row = col_r.cut_top(ROW);
            col_r.cut_top(4.0);
            let label = match tr.sends.len() {
                0 => "Sends".to_string(),
                1 => "1 send".to_string(),
                n => format!("{n} sends"),
            };
            let id = W::new(("sends-open", tr.id));
            let resp = ui.dropdown(id, row, &label, "Sends");
            if resp.clicked {
                ui.open_popup(W::new(("send-menu", tr.id)), row);
            }
            send_menu(ui, tr, row, s);
        }
        SendsRow::List => sends_list(ui, &mut col_r, tr, plan, s),
    }

    // Pan.
    let pan_r = col_r.cut_top(34.0);
    let mut pan = (tr.pan + 1.0) * 0.5;
    let k = ui.knob(
        W::new(("strip-pan", tr.id)),
        Rect::from_center(pan_r.center(), 28.0, 28.0),
        &mut pan,
        0.5,
        t.text_dim,
        true,
    );
    ui.name(&k, &format!("{} pan", tr.name));
    ui.value_text(&k, &format_pan(tr.pan));
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
    if plan.pan_text {
        let pt = col_r.cut_top(14.0);
        ui.text(
            pt,
            &format_pan(tr.pan),
            t.caption,
            t.text_faint,
            Align::Center,
        );
    }
    col_r.cut_top(4.0);

    // Mute / solo.
    let mut ms = col_r.cut_bottom(24.0);
    let half = (ms.w - 4.0) / 2.0;
    let mute_r = ms.cut_left(half);
    ms.cut_left(4.0);
    let solo_r = ms;
    if toggle(
        ui,
        W::new(("mute", tr.id)),
        mute_r,
        "M",
        &format!("Mute {}", tr.name),
        tr.muted,
        t.warn,
    ) {
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
    if !is_master
        && toggle(
            ui,
            W::new(("solo", tr.id)),
            solo_r,
            "S",
            &format!("Solo {}", tr.name),
            tr.solo,
            t.focus,
        )
    {
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

    // Fader level: a number you can drag or type.
    let db_r = col_r.cut_bottom(18.0);
    if plan.numeric {
        let mut v = f64::from(tr.gain_db.max(-96.0));
        let resp = ui.drag_number(
            W::new(("gain-db", tr.id)),
            db_r,
            &mut v,
            -96.0,
            6.0,
            0.25,
            t.readout_small,
            if tr.gain_db > 0.05 {
                t.accent
            } else {
                t.text_dim
            },
            |v| {
                if v <= -95.0 {
                    "-inf".to_string()
                } else {
                    format!("{v:+.1}")
                }
            },
        );
        ui.name(&resp, &format!("{} level in dB", tr.name));
        if resp.changed {
            let tid = tr.id;
            let db = (v as f32 * 10.0).round() / 10.0;
            s.doc.edit_gesture(resp.id.0, "Change volume", |p| {
                if let Some(t) = p.mixer.track_mut(tid) {
                    t.gain_db = db;
                }
            });
        }
        if resp.released {
            s.doc.end_gesture();
        }
    } else {
        ui.text(
            db_r,
            &format_db(tr.gain_db),
            t.readout_small,
            t.text_dim,
            Align::Center,
        );
    }
    col_r.cut_bottom(2.0);

    // Peak hold readout and clip lamp.
    let peaks = track_peaks(ui, s, tr.id);
    let hold_id = W::new(("peak-hold", tr.id));
    {
        let hold = ui.state::<PeakHold>(hold_id);
        let p = peaks.0.max(peaks.1);
        hold.max = hold.max.max(p);
        if p >= 1.0 {
            hold.clipped = true;
        }
    }
    let (hold_max, clipped) = {
        let h = ui.state::<PeakHold>(hold_id);
        (h.max, h.clipped)
    };
    let pk_r = col_r.cut_bottom(16.0);
    let pk = ui.interact_node(
        W::new(("peak-reset", tr.id)),
        pk_r,
        FocusKind::Activate,
        Semantics::new(Role::Button)
            .label(format!(
                "{} peak {} dB, clear",
                tr.name,
                peak_text(hold_max)
            ))
            .toggled(clipped),
    );
    ui.scene.rect(
        pk_r,
        t.radius_s,
        if clipped {
            t.record
        } else if pk.hovered {
            t.control.alpha(0.6)
        } else {
            t.well
        },
    );
    ui.text(
        pk_r,
        &peak_text(hold_max),
        t.caption,
        if clipped {
            ink_on(t.record)
        } else {
            t.text_dim
        },
        Align::Center,
    );
    ui.tooltip(
        &pk,
        if clipped {
            "Clipped. Click to clear."
        } else {
            "Peak hold. Click to clear."
        },
    );
    if pk.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    if pk.clicked {
        *ui.state::<PeakHold>(hold_id) = PeakHold::default();
    }
    col_r.cut_bottom(2.0);

    // Fader, scale and meter.
    let mut fm = col_r;
    let meter_r = fm.cut_right(plan.meter_w);
    fm.cut_right(4.0);
    let scale_r = if plan.scale {
        fm.cut_left(24.0)
    } else {
        Rect::ZERO
    };
    let cap_h = 22.0;
    if plan.scale {
        let travel = (fm.h - cap_h).max(1.0);
        let mut last_y = f32::NEG_INFINITY;
        for db in [6.0f32, 0.0, -6.0, -12.0, -24.0, -48.0] {
            let y = fm.y + cap_h * 0.5 + (1.0 - db_to_fader(db)) * travel;
            if y - last_y < 12.0 {
                continue;
            }
            last_y = y;
            let label = if db == 0.0 {
                "0".to_string()
            } else {
                format!("{db:+}")
            };
            ui.text(
                Rect {
                    x: scale_r.x,
                    y: y - 7.0,
                    w: scale_r.w,
                    h: 14.0,
                },
                &label,
                t.caption.size(10.0),
                if db == 0.0 { t.text_dim } else { t.text_faint },
                Align::End,
            );
        }
    }
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
    ui.meter(W::new(("meter", tr.id)), meter_r, peaks);
    // Level ticks beside the meter at 0, -6, -12, -24 and -48 dB.
    if plan.scale {
        let mt = Rect {
            x: meter_r.x - 3.0,
            w: 3.0,
            ..meter_r
        };
        for db in [0.0f32, -6.0, -12.0, -24.0, -48.0] {
            let y = meter_r.bottom() - meter_r.h * meter_pos(10f32.powf(db / 20.0));
            ui.scene.rect(
                Rect {
                    x: mt.x,
                    y: y - 0.5,
                    w: mt.w,
                    h: 1.0,
                },
                0.0,
                t.line_strong,
            );
        }
    }
}

/// The insert chain: every insert in a scrolling list ending in an add row.
fn inserts(
    ui: &mut Ui,
    col_r: &mut Rect,
    tr: &MixerTrack,
    plan: Plan,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let view = col_r.cut_top(plan.insert_rows as f32 * ROW);
    let rows = tr.inserts.len() + 1;
    let content_h = rows as f32 * ROW;
    let overflow = content_h > view.h + 0.5;
    let scroll_id = W::new(("insert-scroll", tr.id));
    let mut off = vec2(0.0, *ui.state::<f32>(scroll_id));
    if overflow {
        ui.scroll_area(scroll_id, view, vec2(view.w, content_h), &mut off);
    } else {
        off = vec2(0.0, 0.0);
    }
    *ui.state::<f32>(scroll_id) = off.y;
    // Keep the newest insert in view after adding one.
    ui.scene.push_clip(view);
    let bar = if overflow { 8.0 } else { 0.0 };
    for i in 0..rows {
        let slot_r = Rect {
            x: view.x,
            y: view.y + i as f32 * ROW - off.y,
            w: view.w - bar,
            h: ROW - 1.0,
        };
        if slot_r.bottom() < view.y || slot_r.y > view.bottom() {
            continue;
        }
        match tr.inserts.get(i) {
            Some(dev) => insert_row(ui, slot_r, tr, i, dev, s, ws),
            None => {
                let id = W::new(("insert", tr.id, i));
                let resp = ui.interact_node(
                    id,
                    slot_r,
                    FocusKind::Activate,
                    Semantics::new(Role::Button).label(format!("Add effect to {}", tr.name)),
                );
                let hot = ui.animate_bool(id, resp.hovered);
                ui.scene
                    .rect(slot_r, t.radius_s, t.well.mix(t.control, hot));
                ui.icon(slot_r, icons::PLUS, 11.0, t.text_faint.mix(t.text, hot));
                ui.focus_ring(id, slot_r, t.radius_s);
                if resp.clicked {
                    ui.open_popup(W::new(("add-fx", tr.id)), slot_r);
                }
                if resp.hovered {
                    ui.set_cursor(CursorIcon::Pointer);
                }
                ui.tooltip(&resp, "Add effect");
                let effects: Vec<(&'static str, &'static str)> = peel_devices::effects()
                    .map(|d| (d.type_id, d.name))
                    .collect();
                let items: Vec<MenuItem> = effects.iter().map(|(_, n)| MenuItem::new(*n)).collect();
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
    ui.scene.pop_clip();
}

fn insert_row(
    ui: &mut Ui,
    slot_r: Rect,
    tr: &MixerTrack,
    i: usize,
    dev: &peel_document::DeviceInstance,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let id = W::new(("insert", tr.id, i));
    let name = peel_devices::descriptor(&dev.type_id).map_or("Missing", |d| d.name);
    let open_here = ws.fx.target.is_some_and(|f| f.device == dev.id);
    let resp = ui.interact_node(
        id,
        slot_r,
        FocusKind::Activate,
        Semantics::new(Role::Button)
            .label(format!("{name}, {}", tr.name))
            .value(if dev.bypass { "bypassed" } else { "on" }),
    );
    // Insert slots are recessed; the open one and the hovered one lift.
    let fill = if open_here {
        t.control_active
    } else if resp.hovered {
        t.control
    } else {
        t.well
    };
    ui.scene.rect(slot_r, t.radius_s, fill);
    ui.focus_ring(id, slot_r, t.radius_s);
    let mut inner = slot_r;
    let dot_r = inner.cut_left(20.0);
    let dot = ui.interact_node(
        W::new(("insert-bypass", tr.id, i)),
        dot_r,
        FocusKind::Activate,
        Semantics::new(Role::Button)
            .label(format!("Bypass {name}"))
            .toggled(dev.bypass),
    );
    ui.scene.circle(
        dot_r.center(),
        if dot.hovered { 4.0 } else { 3.0 },
        if dev.bypass { t.text_faint } else { t.play },
    );
    ui.tooltip(&dot, if dev.bypass { "Enable" } else { "Bypass" });
    let label = ui.fit_text(name, t.caption, inner.w - 4.0);
    ui.text(
        inner,
        &label,
        t.caption,
        if dev.bypass { t.text_faint } else { t.text },
        Align::Start,
    );
    if label != name {
        ui.tooltip(&resp, name);
    }
    if dot.clicked {
        toggle_bypass(s, tr.id, dev.id, dev.bypass);
    } else if resp.clicked {
        ws.focus = Some(Focus::Track(tr.id));
        ws.fx.open(tr.id, dev.id);
    }
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    let menu = W::new(("insert-menu", tr.id, i));
    if resp.secondary_clicked {
        ui.open_popup(menu, slot_r);
    }
    let items = [
        MenuItem::new("Open editor"),
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
            0 => {
                ws.focus = Some(Focus::Track(tid));
                ws.fx.open(tid, did);
            }
            1 => toggle_bypass(s, tid, did, dev.bypass),
            2 => s.doc.edit("Move effect", |p| {
                if let Some(t) = p.mixer.track_mut(tid) {
                    t.inserts.swap(i, i - 1);
                }
            }),
            3 => s.doc.edit("Move effect", |p| {
                if let Some(t) = p.mixer.track_mut(tid) {
                    t.inserts.swap(i, i + 1);
                }
            }),
            5 => remove_insert(s, ws, tid, did, name),
            _ => {}
        }
    }
}

pub fn toggle_bypass(s: &mut Session, track: Id, device: Id, bypassed: bool) {
    s.doc.edit(
        if bypassed {
            "Enable effect"
        } else {
            "Bypass effect"
        },
        |p| {
            if let Some(d) = p
                .mixer
                .track_mut(track)
                .and_then(|t| t.inserts.iter_mut().find(|d| d.id == device))
            {
                d.bypass = !d.bypass;
            }
        },
    );
}

pub fn remove_insert(s: &mut Session, ws: &mut Workspace, track: Id, device: Id, name: &str) {
    s.doc.edit(format!("Remove {name}"), |p| {
        if let Some(t) = p.mixer.track_mut(track) {
            t.inserts.retain(|d| d.id != device);
        }
    });
    if ws.fx.target.is_some_and(|f| f.device == device) {
        ws.fx.close();
    }
}

/// Output routing dropdown: master or any track that does not feed back.
fn output_menu(ui: &mut Ui, row: Rect, tr: &MixerTrack, s: &mut Session) {
    let p = s.project();
    let master = p.mixer.tracks[0].id;
    let out_id = tr.output.unwrap_or(master);
    let out_name = p
        .mixer
        .track(out_id)
        .map_or("Master".to_string(), |t| t.name.clone());
    let dests: Vec<(Id, String)> = p
        .mixer
        .tracks
        .iter()
        .filter(|t| t.id != tr.id && !crate::inspector::reaches(p, t.id, tr.id))
        .map(|t| (t.id, t.name.clone()))
        .collect();
    let id = W::new(("out-route", tr.id));
    let resp = ui.dropdown(id, row, &out_name, "Output");
    ui.tooltip(&resp, &format!("Output: {out_name}"));
    if resp.clicked {
        ui.open_popup(id.with("menu"), row);
    }
    let items: Vec<MenuItem> = dests
        .iter()
        .map(|(d, n)| MenuItem::new(n.clone()).checked(*d == out_id))
        .collect();
    if let Some(i) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 2.0), &items) {
        let dest = dests[i].0;
        let tid = tr.id;
        s.doc.edit("Route mixer track", |p| {
            if let Some(t) = p.mixer.track_mut(tid) {
                t.output = if dest == master { None } else { Some(dest) };
            }
        });
    }
}

/// Targets a track may send to: not itself, not master, nothing that
/// feeds it.
fn send_targets(s: &Session, tr: &MixerTrack) -> Vec<(Id, String)> {
    let p = s.project();
    p.mixer
        .tracks
        .iter()
        .skip(1)
        .filter(|x| x.id != tr.id)
        .filter(|x| {
            tr.sends.iter().any(|sd| sd.target == x.id)
                || !crate::inspector::reaches(p, x.id, tr.id)
        })
        .map(|x| (x.id, x.name.clone()))
        .collect()
}

/// Menu of send destinations; a checked one removes its send.
fn send_menu(ui: &mut Ui, tr: &MixerTrack, anchor: Rect, s: &mut Session) {
    let targets = send_targets(s, tr);
    let items: Vec<MenuItem> = if targets.is_empty() {
        vec![MenuItem::new("Add a mixer track first").enabled(false)]
    } else {
        targets
            .iter()
            .map(|(id, n)| {
                MenuItem::new(n.clone()).checked(tr.sends.iter().any(|sd| sd.target == *id))
            })
            .collect()
    };
    if let Some(i) = ui.menu(
        W::new(("send-menu", tr.id)),
        vec2(anchor.x, anchor.bottom() + 2.0),
        &items,
    ) && let Some((target, name)) = targets.get(i).cloned()
    {
        let tid = tr.id;
        let exists = tr.sends.iter().any(|sd| sd.target == target);
        s.doc.edit(
            if exists {
                format!("Remove send to {name}")
            } else {
                format!("Send to {name}")
            },
            |p| {
                if let Some(x) = p.mixer.track_mut(tid) {
                    if exists {
                        x.sends.retain(|sd| sd.target != target);
                    } else {
                        x.sends.push(peel_document::MixerSend {
                            target,
                            gain_db: -6.0,
                        });
                    }
                }
            },
        );
    }
}

/// Full strips list each send with an editable level.
fn sends_list(ui: &mut Ui, col_r: &mut Rect, tr: &MixerTrack, plan: Plan, s: &mut Session) {
    let t = ui.theme.clone();
    let mut head = col_r.cut_top(24.0);
    let add = ui.icon_button(
        W::new(("send-add", tr.id)),
        head.cut_right(24.0),
        icons::PLUS,
        false,
        None,
    );
    ui.name(&add, &format!("Send from {}", tr.name));
    ui.tooltip(&add, "Send to a track");
    if add.clicked {
        ui.open_popup(W::new(("send-menu", tr.id)), add.rect);
    }
    ui.text(head, "Sends", t.caption, t.text_faint, Align::Start);
    send_menu(ui, tr, add.rect, s);
    let view = col_r.cut_top(plan.send_rows as f32 * SEND_ROW);
    col_r.cut_top(4.0);
    let content_h = tr.sends.len() as f32 * SEND_ROW;
    let overflow = content_h > view.h + 0.5;
    let scroll_id = W::new(("send-scroll", tr.id));
    let mut off = vec2(0.0, *ui.state::<f32>(scroll_id));
    if overflow {
        ui.scroll_area(scroll_id, view, vec2(view.w, content_h), &mut off);
    } else {
        off.y = 0.0;
    }
    *ui.state::<f32>(scroll_id) = off.y;
    if tr.sends.is_empty() {
        ui.text(view, "None", t.caption, t.text_faint, Align::Start);
    }
    ui.scene.push_clip(view);
    for (i, sd) in tr.sends.iter().enumerate() {
        let mut row = Rect {
            x: view.x,
            y: view.y + i as f32 * SEND_ROW - off.y,
            w: view.w - if overflow { 8.0 } else { 0.0 },
            h: SEND_ROW - 2.0,
        };
        if row.bottom() < view.y || row.y > view.bottom() {
            continue;
        }
        let dest = s.project().mixer.track(sd.target);
        let (name, color) = dest.map_or(("Missing".to_string(), t.text_faint), |d| {
            (d.name.clone(), col(d.color))
        });
        let x = ui.icon_button(
            W::new(("send-remove", tr.id, i)),
            row.cut_right(18.0),
            icons::X,
            false,
            None,
        );
        ui.name(&x, &format!("Remove send to {name}"));
        let num = row.cut_right(42.0);
        let shown = ui.fit_text(&name, t.caption, row.w - 4.0);
        ui.text(row, &shown, t.caption, color, Align::Start);
        let mut v = f64::from(sd.gain_db.max(-60.0));
        let resp = ui.drag_number(
            W::new(("send-db", tr.id, i)),
            num,
            &mut v,
            -60.0,
            6.0,
            0.25,
            t.caption,
            t.text_dim,
            |v| format!("{v:+.1}"),
        );
        ui.name(&resp, &format!("Send level to {name}"));
        if resp.changed {
            let tid = tr.id;
            let db = (v as f32 * 10.0).round() / 10.0;
            s.doc.edit_gesture(resp.id.0, "Change send level", |p| {
                if let Some(x) = p.mixer.track_mut(tid)
                    && let Some(send) = x.sends.get_mut(i)
                {
                    send.gain_db = db;
                }
            });
        }
        if resp.released {
            s.doc.end_gesture();
        }
        if x.clicked {
            let tid = tr.id;
            s.doc.edit("Remove send", |p| {
                if let Some(t) = p.mixer.track_mut(tid)
                    && i < t.sends.len()
                {
                    t.sends.remove(i);
                }
            });
        }
    }
    ui.scene.pop_clip();
}

/// Master's Mastering button: adds the plugin once, then opens it.
fn mastering_button(
    ui: &mut Ui,
    row: Rect,
    master: &MixerTrack,
    plan: Plan,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let id = W::new("master-mastering");
    let exists = master.inserts.iter().find(|d| d.type_id == MASTER_TYPE);
    let available = peel_devices::descriptor(MASTER_TYPE).is_some();
    let resp = ui.interact_node(
        id,
        row,
        FocusKind::Activate,
        Semantics::new(Role::Button).label("Mastering"),
    );
    let hot = ui.animate_bool(id, resp.hovered && available);
    let on = exists.is_some_and(|d| ws.fx.target.is_some_and(|f| f.device == d.id));
    ui.scene.rect(
        row,
        t.radius_s,
        if on {
            t.accent
        } else {
            t.control.mix(t.control_hover, hot)
        },
    );
    let ink = if on {
        t.text_on_accent
    } else if available {
        t.text
    } else {
        t.text_faint
    };
    let mut inner = row.shrink2(4.0, 0.0);
    if plan.width > 70.0 {
        ui.icon(inner.cut_left(16.0), icons::SLIDERS, 14.0, ink);
        inner.cut_left(4.0);
        ui.text(inner, "Mastering", t.label, ink, Align::Start);
    } else {
        ui.icon(row, icons::SLIDERS, 14.0, ink);
    }
    ui.focus_ring(id, row, t.radius_s);
    ui.tooltip(
        &resp,
        if !available {
            "Mastering is not installed"
        } else if exists.is_some() {
            "Open the Mastering editor"
        } else {
            "Add Mastering to the master and open it"
        },
    );
    if resp.hovered && available {
        ui.set_cursor(CursorIcon::Pointer);
    }
    if resp.clicked && available {
        open_mastering(s, ws);
    }
}

/// Adds Mastering to the end of the master chain when missing, then opens
/// its editor. Returns the device id.
pub fn open_mastering(s: &mut Session, ws: &mut Workspace) -> Option<Id> {
    peel_devices::descriptor(MASTER_TYPE)?;
    let master = s.project().mixer.tracks[0].id;
    let existing = s.project().mixer.tracks[0]
        .inserts
        .iter()
        .find(|d| d.type_id == MASTER_TYPE)
        .map(|d| d.id);
    let id = match existing {
        Some(id) => id,
        None => {
            let dev = template::device(MASTER_TYPE, None);
            let id = dev.id;
            s.doc
                .edit("Add Mastering", |p| p.mixer.tracks[0].inserts.push(dev));
            id
        }
    };
    ws.focus = Some(Focus::Track(master));
    ws.fx.open(master, id);
    Some(id)
}

fn toggle(ui: &mut Ui, id: W, r: Rect, label: &str, name: &str, on: bool, color: Color) -> bool {
    let t = ui.theme.clone();
    let resp = ui.interact_node(
        id,
        r,
        FocusKind::Activate,
        Semantics::new(Role::Button).label(name).toggled(on),
    );
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
    ui.focus_ring(id, r, t.radius_s);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    ui.tooltip(&resp, name);
    resp.clicked
}

pub fn add_insert(s: &mut Session, track: Id, type_id: &str) {
    add_insert_preset(s, track, type_id, None);
}

/// Adds an installed plugin effect as an insert.
pub fn add_plugin_insert(s: &mut Session, track: Id, type_id: &str, name: &str) {
    let dev = peel_document::DeviceInstance::new(type_id, 1);
    s.doc.edit(format!("Add {name}"), |p| {
        if let Some(t) = p.mixer.track_mut(track) {
            t.inserts.push(dev);
        }
    });
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
