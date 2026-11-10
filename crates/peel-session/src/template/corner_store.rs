//! "Corner Store": swung boom bap at 86 BPM in C major (Fmaj9, Em7, Dm9,
//! G13). Electric piano chords open through a closed filter, the drums and
//! an upright-style bass drop in, a lazy pentatonic melody takes the second
//! half of the verse, an alto sax leads the middle section, vibes answer in
//! the last verse, and the tape wobbles harder as the song fades.
//!
//! The dust is made in the mixer: the drums run through a twelve-bit
//! crusher and a cassette, the keys through worn tape, and the whole mix
//! through one more tape before the mastering chain.

use peel_document::{AutomationTarget, Id, Note, Project, Time};

use super::kit::{HIT, Song, chords, fader, grid, line, n16, norm, param, roll};

/// Bars in the song.
const BARS: i64 = 56;

/// Rootless electric-piano voicings: Fmaj9, Em7, Dm9, G13.
const KEYS: [&[u8]; 4] = [
    &[57, 60, 64, 67],
    &[55, 59, 62, 64],
    &[53, 57, 60, 64],
    &[53, 57, 59, 64],
];

/// The bass: a root on the one, a ghost before beat three, a push on the
/// "and" of three and a walk into the next chord.
const BASS: [(i64, i64, u8); 16] = [
    (0, 6, 41),
    (7, 1, 41),
    (10, 4, 41),
    (14, 2, 36),
    (16, 6, 40),
    (23, 1, 40),
    (26, 4, 40),
    (30, 2, 39),
    (32, 6, 38),
    (39, 1, 38),
    (42, 4, 38),
    (46, 2, 45),
    (48, 6, 43),
    (55, 1, 43),
    (58, 3, 43),
    (62, 2, 42),
];

/// The melody: C major pentatonic, behind the beat.
const MELODY: [(i64, i64, u8); 18] = [
    (2, 2, 76),
    (4, 2, 79),
    (6, 4, 81),
    (12, 2, 79),
    (14, 2, 76),
    (16, 6, 74),
    (24, 2, 71),
    (26, 4, 74),
    (34, 2, 72),
    (36, 2, 76),
    (38, 4, 77),
    (44, 2, 76),
    (46, 2, 72),
    (48, 6, 71),
    (56, 2, 74),
    (58, 2, 76),
    (60, 2, 79),
    (62, 2, 81),
];

/// The sax in the middle section: eight bars, the second half reaching up.
const SAX: [(i64, i64, u8); 23] = [
    (0, 6, 69),
    (6, 2, 72),
    (8, 8, 76),
    (16, 4, 74),
    (20, 4, 71),
    (24, 8, 67),
    (32, 6, 69),
    (38, 2, 72),
    (40, 4, 77),
    (44, 4, 76),
    (48, 10, 74),
    (58, 2, 71),
    (60, 4, 67),
    (64, 6, 72),
    (70, 2, 76),
    (72, 8, 79),
    (80, 4, 81),
    (84, 4, 79),
    (88, 8, 76),
    (96, 6, 77),
    (102, 2, 76),
    (104, 8, 72),
    (112, 12, 74),
];

/// Vibes: a few high notes in the gaps of the melody.
const VIBES: [(i64, i64, u8); 6] = [
    (10, 2, 72),
    (22, 2, 71),
    (28, 3, 74),
    (42, 2, 72),
    (54, 2, 69),
    (60, 4, 71),
];

/// Chords for one pass of the progression: struck on the one and pushed
/// again before beat three, the second hit softer.
fn comp(ch: Id) -> Vec<Note> {
    let mut v = chords(ch, &KEYS, &[(0, 6)], 0.6, 4);
    v.extend(chords(ch, &KEYS, &[(7, 7)], 0.45, 3));
    v
}

