//! Performance mode: launching clips live.
//!
//! The Playlist's lanes are launch lanes and each lane's distinct clips are
//! its slots; slot N of every lane makes scene N. A launch starts on the
//! next bar and replaces whatever the lane was playing; a stop ends on the
//! next bar. Nothing here is a separate sequencer: the session compiles an
//! arrangement made of the launched clips and plays it with the ordinary
//! engine, extending it as the performance runs. Captured performances
//! become a new arrangement holding the launches as they were heard.

use std::collections::HashMap;
use std::sync::Arc;

use peel_document::{Arrangement, Clip, ClipContent, Id, Project, Time};

use crate::{PlayMode, Session};

/// Beats compiled ahead of the playhead; extended as it approaches.
const AHEAD: i64 = 256;

#[derive(Clone, Debug)]
struct Launch {
    template: Clip,
    start: Time,
    stop: Option<Time>,
}

#[derive(Default)]
pub(crate) struct Performance {
    /// Per lane: what is playing or queued, oldest first (a clip being
    /// replaced stays until its stop).
    lanes: HashMap<Id, Vec<Launch>>,
    /// Launches heard while capturing, with their ends once known.
    log: Vec<(Id, Launch)>,
    capturing: bool,
    horizon: Time,
}

/// A launchable clip in the performance grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub clip: Id,
    pub name: String,
    pub color: peel_document::Color,
}

/// What a slot is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    Idle,
    /// Starts on the next bar.
    Queued,
    Playing,
    /// Stops on the next bar.
    Stopping,
}

fn source_key(c: &Clip) -> Option<String> {
    match &c.content {
        ClipContent::Pattern { pattern } => Some(format!("p{pattern}")),
        ClipContent::Audio { asset, .. } => Some(format!("a{asset}")),
        ClipContent::Automation { .. } => None,
    }
}

/// Distinct clips per lane, in the order they first appear.
pub(crate) fn lane_slots(p: &Project) -> Vec<(Id, Vec<Clip>)> {
    let arr = p.arrangement();
    arr.tracks
        .iter()
        .map(|t| {
            let mut clips: Vec<&Clip> = arr.clips.iter().filter(|c| c.track == t.id).collect();
            clips.sort_by_key(|c| c.start);
            let mut seen = Vec::new();
            let mut out = Vec::new();
            for c in clips {
                if let Some(k) = source_key(c)
                    && !seen.contains(&k)
                {
                    seen.push(k);
                    out.push(c.clone());
                }
            }
            (t.id, out)
        })
        .collect()
}

/// A stable id for one occurrence of a launched clip.
fn occurrence(template: Id, start: Time, n: u64) -> Id {
    let s = (start.as_f64() * 960.0).round() as u128;
    Id::from_u128(
        template.as_u128() ^ s.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (u128::from(n) << 64) ^ 0x5eed,
    )
}

/// The clips a launch plays between `start` and `end`: one long clip for a
/// pattern (which loops inside it), repeats for audio.
fn occurrences(l: &Launch, end: Time) -> Vec<Clip> {
    if end <= l.start {
        return Vec::new();
    }
    match &l.template.content {
        ClipContent::Pattern { .. } => vec![Clip {
            id: occurrence(l.template.id, l.start, 0),
            start: l.start,
            length: end - l.start,
            offset: Time::ZERO,
            muted: false,
            ..l.template.clone()
        }],
        _ => {
            let len = l.template.length.max(Time::new(1, 4));
            let mut v = Vec::new();
            let mut at = l.start;
            let mut n = 0;
            while at < end && n < 512 {
                v.push(Clip {
                    id: occurrence(l.template.id, l.start, n),
                    start: at,
                    length: len.min(end - at),
                    muted: false,
                    ..l.template.clone()
                });
                at += len;
                n += 1;
            }
            v
        }
    }
}

impl Performance {
    /// The project as it plays now: the arrangement holds the launches.
    fn project(&self, base: &Project) -> Project {
        let mut p = base.clone();
        let horizon = self.horizon;
        let arr = p.arrangement_mut();
        arr.clips = self
            .lanes
            .values()
            .flatten()
            .flat_map(|l| occurrences(l, l.stop.unwrap_or(horizon).min(horizon)))
            .collect();
        arr.loop_range = None;
        p
    }
}

