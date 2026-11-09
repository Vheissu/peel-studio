//! The recorded guitar and bass, `peel.guitar.recorded` and
//! `peel.bass.recorded`.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use common::*;
use peel_dsp::device::apply_defaults;
use peel_dsp::{Device, DeviceEvent, EventKind, ProcessContext};

const GUITAR: &str = "peel.guitar.recorded";
const BASS: &str = "peel.bass.recorded";
const PALM: &str = "peel.guitar.palm_muted";

// The palm-muted recordings the palm-muted guitar embeds, `PALM_FILES`.
include!("../src/palm_assets.rs");

// --- allocation tracking (this test binary only) ---------------------------

struct Counting;

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static EVENTS: Cell<usize> = const { Cell::new(0) };
}

fn note_alloc() {
    let _ = TRACK.try_with(|t| {
        if t.get() {
            EVENTS.with(|e| e.set(e.get() + 1));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_alloc();
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_alloc();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

// --- helpers -----------------------------------------------------------------

fn at_rate(id: &str, preset: &str, sample_rate: f32) -> Box<dyn Device> {
    let mut d = peel_instruments::create(id).unwrap();
    d.prepare(sample_rate, 256);
    apply_defaults(d.as_mut());
    set_preset(d.as_mut(), id, preset);
    d
}

fn inst(id: &str, preset: &str) -> Box<dyn Device> {
    at_rate(id, preset, SR)
}

fn set_preset(d: &mut dyn Device, id: &str, preset: &str) {
    let desc = d.descriptor();
    let p = peel_instruments::presets(id)
        .iter()
        .find(|p| p.name == preset)
        .unwrap_or_else(|| panic!("{id}: no preset {preset}"));
    for (k, v) in p.params {
        d.set_param(desc.param_by_key(k).unwrap().id, *v);
    }
}

fn set(d: &mut dyn Device, key: &str, value: f32) {
    let id = d.descriptor().param_by_key(key).unwrap().id;
    d.set_param(id, value);
}

/// Renders at an arbitrary sample rate (the common helper is 48 kHz only).
fn render_at(
    d: &mut dyn Device,
    events: &[Timed],
    total: usize,
    block: usize,
    sr: f32,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(total);
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        let evs: Vec<DeviceEvent> = events
            .iter()
            .filter(|(t, _)| *t >= pos && *t < pos + n)
            .map(|(t, k)| DeviceEvent {
                offset: (*t - pos) as u32,
                kind: *k,
            })
            .collect();
        let ctx = ProcessContext::new(sr);
        let mut l = vec![7.0f32; n];
        let mut r = vec![7.0f32; n];
        d.process(&ctx, &evs, &mut l, &mut r);
        assert_eq!(l, r, "mono instrument");
        out.extend_from_slice(&l);
        pos += n;
    }
    out
}

fn run(d: &mut dyn Device, events: &[Timed], total: usize) -> Vec<f32> {
    render_at(d, events, total, 256, SR)
}

fn rms(b: &[f32]) -> f32 {
    (b.iter().map(|x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

fn s(t: f32) -> usize {
    (t * SR) as usize
}

fn key_hz(key: f32) -> f32 {
    440.0 * 2f32.powf((key - 69.0) / 12.0)
}

/// Magnitude of the component at `hz` (single-bin DFT, Hann window).
fn tone(b: &[f32], hz: f32, sr: f32) -> f32 {
    let n = b.len();
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &x) in b.iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
        let ph = std::f64::consts::TAU * hz as f64 * i as f64 / sr as f64;
        re += x as f64 * w * ph.cos();
        im += x as f64 * w * ph.sin();
    }
    ((re * re + im * im).sqrt() / n as f64) as f32
}

/// The strongest component among the candidates.
#[allow(dead_code)]
fn loudest(b: &[f32], candidates: &[f32], sr: f32) -> f32 {
    *candidates
        .iter()
        .max_by(|a, c| tone(b, **a, sr).total_cmp(&tone(b, **c, sr)))
        .unwrap()
}

/// Detected pitch of a held note, measured on its sustained part.
fn pitch_of(id: &str, preset: &str, key: u8, sr: f32) -> f32 {
    let mut d = at_rate(id, preset, sr);
    let total = (1.2 * sr) as usize;
    let x = render_at(d.as_mut(), &[note_on(0, key, 0.8, 1)], total, 256, sr);
    let start = (0.3 * sr) as usize;
    detect_pitch(&x[start..start + (0.17 * sr) as usize], sr)
}

fn cents(a: f32, b: f32) -> f32 {
    1200.0 * (a / b).log2()
}

// --- descriptors and presets -------------------------------------------------

#[test]
fn descriptors_and_presets() {
    for (id, name, presets) in [
        (
            GUITAR,
            "Recorded Guitar",
            &["Clean", "High Gain", "Palm Mute"][..],
        ),
        (BASS, "Recorded Bass", &["Picked", "Rock Pick"][..]),
    ] {
        let d = peel_instruments::descriptors()
            .iter()
            .find(|d| d.type_id == id)
            .expect("registered");
        assert_eq!(d.name, name);
        assert_eq!(d.category, "Guitar");
        assert_eq!(d.version, 1);
        let all = peel_instruments::presets(id);
        for (i, want) in presets.iter().enumerate() {
            assert_eq!(all[i].name, *want, "{id} preset order");
        }
        for p in all {
            assert_eq!(
                p.params.len(),
                d.params.len(),
                "{id} {}: lists every parameter",
                p.name
            );
            for q in d.params {
                assert!(
                    p.params.iter().any(|(k, _)| *k == q.key),
                    "{id} {}: missing {}",
                    p.name,
                    q.key
                );
            }
        }
        // The first preset is the default sound.
        let first = &all[0];
        for (k, v) in first.params {
            assert_eq!(d.param_by_key(k).unwrap().default, *v, "{id}: {k}");
        }
    }
    // Existing instruments are untouched.
    assert!(peel_instruments::create("peel.guitar.electric").is_some());
    assert!(peel_instruments::create("peel.synth.bass").is_some());
    let g = peel_instruments::descriptors()
        .iter()
        .find(|d| d.type_id == "peel.guitar.electric")
        .unwrap();
    assert_eq!(g.name, "Guitar");
}

// --- samples -----------------------------------------------------------------

#[test]
fn every_recorded_note_plays_finite_and_audible() {
    let guitar_roots: Vec<u8> = (0..16).map(|i| 40 + 3 * i).collect();
    let bass_roots: Vec<u8> = (0..14).map(|i| 23 + 3 * i).collect();
    for (id, preset, articulation, roots) in [
        (GUITAR, "Clean", 0.0, &guitar_roots),
        (GUITAR, "Clean", 1.0, &guitar_roots),
        (BASS, "Picked", 0.0, &bass_roots),
    ] {
        for &k in roots.iter() {
            for vel in [0.3, 0.65, 1.0] {
                let mut d = inst(id, preset);
                if id == GUITAR {
                    set(d.as_mut(), "play.articulation", articulation);
                    set(d.as_mut(), "amp.output", 1.0);
                }
                let x = run(d.as_mut(), &[note_on(0, k, vel, 1)], s(0.3));
                assert!(all_finite(&x), "{id} {k}");
                let pk = peak(&x);
                assert!(pk > 3e-3 && pk < 1.5, "{id} key {k} vel {vel}: peak {pk}");
            }
        }
    }
}

#[test]
fn notes_play_their_own_pitch() {
    // Guitar on the DI path (the amp adds harmonics), bass clean.
    for (id, preset, key) in [
        (GUITAR, "Direct", 40u8),
        (GUITAR, "Direct", 45),
        (GUITAR, "Direct", 52),
        (GUITAR, "Direct", 64),
        (GUITAR, "Direct", 71),
        (BASS, "Picked", 33),
        (BASS, "Picked", 40),
        (BASS, "Picked", 47),
        (BASS, "Picked", 55),
    ] {
        let want = key_hz(key as f32);
        let got = pitch_of(id, preset, key, SR);
        assert!(
            cents(got, want).abs() < 20.0,
            "{id} key {key}: {got} Hz, want {want}"
        );
    }
}

#[test]
fn low_bass_notes_sound_in_the_right_octave() {
    // E1 (key 28) is 41.2 Hz; the recording must not play E2.
    for (key, root) in [(28u8, 41.2f32), (23, 30.87), (26, 36.71), (31, 49.0)] {
        let mut d = inst(BASS, "Picked");
        let x = run(d.as_mut(), &[note_on(0, key, 0.8, 1)], s(1.5));
        let w = &x[s(0.25)..s(1.45)];
        let f = key_hz(key as f32);
        assert!((f - root).abs() < 0.2);
        let (half, fund, oct) = (tone(w, f / 2.0, SR), tone(w, f, SR), tone(w, 2.0 * f, SR));
        // A recording an octave too high would have nothing at `f`; one an
        // octave too low would have energy at `f / 2`. B0 and C1 strings
        // carry less fundamental than second harmonic, as real ones do.
        let need = if key >= 28 { 0.3 } else { 0.08 };
        assert!(
            fund > need * oct,
            "key {key}: fundamental {fund} vs octave {oct}"
        );
        assert!(fund > 4.0 * half, "key {key}: energy an octave below");
        assert!(tone(w, f, SR) > 1e-4, "key {key}: no fundamental");
    }
    // Low E of the guitar is E2, 82.4 Hz.
    let got = pitch_of(GUITAR, "Direct", 40, SR);
    assert!(cents(got, 82.41).abs() < 20.0, "guitar E2: {got}");
}

#[test]
fn pitch_is_right_at_every_rate() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        for (id, preset, key) in [(GUITAR, "Direct", 55u8), (BASS, "Picked", 45)] {
            let got = pitch_of(id, preset, key, sr);
            let want = key_hz(key as f32);
            assert!(
                cents(got, want).abs() < 20.0,
                "{id} at {sr}: {got}, want {want}"
            );
        }
    }
    // Loudness does not depend on the rate.
    let level = |sr: f32| {
        let mut d = at_rate(GUITAR, "Clean", sr);
        let x = render_at(
            d.as_mut(),
            &[note_on(0, 52, 0.8, 1)],
            (0.5 * sr) as usize,
            256,
            sr,
        );
        db(rms(&x))
    };
    let (a, b, c) = (level(44_100.0), level(48_000.0), level(96_000.0));
    assert!((a - b).abs() < 1.0 && (c - b).abs() < 1.0, "{a} {b} {c}");
}

#[test]
fn keys_past_the_range_transpose_then_stop() {
    let mut d = inst(GUITAR, "Direct");
    let x = run(d.as_mut(), &[note_on(0, 30, 0.8, 1)], s(0.3));
    assert!(peak(&x) > 1e-3, "an octave-ish below low E still plays");
    for (id, key) in [(GUITAR, 20u8), (GUITAR, 110), (BASS, 5), (BASS, 90)] {
        let mut d = inst(id, if id == GUITAR { "Clean" } else { "Picked" });
        let x = run(d.as_mut(), &[note_on(0, key, 0.8, 1)], s(0.3));
        assert_eq!(peak(&x), 0.0, "{id} key {key}");
        assert!(d.is_silent());
    }
}

// --- duration, release and articulation ---------------------------------------

#[test]
fn held_notes_sustain_and_short_notes_stop() {
    for (id, preset, key) in [
        (GUITAR, "Clean", 52u8),
        (BASS, "Picked", 40),
        (GUITAR, "High Gain", 47),
    ] {
        let mut held = inst(id, preset);
        let a = run(
            held.as_mut(),
            &[note_on(0, key, 0.8, 1), note_off(s(2.0), key, 1)],
            s(2.0),
        );
        let mut short = inst(id, preset);
        let b = run(
            short.as_mut(),
            &[note_on(0, key, 0.8, 1), note_off(s(0.1), key, 1)],
            s(2.0),
        );
        assert_eq!(&a[..s(0.1)], &b[..s(0.1)], "{id}: same start");
        let late_held = rms(&a[s(0.8)..s(1.0)]);
        let late_short = rms(&b[s(0.8)..s(1.0)]);
        assert!(
            db(late_held) > -60.0,
            "{id} {preset}: held note died ({})",
            db(late_held)
        );
        assert!(
            db(late_short) < db(late_held) - 40.0,
            "{id} {preset}: released note still sounds ({} vs {})",
            db(late_short),
            db(late_held)
        );
    }
}

#[test]
fn fast_muted_riffs_stop_between_notes() {
    let mut d = inst(GUITAR, "Palm Mute");
    set(d.as_mut(), "amp.output", 1.0);
    // Sixteenths at 160 bpm, each held 80 % of its step.
    let step = s(60.0 / 160.0 / 4.0);
    let mut ev = Vec::new();
    for i in 0..12 {
        ev.push(note_on(i * step, 40, 0.8, i as u32 + 1));
        ev.push(note_off(i * step + step * 4 / 5, 40, i as u32 + 1));
    }
    let x = run(d.as_mut(), &ev, step * 13);
    for i in 0..12 {
        let note = &x[i * step..i * step + step / 3];
        let gap = &x[(i + 1) * step - s(0.004)..(i + 1) * step];
        assert!(
            db(rms(gap)) < db(rms(note)) - 12.0,
            "chug {i}: gap {} vs note {}",
            db(rms(gap)),
            db(rms(note))
        );
    }
}

#[test]
fn muted_and_sustained_are_different_recordings() {
    let render = |artic: f32, mute: f32| {
        let mut d = inst(GUITAR, "Direct");
        set(d.as_mut(), "play.articulation", artic);
        set(d.as_mut(), "play.mute", mute);
        run(d.as_mut(), &[note_on(0, 45, 0.9, 1)], s(1.0))
    };
    let sus = render(0.0, 0.0);
    let muted = render(1.0, 0.0);
    // Similar attack level ...
    let (a, b) = (db(rms(&sus[..s(0.08)])), db(rms(&muted[..s(0.08)])));
    assert!((a - b).abs() < 8.0, "attack {a} vs {b}");
    // ... but a different waveform and a much shorter note.
    let n = s(0.05);
    let dot: f32 = sus[..n].iter().zip(&muted[..n]).map(|(x, y)| x * y).sum();
    let corr = dot / (rms(&sus[..n]) * rms(&muted[..n]) * n as f32);
    assert!(corr.abs() < 0.9, "same recording? correlation {corr}");
    assert!(
        db(rms(&muted[s(0.6)..s(0.9)])) < db(rms(&sus[s(0.6)..s(0.9)])) - 20.0,
        "muted note rings like a sustained one"
    );
    // Mute shortens and darkens further.
    let damped = render(1.0, 0.8);
    assert!(rms(&damped[s(0.2)..s(0.4)]) < 0.5 * rms(&muted[s(0.2)..s(0.4)]));
    // The pick attack is kept.
    assert!(peak(&damped[..s(0.015)]) > 0.5 * peak(&muted[..s(0.015)]));
}

#[test]
fn retriggering_a_key_damps_its_previous_note_only() {
    // Same key: the earlier strike is damped, the new one rings.
    let mut d = inst(GUITAR, "Direct");
    let ev = [
        note_on(0, 52, 0.9, 1),
        note_on(s(0.5), 52, 0.9, 2),
        note_off(s(0.6), 52, 1),
    ];
    let x = run(d.as_mut(), &ev, s(1.5));
    let mut solo = inst(GUITAR, "Direct");
    // Same take order: the second strike of key 52 on a fresh instance.
    let y = run(
        solo.as_mut(),
        &[
            note_on(0, 52, 0.9, 7),
            note_off(s(0.01), 52, 7),
            note_on(s(0.5), 52, 0.9, 2),
        ],
        s(1.5),
    );
    let late = |b: &[f32]| rms(&b[s(1.0)..s(1.4)]);
    assert!(
        db(late(&x)) > -60.0,
        "note-off of the old note killed the new one"
    );
    assert!(
        (db(late(&x)) - db(late(&y))).abs() < 1.5,
        "the old strike still rings under the new one: {} vs {}",
        db(late(&x)),
        db(late(&y))
    );
}

#[test]
fn mono_bass_hands_over_and_old_note_offs_do_not_cut() {
    let ev = [
        note_on(0, 40, 0.9, 1),
        note_on(s(0.5), 47, 0.9, 2),
        note_off(s(0.6), 40, 1),
    ];
    let mut mono = inst(BASS, "Picked");
    let x = run(mono.as_mut(), &ev, s(1.5));
    let w = &x[s(0.8)..s(1.3)];
    assert!(db(rms(w)) > -50.0, "newer note was cut");
    let (old_f, new_f) = (key_hz(40.0), key_hz(47.0));
    assert!(
        tone(w, new_f, SR) > 8.0 * tone(w, old_f, SR),
        "mono: the old note still sounds"
    );
    // Poly: both ring until the old note's own note-off.
    let mut poly = inst(BASS, "Picked");
    set(poly.as_mut(), "play.mono", 0.0);
    let y = run(
        poly.as_mut(),
        &[note_on(0, 40, 0.9, 1), note_on(s(0.5), 47, 0.9, 2)],
        s(1.0),
    );
    let w = &y[s(0.6)..s(0.95)];
    assert!(
        tone(w, old_f, SR) > 0.2 * tone(w, new_f, SR),
        "poly lost the old note"
    );
}

#[test]
fn release_noise_follows_the_note_off() {
    let render = |fret: f32| {
        let mut d = inst(GUITAR, "Direct");
        set(d.as_mut(), "play.fret_noise", fret);
        run(
            d.as_mut(),
            &[note_on(0, 52, 0.9, 1), note_off(s(0.5), 52, 1)],
            s(1.0),
        )
    };
    let quiet = render(0.0);
    let noisy = render(1.0);
    assert_eq!(&quiet[..s(0.5)], &noisy[..s(0.5)]);
    let tail = |b: &[f32]| rms(&b[s(0.5)..s(0.6)]);
    assert!(tail(&noisy) > 1.2 * tail(&quiet), "no fret noise");
    // It never outlasts its recording.
    assert!(peak(&noisy[s(0.98)..]) < 1e-3);
}

// --- velocity and takes --------------------------------------------------------

#[test]
fn velocity_layers_rise_in_level_and_brightness() {
    let edge = |b: &[f32]| {
        let d: f32 = b.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        (d / b.iter().map(|x| x * x).sum::<f32>().max(1e-12)).sqrt()
    };
    for (id, preset) in [(GUITAR, "Direct"), (BASS, "Picked")] {
        let mut prev = f32::NEG_INFINITY;
        let mut bright = Vec::new();
        for i in 1..=10 {
            let v = i as f32 / 10.0;
            // Several notes so take and layer picks average out.
            let mut d = inst(id, preset);
            let key = if id == GUITAR { 55 } else { 40 };
            let ev: Vec<Timed> = (0..4)
                .flat_map(|n| {
                    [
                        note_on(n * s(0.3), key, v, n as u32 + 1),
                        note_off(n * s(0.3) + s(0.25), key, n as u32 + 1),
                    ]
                })
                .collect();
            let x = run(d.as_mut(), &ev, s(1.2));
            let level = db(rms(&x));
            assert!(
                level > prev - 0.5,
                "{id} velocity {v}: {level} after {prev}"
            );
            prev = prev.max(level);
            bright.push(edge(&x));
        }
        assert!(
            bright[9] > bright[2] * 1.05,
            "{id}: hard notes not brighter {bright:?}"
        );
    }
}

#[test]
fn alternate_takes_vary_and_render_deterministically() {
    for (id, preset, key) in [(GUITAR, "Direct", 52u8), (BASS, "Picked", 40)] {
        let ev: Vec<Timed> = (0..4)
            .flat_map(|n| {
                [
                    note_on(n * s(0.25), key, 0.95, n as u32 + 1),
                    note_off(n * s(0.25) + s(0.2), key, n as u32 + 1),
                ]
            })
            .collect();
        let mut a = inst(id, preset);
        let mut b = inst(id, preset);
        let x = run(a.as_mut(), &ev, s(1.0));
        let y = run(b.as_mut(), &ev, s(1.0));
        assert_eq!(x, y, "{id}: not deterministic");
        let hit = |i: usize| &x[i * s(0.25)..i * s(0.25) + s(0.05)];
        // Consecutive hits never repeat a take.
        for i in 0..3 {
            assert!(
                max_diff(hit(i), hit(i + 1)) > 1e-3,
                "{id}: hit {i} repeated its take"
            );
        }
        // Onsets do not move: every hit starts on its sample.
        for i in 0..4 {
            let start = i * s(0.25);
            assert!(
                peak(&x[start..start + s(0.005)]) > 1e-3,
                "{id}: hit {i} late"
            );
        }
    }
}

// --- polyphony, headroom, expression --------------------------------------------

#[test]
fn chords_keep_headroom_without_dc() {
    let chord = [40u8, 47, 52, 55, 59, 64];
    for (id, preset, offset) in [
        (GUITAR, "Clean", 0u8),
        (GUITAR, "High Gain", 0),
        (GUITAR, "Direct", 0),
        (BASS, "Picked", 12),
    ] {
        let mut d = inst(id, preset);
        if id == BASS {
            set(d.as_mut(), "play.mono", 0.0);
        }
        let ev: Vec<Timed> = chord
            .iter()
            .enumerate()
            .map(|(i, k)| note_on(i * 40, *k - offset, 1.0, i as u32 + 1))
            .collect();
        let x = run(d.as_mut(), &ev, s(2.0));
        assert!(all_finite(&x));
        assert!(peak(&x) < 1.0, "{id} {preset}: chord peak {}", peak(&x));
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        assert!(mean.abs() < 2e-3, "{id} {preset}: DC {mean}");
    }
}

#[test]
fn per_voice_expression_follows_the_voice_id() {
    // A snapshot at onset starts the voice bent; another key is unaffected.
    let ev = [
        note_on(0, 52, 0.8, 1),
        (
            0,
            EventKind::VoiceExpression {
                voice: 1,
                pitch: 7.0,
                pressure: 0.0,
                timbre: 0.5,
            },
        ),
    ];
    let mut d = inst(GUITAR, "Direct");
    let x = run(d.as_mut(), &ev, s(0.8));
    let got = detect_pitch(&x[s(0.3)..s(0.47)], SR);
    assert!(
        cents(got, key_hz(59.0)).abs() < 20.0,
        "snapshot pitch {got}"
    );
    // Two sounding voices bend independently.
    let ev = [
        note_on(0, 40, 0.8, 1),
        note_on(0, 55, 0.8, 2),
        (
            s(0.1),
            EventKind::VoiceExpression {
                voice: 2,
                pitch: 5.0,
                pressure: 0.0,
                timbre: 0.5,
            },
        ),
    ];
    let mut d = inst(GUITAR, "Direct");
    let x = run(d.as_mut(), &ev, s(1.0));
    let w = &x[s(0.4)..s(0.9)];
    assert!(tone(w, key_hz(40.0), SR) > 1e-4, "unbent voice moved");
    assert!(
        tone(w, key_hz(60.0), SR) > 3.0 * tone(w, key_hz(55.0), SR),
        "bent voice did not move"
    );
    // Pressure raises one voice's level.
    let level = |p: f32| {
        let mut d = inst(BASS, "Picked");
        let ev = [
            note_on(0, 40, 0.8, 1),
            (
                0,
                EventKind::VoiceExpression {
                    voice: 1,
                    pitch: 0.0,
                    pressure: p,
                    timbre: 0.5,
                },
            ),
        ];
        rms(&run(d.as_mut(), &ev, s(0.5)))
    };
    assert!(level(1.0) > 1.4 * level(0.0));
}

#[test]
fn extreme_bends_stay_finite_and_bounded() {
    for (id, preset) in [
        (GUITAR, "High Gain"),
        (GUITAR, "Direct"),
        (BASS, "Rock Pick"),
    ] {
        for pitch in [-120.0, 120.0] {
            let mut d = inst(id, preset);
            let ev = [
                (0, EventKind::PitchBend { semitones: pitch }),
                note_on(0, 50, 1.0, 1),
                note_on(0, 62, 1.0, 2),
                (
                    s(0.1),
                    EventKind::VoiceExpression {
                        voice: 2,
                        pitch: -pitch,
                        pressure: 1.0,
                        timbre: 1.0,
                    },
                ),
                note_off(s(0.5), 50, 1),
                note_off(s(0.5), 62, 2),
            ];
            let x = run(d.as_mut(), &ev, s(1.0));
            assert!(all_finite(&x) && peak(&x) < 2.0, "{id} bend {pitch}");
            let _ = run(d.as_mut(), &[], s(4.0));
            assert!(d.is_silent(), "{id} bend {pitch}: never ended");
        }
    }
}

// --- engine contract -------------------------------------------------------------

fn busy_phrase() -> Vec<Timed> {
    vec![
        note_on(10, 40, 0.9, 1),
        note_on(10, 47, 0.7, 2),
        note_on(1_000, 52, 0.5, 3),
        (2_000, EventKind::PitchBend { semitones: 1.5 }),
        (
            3_000,
            EventKind::VoiceExpression {
                voice: 3,
                pitch: -2.0,
                pressure: 0.7,
                timbre: 0.9,
            },
        ),
        note_off(9_000, 40, 1),
        note_on(9_500, 40, 0.8, 4),
        note_off(15_000, 47, 2),
        note_off(15_000, 52, 3),
        note_off(30_000, 40, 4),
    ]
}

#[test]
fn output_does_not_depend_on_block_size() {
    for (id, preset) in [
        (GUITAR, "High Gain"),
        (GUITAR, "Palm Mute"),
        (BASS, "Rock Pick"),
        (BASS, "Muted"),
    ] {
        let mut ev = busy_phrase();
        // Live control moves mid-note.
        let desc = inst(id, preset).descriptor();
        for (i, p) in desc.params.iter().enumerate() {
            ev.push((
                4_000 + i * 997,
                EventKind::Param {
                    id: p.id,
                    value: p.from_normalized(0.3 + 0.05 * i as f32),
                },
            ));
        }
        ev.sort_by_key(|e| e.0);
        let mut r = inst(id, preset);
        let reference = render_at(r.as_mut(), &ev, 40_000, 256, SR);
        assert!(peak(&reference) > 0.01);
        for block in [1, 61, 997] {
            let mut d = inst(id, preset);
            let x = render_at(d.as_mut(), &ev, 40_000, block, SR);
            assert!(
                max_diff(&x, &reference) < 1e-5,
                "{id} {preset}: block {block}"
            );
        }
    }
}

#[test]
fn panic_reset_and_tails_end_in_silence() {
    for (id, preset) in [
        (GUITAR, "High Gain"),
        (GUITAR, "Clean"),
        (BASS, "Picked"),
        (BASS, "Rock Pick"),
    ] {
        // Panic.
        let mut d = inst(id, preset);
        let x = run(
            d.as_mut(),
            &[
                note_on(0, 45, 1.0, 1),
                note_on(0, 52, 1.0, 2),
                (s(0.3), EventKind::AllSoundOff),
            ],
            s(0.5),
        );
        assert!(peak(&x[..s(0.3)]) > 0.01);
        assert_eq!(peak(&x[s(0.3)..]), 0.0, "{id}: sound after panic");
        assert!(d.is_silent());
        // Reset.
        let _ = run(d.as_mut(), &[note_on(0, 45, 1.0, 3)], s(0.2));
        d.reset();
        assert!(d.is_silent());
        assert_eq!(peak(&run(d.as_mut(), &[], 2_048)), 0.0);
        // Natural end: held to the end of the recording, released, then
        // exact silence and is_silent, including the amp tail.
        let x = run(d.as_mut(), &[note_on(0, 45, 1.0, 4)], s(7.0));
        assert!(
            d.is_silent(),
            "{id} {preset}: still active after the recording ended"
        );
        assert_eq!(peak(&x[s(6.8)..]), 0.0);
        let x = run(
            d.as_mut(),
            &[note_on(0, 45, 1.0, 5), note_off(s(0.2), 45, 5)],
            s(1.5),
        );
        assert!(d.is_silent(), "{id} {preset}: release never ended");
        assert_eq!(peak(&x[s(1.4)..]), 0.0);
        // All notes off releases held notes.
        let _ = run(
            d.as_mut(),
            &[note_on(0, 45, 1.0, 6), (s(0.1), EventKind::AllNotesOff)],
            s(2.0),
        );
        assert!(d.is_silent());
    }
}

#[test]
fn switching_amp_and_direct_is_click_free() {
    let mut d = inst(GUITAR, "Clean");
    let out_id = d.descriptor().param_by_key("amp.output").unwrap().id;
    let ev = [
        note_on(0, 52, 0.9, 1),
        (
            s(0.4),
            EventKind::Param {
                id: out_id,
                value: 1.0,
            },
        ),
        (
            s(0.8),
            EventKind::Param {
                id: out_id,
                value: 0.0,
            },
        ),
    ];
    let x = run(d.as_mut(), &ev, s(1.2));
    let step = |b: &[f32]| b.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
    let normal = step(&x[s(0.2)..s(0.38)]);
    for at in [0.4, 0.8] {
        let around = step(&x[s(at) - 64..s(at) + s(0.03)]);
        assert!(
            around < 2.5 * normal + 1e-3,
            "switch at {at}: step {around} vs {normal}"
        );
    }
}

#[test]
fn bass_drive_keeps_the_low_end() {
    let render = |preset: &str| {
        let mut d = inst(BASS, preset);
        set(d.as_mut(), "tone.cutoff", 20_000.0);
        set(d.as_mut(), "out.level", 0.0);
        run(d.as_mut(), &[note_on(0, 28, 0.9, 1)], s(1.0))
    };
    let clean = render("Picked");
    let driven = render("Rock Pick");
    let f = key_hz(28.0);
    let w = s(0.2)..s(0.9);
    let (a, b) = (
        tone(&clean[w.clone()], f, SR),
        tone(&driven[w.clone()], f, SR),
    );
    assert!(
        (db(b) - db(a)).abs() < 1.5,
        "drive moved the fundamental: {} vs {}",
        db(a),
        db(b)
    );
    // The drive adds high harmonics (its distortion products).
    let hi = |x: &[f32]| (20..=40).map(|h| tone(x, f * h as f32, SR)).sum::<f32>();
    assert!(
        hi(&driven[w.clone()]) > 3.0 * hi(&clean[w]),
        "drive inaudible"
    );
}

#[test]
fn long_held_high_gain_chords_ring_out_smoothly() {
    // A power chord held far past the old 6 s sample cap. The recordings
    // must not stop dead: through High Gain a cut 40 dB down is still loud.
    for (preset, keys) in [
        ("High Gain", [40u8, 47, 52]),
        ("High Gain", [45, 52, 57]),
        ("Clean", [40, 47, 52]),
    ] {
        let mut d = inst(GUITAR, preset);
        let ev: Vec<Timed> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| note_on(i * 30, *k, 0.95, i as u32 + 1))
            .collect();
        let total = s(14.0);
        let x = run(d.as_mut(), &ev, total);
        assert!(all_finite(&x), "{preset} {keys:?}");
        let w = s(0.02);
        let env: Vec<f32> = x.chunks(w).map(|c| db(rms(c))).collect();
        let top = env.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut worst = 0.0f32;
        let mut at = 0;
        for i in s(0.5) / w..env.len() - 1 {
            // Only where the chord is still clearly audible.
            if env[i] > top - 50.0 {
                let drop = env[i] - env[i + 1];
                if drop > worst {
                    worst = drop;
                    at = i;
                }
            }
        }
        assert!(
            worst < 6.0,
            "{preset} {keys:?}: {worst:.1} dB drop in 20 ms at {:.2} s",
            at as f32 * 0.02
        );
        let around = |t: f32| db(rms(&x[s(t)..s(t + 0.2)]));
        // It decays towards the end, and then ends.
        assert!(
            around(3.0) > around(9.0) + 6.0 || around(9.0) < top - 60.0,
            "{preset} {keys:?}: no decay"
        );
        assert!(
            peak(&x[s(13.5)..]) < 1e-4,
            "{preset} {keys:?}: still sounding at 13.5 s"
        );
        assert!(d.is_silent(), "{preset} {keys:?}: not silent at the end");
    }
}

#[test]
fn more_bass_drive_adds_grit_without_dulling() {
    // Same note and take order at every Drive; Tone open, level fixed.
    let render = |drive: f32| {
        let mut d = inst(BASS, "Picked");
        set(d.as_mut(), "tone.drive", drive);
        let ev = [note_on(0, 28, 0.9, 1), note_off(s(0.9), 28, 1)];
        run(d.as_mut(), &ev, s(1.0))
    };
    let f = key_hz(28.0);
    let attack = s(0.01)..s(0.15);
    let body = s(0.2)..s(0.8);
    // Harmonics clear of the 220 Hz split: the upper band.
    let upper = |x: &[f32]| (6..=16).map(|h| tone(x, f * h as f32, SR)).sum::<f32>();
    let grit = |x: &[f32]| (20..=40).map(|h| tone(x, f * h as f32, SR)).sum::<f32>();
    let clean = render(0.0);
    let rock = render(0.35);
    let fund0 = tone(&clean[body.clone()], f, SR);
    let mut prev_grit = grit(&rock[body.clone()]);
    for drive in [0.35, 0.5, 0.75, 1.0] {
        let x = render(drive);
        assert!(
            all_finite(&x) && peak(&x) < 1.0,
            "drive {drive}: peak {}",
            peak(&x)
        );
        // Fundamentals stay where the clean bass has them.
        let fund = tone(&x[body.clone()], f, SR);
        assert!(
            (db(fund) - db(fund0)).abs() < 1.5,
            "drive {drive}: fundamental {} vs clean {}",
            db(fund),
            db(fund0)
        );
        // The upper band's attack is not turned down relative to Rock Pick ...
        let (a, r) = (upper(&x[attack.clone()]), upper(&rock[attack.clone()]));
        assert!(
            db(a) > db(r) - 1.5,
            "drive {drive}: upper attack {} vs Rock Pick {}",
            db(a),
            db(r)
        );
        // ... nor merely made louder overall ...
        assert!(
            db(rms(&x)) < db(rms(&rock)) + 4.0,
            "drive {drive}: level jump"
        );
        // ... and the high harmonics (grit) grow with Drive.
        let g = grit(&x[body.clone()]);
        assert!(
            g >= 0.95 * prev_grit,
            "drive {drive}: grit {g} after {prev_grit}"
        );
        prev_grit = g;
    }
    // Full Drive is clearly grittier than Rock Pick and than clean.
    let full = render(1.0);
    assert!(grit(&full[body.clone()]) > 1.5 * grit(&rock[body.clone()]));
    assert!(grit(&rock[body.clone()]) > grit(&clean[body]));
}

#[test]
fn high_gain_keeps_a_tight_low_end() {
    // The amp's pre-distortion high-pass takes mud out of high-gain chugs
    // relative to the clean setting at the same level.
    let render = |preset: &str| {
        let mut d = inst(GUITAR, preset);
        run(d.as_mut(), &[note_on(0, 40, 0.9, 1)], s(1.0))
    };
    let ratio = |x: &[f32]| {
        let w = &x[s(0.1)..s(0.6)];
        tone(w, 82.4, SR) / rms(w)
    };
    assert!(ratio(&render("High Gain")) < ratio(&render("Clean")));
}

#[test]
fn audio_path_never_allocates_and_is_cheap() {
    for (id, preset) in [
        (GUITAR, "High Gain"),
        (GUITAR, "Palm Mute"),
        (BASS, "Rock Pick"),
    ] {
        let mut d = inst(id, preset);
        let desc = d.descriptor();
        let mut l = vec![0.0f32; 256];
        let mut r = vec![0.0f32; 256];
        let mut events: Vec<DeviceEvent> = Vec::new();
        for i in 0..40u32 {
            events.push(DeviceEvent {
                offset: i * 6,
                kind: EventKind::NoteOn {
                    key: 30 + (i * 7 % 50) as u8,
                    velocity: 0.2 + 0.02 * i as f32,
                    voice: i,
                },
            });
        }
        for (i, p) in desc.params.iter().enumerate() {
            events.push(DeviceEvent {
                offset: 241 + i as u32 % 14,
                kind: EventKind::Param {
                    id: p.id,
                    value: p.from_normalized(0.7),
                },
            });
        }
        events.push(DeviceEvent {
            offset: 250,
            kind: EventKind::VoiceExpression {
                voice: 3,
                pitch: 2.0,
                pressure: 0.5,
                timbre: 0.2,
            },
        });
        let offs: Vec<DeviceEvent> = (0..40u32)
            .map(|i| DeviceEvent {
                offset: i,
                kind: EventKind::NoteOff {
                    key: 30 + (i * 7 % 50) as u8,
                    velocity: 0.0,
                    voice: i,
                },
            })
            .collect();
        let ctx = ProcessContext::new(SR);
        EVENTS.with(|e| e.set(0));
        TRACK.with(|t| t.set(true));
        d.process(&ctx, &events, &mut l, &mut r);
        for _ in 0..50 {
            d.process(&ctx, &[], &mut l, &mut r);
        }
        d.process(&ctx, &offs, &mut l, &mut r);
        for _ in 0..50 {
            d.process(&ctx, &[], &mut l, &mut r);
        }
        d.process(
            &ctx,
            &[DeviceEvent {
                offset: 0,
                kind: EventKind::AllSoundOff,
            }],
            &mut l,
            &mut r,
        );
        d.reset();
        TRACK.with(|t| t.set(false));
        assert_eq!(
            EVENTS.with(Cell::get),
            0,
            "{id}: allocated on the audio path"
        );

        // Six-note chords twice a second for 10 s, well inside realtime.
        let mut ev = Vec::new();
        for i in 0..20 {
            for (j, k) in [40u8, 47, 52, 55, 59, 64].iter().enumerate() {
                let v = (i * 6 + j) as u32 + 100;
                let key = if id == BASS { *k - 12 } else { *k };
                ev.push(note_on(i * s(0.5), key, 0.8, v));
                ev.push(note_off(i * s(0.5) + s(0.45), key, v));
            }
        }
        let t = Instant::now();
        let x = run(d.as_mut(), &ev, s(10.0));
        let load = t.elapsed().as_secs_f32() / 10.0;
        assert!(all_finite(&x));
        assert!(
            load < 0.25,
            "{id} {preset}: {:.1} % of realtime",
            100.0 * load
        );
    }
}

// --- palm-muted guitar -----------------------------------------------------------

/// Samples of an embedded 48 kHz mono 16-bit WAV.
fn wav_samples(bytes: &[u8]) -> Vec<f32> {
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            let end = (pos + 8 + size).min(bytes.len());
            return bytes[pos + 8..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0)
                .collect();
        }
        pos += 8 + size + (size & 1);
    }
    panic!("no data chunk");
}

