//! Three-band upward and downward compressor, the "squash and sparkle"
//! processor that modern EDM masters, buses and drops lean on.
//!
//! Signal path: input gain, a pair of Linkwitz-Riley (4th order) crossovers
//! that split the signal into low, mid and high bands and sum back to a flat
//! magnitude (the high band is run through the low crossover's all-pass
//! complement so the phases line up), then in every band a stereo-linked level
//! detector and a gain computer with two sides:
//!
//! * **Downward** compression above the upper threshold turns loud peaks
//!   down.
//! * **Upward** compression below the lower threshold lifts quiet material
//!   (decays, room, hiss between notes) toward it, with a floor that stops it
//!   from amplifying digital silence.
//!
//! Both sides use a soft knee and a 4:1 slope at full **Depth**; Depth scales
//! the resulting gain change in decibels, so 0% is transparent. **Upward** and
//! **Downward** scale each side separately. **Time** moves attack and release
//! together from fast and aggressive (0%) to slow and smooth (100%); each band
//! has its own base times (slow lows, quick highs). The gain is smoothed in the
//! log domain: attack while the gain falls, release while it rises.
//!
//! The audio runs 1 ms behind the detectors (reported as latency), so the
//! gain is already moving when a transient arrives and boosted quiet material
//! does not let peaks through. Each band has a trim after the compressors, **Mix** blends the processed
//! sum with the unprocessed (phase-matched) sum, and **Output** sets the final
//! level.

use peel_dsp::biquad::{Coeffs, FilterKind, StereoBiquad};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::dynamics::time_coeff;
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_INPUT: ParamId = 0;
pub const ID_DEPTH: ParamId = 1;
pub const ID_TIME: ParamId = 2;
pub const ID_OUTPUT: ParamId = 3;
pub const ID_XOVER_LOW: ParamId = 4;
pub const ID_XOVER_HIGH: ParamId = 5;
pub const ID_UPWARD: ParamId = 6;
pub const ID_DOWNWARD: ParamId = 7;
pub const ID_GAIN_LOW: ParamId = 8;
pub const ID_GAIN_MID: ParamId = 9;
pub const ID_GAIN_HIGH: ParamId = 10;
pub const ID_MIX: ParamId = 11;

static PARAMS: [ParamDescriptor; 12] = [
    ParamDescriptor::linear(
        ID_INPUT,
        "input",
        "Input",
        "Level",
        -24.0,
        24.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_OUTPUT,
        "output",
        "Output",
        "Level",
        -24.0,
        24.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_DEPTH,
        "depth",
        "Depth",
        "Compression",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_TIME,
        "time",
        "Time",
        "Compression",
        0.0,
        1.0,
        0.6,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_UPWARD,
        "upward",
        "Upward",
        "Compression",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_DOWNWARD,
        "downward",
        "Downward",
        "Compression",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_XOVER_LOW,
        "xover.low",
        "Low / Mid",
        "Crossover",
        40.0,
        400.0,
        120.0,
        Unit::Hertz,
    ),
    ParamDescriptor::log(
        ID_XOVER_HIGH,
        "xover.high",
        "Mid / High",
        "Crossover",
        800.0,
        10_000.0,
        2_500.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_GAIN_LOW,
        "gain.low",
        "Low Gain",
        "Bands",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_GAIN_MID,
        "gain.mid",
        "Mid Gain",
        "Bands",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_GAIN_HIGH,
        "gain.high",
        "High Gain",
        "Bands",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 1.0, Unit::Percent),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.multiband",
    name: "Multiband",
    description: "Three-band upward and downward compressor with flat-summing crossovers",
    category: "Dynamics",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 4] = [
    Preset {
        name: "Gentle",
        params: &[],
    },
    Preset {
        name: "Heavy",
        params: &[
            ("depth", 1.0),
            ("time", 0.25),
            ("upward", 1.0),
            ("downward", 1.0),
            ("gain.high", 1.0),
            ("output", 2.5),
        ],
    },
    Preset {
        name: "Bus",
        params: &[
            ("depth", 0.3),
            ("time", 0.75),
            ("upward", 0.5),
            ("downward", 1.0),
            ("xover.low", 100.0),
            ("xover.high", 3_000.0),
        ],
    },
    Preset {
        name: "Master",
        params: &[
            ("depth", 0.2),
            ("time", 0.85),
            ("upward", 0.45),
            ("downward", 0.8),
            ("xover.low", 90.0),
            ("xover.high", 4_000.0),
            ("gain.high", 0.5),
        ],
    },
];

