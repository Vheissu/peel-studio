//! Tests of individual instruments' signature behavior.

mod common;

use std::sync::Arc;

use common::*;
use peel_dsp::device::apply_defaults;
use peel_dsp::{Device, DeviceEvent, EventKind, ProcessContext, SampleBuffer};

fn set(dev: &mut dyn Device, key: &str, value: f32) {
    let id = dev
        .descriptor()
        .param_by_key(key)
        .unwrap_or_else(|| panic!("no param {key}"))
        .id;
    dev.set_param(id, value);
}

/// RMS in consecutive windows of `win` samples.
fn rms_windows(buf: &[f32], win: usize) -> Vec<f32> {
    buf.chunks(win)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect()
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len().max(1) as f32).sqrt()
}

/// Magnitude of the component of `buf` at `freq` (Goertzel).
fn tone_level(buf: &[f32], freq: f32) -> f32 {
    let w = std::f64::consts::TAU * freq as f64 / SR as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &v) in buf.iter().enumerate() {
        // Hann window to keep leakage from neighbouring partials low.
        let win = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / buf.len() as f64).cos();
        re += v as f64 * win * (w * i as f64).cos();
        im += v as f64 * win * (w * i as f64).sin();
    }
    ((re * re + im * im).sqrt() / buf.len() as f64) as f32
}

/// RMS of the first difference over RMS of the signal: grows with
/// brightness (a pure low sine scores near zero, a saw or click far higher).
fn brightness(buf: &[f32]) -> f32 {
    let d: f32 = buf.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
    let s: f32 = buf.iter().map(|x| x * x).sum();
    (d / s.max(1e-12)).sqrt()
}

fn ramp_sample(frames: usize, rate: f32) -> Arc<SampleBuffer> {
    let ch: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    let mut s = SampleBuffer::new("ramp", rate, vec![ch.clone(), ch]);
    s.root_key = 60;
    Arc::new(s)
}

fn sampler_with(sample: Arc<SampleBuffer>) -> Box<dyn Device> {
    let mut d = peel_instruments::create("peel.sampler").unwrap();
    d.prepare(SR, 256);
    apply_defaults(d.as_mut());
    d.set_sample(Some(sample));
    d
}

// ---------------------------------------------------------------- sampler

#[test]
fn sampler_is_silent_without_a_sample() {
    let mut d = peel_instruments::create("peel.sampler").unwrap();
    d.prepare(SR, 256);
    apply_defaults(d.as_mut());
    let (l, r) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 4_000, 256);
    assert_eq!(peak(&l).max(peak(&r)), 0.0);
    assert!(d.is_silent());
}

#[test]
fn sampler_hands_back_the_previous_sample() {
    let mut d = peel_instruments::create("peel.sampler").unwrap();
    let a = test_sample();
    let b = test_sample();
    assert!(d.set_sample(Some(a.clone())).is_none());
    let back = d.set_sample(Some(b.clone())).expect("previous sample");
    assert!(Arc::ptr_eq(&back, &a));
    let back = d.set_sample(None).expect("previous sample");
    assert!(Arc::ptr_eq(&back, &b));
    assert!(d.set_sample(None).is_none());
}

#[test]
fn sampler_plays_at_the_root_key_pitch_and_transposes() {
    // The test sample is a 220 Hz sine recorded at 44.1 kHz, root key 57.
    for (key, expect) in [
        (57u8, 220.0f32),
        (69, 440.0),
        (45, 110.0),
        (64, 220.0 * 2f32.powf(7.0 / 12.0)),
    ] {
        let mut d = fresh("peel.sampler");
        let (l, _) = render(d.as_mut(), &[note_on(0, key, 1.0, 1)], 30_000, 256);
        let f = detect_pitch(&l[3_000..], SR);
        assert!(
            (f / expect - 1.0).abs() < 0.01,
            "key {key}: {f} Hz, expected {expect} Hz"
        );
    }
}

#[test]
fn sampler_fine_tune_and_key_tracking() {
    let mut d = fresh("peel.sampler");
    set(d.as_mut(), "sample.keytrack", 0.0);
    let (l, _) = render(d.as_mut(), &[note_on(0, 84, 1.0, 1)], 30_000, 256);
    let f = detect_pitch(&l[3_000..], SR);
    assert!(
        (f / 220.0 - 1.0).abs() < 0.01,
        "untracked key played {f} Hz"
    );

    let mut d = fresh("peel.sampler");
    set(d.as_mut(), "sample.tune", 12.0);
    let (l, _) = render(d.as_mut(), &[note_on(0, 57, 1.0, 1)], 30_000, 256);
    let f = detect_pitch(&l[3_000..], SR);
    assert!((f / 440.0 - 1.0).abs() < 0.01, "tune +12 played {f} Hz");
}

#[test]
fn sampler_reverse_and_start_offset() {
    let sample = ramp_sample(40_000, SR);
    let play = |settings: &[(&str, f32)]| {
        let mut d = sampler_with(sample.clone());
        for (k, v) in settings {
            set(d.as_mut(), k, *v);
        }
        render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 20_000, 256).0
    };
    let forward = play(&[]);
    assert!(
        forward[10_000] > forward[2_000] * 2.0,
        "forward ramp should rise"
    );
    let reversed = play(&[("sample.reverse", 1.0)]);
    assert!(
        reversed[2_000] > reversed[10_000] + 0.1,
        "reversed ramp should fall"
    );
    let offset = play(&[("sample.start", 0.5)]);
    // Starting halfway up a ramp: the level at the start is about half scale
    // (times the 0.9 voice gain).
    assert!(
        (offset[3_000] - 0.45).abs() < 0.08,
        "start offset ignored: {}",
        offset[3_000]
    );
}

#[test]
fn sampler_reproduces_the_source_at_unity_rate() {
    // Same rate, root key played: after the attack the output is the sample.
    let frames = 24_000;
    let ch: Vec<f32> = (0..frames)
        .map(|i| 0.5 * (std::f32::consts::TAU * 300.0 * i as f32 / SR).sin())
        .collect();
    let mut s = SampleBuffer::new("sine", SR, vec![ch.clone()]);
    s.root_key = 60;
    let mut d = sampler_with(Arc::new(s));
    let (l, r) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], frames, 256);
    for i in 4_000..8_000 {
        assert!(
            (l[i] - ch[i] * 0.9).abs() < 0.01,
            "frame {i}: {} vs {}",
            l[i],
            ch[i] * 0.9
        );
        assert!((l[i] - r[i]).abs() < 1e-6, "mono sample must be centered");
    }
}

#[test]
fn sampler_one_shot_ignores_note_off_but_gate_does_not() {
    let events = [note_on(0, 57, 1.0, 1), note_off(4_800, 57, 1)];
    let mut oneshot = fresh("peel.sampler");
    let (l, _) = render(oneshot.as_mut(), &events, 40_000, 256);
    assert!(
        rms(&l[20_000..30_000]) > 0.1,
        "one-shot stopped at note-off"
    );

    let mut gate = fresh("peel.sampler");
    set(gate.as_mut(), "sample.mode", 0.0);
    let (l, _) = render(gate.as_mut(), &events, 40_000, 256);
    assert!(rms(&l[20_000..30_000]) < 1e-4, "gate mode kept playing");
    assert!(rms(&l[1_000..4_000]) > 0.1);
}

#[test]
fn sampler_ends_at_the_sample_end_without_a_click() {
    let mut d = fresh("peel.sampler");
    let n = 70_000;
    let (l, _) = render(d.as_mut(), &[note_on(0, 57, 1.0, 1)], n, 256);
    assert!(d.is_silent(), "sample should have finished");
    // 1 s of 44.1 kHz material is 48000 output samples at the root key.
    let last = l.iter().rposition(|x| x.abs() > 1e-6).unwrap();
    assert!((46_000..49_000).contains(&last), "ended at {last}");
    let max_step = l[last.saturating_sub(300)..=last]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(max_step < 0.1, "click at the end: step {max_step}");
}

#[test]
fn sampler_loops_while_held_and_releases() {
    let mut s = (*test_sample()).clone();
    s.loop_range = Some((10_000, 20_000));
    let s = Arc::new(s);
    for loop_mode in [1.0, 2.0] {
        let mut d = sampler_with(s.clone());
        set(d.as_mut(), "sample.mode", 0.0);
        set(d.as_mut(), "sample.loop", loop_mode);
        let events = [note_on(0, 57, 1.0, 1), note_off(150_000, 57, 1)];
        let (l, r) = render(d.as_mut(), &events, 190_000, 512);
        assert!(all_finite(&l) && all_finite(&r));
        // Well past the 1 s sample length it is still sounding...
        assert!(
            rms(&l[100_000..140_000]) > 0.1,
            "mode {loop_mode} stopped early"
        );
        // ...and it stops after the note is released.
        assert!(
            rms(&l[180_000..]) < 1e-4,
            "mode {loop_mode} did not release"
        );
        assert!(d.is_silent());
    }
}

