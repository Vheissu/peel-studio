//! A small builder for the demo songs: instruments on their own mixer
//! tracks, buses, sends, keyed effects, patterns, playlist lanes, section
//! markers and automation, plus the note-writing helpers the songs share.

use std::sync::Arc;

use peel_document::{
    AutomationPoint, AutomationTarget, Clip, ClipContent, DeviceInstance, Id, Marker, MixerSend,
    MixerTrack, Note, PALETTE, Pattern, Project, Time, Track, db_to_fader,
};

use super::{add_channel, device};

/// One bar of 4/4, in quarter notes.
pub(super) const BAR: i64 = 4;

/// Drum voices all play their root.
pub(super) const HIT: u8 = 60;

/// A device with a preset and some parameters changed from it.
pub(super) fn tweaked(
    type_id: &str,
    preset: Option<&str>,
    params: &[(&str, f32)],
) -> DeviceInstance {
    let dev = device(type_id, preset);
    debug_assert_eq!(dev.preset.as_deref(), preset, "{type_id}");
    let mut dev = dev;
    for (k, v) in params {
        debug_assert!(dev.params.contains_key(*k), "{type_id}: {k}");
        dev.params.insert((*k).to_string(), *v);
    }
    dev
}

/// Normalized (0..=1) automation value for a plain parameter value.
pub(super) fn norm(type_id: &str, key: &str, plain: f32) -> f32 {
    peel_devices::descriptor(type_id)
        .and_then(|d| d.param_by_key(key))
        .map_or(0.5, |p| p.to_normalized(plain))
}

/// Normalized automation value for a track gain in dB.
pub(super) fn fader(db: f32) -> f32 {
    db_to_fader(db)
}

pub(super) struct Song {
    pub p: Project,
    lanes: Vec<Id>,
}

impl Song {
    pub fn new(title: &str, notes: &str, bpm: f64, swing: f32, lanes: &[&str]) -> Song {
        let mut p = Project::empty(title);
        p.meta.artist = "Peel Studio".into();
        p.meta.notes = notes.into();
        p.set_bpm(bpm);
        p.swing = swing;
        p.patterns.clear();
        let arr = p.arrangement_mut();
        while arr.tracks.len() < lanes.len() {
            arr.tracks.push(Track {
                id: Id::new(),
                name: String::new(),
                color: None,
                muted: false,
            });
        }
        for (t, name) in arr.tracks.iter_mut().zip(lanes) {
            t.name = (*name).to_string();
        }
        let lanes = arr.tracks.iter().map(|t| t.id).collect();
        Song { p, lanes }
    }

    pub fn master(&self) -> Id {
        self.p.mixer.tracks[0].id
    }

    /// Adds an instrument on its own mixer track at `gain_db`.
    #[allow(clippy::too_many_arguments)]
    pub fn inst(
        &mut self,
        name: &str,
        color: usize,
        type_id: &str,
        preset: &str,
        params: &[(&str, f32)],
        root: u8,
        gain_db: f32,
    ) -> Id {
        let dev = tweaked(type_id, Some(preset), params);
        let id = add_channel(&mut self.p, name, color, dev, root);
        if let Some(c) = self.p.channels.iter_mut().find(|c| c.id == id) {
            c.gain_db = gain_db;
        }
        id
    }

    /// The mixer track an instrument plays into.
    pub fn track(&self, ch: Id) -> Id {
        self.p.channel(ch).map(|c| c.track).unwrap_or_default()
    }

    /// The device id of an instrument, for automation.
    pub fn device(&self, ch: Id) -> Id {
        self.p.channel(ch).map(|c| c.device.id).unwrap_or_default()
    }

    fn mixer(&mut self, track: Id) -> &mut MixerTrack {
        self.p.mixer.track_mut(track).expect("song tracks exist")
    }

    /// Mixer bus with no instrument of its own.
    pub fn bus(&mut self, name: &str, color: usize, gain_db: f32) -> Id {
        let track = MixerTrack {
            id: Id::new(),
            name: name.into(),
            color: PALETTE[color % PALETTE.len()],
            gain_db,
            pan: 0.0,
            muted: false,
            solo: false,
            inserts: vec![],
            output: None,
            sends: Vec::new(),
        };
        let id = track.id;
        self.p.mixer.tracks.push(track);
        id
    }

