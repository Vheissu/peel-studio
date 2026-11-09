mod common;

use std::sync::Arc;

use midly::num::{u4, u7};
use midly::{Format, MidiMessage, TrackEventKind};
use peel_document::{
    Channel, Clip, ClipContent, DeviceInstance, ExpressionPoint, ExpressionValue, Id, Note,
    NoteExpression, PALETTE, Pattern, Project, Time, Track, validate,
};
use peel_midi::{
    ExportFormat, ExportOptions, ImportOptions, MergeOptions, MidiImport, PatternLayout,
    export_arrangement, export_pattern, import_smf,
};

use common::{TrackBuilder, smf};

fn channel(project: &mut Project, name: &str) -> Id {
    let id = Id::new();
    project.channels.push(Channel {
        id,
        name: name.into(),
        color: PALETTE[0],
        device: DeviceInstance::new("peel.synth.poly", 1),
        track: project.mixer.master().id,
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        root_key: 60,
    });
    id
}

fn point(offset: Time, value: f32) -> ExpressionPoint {
    ExpressionPoint { offset, value }
}

fn add_pattern(
    project: &mut Project,
    length: Time,
    notes: Vec<Note>,
    expression: Vec<NoteExpression>,
) -> Id {
    let mut pattern = Pattern {
        id: Id::new(),
        name: "Expression".into(),
        color: PALETTE[0],
        length,
        notes,
        expression,
    };
    pattern.sort_notes();
    let id = pattern.id;
    project.patterns.push(Arc::new(pattern));
    id
}

fn merge(import: MidiImport) -> Project {
    let mut project = Project::empty("Imported expression");
    project.patterns.clear();
    import
        .merge_into(&mut project, &MergeOptions::new_project(), |p, _| {
            p.mixer.master().id
        })
        .unwrap();
    let errors: Vec<_> = validate(&project)
        .into_iter()
        .filter(|issue| issue.is_error())
        .collect();
    assert!(errors.is_empty(), "invalid round-trip project: {errors:?}");
    project
}

fn value(pattern: &Pattern, note: &Note, offset: Time) -> ExpressionValue {
    pattern
        .expression_for(note.id)
        .map_or_else(ExpressionValue::default, |expression| expression.at(offset))
}

fn close(actual: f32, expected: f32, tolerance: f32) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected}, got {actual} (tolerance {tolerance})"
    );
}

fn expression_close(actual: ExpressionValue, expected: ExpressionValue) {
    // MIDI 1.0 quantizes pitch to 14 bits and pressure/slide to 7 bits.
    close(actual.pitch, expected.pitch, 0.02);
    close(actual.pressure, expected.pressure, 1.0 / 127.0);
    close(actual.timbre, expected.timbre, 1.0 / 127.0);
}

#[test]
fn same_key_voices_keep_independent_curves_and_lift_through_both_file_formats() {
    let mut source = Project::empty("Two voices");
    let ch = channel(&mut source, "Lead");
    let mut first = Note::new(ch, Time::ZERO, Time::beats(2), 62, 0.4);
    first.release = 0.25;
    let mut second = Note::new(ch, Time::ZERO, Time::beats(2), 62, 0.8);
    second.release = 0.75;
    let curves = vec![
        NoteExpression {
            note: first.id,
            pitch: vec![point(Time::ZERO, 2.0), point(Time::BEAT, 7.0)],
            pressure: vec![point(Time::ZERO, 0.3), point(Time::BEAT, 0.6)],
            timbre: vec![point(Time::ZERO, 0.2), point(Time::BEAT, 0.4)],
        },
        NoteExpression {
            note: second.id,
            pitch: vec![point(Time::ZERO, -3.0), point(Time::BEAT, -9.0)],
            pressure: vec![point(Time::ZERO, 0.8), point(Time::BEAT, 0.1)],
            timbre: vec![point(Time::ZERO, 0.9), point(Time::BEAT, 0.7)],
        },
    ];
    let pattern_id = add_pattern(&mut source, Time::beats(2), vec![first, second], curves);
    let original = source.pattern(pattern_id).unwrap();

    for format in [ExportFormat::Type0, ExportFormat::Type1] {
        let out = export_pattern(
            &source,
            pattern_id,
            &ExportOptions {
                format,
                ..ExportOptions::default()
            },
        )
        .unwrap();
        let back = merge(import_smf(&out.bytes, &ImportOptions::default()).unwrap());
        assert_eq!(back.channels.len(), 1, "MPE members are one instrument");
        assert_eq!(back.patterns.len(), 1);
        let pattern = &back.patterns[0];
        assert_eq!(pattern.notes.len(), 2);
        let mut notes: Vec<_> = pattern.notes.iter().collect();
        notes.sort_by(|a, b| a.velocity.total_cmp(&b.velocity));
        for (old, new) in [first, second].iter().zip(notes) {
            assert_eq!(new.key, old.key);
            assert_eq!((new.start, new.length), (old.start, old.length));
            close(new.velocity, old.velocity, 1.0 / 127.0);
            close(new.release, old.release, 1.0 / 127.0);
            for offset in [Time::ZERO, Time::new(1, 2), Time::BEAT, Time::new(3, 2)] {
                expression_close(value(pattern, new, offset), value(original, old, offset));
            }
        }
    }
}

