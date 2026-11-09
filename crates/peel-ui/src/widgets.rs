//! Standard widgets. Every widget takes an explicit rectangle; layout is
//! done by the caller with the rect-cut helpers on [`Rect`].

use std::f32::consts::PI;

use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

use accesskit::Action;

use crate::a11y::{RANK_ICON, RANK_TEXT, Role, Semantics, adjust_norm};
use crate::focus::FocusKind;
use crate::geom::{Color, Rect, Vec2, vec2};
use crate::icons;
use crate::input::Button;
use crate::scene::{Align, LayerId};
use crate::text::TextStyle;
use crate::ui::{Id, Response, Ui};

/// Knob sweep: 270 degrees centered on 12 o'clock.
const KNOB_START: f32 = -0.75 * PI;
const KNOB_SWEEP: f32 = 1.5 * PI;

/// One entry of a menu.
#[derive(Clone, Debug)]
pub enum MenuItem {
    Item {
        label: String,
        shortcut: Option<String>,
        enabled: bool,
        checked: bool,
        color: Option<Color>,
    },
    Separator,
    Heading(String),
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> MenuItem {
        MenuItem::Item {
            label: label.into(),
            shortcut: None,
            enabled: true,
            checked: false,
            color: None,
        }
    }

    pub fn shortcut(self, s: impl Into<String>) -> MenuItem {
        match self {
            MenuItem::Item {
                label,
                enabled,
                checked,
                color,
                ..
            } => MenuItem::Item {
                label,
                shortcut: Some(s.into()),
                enabled,
                checked,
                color,
            },
            other => other,
        }
    }

    pub fn enabled(self, on: bool) -> MenuItem {
        match self {
            MenuItem::Item {
                label,
                shortcut,
                checked,
                color,
                ..
            } => MenuItem::Item {
                label,
                shortcut,
                enabled: on,
                checked,
                color,
            },
            other => other,
        }
    }

    pub fn checked(self, on: bool) -> MenuItem {
        match self {
            MenuItem::Item {
                label,
                shortcut,
                enabled,
                color,
                ..
            } => MenuItem::Item {
                label,
                shortcut,
                enabled,
                checked: on,
                color,
            },
            other => other,
        }
    }

    pub fn swatch(self, c: Color) -> MenuItem {
        match self {
            MenuItem::Item {
                label,
                shortcut,
                enabled,
                checked,
                ..
            } => MenuItem::Item {
                label,
                shortcut,
                enabled,
                checked,
                color: Some(c),
            },
            other => other,
        }
    }
}

#[derive(Default)]
struct TextEditState {
    buffer: String,
    editing: bool,
    cursor: usize,
    select_all: bool,
}

#[derive(Default)]
struct DragValueState {
    start: f64,
    accum: f32,
}

#[derive(Default)]
struct MeterState {
    level: [f32; 2],
    hold: [f32; 2],
    hold_time: [f64; 2],
    /// Level last reported to assistive technology, in whole dB steps, and
    /// when. Reported rarely so a moving meter does not flood the reader.
    spoken_db: Option<i32>,
    spoken_at: f64,
}

/// Whether an icon button has ever been on, which marks it as a toggle.
#[derive(Default)]
struct ToggleSeen(bool);

/// Keyboard step of a splitter, in logical pixels.
const SPLITTER_STEP: f32 = 16.0;
/// Pointer grab width of a splitter, however thin its seam is drawn.
const SPLITTER_HIT: f32 = 8.0;

/// Icon fonts live in the Unicode private use area.
fn is_icon_glyph(label: &str) -> bool {
    label
        .chars()
        .next()
        .is_some_and(|c| ('\u{e000}'..='\u{f8ff}').contains(&c))
}

/// Linear peak to a 0..=1 meter position (-60 dB .. +6 dB).
pub fn meter_pos(peak: f32) -> f32 {
    if peak <= 1e-6 {
        return 0.0;
    }
    let db = 20.0 * peak.log10();
    ((db + 60.0) / 66.0).clamp(0.0, 1.0)
}

impl Ui {
    // ---- buttons ----

    /// Text button.
    pub fn button(&mut self, id: Id, r: Rect, label: &str) -> Response {
        let resp = self.interact(id, r);
        self.focusable(id, r, FocusKind::Activate);
        self.a11y_node(id, r, RANK_TEXT, || {
            Semantics::new(Role::Button).label(label)
        });
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        let fill = if resp.active {
            t.control_active
        } else {
            t.control.mix(t.control_hover, hot)
        };
        self.scene.rect(r, t.radius_m, fill);
        self.text(r, label, t.label, t.text, Align::Center);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.focus_ring(id, r, t.radius_m);
        resp
    }

    /// Primary (amber) button.
    pub fn button_primary(&mut self, id: Id, r: Rect, label: &str) -> Response {
        let resp = self.interact(id, r);
        self.focusable(id, r, FocusKind::Activate);
        self.a11y_node(id, r, RANK_TEXT, || {
            Semantics::new(Role::Button).label(label)
        });
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        let fill = if resp.active {
            t.accent.darken(0.12)
        } else {
            t.accent.lighten(hot * 0.08)
        };
        self.scene.rect(r, t.radius_m, fill);
        self.text(r, label, t.label, t.text_on_accent, Align::Center);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.focus_ring(id, r, t.radius_m);
        resp
    }

    /// Square icon button; `on` shows the toggled state in `on_color`.
    pub fn icon_button(
        &mut self,
        id: Id,
        r: Rect,
        icon: &str,
        on: bool,
        on_color: Option<Color>,
    ) -> Response {
        let resp = self.interact(id, r);
        self.focusable(id, r, FocusKind::Activate);
        self.describe_icon_button(id, r, icon, on);
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        let accent = on_color.unwrap_or(t.accent);
        if on {
            self.scene
                .rect(r, t.radius_m, accent.with_alpha(0.16 + hot * 0.06));
        } else if hot > 0.0 || resp.active {
            let a = if resp.active { 1.0 } else { hot };
            self.scene.rect(r, t.radius_m, t.control.alpha(a));
        }
        let color = if on {
            accent
        } else {
            t.text_dim.mix(t.text, hot)
        };
        self.icon(r, icon, (r.h * 0.6).min(20.0), color);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.focus_ring(id, r, t.radius_m);
        resp
    }

