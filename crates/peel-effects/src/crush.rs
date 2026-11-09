//! Sample-and-hold reduction and symmetric, zero-preserving quantisation.
use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};
use peel_dsp::biquad::{Biquad, Coeffs, FilterKind, OnePoleLowpass};
use peel_dsp::noise::Noise;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{Smoothed, db_to_gain};
use peel_dsp::{DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};
pub const ID_BITS: ParamId = 1;
pub const ID_RATE: ParamId = 2;
pub const ID_JITTER: ParamId = 3;
pub const ID_FILTER: ParamId = 4;
pub const ID_DRIVE: ParamId = 5;
pub const ID_TONE: ParamId = 6;
pub const ID_OUTPUT: ParamId = 7;
pub const ID_MIX: ParamId = 8;
static PARAMS: [ParamDescriptor; 8] = [
    ParamDescriptor::linear(1, "bits", "Bits", "Resolution", 1.0, 16.0, 10.0, Unit::Bits),
    ParamDescriptor::log(
        2,
        "rate",
        "Rate",
        "Resolution",
        500.0,
        48_000.0,
        16_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        3,
        "jitter",
        "Jitter",
        "Resolution",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::toggle(4, "filter", "Filter", "Resolution", false),
    ParamDescriptor::linear(5, "drive", "Drive", "Tone", 0.0, 24.0, 0.0, Unit::Decibels),
    ParamDescriptor::log(
        6,
        "tone",
        "Tone",
        "Tone",
        1_000.0,
        20_000.0,
        20_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        7,
        "output",
        "Output",
        "Output",
        -24.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(8, "mix", "Mix", "Output", 0.0, 1.0, 1.0, Unit::Percent),
];
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.crush",
    name: "Crush",
    description: "Bit depth and sample rate reduction with jitter",
    category: "Distortion",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};
pub static PRESETS: [Preset; 7] = [
    Preset {
        name: "Early Sampler",
        params: &[],
    },
    Preset {
        name: "Twelve Bit",
        params: &[
            ("bits", 12.0),
            ("rate", 26_000.0),
            ("filter", 1.0),
            ("tone", 12_000.0),
        ],
    },
    Preset {
        name: "Chip",
        params: &[("bits", 5.0), ("rate", 22_050.0)],
    },
    Preset {
        name: "Radio",
        params: &[
            ("bits", 8.0),
            ("rate", 8_000.0),
            ("filter", 1.0),
            ("tone", 3_500.0),
            ("drive", 6.0),
        ],
    },
    Preset {
        name: "Wobbly",
        params: &[("rate", 12_000.0), ("jitter", 0.6)],
    },
    Preset {
        name: "Grit",
        params: &[
            ("bits", 6.0),
            ("rate", 30_000.0),
            ("drive", 9.0),
            ("mix", 0.4),
        ],
    },
    Preset {
        name: "Destroy",
        params: &[
            ("bits", 2.5),
            ("rate", 2_000.0),
            ("drive", 12.0),
            ("mix", 0.6),
            ("output", -6.0),
        ],
    },
];
pub struct Crush {
    store: ParamStore,
    sr: f32,
    p: [Smoothed; 8],
    pre: [[Biquad; 2]; 2],
    post: [OnePoleLowpass; 2],
    hold: [f32; 2],
    phase: f32,
    period: f32,
    rng: Noise,
    control: u8,
    steps: f32,
    drive: f32,
    output: f32,
    silence: SilenceTracker,
}
impl Default for Crush {
    fn default() -> Self {
        Self::new()
    }
}
impl Crush {
    pub fn new() -> Self {
        let mut s = Self {
            store: ParamStore::new(&DESCRIPTOR),
            sr: 48_000.0,
            p: std::array::from_fn(|i| Smoothed::new(PARAMS[i].default)),
            pre: std::array::from_fn(|_| std::array::from_fn(|_| Biquad::new(Coeffs::IDENTITY))),
            post: [OnePoleLowpass::default(); 2],
            hold: [0.0; 2],
            phase: 1.0,
            period: 1.0,
            rng: Noise::new(73),
            control: 0,
            steps: 512.0,
            drive: 1.0,
            output: 1.0,
            silence: SilenceTracker::default(),
        };
        s.setup(48_000.0, 512);
        s
    }
    fn coefficients(&mut self, p: &[f32; 8]) {
        // Reserve a symmetric set around zero. At one bit, retain both
        // polarities and an exact zero code rather than rectifying the signal.
        self.steps = (2.0f32.powf(p[0] - 1.0) - 1.0).max(1.0);
        self.drive = db_to_gain(p[4]);
        self.output = db_to_gain(p[6]);
        for ch in 0..2 {
            for (stage, q) in [0.541_196_1, 1.306_563].into_iter().enumerate() {
                self.pre[ch][stage].coeffs = Coeffs::design(
                    FilterKind::LowPass,
                    self.sr,
                    (p[1] * 0.45).min(self.sr * 0.45),
                    q,
                    0.0,
                );
            }
            self.post[ch].set_cutoff(self.sr, p[5]);
        }
    }
}
impl Fx for Crush {
    fn store(&self) -> &ParamStore {
        &self.store
    }
    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }
    fn setup(&mut self, sr: f32, _max: usize) {
        self.sr = sr.max(8_000.0);
        for p in &mut self.p {
            p.set_ramp(self.sr, RAMP_SECONDS);
        }
        refresh_all(self);
        for p in &mut self.p {
            p.set_immediate(p.target());
        }
        self.coefficients(&self.p.each_ref().map(Smoothed::value));
        self.clear();
    }
    fn param_changed(&mut self, id: ParamId, v: f32) {
        if let Some(p) = self.p.get_mut(id.wrapping_sub(1) as usize) {
            p.set_target(v);
        }
    }
    fn clear(&mut self) {
        for f in self.pre.iter_mut().flatten() {
            f.reset();
        }
        for f in &mut self.post {
            f.reset();
        }
        self.hold = [0.0; 2];
        self.phase = 1.0;
        self.period = 1.0;
        self.rng.reseed(73);
        self.control = 0;
        self.silence.reset();
    }
    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let p = self.p.each_mut().map(Smoothed::next);
            if self.control == 0 {
                self.coefficients(&p);
            }
            self.control = (self.control + 1) % 8;
            let dry = [*l, *r];
            let input = std::array::from_fn::<_, 2, _>(|ch| {
                let x = dry[ch] * self.drive;
                let mut y = x;
                for f in &mut self.pre[ch] {
                    y = f.process(y);
                }
                x + (y - x) * p[3]
            });
            if p[1] >= self.sr || self.phase >= self.period {
                self.phase = if p[1] >= self.sr {
                    0.0
                } else {
                    self.phase - self.period
                };
                self.period = 1.0 + 0.5 * p[2] * self.rng.white();
                self.hold = input.map(|x| {
                    (x.clamp(-1.0, 1.0) * self.steps)
                        .round()
                        .clamp(-self.steps, self.steps)
                        / self.steps
                });
            }
            self.phase += (p[1] / self.sr).min(1.0);
            let wet = std::array::from_fn::<_, 2, _>(|ch| self.post[ch].process(self.hold[ch]));
            *l = (dry[0] + (wet[0] - dry[0]) * p[7]) * self.output;
            *r = (dry[1] + (wet[1] - dry[1]) * p[7]) * self.output;
        }
    }
    fn tail(&self) -> Option<f32> {
        Some(0.1)
    }
    fn silent(&self) -> bool {
        self.silence.quiet_for((0.2 * self.sr) as u32)
    }
}
impl_device!(Crush, DESCRIPTOR);
