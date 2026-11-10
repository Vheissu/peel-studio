//! Per-note expression (MPE members and recorded pattern curves): each
//! runtime voice ID bends, presses and changes timbre on its own, even when
//! two voices play the same key.

mod common;

use common::*;
use peel_dsp::EventKind;

/// Instruments that can sound two voices on one key and hold a pitch.
/// (Zither re-strikes the key's own string, so it is not here.)
const POLY: [&str; 8] = [
    "peel.synth.poly",
    "peel.keys.grand",
    "peel.keys.tine",
    "peel.guitar.electric",
    "peel.sampler",
    "peel.strings.violin",
    "peel.perc.mallet",
    "peel.synth.cross",
];

/// Everything with a held, measurable pitch.
const PITCHED: [&str; 12] = [
    "peel.synth.poly",
    "peel.synth.bass",
    "peel.keys.grand",
    "peel.keys.tine",
    "peel.guitar.electric",
    "peel.sampler",
    "peel.strings.violin",
    "peel.wind.sax",
    "peel.perc.mallet",
    "peel.strings.zither",
    "peel.synth.cross",
    "peel.bass.808",
];

const KEY: u8 = 57;

fn vx(at: usize, voice: u32, pitch: f32, pressure: f32, timbre: f32) -> Timed {
    (
        at,
        EventKind::VoiceExpression {
            voice,
            pitch,
            pressure,
            timbre,
        },
    )
}

/// A bright source for the sampler so timbre is measurable: a 220 Hz saw.
fn saw() -> std::sync::Arc<peel_dsp::SampleBuffer> {
    let sr = 44_100.0f32;
    let left: Vec<f32> = (0..44_100)
        .map(|i| 0.5 * ((220.0 * i as f32 / sr).fract() * 2.0 - 1.0))
        .collect();
    let mut s = peel_dsp::SampleBuffer::new("saw", sr, vec![left.clone(), left]);
    s.root_key = 57;
    std::sync::Arc::new(s)
}

fn dev(id: &str) -> Box<dyn peel_dsp::Device> {
    let mut d = fresh(id);
    if id == "peel.sampler" {
        d.set_sample(Some(saw()));
    }
    d
}

fn run(id: &str, events: &[Timed], total: usize, block: usize) -> Vec<f32> {
    let mut dev = dev(id);
    let mut ev = events.to_vec();
    // Stable: events at one sample keep the order they were written in.
    ev.sort_by_key(|e| e.0);
    render(dev.as_mut(), &ev, total, block).0
}

