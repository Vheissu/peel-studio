//! Feed-forward compressor with soft knee, stereo link, sidechain filter
//! and parallel mix.
//!
//! Detection: the (optionally high-passed) input of each channel goes
//! through a peak or RMS follower, is converted to dB, and the channel levels
//! are blended toward their maximum by the link amount. The static curve
//! ([`GainComputer`]) turns that level into a target gain change, which is
//! smoothed with attack and release in the log domain ([`GainSmoother`]).
//!
//! Auto makeup: when enabled, the compressor adds the static gain reduction
//! that a signal halfway (in dB) between the threshold and 0 dBFS would get,
//! `-threshold * (1 - 1 / ratio) / 2`, ignoring the knee. Loud peaks end up
//! somewhat louder than before and quiet material is brought up by the same
//! amount, which keeps perceived level roughly constant when the ratio or
//! threshold changes. It adds to the manual makeup control.

use peel_dsp::biquad::{Coeffs, FilterKind, StereoBiquad};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::dynamics::{DetectorMode, EnvelopeFollower, GainComputer, GainSmoother};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain, gain_to_db};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_THRESHOLD: ParamId = 0;
pub const ID_RATIO: ParamId = 1;
pub const ID_ATTACK: ParamId = 2;
pub const ID_RELEASE: ParamId = 3;
pub const ID_KNEE: ParamId = 4;
pub const ID_MAKEUP: ParamId = 5;
pub const ID_AUTO_MAKEUP: ParamId = 6;
pub const ID_MIX: ParamId = 7;
pub const ID_DETECTOR: ParamId = 8;
pub const ID_SC_HPF: ParamId = 9;
pub const ID_LINK: ParamId = 10;

const DETECTORS: &[&str] = &["Peak", "RMS"];

