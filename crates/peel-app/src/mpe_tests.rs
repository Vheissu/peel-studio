//! Interaction tests for note expression in the Piano Roll and the MPE
//! controls in the instrument editor: real frames, scripted pointer and keys.

use peel_document::{ExpressionPoint, Id, Note, NoteExpression, Severity, Time};
use peel_session::{MpeConfig, MpeMode, Session, template};
use peel_ui::{App, Id as W, Key, KeyCode, NamedKey, Rect, Ui, Vec2, WindowControl, vec2};

use crate::note_expr::Lane;
use crate::studio::Studio;

struct H {
    ui: Ui,
    studio: Studio,
    win: WindowControl,
    time: f64,
}

impl H {
    fn new() -> H {
        let mut h = H {
            ui: Ui::new(),
            studio: Studio::new(Session::new_silent(template::beat_and_melody())),
            win: WindowControl::headless(),
            time: 0.0,
        };
        h.frames(2);
        h
    }

    fn frame(&mut self) {
        self.time += 1.0 / 60.0;
        let screen = Rect {
            x: 0.0,
            y: 0.0,
            w: 1440.0,
            h: 900.0,
        };
        self.ui.begin_frame(screen, 1.0, self.time);
        self.studio.update(&mut self.ui, &self.win);
        self.ui.end_frame();
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame();
        }
    }

    fn rect(&self, id: W) -> Rect {
        self.ui
            .widget_rect(id)
            .unwrap_or_else(|| panic!("widget {id:?} not on screen"))
    }

    fn shown(&self, id: W) -> bool {
        self.ui.widget_rect(id).is_some()
    }

    fn move_to(&mut self, p: Vec2) {
        let old = self.ui.input.pointer.unwrap_or(p);
        self.ui.input.pointer = Some(p);
        self.ui.input.pointer_delta = p - old;
        self.ui.input.had_event = true;
        self.frame();
    }

    fn press(&mut self, b: usize) {
        self.ui.input.down[b] = true;
        self.ui.input.pressed[b] = true;
        self.ui.input.press_pos[b] = self.ui.input.pointer.unwrap_or_default();
        self.frame();
    }

    fn release(&mut self, b: usize) {
        self.ui.input.down[b] = false;
        self.ui.input.released[b] = true;
        self.frame();
    }

    fn click_at(&mut self, p: Vec2) {
        self.move_to(p);
        self.move_to(p);
        self.press(0);
        self.release(0);
    }

    fn click(&mut self, id: W) {
        let c = self.rect(id).center();
        self.click_at(c);
    }

    fn right_click_at(&mut self, p: Vec2) {
        self.move_to(p);
        self.move_to(p);
        self.press(1);
        self.release(1);
    }

    fn drag(&mut self, from: Vec2, to: Vec2, steps: usize) {
        self.move_to(from);
        self.move_to(from);
        self.press(0);
        for i in 1..=steps {
            self.move_to(from + (to - from) * (i as f32 / steps as f32));
        }
        self.release(0);
    }

    fn key(&mut self, key: Key, code: KeyCode, command: bool) {
        let mods = peel_ui::Modifiers {
            logo: command && cfg!(target_os = "macos"),
            ctrl: command && !cfg!(target_os = "macos"),
            ..Default::default()
        };
        self.ui.input.modifiers = mods;
        self.ui.input.keys.push(peel_ui::input::KeyPress {
            key,
            code: Some(code),
            repeat: false,
            modifiers: mods,
            consumed: false,
        });
        self.frame();
        self.ui.input.modifiers = peel_ui::Modifiers::default();
    }

    fn command(&mut self, ch: &str, code: KeyCode) {
        self.key(Key::Character(ch.into()), code, true);
    }

    fn named(&mut self, key: NamedKey, code: KeyCode) {
        self.key(Key::Named(key), code, false);
    }

    fn session(&self) -> &Session {
        &self.studio.session
    }

    /// Opens the Piano Roll on the Piano channel with two notes on the same
    /// key, and returns (pattern, first, second).
    fn roll(&mut self) -> (Id, Note, Note) {
        let piano = self
            .session()
            .project()
            .channels
            .iter()
            .find(|c| c.name == "Piano")
            .unwrap()
            .id;
        let pid = self.session().selected_pattern();
        let a = Note::new(piano, Time::beats(0), Time::beats(2), 60, 0.8);
        let b = Note::new(piano, Time::beats(2), Time::beats(2), 60, 0.8);
        self.studio.session.doc.edit("setup", |p| {
            let pt = p.pattern_mut(pid).unwrap();
            pt.notes.retain(|n| n.channel != piano);
            pt.notes.extend([a, b]);
            pt.sort_notes();
            pt.expression.clear();
        });
        self.studio.ws.selected_channel = Some(piano);
        self.click(W::new(("dock-tab", "Piano roll")));
        self.studio.ws.roll.scroll_beats = 0.0;
        self.studio.ws.roll.zoom_x = 120.0;
        self.studio.ws.roll.top_key = 72.0;
        self.frames(2);
        (pid, a, b)
    }

    fn expr(&self, pid: Id, note: Id) -> Option<NoteExpression> {
        self.session()
            .project()
            .pattern(pid)
            .unwrap()
            .expression_for(note)
            .cloned()
    }

    fn curve(&self, pid: Id, note: Id, lane: Lane) -> Vec<ExpressionPoint> {
        self.expr(pid, note)
            .map(|e| crate::note_expr::curve(&e, lane).to_vec())
            .unwrap_or_default()
    }

    fn valid(&self) {
        let errors: Vec<_> = peel_document::validate(self.session().project())
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    /// Screen x of a beat in the roll.
    fn x(&self, beat: f64) -> f32 {
        let c = self.rect(W::new("roll-canvas"));
        c.x + (beat as f32 - self.studio.ws.roll.scroll_beats) * self.studio.ws.roll.zoom_x
    }

    fn note_center(&self, n: &Note) -> Vec2 {
        let c = self.rect(W::new("roll-canvas"));
        let y =
            c.y + (self.studio.ws.roll.top_key - n.key as f32 + 0.5) * self.studio.ws.roll.row_h;
        vec2(self.x(n.start.as_f64() + 0.25), y)
    }

    fn pick_lane(&mut self, lane: Lane) {
        let i = Lane::ALL.iter().position(|l| *l == lane).unwrap();
        self.click(W::new("roll-lane").with(i));
        assert_eq!(self.studio.ws.roll.lane, lane);
    }
}

