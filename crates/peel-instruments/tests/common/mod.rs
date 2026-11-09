//! Shared helpers for the instrument integration tests.
#![allow(dead_code)]

use std::sync::Arc;

use peel_dsp::device::apply_defaults;
use peel_dsp::{Device, DeviceEvent, EventKind, ProcessContext, SampleBuffer};

pub const SR: f32 = 48_000.0;

pub const REQUIRED: [&str; 16] = [
    "peel.drum.kick",
    "peel.drum.snare",
    "peel.drum.hat",
    "peel.drum.clap",
    "peel.drum.tom",
    "peel.synth.poly",
    "peel.synth.bass",
    "peel.keys.tine",
    "peel.keys.grand",
    "peel.guitar.electric",
    "peel.sampler",
    "peel.wind.sax",
    "peel.strings.violin",
    "peel.perc.mallet",
    "peel.strings.zither",
    "peel.synth.cross",
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

/// Estimates the fundamental frequency of `buf` from its autocorrelation,
/// preferring the shortest lag that is nearly as strong as the best one.
pub fn detect_pitch(buf: &[f32], sample_rate: f32) -> f32 {
    let n = buf.len().min(8_192);
    let min_lag = (sample_rate / 2_500.0) as usize;
    let max_lag = ((sample_rate / 40.0) as usize).min(n / 2);
    let energy: f32 = buf[..n].iter().map(|x| x * x).sum();
    if energy <= 1e-12 || max_lag <= min_lag + 2 {
        return 0.0;
    }
    let mut nac = vec![0.0f32; max_lag + 2];
    for (lag, slot) in nac.iter_mut().enumerate().take(max_lag + 1).skip(min_lag) {
        let mut sum = 0.0;
        for i in 0..n - lag {
            sum += buf[i] * buf[i + lag];
        }
        *slot = sum / energy;
    }
    // Skip the broad peak around zero lag: start once the correlation has
    // gone negative.
    let Some(start) = (min_lag..=max_lag).find(|&l| nac[l] < 0.0) else {
        return 0.0;
    };
    let best = nac[start..=max_lag].iter().cloned().fold(0.0f32, f32::max);
    for lag in start + 1..max_lag {
        if nac[lag] >= 0.85 * best && nac[lag] >= nac[lag - 1] && nac[lag] >= nac[lag + 1] {
            // Parabolic refinement around the peak.
            let (a, b, c) = (nac[lag - 1], nac[lag], nac[lag + 1]);
            let denom = a - 2.0 * b + c;
            let shift = if denom.abs() > 1e-9 {
                0.5 * (a - c) / denom
            } else {
                0.0
            };
            return sample_rate / (lag as f32 + shift);
        }
    }
    0.0
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
