//! Headless tests for the accessibility tree and keyboard focus.

use accesskit::{Action, ActionData, ActionRequest, NodeId, Role, Toggled, TreeId, TreeUpdate};
use accesskit_consumer::{Tree, TreeChangeHandler};
use winit::keyboard::{Key, NamedKey, SmolStr};

use crate::geom::{Rect, rect};
use crate::icons;
use crate::input::{KeyPress, Modifiers};
use crate::ui::{Id, Response, Ui};
use crate::{FocusKind, MenuItem, Semantics, vec2};

struct NoHandler;

#[test]
fn idle_text_fields_schedule_blinks_without_continuous_redraws() {
    let mut h = Harness::new();
    let id = Id::new("idle-editor");
    let mut text = "Template name".to_string();
    h.ui.focus_text_edit(id, true);
    h.frame(|ui| {
        ui.text_edit(id, rect(10.0, 10.0, 300.0, 30.0), &mut text, "Name");
    });
    assert!(h.ui.has_focus(id));
    assert!(
        !h.ui.wants_repaint(),
        "an idle editor must let the event loop sleep"
    );
    let blink = h.ui.next_repaint_at().expect("a future caret transition");
    assert!(blink > h.time && blink - h.time < 0.625);
    h.time = blink;
    h.frame(|ui| {
        ui.text_edit(id, rect(10.0, 10.0, 300.0, 30.0), &mut text, "Name");
    });
    assert!(!h.ui.wants_repaint());
    assert!(h.ui.next_repaint_at().unwrap() > blink);
    h.ui.clear_focus();
    h.frame(|ui| {
        ui.text_edit(id, rect(10.0, 10.0, 300.0, 30.0), &mut text, "Name");
    });
    assert!(
        h.ui.next_repaint_at().is_none(),
        "blur cancels the caret timer"
    );
}

impl TreeChangeHandler for NoHandler {
    fn node_added(&mut self, _: &accesskit_consumer::NodeRef) {}
    fn node_updated(&mut self, _: &accesskit_consumer::NodeRef, _: &accesskit_consumer::NodeRef) {}
    fn focus_moved(
        &mut self,
        _: Option<&accesskit_consumer::NodeRef>,
        _: Option<&accesskit_consumer::NodeRef>,
    ) {
    }
    fn node_removed(&mut self, _: &accesskit_consumer::NodeRef) {}
}

/// Drives a [`Ui`] frame by frame and plays the part of a platform adapter:
/// every update is applied to AccessKit's own consumer, which panics on a
/// malformed tree.
struct Harness {
    ui: Ui,
    time: f64,
    scale: f32,
    consumer: Option<Tree>,
    updates: Vec<TreeUpdate>,
}

impl Harness {
    fn new() -> Harness {
        let mut h = Harness::inactive();
        h.ui.set_accessibility_active(true);
        h
    }

    fn inactive() -> Harness {
        Harness {
            ui: Ui::new(),
            time: 0.0,
            scale: 1.0,
            consumer: None,
            updates: Vec::new(),
        }
    }

    /// Runs one frame; `f` draws the widgets.
    fn frame<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.time += 1.0 / 60.0;
        self.ui
            .begin_frame(rect(0.0, 0.0, 800.0, 600.0), self.scale, self.time);
        let out = f(&mut self.ui);
        self.ui.end_frame();
        if let Some(update) = self.ui.take_accessibility_update() {
            match &mut self.consumer {
                Some(tree) => tree.update_and_process_changes(update.clone(), &mut NoHandler),
                None => self.consumer = Some(Tree::new(update.clone(), true)),
            }
            self.updates.push(update);
        }
        out
    }

    fn press(&mut self, key: Key) {
        self.press_with(key, Modifiers::default());
    }

    fn press_with(&mut self, key: Key, modifiers: Modifiers) {
        self.ui.input.modifiers = modifiers;
        self.ui.input.keys.push(KeyPress {
            key,
            code: None,
            repeat: false,
            modifiers,
            consumed: false,
        });
    }

    fn tab(&mut self) {
        self.press(Key::Named(NamedKey::Tab));
    }

    fn shift_tab(&mut self) {
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        self.press_with(Key::Named(NamedKey::Tab), shift);
    }

    fn action(&mut self, action: Action, id: Id, data: Option<ActionData>) {
        self.ui.queue_accessibility_action(ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: Ui::accessibility_node_id(id),
            data,
        });
    }

    fn label(&self, id: Id) -> Option<String> {
        self.ui.accessibility_node(id)?.label().map(str::to_string)
    }

    fn focus_in_consumer(&self) -> NodeId {
        self.consumer
            .as_ref()
            .expect("tree")
            .state()
            .focus_in_tree()
            .locate()
            .0
    }
}

fn named(n: NamedKey) -> Key {
    Key::Named(n)
}

/// What an application's global shortcuts see: true if the key is still
/// there to take.
fn app_sees(ui: &mut Ui, key: NamedKey) -> bool {
    ui.input.take_named(key)
}

const CELL: f32 = 40.0;

fn cell(i: usize) -> Rect {
    rect(10.0 + i as f32 * (CELL + 10.0), 10.0, CELL, CELL)
}