fn rms(b: &[f32]) -> f32 {
    (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt()
}

/// Treble proxy: RMS of the first difference relative to the RMS.
fn brightness(b: &[f32]) -> f32 {
    let d: Vec<f32> = b.windows(2).map(|w| w[1] - w[0]).collect();
    rms(&d) / rms(b).max(1.0e-9)
}

fn treble(b: &[f32]) -> f32 {
    let d: Vec<f32> = b.windows(2).map(|w| w[1] - w[0]).collect();
    rms(&d)
}

/// Energy near `hz` (Goertzel, Hann window).
fn energy(b: &[f32], hz: f32) -> f32 {
    let n = b.len();
    let w = std::f32::consts::TAU * hz / SR;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for (i, &x) in b.iter().enumerate() {
        let win = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
        let s = x * win + c * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0)
}

/// Energy in a narrow band around `hz` (tolerates stretch and detune).
fn band(b: &[f32], hz: f32) -> f32 {
    [-0.012f32, -0.006, 0.0, 0.006, 0.012]
        .iter()
        .map(|d| energy(b, hz * (1.0 + d)))
        .fold(0.0, f32::max)
}

fn hz(key: u8) -> f32 {
    440.0 * 2f32.powf((key as f32 - 69.0) / 12.0)
}

#[test]
fn same_key_voices_bend_independently() {
    let up = 2f32.powf(7.0 / 12.0);
    for id in POLY {
        let two = [note_on(0, KEY, 0.8, 1), note_on(0, KEY, 0.8, 2)];
        let mut scoped = two.to_vec();
        scoped.push(vx(2_000, 2, 7.0, 0.0, 0.5));
        let mut global = two.to_vec();
        global.push((2_000, EventKind::PitchBend { semitones: 7.0 }));
        let a = run(id, &scoped, 30_000, 256);
        let g = run(id, &global, 30_000, 256);
        let (wa, wg) = (&a[18_000..26_192], &g[18_000..26_192]);
        let f0 = hz(KEY);
        // Voice 1 still sounds at the played pitch; the shared bend moved it.
        assert!(
            band(wa, f0) > 6.0 * band(wg, f0),
            "{id}: unbent voice moved"
        );
        // Voice 2 sounds a fifth up.
        assert!(
            band(wa, f0 * up) > 0.1 * band(wg, f0 * up),
            "{id}: bent voice missing"
        );
        assert!(
            band(wa, f0 * up) > 6.0 * band(&run(id, &two, 30_000, 256)[18_000..26_192], f0 * up),
            "{id}"
        );
    }
}

#[test]
fn same_key_voices_press_and_change_timbre_independently() {
    for id in POLY {
        let two = [note_on(0, KEY, 0.7, 1), note_on(0, KEY, 0.7, 2)];
        let with = |extra: &[Timed]| {
            let mut ev = two.to_vec();
            ev.extend_from_slice(extra);
            run(id, &ev, 16_000, 256)
        };
        let w = 6_000..14_000;
        let none = with(&[]);
        let one = with(&[vx(1_000, 2, 0.0, 1.0, 0.5)]);
        let both = with(&[vx(1_000, 1, 0.0, 1.0, 0.5), vx(1_000, 2, 0.0, 1.0, 0.5)]);
        let (n, o, b) = (
            rms(&none[w.clone()]),
            rms(&one[w.clone()]),
            rms(&both[w.clone()]),
        );
        assert!(o > n * 1.03 && o < b * 0.97, "{id}: pressure {n} {o} {b}");

        // From the start, so modal instruments still have their overtones.
        let w = 200..6_000;
        let dark = with(&[vx(0, 1, 0.0, 0.0, 0.0), vx(0, 2, 0.0, 0.0, 0.0)]);
        let one_bright = with(&[vx(0, 1, 0.0, 0.0, 0.0), vx(0, 2, 0.0, 0.0, 1.0)]);
        let bright = with(&[vx(0, 1, 0.0, 0.0, 1.0), vx(0, 2, 0.0, 0.0, 1.0)]);
        // Treble energy, not a ratio: the two voices are coherent, so a
        // normalized measure would depend on how their fundamentals sum.
        let (d, m, t) = (
            treble(&dark[w.clone()]),
            treble(&one_bright[w.clone()]),
            treble(&bright[w.clone()]),
        );
        assert!(d < m && m < t, "{id}: timbre {d} {m} {t}");
    }
}

#[test]
fn every_family_hears_pressure_and_timbre() {
    for id in REQUIRED {
        let drum = id.starts_with("peel.drum");
        let perc = drum || id == "peel.perc.mallet";
        let (at, w) = if perc {
            (0, 300..4_000)
        } else {
            (500, 8_000..16_000)
        };
        let base = run(id, &[note_on(0, KEY, 0.8, 4)], 18_000, 256);
        let pressed = run(
            id,
            &[note_on(0, KEY, 0.8, 4), vx(at, 4, 0.0, 1.0, 0.5)],
            18_000,
            256,
        );
        assert!(
            rms(&pressed[w.clone()]) > rms(&base[w.clone()]) * 1.1,
            "{id}: pressure"
        );
        let dark = run(
            id,
            &[note_on(0, KEY, 0.8, 4), vx(at, 4, 0.0, 0.0, 0.0)],
            18_000,
            256,
        );
        let bright = run(
            id,
            &[note_on(0, KEY, 0.8, 4), vx(at, 4, 0.0, 0.0, 1.0)],
            18_000,
            256,
        );
        // A drum's or mallet's tone lives in its strike.
        let w = if perc { 0..1_500 } else { w };
        let (d, b) = (brightness(&dark[w.clone()]), brightness(&bright[w]));
        assert!(b > d * 1.03, "{id}: timbre {d} vs {b}");
    }
}

#[test]
fn per_note_pitch_retunes_held_voices_by_the_right_ratio() {
    for id in PITCHED {
        let plain = run(id, &[note_on(0, KEY, 0.8, 3)], 44_000, 256);
        let bent = run(
            id,
            &[note_on(0, KEY, 0.8, 3), vx(12_000, 3, 2.0, 0.0, 0.5)],
            44_000,
            256,
        );
        let ratio =
            detect_pitch(&bent[30_000..36_000], SR) / detect_pitch(&plain[30_000..36_000], SR);
        let want = 2f32.powf(2.0 / 12.0);
        assert!((ratio / want - 1.0).abs() < 0.02, "{id}: ratio {ratio}");
        assert_eq!(
            &plain[..11_900],
            &bent[..11_900],
            "{id}: moved before the event"
        );
    }
}

#[test]
fn snapshot_replaces_the_shared_bend_instead_of_adding() {
    // A recorded take carries the full pitch; a wheel left bent must not
    // add to it again.
    for id in PITCHED {
        let snap = [note_on(0, KEY, 0.8, 3), vx(0, 3, 3.0, 0.4, 0.6)];
        let mut both = snap.to_vec();
        both.push((0, EventKind::PitchBend { semitones: 3.0 }));
        both.push((0, EventKind::ChannelPressure { value: 1.0 }));
        both.push((6_000, EventKind::PitchBend { semitones: -12.0 }));
        assert_eq!(
            run(id, &snap, 20_000, 256),
            run(id, &both, 20_000, 256),
            "{id}"
        );
    }
}

#[test]
fn unscoped_voices_keep_following_the_shared_bend() {
    for id in ["peel.synth.poly", "peel.keys.grand", "peel.sampler"] {
        // Voice 1 scoped, voice 2 (another key) ordinary MIDI.
        let base = [
            note_on(0, 50, 0.8, 1),
            note_on(0, 64, 0.8, 2),
            vx(0, 1, 0.0, 0.0, 0.5),
        ];
        let mut bent = base.to_vec();
        bent.push((2_000, EventKind::PitchBend { semitones: 2.0 }));
        let a = run(id, &base, 30_000, 256);
        let b = run(id, &bent, 30_000, 256);
        let w = 18_000..26_192;
        let up = 2f32.powf(2.0 / 12.0);
        assert!(
            band(&b[w.clone()], hz(64) * up) > 6.0 * band(&a[w.clone()], hz(64) * up),
            "{id}: unscoped voice ignored bend"
        );
        assert!(
            band(&b[w.clone()], hz(50)) > 0.5 * band(&a[w.clone()], hz(50)),
            "{id}: scoped voice followed bend"
        );
    }
}

#[test]
fn snapshot_before_or_with_its_note_starts_the_note_there() {
    for id in REQUIRED {
        // Planned order: NoteOn then its snapshot at the same sample.
        let after = run(
            id,
            &[note_on(1_000, KEY, 0.8, 5), vx(1_000, 5, 5.0, 0.7, 0.8)],
            12_000,
            256,
        );
        let before = run(
            id,
            &[vx(1_000, 5, 5.0, 0.7, 0.8), note_on(1_000, KEY, 0.8, 5)],
            12_000,
            256,
        );
        let early = run(
            id,
            &[vx(100, 5, 5.0, 0.7, 0.8), note_on(1_000, KEY, 0.8, 5)],
            12_000,
            256,
        );
        assert_eq!(after, before, "{id}: same-sample order changed the note");
        assert_eq!(after, early, "{id}: early snapshot did not start the note");
    }
    // And the start really is bent: no glide up from the played pitch.
    for id in [
        "peel.synth.poly",
        "peel.keys.grand",
        "peel.sampler",
        "peel.keys.tine",
    ] {
        let snap = run(
            id,
            &[note_on(0, KEY, 0.8, 5), vx(0, 5, 5.0, 0.0, 0.5)],
            6_000,
            256,
        );
        let wheel = run(
            id,
            &[
                (0, EventKind::PitchBend { semitones: 5.0 }),
                note_on(0, KEY, 0.8, 5),
            ],
            6_000,
            256,
        );
        let plain = run(id, &[note_on(0, KEY, 0.8, 5)], 6_000, 256);
        let early_ratio =
            detect_pitch(&snap[200..4_200], SR) / detect_pitch(&plain[200..4_200], SR);
        assert!(
            (early_ratio / 2f32.powf(5.0 / 12.0) - 1.0).abs() < 0.02,
            "{id}: {early_ratio}"
        );
        let _ = wheel;
    }
}

#[test]
fn snapshot_for_a_sounding_note_moves_smoothly() {
    for id in [
        "peel.synth.poly",
        "peel.keys.grand",
        "peel.strings.violin",
        "peel.sampler",
    ] {
        let base = run(id, &[note_on(0, KEY, 0.8, 2)], 16_000, 256);
        let moved = run(
            id,
            &[note_on(0, KEY, 0.8, 2), vx(8_000, 2, 0.0, 1.0, 0.5)],
            16_000,
            256,
        );
        let diff: Vec<f32> = base.iter().zip(&moved).map(|(a, b)| b - a).collect();
        let first = diff[8_000..8_048]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()));
        let later = diff[9_500..10_500]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(
            first < later * 0.5 + 1.0e-6,
            "{id}: step {first} vs {later}"
        );
    }
}

