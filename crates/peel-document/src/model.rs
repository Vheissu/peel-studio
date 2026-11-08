//! The authoritative musical document.
//!
//! Heavy collections (`Pattern`, `Arrangement`) sit behind `Arc` so that an
//! undo checkpoint is a cheap structural copy: editing a pattern clones only
//! that pattern (`Arc::make_mut`), while every untouched pattern is shared
//! between the live document and its history.

use std::collections::BTreeMap;
use std::sync::Arc;

use peel_time::{TempoSegment, Time, TimeSignature};
use serde::{Deserialize, Serialize};

use crate::{Color, Id};

pub const FORMAT: &str = "peel-project";
pub const SCHEMA_VERSION: u32 = 1;

/// Key used for a step that has not been given a pitch (C5 in Peel's
/// convention, MIDI 60).
pub const DEFAULT_ROOT_KEY: u8 = 60;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub format: String,
    pub schema: u32,
    pub id: Id,
    /// Incremented by every committed transaction.
    pub revision: u64,
    pub meta: ProjectMeta,
    /// Tempo map. The first segment is the song tempo.
    pub tempo: Vec<TempoSegment>,
    pub time_signature: TimeSignature,
    /// Global swing amount, 0..=1.
    pub swing: f32,
    /// Seed for probability and generators.
    pub seed: u64,
    pub channels: Vec<Channel>,
    pub patterns: Vec<Arc<Pattern>>,
    pub arrangements: Vec<Arc<Arrangement>>,
    pub active_arrangement: Id,
    pub mixer: Mixer,
    pub assets: Vec<AudioAsset>,
    /// Namespaced data from newer minor versions, preserved on save.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub notes: String,
}

/// A sound or control generator in the shared rack. Not an arrangement lane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: Id,
    pub name: String,
    pub color: Color,
    pub device: DeviceInstance,
    /// Destination mixer track.
    pub track: Id,
    pub gain_db: f32,
    pub pan: f32,
    pub muted: bool,
    /// Pitch used for new steps in the step sequencer.
    pub root_key: u8,
}

/// A native (or, later, external) device and its saved parameter values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceInstance {
    pub id: Id,
    /// Stable device type, e.g. `peel.synth.poly`.
    pub type_id: String,
    /// DSP algorithm version the parameters were saved with.
    pub version: u32,
    /// Plain parameter values by stable parameter key.
    pub params: BTreeMap<String, f32>,
    /// Sample asset for sample-based devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<Id>,
    #[serde(default)]
    pub bypass: bool,
    /// Optional user preset name shown in the device header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

impl DeviceInstance {
    pub fn new(type_id: impl Into<String>, version: u32) -> DeviceInstance {
        DeviceInstance {
            id: Id::new(),
            type_id: type_id.into(),
            version,
            params: BTreeMap::new(),
            sample: None,
            bypass: false,
            preset: None,
        }
    }
}

/// A reusable container of notes addressing any subset of channels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub id: Id,
    pub name: String,
    pub color: Color,
    /// Declared length in quarter notes; pattern mode loops this range.
    pub length: Time,
    /// All notes, for every channel. Kept sorted by (start, key).
    pub notes: Vec<Note>,
}

impl Pattern {
    pub fn notes_for(&self, channel: Id) -> impl Iterator<Item = &Note> {
        self.notes.iter().filter(move |n| n.channel == channel)
    }

    pub fn uses_channel(&self, channel: Id) -> bool {
        self.notes.iter().any(|n| n.channel == channel)
    }

    pub fn sort_notes(&mut self) {
        self.notes.sort_by(|a, b| {
            a.start
                .cmp(&b.start)
                .then(a.key.cmp(&b.key))
                .then(a.id.cmp(&b.id))
        });
    }

    /// End of the last note, or zero.
    pub fn content_end(&self) -> Time {
        self.notes
            .iter()
            .map(|n| n.start + n.length)
            .max()
            .unwrap_or(Time::ZERO)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: Id,
    pub channel: Id,
    pub start: Time,
    pub length: Time,
    pub key: u8,
    /// Attack velocity 0..=1.
    pub velocity: f32,
    /// Release velocity 0..=1.
    #[serde(default = "default_release")]
    pub release: f32,
    /// Per-note pan -1..=1.
    #[serde(default)]
    pub pan: f32,
    /// Fine pitch in cents.
    #[serde(default)]
    pub fine: f32,
    #[serde(default)]
    pub muted: bool,
    /// Trigger probability 0..=1 (1 = always).
    #[serde(default = "default_probability")]
    pub probability: f32,
}

fn default_release() -> f32 {
    0.5
}

fn default_probability() -> f32 {
    1.0
}

impl Note {
    pub fn new(channel: Id, start: Time, length: Time, key: u8, velocity: f32) -> Note {
        Note {
            id: Id::new(),
            channel,
            start,
            length,
            key,
            velocity,
            release: 0.5,
            pan: 0.0,
            fine: 0.0,
            muted: false,
            probability: 1.0,
        }
    }

