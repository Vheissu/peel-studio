//! Shared parts of the newer drum models: the `Acoustic` switch on Kick,
//! Snare, Hat and Tom, and the `Circuit` switch for their drum-machine
//! voices.
//!
//! An acoustic hit is a short force pulse from a beater or stick driving a
//! bank of decaying modes (drum heads, shells, cymbal plates). The pulse is a
//! raised cosine with unit area, so the lowest modes start at the same
//! amplitude whatever the contact time, while a shorter contact (a harder
//! hit or a harder beater) reaches further up the spectrum. Each mode is a
//! complex one-pole resonator: it starts from zero, its frequency can move
//! without a click, and its magnitude is its current amplitude, which the
//! snare uses to drive its wires.
//!
//! Every acoustic hit draws a few small deterministic offsets (strike
//! position, contact time, level) from the instrument's hit counter, so
//! repeated hits differ the way real strokes do but the same event stream
//! always renders the same audio. Onset times are never moved.
//!
//! The circuit voices reuse the same resonators as the bridged-T circuits of
//! analogue drum machines, and saturate through [`Adaa`], a tanh with
//! first-order antiderivative anti-aliasing.

use peel_dsp::envelope::t60_to_coef;
use peel_dsp::osc::fast_sin;

use crate::common::{Pool, PoolVoice};

/// Picks the slot for a circuit-style retrigger. A voice still ringing on
/// the same key (any key when `any_key`) that is not fading out carries on
/// into the new slot, so resonators and oscillators keep their state and
/// the caller only re-excites them. Returns the slot and whether it was
/// carried over.
pub(crate) fn analog_slot<V: PoolVoice + Copy>(
    pool: &mut Pool<V>,
    key: u8,
    id: u32,
    any_key: bool,
    fading: impl Fn(&V) -> bool,
) -> (usize, bool) {
    let old = (0..pool.voices.len()).find(|&i| {
        pool.voices[i].is_active()
            && !fading(&pool.voices[i])
            && (any_key || pool.alloc.slot(i).key == key)
    });
    let slot = pool.begin(key, id);
    match old {
        Some(o) => {
            if o != slot {
                pool.voices[slot] = pool.voices[o];
                pool.voices[o].kill();
            }
            (slot, true)
        }
        None => (slot, false),
    }
}

/// `ln(cosh(x))`, the antiderivative of `tanh`, without overflow.
#[inline]
fn ln_cosh(x: f32) -> f32 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f32::consts::LN_2
}

/// `tanh` saturation with first-order antiderivative anti-aliasing: the
/// output is the average of `tanh` over the segment between the previous
/// and current input, which removes most of the harmonics a plain
/// waveshaper folds back below Nyquist. It delays by half a sample.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Adaa {
    x1: f32,
    f1: f32,
}

impl Adaa {
    pub(crate) const fn new() -> Adaa {
        Adaa { x1: 0.0, f1: 0.0 }
    }

    pub(crate) fn reset(&mut self) {
        self.x1 = 0.0;
        self.f1 = 0.0;
    }

    #[inline]
    pub(crate) fn tick(&mut self, x: f32) -> f32 {
        let f = ln_cosh(x);
        let d = x - self.x1;
        let y = if d.abs() > 1.0e-3 {
            (f - self.f1) / d
        } else {
            (0.5 * (x + self.x1)).tanh()
        };
        self.x1 = x;
        self.f1 = f;
        y
    }
}

/// One decaying mode: `z <- p z + x` with `p = r e^(i w)`; the output is
/// `Im z`, so an impulse rings as a sine that starts at zero.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mode {
    re: f32,
    im: f32,
    c: f32,
    s: f32,
    r: f32,
}

impl Mode {
    pub(crate) const fn new() -> Mode {
        Mode {
            re: 0.0,
            im: 0.0,
            c: 0.0,
            s: 0.0,
            r: 0.0,
        }
    }

    /// Silences the mode and sets its frequency and 60 dB decay time.
    pub(crate) fn start(&mut self, hz: f32, t60: f32, sample_rate: f32) {
        self.re = 0.0;
        self.im = 0.0;
        self.r = t60_to_coef(t60, sample_rate);
        self.tune(hz, 1.0 / sample_rate);
    }

    /// Changes the decay time, keeping phase and amplitude; takes effect at
    /// the next [`Mode::tune`].
    pub(crate) fn retarget(&mut self, t60: f32, sample_rate: f32) {
        self.r = t60_to_coef(t60, sample_rate);
    }

