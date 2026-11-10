//! The 808: tuning, the punch, slides between notes, click-free retriggers,
//! ring and choke, and a drive stage that adds harmonics but no aliasing.

mod common;

use common::*;
use peel_dsp::Device;

const ID: &str = "peel.bass.808";

fn with(params: &[(&str, f32)]) -> Box<dyn Device> {
    let mut dev = fresh(ID);
    let desc = dev.descriptor();
    for (key, value) in params {
        let p = desc
            .param_by_key(key)
            .unwrap_or_else(|| panic!("unknown key {key}"));
        dev.set_param(p.id, *value);
    }
    dev
}

fn play(params: &[(&str, f32)], events: &[Timed], total: usize) -> Vec<f32> {
    let mut dev = with(params);
    let (l, r) = render(dev.as_mut(), events, total, 256);
    assert_eq!(l, r, "the 808 is mono");
    assert!(all_finite(&l));
    l
}

fn rms(b: &[f32]) -> f32 {
    (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt()
}

fn hz(key: f32) -> f32 {
    440.0 * 2f32.powf((key - 69.0) / 12.0)
}

/// Frequency from the rising zero crossings inside `buf`, as the mean
/// period between the first and last one.
fn crossing_hz(buf: &[f32]) -> f32 {
    let mut times = Vec::new();
    for i in 1..buf.len() {
        if buf[i - 1] < 0.0 && buf[i] >= 0.0 {
            let frac = -buf[i - 1] / (buf[i] - buf[i - 1]);
            times.push(i as f32 - 1.0 + frac);
        }
    }
    assert!(times.len() >= 2, "too few cycles to measure");
    SR * (times.len() - 1) as f32 / (times[times.len() - 1] - times[0])
}

/// Frequency as semitones (a key number).
fn key_of(freq: f32) -> f32 {
    69.0 + 12.0 * (freq / 440.0).log2()
}

/// Radix-2 FFT power spectrum with a 4-term Blackman-Harris window.
fn power_spectrum(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    assert!(n.is_power_of_two());
    let mut re: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let t = std::f64::consts::TAU * i as f64 / n as f64;
            let w =
                0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
            v as f64 * w
        })
        .collect();
    let mut im = vec![0.0f64; n];
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f64::consts::TAU / len as f64;
        for s in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (a, b) = (s + k, s + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
    (0..n / 2).map(|i| re[i] * re[i] + im[i] * im[i]).collect()
}

/// Level of harmonic `k` of `f0` relative to the fundamental, in dB.
fn harmonic_db(sp: &[f64], f0: f32, k: usize, n: usize) -> f32 {
    let df = SR / n as f32;
    let at = |f: f32| {
        let c = (f / df).round() as usize;
        sp[c.saturating_sub(4)..(c + 5).min(sp.len())]
            .iter()
            .cloned()
            .fold(0.0, f64::max)
    };
    (10.0 * (at(f0 * k as f32) / at(f0)).log10()) as f32
}

/// Strongest spectral line that is not a harmonic of `f0`, relative to the
/// fundamental, in dB.
fn worst_inharmonic_db(sp: &[f64], f0: f32, n: usize, below_hz: f32) -> f32 {
    let df = SR / n as f32;
    let mut harmonic = vec![false; sp.len()];
    let mut k = 1;
    while (k as f32) * f0 < SR / 2.0 + 100.0 {
        let c = (k as f32 * f0 / df).round() as isize;
        for b in (c - 8)..=(c + 8) {
            if b >= 0 && (b as usize) < harmonic.len() {
                harmonic[b as usize] = true;
            }
        }
        k += 1;
    }
    let c0 = (f0 / df).round() as usize;
    let fund = sp[c0 - 5..=c0 + 5].iter().cloned().fold(0.0, f64::max);
    let mut worst = 0.0f64;
    let top = ((below_hz / df) as usize).min(sp.len() - 1);
    for b in (60.0 / df) as usize..top {
        if !harmonic[b] && sp[b] > sp[b - 1] && sp[b] >= sp[b + 1] {
            worst = worst.max(sp[b]);
        }
    }
    (10.0 * (worst / fund).log10()) as f32
}

