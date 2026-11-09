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

use peel_document::{AudioAsset, Clip, ClipContent, Id, MixerTrack, PALETTE, Project, Time};
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
    /// Time-stretched copies for tempo-following clips, by stretched key.
    pub stretched: std::collections::HashMap<Id, Arc<SampleBuffer>>,
    /// Stretches running in the background: asset -> stretched key. One
    /// per asset at a time, so dragging the tempo does not queue dozens.
    pub stretching: std::collections::HashMap<Id, Id>,
    pub stretch_tx: Option<std::sync::mpsc::Sender<(Id, SampleBuffer)>>,
    pub stretch_rx: Option<std::sync::mpsc::Receiver<(Id, SampleBuffer)>>,
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
pub(crate) struct FileInfo {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) frames: u64,
    pub(crate) seconds: f64,
}

/// Reads a file's header (decoding it only when the container does not state
/// its length, which some MP3 files do not).
pub(crate) fn describe(path: &Path) -> Result<FileInfo, String> {
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
pub(crate) fn clip_length(tempo: &TempoMap, start: Time, seconds: f64) -> Time {
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
pub(crate) fn route_for_lane(
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
        sends: Vec::new(),
    });
    id
}

/// One audio clip to place on the Playlist.
pub(crate) struct Placement {
    pub lane: Id,
    pub start: Time,
    pub length: Time,
    /// Where in the file the clip's first audible frame lies, in quarter
    /// notes (see [`Clip::offset`](peel_document::Clip)).
    pub offset: Time,
}

impl Session {
    /// Adds `placements` of one asset as clips, in a single undoable step
    /// named `label`, and registers the asset first if it is `new_asset`.
    /// Every clip routes like [`add_audio_clip`](Session::add_audio_clip)'s
    /// does. Placements on lanes that no longer exist are skipped. Returns
    /// the ids of the clips added, in placement order.
    pub(crate) fn place_audio_clips(
        &mut self,
        label: &str,
        new_asset: Option<AudioAsset>,
        asset: Id,
        placements: &[Placement],
    ) -> Vec<Id> {
        // Adding a file again is an explicit retry for one that failed before.
        self.audio.failed.remove(&asset);
        let ids: Vec<Id> = placements.iter().map(|_| Id::new()).collect();
        let mut placed = Vec::new();
        self.doc.edit(label, |p| {
            if let Some(a) = new_asset {
                p.assets.push(a);
            }
            for (pl, id) in placements.iter().zip(&ids) {
                let Some((name, color)) = p
                    .arrangement()
                    .tracks
                    .iter()
                    .find(|t| t.id == pl.lane)
                    .map(|t| (t.name.clone(), t.color))
                else {
                    continue;
                };
                let track = route_for_lane(p, pl.lane, &name, color);
                p.arrangement_mut().clips.push(Clip {
                    id: *id,
                    track: pl.lane,
                    start: pl.start,
                    length: pl.length,
                    offset: pl.offset,
                    muted: false,
                    content: ClipContent::audio(asset, track),
                });
                placed.push(*id);
            }
        });
        placed
    }

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
        if !self
            .doc
            .project()
            .arrangement()
            .tracks
            .iter()
            .any(|t| t.id == track_lane)
        {
            return Err("That playlist track no longer exists".into());
        }
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

        let asset_id = asset.id;
        let placed = self.place_audio_clips(
            "Add audio clip",
            (!known).then_some(asset),
            asset_id,
            &[Placement {
                lane: track_lane,
                start,
                length,
                offset: Time::ZERO,
            }],
        );
        // Start decoding now instead of at the next frame.
        self.sync();
        placed
            .first()
            .copied()
            .ok_or_else(|| "That playlist track no longer exists".to_string())
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
        // Stretched copies for clips that follow the song tempo, made in
        // the background the first time a ratio is needed.
        let tempo = TempoMap::from_segments(&self.doc.project().tempo);
        let wanted: Vec<(Id, Id, f64)> = self
            .doc
            .project()
            .arrangement()
            .clips
            .iter()
            .filter_map(|c| {
                let ClipContent::Audio { asset, .. } = &c.content else {
                    return None;
                };
                let r = crate::compile::stretch_ratio(&tempo, c)?;
                Some((*asset, crate::compile::stretched_key(*asset, r), r))
            })
            .collect();
        let keep: HashSet<Id> = wanted.iter().map(|w| w.1).collect();
        self.audio.stretched.retain(|k, _| keep.contains(k));
        for (asset, key, ratio) in wanted {
            if let Some(buf) = self.audio.stretched.get(&key) {
                out.insert(key, buf.clone());
                continue;
            }
            let Some(src) = out.get(&asset).cloned() else {
                continue;
            };
            if self.audio.stretching.contains_key(&asset) {
                continue;
            }
            self.audio.stretching.insert(asset, key);
            if self.audio.stretch_tx.is_none() {
                let (tx, rx) = std::sync::mpsc::channel();
                self.audio.stretch_tx = Some(tx);
                self.audio.stretch_rx = Some(rx);
            }
            let tx = self.audio.stretch_tx.clone();
            std::thread::Builder::new()
                .name("peel-stretch".into())
                .spawn(move || {
                    let buf = crate::export::stretched(&src, ratio);
                    if let Some(tx) = tx {
                        let _ = tx.send((key, buf));
                    }
                })
                .ok();
        }
        out
    }

    /// Takes finished stretches; true when any arrived (the plan must be
    /// rebuilt to use them).
    pub(crate) fn poll_stretches(&mut self) -> bool {
        let Some(rx) = &self.audio.stretch_rx else {
            return false;
        };
        let mut any = false;
        while let Ok((key, buf)) = rx.try_recv() {
            self.audio.stretching.retain(|_, k| *k != key);
            self.audio.stretched.insert(key, Arc::new(buf));
            any = true;
        }
        if any {
            self.audio.generation += 1;
        }
        any
    }

    /// True while a tempo-following clip's audio is being stretched.
    pub fn stretching(&self) -> bool {
        !self.audio.stretching.is_empty()
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

#[cfg(test)]
#[path = "audio_tests.rs"]
mod tests;
