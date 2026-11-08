//! Live performance input.
//!
//! Hardware MIDI keyboards (and anything else that plays an instrument in
//! real time) feed the engine through a queue of their own, separate from
//! the command queue, so a flood of parameter changes can never delay a
//! note and a burst of notes can never starve a parameter change.
//!
//! # Threads
//!
//! * [`LiveInput`] is the producer end. It is `Clone + Send + Sync` and is
//!   meant to be moved into MIDI callback threads (one clone per port). The
//!   callbacks share one bounded SPSC ring behind a mutex that only those
//!   threads ever take; the audio thread owns the consumer and never locks.
//! * The audio thread drains the ring at the start of each block, applies
//!   the sustain pedal and stages the notes on the target instrument at a
//!   sample offset (see [`LiveTiming`]).
//! * The control thread picks the instrument ([`LiveTarget`]), reads the
//!   activity counters ([`LiveStats`]) and drains the recorded notes
//!   ([`RecordedReceiver`]).
//!
//! # Sustain pedal
//!
//! The pedal is implemented here, in the host, so every instrument gets it.
//! While it is down a note-off is remembered instead of delivered, and all
//! remembered releases are delivered, in the order they were played, when
//! the pedal lifts. Storage is a fixed array ([`MAX_SUSTAINED`]); if it ever
//! fills, further note-offs are delivered immediately instead.
//!
//! # Target changes
//!
//! A note-on is bound to the target at the time it was played. Its note-off
//! always goes to the same instrument, even if the selected channel changed
//! in between, so switching channels while holding a key never hangs a note.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::plan::Slot;
use crate::telemetry::Telemetry;

/// Value of the live target when no instrument is selected.
pub const LIVE_TARGET_NONE: u32 = u32::MAX;
/// Live events that can be waiting for the audio thread.
pub const LIVE_QUEUE_CAPACITY: usize = 1024;
/// Free queue entries kept back for note-offs, pedal lifts and releases, so
/// a flood of note-ons cannot stop a note from ending.
const LIVE_QUEUE_RESERVE: usize = 64;
/// Recorded notes waiting for the control thread.
pub const RECORD_CAPACITY: usize = 4096;
/// Note-offs the pedal can hold back at once, across all instruments.
pub const MAX_SUSTAINED: usize = 512;
/// Live events applied per block; the rest wait for the next one.
pub(crate) const MAX_EVENTS_PER_BLOCK: usize = 512;
/// Most actions one event can expand to (every sustained release).
const ACTION_CAPACITY: usize = MAX_SUSTAINED + 8;

// ----------------------------------------------------------------------
// Public control-side types
// ----------------------------------------------------------------------

/// The instrument that live notes play. Shared between the control thread
/// (which sets it when the selected channel changes) and the MIDI threads
/// (which read it for every note).
#[derive(Clone, Debug)]
pub struct LiveTarget(Arc<AtomicU32>);

impl LiveTarget {
    pub fn new() -> LiveTarget {
        LiveTarget(Arc::new(AtomicU32::new(LIVE_TARGET_NONE)))
    }

    /// Selects the instrument slot, or `None` to ignore live notes.
    pub fn set(&self, slot: Option<Slot>) {
        self.0
            .store(slot.unwrap_or(LIVE_TARGET_NONE), Ordering::Relaxed);
    }

    pub fn get(&self) -> Option<Slot> {
        match self.raw() {
            LIVE_TARGET_NONE => None,
            slot => Some(slot),
        }
    }

    fn raw(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
}

impl Default for LiveTarget {
    fn default() -> Self {
        LiveTarget::new()
    }
}

/// How live events are placed inside the audio block that plays them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LiveTiming {
    /// Every event starts at the first sample of the next block. Lowest
    /// latency on average; timing jitter is up to one block.
    #[default]
    Immediate,
    /// Events keep the spacing they had when they arrived: an event lands
    /// at the offset it had since the previous block started. Latency is a
    /// constant one block more than the best case, with no jitter. Needs the
    /// block-start times reported by the audio callback; without them events
    /// start at offset zero.
    Spread,
}