/// A plain sustained tone: no punch, no click, held at full level.
const STEADY: [(&str, f32); 5] = [
    ("pitch.punch", 0.0),
    ("click.level", 0.0),
    ("amp.sustain", 1.0),
    ("amp.hold", 0.0),
    ("drive.amount", 0.0),
];

#[test]
fn settles_on_the_key_after_the_punch() {
    for key in [28u8, 36, 43, 48] {
        let out = play(&[], &[note_on(0, key, 0.9, 1)], 48_000);
        let got = crossing_hz(&out[14_400..38_400]);
        let want = hz(key as f32);
        assert!(
            (got / want - 1.0).abs() < 0.004,
            "key {key}: {got} Hz, wanted {want} Hz"
        );
    }
}

#[test]
fn the_punch_starts_above_the_key_and_lands_within_its_time() {
    let out = play(
        &[
            ("pitch.punch", 24.0),
            ("pitch.punch_time", 0.05),
            ("click.level", 0.0),
        ],
        &[note_on(0, 36, 1.0, 1)],
        24_000,
    );
    let early = crossing_hz(&out[..1_200]);
    let late = crossing_hz(&out[9_600..24_000]);
    assert!(early > late * 1.5, "early {early} Hz, settled {late} Hz");
    assert!(
        (late / hz(36.0) - 1.0).abs() < 0.005,
        "settled at {late} Hz"
    );
    // No punch: the first cycles are already on pitch.
    let flat = play(
        &[("pitch.punch", 0.0), ("click.level", 0.0)],
        &[note_on(0, 36, 1.0, 1)],
        24_000,
    );
    let first = crossing_hz(&flat[..4_800]);
    assert!(
        (first / hz(36.0) - 1.0).abs() < 0.01,
        "flat start {first} Hz"
    );
}

#[test]
fn legato_slides_in_a_straight_line_without_restarting() {
    let mut params = STEADY.to_vec();
    params.extend([("glide.mode", 1.0), ("glide.time", 0.2)]);
    let at = 24_000;
    let out = play(
        &params,
        &[
            note_on(0, 36, 0.9, 1),
            note_on(at, 43, 0.9, 2),
            note_off(at + 2_000, 36, 1),
        ],
        48_000,
    );
    // Seven semitones over 200 ms: a quarter, a half and three quarters of
    // the way at 50, 100 and 150 ms.
    for (ms, want) in [(50.0, 37.75), (100.0, 39.5), (150.0, 41.25)] {
        let mid = at + (ms * 48.0) as usize;
        let got = key_of(crossing_hz(&out[mid - 900..mid + 900]));
        assert!(
            (got - want).abs() < 0.35,
            "{ms} ms into the slide: key {got}, wanted {want}"
        );
    }
    let end = key_of(crossing_hz(&out[at + 12_000..at + 20_000]));
    assert!((end - 43.0).abs() < 0.05, "slide ended on key {end}");
    // The drum did not restart: the level carries straight on and no sample
    // jumps.
    let before = rms(&out[at - 2_400..at]);
    let after = rms(&out[at..at + 2_400]);
    assert!((after / before - 1.0).abs() < 0.05, "{before} -> {after}");
    let step = out[at - 50..at + 400]
        .windows(2)
        .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
    assert!(step < 0.02, "sample step {step} at the slide");
}

#[test]
fn glide_time_is_the_same_for_any_interval() {
    let mut params = STEADY.to_vec();
    params.extend([("glide.mode", 1.0), ("glide.time", 0.1)]);
    for to in [38u8, 48] {
        let out = play(
            &params,
            &[note_on(0, 36, 0.9, 1), note_on(12_000, to, 0.9, 2)],
            36_000,
        );
        let mid = key_of(crossing_hz(
            &out[12_000 + 2_400 - 700..12_000 + 2_400 + 700],
        ));
        let want = 36.0 + (to as f32 - 36.0) * 0.5;
        assert!((mid - want).abs() < 0.5, "to {to}: halfway key {mid}");
        let end = key_of(crossing_hz(&out[12_000 + 5_400..12_000 + 12_000]));
        assert!((end - to as f32).abs() < 0.05, "to {to}: ended on {end}");
    }
}