pub fn corner_store() -> Project {
    let mut s = Song::new(
        "Corner Store",
        "Boom bap. Swing is on; the tape and crusher in the mixer make the dust.",
        86.0,
        0.42,
        &[
            "Drums", "Bass", "Keys", "Melody", "Sax", "Vibes", "Filter", "Tape", "Fade",
        ],
    );

    // Drums.
    let kick = s.inst("Kick", 0, "peel.drum.kick", "Boom Bap", &[], HIT, -7.0);
    let snare = s.inst(
        "Snare",
        9,
        "peel.drum.snare",
        "Dusty",
        &[("noise.tone", 4_500.0)],
        HIT,
        -6.0,
    );
    let hat = s.inst("Hat", 2, "peel.drum.hat", "Dusty", &[], HIT, -9.0);
    let open = s.inst(
        "Open Hat",
        2,
        "peel.drum.hat",
        "Dusty Open",
        &[],
        HIT,
        -14.0,
    );
    let rim = s.inst("Rim", 3, "peel.drum.snare", "Rim Click", &[], HIT, -17.0);
    // The band.
    let bass = s.inst("Bass", 6, "peel.synth.bass", "Upright", &[], 41, -9.5);
    let keys = s.inst(
        "Keys",
        4,
        "peel.keys.tine",
        "Lo-Fi",
        &[("out.tone", 0.6), ("tone.bell", 0.45), ("out.width", 1.0)],
        57,
        -10.0,
    );
    let melody = s.inst(
        "Melody",
        5,
        "peel.synth.poly",
        "Lo-Fi Keys",
        &[("filter.cutoff", 2_600.0)],
        76,
        -11.0,
    );
    let sax = s.inst("Sax", 8, "peel.wind.sax", "Smooth Alto", &[], 69, -15.0);
    let vibes = s.inst("Vibes", 10, "peel.perc.mallet", "Vibes", &[], 72, 3.0);

    // Mixer: kick, snare and rim go to tape through a crusher.
    let drum_bus = s.bus("Drum Bus", 0, -7.0);
    // The hats skip the crusher and keep their top end.
    s.route(&[kick, snare, rim], drum_bus);
    s.fx(
        drum_bus,
        "peel.fx.crush",
        Some("Twelve Bit"),
        &[("mix", 0.6)],
    );
    s.fx(
        drum_bus,
        "peel.fx.tape",
        Some("Cassette"),
        &[("hiss", 0.0), ("wow", 0.2), ("tone", 13_000.0)],
    );
    s.fx(drum_bus, "peel.fx.compressor", Some("Punchy Drums"), &[]);
    s.fx(
        drum_bus,
        "peel.fx.limiter",
        Some("Drum Bus Catch"),
        &[("input", 3.0)],
    );
    s.pan(hat, 0.18);
    s.pan(open, -0.15);
    s.pan(rim, -0.25);

    let room = s.bus("Room", 5, -4.0);
    s.fx(room, "peel.fx.reverb", Some("Dusty Room"), &[("mix", 1.0)]);
    let echo = s.bus("Echo", 10, -6.0);
    s.fx(echo, "peel.fx.delay", Some("Worn Echo"), &[("mix", 1.0)]);
    for (ch, bus, db) in [
        (snare, room, -12.0),
        (rim, room, -8.0),
        (keys, room, -12.0),
        (melody, room, -8.0),
        (melody, echo, -10.0),
        (sax, room, -7.0),
        (sax, echo, -12.0),
        (vibes, room, -6.0),
        (vibes, echo, -9.0),
    ] {
        s.send(ch, bus, db);
    }

    // The keys wobble on worn tape and lean back when the kick hits.
    let (keys_track, kick_track) = (s.track(keys), s.track(kick));
    s.fx(
        keys_track,
        "peel.fx.tape",
        Some("Worn"),
        &[("hiss", 0.3), ("tone", 7_500.0)],
    );
    s.keyed(
        keys_track,
        kick_track,
        "peel.fx.compressor",
        "808 Duck",
        &[
            ("threshold", -24.0),
            ("ratio", 3.0),
            ("attack", 5.0),
            ("release", 240.0),
        ],
        false,
    );
    // A direct electric piano is dark; lift its presence after the tape.
    s.fx(keys_track, "peel.fx.eq", Some("Vocal Presence"), &[]);
    let bass_track = s.track(bass);
    s.fx(bass_track, "peel.fx.bass_lift", Some("Warm Bass"), &[]);
    let melody_track = s.track(melody);
    s.fx(melody_track, "peel.fx.chorus", Some("Subtle Width"), &[]);
    let sax_track = s.track(sax);
    s.fx(
        sax_track,
        "peel.fx.tape",
        Some("Cassette"),
        &[("hiss", 0.0), ("wow", 0.25)],
    );

    let master = s.master();
    let sweep = s.fx(
        master,
        "peel.fx.filter",
        Some("Soft Low Pass"),
        &[("cutoff", 20_000.0), ("resonance", 0.15), ("slope", 0.0)],
    );
    let tape = s.fx(
        master,
        "peel.fx.tape",
        Some("Cassette"),
        &[
            ("drive", 5.0),
            ("tone", 14_000.0),
            ("wow", 0.25),
            ("flutter", 0.2),
            ("hiss", 0.45),
        ],
    );
    s.fx(
        master,
        "peel.fx.mastering",
        Some("Dusty"),
        &[("input", 8.0), ("high", 1.0)],
    );

    // Drums: a lazy two-bar beat and a variation for the middle section.
    let mut beat = grid(kick, "X......x..X.....|X.....x...X..x..");
    beat.extend(grid(snare, "....X.......X...|....X.......X..o"));
    beat.extend(grid(hat, "x.xox.x.x.xox.xo|x.x.x.xox.x.x.x."));
    beat.extend(grid(open, "................|......x........."));
    beat.extend(grid(rim, ".......o........|..........o....."));
    let beat = s.pattern("Beat", 0, 2, beat);

    let mut half = grid(kick, "X.........X.....|X......x..X.....");
    half.extend(grid(snare, "....X.......X...|....X.......X..."));
    half.extend(grid(hat, "x...x...x...x.x.|x...x...x...x..."));
    half.extend(grid(rim, "..o.......o.....|..o.......o..o.."));
    let half = s.pattern("Half beat", 1, 2, half);

    // A pickup into the first verse and out of the break.
    let mut pickup = grid(snare, "............x.xX");
    pickup.extend(roll(
        hat,
        HIT,
        Time::beats(2),
        Time::new(1, 4),
        4,
        (0.4, 0.7),
    ));
    let pickup = s.pattern("Pickup", 9, 1, pickup);

    // The band.
    let bass_line = s.pattern("Bass", 6, 4, line(bass, &BASS, 0.8));
    let long_bass = s.pattern(
        "Long bass",
        6,
        4,
        [41u8, 40, 38, 43]
            .into_iter()
            .enumerate()
            .map(|(bar, key)| n16(bass, bar as i64 * 16, 14, key, 0.7))
            .collect(),
    );
    let comping = s.pattern("Keys", 4, 4, comp(keys));
    // Held chords for the intro, the break and the outro.
    let held = s.pattern("Held keys", 4, 4, chords(keys, &KEYS, &[(0, 15)], 0.55, 6));
    let tune = s.pattern("Melody", 5, 4, line(melody, &MELODY, 0.66));
    let sax_line = s.pattern("Sax", 8, 8, line(sax, &SAX, 0.68));
    let tinkles = s.pattern("Vibes", 10, 4, line(vibes, &VIBES, 0.85));

    // Arrangement: intro 0, verse 4, middle 20, verse 28, break 40,
    // verse 44, outro 52.
    const DRUMS: usize = 0;
    const LOW: usize = 1;
    const KEY: usize = 2;
    const TUNE: usize = 3;
    const HORN: usize = 4;
    const VIBE: usize = 5;
    s.put(KEY, held, 0, 4);
    s.put(DRUMS, pickup, 3, 1);

    s.put(DRUMS, beat, 4, 16);
    s.put(LOW, bass_line, 4, 16);
    s.put(KEY, comping, 4, 16);
    s.put(TUNE, tune, 12, 8);

    s.put(DRUMS, half, 20, 8);
    s.put(LOW, bass_line, 20, 8);
    s.put(KEY, comping, 20, 8);
    s.put(HORN, sax_line, 20, 8);

    s.put(DRUMS, beat, 28, 12);
    s.put(LOW, bass_line, 28, 12);
    s.put(KEY, comping, 28, 12);
    s.put(TUNE, tune, 28, 12);
    s.put(VIBE, tinkles, 32, 8);

    s.put(KEY, held, 40, 4);
    s.put(LOW, long_bass, 40, 4);
    s.put(HORN, sax_line, 40, 4);
    s.put(DRUMS, pickup, 43, 1);

    s.put(DRUMS, beat, 44, 8);
    s.put(LOW, bass_line, 44, 8);
    s.put(KEY, comping, 44, 8);
    s.put(TUNE, tune, 44, 8);
    s.put(VIBE, tinkles, 44, 8);
    s.put(HORN, sax_line, 48, 4);

    s.put(KEY, held, 52, 4);
    s.put(VIBE, tinkles, 52, 4);

    // Automation: the filter opens over the intro, closes for the break
    // and at the end; the tape wows harder as the song runs down.
    let lp = |hz: f32| norm("peel.fx.filter", "cutoff", hz);
    s.automate(
        6,
        BARS,
        param(sweep, "cutoff"),
        &[
            (0.0, lp(700.0), 0.3),
            (4.0, lp(20_000.0), 0.0),
            (40.0, lp(20_000.0), 0.0),
            (40.0, lp(1_600.0), 0.3),
            (44.0, lp(20_000.0), 0.0),
            (52.0, lp(20_000.0), -0.4),
            (56.0, lp(500.0), 0.0),
        ],
    );
    let wow = |v: f32| norm("peel.fx.tape", "wow", v);
    s.automate(
        7,
        BARS,
        param(tape, "wow"),
        &[
            (0.0, wow(0.5), 0.0),
            (4.0, wow(0.25), 0.0),
            (52.0, wow(0.25), 0.0),
            (56.0, wow(1.0), 0.0),
        ],
    );
    s.automate(
        8,
        BARS,
        AutomationTarget::TrackGain { track: master },
        &[
            (0.0, fader(0.0), 0.0),
            (53.0, fader(0.0), 0.3),
            (56.0, fader(-40.0), 0.0),
        ],
    );

    s.sections(&[
        (0, "Intro"),
        (4, "Verse"),
        (20, "Middle"),
        (28, "Verse"),
        (40, "Break"),
        (44, "Verse"),
        (52, "Outro"),
    ])
}