#[test]
fn neutral_snapshots_are_bit_exact() {
    for id in REQUIRED {
        let plain = run(
            id,
            &[
                note_on(0, 55, 0.8, 1),
                note_on(400, 62, 0.6, 2),
                note_off(9_000, 55, 1),
            ],
            20_000,
            97,
        );
        let neutral = run(
            id,
            &[
                vx(0, 1, 0.0, 0.0, 0.5),
                note_on(0, 55, 0.8, 1),
                note_on(400, 62, 0.6, 2),
                vx(400, 2, 0.0, 0.0, 0.5),
                vx(3_000, 1, 0.0, 0.0, 0.5),
                note_off(9_000, 55, 1),
                vx(10_000, 1, 0.0, 0.0, 0.5),
                vx(11_000, 99, 0.0, 0.0, 0.5),
            ],
            20_000,
            97,
        );
        assert_eq!(plain, neutral, "{id}");
    }
}

#[test]
fn released_tails_keep_their_own_snapshot() {
    for id in ["peel.synth.poly", "peel.keys.tine", "peel.keys.grand"] {
        let prep = |d: &mut Box<dyn peel_dsp::Device>| {
            if let Some(r) = d.descriptor().param_by_key("amp.release") {
                d.set_param(r.id, 3.0);
            }
        };
        let go = |extra: &[Timed]| {
            let mut dev = dev(id);
            prep(&mut dev);
            let mut ev = vec![note_on(0, KEY, 0.8, 1), note_off(10_000, KEY, 1)];
            ev.extend_from_slice(extra);
            ev.sort_by_key(|e| e.0);
            render(dev.as_mut(), &ev, 40_000, 256).0
        };
        let a = go(&[vx(14_000, 1, 2.0, 0.0, 0.5)]);
        let b = go(&[]);
        // While the tail is still there: the piano's dampers leave nothing
        // to measure a third of a second after the release.
        let ratio = detect_pitch(&a[20_000..26_000], SR) / detect_pitch(&b[20_000..26_000], SR);
        assert!(
            (ratio / 2f32.powf(2.0 / 12.0) - 1.0).abs() < 0.03,
            "{id}: {ratio}"
        );
        // Another voice's snapshot leaves the tail alone.
        assert_eq!(go(&[vx(14_000, 2, 2.0, 1.0, 1.0)]), b, "{id}");
    }
}

