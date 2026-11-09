//! Live spectrum, peak history and resonance data for the open effect
//! editor. All numbers come from the session's analysis tap; nothing here
//! invents samples. With no new frame for a moment (transport stopped,
//! effect bypassed away from the tap) the display falls toward silence
//! instead of holding stale audio.

use peel_document::Id;
use peel_engine::analysis::{
    Resonance, SPECTRUM_BINS, SpectrumAnalyzer, WAVEFORM_POINTS, WaveformPeak,
};
use peel_session::Session;
use peel_ui::Ui;

/// dBFS floor of every display array.
pub const FLOOR_DB: f32 = -120.0;
/// Seconds without a new frame before the display starts to fall.
const STALE_AFTER: f64 = 0.25;
/// Fall speed of a stale or released display, dB per second.
const FALL_DB_S: f32 = 40.0;
/// Fall speed of the live (smoothed) spectrum, dB per second.
const SMOOTH_DB_S: f32 = 60.0;
/// Peak-hold decay after its hold, dB per second.
const PEAK_FALL_DB_S: f32 = 6.0;

pub struct Analysis {
    analyzer: SpectrumAnalyzer,
    pub device: Option<Id>,
    /// Smoothed input and output spectra (dBFS per FFT bin).
    pub pre: Vec<f32>,
    pub post: Vec<f32>,
    /// Output peak hold per bin.
    pub peak: Vec<f32>,
    pub wave: Vec<WaveformPeak>,
    pub sample_rate: f32,
    pub input_peak_db: f32,
    pub output_peak_db: f32,
    pub input_rms_db: f32,
    pub output_rms_db: f32,
    pub analyzed_seconds: f32,
    pub resonances: Vec<Resonance>,
    last_new: f64,
    peak_since: f64,
    /// The tap accepted the device.
    pub tapped: bool,
}

impl Default for Analysis {
    fn default() -> Self {
        Analysis {
            analyzer: SpectrumAnalyzer::new(),
            device: None,
            pre: vec![FLOOR_DB; SPECTRUM_BINS],
            post: vec![FLOOR_DB; SPECTRUM_BINS],
            peak: vec![FLOOR_DB; SPECTRUM_BINS],
            wave: vec![WaveformPeak::default(); WAVEFORM_POINTS],
            sample_rate: 48_000.0,
            input_peak_db: FLOOR_DB,
            output_peak_db: FLOOR_DB,
            input_rms_db: FLOOR_DB,
            output_rms_db: FLOOR_DB,
            analyzed_seconds: 0.0,
            resonances: Vec::new(),
            last_new: f64::NEG_INFINITY,
            peak_since: 0.0,
            tapped: false,
        }
    }
}

impl Analysis {
    /// Clears every display and the learned history (Reset / Relearn).
    pub fn reset(&mut self) {
        let device = self.device;
        *self = Analysis::default();
        self.device = device;
    }

    /// Clears only the peak hold.
    pub fn clear_peaks(&mut self) {
        self.peak.fill(FLOOR_DB);
    }

    /// True while frames are arriving.
    pub fn live(&self, now: f64) -> bool {
        now - self.last_new < STALE_AFTER
    }

