//! Utility: gain, balance, stereo width, mono, polarity, channel swap and
//! DC block in one small device.
//!
//! Order: polarity, swap, DC block, mono, width (mid/side), gain, balance.
//! Width scales the side signal only (`M = (L + R) / 2`, `S = (L - R) / 2`),
//! so the mono sum is unchanged by it. Balance uses the project's equal-power
//! law (`util::pan_gains`) applied to each channel: it is unity at center and
//! attenuates the far side when moved.

use peel_dsp::biquad::DcBlocker;
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain, pan_gains};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_GAIN: ParamId = 0;
pub const ID_BALANCE: ParamId = 1;
pub const ID_WIDTH: ParamId = 2;
pub const ID_MONO: ParamId = 3;
pub const ID_PHASE_L: ParamId = 4;
pub const ID_PHASE_R: ParamId = 5;
pub const ID_SWAP: ParamId = 6;
pub const ID_DC_BLOCK: ParamId = 7;

static PARAMS: [ParamDescriptor; 8] = [
    ParamDescriptor::linear(
        ID_GAIN,
        "gain",
        "Gain",
        "Level",
        -96.0,
        24.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_BALANCE,
        "balance",
        "Balance",
        "Level",
        -1.0,
        1.0,
        0.0,
        Unit::Pan,
    ),
    ParamDescriptor::linear(
        ID_WIDTH,
        "width",
        "Width",
        "Stereo",
        0.0,
        2.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::toggle(ID_MONO, "mono", "Mono", "Stereo", false),
    ParamDescriptor::toggle(ID_SWAP, "swap", "Swap L/R", "Stereo", false),
    ParamDescriptor::toggle(ID_PHASE_L, "phase_l", "Invert L", "Polarity", false),
    ParamDescriptor::toggle(ID_PHASE_R, "phase_r", "Invert R", "Polarity", false),
    ParamDescriptor::toggle(ID_DC_BLOCK, "dc_block", "DC Block", "Filter", false),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.utility",
    name: "Utility",
    description: "Gain, balance, stereo width, mono, polarity and DC block",
    category: "Utility",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Neutral",
        params: &[],
    },
    Preset {
        name: "Mono Check",
        params: &[("mono", 1.0)],
    },
    Preset {
        name: "Wide",
        params: &[("width", 1.4)],
    },
    Preset {
        name: "Narrow",
        params: &[("width", 0.5)],
    },
    Preset {
        name: "Clean DC",
        params: &[("dc_block", 1.0)],
    },
];

/// Gain, balance, width, mono, polarity and DC block.
pub struct Utility {
    store: ParamStore,
    sample_rate: f32,
    gain: Smoothed,
    bal_l: Smoothed,
    bal_r: Smoothed,
    width: Smoothed,
    mono: Smoothed,
    swap: Smoothed,
    pol_l: Smoothed,
    pol_r: Smoothed,
    dc_mix: Smoothed,
    dc_l: DcBlocker,
    dc_r: DcBlocker,
    silence: SilenceTracker,
}

impl Default for Utility {
    fn default() -> Self {
        Utility::new()
    }
}

impl Utility {
    pub fn new() -> Utility {
        let mut u = Utility {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            gain: Smoothed::new(1.0),
            bal_l: Smoothed::new(1.0),
            bal_r: Smoothed::new(1.0),
            width: Smoothed::new(1.0),
            mono: Smoothed::new(0.0),
            swap: Smoothed::new(0.0),
            pol_l: Smoothed::new(1.0),
            pol_r: Smoothed::new(1.0),
            dc_mix: Smoothed::new(0.0),
            dc_l: DcBlocker::new(48_000.0, 10.0),
            dc_r: DcBlocker::new(48_000.0, 10.0),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut u, 48_000.0, 512);
        u
    }

    fn smoothers(&mut self) -> [&mut Smoothed; 9] {
        [
            &mut self.gain,
            &mut self.bal_l,
            &mut self.bal_r,
            &mut self.width,
            &mut self.mono,
            &mut self.swap,
            &mut self.pol_l,
            &mut self.pol_r,
            &mut self.dc_mix,
        ]
    }
}

impl Fx for Utility {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        for s in self.smoothers() {
            s.set_ramp(sr, RAMP_SECONDS);
        }
        self.dc_l = DcBlocker::new(sr, 10.0);
        self.dc_r = DcBlocker::new(sr, 10.0);
        refresh_all(self);
        for s in self.smoothers() {
            let t = s.target();
            s.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_GAIN => {
                let g = if value <= -95.9 {
                    0.0
                } else {
                    db_to_gain(value)
                };
                self.gain.set_target(g);
            }
            ID_BALANCE => {
                let (l, r) = pan_gains(value);
                self.bal_l.set_target(l);
                self.bal_r.set_target(r);
            }
            ID_WIDTH => self.width.set_target(value),
            ID_MONO => self.mono.set_target(value),
            ID_SWAP => self.swap.set_target(value),
            ID_PHASE_L => self.pol_l.set_target(if value >= 0.5 { -1.0 } else { 1.0 }),
            ID_PHASE_R => self.pol_r.set_target(if value >= 0.5 { -1.0 } else { 1.0 }),
            ID_DC_BLOCK => self.dc_mix.set_target(value),
            _ => {}
        }
    }

    fn clear(&mut self) {
        self.dc_l.reset();
        self.dc_r.reset();
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let pl = self.pol_l.next();
            let pr = self.pol_r.next();
            let (mut a, mut b) = (*l * pl, *r * pr);

            let sw = self.swap.next();
            if sw != 0.0 {
                let (x, y) = (a, b);
                a = x + (y - x) * sw;
                b = y + (x - y) * sw;
            }

            let da = self.dc_l.process(a);
            let db = self.dc_r.process(b);
            let dc = self.dc_mix.next();
            if dc != 0.0 {
                a += (da - a) * dc;
                b += (db - b) * dc;
            }

            let mono = self.mono.next();
            let mid = (a + b) * 0.5;
            let side = (a - b) * 0.5;
            let w = self.width.next();
            // Mono crossfades the side signal away; width scales it.
            let side = side * w * (1.0 - mono);
            let g = self.gain.next();
            *l = (mid + side) * g * self.bal_l.next();
            *r = (mid - side) * g * self.bal_r.next();
        }
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for((self.sample_rate * 0.1) as u32)
    }
}

