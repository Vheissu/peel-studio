//! Four-band parametric EQ with high-pass and low-pass cuts.
//!
//! Signal path: high-pass cut, low shelf, two bells, high shelf, low-pass
//! cut, output gain. Every stage is an RBJ biquad (transposed direct form II)
//! whose coefficients ramp over a few milliseconds, so automation and knob
//! drags are click-free. Cuts are Butterworth cascades of 6, 12, 24 or
//! 48 dB/oct. A disabled bell or shelf glides to 0 dB gain before it is
//! skipped, so toggling a band does not click either.

use peel_dsp::biquad::{self, Coeffs, FilterKind, StereoBiquad};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

// Parameter ids. Stable: never renumber, only append.
pub const ID_HP_ENABLE: ParamId = 1;
pub const ID_HP_FREQ: ParamId = 2;
pub const ID_HP_SLOPE: ParamId = 3;
pub const ID_LP_ENABLE: ParamId = 4;
pub const ID_LP_FREQ: ParamId = 5;
pub const ID_LP_SLOPE: ParamId = 6;
pub const ID_OUTPUT: ParamId = 0;
/// Band `n` (1..=4) parameter ids are `10 * n + {0: enable, 1: freq, 2: gain, 3: q}`.
pub const fn band_id(band: u32, field: u32) -> ParamId {
    10 * band + field
}

const SLOPES: &[&str] = &["6 dB/oct", "12 dB/oct", "24 dB/oct", "48 dB/oct"];

#[allow(clippy::too_many_arguments)]
const fn band_params(
    n: u32,
    group: &'static str,
    enable_key: &'static str,
    freq_key: &'static str,
    gain_key: &'static str,
    q_key: &'static str,
    freq: f32,
    q: f32,
) -> [ParamDescriptor; 4] {
    [
        ParamDescriptor::toggle(band_id(n, 0), enable_key, "Enable", group, true),
        ParamDescriptor::log(
            band_id(n, 1),
            freq_key,
            "Freq",
            group,
            20.0,
            20_000.0,
            freq,
            Unit::Hertz,
        ),
        ParamDescriptor::linear(
            band_id(n, 2),
            gain_key,
            "Gain",
            group,
            -24.0,
            24.0,
            0.0,
            Unit::Decibels,
        ),
        ParamDescriptor::log(band_id(n, 3), q_key, "Q", group, 0.1, 18.0, q, Unit::None),
    ]
}

const B1: [ParamDescriptor; 4] = band_params(
    1,
    "Band 1",
    "band1.enable",
    "band1.freq",
    "band1.gain",
    "band1.q",
    100.0,
    0.71,
);
const B2: [ParamDescriptor; 4] = band_params(
    2,
    "Band 2",
    "band2.enable",
    "band2.freq",
    "band2.gain",
    "band2.q",
    400.0,
    1.0,
);
const B3: [ParamDescriptor; 4] = band_params(
    3,
    "Band 3",
    "band3.enable",
    "band3.freq",
    "band3.gain",
    "band3.q",
    2_500.0,
    1.0,
);
const B4: [ParamDescriptor; 4] = band_params(
    4,
    "Band 4",
    "band4.enable",
    "band4.freq",
    "band4.gain",
    "band4.q",
    8_000.0,
    0.71,
);

/// Number of parameters; `eq_response_db` expects a slice of this length.
pub const PARAM_COUNT: usize = 23;

