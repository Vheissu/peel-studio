//! `peel.drum.acoustic`: a recorded acoustic drum kit.
//!
//! Plays multisampled recordings of a real kit (Big Rusty Drums by
//! Karoryfer, CC0) embedded in the binary: kick, snare, closed and open
//! hi-hat, ride, crash, rack tom and floor tom, each with four velocity
//! layers and three alternate takes per layer.
//!
//! ## Samples
//!
//! The files are 48 kHz stereo 16-bit PCM WAVs included with
//! `include_bytes!`. Their headers are parsed once into a process-wide
//! [`Bank`] the first time an instrument is created; the audio stays in
//! the binary's read-only data and every voice reads it in place, so no
//! instance decodes, copies or frees sample data, and nothing on the audio
//! path allocates. Playback uses cubic Hermite interpolation at any host
//! sample rate.
//!
//! ## Velocity
//!
//! Each piece's layers keep their recorded level relationship: the layer
//! levels are measured once (RMS over the first 80 ms of each take) and a
//! hit that lands exactly on a layer's velocity plays that layer at its own
//! level. Between layer centres the hit picks one of the two neighbouring
//! layers, the upper one with a probability that grows towards it, drawn
//! from a deterministic hash of the hit count, and gains it to the level
//! interpolated (in dB) between the two layers. Layers are never summed, so
//! two performances never comb-filter, and loudness never jumps at a layer
//! boundary. Below the softest layer the level falls with the square of the
//! velocity. `Velocity` scales how much of that range is used.
//!
//! ## Alternate takes
//!
//! Every hit moves to a different one of the three takes (never the same
//! take twice in a row), chosen deterministically from the instrument's hit
//! counter, so the same event stream always renders the same audio. Onsets
//! are never moved.
//!
//! ## Kit behaviour
//!
//! - Re-striking a piece damps its own previous hits (kick 0.12 s, snare
//!   0.25 s, toms 0.35 s, closed hat 0.08 s, open hat 0.5 s to -60 dB), the
//!   way a beater or stick contacting a ringing head does. Fast double-kick
//!   keeps clear attacks and the low end stays bounded, while a lone hit
//!   rings out in full.
//! - Ride and crash hits overlap freely, capped at 6 and 4 sounding hits;
//!   the oldest one beyond the cap fades over 0.3 s.
//! - A closed or pedal hat chokes ringing open hats over 70 ms. Hats never
//!   choke the ride or crash.
//! - Notes are one-shots; note-offs are ignored.
//! - 32 sounding voices; a stolen voice (quietest first) fades over 3 ms.
//!
//! ## Keys
//!
//! `MIDI Kit` maps General MIDI drum keys: 35/36 kick, 38/40 snare, 42/44
//! closed hat, 46 open hat, 49/57 crash, 51/53/59 ride, 50/48/47 rack tom
//! (+2, 0, -2 semitones), 45/43/41 floor tom (+2, 0, -2 semitones). Other
//! keys are silent. A single piece plays its original pitch at C5 (key 60)
//! and transposes with the key like the other drums.
//!
//! ## Expression
//!
//! Per-note pitch bends each recorded voice, pressure raises its level
//! (`1 + 0.6 p`) and opens the Tone filter, and timbre moves the shared
//! ±9 dB treble shelf.

use std::sync::OnceLock;

use peel_dsp::envelope::t60_to_coef;
use peel_dsp::filter::{Svf, SvfMode};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::db_to_gain;
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::acoustic::HitRng;
use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, Tilt,
    key_offset, ramp_coef, st_ratio,
};

const PIECE: ParamId = 1;
const TUNE: ParamId = 2;
const DECAY: ParamId = 3;
const TONE: ParamId = 4;
const GAIN: ParamId = 10;
const VELOCITY: ParamId = 11;
const WIDTH: ParamId = 12;

