//! "Afterglow": the project Peel Studio opens with. An 80-bar song in
//! D minor that uses every part of the app: layered drums through a bus,
//! basses, electric piano, grand piano, strings, violin, sax, guitars and
//! synth leads; sidechain ducking, reverb and delay returns; tempo ramps,
//! a slower breakdown and two drops; and automation of volume, pan, filters,
//! pitch and effect sends.

use std::sync::Arc;

use peel_document::{
    AutomationPoint, AutomationTarget, Clip, ClipContent, DeviceInstance, Id, Marker, MixerSend,
    MixerTrack, Note, PALETTE, Project, TempoSegment, Time, Track, db_to_fader,
};

use super::{add_channel, device, pattern, set_gains, steps};

/// One bar of 4/4, in quarter notes.
const BAR: i64 = 4;

/// D minor: Dm, Bb, F, C (i, VI, III, VII), one bar each.
const ROOTS: [u8; 4] = [38, 34, 41, 36];

/// A note from sixteenth-note positions.
fn n16(ch: Id, start: i64, len: i64, key: u8, vel: f32) -> Note {
    Note::new(ch, Time::new(start, 4), Time::new(len, 4), key, vel)
}

/// A melody as `(start, length, key)` in sixteenths, with a gentle
/// velocity shape so it does not sound mechanical.
fn melody(ch: Id, line: &[(i64, i64, u8)], vel: f32) -> Vec<Note> {
    line.iter()
        .enumerate()
        .map(|(i, &(s, l, k))| {
            let accent = if s % 4 == 0 { 0.08 } else { 0.0 };
            let wobble = [0.0, -0.05, 0.03, -0.02][i % 4];
            n16(ch, s, l, k, (vel + accent + wobble).clamp(0.1, 1.0))
        })
        .collect()
}

/// A chord per bar, each hit on the given sixteenths, strummed slightly
/// when `strum` is set.
fn chord_hits(
    ch: Id,
    chords: &[&[u8]],
    hits: &[(i64, i64, f32)],
    strum: bool,
    transpose: i8,
) -> Vec<Note> {
    let mut v = Vec::new();
    for (bar, keys) in chords.iter().enumerate() {
        for &(step, len, vel) in hits {
            for (i, &k) in keys.iter().enumerate() {
                let roll = if strum {
                    Time::new(i as i64, 96)
                } else {
                    Time::ZERO
                };
                let start = Time::new(bar as i64 * 16 + step, 4) + roll;
                let key = (k as i16 + transpose as i16).clamp(0, 127) as u8;
                v.push(Note::new(ch, start, Time::new(len, 4), key, vel));
            }
        }
    }
    v
}

/// Mixer bus with no channel of its own.
fn add_bus(p: &mut Project, name: &str, color: usize) -> Id {
    let track = MixerTrack {
        id: Id::new(),
        name: name.into(),
        color: PALETTE[color % PALETTE.len()],
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        solo: false,
        inserts: vec![],
        output: None,
        sends: Vec::new(),
    };
    let id = track.id;
    p.mixer.tracks.push(track);
    id
}

fn track_of(p: &Project, ch: Id) -> Id {
    p.channel(ch).map(|c| c.track).unwrap_or_default()
}

fn device_of(p: &Project, ch: Id) -> Id {
    p.channel(ch).map(|c| c.device.id).unwrap_or_default()
}

fn mixer(p: &mut Project, track: Id) -> &mut MixerTrack {
    p.mixer.track_mut(track).expect("template tracks exist")
}

/// Adds an insert and returns its device id.
fn insert(p: &mut Project, track: Id, dev: DeviceInstance) -> Id {
    let id = dev.id;
    mixer(p, track).inserts.push(dev);
    id
}

/// A device with a preset and some parameters changed from it.
fn tweaked(type_id: &str, preset: Option<&str>, params: &[(&str, f32)]) -> DeviceInstance {
    let mut d = device(type_id, preset);
    for (k, v) in params {
        d.params.insert((*k).to_string(), *v);
    }
    d
}

/// Normalized (0..=1) automation value for a plain parameter value.
fn norm(type_id: &str, key: &str, plain: f32) -> f32 {
    peel_devices::descriptor(type_id)
        .and_then(|d| d.param_by_key(key))
        .map_or(0.5, |p| p.to_normalized(plain))
}

/// Envelope from `(beat, value, tension)` breakpoints.
fn envelope(points: &[(f64, f32, f32)]) -> Vec<AutomationPoint> {
    points
        .iter()
        .map(|&(beat, value, tension)| AutomationPoint {
            position: Time::new((beat * 4.0).round() as i64, 4),
            value: value.clamp(0.0, 1.0),
            tension,
        })
        .collect()
}

/// A builder for the playlist: one named lane per part.
struct Lanes {
    ids: Vec<Id>,
}