static PARAMS: [ParamDescriptor; PARAM_COUNT] = [
    ParamDescriptor::toggle(ID_HP_ENABLE, "hp.enable", "Enable", "Low Cut", false),
    ParamDescriptor::log(
        ID_HP_FREQ,
        "hp.freq",
        "Freq",
        "Low Cut",
        20.0,
        2_000.0,
        80.0,
        Unit::Hertz,
    ),
    ParamDescriptor::choice(ID_HP_SLOPE, "hp.slope", "Slope", "Low Cut", SLOPES, 1),
    B1[0],
    B1[1],
    B1[2],
    B1[3],
    B2[0],
    B2[1],
    B2[2],
    B2[3],
    B3[0],
    B3[1],
    B3[2],
    B3[3],
    B4[0],
    B4[1],
    B4[2],
    B4[3],
    ParamDescriptor::toggle(ID_LP_ENABLE, "lp.enable", "Enable", "High Cut", false),
    ParamDescriptor::log(
        ID_LP_FREQ,
        "lp.freq",
        "Freq",
        "High Cut",
        1_000.0,
        20_000.0,
        18_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::choice(ID_LP_SLOPE, "lp.slope", "Slope", "High Cut", SLOPES, 1),
    ParamDescriptor::linear(
        ID_OUTPUT,
        "output",
        "Gain",
        "Output",
        -24.0,
        24.0,
        0.0,
        Unit::Decibels,
    ),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.eq",
    name: "EQ",
    description: "Four-band parametric equalizer with low and high cut",
    category: "EQ",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Flat",
        params: &[],
    },
    Preset {
        name: "Clean Low End",
        params: &[
            ("hp.enable", 1.0),
            ("hp.freq", 30.0),
            ("hp.slope", 2.0),
            ("band1.gain", 2.0),
            ("band1.freq", 70.0),
            ("band2.gain", -3.0),
            ("band2.freq", 300.0),
            ("band2.q", 1.2),
        ],
    },
    Preset {
        name: "Vocal Presence",
        params: &[
            ("hp.enable", 1.0),
            ("hp.freq", 90.0),
            ("band2.gain", -2.5),
            ("band2.freq", 280.0),
            ("band3.gain", 3.0),
            ("band3.freq", 3_200.0),
            ("band3.q", 0.9),
            ("band4.gain", 2.5),
            ("band4.freq", 11_000.0),
        ],
    },
    Preset {
        name: "Air",
        params: &[
            ("band4.gain", 4.0),
            ("band4.freq", 10_000.0),
            ("band3.gain", 1.5),
            ("band3.freq", 5_500.0),
            ("band3.q", 0.7),
        ],
    },
    Preset {
        name: "Telephone",
        params: &[
            ("hp.enable", 1.0),
            ("hp.freq", 450.0),
            ("hp.slope", 2.0),
            ("lp.enable", 1.0),
            ("lp.freq", 3_400.0),
            ("lp.slope", 2.0),
            ("band3.gain", 4.0),
            ("band3.freq", 1_500.0),
            ("band3.q", 1.5),
        ],
    },
];

/// Butterworth order for a slope choice index.
fn slope_order(choice: f32) -> u32 {
    match choice as u32 {
        0 => 1,
        1 => 2,
        2 => 4,
        _ => 8,
    }
}

fn filter_kind(band: u32) -> FilterKind {
    match band {
        1 => FilterKind::LowShelf,
        4 => FilterKind::HighShelf,
        _ => FilterKind::Peak,
    }
}

/// Plain value of the parameter with `id` in an index-ordered slice.
fn pv(params: &[f32], id: ParamId) -> f32 {
    match PARAMS.iter().position(|p| p.id == id) {
        Some(i) => params.get(i).copied().unwrap_or(PARAMS[i].default),
        None => 0.0,
    }
}

fn band_coeffs_from(params: &[f32], band: u32, sample_rate: f32) -> Option<Coeffs> {
    if pv(params, band_id(band, 0)) < 0.5 {
        return None;
    }
    Some(Coeffs::design(
        filter_kind(band),
        sample_rate,
        pv(params, band_id(band, 1)),
        pv(params, band_id(band, 3)),
        pv(params, band_id(band, 2)),
    ))
}

fn cut_sections(
    params: &[f32],
    highpass: bool,
    sample_rate: f32,
    out: &mut [Coeffs; 4],
) -> Option<usize> {
    let (en, fr, sl) = if highpass {
        (ID_HP_ENABLE, ID_HP_FREQ, ID_HP_SLOPE)
    } else {
        (ID_LP_ENABLE, ID_LP_FREQ, ID_LP_SLOPE)
    };
    if pv(params, en) < 0.5 {
        return None;
    }
    Some(biquad::butterworth(
        highpass,
        slope_order(pv(params, sl)),
        sample_rate,
        pv(params, fr),
        out,
    ))
}

