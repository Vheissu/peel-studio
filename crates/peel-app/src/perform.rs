//! Performance view: launch lanes and scenes.

use peel_document::Id;
use peel_session::{Session, SlotState};
use peel_ui::{Align, CursorIcon, Id as W, Rect, Ui, icons, vec2};

use crate::util::{col, ink_on};
use crate::workspace::Workspace;

const CELL_H: f32 = 54.0;
const SCENE_W: f32 = 52.0;

pub fn draw(ui: &mut Ui, r: Rect, tools: Rect, s: &mut Session, _ws: &mut Workspace) {
    let t = ui.theme.clone();
    // Header tools: capture and stop.
    let mut row = tools;
    let stop = ui.button(W::new("perform-stop"), row.cut_right(96.0), "Stop all");
    if stop.clicked {
        s.stop_performance();
    }
    row.cut_right(8.0);
    let capturing = s.performance_capturing();
    let cap = ui.icon_button_fill(
        W::new("perform-capture"),
        row.cut_right(34.0),
        icons::RECORD,
        capturing,
        t.record,
        14.0,
    );
    ui.tooltip(
        &cap,
        if capturing {
            "Stop capturing: the take becomes an arrangement"
        } else {
            "Capture the performance as a new arrangement"
        },
    );
    if cap.clicked {
        s.set_performance_capture(!capturing);
    }
    row.cut_right(10.0);
    let hint = if s.performing() {
        "Launches start on the next bar"
    } else {
        "Click a clip to launch it"
    };
    ui.text(row, hint, t.caption, t.text_faint, Align::End);

    let lanes: Vec<(Id, String, Vec<peel_session::PerformSlot>)> = {
        let names: std::collections::HashMap<Id, String> = s
            .project()
            .arrangement()
            .tracks
            .iter()
            .map(|t| (t.id, t.name.clone()))
            .collect();
        s.performance_slots()
            .into_iter()
            .filter(|(_, slots)| !slots.is_empty())
            .map(|(lane, slots)| (lane, names.get(&lane).cloned().unwrap_or_default(), slots))
            .collect()
    };
    let mut body = r.shrink2(12.0, 8.0);
    if lanes.is_empty() {
        ui.text(
            body.with_h(60.0),
            "Put patterns or audio on Playlist lanes to launch them here",
            t.body,
            t.text_faint,
            Align::Center,
        );
        return;
    }
    let scenes = lanes.iter().map(|l| l.2.len()).max().unwrap_or(0);
    let head = body.cut_top(28.0);
    body.cut_top(6.0);
    let mut cols = body;
    let scene_col = cols.cut_left(SCENE_W);
    cols.cut_left(8.0);
    let col_w =
        ((cols.w - 8.0 * (lanes.len() as f32 - 1.0)) / lanes.len() as f32).clamp(110.0, 220.0);
    let pulse = 0.5 + 0.5 * (ui.time() as f32 * 6.0).sin();

    // Scene launchers.
    for i in 0..scenes {
        let cell = Rect {
            y: scene_col.y + i as f32 * (CELL_H + 6.0),
            h: CELL_H,
            ..scene_col
        };
        if cell.bottom() > body.bottom() {
            break;
        }
        let resp = ui.interact_button(
            W::new(("perform-scene", i)),
            cell,
            &format!("Scene {}", i + 1),
        );
        ui.scene.rect(
            cell,
            t.radius_m,
            if resp.hovered {
                t.control_hover
            } else {
                t.control
            },
        );
        let mut inner = cell.shrink2(8.0, 0.0);
        ui.icon(inner.cut_left(14.0), icons::PLAY, 11.0, t.text_dim);
        ui.text(inner, &format!("{}", i + 1), t.label, t.text, Align::Center);
        ui.tooltip(&resp, "Launch this row");
        if resp.clicked {
            s.launch_scene(i);
        }
    }

    for (li, (lane, name, slots)) in lanes.iter().enumerate() {
        let x = cols.x + li as f32 * (col_w + 8.0);
        if x + col_w > cols.right() + 1.0 {
            break;
        }
        // Lane header with its stop button.
        let mut h = Rect {
            x,
            w: col_w,
            ..head
        };
        let stop_r = h.cut_right(26.0);
        let st = ui.icon_button(
            W::new(("perform-lane-stop", *lane)),
            stop_r,
            icons::STOP,
            false,
            None,
        );
        ui.tooltip(&st, "Stop this lane");
        if st.clicked {
            s.stop_lane(*lane);
        }
        let shown = ui.fit_text(name, t.label, h.w);
        ui.text(h, &shown, t.label, t.text_dim, Align::Start);

        for (i, slot) in slots.iter().enumerate() {
            let cell = Rect {
                x,
                y: body.y + i as f32 * (CELL_H + 6.0),
                w: col_w,
                h: CELL_H,
            };
            if cell.bottom() > body.bottom() {
                break;
            }
            let state = s.slot_state(*lane, slot.clip);
            let id = W::new(("perform-slot", slot.clip));
            let resp = ui.interact_button(id, cell, &slot.name);
            let c = col(slot.color);
            let (fill, ink) = match state {
                SlotState::Playing => (c, ink_on(c)),
                SlotState::Queued => (c.alpha(0.25 + 0.35 * pulse), t.text),
                SlotState::Stopping => (c.alpha(0.5 + 0.3 * pulse), ink_on(c)),
                SlotState::Idle => (c.alpha(if resp.hovered { 0.28 } else { 0.16 }), t.text),
            };
            ui.scene.rect(cell, t.radius_m, fill);
            if state == SlotState::Queued {
                ui.scene.outline(cell, t.radius_m, 1.5, c);
            }
            let mut inner = cell.shrink2(10.0, 0.0);
            let icon = match state {
                SlotState::Playing | SlotState::Stopping => icons::SQUARES,
                _ => icons::PLAY,
            };
            ui.icon(inner.cut_left(16.0), icon, 12.0, ink);
            inner.cut_left(6.0);
            let shown = ui.fit_text(&slot.name, t.label, inner.w);
            ui.text(inner, &shown, t.label, ink, Align::Start);
            if state == SlotState::Playing {
                // Where in the bar the performance is.
                let bar = s.project().time_signature.bar_length().as_f64();
                let f = (s.position().max(0.0) % bar / bar) as f32;
                let track = Rect {
                    y: cell.bottom() - 5.0,
                    h: 3.0,
                    ..cell.shrink2(8.0, 0.0)
                };
                ui.scene.rect(track, 1.5, ink.alpha(0.25));
                ui.scene
                    .rect(track.with_w(track.w * f), 1.5, ink.alpha(0.8));
            }
            if resp.hovered {
                ui.set_cursor(CursorIcon::Pointer);
            }
            if resp.clicked {
                match state {
                    SlotState::Playing => s.stop_lane(*lane),
                    _ => s.launch(*lane, slot.clip),
                }
            }
        }
    }
    let _ = vec2;
}
