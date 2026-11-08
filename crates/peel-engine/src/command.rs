use std::sync::Arc;

use peel_dsp::{Device, ParamId, SampleBuffer};

use crate::input::MonitorReceiver;
use crate::plan::{ChannelIndex, RenderPlan, Slot, TrackIndex};

/// Control-to-engine messages. Sent through a bounded lock-free queue and
/// applied at the start of the next block, in order.
pub enum EngineCommand {
    /// Activate a new plan at the next block boundary. The previous plan is
    /// returned through [`Retired::Plan`].
    SetPlan(Box<RenderPlan>),
    /// Install a device that was created and `prepare`d (at the engine's
    /// sample rate and max block) on the control thread. Any device already
    /// in the slot is retired.
    InstallDevice {
        slot: Slot,
        device: Box<dyn Device>,
    },
    /// Remove and retire the device in a slot.
    RemoveDevice {
        slot: Slot,
    },
    SetParam {
        slot: Slot,
        id: ParamId,
        value: f32,
    },
    /// Replace a sampler's sample; the previous one is retired.
    SetSample {
        slot: Slot,
        sample: Option<Arc<SampleBuffer>>,
    },
    /// Bypass an insert effect (instruments ignore this).
    SetBypass {
        slot: Slot,
        bypass: bool,
    },

    SetChannelGain {
        channel: ChannelIndex,
        gain: f32,
    },
    SetChannelPan {
        channel: ChannelIndex,
        pan: f32,
    },
    SetChannelMute {
        channel: ChannelIndex,
        muted: bool,
    },
    SetTrackGain {
        track: TrackIndex,
        gain: f32,
    },
    SetTrackPan {
        track: TrackIndex,
        pan: f32,
    },
    SetTrackMute {
        track: TrackIndex,
        muted: bool,
    },

    Play,
    /// Stop and release notes; position stays where it is.
    Pause,
    /// Stop, release notes and return to `return_to` (quarter notes).
    Stop {
        return_to: f64,
    },
    /// Move the playhead (quarter notes). Sounding notes are released.
    Seek {
        beat: f64,
    },
    Metronome {
        enabled: bool,
    },
    /// Live performance input (computer keyboard, on-screen keys, MIDI in).
    /// Routed to the device in `slot` regardless of transport state.
    LiveNote {
        slot: Slot,
        key: u8,
        velocity: f32,
        on: bool,
    },
    /// Audition a sample on the preview route (excluded from export).
    /// `None` stops the preview. The previous sample is retired.
    Preview {
        sample: Option<Arc<SampleBuffer>>,
        gain: f32,
    },
    /// Release every voice everywhere and reset sustain (does not edit data).
    Panic,
    /// Connect (or, with `None`, disconnect) the monitor FIFO that an audio
    /// input feeds. The previous one is retired. See [`crate::input`].
    SetInputMonitor(Option<Box<MonitorReceiver>>),
}

/// Engine-to-control messages carrying objects that must be freed off the
/// audio thread.
pub enum Retired {
    Plan(Box<RenderPlan>),
    Device(Box<dyn Device>),
    Sample(Arc<SampleBuffer>),
    Monitor(Box<MonitorReceiver>),
}