static PARAMS: [ParamDescriptor; 11] = [
    ParamDescriptor::linear(
        ID_THRESHOLD,
        "threshold",
        "Threshold",
        "Compression",
        -60.0,
        0.0,
        -18.0,
        Unit::Decibels,
    ),
    ParamDescriptor::power(
        ID_RATIO,
        "ratio",
        "Ratio",
        "Compression",
        1.0,
        20.0,
        4.0,
        Unit::Ratio,
        2.0,
    ),
    ParamDescriptor::log(
        ID_ATTACK,
        "attack",
        "Attack",
        "Compression",
        0.1,
        200.0,
        10.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::log(
        ID_RELEASE,
        "release",
        "Release",
        "Compression",
        10.0,
        2_000.0,
        150.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::linear(
        ID_KNEE,
        "knee",
        "Knee",
        "Compression",
        0.0,
        24.0,
        6.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_MAKEUP,
        "makeup",
        "Makeup",
        "Output",
        0.0,
        24.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::toggle(
        ID_AUTO_MAKEUP,
        "auto_makeup",
        "Auto Makeup",
        "Output",
        false,
    ),
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 1.0, Unit::Percent),
    ParamDescriptor::choice(ID_DETECTOR, "detector", "Mode", "Detector", DETECTORS, 0),
    ParamDescriptor::log(
        ID_SC_HPF,
        "sidechain.hpf",
        "Sidechain HP",
        "Detector",
        20.0,
        1_000.0,
        20.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_LINK,
        "link",
        "Stereo Link",
        "Detector",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.compressor",
    name: "Comp",
    description: "Compressor with soft knee, sidechain filter and parallel mix",
    category: "Dynamics",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Default",
        params: &[],
    },
    Preset {
        name: "Gentle Glue",
        params: &[
            ("threshold", -20.0),
            ("ratio", 2.0),
            ("attack", 30.0),
            ("release", 250.0),
            ("knee", 12.0),
            ("detector", 1.0),
            ("auto_makeup", 1.0),
        ],
    },
    Preset {
        name: "Punchy Drums",
        params: &[
            ("threshold", -16.0),
            ("ratio", 5.0),
            ("attack", 25.0),
            ("release", 90.0),
            ("knee", 4.0),
            ("sidechain.hpf", 120.0),
            ("auto_makeup", 1.0),
        ],
    },
    Preset {
        name: "Vocal Leveler",
        params: &[
            ("threshold", -24.0),
            ("ratio", 3.0),
            ("attack", 8.0),
            ("release", 180.0),
            ("knee", 10.0),
            ("detector", 1.0),
            ("auto_makeup", 1.0),
        ],
    },
    Preset {
        name: "Parallel Smash",
        params: &[
            ("threshold", -30.0),
            ("ratio", 12.0),
            ("attack", 2.0),
            ("release", 120.0),
            ("knee", 3.0),
            ("mix", 0.4),
            ("makeup", 6.0),
        ],
    },
];

/// Compressor. Read the current gain reduction with
/// [`Compressor::gain_reduction_db`].
pub struct Compressor {
    store: ParamStore,
    sample_rate: f32,
    computer: GainComputer,
    env: [EnvelopeFollower; 2],
    smooth: [GainSmoother; 2],
    sidechain: StereoBiquad,
    link: f32,
    makeup: Smoothed,
    mix: Smoothed,
    gr_db: f32,
    silence: SilenceTracker,
}

impl Default for Compressor {
    fn default() -> Self {
        Compressor::new()
    }
}

impl Compressor {
    pub fn new() -> Compressor {
        let mut c = Compressor {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            computer: GainComputer::new(-18.0, 4.0, 6.0),
            env: [EnvelopeFollower::new(DetectorMode::Peak); 2],
            smooth: [GainSmoother::new(); 2],
            sidechain: StereoBiquad::default(),
            link: 1.0,
            makeup: Smoothed::new(1.0),
            mix: Smoothed::new(1.0),
            gr_db: 0.0,
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut c, 48_000.0, 512);
        c
    }

    /// Gain reduction in dB (zero or positive) at the end of the last
    /// processed block: the larger of the two channels' smoothed reduction.
    pub fn gain_reduction_db(&self) -> f32 {
        self.gr_db
    }

    fn total_makeup_db(&self) -> f32 {
        let s = &self.store;
        let mut db = s.get(ID_MAKEUP);
        if s.get(ID_AUTO_MAKEUP) >= 0.5 {
            let ratio = s.get(ID_RATIO).max(1.0);
            db += -s.get(ID_THRESHOLD) * (1.0 - 1.0 / ratio) * 0.5;
        }
        db
    }

    fn update_detector_times(&mut self) {
        let sr = self.sample_rate;
        let mode = if self.store.get(ID_DETECTOR) >= 0.5 {
            DetectorMode::Rms
        } else {
            DetectorMode::Peak
        };
        for e in &mut self.env {
            e.set_mode(mode);
            match mode {
                // Fast rise, short hold so ripple does not reach the gain.
                DetectorMode::Peak => e.set_times(0.0, 0.010, sr),
                DetectorMode::Rms => e.set_times(0.015, 0.015, sr),
            }
        }
    }
}

impl Fx for Compressor {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        self.makeup.set_ramp(self.sample_rate, RAMP_SECONDS);
        self.mix.set_ramp(self.sample_rate, RAMP_SECONDS);
        self.sidechain.set_ramp_time(self.sample_rate, 0.01);
        refresh_all(self);
        let t = self.sidechain.target();
        self.sidechain.set_immediate(t);
        let (m, x) = (self.makeup.target(), self.mix.target());
        self.makeup.set_immediate(m);
        self.mix.set_immediate(x);
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        let sr = self.sample_rate;
        match id {
            ID_THRESHOLD | ID_RATIO | ID_KNEE | ID_MAKEUP | ID_AUTO_MAKEUP => {
                self.computer = GainComputer::new(
                    self.store.get(ID_THRESHOLD),
                    self.store.get(ID_RATIO),
                    self.store.get(ID_KNEE),
                );
                self.makeup.set_target(db_to_gain(self.total_makeup_db()));
            }
            ID_ATTACK | ID_RELEASE => {
                let a = self.store.get(ID_ATTACK) * 0.001;
                let r = self.store.get(ID_RELEASE) * 0.001;
                for s in &mut self.smooth {
                    s.set_times(a, r, sr);
                }
            }
            ID_MIX => self.mix.set_target(value),
            ID_DETECTOR => self.update_detector_times(),
            ID_SC_HPF => {
                let c = if value <= 20.5 {
                    Coeffs::IDENTITY
                } else {
                    Coeffs::design(
                        FilterKind::HighPass,
                        sr,
                        value,
                        std::f32::consts::FRAC_1_SQRT_2,
                        0.0,
                    )
                };
                self.sidechain.set_target(c);
            }
            ID_LINK => self.link = value,
            _ => {}
        }
    }

    fn clear(&mut self) {
        for e in &mut self.env {
            e.reset();
        }
        for s in &mut self.smooth {
            s.reset();
        }
        self.sidechain.reset();
        self.gr_db = 0.0;
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        let link = self.link;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (dl, dr) = (*l, *r);
            let (sl, sr) = self.sidechain.process(dl, dr);
            let db_l = gain_to_db(self.env[0].process(sl));
            let db_r = gain_to_db(self.env[1].process(sr));
            let linked = db_l.max(db_r);
            let ul = db_l + (linked - db_l) * link;
            let ur = db_r + (linked - db_r) * link;
            let gr_l = self.smooth[0].process(self.computer.gain_db(ul));
            let gr_r = self.smooth[1].process(self.computer.gain_db(ur));

            let makeup = self.makeup.next();
            let mix = self.mix.next();
            let wet_l = dl * db_to_gain(gr_l) * makeup;
            let wet_r = dr * db_to_gain(gr_r) * makeup;
            *l = dl + (wet_l - dl) * mix;
            *r = dr + (wet_r - dr) * mix;
        }
        self.gr_db = -(self.smooth[0].value_db().min(self.smooth[1].value_db()));
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for((self.sample_rate * 0.25) as u32)
    }
}

