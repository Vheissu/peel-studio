use super::*;
use peel_session::template;

fn setup() -> (Session, Recorder, Id, Id) {
    let mut s = Session::new_silent(template::beat_and_melody());
    let pid = s.selected_pattern();
    let channel = s
        .project()
        .channels
        .iter()
        .find(|c| c.name == "Piano")
        .unwrap()
        .id;
    s.doc.edit("Clear", |p| {
        let pt = p.pattern_mut(pid).unwrap();
        pt.notes.retain(|n| n.channel != channel);
        pt.retain_expression();
    });
    let mut r = Recorder {
        armed: true,
        ..Default::default()
    };
    r.begin(&s, pid);
    (s, r, pid, channel)
}
fn event(
    s: &mut Session,
    r: &mut Recorder,
    ch: Id,
    voice: u32,
    beat: f64,
    kind: RecordedPerformanceKind,
) {
    let latency = Recorder::latency_beats(s);
    r.consume(
        s,
        RecordedPerformance {
            channel: Some(ch),
            voice,
            midi_channel: Some(1),
            beat: beat + latency,
            playing: true,
            sustain: false,
            time_ns: 0,
            kind,
        },
    );
}
fn on(s: &mut Session, r: &mut Recorder, ch: Id, voice: u32, beat: f64) {
    event(
        s,
        r,
        ch,
        voice,
        beat,
        RecordedPerformanceKind::NoteOn {
            key: 60,
            velocity: 0.8,
        },
    );
}
fn off(s: &mut Session, r: &mut Recorder, ch: Id, voice: u32, beat: f64) {
    event(
        s,
        r,
        ch,
        voice,
        beat,
        RecordedPerformanceKind::NoteOff {
            key: 60,
            velocity: 0.3,
        },
    );
}
fn expression(
    s: &mut Session,
    r: &mut Recorder,
    ch: Id,
    voice: u32,
    beat: f64,
    pitch: f32,
    pressure: f32,
) {
    event(
        s,
        r,
        ch,
        voice,
        beat,
        RecordedPerformanceKind::Expression {
            pitch,
            pressure,
            timbre: 0.5,
        },
    );
}

#[test]
fn equal_pitches_keep_independent_saved_expression_and_one_undo() {
    let (mut s, mut r, pid, ch) = setup();
    let before = s.project().clone();
    on(&mut s, &mut r, ch, 1, 1.0);
    on(&mut s, &mut r, ch, 2, 1.0);
    expression(&mut s, &mut r, ch, 1, 1.25, 7.0, 0.8);
    expression(&mut s, &mut r, ch, 2, 1.5, -3.0, 0.2);
    off(&mut s, &mut r, ch, 1, 2.0);
    off(&mut s, &mut r, ch, 2, 2.0);
    r.finish(&mut s);
    let pt = s.project().pattern(pid).unwrap();
    let recorded: Vec<_> = pt.notes_for(ch).collect();
    assert_eq!(recorded.len(), 2);
    let curves: Vec<_> = recorded
        .iter()
        .map(|n| pt.expression_for(n.id).unwrap().at(Time::new(3, 4)))
        .collect();
    assert!(curves.iter().any(|e| e.pitch == 7.0 && e.pressure == 0.8));
    assert!(curves.iter().any(|e| e.pitch == -3.0 && e.pressure == 0.2));
    assert!(
        peel_document::validate(s.project())
            .iter()
            .all(|i| !i.is_error())
    );
    let recorded = s.project().clone();
    s.doc.undo();
    let mut undone = s.project().clone();
    undone.revision = before.revision;
    assert_eq!(undone, before);
    s.doc.redo();
    let mut redone = s.project().clone();
    redone.revision = recorded.revision;
    assert_eq!(redone, recorded);
}

