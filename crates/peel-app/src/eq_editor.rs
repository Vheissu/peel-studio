//! EQ editor: a log-frequency plot with the real transfer function,
//! draggable band nodes, live pre/post spectra, a peak history strip and a
//! rail of exact numbers. Resonance suggestions live in the rail's second
//! tab and only ever write to a bell band the user has agreed to.

use std::collections::BTreeMap;

use peel_document::{DeviceInstance, Id};
use peel_dsp::DeviceDescriptor;
use peel_engine::analysis::Resonance;
use peel_session::Session;
use peel_ui::Id as W;
use peel_ui::{
    Align, Color, CursorIcon, FocusKind, MenuItem, Rect, Role, Semantics, Ui, Vec2, vec2,
};

use crate::analysis_view::FLOOR_DB;
use crate::util::ink_on;
use crate::workspace::Workspace;

pub const F_LO: f32 = 20.0;
pub const F_HI: f32 = 20_000.0;
/// Gain axis half range shown on the plot, dB.
pub const GAIN_DB: f32 = 18.0;
/// Spectrum axis: dBFS at the top and bottom of the plot.
pub const SPEC_TOP: f32 = 0.0;
pub const SPEC_BOTTOM: f32 = -90.0;
pub const BAND_COLORS: [u32; 4] = [0xEC5E48, 0xF5943A, 0x55C48E, 0x5E9FEE];
/// A resonance cut never goes deeper than this.
pub const MAX_CUT_DB: f32 = 6.0;
/// Bands in the rail: four bands, then high-pass and low-pass.
const ROWS: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Bands,
    Resonance,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Learn {
    Idle,
    Listening,
}

pub struct EqState {
    /// Selected row, 1..=6 (5 = low cut, 6 = high cut).
    pub band: usize,
    pub tab: Tab,
    pub show_pre: bool,
    pub show_post: bool,
    pub show_peak: bool,
    /// A/B compare: stored parameter sets and which one is live.
    pub slots: [Option<Vec<(&'static str, f32)>>; 2],
    pub active_slot: usize,
    pub learn: Learn,
    /// Candidate whose curve is previewed on the plot.
    pub preview: Option<usize>,
    /// Replacement waiting for a second click: (candidate, band).
    pub confirm: Option<(usize, usize)>,
}

impl Default for EqState {
    fn default() -> Self {
        EqState {
            band: 2,
            tab: Tab::Bands,
            show_pre: true,
            show_post: true,
            show_peak: true,
            slots: [None, None],
            active_slot: 0,
            learn: Learn::Idle,
            preview: None,
            confirm: None,
        }
    }
}

// ---- pure helpers ----

pub fn fmt_hz(f: f32) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if k >= 10.0 {
            format!("{k:.1} kHz")
        } else {
            format!("{k:.2} kHz")
        }
    } else {
        format!("{f:.0} Hz")
    }
}

/// Plot geometry: frequency and gain axes over `r`.
#[derive(Clone, Copy)]
pub struct Plot {
    pub r: Rect,
}

impl Plot {
    pub fn x_of(&self, f: f32) -> f32 {
        self.r.x + self.r.w * (f.clamp(F_LO, F_HI) / F_LO).ln() / (F_HI / F_LO).ln()
    }
    pub fn f_of(&self, x: f32) -> f32 {
        F_LO * (F_HI / F_LO).powf(((x - self.r.x) / self.r.w).clamp(0.0, 1.0))
    }
    pub fn y_gain(&self, db: f32) -> f32 {
        self.r.center().y - db.clamp(-GAIN_DB, GAIN_DB) / GAIN_DB * self.r.h * 0.5
    }
    pub fn gain_of(&self, y: f32) -> f32 {
        ((self.r.center().y - y) / (self.r.h * 0.5) * GAIN_DB).clamp(-GAIN_DB, GAIN_DB)
    }
    pub fn y_spec(&self, dbfs: f32) -> f32 {
        let f = (dbfs.clamp(SPEC_BOTTOM, SPEC_TOP) - SPEC_BOTTOM) / (SPEC_TOP - SPEC_BOTTOM);
        self.r.bottom() - f * self.r.h
    }
}

/// Which bell a resonance cut would go to, and whether that overwrites a
/// band the user is using.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub band: usize,
    pub replaces: bool,
}

/// A bell is free when it is off or has no gain.
pub fn bell_is_free(enabled: bool, gain_db: f32) -> bool {
    !enabled || gain_db.abs() < 0.05
}

/// Picks the bell for a suggestion. The selected bell wins when there is
/// one; otherwise the first free bell. When no bell is free the selected
/// bell (or Band 2) is offered as an explicit replacement.
pub fn choose_target(selected: usize, free: [bool; 2]) -> Target {
    let idx = |b: usize| b - 2;
    if (2..=3).contains(&selected) {
        return Target {
            band: selected,
            replaces: !free[idx(selected)],
        };
    }
    match free.iter().position(|f| *f) {
        Some(i) => Target {
            band: i + 2,
            replaces: false,
        },
        None => Target {
            band: 2,
            replaces: true,
        },
    }
}