impl Session {
    /// True while clips are being launched (the Playlist is not playing).
    pub fn performing(&self) -> bool {
        self.perform.is_some()
    }

    /// The launch grid: each lane with its slots.
    pub fn performance_slots(&self) -> Vec<(Id, Vec<Slot>)> {
        let p = self.project();
        lane_slots(p)
            .into_iter()
            .map(|(lane, clips)| {
                let slots = clips
                    .iter()
                    .map(|c| {
                        let (name, color) = match &c.content {
                            ClipContent::Pattern { pattern } => p
                                .pattern(*pattern)
                                .map_or(("Pattern".into(), peel_document::PALETTE[0]), |x| {
                                    (x.name.clone(), x.color)
                                }),
                            ClipContent::Audio { asset, .. } => (
                                p.asset(*asset).map_or("Audio".into(), |a| a.name.clone()),
                                peel_document::PALETTE[7],
                            ),
                            ClipContent::Automation { .. } => {
                                ("Automation".into(), peel_document::PALETTE[3])
                            }
                        };
                        Slot {
                            clip: c.id,
                            name,
                            color,
                        }
                    })
                    .collect();
                (lane, slots)
            })
            .collect()
    }

    pub fn slot_state(&self, lane: Id, clip: Id) -> SlotState {
        let Some(perf) = &self.perform else {
            return SlotState::Idle;
        };
        let now = Time::from_f64(self.position().max(0.0), 960);
        let Some(list) = perf.lanes.get(&lane) else {
            return SlotState::Idle;
        };
        for l in list.iter().rev().filter(|l| l.template.id == clip) {
            if l.start > now {
                return SlotState::Queued;
            }
            match l.stop {
                Some(s) if s <= now => continue,
                Some(_) => return SlotState::Stopping,
                None => return SlotState::Playing,
            }
        }
        SlotState::Idle
    }

    /// The next bar line after the playhead (or 0 when stopped).
    fn next_bar(&self) -> Time {
        if !self.is_playing() {
            return Time::ZERO;
        }
        let bar = self.project().time_signature.bar_length();
        Time::from_f64(self.position().max(0.0), 960).floor_to(bar) + bar
    }

    fn begin_performance(&mut self) {
        if self.perform.is_none() {
            // Launches play from the top of a fresh, free-running timeline.
            self.stop();
            self.perform = Some(Performance::default());
            self.set_mode(PlayMode::Song);
        }
    }

    /// Launches a slot on the next bar, replacing what its lane plays.
    pub fn launch(&mut self, lane: Id, clip: Id) {
        self.begin_performance();
        let at = self.next_bar();
        self.launch_at(lane, clip, at);
    }

    fn launch_at(&mut self, lane: Id, clip: Id, at: Time) {
        let Some(template) = self
            .project()
            .arrangement()
            .clips
            .iter()
            .find(|c| c.id == clip)
            .cloned()
        else {
            return;
        };
        self.begin_performance();
        let perf = self.perform.as_mut().expect("performing");
        let list = perf.lanes.entry(lane).or_default();
        // Anything still queued for this lane is replaced; what plays stops
        // where the new clip starts.
        list.retain(|l| l.start < at);
        for l in list.iter_mut() {
            if l.stop.is_none_or(|s| s > at) {
                l.stop = Some(at);
            }
        }
        list.push(Launch {
            template,
            start: at,
            stop: None,
        });
        self.performance_changed(at);
    }

    /// Stops a lane on the next bar.
    pub fn stop_lane(&mut self, lane: Id) {
        let at = self.next_bar();
        self.stop_lane_at(lane, at);
    }

    fn stop_lane_at(&mut self, lane: Id, at: Time) {
        let Some(perf) = self.perform.as_mut() else {
            return;
        };
        if let Some(list) = perf.lanes.get_mut(&lane) {
            list.retain(|l| l.start < at);
            for l in list.iter_mut() {
                if l.stop.is_none_or(|s| s > at) {
                    l.stop = Some(at);
                }
            }
        }
        self.performance_changed(at);
    }