impl_device!(Compressor, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    fn sine_block(amp: f32, freq: f32, start: usize, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * (start + i) as f32 / SR).sin())
            .collect()
    }

    fn steady_peak_db(c: &mut Compressor, amp_db: f32, freq: f32) -> f32 {
        let amp = db_to_gain(amp_db);
        let ctx = ProcessContext::new(SR);
        let mut peak = 0.0f32;
        for block in 0..400 {
            let mut l = sine_block(amp, freq, block * 256, 256);
            let mut r = l.clone();
            c.process(&ctx, &[], &mut l, &mut r);
            if block > 300 {
                peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
            }
        }
        20.0 * peak.log10()
    }

    #[test]
    fn reduces_a_loud_sine_by_the_expected_amount() {
        // -6 dBFS peak into -24 dB threshold, 4:1, hard knee: 18 dB over, so
        // the output sits 13.5 dB lower at -19.5 dBFS.
        let mut c = Compressor::new();
        c.set_param(ID_THRESHOLD, -24.0);
        c.set_param(ID_RATIO, 4.0);
        c.set_param(ID_KNEE, 0.0);
        c.set_param(ID_ATTACK, 1.0);
        c.set_param(ID_RELEASE, 100.0);
        c.prepare(SR, 256);
        let out = steady_peak_db(&mut c, -6.0, 997.0);
        assert!((out - -19.5).abs() < 1.2, "output {out} dBFS");
        let gr = c.gain_reduction_db();
        assert!((gr - 13.5).abs() < 1.5, "gr {gr}");
    }

    #[test]
    fn below_threshold_is_untouched() {
        let mut c = Compressor::new();
        c.set_param(ID_THRESHOLD, -12.0);
        c.set_param(ID_KNEE, 0.0);
        c.prepare(SR, 256);
        let out = steady_peak_db(&mut c, -30.0, 440.0);
        assert!((out - -30.0).abs() < 0.05, "{out}");
        assert!(c.gain_reduction_db() < 0.01);
    }

    #[test]
    fn makeup_and_mix() {
        let mut c = Compressor::new();
        c.set_param(ID_THRESHOLD, -12.0);
        c.set_param(ID_MAKEUP, 6.0);
        c.set_param(ID_MIX, 0.0);
        c.prepare(SR, 256);
        // Fully dry: makeup is not applied to the dry path.
        let out = steady_peak_db(&mut c, -6.0, 440.0);
        assert!((out - -6.0).abs() < 0.05, "{out}");
    }

    #[test]
    fn auto_makeup_matches_documented_formula() {
        let mut c = Compressor::new();
        c.set_param(ID_THRESHOLD, -20.0);
        c.set_param(ID_RATIO, 4.0);
        c.set_param(ID_AUTO_MAKEUP, 1.0);
        c.prepare(SR, 256);
        // -20 dB threshold, 4:1: 10 * 0.75 = 7.5 dB.
        assert!((20.0 * c.makeup.target().log10() - 7.5).abs() < 1e-3);
        // A quiet signal under the threshold is raised by that amount.
        let out = steady_peak_db(&mut c, -40.0, 440.0);
        assert!((out - -32.5).abs() < 0.1, "{out}");
    }

    #[test]
    fn stereo_link_shares_reduction() {
        let ctx = ProcessContext::new(SR);
        let run = |link: f32| {
            let mut c = Compressor::new();
            c.set_param(ID_THRESHOLD, -24.0);
            c.set_param(ID_RATIO, 8.0);
            c.set_param(ID_KNEE, 0.0);
            c.set_param(ID_LINK, link);
            c.prepare(SR, 256);
            let mut quiet_peak = 0.0f32;
            for block in 0..200 {
                let mut l = sine_block(0.7, 300.0, block * 256, 256);
                let mut r = sine_block(0.02, 300.0, block * 256, 256);
                c.process(&ctx, &[], &mut l, &mut r);
                if block > 150 {
                    quiet_peak = r.iter().fold(quiet_peak, |m, v| m.max(v.abs()));
                }
            }
            quiet_peak
        };
        // Unlinked: the quiet right channel is below threshold and untouched.
        assert!((run(0.0) - 0.02).abs() < 0.001);
        // Linked: the loud left channel pulls the right channel down too.
        assert!(run(1.0) < 0.012);
    }

    #[test]
    fn sidechain_filter_ignores_bass() {
        let ctx = ProcessContext::new(SR);
        let run = |hpf: f32| {
            let mut c = Compressor::new();
            c.set_param(ID_THRESHOLD, -30.0);
            c.set_param(ID_RATIO, 10.0);
            c.set_param(ID_KNEE, 0.0);
            c.set_param(ID_SC_HPF, hpf);
            c.prepare(SR, 256);
            for block in 0..200 {
                let mut l = sine_block(0.5, 50.0, block * 256, 256);
                let mut r = l.clone();
                c.process(&ctx, &[], &mut l, &mut r);
            }
            c.gain_reduction_db()
        };
        assert!(run(20.0) > 10.0);
        assert!(run(400.0) < run(20.0) * 0.5);
    }

    #[test]
    fn recovers_to_unity_after_loud_passage() {
        let mut c = Compressor::new();
        c.set_param(ID_RELEASE, 50.0);
        c.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        for block in 0..100 {
            let mut l = sine_block(0.9, 440.0, block * 256, 256);
            let mut r = l.clone();
            c.process(&ctx, &[], &mut l, &mut r);
        }
        assert!(c.gain_reduction_db() > 3.0);
        for _ in 0..100 {
            let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
            c.process(&ctx, &[], &mut l, &mut r);
        }
        assert!(c.gain_reduction_db() < 0.01);
    }
}