#[test]
fn sampler_ping_pong_reverses_direction() {
    let mut s = (*ramp_sample(8_000, SR)).clone();
    s.loop_range = Some((0, 8_000));
    let mut d = sampler_with(Arc::new(s));
    set(d.as_mut(), "sample.mode", 0.0);
    set(d.as_mut(), "sample.loop", 2.0);
    set(d.as_mut(), "amp.attack", 0.001);
    let (l, _) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 40_000, 512);
    // Forward for 8000 frames, then back down, then up again.
    assert!(l[6_000] > l[2_000]);
    assert!(l[14_000] < l[10_000], "second leg should fall");
    assert!(l[22_000] > l[18_000], "third leg should rise");
}

#[test]
fn sampler_survives_degenerate_samples_and_swaps() {
    for frames in [0usize, 1, 2, 3, 5] {
        let ch = vec![0.5f32; frames];
        let s = SampleBuffer::new("tiny", SR, vec![ch]);
        let mut d = sampler_with(Arc::new(s));
        set(d.as_mut(), "sample.loop", 1.0);
        let (l, _) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 2_000, 64);
        assert!(all_finite(&l));
    }
    // Swapping the sample mid-note stops the voices cleanly.
    let mut d = fresh("peel.sampler");
    let _ = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 2_000, 256);
    let old = d.set_sample(Some(test_sample()));
    assert!(old.is_some());
    assert!(d.is_silent());
    let (l, _) = render(d.as_mut(), &[note_on(0, 57, 1.0, 2)], 4_000, 256);
    assert!(peak(&l) > 0.1);
}

#[test]
fn sampler_filter_darkens_the_sound() {
    let play = |cutoff: f32| {
        let mut d = fresh("peel.sampler");
        set(d.as_mut(), "filter.cutoff", cutoff);
        set(d.as_mut(), "sample.tune", 24.0);
        let (l, _) = render(d.as_mut(), &[note_on(0, 57, 1.0, 1)], 20_000, 256);
        rms(&l[4_000..16_000])
    };
    assert!(play(300.0) < play(20_000.0) * 0.2);
}

// ------------------------------------------------------------------- poly

#[test]
fn poly_unison_widens_the_stereo_image() {
    let mut mono = fresh("peel.synth.poly");
    set(mono.as_mut(), "unison.voices", 0.0);
    let (l, r) = render(mono.as_mut(), &[note_on(0, 60, 1.0, 1)], 20_000, 256);
    assert!(
        max_diff(&l, &r) < 1e-6,
        "single oscillator must be centered"
    );

    let mut wide = fresh("peel.synth.poly");
    set(wide.as_mut(), "unison.voices", 3.0);
    set(wide.as_mut(), "unison.spread", 1.0);
    let (l, r) = render(wide.as_mut(), &[note_on(0, 60, 1.0, 1)], 20_000, 256);
    assert!(
        max_diff(&l, &r) > 0.02,
        "unison should be spread across the stereo field"
    );
}

#[test]
fn poly_glide_slides_between_notes() {
    let mut d = fresh("peel.synth.poly");
    set(d.as_mut(), "unison.voices", 0.0);
    set(d.as_mut(), "osc.mix", 0.0);
    set(d.as_mut(), "filter.cutoff", 8_000.0);
    set(d.as_mut(), "filter.env_amount", 0.0);
    set(d.as_mut(), "voice.glide", 0.4);
    let events = [
        note_on(0, 48, 1.0, 1),
        note_off(12_000, 48, 1),
        note_on(12_000, 60, 1.0, 2),
    ];
    let (l, _) = render(d.as_mut(), &events, 48_000, 256);
    let early = detect_pitch(&l[12_500..16_500], SR);
    let late = detect_pitch(&l[40_000..46_000], SR);
    let start = 130.81; // C3
    let target = 261.63; // C4
    assert!(
        early > start * 1.02 && early < target * 0.98,
        "glide not under way: {early}"
    );
    assert!(
        (late / target - 1.0).abs() < 0.03,
        "glide did not arrive: {late}"
    );
}

#[test]
fn poly_tempo_synced_lfo_follows_the_tempo() {
    // Amplitude LFO at 1/8 notes: 4 cycles per second at 120 bpm, 6 at 180.
    let cycles = |bpm: f64| {
        let mut d = fresh("peel.synth.poly");
        for (k, v) in [
            ("unison.voices", 0.0),
            ("osc.mix", 0.0),
            ("osc1.wave", 0.0),
            ("filter.type", 0.0),
            ("filter.cutoff", 12_000.0),
            ("filter.env_amount", 0.0),
            ("amp.attack", 0.001),
            ("amp.sustain", 1.0),
            ("lfo.sync", 5.0),
            ("lfo.shape", 0.0),
            ("lfo.depth", 1.0),
            ("lfo.target", 2.0),
        ] {
            set(d.as_mut(), k, v);
        }
        let mut l = vec![0.0; 48_000];
        let mut r = vec![0.0; 48_000];
        let mut pos = 0;
        let mut first = true;
        while pos < 48_000 {
            let n = 256.min(48_000 - pos);
            let mut ctx = ProcessContext::new(SR);
            ctx.playing = true;
            ctx.tempo_bpm = bpm;
            ctx.beat_position = pos as f64 * bpm / 60.0 / SR as f64;
            let ev = [DeviceEvent {
                offset: 0,
                kind: EventKind::NoteOn {
                    key: 60,
                    velocity: 1.0,
                    voice: 1,
                },
            }];
            d.process(
                &ctx,
                if first { &ev } else { &[] },
                &mut l[pos..pos + n],
                &mut r[pos..pos + n],
            );
            first = false;
            pos += n;
        }
        // Count rising crossings of the smoothed envelope through its mean.
        let env = rms_windows(&l[2_000..], 240);
        let mean = env.iter().sum::<f32>() / env.len() as f32;
        let mut count = 0;
        for w in env.windows(2) {
            if w[0] < mean && w[1] >= mean {
                count += 1;
            }
        }
        count
    };
    let slow = cycles(120.0);
    let fast = cycles(180.0);
    assert!((3..=5).contains(&slow), "120 bpm: {slow} cycles");
    assert!((5..=7).contains(&fast), "180 bpm: {fast} cycles");
}

#[test]
fn poly_filter_envelope_and_velocity_open_the_filter() {
    let tone = |velocity: f32| {
        let mut d = fresh("peel.synth.poly");
        set(d.as_mut(), "filter.velocity", 1.0);
        set(d.as_mut(), "out.velocity", 0.0);
        let (l, _) = render(d.as_mut(), &[note_on(0, 60, velocity, 1)], 12_000, 256);
        brightness(&l[2_000..])
    };
    assert!(
        tone(1.0) > tone(0.2) * 1.15,
        "velocity should brighten the filter"
    );
}

#[test]
fn poly_plays_sixteen_voices_and_steals_beyond() {
    let mut d = fresh("peel.synth.poly");
    let mut events = Vec::new();
    for i in 0..16u32 {
        events.push(note_on(0, 36 + 3 * i as u8, 0.7, i));
    }
    let (l, _) = render(d.as_mut(), &events, 12_000, 256);
    assert!(peak(&l) > 0.3);
    // The 17th note steals the oldest without silencing the rest.
    events.push(note_on(6_000, 100, 0.7, 99));
    let mut d = fresh("peel.synth.poly");
    let (l, _) = render(d.as_mut(), &events, 12_000, 256);
    assert!(all_finite(&l));
    assert!(rms(&l[8_000..12_000]) > 0.05);
}

