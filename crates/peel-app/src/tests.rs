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

fn command_key(h: &mut Harness, ch: &str, code: peel_ui::KeyCode) {
    let mods = peel_ui::Modifiers {
        logo: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Default::default()
    };
    h.ui.input.modifiers = mods;
    h.ui.input.keys.push(peel_ui::input::KeyPress {
        key: peel_ui::Key::Character(ch.into()),
        code: Some(code),
        repeat: false,
        modifiers: mods,
        consumed: false,
    });
    h.frame();
    h.ui.input.modifiers = peel_ui::Modifiers::default();
}

#[test]
fn make_unique_gives_a_clip_its_own_pattern() {
    let mut h = Harness::new();
    // Paint a second Beat clip so the pattern is shared.
    let canvas = h.rect(W::new("pl-canvas"));
    let lane_h = h.studio.ws.playlist.lane_h;
    h.click_at(vec2(canvas.x + 60.0, canvas.y + lane_h * 4.5));
    let beat = h.session().project().patterns[0].id;
    assert_eq!(h.session().project().pattern_usage(beat), 2);
    let clip = h
        .session()
        .project()
        .arrangement()
        .clips
        .last()
        .unwrap()
        .clone();
    // Right click the new clip and choose Make unique.
    let p = vec2(canvas.x + 70.0, canvas.y + lane_h * 4.5);
    h.move_to(p);
    h.move_to(p);
    h.press(1);
    h.release(1);
    h.frame();
    h.click(W::new("pl-clip-menu").with(("item", 1usize)));
    let after = h
        .session()
        .project()
        .arrangement()
        .clips
        .iter()
        .find(|c| c.id == clip.id)
        .unwrap()
        .clone();
    let ClipContent::Pattern { pattern } = after.content else {
        panic!()
    };
    assert_ne!(pattern, beat);
    assert_eq!(h.session().project().pattern_usage(beat), 1);
    assert_eq!(
        h.session().project().pattern(pattern).unwrap().notes.len(),
        h.session().project().pattern(beat).unwrap().notes.len()
    );
    assert_eq!(h.session().selected_pattern(), pattern);
}

#[test]
fn piano_roll_copy_paste_and_quantize() {
    let mut h = Harness::new();
    let chords = h.session().project().patterns[1].id;
    h.studio.session.select_pattern(chords);
    let piano = h
        .session()
        .project()
        .channels
        .iter()
        .find(|c| c.name == "Piano")
        .unwrap()
        .id;
    h.studio.ws.selected_channel = Some(piano);
    h.click(W::new(("dock-tab", "Piano roll")));
    h.frame();
    let count = |h: &Harness| {
        h.session()
            .project()
            .pattern(chords)
            .unwrap()
            .notes_for(piano)
            .count()
    };
    let before = count(&h);
    h.studio.ws.roll.selection = h
        .session()
        .project()
        .pattern(chords)
        .unwrap()
        .notes_for(piano)
        .take(3)
        .map(|n| n.id)
        .collect();
    h.move_to(vec2(5.0, 5.0));
    command_key(&mut h, "c", peel_ui::KeyCode::KeyC);
    command_key(&mut h, "v", peel_ui::KeyCode::KeyV);
    assert_eq!(count(&h), before + 3);
    // Nudge a note off the grid, then quantize everything.
    let off = h
        .session()
        .project()
        .pattern(chords)
        .unwrap()
        .notes_for(piano)
        .next()
        .unwrap()
        .id;
    h.studio.session.doc.edit("nudge", |p| {
        if let Some(pt) = p.pattern_mut(chords)
            && let Some(n) = pt.notes.iter_mut().find(|n| n.id == off)
        {
            n.start = Time::new(1, 17);
        }
    });
    h.studio.ws.roll.selection.clear();
    h.frame();
    h.click(W::new("roll-quantize"));
    let n = h
        .session()
        .project()
        .pattern(chords)
        .unwrap()
        .notes
        .iter()
        .find(|n| n.id == off)
        .copied()
        .unwrap();
    assert!(
        n.start.rem_euclid(h.studio.ws.roll.snap).is_zero(),
        "{:?}",
        n.start
    );
}

#[test]
fn chord_stamp_places_a_diatonic_triad() {
    let mut h = Harness::new();
    h.click(W::new(("dock-tab", "Piano roll")));
    h.frame();
    h.studio.ws.roll.scale = Some(crate::harmony::Scale { root: 9, kind: 1 }); // A minor
    h.studio.ws.roll.chord = 1; // triad in scale
    let ch = h.studio.ws.selected_channel.unwrap();
    let pid = h.session().selected_pattern();
    let before: Vec<_> = h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes_for(ch)
        .map(|n| n.id)
        .collect();
    h.frame();
    let canvas = h.rect(W::new("roll-canvas"));
    h.click_at(vec2(canvas.x + 30.0, canvas.y + 50.0));
    let mut added: Vec<u8> = h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes_for(ch)
        .filter(|n| !before.contains(&n.id))
        .map(|n| n.key)
        .collect();
    added.sort();
    assert_eq!(added.len(), 3, "{added:?}");
    let sc = crate::harmony::Scale { root: 9, kind: 1 };
    assert!(added.iter().all(|k| sc.contains(*k)), "{added:?}");
    assert_eq!(h.session().doc.undo_label(), Some("Add chord"));
}

