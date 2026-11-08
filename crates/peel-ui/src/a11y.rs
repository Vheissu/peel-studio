//! Screen-reader accessibility through [AccessKit](https://accesskit.dev).
//!
//! Everything in `peel-ui` is drawn, so without help a screen reader sees an
//! empty window. Each frame the widgets also describe themselves (role, name,
//! value, range, toggled state, bounds) and, when an assistive technology is
//! connected, [`Ui`] turns those descriptions into an AccessKit tree that the
//! window runner hands to the platform adapter (VoiceOver on macOS, UI
//! Automation on Windows, AT-SPI on Linux).
//!
//! # Cost when unused
//!
//! The tree is only built while an assistive technology is connected
//! ([`Ui::accessibility_active`]). Otherwise every `describe` call returns
//! immediately without allocating. Keyboard focus ([`Ui::focusable`]) works
//! either way.
//!
//! # Names
//!
//! Standard widgets name themselves where they can: a [`Ui::button`] uses its
//! label, an icon from [`icons`](crate::icons) uses its plain-language name,
//! a segmented control uses its options, and a menu item its label. When a
//! widget has no text, the text of [`Ui::tooltip`] called for that response
//! becomes its name (and a tooltip on a widget that already has a name
//! becomes its description). To name a widget yourself, which always wins:
//!
//! ```ignore
//! let cutoff = ui.knob(id, r, &mut norm, 0.5, color, false);
//! ui.name(&cutoff, "Cutoff");
//! ui.value_text(&cutoff, "2.4 kHz");
//! // or, right after drawing it:
//! ui.label_last("Cutoff");
//! ```
//!
//! Knobs, faders and sliders report a 0 to 100 range (percent). Give them a
//! real-world value string with [`Ui::value_text`], which screen readers
//! prefer over the number.
//!
//! # Custom widgets
//!
//! Custom canvases (a piano roll, a playlist, a step grid) describe
//! themselves with [`Ui::describe`] or the fuller [`Ui::describe_node`], and
//! nest children with [`Ui::describe_group`]. Only describe what is
//! on screen; the tree is rebuilt every frame, so virtualized lists stay
//! small.
//!
//! ```ignore
//! // A grid and the notes currently visible in it.
//! let grid = Id::new("roll");
//! ui.describe_group(
//!     grid,
//!     Semantics::new(Role::Group).label("Piano roll").rect(roll_rect),
//!     |ui| {
//!         for note in visible_notes {
//!             ui.describe_node(
//!                 grid.with(note.id),
//!                 Semantics::new(Role::ListItem)
//!                     .label(format!("{} at beat {}", note.name, note.start))
//!                     .selected(note.selected)
//!                     .rect(note.rect)
//!                     .focusable(FocusKind::Custom),
//!             );
//!         }
//!     },
//! );
//! ```
//!
//! # Actions
//!
//! Requests from the assistive technology arrive as AccessKit
//! [`ActionRequest`]s. The runner queues them with
//! [`Ui::queue_accessibility_action`]; the next frame handles them:
//!
//! * *Focus* and *Blur* move keyboard focus.
//! * *Click* becomes [`Response::clicked`](crate::Response::clicked) of the
//!   target's `interact` call.
//! * *Increment*, *Decrement* and *SetValue* reach value widgets through
//!   [`Ui::take_adjust`]; text fields take *SetValue* and
//!   *ReplaceSelectedText*.
//! * Anything else is available to custom widgets through
//!   [`Ui::take_action`].
//!
//! Requests nothing consumes during the frame are dropped.

use std::collections::{HashMap, HashSet};

use accesskit::{Live, Node, NodeId, Orientation, Toggled, TreeId, TreeInfo};

pub use accesskit::{Action, ActionData, ActionRequest, Role, TreeUpdate};

use crate::focus::{Adjust, FocusKind};
use crate::geom::Rect;
use crate::scene::LayerId;
use crate::ui::{Id, Response, Ui};

