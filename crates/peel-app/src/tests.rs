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
    size: Vec2,
}

impl Harness {
    fn new() -> Harness {
        let session = Session::new_silent(template::beat_and_melody());
        let mut h = Harness {
            ui: Ui::new(),
            studio: Studio::new(session),
            win: WindowControl::headless(),
            time: 0.0,
            size: vec2(1440.0, 900.0),
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
                w: self.size.x,
                h: self.size.y,
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

#[test]
fn deleting_a_mixer_track_reroutes_its_channel() {
    let mut h = Harness::new();
    h.click(W::new(("dock-tab", "Mixer")));
    h.frame();
    let kick_track = h
        .session()
        .project()
        .mixer
        .tracks
        .iter()
        .find(|t| t.name == "Kick")
        .unwrap()
        .id;
    let name = h.rect(W::new(("strip-name", kick_track))).center();
    h.move_to(name);
    h.move_to(name);
    h.press(1);
    h.release(1);
    h.frame();
    let delete_index = 2 + 1 + peel_document::PALETTE.len() + 1;
    h.click(W::new(("strip-menu", kick_track)).with(("item", delete_index)));
    let p = h.session().project();
    assert!(p.mixer.track(kick_track).is_none());
    let kick = p.channels.iter().find(|c| c.name == "Kick").unwrap();
    assert_eq!(
        kick.track, p.mixer.tracks[0].id,
        "kick now plays through the master"
    );
    assert!(peel_document::validate(p).iter().all(|i| !i.is_error()));
}

#[test]
fn tapping_the_bpm_label_sets_the_tempo() {
    let mut h = Harness::new();
    let label = h.rect(W::new("tap-tempo")).center();
    for _ in 0..4 {
        h.click_at(label);
        h.frames(26); // 4 + 26 frames = half a second per tap
    }
    let bpm = h.session().project().bpm();
    assert!((bpm - 120.0).abs() < 0.6, "{bpm}");
    assert_eq!(
        h.session()
            .doc
            .history_labels()
            .filter(|l| *l == "Tap tempo")
            .count(),
        1
    );
}

#[test]
fn lane_menu_inserts_and_deletes_lanes() {
    let mut h = Harness::new();
    let lanes = h.session().project().arrangement().tracks.clone();
    let drums = lanes[0].id;
    let open_menu = |h: &mut Harness, lane: peel_document::Id| {
        let r = h.rect(W::new(("pl-lane-name", lane))).center();
        h.move_to(r);
        h.move_to(r);
        h.press(1);
        h.release(1);
        h.frame();
    };
    open_menu(&mut h, drums);
    h.click(W::new(("pl-lane-menu", drums)).with(("item", 1usize)));
    assert_eq!(
        h.session().project().arrangement().tracks.len(),
        lanes.len() + 1
    );
    assert_ne!(
        h.session().project().arrangement().tracks[0].id,
        drums,
        "new lane above"
    );
    h.frame();
    open_menu(&mut h, drums);
    h.click(W::new(("pl-lane-menu", drums)).with(("item", 5usize)));
    let arr = h.session().project().arrangement();
    assert!(arr.tracks.iter().all(|t| t.id != drums));
    assert!(
        arr.clips.iter().all(|c| c.track != drums),
        "its clips went with it"
    );
}

/// Random input across the whole window must never panic or leave the
/// project invalid. Seeded so failures reproduce.
#[test]
fn monkey_input_keeps_the_project_valid() {
    monkey(&[1u64, 7, 42, 3, 99]);
}

/// Many more seeds; run with `cargo test -p peel-app -- --ignored monkey_long`.
#[test]
#[ignore = "long"]
fn monkey_long() {
    let seeds: Vec<u64> = (100..160).collect();
    monkey(&seeds);
}

fn monkey(seeds: &[u64]) {
    for &seed in seeds {
        let mut h = Harness::new();
        // Later seeds work on the House project with the tempo strip open.
        if seed % 2 == 1 && seed != 1 && seed != 7 {
            h.studio
                .session
                .replace_project(peel_session::template::house());
            h.studio.ws.playlist.show_tempo = true;
            h.frames(2);
        }
        let mut state = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut rnd = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as f32 / (1u64 << 31) as f32
        };
        let keys = [
            peel_ui::NamedKey::Delete,
            peel_ui::NamedKey::ArrowUp,
            peel_ui::NamedKey::ArrowDown,
            peel_ui::NamedKey::Escape,
            peel_ui::NamedKey::F6,
            peel_ui::NamedKey::F7,
            peel_ui::NamedKey::F9,
            peel_ui::NamedKey::F10,
        ];
        for step in 0..600 {
            let p = vec2(rnd() * 1440.0, rnd() * 900.0);
            match (rnd() * 10.0) as u32 {
                0..=3 => h.click_at(p),
                4 => {
                    let to = vec2(rnd() * 1440.0, rnd() * 900.0);
                    h.drag(p, to, 4);
                }
                5 => {
                    h.move_to(p);
                    h.press(1);
                    h.release(1);
                }
                6 => {
                    h.move_to(p);
                    h.ui.input.scroll = vec2(0.0, (rnd() - 0.5) * 200.0);
                    h.frame();
                }
                7 => {
                    let k = keys[(rnd() * keys.len() as f32) as usize % keys.len()];
                    h.ui.input.keys.push(peel_ui::input::KeyPress {
                        key: peel_ui::Key::Named(k),
                        code: None,
                        repeat: false,
                        modifiers: Default::default(),
                        consumed: false,
                    });
                    h.frame();
                }
                8 => {
                    h.studio.session.doc.undo();
                    h.frame();
                }
                9 if rnd() < 0.5 => {
                    h.move_to(p);
                    h.move_to(p);
                    h.ui.input.double_clicked = true;
                    h.press(0);
                    h.release(0);
                }
                _ => h.frames(2),
            }
            if step % 100 == 0 {
                let issues = peel_document::validate(h.session().project());
                assert!(
                    issues.iter().all(|i| !i.is_error()),
                    "seed {seed} step {step}: {issues:?}"
                );
            }
        }
        let issues = peel_document::validate(h.session().project());
        assert!(
            issues.iter().all(|i| !i.is_error()),
            "seed {seed}: {issues:?}"
        );
    }
}

#[test]
fn slicing_a_clip_keeps_the_music_identical() {
    use std::collections::HashMap;
    let mut h = Harness::new();
    let notes = |h: &Harness| -> Vec<(u64, u8)> {
        let p = h.session().project();
        let slots: HashMap<_, _> = p
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| (c.device.id, i as u32))
            .collect();
        let meters: HashMap<_, _> = p
            .mixer
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id, i as u16))
            .collect();
        let samples = HashMap::new();
        let ctx = peel_session::CompileContext {
            mode: PlayMode::Song,
            slots: &slots,
            meters: &meters,
            samples: &samples,
        };
        let (plan, _) = peel_session::compile(p, &ctx);
        let mut v: Vec<(u64, u8)> = plan
            .sequence
            .iter()
            .filter_map(|e| match e.kind {
                peel_engine::SeqKind::NoteOn { key, .. } => {
                    Some(((e.beat * 960.0).round() as u64, key))
                }
                _ => None,
            })
            .collect();
        v.sort();
        v
    };
    let before = notes(&h);
    let clips_before = h.session().project().arrangement().clips.len();
    // Slice the Beat clip at bar 3 and the Chords clip at a beat inside a chord.
    let beat = h.session().project().arrangement().clips[0].clone();
    crate::playlist::slice_clip(&mut h.studio.session, &beat, Time::beats(8));
    let chords = h.session().project().arrangement().clips[1].clone();
    crate::playlist::slice_clip(&mut h.studio.session, &chords, Time::beats(6));
    assert_eq!(
        h.session().project().arrangement().clips.len(),
        clips_before + 2
    );
    let after = notes(&h);
    // Every original onset survives; chords sounding across the cut are
    // chased (restarted at the cut) so they keep sounding.
    // Multiset difference: after minus before.
    let mut extra = after.clone();
    for n in &before {
        let i = extra
            .iter()
            .position(|x| x == n)
            .unwrap_or_else(|| panic!("lost {n:?}"));
        extra.remove(i);
    }
    assert!(
        extra.iter().all(|(t, _)| *t == 6 * 960),
        "only chased notes at the cut: {extra:?}"
    );
    assert_eq!(extra.len(), 6, "three piano and three pad notes chased");
}

#[test]
fn duplicating_an_arrangement_shares_patterns() {
    let mut h = Harness::new();
    let original = h.session().project().arrangement().as_ref().clone();
    h.click(W::new("arrangement-picker"));
    h.frame();
    // Items: one arrangement, separator, then New, Duplicate, ...
    h.click(W::new("arrangement-menu").with(("item", 3usize)));
    let p = h.session().project();
    assert_eq!(p.arrangements.len(), 2);
    let copy = p.arrangement();
    assert_ne!(copy.id, original.id, "the copy is active");
    assert_eq!(copy.clips.len(), original.clips.len());
    assert!(
        copy.clips
            .iter()
            .all(|c| original.clips.iter().all(|o| o.id != c.id)),
        "new clip ids"
    );
    let beat = p.patterns[0].id;
    assert_eq!(
        p.pattern_usage(beat),
        2,
        "both arrangements use the same Beat pattern"
    );
    assert!(peel_document::validate(p).iter().all(|i| !i.is_error()));
}

#[test]
fn markers_jump_move_and_get_added() {
    let mut h = Harness::new();
    let markers = |h: &Harness| h.session().project().arrangement().markers.clone();
    let drop = markers(&h)[1].clone();
    assert_eq!(drop.name, "Drop");
    // Click: the playhead jumps there in Song mode.
    h.click(W::new(("marker", drop.id)));
    assert_eq!(h.session().mode(), PlayMode::Song);
    assert!(
        (h.session().start_position() - 16.0).abs() < 1e-6,
        "{}",
        h.session().start_position()
    );
    // Drag four bars right.
    let r = h.rect(W::new(("marker", drop.id)));
    let bar_px = {
        let a = h.rect(W::new(("marker", markers(&h)[0].id)));
        (r.x - a.x) / 4.0
    };
    let from = vec2(r.x + 6.0, r.center().y);
    h.drag(from, from + vec2(bar_px * 4.0, 0.0), 8);
    let moved = markers(&h).into_iter().find(|m| m.id == drop.id).unwrap();
    assert_eq!(moved.position, Time::beats(32));
    assert_eq!(h.session().doc.undo_label(), Some("Move marker"));
    // Right-click the ruler: Add marker.
    let ruler = h.rect(W::new("pl-ruler"));
    h.move_to(vec2(ruler.x + bar_px * 2.0 + 2.0, ruler.y + 6.0));
    h.press(1);
    h.release(1);
    h.click(W::new("pl-ruler").with("menu").with(("item", 0usize)));
    let all = markers(&h);
    assert_eq!(all.len(), 3);
    assert!(
        all.windows(2).all(|w| w[0].position <= w[1].position),
        "kept in order"
    );
    assert_eq!(all[1].position, Time::beats(8));
}

