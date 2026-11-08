mod common;

use midly::{Format, Fps, MetaMessage};
use peel_document::{Document, Id, Project, Time, TimeSignature, validate};
use peel_midi::{
    ImportOptions, MergeOptions, MidiError, PatternLayout, Quantize, SmfFormat, SustainPolicy,
    import_smf,
};

use common::{TrackBuilder, smf, smf_smpte};

fn count(report: &peel_midi::ImportReport, description: &str) -> u64 {
    report
        .unsupported
        .iter()
        .find(|u| u.description == description)
        .map_or(0, |u| u.count)
}

#[test]
fn type1_tempo_meter_names_and_exact_triplets() {
    // 480 PPQ. Triplet eighths are 160 ticks, quintuplet sixteenths 96.
    let conductor = TrackBuilder::new()
        .name(0, "My Song")
        .tempo(0, 500_000)
        .signature(0, 6, 3) // 6/8
        .tempo(1920, 400_000)
        .signature(3840, 4, 2)
        .build(Some(3840));
    let lead = TrackBuilder::new()
        .name(0, "Lead")
        .note(0, 160, 0, 60, 100)
        .note(160, 320, 0, 62, 80)
        .note(320, 480, 0, 64, 127)
        .note(480, 576, 0, 65, 1)
        .note(576, 672, 0, 67, 64)
        .build(Some(1000));
    let bass = TrackBuilder::new()
        .name(0, "Bass")
        .program(0, 1, 33)
        .note(0, 960, 1, 36, 90)
        .build(None);
    let bytes = smf(Format::Parallel, 480, vec![conductor, lead, bass]);

    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();
    let r = &import.report;
    assert_eq!(r.format, SmfFormat::Type1);
    assert_eq!(r.ticks_per_quarter, Some(480));
    assert_eq!(r.total_notes, 6);
    assert_eq!(import.title.as_deref(), Some("My Song"));

    // Tempo: 120 BPM, then 150 BPM exactly at beat 4.
    assert_eq!(import.tempo.len(), 2);
    assert_eq!(import.tempo[0].start, Time::ZERO);
    assert_eq!(import.tempo[0].bpm, 120.0);
    assert_eq!(import.tempo[1].start, Time::beats(4));
    assert_eq!(import.tempo[1].bpm, 150.0);
    assert_eq!(r.tempo_changes, 1);

    // The first signature goes to the project, later ones to the report.
    assert_eq!(
        import.time_signature,
        TimeSignature {
            numerator: 6,
            denominator: 8
        }
    );
    assert_eq!(r.time_signatures.len(), 2);
    assert_eq!(r.time_signatures[1].0, Time::beats(8));
    assert_eq!(
        count(
            r,
            "Time Signature change after the first (listed in report.time_signatures)"
        ),
        1
    );

    // Two note-bearing tracks, one pattern each, named after their tracks.
    assert_eq!(import.patterns.len(), 2);
    assert_eq!(import.patterns[0].name, "Lead");
    assert_eq!(import.patterns[1].name, "Bass");
    let names: Vec<&str> = import
        .channels
        .iter()
        .map(|c| c.channel.name.as_str())
        .collect();
    assert_eq!(names, ["Lead", "Bass"]);
    assert_eq!(import.channels[1].source.program, Some(33));
    assert_eq!(
        r.tracks[2].channels[0].programs[0].name,
        "Electric Bass (finger)"
    );

    // Triplets and quintuplets are exact rationals.
    let lead_notes = &import.patterns[0].notes;
    assert_eq!(lead_notes[0].start, Time::ZERO);
    assert_eq!(lead_notes[0].length, Time::new(1, 3));
    assert_eq!(lead_notes[1].start, Time::new(1, 3));
    assert_eq!(lead_notes[2].start, Time::new(2, 3));
    assert_eq!(lead_notes[3].start, Time::BEAT);
    assert_eq!(lead_notes[3].length, Time::new(1, 5));
    assert_eq!(lead_notes[4].start, Time::new(6, 5));
    assert_eq!(lead_notes[0].velocity, 100.0 / 127.0);
    assert_eq!(lead_notes[2].velocity, 1.0);
    assert_eq!(lead_notes[3].velocity, 1.0 / 127.0);

    // Pattern length: content (to track end 1000 ticks) rounded up to 3-beat bars.
    assert_eq!(import.patterns[0].length, Time::beats(3));
    assert_eq!(import.patterns[1].length, Time::beats(3));
    // The channel ids on notes are the imported channels.
    assert_eq!(lead_notes[0].channel, import.channels[0].channel.id);
    assert_eq!(import.channels[0].channel.track, Id::NIL);
}