#[test]
fn reused_mpe_members_start_with_fine_pitch_and_neutral_controllers() {
    let mut source = Project::empty("Reused member");
    let ch = channel(&mut source, "Lead");
    let mut expressive = Note::new(ch, Time::ZERO, Time::BEAT, 62, 0.7);
    expressive.fine = 25.0;
    let mut fine_only = Note::new(ch, Time::BEAT, Time::BEAT, 62, 0.7);
    fine_only.fine = -25.0;
    let mut notes = vec![expressive, fine_only];
    // More notes than member channels guarantees reuse even when allocation
    // prefers idle members to leave earlier release tails alone.
    notes.extend((2..16).map(|beat| Note::new(ch, Time::beats(beat), Time::BEAT, 62, 0.7)));
    let expression = NoteExpression {
        note: expressive.id,
        pitch: vec![point(Time::ZERO, 2.0)],
        pressure: vec![point(Time::ZERO, 0.9)],
        timbre: vec![point(Time::ZERO, 0.9)],
    };
    let pattern_id = add_pattern(&mut source, Time::beats(16), notes, vec![expression]);
    let out = export_pattern(&source, pattern_id, &ExportOptions::default()).unwrap();
    let back = merge(import_smf(&out.bytes, &ImportOptions::default()).unwrap());
    let pattern = &back.patterns[0];
    assert_eq!(pattern.notes.len(), 16);
    let first = value(pattern, &pattern.notes[0], Time::ZERO);
    close(first.pitch, 2.25, 0.02);
    close(first.pressure, 0.9, 1.0 / 127.0);
    close(first.timbre, 0.9, 1.0 / 127.0);
    let second = value(pattern, &pattern.notes[1], Time::ZERO);
    close(second.pitch, -0.25, 0.02);
    assert_eq!((second.pressure, second.timbre), (0.0, 0.5));
    for note in &pattern.notes[2..] {
        assert_eq!(
            value(pattern, note, Time::ZERO),
            ExpressionValue::default(),
            "a reused member must not inherit the previous note's controls"
        );
    }
}

#[test]
fn ordinary_bend_and_both_aftertouch_messages_survive_an_mpe_round_trip() {
    let track = TrackBuilder::new()
        .cc(0, 0, 101, 0)
        .cc(0, 0, 100, 0)
        .cc(0, 0, 6, 12)
        .cc(0, 0, 38, 50)
        .bend(0, 0, 4096)
        .cc(0, 0, 74, 24)
        .raw(
            0,
            TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::ChannelAftertouch { vel: u7::new(32) },
            },
        )
        .on(0, 0, 60, 90)
        .on(0, 0, 64, 100)
        .raw(
            240,
            TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::Aftertouch {
                    key: u7::new(60),
                    vel: u7::new(96),
                },
            },
        )
        .bend(480, 0, -4096)
        .off(960, 0, 60)
        .off(960, 0, 64)
        .build(None);
    let bytes = smf(Format::SingleTrack, 480, vec![track]);
    let original = merge(import_smf(&bytes, &ImportOptions::default()).unwrap());
    let pattern = &original.patterns[0];
    assert_eq!(pattern.notes.len(), 2);
    let lower = pattern.notes.iter().find(|note| note.key == 60).unwrap();
    let upper = pattern.notes.iter().find(|note| note.key == 64).unwrap();
    close(value(pattern, lower, Time::ZERO).pitch, 6.25, 0.01);
    close(value(pattern, lower, Time::BEAT).pitch, -6.25, 0.01);
    close(
        value(pattern, lower, Time::new(1, 2)).pressure,
        96.0 / 127.0,
        0.0001,
    );
    close(
        value(pattern, upper, Time::new(1, 2)).pressure,
        32.0 / 127.0,
        0.0001,
    );
    let out = export_pattern(&original, pattern.id, &ExportOptions::default()).unwrap();
    let restored = merge(import_smf(&out.bytes, &ImportOptions::default()).unwrap());
    let restored_pattern = &restored.patterns[0];
    for old in &pattern.notes {
        let new = restored_pattern
            .notes
            .iter()
            .find(|note| note.key == old.key)
            .unwrap();
        for offset in [Time::ZERO, Time::new(1, 2), Time::BEAT] {
            expression_close(
                value(restored_pattern, new, offset),
                value(pattern, old, offset),
            );
        }
    }
}