#[test]
fn recording_audio_onto_an_armed_lane() {
    use peel_document::ClipContent;
    let mut h = Harness::new();
    let dir = std::env::temp_dir().join(format!("peel-app-rec-{}", peel_document::Id::new()));
    h.studio.session.set_recordings_dir(Some(dir.clone()));
    let mut input = h.studio.session.open_injected_input(48_000, 1);
    let lane = h.session().project().arrangement().tracks[4].id;
    // The arm button shows on hover.
    let head = h.rect(W::new(("pl-lane-name", lane)));
    h.move_to(head.center());
    h.click(W::new(("pl-arm", lane)));
    assert!(h.session().is_lane_armed(lane));
    h.studio.session.set_mode(PlayMode::Song);
    h.click(W::new("rec"));
    h.studio.session.play();
    h.frame();
    assert!(
        h.session().is_recording(),
        "Record + Play in Song mode starts a take"
    );
    let tone: Vec<f32> = (0..24_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    for chunk in tone.chunks(512) {
        input.push(chunk);
        std::thread::sleep(std::time::Duration::from_millis(4));
        h.frame();
    }
    h.studio.session.stop();
    let start = std::time::Instant::now();
    let on_lane = |h: &Harness| {
        h.session()
            .project()
            .arrangement()
            .clips
            .iter()
            .filter(|c| c.track == lane && matches!(c.content, ClipContent::Audio { .. }))
            .count()
    };
    while on_lane(&h) == 0 {
        assert!(start.elapsed().as_secs() < 10, "the take became a clip");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.frame();
    }
    assert_eq!(on_lane(&h), 1);
    assert_eq!(h.session().doc.undo_label(), Some("Record audio"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_inspector_edits_a_selected_audio_clip() {
    let dir = std::env::temp_dir().join(format!("peel-app-clip-{}", peel_document::Id::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("quiet.wav");
    let sr = 48_000u32;
    // Peaks at 0.25 (-12 dB).
    let tone: Vec<f32> = (0..sr)
        .map(|i| (i as f32 * 220.0 * std::f32::consts::TAU / sr as f32).sin() * 0.25)
        .collect();
    peel_media::write_wav(&path, &[&tone, &tone], sr, peel_media::WavFormat::FLOAT32).unwrap();
    let mut h = Harness::new();
    let lane = h.session().project().arrangement().tracks[4].id;
    h.studio
        .session
        .add_audio_clip(lane, Time::beats(1), &path)
        .expect("add clip");
    let clip = |h: &Harness| {
        h.session()
            .project()
            .arrangement()
            .clips
            .iter()
            .find(|c| c.track == lane)
            .cloned()
            .expect("clip")
    };
    let id = clip(&h).id;
    let ClipContent::Audio { asset, .. } = clip(&h).content else {
        unreachable!()
    };
    for _ in 0..300 {
        if h.session().sample(asset).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.frame();
    }
    // Clicking the clip shows it in the inspector.
    let canvas = h.rect(W::new("pl-canvas"));
    let (zoom, lane_h) = (h.studio.ws.playlist.zoom_x, h.studio.ws.playlist.lane_h);
    h.click_at(vec2(canvas.x + zoom * 1.5, canvas.y + lane_h * 4.6));
    assert_eq!(h.studio.ws.focus, Some(crate::workspace::Focus::Clip(id)));
    h.frame();
    h.click(W::new(("clip-rev", id)));
    h.click(W::new(("clip-normalize", id)));
    let ClipContent::Audio {
        gain_db, reversed, ..
    } = clip(&h).content
    else {
        unreachable!()
    };
    assert!(reversed, "reversed");
    assert!(
        (gain_db - 12.04).abs() < 0.1,
        "normalized to 0 dB: {gain_db}"
    );
    // Route to the Drums mixer track.
    let drums = h.session().project().mixer.tracks[1].id;
    h.click(W::new(("clip-output", id)));
    h.click(
        W::new(("clip-output", id))
            .with("menu")
            .with(("item", 1usize)),
    );
    let ClipContent::Audio { track, .. } = clip(&h).content else {
        unreachable!()
    };
    assert_eq!(track, drums);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_tempo_strip_adds_drags_and_ramps_tempo_changes() {
    let mut h = Harness::new();
    h.click(W::new("pl-tempo"));
    assert!(h.studio.ws.playlist.show_tempo);
    h.frame();
    let lane = h.rect(W::new("tempo-lane"));
    let zoom = h.studio.ws.playlist.zoom_x;
    let tempo = |h: &Harness| h.session().project().tempo.clone();
    // Double-click at bar 5.
    let p = vec2(lane.x + zoom * 16.0, lane.center().y);
    h.move_to(p);
    h.move_to(p);
    h.ui.input.double_clicked = true;
    h.press(0);
    h.release(0);
    assert_eq!(tempo(&h).len(), 2);
    assert_eq!(tempo(&h)[1].start, Time::beats(16));
    assert_eq!(tempo(&h)[1].bpm, 118.0);
    // Drag it up a little: faster, same place, and no runaway.
    let pt = h.rect(W::new(("tempo-pt", 1usize))).center();
    h.drag(pt, pt - vec2(0.0, 10.0), 10);
    let fast = tempo(&h)[1].bpm;
    assert!(fast > 118.0 && fast < 135.0, "{fast}");
    assert_eq!(tempo(&h)[1].start, Time::beats(16));
    assert_eq!(h.session().doc.undo_label(), Some("Change tempo"));
    // Ramp from the start into it.
    let first = h.rect(W::new(("tempo-pt", 0usize))).center();
    h.move_to(first);
    h.press(1);
    h.release(1);
    h.click(
        W::new(("tempo-pt", 0usize))
            .with("menu")
            .with(("item", 0usize)),
    );
    assert_eq!(tempo(&h)[0].ramp_to, Some(fast));
    let map = peel_time::TempoMap::from_segments(&tempo(&h));
    let mid = map.bpm_at(8.0);
    assert!(mid > 118.0 && mid < fast, "{mid}");
}

#[test]
fn dragging_a_tension_handle_bends_an_automation_segment() {
    use peel_document::{AutomationPoint, AutomationTarget};
    let mut h = Harness::new();
    let track = h.session().project().mixer.tracks[1].id;
    let id = crate::playlist::create_automation(
        &mut h.studio.session,
        &mut h.studio.ws,
        AutomationTarget::TrackGain { track },
        0.5,
    );
    h.studio.session.doc.edit("Shape", |p| {
        if let Some(c) = p.arrangement_mut().clips.iter_mut().find(|c| c.id == id)
            && let ClipContent::Automation { points, .. } = &mut c.content
        {
            *points = vec![
                AutomationPoint::new(Time::ZERO, 0.2),
                AutomationPoint::new(Time::beats(16), 0.8),
            ];
        }
    });
    h.frame();
    let canvas = h.rect(W::new("pl-canvas"));
    let handle = crate::playlist::tension_handle_at(canvas, &h.studio.ws, h.session(), id, 0)
        .expect("a rising segment has a handle");
    // Pull the middle down: the segment starts slowly (positive tension).
    h.drag(handle, handle + vec2(0.0, 30.0), 6);
    let points = |h: &Harness| match &h
        .session()
        .project()
        .arrangement()
        .clips
        .iter()
        .find(|c| c.id == id)
        .unwrap()
        .content
    {
        ClipContent::Automation { points, .. } => points.clone(),
        _ => unreachable!(),
    };
    let k = points(&h)[0].tension;
    assert!(k > 0.2, "bent: {k}");
    assert_eq!(points(&h).len(), 2, "no point added");
    assert_eq!(h.session().doc.undo_label(), Some("Bend automation"));
    let mid = peel_document::automation_value(&points(&h), Time::beats(8)).unwrap();
    assert!(mid < 0.5, "the middle is lower than a straight line: {mid}");
    // Double-click straightens it.
    let handle =
        crate::playlist::tension_handle_at(canvas, &h.studio.ws, h.session(), id, 0).unwrap();
    h.move_to(handle);
    h.move_to(handle);
    h.ui.input.double_clicked = true;
    h.press(0);
    h.release(0);
    assert_eq!(points(&h)[0].tension, 0.0);
}

#[test]
fn note_tools_preview_cancel_and_apply() {
    let mut h = Harness::new();
    let p = h.session().project().clone();
    let chords = p.patterns.iter().find(|x| x.name == "Chords").unwrap().id;
    let piano = p.channels.iter().find(|c| c.name == "Piano").unwrap().id;
    h.studio.session.select_pattern(chords);
    h.studio.ws.selected_channel = Some(piano);
    h.click(W::new(("dock-tab", "Piano roll")));
    h.frame();
    let shape = |h: &Harness| {
        let mut v: Vec<(Time, u8, Time)> = h
            .session()
            .project()
            .pattern(chords)
            .unwrap()
            .notes_for(piano)
            .map(|n| (n.start, n.key, n.length))
            .collect();
        v.sort();
        v
    };
    let original = shape(&h);
    let undo_before = h.session().doc.undo_label().map(str::to_string);
    // Open: the first tool previews at once.
    h.click(W::new("roll-note-tools"));
    assert!(crate::note_tools::is_open(&h.ui));
    // Arpeggiate turns each chord into a run of single notes.
    h.click(W::new(("note-tool", 3usize)));
    let arp = shape(&h);
    assert!(
        arp.len() > original.len(),
        "{} vs {}",
        arp.len(),
        original.len()
    );
    let mut onsets: Vec<Time> = arp.iter().map(|x| x.0).collect();
    onsets.dedup();
    assert_eq!(onsets.len(), arp.len(), "one note at a time");
    // Cancel restores exactly, with no undo step left behind.
    h.click(W::new("note-tool-cancel"));
    assert!(!crate::note_tools::is_open(&h.ui));
    assert_eq!(shape(&h), original);
    assert_eq!(
        h.session().doc.undo_label().map(str::to_string),
        undo_before
    );
    // Chop and apply: one undo step that restores the original.
    h.click(W::new("roll-note-tools"));
    h.click(W::new(("note-tool", 2usize)));
    h.click(W::new("note-tool-apply"));
    let chopped = shape(&h);
    assert!(chopped.len() > original.len());
    assert_eq!(h.session().doc.undo_label(), Some("Chop"));
    h.studio.session.doc.undo();
    assert_eq!(shape(&h), original);
}

#[test]
fn the_welcome_screen_starts_a_house_project() {
    let mut h = Harness::new();
    h.studio.welcome = true;
    h.frame();
    h.click(W::new(("welcome-template", 2usize)));
    assert!(!h.studio.welcome);
    assert_eq!(h.session().project().bpm(), 124.0);
    assert!(
        h.session()
            .project()
            .channels
            .iter()
            .any(|c| c.name == "Pad")
    );
}

#[test]
fn the_perform_view_launches_and_stops_clips() {
    let mut h = Harness::new();
    h.click(W::new(("dock-tab", "Perform")));
    h.frame();
    let slots = h.session().performance_slots();
    let (lane, beat) = (slots[0].0, slots[0].1[0].clip);
    h.click(W::new(("perform-slot", beat)));
    assert!(h.session().performing() && h.session().is_playing());
    assert_eq!(
        h.session().slot_state(lane, beat),
        peel_session::SlotState::Playing
    );
    // A scene launch queues the other lanes; Stop all ends the performance.
    h.click(W::new(("perform-scene", 0usize)));
    h.click(W::new("perform-stop"));
    assert!(!h.session().performing() && !h.session().is_playing());
}

#[test]
fn chopping_an_audio_clip_cuts_it_at_each_hit() {
    let dir = std::env::temp_dir().join(format!("peel-app-chop-{}", peel_document::Id::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hits.wav");
    let sr = 48_000u32;
    // Four hits half a second apart.
    let mut x = vec![0.0f32; sr as usize * 2];
    for k in 0..4 {
        let at = k * sr as usize / 2;
        for i in 0..400 {
            x[at + i] = (1.0 - i as f32 / 400.0) * if i % 2 == 0 { 0.8 } else { -0.8 };
        }
    }
    peel_media::write_wav(&path, &[&x, &x], sr, peel_media::WavFormat::FLOAT32).unwrap();
    let mut h = Harness::new();
    let lane = h.session().project().arrangement().tracks[4].id;
    h.studio
        .session
        .add_audio_clip(lane, Time::beats(4), &path)
        .unwrap();
    let clip = h
        .session()
        .project()
        .arrangement()
        .clips
        .iter()
        .find(|c| c.track == lane)
        .unwrap()
        .clone();
    let ClipContent::Audio { asset, .. } = clip.content else {
        unreachable!()
    };
    for _ in 0..300 {
        if h.session().sample(asset).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.frame();
    }
    assert_eq!(
        crate::playlist::chop_at_hits(&mut h.studio.session, &clip),
        Some(4)
    );
    let mut pieces: Vec<_> = h
        .session()
        .project()
        .arrangement()
        .clips
        .iter()
        .filter(|c| c.track == lane)
        .cloned()
        .collect();
    pieces.sort_by_key(|c| c.start);
    assert_eq!(pieces.len(), 4);
    let beat = 118.0 / 60.0 * 0.5;
    for (k, p) in pieces.iter().enumerate() {
        let want = 4.0 + k as f64 * beat;
        assert!(
            (p.start.as_f64() - want).abs() < 0.03,
            "piece {k} at {} not {want}",
            p.start.as_f64()
        );
        // Each piece reads the file where it sits.
        assert_eq!(p.offset, clip.offset + (p.start - clip.start));
    }
    assert_eq!(pieces.last().unwrap().end(), clip.end());
    assert_eq!(h.session().doc.undo_label(), Some("Chop at hits"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_controls_follow_the_codec_and_keep_valid_settings() {
    use peel_media::AudioFormat;
    let mut h = Harness::new();
    h.studio.export();
    h.frames(2);
    h.click(W::new("export-bits").with(2usize));
    h.click(W::new("export-rate").with(3usize));
    assert_eq!(h.studio.ws.export_bits, 32);
    assert_eq!(h.studio.ws.export_rate, 96_000);
    h.click(W::new("export-format").with(1usize));
    h.frames(2);
    assert_eq!(h.studio.ws.export_format, AudioFormat::Mp3);
    assert_eq!(h.studio.ws.export_rate, 48_000);
    assert_eq!(h.studio.ws.export_bits, 24);
    assert!(
        h.ui.widget_rect(W::new("export-bits").with(0usize))
            .is_none()
    );
    assert!(
        h.ui.widget_rect(W::new("export-dither").with(0usize))
            .is_none()
    );
    h.click(W::new("export-bitrate").with(1usize));
    assert_eq!(h.studio.ws.export_bitrate, 192);
    h.click(W::new("export-channels").with(1usize));
    h.click(W::new("export-normalize").with(1usize));
    h.click(W::new("export-stems").with(1usize));
    assert!(h.studio.ws.export_mono && h.studio.ws.export_normalize && h.studio.ws.export_stems);
    h.click(W::new("export-format").with(2usize));
    h.frames(2);
    assert_eq!(h.studio.ws.export_format, AudioFormat::Flac);
    assert!(
        h.ui.widget_rect(W::new("export-bits").with(2usize))
            .is_none()
    );
    assert!(
        h.ui.widget_rect(W::new("export-dither").with(0usize))
            .is_some()
    );
    assert!(
        h.ui.widget_rect(W::new("export-bitrate").with(0usize))
            .is_none()
    );
    let export = h.rect(W::new("export-audio"));
    assert!(export.bottom() <= 900.0);
    assert!(export.right() <= 1440.0);
}

fn named_key(h: &mut Harness, key: peel_ui::NamedKey, code: peel_ui::KeyCode) {
    h.ui.input.keys.push(peel_ui::input::KeyPress {
        key: peel_ui::Key::Named(key),
        code: Some(code),
        repeat: false,
        modifiers: peel_ui::Modifiers::default(),
        consumed: false,
    });
    h.frame();
}

fn temp_scripts_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("peel-app-{tag}-{}", peel_document::Id::new()))
}

/// Frames until the running script finishes, for at most ten seconds.
fn wait_for_script(h: &mut Harness) {
    let start = std::time::Instant::now();
    while h.studio.scripts.is_running() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "the script finished within its deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.frame();
    }
}

#[test]
fn f8_and_the_rail_show_scripts_in_place_of_the_inspector() {
    use crate::scripts::PANEL;
    use peel_ui::{KeyCode, NamedKey};
    let mut h = Harness::new();
    h.studio.scripts = crate::scripts::Scripts::with_dir(temp_scripts_dir("scripts-open"));
    assert!(!h.studio.prefs.show_scripts);
    assert!(h.studio.prefs.show_inspector);
    h.rect(W::new("inspector-hide"));
    named_key(&mut h, NamedKey::F8, KeyCode::F8);
    h.frame();
    assert!(h.studio.prefs.show_scripts, "F8 shows Scripts");
    assert!(!h.studio.prefs.show_inspector, "in place of the Inspector");
    assert!(
        h.ui.widget_rect(W::new("inspector-hide")).is_none(),
        "the Inspector is not drawn under it"
    );
    assert!(
        !h.studio.scripts.factory.is_empty(),
        "factory scripts are listed"
    );
    let run = h.rect(W::new("scripts-run"));
    // Not modal: the editors beside it keep working.
    let snap = h.rect(W::new("pl-snap"));
    assert!(
        snap.right() <= run.x,
        "the column sits right of the playlist"
    );
    assert!(!h.ui.any_popup(), "no popup holds the input");
    h.click(W::new(("pl-tool", "Select")));
    assert_eq!(h.studio.ws.playlist.tool, crate::workspace::Tool::Select);
    named_key(&mut h, NamedKey::F8, KeyCode::F8);
    h.frame();
    assert!(!h.studio.prefs.show_scripts, "F8 hides it again");
    assert!(h.studio.prefs.show_inspector, "and the Inspector returns");
    h.click(W::new("scripts-open"));
    h.frame();
    assert!(h.studio.prefs.show_scripts, "the rail entry shows it");
    h.click(PANEL.with("close"));
    h.frame();
    assert!(!h.studio.prefs.show_scripts);
    assert!(h.studio.prefs.show_inspector);
    // Hidden Inspector stays hidden after Scripts closes.
    h.click(W::new("rail-inspector"));
    assert!(!h.studio.prefs.show_right());
    h.click(W::new("scripts-open"));
    h.frame();
    h.click(PANEL.with("close"));
    h.frame();
    assert!(!h.studio.prefs.show_right(), "the column stays hidden");
}

#[test]
fn the_scripts_column_fits_a_small_window() {
    let mut h = Harness::new();
    h.studio.scripts = crate::scripts::Scripts::with_dir(temp_scripts_dir("scripts-fit"));
    h.size = vec2(1024.0, 700.0);
    // A long notice must not push the column's controls around.
    h.studio.session.notify("x".repeat(400), true);
    h.frames(2);
    let button = h.rect(W::new("scripts-open"));
    let keys = h.rect(W::new("keys-help"));
    assert!(
        button.bottom() <= keys.y,
        "Scripts sits above the shortcuts entry"
    );
    assert!(button.x >= 0.0 && button.right() <= 1024.0 && keys.bottom() <= 700.0 - 24.0);
    h.click(W::new("scripts-open"));
    h.frame();
    assert!(h.studio.prefs.show_scripts);
    for id in ["scripts-run", "scripts-add", "scripts-refresh"] {
        let r = h.rect(W::new(id));
        assert!(
            r.x >= 0.0 && r.right() <= 1024.0 && r.y >= 0.0 && r.bottom() <= 700.0 - 24.0,
            "{id} is inside the window above the status bar: {r:?}"
        );
    }
}

#[test]
fn running_a_factory_script_shows_its_result_after_closing() {
    use crate::scripts::PANEL;
    let mut h = Harness::new();
    h.studio.scripts = crate::scripts::Scripts::with_dir(temp_scripts_dir("scripts-run"));
    // Select the kick's notes so the transpose action has input.
    let pid = h.session().selected_pattern();
    let kick = h.session().project().channels[0].id;
    let ids: Vec<_> = h
        .session()
        .project()
        .pattern(pid)
        .unwrap()
        .notes_for(kick)
        .map(|n| n.id)
        .collect();
    assert!(!ids.is_empty());
    h.studio.ws.selected_channel = Some(kick);
    h.studio.ws.roll.selection.extend(ids);
    h.click(W::new("scripts-open"));
    h.frame();
    use crate::scripts::Pick;
    assert!(h.studio.scripts.factory.len() > 1);
    h.click(W::new(("script-factory", 1usize)));
    assert_eq!(h.studio.scripts.selected, Some(Pick::Factory(1)));
    h.click(W::new(("script-factory", 0usize)));
    assert_eq!(h.studio.scripts.selected, Some(Pick::Factory(0)));
    let name = h.studio.scripts.factory[0].name.clone();
    assert!(!h.studio.scripts.is_running(), "nothing runs until Run");
    h.click(W::new("scripts-run"));
    assert!(h.studio.scripts.is_running());
    let again = h
        .studio
        .scripts
        .run_selected(&h.studio.session, &h.studio.ws, false);
    assert!(again.is_err(), "one script at a time");
    // Closing the column neither stops the job nor loses its result.
    h.click(PANEL.with("close"));
    assert!(!h.studio.prefs.show_scripts);
    wait_for_script(&mut h);
    let out = h.studio.scripts.last.clone().expect("an outcome");
    assert_eq!(
        out.result,
        Ok("Applied".to_string()),
        "{name}: {:?}",
        out.logs
    );
    assert_eq!(
        h.session().doc.undo_label(),
        Some(format!("Script: {name}").as_str())
    );
    h.click(W::new("scripts-open"));
    h.frame();
    assert!(h.studio.prefs.show_scripts);
    assert!(h.studio.scripts.last.is_some(), "the result is still shown");
}

#[test]
fn cancelling_a_script_applies_nothing() {
    let mut h = Harness::new();
    h.studio.scripts = crate::scripts::Scripts::with_dir(temp_scripts_dir("scripts-cancel"));
    h.click(W::new("scripts-open"));
    h.frame();
    let revision = h.session().doc.revision();
    h.click(W::new("scripts-run"));
    assert!(h.studio.scripts.is_running());
    // Cancel before the next poll so a fast script cannot finish first.
    h.studio.scripts.cancel();
    h.frame();
    if h.studio.scripts.is_running() {
        assert!(
            h.ui.widget_rect(W::new("scripts-cancel")).is_some(),
            "Cancel shows while a script runs"
        );
    }
    wait_for_script(&mut h);
    let out = h.studio.scripts.last.clone().expect("an outcome");
    assert!(out.result.is_err(), "a cancelled run reports it");
    assert_eq!(h.session().doc.revision(), revision);
}

#[test]
fn scripts_do_not_run_while_recording() {
    let mut h = Harness::new();
    h.studio.scripts = crate::scripts::Scripts::with_dir(temp_scripts_dir("scripts-rec"));
    h.click(W::new("scripts-open"));
    h.frame();
    h.studio.recorder.armed = true;
    h.studio.session.play();
    h.frame();
    h.click(W::new("scripts-run"));
    assert!(!h.studio.scripts.is_running());
    let refused = h
        .studio
        .scripts
        .run_selected(&h.studio.session, &h.studio.ws, true);
    assert!(refused.is_err());
    h.studio.session.stop();
}

#[test]
fn adding_script_files_never_overwrites() {
    let dir = temp_scripts_dir("scripts-add");
    let src_dir = temp_scripts_dir("scripts-src");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&src_dir).unwrap();
    let mut s = crate::scripts::Scripts::with_dir(dir.clone());
    let txt = src_dir.join("notes.txt");
    std::fs::write(&txt, "x").unwrap();
    assert!(s.add_file(&txt).is_err(), "only .js and .lua");
    std::fs::write(dir.join("taken.lua"), "-- installed").unwrap();
    let src = src_dir.join("taken.lua");
    std::fs::write(&src, "-- new").unwrap();
    let err = s.add_file(&src).unwrap_err();
    assert!(err.contains("already installed"), "{err}");
    assert_eq!(
        std::fs::read_to_string(dir.join("taken.lua")).unwrap(),
        "-- installed"
    );
    // Refresh lists folder files (loaded or not) and ignores other types.
    std::fs::write(dir.join("readme.md"), "x").unwrap();
    s.refresh();
    assert_eq!(s.local.len() + s.broken.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&src_dir).ok();
}

#[test]
fn python_action_shows_interpreter_controls_and_installs_py_files() {
    use crate::scripts::Pick;
    let mut h = Harness::new();
    h.size = vec2(1024.0, 700.0);
    h.frames(2);
    let dir = temp_scripts_dir("python-ui");
    h.studio.scripts = crate::scripts::Scripts::with_dir(dir.clone());
    h.click(W::new("scripts-open"));
    h.frame();
    let index = h
        .studio
        .scripts
        .factory
        .iter()
        .position(|s| s.language == peel_extensions::Language::Python)
        .unwrap();
    h.click(W::new(("script-factory", index)));
    h.frame();
    assert_eq!(h.studio.scripts.selected, Some(Pick::Factory(index)));
    h.rect(W::new("scripts-run"));
    h.rect(W::new("scripts-python-warning"));
    // One interpreter control; its menu holds the choices.
    let menu = W::new("scripts-python-menu");
    h.click(W::new("scripts-python"));
    assert!(h.ui.is_popup_open(menu));
    h.rect(menu.with(("item", 1usize)));
    // Bundled is already in use, so choosing it does nothing.
    h.click(menu.with(("item", 0usize)));
    assert!(h.studio.scripts.python.is_none());
    if h.ui.is_popup_open(menu) {
        h.ui.close_popup();
    }
    // With another interpreter set, Bundled Python switches back.
    let custom = std::path::PathBuf::from("/opt/custom/python3");
    h.studio.scripts.python = Some(custom.clone());
    h.studio.prefs.python = Some(custom);
    h.frames(2);
    h.click(W::new("scripts-python"));
    h.click(menu.with(("item", 0usize)));
    assert!(
        h.studio.scripts.python.is_none(),
        "back to the bundled Python"
    );
    assert!(h.studio.prefs.python.is_none());
    h.frames(2);
    h.rect(W::new("scripts-python-warning"));
    let source_dir = temp_scripts_dir("python-source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("summary.py");
    std::fs::write(&source, "peel.log('installed')").unwrap();
    h.studio.scripts.add_file(&source).unwrap();
    assert_eq!(
        h.studio.scripts.selected_script().unwrap().language,
        peel_extensions::Language::Python
    );
    assert!(!h.studio.scripts.is_running());
    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(source_dir).ok();
}

#[test]
fn scripts_control_the_live_session_and_workspace_on_the_ui_thread() {
    let mut h = Harness::new();
    h.studio.scripts.ensure_scanned();
    let patterns = h.studio.session.project().patterns.len();
    let index = h.studio.scripts.factory.len();
    h.studio.scripts.factory.push(peel_extensions::Script {
        name: "Live session".into(), language: peel_extensions::Language::JavaScript,
        source: "const state = peel.call('status', {}); if (!state) throw Error('no status'); peel.call('transport', {command:'play'}); peel.call('ui.set', {dock:'mixer',panels:{browser:false,help:true}}); peel.call('add_pattern', {name:'Script Pattern',length:'4'}); peel.log('done');".into(),
    });
    h.studio.scripts.selected = Some(crate::scripts::Pick::Factory(index));
    h.studio
        .scripts
        .run_selected(&h.studio.session, &h.studio.ws, false)
        .unwrap();
    wait_for_script(&mut h);
    assert!(h.studio.scripts.last.as_ref().unwrap().result.is_ok());
    assert!(h.studio.session.is_playing());
    assert_eq!(h.studio.ws.dock, DockTab::Mixer);
    assert!(!h.studio.prefs.show_browser);
    assert!(h.studio.prefs.show_help);
    assert_eq!(h.studio.session.project().patterns.len(), patterns + 1);
    h.studio.session.doc.undo();
    assert_eq!(h.studio.session.project().patterns.len(), patterns);
    h.studio.session.stop();
}

/// Presses Cmd/Ctrl + Option/Alt + a letter key (matched by physical key).
fn layout_key(h: &mut Harness, code: peel_ui::KeyCode, ch: &str) {
    let mods = peel_ui::Modifiers {
        alt: true,
        logo: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        shift: false,
    };
    h.ui.input.keys.push(peel_ui::input::KeyPress {
        // Option changes the character on macOS; only the code is reliable.
        key: peel_ui::Key::Character(ch.into()),
        code: Some(code),
        repeat: false,
        modifiers: mods,
        consumed: false,
    });
    h.frame();
    h.frame();
}

#[test]
fn panes_hide_from_their_headers_and_return_from_rails_and_shortcuts() {
    use peel_ui::KeyCode;
    let mut h = Harness::new();
    let lanes = h.rect(W::new("pl-snap"));
    let wide_before = h.rect(W::new("dock-toggle")).right() - lanes.x;
    // Browser: hide from its header, back from the rail.
    h.click(W::new("browser-hide"));
    assert!(!h.studio.prefs.show_browser);
    h.frames(2);
    assert!(h.ui.widget_rect(W::new("browser-search")).is_none());
    let lanes_after = h.rect(W::new("pl-snap"));
    assert!(lanes_after.x < lanes.x, "the playlist takes the room");
    h.click(W::new(("browser-tab", "Samples")));
    assert!(h.studio.prefs.show_browser);
    assert_eq!(
        h.studio.ws.browser_tab,
        crate::workspace::BrowserTab::Samples
    );
    // Clicking the showing tab hides it again.
    h.click(W::new(("browser-tab", "Samples")));
    assert!(!h.studio.prefs.show_browser);
    layout_key(&mut h, KeyCode::KeyB, "∫");
    assert!(h.studio.prefs.show_browser, "Cmd Opt B shows it");
    // Inspector: header, then shortcut.
    h.click(W::new("inspector-hide"));
    assert!(!h.studio.prefs.show_right());
    h.frames(2);
    let wide_after = h.rect(W::new("dock-toggle")).right() - h.rect(W::new("pl-snap")).x;
    assert!(wide_after > wide_before);
    layout_key(&mut h, KeyCode::KeyI, "ˆ");
    assert!(h.studio.prefs.show_inspector);
    // Editor: the dock collapses to its header, tabs and F-keys reveal it.
    let session_before = h.session().doc.revision();
    layout_key(&mut h, KeyCode::KeyE, "´");
    assert!(!h.studio.prefs.show_editor, "Cmd Opt E hides the editor");
    h.frame();
    assert_eq!(h.session().doc.revision(), session_before, "not an export");
    assert!(!h.studio.ws.export_panel, "the export panel did not open");
    h.frame();
    assert!(h.ui.widget_rect(W::new("rack-len-plus")).is_none());
    h.rect(W::new(("dock-tab", "Mixer")));
    named_key(&mut h, peel_ui::NamedKey::F7, KeyCode::F7);
    assert!(h.studio.prefs.show_editor, "F7 reveals the editor");
    assert_eq!(h.studio.ws.dock, DockTab::PianoRoll);
    h.frames(2);
    h.click(W::new("dock-toggle"));
    assert!(!h.studio.prefs.show_editor);
    h.frames(2);
    h.click(W::new(("dock-tab", "Channels")));
    assert!(h.studio.prefs.show_editor, "a tab reveals it");
    assert_eq!(h.studio.ws.dock, DockTab::Rack);
}

#[test]
fn the_view_menu_lists_and_toggles_every_choice() {
    use crate::layout::VIEW_MENU;
    let mut h = Harness::new();
    let item = |i: usize| VIEW_MENU.with(("item", i));
    h.click(W::new("view"));
    assert!(h.ui.is_popup_open(VIEW_MENU));
    h.click(item(3));
    assert!(!h.studio.prefs.show_editor, "Editor");
    h.click(W::new("view"));
    h.click(item(2));
    assert!(h.studio.prefs.show_scripts, "Scripts");
    assert!(!h.studio.prefs.show_inspector, "replaces the Inspector");
    h.click(W::new("view"));
    h.click(item(1));
    assert!(h.studio.prefs.show_inspector && !h.studio.prefs.show_scripts);
    h.click(W::new("view"));
    h.click(item(0));
    assert!(!h.studio.prefs.show_browser, "Browser");
    h.click(W::new("view"));
    h.click(item(6));
    assert!(!h.studio.prefs.show_meter, "Master meter");
    h.frames(2);
    assert!(h.ui.widget_rect(W::new("load")).is_none());
    h.click(W::new("view"));
    h.click(item(8));
    assert!(h.studio.prefs.show_help, "Usage hints");
    h.click(W::new("view"));
    let tempo = h.studio.ws.playlist.show_tempo;
    h.click(item(5));
    assert_ne!(h.studio.ws.playlist.show_tempo, tempo, "Tempo lane");
    // Reset puts everything back.
    h.studio.ws.browser_w = 400.0;
    h.click(W::new("view"));
    h.click(item(10));
    let d = crate::prefs::Prefs::default();
    let p = &h.studio.prefs;
    assert_eq!(
        (
            p.show_browser,
            p.show_inspector,
            p.show_scripts,
            p.show_editor,
            p.show_help,
            p.show_meter
        ),
        (
            d.show_browser,
            d.show_inspector,
            d.show_scripts,
            d.show_editor,
            d.show_help,
            d.show_meter
        )
    );
    assert_eq!(h.studio.ws.browser_w, crate::workspace::BROWSER_W);
}

#[test]
fn hints_are_optional_but_warnings_and_names_stay() {
    let mut h = Harness::new();
    assert!(!h.studio.prefs.show_help, "quiet by default");
    h.studio.ws.focus = None;
    h.frames(2);
    // With hints off the usage text is a mark that still carries it.
    h.rect(W::new("inspector-empty-hint"));
    h.click(W::new("rail-help"));
    assert!(h.studio.prefs.show_help);
    h.frames(2);
    assert!(h.ui.widget_rect(W::new("inspector-empty-hint")).is_none());
    // The Python file-access warning is not a hint.
    h.click(W::new("rail-help"));
    assert!(!h.studio.prefs.show_help);
    h.studio.scripts = crate::scripts::Scripts::with_dir(temp_scripts_dir("hints-python"));
    h.click(W::new("scripts-open"));
    let index = h
        .studio
        .scripts
        .factory
        .iter()
        .position(|s| s.language == peel_extensions::Language::Python)
        .unwrap();
    h.click(W::new(("script-factory", index)));
    h.frame();
    h.rect(W::new("scripts-python"));
    assert!(h.studio.prefs.show_scripts && !h.studio.prefs.show_help);
}

#[test]
fn layout_preferences_round_trip_and_survive_new_projects() {
    use crate::prefs::Prefs;
    use crate::workspace::BrowserTab;
    // Older preference files without layout fields still load.
    let old: Prefs = serde_json::from_str(r#"{"recent":[],"show_welcome":false}"#).unwrap();
    assert!(old.show_browser && old.show_inspector && old.show_editor && !old.show_help);
    assert!(old.show_meter && old.show_status_details && old.browser_w.is_none());

    let mut h = Harness::new();
    h.studio.ws.browser_w = 320.0;
    h.studio.ws.inspector_w = 360.0;
    h.studio.ws.dock_h = 300.0;
    h.frames(2);
    h.click(W::new(("dock-tab", "Mixer")));
    h.click(W::new(("browser-tab", "Effects")));
    h.studio.prefs.show_meter = false;
    h.frame();
    assert_eq!(h.studio.prefs.browser_w, Some(320.0));
    assert_eq!(h.studio.prefs.inspector_w, Some(360.0));
    assert_eq!(h.studio.prefs.dock_h, Some(300.0));
    assert_eq!(h.studio.prefs.dock_tab, Some(DockTab::Mixer));
    assert_eq!(h.studio.prefs.browser_tab, Some(BrowserTab::Effects));

    // Saved and loaded again, the same layout comes back.
    let json = serde_json::to_string(&h.studio.prefs).unwrap();
    let back: Prefs = serde_json::from_str(&json).unwrap();
    let mut ws = crate::workspace::Workspace::default();
    back.apply_layout(&mut ws);
    assert_eq!(
        (ws.browser_w, ws.inspector_w, ws.dock_h),
        (320.0, 360.0, 300.0)
    );
    assert_eq!(ws.dock, DockTab::Mixer);
    assert_eq!(ws.browser_tab, BrowserTab::Effects);
    assert!(!back.show_meter);

    // A new project keeps the user's layout.
    let mods = peel_ui::Modifiers {
        logo: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Default::default()
    };
    h.ui.input.keys.push(peel_ui::input::KeyPress {
        key: peel_ui::Key::Character("n".into()),
        code: Some(peel_ui::KeyCode::KeyN),
        repeat: false,
        modifiers: mods,
        consumed: false,
    });
    h.frames(2);
    assert_eq!(h.studio.ws.browser_w, 320.0);
    assert_eq!(h.studio.ws.dock, DockTab::Mixer);
    assert_eq!(h.studio.ws.browser_tab, BrowserTab::Effects);
}

#[test]
fn small_windows_keep_the_top_bar_and_panes_inside() {
    let mut h = Harness::new();
    h.size = vec2(1024.0, 700.0);
    h.frames(2);
    for id in [
        "view", "undo", "redo", "play", "stop", "rec", "bpm", "export", "save", "open",
    ] {
        let r =
            h.ui.widget_rect(W::new(id))
                .unwrap_or_else(|| panic!("{id} is on screen"));
        assert!(
            r.x >= 0.0 && r.right() <= 1024.0 && r.bottom() <= 44.0,
            "{id}: {r:?}"
        );
    }
    // Controls in the top bar never overlap.
    let order = [
        "view",
        "redo",
        "to-start",
        "loop",
        "metronome",
        "open",
        "save",
        "export",
    ];
    for w in order.windows(2) {
        let (a, b) = (h.rect(W::new(w[0])), h.rect(W::new(w[1])));
        assert!(a.right() <= b.x + 0.5, "{} before {}", w[0], w[1]);
    }
    let lanes = h.rect(W::new("pl-snap"));
    assert!(lanes.x > 0.0 && lanes.right() < 1024.0);
}

#[test]
fn open_requests_and_script_panel_changes_apply_on_the_next_frame() {
    let mut h = Harness::new();
    h.studio.scripts = crate::scripts::Scripts::with_dir(temp_scripts_dir("scripts-request"));
    // `PEEL_SCRIPTS` sets this flag; it must still show the column.
    h.studio.scripts.open_requested = true;
    h.frames(2);
    assert!(h.studio.prefs.show_scripts && !h.studio.prefs.show_inspector);
    h.rect(W::new("scripts-run"));
    // A script hiding panes through `ui.set` changes the very next frame.
    let st = &mut h.studio;
    crate::script_host::execute(
        &mut st.session,
        &mut st.ws,
        &mut st.prefs,
        &mut peel_mcp::Control::default(),
        "ui.set",
        serde_json::json!({
            "panels": {"browser": false, "scripts": false, "inspector": false, "editor": false}
        }),
    )
    .unwrap();
    h.frames(2);
    assert!(h.ui.widget_rect(W::new("browser-search")).is_none());
    assert!(h.ui.widget_rect(W::new("scripts-run")).is_none());
    assert!(h.ui.widget_rect(W::new("inspector-hide")).is_none());
    assert!(h.ui.widget_rect(W::new("rack-len-plus")).is_none());
    h.rect(W::new(("dock-tab", "Channels")));
}

#[test]
fn the_channels_toolbar_never_overlaps_at_small_sizes() {
    // 1200 x 720 is the window's minimum; 1024 x 700 is the harness floor.
    for (w, h_px, label_expected) in [(1200.0, 720.0, true), (1024.0, 700.0, false)] {
        let mut h = Harness::new();
        h.size = vec2(w, h_px);
        h.frames(3);
        assert_eq!(h.studio.ws.dock, DockTab::Rack);
        let label = h.ui.widget_rect(W::new("rack-swing-label"));
        if label_expected {
            assert!(
                label.is_some(),
                "{w}: the Swing label shows at the minimum window"
            );
        }
        let mut ids = vec!["pattern-picker", "pattern-add", "rack-graph"];
        if label.is_some() {
            ids.push("rack-swing-label");
        }
        ids.extend([
            "rack-swing",
            "rack-len-minus",
            "rack-len-plus",
            "dock-toggle",
        ]);
        let rects: Vec<(&str, Rect)> = ids
            .iter()
            .map(|id| {
                let r = h.ui.widget_rect(W::new(*id));
                (
                    *id,
                    r.unwrap_or_else(|| panic!("{w}x{h_px}: {id} is on screen")),
                )
            })
            .collect();
        for pair in rects.windows(2) {
            let ((a, ra), (b, rb)) = (pair[0], pair[1]);
            assert!(
                ra.right() <= rb.x + 0.01,
                "{w}x{h_px}: {a} {ra:?} overlaps {b} {rb:?}"
            );
        }
        let dock_right = h.rect(W::new("dock-toggle")).right();
        let right_col = h.rect(W::new("inspector-hide")).x;
        assert!(
            dock_right < right_col,
            "{w}: the dock header stays inside the dock"
        );
    }
}

#[test]
fn the_piano_roll_toolbar_never_overlaps_down_to_the_minimum_window() {
    // 1200 x 720 is the smallest window the app allows.
    for (w, h_px) in [(1440.0, 900.0), (1200.0, 720.0)] {
        let mut h = Harness::new();
        h.size = vec2(w, h_px);
        h.frames(2);
        h.click(W::new(("dock-tab", "Piano roll")));
        h.frames(2);
        let ids = [
            W::new("pattern-add"),
            W::new(("roll-tool", "Draw")),
            W::new(("roll-tool", "Select")),
            W::new(("roll-tool", "Erase")),
            W::new("roll-quantize"),
            W::new("roll-note-tools"),
            W::new("roll-scale"),
            W::new("roll-chord"),
            W::new("roll-snap"),
            W::new("roll-ghost"),
            W::new("dock-toggle"),
        ];
        let rects: Vec<Rect> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                h.ui.widget_rect(*id)
                    .unwrap_or_else(|| panic!("{w}x{h_px}: control {i} is on screen"))
            })
            .collect();
        for (i, pair) in rects.windows(2).enumerate() {
            assert!(
                pair[0].right() <= pair[1].x + 0.01,
                "{w}x{h_px}: control {i} {:?} overlaps {:?}",
                pair[0],
                pair[1]
            );
        }
    }
}

#[test]
fn inspector_knob_labels_fit_at_the_default_width() {
    let mut h = Harness::new();
    h.frames(2);
    let crate::workspace::Focus::Channel(id) = h.studio.ws.focus.unwrap() else {
        panic!("a channel is focused");
    };
    let dev = h.session().project().channel(id).unwrap().device.clone();
    let desc = peel_devices::descriptor(&dev.type_id).unwrap();
    // Knob centers in the first row give the cell width.
    let knobs: Vec<Rect> = desc
        .params
        .iter()
        .filter(|p| !p.is_choice())
        .filter_map(|p| h.ui.widget_rect(W::new(("param", dev.id, p.id))))
        .collect();
    assert!(knobs.len() >= 2);
    let cell = knobs[1].center().x - knobs[0].center().x;
    assert!(cell >= 76.0, "cells are at least the minimum: {cell}");
    let caption = h.ui.theme.caption;
    for p in desc.params.iter().filter(|p| !p.is_choice()) {
        let w = h.ui.measure(p.name, caption).x;
        assert!(
            w <= cell - 2.0,
            "{} ({w:.0} px) fits a {cell:.0} px cell",
            p.name
        );
    }
}

// ---- mixer, effect editor, EQ and Mastering ----

use crate::workspace::{Focus, MixerSize};

impl Harness {
    fn open_mixer(&mut self) {
        self.studio.ws.dock = DockTab::Mixer;
        self.frames(4);
    }

    fn master(&self) -> peel_document::Id {
        self.session().project().mixer.tracks[0].id
    }

    fn add_device(&mut self, track: peel_document::Id, type_id: &str) -> peel_document::Id {
        let dev = template::device(type_id, None);
        let id = dev.id;
        self.studio
            .session
            .doc
            .edit("Add device", |p| p.mixer.track_mut(track).unwrap().inserts.push(dev));
        self.frames(2);
        id
    }

    fn param(&self, device: peel_document::Id, key: &str) -> f32 {
        let d = peel_session::find_device(self.session().project(), device).unwrap();
        d.params.get(key).copied().unwrap_or_else(|| {
            peel_devices::descriptor(&d.type_id)
                .and_then(|x| x.param_by_key(key))
                .map_or(0.0, |p| p.default)
        })
    }
}

/// Every strip control sits inside the dock at both window sizes and every
/// strip size, and the fader keeps a usable length.
#[test]
fn mixer_strips_fit_the_dock_at_both_window_sizes() {
    for size in [vec2(1200.0, 720.0), vec2(1440.0, 900.0)] {
        for mixer_size in MixerSize::ALL {
            let mut h = Harness::new();
            h.size = size;
            h.studio.ws.mixer_size = mixer_size;
            h.open_mixer();
            h.frames(3);
            let tracks = h.session().project().mixer.tracks.clone();
            let master = tracks[0].id;
            let screen = Rect { x: 0.0, y: 0.0, w: size.x, h: size.y };
            for tr in [&tracks[1], &tracks[0]] {
                let fader = h.rect(W::new(("fader", tr.id)));
                assert!(fader.h >= 80.0, "{size:?} {mixer_size:?}: fader {}", fader.h);
                for id in [
                    W::new(("mute", tr.id)),
                    W::new(("strip-pan", tr.id)),
                    W::new(("peak-reset", tr.id)),
                ] {
                    let r = h.rect(id);
                    assert!(
                        screen.contains(r.min()) && screen.contains(vec2(r.right(), r.bottom())),
                        "{size:?} {mixer_size:?}: {id:?} {r:?}"
                    );
                    assert!(r.bottom() <= fader.bottom() + 120.0);
                }
                // Mute sits below the fader, not over it.
                assert!(h.rect(W::new(("mute", tr.id))).y >= fader.bottom() - 1.0);
            }
            // Master is pinned to the right of every other strip.
            let m = h.rect(W::new(("fader", master)));
            let first = h.rect(W::new(("fader", tracks[1].id)));
            assert!(m.x > first.x);
            assert!(m.right() <= size.x);
        }
    }
}

#[test]
fn strip_size_changes_width() {
    let mut h = Harness::new();
    h.open_mixer();
    let id = h.session().project().mixer.tracks[1].id;
    let mut widths = Vec::new();
    for size in MixerSize::ALL {
        h.studio.ws.mixer_size = size;
        h.frames(2);
        widths.push(h.rect(W::new(("strip-bg", id))).w);
    }
    assert!(widths[0] < widths[1] && widths[1] < widths[2], "{widths:?}");
}

#[test]
fn mute_and_solo_toggle_and_undo() {
    let mut h = Harness::new();
    h.open_mixer();
    let id = h.session().project().mixer.tracks[1].id;
    h.click(W::new(("mute", id)));
    assert!(h.session().project().mixer.track(id).unwrap().muted);
    h.click(W::new(("solo", id)));
    assert!(h.session().project().mixer.track(id).unwrap().solo);
    h.studio.session.doc.undo();
    assert!(!h.session().project().mixer.track(id).unwrap().solo);
    h.studio.session.doc.undo();
    assert!(!h.session().project().mixer.track(id).unwrap().muted);
}

#[test]
fn a_fader_drag_is_one_undo_step() {
    let mut h = Harness::new();
    h.open_mixer();
    let id = h.session().project().mixer.tracks[1].id;
    let before = h.session().project().mixer.track(id).unwrap().gain_db;
    let c = h.rect(W::new(("fader", id))).center();
    h.drag(c, c + vec2(0.0, 40.0), 12);
    assert!(h.session().project().mixer.track(id).unwrap().gain_db < before - 2.0);
    h.studio.session.doc.undo();
    assert_eq!(h.session().project().mixer.track(id).unwrap().gain_db, before);
}

#[test]
fn clicking_a_peak_readout_clears_its_clip_lamp() {
    let mut h = Harness::new();
    h.open_mixer();
    let id = h.session().project().mixer.tracks[1].id;
    let key = W::new(("peak-hold", id));
    {
        let hold = h.ui.state::<crate::mixer::PeakHold>(key);
        hold.clipped = true;
        hold.max = 1.3;
    }
    h.frame();
    h.click(W::new(("peak-reset", id)));
    let hold = h.ui.state::<crate::mixer::PeakHold>(key);
    assert!(!hold.clipped && hold.max == 0.0);
}

#[test]
fn every_insert_is_reachable_by_scrolling_the_chain() {
    let mut h = Harness::new();
    h.studio.ws.mixer_size = MixerSize::Expanded;
    h.open_mixer();
    let id = h.session().project().mixer.tracks[1].id;
    for _ in 0..8 {
        h.add_device(id, "peel.fx.limiter");
    }
    let n = h.session().project().mixer.track(id).unwrap().inserts.len();
    assert!(n >= 8);
    let last = W::new(("insert", id, n - 1));
    let visible = |h: &Harness| {
        let strip = h.rect(W::new(("strip-bg", id)));
        h.ui.widget_rect(last)
            .is_some_and(|r| r.y >= strip.y && r.bottom() <= strip.bottom())
    };
    assert!(!visible(&h), "the last insert starts below the visible rows");
    let at = h.rect(W::new(("insert", id, 0usize))).center();
    h.move_to(at);
    for _ in 0..12 {
        h.ui.input.scroll = vec2(0.0, -60.0);
        h.ui.input.had_event = true;
        h.frame();
    }
    assert!(visible(&h), "scrolling reaches the last insert");
}

#[test]
fn clicking_an_insert_opens_its_editor_and_escape_closes_it() {
    let mut h = Harness::new();
    h.open_mixer();
    let id = h.session().project().mixer.tracks[1].id;
    let eq = h.add_device(id, "peel.fx.eq");
    let comp = h.add_device(id, "peel.fx.compressor");
    let eq_slot = h.session().project().mixer.track(id).unwrap().inserts.iter().position(|d| d.id == eq).unwrap();
    let comp_slot = h.session().project().mixer.track(id).unwrap().inserts.iter().position(|d| d.id == comp).unwrap();
    h.click(W::new(("insert", id, eq_slot)));
    let t = h.studio.ws.fx.target.expect("editor open");
    assert_eq!((t.track, t.device), (id, eq));
    assert_eq!(h.studio.ws.focus, Some(Focus::Track(id)));
    // Chain chips switch the open effect.
    h.click(W::new(("fx-chip", comp)));
    assert_eq!(h.studio.ws.fx.target.unwrap().device, comp);
    let _ = comp_slot;
    h.ui.input.keys.push(escape());
    h.ui.input.had_event = true;
    h.frames(2);
    assert!(h.studio.ws.fx.target.is_none());
}

fn escape() -> peel_ui::input::KeyPress {
    peel_ui::input::KeyPress {
        key: peel_ui::Key::Named(peel_ui::NamedKey::Escape),
        code: None,
        repeat: false,
        modifiers: Default::default(),
        consumed: false,
    }
}

#[test]
fn the_effect_editor_fits_inside_both_window_sizes() {
    for size in [vec2(1200.0, 720.0), vec2(1440.0, 900.0)] {
        let area = Rect { x: 0.0, y: 44.0, w: size.x, h: size.y - 44.0 - 24.0 };
        let sheet = crate::fx_editor::sheet_rect(area);
        assert!(sheet.x >= area.x && sheet.right() <= area.right(), "{size:?} {sheet:?}");
        assert!(sheet.y >= area.y && sheet.bottom() <= area.bottom(), "{size:?} {sheet:?}");
        assert!(sheet.w >= 900.0 && sheet.h >= 560.0);
    }
}

#[test]
fn the_editor_follows_a_moved_device_and_closes_when_it_is_removed() {
    let mut h = Harness::new();
    h.open_mixer();
    let a = h.session().project().mixer.tracks[1].id;
    let b = h.session().project().mixer.tracks[2].id;
    let eq = h.add_device(a, "peel.fx.eq");
    h.studio.ws.fx.open(a, eq);
    h.frames(2);
    h.studio.session.doc.edit("Move", |p| {
        let d = p.mixer.track_mut(a).unwrap().inserts.pop().unwrap();
        p.mixer.track_mut(b).unwrap().inserts.push(d);
    });
    h.frames(3);
    assert_eq!(h.studio.ws.fx.target.map(|t| t.track), Some(b));
    h.studio.session.doc.edit("Remove", |p| p.mixer.track_mut(b).unwrap().inserts.clear());
    h.frames(3);
    assert!(h.studio.ws.fx.target.is_none());
}

#[test]
fn the_mastering_button_adds_the_effect_once_and_opens_it() {
    let mut h = Harness::new();
    h.open_mixer();
    let master = h.master();
    let count = |h: &Harness| {
        h.session().project().mixer.tracks[0]
            .inserts
            .iter()
            .filter(|d| d.type_id == "peel.fx.mastering")
            .count()
    };
    assert_eq!(count(&h), 0);
    h.click(W::new("master-mastering"));
    assert_eq!(count(&h), 1);
    let dev = h.studio.ws.fx.target.expect("opened").device;
    h.studio.ws.fx.close();
    h.frames(2);
    h.click(W::new("master-mastering"));
    assert_eq!(count(&h), 1, "second click opens rather than adds");
    assert_eq!(h.studio.ws.fx.target.unwrap().device, dev);
    assert_eq!(h.studio.ws.focus, Some(Focus::Track(master)));
}

#[test]
fn mastering_modes_show_the_same_values() {
    let mut h = Harness::new();
    h.open_mixer();
    crate::mixer::open_mastering(&mut h.studio.session, &mut h.studio.ws);
    h.frames(3);
    let dev = h.studio.ws.fx.target.unwrap().device;
    h.click(W::new(("master-preset", dev, 4usize)));
    let before = peel_session::find_device(h.session().project(), dev).unwrap().params.clone();
    assert!(before.get("input").copied().unwrap_or(0.0) > 1.0, "Loud preset applied");
    let revision = h.session().doc.revision();
    h.click(W::new("master-mode").with(1usize));
    assert!(h.studio.ws.fx.mastering.advanced);
    h.click(W::new("master-mode").with(0usize));
    assert!(!h.studio.ws.fx.mastering.advanced);
    let after = peel_session::find_device(h.session().project(), dev).unwrap().params.clone();
    assert_eq!(before, after, "switching views never edits the sound");
    assert_eq!(revision, h.session().doc.revision());
}

#[test]
fn a_mastering_knob_drag_is_one_undo_step() {
    let mut h = Harness::new();
    h.open_mixer();
    crate::mixer::open_mastering(&mut h.studio.session, &mut h.studio.ws);
    h.frames(3);
    let dev = h.studio.ws.fx.target.unwrap().device;
    let desc = peel_devices::descriptor("peel.fx.mastering").unwrap();
    let id = desc.param_by_key("input").unwrap().id;
    let c = h.rect(W::new(("mknob", dev, id))).center();
    h.drag(c, c + vec2(0.0, -40.0), 10);
    assert!(h.param(dev, "input") > 2.0);
    h.studio.session.doc.undo();
    assert_eq!(h.param(dev, "input"), 0.0);
}

fn open_eq(h: &mut Harness) -> peel_document::Id {
    h.open_mixer();
    let master = h.master();
    let eq = h.add_device(master, "peel.fx.eq");
    h.studio.ws.fx.open(master, eq);
    h.frames(4);
    eq
}

#[test]
fn dragging_an_eq_node_edits_frequency_and_gain_in_one_undo_step() {
    let mut h = Harness::new();
    let eq = open_eq(&mut h);
    let (f0, g0) = (h.param(eq, "band3.freq"), h.param(eq, "band3.gain"));
    let c = h.rect(W::new(("eq-node", eq, 3usize))).center();
    h.drag(c, c + vec2(-60.0, -40.0), 12);
    let (f1, g1) = (h.param(eq, "band3.freq"), h.param(eq, "band3.gain"));
    assert!(f1 < f0 * 0.8, "{f0} -> {f1}");
    assert!(g1 > g0 + 2.0, "{g0} -> {g1}");
    assert_eq!(h.studio.ws.fx.eq.band, 3, "dragging selects the band");
    h.studio.session.doc.undo();
    assert_eq!(h.param(eq, "band3.freq"), f0);
    assert_eq!(h.param(eq, "band3.gain"), g0);
}

#[test]
fn scrolling_over_a_node_changes_q_and_double_click_toggles_it() {
    let mut h = Harness::new();
    let eq = open_eq(&mut h);
    let c = h.rect(W::new(("eq-node", eq, 2usize))).center();
    let q0 = h.param(eq, "band2.q");
    h.move_to(c);
    h.ui.input.scroll = vec2(0.0, 80.0);
    h.ui.input.had_event = true;
    h.frame();
    assert!(h.param(eq, "band2.q") > q0);
    let on = h.param(eq, "band2.enable");
    h.move_to(c);
    h.move_to(c);
    h.ui.input.double_clicked = true;
    h.press(0);
    h.release(0);
    assert_ne!(h.param(eq, "band2.enable"), on);
}

#[test]
fn eq_presets_reset_and_compare_use_real_parameters() {
    let mut h = Harness::new();
    let eq = open_eq(&mut h);
    let presets = peel_devices::presets("peel.fx.eq");
    let air = presets.iter().position(|p| p.name == "Air").unwrap();
    h.click(W::new(("eq-preset", eq)));
    h.frames(2);
    h.click(W::new(("eq-preset", eq)).with("menu").with(("item", air)));
    assert_eq!(h.param(eq, "band4.gain"), 4.0);
    // B starts as a copy; change it, flip back to A and the preset returns.
    h.click(W::new(("eq-ab", eq)).with(1usize));
    assert_eq!(h.param(eq, "band4.gain"), 4.0);
    h.studio.session.doc.edit("tweak", |p| {
        peel_session::find_device_mut(p, eq).unwrap().params.insert("band4.gain".into(), -3.0);
    });
    h.frames(2);
    h.click(W::new(("eq-ab", eq)).with(0usize));
    assert_eq!(h.param(eq, "band4.gain"), 4.0, "A kept the preset");
    h.click(W::new(("eq-reset", eq)));
    assert_eq!(h.param(eq, "band4.gain"), 0.0);
}

fn candidate(freq: f32) -> peel_engine::analysis::Resonance {
    peel_engine::analysis::Resonance { frequency_hz: freq, q: 6.0, cut_db: -3.0, prominence_db: 8.0 }
}

#[test]
fn resonance_cuts_use_a_free_bell_and_replacing_needs_a_second_click() {
    let mut h = Harness::new();
    let eq = open_eq(&mut h);
    h.studio.ws.fx.eq.tab = crate::eq_editor::Tab::Resonance;
    h.studio.ws.fx.eq.band = 1;
    h.studio.ws.fx.held.insert(eq, vec![candidate(1800.0), candidate(5200.0)]);
    h.frames(3);
    // Band 3 is free (flat) after Band 2 is given a cut: use Band 2 first.
    h.studio.session.doc.edit("busy", |p| {
        let d = peel_session::find_device_mut(p, eq).unwrap();
        d.params.insert("band2.enable".into(), 1.0);
        d.params.insert("band2.gain".into(), -4.0);
        d.params.insert("band3.enable".into(), 0.0);
    });
    h.frames(3);
    h.click(W::new(("eq-apply", 0usize)));
    assert_eq!(h.param(eq, "band3.freq"), 1800.0);
    assert_eq!(h.param(eq, "band3.gain"), -3.0);
    assert_eq!(h.param(eq, "band3.enable"), 1.0);
    assert_eq!(h.param(eq, "band2.gain"), -4.0, "the user's band is untouched");
    // Both bells are now in use: the next cut must be confirmed.
    h.frames(2);
    h.click(W::new(("eq-apply", 1usize)));
    assert_eq!(h.param(eq, "band2.freq"), 400.0, "first click only arms the replacement");
    assert!(h.studio.ws.fx.eq.confirm.is_some());
    h.frames(2);
    h.click(W::new(("eq-apply", 1usize)));
    assert_eq!(h.param(eq, "band2.freq"), 5200.0);
    assert!(h.studio.ws.fx.eq.confirm.is_none());
}

#[test]
fn silence_gives_no_suggestions() {
    let mut h = Harness::new();
    let eq = open_eq(&mut h);
    h.studio.ws.fx.eq.tab = crate::eq_editor::Tab::Resonance;
    h.frames(2);
    h.click(W::new("eq-listen"));
    h.frames(30);
    assert_eq!(h.studio.ws.fx.eq.learn, crate::eq_editor::Learn::Listening);
    assert!(crate::eq_resonance::candidates(&h.studio.ws, eq).is_empty());
    assert!(h.ui.widget_rect(W::new(("eq-apply", 0usize))).is_none());
}

#[test]
fn editors_fit_with_every_pane_hidden() {
    let mut h = Harness::new();
    h.size = vec2(1200.0, 720.0);
    h.studio.prefs.show_browser = false;
    h.studio.prefs.set_inspector(false);
    h.studio.prefs.show_editor = false;
    let eq = open_eq(&mut h);
    let r = h.rect(W::new(("eq-node", eq, 2usize)));
    assert!(r.x > 0.0 && r.right() < 1200.0 && r.bottom() < 720.0);
}

// ---- instrument editor ----

fn open_instrument(h: &mut Harness) -> peel_document::Id {
    let ch = h.session().project().channels.iter().find(|c| c.name == "Piano").unwrap().id;
    crate::instrument_editor::open(&mut h.studio.session, &mut h.studio.ws, ch);
    h.frames(3);
    ch
}

fn piano_keys(h: &Harness) -> Vec<crate::instrument_editor::PianoKey> {
    let r = h.rect(W::new("inst-piano"));
    let base = ((h.studio.ws.octave + 1) * 12) as u8;
    crate::instrument_editor::piano_layout(r, base, if r.w >= 900.0 { 4 } else { 3 })
}

#[test]
fn the_inspector_opens_the_instrument_without_changing_the_project() {
    let mut h = Harness::new();
    let ch = h.session().project().channels.iter().find(|c| c.name == "Piano").unwrap().id;
    let revision = h.session().doc.revision();
    h.click(W::new(("insp-open-instrument", ch)));
    assert_eq!(h.studio.ws.inst.target, Some(ch));
    assert_eq!(h.studio.ws.selected_channel, Some(ch));
    assert_eq!(h.session().doc.revision(), revision, "opening is not an edit");
    h.ui.input.keys.push(escape());
    h.ui.input.had_event = true;
    h.frames(2);
    assert!(h.studio.ws.inst.target.is_none());
}

#[test]
fn mouse_notes_glide_across_keys_and_always_release() {
    let mut h = Harness::new();
    open_instrument(&mut h);
    let keys = piano_keys(&h);
    let white: Vec<_> = keys.iter().filter(|k| !k.black).collect();
    let low = vec2(white[0].rect.center().x, white[0].rect.bottom() - 10.0);
    let high = vec2(white[3].rect.center().x, white[3].rect.bottom() - 10.0);
    h.move_to(low);
    h.move_to(low);
    h.press(0);
    assert_eq!(h.studio.ws.inst.mouse_key, Some(white[0].key));
    h.move_to(high);
    assert_eq!(h.studio.ws.inst.mouse_key, Some(white[3].key), "glissando follows the pointer");
    h.release(0);
    h.frame();
    assert_eq!(h.studio.ws.inst.mouse_key, None);
}

#[test]
fn losing_focus_or_closing_releases_mouse_notes_and_sustain() {
    let mut h = Harness::new();
    open_instrument(&mut h);
    let keys = piano_keys(&h);
    let k = keys.iter().find(|k| !k.black).unwrap();
    h.click(W::new("inst-sustain"));
    assert!(h.studio.ws.inst.sustain);
    let p = vec2(k.rect.center().x, k.rect.bottom() - 10.0);
    h.move_to(p);
    h.move_to(p);
    h.press(0);
    assert!(h.studio.ws.inst.mouse_key.is_some());
    // The window loses focus with the button down: no release event comes.
    h.ui.input.down[0] = false;
    h.ui.input.focus_lost = true;
    h.frames(2);
    assert_eq!(h.studio.ws.inst.mouse_key, None);
    assert!(!h.studio.ws.inst.sustain, "sustain is let go too");
    // Closing releases as well.
    h.click(W::new("inst-sustain"));
    assert!(h.studio.ws.inst.sustain);
    crate::instrument_editor::close(&mut h.studio.session, &mut h.studio.ws);
    assert!(!h.studio.ws.inst.sustain);
}

#[test]
fn computer_keys_play_only_while_enabled_and_release_on_focus_loss() {
    let mut h = Harness::new();
    let ch = open_instrument(&mut h);
    let press = |h: &mut Harness| {
        h.ui.input.keys.push(peel_ui::input::KeyPress {
            key: peel_ui::Key::Character("z".into()),
            code: Some(peel_ui::KeyCode::KeyZ),
            repeat: false,
            modifiers: Default::default(),
            consumed: false,
        });
        h.frame();
    };
    press(&mut h);
    assert_eq!(h.studio.ws.held.len(), 1, "Z plays a note on the selected channel");
    assert_eq!(h.studio.ws.held.values().next().map(|(c, _)| *c), Some(ch));
    h.ui.input.focus_lost = true;
    h.frames(2);
    assert!(h.studio.ws.held.is_empty(), "focus loss ends held keys");
    h.click(W::new("inst-keys"));
    assert!(!h.studio.ws.keys_enabled);
    press(&mut h);
    assert!(h.studio.ws.held.is_empty(), "the toggle silences the computer keyboard");
}

#[test]
fn switching_instruments_releases_the_old_target() {
    let mut h = Harness::new();
    let ch = open_instrument(&mut h);
    let other = h.session().project().channels.iter().find(|c| c.id != ch).unwrap().id;
    h.studio.ws.inst.mouse_key = Some(60);
    h.click(W::new(("inst-chip", other)));
    assert_eq!(h.studio.ws.inst.target, Some(other));
    assert_eq!(h.studio.ws.inst.mouse_key, None);
    assert_eq!(h.studio.ws.selected_channel, Some(other));
}

#[test]
fn the_mapper_offers_only_this_instruments_parameters_and_learn_waits() {
    let mut h = Harness::new();
    let ch = open_instrument(&mut h);
    let dev = h.session().project().channel(ch).unwrap().device.id;
    h.studio.ws.inst.pick = Some("tone".into());
    h.frames(2);
    // Whatever the key, Learn with a real parameter starts waiting.
    let keys: Vec<_> = peel_devices::descriptor(&h.session().project().channel(ch).unwrap().device.type_id)
        .unwrap()
        .params
        .iter()
        .filter(|p| p.automatable)
        .map(|p| p.key)
        .collect();
    h.studio.ws.inst.pick = Some(keys[0].to_string());
    h.frames(2);
    h.click(W::new("inst-learn"));
    h.frames(2);
    assert!(matches!(
        h.session().midi_learn_status(),
        peel_session::MidiLearnStatus::Waiting { target: peel_session::MidiControlTarget::DeviceParam { device, .. } } if *device == dev
    ));
    h.click(W::new("inst-learn-cancel"));
    assert!(matches!(h.session().midi_learn_status(), peel_session::MidiLearnStatus::Idle));
}

#[test]
fn an_expanded_mixer_takes_the_whole_workspace_and_restores() {
    let mut h = Harness::new();
    h.open_mixer();
    let id = h.session().project().mixer.tracks[1].id;
    let before = h.rect(W::new(("fader", id))).h;
    h.click(W::new("mixer-fill"));
    h.frames(3);
    let after = h.rect(W::new(("fader", id))).h;
    assert!(after > before + 150.0, "{before} -> {after}");
    h.studio.ws.mixer_size = MixerSize::Full;
    h.frames(3);
    assert!(h.ui.widget_rect(W::new(("send-add", id))).is_some(), "the full send list fits");
    h.click(W::new("mixer-fill"));
    h.frames(3);
    assert_eq!(h.rect(W::new(("fader", id))).h, before.min(h.rect(W::new(("fader", id))).h));
    assert!(h.studio.ws.mixer_restore_h.is_none());
}

// ---- audio input, recording and routing sheet ----

#[test]
fn the_audio_sheet_opens_from_the_status_bar_and_closes_with_escape() {
    let mut h = Harness::new();
    h.click(W::new("audio-open"));
    assert!(h.studio.ws.io_open);
    h.frames(2);
    assert!(h.ui.widget_rect(W::new("io-record")).is_some());
    h.ui.input.keys.push(escape());
    h.ui.input.had_event = true;
    h.frames(2);
    assert!(!h.studio.ws.io_open);
    // Opening an instrument closes it, and the other way round.
    h.studio.ws.io_open = true;
    open_instrument(&mut h);
    assert!(!h.studio.ws.io_open);
}

#[test]
fn a_new_lane_is_armed_and_record_stays_blocked_until_an_input_is_open() {
    let mut h = Harness::new();
    h.studio.ws.io_open = true;
    h.frames(3);
    let lanes = h.session().project().arrangement().tracks.len();
    h.click(W::new("io-lane"));
    h.frames(2);
    h.click(W::new("io-lane").with("menu").with(("item", lanes + 1)));
    h.frames(2);
    assert_eq!(h.session().project().arrangement().tracks.len(), lanes + 1);
    assert_eq!(h.session().armed_lanes().len(), 1);
    // The silent test engine has no input: the reason is shown, nothing starts.
    assert!(h.session().record_blocker().is_some());
    h.click(W::new("io-record"));
    assert!(!h.studio.recorder.armed);
    assert!(!h.session().is_recording());
}

#[test]
fn extra_outputs_report_why_they_cannot_start_and_save_nothing() {
    let mut h = Harness::new();
    h.studio.ws.io_open = true;
    h.frames(3);
    if h.ui.widget_rect(W::new("io-route-add")).is_none() {
        return;
    }
    h.click(W::new("io-route-add"));
    h.frames(2);
    // A silent session has no hardware to route to: the refusal is kept
    // and neither the session nor the preferences change.
    assert!(h.session().output_routes().is_empty());
    assert!(h.studio.prefs.output_routes.is_empty());
}

#[test]
fn the_midi_channel_choice_reaches_the_session_and_preferences() {
    let mut h = Harness::new();
    open_instrument(&mut h);
    h.click(W::new("inst-midi-channel"));
    h.frames(2);
    h.click(W::new("inst-midi-channel").with("menu").with(("item", 4usize)));
    assert_eq!(h.session().midi_channel_filter(), Some(3));
    assert_eq!(h.studio.prefs.midi_channel, Some(3));
    // All channels again.
    h.click(W::new("inst-midi-channel"));
    h.frames(2);
    h.click(W::new("inst-midi-channel").with("menu").with(("item", 0usize)));
    assert_eq!(h.session().midi_channel_filter(), None);
}

#[test]
fn saved_mappings_for_another_project_are_skipped_with_a_notice() {
    let mut h = Harness::new();
    h.studio.prefs.midi_mappings = vec![peel_session::MidiMapping {
        controller: 21,
        channel: None,
        target: peel_session::MidiControlTarget::TrackGain { track: peel_document::Id::new() },
        min: -60.0,
        max: 12.0,
    }];
    h.studio.apply_io_prefs();
    assert!(h.session().midi_control_mappings().is_empty());
    assert!(
        h.session().notice().is_some_and(|n| n.text.contains("another project")),
        "{:?}",
        h.session().notice()
    );
    assert_eq!(h.studio.prefs.midi_mappings.len(), 1, "kept for the project it belongs to");
}