    /// Moves the frequency, keeping phase, amplitude and decay.
    #[inline]
    pub(crate) fn tune(&mut self, hz: f32, sr_inv: f32) {
        let w = std::f32::consts::TAU * (hz * sr_inv).clamp(0.0, 0.45);
        let (s, c) = w.sin_cos();
        self.c = self.r * c;
        self.s = self.r * s;
    }

    #[inline]
    pub(crate) fn tick(&mut self, x: f32) -> f32 {
        let re = self.c * self.re - self.s * self.im + x;
        let im = self.s * self.re + self.c * self.im;
        self.re = re;
        self.im = im;
        im
    }

    /// Current amplitude.
    #[inline]
    pub(crate) fn amp(&self) -> f32 {
        (self.re * self.re + self.im * self.im).sqrt()
    }

    /// The mode's phase as a unit sine (`Im z / |z|`), 0 when silent.
    #[inline]
    pub(crate) fn unit(&self) -> f32 {
        let a = self.amp();
        if a > 1.0e-12 { self.im / a } else { 0.0 }
    }

    /// Clears denormal-sized state; call at the control rate.
    #[inline]
    pub(crate) fn flush(&mut self) {
        if self.re.abs() + self.im.abs() < 1.0e-20 {
            self.re = 0.0;
            self.im = 0.0;
        }
    }
}

/// Raised-cosine force pulse with unit area: the contact of a beater or
/// stick. Its spectrum is flat below about `0.5 / contact` and falls away
/// above `2 / contact`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pulse {
    n: u32,
    len: u32,
    inc: f32,
    g: f32,
}

impl Pulse {
    pub(crate) const fn new() -> Pulse {
        Pulse {
            n: 0,
            len: 0,
            inc: 0.0,
            g: 0.0,
        }
    }

    pub(crate) fn start(&mut self, contact: f32, sample_rate: f32) {
        let len = (contact * sample_rate).round().max(2.0) as u32;
        self.n = 0;
        self.len = len;
        self.inc = 1.0 / len as f32;
        self.g = 1.0 / len as f32;
    }

    #[inline]
    pub(crate) fn tick(&mut self) -> f32 {
        if self.n >= self.len {
            return 0.0;
        }
        let t = (self.n as f32 + 0.5) * self.inc;
        self.n += 1;
        (1.0 - fast_sin(t + 0.25)) * self.g
    }

    #[inline]
    pub(crate) fn busy(&self) -> bool {
        self.n < self.len
    }
}

/// Small deterministic random numbers for one hit (splitmix32).
pub(crate) struct HitRng(u32);

impl HitRng {
    pub(crate) fn new(hit: u32, salt: u32) -> HitRng {
        HitRng(hit.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B))
    }

    /// Uniform in `[-1, 1)`.
    pub(crate) fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9);
        let mut z = self.0;
        z = (z ^ (z >> 16)).wrapping_mul(0x21F0_AAAD);
        z = (z ^ (z >> 15)).wrapping_mul(0x735A_2D97);
        z ^= z >> 15;
        (z >> 8) as f32 * (2.0 / 16_777_216.0) - 1.0
    }
}

/// Early-reflection taps for a small, damped drum room: (milliseconds,
/// left gain, right gain).
const ROOM_TAPS: [(f32, f32, f32); 8] = [
    (4.1, 0.0, 0.42),
    (6.3, 0.46, 0.0),
    (11.7, 0.0, 0.33),
    (14.9, 0.35, 0.0),
    (21.3, 0.18, 0.24),
    (26.9, 0.26, 0.15),
    (34.1, 0.12, 0.17),
    (41.3, 0.14, 0.1),
];

/// A short stereo set of early reflections from a mono drum, with the high
/// end damped. The buffer is allocated in `prepare`; `tail` says how long
/// the reflections still sound after the input stops.
pub(crate) struct Room {
    buf: Vec<f32>,
    pos: usize,
    taps: [(usize, f32, f32); 8],
    damp_l: f32,
    damp_r: f32,
    a: f32,
    quiet: usize,
}

impl Room {
    pub(crate) fn new() -> Room {
        Room {
            buf: Vec::new(),
            pos: 0,
            taps: [(1, 0.0, 0.0); 8],
            damp_l: 0.0,
            damp_r: 0.0,
            a: 0.5,
            quiet: 1,
        }
    }