#[test]
fn standard_widgets_describe_themselves() {
    let mut h = Harness::new();
    h.scale = 2.0;
    let (btn, play, led, knob, fader, slider, bpm, text, seg, meter) = (
        Id::new("btn"),
        Id::new("play"),
        Id::new("led"),
        Id::new("knob"),
        Id::new("fader"),
        Id::new("slider"),
        Id::new("bpm"),
        Id::new("text"),
        Id::new("seg"),
        Id::new("meter"),
    );
    let mut norm = 0.25_f32;
    let mut v = 0.5_f32;
    let mut s = 1.0_f32;
    let mut tempo = 120.0_f64;
    let mut name = String::from("Lead");
    let mut sel = 1_usize;
    h.frame(|ui| {
        let t = ui.theme.clone();
        ui.button(btn, cell(0), "Export");
        ui.icon_button_fill(play, cell(1), icons::PLAY, true, t.play, 16.0);
        ui.led(led, cell(2), true, t.play);
        let r = ui.knob(knob, cell(3), &mut norm, 0.5, t.accent, false);
        ui.name(&r, "Cutoff");
        ui.fader(fader, rect(10.0, 100.0, 40.0, 120.0), &mut v, 0.8, t.accent);
        ui.hslider(
            slider,
            rect(100.0, 100.0, 120.0, 20.0),
            &mut s,
            1.0,
            t.accent,
        );
        ui.drag_number(
            bpm,
            rect(300.0, 10.0, 80.0, 30.0),
            &mut tempo,
            20.0,
            400.0,
            0.2,
            t.readout,
            t.text,
            |x| format!("{x:.1}"),
        );
        ui.text_edit(text, rect(10.0, 300.0, 200.0, 30.0), &mut name, "Name");
        ui.segmented(
            seg,
            rect(10.0, 400.0, 200.0, 28.0),
            &["Pattern", "Song"],
            &mut sel,
        );
        ui.meter(meter, rect(500.0, 100.0, 20.0, 100.0), (0.5, 0.25));
    });

    let tree = h.ui.accessibility_tree();
    let root = &tree[&NodeId(0)];
    assert_eq!(root.role(), Role::Window);
    assert!(root.children().len() >= 10);
    let b = root.bounds().expect("root bounds");
    assert_eq!((b.x1, b.y1), (1600.0, 1200.0), "bounds are physical pixels");

    let n = h.ui.accessibility_node(btn).unwrap();
    assert_eq!(n.role(), Role::Button);
    assert_eq!(n.label(), Some("Export"));
    assert!(n.supports_action(Action::Click));
    assert!(n.supports_action(Action::Focus), "buttons are focusable");
    let b = n.bounds().unwrap();
    assert_eq!((b.x0, b.y0, b.x1, b.y1), (20.0, 20.0, 100.0, 100.0));

    let n = h.ui.accessibility_node(play).unwrap();
    assert_eq!(n.role(), Role::Button);
    assert_eq!(n.label(), Some("Play"), "icon buttons use the icon's name");
    assert_eq!(n.toggled(), Some(Toggled::True));

    let n = h.ui.accessibility_node(led).unwrap();
    assert_eq!(n.role(), Role::Switch);
    assert_eq!(n.toggled(), Some(Toggled::True));

    let n = h.ui.accessibility_node(knob).unwrap();
    assert_eq!(n.role(), Role::Slider);
    assert_eq!(n.label(), Some("Cutoff"));
    assert_eq!(n.numeric_value(), Some(25.0));
    assert_eq!(n.min_numeric_value(), Some(0.0));
    assert_eq!(n.max_numeric_value(), Some(100.0));
    assert_eq!(n.value(), Some("25%"));
    for a in [Action::Increment, Action::Decrement, Action::SetValue] {
        assert!(n.supports_action(a));
    }

    let n = h.ui.accessibility_node(fader).unwrap();
    assert_eq!(n.role(), Role::Slider);
    assert_eq!(n.orientation(), Some(accesskit::Orientation::Vertical));
    assert_eq!(n.numeric_value(), Some(50.0));

    let n = h.ui.accessibility_node(slider).unwrap();
    assert_eq!(n.orientation(), Some(accesskit::Orientation::Horizontal));
    assert_eq!(n.numeric_value(), Some(100.0));

    let n = h.ui.accessibility_node(bpm).unwrap();
    assert_eq!(n.role(), Role::SpinButton);
    assert_eq!(n.numeric_value(), Some(120.0));
    assert_eq!(n.min_numeric_value(), Some(20.0));
    assert_eq!(n.max_numeric_value(), Some(400.0));
    assert_eq!(n.value(), Some("120.0"));

    let n = h.ui.accessibility_node(text).unwrap();
    assert_eq!(n.role(), Role::TextInput);
    assert_eq!(n.value(), Some("Lead"));
    assert_eq!(n.placeholder(), Some("Name"));
    assert!(n.supports_action(Action::SetValue));

    let group = h.ui.accessibility_node(seg).unwrap();
    assert_eq!(group.role(), Role::RadioGroup);
    assert_eq!(group.children().len(), 2);
    let on = h.ui.accessibility_node(seg.with(1usize)).unwrap();
    assert_eq!(on.role(), Role::RadioButton);
    assert_eq!(on.label(), Some("Song"));
    assert_eq!(on.toggled(), Some(Toggled::True));
    let off = h.ui.accessibility_node(seg.with(0usize)).unwrap();
    assert_eq!(off.toggled(), Some(Toggled::False));

    let n = h.ui.accessibility_node(meter).unwrap();
    assert_eq!(n.role(), Role::Meter);
    assert_eq!(n.value(), Some("-6 dB"));
}

#[test]
fn nothing_is_built_without_a_screen_reader() {
    let mut h = Harness::inactive();
    let id = Id::new("b");
    h.frame(|ui| {
        ui.button(id, cell(0), "Go");
        ui.describe(Id::new("custom"), Role::Canvas, "Piano roll", "");
    });
    assert!(!h.ui.accessibility_active());
    assert!(h.ui.accessibility_tree().is_empty());
    assert!(h.updates.is_empty());
    // Keyboard focus does not depend on a screen reader.
    h.tab();
    h.frame(|ui| {
        ui.button(id, cell(0), "Go");
    });
    h.tab();
    let clicked = h.frame(|ui| ui.button(id, cell(0), "Go").clicked);
    assert!(!clicked);
    assert!(h.ui.is_kb_focused(id));
}

#[test]
fn updates_carry_only_what_changed() {
    let mut h = Harness::new();
    let a = Id::new("a");
    let b = Id::new("b");
    let mut on = false;
    let draw = |ui: &mut Ui, on: bool| {
        ui.button(a, cell(0), "Static");
        let t = ui.theme.clone();
        ui.icon_button(b, cell(1), icons::METRONOME, on, Some(t.accent));
    };
    h.frame(|ui| draw(ui, on));
    let first = &h.updates[0];
    assert!(
        first.tree.is_some(),
        "the first update carries the tree info"
    );
    assert_eq!(first.nodes.len(), 3, "root and two buttons");

    h.frame(|ui| draw(ui, on));
    assert_eq!(h.updates.len(), 1, "an unchanged frame sends nothing");
    assert_eq!(
        h.ui.accessibility_node(b).unwrap().toggled(),
        None,
        "a button that was never on looks like a plain button"
    );

    on = true;
    h.frame(|ui| draw(ui, on));
    assert_eq!(h.updates.len(), 2);
    let ids: Vec<NodeId> = h.updates[1].nodes.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![Ui::accessibility_node_id(b)]);
    assert!(h.updates[1].tree.is_none());
    assert_eq!(
        h.ui.accessibility_node(b).unwrap().toggled(),
        Some(Toggled::True)
    );
    on = false;
    h.frame(|ui| draw(ui, on));
    assert_eq!(
        h.ui.accessibility_node(b).unwrap().toggled(),
        Some(Toggled::False),
        "once seen on, a button stays a toggle"
    );
    h.updates.truncate(2);

    // Reconnecting a screen reader starts over with the whole tree.
    h.ui.set_accessibility_active(false);
    h.ui.set_accessibility_active(true);
    h.consumer = None;
    h.frame(|ui| draw(ui, on));
    assert_eq!(h.updates[2].nodes.len(), 3);
    assert!(h.updates[2].tree.is_some());
}

#[test]
fn removed_widgets_leave_the_tree() {
    let mut h = Harness::new();
    let a = Id::new("a");
    let b = Id::new("b");
    h.frame(|ui| {
        ui.button(a, cell(0), "A");
        ui.button(b, cell(1), "B");
    });
    assert!(h.ui.accessibility_node(b).is_some());
    h.frame(|ui| {
        ui.button(a, cell(0), "A");
    });
    assert!(h.ui.accessibility_node(b).is_none());
    assert_eq!(h.ui.accessibility_tree()[&NodeId(0)].children().len(), 1);
}

#[test]
fn widgets_scrolled_out_of_view_are_not_described() {
    let mut h = Harness::new();
    let hidden = Id::new("hidden");
    let shown = Id::new("shown");
    h.frame(|ui| {
        ui.with_clip(rect(0.0, 0.0, 100.0, 100.0), |ui| {
            ui.button(shown, rect(10.0, 10.0, 50.0, 20.0), "Shown");
            ui.button(hidden, rect(10.0, 300.0, 50.0, 20.0), "Hidden");
        });
    });
    assert!(h.ui.accessibility_node(shown).is_some());
    assert!(h.ui.accessibility_node(hidden).is_none());
    // And not reachable with Tab either.
    h.tab();
    h.frame(|ui| {
        ui.with_clip(rect(0.0, 0.0, 100.0, 100.0), |ui| {
            ui.button(shown, rect(10.0, 10.0, 50.0, 20.0), "Shown");
            ui.button(hidden, rect(10.0, 300.0, 50.0, 20.0), "Hidden");
        });
    });
    h.tab();
    h.frame(|ui| {
        ui.with_clip(rect(0.0, 0.0, 100.0, 100.0), |ui| {
            ui.button(shown, rect(10.0, 10.0, 50.0, 20.0), "Shown");
            ui.button(hidden, rect(10.0, 300.0, 50.0, 20.0), "Hidden");
        });
    });
    assert!(h.ui.is_kb_focused(shown));
}

