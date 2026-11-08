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
//! | `peel.sampler`     | Sampler | Sampler  | Pitched one-shot / looping sample player     |

use peel_dsp::{Device, DeviceDescriptor, Preset};

/// Every instrument descriptor, in browser order.
pub fn descriptors() -> &'static [&'static DeviceDescriptor] {
    &[]
}

/// Creates an instrument with default parameter values (not yet prepared).
pub fn create(_type_id: &str) -> Option<Box<dyn Device>> {
    None
}

/// Factory presets for a device type, in browser order. The first preset
/// should equal the descriptor defaults.
pub fn presets(_type_id: &str) -> &'static [Preset] {
    &[]
}
