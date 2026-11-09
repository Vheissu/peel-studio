//! `peel.guitar.recorded`, `peel.guitar.palm_muted` and `peel.bass.recorded`:
//! recorded electric guitar, palm-muted guitar and bass.
//!
//! The legacy guitar and bass play DI recordings embedded in the binary (Karoryfer
//! Black And Green Guitars, green guitar, and Black And Blue Basses, blue
//! solid-body bass played with a pick; both CC0). Every note plays its real
//! MIDI pitch from the nearest recorded note.
//!
//! ## Samples
//!
//! The files are 48 kHz mono 16-bit PCM WAVs included with `include_bytes!`.
//! Their headers are parsed once into a process-wide bank the first time an
//! instrument is created; the audio stays in the binary's read-only data and
//! every voice reads it in place, so no instance decodes, copies or frees
//! sample data, and nothing on the audio path allocates or locks.
//!
//! - Guitar sustain: 16 notes from E2 (key 40) to C#6 (key 85) every three
//!   semitones, three velocity layers, two takes each.
//! - Guitar muted: the same 16 notes recorded staccato, one layer, two takes.
//!   They are short, separately recorded notes; the Mute control damps and
//!   darkens them further for palm-muted chugs.
//! - Guitar release: the sound of each string being stopped, two takes,
//!   played at note-off of a sustained note (`Fret Noise`).
//! - Palm-muted guitar (`peel.guitar.palm_muted`, its own instrument):
//!   genuinely palm-muted DI notes from Guitar-TECHS (P2, an EVH Wolfgang on
//!   the neck pickup, CC BY 4.0), one layer, chromatic across the neck with
//!   as many separate strikes per note as were recorded (one for some low
//!   notes). `palm_assets.rs` lists them; any number of notes and takes per
//!   note is accepted. This instrument plays only these recordings, never the
//!   sustain, muted or fret-noise sets, and adds no damping or darkening:
//!   velocity follows the sustain curve but only scales the level. Takes
//!   rotate deterministically and never repeat back to back on a note that
//!   has more than one.
//! - Bass: 14 notes from B0 (key 23) to D4 (key 62) every three semitones,
//!   two velocity layers, three takes each.
//!
//! ## Playback
//!
//! A note plays the nearest recorded note, resampled with cubic Hermite
//! interpolation at any host sample rate (at most 1.5 semitones away inside
//! the range). Keys up to an octave past either end transpose the end note;
//! keys further out are silent. Pitch bends are capped at 8x speed (three
//! octaves up at 48 kHz); fret noise keeps the pitch it started at.
//!
//! A held note plays the recording's natural sustain and decay, faded a
//! further 60 dB over the recording's last second so even a High Gain chord
//! rings out instead of stopping when the recording ends; note-off
//! damps it with `Release` (time to -60 dB), so short notes and fast muted
//! riffs stop clearly. Re-striking a key damps that key's earlier note over
//! 60 ms, as a pick on a ringing string does. The bass is monophonic by
//! default (`Mono`): a new note damps the previous one over 40 ms. A
//! note-off only releases the note it belongs to.
//!
//! ## Velocity and takes
//!
//! Each layer keeps its recorded tone. Layer levels are measured once (RMS
//! over the first 80 ms) and averaged across the neck into a velocity curve;
//! a note between layer centres picks one of the two neighbouring layers
//! (the upper one more often towards it) and is gained to the curve, so
//! loudness rises smoothly and layers are never summed. Below the softest
//! layer the level falls with the square of velocity. Takes alternate
//! deterministically (guitar: never the same take twice in a row on a note;
//! bass: one of the two other takes), so the same events always render the
//! same audio and onsets never move.
//!
//! ## Guitar amp
//!
//! The guitar runs through the modelled guitar's 4x oversampled preamp,
//! tone stack, power stage and cabinet (`guitar::Amp`), with its pickup and
//! cable filters bypassed because the recordings already carry the real
//! pickup's response. `Output` crossfades between the amp and the DI signal
//! over about 10 ms.
//!
//! ## Bass tone
//!
//! The bass has no guitar amp (its high-pass would remove B0 and E1). `Drive`
//! adds the saturation of only the band above 220 Hz (a 4th-order split),
//! from a first-order antiderivative anti-aliased soft clipper, so the fundamentals stay clean. Its make-up
//! holds a typical picked attack in that band at the same level at any
//! Drive, so turning Drive up adds grit and sustain rather than dulling
//! the band. `Tone` is a 12 dB/octave low-pass.
//!
//! ## Expression
//!
//! Per-note pitch bends each voice, pressure raises its level (`1 + 0.6 p`)
//! and opens its mute filter, and timbre moves the shared ±9 dB treble shelf.

use std::sync::OnceLock;

use peel_dsp::biquad::Coeffs;
use peel_dsp::envelope::t60_to_coef;
use peel_dsp::filter::{Svf, SvfMode};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::db_to_gain;
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::acoustic::HitRng;
use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, Tilt, lerp,
    ramp_coef, st_ratio,
};
use crate::guitar::Amp;

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

const ARTICULATION: ParamId = 1;
const MUTE: ParamId = 2;
const RELEASE: ParamId = 3;
const FRET_NOISE: ParamId = 4;
const MONO: ParamId = 5;
const OUTPUT: ParamId = 10;
const AMP_GAIN: ParamId = 11;
const AMP_BASS: ParamId = 12;
const AMP_MID: ParamId = 13;
const AMP_TREBLE: ParamId = 14;
const CAB_TYPE: ParamId = 15;
const LEVEL: ParamId = 20;
const VELOCITY: ParamId = 21;
const DRIVE: ParamId = 30;
const TONE: ParamId = 31;

const ARTICULATIONS: &[&str] = &["Sustain", "Muted"];
const OUTPUTS: &[&str] = &["Amp", "Direct"];
const CABS: &[&str] = &["1x12 Open", "2x12", "4x12 Closed"];
const OFF_ON: &[&str] = &["Off", "On"];

const fn mute_param() -> ParamDescriptor {
    ParamDescriptor::linear(
        MUTE,
        "play.mute",
        "Mute",
        "Performance",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    )
}

const fn level_param() -> ParamDescriptor {
    ParamDescriptor::linear(
        LEVEL,
        "out.level",
        "Level",
        "Output",
        -24.0,
        12.0,
        0.0,
        Unit::Decibels,
    )
}

const fn velocity_param(default: f32) -> ParamDescriptor {
    ParamDescriptor::linear(
        VELOCITY,
        "out.velocity",
        "Velocity",
        "Output",
        0.0,
        1.0,
        default,
        Unit::Percent,
    )
}

const fn release_param(default: f32) -> ParamDescriptor {
    ParamDescriptor::log(
        RELEASE,
        "play.release",
        "Release",
        "Performance",
        0.03,
        3.0,
        default,
        Unit::Seconds,
    )
}