/// Magnitude response of one band in dB at `freq` Hz.
///
/// `band` is 1..=4. `params` is the plain value of every EQ parameter in
/// descriptor order (`DESCRIPTOR.params[i]` is described by `params[i]`; see
/// [`crate::read_params`]). Returns 0 dB for a disabled band.
pub fn eq_band_response_db(params: &[f32], band: u32, sample_rate: f32, freq: f32) -> f32 {
    band_coeffs_from(params, band.clamp(1, 4), sample_rate)
        .map_or(0.0, |c| c.magnitude_db(sample_rate, freq))
}

/// Total magnitude response of the EQ in dB at `freq` Hz, including the
/// output gain. This evaluates the same coefficients the audio path uses, so
/// a UI can draw the curve with it. `params` is the plain value of every
/// parameter in descriptor order (`DESCRIPTOR.params[i]` is described by
/// `params[i]`; see [`crate::read_params`]). Missing entries fall back to
/// the descriptor default, so a short slice is safe.
pub fn eq_response_db(params: &[f32], sample_rate: f32, freq: f32) -> f32 {
    let mut db = pv(params, ID_OUTPUT);
    let mut sections = [Coeffs::IDENTITY; 4];
    for highpass in [true, false] {
        if let Some(n) = cut_sections(params, highpass, sample_rate, &mut sections) {
            db += sections[..n]
                .iter()
                .map(|c| c.magnitude_db(sample_rate, freq))
                .sum::<f32>();
        }
    }
    for band in 1..=4 {
        db += eq_band_response_db(params, band, sample_rate, freq);
    }
    db
}

struct Band {
    filter: StereoBiquad,
    enabled: bool,
}

impl Band {
    fn new() -> Band {
        Band {
            filter: StereoBiquad::default(),
            enabled: false,
        }
    }
}

/// Four-band parametric EQ. See the module docs.
pub struct Eq {
    store: ParamStore,
    sample_rate: f32,
    hp: [StereoBiquad; 4],
    lp: [StereoBiquad; 4],
    bands: [Band; 4],
    gain: Smoothed,
    silence: SilenceTracker,
}

impl Default for Eq {
    fn default() -> Self {
        Eq::new()
    }
}

impl Eq {
    pub fn new() -> Eq {
        let mut eq = Eq {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            hp: [StereoBiquad::default(); 4],
            lp: [StereoBiquad::default(); 4],
            bands: [Band::new(), Band::new(), Band::new(), Band::new()],
            gain: Smoothed::new(1.0),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut eq, 48_000.0, 512);
        eq
    }

    fn update_cut(&mut self, highpass: bool) {
        let mut sections = [Coeffs::IDENTITY; 4];
        let params = self.store.values();
        let used = cut_sections(params, highpass, self.sample_rate, &mut sections);
        let filters = if highpass { &mut self.hp } else { &mut self.lp };
        let n = used.unwrap_or(0);
        for (i, f) in filters.iter_mut().enumerate() {
            f.set_target(if i < n { sections[i] } else { Coeffs::IDENTITY });
        }
    }

    fn update_band(&mut self, band: u32) {
        let params = self.store.values();
        let enabled = pv(params, band_id(band, 0)) >= 0.5;
        let kind = filter_kind(band);
        // A disabled band glides to 0 dB so the switch is click-free.
        let gain = if enabled {
            pv(params, band_id(band, 2))
        } else {
            0.0
        };
        let c = Coeffs::design(
            kind,
            self.sample_rate,
            pv(params, band_id(band, 1)),
            pv(params, band_id(band, 3)),
            gain,
        );
        let b = &mut self.bands[band as usize - 1];
        b.enabled = enabled;
        b.filter.set_target(c);
    }
}

