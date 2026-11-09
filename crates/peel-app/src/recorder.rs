//! Pattern recording uses the same accepted performance feed for every input.
//! Note identity keeps equal pitches independent; expression follows the note.

use std::collections::HashMap;

use peel_document::{ExpressionPoint, ExpressionValue, Id, Note, NoteExpression, Time};
use peel_session::{PlayMode, RecordedPerformance, RecordedPerformanceKind, Session};
use peel_time::TempoMap;

const PPQ: i64 = 9_600;
const MAX_HELD: usize = 2_048;
const MAX_SNAPSHOTS: usize = 65_536;
const MAX_TAKE_NOTES: usize = 16_384;

struct Held {
    key: u8,
    velocity: f32,
    start: f64,
    snapshots: Vec<(f64, ExpressionValue)>,
}

/// Unwrap a looping playhead. Input timestamps also recover whole loops when
/// the UI stalls while a static note is held and no controllers are moving.
#[derive(Default)]
struct Clock {
    source_seconds: f64,
    anchor: Option<(u64, f64)>,
    cursor: f64,
}
impl Clock {
    fn position(&mut self, raw: f64, stamp: Option<u64>, take: &TakeTiming) -> f64 {
        let phase = take.tempo.beat_to_seconds(raw.rem_euclid(take.length));
        let period = take.tempo.beat_to_seconds(take.length).max(1e-6);
        let predicted = match (stamp, self.anchor) {
            (Some(now), Some((then, seconds))) if now >= then => {
                seconds + (now - then) as f64 * 1e-9
            }
            _ => self.source_seconds,
        };
        let epoch = ((predicted - phase) / period).round().max(0.0);
        let source = phase + epoch * period;
        self.source_seconds = self.source_seconds.max(source);
        if let Some(stamp) = stamp {
            self.anchor = Some((stamp, source));
        }
        let heard = (source - take.latency_seconds).max(0.0);
        let loop_number = (heard / period).floor();
        let beat =
            loop_number * take.length + take.tempo.seconds_to_beat(heard - loop_number * period);
        self.cursor = self.cursor.max(beat);
        beat
    }
}

struct TakeTiming {
    length: f64,
    tempo: TempoMap,
    latency_seconds: f64,
}
struct Take {
    project: Id,
    pattern: Id,
    timing: TakeTiming,
    clock: Clock,
    notes: usize,
    awaiting_note: bool,
}

#[derive(Default)]
pub struct Recorder {
    pub armed: bool,
    held: HashMap<(Id, u32), Held>,
    take: Option<Take>,
    gesture: Option<u64>,
    takes: u64,
    snapshots: usize,
    warned: bool,
    overflow_warned: bool,
    dropped: u64,
    audio_tried: bool,
}

impl Recorder {
    fn latency_seconds(s: &Session) -> f64 {
        (f64::from(s.buffer_frames()) * 2.0 + f64::from(s.compensation_latency_samples()))
            / f64::from(s.sample_rate().max(1.0))
    }

    #[cfg(test)]
    fn latency_beats(s: &Session) -> f64 {
        Self::latency_seconds(s) * s.project().bpm() / 60.0
    }

    fn active(&mut self, s: &mut Session) -> Option<Id> {
        if !self.armed || !s.is_playing() {
            return None;
        }
        match s.mode() {
            PlayMode::Pattern(p) => Some(p),
            PlayMode::Song => {
                if !self.warned && s.armed_lanes().is_empty() {
                    s.notify(
                        "Switch to Pattern mode to record notes into a pattern",
                        true,
                    );
                    self.warned = true;
                }
                None
            }
        }
    }

    fn begin(&mut self, s: &Session, pattern: Id) {
        let Some(pattern) = s.project().pattern(pattern) else {
            return;
        };
        let length = pattern.length.as_f64();
        if !length.is_finite() || length <= 0.0 {
            return;
        }
        self.take = Some(Take {
            project: s.project().id,
            pattern: pattern.id,
            timing: TakeTiming {
                length,
                tempo: TempoMap::from_segments(&s.project().tempo),
                latency_seconds: Self::latency_seconds(s),
            },
            clock: Clock::default(),
            notes: 0,
            awaiting_note: false,
        });
        self.overflow_warned = false;
        self.snapshots = 0;
    }

    fn overflow(&mut self, s: &mut Session) {
        if !self.overflow_warned {
            s.notify(
                "Recording limit reached; some notes or expression were not captured",
                true,
            );
            self.overflow_warned = true;
        }
    }

