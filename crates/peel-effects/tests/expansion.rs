//! Signature behavior of the new creative effects, beyond registry checks.
mod common;
use common::*;
use peel_dsp::{Device, ProcessContext};

fn set(d: &mut dyn Device, key: &str, v: f32) {
    let id = d.descriptor().param_by_key(key).expect(key).id;
    d.set_param(id, v);
}
fn configured(id: &str, params: &[(&str, f32)]) -> Box<dyn Device> {
    let mut d = make(id, None, SR, 256);
    for (k, v) in params {
        set(d.as_mut(), k, *v);
    }
    d.prepare(SR, 256);
    d
}
fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}
fn drum(n: usize, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| amp * sine(850.0, i) * (-(i as f32) / (SR * 0.04)).exp())
        .collect()
}
fn process(d: &mut dyn Device, src: &[f32]) -> Vec<f32> {
    let mut l = src.to_vec();
    let mut r = l.clone();
    run(d, &mut l, &mut r, 256, &[]);
    l
}
fn tone_level(x: &[f32], freq: f64) -> f64 {
    let (mut a, mut b) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let t = std::f64::consts::TAU * freq * i as f64 / SR as f64;
        a += *v as f64 * t.cos();
        b += *v as f64 * t.sin();
    }
    (a * a + b * b).sqrt() / x.len() as f64
}