#[test]
fn poly_voice_steal_does_not_click() {
    // Sixteen pure sine voices, then a seventeenth note steals one. The
    // steepest step in the output must not jump when that happens.
    let mut d = fresh("peel.synth.poly");
    for (k, v) in [
        ("unison.voices", 0.0),
        ("osc.mix", 0.0),
        ("osc1.wave", 0.0),
        ("filter.type", 0.0),
        ("filter.cutoff", 12_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("amp.attack", 0.001),
        ("amp.sustain", 1.0),
        ("out.gain", -12.0),
    ] {
        set(d.as_mut(), k, v);
    }
    let mut events: Vec<Timed> = (0..16u32)
        .map(|i| note_on(0, 36 + 2 * i as u8, 1.0, i))
        .collect();
    events.push(note_on(30_000, 90, 1.0, 99));
    let (l, _) = render(d.as_mut(), &events, 40_000, 256);
    let step = |a: &[f32]| {
        a.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    };
    let steady = step(&l[10_000..29_000]);
    let around = step(&l[29_900..33_000]);
    assert!(
        around < steady * 1.8 + 0.005,
        "steal clicked: {around} vs steady {steady}"
    );
}

// ------------------------------------------------------------------- bass

#[test]
fn bass_is_monophonic_with_last_note_priority() {
    let mut d = fresh("peel.synth.bass");
    set(d.as_mut(), "filter.cutoff", 4_000.0);
    set(d.as_mut(), "filter.env_amount", 0.0);
    set(d.as_mut(), "osc.sub", 0.0);
    set(d.as_mut(), "osc.wave", 3.0);
    let events = [
        note_on(0, 36, 0.8, 1),
        note_on(12_000, 43, 0.8, 2),
        note_off(30_000, 43, 2),
        note_off(48_000, 36, 1),
    ];
    let (l, _) = render(d.as_mut(), &events, 60_000, 256);
    let first = detect_pitch(&l[6_000..11_000], SR);
    let second = detect_pitch(&l[20_000..28_000], SR);
    let back = detect_pitch(&l[36_000..46_000], SR);
    assert!((first / 65.41 - 1.0).abs() < 0.03, "first note {first}");
    assert!((second / 98.0 - 1.0).abs() < 0.03, "second note {second}");
    assert!(
        (back / 65.41 - 1.0).abs() < 0.03,
        "should fall back to the held note: {back}"
    );
    assert!(
        rms(&l[56_000..]) < 1e-3,
        "note off for the last key must release"
    );
}

#[test]
fn bass_slide_glides_only_between_overlapping_notes() {
    let play = |overlap: bool| {
        let mut d = fresh("peel.synth.bass");
        set(d.as_mut(), "voice.mode", 2.0);
        set(d.as_mut(), "voice.glide", 0.3);
        set(d.as_mut(), "filter.cutoff", 4_000.0);
        set(d.as_mut(), "filter.env_amount", 0.0);
        set(d.as_mut(), "osc.sub", 0.0);
        let off1 = if overlap { 20_000 } else { 9_000 };
        let events = [
            note_on(0, 36, 0.8, 1),
            note_off(off1, 36, 1),
            note_on(12_000, 48, 0.8, 2),
            note_off(40_000, 48, 2),
        ];
        let (l, _) = render(d.as_mut(), &events, 44_000, 256);
        detect_pitch(&l[12_500..15_500], SR)
    };
    let jumped = play(false);
    let slid = play(true);
    assert!(
        (jumped / 130.81 - 1.0).abs() < 0.04,
        "non-overlapping notes should jump: {jumped}"
    );
    assert!(
        slid < 130.81 * 0.95,
        "overlapping notes should glide up: {slid}"
    );
}

#[test]
fn bass_accent_deepens_the_filter_sweep() {
    let tone = |velocity: f32| {
        let mut d = fresh("peel.synth.bass");
        for (k, v) in [
            ("osc.wave", 0.0),
            ("filter.cutoff", 250.0),
            ("filter.env_amount", 2.0),
            ("filter.accent", 1.0),
            ("out.velocity", 0.0),
        ] {
            set(d.as_mut(), k, v);
        }
        let (l, _) = render(d.as_mut(), &[note_on(0, 36, velocity, 1)], 8_000, 256);
        brightness(&l[100..2_400])
    };
    assert!(
        tone(1.0) > tone(0.5) * 1.2,
        "accent should open the filter more: {} vs {}",
        tone(1.0),
        tone(0.5)
    );
}

#[test]
fn bass_resonance_makes_the_acid_squelch() {
    let squelch = |res: f32| {
        let mut d = fresh("peel.synth.bass");
        for (k, v) in [
            ("osc.wave", 0.0),
            ("filter.cutoff", 300.0),
            ("filter.env_amount", 3.0),
            ("filter.decay", 0.3),
            ("filter.resonance", res),
        ] {
            set(d.as_mut(), k, v);
        }
        let (l, _) = render(d.as_mut(), &[note_on(0, 36, 1.0, 1)], 24_000, 256);
        assert!(all_finite(&l) && peak(&l) < 3.0);
        // Spectral movement: brightness early vs late in the sweep.
        brightness(&l[500..4_000]) / brightness(&l[16_000..24_000]).max(1e-6)
    };
    assert!(squelch(0.8) > 1.3, "filter sweep should be audible");
}

// ------------------------------------------------------------------- drums

#[test]
fn kick_fundamental_follows_the_pitch_parameter() {
    let mut d = fresh("peel.drum.kick");
    for (k, v) in [
        ("pitch.freq", 45.0),
        ("pitch.sweep", 0.0),
        ("body.decay", 1.5),
        ("body.drive", 0.0),
        ("click.level", 0.0),
    ] {
        set(d.as_mut(), k, v);
    }
    let (l, _) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 30_000, 256);
    let f = detect_pitch(&l[3_000..20_000], SR);
    assert!((f / 45.0 - 1.0).abs() < 0.03, "kick at {f} Hz");
    // Playing an octave up doubles it.
    let mut d = fresh("peel.drum.kick");
    for (k, v) in [
        ("pitch.freq", 45.0),
        ("pitch.sweep", 0.0),
        ("body.decay", 1.5),
        ("body.drive", 0.0),
        ("click.level", 0.0),
    ] {
        set(d.as_mut(), k, v);
    }
    let (l, _) = render(d.as_mut(), &[note_on(0, 72, 1.0, 1)], 30_000, 256);
    let f = detect_pitch(&l[3_000..20_000], SR);
    assert!((f / 90.0 - 1.0).abs() < 0.03, "kick an octave up at {f} Hz");
}

#[test]
fn kick_pitch_sweeps_downward() {
    let mut d = fresh("peel.drum.kick");
    let (l, _) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 24_000, 256);
    let early = detect_pitch(&l[400..2_000], SR);
    let late = detect_pitch(&l[6_000..16_000], SR);
    assert!(early > late * 1.4, "no sweep: {early} -> {late}");
}

#[test]
fn kick_808_rings_much_longer_than_punch() {
    let tail = |preset: &str| {
        let mut d = fresh("peel.drum.kick");
        let desc = d.descriptor();
        let p = peel_instruments::presets("peel.drum.kick")
            .iter()
            .find(|p| p.name == preset)
            .unwrap();
        for (k, v) in p.params {
            d.set_param(desc.param_by_key(k).unwrap().id, *v);
        }
        let (l, _) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 48_000, 256);
        rms(&l[24_000..36_000])
    };
    assert!(tail("808 Long") > tail("Punch") * 10.0);
}

#[test]
fn snare_has_a_tonal_body_and_a_noisy_top() {
    let mut d = fresh("peel.drum.snare");
    let (l, _) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 12_000, 256);
    let body = tone_level(&l[0..4_000], 190.0);
    let top = brightness(&l[0..4_000]);
    assert!(body > 0.005, "no body tone: {body}");
    assert!(top > 0.2, "no noise on top: {top}");
}

#[test]
fn hat_retrigger_chokes_the_previous_hit() {
    let events_open = [note_on(0, 60, 1.0, 1)];
    let mut open = fresh("peel.drum.hat");
    set(open.as_mut(), "amp.decay", 1.2);
    let (l, _) = render(open.as_mut(), &events_open, 24_000, 256);
    let ringing = rms(&l[9_000..12_000]);
    assert!(ringing > 0.002, "open hat should ring: {ringing}");

    // A very short hit at 4800 chokes the open one.
    let mut chokes = fresh("peel.drum.hat");
    set(chokes.as_mut(), "amp.decay", 1.2);
    let short = chokes.descriptor().param_by_key("amp.decay").unwrap().id;
    let events = [
        note_on(0, 60, 1.0, 1),
        (
            4_799,
            EventKind::Param {
                id: short,
                value: 0.015,
            },
        ),
        note_on(4_800, 60, 1.0, 2),
    ];
    let (l, _) = render(chokes.as_mut(), &events, 24_000, 256);
    let after = rms(&l[9_000..12_000]);
    assert!(
        after < ringing * 0.05,
        "previous hit not choked: {after} vs {ringing}"
    );
    assert!(rms(&l[4_810..5_200]) > 0.002, "new hit must sound");
}

