//! The Acoustic and Circuit drum models on Kick, Snare, Hat and Tom.
//!
//! These check audible properties (level, onsets, decay order, velocity,
//! variation, chokes, retrigger clarity, aliasing, cost) rather than exact
//! sample values, and that the electronic sounds come back untouched when
//! both switches are off.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use common::*;
use peel_dsp::device::apply_defaults;
use peel_dsp::{Device, DeviceEvent, EventKind, ProcessContext};

// ------------------------------------------------------------- allocation

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
}

fn note_alloc() {
    if TRACK.with(Cell::get) {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
    }
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

// ---------------------------------------------------------------- presets

const KICK: &str = "peel.drum.kick";
const SNARE: &str = "peel.drum.snare";
const HAT: &str = "peel.drum.hat";
const TOM: &str = "peel.drum.tom";

const ACOUSTIC: [(&str, &[&str]); 4] = [
    (KICK, &["Acoustic Kick", "Metal Kick"]),
    (SNARE, &["Acoustic Snare", "Rock Snare"]),
    (
        HAT,
        &[
            "Acoustic Closed",
            "Acoustic Open",
            "Acoustic Ride",
            "Acoustic Crash",
            "Dusty",
            "Dusty Open",
        ],
    ),
    (TOM, &["Acoustic Rack", "Acoustic Floor"]),
];

const CIRCUIT: [(&str, &[&str]); 4] = [
    (
        KICK,
        &["909 Kick", "808 Kick", "Hard Kick", "Synthwave Kick"],
    ),
    (SNARE, &["909 Snare", "808 Snare"]),
    (
        HAT,
        &[
            "808 Closed",
            "808 Open",
            "909 Closed",
            "Trap Tight",
            "Trap Open",
        ],
    ),
    (TOM, &["808 Tom", "909 Tom"]),
];

fn switches(id: &str) -> (&'static str, &'static str) {
    if id == HAT {
        ("tone.acoustic", "tone.circuit")
    } else {
        ("body.acoustic", "body.circuit")
    }
}

fn every_new() -> impl Iterator<Item = (&'static str, &'static str)> {
    ACOUSTIC
        .iter()
        .chain(CIRCUIT.iter())
        .flat_map(|(id, names)| names.iter().map(move |n| (*id, *n)))
}

/// Loads a preset the way the app does: defaults first, then its values.
fn load(dev: &mut dyn Device, id: &str, name: &str) {
    let desc = dev.descriptor();
    apply_defaults(dev);
    let p = peel_instruments::presets(id)
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("{id} has no preset {name}"));
    for (k, v) in p.params {
        dev.set_param(desc.param_by_key(k).unwrap().id, *v);
    }
}

fn at_rate(id: &str, name: &str, sr: f32) -> Box<dyn Device> {
    let mut d = peel_instruments::create(id).unwrap();
    d.prepare(sr, 256);
    load(d.as_mut(), id, name);
    d
}

fn drum(id: &str, name: &str) -> Box<dyn Device> {
    at_rate(id, name, SR)
}

/// Renders at any sample rate; returns the mid (L+R)/2 signal.
fn render_at(
    dev: &mut dyn Device,
    events: &[Timed],
    total: usize,
    block: usize,
    sr: f32,
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
        let mut ctx = ProcessContext::new(sr);
        ctx.playing = true;
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        dev.process(&ctx, &evs, &mut l, &mut r);
        out_l.extend_from_slice(&l);
        out_r.extend_from_slice(&r);
        pos += n;
    }
    (out_l, out_r)
}

fn mid(l: &[f32], r: &[f32]) -> Vec<f32> {
    l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)).collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

/// Spectral centroid estimate from the derivative-to-signal RMS ratio.
fn brightness(x: &[f32], sr: f32) -> f32 {
    let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
    sr / std::f32::consts::TAU * rms(&d) / rms(x).max(1e-12)
}