fn pt(beats: f64, value: f32) -> ExpressionPoint {
    ExpressionPoint {
        offset: Time::from_f64(beats, 960),
        value,
    }
}

#[test]
fn lane_picker_switches_lanes_by_mouse_and_keyboard() {
    let mut h = H::new();
    h.roll();
    assert_eq!(h.studio.ws.roll.lane, Lane::Velocity);
    assert!(h.shown(W::new("roll-vel")));
    h.pick_lane(Lane::Pitch);
    h.frame();
    assert!(h.shown(W::new("roll-expr")) && !h.shown(W::new("roll-vel")));
    // Arrow keys move through the lanes once the picker has focus.
    h.ui.request_kb_focus(W::new("roll-lane").with(1usize));
    h.frame();
    h.named(NamedKey::ArrowDown, KeyCode::ArrowDown);
    assert_eq!(h.studio.ws.roll.lane, Lane::Pressure);
    h.named(NamedKey::ArrowDown, KeyCode::ArrowDown);
    assert_eq!(h.studio.ws.roll.lane, Lane::Timbre);
    h.ui.clear_kb_focus();
    h.pick_lane(Lane::Velocity);
    h.frame();
    assert!(h.shown(W::new("roll-vel")));
}

#[test]
fn velocity_lane_still_edits_velocity() {
    let mut h = H::new();
    let (pid, a, _) = h.roll();
    let lane = h.rect(W::new("roll-vel"));
    let x = h.x(a.start.as_f64());
    h.drag(vec2(x, lane.y + 12.0), vec2(x, lane.y + 14.0), 2);
    let v = h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes
        .iter()
        .find(|n| n.id == a.id)
        .unwrap()
        .velocity;
    assert!(v > 0.85, "{v}");
}