#[test]
fn legato_only_slides_between_overlapping_notes() {
    let mut params = STEADY.to_vec();
    params.extend([("glide.mode", 1.0), ("glide.time", 0.3)]);
    let out = play(
        &params,
        &[
            note_on(0, 36, 0.9, 1),
            note_off(6_000, 36, 1),
            note_on(24_000, 48, 0.9, 2),
        ],
        36_000,
    );
    let start = key_of(crossing_hz(&out[24_000..26_400]));
    assert!((start - 48.0).abs() < 0.1, "separate note began at {start}");
}

#[test]
fn always_slides_in_from_the_previous_note() {
    let mut params = STEADY.to_vec();
    params.extend([("glide.mode", 2.0), ("glide.time", 0.3)]);
    let out = play(
        &params,
        &[
            note_on(0, 36, 0.9, 1),
            note_off(6_000, 36, 1),
            note_on(24_000, 48, 0.9, 2),
        ],
        48_000,
    );
    let start = key_of(crossing_hz(&out[24_000..26_400]));
    assert!(
        (36.5..39.5).contains(&start),
        "the new note should begin near the old pitch, began at {start}"
    );
    let end = key_of(crossing_hz(&out[40_000..48_000]));
    assert!((end - 48.0).abs() < 0.05, "ended on {end}");
}

#[test]
fn off_restarts_every_note_on_its_own_pitch_without_a_click() {
    let mut params = STEADY.to_vec();
    params.extend([("glide.mode", 0.0), ("glide.time", 0.3)]);
    // The second note lands while the first is at a peak of its cycle.
    for at in [10_000usize, 10_183, 10_367, 10_551] {
        let out = play(
            &params,
            &[note_on(0, 36, 1.0, 1), note_on(at, 41, 1.0, 2)],
            24_000,
        );
        let start = key_of(crossing_hz(&out[at + 400..at + 4_000]));
        assert!((start - 41.0).abs() < 0.1, "restart began at key {start}");
        let step = out[at - 20..at + 600]
            .windows(2)
            .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        // A 87 Hz sine at this level moves under 0.01 per sample; a hard
        // phase reset would step by up to its full amplitude.
        assert!(step < 0.03, "note at {at}: sample step {step}");
    }
}

#[test]
fn a_restart_from_silence_begins_at_zero() {
    let out = play(&[], &[note_on(100, 36, 1.0, 1)], 2_000);
    assert_eq!(peak(&out[..100]), 0.0);
    assert!(
        out[100].abs() < 0.01 && out[101].abs() < 0.02,
        "{:?}",
        &out[100..104]
    );
    assert!(peak(&out[100..1_000]) > 0.3);
}

#[test]
fn release_gates_and_ring_runs_out() {
    let events = [note_on(0, 36, 1.0, 1), note_off(4_800, 36, 1)];
    let gated = play(
        &[("amp.note_off", 0.0), ("amp.release", 0.05)],
        &events,
        48_000,
    );
    let ringing = play(
        &[("amp.note_off", 1.0), ("amp.release", 0.05)],
        &events,
        48_000,
    );
    assert!(rms(&gated[12_000..24_000]) < 1.0e-4, "release left a tail");
    assert!(rms(&ringing[12_000..24_000]) > 0.05, "ring was cut short");
    // Until the key comes up they are the same drum.
    assert_eq!(&gated[..4_800], &ringing[..4_800]);
}

#[test]
fn a_short_release_chokes_within_milliseconds() {
    let out = play(
        &[("amp.release", 0.005)],
        &[note_on(0, 36, 1.0, 1), note_off(9_600, 36, 1)],
        14_400,
    );
    assert!(rms(&out[8_000..9_600]) > 0.2);
    assert!(
        peak(&out[9_600 + 480..]) < 2.0e-3,
        "still sounding 10 ms on"
    );
}

