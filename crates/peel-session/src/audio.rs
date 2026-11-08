//! Audio clips on the Playlist: adding them, loading their audio and
//! answering the questions a waveform view asks.
//!
//! Decoding, resampling to the engine rate and the peak pyramid all happen on
//! the sample loader thread. The session only compiles a clip into the plan
//! once its audio has arrived; until then the clip is silent, and the arrival
//! bumps a generation counter so the next [`Session::tick`] recompiles even
//! though the document did not change.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use peel_document::{Clip, ClipContent, Id, MixerTrack, PALETTE, Project, Time};
use peel_dsp::SampleBuffer;
use peel_media::Peaks;
use peel_time::TempoMap;

use crate::Session;

/// A clip added from a file is rounded up to this many quarter notes (a
/// sixteenth note), so it covers the whole file without a long empty tail.
const CLIP_GRID: i64 = 4;

/// Session bookkeeping for audio clips.
#[derive(Default)]
pub(crate) struct AudioState {
    /// Incremented whenever audio used by a clip finishes loading.
    pub generation: u64,
    /// Assets that failed to decode. They are not requested again until the
    /// project is replaced, so a broken file does not retry on every edit.
    pub failed: HashSet<Id>,
}

/// Assets used by audio clips of the active arrangement (muted or not, so
/// every visible waveform has its audio).
pub(crate) fn clip_assets(p: &Project) -> Vec<Id> {
    let mut out: Vec<Id> = Vec::new();
    for clip in &p.arrangement().clips {
        if let ClipContent::Audio { asset, .. } = &clip.content
            && !out.contains(asset)
        {
            out.push(*asset);
        }
    }
    out
}

/// What a file contains: sample rate, channels, frames and seconds.
struct FileInfo {
    sample_rate: u32,
    channels: u16,
    frames: u64,
    seconds: f64,
}

/// Reads a file's header (decoding it only when the container does not state
/// its length, which some MP3 files do not).
fn describe(path: &Path) -> Result<FileInfo, String> {
    let info = peel_media::probe(path).map_err(|e| e.to_string())?;
    let rate = f64::from(info.sample_rate.max(1));
    let frames = match (info.frames, info.duration_seconds) {
        (Some(frames), _) => frames,
        (None, Some(seconds)) => (seconds * rate).round() as u64,
        (None, None) => peel_media::decode_file(path)
            .map_err(|e| e.to_string())?
            .frames() as u64,
    };
    Ok(FileInfo {
        sample_rate: info.sample_rate,
        channels: info.channels.min(usize::from(u16::MAX)) as u16,
        frames,
        seconds: frames as f64 / rate,
    })
}

/// Length in quarter notes of a clip that plays `seconds` of audio from
/// `start`, rounded up to a sixteenth note.
fn clip_length(tempo: &TempoMap, start: Time, seconds: f64) -> Time {
    let from = start.as_f64();
    let end = tempo.seconds_to_beat(tempo.beat_to_seconds(from) + seconds);
    let grid = CLIP_GRID as f64;
    let steps = ((end - from) * grid - 1e-9).ceil().max(1.0) as i64;
    Time::new(steps, CLIP_GRID)
}

/// The mixer track an audio clip on `lane` plays through. The lane keeps
/// using the track of its existing audio clips; failing that it adopts an
/// otherwise unused track named like the lane; failing that a new one is
/// created. A full mixer sends the clip to the master instead.
fn route_for_lane(
    p: &mut Project,
    lane: Id,
    name: &str,
    color: Option<peel_document::Color>,
) -> Id {
    let arr = p.arrangement();
    let audio_track = |c: &Clip| match c.content {
        ClipContent::Audio { track, .. } => Some(track),
        _ => None,
    };
    if let Some(existing) = arr
        .clips
        .iter()
        .rev()
        .filter(|c| c.track == lane)
        .filter_map(audio_track)
        .find(|t| p.mixer.track(*t).is_some())
    {
        return existing;
    }
    let master = p.mixer.master().id;
    let used_elsewhere = |t: Id| {
        p.channels.iter().any(|c| c.track == t)
            || arr
                .clips
                .iter()
                .any(|c| c.track != lane && audio_track(c) == Some(t))
    };
    if let Some(t) = p
        .mixer
        .tracks
        .iter()
        .skip(1)
        .find(|t| t.name == name && !used_elsewhere(t.id))
    {
        return t.id;
    }
    if p.mixer.tracks.len() >= peel_engine::MAX_TRACKS {
        return master;
    }
    let id = Id::new();
    let color = color.unwrap_or(PALETTE[p.mixer.tracks.len() % PALETTE.len()]);
    p.mixer.tracks.push(MixerTrack {
        id,
        name: name.to_string(),
        color,
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        solo: false,
        inserts: Vec::new(),
        output: None,
    });
    id
}

