//! The recorded acoustic kit, `peel.drum.acoustic`.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use common::*;
use peel_dsp::device::apply_defaults;
use peel_dsp::{Device, DeviceEvent, DeviceKind, EventKind, ProcessContext};

const ID: &str = "peel.drum.acoustic";

const PIECES: [&str; 8] = [
    "Kick",
    "Snare",
    "Closed Hat",
    "Open Hat",
    "Ride",
    "Crash",
    "Rack Tom",
    "Floor Tom",
];

/// Velocities at which each layer plays unblended.
const LAYER_VELOCITY: [f32; 4] = [0.22, 0.48, 0.73, 0.95];

// --- allocation tracking (this test binary only) ---------------------------

struct Counting;

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static EVENTS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

fn note(size: usize) {
    // `try_with`: allocations can happen while a thread is shutting down.
    let _ = TRACK.try_with(|t| {
        if t.get() {
            EVENTS.with(|e| e.set(e.get() + 1));
            BYTES.with(|b| b.set(b.get() + size));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(0);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Runs `f` and returns how many allocations or frees it made, and how many
/// bytes it allocated, on this thread.
fn tracked(f: impl FnOnce()) -> (usize, usize) {
    EVENTS.with(|e| e.set(0));
    BYTES.with(|b| b.set(0));
    TRACK.with(|t| t.set(true));
    f();
    TRACK.with(|t| t.set(false));
    (EVENTS.with(Cell::get), BYTES.with(Cell::get))
}

// --- helpers -----------------------------------------------------------------

fn kit(preset: &str) -> Box<dyn Device> {
    kit_at(preset, SR)
}

fn kit_at(preset: &str, sample_rate: f32) -> Box<dyn Device> {
    let mut d = peel_instruments::create(ID).unwrap();
    d.prepare(sample_rate, 256);
    apply_defaults(d.as_mut());
    let desc = d.descriptor();
    let p = peel_instruments::presets(ID)
        .iter()
        .find(|p| p.name == preset)
        .unwrap_or_else(|| panic!("no preset {preset}"));
    for (k, v) in p.params {
        d.set_param(desc.param_by_key(k).unwrap().id, *v);
    }
    d
}

fn set(dev: &mut dyn Device, key: &str, value: f32) {
    let id = dev.descriptor().param_by_key(key).unwrap().id;
    dev.set_param(id, value);
}

/// Like `common::render` but at any sample rate.
fn render_at(
    dev: &mut dyn Device,
    events: &[Timed],
    total: usize,
    block: usize,
    sample_rate: f32,
) -> (Vec<f32>, Vec<f32>) {
    let mut out_l = Vec::with_capacity(total);
    let mut out_r = Vec::with_capacity(total);
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
        let mut ctx = ProcessContext::new(sample_rate);
        ctx.playing = true;
        let mut l = vec![7.0f32; n];
        let mut r = vec![7.0f32; n];
        dev.process(&ctx, &evs, &mut l, &mut r);
        out_l.extend_from_slice(&l);
        out_r.extend_from_slice(&r);
        pos += n;
    }
    (out_l, out_r)
}

fn mono(l: &[f32], r: &[f32]) -> Vec<f32> {
    l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)).collect()
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len().max(1) as f32).sqrt()
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

/// Mean absolute first difference over mean absolute level: rises with
/// spectral centroid.
fn brightness(buf: &[f32]) -> f32 {
    let diff: f32 = buf.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
    let sum: f32 = buf.iter().map(|x| x.abs()).sum();
    diff / sum.max(1e-9)
}

/// Seconds until the signal stays below `-60 dB` relative to its peak.
fn ring_time(buf: &[f32], sample_rate: f32) -> f32 {
    ring_time_db(buf, sample_rate, -60.0)
}

/// Seconds until the signal stays below `floor_db` relative to its peak.
fn ring_time_db(buf: &[f32], sample_rate: f32, floor_db: f32) -> f32 {
    let pk = peak(buf);
    let floor = pk * 10f32.powf(floor_db / 20.0);
    let last = buf.iter().rposition(|x| x.abs() > floor).unwrap_or(0);
    last as f32 / sample_rate
}

/// One hit of `piece` (a preset name) at `velocity`, rendered alone.
fn hit(piece: &str, velocity: f32, seconds: f32) -> Vec<f32> {
    let mut d = kit(piece);
    let n = (seconds * SR) as usize;
    let (l, r) = render(d.as_mut(), &[note_on(0, 60, velocity, 1)], n, 256);
    mono(&l, &r)
}

fn first_sound(buf: &[f32]) -> Option<usize> {
    buf.iter().position(|x| *x != 0.0)
}

// --- descriptor and presets --------------------------------------------------

#[test]
fn descriptor_and_presets() {
    let d = peel_instruments::descriptors()
        .iter()
        .find(|d| d.type_id == ID)
        .expect("listed");
    assert_eq!(d.name, "Acoustic Kit");
    assert_eq!(d.category, "Drums");
    assert_eq!(d.kind, DeviceKind::Instrument);
    assert_eq!(d.version, 1);
    let keys: Vec<_> = d.params.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        [
            "kit.piece",
            "kit.tune",
            "kit.decay",
            "kit.tone",
            "out.gain",
            "out.velocity",
            "out.width"
        ]
    );
    let piece = d.param_by_key("kit.piece").unwrap();
    assert_eq!(
        piece.choices,
        [
            "MIDI Kit",
            "Kick",
            "Snare",
            "Closed Hat",
            "Open Hat",
            "Ride",
            "Crash",
            "Rack Tom",
            "Floor Tom"
        ]
    );
    assert_eq!(piece.default, 0.0);

    let presets = peel_instruments::presets(ID);
    let names: Vec<_> = presets.iter().map(|p| p.name).collect();
    let mut expected = vec!["Full Kit"];
    expected.extend(PIECES);
    assert_eq!(names, expected);
    for (i, p) in presets.iter().enumerate() {
        // Every preset sets every parameter, so switching is exact.
        let mut set: Vec<_> = p.params.iter().map(|(k, _)| *k).collect();
        set.sort();
        let mut all = keys.clone();
        all.sort();
        assert_eq!(set, all, "{}", p.name);
        let piece = p.params.iter().find(|(k, _)| *k == "kit.piece").unwrap().1;
        assert_eq!(piece, i as f32, "{}", p.name);
    }
    for (k, v) in presets[0].params {
        assert_eq!(d.param_by_key(k).unwrap().default, *v, "{k}");
    }
}

