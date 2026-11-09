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
