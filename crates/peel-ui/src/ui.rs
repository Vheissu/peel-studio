//! The immediate-mode UI context.
//!
//! Widgets call [`Ui::interact`] with a stable [`Id`] and a rectangle. Hover
//! is resolved from the previous frame's interaction regions (topmost layer,
//! then latest registered), so popups and overlays always win over what is
//! underneath them. A pressed widget becomes *active* and keeps receiving
//! drag events until the button is released, even outside its rectangle.

use std::any::Any;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use winit::window::CursorIcon;

use crate::a11y::A11y;
use crate::focus::{Adjust, FocusStop};
use crate::geom::{Color, Rect, Vec2, vec2};
use crate::input::{Button, Input};
use crate::scene::{Align, LayerId, Scene};
use crate::text::{TextStyle, TextSystem};
use crate::theme::Theme;

/// Stable widget identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Id(pub u64);

impl Id {
    pub fn new(source: impl Hash) -> Id {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut h);
        Id(h.finish())
    }

    /// Derives a child id.
    pub fn with(self, source: impl Hash) -> Id {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.0.hash(&mut h);
        source.hash(&mut h);
        Id(h.finish())
    }
}

#[derive(Clone, Copy, Debug)]
struct Hit {
    id: Id,
    rect: Rect,
    layer: LayerId,
}

/// Result of interacting with a widget this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Response {
    pub id: Id,
    pub rect: Rect,
    pub hovered: bool,
    /// Left button went down on this widget this frame.
    pub pressed: bool,
    /// Left button released over the widget after pressing it, without
    /// dragging far.
    pub clicked: bool,
    pub double_clicked: bool,
    /// Right button pressed on the widget.
    pub secondary_clicked: bool,
    /// Middle button pressed on the widget.
    pub middle_pressed: bool,
    /// Widget is active (pressed and held).
    pub active: bool,
    pub dragging: bool,
    pub drag_delta: Vec2,
    /// Left button released this frame while the widget was active.
    pub released: bool,
    /// A value changed this frame (set by value widgets).
    pub changed: bool,
    /// The widget holds keyboard focus (Tab or assistive technology).
    pub focused: bool,
}

impl Response {
    pub fn union(mut self, o: Response) -> Response {
        self.hovered |= o.hovered;
        self.pressed |= o.pressed;
        self.clicked |= o.clicked;
        self.double_clicked |= o.double_clicked;
        self.secondary_clicked |= o.secondary_clicked;
        self.changed |= o.changed;
        self.dragging |= o.dragging;
        self.focused |= o.focused;
        self
    }
}

struct Anim {
    value: f32,
    target: f32,
}

/// Payload carried by an in-app drag and drop.
pub struct DragPayload {
    pub data: Box<dyn Any>,
    pub label: String,
    pub color: Color,
}

pub struct Ui {
    pub theme: Theme,
    pub text: TextSystem,
    pub scene: Scene,
    pub input: Input,
    screen: Rect,
    time: f64,
    dt: f32,
    hits: Vec<Hit>,
    prev_hits: Vec<Hit>,
    hover: Option<Id>,
    active: Option<Id>,
    active_drag_dist: f32,
    pub(crate) focus: Option<Id>,
    /// Keyboard focus (Tab or assistive technology), see `focus.rs`.
    pub(crate) kb_focus: Option<Id>,
    pub(crate) kb_entered: Option<Id>,
    pub(crate) kb_target: Option<Id>,
    pub(crate) kb_adjust: Adjust,
    pub(crate) kb_activate: bool,
    pub(crate) focus_stops: Vec<FocusStop>,
    pub(crate) prev_focus_stops: Vec<FocusStop>,
    pub(crate) a11y: A11y,
    state: HashMap<Id, Box<dyn Any>>,
    anims: HashMap<Id, Anim>,
    cursor: CursorIcon,
    repaint: bool,
    tooltip: Option<(Id, String)>,
    hover_since: (Option<Id>, f64),
    pub(crate) popup: Option<(Id, Rect)>,
    popup_opened_this_frame: bool,
    /// Keyboard focus before the popup took it, restored when it closes.
    popup_return_focus: Option<Id>,
    /// The popup was opened from the keyboard: focus its first row.
    popup_focus_pending: bool,
    popup_took_focus: bool,
    dnd: Option<DragPayload>,
    dnd_dropped: bool,
}