#[test]
fn names_come_from_text_icons_tooltips_and_the_app() {
    let mut h = Harness::new();
    let icon = Id::new("icon");
    let tip = Id::new("tip");
    let named = Id::new("named");
    let text = Id::new("text");
    let plain = Id::new("plain");
    h.frame(|ui| {
        let t = ui.theme.clone();
        // An icon with a known name.
        ui.icon_button(icon, cell(0), icons::GEAR, false, None);
        // An unknown glyph: the tooltip names it.
        let r = ui.icon_button(tip, cell(1), "\u{e999}", false, None);
        ui.tooltip(&r, "Mute");
        // The app names it explicitly; the tooltip becomes the description.
        let r = ui.icon_button(named, cell(2), icons::GEAR, false, None);
        ui.tooltip(&r, "Open preferences");
        ui.name(&r, "Settings");
        // Visible text wins over the tooltip, which describes.
        let r = ui.button(text, cell(3), "Save");
        ui.tooltip(&r, "Save the project (Cmd+S)");
        // label_last names whatever was drawn last.
        ui.led(plain, cell(4), false, t.play);
        ui.label_last("Solo");
    });
    assert_eq!(h.label(icon).as_deref(), Some("Settings"));
    assert_eq!(h.label(tip).as_deref(), Some("Mute"));
    assert_eq!(h.label(named).as_deref(), Some("Settings"));
    assert_eq!(
        h.ui.accessibility_node(named).unwrap().description(),
        Some("Open preferences")
    );
    assert_eq!(h.label(text).as_deref(), Some("Save"));
    assert_eq!(
        h.ui.accessibility_node(text).unwrap().description(),
        Some("Save the project (Cmd+S)")
    );
    assert_eq!(h.label(plain).as_deref(), Some("Solo"));
}

#[test]
fn custom_canvases_describe_a_virtualized_tree() {
    let mut h = Harness::new();
    let roll = Id::new("roll");
    let draw = |ui: &mut Ui, notes: &[(u32, bool)]| {
        ui.describe_group(
            roll,
            Semantics::new(Role::Group)
                .label("Piano roll")
                .rect(rect(0.0, 0.0, 400.0, 300.0)),
            |ui| {
                for (i, (pitch, selected)) in notes.iter().enumerate() {
                    ui.describe_node(
                        roll.with(i),
                        Semantics::new(Role::ListItem)
                            .label(format!("Note {pitch}"))
                            .selected(*selected)
                            .rect(rect(10.0 + i as f32 * 30.0, 10.0, 28.0, 10.0))
                            .focusable(FocusKind::Custom),
                    );
                }
            },
        );
    };
    h.frame(|ui| draw(ui, &[(60, false), (64, true)]));
    let group = h.ui.accessibility_node(roll).unwrap();
    assert_eq!(group.role(), Role::Group);
    assert_eq!(group.label(), Some("Piano roll"));
    assert_eq!(group.children().len(), 2);
    let note = h.ui.accessibility_node(roll.with(1usize)).unwrap();
    assert_eq!(note.label(), Some("Note 64"));
    assert_eq!(note.is_selected(), Some(true));
    assert!(note.supports_action(Action::Focus));
    // The root holds the group, not the notes.
    assert_eq!(h.ui.accessibility_tree()[&NodeId(0)].children().len(), 1);

    // The tree is rebuilt each frame: fewer visible notes, fewer nodes.
    h.frame(|ui| draw(ui, &[(60, false)]));
    assert!(h.ui.accessibility_node(roll.with(1usize)).is_none());
    assert_eq!(h.ui.accessibility_node(roll).unwrap().children().len(), 1);

    // The simple form.
    let c = Id::new("canvas");
    h.frame(|ui| {
        draw(ui, &[]);
        ui.describe(c, Role::Canvas, "Step grid", "16 steps");
    });
    // No rect and no interact: described without bounds, still present.
    let n = h.ui.accessibility_node(c);
    assert!(n.is_none_or(|n| n.label() == Some("Step grid")));
}

#[test]
fn describe_uses_the_interaction_rect() {
    let mut h = Harness::new();
    let c = Id::new("canvas");
    h.frame(|ui| {
        ui.interact(c, rect(20.0, 30.0, 100.0, 50.0));
        ui.describe(c, Role::Canvas, "Playlist", "4 clips");
    });
    let n = h.ui.accessibility_node(c).unwrap();
    assert_eq!(n.label(), Some("Playlist"));
    assert_eq!(n.value(), Some("4 clips"));
    let b = n.bounds().unwrap();
    assert_eq!((b.x0, b.y0, b.x1, b.y1), (20.0, 30.0, 120.0, 80.0));
}

#[test]
fn tab_walks_the_focus_stops_in_order_and_wraps() {
    let mut h = Harness::new();
    let ids = [Id::new("a"), Id::new("b"), Id::new("c")];
    let draw = |ui: &mut Ui| {
        for (i, id) in ids.iter().enumerate() {
            ui.button(*id, cell(i), "x");
        }
    };
    h.frame(draw);
    assert_eq!(h.ui.kb_focus(), None);
    h.tab();
    h.frame(draw);
    assert_eq!(h.ui.kb_focus(), Some(ids[0]));
    assert_eq!(h.focus_in_consumer(), Ui::accessibility_node_id(ids[0]));
    h.tab();
    h.frame(draw);
    h.tab();
    h.frame(draw);
    assert_eq!(h.ui.kb_focus(), Some(ids[2]));
    h.tab();
    h.frame(draw);
    assert_eq!(h.ui.kb_focus(), Some(ids[0]), "wraps around");
    h.shift_tab();
    h.frame(draw);
    assert_eq!(h.ui.kb_focus(), Some(ids[2]), "Shift+Tab goes back");
    h.shift_tab();
    h.frame(draw);
    assert_eq!(h.ui.kb_focus(), Some(ids[1]));
}

#[test]
fn shift_tab_with_no_focus_starts_at_the_end() {
    let mut h = Harness::new();
    let ids = [Id::new("a"), Id::new("b")];
    let draw = |ui: &mut Ui| {
        for (i, id) in ids.iter().enumerate() {
            ui.button(*id, cell(i), "x");
        }
    };
    h.frame(draw);
    h.shift_tab();
    h.frame(draw);
    assert_eq!(h.ui.kb_focus(), Some(ids[1]));
}

#[test]
fn the_focus_ring_only_shows_for_keyboard_focus() {
    let mut h = Harness::new();
    let id = Id::new("b");
    let shapes = |h: &Harness| h.ui.scene.layers[0].shapes.len();
    h.frame(|ui| {
        ui.button(id, cell(0), "x");
    });
    let plain = shapes(&h);
    h.tab();
    h.frame(|ui| {
        ui.button(id, cell(0), "x");
    });
    assert_eq!(shapes(&h), plain + 1, "one outline shape for the ring");
    // The ring is 2 px wide in the theme's focus color.
    let ring = h.ui.scene.layers[0].shapes.last().unwrap();
    assert_eq!(ring.params[1], 2.0);
    assert_eq!(ring.color2, h.ui.theme.focus.to_array());
    // A pointer press clears it again.
    h.ui.input.pressed[0] = true;
    h.ui.input.down[0] = true;
    h.frame(|ui| {
        ui.button(id, cell(0), "x");
    });
    assert_eq!(shapes(&h), plain);
    assert_eq!(h.ui.kb_focus(), None);
}