#[test]
fn hat_gate_cuts_on_release_when_enabled() {
    let tail = |gate: f32| {
        let mut d = fresh("peel.drum.hat");
        set(d.as_mut(), "amp.decay", 1.2);
        set(d.as_mut(), "amp.gate", gate);
        let (l, _) = render(
            d.as_mut(),
            &[note_on(0, 60, 1.0, 1), note_off(2_400, 60, 1)],
            24_000,
            256,
        );
        rms(&l[9_000..12_000])
    };
    assert!(tail(1.0) < tail(0.0) * 0.05);
}

#[test]
fn clap_has_multiple_bursts_and_a_wide_tail() {
    let mut d = fresh("peel.drum.clap");
    set(d.as_mut(), "burst.count", 3.0); // five bursts
    set(d.as_mut(), "burst.spread", 0.012);
    let (l, r) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 12_000, 64);
    // Count separate energy bursts in 2 ms windows over the first 60 ms.
    let env = rms_windows(&l[..2_880], 96);
    let max = env.iter().cloned().fold(0.0, f32::max);
    let mut peaks = 0;
    for i in 1..env.len() - 1 {
        if env[i] > env[i - 1] * 1.3 && env[i] >= env[i + 1] && env[i] > max * 0.15 {
            peaks += 1;
        }
    }
    assert!(peaks >= 3, "expected several bursts, found {peaks}");
    assert!(
        max_diff(&l[3_000..9_000], &r[3_000..9_000]) > 0.001,
        "tail should be stereo"
    );
}

#[test]
fn tom_pitch_follows_the_parameter_and_key() {
    let mut d = fresh("peel.drum.tom");
    for (k, v) in [
        ("pitch.freq", 120.0),
        ("pitch.sweep", 0.0),
        ("body.overtone", 0.0),
        ("body.stick", 0.0),
        ("body.drive", 0.0),
    ] {
        set(d.as_mut(), k, v);
    }
    let (l, _) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 20_000, 256);
    let f = detect_pitch(&l[2_000..14_000], SR);
    assert!((f / 120.0 - 1.0).abs() < 0.03, "tom at {f} Hz");
}

// ------------------------------------------------------------------- keys

#[test]
fn tine_autopan_alternates_the_channels() {
    let mut d = fresh("peel.keys.tine");
    set(d.as_mut(), "trem.depth", 1.0);
    set(d.as_mut(), "trem.mode", 1.0);
    set(d.as_mut(), "trem.rate", 4.0);
    set(d.as_mut(), "out.width", 0.0);
    let (l, r) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 36_000, 256);
    let lr: Vec<f32> = rms_windows(&l, 960)
        .iter()
        .zip(rms_windows(&r, 960))
        .map(|(a, b)| (a + 1e-6) / (b + 1e-6))
        .collect();
    let hi = lr.iter().cloned().fold(0.0, f32::max);
    let lo = lr.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        hi > 2.0 && lo < 0.5,
        "channels should alternate: {lo}..{hi}"
    );

    // Tremolo mode modulates both channels together.
    let mut d = fresh("peel.keys.tine");
    set(d.as_mut(), "trem.depth", 1.0);
    set(d.as_mut(), "trem.mode", 0.0);
    set(d.as_mut(), "out.width", 0.0);
    let (l, r) = render(d.as_mut(), &[note_on(0, 60, 1.0, 1)], 36_000, 256);
    assert!(max_diff(&l, &r) < 1e-6);
}

#[test]
fn tine_velocity_brightens_the_attack() {
    let tone = |v: f32| {
        let mut d = fresh("peel.keys.tine");
        let (l, _) = render(d.as_mut(), &[note_on(0, 60, v, 1)], 4_000, 256);
        brightness(&l)
    };
    assert!(tone(1.0) > tone(0.3) * 1.2);
}

#[test]
fn grand_partials_are_stretched_by_stiffness() {
    // Low note: the 10th partial should sit a bit above 10 x the fundamental.
    let mut d = fresh("peel.keys.grand");
    set(d.as_mut(), "string.detune", 0.0);
    let (l, _) = render(d.as_mut(), &[note_on(0, 36, 0.9, 1)], 48_000, 256);
    let f0 = 65.406f32;
    let seg = &l[4_000..44_000];
    let mut best = (0.0f32, 0.0f32);
    let mut f = f0 * 10.0 * 0.99;
    while f < f0 * 10.0 * 1.04 {
        let level = tone_level(seg, f);
        if level > best.0 {
            best = (level, f);
        }
        f += 0.25;
    }
    let stretch = best.1 / (f0 * 10.0);
    assert!(
        stretch > 1.005 && stretch < 1.03,
        "10th partial at {} Hz is x{stretch} of harmonic",
        best.1
    );
}

#[test]
fn grand_velocity_changes_loudness_and_brightness() {
    let play = |v: f32| {
        let mut d = fresh("peel.keys.grand");
        render(d.as_mut(), &[note_on(0, 60, v, 1)], 12_000, 256).0
    };
    let soft = play(0.25);
    let hard = play(1.0);
    assert!(rms(&hard) > rms(&soft) * 2.0);
    assert!(brightness(&hard) > brightness(&soft) * 1.1);
}

#[test]
fn grand_bass_rings_longer_than_treble() {
    let decay_time = |key: u8| {
        let mut d = fresh("peel.keys.grand");
        let (l, _) = render(d.as_mut(), &[note_on(0, key, 0.8, 1)], 48_000 * 8, 512);
        let env = rms_windows(&l, 2_400);
        let start = env[1];
        env.iter()
            .position(|&e| e < start * 0.03)
            .unwrap_or(env.len()) as f32
            * 0.05
    };
    let bass = decay_time(36);
    let treble = decay_time(84);
    assert!(bass > treble * 1.8, "bass {bass} s, treble {treble} s");
}

#[test]
fn grand_has_a_two_stage_decay() {
    let mut d = fresh("peel.keys.grand");
    let (l, _) = render(d.as_mut(), &[note_on(0, 60, 0.8, 1)], 48_000 * 5, 512);
    let db = |a: &[f32]| 20.0 * rms(a).log10();
    let early = db(&l[2_400..14_400]) - db(&l[26_400..38_400]); // dB lost 50..300 ms -> 550..800 ms
    let late = db(&l[96_000..108_000]) - db(&l[168_000..180_000]); // 2.0 s -> 3.5 s
    let early_rate = early / 0.5;
    let late_rate = late / 1.5;
    assert!(
        early_rate > late_rate * 1.3,
        "early {early_rate} dB/s, late {late_rate} dB/s"
    );
}

#[test]
fn grand_sustain_pedal_holds_released_notes() {
    let pedal = peel_instruments::descriptors()
        .iter()
        .find(|d| d.type_id == "peel.keys.grand")
        .unwrap()
        .param_by_key("pedal.sustain")
        .unwrap()
        .id;
    let n = 48_000 * 6;
    let note = [note_on(0, 60, 0.8, 1), note_off(9_600, 60, 1)];

    let mut dry = fresh("peel.keys.grand");
    let (l, _) = render(dry.as_mut(), &note, n, 512);
    let dry_level = rms(&l[48_000 * 2..48_000 * 3]);

    let mut held = fresh("peel.keys.grand");
    let events = [
        (
            0,
            EventKind::Param {
                id: pedal,
                value: 1.0,
            },
        ),
        note[0],
        note[1],
        (
            48_000 * 4,
            EventKind::Param {
                id: pedal,
                value: 0.0,
            },
        ),
    ];
    let (l, _) = render(held.as_mut(), &events, n, 512);
    let held_level = rms(&l[48_000 * 2..48_000 * 3]);
    assert!(
        held_level > dry_level * 30.0 + 1e-6,
        "pedal had no effect: {held_level} vs {dry_level}"
    );
    assert!(held_level > 0.003, "sound did not sustain: {held_level}");
    // Lifting the pedal damps the note.
    assert!(rms(&l[48_000 * 5 + 24_000..]) < held_level * 0.02);
}