/// Normalised correlation of two equal-length slices.
fn corr(a: &[f32], b: &[f32]) -> f32 {
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (*x as f64) * (*y as f64))
        .sum();
    let ea: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum();
    let eb: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum();
    (dot / (ea * eb).sqrt().max(1e-30)) as f32
}

#[test]
fn legacy_recorded_guitar_is_unchanged() {
    // The existing instrument keeps its two articulations, so a normalized
    // automation value of 1.0 still means the staccato "Muted" recordings.
    let g = peel_instruments::descriptors()
        .iter()
        .find(|d| d.type_id == GUITAR)
        .unwrap();
    let art = g.param_by_key("play.articulation").unwrap();
    assert_eq!(art.max, 1.0);
    assert_eq!(art.choices, &["Sustain", "Muted"]);
    assert_eq!(art.from_normalized(1.0), 1.0);
    assert_eq!(g.params.len(), 12);
    let names: Vec<&str> = peel_instruments::presets(GUITAR)
        .iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(
        names,
        ["Clean", "High Gain", "Palm Mute", "Crunch", "Direct"]
    );
    let palm_preset = &peel_instruments::presets(GUITAR)[2];
    assert!(palm_preset.params.contains(&("play.articulation", 1.0)));
    assert!(palm_preset.params.contains(&("play.mute", 0.45)));
    // A normalized automation event of 1.0 renders exactly the explicit
    // staccato setting.
    let ev = |d: &dyn Device| {
        let id = d.descriptor().param_by_key("play.articulation").unwrap().id;
        let value = d
            .descriptor()
            .param_by_key("play.articulation")
            .unwrap()
            .from_normalized(1.0);
        vec![
            (0, EventKind::Param { id, value }),
            note_on(64, 45, 0.8, 1),
            note_off(s(0.2), 45, 1),
        ]
    };
    let mut a = inst(GUITAR, "Clean");
    let e = ev(a.as_ref());
    let x = run(a.as_mut(), &e, s(0.6));
    let mut b = inst(GUITAR, "Clean");
    set(b.as_mut(), "play.articulation", 1.0);
    let y = run(
        b.as_mut(),
        &[note_on(64, 45, 0.8, 1), note_off(s(0.2), 45, 1)],
        s(0.6),
    );
    assert_eq!(x, y, "normalized 1.0 no longer plays staccato");
}