#[test]
fn type1_ports_keep_multiple_expressive_instruments_separate() {
    let mut source = Project::empty("Multiple instruments");
    let lead = channel(&mut source, "Lead");
    let bass = channel(&mut source, "Bass");
    let notes = vec![
        Note::new(lead, Time::ZERO, Time::BEAT, 62, 0.4),
        Note::new(bass, Time::ZERO, Time::BEAT, 62, 0.8),
    ];
    let expression = notes
        .iter()
        .zip([3.0, -5.0])
        .map(|(note, pitch)| NoteExpression {
            note: note.id,
            pitch: vec![point(Time::ZERO, pitch)],
            ..NoteExpression::default()
        })
        .collect();
    let pattern_id = add_pattern(&mut source, Time::BEAT, notes, expression);
    let type0 = export_pattern(
        &source,
        pattern_id,
        &ExportOptions {
            format: ExportFormat::Type0,
            ..ExportOptions::default()
        },
    );
    assert!(
        type0.is_err(),
        "type 0 must not combine independent MPE instruments on one port"
    );
    let out = export_pattern(&source, pattern_id, &ExportOptions::default()).unwrap();
    let back = merge(
        import_smf(
            &out.bytes,
            &ImportOptions {
                layout: PatternLayout::Combined,
                ..ImportOptions::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(back.channels.len(), 2);
    assert_eq!(back.patterns.len(), 1);
    let pattern = &back.patterns[0];
    assert_eq!(pattern.notes.len(), 2);
    let mut notes: Vec<_> = pattern.notes.iter().collect();
    notes.sort_by(|a, b| a.velocity.total_cmp(&b.velocity));
    assert_ne!(notes[0].channel, notes[1].channel);
    close(value(pattern, notes[0], Time::ZERO).pitch, 3.0, 0.02);
    close(value(pattern, notes[1], Time::ZERO).pitch, -5.0, 0.02);
}

#[test]
fn arrangement_offset_chases_expression_and_loop_cuts_keep_their_final_values() {
    let mut source = Project::empty("Trimmed loops");
    let ch = channel(&mut source, "Lead");
    let note = Note::new(ch, Time::ZERO, Time::beats(4), 62, 0.7);
    let expression = NoteExpression {
        note: note.id,
        pitch: vec![
            point(Time::ZERO, 0.0),
            point(Time::BEAT, 4.0),
            point(Time::beats(2), 8.0),
            point(Time::beats(3), 12.0),
        ],
        pressure: vec![point(Time::ZERO, 0.2), point(Time::beats(2), 0.8)],
        timbre: vec![point(Time::ZERO, 0.3), point(Time::beats(3), 0.7)],
    };
    let pattern_id = add_pattern(&mut source, Time::beats(4), vec![note], vec![expression]);
    let lane = Track {
        id: Id::new(),
        name: "Lead".into(),
        color: None,
        muted: false,
    };
    let clip = Clip {
        id: Id::new(),
        track: lane.id,
        start: Time::beats(2),
        length: Time::beats(6),
        offset: Time::BEAT,
        muted: false,
        content: ClipContent::Pattern {
            pattern: pattern_id,
        },
    };
    source.arrangement_mut().tracks.push(lane);
    source.arrangement_mut().clips.push(clip);
    let out = export_arrangement(&source, &ExportOptions::default()).unwrap();
    let back = merge(import_smf(&out.bytes, &ImportOptions::default()).unwrap());
    let pattern = &back.patterns[0];
    assert_eq!(pattern.notes.len(), 2);
    let first = &pattern.notes[0];
    let second = &pattern.notes[1];
    assert_eq!(
        (first.start, first.length),
        (Time::beats(2), Time::beats(3))
    );
    assert_eq!(
        (second.start, second.length),
        (Time::beats(5), Time::beats(3))
    );
    close(value(pattern, first, Time::ZERO).pitch, 4.0, 0.02);
    close(value(pattern, first, Time::BEAT).pitch, 8.0, 0.02);
    close(value(pattern, first, Time::beats(2)).pitch, 12.0, 0.02);
    close(value(pattern, second, Time::ZERO).pitch, 0.0, 0.02);
    close(value(pattern, second, Time::BEAT).pitch, 4.0, 0.02);
    close(value(pattern, second, Time::beats(2)).pitch, 8.0, 0.02);
    close(value(pattern, first, Time::BEAT).pressure, 0.8, 1.0 / 127.0);
    close(
        value(pattern, second, Time::BEAT).pressure,
        0.2,
        1.0 / 127.0,
    );
}