impl_device!(Utility, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    fn run(u: &mut Utility, l: &mut [f32], r: &mut [f32]) {
        u.process(&ProcessContext::new(48_000.0), &[], l, r);
    }

    #[test]
    fn default_is_transparent() {
        let mut u = Utility::new();
        let mut l: Vec<f32> = (0..512).map(|i| (i as f32 * 0.07).sin() * 0.8).collect();
        let mut r: Vec<f32> = (0..512).map(|i| (i as f32 * 0.11).cos() * 0.6).collect();
        let (l0, r0) = (l.clone(), r.clone());
        run(&mut u, &mut l, &mut r);
        for i in 0..512 {
            assert!((l[i] - l0[i]).abs() < 1e-6 && (r[i] - r0[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn mono_collapses_channels() {
        let mut u = Utility::new();
        u.set_param(ID_MONO, 1.0);
        u.prepare(48_000.0, 256);
        let mut l: Vec<f32> = (0..256).map(|i| (i as f32 * 0.07).sin()).collect();
        let mut r: Vec<f32> = (0..256).map(|i| (i as f32 * 0.31).cos() * 0.5).collect();
        let expect: Vec<f32> = l.iter().zip(&r).map(|(a, b)| (a + b) * 0.5).collect();
        run(&mut u, &mut l, &mut r);
        for i in 0..256 {
            assert!((l[i] - r[i]).abs() < 1e-6);
            assert!((l[i] - expect[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn width_zero_is_mono_and_two_doubles_side() {
        let mut u = Utility::new();
        u.set_param(ID_WIDTH, 0.0);
        u.prepare(48_000.0, 64);
        let mut l = vec![1.0; 64];
        let mut r = vec![0.0; 64];
        run(&mut u, &mut l, &mut r);
        assert!((l[63] - 0.5).abs() < 1e-6 && (r[63] - 0.5).abs() < 1e-6);

        let mut u = Utility::new();
        u.set_param(ID_WIDTH, 2.0);
        u.prepare(48_000.0, 64);
        let mut l = vec![1.0; 64];
        let mut r = vec![0.0; 64];
        run(&mut u, &mut l, &mut r);
        // M = 0.5, S = 1.0 -> L = 1.5, R = -0.5. The mono sum is unchanged.
        assert!((l[63] - 1.5).abs() < 1e-6 && (r[63] + 0.5).abs() < 1e-6);
        assert!(((l[63] + r[63]) * 0.5 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn polarity_swap_gain_and_balance() {
        let mut u = Utility::new();
        u.set_param(ID_PHASE_L, 1.0);
        u.set_param(ID_SWAP, 1.0);
        u.set_param(ID_GAIN, -6.0206);
        u.prepare(48_000.0, 64);
        let mut l = vec![0.4; 64];
        let mut r = vec![0.8; 64];
        run(&mut u, &mut l, &mut r);
        // After polarity: (-0.4, 0.8); swapped: (0.8, -0.4); gain 0.5.
        assert!((l[63] - 0.4).abs() < 1e-4 && (r[63] + 0.2).abs() < 1e-4);

        let mut u = Utility::new();
        u.set_param(ID_BALANCE, 1.0);
        u.prepare(48_000.0, 64);
        let mut l = vec![1.0; 64];
        let mut r = vec![1.0; 64];
        run(&mut u, &mut l, &mut r);
        assert!(l[63].abs() < 1e-5 && (r[63] - std::f32::consts::SQRT_2).abs() < 1e-4);
    }

    #[test]
    fn minimum_gain_mutes() {
        let mut u = Utility::new();
        u.set_param(ID_GAIN, -96.0);
        u.prepare(48_000.0, 64);
        let mut l = vec![1.0; 64];
        let mut r = vec![1.0; 64];
        run(&mut u, &mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|v| *v == 0.0));
    }

    #[test]
    fn dc_block_removes_offset() {
        let mut u = Utility::new();
        u.set_param(ID_DC_BLOCK, 1.0);
        u.prepare(48_000.0, 512);
        let mut tail = 1.0f32;
        for _ in 0..200 {
            let mut l = vec![0.3; 512];
            let mut r = vec![-0.2; 512];
            run(&mut u, &mut l, &mut r);
            tail = l[511].abs().max(r[511].abs());
        }
        assert!(tail < 1e-3, "{tail}");
    }

    #[test]
    fn toggling_mono_mid_stream_is_click_free() {
        let mut u = Utility::new();
        u.prepare(48_000.0, 256);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for block in 0..20 {
            let mut l: Vec<f32> = (0..256)
                .map(|i| ((block * 256 + i) as f32 * 0.05).sin() * 0.5)
                .collect();
            let mut r: Vec<f32> = (0..256)
                .map(|i| -((block * 256 + i) as f32 * 0.05).sin() * 0.5)
                .collect();
            if block == 10 {
                u.set_param(ID_MONO, 1.0);
            }
            run(&mut u, &mut l, &mut r);
            for v in l {
                worst = worst.max((v - prev).abs());
                prev = v;
            }
        }
        assert!(worst < 0.06, "{worst}");
    }
}