    /// Points the tap at `device` (or turns it off) and folds in the newest
    /// frame. Call once per frame; keeps the UI redrawing while moving.
    pub fn sync(&mut self, ui: &mut Ui, s: &mut Session, device: Option<Id>) {
        if self.device != device {
            self.reset();
            self.device = device;
        }
        self.tapped = s.set_analysis_device(device);
        let Some(_) = device.filter(|_| self.tapped) else {
            return;
        };
        let now = ui.time();
        let dt = ui.dt();
        let mut fresh = false;
        if let Some(frame) = s.analysis_snapshot()
            && self.analyzer.update(&frame)
        {
            fresh = true;
            self.last_new = now;
            let snap = self.analyzer.snapshot();
            self.sample_rate = snap.sample_rate;
            for i in 0..SPECTRUM_BINS {
                self.pre[i] = rise_fall(self.pre[i], snap.input_db[i], dt, SMOOTH_DB_S);
                self.post[i] = rise_fall(self.post[i], snap.output_db[i], dt, SMOOTH_DB_S);
                if snap.output_db[i] >= self.peak[i] {
                    self.peak[i] = snap.output_db[i];
                }
            }
            self.peak_since = now;
            self.wave.copy_from_slice(&snap.waveform);
            self.input_peak_db = snap.input_peak_db;
            self.output_peak_db = snap.output_peak_db;
            self.input_rms_db = snap.input_rms_db;
            self.output_rms_db = snap.output_rms_db;
            self.analyzed_seconds = snap.analyzed_seconds;
            self.resonances = snap.resonances.clone();
        }
        if !fresh && !self.live(now) {
            let fall = FALL_DB_S * dt;
            for v in self.pre.iter_mut().chain(self.post.iter_mut()) {
                *v = (*v - fall).max(FLOOR_DB);
            }
            for w in self.wave.iter_mut() {
                w.min *= 0.9;
                w.max *= 0.9;
            }
            self.input_peak_db = (self.input_peak_db - fall).max(FLOOR_DB);
            self.output_peak_db = (self.output_peak_db - fall).max(FLOOR_DB);
            self.input_rms_db = (self.input_rms_db - fall).max(FLOOR_DB);
            self.output_rms_db = (self.output_rms_db - fall).max(FLOOR_DB);
            // Candidates describe audio that is no longer arriving.
            self.resonances.clear();
        }
        if now - self.peak_since > 1.5 {
            for v in self.peak.iter_mut() {
                *v = (*v - PEAK_FALL_DB_S * dt).max(FLOOR_DB);
            }
        }
        // Keep drawing until everything has settled.
        if self.live(now) || self.pre.iter().any(|v| *v > FLOOR_DB + 0.5) {
            ui.request_repaint();
        }
    }

    /// Spectrum value for the frequency band `[lo, hi]` Hz taken from
    /// `data`: interpolated when the band is narrower than a bin, the
    /// loudest bin otherwise.
    pub fn sample(&self, data: &[f32], lo: f32, hi: f32) -> f32 {
        let bin_hz = self.sample_rate / peel_engine::analysis::FFT_SIZE as f32;
        let (a, b) = (lo / bin_hz, hi / bin_hz);
        let last = (SPECTRUM_BINS - 1) as f32;
        if b - a < 1.0 {
            let c = ((a + b) * 0.5).clamp(0.0, last);
            let i = c.floor() as usize;
            let j = (i + 1).min(SPECTRUM_BINS - 1);
            let f = c - i as f32;
            data[i] * (1.0 - f) + data[j] * f
        } else {
            let i0 = a.floor().clamp(0.0, last) as usize;
            let i1 = b.ceil().clamp(0.0, last) as usize;
            data[i0..=i1].iter().copied().fold(FLOOR_DB, f32::max)
        }
    }
}

fn rise_fall(old: f32, new: f32, dt: f32, fall_db_s: f32) -> f32 {
    if new >= old {
        new
    } else {
        (old - fall_db_s * dt).max(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rise_is_instant_and_fall_is_bounded() {
        assert_eq!(rise_fall(-60.0, -20.0, 0.016, 60.0), -20.0);
        let v = rise_fall(-20.0, -100.0, 0.1, 60.0);
        assert!((v + 26.0).abs() < 1e-4);
        assert_eq!(rise_fall(-20.0, -21.0, 1.0, 60.0), -21.0);
    }

    #[test]
    fn narrow_bands_interpolate_and_wide_bands_take_the_loudest_bin() {
        let a = Analysis::default();
        let mut data = vec![-100.0; SPECTRUM_BINS];
        data[10] = -10.0;
        let bin_hz = 48_000.0 / 2048.0;
        let wide = a.sample(&data, 9.0 * bin_hz, 12.0 * bin_hz);
        assert_eq!(wide, -10.0);
        let narrow = a.sample(&data, 10.2 * bin_hz, 10.4 * bin_hz);
        assert!(narrow < -10.0 && narrow > -100.0);
    }
}