/// The bell settings a candidate asks for, clamped to a conservative cut.
pub fn bell_for(r: &Resonance) -> (f32, f32, f32) {
    let freq = r.frequency_hz.clamp(F_LO, F_HI);
    let gain = (-r.cut_db.abs()).clamp(-MAX_CUT_DB, -0.5);
    let q = r.q.clamp(1.0, 12.0);
    (freq, gain, q)
}

// ---- parameter access ----

fn value(dev: &DeviceInstance, desc: &DeviceDescriptor, key: &str) -> f32 {
    dev.params
        .get(key)
        .copied()
        .or_else(|| desc.param_by_key(key).map(|p| p.default))
        .unwrap_or(0.0)
}

fn all_params(dev: &DeviceInstance, desc: &DeviceDescriptor) -> Vec<f32> {
    desc.params
        .iter()
        .map(|p| dev.params.get(p.key).copied().unwrap_or(p.default))
        .collect()
}

/// One undo step that rewrites several parameters (and the preset name).
pub(crate) fn edit_params(
    s: &mut Session,
    device: Id,
    label: &str,
    f: impl FnOnce(&mut BTreeMap<String, f32>, &mut Option<String>),
) {
    s.doc.edit(label.to_string(), |p| {
        if let Some(d) = peel_session::find_device_mut(p, device) {
            f(&mut d.params, &mut d.preset);
        }
    });
}

pub(crate) fn load_preset(
    s: &mut Session,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    i: usize,
) {
    let preset = peel_devices::presets(desc.type_id)[i];
    edit_params(
        s,
        dev.id,
        &format!("Load preset {}", preset.name),
        |m, name| {
            for pd in desc.params {
                m.insert(pd.key.to_string(), pd.default);
            }
            for (k, v) in preset.params {
                m.insert((*k).to_string(), *v);
            }
            *name = Some(preset.name.to_string());
        },
    );
}

pub(crate) fn reset_all(s: &mut Session, dev: &DeviceInstance, desc: &'static DeviceDescriptor) {
    edit_params(s, dev.id, "Reset EQ", |m, name| {
        for pd in desc.params {
            m.insert(pd.key.to_string(), pd.default);
        }
        *name = None;
    });
}

fn snapshot(dev: &DeviceInstance, desc: &DeviceDescriptor) -> Vec<(&'static str, f32)> {
    desc.params
        .iter()
        .map(|p| (p.key, dev.params.get(p.key).copied().unwrap_or(p.default)))
        .collect()
}

fn restore(s: &mut Session, dev: &DeviceInstance, snap: &[(&'static str, f32)]) {
    edit_params(s, dev.id, "Compare EQ", |m, name| {
        for (k, v) in snap {
            m.insert((*k).to_string(), *v);
        }
        *name = None;
    });
}

/// Sets one parameter inside a drag gesture.
fn set(s: &mut Session, dev: Id, key: &str, v: f32, gesture: u64) {
    s.set_param(dev, key, v, gesture);
}

// ---- main ----

pub fn draw(
    ui: &mut Ui,
    body: Rect,
    _track: Id,
    dev: &DeviceInstance,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let Some(desc) = peel_devices::descriptor("peel.fx.eq") else {
        return;
    };
    let mut c = body;
    top_bar(ui, c.cut_top(32.0), dev, desc, s, ws);
    c.cut_top(10.0);
    let mut rail = c.cut_right(300.0);
    c.cut_right(14.0);
    // A rail narrower than its table never happens at 1200x720, but keep
    // the plot from collapsing on odd sizes.
    if c.w < 360.0 {
        rail = Rect::ZERO;
        c = body;
        c.cut_top(42.0);
    }
    let strip_h = if c.h > 360.0 { 64.0 } else { 44.0 };
    let strip = c.cut_bottom(strip_h);
    c.cut_bottom(8.0);
    plot_area(ui, c, dev, desc, s, ws);
    history(ui, strip, ws, c);
    if rail.w > 0.0 {
        match ws.fx.eq.tab {
            Tab::Bands => rail_bands(ui, rail, dev, desc, s, ws),
            Tab::Resonance => crate::eq_resonance::rail(ui, rail, dev, desc, s, ws),
        }
    }
    let _ = t;
}

fn chip(ui: &mut Ui, id: W, r: Rect, label: &str, on: bool) -> bool {
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
            t.control_active
        } else if resp.hovered {
            t.control_hover
        } else {
            t.control.alpha(0.5)
        },
    );
    ui.text(
        r,
        label,
        t.caption,
        if on { t.text } else { t.text_dim },
        Align::Center,
    );
    ui.focus_ring(id, r, t.radius_s);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    resp.clicked
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
    // Preset.
    let presets = peel_devices::presets(desc.type_id);
    let pr = row.cut_left(200.0);
    let id = W::new(("eq-preset", dev.id));
    let label = dev.preset.clone().unwrap_or_else(|| "Custom".into());
    let resp = ui.dropdown(id, pr, &label, "Preset");
    if resp.clicked {
        ui.open_popup(id.with("menu"), pr);
    }
    let items: Vec<MenuItem> = presets
        .iter()
        .map(|p| MenuItem::new(p.name).checked(dev.preset.as_deref() == Some(p.name)))
        .collect();
    if let Some(i) = ui.menu(id.with("menu"), vec2(pr.x, pr.bottom() + 4.0), &items) {
        load_preset(s, dev, desc, i);
    }
    row.cut_left(12.0);

    // A/B compare.
    let ab = row.cut_left(72.0);
    let mut sel = ws.fx.eq.active_slot;
    if ui.segmented(W::new(("eq-ab", dev.id)), ab, &["A", "B"], &mut sel)
        && sel != ws.fx.eq.active_slot
    {
        let live = snapshot(dev, desc);
        let eq = &mut ws.fx.eq;
        eq.slots[eq.active_slot] = Some(live.clone());
        let next = eq.slots[sel].clone().unwrap_or(live);
        eq.active_slot = sel;
        restore(s, dev, &next);
    }
    row.cut_left(12.0);
    let reset = ui.button(W::new(("eq-reset", dev.id)), row.cut_left(64.0), "Reset");
    ui.tooltip(&reset, "Return every band and cut to flat");
    if reset.clicked {
        reset_all(s, dev, desc);
    }

    // Spectrum toggles on the right.
    let mut right = row;
    let live = ws.fx.analysis.live(ui.time());
    for (label, flag) in [
        ("Hold", &mut ws.fx.eq.show_peak),
        ("Post", &mut ws.fx.eq.show_post),
        ("Pre", &mut ws.fx.eq.show_pre),
    ] {
        let r = right.cut_right(46.0).shrink2(0.0, 3.0);
        right.cut_right(4.0);
        if chip(ui, W::new(("eq-show", label)), r, label, *flag) {
            *flag = !*flag;
            if label == "Hold" {
                ws.fx.analysis.clear_peaks();
            }
        }
    }
    right.cut_right(6.0);
    ui.text(
        right.cut_right(80.0),
        "Spectrum",
        t.caption,
        t.text_faint,
        Align::End,
    );
    let _ = live;
}