/// Counters the UI can poll to light an input indicator.
#[derive(Default)]
pub struct LiveStats {
    /// Note-ons accepted from input threads.
    pub note_ons: AtomicU64,
    /// Note-offs accepted from input threads.
    pub note_offs: AtomicU64,
    /// Every event accepted (notes, pedal, all-notes-off).
    pub events: AtomicU64,
    /// Events refused because the queue was full.
    pub dropped: AtomicU64,
    /// Recorded notes refused because nobody drained the record ring.
    pub record_dropped: AtomicU64,
}

impl LiveStats {
    pub fn note_ons(&self) -> u64 {
        self.note_ons.load(Ordering::Relaxed)
    }

    pub fn events(&self) -> u64 {
        self.events.load(Ordering::Relaxed)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// One note event as it arrived, for recording into a pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordedNote {
    /// Instrument slot the note was played on.
    pub slot: Slot,
    pub key: u8,
    /// 0..=1; zero for a note-off.
    pub velocity: f32,
    pub on: bool,
    /// Transport position (quarter notes) from telemetry when the note
    /// arrived. Telemetry is updated once per audio block, so this is the
    /// position at the end of the last block rendered, not what the player
    /// was hearing: subtract the output latency when recording.
    pub beat: f64,
    /// The transport was running when the note arrived.
    pub playing: bool,
    /// The sustain pedal was down when the note arrived.
    pub sustain: bool,
    /// Arrival time on the live clock (nanoseconds, monotonic).
    pub time_ns: u64,
}

/// Control-side end of the record ring.
pub struct RecordedReceiver {
    rx: Consumer<RecordedNote>,
}

impl RecordedReceiver {
    pub fn pop(&mut self) -> Option<RecordedNote> {
        self.rx.pop().ok()
    }

    /// Moves everything waiting into `out`; returns how many were moved.
    pub fn drain_into(&mut self, out: &mut Vec<RecordedNote>) -> usize {
        let mut n = 0;
        while let Ok(note) = self.rx.pop() {
            out.push(note);
            n += 1;
        }
        n
    }
}

// ----------------------------------------------------------------------
// Shared state and the producer end
// ----------------------------------------------------------------------

/// State shared by the producer end, the engine and the control thread.
pub(crate) struct LiveShared {
    epoch: Instant,
    target: LiveTarget,
    spread: AtomicBool,
    pedal: AtomicBool,
    stats: LiveStats,
    telemetry: Arc<Telemetry>,
}

impl LiveShared {
    fn new(telemetry: Arc<Telemetry>) -> LiveShared {
        LiveShared {
            epoch: Instant::now(),
            target: LiveTarget::new(),
            spread: AtomicBool::new(false),
            pedal: AtomicBool::new(false),
            stats: LiveStats::default(),
            telemetry,
        }
    }

    /// Nanoseconds on the live clock.
    #[inline]
    pub(crate) fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    pub(crate) fn target(&self) -> &LiveTarget {
        &self.target
    }

    pub(crate) fn stats(&self) -> &LiveStats {
        &self.stats
    }

    pub(crate) fn set_timing(&self, timing: LiveTiming) {
        self.spread
            .store(timing == LiveTiming::Spread, Ordering::Relaxed);
    }