/// Milliseconds until the 5 ms RMS envelope has fallen `drop_db` below
/// its peak.
fn fall_ms(x: &[f32], drop_db: f32, sr: f32) -> f32 {
    let w = (0.005 * sr) as usize;
    let env: Vec<f32> = x.chunks(w).map(rms).collect();
    let (start, top) = env
        .iter()
        .enumerate()
        .fold((0, 0.0f32), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
    let floor = top * 10f32.powf(-drop_db / 20.0);
    env.iter()
        .enumerate()
        .skip(start)
        .find(|(_, v)| **v < floor)
        .map_or(f32::INFINITY, |(i, _)| i as f32 * 5.0)
}

/// RMS of a band-passed copy (a quick spectral band level).
fn band(x: &[f32], hz: f32, sr: f32) -> f32 {
    let mut f = peel_dsp::filter::Svf::new();
    f.set_q(hz, 1.2, sr);
    let y: Vec<f32> = x.iter().map(|&v| f.tick(v).band_unity).collect();
    rms(&y)
}

// ------------------------------------------------------------------ tests

#[test]
fn new_presets_exist_and_fully_set_both_switches() {
    let acoustic: Vec<_> = ACOUSTIC
        .iter()
        .flat_map(|(id, n)| n.iter().map(move |n| (*id, *n)))
        .collect();
    let circuit: Vec<_> = CIRCUIT
        .iter()
        .flat_map(|(id, n)| n.iter().map(move |n| (*id, *n)))
        .collect();
    for id in [KICK, SNARE, HAT, TOM] {
        let desc = peel_instruments::create(id).unwrap().descriptor();
        let (a, c) = switches(id);
        // Old projects have neither key: both must default to off.
        assert_eq!(desc.param_by_key(a).unwrap().default, 0.0, "{id}");
        assert_eq!(desc.param_by_key(c).unwrap().default, 0.0, "{id}");
        for p in peel_instruments::presets(id) {
            let get = |k: &str| p.params.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
            let want = if acoustic.contains(&(id, p.name)) {
                (1.0, 0.0)
            } else if circuit.contains(&(id, p.name)) {
                (0.0, 1.0)
            } else {
                (0.0, 0.0)
            };
            // Every preset states both switches, so loading any preset after
            // any other leaves no model switched on by accident.
            assert_eq!(
                (get(a), get(c)),
                (Some(want.0), Some(want.1)),
                "{id} preset {}",
                p.name
            );
        }
    }
}

#[test]
fn switching_back_plays_the_electronic_kit_exactly() {
    let legacy = [
        (KICK, "Punch"),
        (SNARE, "Crack"),
        (HAT, "Open"),
        (TOM, "Floor"),
    ];
    let later = [
        note_on(0, 60, 1.0, 10),
        note_on(7_000, 55, 0.6, 11),
        note_off(9_000, 55, 11),
        note_on(12_345, 67, 0.8, 12),
    ];
    for (id, old) in legacy {
        let (a_key, c_key) = switches(id);
        // Flipping the switches with nothing sounding changes nothing.
        let mut plain = drum(id, old);
        let mut flipped = drum(id, old);
        for (k, v) in [(a_key, 1.0), (c_key, 1.0), (a_key, 0.0), (c_key, 0.0)] {
            flipped.set_param(flipped.descriptor().param_by_key(k).unwrap().id, v);
        }
        let (pl, pr) = render(plain.as_mut(), &later, 30_000, 256);
        let (fl, fr) = render(flipped.as_mut(), &later, 30_000, 256);
        assert!(pl == fl && pr == fr, "{id}: flipped switches changed {old}");

        // Whichever new model played before, once it has rung out and the
        // old preset is loaded again, the old kit plays exactly the same.
        let early = [note_on(0, 60, 0.9, 1), note_on(4_800, 62, 0.5, 2)];
        let news: Vec<&str> = ACOUSTIC
            .iter()
            .chain(CIRCUIT.iter())
            .filter(|(i, _)| *i == id)
            .flat_map(|(_, n)| n.iter().copied())
            .collect();
        let mut reference: Option<(Vec<f32>, Vec<f32>)> = None;
        for new in news {
            let mut d = drum(id, new);
            render(d.as_mut(), &early, 6 * SR as usize, 256);
            assert!(d.is_silent(), "{id} {new} should ring out in 6 s");
            load(d.as_mut(), id, old);
            let out = render(d.as_mut(), &later, 30_000, 256);
            assert!(peak(&out.0) > 0.05, "{id} {old} silent after {new}");
            match &reference {
                None => reference = Some(out),
                Some(r) => assert!(*r == out, "{id}: {old} after {new} differs"),
            }
        }
    }
}

#[test]
fn new_presets_are_finite_clean_and_level_matched_at_common_rates() {
    for (id, name) in every_new() {
        for sr in [44_100.0f32, 48_000.0, 96_000.0] {
            let mut d = at_rate(id, name, sr);
            let n = (3.0 * sr) as usize;
            let (l, r) = render_at(d.as_mut(), &[note_on(0, 60, 1.0, 1)], n, 256, sr);
            assert!(all_finite(&l) && all_finite(&r), "{id} {name} @ {sr}");
            let pk = peak(&l).max(peak(&r));
            assert!((0.1..1.2).contains(&pk), "{id} {name} @ {sr}: peak {pk}");
            let m = mid(&l, &r);
            let dc = m.iter().sum::<f32>() / m.len() as f32;
            assert!(dc.abs() < 2e-3, "{id} {name} @ {sr}: DC {dc}");
            // The hit starts from silence rather than on a step.
            assert!(
                l[0].abs() < 0.03 && r[0].abs() < 0.03,
                "{id} {name}: {}",
                l[0]
            );
            if id == KICK {
                assert!(l[0].abs() < 1e-3, "{name}: kick first sample {}", l[0]);
            }
        }
    }
}

#[test]
fn new_models_are_block_size_and_run_invariant() {
    let events = [
        note_on(0, 60, 1.0, 1),
        note_on(3_001, 64, 0.4, 2),
        (
            5_000,
            EventKind::VoiceExpression {
                voice: 3,
                pitch: 0.5,
                pressure: 0.4,
                timbre: 0.7,
            },
        ),
        note_on(5_000, 57, 0.8, 3),
        note_off(6_000, 57, 3),
        note_on(6_002, 60, 0.95, 4),
        (9_000, EventKind::PitchBend { semitones: -1.0 }),
        note_on(9_500, 60, 0.7, 5),
    ];
    for (id, name) in every_new() {
        let mut a = drum(id, name);
        let (l1, r1) = render(a.as_mut(), &events, 24_000, 256);
        for block in [1usize, 61, 997] {
            let mut b = drum(id, name);
            let (l2, r2) = render(b.as_mut(), &events, 24_000, block);
            assert!(l1 == l2 && r1 == r2, "{id} {name}: block {block} differs");
        }
    }
}

#[test]
fn velocity_changes_level_and_tone() {
    for (id, name) in every_new() {
        let hit = |vel: f32| {
            let mut d = drum(id, name);
            let (l, r) = render(d.as_mut(), &[note_on(0, 60, vel, 1)], 24_000, 256);
            mid(&l, &r)
        };
        let (hard, soft) = (hit(1.0), hit(0.35));
        let w = (0.25 * SR) as usize;
        let gain = db(rms(&hard[..w])) - db(rms(&soft[..w]));
        assert!(gain > 4.0, "{id} {name}: only {gain:.1} dB from velocity");
        let a = (0.03 * SR) as usize;
        let acoustic = ACOUSTIC.iter().any(|(i, n)| *i == id && n.contains(&name));
        if id == SNARE && acoustic {
            // Soft strokes are relatively wirier: more top against the body.
            let ratio = |x: &[f32]| band(&x[..w], 6_000.0, SR) / band(&x[..w], 220.0, SR);
            assert!(
                ratio(&soft) > ratio(&hard) * 1.1,
                "{name}: wire balance {} vs {}",
                ratio(&soft),
                ratio(&hard)
            );
        } else {
            // Everything else gets brighter the harder it is hit.
            let (bh, bs) = (brightness(&hard[..a], SR), brightness(&soft[..a], SR));
            assert!(
                bh > bs * 1.02,
                "{id} {name}: attack {bh:.0} Hz vs soft {bs:.0} Hz"
            );
        }
    }
}

#[test]
fn acoustic_hits_vary_but_onsets_stay_exact() {
    for (id, names) in ACOUSTIC {
        for name in names {
            let gap = 24_000usize;
            let events: Vec<Timed> = (0..4)
                .map(|i| note_on(i * gap, 60, 0.9, i as u32 + 1))
                .collect();
            let mut d = drum(id, name);
            let (l, r) = render(d.as_mut(), &events, 4 * gap, 256);
            let m = mid(&l, &r);
            let w = (0.03 * SR) as usize;
            let hits: Vec<&[f32]> = (0..4).map(|i| &m[i * gap..i * gap + w]).collect();
            let levels: Vec<f32> = hits.iter().map(|h| db(rms(h))).collect();
            for i in 1..4 {
                let diff: Vec<f32> = hits[i]
                    .iter()
                    .zip(hits[i - 1])
                    .map(|(a, b)| a - b)
                    .collect();
                let change = rms(&diff) / rms(hits[i]);
                assert!(
                    change > 0.02,
                    "{id} {name}: hits {i} and {} identical",
                    i - 1
                );
                assert!(
                    (levels[i] - levels[0]).abs() < 1.5,
                    "{id} {name}: levels {levels:?}"
                );
                // The onset lands on the note: silence-to-sound happens in the
                // first millisecond, never later.
                let first = hits[i].iter().position(|v| v.abs() > 1e-3).unwrap();
                assert!(
                    first < (0.001 * SR) as usize,
                    "{id} {name}: onset at {first}"
                );
            }
            // The same events always render the same audio.
            let mut again = drum(id, name);
            let (l2, _) = render(again.as_mut(), &events, 4 * gap, 256);
            assert!(l == l2, "{id} {name} not deterministic");
        }
    }
}

#[test]
fn hats_decay_in_the_right_order_for_both_kits() {
    let tail = |name: &str| {
        let mut d = drum(HAT, name);
        let (l, r) = render(d.as_mut(), &[note_on(0, 60, 0.9, 1)], 6 * SR as usize, 256);
        fall_ms(&mid(&l, &r), 40.0, SR)
    };
    let closed = tail("Acoustic Closed");
    let open = tail("Acoustic Open");
    let ride = tail("Acoustic Ride");
    let crash = tail("Acoustic Crash");
    assert!(closed < 120.0, "closed {closed}");
    assert!(open > 3.0 * closed, "open {open} vs closed {closed}");
    assert!(
        ride > 2.0 * open && crash > 2.0 * open,
        "ride {ride} crash {crash} open {open}"
    );
    let c808 = tail("808 Closed");
    let o808 = tail("808 Open");
    assert!(
        c808 < 100.0 && o808 > 4.0 * c808,
        "808 closed {c808} open {o808}"
    );
}

#[test]
fn hats_choke_on_retrigger_and_release_and_panic() {
    for (open, closed) in [
        ("Acoustic Open", "Acoustic Closed"),
        ("808 Open", "808 Closed"),
    ] {
        // An open hit cut by a closed one on the same device.
        let mut d = drum(HAT, open);
        let (l, _) = render(d.as_mut(), &[note_on(0, 60, 0.9, 1)], 24_000, 256);
        let ringing = rms(&l[14_400..19_200]);
        let mut d = drum(HAT, open);
        let decay = d.descriptor().param_by_key("amp.decay").unwrap().id;
        // A short hit 100 ms in: the open tail must not survive it.
        let evs = [
            note_on(0, 60, 0.9, 1),
            (
                4_800,
                EventKind::Param {
                    id: decay,
                    value: 0.06,
                },
            ),
            note_on(4_801, 60, 0.9, 2),
        ];
        let (l, _) = render(d.as_mut(), &evs, 24_000, 256);
        let after = rms(&l[14_400..19_200]);
        assert!(
            after < ringing * 0.05,
            "{open}: choke left {after} of {ringing}"
        );

        // Cut On Release silences within the choke time.
        let mut d = drum(HAT, open);
        let gate = d.descriptor().param_by_key("amp.gate").unwrap().id;
        d.set_param(gate, 1.0);
        let (l, _) = render(
            d.as_mut(),
            &[note_on(0, 60, 0.9, 1), note_off(4_800, 60, 1)],
            12_000,
            256,
        );
        assert!(
            peak(&l[4_800 + 2_000..]) < 1e-4,
            "{open}: release did not cut"
        );
        assert!(d.is_silent());

        // Panic stops everything at once.
        let mut d = drum(HAT, closed);
        let (l, _) = render(
            d.as_mut(),
            &[note_on(0, 60, 0.9, 1), (2_000, EventKind::AllSoundOff)],
            6_000,
            256,
        );
        assert!(peak(&l[2_000..]) == 0.0 && d.is_silent(), "{closed}: panic");
    }
    // Panic also clears the snare room and every other new model.
    for (id, name) in every_new() {
        let mut d = drum(id, name);
        let (l, _) = render(
            d.as_mut(),
            &[note_on(0, 60, 1.0, 1), (1_000, EventKind::AllSoundOff)],
            4_000,
            256,
        );
        assert!(
            peak(&l[1_000..]) == 0.0 && d.is_silent(),
            "{id} {name}: panic"
        );
    }
}

#[test]
fn fast_double_kicks_stay_separate() {
    // Sixteenth-note triplets at 160 BPM (62.5 ms apart), as in a metal
    // double-kick run, and at 108 BPM (92.6 ms).
    for (name, bpm, circuit) in [
        ("Metal Kick", 160.0, false),
        ("Acoustic Kick", 108.0, false),
        ("Hard Kick", 160.0, true),
        ("909 Kick", 108.0, true),
    ] {
        let gap = (60.0 / bpm / 6.0 * SR as f64) as usize;
        let events: Vec<Timed> = (0..8)
            .map(|i| note_on(i * gap, 60, 0.9, i as u32 + 1))
            .collect();
        let mut d = drum(KICK, name);
        let (l, _) = render(d.as_mut(), &events, 9 * gap, 256);
        let w = (0.012 * SR) as usize;
        let peaks: Vec<f32> = (0..8).map(|i| peak(&l[i * gap..i * gap + w])).collect();
        // Attack energy above 2 kHz just after and just before each hit.
        let top: Vec<f32> = {
            let mut f = peel_dsp::filter::Svf::new();
            f.set_q(2_000.0, 0.7, SR);
            l.iter().map(|&v| f.tick(v).high).collect()
        };
        let t = (0.004 * SR) as usize;
        for i in 1..8 {
            // Every hit lands as hard as the first; nothing stacks up.
            assert!(
                (db(peaks[i]) - db(peaks[0])).abs() < 2.0,
                "{name}: peaks {peaks:?}"
            );
            let attack = db(rms(&top[i * gap..i * gap + t])) - db(rms(&top[i * gap - t..i * gap]));
            assert!(attack > 10.0, "{name}: hit {i} attack only {attack:.1} dB");
            if !circuit {
                // A real head is damped by the next stroke: the previous
                // hit has fallen well away before it.
                let before = peak(&l[i * gap - w / 3..i * gap]);
                assert!(
                    db(peaks[i]) - db(before) > 4.0,
                    "{name}: hit {i} only {:.1} dB above the tail",
                    db(peaks[i]) - db(before)
                );
            }
        }
    }
}

#[test]
fn circuit_rolls_retrigger_without_clicks() {
    // A 32nd-note roll at 180 BPM: the circuits re-ping their resonators
    // (and the kick re-rises in the same voice), so the roll never jumps
    // harder, for its level, than a single hit's own attack.
    for (id, name) in [
        (SNARE, "909 Snare"),
        (SNARE, "808 Snare"),
        (TOM, "808 Tom"),
        (TOM, "909 Tom"),
        (KICK, "909 Kick"),
        (KICK, "Hard Kick"),
    ] {
        // Largest sample-to-sample step relative to the level around it.
        let jump = |l: &[f32]| {
            let w = 480;
            let mut worst = 0.0f32;
            for (i, s) in l.windows(2).enumerate().skip(1) {
                let lo = i.saturating_sub(w);
                let local = peak(&l[lo..(i + w).min(l.len())]).max(1e-6);
                worst = worst.max((s[1] - s[0]).abs() / local);
            }
            worst
        };
        // The tom's skin noise is noise by design; leave it out so only the
        // resonator retrigger is measured.
        let drum = |id: &str, name: &str| {
            let mut d = drum(id, name);
            if id == TOM {
                d.set_param(d.descriptor().param_by_key("body.stick").unwrap().id, 0.0);
            }
            d
        };
        let single = {
            let mut d = drum(id, name);
            let (l, _) = render(d.as_mut(), &[note_on(0, 60, 0.8, 1)], 8_000, 256);
            jump(&l)
        };
        let gap = (60.0 / 180.0 / 8.0 * SR as f64) as usize;
        let events: Vec<Timed> = (0..24)
            .map(|i| note_on(i * gap, 60, 0.8, i as u32 + 1))
            .collect();
        let mut d = drum(id, name);
        let (l, r) = render(d.as_mut(), &events, 26 * gap, 256);
        assert!(all_finite(&l) && all_finite(&r));
        let step = jump(&l);
        assert!(
            step < single * 1.5,
            "{name}: roll step {step} vs single {single}"
        );
        assert!(peak(&l) < 1.5, "{name}: roll builds up to {}", peak(&l));
    }
}

/// Power spectrum of a windowed segment (in-place radix-2 FFT).
fn spectrum(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    assert!(n.is_power_of_two());
    let mut re: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            // 4-term Blackman-Harris: sidelobes near -92 dB, so leakage from
            // the harmonics does not hide the folded-back energy.
            let t = std::f64::consts::TAU * i as f64 / n as f64;
            let w = 0.358_75 - 0.488_29 * t.cos() + 0.141_28 * (2.0 * t).cos()
                - 0.011_68 * (3.0 * t).cos();
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
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f64::consts::TAU / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f64).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
    (0..n / 2)
        .map(|k| (re[k] * re[k] + im[k] * im[k]) as f32)
        .collect()
}