#[test]
fn reused_voice_ids_start_clean() {
    for id in PITCHED {
        // The first note used ID 7 with a bend; after its tail ended a new
        // note reuses ID 7 without a snapshot of its own.
        let at = 250_000;
        let reused = run(
            id,
            &[
                note_on(0, KEY, 0.8, 7),
                vx(0, 7, 5.0, 1.0, 1.0),
                note_off(4_000, KEY, 7),
                note_on(at, KEY, 0.8, 7),
            ],
            at + 20_000,
            1_024,
        );
        let fresh_id = run(
            id,
            &[
                note_on(0, KEY, 0.8, 7),
                vx(0, 7, 5.0, 1.0, 1.0),
                note_off(4_000, KEY, 7),
                note_on(at, KEY, 0.8, 8),
            ],
            at + 20_000,
            1_024,
        );
        assert_eq!(reused, fresh_id, "{id}: a stale snapshot followed the ID");
    }
}

#[test]
fn held_voices_survive_snapshot_table_pressure() {
    for id in ["peel.synth.poly", "peel.keys.grand", "peel.synth.cross"] {
        let mut ev = vec![note_on(0, KEY, 0.8, 5), vx(0, 5, 7.0, 0.0, 0.5)];
        // Far more stray snapshots than the table holds, none with a note.
        for i in 0..300u32 {
            ev.push(vx(1_000 + i as usize, 1_000 + i, -12.0, 1.0, 0.0));
        }
        let a = run(id, &ev, 30_000, 256);
        let b = run(
            id,
            &[note_on(0, KEY, 0.8, 5), vx(0, 5, 7.0, 0.0, 0.5)],
            30_000,
            256,
        );
        assert_eq!(a, b, "{id}: eviction touched a held voice");
    }
}

