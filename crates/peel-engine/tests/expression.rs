mod common;

use common::*;
use peel_dsp::{Device, DeviceDescriptor, DeviceEvent, EventKind, ParamId, ProcessContext};
use peel_engine::{
    ChannelRoute, EngineCommand as Cmd, ExpressionNote, ExpressionState, OfflineEngine, RenderPlan,
    SeqEvent, SeqKind,
};

const BEAT: usize = 24_000;

#[derive(Clone, Copy)]
struct Voice {
    id: u32,
    pitch: f32,
    pressure: f32,
    timbre: f32,
    remaining: Option<usize>,
}
struct Probe {
    voices: [Option<Voice>; 8],
    release: usize,
}
impl Device for Probe {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        static DESC: DeviceDescriptor = DeviceDescriptor {
            type_id: "test.expression",
            name: "Expression",
            description: "Expression test probe",
            category: "Test",
            kind: peel_dsp::DeviceKind::Instrument,
            version: 1,
            params: &[],
        };
        &DESC
    }
    fn prepare(&mut self, _: f32, _: usize) {}
    fn reset(&mut self) {
        self.voices.fill(None);
    }
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn param(&self, _: ParamId) -> f32 {
        0.0
    }
    fn process(
        &mut self,
        _: &ProcessContext,
        events: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) {
        let mut next = 0;
        for (i, (l, r)) in l.iter_mut().zip(r).enumerate() {
            while let Some(event) = events.get(next).filter(|e| e.offset as usize <= i) {
                match event.kind {
                    EventKind::NoteOn { voice, .. } => {
                        if let Some(slot) = self.voices.iter_mut().find(|v| v.is_none()) {
                            *slot = Some(Voice {
                                id: voice,
                                pitch: 0.0,
                                pressure: 0.0,
                                timbre: 0.5,
                                remaining: None,
                            });
                        }
                    }
                    EventKind::VoiceExpression {
                        voice,
                        pitch,
                        pressure,
                        timbre,
                    } => {
                        for v in self.voices.iter_mut().flatten().filter(|v| v.id == voice) {
                            *v = Voice {
                                id: voice,
                                pitch,
                                pressure,
                                timbre,
                                remaining: v.remaining,
                            };
                        }
                    }
                    EventKind::NoteOff { voice, .. } => {
                        for slot in &mut self.voices {
                            if let Some(v) = slot.as_mut().filter(|v| v.id == voice) {
                                if self.release == 0 {
                                    *slot = None;
                                } else {
                                    v.remaining = Some(self.release);
                                }
                            }
                        }
                    }
                    EventKind::AllSoundOff | EventKind::AllNotesOff => self.voices.fill(None),
                    _ => {}
                }
                next += 1;
            }
            *l = self
                .voices
                .iter()
                .flatten()
                .map(|v| (1.0 + v.pitch / 120.0 + v.pressure + v.timbre) * 0.1)
                .sum();
            *r = *l;
            for slot in &mut self.voices {
                if let Some(remaining) = slot.as_mut().and_then(|v| v.remaining.as_mut()) {
                    *remaining -= 1;
                    if *remaining == 0 {
                        *slot = None;
                    }
                }
            }
        }
    }
}
fn snapshot(beat: f64, voice: u32, pitch: f32, pressure: f32, timbre: f32) -> SeqEvent {
    SeqEvent {
        beat,
        channel: 0,
        order: 2,
        kind: SeqKind::Expression {
            voice,
            pitch,
            pressure,
            timbre,
        },
    }
}
fn curved_plan() -> RenderPlan {
    let mut p = RenderPlan::empty();
    p.loop_range = None;
    p.length_beats = 2.0;
    p.channels = vec![ChannelRoute {
        slot: 0,
        track: 0,
        gain: 1.0,
        pan: 0.0,
        muted: false,
    }];
    p.sequence = vec![
        SeqEvent {
            beat: 0.0,
            channel: 0,
            order: 1,
            kind: SeqKind::NoteOn {
                key: 60,
                velocity: 1.0,
                voice: 1,
            },
        },
        snapshot(0.0, 1, 0.0, 0.0, 0.5),
        snapshot(1.0, 1, 12.0, 0.8, 1.0),
        SeqEvent {
            beat: 2.0,
            channel: 0,
            order: 0,
            kind: SeqKind::NoteOff {
                key: 60,
                velocity: 0.0,
                voice: 1,
            },
        },
    ];
    p.expression_notes = vec![ExpressionNote {
        start_beat: 0.0,
        end_beat: 2.0,
        channel: 0,
        key: 60,
        velocity: 1.0,
        voice: 1,
        points: vec![
            ExpressionState {
                beat: 0.0,
                pitch: 0.0,
                pressure: 0.0,
                timbre: 0.5,
            },
            ExpressionState {
                beat: 1.0,
                pitch: 12.0,
                pressure: 0.8,
                timbre: 1.0,
            },
        ],
    }];
    p
}
fn engine(p: RenderPlan) -> OfflineEngine {
    let mut e = OfflineEngine::new(SR, &config());
    e.send(Cmd::InstallDevice {
        slot: 0,
        device: Box::new(Probe {
            voices: [None; 8],
            release: 0,
        }),
    });
    e.try_set_plan(Box::new(p)).unwrap();
    e
}
fn assert_value(value: f32, expected: f32) {
    assert!((value - expected).abs() < 1e-6, "{value} != {expected}");
}

