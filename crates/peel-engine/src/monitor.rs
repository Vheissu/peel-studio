//! Monitoring signals that sit outside the project mix: the metronome click
//! and the sample preview voice.
//!
//! Both are summed into the output after the master track. They do not pass
//! through the master's insert effects, gain or meters, and they are absent
//! from offline renders unless monitoring is explicitly enabled.

use std::sync::Arc;

use peel_dsp::SampleBuffer;

use crate::command::Retired;
use crate::mixer::slew;
use crate::retire::Retirer;

const CLICK_VOICES: usize = 4;
const CLICK_SECONDS: f32 = 0.03;

#[derive(Clone, Copy)]
struct ClickVoice {
    /// 0 = idle, 1 = normal, 2 = accent.
    kind: u8,
    /// Frames since the click started; negative until it begins inside the
    /// current block.
    pos: i32,
}

/// Synthesized click. The two waveforms are rendered once at creation.
pub(crate) struct Metronome {
    pub enabled: bool,
    normal: Vec<f32>,
    accent: Vec<f32>,
    voices: [ClickVoice; CLICK_VOICES],
}

impl Metronome {
    pub fn new(sample_rate: f32) -> Metronome {
        let len = ((CLICK_SECONDS * sample_rate) as usize).max(8);
        let make = |hz: f32, amp: f32| -> Vec<f32> {
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (-t / 0.006).exp();
                    // Short linear attack avoids a hard first sample.
                    let attack = (i as f32 / 8.0).min(1.0);
                    amp * env * attack * (std::f32::consts::TAU * hz * t).sin()
                })
                .collect()
        };
        Metronome {
            enabled: false,
            normal: make(1000.0, 0.35),
            accent: make(1600.0, 0.5),
            voices: [ClickVoice { kind: 0, pos: 0 }; CLICK_VOICES],
        }
    }

    /// Starts a click `offset` frames into the current block.
    pub fn trigger(&mut self, accent: bool, offset: u32) {
        let new = ClickVoice {
            kind: if accent { 2 } else { 1 },
            pos: -(offset.min(i32::MAX as u32) as i32),
        };
        if let Some(v) = self.voices.iter_mut().find(|v| v.kind == 0) {
            *v = new;
            return;
        }
        // All busy: steal the oldest.
        if let Some(v) = self.voices.iter_mut().max_by_key(|v| v.pos) {
            *v = new;
        }
    }

    pub fn clear(&mut self) {
        for v in &mut self.voices {
            v.kind = 0;
        }
    }

    pub fn mix_into(&mut self, l: &mut [f32], r: &mut [f32]) {
        let frames = l.len().min(r.len());
        for v in &mut self.voices {
            if v.kind == 0 {
                continue;
            }
            let table = if v.kind == 2 {
                &self.accent
            } else {
                &self.normal
            };
            let len = table.len() as i32;
            let mut pos = v.pos;
            for i in 0..frames {
                if pos >= len {
                    break;
                }
                if pos >= 0 {
                    let s = table[pos as usize];
                    l[i] += s;
                    r[i] += s;
                }
                pos += 1;
            }
            if pos >= len {
                v.kind = 0;
            } else {
                v.pos = pos;
            }
        }
    }
}

const PREVIEW_FADE_SECONDS: f32 = 0.004;

struct PreviewVoice {
    sample: Option<Arc<SampleBuffer>>,
    /// Read position in source frames.
    pos: f64,
    /// Source frames per output frame.
    step: f64,
    gain: f32,
    env: f32,
    fading_out: bool,
}

impl PreviewVoice {
    const EMPTY: PreviewVoice = PreviewVoice {
        sample: None,
        pos: 0.0,
        step: 1.0,
        gain: 1.0,
        env: 0.0,
        fading_out: false,
    };
}

/// Audition voice. A replaced voice fades out over a few milliseconds while
/// the new one fades in, so auditioning never clicks.
pub(crate) struct Preview {
    sample_rate: f32,
    fade_step: f32,
    current: PreviewVoice,
    fading: PreviewVoice,
}

impl Preview {
    pub fn new(sample_rate: f32) -> Preview {
        Preview {
            sample_rate,
            fade_step: 1.0 / (PREVIEW_FADE_SECONDS * sample_rate).max(1.0),
            current: PreviewVoice::EMPTY,
            fading: PreviewVoice::EMPTY,
        }
    }

    pub fn is_active(&self) -> bool {
        self.current.sample.is_some() || self.fading.sample.is_some()
    }

    /// Starts (or stops, with `None`) the preview.
    pub fn start(&mut self, sample: Option<Arc<SampleBuffer>>, gain: f32, retirer: &mut Retirer) {
        // The outgoing voice moves to the fade slot; whatever was fading is
        // cut and returned to the control thread.
        if self.current.sample.is_some() {
            if let Some(old) = self.fading.sample.take() {
                retirer.retire(Retired::Sample(old));
            }
            self.fading = std::mem::replace(&mut self.current, PreviewVoice::EMPTY);
            self.fading.fading_out = true;
        }
        let Some(sample) = sample else {
            return;
        };
        if sample.frames() == 0 || !sample.sample_rate.is_finite() || sample.sample_rate <= 0.0 {
            retirer.retire(Retired::Sample(sample));
            return;
        }
        let step = sample.sample_rate as f64 / self.sample_rate as f64;
        self.current = PreviewVoice {
            sample: Some(sample),
            pos: 0.0,
            step,
            gain: if gain.is_finite() {
                gain.clamp(0.0, 4.0)
            } else {
                1.0
            },
            env: 0.0,
            fading_out: false,
        };
    }

    pub fn mix_into(&mut self, l: &mut [f32], r: &mut [f32], retirer: &mut Retirer) {
        let step = self.fade_step;
        Self::mix_voice(&mut self.current, l, r, step, retirer);
        Self::mix_voice(&mut self.fading, l, r, step, retirer);
    }

    fn mix_voice(
        v: &mut PreviewVoice,
        l: &mut [f32],
        r: &mut [f32],
        fade_step: f32,
        retirer: &mut Retirer,
    ) {
        let Some(sample) = v.sample.as_ref() else {
            return;
        };
        let (sl, sr) = (sample.left(), sample.right());
        let n = sl.len().min(sr.len());
        let frames = l.len().min(r.len());
        let mut done = n == 0;
        if !done {
            let target = if v.fading_out { 0.0 } else { 1.0 };
            for i in 0..frames {
                let idx = v.pos as usize;
                if idx >= n {
                    done = true;
                    break;
                }
                let frac = (v.pos - idx as f64) as f32;
                let next = (idx + 1).min(n - 1);
                let a = sl[idx] + (sl[next] - sl[idx]) * frac;
                let b = sr[idx] + (sr[next] - sr[idx]) * frac;
                v.env = slew(v.env, target, fade_step);
                let g = v.gain * v.env;
                l[i] += a * g;
                r[i] += b * g;
                v.pos += v.step;
                if v.fading_out && v.env == 0.0 {
                    done = true;
                    break;
                }
            }
        }
        if done && let Some(s) = v.sample.take() {
            retirer.retire(Retired::Sample(s));
        }
    }
}
