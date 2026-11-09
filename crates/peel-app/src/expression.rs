//! Pitch and Pressure controls for the instrument editor.
//!
//! Both are momentary: while the mouse holds one it drives the selected
//! instrument through the session, and letting go (or losing focus, closing
//! the editor, switching channel, or Panic) sends it back to rest. Nothing
//! is saved in the project. When the mouse is not holding a control it shows
//! what the session last accepted, so a hardware wheel moves it too.

use peel_session::Session;
use peel_ui::Id as W;
use peel_ui::{Align, Color, CursorIcon, FocusKind, Rect, Role, Semantics, Ui, vec2};

use crate::instrument_editor::Rows;
use crate::workspace::Workspace;

/// Bend ranges offered in the picker, in semitones.
pub const RANGES: [f32; 8] = [1.0, 2.0, 3.0, 5.0, 7.0, 12.0, 24.0, 48.0];

/// One mouse-driven value (bend or pressure) and what the session still
/// owes it. The session refuses a send when its live queue is full, so
/// nothing here is forgotten until a send is accepted.
#[derive(Default)]
pub struct Axis {
    /// The mouse is holding the control.
    pub held: bool,
    /// What the pointer asks for while held.
    pub want: f32,
    /// What the session last accepted from this control.
    pub sent: f32,
    /// The session holds a nonzero value this control put there.
    owned: bool,
    /// Live target that value went to; a target change clears it for us.
    owner: Option<peel_document::Id>,
    /// A return to rest is still waiting for the queue.
    neutral_due: bool,
}

type Sender = fn(&mut Session, f32) -> bool;

impl Axis {
    /// A return to rest is waiting for the live queue.
    #[cfg(test)]
    pub fn neutral_due(&self) -> bool {
        self.neutral_due
    }

    /// What the control should show.
    pub fn shown(&self, session_value: f32) -> f32 {
        if self.held { self.want } else { session_value }
    }

    /// Mouse is holding the control at `v`.
    pub fn drive(&mut self, s: &mut Session, v: f32, send: Sender) {
        self.held = true;
        self.want = v;
        self.neutral_due = false;
        self.flush(s, send);
    }

    /// Mouse let go: owe the session a return to rest (only for a value this
    /// control actually put there) and try to pay it now.
    pub fn let_go(&mut self, s: &mut Session, send: Sender) {
        self.held = false;
        if self.owned {
            self.neutral_due = true;
        }
        self.flush(s, send);
    }

    /// Retries whatever the session has not accepted yet. Run every frame.
    pub fn flush(&mut self, s: &mut Session, send: Sender) {
        let target = s.live_target();
        if self.owned && self.owner != target {
            // The session already centred the old target when it changed;
            // a neutral now would wipe a hardware value on the new one.
            self.owned = false;
            self.sent = 0.0;
            self.neutral_due = false;
        }
        if self.held {
            if self.want != self.sent && send(s, self.want) {
                self.sent = self.want;
                self.owned = self.want != 0.0;
                self.owner = target;
            }
        } else if self.neutral_due && send(s, 0.0) {
            self.neutral_due = false;
            self.owned = false;
            self.sent = 0.0;
        }
    }
}

/// Mouse-held expression. Session only.
#[derive(Default)]
pub struct ExprState {
    pub bend: Axis,
    pub pressure: Axis,
}

fn send_bend(s: &mut Session, v: f32) -> bool {
    s.pitch_bend(v)
}
fn send_pressure(s: &mut Session, v: f32) -> bool {
    s.channel_pressure(v)
}

/// Lets go of anything the mouse held and tries to centre it now.
pub fn release(s: &mut Session, st: &mut ExprState) {
    st.bend.let_go(s, send_bend);
    st.pressure.let_go(s, send_pressure);
}

/// Per-frame retry of unsent values, whether or not the editor is open.
pub fn flush(s: &mut Session, st: &mut ExprState) {
    st.bend.flush(s, send_bend);
    st.pressure.flush(s, send_pressure);
}

/// "+2.0" style readout for a bend of `norm` of the range.
pub fn bend_text(norm: f32, range: f32) -> String {
    let st = norm * range;
    if st.abs() < 0.05 {
        "0".to_string()
    } else {
        format!("{st:+.1}")
    }
}

/// "±2 st", or "Off" when the wheel does nothing.
pub fn range_text(range: f32) -> String {
    if range < 0.05 {
        "Off".to_string()
    } else if (range - range.round()).abs() < 0.05 {
        format!("±{} st", range.round() as i32)
    } else {
        format!("±{range:.1} st")
    }
}

