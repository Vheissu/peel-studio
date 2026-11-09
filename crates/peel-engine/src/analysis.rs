//! A bounded, selected-insert audio tap and control-thread spectrum analysis.
//!
//! The callback only copies into atomics. A reader makes one seqlock attempt,
//! never blocks the writer, and discards a concurrent/incomplete snapshot. FFTs,
//! peak history and resonance suggestions are computed by [`SpectrumAnalyzer`]
//! on the control thread. Stereo power is averaged, so anti-phase material is
//! visible instead of disappearing in a mono sum.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const FFT_SIZE: usize = 2048;
pub const SPECTRUM_BINS: usize = FFT_SIZE / 2 + 1;
pub const WAVEFORM_POINTS: usize = 128;
const FLOOR: f32 = -120.0;

/// Chronological audio at one selected mixer insert's input and output.
#[derive(Clone)]
pub struct AnalysisFrame {
    pub key: u64,
    pub sequence: u64,
    pub sample_rate: f32,
    pub valid_frames: usize,
    pub total_frames: u64,
    pub input_left: [f32; FFT_SIZE],
    pub input_right: [f32; FFT_SIZE],
    pub output_left: [f32; FFT_SIZE],
    pub output_right: [f32; FFT_SIZE],
}

/// Shared audio tap. All storage is allocated before the callback starts.
pub struct AnalysisTap {
    // Low 32 bits are slot + 1 (zero disables); high bits are generation.
    target: AtomicU64,
    writer_key: AtomicU64,
    sequence: AtomicU64,
    frames: AtomicU64,
    sample_rate: AtomicU32,
    samples: Box<[AtomicU32]>,
}

pub(crate) struct Capture {
    key: u64,
    start: u64,
    frames: usize,
}

impl AnalysisTap {
    pub fn new() -> Self {
        Self {
            target: AtomicU64::new(0),
            writer_key: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            sample_rate: AtomicU32::new(0),
            samples: (0..FFT_SIZE * 4).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    /// Control thread: selecting the current slot is a no-op. A new selection
    /// invalidates all earlier audio immediately, including an in-flight block.
    pub fn set_slot(&self, slot: Option<u32>) {
        let slot = slot
            .filter(|&s| s < crate::telemetry::MAX_SLOTS as u32)
            .map_or(0, |s| u64::from(s) + 1);
        let _ = self
            .target
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current & 0xffff_ffff != slot)
                    .then(|| ((current.wrapping_add(1 << 32)) & !0xffff_ffff) | slot)
            });
    }

    /// Transport cuts and replacement devices start a fresh capture window.
    /// This bounded atomic operation is also safe on the audio thread.
    pub fn invalidate(&self) {
        self.target.fetch_add(1 << 32, Ordering::AcqRel);
    }

    pub(crate) fn is_selected(&self, slot: u32) -> bool {
        self.target.load(Ordering::Relaxed) as u32 == slot + 1
    }

    pub(crate) fn begin(&self, slot: u32, left: &[f32], right: &[f32]) -> Option<Capture> {
        let key = self.target.load(Ordering::Acquire);
        if key as u32 != slot + 1 {
            return None;
        }
        self.sequence.fetch_add(1, Ordering::AcqRel); // odd while writing
        let start = if self.writer_key.load(Ordering::Relaxed) == key {
            self.frames.load(Ordering::Relaxed)
        } else {
            self.writer_key.store(key, Ordering::Relaxed);
            0
        };
        let frames = left.len().min(right.len());
        // Only the final window matters when a host gives a very large block.
        for i in frames.saturating_sub(FFT_SIZE)..frames {
            let at = (start as usize + i) % FFT_SIZE;
            self.store(at, 0, left[i]);
            self.store(at, 1, right[i]);
        }
        Some(Capture { key, start, frames })
    }

    pub(crate) fn finish(&self, capture: Option<Capture>, left: &[f32], right: &[f32], rate: f32) {
        let Some(c) = capture else { return };
        debug_assert!(left.len() >= c.frames && right.len() >= c.frames);
        for i in c.frames.saturating_sub(FFT_SIZE)..c.frames {
            let at = (c.start as usize + i) % FFT_SIZE;
            self.store(at, 2, left[i]);
            self.store(at, 3, right[i]);
        }
        self.frames
            .store(c.start + c.frames as u64, Ordering::Relaxed);
        self.sample_rate.store(rate.to_bits(), Ordering::Relaxed);
        self.writer_key.store(c.key, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release); // publish even version
    }

    #[inline]
    fn store(&self, frame: usize, channel: usize, sample: f32) {
        let sample = if sample.is_finite() {
            sample.clamp(-1e6, 1e6)
        } else {
            0.0
        };
        self.samples[frame * 4 + channel].store(sample.to_bits(), Ordering::Relaxed);
    }

    /// One bounded read attempt. `None` means disabled, no fresh selection
    /// audio, or a writer overlapped the read; retry on a later UI frame.
    pub fn snapshot(&self) -> Option<AnalysisFrame> {
        let sequence = self.sequence.load(Ordering::Acquire);
        let key = self.target.load(Ordering::Acquire);
        if sequence & 1 != 0 || key as u32 == 0 || self.writer_key.load(Ordering::Relaxed) != key {
            return None;
        }
        let total = self.frames.load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        let valid = total.min(FFT_SIZE as u64) as usize;
        let mut out = AnalysisFrame {
            key,
            sequence,
            sample_rate: f32::from_bits(self.sample_rate.load(Ordering::Relaxed)),
            valid_frames: valid,
            total_frames: total,
            input_left: [0.0; FFT_SIZE],
            input_right: [0.0; FFT_SIZE],
            output_left: [0.0; FFT_SIZE],
            output_right: [0.0; FFT_SIZE],
        };
        for i in 0..valid {
            let at = ((total - valid as u64) as usize + i) % FFT_SIZE;
            let dest = FFT_SIZE - valid + i;
            out.input_left[dest] = f32::from_bits(self.samples[at * 4].load(Ordering::Relaxed));
            out.input_right[dest] =
                f32::from_bits(self.samples[at * 4 + 1].load(Ordering::Relaxed));
            out.output_left[dest] =
                f32::from_bits(self.samples[at * 4 + 2].load(Ordering::Relaxed));
            out.output_right[dest] =
                f32::from_bits(self.samples[at * 4 + 3].load(Ordering::Relaxed));
        }
        // Acquire fence prevents the payload loads from moving past the final
        // version check (the payload itself is atomic, so no data race exists).
        std::sync::atomic::fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Acquire) != sequence
            || self.target.load(Ordering::Acquire) != key
        {
            return None;
        }
        Some(out)
    }
}