impl Fx for Eq {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let ramp = |f: &mut StereoBiquad| f.set_ramp_time(sample_rate, 0.006);
        self.hp.iter_mut().for_each(ramp);
        self.lp.iter_mut().for_each(ramp);
        self.bands.iter_mut().for_each(|b| ramp(&mut b.filter));
        self.gain.set_ramp(sample_rate, RAMP_SECONDS);
        refresh_all(self);
        // Snap everything to the stored values: no ramp from stale state.
        for f in self.hp.iter_mut().chain(self.lp.iter_mut()) {
            let t = f.target();
            f.set_immediate(t);
        }
        for b in &mut self.bands {
            let t = b.filter.target();
            b.filter.set_immediate(t);
        }
        let g = db_to_gain(self.store.get(ID_OUTPUT));
        self.gain.set_immediate(g);
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_HP_ENABLE | ID_HP_FREQ | ID_HP_SLOPE => self.update_cut(true),
            ID_LP_ENABLE | ID_LP_FREQ | ID_LP_SLOPE => self.update_cut(false),
            ID_OUTPUT => self.gain.set_target(db_to_gain(value)),
            _ => {
                let band = id / 10;
                if (1..=4).contains(&band) {
                    self.update_band(band);
                }
            }
        }
    }

    fn clear(&mut self) {
        for f in self.hp.iter_mut().chain(self.lp.iter_mut()) {
            f.reset();
        }
        for b in &mut self.bands {
            b.filter.reset();
        }
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        // Unused cut sections sit at identity and cost nothing.
        for f in &mut self.hp {
            f.process_block(left, right);
        }
        for b in &mut self.bands {
            if !b.enabled && !b.filter.is_ramping() {
                b.filter.reset();
                continue;
            }
            b.filter.process_block(left, right);
        }
        for f in &mut self.lp {
            f.process_block(left, right);
        }
        if self.gain.is_smoothing() || (self.gain.value() - 1.0).abs() > 1e-6 {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let g = self.gain.next();
                *l *= g;
                *r *= g;
            }
        }
    }

    fn tail(&self) -> Option<f32> {
        // Resonant low-frequency bells ring for a short while.
        Some(0.5)
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for((self.sample_rate * 0.5) as u32)
    }
}

