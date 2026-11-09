//! "Afterglow": the project Peel Studio opens with. An 80-bar progressive
//! house song in A minor (Am, F, C, G): a Rhodes intro, a sax verse, a
//! piano-house build, a supersaw drop with a pluck counter-line, a slower
//! piano and strings breakdown, a second drop that lifts a whole tone
//! halfway through, and an outro. It uses tempo ramps, sidechain ducking,
//! reverb and delay returns, glitch effects and automation of volume, pan,
//! filters, pitch and effect sends.

use std::sync::Arc;

use peel_document::{
    AutomationPoint, AutomationTarget, Clip, ClipContent, DeviceInstance, Id, Marker, MixerSend,
    MixerTrack, Note, PALETTE, Project, TempoSegment, Time, Track, db_to_fader,
};

use super::{add_channel, device, pattern, set_gains, steps};

/// One bar of 4/4, in quarter notes.
const BAR: i64 = 4;

/// Bass roots for Am, F, C, G (A1, F1, C2, G1).
const ROOTS: [u8; 4] = [33, 29, 36, 31];

/// The final drop and the outro move up a whole tone.
const LIFT: i8 = 2;

/// A note from sixteenth-note positions.
fn n16(ch: Id, start: i64, len: i64, key: u8, vel: f32) -> Note {
    Note::new(ch, Time::new(start, 4), Time::new(len, 4), key, vel)
}

/// A melody as `(start, length, key)` in sixteenths, with a gentle
/// velocity shape so it does not sound mechanical.
fn melody(ch: Id, line: &[(i64, i64, u8)], vel: f32, tr: i8) -> Vec<Note> {
    line.iter()
        .enumerate()
        .map(|(i, &(s, l, k))| {
            let accent = if s % 4 == 0 { 0.08 } else { 0.0 };
            let wobble = [0.0, -0.05, 0.03, -0.02][i % 4];
            n16(ch, s, l, up(k, tr), (vel + accent + wobble).clamp(0.1, 1.0))
        })
        .collect()
}

fn up(key: u8, tr: i8) -> u8 {
    (key as i16 + tr as i16).clamp(0, 127) as u8
}