static PARAMS: [ParamDescriptor; 7] = [
    ParamDescriptor::choice(
        PIECE,
        "kit.piece",
        "Piece",
        "Kit",
        &[
            "MIDI Kit",
            "Kick",
            "Snare",
            "Closed Hat",
            "Open Hat",
            "Ride",
            "Crash",
            "Rack Tom",
            "Floor Tom",
        ],
        0,
    ),
    ParamDescriptor::linear(
        TUNE,
        "kit.tune",
        "Tune",
        "Kit",
        -12.0,
        12.0,
        0.0,
        Unit::Semitones,
    ),
    ParamDescriptor::linear(
        DECAY,
        "kit.decay",
        "Decay",
        "Kit",
        0.05,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        TONE,
        "kit.tone",
        "Tone",
        "Kit",
        200.0,
        20_000.0,
        20_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        GAIN,
        "out.gain",
        "Gain",
        "Output",
        -24.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        VELOCITY,
        "out.velocity",
        "Velocity",
        "Output",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        WIDTH,
        "out.width",
        "Width",
        "Output",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.drum.acoustic",
    name: "Acoustic Kit",
    description: "Recorded acoustic drum kit with velocity layers and alternate takes",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

/// Every preset lists every parameter, so switching between them is exact.
macro_rules! preset {
    ($name:literal, $piece:literal) => {
        Preset {
            name: $name,
            params: &[
                ("kit.piece", $piece),
                ("kit.tune", 0.0),
                ("kit.decay", 1.0),
                ("kit.tone", 20_000.0),
                ("out.gain", 0.0),
                ("out.velocity", 1.0),
                ("out.width", 1.0),
            ],
        }
    };
}

pub static PRESETS: [Preset; 9] = [
    preset!("Full Kit", 0.0),
    preset!("Kick", 1.0),
    preset!("Snare", 2.0),
    preset!("Closed Hat", 3.0),
    preset!("Open Hat", 4.0),
    preset!("Ride", 5.0),
    preset!("Crash", 6.0),
    preset!("Rack Tom", 7.0),
    preset!("Floor Tom", 8.0),
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, RecordedKit::new()))
}

// ---------------------------------------------------------------------------
// Sample bank
// ---------------------------------------------------------------------------

const KICK: usize = 0;
const SNARE: usize = 1;
const CLOSED: usize = 2;
const OPEN: usize = 3;
const RIDE: usize = 4;
const CRASH: usize = 5;
const RACK: usize = 6;
const FLOOR: usize = 7;

const PIECES: usize = 8;
const LAYERS: usize = 4;
const TAKES: usize = 3;

/// Rate the embedded recordings were written at.
const SOURCE_RATE: f64 = 48_000.0;

macro_rules! take {
    ($piece:literal, $layer:literal, $rr:literal) => {
        include_bytes!(concat!(
            "../../../assets/drums/big-rusty/",
            $piece,
            "-",
            $layer,
            "-",
            $rr,
            ".wav"
        ))
    };
}

macro_rules! piece_files {
    ($piece:literal) => {
        [
            [
                take!($piece, "1", "1"),
                take!($piece, "1", "2"),
                take!($piece, "1", "3"),
            ],
            [
                take!($piece, "2", "1"),
                take!($piece, "2", "2"),
                take!($piece, "2", "3"),
            ],
            [
                take!($piece, "3", "1"),
                take!($piece, "3", "2"),
                take!($piece, "3", "3"),
            ],
            [
                take!($piece, "4", "1"),
                take!($piece, "4", "2"),
                take!($piece, "4", "3"),
            ],
        ]
    };
}

/// The embedded WAV files: piece, velocity layer (soft to hard), take.
static FILES: [[[&[u8]; TAKES]; LAYERS]; PIECES] = [
    piece_files!("kick"),
    piece_files!("snare"),
    piece_files!("closed-hat"),
    piece_files!("open-hat"),
    piece_files!("ride"),
    piece_files!("crash"),
    piece_files!("rack-tom"),
    piece_files!("floor-tom"),
];

/// Level trim per piece, in dB, applied on top of the recorded levels (one
/// gain per articulation; the takes and layers inside a piece keep their
/// recorded relationship).
const PIECE_TRIM_DB: [f32; PIECES] = [0.0; PIECES];