impl_device!(Eq, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    fn run_sine(eq: &mut Eq, freq: f32, sr: f32) -> f32 {
        let n = (sr as usize) / 2;
        let mut l: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / sr).sin())
            .collect();
        let mut r = l.clone();
        let ctx = ProcessContext::new(sr);
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            eq.process(&ctx, &[], cl, cr);
        }
        l[n / 2..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn descriptor_is_consistent() {
        let ids: std::collections::HashSet<_> = PARAMS.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), PARAMS.len());
        let keys: std::collections::HashSet<_> = PARAMS.iter().map(|p| p.key).collect();
        assert_eq!(keys.len(), PARAMS.len());
        assert_eq!(DESCRIPTOR.params.len(), PARAM_COUNT);
    }

    #[test]
    fn response_function_matches_audio_path_at_band_centers() {
        let sr = 48_000.0;
        let mut eq = Eq::new();
        eq.prepare(sr, 256);
        let setup = [
            (1, 120.0, 6.0, 0.8),
            (2, 500.0, -5.0, 1.4),
            (3, 3_000.0, 7.5, 2.0),
            (4, 9_000.0, -4.0, 0.9),
        ];
        for (band, f, g, q) in setup {
            eq.set_param(band_id(band, 1), f);
            eq.set_param(band_id(band, 2), g);
            eq.set_param(band_id(band, 3), q);
        }
        eq.prepare(sr, 256); // snap smoothers
        let values: Vec<f32> = DESCRIPTOR.params.iter().map(|p| eq.param(p.id)).collect();
        for (band, f, g, _q) in setup {
            // Each band alone reaches its gain at its center (shelves reach
            // half their gain there).
            let alone = eq_band_response_db(&values, band, sr, f);
            let expected = if band == 1 || band == 4 { g * 0.5 } else { g };
            assert!(
                (alone - expected).abs() < 0.5,
                "band {band}: {alone} vs {expected}"
            );
            // The audio path agrees with the analytic total.
            let analytic = eq_response_db(&values, sr, f);
            let measured = {
                let mut e = Eq::new();
                for p in DESCRIPTOR.params {
                    e.set_param(p.id, eq.param(p.id));
                }
                e.prepare(sr, 256);
                20.0 * run_sine(&mut e, f, sr).log10()
            };
            assert!(
                (analytic - measured).abs() < 0.3,
                "band {band}: analytic {analytic} measured {measured}"
            );
        }
    }

    #[test]
    fn peak_bands_hit_configured_gain_at_center() {
        let sr = 44_100.0;
        for (band, g) in [(2u32, 9.0f32), (3, -9.0)] {
            let mut eq = Eq::new();
            eq.set_param(band_id(band, 1), 1_234.0);
            eq.set_param(band_id(band, 2), g);
            eq.prepare(sr, 256);
            let m = 20.0 * run_sine(&mut eq, 1_234.0, sr).log10();
            assert!((m - g).abs() < 0.5, "{m} vs {g}");
        }
    }

    #[test]
    fn cuts_roll_off() {
        let sr = 48_000.0;
        let mut eq = Eq::new();
        eq.set_param(ID_HP_ENABLE, 1.0);
        eq.set_param(ID_HP_FREQ, 1_000.0);
        eq.set_param(ID_HP_SLOPE, 2.0);
        eq.set_param(ID_LP_ENABLE, 1.0);
        eq.set_param(ID_LP_FREQ, 4_000.0);
        eq.set_param(ID_LP_SLOPE, 3.0);
        eq.prepare(sr, 256);
        let values: Vec<f32> = DESCRIPTOR.params.iter().map(|p| eq.param(p.id)).collect();
        assert!((eq_response_db(&values, sr, 1_000.0) + 3.0).abs() < 0.3);
        assert!((eq_response_db(&values, sr, 4_000.0) + 3.0).abs() < 0.3);
        // 24 dB/oct high-pass: one octave down is about -24 dB.
        assert!((eq_response_db(&values, sr, 500.0) + 24.0).abs() < 2.0);
        let m = 20.0 * run_sine(&mut eq, 250.0, sr).log10();
        assert!(m < -45.0, "{m}");
        let m = 20.0 * run_sine(&mut eq, 12_000.0, sr).log10();
        assert!(m < -60.0, "{m}");
        let m = 20.0 * run_sine(&mut eq, 2_000.0, sr).log10();
        assert!(m.abs() < 1.0, "{m}");
    }

    #[test]
    fn default_eq_is_transparent() {
        let sr = 48_000.0;
        let mut eq = Eq::new();
        eq.prepare(sr, 256);
        for f in [50.0, 1_000.0, 15_000.0] {
            let m = 20.0 * run_sine(&mut eq, f, sr).log10();
            assert!(m.abs() < 0.01, "{f}: {m}");
        }
    }

    #[test]
    fn output_gain_and_band_toggle_are_smooth() {
        let sr = 48_000.0;
        let mut eq = Eq::new();
        eq.set_param(band_id(2, 2), 12.0);
        eq.set_param(band_id(2, 1), 200.0);
        eq.prepare(sr, 256);
        let ctx = ProcessContext::new(sr);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for block in 0..40 {
            let mut l: Vec<f32> = (0..256)
                .map(|i| {
                    (std::f32::consts::TAU * 200.0 * (block * 256 + i) as f32 / sr).sin() * 0.3
                })
                .collect();
            let mut r = l.clone();
            if block == 20 {
                eq.set_param(band_id(2, 0), 0.0);
            }
            eq.process(&ctx, &[], &mut l, &mut r);
            for v in l {
                worst = worst.max((v - prev).abs());
                prev = v;
            }
        }
        // 200 Hz at 0.3 * 4x gain: slope per sample is tiny; a click would
        // be far larger.
        assert!(worst < 0.1, "{worst}");
        // After disabling, the band is flat again.
        let m = 20.0 * run_sine(&mut eq, 200.0, sr).log10();
        assert!(m.abs() < 0.1, "{m}");
    }
}
