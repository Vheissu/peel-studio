//! Song starters, one per style. Each is a short arrangement with named
//! sections (intro, verse, hook or chorus, build and drop, outro) built
//! from editable pattern clips on native instruments. They are starting
//! material to extend, not finished mixes, so every part sits well below
//! full scale and the master only carries a safety limiter.

use std::sync::Arc;

use peel_document::{Clip, ClipContent, Id, Marker, Note, PALETTE, Pattern, Project, Time, Track};

use super::{add_channel, device};

/// One bar of 4/4, in quarter notes.
const BAR: i64 = 4;

/// Drum voices all play their root.
const HIT: u8 = 60;

/// Builds a song: parts on their own mixer tracks, patterns, and a
/// playlist with one named lane per role.
struct Song {
    p: Project,
    lanes: Vec<Id>,
}

impl Song {
    fn new(bpm: f64, swing: f32, lanes: &[&str]) -> Song {
        let mut p = Project::empty("Untitled");
        p.set_bpm(bpm);
        p.swing = swing;
        p.patterns.clear();
        p.mixer.tracks[0]
            .inserts
            .push(device("peel.fx.limiter", Some("Master Safety")));
        let arr = p.arrangement_mut();
        while arr.tracks.len() < lanes.len() {
            arr.tracks.push(Track {
                id: Id::new(),
                name: String::new(),
                color: None,
                muted: false,
            });
        }
        for (t, name) in arr.tracks.iter_mut().zip(lanes) {
            t.name = (*name).to_string();
        }
        let lanes = arr.tracks.iter().map(|t| t.id).collect();
        Song { p, lanes }
    }

    /// Adds an instrument with a factory preset at `gain_db`.
    fn part(
        &mut self,
        name: &str,
        color: usize,
        type_id: &str,
        preset: &str,
        root: u8,
        gain_db: f32,
    ) -> Id {
        let dev = device(type_id, Some(preset));
        debug_assert_eq!(dev.preset.as_deref(), Some(preset), "{type_id}");
        let id = add_channel(&mut self.p, name, color, dev, root);
        if let Some(c) = self.p.channels.iter_mut().find(|c| c.id == id) {
            c.gain_db = gain_db;
        }
        id
    }

    fn track(&self, ch: Id) -> Id {
        self.p.channel(ch).map(|c| c.track).unwrap_or_default()
    }

    /// Adds a factory-preset effect to a part's mixer track.
    fn fx(&mut self, ch: Id, type_id: &str, preset: &str) {
        let track = self.track(ch);
        if let Some(t) = self.p.mixer.track_mut(track) {
            t.inserts.push(device(type_id, Some(preset)));
        }
    }

    /// Ducks a part under the kick with a keyed compressor.
    fn pump(&mut self, ch: Id, kick: Id, preset: &str) {
        let (track, key) = (self.track(ch), self.track(kick));
        if let Some(t) = self.p.mixer.track_mut(track) {
            let mut duck = device("peel.fx.compressor", Some(preset));
            duck.sidechain = Some(key);
            t.inserts.insert(0, duck);
        }
    }

    fn pattern(&mut self, name: &str, color: usize, bars: i64, mut notes: Vec<Note>) -> Id {
        notes.sort_by_key(|n| (n.start, n.key));
        let pat = Pattern {
            id: Id::new(),
            name: name.into(),
            color: PALETTE[color % PALETTE.len()],
            length: Time::beats(bars * BAR),
            notes,
            expression: Vec::new(),
        };
        let id = pat.id;
        self.p.patterns.push(Arc::new(pat));
        id
    }

    /// Places clips as `(lane, pattern, first bar, bars)`.
    fn place(&mut self, clips: &[(usize, Id, i64, i64)]) {
        let arr = self.p.arrangement_mut();
        for &(lane, pattern, bar, bars) in clips {
            arr.clips.push(Clip {
                id: Id::new(),
                track: self.lanes[lane],
                start: Time::beats(bar * BAR),
                length: Time::beats(bars * BAR),
                offset: Time::ZERO,
                muted: false,
                content: ClipContent::Pattern { pattern },
            });
        }
    }

    /// Section markers as `(bar, name)`.
    fn sections(mut self, marks: &[(i64, &str)]) -> Project {
        let arr = self.p.arrangement_mut();
        for &(bar, name) in marks {
            arr.markers.push(Marker {
                id: Id::new(),
                position: Time::beats(bar * BAR),
                name: name.into(),
            });
        }
        self.p
    }
}

/// A note on the sixteenth grid.
fn n16(ch: Id, step: i64, len: i64, key: u8, vel: f32) -> Note {
    Note::new(ch, Time::new(step, 4), Time::new(len, 4), key, vel)
}

