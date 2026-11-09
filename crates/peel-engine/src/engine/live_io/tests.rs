//! Placement of live events inside a callback, against a recording device.

use std::sync::{Arc, Mutex};

use peel_dsp::{
    Device, DeviceDescriptor, DeviceEvent, DeviceKind, EventKind, ParamId, ProcessContext,
};

use crate::command::EngineCommand;
use crate::live::{LiveKind, LiveTiming, MAX_EVENTS_PER_BLOCK};
use crate::plan::{ChannelRoute, RenderPlan, SeqEvent, SeqKind, TrackRoute};
use crate::telemetry::Telemetry;
use crate::{Engine, EngineConfig};

static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "test.recorder",
    name: "Recorder",
    description: "Logs the events it receives",
    category: "Test",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &[],
};

/// `(frames rendered before the block, offset, event)`.
type Seen = Arc<Mutex<Vec<(u64, u32, EventKind)>>>;

struct Recorder {
    seen: Seen,
    rendered: u64,
}

impl Device for Recorder {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESC
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: ParamId, _value: f32) {}
    fn param(&self, _id: ParamId) -> f32 {
        0.0
    }
    fn process(
        &mut self,
        _ctx: &ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) {
        // The device contract: events arrive sorted and inside the block.
        assert!(events.windows(2).all(|w| w[0].offset <= w[1].offset));
        let mut seen = self.seen.lock().unwrap();
        for e in events {
            assert!((e.offset as usize) < left.len());
            seen.push((self.rendered, e.offset, e.kind));
        }
        self.rendered += left.len() as u64;
        left.fill(0.0);
        right.fill(0.0);
    }
    fn is_silent(&self) -> bool {
        false
    }
}

/// An engine with the recorder in slot 0 and a plan holding one sequence
/// note starting at frame 150 of the transport (120 bpm: a beat is 24000
/// frames, so beat 0.00625 is frame 150).
fn engine(seen: &Seen) -> Engine {
    let config = EngineConfig {
        max_block: 256,
        device_slots: 4,
        events_per_block: 64,
        ..EngineConfig::default()
    };
    let (tx, _rx) = rtrb::RingBuffer::new(64);
    let mut e = Engine::new(48_000.0, &config, Arc::new(Telemetry::new()), tx, None);
    e.set_monitoring(false);
    e.set_cpu_metering(false);
    e.handle_command(EngineCommand::InstallDevice {
        slot: 0,
        device: Box::new(Recorder {
            seen: seen.clone(),
            rendered: 0,
        }),
    });
    let mut plan = RenderPlan::empty();
    plan.channels = vec![ChannelRoute {
        slot: 0,
        track: 0,
        gain: 1.0,
        pan: 0.0,
        muted: false,
    }];
    plan.tracks = vec![TrackRoute::new(None, 0)];
    plan.sequence = vec![
        SeqEvent {
            beat: 0.00625,
            channel: 0,
            order: 1,
            kind: SeqKind::NoteOn {
                key: 40,
                velocity: 1.0,
                voice: 1,
            },
        },
        SeqEvent {
            beat: 4.0,
            channel: 0,
            order: 0,
            kind: SeqKind::NoteOff {
                key: 40,
                velocity: 0.0,
                voice: 1,
            },
        },
    ];
    plan.loop_range = None;
    plan.length_beats = 8.0;
    e.handle_command(EngineCommand::SetPlan(Box::new(plan)));
    e
}

fn on(key: u8) -> LiveKind {
    LiveKind::NoteOn { key, velocity: 0.5 }
}

/// `(offset, key)` of the note-ons in the block that started at `start`.
fn notes_at(seen: &Seen, start: u64) -> Vec<(u32, u8)> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|(s, ..)| *s == start)
        .filter_map(|&(_, off, kind)| match kind {
            EventKind::NoteOn { key, .. } => Some((off, key)),
            _ => None,
        })
        .collect()
}

const T0: u64 = 1_000_000_000;
/// 256 frames at 48 kHz.
const PERIOD: u64 = 5_333_333;

