mod common;

use std::sync::Arc;

use midly::{Format, MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use peel_document::{
    Channel, Clip, ClipContent, Color, DeviceInstance, Id, MixerTrack, Note, PALETTE, Pattern,
    Project, TempoSegment, Time, TimeSignature, Track,
};
use peel_midi::{
    ExportFormat, ExportOptions, ImportOptions, MergeOptions, MidiError, ProbabilityMode,
    SustainPolicy, export_arrangement, export_pattern, import_smf,
};

use common::{TrackBuilder, smf};

fn channel(project: &mut Project, name: &str, device: &str) -> Id {
    let track = MixerTrack {
        id: Id::new(),
        name: name.into(),
        color: PALETTE[0],
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        solo: false,
        inserts: vec![],
        output: None,
        sends: Vec::new(),
    };
    let ch = Channel {
        id: Id::new(),
        name: name.into(),
        color: Color::hex(0x336699),
        device: DeviceInstance::new(device, 1),
        track: track.id,
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        root_key: 60,
    };
    let id = ch.id;
    project.mixer.tracks.push(track);
    project.channels.push(ch);
    id
}

fn add_pattern(project: &mut Project, name: &str, length: Time, notes: Vec<Note>) -> Id {
    let mut p = Pattern {
        id: Id::new(),
        name: name.into(),
        color: PALETTE[1],
        length,
        notes,
        expression: Vec::new(),
    };
    p.sort_notes();
    let id = p.id;
    project.patterns.push(Arc::new(p));
    id
}

/// (tick, message) pairs of a parsed file, for each track.
type Parsed<'a> = Vec<Vec<(u64, TrackEventKind<'a>)>>;

fn parse(bytes: &[u8]) -> (Smf<'_>, Parsed<'_>) {
    let smf = Smf::parse(bytes).unwrap();
    let tracks = smf
        .tracks
        .iter()
        .map(|t| {
            let mut tick = 0u64;
            t.iter()
                .map(|e| {
                    tick += u64::from(e.delta.as_int());
                    (tick, e.kind)
                })
                .collect()
        })
        .collect();
    (smf, tracks)
}

