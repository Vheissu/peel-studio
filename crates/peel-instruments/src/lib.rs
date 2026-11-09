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
//! | `peel.keys.tine`   | Tine    | Keys     | FM electric piano                            |
//! | `peel.keys.grand`  | Grand   | Keys     | Synthesized acoustic piano (modal)           |
//! | `peel.wind.sax`    | Sax     | Wind     | Saxophone: reed and breath model, legato     |
//! | `peel.guitar.electric` | Guitar | Guitar | Plucked string through pickup, amp and cabinet |
//! | `peel.sampler`     | Sampler | Sampler  | Pitched one-shot / looping sample player     |
//!
//! Each instrument is a [`common::Synth`] wrapped in a shared adapter that
//! implements `Device`: parameters are stored and clamped once, events split
//! the block at their sample offsets, and output is kept finite. Voices are
//! preallocated in `prepare`; nothing allocates while rendering.

mod common;

mod bass;
mod clap;
mod grand;
mod guitar;
mod hat;
mod kick;
mod poly;
mod sampler;
mod sax;
mod snare;
mod tine;
mod tom;

use peel_dsp::{Device, DeviceDescriptor, Preset};

static ALL: [&DeviceDescriptor; 12] = [
    &kick::DESC,
    &snare::DESC,
    &hat::DESC,
    &clap::DESC,
    &tom::DESC,
    &poly::DESC,
    &bass::DESC,
    &tine::DESC,
    &grand::DESC,
    &guitar::DESC,
    &sampler::DESC,
    &sax::DESC,
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
        "peel.synth.poly" => Some(poly::create()),
        "peel.synth.bass" => Some(bass::create()),
        "peel.keys.tine" => Some(tine::create()),
        "peel.keys.grand" => Some(grand::create()),
        "peel.guitar.electric" => Some(guitar::create()),
        "peel.sampler" => Some(sampler::create()),
        "peel.wind.sax" => Some(sax::create()),
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
        "peel.synth.poly" => &poly::PRESETS,
        "peel.synth.bass" => &bass::PRESETS,
        "peel.keys.tine" => &tine::PRESETS,
        "peel.keys.grand" => &grand::PRESETS,
        "peel.guitar.electric" => &guitar::PRESETS,
        "peel.sampler" => &sampler::PRESETS,
        "peel.wind.sax" => &sax::PRESETS,
        _ => &[],
    }
}
