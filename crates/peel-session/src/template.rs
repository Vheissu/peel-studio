//! Project templates built from native devices only, so they work offline
//! on a fresh install.

mod showcase;
mod songs;
#[cfg(test)]
mod tests;

use std::sync::Arc;

pub use showcase::showcase;
pub use songs::{edm, hip_hop, rap, rock_metal, synthwave, trap};

use peel_document::{
    Arrangement, Channel, Clip, ClipContent, DeviceInstance, Id, MixerTrack, Note, PALETTE,
    Pattern, Project, Time,
};

/// A device instance with its descriptor version and an optional named
/// factory preset applied.
pub fn device(type_id: &str, preset: Option<&str>) -> DeviceInstance {
    let version = peel_devices::descriptor(type_id).map_or(1, |d| d.version);
    let mut inst = DeviceInstance::new(type_id, version);
    if let Some(d) = peel_devices::descriptor(type_id) {
        for p in d.params {
            inst.params.insert(p.key.to_string(), p.default);
        }
    }
    if let Some(name) = preset
        && let Some(pr) = peel_devices::presets(type_id)
            .iter()
            .find(|p| p.name == name)
    {
        for (k, v) in pr.params {
            inst.params.insert((*k).to_string(), *v);
        }
        inst.preset = Some(name.to_string());
    }
    inst
}

/// Adds a channel with its own mixer track. Returns the channel id.
pub fn add_channel(
    p: &mut Project,
    name: &str,
    color_idx: usize,
    dev: DeviceInstance,
    root_key: u8,
) -> Id {
    let color = PALETTE[color_idx % PALETTE.len()];
    let track = MixerTrack {
        id: Id::new(),
        name: name.into(),
        color,
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
        color,
        device: dev,
        track: track.id,
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        root_key,
    };
    let id = ch.id;
    p.mixer.tracks.push(track);
    p.channels.push(ch);
    id
}

fn steps(ch: Id, positions: &[(u32, f32)], key: u8) -> Vec<Note> {
    positions
        .iter()
        .map(|&(s, vel)| Note::new(ch, Time::new(s as i64, 4), Time::new(1, 4), key, vel))
        .collect()
}