#[test]
fn transient_attack_and_sustain_controls_change_the_intended_parts() {
    let src = drum(24_000, 0.3);
    let neutral = process(
        configured("peel.fx.transient", &[("attack", 0.0)]).as_mut(),
        &src,
    );
    let punch = process(
        configured("peel.fx.transient", &[("attack", 1.0)]).as_mut(),
        &src,
    );
    let soft = process(
        configured("peel.fx.transient", &[("attack", -1.0)]).as_mut(),
        &src,
    );
    assert!(rms(&punch[..480]) > rms(&neutral[..480]) * 1.3);
    assert!(rms(&soft[..480]) < rms(&neutral[..480]) * 0.8);
    let up = process(
        configured("peel.fx.transient", &[("attack", 0.0), ("sustain", 1.0)]).as_mut(),
        &src,
    );
    let down = process(
        configured("peel.fx.transient", &[("attack", 0.0), ("sustain", -1.0)]).as_mut(),
        &src,
    );
    assert!(rms(&up[960..4_800]) > rms(&neutral[960..4_800]) * 1.2);
    assert!(rms(&down[960..4_800]) < rms(&neutral[960..4_800]) * 0.85);
}
#[test]
fn transient_shape_is_level_independent_and_stereo_linked() {
    let a = process(
        configured("peel.fx.transient", &[("sustain", 0.5)]).as_mut(),
        &drum(12_000, 0.1),
    );
    let b = process(
        configured("peel.fx.transient", &[("sustain", 0.5)]).as_mut(),
        &drum(12_000, 0.4),
    );
    assert!(a.iter().zip(&b).all(|(a, b)| (a * 4.0 - b).abs() < 2e-5));
    let mut d = configured("peel.fx.transient", &[]);
    let mut l = drum(12_000, 0.3);
    let mut r: Vec<_> = l.iter().map(|v| v * 0.25).collect();
    run(d.as_mut(), &mut l, &mut r, 256, &[]);
    assert!(l.iter().zip(&r).all(|(l, r)| (l * 0.25 - r).abs() < 1e-6));
}
#[test]
fn transient_reports_signed_gain_and_clip_bounds_hot_attacks() {
    let mut d = peel_effects::transient::Transient::new();
    d.set_param(1, -1.0);
    d.prepare(SR, 256);
    let mut l = drum(256, 0.9);
    let mut r = l.clone();
    d.process(&ProcessContext::new(SR), &[], &mut l, &mut r);
    assert!(d.gain_change_db() < 0.0);
    let clipped = process(
        configured("peel.fx.transient", &[("attack", 1.0), ("clip", 1.0)]).as_mut(),
        &drum(12_000, 1.5),
    );
    assert!(peak(&clipped) <= 1.0);
}
#[test]
fn crush_four_bits_has_symmetric_zero_preserving_levels() {
    let mut d = configured("peel.fx.crush", &[("bits", 4.0), ("rate", 48_000.0)]);
    let mut values = Vec::new();
    for i in 0..33 {
        let x = -1.0 + i as f32 / 16.0;
        let out = process(d.as_mut(), &vec![x; 512]);
        let q = out[511];
        assert!((q * 7.0 - (q * 7.0).round()).abs() < 1e-4);
        if !values.iter().any(|v: &f32| (v - q).abs() < 1e-4) {
            values.push(q);
        }
    }
    assert_eq!(values.len(), 15);
    for value in &values {
        assert!(values.iter().any(|other| (value + other).abs() < 1e-4));
    }
}
#[test]
fn crush_filter_removes_aliasing_and_jitter_changes_hold_times() {
    let src: Vec<_> = (0..24_000).map(|i| 0.6 * sine(10_000.0, i)).collect();
    let raw = process(
        configured("peel.fx.crush", &[("rate", 8_000.0), ("bits", 16.0)]).as_mut(),
        &src,
    );
    let clean = process(
        configured(
            "peel.fx.crush",
            &[("rate", 8_000.0), ("bits", 16.0), ("filter", 1.0)],
        )
        .as_mut(),
        &src,
    );
    assert!(rms(&clean[2_000..]) < rms(&raw[2_000..]) * 0.1);
    let stable = process(
        configured("peel.fx.crush", &[("rate", 12_000.0)]).as_mut(),
        &src,
    );
    let jitter = process(
        configured("peel.fx.crush", &[("rate", 12_000.0), ("jitter", 0.8)]).as_mut(),
        &src,
    );
    assert!(stable.iter().zip(&jitter).any(|(a, b)| (a - b).abs() > 0.1));
}
#[test]
fn new_effects_mix_zero_preserves_stereo_exactly() {
    for id in ["peel.fx.transient", "peel.fx.crush", "peel.fx.resonator"] {
        let mut d = configured(id, &[("mix", 0.0)]);
        let (mut l, mut r) = program(8_000, 9);
        let (a, b) = (l.clone(), r.clone());
        run(d.as_mut(), &mut l, &mut r, 77, &[]);
        assert_eq!(l, a, "{id}");
        assert_eq!(r, b, "{id}");
    }
}
#[test]
fn resonator_note_names_match_tuning_and_sanitize_untrusted_values() {
    use peel_effects::resonator::notes;
    assert_eq!(notes(&[]), [48, 55, 60, 67]);
    assert_eq!(notes(&[0.0, 2.0, 3.0]), [48, 52, 55, 60]);
    assert_eq!(notes(&[2.0, 1.0, 4.0]), [38, 41, 45, 50]);
    assert_eq!(
        notes(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]),
        notes(&[])
    );
    assert_eq!(notes(&[999.0, -999.0, 999.0]), [35, 38, 42, 45]);
}
#[test]
fn resonator_rings_at_the_selected_root_and_decay_changes_tail() {
    let mut src = vec![0.0; 96_000];
    src[0] = 1.0;
    let a = process(
        configured(
            "peel.fx.resonator",
            &[
                ("chord", 0.0),
                ("mix", 1.0),
                ("decay", 3.0),
                ("damping", 0.9),
            ],
        )
        .as_mut(),
        &src,
    );
    assert!(peak(&a[500..]) > 1e-5);
    let f = 130.8128;
    let root = tone_level(&a[1_000..24_000], f);
    assert!(
        root > tone_level(&a[1_000..24_000], f * 1.04) * 3.0,
        "root {root}"
    );
    let short = process(
        configured(
            "peel.fx.resonator",
            &[
                ("chord", 0.0),
                ("mix", 1.0),
                ("decay", 0.1),
                ("damping", 0.9),
            ],
        )
        .as_mut(),
        &src,
    );
    assert!(rms(&a[24_000..48_000]) > rms(&short[24_000..48_000]) * 20.0);
}
#[test]
fn resonator_stays_bounded_with_long_undamped_feedback() {
    let mut d = configured(
        "peel.fx.resonator",
        &[("damping", 0.0), ("decay", 10.0), ("mix", 1.0)],
    );
    let (mut l, mut r) = program(144_000, 51);
    run(d.as_mut(), &mut l, &mut r, 256, &[]);
    assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 2.0));
}