#[test]
fn points_insert_drag_delete_and_clear_with_undo() {
    let mut h = H::new();
    let (pid, a, b) = h.roll();
    // Focus the first note, then edit its pitch.
    h.click_at(h.note_center(&a));
    h.pick_lane(Lane::Pitch);
    h.frame();
    let lane = h.rect(W::new("roll-expr"));
    let inner_top = lane.y + 8.0;
    let inner_h = lane.h - 16.0;
    // Auto range is ±2 st with nothing recorded: a quarter down from the
    // top is +1 st.
    let at = vec2(h.x(1.0), inner_top + inner_h * 0.25);
    h.click_at(at);
    let c = h.curve(pid, a.id, Lane::Pitch);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].offset, Time::beats(1));
    assert!((c[0].value - 1.0).abs() < 0.05, "{c:?}");
    assert!(
        h.curve(pid, b.id, Lane::Pitch).is_empty(),
        "same-key note changed"
    );
    assert_eq!(h.session().doc.undo_label(), Some("Add pitch point"));

    // Drag it down to -1 st and later in the note: one undo step.
    let from = vec2(h.x(1.0), inner_top + inner_h * 0.25);
    let to = vec2(h.x(1.5), inner_top + inner_h * 0.75);
    h.drag(from, to, 6);
    let c = h.curve(pid, a.id, Lane::Pitch);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].offset, Time::new(3, 2));
    assert!((c[0].value + 1.0).abs() < 0.05, "{c:?}");
    assert_eq!(h.session().doc.undo_label(), Some("Move pitch point"));
    h.command("z", KeyCode::KeyZ);
    assert_eq!(h.curve(pid, a.id, Lane::Pitch)[0].offset, Time::beats(1));
    h.command("z", KeyCode::KeyZ);
    assert!(h.curve(pid, a.id, Lane::Pitch).is_empty());
    h.command("z", KeyCode::KeyZ);
    h.valid();

    // Two points, then right-click one away.
    h.studio.session.doc.edit("curve", |p| {
        crate::note_expr::set_curve(
            p.pattern_mut(pid).unwrap(),
            a.id,
            Lane::Pitch,
            &[pt(0.0, 0.0), pt(1.0, 2.0)],
        );
    });
    h.frame();
    h.right_click_at(vec2(h.x(1.0), inner_top));
    assert_eq!(h.curve(pid, a.id, Lane::Pitch), vec![pt(0.0, 0.0)]);
    assert_eq!(h.session().doc.undo_label(), Some("Delete pitch point"));
    // Delete over a point removes it too, not the selected notes.
    h.move_to(vec2(h.x(0.0) + 1.0, inner_top + inner_h * 0.5));
    h.named(NamedKey::Delete, KeyCode::Delete);
    assert!(h.curve(pid, a.id, Lane::Pitch).is_empty());
    assert_eq!(
        h.session()
            .project()
            .pattern(pid)
            .unwrap()
            .notes_for(a.channel)
            .count(),
        2
    );

    // Clear by keyboard: one step, undoable.
    h.studio.session.doc.edit("curve", |p| {
        crate::note_expr::set_curve(
            p.pattern_mut(pid).unwrap(),
            a.id,
            Lane::Pitch,
            &[pt(0.5, 1.5), pt(1.0, -0.5)],
        );
    });
    h.frame();
    h.ui.request_kb_focus(W::new("roll-expr-clear"));
    h.frame();
    h.named(NamedKey::Enter, KeyCode::Enter);
    assert!(h.curve(pid, a.id, Lane::Pitch).is_empty());
    assert_eq!(h.session().doc.undo_label(), Some("Clear pitch"));
    h.command("z", KeyCode::KeyZ);
    assert_eq!(h.curve(pid, a.id, Lane::Pitch).len(), 2);
    h.valid();
}