#[test]
fn switching_presets_is_exact() {
    // A preset applied over another gives the same sound as on its own.
    for name in ["Full Kit", "Snare", "Crash"] {
        let mut fresh = kit(name);
        let mut over = kit("Floor Tom");
        set(over.as_mut(), "kit.tune", 7.0);
        set(over.as_mut(), "kit.decay", 0.2);
        set(over.as_mut(), "kit.tone", 900.0);
        set(over.as_mut(), "out.width", 0.0);
        let desc = over.descriptor();
        for (k, v) in peel_instruments::presets(ID)
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .params
        {
            over.set_param(desc.param_by_key(k).unwrap().id, *v);
        }
        let ev = [note_on(10, 38, 0.8, 1), note_on(4_000, 60, 0.6, 2)];
        let a = render(fresh.as_mut(), &ev, 24_000, 256);
        let b = render(over.as_mut(), &ev, 24_000, 256);
        assert_eq!(a, b, "{name}");
    }
}

// --- the recordings ----------------------------------------------------------

#[test]
fn every_recording_plays_finite_and_audible() {
    // At a layer's own velocity a hit always plays that layer, and every hit
    // moves to another take. A panic between hits (which keeps the hit
    // count) leaves each hit alone, so identical windows mean the same
    // take: nine hits per layer must find exactly three different takes.
    for piece in PIECES {
        for (layer, &v) in LAYER_VELOCITY.iter().enumerate() {
            let mut d = kit(piece);
            let gap = 4_800;
            let mut events = Vec::new();
            for i in 0..9 {
                events.push(note_on(i * gap, 60, v, i as u32));
                events.push(((i + 1) * gap - 1, EventKind::AllSoundOff));
            }
            let (l, r) = render(d.as_mut(), &events, 9 * gap, 256);
            assert!(all_finite(&l) && all_finite(&r), "{piece} layer {layer}");
            let m = mono(&l, &r);
            let mut takes: Vec<&[f32]> = Vec::new();
            for i in 0..9 {
                let a = &m[i * gap..(i + 1) * gap - 1];
                assert!(
                    peak(a) > 0.003,
                    "{piece} layer {layer} hit {i}: {}",
                    peak(a)
                );
                if !takes.iter().any(|b| max_diff(a, b) == 0.0) {
                    takes.push(a);
                }
            }
            assert_eq!(takes.len(), 3, "{piece} layer {layer}");
        }
    }
}

#[test]
fn creating_instances_shares_the_bank() {
    // The bank is parsed once per process; another instance must not copy
    // any of the 70 MB of audio.
    let _ = kit("Full Kit");
    let (_, bytes) = tracked(|| {
        let mut d = peel_instruments::create(ID).unwrap();
        d.prepare(96_000.0, 1_024);
        apply_defaults(d.as_mut());
        drop(d);
    });
    assert!(bytes < 256 * 1024, "instance allocated {bytes} bytes");
    let started = Instant::now();
    for _ in 0..20 {
        let _ = kit("Crash");
    }
    let each = started.elapsed().as_secs_f64() / 20.0;
    assert!(each < 0.02, "create took {:.1} ms", each * 1e3);
}