impl Ui {
    pub fn new() -> Ui {
        Ui {
            theme: Theme::dark(),
            text: TextSystem::new(),
            scene: Scene::new(),
            input: Input::new(),
            screen: Rect::ZERO,
            time: 0.0,
            dt: 1.0 / 60.0,
            hits: Vec::new(),
            prev_hits: Vec::new(),
            hover: None,
            active: None,
            active_drag_dist: 0.0,
            focus: None,
            kb_focus: None,
            kb_entered: None,
            kb_target: None,
            kb_adjust: Adjust::default(),
            kb_activate: false,
            focus_stops: Vec::new(),
            prev_focus_stops: Vec::new(),
            a11y: A11y::default(),
            state: HashMap::new(),
            anims: HashMap::new(),
            cursor: CursorIcon::Default,
            repaint: false,
            tooltip: None,
            hover_since: (None, 0.0),
            popup: None,
            popup_opened_this_frame: false,
            popup_return_focus: None,
            popup_focus_pending: false,
            popup_took_focus: false,
            dnd: None,
            dnd_dropped: false,
        }
    }

    // ---- frame lifecycle (called by the app runner) ----

    pub fn begin_frame(&mut self, screen: Rect, scale: f32, time: f64) {
        self.dt = ((time - self.time) as f32).clamp(0.0, 0.1);
        self.time = time;
        self.screen = screen;
        self.scene.begin(scale);
        self.input.set_scale(scale);
        std::mem::swap(&mut self.hits, &mut self.prev_hits);
        self.hits.clear();
        std::mem::swap(&mut self.focus_stops, &mut self.prev_focus_stops);
        self.focus_stops.clear();
        self.a11y.begin_frame();
        self.cursor = CursorIcon::Default;
        self.repaint = false;
        self.tooltip = None;
        self.popup_opened_this_frame = false;
        self.dnd_dropped = false;

        self.hover = None;
        if let Some(p) = self.input.pointer {
            let mut best: Option<Hit> = None;
            for h in &self.prev_hits {
                if h.rect.contains(p) && best.is_none_or(|b| h.layer >= b.layer) {
                    best = Some(*h);
                }
            }
            self.hover = best.map(|h| h.id);
        }
        if self.hover != self.hover_since.0 {
            self.hover_since = (self.hover, time);
        }
        if let Some(a) = self.active {
            let alive = self.prev_hits.iter().any(|h| h.id == a);
            if !alive && !self.input.down.iter().any(|d| *d) {
                self.active = None;
            }
            self.active_drag_dist += self.input.pointer_delta.length();
        }
        // Close the popup on an outside press or Escape.
        if let Some((_, r)) = self.popup {
            let outside = self.input.pressed.iter().any(|p| *p)
                && self.input.pointer.is_none_or(|p| !r.contains(p));
            if outside {
                self.dismiss_popup();
            }
        }
        self.process_keyboard();
    }

    pub fn end_frame(&mut self) {
        if self.input.released.iter().any(|r| *r) {
            self.active = None;
        }
        if self.input.released(Button::Left) && self.dnd.is_some() {
            self.dnd = None;
            self.repaint = true;
        }
        self.draw_drag_preview();
        self.draw_tooltip();
        if !self.input.keys.is_empty() && self.popup.is_some() {
            let esc = self.input.keys.iter().any(|k| {
                !k.consumed
                    && k.key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
            });
            if esc {
                self.dismiss_popup();
                self.repaint = true;
            }
        }
        self.finish_keyboard();
        self.a11y_end_frame();
        self.text.end_frame();
        self.input.end_frame();
    }

    // ---- accessors ----

    pub fn screen(&self) -> Rect {
        self.screen
    }

    pub fn time(&self) -> f64 {
        self.time
    }

    pub fn dt(&self) -> f32 {
        self.dt
    }

    pub fn scale(&self) -> f32 {
        self.scene.scale()
    }