/// "Beat and melody": drums, bass, piano and a poly synth, each on its own
/// mixer track, with a starter groove arranged over eight bars.
pub fn beat_and_melody() -> Project {
    let mut p = Project::empty("Untitled");
    p.set_bpm(118.0);
    p.patterns.clear();

    let kick = add_channel(&mut p, "Kick", 0, device("peel.drum.kick", None), 60);
    let clap = add_channel(&mut p, "Clap", 1, device("peel.drum.clap", None), 60);
    let hat = add_channel(&mut p, "Hat", 2, device("peel.drum.hat", None), 60);
    let snare = add_channel(&mut p, "Snare", 9, device("peel.drum.snare", None), 60);
    let bass = add_channel(&mut p, "Bass", 6, device("peel.synth.bass", None), 45);
    let keys = add_channel(&mut p, "Piano", 4, device("peel.keys.grand", None), 60);
    let synth = add_channel(
        &mut p,
        "Synth",
        5,
        device("peel.synth.poly", Some("Warm Pad")),
        60,
    );

    // Master bus protection and a little space on the keys.
    p.mixer.tracks[0]
        .inserts
        .push(device("peel.fx.limiter", None));
    if let Some(t) = p.mixer.tracks.iter_mut().find(|t| t.name == "Piano") {
        t.inserts.push(device("peel.fx.reverb", None));
    }
    if let Some(t) = p.mixer.tracks.iter_mut().find(|t| t.name == "Synth") {
        t.gain_db = -6.0;
        t.inserts.push(device("peel.fx.delay", None));
    }
    // Leave headroom so the master limiter only catches peaks.
    for (id, db) in [
        (kick, -4.0),
        (clap, -5.0),
        (hat, -7.0),
        (snare, -8.0),
        (bass, -5.0),
        (keys, -6.0),
        (synth, -6.0),
    ] {
        if let Some(c) = p.channels.iter_mut().find(|c| c.id == id) {
            c.gain_db = db;
        }
    }

    // Beat: four on the floor, claps on 2 and 4, off-beat hats with ghosts.
    let mut beat_notes = steps(kick, &[(0, 1.0), (4, 0.95), (8, 1.0), (12, 0.95)], 60);
    beat_notes.extend(steps(clap, &[(4, 0.9), (12, 0.9)], 60));
    beat_notes.extend(steps(
        hat,
        &[
            (2, 0.85),
            (6, 0.85),
            (10, 0.85),
            (14, 0.85),
            (7, 0.35),
            (15, 0.4),
        ],
        60,
    ));
    beat_notes.extend(steps(snare, &[(15, 0.3)], 60));
    let beat = Pattern {
        id: Id::new(),
        name: "Beat".into(),
        color: PALETTE[0],
        length: Time::beats(4),
        notes: beat_notes,
        expression: Vec::new(),
    };

    // Chords: Am - F - C - G, one bar each.
    let chords: [&[u8]; 4] = [&[57, 60, 64], &[53, 57, 60], &[55, 60, 64], &[55, 59, 62]];
    let mut chord_notes = Vec::new();
    for (bar, keys_in_chord) in chords.iter().enumerate() {
        for &k in keys_in_chord.iter() {
            chord_notes.push(Note::new(
                keys,
                Time::beats(bar as i64 * 4),
                Time::new(7, 2),
                k,
                0.7,
            ));
        }
        // Pad doubles the chord an octave down, softly.
        for &k in keys_in_chord.iter() {
            chord_notes.push(Note::new(
                synth,
                Time::beats(bar as i64 * 4),
                Time::beats(4),
                k - 12,
                0.45,
            ));
        }
    }
    let chords_pat = Pattern {
        id: Id::new(),
        name: "Chords".into(),
        color: PALETTE[4],
        length: Time::beats(16),
        notes: chord_notes,
        expression: Vec::new(),
    };

    // Bass: off-beat eighths on the chord roots.
    let roots = [45u8, 41, 48, 43];
    let mut bass_notes = Vec::new();
    for (bar, &root) in roots.iter().enumerate() {
        for beat_i in 0..4 {
            let start = Time::beats(bar as i64 * 4 + beat_i) + Time::new(1, 2);
            bass_notes.push(Note::new(bass, start, Time::new(3, 8), root, 0.85));
        }
    }
    let bass_pat = Pattern {
        id: Id::new(),
        name: "Bass".into(),
        color: PALETTE[6],
        length: Time::beats(16),
        notes: bass_notes,
        expression: Vec::new(),
    };

    let (beat_id, chords_id, bass_id) = (beat.id, chords_pat.id, bass_pat.id);
    p.patterns = vec![Arc::new(beat), Arc::new(chords_pat), Arc::new(bass_pat)];

    let arr: &mut Arrangement = p.arrangement_mut();
    for (i, name) in ["Drums", "Chords", "Bass"].iter().enumerate() {
        arr.tracks[i].name = (*name).to_string();
    }
    let lanes: Vec<Id> = arr.tracks.iter().map(|t| t.id).collect();
    let clip = |track: Id, start: i64, len: i64, pattern: Id| Clip {
        id: Id::new(),
        track,
        start: Time::beats(start),
        length: Time::beats(len),
        offset: Time::ZERO,
        muted: false,
        content: ClipContent::Pattern { pattern },
    };
    arr.clips.push(clip(lanes[0], 0, 32, beat_id));
    arr.clips.push(clip(lanes[1], 0, 32, chords_id));
    arr.clips.push(clip(lanes[2], 16, 16, bass_id));
    for (beat, name) in [(0, "Intro"), (16, "Drop")] {
        arr.markers.push(peel_document::Marker {
            id: Id::new(),
            position: Time::beats(beat),
            name: name.into(),
        });
    }
    p
}