fn plot_area(
    ui: &mut Ui,
    area: Rect,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let mut a = area;
    let left = a.cut_left(34.0);
    let right = a.cut_right(36.0);
    let bottom = a.cut_bottom(18.0);
    let plot = Plot { r: a };
    ui.scene.rect(a, t.radius_m, t.well);
    ui.scene.push_clip(a);

    // Grid: frequency decades and gain steps.
    for f in [
        30.0, 40.0, 60.0, 70.0, 80.0, 90.0, 300.0, 400.0, 600.0, 700.0, 800.0, 900.0, 3000.0,
        4000.0, 6000.0, 7000.0, 8000.0, 9000.0,
    ] {
        ui.scene
            .vline(plot.x_of(f), a.y, a.bottom(), t.grid_step.alpha(0.6));
    }
    for f in [50.0, 200.0, 500.0, 2000.0, 5000.0] {
        ui.scene.vline(plot.x_of(f), a.y, a.bottom(), t.grid_step);
    }
    for f in [100.0, 1000.0, 10_000.0] {
        ui.scene.vline(plot.x_of(f), a.y, a.bottom(), t.grid_beat);
    }
    let mut g = -GAIN_DB;
    while g <= GAIN_DB + 0.01 {
        let y = plot.y_gain(g);
        ui.scene.hline(
            a.x,
            a.right(),
            y,
            if g == 0.0 { t.grid_bar } else { t.grid_beat },
        );
        g += 6.0;
    }

    let now = ui.time();
    let an = &ws.fx.analysis;
    let es = &ws.fx.eq;
    // Spectra from the real analysis tap.
    let cols = (a.w / 2.0) as usize;
    let hz = |x: f32| plot.f_of(x);
    let mut post_pts: Vec<Vec2> = Vec::with_capacity(cols + 1);
    let mut peak_pts: Vec<Vec2> = Vec::with_capacity(cols + 1);
    for i in 0..=cols {
        let x = a.x + a.w * i as f32 / cols as f32;
        let (lo, hi) = (hz(x - 1.0), hz(x + 1.0));
        if es.show_pre {
            let db = an.sample(&an.pre, lo, hi);
            if db > FLOOR_DB + 1.0 {
                let y = plot.y_spec(db);
                ui.scene.rect(
                    Rect {
                        x: x - 1.0,
                        y,
                        w: 2.0,
                        h: a.bottom() - y,
                    },
                    0.0,
                    t.text_faint.alpha(0.16),
                );
            }
        }
        if es.show_post {
            let db = an.sample(&an.post, lo, hi);
            let y = plot.y_spec(db);
            if db > FLOOR_DB + 1.0 {
                ui.scene.rect(
                    Rect {
                        x: x - 1.0,
                        y,
                        w: 2.0,
                        h: a.bottom() - y,
                    },
                    0.0,
                    t.play.alpha(0.16),
                );
            }
            post_pts.push(vec2(x, y));
        }
        if es.show_peak {
            let db = an.sample(&an.peak, lo, hi);
            peak_pts.push(vec2(x, plot.y_spec(db)));
        }
    }
    if es.show_peak && peak_pts.iter().any(|p| p.y < a.bottom() - 1.0) {
        ui.scene.polyline(&peak_pts, 1.0, t.text_dim.alpha(0.55));
    }
    if es.show_post && post_pts.iter().any(|p| p.y < a.bottom() - 1.0) {
        ui.scene.polyline(&post_pts, 1.5, t.play);
    }

    // Transfer function: per-band fills and the total curve.
    let params = all_params(dev, desc);
    let sr = s.sample_rate().max(8000.0);
    let n = (a.w / 3.0) as usize;
    let xs: Vec<f32> = (0..=n).map(|i| a.x + a.w * i as f32 / n as f32).collect();
    let y0 = plot.y_gain(0.0);
    for band in 1..=4u32 {
        if value(dev, desc, &format!("band{band}.enable")) < 0.5 {
            continue;
        }
        let c = Color::hex(BAND_COLORS[band as usize - 1]);
        for w in xs.windows(2) {
            let db = peel_effects::eq::eq_band_response_db(&params, band, sr, plot.f_of(w[0]));
            let y = plot.y_gain(db);
            ui.scene.rect(
                Rect {
                    x: w[0],
                    y: y.min(y0),
                    w: (w[1] - w[0]).max(1.0),
                    h: (y - y0).abs(),
                },
                0.0,
                c.alpha(0.10),
            );
        }
    }
    let total: Vec<Vec2> = xs
        .iter()
        .map(|x| {
            vec2(
                *x,
                plot.y_gain(peel_effects::eq::eq_response_db(&params, sr, plot.f_of(*x))),
            )
        })
        .collect();
    ui.scene.polyline(&total, 2.0, t.text);

    // Resonance preview and candidate markers.
    crate::eq_resonance::overlay(ui, plot, dev, desc, &params, sr, ws);

    // Nodes.
    nodes(ui, plot, dev, desc, s, ws);
    if !ws.fx.analysis.live(now) && ws.fx.analysis.pre.iter().all(|v| *v <= FLOOR_DB + 0.5) {
        ui.text(
            Rect {
                x: a.x + 10.0,
                y: a.y + 6.0,
                w: a.w - 20.0,
                h: 16.0,
            },
            "Play audio through this EQ to see the spectrum",
            t.caption,
            t.text_faint,
            Align::Start,
        );
    }
    ui.scene.pop_clip();

    // Axes: frequency along the bottom, gain on the right, dBFS on the left.
    for (f, label) in [
        (20.0, "20"),
        (50.0, "50"),
        (100.0, "100"),
        (200.0, "200"),
        (500.0, "500"),
        (1000.0, "1k"),
        (2000.0, "2k"),
        (5000.0, "5k"),
        (10_000.0, "10k"),
        (20_000.0, "20k"),
    ] {
        let x = plot.x_of(f);
        let w = 30.0;
        let lx = (x - w * 0.5).clamp(bottom.x, bottom.right() - w);
        ui.text(
            Rect {
                x: lx,
                y: bottom.y + 2.0,
                w,
                h: 14.0,
            },
            label,
            t.caption.size(10.5),
            t.text_faint,
            Align::Center,
        );
    }
    ui.text(
        Rect {
            x: bottom.x,
            y: bottom.y + 2.0,
            w: 20.0,
            h: 14.0,
        },
        "",
        t.caption,
        t.text_faint,
        Align::Start,
    );
    let mut g = -12.0;
    while g <= 12.01 {
        let y = plot.y_gain(g);
        let label = if g == 0.0 {
            "0".to_string()
        } else {
            format!("{g:+.0}")
        };
        ui.text(
            Rect {
                x: right.x + 4.0,
                y: y - 7.0,
                w: right.w - 4.0,
                h: 14.0,
            },
            &label,
            t.caption.size(10.5),
            t.accent.alpha(0.85),
            Align::Start,
        );
        g += 6.0;
    }
    ui.text(
        Rect {
            x: right.x + 4.0,
            y: a.y - 1.0,
            w: right.w,
            h: 14.0,
        },
        "dB",
        t.caption.size(10.5),
        t.accent.alpha(0.85),
        Align::Start,
    );
    for db in [-12.0f32, -24.0, -36.0, -48.0, -60.0, -72.0, -84.0] {
        let y = plot.y_spec(db);
        ui.text(
            Rect {
                x: left.x,
                y: y - 7.0,
                w: left.w - 4.0,
                h: 14.0,
            },
            &format!("{db:.0}"),
            t.caption.size(10.5),
            t.text_faint,
            Align::End,
        );
    }
    ui.text(
        Rect {
            x: left.x,
            y: a.y - 1.0,
            w: left.w - 4.0,
            h: 14.0,
        },
        "dBFS",
        t.caption.size(10.5),
        t.text_faint,
        Align::End,
    );
}