fn note_ons(track: &[(u64, TrackEventKind<'_>)]) -> Vec<(u64, u8, u8, u8)> {
    track
        .iter()
        .filter_map(|(tick, kind)| match kind {
            TrackEventKind::Midi {
                channel,
                message: MidiMessage::NoteOn { key, vel },
            } => Some((*tick, channel.as_int(), key.as_int(), vel.as_int())),
            _ => None,
        })
        .collect()
}

fn note_offs(track: &[(u64, TrackEventKind<'_>)]) -> Vec<(u64, u8)> {
    track
        .iter()
        .filter_map(|(tick, kind)| match kind {
            TrackEventKind::Midi {
                message: MidiMessage::NoteOff { key, .. },
                ..
            } => Some((*tick, key.as_int())),
            _ => None,
        })
        .collect()
}

#[test]
fn pattern_export_is_type1_with_tempo_meter_names_and_percussion() {
    let mut p = Project::empty("Export Song");
    p.tempo = vec![TempoSegment {
        start: Time::ZERO,
        bpm: 100.0,
        ramp_to: None,
    }];
    p.time_signature = TimeSignature {
        numerator: 7,
        denominator: 8,
    };
    let lead = channel(&mut p, "Lead", "peel.synth.poly");
    let kick = channel(&mut p, "Kick", "peel.drum.kick");
    let pat = add_pattern(
        &mut p,
        "Groove",
        Time::beats(4),
        vec![
            Note::new(lead, Time::ZERO, Time::new(1, 3), 60, 1.0),
            Note::new(lead, Time::new(1, 3), Time::new(1, 3), 62, 0.5),
            Note::new(lead, Time::new(2, 3), Time::new(1, 3), 64, 0.25),
            Note::new(kick, Time::beats(1), Time::new(1, 4), 36, 0.8),
        ],
    );

    let out = export_pattern(&p, pat, &ExportOptions::default()).unwrap();
    assert_eq!(out.report.notes, 4);
    assert_eq!(out.report.rounded_values, 0, "triplets fit 960 PPQ exactly");
    assert_eq!(out.report.tracks, 3);

    let (smf, tracks) = parse(&out.bytes);
    assert_eq!(smf.header.format, Format::Parallel);
    assert_eq!(smf.header.timing, Timing::Metrical(960.into()));
    assert_eq!(tracks.len(), 3, "conductor + one per channel");

    // Conductor: name, 7/8, 100 BPM, end of track at the pattern end.
    let conductor = &tracks[0];
    assert!(conductor.iter().any(|(_, k)| matches!(k, TrackEventKind::Meta(MetaMessage::TrackName(n)) if *n == b"Export Song")));
    assert!(conductor.iter().any(|(t, k)| *t == 0
        && matches!(
            k,
            TrackEventKind::Meta(MetaMessage::TimeSignature(7, 3, _, _))
        )));
    assert!(conductor.iter().any(|(t, k)| *t == 0
        && matches!(k, TrackEventKind::Meta(MetaMessage::Tempo(us)) if us.as_int() == 600_000)));
    assert_eq!(conductor.last().unwrap().0, 4 * 960);

    // Lead on the first melodic channel, triplets at 320-tick spacing.
    let lead_on = note_ons(&tracks[1]);
    assert_eq!(
        lead_on,
        vec![(0, 0, 60, 127), (320, 0, 62, 64), (640, 0, 64, 32)]
    );
    assert_eq!(
        note_offs(&tracks[1])
            .iter()
            .map(|o| o.0)
            .collect::<Vec<_>>(),
        vec![320, 640, 960]
    );
    // Kick is percussion: MIDI channel 10 (index 9).
    assert_eq!(note_ons(&tracks[2]), vec![(960, 9, 36, 102)]);
    assert!(tracks[1].iter().any(
        |(_, k)| matches!(k, TrackEventKind::Meta(MetaMessage::TrackName(n)) if *n == b"Lead")
    ));
}

#[test]
fn type0_export_is_one_track() {
    let mut p = Project::empty("T0");
    let a = channel(&mut p, "A", "peel.synth.poly");
    let b = channel(&mut p, "B", "peel.synth.poly");
    let pat = add_pattern(
        &mut p,
        "P",
        Time::beats(2),
        vec![
            Note::new(a, Time::ZERO, Time::BEAT, 60, 0.5),
            Note::new(b, Time::BEAT, Time::BEAT, 62, 0.5),
        ],
    );
    let opts = ExportOptions {
        format: ExportFormat::Type0,
        ppq: 480,
        ..ExportOptions::default()
    };
    let out = export_pattern(&p, pat, &opts).unwrap();
    let (smf, tracks) = parse(&out.bytes);
    assert_eq!(smf.header.format, Format::SingleTrack);
    assert_eq!(tracks.len(), 1);
    assert_eq!(note_ons(&tracks[0]), vec![(0, 0, 60, 64), (480, 1, 62, 64)]);

    // And it reads back, split by channel.
    let back = import_smf(&out.bytes, &ImportOptions::default()).unwrap();
    assert_eq!(back.channels.len(), 2);
    assert_eq!(back.patterns[0].notes.len(), 2);
}

#[test]
fn mute_and_probability_are_options() {
    let mut p = Project::empty("Opts");
    p.seed = 1234;
    let ch = channel(&mut p, "Lead", "peel.synth.poly");
    let silent = channel(&mut p, "Silent", "peel.synth.poly");
    p.channel_mut(silent).unwrap().muted = true;

    let mut notes = vec![
        Note::new(ch, Time::ZERO, Time::BEAT, 60, 0.5),
        Note::new(silent, Time::ZERO, Time::BEAT, 61, 0.5),
    ];
    let mut muted = Note::new(ch, Time::BEAT, Time::BEAT, 62, 0.5);
    muted.muted = true;
    notes.push(muted);
    let mut never = Note::new(ch, Time::beats(2), Time::BEAT, 63, 0.5);
    never.probability = 0.0;
    notes.push(never);
    let mut half = Note::new(ch, Time::beats(3), Time::BEAT, 64, 0.5);
    half.probability = 0.5;
    notes.push(half);
    let pat = add_pattern(&mut p, "P", Time::beats(4), notes);

    let default = export_pattern(&p, pat, &ExportOptions::default()).unwrap();
    let r = &default.report;
    assert_eq!(r.skipped_muted, 2, "muted note and muted channel");
    assert!(r.skipped_probability >= 1, "probability 0 never plays");
    // Same seed, same file.
    let again = export_pattern(&p, pat, &ExportOptions::default()).unwrap();
    assert_eq!(default.bytes, again.bytes);
    let seeded = |seed| ExportOptions {
        probability: ProbabilityMode::Seeded(seed),
        ..ExportOptions::default()
    };
    assert_eq!(
        export_pattern(&p, pat, &seeded(1234)).unwrap().bytes,
        default.bytes,
        "ProjectSeed is the project's seed"
    );

    let everything = ExportOptions {
        respect_mute: false,
        probability: ProbabilityMode::Ignore,
        ..ExportOptions::default()
    };
    let all = export_pattern(&p, pat, &everything).unwrap();
    assert_eq!(all.report.notes, 5);
    assert_eq!(all.report.skipped_muted, 0);
    assert_eq!(all.report.probability_ignored, 2);
    let (_, tracks) = parse(&all.bytes);
    // Conductor + both channels (the muted channel is written when muting is off).
    assert_eq!(tracks.len(), 3);
}

#[test]
fn fractional_ticks_are_rounded_and_counted() {
    let mut p = Project::empty("Round");
    let ch = channel(&mut p, "Lead", "peel.synth.poly");
    let pat = add_pattern(
        &mut p,
        "P",
        Time::beats(2),
        vec![
            // 1/7 beat at 960 PPQ is 137.142857 ticks.
            Note::new(ch, Time::new(1, 7), Time::new(1, 7), 60, 0.5),
            // Far shorter than a tick: lengthened to one tick.
            Note::new(ch, Time::BEAT, Time::new(1, 10_000), 62, 0.5),
        ],
    );
    let out = export_pattern(&p, pat, &ExportOptions::default()).unwrap();
    let r = &out.report;
    // Start and end of the first note, and the end of the second.
    assert_eq!(r.rounded_values, 3);
    assert_eq!(r.lengthened_notes, 1);
    assert!(r.max_rounding_error_ticks > 0.28 && r.max_rounding_error_ticks < 0.29);
    assert!(r.warnings.iter().any(|w| w.contains("rounded")));
    let (_, tracks) = parse(&out.bytes);
    assert_eq!(note_ons(&tracks[1])[0].0, 137);
    assert_eq!(note_offs(&tracks[1])[0].0, 274);
    let second = note_ons(&tracks[1])[1].0;
    assert_eq!(second, 960);
    assert!(note_offs(&tracks[1]).iter().any(|o| o.0 == 961));
}

#[test]
fn per_note_expression_and_audio_are_reported_as_lost() {
    let mut p = Project::empty("Lossy");
    let ch = channel(&mut p, "Lead", "peel.synth.poly");
    let mut n = Note::new(ch, Time::ZERO, Time::BEAT, 60, 0.5);
    n.pan = 0.5;
    n.fine = 25.0;
    let pat = add_pattern(&mut p, "P", Time::beats(4), vec![n]);
    p.pattern_mut(pat)
        .unwrap()
        .expression
        .push(peel_document::NoteExpression {
            note: n.id,
            pitch: vec![peel_document::ExpressionPoint {
                offset: Time::ZERO,
                value: 3.0,
            }],
            ..Default::default()
        });
    let pattern_export = export_pattern(&p, pat, &ExportOptions::default()).unwrap();
    assert!(
        pattern_export
            .report
            .warnings
            .iter()
            .any(|w| w.contains("Note expression"))
    );
    let lane = p.arrangement().tracks[0].id;
    let master = p.mixer.tracks[0].id;
    let asset = Id::new();
    p.assets.push(peel_document::AudioAsset {
        id: asset,
        hash: "0".repeat(64),
        name: "loop.wav".into(),
        path: "/x/loop.wav".into(),
        managed: false,
        sample_rate: 44_100,
        channels: 2,
        frames: 100,
    });
    let arr = p.arrangement_mut();
    arr.clips.push(Clip {
        id: Id::new(),
        track: lane,
        start: Time::ZERO,
        length: Time::beats(4),
        offset: Time::ZERO,
        muted: false,
        content: ClipContent::Pattern { pattern: pat },
    });
    arr.clips.push(Clip {
        id: Id::new(),
        track: lane,
        start: Time::beats(4),
        length: Time::beats(4),
        offset: Time::ZERO,
        muted: false,
        content: ClipContent::audio(asset, master),
    });
    let out = export_arrangement(&p, &ExportOptions::default()).unwrap();
    assert_eq!(out.report.notes_with_pan, 1);
    assert_eq!(out.report.notes_with_fine_pitch, 1);
    assert_eq!(out.report.audio_clips_omitted, 1);
    assert!(out.report.warnings.iter().any(|w| w.contains("pan")));
    assert!(out.report.warnings.iter().any(|w| w.contains("audio")));
    assert!(
        out.report
            .warnings
            .iter()
            .any(|w| w.contains("Note expression"))
    );
    assert_eq!(out.report.notes, 1);
    // The song still ends where the arrangement does (8 beats).
    let (_, tracks) = parse(&out.bytes);
    assert_eq!(tracks[0].last().unwrap().0, 8 * 960);
}

#[test]
fn arrangement_export_flattens_loops_offsets_and_clip_ends() {
    let mut p = Project::empty("Flatten");
    let ch = channel(&mut p, "Lead", "peel.synth.poly");
    // Two-beat pattern; note A at 0, note B at 1 lasting 2 beats (crosses the loop).
    let pat = add_pattern(
        &mut p,
        "Loop",
        Time::beats(2),
        vec![
            Note::new(ch, Time::ZERO, Time::new(1, 2), 60, 0.5),
            Note::new(ch, Time::BEAT, Time::beats(2), 64, 0.5),
        ],
    );
    let lane = p.arrangement().tracks[0].id;
    let muted_lane = Track {
        id: Id::new(),
        name: "Muted".into(),
        color: None,
        muted: true,
    };
    let muted_lane_id = muted_lane.id;
    let arr = p.arrangement_mut();
    arr.tracks.push(muted_lane);
    let clip = |track, start: Time, length: Time, offset: Time, muted: bool| Clip {
        id: Id::new(),
        track,
        start,
        length,
        offset,
        muted,
        content: ClipContent::Pattern { pattern: pat },
    };
    // Plays at beat 4 for 5 beats: iterations at 4, 6 and a trimmed third at 8.
    arr.clips.push(clip(
        lane,
        Time::beats(4),
        Time::beats(5),
        Time::ZERO,
        false,
    ));
    // Starts half a beat into the pattern.
    arr.clips.push(clip(
        lane,
        Time::beats(20),
        Time::beats(2),
        Time::new(1, 2),
        false,
    ));
    // Never exported.
    arr.clips.push(clip(
        lane,
        Time::beats(30),
        Time::beats(2),
        Time::ZERO,
        true,
    ));
    arr.clips.push(clip(
        muted_lane_id,
        Time::beats(40),
        Time::beats(2),
        Time::ZERO,
        false,
    ));

    let out = export_arrangement(&p, &ExportOptions::default()).unwrap();
    assert_eq!(out.report.muted_clips_skipped, 2);
    let (_, tracks) = parse(&out.bytes);
    let at = |t: u64| t * 960;
    let ons: Vec<(u64, u8)> = note_ons(&tracks[1]).iter().map(|n| (n.0, n.2)).collect();
    assert_eq!(
        ons,
        vec![
            // Clip at beat 4, five beats long: A B A B A, the last B cut off.
            (at(4), 60),
            (at(5), 64),
            (at(6), 60),
            (at(7), 64),
            (at(8), 60),
            // Clip at beat 20 starting half a beat into the loop: A is
            // skipped, B plays from beat 20.5, A returns at 21.5.
            (at(20) + 480, 64),
            (at(21) + 480, 60),
        ]
    );
    let offs = note_offs(&tracks[1]);
    // B at beat 7 lasts two beats and ends exactly at the clip end (9).
    assert!(offs.contains(&(at(9), 64)));
    // The second clip ends at beat 22 and trims B (which would run to 22.5).
    assert!(offs.contains(&(at(22), 64)));
    assert!(offs.contains(&(at(22), 60)));
}

#[test]
fn tempo_map_and_ramps() {
    let mut p = Project::empty("Tempo");
    let ch = channel(&mut p, "Lead", "peel.synth.poly");
    p.tempo = vec![
        TempoSegment {
            start: Time::ZERO,
            bpm: 100.0,
            ramp_to: Some(140.0),
        },
        TempoSegment {
            start: Time::beats(8),
            bpm: 140.0,
            ramp_to: None,
        },
        TempoSegment {
            start: Time::beats(12),
            bpm: 90.0,
            ramp_to: None,
        },
    ];
    let pat = add_pattern(
        &mut p,
        "P",
        Time::beats(16),
        vec![Note::new(ch, Time::ZERO, Time::BEAT, 60, 0.5)],
    );
    let out = export_pattern(&p, pat, &ExportOptions::default()).unwrap();
    assert_eq!(out.report.tempo_ramps_approximated, 1);
    let (_, tracks) = parse(&out.bytes);
    let tempos: Vec<(u64, u32)> = tracks[0]
        .iter()
        .filter_map(|(t, k)| match k {
            TrackEventKind::Meta(MetaMessage::Tempo(us)) => Some((*t, us.as_int())),
            _ => None,
        })
        .collect();
    // 8 ramp steps, then 140 at beat 8, then 90 at beat 12.
    assert_eq!(tempos.len(), 10);
    assert_eq!(tempos[0].0, 0);
    assert!(tempos[0].1 > tempos[7].1, "tempo rises, so micros fall");
    assert_eq!(tempos[8], (8 * 960, 428_571));
    assert_eq!(tempos[9], (12 * 960, 666_667));

    // Without the tempo map only the first tempo is written.
    let flat = export_pattern(
        &p,
        pat,
        &ExportOptions {
            include_tempo_map: false,
            ..ExportOptions::default()
        },
    )
    .unwrap();
    let (_, tracks) = parse(&flat.bytes);
    let count = tracks[0]
        .iter()
        .filter(|(_, k)| matches!(k, TrackEventKind::Meta(MetaMessage::Tempo(_))))
        .count();
    assert_eq!(count, 1);

    // Re-import sees the changes as segments.
    let back = import_smf(&out.bytes, &ImportOptions::default()).unwrap();
    assert_eq!(back.tempo.last().unwrap().bpm.round(), 90.0);
    assert!(back.tempo.len() >= 9);
}

#[test]
fn export_errors() {
    let p = Project::empty("Err");
    assert!(matches!(
        export_pattern(&p, Id::new(), &ExportOptions::default()),
        Err(MidiError::UnknownPattern(_))
    ));
    let bad_ppq = ExportOptions {
        ppq: 0,
        ..ExportOptions::default()
    };
    assert!(matches!(
        export_pattern(&p, p.patterns[0].id, &bad_ppq),
        Err(MidiError::Export(_))
    ));
    let too_big = ExportOptions {
        ppq: 40_000,
        ..ExportOptions::default()
    };
    assert!(export_pattern(&p, p.patterns[0].id, &too_big).is_err());

    // An empty song is still a valid file.
    let out = export_arrangement(&p, &ExportOptions::default()).unwrap();
    let (_, tracks) = parse(&out.bytes);
    assert_eq!(tracks.len(), 1);
}

#[test]
fn more_than_fifteen_melodic_channels_share_midi_channels_with_a_warning() {
    let mut p = Project::empty("Many");
    let mut notes = Vec::new();
    for i in 0..17 {
        let ch = channel(&mut p, &format!("Ch {i}"), "peel.synth.poly");
        notes.push(Note::new(ch, Time::ZERO, Time::BEAT, 60, 0.5));
    }
    let pat = add_pattern(&mut p, "P", Time::beats(1), notes);
    let out = export_pattern(&p, pat, &ExportOptions::default()).unwrap();
    assert!(out.report.warnings.iter().any(|w| w.contains("share")));
    let (_, tracks) = parse(&out.bytes);
    assert_eq!(tracks.len(), 18);
    // Percussion channel 9 is never used by a melodic channel.
    for t in &tracks[1..] {
        assert!(note_ons(t).iter().all(|n| n.1 != 9));
    }
}

// ------------------------------------------------------------ round trip

type NoteKey = (Time, Time, u8, u32, u32);

fn notes_of(project: &Project) -> Vec<NoteKey> {
    let mut all: Vec<NoteKey> = project
        .patterns
        .iter()
        .flat_map(|p| p.notes.iter())
        .map(|n| {
            (
                n.start,
                n.length,
                n.key,
                n.velocity.to_bits(),
                n.release.to_bits(),
            )
        })
        .collect();
    all.sort();
    all
}

fn import_into_project(bytes: &[u8], opts: &ImportOptions) -> Project {
    let import = import_smf(bytes, opts).unwrap();
    let mut project = Project::empty("Round trip");
    import
        .merge_with_new_mixer_tracks(&mut project, &MergeOptions::new_project())
        .unwrap();
    project
}

fn fixture() -> Vec<u8> {
    // 480 PPQ with triplets, quintuplets, drums, a tempo change, 5/4.
    let conductor = TrackBuilder::new()
        .name(0, "Round trip")
        .tempo(0, 500_000)
        .signature(0, 5, 2)
        .tempo(4800, 400_000)
        .build(Some(9600));
    let lead = TrackBuilder::new()
        .name(0, "Lead")
        .note(0, 160, 0, 60, 100)
        .note(160, 320, 0, 62, 90)
        .note(320, 480, 0, 64, 80)
        .note(480, 576, 0, 65, 70)
        .note(576, 672, 0, 67, 60)
        .note(1000, 1960, 0, 72, 127)
        .off_vel(0, 0, 1, 0) // orphan note-off, repaired away
        .build(Some(2400));
    let bass = TrackBuilder::new()
        .name(0, "Bass")
        .program(0, 1, 38)
        .on(0, 1, 36, 110)
        .on(240, 1, 36, 95) // overlapping same pitch
        .off_vel(480, 1, 36, 20)
        .off(960, 1, 36)
        .build(None);
    let drums = TrackBuilder::new()
        .name(0, "Drums")
        .note(0, 48, 9, 36, 127)
        .note(160, 208, 9, 42, 70)
        .note(320, 368, 9, 42, 70)
        .note(480, 528, 9, 38, 100)
        .build(None);
    smf(Format::Parallel, 480, vec![conductor, lead, bass, drums])
}

#[test]
fn import_export_import_keeps_every_note_exact() {
    let first = import_into_project(&fixture(), &ImportOptions::default());
    let original_notes = notes_of(&first);
    assert_eq!(original_notes.len(), 6 + 2 + 4);
    assert!(original_notes.iter().any(|n| n.0 == Time::new(1, 3)));
    assert!(original_notes.iter().any(|n| n.0 == Time::new(6, 5)));

    let out = export_arrangement(
        &first,
        &ExportOptions {
            probability: ProbabilityMode::Ignore,
            ..ExportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(out.report.rounded_values, 0, "{:?}", out.report);
    assert_eq!(out.report.notes, original_notes.len());

    let second = import_into_project(&out.bytes, &ImportOptions::default());
    assert_eq!(notes_of(&second), original_notes);
    assert_eq!(second.time_signature, first.time_signature);
    assert_eq!(second.tempo, first.tempo);
    assert_eq!(second.channels.len(), first.channels.len());
    // Drums stay on channel 10, split by key again.
    let drum_channels = second
        .channels
        .iter()
        .filter(|c| c.device.type_id.starts_with("peel.drum."))
        .count();
    assert_eq!(drum_channels, 3, "kick, hat and snare");

    // And a third pass is byte-identical to the second export.
    let again = export_arrangement(
        &second,
        &ExportOptions {
            probability: ProbabilityMode::Ignore,
            ..ExportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        notes_of(&import_into_project(
            &again.bytes,
            &ImportOptions::default()
        )),
        original_notes
    );
}

#[test]
fn round_trip_at_other_resolutions_and_type0() {
    let first = import_into_project(&fixture(), &ImportOptions::default());
    let original_notes = notes_of(&first);
    for (ppq, format) in [
        (480u16, ExportFormat::Type1),
        (1920, ExportFormat::Type1),
        (960, ExportFormat::Type0),
        (15360, ExportFormat::Type0),
    ] {
        let out = export_arrangement(
            &first,
            &ExportOptions {
                ppq,
                format,
                probability: ProbabilityMode::Ignore,
                ..ExportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(out.report.rounded_values, 0, "ppq {ppq}");
        let back = import_into_project(&out.bytes, &ImportOptions::default());
        assert_eq!(notes_of(&back), original_notes, "ppq {ppq}, {format:?}");
    }
}

#[test]
fn project_to_midi_to_project_matches_for_exact_grids() {
    let mut p = Project::empty("Native");
    let lead = channel(&mut p, "Lead", "peel.synth.poly");
    let snare = channel(&mut p, "Snare", "peel.drum.snare");
    let mut notes = Vec::new();
    for i in 0..12 {
        notes.push(Note::new(
            lead,
            Time::new(i, 3),
            Time::new(1, 4),
            48 + i as u8,
            f32::from(10 + 8 * i as u8) / 127.0,
        ));
    }
    notes.push(Note::new(
        snare,
        Time::BEAT,
        Time::new(1, 8),
        38,
        114.0 / 127.0,
    ));
    let pat = add_pattern(&mut p, "Native", Time::beats(4), notes);
    let before = notes_of(&p);

    let out = export_pattern(
        &p,
        pat,
        &ExportOptions {
            probability: ProbabilityMode::Ignore,
            ..ExportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(out.report.rounded_values, 0);
    let back = import_into_project(&out.bytes, &ImportOptions::default());
    let after = notes_of(&back);
    assert_eq!(after.len(), before.len());
    // Positions, lengths, keys, velocities and the default release survive.
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(a, b);
    }
}

#[test]
fn sustain_import_then_export_bakes_pedal_into_note_lengths() {
    let track = TrackBuilder::new()
        .cc(0, 0, 64, 127)
        .note(0, 100, 0, 60, 100)
        .cc(300, 0, 64, 0)
        .build(None);
    let bytes = smf(Format::SingleTrack, 100, vec![track]);
    let project = import_into_project(
        &bytes,
        &ImportOptions {
            sustain: SustainPolicy::ExtendNotes,
            ..ImportOptions::default()
        },
    );
    let out = export_arrangement(&project, &ExportOptions::default()).unwrap();
    let (_, tracks) = parse(&out.bytes);
    assert_eq!(note_offs(&tracks[1])[0].0, 3 * 960);
}

#[test]
fn damaged_files_never_panic() {
    // Flip every byte of a real file to a few hostile values; whatever the
    // parser makes of it, importing must return a result, not crash, and a
    // successful import must merge into a valid project.
    let original = fixture();
    let mut imported = 0usize;
    for pos in 0..original.len() {
        for value in [0x00u8, 0xFF, 0x80, 0x7F, original[pos] ^ 0x55] {
            let mut bytes = original.clone();
            bytes[pos] = value;
            if let Ok(import) = import_smf(&bytes, &ImportOptions::default()) {
                imported += 1;
                let mut project = Project::empty("Fuzz");
                // A merge may legitimately refuse (for example a pattern
                // length of zero), but never panic or corrupt the project.
                let _ = import.merge_with_new_mixer_tracks(&mut project, &MergeOptions::default());
                assert!(
                    peel_document::validate(&project)
                        .iter()
                        .all(|i| !i.is_error()),
                    "byte {pos} -> {value:#x}"
                );
            }
        }
    }
    assert!(imported > original.len(), "most flips still import");

    // Truncations at every length.
    for len in 0..original.len() {
        let _ = import_smf(&original[..len], &ImportOptions::default());
    }
}
