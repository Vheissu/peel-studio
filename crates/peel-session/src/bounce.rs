//! Render to audio: a pattern clip becomes an audio clip.
//!
//! The clip is rendered on its own through its channels and mixer tracks
//! (without the master's processing, which still applies on playback) in
//! the background. When it is done, the audio lands on a new lane below the
//! clip, and the pattern clip is muted rather than deleted, as one undo step.

use std::path::PathBuf;

use peel_document::{AudioAsset, Clip, ClipContent, Id, Project, Time, Track};
use peel_time::{TempoMap, TempoSegment};

use crate::Session;
use crate::audio::{clip_length, describe, route_for_lane};
use crate::export::{ExportJob, ExportSettings};

/// Seconds rendered after the clip ends, for reverb and delay tails.
const TAIL_SECONDS: f32 = 2.0;

pub(crate) struct Bounce {
    job: ExportJob,
    clip: Id,
}

/// `project` reduced to one clip, moved to the start of the song with the
/// tempo, automation and mixer it hears there, and a neutral master.
fn isolate(project: &Project, clip: &Clip) -> Project {
    let mut p = project.clone();
    let at = clip.start;
    let end = clip.end();
    // Tempo as heard from the clip's start onwards.
    let map = TempoMap::from_segments(&project.tempo);
    let mut segs: Vec<TempoSegment> = Vec::new();
    let mut sorted = project.tempo.clone();
    sorted.sort_by_key(|s| s.start);
    let next_after = |t: Time| sorted.iter().map(|s| s.start).find(|s| *s > t);
    let first_ramp = sorted.iter().rev().find(|s| s.start <= at).and_then(|s| {
        s.ramp_to.map(|_| {
            next_after(at)
                .map(|n| map.bpm_at(n.as_f64()))
                .unwrap_or(s.bpm)
        })
    });
    segs.push(TempoSegment {
        start: Time::ZERO,
        bpm: map.bpm_at(at.as_f64()),
        ramp_to: first_ramp,
    });
    segs.extend(
        sorted
            .iter()
            .filter(|s| s.start > at)
            .map(|s| TempoSegment {
                start: s.start - at,
                ..*s
            }),
    );
    p.tempo = segs;
    // The clip and the automation overlapping it, shifted to zero.
    let arr = p.arrangement_mut();
    let mut clips = Vec::new();
    for c in &project.arrangement().clips {
        let keep =
            c.id == clip.id || (matches!(c.content, ClipContent::Automation { .. }) && !c.muted);
        if !keep || c.end() <= at || c.start >= end {
            continue;
        }
        let mut c = c.clone();
        if c.start < at {
            let cut = at - c.start;
            c.offset += cut;
            c.length -= cut;
            c.start = at;
        }
        c.start -= at;
        clips.push(c);
    }
    arr.clips = clips;
    arr.loop_range = None;
    arr.markers.clear();
    let master = &mut p.mixer.tracks[0];
    master.inserts.clear();
    master.gain_db = 0.0;
    master.pan = 0.0;
    master.muted = false;
    for t in &mut p.mixer.tracks {
        t.solo = false;
    }
    p
}

impl Session {
    /// Starts rendering a pattern clip to audio in the background. The
    /// result is placed when it finishes (see [`tick`](Session::tick)).
    pub fn render_clip_to_audio(&mut self, clip: Id) -> Result<(), String> {
        if self.bounce.is_some() {
            return Err("Already rendering a clip".into());
        }
        let c = self
            .project()
            .arrangement()
            .clips
            .iter()
            .find(|c| c.id == clip)
            .cloned()
            .ok_or("That clip no longer exists")?;
        if !matches!(c.content, ClipContent::Pattern { .. }) {
            return Err("Only pattern clips can be rendered to audio".into());
        }
        let dir = self.recordings_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        let path: PathBuf = dir.join(format!("render-{:08x}.wav", Id::new().as_u128() as u32));
        let project = isolate(self.project(), &c);
        let rate = self.sample_rate().round() as u32;
        let settings = ExportSettings {
            path,
            sample_rate: rate.max(44_100),
            bits: 32,
            tail_seconds: TAIL_SECONDS,
        };
        self.capture_plugin_states();
        let (prebuilt, handles) = self.offline_plugins(&project, settings.sample_rate);
        let samples = self.cached_project_samples();
        let job = ExportJob::start(
            project,
            samples,
            self.export_loads(),
            prebuilt,
            handles,
            settings,
        );
        self.bounce = Some(Bounce { job, clip });
        Ok(())
    }

    /// Progress of a running render to audio, 0..=1.
    pub fn render_progress(&self) -> Option<f32> {
        self.bounce.as_ref().map(|b| b.job.progress())
    }

    /// Places a finished render. Called from `tick`.
    pub(crate) fn poll_bounce(&mut self) {
        let Some(b) = &self.bounce else { return };
        b.job.idle();
        let Some(result) = b.job.take_result() else {
            return;
        };
        let clip = b.clip;
        self.bounce = None;
        match result.and_then(|path| self.place_render(clip, &path)) {
            Ok(()) => self.notify("Rendered to audio", false),
            Err(e) => self.notify(format!("Could not render to audio: {e}"), true),
        }
    }

    fn place_render(&mut self, clip: Id, path: &std::path::Path) -> Result<(), String> {
        let src = self
            .project()
            .arrangement()
            .clips
            .iter()
            .find(|c| c.id == clip)
            .cloned()
            .ok_or("The clip was deleted while it rendered")?;
        let name = match src.content {
            ClipContent::Pattern { pattern } => self
                .project()
                .pattern(pattern)
                .map_or("Render".into(), |p| p.name.clone()),
            _ => "Render".into(),
        };
        let mut asset: AudioAsset = self.reference_audio_file(path)?;
        let info = describe(path)?;
        asset.sample_rate = info.sample_rate;
        asset.channels = info.channels;
        asset.frames = info.frames;
        let tempo = TempoMap::from_segments(&self.project().tempo);
        let length = clip_length(&tempo, src.start, info.seconds);
        let asset_id = asset.id;
        let lane_name = format!("{name} audio");
        self.doc.edit("Render to audio", |p| {
            crate::inherit_audio_credits(p, &asset);
            if p.asset(asset_id).is_none() {
                p.assets.push(asset);
            }
            let arr = p.arrangement_mut();
            let at = arr
                .tracks
                .iter()
                .position(|t| t.id == src.track)
                .map_or(arr.tracks.len(), |i| i + 1);
            let lane = Track {
                id: Id::new(),
                name: lane_name.clone(),
                color: None,
                muted: false,
            };
            let lane_id = lane.id;
            arr.tracks.insert(at, lane);
            if let Some(c) = arr.clips.iter_mut().find(|c| c.id == clip) {
                c.muted = true;
            }
            let track = route_for_lane(p, lane_id, &lane_name, None);
            p.arrangement_mut().clips.push(Clip {
                id: Id::new(),
                track: lane_id,
                start: src.start,
                length,
                offset: Time::ZERO,
                muted: false,
                content: ClipContent::audio(asset_id, track),
            });
        });
        Ok(())
    }
}
