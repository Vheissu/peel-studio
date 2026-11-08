//! Peel Studio's native effects.
//!
//! Required device types (stable `type_id`s; do not rename):
//!
//! | type_id              | Name    | Category   |
//! |----------------------|---------|------------|
//! | `peel.fx.eq`         | EQ      | EQ         |
//! | `peel.fx.compressor` | Comp    | Dynamics   |
//! | `peel.fx.limiter`    | Limiter | Dynamics   |
//! | `peel.fx.delay`      | Delay   | Delay      |
//! | `peel.fx.reverb`     | Reverb  | Reverb     |
//! | `peel.fx.drive`      | Drive   | Distortion |
//! | `peel.fx.filter`     | Filter  | Filter     |
//! | `peel.fx.chorus`     | Chorus  | Modulation |
//! | `peel.fx.utility`    | Utility | Utility    |

use peel_dsp::{Device, DeviceDescriptor, Preset};

mod common;
pub mod compressor;
pub mod delay;
pub mod eq;
pub mod limiter;
pub mod utility;

/// Every effect descriptor, in browser order.
pub fn descriptors() -> &'static [&'static DeviceDescriptor] {
    &[]
}

/// Creates an effect with default parameter values (not yet prepared).
pub fn create(_type_id: &str) -> Option<Box<dyn Device>> {
    None
}

/// Factory presets for a device type, in browser order. The first preset
/// should equal the descriptor defaults.
pub fn presets(_type_id: &str) -> &'static [Preset] {
    &[]
}
