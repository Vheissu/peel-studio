mod common;
use common::{TrackBuilder, smf};
use midly::num::{u4, u7};
use midly::{Format, MetaMessage, MidiMessage, TrackEventKind};
use peel_document::{ExpressionValue, Note, Pattern, Project, Time, validate};
use peel_midi::{ImportOptions, MergeOptions, MidiError, Quantize, SustainPolicy, import_smf};

fn pressure(t: TrackBuilder, tick: u64, ch: u8, value: u8) -> TrackBuilder {
    t.raw(
        tick,
        TrackEventKind::Midi {
            channel: u4::new(ch),
            message: MidiMessage::ChannelAftertouch {
                vel: u7::new(value),
            },
        },
    )
}
fn poly(t: TrackBuilder, tick: u64, ch: u8, key: u8, value: u8) -> TrackBuilder {
    t.raw(
        tick,
        TrackEventKind::Midi {
            channel: u4::new(ch),
            message: MidiMessage::Aftertouch {
                key: u7::new(key),
                vel: u7::new(value),
            },
        },
    )
}
fn zone(t: TrackBuilder, manager: u8, members: u8) -> TrackBuilder {
    t.cc(0, manager, 101, 0)
        .cc(0, manager, 100, 6)
        .cc(0, manager, 6, members)
}
fn sensitivity(t: TrackBuilder, tick: u64, ch: u8, semitones: u8, cents: u8) -> TrackBuilder {
    t.cc(tick, ch, 101, 0)
        .cc(tick, ch, 100, 0)
        .cc(tick, ch, 6, semitones)
        .cc(tick, ch, 38, cents)
}
fn at(pattern: &Pattern, note: &Note, offset: Time) -> ExpressionValue {
    pattern
        .expression_for(note.id)
        .map_or_else(ExpressionValue::default, |e| e.at(offset))
}
fn near(a: f32, b: f32) {
    assert!((a - b).abs() < 0.01, "{a} != {b}");
}

#[test]
fn ordinary_bend_and_pressure_follow_notes_and_poly_pressure_stays_local() {
    let t = sensitivity(TrackBuilder::new(), 0, 2, 12, 50)
        .bend(0, 2, 4096)
        .on(0, 2, 60, 100)
        .on(10, 2, 64, 90);
    let t = pressure(t, 20, 2, 32);
    let t = poly(t, 30, 2, 60, 100);
    let t = pressure(t, 40, 2, 64)
        .off(80, 2, 60)
        .off(80, 2, 64)
        .on(100, 2, 60, 100)
        .off(160, 2, 60);
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions::default(),
    )
    .unwrap();
    let p = &i.patterns[0];
    let n = &p.notes[0];
    near(at(p, n, Time::ZERO).pitch, 6.25);
    near(at(p, n, Time::new(1, 2)).pressure, 100.0 / 127.0);
    near(at(p, &p.notes[1], Time::new(1, 2)).pressure, 64.0 / 127.0);
    near(at(p, &p.notes[2], Time::ZERO).pressure, 64.0 / 127.0);
    assert!(i.report.unsupported.iter().all(|u| !u.description.contains("Aftertouch") && !u.description.contains("Pitch Bend")));
}

#[test]
fn lower_zone_collapses_members_including_channel_ten_and_combines_manager_controls() {
    let t = zone(TrackBuilder::new(), 0, 9);
    let t = sensitivity(t, 0, 1, 24, 0);
    // Last member sensitivity applies across the zone, including channel 2.
    let t = sensitivity(t, 0, 9, 12, 0)
        .bend(0, 0, 4096)
        .bend(0, 1, 4096)
        .bend(0, 9, -4096);
    let t = pressure(pressure(t, 0, 0, 20), 0, 1, 40)
        .cc(0, 9, 74, 127)
        .on(10, 1, 60, 100)
        .on(10, 9, 60, 100)
        .off_vel(100, 1, 60, 99)
        .off(100, 9, 60)
        .bend(150, 1, 0);
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(i.channels.len(), 1);
    assert_eq!(i.channels[0].source.drum_key, None);
    let p = &i.patterns[0];
    assert_eq!(p.notes[0].channel, p.notes[1].channel);
    let values: Vec<_> = p.notes.iter().map(|n| at(p, n, Time::ZERO)).collect();
    assert!(
        values
            .iter()
            .any(|v| (v.pitch - 7.0).abs() < 0.01 && (v.pressure - 60.0 / 127.0).abs() < 0.001)
    );
    assert!(
        values
            .iter()
            .any(|v| (v.pitch + 5.0).abs() < 0.01 && v.timbre == 1.0)
    );
    assert!(
        p.expression
            .iter()
            .all(|e| e.pitch.iter().all(|x| x.offset <= Time::new(9, 10)))
    );
    let mut project = Project::empty("MPE");
    i.merge_into(&mut project, &MergeOptions::default(), |p, _| {
        p.mixer.master().id
    })
    .unwrap();
    assert!(!validate(&project).iter().any(|issue| issue.is_error()));
}