#[test]
fn recorded_expression_is_sample_accurate_and_independent_of_chunk_size() {
    let a = curved_plan();
    let mut e = engine(a.clone());
    e.send(Cmd::Play);
    let (a, _) = render_blocks(&mut e, BEAT * 2 + 32, 37);
    let mut e = engine(curved_plan());
    e.send(Cmd::Play);
    let (b, _) = render_blocks(&mut e, BEAT * 2 + 32, 256);
    assert_eq!(a, b);
    assert_value(a[BEAT - 1], 0.15);
    assert_value(a[BEAT], 0.29);
    assert_value(a[BEAT * 2], 0.0);
}

#[test]
fn a_refused_last_expression_point_recovers_when_the_next_chunk_has_room() {
    use peel_engine::EngineConfig;
    let config = EngineConfig {
        events_per_block: 4,
        max_block: 256,
        ..config()
    };
    let mut e = OfflineEngine::new(SR, &config);
    e.send(Cmd::InstallDevice {
        slot: 0,
        device: Box::new(Probe {
            voices: [None; 8],
            release: 0,
        }),
    });
    e.try_set_plan(Box::new(curved_plan())).unwrap();
    e.send(Cmd::Play);
    render_blocks(&mut e, BEAT - 1, 256);
    for key in 70..74 {
        e.send(Cmd::LiveNote {
            slot: 0,
            key,
            velocity: 1.0,
            on: true,
        });
    }
    let (before, _) = render_blocks(&mut e, 32, 32);
    assert_value(before[31], 0.75); // four queued live notes consume the soft cap
    let (after, _) = render_blocks(&mut e, 32, 32);
    assert_value(after[0], 0.89); // the held sequence voice receives its deferred last point
}

#[test]
fn expression_events_follow_the_tempo_map_during_a_ramp() {
    use peel_time::{TempoMap, TempoSegment, Time};
    let mut p = curved_plan();
    p.tempo = TempoMap::from_segments(&[
        TempoSegment {
            start: Time::ZERO,
            bpm: 80.0,
            ramp_to: Some(200.0),
        },
        TempoSegment {
            start: Time::beats(2),
            bpm: 200.0,
            ramp_to: None,
        },
    ]);
    let change = p.tempo.beat_to_sample(1.0, SR as f64) as usize;
    let end = p.tempo.beat_to_sample(2.0, SR as f64) as usize;
    let mut e = engine(p);
    e.send(Cmd::Play);
    let (out, _) = render_blocks(&mut e, end + 32, 137);
    assert_value(out[change - 1], 0.15);
    assert_value(out[change], 0.29);
    assert_value(out[end], 0.0);
}

#[test]
fn seeking_pausing_and_plan_edits_chase_the_current_note_expression() {
    let mut e = engine(curved_plan());
    e.send(Cmd::Seek { beat: 1.25 });
    e.send(Cmd::Play);
    let (a, _) = render_blocks(&mut e, 128, 128);
    assert_value(a[0], 0.29);
    e.send(Cmd::Pause);
    e.send(Cmd::Play);
    let (a, _) = render_blocks(&mut e, 128, 128);
    assert_value(a[0], 0.29);
    let mut changed = curved_plan();
    changed.revision = 2;
    changed.expression_notes[0].points[1].pressure = 0.2;
    if let SeqKind::Expression { pressure, .. } = &mut changed.sequence[2].kind {
        *pressure = 0.2;
    }
    e.try_set_plan(Box::new(changed)).unwrap();
    let (a, _) = render_blocks(&mut e, 128, 128);
    assert_value(a[0], 0.23);
    e.send(Cmd::Stop { return_to: 0.0 });
    let (a, _) = render_blocks(&mut e, 128, 128);
    assert!(a.iter().all(|&v| v == 0.0));
}

#[test]
fn a_loop_start_inside_a_note_chases_expression_every_wrap() {
    let mut p = curved_plan();
    p.loop_range = Some((0.5, 1.5));
    let mut e = engine(p);
    e.send(Cmd::Seek { beat: 0.5 });
    e.send(Cmd::Play);
    let (out, _) = render_blocks(&mut e, BEAT * 3, 137);
    for iteration in 0..3 {
        assert_value(out[iteration * BEAT], 0.15);
        assert_value(out[iteration * BEAT + BEAT / 2], 0.29);
    }
}