    fn consume(&mut self, s: &mut Session, event: RecordedPerformance) {
        if !event.beat.is_finite()
            || (!event.playing && event.kind != RecordedPerformanceKind::Reset)
        {
            return;
        }
        let Some(take) = &mut self.take else { return };
        if take.project != s.project().id {
            return;
        }
        if matches!(event.kind, RecordedPerformanceKind::NoteOn { .. }) {
            take.awaiting_note = false;
        }
        let pos = if event.playing {
            take.clock.position(
                event.beat,
                (event.time_ns > 0).then_some(event.time_ns),
                &take.timing,
            )
        } else {
            take.clock.cursor
        };
        if event.kind == RecordedPerformanceKind::Reset {
            self.flush(s, event.channel, pos);
            if self.held.is_empty()
                && let Some(take) = &mut self.take
            {
                take.clock = Clock::default();
                take.awaiting_note = true;
            }
            return;
        }
        let Some(channel) = event
            .channel
            .filter(|id| s.project().channel(*id).is_some())
        else {
            return;
        };
        let identity = (channel, event.voice);
        match event.kind {
            RecordedPerformanceKind::NoteOn { key, velocity } => {
                if key > 127 || !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
                    return;
                }
                if let Some(previous) = self.held.remove(&identity) {
                    self.snapshots = self.snapshots.saturating_sub(previous.snapshots.len());
                    self.store(s, channel, previous, pos, 0.5);
                }
                if self.held.len() >= MAX_HELD || self.snapshots >= MAX_SNAPSHOTS {
                    self.overflow(s);
                    return;
                }
                self.held.insert(
                    identity,
                    Held {
                        key,
                        velocity,
                        start: pos,
                        snapshots: vec![(pos, ExpressionValue::default())],
                    },
                );
                self.snapshots += 1;
            }
            RecordedPerformanceKind::NoteOff { velocity, .. } => {
                if let Some(note) = self.held.remove(&identity) {
                    self.snapshots = self.snapshots.saturating_sub(note.snapshots.len());
                    self.store(
                        s,
                        channel,
                        note,
                        pos,
                        if velocity.is_finite() {
                            velocity.clamp(0.0, 1.0)
                        } else {
                            0.5
                        },
                    );
                }
            }
            RecordedPerformanceKind::Expression {
                pitch,
                pressure,
                timbre,
            } => {
                if !pitch.is_finite()
                    || !pressure.is_finite()
                    || !timbre.is_finite()
                    || !(-120.0..=120.0).contains(&pitch)
                    || !(0.0..=1.0).contains(&pressure)
                    || !(0.0..=1.0).contains(&timbre)
                {
                    return;
                }
                let Some(note) = self.held.get_mut(&identity) else {
                    return;
                };
                let value = ExpressionValue {
                    pitch,
                    pressure,
                    timbre,
                };
                let pos = pos
                    .max(note.start)
                    .max(note.snapshots.last().map_or(note.start, |p| p.0));
                if let Some((at, old)) = note.snapshots.last_mut() {
                    if *old == value {
                        return;
                    }
                    if (pos - *at).abs() < 0.5 / PPQ as f64 {
                        *old = value;
                        return;
                    }
                }
                if self.snapshots >= MAX_SNAPSHOTS
                    || note.snapshots.len() >= peel_document::MAX_EXPRESSION_POINTS / 3
                {
                    self.overflow(s);
                    return;
                }
                note.snapshots.push((pos, value));
                self.snapshots += 1;
            }
            RecordedPerformanceKind::Reset => {}
        }
    }

    fn store(&mut self, s: &mut Session, channel: Id, note: Held, end: f64, release: f32) {
        let Some(take) = &mut self.take else { return };
        if take.project != s.project().id
            || s.project().channel(channel).is_none()
            || s.project().pattern(take.pattern).is_none()
        {
            return;
        }
        let pid = take.pattern;
        let pattern_length = s.project().pattern(pid).unwrap().length;
        let length = take.timing.length;
        let end = end.max(note.start + 1.0 / PPQ as f64);
        let tick = Time::new(1, PPQ).min(pattern_length);
        let mut cursor = note.start;
        let mut notes = Vec::new();
        let mut expressions = Vec::new();
        let mut point_budget = 1_000_000usize.saturating_sub(
            s.project()
                .pattern(pid)
                .unwrap()
                .expression
                .iter()
                .map(|e| e.pitch.len() + e.pressure.len() + e.timbre.len())
                .sum::<usize>(),
        );
        let mut limit = false;
        while cursor < end - 1e-9 {
            if take.notes >= MAX_TAKE_NOTES {
                limit = true;
                break;
            }
            let epoch = (cursor / length).floor();
            let boundary = (epoch + 1.0) * length;
            let stop = end.min(boundary);
            let start = Time::from_f64(cursor - epoch * length, PPQ).min(pattern_length - tick);
            let duration = Time::from_f64(stop - cursor, PPQ)
                .max(tick)
                .min(pattern_length - start);
            if start.is_negative() || !duration.is_positive() {
                break;
            }
            let mut saved = Note::new(channel, start, duration, note.key, note.velocity);
            saved.release = release;
            let before = note
                .snapshots
                .partition_point(|(at, _)| *at <= cursor + 1e-9);
            let initial = before
                .checked_sub(1)
                .map_or(ExpressionValue::default(), |i| note.snapshots[i].1);
            let mut curve = NoteExpression {
                note: saved.id,
                ..Default::default()
            };
            let mut previous = initial;
            for (points, value) in [
                (&mut curve.pitch, initial.pitch),
                (&mut curve.pressure, initial.pressure),
                (&mut curve.timbre, initial.timbre),
            ] {
                points.push(ExpressionPoint {
                    offset: Time::ZERO,
                    value,
                });
            }
            for &(at, value) in &note.snapshots[before..] {
                if at >= stop {
                    break;
                }
                let offset = Time::from_f64(at - cursor, PPQ).min(duration);
                for (points, old, value) in [
                    (&mut curve.pitch, previous.pitch, value.pitch),
                    (&mut curve.pressure, previous.pressure, value.pressure),
                    (&mut curve.timbre, previous.timbre, value.timbre),
                ] {
                    if old != value {
                        if let Some(last) = points.last_mut().filter(|p| p.offset == offset) {
                            last.value = value;
                        } else {
                            points.push(ExpressionPoint { offset, value });
                        }
                    }
                }
                previous = value;
            }
            let count = curve.pitch.len() + curve.pressure.len() + curve.timbre.len();
            if count > point_budget {
                limit = true;
                break;
            }
            point_budget -= count;
            notes.push(saved);
            expressions.push(curve);
            take.notes += 1;
            cursor = stop;
        }
        if !notes.is_empty() {
            let gesture = *self.gesture.get_or_insert(0x7ec0_0000 + self.takes);
            s.doc.edit_gesture(gesture, "Record notes", |p| {
                if let Some(pattern) = p.pattern_mut(pid) {
                    pattern.notes.extend(notes);
                    pattern.expression.extend(expressions);
                    pattern.sort_notes();
                }
            });
        }
        if limit {
            self.overflow(s);
        }
    }

    fn flush(&mut self, s: &mut Session, channel: Option<Id>, at: f64) {
        let ids: Vec<_> = self
            .held
            .keys()
            .filter(|(id, _)| channel.is_none_or(|ch| *id == ch))
            .copied()
            .collect();
        for id in ids {
            if let Some(note) = self.held.remove(&id) {
                self.snapshots = self.snapshots.saturating_sub(note.snapshots.len());
                self.store(s, id.0, note, at, 0.5);
            }
        }
    }

    fn finish(&mut self, s: &mut Session) {
        let at = self.take.as_ref().map_or(0.0, |t| t.clock.cursor);
        self.flush(s, None, at);
        self.take = None;
        self.held.clear();
        self.snapshots = 0;
        if self.gesture.take().is_some() {
            self.takes += 1;
            s.doc.end_gesture();
        }
    }

    /// Drains even when disarmed so an earlier performance cannot enter a take.
    pub fn frame(&mut self, s: &mut Session) {
        if self.armed && s.is_playing() && s.mode() == PlayMode::Song && !s.armed_lanes().is_empty()
        {
            if !s.is_recording() && !self.audio_tried {
                self.audio_tried = true;
                let _ = s.start_recording();
            }
        } else if s.is_recording() && !self.armed {
            s.stop_recording();
        }
        if !s.is_playing() {
            self.audio_tried = false;
        }
        let active = self.active(s);
        let mut events = s.take_recorded_performance();
        events.sort_by_key(|e| e.time_ns);
        if self.take.as_ref().is_some_and(|t| {
            t.project != s.project().id
                || active.is_some_and(|pid| pid != t.pattern)
                || s.project()
                    .pattern(t.pattern)
                    .is_none_or(|p| p.length.as_f64() != t.timing.length)
        }) {
            // Events already accepted before the switch belong to the old take.
            for event in events.drain(..) {
                self.consume(s, event);
            }
            self.finish(s);
        }
        if self.take.is_none()
            && let Some(pid) = active
        {
            self.begin(s, pid);
        }
        for event in events {
            self.consume(s, event);
        }
        if let Some(take) = &mut self.take
            && active == Some(take.pattern)
            && !take.awaiting_note
        {
            take.clock.position(s.position(), None, &take.timing);
        }
        let dropped = s.recorded_performance_dropped();
        if dropped > self.dropped && self.take.is_some() {
            self.overflow(s);
        }
        self.dropped = dropped;
        if active.is_none() {
            self.finish(s);
        }
        if !self.armed {
            self.warned = false;
        }
    }
}

#[cfg(test)]
mod tests;