/// The tree root (the window).
pub(crate) const ROOT: NodeId = NodeId(0);

/// Maps a widget [`Id`] to its stable AccessKit node id.
pub(crate) fn node_id(id: Id) -> NodeId {
    NodeId(if id.0 == 0 { u64::MAX } else { id.0 })
}

pub(crate) fn widget_id(n: NodeId) -> Id {
    Id(if n.0 == u64::MAX { 0 } else { n.0 })
}

// Where a name came from; a stronger source replaces a weaker one.
pub(crate) const RANK_ICON: u8 = 1;
pub(crate) const RANK_TOOLTIP: u8 = 2;
pub(crate) const RANK_TEXT: u8 = 3;
pub(crate) const RANK_EXPLICIT: u8 = 4;

/// A numeric value with its range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Numeric {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// One *Increment* or *Decrement*.
    pub step: f64,
}

/// What a widget tells assistive technology about itself.
///
/// Build one with [`Semantics::new`] and the chained setters, then pass it to
/// [`Ui::describe_node`]. Fields you leave out stay as they were, so
/// [`Semantics::patch`] adjusts a widget that already described itself.
#[derive(Clone, Debug, Default)]
pub struct Semantics {
    pub(crate) role: Option<Role>,
    pub(crate) label: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) placeholder: Option<String>,
    pub(crate) shortcut: Option<String>,
    pub(crate) numeric: Option<Numeric>,
    pub(crate) toggled: Option<bool>,
    pub(crate) selected: Option<bool>,
    pub(crate) expanded: Option<bool>,
    pub(crate) disabled: Option<bool>,
    pub(crate) focusable: Option<FocusKind>,
    pub(crate) actions: Vec<Action>,
    pub(crate) rect: Option<Rect>,
    pub(crate) vertical: Option<bool>,
    /// Scroll position and its maximum, per axis.
    pub(crate) scroll_x: Option<(f64, f64)>,
    pub(crate) scroll_y: Option<(f64, f64)>,
    pub(crate) live: bool,
}

/// Actions a role supports without being asked.
fn default_actions(role: Role) -> &'static [Action] {
    match role {
        Role::Button
        | Role::DefaultButton
        | Role::CheckBox
        | Role::Switch
        | Role::RadioButton
        | Role::MenuItem
        | Role::MenuItemCheckBox
        | Role::MenuItemRadio
        | Role::Tab
        | Role::Link
        | Role::DisclosureTriangle => &[Action::Click],
        Role::Slider | Role::SpinButton | Role::Splitter => {
            &[Action::Increment, Action::Decrement, Action::SetValue]
        }
        Role::TextInput | Role::SearchInput | Role::MultilineTextInput | Role::NumberInput => {
            &[Action::SetValue, Action::ReplaceSelectedText]
        }
        _ => &[],
    }
}

impl Semantics {
    /// A new description. The role brings its usual actions: buttons accept
    /// *Click*; sliders, spin buttons and splitters accept *Increment*,
    /// *Decrement* and *SetValue* (answer them with [`Ui::take_adjust`]);
    /// text inputs accept *SetValue* and *ReplaceSelectedText*. See
    /// [`Semantics::no_actions`].
    pub fn new(role: Role) -> Semantics {
        Semantics {
            role: Some(role),
            actions: default_actions(role).to_vec(),
            ..Default::default()
        }
    }

    /// A description with no role of its own, for adjusting a widget that
    /// already described itself this frame.
    pub fn patch() -> Semantics {
        Semantics::default()
    }

    pub fn label(mut self, s: impl Into<String>) -> Semantics {
        self.label = Some(s.into());
        self
    }

    /// The value as text ("2.4 kHz", "C#4"). Screen readers prefer it over
    /// the number from [`Semantics::numeric`].
    pub fn value(mut self, s: impl Into<String>) -> Semantics {
        self.value = Some(s.into());
        self
    }

    pub fn description(mut self, s: impl Into<String>) -> Semantics {
        self.description = Some(s.into());
        self
    }