#[test]
fn same_key_voices_keep_independent_recorded_expression() {
    let mut p = curved_plan();
    p.sequence.extend([
        SeqEvent {
            beat: 0.0,
            channel: 0,
            order: 1,
            kind: SeqKind::NoteOn {
                key: 60,
                velocity: 1.0,
                voice: 2,
            },
        },
        snapshot(0.0, 2, -12.0, 0.2, 0.0),
        SeqEvent {
            beat: 2.0,
            channel: 0,
            order: 0,
            kind: SeqKind::NoteOff {
                key: 60,
                velocity: 0.0,
                voice: 2,
            },
        },
    ]);
    p.sequence
        .sort_by(|a, b| a.beat.total_cmp(&b.beat).then(a.order.cmp(&b.order)));
    let mut second = p.expression_notes[0].clone();
    second.voice = 2;
    second.points = vec![ExpressionState {
        beat: 0.0,
        pitch: -12.0,
        pressure: 0.2,
        timbre: 0.0,
    }];
    p.expression_notes.push(second);
    let mut e = engine(p);
    e.send(Cmd::Seek { beat: 1.25 });
    e.send(Cmd::Play);
    let (out, _) = render_blocks(&mut e, 256, 37);
    assert_value(out[0], 0.4);
}

#[test]
fn loop_articulations_do_not_retarget_the_previous_release_tail() {
    let mut p = curved_plan();
    p.loop_range = Some((0.0, 0.0005)); // twelve samples at 120 BPM
    p.length_beats = 0.001;
    p.sequence[1] = snapshot(0.0, 1, 12.0, 1.0, 1.0);
    p.sequence[2] = snapshot(0.00025, 1, -12.0, 0.0, 0.0);
    p.sequence[3].beat = 0.001;
    p.expression_notes[0].end_beat = 0.001;
    p.expression_notes[0].points = vec![
        ExpressionState {
            beat: 0.0,
            pitch: 12.0,
            pressure: 1.0,
            timbre: 1.0,
        },
        ExpressionState {
            beat: 0.00025,
            pitch: -12.0,
            pressure: 0.0,
            timbre: 0.0,
        },
    ];
    let mut e = engine(p);
    e.send(Cmd::InstallDevice {
        slot: 0,
        device: Box::new(Probe {
            voices: [None; 8],
            release: 16,
        }),
    });
    e.send(Cmd::Play);
    let (out, _) = render_blocks(&mut e, 40, 7);
    assert_value(out[0], 0.31);
    assert_value(out[6], 0.09);
    assert_value(out[12], 0.40);
    assert_value(out[18], 0.18);
    assert_value(out[24], 0.49);
}

#[cfg(all(feature = "rt-check", debug_assertions))]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

#[cfg(all(feature = "rt-check", debug_assertions))]
#[test]
fn expression_playback_seek_loop_and_plan_chase_never_allocate_or_free() {
    use peel_engine::{Engine, EngineConfig, Telemetry};
    use std::sync::Arc;
    let mut p = curved_plan();
    p.loop_range = Some((0.5, 1.5));
    let config = EngineConfig {
        device_slots: 4,
        max_block: 256,
        events_per_block: 4,
        ..config()
    };
    let make = |p: RenderPlan| Cmd::SetCompensatedPlan {
        compensation: Box::new(
            peel_engine::compensation::Compensation::prepare(&p, &[0], 256).unwrap(),
        ),
        plan: Box::new(p),
    };
    let mut replacement = Some(make(p.clone()));
    let (tx, _retired) = rtrb::RingBuffer::new(64);
    let mut e = Engine::new(SR, &config, Arc::new(Telemetry::new()), tx, None);
    e.handle_command(Cmd::InstallDevice {
        slot: 0,
        device: Box::new(Probe {
            voices: [None; 8],
            release: 0,
        }),
    });
    e.handle_command(make(p));
    e.handle_command(Cmd::Seek { beat: 0.5 });
    e.handle_command(Cmd::Play);
    let (mut l, mut r) = ([0.0; 256], [0.0; 256]);
    assert_no_alloc::assert_no_alloc(|| {
        for i in 0..1000 {
            if i == 46 {
                // Fill the exact chunk containing the next expression point.
                for key in 70..74 {
                    e.handle_command(Cmd::LiveNote {
                        slot: 0,
                        key,
                        velocity: 1.0,
                        on: true,
                    });
                }
            }
            if i == 200 {
                e.handle_command(replacement.take().unwrap());
            }
            if i == 400 {
                e.handle_command(Cmd::Seek { beat: 1.25 });
            }
            e.render(&mut l, &mut r);
        }
    });
}
