//! Feed-forward compressor with soft knee, stereo link, sidechain filter
//! and parallel mix.
//!
//! Detection: the (optionally high-passed) input of each channel goes
//! through a peak or RMS follower, is converted to dB, and the channel levels
//! are blended toward their maximum by the link amount. The static curve
//! ([`GainComputer`]) turns that level into a target gain change, which is
//! smoothed with attack, hold and release in the log domain ([`Ballistics`]).
//!
//! Three detector modes: *Peak* follows the rectified signal, *RMS* a
//! mean-square average (gentle, little low-frequency ripple) and *Glue* an
//! RMS-style level with a two-stage program-dependent release: short peaks
//! recover quickly while sustained reduction eases off slowly, which is what
//! lets a bus compressor hold a mix together without pumping. *Hold* keeps the
//! gain at its deepest for a set time before release begins, which shapes a
//! sidechain duck into a plateau and a smooth return instead of a ragged
//! follow of the key's decay.
//!
//! Auto makeup: when enabled, the compressor adds the static gain reduction
//! that a signal halfway (in dB) between the threshold and 0 dBFS would get,
//! `-threshold * (1 - 1 / ratio) / 2`, ignoring the knee. Loud peaks end up
//! somewhat louder than before and quiet material is brought up by the same
//! amount, which keeps perceived level roughly constant when the ratio or
//! threshold changes. It adds to the manual makeup control.

use peel_dsp::biquad::{Coeffs, FilterKind, StereoBiquad};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::dynamics::{DetectorMode, EnvelopeFollower, GainComputer, time_coeff};
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
pub const ID_HOLD: ParamId = 11;

const DETECTORS: &[&str] = &["Peak", "RMS", "Glue"];

