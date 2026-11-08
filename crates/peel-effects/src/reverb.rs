//! Algorithmic reverb built on an eight-line feedback delay network.
//!
//! Signal path: stereo pre-delay, two chains of four Schroeder all-pass
//! diffusers (different lengths for left and right), an eight-line FDN with
//! Hadamard mixing, modulated delay lines and in-loop damping, a stereo
//! output tap matrix, low and high cuts on the wet signal, mid/side width,
//! and a linear dry/wet mix.
//!
//! * **Size** scales all line lengths from 0.25x to 2x (an exponential map),
//!   so small values are rooms and large values are halls. Changes glide
//!   rather than click.
//! * **Decay** is the time the tail takes to fall by 60 dB, set per line so
//!   it is independent of size.
//! * **Damping** lowers the in-loop low-pass cutoff from 20 kHz to about
//!   1.2 kHz, so highs die faster than lows like in real rooms.
//! * **Diffusion** is the all-pass gain: 0 keeps early echoes distinct,
//!   100% smears them into a smooth wash.
//! * **Early** sets the level of six discrete early reflections per side,
//!   read from the pre-delay line with times that scale with size. They fill
//!   the gap before the FDN's shortest line speaks.
//! * **Modulation** wobbles the line lengths a few samples at unrelated slow
//!   rates, which breaks up metallic ringing.

use peel_dsp::biquad::{OnePoleHighpass, OnePoleLowpass};
use peel_dsp::delay::{DelayLine, TimeGlide};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::reverb::{Allpass, Fdn8, Mixing};
use peel_dsp::util::Smoothed;

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_SIZE: ParamId = 0;
pub const ID_DECAY: ParamId = 1;
pub const ID_PREDELAY: ParamId = 2;
pub const ID_DAMPING: ParamId = 3;
pub const ID_DIFFUSION: ParamId = 4;
pub const ID_MODULATION: ParamId = 5;
pub const ID_WIDTH: ParamId = 6;
pub const ID_LOW_CUT: ParamId = 7;
pub const ID_HIGH_CUT: ParamId = 8;
pub const ID_MIX: ParamId = 9;
pub const ID_EARLY: ParamId = 10;

static PARAMS: [ParamDescriptor; 11] = [
    ParamDescriptor::linear(
        ID_SIZE,
        "size",
        "Size",
        "Space",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_DECAY,
        "decay",
        "Decay",
        "Space",
        0.2,
        30.0,
        2.5,
        Unit::Seconds,
    ),
    ParamDescriptor::power(
        ID_PREDELAY,
        "predelay",
        "Pre-delay",
        "Space",
        0.0,
        250.0,
        12.0,
        Unit::Milliseconds,
        2.0,
    ),
    ParamDescriptor::linear(
        ID_DAMPING,
        "damping",
        "Damping",
        "Tone",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_LOW_CUT,
        "lowcut",
        "Low Cut",
        "Tone",
        20.0,
        1_000.0,
        20.0,
        Unit::Hertz,
    ),
    ParamDescriptor::log(
        ID_HIGH_CUT,
        "highcut",
        "High Cut",
        "Tone",
        1_000.0,
        20_000.0,
        20_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_DIFFUSION,
        "diffusion",
        "Diffusion",
        "Character",
        0.0,
        1.0,
        0.75,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_EARLY,
        "early",
        "Early",
        "Character",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_MODULATION,
        "modulation",
        "Modulation",
        "Character",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_WIDTH,
        "width",
        "Width",
        "Output",
        0.0,
        1.5,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_MIX,
        "mix",
        "Mix",
        "Output",
        0.0,
        1.0,
        0.25,
        Unit::Percent,
    ),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.reverb",
    name: "Reverb",
    description: "Lush algorithmic reverb with modulated feedback delay network",
    category: "Reverb",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Room",
        params: &[],
    },
    Preset {
        name: "Small Plate",
        params: &[
            ("size", 0.3),
            ("decay", 1.6),
            ("predelay", 4.0),
            ("damping", 0.25),
            ("diffusion", 0.95),
            ("modulation", 0.45),
            ("lowcut", 120.0),
            ("mix", 0.22),
        ],
    },
    Preset {
        name: "Warm Hall",
        params: &[
            ("size", 0.72),
            ("decay", 3.8),
            ("predelay", 24.0),
            ("damping", 0.6),
            ("diffusion", 0.8),
            ("modulation", 0.3),
            ("lowcut", 80.0),
            ("mix", 0.26),
        ],
    },
    Preset {
        name: "Big Cathedral",
        params: &[
            ("size", 0.95),
            ("decay", 9.0),
            ("predelay", 38.0),
            ("damping", 0.55),
            ("diffusion", 0.85),
            ("modulation", 0.35),
            ("lowcut", 140.0),
            ("highcut", 9_000.0),
            ("mix", 0.3),
        ],
    },
    Preset {
        name: "Ambient Wash",
        params: &[
            ("size", 0.85),
            ("decay", 14.0),
            ("predelay", 60.0),
            ("damping", 0.4),
            ("diffusion", 1.0),
            ("modulation", 0.7),
            ("lowcut", 200.0),
            ("highcut", 12_000.0),
            ("width", 1.3),
            ("mix", 0.4),
        ],
    },
    Preset {
        name: "Tight Ambience",
        params: &[
            ("size", 0.12),
            ("decay", 0.7),
            ("predelay", 0.0),
            ("damping", 0.5),
            ("diffusion", 0.6),
            ("modulation", 0.15),
            ("mix", 0.18),
        ],
    },
];