impl Default for AnalysisTap {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WaveformPeak {
    pub min: f32,
    pub max: f32,
}

/// A sustained narrow peak above its local spectral neighborhood. Musical
/// fundamentals can qualify; these are candidates to audition, not errors.
#[derive(Clone, Copy, Debug)]
pub struct Resonance {
    pub frequency_hz: f32,
    pub q: f32,
    pub cut_db: f32,
    pub prominence_db: f32,
}

pub struct SpectrumSnapshot {
    pub sample_rate: f32,
    pub input_db: [f32; SPECTRUM_BINS],
    pub output_db: [f32; SPECTRUM_BINS],
    pub input_peak_db: f32,
    pub output_peak_db: f32,
    pub input_rms_db: f32,
    pub output_rms_db: f32,
    pub waveform: [WaveformPeak; WAVEFORM_POINTS],
    pub analyzed_seconds: f32,
    pub resonances: Vec<Resonance>,
}

impl SpectrumSnapshot {
    pub fn frequency(&self, bin: usize) -> f32 {
        bin.min(SPECTRUM_BINS - 1) as f32 * self.sample_rate / FFT_SIZE as f32
    }
}
impl Default for SpectrumSnapshot {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            input_db: [FLOOR; SPECTRUM_BINS],
            output_db: [FLOOR; SPECTRUM_BINS],
            input_peak_db: FLOOR,
            output_peak_db: FLOOR,
            input_rms_db: FLOOR,
            output_rms_db: FLOOR,
            waveform: [WaveformPeak::default(); WAVEFORM_POINTS],
            analyzed_seconds: 0.0,
            resonances: Vec::new(),
        }
    }
}