    pub fn placeholder(mut self, s: impl Into<String>) -> Semantics {
        self.placeholder = Some(s.into());
        self
    }

    /// A keyboard shortcut to announce, such as "Space".
    pub fn shortcut(mut self, s: impl Into<String>) -> Semantics {
        self.shortcut = Some(s.into());
        self
    }

    /// A numeric value, its range and the size of one step.
    pub fn numeric(mut self, value: f64, min: f64, max: f64, step: f64) -> Semantics {
        self.numeric = Some(Numeric {
            value,
            min,
            max,
            step,
        });
        self
    }

    /// On or off, for buttons, switches, check boxes and radio buttons.
    pub fn toggled(mut self, on: bool) -> Semantics {
        self.toggled = Some(on);
        self
    }

    pub fn selected(mut self, on: bool) -> Semantics {
        self.selected = Some(on);
        self
    }

    pub fn expanded(mut self, on: bool) -> Semantics {
        self.expanded = Some(on);
        self
    }

    pub fn disabled(mut self, on: bool) -> Semantics {
        self.disabled = Some(on);
        self
    }

    /// Screen position. Defaults to where the widget's `interact` region is.
    pub fn rect(mut self, r: Rect) -> Semantics {
        self.rect = Some(r);
        self
    }

    /// Makes the node a keyboard focus stop (see [`Ui::focusable`]).
    pub fn focusable(mut self, kind: FocusKind) -> Semantics {
        self.focusable = Some(kind);
        self
    }

    /// Advertises one more action.
    pub fn action(mut self, a: Action) -> Semantics {
        if !self.actions.contains(&a) {
            self.actions.push(a);
        }
        self
    }

    /// Drops the role's default actions.
    pub fn no_actions(mut self) -> Semantics {
        self.actions.clear();
        self
    }

    /// Direction of a slider, scroll bar or splitter.
    pub fn vertical(mut self, v: bool) -> Semantics {
        self.vertical = Some(v);
        self
    }

    /// Scroll position (`offset`) and extent (`max` offset) of a scrolling
    /// view, per axis; an axis with nothing to scroll is `None`. Advertises
    /// the scroll actions for the axes that can move.
    pub fn scroll(mut self, x: Option<(f64, f64)>, y: Option<(f64, f64)>) -> Semantics {
        self.scroll_x = x;
        self.scroll_y = y;
        if x.is_some() || y.is_some() {
            self = self.action(Action::SetScrollOffset);
        }
        if x.is_some() {
            self = self.action(Action::ScrollLeft).action(Action::ScrollRight);
        }
        if y.is_some() {
            self = self.action(Action::ScrollUp).action(Action::ScrollDown);
        }
        self
    }

    /// A polite live region: changes to its label are announced.
    pub fn live(mut self) -> Semantics {
        self.live = true;
        self
    }

    /// Overlays the fields `o` sets. `label` is handled by the caller (it
    /// has a rank).
    fn merge(&mut self, o: Semantics) {
        macro_rules! take {
            ($($f:ident),*) => {$( if o.$f.is_some() { self.$f = o.$f; } )*};
        }
        take!(
            role,
            value,
            description,
            placeholder,
            shortcut,
            numeric,
            toggled,
            selected,
            expanded,
            disabled,
            focusable,
            rect,
            vertical,
            scroll_x,
            scroll_y
        );
        for a in o.actions {
            if !self.actions.contains(&a) {
                self.actions.push(a);
            }
        }
        self.live |= o.live;
    }