#[test]
fn points_stay_inside_the_note_and_between_neighbours() {
    let mut h = H::new();
    let (pid, a, _) = h.roll();
    h.studio.session.doc.edit("curve", |p| {
        crate::note_expr::set_curve(
            p.pattern_mut(pid).unwrap(),
            a.id,
            Lane::Pressure,
            &[pt(0.5, 0.2), pt(1.0, 0.4), pt(1.5, 0.6)],
        );
    });
    h.studio.ws.roll.selection = std::iter::once(a.id).collect();
    h.pick_lane(Lane::Pressure);
    h.frame();
    let lane = h.rect(W::new("roll-expr"));
    let y = |v: f32| lane.y + 8.0 + (lane.h - 16.0) * (1.0 - v);
    // Drag the middle point far right and above the top: it stops just
    // before its neighbour, at 100%.
    h.drag(vec2(h.x(1.0), y(0.4)), vec2(h.x(3.5), lane.y - 40.0), 6);
    let c = h.curve(pid, a.id, Lane::Pressure);
    assert_eq!(c.len(), 3);
    assert!(
        c[1].offset < c[2].offset && c[1].offset > c[0].offset,
        "{c:?}"
    );
    assert_eq!(c[1].value, 1.0);
    // The last point cannot leave the note.
    h.drag(vec2(h.x(1.5), y(0.6)), vec2(h.x(6.0), y(0.6)), 6);
    let c = h.curve(pid, a.id, Lane::Pressure);
    assert!(c[2].offset <= a.length, "{c:?}");
    h.valid();
}

#[test]
fn clicking_another_note_in_the_lane_focuses_it() {
    let mut h = H::new();
    let (pid, a, b) = h.roll();
    h.studio.session.doc.edit("curve", |p| {
        crate::note_expr::set_curve(
            p.pattern_mut(pid).unwrap(),
            b.id,
            Lane::Timbre,
            &[pt(0.0, 0.9)],
        );
    });
    h.studio.ws.roll.selection = std::iter::once(a.id).collect();
    h.pick_lane(Lane::Timbre);
    h.frame();
    let lane = h.rect(W::new("roll-expr"));
    h.click_at(vec2(h.x(3.0), lane.center().y));
    assert_eq!(
        h.studio
            .ws
            .roll
            .selection
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![b.id]
    );
    // That click only picked the note.
    assert_eq!(h.curve(pid, b.id, Lane::Timbre), vec![pt(0.0, 0.9)]);
    assert!(h.expr(pid, a.id).is_none());
}

