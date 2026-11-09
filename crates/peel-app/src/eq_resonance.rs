//! Resonance suggestions for the EQ editor.
//!
//! The analysis tap reports up to three sustained narrow spectral peaks.
//! They are candidates to audition, not defects: the user listens, reviews
//! them, previews the curve and applies a bounded bell cut to a bell band
//! they have agreed to. Nothing is written without a click, and replacing
//! a band that is in use takes a second, explicit click.

use peel_document::{DeviceInstance, Id};
use peel_dsp::DeviceDescriptor;
use peel_engine::analysis::Resonance;
use peel_session::Session;
use peel_ui::Id as W;
use peel_ui::{Align, Color, Rect, Ui, vec2};

use crate::eq_editor::{
    BAND_COLORS, Learn, Plot, Target, bell_for, bell_is_free, choose_target, fmt_hz, rail_tabs,
};
use crate::workspace::Workspace;

/// Candidates currently on offer: held ones when the user kept them,
/// otherwise what the analyzer reports while listening.
pub fn candidates(ws: &Workspace, device: Id) -> Vec<Resonance> {
    match ws.fx.eq.learn {
        Learn::Listening => ws.fx.analysis.resonances.clone(),
        Learn::Idle => ws.fx.held.get(&device).cloned().unwrap_or_default(),
    }
}

fn bell_state(dev: &DeviceInstance, desc: &DeviceDescriptor, band: usize) -> bool {
    let get = |k: &str| {
        let key = format!("band{band}.{k}");
        dev.params
            .get(&key)
            .copied()
            .or_else(|| desc.param_by_key(&key).map(|p| p.default))
            .unwrap_or(0.0)
    };
    bell_is_free(get("enable") >= 0.5, get("gain"))
}

fn target_for(dev: &DeviceInstance, desc: &DeviceDescriptor, selected: usize) -> Target {
    choose_target(
        selected,
        [bell_state(dev, desc, 2), bell_state(dev, desc, 3)],
    )
}

/// Preview curve and frequency markers, drawn inside the plot clip.
pub fn overlay(
    ui: &mut Ui,
    plot: Plot,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    params: &[f32],
    sr: f32,
    ws: &Workspace,
) {
    let t = ui.theme.clone();
    if ws.fx.eq.tab != crate::eq_editor::Tab::Resonance {
        return;
    }
    let list = candidates(ws, dev.id);
    for (i, r) in list.iter().enumerate() {
        let x = plot.x_of(r.frequency_hz);
        let previewed = ws.fx.eq.preview == Some(i);
        ui.scene.vline(
            x,
            plot.r.y,
            plot.r.bottom(),
            if previewed {
                t.text.alpha(0.7)
            } else {
                t.text_dim.alpha(0.35)
            },
        );
        let tag = Rect::from_center(vec2(x, plot.r.y + 12.0), 16.0, 16.0);
        ui.scene
            .rect(tag, 8.0, if previewed { t.text } else { t.raised });
        ui.text(
            tag,
            &(i + 1).to_string(),
            t.caption.size(10.0),
            if previewed { t.bg } else { t.text },
            Align::Center,
        );
    }
    if let Some(r) = ws.fx.eq.preview.and_then(|i| list.get(i)) {
        let target = target_for(dev, desc, ws.fx.eq.band);
        let (f, g, q) = bell_for(r);
        let mut p = params.to_vec();
        let set = |p: &mut [f32], k: &str, v: f32| {
            if let Some(i) = desc.params.iter().position(|d| d.key == k) {
                p[i] = v;
            }
        };
        set(&mut p, &format!("band{}.enable", target.band), 1.0);
        set(&mut p, &format!("band{}.freq", target.band), f);
        set(&mut p, &format!("band{}.gain", target.band), g);
        set(&mut p, &format!("band{}.q", target.band), q);
        let n = (plot.r.w / 3.0) as usize;
        let pts: Vec<_> = (0..=n)
            .map(|i| {
                let x = plot.r.x + plot.r.w * i as f32 / n as f32;
                vec2(
                    x,
                    plot.y_gain(peel_effects::eq::eq_response_db(&p, sr, plot.f_of(x))),
                )
            })
            .collect();
        ui.scene
            .polyline(&pts, 1.5, Color::hex(BAND_COLORS[target.band - 1]));
    }
}