/// Downward threshold: above this band level (RMS dBFS) the band is turned
/// down.
const THRESH_DOWN: f32 = -14.0;
/// Upward threshold: below this band level the band is lifted.
const THRESH_UP: f32 = -30.0;
/// Slope of both sides at full depth.
const RATIO: f32 = 4.0;
/// Soft knee width in dB.
const KNEE: f32 = 8.0;
/// Largest lift in dB before depth scaling.
const MAX_BOOST: f32 = 24.0;
/// Below this level (dBFS) the upward side fades out completely, so noise
/// floors and digital silence are not amplified; it is fully active above
/// `FLOOR_ON`.
const FLOOR_OFF: f32 = -85.0;
const FLOOR_ON: f32 = -70.0;

/// Look-ahead in seconds.
const LOOKAHEAD: f32 = 0.001;

/// Base attack and release in seconds at Time = 50%, and the detector
/// release for the low, mid and high band.
const ATTACK: [f32; 3] = [0.008, 0.0035, 0.0008];
const RELEASE: [f32; 3] = [0.300, 0.160, 0.070];
const DETECTOR: [f32; 3] = [0.012, 0.005, 0.002];
/// Detector attack in seconds (the rise of the power average).
const DETECTOR_ATTACK: f32 = 0.0004;

/// Gain in dB the compressor applies for a band `level` in dB at full
/// depth: negative above the downward threshold, positive below the upward
/// one, scaled by the two side amounts.
fn gain_curve_db(level: f32, up: f32, down: f32) -> f32 {
    let slope = 1.0 - 1.0 / RATIO;
    // Downward: soft knee centred on the threshold.
    let d = level - THRESH_DOWN;
    let cut = if 2.0 * d <= -KNEE {
        0.0
    } else if 2.0 * d < KNEE {
        slope * (d + 0.5 * KNEE).powi(2) / (2.0 * KNEE)
    } else {
        slope * d
    };
    // Upward: the mirror image around the lower threshold.
    let u = THRESH_UP - level;
    let lift = if 2.0 * u <= -KNEE {
        0.0
    } else if 2.0 * u < KNEE {
        slope * (u + 0.5 * KNEE).powi(2) / (2.0 * KNEE)
    } else {
        slope * u
    };
    let floor = ((level - FLOOR_OFF) / (FLOOR_ON - FLOOR_OFF)).clamp(0.0, 1.0);
    let lift = lift.min(MAX_BOOST) * floor * floor * (3.0 - 2.0 * floor);
    up * lift - down * cut
}

/// One band's detector and gain smoother.
#[derive(Clone, Copy, Debug)]
struct Band {
    power: f32,
    det_attack: f32,
    det_release: f32,
    gain_db: f32,
    attack: f32,
    release: f32,
}

impl Band {
    fn new() -> Band {
        Band {
            power: 0.0,
            det_attack: 0.9,
            det_release: 0.99,
            gain_db: 0.0,
            attack: 0.9,
            release: 0.99,
        }
    }

    fn reset(&mut self) {
        self.power = 0.0;
        self.gain_db = 0.0;
    }

    /// Feeds a stereo frame of this band, returns the gain to apply in dB.
    #[inline]
    fn process(&mut self, l: f32, r: f32, depth: f32, up: f32, down: f32) -> f32 {
        let p = 0.5 * (l * l + r * r);
        let c = if p > self.power {
            self.det_attack
        } else {
            self.det_release
        };
        self.power = p + c * (self.power - p);
        if self.power < 1e-30 {
            self.power = 0.0;
        }
        let level = 10.0 * (self.power + 1e-12).log10();
        let target = depth * gain_curve_db(level, up, down);
        let coeff = if target < self.gain_db {
            self.attack
        } else {
            self.release
        };
        self.gain_db = target + coeff * (self.gain_db - target);
        self.gain_db
    }
}

/// Short stereo delay for one band, so the audio lags its detector.
#[derive(Clone, Debug, Default)]
struct Lookahead {
    buf: Vec<(f32, f32)>,
    pos: usize,
}

impl Lookahead {
    fn resize(&mut self, len: usize) {
        self.buf = vec![(0.0, 0.0); len.max(1)];
        self.pos = 0;
    }

    fn clear(&mut self) {
        self.buf.fill((0.0, 0.0));
        self.pos = 0;
    }

    /// Pushes a frame and returns the one pushed `len` frames ago.
    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let out = self.buf[self.pos];
        self.buf[self.pos] = (l, r);
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        out
    }
}

/// A Linkwitz-Riley 4th-order section: two identical Butterworth biquads.
#[derive(Clone, Copy, Debug, Default)]
struct Lr4([StereoBiquad; 2]);