/// Overall level of the kit relative to the recordings at full scale.
const KIT_GAIN_DB: f32 = -5.0;

/// Velocities at which each layer plays at its own recorded level.
const LAYER_VELOCITY: [f32; LAYERS] = [0.22, 0.48, 0.73, 0.95];

/// Time for a piece's earlier hits to fall 60 dB when it is struck again.
const RETRIGGER_T60: [f32; PIECES] = [
    0.12,          // kick
    0.25,          // snare
    0.08,          // closed hat
    0.5,           // open hat
    f32::INFINITY, // ride
    f32::INFINITY, // crash
    0.35,          // rack tom
    0.35,          // floor tom
];

/// Sounding hits per piece before the oldest is faded out.
const MAX_HITS: [usize; PIECES] = [8, 8, 8, 4, 6, 4, 8, 8];
/// Fade for a hit beyond [`MAX_HITS`] (60 dB time).
const CAP_T60: f32 = 0.3;
/// Fade for an open hat closed by the pedal or a closed hit (60 dB time).
const CHOKE_T60: f32 = 0.07;
/// Decay starts after this much of the hit has played untouched.
const DECAY_HOLD_S: f32 = 0.005;
/// Safety fade over the last frames of a recording.
const END_FADE_FRAMES: f64 = 96.0;
/// Level window used to measure a take's loudness.
const LEVEL_WINDOW_S: f64 = 0.08;
/// Fastest playback, in source frames per output sample.
const MAX_STEP: f64 = 8.0;
const SCALE: f32 = 1.0 / 32_768.0;

/// One recording: interleaved stereo 16-bit little-endian frames, borrowed
/// from the binary.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Take {
    data: &'static [u8],
    frames: usize,
}

impl Take {
    const EMPTY: Take = Take {
        data: &[],
        frames: 0,
    };

    /// One stereo frame; silence outside the recording.
    #[inline]
    fn frame(&self, i: isize) -> (f32, f32) {
        if i < 0 || i as usize >= self.frames {
            return (0.0, 0.0);
        }
        let o = i as usize * 4;
        let d = &self.data[o..o + 4];
        (
            i16::from_le_bytes([d[0], d[1]]) as f32 * SCALE,
            i16::from_le_bytes([d[2], d[3]]) as f32 * SCALE,
        )
    }

    /// RMS over the first `window` seconds, both channels.
    fn attack_rms(&self) -> f32 {
        let n = ((LEVEL_WINDOW_S * SOURCE_RATE) as usize).min(self.frames);
        if n == 0 {
            return 0.0;
        }
        let mut sum = 0.0f64;
        for i in 0..n {
            let (l, r) = self.frame(i as isize);
            sum += (l * l + r * r) as f64;
        }
        (sum / (2 * n) as f64).sqrt() as f32
    }
}

/// Finds the PCM data of a 48 kHz stereo 16-bit RIFF WAVE file. These are
/// our own embedded files, so anything else is rejected rather than
/// converted.
pub(crate) fn parse_wav(bytes: &'static [u8]) -> Option<Take> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at =
        |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let mut pos = 12;
    let mut format_ok = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_at(pos + 4) as usize;
        let body = pos + 8;
        let end = body.checked_add(size)?;
        if id == b"fmt " {
            if size < 16 || end > bytes.len() {
                return None;
            }
            let tag = u16_at(body);
            let channels = u16_at(body + 2);
            let rate = u32_at(body + 4);
            let align = u16_at(body + 12);
            let bits = u16_at(body + 14);
            format_ok = tag == 1 && channels == 2 && rate == 48_000 && align == 4 && bits == 16;
            if !format_ok {
                return None;
            }
        } else if id == b"data" {
            if !format_ok {
                return None;
            }
            // Tolerate a data size that overstates the file.
            let end = end.min(bytes.len());
            let data = &bytes[body..end];
            let frames = data.len() / 4;
            return Some(Take {
                data: &data[..frames * 4],
                frames,
            });
        }
        // Chunks are padded to an even size.
        pos = end + (size & 1);
    }
    None
}

