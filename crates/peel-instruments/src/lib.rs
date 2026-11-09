//! Peel Studio's native instruments.
//!
//! Required device types (stable `type_id`s; do not rename):
//!
//! | type_id            | Name    | Category | Sound                                        |
//! |--------------------|---------|----------|----------------------------------------------|
//! | `peel.drum.kick`   | Kick    | Drums    | Synthesized kick: pitch sweep, click, drive  |
//! | `peel.drum.snare`  | Snare   | Drums    | Tuned body plus filtered noise and snap      |
//! | `peel.drum.hat`    | Hat     | Drums    | Metallic oscillator bank and noise, closed/open |
//! | `peel.drum.clap`   | Clap    | Drums    | Multi-burst filtered noise with room tail    |
//! | `peel.drum.tom`    | Tom     | Drums    | Pitched membrane with sweep                  |
//! | `peel.synth.poly`  | Poly    | Synth    | Polyphonic subtractive synth                 |
//! | `peel.synth.bass`  | Bass    | Synth    | Monophonic bass with glide and drive         |
//! | `peel.keys.tine`   | Tine    | Keys     | Electric piano: tine and pickup model        |
//! | `peel.keys.grand`  | Grand   | Keys     | Synthesized acoustic piano (modal)           |
//! | `peel.wind.sax`    | Sax     | Wind     | Saxophone: reed and breath model, legato     |
//! | `peel.guitar.electric` | Guitar | Guitar | Plucked string through pickup, amp and cabinet |
//! | `peel.sampler`     | Sampler | Sampler  | Pitched one-shot / looping sample player     |
//! | `peel.strings.violin` | Violin | Strings | Bowed-string waveguide, body, ensemble, pizzicato |
//! | `peel.perc.mallet` | Mallet | Percussion | Pitched modal bars, membranes, plates and bells |
//! | `peel.strings.zither` | Zither | Strings | Paired acoustic strings with coupling and body |
//! | `peel.synth.cross` | Cross | Synth | Cross FM, oscillator sync and ring modulation |
//!
//! ## Per-note expression
//!
//! `EventKind::VoiceExpression` gives one runtime voice its own pitch
//! (semitones, ±120), pressure and timbre (CC74, neutral 0.5). It replaces
//! the instrument-wide bend and aftertouch for that voice only, so MPE
//! members and recorded pattern curves move each note on its own, even two
//! notes on the same key. A snapshot sent before or with its NoteOn starts
//! the note there. Pressure raises level (and opens filters) everywhere.
//! Timbre moves brightness:
//!
//! - filter cutoff: Poly, Bass, Cross, Sax (±2 octaves);
//! - tone filter and attack: Kick (beater click), Tom (stick), Snare (wire
//!   band), Hat (bright band), Clap (burst band);
//! - string loss: Guitar, Zither; mallet hardness and mode tilt: Mallet;
//! - a ±9 dB treble shelf: Grand, Tine, Sampler, Violin.
//!
//! Limits: Hat and Clap have no pitch to bend. Bass, Sax (mono) and Poly in
//! Mono/Legato play the expression of the note on top of their key stack.
//! Zither re-strikes the key's own string, so same-key notes share it.
//! Extreme bends stay finite: oscillators and resonators clamp below
//! Nyquist, waveguides clamp their loop lengths and silence strings out of
//! range, and the sampler caps its playback rate.
//!
//! Each instrument is a [`common::Synth`] wrapped in a shared adapter that
//! implements `Device`: parameters are stored and clamped once, events split
//! the block at their sample offsets, and output is kept finite. Voices are
//! preallocated in `prepare`; nothing allocates while rendering.

mod analog;
mod common;
mod minblep_table;

mod bass;
mod clap;
mod cross;
mod grand;
mod guitar;
mod hat;
mod kick;
mod mallet;
mod poly;
mod sampler;
mod sax;
mod snare;
mod tine;
mod tom;
mod violin;
mod zither;

use peel_dsp::{Device, DeviceDescriptor, Preset};

static ALL: [&DeviceDescriptor; 16] = [
    &kick::DESC,
    &snare::DESC,
    &hat::DESC,
    &clap::DESC,
    &tom::DESC,
    &mallet::DESC,
    &poly::DESC,
    &bass::DESC,
    &cross::DESC,
    &tine::DESC,
    &grand::DESC,
    &guitar::DESC,
    &sampler::DESC,
    &sax::DESC,
    &violin::DESC,
    &zither::DESC,
];

/// Every instrument descriptor, in browser order.
pub fn descriptors() -> &'static [&'static DeviceDescriptor] {
    &ALL
}

/// Creates an instrument with default parameter values (not yet prepared).
pub fn create(type_id: &str) -> Option<Box<dyn Device>> {
    match type_id {
        "peel.drum.kick" => Some(kick::create()),
        "peel.drum.snare" => Some(snare::create()),
        "peel.drum.hat" => Some(hat::create()),
        "peel.drum.clap" => Some(clap::create()),
        "peel.drum.tom" => Some(tom::create()),
        "peel.perc.mallet" => Some(mallet::create()),
        "peel.synth.cross" => Some(cross::create()),
        "peel.strings.zither" => Some(zither::create()),
        "peel.synth.poly" => Some(poly::create()),
        "peel.synth.bass" => Some(bass::create()),
        "peel.keys.tine" => Some(tine::create()),
        "peel.keys.grand" => Some(grand::create()),
        "peel.guitar.electric" => Some(guitar::create()),
        "peel.sampler" => Some(sampler::create()),
        "peel.wind.sax" => Some(sax::create()),
        "peel.strings.violin" => Some(violin::create()),
        _ => None,
    }
}

/// Factory presets for a device type, in browser order. The first preset
/// should equal the descriptor defaults.
pub fn presets(type_id: &str) -> &'static [Preset] {
    match type_id {
        "peel.drum.kick" => &kick::PRESETS,
        "peel.drum.snare" => &snare::PRESETS,
        "peel.drum.hat" => &hat::PRESETS,
        "peel.drum.clap" => &clap::PRESETS,
        "peel.drum.tom" => &tom::PRESETS,
        "peel.perc.mallet" => &mallet::PRESETS,
        "peel.synth.cross" => &cross::PRESETS,
        "peel.strings.zither" => &zither::PRESETS,
        "peel.synth.poly" => &poly::PRESETS,
        "peel.synth.bass" => &bass::PRESETS,
        "peel.keys.tine" => &tine::PRESETS,
        "peel.keys.grand" => &grand::PRESETS,
        "peel.guitar.electric" => &guitar::PRESETS,
        "peel.sampler" => &sampler::PRESETS,
        "peel.wind.sax" => &sax::PRESETS,
        "peel.strings.violin" => &violin::PRESETS,
        _ => &[],
    }
}
