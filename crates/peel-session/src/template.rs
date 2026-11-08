//! Project templates built from native devices only, so they work offline
//! on a fresh install.

use std::sync::Arc;

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
        (clap, -7.0),
        (hat, -10.0),
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
    p
}

/// An empty project with one pattern and no channels.
pub fn empty() -> Project {
    Project::empty("Untitled")
}

#[cfg(test)]
mod tests {
    #[test]
    fn template_is_valid() {
        let p = super::beat_and_melody();
        let issues = peel_document::validate(&p);
        assert!(issues.iter().all(|i| !i.is_error()), "{issues:?}");
        assert_eq!(p.channels.len(), 7);
        assert_eq!(p.patterns.len(), 3);
    }
}