    /// Sends these instruments' tracks to `bus` instead of the master.
    pub fn route(&mut self, parts: &[Id], bus: Id) {
        for &ch in parts {
            let t = self.track(ch);
            self.mixer(t).output = Some(bus);
        }
    }

    pub fn pan(&mut self, ch: Id, pan: f32) {
        let t = self.track(ch);
        self.mixer(t).pan = pan;
    }

    /// Adds an effect to the end of a mixer track's chain. Returns its
    /// device id.
    pub fn fx(
        &mut self,
        track: Id,
        type_id: &str,
        preset: Option<&str>,
        params: &[(&str, f32)],
    ) -> Id {
        let dev = tweaked(type_id, preset, params);
        let id = dev.id;
        self.mixer(track).inserts.push(dev);
        id
    }

    /// Adds an effect that listens to `key` (a mixer track) at the start
    /// of `track`'s chain: a ducking compressor or a keyed gate.
    pub fn keyed(
        &mut self,
        track: Id,
        key: Id,
        type_id: &str,
        preset: &str,
        params: &[(&str, f32)],
        first: bool,
    ) -> Id {
        let mut dev = tweaked(type_id, Some(preset), params);
        dev.sidechain = Some(key);
        let id = dev.id;
        let inserts = &mut self.mixer(track).inserts;
        if first {
            inserts.insert(0, dev);
        } else {
            inserts.push(dev);
        }
        id
    }

    /// A post-fader send from an instrument's track to a bus.
    pub fn send(&mut self, ch: Id, target: Id, gain_db: f32) {
        let t = self.track(ch);
        self.mixer(t).sends.push(MixerSend { target, gain_db });
    }

    pub fn pattern(&mut self, name: &str, color: usize, bars: i64, mut notes: Vec<Note>) -> Id {
        notes.sort_by_key(|n| (n.start, n.key));
        let pat = Pattern {
            id: Id::new(),
            name: name.into(),
            color: PALETTE[color % PALETTE.len()],
            length: Time::beats(bars * BAR),
            notes,
            expression: Vec::new(),
        };
        let id = pat.id;
        self.p.patterns.push(Arc::new(pat));
        id
    }

    /// Places `pattern` on `lane` from `bar` for `bars` bars.
    pub fn put(&mut self, lane: usize, pattern: Id, bar: i64, bars: i64) {
        let track = self.lanes[lane];
        self.p.arrangement_mut().clips.push(Clip {
            id: Id::new(),
            track,
            start: Time::beats(bar * BAR),
            length: Time::beats(bars * BAR),
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::Pattern { pattern },
        });
    }

    /// An automation clip covering the whole song, so every part of it
    /// (and every loop back to the start) sees a defined value. Points are
    /// `(bar, normalized value, tension)`.
    pub fn automate(
        &mut self,
        lane: usize,
        bars: i64,
        target: AutomationTarget,
        points: &[(f64, f32, f32)],
    ) {
        let track = self.lanes[lane];
        let points = points
            .iter()
            .map(|&(bar, value, tension)| AutomationPoint {
                position: Time::new((bar * 16.0).round() as i64, 4),
                value: value.clamp(0.0, 1.0),
                tension,
            })
            .collect();
        self.p.arrangement_mut().clips.push(Clip {
            id: Id::new(),
            track,
            start: Time::ZERO,
            length: Time::beats(bars * BAR),
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::Automation { target, points },
        });
    }

    /// Section markers as `(bar, name)`.
    pub fn sections(mut self, marks: &[(i64, &str)]) -> Project {
        let arr = self.p.arrangement_mut();
        for &(bar, name) in marks {
            arr.markers.push(Marker {
                id: Id::new(),
                position: Time::beats(bar * BAR),
                name: name.into(),
            });
        }
        self.p
    }
}

/// Automation target for a device parameter.
pub(super) fn param(device: Id, key: &str) -> AutomationTarget {
    AutomationTarget::Param {
        device,
        param: key.into(),
    }
}

/// A note on the sixteenth grid.
pub(super) fn n16(ch: Id, step: i64, len: i64, key: u8, vel: f32) -> Note {
    Note::new(ch, Time::new(step, 4), Time::new(len, 4), key, vel)
}