/// Base line lengths in milliseconds at size scale 1.0. Spread widely and
/// not harmonically related, so the modes do not stack.
const LINE_MS: [f32; 8] = [31.0, 37.7, 43.9, 51.1, 59.3, 67.7, 77.9, 89.3];
/// Left and right input diffuser lengths in milliseconds.
const DIFFUSER_MS: [[f32; 4]; 2] = [[4.77, 7.31, 11.13, 16.07], [5.21, 8.17, 12.39, 17.61]];

/// Early reflection tap times in milliseconds (before size scaling) and
/// their gains, left then right. Irregular on purpose.
const ER_MS: [[f32; 6]; 2] = [
    [7.3, 12.9, 19.7, 27.1, 36.3, 46.9],
    [9.1, 15.3, 22.3, 30.7, 40.1, 52.3],
];
const ER_GAIN: [[f32; 6]; 2] = [
    [0.9, -0.75, 0.6, 0.5, -0.4, 0.3],
    [0.85, 0.7, -0.6, 0.45, -0.4, -0.3],
];

/// FDN injection signs. Even lines take the left diffuser, odd lines the
/// right one. The patterns are not Hadamard rows, so the first mixing pass
/// spreads the input over all lines instead of concentrating it.
const INJECT: [f32; 8] = [1.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
/// Output tap signs: two orthogonal Hadamard rows decorrelate left and right.
const TAP_L: [f32; 8] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
const TAP_R: [f32; 8] = [1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0];

/// 1/sqrt(8): keeps the tap sum at unity RMS for uncorrelated lines.
const TAP_NORM: f32 = 0.353_553_4;

/// Reverb effect. See the module docs.
pub struct Reverb {
    store: ParamStore,
    sample_rate: f32,
    pre: [DelayLine; 2],
    pre_glide: TimeGlide,
    diffusers: [[Allpass; 4]; 2],
    fdn: Fdn8,
    low_cut: [OnePoleHighpass; 2],
    high_cut: [OnePoleLowpass; 2],
    low_cut_on: bool,
    high_cut_on: bool,
    width: Smoothed,
    mix: Smoothed,
    early: Smoothed,
    er_scale: Smoothed,
    silence: SilenceTracker,
}

impl Default for Reverb {
    fn default() -> Self {
        Reverb::new()
    }
}

fn make_diffusers(sample_rate: f32) -> [[Allpass; 4]; 2] {
    std::array::from_fn(|ch| {
        std::array::from_fn(|k| {
            Allpass::new(
                (DIFFUSER_MS[ch][k] * 0.001 * sample_rate).round().max(2.0) as usize,
                0.5,
            )
        })
    })
}

impl Reverb {
    pub fn new() -> Reverb {
        let mut r = Reverb {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            pre: [DelayLine::new(16), DelayLine::new(16)],
            pre_glide: TimeGlide::new(2.0),
            diffusers: make_diffusers(48_000.0),
            fdn: Fdn8::new(48_000.0, 64),
            low_cut: [OnePoleHighpass::default(); 2],
            high_cut: [OnePoleLowpass::default(); 2],
            low_cut_on: false,
            high_cut_on: false,
            width: Smoothed::new(1.0),
            mix: Smoothed::new(0.25),
            early: Smoothed::new(0.5),
            er_scale: Smoothed::new(0.7),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut r, 48_000.0, 512);
        r
    }

    fn update_size(&mut self) {
        let size = self.store.get(ID_SIZE);
        let scale = 0.25 * 8f32.powf(size);
        self.er_scale.set_target(scale);
        let lens = LINE_MS.map(|ms| ms * 0.001 * self.sample_rate * scale);
        self.fdn.set_lengths(lens, false);
    }

    fn pre_samples(&self) -> f32 {
        (self.store.get(ID_PREDELAY) * 0.001 * self.sample_rate).max(2.0)
    }
}

impl Fx for Reverb {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        let pre_max = (0.26 * sr + 0.12 * sr) as usize + 8;
        self.pre = [DelayLine::new(pre_max), DelayLine::new(pre_max)];
        self.pre_glide.configure(sr, 0.08, 0.4);
        self.diffusers = make_diffusers(sr);
        // Longest line at the largest size, plus modulation headroom.
        let max_line = (LINE_MS[7] * 0.001 * sr * 2.0) as usize + 64;
        self.fdn = Fdn8::new(sr, max_line);
        self.fdn.set_mixing(Mixing::Hadamard);
        self.width.set_ramp(sr, RAMP_SECONDS);
        self.mix.set_ramp(sr, RAMP_SECONDS);
        self.early.set_ramp(sr, RAMP_SECONDS);
        self.er_scale.set_ramp(sr, 0.1);
        refresh_all(self);
        let lens = self.fdn.len_target();
        self.fdn.set_lengths(lens, true);
        let p = self.pre_samples();
        self.pre_glide.set_immediate(p);
        for s in [
            &mut self.width,
            &mut self.mix,
            &mut self.early,
            &mut self.er_scale,
        ] {
            let t = s.target();
            s.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        let sr = self.sample_rate;
        match id {
            ID_SIZE => self.update_size(),
            ID_DECAY => self.fdn.set_t60(value),
            ID_PREDELAY => {
                let p = self.pre_samples();
                self.pre_glide.set_target(p);
            }
            ID_DAMPING => {
                let hz = 20_000.0 * (1_200.0f32 / 20_000.0).powf(value);
                self.fdn.set_damping_hz(hz);
            }
            ID_DIFFUSION => {
                let g = 0.7 * value;
                for chain in &mut self.diffusers {
                    for a in chain {
                        a.set_gain(g);
                    }
                }
            }
            ID_MODULATION => self.fdn.set_mod_depth(value * 0.35e-3 * sr),
            ID_WIDTH => self.width.set_target(value),
            ID_LOW_CUT => {
                self.low_cut_on = value > 20.5;
                for f in &mut self.low_cut {
                    f.set_cutoff(sr, value);
                }
            }
            ID_HIGH_CUT => {
                self.high_cut_on = value < 19_500.0;
                for f in &mut self.high_cut {
                    f.set_cutoff(sr, value);
                }
            }
            ID_MIX => self.mix.set_target(value),
            ID_EARLY => self.early.set_target(value * 0.7),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for l in &mut self.pre {
            l.clear();
        }
        for chain in &mut self.diffusers {
            for a in chain {
                a.clear();
            }
        }
        self.fdn.clear();
        for f in &mut self.low_cut {
            f.reset();
        }
        for f in &mut self.high_cut {
            f.reset();
        }
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (in_l, in_r) = (*l, *r);
            let pd = self.pre_glide.next();
            let pl = self.pre[0].read_hermite(pd);
            let pr = self.pre[1].read_hermite(pd);
            let early = self.early.next();
            let er_scale = self.er_scale.next() * 0.001 * self.sample_rate;
            let (mut er_l, mut er_r) = (0.0f32, 0.0f32);
            if early > 0.0 {
                for k in 0..6 {
                    er_l += ER_GAIN[0][k] * self.pre[0].read_linear(pd + ER_MS[0][k] * er_scale);
                    er_r += ER_GAIN[1][k] * self.pre[1].read_linear(pd + ER_MS[1][k] * er_scale);
                }
                er_l *= early * 0.5;
                er_r *= early * 0.5;
            }
            self.pre[0].push(in_l);
            self.pre[1].push(in_r);

            let mut a = pl;
            let mut b = pr;
            for k in 0..4 {
                a = self.diffusers[0][k].process(a);
                b = self.diffusers[1][k].process(b);
            }

            let mut feed = [0.0f32; 8];
            for (i, f) in feed.iter_mut().enumerate() {
                *f = 0.5 * INJECT[i] * if i % 2 == 0 { a } else { b };
            }
            let taps = self.fdn.process(&feed);
            let mut wl = 0.0;
            let mut wr = 0.0;
            for i in 0..8 {
                wl += TAP_L[i] * taps[i];
                wr += TAP_R[i] * taps[i];
            }
            wl = wl * TAP_NORM + er_l;
            wr = wr * TAP_NORM + er_r;

            if self.low_cut_on {
                wl = self.low_cut[0].process(wl);
                wr = self.low_cut[1].process(wr);
            }
            if self.high_cut_on {
                wl = self.high_cut[0].process(wl);
                wr = self.high_cut[1].process(wr);
            }

            let width = self.width.next();
            let mid = (wl + wr) * 0.5;
            let side = (wl - wr) * 0.5 * width;
            let (wl, wr) = (mid + side, mid - side);

            let mix = self.mix.next();
            *l = in_l * (1.0 - mix) + wl * mix;
            *r = in_r * (1.0 - mix) + wr * mix;
        }
    }

    fn tail(&self) -> Option<f32> {
        Some(self.store.get(ID_DECAY) + self.store.get(ID_PREDELAY) * 0.001)
    }

    fn silent(&self) -> bool {
        // Twice the 60 dB time puts the tail at -120 dB.
        let secs = 2.0 * self.store.get(ID_DECAY) + self.store.get(ID_PREDELAY) * 0.001 + 0.1;
        self.silence.quiet_for((secs * self.sample_rate) as u32)
    }
}

impl_device!(Reverb, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    fn impulse_response(rv: &mut Reverb, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        l[0] = 1.0;
        r[0] = 1.0;
        let ctx = ProcessContext::new(SR);
        for (cl, cr) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            rv.process(&ctx, &[], cl, cr);
        }
        (l, r)
    }

    /// Reverse-integrated energy decay curve in dB, normalized to 0 dB.
    fn edc_db(x: &[f32]) -> Vec<f32> {
        let mut acc = 0.0f64;
        let mut out = vec![0.0f32; x.len()];
        for i in (0..x.len()).rev() {
            acc += f64::from(x[i]) * f64::from(x[i]);
            out[i] = acc as f32;
        }
        let total = out[0].max(1e-30);
        out.iter()
            .map(|v| 10.0 * (v / total).max(1e-30).log10())
            .collect()
    }

    fn t60_from_edc(edc: &[f32]) -> f32 {
        let find = |db: f32| edc.iter().position(|v| *v < db).unwrap_or(edc.len()) as f32;
        let (a, b) = (find(-5.0), find(-25.0));
        // 20 dB of decay measured between -5 and -25 dB, scaled to 60.
        (b - a) / SR * 3.0
    }

    #[test]
    fn decay_matches_the_decay_parameter() {
        for decay in [1.0f32, 2.5, 6.0] {
            let mut rv = Reverb::new();
            rv.set_param(ID_DECAY, decay);
            rv.set_param(ID_DAMPING, 0.0);
            rv.set_param(ID_MIX, 1.0);
            rv.set_param(ID_PREDELAY, 0.0);
            rv.prepare(SR, 512);
            let (l, _) = impulse_response(&mut rv, (SR * (decay * 1.6 + 1.0)) as usize);
            let measured = t60_from_edc(&edc_db(&l));
            assert!(
                (measured / decay - 1.0).abs() < 0.3,
                "decay {decay}: measured {measured}"
            );
        }
    }

    #[test]
    fn predelay_holds_back_the_wet_signal() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.set_param(ID_PREDELAY, 50.0);
        rv.prepare(SR, 512);
        let (l, r) = impulse_response(&mut rv, 12_000);
        let first = l
            .iter()
            .zip(&r)
            .position(|(a, b)| a.abs() > 1e-6 || b.abs() > 1e-6)
            .unwrap();
        let pd = (0.05 * SR) as usize;
        assert!(first >= pd - 2, "first sound at {first}, pre-delay {pd}");
        assert!(first < pd + 600, "{first}");
    }

    #[test]
    fn width_zero_is_mono_and_default_is_decorrelated() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.set_param(ID_WIDTH, 0.0);
        rv.prepare(SR, 512);
        let (l, r) = impulse_response(&mut rv, 20_000);
        for i in 0..l.len() {
            assert!((l[i] - r[i]).abs() < 1e-5);
        }
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.prepare(SR, 512);
        let (l, r) = impulse_response(&mut rv, 20_000);
        let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..l.len() {
            ll += f64::from(l[i] * l[i]);
            rr += f64::from(r[i] * r[i]);
            lr += f64::from(l[i] * r[i]);
        }
        let corr = lr / (ll * rr).sqrt();
        assert!(corr.abs() < 0.3, "inter-channel correlation {corr}");
        assert!(ll > 0.0 && rr > 0.0);
    }

    #[test]
    fn mix_zero_passes_dry_exactly() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 0.0);
        rv.prepare(SR, 512);
        let ctx = ProcessContext::new(SR);
        let mut l: Vec<f32> = (0..512).map(|i| (i as f32 * 0.1).sin()).collect();
        let mut r = l.clone();
        let want = l.clone();
        rv.process(&ctx, &[], &mut l, &mut r);
        for i in 0..512 {
            assert!((l[i] - want[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn size_changes_are_click_free_and_stable() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.set_param(ID_DECAY, 8.0);
        rv.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        let mut seed = 11u32;
        for block in 0..400 {
            let mut l: Vec<f32> = (0..256)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.3
                })
                .collect();
            let mut r = l.clone();
            if block % 50 == 0 {
                rv.set_param(ID_SIZE, if (block / 50) % 2 == 0 { 1.0 } else { 0.0 });
            }
            rv.process(&ctx, &[], &mut l, &mut r);
            for v in l.iter().chain(r.iter()) {
                assert!(v.is_finite() && v.abs() < 10.0, "{v}");
            }
        }
    }

    #[test]
    fn reports_tail_and_goes_silent() {
        let mut rv = Reverb::new();
        rv.set_param(ID_DECAY, 0.5);
        rv.set_param(ID_PREDELAY, 0.0);
        rv.prepare(SR, 512);
        let tail = rv.tail_seconds().unwrap();
        assert!((tail - 0.5).abs() < 0.01);
        let (l, _) = impulse_response(&mut rv, 4096);
        assert!(l.iter().any(|v| v.abs() > 1e-4));
        assert!(!rv.is_silent());
        let ctx = ProcessContext::new(SR);
        for _ in 0..((SR * 1.3) as usize / 512 + 2) {
            let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
            rv.process(&ctx, &[], &mut l, &mut r);
        }
        assert!(rv.is_silent());
    }
}
