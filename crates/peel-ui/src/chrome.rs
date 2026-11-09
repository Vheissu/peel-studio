//! Workspace chrome: the quiet controls that frame the editors.
//!
//! Hierarchy rules (see docs/DESIGN.md): one accent-filled primary action
//! per region, secondary buttons on `control`, and everything in toolbars,
//! rails and pane headers quiet — no fill until hovered, a tint when on.
//! Related quiet controls share a recessed [`Ui::well`].

use winit::window::CursorIcon;

use crate::a11y::{Role, Semantics};
use crate::focus::FocusKind;
use crate::geom::{Rect, vec2};
use crate::icons;
use crate::scene::Align;
use crate::ui::{Id, Response, Ui};

/// Which side of a rail faces its panel; the selection bar sits there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
}

impl Ui {
    /// Recessed tray behind a group of related controls.
    pub fn well(&mut self, r: Rect) {
        let t = self.theme.clone();
        self.scene.rect(r, t.radius_m, t.well);
    }

    /// A short vertical rule between control groups, centered in `r`.
    pub fn divider_v(&mut self, r: Rect) {
        let t = self.theme.clone();
        let h = r.h.min(16.0);
        let x = (r.center().x).round() + 0.5;
        let y = r.center().y - h * 0.5;
        self.scene.vline(x, y, y + h, t.line_strong);
    }

    /// Text button without a fill until hovered: toolbar entries such as
    /// View. `icon` leads the label; `caret` marks a menu. `on` shows the
    /// open state. Returns the response; the caller opens the menu.
    pub fn quiet_button(
        &mut self,
        id: Id,
        r: Rect,
        icon: Option<&str>,
        label: &str,
        caret: bool,
        on: bool,
    ) -> Response {
        let mut sem = Semantics::new(Role::Button).label(label);
        if caret {
            sem = sem.expanded(on);
        }
        let resp = self.interact_node(id, r, FocusKind::Activate, sem);
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        if on || resp.active {
            self.scene.rect(r, t.radius_m, t.control);
        } else if hot > 0.0 {
            self.scene.rect(r, t.radius_m, t.control.alpha(hot));
        }
        let color = if on {
            t.text
        } else {
            t.text_dim.mix(t.text, hot)
        };
        let mut inner = r.shrink2(8.0, 0.0);
        if let Some(icon) = icon {
            self.icon(inner.cut_left(16.0), icon, 15.0, color);
            inner.cut_left(6.0);
        }
        if caret {
            self.icon(inner.cut_right(12.0), icons::CARET_DOWN, 11.0, t.text_faint);
            inner.cut_right(4.0);
        }
        let shown = self.fit_text(label, t.label, inner.w);
        self.text(inner, &shown, t.label, color, Align::Start);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.focus_ring(id, r, t.radius_m);
        resp
    }

    /// Natural width of a [`Ui::quiet_button`].
    pub fn quiet_button_width(&mut self, icon: bool, label: &str, caret: bool) -> f32 {
        let t = self.theme.clone();
        16.0 + self.measure(label, t.label).x
            + if icon { 22.0 } else { 0.0 }
            + if caret { 16.0 } else { 0.0 }
    }

    /// A pane tab: icon and (when `label` is shown) its name, underlined in
    /// the accent when selected. Widths never change with the selection.
    pub fn tab(
        &mut self,
        id: Id,
        r: Rect,
        icon: &str,
        label: &str,
        show_label: bool,
        on: bool,
    ) -> Response {
        let resp = self.interact_node(
            id,
            r,
            FocusKind::Activate,
            Semantics::new(Role::Tab).label(label).selected(on),
        );
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        if hot > 0.0 && !on {
            self.scene
                .rect(r.shrink2(0.0, 3.0), t.radius_m, t.control.alpha(hot * 0.6));
        }
        let color = if on {
            t.text
        } else {
            t.text_dim.mix(t.text, hot)
        };
        let icon_color = if on { t.accent } else { color };
        if show_label {
            let mut inner = r.shrink2(10.0, 0.0);
            self.icon(inner.cut_left(16.0), icon, 15.0, icon_color);
            inner.cut_left(7.0);
            self.text(inner, label, t.label, color, Align::Start);
        } else {
            self.icon(r, icon, 15.0, icon_color);
        }
        if on {
            let bar = Rect {
                x: r.x + 8.0,
                y: r.bottom() - 2.0,
                w: r.w - 16.0,
                h: 2.0,
            };
            self.scene.rect(bar, 1.0, t.accent);
        }
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.focus_ring(id, r, t.radius_m);
        resp
    }

    /// Natural width of a labelled [`Ui::tab`].
    pub fn tab_width(&mut self, label: &str) -> f32 {
        let t = self.theme.clone();
        self.measure(label, t.label).x + 43.0
    }