/// Drum hits from a step grid, one character per sixteenth: `X` accent,
/// `x` hit, `o` ghost and anything else a rest. `|` and spaces only make
/// bars easier to read.
fn grid(ch: Id, steps: &str) -> Vec<Note> {
    steps
        .chars()
        .filter(|c| !matches!(c, '|' | ' '))
        .enumerate()
        .filter_map(|(i, c)| {
            let vel = match c {
                'X' => 1.0,
                'x' => 0.78,
                'o' => 0.42,
                _ => return None,
            };
            Some(n16(ch, i as i64, 1, HIT, vel))
        })
        .collect()
}

/// `count` evenly spaced hits from `start`, rising from `vel.0` to `vel.1`.
fn roll(ch: Id, key: u8, start: Time, every: Time, count: i64, vel: (f32, f32)) -> Vec<Note> {
    (0..count)
        .map(|i| {
            let t = i as f32 / (count - 1).max(1) as f32;
            let v = vel.0 + (vel.1 - vel.0) * t;
            Note::new(ch, start + every.mul_int(i), every, key, v)
        })
        .collect()
}

/// A line of `(step, length, key)` in sixteenths, accenting the beat.
fn line(ch: Id, notes: &[(i64, i64, u8)], vel: f32) -> Vec<Note> {
    notes
        .iter()
        .map(|&(s, l, k)| {
            let accent = if s % 4 == 0 { 0.06 } else { -0.04 };
            n16(ch, s, l, k, (vel + accent).clamp(0.1, 1.0))
        })
        .collect()
}

/// One chord per bar, struck at every `(step, length)`, optionally rolled
/// from the bottom up.
fn chords(ch: Id, prog: &[&[u8]], hits: &[(i64, i64)], vel: f32, strum: bool) -> Vec<Note> {
    let mut v = Vec::new();
    for (bar, keys) in prog.iter().enumerate() {
        for &(step, len) in hits {
            for (i, &k) in keys.iter().enumerate() {
                let roll = if strum {
                    Time::new(i as i64, 64)
                } else {
                    Time::ZERO
                };
                let start = Time::new(bar as i64 * 16 + step, 4) + roll;
                v.push(Note::new(ch, start, Time::new(len, 4), k, vel));
            }
        }
    }
    v
}

