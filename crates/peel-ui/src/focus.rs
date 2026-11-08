//! Keyboard focus and traversal.
//!
//! Widgets that can be operated without a pointer register a *focus stop*
//! each frame with [`Ui::focusable`]. Tab and Shift+Tab walk the stops of the
//! previous frame in registration order, and a widget that holds keyboard
//! focus draws a focus ring (the theme's `focus` color, 2 px).
//!
//! Keyboard focus is separate from text focus ([`Ui::has_focus`]), which
//! belongs to a text field that is being edited. It is only ever set by
//! Tab navigation, an assistive-technology *Focus* action or
//! [`Ui::request_kb_focus`], and any pointer press clears it. While nothing
//! holds keyboard focus the toolkit consumes no keys except Tab itself, so
//! global shortcuts (Space for play and stop, letters for the on-screen
//! piano) keep working.
//!
//! Which keys a focused widget claims depends on its [`FocusKind`]. They are
//! claimed at the start of the frame, before the application's own shortcut
//! handling runs, so the focused widget wins.

use accesskit::{Action, ActionData, ActionRequest};
use winit::keyboard::{Key, NamedKey};

use crate::a11y::{ROOT, node_id, widget_id};
use crate::geom::Rect;
use crate::scene::LayerId;
use crate::ui::{Id, Ui};

/// What a focus stop does with the keyboard, which decides the keys it
/// claims while it holds focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusKind {
    /// A button or switch. Space and Enter click it.
    Activate,
    /// A knob, fader or slider. Arrow keys step the value, Page Up and
    /// Page Down take larger steps, Home and End jump to the ends,
    /// Backspace and Delete reset it, and Shift makes steps ten times finer.
    Adjust,
    /// A number field. Like [`FocusKind::Adjust`] without reset; Enter starts
    /// typing a value.
    Spin,
    /// One option of a radio-style group. Arrow keys move the selection and
    /// Space or Enter selects.
    Choice,
    /// A menu row. Up, Down, Home and End move between rows and Space or
    /// Enter picks.
    Menu,
    /// A text field. Tab starts editing it and Enter starts editing again
    /// after a commit.
    Text,
    /// Takes focus and draws a ring but claims no keys. Custom widgets read
    /// [`Ui::input`] themselves when [`Ui::is_kb_focused`] is true.
    Custom,
}

impl FocusKind {
    /// True if a click or activation key on this kind is delivered through
    /// [`Response::clicked`](crate::Response::clicked) by `interact`.
    fn clicks_in_interact(self) -> bool {
        !matches!(self, FocusKind::Spin | FocusKind::Text)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FocusStop {
    pub id: Id,
    pub layer: LayerId,
    pub kind: FocusKind,
}

/// Value changes aimed at one widget this frame, from the keyboard while it
/// holds focus or from an assistive technology.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Adjust {
    /// Single steps; positive increases.
    pub steps: i32,
    /// Larger steps (Page Up and Page Down); positive increases.
    pub pages: i32,
    pub home: bool,
    pub end: bool,
    /// Reset to the default value.
    pub reset: bool,
    /// Shift was held: steps are ten times smaller.
    pub fine: bool,
    /// An assistive technology set the value directly, in the units the
    /// node reports (see the widget's documentation).
    pub set: Option<f64>,
}

impl Adjust {
    pub fn any(&self) -> bool {
        self.steps != 0
            || self.pages != 0
            || self.home
            || self.end
            || self.reset
            || self.set.is_some()
    }