impl Lanes {
    fn new(p: &mut Project, names: &[&str]) -> Lanes {
        let arr = p.arrangement_mut();
        while arr.tracks.len() < names.len() {
            arr.tracks.push(Track {
                id: Id::new(),
                name: String::new(),
                color: None,
                muted: false,
            });
        }
        for (t, name) in arr.tracks.iter_mut().zip(names) {
            t.name = (*name).to_string();
        }
        Lanes {
            ids: arr.tracks.iter().map(|t| t.id).collect(),
        }
    }

    /// Places `pattern` on `lane` from `bar` for `bars` bars.
    fn put(&self, p: &mut Project, lane: usize, pattern: Id, bar: i64, bars: i64) {
        p.arrangement_mut().clips.push(Clip {
            id: Id::new(),
            track: self.ids[lane],
            start: Time::beats(bar * BAR),
            length: Time::beats(bars * BAR),
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::Pattern { pattern },
        });
    }

    /// A full-length automation clip, so every part of the song (and every
    /// loop back to the start) sees a defined value.
    fn automate(
        &self,
        p: &mut Project,
        lane: usize,
        bars: i64,
        target: AutomationTarget,
        points: &[(f64, f32, f32)],
    ) {
        p.arrangement_mut().clips.push(Clip {
            id: Id::new(),
            track: self.ids[lane],
            start: Time::ZERO,
            length: Time::beats(bars * BAR),
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::Automation {
                target,
                points: envelope(points),
            },
        });
    }
}