struct PieceBank {
    takes: [[Take; TAKES]; LAYERS],
    /// Measured layer level in dB (mean power over the takes).
    level_db: [f32; LAYERS],
    /// The levels made strictly increasing, used as velocity targets.
    target_db: [f32; LAYERS],
    /// Longest take, in seconds.
    len_s: f32,
}

/// Every recording, parsed once per process.
pub(crate) struct Bank {
    pieces: [PieceBank; PIECES],
}

impl Bank {
    fn load() -> Bank {
        let pieces = std::array::from_fn(|p| {
            let mut takes = [[Take::EMPTY; TAKES]; LAYERS];
            let mut level_db = [0.0f32; LAYERS];
            let mut len = 0usize;
            for (l, row) in takes.iter_mut().enumerate() {
                let mut power = 0.0f32;
                for (t, take) in row.iter_mut().enumerate() {
                    *take = parse_wav(FILES[p][l][t]).unwrap_or(Take::EMPTY);
                    len = len.max(take.frames);
                    let rms = take.attack_rms();
                    power += rms * rms / TAKES as f32;
                }
                level_db[l] = 10.0 * power.max(1.0e-12).log10();
            }
            let mut target_db = level_db;
            for l in 1..LAYERS {
                target_db[l] = target_db[l].max(target_db[l - 1] + 0.5);
            }
            PieceBank {
                takes,
                level_db,
                target_db,
                len_s: (len as f64 / SOURCE_RATE) as f32,
            }
        });
        Bank { pieces }
    }
}

static BANK: OnceLock<Bank> = OnceLock::new();

/// The shared bank, parsing the embedded files on first use. Called when an
/// instrument is created, never from the audio path.
pub(crate) fn bank() -> &'static Bank {
    BANK.get_or_init(Bank::load)
}

/// Level in dB for `velocity` on `piece` at full velocity sensitivity:
/// the layer levels at their velocities, interpolated in dB between them,
/// falling with the square of velocity below the softest layer.
fn velocity_db(pb: &PieceBank, velocity: f32) -> f32 {
    let v = velocity;
    let c = LAYER_VELOCITY;
    let t = pb.target_db;
    if v <= c[0] {
        return t[0] + 40.0 * (v.max(1.0e-4) / c[0]).log10();
    }
    if v >= c[LAYERS - 1] {
        return t[LAYERS - 1] + 20.0 * (v / c[LAYERS - 1]).log10();
    }
    for l in 0..LAYERS - 1 {
        if v <= c[l + 1] {
            let f = (v - c[l]) / (c[l + 1] - c[l]);
            return t[l] + (t[l + 1] - t[l]) * f;
        }
    }
    t[LAYERS - 1]
}

/// The lower layer for `velocity` and how far it is towards the next one.
fn layer_position(velocity: f32) -> (usize, f32) {
    let c = LAYER_VELOCITY;
    if velocity <= c[0] {
        return (0, 0.0);
    }
    for l in 0..LAYERS - 1 {
        if velocity < c[l + 1] {
            return (l, (velocity - c[l]) / (c[l + 1] - c[l]));
        }
    }
    (LAYERS - 1, 0.0)
}

/// Piece and transpose for a key in the MIDI kit, or `None` for a key the
/// kit does not use.
fn midi_kit(key: u8) -> Option<(usize, f32)> {
    Some(match key {
        35 | 36 => (KICK, 0.0),
        38 | 40 => (SNARE, 0.0),
        42 | 44 => (CLOSED, 0.0),
        46 => (OPEN, 0.0),
        49 | 57 => (CRASH, 0.0),
        51 | 53 | 59 => (RIDE, 0.0),
        50 => (RACK, 2.0),
        48 => (RACK, 0.0),
        47 => (RACK, -2.0),
        45 => (FLOOR, 2.0),
        43 => (FLOOR, 0.0),
        41 => (FLOOR, -2.0),
        _ => return None,
    })
}