    /// Launches slot `index` of every lane on the same bar; lanes without
    /// one stop there.
    pub fn launch_scene(&mut self, index: usize) {
        self.begin_performance();
        let at = self.next_bar();
        for (lane, slots) in lane_slots(self.project()) {
            match slots.get(index) {
                Some(c) => self.launch_at(lane, c.id, at),
                None => self.stop_lane_at(lane, at),
            }
        }
    }

    /// Stops every lane and leaves performance mode (keeping a capture).
    pub fn stop_performance(&mut self) {
        if self.perform.is_none() {
            return;
        }
        if self.performance_capturing() {
            self.set_performance_capture(false);
        }
        self.perform = None;
        self.stop();
        self.synced = None;
    }

    pub fn performance_capturing(&self) -> bool {
        self.perform.as_ref().is_some_and(|p| p.capturing)
    }

    /// Starts or ends capture. Ending it writes what was heard into a new
    /// arrangement and makes it active.
    pub fn set_performance_capture(&mut self, on: bool) {
        if on {
            self.begin_performance();
            if let Some(p) = self.perform.as_mut() {
                p.capturing = true;
                p.log.clear();
                // What is already playing is part of the take.
                let lanes: Vec<(Id, Launch)> = p
                    .lanes
                    .iter()
                    .flat_map(|(k, v)| v.iter().map(|l| (*k, l.clone())))
                    .collect();
                p.log = lanes;
            }
            return;
        }
        let end = self.next_bar();
        let Some(p) = self.perform.as_mut() else {
            return;
        };
        if !p.capturing {
            return;
        }
        p.capturing = false;
        let mut taken: Vec<(Id, Launch)> = std::mem::take(&mut p.log);
        // Close what is still playing at the next bar, and take the latest
        // stops of logged launches from the live state.
        for (lane, l) in &mut taken {
            if let Some(live) = p.lanes.get(lane).and_then(|v| {
                v.iter()
                    .find(|x| x.start == l.start && x.template.id == l.template.id)
            }) {
                l.stop = live.stop;
            }
            l.stop = Some(l.stop.unwrap_or(end).min(end));
        }
        taken.retain(|(_, l)| l.stop.is_some_and(|s| s > l.start));
        if taken.is_empty() {
            return;
        }
        // Start the take at its first launch.
        let first = taken
            .iter()
            .map(|(_, l)| l.start)
            .min()
            .unwrap_or(Time::ZERO);
        let base = self.project().arrangement().as_ref().clone();
        let n = self.project().arrangements.len() + 1;
        // The take gets lanes of its own, mirroring the source lanes.
        let lanes: HashMap<Id, Id> = base.tracks.iter().map(|t| (t.id, Id::new())).collect();
        let tracks = base
            .tracks
            .iter()
            .map(|t| peel_document::Track {
                id: lanes[&t.id],
                ..t.clone()
            })
            .collect();
        let clips: Vec<Clip> = taken
            .iter()
            .flat_map(|(_, l)| {
                let shifted = Launch {
                    start: l.start - first,
                    stop: l.stop.map(|s| s - first),
                    template: l.template.clone(),
                };
                occurrences(&shifted, shifted.stop.unwrap_or(shifted.start))
            })
            .map(|c| Clip {
                id: Id::new(),
                track: lanes.get(&c.track).copied().unwrap_or(c.track),
                ..c
            })
            .collect();
        let arr = Arrangement {
            id: Id::new(),
            name: format!("Performance {n}"),
            tracks,
            clips,
            markers: Vec::new(),
            loop_range: None,
        };
        let aid = arr.id;
        self.doc.edit("Capture performance", |p| {
            p.arrangements.push(Arc::new(arr));
            p.active_arrangement = aid;
        });
    }