    /// A navigation rail entry: an icon with a selection bar on the edge
    /// that faces the panel it controls. `label` names it for tooltips and
    /// screen readers; `on` is the selected (panel showing) state.
    pub fn rail_item(
        &mut self,
        id: Id,
        r: Rect,
        icon: &str,
        label: &str,
        on: bool,
        edge: Edge,
    ) -> Response {
        let resp = self.interact_node(
            id,
            r,
            FocusKind::Activate,
            Semantics::new(Role::Tab).label(label).selected(on),
        );
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        let cell = r.shrink(3.0);
        if on {
            self.scene.rect(cell, t.radius_m, t.panel);
        } else if hot > 0.0 {
            self.scene.rect(cell, t.radius_m, t.panel.alpha(hot * 0.7));
        }
        if on {
            let x = match edge {
                Edge::Left => r.x,
                Edge::Right => r.right() - 2.0,
            };
            let bar = Rect {
                x,
                y: cell.y + 6.0,
                w: 2.0,
                h: cell.h - 12.0,
            };
            self.scene.rect(bar, 1.0, t.accent);
        }
        let color = if on {
            t.text
        } else {
            t.text_faint.mix(t.text, hot * 0.7)
        };
        self.icon(r, icon, 17.0, color);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.tooltip(&resp, label);
        self.focus_ring(id, cell, t.radius_m);
        resp
    }

    /// Small header label with an optional count or detail on the right,
    /// used for property groups: a rule above and the name below.
    pub fn group_heading(&mut self, r: Rect, text: &str, first: bool) {
        let t = self.theme.clone();
        if !first {
            self.scene.hline(r.x, r.right(), r.y + 0.5, t.line);
        }
        let tr = Rect {
            y: r.y + if first { 0.0 } else { 6.0 },
            h: r.h - if first { 0.0 } else { 6.0 },
            ..r
        };
        self.text(tr, text, t.label, t.text_dim, Align::Start);
    }

    /// A dropdown value cell for a property row: `control` fill, the value
    /// left-aligned and a caret on the right. Returns the response; the
    /// caller opens its menu below `r`.
    pub fn dropdown(&mut self, id: Id, r: Rect, value: &str, name: &str) -> Response {
        let resp = self.interact_node(
            id,
            r,
            FocusKind::Activate,
            Semantics::new(Role::ComboBox).label(name).value(value),
        );
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        self.scene
            .rect(r, t.radius_m, t.control.mix(t.control_hover, hot));
        let mut inner = r.shrink2(8.0, 0.0);
        self.icon(inner.cut_right(12.0), icons::CARET_DOWN, 11.0, t.text_dim);
        inner.cut_right(4.0);
        let shown = self.fit_text(value, t.label, inner.w);
        self.text(inner, &shown, t.label, t.text, Align::Start);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.focus_ring(id, r, t.radius_m);
        resp
    }

    /// A tool in a [`Ui::well`]: quiet when off, the segmented selection
    /// fill when on (one of a set, like the editing tools). `name` is the
    /// tooltip and accessible name.
    pub fn tool_button(&mut self, id: Id, r: Rect, icon: &str, name: &str, on: bool) -> Response {
        let resp = self.interact_node(
            id,
            r,
            FocusKind::Activate,
            Semantics::new(Role::RadioButton).label(name).toggled(on),
        );
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        let cell = r.shrink(2.0);
        if on {
            self.scene.rect(cell, t.radius_s, t.control_hover);
        } else if hot > 0.0 || resp.active {
            let a = if resp.active { 1.0 } else { hot * 0.7 };
            self.scene.rect(cell, t.radius_s, t.control.alpha(a));
        }
        let color = if on {
            t.text
        } else {
            t.text_dim.mix(t.text, hot)
        };
        self.icon(r, icon, 15.0, color);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.tooltip(&resp, name);
        self.focus_ring(id, cell, t.radius_s);
        resp
    }

    /// A quiet menu button showing a value with a leading icon (grid snap,
    /// scale, chord). `on` tints the icon in the accent when the setting is
    /// active.
    pub fn value_button(
        &mut self,
        id: Id,
        r: Rect,
        icon: &str,
        value: &str,
        name: &str,
        on: bool,
    ) -> Response {
        let resp = self.interact_node(
            id,
            r,
            FocusKind::Activate,
            Semantics::new(Role::Button).label(name).value(value),
        );
        let t = self.theme.clone();
        let hot = self.animate_bool(id, resp.hovered);
        if hot > 0.0 || resp.active {
            let a = if resp.active { 1.0 } else { hot };
            self.scene.rect(r, t.radius_s, t.control.alpha(a));
        }
        let mut inner = r.shrink2(6.0, 0.0);
        self.icon(
            inner.cut_left(16.0),
            icon,
            14.0,
            if on { t.accent } else { t.text_dim },
        );
        inner.cut_left(4.0);
        let shown = self.fit_text(value, t.label, inner.w);
        let color = if on {
            t.text
        } else {
            t.text_dim.mix(t.text, 0.4 + hot * 0.6)
        };
        self.text(inner, &shown, t.label, color, Align::Start);
        if resp.hovered {
            self.set_cursor(CursorIcon::Pointer);
        }
        self.tooltip(&resp, name);
        self.focus_ring(id, r, t.radius_s);
        resp
    }

    /// Where a menu opened from `r` should appear.
    pub fn below(r: Rect) -> crate::geom::Vec2 {
        vec2(r.x, r.bottom() + 4.0)
    }
}