    fn to_node(&self, bounds: Option<Rect>, scale: f64, children: &[NodeId], focus: bool) -> Node {
        let role = self.role.unwrap_or(Role::GenericContainer);
        let mut n = Node::new(role);
        if let Some(v) = &self.label {
            n.set_label(v.as_str());
        }
        if let Some(v) = &self.value {
            n.set_value(v.as_str());
        }
        if let Some(v) = &self.description {
            n.set_description(v.as_str());
        }
        if let Some(v) = &self.placeholder {
            n.set_placeholder(v.as_str());
        }
        if let Some(v) = &self.shortcut {
            n.set_keyboard_shortcut(v.as_str());
        }
        if let Some(v) = self.numeric {
            n.set_numeric_value(v.value);
            n.set_min_numeric_value(v.min);
            n.set_max_numeric_value(v.max);
            n.set_numeric_value_step(v.step);
            n.set_numeric_value_jump(v.step * 10.0);
        }
        if let Some(on) = self.toggled {
            n.set_toggled(Toggled::from(on));
        }
        if let Some(on) = self.selected {
            n.set_selected(on);
        }
        if let Some(on) = self.expanded {
            n.set_expanded(on);
        }
        if let Some(v) = self.vertical {
            n.set_orientation(if v {
                Orientation::Vertical
            } else {
                Orientation::Horizontal
            });
        }
        if let Some((v, max)) = self.scroll_x {
            n.set_scroll_x(v);
            n.set_scroll_x_min(0.0);
            n.set_scroll_x_max(max);
        }
        if let Some((v, max)) = self.scroll_y {
            n.set_scroll_y(v);
            n.set_scroll_y_min(0.0);
            n.set_scroll_y_max(max);
        }
        if self.live {
            n.set_live(Live::Polite);
        }
        if self.disabled == Some(true) {
            n.set_disabled();
        } else {
            for a in &self.actions {
                n.add_action(*a);
            }
            if focus {
                n.add_action(Action::Focus);
            }
        }
        if let Some(b) = bounds {
            n.set_bounds(accesskit::Rect::new(
                f64::from(b.x) * scale,
                f64::from(b.y) * scale,
                f64::from(b.right()) * scale,
                f64::from(b.bottom()) * scale,
            ));
        }
        if !children.is_empty() {
            n.set_children(children);
        }
        n
    }
}

struct Slot {
    id: Id,
    parent: Option<Id>,
    layer: LayerId,
    bounds: Option<Rect>,
    label_rank: u8,
    sem: Semantics,
}

/// Accessibility state held by [`Ui`].
#[derive(Default)]
pub(crate) struct A11y {
    pub on: bool,
    title: String,
    slots: Vec<Slot>,
    pub(crate) index: HashMap<Id, usize>,
    stack: Vec<Id>,
    last: Option<Id>,
    pub(crate) actions: Vec<ActionRequest>,
    announce: Option<(String, f64)>,
    /// The latest full tree, also used to send only what changed.
    tree: HashMap<NodeId, Node>,
    tree_focus: Option<NodeId>,
    need_full: bool,
    update: Option<TreeUpdate>,
}

impl A11y {
    pub(crate) fn begin_frame(&mut self) {
        self.slots.clear();
        self.index.clear();
        self.stack.clear();
        self.last = None;
    }

    pub(crate) fn clear_stack(&mut self) -> Vec<Id> {
        std::mem::take(&mut self.stack)
    }

    pub(crate) fn restore_stack(&mut self, s: Vec<Id>) {
        self.stack = s;
    }
}

/// How long an announcement stays in the tree.
const ANNOUNCE_SECONDS: f64 = 6.0;

impl Ui {
    // ---- connection ----

    /// True while an assistive technology is connected and the tree is
    /// being built.
    pub fn accessibility_active(&self) -> bool {
        self.a11y.on
    }

    /// The window runner calls this when an assistive technology connects or
    /// disconnects. Tests call it to build the tree headlessly.
    pub fn set_accessibility_active(&mut self, on: bool) {
        if self.a11y.on == on {
            return;
        }
        self.a11y.on = on;
        self.a11y.need_full = true;
        self.a11y.tree.clear();
        self.a11y.tree_focus = None;
        self.a11y.update = None;
        self.a11y.actions.clear();
        if !on {
            self.a11y.begin_frame();
        }
        self.request_repaint();
    }

