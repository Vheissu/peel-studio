//! Interaction tests: drive the real Studio UI frame by frame with
//! scripted pointer input against a silent engine.

use peel_document::{ClipContent, Time};
use peel_session::{PlayMode, Session, template};
use peel_ui::{App, Id as W, Rect, Ui, Vec2, WindowControl, vec2};

use crate::studio::Studio;
use crate::workspace::DockTab;

struct Harness {
    ui: Ui,
    studio: Studio,
    win: WindowControl,
    time: f64,
}

impl Harness {
    fn new() -> Harness {
        let session = Session::new_silent(template::beat_and_melody());
        let mut h = Harness {
            ui: Ui::new(),
            studio: Studio::new(session),
            win: WindowControl::headless(),
            time: 0.0,
        };
        h.frames(2);
        h
    }

    fn frame(&mut self) {
        self.time += 1.0 / 60.0;
        self.ui.begin_frame(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1440.0,
                h: 900.0,
            },
            1.0,
            self.time,
        );
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

    fn move_to(&mut self, p: Vec2) {
        let old = self.ui.input.pointer.unwrap_or(p);
        self.ui.input.pointer = Some(p);
        self.ui.input.pointer_delta = p - old;
        self.ui.input.had_event = true;
        self.frame();
    }

    fn press(&mut self, button: usize) {
        self.ui.input.down[button] = true;
        self.ui.input.pressed[button] = true;
        self.ui.input.press_pos[button] = self.ui.input.pointer.unwrap_or_default();
        self.frame();
    }

    fn release(&mut self, button: usize) {
        self.ui.input.down[button] = false;
        self.ui.input.released[button] = true;
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

    fn drag(&mut self, from: Vec2, to: Vec2, steps: usize) {
        self.move_to(from);
        self.move_to(from);
        self.press(0);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            self.move_to(from + (to - from) * t);
        }
        self.release(0);
    }

    fn session(&self) -> &Session {
        &self.studio.session
    }
}

#[test]
fn clicking_a_step_adds_and_removes_a_note() {
    let mut h = Harness::new();
    let kick = h.session().project().channels[0].id;
    let pid = h.session().selected_pattern();
    let count = |h: &Harness| {
        h.session()
            .project()
            .pattern(pid)
            .unwrap()
            .notes_for(kick)
            .count()
    };
    assert_eq!(count(&h), 4);
    h.click(W::new(("step", kick, 2usize)));
    assert_eq!(count(&h), 5);
    let note = *h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes_for(kick)
        .find(|n| n.start == Time::new(1, 2))
        .unwrap();
    assert_eq!(note.length, Time::new(1, 4));
    h.click(W::new(("step", kick, 2usize)));
    assert_eq!(count(&h), 4);
    // Undo brings the step back; redo removes it again.
    h.studio.session.doc.undo();
    h.frame();
    assert_eq!(count(&h), 5);
    h.studio.session.doc.redo();
    h.frame();
    assert_eq!(count(&h), 4);
}

#[test]
fn dragging_across_steps_paints_one_undo_step() {
    let mut h = Harness::new();
    let snare = h
        .session()
        .project()
        .channels
        .iter()
        .find(|c| c.name == "Snare")
        .unwrap()
        .id;
    let pid = h.session().selected_pattern();
    let before = h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes_for(snare)
        .count();
    let a = h.rect(W::new(("step", snare, 0usize))).center();
    let b = h.rect(W::new(("step", snare, 3usize))).center();
    let undo_before = h.session().doc.history_labels().count();
    h.drag(a, b, 12);
    let after = h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes_for(snare)
        .count();
    assert_eq!(after, before + 4, "four steps painted");
    assert_eq!(
        h.session().doc.history_labels().count(),
        undo_before + 1,
        "one history entry"
    );
}