#[test]
fn palm_descriptor_and_presets() {
    let d = peel_instruments::descriptors()
        .iter()
        .find(|d| d.type_id == PALM)
        .expect("registered");
    assert_eq!(d.name, "Palm-Muted Guitar");
    assert_eq!(d.category, "Guitar");
    assert_eq!(d.version, 1);
    let keys: Vec<&str> = d.params.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        [
            "play.release",
            "amp.output",
            "amp.gain",
            "amp.bass",
            "amp.mid",
            "amp.treble",
            "cab.type",
            "out.level",
            "out.velocity"
        ]
    );
    // Same stable IDs as the recorded guitar's matching controls.
    let g = peel_instruments::descriptors()
        .iter()
        .find(|d| d.type_id == GUITAR)
        .unwrap();
    for p in d.params {
        let q = g.param_by_key(p.key).unwrap();
        assert_eq!(p.id, q.id, "{}", p.key);
        assert_eq!((p.min, p.max), (q.min, q.max), "{}", p.key);
    }
    let all = peel_instruments::presets(PALM);
    let names: Vec<&str> = all.iter().map(|p| p.name).collect();
    assert_eq!(names, ["Metal Chugs", "Clean Palm Mute", "Direct"]);
    for p in all {
        assert_eq!(p.params.len(), d.params.len(), "{}", p.name);
        for q in d.params {
            assert!(
                p.params.iter().any(|(k, _)| *k == q.key),
                "{} {}",
                p.name,
                q.key
            );
        }
    }
    for (k, v) in all[0].params {
        assert_eq!(d.param_by_key(k).unwrap().default, *v, "{k}");
    }
    let chugs = &all[0];
    for (k, v) in [
        ("play.release", 0.025),
        ("amp.output", 0.0),
        ("amp.gain", 0.85),
        ("amp.bass", 2.0),
        ("amp.mid", -1.0),
        ("amp.treble", 0.0),
        ("cab.type", 2.0),
        ("out.level", 0.0),
        ("out.velocity", 0.6),
    ] {
        assert!(chugs.params.contains(&(k, v)), "Metal Chugs {k}");
    }
}