    pub(crate) fn prepare(&mut self, sample_rate: f32) {
        let len = (0.045 * sample_rate) as usize + 2;
        self.buf = vec![0.0; len];
        self.pos = 0;
        for (t, &(ms, l, r)) in self.taps.iter_mut().zip(ROOM_TAPS.iter()) {
            *t = (
                ((ms * 0.001 * sample_rate) as usize).clamp(1, len - 1),
                l,
                r,
            );
        }
        self.a = 1.0 - (-std::f32::consts::TAU * 5_500.0 / sample_rate).exp();
        self.clear();
    }

    pub(crate) fn clear(&mut self) {
        self.buf.fill(0.0);
        self.damp_l = 0.0;
        self.damp_r = 0.0;
        self.quiet = self.buf.len();
    }

    /// True while reflections of earlier input are still sounding.
    pub(crate) fn ringing(&self) -> bool {
        self.quiet < self.buf.len()
    }

    #[inline]
    pub(crate) fn tick(&mut self, x: f32) -> (f32, f32) {
        let len = self.buf.len();
        if len == 0 {
            return (0.0, 0.0);
        }
        self.buf[self.pos] = x;
        self.quiet = if x == 0.0 {
            self.quiet.saturating_add(1)
        } else {
            0
        };
        let (mut l, mut r) = (0.0, 0.0);
        for &(d, gl, gr) in &self.taps {
            let i = if self.pos >= d {
                self.pos - d
            } else {
                self.pos + len - d
            };
            let s = self.buf[i];
            l += s * gl;
            r += s * gr;
        }
        self.pos += 1;
        if self.pos == len {
            self.pos = 0;
        }
        self.damp_l += self.a * (l - self.damp_l);
        self.damp_r += self.a * (r - self.damp_r);
        if self.quiet >= len || self.damp_l.abs() < 1.0e-20 {
            self.damp_l = 0.0;
        }
        if self.quiet >= len || self.damp_r.abs() < 1.0e-20 {
            self.damp_r = 0.0;
        }
        (self.damp_l, self.damp_r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_has_unit_area_at_any_length() {
        for contact in [0.000_05, 0.000_3, 0.002] {
            let mut p = Pulse::new();
            p.start(contact, 48_000.0);
            let mut sum = 0.0;
            while p.busy() {
                sum += p.tick();
            }
            assert!((sum - 1.0).abs() < 1e-4, "{contact}: {sum}");
        }
    }

    #[test]
    fn mode_rings_at_its_frequency_and_decays() {
        let sr = 48_000.0;
        let mut m = Mode::new();
        m.start(1_000.0, 0.1, sr);
        let first = m.tick(1.0);
        assert_eq!(first, 0.0);
        let mut crossings = 0;
        let mut prev = 0.0;
        for _ in 0..4_800 {
            let y = m.tick(0.0);
            if prev < 0.0 && y >= 0.0 {
                crossings += 1;
            }
            prev = y;
        }
        assert!((99..=101).contains(&crossings), "{crossings}");
        // 100 ms is the 60 dB time.
        assert!((m.amp() - 1.0e-3).abs() < 2.0e-4, "{}", m.amp());
    }

    #[test]
    fn adaa_tracks_tanh_for_slow_signals() {
        let mut a = Adaa::new();
        let mut worst = 0.0f32;
        for n in 0..4_800 {
            let x = 3.0 * (std::f32::consts::TAU * 50.0 * n as f32 / 48_000.0).sin();
            let y = a.tick(x);
            if n > 0 {
                // Compare with tanh half a sample back.
                let xm = 3.0 * (std::f32::consts::TAU * 50.0 * (n as f32 - 0.5) / 48_000.0).sin();
                worst = worst.max((y - xm.tanh()).abs());
            }
        }
        assert!(worst < 2e-3, "{worst}");
    }

    #[test]
    fn hit_rng_is_deterministic_and_bounded() {
        let mut a = HitRng::new(7, 3);
        let mut b = HitRng::new(7, 3);
        let mut c = HitRng::new(8, 3);
        let mut differs = false;
        for _ in 0..1_000 {
            let x = a.next();
            assert_eq!(x, b.next());
            assert!((-1.0..1.0).contains(&x));
            differs |= x != c.next();
        }
        assert!(differs);
    }
}