#[test]
fn copy_paste_duplicate_and_delete_keep_curves_with_their_notes() {
    let mut h = H::new();
    let (pid, a, b) = h.roll();
    let curve = vec![pt(0.0, 3.0), pt(1.0, -2.0)];
    let c2 = curve.clone();
    h.studio.session.doc.edit("curve", |p| {
        crate::note_expr::set_curve(p.pattern_mut(pid).unwrap(), a.id, Lane::Pitch, &c2);
    });
    h.studio.ws.roll.selection = std::iter::once(a.id).collect();
    h.frame();
    let notes = |h: &H| {
        h.session()
            .project()
            .pattern(pid)
            .unwrap()
            .notes_for(a.channel)
            .copied()
            .collect::<Vec<_>>()
    };
    h.move_to(vec2(5.0, 5.0));
    h.command("c", KeyCode::KeyC);
    h.command("v", KeyCode::KeyV);
    let pasted: Vec<Note> = notes(&h)
        .into_iter()
        .filter(|n| n.id != a.id && n.id != b.id)
        .collect();
    assert_eq!(pasted.len(), 1);
    assert_eq!(h.curve(pid, pasted[0].id, Lane::Pitch), curve);
    assert_eq!(h.curve(pid, a.id, Lane::Pitch), curve);
    // Duplicate the paste.
    h.command("d", KeyCode::KeyD);
    let dup: Vec<Note> = notes(&h)
        .into_iter()
        .filter(|n| ![a.id, b.id, pasted[0].id].contains(&n.id))
        .collect();
    assert_eq!(dup.len(), 1);
    assert_eq!(h.curve(pid, dup[0].id, Lane::Pitch), curve);
    h.valid();
    // Deleting the duplicate removes its curve; undo brings both back.
    h.named(NamedKey::Delete, KeyCode::Delete);
    assert!(h.expr(pid, dup[0].id).is_none());
    h.valid();
    h.command("z", KeyCode::KeyZ);
    assert_eq!(h.curve(pid, dup[0].id, Lane::Pitch), curve);
    // Cut takes the curve along to the next paste.
    h.studio.ws.roll.selection = std::iter::once(a.id).collect();
    h.frame();
    h.command("x", KeyCode::KeyX);
    assert!(h.expr(pid, a.id).is_none());
    h.valid();
    h.command("v", KeyCode::KeyV);
    let back: Vec<Note> = notes(&h)
        .into_iter()
        .filter(|n| h.studio.ws.roll.selection.contains(&n.id))
        .collect();
    assert_eq!(h.curve(pid, back[0].id, Lane::Pitch), curve);
    h.valid();
}

#[test]
fn resizing_trims_curves_and_lengthening_in_the_same_drag_restores_them() {
    let mut h = H::new();
    let (pid, a, _) = h.roll();
    h.studio.session.doc.edit("curve", |p| {
        crate::note_expr::set_curve(
            p.pattern_mut(pid).unwrap(),
            a.id,
            Lane::Pitch,
            &[pt(0.0, 1.0), pt(1.5, 2.0)],
        );
    });
    h.studio.ws.roll.selection = std::iter::once(a.id).collect();
    h.frame();
    let y = h.note_center(&a).y;
    let edge = h.x(a.end().as_f64()) - 3.0;
    // Shorten to one beat, then back to two in one gesture.
    h.move_to(vec2(edge, y));
    h.move_to(vec2(edge, y));
    h.press(0);
    for x in [edge - 60.0, h.x(1.0) - 3.0] {
        h.move_to(vec2(x, y));
    }
    assert_eq!(h.curve(pid, a.id, Lane::Pitch), vec![pt(0.0, 1.0)]);
    h.move_to(vec2(edge, y));
    h.release(0);
    assert_eq!(
        h.curve(pid, a.id, Lane::Pitch),
        vec![pt(0.0, 1.0), pt(1.5, 2.0)]
    );
    h.valid();
}

#[test]
fn erasing_notes_removes_their_curves() {
    let mut h = H::new();
    let (pid, a, b) = h.roll();
    h.studio.session.doc.edit("curve", |p| {
        let pt = p.pattern_mut(pid).unwrap();
        crate::note_expr::set_curve(pt, a.id, Lane::Pitch, &[self::pt(0.0, 1.0)]);
        crate::note_expr::set_curve(pt, b.id, Lane::Pitch, &[self::pt(0.0, 2.0)]);
    });
    h.frame();
    h.right_click_at(h.note_center(&b));
    assert!(h.expr(pid, b.id).is_none());
    assert!(h.expr(pid, a.id).is_some());
    h.valid();
}