/// Control-thread FFT, smooth spectra and conservative resonance candidates.
pub struct SpectrumAnalyzer {
    snapshot: SpectrumSnapshot,
    key: u64,
    sequence: u64,
    total_frames: u64,
    re: [f32; FFT_SIZE],
    im: [f32; FFT_SIZE],
    window: [f32; FFT_SIZE],
    input_power: [f32; SPECTRUM_BINS],
    output_power: [f32; SPECTRUM_BINS],
    persistence: [f32; SPECTRUM_BINS],
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}
impl SpectrumAnalyzer {
    pub fn new() -> Self {
        Self {
            snapshot: SpectrumSnapshot::default(),
            key: 0,
            sequence: 0,
            total_frames: 0,
            re: [0.0; FFT_SIZE],
            im: [0.0; FFT_SIZE],
            window: std::array::from_fn(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos()
            }),
            input_power: [0.0; SPECTRUM_BINS],
            output_power: [0.0; SPECTRUM_BINS],
            persistence: [0.0; SPECTRUM_BINS],
        }
    }
    pub fn reset(&mut self) {
        self.snapshot = SpectrumSnapshot::default();
        self.key = 0;
        self.sequence = 0;
        self.total_frames = 0;
        self.input_power.fill(0.0);
        self.output_power.fill(0.0);
        self.persistence.fill(0.0);
    }
    pub fn snapshot(&self) -> &SpectrumSnapshot {
        &self.snapshot
    }

    pub fn update(&mut self, frame: &AnalysisFrame) -> bool {
        if !frame.sample_rate.is_finite() || frame.sample_rate < 1000.0 || frame.valid_frames == 0 {
            return false;
        }
        if self.key != frame.key
            || self.snapshot.sample_rate != frame.sample_rate
            || frame.total_frames < self.total_frames
        {
            self.reset();
            self.key = frame.key;
            self.snapshot.sample_rate = frame.sample_rate;
        }
        if self.sequence == frame.sequence {
            return false;
        }
        self.sequence = frame.sequence;
        let new_frames = frame
            .total_frames
            .saturating_sub(self.total_frames)
            .min(FFT_SIZE as u64) as usize;
        self.total_frames = frame.total_frames;
        let dt = new_frames as f32 / frame.sample_rate;
        let (ip, ir) = levels(&frame.input_left, &frame.input_right, frame.valid_frames);
        let (op, or) = levels(&frame.output_left, &frame.output_right, frame.valid_frames);
        self.snapshot.input_peak_db = amplitude_db(ip);
        self.snapshot.output_peak_db = amplitude_db(op);
        self.snapshot.input_rms_db = amplitude_db(ir);
        self.snapshot.output_rms_db = amplitude_db(or);
        let start = FFT_SIZE - new_frames.min(FFT_SIZE);
        let mut wave = WaveformPeak::default();
        for (&l, &r) in frame.output_left[start..]
            .iter()
            .zip(&frame.output_right[start..])
        {
            wave.min = wave.min.min(l).min(r);
            wave.max = wave.max.max(l).max(r);
        }
        self.snapshot.waveform.rotate_left(1);
        self.snapshot.waveform[WAVEFORM_POINTS - 1] = wave;
        if frame.valid_frames < FFT_SIZE {
            return true;
        }
        let input = stereo_power(
            &frame.input_left,
            &frame.input_right,
            &self.window,
            &mut self.re,
            &mut self.im,
        );
        let output = stereo_power(
            &frame.output_left,
            &frame.output_right,
            &self.window,
            &mut self.re,
            &mut self.im,
        );
        // Time-based smoothing behaves the same at 30 and 144 display Hz.
        let alpha = 1.0 - (-dt / 0.15).exp();
        for i in 0..SPECTRUM_BINS {
            self.input_power[i] += (input[i] - self.input_power[i]) * alpha;
            self.output_power[i] += (output[i] - self.output_power[i]) * alpha;
            self.snapshot.input_db[i] = power_db(self.input_power[i]);
            self.snapshot.output_db[i] = power_db(self.output_power[i]);
        }
        if ir > 1e-5 {
            self.snapshot.analyzed_seconds += dt;
        }
        self.find_resonances(dt, ir);
        true
    }

    fn find_resonances(&mut self, dt: f32, rms: f32) {
        self.snapshot.resonances.clear();
        for i in 3..SPECTRUM_BINS - 3 {
            let freq = self.snapshot.frequency(i);
            let db = self.snapshot.input_db[i];
            let radius = (i / 5).clamp(5, 80);
            let lo = i.saturating_sub(radius).max(1);
            let hi = (i + radius).min(SPECTRUM_BINS - 1);
            let mut sum = 0.0;
            let mut n = 0;
            for j in lo..=hi {
                if i.abs_diff(j) > 2 {
                    sum += self.input_power[j];
                    n += 1;
                }
            }
            let baseline = power_db(sum / n.max(1) as f32);
            let prominence = db - baseline;
            let candidate = rms > 1e-5
                && (60.0..=16_000.0).contains(&freq)
                && db > -60.0
                && prominence > 9.0
                && db > self.snapshot.input_db[i - 1]
                && db >= self.snapshot.input_db[i + 1];
            if candidate {
                self.persistence[i] += dt;
            } else {
                self.persistence[i] = (self.persistence[i] - dt * 2.0).max(0.0);
            }
            if candidate && self.persistence[i] >= 0.75 && self.snapshot.analyzed_seconds >= 0.75 {
                self.snapshot.resonances.push(Resonance {
                    frequency_hz: freq,
                    q: (freq / (self.snapshot.sample_rate / FFT_SIZE as f32 * 3.0))
                        .clamp(1.5, 10.0),
                    cut_db: -(prominence * 0.3).clamp(1.5, 6.0),
                    prominence_db: prominence,
                });
            }
        }
        self.snapshot
            .resonances
            .sort_by(|a, b| b.prominence_db.total_cmp(&a.prominence_db));
        // Keep widely spaced candidates instead of neighboring FFT-bin copies.
        let mut kept: Vec<Resonance> = Vec::with_capacity(3);
        for r in &self.snapshot.resonances {
            if kept
                .iter()
                .all(|k| (k.frequency_hz / r.frequency_hz).log2().abs() > 0.16)
            {
                kept.push(*r);
            }
            if kept.len() == 3 {
                break;
            }
        }
        self.snapshot.resonances = kept;
    }
}

