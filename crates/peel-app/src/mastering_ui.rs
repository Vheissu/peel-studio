//! Mastering editor. Simple and Advanced are two views of the same 13
//! parameters: switching changes the layout, never a value.
//!
//! The signal path is fixed and shown as a chain: Input, Tone, Compress,
//! Width, Limit. Meters are real: Peak and RMS (dBFS, not LUFS and not a
//! true-peak measurement) from the analysis tap at the effect's input and
//! output, and gain reduction from the effect's own telemetry.

use peel_document::{DeviceInstance, Id};
use peel_dsp::{DeviceDescriptor, ParamDescriptor};
use peel_session::Session;
use peel_ui::Id as W;
use peel_ui::{Align, Color, CursorIcon, FocusKind, Rect, Role, Semantics, Ui, Vec2, vec2};

use crate::analysis_view::FLOOR_DB;
use crate::eq_editor::{load_preset, reset_all};
use crate::util::reduction_db;
use crate::workspace::Workspace;

/// Seconds of gain-reduction history kept (at the frame rate).
const HISTORY: usize = 360;
/// Gain-reduction scale of the history plot, dB.
const GR_SCALE: f32 = 12.0;
/// Level meters span this range, dBFS.
const METER_FLOOR: f32 = -60.0;

const STAGES: [&str; 5] = ["Input", "Tone", "Compress", "Width", "Limit"];

#[derive(Default)]
pub struct MasteringState {
    pub advanced: bool,
    /// Selected chain stage (Advanced shows its controls).
    pub stage: usize,
    gr_history: Vec<f32>,
}

fn value(dev: &DeviceInstance, desc: &DeviceDescriptor, key: &str) -> f32 {
    dev.params
        .get(key)
        .copied()
        .or_else(|| desc.param_by_key(key).map(|p| p.default))
        .unwrap_or(0.0)
}

pub fn draw(
    ui: &mut Ui,
    body: Rect,
    _track: Id,
    dev: &DeviceInstance,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let Some(desc) = peel_devices::descriptor("peel.fx.mastering") else {
        return;
    };
    // Gain reduction is read once and drawn in several places.
    let gr = reduction_db(ui, s, dev.id);
    {
        let h = &mut ws.fx.mastering.gr_history;
        if ui.dt() > 0.0 {
            h.push(gr);
            if h.len() > HISTORY {
                h.remove(0);
            }
        }
    }
    if gr > 0.01 || ws.fx.mastering.gr_history.iter().any(|v| *v > 0.01) {
        ui.request_repaint();
    }

    let mut c = body;
    top_bar(ui, c.cut_top(32.0), dev, desc, s, ws);
    c.cut_top(10.0);
    // The chain strip is the Advanced view's map; Simple keeps to the
    // handful of controls that finish a mix.
    if ws.fx.mastering.advanced {
        chain(ui, c.cut_top(48.0), dev, desc, ws);
        c.cut_top(12.0);
    }
    let rail = c.cut_right(264.0);
    c.cut_right(16.0);
    meters(ui, rail, dev, desc, ws, gr);

    // Controls.
    let advanced = ws.fx.mastering.advanced;
    let controls_h = if advanced { 128.0 } else { 150.0 };
    let controls = c.cut_top(controls_h);
    c.cut_top(14.0);
    if advanced {
        stage_controls(ui, controls, dev, desc, s, ws);
    } else {
        simple_controls(ui, controls, dev, desc, s);
    }
    ui.scene.hline(c.x, c.right(), c.y - 7.0, t.line);

    if !advanced {
        let c = Rect {
            h: c.h.min(190.0),
            ..c
        };
        gr_history(ui, c, &ws.fx.mastering.gr_history, gr);
        return;
    }
    // Real transfer function of the tone stage and the reduction history.
    let half = (c.w - 16.0) * 0.5;
    let tone_r = Rect { w: half, ..c };
    let hist_r = Rect {
        x: c.right() - half,
        w: half,
        ..c
    };
    tone_plot(ui, tone_r, dev, desc, s);
    gr_history(ui, hist_r, &ws.fx.mastering.gr_history, gr);
}