/// Drum hits from a step grid, one character per sixteenth: `X` accent,
/// `x` hit, `o` ghost and anything else a rest. `|` and spaces only make
/// bars easier to read.
pub(super) fn grid(ch: Id, steps: &str) -> Vec<Note> {
    steps
        .chars()
        .filter(|c| !matches!(c, '|' | ' '))
        .enumerate()
        .filter_map(|(i, c)| {
            let vel = match c {
                'X' => 1.0,
                'x' => 0.78,
                'o' => 0.42,
                _ => return None,
            };
            Some(n16(ch, i as i64, 1, HIT, vel))
        })
        .collect()
}

/// `count` evenly spaced hits from `start`, rising from `vel.0` to `vel.1`.
pub(super) fn roll(
    ch: Id,
    key: u8,
    start: Time,
    every: Time,
    count: i64,
    vel: (f32, f32),
) -> Vec<Note> {
    (0..count)
        .map(|i| {
            let t = i as f32 / (count - 1).max(1) as f32;
            let v = vel.0 + (vel.1 - vel.0) * t;
            Note::new(ch, start + every.mul_int(i), every, key, v)
        })
        .collect()
}

/// A line of `(step, length, key)` in sixteenths. Notes on the beat are a
/// little louder and the rest wander slightly, so it does not sound typed.
pub(super) fn line(ch: Id, notes: &[(i64, i64, u8)], vel: f32) -> Vec<Note> {
    notes
        .iter()
        .enumerate()
        .map(|(i, &(s, l, k))| {
            let accent = if s % 4 == 0 { 0.06 } else { -0.03 };
            let wander = [0.0, -0.04, 0.03, -0.02, 0.02][i % 5];
            n16(ch, s, l, k, (vel + accent + wander).clamp(0.1, 1.0))
        })
        .collect()
}

/// A bass line as `(step, length, key, slide)` in sixteenths. A note that
/// slides is lengthened to run into the following note, which a legato
/// instrument plays as a glide instead of a new attack; every other note
/// stops just short of the next, so that one always restarts.
pub(super) fn slides(ch: Id, notes: &[(i64, i64, u8, bool)], vel: f32) -> Vec<Note> {
    notes
        .iter()
        .enumerate()
        .map(|(i, &(s, l, k, slide))| {
            let accent = if s % 16 == 0 { 0.08 } else { 0.0 };
            let mut n = n16(ch, s, l, k, (vel + accent).min(1.0));
            if slide && let Some(next) = notes.get(i + 1) {
                // Reach an eighth of a beat into the next note.
                n.length = Time::new(next.0 - s, 4) + Time::new(1, 8);
            } else {
                n.length -= Time::new(1, 16);
            }
            n
        })
        .collect()
}

/// One chord per bar, struck at every `(step, length)`, optionally rolled
/// from the bottom up. `repeat` plays the progression that many times.
pub(super) fn chords(
    ch: Id,
    prog: &[&[u8]],
    hits: &[(i64, i64)],
    vel: f32,
    strum: i64,
) -> Vec<Note> {
    let mut v = Vec::new();
    for (bar, keys) in prog.iter().enumerate() {
        for &(step, len) in hits {
            for (i, &k) in keys.iter().enumerate() {
                let roll = Time::new(i as i64 * strum, 96);
                let start = Time::new(bar as i64 * 16 + step, 4) + roll;
                // Upper voices sit a touch under the bass note.
                let v_i = vel - 0.03 * i as f32;
                v.push(Note::new(ch, start, Time::new(len, 4), k, v_i.max(0.1)));
            }
        }
    }
    v
}

/// Chord tones cycled every `every` sixteenths in the order `shape`
/// (indexes into the chord, wrapping an octave up past its top).
pub(super) fn arp(ch: Id, prog: &[&[u8]], shape: &[usize], every: i64, vel: f32) -> Vec<Note> {
    let mut v = Vec::new();
    for (bar, keys) in prog.iter().enumerate() {
        for i in 0..16 / every {
            let idx = shape[i as usize % shape.len()];
            let key = keys[idx % keys.len()] + 12 * (idx / keys.len()) as u8;
            let accent = if i % 4 == 0 { 0.1 } else { 0.0 };
            v.push(n16(
                ch,
                bar as i64 * 16 + i * every,
                every,
                key,
                vel + accent,
            ));
        }
    }
    v
}