    /// Filled-icon variant used by transport buttons.
    pub fn icon_button_fill(
        &mut self,
        id: Id,
        r: Rect,
        icon: &str,
        on: bool,
        on_color: Color,
        size: f32,
    ) -> Response {
        let resp = self.interact(id, r);
        self.focusable(id, r, FocusKind::Activate);
        self.describe_icon_button(id, r, icon, on);
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        // Quiet until hovered; only the on state carries a tint.
        if on {
            self.scene
                .rect(r, t.radius_m, on_color.with_alpha(0.18 + hot * 0.06));
        } else if hot > 0.0 || resp.active {
            let a = if resp.active { 1.0 } else { hot };
            self.scene.rect(r, t.radius_m, t.control.alpha(a));
        }
        let color = if on {
            on_color
        } else {
            t.text_dim.mix(t.text, hot)
        };
        self.icon_fill(r, icon, size, color);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.focus_ring(id, r, t.radius_m);
        resp
    }

    /// An icon button is named after its icon until a tooltip or an explicit
    /// name says better. A button that has been `on` is a toggle from then
    /// on; one that never was cannot be told from a plain button (say
    /// `Semantics::patch().toggled(false)` to declare a toggle early).
    fn describe_icon_button(&mut self, id: Id, r: Rect, icon: &str, on: bool) {
        if !self.accessibility_active() {
            return;
        }
        let seen = &mut self.state::<ToggleSeen>(id.with("a11y-toggle")).0;
        *seen |= on;
        let toggle = *seen;
        self.a11y_node(id, r, RANK_ICON, || {
            let mut s = Semantics::new(Role::Button);
            if toggle {
                s = s.toggled(on);
            }
            match icons::name(icon) {
                Some(n) => s.label(n),
                None => s,
            }
        });
    }

    /// Small round LED switch.
    pub fn led(&mut self, id: Id, r: Rect, on: bool, color: Color) -> Response {
        let resp = self.interact(id, r);
        self.focusable(id, r, FocusKind::Activate);
        self.a11y_node(id, r, RANK_ICON, || {
            Semantics::new(Role::Switch).toggled(on)
        });
        let t = self.theme.clone();
        let c = r.center();
        let rad = (r.w.min(r.h) * 0.5 - 1.0).clamp(3.0, 5.0);
        let k = self.animate_bool(id.with("led"), on);
        // No halo: lit LEDs are a flat, slightly muted dot.
        let fill = t.control_hover.mix(color, k * 0.82);
        let stroke = if resp.hovered {
            t.text_dim
        } else {
            t.line_strong.mix(color.lighten(0.2), k)
        };
        self.scene.circle_stroke(c, rad, fill, 1.0, stroke);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.focus_ring(id, r, r.w.min(r.h) * 0.5);
        resp
    }

    /// Segmented control. Returns true when the selection changes.
    pub fn segmented(&mut self, id: Id, r: Rect, options: &[&str], selected: &mut usize) -> bool {
        let t = self.theme.clone();
        self.scene.rect(r, t.radius_m, t.well);
        let inner = r.shrink(2.0);
        let cells = inner.columns(options.len(), 2.0);
        let mut changed = false;
        self.a11y_node(id, r, RANK_TEXT, || Semantics::new(Role::RadioGroup));
        self.a11y_push(id);
        // Only the selected option is a tab stop. Arrow keys move the
        // selection (and focus with it) like a radio group.
        let old_sel = *selected;
        let steps = if options.is_empty() {
            0
        } else {
            self.take_adjust(id.with(old_sel)).steps
        };
        if steps != 0 {
            let next = (old_sel as i32 + steps).rem_euclid(options.len() as i32) as usize;
            if next != old_sel {
                *selected = next;
                changed = true;
            }
        }
        let sel_x = self.animate(id.with("sel"), *selected as f32, 22.0);
        if let Some(first) = cells.first() {
            let step = first.w + 2.0;
            let sr = Rect {
                x: inner.x + sel_x * step,
                ..*first
            };
            self.scene.rect(sr, t.radius_s, t.control_hover);
        }
        for (i, (cell, label)) in cells.iter().zip(options).enumerate() {
            let seg = id.with(i);
            let resp = self.interact(seg, *cell);
            let is_icon = is_icon_glyph(label);
            if resp.clicked && *selected != i {
                *selected = i;
                changed = true;
            }
            let color = if *selected == i {
                t.text
            } else if resp.hovered {
                t.text_dim.mix(t.text, 0.5)
            } else {
                t.text_dim
            };
            if is_icon {
                self.icon(*cell, label, 15.0, color);
            } else {
                self.text(*cell, label, t.label, color, Align::Center);
            }
            if resp.hovered {
                self.set_cursor(CursorIcon::Pointer);
            }
            self.focus_ring(seg, *cell, t.radius_s);
        }
        // The focus stop follows the selection, however it changed.
        if *selected != old_sel && self.kb_focus == Some(id.with(old_sel)) {
            self.set_kb_focus(Some(id.with(*selected)));
        }
        if let Some(cell) = cells.get(*selected) {
            self.focusable(id.with(*selected), *cell, FocusKind::Choice);
        }
        // Describe the options last, from the final selection.
        for (i, (cell, label)) in cells.iter().zip(options).enumerate() {
            let on = *selected == i;
            self.a11y_node(id.with(i), *cell, RANK_TEXT, || {
                let name: &str = if is_icon_glyph(label) {
                    icons::name(label).unwrap_or("")
                } else {
                    label
                };
                Semantics::new(Role::RadioButton).label(name).toggled(on)
            });
        }
        self.a11y_pop(id);
        changed
    }

    // ---- continuous controls ----