static PARAMS: [ParamDescriptor; 12] = [
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
    ParamDescriptor::power(
        ID_HOLD,
        "hold",
        "Hold",
        "Compression",
        0.0,
        250.0,
        0.0,
        Unit::Milliseconds,
        2.0,
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

pub static PRESETS: [Preset; 8] = [
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
            ("detector", 2.0),
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
        // Set a sidechain from the kick's track to pump with the beat.
        name: "Sidechain Duck",
        params: &[
            ("threshold", -32.0),
            ("ratio", 12.0),
            ("attack", 1.0),
            ("release", 140.0),
            ("knee", 3.0),
            ("auto_makeup", 0.0),
            ("makeup", 0.0),
        ],
    },
    Preset {
        // Key from the kick: drops fast, sits, then swells back before the
        // next beat.
        name: "EDM Pump",
        params: &[
            ("threshold", -34.0),
            ("ratio", 20.0),
            ("attack", 0.5),
            ("hold", 40.0),
            ("release", 105.0),
            ("knee", 2.0),
        ],
    },
    Preset {
        name: "Bus Glue",
        params: &[
            ("threshold", -16.0),
            ("ratio", 2.0),
            ("attack", 30.0),
            ("release", 200.0),
            ("knee", 14.0),
            ("detector", 2.0),
            ("sidechain.hpf", 90.0),
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

/// Gain-reduction ballistics in the log domain: attack while the reduction
/// deepens, an optional hold at the deepest point, then release. In dual mode
/// the release runs as two parallel recoveries (a fast one and a slow one)
/// whose average is the gain, so it eases back quickly at first and finishes
/// slowly.
#[derive(Clone, Copy, Debug)]
struct Ballistics {
    attack: f32,
    fast: f32,
    slow: f32,
    dual: bool,
    hold_samples: u32,
    hold: u32,
    state_fast: f32,
    state_slow: f32,
}

impl Ballistics {
    fn new() -> Ballistics {
        Ballistics {
            attack: 0.0,
            fast: 0.999,
            slow: 0.999,
            dual: false,
            hold_samples: 0,
            hold: 0,
            state_fast: 0.0,
            state_slow: 0.0,
        }
    }

    fn set(&mut self, attack_s: f32, release_s: f32, hold_s: f32, dual: bool, sample_rate: f32) {
        self.attack = time_coeff(attack_s, sample_rate);
        self.dual = dual;
        if dual {
            self.fast = time_coeff(release_s * 0.3, sample_rate);
            self.slow = time_coeff(release_s * 3.0, sample_rate);
        } else {
            self.fast = time_coeff(release_s, sample_rate);
            self.slow = self.fast;
        }
        self.hold_samples = (hold_s * sample_rate) as u32;
    }

    fn reset(&mut self) {
        self.state_fast = 0.0;
        self.state_slow = 0.0;
        self.hold = 0;
    }

    /// Current gain in dB (zero or negative).
    fn value_db(&self) -> f32 {
        if self.dual {
            0.5 * (self.state_fast + self.state_slow)
        } else {
            self.state_fast
        }
    }

    /// Moves toward `target_db` (zero or negative); returns the new gain.
    #[inline]
    fn process(&mut self, target_db: f32) -> f32 {
        if target_db < self.value_db() {
            self.hold = self.hold_samples;
        } else if self.hold > 0 {
            self.hold -= 1;
        }
        let hold = self.hold > 0;
        let step = |state: f32, release: f32| {
            if target_db < state {
                target_db + self.attack * (state - target_db)
            } else if hold {
                state
            } else {
                target_db + release * (state - target_db)
            }
        };
        self.state_fast = step(self.state_fast, self.fast);
        self.state_slow = if self.dual {
            step(self.state_slow, self.slow)
        } else {
            self.state_fast
        };
        for st in [&mut self.state_fast, &mut self.state_slow] {
            if *st > -1e-3 && target_db >= 0.0 {
                *st = 0.0;
            }
        }
        self.value_db()
    }
}

/// Compressor. Read the current gain reduction with
/// [`Compressor::gain_reduction_db`].
pub struct Compressor {
    store: ParamStore,
    sample_rate: f32,
    computer: GainComputer,
    env: [EnvelopeFollower; 2],
    smooth: [Ballistics; 2],
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
            smooth: [Ballistics::new(); 2],
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
        let kind = self.store.get(ID_DETECTOR) as usize;
        let mode = if kind >= 1 {
            DetectorMode::Rms
        } else {
            DetectorMode::Peak
        };
        // The peak follower lets go over a few milliseconds; the longer the
        // release setting, the longer it holds, so the ripple of low notes
        // never reaches the gain.
        let peak_release = (self.store.get(ID_RELEASE) * 0.0002).clamp(0.008, 0.03);
        for e in &mut self.env {
            e.set_mode(mode);
            match kind {
                0 => e.set_times(0.0, peak_release, sr),
                1 => e.set_times(0.015, 0.015, sr),
                _ => e.set_times(0.003, 0.05, sr),
            }
        }
    }

    fn update_ballistics(&mut self) {
        let a = self.store.get(ID_ATTACK) * 0.001;
        let r = self.store.get(ID_RELEASE) * 0.001;
        let h = self.store.get(ID_HOLD) * 0.001;
        let dual = self.store.get(ID_DETECTOR) >= 1.5;
        for s in &mut self.smooth {
            s.set(a, r, h, dual, self.sample_rate);
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

    fn reduction(&self) -> Option<f32> {
        Some(self.gr_db)
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
            ID_ATTACK | ID_HOLD => self.update_ballistics(),
            ID_RELEASE => {
                self.update_ballistics();
                self.update_detector_times();
            }
            ID_MIX => self.mix.set_target(value),
            ID_DETECTOR => {
                self.update_detector_times();
                self.update_ballistics();
            }
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
        self.compress(left, right, None);
    }

    fn render_keyed(
        &mut self,
        _ctx: &ProcessContext,
        left: &mut [f32],
        right: &mut [f32],
        key: (&[f32], &[f32]),
    ) {
        self.compress(left, right, Some(key));
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for((self.sample_rate * 0.25) as u32)
    }
}

impl Compressor {
    /// Compresses in place. The detector listens to `key` when given (an
    /// external sidechain), otherwise to the signal itself.
    fn compress(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        self.silence.feed(left, right);
        let link = self.link;
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let (dl, dr) = (*l, *r);
            let (kl, kr) = match key {
                Some((kl, kr)) => (kl[i], kr[i]),
                None => (dl, dr),
            };
            let (sl, sr) = self.sidechain.process(kl, kr);
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
    fn an_external_key_ducks_a_quiet_signal() {
        let mut c = Compressor::new();
        c.set_param(ID_THRESHOLD, -30.0);
        c.set_param(ID_RATIO, 10.0);
        c.set_param(ID_KNEE, 0.0);
        c.set_param(ID_ATTACK, 1.0);
        c.set_param(ID_RELEASE, 50.0);
        c.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        let quiet = db_to_gain(-36.0);
        let run = |c: &mut Compressor, key_amp: f32| {
            let mut peak = 0.0f32;
            for block in 0..200 {
                let mut l = sine_block(quiet, 440.0, block * 256, 256);
                let mut r = l.clone();
                let k = sine_block(key_amp, 60.0, block * 256, 256);
                c.process_keyed(&ctx, &[], &mut l, &mut r, &k, &k);
                if block > 150 {
                    peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
                }
            }
            20.0 * peak.log10()
        };
        // The signal alone is below the threshold: untouched by its own level.
        let alone = run(&mut c, 0.0);
        assert!((alone - -36.0).abs() < 0.5, "{alone}");
        // A loud key 24 dB over the threshold pulls it down about 21.6 dB.
        let ducked = run(&mut c, db_to_gain(-6.0));
        assert!(alone - ducked > 18.0, "ducked {ducked} vs {alone}");
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

    /// Level of the harmonics 2..=5 against the fundamental, in dB, for a
    /// steady low sine at `amp` through the compressor.
    fn harmonic_distortion_db(c: &mut Compressor, freq: f32, amp: f32) -> f32 {
        let ctx = ProcessContext::new(SR);
        let n = 48_128;
        let mut out = Vec::with_capacity(n);
        for block in 0..(2 * n / 256) {
            let mut l = sine_block(amp, freq, block * 256, 256);
            let mut r = l.clone();
            c.process(&ctx, &[], &mut l, &mut r);
            if block >= n / 256 {
                out.extend_from_slice(&l);
            }
        }
        out.truncate(48_000);
        let tone = |f: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, v) in out.iter().enumerate() {
                let ph = std::f64::consts::TAU * f64::from(f) * (n + i) as f64 / f64::from(SR);
                re += f64::from(*v) * ph.cos();
                im += f64::from(*v) * ph.sin();
            }
            (re * re + im * im).sqrt()
        };
        let harm: f64 = (2..=5)
            .map(|k| tone(freq * k as f32).powi(2))
            .sum::<f64>()
            .sqrt();
        (20.0 * (harm / tone(freq)).log10()) as f32
    }

    #[test]
    fn low_notes_are_compressed_without_audible_distortion() {
        for mode in [0.0f32, 1.0, 2.0] {
            let mut c = Compressor::new();
            c.set_param(ID_THRESHOLD, -18.0);
            c.set_param(ID_RATIO, 4.0);
            c.set_param(ID_ATTACK, 10.0);
            c.set_param(ID_RELEASE, 150.0);
            c.set_param(ID_DETECTOR, mode);
            c.prepare(SR, 256);
            let thd = harmonic_distortion_db(&mut c, 60.0, 0.5);
            assert!(c.gain_reduction_db() > 4.0, "mode {mode}: no reduction");
            assert!(thd < -62.0, "mode {mode}: harmonics at {thd} dB");
        }
    }

    #[test]
    fn hold_keeps_the_reduction_before_release_begins() {
        let gr_after = |hold: f32, gap_ms: usize| {
            let mut c = Compressor::new();
            c.set_param(ID_THRESHOLD, -30.0);
            c.set_param(ID_RATIO, 10.0);
            c.set_param(ID_KNEE, 0.0);
            c.set_param(ID_ATTACK, 1.0);
            c.set_param(ID_RELEASE, 40.0);
            c.set_param(ID_HOLD, hold);
            c.prepare(SR, 256);
            let ctx = ProcessContext::new(SR);
            for block in 0..60 {
                let mut l = sine_block(0.5, 200.0, block * 256, 256);
                let mut r = l.clone();
                c.process(&ctx, &[], &mut l, &mut r);
            }
            let peak = c.gain_reduction_db();
            let frames = SR as usize * gap_ms / 1000;
            for _ in 0..(frames / 8) {
                let (mut l, mut r) = (vec![0.0; 8], vec![0.0; 8]);
                c.process(&ctx, &[], &mut l, &mut r);
            }
            (peak, c.gain_reduction_db())
        };
        let (peak, after) = gr_after(0.0, 50);
        assert!(after < peak * 0.5, "no hold: {after} of {peak} dB left");
        let (peak, after) = gr_after(80.0, 50);
        assert!(
            after > peak * 0.95,
            "hold 80 ms: {after} of {peak} dB left at 50 ms"
        );
        let (peak, after) = gr_after(80.0, 400);
        assert!(
            after < peak * 0.1,
            "hold 80 ms: {after} of {peak} dB left at 400 ms"
        );
    }

    #[test]
    fn glue_release_is_slower_but_complete() {
        // The glue mode lets go more slowly than RMS with the same setting
        // (a slow second stage and a smoother detector), but still lets go.
        let remaining = |mode: f32, gap_ms: usize| {
            let mut c = Compressor::new();
            c.set_param(ID_THRESHOLD, -24.0);
            c.set_param(ID_RATIO, 4.0);
            c.set_param(ID_ATTACK, 5.0);
            c.set_param(ID_RELEASE, 200.0);
            c.set_param(ID_KNEE, 0.0);
            c.set_param(ID_DETECTOR, mode);
            c.prepare(SR, 256);
            let ctx = ProcessContext::new(SR);
            for block in 0..200 {
                let mut l = sine_block(0.5, 300.0, block * 256, 256);
                let mut r = l.clone();
                c.process(&ctx, &[], &mut l, &mut r);
            }
            let peak = c.gain_reduction_db();
            let frames = SR as usize * gap_ms / 1000;
            for _ in 0..(frames / 8) {
                let (mut l, mut r) = (vec![0.0; 8], vec![0.0; 8]);
                c.process(&ctx, &[], &mut l, &mut r);
            }
            c.gain_reduction_db() / peak
        };
        let (rms_early, glue_early) = (remaining(1.0, 100), remaining(2.0, 100));
        let (rms_late, glue_late) = (remaining(1.0, 800), remaining(2.0, 800));
        assert!(glue_late > rms_late, "{glue_late} vs {rms_late}");
        assert!(glue_early > rms_early, "{glue_early} vs {rms_early}");
        assert!(glue_late < 0.3, "glue should still let go: {glue_late}");
    }
}
