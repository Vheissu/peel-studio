//! "Night Shift": trap at 142 BPM in F minor (Fm, Db, Bbm, C). A bell
//! melody over a felt piano opens it, the hook drops in with a sliding 808
//! under a half-time snare and rolling hats, the verse thins out to a
//! pluck, a filtered break builds on a snare roll, and the second hook
//! adds a flute line and a harder 808.
//!
//! The 808 plays the `Slide` preset: a note that runs into the next one
//! bends to it instead of restarting, so every octave jump and fall in the
//! bass line is two overlapping notes in the Piano Roll.

use peel_document::{AutomationTarget, Note, Project, Time};

use super::kit::{HIT, Song, arp, chords, fader, grid, line, n16, norm, param, roll, slides};

/// Bars in the song.
const BARS: i64 = 68;

/// Piano voicings: Fm(add9), Dbmaj7, Bbm7, C7.
const KEYS: [&[u8]; 4] = [
    &[53, 56, 60, 67],
    &[49, 53, 56, 60],
    &[46, 53, 56, 61],
    &[48, 52, 58, 64],
];
/// Pad triads an octave above the piano.
const PAD: [&[u8]; 4] = [&[65, 68, 72], &[61, 65, 68], &[58, 61, 65], &[60, 64, 67]];
/// Pluck tones for the verse.
const PLUCK: [&[u8]; 4] = [&[65, 68, 72], &[65, 68, 73], &[65, 70, 73], &[64, 67, 72]];

/// The bell line: a 3-3-2 figure that falls through each chord.
const BELLS: [(i64, i64, u8); 23] = [
    (0, 3, 84),
    (3, 3, 80),
    (6, 2, 77),
    (8, 2, 79),
    (10, 4, 80),
    (14, 2, 72),
    (16, 3, 80),
    (19, 3, 77),
    (22, 2, 73),
    (24, 2, 77),
    (26, 6, 75),
    (32, 3, 82),
    (35, 3, 80),
    (38, 2, 77),
    (40, 2, 73),
    (42, 4, 77),
    (46, 2, 73),
    (48, 3, 79),
    (51, 3, 76),
    (54, 2, 72),
    (56, 4, 76),
    (60, 2, 79),
    (62, 2, 82),
];

/// The flute answer in the second hook: long notes against the bells.
const FLUTE: [(i64, i64, u8); 12] = [
    (0, 10, 72),
    (12, 4, 75),
    (16, 8, 77),
    (24, 4, 75),
    (28, 4, 73),
    (32, 6, 77),
    (38, 4, 73),
    (42, 6, 70),
    (48, 8, 72),
    (56, 4, 76),
    (60, 2, 79),
    (62, 2, 76),
];

/// Hook 808 as `(step, length, key, slide)`: F1, Db2, Bb1 and C2 following
/// the kick, with a slide at the end of every bar (up an octave, down a
/// fourth, up an octave, down a fourth).
const HOOK_808: [(i64, i64, u8, bool); 16] = [
    (0, 6, 29, false),
    (6, 4, 29, false),
    (10, 3, 29, true),
    (13, 3, 41, false),
    (16, 3, 37, false),
    (19, 7, 37, false),
    (26, 3, 37, true),
    (29, 3, 32, false),
    (32, 6, 34, false),
    (38, 4, 34, false),
    (42, 3, 34, true),
    (45, 3, 46, false),
    (48, 3, 36, false),
    (51, 7, 36, false),
    (58, 2, 36, true),
    (60, 4, 31, false),
];

/// Verse 808: longer notes, a semitone fall and a rise of a fifth.
const VERSE_808: [(i64, i64, u8, bool); 10] = [
    (0, 9, 29, false),
    (10, 5, 29, false),
    (16, 9, 37, false),
    (26, 3, 37, true),
    (29, 3, 36, false),
    (32, 9, 34, false),
    (42, 5, 34, false),
    (48, 9, 36, false),
    (58, 2, 36, true),
    (60, 4, 43, false),
];

