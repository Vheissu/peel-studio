//! Port-scoped performance replay after the track parser has paired notes.
//! Type 1 tracks share their port's controls; type 2 sequences do not.
use super::*;
use peel_document::{ExpressionPoint, MAX_EXPRESSION_POINTS, NoteExpression};
use std::collections::HashSet;

const MAX_CONTROL_EVENTS: usize = 1_000_000;
const MAX_REPLAY_WORK: usize = 8_000_000;

#[derive(Clone, Copy)]
pub(super) struct Control {
    tick: u64,
    order: usize,
    port: u8,
    channel: u8,
    message: MidiMessage,
}

pub(super) fn is_controller(cc: u8, sustain: SustainPolicy) -> bool {
    matches!(cc, 6 | 38 | 74 | 98..=101 | 120 | 121 | 123)
        || (cc == 64 && sustain == SustainPolicy::ExtendNotes)
}

pub(super) fn collect(
    scan: &mut TrackScan,
    ctx: &mut Collected,
    tick: u64,
    port: u8,
    channel: u8,
    message: MidiMessage,
) -> Result<()> {
    ctx.expression_events += 1;
    if ctx.expression_events > MAX_CONTROL_EVENTS {
        return Err(MidiError::TooLarge(
            "more than 1000000 performance-control events".into(),
        ));
    }
    scan.expression_events.push(Control {
        tick,
        order: scan.events,
        port,
        channel,
        message,
    });
    Ok(())
}

#[derive(Default)]
pub(super) struct RawExpression {
    pitch: Vec<(u64, f32)>,
    pressure: Vec<(u64, f32)>,
    timbre: Vec<(u64, f32)>,
}
impl RawExpression {
    fn push(&mut self, start: u64, tick: u64, value: [f32; 3]) -> Result<usize> {
        let before = self.pitch.len() + self.pressure.len() + self.timbre.len();
        for (curve, neutral, value) in [
            (&mut self.pitch, 0.0, value[0]),
            (&mut self.pressure, 0.0, value[1]),
            (&mut self.timbre, 0.5, value[2]),
        ] {
            if curve.last().map_or(neutral, |p| p.1) == value {
                continue;
            }
            if curve.is_empty() && tick > start {
                curve.push((start, neutral));
            }
            if let Some(last) = curve.last_mut().filter(|p| p.0 == tick) {
                last.1 = value;
            } else {
                curve.push((tick, value));
            }
        }
        if self.pitch.len() + self.pressure.len() + self.timbre.len() > MAX_EXPRESSION_POINTS {
            return Err(MidiError::TooLarge(
                "more than 65536 expression points for one note".into(),
            ));
        }
        Ok(self.pitch.len() + self.pressure.len() + self.timbre.len() - before)
    }
    pub(super) fn to_document(
        &self,
        note: Id,
        clock: TickClock,
        start: u64,
        length: Time,
    ) -> Result<NoteExpression> {
        fn curve(
            points: &[(u64, f32)],
            neutral: f32,
            clock: TickClock,
            start: u64,
            length: Time,
        ) -> Result<Vec<ExpressionPoint>> {
            if points.iter().all(|p| p.1 == neutral) {
                return Ok(Vec::new());
            }
            let mut out: Vec<ExpressionPoint> = Vec::new();
            for &(tick, value) in points {
                let offset = clock.time(tick.saturating_sub(start))?;
                if offset > length {
                    break;
                }
                if let Some(last) = out.last_mut().filter(|p| p.offset == offset) {
                    last.value = value;
                } else {
                    out.push(ExpressionPoint { offset, value });
                }
            }
            if out.iter().all(|p| p.value == neutral) {
                out.clear();
            }
            Ok(out)
        }
        Ok(NoteExpression {
            note,
            pitch: curve(&self.pitch, 0.0, clock, start, length)?,
            pressure: curve(&self.pressure, 0.0, clock, start, length)?,
            timbre: curve(&self.timbre, 0.5, clock, start, length)?,
        })
    }
}