#[test]
fn a_held_note_and_its_curves_continue_after_the_pattern_wrap() {
    let (mut s, mut r, pid, ch) = setup();
    assert_eq!(s.project().pattern(pid).unwrap().length, Time::beats(4));
    on(&mut s, &mut r, ch, 7, 3.5);
    expression(&mut s, &mut r, ch, 7, 3.75, 2.0, 0.7);
    expression(&mut s, &mut r, ch, 7, 0.125, 4.0, 0.9);
    off(&mut s, &mut r, ch, 7, 0.5);
    r.finish(&mut s);
    let pt = s.project().pattern(pid).unwrap();
    let notes: Vec<_> = pt.notes_for(ch).collect();
    assert_eq!(notes.len(), 2);
    let start = notes.iter().find(|n| n.start == Time::new(7, 2)).unwrap();
    assert_eq!(start.end(), Time::beats(4));
    let continuation = notes.iter().find(|n| n.start == Time::ZERO).unwrap();
    assert_eq!(continuation.length, Time::new(1, 2));
    let curve = pt.expression_for(continuation.id).unwrap();
    assert_eq!(curve.at(Time::ZERO).pitch, 2.0);
    assert_eq!(curve.at(Time::new(1, 4)).pitch, 4.0);
    assert!(
        peel_document::validate(s.project())
            .iter()
            .all(|i| !i.is_error())
    );
}

#[test]
fn ending_a_take_closes_held_notes_and_keeps_the_last_pressure() {
    let (mut s, mut r, pid, ch) = setup();
    on(&mut s, &mut r, ch, 7, 1.0);
    expression(&mut s, &mut r, ch, 7, 1.5, 3.0, 0.8);
    r.finish(&mut s);
    let pt = s.project().pattern(pid).unwrap();
    let note = pt.notes_for(ch).next().unwrap();
    assert_eq!(note.length, Time::new(1, 2));
    assert!(r.held.is_empty());
    assert!(r.take.is_none());
    assert_eq!(s.doc.undo_label(), Some("Record notes"));
}

#[test]
fn a_reset_closes_only_the_matching_instrument_and_rebases_seek_time() {
    let (mut s, mut r, pid, ch) = setup();
    let other = s.project().channels.iter().find(|c| c.id != ch).unwrap().id;
    on(&mut s, &mut r, ch, 7, 1.0);
    on(&mut s, &mut r, other, 8, 1.0);
    event(&mut s, &mut r, ch, 0, 2.0, RecordedPerformanceKind::Reset);
    assert!(r.held.contains_key(&(other, 8)));
    on(&mut s, &mut r, ch, 9, 0.25);
    off(&mut s, &mut r, ch, 9, 0.5);
    r.finish(&mut s);
    let pt = s.project().pattern(pid).unwrap();
    assert!(
        pt.notes_for(ch)
            .any(|n| n.start == Time::new(1, 4) && n.length == Time::new(1, 4))
    );
}

#[test]
fn stopped_and_invalid_expression_never_enter_a_take() {
    let (mut s, mut r, pid, ch) = setup();
    r.consume(
        &mut s,
        RecordedPerformance {
            channel: Some(ch),
            voice: 1,
            midi_channel: None,
            beat: 0.0,
            playing: false,
            sustain: false,
            time_ns: 0,
            kind: RecordedPerformanceKind::NoteOn {
                key: 60,
                velocity: 0.8,
            },
        },
    );
    assert!(r.held.is_empty());
    on(&mut s, &mut r, ch, 1, 1.0);
    expression(&mut s, &mut r, ch, 1, 1.25, f32::NAN, 0.5);
    expression(&mut s, &mut r, ch, 1, 1.25, 121.0, 0.5);
    off(&mut s, &mut r, ch, 1, 2.0);
    r.finish(&mut s);
    let pt = s.project().pattern(pid).unwrap();
    let note = pt.notes_for(ch).next().unwrap();
    assert_eq!(
        pt.expression_for(note.id).unwrap().at(Time::new(1, 2)),
        ExpressionValue::default()
    );
}