#[test]
fn plays_at_common_sample_rates() {
    for piece in ["Kick", "Snare", "Ride"] {
        let mut reference = None;
        for sr in [44_100.0f32, 48_000.0, 96_000.0] {
            let mut d = kit_at(piece, sr);
            let at = (0.01 * sr) as usize;
            let (l, r) = render_at(
                d.as_mut(),
                &[note_on(at, 60, 0.9, 1)],
                (3.0 * sr) as usize,
                256,
                sr,
            );
            assert!(all_finite(&l) && all_finite(&r), "{piece} {sr}");
            let m = mono(&l, &r);
            assert!(m[..at].iter().all(|x| *x == 0.0), "{piece} {sr}: early");
            let level = db(rms(&m[at..at + (0.1 * sr) as usize]));
            // -40 dB: well above the recordings' own noise floor.
            let ring = ring_time_db(&m[at..], sr, -40.0);
            let bright = brightness(&m[at..at + (0.05 * sr) as usize]) * sr;
            match reference {
                None => reference = Some((level, ring, bright)),
                Some((l0, r0, b0)) => {
                    assert!((level - l0).abs() < 0.5, "{piece} {sr}: {level} vs {l0} dB");
                    assert!(
                        (ring / r0 - 1.0).abs() < 0.05,
                        "{piece} {sr}: {ring} vs {r0} s"
                    );
                    // Brightness in Hz-like units stays put across rates.
                    assert!(
                        (bright / b0 - 1.0).abs() < 0.25,
                        "{piece} {sr}: {bright} vs {b0}"
                    );
                }
            }
        }
    }
}

#[test]
fn event_offsets_are_exact() {
    for at in [0usize, 1, 255, 256, 1_234, 9_999] {
        let mut a = kit("Full Kit");
        let mut b = kit("Full Kit");
        let (al, ar) = render(a.as_mut(), &[note_on(0, 38, 0.9, 1)], 20_000, 256);
        let (bl, br) = render(b.as_mut(), &[note_on(at, 38, 0.9, 1)], 20_000 + at, 256);
        assert!(bl[..at].iter().chain(&br[..at]).all(|x| *x == 0.0), "{at}");
        assert_eq!(&al[..], &bl[at..], "{at}");
        assert_eq!(&ar[..], &br[at..], "{at}");
        // The recording starts right away.
        let first = first_sound(&mono(&bl, &br)).unwrap();
        assert!(first - at < 48, "{at}: first sound at {first}");
    }
}

#[test]
fn is_deterministic_but_repeated_hits_differ() {
    let events: Vec<Timed> = (0..12)
        .map(|i| note_on(i * 8_000, if i % 2 == 0 { 36 } else { 38 }, 0.8, i as u32))
        .collect();
    let mut a = kit("Full Kit");
    let mut b = kit("Full Kit");
    let ra = render(a.as_mut(), &events, 100_000, 256);
    let rb = render(b.as_mut(), &events, 100_000, 256);
    assert_eq!(ra, rb);

    // The same note twice in a row never repeats the same recording.
    for piece in PIECES {
        let mut d = kit(piece);
        let gap = 24_000;
        let (l, r) = render(
            d.as_mut(),
            &(0..6)
                .map(|i| note_on(i * gap, 60, 0.8, i as u32))
                .collect::<Vec<_>>(),
            6 * gap,
            256,
        );
        let m = mono(&l, &r);
        for i in 1..6 {
            // Compare the first 10 ms, where the earlier hit's damped tail
            // is far below the new attack.
            let a = &m[(i - 1) * gap..(i - 1) * gap + 480];
            let b = &m[i * gap..i * gap + 480];
            assert!(max_diff(a, b) > 1e-3, "{piece}: hit {i} repeats the take");
        }
    }
}

#[test]
fn output_is_independent_of_block_size() {
    let mut events: Vec<Timed> = Vec::new();
    for i in 0..40usize {
        let key = [36u8, 42, 38, 42, 46, 49, 51, 48, 43, 44][i % 10];
        events.push(note_on(
            37 + i * 1_511,
            key,
            0.3 + 0.017 * i as f32,
            i as u32,
        ));
    }
    events.push((
        20_003,
        EventKind::VoiceExpression {
            voice: 13,
            pitch: 3.0,
            pressure: 0.5,
            timbre: 0.9,
        },
    ));
    let desc = peel_instruments::create(ID).unwrap().descriptor();
    for (at, key, value) in [
        (9_001, "kit.tone", 2_000.0),
        (30_000, "kit.decay", 0.4),
        (41_000, "kit.tune", -3.0),
        (50_000, "out.width", 0.3),
    ] {
        events.push((
            at,
            EventKind::Param {
                id: desc.param_by_key(key).unwrap().id,
                value,
            },
        ));
    }
    events.sort_by_key(|e| e.0);
    let mut reference = kit("Full Kit");
    let (rl, rr) = render(reference.as_mut(), &events, 70_000, 256);
    assert!(peak(&rl) > 0.01);
    for block in [1, 61, 997] {
        let mut d = kit("Full Kit");
        let (l, r) = render(d.as_mut(), &events, 70_000, block);
        assert_eq!(l, rl, "block {block}");
        assert_eq!(r, rr, "block {block}");
    }
}