fn top_bar(
    ui: &mut Ui,
    row: Rect,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let mut row = row;
    let presets = peel_devices::presets(desc.type_id);
    for (i, p) in presets.iter().enumerate() {
        let w = ui.measure(p.name, t.label).x + 24.0;
        let r = row.cut_left(w);
        row.cut_left(4.0);
        let on = dev.preset.as_deref() == Some(p.name);
        let id = W::new(("master-preset", dev.id, i));
        let resp = ui.interact_node(
            id,
            r,
            FocusKind::Activate,
            Semantics::new(Role::Button)
                .label(format!("Preset {}", p.name))
                .toggled(on),
        );
        ui.scene.rect(
            r,
            t.radius_m,
            if on {
                t.accent
            } else if resp.hovered {
                t.control_hover
            } else {
                t.control
            },
        );
        ui.text(
            r,
            p.name,
            t.label,
            if on { t.text_on_accent } else { t.text },
            Align::Center,
        );
        ui.focus_ring(id, r, t.radius_m);
        if resp.hovered {
            ui.set_cursor(CursorIcon::Pointer);
        }
        if resp.clicked {
            load_preset(s, dev, desc, i);
        }
    }
    row.cut_left(8.0);
    let reset = ui.button(
        W::new(("master-reset", dev.id)),
        row.cut_left(64.0),
        "Reset",
    );
    ui.tooltip(&reset, "Return every control to its default");
    if reset.clicked {
        reset_all(s, dev, desc);
    }
    let seg = row.cut_right(190.0);
    let mut sel = usize::from(ws.fx.mastering.advanced);
    if ui.segmented(
        W::new("master-mode"),
        seg,
        &["Simple", "Advanced"],
        &mut sel,
    ) {
        ws.fx.mastering.advanced = sel == 1;
    }
}