    /// Rotary knob editing a normalized value. Drag vertically (Shift for
    /// fine), scroll, double-click to reset.
    #[allow(clippy::too_many_arguments)]
    pub fn knob(
        &mut self,
        id: Id,
        r: Rect,
        norm: &mut f32,
        default: f32,
        color: Color,
        bipolar: bool,
    ) -> Response {
        let mut resp = self.interact(id, r);
        self.focusable(id, r, FocusKind::Adjust);
        let adj = self.take_adjust(id);
        if adjust_norm(&adj, norm, default) {
            resp.changed = true;
        }
        if resp.double_clicked {
            *norm = default;
            resp.changed = true;
        } else if resp.dragging {
            let fine = if self.input.modifiers.shift { 0.2 } else { 1.0 };
            let d = -resp.drag_delta.y / 180.0 * fine;
            if d != 0.0 {
                *norm = (*norm + d).clamp(0.0, 1.0);
                resp.changed = true;
            }
        }
        if resp.hovered && self.input.scroll.y != 0.0 && !self.input.modifiers.command() {
            let fine = if self.input.modifiers.shift { 0.2 } else { 1.0 };
            *norm = (*norm + self.input.scroll.y / 600.0 * fine).clamp(0.0, 1.0);
            self.input.scroll.y = 0.0;
            resp.changed = true;
        }
        if resp.hovered || resp.active {
            self.set_cursor(CursorIcon::NsResize);
        }
        self.draw_knob(id, r, *norm, color, bipolar, resp.hovered || resp.active);
        self.focus_ring(id, r, r.w.min(r.h) * 0.5);
        self.describe_norm(id, r, *norm, None);
        resp
    }

    /// Describes a 0..=1 control as a slider reporting percent.
    fn describe_norm(&mut self, id: Id, r: Rect, norm: f32, vertical: Option<bool>) {
        self.a11y_node(id, r, 0, || {
            let pct = f64::from(norm.clamp(0.0, 1.0)) * 100.0;
            let mut s = Semantics::new(Role::Slider)
                .numeric(pct, 0.0, 100.0, 1.0)
                .value(format!("{pct:.0}%"));
            if let Some(v) = vertical {
                s = s.vertical(v);
            }
            s
        });
    }

    pub fn draw_knob(
        &mut self,
        id: Id,
        r: Rect,
        norm: f32,
        color: Color,
        bipolar: bool,
        hot: bool,
    ) {
        let t = self.theme.clone();
        let size = r.w.min(r.h);
        let c = r.center();
        // A thin value arc around a flat cap with a pointer: the arc and
        // pointer carry the value, the cap only gives it something to sit on.
        let thick = (size * 0.07).clamp(1.5, 2.5);
        let ring = size * 0.5 - thick * 0.5 - 0.5;
        let h = self.animate_bool(id.with("hot"), hot);
        self.scene
            .arc(c, ring, thick, KNOB_START, KNOB_SWEEP, t.line_strong);
        let v = norm.clamp(0.0, 1.0);
        let col = color.lighten(h * 0.12);
        if bipolar {
            let mid = KNOB_START + KNOB_SWEEP * 0.5;
            let a = KNOB_START + KNOB_SWEEP * v;
            let (s, e) = if a < mid { (a, mid) } else { (mid, a) };
            self.scene.arc(c, ring, thick, s, (e - s).max(0.001), col);
        } else if v > 0.0 {
            self.scene
                .arc(c, ring, thick, KNOB_START, KNOB_SWEEP * v, col);
        }
        let body = ring - thick * 0.5 - 1.5;
        self.scene
            .circle(c, body, t.control.mix(t.control_hover, h * 0.7));
        let a = KNOB_START + KNOB_SWEEP * v;
        let dir = vec2(a.sin(), -a.cos());
        self.scene.line(
            c + dir * (body * 0.2),
            c + dir * (body - 0.5),
            (size * 0.06).clamp(1.5, 2.0),
            t.text,
        );
    }

    /// Vertical fader. `norm` is the fader position (0..=1).
    pub fn fader(
        &mut self,
        id: Id,
        r: Rect,
        norm: &mut f32,
        default: f32,
        color: Color,
    ) -> Response {
        let mut resp = self.interact(id, r);
        self.focusable(id, r, FocusKind::Adjust);
        let adj = self.take_adjust(id);
        if adjust_norm(&adj, norm, default) {
            resp.changed = true;
        }
        let cap_h = 16.0;
        let travel = (r.h - cap_h).max(1.0);
        if resp.double_clicked {
            *norm = default;
            resp.changed = true;
        } else if resp.dragging && resp.drag_delta.y != 0.0 {
            let fine = if self.input.modifiers.shift { 0.2 } else { 1.0 };
            *norm = (*norm - resp.drag_delta.y / travel * fine).clamp(0.0, 1.0);
            resp.changed = true;
        }
        if resp.hovered && self.input.scroll.y != 0.0 {
            *norm = (*norm + self.input.scroll.y / 800.0).clamp(0.0, 1.0);
            self.input.scroll.y = 0.0;
            resp.changed = true;
        }
        if resp.hovered || resp.active {
            self.set_cursor(CursorIcon::NsResize);
        }
        let t = self.theme.clone();
        let cx = r.center().x;
        let track = Rect {
            x: cx - 1.5,
            y: r.y + cap_h * 0.5,
            w: 3.0,
            h: travel,
        };
        self.scene.rect(track, 0.0, t.bg);
        // Scale ticks on the left only, longer at the ends and the middle.
        for i in 0..=10 {
            let y = track.y + travel * i as f32 / 10.0;
            let w = if i % 5 == 0 { 6.0 } else { 3.0 };
            self.scene.rect(
                Rect {
                    x: cx - 9.0 - w,
                    y: y.round() - 0.5,
                    w,
                    h: 1.0,
                },
                0.0,
                t.line_strong,
            );
        }
        let v = norm.clamp(0.0, 1.0);
        let cap_y = r.y + (1.0 - v) * travel;
        let fill = Rect {
            x: track.x,
            y: cap_y + cap_h * 0.5,
            w: track.w,
            h: track.bottom() - (cap_y + cap_h * 0.5),
        };
        self.scene.rect(fill, 0.0, color.with_alpha(0.8));
        let hot = self.animate_bool(id.with("hot"), resp.hovered || resp.active);
        let cap = Rect {
            x: cx - 11.0,
            y: cap_y,
            w: 22.0,
            h: cap_h,
        };
        // Flat cap with a dark rim so it reads over the fill; the index
        // line marks the exact position.
        self.scene.rect_stroke(
            cap,
            t.radius_s,
            t.control_active.lighten(hot * 0.08),
            1.0,
            t.bg,
        );
        self.scene.rect(
            Rect {
                x: cap.x + 3.0,
                y: cap.center().y - 1.0,
                w: cap.w - 6.0,
                h: 2.0,
            },
            0.0,
            t.text,
        );
        self.focus_ring(id, r, t.radius_m);
        self.describe_norm(id, r, *norm, Some(true));
        resp
    }