#[test]
fn space_and_enter_click_only_the_focused_button() {
    let mut h = Harness::new();
    let a = Id::new("a");
    let b = Id::new("b");
    let draw = |ui: &mut Ui| {
        let ra = ui.button(a, cell(0), "A").clicked;
        let rb = ui.button(b, cell(1), "B").clicked;
        (ra, rb)
    };
    // Nothing focused: the app gets Space and the buttons stay quiet.
    h.frame(draw);
    h.press(named(NamedKey::Space));
    let (seen, clicks) = h.frame(|ui| {
        let seen = app_sees(ui, NamedKey::Space);
        (seen, draw(ui))
    });
    assert!(seen, "an unfocused UI leaves Space to the app");
    assert_eq!(clicks, (false, false));

    h.tab();
    h.frame(draw);
    assert!(h.ui.is_kb_focused(a));
    h.press(named(NamedKey::Space));
    let (seen, clicks) = h.frame(|ui| {
        let seen = app_sees(ui, NamedKey::Space);
        (seen, draw(ui))
    });
    assert!(!seen, "the focused button took Space");
    assert_eq!(clicks, (true, false));

    h.press(named(NamedKey::Enter));
    let (_, clicks) = h.frame(|ui| ((), draw(ui)));
    assert_eq!(clicks, (true, false));

    h.tab();
    h.frame(draw);
    h.press(Key::Character(SmolStr::new(" ")));
    let (_, clicks) = h.frame(|ui| ((), draw(ui)));
    assert_eq!(clicks, (false, true), "a space character counts as Space");
}

#[test]
fn letters_and_other_keys_stay_with_the_app() {
    let mut h = Harness::new();
    let a = Id::new("a");
    let draw = |ui: &mut Ui| {
        ui.button(a, cell(0), "A");
    };
    h.frame(draw);
    h.tab();
    h.frame(draw);
    assert!(h.ui.is_kb_focused(a));
    h.press(Key::Character(SmolStr::new("a")));
    h.press(named(NamedKey::ArrowLeft));
    h.press(named(NamedKey::F6));
    h.frame(|ui| {
        assert!(app_sees(ui, NamedKey::ArrowLeft), "a button ignores arrows");
        assert!(app_sees(ui, NamedKey::F6));
        assert!(ui.input.keys.iter().any(|k| !k.consumed));
        draw(ui);
    });
    // Modified keys are never taken.
    h.press_with(
        named(NamedKey::Space),
        Modifiers {
            ctrl: true,
            ..Default::default()
        },
    );
    h.frame(|ui| {
        assert!(ui.input.keys.iter().all(|k| !k.consumed));
        draw(ui);
    });
}

#[test]
fn escape_belongs_to_the_app_unless_something_is_focused() {
    let mut h = Harness::new();
    let a = Id::new("a");
    let draw = |ui: &mut Ui| {
        ui.button(a, cell(0), "A");
    };
    h.frame(draw);
    h.press(named(NamedKey::Escape));
    h.frame(|ui| {
        assert!(app_sees(ui, NamedKey::Escape));
        draw(ui);
    });
    h.tab();
    h.frame(draw);
    assert!(h.ui.is_kb_focused(a));
    h.press(named(NamedKey::Escape));
    h.frame(|ui| {
        assert!(!app_sees(ui, NamedKey::Escape), "Escape drops focus first");
        draw(ui);
    });
    assert_eq!(h.ui.kb_focus(), None);
    assert_eq!(
        h.focus_in_consumer(),
        NodeId(0),
        "the tree focus is the window"
    );
}

fn knobs(ui: &mut Ui, id: Id, norm: &mut f32) -> Response {
    let c = ui.theme.accent;
    ui.knob(id, cell(0), norm, 0.5, c, false)
}

