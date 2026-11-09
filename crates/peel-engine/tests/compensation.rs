mod common;

use common::*;
use peel_dsp::{Device, DeviceDescriptor, DeviceEvent, ParamId, ProcessContext};
use peel_engine::{
    ChannelRoute, EngineCommand as Cmd, EngineConfig, OfflineEngine, RenderPlan, TrackRoute,
    TrackSend,
};

struct Latent {
    instrument: Option<Gate>,
    latency: usize,
    ring: [[f32; 2]; 128],
    cursor: usize,
}
impl Latent {
    fn effect(latency: usize) -> Self {
        Self {
            instrument: None,
            latency,
            ring: [[0.0; 2]; 128],
            cursor: 0,
        }
    }
    fn instrument(latency: usize) -> Self {
        Self {
            instrument: Some(Gate::new()),
            ..Self::effect(latency)
        }
    }
}
impl Device for Latent {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        Half.descriptor()
    }
    fn prepare(&mut self, _: f32, _: usize) {}
    fn reset(&mut self) {
        self.ring.fill([0.0; 2]);
        self.cursor = 0;
        if let Some(g) = &mut self.instrument {
            g.reset();
        }
    }
    fn set_param(&mut self, _: ParamId, v: f32) {
        self.latency = v.clamp(0.0, 127.0) as usize;
        self.reset();
    }
    fn param(&self, _: ParamId) -> f32 {
        self.latency as f32
    }
    fn latency(&self) -> u32 {
        self.latency as u32
    }
    fn process(
        &mut self,
        ctx: &ProcessContext,
        events: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
    ) {
        if let Some(g) = &mut self.instrument {
            g.process(ctx, events, l, r);
        }
        if self.latency == 0 {
            return;
        }
        for (l, r) in l.iter_mut().zip(r) {
            let prev = self.ring[self.cursor];
            self.ring[self.cursor] = [*l, *r];
            self.cursor = (self.cursor + 1) % self.latency;
            *l = prev[0];
            *r = prev[1];
        }
    }
}
struct SubtractKey;
impl Device for SubtractKey {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        Half.descriptor()
    }
    fn prepare(&mut self, _: f32, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn param(&self, _: ParamId) -> f32 {
        0.0
    }
    fn process(&mut self, _: &ProcessContext, _: &[DeviceEvent], _: &mut [f32], _: &mut [f32]) {}
    fn process_keyed(
        &mut self,
        _: &ProcessContext,
        _: &[DeviceEvent],
        l: &mut [f32],
        r: &mut [f32],
        kl: &[f32],
        kr: &[f32],
    ) {
        for i in 0..l.len() {
            l[i] -= kl[i];
            r[i] -= kr[i];
        }
    }
}
fn engine() -> OfflineEngine {
    OfflineEngine::new(
        SR,
        &EngineConfig {
            device_slots: 16,
            max_block: 13,
            ..EngineConfig::default()
        },
    )
}
fn channel(slot: u32, track: u16) -> ChannelRoute {
    ChannelRoute {
        slot,
        track,
        gain: 1.0,
        pan: 0.0,
        muted: false,
    }
}
fn install(engine: &mut OfflineEngine, slot: u32, device: impl Device + 'static) {
    engine.send(Cmd::InstallDevice {
        slot,
        device: Box::new(device),
    });
}
fn impulse(engine: &mut OfflineEngine, slots: &[u32]) {
    for &slot in slots {
        engine.send(Cmd::LiveNote {
            slot,
            key: 60,
            velocity: 1.0,
            on: true,
        });
    }
    let (mut l, mut r) = ([0.0], [0.0]);
    engine.render(&mut l, &mut r);
    for &slot in slots {
        engine.send(Cmd::LiveNote {
            slot,
            key: 60,
            velocity: 0.0,
            on: false,
        });
    }
}
fn render(engine: &mut OfflineEngine, n: usize) -> Vec<f32> {
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    engine.render(&mut l, &mut r);
    assert_eq!(l, r);
    l
}