    /// Horizontal slider editing a normalized value.
    pub fn hslider(
        &mut self,
        id: Id,
        r: Rect,
        norm: &mut f32,
        default: f32,
        color: Color,
    ) -> Response {
        let mut resp = self.interact(id, r);
        self.focusable(id, r, FocusKind::Adjust);
        let adj = self.take_adjust(id);
        if adjust_norm(&adj, norm, default) {
            resp.changed = true;
        }
        if resp.double_clicked {
            *norm = default;
            resp.changed = true;
        } else if (resp.pressed || resp.dragging)
            && let Some(p) = self.input.pointer
        {
            let v = ((p.x - r.x - 5.0) / (r.w - 10.0).max(1.0)).clamp(0.0, 1.0);
            if (v - *norm).abs() > f32::EPSILON {
                *norm = v;
                resp.changed = true;
            }
        }
        let t = self.theme.clone();
        let track = Rect {
            x: r.x + 5.0,
            y: r.center().y - 1.5,
            w: r.w - 10.0,
            h: 3.0,
        };
        self.scene.rect(track, 1.5, t.control_hover);
        let v = norm.clamp(0.0, 1.0);
        self.scene.rect(track.with_w(track.w * v), 1.5, color);
        let hot = self.animate_bool(id.with("hot"), resp.hovered || resp.active);
        self.scene.circle(
            vec2(track.x + track.w * v, track.center().y),
            5.0 + hot,
            t.text,
        );
        if resp.hovered {
            self.set_cursor(CursorIcon::EwResize);
        }
        self.focus_ring(id, r, t.radius_s);
        self.describe_norm(id, r, *norm, Some(false));
        resp
    }

    /// A number readout edited by dragging vertically or typing after a
    /// double-click. `fmt` renders the value.
    #[allow(clippy::too_many_arguments)]
    pub fn drag_number(
        &mut self,
        id: Id,
        r: Rect,
        value: &mut f64,
        min: f64,
        max: f64,
        per_px: f64,
        style: TextStyle,
        color: Color,
        fmt: impl Fn(f64) -> String,
    ) -> Response {
        // Keyboard: arrow keys step by about five pixels of dragging, and
        // Enter types a value.
        let step = (per_px * 5.0).max(f64::EPSILON);
        self.focusable(id, r, FocusKind::Spin);
        let resp =
            self.drag_number_inner(id, r, value, (min, max), (per_px, step), style, color, &fmt);
        // Described last, from the value as it ended up this frame.
        self.a11y_node(id, r, 0, || {
            Semantics::new(Role::SpinButton)
                .numeric(*value, min, max, step)
                .value(fmt(*value))
        });
        resp
    }

    #[allow(clippy::too_many_arguments)]
    fn drag_number_inner(
        &mut self,
        id: Id,
        r: Rect,
        value: &mut f64,
        (min, max): (f64, f64),
        (per_px, step): (f64, f64),
        style: TextStyle,
        color: Color,
        fmt: &impl Fn(f64) -> String,
    ) -> Response {
        if self.state::<TextEditState>(id.with("edit")).editing {
            let mut s = std::mem::take(&mut self.state::<TextEditState>(id.with("edit")).buffer);
            let resp = self.text_edit_inner(id.with("edit"), id, r, &mut s, style);
            let done = resp.changed;
            self.state::<TextEditState>(id.with("edit")).buffer = s.clone();
            if done {
                if let Ok(v) = s
                    .trim()
                    .trim_end_matches(|c: char| c.is_alphabetic() || c == ' ')
                    .parse::<f64>()
                {
                    *value = v.clamp(min, max);
                }
                let st = self.state::<TextEditState>(id.with("edit"));
                st.editing = false;
                let mut out = Response {
                    id,
                    rect: r,
                    changed: true,
                    ..Default::default()
                };
                out.released = true;
                return out;
            }
            return Response {
                id,
                rect: r,
                ..Default::default()
            };
        }
        let mut resp = self.interact(id, r);
        let t = self.theme.clone();
        let adj = self.take_adjust(id);
        if adj.any() {
            let nv = adj.apply(*value, min, max, step, None, 1.0);
            if nv != *value {
                *value = nv;
                resp.changed = true;
            }
        }
        if resp.double_clicked || self.take_activate(id) {
            let text = fmt(*value);
            let st = self.state::<TextEditState>(id.with("edit"));
            st.editing = true;
            st.buffer = text;
            st.cursor = st.buffer.chars().count();
            st.select_all = true;
            self.request_focus(id.with("edit"));
        } else if resp.pressed {
            let start = *value;
            let st = self.state::<DragValueState>(id);
            st.start = start;
            st.accum = 0.0;
        } else if resp.dragging {
            let fine = if self.input.modifiers.shift { 0.1 } else { 1.0 };
            let dy = -resp.drag_delta.y;
            let st = self.state::<DragValueState>(id);
            st.accum += dy;
            let nv = (st.start + st.accum as f64 * per_px * fine).clamp(min, max);
            if nv != *value {
                *value = nv;
                resp.changed = true;
            }
        }
        if resp.hovered || resp.active {
            self.set_cursor(CursorIcon::NsResize);
            self.scene.rect(r, t.radius_s, t.control.alpha(0.6));
        }
        let s = fmt(*value);
        self.text(r, &s, style, color, Align::Center);
        self.focus_ring(id, r, t.radius_s);
        resp
    }

    // ---- text input ----

    /// Focuses a text field, optionally selecting its contents so typing
    /// replaces them.
    pub fn focus_text_edit(&mut self, id: Id, select_all: bool) {
        self.request_focus(id);
        let st = self.state::<TextEditState>(id);
        st.select_all = select_all;
        st.editing = true;
        st.cursor = usize::MAX;
    }

