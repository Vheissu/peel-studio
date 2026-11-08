//! Peel Studio's realtime audio engine.
//!
//! The engine renders a compiled [`RenderPlan`] through native devices and
//! the mixer graph. It is driven by an audio device callback (via an
//! [`EngineHandle`]) or by an [`OfflineEngine`] for export and tests; both use
//! the same [`Engine`] and the same `process` code. See `docs/REALTIME.md`
//! for the rules every function reachable from the callback must follow.
//!
//! # Ownership
//!
//! * **Control thread**: builds plans and devices, sends [`EngineCommand`]s,
//!   reads [`Telemetry`], and frees everything the engine returns as
//!   [`Retired`] (call [`EngineHandle::collect_garbage`] regularly).
//! * **Audio thread**: owns the [`Engine`]. It never allocates or frees.
//!
//! # Buffer ownership
//!
//! Plans arrive on the audio thread, so the audio side cannot allocate
//! buffers for them. The engine instead preallocates a pool when it is
//! created: [`MAX_TRACKS`] stereo track buffers of `max_block` frames, one
//! render scratch pair, per-slot event buffers (`events_per_block` each) and
//! an active-note table ([`MAX_ACTIVE_NOTES`]). A plan with more tracks than
//! the pool, more channels than `device_slots`, or an invalid master index is
//! refused (returned as [`Retired::Plan`] and counted in
//! [`Telemetry::plans_rejected`]) and the previous plan keeps running. The
//! [`RenderPlan`] structure itself is unchanged and carries no buffers.
//!
//! # Metronome and preview routing
//!
//! The metronome click and the sample preview voice are summed into the
//! output *after* the master track. They bypass master inserts, master gain
//! and every meter, so they never show on the master meter and never clip
//! the mix. [`OfflineEngine`] disables both so exports never contain them.
//!
//! # Voice and note policy
//!
//! The engine tracks every note it starts, by `(slot, voice)`. Stop, Pause,
//! Seek, a loop wrap and the end of the material release exactly the
//! sounding sequence notes. A plan swap releases only those notes whose
//! note-off no longer lies ahead in the new plan, so editing during
//! playback does not cut sustained notes. Starting or wrapping in the middle
//! of a note does not retrigger it (no note chase); a note that crosses the
//! loop end is cut at the wrap and plays again on the next pass. Live notes
//! are released only by their own note-off or by `Panic`. Plan note voice ids
//! must be below [`LIVE_VOICE_BASE`].
//!
//! # Live input
//!
//! Hardware MIDI does not go through the command queue. [`EngineHandle::take_live_input`]
//! hands out a [`LiveInput`] whose notes travel through a queue of their own to
//! the instrument chosen with [`EngineHandle::live_target`]. The sustain pedal
//! is applied by the engine (see [`live`]), live notes are staged with the
//! same voice ids as `EngineCommand::LiveNote` (`LIVE_VOICE_BASE + key`), and
//! every note that arrives is also copied, with the transport position, into a
//! ring the control thread drains with [`EngineHandle::drain_recorded`].
//!
//! # Audio devices
//!
//! [`list_output_devices`] enumerates what the system offers and
//! [`EngineConfig::device`] picks one. To switch, drop or
//! [`stop_audio`](EngineHandle::stop_audio) the handle and start a new one;
//! the new engine begins empty, so the caller resends devices, samples and the
//! plan (`peel-session` does this in `restart_audio`).
//!
//! # Realtime allocation checking
//!
//! With the `rt-check` feature the cpal callback runs inside
//! `assert_no_alloc`. The binary must also install the checking allocator,
//! which exists only in builds with `debug_assertions`:
//!
//! ```ignore
//! #[cfg(all(feature = "rt-check", debug_assertions))]
//! #[global_allocator]
//! static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
//! ```
//!
//! An allocation on the audio thread then aborts the process in debug
//! builds, with a stack trace under a debugger.

mod clock;
pub mod command;
mod engine;
mod events;
mod handle;
pub mod live;
mod mixer;
mod monitor;
mod offline;
pub mod plan;
mod retire;
pub mod rt;
pub mod telemetry;

pub use command::{EngineCommand, Retired};
pub use engine::Engine;
pub use handle::{EngineError, EngineHandle, OutputDeviceInfo, list_output_devices};
pub use live::{
    LIVE_TARGET_NONE, LiveInput, LiveStats, LiveTarget, LiveTiming, RecordedNote, RecordedReceiver,
};
pub use offline::OfflineEngine;
pub use plan::{
    ChannelIndex, ChannelRoute, RenderPlan, SeqEvent, SeqKind, Slot, TrackIndex, TrackRoute,
};
pub use telemetry::Telemetry;

/// Mixer tracks the engine can run (one per telemetry meter).
pub const MAX_TRACKS: usize = telemetry::MAX_METERS;
/// Notes the engine can track as sounding at once; further note-ons are
/// dropped and counted in [`Telemetry::dropped_notes`].
pub const MAX_ACTIVE_NOTES: usize = 4096;
/// Voice ids at or above this value are reserved for live notes
/// (`LIVE_VOICE_BASE + key`). Plan voice ids must stay below it.
pub const LIVE_VOICE_BASE: u32 = 0xFFFF_FF00;

/// Engine sizing. Every capacity is allocated before activation.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Preferred sample rate; the device may negotiate another.
    pub sample_rate: Option<u32>,
    /// Preferred device buffer size in frames.
    pub buffer_frames: Option<u32>,
    /// Output device to open, by the name or id from
    /// [`list_output_devices`]. `None`, or a device that is not found, uses
    /// the system default (the latter with a logged warning).
    pub device: Option<String>,
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
            device: None,
            max_block: 256,
            device_slots: telemetry::MAX_SLOTS,
            command_capacity: 8192,
            retire_capacity: 8192,
            events_per_block: 512,
        }
    }
}