#[test]
fn nested_buses_and_parallel_sends_align_with_latent_instruments() {
    let mut e = engine();
    install(&mut e, 0, Gate::new());
    install(&mut e, 1, Latent::instrument(17));
    install(&mut e, 2, Latent::effect(7));
    let mut p = RenderPlan::empty();
    p.channels = vec![channel(0, 0), channel(1, 1)];
    p.tracks = vec![
        TrackRoute::new(Some(2), 0),
        TrackRoute::new(Some(3), 1),
        TrackRoute::new(Some(3), 2),
        TrackRoute::new(None, 3),
    ];
    p.tracks[0].sends.push(TrackSend {
        dest: 3,
        gain: 0.5,
        last: peel_engine::AUTOMATION_UNSET,
    });
    p.tracks[2].inserts.push(2);
    p.master = 3;
    e.try_set_plan(Box::new(p)).unwrap();
    assert_eq!(e.compensation_latency_samples(), 17);
    impulse(&mut e, &[0, 1]);
    let out = render(&mut e, 48);
    assert_eq!(out.iter().filter(|&&x| x != 0.0).count(), 1, "{out:?}");
    assert_eq!(out[16], 2.5);
}

#[test]
fn sidechain_arrives_at_the_actual_insert_input() {
    let mut e = engine();
    install(&mut e, 0, Latent::instrument(23));
    install(&mut e, 1, Gate::new());
    install(&mut e, 2, Latent::effect(11));
    install(&mut e, 3, SubtractKey);
    let mut p = RenderPlan::empty();
    p.channels = vec![channel(0, 0), channel(1, 1)];
    p.tracks = vec![
        TrackRoute::new(None, 0),
        TrackRoute::new(Some(2), 1),
        TrackRoute::new(None, 2),
    ];
    p.tracks[0].key_source = true;
    p.tracks[1].inserts = vec![2, 3];
    p.tracks[1].keys = vec![(3, 0)];
    p.master = 2;
    e.try_set_plan(Box::new(p)).unwrap();
    assert_eq!(e.compensation_latency_samples(), 23);
    impulse(&mut e, &[0, 1]);
    let out = render(&mut e, 64);
    assert!(
        out.iter().all(|&v| v == 0.0),
        "main/key failed to null: {out:?}"
    );
}

#[test]
fn latency_changes_bypass_and_removal_reprepare_without_stale_audio() {
    let mut e = engine();
    install(&mut e, 0, Gate::new());
    install(&mut e, 1, Latent::effect(17));
    let mut p = RenderPlan::empty();
    p.channels = vec![channel(0, 0)];
    p.tracks[0].inserts.push(1);
    e.try_set_plan(Box::new(p)).unwrap();
    e.send(Cmd::SetParam {
        slot: 1,
        id: 0,
        value: 7.0,
    });
    render(&mut e, 1);
    assert_eq!(e.compensation_latency_samples(), 7);
    e.send(Cmd::SetBypass {
        slot: 1,
        bypass: true,
    });
    render(&mut e, 256);
    assert_eq!(e.compensation_latency_samples(), 0);
    e.send(Cmd::RemoveDevice { slot: 1 });
    assert_eq!(e.compensation_latency_samples(), 0);
    impulse(&mut e, &[0]);
    assert!(render(&mut e, 32).iter().all(|&v| v == 0.0));
}

#[test]
fn seek_stop_and_panic_clear_pending_compensation() {
    for command in [
        Cmd::Seek { beat: 1.0 },
        Cmd::Stop { return_to: 0.0 },
        Cmd::Panic,
    ] {
        let mut e = engine();
        install(&mut e, 0, Gate::new());
        install(&mut e, 1, Latent::instrument(40));
        let mut p = RenderPlan::empty();
        p.channels = vec![channel(0, 0), channel(1, 0)];
        e.try_set_plan(Box::new(p)).unwrap();
        // Only the dry source is triggered. Its impulse is pending in PDC.
        impulse(&mut e, &[0]);
        e.send(command);
        assert!(render(&mut e, 64).iter().all(|&v| v == 0.0));
    }
}