#[test]
fn upper_zone_defaults_and_manager_sustain_extend_note_expression_and_lift() {
    let t = zone(TrackBuilder::new(), 15, 2)
        .bend(0, 14, 4096)
        .on(0, 14, 64, 100)
        .cc(20, 15, 64, 127)
        .off_vel(40, 14, 64, 30)
        .bend(80, 14, -4096)
        .cc(120, 15, 64, 0);
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions {
            sustain: SustainPolicy::ExtendNotes,
            ..ImportOptions::default()
        },
    )
    .unwrap();
    let p = &i.patterns[0];
    let n = &p.notes[0];
    assert_eq!(i.channels[0].source.midi_channel, 15);
    assert_eq!(n.length, Time::new(6, 5));
    near(n.release, 30.0 / 127.0);
    near(at(p, n, Time::ZERO).pitch, 24.0);
    near(at(p, n, Time::BEAT).pitch, -24.0);
}

#[test]
fn type_one_controls_cross_tracks_but_never_cross_ports() {
    let controls = sensitivity(TrackBuilder::new(), 0, 1, 12, 0)
        .bend(0, 1, 4096)
        .build(Some(200));
    let first = TrackBuilder::new().note(10, 100, 1, 60, 100).build(None);
    let other_port = TrackBuilder::new()
        .meta(0, MetaMessage::MidiPort(u7::new(4)))
        .note(10, 100, 1, 64, 100)
        .build(None);
    let i = import_smf(
        &smf(Format::Parallel, 100, vec![controls, first, other_port]),
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(i.channels.len(), 2);
    near(
        at(&i.patterns[0], &i.patterns[0].notes[0], Time::ZERO).pitch,
        6.0,
    );
    assert!(i.patterns[1].expression.is_empty());
    assert_eq!(i.channels[1].source.midi_port, 4);
}

#[test]
fn type_two_sequences_do_not_share_controls_or_zones() {
    let first = zone(TrackBuilder::new(), 0, 2)
        .bend(0, 1, 4096)
        .note(10, 100, 1, 60, 100)
        .build(None);
    let second = TrackBuilder::new().note(10, 100, 1, 60, 100).build(None);
    let i = import_smf(
        &smf(Format::Sequential, 100, vec![first, second]),
        &ImportOptions::default(),
    )
    .unwrap();
    near(
        at(&i.patterns[0], &i.patterns[0].notes[0], Time::ZERO).pitch,
        24.0,
    );
    assert!(i.patterns[1].expression.is_empty());
    assert_eq!(i.channels[1].source.midi_channel, 1);
}

#[test]
fn rpn_lsb_only_nrpn_and_reset_are_handled_without_future_poly_pressure() {
    let t = TrackBuilder::new()
        .cc(0, 0, 101, 0)
        .cc(0, 0, 100, 0)
        .cc(0, 0, 38, 50)
        .bend(0, 0, 4096)
        .on(0, 0, 60, 100);
    let t = poly(t, 10, 0, 60, 127)
        .cc(20, 0, 121, 0)
        .bend(30, 0, 4096)
        .cc(40, 0, 99, 1)
        .cc(40, 0, 6, 100)
        .off(100, 0, 60);
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions::default(),
    )
    .unwrap();
    let p = &i.patterns[0];
    let n = &p.notes[0];
    near(at(p, n, Time::ZERO).pitch, 1.25);
    near(at(p, n, Time::new(1, 4)).pressure, 0.0);
    near(at(p, n, Time::new(1, 2)).pitch, 1.25);
    assert!(
        i.report
            .unsupported
            .iter()
            .any(|u| u.description.contains("Data Entry MSB"))
    );
}