#[test]
fn ring_lets_go_of_a_sustained_note() {
    let mut dev = with(&[
        ("amp.note_off", 1.0),
        ("amp.sustain", 0.8),
        ("amp.decay", 0.4),
    ]);
    let events = [note_on(0, 36, 1.0, 1), note_off(24_000, 36, 1)];
    let (l, _) = render(dev.as_mut(), &events, 96_000, 256);
    assert!(rms(&l[16_000..24_000]) > 0.2, "sustain did not hold");
    assert!(dev.is_silent(), "a released ring note must still end");
}

#[test]
fn sustain_holds_while_the_key_is_down() {
    let held = play(
        &[
            ("amp.sustain", 0.7),
            ("amp.decay", 0.3),
            ("amp.hold", 0.2),
            ("drive.amount", 0.0),
            ("pitch.punch", 0.0),
            ("click.level", 0.0),
        ],
        &[note_on(0, 36, 1.0, 1)],
        96_000,
    );
    let late = rms(&held[72_000..96_000]);
    let early = rms(&held[2_400..7_200]);
    assert!((late / early - 0.7).abs() < 0.05, "{early} -> {late}");
}

#[test]
fn hold_keeps_the_drum_at_full_level() {
    let base = [
        ("pitch.punch", 0.0),
        ("click.level", 0.0),
        ("amp.decay", 0.3),
    ];
    let mut held = base.to_vec();
    held.push(("amp.hold", 0.5));
    let mut none = base.to_vec();
    none.push(("amp.hold", 0.0));
    let a = play(&held, &[note_on(0, 36, 1.0, 1)], 36_000);
    let b = play(&none, &[note_on(0, 36, 1.0, 1)], 36_000);
    let (start, later) = (rms(&a[1_500..6_000]), rms(&a[16_000..22_000]));
    assert!(
        (later / start - 1.0).abs() < 0.03,
        "held: {start} -> {later}"
    );
    assert!(
        rms(&b[16_000..22_000]) < start * 0.05,
        "no hold should have decayed"
    );
}

#[test]
fn decay_track_shortens_higher_notes() {
    let length = |key: u8, track: f32| {
        let out = play(
            &[
                ("amp.decay_track", track),
                ("amp.decay", 1.0),
                ("amp.hold", 0.0),
            ],
            &[note_on(0, key, 1.0, 1)],
            96_000,
        );
        let top = peak(&out);
        out.iter().rposition(|x| x.abs() > top * 0.05).unwrap_or(0)
    };
    let (low, high) = (length(36, 1.0), length(48, 1.0));
    // An octave up with full tracking halves the decay.
    assert!(
        (high as f32 / low as f32 - 0.5).abs() < 0.08,
        "{low} vs {high} samples"
    );
    let (low, high) = (length(36, 0.0), length(48, 0.0));
    assert!(
        (high as f32 / low as f32 - 1.0).abs() < 0.05,
        "{low} vs {high}"
    );
}

#[test]
fn a_clean_sub_is_a_sine() {
    let n = 32_768;
    let out = play(&STEADY, &[note_on(0, 45, 1.0, 1)], 8_000 + n);
    let sp = power_spectrum(&out[8_000..]);
    let f0 = hz(45.0);
    for k in 2..=6 {
        let db = harmonic_db(&sp, f0, k, n);
        assert!(db < -70.0, "harmonic {k} at {db} dB");
    }
}

#[test]
fn shape_octave_and_drive_add_harmonics() {
    let n = 32_768;
    let f0 = hz(45.0);
    let level = |extra: &[(&str, f32)], k: usize| {
        let mut params = STEADY.to_vec();
        params.extend_from_slice(extra);
        let out = play(&params, &[note_on(0, 45, 1.0, 1)], 8_000 + n);
        harmonic_db(&power_spectrum(&out[8_000..]), f0, k, n)
    };
    // The triangle brings odd harmonics, the octave partial the second.
    assert!(level(&[("osc.shape", 1.0)], 3) > -25.0);
    assert!(level(&[("osc.shape", 1.0)], 2) < -60.0);
    assert!(level(&[("osc.octave", 1.0)], 2) > -12.0);
    // Symmetric shapers make odd harmonics; the tube adds even ones too.
    for kind in [0.0, 2.0, 3.0] {
        let third = level(&[("drive.type", kind), ("drive.amount", 0.8)], 3);
        assert!(third > -22.0, "type {kind}: third harmonic {third} dB");
    }
    let second = level(&[("drive.type", 1.0), ("drive.amount", 0.6)], 2);
    assert!(second > -30.0, "tube second harmonic {second} dB");
    let second = level(&[("drive.type", 0.0), ("drive.amount", 0.6)], 2);
    assert!(second < -60.0, "warm second harmonic {second} dB");
}