#[test]
fn play_button_starts_the_transport() {
    let mut h = Harness::new();
    h.click(W::new("play"));
    assert!(h.session().is_playing());
    std::thread::sleep(std::time::Duration::from_millis(150));
    h.frame();
    assert!(h.session().position() > 0.0, "silent clock advances");
    h.click(W::new("stop"));
    assert!(!h.session().is_playing());
}

#[test]
fn piano_roll_draws_a_note_where_clicked() {
    let mut h = Harness::new();
    h.click(W::new(("dock-tab", "Piano roll")));
    assert_eq!(h.studio.ws.dock, DockTab::PianoRoll);
    h.frame();
    let ch = h.studio.ws.selected_channel.unwrap();
    let pid = h.session().selected_pattern();
    let before = h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes_for(ch)
        .count();
    let canvas = h.rect(W::new("roll-canvas"));
    h.click_at(vec2(canvas.x + 40.0, canvas.y + 40.0));
    let notes: Vec<_> = h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes_for(ch)
        .copied()
        .collect();
    assert_eq!(notes.len(), before + 1);
    // Drag the new note's right edge to lengthen it.
    let new = notes
        .iter()
        .find(|n| h.studio.ws.roll.selection.contains(&n.id))
        .copied()
        .unwrap();
    let len_before = new.length;
    let canvas = h.rect(W::new("roll-canvas"));
    let zoom = h.studio.ws.roll.zoom_x;
    let x_end = canvas.x + (new.end().as_f64() as f32 - h.studio.ws.roll.scroll_beats) * zoom - 3.0;
    let row_y =
        canvas.y + (h.studio.ws.roll.top_key - new.key as f32 + 0.5) * h.studio.ws.roll.row_h;
    h.drag(vec2(x_end, row_y), vec2(x_end + zoom, row_y), 8);
    let resized = h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes
        .iter()
        .find(|n| n.id == new.id)
        .copied()
        .unwrap();
    assert!(
        resized.length > len_before,
        "{:?} -> {:?}",
        len_before,
        resized.length
    );
}

#[test]
fn mixer_fader_changes_track_gain() {
    let mut h = Harness::new();
    h.click(W::new(("dock-tab", "Mixer")));
    h.frame();
    let track = h.session().project().mixer.tracks[1].clone();
    let fader = h.rect(W::new(("fader", track.id)));
    let c = fader.center();
    h.drag(c, c + vec2(0.0, 60.0), 10);
    let after = h.session().project().mixer.track(track.id).unwrap().gain_db;
    assert!(
        after < track.gain_db - 3.0,
        "{} -> {}",
        track.gain_db,
        after
    );
}

#[test]
fn clicking_an_empty_lane_paints_the_selected_pattern() {
    let mut h = Harness::new();
    let lane = h.session().project().arrangement().tracks[5].id;
    let before = h.session().project().arrangement().clips.len();
    let canvas = h.rect(W::new("pl-canvas"));
    let lane_h = h.studio.ws.playlist.lane_h;
    h.click_at(vec2(canvas.x + 100.0, canvas.y + lane_h * 5.5));
    let arr = h.session().project().arrangement();
    assert_eq!(arr.clips.len(), before + 1);
    let clip = arr.clips.last().unwrap();
    assert_eq!(clip.track, lane);
    assert!(
        matches!(clip.content, ClipContent::Pattern { pattern } if pattern == h.session().selected_pattern())
    );
}

#[test]
fn mode_switch_and_pattern_selection() {
    let mut h = Harness::new();
    let song = h.rect(W::new("mode").with(1usize)).center();
    h.click_at(song);
    assert_eq!(h.session().mode(), PlayMode::Song);
    let chords = h.session().project().patterns[1].id;
    h.studio.session.select_pattern(chords);
    h.frame();
    assert_eq!(h.session().selected_pattern(), chords);
    assert_eq!(
        h.session().mode(),
        PlayMode::Song,
        "selecting a pattern does not leave song mode"
    );
}
