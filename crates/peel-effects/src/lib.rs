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
//! | `peel.fx.bass_lift`  | Bass Lift | EQ       |
//! | `peel.fx.vintage_chorus` | Vintage Chorus | Modulation |
//! | `peel.fx.flanger`    | Flanger | Modulation |
//! | `peel.fx.phaser`     | Phaser  | Modulation |
//! | `peel.fx.glitch`     | Glitch  | Creative   |
//! | `peel.fx.multiband`  | Multiband | Dynamics |
//! | `peel.fx.mastering`  | Mastering | Mastering |
//!
//! Every effect processes the left and right buffers in place, ignores note
//! events, splits blocks at parameter events so changes land on the exact
//! frame, smooths its parameters against zipper noise, and allocates only in
//! `prepare`. Concrete types are public so a host can reach device-specific
//! readouts, for example [`compressor::Compressor::gain_reduction_db`],
//! [`limiter::Limiter::gain_reduction_db`] and [`eq::eq_response_db`].

use peel_dsp::{Device, DeviceDescriptor, Preset};

mod common;
mod oversample;

pub mod bass_lift;
pub mod chorus;
pub mod compressor;
pub mod delay;
pub mod drive;
pub mod eq;
pub mod filter;
pub mod flanger;
pub mod glitch;
pub mod limiter;
pub mod mastering;
pub mod multiband;
pub mod phaser;
pub mod reverb;
pub mod utility;
pub mod vintage_chorus;

/// Every effect descriptor, in browser order.
pub fn descriptors() -> &'static [&'static DeviceDescriptor] {
    static ALL: [&DeviceDescriptor; 16] = [
        &eq::DESCRIPTOR,
        &compressor::DESCRIPTOR,
        &limiter::DESCRIPTOR,
        &delay::DESCRIPTOR,
        &reverb::DESCRIPTOR,
        &drive::DESCRIPTOR,
        &filter::DESCRIPTOR,
        &chorus::DESCRIPTOR,
        &utility::DESCRIPTOR,
        &bass_lift::DESCRIPTOR,
        &vintage_chorus::DESCRIPTOR,
        &flanger::DESCRIPTOR,
        &phaser::DESCRIPTOR,
        &glitch::DESCRIPTOR,
        &multiband::DESCRIPTOR,
        &mastering::DESCRIPTOR,
    ];
    &ALL
}

/// Creates an effect with default parameter values (not yet prepared).
///
/// The device is usable straight away at 48 kHz; the host should still call
/// `prepare` with the real sample rate and block size before installing it.
pub fn create(type_id: &str) -> Option<Box<dyn Device>> {
    Some(match type_id {
        "peel.fx.eq" => Box::new(eq::Eq::new()),
        "peel.fx.compressor" => Box::new(compressor::Compressor::new()),
        "peel.fx.limiter" => Box::new(limiter::Limiter::new()),
        "peel.fx.delay" => Box::new(delay::Delay::new()),
        "peel.fx.reverb" => Box::new(reverb::Reverb::new()),
        "peel.fx.drive" => Box::new(drive::Drive::new()),
        "peel.fx.filter" => Box::new(filter::Filter::new()),
        "peel.fx.chorus" => Box::new(chorus::Chorus::new()),
        "peel.fx.utility" => Box::new(utility::Utility::new()),
        "peel.fx.bass_lift" => Box::new(bass_lift::BassLift::new()),
        "peel.fx.vintage_chorus" => Box::new(vintage_chorus::VintageChorus::new()),
        "peel.fx.flanger" => Box::new(flanger::Flanger::new()),
        "peel.fx.phaser" => Box::new(phaser::Phaser::new()),
        "peel.fx.glitch" => Box::new(glitch::Glitch::new()),
        "peel.fx.multiband" => Box::new(multiband::Multiband::new()),
        "peel.fx.mastering" => Box::new(mastering::Mastering::new()),
        _ => return None,
    })
}

/// Factory presets for a device type, in browser order. The first preset
/// equals the descriptor defaults.
pub fn presets(type_id: &str) -> &'static [Preset] {
    match type_id {
        "peel.fx.eq" => &eq::PRESETS,
        "peel.fx.compressor" => &compressor::PRESETS,
        "peel.fx.limiter" => &limiter::PRESETS,
        "peel.fx.delay" => &delay::PRESETS,
        "peel.fx.reverb" => &reverb::PRESETS,
        "peel.fx.drive" => &drive::PRESETS,
        "peel.fx.filter" => &filter::PRESETS,
        "peel.fx.chorus" => &chorus::PRESETS,
        "peel.fx.utility" => &utility::PRESETS,
        "peel.fx.bass_lift" => &bass_lift::PRESETS,
        "peel.fx.vintage_chorus" => &vintage_chorus::PRESETS,
        "peel.fx.flanger" => &flanger::PRESETS,
        "peel.fx.phaser" => &phaser::PRESETS,
        "peel.fx.glitch" => &glitch::PRESETS,
        "peel.fx.multiband" => &multiband::PRESETS,
        "peel.fx.mastering" => &mastering::PRESETS,
        _ => &[],
    }
}

/// Plain values of every parameter of `device` in descriptor order. This is
/// the layout [`eq::eq_response_db`] expects, and a convenient snapshot for
/// any UI that needs all values at once (control thread only; it allocates).
pub fn read_params(device: &dyn Device) -> Vec<f32> {
    device
        .descriptor()
        .params
        .iter()
        .map(|p| device.param(p.id))
        .collect()
}