    pub fn pointer(&self) -> Option<Vec2> {
        self.input.pointer
    }

    pub fn cursor(&self) -> CursorIcon {
        self.cursor
    }

    pub fn set_cursor(&mut self, c: CursorIcon) {
        self.cursor = c;
    }

    /// Asks the runner to draw another frame (animation, playback).
    pub fn request_repaint(&mut self) {
        self.repaint = true;
    }

    pub fn wants_repaint(&self) -> bool {
        self.repaint || self.active.is_some() || self.dnd.is_some()
    }

    pub fn is_active(&self, id: Id) -> bool {
        self.active == Some(id)
    }

    pub fn any_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn is_hovered(&self, id: Id) -> bool {
        self.hover == Some(id)
    }

    /// True if the pointer is inside `r` and nothing on a higher layer
    /// covers it.
    pub fn pointer_in(&self, r: Rect) -> bool {
        let Some(p) = self.input.pointer else {
            return false;
        };
        if !r.contains(p) || !self.scene.clip().contains(p) {
            return false;
        }
        let layer = self.scene.layer_id();
        !self
            .prev_hits
            .iter()
            .any(|h| h.layer > layer && h.rect.contains(p))
    }

    // ---- focus ----

    pub fn request_focus(&mut self, id: Id) {
        self.focus = Some(id);
    }

    pub fn has_focus(&self, id: Id) -> bool {
        self.focus == Some(id)
    }

    pub fn clear_focus(&mut self) {
        self.focus = None;
    }

    /// True when a text field holds keyboard focus (global shortcuts should
    /// not fire).
    pub fn text_focus(&self) -> bool {
        self.focus.is_some()
    }

    pub(crate) fn focus_id(&self) -> Option<Id> {
        self.focus
    }

    // ---- persistent widget state ----