/// Extra decay time for the Decay control: none at 100 %, shorter below.
fn decay_t60(len_s: f32, decay: f32) -> f32 {
    if decay >= 0.9995 {
        return f32::INFINITY;
    }
    (len_s * decay * decay / (1.0 - decay)).max(0.03)
}

#[inline]
fn coef(t60: f32, sample_rate: f32) -> f32 {
    if t60.is_finite() {
        t60_to_coef(t60, sample_rate)
    } else {
        1.0
    }
}

/// Cubic Hermite (Catmull-Rom) interpolation between `x1` and `x2`.
#[inline]
fn hermite(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x2 - x0);
    let c2 = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
    let c3 = 0.5 * (x3 - x0) + 1.5 * (x1 - x2);
    ((c3 * t + c2) * t + c1) * t + x1
}

// ---------------------------------------------------------------------------
// Voices
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct HitVoice {
    active: bool,
    take: Take,
    piece: usize,
    key: u8,
    id: u32,
    /// Start order, for the per-piece cap.
    seq: u32,
    pos: f64,
    /// Source frames per output sample before bending, and with it.
    base_step: f64,
    step: f64,
    applied: f32,
    /// Level from velocity, layer match and trims.
    gain: f32,
    /// Pressure gain.
    xgain: f32,
    /// Damping from re-strikes, chokes and the hit cap.
    damp: f32,
    damp_k: f32,
    /// Decay control envelope.
    decay: f32,
    decay_k: f32,
    hold: u32,
    fade: Fade,
    /// Peak follower for quietest-voice stealing.
    follow: f32,
    filter_l: Svf,
    filter_r: Svf,
    tilt: Tilt,
}

impl HitVoice {
    fn new() -> HitVoice {
        HitVoice {
            active: false,
            take: Take::EMPTY,
            piece: 0,
            key: 60,
            id: 0,
            seq: 0,
            pos: 0.0,
            base_step: 1.0,
            step: 1.0,
            applied: 1.0,
            gain: 0.0,
            xgain: 1.0,
            damp: 1.0,
            damp_k: 1.0,
            decay: 1.0,
            decay_k: 1.0,
            hold: 0,
            fade: Fade::new(),
            follow: 0.0,
            filter_l: Svf::new(),
            filter_r: Svf::new(),
            tilt: Tilt::new(),
        }
    }

    fn retune(&mut self, ratio: f32) {
        if ratio == self.applied {
            return;
        }
        self.applied = ratio;
        self.step = (self.base_step * ratio as f64).min(MAX_STEP);
    }

    /// Starts (or speeds up) an exponential fade with the given 60 dB time.
    fn damp_with(&mut self, k: f32) {
        self.damp_k = self.damp_k.min(k);
    }

    #[inline]
    fn tick(&mut self, filter_on: bool) -> (f32, f32) {
        let i = self.pos.floor();
        let t = (self.pos - i) as f32;
        let i = i as isize;
        let (mut l, mut r) = if t == 0.0 {
            self.take.frame(i)
        } else {
            let (a0, b0) = self.take.frame(i - 1);
            let (a1, b1) = self.take.frame(i);
            let (a2, b2) = self.take.frame(i + 1);
            let (a3, b3) = self.take.frame(i + 2);
            (hermite(a0, a1, a2, a3, t), hermite(b0, b1, b2, b3, t))
        };
        self.pos += self.step;
        let frames = self.take.frames as f64;
        let end = ((frames - self.pos) / END_FADE_FRAMES).clamp(0.0, 1.0) as f32;
        if self.pos >= frames {
            self.active = false;
        }
        if filter_on {
            l = self.filter_l.process(l, SvfMode::LowPass);
            r = self.filter_r.process(r, SvfMode::LowPass);
        }
        (l, r) = self.tilt.tick(l, r);
        self.damp *= self.damp_k;
        if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.decay *= self.decay_k;
        }
        let env = self.damp * self.decay;
        if env < 1.0e-5 {
            self.active = false;
        }
        let g = self.gain * self.xgain * env * end * self.fade.tick();
        if self.fade.is_done() {
            self.active = false;
        }
        let (l, r) = (l * g, r * g);
        let a = l.abs().max(r.abs());
        self.follow = if a > self.follow {
            a
        } else {
            self.follow * 0.9995
        };
        (l, r)
    }
}