    /// Single-line text field. `changed` is set when Enter commits.
    pub fn text_edit(&mut self, id: Id, r: Rect, text: &mut String, placeholder: &str) -> Response {
        let style = self.theme.body;
        let t = self.theme.clone();
        self.focusable(id, r, FocusKind::Text);
        // Tabbing (or an assistive technology) onto the field, or Enter on
        // it, starts editing.
        let entered = self.take_kb_entered(id);
        let activated = self.take_activate(id);
        if !self.has_focus(id) && (entered || activated) {
            self.focus_text_edit(id, true);
        }
        let focused = self.has_focus(id);
        self.scene.rect_stroke(
            r,
            t.radius_m,
            t.bg,
            1.0,
            if focused {
                t.focus.alpha(0.7)
            } else {
                t.line_strong
            },
        );
        if !focused && text.is_empty() {
            self.text(
                r.shrink2(8.0, 0.0),
                placeholder,
                style,
                t.text_faint,
                Align::Start,
            );
        }
        let mut resp = self.text_edit_inner(id, id, r.shrink2(8.0, 0.0), text, style);
        if !focused && resp.pressed {
            self.request_focus(id);
            let n = text.chars().count();
            let st = self.state::<TextEditState>(id);
            st.cursor = n;
            st.editing = true;
        }
        // Text sent by an assistive technology: *SetValue* replaces the
        // content and commits it; *ReplaceSelectedText* types at the cursor.
        if let Some(a) = self.take_action(id, Action::SetValue)
            && let Some(accesskit::ActionData::Value(v)) = a.data
        {
            *text = v.to_string();
            resp.changed = true;
        }
        if let Some(a) = self.take_action(id, Action::ReplaceSelectedText)
            && let Some(accesskit::ActionData::Value(v)) = a.data
        {
            let st = self.state::<TextEditState>(id);
            if st.select_all || !st.editing {
                text.clear();
                st.cursor = 0;
                st.select_all = false;
            }
            let at = st.cursor.min(text.chars().count());
            let byte = text.char_indices().nth(at).map_or(text.len(), |(b, _)| b);
            text.insert_str(byte, &v);
            st.cursor = at + v.chars().count();
        }
        if !focused {
            self.focus_ring(id, r, t.radius_m);
        }
        // Described last, from the text as it ended up this frame.
        self.a11y_node(id, r, RANK_ICON, || {
            let mut s = Semantics::new(Role::TextInput).value(text.as_str());
            if !placeholder.is_empty() {
                s = s.placeholder(placeholder).label(placeholder);
            }
            s
        });
        resp
    }

    /// `nav_from` is the focus stop Tab moves on from when the field commits.
    fn text_edit_inner(
        &mut self,
        id: Id,
        nav_from: Id,
        r: Rect,
        text: &mut String,
        style: TextStyle,
    ) -> Response {
        let mut resp = self.interact(id, r);
        if resp.hovered {
            self.set_cursor(CursorIcon::Text);
        }
        let focused = self.has_focus(id);
        let t = self.theme.clone();
        if focused {
            let typed = std::mem::take(&mut self.input.text);
            let mut st_cursor = self
                .state::<TextEditState>(id)
                .cursor
                .min(text.chars().count());
            let mut select_all = self.state::<TextEditState>(id).select_all;
            if !typed.is_empty() {
                if select_all {
                    text.clear();
                    st_cursor = 0;
                    select_all = false;
                }
                let byte = text
                    .char_indices()
                    .nth(st_cursor)
                    .map_or(text.len(), |(b, _)| b);
                text.insert_str(byte, &typed);
                st_cursor += typed.chars().count();
            }
            let keys: Vec<Key> = self
                .input
                .keys
                .iter()
                .filter(|k| !k.consumed)
                .map(|k| k.key.clone())
                .collect();
            let command = self.input.modifiers.command();
            let mut tab = None;
            for k in &keys {
                match k {
                    Key::Named(NamedKey::Backspace) => {
                        if select_all {
                            text.clear();
                            st_cursor = 0;
                            select_all = false;
                        } else if st_cursor > 0 {
                            let byte = text
                                .char_indices()
                                .nth(st_cursor - 1)
                                .map(|(b, _)| b)
                                .unwrap_or(0);
                            text.remove(byte);
                            st_cursor -= 1;
                        }
                    }
                    Key::Named(NamedKey::Delete) => {
                        if st_cursor < text.chars().count() {
                            let byte = text
                                .char_indices()
                                .nth(st_cursor)
                                .map(|(b, _)| b)
                                .unwrap_or(0);
                            text.remove(byte);
                        }
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        select_all = false;
                        st_cursor = st_cursor.saturating_sub(1);
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        select_all = false;
                        st_cursor = (st_cursor + 1).min(text.chars().count());
                    }
                    Key::Named(NamedKey::Home) => st_cursor = 0,
                    Key::Named(NamedKey::End) => st_cursor = text.chars().count(),
                    Key::Named(NamedKey::Enter) => {
                        resp.changed = true;
                        self.clear_focus();
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.clear_focus();
                        let st = self.state::<TextEditState>(id);
                        st.editing = false;
                    }
                    // Tab commits like Enter and moves on.
                    Key::Named(NamedKey::Tab) => {
                        resp.changed = true;
                        self.clear_focus();
                        tab = Some(self.input.modifiers.shift);
                    }
                    Key::Character(c) if command && c.as_str() == "a" => select_all = true,
                    _ => {}
                }
            }
            for k in self.input.keys.iter_mut() {
                k.consumed = true;
            }
            if let Some(back) = tab {
                self.set_kb_focus(Some(nav_from));
                self.kb_move(if back { -1 } else { 1 });
            }
            let st = self.state::<TextEditState>(id);
            st.cursor = st_cursor;
            st.select_all = select_all;
            let before: String = text.chars().take(st_cursor).collect();
            let w = self.measure(&before, style).x;
            let full = self.measure(text, style);
            if select_all && !text.is_empty() {
                let sr = Rect {
                    x: r.x - 1.0,
                    y: r.center().y - full.y * 0.5,
                    w: full.x + 2.0,
                    h: full.y,
                };
                self.scene.rect(sr, 2.0, t.focus.alpha(0.3));
            }
            let blink = ((self.time() * 1.6).fract() < 0.6) as i32 as f32;
            let cx = r.x + w;
            self.scene.rect(
                Rect {
                    x: cx,
                    y: r.center().y - style.size * 0.65,
                    w: 1.5,
                    h: style.size * 1.3,
                },
                0.0,
                t.focus.alpha(blink),
            );
            self.request_repaint();
            // An outside click ends editing.
            if self.input.pressed(Button::Left) && !resp.hovered {
                resp.changed = true;
                self.clear_focus();
            }
        }
        self.with_clip(r, |ui| ui.text(r, text, style, t.text, Align::Start));
        resp
    }