#[test]
fn spread_places_live_events_among_sequence_events_in_order() {
    let seen: Seen = Arc::default();
    let mut e = engine(&seen);
    e.set_live_timing(LiveTiming::Spread);
    let live = e.take_live_input().unwrap();
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);

    e.live.begin_callback(T0, 256);
    e.render(&mut l, &mut r);
    e.handle_command(EngineCommand::Play);

    // Two keys arrived 128 and 192 frames into the previous period, a third
    // just after this callback began.
    e.live.begin_callback(T0 + PERIOD, 256);
    assert!(live.push_at(T0 + 2_666_667, 0, on(60)));
    assert!(live.push_at(T0 + 4_000_000, 0, on(64)));
    assert!(live.push_at(T0 + PERIOD + 1_000, 0, on(67)));
    e.render(&mut l, &mut r);
    assert_eq!(
        notes_at(&seen, 256),
        vec![(128, 60), (150, 40), (192, 64)],
        "live notes keep their spacing and sort around the sequence note"
    );
    assert!(notes_at(&seen, 512).is_empty());

    // The late key starts the next callback, near its beginning.
    e.live.begin_callback(T0 + 2 * PERIOD, 256);
    e.render(&mut l, &mut r);
    assert_eq!(notes_at(&seen, 512), vec![(0, 67)]);
}

#[test]
fn immediate_timing_starts_every_live_event_at_the_block_start() {
    let seen: Seen = Arc::default();
    let mut e = engine(&seen);
    let live = e.take_live_input().unwrap();
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    e.live.begin_callback(T0, 256);
    e.render(&mut l, &mut r);
    e.handle_command(EngineCommand::Play);
    e.live.begin_callback(T0 + PERIOD, 256);
    live.push_at(T0 + 4_000_000, 0, on(64));
    live.push_at(T0 + PERIOD + 1_000, 0, on(67));
    e.render(&mut l, &mut r);
    assert_eq!(notes_at(&seen, 256), vec![(0, 64), (0, 67), (150, 40)]);
}

#[test]
fn spread_events_go_to_the_chunk_they_fall_in_when_a_callback_is_split() {
    let seen: Seen = Arc::default();
    let mut e = engine(&seen);
    e.set_live_timing(LiveTiming::Spread);
    let live = e.take_live_input().unwrap();
    let (mut l, mut r) = (vec![0.0f32; 1024], vec![0.0f32; 1024]);

    // A 1024-frame callback is rendered as four 256-frame chunks.
    let period = 21_333_333u64;
    e.live.begin_callback(T0, 1024);
    e.render(&mut l, &mut r);
    e.live.begin_callback(T0 + period, 1024);
    // Frames 100 and 600 (+ a fraction so the floor is exact) after the
    // previous callback began.
    live.push_at(T0 + 2_083_400, 0, on(60));
    live.push_at(T0 + 12_500_100, 0, on(62));
    e.render(&mut l, &mut r);
    assert_eq!(notes_at(&seen, 1024), vec![(100, 60)]);
    assert_eq!(notes_at(&seen, 1024 + 512), vec![(88, 62)]);
}

#[test]
fn live_events_wait_for_the_next_block_when_the_per_block_limit_is_reached() {
    let seen: Seen = Arc::default();
    let mut e = engine(&seen);
    let live = e.take_live_input().unwrap();
    let mut l = vec![0.0f32; 256];
    let mut r = vec![0.0f32; 256];
    // More events than one block applies: the rest wait, none are lost.
    for i in 0..900u32 {
        assert!(live.push_at(0, 0, on((i % 100) as u8)));
    }
    assert_eq!(e.live.pending(), 900);
    e.render(&mut l, &mut r);
    assert_eq!(e.live.pending(), 900 - MAX_EVENTS_PER_BLOCK);
    e.render(&mut l, &mut r);
    assert_eq!(e.live.pending(), 0);
}

#[test]
fn expression_keeps_sample_offsets_and_reset_preserves_notes() {
    let seen: Seen = Arc::default();
    let mut e = engine(&seen);
    e.set_live_timing(LiveTiming::Spread);
    let live = e.take_live_input().unwrap();
    live.target().set(Some(0));
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    e.live.begin_callback(T0, 256);
    e.render(&mut l, &mut r);
    e.live.begin_callback(T0 + PERIOD, 256);
    live.push_at(T0 + 1_333_333, 0, on(60));
    live.push_at(
        T0 + 2_666_667,
        0,
        LiveKind::Expression {
            kind: EventKind::PitchBend { semitones: 2.0 },
        },
    );
    live.push_at(
        T0 + 4_000_000,
        0,
        LiveKind::Expression {
            kind: EventKind::ResetExpression,
        },
    );
    e.render(&mut l, &mut r);
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter()
            .any(|&(_, offset, kind)| offset == 128
                && kind == EventKind::PitchBend { semitones: 2.0 })
    );
    assert!(
        seen.iter()
            .any(|&(_, offset, kind)| offset == 192 && kind == EventKind::ResetExpression)
    );
    assert!(
        !seen
            .iter()
            .any(|&(_, _, kind)| matches!(kind, EventKind::NoteOff { .. }))
    );
}