#[test]
fn shortened_quantized_gate_discards_later_controller_values() {
    let t = TrackBuilder::new()
        .on(0, 0, 60, 100)
        .bend(20, 0, 4096)
        .bend(120, 0, -4096)
        .off(130, 0, 60);
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions {
            quantize: Quantize::StartsAndLengths(Time::BEAT),
            ..ImportOptions::default()
        },
    )
    .unwrap();
    let p = &i.patterns[0];
    let n = &p.notes[0];
    assert_eq!(n.length, Time::BEAT);
    near(at(p, n, Time::BEAT).pitch, 1.0);
    assert!(
        p.expression[0]
            .pitch
            .iter()
            .all(|point| point.offset <= n.length)
    );
}

#[test]
fn dense_controller_stream_fails_before_exceeding_model_point_limit() {
    let mut t = TrackBuilder::new().on(0, 0, 60, 100);
    for tick in 1..=65_537 {
        t = t.bend(tick, 0, if tick % 2 == 0 { 4096 } else { -4096 });
    }
    t = t.off(70_000, 0, 60);
    let err = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, MidiError::TooLarge(_)));
}

#[test]
fn zone_overlap_closes_reassigned_notes_and_disabling_resets_departing_members() {
    let t = zone(TrackBuilder::new(), 0, 5)
        .bend(0, 5, 4096)
        .note(10, 150, 5, 60, 100)
        .cc(50, 15, 101, 0)
        .cc(50, 15, 100, 6)
        .cc(50, 15, 6, 12)
        .note(60, 90, 5, 64, 100)
        .cc(100, 15, 101, 0)
        .cc(100, 15, 100, 6)
        .cc(100, 15, 6, 0)
        .note(110, 140, 5, 67, 100);
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions::default(),
    )
    .unwrap();
    let p = &i.patterns[0];
    let first = p.notes.iter().find(|n| n.key == 60).unwrap();
    assert_eq!(first.length, Time::new(2, 5));
    let second = p.notes.iter().find(|n| n.key == 64).unwrap();
    let third = p.notes.iter().find(|n| n.key == 67).unwrap();
    assert_eq!(at(p, second, Time::ZERO), ExpressionValue::default());
    assert_eq!(at(p, third, Time::ZERO), ExpressionValue::default());
    assert_ne!(first.channel, second.channel);
    assert_ne!(second.channel, third.channel);
}