fn apply(s: &mut Session, dev: &DeviceInstance, band: usize, r: &Resonance) {
    let (f, g, q) = bell_for(r);
    s.doc.edit(format!("Cut {} resonance", fmt_hz(f)), |p| {
        if let Some(d) = peel_session::find_device_mut(p, dev.id) {
            for (k, v) in [("enable", 1.0), ("freq", f), ("gain", g), ("q", q)] {
                d.params.insert(format!("band{band}.{k}"), v);
            }
            d.preset = None;
        }
    });
}

pub fn rail(
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
    let live = ws.fx.analysis.live(ui.time());
    let held = ws.fx.held.contains_key(&dev.id);
    let list = candidates(ws, dev.id);

    // Controls.
    let mut row = r.cut_top(30.0);
    match (ws.fx.eq.learn, held) {
        (Learn::Idle, false) => {
            if ui
                .button_primary(W::new("eq-listen"), row.cut_left(110.0), "Listen")
                .clicked
            {
                ws.fx.analysis.reset();
                ws.fx.eq.learn = Learn::Listening;
                ws.fx.eq.preview = None;
                ws.fx.eq.confirm = None;
            }
        }
        (Learn::Listening, _) => {
            let can_hold = !list.is_empty();
            if can_hold
                && ui
                    .button_primary(W::new("eq-hold"), row.cut_left(120.0), "Hold results")
                    .clicked
            {
                ws.fx.held.insert(dev.id, list.clone());
                ws.fx.eq.learn = Learn::Idle;
                ws.fx.eq.preview = None;
            }
            row.cut_left(8.0);
            if ui
                .button(W::new("eq-stop"), row.cut_left(64.0), "Stop")
                .clicked
            {
                ws.fx.eq.learn = Learn::Idle;
                ws.fx.eq.preview = None;
                ws.fx.eq.confirm = None;
            }
        }
        (Learn::Idle, true) => {
            if ui
                .button(W::new("eq-relearn"), row.cut_left(90.0), "Relearn")
                .clicked
            {
                ws.fx.held.remove(&dev.id);
                ws.fx.analysis.reset();
                ws.fx.eq.learn = Learn::Listening;
                ws.fx.eq.preview = None;
                ws.fx.eq.confirm = None;
            }
            row.cut_left(8.0);
            if ui
                .button(W::new("eq-clear"), row.cut_left(64.0), "Clear")
                .clicked
            {
                ws.fx.held.remove(&dev.id);
                ws.fx.eq.preview = None;
                ws.fx.eq.confirm = None;
            }
        }
    }
    r.cut_top(8.0);

    // Status line.
    let an = &ws.fx.analysis;
    let status: String = match (ws.fx.eq.learn, held) {
        (Learn::Idle, false) => {
            "Play the track, then Listen. Candidates are sustained narrow peaks to check by ear."
                .into()
        }
        (Learn::Idle, true) => format!(
            "Held: {} candidate{}. Not live; relearn after changes.",
            list.len(),
            if list.len() == 1 { "" } else { "s" }
        ),
        (Learn::Listening, _) if !live => "Waiting for audio. Play the project.".into(),
        (Learn::Listening, _) if an.analyzed_seconds < 0.75 => {
            format!("Listening {:.1} s", an.analyzed_seconds)
        }
        (Learn::Listening, _) if an.input_peak_db < -70.0 => "Not enough signal.".into(),
        (Learn::Listening, _) if list.is_empty() => {
            format!(
                "Heard {:.1} s. Nothing stands out yet.",
                an.analyzed_seconds
            )
        }
        (Learn::Listening, _) => format!("Heard {:.1} s", an.analyzed_seconds),
    };
    let used = ui.text_wrapped(r.min(), r.w, &status, t.caption, t.text_dim);
    r.cut_top(used + 10.0);

    // Where a cut would go.
    let target = target_for(dev, desc, ws.fx.eq.band);
    if !list.is_empty() {
        let note = if target.replaces {
            format!(
                "Both bells are in use. Applying replaces Band {}; select a bell to choose which.",
                target.band
            )
        } else {
            format!("Cuts go to Band {}, which is free.", target.band)
        };
        let used = ui.text_wrapped(r.min(), r.w, &note, t.caption, t.text_faint);
        r.cut_top(used + 8.0);
    }
    for (i, cand) in list.iter().enumerate() {
        if r.h < 80.0 {
            break;
        }
        ui.scene.hline(r.x, r.right(), r.y, t.line);
        r.cut_top(8.0);
        let (_, gain, q) = bell_for(cand);
        let mut head = r.cut_top(22.0);
        ui.text(
            head.cut_left(26.0),
            &(i + 1).to_string(),
            t.label,
            t.text_faint,
            Align::Start,
        );
        ui.text(
            head.cut_left(110.0),
            &fmt_hz(cand.frequency_hz),
            t.heading,
            t.text,
            Align::Start,
        );
        ui.text(
            head,
            &format!("{gain:.1} dB  Q {q:.1}"),
            t.readout_small,
            t.text_dim,
            Align::End,
        );
        let sub = r.cut_top(16.0);
        ui.text(
            sub,
            &format!("{:.1} dB above its surroundings", cand.prominence_db),
            t.caption,
            t.text_faint,
            Align::Start,
        );
        r.cut_top(6.0);
        let mut btns = r.cut_top(28.0);
        let prev_on = ws.fx.eq.preview == Some(i);
        let pr = btns.cut_left(92.0);
        let pid = W::new(("eq-preview", i));
        let resp = ui.interact_node(
            pid,
            pr,
            peel_ui::FocusKind::Activate,
            peel_ui::Semantics::new(peel_ui::Role::Button)
                .label(format!("Show curve for candidate {}", i + 1))
                .toggled(prev_on),
        );
        ui.scene.rect(
            pr,
            t.radius_s,
            if prev_on {
                t.control_active
            } else if resp.hovered {
                t.control_hover
            } else {
                t.control
            },
        );
        ui.text(pr, "Show curve", t.label, t.text, Align::Center);
        ui.focus_ring(pid, pr, t.radius_s);
        ui.tooltip(
            &resp,
            "Draws the curve this cut would make. Audio does not change.",
        );
        if resp.clicked {
            ws.fx.eq.preview = if prev_on { None } else { Some(i) };
        }
        btns.cut_left(6.0);
        let armed = ws.fx.eq.confirm == Some((i, target.band));
        let label = if !target.replaces {
            format!("Apply to Band {}", target.band)
        } else if armed {
            format!("Confirm: replace Band {}", target.band)
        } else {
            format!("Replace Band {}", target.band)
        };
        let ar = btns;
        let aid = W::new(("eq-apply", i));
        let clicked = if !target.replaces || armed {
            ui.button_primary(aid, ar, &label).clicked
        } else {
            ui.button(aid, ar, &label).clicked
        };
        if clicked {
            if target.replaces && !armed {
                ws.fx.eq.confirm = Some((i, target.band));
            } else {
                apply(s, dev, target.band, cand);
                ws.fx.eq.confirm = None;
                ws.fx.eq.preview = None;
            }
        }
        r.cut_top(10.0);
    }
    if ws.fx.eq.confirm.is_some() && !list.is_empty() {
        let cancel = ui.button(
            W::new("eq-cancel"),
            r.cut_top(28.0).cut_left_ret(90.0),
            "Cancel",
        );
        if cancel.clicked {
            ws.fx.eq.confirm = None;
        }
    }
}

trait CutLeftRet {
    fn cut_left_ret(self, w: f32) -> Rect;
}

impl CutLeftRet for Rect {
    fn cut_left_ret(mut self, w: f32) -> Rect {
        self.cut_left(w)
    }
}