impl Lr4 {
    fn set_ramp(&mut self, sample_rate: f32, seconds: f32) {
        for b in &mut self.0 {
            b.set_ramp_time(sample_rate, seconds);
        }
    }

    fn set_target(&mut self, c: Coeffs) {
        for b in &mut self.0 {
            b.set_target(c);
        }
    }

    fn snap(&mut self) {
        for b in &mut self.0 {
            let t = b.target();
            b.set_immediate(t);
        }
    }

    fn reset(&mut self) {
        for b in &mut self.0 {
            b.reset();
        }
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (l, r) = self.0[0].process(l, r);
        self.0[1].process(l, r)
    }
}

/// Multiband upward/downward compressor. See the module docs.
pub struct Multiband {
    store: ParamStore,
    sample_rate: f32,
    // High crossover (splits off the high band), then the low crossover on
    // the remainder, and the low crossover's complement for the high band.
    lp_high: Lr4,
    hp_high: Lr4,
    lp_low: Lr4,
    hp_low: Lr4,
    ap_lp: Lr4,
    ap_hp: Lr4,
    bands: [Band; 3],
    delays: [Lookahead; 3],
    lookahead: u32,
    trims: [Smoothed; 3],
    input: Smoothed,
    output: Smoothed,
    mix: Smoothed,
    depth: f32,
    up: f32,
    down: f32,
    silence: SilenceTracker,
}

impl Default for Multiband {
    fn default() -> Self {
        Multiband::new()
    }
}

impl Multiband {
    pub fn new() -> Multiband {
        let mut m = Multiband {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            lp_high: Lr4::default(),
            hp_high: Lr4::default(),
            lp_low: Lr4::default(),
            hp_low: Lr4::default(),
            ap_lp: Lr4::default(),
            ap_hp: Lr4::default(),
            bands: [Band::new(); 3],
            delays: Default::default(),
            lookahead: 0,
            trims: [Smoothed::new(1.0); 3],
            input: Smoothed::new(1.0),
            output: Smoothed::new(1.0),
            mix: Smoothed::new(1.0),
            depth: 0.35,
            up: 0.7,
            down: 1.0,
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut m, 48_000.0, 512);
        m
    }

    /// Current gain of a band (0 low, 1 mid, 2 high) in dB.
    pub fn band_gain_db(&self, band: usize) -> f32 {
        self.bands.get(band).map_or(0.0, |b| b.gain_db)
    }

    fn update_crossovers(&mut self) {
        let sr = self.sample_rate;
        let low = self.store.get(ID_XOVER_LOW);
        let high = self.store.get(ID_XOVER_HIGH).max(low * 1.5).min(sr * 0.45);
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let design = |kind, f| Coeffs::design(kind, sr, f, q, 0.0);
        self.lp_high.set_target(design(FilterKind::LowPass, high));
        self.hp_high.set_target(design(FilterKind::HighPass, high));
        self.lp_low.set_target(design(FilterKind::LowPass, low));
        self.hp_low.set_target(design(FilterKind::HighPass, low));
        self.ap_lp.set_target(design(FilterKind::LowPass, low));
        self.ap_hp.set_target(design(FilterKind::HighPass, low));
    }

    fn update_times(&mut self) {
        let sr = self.sample_rate;
        // Time 50% keeps the base times; the ends scale them by 4x.
        let scale = 4.0f32.powf(2.0 * self.store.get(ID_TIME) - 1.0);
        for (i, b) in self.bands.iter_mut().enumerate() {
            b.attack = time_coeff(ATTACK[i] * scale, sr);
            b.release = time_coeff(RELEASE[i] * scale, sr);
            b.det_attack = time_coeff(DETECTOR_ATTACK, sr);
            b.det_release = time_coeff(DETECTOR[i], sr);
        }
    }
}