static GUITAR_PARAMS: [ParamDescriptor; 12] = [
    ParamDescriptor::choice(
        ARTICULATION,
        "play.articulation",
        "Articulation",
        "Performance",
        ARTICULATIONS,
        0,
    ),
    mute_param(),
    release_param(0.15),
    ParamDescriptor::linear(
        FRET_NOISE,
        "play.fret_noise",
        "Fret Noise",
        "Performance",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::choice(OUTPUT, "amp.output", "Output", "Amp", OUTPUTS, 0),
    ParamDescriptor::linear(
        AMP_GAIN,
        "amp.gain",
        "Gain",
        "Amp",
        0.0,
        1.0,
        0.12,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        AMP_BASS,
        "amp.bass",
        "Bass",
        "Amp",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        AMP_MID,
        "amp.mid",
        "Mid",
        "Amp",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        AMP_TREBLE,
        "amp.treble",
        "Treble",
        "Amp",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::choice(CAB_TYPE, "cab.type", "Cabinet", "Amp", CABS, 0),
    level_param(),
    velocity_param(0.8),
];

static BASS_PARAMS: [ParamDescriptor; 7] = [
    ParamDescriptor::choice(MONO, "play.mono", "Mono", "Performance", OFF_ON, 1),
    mute_param(),
    release_param(0.12),
    ParamDescriptor::linear(
        DRIVE,
        "tone.drive",
        "Drive",
        "Tone",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        TONE,
        "tone.cutoff",
        "Tone",
        "Tone",
        400.0,
        20_000.0,
        20_000.0,
        Unit::Hertz,
    ),
    level_param(),
    velocity_param(0.8),
];

static PALM_PARAMS: [ParamDescriptor; 9] = [
    release_param(0.025),
    ParamDescriptor::choice(OUTPUT, "amp.output", "Output", "Amp", OUTPUTS, 0),
    ParamDescriptor::linear(
        AMP_GAIN,
        "amp.gain",
        "Gain",
        "Amp",
        0.0,
        1.0,
        0.85,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        AMP_BASS,
        "amp.bass",
        "Bass",
        "Amp",
        -12.0,
        12.0,
        2.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        AMP_MID,
        "amp.mid",
        "Mid",
        "Amp",
        -12.0,
        12.0,
        -1.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        AMP_TREBLE,
        "amp.treble",
        "Treble",
        "Amp",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::choice(CAB_TYPE, "cab.type", "Cabinet", "Amp", CABS, 2),
    level_param(),
    velocity_param(0.6),
];

pub static GUITAR_DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.guitar.recorded",
    name: "Recorded Guitar",
    description: "Recorded electric guitar: sustained and muted notes through an amp and cabinet",
    category: "Guitar",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &GUITAR_PARAMS,
};

pub static PALM_DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.guitar.palm_muted",
    name: "Palm-Muted Guitar",
    description: "Recorded palm-muted electric guitar through an amp and cabinet",
    category: "Guitar",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PALM_PARAMS,
};

pub static BASS_DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.bass.recorded",
    name: "Recorded Bass",
    description: "Recorded picked electric bass with velocity layers and alternate takes",
    category: "Guitar",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &BASS_PARAMS,
};

/// Every guitar preset lists every parameter, so switching between them is
/// exact.
macro_rules! guitar_preset {
    (
        $name:literal, art: $art:literal, mute: $mute:literal, release: $rel:literal,
        fret: $fret:literal, output: $out:literal, gain: $gain:literal, bass: $bass:literal,
        mid: $mid:literal, treble: $treble:literal, cab: $cab:literal, level: $level:literal,
        velocity: $vel:literal
    ) => {
        Preset {
            name: $name,
            params: &[
                ("play.articulation", $art),
                ("play.mute", $mute),
                ("play.release", $rel),
                ("play.fret_noise", $fret),
                ("amp.output", $out),
                ("amp.gain", $gain),
                ("amp.bass", $bass),
                ("amp.mid", $mid),
                ("amp.treble", $treble),
                ("cab.type", $cab),
                ("out.level", $level),
                ("out.velocity", $vel),
            ],
        }
    };
}

pub static GUITAR_PRESETS: [Preset; 5] = [
    guitar_preset!("Clean", art: 0.0, mute: 0.0, release: 0.15, fret: 0.5, output: 0.0,
        gain: 0.12, bass: 0.0, mid: 0.0, treble: 0.0, cab: 0.0, level: 0.0, velocity: 0.8),
    guitar_preset!("High Gain", art: 0.0, mute: 0.0, release: 0.12, fret: 0.4, output: 0.0,
        gain: 0.9, bass: 1.0, mid: -2.0, treble: 1.0, cab: 2.0, level: 0.0, velocity: 0.6),
    guitar_preset!("Palm Mute", art: 1.0, mute: 0.45, release: 0.08, fret: 0.0, output: 0.0,
        gain: 0.85, bass: 2.0, mid: -1.0, treble: 0.0, cab: 2.0, level: 0.0, velocity: 0.6),
    guitar_preset!("Crunch", art: 0.0, mute: 0.0, release: 0.15, fret: 0.5, output: 0.0,
        gain: 0.5, bass: 1.0, mid: 2.0, treble: 1.0, cab: 1.0, level: 0.0, velocity: 0.7),
    guitar_preset!("Direct", art: 0.0, mute: 0.0, release: 0.15, fret: 0.5, output: 1.0,
        gain: 0.12, bass: 0.0, mid: 0.0, treble: 0.0, cab: 0.0, level: 0.0, velocity: 0.8),
];

macro_rules! palm_preset {
    (
        $name:literal, release: $rel:literal, output: $out:literal, gain: $gain:literal,
        bass: $bass:literal, mid: $mid:literal, treble: $treble:literal, cab: $cab:literal,
        level: $level:literal, velocity: $vel:literal
    ) => {
        Preset {
            name: $name,
            params: &[
                ("play.release", $rel),
                ("amp.output", $out),
                ("amp.gain", $gain),
                ("amp.bass", $bass),
                ("amp.mid", $mid),
                ("amp.treble", $treble),
                ("cab.type", $cab),
                ("out.level", $level),
                ("out.velocity", $vel),
            ],
        }
    };
}

pub static PALM_PRESETS: [Preset; 3] = [
    palm_preset!("Metal Chugs", release: 0.025, output: 0.0, gain: 0.85, bass: 2.0, mid: -1.0,
        treble: 0.0, cab: 2.0, level: 0.0, velocity: 0.6),
    palm_preset!("Clean Palm Mute", release: 0.08, output: 0.0, gain: 0.12, bass: 0.0, mid: 0.0,
        treble: 0.0, cab: 0.0, level: 0.0, velocity: 0.8),
    palm_preset!("Direct", release: 0.08, output: 1.0, gain: 0.12, bass: 0.0, mid: 0.0,
        treble: 0.0, cab: 0.0, level: 0.0, velocity: 0.8),
];

macro_rules! bass_preset {
    (
        $name:literal, mono: $mono:literal, mute: $mute:literal, release: $rel:literal,
        drive: $drive:literal, tone: $tone:literal, level: $level:literal, velocity: $vel:literal
    ) => {
        Preset {
            name: $name,
            params: &[
                ("play.mono", $mono),
                ("play.mute", $mute),
                ("play.release", $rel),
                ("tone.drive", $drive),
                ("tone.cutoff", $tone),
                ("out.level", $level),
                ("out.velocity", $vel),
            ],
        }
    };
}

pub static BASS_PRESETS: [Preset; 3] = [
    bass_preset!("Picked", mono: 1.0, mute: 0.0, release: 0.12, drive: 0.0, tone: 20_000.0,
        level: 0.0, velocity: 0.8),
    bass_preset!("Rock Pick", mono: 1.0, mute: 0.0, release: 0.1, drive: 0.35, tone: 7_000.0,
        level: 0.0, velocity: 0.7),
    bass_preset!("Muted", mono: 1.0, mute: 0.5, release: 0.08, drive: 0.2, tone: 4_000.0,
        level: 0.0, velocity: 0.7),
];

pub(crate) fn create_guitar() -> Box<dyn Device> {
    Box::new(Instrument::new(
        &GUITAR_DESC,
        Strings::new(Kind::Guitar, false),
    ))
}

pub(crate) fn create_palm_guitar() -> Box<dyn Device> {
    Box::new(Instrument::new(
        &PALM_DESC,
        Strings::new(Kind::Guitar, true),
    ))
}

pub(crate) fn create_bass() -> Box<dyn Device> {
    Box::new(Instrument::new(&BASS_DESC, Strings::new(Kind::Bass, false)))
}