fn nodes(
    ui: &mut Ui,
    plot: Plot,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    // Cuts first so bands sit on top of them.
    for (row, key) in [(5usize, "hp"), (6, "lp")] {
        let freq = value(dev, desc, &format!("{key}.freq"));
        let on = value(dev, desc, &format!("{key}.enable")) >= 0.5;
        let c = vec2(plot.x_of(freq), plot.y_gain(0.0));
        let id = W::new(("eq-node", dev.id, row));
        let name = if row == 5 { "Low cut" } else { "High cut" };
        let resp = ui.interact_node(
            id,
            Rect::from_center(c, 22.0, 26.0),
            FocusKind::Adjust,
            Semantics::new(Role::Button).label(name).value(format!(
                "{}{}",
                fmt_hz(freq),
                if on { "" } else { ", off" }
            )),
        );
        let hot = resp.hovered || resp.active || ws.fx.eq.band == row;
        let body = Rect::from_center(c, 10.0, if hot { 22.0 } else { 18.0 });
        ui.scene
            .rect(body, 3.0, if on { t.text_dim } else { t.well });
        if !on {
            ui.scene.outline(body, 3.0, 1.5, t.text_faint);
        }
        if resp.pressed {
            ws.fx.eq.band = row;
        }
        if resp.double_clicked {
            set(
                s,
                dev.id,
                &format!("{key}.enable"),
                if on { 0.0 } else { 1.0 },
                id.0 ^ 0xe0,
            );
            s.doc.end_gesture();
        } else if resp.dragging {
            let fine = if ui.input.modifiers.shift { 0.2 } else { 1.0 };
            let x = plot.x_of(freq) + resp.drag_delta.x * fine;
            if !on {
                set(s, dev.id, &format!("{key}.enable"), 1.0, id.0);
            }
            set(s, dev.id, &format!("{key}.freq"), plot.f_of(x), id.0);
        }
        let adj = ui.take_adjust(id);
        if adj.steps != 0 {
            let f = (freq * 1.05f32.powi(adj.steps)).clamp(F_LO, F_HI);
            set(s, dev.id, &format!("{key}.freq"), f, id.0 ^ 0xad);
            s.doc.end_gesture();
        }
        if resp.released {
            s.doc.end_gesture();
        }
        if resp.hovered {
            ui.set_cursor(CursorIcon::EwResize);
        }
    }
    for band in 1..=4usize {
        let key = |k: &str| format!("band{band}.{k}");
        let (freq, gain, q, on) = (
            value(dev, desc, &key("freq")),
            value(dev, desc, &key("gain")),
            value(dev, desc, &key("q")),
            value(dev, desc, &key("enable")) >= 0.5,
        );
        let color = Color::hex(BAND_COLORS[band - 1]);
        let c = vec2(plot.x_of(freq), plot.y_gain(gain));
        let id = W::new(("eq-node", dev.id, band));
        let kind = if band == 1 || band == 4 {
            "shelf"
        } else {
            "bell"
        };
        let resp = ui.interact_node(
            id,
            Rect::from_center(c, 26.0, 26.0),
            FocusKind::Adjust,
            Semantics::new(Role::Button)
                .label(format!("Band {band} {kind}"))
                .value(format!(
                    "{}, {gain:+.1} dB, Q {q:.2}{}",
                    fmt_hz(freq),
                    if on { "" } else { ", off" }
                )),
        );
        let selected = ws.fx.eq.band == band;
        let hot = resp.hovered || resp.active;
        let radius = if hot { 9.5 } else { 8.0 };
        if on {
            ui.scene.circle_stroke(
                c,
                radius,
                color,
                if selected { 2.5 } else { 1.0 },
                if selected { t.text } else { t.bg },
            );
        } else {
            ui.scene
                .circle_stroke(c, radius - 1.0, t.well, 1.5, color.alpha(0.8));
        }
        ui.text(
            Rect::from_center(c, 14.0, 14.0),
            &band.to_string(),
            t.caption.size(10.0),
            if on { ink_on(color) } else { color },
            Align::Center,
        );
        if resp.pressed {
            ws.fx.eq.band = band;
        }
        if resp.double_clicked {
            set(
                s,
                dev.id,
                &key("enable"),
                if on { 0.0 } else { 1.0 },
                id.0 ^ 0xe0,
            );
            s.doc.end_gesture();
        } else if resp.dragging {
            let fine = if ui.input.modifiers.shift { 0.2 } else { 1.0 };
            let nx = c.x + resp.drag_delta.x * fine;
            let ny = c.y + resp.drag_delta.y * fine;
            if !on {
                set(s, dev.id, &key("enable"), 1.0, id.0);
            }
            set(s, dev.id, &key("freq"), plot.f_of(nx), id.0);
            set(s, dev.id, &key("gain"), plot.gain_of(ny), id.0);
        }
        let adj = ui.take_adjust(id);
        if adj.steps != 0 {
            let g = (gain + adj.steps as f32 * 0.5).clamp(-24.0, 24.0);
            set(s, dev.id, &key("gain"), g, id.0 ^ 0xad);
            s.doc.end_gesture();
        }
        if resp.released {
            s.doc.end_gesture();
        }
        if hot {
            ui.set_cursor(CursorIcon::Grab);
        }
        // Wheel adjusts Q.
        if resp.hovered && ui.input.scroll.y != 0.0 {
            let nq = (q * (1.0 + ui.input.scroll.y * 0.004)).clamp(0.1, 18.0);
            ui.input.scroll.y = 0.0;
            set(s, dev.id, &key("q"), nq, id.0 ^ 0x51);
        }
        // Readout beside the selected or dragged node.
        if selected || hot {
            let text = format!("{}  {gain:+.1} dB  Q {q:.2}", fmt_hz(freq));
            let w = ui.measure(&text, t.caption).x + 12.0;
            let mut x = c.x - w * 0.5;
            x = x.clamp(plot.r.x + 2.0, plot.r.right() - w - 2.0);
            let ry = if c.y - plot.r.y > 40.0 {
                c.y - 36.0
            } else {
                c.y + 16.0
            };
            let br = Rect {
                x,
                y: ry,
                w,
                h: 20.0,
            };
            ui.scene.rect(br, t.radius_s, t.raised);
            ui.text(br, &text, t.caption, t.text, Align::Center);
        }
    }
    // Wheel on empty plot also adjusts nothing: leave scroll for the page.
}