// --- velocity ------------------------------------------------------------------

/// Attack level (RMS over 80 ms) of a hit at `velocity`, averaged over
/// several hits to even out takes and layer blending.
fn attack_db(piece: &str, velocity: f32) -> f32 {
    let mut d = kit(piece);
    let gap = 30_000;
    let n = 6;
    let events: Vec<Timed> = (0..n)
        .map(|i| note_on(i * gap, 60, velocity, i as u32))
        .collect();
    let (l, r) = render(d.as_mut(), &events, n * gap, 256);
    let m = mono(&l, &r);
    let p: f32 = (0..n)
        .map(|i| rms(&m[i * gap..i * gap + 3_840]).powi(2))
        .sum::<f32>()
        / n as f32;
    db(p.sqrt())
}

#[test]
fn soft_and_hard_hits_differ_in_level_and_timbre() {
    for piece in PIECES {
        let levels: Vec<f32> = LAYER_VELOCITY
            .iter()
            .map(|&v| attack_db(piece, v))
            .collect();
        for w in levels.windows(2) {
            // The source strengths are not evenly spaced (the crash's first
            // two and last two are close), so only require a rise.
            assert!(w[1] > w[0] + 0.3, "{piece}: layer levels {levels:?}");
        }
        assert!(
            levels[3] - levels[0] > 8.0,
            "{piece}: soft to hard only {:.1} dB",
            levels[3] - levels[0]
        );
    }
    // Hard strokes are not just louder copies of soft ones. Kick, toms, hat
    // and ride get brighter; the snare's soft strokes are wire-heavy and its
    // hard ones carry more body.
    for (piece, ms, brighter) in [
        ("Kick", 100, true),
        ("Rack Tom", 100, true),
        ("Floor Tom", 100, true),
        ("Closed Hat", 100, true),
        ("Ride", 100, true),
        ("Snare", 20, false),
    ] {
        let seconds = ms as f32 / 1_000.0;
        let soft = brightness(&hit(piece, LAYER_VELOCITY[0], seconds));
        let hard = brightness(&hit(piece, LAYER_VELOCITY[3], seconds));
        let ratio = hard / soft;
        if brighter {
            assert!(ratio > 1.1, "{piece}: soft {soft} hard {hard}");
        } else {
            assert!(ratio < 0.9, "{piece}: soft {soft} hard {hard}");
        }
    }
}

#[test]
fn velocity_sweep_has_no_jumps() {
    // The level follows velocity smoothly across layer boundaries; a soft
    // hit is never louder than a slightly harder one by more than take
    // variation.
    for piece in ["Kick", "Snare", "Closed Hat", "Ride"] {
        let mut prev: Option<f32> = None;
        let mut v = 0.15;
        while v <= 1.0 {
            let level = attack_db(piece, v);
            if let Some(p) = prev {
                assert!(
                    level > p - 1.5,
                    "{piece} {v:.2}: {level:.1} after {p:.1} dB"
                );
                // Below the softest layer the level follows velocity squared
                // (5 dB from 0.15 to 0.2); above it steps stay small.
                let limit = if v < 0.25 { 6.0 } else { 3.5 };
                assert!(
                    level < p + limit,
                    "{piece} {v:.2}: {level:.1} after {p:.1} dB"
                );
            }
            prev = Some(level);
            v += 0.05;
        }
    }
}

#[test]
fn velocity_control_narrows_the_range() {
    let range = |sens: f32| {
        let mut soft = kit("Snare");
        let mut hard = kit("Snare");
        set(soft.as_mut(), "out.velocity", sens);
        set(hard.as_mut(), "out.velocity", sens);
        let (sl, _) = render(soft.as_mut(), &[note_on(0, 60, 0.3, 1)], 4_000, 256);
        let (hl, _) = render(hard.as_mut(), &[note_on(0, 60, 1.0, 1)], 4_000, 256);
        db(rms(&hl)) - db(rms(&sl))
    };
    let full = range(1.0);
    let none = range(0.0);
    assert!(full > 10.0, "{full}");
    assert!(none < full - 8.0, "{none} vs {full}");
}

// --- kit behaviour -------------------------------------------------------------

fn kick_burst(bpm: f32, velocity: f32) -> (Vec<f32>, Vec<usize>) {
    // Two beats of sixteenth-note triplets.
    let step = (60.0 / bpm / 6.0 * SR) as usize;
    let onsets: Vec<usize> = (0..12).map(|i| 2_000 + i * step).collect();
    let events: Vec<Timed> = onsets
        .iter()
        .enumerate()
        .map(|(i, &t)| note_on(t, 36, velocity, i as u32))
        .collect();
    let mut d = kit("Full Kit");
    let (l, r) = render(d.as_mut(), &events, 2_000 + 12 * step + 48_000, 256);
    (mono(&l, &r), onsets)
}