/// Hats for one bar: eighths, with the beats named in `gaps` left empty
/// for a roll.
fn hats(ch: peel_document::Id, bar: i64, gaps: &[i64]) -> Vec<Note> {
    (0..8)
        .filter(|i| !gaps.contains(&(i / 2)))
        .map(|i| {
            let vel = if i % 2 == 0 { 0.8 } else { 0.5 };
            n16(ch, bar * 16 + i * 2, 1, HIT, vel)
        })
        .collect()
}

pub fn night_shift() -> Project {
    let mut s = Song::new(
        "Night Shift",
        "Trap. The 808 slides wherever two of its notes overlap in the Piano Roll.",
        142.0,
        0.0,
        &[
            "Drums",
            "Perc",
            "808",
            "Bells",
            "Keys",
            "Pad",
            "Lead",
            "FX",
            "Bell filter",
            "Drum filter",
            "808 drive",
            "Reverb",
            "Fade",
        ],
    );

    // Drums.
    let kick = s.inst("Kick", 0, "peel.drum.kick", "Trap Kick", &[], HIT, -7.0);
    let snare = s.inst("Snare", 9, "peel.drum.snare", "Trap", &[], HIT, -6.0);
    let clap = s.inst("Clap", 1, "peel.drum.clap", "Trap", &[], HIT, -9.0);
    let hat = s.inst("Hat", 2, "peel.drum.hat", "Trap Tight", &[], HIT, -9.0);
    let open = s.inst("Open Hat", 2, "peel.drum.hat", "Trap Open", &[], HIT, -15.0);
    let rim = s.inst("Rim", 3, "peel.drum.snare", "Rim Click", &[], HIT, -14.0);
    let crash = s.inst("Crash", 2, "peel.drum.hat", "Crash", &[], HIT, -17.0);
    // Low end.
    let sub = s.inst("808", 6, "peel.bass.808", "Slide", &[], 29, -7.0);
    // Melody and harmony.
    let bell = s.inst("Bells", 5, "peel.synth.cross", "Night Bell", &[], 72, 0.5);
    let keys = s.inst("Keys", 4, "peel.keys.grand", "Felt", &[], 53, -13.0);
    let pad = s.inst("Pad", 7, "peel.synth.poly", "Dark Choir", &[], 65, -11.0);
    let pluck = s.inst("Pluck", 10, "peel.synth.poly", "Dark Pluck", &[], 65, -8.0);
    let flute = s.inst("Flute", 8, "peel.synth.poly", "Flute Lead", &[], 72, -13.0);
    let riser = s.inst(
        "Riser",
        11,
        "peel.synth.poly",
        "Noise Riser",
        &[],
        72,
        -12.0,
    );

    // Mixer: the drums share a bus, the 808 ducks under the kick, and two
    // returns give the melody its space.
    let drum_bus = s.bus("Drum Bus", 0, -7.0);
    s.route(&[kick, snare, clap, hat, open, rim, crash], drum_bus);
    let drum_filter = s.fx(
        drum_bus,
        "peel.fx.filter",
        Some("Soft Low Pass"),
        &[("cutoff", 20_000.0), ("resonance", 0.2), ("slope", 1.0)],
    );
    s.fx(drum_bus, "peel.fx.eq", Some("Air"), &[]);
    s.fx(drum_bus, "peel.fx.compressor", Some("Punchy Drums"), &[]);
    s.fx(
        drum_bus,
        "peel.fx.limiter",
        Some("Drum Bus Catch"),
        &[("input", 4.0)],
    );
    s.pan(hat, 0.12);
    s.pan(open, -0.2);
    s.pan(rim, 0.25);

    let (sub_track, kick_track) = (s.track(sub), s.track(kick));
    s.keyed(
        sub_track,
        kick_track,
        "peel.fx.compressor",
        "808 Duck",
        &[],
        true,
    );

    let space = s.bus("Space", 5, -2.0);
    s.fx(space, "peel.fx.reverb", Some("Dark Space"), &[("mix", 1.0)]);
    let echo = s.bus("Echo", 10, -4.0);
    s.fx(
        echo,
        "peel.fx.delay",
        Some("Dotted Ping-Pong"),
        &[("mix", 1.0), ("feedback", 0.38)],
    );
    s.fx(echo, "peel.fx.eq", Some("Clean Low End"), &[]);
    for (ch, bus, db) in [
        (bell, space, -8.0),
        (bell, echo, -12.0),
        (keys, space, -10.0),
        (pad, space, -6.0),
        (pluck, space, -12.0),
        (pluck, echo, -10.0),
        (flute, space, -8.0),
        (flute, echo, -14.0),
        (snare, space, -18.0),
        (clap, space, -14.0),
        (rim, echo, -16.0),
        (riser, space, -8.0),
    ] {
        s.send(ch, bus, db);
    }
    let bell_track = s.track(bell);
    s.fx(bell_track, "peel.fx.eq", Some("Air"), &[]);
    let keys_track = s.track(keys);
    s.fx(
        keys_track,
        "peel.fx.tape",
        Some("Cassette"),
        &[("hiss", 0.15)],
    );
    let pad_track = s.track(pad);
    s.fx(pad_track, "peel.fx.vintage_chorus", Some("Wide Wash"), &[]);

    let master = s.master();
    s.fx(master, "peel.fx.eq", Some("Clean Low End"), &[]);
    s.fx(master, "peel.fx.compressor", Some("Gentle Glue"), &[]);
    s.fx(
        master,
        "peel.fx.mastering",
        Some("Low End"),
        &[("input", 9.0), ("high", 3.0)],
    );

    // Drums.
    let third = Time::new(1, 6);
    let thirty_second = Time::new(1, 8);
    let mut hook = grid(
        kick,
        "X.....x...x.....|X..x......x...x.|X.....x...x.....|X..x......x.....",
    );
    hook.extend(grid(
        snare,
        "........X.......|........X.......|........X.......|........X......o",
    ));
    hook.extend(grid(
        clap,
        "........x.......|........x.......|........x.......|........x.......",
    ));
    hook.extend(hats(hat, 0, &[]));
    hook.extend(hats(hat, 1, &[3]));
    hook.extend(hats(hat, 2, &[1]));
    hook.extend(hats(hat, 3, &[2, 3]));
    // Rolls: sixteenth triplets, a quick pair of thirty-seconds, and a
    // rising run of thirty-seconds into the next phrase.
    hook.extend(roll(hat, HIT, Time::beats(7), third, 6, (0.45, 0.85)));
    hook.extend(roll(hat, HIT, Time::beats(9), thirty_second, 4, (0.7, 0.4)));
    hook.extend(roll(
        hat,
        HIT,
        Time::new(19, 2),
        Time::new(1, 4),
        2,
        (0.5, 0.6),
    ));
    hook.extend(roll(
        hat,
        HIT,
        Time::beats(14),
        thirty_second,
        8,
        (0.35, 0.7),
    ));
    hook.extend(roll(hat, HIT, Time::beats(15), third, 6, (0.5, 0.95)));
    hook.extend(grid(
        open,
        "..............x.|................|......x.........|................",
    ));
    hook.extend(grid(
        rim,
        "...........x....|......x.....x...|...x.......x....|......x......x..",
    ));
    let hook_drums = s.pattern("Hook drums", 0, 4, hook);

    let mut verse = grid(
        kick,
        "X.........x.....|X.........x.....|X.........x.....|X.........x.....",
    );
    verse.extend(grid(
        snare,
        "........X.......|........X.......|........X.......|........X.......",
    ));
    verse.extend(hats(hat, 0, &[]));
    verse.extend(hats(hat, 1, &[]));
    verse.extend(hats(hat, 2, &[]));
    verse.extend(hats(hat, 3, &[3]));
    verse.extend(roll(hat, HIT, Time::beats(15), third, 6, (0.4, 0.8)));
    verse.extend(grid(
        rim,
        "................|......x.........|................|......x......x..",
    ));
    let verse_drums = s.pattern("Verse drums", 0, 4, verse);

    // The hats alone, for the second half of the intro.
    let mut ticks = hats(hat, 0, &[]);
    ticks.extend(hats(hat, 1, &[3]));
    ticks.extend(roll(hat, HIT, Time::beats(7), third, 6, (0.35, 0.7)));
    let intro_hats = s.pattern("Intro hats", 2, 2, ticks);

    // The build: a snare roll that doubles each bar under the riser.
    let mut build = Vec::new();
    for (bar, every) in [(0, Time::new(1, 2)), (1, Time::new(1, 4))] {
        let count = (4.0 / every.as_f64()) as i64;
        let lo = 0.4 + 0.2 * bar as f32;
        build.extend(roll(
            snare,
            HIT,
            Time::beats(bar * 4),
            every,
            count,
            (lo, lo + 0.25),
        ));
    }
    let build = s.pattern("Build", 9, 2, build);
    let rise = s.pattern(
        "Riser",
        11,
        4,
        vec![Note::new(riser, Time::ZERO, Time::beats(16), 72, 0.8)],
    );
    let crash_hit = s.pattern("Crash", 2, 1, grid(crash, "X..............."));

    let hook_808 = s.pattern("Hook 808", 6, 4, slides(sub, &HOOK_808, 0.9));
    let verse_808 = s.pattern("Verse 808", 6, 4, slides(sub, &VERSE_808, 0.85));
    // One long F that falls an octave to end the song.
    let last_808 = s.pattern(
        "Last 808",
        6,
        2,
        slides(sub, &[(0, 12, 41, true), (12, 20, 29, false)], 0.9),
    );

    // Melody and harmony.
    let bells = s.pattern("Bells", 5, 4, line(bell, &BELLS, 0.72));
    // The second hook answers the last bar an octave up.
    let mut high = BELLS.to_vec();
    for n in high.iter_mut().filter(|n| n.0 >= 48) {
        n.2 += 12;
    }
    let bells_high = s.pattern("Bells high", 5, 4, line(bell, &high, 0.75));
    let mut piano = chords(keys, &KEYS, &[(0, 9)], 0.62, 3);
    piano.extend(chords(keys, &KEYS, &[(10, 5)], 0.42, 2));
    let piano = s.pattern("Keys", 4, 4, piano);
    let pads = s.pattern("Pad", 7, 4, chords(pad, &PAD, &[(0, 16)], 0.5, 0));
    let plucks = s.pattern(
        "Pluck",
        10,
        4,
        arp(pluck, &PLUCK, &[0, 1, 2, 3, 2], 3, 0.62),
    );
    let flutes = s.pattern("Flute", 8, 4, line(flute, &FLUTE, 0.7));
    // A two-note pickup into each hook.
    let pickup = s.pattern(
        "Pickup",
        6,
        1,
        slides(sub, &[(8, 4, 41, true), (12, 4, 29, false)], 0.85),
    );

    // Arrangement: intro 0, hook 8, verse 24, break 40, hook 44, outro 60.
    const DRUMS: usize = 0;
    const PERC: usize = 1;
    const BASS: usize = 2;
    const BELL: usize = 3;
    const KEY: usize = 4;
    const PADS: usize = 5;
    const LEAD: usize = 6;
    const FX: usize = 7;
    s.put(BELL, bells, 0, 8);
    s.put(KEY, piano, 0, 8);
    s.put(PADS, pads, 4, 4);
    s.put(PERC, intro_hats, 4, 4);
    s.put(FX, rise, 4, 4);
    s.put(BASS, pickup, 7, 1);

    s.put(FX, crash_hit, 8, 1);
    s.put(DRUMS, hook_drums, 8, 16);
    s.put(BASS, hook_808, 8, 16);
    s.put(BELL, bells, 8, 16);
    s.put(KEY, piano, 8, 16);
    s.put(PADS, pads, 16, 8);

    s.put(DRUMS, verse_drums, 24, 16);
    s.put(BASS, verse_808, 24, 16);
    s.put(LEAD, plucks, 24, 16);
    s.put(KEY, piano, 32, 8);
    s.put(PADS, pads, 32, 8);

    s.put(BELL, bells, 40, 4);
    s.put(PADS, pads, 40, 4);
    s.put(FX, rise, 40, 4);
    s.put(PERC, build, 42, 2);
    s.put(BASS, pickup, 43, 1);

    s.put(FX, crash_hit, 44, 1);
    s.put(DRUMS, hook_drums, 44, 16);
    s.put(BASS, hook_808, 44, 16);
    s.put(BELL, bells_high, 44, 16);
    s.put(KEY, piano, 44, 16);
    s.put(PADS, pads, 44, 16);
    s.put(LEAD, flutes, 44, 16);

    s.put(FX, crash_hit, 60, 1);
    s.put(BELL, bells, 60, 8);
    s.put(KEY, piano, 60, 6);
    s.put(PADS, pads, 60, 8);
    s.put(BASS, last_808, 60, 2);

    // Automation. The bells open up through the intro and again through
    // the break; the drums are filtered out of the build; the 808 is
    // driven harder in the second hook; the reverb return swells in the
    // break and the outro.
    let cut = |hz: f32| norm("peel.synth.cross", "filter.cutoff", hz);
    let bell_dev = s.device(bell);
    s.automate(
        8,
        BARS,
        param(bell_dev, "filter.cutoff"),
        &[
            (0.0, cut(700.0), 0.3),
            (8.0, cut(5_000.0), 0.0),
            (40.0, cut(5_000.0), 0.0),
            (40.0, cut(900.0), 0.4),
            (44.0, cut(6_500.0), 0.0),
            (60.0, cut(6_500.0), -0.3),
            (68.0, cut(1_200.0), 0.0),
        ],
    );
    let lp = |hz: f32| norm("peel.fx.filter", "cutoff", hz);
    s.automate(
        9,
        BARS,
        param(drum_filter, "cutoff"),
        &[
            (0.0, lp(20_000.0), 0.0),
            (42.0, lp(20_000.0), 0.0),
            (42.0, lp(900.0), 0.5),
            (44.0, lp(20_000.0), 0.0),
            (68.0, lp(20_000.0), 0.0),
        ],
    );
    let drive = |v: f32| norm("peel.bass.808", "drive.amount", v);
    let sub_dev = s.device(sub);
    s.automate(
        10,
        BARS,
        param(sub_dev, "drive.amount"),
        &[
            (0.0, drive(0.35), 0.0),
            (44.0, drive(0.35), 0.0),
            (44.0, drive(0.5), 0.0),
            (52.0, drive(0.6), 0.0),
            (60.0, drive(0.35), 0.0),
            (68.0, drive(0.35), 0.0),
        ],
    );
    s.automate(
        11,
        BARS,
        AutomationTarget::TrackGain { track: space },
        &[
            (0.0, fader(-2.0), 0.0),
            (40.0, fader(-2.0), 0.0),
            (42.0, fader(3.0), 0.0),
            (44.0, fader(-2.0), 0.0),
            (60.0, fader(-2.0), 0.0),
            (66.0, fader(3.0), 0.0),
            (68.0, fader(3.0), 0.0),
        ],
    );
    s.automate(
        12,
        BARS,
        AutomationTarget::TrackGain { track: master },
        &[
            (0.0, fader(0.0), 0.0),
            (65.0, fader(0.0), 0.3),
            (68.0, fader(-40.0), 0.0),
        ],
    );

    s.sections(&[
        (0, "Intro"),
        (8, "Hook"),
        (24, "Verse"),
        (40, "Break"),
        (44, "Hook"),
        (60, "Outro"),
    ])
}
