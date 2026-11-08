//! Shared helpers for the instrument integration tests.
#![allow(dead_code)]

use std::sync::Arc;

use peel_dsp::device::apply_defaults;
use peel_dsp::{Device, DeviceEvent, EventKind, ProcessContext, SampleBuffer};

pub const SR: f32 = 48_000.0;

pub const REQUIRED: [&str; 10] = [
    "peel.drum.kick",
    "peel.drum.snare",
    "peel.drum.hat",
    "peel.drum.clap",
    "peel.drum.tom",
    "peel.synth.poly",
    "peel.synth.bass",
    "peel.keys.tine",
    "peel.keys.grand",
    "peel.sampler",
];

/// An event placed at an absolute sample position.
pub type Timed = (usize, EventKind);

pub fn note_on(at: usize, key: u8, velocity: f32, voice: u32) -> Timed {
    (
        at,
        EventKind::NoteOn {
            key,
            velocity,
            voice,
        },
    )
}

pub fn note_off(at: usize, key: u8, voice: u32) -> Timed {
    (
        at,
        EventKind::NoteOff {
            key,
            velocity: 0.0,
            voice,
        },
    )
}

/// A one-second stereo test tone for the sampler: a 220 Hz sine recorded at
/// 44.1 kHz with root key 57 (A3).
pub fn test_sample() -> Arc<SampleBuffer> {
    let sr = 44_100.0f32;
    let n = 44_100;
    let left: Vec<f32> = (0..n)
        .map(|i| 0.8 * (std::f32::consts::TAU * 220.0 * i as f32 / sr).sin())
        .collect();
    let right: Vec<f32> = left.iter().map(|v| v * 0.5).collect();
    let mut s = SampleBuffer::new("test", sr, vec![left, right]);
    s.root_key = 57;
    Arc::new(s)
}

/// Creates, prepares and applies defaults. Samplers get a test sample.
pub fn fresh(type_id: &str) -> Box<dyn Device> {
    let mut d = peel_instruments::create(type_id).expect("known type");
    d.prepare(SR, 256);
    apply_defaults(d.as_mut());
    if type_id == "peel.sampler" {
        let prev = d.set_sample(Some(test_sample()));
        assert!(prev.is_none());
    }
    d
}

/// Renders `total` samples in blocks of `block`, delivering `events` at their
/// absolute positions. Buffers are pre-filled with garbage to prove the
/// instrument overwrites them.
pub fn render(
    dev: &mut dyn Device,
    events: &[Timed],
    total: usize,
    block: usize,
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
        let mut ctx = ProcessContext::new(SR);
        ctx.playing = true;
        ctx.tempo_bpm = 120.0;
        ctx.beat_position = pos as f64 * 120.0 / 60.0 / SR as f64;
        let mut l = vec![7.0f32; n];
        let mut r = vec![7.0f32; n];
        dev.process(&ctx, &evs, &mut l, &mut r);
        out_l.extend_from_slice(&l);
        out_r.extend_from_slice(&r);
        pos += n;
    }
    (out_l, out_r)
}

pub fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0, |m, &x| m.max(x.abs()))
}

pub fn all_finite(buf: &[f32]) -> bool {
    buf.iter().all(|x| x.is_finite())
}

pub fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).fold(0.0, |m, (x, y)| m.max((x - y).abs()))
}

/// Estimates the fundamental of `buf` by counting rising zero crossings.
pub fn crossing_freq(buf: &[f32], sample_rate: f32) -> f32 {
    let mut first = None;
    let mut last = 0;
    let mut count = 0usize;
    for i in 1..buf.len() {
        if buf[i - 1] < 0.0 && buf[i] >= 0.0 {
            if first.is_none() {
                first = Some(i);
            } else {
                count += 1;
            }
            last = i;
        }
    }
    match first {
        Some(f) if count > 0 => count as f32 * sample_rate / (last - f) as f32,
        _ => 0.0,
    }
}

/// Deterministic pseudo-random generator for property-style tests.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}