#[test]
fn crush_low_bit_depths_preserve_both_polarities_and_exact_silence() {
    for bits in [1.0, 2.5, 4.0, 10.0] {
        let params = [("bits", bits), ("rate", SR), ("drive", 12.0)];
        let positive = process(configured("peel.fx.crush", &params).as_mut(), &[0.8; 1_024]);
        let negative = process(
            configured("peel.fx.crush", &params).as_mut(),
            &[-0.8; 1_024],
        );
        assert!(
            positive[1_023] > 0.9,
            "{bits} bit lost the positive half-wave"
        );
        assert!(
            negative[1_023] < -0.9,
            "{bits} bit lost the negative half-wave"
        );
        assert!(
            positive
                .iter()
                .zip(&negative)
                .all(|(a, b)| (a + b).abs() < 1e-6),
            "{bits} bit added polarity bias"
        );
        let quiet = process(configured("peel.fx.crush", &params).as_mut(), &[0.0; 1_024]);
        assert!(
            quiet.iter().all(|v| *v == 0.0),
            "{bits} bit added a zero-input offset"
        );
        let sine: Vec<_> = (0..48_000).map(|i| 0.8 * common::sine(440.0, i)).collect();
        let crushed = process(configured("peel.fx.crush", &params).as_mut(), &sine);
        let mean = crushed[24_000..].iter().map(|v| *v as f64).sum::<f64>() / 24_000.0;
        assert!(mean.abs() < 1e-4, "{bits} bit sine acquired DC {mean}");
    }
}

#[test]
fn transient_sustain_keeps_shaping_the_late_decay_in_the_same_direction() {
    // This exponential tail is still audible well after Sustain Time.
    // Tightening must not let it return to unity as the envelope falls.
    let src: Vec<_> = (0..48_000)
        .map(|i| 0.6 * sine(1_000.0, i) * (-(i as f32) / (SR * 0.08)).exp())
        .collect();
    let common = [("attack", 0.0), ("sustain_time", 150.0)];
    let neutral = process(configured("peel.fx.transient", &common).as_mut(), &src);
    let mut params = common.to_vec();
    params.push(("sustain", -0.75));
    let cut = process(configured("peel.fx.transient", &params).as_mut(), &src);
    params.push(("sustain", 0.75));
    let boost = process(configured("peel.fx.transient", &params).as_mut(), &src);
    for range in [3_840..5_760, 24_000..33_600] {
        let input = rms(&neutral[range.clone()]);
        let down = rms(&cut[range.clone()]) / input;
        let up = rms(&boost[range.clone()]) / input;
        assert!(
            down < 0.85,
            "tail {range:?} swelled back to unity: cut ratio {down}"
        );
        assert!(
            up > 1.2,
            "tail {range:?} lost its sustain lift: boost ratio {up}"
        );
    }
}

#[test]
fn resonator_lowest_readout_notes_sound_at_the_advertised_pitch() {
    for root in [0.0, 7.0] {
        let values = [root, 0.0, 1.0]; // Octave 1, Octaves chord includes C0/G0.
        let note = peel_effects::resonator::notes(&values)[3];
        let expected = peel_dsp::util::key_to_hz(note as f32) as f64;
        for sr in [44_100.0, 96_000.0] {
            let mut d = configured(
                "peel.fx.resonator",
                &[
                    ("root", root),
                    ("octave", 0.0),
                    ("chord", 1.0),
                    ("mix", 1.0),
                    ("damping", 0.0),
                    ("decay", 5.0),
                ],
            );
            d.prepare(sr, 256);
            let n = (sr * 4.0) as usize;
            let mut l = vec![0.0; n];
            let mut r = l.clone();
            // A low-frequency burst charges the selected low mode without
            // depending on an impulse's vanishing energy per sub-bass bin.
            for i in 0..sr as usize {
                let x =
                    0.5 * (std::f64::consts::TAU * expected * i as f64 / sr as f64).sin() as f32;
                l[i] = x;
                r[i] = x;
            }
            let ctx = ProcessContext::new(sr);
            for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
                d.process(&ctx, &[], a, b);
            }
            let signal = &l[(sr * 1.5) as usize..(sr * 3.5) as usize];
            let magnitude = |freq: f64| {
                let (mut re, mut im) = (0.0, 0.0);
                for (i, v) in signal.iter().enumerate() {
                    let win =
                        0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / signal.len() as f64).cos();
                    let phase = std::f64::consts::TAU * freq * i as f64 / sr as f64;
                    re += *v as f64 * win * phase.cos();
                    im += *v as f64 * win * phase.sin();
                }
                (re * re + im * im).sqrt() / signal.len() as f64
            };
            let strongest = (0..101)
                .map(|i| expected * (0.95 + i as f64 * 0.001))
                .max_by(|a, b| magnitude(*a).total_cmp(&magnitude(*b)))
                .unwrap();
            assert!(
                magnitude(expected) > 1e-6,
                "MIDI {note} has no low-note resonance at {sr} Hz"
            );
            assert!(
                (strongest / expected - 1.0).abs() < 0.005,
                "MIDI {note} should ring at {expected} Hz, measured {strongest} at SR {sr}"
            );
        }
    }
}