// ---------------------------------------------------------------------------
// Sample bank
// ---------------------------------------------------------------------------

/// Rate the embedded recordings were written at.
const SOURCE_RATE: f64 = 48_000.0;
const SCALE: f32 = 1.0 / 32_768.0;

const G_ROOTS: [u8; 16] = [
    40, 43, 46, 49, 52, 55, 58, 61, 64, 67, 70, 73, 76, 79, 82, 85,
];
const B_ROOTS: [u8; 14] = [23, 26, 29, 32, 35, 38, 41, 44, 47, 50, 53, 56, 59, 62];
const G_LAYERS: usize = 3;
const G_TAKES: usize = 2;
const B_LAYERS: usize = 2;
const B_TAKES: usize = 3;

macro_rules! wav {
    ($kind:literal, $root:literal, $layer:literal, $rr:literal) => {
        include_bytes!(concat!(
            "../../../assets/strings/recorded/",
            $kind,
            "-",
            $root,
            "-",
            $layer,
            "-",
            $rr,
            ".wav"
        ))
    };
}

macro_rules! g_sustain {
    ($($root:literal),*) => {
        [$([
            [wav!("guitar-sustain", $root, "1", "1"), wav!("guitar-sustain", $root, "1", "2")],
            [wav!("guitar-sustain", $root, "2", "1"), wav!("guitar-sustain", $root, "2", "2")],
            [wav!("guitar-sustain", $root, "3", "1"), wav!("guitar-sustain", $root, "3", "2")],
        ]),*]
    };
}

macro_rules! g_single {
    ($kind:literal; $($root:literal),*) => {
        [$([[wav!($kind, $root, "1", "1"), wav!($kind, $root, "1", "2")]]),*]
    };
}

macro_rules! b_pick {
    ($($root:literal),*) => {
        [$([
            [
                wav!("bass-pick", $root, "1", "1"),
                wav!("bass-pick", $root, "1", "2"),
                wav!("bass-pick", $root, "1", "3"),
            ],
            [
                wav!("bass-pick", $root, "2", "1"),
                wav!("bass-pick", $root, "2", "2"),
                wav!("bass-pick", $root, "2", "3"),
            ],
        ]),*]
    };
}