/// Output peak history (real envelope) and the peak/RMS figures.
fn history(ui: &mut Ui, strip: Rect, ws: &mut Workspace, plot_area: Rect) {
    let t = ui.theme.clone();
    let an = &ws.fx.analysis;
    let mut r = strip;
    let label_h = 16.0;
    let head = r.cut_top(label_h);
    let gutter_l = 34.0;
    let gutter_r = 36.0;
    let w = Rect {
        x: plot_area.x + gutter_l,
        w: plot_area.w - gutter_l - gutter_r,
        ..r
    };
    ui.text(
        Rect {
            x: w.x,
            w: 150.0,
            ..head
        },
        "Output peak history",
        t.caption,
        t.text_faint,
        Align::Start,
    );
    let figures = format!(
        "Peak in {} out {}   RMS in {} out {} dBFS",
        db_text(an.input_peak_db),
        db_text(an.output_peak_db),
        db_text(an.input_rms_db),
        db_text(an.output_rms_db)
    );
    ui.text(
        Rect {
            x: w.x + 150.0,
            w: w.w - 150.0,
            ..head
        },
        &figures,
        t.caption,
        t.text_dim,
        Align::End,
    );
    ui.scene.rect(w, t.radius_s, t.well);
    let n = an.wave.len();
    let cx = w.center().y;
    let half = w.h * 0.5 - 2.0;
    ui.scene.hline(w.x, w.right(), cx, t.grid_beat);
    ui.scene.push_clip(w);
    for (i, p) in an.wave.iter().enumerate() {
        let x = w.x + w.w * i as f32 / n as f32;
        let bw = (w.w / n as f32 - 0.5).max(1.0);
        let top = cx - p.max.clamp(-1.0, 1.0) * half;
        let bot = cx - p.min.clamp(-1.0, 1.0) * half;
        let hot = p.max >= 1.0 || p.min <= -1.0;
        if bot - top > 0.2 {
            ui.scene.rect(
                Rect {
                    x,
                    y: top,
                    w: bw,
                    h: (bot - top).max(1.0),
                },
                0.0,
                if hot { t.record } else { t.play.alpha(0.8) },
            );
        }
    }
    ui.scene.pop_clip();
}