    // ---- meters ----

    /// Stereo peak meter with ballistics and peak hold. `peaks` are linear
    /// values read from telemetry since the last frame.
    pub fn meter(&mut self, id: Id, r: Rect, peaks: (f32, f32)) {
        let t = self.theme.clone();
        let now = self.time();
        let dt = self.dt();
        let st = self.state::<MeterState>(id);
        let mut animating = false;
        for (i, p) in [peaks.0, peaks.1].into_iter().enumerate() {
            let target = meter_pos(p);
            if target >= st.level[i] {
                st.level[i] = target;
            } else {
                // ~20 dB/s fall.
                st.level[i] = (st.level[i] - dt * 0.3).max(target);
                animating = true;
            }
            if target >= st.hold[i] {
                st.hold[i] = target;
                st.hold_time[i] = now;
            } else if now - st.hold_time[i] > 1.2 {
                st.hold[i] = (st.hold[i] - dt * 0.5).max(st.level[i]);
                animating = true;
            }
            if st.level[i] > 0.0 || st.hold[i] > 0.0 {
                animating = true;
            }
        }
        let level = st.level;
        let hold = st.hold;
        if animating {
            self.request_repaint();
        }
        if self.accessibility_active() {
            self.describe_meter(id, r, peaks.0.max(peaks.1), now);
        }
        let bars = r.columns(2, 1.0);
        for (i, b) in bars.iter().enumerate() {
            self.scene.rect(*b, 0.0, t.bg);
            let h = b.h * level[i];
            if h > 0.5 {
                let fill = Rect {
                    y: b.bottom() - h,
                    h,
                    ..*b
                };
                // Green body, amber above -6 dB, coral above 0 dB.
                let amber_from = meter_pos(0.5);
                let red_from = meter_pos(1.0);
                let seg = |lo: f32, hi: f32| Rect {
                    y: b.bottom() - b.h * hi,
                    h: b.h * (hi - lo),
                    ..*b
                };
                self.scene.push_clip(fill);
                self.scene.rect(seg(0.0, amber_from), 0.0, t.play);
                self.scene.rect(seg(amber_from, red_from), 0.0, t.accent);
                self.scene.rect(seg(red_from, 1.0), 0.0, t.record);
                self.scene.pop_clip();
            }
            if hold[i] > 0.01 {
                let y = b.bottom() - b.h * hold[i];
                let c = if hold[i] >= meter_pos(1.0) {
                    t.record
                } else {
                    t.text_dim
                };
                self.scene.rect(
                    Rect {
                        y: y - 1.0,
                        h: 1.5,
                        ..*b
                    },
                    0.0,
                    c,
                );
            }
        }
    }

    /// A level meter for assistive technology: whole 3 dB steps, reported at
    /// most once a second so a moving meter does not flood the reader.
    fn describe_meter(&mut self, id: Id, r: Rect, peak: f32, now: f64) {
        let db = if peak <= 0.001 {
            -60
        } else {
            ((20.0 * peak.log10() / 3.0).round() as i32 * 3).clamp(-60, 6)
        };
        let st = self.state::<MeterState>(id);
        let speak = match st.spoken_db {
            None => true,
            Some(old) => old != db && now - st.spoken_at >= 1.0,
        };
        if speak {
            st.spoken_db = Some(db);
            st.spoken_at = now;
        }
        let shown = st.spoken_db.unwrap_or(-60);
        self.a11y_node(id, r, 0, || {
            let text = if shown <= -60 {
                "silent".to_string()
            } else {
                format!("{shown} dB")
            };
            Semantics::new(Role::Meter)
                .numeric(f64::from(shown), -60.0, 6.0, 3.0)
                .value(text)
        });
    }

    // ---- scrolling and splitting ----

    /// Handles wheel scrolling over `view` for content of `content` size.
    /// Returns the clamped offset; draws scrollbars.
    pub fn scroll_area(&mut self, id: Id, view: Rect, content: Vec2, offset: &mut Vec2) {
        if self.pointer_in(view) && !self.input.modifiers.command() {
            let mut d = self.input.scroll;
            if self.input.modifiers.shift && d.x == 0.0 {
                d = vec2(d.y, 0.0);
            }
            if d != Vec2::ZERO {
                offset.x -= d.x;
                offset.y -= d.y;
                self.input.scroll = Vec2::ZERO;
            }
        }
        self.take_scroll_requests(id, view, offset);
        let max = vec2((content.x - view.w).max(0.0), (content.y - view.h).max(0.0));
        offset.x = offset.x.clamp(0.0, max.x);
        offset.y = offset.y.clamp(0.0, max.y);
        let (ox, oy) = (f64::from(offset.x), f64::from(offset.y));
        self.a11y_node(id, view, 0, || {
            Semantics::new(Role::ScrollView).scroll(
                (max.x > 0.0).then_some((ox, f64::from(max.x))),
                (max.y > 0.0).then_some((oy, f64::from(max.y))),
            )
        });
        let t = self.theme.clone();
        if max.y > 0.0 {
            let track = Rect {
                x: view.right() - 6.0,
                y: view.y + 2.0,
                w: 4.0,
                h: view.h - 4.0,
            };
            let th = (track.h * view.h / content.y).max(24.0);
            let ty = track.y + (track.h - th) * offset.y / max.y;
            let thumb = Rect {
                y: ty,
                h: th,
                ..track
            };
            let resp = self.interact(id.with("vbar"), track.expand(3.0));
            if resp.dragging {
                offset.y = (offset.y + resp.drag_delta.y * max.y / (track.h - th).max(1.0))
                    .clamp(0.0, max.y);
            }
            let hot = self.animate_bool(
                id.with("vhot"),
                resp.hovered || resp.active || self.pointer_in(view),
            );
            self.scene
                .rect(thumb, 1.0, t.text_faint.alpha(0.25 + hot * 0.35));
        }
        if max.x > 0.0 {
            let track = Rect {
                x: view.x + 2.0,
                y: view.bottom() - 6.0,
                w: view.w - 4.0,
                h: 4.0,
            };
            let tw = (track.w * view.w / content.x).max(24.0);
            let tx = track.x + (track.w - tw) * offset.x / max.x;
            let thumb = Rect {
                x: tx,
                w: tw,
                ..track
            };
            let resp = self.interact(id.with("hbar"), track.expand(3.0));
            if resp.dragging {
                offset.x = (offset.x + resp.drag_delta.x * max.x / (track.w - tw).max(1.0))
                    .clamp(0.0, max.x);
            }
            let hot = self.animate_bool(
                id.with("hhot"),
                resp.hovered || resp.active || self.pointer_in(view),
            );
            self.scene
                .rect(thumb, 1.0, t.text_faint.alpha(0.25 + hot * 0.35));
        }
    }