type Files<const L: usize, const T: usize, const R: usize> = [[[&'static [u8]; T]; L]; R];

static G_SUSTAIN_FILES: Files<G_LAYERS, G_TAKES, 16> = g_sustain!(
    "40", "43", "46", "49", "52", "55", "58", "61", "64", "67", "70", "73", "76", "79", "82", "85"
);
static G_MUTE_FILES: Files<1, G_TAKES, 16> = g_single!("guitar-mute";
    "40", "43", "46", "49", "52", "55", "58", "61", "64", "67", "70", "73", "76", "79", "82", "85");
static G_RELEASE_FILES: Files<1, G_TAKES, 16> = g_single!("guitar-release";
    "40", "43", "46", "49", "52", "55", "58", "61", "64", "67", "70", "73", "76", "79", "82", "85");
static B_PICK_FILES: Files<B_LAYERS, B_TAKES, 14> = b_pick!(
    "23", "26", "29", "32", "35", "38", "41", "44", "47", "50", "53", "56", "59", "62"
);

// Palm-muted recordings: `PALM_FILES`, a list of (root key, takes), written
// by the asset preparation script.
include!("palm_assets.rs");

/// Velocities at which each layer plays at its own (averaged) level.
const G_SUSTAIN_VEL: [f32; G_LAYERS] = [0.35, 0.65, 0.95];
const B_PICK_VEL: [f32; B_LAYERS] = [0.6, 0.95];

/// How much of a recorded note's level difference from the neck average is
/// corrected (1.0 levels every note; 0.0 keeps the recorded differences).
const LEVEL_CORRECTION: f32 = 0.8;

/// Gain from the recordings into the guitar amp: brings the DI recordings
/// to the level the modelled guitar's pickup drives the same amp with, so
/// Gain settings mean the same thing on both.
const GUITAR_IN_DB: f32 = 12.0;
/// The DI path relative to the amp input, roughly loudness matched to the
/// Clean amp.
const DIRECT_DB: f32 = -5.0;
/// Overall level of each instrument.
const GUITAR_DB: f32 = 0.0;
const BASS_DB: f32 = -6.0;
/// Muted notes relative to the sustain curve at the same velocity.
const MUTE_TRIM_DB: f32 = -1.0;
/// Palm-muted notes relative to the sustain curve at the same velocity
/// (measured: their attack then matches the muted set within 0.1 dB).
const PALM_TRIM_DB: f32 = -1.0;
/// Fret noise relative to its recorded level (as recorded next to the
/// sustained notes, the stop is about as loud as a medium attack).
const RELEASE_TRIM_DB: f32 = -10.0;
/// Safety fade over the last frames of a recording.
const END_FADE_FRAMES: f64 = 96.0;
/// Every recording also fades a further 60 dB (linear in dB) over its last
/// 1 s, or its last 15 % if shorter. A tail that ends 60 dB below its peak
/// is still loud after the guitar amp's gain, so without this a long-held
/// High Gain chord would stop dead at the end of the recording.
const TAIL_FADE_S: f64 = 1.0;
const TAIL_FADE_SHARE: f64 = 0.15;
/// Level window used to measure a take's loudness.
const LEVEL_WINDOW_S: f64 = 0.08;
/// Playback speed limits, in source frames per output sample.
const MAX_STEP: f64 = 8.0;
const MIN_STEP: f64 = 1.0 / 64.0;
/// Keys this far past the recorded range still transpose the end note.
const EDGE_SEMIS: i32 = 12;

/// One recording: mono 16-bit little-endian frames, borrowed from the binary.
#[derive(Clone, Copy, Debug)]
struct Take {
    data: &'static [u8],
    frames: usize,
}

impl Take {
    const EMPTY: Take = Take {
        data: &[],
        frames: 0,
    };

    /// One frame; silence outside the recording.
    #[inline]
    fn frame(&self, i: isize) -> f32 {
        if i < 0 || i as usize >= self.frames {
            return 0.0;
        }
        let o = i as usize * 2;
        i16::from_le_bytes([self.data[o], self.data[o + 1]]) as f32 * SCALE
    }

    fn attack_rms(&self) -> f32 {
        let n = ((LEVEL_WINDOW_S * SOURCE_RATE) as usize).min(self.frames);
        if n == 0 {
            return 0.0;
        }
        let sum: f64 = (0..n)
            .map(|i| {
                let x = self.frame(i as isize) as f64;
                x * x
            })
            .sum();
        (sum / n as f64).sqrt() as f32
    }
}

/// Finds the PCM data of a 48 kHz mono 16-bit RIFF WAVE file. These are our
/// own embedded files, so anything else is rejected rather than converted.
fn parse_wav(bytes: &'static [u8]) -> Option<Take> {
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
            format_ok = u16_at(body) == 1
                && u16_at(body + 2) == 1
                && u32_at(body + 4) == 48_000
                && u16_at(body + 12) == 2
                && u16_at(body + 14) == 16;
            if !format_ok {
                return None;
            }
        } else if id == b"data" {
            if !format_ok {
                return None;
            }
            let end = end.min(bytes.len());
            let data = &bytes[body..end];
            let frames = data.len() / 2;
            return Some(Take {
                data: &data[..frames * 2],
                frames,
            });
        }
        pos = end + (size & 1);
    }
    None
}

/// The takes of one recorded note.
struct Zone<const L: usize, const T: usize> {
    takes: [[Take; T]; L],
    /// Measured layer level in dB (mean power over the takes).
    level_db: [f32; L],
}

/// One articulation across the neck.
struct Set<const L: usize, const T: usize, const R: usize> {
    zones: [Zone<L, T>; R],
    /// Layer levels averaged over the notes, made strictly increasing.
    target_db: [f32; L],
}

impl<const L: usize, const T: usize, const R: usize> Set<L, T, R> {
    fn load(files: &'static Files<L, T, R>) -> Set<L, T, R> {
        let zones: [Zone<L, T>; R] = std::array::from_fn(|r| {
            let mut takes = [[Take::EMPTY; T]; L];
            let mut level_db = [-120.0f32; L];
            for (l, row) in takes.iter_mut().enumerate() {
                let mut power = 0.0f32;
                for (t, take) in row.iter_mut().enumerate() {
                    *take = parse_wav(files[r][l][t]).unwrap_or(Take::EMPTY);
                    let rms = take.attack_rms();
                    power += rms * rms / T as f32;
                }
                level_db[l] = 10.0 * power.max(1.0e-12).log10();
            }
            Zone { takes, level_db }
        });
        let mut target_db = [0.0f32; L];
        for (l, t) in target_db.iter_mut().enumerate() {
            *t = zones.iter().map(|z| z.level_db[l]).sum::<f32>() / R as f32;
        }
        for l in 1..L {
            target_db[l] = target_db[l].max(target_db[l - 1] + 0.5);
        }
        Set { zones, target_db }
    }

    /// Gain in dB that brings layer `layer` of zone `zone` onto the curve.
    fn correction_db(&self, zone: usize, layer: usize) -> f32 {
        let z = self.zones[zone].level_db[layer];
        let avg = self.target_db[layer];
        -(LEVEL_CORRECTION * z + (1.0 - LEVEL_CORRECTION) * avg)
    }
}

/// One palm-muted note: its separate strikes, each with the gain in dB that
/// brings it towards the neck average.
struct PalmZone {
    root: u8,
    takes: Vec<Take>,
    correction_db: Vec<f32>,
}

/// The palm-muted notes, sorted by root: however many notes and takes the
/// asset list holds. Built once, off the audio thread.
struct PalmSet {
    zones: Vec<PalmZone>,
}

impl PalmSet {
    fn load(files: &'static [(u8, &'static [&'static [u8]])]) -> PalmSet {
        let mut zones: Vec<PalmZone> = Vec::with_capacity(files.len());
        for &(root, takes) in files {
            let parsed: Vec<Take> = takes
                .iter()
                .filter_map(|b| parse_wav(b))
                .filter(|t| t.frames >= 4)
                .collect();
            if root > 127 || parsed.is_empty() || zones.iter().any(|z| z.root == root) {
                continue;
            }
            // Attack level in dB for now; turned into a correction below.
            let correction_db = parsed
                .iter()
                .map(|t| 20.0 * t.attack_rms().max(1.0e-6).log10())
                .collect();
            zones.push(PalmZone {
                root,
                takes: parsed,
                correction_db,
            });
        }
        let all: Vec<f32> = zones
            .iter()
            .flat_map(|z| z.correction_db.iter().copied())
            .collect();
        let avg = all.iter().sum::<f32>() / all.len().max(1) as f32;
        for z in &mut zones {
            for db in &mut z.correction_db {
                // A weak/noisy recording must not receive unbounded gain.
                // Keep take balancing within 6 dB of the bank's common gain.
                *db = -avg + (LEVEL_CORRECTION * (avg - *db)).clamp(-6.0, 6.0);
            }
        }
        zones.sort_by_key(|z| z.root);
        PalmSet { zones }
    }

    /// The zone for `key` and its distance in semitones: the nearest
    /// recorded root (the upper one on a tie), or `None` for a key more than
    /// [`EDGE_SEMIS`] outside the recorded range.
    fn nearest(&self, key: u8) -> Option<(usize, i32)> {
        let (first, last) = (self.zones.first()?, self.zones.last()?);
        let k = key as i32;
        if k < first.root as i32 - EDGE_SEMIS || k > last.root as i32 + EDGE_SEMIS {
            return None;
        }
        let idx = self.zones.partition_point(|z| (z.root as i32) < k);
        let best = if idx == 0 {
            0
        } else if idx == self.zones.len() {
            idx - 1
        } else {
            let below = k - self.zones[idx - 1].root as i32;
            let above = self.zones[idx].root as i32 - k;
            if below < above { idx - 1 } else { idx }
        };
        Some((best, k - self.zones[best].root as i32))
    }
}

/// Every recording, parsed once per process.
struct Bank {
    g_sustain: Set<G_LAYERS, G_TAKES, 16>,
    g_mute: Set<1, G_TAKES, 16>,
    g_release: Set<1, G_TAKES, 16>,
    g_palm: PalmSet,
    bass: Set<B_LAYERS, B_TAKES, 14>,
}

static BANK: OnceLock<Bank> = OnceLock::new();

/// The shared bank, parsing the embedded files on first use. Called when an
/// instrument is created, never from the audio path.
fn bank() -> &'static Bank {
    BANK.get_or_init(|| Bank {
        g_sustain: Set::load(&G_SUSTAIN_FILES),
        g_mute: Set::load(&G_MUTE_FILES),
        g_release: Set::load(&G_RELEASE_FILES),
        g_palm: PalmSet::load(PALM_FILES),
        bass: Set::load(&B_PICK_FILES),
    })
}

/// Level in dB for `v` at full velocity sensitivity: the layer levels at
/// their velocities, interpolated in dB, falling with the square of velocity
/// below the softest layer.
fn velocity_db(targets: &[f32], centres: &[f32], v: f32) -> f32 {
    let n = targets.len();
    if v <= centres[0] {
        return targets[0] + 40.0 * (v.max(1.0e-4) / centres[0]).log10();
    }
    if v >= centres[n - 1] {
        return targets[n - 1] + 20.0 * (v / centres[n - 1]).log10();
    }
    for l in 0..n - 1 {
        if v <= centres[l + 1] {
            let f = (v - centres[l]) / (centres[l + 1] - centres[l]);
            return targets[l] + (targets[l + 1] - targets[l]) * f;
        }
    }
    targets[n - 1]
}

/// The lower layer for `v` and how far it is towards the next one.
fn layer_position(centres: &[f32], v: f32) -> (usize, f32) {
    let n = centres.len();
    if v <= centres[0] {
        return (0, 0.0);
    }
    for l in 0..n - 1 {
        if v < centres[l + 1] {
            return (l, (v - centres[l]) / (centres[l + 1] - centres[l]));
        }
    }
    (n - 1, 0.0)
}

/// The recorded note to play for `key` and its distance in semitones, or
/// `None` for a key too far outside the range.
fn nearest(roots: &[u8], key: u8) -> Option<(usize, i32)> {
    let k = key as i32;
    let lo = roots[0] as i32;
    let hi = roots[roots.len() - 1] as i32;
    if k < lo - EDGE_SEMIS || k > hi + EDGE_SEMIS {
        return None;
    }
    let step = roots[1] as i32 - lo;
    let idx = ((k - lo + step / 2).div_euclid(step)).clamp(0, roots.len() as i32 - 1) as usize;
    Some((idx, k - roots[idx] as i32))
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
// Behaviour constants
// ---------------------------------------------------------------------------

/// Re-striking a key damps its earlier note to -60 dB in this time.
const RETRIGGER_T60: f32 = 0.06;
/// A new mono bass note damps the previous one in this time.
const MONO_T60: f32 = 0.04;
/// The pick attack plays untouched this long before Mute starts damping.
const MUTE_HOLD_S: f32 = 0.015;
/// Fret noise is skipped for notes released sooner than this.
const RELEASE_MIN_S: f32 = 0.03;
/// Fret noise falls this many dB per second the note was held, down to
/// [`RELEASE_FLOOR_DB`].
const RELEASE_DB_PER_S: f32 = 6.0;
const RELEASE_FLOOR_DB: f32 = -24.0;
/// Seconds the guitar amp and bass filters keep running after the last voice.
const TAIL_S: f32 = 0.12;
/// Voice level below which a note is finished.
const DEAD: f32 = 1.0e-5;
/// Corner of the band the bass Drive saturates.
const DRIVE_SPLIT_HZ: f32 = 220.0;
/// Pre-gain of the bass clipper at full Drive.
const DRIVE_MAX_DB: f32 = 24.0;
/// Upper-band level the Drive make-up holds steady (a picked attack above
/// the split is around this level).
const DRIVE_REF: f32 = 0.15;

/// Decay time (to -60 dB) for the Mute control, infinite at 0.
fn mute_t60(m: f32) -> f32 {
    if m <= 0.0 {
        return f32::INFINITY;
    }
    let r = 1.0 - m;
    0.12 + 3.0 * r * r * r
}

/// Low-pass cutoff for the Mute control and a muted note's velocity tone
/// (octaves darker), infinite when both are zero.
fn mute_cutoff(m: f32, velocity_tone: f32) -> f32 {
    let oct = 3.5 * m + velocity_tone;
    if oct <= 0.0 {
        return f32::INFINITY;
    }
    12_000.0 * (-oct * std::f32::consts::LN_2).exp()
}

// ---------------------------------------------------------------------------
// Voices
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    take: Take,
    key: u8,
    id: u32,
    /// Fret-noise voices ignore note-offs and the Mute control.
    one_shot: bool,
    /// Muted articulation (darker with low velocity).
    muted: bool,
    /// Octaves the velocity darkens a muted note.
    vel_tone: f32,
    pos: f64,
    /// Source frames over which the end of the recording fades out.
    tail_frames: f64,
    base_step: f64,
    step: f64,
    applied: f32,
    gain: f32,
    xgain: f32,
    /// Retrigger and mono damping.
    damp: f32,
    damp_k: f32,
    /// Mute control decay.
    mute: f32,
    mute_k: f32,
    hold: u32,
    released: bool,
    rel: f32,
    rel_k: f32,
    age: u32,
    fade: Fade,
    follow: f32,
    lp_on: bool,
    lp: Svf,
    tilt: Tilt,
}

impl Voice {
    fn new() -> Voice {
        Voice {
            active: false,
            take: Take::EMPTY,
            key: 60,
            id: 0,
            one_shot: false,
            muted: false,
            vel_tone: 0.0,
            pos: 0.0,
            tail_frames: 1.0,
            base_step: 1.0,
            step: 1.0,
            applied: 1.0,
            gain: 0.0,
            xgain: 1.0,
            damp: 1.0,
            damp_k: 1.0,
            mute: 1.0,
            mute_k: 1.0,
            hold: 0,
            released: false,
            rel: 1.0,
            rel_k: 1.0,
            age: 0,
            fade: Fade::new(),
            follow: 0.0,
            lp_on: false,
            lp: Svf::new(),
            tilt: Tilt::new(),
        }
    }

    fn retune(&mut self, ratio: f32) {
        if ratio == self.applied {
            return;
        }
        self.applied = ratio;
        self.step = (self.base_step * ratio as f64).clamp(MIN_STEP, MAX_STEP);
    }

    fn damp_with(&mut self, k: f32) {
        self.damp_k = self.damp_k.min(k);
    }

    /// Current envelope, used to scale fret noise.
    fn envelope(&self) -> f32 {
        self.damp * self.mute * self.rel
    }

    /// Mute decay and filter for the Mute amount `m`; `pressure_oct` opens
    /// the filter.
    fn set_mute(&mut self, m: f32, pressure_oct: f32, sr: f32) {
        if self.one_shot {
            return;
        }
        self.mute_k = coef(mute_t60(m), sr);
        let tone = if self.muted { self.vel_tone } else { 0.0 };
        let cut = mute_cutoff(m, tone) * (pressure_oct * std::f32::consts::LN_2).exp();
        let on = cut < 19_500.0 && cut < 0.45 * sr;
        if on {
            if !self.lp_on {
                self.lp.reset();
            }
            self.lp.set(cut, 0.0, sr);
        }
        self.lp_on = on;
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        let i = self.pos.floor();
        let t = (self.pos - i) as f32;
        let i = i as isize;
        let mut x = if t == 0.0 {
            self.take.frame(i)
        } else {
            hermite(
                self.take.frame(i - 1),
                self.take.frame(i),
                self.take.frame(i + 1),
                self.take.frame(i + 2),
                t,
            )
        };
        self.pos += self.step;
        self.age = self.age.saturating_add(1);
        let frames = self.take.frames as f64;
        let left = frames - self.pos;
        let mut end = (left / END_FADE_FRAMES).clamp(0.0, 1.0) as f32;
        if left < self.tail_frames {
            end *= (-6.907_755 * (1.0 - (left / self.tail_frames).max(0.0)) as f32).exp();
        }
        if self.pos >= frames {
            self.active = false;
        }
        if self.lp_on {
            x = self.lp.process(x, SvfMode::LowPass);
        }
        x = self.tilt.tick_mono(x);
        self.damp *= self.damp_k;
        if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.mute *= self.mute_k;
        }
        if self.released {
            self.rel *= self.rel_k;
        }
        let env = self.damp * self.mute * self.rel;
        if env < DEAD {
            self.active = false;
        }
        let y = x * self.gain * self.xgain * env * end * self.fade.tick();
        if self.fade.is_done() {
            self.active = false;
        }
        let a = y.abs();
        self.follow = if a > self.follow {
            a
        } else {
            self.follow * 0.9995
        };
        y
    }
}

