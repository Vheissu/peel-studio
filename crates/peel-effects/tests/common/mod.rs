//! Helpers shared by the integration tests.
#![allow(dead_code)]

use peel_dsp::{Device, DeviceEvent, EventKind, Preset, ProcessContext};

pub const SR: f32 = 48_000.0;

/// Deterministic white noise in -1..1.
pub struct Noise(u32);

impl Noise {
    pub fn new(seed: u32) -> Noise {
        Noise(seed.max(1))
    }

    pub fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

/// Sine sample with the phase reduced in f64 (no f32 phase noise).
pub fn sine(freq: f64, i: usize) -> f32 {
    (std::f64::consts::TAU * (i as f64 * freq / f64::from(SR)).fract()).sin() as f32
}

/// A test program with a sine, noise bursts, silence and an impulse, so
/// every kind of effect has something to chew on. Stereo, different content
/// per channel.
pub fn program(frames: usize, seed: u32) -> (Vec<f32>, Vec<f32>) {
    let mut n = Noise::new(seed);
    let mut l = Vec::with_capacity(frames);
    let mut r = Vec::with_capacity(frames);
    for i in 0..frames {
        let seg = (i / 6_000) % 5;
        let (a, b) = match seg {
            0 => (0.5 * sine(440.0, i), 0.4 * sine(660.0, i)),
            1 => (0.3 * n.next(), 0.3 * n.next()),
            2 => (0.0, 0.0),
            3 => {
                let imp = if i % 6_000 == 0 { 0.9 } else { 0.0 };
                (imp, -imp)
            }
            _ => (
                0.35 * sine(3_000.0, i) + 0.1 * n.next(),
                0.35 * sine(2_500.0, i),
            ),
        };
        l.push(a);
        r.push(b);
    }
    (l, r)
}

/// Runs `device` over the program in blocks of `block` frames. `events` are
/// `(absolute frame, event kind)` pairs sorted by frame; they are delivered
/// inside the block that contains them.
pub fn run(
    device: &mut dyn Device,
    l: &mut [f32],
    r: &mut [f32],
    block: usize,
    events: &[(usize, EventKind)],
) {
    let mut ctx = ProcessContext::new(SR);
    let mut start = 0;
    while start < l.len() {
        let end = (start + block).min(l.len());
        let evs: Vec<DeviceEvent> = events
            .iter()
            .filter(|(f, _)| *f >= start && *f < end)
            .map(|(f, k)| DeviceEvent {
                offset: (*f - start) as u32,
                kind: *k,
            })
            .collect();
        ctx.beat_position = start as f64 * 120.0 / 60.0 / f64::from(SR);
        device.process(&ctx, &evs, &mut l[start..end], &mut r[start..end]);
        start = end;
    }
}

/// Creates a prepared device with a preset applied.
pub fn make(
    type_id: &str,
    preset: Option<&Preset>,
    sample_rate: f32,
    block: usize,
) -> Box<dyn Device> {
    let mut d = peel_effects::create(type_id).expect("known type");
    peel_dsp::device::apply_defaults(d.as_mut());
    if let Some(p) = preset {
        let desc = d.descriptor();
        for (key, value) in p.params {
            let param = desc
                .param_by_key(key)
                .unwrap_or_else(|| panic!("{type_id}: preset {} uses unknown key {key}", p.name));
            d.set_param(param.id, *value);
        }
    }
    d.prepare(sample_rate, block);
    d
}

pub fn all_type_ids() -> Vec<&'static str> {
    peel_effects::descriptors()
        .iter()
        .map(|d| d.type_id)
        .collect()
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}