    /// Queues an action request from an assistive technology for the next
    /// frame.
    pub fn queue_accessibility_action(&mut self, req: ActionRequest) {
        if self.a11y.on {
            self.a11y.actions.push(req);
            self.request_repaint();
        }
    }

    /// The tree changes made by the last frame, if any. The runner passes
    /// them to the platform adapter.
    pub fn take_accessibility_update(&mut self) -> Option<TreeUpdate> {
        self.a11y.update.take()
    }

    /// Call when the platform adapter did not accept an update (it had gone
    /// inactive). The next frame sends the whole tree again.
    pub fn accessibility_resync(&mut self) {
        self.a11y.need_full = true;
    }

    /// The window's accessible name (normally its title).
    pub fn set_window_label(&mut self, title: &str) {
        if self.a11y.title != title {
            self.a11y.title = title.to_string();
        }
    }

    /// The most recently built tree, keyed by AccessKit node id. For tests
    /// and tooling.
    pub fn accessibility_tree(&self) -> &HashMap<NodeId, Node> {
        &self.a11y.tree
    }

    /// The node a widget produced in the last built tree.
    pub fn accessibility_node(&self, id: Id) -> Option<&Node> {
        self.a11y.tree.get(&node_id(id))
    }

    /// The AccessKit id of a widget.
    pub fn accessibility_node_id(id: Id) -> NodeId {
        node_id(id)
    }

    // ---- describing ----

    /// Describes a custom widget in the common case: a role, a name and an
    /// optional value text (pass `""` for none). The node's position is
    /// where `id` was last given to [`Ui::interact`]; use
    /// [`Ui::describe_node`] with [`Semantics::rect`] when it was not.
    pub fn describe(&mut self, id: Id, role: Role, label: &str, value: &str) {
        if !self.a11y.on {
            return;
        }
        let mut s = Semantics::new(role);
        if !label.is_empty() {
            s = s.label(label);
        }
        if !value.is_empty() {
            s = s.value(value);
        }
        self.describe_node(id, s);
    }

    /// Describes a widget in full, or adjusts one that already described
    /// itself: whatever `sem` sets replaces what was there, and a name set
    /// here beats names derived from visible text, icons or tooltips.
    pub fn describe_node(&mut self, id: Id, sem: Semantics) {
        if let Some(kind) = sem.focusable {
            let r = sem.rect.or_else(|| self.widget_rect(id));
            if let Some(r) = r {
                self.focusable(id, r, kind);
            }
        }
        if !self.a11y.on {
            return;
        }
        self.a11y_upsert(id, RANK_EXPLICIT, sem);
        self.a11y.last = Some(id);
    }

    /// Describes `id` and makes it the parent of every widget described
    /// inside `f`.
    pub fn describe_group<R>(&mut self, id: Id, sem: Semantics, f: impl FnOnce(&mut Ui) -> R) -> R {
        let on = self.a11y.on;
        if on {
            self.describe_node(id, sem);
            self.a11y.stack.push(id);
        }
        let out = f(self);
        if on {
            self.a11y.stack.pop();
            self.a11y.last = Some(id);
        }
        out
    }

    /// Names a widget (for example the response of an icon button or a
    /// knob). Beats any name the widget derived itself.
    pub fn name(&mut self, r: &Response, name: &str) {
        if self.a11y.on {
            self.describe_node(r.id, Semantics::patch().label(name));
        }
    }

    /// Sets the value text of a widget ("2.4 kHz").
    pub fn value_text(&mut self, r: &Response, text: &str) {
        if self.a11y.on {
            self.describe_node(r.id, Semantics::patch().value(text));
        }
    }

    /// Describes a piece of visible text (a heading, a channel name) so
    /// screen readers can find it. `ui.text` itself is not described.
    pub fn describe_label(&mut self, id: Id, rect: Rect, text: &str) {
        if self.a11y.on {
            self.describe_node(id, Semantics::new(Role::Label).label(text).rect(rect));
        }
    }