#[test]
fn reset_expression_keeps_curves_but_panic_and_reset_end_them() {
    for id in PITCHED {
        // A controller reset centres the wheel and pressure only: the
        // sequenced note keeps its own curve.
        let curve = [note_on(0, KEY, 0.8, 9), vx(0, 9, 3.0, 0.5, 0.7)];
        let mut reset = curve.to_vec();
        reset.push((0, EventKind::PitchBend { semitones: -5.0 }));
        reset.push((0, EventKind::ChannelPressure { value: 1.0 }));
        reset.push((4_000, EventKind::ResetExpression));
        assert_eq!(
            run(id, &curve, 20_000, 256),
            run(id, &reset, 20_000, 256),
            "{id}"
        );
        {
            let reset = EventKind::AllSoundOff;
            // A snapshot waiting for its note is dropped.
            let a = run(
                id,
                &[
                    vx(0, 9, 7.0, 1.0, 1.0),
                    (2_000, reset),
                    note_on(6_000, KEY, 0.8, 9),
                ],
                20_000,
                256,
            );
            let b = run(
                id,
                &[(2_000, reset), note_on(6_000, KEY, 0.8, 9)],
                20_000,
                256,
            );
            assert_eq!(a, b, "{id}: pending snapshot survived {reset:?}");
        }
        let mut dev = dev(id);
        let ev = [vx(0, 9, 7.0, 1.0, 1.0)];
        render(dev.as_mut(), &ev, 2_000, 256);
        dev.reset();
        let a = render(dev.as_mut(), &[note_on(0, KEY, 0.8, 9)], 8_000, 256).0;
        let mut clean = self::dev(id);
        render(clean.as_mut(), &[], 2_000, 256);
        clean.reset();
        let b = render(clean.as_mut(), &[note_on(0, KEY, 0.8, 9)], 8_000, 256).0;
        assert_eq!(a, b, "{id}: snapshot survived reset()");
    }
}

