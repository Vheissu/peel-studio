//! Test devices and plan builders shared by the engine tests.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use peel_dsp::{
    Device, DeviceDescriptor, DeviceEvent, DeviceKind, EventKind, ParamId, ProcessContext,
};
use peel_engine::{
    ChannelRoute, EngineConfig, OfflineEngine, RenderPlan, SeqEvent, SeqKind, TrackRoute,
};
use peel_time::TempoMap;

pub const SR: f32 = 48_000.0;

/// Param ids of [`Gate`].
pub const P_GAIN: ParamId = 0;
pub const P_POISON: ParamId = 1;

static GATE: DeviceDescriptor = DeviceDescriptor {
    type_id: "test.gate",
    name: "Gate",
    description: "Test instrument: DC level while a note is held",
    category: "Test",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &[],
};

static HALF: DeviceDescriptor = DeviceDescriptor {
    type_id: "test.half",
    name: "Half",
    description: "Test effect: scales by 0.5",
    category: "Test",
    kind: DeviceKind::Effect,
    version: 1,
    params: &[],
};

/// One event as the device received it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Logged {
    /// Frames the device had rendered before this block.
    pub block_start: u64,
    pub offset: u32,
    pub kind: EventKind,
}

impl Logged {
    pub fn abs(&self) -> u64 {
        self.block_start + self.offset as u64
    }
}

pub type Log = Arc<Mutex<Vec<Logged>>>;

/// Instrument whose output is the sum of the velocities of its held notes,
/// on both channels. Its output therefore shows exactly when events arrived.
pub struct Gate {
    voices: [Option<(u32, f32)>; 64],
    gain: f32,
    poison: bool,
    rendered: u64,
    log: Option<Log>,
    pub resets: Arc<AtomicUsize>,
}

impl Gate {
    pub fn new() -> Gate {
        Gate {
            voices: [None; 64],
            gain: 1.0,
            poison: false,
            rendered: 0,
            log: None,
            resets: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn with_log(log: Log) -> Gate {
        let mut g = Gate::new();
        g.log = Some(log);
        g
    }

    fn held(&self) -> f32 {
        self.voices.iter().flatten().map(|v| v.1).sum::<f32>() * self.gain
    }
}

impl Device for Gate {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &GATE
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize) {}

    fn reset(&mut self) {
        self.voices = [None; 64];
        self.poison = false;
        self.resets.fetch_add(1, Ordering::Relaxed);
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        match id {
            P_GAIN => self.gain = value,
            P_POISON => self.poison = value >= 0.5,
            _ => {}
        }
    }

    fn param(&self, id: ParamId) -> f32 {
        match id {
            P_GAIN => self.gain,
            P_POISON => f32::from(u8::from(self.poison)),
            _ => 0.0,
        }
    }

    fn process(
        &mut self,
        _ctx: &ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) {
        if let Some(log) = &self.log {
            let mut log = log.lock().unwrap();
            for e in events {
                log.push(Logged {
                    block_start: self.rendered,
                    offset: e.offset,
                    kind: e.kind,
                });
            }
        }
        let mut next = 0;
        for i in 0..left.len() {
            while next < events.len() && events[next].offset as usize == i {
                match events[next].kind {
                    EventKind::NoteOn {
                        voice, velocity, ..
                    } => {
                        if let Some(slot) = self.voices.iter_mut().find(|v| v.is_none()) {
                            *slot = Some((voice, velocity));
                        }
                    }
                    EventKind::NoteOff { voice, .. } => {
                        for v in self.voices.iter_mut() {
                            if v.is_some_and(|(id, _)| id == voice) {
                                *v = None;
                            }
                        }
                    }
                    EventKind::AllNotesOff | EventKind::AllSoundOff => {
                        self.voices = [None; 64];
                    }
                    _ => {}
                }
                next += 1;
            }
            let v = if self.poison && i == 0 {
                f32::NAN
            } else {
                self.held()
            };
            left[i] = v;
            right[i] = v;
        }
        self.rendered += left.len() as u64;
    }

    fn is_silent(&self) -> bool {
        // A logging gate must see every block to count frames.
        self.log.is_none() && !self.poison && self.voices.iter().all(Option::is_none)
    }
}

/// Effect that halves its input.
pub struct Half;

impl Device for Half {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &HALF
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
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) {
        for x in left.iter_mut().chain(right.iter_mut()) {
            *x *= 0.5;
        }
    }
    fn is_silent(&self) -> bool {
        true
    }
}