fn pattern(name: &str, color: usize, beats: i64, notes: Vec<Note>) -> Pattern {
    Pattern {
        id: Id::new(),
        name: name.into(),
        color: PALETTE[color % PALETTE.len()],
        length: Time::beats(beats),
        notes,
        expression: Vec::new(),
    }
}

/// Chords as stabs: every `(bar, keys)` played on the given sixteenth steps.
fn stabs(ch: Id, chords: &[&[u8]], at: &[u32], len: Time, vel: f32) -> Vec<Note> {
    let mut v = Vec::new();
    for (bar, keys) in chords.iter().enumerate() {
        for &step in at {
            for &k in keys.iter() {
                let start = Time::beats(bar as i64 * 4) + Time::new(step as i64, 4);
                v.push(Note::new(ch, start, len, k, vel));
            }
        }
    }
    v
}

fn set_gains(p: &mut Project, gains: &[(Id, f32)]) {
    for (id, db) in gains {
        if let Some(c) = p.channels.iter_mut().find(|c| c.id == *id) {
            c.gain_db = *db;
        }
    }
}

/// Lays clips out: `(lane, pattern, start bar, bars)`, and names the lanes.
fn arrange(
    p: &mut Project,
    lanes: &[&str],
    clips: &[(usize, Id, i64, i64)],
    markers: &[(i64, &str)],
) {
    let bar = 4;
    let arr: &mut Arrangement = p.arrangement_mut();
    for (i, name) in lanes.iter().enumerate() {
        arr.tracks[i].name = (*name).to_string();
    }
    let ids: Vec<Id> = arr.tracks.iter().map(|t| t.id).collect();
    for &(lane, pattern, start, bars) in clips {
        arr.clips.push(Clip {
            id: Id::new(),
            track: ids[lane],
            start: Time::beats(start * bar),
            length: Time::beats(bars * bar),
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::Pattern { pattern },
        });
    }
    for &(at, name) in markers {
        arr.markers.push(peel_document::Marker {
            id: Id::new(),
            position: Time::beats(at * bar),
            name: name.into(),
        });
    }
}