    /// Names the widget described last.
    pub fn label_last(&mut self, name: &str) {
        if let Some(id) = self.a11y.last {
            self.describe_node(id, Semantics::patch().label(name));
        }
    }

    /// Sets the value text of the widget described last.
    pub fn value_last(&mut self, text: &str) {
        if let Some(id) = self.a11y.last {
            self.describe_node(id, Semantics::patch().value(text));
        }
    }

    /// Has the screen reader say `message` (a polite live region), for
    /// results that otherwise only show on screen: "Exported", "Recording
    /// stopped". It stays in the tree for a few seconds.
    pub fn announce(&mut self, message: impl Into<String>) {
        if self.a11y.on {
            let now = self.time();
            self.a11y.announce = Some((message.into(), now));
            self.request_repaint();
        }
    }

    // ---- internals for widgets ----

    /// Describes a standard widget. `f` only runs while the tree is built.
    pub(crate) fn a11y_node(
        &mut self,
        id: Id,
        rect: Rect,
        rank: u8,
        f: impl FnOnce() -> Semantics,
    ) {
        if !self.a11y.on {
            return;
        }
        let mut s = f();
        s.rect = Some(rect);
        self.a11y_upsert(id, rank, s);
        self.a11y.last = Some(id);
    }

    pub(crate) fn a11y_push(&mut self, id: Id) {
        if self.a11y.on {
            self.a11y.stack.push(id);
        }
    }

    pub(crate) fn a11y_pop(&mut self, last: Id) {
        if self.a11y.on {
            self.a11y.stack.pop();
            self.a11y.last = Some(last);
        }
    }

    /// Called by [`Ui::tooltip`] every frame: the tooltip text names an
    /// unnamed widget and describes a named one.
    pub(crate) fn a11y_tooltip(&mut self, id: Id, text: &str) {
        if !self.a11y.on {
            return;
        }
        let Some(&i) = self.a11y.index.get(&id) else {
            return;
        };
        let slot = &mut self.a11y.slots[i];
        if slot.label_rank < RANK_TOOLTIP {
            slot.sem.label = Some(text.to_string());
            slot.label_rank = RANK_TOOLTIP;
        } else if slot.sem.label.as_deref() != Some(text) && slot.sem.description.is_none() {
            slot.sem.description = Some(text.to_string());
        }
    }

    fn a11y_upsert(&mut self, id: Id, rank: u8, mut sem: Semantics) {
        let rect = sem.rect.or_else(|| {
            if self.a11y.index.contains_key(&id) {
                None
            } else {
                self.widget_rect(id)
            }
        });
        let bounds = rect.map(|r| r.intersect(&self.scene.clip()));
        if let Some(&i) = self.a11y.index.get(&id) {
            let slot = &mut self.a11y.slots[i];
            if let Some(l) = sem.label.take()
                && rank >= slot.label_rank
            {
                // A tooltip that served as the name becomes the
                // description once a better name arrives.
                if slot.label_rank == RANK_TOOLTIP
                    && slot.sem.description.is_none()
                    && slot.sem.label.as_deref() != Some(l.as_str())
                {
                    slot.sem.description = slot.sem.label.take();
                }
                slot.sem.label = Some(l);
                slot.label_rank = rank;
            }
            if bounds.is_some() {
                slot.bounds = bounds;
            }
            slot.sem.merge(sem);
            return;
        }
        // Skip widgets scrolled out of view altogether.
        if bounds.is_some_and(|b| b.is_empty()) {
            return;
        }
        let label_rank = if sem.label.is_some() { rank } else { 0 };
        let parent = self.a11y.stack.last().copied().filter(|p| *p != id);
        self.a11y.index.insert(id, self.a11y.slots.len());
        self.a11y.slots.push(Slot {
            id,
            parent,
            layer: self.scene.layer_id(),
            bounds,
            label_rank,
            sem,
        });
    }