#[test]
fn member_tracks_in_one_port_share_an_instrument_but_keep_separate_patterns() {
    let setup = zone(TrackBuilder::new(), 0, 2).build(Some(200));
    let first = TrackBuilder::new()
        .bend(0, 1, 4096)
        .note(10, 100, 1, 60, 100)
        .build(None);
    let second = TrackBuilder::new()
        .bend(0, 2, -4096)
        .note(10, 100, 2, 60, 100)
        .build(None);
    let i = import_smf(
        &smf(Format::Parallel, 100, vec![setup, first, second]),
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(i.channels.len(), 1);
    assert_eq!(i.patterns.len(), 2);
    assert_eq!(
        i.patterns[0].notes[0].channel,
        i.patterns[1].notes[0].channel
    );
    near(
        at(&i.patterns[0], &i.patterns[0].notes[0], Time::ZERO).pitch,
        24.0,
    );
    near(
        at(&i.patterns[1], &i.patterns[1].notes[0], Time::ZERO).pitch,
        -24.0,
    );
}

#[test]
fn later_track_zone_setup_precedes_parallel_note_on_at_the_same_tick() {
    let notes = TrackBuilder::new().note(0, 100, 1, 60, 100).build(None);
    let setup = zone(TrackBuilder::new(), 0, 2)
        .bend(0, 1, 4096)
        .build(Some(100));
    let i = import_smf(
        &smf(Format::Parallel, 100, vec![notes, setup]),
        &ImportOptions::default(),
    )
    .unwrap();
    let p = &i.patterns[0];
    let n = &p.notes[0];
    assert_eq!(n.length, Time::BEAT);
    near(at(p, n, Time::ZERO).pitch, 24.0);
    assert_eq!(i.channels[0].source.midi_channel, 0);
}

#[test]
fn repeated_parallel_zone_setup_does_not_cut_notes_but_same_track_reset_does() {
    let first = zone(TrackBuilder::new(), 0, 2)
        .note(0, 100, 1, 60, 100)
        .build(None);
    let second = zone(TrackBuilder::new(), 0, 2)
        .note(0, 100, 2, 64, 100)
        .build(None);
    let i = import_smf(
        &smf(Format::Parallel, 100, vec![first, second]),
        &ImportOptions::default(),
    )
    .unwrap();
    assert!(
        i.patterns
            .iter()
            .flat_map(|p| &p.notes)
            .all(|n| n.length == Time::BEAT)
    );
    for reset in [120, 6] {
        let t = TrackBuilder::new().on(0, 1, 60, 100);
        let t = if reset == 120 {
            t.cc(0, 1, 120, 0)
        } else {
            t.cc(0, 0, 101, 0).cc(0, 0, 100, 6).cc(0, 0, 6, 2)
        };
        let i = import_smf(
            &smf(
                Format::SingleTrack,
                100,
                vec![t.off(100, 1, 60).build(None)],
            ),
            &ImportOptions::default(),
        )
        .unwrap();
        assert_eq!(i.patterns[0].notes[0].length, Time::new(1, 100));
    }
}

#[test]
fn range_changes_during_a_note_update_all_zone_members_and_accept_mcm_lsb() {
    let t = zone(TrackBuilder::new(), 0, 2)
        .cc(0, 0, 38, 0)
        .bend(0, 1, 4096)
        .bend(0, 2, 4096)
        .on(0, 1, 60, 100)
        .on(0, 2, 64, 100);
    let t = sensitivity(t, 50, 1, 12, 0).off(100, 1, 60).off(100, 2, 64);
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions::default(),
    )
    .unwrap();
    let p = &i.patterns[0];
    for n in &p.notes {
        near(at(p, n, Time::new(3, 4)).pitch, 6.0);
    }
    assert!(
        i.report.unsupported.is_empty(),
        "{:?}",
        i.report.unsupported
    );
}

#[test]
fn dense_controls_do_not_visit_thousands_of_unrelated_channel_notes() {
    let mut t = TrackBuilder::new();
    for i in 0..12_000 {
        t = t
            .on(0, 1, (i % 128) as u8, 100)
            .off(1000, 1, (i % 128) as u8);
    }
    t = t.on(0, 0, 60, 100).off(1000, 0, 60);
    for tick in 1..=800 {
        t = t.bend(tick, 0, if tick % 2 == 0 { 4096 } else { -4096 });
    }
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(i.report.total_notes, 12_001);
    assert_eq!(i.patterns[0].expression.len(), 1);
}

#[test]
fn curves_whose_only_changes_are_after_shortened_gate_are_removed() {
    let t = pressure(TrackBuilder::new().on(0, 0, 60, 100), 120, 0, 100).off(130, 0, 60);
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions {
            quantize: Quantize::StartsAndLengths(Time::BEAT),
            ..ImportOptions::default()
        },
    )
    .unwrap();
    assert!(i.patterns[0].expression.is_empty());
}

#[test]
fn poly_pressure_does_not_visit_other_pitches_on_the_same_channel() {
    let mut t = TrackBuilder::new();
    for i in 0..12_000 {
        t = t
            .on(0, 1, (i % 127) as u8, 100)
            .off(1000, 1, (i % 127) as u8);
    }
    t = t.on(0, 1, 127, 100).off(1000, 1, 127);
    for tick in 1..=800 {
        t = poly(t, tick, 1, 127, if tick % 2 == 0 { 100 } else { 20 });
    }
    let i = import_smf(
        &smf(Format::SingleTrack, 100, vec![t.build(None)]),
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(i.report.total_notes, 12_001);
    assert_eq!(i.patterns[0].expression.len(), 1);
}