#[test]
fn heavy_drive_does_not_alias() {
    let n = 32_768;
    for key in [60u8, 84] {
        for kind in 0..4 {
            let mut params = STEADY.to_vec();
            params.extend([
                ("drive.type", kind as f32),
                ("drive.amount", 1.0),
                ("osc.shape", 0.5),
                ("tone.cutoff", 18_000.0),
            ]);
            let out = play(&params, &[note_on(0, key, 1.0, 1)], 8_000 + n);
            let sp = power_spectrum(&out[8_000..]);
            let db = worst_inharmonic_db(&sp, hz(key as f32), n, 24_000.0);
            assert!(db < -60.0, "key {key}, type {kind}: alias at {db} dB");
        }
    }
}

#[test]
fn tone_darkens_and_click_brightens() {
    let treble = |params: &[(&str, f32)]| {
        let out = play(params, &[note_on(0, 36, 1.0, 1)], 4_800);
        out.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>()
    };
    let driven = [
        ("drive.type", 2.0),
        ("drive.amount", 0.8),
        ("click.level", 0.0),
    ];
    let mut open = driven.to_vec();
    open.push(("tone.cutoff", 12_000.0));
    let mut dark = driven.to_vec();
    dark.push(("tone.cutoff", 150.0));
    assert!(treble(&open) > treble(&dark) * 1.5);
    let quiet = treble(&[("click.level", 0.0), ("drive.amount", 0.0)]);
    let clicky = treble(&[("click.level", 1.0), ("drive.amount", 0.0)]);
    assert!(clicky > quiet * 1.5, "{quiet} vs {clicky}");
}

#[test]
fn asymmetric_drive_leaves_no_dc() {
    let out = play(
        &[
            ("drive.type", 1.0),
            ("drive.amount", 1.0),
            ("amp.sustain", 1.0),
        ],
        &[note_on(0, 36, 1.0, 1)],
        96_000,
    );
    let tail = &out[48_000..];
    let mean = tail.iter().sum::<f32>() / tail.len() as f32;
    assert!(mean.abs() < 0.003, "dc {mean}");
}

#[test]
fn letting_go_of_the_top_note_slides_back_to_the_held_one() {
    let mut params = STEADY.to_vec();
    params.extend([("glide.mode", 1.0), ("glide.time", 0.05)]);
    let out = play(
        &params,
        &[
            note_on(0, 36, 0.9, 1),
            note_on(8_000, 43, 0.9, 2),
            note_off(20_000, 43, 2),
        ],
        40_000,
    );
    let up = key_of(crossing_hz(&out[14_000..20_000]));
    let back = key_of(crossing_hz(&out[28_000..40_000]));
    assert!(
        (up - 43.0).abs() < 0.05 && (back - 36.0).abs() < 0.05,
        "{up}, {back}"
    );
}

#[test]
fn every_preset_is_loud_enough_and_stays_below_full_scale() {
    for preset in peel_instruments::presets(ID) {
        let mut dev = fresh(ID);
        let desc = dev.descriptor();
        for (key, value) in preset.params {
            dev.set_param(desc.param_by_key(key).unwrap().id, *value);
        }
        let events = [note_on(0, 33, 1.0, 1), note_off(24_000, 33, 1)];
        let (l, _) = render(dev.as_mut(), &events, 48_000, 256);
        let (p, body) = (peak(&l), rms(&l[2_400..12_000]));
        assert!(p < 0.98, "{}: peak {p}", preset.name);
        assert!(
            (0.18..0.75).contains(&body),
            "{}: body rms {body}",
            preset.name
        );
    }
}