impl Fx for Multiband {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn reduction(&self) -> Option<f32> {
        let lowest = self.bands.iter().map(|b| b.gain_db).fold(0.0f32, f32::min);
        Some(-lowest)
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        self.lookahead = (LOOKAHEAD * sr).round().max(1.0) as u32;
        for d in &mut self.delays {
            d.resize(self.lookahead as usize);
        }
        for s in [
            &mut self.lp_high,
            &mut self.hp_high,
            &mut self.lp_low,
            &mut self.hp_low,
            &mut self.ap_lp,
            &mut self.ap_hp,
        ] {
            s.set_ramp(sr, 0.02);
        }
        for t in &mut self.trims {
            t.set_ramp(sr, RAMP_SECONDS);
        }
        self.input.set_ramp(sr, RAMP_SECONDS);
        self.output.set_ramp(sr, RAMP_SECONDS);
        self.mix.set_ramp(sr, RAMP_SECONDS);
        refresh_all(self);
        for s in [
            &mut self.lp_high,
            &mut self.hp_high,
            &mut self.lp_low,
            &mut self.hp_low,
            &mut self.ap_lp,
            &mut self.ap_hp,
        ] {
            s.snap();
        }
        for s in self
            .trims
            .iter_mut()
            .chain([&mut self.input, &mut self.output, &mut self.mix])
        {
            let t = s.target();
            s.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_INPUT => self.input.set_target(db_to_gain(value)),
            ID_OUTPUT => self.output.set_target(db_to_gain(value)),
            ID_DEPTH => self.depth = value,
            ID_UPWARD => self.up = value,
            ID_DOWNWARD => self.down = value,
            ID_TIME => self.update_times(),
            ID_XOVER_LOW | ID_XOVER_HIGH => self.update_crossovers(),
            ID_GAIN_LOW => self.trims[0].set_target(db_to_gain(value)),
            ID_GAIN_MID => self.trims[1].set_target(db_to_gain(value)),
            ID_GAIN_HIGH => self.trims[2].set_target(db_to_gain(value)),
            ID_MIX => self.mix.set_target(value),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for s in [
            &mut self.lp_high,
            &mut self.hp_high,
            &mut self.lp_low,
            &mut self.hp_low,
            &mut self.ap_lp,
            &mut self.ap_hp,
        ] {
            s.reset();
        }
        for b in &mut self.bands {
            b.reset();
        }
        for d in &mut self.delays {
            d.clear();
        }
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g_in = self.input.next();
            let (xl, xr) = (*l * g_in, *r * g_in);

            // Split. The high band keeps its phase in step with the low
            // crossover by passing through that crossover's low + high sum.
            let (ml, mr) = self.lp_high.process(xl, xr);
            let (hl, hr) = self.hp_high.process(xl, xr);
            let (low_l, low_r) = self.lp_low.process(ml, mr);
            let (mid_l, mid_r) = self.hp_low.process(ml, mr);
            let (al, ar) = self.ap_lp.process(hl, hr);
            let (bl, br) = self.ap_hp.process(hl, hr);
            let (high_l, high_r) = (al + bl, ar + br);

            let bands = [(low_l, low_r), (mid_l, mid_r), (high_l, high_r)];
            let (mut wl, mut wr) = (0.0f32, 0.0f32);
            let (mut dry_l, mut dry_r) = (0.0f32, 0.0f32);
            for (i, &(bl, br)) in bands.iter().enumerate() {
                let gain_db = self.bands[i].process(bl, br, self.depth, self.up, self.down);
                let g = db_to_gain(gain_db) * self.trims[i].next();
                let (dl, dr) = self.delays[i].process(bl, br);
                wl += dl * g;
                wr += dr * g;
                dry_l += dl;
                dry_r += dr;
            }

            let mix = self.mix.next();
            let g_out = self.output.next();
            *l = (dry_l + (wl - dry_l) * mix) * g_out;
            *r = (dry_r + (wr - dry_r) * mix) * g_out;
        }
    }

    fn latency_samples(&self) -> u32 {
        self.lookahead
    }

    fn tail(&self) -> Option<f32> {
        Some(LOOKAHEAD)
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for((self.sample_rate * 0.2) as u32)
    }
}

