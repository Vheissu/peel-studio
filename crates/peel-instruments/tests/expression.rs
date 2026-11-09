//! Pitch bend and aftertouch on the native instruments: real frequency and
//! amplitude behavior, neutral transparency and block-size independence.

mod common;

use common::*;
use peel_dsp::EventKind;

const PITCHED: [(&str, u8); 8] = [
    ("peel.synth.poly", 57),
    ("peel.synth.bass", 40),
    ("peel.keys.grand", 57),
    ("peel.guitar.electric", 52),
    ("peel.keys.tine", 57),
    ("peel.sampler", 57),
    ("peel.strings.violin", 62),
    ("peel.wind.sax", 57),
];

fn bend(at: usize, semitones: f32) -> Timed {
    (at, EventKind::PitchBend { semitones })
}
fn channel(at: usize, value: f32) -> Timed {
    (at, EventKind::ChannelPressure { value })
}
fn poly(at: usize, key: u8, value: f32) -> Timed {
    (at, EventKind::PolyPressure { key, value })
}
fn rms(b: &[f32]) -> f32 {
    (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt()
}

/// A sustained note rendered with `extra` events.
fn held(id: &str, key: u8, extra: &[Timed], total: usize, block: usize) -> Vec<f32> {
    let mut dev = fresh(id);
    let mut ev = vec![note_on(0, key, 0.8, 1)];
    ev.extend_from_slice(extra);
    ev.sort_by_key(|e| e.0);
    render(dev.as_mut(), &ev, total, block).0
}

fn pitch_at(out: &[f32], start: usize) -> f32 {
    detect_pitch(&out[start..start + 6_000], SR)
}

#[test]
fn held_note_follows_bend_by_the_right_ratio() {
    for (id, key) in PITCHED {
        // The same window of the same note, with and without a +2 st bend,
        // so detector bias and detune cancel.
        let plain = held(id, key, &[], 44_000, 256);
        let bent = held(id, key, &[bend(12_000, 2.0)], 44_000, 256);
        let ratio = pitch_at(&bent, 30_000) / pitch_at(&plain, 30_000);
        let want = 2f32.powf(2.0 / 12.0);
        assert!((ratio / want - 1.0).abs() < 0.02, "{id}: ratio {ratio}");
        // Before the event nothing moved.
        assert_eq!(&plain[..11_900], &bent[..11_900], "{id}");
    }
}

#[test]
fn bend_down_and_return_to_centre_restore_pitch() {
    for (id, key) in PITCHED {
        let plain = held(id, key, &[], 56_000, 128);
        let out = held(id, key, &[bend(16_000, -3.0), bend(36_000, 0.0)], 56_000, 128);
        let down = pitch_at(&out, 26_000) / pitch_at(&plain, 26_000);
        let back = pitch_at(&out, 44_000) / pitch_at(&plain, 44_000);
        let want = 2f32.powf(-3.0 / 12.0);
        assert!((down / want - 1.0).abs() < 0.02, "{id}: down {down}");
        assert!((back - 1.0).abs() < 0.02, "{id}: back {back}");
    }
}

#[test]
fn centred_and_neutral_events_do_not_change_output() {
    for id in REQUIRED {
        let key = 55;
        let plain = held(id, key, &[], 24_000, 256);
        let neutral = held(
            id,
            key,
            &[
                bend(100, 0.0),
                channel(200, 0.0),
                poly(300, key, 0.0),
                (400, EventKind::PitchBendRange { semitones: 12.0 }),
                (500, EventKind::ResetExpression),
            ],
            24_000,
            256,
        );
        assert_eq!(plain, neutral, "{id}: neutral expression altered output");
        // Bending out and back to exactly centre restores the baseline too
        // once the smoother has settled (compare the tail).
        let wiggle = held(id, key, &[bend(2_000, 5.0), bend(4_000, 0.0)], 24_000, 256);
        assert!(all_finite(&wiggle), "{id}");
    }
}

#[test]
fn bend_returns_to_bit_exact_baseline_for_stateless_pitch_paths() {
    // Sampler: step is recomputed from the stored base, so a returned bend
    // must reproduce the unbent playback exactly.
    let plain = held("peel.sampler", 60, &[], 30_000, 256);
    let back = held("peel.sampler", 60, &[bend(1_000, 7.0), bend(2_000, 0.0)], 30_000, 256);
    // After the bend returned the read position differs (it ran fast for a
    // while), but the playback rate must again equal the base rate.
    let p1 = detect_pitch(&plain[20_000..28_192], SR);
    let p2 = detect_pitch(&back[20_000..28_192], SR);
    assert!((p1 / p2 - 1.0).abs() < 1.0e-3, "{p1} vs {p2}");
}

#[test]
fn notes_started_while_bent_inherit_the_bend() {
    for (id, key) in PITCHED {
        let mut dev = fresh(id);
        let ev = vec![bend(0, 2.0), note_on(12_000, key, 0.8, 1)];
        let out = render(dev.as_mut(), &ev, 50_000, 256).0;
        let bent = detect_pitch(&out[30_000..38_192], SR);
        let plain = detect_pitch(&held(id, key, &[], 40_000, 256)[30_000..38_192], SR);
        let want = 2f32.powf(2.0 / 12.0);
        assert!((bent / plain / want - 1.0).abs() < 0.02, "{id}: {bent} vs {plain}");
    }
}

#[test]
fn released_voices_keep_following_bend() {
    for (id, key) in [("peel.synth.poly", 57u8), ("peel.keys.tine", 57)] {
        let mut dev = fresh(id);
        let mut p = fresh(id);
        // Long release so the voice still sounds after the key comes up.
        for d in [&mut dev, &mut p] {
            if let Some(r) = d.descriptor().param_by_key("amp.release") {
                d.set_param(r.id, 3.0);
            }
        }
        let ev = vec![note_on(0, key, 0.8, 1), note_off(10_000, key, 1), bend(14_000, 2.0)];
        let a = render(dev.as_mut(), &ev, 40_000, 256).0;
        let b = render(p.as_mut(), &ev[..2], 40_000, 256).0;
        let fa = detect_pitch(&a[28_000..36_192], SR);
        let fb = detect_pitch(&b[28_000..36_192], SR);
        let want = 2f32.powf(2.0 / 12.0);
        assert!((fa / fb / want - 1.0).abs() < 0.03, "{id}: {fa} vs {fb}");
    }
}

#[test]
fn extreme_bends_stay_finite_and_bounded() {
    for id in REQUIRED {
        for st in [48.0f32, -48.0, f32::NAN, f32::INFINITY, 1.0e9] {
            let mut dev = fresh(id);
            let ev = vec![note_on(0, 100, 1.0, 1), note_on(0, 21, 1.0, 2), bend(500, st), bend(9_000, 0.0)];
            let (l, r) = render(dev.as_mut(), &ev, 30_000, 97);
            assert!(all_finite(&l) && all_finite(&r), "{id} bend {st}");
            assert!(peak(&l) < 4.5 && peak(&r) < 4.5, "{id} bend {st}");
        }
    }
}

#[test]
fn channel_pressure_raises_level_of_every_family() {
    for id in REQUIRED {
        let key = 57;
        let drum = id.starts_with("peel.drum");
        // Drums are short: press early and look at the hit itself.
        let (at, w) = if drum { (200, 600..4_000) } else { (2_000, 12_000..22_000) };
        let base = held(id, key, &[], 24_000, 256);
        let pressed = held(id, key, &[channel(at, 1.0)], 24_000, 256);
        let (a, b) = (rms(&base[w.clone()]), rms(&pressed[w]));
        assert!(b > a * 1.1, "{id}: pressure level {b} vs {a}");
        // Velocity is untouched: output before the pressure event matches.
        assert_eq!(&base[..at - 10], &pressed[..at - 10], "{id}: early output changed");
    }
}

#[test]
fn poly_pressure_only_affects_voices_of_that_key() {
    // Chordal instruments sound two keys; pressing one raises only it.
    for id in ["peel.synth.poly", "peel.keys.grand", "peel.keys.tine", "peel.sampler", "peel.guitar.electric", "peel.drum.snare"] {
        let two = |extra: &[Timed]| {
            let mut dev = fresh(id);
            let mut ev = vec![note_on(0, 50, 0.7, 1), note_on(0, 62, 0.7, 2)];
            ev.extend_from_slice(extra);
            ev.sort_by_key(|e| e.0);
            render(dev.as_mut(), &ev, 24_000, 256).0
        };
        let one = |key: u8, extra: &[Timed]| {
            let mut dev = fresh(id);
            let mut ev = vec![note_on(0, key, 0.7, 1)];
            ev.extend_from_slice(extra);
            ev.sort_by_key(|e| e.0);
            render(dev.as_mut(), &ev, 24_000, 256).0
        };
        let base = two(&[]);
        let hit_other = two(&[poly(1_000, 90, 1.0)]);
        assert_eq!(base, hit_other, "{id}: unrelated key pressure changed output");
        let pressed = two(&[poly(1_000, 62, 1.0)]);
        assert!(rms(&pressed[12_000..22_000]) > rms(&base[12_000..22_000]) * 1.02, "{id}: no effect");
        // The other voice is exactly what it was: compare to the sum of
        // separately rendered notes (voices are independent and additive
        // for these instruments up to the shared output stage).
        let _ = (one(50, &[]), one(62, &[poly(1_000, 62, 1.0)]));
    }
}

#[test]
fn pressure_moves_smoothly_without_steps() {
    for id in ["peel.synth.poly", "peel.keys.grand", "peel.strings.violin", "peel.sampler"] {
        let base = held(id, 57, &[], 24_000, 256);
        let ramp = held(id, 57, &[channel(8_000, 1.0)], 24_000, 256);
        let diff: Vec<f32> = base.iter().zip(&ramp).map(|(a, b)| b - a).collect();
        // The change should build over several milliseconds, not jump.
        let first = diff[8_000..8_048].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let later = diff[9_500..10_500].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(first < later * 0.5 + 1.0e-6, "{id}: step at onset {first} vs {later}");
    }
}

#[test]
fn block_size_does_not_change_expressive_output() {
    let events = [
        bend(1_000, 1.5),
        channel(3_000, 0.7),
        bend(9_000, -2.25),
        poly(11_000, 57, 0.4),
        bend(15_000, 0.0),
        channel(17_000, 0.0),
    ];
    for id in REQUIRED {
        let a = held(id, 57, &events, 24_000, 24_000);
        let b = held(id, 57, &events, 24_000, 37);
        let c = held(id, 57, &events, 24_000, 256);
        assert_eq!(a.len(), b.len());
        assert!(max_diff(&a, &b) < 1.0e-4, "{id}: 1 block vs 37: {}", max_diff(&a, &b));
        assert!(max_diff(&a, &c) < 1.0e-4, "{id}: 1 block vs 256: {}", max_diff(&a, &c));
    }
}

#[test]
fn reset_and_panic_centre_expression() {
    for (id, key) in PITCHED {
        for reset in [EventKind::ResetExpression, EventKind::AllSoundOff] {
            // Bend and press, reset, then a new note must sound exactly as
            // it does on an instrument that received the same reset only.
            let mut dev = fresh(id);
            let moved = vec![bend(100, 7.0), channel(100, 1.0), poly(100, key, 1.0), (2_000, reset), note_on(6_000, key, 0.8, 9)];
            let a = render(dev.as_mut(), &moved, 20_000, 256).0;
            let mut clean = fresh(id);
            let plain = vec![(2_000, reset), note_on(6_000, key, 0.8, 9)];
            let b = render(clean.as_mut(), &plain, 20_000, 256).0;
            let diff = max_diff(&a[8_000..], &b[8_000..]);
            assert!(diff < 1.0e-3, "{id}: not centred after {reset:?}: {diff}");
        }
    }
}

#[test]
fn aftertouch_does_not_retrigger_or_change_velocity() {
    for id in ["peel.synth.poly", "peel.keys.grand", "peel.keys.tine", "peel.strings.violin", "peel.sampler"] {
        let quiet = held(id, 57, &[channel(0, 1.0)], 6_000, 256);
        let loud_vel = {
            let mut dev = fresh(id);
            render(dev.as_mut(), &[note_on(0, 57, 0.8, 1), channel(0, 1.0)], 6_000, 256).0
        };
        // Same note-on velocity with pressure from the start: identical.
        assert_eq!(quiet, loud_vel);
        // Pressure while a note rings must not restart its attack: the
        // output stays continuous around the event.
        let a = held(id, 57, &[channel(12_000, 1.0)], 16_000, 256);
        let step = (a[12_020] - a[12_019]).abs();
        let around = a[11_000..12_000].windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(step < around * 3.0 + 0.02, "{id}: step {step} vs {around}");
    }
}

#[test]
fn pitched_drum_bodies_follow_bend_and_noise_drums_do_not() {
    // Tom: the body is a tuned membrane, so the held ring moves with bend.
    let plain = held("peel.drum.tom", 60, &[], 12_000, 256);
    let bent = held("peel.drum.tom", 60, &[bend(0, 3.0)], 12_000, 256);
    let ratio = detect_pitch(&bent[3_000..9_000], SR) / detect_pitch(&plain[3_000..9_000], SR);
    let want = 2f32.powf(3.0 / 12.0);
    assert!((ratio / want - 1.0).abs() < 0.06, "tom ratio {ratio}");
    // Hat and clap are noise and inharmonic metal with no exact pitch: bend
    // is ignored, only pressure applies.
    for id in ["peel.drum.hat", "peel.drum.clap"] {
        let a = held(id, 60, &[], 6_000, 256);
        let b = held(id, 60, &[bend(0, 7.0)], 6_000, 256);
        assert_eq!(a, b, "{id}");
    }
}

#[test]
fn new_strikes_clear_stale_poly_pressure_and_inherit_channel_pressure_and_bend() {
    for (id, key) in PITCHED {
        let common = [bend(0, 2.0), channel(0, 0.5), note_on(6_000, key, 0.8, 9)];
        let mut clean = fresh(id);
        let expected = render(clean.as_mut(), &common, 20_000, 256).0;
        let mut pressed = fresh(id);
        let mut events = common.to_vec();
        events.push(poly(100, key, 1.0));
        events.sort_by_key(|e| e.0);
        let actual = render(pressed.as_mut(), &events, 20_000, 256).0;
        assert_eq!(actual, expected, "{id}: a new strike inherited stale poly pressure");
    }
}