#[test]
fn arrow_keys_adjust_only_a_focused_knob() {
    let mut h = Harness::new();
    let id = Id::new("k");
    let mut norm = 0.5_f32;
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    // Unfocused: the arrow reaches the app and the knob does not move.
    h.press(named(NamedKey::ArrowUp));
    h.frame(|ui| {
        assert!(app_sees(ui, NamedKey::ArrowUp));
        knobs(ui, id, &mut norm);
    });
    assert_eq!(norm, 0.5);

    h.tab();
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert!(h.ui.is_kb_focused(id));

    h.press(named(NamedKey::ArrowUp));
    let (seen, changed) = h.frame(|ui| {
        let seen = app_sees(ui, NamedKey::ArrowUp);
        (seen, knobs(ui, id, &mut norm).changed)
    });
    assert!(!seen);
    assert!(changed);
    assert!((norm - 0.51).abs() < 1e-6, "{norm}");

    h.press(named(NamedKey::ArrowRight));
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert!((norm - 0.52).abs() < 1e-6);
    h.press(named(NamedKey::ArrowLeft));
    h.press(named(NamedKey::ArrowDown));
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert!((norm - 0.50).abs() < 1e-6);

    h.press(named(NamedKey::PageUp));
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert!((norm - 0.60).abs() < 1e-6);
    h.press(named(NamedKey::PageDown));
    h.press(named(NamedKey::PageDown));
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert!((norm - 0.40).abs() < 1e-6);

    // Shift is ten times finer.
    h.press_with(
        named(NamedKey::ArrowUp),
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert!((norm - 0.401).abs() < 1e-6, "{norm}");

    h.press(named(NamedKey::Home));
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert_eq!(norm, 0.0);
    h.press(named(NamedKey::ArrowDown));
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert_eq!(norm, 0.0, "clamped at the bottom");
    h.press(named(NamedKey::End));
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert_eq!(norm, 1.0);
    h.press(named(NamedKey::Backspace));
    h.frame(|ui| {
        knobs(ui, id, &mut norm);
    });
    assert_eq!(norm, 0.5, "Backspace resets to the default");
}

#[test]
fn faders_and_sliders_take_arrow_keys_too() {
    let mut h = Harness::new();
    let f = Id::new("f");
    let s = Id::new("s");
    let (mut fv, mut sv) = (0.5_f32, 0.5_f32);
    let draw = |ui: &mut Ui, fv: &mut f32, sv: &mut f32| {
        let c = ui.theme.accent;
        ui.fader(f, rect(10.0, 10.0, 40.0, 120.0), fv, 0.8, c);
        ui.hslider(s, rect(100.0, 10.0, 120.0, 20.0), sv, 0.5, c);
    };
    h.frame(|ui| draw(ui, &mut fv, &mut sv));
    h.tab();
    h.frame(|ui| draw(ui, &mut fv, &mut sv));
    assert!(h.ui.is_kb_focused(f));
    h.press(named(NamedKey::ArrowUp));
    h.frame(|ui| draw(ui, &mut fv, &mut sv));
    assert!((fv - 0.51).abs() < 1e-6);
    assert_eq!(sv, 0.5);
    h.tab();
    h.frame(|ui| draw(ui, &mut fv, &mut sv));
    h.press(named(NamedKey::ArrowLeft));
    h.frame(|ui| draw(ui, &mut fv, &mut sv));
    assert!((sv - 0.49).abs() < 1e-6);
    assert!((fv - 0.51).abs() < 1e-6);
    h.press(named(NamedKey::Backspace));
    h.frame(|ui| draw(ui, &mut fv, &mut sv));
    assert_eq!(sv, 0.5);
}

#[test]
fn number_fields_step_with_arrows_and_type_with_enter() {
    let mut h = Harness::new();
    let id = Id::new("bpm");
    let mut tempo = 120.0_f64;
    let draw = |ui: &mut Ui, tempo: &mut f64| {
        let t = ui.theme.clone();
        ui.drag_number(
            id,
            rect(10.0, 10.0, 80.0, 30.0),
            tempo,
            20.0,
            400.0,
            0.2,
            t.readout,
            t.text,
            |x| format!("{x:.1}"),
        )
    };
    h.frame(|ui| {
        draw(ui, &mut tempo);
    });
    h.press(named(NamedKey::ArrowUp));
    h.frame(|ui| {
        assert!(app_sees(ui, NamedKey::ArrowUp), "not focused yet");
        draw(ui, &mut tempo);
    });
    assert_eq!(tempo, 120.0);
    h.tab();
    h.frame(|ui| {
        draw(ui, &mut tempo);
    });
    h.press(named(NamedKey::ArrowUp));
    let changed = h.frame(|ui| draw(ui, &mut tempo).changed);
    assert!(changed);
    assert!((tempo - 121.0).abs() < 1e-9, "one step is five drag pixels");
    h.press(named(NamedKey::PageDown));
    h.frame(|ui| {
        draw(ui, &mut tempo);
    });
    assert!((tempo - 111.0).abs() < 1e-9);
    h.press(named(NamedKey::End));
    h.frame(|ui| {
        draw(ui, &mut tempo);
    });
    assert_eq!(tempo, 400.0);

    // Enter starts typing a value; Enter commits it.
    h.press(named(NamedKey::Enter));
    h.frame(|ui| {
        draw(ui, &mut tempo);
    });
    assert!(h.ui.text_focus());
    h.ui.input.text.push_str("90");
    h.frame(|ui| {
        draw(ui, &mut tempo);
    });
    h.press(named(NamedKey::Enter));
    let changed = h.frame(|ui| draw(ui, &mut tempo).changed);
    assert!(changed);
    assert_eq!(tempo, 90.0);
    assert!(!h.ui.text_focus());
    assert!(h.ui.is_kb_focused(id), "focus stays on the field");
}

#[test]
fn segmented_controls_are_a_radio_group() {
    let mut h = Harness::new();
    let id = Id::new("seg");
    let mut sel = 0_usize;
    let draw = |ui: &mut Ui, sel: &mut usize| {
        ui.segmented(id, rect(10.0, 10.0, 300.0, 28.0), &["A", "B", "C"], sel)
    };
    h.frame(|ui| {
        draw(ui, &mut sel);
    });
    h.tab();
    h.frame(|ui| {
        draw(ui, &mut sel);
    });
    assert!(
        h.ui.is_kb_focused(id.with(0usize)),
        "the selected option is the tab stop"
    );
    h.press(named(NamedKey::ArrowRight));
    let changed = h.frame(|ui| draw(ui, &mut sel));
    assert!(changed);
    assert_eq!(sel, 1);
    assert!(
        h.ui.is_kb_focused(id.with(1usize)),
        "focus follows the selection"
    );
    h.press(named(NamedKey::ArrowLeft));
    h.frame(|ui| {
        draw(ui, &mut sel);
    });
    h.press(named(NamedKey::ArrowLeft));
    h.frame(|ui| {
        draw(ui, &mut sel);
    });
    assert_eq!(sel, 2, "wraps");
    // Tab leaves the group in one step.
    h.tab();
    h.frame(|ui| {
        draw(ui, &mut sel);
    });
    assert!(
        h.ui.is_kb_focused(id.with(2usize)),
        "only one stop: wraps to itself"
    );
}

#[test]
fn splitters_move_with_the_arrow_keys() {
    let mut h = Harness::new();
    let id = Id::new("split");
    let draw = |ui: &mut Ui| ui.splitter(id, rect(200.0, 0.0, 6.0, 400.0), true);
    h.frame(|ui| {
        draw(ui);
    });
    h.tab();
    h.frame(|ui| {
        draw(ui);
    });
    h.press(named(NamedKey::ArrowRight));
    let d = h.frame(draw);
    assert_eq!(d, 16.0);
    h.press(named(NamedKey::ArrowLeft));
    assert_eq!(h.frame(draw), -16.0);
    assert_eq!(h.frame(draw), 0.0);
    let n = h.ui.accessibility_node(id).unwrap();
    assert_eq!(n.role(), Role::Splitter);
    assert_eq!(n.orientation(), Some(accesskit::Orientation::Vertical));
}

#[test]
fn thin_splitters_still_grab_beside_the_seam() {
    // Panes meet at a 2 px seam; the pointer can grab a few pixels to
    // either side of it.
    let mut h = Harness::new();
    let id = Id::new("split");
    let draw = |ui: &mut Ui| ui.splitter(id, rect(200.0, 0.0, 2.0, 400.0), true);
    h.ui.input.pointer = Some(vec2(198.0, 100.0));
    h.frame(draw);
    h.ui.input.pressed[0] = true;
    h.ui.input.down[0] = true;
    h.frame(draw);
    h.ui.input.pointer = Some(vec2(208.0, 100.0));
    h.ui.input.pointer_delta = vec2(10.0, 0.0);
    assert_eq!(h.frame(draw), 10.0);
    // Well away from the seam, nothing grabs.
    h.ui.input.down[0] = false;
    h.ui.input.released[0] = true;
    h.frame(draw);
    h.ui.input.pointer = Some(vec2(190.0, 100.0));
    h.frame(draw);
    h.ui.input.pressed[0] = true;
    h.ui.input.down[0] = true;
    h.frame(draw);
    h.ui.input.pointer_delta = vec2(10.0, 0.0);
    assert_eq!(h.frame(draw), 0.0);
}

#[test]
fn focus_requests_from_assistive_technology() {
    let mut h = Harness::new();
    let a = Id::new("a");
    let b = Id::new("b");
    let draw = |ui: &mut Ui| {
        ui.button(a, cell(0), "A");
        ui.button(b, cell(1), "B");
    };
    h.frame(draw);
    h.action(Action::Focus, b, None);
    h.frame(draw);
    assert!(h.ui.is_kb_focused(b));
    assert_eq!(h.focus_in_consumer(), Ui::accessibility_node_id(b));
    // Tab continues from where the screen reader put focus.
    h.tab();
    h.frame(draw);
    assert!(h.ui.is_kb_focused(a));
    h.action(Action::Blur, a, None);
    h.frame(draw);
    assert_eq!(h.ui.kb_focus(), None);
}

#[test]
fn click_requests_click_the_widget() {
    let mut h = Harness::new();
    let a = Id::new("a");
    let b = Id::new("b");
    let draw = |ui: &mut Ui| {
        let ra = ui.button(a, cell(0), "A").clicked;
        let rb = ui.button(b, cell(1), "B").clicked;
        (ra, rb)
    };
    h.frame(draw);
    h.action(Action::Click, b, None);
    assert_eq!(h.frame(draw), (false, true));
    assert_eq!(h.frame(draw), (false, false), "a request acts once");
    // A request nobody consumed does not wait around.
    h.action(Action::Click, Id::new("nobody"), None);
    h.frame(draw);
    assert_eq!(h.frame(draw), (false, false));
}

#[test]
fn increment_decrement_and_set_value_reach_value_widgets() {
    let mut h = Harness::new();
    let k = Id::new("k");
    let n = Id::new("n");
    let mut norm = 0.5_f32;
    let mut tempo = 120.0_f64;
    let draw = |ui: &mut Ui, norm: &mut f32, tempo: &mut f64| {
        let t = ui.theme.clone();
        let a = ui.knob(k, cell(0), norm, 0.5, t.accent, false).changed;
        let b = ui
            .drag_number(
                n,
                rect(100.0, 10.0, 80.0, 30.0),
                tempo,
                20.0,
                400.0,
                0.2,
                t.readout,
                t.text,
                |x| format!("{x:.1}"),
            )
            .changed;
        (a, b)
    };
    h.frame(|ui| {
        draw(ui, &mut norm, &mut tempo);
    });

    h.action(Action::Increment, k, None);
    let (a, b) = h.frame(|ui| draw(ui, &mut norm, &mut tempo));
    assert!(a && !b);
    assert!((norm - 0.51).abs() < 1e-6);
    h.action(Action::Decrement, k, None);
    h.action(Action::Decrement, k, None);
    h.frame(|ui| draw(ui, &mut norm, &mut tempo));
    assert!((norm - 0.49).abs() < 1e-6);

    // SetValue is in the node's units: percent for a knob.
    h.action(Action::SetValue, k, Some(ActionData::NumericValue(80.0)));
    h.frame(|ui| draw(ui, &mut norm, &mut tempo));
    assert!((norm - 0.8).abs() < 1e-6);
    h.action(Action::SetValue, k, Some(ActionData::NumericValue(500.0)));
    h.frame(|ui| draw(ui, &mut norm, &mut tempo));
    assert_eq!(norm, 1.0, "clamped");
    h.action(Action::SetValue, k, Some(ActionData::Value("25%".into())));
    h.frame(|ui| draw(ui, &mut norm, &mut tempo));
    assert!((norm - 0.25).abs() < 1e-6, "text values parse too");

    h.action(Action::Increment, n, None);
    let (_, b) = h.frame(|ui| draw(ui, &mut norm, &mut tempo));
    assert!(b);
    assert!((tempo - 121.0).abs() < 1e-9);
    h.action(Action::SetValue, n, Some(ActionData::NumericValue(98.5)));
    h.frame(|ui| draw(ui, &mut norm, &mut tempo));
    assert_eq!(tempo, 98.5);
    h.action(Action::SetValue, n, Some(ActionData::NumericValue(1.0)));
    h.frame(|ui| draw(ui, &mut norm, &mut tempo));
    assert_eq!(tempo, 20.0, "clamped to the range");

    // The tree reflects the new values.
    let node = h.ui.accessibility_node(n).unwrap();
    assert_eq!(node.numeric_value(), Some(20.0));
}

#[test]
fn text_fields_take_text_from_assistive_technology() {
    let mut h = Harness::new();
    let id = Id::new("t");
    let mut text = String::from("Kick");
    let draw = |ui: &mut Ui, text: &mut String| {
        ui.text_edit(id, rect(10.0, 10.0, 200.0, 30.0), text, "Name")
            .changed
    };
    h.frame(|ui| {
        draw(ui, &mut text);
    });
    h.action(
        Action::SetValue,
        id,
        Some(ActionData::Value("Snare".into())),
    );
    assert!(h.frame(|ui| draw(ui, &mut text)));
    assert_eq!(text, "Snare");
    assert_eq!(h.ui.accessibility_node(id).unwrap().value(), Some("Snare"));

    // Replacing the selection while not editing replaces everything.
    h.action(
        Action::ReplaceSelectedText,
        id,
        Some(ActionData::Value("Hat".into())),
    );
    h.frame(|ui| {
        draw(ui, &mut text);
    });
    assert_eq!(text, "Hat");
}

#[test]
fn tab_starts_editing_a_text_field_and_tab_moves_on() {
    let mut h = Harness::new();
    let t = Id::new("t");
    let b = Id::new("b");
    let mut text = String::from("Kick");
    let draw = |ui: &mut Ui, text: &mut String| {
        let c = ui.text_edit(t, rect(10.0, 10.0, 200.0, 30.0), text, "Name");
        ui.button(b, rect(250.0, 10.0, 60.0, 30.0), "OK");
        c.changed
    };
    h.frame(|ui| {
        draw(ui, &mut text);
    });
    // Space is for the app until a field is being edited.
    h.tab();
    h.frame(|ui| {
        draw(ui, &mut text);
    });
    assert!(h.ui.has_focus(t), "Tab into a field starts editing");
    assert!(h.ui.text_focus(), "so global shortcuts stand down");
    h.ui.input.text.push_str("Hi");
    h.frame(|ui| {
        draw(ui, &mut text);
    });
    assert_eq!(text, "Hi", "typing replaces the selected text");

    h.tab();
    let changed = h.frame(|ui| draw(ui, &mut text));
    assert!(changed, "Tab commits like Enter");
    assert!(!h.ui.text_focus());
    assert!(h.ui.is_kb_focused(b), "and moves on to the next stop");

    // Shift+Tab comes back and edits again.
    h.shift_tab();
    h.frame(|ui| {
        draw(ui, &mut text);
    });
    assert!(h.ui.has_focus(t));
}

#[test]
fn an_edited_field_keeps_enter_for_itself() {
    let mut h = Harness::new();
    let t = Id::new("t");
    let b = Id::new("b");
    let mut text = String::from("x");
    let mut clicks = 0;
    let draw = |ui: &mut Ui, text: &mut String, clicks: &mut i32| {
        ui.text_edit(t, rect(10.0, 10.0, 200.0, 30.0), text, "Name");
        if ui.button(b, rect(250.0, 10.0, 60.0, 30.0), "OK").clicked {
            *clicks += 1;
        }
    };
    h.frame(|ui| draw(ui, &mut text, &mut clicks));
    h.tab();
    h.frame(|ui| draw(ui, &mut text, &mut clicks));
    assert!(h.ui.has_focus(t));
    h.press(named(NamedKey::Enter));
    h.frame(|ui| draw(ui, &mut text, &mut clicks));
    assert_eq!(clicks, 0);
    assert!(!h.ui.text_focus());
    // After the commit Enter starts editing again.
    h.press(named(NamedKey::Enter));
    h.frame(|ui| draw(ui, &mut text, &mut clicks));
    assert!(h.ui.has_focus(t));
}

#[test]
fn menus_have_roles_and_work_from_the_keyboard() {
    let mut h = Harness::new();
    let opener = Id::new("open");
    let menu = Id::new("menu");
    let items = [
        MenuItem::new("Open").shortcut("Cmd+O"),
        MenuItem::Separator,
        MenuItem::new("Save").checked(true),
        MenuItem::new("Disabled").enabled(false),
        MenuItem::new("Quit"),
    ];
    let draw = |ui: &mut Ui| {
        if ui.button(opener, cell(0), "File").clicked {
            ui.open_popup(menu, cell(0));
        }
        ui.menu(menu, vec2(10.0, 60.0), &items)
    };
    h.frame(|ui| {
        draw(ui);
    });
    h.tab();
    h.frame(|ui| {
        draw(ui);
    });
    assert!(h.ui.is_kb_focused(opener));

    // Enter on the button opens the menu and lands on its first row.
    h.press(named(NamedKey::Enter));
    h.frame(|ui| {
        draw(ui);
    });
    assert!(h.ui.any_popup());
    h.frame(|ui| {
        draw(ui);
    });
    assert!(h.ui.is_kb_focused(menu.with(("item", 0usize))));

    // Roles: Menu with MenuItem rows; checked rows are check boxes.
    let tree = h.ui.accessibility_tree();
    let menu_node = tree
        .values()
        .find(|n| n.role() == Role::Menu)
        .expect("menu node");
    let kids: Vec<_> = menu_node.children().to_vec();
    let roles: Vec<Role> = kids.iter().map(|k| tree[k].role()).collect();
    assert_eq!(
        roles,
        vec![
            Role::MenuItem,
            Role::MenuItemCheckBox,
            Role::MenuItem,
            Role::MenuItem
        ]
    );
    let open = &tree[&kids[0]];
    assert_eq!(open.label(), Some("Open"));
    assert_eq!(open.keyboard_shortcut(), Some("Cmd+O"));
    assert!(open.supports_action(Action::Click));
    assert_eq!(tree[&kids[1]].toggled(), Some(Toggled::True));
    assert!(tree[&kids[2]].is_disabled());
    // The menu is a popup, so it comes after the window's own content.
    let root = &tree[&NodeId(0)];
    assert_eq!(*root.children().last().unwrap(), kids_parent(tree, kids[0]));

    // Tab stays inside the open menu; the disabled row is skipped.
    h.press(named(NamedKey::ArrowDown));
    h.frame(|ui| {
        draw(ui);
    });
    assert!(h.ui.is_kb_focused(menu.with(("item", 2usize))));
    h.press(named(NamedKey::ArrowDown));
    h.frame(|ui| {
        draw(ui);
    });
    assert!(h.ui.is_kb_focused(menu.with(("item", 4usize))));
    h.tab();
    h.frame(|ui| {
        draw(ui);
    });
    assert!(
        h.ui.is_kb_focused(menu.with(("item", 0usize))),
        "Tab wraps inside the menu"
    );
    h.press(named(NamedKey::End));
    h.frame(|ui| {
        draw(ui);
    });
    assert!(h.ui.is_kb_focused(menu.with(("item", 4usize))));

    // Enter picks the row, closes the menu and returns focus to the opener.
    h.press(named(NamedKey::Enter));
    let picked = h.frame(draw);
    assert_eq!(picked, Some(4));
    assert!(!h.ui.any_popup());
    assert!(h.ui.is_kb_focused(opener));
}

fn kids_parent(tree: &std::collections::HashMap<NodeId, accesskit::Node>, child: NodeId) -> NodeId {
    *tree
        .iter()
        .find(|(_, n)| n.children().contains(&child))
        .map(|(id, _)| id)
        .expect("parent")
}

#[test]
fn escape_closes_a_menu_and_gives_focus_back() {
    let mut h = Harness::new();
    let opener = Id::new("open");
    let menu = Id::new("menu");
    let items = [MenuItem::new("One"), MenuItem::new("Two")];
    let draw = |ui: &mut Ui| {
        if ui.button(opener, cell(0), "File").clicked {
            ui.open_popup(menu, cell(0));
        }
        ui.menu(menu, vec2(10.0, 60.0), &items);
    };
    h.frame(draw);
    h.tab();
    h.frame(draw);
    h.press(named(NamedKey::Space));
    h.frame(draw);
    h.frame(draw);
    assert!(h.ui.any_popup());
    assert!(h.ui.is_kb_focused(menu.with(("item", 0usize))));
    h.press(named(NamedKey::Escape));
    h.frame(draw);
    assert!(!h.ui.any_popup());
    h.frame(draw);
    assert!(h.ui.is_kb_focused(opener));
}

#[test]
fn a_menu_opened_with_the_mouse_does_not_take_keyboard_focus() {
    let mut h = Harness::new();
    let menu = Id::new("menu");
    let items = [MenuItem::new("One")];
    h.frame(|ui| {
        ui.open_popup(menu, rect(10.0, 10.0, 100.0, 40.0));
        ui.menu(menu, vec2(10.0, 60.0), &items);
    });
    h.frame(|ui| {
        ui.menu(menu, vec2(10.0, 60.0), &items);
    });
    assert_eq!(h.ui.kb_focus(), None);
}

#[test]
fn popup_panels_are_dialogs_that_own_tab() {
    let mut h = Harness::new();
    let base = Id::new("base");
    let panel = Id::new("panel");
    let inner = Id::new("inner");
    let draw = |ui: &mut Ui| {
        ui.button(base, cell(0), "Base");
        ui.popup_panel(panel, rect(100.0, 100.0, 200.0, 100.0), |ui, r| {
            ui.button(inner, r.shrink(10.0), "Inside");
        });
    };
    h.frame(|ui| {
        ui.open_popup(panel, rect(100.0, 100.0, 200.0, 100.0));
        draw(ui);
    });
    h.frame(draw);
    h.tab();
    h.frame(draw);
    assert!(h.ui.is_kb_focused(inner), "the open popup traps Tab");
    h.tab();
    h.frame(draw);
    assert!(h.ui.is_kb_focused(inner));
    let tree = h.ui.accessibility_tree();
    let dialog = tree
        .values()
        .find(|n| n.role() == Role::Dialog)
        .expect("dialog");
    assert_eq!(dialog.children().len(), 1);
    assert_eq!(tree[&dialog.children()[0]].label(), Some("Inside"));
}

#[test]
fn meter_values_are_throttled() {
    let mut h = Harness::new();
    let id = Id::new("m");
    let value = |h: &Harness| {
        h.ui.accessibility_node(id)
            .unwrap()
            .value()
            .map(str::to_string)
    };
    let draw = |ui: &mut Ui, peak: f32| ui.meter(id, rect(10.0, 10.0, 20.0, 100.0), (peak, peak));
    h.frame(|ui| draw(ui, 0.5));
    assert_eq!(value(&h).as_deref(), Some("-6 dB"));
    let updates = h.updates.len();
    // A level that moves around for half a second changes nothing.
    for i in 0..30 {
        let peak = if i % 2 == 0 { 0.1 } else { 0.9 };
        h.frame(|ui| draw(ui, peak));
    }
    assert_eq!(value(&h).as_deref(), Some("-6 dB"));
    assert_eq!(
        h.updates.len(),
        updates,
        "no updates sent for meter chatter"
    );
    // After a second it reports the new level, quantized to 3 dB.
    for _ in 0..60 {
        h.frame(|ui| draw(ui, 0.25));
    }
    assert_eq!(value(&h).as_deref(), Some("-12 dB"));
    // Silence is spoken as such.
    for _ in 0..80 {
        h.frame(|ui| draw(ui, 0.0));
    }
    assert_eq!(value(&h).as_deref(), Some("silent"));
}

#[test]
fn announcements_become_a_live_region() {
    let mut h = Harness::new();
    h.frame(|_| {});
    h.ui.announce("Exported 3 files");
    h.frame(|_| {});
    let tree = h.ui.accessibility_tree();
    let live = tree
        .values()
        .find(|n| n.role() == Role::Status)
        .expect("status node");
    assert_eq!(live.label(), Some("Exported 3 files"));
    assert_eq!(live.live(), Some(accesskit::Live::Polite));
    // It goes away after a while.
    for _ in 0..(60 * 7) {
        h.frame(|_| {});
    }
    assert!(
        h.ui.accessibility_tree()
            .values()
            .all(|n| n.role() != Role::Status)
    );
}

#[test]
fn the_window_node_carries_the_title() {
    let mut h = Harness::new();
    h.ui.set_window_label("Peel Studio - Untitled");
    h.frame(|_| {});
    let root = &h.ui.accessibility_tree()[&NodeId(0)];
    assert_eq!(root.label(), Some("Peel Studio - Untitled"));
}

#[test]
fn custom_widgets_can_take_focus_and_actions() {
    let mut h = Harness::new();
    let id = Id::new("canvas");
    let r = rect(10.0, 10.0, 200.0, 100.0);
    let draw = |ui: &mut Ui| {
        let resp = ui.interact(id, r);
        ui.focusable(id, r, FocusKind::Custom);
        ui.focus_ring(id, r, 4.0);
        ui.describe_node(
            id,
            Semantics::new(Role::Canvas)
                .label("Piano roll")
                .rect(r)
                .action(Action::ScrollDown),
        );
        let scrolled = ui.take_action(id, Action::ScrollDown).is_some();
        (resp.focused, resp.clicked, scrolled)
    };
    h.frame(draw);
    h.action(Action::Focus, id, None);
    assert_eq!(h.frame(draw), (true, false, false));
    // Custom stops claim no keys: the canvas reads them itself.
    h.press(named(NamedKey::ArrowRight));
    h.frame(|ui| {
        assert!(app_sees(ui, NamedKey::ArrowRight));
        draw(ui);
    });
    h.action(Action::ScrollDown, id, None);
    assert_eq!(h.frame(draw), (true, false, true));
    let n = h.ui.accessibility_node(id).unwrap();
    assert!(n.supports_action(Action::ScrollDown));
    assert!(n.supports_action(Action::Focus));
    assert!(!n.supports_action(Action::Click));
}

#[test]
fn keyboard_focus_dropped_when_its_widget_disappears() {
    let mut h = Harness::new();
    let a = Id::new("a");
    let draw = |ui: &mut Ui, show: bool| {
        if show {
            ui.button(a, cell(0), "A");
        }
    };
    h.frame(|ui| draw(ui, true));
    h.tab();
    h.frame(|ui| draw(ui, true));
    assert!(h.ui.is_kb_focused(a));
    h.frame(|ui| draw(ui, false));
    assert_eq!(
        h.ui.kb_focus(),
        None,
        "keys are not held for a widget that is gone"
    );
    h.press(named(NamedKey::Space));
    h.frame(|ui| {
        assert!(app_sees(ui, NamedKey::Space));
        draw(ui, true);
    });
}

#[test]
fn the_consumer_accepts_a_long_sequence_of_changing_trees() {
    // Widgets appear, move between parents, disappear and reappear; the
    // AccessKit consumer panics on any inconsistent update.
    let mut h = Harness::new();
    let g1 = Id::new("g1");
    let g2 = Id::new("g2");
    for step in 0..12_usize {
        h.frame(|ui| {
            let c = ui.theme.accent;
            let mut norm = 0.1 * step as f32;
            ui.describe_group(g1, Semantics::new(Role::Group).label("One"), |ui| {
                if step % 2 == 0 {
                    ui.button(Id::new("moving"), cell(0), "Moving");
                }
                ui.knob(Id::new("k"), cell(1), &mut norm, 0.5, c, false);
            });
            ui.describe_group(g2, Semantics::new(Role::Group).label("Two"), |ui| {
                if step % 2 == 1 {
                    ui.button(Id::new("moving"), cell(0), "Moving");
                }
                if step % 3 != 0 {
                    ui.button(Id::new("sometimes"), cell(2), "Sometimes");
                }
            });
        });
        if step % 4 == 3 {
            h.tab();
        }
    }
    assert!(h.updates.len() > 3);
}

#[test]
fn scroll_areas_scroll_for_assistive_technology() {
    use crate::vec2;
    let mut h = Harness::new();
    let id = Id::new("list");
    let view = rect(0.0, 0.0, 200.0, 300.0);
    let mut offset = vec2(0.0, 0.0);
    let content = vec2(200.0, 1000.0);
    h.frame(|ui| ui.scroll_area(id, view, content, &mut offset));
    let n = h.ui.accessibility_node(id).unwrap();
    assert_eq!(n.role(), Role::ScrollView);
    assert_eq!(n.scroll_y(), Some(0.0));
    assert_eq!(n.scroll_y_max(), Some(700.0));
    assert!(n.supports_action(Action::ScrollDown));
    assert!(
        !n.supports_action(Action::ScrollRight),
        "nothing to scroll sideways"
    );

    h.action(
        Action::ScrollDown,
        id,
        Some(ActionData::ScrollUnit(accesskit::ScrollUnit::Item)),
    );
    h.frame(|ui| ui.scroll_area(id, view, content, &mut offset));
    assert_eq!(offset.y, 36.0);
    h.action(
        Action::ScrollDown,
        id,
        Some(ActionData::ScrollUnit(accesskit::ScrollUnit::Page)),
    );
    h.frame(|ui| ui.scroll_area(id, view, content, &mut offset));
    assert_eq!(offset.y, 36.0 + 270.0);
    h.action(Action::ScrollUp, id, None);
    h.frame(|ui| ui.scroll_area(id, view, content, &mut offset));
    assert_eq!(offset.y, 270.0);
    h.action(
        Action::SetScrollOffset,
        id,
        Some(ActionData::SetScrollOffset(accesskit::Point::new(
            0.0, 5000.0,
        ))),
    );
    h.frame(|ui| ui.scroll_area(id, view, content, &mut offset));
    assert_eq!(offset.y, 700.0, "clamped to the content");
    assert_eq!(h.ui.accessibility_node(id).unwrap().scroll_y(), Some(700.0));
}

#[test]
fn visible_text_can_be_described() {
    let mut h = Harness::new();
    let id = Id::new("heading");
    h.frame(|ui| ui.describe_label(id, rect(10.0, 10.0, 100.0, 20.0), "Mixer"));
    let n = h.ui.accessibility_node(id).unwrap();
    assert_eq!(n.role(), Role::Label);
    assert_eq!(n.label(), Some("Mixer"));
}

#[test]
fn pointer_use_never_shows_a_focus_ring_or_takes_keys() {
    // Clicking around with the mouse leaves keyboard focus alone, so the
    // app's shortcuts keep working afterwards.
    let mut h = Harness::new();
    let id = Id::new("b");
    let draw = |ui: &mut Ui| ui.button(id, cell(0), "Go");
    h.frame(draw);
    let p = cell(0).center();
    h.ui.input.pointer = Some(p);
    h.frame(draw);
    h.ui.input.pressed[0] = true;
    h.ui.input.down[0] = true;
    h.frame(draw);
    h.ui.input.released[0] = true;
    h.ui.input.down[0] = false;
    let r = h.frame(draw);
    assert!(r.clicked);
    assert_eq!(h.ui.kb_focus(), None);
    h.press(named(NamedKey::Space));
    h.frame(|ui| {
        assert!(app_sees(ui, NamedKey::Space));
        draw(ui);
    });
}

#[test]
fn custom_drawn_buttons_join_the_keyboard_and_the_tree() {
    let mut h = Harness::new();
    let id = Id::new("tab");
    let r = cell(0);
    let draw = |ui: &mut Ui| {
        let resp = ui.interact_button(id, r, "Mixer");
        ui.scene.rect(r, 4.0, ui.theme.control);
        ui.focus_ring(id, r, 4.0);
        resp.clicked
    };
    h.frame(draw);
    let n = h.ui.accessibility_node(id).unwrap();
    assert_eq!(n.role(), Role::Button);
    assert_eq!(n.label(), Some("Mixer"));
    assert!(n.supports_action(Action::Click));
    h.tab();
    h.frame(draw);
    assert!(h.ui.is_kb_focused(id));
    h.press(named(NamedKey::Enter));
    assert!(h.frame(draw));
    h.action(Action::Click, id, None);
    assert!(h.frame(draw));

    // Full control for other kinds of widget.
    let tab = Id::new("tab2");
    let selected = true;
    h.frame(|ui| {
        ui.interact_node(
            tab,
            cell(1),
            FocusKind::Activate,
            Semantics::new(Role::Tab)
                .label("Piano roll")
                .selected(selected),
        );
    });
    let n = h.ui.accessibility_node(tab).unwrap();
    assert_eq!(n.role(), Role::Tab);
    assert_eq!(n.is_selected(), Some(true));
}