impl PoolVoice for HitVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.follow
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

// ---------------------------------------------------------------------------
// Instrument
// ---------------------------------------------------------------------------

pub(crate) struct RecordedKit {
    bank: &'static Bank,
    expr: Expression,
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    pool: Pool<HitVoice>,
    /// Hits started since the last reset; seeds layer and take choice.
    hits: u32,
    last_take: [usize; PIECES],
    seq: u32,
    // Settings.
    piece: usize,
    tune: f32,
    decay: f32,
    decay_dirty: bool,
    vel_sens: f32,
    tone: Ramp,
    gain: Ramp,
    width: Ramp,
    // Control-rate values.
    filter_on: bool,
    tone_hz: f32,
    out_gain: f32,
    out_width: f32,
}

impl RecordedKit {
    pub(crate) fn new() -> RecordedKit {
        RecordedKit {
            bank: bank(),
            expr: Expression::new(),
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            pool: Pool::new(40, 32, StealPolicy::Quietest, 48_000.0, HitVoice::new),
            hits: 0,
            last_take: [0; PIECES],
            seq: 0,
            piece: 0,
            tune: 0.0,
            decay: 1.0,
            decay_dirty: false,
            vel_sens: 1.0,
            tone: Ramp::new(20_000.0f32.ln()),
            gain: Ramp::new(0.0),
            width: Ramp::new(1.0),
            filter_on: false,
            tone_hz: 20_000.0,
            out_gain: 1.0,
            out_width: 1.0,
        }
    }

    fn derive(&mut self) {
        self.tone_hz = self.tone.get().exp();
        self.filter_on = self.tone_hz < 19_500.0;
        self.out_gain = db_to_gain(self.gain.get() + KIT_GAIN_DB);
        self.out_width = self.width.get();
    }

    fn update_decay(&mut self) {
        let (sr, decay, bank) = (self.sr, self.decay, self.bank);
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.decay_k = coef(decay_t60(bank.pieces[v.piece].len_s, decay), sr);
        }
        self.decay_dirty = false;
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.tone.tick(k);
        self.gain.tick(k);
        self.width.tick(k);
        self.derive();
        if self.decay_dirty {
            self.update_decay();
        }
        self.expr.tick();
        let (filter_on, tone, sr) = (self.filter_on, self.tone_hz, self.sr);
        let ex = &self.expr;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            let x = ex.voice(v.id, v.key);
            v.xgain = x.gain();
            v.retune(x.ratio);
            v.tilt.set(&x);
            if filter_on {
                let c = tone * (x.octaves() * std::f32::consts::LN_2).exp();
                v.filter_l.set(c.min(0.45 * sr), 0.0, sr);
                v.filter_r.copy_coeffs(&v.filter_l);
            }
        }
        self.pool.sync();
    }

    /// Damps every sounding hit of `piece` that is not already fading out.
    fn damp_piece(&mut self, piece: usize, t60: f32) {
        if !t60.is_finite() {
            return;
        }
        let k = coef(t60, self.sr);
        for v in self.pool.voices.iter_mut() {
            if v.active && v.piece == piece {
                v.damp_with(k);
            }
        }
    }

    /// Keeps a piece below its hit cap by fading its oldest undamped hits.
    fn cap_piece(&mut self, piece: usize) {
        let k = coef(CAP_T60, self.sr);
        loop {
            let mut count = 0;
            let mut oldest: Option<usize> = None;
            for (i, v) in self.pool.voices.iter().enumerate() {
                if v.active && v.piece == piece && v.damp_k >= 1.0 && !v.fade.is_fading() {
                    count += 1;
                    if oldest
                        .is_none_or(|o| v.seq.wrapping_sub(self.pool.voices[o].seq) > u32::MAX / 2)
                    {
                        oldest = Some(i);
                    }
                }
            }
            match oldest {
                Some(o) if count >= MAX_HITS[piece] => self.pool.voices[o].damp_with(k),
                _ => break,
            }
        }
    }
}