fn db_text(db: f32) -> String {
    if db <= -99.0 {
        "-inf".into()
    } else {
        format!("{db:.1}")
    }
}

// ---- rail: numbers ----

/// A numeric field whose drag speed scales with the value (log-like).
/// Returns the new value when it changed.
fn log_field(
    ui: &mut Ui,
    id: W,
    r: Rect,
    value: f32,
    (min, max): (f32, f32),
    fmt: impl Fn(f32) -> String,
    name: &str,
) -> Option<(f32, bool)> {
    let t = ui.theme.clone();
    // Drag speed is fixed when the drag begins so it does not feed back.
    if !ui.is_active(id) {
        *ui.state::<f64>(id.with("ppx")) = f64::from(value) * 0.004;
    }
    let ppx = *ui.state::<f64>(id.with("ppx"));
    let mut v = f64::from(value);
    let resp = ui.drag_number(
        id,
        r,
        &mut v,
        f64::from(min),
        f64::from(max),
        ppx,
        t.readout_small,
        t.text,
        |v| fmt(v as f32),
    );
    ui.name(&resp, name);
    resp.changed.then_some((v as f32, resp.released))
}

pub(crate) fn rail_tabs(ui: &mut Ui, r: Rect, ws: &mut Workspace) {
    let mut sel = usize::from(ws.fx.eq.tab == Tab::Resonance);
    if ui.segmented(W::new("eq-tab"), r, &["Bands", "Resonance"], &mut sel) {
        ws.fx.eq.tab = if sel == 1 { Tab::Resonance } else { Tab::Bands };
    }
}