/// One root per bar, played at every `(step, length, semitones up)`.
fn bassline(ch: Id, roots: &[u8], hits: &[(i64, i64, u8)], vel: f32) -> Vec<Note> {
    let mut v = Vec::new();
    for (bar, &root) in roots.iter().enumerate() {
        for &(step, len, up) in hits {
            let accent = if step == 0 { 0.08 } else { 0.0 };
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

/// Chord tones and the octave above the root, cycled upward every
/// `every` sixteenths.
fn arp(ch: Id, prog: &[&[u8]], every: i64, vel: f32) -> Vec<Note> {
    let mut v = Vec::new();
    for (bar, keys) in prog.iter().enumerate() {
        let mut tones = keys.to_vec();
        tones.push(keys[0] + 12);
        for i in 0..16 / every {
            let accent = if i == 0 { 0.1 } else { 0.0 };
            let key = tones[i as usize % tones.len()];
            v.push(n16(
                ch,
                bar as i64 * 16 + i * every,
                every,
                key,
                vel + accent,
            ));
        }
    }
    v
}

/// Rap at 92 BPM in G minor. Sparse verse drums, a sub bass and a felt
/// piano leave the middle open for a vocal; the hook fills out with a
/// clap, a pad and a kalimba riff.
pub fn rap() -> Project {
    let mut s = Song::new(92.0, 0.22, &["Drums", "Bass", "Keys", "Pad", "Hook"]);
    let kick = s.part("Kick", 0, "peel.drum.kick", "Punch", HIT, -6.0);
    let snare = s.part("Snare", 9, "peel.drum.snare", "Crack", HIT, -9.0);
    let clap = s.part("Clap", 1, "peel.drum.clap", "Dry", HIT, -12.0);
    let hat = s.part("Hat", 2, "peel.drum.hat", "Closed", HIT, -15.0);
    let bass = s.part("Sub", 6, "peel.synth.bass", "Sub", 31, -6.0);
    let piano = s.part("Piano", 4, "peel.keys.grand", "Felt", 55, -4.0);
    let pad = s.part("Pad", 5, "peel.synth.poly", "Dark Pad", 55, -12.0);
    let bell = s.part("Kalimba", 3, "peel.perc.mallet", "Kalimba", 67, -10.0);
    s.fx(piano, "peel.fx.reverb", "Room");
    s.fx(pad, "peel.fx.reverb", "Warm Hall");
    s.fx(bell, "peel.fx.delay", "Eighth Note");

    // Gm - Eb - Cm - D
    let prog: [&[u8]; 4] = [&[55, 58, 62], &[55, 58, 63], &[55, 60, 63], &[54, 57, 62]];
    let roots = [31, 27, 36, 38];

    let mut verse = grid(kick, "X.........x.....|X......x..x.....");
    verse.extend(grid(snare, "....X.......X...|....X.......X..o"));
    verse.extend(grid(hat, "x...x.o.x...x.o.|x...x.o.x...x.ox"));
    let verse = s.pattern("Verse drums", 0, 2, verse);

    let mut hook = grid(kick, "X.....x...X.....|X.....x...x..x..");
    hook.extend(grid(snare, "....X.......X...|....X.......X..."));
    hook.extend(grid(clap, "....x.......x...|....x.......x..x"));
    hook.extend(grid(hat, "x.x.x.xox.x.x.xo|x.x.x.xox.x.xxxx"));
    let hook = s.pattern("Hook drums", 1, 2, hook);

    let sub = s.pattern(
        "Verse sub",
        6,
        4,
        bassline(bass, &roots, &[(0, 8, 0), (10, 4, 0)], 0.85),
    );
    let hook_sub = s.pattern(
        "Hook sub",
        6,
        4,
        bassline(
            bass,
            &roots,
            &[(0, 6, 0), (6, 2, 0), (10, 3, 0), (14, 2, 12)],
            0.85,
        ),
    );

    // A chord on the one and a single answering note: room for words.
    let mut keys = chords(piano, &prog, &[(0, 6)], 0.75, true);
    keys.extend(line(
        piano,
        &[(10, 4, 70), (26, 4, 70), (42, 4, 67), (58, 4, 69)],
        0.65,
    ));
    let keys = s.pattern("Piano", 4, 4, keys);
    let pads = s.pattern("Pad", 5, 4, chords(pad, &prog, &[(0, 16)], 0.5, false));
    let riff = s.pattern(
        "Hook riff",
        3,
        4,
        line(
            bell,
            &[
                (0, 2, 74),
                (3, 1, 72),
                (4, 2, 70),
                (8, 4, 67),
                (12, 2, 70),
                (16, 2, 70),
                (19, 1, 72),
                (20, 2, 74),
                (24, 6, 75),
                (32, 2, 75),
                (35, 1, 74),
                (36, 2, 72),
                (40, 4, 67),
                (44, 2, 70),
                (48, 2, 69),
                (52, 2, 66),
                (56, 8, 62),
            ],
            0.7,
        ),
    );

    s.place(&[
        (2, keys, 0, 32),
        (0, verse, 4, 16),
        (1, sub, 4, 16),
        (0, hook, 20, 8),
        (1, hook_sub, 20, 8),
        (3, pads, 20, 12),
        (4, riff, 20, 8),
    ]);
    s.sections(&[(0, "Intro"), (4, "Verse"), (20, "Hook"), (28, "Outro")])
}

/// Trap at 140 BPM in C minor, felt in half time: the snare lands on beat
/// three, hats roll in sixteenth triplets and thirty-seconds, and an 808
/// carries the low end under a bell melody and dark strings.
pub fn trap() -> Project {
    let mut s = Song::new(140.0, 0.0, &["Drums", "808", "Bells", "Strings"]);
    let kick = s.part("Kick", 0, "peel.drum.kick", "909 Kick", HIT, -9.0);
    let snare = s.part("Snare", 9, "peel.drum.snare", "808 Snare", HIT, -9.0);
    let clap = s.part("Clap", 1, "peel.drum.clap", "Snappy", HIT, -13.0);
    let hat = s.part("Hat", 2, "peel.drum.hat", "808 Closed", HIT, -15.0);
    let open = s.part("Open hat", 3, "peel.drum.hat", "Open", HIT, -18.0);
    let bass = s.part("808", 6, "peel.synth.bass", "808", 36, -10.0);
    let bell = s.part("Bells", 5, "peel.synth.poly", "Glass Bell", 72, -8.0);
    let strings = s.part(
        "Strings",
        4,
        "peel.strings.violin",
        "Cinematic Low Strings",
        48,
        -7.0,
    );
    s.fx(bell, "peel.fx.delay", "Dotted Ping-Pong");
    s.fx(bell, "peel.fx.reverb", "Plate");
    s.fx(strings, "peel.fx.reverb", "Warm Hall");

    // Cm - Ab - Fm - G
    let prog: [&[u8]; 4] = [&[48, 51, 55], &[48, 51, 56], &[48, 53, 56], &[47, 50, 55]];
    let roots = [36, 32, 29, 31];
    let sixteenth = Time::new(1, 4);
    let triplet = Time::new(1, 6);
    let thirty_second = Time::new(1, 8);

    let mut hook = grid(kick, "X......x..X.....|X.........x..x..");
    hook.extend(grid(snare, "........X.......|........X......."));
    hook.extend(grid(clap, "........x.......|........x......."));
    hook.extend(grid(hat, "x.x.x.x.x.x.....|x.x.x...x......."));
    hook.extend(grid(open, "..............x.|................"));
    hook.extend(roll(hat, HIT, Time::beats(3), sixteenth, 4, (0.5, 0.8)));
    hook.extend(roll(
        hat,
        HIT,
        Time::new(13, 2),
        thirty_second,
        4,
        (0.4, 0.7),
    ));
    hook.extend(roll(hat, HIT, Time::beats(7), triplet, 6, (0.45, 0.9)));
    let hook = s.pattern("Hook drums", 1, 2, hook);

    let mut verse = grid(kick, "X.........X.....|X..X......x.....");
    verse.extend(grid(snare, "........X.......|........X......o"));
    verse.extend(grid(hat, "x.x.x.x.x.x.x.x.|x.x.x.x.x.x....."));
    verse.extend(roll(hat, HIT, Time::beats(7), triplet, 6, (0.4, 0.75)));
    let verse = s.pattern("Verse drums", 0, 2, verse);

    // The 808 follows the kick and jumps an octave at the end of each bar.
    let hook_808 = s.pattern(
        "Hook 808",
        6,
        4,
        bassline(
            bass,
            &roots,
            &[(0, 6, 0), (7, 3, 0), (10, 4, 0), (14, 2, 12)],
            0.9,
        ),
    );
    let verse_808 = s.pattern(
        "Verse 808",
        6,
        4,
        bassline(bass, &roots, &[(0, 10, 0), (10, 6, 0)], 0.85),
    );

    // A 3-3-2 bell figure over each chord.
    let tones: [[u8; 3]; 4] = [[79, 75, 72], [80, 75, 72], [80, 77, 72], [79, 74, 71]];
    let mut melody = Vec::new();
    for (bar, t) in tones.iter().enumerate() {
        let at = bar as i64 * 16;
        for (step, i) in [(0, 0), (3, 1), (6, 2), (8, 0), (11, 1), (14, 2)] {
            melody.push(n16(
                bell,
                at + step,
                2,
                t[i],
                if step == 0 { 0.75 } else { 0.6 },
            ));
        }
    }
    let bells = s.pattern("Bells", 5, 4, melody);
    let pads = s.pattern(
        "Strings",
        4,
        4,
        chords(strings, &prog, &[(0, 16)], 0.55, false),
    );

    s.place(&[
        (2, bells, 0, 12),
        (3, pads, 0, 40),
        (0, hook, 4, 8),
        (1, hook_808, 4, 8),
        (0, verse, 12, 16),
        (1, verse_808, 12, 16),
        (0, hook, 28, 8),
        (1, hook_808, 28, 8),
        (2, bells, 28, 12),
    ]);
    s.sections(&[
        (0, "Intro"),
        (4, "Hook"),
        (12, "Verse"),
        (28, "Hook"),
        (36, "Outro"),
    ])
}

/// Boom-bap hip-hop at 88 BPM in D minor with a heavy swing: a lazy
/// kick, cracking backbeat snare, jazzy electric piano, a fingered bass
/// and an alto sax line for the hook.
pub fn hip_hop() -> Project {
    let mut s = Song::new(88.0, 0.45, &["Drums", "Bass", "Keys", "Horn"]);
    let kick = s.part("Kick", 0, "peel.drum.kick", "Deep", HIT, -7.0);
    let snare = s.part("Snare", 9, "peel.drum.snare", "Fat", HIT, -9.0);
    let hat = s.part("Hat", 2, "peel.drum.hat", "Dark", HIT, -12.0);
    let bass = s.part("Bass", 6, "peel.synth.bass", "Fingered", 38, -7.0);
    let keys = s.part("Keys", 4, "peel.keys.tine", "Mellow", 50, -6.0);
    let horn = s.part("Sax", 5, "peel.wind.sax", "Smooth Alto", 69, -9.0);
    s.fx(keys, "peel.fx.chorus", "Subtle Width");
    s.fx(keys, "peel.fx.reverb", "Room");
    s.fx(horn, "peel.fx.reverb", "Plate");

    // Dm9 - G13 - Cmaj9 - A7(b9)
    let prog: [&[u8]; 4] = [
        &[50, 53, 57, 60, 64],
        &[43, 53, 59, 64],
        &[48, 52, 55, 59, 62],
        &[45, 55, 58, 61],
    ];
    let roots = [38, 43, 36, 45];

    let mut beat = grid(kick, "X......x..X.....|X.x.......X..x..");
    beat.extend(grid(snare, "....X.......X...|....X.......X..o"));
    beat.extend(grid(hat, "x.o.x.o.x.o.x.o.|x.o.x.o.x.o.x.o."));
    let beat = s.pattern("Boom bap", 0, 2, beat);

    let mut hook = grid(kick, "X......x.xX.....|X.x....x..X..x..");
    hook.extend(grid(snare, "....X.......X...|....X..o....X.o."));
    hook.extend(grid(hat, "x.xox.xox.xox.xo|x.xox.xox.xox.xx"));
    let hook = s.pattern("Hook beat", 1, 2, hook);

    let bass_pat = s.pattern(
        "Bass",
        6,
        4,
        bassline(bass, &roots, &[(0, 7, 0), (10, 4, 0), (14, 2, 7)], 0.85),
    );
    let keys_pat = s.pattern(
        "Keys",
        4,
        4,
        chords(keys, &prog, &[(0, 10), (11, 4)], 0.6, true),
    );
    let horn_line = s.pattern(
        "Sax hook",
        5,
        4,
        line(
            horn,
            &[
                (0, 3, 69),
                (4, 2, 72),
                (6, 4, 74),
                (12, 4, 72),
                (16, 3, 71),
                (20, 2, 69),
                (22, 6, 67),
                (32, 3, 67),
                (36, 2, 69),
                (38, 4, 71),
                (44, 4, 72),
                (48, 4, 73),
                (52, 4, 70),
                (56, 8, 69),
            ],
            0.7,
        ),
    );

    s.place(&[
        (2, keys_pat, 0, 40),
        (0, beat, 4, 8),
        (1, bass_pat, 4, 32),
        (0, hook, 12, 8),
        (3, horn_line, 12, 8),
        (0, beat, 20, 8),
        (0, hook, 28, 8),
        (3, horn_line, 28, 12),
    ]);
    s.sections(&[
        (0, "Intro"),
        (4, "Verse"),
        (12, "Hook"),
        (20, "Verse"),
        (28, "Hook"),
        (36, "Outro"),
    ])
}

/// Synthwave at 100 BPM in E minor: a steady rock beat with a big snare
/// and tom fills, driving eighth-note bass, a sixteenth arpeggio, a warm
/// chorused pad and a sync lead for the chorus.
pub fn synthwave() -> Project {
    let mut s = Song::new(100.0, 0.0, &["Drums", "Bass", "Arp", "Pad", "Lead"]);
    let kick = s.part("Kick", 0, "peel.drum.kick", "909 Kick", HIT, -7.0);
    let snare = s.part("Snare", 9, "peel.drum.snare", "909 Snare", HIT, -10.0);
    let hat = s.part("Hat", 2, "peel.drum.hat", "808 Closed", HIT, -15.0);
    let tom = s.part("Toms", 3, "peel.drum.tom", "909 Tom", HIT, -12.0);
    let bass = s.part("Bass", 6, "peel.synth.bass", "Synthwave", 40, -9.0);
    let arp_ch = s.part("Arp", 7, "peel.synth.poly", "Arp", 64, -7.0);
    let pad = s.part("Pad", 5, "peel.synth.poly", "Warm Pad", 52, -7.0);
    let lead = s.part("Lead", 4, "peel.synth.cross", "Sync Lead", 71, -12.0);
    s.fx(snare, "peel.fx.reverb", "Big Hall");
    s.fx(arp_ch, "peel.fx.delay", "Dotted Ping-Pong");
    s.fx(pad, "peel.fx.vintage_chorus", "Mode II");
    s.fx(pad, "peel.fx.reverb", "Warm Hall");
    s.fx(lead, "peel.fx.delay", "Eighth Note");

    // Em - C - G - D
    let prog: [&[u8]; 4] = [&[52, 55, 59], &[52, 55, 60], &[50, 55, 59], &[50, 54, 57]];
    let roots = [40, 36, 43, 38];
    let high: Vec<Vec<u8>> = prog
        .iter()
        .map(|c| c.iter().map(|k| k + 12).collect())
        .collect();
    let high: Vec<&[u8]> = high.iter().map(|c| c.as_slice()).collect();

    let groove = "X.......X.x.....|X.......X.x.....|X.......X.x.....|X.......X.x.....";
    let mut drums = grid(kick, groove);
    drums.extend(grid(
        snare,
        "....X.......X...|....X.......X...|....X.......X...|....X.......X...",
    ));
    drums.extend(grid(
        hat,
        "x.x.x.x.x.x.x.x.|x.x.x.x.x.x.x.x.|x.x.x.x.x.x.x.x.|x.x.x.x.x.x.....",
    ));
    for (i, key) in [62u8, 60, 59, 58].into_iter().enumerate() {
        drums.push(n16(tom, 60 + i as i64, 1, key, 0.7 + 0.08 * i as f32));
    }
    let drums = s.pattern("Drums", 0, 4, drums);

    let eighths: Vec<(i64, i64, u8)> = (0..8)
        .map(|i| (i * 2, 2, if i == 7 { 12 } else { 0 }))
        .collect();
    let bass_pat = s.pattern("Bass", 6, 4, bassline(bass, &roots, &eighths, 0.8));
    let arp_pat = s.pattern("Arp", 7, 4, arp(arp_ch, &high, 1, 0.55));
    let pad_pat = s.pattern("Pad", 5, 4, chords(pad, &prog, &[(0, 16)], 0.5, false));
    let lead_pat = s.pattern(
        "Lead",
        4,
        4,
        line(
            lead,
            &[
                (0, 6, 71),
                (6, 2, 74),
                (8, 8, 76),
                (16, 4, 72),
                (20, 4, 71),
                (24, 8, 67),
                (32, 6, 71),
                (38, 2, 74),
                (40, 6, 79),
                (46, 2, 78),
                (48, 8, 76),
                (56, 8, 74),
            ],
            0.7,
        ),
    );

    s.place(&[
        (2, arp_pat, 0, 36),
        (3, pad_pat, 0, 4),
        (0, drums, 4, 16),
        (1, bass_pat, 4, 16),
        (3, pad_pat, 12, 24),
        (4, lead_pat, 12, 8),
        (0, drums, 24, 8),
        (1, bass_pat, 24, 8),
        (4, lead_pat, 24, 8),
    ]);
    s.sections(&[
        (0, "Intro"),
        (4, "Verse"),
        (12, "Chorus"),
        (20, "Break"),
        (24, "Chorus"),
        (32, "Outro"),
    ])
}

/// EDM at 128 BPM in F minor: four-on-the-floor, a pluck intro, a
/// breakdown on supersaw chords, a snare-roll build with a noise riser,
/// and a drop where the bass and chords pump under the kick beneath a
/// big-room lead.
pub fn edm() -> Project {
    let mut s = Song::new(128.0, 0.0, &["Drums", "Bass", "Chords", "Lead", "Pluck"]);
    let kick = s.part("Kick", 0, "peel.drum.kick", "Hard Kick", HIT, -7.0);
    let clap = s.part("Clap", 1, "peel.drum.clap", "EDM", HIT, -11.0);
    let hat = s.part("Hat", 2, "peel.drum.hat", "909 Closed", HIT, -17.0);
    let open = s.part("Open hat", 3, "peel.drum.hat", "Open", HIT, -16.0);
    let snare = s.part("Snare roll", 9, "peel.drum.snare", "909 Snare", HIT, -6.0);
    let riser = s.part("Riser", 8, "peel.synth.poly", "Noise Riser", 60, -9.0);
    let bass = s.part("Bass", 6, "peel.synth.bass", "Sub", 29, -8.0);
    let saw = s.part("Supersaw", 5, "peel.synth.poly", "Supersaw", 56, -7.0);
    let lead = s.part("Lead", 4, "peel.synth.poly", "Big Room Lead", 72, -13.0);
    let pluck = s.part("Pluck", 7, "peel.synth.poly", "Pluck", 68, -6.0);
    s.pump(bass, kick, "Sidechain Duck");
    s.pump(saw, kick, "EDM Pump");
    s.fx(saw, "peel.fx.reverb", "Big Hall");
    s.fx(lead, "peel.fx.reverb", "Big Hall");
    s.fx(pluck, "peel.fx.delay", "Eighth Note");

    // Fm - Db - Ab - Eb
    let prog: [&[u8]; 4] = [&[56, 60, 65], &[56, 61, 65], &[56, 60, 63], &[55, 58, 63]];
    let roots = [29, 25, 32, 27];
    let pluck_prog: [&[u8]; 4] = [&[65, 68, 72], &[65, 68, 73], &[63, 68, 72], &[63, 67, 70]];

    let mut intro = grid(kick, "X...X...X...X...");
    intro.extend(grid(hat, "..x...x...x...x."));
    let intro = s.pattern("Intro beat", 0, 1, intro);

    let mut drop = grid(kick, "X...X...X...X...");
    drop.extend(grid(clap, "....X.......X..."));
    drop.extend(grid(open, "..x...x...x...x."));
    drop.extend(grid(hat, ".o.o.o.o.o.o.o.o"));
    let drop = s.pattern("Drop beat", 1, 1, drop);

    // The roll doubles each bar while the riser climbs underneath.
    let mut build = Vec::new();
    for (bar, every) in [
        (0, Time::beats(1)),
        (1, Time::new(1, 2)),
        (2, Time::new(1, 4)),
        (3, Time::new(1, 8)),
    ] {
        let count = (Time::beats(BAR).as_f64() / every.as_f64()) as i64;
        let lo = 0.35 + 0.15 * bar as f32;
        build.extend(roll(
            snare,
            HIT,
            Time::beats(bar * BAR),
            every,
            count,
            (lo, lo + 0.15),
        ));
    }
    build.push(Note::new(riser, Time::ZERO, Time::beats(4 * BAR), 60, 0.8));
    let build = s.pattern("Build", 9, 4, build);

    let offbeat = [(2, 2, 0), (6, 2, 0), (10, 2, 0), (14, 2, 12)];
    let bass_pat = s.pattern("Drop bass", 6, 4, bassline(bass, &roots, &offbeat, 0.85));
    let saws = s.pattern("Chords", 5, 4, chords(saw, &prog, &[(0, 16)], 0.6, false));
    let pluck_pat = s.pattern("Pluck", 7, 4, arp(pluck, &pluck_prog, 2, 0.6));
    let lead_pat = s.pattern(
        "Drop lead",
        4,
        4,
        line(
            lead,
            &[
                (0, 2, 77),
                (2, 2, 77),
                (4, 2, 80),
                (6, 2, 77),
                (8, 4, 84),
                (12, 2, 82),
                (14, 2, 80),
                (16, 2, 77),
                (18, 2, 77),
                (20, 2, 80),
                (22, 2, 77),
                (24, 8, 73),
                (32, 2, 75),
                (34, 2, 75),
                (36, 2, 79),
                (38, 2, 75),
                (40, 4, 84),
                (44, 2, 82),
                (46, 2, 79),
                (48, 4, 79),
                (52, 4, 77),
                (56, 8, 75),
            ],
            0.75,
        ),
    );

    s.place(&[
        (0, intro, 0, 8),
        (4, pluck_pat, 4, 12),
        (2, saws, 8, 20),
        (0, build, 16, 4),
        (0, drop, 20, 8),
        (1, bass_pat, 20, 8),
        (3, lead_pat, 20, 8),
        (0, intro, 28, 4),
        (4, pluck_pat, 28, 4),
    ]);
    s.sections(&[
        (0, "Intro"),
        (8, "Breakdown"),
        (16, "Build"),
        (20, "Drop"),
        (28, "Outro"),
    ])
}

/// Rock / metal at 108 BPM in E minor. The chorus rides a double kick in
/// sixteenth triplets under ringing power chords; the verse is a simpler
/// rock beat with a palm-muted riff, and the breakdown locks the kick to
/// half-time chugs.
pub fn rock_metal() -> Project {
    let mut s = Song::new(108.0, 0.0, &["Drums", "Bass", "Rhythm", "Chug"]);
    let kick = s.part("Kick", 0, "peel.drum.acoustic", "Kick", HIT, -2.0);
    let snare = s.part("Snare", 9, "peel.drum.acoustic", "Snare", HIT, -4.0);
    let hat = s.part("Hat", 2, "peel.drum.acoustic", "Closed Hat", HIT, -11.0);
    let crash = s.part("Crash", 3, "peel.drum.acoustic", "Crash", HIT, -15.0);
    let tom = s.part("Toms", 8, "peel.drum.acoustic", "Floor Tom", HIT, -7.0);
    let bass = s.part("Bass", 6, "peel.bass.recorded", "Rock Pick", 28, -3.0);
    let gtr = s.part("Guitar", 4, "peel.guitar.recorded", "High Gain", 40, -14.0);
    let chug = s.part("Chug", 5, "peel.guitar.recorded", "Palm Mute", 40, -12.0);
    s.fx(snare, "peel.fx.reverb", "Drum Room");
    s.fx(gtr, "peel.fx.reverb", "Tight Ambience");

    // E5 - C5 - A5 - B5 as root, fifth and octave.
    let prog: [&[u8]; 4] = [&[40, 47, 52], &[48, 55, 60], &[45, 52, 57], &[47, 54, 59]];
    let roots = [28, 36, 33, 35];

    // The starting groove: six sixteenth-triplet kicks on beats one and
    // three, snare on two and four, a hat on every beat.
    let triplet = Time::new(1, 6);
    let mut double = roll(kick, HIT, Time::ZERO, triplet, 6, (0.95, 0.8));
    double.extend(roll(kick, HIT, Time::beats(2), triplet, 6, (0.95, 0.8)));
    double.extend(grid(snare, "....X.......X..."));
    double.extend(grid(hat, "x...x...x...x..."));
    let double = s.pattern("Double kick", 0, 1, double);

    let rock = "X.....x.X.x.....|X.....x.X.......";
    let mut beat = grid(kick, rock);
    beat.extend(grid(snare, "....X.......X...|....X.......X..."));
    beat.extend(grid(hat, "x.x.x.x.x.x.x.x.|x.x.x.x.x.x.x.x."));
    let beat = s.pattern("Rock beat", 1, 2, beat);

    // The rock beat with a floor-tom run into the chorus.
    let mut fill = grid(kick, "X.....x.X.x.....|X.....x.........");
    fill.extend(grid(snare, "....X.......X...|....X...x.x.X.XX"));
    fill.extend(grid(hat, "x.x.x.x.x.x.x.x.|x.x.x.x........."));
    for (i, key) in [67u8, 64, 60, 55].into_iter().enumerate() {
        fill.push(n16(tom, 25 + 2 * i as i64, 1, key, 0.7 + 0.06 * i as f32));
    }
    let fill = s.pattern("Rock fill", 1, 2, fill);

    // Half time: the kick doubles the chugs, a crash marks each bar.
    let chugs: [(i64, i64, u8); 12] = [
        (0, 2, 40),
        (3, 1, 40),
        (6, 2, 40),
        (10, 1, 40),
        (11, 1, 40),
        (14, 2, 46),
        (16, 3, 40),
        (19, 1, 40),
        (22, 2, 40),
        (24, 1, 40),
        (26, 2, 43),
        (28, 4, 40),
    ];
    let mut down = Vec::new();
    for &(step, _, _) in &chugs {
        down.push(n16(
            kick,
            step,
            1,
            HIT,
            if step % 4 == 0 { 0.95 } else { 0.8 },
        ));
    }
    down.extend(grid(snare, "........X.......|........X......."));
    down.extend(grid(crash, "x...............|x..............."));
    down.extend(grid(hat, "....x.......x...|....x.......x..."));
    let down = s.pattern("Breakdown beat", 9, 2, down);
    let down_gtr = s.pattern("Breakdown chug", 5, 2, line(chug, &chugs, 0.85));
    let low: Vec<(i64, i64, u8)> = chugs.iter().map(|&(s, l, k)| (s, l, k - 12)).collect();
    let down_bass = s.pattern("Breakdown bass", 6, 2, line(bass, &low, 0.85));

    // Verse riff: open-string chugs answered by G, A and a flat fifth.
    let riff: [(i64, i64, u8); 21] = [
        (0, 1, 40),
        (1, 1, 40),
        (2, 2, 40),
        (4, 1, 40),
        (5, 1, 40),
        (6, 2, 43),
        (8, 1, 40),
        (9, 1, 40),
        (10, 2, 40),
        (12, 2, 45),
        (14, 2, 43),
        (16, 1, 40),
        (17, 1, 40),
        (18, 2, 40),
        (20, 1, 40),
        (21, 1, 40),
        (22, 2, 46),
        (24, 2, 45),
        (26, 2, 43),
        (28, 2, 40),
        (30, 2, 38),
    ];
    let verse_gtr = s.pattern("Verse riff", 5, 2, line(chug, &riff, 0.8));
    let verse_bass: Vec<(i64, i64, u8)> = riff
        .iter()
        .filter(|&&(s, _, _)| s % 2 == 0)
        .map(|&(s, l, k)| (s, l.max(2), k - 12))
        .collect();
    let verse_bass = s.pattern("Verse bass", 6, 2, line(bass, &verse_bass, 0.85));

    // Chorus chords pushed in a 3-3-2 figure; the bass pumps eighths.
    let chorus_gtr = s.pattern(
        "Chorus chords",
        4,
        4,
        chords(
            gtr,
            &prog,
            &[(0, 3), (3, 3), (6, 2), (8, 4), (12, 2), (14, 2)],
            0.8,
            false,
        ),
    );
    let eighths: Vec<(i64, i64, u8)> = (0..8).map(|i| (i * 2, 2, 0)).collect();
    let chorus_bass = s.pattern("Chorus bass", 6, 4, bassline(bass, &roots, &eighths, 0.8));
    let ring = s.pattern(
        "Ringing chords",
        4,
        4,
        chords(gtr, &prog, &[(0, 16)], 0.75, false),
    );
    let ring_bass = s.pattern(
        "Long bass",
        6,
        4,
        bassline(bass, &roots, &[(0, 16, 0)], 0.75),
    );

    s.place(&[
        (2, ring, 0, 4),
        (1, ring_bass, 0, 4),
        (0, beat, 2, 2),
        (0, beat, 4, 6),
        (0, fill, 10, 2),
        (1, verse_bass, 4, 8),
        (3, verse_gtr, 4, 8),
        (0, double, 12, 8),
        (1, chorus_bass, 12, 8),
        (2, chorus_gtr, 12, 8),
        (0, down, 20, 8),
        (1, down_bass, 20, 8),
        (3, down_gtr, 20, 8),
        (0, double, 28, 3),
        (1, ring_bass, 28, 4),
        (2, ring, 28, 4),
    ]);
    s.sections(&[
        (0, "Intro"),
        (4, "Verse"),
        (12, "Chorus"),
        (20, "Breakdown"),
        (28, "Outro"),
    ])
}