#[test]
fn grand_release_damps_quickly_and_smoothly() {
    let mut d = fresh("peel.keys.grand");
    let (l, _) = render(
        d.as_mut(),
        &[note_on(0, 60, 0.8, 1), note_off(24_000, 60, 1)],
        48_000 * 3,
        256,
    );
    // Quieter than -40 dB re the held level within a second of release.
    let held = rms(&l[20_000..24_000]);
    let after = rms(&l[72_000..76_000]);
    assert!(after < held * 0.01, "damper too slow: {after} vs {held}");
    // No discontinuity at the moment of release.
    let step = l[23_990..24_020]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    let typical = l[20_000..23_990]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(
        step < typical * 1.5 + 1e-4,
        "click at release: {step} vs {typical}"
    );
}

#[test]
fn grand_sixteen_note_chord_is_cheap_and_clean() {
    let mut d = fresh("peel.keys.grand");
    let events: Vec<Timed> = (0..16u32)
        .map(|i| note_on(i as usize * 40, 40 + 3 * i as u8, 0.7, i))
        .collect();
    let started = std::time::Instant::now();
    let (l, r) = render(d.as_mut(), &events, 48_000 * 4, 256);
    let elapsed = started.elapsed().as_secs_f64();
    assert!(all_finite(&l) && all_finite(&r));
    // Four seconds of 16 notes in well under a quarter of realtime even in a
    // debug-profile test build (dependencies are optimized).
    assert!(elapsed < 1.0, "took {elapsed} s for 4 s of audio");
}

// -------------------------------------------------------------------- sax

fn sax_with_preset(name: &str) -> Box<dyn Device> {
    let mut d = fresh("peel.wind.sax");
    let preset = peel_instruments::presets("peel.wind.sax")
        .iter()
        .find(|p| p.name == name)
        .unwrap();
    for (key, value) in preset.params {
        set(d.as_mut(), key, *value);
    }
    d
}

fn sax_note_range(name: &str) -> (u8, u8) {
    match name {
        "Tenor" => (44, 76),
        "Soprano" => (56, 88),
        _ => (49, 80),
    }
}

#[test]
fn sax_holds_pitch_across_its_range() {
    for key in [45u8, 52, 60, 69, 76] {
        let mut d = fresh("peel.wind.sax");
        set(d.as_mut(), "vibrato.depth", 0.0);
        let (l, _) = render(d.as_mut(), &[note_on(0, key, 0.8, 1)], 24_000, 256);
        let want = 440.0 * 2f32.powf((key as f32 - 69.0) / 12.0);
        let got = detect_pitch(&l[9_600..], SR);
        assert!(
            (got / want - 1.0).abs() < 0.01,
            "key {key}: wanted {want} Hz, got {got} Hz"
        );
    }
}

#[test]
fn sax_presets_sound_across_their_range() {
    for preset in peel_instruments::presets("peel.wind.sax") {
        let (lo, hi) = sax_note_range(preset.name);
        for key in [lo, (lo + hi) / 2, hi] {
            let mut d = sax_with_preset(preset.name);
            let (l, r) = render(
                d.as_mut(),
                &[note_on(0, key, 0.8, 1), note_off(30_000, key, 1)],
                48_000 * 2,
                256,
            );
            assert!(all_finite(&l) && all_finite(&r), "{} {key}", preset.name);
            let body = rms(&l[12_000..28_000]);
            assert!(
                body > 0.01 && peak(&l) < 1.5,
                "{} key {key}: rms {body}, peak {}",
                preset.name,
                peak(&l)
            );
            assert!(rms(&l[48_000 * 2 - 4_000..]) < 1e-4, "{} tail", preset.name);
            assert!(d.is_silent(), "{} key {key} not silent", preset.name);
        }
    }
}

#[test]
fn sax_long_note_is_stable_and_has_no_dc() {
    for name in ["Alto", "Growl", "Breathy"] {
        let mut d = sax_with_preset(name);
        let total = 48_000 * 20;
        let (l, _) = render(d.as_mut(), &[note_on(0, 62, 0.8, 1)], total, 512);
        assert!(all_finite(&l));
        let early = rms(&l[48_000..96_000]);
        let late = rms(&l[total - 96_000..total - 48_000]);
        assert!(
            (late / early - 1.0).abs() < 0.25,
            "{name}: level drifts {early} -> {late}"
        );
        let mean = l[48_000..].iter().sum::<f32>() / (total - 48_000) as f32;
        assert!(mean.abs() < 0.002, "{name}: dc {mean}");
        assert!(peak(&l) < 1.0, "{name}: peak {}", peak(&l));
    }
}

#[test]
fn sax_mono_legato_changes_pitch_without_retriggering() {
    let mut d = fresh("peel.wind.sax");
    set(d.as_mut(), "vibrato.depth", 0.0);
    set(d.as_mut(), "breath.scoop", 0.0);
    let events = [
        note_on(0, 60, 0.8, 1),
        note_on(24_000, 64, 0.8, 2),
        note_off(40_000, 64, 2),
        note_off(41_000, 60, 1),
    ];
    let (l, _) = render(d.as_mut(), &events, 48_000, 256);
    let first = detect_pitch(&l[10_000..20_000], SR);
    let second = detect_pitch(&l[30_000..38_000], SR);
    assert!((first / 261.63 - 1.0).abs() < 0.01, "first {first}");
    assert!((second / 329.63 - 1.0).abs() < 0.01, "second {second}");
    // No breath restart: no dip around the legato change.
    let before = rms(&l[23_000..23_900]);
    let after = rms(&l[24_100..25_000]);
    assert!(after > before * 0.7, "level dipped {before} -> {after}");
}

#[test]
fn sax_poly_mode_plays_chords() {
    let mut d = fresh("peel.wind.sax");
    set(d.as_mut(), "voice.mode", 1.0);
    set(d.as_mut(), "vibrato.depth", 0.0);
    let events = [note_on(0, 60, 0.8, 1), note_on(0, 67, 0.8, 2)];
    let (l, _) = render(d.as_mut(), &events, 24_000, 256);
    let w = &l[9_600..24_000];
    let c = tone_level(w, 261.63);
    let g = tone_level(w, 392.0);
    let between = tone_level(w, 330.0);
    assert!(
        c > between * 5.0 && g > between * 5.0,
        "c {c}, g {g}, mid {between}"
    );
}

#[test]
fn sax_vibrato_fades_in_after_its_delay() {
    let mut d = fresh("peel.wind.sax");
    set(d.as_mut(), "vibrato.depth", 60.0);
    set(d.as_mut(), "vibrato.delay", 0.5);
    set(d.as_mut(), "breath.scoop", 0.0);
    let (l, _) = render(d.as_mut(), &[note_on(0, 60, 0.8, 1)], 48_000 * 2, 256);
    let pitch_spread = |a: usize, b: usize| {
        let ps: Vec<f32> = (a..b)
            .step_by(2_400)
            .map(|s| detect_pitch(&l[s..s + 4_800], SR))
            .collect();
        let hi = ps.iter().cloned().fold(0.0, f32::max);
        let lo = ps.iter().cloned().fold(f32::MAX, f32::min);
        hi / lo
    };
    let early = pitch_spread(4_800, 20_000);
    let late = pitch_spread(60_000, 90_000);
    assert!(early < 1.01, "vibrato before its delay: {early}");
    assert!(late > 1.015, "no vibrato after the delay: {late}");
}

#[test]
fn sax_noise_growl_and_velocity_shape_the_sound() {
    let play = |key: &str, value: f32| {
        let mut d = fresh("peel.wind.sax");
        set(d.as_mut(), "vibrato.depth", 0.0);
        set(d.as_mut(), key, value);
        let (l, _) = render(d.as_mut(), &[note_on(0, 60, 0.8, 1)], 24_000, 256);
        l[9_600..].to_vec()
    };
    // Breath noise adds energy between the harmonics.
    let clean = play("breath.noise", 0.0);
    let airy = play("breath.noise", 1.0);
    let gap = |b: &[f32]| tone_level(b, 261.63 * 3.5) + tone_level(b, 261.63 * 8.5);
    assert!(gap(&airy) > gap(&clean) * 3.0, "noise should fill the gaps");
    // Growl adds low-rate sidebands next to the fundamental.
    let plain = play("growl.amount", 0.0);
    let growl = play("growl.amount", 1.0);
    let side = |b: &[f32]| tone_level(b, 261.63 - 70.0) + tone_level(b, 261.63 + 70.0);
    assert!(side(&growl) > side(&plain) * 3.0, "growl sidebands");
    // Harder blowing is brighter.
    let bright = |vel: f32| {
        let mut d = fresh("peel.wind.sax");
        set(d.as_mut(), "vibrato.depth", 0.0);
        let (l, _) = render(d.as_mut(), &[note_on(0, 60, vel, 1)], 24_000, 256);
        brightness(&l[9_600..])
    };
    assert!(
        bright(1.0) > bright(0.3) * 1.2,
        "velocity should open the tone"
    );
}