#[derive(Clone, Copy)]
struct ChannelState {
    bend: f32,
    range: f32,
    pressure: f32,
    timbre: f32,
    pedal: bool,
    rpn: [u8; 2],
}
impl Default for ChannelState {
    fn default() -> Self {
        Self {
            bend: 0.0,
            range: 2.0,
            pressure: 0.0,
            timbre: 0.5,
            pedal: false,
            rpn: [127; 2],
        }
    }
}
type NoteKey = (usize, usize);
struct Active {
    released: bool,
    poly: Option<f32>,
}
#[derive(Default)]
struct PortState {
    channels: [ChannelState; 16],
    lower: u8,
    upper: u8,
    active: HashMap<NoteKey, Active>,
    channel_notes: [HashSet<NoteKey>; 16],
    key_notes: HashMap<(u8, u8), HashSet<NoteKey>>,
}
impl PortState {
    fn manager(&self, ch: u8) -> Option<u8> {
        if ch > 0 && ch <= self.lower {
            Some(0)
        } else if ch < 15 && ch >= 15 - self.upper {
            Some(15)
        } else {
            None
        }
    }
    fn raw_pitch(&self, note: &RawNote) -> f32 {
        let member = self.channels[usize::from(note.chan)];
        member.bend * member.range
            + note.manager.map_or(0.0, |m| {
                let m = self.channels[usize::from(m)];
                m.bend * m.range
            })
    }
    fn snapshot(&self, note: &RawNote, poly: Option<f32>) -> [f32; 3] {
        let member = self.channels[usize::from(note.chan)];
        let manager = note.manager.map(|m| self.channels[usize::from(m)]);
        [
            self.raw_pitch(note).clamp(-120.0, 120.0),
            (poly.unwrap_or(member.pressure) + manager.map_or(0.0, |m| m.pressure)).clamp(0.0, 1.0),
            (member.timbre + manager.map_or(0.0, |m| m.timbre - 0.5)).clamp(0.0, 1.0),
        ]
    }
    fn pedal(&self, note: &RawNote) -> bool {
        self.channels[usize::from(note.chan)].pedal
            || note
                .manager
                .is_some_and(|m| self.channels[usize::from(m)].pedal)
    }
    fn finish(&mut self, key: NoteKey, tick: u64, scans: &mut [TrackScan]) {
        self.active.remove(&key);
        let note = &mut scans[key.0].notes[key.1];
        self.channel_notes[usize::from(note.chan)].remove(&key);
        if let Some(bucket) = self.key_notes.get_mut(&(note.chan, note.key)) {
            bucket.remove(&key);
        }
        note.end = tick.max(note.start + 1);
    }
    fn affected_channels(&self, channel: u8) -> [bool; 16] {
        std::array::from_fn(|ch| {
            ch == usize::from(channel) || self.manager(ch as u8) == Some(channel)
        })
    }
    fn affected_notes(
        &self,
        channels: [bool; 16],
        pitch: Option<u8>,
        work: &mut usize,
    ) -> Result<Vec<NoteKey>> {
        let buckets: Vec<_> = channels
            .iter()
            .enumerate()
            .filter_map(|(ch, include)| {
                if !*include {
                    None
                } else if let Some(pitch) = pitch {
                    self.key_notes.get(&(ch as u8, pitch))
                } else {
                    Some(&self.channel_notes[ch])
                }
            })
            .collect();
        let count: usize = buckets.iter().map(|notes| notes.len()).sum();
        *work += count;
        if *work > MAX_REPLAY_WORK {
            return Err(MidiError::TooLarge(
                "performance replay exceeds 8000000 note updates".into(),
            ));
        }
        let mut notes = Vec::with_capacity(count);
        for bucket in buckets {
            notes.extend(bucket.iter().copied());
        }
        Ok(notes)
    }
    fn update(
        &mut self,
        keys: &[NoteKey],
        tick: u64,
        scans: &mut [TrackScan],
        points: &mut usize,
        clipped: &mut bool,
    ) -> Result<()> {
        for &key in keys {
            let Some(a) = self.active.get(&key) else {
                continue;
            };
            let note = &mut scans[key.0].notes[key.1];
            *clipped |= self.raw_pitch(note).abs() > 120.0;
            let value = self.snapshot(note, a.poly);
            *points += note.expression.push(note.start, tick, value)?;
            check_points(*points)?;
        }
        Ok(())
    }
    fn configure(&mut self, manager: u8, members: u8) -> [bool; 16] {
        let old_members: [Option<u8>; 16] = std::array::from_fn(|ch| self.manager(ch as u8));
        let old_lower = self.lower;
        let old_upper = self.upper;
        let members = members.min(15);
        // The most recent zone wins any overlap. Two zones leave room for
        // both managers whenever the opposite zone is still enabled.
        if manager == 0 {
            self.lower = members;
            if self.upper != 0 && members != 0 {
                self.upper = self.upper.min(14u8.saturating_sub(members));
            }
        } else {
            self.upper = members;
            if self.lower != 0 && members != 0 {
                self.lower = self.lower.min(14u8.saturating_sub(members));
            }
        }
        let mut reset = [false; 16];
        for ch in 0..16u8 {
            let now = self.manager(ch);
            let before = old_members[usize::from(ch)];
            let manager_disabled = (ch == 0 && old_lower != 0 && self.lower == 0)
                || (ch == 15 && old_upper != 0 && self.upper == 0);
            if before != now
                || before == Some(manager)
                || now == Some(manager)
                || ch == manager
                || manager_disabled
            {
                reset[usize::from(ch)] = true;
                self.channels[usize::from(ch)] = ChannelState {
                    range: if now.is_some() { 48.0 } else { 2.0 },
                    ..ChannelState::default()
                };
            }
        }
        reset
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Start(usize),
    End(usize),
    Control(Control),
}
struct Event {
    tick: u64,
    track: usize,
    order: usize,
    port: u8,
    kind: Kind,
    phase: u8,
}

pub(super) fn apply(
    scans: &mut [TrackScan],
    format: SmfFormat,
    opts: &ImportOptions,
    ctx: &mut Collected,
) -> Result<()> {
    let mut events = Vec::new();
    for (track, scan) in scans.iter().enumerate() {
        for (i, n) in scan.notes.iter().enumerate() {
            events.push(Event {
                tick: n.start,
                track,
                order: n.start_order,
                port: n.port,
                kind: Kind::Start(i),
                phase: 0,
            });
            events.push(Event {
                tick: n.end,
                track,
                order: n.end_order,
                port: n.port,
                kind: Kind::End(i),
                phase: 0,
            });
        }
        for &c in &scan.expression_events {
            events.push(Event {
                tick: c.tick,
                track,
                order: c.order,
                port: c.port,
                kind: Kind::Control(c),
                phase: 0,
            });
        }
    }
    events.sort_by_key(|e| (e.tick, e.track, e.order));
    // Parallel tracks have no total order within a tick. Apply each track's
    // prefix before its first note-on as setup before starting notes in any
    // track. Keep the entire prefix (including note-offs) in original order,
    // and never promote a reset written after a note-on in its own track.
    let mut track_tick = None;
    let mut started = false;
    for event in &mut events {
        let here = (event.tick, event.track);
        if track_tick != Some(here) {
            track_tick = Some(here);
            started = false;
        }
        started |= matches!(event.kind, Kind::Start(_));
        event.phase = u8::from(started);
    }
    events.sort_by_key(|e| (e.tick, e.phase, e.track, e.order));
    let mut ports: BTreeMap<(usize, u8), PortState> = BTreeMap::new();
    let mut work = 0usize;
    let mut points = 0usize;
    let mut clipped_pitch = false;
    for e in events {
        let sequence = if format == SmfFormat::Type2 {
            e.track
        } else {
            0
        };
        let state = ports.entry((sequence, e.port)).or_default();
        match e.kind {
            Kind::Start(index) => {
                let chan = scans[e.track].notes[index].chan;
                let key = scans[e.track].notes[index].key;
                if opts.sustain == SustainPolicy::ExtendNotes {
                    let mask = std::array::from_fn(|ch| ch == usize::from(chan));
                    for old_key in state.affected_notes(mask, Some(key), &mut work)? {
                        let old = &scans[old_key.0].notes[old_key.1];
                        if state.active[&old_key].released && old.key == key {
                            state.finish(old_key, e.tick, scans);
                        }
                    }
                }
                let note = &mut scans[e.track].notes[index];
                note.manager = state.manager(note.chan);
                clipped_pitch |= state.raw_pitch(note).abs() > 120.0;
                let value = state.snapshot(note, None);
                points += note.expression.push(note.start, e.tick, value)?;
                check_points(points)?;
                let key = (e.track, index);
                state.channel_notes[usize::from(note.chan)].insert(key);
                state
                    .key_notes
                    .entry((note.chan, note.key))
                    .or_default()
                    .insert(key);
                state.active.insert(
                    key,
                    Active {
                        released: false,
                        poly: None,
                    },
                );
            }
            Kind::End(index) => {
                let key = (e.track, index);
                if state.active.contains_key(&key) {
                    if opts.sustain == SustainPolicy::ExtendNotes
                        && state.pedal(&scans[e.track].notes[index])
                    {
                        state.active.get_mut(&key).unwrap().released = true;
                    } else {
                        state.finish(key, e.tick, scans);
                    }
                }
            }
            Kind::Control(c) => {
                let ci = usize::from(c.channel);
                let mut affected = state.affected_channels(c.channel);
                let mut update = true;
                let mut finish = None;
                let mut release_held = false;
                let mut clear_poly = false;
                let mut poly = None;
                match c.message {
                    MidiMessage::PitchBend { bend } => {
                        let value = i32::from(bend.0.as_int()) - 8192;
                        state.channels[ci].bend =
                            value as f32 / if value >= 0 { 8191.0 } else { 8192.0 };
                    }
                    MidiMessage::ChannelAftertouch { vel } => {
                        state.channels[ci].pressure = f32::from(vel.as_int()) / 127.0
                    }
                    MidiMessage::Aftertouch { key, vel } => {
                        poly = Some((key.as_int(), f32::from(vel.as_int()) / 127.0))
                    }
                    MidiMessage::Controller { controller, value } => {
                        let cc = controller.as_int();
                        let value = value.as_int();
                        match cc {
                            74 => {
                                state.channels[ci].timbre = if value <= 64 {
                                    f32::from(value) / 128.0
                                } else {
                                    0.5 + f32::from(value - 64) / 126.0
                                }
                            }
                            64 => {
                                state.channels[ci].pedal = value >= 64;
                                release_held = true;
                                update = false;
                            }
                            101 => {
                                state.channels[ci].rpn[0] = value;
                                update = false;
                            }
                            100 => {
                                state.channels[ci].rpn[1] = value;
                                update = false;
                            }
                            98 | 99 => {
                                state.channels[ci].rpn = [127; 2];
                                ctx.skip(controller_name(cc));
                                update = false;
                            }
                            6 | 38 => {
                                let channel = &mut state.channels[ci];
                                match channel.rpn {
                                    [0, 0] => {
                                        let range = if cc == 6 {
                                            f32::from(value) + channel.range.fract()
                                        } else {
                                            channel.range.floor() + f32::from(value.min(99)) / 100.0
                                        };
                                        channel.range = range;
                                        if let Some(manager) = state.manager(c.channel) {
                                            affected = std::array::from_fn(|ch| {
                                                state.manager(ch as u8) == Some(manager)
                                            });
                                            for (ch, include) in affected.iter().enumerate() {
                                                if *include {
                                                    state.channels[ch].range = range;
                                                }
                                            }
                                        }
                                    }
                                    [0, 6] if matches!(c.channel, 0 | 15) => {
                                        if cc == 6 {
                                            affected = state.configure(c.channel, value);
                                            // MCM does not deselect RPN 6. Its optional data LSB
                                            // is accepted but has no effect on the member count.
                                            state.channels[ci].rpn = [0, 6];
                                            finish = Some(true);
                                        }
                                        update = false;
                                    }
                                    _ => {
                                        ctx.skip(controller_name(cc));
                                        update = false;
                                    }
                                }
                            }
                            121 => {
                                let channel = &mut state.channels[ci];
                                channel.bend = 0.0;
                                channel.pressure = 0.0;
                                channel.timbre = 0.5;
                                channel.pedal = false;
                                channel.rpn = [127; 2];
                                clear_poly = true;
                                release_held = true;
                            }
                            120 | 123 => {
                                finish = Some(cc == 120);
                                update = false;
                            }
                            _ => update = false,
                        }
                    }
                    _ => update = false,
                }
                if update || finish.is_some() || release_held {
                    let keys =
                        state.affected_notes(affected, poly.map(|(key, _)| key), &mut work)?;
                    for &key in &keys {
                        let note = &scans[key.0].notes[key.1];
                        let pedal = opts.sustain == SustainPolicy::ExtendNotes && state.pedal(note);
                        if let Some(force) = finish {
                            if force || !pedal {
                                state.finish(key, e.tick, scans);
                                continue;
                            }
                            state.active.get_mut(&key).unwrap().released = true;
                        }
                        let active = state.active.get_mut(&key).unwrap();
                        if clear_poly {
                            active.poly = None;
                        }
                        if let Some((poly_key, value)) = poly
                            && note.key == poly_key
                        {
                            active.poly = Some(value);
                        }
                        if release_held && active.released && !pedal {
                            state.finish(key, e.tick, scans);
                        }
                    }
                    if update {
                        state.update(&keys, e.tick, scans, &mut points, &mut clipped_pitch)?;
                    }
                }
            }
        }
    }
    for ((sequence, _), mut state) in ports {
        let end = if format == SmfFormat::Type2 {
            scans[sequence].end_tick
        } else {
            scans.iter().map(|s| s.end_tick).max().unwrap_or(0)
        };
        let keys: Vec<_> = state.active.keys().copied().collect();
        for key in keys {
            state.finish(key, end, scans);
        }
    }
    if clipped_pitch {
        ctx.warnings.push(
            "Pitch bend beyond 120 semitones was limited to Peel's note-expression range.".into(),
        );
    }
    Ok(())
}

fn check_points(points: usize) -> Result<()> {
    if points > 2_000_000 {
        return Err(MidiError::TooLarge(
            "more than 2000000 stored expression points".into(),
        ));
    }
    Ok(())
}
