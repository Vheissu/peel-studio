//! Registry of every native device type.

use peel_dsp::{Device, DeviceDescriptor, DeviceKind};

/// All native device descriptors: instruments first, then effects.
pub fn all() -> impl Iterator<Item = &'static DeviceDescriptor> {
    peel_instruments::descriptors()
        .iter()
        .chain(peel_effects::descriptors().iter())
        .copied()
}

pub fn instruments() -> impl Iterator<Item = &'static DeviceDescriptor> {
    all().filter(|d| d.kind == DeviceKind::Instrument)
}

pub fn effects() -> impl Iterator<Item = &'static DeviceDescriptor> {
    all().filter(|d| d.kind == DeviceKind::Effect)
}

pub fn descriptor(type_id: &str) -> Option<&'static DeviceDescriptor> {
    all().find(|d| d.type_id == type_id)
}

/// Creates a device with descriptor defaults applied. Call `prepare` on a
/// control thread before installing it in the engine.
pub fn create(type_id: &str) -> Option<Box<dyn Device>> {
    let mut device = peel_instruments::create(type_id).or_else(|| peel_effects::create(type_id))?;
    peel_dsp::device::apply_defaults(device.as_mut());
    Some(device)
}