#[test]
fn fast_kick_triplets_keep_attacks_and_bounded_low_end() {
    let single = {
        let mut d = kit("Kick");
        let (l, r) = render(d.as_mut(), &[note_on(0, 60, 0.95, 1)], 48_000, 256);
        mono(&l, &r)
    };
    let single_peak = peak(&single);
    let single_rms = rms(&single[..4_800]);
    for bpm in [108.0, 160.0] {
        let (m, onsets) = kick_burst(bpm, 0.95);
        let step = onsets[1] - onsets[0];
        for (i, &t) in onsets.iter().enumerate() {
            // Every stroke is heard: a fresh peak right after its onset,
            // well above what was ringing just before it.
            let before = rms(&m[t - 240..t]);
            let after = peak(&m[t..t + 480]);
            assert!(after > 0.5 * single_peak, "{bpm} hit {i}: {after}");
            assert!(rms(&m[t..t + 480]) > 2.0 * before, "{bpm} hit {i}: masked");
        }
        // Earlier hits are damped, so the low end does not pile up.
        let burst_rms = rms(&m[onsets[2]..onsets[11] + step]);
        assert!(
            burst_rms < 1.6 * single_rms,
            "{bpm}: burst RMS {burst_rms} vs single {single_rms}"
        );
        assert!(peak(&m) < 1.5 * single_peak, "{bpm}: peak {}", peak(&m));
        // But the last hit rings out naturally.
        let last = *onsets.last().unwrap();
        let tail = ring_time(&m[last..], SR);
        let alone = ring_time(&single, SR);
        assert!(tail > 0.7 * alone, "{bpm}: tail {tail} s vs {alone} s");
    }
}

#[test]
fn cymbals_ring_naturally_and_overlap() {
    for piece in ["Crash", "Ride"] {
        let m = hit(piece, 0.9, 9.5);
        let pk = peak(&m);
        let at2 = peak(&m[2 * 48_000..(2.1 * SR) as usize]);
        assert!(db(at2 / pk) > -40.0, "{piece}: {} dB at 2 s", db(at2 / pk));
        let at6 = peak(&m[6 * 48_000..(6.1 * SR) as usize]);
        assert!(at6 > 0.0 && db(at6 / pk) < db(at2 / pk), "{piece}");
    }
    // Two crashes a beat apart: the second adds to the first, which keeps
    // ringing untouched. `second` is the second hit on its own (a panic
    // right after the first keeps the hit count).
    let play = |events: &[Timed]| {
        let mut d = kit("Crash");
        let (l, r) = render(d.as_mut(), events, 96_000, 256);
        mono(&l, &r)
    };
    let both = play(&[note_on(0, 60, 0.9, 1), note_on(24_000, 60, 0.9, 2)]);
    let first = play(&[note_on(0, 60, 0.9, 1)]);
    let second = play(&[
        note_on(0, 60, 0.9, 1),
        (1, EventKind::AllSoundOff),
        note_on(24_000, 60, 0.9, 2),
    ]);
    for i in 24_000..96_000 {
        assert!(
            (both[i] - first[i] - second[i]).abs() < 1e-5,
            "first crash changed at {i}"
        );
    }
}

#[test]
fn closed_hat_chokes_open_hat_but_not_crash() {
    let choke_at = 24_000;
    let render_kit = |events: &[Timed]| {
        let mut d = kit("Full Kit");
        let (l, r) = render(d.as_mut(), events, 3 * 48_000, 256);
        mono(&l, &r)
    };
    // Open hat alone, then with the pedal closing it. The pedal hit alone
    // (as the second hit, after a panic) is subtracted to leave what is
    // left of the open hat.
    let open = render_kit(&[note_on(0, 46, 0.9, 1)]);
    let choked = render_kit(&[note_on(0, 46, 0.9, 1), note_on(choke_at, 44, 0.5, 2)]);
    let pedal = render_kit(&[
        note_on(0, 46, 0.9, 1),
        (1, EventKind::AllSoundOff),
        note_on(choke_at, 44, 0.5, 2),
    ]);
    assert_eq!(&open[..choke_at], &choked[..choke_at]);
    let rest: Vec<f32> = choked.iter().zip(&pedal).map(|(a, b)| a - b).collect();
    let level = |b: &[f32], ms: usize| rms(&b[choke_at + ms * 48..choke_at + (ms + 20) * 48]);
    // 70 ms to -60 dB: down over 25 dB at 30 ms, gone by 150 ms, while the
    // unchoked hat rings on.
    assert!(db(level(&rest, 30) / level(&open, 30)) < -20.0);
    assert!(level(&rest, 150) < 1e-3 * level(&open, 150));
    // It fades rather than cuts: the first 2 ms after the pedal still
    // carry most of the hat.
    assert!(level(&rest, 0) > 0.5 * level(&open, 0));

    // The crash under the same pedal is untouched once the short closed hat
    // has ended (closed hats are under a second long).
    let crash = render_kit(&[note_on(0, 49, 0.9, 1)]);
    let with_pedal = render_kit(&[note_on(0, 49, 0.9, 1), note_on(choke_at, 42, 0.9, 2)]);
    let tail = choke_at + 60_000..3 * 48_000;
    assert_eq!(
        &crash[tail.clone()],
        &with_pedal[tail],
        "hat choked the crash"
    );
    // And a ride keeps ringing through closed hats too.
    let ride = render_kit(&[note_on(0, 51, 0.9, 1)]);
    let with_hats = render_kit(&[
        note_on(0, 51, 0.9, 1),
        note_on(choke_at, 42, 0.9, 2),
        note_on(choke_at + 100, 46, 0.9, 3),
        note_on(choke_at + 9_000, 44, 0.9, 4),
    ]);
    let after = choke_at + 70_000..3 * 48_000;
    assert_eq!(&ride[after.clone()], &with_hats[after]);
}