    pub fn state<T: 'static + Default>(&mut self, id: Id) -> &mut T {
        let entry = self
            .state
            .entry(id)
            .or_insert_with(|| Box::new(T::default()));
        if !entry.is::<T>() {
            *entry = Box::new(T::default());
        }
        entry.downcast_mut::<T>().expect("state type")
    }

    pub fn remove_state(&mut self, id: Id) {
        self.state.remove(&id);
    }

    // ---- interaction ----

    pub fn interact(&mut self, id: Id, rect: Rect) -> Response {
        let clipped = rect.intersect(&self.scene.clip());
        if !clipped.is_empty() {
            self.hits.push(Hit {
                id,
                rect: clipped,
                layer: self.scene.layer_id(),
            });
        }
        let blocked_by_popup = self.popup.is_some_and(|(_, r)| {
            self.scene.layer_id() < LayerId::POPUP
                && self.input.pointer.is_some_and(|p| r.contains(p))
        });
        let hovered =
            self.hover == Some(id) && self.active.is_none_or(|a| a == id) && !blocked_by_popup;
        let mut r = Response {
            id,
            rect,
            hovered,
            ..Default::default()
        };

        if hovered && self.input.pressed(Button::Left) && self.active.is_none() {
            self.active = Some(id);
            self.active_drag_dist = 0.0;
            r.pressed = true;
            r.double_clicked = self.input.double_clicked;
            if self.focus.is_some() && self.focus != Some(id) {
                self.focus = None;
            }
        }
        if hovered && self.input.pressed(Button::Right) {
            r.secondary_clicked = true;
        }
        if hovered && self.input.pressed(Button::Middle) {
            r.middle_pressed = true;
            if self.active.is_none() {
                self.active = Some(id);
                self.active_drag_dist = 0.0;
            }
        }
        if self.active == Some(id) {
            r.active = true;
            let any_down = self.input.down.iter().any(|d| *d);
            r.dragging = any_down && !r.pressed;
            r.drag_delta = self.input.pointer_delta;
            if self.input.released(Button::Left) || self.input.released(Button::Middle) {
                r.released = true;
                let over = self.input.pointer.is_some_and(|p| rect.contains(p));
                r.clicked =
                    over && self.active_drag_dist < 6.0 && self.input.released(Button::Left);
            }
            self.repaint = true;
        }
        if hovered {
            self.repaint |= self.input.had_event;
        }
        self.interact_extras(id, &mut r);
        r
    }

    /// Where a widget was last drawn (its clipped interaction rectangle).
    /// Used by tests and tooling to target widgets by id.
    pub fn widget_rect(&self, id: Id) -> Option<Rect> {
        self.hits
            .iter()
            .chain(self.prev_hits.iter())
            .find(|h| h.id == id)
            .map(|h| h.rect)
    }

    /// Total pointer travel since the active widget was pressed.
    pub fn drag_distance(&self) -> f32 {
        self.active_drag_dist
    }

    /// Where the active drag started.
    pub fn press_pos(&self) -> Vec2 {
        self.input.press_pos[0]
    }

    // ---- animation ----

    /// Exponentially approaches `target`; `speed` is roughly 1/seconds.
    pub fn animate(&mut self, id: Id, target: f32, speed: f32) -> f32 {
        let dt = self.dt;
        let a = self.anims.entry(id).or_insert(Anim {
            value: target,
            target,
        });
        a.target = target;
        let k = 1.0 - (-speed * dt).exp();
        a.value += (a.target - a.value) * k;
        if (a.value - a.target).abs() < 0.002 {
            a.value = a.target;
        } else {
            self.repaint = true;
        }
        a.value
    }

    pub fn animate_bool(&mut self, id: Id, on: bool) -> f32 {
        self.animate(id, if on { 1.0 } else { 0.0 }, 18.0)
    }

    // ---- text ----

    pub fn measure(&mut self, text: &str, style: TextStyle) -> Vec2 {
        self.text.measure(text, style)
    }

    /// Draws text with its top-left at `pos`.
    pub fn text_at(&mut self, pos: Vec2, text: &str, style: TextStyle, color: Color) -> Vec2 {
        if text.is_empty() {
            return Vec2::ZERO;
        }
        let (key, size) = self.text.layout(text, style, None);
        self.scene.push_text(key, pos, color);
        size
    }

    /// Draws single-line text inside `r`, vertically centered.
    pub fn text(&mut self, r: Rect, text: &str, style: TextStyle, color: Color, align: Align) {
        if text.is_empty() || r.is_empty() {
            return;
        }
        let (key, size) = self.text.layout(text, style, None);
        let x = match align {
            Align::Start => r.x,
            Align::Center => r.x + (r.w - size.x) * 0.5,
            Align::End => r.right() - size.x,
        };
        // Optical centering: cap-height glyphs sit slightly high in the
        // line box, so nudge down a hair.
        let y = r.y + (r.h - size.y) * 0.5 + style.size * 0.04;
        let sc = self.scene.scale();
        let snap = |v: f32| (v * sc).round() / sc;
        if size.x > r.w + 0.5 {
            self.scene.push_clip(r);
            self.scene
                .push_text(key, vec2(snap(x.max(r.x)), snap(y)), color);
            self.scene.pop_clip();
        } else {
            self.scene.push_text(key, vec2(snap(x), snap(y)), color);
        }
    }

    /// Draws wrapped text in a box; returns the height used.
    pub fn text_wrapped(
        &mut self,
        pos: Vec2,
        width: f32,
        text: &str,
        style: TextStyle,
        color: Color,
    ) -> f32 {
        let (key, size) = self.text.layout(text, style, Some(width));
        self.scene.push_text(key, pos, color);
        size.y
    }

    /// Text truncated with an ellipsis to fit `max_w`.
    pub fn fit_text(&mut self, text: &str, style: TextStyle, max_w: f32) -> String {
        if self.measure(text, style).x <= max_w {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut lo = 0;
        let mut hi = chars.len();
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let s: String = chars[..mid].iter().collect::<String>() + "…";
            if self.measure(&s, style).x <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        chars[..lo].iter().collect::<String>() + "…"
    }

    /// Draws an icon glyph centered in `r`.
    pub fn icon(&mut self, r: Rect, icon: &str, size: f32, color: Color) {
        let style = TextStyle::icon(size);
        let (key, s) = self.text.layout(icon, style, None);
        let sc = self.scene.scale();
        let snap = |v: f32| (v * sc).round() / sc;
        self.scene.push_text(
            key,
            vec2(snap(r.x + (r.w - s.x) * 0.5), snap(r.y + (r.h - s.y) * 0.5)),
            color,
        );
    }

    pub fn icon_fill(&mut self, r: Rect, icon: &str, size: f32, color: Color) {
        let style = TextStyle::icon_fill(size);
        let (key, s) = self.text.layout(icon, style, None);
        let sc = self.scene.scale();
        let snap = |v: f32| (v * sc).round() / sc;
        self.scene.push_text(
            key,
            vec2(snap(r.x + (r.w - s.x) * 0.5), snap(r.y + (r.h - s.y) * 0.5)),
            color,
        );
    }

    // ---- layers ----

    pub fn with_layer<R>(&mut self, layer: LayerId, f: impl FnOnce(&mut Ui) -> R) -> R {
        let prev = self.scene.set_layer(layer);
        // Widgets on another layer (popups) are not children of the widget
        // that is open on this one.
        let stack = (layer != prev).then(|| self.a11y.clear_stack());
        let r = f(self);
        if let Some(stack) = stack {
            self.a11y.restore_stack(stack);
        }
        self.scene.set_layer(prev);
        r
    }

    pub fn with_clip<R>(&mut self, clip: Rect, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.scene.push_clip(clip);
        let r = f(self);
        self.scene.pop_clip();
        r
    }

    // ---- tooltips ----

    /// Shows `text` after the pointer rests on the response for a moment.
    pub fn tooltip(&mut self, r: &Response, text: &str) {
        self.a11y_tooltip(r.id, text);
        if r.hovered && self.active.is_none() && self.popup.is_none() && self.dnd.is_none() {
            let rest = self.time - self.hover_since.1;
            if rest > 0.55 {
                self.tooltip = Some((r.id, text.to_string()));
            } else {
                self.repaint = true;
            }
        }
    }

    fn draw_tooltip(&mut self) {
        let Some((_, text)) = self.tooltip.take() else {
            return;
        };
        let Some(p) = self.input.pointer else { return };
        let style = self.theme.caption;
        let size = self.measure(&text, style);
        let pad = vec2(8.0, 5.0);
        let w = size.x + pad.x * 2.0;
        let h = size.y + pad.y * 2.0;
        let mut x = p.x + 12.0;
        let mut y = p.y + 20.0;
        if x + w > self.screen.right() - 4.0 {
            x = self.screen.right() - 4.0 - w;
        }
        if y + h > self.screen.bottom() - 4.0 {
            y = p.y - h - 8.0;
        }
        let r = Rect { x, y, w, h };
        let theme = self.theme.clone();
        self.with_layer(LayerId::TOOLTIP, |ui| {
            ui.scene.push_clip_absolute(Rect::EVERYTHING);
            ui.scene.shadow(
                r.translate(vec2(0.0, 2.0)),
                theme.radius_s,
                6.0,
                theme.shadow.alpha(0.6),
            );
            ui.scene
                .rect_stroke(r, theme.radius_s, theme.raised, 1.0, theme.line_strong);
            ui.text_at(vec2(r.x + pad.x, r.y + pad.y), &text, style, theme.text);
            ui.scene.pop_clip();
        });
    }

    // ---- popups ----

    pub fn open_popup(&mut self, id: Id, rect: Rect) {
        self.popup = Some((id, rect));
        self.popup_opened_this_frame = true;
        self.repaint = true;
        // Opened from the keyboard: the popup takes focus, and gives it
        // back to the opener when it closes.
        self.popup_return_focus = self.kb_focus;
        self.popup_focus_pending = self.kb_focus.is_some();
        self.popup_took_focus = false;
    }

    pub fn close_popup(&mut self) {
        self.dismiss_popup();
        self.repaint = true;
    }

    fn dismiss_popup(&mut self) {
        self.popup = None;
        if std::mem::take(&mut self.popup_took_focus) {
            let back = self.popup_return_focus.take();
            self.set_kb_focus(back);
        }
        self.popup_focus_pending = false;
    }

    /// Moves keyboard focus into a popup that was opened from the keyboard.
    /// `first` is its first focusable row.
    pub(crate) fn popup_take_focus(&mut self, first: Option<Id>) {
        if std::mem::take(&mut self.popup_focus_pending)
            && let Some(id) = first
        {
            self.popup_took_focus = true;
            self.set_kb_focus(Some(id));
        }
    }

    pub fn is_popup_open(&self, id: Id) -> bool {
        self.popup.is_some_and(|(p, _)| p == id)
    }

    pub fn popup_rect(&self) -> Option<Rect> {
        self.popup.map(|(_, r)| r)
    }

    pub fn any_popup(&self) -> bool {
        self.popup.is_some()
    }

    /// Updates the open popup's rectangle (when content size is known).
    pub fn set_popup_rect(&mut self, rect: Rect) {
        if let Some((_, r)) = &mut self.popup {
            *r = rect;
        }
    }

    // ---- drag and drop ----

    pub fn start_drag(&mut self, payload: DragPayload) {
        self.dnd = Some(payload);
    }

    pub fn dragging_payload(&self) -> Option<&DragPayload> {
        self.dnd.as_ref()
    }

    pub fn is_dragging_payload<T: 'static>(&self) -> bool {
        self.dnd.as_ref().is_some_and(|d| d.data.is::<T>())
    }

    /// If a payload of type `T` is released over `rect`, takes it.
    pub fn take_drop<T: 'static>(&mut self, rect: Rect) -> Option<(T, Vec2)> {
        if !self.input.released(Button::Left) || self.dnd_dropped {
            return None;
        }
        let p = self.input.pointer?;
        if !rect.contains(p) || !self.scene.clip().contains(p) {
            return None;
        }
        if !self.dnd.as_ref()?.data.is::<T>() {
            return None;
        }
        let payload = self.dnd.take()?;
        self.dnd_dropped = true;
        payload.data.downcast::<T>().ok().map(|b| (*b, p))
    }

    /// Payload of type `T` currently hovering over `rect` (for previews).
    pub fn drop_hover<T: 'static>(&self, rect: Rect) -> Option<&T> {
        let p = self.input.pointer?;
        if !rect.contains(p) {
            return None;
        }
        self.dnd.as_ref()?.data.downcast_ref::<T>()
    }

    /// Files dropped from the OS onto `rect` this frame.
    pub fn take_dropped_files(&mut self, rect: Rect) -> Vec<std::path::PathBuf> {
        match self.input.pointer {
            Some(p) if rect.contains(p) && !self.input.dropped_files.is_empty() => {
                std::mem::take(&mut self.input.dropped_files)
            }
            _ => Vec::new(),
        }
    }

    fn draw_drag_preview(&mut self) {
        let Some(p) = self.input.pointer else { return };
        let Some(d) = &self.dnd else { return };
        let label = d.label.clone();
        let color = d.color;
        let style = self.theme.label;
        let size = self.measure(&label, style);
        let r = Rect {
            x: p.x + 10.0,
            y: p.y + 6.0,
            w: size.x + 26.0,
            h: 26.0,
        };
        let theme = self.theme.clone();
        self.with_layer(LayerId::DRAG, |ui| {
            ui.scene.push_clip_absolute(Rect::EVERYTHING);
            ui.scene.shadow(
                r.translate(vec2(0.0, 3.0)),
                theme.radius_m,
                8.0,
                theme.shadow,
            );
            ui.scene.rect(r, theme.radius_m, theme.raised);
            ui.scene.rect(
                Rect { w: 4.0, ..r }
                    .shrink2(0.0, 5.0)
                    .translate(vec2(6.0, 0.0)),
                2.0,
                color,
            );
            ui.text(
                Rect { x: r.x + 16.0, ..r },
                &label,
                style,
                theme.text,
                Align::Start,
            );
            ui.scene.pop_clip();
        });
        self.set_cursor(CursorIcon::Grabbing);
    }
}

impl Default for Ui {
    fn default() -> Self {
        Ui::new()
    }
}
