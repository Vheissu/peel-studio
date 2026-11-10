//! "Neon Coast": synthwave at 108 BPM in F sharp minor (F#m, D, A, E). A
//! filtered arpeggio opens over a pulse-width pad, the verse brings in an
//! eighth-note bass and a snare with a gated plate, the chorus adds brass
//! stabs, a sixteenth-note bass and a gliding lead, a break strips back to
//! the arpeggio and bells, and the second chorus answers the lead with
//! bells above it.
//!
//! The snare's reverb is the real thing: a plate on a return, shut by a
//! gate that listens to the dry snare.

use peel_document::{AutomationTarget, Id, Note, Project, Time};

use super::kit::{HIT, Song, arp, chords, fader, grid, line, n16, norm, param, roll};

/// Bars in the song.
const BARS: i64 = 72;

/// Pad voicings with the top note held where it can be.
const PAD: [&[u8]; 4] = [
    &[54, 57, 61, 66],
    &[54, 57, 62, 66],
    &[52, 57, 61, 64],
    &[52, 56, 59, 64],
];
/// Brass stabs an octave above the pad's lower three notes.
const BRASS: [&[u8]; 4] = [&[66, 69, 73], &[66, 69, 74], &[64, 69, 73], &[64, 68, 71]];
/// Arpeggio tones.
const ARP: [&[u8]; 4] = [&[66, 69, 73], &[66, 69, 74], &[64, 69, 73], &[64, 68, 71]];
/// Bass roots: F#1, D2, A1, E2.
const ROOTS: [u8; 4] = [30, 38, 33, 40];

/// The chorus lead: two passes over the progression, the second climbing.
const LEAD: [(i64, i64, u8); 29] = [
    (0, 6, 73),
    (6, 2, 76),
    (8, 8, 78),
    (16, 4, 78),
    (20, 4, 76),
    (24, 6, 74),
    (30, 2, 69),
    (32, 6, 73),
    (38, 2, 76),
    (40, 6, 81),
    (46, 2, 80),
    (48, 8, 80),
    (56, 4, 76),
    (60, 4, 71),
    (64, 6, 73),
    (70, 2, 76),
    (72, 4, 78),
    (76, 4, 81),
    (80, 6, 83),
    (86, 2, 81),
    (88, 8, 78),
    (96, 4, 76),
    (100, 4, 81),
    (104, 6, 85),
    (110, 2, 83),
    (112, 6, 83),
    (118, 2, 80),
    (120, 6, 76),
    (126, 2, 73),
];

/// The verse call: a few bell notes that leave the bar open.
const CALL: [(i64, i64, u8); 9] = [
    (8, 4, 73),
    (12, 4, 76),
    (24, 4, 74),
    (28, 4, 69),
    (40, 4, 73),
    (44, 4, 76),
    (56, 3, 71),
    (59, 3, 73),
    (62, 2, 76),
];

/// One bar of bass on a root: `(step, length, semitones up)`.
fn bass(ch: Id, hits: &[(i64, i64, u8)], vel: f32) -> Vec<Note> {
    let mut v = Vec::new();
    for (bar, &root) in ROOTS.iter().enumerate() {
        for &(step, len, up) in hits {
            let accent = if step % 4 == 0 { 0.08 } else { 0.0 };
            v.push(n16(
                ch,
                bar as i64 * 16 + step,
                len,
                root + up,
                vel + accent,
            ));
        }
    }
    v
}

/// A tom run down four drums over the last half bar of a four-bar pattern.
fn tom_fill(tom: Id) -> Vec<Note> {
    [67u8, 67, 64, 64, 60, 60, 57, 55]
        .into_iter()
        .enumerate()
        .map(|(i, key)| n16(tom, 56 + i as i64, 1, key, 0.62 + 0.045 * i as f32))
        .collect()
}