#[test]
fn excessive_latency_rejects_the_change_and_keeps_the_working_graph() {
    struct Huge;
    impl Device for Huge {
        fn descriptor(&self) -> &'static DeviceDescriptor {
            Half.descriptor()
        }
        fn prepare(&mut self, _: f32, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: ParamId, _: f32) {}
        fn param(&self, _: ParamId) -> f32 {
            0.0
        }
        fn latency(&self) -> u32 {
            u32::MAX
        }
        fn process(&mut self, _: &ProcessContext, _: &[DeviceEvent], _: &mut [f32], _: &mut [f32]) {
        }
    }
    let mut e = engine();
    install(&mut e, 0, Gate::new());
    let mut p = RenderPlan::empty();
    p.channels.push(channel(0, 0));
    e.try_set_plan(Box::new(p)).unwrap();
    install(&mut e, 0, Huge);
    assert!(e.compensation_error().is_some());
    e.send(Cmd::LiveNote {
        slot: 0,
        key: 60,
        velocity: 1.0,
        on: true,
    });
    assert_eq!(render(&mut e, 1), vec![1.0]);
}

#[test]
fn looping_keeps_paths_aligned_even_when_the_loop_is_shorter_than_latency() {
    for period in [12usize, 64] {
        let mut e = engine();
        install(&mut e, 0, Gate::new());
        install(&mut e, 1, Latent::instrument(40));
        let beat = period as f64 / 24_000.0;
        let notes = [
            note(0.0, 1.0 / 24_000.0, 1),
            Note {
                channel: 1,
                ..note(0.0, 1.0 / 24_000.0, 2)
            },
        ];
        let p = plan(
            peel_time::TempoMap::constant(120.0),
            &notes,
            2,
            Some((0.0, beat)),
            beat,
        );
        e.try_set_plan(Box::new(p)).unwrap();
        e.send(Cmd::Play);
        let out = render(&mut e, 256);
        for (i, &sample) in out.iter().enumerate() {
            let expected = if i >= 40 && (i - 40) % period == 0 {
                2.0
            } else {
                0.0
            };
            assert_eq!(sample, expected, "period {period}, frame {i}");
        }
    }
}

#[test]
fn timeline_audio_and_latent_instruments_null_at_their_common_track_input() {
    use peel_dsp::SampleBuffer;
    use std::sync::Arc;
    let mut e = engine();
    install(&mut e, 0, Latent::instrument(40));
    let notes = [note(96.0 / 24_000.0, 1.0 / 24_000.0, 1)];
    let mut p = plan(peel_time::TempoMap::constant(120.0), &notes, 1, None, 1.0);
    p.channels[0].gain = -1.0;
    let mut samples = vec![0.0; 256];
    samples[96] = 1.0;
    p.audio.push(peel_engine::AudioRegion {
        track: 0,
        sample: Arc::new(SampleBuffer::new("impulse", SR, vec![samples])),
        start_beat: 0.0,
        end_beat: 1.0,
        source_start_seconds: 0.0,
        gain: 1.0,
        reversed: false,
        fade_in_beats: 0.0,
        fade_out_beats: 0.0,
    });
    e.try_set_plan(Box::new(p)).unwrap();
    e.send(Cmd::Play);
    assert!(render(&mut e, 200).iter().all(|&x| x == 0.0));
}

#[test]
fn compensation_storage_is_bounded_by_the_complete_graph() {
    let mut p = RenderPlan::empty();
    p.tracks = (0..34)
        .map(|ti| TrackRoute::new(if ti == 33 { None } else { Some(33) }, ti))
        .collect();
    p.master = 33;
    p.channels = (0..34).map(|ti| channel(ti, ti as u16)).collect();
    let mut latency = vec![0; 34];
    latency[33] = peel_engine::compensation::MAX_LATENCY;
    let error = peel_engine::compensation::Compensation::prepare(&p, &latency, 256)
        .err()
        .expect("reject excessive aggregate storage");
    assert!(error.contains("64 MiB"), "{error}");
}

#[cfg(all(feature = "rt-check", debug_assertions))]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