    /// A standalone scrollbar for views that manage their own scrolling
    /// (timelines). `offset`, `view` and `content` share one unit (beats,
    /// pixels, rows). Drag the thumb, or press the track to jump there and
    /// keep dragging. Returns true when the offset changed.
    pub fn scrollbar(
        &mut self,
        id: Id,
        track: Rect,
        horizontal: bool,
        offset: &mut f32,
        view: f32,
        content: f32,
    ) -> bool {
        let t = self.theme.clone();
        let len = if horizontal { track.w } else { track.h };
        let max = (content - view).max(0.0);
        self.scene.rect(track, 0.0, t.well);
        if max <= 0.0 || len <= 0.0 {
            return false;
        }
        let thumb_len = (len * view / content).clamp(28.0_f32.min(len), len);
        let travel = (len - thumb_len).max(1.0);
        let at = |o: f32| travel * (o / max).clamp(0.0, 1.0);
        let thumb = |o: f32| {
            if horizontal {
                Rect {
                    x: track.x + at(o),
                    w: thumb_len,
                    ..track
                }
            } else {
                Rect {
                    y: track.y + at(o),
                    h: thumb_len,
                    ..track
                }
            }
        };
        let before = *offset;
        let resp = self.interact(id, track.expand(2.0));
        if resp.pressed
            && let Some(p) = self.pointer()
            && !thumb(*offset).contains(p)
        {
            // Jump so the thumb centres under the pointer.
            let along = if horizontal {
                p.x - track.x
            } else {
                p.y - track.y
            };
            *offset = ((along - thumb_len * 0.5) / travel * max).clamp(0.0, max);
        }
        if resp.dragging {
            let d = if horizontal {
                resp.drag_delta.x
            } else {
                resp.drag_delta.y
            };
            *offset = (*offset + d * max / travel).clamp(0.0, max);
        }
        let hot = self.animate_bool(id.with("hot"), resp.hovered || resp.active);
        let r = thumb(*offset);
        self.scene
            .rect(r.shrink(1.0), 1.0, t.text_faint.alpha(0.35 + hot * 0.4));
        (*offset - before).abs() > f32::EPSILON
    }

    /// Scroll requests from assistive technology: by a line or a page, or to
    /// an offset.
    fn take_scroll_requests(&mut self, id: Id, view: Rect, offset: &mut Vec2) {
        use accesskit::{ActionData, ScrollUnit};
        for (action, dx, dy) in [
            (Action::ScrollDown, 0.0, 1.0),
            (Action::ScrollUp, 0.0, -1.0),
            (Action::ScrollRight, 1.0, 0.0),
            (Action::ScrollLeft, -1.0, 0.0),
        ] {
            while let Some(req) = self.take_action(id, action) {
                let page = matches!(req.data, Some(ActionData::ScrollUnit(ScrollUnit::Page)));
                offset.x += dx * if page { view.w * 0.9 } else { 36.0 };
                offset.y += dy * if page { view.h * 0.9 } else { 36.0 };
            }
        }
        if let Some(req) = self.take_action(id, Action::SetScrollOffset)
            && let Some(ActionData::SetScrollOffset(p)) = req.data
        {
            *offset = vec2(p.x as f32, p.y as f32);
        }
    }

    /// A draggable divider. Returns the pointer delta along its axis.
    pub fn splitter(&mut self, id: Id, r: Rect, vertical_bar: bool) -> f32 {
        // The seam may be only a couple of pixels wide; the grab zone
        // reaches into the pane edges so it stays easy to hit.
        let reach = ((SPLITTER_HIT - if vertical_bar { r.w } else { r.h }) * 0.5).max(0.0);
        let hit = if vertical_bar {
            r.shrink2(-reach, 0.0)
        } else {
            r.shrink2(0.0, -reach)
        };
        let resp = self.interact(id, hit);
        self.focusable(id, r, FocusKind::Spin);
        self.a11y_node(id, r, 0, || {
            Semantics::new(Role::Splitter).vertical(vertical_bar)
        });
        // Arrow keys and assistive technology nudge it by a fixed step.
        let adj = self.take_adjust(id);
        let fine = if adj.fine { 0.25 } else { 1.0 };
        let nudge = (adj.steps as f32 + adj.pages as f32 * 4.0) * SPLITTER_STEP * fine;
        let t = self.theme.clone();
        if resp.hovered || resp.active {
            self.set_cursor(if vertical_bar {
                CursorIcon::EwResize
            } else {
                CursorIcon::NsResize
            });
        }
        if resp.hovered || resp.active || resp.focused {
            let line = if vertical_bar {
                Rect {
                    x: r.center().x - 1.0,
                    w: 2.0,
                    ..r
                }
            } else {
                Rect {
                    y: r.center().y - 1.0,
                    h: 2.0,
                    ..r
                }
            };
            self.scene.rect(
                line,
                0.0,
                t.focus.alpha(if resp.active || resp.focused {
                    0.8
                } else {
                    0.4
                }),
            );
        }
        let dragged = if resp.dragging {
            if vertical_bar {
                resp.drag_delta.x
            } else {
                resp.drag_delta.y
            }
        } else {
            0.0
        };
        dragged + nudge
    }

    // ---- menus ----