impl Session {
    /// Places an audio file on a Playlist lane as a clip, as one undoable
    /// step.
    ///
    /// The file is referenced in place (see
    /// [`reference_audio_file`](Session::reference_audio_file)) and added to
    /// the project's assets if new. The clip starts at `start`, plays the
    /// whole file at its native speed (no time stretching), and is as long as
    /// the file at the tempo in force there, rounded up to a sixteenth note.
    /// It routes to the mixer track of the lane's other audio clips, or to a
    /// new mixer track named after the lane.
    ///
    /// Decoding happens on the loader thread; the clip is silent until it
    /// finishes, and [`audio_peaks`](Session::audio_peaks) returns `None`
    /// until then. Returns the new clip's id.
    pub fn add_audio_clip(
        &mut self,
        track_lane: Id,
        start: Time,
        path: &Path,
    ) -> Result<Id, String> {
        let (lane_name, lane_color) = self
            .doc
            .project()
            .arrangement()
            .tracks
            .iter()
            .find(|t| t.id == track_lane)
            .map(|t| (t.name.clone(), t.color))
            .ok_or("That playlist track no longer exists")?;
        let start = start.max(Time::ZERO);

        let mut asset = self.reference_audio_file(path)?;
        let known = self.doc.project().asset(asset.id).is_some();
        let info = describe(path)?;
        if !known {
            asset.sample_rate = info.sample_rate;
            asset.channels = info.channels;
            asset.frames = info.frames;
        }
        let tempo = TempoMap::from_segments(&self.doc.project().tempo);
        let length = clip_length(&tempo, start, info.seconds);

        let clip_id = Id::new();
        let asset_id = asset.id;
        self.doc.edit("Add audio clip", |p| {
            if !known {
                p.assets.push(asset);
            }
            let track = route_for_lane(p, track_lane, &lane_name, lane_color);
            p.arrangement_mut().clips.push(Clip {
                id: clip_id,
                track: track_lane,
                start,
                length,
                offset: Time::ZERO,
                muted: false,
                content: ClipContent::audio(asset_id, track),
            });
        });
        // Start decoding now instead of at the next frame.
        self.sync();
        Ok(clip_id)
    }

    /// Waveform summary of an audio asset, for drawing clips with
    /// [`Peaks::query`](peel_media::Peaks::query). Built on the loader thread
    /// right after decoding and cached; `None` until then (and for assets
    /// that failed to load).
    ///
    /// The peaks cover the audio as decoded at the engine sample rate, so
    /// frame `n` of the pyramid is `n / sample_rate()` seconds into the file.
    pub fn audio_peaks(&self, asset: Id) -> Option<Arc<Peaks>> {
        self.samples.peaks(asset)
    }

    /// Length of an audio asset in seconds, from the project's record of the
    /// file or, for assets without one, from the decoded audio.
    pub fn asset_duration_seconds(&self, asset: Id) -> Option<f64> {
        if let Some(a) = self.doc.project().asset(asset)
            && a.sample_rate > 0
            && a.frames > 0
        {
            return Some(a.frames as f64 / f64::from(a.sample_rate));
        }
        let buf = self.samples.cached(asset)?;
        (buf.sample_rate > 0.0).then(|| buf.frames() as f64 / f64::from(buf.sample_rate))
    }

    /// True while the audio of an asset is being decoded.
    pub fn audio_loading(&self, asset: Id) -> bool {
        self.samples.is_pending(asset)
    }

    /// Decoded audio for the clips of the active arrangement. Assets that are
    /// not decoded yet are requested from the loader (once) and left out.
    pub(crate) fn clip_samples(&mut self) -> std::collections::HashMap<Id, Arc<SampleBuffer>> {
        let rate = self.engine.sample_rate();
        let mut out = std::collections::HashMap::new();
        for id in clip_assets(self.doc.project()) {
            if let Some(buf) = self.samples.cached(id) {
                out.insert(id, buf);
            } else if !self.audio.failed.contains(&id)
                && let Some(asset) = self.doc.project().asset(id).cloned()
            {
                let path = self.asset_path(&asset);
                self.samples.request(id, path, rate);
            }
        }
        out
    }

    /// Audio of unmuted clips that is not decoded yet, with where to read it
    /// from, for the export thread to load itself.
    pub(crate) fn export_loads(&self) -> Vec<(Id, PathBuf)> {
        let p = self.doc.project();
        let arr = p.arrangement();
        let mut out: Vec<(Id, PathBuf)> = Vec::new();
        for clip in &arr.clips {
            let ClipContent::Audio { asset, .. } = &clip.content else {
                continue;
            };
            let lane_muted = arr.tracks.iter().any(|t| t.id == clip.track && t.muted);
            if clip.muted
                || lane_muted
                || self.samples.cached(*asset).is_some()
                || out.iter().any(|(id, _)| id == asset)
            {
                continue;
            }
            if let Some(a) = p.asset(*asset) {
                out.push((*asset, self.asset_path(a)));
            }
        }
        out
    }

    /// Audio for a clip finished loading: recompile when a clip uses it.
    pub(crate) fn audio_loaded(&mut self, asset: Id) {
        self.audio.failed.remove(&asset);
        if clip_assets(self.doc.project()).contains(&asset) {
            self.audio.generation += 1;
        }
    }

    /// Audio for an asset could not be decoded.
    pub(crate) fn audio_failed(&mut self, asset: Id) {
        self.audio.failed.insert(asset);
    }
}