fn levels(l: &[f32; FFT_SIZE], r: &[f32; FFT_SIZE], valid: usize) -> (f32, f32) {
    let valid = valid.min(FFT_SIZE);
    let mut peak = 0.0f32;
    let mut sum = 0.0f64;
    for i in FFT_SIZE - valid..FFT_SIZE {
        peak = peak.max(l[i].abs()).max(r[i].abs());
        sum += f64::from(l[i]).powi(2) + f64::from(r[i]).powi(2);
    }
    (peak, (sum / (2 * valid).max(1) as f64).sqrt() as f32)
}
fn amplitude_db(v: f32) -> f32 {
    (20.0 * v.max(1e-6).log10()).max(FLOOR)
}
fn power_db(v: f32) -> f32 {
    (10.0 * v.max(1e-12).log10()).max(FLOOR)
}

fn stereo_power(
    l: &[f32; FFT_SIZE],
    r: &[f32; FFT_SIZE],
    window: &[f32; FFT_SIZE],
    re: &mut [f32; FFT_SIZE],
    im: &mut [f32; FFT_SIZE],
) -> [f32; SPECTRUM_BINS] {
    let mut out = [0.0; SPECTRUM_BINS];
    let coherent = window.iter().sum::<f32>();
    for channel in [l, r] {
        for i in 0..FFT_SIZE {
            re[i] = channel[i] * window[i];
            im[i] = 0.0;
        }
        fft(re, im);
        for i in 0..SPECTRUM_BINS {
            let scale = if i == 0 || i == SPECTRUM_BINS - 1 {
                1.0
            } else {
                2.0
            } / coherent;
            out[i] += (re[i] * re[i] + im[i] * im[i]) * scale * scale * 0.5;
        }
    }
    out
}

fn fft(re: &mut [f32; FFT_SIZE], im: &mut [f32; FFT_SIZE]) {
    let bits = FFT_SIZE.trailing_zeros();
    for i in 0..FFT_SIZE {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= FFT_SIZE {
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        let (wi, wr) = angle.sin_cos();
        for start in (0..FFT_SIZE).step_by(len) {
            let (mut tr, mut ti) = (1.0, 0.0);
            for j in 0..len / 2 {
                let a = start + j;
                let b = a + len / 2;
                let br = re[b] * tr - im[b] * ti;
                let bi = re[b] * ti + im[b] * tr;
                re[b] = re[a] - br;
                im[b] = im[a] - bi;
                re[a] += br;
                im[a] += bi;
                (tr, ti) = (tr * wr - ti * wi, tr * wi + ti * wr);
            }
        }
        len *= 2;
    }
}
