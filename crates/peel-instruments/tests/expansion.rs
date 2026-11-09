//! Audible behavior of Mallet, Zither and Cross beyond catalog integrity.
mod common;
use common::*;
use peel_dsp::{Device, EventKind};

const MALLET: &str = "peel.perc.mallet";
const ZITHER: &str = "peel.strings.zither";
const CROSS: &str = "peel.synth.cross";
fn set(d: &mut dyn Device, key: &str, value: f32) {
    let id = d.descriptor().param_by_key(key).expect(key).id;
    d.set_param(id, value);
}
fn configured(id: &str, params: &[(&str, f32)]) -> Box<dyn Device> {
    let mut d = fresh(id);
    for (k, v) in params {
        set(d.as_mut(), k, *v);
    }
    d
}
fn sound(id: &str, params: &[(&str, f32)], events: &[Timed], total: usize) -> Vec<f32> {
    render(configured(id, params).as_mut(), events, total, 256).0
}
fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}
fn tone(x: &[f32], freq: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / x.len() as f64).cos();
        let p = std::f64::consts::TAU * freq * i as f64 / SR as f64;
        re += *v as f64 * w * p.cos();
        im += *v as f64 * w * p.sin();
    }
    (re * re + im * im).sqrt() / x.len() as f64
}
fn spectral_pitch(x: &[f32], want: f64) -> f64 {
    (0..101)
        .map(|i| want * (0.9 + i as f64 * 0.002))
        .max_by(|a, b| tone(x, *a).total_cmp(&tone(x, *b)))
        .unwrap()
}
fn mallet_params() -> Vec<(&'static str, f32)> {
    vec![
        ("mallet.noise", 0.0),
        ("mallet.hardness", 1.0),
        ("mallet.vel_tone", 0.0),
        ("body.material", 1.0),
        ("body.decay", 4.0),
        ("out.width", 0.0),
    ]
}
fn zither_params() -> Vec<(&'static str, f32)> {
    vec![
        ("body.type", 0.0),
        ("string.mix", 0.0),
        ("string.coupling", 0.0),
        ("string.detune", 0.0),
        ("string.damping", 0.1),
        ("string.decay", 4.0),
        ("out.width", 0.0),
    ]
}
fn cross_params() -> Vec<(&'static str, f32)> {
    vec![
        ("osc1.wave", 0.0),
        ("osc2.wave", 0.0),
        ("osc2.detune", 0.0),
        ("osc.mix", 0.0),
        ("cross.fm12", 0.0),
        ("cross.fm21", 0.0),
        ("mod.amount", 0.0),
        ("filter.cutoff", 18_000.0),
        ("filter.resonance", 0.0),
        ("filter.env", 0.0),
        ("amp.attack", 0.001),
        ("amp.sustain", 1.0),
    ]
}