#[test]
fn note_tools_carry_curves_through_chop() {
    let mut h = H::new();
    let (pid, a, b) = h.roll();
    h.studio.session.doc.edit("curve", |p| {
        crate::note_expr::set_curve(
            p.pattern_mut(pid).unwrap(),
            a.id,
            Lane::Pitch,
            &[pt(0.0, 1.0), pt(1.0, 5.0)],
        );
    });
    h.studio.ws.roll.selection = std::iter::once(a.id).collect();
    h.frame();
    // Chop (tool 2) in quarters.
    crate::note_tools::select(&mut h.ui, 2);
    h.click(W::new("roll-note-tools"));
    h.frames(2);
    h.click(W::new("note-tool-apply"));
    h.frames(2);
    let pattern = h.session().project().pattern(pid).unwrap().clone();
    let pieces: Vec<&Note> = pattern
        .notes_for(a.channel)
        .filter(|n| n.id != b.id && n.start < Time::beats(2))
        .collect();
    assert!(pieces.len() > 1, "chop made {} notes", pieces.len());
    let late = pieces.iter().find(|n| n.start >= Time::beats(1)).unwrap();
    assert_eq!(
        pattern
            .expression_for(late.id)
            .unwrap()
            .at(Time::ZERO)
            .pitch,
        5.0
    );
    h.valid();
}

fn open_instrument(h: &mut H) {
    let ch = h
        .session()
        .project()
        .channels
        .iter()
        .find(|c| c.name == "Piano")
        .unwrap()
        .id;
    crate::instrument_editor::open(&mut h.studio.session, &mut h.studio.ws, ch);
    h.frames(3);
}

fn pick(h: &mut H, id: &str, item: usize) {
    h.click(W::new(id));
    h.frames(2);
    h.click(W::new(id).with("menu").with(("item", item)));
}

#[test]
fn mpe_controls_reach_the_session_and_preferences() {
    let mut h = H::new();
    open_instrument(&mut h);
    assert_eq!(h.session().mpe_config(), MpeConfig::default());
    assert!(h.shown(W::new("inst-midi-channel")) && h.shown(W::new("inst-bend-range")));
    assert!(!h.shown(W::new("inst-mpe-members")));
    pick(&mut h, "inst-mpe-mode", 1);
    h.frame();
    assert_eq!(h.session().mpe_config().mode, MpeMode::Lower);
    assert_eq!(h.studio.prefs.mpe.mode, MpeMode::Lower);
    // MPE replaces the ordinary channel filter and bend range rows.
    assert!(!h.shown(W::new("inst-midi-channel")) && !h.shown(W::new("inst-bend-range")));
    pick(&mut h, "inst-mpe-members", 6);
    pick(&mut h, "inst-mpe-note-bend", 2);
    pick(&mut h, "inst-mpe-zone-bend", 6);
    let c = h.session().mpe_config();
    assert_eq!(
        (c.members, c.member_bend_range, c.manager_bend_range),
        (7, 24.0, 12.0)
    );
    assert_eq!(h.studio.prefs.mpe, c);
    assert_eq!(crate::instrument_editor::members_text(&c), "7 · ch 2–8");
    assert_eq!(
        crate::instrument_editor::members_text(&MpeConfig {
            mode: MpeMode::Upper,
            ..c
        }),
        "7 · ch 9–15"
    );
    // A controller's RPN can change the live setup; the editor shows it and
    // the saved preference does not fight it frame by frame.
    let live = MpeConfig {
        member_bend_range: 96.0,
        ..c
    };
    h.studio.session.configure_mpe(live).unwrap();
    h.frames(5);
    assert_eq!(h.session().mpe_config(), live);
    // A fresh project picks the saved setup up again.
    h.studio
        .session
        .configure_mpe(MpeConfig::default())
        .unwrap();
    h.studio.apply_io_prefs();
    assert_eq!(h.session().mpe_config(), c);
    // Off brings the ordinary rows back.
    pick(&mut h, "inst-mpe-mode", 0);
    h.frame();
    assert_eq!(h.session().mpe_config().mode, MpeMode::Off);
    assert!(h.shown(W::new("inst-midi-channel")));
}