    pub(crate) fn timing(&self) -> LiveTiming {
        if self.spread.load(Ordering::Relaxed) {
            LiveTiming::Spread
        } else {
            LiveTiming::Immediate
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LiveKind {
    NoteOn { key: u8, velocity: f32 },
    NoteOff { key: u8 },
    Sustain { down: bool },
    AllNotesOff,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LiveEvent {
    /// Arrival time on the live clock.
    pub t_ns: u64,
    /// Instrument the event was played on ([`LIVE_TARGET_NONE`] if unknown).
    pub slot: u32,
    pub kind: LiveKind,
}

struct Tx {
    events: Producer<LiveEvent>,
    record: Producer<RecordedNote>,
}

struct Inner {
    shared: Arc<LiveShared>,
    tx: Mutex<Tx>,
}

/// The producer end of the live queue. Cheap to clone; every clone feeds the
/// same engine. All methods are safe to call from MIDI callback threads: they
/// never allocate and never wait for the audio thread. Each returns whether
/// the event was queued.
#[derive(Clone)]
pub struct LiveInput {
    inner: Arc<Inner>,
}

impl LiveInput {
    /// Plays or releases `key` on the instrument in `slot`.
    pub fn note(&self, slot: Slot, key: u8, velocity: f32, on: bool) -> bool {
        let key = key.min(127);
        let velocity = if on { clean_velocity(velocity) } else { 0.0 };
        let shared = &self.inner.shared;
        let t_ns = shared.now_ns();
        let kind = if on {
            LiveKind::NoteOn { key, velocity }
        } else {
            LiveKind::NoteOff { key }
        };
        let mut tx = self.lock();
        if !push_event(&mut tx, shared, LiveEvent { t_ns, slot, kind }, !on) {
            return false;
        }
        if on {
            shared.stats.note_ons.fetch_add(1, Ordering::Relaxed);
        } else {
            shared.stats.note_offs.fetch_add(1, Ordering::Relaxed);
        }
        if slot != LIVE_TARGET_NONE {
            let note = RecordedNote {
                slot,
                key,
                velocity,
                on,
                beat: shared.telemetry.beat(),
                playing: shared.telemetry.playing.load(Ordering::Relaxed),
                sustain: shared.pedal.load(Ordering::Relaxed),
                time_ns: t_ns,
            };
            if tx.record.push(note).is_err() {
                shared.stats.record_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        true
    }

    /// Plays or releases `key` on whatever instrument is the live target now.
    /// A note-on with no target is ignored (returns false); a note-off is
    /// always passed on so a note started before the target changed ends.
    pub fn note_to_target(&self, key: u8, velocity: f32, on: bool) -> bool {
        let slot = self.inner.shared.target.raw();
        if on && slot == LIVE_TARGET_NONE {
            return false;
        }
        self.note(slot, key, velocity, on)
    }

    /// Sustain pedal (MIDI CC64) down or up. The pedal is global: it holds
    /// the releases of every instrument.
    pub fn sustain(&self, down: bool) -> bool {
        let shared = &self.inner.shared;
        let t_ns = shared.now_ns();
        let ev = LiveEvent {
            t_ns,
            slot: LIVE_TARGET_NONE,
            kind: LiveKind::Sustain { down },
        };
        let mut tx = self.lock();
        let ok = push_event(&mut tx, shared, ev, !down);
        if ok {
            shared.pedal.store(down, Ordering::Relaxed);
        }
        ok
    }

    /// Releases every live note on `slot` now, including notes the pedal is
    /// holding (MIDI CC123 / CC120).
    pub fn all_notes_off(&self, slot: Slot) -> bool {
        let shared = &self.inner.shared;
        let ev = LiveEvent {
            t_ns: shared.now_ns(),
            slot,
            kind: LiveKind::AllNotesOff,
        };
        let mut tx = self.lock();
        push_event(&mut tx, shared, ev, true)
    }

    /// [`all_notes_off`](LiveInput::all_notes_off) on the current target.
    pub fn all_notes_off_target(&self) -> bool {
        match self.inner.shared.target.get() {
            Some(slot) => self.all_notes_off(slot),
            None => false,
        }
    }

    /// The instrument live notes are played on.
    pub fn target(&self) -> &LiveTarget {
        &self.inner.shared.target
    }

    pub fn stats(&self) -> &LiveStats {
        &self.inner.shared.stats
    }

    /// Whether the engine behind this input is gone.
    pub fn is_closed(&self) -> bool {
        self.lock().events.is_abandoned()
    }

    /// Queues an event with an explicit arrival time (timing tests).
    #[cfg(test)]
    pub(crate) fn push_at(&self, t_ns: u64, slot: Slot, kind: LiveKind) -> bool {
        let mut tx = self.lock();
        let ev = LiveEvent { t_ns, slot, kind };
        push_event(&mut tx, &self.inner.shared, ev, false)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tx> {
        // A panic in another MIDI thread must not silence this one.
        self.inner.tx.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn push_event(tx: &mut Tx, shared: &LiveShared, ev: LiveEvent, critical: bool) -> bool {
    let reserve = if critical { 0 } else { LIVE_QUEUE_RESERVE };
    if tx.events.slots() <= reserve || tx.events.push(ev).is_err() {
        shared.stats.dropped.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    shared.stats.events.fetch_add(1, Ordering::Relaxed);
    true
}

fn clean_velocity(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

// ----------------------------------------------------------------------
// Audio-thread side
// ----------------------------------------------------------------------

/// What the router asks the engine to do with a live event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LiveOp {
    On {
        slot: u32,
        key: u8,
        velocity: f32,
    },
    Off {
        slot: u32,
        key: u8,
    },
    /// Release every live note on the slot.
    AllOff {
        slot: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LiveAction {
    pub offset: u32,
    pub op: LiveOp,
}

/// The audio thread's half: the consumer, the pedal and held-key tables, and
/// the callback timing used to place events inside a block. Everything is
/// allocated in [`LiveState::new`].
pub(crate) struct LiveState {
    rx: Consumer<LiveEvent>,
    shared: Arc<LiveShared>,
    input: Option<LiveInput>,
    record: Option<RecordedReceiver>,
    sample_rate: f64,

    pedal: bool,
    /// Instrument each physically held key was played on.
    held: [u32; 128],
    /// Releases the pedal is holding back, oldest first.
    sustained: Vec<(u32, u8)>,

    /// Start of this and of the previous device callback, on the live clock.
    cb_ns: Option<u64>,
    prev_cb_ns: Option<u64>,
    /// Frames in this callback (0 if unknown) and how many are done.
    cb_frames: usize,
    cb_done: usize,

    /// Scratch for the actions one event expands to.
    pub actions: Vec<LiveAction>,
}

impl LiveState {
    pub fn new(telemetry: Arc<Telemetry>, sample_rate: f32) -> LiveState {
        let shared = Arc::new(LiveShared::new(telemetry));
        let (events_tx, rx) = RingBuffer::new(LIVE_QUEUE_CAPACITY);
        let (record_tx, record_rx) = RingBuffer::new(RECORD_CAPACITY);
        let input = LiveInput {
            inner: Arc::new(Inner {
                shared: shared.clone(),
                tx: Mutex::new(Tx {
                    events: events_tx,
                    record: record_tx,
                }),
            }),
        };
        LiveState {
            rx,
            shared,
            input: Some(input),
            record: Some(RecordedReceiver { rx: record_rx }),
            sample_rate: sample_rate as f64,
            pedal: false,
            held: [LIVE_TARGET_NONE; 128],
            sustained: Vec::with_capacity(MAX_SUSTAINED),
            cb_ns: None,
            prev_cb_ns: None,
            cb_frames: 0,
            cb_done: 0,
            actions: Vec::with_capacity(ACTION_CAPACITY),
        }
    }

    pub fn shared(&self) -> &Arc<LiveShared> {
        &self.shared
    }

    pub fn take_input(&mut self) -> Option<LiveInput> {
        self.input.take()
    }

    pub fn take_record(&mut self) -> Option<RecordedReceiver> {
        self.record.take()
    }

    /// Forgets the pedal and every held key (panic, device reset).
    pub fn reset(&mut self) {
        self.pedal = false;
        self.held = [LIVE_TARGET_NONE; 128];
        self.sustained.clear();
        self.shared.pedal.store(false, Ordering::Relaxed);
    }

    /// Notes the start of a device callback of `frames` frames (0 = unknown).
    pub fn begin_callback(&mut self, now_ns: u64, frames: usize) {
        self.prev_cb_ns = self.cb_ns;
        self.cb_ns = Some(now_ns);
        self.cb_frames = frames;
        self.cb_done = 0;
    }

    /// Time of the callback the engine is in, for `begin_callback`.
    pub fn now_ns(&self) -> u64 {
        self.shared.now_ns()
    }

    /// Marks `frames` rendered; the next chunk of the callback starts after.
    pub fn finish_chunk(&mut self, frames: usize) {
        self.cb_done += frames;
    }

    /// The next event that belongs in a chunk of `frames`, with its sample
    /// offset. Events that arrived after this callback began, or that fall
    /// in a later chunk of the callback, stay queued.
    pub fn next(&mut self, frames: usize) -> Option<(LiveEvent, u32)> {
        let ev = *self.rx.peek().ok()?;
        let offset = self.place(ev.t_ns, frames)?;
        let _ = self.rx.pop();
        Some((ev, offset))
    }

    /// Sample offset inside a chunk for an event that arrived at `t_ns`;
    /// `None` if it belongs to a later chunk or callback.
    fn place(&self, t_ns: u64, frames: usize) -> Option<u32> {
        let last = frames.saturating_sub(1) as u32;
        if self.shared.timing() == LiveTiming::Immediate {
            return Some(0);
        }
        let (Some(cb), Some(prev)) = (self.cb_ns, self.prev_cb_ns) else {
            return Some(0);
        };
        if t_ns > cb {
            return None;
        }
        let since_prev = t_ns.saturating_sub(prev) as f64 * 1e-9 * self.sample_rate;
        let local = since_prev - self.cb_done as f64;
        if local < 0.0 {
            return Some(0);
        }
        if local >= frames as f64 {
            let final_chunk = self.cb_frames == 0 || self.cb_done + frames >= self.cb_frames;
            return final_chunk.then_some(last);
        }
        Some((local as u32).min(last))
    }

    /// Applies the pedal and held-key rules to one event, appending what the
    /// engine must do to `out`. `offset` is where in the block it happens.
    pub fn route(&mut self, ev: LiveEvent, offset: u32, out: &mut Vec<LiveAction>) {
        match ev.kind {
            LiveKind::NoteOn { key, velocity } => {
                if ev.slot == LIVE_TARGET_NONE {
                    return;
                }
                let k = key as usize & 127;
                let prev = self.held[k];
                if prev != LIVE_TARGET_NONE && prev != ev.slot {
                    emit(out, offset, LiveOp::Off { slot: prev, key });
                }
                // A struck key takes over from a release the pedal holds.
                if let Some(i) = self
                    .sustained
                    .iter()
                    .position(|&(s, kk)| s == ev.slot && kk == key)
                {
                    self.sustained.remove(i);
                }
                self.held[k] = ev.slot;
                emit(
                    out,
                    offset,
                    LiveOp::On {
                        slot: ev.slot,
                        key,
                        velocity,
                    },
                );
            }
            LiveKind::NoteOff { key } => {
                let k = key as usize & 127;
                // The release follows the note-on, not the current target.
                let slot = std::mem::replace(&mut self.held[k], LIVE_TARGET_NONE);
                let slot = if slot == LIVE_TARGET_NONE {
                    ev.slot
                } else {
                    slot
                };
                if slot == LIVE_TARGET_NONE {
                    return;
                }
                if !self.pedal {
                    emit(out, offset, LiveOp::Off { slot, key });
                } else if !self.sustained.contains(&(slot, key)) {
                    if self.sustained.len() < MAX_SUSTAINED {
                        self.sustained.push((slot, key));
                    } else {
                        // Out of room: stop honoring the pedal for this note.
                        emit(out, offset, LiveOp::Off { slot, key });
                    }
                }
            }
            LiveKind::Sustain { down } => {
                if down == self.pedal {
                    return;
                }
                self.pedal = down;
                if !down {
                    for &(slot, key) in &self.sustained {
                        emit(out, offset, LiveOp::Off { slot, key });
                    }
                    self.sustained.clear();
                }
            }
            LiveKind::AllNotesOff => {
                let slot = ev.slot;
                if slot == LIVE_TARGET_NONE {
                    return;
                }
                for h in &mut self.held {
                    if *h == slot {
                        *h = LIVE_TARGET_NONE;
                    }
                }
                self.sustained.retain(|&(s, _)| s != slot);
                emit(out, offset, LiveOp::AllOff { slot });
            }
        }
    }

    #[cfg(test)]
    pub fn sustained_len(&self) -> usize {
        self.sustained.len()
    }

    /// Events waiting in the queue.
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.rx.slots()
    }
}

/// Appends an action unless the scratch is full (it never is: it is sized
/// for the largest expansion of one event).
#[inline]
fn emit(out: &mut Vec<LiveAction>, offset: u32, op: LiveOp) {
    if out.len() < out.capacity() {
        out.push(LiveAction { offset, op });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> LiveState {
        LiveState::new(Arc::new(Telemetry::new()), 48_000.0)
    }

    fn ev(slot: u32, kind: LiveKind) -> LiveEvent {
        LiveEvent {
            t_ns: 0,
            slot,
            kind,
        }
    }

    fn on(slot: u32, key: u8) -> LiveEvent {
        ev(slot, LiveKind::NoteOn { key, velocity: 0.5 })
    }

    fn off(slot: u32, key: u8) -> LiveEvent {
        ev(slot, LiveKind::NoteOff { key })
    }

    fn run(s: &mut LiveState, events: &[LiveEvent]) -> Vec<LiveOp> {
        let mut all = Vec::new();
        let mut out = Vec::with_capacity(ACTION_CAPACITY);
        for e in events {
            out.clear();
            s.route(*e, 0, &mut out);
            all.extend(out.iter().map(|a| a.op));
        }
        all
    }

    #[test]
    fn pedal_holds_releases_and_lifts_in_order() {
        let mut s = state();
        let ops = run(
            &mut s,
            &[
                ev(LIVE_TARGET_NONE, LiveKind::Sustain { down: true }),
                on(0, 60),
                on(0, 64),
                off(0, 64),
                off(0, 60),
            ],
        );
        assert_eq!(ops.len(), 2, "only the note-ons got through");
        assert_eq!(s.sustained_len(), 2);
        let ops = run(
            &mut s,
            &[ev(LIVE_TARGET_NONE, LiveKind::Sustain { down: false })],
        );
        assert_eq!(
            ops,
            vec![
                LiveOp::Off { slot: 0, key: 64 },
                LiveOp::Off { slot: 0, key: 60 }
            ]
        );
        assert_eq!(s.sustained_len(), 0);
    }

    #[test]
    fn sustained_storage_is_bounded() {
        let mut s = state();
        run(
            &mut s,
            &[ev(LIVE_TARGET_NONE, LiveKind::Sustain { down: true })],
        );
        let mut immediate = 0;
        // 6 slots x 128 keys is more than the store can hold.
        for slot in 0..6u32 {
            for key in 0..128u8 {
                let ops = run(&mut s, &[on(slot, key), off(slot, key)]);
                immediate += ops
                    .iter()
                    .filter(|o| matches!(o, LiveOp::Off { .. }))
                    .count();
                assert!(s.sustained_len() <= MAX_SUSTAINED);
            }
        }
        assert_eq!(s.sustained_len(), MAX_SUSTAINED);
        assert_eq!(immediate, 6 * 128 - MAX_SUSTAINED);
        assert_eq!(s.sustained.capacity(), MAX_SUSTAINED, "never reallocated");
        let out = run(
            &mut s,
            &[ev(LIVE_TARGET_NONE, LiveKind::Sustain { down: false })],
        );
        assert_eq!(out.len(), MAX_SUSTAINED);
    }

    #[test]
    fn the_release_goes_where_the_note_started() {
        let mut s = state();
        let ops = run(&mut s, &[on(3, 60), off(5, 60)]);
        assert_eq!(ops[1], LiveOp::Off { slot: 3, key: 60 });
        // Nothing held and nowhere to send it: ignored.
        let ops = run(&mut s, &[off(LIVE_TARGET_NONE, 61)]);
        assert!(ops.is_empty());
    }

    #[test]
    fn restriking_a_sustained_key_cancels_its_pending_release() {
        let mut s = state();
        let ops = run(
            &mut s,
            &[
                ev(LIVE_TARGET_NONE, LiveKind::Sustain { down: true }),
                on(0, 60),
                off(0, 60),
                on(0, 60),
                ev(LIVE_TARGET_NONE, LiveKind::Sustain { down: false }),
            ],
        );
        // Two note-ons, and no release: the second strike is still held.
        assert_eq!(
            ops.iter()
                .filter(|o| matches!(o, LiveOp::Off { .. }))
                .count(),
            0
        );
        assert_eq!(s.sustained_len(), 0);
    }

    #[test]
    fn all_notes_off_clears_the_slot_everywhere() {
        let mut s = state();
        run(
            &mut s,
            &[
                ev(LIVE_TARGET_NONE, LiveKind::Sustain { down: true }),
                on(1, 60),
                on(2, 62),
                off(2, 62),
                on(1, 64),
                off(1, 64),
            ],
        );
        let ops = run(&mut s, &[ev(1, LiveKind::AllNotesOff)]);
        assert_eq!(ops, vec![LiveOp::AllOff { slot: 1 }]);
        // Slot 2's sustained release is untouched, slot 1's is gone.
        assert_eq!(s.sustained_len(), 1);
        let ops = run(
            &mut s,
            &[ev(LIVE_TARGET_NONE, LiveKind::Sustain { down: false })],
        );
        assert_eq!(ops, vec![LiveOp::Off { slot: 2, key: 62 }]);
    }

    #[test]
    fn reset_forgets_the_pedal() {
        let mut s = state();
        run(
            &mut s,
            &[
                ev(LIVE_TARGET_NONE, LiveKind::Sustain { down: true }),
                on(0, 60),
                off(0, 60),
            ],
        );
        s.reset();
        assert_eq!(s.sustained_len(), 0);
        let ops = run(&mut s, &[on(0, 60), off(0, 60)]);
        assert_eq!(ops.len(), 2, "the pedal no longer holds the release");
    }

    fn timed(timing: LiveTiming, prev: u64, cb: u64, frames: usize) -> LiveState {
        let s = state();
        s.shared.set_timing(timing);
        let mut s = s;
        s.begin_callback(prev, frames);
        s.begin_callback(cb, frames);
        s
    }

    #[test]
    fn immediate_timing_ignores_arrival_times() {
        let s = timed(LiveTiming::Immediate, 0, 5_000_000, 256);
        assert_eq!(s.place(2_000_000, 256), Some(0));
        assert_eq!(s.place(9_000_000, 256), Some(0));
    }

    #[test]
    fn spread_timing_keeps_the_arrival_spacing() {
        // 256 frames at 48 kHz is 5.333 ms; the previous callback began at 0.
        let s = timed(LiveTiming::Spread, 0, 5_333_333, 256);
        assert_eq!(s.place(0, 256), Some(0));
        let mid = s.place(2_666_666, 256).unwrap();
        assert!((126..=129).contains(&mid), "{mid}");
        assert_eq!(s.place(5_000_000, 256), Some(240));
        // After the callback began: next callback.
        assert_eq!(s.place(5_400_000, 256), None);
    }

    #[test]
    fn spread_timing_uses_the_chunk_the_event_falls_in() {
        // One 1024-frame callback played as four 256-frame chunks. The event
        // is at frame ~513 of the callback, so it belongs to the third chunk.
        let mut s = timed(LiveTiming::Spread, 0, 21_333_333, 1024);
        let t = 10_700_000;
        assert_eq!(s.place(t, 256), None, "not in the first chunk");
        s.finish_chunk(256);
        assert_eq!(s.place(t, 256), None, "not in the second chunk");
        s.finish_chunk(256);
        assert_eq!(s.place(t, 256), Some(1));
    }

    #[test]
    fn a_stalled_callback_piles_late_events_at_the_end_of_the_block() {
        // The device stalled for 30 ms; the event is far past the 256 frames
        // the callback renders, and this is its last chunk.
        let s = timed(LiveTiming::Spread, 0, 30_000_000, 256);
        assert_eq!(s.place(20_000_000, 256), Some(255));
    }

    #[test]
    fn spread_timing_without_a_previous_callback_starts_immediately() {
        let mut s = state();
        s.shared.set_timing(LiveTiming::Spread);
        s.begin_callback(1_000_000, 256);
        assert_eq!(s.place(500_000, 256), Some(0));
    }

    #[test]
    fn events_older_than_the_previous_callback_start_at_zero() {
        let s = timed(LiveTiming::Spread, 10_000_000, 15_333_333, 256);
        assert_eq!(s.place(1_000_000, 256), Some(0));
    }
}
