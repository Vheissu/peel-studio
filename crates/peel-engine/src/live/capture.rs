//! Producer-side performance capture. The fixed table mirrors held legacy keys;
//! scoped MPE notes are recorded directly by the voice APIs.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Held {
    slot: u32,
    channel: Option<u8>,
    held: bool,
}
pub(super) struct Capture {
    notes: [Option<Held>; 128],
    slot: u32,
    bend: f32,
    pressure: f32,
    keys: [f32; 128],
    pedal: bool,
}
impl Default for Capture {
    fn default() -> Self {
        Self {
            notes: [None; 128],
            slot: LIVE_TARGET_NONE,
            bend: 0.0,
            pressure: 0.0,
            keys: [0.0; 128],
            pedal: false,
        }
    }
}
impl Tx {
    pub(super) fn performance(
        &mut self,
        shared: &LiveShared,
        t_ns: u64,
        slot: u32,
        voice: u32,
        midi_channel: Option<u8>,
        kind: RecordedPerformanceKind,
    ) {
        if slot == LIVE_TARGET_NONE && !matches!(kind, RecordedPerformanceKind::Reset) {
            return;
        }
        let event = RecordedPerformanceEvent {
            slot,
            voice,
            midi_channel,
            kind,
            beat: shared.telemetry.beat(),
            playing: shared.telemetry.playing.load(Ordering::Relaxed),
            sustain: shared.pedal.load(Ordering::Relaxed),
            time_ns: t_ns,
        };
        if self.performance.push(event).is_err() {
            shared
                .stats
                .performance_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn capture_note(
        &mut self,
        shared: &LiveShared,
        t: u64,
        slot: u32,
        key: u8,
        velocity: f32,
        on: bool,
        channel: Option<u8>,
    ) {
        let k = usize::from(key);
        let voice = crate::LIVE_VOICE_BASE + u32::from(key);
        if on {
            if let Some(previous) = self.capture.notes[k] {
                self.performance(
                    shared,
                    t,
                    previous.slot,
                    voice,
                    previous.channel,
                    RecordedPerformanceKind::NoteOff { key, velocity: 0.0 },
                );
            }
            self.capture.keys[k] = 0.0;
            self.capture.notes[k] = Some(Held {
                slot,
                channel,
                held: true,
            });
            self.performance(
                shared,
                t,
                slot,
                voice,
                channel,
                RecordedPerformanceKind::NoteOn { key, velocity },
            );
            self.capture_snapshot(shared, t, key);
        } else if let Some(mut note) = self.capture.notes[k] {
            if self.capture.pedal {
                note.held = false;
                self.capture.notes[k] = Some(note);
            } else {
                self.performance(
                    shared,
                    t,
                    note.slot,
                    voice,
                    note.channel,
                    RecordedPerformanceKind::NoteOff { key, velocity: 0.0 },
                );
                self.capture.notes[k] = None;
            }
        }
    }
    pub(super) fn capture_sustain(&mut self, shared: &LiveShared, t: u64, down: bool) {
        self.capture.pedal = down;
        if !down {
            for k in 0..128 {
                if let Some(note) = self.capture.notes[k]
                    && !note.held
                {
                    self.performance(
                        shared,
                        t,
                        note.slot,
                        crate::LIVE_VOICE_BASE + k as u32,
                        note.channel,
                        RecordedPerformanceKind::NoteOff {
                            key: k as u8,
                            velocity: 0.0,
                        },
                    );
                    self.capture.notes[k] = None;
                }
            }
        }
    }
    pub(super) fn capture_reset(&mut self, shared: &LiveShared, t: u64, slot: u32) {
        for k in 0..128 {
            if let Some(note) = self.capture.notes[k]
                && (slot == LIVE_TARGET_NONE || note.slot == slot)
            {
                self.performance(
                    shared,
                    t,
                    note.slot,
                    crate::LIVE_VOICE_BASE + k as u32,
                    note.channel,
                    RecordedPerformanceKind::NoteOff {
                        key: k as u8,
                        velocity: 0.0,
                    },
                );
                self.capture.notes[k] = None;
            }
        }
        // Reset is a take boundary, not merely a return of a wheel to centre.
        self.performance(shared, t, slot, 0, None, RecordedPerformanceKind::Reset);
        self.capture.bend = 0.0;
        self.capture.pressure = 0.0;
        self.capture.keys.fill(0.0);
        self.capture.pedal = false;
    }
    fn capture_snapshot(&mut self, shared: &LiveShared, t: u64, key: u8) {
        if let Some(note) = self.capture.notes[usize::from(key)] {
            let applies = note.slot == self.capture.slot;
            let c = if applies { self.capture.pressure } else { 0.0 };
            let p = if applies {
                self.capture.keys[usize::from(key)]
            } else {
                0.0
            };
            self.performance(
                shared,
                t,
                note.slot,
                crate::LIVE_VOICE_BASE + u32::from(key),
                note.channel,
                RecordedPerformanceKind::Expression {
                    pitch: if applies { self.capture.bend } else { 0.0 },
                    pressure: c + p - c * p,
                    timbre: 0.5,
                },
            );
        }
    }
    pub(super) fn capture_expression(
        &mut self,
        shared: &LiveShared,
        t: u64,
        slot: u32,
        kind: EventKind,
    ) {
        if self.capture.slot != slot {
            self.capture.slot = slot;
            self.capture.bend = 0.0;
            self.capture.pressure = 0.0;
            self.capture.keys.fill(0.0);
        }
        match kind {
            EventKind::PitchBend { semitones } => self.capture.bend = semitones,
            EventKind::ChannelPressure { value } => self.capture.pressure = value,
            EventKind::PolyPressure { key, value } => self.capture.keys[usize::from(key)] = value,
            EventKind::ResetExpression => {
                self.capture.bend = 0.0;
                self.capture.pressure = 0.0;
                self.capture.keys.fill(0.0);
            }
            EventKind::VoiceExpression {
                voice,
                pitch,
                pressure,
                timbre,
            } => {
                self.performance(
                    shared,
                    t,
                    slot,
                    voice,
                    None,
                    RecordedPerformanceKind::Expression {
                        pitch,
                        pressure,
                        timbre,
                    },
                );
                return;
            }
            _ => return,
        }
        for k in 0..128 {
            if self.capture.notes[k].is_some_and(|n| n.slot == slot)
                && !matches!(kind,EventKind::PolyPressure{key,..} if usize::from(key)!=k)
            {
                self.capture_snapshot(shared, t, k as u8);
            }
        }
    }
}
impl LiveInput {
    /// Starts an independently addressed voice and its initial expression atomically.
    #[allow(clippy::too_many_arguments)]
    pub fn voice_on(
        &self,
        slot: u32,
        voice: u32,
        key: u8,
        velocity: f32,
        channel: u8,
        pitch: f32,
        pressure: f32,
        timbre: f32,
    ) -> bool {
        if slot == LIVE_TARGET_NONE
            || !(crate::LIVE_VOICE_BASE..0x7fff_ff00).contains(&voice)
            || key > 127
            || channel > 15
            || !pitch.is_finite()
            || !pressure.is_finite()
            || !timbre.is_finite()
        {
            return false;
        }
        let shared = &self.inner.shared;
        let t = shared.now_ns();
        let mut tx = self.lock();
        if tx.events.slots() < LIVE_QUEUE_RESERVE + 1 {
            shared.stats.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let velocity = clean_velocity(velocity);
        let expression = EventKind::VoiceExpression {
            voice,
            pitch: pitch.clamp(-120.0, 120.0),
            pressure: pressure.clamp(0.0, 1.0),
            timbre: timbre.clamp(0.0, 1.0),
        };
        push_event(
            &mut tx,
            shared,
            LiveEvent {
                t_ns: t,
                slot,
                kind: LiveKind::VoiceOn {
                    key,
                    velocity,
                    voice,
                    pitch: pitch.clamp(-120.0, 120.0),
                    pressure: pressure.clamp(0.0, 1.0),
                    timbre: timbre.clamp(0.0, 1.0),
                },
            },
            false,
        );
        tx.performance(
            shared,
            t,
            slot,
            voice,
            Some(channel),
            RecordedPerformanceKind::NoteOn { key, velocity },
        );
        let note = RecordedNote {
            slot,
            key,
            velocity,
            on: true,
            beat: shared.telemetry.beat(),
            playing: shared.telemetry.playing.load(Ordering::Relaxed),
            sustain: shared.pedal.load(Ordering::Relaxed),
            time_ns: t,
        };
        if tx.record.push(note).is_err() {
            shared.stats.record_dropped.fetch_add(1, Ordering::Relaxed);
        }

        if let EventKind::VoiceExpression {
            pitch,
            pressure,
            timbre,
            ..
        } = expression
        {
            tx.performance(
                shared,
                t,
                slot,
                voice,
                Some(channel),
                RecordedPerformanceKind::Expression {
                    pitch,
                    pressure,
                    timbre,
                },
            );
        }
        shared.stats.note_ons.fetch_add(1, Ordering::Relaxed);
        true
    }
    pub fn voice_off(&self, slot: u32, voice: u32, key: u8, channel: u8) -> bool {
        let shared = &self.inner.shared;
        let t = shared.now_ns();
        let mut tx = self.lock();
        if !push_event(
            &mut tx,
            shared,
            LiveEvent {
                t_ns: t,
                slot,
                kind: LiveKind::VoiceOff { key, voice },
            },
            true,
        ) {
            return false;
        }
        tx.performance(
            shared,
            t,
            slot,
            voice,
            Some(channel),
            RecordedPerformanceKind::NoteOff { key, velocity: 0.0 },
        );
        let note = RecordedNote {
            slot,
            key,
            velocity: 0.0,
            on: false,
            beat: shared.telemetry.beat(),
            playing: shared.telemetry.playing.load(Ordering::Relaxed),
            sustain: shared.pedal.load(Ordering::Relaxed),
            time_ns: t,
        };
        if tx.record.push(note).is_err() {
            shared.stats.record_dropped.fetch_add(1, Ordering::Relaxed);
        }

        shared.stats.note_offs.fetch_add(1, Ordering::Relaxed);
        true
    }
    #[allow(clippy::too_many_arguments)]
    pub fn voice_expression(
        &self,
        slot: u32,
        voice: u32,
        channel: u8,
        pitch: f32,
        pressure: f32,
        timbre: f32,
    ) -> bool {
        if !pitch.is_finite() || !pressure.is_finite() || !timbre.is_finite() {
            return false;
        }
        let shared = &self.inner.shared;
        let t = shared.now_ns();
        let mut tx = self.lock();
        let (pitch, pressure, timbre) = (
            pitch.clamp(-120.0, 120.0),
            pressure.clamp(0.0, 1.0),
            timbre.clamp(0.0, 1.0),
        );
        let kind = EventKind::VoiceExpression {
            voice,
            pitch,
            pressure,
            timbre,
        };
        if !push_event(
            &mut tx,
            shared,
            LiveEvent {
                t_ns: t,
                slot,
                kind: LiveKind::Expression { kind },
            },
            false,
        ) {
            return false;
        }
        tx.performance(
            shared,
            t,
            slot,
            voice,
            Some(channel),
            RecordedPerformanceKind::Expression {
                pitch,
                pressure,
                timbre,
            },
        );
        true
    }
}