#[test]
fn palm_plays_every_recorded_take_as_recorded() {
    let total: usize = PALM_FILES.iter().map(|(_, t)| t.len()).sum();
    assert!(PALM_FILES.len() >= 40 && total >= PALM_FILES.len());
    let gap = s(0.4);
    for &(root, files) in PALM_FILES {
        let takes: Vec<Vec<f32>> = files.iter().map(|b| wav_samples(b)).collect();
        let strikes = files.len() + 2;
        let ev: Vec<Timed> = (0..strikes)
            .flat_map(|i| {
                [
                    note_on(i * gap, root, 0.8, i as u32 + 1),
                    note_off(i * gap + s(0.3), root, i as u32 + 1),
                ]
            })
            .collect();
        let mut d = inst(PALM, "Direct");
        let x = run(d.as_mut(), &ev, strikes * gap);
        assert!(all_finite(&x), "root {root}");
        let mut last = usize::MAX;
        for i in 0..strikes {
            let (best, c) = takes
                .iter()
                .enumerate()
                .map(|(t, src)| {
                    let n = s(0.04).min(src.len() / 2);
                    (t, corr(&x[i * gap..i * gap + n], &src[..n]))
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert!(c > 0.995, "root {root} strike {i}: best correlation {c}");
            assert!(
                peak(&x[i * gap..i * gap + s(0.005)]) > 1e-3,
                "root {root} {i}: late"
            );
            if takes.len() > 1 {
                assert_ne!(best, last, "root {root} strike {i} repeated its take");
            }
            last = best;
        }
        // Deterministic.
        let mut e = inst(PALM, "Direct");
        assert_eq!(x, run(e.as_mut(), &ev, strikes * gap), "root {root}");
    }
}

#[test]
fn palm_keeps_pitch_at_every_rate_without_octave_shift() {
    // Low E (key 40) has a recording of its own: played back untransposed.
    assert!(PALM_FILES.iter().any(|(k, _)| *k == 40));
    let render = |root: u8, sr: f32| {
        let mut d = at_rate(PALM, "Direct", sr);
        let x = render_at(
            d.as_mut(),
            &[note_on(0, root, 0.8, 1)],
            (0.3 * sr) as usize,
            256,
            sr,
        );
        let a = (0.02 * sr) as usize;
        detect_pitch(&x[a..a + (0.1 * sr) as usize], sr)
    };
    for &(root, files) in PALM_FILES
        .iter()
        .filter(|(k, _)| [40u8, 45, 52, 64].contains(k))
    {
        // At 48 kHz the note is one of its recordings, untransposed.
        let at48 = render(root, SR);
        let nearest_src = files
            .iter()
            .map(|b| {
                let src = wav_samples(b);
                cents(detect_pitch(&src[s(0.02)..s(0.12)], SR), at48).abs()
            })
            .fold(f32::INFINITY, f32::min);
        assert!(
            nearest_src < 5.0,
            "root {root}: {at48} Hz matches no recording"
        );
        if root == 40 {
            assert!(
                cents(at48, key_hz(40.0)).abs() < 60.0,
                "low E plays {at48} Hz"
            );
        }
        // Other rates resample to the same pitch (the estimate drifts a
        // little on a short palm note).
        for sr in [44_100.0f32, 96_000.0] {
            let got = render(root, sr);
            assert!(
                cents(got, at48).abs() < 25.0,
                "root {root} at {sr}: {got} vs {at48}"
            );
        }
    }
    // Keys one octave past either end transpose, further out are silent.
    let lo = PALM_FILES.iter().map(|(k, _)| *k).min().unwrap();
    let hi = PALM_FILES.iter().map(|(k, _)| *k).max().unwrap();
    for (key, sounds) in [
        (lo - 12, true),
        (lo - 13, false),
        (hi + 12, true),
        (hi + 13, false),
    ] {
        let mut d = inst(PALM, "Direct");
        let x = run(d.as_mut(), &[note_on(0, key, 0.8, 1)], s(0.2));
        assert_eq!(peak(&x) > 1e-3, sounds, "key {key}");
    }
}

#[test]
fn palm_differs_from_staccato_and_has_no_fret_noise() {
    let mut p = inst(PALM, "Direct");
    let palm = run(p.as_mut(), &[note_on(0, 45, 0.9, 1)], s(0.5));
    let mut g = inst(GUITAR, "Direct");
    set(g.as_mut(), "play.articulation", 1.0);
    let stac = run(g.as_mut(), &[note_on(0, 45, 0.9, 1)], s(0.5));
    assert!(peak(&palm) > 3e-3);
    let n = s(0.05);
    assert!(
        corr(&palm[..n], &stac[..n]).abs() < 0.9,
        "same recording as staccato"
    );
    // Attack level matches the staccato set within a few dB.
    let (a, b) = (db(rms(&palm[..s(0.08)])), db(rms(&stac[..s(0.08)])));
    assert!((a - b).abs() < 4.0, "attack {a} vs {b}");
    // Note-off adds nothing: the released note only decays.
    let mut q = inst(PALM, "Direct");
    let x = run(
        q.as_mut(),
        &[note_on(0, 45, 0.9, 1), note_off(s(0.15), 45, 1)],
        s(0.6),
    );
    assert!(db(rms(&x[s(0.3)..s(0.6)])) < -80.0, "sound after release");
}

#[test]
fn palm_chugs_decay_and_stop_at_every_tempo() {
    for bpm in [108.0f32, 160.0] {
        for (name, per_beat) in [("8ths", 2usize), ("triplets", 3), ("16ths", 4)] {
            let step = s(60.0 / bpm / per_beat as f32);
            let mut ev = Vec::new();
            for i in 0..12 {
                ev.push(note_on(i * step, 40 + (i % 2) as u8 * 2, 0.8, i as u32 + 1));
                ev.push(note_off(
                    i * step + step * 4 / 5,
                    40 + (i % 2) as u8 * 2,
                    i as u32 + 1,
                ));
            }
            // On the DI path every chug stops well before the next one.
            let mut d = inst(PALM, "Direct");
            let x = run(d.as_mut(), &ev, step * 13);
            assert!(all_finite(&x));
            for i in 0..12 {
                let note = &x[i * step..i * step + step / 4];
                let gap = &x[(i + 1) * step - s(0.004)..(i + 1) * step];
                assert!(
                    db(rms(gap)) < db(rms(note)) - 12.0,
                    "{name} at {bpm}: chug {i} gap {} vs note {}",
                    db(rms(gap)),
                    db(rms(note))
                );
            }
            // Through the high-gain amp, which compresses the tail, compare
            // against holding each chug.
            let held: Vec<Timed> = ev
                .iter()
                .copied()
                .filter(|e| !matches!(e.1, EventKind::NoteOff { .. }))
                .collect();
            let mut a = inst(PALM, "Metal Chugs");
            let x = run(a.as_mut(), &ev, step * 13);
            let mut b = inst(PALM, "Metal Chugs");
            let y = run(b.as_mut(), &held, step * 13);
            assert!(all_finite(&x));
            for i in 0..12 {
                let w = (i + 1) * step - s(0.004)..(i + 1) * step;
                let (r, h) = (db(rms(&x[w.clone()])), db(rms(&y[w])));
                // The short release must separate even 16ths at 160 BPM
                // through the amp, whose gain lifts the recorded tail.
                let need = 2.0;
                assert!(
                    r < h - need,
                    "Metal Chugs {name} at {bpm}: chug {i} released {r:.1} dB, held {h:.1} dB"
                );
            }
        }
    }
    // A held chug decays on its own, without note-off (the recording's
    // natural palm-mute decay at Mute 0).
    let mut d = inst(PALM, "Direct");
    let x = run(d.as_mut(), &[note_on(0, 40, 0.8, 1)], s(0.4));
    assert!(db(rms(&x[s(0.25)..s(0.35)])) < db(rms(&x[..s(0.05)])) - 10.0);
}

#[test]
fn palm_per_voice_expression() {
    // Pitch: a +12 snapshot plays an octave up.
    let base = |ev: &[Timed]| {
        let mut d = inst(PALM, "Direct");
        run(d.as_mut(), ev, s(0.3))
    };
    let plain = base(&[note_on(0, 52, 0.8, 1)]);
    let bent = base(&[
        note_on(0, 52, 0.8, 1),
        (
            0,
            EventKind::VoiceExpression {
                voice: 1,
                pitch: 12.0,
                pressure: 0.0,
                timbre: 0.5,
            },
        ),
    ]);
    let (w0, w1) = (s(0.02), s(0.1));
    let (p0, p1) = (
        detect_pitch(&plain[w0..w1], SR),
        detect_pitch(&bent[w0..w1], SR),
    );
    assert!(cents(p1, 2.0 * p0).abs() < 30.0, "bend {p0} -> {p1}");
    // Pressure raises the level, timbre changes the tone.
    let with = |pressure: f32, timbre: f32| {
        base(&[
            note_on(0, 45, 0.8, 1),
            (
                0,
                EventKind::VoiceExpression {
                    voice: 1,
                    pitch: 0.0,
                    pressure,
                    timbre,
                },
            ),
        ])
    };
    assert!(rms(&with(1.0, 0.5)) > 1.4 * rms(&with(0.0, 0.5)));
    assert!(max_diff(&with(0.0, 1.0), &with(0.0, 0.5)) > 1e-3);
    // Another voice's expression leaves this one alone.
    let other = base(&[
        note_on(0, 52, 0.8, 1),
        (
            0,
            EventKind::VoiceExpression {
                voice: 9,
                pitch: 12.0,
                pressure: 1.0,
                timbre: 1.0,
            },
        ),
    ]);
    assert_eq!(other, plain);
}

#[test]
fn palm_engine_contract() {
    // Block size and event offsets never change the output, at any rate.
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        for preset in ["Metal Chugs", "Direct"] {
            let mut ev = busy_phrase();
            let desc = inst(PALM, preset).descriptor();
            for (i, p) in desc.params.iter().enumerate() {
                ev.push((
                    4_000 + i * 997,
                    EventKind::Param {
                        id: p.id,
                        value: p.from_normalized(0.3 + 0.05 * i as f32),
                    },
                ));
            }
            ev.sort_by_key(|e| e.0);
            let mut r = at_rate(PALM, preset, sr);
            let reference = render_at(r.as_mut(), &ev, 40_000, 256, sr);
            assert!(peak(&reference) > 0.01 && all_finite(&reference));
            for block in [1, 61, 997] {
                let mut d = at_rate(PALM, preset, sr);
                let x = render_at(d.as_mut(), &ev, 40_000, block, sr);
                assert!(
                    max_diff(&x, &reference) < 1e-5,
                    "{preset} {sr} block {block}"
                );
            }
        }
    }
    // Panic, reset and natural end.
    let mut d = inst(PALM, "Metal Chugs");
    let x = run(
        d.as_mut(),
        &[
            note_on(0, 40, 1.0, 1),
            note_on(0, 47, 1.0, 2),
            (s(0.1), EventKind::AllSoundOff),
        ],
        s(0.3),
    );
    assert!(peak(&x[..s(0.1)]) > 0.01);
    assert_eq!(peak(&x[s(0.1)..]), 0.0);
    assert!(d.is_silent());
    let _ = run(d.as_mut(), &[note_on(0, 45, 1.0, 3)], s(0.1));
    d.reset();
    assert!(d.is_silent());
    let x = run(d.as_mut(), &[note_on(0, 45, 1.0, 4)], s(4.0));
    assert!(d.is_silent(), "held palm note never ended");
    assert_eq!(peak(&x[s(3.8)..]), 0.0);
    let _ = run(
        d.as_mut(),
        &[note_on(0, 45, 1.0, 5), (s(0.05), EventKind::AllNotesOff)],
        s(1.0),
    );
    assert!(d.is_silent());

    // No allocation on the audio path, and cheap.
    let mut d = inst(PALM, "Metal Chugs");
    let desc = d.descriptor();
    let mut l = vec![0.0f32; 256];
    let mut r = vec![0.0f32; 256];
    let mut events: Vec<DeviceEvent> = (0..40u32)
        .map(|i| DeviceEvent {
            offset: i * 6,
            kind: EventKind::NoteOn {
                key: 30 + (i * 7 % 60) as u8,
                velocity: 0.2 + 0.02 * i as f32,
                voice: i,
            },
        })
        .collect();
    for (i, p) in desc.params.iter().enumerate() {
        events.push(DeviceEvent {
            offset: 241 + i as u32,
            kind: EventKind::Param {
                id: p.id,
                value: p.from_normalized(0.7),
            },
        });
    }
    events.push(DeviceEvent {
        offset: 250,
        kind: EventKind::VoiceExpression {
            voice: 3,
            pitch: 2.0,
            pressure: 0.5,
            timbre: 0.2,
        },
    });
    let offs: Vec<DeviceEvent> = (0..40u32)
        .map(|i| DeviceEvent {
            offset: i,
            kind: EventKind::NoteOff {
                key: 30 + (i * 7 % 60) as u8,
                velocity: 0.0,
                voice: i,
            },
        })
        .collect();
    let ctx = ProcessContext::new(SR);
    EVENTS.with(|e| e.set(0));
    TRACK.with(|t| t.set(true));
    d.process(&ctx, &events, &mut l, &mut r);
    for _ in 0..50 {
        d.process(&ctx, &[], &mut l, &mut r);
    }
    d.process(&ctx, &offs, &mut l, &mut r);
    for _ in 0..20 {
        d.process(&ctx, &[], &mut l, &mut r);
    }
    d.process(
        &ctx,
        &[DeviceEvent {
            offset: 0,
            kind: EventKind::AllSoundOff,
        }],
        &mut l,
        &mut r,
    );
    d.reset();
    TRACK.with(|t| t.set(false));
    assert_eq!(EVENTS.with(Cell::get), 0, "allocated on the audio path");
    let mut ev = Vec::new();
    for i in 0..80 {
        for (j, k) in [40u8, 47, 52].iter().enumerate() {
            let v = (i * 3 + j) as u32 + 100;
            ev.push(note_on(i * s(0.125), *k, 0.8, v));
            ev.push(note_off(i * s(0.125) + s(0.1), *k, v));
        }
    }
    let t = Instant::now();
    let x = run(d.as_mut(), &ev, s(10.0));
    let load = t.elapsed().as_secs_f32() / 10.0;
    assert!(all_finite(&x) && peak(&x) < 1.5);
    assert!(load < 0.25, "{:.1} % of realtime", 100.0 * load);
}
