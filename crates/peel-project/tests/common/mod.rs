#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use peel_document::{
    Arrangement, Channel, Clip, ClipContent, Color, DeviceInstance, Id, Marker, MixerTrack, Note,
    PALETTE, Pattern, Project, TempoSegment, Time, Track, validate,
};

/// A project exercising every part of the model: triplet and fractional
/// times, a tempo ramp, several channels and patterns, two arrangements,
/// a managed and an external asset, mixer routing, inserts and extensions.
pub fn rich_project(managed_asset: Option<peel_document::AudioAsset>) -> Project {
    let mut p = Project::empty("Rich Song");
    p.meta.artist = "Test Artist".into();
    p.meta.notes = "line one\nline two \u{1F3B5}".into();
    p.seed = u64::MAX - 12345;
    p.swing = 0.25;
    p.revision = 41;
    p.tempo = vec![
        TempoSegment {
            start: Time::ZERO,
            bpm: 128.5,
            ramp_to: Some(140.3),
        },
        TempoSegment {
            start: Time::beats(16),
            bpm: 140.3,
            ramp_to: None,
        },
    ];
    p.time_signature = peel_document::TimeSignature {
        numerator: 7,
        denominator: 8,
    };

    // Mixer: bus plus a track with inserts.
    let bus = MixerTrack {
        id: Id::new(),
        name: "Drum bus".into(),
        color: PALETTE[2],
        gain_db: -3.5,
        pan: -0.25,
        muted: false,
        solo: false,
        inserts: vec![{
            let mut d = DeviceInstance::new("peel.fx.compressor", 2);
            d.params.insert("threshold".into(), -18.25);
            d.params.insert("ratio".into(), 4.0);
            d.preset = Some("Glue".into());
            d
        }],
        output: None,
        sends: Vec::new(),
    };
    let lead_track = MixerTrack {
        id: Id::new(),
        name: "Lead".into(),
        color: PALETTE[5],
        gain_db: 1.5,
        pan: 0.1,
        muted: false,
        solo: true,
        inserts: vec![],
        output: Some(bus.id),
        sends: Vec::new(),
    };
    p.mixer.tracks.push(bus.clone());
    p.mixer.tracks.push(lead_track.clone());

    let kick = channel("Kick", "peel.drum.kick", bus.id, 36);
    let mut lead = channel("Lead", "peel.synth.poly", lead_track.id, 72);
    lead.device.params.insert("cutoff".into(), 0.123_456_79);
    lead.device.params.insert("resonance".into(), 0.7);
    lead.gain_db = -6.0;
    lead.pan = 0.3;
    let mut sampler = channel("Chop", "peel.sampler", bus.id, 60);
    sampler.muted = true;

    // Assets.
    if let Some(asset) = managed_asset {
        sampler.device.sample = Some(asset.id);
        p.assets.push(asset);
    }
    p.assets.push(peel_document::AudioAsset {
        id: Id::new(),
        hash: "0".repeat(64),
        name: "external loop.wav".into(),
        path: "/Users/someone/Music/external loop.wav".into(),
        managed: false,
        sample_rate: 48_000,
        channels: 2,
        frames: 1_234_567_890,
    });

    p.channels = vec![kick.clone(), lead.clone(), sampler.clone()];

    // Pattern 1: triplets and tuplets exactly.
    let mut triplets = Pattern {
        id: Id::new(),
        name: "Triplets".into(),
        color: PALETTE[3],
        length: Time::new(7, 2),
        notes: vec![],
    };
    for i in 0..9 {
        let mut n = Note::new(
            lead.id,
            Time::new(i, 3),
            Time::new(1, 3),
            60 + (i as u8 % 12),
            0.25 + i as f32 / 20.0,
        );
        n.fine = 12.5;
        n.pan = -0.5;
        n.probability = 0.75;
        n.release = 0.3;
        triplets.notes.push(n);
    }
    triplets.notes.push(Note::new(
        kick.id,
        Time::new(5, 7),
        Time::new(1, 960),
        36,
        1.0,
    ));
    let mut muted = Note::new(sampler.id, Time::new(1, 5), Time::new(2, 5), 48, 0.5);
    muted.muted = true;
    triplets.notes.push(muted);
    triplets.sort_notes();

    // Pattern 2: a plain step pattern.
    let mut steps = Pattern {
        id: Id::new(),
        name: "Four on the floor".into(),
        color: PALETTE[0],
        length: Time::beats(4),
        notes: vec![],
    };
    for beat in 0..4 {
        steps.notes.push(Note::new(
            kick.id,
            Time::beats(beat),
            Time::new(1, 4),
            36,
            0.9,
        ));
    }

    // Arrangements.
    let main_tracks: Vec<Track> = (1..=3)
        .map(|i| Track {
            id: Id::new(),
            name: format!("Lane {i}"),
            color: if i == 2 { Some(PALETTE[7]) } else { None },
            muted: i == 3,
        })
        .collect();
    let mut main = Arrangement {
        id: Id::new(),
        name: "Main".into(),
        tracks: main_tracks.clone(),
        clips: vec![
            Clip {
                id: Id::new(),
                track: main_tracks[0].id,
                start: Time::ZERO,
                length: Time::beats(16),
                offset: Time::new(1, 3),
                muted: false,
                content: ClipContent::Pattern { pattern: steps.id },
            },
            Clip {
                id: Id::new(),
                track: main_tracks[1].id,
                start: Time::new(7, 3),
                length: Time::new(14, 3),
                offset: Time::ZERO,
                muted: true,
                content: ClipContent::Pattern {
                    pattern: triplets.id,
                },
            },
        ],
        markers: vec![Marker {
            id: Id::new(),
            position: Time::beats(8),
            name: "Drop".into(),
        }],
        loop_range: Some((Time::beats(4), Time::new(35, 2))),
    };
    if let Some(asset) = p.assets.iter().find(|a| a.managed) {
        main.clips.push(Clip {
            id: Id::new(),
            track: main_tracks[2].id,
            start: Time::beats(32),
            length: Time::beats(8),
            offset: Time::new(1, 7),
            muted: false,
            content: ClipContent::Audio {
                asset: asset.id,
                gain_db: -2.25,
                reversed: true,
                track: lead_track.id,
                fade_in: Time::new(1, 4),
                fade_out: Time::new(3, 8),
                source_bpm: None,
            },
        });
    }
    let alt_track = Track {
        id: Id::new(),
        name: "Only".into(),
        color: None,
        muted: false,
    };
    let alt = Arrangement {
        id: Id::new(),
        name: "Alt mix".into(),
        tracks: vec![alt_track.clone()],
        clips: vec![Clip {
            id: Id::new(),
            track: alt_track.id,
            start: Time::beats(2),
            length: Time::beats(4),
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::Pattern { pattern: steps.id },
        }],
        markers: vec![],
        loop_range: None,
    };
    p.active_arrangement = alt.id;
    p.patterns = vec![Arc::new(triplets), Arc::new(steps)];
    p.arrangements = vec![Arc::new(main), Arc::new(alt)];

    p.extensions.insert(
        "vendor.zeta".into(),
        serde_json::json!({"z": [1, 2, {"b": 1, "a": 2}], "a": "text", "n": 1.5}),
    );
    p.extensions
        .insert("vendor.alpha".into(), serde_json::json!(null));

    let issues = validate(&p);
    assert!(issues.iter().all(|i| !i.is_error()), "{issues:?}");
    p
}

fn channel(name: &str, device: &str, track: Id, root_key: u8) -> Channel {
    Channel {
        id: Id::new(),
        name: name.into(),
        color: Color::hex(0x336699),
        device: DeviceInstance::new(device, 1),
        track,
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        root_key,
    }
}

/// Paths of every shard file under a project's data directory.
pub fn shard_files(project_path: &Path) -> Vec<PathBuf> {
    let dir = peel_project::data_dir_for(project_path).join("objects");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files
}

/// Writes a small fake audio file and returns its path.
pub fn fake_audio(dir: &Path, name: &str, fill: u8, len: usize) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, vec![fill; len]).unwrap();
    path
}