#[test]
fn sax_phrase_report() {
    // Numeric look at a legato phrase; prints with --nocapture.
    let mut d = fresh("peel.wind.sax");
    let notes = [(60u8, 0.8f32), (62, 0.7), (64, 0.9), (67, 0.6), (65, 0.8)];
    let mut events = Vec::new();
    for (i, (k, v)) in notes.iter().enumerate() {
        let at = i * 18_000;
        events.push(note_on(at, *k, *v, i as u32 + 1));
        events.push(note_off(at + 17_000, *k, i as u32 + 1));
    }
    let total = 5 * 18_000 + 12_000;
    let (l, _) = render(d.as_mut(), &events, total, 512);
    let mean = l.iter().sum::<f32>() / l.len() as f32;
    println!(
        "phrase: peak {:.3}, rms {:.3}, dc {:.5}, finite {}",
        peak(&l),
        rms(&l),
        mean,
        all_finite(&l)
    );
    assert!(peak(&l) > 0.1 && peak(&l) < 1.0 && mean.abs() < 0.005);
}

// ---------------------------------------------------------------- guitar

/// Fundamental from normalized autocorrelation around `expect`, with
/// parabolic refinement.
fn autocorr_pitch(buf: &[f32], expect: f32) -> f32 {
    let lo = (SR / (expect * 1.06)) as usize;
    let hi = (SR / (expect * 0.94)) as usize + 1;
    let n = buf.len() - hi - 1;
    let ac = |lag: usize| -> f32 { (0..n).map(|i| buf[i] * buf[i + lag]).sum::<f32>() };
    let r: Vec<f32> = (lo - 1..=hi + 1).map(ac).collect();
    let mut best = 1;
    for i in 1..r.len() - 1 {
        if r[i] > r[best] {
            best = i;
        }
    }
    let (a, b, c) = (r[best - 1], r[best], r[best + 1]);
    let frac = 0.5 * (a - c) / (a - 2.0 * b + c);
    SR / ((lo - 1 + best) as f32 + frac)
}

#[test]
fn guitar_notes_are_in_tune() {
    for (key, hz) in [
        (40u8, 82.407f32),
        (45, 110.0),
        (57, 220.0),
        (69, 440.0),
        (81, 880.0),
    ] {
        for gain in [0.1f32, 0.9] {
            let mut d = fresh("peel.guitar.electric");
            set(d.as_mut(), "amp.gain", gain);
            let (l, _) = render(d.as_mut(), &[note_on(0, key, 0.8, 1)], 40_000, 256);
            let f = autocorr_pitch(&l[6_000..30_000], hz);
            let cents = 1200.0 * (f / hz).log2();
            assert!(
                cents.abs() < 6.0,
                "key {key} gain {gain}: {f} Hz, {cents} cents"
            );
        }
    }
}

#[test]
fn guitar_chord_is_polyphonic_and_every_preset_is_clean() {
    let chord = [
        note_on(0, 40, 0.9, 1),
        note_on(300, 47, 0.9, 2),
        note_on(600, 52, 0.9, 3),
        note_on(900, 59, 0.9, 4),
        note_off(30_000, 40, 1),
        note_off(30_000, 47, 2),
        note_off(30_000, 52, 3),
        note_off(30_000, 59, 4),
    ];
    for preset in peel_instruments::presets("peel.guitar.electric") {
        let mut d = fresh("peel.guitar.electric");
        for (key, value) in preset.params {
            set(d.as_mut(), key, *value);
        }
        let (l, r) = render(d.as_mut(), &chord, 48_000 * 3, 256);
        assert!(all_finite(&l) && all_finite(&r), "{}", preset.name);
        let pk = peak(&l);
        assert!((0.05..1.5).contains(&pk), "{}: peak {pk}", preset.name);
        let mean = l.iter().sum::<f32>() / l.len() as f32;
        assert!(mean.abs() < 0.01, "{}: DC {mean}", preset.name);
        assert!(
            rms(&l[2_000..20_000]) > 0.02,
            "{}: chord too quiet",
            preset.name
        );
    }
}

#[test]
fn guitar_palm_mute_shortens_the_note() {
    let tail = |palm: f32| {
        let mut d = fresh("peel.guitar.electric");
        set(d.as_mut(), "string.palm_mute", palm);
        let (l, _) = render(d.as_mut(), &[note_on(0, 45, 0.9, 1)], 48_000 * 2, 256);
        rms(&l[48_000..48_000 + 4_800]) / rms(&l[2_400..7_200])
    };
    assert!(
        tail(1.0) < tail(0.0) * 0.1,
        "{} vs {}",
        tail(1.0),
        tail(0.0)
    );
}

#[test]
fn guitar_velocity_brightens_the_attack() {
    let bright = |vel: f32| {
        let mut d = fresh("peel.guitar.electric");
        let (l, _) = render(d.as_mut(), &[note_on(0, 57, vel, 1)], 12_000, 256);
        let seg = &l[1_000..9_000];
        let diff: f32 = seg.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        let tot: f32 = seg.iter().map(|x| x * x).sum();
        diff / tot
    };
    assert!(
        bright(1.0) > bright(0.2),
        "{} vs {}",
        bright(1.0),
        bright(0.2)
    );
}

#[test]
fn guitar_rings_out_and_goes_exactly_silent() {
    let mut d = fresh("peel.guitar.electric");
    let (l, _) = render(
        d.as_mut(),
        &[note_on(0, 45, 1.0, 1), note_off(24_000, 45, 1)],
        48_000 * 3,
        256,
    );
    assert!(rms(&l[10_000..14_000]) > 0.02);
    assert_eq!(peak(&l[48_000 * 2..]), 0.0);
    assert!(d.is_silent());
}

// ----------------------------------------------------------------- violin

fn violin_note(d: &mut dyn Device, key: u8, vel: f32, secs: f32) -> (Vec<f32>, Vec<f32>) {
    let n = (SR * secs) as usize;
    render(d, &[note_on(0, key, vel, 1)], n, 256)
}

#[test]
fn violin_sustained_notes_hold_pitch() {
    for key in [55u8, 62, 69, 76, 84, 88] {
        let mut d = fresh("peel.strings.violin");
        set(d.as_mut(), "vibrato.depth", 0.0);
        set(d.as_mut(), "vibrato.variation", 0.0);
        let (l, _) = violin_note(d.as_mut(), key, 0.8, 1.5);
        let want = 440.0 * 2f32.powf((key as f32 - 69.0) / 12.0);
        let got = detect_pitch(&l[SR as usize / 2..], SR);
        let cents = 1200.0 * (got / want).log2();
        println!(
            "key {key}: {got} Hz vs {want} Hz ({cents} cents) f0 {} half {}",
            tone_level(&l[SR as usize / 2..], want),
            tone_level(&l[SR as usize / 2..], want / 2.0)
        );
        assert!(
            cents.abs() < 15.0,
            "key {key}: {got} Hz vs {want} Hz ({cents} cents)"
        );
    }
}

#[test]
fn violin_top_notes_stay_on_the_fundamental() {
    // Autocorrelation is unreliable up here, so compare spectral peaks.
    let near = |buf: &[f32], f: f32| {
        (-20..=20)
            .map(|i| tone_level(buf, f * (1.0 + i as f32 * 0.0007)))
            .fold(0.0f32, f32::max)
    };
    for key in [93u8, 96, 98, 100] {
        let mut d = fresh("peel.strings.violin");
        set(d.as_mut(), "vibrato.depth", 0.0);
        set(d.as_mut(), "vibrato.variation", 0.0);
        let (l, _) = violin_note(d.as_mut(), key, 0.8, 1.0);
        let want = 440.0 * 2f32.powf((key as f32 - 69.0) / 12.0);
        let s = &l[24_000..];
        let f0 = near(s, want);
        assert!(f0 > 0.003, "key {key}: fundamental {f0}");
        assert!(near(s, want / 2.0) < 0.1 * f0, "key {key}: subharmonic");
    }
}