#[test]
#[cfg(all(feature = "rt-check", debug_assertions))]
fn compensated_render_and_transport_never_allocate_or_free() {
    use peel_engine::{Engine, Retired, Telemetry};
    use std::sync::Arc;
    let config = EngineConfig {
        device_slots: 16,
        max_block: 13,
        ..EngineConfig::default()
    };
    let (tx, mut rx) = rtrb::RingBuffer::new(64);
    let mut e = Engine::new(SR, &config, Arc::new(Telemetry::new()), tx, None);
    e.handle_command(Cmd::InstallDevice {
        slot: 0,
        device: Box::new(Gate::new()),
    });
    e.handle_command(Cmd::InstallDevice {
        slot: 1,
        device: Box::new(Latent::instrument(40)),
    });
    let mut p = RenderPlan::empty();
    p.channels = vec![channel(0, 0), channel(1, 0)];
    let compensation =
        Box::new(peel_engine::compensation::Compensation::prepare(&p, &[0, 40], 13).unwrap());
    let mut replacement = Some(Cmd::SetCompensatedPlan {
        compensation: Box::new(
            peel_engine::compensation::Compensation::prepare(&p, &[0, 40], 13).unwrap(),
        ),
        plan: Box::new(p.clone()),
    });
    let command = Cmd::SetCompensatedPlan {
        plan: Box::new(p),
        compensation,
    };
    let (mut l, mut r) = ([0.0; 13], [0.0; 13]);
    assert_no_alloc::assert_no_alloc(|| {
        e.handle_command(command);
        e.handle_command(Cmd::LiveNote {
            slot: 0,
            key: 60,
            velocity: 1.0,
            on: true,
        });
        e.handle_command(Cmd::Play);
        for i in 0..100 {
            if i == 10 {
                e.handle_command(replacement.take().unwrap());
            }
            if i == 20 {
                e.handle_command(Cmd::Seek { beat: 0.2 });
            }
            if i == 40 {
                e.handle_command(Cmd::Stop { return_to: 0.0 });
            }
            if i == 60 {
                e.handle_command(Cmd::Panic);
            }
            e.process(&mut l, &mut r);
        }
    });
    let retired: Vec<_> = std::iter::from_fn(|| rx.pop().ok()).collect();
    assert!(
        retired
            .iter()
            .any(|r| matches!(r, Retired::Compensation(_)))
    );
}

#[test]
fn latency_change_during_first_process_does_not_wait_for_an_obsolete_report() {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    struct Changing(u32);
    impl Device for Changing {
        fn descriptor(&self) -> &'static DeviceDescriptor {
            Half.descriptor()
        }
        fn prepare(&mut self, _: f32, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: ParamId, _: f32) {}
        fn param(&self, _: ParamId) -> f32 {
            0.0
        }
        fn latency(&self) -> u32 {
            self.0
        }
        fn process(&mut self, _: &ProcessContext, _: &[DeviceEvent], l: &mut [f32], r: &mut [f32]) {
            self.0 = 64;
            l.fill(0.0);
            r.fill(0.0);
        }
    }
    let mut h = peel_engine::EngineHandle::start_null(EngineConfig {
        device_slots: 16,
        buffer_frames: Some(32),
        ..EngineConfig::default()
    })
    .unwrap();
    assert!(
        h.send(Cmd::InstallDevice {
            slot: 0,
            device: Box::new(Changing(0))
        })
        .is_ok()
    );
    let mut p = RenderPlan::empty();
    p.channels.push(channel(0, 0));
    assert!(h.send(Cmd::SetPlan(Box::new(p))).is_ok());
    let start = Instant::now();
    // Deliberately never observe the original zero-latency install report.
    while h.telemetry().slot_latencies[0].load(Ordering::Relaxed) != 64 {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    while h.compensation_latency_samples() != 64 {
        h.collect_garbage();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn stale_restart_transfers_never_replace_a_new_instance_in_the_same_slot() {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };
    use std::time::{Duration, Instant};
    struct Counting {
        latency: u32,
        prepares: Arc<AtomicU32>,
    }
    impl Device for Counting {
        fn descriptor(&self) -> &'static DeviceDescriptor {
            Half.descriptor()
        }
        fn prepare(&mut self, _: f32, _: usize) {
            self.prepares.fetch_add(1, Ordering::Relaxed);
        }
        fn reset(&mut self) {}
        fn set_param(&mut self, _: ParamId, _: f32) {}
        fn param(&self, _: ParamId) -> f32 {
            0.0
        }
        fn latency(&self) -> u32 {
            self.latency
        }
        fn process(&mut self, _: &ProcessContext, _: &[DeviceEvent], l: &mut [f32], r: &mut [f32]) {
            l.fill(0.0);
            r.fill(0.0);
        }
    }
    let a = Arc::new(AtomicU32::new(0));
    let b = Arc::new(AtomicU32::new(0));
    let mut h = peel_engine::EngineHandle::start_null(EngineConfig {
        device_slots: 16,
        buffer_frames: Some(128),
        ..EngineConfig::default()
    })
    .unwrap();
    assert!(
        h.send(Cmd::InstallDevice {
            slot: 0,
            device: Box::new(Counting {
                latency: 7,
                prepares: a.clone()
            })
        })
        .is_ok()
    );
    let mut p = RenderPlan::empty();
    p.channels.push(channel(0, 0));
    assert!(h.send(Cmd::SetPlan(Box::new(p))).is_ok());
    h.request_device_restart(0).unwrap();
    assert!(h.send(Cmd::RemoveDevice { slot: 0 }).is_ok());
    assert!(
        h.send(Cmd::InstallDevice {
            slot: 0,
            device: Box::new(Counting {
                latency: 11,
                prepares: b.clone()
            })
        })
        .is_ok()
    );
    h.request_device_restart(0).unwrap();
    let start = Instant::now();
    while b.load(Ordering::Relaxed) == 0 || h.compensation_latency_samples() != 11 {
        h.collect_garbage();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        a.load(Ordering::Relaxed),
        0,
        "stale retired instance must be discarded"
    );
    assert_eq!(b.load(Ordering::Relaxed), 1);
}