#[test]
fn midi_kit_maps_general_midi_keys() {
    // A Full Kit key plays exactly what the matching piece preset plays at
    // C5, with toms transposed by their key's offset.
    let map: [(u8, &str, i8); 18] = [
        (35, "Kick", 0),
        (36, "Kick", 0),
        (38, "Snare", 0),
        (40, "Snare", 0),
        (42, "Closed Hat", 0),
        (44, "Closed Hat", 0),
        (46, "Open Hat", 0),
        (49, "Crash", 0),
        (57, "Crash", 0),
        (51, "Ride", 0),
        (53, "Ride", 0),
        (59, "Ride", 0),
        (50, "Rack Tom", 2),
        (48, "Rack Tom", 0),
        (47, "Rack Tom", -2),
        (45, "Floor Tom", 2),
        (43, "Floor Tom", 0),
        (41, "Floor Tom", -2),
    ];
    for (key, piece, st) in map {
        let mut full = kit("Full Kit");
        let mut one = kit(piece);
        let a = render(full.as_mut(), &[note_on(5, key, 0.8, 1)], 12_000, 256);
        let b = render(
            one.as_mut(),
            &[note_on(5, (60 + st) as u8, 0.8, 1)],
            12_000,
            256,
        );
        assert!(peak(&a.0) > 0.003, "{key}");
        assert_eq!(a, b, "key {key} should be {piece} {st:+}");
    }
    for key in [0u8, 34, 37, 39, 52, 54, 55, 56, 58, 60, 72, 127] {
        let mut full = kit("Full Kit");
        let (l, r) = render(full.as_mut(), &[note_on(0, key, 1.0, 1)], 4_800, 256);
        assert_eq!(peak(&l).max(peak(&r)), 0.0, "key {key} should be silent");
        assert!(full.is_silent());
    }
}

#[test]
fn single_pieces_ignore_the_key_for_articulation() {
    // Any key plays the chosen piece; C5 is its recorded pitch.
    for piece in PIECES {
        let at = |key: u8| {
            let mut d = kit(piece);
            let (l, r) = render(d.as_mut(), &[note_on(0, key, 0.8, 1)], 10 * 48_000, 1_024);
            mono(&l, &r)
        };
        let c5 = at(60);
        let high = at(72);
        let low = at(48);
        assert!(peak(&high) > 0.003 && peak(&low) > 0.003, "{piece}");
        // An octave up plays the same recording twice as fast.
        let ring_c5 = ring_time_db(&c5, SR, -40.0);
        let ring_hi = ring_time_db(&high, SR, -40.0);
        assert!(
            ring_hi < 0.6 * ring_c5 + 0.01,
            "{piece}: {ring_hi} vs {ring_c5}"
        );
        assert!(
            brightness(&high[..2_400]) > brightness(&c5[..2_400]),
            "{piece}"
        );
    }
}

#[test]
fn note_offs_do_not_cut_hits() {
    let mut a = kit("Full Kit");
    let mut b = kit("Full Kit");
    let ev = [note_on(0, 38, 0.9, 1), note_on(10, 49, 0.9, 2)];
    let (al, _) = render(a.as_mut(), &ev, 48_000, 256);
    let mut ev2 = ev.to_vec();
    ev2.push(note_off(100, 38, 1));
    ev2.push(note_off(120, 49, 2));
    ev2.push((200, EventKind::AllNotesOff));
    let (bl, _) = render(b.as_mut(), &ev2, 48_000, 256);
    assert_eq!(al, bl);
}

#[test]
fn panic_and_reset_silence_everything() {
    let busy: Vec<Timed> = (0..20)
        .map(|i| note_on(i * 400, [36u8, 38, 46, 49, 51][i % 5], 1.0, i as u32))
        .collect();
    let mut d = kit("Full Kit");
    let mut ev = busy.clone();
    ev.push((9_000, EventKind::AllSoundOff));
    let (l, r) = render(d.as_mut(), &ev, 12_000, 256);
    assert!(peak(&l[..9_000]) > 0.01);
    assert_eq!(peak(&l[9_000..]).max(peak(&r[9_000..])), 0.0);
    assert!(d.is_silent());

    // Reset silences and starts over: the same events play exactly as on a
    // fresh instance.
    let mut d = kit("Full Kit");
    let _ = render(d.as_mut(), &busy, 9_000, 256);
    d.reset();
    assert!(d.is_silent());
    let (l, _) = render(d.as_mut(), &[], 1_000, 256);
    assert_eq!(peak(&l), 0.0);
    let after = render(d.as_mut(), &busy, 30_000, 256);
    let mut fresh = kit("Full Kit");
    assert_eq!(after, render(fresh.as_mut(), &busy, 30_000, 256));
}

