//! Sidechain keys reach an insert from an earlier track, taken after its
//! inserts and before its fader.

mod common;

use std::sync::Arc;

use common::*;
use peel_dsp::ParamId;
use peel_dsp::SampleBuffer;
use peel_dsp::device::{Device, DeviceDescriptor, DeviceEvent, DeviceKind, ProcessContext};
use peel_engine::{AudioRegion, EngineCommand, TrackRoute};
use peel_time::TempoMap;

const BEAT: usize = 24_000;

static PROBE: DeviceDescriptor = DeviceDescriptor {
    type_id: "test.key-probe",
    name: "Key probe",
    description: "Test effect: outputs its sidechain key",
    category: "Test",
    kind: DeviceKind::Effect,
    version: 1,
    params: &[],
};

/// Replaces its input with the key, or silence without one.
struct KeyProbe;

impl Device for KeyProbe {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &PROBE
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
        left.fill(0.0);
        right.fill(0.0);
    }
    fn process_keyed(
        &mut self,
        _ctx: &ProcessContext,
        _events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
        key_left: &[f32],
        key_right: &[f32],
    ) {
        left.copy_from_slice(key_left);
        right.copy_from_slice(key_right);
    }
    fn is_silent(&self) -> bool {
        false
    }
}

fn region(track: u16, level: f32) -> AudioRegion {
    AudioRegion {
        track,
        sample: Arc::new(SampleBuffer::new("dc", SR, vec![vec![level; BEAT * 8]])),
        start_beat: 0.0,
        end_beat: 8.0,
        source_start_seconds: 0.0,
        gain: 1.0,
        reversed: false,
        fade_in_beats: 0.0,
        fade_out_beats: 0.0,
    }
}

#[test]
fn an_insert_hears_the_key_track_before_its_fader() {
    let mut p = plan(TempoMap::constant(120.0), &[], 0, None, 16.0);
    // Track 0 (the key, fader at 0.5) and track 1 (keyed) both feed master 2.
    let mut key = TrackRoute::new(Some(2), 0);
    key.gain = 0.5;
    key.key_source = true;
    let mut keyed = TrackRoute::new(Some(2), 1);
    keyed.inserts.push(5);
    keyed.keys.push((5, 0));
    p.tracks = vec![key, keyed, TrackRoute::new(None, 2)];
    p.master = 2;
    p.audio = vec![region(0, 0.4), region(1, 0.1)];
    let mut off = rig(p, None);
    off.send(EngineCommand::InstallDevice {
        slot: 5,
        device: Box::new(KeyProbe),
    });
    off.send(EngineCommand::Play);
    let (left, _) = render_blocks(&mut off, BEAT * 2, 256);
    let v = left[BEAT];
    // Master: 0.2 from the key track's fader, plus the probe's copy of its
    // pre-fader signal, 0.4.
    assert!((v - 0.6).abs() < 1e-4, "{v}");
}

#[test]
fn a_muted_key_track_still_keys() {
    let mut p = plan(TempoMap::constant(120.0), &[], 0, None, 16.0);
    let mut key = TrackRoute::new(Some(2), 0);
    key.muted = true;
    key.key_source = true;
    let mut keyed = TrackRoute::new(Some(2), 1);
    keyed.inserts.push(5);
    keyed.keys.push((5, 0));
    p.tracks = vec![key, keyed, TrackRoute::new(None, 2)];
    p.master = 2;
    p.audio = vec![region(0, 0.4), region(1, 0.1)];
    let mut off = rig(p, None);
    off.send(EngineCommand::InstallDevice {
        slot: 5,
        device: Box::new(KeyProbe),
    });
    off.send(EngineCommand::Play);
    let (left, _) = render_blocks(&mut off, BEAT * 2, 256);
    // The muted track is silent, but the probe still hears it.
    assert!((left[BEAT] - 0.4).abs() < 1e-4, "{}", left[BEAT]);
}

#[test]
fn a_key_from_a_later_track_is_ignored() {
    let mut p = plan(TempoMap::constant(120.0), &[], 0, None, 16.0);
    let mut keyed = TrackRoute::new(Some(2), 0);
    keyed.inserts.push(5);
    keyed.keys.push((5, 1));
    p.tracks = vec![keyed, TrackRoute::new(Some(2), 1), TrackRoute::new(None, 2)];
    p.master = 2;
    p.audio = vec![region(0, 0.1), region(1, 0.3)];
    let mut off = rig(p, None);
    off.send(EngineCommand::InstallDevice {
        slot: 5,
        device: Box::new(KeyProbe),
    });
    off.send(EngineCommand::Play);
    let (left, _) = render_blocks(&mut off, BEAT * 2, 256);
    // The probe runs unkeyed (silence), so only track 1 is heard.
    assert!((left[BEAT] - 0.3).abs() < 1e-4, "{}", left[BEAT]);
}