    fn performance_changed(&mut self, at: Time) {
        let ahead = Time::beats(AHEAD);
        if let Some(p) = self.perform.as_mut() {
            if p.capturing {
                for (lane, list) in &p.lanes {
                    for l in list {
                        if !p.log.iter().any(|(k, x)| {
                            k == lane && x.start == l.start && x.template.id == l.template.id
                        }) {
                            p.log.push((*lane, l.clone()));
                        }
                    }
                }
            }
            p.horizon = p.horizon.max(at + ahead);
        }
        self.synced = None;
        if !self.is_playing() {
            self.seek(0.0);
            self.play();
        }
    }

    /// The project the plan is compiled from while performing.
    pub(crate) fn performance_project(&self) -> Option<Project> {
        self.perform.as_ref().map(|p| p.project(self.doc.project()))
    }

    /// Keeps the compiled performance ahead of the playhead.
    pub(crate) fn extend_performance(&mut self) {
        let now = Time::from_f64(self.position().max(0.0), 960);
        let Some(p) = self.perform.as_mut() else {
            return;
        };
        if now + Time::beats(AHEAD / 2) > p.horizon {
            p.horizon = now + Time::beats(AHEAD);
            self.synced = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template;

    fn clips_on(p: &Project, lane: Id) -> Vec<Clip> {
        let mut v: Vec<Clip> = p
            .arrangement()
            .clips
            .iter()
            .filter(|c| c.track == lane)
            .cloned()
            .collect();
        v.sort_by_key(|c| c.start);
        v
    }

    #[test]
    fn lanes_offer_their_distinct_clips_as_slots() {
        let s = Session::new_silent(template::beat_and_melody());
        let slots = s.performance_slots();
        assert_eq!(slots[0].1.len(), 1, "the Beat lane has one pattern");
        assert_eq!(slots[0].1[0].name, "Beat");
        assert!(
            slots.iter().skip(3).all(|(_, s)| s.is_empty()),
            "empty lanes have no slots"
        );
    }

    #[test]
    fn launching_plays_from_the_top_and_switches_on_the_bar() {
        let mut s = Session::new_silent(template::beat_and_melody());
        let slots = s.performance_slots();
        let (drums, beat) = (slots[0].0, slots[0].1[0].clip);
        let (chords_lane, chords) = (slots[1].0, slots[1].1[0].clip);
        s.launch(drums, beat);
        assert!(s.performing() && s.is_playing());
        let p = s.performance_project().unwrap();
        let d = clips_on(&p, drums);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].start, Time::ZERO);
        assert!(d[0].length >= Time::beats(AHEAD), "plays on");
        // A launch while playing waits for the next bar.
        s.launch(chords_lane, chords);
        let p = s.performance_project().unwrap();
        let c = clips_on(&p, chords_lane);
        let bar = Time::beats(4);
        assert!(
            c[0].start > Time::ZERO && c[0].start.rem_euclid(bar) == Time::ZERO,
            "{:?}",
            c[0].start
        );
        // Stopping the drums ends them on a bar.
        s.stop_lane(drums);
        let p = s.performance_project().unwrap();
        let d = clips_on(&p, drums);
        assert!(d[0].end().rem_euclid(bar) == Time::ZERO);
        assert!(d[0].end() <= c[0].start + bar);
        s.stop_performance();
        assert!(!s.performing());
    }

    #[test]
    fn a_captured_performance_becomes_an_arrangement() {
        let mut s = Session::new_silent(template::beat_and_melody());
        let slots = s.performance_slots();
        let arrangements = s.project().arrangements.len();
        s.set_performance_capture(true);
        s.launch_scene(0);
        s.set_performance_capture(false);
        let p = s.project();
        assert_eq!(p.arrangements.len(), arrangements + 1);
        let take = p.arrangement();
        assert!(take.name.starts_with("Performance"));
        // Every lane with a slot was captured, starting at zero.
        let lanes_with_slots = slots.iter().filter(|(_, s)| !s.is_empty()).count();
        for i in 0..lanes_with_slots {
            let on = clips_on(p, take.tracks[i].id);
            assert!(!on.is_empty(), "lane {i} captured");
            assert_eq!(on[0].start, Time::ZERO);
        }
        assert!(peel_document::validate(p).iter().all(|i| !i.is_error()));
        assert_eq!(s.doc.undo_label(), Some("Capture performance"));
    }
}