#[test]
fn every_hit_ends_in_exact_silence() {
    let mut d = kit("Full Kit");
    let ev: Vec<Timed> = [36u8, 38, 42, 46, 49, 51, 48, 43]
        .iter()
        .enumerate()
        .map(|(i, &k)| note_on(i * 100, k, 1.0, i as u32))
        .collect();
    let _ = render(d.as_mut(), &ev, 48_000 * 10, 1_024);
    assert!(d.is_silent(), "still sounding after the longest tail");
    let (l, r) = render(d.as_mut(), &[], 4_096, 256);
    assert_eq!(peak(&l).max(peak(&r)), 0.0);
}

#[test]
fn polyphony_survives_a_dense_flood() {
    // Rapid hits on every piece plus overlapping cymbals: 400 hits in four
    // seconds. Voices are stolen with a fade, never a click.
    let mut events = Vec::new();
    for i in 0..400usize {
        let key = [36u8, 38, 42, 46, 49, 51, 57, 48, 43, 44][i % 10];
        events.push(note_on(
            i * 480,
            key,
            0.6 + 0.4 * ((i * 7) % 10) as f32 / 10.0,
            i as u32,
        ));
    }
    let mut d = kit("Full Kit");
    let (l, r) = render(d.as_mut(), &events, 48_000 * 5, 128);
    assert!(all_finite(&l) && all_finite(&r));
    assert!(peak(&l).max(peak(&r)) < 4.0);
    let jump = l
        .windows(2)
        .chain(r.windows(2))
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(jump < 0.8, "largest step {jump}");
}

// --- controls and expression ---------------------------------------------------

#[test]
fn decay_tune_tone_and_width_work() {
    let base = hit("Crash", 0.9, 9.0);
    let short = {
        let mut d = kit("Crash");
        set(d.as_mut(), "kit.decay", 0.3);
        let (l, r) = render(d.as_mut(), &[note_on(0, 60, 0.9, 1)], 9 * 48_000, 256);
        mono(&l, &r)
    };
    // Decay shortens the tail but leaves the attack alone.
    assert_eq!(&base[..240], &short[..240]);
    assert!(ring_time(&short, SR) < 0.5 * ring_time(&base, SR));

    let tuned = {
        let mut d = kit("Floor Tom");
        set(d.as_mut(), "kit.tune", -12.0);
        let (l, r) = render(d.as_mut(), &[note_on(0, 60, 0.9, 1)], 6 * 48_000, 256);
        mono(&l, &r)
    };
    let tom = hit("Floor Tom", 0.9, 6.0);
    let ratio = ring_time(&tuned, SR) / ring_time(&tom, SR);
    assert!(
        (ratio - 2.0).abs() < 0.1,
        "an octave down plays twice as long: {ratio}"
    );

    let dark = {
        let mut d = kit("Snare");
        set(d.as_mut(), "kit.tone", 800.0);
        let (l, r) = render(d.as_mut(), &[note_on(0, 60, 0.9, 1)], 9_600, 256);
        mono(&l, &r)
    };
    let snare = hit("Snare", 0.9, 0.2);
    assert!(brightness(&dark) < 0.6 * brightness(&snare));

    let mut d = kit("Ride");
    set(d.as_mut(), "out.width", 0.0);
    let (l, r) = render(d.as_mut(), &[note_on(0, 60, 0.9, 1)], 24_000, 256);
    assert!(max_diff(&l, &r) < 1e-6, "width 0 is mono");
    let mut d = kit("Ride");
    let (l, r) = render(d.as_mut(), &[note_on(0, 60, 0.9, 1)], 24_000, 256);
    assert!(max_diff(&l, &r) > 1e-3, "the recordings are stereo");
}