/// A note for [`plan`].
#[derive(Clone, Copy)]
pub struct Note {
    pub beat: f64,
    pub len: f64,
    pub key: u8,
    pub velocity: f32,
    pub voice: u32,
    pub channel: u16,
}

pub fn note(beat: f64, len: f64, voice: u32) -> Note {
    Note {
        beat,
        len,
        key: 60,
        velocity: 1.0,
        voice,
        channel: 0,
    }
}

/// Builds a plan with `channels` gate channels (slot = channel index) each
/// feeding mixer track 0, which feeds the master (track 1).
pub fn plan(
    tempo: TempoMap,
    notes: &[Note],
    channels: usize,
    loop_range: Option<(f64, f64)>,
    length_beats: f64,
) -> RenderPlan {
    let mut sequence = Vec::new();
    for n in notes {
        sequence.push(SeqEvent {
            beat: n.beat,
            channel: n.channel,
            order: 1,
            kind: SeqKind::NoteOn {
                key: n.key,
                velocity: n.velocity,
                voice: n.voice,
            },
        });
        sequence.push(SeqEvent {
            beat: n.beat + n.len,
            channel: n.channel,
            order: 0,
            kind: SeqKind::NoteOff {
                key: n.key,
                velocity: 0.0,
                voice: n.voice,
            },
        });
    }
    sequence.sort_by(|a, b| a.beat.total_cmp(&b.beat).then(a.order.cmp(&b.order)));
    RenderPlan {
        revision: 1,
        tempo,
        sequence,
        expression_notes: Vec::new(),
        expression_chase: Vec::new(),
        note_identities: Vec::new(),
        note_identity_index: Vec::new(),
        length_beats,
        loop_range,
        audio: Vec::new(),
        automation: Vec::new(),
        channels: (0..channels)
            .map(|c| ChannelRoute {
                slot: c as u32,
                track: 0,
                gain: 1.0,
                pan: 0.0,
                muted: false,
            })
            .collect(),
        tracks: vec![TrackRoute::new(Some(1), 0), TrackRoute::new(None, 1)],
        master: 1,
        beats_per_bar: 4,
    }
}

pub fn config() -> EngineConfig {
    EngineConfig {
        max_block: 256,
        device_slots: 32,
        retire_capacity: 256,
        events_per_block: 64,
        ..EngineConfig::default()
    }
}

/// An offline engine with a plan installed and one gate per channel.
pub fn rig(plan: RenderPlan, log: Option<Log>) -> OfflineEngine {
    rig_with(plan, log, config())
}

pub fn rig_with(plan: RenderPlan, log: Option<Log>, config: EngineConfig) -> OfflineEngine {
    use peel_engine::EngineCommand;
    let channels = plan.channels.len();
    let mut off = OfflineEngine::new(SR, &config);
    for c in 0..channels {
        let gate = match &log {
            Some(l) if c == 0 => Gate::with_log(l.clone()),
            _ => Gate::new(),
        };
        off.send(EngineCommand::InstallDevice {
            slot: c as u32,
            device: Box::new(gate),
        });
    }
    off.send(EngineCommand::SetPlan(Box::new(plan)));
    off
}

/// Renders `total` frames in calls of `block` frames.
pub fn render_blocks(off: &mut OfflineEngine, total: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0; total];
    let mut r = vec![0.0; total];
    let mut done = 0;
    while done < total {
        let n = block.min(total - done);
        off.render(&mut l[done..done + n], &mut r[done..done + n]);
        done += n;
    }
    (l, r)
}

/// Indices where the signal goes from zero to non-zero.
pub fn rising_edges(buf: &[f32]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev = 0.0f32;
    for (i, &x) in buf.iter().enumerate() {
        if prev == 0.0 && x != 0.0 {
            out.push(i);
        }
        prev = x;
    }
    out
}

/// Indices where the signal goes from non-zero to zero.
pub fn falling_edges(buf: &[f32]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev = 0.0f32;
    for (i, &x) in buf.iter().enumerate() {
        if prev != 0.0 && x == 0.0 {
            out.push(i);
        }
        prev = x;
    }
    out
}