impl Synth for RecordedKit {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(40, 32, StealPolicy::Quietest, sample_rate, HitVoice::new);
        for v in &mut self.pool.voices {
            v.tilt.prepare(sample_rate);
        }
        self.ctrl = 0;
        self.hits = 0;
        self.last_take = [0; PIECES];
        self.seq = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
        self.hits = 0;
        self.last_take = [0; PIECES];
        self.seq = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            PIECE => self.piece = (v.max(0.0) as usize).min(PIECES),
            TUNE => self.tune = v,
            DECAY => {
                self.decay = v;
                self.decay_dirty = true;
            }
            TONE => self.tone.set(v.ln(), imm),
            GAIN => self.gain.set(v, imm),
            VELOCITY => self.vel_sens = v,
            WIDTH => self.width.set(v, imm),
            _ => {}
        }
        if imm {
            self.derive();
            if self.decay_dirty {
                self.update_decay();
            }
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let (piece, transpose) = if self.piece == 0 {
            match midi_kit(key) {
                Some(m) => m,
                None => return,
            }
        } else {
            (self.piece - 1, key_offset(key))
        };
        let bank = self.bank;
        let pb = &bank.pieces[piece];

        // Level: the natural curve, narrowed towards the hardest layer's
        // level as velocity sensitivity drops.
        let natural = velocity_db(pb, velocity);
        let top = pb.target_db[LAYERS - 1];
        let level_db = top + self.vel_sens * (natural - top);
        if level_db < top - 90.0 || velocity <= 0.0 {
            return;
        }

        self.hits = self.hits.wrapping_add(1);
        let mut rng = HitRng::new(self.hits, 0x5EED_0000 + piece as u32);
        let (lo, frac) = layer_position(velocity);
        let pick = 0.5 * (rng.next() + 1.0);
        let layer = if lo + 1 < LAYERS && frac > pick {
            lo + 1
        } else {
            lo
        };
        let take = (self.last_take[piece] + 1 + usize::from(rng.next() >= 0.0)) % TAKES;
        self.last_take[piece] = take;
        let sample = pb.takes[layer][take];
        if sample.frames < 4 {
            return;
        }
        let gain = db_to_gain(level_db - pb.level_db[layer] + PIECE_TRIM_DB[piece]);

        // The kit reacts before the new hit sounds.
        self.damp_piece(piece, RETRIGGER_T60[piece]);
        if piece == CLOSED {
            self.damp_piece(OPEN, CHOKE_T60);
        }
        self.cap_piece(piece);

        let semis = self.tune + transpose;
        let base_step = (SOURCE_RATE / self.sr as f64) * st_ratio(semis) as f64;
        let x = self.expr.voice(id, key);
        let decay_k = coef(decay_t60(pb.len_s, self.decay), self.sr);
        let hold = (DECAY_HOLD_S * self.sr) as u32;
        let (filter_on, tone, sr) = (self.filter_on, self.tone_hz, self.sr);
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;

        let slot = self.pool.begin(key, id);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.take = sample;
        v.piece = piece;
        v.key = key;
        v.id = id;
        v.seq = seq;
        v.pos = 0.0;
        v.base_step = base_step.min(MAX_STEP);
        v.step = v.base_step;
        v.applied = 1.0;
        v.retune(x.ratio);
        v.gain = gain;
        v.xgain = x.gain();
        v.damp = 1.0;
        v.damp_k = 1.0;
        v.decay = 1.0;
        v.decay_k = decay_k;
        v.hold = hold;
        v.fade.reset();
        v.follow = gain;
        v.tilt.reset();
        v.tilt.set(&x);
        v.filter_l.reset();
        v.filter_r.reset();
        if filter_on {
            let c = tone * (x.octaves() * std::f32::consts::LN_2).exp();
            v.filter_l.set(c.min(0.45 * sr), 0.0, sr);
            v.filter_r.copy_coeffs(&v.filter_l);
        }
    }

    // One-shots: a hit always plays out.
    fn note_off(&mut self, _key: u8, _velocity: f32, _id: u32) {}

    fn all_notes_off(&mut self) {}

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        for n in 0..frames {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let (mut l, mut r) = (0.0, 0.0);
            let filter_on = self.filter_on;
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick(filter_on);
                l += a;
                r += b;
            }
            let m = 0.5 * (l + r);
            let s = 0.5 * (l - r) * self.out_width;
            left[n] = (m + s) * self.out_gain;
            right[n] = (m - s) * self.out_gain;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(channels: u16, rate: u32, bits: u16, frames: usize, extra: bool) -> &'static [u8] {
        let align = channels * bits / 8;
        let data_len = frames * align as usize;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WAVE");
        if extra {
            // An odd-sized chunk before fmt, padded to even.
            b.extend_from_slice(b"LIST");
            b.extend_from_slice(&3u32.to_le_bytes());
            b.extend_from_slice(&[1, 2, 3, 0]);
        }
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * align as u32).to_le_bytes());
        b.extend_from_slice(&align.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data_len as u32).to_le_bytes());
        for i in 0..frames {
            for c in 0..channels {
                let v = (i as i32) * if c == 0 { 1 } else { -1 };
                b.extend_from_slice(&v.to_le_bytes()[..(bits / 8) as usize]);
            }
        }
        let len = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&len.to_le_bytes());
        Box::leak(b.into_boxed_slice())
    }

    #[test]
    fn parses_our_wav_format() {
        for extra in [false, true] {
            let t = parse_wav(wav(2, 48_000, 16, 10, extra)).expect("parses");
            assert_eq!(t.frames, 10);
            let (l, r) = t.frame(3);
            assert_eq!(l, 3.0 * SCALE);
            assert_eq!(r, -3.0 * SCALE);
            assert_eq!(t.frame(-1), (0.0, 0.0));
            assert_eq!(t.frame(10), (0.0, 0.0));
        }
    }

    #[test]
    fn rejects_other_formats() {
        assert!(parse_wav(wav(1, 48_000, 16, 10, false)).is_none());
        assert!(parse_wav(wav(2, 44_100, 16, 10, false)).is_none());
        assert!(parse_wav(wav(2, 48_000, 24, 10, false)).is_none());
        assert!(parse_wav(b"RIFF\0\0\0\0WAVE").is_none());
        assert!(parse_wav(b"nope").is_none());
    }

    #[test]
    fn velocity_curve_is_continuous_and_rising() {
        let pb = PieceBank {
            takes: [[Take::EMPTY; TAKES]; LAYERS],
            level_db: [-30.0, -22.0, -15.0, -9.0],
            target_db: [-30.0, -22.0, -15.0, -9.0],
            len_s: 1.0,
        };
        let mut prev = f32::NEG_INFINITY;
        for i in 1..=1_000 {
            let v = i as f32 / 1_000.0;
            let db = velocity_db(&pb, v);
            assert!(db > prev, "{v}: {db} after {prev}");
            // No step anywhere a player can reach (the curve is a log below
            // the softest layer, steep only near zero).
            assert!(db - prev < 1.0 || v < 0.05, "{v}: jump {}", db - prev);
            prev = db;
        }
        for (l, &c) in LAYER_VELOCITY.iter().enumerate() {
            assert!((velocity_db(&pb, c) - pb.target_db[l]).abs() < 1e-4);
        }
    }

    #[test]
    fn midi_kit_covers_the_general_midi_keys() {
        for k in [
            35, 36, 38, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 53, 57, 59,
        ] {
            assert!(midi_kit(k).is_some(), "{k}");
        }
        for k in [0, 34, 37, 39, 52, 54, 55, 56, 58, 60, 127] {
            assert!(midi_kit(k).is_none(), "{k}");
        }
    }
}