#[test]
fn per_note_expression_reaches_each_voice() {
    let expr = |at: usize, voice: u32, pitch: f32, pressure: f32, timbre: f32| {
        (
            at,
            EventKind::VoiceExpression {
                voice,
                pitch,
                pressure,
                timbre,
            },
        )
    };
    let play = |events: &[Timed]| {
        let mut d = kit("Full Kit");
        let (l, r) = render(d.as_mut(), events, 6 * 48_000, 256);
        mono(&l, &r)
    };
    let plain = play(&[note_on(0, 43, 0.8, 7)]);

    // A snapshot for another voice changes nothing.
    assert_eq!(
        plain,
        play(&[note_on(0, 43, 0.8, 7), expr(0, 8, 12.0, 1.0, 1.0)])
    );

    // Pitch: up an octave plays the recording twice as fast.
    let up = play(&[note_on(0, 43, 0.8, 7), expr(0, 7, 12.0, 0.0, 0.5)]);
    let ratio = ring_time(&up, SR) / ring_time(&plain, SR);
    assert!((ratio - 0.5).abs() < 0.05, "{ratio}");

    // Pressure: 1 + 0.6 p.
    let pressed = play(&[note_on(0, 43, 0.8, 7), expr(0, 7, 0.0, 1.0, 0.5)]);
    let gain = peak(&pressed) / peak(&plain);
    assert!((gain - 1.6).abs() < 0.02, "{gain}");

    // Timbre: brighter at 1, darker at 0.
    let bright = play(&[note_on(0, 43, 0.8, 7), expr(0, 7, 0.0, 0.0, 1.0)]);
    let dull = play(&[note_on(0, 43, 0.8, 7), expr(0, 7, 0.0, 0.0, 0.0)]);
    let b = |x: &[f32]| brightness(&x[..4_800]);
    assert!(b(&bright) > b(&plain) && b(&plain) > b(&dull));

    // Two voices on the same key bend independently: a later snapshot for
    // the first voice leaves the second voice's sound unchanged.
    let both = [note_on(0, 43, 0.8, 7), note_on(12_000, 43, 0.8, 9)];
    let a = play(&both);
    let mut moved = both.to_vec();
    moved.push(expr(18_000, 7, -5.0, 0.0, 0.5));
    let b = play(&moved);
    assert_eq!(&a[..18_000], &b[..18_000]);
    // Only the first voice (damped by the re-strike) changes; the second
    // dominates and still matches closely.
    let diff = rms(&a[18_000..30_000]
        .iter()
        .zip(&b[18_000..30_000])
        .map(|(x, y)| x - y)
        .collect::<Vec<_>>());
    assert!(diff > 0.0 && diff < 0.2 * rms(&a[18_000..30_000]), "{diff}");
}

// --- realtime ------------------------------------------------------------------

#[test]
fn audio_path_never_allocates() {
    let mut d = kit("Full Kit");
    let desc = d.descriptor();
    let mut l = vec![0.0f32; 256];
    let mut r = vec![0.0f32; 256];
    let mut blocks: Vec<Vec<DeviceEvent>> = Vec::new();
    for b in 0..200u32 {
        let mut evs = Vec::new();
        for j in 0..4u32 {
            let i = b * 4 + j;
            evs.push(DeviceEvent {
                offset: j * 60,
                kind: EventKind::NoteOn {
                    key: [36u8, 38, 42, 46, 49, 51, 48, 43, 44, 57, 60][(i % 11) as usize],
                    velocity: 0.2 + (i % 9) as f32 * 0.1,
                    voice: i,
                },
            });
        }
        evs.push(DeviceEvent {
            offset: 200,
            kind: EventKind::VoiceExpression {
                voice: b * 4,
                pitch: (b % 7) as f32,
                pressure: 0.5,
                timbre: 0.8,
            },
        });
        if b % 10 == 5 {
            for p in desc.params {
                evs.push(DeviceEvent {
                    offset: 230,
                    kind: EventKind::Param {
                        id: p.id,
                        value: p.from_normalized(((b / 10) % 3) as f32 / 2.0),
                    },
                });
            }
        }
        if b == 150 {
            evs.push(DeviceEvent {
                offset: 250,
                kind: EventKind::AllSoundOff,
            });
        }
        blocks.push(evs);
    }
    let ctx = ProcessContext::new(SR);
    let (events, _) = tracked(|| {
        for evs in &blocks {
            d.process(&ctx, evs, &mut l, &mut r);
        }
        d.reset();
        d.process(&ctx, &blocks[0], &mut l, &mut r);
    });
    assert_eq!(events, 0, "allocations or frees on the audio path");
}

#[test]
fn renders_well_within_real_time() {
    // A dense kit: sixteenth notes at 180 BPM on kick, snare and hats with
    // cymbals every beat (most voices are long cymbal tails), 20 seconds.
    let step = (60.0 / 180.0 / 4.0 * SR) as usize;
    let total = 20 * 48_000;
    let mut events = Vec::new();
    let mut id = 0u32;
    for s in 0..total / step {
        let t = s * step;
        for key in [36u8, 42] {
            events.push(note_on(t, key, 0.8, id));
            id += 1;
        }
        if s % 2 == 1 {
            events.push(note_on(t, 38, 0.7, id));
            id += 1;
        }
        if s % 4 == 0 {
            events.push(note_on(t, if s % 8 == 0 { 49 } else { 51 }, 0.9, id));
            id += 1;
        }
    }
    let mut d = kit("Full Kit");
    let started = Instant::now();
    let (l, _) = render(d.as_mut(), &events, total, 256);
    let secs = started.elapsed().as_secs_f64();
    assert!(peak(&l) > 0.05);
    // Generous for debug builds and loaded CI machines.
    assert!(secs < 10.0, "20 s of audio took {secs:.2} s");
}