#[test]
fn block_size_does_not_change_per_note_output() {
    let ev = [
        note_on(0, KEY, 0.8, 1),
        note_on(0, KEY, 0.8, 2),
        vx(700, 1, 1.5, 0.3, 0.7),
        vx(3_001, 2, -4.0, 0.9, 0.2),
        vx(9_123, 1, 12.0, 0.0, 0.5),
        note_off(12_000, KEY, 1),
        vx(13_333, 1, -2.0, 0.5, 1.0),
        vx(15_000, 2, 0.0, 0.0, 0.5),
    ];
    for id in REQUIRED {
        let a = run(id, &ev, 24_000, 24_000);
        let b = run(id, &ev, 24_000, 37);
        let c = run(id, &ev, 24_000, 1);
        assert!(
            max_diff(&a, &b) < 1.0e-4,
            "{id}: 1 block vs 37: {}",
            max_diff(&a, &b)
        );
        assert!(
            max_diff(&a, &c) < 1.0e-4,
            "{id}: 1 block vs 1: {}",
            max_diff(&a, &c)
        );
    }
}

#[test]
fn extreme_snapshots_stay_finite_and_bounded() {
    let cases = [
        (120.0f32, 1.0f32, 1.0f32),
        (-120.0, 1.0, 0.0),
        (f32::NAN, f32::NAN, f32::NAN),
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY),
        (1.0e9, -3.0, 9.0),
    ];
    for id in REQUIRED {
        for (pitch, pressure, timbre) in cases {
            let ev = [
                note_on(0, 108, 1.0, 1),
                note_on(0, 21, 1.0, 2),
                vx(0, 1, pitch, pressure, timbre),
                vx(500, 2, pitch, pressure, timbre),
                vx(9_000, 1, -pitch, 0.0, 0.5),
            ];
            let mut dev = dev(id);
            let mut ev = ev.to_vec();
            ev.sort_by_key(|e| e.0);
            let (l, r) = render(dev.as_mut(), &ev, 30_000, 97);
            assert!(all_finite(&l) && all_finite(&r), "{id} {pitch}");
            assert!(peak(&l) < 4.5 && peak(&r) < 4.5, "{id} {pitch}");
        }
    }
}

#[test]
fn mono_voices_follow_the_note_on_top() {
    let mono = |id: &str| -> Box<dyn peel_dsp::Device> {
        let mut d = dev(id);
        // Poly in Legato mode; Bass and Sax are monophonic by default.
        if id == "peel.synth.poly"
            && let Some(m) = d.descriptor().param_by_key("voice.mode")
        {
            d.set_param(m.id, 2.0);
        }
        d
    };
    for id in ["peel.synth.bass", "peel.wind.sax", "peel.synth.poly"] {
        // Legato: note 2 (bent a fifth) on top of note 1, then released.
        let ev = vec![
            note_on(0, KEY, 0.8, 1),
            vx(0, 1, 0.0, 0.0, 0.5),
            note_on(8_000, KEY, 0.8, 2),
            vx(8_000, 2, 7.0, 0.0, 0.5),
            note_off(24_000, KEY, 2),
        ];
        let mut dev = mono(id);
        let out = render(dev.as_mut(), &ev, 40_000, 256).0;
        let low = detect_pitch(&out[2_000..7_000], SR);
        let top = detect_pitch(&out[16_000..22_000], SR);
        let back = detect_pitch(&out[32_000..38_000], SR);
        let want = 2f32.powf(7.0 / 12.0);
        assert!(
            (top / low / want - 1.0).abs() < 0.03,
            "{id}: top {top} low {low}"
        );
        assert!(
            (back / low - 1.0).abs() < 0.03,
            "{id}: back {back} low {low}"
        );
    }
}