/// The five stages with what each is doing. Clicking one selects it and
/// shows its controls in the Advanced view.
fn chain(
    ui: &mut Ui,
    row: Rect,
    dev: &DeviceInstance,
    desc: &DeviceDescriptor,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let v = |k: &str| value(dev, desc, k);
    let summaries = [
        format!("{:+.1} dB", v("input")),
        format!("{:+.1}  {:+.1}  {:+.1} dB", v("low"), v("mid"), v("high")),
        format!("{:.1}:1 above {:.0} dB", v("ratio"), v("threshold")),
        format!("{:.0}%", v("width") * 100.0),
        format!("Ceiling {:.1} dB", v("ceiling")),
    ];
    let arrow = 16.0;
    let cell_w = (row.w - arrow * 4.0) / 5.0;
    let mut x = row.x;
    for (i, name) in STAGES.iter().enumerate() {
        let cell = Rect {
            x,
            y: row.y,
            w: cell_w,
            h: row.h,
        };
        x += cell_w + arrow;
        let on = ws.fx.mastering.advanced && ws.fx.mastering.stage == i;
        let id = W::new(("master-stage", dev.id, i));
        let resp = ui.interact_node(
            id,
            cell,
            FocusKind::Activate,
            Semantics::new(Role::Tab)
                .label(format!("{name} stage"))
                .value(summaries[i].clone())
                .selected(on),
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
        if on {
            ui.scene.rect(
                Rect {
                    x: cell.x + 8.0,
                    y: cell.bottom() - 2.0,
                    w: cell.w - 16.0,
                    h: 2.0,
                },
                1.0,
                t.accent,
            );
        }
        let mut inner = cell.shrink2(10.0, 6.0);
        let head = inner.cut_top(18.0);
        ui.text(head, name, t.label, t.text, Align::Start);
        ui.text(inner, &summaries[i], t.caption, t.text_dim, Align::Start);
        ui.focus_ring(id, cell, t.radius_m);
        if resp.hovered {
            ui.set_cursor(CursorIcon::Pointer);
        }
        if resp.clicked {
            ws.fx.mastering.stage = i;
            ws.fx.mastering.advanced = true;
        }
        if i < 4 {
            ui.text(
                Rect {
                    x: cell.right(),
                    w: arrow,
                    ..cell
                },
                ">",
                t.label,
                t.text_faint,
                Align::Center,
            );
        }
    }
}

/// One real parameter as a knob with its name and value beneath.
fn knob(
    ui: &mut Ui,
    dev: &DeviceInstance,
    desc: &DeviceDescriptor,
    s: &mut Session,
    key: &str,
    cell: Rect,
    color: Color,
) {
    let t = ui.theme.clone();
    let Some(p) = desc.param_by_key(key) else {
        return;
    };
    if p.is_choice() {
        toggle(ui, dev, desc, s, p, cell, color);
        return;
    }
    let v = value(dev, desc, key);
    let text = s.param_text(dev.id, p, v);
    let mut c = cell;
    let kr = c.cut_top(cell.h - 36.0);
    let d = kr.h.min(kr.w).min(84.0);
    let mut norm = p.to_normalized(v);
    let resp = ui.knob(
        W::new(("mknob", dev.id, p.id)),
        Rect::from_center(kr.center(), d, d),
        &mut norm,
        p.to_normalized(p.default),
        color,
        p.min < 0.0 && p.max > 0.0,
    );
    ui.name(&resp, p.name);
    ui.value_text(&resp, &text);
    let extra = match key {
        // Gain after this effect can still push the signal past the ceiling.
        "ceiling" => " Applies at this effect; faders and effects after it can exceed it.",
        _ => "",
    };
    ui.tooltip(
        &resp,
        &format!("{}: {text}. Double-click to reset.{extra}", p.name),
    );
    if resp.changed {
        s.set_param(dev.id, p.key, p.from_normalized(norm), resp.id.0);
    }
    if resp.released {
        s.doc.end_gesture();
    }
    ui.text(
        c.cut_top(16.0),
        p.name,
        t.caption,
        t.text_dim,
        Align::Center,
    );
    ui.text(
        c.cut_top(18.0),
        &text,
        t.readout_small,
        t.text,
        Align::Center,
    );
}

fn toggle(
    ui: &mut Ui,
    dev: &DeviceInstance,
    desc: &DeviceDescriptor,
    s: &mut Session,
    p: &ParamDescriptor,
    cell: Rect,
    color: Color,
) {
    let t = ui.theme.clone();
    let on = value(dev, desc, p.key) >= 0.5;
    let mut c = cell;
    let kr = c.cut_top(cell.h - 36.0);
    let id = W::new(("mtoggle", dev.id, p.id));
    let r = Rect::from_center(kr.center(), 44.0, 26.0);
    let resp = ui.interact_node(
        id,
        r,
        FocusKind::Activate,
        Semantics::new(Role::Button).label(p.name).toggled(on),
    );
    ui.scene.rect(r, 13.0, if on { color } else { t.control });
    let knob_x = if on { r.right() - 13.0 } else { r.x + 13.0 };
    ui.scene.circle(
        vec2(knob_x, r.center().y),
        9.0,
        if on { t.text_on_accent } else { t.text_dim },
    );
    ui.focus_ring(id, r, 13.0);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    if resp.clicked {
        s.set_param(
            dev.id,
            p.key,
            if on { 0.0 } else { 1.0 },
            id.0 ^ u64::from(on),
        );
        s.doc.end_gesture();
    }
    ui.text(
        c.cut_top(16.0),
        p.name,
        t.caption,
        t.text_dim,
        Align::Center,
    );
    ui.text(
        c.cut_top(18.0),
        if on { "On" } else { "Off" },
        t.readout_small,
        t.text,
        Align::Center,
    );
}

/// Lays `groups` (a heading and its keys) across `r`, sized by knob count.
fn group_row(
    ui: &mut Ui,
    r: Rect,
    groups: &[(&str, &[&str])],
    dev: &DeviceInstance,
    desc: &DeviceDescriptor,
    s: &mut Session,
) {
    let t = ui.theme.clone();
    let total: usize = groups.iter().map(|g| g.1.len()).sum();
    let gap = 18.0;
    let knob_w = ((r.w - gap * (groups.len() as f32 - 1.0)) / total as f32).min(130.0);
    let mut x = r.x;
    for (gi, (name, keys)) in groups.iter().enumerate() {
        let w = knob_w * keys.len() as f32;
        let g = Rect {
            x,
            y: r.y,
            w,
            h: r.h,
        };
        x += w + gap;
        let mut inner = g;
        let head = inner.cut_top(22.0);
        ui.text(head, name, t.label, t.text_dim, Align::Start);
        ui.scene.hline(g.x, g.right(), head.bottom() - 1.0, t.line);
        inner.cut_top(4.0);
        for (ki, key) in keys.iter().enumerate() {
            let cell = Rect {
                x: inner.x + ki as f32 * knob_w,
                y: inner.y,
                w: knob_w,
                h: inner.h,
            };
            knob(ui, dev, desc, s, key, cell, t.accent);
        }
        let _ = gi;
    }
}

fn simple_controls(
    ui: &mut Ui,
    r: Rect,
    dev: &DeviceInstance,
    desc: &DeviceDescriptor,
    s: &mut Session,
) {
    group_row(
        ui,
        r,
        &[
            ("Input", &["input"]),
            ("Tone", &["low", "high"]),
            ("Glue", &["threshold"]),
            ("Stereo", &["width"]),
            ("Limiter", &["ceiling"]),
        ],
        dev,
        desc,
        s,
    );
}

fn stage_controls(
    ui: &mut Ui,
    r: Rect,
    dev: &DeviceInstance,
    desc: &DeviceDescriptor,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let (name, keys): (&str, &[&str]) = match ws.fx.mastering.stage {
        0 => ("Input", &["input"]),
        1 => ("Tone", &["low", "mid", "high"]),
        2 => (
            "Compress",
            &["threshold", "ratio", "attack", "release", "makeup"],
        ),
        3 => ("Width", &["width"]),
        _ => ("Limit", &["ceiling", "limiter_release", "true_peak"]),
    };
    let mut r = r;
    if ws.fx.mastering.stage == 2 && r.w > 700.0 {
        let curve = r.cut_right(150.0);
        r.cut_right(16.0);
        compressor_curve(ui, curve, dev, desc);
    }
    group_row(ui, r, &[(name, keys)], dev, desc, s);
}

/// Static curve of the glue compressor: output level against input level
/// for the current threshold, ratio and makeup (the knee is fixed at 12 dB
/// by the effect).
fn compressor_curve(ui: &mut Ui, r: Rect, dev: &DeviceInstance, desc: &DeviceDescriptor) {
    let t = ui.theme.clone();
    const KNEE: f32 = 12.0;
    let thr = value(dev, desc, "threshold");
    let ratio = value(dev, desc, "ratio").max(1.0);
    let makeup = value(dev, desc, "makeup");
    let mut c = r;
    ui.text(
        c.cut_top(18.0),
        "Static curve",
        t.label,
        t.text_dim,
        Align::Start,
    );
    c.cut_top(4.0);
    let side = c.w.min(c.h);
    let plot = Rect {
        x: c.x,
        y: c.y,
        w: side,
        h: side,
    };
    ui.scene.rect(plot, t.radius_m, t.well);
    let (lo, hi) = (-48.0f32, 0.0f32);
    let x_of = |db: f32| plot.x + plot.w * (db - lo) / (hi - lo);
    let y_of = |db: f32| plot.bottom() - plot.h * (db.clamp(lo, hi) - lo) / (hi - lo);
    for db in [-36.0f32, -24.0, -12.0] {
        ui.scene.vline(x_of(db), plot.y, plot.bottom(), t.grid_beat);
        ui.scene.hline(plot.x, plot.right(), y_of(db), t.grid_beat);
    }
    ui.scene
        .vline(x_of(thr), plot.y, plot.bottom(), t.accent.alpha(0.5));
    let out = |x: f32| {
        let over = x - thr;
        let y = if 2.0 * over < -KNEE {
            x
        } else if 2.0 * over.abs() <= KNEE {
            x + (1.0 / ratio - 1.0) * (over + KNEE / 2.0).powi(2) / (2.0 * KNEE)
        } else {
            thr + over / ratio
        };
        y + makeup
    };
    let pts: Vec<Vec2> = (0..=48)
        .map(|i| {
            let x = lo + (hi - lo) * i as f32 / 48.0;
            vec2(x_of(x), y_of(out(x)))
        })
        .collect();
    ui.scene.push_clip(plot);
    ui.scene.line(
        vec2(plot.x, plot.bottom()),
        vec2(plot.right(), plot.y),
        1.0,
        t.grid_bar,
    );
    ui.scene.polyline(&pts, 2.0, t.text);
    ui.scene.pop_clip();
}

// ---- meters ----

fn level_bar(ui: &mut Ui, r: Rect, rms: f32, peak: f32, marker: Option<f32>) {
    let t = ui.theme.clone();
    ui.scene.rect(r, 3.0, t.well);
    let x_of = |db: f32| r.x + r.w * ((db.clamp(METER_FLOOR, 0.0) - METER_FLOOR) / -METER_FLOOR);
    if rms > METER_FLOOR {
        ui.scene.rect(
            Rect {
                w: x_of(rms) - r.x,
                ..r
            },
            3.0,
            t.play.alpha(0.85),
        );
    }
    if peak > METER_FLOOR {
        ui.scene.rect(
            Rect {
                x: x_of(peak) - 1.5,
                w: 3.0,
                ..r
            },
            0.0,
            if peak >= -0.1 { t.record } else { t.text },
        );
    }
    for db in [-48.0, -36.0, -24.0, -12.0, -6.0] {
        ui.scene
            .vline(x_of(db), r.bottom() - 4.0, r.bottom(), t.line_strong);
    }
    if let Some(m) = marker {
        ui.scene.rect(
            Rect {
                x: x_of(m) - 0.5,
                y: r.y - 2.0,
                w: 1.5,
                h: r.h + 4.0,
            },
            0.0,
            t.accent,
        );
    }
}

fn meters(
    ui: &mut Ui,
    rail: Rect,
    dev: &DeviceInstance,
    desc: &DeviceDescriptor,
    ws: &mut Workspace,
    gr: f32,
) {
    let t = ui.theme.clone();
    let an = &ws.fx.analysis;
    let mut r = rail;
    let mut head = r.cut_top(20.0);
    ui.text(
        head.cut_right(40.0),
        "dBFS",
        t.caption,
        t.text_faint,
        Align::End,
    );
    ui.text(head, "Levels", t.label, t.text_dim, Align::Start);
    ui.scene.hline(r.x, r.right(), r.y, t.line);
    r.cut_top(8.0);
    let ceiling = value(dev, desc, "ceiling");
    let live = an.live(ui.time());
    for (name, rms, peak, marker) in [
        ("In", an.input_rms_db, an.input_peak_db, None),
        ("Out", an.output_rms_db, an.output_peak_db, Some(ceiling)),
    ] {
        let mut head = r.cut_top(18.0);
        ui.text(head.cut_left(30.0), name, t.label, t.text, Align::Start);
        let fig = |v: f32| {
            if v <= FLOOR_DB + 1.0 {
                "-inf".to_string()
            } else {
                format!("{v:.1}")
            }
        };
        ui.text(
            head,
            &format!("Peak {}  RMS {}", fig(peak), fig(rms)),
            t.caption,
            t.text_dim,
            Align::End,
        );
        r.cut_top(2.0);
        level_bar(ui, r.cut_top(12.0), rms, peak, marker);
        r.cut_top(12.0);
    }
    if !live && an.output_peak_db <= FLOOR_DB + 1.0 {
        ui.text(
            r.cut_top(18.0),
            "Play to meter",
            t.caption,
            t.text_faint,
            Align::Start,
        );
    } else {
        // Peak is the sample peak and RMS is not LUFS; said once, on request.
        let used = crate::util::hint(
            ui,
            W::new(("mlevels-hint", dev.id)),
            r,
            "Peak is the sample peak and RMS is not LUFS. The amber mark is the ceiling.",
        );
        r.cut_top(used);
    }
    r.cut_top(14.0);
    // Gain reduction, large.
    ui.text(
        r.cut_top(20.0),
        "Gain reduction",
        t.label,
        t.text_dim,
        Align::Start,
    );
    ui.scene.hline(r.x, r.right(), r.y, t.line);
    r.cut_top(8.0);
    let figure = r.cut_top(34.0);
    ui.text(
        figure,
        &format!("{gr:.1} dB"),
        t.readout,
        if gr > 0.05 { t.warn } else { t.text_faint },
        Align::Start,
    );
    let bar = r.cut_top(12.0);
    ui.scene.rect(bar, 3.0, t.well);
    let f = (gr / GR_SCALE).clamp(0.0, 1.0);
    if f > 0.002 {
        ui.scene.rect(
            Rect {
                x: bar.right() - bar.w * f,
                w: bar.w * f,
                ..bar
            },
            3.0,
            t.warn,
        );
    }
    let resp = ui.interact(W::new(("mgr-hit", dev.id)), bar);
    ui.tooltip(&resp, "Compressor and limiter reduction together");
    ui.describe_label(
        W::new(("mgr-label", dev.id)),
        bar,
        &format!("Gain reduction {gr:.1} dB"),
    );
}

/// Response of the three tone filters, from the EQ's own transfer function.
fn tone_plot(ui: &mut Ui, r: Rect, dev: &DeviceInstance, desc: &DeviceDescriptor, s: &Session) {
    let t = ui.theme.clone();
    let mut c = r;
    ui.text(
        c.cut_top(18.0),
        "Tone response",
        t.label,
        t.text_dim,
        Align::Start,
    );
    c.cut_top(4.0);
    let Some(eq) = peel_devices::descriptor("peel.fx.eq") else {
        return;
    };
    let mut params: Vec<f32> = eq.params.iter().map(|p| p.default).collect();
    let mut set = |k: &str, v: f32| {
        if let Some(i) = eq.params.iter().position(|p| p.key == k) {
            params[i] = v;
        }
    };
    // The filters Mastering applies: low shelf 120 Hz, bell 500 Hz (Q 0.7),
    // high shelf 8 kHz.
    set("band1.freq", 120.0);
    set("band1.gain", value(dev, desc, "low"));
    set("band2.freq", 500.0);
    set("band2.q", 0.7);
    set("band2.gain", value(dev, desc, "mid"));
    set("band3.enable", 0.0);
    set("band4.freq", 8_000.0);
    set("band4.gain", value(dev, desc, "high"));
    let plot = Rect {
        x: c.x + 26.0,
        w: c.w - 26.0,
        h: (c.h - 18.0).max(40.0),
        ..c
    };
    ui.scene.rect(plot, t.radius_m, t.well);
    let range = 8.0f32;
    let y_of = |db: f32| plot.center().y - db.clamp(-range, range) / range * plot.h * 0.5;
    let x_of = |f: f32| plot.x + plot.w * (f / 20.0).ln() / (20_000.0f32 / 20.0).ln();
    for f in [100.0, 1000.0, 10_000.0] {
        ui.scene.vline(x_of(f), plot.y, plot.bottom(), t.grid_beat);
    }
    for db in [-6.0f32, -3.0, 0.0, 3.0, 6.0] {
        ui.scene.hline(
            plot.x,
            plot.right(),
            y_of(db),
            if db == 0.0 { t.grid_bar } else { t.grid_beat },
        );
        ui.text(
            Rect {
                x: c.x,
                y: y_of(db) - 7.0,
                w: 22.0,
                h: 14.0,
            },
            &format!("{db:+.0}"),
            t.caption.size(10.0),
            t.accent.alpha(0.85),
            Align::End,
        );
    }
    for (f, l) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        ui.text(
            Rect {
                x: x_of(f) - 15.0,
                y: plot.bottom() + 2.0,
                w: 30.0,
                h: 14.0,
            },
            l,
            t.caption.size(10.0),
            t.text_faint,
            Align::Center,
        );
    }
    let sr = s.sample_rate().max(8000.0);
    let n = (plot.w / 3.0) as usize;
    let pts: Vec<Vec2> = (0..=n)
        .map(|i| {
            let x = plot.x + plot.w * i as f32 / n as f32;
            let f = 20.0 * (20_000.0f32 / 20.0).powf(i as f32 / n as f32);
            vec2(x, y_of(peel_effects::eq::eq_response_db(&params, sr, f)))
        })
        .collect();
    ui.scene.push_clip(plot);
    ui.scene.polyline(&pts, 2.0, t.text);
    ui.scene.pop_clip();
}