#[test]
fn violin_presets_sound_across_the_range() {
    let d0 = peel_instruments::descriptors()
        .iter()
        .find(|d| d.type_id == "peel.strings.violin")
        .unwrap();
    for preset in peel_instruments::presets("peel.strings.violin") {
        for key in [55u8, 67, 79, 91, 100] {
            let mut d = fresh("peel.strings.violin");
            for (k, v) in preset.params {
                d.set_param(d0.param_by_key(k).unwrap().id, *v);
            }
            let (l, r) = render(
                d.as_mut(),
                &[note_on(0, key, 0.9, 1), note_off(36_000, key, 1)],
                60_000,
                256,
            );
            assert!(
                all_finite(&l) && all_finite(&r),
                "{} key {key}",
                preset.name
            );
            let body = rms(&l[..60_000]).max(rms(&r[..60_000]));
            println!(
                "{} key {key}: rms {body} peak {}",
                preset.name,
                peak(&l).max(peak(&r))
            );
            assert!(body > 0.001, "{} key {key}: rms {body}", preset.name);
            assert!(peak(&l).max(peak(&r)) < 1.5, "{} key {key}", preset.name);
        }
    }
}

#[test]
fn violin_is_silent_after_release_and_has_no_dc() {
    let mut d = fresh("peel.strings.violin");
    set(d.as_mut(), "voice.ensemble", 1.0);
    let ev = [
        note_on(0, 60, 0.9, 1),
        note_on(0, 67, 0.9, 2),
        note_off(48_000, 60, 1),
        note_off(48_000, 67, 2),
    ];
    let (l, r) = render(d.as_mut(), &ev, 48_000 * 6, 512);
    assert!(d.is_silent());
    assert_eq!(peak(&l[48_000 * 5..]).max(peak(&r[48_000 * 5..])), 0.0);
    let mean = l[10_000..48_000].iter().sum::<f32>() / 38_000.0;
    assert!(
        mean.abs() < 0.01 * rms(&l[10_000..48_000]).max(1e-6) + 1e-3,
        "dc {mean}"
    );
}

#[test]
fn violin_pizzicato_decays_and_ensemble_is_wider() {
    let mut d = fresh("peel.strings.violin");
    set(d.as_mut(), "bow.style", 1.0);
    let (l, _) = violin_note(d.as_mut(), 69, 0.9, 3.0);
    let early = rms(&l[2_000..6_000]);
    let late = rms(&l[96_000..100_000]);
    println!("pizz {early} -> {late}");
    assert!(early > 0.01 && late < early * 0.2, "pizz {early} -> {late}");

    let side = |ens: f32| {
        let mut d = fresh("peel.strings.violin");
        set(d.as_mut(), "voice.ensemble", ens);
        set(d.as_mut(), "voice.width", 1.0);
        let (l, r) = violin_note(d.as_mut(), 67, 0.8, 1.5);
        let s: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a - b).collect();
        rms(&s[24_000..]) / rms(&l[24_000..])
    };
    assert!(side(1.0) > side(0.0) + 0.1);
}

#[test]
fn violin_legato_glides_between_overlapping_notes() {
    let mut d = fresh("peel.strings.violin");
    set(d.as_mut(), "voice.mode", 1.0);
    set(d.as_mut(), "voice.glide", 0.3);
    set(d.as_mut(), "vibrato.depth", 0.0);
    let ev = [
        note_on(0, 60, 0.8, 1),
        note_on(48_000, 67, 0.8, 2),
        note_off(49_000, 60, 1),
        note_off(96_000, 67, 2),
    ];
    let (l, _) = render(d.as_mut(), &ev, 100_000, 256);
    let before = detect_pitch(&l[30_000..46_000], SR);
    let mid = detect_pitch(&l[50_000..54_000], SR);
    let after = detect_pitch(&l[80_000..96_000], SR);
    let (a, b) = (261.63, 392.0);
    println!("legato {before} {mid} {after}");
    assert!((before / a - 1.0).abs() < 0.015, "before {before}");
    assert!(mid > a * 1.02 && mid < b * 0.995, "mid-glide {mid}");
    assert!((after / b - 1.0).abs() < 0.015, "after {after}");
}

#[test]
fn violin_phrase_analysis() {
    let mut d = fresh("peel.strings.violin");
    let (l, _) = violin_note(d.as_mut(), 69, 0.8, 30.0);
    let w = |a: usize, b: usize| rms(&l[a * 48_000..b * 48_000]);
    let mean = l.iter().sum::<f32>() / l.len() as f32;
    println!(
        "violin A4 30 s: peak {} rms 1-2s {} 14-15s {} 28-30s {} mean {mean}",
        peak(&l),
        w(1, 2),
        w(14, 15),
        w(28, 30)
    );
    assert!(all_finite(&l) && w(28, 30) > 0.5 * w(1, 2) && w(28, 30) < 2.0 * w(1, 2));
}

// ------------------------------------------- synth realism pass (poly, bass)

/// Level (dB re `fund`) of the strongest alias line of a saw at `f0`: the
/// folded images `SR - k * f0` of the harmonics above Nyquist.
fn worst_alias_db(buf: &[f32], f0: f32) -> f32 {
    let fund = tone_level(buf, f0);
    let mut worst = 0.0f32;
    let mut k = (SR / 2.0 / f0).ceil() as u32;
    while (k as f32) * f0 < SR {
        let alias = SR - k as f32 * f0;
        // Skip lines that land on (or next to) a true harmonic.
        let near = (alias / f0 - (alias / f0).round()).abs() * f0;
        if alias > 200.0 && near > 150.0 {
            worst = worst.max(tone_level(buf, alias));
        }
        k += 1;
    }
    20.0 * (worst / fund).log10()
}

#[test]
fn poly_saw_is_alias_free_up_to_the_top_octaves() {
    for key in [84u8, 96, 105] {
        let mut d = fresh("peel.synth.poly");
        for (k, v) in [
            ("unison.voices", 0.0),
            ("osc.mix", 0.0),
            ("filter.type", 0.0),
            ("filter.cutoff", 20_000.0),
            ("filter.env_amount", 0.0),
            ("filter.key_track", 0.0),
            ("filter.velocity", 0.0),
            ("out.velocity", 0.0),
            ("analog.drift", 0.0),
            ("amp.attack", 0.001),
            ("amp.sustain", 1.0),
        ] {
            set(d.as_mut(), k, v);
        }
        let (l, _) = render(d.as_mut(), &[note_on(0, key, 1.0, 1)], 30_000, 256);
        let f0 = 440.0 * ((key as f32 - 69.0) / 12.0).exp2();
        let db = worst_alias_db(&l[10_000..26_096], f0);
        // The previous 2-point PolyBLEP saw measured -45 dB (C5) to -29 dB
        // (A7) on this kind of test.
        assert!(db < -62.0, "key {key}: worst alias line {db} dB");
    }
}

#[test]
fn poly_supersaw_is_wide_but_keeps_its_mono_sum() {
    let mut d = fresh("peel.synth.poly");
    for (k, v) in [
        ("unison.voices", 6.0),
        ("unison.type", 1.0),
        ("unison.detune", 55.0),
        ("unison.spread", 1.0),
        ("unison.lowcut", 1.0),
        ("osc.mix", 0.0),
        ("amp.sustain", 1.0),
    ] {
        set(d.as_mut(), k, v);
    }
    let (l, r) = render(d.as_mut(), &[note_on(0, 55, 0.9, 1)], 60_000, 256);
    let (l, r) = (&l[12_000..], &r[12_000..]);
    let corr = {
        let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
        for (a, b) in l.iter().zip(r) {
            ll += (*a as f64).powi(2);
            rr += (*b as f64).powi(2);
            lr += *a as f64 * *b as f64;
        }
        lr / (ll * rr).sqrt()
    };
    let mono: Vec<f32> = l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)).collect();
    let loss_db = 20.0 * (rms(&mono) / (0.5 * (rms(l) + rms(r)))).log10();
    assert!(corr < 0.85, "supersaw should be wide, correlation {corr}");
    assert!(
        loss_db > -3.0,
        "supersaw collapses in mono: {loss_db} dB (correlation {corr})"
    );
}

#[test]
fn poly_every_unison_setting_plays_sanely() {
    for kind in [0.0, 1.0] {
        for voices in 0..9 {
            let mut d = fresh("peel.synth.poly");
            set(d.as_mut(), "unison.type", kind);
            set(d.as_mut(), "unison.voices", voices as f32);
            set(d.as_mut(), "unison.lowcut", 0.8);
            let events = [
                note_on(0, 48, 0.9, 1),
                note_on(0, 55, 0.9, 2),
                note_on(0, 64, 0.9, 3),
            ];
            let (l, r) = render(d.as_mut(), &events, 24_000, 256);
            let pk = peak(&l).max(peak(&r));
            assert!(all_finite(&l) && all_finite(&r));
            assert!(
                (0.05..2.0).contains(&pk),
                "type {kind} voices {}: peak {pk}",
                voices + 1
            );
        }
    }
}

