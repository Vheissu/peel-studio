//! The compiled, immutable execution snapshot of a document revision.
//!
//! The control thread builds a [`RenderPlan`] (allocating freely), sends it
//! to the engine, and receives the previous plan back through the retirement
//! queue. The engine never edits a plan's structure; realtime parameter
//! commands only adjust gains, pans and mutes in place.

use peel_time::TempoMap;

/// Index into [`RenderPlan::channels`].
pub type ChannelIndex = u16;
/// Index into [`RenderPlan::tracks`].
pub type TrackIndex = u16;
/// Stable engine device slot (instruments and effects).
pub type Slot = u32;

#[derive(Clone, Debug)]
pub struct RenderPlan {
    /// Document revision this plan was compiled from.
    pub revision: u64,
    pub tempo: TempoMap,
    /// Note events sorted by `(beat, order)`; ties keep note-offs before
    /// note-ons (see [`SeqEvent::order`]).
    pub sequence: Vec<SeqEvent>,
    /// Musical end of the material (quarter notes). Song mode stops (or
    /// keeps running silently) after it; pattern mode loops at it.
    pub length_beats: f64,
    /// Half-open loop range in quarter notes. When set, the transport wraps
    /// from `end` to `start`, releasing sounding notes at the wrap.
    pub loop_range: Option<(f64, f64)>,
    /// Instrument channels.
    pub channels: Vec<ChannelRoute>,
    /// Mixer tracks in processing order: every track appears after all of
    /// the tracks that feed it. The master is `tracks[master]`.
    pub tracks: Vec<TrackRoute>,
    pub master: TrackIndex,
    /// Beats per bar, for the metronome accent.
    pub beats_per_bar: u32,
}

impl RenderPlan {
    /// A silent plan with only a master track.
    pub fn empty() -> RenderPlan {
        RenderPlan {
            revision: 0,
            tempo: TempoMap::constant(120.0),
            sequence: Vec::new(),
            length_beats: 4.0,
            loop_range: Some((0.0, 4.0)),
            channels: Vec::new(),
            tracks: vec![TrackRoute::new(None, 0)],
            master: 0,
            beats_per_bar: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeqEvent {
    /// Musical position in quarter notes from the plan origin.
    pub beat: f64,
    pub channel: ChannelIndex,
    /// Tie-break for equal beats: note-offs use 0, note-ons use 1.
    pub order: u8,
    pub kind: SeqKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeqKind {
    /// `voice` is unique per realized note (pattern instance x loop
    /// iteration x note), so overlapping instances never share note-offs.
    NoteOn {
        key: u8,
        velocity: f32,
        voice: u32,
    },
    NoteOff {
        key: u8,
        velocity: f32,
        voice: u32,
    },
}

/// One instrument channel: a device slot feeding a mixer track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelRoute {
    pub slot: Slot,
    pub track: TrackIndex,
    /// Linear gain.
    pub gain: f32,
    /// -1..=1.
    pub pan: f32,
    pub muted: bool,
}

/// One mixer track (bus).
#[derive(Clone, Debug, PartialEq)]
pub struct TrackRoute {
    /// Insert effect slots in processing order.
    pub inserts: Vec<Slot>,
    /// Linear gain.
    pub gain: f32,
    pub pan: f32,
    /// Effective mute (includes solo logic resolved by the control side).
    pub muted: bool,
    /// Destination track; `None` only for the master.
    pub output: Option<TrackIndex>,
    /// Telemetry meter index (stable per mixer track identity).
    pub meter: u16,
}

impl TrackRoute {
    pub fn new(output: Option<TrackIndex>, meter: u16) -> TrackRoute {
        TrackRoute {
            inserts: Vec::new(),
            gain: 1.0,
            pan: 0.0,
            muted: false,
            output,
            meter,
        }
    }
}