    /// Applies the changes to `value`, clamped to `min..=max`. A page is ten
    /// steps. `set_scale` converts a directly set value into `value`'s units
    /// (for example `100.0` when the node reports percent for a 0..=1 value).
    pub fn apply(
        &self,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        default: Option<f64>,
        set_scale: f64,
    ) -> f64 {
        let mut v = value;
        if let Some(s) = self.set {
            v = s / set_scale;
        }
        if self.reset
            && let Some(d) = default
        {
            v = d;
        }
        if self.home {
            v = min;
        }
        if self.end {
            v = max;
        }
        let fine = if self.fine { 0.1 } else { 1.0 };
        v += f64::from(self.steps) * step * fine + f64::from(self.pages) * step * 10.0;
        v.clamp(min, max)
    }
}

enum KbAction {
    Activate,
    Step(i32),
    Page(i32),
    Home,
    End,
    Reset,
    Nav(i32),
    NavFirst,
    NavLast,
}

fn is_space(key: &Key) -> bool {
    match key {
        Key::Named(NamedKey::Space) => true,
        Key::Character(c) => c.as_str() == " ",
        _ => false,
    }
}

/// The action a key performs on a widget of `kind`, if the kind claims it.
fn reserved(kind: FocusKind, key: &Key, repeat: bool) -> Option<KbAction> {
    use NamedKey::*;
    let named = match key {
        Key::Named(n) => Some(*n),
        _ => None,
    };
    let enter = named == Some(Enter);
    let activate = !repeat && (enter || is_space(key));
    let adjust = |allow_reset: bool| match named {
        Some(ArrowRight | ArrowUp) => Some(KbAction::Step(1)),
        Some(ArrowLeft | ArrowDown) => Some(KbAction::Step(-1)),
        Some(PageUp) => Some(KbAction::Page(1)),
        Some(PageDown) => Some(KbAction::Page(-1)),
        Some(Home) => Some(KbAction::Home),
        Some(End) => Some(KbAction::End),
        Some(Backspace | Delete) if allow_reset => Some(KbAction::Reset),
        _ => None,
    };
    match kind {
        FocusKind::Activate => activate.then_some(KbAction::Activate),
        FocusKind::Choice => {
            if activate {
                Some(KbAction::Activate)
            } else {
                match named {
                    Some(ArrowLeft | ArrowUp) => Some(KbAction::Step(-1)),
                    Some(ArrowRight | ArrowDown) => Some(KbAction::Step(1)),
                    _ => None,
                }
            }
        }
        FocusKind::Adjust => adjust(true),
        FocusKind::Spin => {
            if enter && !repeat {
                Some(KbAction::Activate)
            } else {
                adjust(false)
            }
        }
        FocusKind::Menu => {
            if activate {
                Some(KbAction::Activate)
            } else {
                match named {
                    Some(ArrowDown) => Some(KbAction::Nav(1)),
                    Some(ArrowUp) => Some(KbAction::Nav(-1)),
                    Some(Home) => Some(KbAction::NavFirst),
                    Some(End) => Some(KbAction::NavLast),
                    _ => None,
                }
            }
        }
        FocusKind::Text => (enter && !repeat).then_some(KbAction::Activate),
        FocusKind::Custom => None,
    }
}

impl Ui {
    // ---- registration ----

    /// Registers `id` as a keyboard focus stop for this frame, in call
    /// order. Standard widgets do this themselves; call it for custom
    /// widgets (and draw the ring with [`Ui::focus_ring`]). Stops that are
    /// clipped away entirely are skipped, so scrolled-off widgets are not
    /// reachable with Tab.
    pub fn focusable(&mut self, id: Id, rect: Rect, kind: FocusKind) {
        if rect.intersect(&self.scene.clip()).is_empty() {
            return;
        }
        self.focus_stops.push(FocusStop {
            id,
            layer: self.scene.layer_id(),
            kind,
        });
    }

    /// Draws the keyboard focus ring (2 px, theme `focus` color) around
    /// `rect` when `id` holds keyboard focus. Nothing is drawn for mouse
    /// use.
    pub fn focus_ring(&mut self, id: Id, rect: Rect, radius: f32) {
        if self.kb_focus == Some(id) {
            let c = self.theme.focus;
            self.scene.outline(rect.expand(1.0), radius + 1.0, 2.0, c);
        }
    }

    // ---- state ----

    /// The widget that holds keyboard focus (set by Tab, an assistive
    /// technology or [`Ui::request_kb_focus`]).
    pub fn kb_focus(&self) -> Option<Id> {
        self.kb_focus
    }

    pub fn is_kb_focused(&self, id: Id) -> bool {
        self.kb_focus == Some(id)
    }

    /// Moves keyboard focus to `id`, for example into a dialog that just
    /// opened. A text field takes it by starting to edit.
    pub fn request_kb_focus(&mut self, id: Id) {
        self.set_kb_focus(Some(id));
    }

    pub fn clear_kb_focus(&mut self) {
        self.set_kb_focus(None);
    }

    pub(crate) fn set_kb_focus(&mut self, id: Option<Id>) {
        if self.kb_focus != id {
            self.kb_focus = id;
            self.kb_entered = id;
            self.request_repaint();
        }
    }