#[test]
fn restart_transfer_waits_for_retirement_capacity_without_losing_the_device() {
    use peel_engine::{Engine, Retired, Telemetry};
    use std::sync::{Arc, atomic::Ordering};
    let config = EngineConfig {
        device_slots: 16,
        ..EngineConfig::default()
    };
    let telemetry = Arc::new(Telemetry::new());
    let (tx, mut rx) = rtrb::RingBuffer::new(1);
    let mut e = Engine::new(SR, &config, telemetry.clone(), tx, None);
    e.handle_command(Cmd::InstallDevice {
        slot: 0,
        device: Box::new(Latent::instrument(40)),
    });
    // One queued object and every one of the 256 fallback retirement cells.
    for _ in 0..257 {
        e.handle_command(Cmd::SetPlan(Box::new(RenderPlan::empty())));
    }
    e.handle_command(Cmd::SuspendDevice {
        slot: 0,
        token: 123,
    });
    assert_eq!(
        e.slot_latency(0),
        40,
        "instance stays owned and running until space exists"
    );
    let (mut l, mut r) = ([0.0; 16], [0.0; 16]);
    let mut transferred = false;
    for _ in 0..300 {
        if let Ok(Retired::RestartDevice { token, .. }) = rx.pop() {
            assert_eq!(token, 123);
            transferred = true;
        }
        e.process(&mut l, &mut r);
    }
    assert!(transferred);
    assert_eq!(e.slot_latency(0), 0);
    assert_eq!(telemetry.retire_leaks.load(Ordering::Relaxed), 0);
}

#[test]
fn musical_edits_keep_compatible_delay_history_without_a_silence_gap() {
    let mut e = engine();
    install(&mut e, 0, Gate::new());
    install(&mut e, 1, Latent::instrument(40));
    let mut p = RenderPlan::empty();
    p.channels = vec![channel(0, 0), channel(1, 0)];
    e.try_set_plan(Box::new(p.clone())).unwrap();
    e.send(Cmd::LiveNote {
        slot: 0,
        key: 60,
        velocity: 1.0,
        on: true,
    });
    assert_eq!(render(&mut e, 100)[99], 1.0);
    // A new revision with different sequence metadata retains the same paths.
    p.revision = 2;
    p.length_beats = 8.0;
    e.try_set_plan(Box::new(p)).unwrap();
    assert!(render(&mut e, 80).iter().all(|&v| v == 1.0));
}

#[test]
fn changed_channel_identity_never_inherits_stale_audio_from_the_old_source() {
    let mut e = engine();
    install(&mut e, 0, Gate::new());
    install(&mut e, 1, Latent::instrument(40));
    install(&mut e, 2, Gate::new());
    let mut p = RenderPlan::empty();
    p.channels = vec![channel(0, 0), channel(1, 0)];
    e.try_set_plan(Box::new(p.clone())).unwrap();
    impulse(&mut e, &[0]);
    p.channels[0].slot = 2;
    p.revision = 2;
    e.try_set_plan(Box::new(p)).unwrap();
    assert!(render(&mut e, 80).iter().all(|&v| v == 0.0));
}