#[test]
fn type0_splits_by_channel_and_names_from_programs() {
    let track = TrackBuilder::new()
        .program(0, 0, 0)
        .program(0, 2, 40)
        .note(0, 96, 0, 60, 100)
        .note(96, 192, 2, 70, 100)
        .note(192, 288, 0, 62, 100)
        .build(None);
    let bytes = smf(Format::SingleTrack, 96, vec![track]);

    // One pattern for the whole track, two channels.
    let one = import_smf(&bytes, &ImportOptions::default()).unwrap();
    assert_eq!(one.report.format, SmfFormat::Type0);
    assert_eq!(one.patterns.len(), 1);
    assert_eq!(one.channels.len(), 2);
    assert_eq!(one.channels[0].channel.name, "Acoustic Grand Piano (ch 1)");
    assert_eq!(one.channels[1].channel.name, "Violin (ch 3)");

    // Split into patterns per MIDI channel.
    let split = import_smf(
        &bytes,
        &ImportOptions {
            layout: PatternLayout::PerMidiChannel,
            ..ImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(split.patterns.len(), 2);
    assert_eq!(split.patterns[0].name, "Channel 1");
    assert_eq!(split.patterns[0].notes.len(), 2);
    assert_eq!(split.patterns[1].name, "Channel 3");

    // Everything in one pattern regardless of tracks.
    let combined = import_smf(
        &bytes,
        &ImportOptions {
            layout: PatternLayout::Combined,
            combined_pattern_name: "Whole song".into(),
            ..ImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(combined.patterns.len(), 1);
    assert_eq!(combined.patterns[0].name, "Whole song");
    assert_eq!(combined.patterns[0].notes.len(), 3);
}

#[test]
fn channel_ten_is_split_into_one_channel_per_drum_key() {
    let drums = TrackBuilder::new()
        .name(0, "Drums")
        .note(0, 24, 9, 36, 120)
        .note(96, 120, 9, 38, 100)
        .note(192, 216, 9, 36, 110)
        .note(0, 24, 9, 42, 70)
        .note(48, 72, 9, 42, 70)
        .build(None);
    let more_drums = TrackBuilder::new().note(0, 24, 9, 36, 90).build(None);
    let bytes = smf(Format::Parallel, 96, vec![drums, more_drums]);

    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();
    // One channel per key (36, 38, 42), shared by both tracks.
    assert_eq!(import.channels.len(), 3);
    let kick = &import.channels[0];
    assert_eq!(kick.source.drum_key, Some(36));
    assert_eq!(kick.source.track, None);
    assert_eq!(kick.channel.device.type_id, "peel.drum.kick");
    assert_eq!(kick.channel.name, "Bass Drum 1");
    assert_eq!(kick.channel.root_key, 36);
    assert_eq!(import.channels[1].channel.device.type_id, "peel.drum.snare");
    assert_eq!(import.channels[2].channel.device.type_id, "peel.drum.hat");
    assert_eq!(kick.source.notes, 3);
    // Two patterns (per track) both use the shared kick channel.
    assert_eq!(import.patterns.len(), 2);
    assert!(
        import.patterns[1]
            .notes
            .iter()
            .all(|n| n.channel == kick.channel.id)
    );
    // Keys are preserved on the notes.
    assert!(import.patterns[0].notes.iter().any(|n| n.key == 38));
    assert!(import.report.tracks[0].channels[0].is_drum);

    // Custom mapping and no splitting.
    fn custom(key: u8) -> (String, u32, String) {
        ("test.drum".into(), 7, format!("Key {key}"))
    }
    let custom_import = import_smf(
        &bytes,
        &ImportOptions {
            drum_device_for_key: custom,
            ..ImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        custom_import.channels[0].channel.device.type_id,
        "test.drum"
    );
    assert_eq!(custom_import.channels[0].channel.device.version, 7);
    assert_eq!(custom_import.channels[0].channel.name, "Key 36");

    let merged = import_smf(
        &bytes,
        &ImportOptions {
            split_drums: false,
            melodic_device: ("test.synth".into(), 3),
            ..ImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(merged.channels.len(), 2, "one per track");
    assert_eq!(merged.channels[0].channel.device.type_id, "test.synth");
    assert_eq!(merged.channels[0].channel.device.version, 3);
}

#[test]
fn overlapping_same_pitch_notes_pair_first_in_first_out() {
    let track = TrackBuilder::new()
        .on(0, 0, 60, 100) // A
        .on(100, 0, 60, 90) // B, while A still sounds
        .off(200, 0, 60) // ends A
        .off(300, 0, 60) // ends B
        .build(None);
    let bytes = smf(Format::SingleTrack, 100, vec![track]);
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();
    let notes = &import.patterns[0].notes;
    assert_eq!(notes.len(), 2);
    assert_eq!(
        (notes[0].start, notes[0].length),
        (Time::ZERO, Time::beats(2))
    );
    assert_eq!(
        (notes[1].start, notes[1].length),
        (Time::beats(1), Time::beats(2))
    );
    assert_eq!(import.report.repairs.overlapping_same_pitch, 1);
}

#[test]
fn missing_note_off_ends_at_track_end_and_zero_length_is_repaired() {
    let track = TrackBuilder::new()
        .on(0, 0, 60, 100) // never released
        .note(100, 100, 0, 62, 100) // zero length
        .off(150, 0, 70) // no matching note-on
        .on(300, 0, 64, 100) // never released, later
        .build(Some(1000));
    let bytes = smf(Format::SingleTrack, 100, vec![track]);
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();
    let notes = &import.patterns[0].notes;
    assert_eq!(notes.len(), 3);
    let unterminated: Vec<_> = notes
        .iter()
        .filter(|n| n.key == 60 || n.key == 64)
        .collect();
    assert_eq!(unterminated.len(), 2);
    assert!(
        unterminated
            .iter()
            .all(|n| n.start + n.length == Time::beats(10))
    );
    let zero = notes.iter().find(|n| n.key == 62).unwrap();
    assert_eq!(zero.length, Time::new(1, 100));
    let r = &import.report.repairs;
    assert_eq!(
        (
            r.unterminated_notes,
            r.zero_length_notes,
            r.orphan_note_offs
        ),
        (2, 1, 1)
    );
    assert!(
        peel_document::validate(&{
            let mut p = Project::empty("x");
            import
                .clone()
                .merge_with_new_mixer_tracks(&mut p, &MergeOptions::default())
                .unwrap();
            p
        })
        .iter()
        .all(|i| !i.is_error())
    );
}

#[test]
fn note_on_velocity_zero_is_a_note_off_and_release_velocity_is_kept() {
    let track = TrackBuilder::new()
        .on(0, 0, 60, 100)
        .on(100, 0, 60, 0) // note-off by velocity 0
        .on(100, 0, 62, 100)
        .off_vel(200, 0, 62, 32)
        .on(200, 0, 64, 100)
        .off_vel(300, 0, 64, 64)
        .build(None);
    let import = import_smf(
        &smf(Format::SingleTrack, 100, vec![track]),
        &ImportOptions::default(),
    )
    .unwrap();
    let notes = &import.patterns[0].notes;
    assert_eq!(notes.len(), 3);
    assert_eq!(notes[0].length, Time::BEAT);
    assert_eq!(notes[0].release, 0.5, "no release velocity -> default");
    assert_eq!(notes[1].release, 32.0 / 127.0);
    assert_eq!(notes[2].release, 0.5, "64 is the neutral release velocity");
    assert_eq!(import.report.repairs.orphan_note_offs, 0);
}

#[test]
fn running_status_in_raw_bytes() {
    // Hand-written type 0 file, 96 PPQ. The second and third events omit the
    // status byte (running status); a note-on with velocity 0 ends the note.
    #[rustfmt::skip]
    let bytes: Vec<u8> = vec![
        b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0, 96,
        b'M', b'T', b'r', b'k', 0, 0, 0, 17,
        0x00, 0x90, 60, 100,       // note on
        0x60, 60, 0,               // running status: note on vel 0 after 96 ticks
        0x00, 62, 90,              // running status: note on
        0x30, 62, 0,               // running status: off after 48 ticks
        0x00, 0xFF, 0x2F, 0x00,    // end of track
    ];
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();
    let notes = &import.patterns[0].notes;
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].length, Time::BEAT);
    assert_eq!(notes[1].start, Time::BEAT);
    assert_eq!(notes[1].length, Time::new(1, 2));
    assert!(
        import.report.warnings.is_empty(),
        "{:?}",
        import.report.warnings
    );
}

#[test]
fn every_ignored_event_is_counted() {
    let track = TrackBuilder::new()
        .name(0, "Everything")
        .note(0, 96, 0, 60, 100)
        .cc(0, 0, 7, 100)
        .cc(10, 0, 7, 90)
        .cc(10, 0, 10, 64)
        .cc(10, 1, 64, 127) // pedal, reported by default
        .cc(10, 0, 0, 1) // bank select
        .program(0, 0, 5)
        .bend(20, 0, 1000)
        .bend(30, 0, -1000)
        .aftertouch(40, 0)
        .sysex(50)
        .meta(0, MetaMessage::Text(b"hello"))
        .meta(0, MetaMessage::Copyright(b"(c) me"))
        .meta(0, MetaMessage::Lyric(b"la"))
        .meta(0, MetaMessage::CuePoint(b"cue"))
        .meta(0, MetaMessage::KeySignature(-3, true))
        .meta(0, MetaMessage::Unknown(0x10, &[1]))
        .meta(0, MetaMessage::SequencerSpecific(&[1, 2, 3]))
        .meta(0, MetaMessage::Marker(b"Verse"))
        .build(None);
    let bytes = smf(Format::SingleTrack, 96, vec![track]);
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();
    let r = &import.report;
    assert_eq!(count(r, "Control Change 7 (Channel Volume)"), 2);
    assert_eq!(count(r, "Control Change 10 (Pan)"), 1);
    assert_eq!(count(r, "Control Change 64 (Sustain Pedal)"), 1);
    assert_eq!(count(r, "Control Change 0 (Bank Select MSB)"), 1);
    assert_eq!(count(r, "Pitch Bend"), 2);
    assert_eq!(count(r, "Channel Aftertouch"), 1);
    assert_eq!(count(r, "System Exclusive"), 1);
    assert_eq!(
        count(r, "Program Change (reported per channel, not applied)"),
        1
    );
    assert_eq!(count(r, "Text (kept in report metadata)"), 1);
    assert_eq!(count(r, "Copyright (kept in report metadata)"), 1);
    assert_eq!(count(r, "Lyric (kept in report metadata)"), 1);
    assert_eq!(count(r, "Cue Point (kept in report metadata)"), 1);
    assert_eq!(
        count(r, "Key Signature (listed in report.key_signatures)"),
        1
    );
    assert_eq!(count(r, "Unknown Meta Event 0x10"), 1);
    assert_eq!(count(r, "Sequencer-Specific Meta Event"), 1);
    // Markers are imported, key signatures and text are retained.
    assert_eq!(import.markers, vec![(Time::ZERO, "Verse".to_string())]);
    assert_eq!(r.key_signatures.len(), 1);
    assert_eq!(r.key_signatures[0].accidentals, -3);
    assert!(r.key_signatures[0].minor);
    assert!(
        r.metadata
            .iter()
            .any(|m| m.kind == "Lyric" && m.text == "la")
    );
    // The report prints.
    let text = r.to_string();
    assert!(text.contains("Pitch Bend x2"), "{text}");
    assert!(
        text.contains("1 notes") || text.contains("1 notes in"),
        "{text}"
    );
}

#[test]
fn sustain_pedal_report_versus_extend() {
    let track = TrackBuilder::new()
        .cc(0, 0, 64, 127) // pedal down
        .note(0, 100, 0, 60, 100) // released at 100, pedal holds it
        .note(50, 150, 0, 64, 100)
        .on(200, 0, 60, 100) // striking again... but earlier copy already ended at release
        .off(220, 0, 60)
        .cc(400, 0, 64, 0) // pedal up
        .build(None);
    let bytes = smf(Format::SingleTrack, 100, vec![track]);

    let plain = import_smf(&bytes, &ImportOptions::default()).unwrap();
    let notes = &plain.patterns[0].notes;
    assert_eq!(notes[0].length, Time::BEAT);
    assert_eq!(count(&plain.report, "Control Change 64 (Sustain Pedal)"), 2);

    let extended = import_smf(
        &bytes,
        &ImportOptions {
            sustain: SustainPolicy::ExtendNotes,
            ..ImportOptions::default()
        },
    )
    .unwrap();
    let notes = &extended.patterns[0].notes;
    assert_eq!(notes.len(), 3);
    // First C: held until the same key is struck again at tick 200.
    assert_eq!(
        (notes[0].key, notes[0].start, notes[0].length),
        (60, Time::ZERO, Time::beats(2))
    );
    // E: held until the pedal comes up at 400.
    let e = notes.iter().find(|n| n.key == 64).unwrap();
    assert_eq!(e.start + e.length, Time::beats(4));
    // Second C: also held until the pedal comes up.
    let c2 = notes
        .iter()
        .find(|n| n.key == 60 && n.start == Time::beats(2))
        .unwrap();
    assert_eq!(c2.start + c2.length, Time::beats(4));
    assert_eq!(
        count(&extended.report, "Control Change 64 (Sustain Pedal)"),
        0
    );
}

#[test]
fn quantize_is_opt_in_and_reported() {
    let track = TrackBuilder::new()
        .note(110, 200, 0, 60, 100) // 110/96 beats
        .build(None);
    let bytes = smf(Format::SingleTrack, 96, vec![track]);
    let exact = import_smf(&bytes, &ImportOptions::default()).unwrap();
    assert_eq!(exact.patterns[0].notes[0].start, Time::new(110, 96));
    assert_eq!(exact.report.repairs.notes_quantized, 0);

    let q = import_smf(
        &bytes,
        &ImportOptions {
            quantize: Quantize::Starts(Time::new(1, 4)),
            ..ImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(q.patterns[0].notes[0].start, Time::new(5, 4));
    assert_eq!(
        q.patterns[0].notes[0].length,
        Time::new(90, 96),
        "length untouched"
    );
    assert_eq!(q.report.repairs.notes_quantized, 1);

    let ql = import_smf(
        &bytes,
        &ImportOptions {
            quantize: Quantize::StartsAndLengths(Time::new(1, 4)),
            ..ImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(ql.patterns[0].notes[0].length, Time::BEAT);
}

#[test]
fn type2_sequences_become_separate_sequential_patterns() {
    let a = TrackBuilder::new()
        .name(0, "Seq A")
        .tempo(0, 600_000)
        .note(0, 96, 0, 60, 100)
        .build(Some(384)); // 4 beats
    let b = TrackBuilder::new()
        .name(0, "Seq B")
        .tempo(0, 300_000) // belongs to its own sequence
        .note(0, 48, 0, 62, 100)
        .build(Some(192)); // 2 beats
    let bytes = smf(Format::Sequential, 96, vec![a, b]);

    let import = import_smf(
        &bytes,
        &ImportOptions {
            layout: PatternLayout::Combined, // ignored for type 2
            ..ImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(import.report.format, SmfFormat::Type2);
    assert!(import.sequential);
    assert_eq!(import.patterns.len(), 2);
    assert_eq!(import.patterns[0].name, "Seq A");
    assert_eq!(import.tempo.len(), 1);
    assert_eq!(import.tempo[0].bpm, 100.0, "first sequence's tempo");
    assert_eq!(
        count(&import.report, "Tempo (in a later independent sequence)"),
        1
    );
    assert!(
        import
            .report
            .warnings
            .iter()
            .any(|w| w.contains("independent"))
    );

    let mut p = Project::empty("t2");
    let report = import
        .merge_with_new_mixer_tracks(&mut p, &MergeOptions::new_project())
        .unwrap();
    let arrangement = p.arrangement();
    let clips: Vec<_> = arrangement.clips.iter().collect();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].start, Time::ZERO);
    assert_eq!(
        clips[1].start,
        Time::beats(4),
        "placed after the first sequence"
    );
    assert_eq!(
        clips[0].track, clips[1].track,
        "same lane, one after another"
    );
    assert_eq!(report.patterns.len(), 2);
    assert!(validate(&p).iter().all(|i| !i.is_error()));
}

#[test]
fn smpte_timing_is_converted_at_120_bpm() {
    // 30 fps, 40 ticks per frame: 1200 ticks per second, 600 per beat at 120.
    let track = TrackBuilder::new()
        .tempo(0, 250_000) // meaningless with SMPTE; must be ignored
        .note(0, 600, 0, 60, 100)
        .note(1200, 1500, 0, 62, 100)
        .build(None);
    let bytes = smf_smpte(Fps::Fps30, 40, vec![track]);
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();
    assert_eq!(import.report.ticks_per_quarter, None);
    assert_eq!(import.tempo.len(), 1);
    assert_eq!(import.tempo[0].bpm, 120.0);
    let notes = &import.patterns[0].notes;
    assert_eq!(notes[0].length, Time::BEAT);
    assert_eq!(notes[1].start, Time::beats(2));
    assert_eq!(notes[1].length, Time::new(1, 2));
    assert_eq!(
        count(&import.report, "Tempo (ignored with SMPTE timing)"),
        1
    );
    assert!(import.report.warnings.iter().any(|w| w.contains("SMPTE")));

    // 29.97 fps stays an exact rational.
    let track = TrackBuilder::new().note(0, 3000, 0, 60, 100).build(None);
    let import = import_smf(
        &smf_smpte(Fps::Fps29, 100, vec![track]),
        &ImportOptions::default(),
    )
    .unwrap();
    // 3000 ticks at 100 ticks/frame = 30 frames = 1.001 s = 2.002 beats.
    assert_eq!(import.patterns[0].notes[0].length, Time::new(1001, 500));
}

#[test]
fn malformed_data_is_reported_with_an_offset_and_partially_imported() {
    let track = TrackBuilder::new()
        .note(0, 96, 0, 60, 100)
        .note(96, 192, 0, 62, 100)
        .build(None);
    let mut bytes = smf(Format::SingleTrack, 96, vec![track]);
    // Cut the file mid-event: drop the end-of-track and part of the last
    // note-off, leaving a dangling status byte.
    let cut = bytes.len() - 5;
    bytes.truncate(cut);
    // A truncated track chunk must still be tolerated by the length field
    // being wrong, so fix the declared chunk length to match.
    let track_len = (bytes.len() - 22) as u32;
    bytes[18..22].copy_from_slice(&track_len.to_be_bytes());

    let strict = import_smf(
        &bytes,
        &ImportOptions {
            allow_partial: false,
            ..ImportOptions::default()
        },
    );
    assert!(
        matches!(strict, Err(MidiError::Malformed { track: 0, .. })),
        "{strict:?}"
    );

    let lenient = import_smf(&bytes, &ImportOptions::default()).unwrap();
    assert_eq!(lenient.report.repairs.malformed_tracks.len(), 1);
    let (track, offset, message) = &lenient.report.repairs.malformed_tracks[0];
    assert_eq!(*track, 0);
    assert!(*offset >= 22 && *offset < bytes.len(), "offset {offset}");
    assert!(!message.is_empty());
    // The first note and the one whose off was lost still import.
    assert_eq!(lenient.patterns[0].notes.len(), 2);
    assert_eq!(lenient.report.repairs.unterminated_notes, 1);
}

#[test]
fn garbage_and_limits_are_errors_not_panics() {
    assert!(matches!(
        import_smf(b"not midi at all", &ImportOptions::default()),
        Err(MidiError::Parse(_))
    ));
    assert!(matches!(
        import_smf(b"", &ImportOptions::default()),
        Err(MidiError::Parse(_))
    ));
    // Truncated header.
    assert!(import_smf(b"MThd\0\0\0\x06\0\0", &ImportOptions::default()).is_err());

    let track = TrackBuilder::new()
        .note(0, 10, 0, 60, 100)
        .note(10, 20, 0, 61, 100)
        .note(20, 30, 0, 62, 100)
        .build(None);
    let bytes = smf(Format::SingleTrack, 96, vec![track]);
    assert!(matches!(
        import_smf(
            &bytes,
            &ImportOptions {
                max_notes: 2,
                ..ImportOptions::default()
            }
        ),
        Err(MidiError::TooLarge(_))
    ));

    // A file with a header and no tracks is an empty import.
    let empty = smf(Format::Parallel, 96, vec![]);
    let import = import_smf(&empty, &ImportOptions::default()).unwrap();
    assert!(import.patterns.is_empty() && import.channels.is_empty());
}

#[test]
fn header_track_count_mismatch_is_a_warning() {
    let t = TrackBuilder::new().note(0, 96, 0, 60, 100).build(None);
    let mut bytes = smf(Format::Parallel, 96, vec![t]);
    bytes[10..12].copy_from_slice(&3u16.to_be_bytes()); // claims 3 tracks
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();
    assert!(
        import
            .report
            .warnings
            .iter()
            .any(|w| w.contains("declares 3 tracks"))
    );
}

#[test]
fn merge_into_new_project_adopts_and_validates() {
    let conductor = TrackBuilder::new()
        .tempo(0, 461_538) // ~130 BPM
        .signature(0, 3, 2)
        .meta(960, MetaMessage::Marker(b"Chorus"))
        .build(Some(1920));
    let t1 = TrackBuilder::new()
        .name(0, "Keys")
        .note(0, 480, 0, 60, 100)
        .build(None);
    let t2 = TrackBuilder::new()
        .name(0, "Pad")
        .note(0, 480, 1, 48, 100)
        .build(None);
    let bytes = smf(Format::Parallel, 480, vec![conductor, t1, t2]);
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();

    let mut project = Project::empty("Fresh");
    let before_lanes = project.arrangement().tracks.len();
    let report = import
        .merge_with_new_mixer_tracks(&mut project, &MergeOptions::new_project())
        .unwrap();

    assert!((project.bpm() - 60_000_000.0 / 461_538.0).abs() < 1e-9);
    assert_eq!(project.time_signature.numerator, 3);
    assert_eq!(project.channels.len(), 2);
    assert_eq!(report.mixer_tracks_created, 2);
    assert_eq!(project.mixer.tracks.len(), 3, "master plus one per channel");
    for ch in &project.channels {
        assert!(project.mixer.track(ch.track).is_some());
        assert_eq!(project.mixer.track(ch.track).unwrap().name, ch.name);
    }
    // Patterns sit side by side on separate lanes (the empty project already
    // has free ones).
    let a = project.arrangement();
    assert_eq!(a.clips.len(), 2);
    assert_ne!(a.clips[0].track, a.clips[1].track);
    assert_eq!(a.tracks.len(), before_lanes);
    assert_eq!(a.markers.len(), 1);
    assert_eq!(a.markers[0].position, Time::beats(2));
    assert!(validate(&project).iter().all(|i| !i.is_error()));
}

#[test]
fn merge_into_existing_song_keeps_tempo_and_meter_by_default() {
    let conductor = TrackBuilder::new()
        .tempo(0, 300_000)
        .signature(0, 7, 3)
        .build(None);
    let t = TrackBuilder::new()
        .name(0, "Added")
        .note(0, 96, 0, 60, 100)
        .build(None);
    let bytes = smf(Format::Parallel, 96, vec![conductor, t]);
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();

    let mut project = Project::empty("Existing");
    project.set_bpm(90.0);
    let existing_track = project.mixer.tracks[0].id;
    let existing_clip_lane = project.arrangement().tracks[0].id;
    let before_patterns = project.patterns.len();

    // Route into an existing track; occupy the first lane so the clip moves on.
    {
        let pattern = project.patterns[0].id;
        project.arrangement_mut().clips.push(peel_document::Clip {
            id: Id::new(),
            track: existing_clip_lane,
            start: Time::ZERO,
            length: Time::beats(4),
            offset: Time::ZERO,
            muted: false,
            content: peel_document::ClipContent::Pattern { pattern },
        });
    }
    let mut doc = Document::new(project);
    let report = doc.edit("Import MIDI", |p| {
        import.merge_into(p, &MergeOptions::default(), |_, _| existing_track)
    });
    let report = report.unwrap();

    let p = doc.project();
    assert_eq!(p.bpm(), 90.0, "tempo untouched");
    assert_eq!(p.time_signature, TimeSignature::default());
    assert_eq!(p.patterns.len(), before_patterns + 1);
    assert_eq!(p.channels[0].track, existing_track);
    assert_eq!(p.mixer.tracks.len(), 1);
    let new_clip = p
        .arrangement()
        .clips
        .iter()
        .find(|c| c.id == report.clips[0])
        .unwrap();
    assert_ne!(new_clip.track, existing_clip_lane, "first lane was busy");
    assert!(doc.validate().iter().all(|i| !i.is_error()));

    // Undo removes the whole import in one step.
    doc.undo();
    assert_eq!(doc.project().patterns.len(), before_patterns);
    assert!(doc.project().channels.is_empty());
}

#[test]
fn merge_rejects_unknown_mixer_tracks_and_leaves_the_project_alone() {
    let t = TrackBuilder::new().note(0, 96, 0, 60, 100).build(None);
    let bytes = smf(Format::SingleTrack, 96, vec![t]);
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();

    let mut project = Project::empty("Safe");
    let snapshot = project.clone();
    let bogus = Id::new();
    let err = import
        .merge_into(&mut project, &MergeOptions::default(), |_, _| bogus)
        .unwrap_err();
    assert!(matches!(err, MidiError::UnknownMixerTrack(id) if id == bogus));
    assert_eq!(project, snapshot);
}

#[test]
fn imported_pattern_notes_are_valid_for_the_model() {
    // A dense file: validate catches any non-positive length or bad key.
    let mut t = TrackBuilder::new().name(0, "Dense");
    for i in 0..200u64 {
        t = t.note(
            i * 7,
            i * 7 + (i % 5),
            0,
            (i % 128) as u8,
            ((i % 127) + 1) as u8,
        );
    }
    let bytes = smf(Format::SingleTrack, 120, vec![t.build(None)]);
    let import = import_smf(&bytes, &ImportOptions::default()).unwrap();
    let mut project = Project::empty("Dense");
    import
        .merge_with_new_mixer_tracks(&mut project, &MergeOptions::default())
        .unwrap();
    assert!(validate(&project).iter().all(|i| !i.is_error()));
    assert_eq!(project.patterns.last().unwrap().notes.len(), 200);
}

#[test]
fn riff_wrapped_midi_imports() {
    let t = TrackBuilder::new()
        .name(0, "RMID")
        .note(0, 96, 0, 60, 100)
        .build(None);
    let inner = smf(Format::SingleTrack, 96, vec![t]);
    let pad = inner.len() & 1;
    let mut rmi = Vec::new();
    rmi.extend_from_slice(b"RIFF");
    rmi.extend_from_slice(&((4 + 8 + inner.len() + pad) as u32).to_le_bytes());
    rmi.extend_from_slice(b"RMID");
    rmi.extend_from_slice(b"data");
    rmi.extend_from_slice(&(inner.len() as u32).to_le_bytes());
    rmi.extend_from_slice(&inner);
    rmi.extend(std::iter::repeat_n(0u8, pad));
    let import = import_smf(&rmi, &ImportOptions::default()).unwrap();
    assert_eq!(import.patterns[0].name, "RMID");
    assert_eq!(import.patterns[0].notes.len(), 1);
}

#[test]
fn hostile_smpte_header_is_an_error_not_a_panic() {
    // A division of 0x8000 makes midly negate i8::MIN.
    let t = TrackBuilder::new().note(0, 96, 0, 60, 100).build(None);
    let mut bytes = smf(Format::SingleTrack, 96, vec![t]);
    bytes[12] = 0x80;
    bytes[13] = 0x00;
    assert!(matches!(
        import_smf(&bytes, &ImportOptions::default()),
        Err(MidiError::UnsupportedTiming(_))
    ));
}