impl PoolVoice for Voice {
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
// Bass drive
// ---------------------------------------------------------------------------

/// Soft clipper `x / sqrt(1 + x^2)` with first-order antiderivative
/// anti-aliasing.
#[derive(Clone, Copy)]
struct Adaa {
    x1: f32,
    f1: f32,
}

impl Adaa {
    const fn new() -> Adaa {
        Adaa { x1: 0.0, f1: 1.0 }
    }

    #[inline]
    fn f(x: f32) -> f32 {
        x / (1.0 + x * x).sqrt()
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let fx = (1.0 + x * x).sqrt();
        let dx = x - self.x1;
        let y = if dx.abs() > 1.0e-3 {
            (fx - self.f1) / dx
        } else {
            Adaa::f(0.5 * (x + self.x1))
        };
        self.x1 = x;
        self.f1 = fx;
        y
    }
}

// ---------------------------------------------------------------------------
// Instrument
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Guitar,
    Bass,
}

pub(crate) struct Strings {
    kind: Kind,
    bank: &'static Bank,
    expr: Expression,
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    pool: Pool<Voice>,
    hits: u32,
    last_take: [usize; 16],
    /// Palm-muted guitar: plays only the palm-muted recordings.
    palm: bool,
    /// Last palm-muted take played on each recorded root key.
    palm_last: [u8; 128],
    tail: usize,
    // Settings.
    muted: bool,
    mono: bool,
    release_s: f32,
    fret: f32,
    vel_sens: f32,
    cab_type: usize,
    mute: Ramp,
    level: Ramp,
    amp_mix: Ramp,
    gain: Ramp,
    bass: Ramp,
    mid: Ramp,
    treble: Ramp,
    drive: Ramp,
    tone: Ramp,
    // Guitar amp.
    amp: Amp,
    amp_running: bool,
    designed: [f32; 4],
    designed_cab: usize,
    // Bass tone.
    split: [Svf; 2],
    adaa: Adaa,
    tone_lp: Svf,
    tone_on: bool,
    drive_pre: f32,
    drive_makeup: f32,
    drive_wet: f32,
    // Control-rate values.
    out_gain: f32,
    in_gain: f32,
    direct_gain: f32,
}