/// A chord per bar, each hit on the given sixteenths, strummed slightly
/// when `strum` is set.
fn chord_hits(
    ch: Id,
    chords: &[&[u8]],
    hits: &[(i64, i64, f32)],
    strum: bool,
    tr: i8,
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
                v.push(Note::new(ch, start, Time::new(len, 4), up(k, tr), vel));
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

// Voicings, one chord per bar: Am, F, C, G.
const EP: [&[u8]; 4] = [
    &[57, 60, 64, 67],
    &[57, 60, 64, 65],
    &[55, 60, 64, 67],
    &[55, 59, 62, 67],
];
const PAD: [&[u8]; 4] = [
    &[57, 64, 69, 72],
    &[57, 65, 69, 72],
    &[55, 64, 67, 72],
    &[55, 62, 67, 71],
];
const SAW: [&[u8]; 4] = [
    &[64, 69, 72, 76],
    &[65, 69, 72, 77],
    &[64, 67, 72, 76],
    &[62, 67, 71, 74],
];
const TRIADS: [&[u8]; 4] = [&[64, 69, 72], &[65, 69, 72], &[64, 67, 72], &[62, 67, 71]];
/// Pluck arpeggio tones per chord, low to high.
const ARP: [[u8; 4]; 4] = [
    [69, 72, 76, 81],
    [69, 72, 77, 81],
    [67, 72, 76, 79],
    [67, 71, 74, 79],
];

/// The hook: an eight-bar syncopated riff over the progression twice.
fn hook_line() -> Vec<(i64, i64, u8)> {
    let first: [&[(i64, i64, u8)]; 4] = [
        &[
            (0, 2, 69),
            (3, 2, 72),
            (6, 2, 76),
            (8, 1, 74),
            (10, 2, 72),
            (12, 2, 76),
            (14, 2, 79),
        ],
        &[
            (0, 3, 81),
            (3, 3, 79),
            (6, 2, 76),
            (8, 2, 72),
            (10, 2, 74),
            (12, 4, 76),
        ],
        &[
            (0, 2, 76),
            (3, 2, 76),
            (6, 2, 79),
            (8, 1, 76),
            (10, 2, 74),
            (12, 2, 72),
            (14, 2, 74),
        ],
        &[
            (0, 3, 71),
            (3, 3, 74),
            (6, 4, 79),
            (10, 2, 74),
            (12, 2, 71),
            (14, 2, 74),
        ],
    ];
    let mut line = Vec::new();
    for half in 0..2i64 {
        for (bar, notes) in first.iter().enumerate() {
            let last = half == 1 && bar == 3;
            if last {
                // The second time round the riff climbs instead of falling.
                for &(s, l, k) in &[(0, 3, 71), (3, 3, 74), (6, 2, 79), (8, 2, 81), (10, 6, 83)] {
                    line.push((half * 64 + 48 + s, l, k));
                }
            } else {
                for &(s, l, k) in notes.iter() {
                    line.push((half * 64 + bar as i64 * 16 + s, l, k));
                }
            }
        }
    }
    line
}

/// The sax topline for the verse, answered again in the second drop.
const SAX: [(i64, i64, u8); 32] = [
    (4, 2, 72),
    (6, 2, 74),
    (8, 6, 76),
    (14, 2, 74),
    (16, 4, 72),
    (20, 2, 69),
    (22, 2, 67),
    (24, 8, 69),
    (34, 2, 67),
    (36, 2, 69),
    (38, 2, 72),
    (40, 4, 76),
    (44, 4, 72),
    (48, 2, 71),
    (50, 2, 67),
    (52, 12, 74),
    (68, 2, 72),
    (70, 2, 74),
    (72, 4, 76),
    (76, 2, 79),
    (78, 2, 77),
    (80, 6, 76),
    (86, 2, 74),
    (88, 8, 72),
    (98, 2, 72),
    (100, 2, 74),
    (102, 2, 72),
    (104, 4, 69),
    (108, 4, 67),
    (112, 4, 71),
    (116, 2, 67),
    (118, 10, 69),
];

/// A long-note violin counter-melody for the breakdown and the outro.
const VIOLIN: [(i64, i64, u8); 14] = [
    (0, 12, 76),
    (12, 4, 74),
    (16, 8, 72),
    (24, 8, 69),
    (32, 12, 76),
    (44, 4, 79),
    (48, 16, 74),
    (64, 12, 81),
    (76, 4, 79),
    (80, 8, 77),
    (88, 8, 76),
    (96, 8, 76),
    (104, 8, 79),
    (112, 16, 83),
];

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
        tweaked(
            "peel.drum.kick",
            Some("Big Room"),
            &[("pitch.freq", 55.0), ("body.decay", 0.3)],
        ),
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
        device("peel.drum.hat", Some("Shaker")),
        60,
    );
    let perc = add_channel(
        &mut p,
        "Perc",
        3,
        device("peel.drum.tom", Some("Tribal")),
        60,
    );
    let tom = add_channel(&mut p, "Tom", 0, device("peel.drum.tom", Some("Floor")), 60);
    let crash = add_channel(
        &mut p,
        "Crash",
        2,
        device("peel.drum.hat", Some("Crash")),
        60,
    );
    // Low end.
    let sub = add_channel(&mut p, "Sub", 6, device("peel.synth.bass", Some("Sub")), 33);
    let bass = add_channel(
        &mut p,
        "Bass",
        6,
        device("peel.synth.bass", Some("Rolling")),
        33,
    );
    let groove_bass = add_channel(
        &mut p,
        "Groove Bass",
        7,
        device("peel.synth.bass", Some("Deep House")),
        33,
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
        device("peel.keys.grand", Some("Pop")),
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
        76,
    );
    let pizz = add_channel(
        &mut p,
        "Pizzicato",
        5,
        device("peel.strings.violin", Some("Pizzicato")),
        72,
    );
    let sax = add_channel(
        &mut p,
        "Sax",
        8,
        tweaked(
            "peel.wind.sax",
            Some("House Sax"),
            &[("vibrato.depth", 12.0)],
        ),
        72,
    );
    // Guitars.
    let funk_gtr = add_channel(
        &mut p,
        "Funk Guitar",
        9,
        device("peel.guitar.electric", Some("Muted Pluck")),
        64,
    );
    let lead_gtr = add_channel(
        &mut p,
        "Lead Guitar",
        9,
        device("peel.guitar.electric", Some("Lead")),
        64,
    );
    // Synths.
    let lead = add_channel(
        &mut p,
        "Lead",
        10,
        device("peel.synth.poly", Some("Supersaw Lead")),
        76,
    );
    let pluck = add_channel(
        &mut p,
        "Pluck",
        10,
        device("peel.synth.poly", Some("Pluck")),
        76,
    );
    let stabs = add_channel(
        &mut p,
        "Stabs",
        11,
        device("peel.synth.poly", Some("Super Chord")),
        69,
    );
    let pad = add_channel(
        &mut p,
        "Pad",
        11,
        device("peel.synth.poly", Some("Future Pad")),
        64,
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
            (kick, -4.0),
            (clap, -3.0),
            (snare, -11.0),
            (hat, -6.0),
            (open_hat, -11.0),
            (shaker, -9.0),
            (perc, -14.0),
            (tom, -16.0),
            (crash, -12.0),
            (sub, -17.0),
            (bass, -12.0),
            (groove_bass, -14.0),
            (keys, -12.0),
            (piano, -9.0),
            (strings, -9.0),
            (violin, -9.0),
            (pizz, -9.0),
            (sax, -12.0),
            (funk_gtr, -9.0),
            (lead_gtr, -13.0),
            (lead, 1.0),
            (pluck, -5.0),
            (stabs, -7.0),
            (pad, -10.0),
            (riser, -3.0),
        ],
    );

    // Mixer: a drum bus, a reverb and a delay return, sidechain ducking
    // under the kick, and a mastering chain.
    let master = p.mixer.tracks[0].id;
    insert(&mut p, master, device("peel.fx.eq", Some("Clean Low End")));
    insert(&mut p, master, device("peel.fx.multiband", Some("Master")));
    insert(
        &mut p,
        master,
        device("peel.fx.compressor", Some("Gentle Glue")),
    );
    // A soft clipper shaves the kick peaks so the limiter does not pump.
    insert(
        &mut p,
        master,
        tweaked(
            "peel.fx.drive",
            None,
            &[
                ("type", 0.0),
                ("drive", 4.0),
                ("output", -4.0),
                ("oversampling", 2.0),
            ],
        ),
    );
    insert(
        &mut p,
        master,
        tweaked("peel.fx.limiter", Some("Master Safety"), &[("input", 7.0)]),
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
    // Shave the drum transients so the mix can sit louder.
    insert(
        &mut p,
        drum_bus,
        tweaked("peel.fx.limiter", Some("Drum Bus Catch"), &[("input", 5.0)]),
    );
    for ch in [kick, clap, snare, hat, open_hat, shaker, perc, tom, crash] {
        let t = track_of(&p, ch);
        mixer(&mut p, t).output = Some(drum_bus);
    }

    let hall = add_bus(&mut p, "Hall", 5);
    insert(
        &mut p,
        hall,
        tweaked("peel.fx.reverb", Some("Big Hall"), &[("mix", 1.0)]),
    );
    let echo = add_bus(&mut p, "Echo", 10);
    insert(
        &mut p,
        echo,
        tweaked("peel.fx.delay", Some("Dotted Ping-Pong"), &[("mix", 1.0)]),
    );
    insert(&mut p, echo, device("peel.fx.eq", Some("Clean Low End")));
    let sends: [(Id, Id, f32); 14] = [
        (clap, hall, -16.0),
        (keys, hall, -12.0),
        (piano, hall, -8.0),
        (strings, hall, -9.0),
        (violin, hall, -6.0),
        (pizz, hall, -10.0),
        (sax, hall, -9.0),
        (lead, hall, -12.0),
        (pluck, hall, -12.0),
        (pad, hall, -10.0),
        (lead, echo, -14.0),
        (pluck, echo, -13.0),
        (lead_gtr, echo, -12.0),
        (violin, echo, -16.0),
    ];
    for (ch, target, gain_db) in sends {
        let t = track_of(&p, ch);
        mixer(&mut p, t).sends.push(MixerSend { target, gain_db });
    }

    let kick_track = track_of(&p, kick);
    for ch in [sub, bass, groove_bass, stabs, strings, pad] {
        let t = track_of(&p, ch);
        let mut duck = device("peel.fx.compressor", Some("EDM Pump"));
        duck.sidechain = Some(kick_track);
        mixer(&mut p, t).inserts.insert(0, duck);
    }

    // Glitch: a tape stop into the breakdown and stutters into the second
    // drop and the key change, plus a trance gate on the strings.
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

    // Patterns.
    let mut pats: Vec<peel_document::Pattern> = Vec::new();
    let mut add = |pat: peel_document::Pattern| {
        let id = pat.id;
        pats.push(pat);
        id
    };

    // Drums. Hats and percussion run over two bars so the groove breathes.
    let two_bars = |vels: [f32; 4], skip: &[u32]| -> Vec<(u32, f32)> {
        (0..32)
            .filter(|i| !skip.contains(i))
            .map(|i| (i, vels[i as usize % 4]))
            .collect()
    };
    let at =
        |list: &[u32], vel: f32| -> Vec<(u32, f32)> { list.iter().map(|&s| (s, vel)).collect() };

    // Intro percussion, filtered in by the keys.
    let mut notes = steps(shaker, &two_bars([0.3, 0.45, 0.35, 0.5], &[]), 60);
    for (s, k, v) in [
        (6, 64u8, 0.5),
        (10, 60, 0.45),
        (22, 64, 0.5),
        (27, 67, 0.4),
        (30, 60, 0.45),
    ] {
        notes.push(n16(perc, s, 1, k, v));
    }
    let intro_perc = add(pattern("Intro Perc", 3, 8, notes));

    // Verse groove, held back so the drop hits harder.
    let mut notes = steps(clap, &at(&[4, 12, 20, 28], 0.6), 60);
    notes.extend(steps(
        hat,
        &[
            (2, 0.75),
            (6, 0.75),
            (7, 0.3),
            (10, 0.75),
            (14, 0.75),
            (18, 0.75),
            (22, 0.75),
            (23, 0.3),
            (26, 0.75),
            (29, 0.35),
            (30, 0.8),
            (31, 0.4),
        ],
        60,
    ));
    notes.extend(steps(shaker, &two_bars([0.35, 0.55, 0.4, 0.6], &[]), 60));
    for (s, k, v) in [
        (3, 64u8, 0.55),
        (11, 60, 0.5),
        (14, 67, 0.45),
        (19, 64, 0.55),
        (26, 60, 0.5),
        (27, 60, 0.4),
    ] {
        notes.push(n16(perc, s, 1, k, v));
    }
    let groove_top = add(pattern(
        "Groove Top",
        0,
        8,
        notes
            .iter()
            .map(|n| Note {
                id: Id::new(),
                ..*n
            })
            .collect(),
    ));
    notes.extend(steps(kick, &at(&[0, 4, 8, 12, 16, 20, 24, 28], 0.68), 60));
    let groove = add(pattern("Groove", 0, 8, notes));

    // Build: four on the floor with claps, then eighth-note kicks.
    let mut notes = steps(kick, &at(&[0, 4, 8, 12], 0.95), 60);
    notes.extend(steps(clap, &at(&[4, 12], 0.7), 60));
    notes.extend(steps(hat, &at(&[2, 6, 10, 14], 0.6), 60));
    let build_beat = add(pattern("Build Beat", 0, 4, notes));
    let mut notes = steps(kick, &at(&[0, 2, 4, 6, 8, 10, 12, 14], 0.9), 60);
    notes.extend(steps(clap, &at(&[4, 12], 0.8), 60));
    let build_eighths = add(pattern("Build Eighths", 0, 4, notes));

    // Drop beat: claps layered with snare, sixteenth hats with accents and
    // ghosts, open hats on the off-beats, and percussion answering in the
    // second bar.
    let mut notes = steps(kick, &at(&[0, 4, 8, 12, 16, 20, 24, 28], 1.0), 60);
    notes.extend(steps(clap, &at(&[4, 12, 20, 28], 0.95), 60));
    notes.push(n16(clap, 31, 1, 60, 0.35));
    notes.extend(steps(
        snare,
        &[
            (4, 0.55),
            (12, 0.55),
            (20, 0.55),
            (28, 0.55),
            (15, 0.22),
            (30, 0.25),
        ],
        60,
    ));
    notes.extend(steps(
        hat,
        &two_bars([0.4, 0.25, 0.75, 0.3], &[2, 6, 10, 14, 18, 22, 26, 30]),
        60,
    ));
    notes.extend(steps(
        open_hat,
        &at(&[2, 6, 10, 14, 18, 22, 26, 30], 0.72),
        60,
    ));
    notes.extend(steps(shaker, &two_bars([0.3, 0.45, 0.3, 0.5], &[]), 60));
    for (s, k, v) in [
        (19, 64u8, 0.5),
        (23, 60, 0.45),
        (29, 67, 0.5),
        (30, 64, 0.45),
    ] {
        notes.push(n16(perc, s, 1, k, v));
    }
    let drop_beat = add(pattern("Drop Beat", 0, 8, notes));

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

    // An eight-bar snare roll that speeds up and gets louder, then leaves
    // the last beat empty before the drop.
    let mut roll = Vec::new();
    let mut hit = |pos32: i64| {
        let vel = 0.3 + 0.7 * (pos32 as f32 / 248.0);
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
    for pos in 224..248 {
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
    // A crash with a low tom boom under it.
    let mut notes = steps(crash, &[(0, 0.9)], 60);
    notes.push(n16(tom, 0, 4, 43, 0.8));
    let crash_hit = add(pattern("Crash", 2, 4, notes));

    // Basses. Patterns are built in the home key and lifted a whole tone
    // for the final drop and the outro.
    let groove_bass_notes = |tr: i8| {
        let mut notes = Vec::new();
        for (bar, &r) in ROOTS.iter().enumerate() {
            let b = bar as i64 * 16;
            for (s, l, k, v) in [
                (2, 2, r + 12, 0.85),
                (6, 2, r + 12, 0.75),
                (10, 2, r + 12, 0.8),
                (13, 1, r + 24, 0.6),
                (14, 2, r + 12, 0.75),
            ] {
                notes.push(n16(groove_bass, b + s, l, up(k, tr), v));
            }
        }
        notes
    };
    let rolling_notes = |tr: i8| {
        let mut notes = Vec::new();
        for (bar, &r) in ROOTS.iter().enumerate() {
            let b = bar as i64 * 16;
            for s in [2, 3, 6, 7, 10, 11, 14, 15] {
                let key = if s % 4 == 3 { r + 24 } else { r + 12 };
                let vel = if s % 4 == 2 { 0.9 } else { 0.65 };
                notes.push(n16(bass, b + s, 1, up(key, tr), vel));
            }
        }
        notes
    };
    let sub_notes = |tr: i8| -> Vec<Note> {
        ROOTS
            .iter()
            .enumerate()
            .map(|(bar, &r)| n16(sub, bar as i64 * 16, 15, up(r, tr), 0.9))
            .collect()
    };
    let groove_bass_pat = add(pattern("Groove Bass", 7, 16, groove_bass_notes(0)));
    let groove_bass_up = add(pattern("Groove Bass Up", 7, 16, groove_bass_notes(LIFT)));
    let rolling_bass = add(pattern("Rolling Bass", 6, 16, rolling_notes(0)));
    let rolling_up = add(pattern("Rolling Bass Up", 6, 16, rolling_notes(LIFT)));
    let sub_pat = add(pattern("Sub", 6, 16, sub_notes(0)));
    let sub_up = add(pattern("Sub Up", 6, 16, sub_notes(LIFT)));

    // Electric piano: seventh chords in a dotted, house rhythm.
    let ep_hits = [(0, 5, 0.72), (6, 5, 0.6), (12, 3, 0.66), (14, 2, 0.48)];
    let keys_pat = add(pattern(
        "Keys",
        4,
        16,
        chord_hits(keys, &EP, &ep_hits, true, 0),
    ));
    let keys_up = add(pattern(
        "Keys Up",
        4,
        16,
        chord_hits(keys, &EP, &ep_hits, true, LIFT),
    ));

    // Piano: driving eighths through the build, getting louder.
    let mut notes = Vec::new();
    for bar in 0..8i64 {
        let chord = EP[bar as usize % 4];
        for eighth in 0..8i64 {
            let vel = 0.45 + 0.4 * (bar as f32 / 7.0) + if eighth % 2 == 0 { 0.08 } else { 0.0 };
            for &k in chord {
                notes.push(n16(piano, bar * 16 + eighth * 2, 2, k, vel.min(1.0)));
            }
            // The left hand doubles the root an octave down on the beat.
            if eighth % 2 == 0 {
                notes.push(n16(
                    piano,
                    bar * 16 + eighth * 2,
                    4,
                    ROOTS[bar as usize % 4] + 12,
                    vel,
                ));
            }
        }
    }
    let piano_build = add(pattern("Piano Build", 4, 32, notes));

    // Piano in the breakdown: the hook, unhurried, over open left-hand
    // chords.
    let mut notes = melody(piano, &hook_line(), 0.62, 0);
    for bar in 0..8i64 {
        let r = ROOTS[bar as usize % 4] + 12;
        notes.push(n16(piano, bar * 16, 16, r, 0.55));
        notes.push(n16(piano, bar * 16, 16, r + 7, 0.45));
        notes.push(n16(piano, bar * 16 + 8, 8, r + 12, 0.4));
    }
    let piano_hook = add(pattern("Piano Hook", 4, 32, notes));

    // Pad and strings: sustained chords.
    let pad_pat = add(pattern(
        "Pad",
        11,
        16,
        chord_hits(pad, &PAD, &[(0, 16, 0.6)], false, 0),
    ));
    let strings_pat = add(pattern(
        "Strings",
        5,
        16,
        chord_hits(strings, &PAD, &[(0, 16, 0.6)], false, 0),
    ));
    let strings_up = add(pattern(
        "Strings Up",
        5,
        16,
        chord_hits(strings, &PAD, &[(0, 16, 0.6)], false, LIFT),
    ));

    // Pizzicato climbs through the second build in eighths.
    let mut notes = Vec::new();
    for (bar, tones) in ARP.iter().enumerate() {
        let line = [
            tones[0],
            tones[1],
            tones[2],
            tones[3],
            tones[2] + 12,
            tones[3],
            tones[2],
            tones[1],
        ];
        for (i, &k) in line.iter().enumerate() {
            let vel = if i % 2 == 0 { 0.75 } else { 0.55 };
            notes.push(n16(pizz, bar as i64 * 16 + i as i64 * 2, 2, k - 12, vel));
        }
    }
    let pizz_pat = add(pattern("Pizzicato", 5, 16, notes));

    let violin_pat = add(pattern("Violin", 5, 32, melody(violin, &VIOLIN, 0.72, 0)));
    let violin_up = add(pattern(
        "Violin Up",
        5,
        32,
        melody(violin, &VIOLIN, 0.72, LIFT),
    ));
    let sax_pat = add(pattern("Sax", 8, 32, melody(sax, &SAX, 0.78, 0)));

    // Funk guitar: muted, syncopated chops.
    let funk_pat = add(pattern(
        "Funk Guitar",
        9,
        16,
        chord_hits(
            funk_gtr,
            &TRIADS,
            &[
                (2, 1, 0.85),
                (5, 1, 0.5),
                (7, 1, 0.75),
                (10, 1, 0.85),
                (13, 1, 0.5),
                (14, 1, 0.8),
            ],
            true,
            0,
        ),
    ));

    // The hook on the supersaw lead, an octave down on the lead guitar in
    // the second drop.
    let hook = hook_line();
    let hook_pat = add(pattern("Hook", 10, 32, melody(lead, &hook, 0.8, 0)));
    let hook_up = add(pattern("Hook Up", 10, 32, melody(lead, &hook, 0.82, LIFT)));
    let low_hook: Vec<(i64, i64, u8)> = hook.iter().map(|&(s, l, k)| (s, l, k - 12)).collect();
    let gtr_hook = add(pattern(
        "Guitar Hook",
        9,
        32,
        melody(lead_gtr, &low_hook, 0.85, 0),
    ));
    let gtr_hook_up = add(pattern(
        "Guitar Hook Up",
        9,
        32,
        melody(lead_gtr, &low_hook, 0.85, LIFT),
    ));

    // Pluck: a sixteenth-note arpeggio that answers the hook, accented in a
    // 3-3-2 pattern.
    let pluck_notes = |tr: i8| {
        let order = [0usize, 1, 2, 3, 2, 1, 3, 2, 0, 1, 2, 3, 1, 2, 3, 2];
        let mut notes = Vec::new();
        for (bar, tones) in ARP.iter().enumerate() {
            for (s, &i) in order.iter().enumerate() {
                let accent = matches!(s, 0 | 3 | 6 | 8 | 11 | 14);
                let vel = if accent { 0.82 } else { 0.5 };
                notes.push(n16(
                    pluck,
                    bar as i64 * 16 + s as i64,
                    1,
                    up(tones[i], tr),
                    vel,
                ));
            }
        }
        notes
    };
    let pluck_pat = add(pattern("Pluck Arp", 10, 16, pluck_notes(0)));
    let pluck_up = add(pattern("Pluck Arp Up", 10, 16, pluck_notes(LIFT)));

    // Supersaw chord stabs on the off-beats, pumped by the kick.
    let stab_hits = [(2, 3, 0.8), (6, 3, 0.75), (10, 3, 0.8), (14, 2, 0.75)];
    let stabs_pat = add(pattern(
        "Stabs",
        11,
        16,
        chord_hits(stabs, &SAW, &stab_hits, false, 0),
    ));
    let stabs_up = add(pattern(
        "Stabs Up",
        11,
        16,
        chord_hits(stabs, &SAW, &stab_hits, false, LIFT),
    ));
    let riser_pat = add(pattern("Riser", 11, 32, vec![n16(riser, 0, 124, 72, 0.85)]));

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
            "Pad",
            "Strings",
            "Violin",
            "Pizzicato",
            "Sax",
            "Funk Guitar",
            "Lead",
            "Lead Guitar",
            "Pluck",
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
        (0, intro_perc, 4, 4),
        (0, groove_top, 8, 4),
        (0, groove, 12, 4),
        (0, build_beat, 16, 6),
        (0, build_eighths, 22, 1),
        (0, drop_beat, 24, 7),
        (0, drop_beat, 32, 8),
        (0, half_time, 44, 4),
        (0, build_beat, 48, 6),
        (0, build_eighths, 54, 2),
        (0, drop_beat, 56, 7),
        (0, drop_beat, 64, 8),
        (0, groove, 72, 6),
        (1, crash_hit, 8, 1),
        (1, fill, 15, 1),
        (1, crash_hit, 24, 1),
        (1, fill, 31, 1),
        (1, crash_hit, 32, 1),
        (1, crash_hit, 40, 1),
        (1, crash_hit, 56, 1),
        (1, fill, 63, 1),
        (1, crash_hit, 64, 1),
        (1, crash_hit, 72, 1),
        (2, snare_roll, 16, 8),
        (2, snare_roll, 48, 8),
        // Bass.
        (3, groove_bass_pat, 8, 14),
        (3, groove_bass_up, 72, 8),
        (4, sub_pat, 24, 16),
        (4, sub_pat, 56, 8),
        (4, sub_up, 64, 8),
        (5, rolling_bass, 24, 16),
        (5, rolling_bass, 56, 8),
        (5, rolling_up, 64, 8),
        // Keys.
        (6, keys_pat, 0, 22),
        (6, keys_up, 72, 8),
        (7, piano_build, 16, 7),
        (7, piano_hook, 40, 8),
        (8, pad_pat, 0, 8),
        (8, pad_pat, 40, 16),
        // Strings and sax.
        (9, strings_pat, 16, 7),
        (9, strings_pat, 24, 24),
        (9, strings_pat, 56, 8),
        (9, strings_up, 64, 16),
        (10, violin_pat, 40, 8),
        (10, violin_up, 72, 8),
        (11, pizz_pat, 48, 8),
        (12, sax_pat, 8, 8),
        (12, sax_pat, 56, 8),
        // Guitars.
        (13, funk_pat, 8, 8),
        (13, funk_pat, 56, 8),
        (15, gtr_hook, 56, 8),
        (15, gtr_hook_up, 64, 8),
        // Synths.
        (14, hook_pat, 24, 16),
        (14, hook_pat, 56, 8),
        (14, hook_up, 64, 8),
        (16, pluck_pat, 4, 4),
        (16, pluck_pat, 16, 7),
        (16, pluck_pat, 24, 16),
        (16, pluck_pat, 56, 8),
        (16, pluck_up, 64, 8),
        (17, stabs_pat, 24, 16),
        (17, stabs_pat, 56, 8),
        (17, stabs_up, 64, 8),
        (18, riser_pat, 16, 8),
        (18, riser_pat, 48, 8),
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
        19,
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

    // Strings swell in through the first build, sit back in the drop,
    // come forward for the breakdown and fade at the end.
    let strings_track = track_of(&p, strings);
    lanes.automate(
        &mut p,
        20,
        SONG,
        AutomationTarget::TrackGain {
            track: strings_track,
        },
        &[
            (0.0, 0.0, 0.0),
            (b(16.0), 0.0, -0.4),
            (b(23.0), unity, 0.0),
            (b(24.0), db_to_fader(-4.0), 0.0),
            (b(40.0), unity, 0.0),
            (b(56.0), db_to_fader(-4.0), 0.0),
            (b(76.0), db_to_fader(-4.0), -0.3),
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
        21,
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
        22,
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
        23,
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
        24,
        SONG,
        AutomationTarget::Param {
            device: drum_filter,
            param: "cutoff".into(),
        },
        &[
            (b(19.0), hp(20.0), 0.5),
            (b(23.0), hp(600.0), 0.0),
            (b(24.0), hp(20.0), 0.0),
            (b(52.0), hp(20.0), 0.5),
            (b(56.0), hp(900.0), 0.0),
            (b(56.0) + 0.25, hp(20.0), 0.0),
        ],
    );

    // The lead starts dark at the first drop and opens over four bars.
    let lead_dev = device_of(&p, lead);
    lanes.automate(
        &mut p,
        25,
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
    for start in [b(8.0), b(56.0)] {
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
        26,
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
        27,
        SONG,
        AutomationTarget::TrackGain { track: master },
        &[
            (0.0, unity, 0.0),
            (b(76.0), unity, 0.2),
            (b(80.0), 0.0, 0.0),
        ],
    );

    // Pattern switches jump: two points at the same position. A tape stop
    // into the breakdown, a stutter build into the second drop and a short
    // sixteenth stutter into the key change.
    let tp = |i: f32| norm("peel.fx.glitch", "time.pattern", i);
    let (off, s16, build, tape) = (tp(0.0), tp(3.0), tp(6.0), tp(9.0));
    lanes.automate(
        &mut p,
        28,
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
            (b(63.5), off, 0.0),
            (b(63.5), s16, 0.0),
            (b(64.0), s16, 0.0),
            (b(64.0), off, 0.0),
        ],
    );
    let gp = |i: f32| norm("peel.fx.glitch", "gate.pattern", i);
    lanes.automate(
        &mut p,
        29,
        SONG,
        AutomationTarget::Param {
            device: strings_gate,
            param: "gate.pattern".into(),
        },
        &[
            (0.0, gp(0.0), 0.0),
            (b(64.0), gp(0.0), 0.0),
            (b(64.0), gp(1.0), 0.0),
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
        (64, "Lift"),
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
