//! Shared DSP contracts and primitives for Peel Studio's native devices.
//!
//! The [`Device`] trait is the boundary between the realtime engine and every
//! first-party instrument and effect. Its rules follow the realtime contract
//! in `docs/REALTIME.md`:
//!
//! * [`Device::prepare`] runs on a control thread and may allocate.
//! * [`Device::process`], [`Device::set_param`], [`Device::reset`] and
//!   [`Device::set_sample`] run on the audio thread and must not allocate,
//!   free, lock, block, log or perform I/O.
//!
//! Primitive modules (`osc`, `envelope`, ...) are building blocks with the same
//! realtime rules.

pub mod device;
pub mod param;
pub mod preset;
pub mod sample;
pub mod util;

// Primitive building blocks. Instruments primarily use the first group and
// effects the second, but every module is shared.
pub mod envelope;
pub mod filter;
pub mod noise;
pub mod osc;
pub mod voice;

pub mod biquad;
pub mod delay;
pub mod dynamics;
pub mod reverb;

pub use device::{Device, DeviceDescriptor, DeviceEvent, DeviceKind, EventKind, ProcessContext};
pub use param::{ParamDescriptor, ParamId, Taper, Unit};
pub use preset::Preset;
pub use sample::SampleBuffer;