/// The instrument-wide bend reaches an MPE zone's ±96 st manager range.
#[test]
fn shared_bend_reaches_96_semitones_where_the_pitch_fits() {
    // A0 up eight octaves is A8 (7040 Hz). Excluded: Sax (its reed stops
    // near 5.8 kHz), the Sampler (its rate cap is 32x), and the Violin and
    // Zither waveguides (a loop under seven samples holds little of the
    // excitation at the new pitch). All of them stay finite (next test).
    let skip = [
        "peel.wind.sax",
        "peel.sampler",
        "peel.strings.violin",
        "peel.strings.zither",
    ];
    for id in PITCHED.into_iter().filter(|id| !skip.contains(id)) {
        let ev = [
            note_on(0, 21, 0.9, 1),
            (0, EventKind::PitchBend { semitones: 96.0 }),
        ];
        let up = run(id, &ev, 16_000, 256);
        // Narrow-band energy: A8 itself against the semitones either side.
        let w = &up[6_000..14_192];
        let semi = 2f32.powf(1.0 / 12.0);
        let (at, below, above) = (
            band(w, 7_040.0),
            band(w, 7_040.0 / semi),
            band(w, 7_040.0 * semi),
        );
        assert!(
            at > 20.0 * below && at > 20.0 * above,
            "{id}: {at} vs {below} {above}"
        );
        // The same note from a snapshot sounds the same.
        let snap = run(
            id,
            &[vx(0, 1, 96.0, 0.0, 0.5), note_on(0, 21, 0.9, 1)],
            16_000,
            256,
        );
        let ws = &snap[6_000..14_192];
        assert!(
            band(ws, 7_040.0) > 20.0 * band(ws, 7_040.0 / semi),
            "{id}: snapshot"
        );
    }
}

#[test]
fn shared_bend_extremes_stay_finite_and_bounded() {
    for id in REQUIRED {
        for st in [120.0f32, -120.0, 96.0, -96.0, 500.0] {
            let ev = [
                note_on(0, 108, 1.0, 1),
                note_on(0, 21, 1.0, 2),
                (300, EventKind::PitchBend { semitones: st }),
                (9_000, EventKind::PitchBend { semitones: -st }),
            ];
            let mut dev = dev(id);
            let (l, r) = render(dev.as_mut(), &ev, 24_000, 97);
            assert!(all_finite(&l) && all_finite(&r), "{id} {st}");
            assert!(peak(&l) < 4.5 && peak(&r) < 4.5, "{id} {st}");
        }
    }
}

#[test]
fn clearing_a_curve_returns_the_held_voice_to_shared_controllers() {
    for id in PITCHED {
        let output = run(
            id,
            &[
                (0, EventKind::PitchBend { semitones: 2.0 }),
                note_on(0, KEY, 0.8, 99),
                vx(0, 99, 7.0, 0.7, 0.8),
                (12_000, EventKind::ClearVoiceExpression { voice: 99 }),
                (26_000, EventKind::PitchBend { semitones: 3.0 }),
            ],
            44_000,
            97,
        );
        let reference = run(id, &[note_on(0, KEY, 0.8, 99)], 44_000, 97);
        let ratio = detect_pitch(&output[18_000..24_000], SR)
            / detect_pitch(&reference[18_000..24_000], SR);
        assert!(
            (ratio / 2f32.powf(2.0 / 12.0) - 1.0).abs() < 0.03,
            "{id}: clear ratio {ratio}"
        );
        let ratio = detect_pitch(&output[36_000..42_000], SR)
            / detect_pitch(&reference[36_000..42_000], SR);
        assert!(
            (ratio / 2f32.powf(3.0 / 12.0) - 1.0).abs() < 0.03,
            "{id}: shared controller ratio {ratio}"
        );
        assert!(all_finite(&output), "{id}");
    }
}