#[test]
fn circuit_drive_folds_back_less_than_the_classic_shaper() {
    // A steady, heavily driven and clipped 1.13 kHz tone, whose aliases
    // land between its harmonics. The share of energy away from the
    // harmonics is the folded-back distortion.
    let inharmonic = |circuit: bool| {
        let mut d = drum(KICK, "Punch");
        for (k, v) in [
            ("body.circuit", if circuit { 1.0 } else { 0.0 }),
            ("pitch.freq", 150.0),
            ("pitch.sweep", 0.0),
            ("pitch.snap", 0.0),
            ("body.decay", 3.0),
            ("body.drive", 1.0),
            ("body.clip", 1.0),
            ("body.tone", 16_000.0),
            ("click.level", 0.0),
        ] {
            d.set_param(d.descriptor().param_by_key(k).unwrap().id, v);
        }
        let (l, _) = render(d.as_mut(), &[note_on(0, 95, 1.0, 1)], 24_000, 256);
        let f0 = 150.0 * 2f32.powf(35.0 / 12.0);
        let n = 8_192;
        let spec = spectrum(&l[2_000..2_000 + n]);
        let hz = SR / n as f32;
        let (mut on, mut off) = (0.0f64, 0.0f64);
        for (k, &p) in spec.iter().enumerate().skip(8) {
            let h = k as f32 * hz / f0;
            if (h - h.round()).abs() * f0 < 6.0 * hz {
                on += p as f64;
            } else {
                off += p as f64;
            }
        }
        10.0 * (off / on).log10()
    };
    let (classic, circuit) = (inharmonic(false), inharmonic(true));
    assert!(
        circuit < classic - 6.0,
        "aliasing {circuit:.1} dB vs classic {classic:.1} dB"
    );
}