impl Strings {
    fn new(kind: Kind, palm: bool) -> Strings {
        let (cap, poly) = match kind {
            Kind::Guitar => (40, 32),
            Kind::Bass => (16, 12),
        };
        let mut s = Strings {
            kind,
            bank: bank(),
            expr: Expression::new(),
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            pool: Pool::new(cap, poly, StealPolicy::Quietest, 48_000.0, Voice::new),
            hits: 0,
            last_take: [0; 16],
            palm,
            palm_last: [0; 128],
            tail: 0,
            muted: false,
            mono: kind == Kind::Bass,
            release_s: 0.15,
            fret: 0.5,
            vel_sens: 0.8,
            cab_type: 0,
            mute: Ramp::new(0.0),
            level: Ramp::new(0.0),
            amp_mix: Ramp::new(1.0),
            gain: Ramp::new(0.12),
            bass: Ramp::new(0.0),
            mid: Ramp::new(0.0),
            treble: Ramp::new(0.0),
            drive: Ramp::new(0.0),
            tone: Ramp::new(20_000.0f32.ln()),
            amp: Amp::new(),
            amp_running: false,
            designed: [f32::NAN; 4],
            designed_cab: usize::MAX,
            split: [Svf::new(), Svf::new()],
            adaa: Adaa::new(),
            tone_lp: Svf::new(),
            tone_on: false,
            drive_pre: 1.0,
            drive_makeup: 1.0,
            drive_wet: 0.0,
            out_gain: 1.0,
            in_gain: db_to_gain(GUITAR_IN_DB),
            direct_gain: db_to_gain(DIRECT_DB),
        };
        s.prepare_amp();
        s
    }

    fn prepare_amp(&mut self) {
        self.amp.prepare(self.sr);
        // The recordings already carry the pickup and cable response.
        self.amp.pickup.coeffs = Coeffs::IDENTITY;
        self.amp.cable.coeffs = Coeffs::IDENTITY;
        self.amp.pickup_gain = 1.0;
        self.amp.set_sag(0.4);
        self.designed = [f32::NAN; 4];
        self.designed_cab = usize::MAX;
        self.amp_running = false;
    }

    fn reset_tone(&mut self) {
        self.amp.reset();
        self.amp_running = false;
        self.split[0].reset();
        self.split[1].reset();
        self.adaa = Adaa::new();
        self.tone_lp.reset();
    }