#[test]
fn poly_legato_holds_one_voice_glides_and_falls_back() {
    let mut d = fresh("peel.synth.poly");
    for (k, v) in [
        ("voice.mode", 2.0),
        ("voice.glide", 0.25),
        ("unison.voices", 0.0),
        ("osc.mix", 0.0),
        ("osc1.wave", 0.0),
        ("filter.type", 0.0),
        ("filter.cutoff", 12_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("analog.drift", 0.0),
        ("amp.attack", 0.005),
        ("amp.sustain", 1.0),
        ("amp.release", 0.02),
    ] {
        set(d.as_mut(), k, v);
    }
    let events = [
        note_on(0, 48, 0.9, 1),
        note_on(20_000, 60, 0.9, 2),
        note_off(40_000, 60, 2),
        note_off(70_000, 48, 1),
    ];
    let (l, _) = render(d.as_mut(), &events, 80_000, 256);
    let before = detect_pitch(&l[12_000..19_000], SR);
    let gliding = detect_pitch(&l[21_000..24_000], SR);
    let arrived = detect_pitch(&l[33_000..39_000], SR);
    let back = detect_pitch(&l[58_000..66_000], SR);
    assert!((before / 130.81 - 1.0).abs() < 0.03, "first note {before}");
    assert!(
        gliding > 135.0 && gliding < 255.0,
        "legato should glide, got {gliding}"
    );
    assert!((arrived / 261.63 - 1.0).abs() < 0.04, "arrived {arrived}");
    assert!(
        (back / 130.81 - 1.0).abs() < 0.04,
        "releasing the top note falls back to the held one: {back}"
    );
    // No retrigger dip when the second note joins.
    let hold = rms(&l[18_000..20_000]);
    let join = rms_windows(&l[20_000..23_000], 240)
        .into_iter()
        .fold(f32::MAX, f32::min);
    assert!(join > 0.7 * hold, "legato dipped: {join} vs {hold}");
    assert!(rms(&l[76_000..]) < 1e-3, "released at the end");
}

#[test]
fn poly_pitch_envelope_dives_from_above() {
    let mut d = fresh("peel.synth.poly");
    for (k, v) in [
        ("unison.voices", 0.0),
        ("osc.mix", 0.0),
        ("osc1.wave", 0.0),
        ("filter.type", 0.0),
        ("filter.cutoff", 12_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("analog.drift", 0.0),
        ("amp.attack", 0.001),
        ("amp.sustain", 1.0),
        ("pitch.env_amount", 12.0),
        ("pitch.env_decay", 0.5),
    ] {
        set(d.as_mut(), k, v);
    }
    let (l, _) = render(d.as_mut(), &[note_on(0, 48, 0.9, 1)], 48_000, 256);
    let early = detect_pitch(&l[300..2_800], SR);
    let late = detect_pitch(&l[40_000..46_000], SR);
    assert!(
        (late / 130.81 - 1.0).abs() < 0.03,
        "settles on the note: {late}"
    );
    assert!(early > late * 1.4, "starts high: {early} vs {late}");
}

#[test]
fn poly_resonant_drive_survives_audio_rate_cutoff_modulation() {
    for filter in [0.0, 1.0, 2.0] {
        let mut d = fresh("peel.synth.poly");
        for (k, v) in [
            ("filter.type", filter),
            ("filter.resonance", 1.0),
            ("filter.drive", 1.0),
            ("filter.cutoff", 400.0),
            ("lfo.rate", 30.0),
            ("lfo.depth", 1.0),
            ("lfo.target", 1.0),
            ("lfo.shape", 3.0),
            ("amp.sustain", 1.0),
        ] {
            set(d.as_mut(), k, v);
        }
        let events = [note_on(0, 36, 1.0, 1), note_on(0, 60, 1.0, 2)];
        let (l, r) = render(d.as_mut(), &events, 48_000, 128);
        assert!(all_finite(&l) && all_finite(&r), "filter {filter}");
        assert!(peak(&l).max(peak(&r)) < 3.0, "filter {filter} blew up");
    }
}

#[test]
fn bass_sub_is_mono_clean_and_starts_at_zero_phase() {
    let mut d = fresh("peel.synth.bass");
    set(d.as_mut(), "osc.sub", 0.6);
    let (l, r) = render(d.as_mut(), &[note_on(100, 33, 0.9, 1)], 48_000, 256);
    assert_eq!(max_diff(&l, &r), 0.0, "bass must stay mono");
    // The sub starts at its zero crossing, so the very first samples are tiny.
    assert!(
        l[100..120].iter().all(|v| v.abs() < 0.01),
        "click at note start: {:?}",
        &l[100..110]
    );
    let mean = l[8_000..].iter().sum::<f32>() / (l.len() - 8_000) as f32;
    assert!(mean.abs() < 0.003, "dc offset {mean}");
}

#[test]
fn bass_reese_and_pulse_are_dc_free() {
    for (wave, pw) in [(0.0, 0.5), (1.0, 0.3)] {
        let mut d = fresh("peel.synth.bass");
        for (k, v) in [
            ("osc.wave", wave),
            ("osc.pw", pw),
            ("osc2.level", 0.9),
            ("osc2.voices", 2.0),
            ("filter.drive", 0.8),
            ("dist.amount", 0.4),
            ("amp.sustain", 1.0),
        ] {
            set(d.as_mut(), k, v);
        }
        let (l, _) = render(d.as_mut(), &[note_on(0, 36, 0.9, 1)], 48_000, 256);
        let mean = l[12_000..].iter().sum::<f32>() / (l.len() - 12_000) as f32;
        assert!(mean.abs() < 0.01, "wave {wave}: dc {mean}");
    }
}

#[test]
fn bass_pitch_envelope_gives_an_808_dive() {
    let mut d = fresh("peel.synth.bass");
    for (k, v) in [
        ("osc.wave", 3.0),
        ("osc.sub", 0.0),
        ("analog.drift", 0.0),
        ("filter.cutoff", 4_000.0),
        ("filter.env_amount", 0.0),
        ("pitch.env_amount", 12.0),
        ("pitch.env_decay", 0.5),
        ("amp.sustain", 1.0),
    ] {
        set(d.as_mut(), k, v);
    }
    let (l, _) = render(d.as_mut(), &[note_on(0, 36, 0.9, 1)], 48_000, 256);
    let early = detect_pitch(&l[300..2_800], SR);
    let late = detect_pitch(&l[40_000..46_000], SR);
    assert!((late / 65.41 - 1.0).abs() < 0.03, "late {late}");
    assert!(early > late * 1.5, "early {early} late {late}");
}

#[test]
fn bass_drive_is_oversampled_and_stays_alias_free() {
    let mut d = fresh("peel.synth.bass");
    for (k, v) in [
        ("osc.wave", 0.0),
        ("osc.sub", 0.0),
        ("analog.drift", 0.0),
        ("filter.cutoff", 8_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("filter.velocity", 0.0),
        ("filter.accent", 0.0),
        ("filter.drive", 1.0),
        ("out.velocity", 0.0),
        ("amp.sustain", 1.0),
    ] {
        set(d.as_mut(), k, v);
    }
    let (l, _) = render(d.as_mut(), &[note_on(0, 81, 1.0, 1)], 30_000, 256);
    let db = worst_alias_db(&l[10_000..26_096], 880.0);
    // The previous ladder ran at 1x and folded back well above the fundamental.
    assert!(db < -60.0, "driven bass alias line {db} dB");
}

#[test]
fn bass_every_growl_type_is_bounded() {
    for kind in 0..3 {
        let mut d = fresh("peel.synth.bass");
        set(d.as_mut(), "dist.type", kind as f32);
        set(d.as_mut(), "dist.amount", 1.0);
        set(d.as_mut(), "filter.drive", 1.0);
        set(d.as_mut(), "filter.resonance", 0.9);
        set(d.as_mut(), "osc.wave", 0.0);
        set(d.as_mut(), "osc2.level", 1.0);
        let (l, _) = render(d.as_mut(), &[note_on(0, 33, 1.0, 1)], 24_000, 256);
        assert!(all_finite(&l));
        assert!(peak(&l) < 2.0, "type {kind}: peak {}", peak(&l));
        assert!(peak(&l) > 0.05, "type {kind} is silent");
    }
}