#[test]
fn mallet_models_have_their_distinct_partial_families() {
    let mut p = mallet_params();
    p.push(("body.model", 0.0));
    let bar = sound(MALLET, &p, &[note_on(0, 48, 0.9, 1)], 24_000);
    p.push(("body.model", 2.0));
    let membrane = sound(MALLET, &p, &[note_on(0, 48, 0.9, 1)], 24_000);
    let f = 130.8128;
    let b = tone(&bar[1_000..20_000], f * 2.756);
    let m = tone(&membrane[1_000..20_000], f * 1.594);
    assert!(
        b > tone(&membrane[1_000..20_000], f * 2.756) * 10.0,
        "bar partial {b}"
    );
    assert!(
        m > tone(&bar[1_000..20_000], f * 1.594) * 10.0,
        "membrane partial {m}"
    );
}
#[test]
fn mallet_key_tracking_can_lock_a_percussion_pitch() {
    let mut p = mallet_params();
    p.push(("pitch.keytrack", 0.0));
    let low = sound(MALLET, &p, &[note_on(0, 48, 0.8, 1)], 24_000);
    let high = sound(MALLET, &p, &[note_on(0, 72, 0.8, 1)], 24_000);
    assert_eq!(low, high);
    p.push(("pitch.keytrack", 1.0));
    let high = sound(MALLET, &p, &[note_on(0, 72, 0.8, 1)], 24_000);
    assert!(max_diff(&low, &high) > 0.01);
}
#[test]
fn mallet_damper_release_starts_at_note_off_even_after_a_long_hold() {
    let mut p = mallet_params();
    p.extend([
        ("body.model", 2.0),
        ("body.decay", 8.0),
        ("damp.enable", 1.0),
        ("damp.release", 0.3),
    ]);
    let off = 96_000;
    let total = off + 48_000;
    let mut d = configured(MALLET, &p);
    let (l, _) = render(
        d.as_mut(),
        &[note_on(0, 48, 0.9, 1), note_off(off, 48, 1)],
        total,
        256,
    );
    let before = rms(&l[off - 960..off]);
    let after = rms(&l[off..off + 960]);
    assert!(
        after > before * 0.4,
        "note-off abruptly killed an old voice: before {before}, after {after}"
    );
    assert!(
        peak(&l[off + 24_000..]) < before * 0.01,
        "damper did not settle"
    );
    assert!(d.is_silent());
}
#[test]
fn zither_root_and_second_string_follow_the_keyboard_and_ratio() {
    let mut p = zither_params();
    p.extend([
        ("string.mix", 1.0),
        ("string.ratio", 1.0),
        ("out.width", 1.0),
    ]);
    let mut d = configured(ZITHER, &p);
    let (l, r) = render(d.as_mut(), &[note_on(0, 57, 0.9, 1)], 24_000, 256);
    let a = detect_pitch(&l[1_000..9_000], SR);
    let b = detect_pitch(&r[1_000..9_000], SR);
    assert!((a / 220.0 - 1.0).abs() < 0.01, "A3 string is {a} Hz");
    assert!((b / 440.0 - 1.0).abs() < 0.01, "octave string is {b} Hz");
    assert!(
        (b / a - 2.0).abs() < 0.03,
        "String 2 octave ratio is {}",
        b / a
    );
}
#[test]
fn zither_bow_sustains_after_a_natural_pluck_has_decayed() {
    let mut p = zither_params();
    p.push(("string.decay", 0.6));
    let pluck = sound(ZITHER, &p, &[note_on(0, 57, 0.8, 1)], 96_000);
    p.push(("exciter.bow", 0.7));
    let bow = sound(ZITHER, &p, &[note_on(0, 57, 0.8, 1)], 96_000);
    assert!(rms(&bow[72_000..96_000]) > 0.001, "bow did not sustain");
    assert!(rms(&bow[72_000..96_000]) > rms(&pluck[72_000..96_000]) * 100.0);
}
#[test]
fn zither_retrigger_owns_the_string_and_old_note_off_cannot_release_it() {
    let mut p = zither_params();
    p.extend([
        ("exciter.bow", 0.8),
        ("string.decay", 20.0),
        ("damp.enable", 1.0),
        ("damp.release", 0.05),
    ]);
    let events = [
        note_on(0, 57, 0.8, 1),
        note_on(12_000, 57, 0.8, 2),
        note_off(24_000, 57, 2),
    ];
    let plain = sound(ZITHER, &p, &events, 48_000);
    let old_off = [events[0], events[1], note_off(18_000, 57, 1), events[2]];
    let mut d = configured(ZITHER, &p);
    let (l, _) = render(d.as_mut(), &old_off, 48_000, 256);
    assert_eq!(l, plain, "an old note-off reached the new string ownership");
    assert!(
        peak(&l[36_000..]) < 1e-5,
        "a retrigger left a bowed voice ringing"
    );
    assert!(d.is_silent());
}
#[test]
fn zither_damper_shortens_the_release_without_cutting_the_note() {
    let mut p = zither_params();
    p.extend([
        ("string.decay", 8.0),
        ("exciter.bow", 0.7),
        ("damp.enable", 1.0),
        ("damp.release", 0.2),
    ]);
    let off = 24_000;
    let d = sound(
        ZITHER,
        &p,
        &[note_on(0, 57, 0.9, 1), note_off(off, 57, 1)],
        72_000,
    );
    assert!(rms(&d[off..off + 480]) > 1e-4, "damper cut at note-off");
    assert!(peak(&d[off + 24_000..]) < 1e-5, "damper left a tail");
}
#[test]
fn cross_fm_creates_audible_sidebands() {
    let mut p = cross_params();
    p.push(("osc2.semi", 12.0));
    let clean = sound(CROSS, &p, &[note_on(0, 57, 0.8, 1)], 24_000);
    p.push(("cross.fm21", 0.3));
    let fm = sound(CROSS, &p, &[note_on(0, 57, 0.8, 1)], 24_000);
    let side = tone(&fm[4_000..24_000], 660.0);
    assert!(
        side > tone(&clean[4_000..24_000], 660.0) * 5.0,
        "FM sideband {side}"
    );
    assert!(side > 0.005, "FM sideband too quiet {side}");
}
#[test]
fn cross_ring_produces_sum_and_difference_tones() {
    let mut p = cross_params();
    p.extend([("osc2.semi", 7.0), ("osc.mix", 1.0)]);
    let free = sound(CROSS, &p, &[note_on(0, 57, 0.8, 1)], 24_000);
    p.push(("cross.mode", 2.0));
    let ring = sound(CROSS, &p, &[note_on(0, 57, 0.8, 1)], 24_000);
    let f2 = 220.0 * 2.0f64.powf(7.0 / 12.0);
    let f = tone(&free[4_000..24_000], f2);
    assert!(tone(&ring[4_000..24_000], f2) < f * 0.1);
    assert!(tone(&ring[4_000..24_000], f2 + 220.0) > f * 0.1);
    assert!(tone(&ring[4_000..24_000], f2 - 220.0) > f * 0.1);
}
#[test]
fn cross_sync_locks_the_slave_to_the_master_and_amp_releases() {
    let mut p = cross_params();
    p.extend([("osc2.wave", 2.0), ("osc2.semi", 7.0), ("osc.mix", 1.0)]);
    let free = sound(CROSS, &p, &[note_on(0, 57, 0.8, 1)], 24_000);
    p.extend([("cross.mode", 1.0), ("amp.release", 0.1)]);
    let mut d = configured(CROSS, &p);
    let (sync, _) = render(
        d.as_mut(),
        &[note_on(0, 57, 0.8, 1), note_off(24_000, 57, 1)],
        48_000,
        256,
    );
    let master = tone(&sync[4_000..24_000], 220.0);
    assert!(
        master > tone(&free[4_000..24_000], 220.0) * 5.0,
        "slave did not lock to the master"
    );
    assert!(peak(&sync[36_000..]) < 1e-5);
    assert!(d.is_silent());
}
#[test]
fn new_instruments_bend_held_notes_by_two_semitones() {
    for (id, p) in [
        (MALLET, mallet_params()),
        (ZITHER, zither_params()),
        (CROSS, cross_params()),
    ] {
        let plain = sound(id, &p, &[note_on(0, 57, 0.8, 1)], 36_000);
        let bent = sound(
            id,
            &p,
            &[
                note_on(0, 57, 0.8, 1),
                (12_000, EventKind::PitchBend { semitones: 2.0 }),
            ],
            36_000,
        );
        assert_eq!(
            &plain[..12_000],
            &bent[..12_000],
            "{id}: bend affected earlier samples"
        );
        let a = spectral_pitch(&plain[24_000..36_000], 220.0);
        let want = 220.0 * 2.0f64.powf(2.0 / 12.0);
        let b = spectral_pitch(&bent[24_000..36_000], want);
        assert!(
            (b / a / 2.0f64.powf(2.0 / 12.0) - 1.0).abs() < 0.015,
            "{id}: pitch bend ratio {}",
            b / a
        );
    }
}
#[test]
fn new_instrument_pressure_is_live_key_specific_and_block_independent() {
    for (id, p) in [
        (MALLET, mallet_params()),
        (ZITHER, zither_params()),
        (CROSS, cross_params()),
    ] {
        let base = [note_on(0, 57, 0.8, 1)];
        let other = [
            base[0],
            (
                8_000,
                EventKind::PolyPressure {
                    key: 62,
                    value: 1.0,
                },
            ),
        ];
        let pressed = [
            base[0],
            (
                8_000,
                EventKind::PolyPressure {
                    key: 57,
                    value: 1.0,
                },
            ),
        ];
        let plain = sound(id, &p, &base, 24_000);
        let unrelated = sound(id, &p, &other, 24_000);
        assert_eq!(plain, unrelated, "{id}: pressure changed another key");
        let out = sound(id, &p, &pressed, 24_000);
        assert_eq!(&plain[..8_000], &out[..8_000]);
        assert!(
            rms(&out[16_000..24_000]) > rms(&plain[16_000..24_000]) * 1.25,
            "{id}: held note did not respond to pressure"
        );
        let a = render(configured(id, &p).as_mut(), &pressed, 24_000, 77).0;
        assert!(
            max_diff(&a, &out) < 1e-5,
            "{id}: expression depends on block boundaries"
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum ExpressionReset {
    Device,
    Panic,
    Prepare,
}

fn reset_restores_neutral_expression(reset: ExpressionReset) {
    use peel_dsp::{DeviceEvent, ProcessContext};
    for (id, params) in [
        (MALLET, mallet_params()),
        (ZITHER, zither_params()),
        (CROSS, cross_params()),
    ] {
        let mut plain = configured(id, &params);
        let mut moved = configured(id, &params);
        let on = note_on(0, 57, 0.8, 1);
        render(plain.as_mut(), &[on], 12_000, 256);
        render(
            moved.as_mut(),
            &[
                on,
                (100, EventKind::PitchBend { semitones: 5.0 }),
                (100, EventKind::ChannelPressure { value: 1.0 }),
                (
                    100,
                    EventKind::PolyPressure {
                        key: 57,
                        value: 1.0,
                    },
                ),
            ],
            12_000,
            256,
        );
        let values: Vec<_> = moved
            .descriptor()
            .params
            .iter()
            .map(|p| moved.param(p.id))
            .collect();
        for d in [&mut plain, &mut moved] {
            match reset {
                ExpressionReset::Device => d.reset(),
                ExpressionReset::Prepare => d.prepare(SR, 256),
                ExpressionReset::Panic => {
                    let mut l = [1.0; 64];
                    let mut r = [1.0; 64];
                    d.process(
                        &ProcessContext::new(SR),
                        &[DeviceEvent {
                            offset: 0,
                            kind: EventKind::AllSoundOff,
                        }],
                        &mut l,
                        &mut r,
                    );
                    assert_eq!(l, [0.0; 64], "{id}: panic left audible output");
                    assert_eq!(r, [0.0; 64]);
                }
            }
            assert!(d.is_silent(), "{id}: {reset:?} left active voices");
        }
        let note = [note_on(0, 57, 0.8, 2)];
        let expected = render(plain.as_mut(), &note, 24_000, 256);
        let actual = render(moved.as_mut(), &note, 24_000, 256);
        assert_eq!(
            actual, expected,
            "{id}: {reset:?} left stale pitch bend or aftertouch on the next note"
        );
        let after: Vec<_> = moved
            .descriptor()
            .params
            .iter()
            .map(|p| moved.param(p.id))
            .collect();
        assert_eq!(after, values, "{id}: {reset:?} changed saved parameters");
    }
}

#[test]
fn new_instrument_reset_clears_pitch_bend_and_pressure() {
    reset_restores_neutral_expression(ExpressionReset::Device);
}

#[test]
fn new_instrument_panic_clears_pitch_bend_and_pressure() {
    reset_restores_neutral_expression(ExpressionReset::Panic);
}

#[test]
fn new_instrument_prepare_clears_pitch_bend_and_pressure() {
    reset_restores_neutral_expression(ExpressionReset::Prepare);
}

#[test]
fn zither_stays_in_tune_across_registers_and_sample_rates() {
    use peel_dsp::{DeviceEvent, ProcessContext};
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        for key in [36u8, 45, 60, 69] {
            let mut d = configured(ZITHER, &zither_params());
            d.prepare(sr, 256);
            let n = sr as usize;
            let mut l = vec![0.0; n];
            let mut r = vec![0.0; n];
            for start in (0..n).step_by(256) {
                let end = (start + 256).min(n);
                let event = [DeviceEvent {
                    offset: 0,
                    kind: EventKind::NoteOn {
                        key,
                        velocity: 0.7,
                        voice: 1,
                    },
                }];
                d.process(
                    &ProcessContext::new(sr),
                    if start == 0 { &event } else { &[] },
                    &mut l[start..end],
                    &mut r[start..end],
                );
            }
            let expected = f64::from(peel_dsp::util::key_to_hz(key as f32));
            let signal = &l[n / 10..n * 4 / 5];
            let level = |hz: f64| {
                let (mut re, mut im) = (0.0, 0.0);
                for (i, &x) in signal.iter().enumerate() {
                    let win =
                        0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / signal.len() as f64).cos();
                    let phase = std::f64::consts::TAU * hz * i as f64 / f64::from(sr);
                    re += f64::from(x) * win * phase.cos();
                    im += f64::from(x) * win * phase.sin();
                }
                re * re + im * im
            };
            let actual = (0..161)
                .map(|i| expected * (0.98 + i as f64 * 0.00025))
                .max_by(|a, b| level(*a).total_cmp(&level(*b)))
                .unwrap();
            let cents = 1200.0 * (actual / expected).log2();
            assert!(
                cents.abs() < 5.0,
                "key {key} at {sr}: {actual} Hz vs {expected} Hz ({cents:.1} cents)"
            );
        }
    }
}

#[test]
fn zither_position_changes_every_exciter() {
    for kind in 0..3 {
        let mut p = zither_params();
        p.extend([("exciter.type", kind as f32), ("exciter.position", 0.08)]);
        let a = sound(ZITHER, &p, &[note_on(0, 60, 0.8, 1)], 24_000);
        p.push(("exciter.position", 0.4));
        let b = sound(ZITHER, &p, &[note_on(0, 60, 0.8, 1)], 24_000);
        assert!(
            max_diff(&a, &b) > 0.02,
            "Position did not change exciter {kind}"
        );
    }
}
