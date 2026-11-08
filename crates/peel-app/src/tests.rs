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
    for seed in [1u64, 7, 42] {
        let mut h = Harness::new();
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