#[test]
fn playlist_marquee_selects_and_moves_a_group() {
    let mut h = Harness::new();
    h.studio.ws.playlist.tool = crate::workspace::Tool::Select;
    h.frame();
    let canvas = h.rect(W::new("pl-canvas"));
    let lane_h = h.studio.ws.playlist.lane_h;
    // Marquee from an empty lane up across the first two lanes.
    h.drag(
        vec2(canvas.x + 60.0, canvas.y + lane_h * 3.5),
        vec2(canvas.x + 5.0, canvas.y + lane_h * 0.5),
        6,
    );
    assert_eq!(
        h.studio.ws.playlist.selection.len(),
        2,
        "beat and chords clips selected"
    );
    let before: Vec<_> = h
        .session()
        .project()
        .arrangement()
        .clips
        .iter()
        .filter(|c| h.studio.ws.playlist.selection.contains(&c.id))
        .map(|c| (c.id, c.start))
        .collect();
    // Drag the group four beats to the right.
    let zoom = h.studio.ws.playlist.zoom_x;
    let from = vec2(canvas.x + 30.0, canvas.y + lane_h * 0.5);
    h.drag(from, from + vec2(zoom * 4.0, 0.0), 8);
    for (id, start) in before {
        let now = h
            .session()
            .project()
            .arrangement()
            .clips
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .start;
        assert_eq!(now, start + Time::beats(4));
    }
    assert_eq!(h.session().doc.undo_label(), Some("Move 2 clips"));
}

#[test]
fn browser_presets_add_configured_channels() {
    let mut h = Harness::new();
    let n = h.session().project().channels.len();
    h.click(W::new(("browser-dev", "peel.synth.bass")).with("caret"));
    h.frame();
    h.click(W::new(("browser-preset", "peel.synth.bass", "Acid")));
    let p = h.session().project();
    assert_eq!(p.channels.len(), n + 1);
    let ch = p.channels.last().unwrap();
    assert_eq!(ch.name, "Acid");
    assert_eq!(ch.device.preset.as_deref(), Some("Acid"));
    let preset = peel_devices::presets("peel.synth.bass")
        .iter()
        .find(|p| p.name == "Acid")
        .unwrap();
    for (k, v) in preset.params {
        assert_eq!(ch.device.params.get(*k), Some(v), "{k}");
    }
    assert_eq!(h.studio.ws.selected_channel, Some(ch.id));
}