/// House at 124 BPM in F minor: four-on-the-floor, off-beat bass, chord
/// stabs and a pad that ducks under the kick through a sidechain.
pub fn house() -> Project {
    let mut p = Project::empty("Untitled");
    p.set_bpm(124.0);
    p.patterns.clear();
    let kick = add_channel(
        &mut p,
        "Kick",
        0,
        device("peel.drum.kick", Some("Punch")),
        60,
    );
    let clap = add_channel(
        &mut p,
        "Clap",
        1,
        device("peel.drum.clap", Some("Big Room")),
        60,
    );
    let hat = add_channel(&mut p, "Hat", 2, device("peel.drum.hat", Some("Open")), 60);
    let shaker = add_channel(
        &mut p,
        "Shaker",
        3,
        device("peel.drum.hat", Some("Tight")),
        60,
    );
    let bass = add_channel(
        &mut p,
        "Bass",
        6,
        device("peel.synth.bass", Some("Funk")),
        41,
    );
    let keys = add_channel(
        &mut p,
        "Keys",
        4,
        device("peel.keys.tine", Some("Classic")),
        60,
    );
    let pad = add_channel(
        &mut p,
        "Pad",
        5,
        device("peel.synth.poly", Some("Strings")),
        60,
    );
    set_gains(
        &mut p,
        &[
            (kick, -3.0),
            (clap, -6.0),
            (hat, -9.0),
            (shaker, -14.0),
            (bass, -6.0),
            (keys, -8.0),
            (pad, -10.0),
        ],
    );

    p.mixer.tracks[0]
        .inserts
        .push(device("peel.fx.limiter", None));
    let kick_track = p.channel(kick).map(|c| c.track);
    for (ch, fx) in [(pad, "peel.fx.reverb"), (keys, "peel.fx.delay")] {
        let track = p.channel(ch).map(|c| c.track);
        if let Some(t) = track.and_then(|t| p.mixer.track_mut(t)) {
            t.inserts.push(device(fx, None));
        }
    }
    // The pad and bass pump with the kick.
    for ch in [pad, bass] {
        let track = p.channel(ch).map(|c| c.track);
        if let Some(t) = track.and_then(|t| p.mixer.track_mut(t)) {
            let mut duck = device("peel.fx.compressor", Some("Sidechain Duck"));
            duck.sidechain = kick_track;
            duck.preset = Some("Sidechain Duck".into());
            t.inserts.insert(0, duck);
        }
    }

    let mut beat = steps(kick, &[(0, 1.0), (4, 1.0), (8, 1.0), (12, 1.0)], 60);
    beat.extend(steps(clap, &[(4, 0.9), (12, 0.9)], 60));
    beat.extend(steps(hat, &[(2, 0.8), (6, 0.8), (10, 0.8), (14, 0.8)], 60));
    beat.extend(steps(
        shaker,
        &(0..16)
            .map(|i| (i, if i % 2 == 0 { 0.5 } else { 0.8 }))
            .collect::<Vec<_>>(),
        60,
    ));
    let beat = pattern("Beat", 0, 4, beat);

    // Fm7 - Dbmaj7 - Ebsus - Cm7
    let chords: [&[u8]; 4] = [
        &[56, 60, 63, 65],
        &[56, 60, 61, 65],
        &[58, 63, 65, 68],
        &[55, 58, 60, 63],
    ];
    let mut chord_notes = stabs(keys, &chords, &[3, 6, 10], Time::new(3, 8), 0.75);
    for (bar, ks) in chords.iter().enumerate() {
        for &k in ks.iter() {
            chord_notes.push(Note::new(
                pad,
                Time::beats(bar as i64 * 4),
                Time::beats(4),
                k - 12,
                0.5,
            ));
        }
    }
    let chords_pat = pattern("Chords", 4, 16, chord_notes);

    let roots = [41u8, 37, 39, 36];
    let mut bass_notes = Vec::new();
    for (bar, &root) in roots.iter().enumerate() {
        for b in 0..4 {
            let start = Time::beats(bar as i64 * 4 + b) + Time::new(1, 2);
            let key = if b == 3 { root + 12 } else { root };
            bass_notes.push(Note::new(bass, start, Time::new(3, 8), key, 0.85));
        }
    }
    let bass_pat = pattern("Bass", 6, 16, bass_notes);

    let (b, c, l) = (beat.id, chords_pat.id, bass_pat.id);
    p.patterns = vec![Arc::new(beat), Arc::new(chords_pat), Arc::new(bass_pat)];
    arrange(
        &mut p,
        &["Drums", "Chords", "Bass"],
        &[(0, b, 0, 16), (1, c, 4, 12), (2, l, 8, 8)],
        &[(0, "Intro"), (4, "Build"), (8, "Groove")],
    );
    p
}

/// A starter project: its name and how to build it.
pub type Template = (&'static str, fn() -> Project);

/// Starter projects offered on the welcome screen.
pub fn templates() -> Vec<Template> {
    vec![
        ("Demo song", showcase),
        ("Beat and melody", beat_and_melody),
        ("House", house),
        ("Hip-hop", hip_hop),
        ("Rap", rap),
        ("Trap", trap),
        ("Synthwave", synthwave),
        ("EDM", edm),
        (ROCK_METAL, rock_metal),
    ]
}

/// Catalog name of the rock / metal song.
const ROCK_METAL: &str = "Rock / Metal";

/// Where the editor opens on a template that has a groove to show first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Starter {
    /// Pattern shown in Channels.
    pub pattern: &'static str,
    /// Channel selected in it.
    pub channel: &'static str,
    /// Step and note grid for the Channel Rack and Piano Roll.
    pub grid: Time,
}

/// The starting view for the template called `name`, if it has one.
pub fn starter(name: &str) -> Option<Starter> {
    (name == ROCK_METAL).then_some(Starter {
        pattern: "Double kick",
        channel: "Kick",
        grid: Time::new(1, 6),
    })
}

/// A blank project: one empty pattern, no channels, no clips and no
/// effects on the master.
pub fn empty() -> Project {
    Project::empty("Untitled")
}