    /// Ramps, amp and filter design and per-voice expression, once per
    /// control interval.
    fn control(&mut self) {
        let k = self.ramp_k;
        let before = self.mute.get();
        let mute_moving = self.mute.tick(k) != before;
        self.level.tick(k);
        let base_db = match self.kind {
            Kind::Guitar => GUITAR_DB,
            Kind::Bass => BASS_DB,
        };
        self.out_gain = db_to_gain(self.level.get() + base_db);
        self.expr.tick();
        match self.kind {
            Kind::Guitar => {
                self.amp_mix.tick(k);
                let now = [
                    self.gain.tick(k),
                    self.bass.tick(k),
                    self.mid.tick(k),
                    self.treble.tick(k),
                ];
                if now[0] != self.designed[0] {
                    self.amp.set_gain(now[0]);
                }
                if now != self.designed {
                    self.amp.set_tone(now[1], now[2], now[3]);
                    self.designed = now;
                }
                if self.cab_type != self.designed_cab {
                    self.amp.set_cab_type(self.cab_type, 5_200.0);
                    self.designed_cab = self.cab_type;
                }
            }
            Kind::Bass => {
                self.drive.tick(k);
                self.tone.tick(k);
                self.design_tone();
                self.design_drive();
            }
        }
        let (m, sr) = (self.mute.get(), self.sr);
        let ex = &self.expr;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            let x = ex.voice(v.id, v.key);
            v.xgain = x.gain();
            // Fret noise keeps the pitch it started with.
            if !v.one_shot {
                v.retune(x.ratio);
            }
            v.tilt.set(&x);
            if mute_moving || x.pressure > 0.0 || v.lp_on {
                v.set_mute(m, x.octaves(), sr);
            }
        }
        self.pool.sync();
    }

    /// Drive gain, make-up and blend for the current Drive amount. Make-up
    /// brings an upper-band signal at [`DRIVE_REF`] back to its own level
    /// whatever the Drive, so more Drive adds grit and sustain instead of
    /// turning the band down.
    fn design_drive(&mut self) {
        let d = self.drive.get();
        self.drive_pre = db_to_gain(DRIVE_MAX_DB * d);
        self.drive_makeup = DRIVE_REF / Adaa::f(self.drive_pre * DRIVE_REF);
        self.drive_wet = (4.0 * d).min(1.0);
    }

    fn design_tone(&mut self) {
        let hz = self.tone.get().exp();
        let on = hz < 19_500.0;
        if on {
            if !self.tone_on {
                self.tone_lp.reset();
            }
            self.tone_lp.set(hz.min(0.45 * self.sr), 0.0, self.sr);
        }
        self.tone_on = on;
    }

    /// Damps sounding voices that match `pred` with the given 60 dB time.
    fn damp_where(&mut self, t60: f32, pred: impl Fn(&Voice) -> bool) {
        let k = coef(t60, self.sr);
        for v in self.pool.voices.iter_mut() {
            if v.active && pred(v) {
                v.damp_with(k);
            }
        }
    }

    /// Starts a voice on `take`. Returns the slot.
    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        key: u8,
        id: u32,
        take: Take,
        semis: f32,
        gain: f32,
        one_shot: bool,
        vel_tone: f32,
    ) -> usize {
        let x = self.expr.voice(id, key);
        let base_step = (SOURCE_RATE / self.sr as f64) * st_ratio(semis) as f64;
        let (m, sr) = (self.mute.get(), self.sr);
        // Palm-muted notes count as muted: no fret noise at note-off.
        let muted = (self.muted || self.palm) && self.kind == Kind::Guitar && !one_shot;
        let slot = self.pool.begin(key, id);
        if one_shot {
            // Fret noise belongs to no held note.
            self.pool.alloc.release(slot);
        }
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.take = take;
        v.key = key;
        v.id = id;
        v.one_shot = one_shot;
        v.muted = muted;
        v.vel_tone = vel_tone;
        v.pos = 0.0;
        v.tail_frames = (TAIL_FADE_S * SOURCE_RATE)
            .min(TAIL_FADE_SHARE * take.frames as f64)
            .max(1.0);
        v.base_step = base_step;
        v.step = base_step.clamp(MIN_STEP, MAX_STEP);
        v.applied = 1.0;
        if !one_shot {
            v.retune(x.ratio);
        }
        v.gain = gain;
        v.xgain = x.gain();
        v.damp = 1.0;
        v.damp_k = 1.0;
        v.mute = 1.0;
        v.mute_k = 1.0;
        v.hold = (MUTE_HOLD_S * sr) as u32;
        v.released = false;
        v.rel = 1.0;
        v.rel_k = 1.0;
        v.age = 0;
        v.fade.reset();
        v.follow = gain;
        v.tilt.reset();
        v.tilt.set(&x);
        v.lp_on = false;
        v.lp.reset();
        v.set_mute(m, x.octaves(), sr);
        slot
    }

    fn guitar_note(&mut self, key: u8, velocity: f32, id: u32) {
        if self.palm {
            self.palm_note(key, velocity, id);
            return;
        }
        let Some((zone, semis)) = nearest(&G_ROOTS, key) else {
            return;
        };
        let bank = self.bank;
        self.hits = self.hits.wrapping_add(1);
        let mut rng = HitRng::new(self.hits, 0x6017_0000 + zone as u32);
        let (take, gain_db, vel_tone) = if self.muted {
            // Muted takes were recorded hotter than the sustains; they follow
            // the sustain velocity curve so switching articulation keeps the
            // attack level.
            let sus = &bank.g_sustain.target_db;
            let natural = velocity_db(sus, &G_SUSTAIN_VEL, velocity);
            let top = sus[G_LAYERS - 1];
            let level = top + self.vel_sens * (natural - top);
            let set = &bank.g_mute;
            if level < top - 90.0 {
                return;
            }
            let t = (self.last_take[zone] + 1) % G_TAKES;
            self.last_take[zone] = t;
            // Softer muted picking is darker as well as quieter.
            let tone = 1.5 * (1.0 - velocity) * self.vel_sens;
            (
                set.zones[zone].takes[0][t],
                level + set.correction_db(zone, 0) + MUTE_TRIM_DB,
                tone,
            )
        } else {
            let set = &bank.g_sustain;
            let natural = velocity_db(&set.target_db, &G_SUSTAIN_VEL, velocity);
            let top = set.target_db[G_LAYERS - 1];
            let level = top + self.vel_sens * (natural - top);
            if level < top - 90.0 {
                return;
            }
            let (lo, frac) = layer_position(&G_SUSTAIN_VEL, velocity);
            let pick = 0.5 * (rng.next() + 1.0);
            let layer = if lo + 1 < G_LAYERS && frac > pick {
                lo + 1
            } else {
                lo
            };
            let t = (self.last_take[zone] + 1) % G_TAKES;
            self.last_take[zone] = t;
            (
                set.zones[zone].takes[layer][t],
                level + set.correction_db(zone, layer),
                0.0,
            )
        };
        if take.frames < 4 {
            return;
        }
        // The pick stops this key's ringing string first.
        self.damp_where(RETRIGGER_T60, |v| v.key == key);
        self.start(
            key,
            id,
            take,
            semis as f32,
            db_to_gain(gain_db),
            false,
            vel_tone,
        );
    }

    /// Plays the nearest palm-muted recording as recorded: velocity follows
    /// the sustain curve but only sets the level.
    fn palm_note(&mut self, key: u8, velocity: f32, id: u32) {
        let set = &self.bank.g_palm;
        let Some((zone, semis)) = set.nearest(key) else {
            return;
        };
        let sus = &self.bank.g_sustain.target_db;
        let natural = velocity_db(sus, &G_SUSTAIN_VEL, velocity);
        let top = sus[G_LAYERS - 1];
        let level = top + self.vel_sens * (natural - top);
        if level < top - 90.0 {
            return;
        }
        let z = &set.zones[zone];
        let count = z.takes.len();
        self.hits = self.hits.wrapping_add(1);
        let mut rng = HitRng::new(self.hits, 0x9A1A_0000 + z.root as u32);
        let last = self.palm_last[z.root as usize] as usize;
        // Any take but the last one, chosen deterministically from the hit
        // count.
        let t = if count > 1 {
            let skip = (0.5 * (rng.next() + 1.0) * (count - 1) as f32) as usize;
            (last + 1 + skip.min(count - 2)) % count
        } else {
            0
        };
        self.palm_last[z.root as usize] = t as u8;
        let take = z.takes[t];
        let gain_db = level + z.correction_db[t] + PALM_TRIM_DB;
        self.damp_where(RETRIGGER_T60, |v| v.key == key);
        self.start(key, id, take, semis as f32, db_to_gain(gain_db), false, 0.0);
    }

    fn bass_note(&mut self, key: u8, velocity: f32, id: u32) {
        let Some((zone, semis)) = nearest(&B_ROOTS, key) else {
            return;
        };
        let set = &self.bank.bass;
        let natural = velocity_db(&set.target_db, &B_PICK_VEL, velocity);
        let top = set.target_db[B_LAYERS - 1];
        let level = top + self.vel_sens * (natural - top);
        if level < top - 90.0 {
            return;
        }
        self.hits = self.hits.wrapping_add(1);
        let mut rng = HitRng::new(self.hits, 0xBA55_0000 + zone as u32);
        let (lo, frac) = layer_position(&B_PICK_VEL, velocity);
        let pick = 0.5 * (rng.next() + 1.0);
        let layer = if lo + 1 < B_LAYERS && frac > pick {
            lo + 1
        } else {
            lo
        };
        let t = (self.last_take[zone] + 1 + usize::from(rng.next() >= 0.0)) % B_TAKES;
        self.last_take[zone] = t;
        let take = set.zones[zone].takes[layer][t];
        if take.frames < 4 {
            return;
        }
        let gain = db_to_gain(level + set.correction_db(zone, layer));
        if self.mono {
            self.damp_where(MONO_T60, |_| true);
        } else {
            self.damp_where(RETRIGGER_T60, |v| v.key == key);
        }
        self.start(key, id, take, semis as f32, gain, false, 0.0);
    }

    /// Plays the sound of a string being stopped for a released sustain note.
    fn fret_noise(&mut self, key: u8, id: u32, note_gain: f32, env: f32, age: u32) {
        let held_s = age as f32 / self.sr;
        if self.fret <= 0.0 || held_s < RELEASE_MIN_S {
            return;
        }
        let Some((zone, semis)) = nearest(&G_ROOTS, key) else {
            return;
        };
        let set = &self.bank.g_release;
        self.hits = self.hits.wrapping_add(1);
        // The two takes alternate with the hit count.
        let t = (self.hits as usize) % G_TAKES;
        let take = set.zones[zone].takes[0][t];
        if take.frames < 4 {
            return;
        }
        let held_db = (-RELEASE_DB_PER_S * held_s).max(RELEASE_FLOOR_DB);
        let gain = note_gain * env.min(1.0) * self.fret * db_to_gain(held_db + RELEASE_TRIM_DB);
        if gain < 1.0e-4 {
            return;
        }
        self.start(key, id, take, semis as f32, gain, true, 0.0);
    }
}

impl Synth for Strings {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        let (cap, poly) = match self.kind {
            Kind::Guitar => (40, 32),
            Kind::Bass => (16, 12),
        };
        self.pool = Pool::new(cap, poly, StealPolicy::Quietest, sample_rate, Voice::new);
        for v in &mut self.pool.voices {
            v.tilt.prepare(sample_rate);
        }
        self.prepare_amp();
        // Fourth-order Butterworth high-pass: the band below the split never
        // reaches the clipper's make-up gain.
        self.split[0].set_q(DRIVE_SPLIT_HZ, 0.541_196, sample_rate);
        self.split[1].set_q(DRIVE_SPLIT_HZ, 1.306_563, sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.reset_tone();
        self.ctrl = 0;
        self.hits = 0;
        self.last_take = [0; 16];
        self.palm_last = [0; 128];
        self.tail = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            ARTICULATION => self.muted = v >= 0.5,
            MUTE => self.mute.set(v, imm),
            RELEASE => self.release_s = v,
            FRET_NOISE => self.fret = v,
            MONO => self.mono = v >= 0.5,
            OUTPUT => self.amp_mix.set(if v >= 0.5 { 0.0 } else { 1.0 }, imm),
            AMP_GAIN => self.gain.set(v, imm),
            AMP_BASS => self.bass.set(v, imm),
            AMP_MID => self.mid.set(v, imm),
            AMP_TREBLE => self.treble.set(v, imm),
            CAB_TYPE => self.cab_type = (v.max(0.0) as usize).min(CABS.len() - 1),
            LEVEL => self.level.set(v, imm),
            VELOCITY => self.vel_sens = v,
            DRIVE => self.drive.set(v, imm),
            TONE => self.tone.set(v.ln(), imm),
            _ => {}
        }
        if imm {
            self.out_gain = db_to_gain(
                self.level.get()
                    + match self.kind {
                        Kind::Guitar => GUITAR_DB,
                        Kind::Bass => BASS_DB,
                    },
            );
            if self.kind == Kind::Bass {
                self.design_tone();
                self.design_drive();
            }
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        if velocity <= 0.0 {
            return;
        }
        match self.kind {
            Kind::Guitar => self.guitar_note(key, velocity, id),
            Kind::Bass => self.bass_note(key, velocity, id),
        }
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        let rel_k = coef(self.release_s, self.sr);
        let guitar = self.kind == Kind::Guitar;
        let mut noise: [(u8, f32, f32, u32); 8] = [(0, 0.0, 0.0, 0); 8];
        let mut n = 0;
        self.pool.release_id(id, |v| {
            v.released = true;
            v.rel_k = rel_k;
            if guitar && !v.muted && !v.one_shot && n < noise.len() {
                noise[n] = (v.key, v.gain, v.envelope(), v.age);
                n += 1;
            }
        });
        for &(key, gain, env, age) in &noise[..n] {
            self.fret_noise(key, id, gain, env, age);
        }
    }

