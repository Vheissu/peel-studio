//! What Tape and Gate are for: saturation that squeezes peaks, a head bump,
//! a high cut, real pitch wobble, hiss that needs a signal, and a gate that
//! opens, holds, releases and follows a sidechain key.
mod common;
use common::*;
use peel_dsp::{Device, ProcessContext};

const TAPE: &str = "peel.fx.tape";
const GATE: &str = "peel.fx.gate";

fn configured(id: &str, params: &[(&str, f32)]) -> Box<dyn Device> {
    let mut d = make(id, None, SR, 256);
    for (k, v) in params {
        let p = d.descriptor().param_by_key(k).expect(k).id;
        d.set_param(p, *v);
    }
    d.prepare(SR, 256);
    d
}

fn process(d: &mut dyn Device, src: &[f32]) -> Vec<f32> {
    let mut l = src.to_vec();
    let mut r = l.clone();
    run(d, &mut l, &mut r, 256, &[]);
    l
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn tone(freq: f64, amp: f32, n: usize) -> Vec<f32> {
    (0..n).map(|i| amp * sine(freq, i)).collect()
}

/// Level of `freq` in `x` (a single-bin DFT), as a linear amplitude.
fn level_at(x: &[f32], freq: f64) -> f64 {
    let (mut a, mut b) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let t = std::f64::consts::TAU * freq * i as f64 / f64::from(SR);
        a += f64::from(*v) * t.cos();
        b += f64::from(*v) * t.sin();
    }
    2.0 * (a * a + b * b).sqrt() / x.len() as f64
}

/// Tape with the motion and tone shaping off, so one thing is measured.
const STILL: [(&str, f32); 4] = [
    ("wow", 0.0),
    ("flutter", 0.0),
    ("bump", 0.0),
    ("tone", 20_000.0),
];

fn tape(extra: &[(&str, f32)]) -> Box<dyn Device> {
    let mut params = STILL.to_vec();
    params.extend_from_slice(extra);
    configured(TAPE, &params)
}

#[test]
fn tape_reports_its_transport_delay_and_keeps_the_dry_mix_aligned() {
    let mut d = tape(&[("drive", 0.0), ("mix", 0.0)]);
    let lat = d.latency() as usize;
    assert_eq!(lat, 144, "3 ms at 48 kHz");
    let src = tone(440.0, 0.3, 9_600);
    let out = process(d.as_mut(), &src);
    // Fully dry is the input, late by the reported latency.
    for i in 2_000..9_000 {
        assert!((out[i] - src[i - lat]).abs() < 1e-6, "sample {i}");
    }
    // Half wet at a low level does not comb: the wet copy arrives at the
    // same time, so the tone keeps its level.
    let mut d = tape(&[("drive", 0.0), ("mix", 0.5)]);
    let quiet = tone(440.0, 0.02, 9_600);
    let out = process(d.as_mut(), &quiet);
    let ratio = rms(&out[4_800..]) / rms(&quiet[4_800..]);
    assert!((ratio - 1.0).abs() < 0.02, "level ratio {ratio}");
}

#[test]
fn tape_drive_lifts_quiet_signals_and_squeezes_loud_ones() {
    let gain = |amp: f32, drive: f32| {
        let mut d = tape(&[("drive", drive)]);
        let src = tone(220.0, amp, 14_400);
        let out = process(d.as_mut(), &src);
        20.0 * (rms(&out[4_800..]) / rms(&src[4_800..])).log10()
    };
    // No drive: quiet material passes at unity.
    assert!(gain(0.01, 0.0).abs() < 0.2, "{}", gain(0.01, 0.0));
    // 12 dB of drive raises a quiet tone by about half of it.
    let quiet = gain(0.01, 12.0);
    assert!((quiet - 6.0).abs() < 0.6, "quiet gain {quiet} dB");
    // A full-scale tone gains far less: the peaks are compressed.
    let loud = gain(1.0, 12.0);
    assert!(loud < quiet - 4.0, "loud gain {loud} dB");
    // And it never exceeds the curve's ceiling.
    let mut d = tape(&[("drive", 24.0)]);
    let out = process(d.as_mut(), &tone(110.0, 1.8, 14_400));
    assert!(peak(&out) < 1.0, "peak {}", peak(&out));
}