/// A horizontal control that follows the pointer while held. `bipolar`
/// draws from the middle. Returns the pointer's value (`-1..1`, or `0..1`)
/// while the mouse holds it.
#[allow(clippy::too_many_arguments)]
fn spring(
    ui: &mut Ui,
    id: W,
    r: Rect,
    shown: f32,
    bipolar: bool,
    color: Color,
    name: &str,
    value_text: &str,
) -> Option<f32> {
    let t = ui.theme.clone();
    let resp = ui.interact_node(
        id,
        r,
        FocusKind::Activate,
        Semantics::new(Role::Slider)
            .label(name)
            .numeric(
                f64::from(shown),
                if bipolar { -1.0 } else { 0.0 },
                1.0,
                0.01,
            )
            .value(value_text),
    );
    let holding = resp.pressed || resp.dragging || (resp.active && ui.input.down[0]);
    let mut out = None;
    if holding && let Some(p) = ui.input.pointer {
        let u = ((p.x - r.x - 5.0) / (r.w - 10.0).max(1.0)).clamp(0.0, 1.0);
        out = Some(if bipolar {
            let v = u * 2.0 - 1.0;
            // A small dead zone makes the centre easy to hit.
            if v.abs() < 0.04 { 0.0 } else { v }
        } else {
            u
        });
    }
    let v = out.unwrap_or(shown);
    let track = Rect {
        x: r.x + 5.0,
        y: r.center().y - 1.5,
        w: r.w - 10.0,
        h: 3.0,
    };
    ui.scene.rect(track, 1.5, t.control_hover);
    let at = |n: f32| {
        if bipolar {
            track.x + track.w * (n.clamp(-1.0, 1.0) * 0.5 + 0.5)
        } else {
            track.x + track.w * n.clamp(0.0, 1.0)
        }
    };
    let origin = at(0.0);
    let x = at(v);
    let (a, b) = if x < origin { (x, origin) } else { (origin, x) };
    ui.scene.rect(
        Rect {
            x: a,
            y: track.y,
            w: b - a,
            h: track.h,
        },
        1.5,
        color,
    );
    if bipolar {
        ui.scene.rect(
            Rect {
                x: origin - 0.5,
                y: track.y - 3.0,
                w: 1.0,
                h: track.h + 6.0,
            },
            0.0,
            t.text_faint,
        );
    }
    let hot = ui.animate_bool(id.with("hot"), resp.hovered || holding);
    ui.scene
        .circle(vec2(x, track.center().y), 5.0 + hot, t.text);
    if resp.hovered {
        ui.set_cursor(CursorIcon::EwResize);
    }
    ui.focus_ring(id, r, t.radius_s);
    out
}

/// The Pitch and Pressure rows of the Play section.
pub fn rows(ui: &mut Ui, rows: &mut Rows, s: &mut Session, ws: &mut Workspace, color: Color) {
    let t = ui.theme.clone();
    let range = s.pitch_bend_range();

    // Pitch: springs back to the centre.
    let mut row = rows.row(28.0);
    let label = row.cut_left(70.0);
    ui.text(label, "Pitch", t.caption, t.text_faint, Align::Start);
    let readout = row.cut_right(40.0);
    let shown = ws.inst.expr.bend.shown(s.pitch_bend_value());
    let text = bend_text(shown, range);
    ui.text(readout, &text, t.readout_small, t.text, Align::End);
    let held = spring(
        ui,
        W::new("inst-pitch"),
        row,
        shown,
        true,
        color,
        "Pitch bend",
        &text,
    );
    match held {
        Some(v) => ws.inst.expr.bend.drive(s, v, send_bend),
        None if ws.inst.expr.bend.held => ws.inst.expr.bend.let_go(s, send_bend),
        None => {}
    }
    rows.gap(4.0);

    // Pressure: aftertouch for held notes, back to zero on release.
    let mut row = rows.row(28.0);
    ui.text(
        row.cut_left(70.0),
        "Pressure",
        t.caption,
        t.text_faint,
        Align::Start,
    );
    let readout = row.cut_right(40.0);
    let shown = ws.inst.expr.pressure.shown(s.channel_pressure_value());
    let text = format!("{:.0}%", shown * 100.0);
    ui.text(readout, &text, t.readout_small, t.text, Align::End);
    let held = spring(
        ui,
        W::new("inst-pressure"),
        row,
        shown,
        false,
        color,
        "Pressure",
        &text,
    );
    match held {
        Some(v) => ws.inst.expr.pressure.drive(s, v, send_pressure),
        None if ws.inst.expr.pressure.held => ws.inst.expr.pressure.let_go(s, send_pressure),
        None => {}
    }
    rows.gap(4.0);
}