    fn all_notes_off(&mut self) {
        let rel_k = coef(self.release_s, self.sr);
        self.pool.release_all(|v| {
            v.released = true;
            v.rel_k = rel_k;
        });
    }

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
        self.reset_tone();
        self.tail = 0;
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let tail_len = (TAIL_S * self.sr) as usize;
        for n in 0..left.len().min(right.len()) {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let mut mix = 0.0;
            let mut any = false;
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                any = true;
                mix += v.tick();
            }
            if any {
                self.tail = tail_len;
            } else if self.tail > 0 {
                self.tail -= 1;
                if self.tail == 0 {
                    self.reset_tone();
                }
            }
            let out = if self.tail == 0 {
                0.0
            } else {
                match self.kind {
                    Kind::Guitar => {
                        let a = self.amp_mix.get();
                        let x = mix * self.in_gain;
                        let wet = if a > 0.0 {
                            self.amp_running = true;
                            self.amp.process(x)
                        } else {
                            if self.amp_running {
                                self.amp.reset();
                                self.amp_running = false;
                            }
                            0.0
                        };
                        lerp(x * self.direct_gain, wet, a)
                    }
                    Kind::Bass => {
                        let mut y = mix;
                        // The split filters always run, so engaging Drive
                        // mid-note continues from settled state.
                        let h1 = self.split[0].process(mix, SvfMode::HighPass);
                        let high = self.split[1].process(h1, SvfMode::HighPass);
                        if self.drive_wet > 0.0 {
                            // Only the distortion of the upper band is added, so
                            // the band below the split (B0 to about A3) stays the
                            // clean recording.
                            let shaped = self.adaa.tick(high * self.drive_pre) * self.drive_makeup;
                            y = mix + self.drive_wet * (shaped - high);
                        } else {
                            self.adaa = Adaa::new();
                        }
                        if self.tone_on {
                            y = self.tone_lp.process(y, SvfMode::LowPass);
                        }
                        y
                    }
                }
            };
            let out = out * self.out_gain;
            left[n] = out;
            right[n] = out;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent() && self.tail == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_note_and_range() {
        assert_eq!(nearest(&G_ROOTS, 40), Some((0, 0)));
        assert_eq!(nearest(&G_ROOTS, 41), Some((0, 1)));
        assert_eq!(nearest(&G_ROOTS, 42), Some((1, -1)));
        assert_eq!(nearest(&G_ROOTS, 28), Some((0, -12)));
        assert_eq!(nearest(&G_ROOTS, 27), None);
        assert_eq!(nearest(&G_ROOTS, 97), Some((15, 12)));
        assert_eq!(nearest(&G_ROOTS, 98), None);
        assert_eq!(nearest(&B_ROOTS, 28), Some((2, -1)));
        assert_eq!(nearest(&B_ROOTS, 11), Some((0, -12)));
        assert_eq!(nearest(&B_ROOTS, 0), None);
        for k in 0..=127u8 {
            if let Some((_, s)) = nearest(&G_ROOTS, k) {
                assert!(s.abs() <= 12);
                if (40..=85).contains(&k) {
                    assert!(s.abs() <= 1);
                }
            }
        }
    }

    #[test]
    fn velocity_curve_is_continuous_and_rising() {
        for (targets, centres) in [
            (&[-30.0f32, -22.0, -15.0][..], &G_SUSTAIN_VEL[..]),
            (&[-20.0][..], &[0.85][..]),
            (&[-18.0, -12.0][..], &B_PICK_VEL[..]),
        ] {
            let mut prev = f32::NEG_INFINITY;
            for i in 1..=1_000 {
                let v = i as f32 / 1_000.0;
                let db = velocity_db(targets, centres, v);
                assert!(db > prev);
                assert!(db - prev < 1.0 || v < 0.05);
                prev = db;
            }
            for (l, &c) in centres.iter().enumerate() {
                assert!((velocity_db(targets, centres, c) - targets[l]).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn adaa_is_bounded_and_odd() {
        let mut a = Adaa::new();
        let mut b = Adaa::new();
        for i in 0..2_000 {
            let x = 40.0 * (i as f32 * 0.01).sin();
            let ya = a.tick(x);
            let yb = b.tick(-x);
            assert!(
                ya.abs() <= 1.0001 && (ya + yb).abs() < 1e-4,
                "{x}: {ya} {yb}"
            );
        }
    }

    #[test]
    fn parses_mono_only() {
        assert!(parse_wav(b"RIFF\0\0\0\0WAVE").is_none());
        assert!(parse_wav(b"nope").is_none());
    }

    #[test]
    fn every_embedded_file_parses() {
        let b = bank();
        let check = |t: &Take| assert!(t.frames > 4_800, "short or unparsed sample");
        for z in &b.g_sustain.zones {
            z.takes.iter().flatten().for_each(check);
        }
        for z in &b.g_mute.zones {
            z.takes.iter().flatten().for_each(check);
        }
        for z in &b.g_release.zones {
            z.takes
                .iter()
                .flatten()
                .for_each(|t| assert!(t.frames > 480));
        }
        for z in &b.bass.zones {
            z.takes.iter().flatten().for_each(check);
        }
    }

    #[test]
    fn every_palm_recording_loads() {
        let palm = &bank().g_palm;
        assert_eq!(palm.zones.len(), PALM_FILES.len());
        for (z, (root, files)) in palm.zones.iter().zip(PALM_FILES) {
            assert_eq!(z.root, *root);
            assert_eq!(z.takes.len(), files.len(), "root {root}");
            assert!(z.takes.iter().all(|t| t.frames > 2_400), "root {root}");
            assert!(z.correction_db.iter().all(|g| g.is_finite()));
        }
        let gains: Vec<f32> = palm
            .zones
            .iter()
            .flat_map(|z| z.correction_db.iter().copied())
            .collect();
        let lo = gains.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = gains.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(hi - lo <= 12.001, "palm take gain must remain bounded");
        assert!(palm.zones.windows(2).all(|w| w[0].root < w[1].root));
        // Exact roots, nearest otherwise, silent past an octave.
        for z in &palm.zones {
            assert_eq!(
                palm.nearest(z.root).map(|(i, s)| (palm.zones[i].root, s)),
                Some((z.root, 0))
            );
        }
        let lo = palm.zones[0].root;
        assert_eq!(palm.nearest(lo - 12).map(|x| x.1), Some(-12));
        assert_eq!(palm.nearest(lo - 13), None);
    }
}