fn rail_bands(
    ui: &mut Ui,
    rail: Rect,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let mut r = rail;
    rail_tabs(ui, r.cut_top(28.0), ws);
    r.cut_top(10.0);
    // Column captions.
    let mut cap = r.cut_top(16.0);
    cap.cut_left(26.0 + 42.0);
    let f_col = cap.cut_left(80.0);
    cap.cut_left(4.0);
    let g_col = cap.cut_left(64.0);
    cap.cut_left(4.0);
    let q_col = cap.cut_left(54.0);
    for (rc, l) in [(f_col, "Freq"), (g_col, "Gain"), (q_col, "Q")] {
        ui.text(rc, l, t.caption, t.text_faint, Align::Center);
    }
    for row in 1..=ROWS {
        let mut rr = r.cut_top(32.0);
        r.cut_top(2.0);
        let sel = ws.fx.eq.band == row;
        ui.scene.rect(
            rr,
            t.radius_s,
            if sel { t.raised } else { Color::TRANSPARENT },
        );
        let (name, key, color) = match row {
            1..=4 => (
                format!("Band {row}"),
                format!("band{row}"),
                Color::hex(BAND_COLORS[row - 1]),
            ),
            5 => ("Low cut".to_string(), "hp".to_string(), t.text_dim),
            _ => ("High cut".to_string(), "lp".to_string(), t.text_dim),
        };
        let on = value(dev, desc, &format!("{key}.enable")) >= 0.5;
        let row_id = W::new(("eq-row", dev.id, row));
        let toggle_r = rr.cut_left(26.0);
        let t_resp = ui.interact_node(
            row_id.with("on"),
            toggle_r,
            FocusKind::Activate,
            Semantics::new(Role::Button)
                .label(format!("{name} on"))
                .toggled(on),
        );
        let dot = Rect::from_center(toggle_r.center(), 12.0, 12.0);
        if on {
            ui.scene.circle(dot.center(), 6.0, color);
        } else {
            ui.scene
                .circle_stroke(dot.center(), 5.0, t.well, 1.5, color.alpha(0.8));
        }
        ui.focus_ring(row_id.with("on"), toggle_r, 6.0);
        if t_resp.clicked {
            set(
                s,
                dev.id,
                &format!("{key}.enable"),
                if on { 0.0 } else { 1.0 },
                row_id.0,
            );
            s.doc.end_gesture();
        }
        let name_r = rr.cut_left(42.0);
        let nr = ui.interact_node(
            row_id,
            name_r,
            FocusKind::Activate,
            Semantics::new(Role::Button)
                .label(format!("Select {name}"))
                .selected(sel),
        );
        let short = match row {
            5 => "HP",
            6 => "LP",
            n => ["1", "2", "3", "4"][n - 1],
        };
        ui.text(
            name_r,
            short,
            t.label,
            if on { t.text } else { t.text_faint },
            Align::Start,
        );
        ui.focus_ring(row_id, name_r, t.radius_s);
        if nr.clicked {
            ws.fx.eq.band = row;
        }
        // Freq.
        let fr = rr.cut_left(80.0).shrink2(0.0, 4.0);
        rr.cut_left(4.0);
        let fid = W::new(("eq-f", dev.id, row));
        if let Some((v, done)) = log_field(
            ui,
            fid,
            fr,
            value(dev, desc, &format!("{key}.freq")),
            (F_LO, F_HI),
            fmt_hz,
            &format!("{name} frequency"),
        ) {
            set(s, dev.id, &format!("{key}.freq"), v, fid.0);
            if done {
                s.doc.end_gesture();
            }
        }
        if row <= 4 {
            let gr = rr.cut_left(64.0).shrink2(0.0, 4.0);
            rr.cut_left(4.0);
            let gid = W::new(("eq-g", dev.id, row));
            let mut gv = f64::from(value(dev, desc, &format!("{key}.gain")));
            let resp = ui.drag_number(
                gid,
                gr,
                &mut gv,
                -24.0,
                24.0,
                0.1,
                t.readout_small,
                t.text,
                |v| format!("{v:+.1}"),
            );
            ui.name(&resp, &format!("{name} gain in dB"));
            if resp.changed {
                set(
                    s,
                    dev.id,
                    &format!("{key}.gain"),
                    (gv as f32 * 10.0).round() / 10.0,
                    gid.0,
                );
            }
            if resp.released {
                s.doc.end_gesture();
            }
            let qr = rr.cut_left(54.0).shrink2(0.0, 4.0);
            let qid = W::new(("eq-q", dev.id, row));
            if let Some((v, done)) = log_field(
                ui,
                qid,
                qr,
                value(dev, desc, &format!("{key}.q")),
                (0.1, 18.0),
                |v| format!("{v:.2}"),
                &format!("{name} Q"),
            ) {
                set(s, dev.id, &format!("{key}.q"), v, qid.0);
                if done {
                    s.doc.end_gesture();
                }
            }
        } else {
            // Slope of a cut.
            let slope = value(dev, desc, &format!("{key}.slope")).round() as usize;
            let mut sel_s = slope.min(3);
            let sr = rr.cut_left(122.0).shrink2(0.0, 3.0);
            if ui.segmented(
                W::new(("eq-slope", dev.id, row)),
                sr,
                &["6", "12", "24", "48"],
                &mut sel_s,
            ) {
                set(
                    s,
                    dev.id,
                    &format!("{key}.slope"),
                    sel_s as f32,
                    W::new(("eq-slope-g", row, sel_s)).0,
                );
                s.doc.end_gesture();
            }
        }
    }
    r.cut_top(6.0);
    ui.scene.hline(r.x, r.right(), r.y, t.line);
    r.cut_top(10.0);
    // Output gain.
    let mut orow = r.cut_top(30.0);
    ui.text(
        orow.cut_left(70.0),
        "Output",
        t.label,
        t.text_dim,
        Align::Start,
    );
    let gid = W::new(("eq-out", dev.id));
    let mut gv = f64::from(value(dev, desc, "output"));
    let resp = ui.drag_number(
        gid,
        orow.cut_left(80.0).shrink2(0.0, 3.0),
        &mut gv,
        -24.0,
        24.0,
        0.1,
        t.readout_small,
        t.text,
        |v| format!("{v:+.1} dB"),
    );
    ui.name(&resp, "EQ output gain in dB");
    if resp.changed {
        set(
            s,
            dev.id,
            "output",
            (gv as f32 * 10.0).round() / 10.0,
            gid.0,
        );
    }
    if resp.released {
        s.doc.end_gesture();
    }
    r.cut_top(10.0);
    crate::util::hint(
        ui,
        W::new("eq-hint"),
        r,
        "Drag a node for frequency and gain, scroll over it for Q, double-click to switch a band on or off. Hold Shift to move slowly.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plot_axes_round_trip() {
        let p = Plot {
            r: Rect {
                x: 10.0,
                y: 20.0,
                w: 800.0,
                h: 300.0,
            },
        };
        for f in [20.0f32, 100.0, 1000.0, 20_000.0] {
            assert!((p.f_of(p.x_of(f)) - f).abs() / f < 1e-3, "{f}");
        }
        assert!((p.gain_of(p.y_gain(6.0)) - 6.0).abs() < 1e-3);
        assert!((p.y_gain(0.0) - p.r.center().y).abs() < 1e-3);
        assert_eq!(p.y_spec(0.0), p.r.y);
        assert_eq!(p.y_spec(-90.0), p.r.bottom());
    }

    #[test]
    fn suggestions_use_the_selected_free_bell_first() {
        // Selected bell 3 is free: use it, no replacement.
        assert_eq!(
            choose_target(3, [false, true]),
            Target {
                band: 3,
                replaces: false
            }
        );
        // Selected bell 2 is in use: replacing it must be explicit.
        assert_eq!(
            choose_target(2, [false, true]),
            Target {
                band: 2,
                replaces: true
            }
        );
        // A shelf is selected: take the first free bell.
        assert_eq!(
            choose_target(1, [false, true]),
            Target {
                band: 3,
                replaces: false
            }
        );
        assert_eq!(
            choose_target(4, [true, true]),
            Target {
                band: 2,
                replaces: false
            }
        );
        // Both bells busy and nothing selected: offer Band 2 as a replacement.
        assert_eq!(
            choose_target(1, [false, false]),
            Target {
                band: 2,
                replaces: true
            }
        );
    }

    #[test]
    fn bells_are_free_when_off_or_flat() {
        assert!(bell_is_free(false, 5.0));
        assert!(bell_is_free(true, 0.0));
        assert!(!bell_is_free(true, -2.0));
    }

    #[test]
    fn candidate_cuts_stay_conservative() {
        let r = Resonance {
            frequency_hz: 30_000.0,
            q: 40.0,
            cut_db: -20.0,
            prominence_db: 12.0,
        };
        let (f, g, q) = bell_for(&r);
        assert_eq!(f, F_HI);
        assert_eq!(g, -MAX_CUT_DB);
        assert_eq!(q, 12.0);
        let tiny = Resonance {
            frequency_hz: 1000.0,
            q: 0.2,
            cut_db: -0.1,
            prominence_db: 5.0,
        };
        let (_, g, q) = bell_for(&tiny);
        assert_eq!(g, -0.5);
        assert_eq!(q, 1.0);
    }

    #[test]
    fn frequency_labels() {
        assert_eq!(fmt_hz(850.0), "850 Hz");
        assert_eq!(fmt_hz(2500.0), "2.50 kHz");
        assert_eq!(fmt_hz(12_000.0), "12.0 kHz");
    }
}