/// Gain reduction over the last few seconds, from real telemetry.
fn gr_history(ui: &mut Ui, r: Rect, history: &[f32], now: f32) {
    let t = ui.theme.clone();
    let mut c = r;
    let mut head = c.cut_top(18.0);
    ui.text(
        head.cut_left(150.0),
        "Gain reduction history",
        t.label,
        t.text_dim,
        Align::Start,
    );
    let max = history.iter().copied().fold(0.0f32, f32::max);
    ui.text(
        head,
        &format!("now {now:.1}   max {max:.1} dB"),
        t.caption,
        t.text_dim,
        Align::End,
    );
    c.cut_top(4.0);
    let plot = Rect {
        h: (c.h - 18.0).max(40.0),
        ..c
    };
    ui.scene.rect(plot, t.radius_m, t.well);
    for db in [3.0f32, 6.0, 9.0] {
        let y = plot.y + plot.h * db / GR_SCALE;
        ui.scene.hline(plot.x, plot.right(), y, t.grid_beat);
        ui.text(
            Rect {
                x: plot.x + 4.0,
                y: y - 12.0,
                w: 40.0,
                h: 12.0,
            },
            &format!("{db:.0} dB"),
            t.caption.size(10.0),
            t.text_faint,
            Align::Start,
        );
    }
    if history.len() < 2 {
        return;
    }
    let n = HISTORY as f32;
    let w = plot.w / n;
    ui.scene.push_clip(plot);
    let start = plot.right() - history.len() as f32 * w;
    for (i, v) in history.iter().enumerate() {
        if *v < 0.02 {
            continue;
        }
        let h = (v / GR_SCALE).clamp(0.0, 1.0) * plot.h;
        ui.scene.rect(
            Rect {
                x: start + i as f32 * w,
                y: plot.y,
                w: w + 0.75,
                h,
            },
            0.0,
            t.warn.alpha(0.85),
        );
    }
    ui.scene.pop_clip();
}