pub fn neon_coast() -> Project {
    let mut s = Song::new(
        "Neon Coast",
        "Synthwave. The snare's plate reverb is gated by the dry snare: see the Gate on Snare Plate.",
        108.0,
        0.0,
        &[
            "Drums",
            "Fills",
            "Bass",
            "Arp",
            "Pad",
            "Brass",
            "Lead",
            "Bells",
            "FX",
            "Arp filter",
            "Hall",
            "Fade",
        ],
    );

    // Drums.
    let kick = s.inst(
        "Kick",
        0,
        "peel.drum.kick",
        "Synthwave Kick",
        &[],
        HIT,
        -7.0,
    );
    let snare = s.inst("Snare", 9, "peel.drum.snare", "Gated", &[], HIT, -8.0);
    let clap = s.inst("Clap", 1, "peel.drum.clap", "Gated", &[], HIT, -12.0);
    let hat = s.inst("Hat", 2, "peel.drum.hat", "909 Closed", &[], HIT, -11.0);
    let open = s.inst("Open Hat", 2, "peel.drum.hat", "Open", &[], HIT, -18.0);
    let tom = s.inst("Toms", 3, "peel.drum.tom", "909 Tom", &[], HIT, -10.0);
    let crash = s.inst("Crash", 2, "peel.drum.hat", "Crash", &[], HIT, -16.0);
    // Synths.
    let low = s.inst("Bass", 6, "peel.synth.bass", "Octave Drive", &[], 30, -3.0);
    let arp_ch = s.inst("Arp", 7, "peel.synth.poly", "Analog Arp", &[], 66, -8.0);
    let pad = s.inst("Pad", 5, "peel.synth.poly", "PWM Pad", &[], 54, -12.0);
    let brass = s.inst("Brass", 4, "peel.synth.poly", "Synth Brass", &[], 66, -7.0);
    let lead = s.inst("Lead", 10, "peel.synth.poly", "Night Lead", &[], 73, -6.0);
    let bell = s.inst("Bells", 8, "peel.synth.poly", "Glass Bell", &[], 73, -12.0);
    let riser = s.inst(
        "Riser",
        11,
        "peel.synth.poly",
        "Noise Riser",
        &[],
        72,
        -13.0,
    );

    // Mixer.
    let drum_bus = s.bus("Drum Bus", 0, -8.0);
    s.route(&[kick, snare, clap, hat, open, tom, crash], drum_bus);
    s.fx(drum_bus, "peel.fx.compressor", Some("Punchy Drums"), &[]);
    s.fx(
        drum_bus,
        "peel.fx.limiter",
        Some("Drum Bus Catch"),
        &[("input", 3.0)],
    );
    s.pan(hat, 0.15);
    s.pan(open, -0.2);

    // The gated snare: a bright plate on a return, shut by a gate that
    // listens to the snare itself.
    let plate = s.bus("Snare Plate", 9, 0.0);
    s.fx(
        plate,
        "peel.fx.reverb",
        Some("Snare Plate"),
        &[("mix", 1.0)],
    );
    let snare_track = s.track(snare);
    s.keyed(
        plate,
        snare_track,
        "peel.fx.gate",
        "Gated Reverb",
        &[("threshold", -34.0), ("hold", 210.0), ("release", 45.0)],
        false,
    );
    s.send(snare, plate, -2.0);
    s.send(clap, plate, -8.0);
    s.send(tom, plate, -10.0);

    let hall = s.bus("Hall", 5, -3.0);
    s.fx(hall, "peel.fx.reverb", Some("Neon Hall"), &[("mix", 1.0)]);
    let echo = s.bus("Echo", 10, -5.0);
    s.fx(
        echo,
        "peel.fx.delay",
        Some("Dotted Ping-Pong"),
        &[("mix", 1.0), ("feedback", 0.42)],
    );
    s.fx(echo, "peel.fx.eq", Some("Clean Low End"), &[]);
    for (ch, bus, db) in [
        (arp_ch, echo, -9.0),
        (arp_ch, hall, -14.0),
        (pad, hall, -8.0),
        (brass, hall, -10.0),
        (lead, hall, -9.0),
        (lead, echo, -11.0),
        (bell, hall, -6.0),
        (bell, echo, -10.0),
        (riser, hall, -8.0),
        (crash, hall, -10.0),
    ] {
        s.send(ch, bus, db);
    }

    // Chorus on the pad and brass, and a pump under the kick.
    let kick_track = s.track(kick);
    let pad_track = s.track(pad);
    s.fx(pad_track, "peel.fx.vintage_chorus", Some("Both"), &[]);
    let brass_track = s.track(brass);
    s.fx(brass_track, "peel.fx.vintage_chorus", Some("Mode II"), &[]);
    let pump = [
        ("threshold", -26.0),
        ("ratio", 4.0),
        ("attack", 1.0),
        ("release", 170.0),
    ];
    for track in [pad_track, brass_track, s.track(low)] {
        s.keyed(
            track,
            kick_track,
            "peel.fx.compressor",
            "808 Duck",
            &pump,
            true,
        );
    }
    let lead_track = s.track(lead);
    s.fx(lead_track, "peel.fx.vintage_chorus", Some("Subtle"), &[]);
    let hat_track = s.track(hat);
    s.fx(hat_track, "peel.fx.eq", Some("Air"), &[]);

    let master = s.master();
    s.fx(master, "peel.fx.eq", Some("Clean Low End"), &[]);
    s.fx(master, "peel.fx.tape", Some("Master Glue"), &[]);
    s.fx(master, "peel.fx.mastering", Some("Neon"), &[("input", 9.0)]);

    // Drums.
    let mut verse = grid(
        kick,
        "X.......X.x.....|X.......X.......|X.......X.x.....|X.......X.....x.",
    );
    verse.extend(grid(
        snare,
        "....X.......X...|....X.......X...|....X.......X...|....X.......X...",
    ));
    verse.extend(grid(
        hat,
        "x.x.x.x.x.x.x.x.|x.x.x.x.x.x.x.x.|x.x.x.x.x.x.x.x.|x.x.x.x.x.x.x.x.",
    ));
    let verse_drums = s.pattern("Verse drums", 0, 4, verse);

    let mut chorus = grid(
        kick,
        "X.....x.X.x.....|X.....x.X.......|X.....x.X.x.....|X.....x.X.....x.",
    );
    chorus.extend(grid(
        snare,
        "....X.......X...|....X.......X...|....X.......X...|....X.......X...",
    ));
    chorus.extend(grid(
        clap,
        "....x.......x...|....x.......x...|....x.......x...|....x.......x...",
    ));
    chorus.extend(grid(
        hat,
        "xoxoxoxoxoxoxo.o|xoxoxoxoxoxoxo.o|xoxoxoxoxoxoxo.o|xoxoxoxoxoxoxoxo",
    ));
    chorus.extend(grid(
        open,
        "..............x.|..............x.|..............x.|................",
    ));
    let chorus_drums = s.pattern("Chorus drums", 1, 4, chorus);

    let fill = s.pattern("Tom fill", 3, 4, tom_fill(tom));
    // The break keeps time with the hat alone.
    let ticks = s.pattern("Break hats", 2, 1, grid(hat, "x...x...x...x.x."));
    // A snare that doubles into the second chorus.
    let mut lift = roll(snare, HIT, Time::ZERO, Time::new(1, 2), 8, (0.4, 0.65));
    lift.extend(roll(
        snare,
        HIT,
        Time::beats(4),
        Time::new(1, 4),
        16,
        (0.5, 0.95),
    ));
    let lift = s.pattern("Snare lift", 9, 2, lift);
    let crash_hit = s.pattern("Crash", 2, 1, grid(crash, "X..............."));
    let rise = s.pattern(
        "Riser",
        11,
        4,
        vec![Note::new(riser, Time::ZERO, Time::beats(16), 72, 0.8)],
    );

    // Bass: long notes for the intro, eighths with an octave pop for the
    // verse, driving sixteenths for the chorus.
    let long_bass = s.pattern("Long bass", 6, 4, bass(low, &[(0, 15, 0)], 0.75));
    let eighths: Vec<(i64, i64, u8)> = (0..8)
        .map(|i| (i * 2, 2, if i == 3 || i == 7 { 12 } else { 0 }))
        .collect();
    let verse_bass = s.pattern("Verse bass", 6, 4, bass(low, &eighths, 0.78));
    let sixteenths: Vec<(i64, i64, u8)> = (0..16)
        .map(|i| {
            (
                i,
                1,
                if matches!(i, 2 | 6 | 10 | 13 | 15) {
                    12
                } else {
                    0
                },
            )
        })
        .collect();
    let chorus_bass = s.pattern("Chorus bass", 6, 4, bass(low, &sixteenths, 0.76));

    // Harmony and melody.
    let arps = s.pattern(
        "Arp",
        7,
        4,
        arp(arp_ch, &ARP, &[0, 1, 2, 3, 2, 1, 0, 2], 1, 0.55),
    );
    let pads = s.pattern("Pad", 5, 4, chords(pad, &PAD, &[(0, 16)], 0.5, 0));
    let stabs = s.pattern(
        "Brass",
        4,
        4,
        chords(brass, &BRASS, &[(0, 5), (6, 2), (8, 7)], 0.7, 1),
    );
    let lead_line = s.pattern("Lead", 10, 8, line(lead, &LEAD, 0.74));
    let call = s.pattern("Bell call", 8, 4, line(bell, &CALL, 0.62));
    // The answer in the second chorus: two bell notes after each phrase.
    let mut answer = Vec::new();
    for (bar, tones) in ARP.iter().enumerate() {
        let at = bar as i64 * 16;
        answer.push((at + 4, 2, tones[2] + 12));
        answer.push((at + 12, 3, tones[1] + 12));
    }
    let answers = s.pattern("Bell answer", 8, 4, line(bell, &answer, 0.58));

    // Arrangement: intro 0, verse 8, chorus 24, break 40, chorus 48,
    // outro 64.
    const DRUMS: usize = 0;
    const FILLS: usize = 1;
    const BASS: usize = 2;
    const ARPS: usize = 3;
    const PADS: usize = 4;
    const BRASSES: usize = 5;
    const LEADS: usize = 6;
    const BELLS: usize = 7;
    const FX: usize = 8;
    s.put(ARPS, arps, 0, 72);
    s.put(PADS, pads, 0, 72);
    s.put(BASS, long_bass, 4, 4);
    s.put(FILLS, fill, 4, 4);
    s.put(FX, rise, 4, 4);

    s.put(FX, crash_hit, 8, 1);
    s.put(DRUMS, verse_drums, 8, 16);
    s.put(BASS, verse_bass, 8, 16);
    s.put(BELLS, call, 16, 8);
    s.put(FILLS, fill, 12, 4);
    s.put(FILLS, fill, 20, 4);
    s.put(FX, rise, 20, 4);

    s.put(FX, crash_hit, 24, 1);
    s.put(DRUMS, chorus_drums, 24, 16);
    s.put(BASS, chorus_bass, 24, 16);
    s.put(BRASSES, stabs, 24, 16);
    s.put(LEADS, lead_line, 24, 16);
    s.put(FILLS, fill, 28, 4);
    s.put(FILLS, fill, 36, 4);

    s.put(FX, crash_hit, 40, 1);
    s.put(DRUMS, ticks, 40, 6);
    s.put(BASS, long_bass, 40, 8);
    s.put(BELLS, call, 40, 8);
    s.put(FILLS, lift, 46, 2);
    s.put(FX, rise, 44, 4);

    s.put(FX, crash_hit, 48, 1);
    s.put(DRUMS, chorus_drums, 48, 16);
    s.put(BASS, chorus_bass, 48, 16);
    s.put(BRASSES, stabs, 48, 16);
    s.put(LEADS, lead_line, 48, 16);
    s.put(BELLS, answers, 48, 16);
    s.put(FILLS, fill, 52, 4);
    s.put(FILLS, fill, 60, 4);

    s.put(FX, crash_hit, 64, 1);
    s.put(DRUMS, verse_drums, 64, 4);
    s.put(BASS, long_bass, 64, 8);
    s.put(BELLS, call, 64, 8);

    // Automation: the arpeggio's filter opens through the intro, closes
    // for the break and opens again; the hall swells in the break; the
    // song fades over its last bars.
    let cut = |hz: f32| norm("peel.synth.poly", "filter.cutoff", hz);
    let arp_dev = s.device(arp_ch);
    s.automate(
        9,
        BARS,
        param(arp_dev, "filter.cutoff"),
        &[
            (0.0, cut(220.0), 0.3),
            (8.0, cut(900.0), 0.0),
            (24.0, cut(1_300.0), 0.0),
            (40.0, cut(1_300.0), 0.0),
            (40.0, cut(300.0), 0.4),
            (48.0, cut(1_400.0), 0.0),
            (64.0, cut(1_400.0), -0.3),
            (72.0, cut(260.0), 0.0),
        ],
    );
    s.automate(
        10,
        BARS,
        AutomationTarget::TrackGain { track: hall },
        &[
            (0.0, fader(-1.0), 0.0),
            (8.0, fader(-3.0), 0.0),
            (40.0, fader(-3.0), 0.0),
            (41.0, fader(1.0), 0.0),
            (47.0, fader(1.0), 0.0),
            (48.0, fader(-3.0), 0.0),
            (64.0, fader(-3.0), 0.0),
            (70.0, fader(2.0), 0.0),
            (72.0, fader(2.0), 0.0),
        ],
    );
    s.automate(
        11,
        BARS,
        AutomationTarget::TrackGain { track: master },
        &[
            (0.0, fader(0.0), 0.0),
            (68.0, fader(0.0), 0.3),
            (72.0, fader(-40.0), 0.0),
        ],
    );

    s.sections(&[
        (0, "Intro"),
        (8, "Verse"),
        (24, "Chorus"),
        (40, "Break"),
        (48, "Chorus"),
        (64, "Outro"),
    ])
}