    pub fn end(&self) -> Time {
        self.start + self.length
    }
}

/// An arrangement: lanes holding clips. Alternate arrangements share the
/// underlying patterns and media.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Arrangement {
    pub id: Id,
    pub name: String,
    pub tracks: Vec<Track>,
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Half-open loop range in quarter notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_range: Option<(Time, Time)>,
}

impl Arrangement {
    /// End of the last clip, or zero.
    pub fn end(&self) -> Time {
        self.clips
            .iter()
            .map(|c| c.start + c.length)
            .max()
            .unwrap_or(Time::ZERO)
    }
}

/// A playlist lane. Any clip type may be placed on any lane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: Id,
    pub track: Id,
    /// Arrangement position.
    pub start: Time,
    /// Visible length. A pattern clip longer than its pattern loops it.
    pub length: Time,
    /// Offset into the source where playback begins.
    #[serde(default)]
    pub offset: Time,
    #[serde(default)]
    pub muted: bool,
    pub content: ClipContent,
}

impl Clip {
    pub fn end(&self) -> Time {
        self.start + self.length
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipContent {
    /// A linked instance of a pattern source.
    Pattern { pattern: Id },
    /// Audio from an immutable asset, routed to a mixer track.
    Audio {
        asset: Id,
        #[serde(default)]
        gain_db: f32,
        #[serde(default)]
        reversed: bool,
        track: Id,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: Id,
    pub position: Time,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mixer {
    /// Mixer tracks. `tracks[0]` is always the master.
    pub tracks: Vec<MixerTrack>,
}

impl Mixer {
    pub fn master(&self) -> &MixerTrack {
        &self.tracks[0]
    }

    pub fn track(&self, id: Id) -> Option<&MixerTrack> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: Id) -> Option<&mut MixerTrack> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerTrack {
    pub id: Id,
    pub name: String,
    pub color: Color,
    pub gain_db: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    /// Insert effects in processing order.
    pub inserts: Vec<DeviceInstance>,
    /// Destination track (master when `None`; ignored on the master).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Id>,
}

/// An immutable audio file owned by or referenced from the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioAsset {
    pub id: Id,
    /// BLAKE3 hash of the original file bytes.
    pub hash: String,
    /// Original file name.
    pub name: String,
    /// Location: relative to the project's data directory when managed,
    /// absolute when referenced externally.
    pub path: String,
    #[serde(default)]
    pub managed: bool,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
}

impl Project {
    pub fn channel(&self, id: Id) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn channel_mut(&mut self, id: Id) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    pub fn pattern(&self, id: Id) -> Option<&Arc<Pattern>> {
        self.patterns.iter().find(|p| p.id == id)
    }

    /// Mutable access to a pattern, cloning it only if history shares it.
    pub fn pattern_mut(&mut self, id: Id) -> Option<&mut Pattern> {
        self.patterns
            .iter_mut()
            .find(|p| p.id == id)
            .map(Arc::make_mut)
    }

    pub fn arrangement(&self) -> &Arc<Arrangement> {
        self.arrangements
            .iter()
            .find(|a| a.id == self.active_arrangement)
            .unwrap_or(&self.arrangements[0])
    }

    pub fn arrangement_mut(&mut self) -> &mut Arrangement {
        let idx = self
            .arrangements
            .iter()
            .position(|a| a.id == self.active_arrangement)
            .unwrap_or(0);
        Arc::make_mut(&mut self.arrangements[idx])
    }

    pub fn asset(&self, id: Id) -> Option<&AudioAsset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// Song tempo (first tempo segment).
    pub fn bpm(&self) -> f64 {
        self.tempo.first().map_or(120.0, |s| s.bpm)
    }

    pub fn set_bpm(&mut self, bpm: f64) {
        let bpm = bpm.clamp(10.0, 999.0);
        match self.tempo.first_mut() {
            Some(s) => s.bpm = bpm,
            None => self.tempo.push(TempoSegment {
                start: Time::ZERO,
                bpm,
                ramp_to: None,
            }),
        }
    }

    /// Number of arrangement clips that reference a pattern.
    pub fn pattern_usage(&self, pattern: Id) -> usize {
        self.arrangements
            .iter()
            .flat_map(|a| a.clips.iter())
            .filter(|c| matches!(c.content, ClipContent::Pattern { pattern: p } if p == pattern))
            .count()
    }

    /// Next palette color not yet used by a channel, cycling when all are.
    pub fn next_channel_color(&self) -> Color {
        let n = self.channels.len();
        crate::color::PALETTE[n % crate::color::PALETTE.len()]
    }
}
