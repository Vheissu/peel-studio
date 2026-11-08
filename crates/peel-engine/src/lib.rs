//! Peel Studio's realtime audio engine.
//!
//! The engine renders a compiled [`RenderPlan`] through native devices and
//! the mixer graph. It is driven by an audio device callback (via an
//! [`EngineHandle`]) or by an [`OfflineEngine`] for export and tests; both use
//! the same processing code. See `docs/REALTIME.md` for the rules every
//! function reachable from the callback must follow.

pub mod command;
pub mod plan;
pub mod telemetry;

pub use command::{EngineCommand, Retired};
pub use plan::{
    ChannelIndex, ChannelRoute, RenderPlan, SeqEvent, SeqKind, Slot, TrackIndex, TrackRoute,
};
pub use telemetry::Telemetry;

/// Engine sizing. Every capacity is allocated before activation.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Preferred sample rate; the device may negotiate another.
    pub sample_rate: Option<u32>,
    /// Preferred device buffer size in frames.
    pub buffer_frames: Option<u32>,
    /// Largest block the engine processes at once; device callbacks larger
    /// than this are split.
    pub max_block: usize,
    pub device_slots: usize,
    pub command_capacity: usize,
    pub retire_capacity: usize,
    /// Event capacity per channel per block. Overflow drops note-ons first
    /// and never drops note-offs.
    pub events_per_block: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            sample_rate: None,
            buffer_frames: Some(256),
            max_block: 256,
            device_slots: telemetry::MAX_SLOTS,
            command_capacity: 8192,
            retire_capacity: 8192,
            events_per_block: 512,
        }
    }
}