impl_device!(Multiband, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    fn sine(amp: f32, freq: f32, start: usize, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                amp * (std::f64::consts::TAU * f64::from(freq) * (start + i) as f64 / f64::from(SR))
                    .sin() as f32
            })
            .collect()
    }

    /// Steady gain in dB of a sine through `mb`, from the RMS of the last
    /// part of the run (peak sampling would misread high frequencies).
    fn through(mb: &mut Multiband, amp_db: f32, freq: f32) -> f32 {
        let amp = db_to_gain(amp_db);
        let ctx = ProcessContext::new(SR);
        let (mut sum, mut count) = (0.0f64, 0usize);
        for block in 0..375 {
            let mut l = sine(amp, freq, block * 256, 256);
            let mut r = l.clone();
            mb.process(&ctx, &[], &mut l, &mut r);
            if block > 300 {
                sum += l.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
                count += l.len();
            }
        }
        let rms = (sum / count as f64).sqrt() as f32;
        20.0 * (rms * std::f32::consts::SQRT_2).log10() - amp_db
    }

    #[test]
    fn crossovers_sum_flat_at_zero_depth() {
        for (lo, hi) in [(120.0, 2_500.0), (60.0, 800.0), (350.0, 9_000.0)] {
            let mut mb = Multiband::new();
            mb.set_param(ID_DEPTH, 0.0);
            mb.set_param(ID_XOVER_LOW, lo);
            mb.set_param(ID_XOVER_HIGH, hi);
            mb.prepare(SR, 256);
            for f in [30.0, 80.0, lo, 200.0, 700.0, 1_800.0, hi, 5_000.0, 12_000.0] {
                let g = through(&mut mb, -12.0, f);
                assert!(g.abs() < 0.1, "xover {lo}/{hi}: {g} dB at {f} Hz");
            }
        }
    }

    #[test]
    fn upward_lifts_quiet_and_downward_tames_loud() {
        let mut mb = Multiband::new();
        mb.set_param(ID_DEPTH, 1.0);
        mb.set_param(ID_UPWARD, 1.0);
        mb.set_param(ID_TIME, 0.2);
        mb.prepare(SR, 256);
        let quiet = through(&mut mb, -52.0, 1_000.0);
        assert!(quiet > 8.0, "quiet material moved {quiet} dB");
        let mut mb2 = Multiband::new();
        mb2.set_param(ID_DEPTH, 1.0);
        mb2.set_param(ID_TIME, 0.2);
        mb2.prepare(SR, 256);
        let loud = through(&mut mb2, -3.0, 1_000.0);
        assert!(loud < -5.0, "loud material moved {loud} dB");
        // The gap between the two narrows by a good margin.
        assert!((-3.0 + loud) - (-52.0 + quiet) < 40.0);
    }

    #[test]
    fn digital_silence_and_noise_floor_are_not_amplified() {
        let mut mb = Multiband::new();
        mb.set_param(ID_DEPTH, 1.0);
        mb.set_param(ID_UPWARD, 1.0);
        mb.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        let mut peak = 0.0f32;
        for _ in 0..200 {
            let mut l = vec![1e-6f32; 256];
            let mut r = vec![1e-6f32; 256];
            mb.process(&ctx, &[], &mut l, &mut r);
            peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
        }
        // -120 dBFS stays far below -90 dBFS.
        assert!(peak < 3e-5, "{peak}");
    }

    #[test]
    fn bands_compress_independently() {
        // A loud bass tone must not turn down a quiet high tone.
        let mut mb = Multiband::new();
        mb.set_param(ID_DEPTH, 1.0);
        mb.set_param(ID_UPWARD, 0.0);
        mb.set_param(ID_TIME, 0.2);
        mb.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        for block in 0..375 {
            let bass = sine(0.7, 60.0, block * 256, 256);
            let hi = sine(0.01, 6_000.0, block * 256, 256);
            let mut l: Vec<f32> = bass.iter().zip(&hi).map(|(a, b)| a + b).collect();
            let mut r = l.clone();
            mb.process(&ctx, &[], &mut l, &mut r);
        }
        assert!(
            mb.band_gain_db(0) < -6.0,
            "bass band {}",
            mb.band_gain_db(0)
        );
        assert!(
            mb.band_gain_db(2).abs() < 0.2,
            "high band {}",
            mb.band_gain_db(2)
        );
    }

    #[test]
    fn mix_zero_is_the_flat_dry_sum() {
        let mut mb = Multiband::new();
        mb.set_param(ID_DEPTH, 1.0);
        mb.set_param(ID_MIX, 0.0);
        mb.prepare(SR, 256);
        for f in [50.0, 500.0, 5_000.0] {
            let g = through(&mut mb, -20.0, f);
            assert!(g.abs() < 0.1, "{g} dB at {f}");
        }
    }

    #[test]
    fn crossover_changes_stay_stable() {
        let mut mb = Multiband::new();
        mb.set_param(ID_DEPTH, 0.6);
        mb.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        for block in 0..400 {
            if block % 20 == 0 {
                let k = (block / 20) % 2;
                mb.set_param(ID_XOVER_LOW, if k == 0 { 60.0 } else { 380.0 });
                mb.set_param(ID_XOVER_HIGH, if k == 0 { 9_000.0 } else { 900.0 });
            }
            let mut l = sine(0.5, 220.0, block * 256, 256);
            let mut r = sine(0.5, 3_000.0, block * 256, 256);
            mb.process(&ctx, &[], &mut l, &mut r);
            assert!(
                l.iter()
                    .chain(r.iter())
                    .all(|v| v.is_finite() && v.abs() < 4.0)
            );
        }
    }
}