#[test]
fn dropping_a_sample_on_a_lane_adds_an_audio_clip() {
    let dir = std::env::temp_dir().join(format!("peel-app-audio-{}", peel_document::Id::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    let sr = 48_000u32;
    let tone: Vec<f32> = (0..sr)
        .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() * 0.5)
        .collect();
    peel_media::write_wav(&path, &[&tone, &tone], sr, peel_media::WavFormat::PCM24).unwrap();

    let mut h = Harness::new();
    let lane = h.session().project().arrangement().tracks[4].id;
    let canvas = h.rect(W::new("pl-canvas"));
    let lane_h = h.studio.ws.playlist.lane_h;
    h.ui.start_drag(peel_ui::DragPayload {
        data: Box::new(crate::browser::BrowserItem::Sample(path.clone())),
        label: "tone".into(),
        color: peel_ui::Color::WHITE,
    });
    h.ui.input.down[0] = true;
    let p = vec2(canvas.x + 50.0, canvas.y + lane_h * 4.5);
    h.move_to(p);
    h.release(0);
    let clip = h
        .session()
        .project()
        .arrangement()
        .clips
        .iter()
        .find(|c| matches!(c.content, ClipContent::Audio { .. }))
        .cloned()
        .expect("audio clip");
    assert_eq!(clip.track, lane);
    // One second at 118 BPM is just under two beats.
    assert!(
        clip.length >= Time::new(15, 8) && clip.length <= Time::beats(2),
        "{:?}",
        clip.length
    );
    let ClipContent::Audio { asset, .. } = clip.content else {
        unreachable!()
    };
    // The decode runs on the loader thread; peaks appear once it finishes.
    let mut ok = false;
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.frame();
        if h.session().audio_peaks(asset).is_some() {
            ok = true;
            break;
        }
    }
    assert!(ok, "peaks computed");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dragging_a_clip_left_edge_trims_its_start() {
    let mut h = Harness::new();
    let beat_clip = h.session().project().arrangement().clips[0].clone();
    let canvas = h.rect(W::new("pl-canvas"));
    let zoom = h.studio.ws.playlist.zoom_x;
    let lane_h = h.studio.ws.playlist.lane_h;
    let from = vec2(canvas.x + 3.0, canvas.y + lane_h * 0.6);
    h.drag(from, from + vec2(zoom * 4.0, 0.0), 8);
    let c = h
        .session()
        .project()
        .arrangement()
        .clips
        .iter()
        .find(|c| c.id == beat_clip.id)
        .cloned()
        .unwrap();
    assert_eq!(c.start, Time::beats(4));
    assert_eq!(c.offset, Time::beats(4));
    assert_eq!(c.length, beat_clip.length - Time::beats(4));
    assert_eq!(c.end(), beat_clip.end(), "the end stays put");
}

#[test]
fn automating_a_fader_creates_an_editable_clip() {
    let mut h = Harness::new();
    h.click(W::new(("dock-tab", "Mixer")));
    h.frame();
    let track = h.session().project().mixer.tracks[1].id;
    let fader = h.rect(W::new(("fader", track)));
    h.move_to(fader.center());
    h.move_to(fader.center());
    h.press(1);
    h.release(1);
    h.frame();
    h.click(
        W::new(("fader", track))
            .with("automate")
            .with(("item", 0usize)),
    );
    let clip = h
        .session()
        .project()
        .arrangement()
        .clips
        .iter()
        .find(|c| matches!(&c.content, ClipContent::Automation { target: peel_document::AutomationTarget::TrackGain { track: t }, .. } if *t == track))
        .cloned()
        .expect("automation clip");
    let ClipContent::Automation { points, .. } = &clip.content else {
        unreachable!()
    };
    assert_eq!(points.len(), 2);
    // Click inside the envelope to add a breakpoint.
    h.frame();
    let canvas = h.rect(W::new("pl-canvas"));
    let lane = h
        .session()
        .project()
        .arrangement()
        .tracks
        .iter()
        .position(|t| t.id == clip.track)
        .unwrap();
    let lane_h = h.studio.ws.playlist.lane_h;
    let zoom = h.studio.ws.playlist.zoom_x;
    let p = vec2(
        canvas.x + zoom * 6.0,
        canvas.y + lane_h * lane as f32 + lane_h * 0.75,
    );
    h.click_at(p);
    let after = h
        .session()
        .project()
        .arrangement()
        .clips
        .iter()
        .find(|c| c.id == clip.id)
        .cloned()
        .unwrap();
    let ClipContent::Automation { points, .. } = &after.content else {
        unreachable!()
    };
    assert_eq!(points.len(), 3, "{points:?}");
    assert!(points.windows(2).all(|w| w[0].position <= w[1].position));
}

#[test]
fn history_panel_jumps_back_several_steps() {
    let mut h = Harness::new();
    for bpm in [100.0, 110.0, 120.0] {
        h.studio
            .session
            .doc
            .edit(format!("Tempo {bpm}"), |p| p.set_bpm(bpm));
    }
    h.frame();
    let undo = h.rect(W::new("undo")).center();
    h.move_to(undo);
    h.move_to(undo);
    h.press(1);
    h.release(1);
    h.frame();
    // Click the second entry (two steps back).
    h.click(W::new(("history-undo", 1usize)));
    assert_eq!(h.session().project().bpm(), 100.0);
    assert_eq!(h.session().doc.redo_label(), Some("Tempo 110"));
}

#[test]
fn adding_a_send_routes_a_track_to_a_bus() {
    let mut h = Harness::new();
    // Make a bus and focus the Piano track.
    h.studio.session.doc.edit("Add bus", |p| {
        p.mixer.tracks.push(peel_document::MixerTrack {
            id: peel_document::Id::new(),
            name: "Verb".into(),
            color: peel_document::PALETTE[7],
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            inserts: vec![],
            output: None,
            sends: vec![],
        })
    });
    let piano = h
        .session()
        .project()
        .mixer
        .tracks
        .iter()
        .find(|t| t.name == "Piano")
        .unwrap()
        .id;
    let bus = h.session().project().mixer.tracks.last().unwrap().id;
    h.studio.ws.focus = Some(crate::workspace::Focus::Track(piano));
    h.frame();
    h.click(W::new(("send-add", piano)));
    h.frame();
    // The bus is the only valid target besides other channel tracks; pick it by name.
    let targets: Vec<String> = h
        .session()
        .project()
        .mixer
        .tracks
        .iter()
        .skip(1)
        .filter(|t| t.id != piano)
        .map(|t| t.name.clone())
        .collect();
    let idx = targets.iter().position(|n| n == "Verb").unwrap();
    h.click(W::new(("send-menu", piano)).with(("item", idx)));
    let t = h.session().project().mixer.track(piano).unwrap().clone();
    assert_eq!(t.sends.len(), 1);
    assert_eq!(t.sends[0].target, bus);
    // The bus can no longer send back to the piano track (that would loop).
    assert!(super::inspector_reaches(h.session().project(), piano, bus));
}