    /// Builds this frame's tree and the update for the platform adapter.
    pub(crate) fn a11y_end_frame(&mut self) {
        if !self.a11y.on {
            return;
        }
        let scale = f64::from(self.scale());
        let focusable: HashSet<Id> = self.focus_stops.iter().map(|s| s.id).collect();

        let mut kids: HashMap<Id, Vec<NodeId>> = HashMap::new();
        let mut top: Vec<(LayerId, usize, NodeId)> = Vec::new();
        for (i, s) in self.a11y.slots.iter().enumerate() {
            match s.parent.filter(|p| self.a11y.index.contains_key(p)) {
                Some(p) => kids.entry(p).or_default().push(node_id(s.id)),
                None => top.push((s.layer, i, node_id(s.id))),
            }
        }
        top.sort_by_key(|t| (t.0, t.1));

        let mut nodes: HashMap<NodeId, Node> = HashMap::with_capacity(self.a11y.slots.len() + 2);
        for s in &self.a11y.slots {
            let children = kids.get(&s.id).map_or(&[][..], Vec::as_slice);
            nodes.insert(
                node_id(s.id),
                s.sem
                    .to_node(s.bounds, scale, children, focusable.contains(&s.id)),
            );
        }

        let mut root_children: Vec<NodeId> = top.iter().map(|t| t.2).collect();
        let now = self.time();
        if let Some((msg, at)) = &self.a11y.announce {
            if now - at > ANNOUNCE_SECONDS {
                self.a11y.announce = None;
            } else {
                let id = node_id(Id::new("peel-ui/announcement"));
                let mut n = Node::new(Role::Status);
                n.set_label(msg.as_str());
                n.set_live(Live::Polite);
                nodes.insert(id, n);
                root_children.push(id);
            }
        }

        let mut root = Node::new(Role::Window);
        if !self.a11y.title.is_empty() {
            root.set_label(self.a11y.title.as_str());
        }
        let s = self.screen();
        root.set_bounds(accesskit::Rect::new(
            f64::from(s.x) * scale,
            f64::from(s.y) * scale,
            f64::from(s.right()) * scale,
            f64::from(s.bottom()) * scale,
        ));
        root.set_children(root_children);
        nodes.insert(ROOT, root);

        let focus_id = [self.kb_focus, self.focus_id()]
            .into_iter()
            .flatten()
            .find(|f| self.a11y.index.contains_key(f))
            .map_or(ROOT, node_id);

        // A pending update nobody collected means the adapter is behind, so
        // send everything.
        let full = self.a11y.need_full || self.a11y.update.is_some();
        let changed: Vec<(NodeId, Node)> = nodes
            .iter()
            .filter(|(id, n)| full || self.a11y.tree.get(*id) != Some(*n))
            .map(|(id, n)| (*id, n.clone()))
            .collect();
        let focus_changed = self.a11y.tree_focus != Some(focus_id);
        if full || !changed.is_empty() || focus_changed {
            self.a11y.update = Some(TreeUpdate {
                nodes: changed,
                tree: full.then(|| TreeInfo {
                    root: ROOT,
                    toolkit_name: Some("peel-ui".into()),
                    toolkit_version: Some(env!("CARGO_PKG_VERSION").into()),
                }),
                tree_id: TreeId::ROOT,
                focus: focus_id,
            });
        }
        self.a11y.need_full = false;
        self.a11y.tree = nodes;
        self.a11y.tree_focus = Some(focus_id);
    }
}

/// Applies a normalized value change (0..=1, reported as percent) from the
/// keyboard or assistive technology. True if `norm` changed.
pub(crate) fn adjust_norm(adj: &Adjust, norm: &mut f32, default: f32) -> bool {
    if !adj.any() {
        return false;
    }
    let v = adj.apply(
        f64::from(*norm),
        0.0,
        1.0,
        0.01,
        Some(f64::from(default)),
        100.0,
    ) as f32;
    let changed = v != *norm;
    *norm = v;
    changed
}