#[test]
fn tape_saturation_adds_odd_and_even_harmonics_without_aliasing() {
    let mut d = tape(&[("drive", 12.0)]);
    let n = 48_000;
    let out = process(d.as_mut(), &tone(1_000.0, 0.8, n));
    let x = &out[9_600..];
    let fund = level_at(x, 1_000.0);
    let db = |f: f64| 20.0 * (level_at(x, f) / fund).log10();
    assert!(db(3_000.0) > -25.0, "third {}", db(3_000.0));
    assert!(db(2_000.0) > -45.0, "second {}", db(2_000.0));
    // A 7 kHz tone's third and fifth harmonics fold to 3 and 13 kHz at
    // 48 kHz; antialiasing keeps the folded ones well down.
    let mut d = tape(&[("drive", 12.0)]);
    let out = process(d.as_mut(), &tone(7_000.0, 0.8, n));
    let x = &out[9_600..];
    let fund = level_at(x, 7_000.0);
    let alias = 20.0 * (level_at(x, 13_000.0) / fund).log10();
    assert!(alias < -40.0, "fifth harmonic alias at {alias} dB");
}

#[test]
fn tape_head_bump_and_tone_shape_the_ends() {
    let level = |params: &[(&str, f32)], freq: f64| {
        let mut all = vec![("wow", 0.0), ("flutter", 0.0), ("drive", 0.0)];
        all.extend_from_slice(params);
        let mut d = configured(TAPE, &all);
        let src = tone(freq, 0.02, 24_000);
        let out = process(d.as_mut(), &src);
        20.0 * (rms(&out[9_600..]) / rms(&src[9_600..])).log10()
    };
    let flat = [("bump", 0.0), ("tone", 20_000.0)];
    assert!(level(&flat, 85.0).abs() < 0.3);
    let bumped = level(&[("bump", 1.0), ("tone", 20_000.0)], 85.0);
    assert!((bumped - 5.0).abs() < 0.5, "bump {bumped} dB");
    assert!(level(&[("bump", 1.0), ("tone", 20_000.0)], 1_000.0).abs() < 0.3);
    let dark = level(&[("bump", 0.0), ("tone", 3_000.0)], 9_000.0);
    assert!(dark < -15.0, "9 kHz through a 3 kHz tone: {dark} dB");
}

#[test]
fn wow_and_flutter_move_the_pitch_and_still_tape_does_not() {
    // Period-by-period frequency of a steady tone.
    let spread = |params: &[(&str, f32)]| {
        let mut d = configured(TAPE, params);
        let out = process(d.as_mut(), &tone(1_000.0, 0.2, 96_000));
        let x = &out[4_800..];
        let mut crossings = Vec::new();
        for i in 1..x.len() {
            if x[i - 1] < 0.0 && x[i] >= 0.0 {
                crossings.push(i as f32 - 1.0 - x[i - 1] / (x[i] - x[i - 1]));
            }
        }
        // Frequency over runs of 40 cycles, as a deviation in percent.
        let f: Vec<f32> = crossings
            .windows(41)
            .step_by(20)
            .map(|w| 40.0 * SR / (w[40] - w[0]))
            .collect();
        let (lo, hi) = f
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        (hi - lo) / 1_000.0 * 100.0
    };
    let still = spread(&[("wow", 0.0), ("flutter", 0.0), ("drive", 0.0)]);
    assert!(still < 0.01, "a still transport moved {still}%");
    let wow = spread(&[("wow", 1.0), ("flutter", 0.0), ("drive", 0.0)]);
    assert!((0.5..1.4).contains(&wow), "full wow swings {wow}%");
    let flutter = spread(&[("wow", 0.0), ("flutter", 1.0), ("drive", 0.0)]);
    assert!(
        (0.1..0.5).contains(&flutter),
        "full flutter swings {flutter}%"
    );
}

#[test]
fn hiss_needs_a_signal() {
    let mut d = tape(&[("hiss", 1.0)]);
    let silent = process(d.as_mut(), &vec![0.0; 24_000]);
    assert_eq!(peak(&silent), 0.0);
    let src = tone(220.0, 0.2, 24_000);
    let mut with = tape(&[("hiss", 1.0), ("drive", 0.0)]);
    let mut without = tape(&[("hiss", 0.0), ("drive", 0.0)]);
    let (a, b) = (
        process(with.as_mut(), &src),
        process(without.as_mut(), &src),
    );
    let noise: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x - y).collect();
    let level = 20.0 * rms(&noise[4_800..]).log10();
    assert!((-62.0..-44.0).contains(&level), "hiss at {level} dBFS");
}

/// A tone that is loud, then quiet, then loud again.
fn steps() -> Vec<f32> {
    (0..48_000)
        .map(|i| {
            let amp = if (12_000..36_000).contains(&i) {
                0.004
            } else {
                0.5
            };
            amp * sine(500.0, i)
        })
        .collect()
}