#[test]
fn recording_latency_uses_the_tempo_map_across_a_ramp() {
    let (mut s, mut r, _, _) = setup();
    s.doc.edit("Tempo", |p| {
        p.tempo = vec![
            peel_document::TempoSegment {
                start: Time::ZERO,
                bpm: 60.0,
                ramp_to: Some(180.0),
            },
            peel_document::TempoSegment {
                start: Time::beats(4),
                bpm: 180.0,
                ramp_to: None,
            },
        ];
    });
    r.begin(&s, s.selected_pattern());
    let take = r.take.as_mut().unwrap();
    take.timing.latency_seconds = 0.1;
    let raw = 2.0;
    let heard = take.clock.position(raw, None, &take.timing);
    let want = take
        .timing
        .tempo
        .seconds_to_beat(take.timing.tempo.beat_to_seconds(raw) - 0.1);
    assert!((heard - want).abs() < 1e-9);
    assert!(
        (heard - (raw - 0.1)).abs() > 0.05,
        "must not subtract a fixed-tempo beat offset"
    );
}

#[test]
fn timestamps_recover_unobserved_whole_loops() {
    let timing = TakeTiming {
        length: 4.0,
        tempo: TempoMap::constant(120.0),
        latency_seconds: 0.0,
    };
    let mut clock = Clock::default();
    assert_eq!(clock.position(1.0, Some(1_000_000_000), &timing), 1.0);
    assert_eq!(clock.position(1.5, Some(7_250_000_000), &timing), 13.5);
}

#[test]
fn software_notes_and_controllers_record_once_through_the_shared_feed() {
    let (mut s, mut r, pid, ch) = setup();
    s.set_live_target(Some(ch));
    s.play();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while s.position() <= 0.01 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    r.frame(&mut s);
    s.live_note(ch, 60, 0.8, true);
    assert!(s.pitch_bend(0.5));
    assert!(s.channel_pressure(0.75));
    r.frame(&mut s);
    std::thread::sleep(std::time::Duration::from_millis(20));
    s.live_note(ch, 60, 0.0, false);
    r.frame(&mut s);
    s.stop();
    r.frame(&mut s);
    let pt = s.project().pattern(pid).unwrap();
    let notes: Vec<_> = pt.notes_for(ch).collect();
    assert_eq!(notes.len(), 1, "keyboard notes must not be recorded twice");
    let curve = pt.expression_for(notes[0].id).unwrap();
    assert!(curve.pitch.iter().any(|p| p.value == 1.0));
    assert!(curve.pressure.iter().any(|p| p.value == 0.75));
    assert!(
        peel_document::validate(s.project())
            .iter()
            .all(|i| !i.is_error())
    );
}

#[test]
fn disarming_after_a_ui_stall_closes_the_entire_held_performance() {
    let (mut s, mut r, pid, ch) = setup();
    s.doc.edit("Short loop", |p| {
        p.pattern_mut(pid).unwrap().length = Time::new(1, 4)
    });
    r.begin(&s, pid);
    s.set_live_target(Some(ch));
    s.play();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while s.position() < 0.01 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    r.frame(&mut s);
    s.live_note(ch, 60, 0.8, true);
    r.frame(&mut s);
    std::thread::sleep(std::time::Duration::from_millis(400));
    r.armed = false;
    r.frame(&mut s);
    let notes: Vec<_> = s.project().pattern(pid).unwrap().notes_for(ch).collect();
    assert!(
        notes.len() >= 3,
        "a stalled UI must not truncate whole loops: {}",
        notes.len()
    );
    let duration: f64 = notes.iter().map(|n| n.length.as_f64()).sum();
    assert!(duration > 0.6, "held duration {duration}");
    assert!(r.take.is_none());
    s.live_note(ch, 60, 0.0, false);
    s.stop();
}

#[test]
fn a_tempo_edit_finishes_the_take_instead_of_reusing_a_stale_clock() {
    let (mut s, mut r, pid, ch) = setup();
    s.set_live_target(Some(ch));
    s.play();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while s.position() < 0.01 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    r.frame(&mut s);
    s.live_note(ch, 60, 0.8, true);
    r.frame(&mut s);
    s.doc.edit("Tempo", |p| p.tempo[0].bpm = 170.0);
    r.frame(&mut s);
    assert!(!r.armed);
    assert!(r.take.is_none());
    assert_eq!(s.project().pattern(pid).unwrap().notes_for(ch).count(), 1);
    assert!(
        peel_document::validate(s.project())
            .iter()
            .all(|i| !i.is_error())
    );
    s.stop();
}