/// A rock or electronic kit at 140 BPM: kick, snare, closed and open hat,
/// crash and two toms, sixteenths on the hats.
fn kit_events(seconds: f32) -> Vec<Vec<Timed>> {
    let step = (60.0 / 140.0 / 4.0 * SR) as usize;
    let steps = (seconds * SR) as usize / step;
    let mut kit = vec![Vec::new(); 7];
    for s in 0..steps {
        let t = s * step;
        let id = s as u32 + 1;
        if s % 4 == 0 || s % 16 == 10 {
            kit[0].push(note_on(t, 60, 0.95, id));
        }
        if s % 8 == 4 {
            kit[1].push(note_on(t, 60, 0.9, id));
        }
        kit[2].push(note_on(t, 60, if s % 2 == 0 { 0.8 } else { 0.5 }, id));
        if s % 16 == 14 {
            kit[3].push(note_on(t, 60, 0.8, id));
        }
        if s % 32 == 0 {
            kit[4].push(note_on(t, 60, 0.9, id));
        }
        if s % 16 == 12 || s % 16 == 13 {
            kit[5].push(note_on(t, 64, 0.8, id));
            kit[6].push(note_on(t + step / 2, 55, 0.8, id));
        }
    }
    kit
}

#[test]
fn a_full_kit_stays_cheap_and_never_allocates() {
    let kits: [[(&str, &str); 7]; 2] = [
        [
            (KICK, "Metal Kick"),
            (SNARE, "Rock Snare"),
            (HAT, "Acoustic Closed"),
            (HAT, "Acoustic Open"),
            (HAT, "Acoustic Crash"),
            (TOM, "Acoustic Rack"),
            (TOM, "Acoustic Floor"),
        ],
        [
            (KICK, "909 Kick"),
            (SNARE, "909 Snare"),
            (HAT, "808 Closed"),
            (HAT, "808 Open"),
            (HAT, "Acoustic Ride"),
            (TOM, "808 Tom"),
            (TOM, "909 Tom"),
        ],
    ];
    let seconds = 8.0;
    let events = kit_events(seconds);
    for kit in kits {
        let mut devs: Vec<Box<dyn Device>> = kit.iter().map(|(id, n)| drum(id, n)).collect();
        // Build every block's events and buffers before timing or tracking.
        let block = 256;
        let total = (seconds * SR) as usize;
        let blocks: Vec<Vec<Vec<DeviceEvent>>> = events
            .iter()
            .map(|evs| {
                (0..total.div_ceil(block))
                    .map(|b| {
                        evs.iter()
                            .filter(|(t, _)| *t >= b * block && *t < (b + 1) * block)
                            .map(|(t, k)| DeviceEvent {
                                offset: (t - b * block) as u32,
                                kind: *k,
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();
        let mut l = vec![0.0f32; block];
        let mut r = vec![0.0f32; block];
        let mut ctx = ProcessContext::new(SR);
        ctx.playing = true;
        let mut worst = 0.0f32;
        ALLOCS.store(0, Ordering::Relaxed);
        TRACK.with(|t| t.set(true));
        let started = Instant::now();
        for b in 0..total / block {
            for (d, evs) in devs.iter_mut().zip(blocks.iter()) {
                d.process(&ctx, &evs[b], &mut l, &mut r);
                worst = worst.max(peak(&l)).max(peak(&r));
            }
        }
        let elapsed = started.elapsed().as_secs_f32();
        TRACK.with(|t| t.set(false));
        assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "{kit:?} allocated");
        assert!(worst.is_finite() && worst < 2.0, "{kit:?}: peak {worst}");
        // Generous enough for a loaded machine; a real run takes a few
        // percent of real time.
        assert!(
            elapsed < seconds * 0.25,
            "{kit:?}: {elapsed:.2} s for {seconds} s of audio"
        );
    }
}