    /// Shows a menu if `id` is the open popup. Returns the index of the
    /// clicked item.
    pub fn menu(&mut self, id: Id, at: Vec2, items: &[MenuItem]) -> Option<usize> {
        if !self.is_popup_open(id) {
            return None;
        }
        let t = self.theme.clone();
        let style = t.body;
        let row_h = 26.0;
        let mut w: f32 = 160.0;
        let mut h = 8.0;
        for item in items {
            match item {
                MenuItem::Item {
                    label, shortcut, ..
                } => {
                    let lw = self.measure(label, style).x;
                    let sw = shortcut
                        .as_ref()
                        .map_or(0.0, |s| self.measure(s, t.caption).x + 24.0);
                    w = w.max(lw + sw + 48.0);
                    h += row_h;
                }
                MenuItem::Separator => h += 9.0,
                MenuItem::Heading(_) => h += 24.0,
            }
        }
        let screen = self.screen();
        let x = at.x.min(screen.right() - w - 6.0).max(screen.x + 4.0);
        let y = if at.y + h > screen.bottom() - 6.0 {
            (at.y - h).max(4.0)
        } else {
            at.y
        };
        let r = Rect { x, y, w, h };
        self.set_popup_rect(r);
        let mut clicked = None;
        self.with_layer(LayerId::POPUP, |ui| {
            ui.scene.push_clip_absolute(Rect::EVERYTHING);
            ui.interact(id.with("bg"), r);
            // Not `id` itself: the button that opened the menu may use it.
            let node = id.with("a11y-menu");
            ui.a11y_node(node, r, 0, || Semantics::new(Role::Menu).vertical(true));
            ui.a11y_push(node);
            // Opened from the keyboard: start on the first usable row.
            let first = items.iter().enumerate().find_map(|(i, it)| {
                matches!(it, MenuItem::Item { enabled: true, .. }).then(|| id.with(("item", i)))
            });
            ui.popup_take_focus(first);
            ui.scene
                .shadow(r.translate(vec2(0.0, 6.0)), t.radius_l, 14.0, t.shadow);
            ui.scene
                .rect_stroke(r, t.radius_l, t.raised, 1.0, t.line_strong);
            let mut cy = r.y + 4.0;
            for (i, item) in items.iter().enumerate() {
                match item {
                    MenuItem::Item {
                        label,
                        shortcut,
                        enabled,
                        checked,
                        color,
                    } => {
                        let row = Rect {
                            x: r.x + 4.0,
                            y: cy,
                            w: r.w - 8.0,
                            h: row_h,
                        };
                        let item_id = id.with(("item", i));
                        let resp = ui.interact(item_id, row);
                        if *enabled {
                            ui.focusable(item_id, row, FocusKind::Menu);
                        }
                        ui.a11y_node(item_id, row, RANK_TEXT, || {
                            let role = if *checked {
                                Role::MenuItemCheckBox
                            } else {
                                Role::MenuItem
                            };
                            let mut s = Semantics::new(role)
                                .label(label.as_str())
                                .disabled(!*enabled);
                            if *checked {
                                s = s.toggled(true);
                            }
                            if let Some(sc) = shortcut {
                                s = s.shortcut(sc.as_str());
                            }
                            s
                        });
                        if (resp.hovered || resp.focused) && *enabled {
                            ui.scene.rect(row, t.radius_s, t.control_hover);
                        }
                        if resp.hovered && *enabled {
                            ui.set_cursor(CursorIcon::Pointer);
                        }
                        let tc = if *enabled { t.text } else { t.text_faint };
                        let lead = Rect {
                            x: row.x + 4.0,
                            w: 20.0,
                            ..row
                        };
                        if let Some(c) = color {
                            ui.scene
                                .rect(Rect::from_center(lead.center(), 10.0, 10.0), 3.0, *c);
                        } else if *checked {
                            ui.scene.circle(lead.center(), 3.0, t.accent);
                        }
                        ui.text(
                            Rect {
                                x: row.x + 28.0,
                                w: row.w - 32.0,
                                ..row
                            },
                            label,
                            style,
                            tc,
                            Align::Start,
                        );
                        if let Some(s) = shortcut {
                            ui.text(
                                Rect {
                                    w: row.w - 10.0,
                                    ..row
                                },
                                s,
                                t.caption,
                                t.text_faint,
                                Align::End,
                            );
                        }
                        ui.focus_ring(item_id, row, t.radius_s);
                        if resp.clicked && *enabled {
                            clicked = Some(i);
                        }
                        cy += row_h;
                    }
                    MenuItem::Separator => {
                        ui.scene.rect(
                            Rect {
                                x: r.x + 10.0,
                                y: cy + 4.0,
                                w: r.w - 20.0,
                                h: 1.0,
                            },
                            0.0,
                            t.line_strong,
                        );
                        cy += 9.0;
                    }
                    MenuItem::Heading(label) => {
                        let row = Rect {
                            x: r.x + 12.0,
                            y: cy,
                            w: r.w - 24.0,
                            h: 24.0,
                        };
                        ui.a11y_node(id.with(("heading", i)), row, RANK_TEXT, || {
                            Semantics::new(Role::Label).label(label.as_str())
                        });
                        ui.text(row, label, t.caption, t.text_faint, Align::Start);
                        cy += 24.0;
                    }
                }
            }
            ui.a11y_pop(node);
            ui.scene.pop_clip();
        });
        if clicked.is_some() {
            self.close_popup();
        }
        clicked
    }

    /// Draws a floating panel on the popup layer; `f` draws its content.
    pub fn popup_panel<R>(
        &mut self,
        id: Id,
        r: Rect,
        f: impl FnOnce(&mut Ui, Rect) -> R,
    ) -> Option<R> {
        if !self.is_popup_open(id) {
            return None;
        }
        self.set_popup_rect(r);
        let t = self.theme.clone();
        Some(self.with_layer(LayerId::POPUP, |ui| {
            ui.scene.push_clip_absolute(Rect::EVERYTHING);
            ui.interact(id.with("bg"), r);
            ui.scene
                .shadow(r.translate(vec2(0.0, 6.0)), t.radius_l, 14.0, t.shadow);
            ui.scene
                .rect_stroke(r, t.radius_l, t.raised, 1.0, t.line_strong);
            ui.scene.push_clip(r);
            let node = id.with("a11y-panel");
            ui.a11y_node(node, r, 0, || Semantics::new(Role::Dialog));
            ui.a11y_push(node);
            let first = ui.focus_stops.len();
            let out = f(ui, r);
            // Opened from the keyboard: land on the first control inside.
            let take = ui.focus_stops.get(first).map(|s| s.id);
            ui.popup_take_focus(take);
            ui.a11y_pop(node);
            ui.scene.pop_clip();
            ui.scene.pop_clip();
            out
        }))
    }
}