/// The showcase song.
pub fn showcase() -> Project {
    let mut p = Project::empty("Afterglow");
    p.meta.artist = "Peel Studio".into();
    p.meta.notes = "The demo song. Press Space to play; every part is editable.".into();
    p.patterns.clear();

    // Tempo: a steady start, a ramp into the first drop, a slower
    // breakdown, a long accelerando into the second drop and a slowdown at
    // the end.
    let seg = |bar: i64, bpm: f64, ramp_to: Option<f64>| TempoSegment {
        start: Time::beats(bar * BAR),
        bpm,
        ramp_to,
    };
    p.tempo = vec![
        seg(0, 120.0, None),
        seg(16, 120.0, Some(128.0)),
        seg(24, 128.0, None),
        seg(40, 92.0, None),
        seg(48, 92.0, Some(128.0)),
        seg(56, 128.0, None),
        seg(72, 128.0, Some(108.0)),
        seg(80, 108.0, None),
    ];

    // Drums.
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
    let snare = add_channel(
        &mut p,
        "Snare",
        1,
        device("peel.drum.snare", Some("Crack")),
        60,
    );
    let hat = add_channel(
        &mut p,
        "Hat",
        2,
        device("peel.drum.hat", Some("Closed")),
        60,
    );
    let open_hat = add_channel(
        &mut p,
        "Open Hat",
        2,
        device("peel.drum.hat", Some("Open")),
        60,
    );
    let shaker = add_channel(
        &mut p,
        "Shaker",
        3,
        device("peel.drum.hat", Some("Tight")),
        60,
    );
    let tom = add_channel(&mut p, "Tom", 0, device("peel.drum.tom", Some("Floor")), 60);
    let crash = add_channel(
        &mut p,
        "Crash",
        2,
        tweaked(
            "peel.drum.hat",
            Some("Sizzle"),
            &[
                ("amp.decay", 1.4),
                ("tone.tune", -3.0),
                ("tone.metal", 0.45),
            ],
        ),
        60,
    );
    // Low end.
    let sub = add_channel(&mut p, "Sub", 6, device("peel.synth.bass", Some("Sub")), 38);
    let bass = add_channel(
        &mut p,
        "Bass",
        6,
        device("peel.synth.bass", Some("Synthwave")),
        38,
    );
    let groove_bass = add_channel(
        &mut p,
        "Groove Bass",
        7,
        device("peel.synth.bass", Some("Fingered")),
        38,
    );
    // Keys and orchestra.
    let keys = add_channel(
        &mut p,
        "Keys",
        4,
        device("peel.keys.tine", Some("Classic")),
        60,
    );
    let piano = add_channel(
        &mut p,
        "Piano",
        4,
        device("peel.keys.grand", Some("Soft")),
        60,
    );
    let strings = add_channel(
        &mut p,
        "Strings",
        5,
        device("peel.strings.violin", Some("Ensemble")),
        60,
    );
    let violin = add_channel(
        &mut p,
        "Violin",
        5,
        device("peel.strings.violin", Some("Legato Lead")),
        74,
    );
    let pizz = add_channel(
        &mut p,
        "Pizzicato",
        5,
        device("peel.strings.violin", Some("Pizzicato")),
        74,
    );
    let sax = add_channel(&mut p, "Sax", 8, device("peel.wind.sax", Some("Alto")), 69);
    // Guitars.
    let funk_gtr = add_channel(
        &mut p,
        "Funk Guitar",
        9,
        device("peel.guitar.electric", Some("Funk")),
        62,
    );
    let rhythm_gtr = add_channel(
        &mut p,
        "Rhythm Guitar",
        9,
        device("peel.guitar.electric", Some("Palm Mute")),
        50,
    );
    let lead_gtr = add_channel(
        &mut p,
        "Lead Guitar",
        9,
        device("peel.guitar.electric", Some("Lead")),
        62,
    );
    // Synths.
    let lead = add_channel(
        &mut p,
        "Lead",
        10,
        device("peel.synth.poly", Some("Supersaw Lead")),
        74,
    );
    let stabs = add_channel(
        &mut p,
        "Stabs",
        11,
        device("peel.synth.poly", Some("Super Chord")),
        69,
    );
    let riser = add_channel(
        &mut p,
        "Riser",
        11,
        device("peel.synth.poly", Some("Noise Riser")),
        72,
    );

    set_gains(
        &mut p,
        &[
            (kick, -3.0),
            (clap, -7.0),
            (snare, -11.0),
            (hat, -9.0),
            (open_hat, -13.0),
            (shaker, -12.0),
            (tom, -11.0),
            (crash, -14.0),
            (sub, -12.0),
            (bass, -10.0),
            (groove_bass, -9.0),
            (keys, -9.0),
            (piano, -9.0),
            (strings, -11.0),
            (violin, -9.0),
            (pizz, -7.0),
            (sax, -10.0),
            (funk_gtr, -9.0),
            (rhythm_gtr, -15.0),
            (lead_gtr, -14.0),
            (lead, -8.0),
            (stabs, -12.0),
            (riser, -8.0),
        ],
    );

    // Mixer: a drum bus, a reverb and a delay return, sidechain ducking
    // under the kick, and a mastering chain.
    let master = p.mixer.tracks[0].id;
    insert(&mut p, master, device("peel.fx.eq", Some("Clean Low End")));
    insert(
        &mut p,
        master,
        device("peel.fx.compressor", Some("Gentle Glue")),
    );
    insert(
        &mut p,
        master,
        tweaked("peel.fx.limiter", Some("Master Safety"), &[("input", 3.0)]),
    );

    let drum_bus = add_bus(&mut p, "Drum Bus", 0);
    let drum_filter = insert(
        &mut p,
        drum_bus,
        tweaked(
            "peel.fx.filter",
            None,
            &[
                ("mode", 1.0),
                ("cutoff", 20.0),
                ("slope", 1.0),
                ("resonance", 0.3),
            ],
        ),
    );
    insert(
        &mut p,
        drum_bus,
        device("peel.fx.compressor", Some("Punchy Drums")),
    );
    for ch in [kick, clap, snare, hat, open_hat, shaker, tom, crash] {
        let t = track_of(&p, ch);
        mixer(&mut p, t).output = Some(drum_bus);
    }

    let hall = add_bus(&mut p, "Hall", 5);
    insert(
        &mut p,
        hall,
        tweaked("peel.fx.reverb", Some("Warm Hall"), &[("mix", 1.0)]),
    );
    let echo = add_bus(&mut p, "Echo", 10);
    insert(
        &mut p,
        echo,
        tweaked("peel.fx.delay", Some("Dotted Ping-Pong"), &[("mix", 1.0)]),
    );
    insert(&mut p, echo, device("peel.fx.eq", Some("Clean Low End")));
    let sends: [(Id, Id, f32); 11] = [
        (clap, hall, -16.0),
        (keys, hall, -12.0),
        (piano, hall, -8.0),
        (strings, hall, -9.0),
        (violin, hall, -6.0),
        (pizz, hall, -10.0),
        (sax, hall, -9.0),
        (lead, hall, -12.0),
        (lead, echo, -14.0),
        (lead_gtr, echo, -12.0),
        (violin, echo, -16.0),
    ];
    for (ch, target, gain_db) in sends {
        let t = track_of(&p, ch);
        mixer(&mut p, t).sends.push(MixerSend { target, gain_db });
    }

    let kick_track = track_of(&p, kick);
    for ch in [sub, bass, stabs, strings, rhythm_gtr] {
        let t = track_of(&p, ch);
        let mut duck = device("peel.fx.compressor", Some("Sidechain Duck"));
        duck.sidechain = Some(kick_track);
        mixer(&mut p, t).inserts.insert(0, duck);
    }

    // Glitch: a tape stop into the breakdown and a stutter into the
    // second drop on the whole mix, and a trance gate on the strings.
    let master_glitch = device("peel.fx.glitch", None);
    let master_glitch_id = master_glitch.id;
    mixer(&mut p, master).inserts.insert(0, master_glitch);
    let strings_mix = track_of(&p, strings);
    let strings_gate = insert(
        &mut p,
        strings_mix,
        tweaked("peel.fx.glitch", None, &[("gate.smooth", 0.35)]),
    );

    let keys_track = track_of(&p, keys);
    let keys_filter = insert(
        &mut p,
        keys_track,
        tweaked(
            "peel.fx.filter",
            None,
            &[("cutoff", 20000.0), ("resonance", 0.35), ("slope", 1.0)],
        ),
    );
    insert(
        &mut p,
        keys_track,
        device("peel.fx.vintage_chorus", Some("Both")),
    );
    let sax_track = track_of(&p, sax);
    let sax_delay = insert(
        &mut p,
        sax_track,
        tweaked(
            "peel.fx.delay",
            Some("Wide Quarter"),
            &[("mix", 0.0), ("feedback", 0.45)],
        ),
    );
    let funk_track = track_of(&p, funk_gtr);
    insert(&mut p, funk_track, device("peel.fx.phaser", Some("Subtle")));
    let lead_track = track_of(&p, lead);
    insert(
        &mut p,
        lead_track,
        device("peel.fx.vintage_chorus", Some("Subtle")),
    );
    let riser_track = track_of(&p, riser);
    insert(
        &mut p,
        riser_track,
        device("peel.fx.flanger", Some("Slow Sweep")),
    );
    insert(
        &mut p,
        riser_track,
        device("peel.fx.reverb", Some("Ambient Wash")),
    );
    // Harmonics of the sub so it still reads on laptop speakers.
    let sub_track = track_of(&p, sub);
    insert(
        &mut p,
        sub_track,
        device("peel.fx.bass_lift", Some("Small Speakers")),
    );
    let piano_track = track_of(&p, piano);
    insert(&mut p, piano_track, device("peel.fx.eq", Some("Air")));

    // Patterns. Chord voicings per bar follow Dm, Bb, F, C.
    let mut pats: Vec<peel_document::Pattern> = Vec::new();
    let mut add = |pat: peel_document::Pattern| {
        let id = pat.id;
        pats.push(pat);
        id
    };

    let sixteenths = |vels: [f32; 4]| -> Vec<(u32, f32)> {
        (0..16).map(|i| (i, vels[i as usize % 4])).collect()
    };

    // Verse groove.
    let mut notes = steps(kick, &[(0, 1.0), (4, 0.9), (8, 1.0), (12, 0.9)], 60);
    notes.extend(steps(clap, &[(4, 0.7), (12, 0.75)], 60));
    notes.extend(steps(
        hat,
        &[
            (2, 0.75),
            (6, 0.75),
            (7, 0.3),
            (10, 0.75),
            (14, 0.75),
            (15, 0.35),
        ],
        60,
    ));
    notes.extend(steps(shaker, &sixteenths([0.4, 0.6, 0.45, 0.7]), 60));
    let groove = add(pattern("Groove", 0, 4, notes));

    // Drop beat.
    let mut notes = steps(kick, &[(0, 1.0), (4, 1.0), (8, 1.0), (12, 1.0)], 60);
    notes.extend(steps(clap, &[(4, 0.95), (12, 0.95)], 60));
    notes.extend(steps(snare, &[(4, 0.6), (12, 0.6), (15, 0.25)], 60));
    notes.extend(steps(hat, &sixteenths([0.45, 0.3, 0.8, 0.35]), 60));
    notes.extend(steps(
        open_hat,
        &[(2, 0.7), (6, 0.7), (10, 0.7), (14, 0.75)],
        60,
    ));
    notes.extend(steps(shaker, &sixteenths([0.35, 0.5, 0.35, 0.55]), 60));
    let drop_beat = add(pattern("Drop Beat", 0, 4, notes));

    let kick_only = add(pattern(
        "Four on the Floor",
        0,
        4,
        steps(kick, &[(0, 1.0), (4, 0.95), (8, 1.0), (12, 0.95)], 60),
    ));

    // Breakdown: a half-time groove.
    let mut notes = steps(kick, &[(0, 0.85), (10, 0.7)], 60);
    notes.extend(steps(snare, &[(8, 0.8)], 60));
    notes.extend(steps(
        hat,
        &(0..8)
            .map(|i| (i * 2, if i % 2 == 0 { 0.4 } else { 0.3 }))
            .collect::<Vec<_>>(),
        60,
    ));
    let half_time = add(pattern("Half Time", 0, 4, notes));

    // An eight-bar snare roll that speeds up and gets louder.
    let mut roll = Vec::new();
    let mut hit = |pos32: i64| {
        let vel = 0.3 + 0.7 * (pos32 as f32 / 256.0);
        roll.push(Note::new(
            snare,
            Time::new(pos32, 8),
            Time::new(1, 8),
            60,
            vel,
        ));
    };
    for pos in (0..128).step_by(8) {
        hit(pos);
    }
    for pos in (128..192).step_by(4) {
        hit(pos);
    }
    for pos in (192..224).step_by(2) {
        hit(pos);
    }
    for pos in 224..256 {
        hit(pos);
    }
    let snare_roll = add(pattern("Snare Roll", 1, 32, roll));

    // A one-bar tom fill into the next section.
    let mut notes = steps(kick, &[(0, 0.9), (4, 0.8)], 60);
    notes.extend(steps(snare, &[(6, 0.5), (7, 0.6)], 60));
    for (i, key) in [67u8, 67, 64, 64, 60, 60, 55, 52].iter().enumerate() {
        notes.push(n16(tom, 8 + i as i64, 1, *key, 0.7 + i as f32 * 0.035));
    }
    let fill = add(pattern("Fill", 1, 4, notes));
    let crash_hit = add(pattern("Crash", 2, 4, steps(crash, &[(0, 0.9)], 60)));

    // Basses.
    let mut notes = Vec::new();
    for (bar, &r) in ROOTS.iter().enumerate() {
        let b = bar as i64 * 16;
        for (s, l, k, v) in [
            (0, 3, r, 0.95),
            (4, 2, r, 0.7),
            (6, 2, r + 12, 0.8),
            (10, 2, r, 0.75),
            (12, 2, r + 7, 0.7),
            (14, 2, r + 12, 0.85),
        ] {
            notes.push(n16(groove_bass, b + s, l, k, v));
        }
    }
    let groove_bass_pat = add(pattern("Groove Bass", 6, 16, notes));

    let mut notes = Vec::new();
    for (bar, &r) in ROOTS.iter().enumerate() {
        let b = bar as i64 * 16;
        for s in [1, 2, 3, 5, 6, 7, 9, 10, 11, 13, 14, 15] {
            let key = if s == 7 || s == 15 { r + 12 } else { r };
            let vel = if s % 4 == 2 { 0.9 } else { 0.7 };
            notes.push(n16(bass, b + s, 1, key, vel));
        }
    }
    let rolling_bass = add(pattern("Rolling Bass", 6, 16, notes));

    let sub_notes = ROOTS
        .iter()
        .enumerate()
        .map(|(bar, &r)| n16(sub, bar as i64 * 16, 15, r, 0.9))
        .collect();
    let sub_pat = add(pattern("Sub", 6, 16, sub_notes));

    // Electric piano: seventh chords with a push on the and of three.
    let ep: [&[u8]; 4] = [
        &[57, 60, 62, 65],
        &[57, 58, 62, 65],
        &[57, 60, 64, 65],
        &[55, 60, 62, 64],
    ];
    let keys_pat = add(pattern(
        "Keys",
        4,
        16,
        chord_hits(keys, &ep, &[(0, 10, 0.72), (10, 6, 0.6)], true, 0),
    ));

    // Grand piano: broken chords for the breakdown.
    let arps: [[u8; 8]; 4] = [
        [50, 57, 62, 65, 69, 65, 62, 57],
        [46, 53, 58, 62, 65, 62, 58, 53],
        [41, 48, 53, 57, 60, 57, 53, 48],
        [48, 55, 60, 64, 67, 64, 60, 55],
    ];
    let mut notes = Vec::new();
    for (bar, arp) in arps.iter().enumerate() {
        for (i, &k) in arp.iter().enumerate() {
            let vel = if i == 0 {
                0.7
            } else {
                0.5 + (i % 3) as f32 * 0.05
            };
            notes.push(n16(piano, bar as i64 * 16 + i as i64 * 2, 6, k, vel));
        }
    }
    let piano_pat = add(pattern("Piano Arp", 4, 16, notes));

    // Strings: sustained chords.
    let pad: [&[u8]; 4] = [
        &[57, 62, 65, 69],
        &[58, 62, 65, 70],
        &[57, 60, 65, 69],
        &[55, 60, 64, 67],
    ];
    let strings_pat = add(pattern(
        "Strings",
        5,
        16,
        chord_hits(strings, &pad, &[(0, 16, 0.6)], false, 0),
    ));

    let pizz_lines: [[u8; 8]; 4] = [
        [62, 65, 69, 74, 77, 74, 69, 65],
        [62, 65, 70, 74, 77, 74, 70, 65],
        [60, 65, 69, 72, 77, 72, 69, 65],
        [60, 64, 67, 72, 76, 72, 67, 64],
    ];
    let mut notes = Vec::new();
    for (bar, line) in pizz_lines.iter().enumerate() {
        for (i, &k) in line.iter().enumerate() {
            let vel = if i % 2 == 0 { 0.75 } else { 0.55 };
            notes.push(n16(pizz, bar as i64 * 16 + i as i64 * 2, 2, k, vel));
        }
    }
    let pizz_pat = add(pattern("Pizzicato", 5, 16, notes));

    let violin_pat = add(pattern(
        "Violin Solo",
        5,
        32,
        melody(
            violin,
            &[
                (0, 12, 74),
                (12, 4, 76),
                (16, 8, 77),
                (24, 4, 76),
                (28, 4, 74),
                (32, 12, 72),
                (44, 4, 74),
                (48, 16, 76),
                (64, 8, 81),
                (72, 4, 79),
                (76, 4, 77),
                (80, 12, 77),
                (92, 4, 79),
                (96, 8, 76),
                (104, 8, 72),
                (112, 16, 74),
            ],
            0.72,
        ),
    ));

    // Sax: the verse melody, brought back over the second drop.
    let sax_pat = add(pattern(
        "Sax Melody",
        8,
        32,
        melody(
            sax,
            &[
                (4, 2, 69),
                (6, 2, 72),
                (8, 6, 74),
                (14, 2, 72),
                (16, 4, 70),
                (20, 2, 69),
                (22, 2, 67),
                (24, 8, 65),
                (34, 2, 65),
                (36, 2, 67),
                (38, 2, 69),
                (40, 4, 72),
                (44, 4, 69),
                (48, 2, 67),
                (50, 2, 64),
                (52, 12, 67),
                (68, 2, 69),
                (70, 2, 72),
                (72, 4, 74),
                (76, 2, 77),
                (78, 2, 76),
                (80, 6, 74),
                (86, 2, 72),
                (88, 8, 70),
                (98, 2, 72),
                (100, 2, 74),
                (102, 2, 72),
                (104, 4, 69),
                (108, 4, 65),
                (112, 4, 67),
                (116, 2, 64),
                (118, 10, 62),
            ],
            0.78,
        ),
    ));

    // Guitars.
    let triads: [&[u8]; 4] = [&[62, 65, 69], &[62, 65, 70], &[60, 65, 69], &[60, 64, 67]];
    let funk_pat = add(pattern(
        "Funk Guitar",
        9,
        16,
        chord_hits(
            funk_gtr,
            &triads,
            &[
                (2, 1, 0.85),
                (6, 1, 0.8),
                (7, 1, 0.45),
                (10, 1, 0.85),
                (13, 1, 0.45),
                (14, 1, 0.8),
            ],
            true,
            0,
        ),
    ));
    let power: [&[u8]; 4] = [&[50, 57, 62], &[46, 53, 58], &[41, 48, 53], &[48, 55, 60]];
    let chug: Vec<(i64, i64, f32)> = (0..8)
        .map(|i| {
            let accent = matches!(i, 0 | 3 | 6);
            (i * 2, 2, if accent { 0.95 } else { 0.65 })
        })
        .collect();
    let power_pat = add(pattern(
        "Power Chords",
        9,
        16,
        chord_hits(rhythm_gtr, &power, &chug, true, 0),
    ));

    // The hook: synth lead, doubled an octave down by the lead guitar in
    // the second drop.
    let mut hook: Vec<(i64, i64, u8)> = Vec::new();
    let bars: [[(i64, i64, u8); 6]; 3] = [
        [
            (0, 3, 74),
            (3, 3, 77),
            (6, 2, 81),
            (8, 2, 79),
            (10, 3, 77),
            (13, 3, 74),
        ],
        [
            (0, 3, 74),
            (3, 3, 77),
            (6, 2, 82),
            (8, 2, 81),
            (10, 3, 77),
            (13, 3, 74),
        ],
        [
            (0, 3, 72),
            (3, 3, 77),
            (6, 2, 81),
            (8, 2, 84),
            (10, 3, 81),
            (13, 3, 77),
        ],
    ];
    for half in [0i64, 64] {
        for (bar, line) in bars.iter().enumerate() {
            hook.extend(
                line.iter()
                    .map(|&(s, l, k)| (half + bar as i64 * 16 + s, l, k)),
            );
        }
    }
    hook.extend([
        (48, 3, 76),
        (51, 3, 79),
        (54, 2, 84),
        (56, 4, 81),
        (60, 4, 79),
    ]);
    hook.extend([
        (112, 2, 79),
        (114, 2, 81),
        (116, 2, 84),
        (118, 2, 86),
        (120, 8, 88),
    ]);
    hook.sort_by_key(|n| n.0);
    let hook_pat = add(pattern("Hook", 10, 32, melody(lead, &hook, 0.8)));
    let low_hook: Vec<(i64, i64, u8)> = hook.iter().map(|&(s, l, k)| (s, l, k - 12)).collect();
    let gtr_hook_pat = add(pattern(
        "Guitar Hook",
        9,
        32,
        melody(lead_gtr, &low_hook, 0.85),
    ));

    let stab_chords: [&[u8]; 4] = [
        &[69, 74, 77, 81],
        &[70, 74, 77, 82],
        &[69, 72, 77, 81],
        &[67, 72, 76, 79],
    ];
    let stabs_pat = add(pattern(
        "Stabs",
        11,
        16,
        chord_hits(
            stabs,
            &stab_chords,
            &[(2, 2, 0.8), (6, 2, 0.75), (10, 2, 0.8), (14, 2, 0.75)],
            false,
            0,
        ),
    ));
    let riser_pat = add(pattern("Riser", 11, 32, vec![n16(riser, 0, 128, 72, 0.85)]));

    p.patterns = pats.into_iter().map(Arc::new).collect();

    // Playlist.
    const SONG: i64 = 80;
    let lanes = Lanes::new(
        &mut p,
        &[
            "Drums",
            "Fills",
            "Roll",
            "Groove Bass",
            "Sub",
            "Bass",
            "Keys",
            "Piano",
            "Strings",
            "Violin",
            "Pizzicato",
            "Sax",
            "Funk Guitar",
            "Rhythm Guitar",
            "Lead",
            "Lead Guitar",
            "Stabs",
            "Riser",
            "Keys Filter",
            "Strings Volume",
            "Shaker Pan",
            "Riser Filter",
            "Snare Pitch",
            "Drum Filter",
            "Lead Filter",
            "Sax Echo",
            "Master Volume",
            "Master Glitch",
            "Strings Gate",
        ],
    );
    let placements: &[(usize, Id, i64, i64)] = &[
        // Drums.
        (0, groove, 8, 8),
        (0, kick_only, 16, 7),
        (0, drop_beat, 24, 16),
        (0, half_time, 44, 4),
        (0, kick_only, 48, 7),
        (0, drop_beat, 56, 15),
        (0, groove, 72, 4),
        (1, crash_hit, 8, 1),
        (1, fill, 23, 1),
        (1, crash_hit, 24, 1),
        (1, crash_hit, 32, 1),
        (1, fill, 55, 1),
        (1, crash_hit, 56, 1),
        (1, crash_hit, 64, 1),
        (1, fill, 71, 1),
        (1, crash_hit, 72, 1),
        (2, snare_roll, 16, 8),
        (2, snare_roll, 48, 8),
        // Bass.
        (3, groove_bass_pat, 8, 12),
        (3, groove_bass_pat, 72, 8),
        (4, sub_pat, 24, 16),
        (4, sub_pat, 56, 16),
        (5, rolling_bass, 24, 16),
        (5, rolling_bass, 56, 16),
        // Keys.
        (6, keys_pat, 0, 24),
        (6, keys_pat, 48, 8),
        (6, keys_pat, 72, 8),
        (7, piano_pat, 40, 12),
        // Strings and sax.
        (8, strings_pat, 0, 8),
        (8, strings_pat, 16, 24),
        (8, strings_pat, 44, 4),
        (8, strings_pat, 56, 24),
        (9, violin_pat, 40, 8),
        (9, violin_pat, 72, 8),
        (10, pizz_pat, 16, 8),
        (10, pizz_pat, 48, 8),
        (11, sax_pat, 8, 8),
        (11, sax_pat, 64, 8),
        // Guitars.
        (12, funk_pat, 8, 16),
        (13, power_pat, 32, 8),
        (13, power_pat, 56, 16),
        // Synths.
        (14, hook_pat, 24, 16),
        (14, hook_pat, 56, 16),
        (15, gtr_hook_pat, 56, 16),
        (16, stabs_pat, 24, 16),
        (16, stabs_pat, 56, 16),
        (17, riser_pat, 16, 8),
        (17, riser_pat, 48, 8),
    ];
    for &(lane, pat, bar, bars) in placements {
        lanes.put(&mut p, lane, pat, bar, bars);
    }

    // Automation. Positions are in beats; one bar is four.
    let b = |bar: f64| bar * BAR as f64;
    let unity = db_to_fader(0.0);

    // The electric piano opens up from a muffled low pass over the intro
    // and closes again at the end.
    let lp = |hz: f32| norm("peel.fx.filter", "cutoff", hz);
    lanes.automate(
        &mut p,
        18,
        SONG,
        AutomationTarget::Param {
            device: keys_filter,
            param: "cutoff".into(),
        },
        &[
            (0.0, lp(280.0), 0.0),
            (b(8.0), lp(20000.0), 0.0),
            (b(72.0), lp(20000.0), 0.0),
            (b(80.0), lp(500.0), 0.0),
        ],
    );

    // Strings fade in, dip for the breakdown and swell back.
    let strings_track = track_of(&p, strings);
    lanes.automate(
        &mut p,
        19,
        SONG,
        AutomationTarget::TrackGain {
            track: strings_track,
        },
        &[
            (0.0, 0.0, -0.4),
            (b(7.0), unity, 0.0),
            (b(44.0), db_to_fader(-14.0), 0.3),
            (b(48.0), unity, 0.0),
            (b(76.0), unity, -0.3),
            (b(80.0), db_to_fader(-30.0), 0.0),
        ],
    );

    // The shaker swings left and right through the verse.
    let shaker_track = track_of(&p, shaker);
    let mut pan = vec![(0.0, 0.5, 0.0), (b(8.0), 0.5, 0.0)];
    for half in 0..16 {
        let v = if half % 2 == 0 { 0.2 } else { 0.8 };
        pan.push((b(8.0) + 2.0 * half as f64 + 2.0, v, 0.0));
    }
    pan.push((b(16.0), 0.5, 0.0));
    lanes.automate(
        &mut p,
        20,
        SONG,
        AutomationTarget::TrackPan {
            track: shaker_track,
        },
        &pan,
    );

    // The noise riser opens over each build.
    let riser_dev = device_of(&p, riser);
    let rc = |hz: f32| norm("peel.synth.poly", "filter.cutoff", hz);
    lanes.automate(
        &mut p,
        21,
        SONG,
        AutomationTarget::Param {
            device: riser_dev,
            param: "filter.cutoff".into(),
        },
        &[
            (b(16.0), rc(300.0), 0.5),
            (b(24.0), rc(14000.0), 0.0),
            (b(24.0) + 0.25, rc(300.0), 0.0),
            (b(48.0), rc(300.0), 0.5),
            (b(56.0), rc(14000.0), 0.0),
            (b(56.0) + 0.25, rc(300.0), 0.0),
        ],
    );

    // The snare roll rises in pitch as it speeds up.
    let snare_dev = device_of(&p, snare);
    let sp = |hz: f32| norm("peel.drum.snare", "body.pitch", hz);
    lanes.automate(
        &mut p,
        22,
        SONG,
        AutomationTarget::Param {
            device: snare_dev,
            param: "body.pitch".into(),
        },
        &[
            (b(16.0), sp(190.0), 0.0),
            (b(16.0), sp(150.0), 0.4),
            (b(24.0), sp(340.0), 0.0),
            (b(24.0) + 0.25, sp(190.0), 0.0),
            (b(48.0), sp(150.0), 0.4),
            (b(56.0), sp(340.0), 0.0),
            (b(56.0) + 0.25, sp(190.0), 0.0),
        ],
    );

    // A high pass sweeps the drum bus thin before each drop.
    let hp = |hz: f32| norm("peel.fx.filter", "cutoff", hz);
    lanes.automate(
        &mut p,
        23,
        SONG,
        AutomationTarget::Param {
            device: drum_filter,
            param: "cutoff".into(),
        },
        &[
            (b(20.0), hp(20.0), 0.5),
            (b(24.0), hp(600.0), 0.0),
            (b(24.0) + 0.25, hp(20.0), 0.0),
            (b(52.0), hp(20.0), 0.5),
            (b(56.0), hp(900.0), 0.0),
            (b(56.0) + 0.25, hp(20.0), 0.0),
        ],
    );

    // The lead starts dark at the first drop and opens over four bars.
    let lead_dev = device_of(&p, lead);
    lanes.automate(
        &mut p,
        24,
        SONG,
        AutomationTarget::Param {
            device: lead_dev,
            param: "filter.cutoff".into(),
        },
        &[
            (b(24.0), rc(700.0), -0.3),
            (b(28.0), rc(5200.0), 0.0),
            (b(56.0), rc(5200.0), 0.0),
        ],
    );

    // Echo throws on the last note of each sax phrase.
    let throw = norm("peel.fx.delay", "mix", 0.45);
    let mut echo_pts = vec![(0.0, 0.0, 0.0)];
    for start in [b(8.0), b(64.0)] {
        for phrase_end in [13.0, 29.5] {
            let at = start + phrase_end;
            echo_pts.push((at - 0.25, 0.0, 0.0));
            echo_pts.push((at, throw, 0.0));
            echo_pts.push((at + 2.5, throw, 0.0));
            echo_pts.push((at + 3.0, 0.0, 0.0));
        }
    }
    lanes.automate(
        &mut p,
        25,
        SONG,
        AutomationTarget::Param {
            device: sax_delay,
            param: "mix".into(),
        },
        &echo_pts,
    );

    // The whole mix fades out over the last four bars.
    lanes.automate(
        &mut p,
        26,
        SONG,
        AutomationTarget::TrackGain { track: master },
        &[
            (0.0, unity, 0.0),
            (b(76.0), unity, 0.2),
            (b(80.0), 0.0, 0.0),
        ],
    );

    // Pattern switches jump: two points at the same position.
    let tp = |i: f32| norm("peel.fx.glitch", "time.pattern", i);
    let (off, build, tape) = (tp(0.0), tp(6.0), tp(9.0));
    lanes.automate(
        &mut p,
        27,
        SONG,
        AutomationTarget::Param {
            device: master_glitch_id,
            param: "time.pattern".into(),
        },
        &[
            (0.0, off, 0.0),
            (b(39.0), off, 0.0),
            (b(39.0), tape, 0.0),
            (b(40.0), tape, 0.0),
            (b(40.0), off, 0.0),
            (b(55.0), off, 0.0),
            (b(55.0), build, 0.0),
            (b(56.0), build, 0.0),
            (b(56.0), off, 0.0),
        ],
    );
    let gp = |i: f32| norm("peel.fx.glitch", "gate.pattern", i);
    lanes.automate(
        &mut p,
        28,
        SONG,
        AutomationTarget::Param {
            device: strings_gate,
            param: "gate.pattern".into(),
        },
        &[
            (0.0, gp(0.0), 0.0),
            (b(56.0), gp(0.0), 0.0),
            (b(56.0), gp(1.0), 0.0),
            (b(72.0), gp(1.0), 0.0),
            (b(72.0), gp(0.0), 0.0),
        ],
    );

    let arr = p.arrangement_mut();
    for (bar, name) in [
        (0, "Intro"),
        (8, "Verse"),
        (16, "Build"),
        (24, "Drop"),
        (40, "Breakdown"),
        (48, "Build"),
        (56, "Drop 2"),
        (72, "Outro"),
    ] {
        arr.markers.push(Marker {
            id: Id::new(),
            position: Time::beats(bar * BAR),
            name: name.into(),
        });
    }
    p
}