#[test]
fn gate_passes_loud_signal_and_mutes_quiet_signal_after_the_hold() {
    let mut d = configured(
        GATE,
        &[
            ("threshold", -30.0),
            ("hold", 100.0),
            ("release", 50.0),
            ("attack", 0.1),
        ],
    );
    let src = steps();
    let out = process(d.as_mut(), &src);
    // Open: the loud part passes untouched once the attack is over.
    for i in 4_800..11_900 {
        assert!((out[i] - src[i]).abs() < 1e-4, "sample {i}");
    }
    // Still open during the 100 ms hold after the level drops.
    let held = rms(&out[12_500..16_000]) / rms(&src[12_500..16_000]);
    assert!((held - 1.0).abs() < 0.02, "during hold: {held}");
    // Closed once hold and release have passed.
    assert!(
        peak(&out[22_000..35_900]) < 1e-5,
        "{}",
        peak(&out[22_000..35_900])
    );
    // And open again within a millisecond of the signal returning.
    let back = rms(&out[36_100..38_000]) / rms(&src[36_100..38_000]);
    assert!((back - 1.0).abs() < 0.02, "reopened at {back}");
}

#[test]
fn gate_range_ducks_instead_of_muting() {
    let mut d = configured(
        GATE,
        &[
            ("threshold", -30.0),
            ("range", -12.0),
            ("hold", 0.0),
            ("release", 20.0),
        ],
    );
    let src = steps();
    let out = process(d.as_mut(), &src);
    let db = 20.0 * (rms(&out[24_000..35_000]) / rms(&src[24_000..35_000])).log10();
    assert!((db + 12.0).abs() < 0.3, "closed gain {db} dB");
    assert!(
        (d.reduction_db().unwrap() - 0.0).abs() < 0.1,
        "open again at the end"
    );
}

#[test]
fn hysteresis_stops_chatter_near_the_threshold() {
    // A tone sitting 2 dB under the threshold after opening above it.
    let src: Vec<f32> = (0..48_000)
        .map(|i| {
            let db = if i < 4_800 { -20.0 } else { -32.0 };
            10f32.powf(db / 20.0) * sine(500.0, i)
        })
        .collect();
    let closed_after = |hysteresis: f32| {
        let mut d = configured(
            GATE,
            &[
                ("threshold", -30.0),
                ("hysteresis", hysteresis),
                ("hold", 0.0),
                ("release", 10.0),
            ],
        );
        let out = process(d.as_mut(), &src);
        rms(&out[24_000..]) < 1e-4
    };
    assert!(
        closed_after(0.0),
        "without hysteresis it closes below threshold"
    );
    assert!(
        !closed_after(6.0),
        "6 dB of hysteresis keeps it open at -32 dB"
    );
}

#[test]
fn gate_follows_a_sidechain_key() {
    let mut d = configured(
        GATE,
        &[
            ("threshold", -30.0),
            ("hold", 0.0),
            ("release", 10.0),
            ("attack", 0.1),
        ],
    );
    // A steady pad, keyed by a tone that only sounds in the middle.
    let pad = tone(300.0, 0.3, 36_000);
    let key: Vec<f32> = (0..36_000)
        .map(|i| {
            if (12_000..24_000).contains(&i) {
                0.5 * sine(900.0, i)
            } else {
                0.0
            }
        })
        .collect();
    let (mut l, mut r) = (pad.clone(), pad.clone());
    let ctx = ProcessContext::new(SR);
    let mut pos = 0;
    while pos < l.len() {
        let end = (pos + 256).min(l.len());
        d.process_keyed(
            &ctx,
            &[],
            &mut l[pos..end],
            &mut r[pos..end],
            &key[pos..end],
            &key[pos..end],
        );
        pos = end;
    }
    assert!(peak(&l[1_000..11_900]) < 1e-5, "closed before the key");
    let open = rms(&l[13_000..23_500]) / rms(&pad[13_000..23_500]);
    assert!((open - 1.0).abs() < 0.02, "open with the key: {open}");
    assert!(peak(&l[30_000..]) < 1e-5, "closed after the key");
    assert_eq!(l, r, "both channels share one gain");
}

#[test]
fn sidechain_high_pass_ignores_low_keys() {
    let run_with = |hpf: f32| {
        let mut d = configured(
            GATE,
            &[
                ("threshold", -30.0),
                ("sidechain.hpf", hpf),
                ("hold", 0.0),
                ("release", 10.0),
            ],
        );
        // An 50 Hz rumble at -14 dBFS: over the threshold unless filtered.
        let out = process(d.as_mut(), &tone(50.0, 0.2, 24_000));
        rms(&out[12_000..])
    };
    assert!(
        run_with(20.0) > 0.1,
        "unfiltered, the rumble opens the gate"
    );
    assert!(run_with(1_000.0) < 1e-4, "filtered, it stays shut");
}