    /// True once after keyboard focus moved to `id`; text fields use it to
    /// start editing.
    pub(crate) fn take_kb_entered(&mut self, id: Id) -> bool {
        if self.kb_entered == Some(id) {
            self.kb_entered = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn kind_of(&self, id: Id) -> Option<FocusKind> {
        self.prev_focus_stops
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.kind)
    }

    /// The ids Tab can reach, in order. While a popup is open only the
    /// popup's own stops are reachable.
    fn focus_candidates(&self) -> Vec<Id> {
        let trap = self.popup.is_some()
            && self
                .prev_focus_stops
                .iter()
                .any(|s| s.layer >= LayerId::POPUP);
        let mut out: Vec<Id> = Vec::new();
        for s in &self.prev_focus_stops {
            if (!trap || s.layer >= LayerId::POPUP) && !out.contains(&s.id) {
                out.push(s.id);
            }
        }
        out
    }

    /// Moves keyboard focus by `delta` stops, wrapping around.
    pub(crate) fn kb_move(&mut self, delta: i32) {
        let ids = self.focus_candidates();
        if ids.is_empty() {
            return;
        }
        let n = ids.len() as i32;
        let pos = self
            .kb_focus
            .and_then(|f| ids.iter().position(|i| *i == f));
        let next = match pos {
            Some(p) => (p as i32 + delta).rem_euclid(n),
            None if delta > 0 => 0,
            None => n - 1,
        };
        self.set_kb_focus(Some(ids[next as usize]));
    }

    fn kb_move_to_end(&mut self, last: bool) {
        let ids = self.focus_candidates();
        let pick = if last { ids.last() } else { ids.first() };
        if let Some(id) = pick.copied() {
            self.set_kb_focus(Some(id));
        }
    }

    // ---- per-frame processing ----

    /// Resolves keyboard focus input for this frame. Runs at the end of
    /// `begin_frame`, before the application sees the keys.
    pub(crate) fn process_keyboard(&mut self) {
        self.kb_target = None;
        self.kb_adjust = Adjust::default();
        self.kb_activate = false;
        if self.input.pressed.iter().any(|p| *p) {
            self.kb_focus = None;
            self.kb_entered = None;
        }
        // A text field being edited owns the keyboard.
        if self.focus.is_none() {
            for i in 0..self.input.keys.len() {
                self.process_key(i);
            }
        }
        if !self.a11y.actions.is_empty() {
            self.process_focus_actions();
        }
    }

    fn process_key(&mut self, i: usize) {
        let k = &self.input.keys[i];
        let m = k.modifiers;
        if k.consumed || m.ctrl || m.alt || m.logo {
            return;
        }
        let key = k.key.clone();
        let repeat = k.repeat;
        let shift = m.shift;
        match key {
            Key::Named(NamedKey::Tab) => {
                self.input.keys[i].consumed = true;
                self.kb_move(if shift { -1 } else { 1 });
            }
            Key::Named(NamedKey::Escape) => {
                // With a popup open, Escape closes the popup instead.
                if self.kb_focus.is_some() && self.popup.is_none() {
                    self.input.keys[i].consumed = true;
                    self.set_kb_focus(None);
                }
            }
            _ => {
                let Some(f) = self.kb_focus else { return };
                let Some(kind) = self.kind_of(f) else { return };
                let Some(action) = reserved(kind, &key, repeat) else {
                    return;
                };
                self.input.keys[i].consumed = true;
                if self.kb_target != Some(f) {
                    self.kb_target = Some(f);
                    self.kb_adjust = Adjust::default();
                    self.kb_activate = false;
                }
                let adj = &mut self.kb_adjust;
                adj.fine |= shift;
                match action {
                    KbAction::Activate => self.kb_activate = true,
                    KbAction::Step(d) => adj.steps += d,
                    KbAction::Page(d) => adj.pages += d,
                    KbAction::Home => adj.home = true,
                    KbAction::End => adj.end = true,
                    KbAction::Reset => adj.reset = true,
                    KbAction::Nav(d) => self.kb_move(d),
                    KbAction::NavFirst => self.kb_move_to_end(false),
                    KbAction::NavLast => self.kb_move_to_end(true),
                }
            }
        }
    }

    /// Focus and blur requests from assistive technology are handled
    /// immediately; everything else waits for the target widget.
    fn process_focus_actions(&mut self) {
        let mut i = 0;
        while i < self.a11y.actions.len() {
            let (action, target) = {
                let a = &self.a11y.actions[i];
                (a.action, a.target_node)
            };
            match action {
                Action::Focus => {
                    self.a11y.actions.remove(i);
                    if target == ROOT {
                        self.set_kb_focus(None);
                    } else {
                        self.set_kb_focus(Some(widget_id(target)));
                    }
                }
                Action::Blur => {
                    self.a11y.actions.remove(i);
                    if target != ROOT && self.kb_focus == Some(widget_id(target)) {
                        self.set_kb_focus(None);
                    }
                }
                _ => i += 1,
            }
        }
    }

    /// Drops keyboard focus that points at a widget which was not drawn.
    pub(crate) fn finish_keyboard(&mut self) {
        if let Some(f) = self.kb_focus {
            let alive =
                self.focus_stops.iter().any(|s| s.id == f) || self.a11y.index.contains_key(&f);
            if !alive {
                self.kb_focus = None;
                self.kb_entered = None;
            }
        }
        self.a11y.actions.clear();
    }

    // ---- consumption by widgets ----

    /// Consumes an activation (Space or Enter on the focused widget) aimed
    /// at `id`, or an assistive-technology click on a text or number field.
    pub(crate) fn take_activate(&mut self, id: Id) -> bool {
        let mut hit = false;
        if self.kb_target == Some(id) && self.kb_activate {
            self.kb_activate = false;
            hit = true;
        }
        hit | self.take_action(id, Action::Click).is_some()
    }

    /// Consumes the keyboard and assistive-technology value changes aimed at
    /// `id`: arrow keys, paging, Home and End, reset and the *Increment*,
    /// *Decrement* and *SetValue* actions. Standard value widgets call this;
    /// custom ones can too.
    pub fn take_adjust(&mut self, id: Id) -> Adjust {
        let mut out = Adjust::default();
        if self.kb_target == Some(id) {
            out = std::mem::take(&mut self.kb_adjust);
        }
        let mut i = 0;
        while i < self.a11y.actions.len() {
            let a = &self.a11y.actions[i];
            if widget_id(a.target_node) != id {
                i += 1;
                continue;
            }
            match a.action {
                Action::Increment => out.steps += 1,
                Action::Decrement => out.steps -= 1,
                Action::SetValue => match &a.data {
                    Some(ActionData::NumericValue(v)) => out.set = Some(*v),
                    Some(ActionData::Value(s)) => {
                        if let Ok(v) = s.trim().trim_end_matches('%').trim().parse::<f64>() {
                            out.set = Some(v);
                        }
                    }
                    _ => {}
                },
                _ => {
                    i += 1;
                    continue;
                }
            }
            self.a11y.actions.remove(i);
        }
        out
    }

    /// Consumes the first pending assistive-technology `action` aimed at
    /// `id`.
    pub fn take_action(&mut self, id: Id, action: Action) -> Option<ActionRequest> {
        if self.a11y.actions.is_empty() {
            return None;
        }
        let nid = node_id(id);
        let pos = self
            .a11y
            .actions
            .iter()
            .position(|a| a.target_node == nid && a.action == action)?;
        Some(self.a11y.actions.remove(pos))
    }

    /// What `interact` does on top of the pointer: keyboard activation of
    /// the focused widget and the assistive-technology *Click* and *Show
    /// context menu* actions.
    pub(crate) fn interact_extras(&mut self, id: Id, r: &mut crate::ui::Response) {
        r.focused = self.kb_focus == Some(id);
        let has_actions = !self.a11y.actions.is_empty();
        if !has_actions && self.kb_target != Some(id) {
            return;
        }
        let clicks = self.kind_of(id).is_none_or(FocusKind::clicks_in_interact);
        if clicks {
            if self.kb_target == Some(id) && self.kb_activate {
                self.kb_activate = false;
                r.clicked = true;
            }
            if has_actions && self.take_action(id, Action::Click).is_some() {
                r.clicked = true;
            }
        }
        if has_actions && self.take_action(id, Action::ShowContextMenu).is_some() {
            r.secondary_clicked = true;
        }
    }
}
