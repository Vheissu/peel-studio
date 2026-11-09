//! Secondary monitor/cue outputs with bounded clock drift compensation.
//!
//! Each stream owns the consumer of a preallocated SPSC FIFO. The primary
//! engine is its only producer. Resampling and queue correction happen on
//! the secondary callback; neither callback allocates or takes a lock.
//! These independent-clock feeds are not phase-synchronous hardware buses.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use rtrb::{Consumer, Producer, RingBuffer};
use serde::{Deserialize, Serialize};

pub const MAX_OUTPUT_ROUTES: usize = 4;
const CAPACITY: usize = 16_384;
const TARGET: usize = 1024;
const MASTER: u32 = u32::MAX;
const DISABLED: u32 = u32::MAX - 1;

/// Zero-based physical output channels. Unselected channels remain silent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputChannels {
    Mono(u16),
    Stereo(u16, u16),
}

impl Default for OutputChannels {
    fn default() -> Self {
        Self::Stereo(0, 1)
    }
}

impl OutputChannels {
    pub fn validate(self, count: u16) -> Result<(), String> {
        match self {
            Self::Mono(a) if a < count => Ok(()),
            Self::Stereo(a, b) if a < count && b < count && a != b => Ok(()),
            _ => Err(format!(
                "Output channels must be distinct and within 1–{count}"
            )),
        }
    }
    pub(crate) fn encode(self) -> u32 {
        match self {
            Self::Mono(a) => u32::from(a) | (u32::from(a) << 16),
            Self::Stereo(a, b) => u32::from(a) | (u32::from(b) << 16),
        }
    }
    pub(crate) fn decode(value: u32) -> Self {
        let (a, b) = (value as u16, (value >> 16) as u16);
        if a == b {
            Self::Mono(a)
        } else {
            Self::Stereo(a, b)
        }
    }
    fn required(self) -> u16 {
        match self {
            Self::Mono(a) => a.saturating_add(1),
            Self::Stereo(a, b) => a.max(b).saturating_add(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputSource {
    Master,
    TrackMeter(u16),
}
impl OutputSource {
    fn encode(self) -> u32 {
        match self {
            Self::Master => MASTER,
            Self::TrackMeter(m) => u32::from(m),
        }
    }
}

#[derive(Clone, Debug)]
pub struct OutputRouteRequest {
    pub source: OutputSource,
    pub device: String,
    pub channels: OutputChannels,
    pub gain_db: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutputRouteInfo {
    pub device: String,
    pub sample_rate: u32,
    pub channels: OutputChannels,
    pub healthy: bool,
    /// Number of FIFO starvation events after playback has started.
    pub underruns: u64,
    /// Frames dropped because the queue was full or too far behind.
    pub overruns: u64,
    pub buffered_frames: usize,
    pub latency_ms: f32,
    pub clock_correction_ppm: f32,
}

struct Stats {
    healthy: AtomicBool,
    underruns: AtomicU64,
    overruns: AtomicU64,
    buffered: AtomicU32,
    correction: AtomicU32,
}
impl Default for Stats {
    fn default() -> Self {
        Self {
            healthy: AtomicBool::new(true),
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            buffered: AtomicU32::new(0),
            correction: AtomicU32::new(0),
        }
    }
}

struct Tap {
    source: Arc<AtomicU32>,
    producer: Producer<[f32; 2]>,
    stats: Arc<Stats>,
}

/// Control-allocated route set, replaced and retired at an audio block boundary.
#[derive(Default)]
pub struct OutputTaps {
    taps: Vec<Tap>,
}
impl OutputTaps {
    pub(crate) fn capture(&mut self, source: OutputSource, left: &[f32], right: &[f32]) {
        let source = source.encode();
        for tap in &mut self.taps {
            if tap.source.load(Ordering::Relaxed) != source {
                continue;
            }
            for (&l, &r) in left.iter().zip(right) {
                let clean = |v: f32| if v.is_finite() { v } else { 0.0 };
                if tap.producer.push([clean(l), clean(r)]).is_err() {
                    tap.stats.overruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    pub(crate) fn silence(&mut self, source: OutputSource, frames: usize) {
        let source = source.encode();
        for tap in &mut self.taps {
            if tap.source.load(Ordering::Relaxed) != source {
                continue;
            }
            for _ in 0..frames {
                if tap.producer.push([0.0; 2]).is_err() {
                    tap.stats.overruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

/// Owns hardware streams exclusively on the control thread.
#[derive(Default)]
pub(crate) struct OutputStreams {
    routes: Vec<RouteStream>,
}
struct RouteStream {
    _stream: cpal::Stream,
    source: Arc<AtomicU32>,
    stats: Arc<Stats>,
    device: String,
    sample_rate: u32,
    source_rate: f32,
    channels: OutputChannels,
}
impl OutputStreams {
    pub(crate) fn prepare(
        requests: &[OutputRouteRequest],
        rate: f32,
    ) -> Result<(Self, Box<OutputTaps>), String> {
        if requests.len() > MAX_OUTPUT_ROUTES {
            return Err(format!(
                "At most {MAX_OUTPUT_ROUTES} secondary outputs are supported"
            ));
        }
        for r in requests {
            if r.device.trim().is_empty() {
                return Err("Choose a secondary output device".into());
            }
            if !r.gain_db.is_finite() || !(-90.0..=12.0).contains(&r.gain_db) {
                return Err("Output route gain must be between -90 and +12 dB".into());
            }
            r.channels.validate(u16::MAX)?;
        }
        let mut routes = Vec::with_capacity(requests.len());
        let mut taps = Vec::with_capacity(requests.len());
        for request in requests {
            let (tx, rx) = RingBuffer::new(CAPACITY);
            let stats = Arc::new(Stats::default());
            let source = Arc::new(AtomicU32::new(request.source.encode()));
            let (stream, name, destination_rate) = open(request, rate, rx, stats.clone())?;
            // A prepared stream consumes only silence until the engine installs
            // the taps. Failed preparation therefore leaves all current feeds intact.
            stream
                .play()
                .map_err(|e| format!("Could not start secondary output {name}: {e}"))?;
            routes.push(RouteStream {
                _stream: stream,
                source: source.clone(),
                stats: stats.clone(),
                device: name,
                sample_rate: destination_rate,
                source_rate: rate,
                channels: request.channels,
            });
            taps.push(Tap {
                source,
                producer: tx,
                stats,
            });
        }
        Ok((Self { routes }, Box::new(OutputTaps { taps })))
    }
    pub(crate) fn set_source(&self, index: usize, source: Option<OutputSource>) {
        if let Some(route) = self.routes.get(index) {
            route.source.store(
                source.map_or(DISABLED, OutputSource::encode),
                Ordering::Relaxed,
            );
        }
    }
    pub(crate) fn infos(&self) -> Vec<OutputRouteInfo> {
        self.routes
            .iter()
            .map(|r| {
                let buffered = r.stats.buffered.load(Ordering::Relaxed) as usize;
                OutputRouteInfo {
                    device: r.device.clone(),
                    sample_rate: r.sample_rate,
                    channels: r.channels,
                    healthy: r.stats.healthy.load(Ordering::Relaxed),
                    underruns: r.stats.underruns.load(Ordering::Relaxed),
                    overruns: r.stats.overruns.load(Ordering::Relaxed),
                    buffered_frames: buffered,
                    latency_ms: buffered as f32 / r.source_rate.max(1.0) * 1000.0,
                    clock_correction_ppm: f32::from_bits(
                        r.stats.correction.load(Ordering::Relaxed),
                    ) * 1_000_000.0,
                }
            })
            .collect()
    }
}

struct Reader {
    consumer: Consumer<[f32; 2]>,
    stats: Arc<Stats>,
    source_per_output: f64,
    correction: f64,
    phase: f64,
    a: [f32; 2],
    b: [f32; 2],
    ready: bool,
    target: usize,
    gain: f32,
    target_gain: f32,
    gain_step: f32,
}
impl Reader {
    fn new(
        consumer: Consumer<[f32; 2]>,
        stats: Arc<Stats>,
        source_rate: f32,
        dest_rate: u32,
        gain_db: f32,
        muted: bool,
    ) -> Self {
        let target = (source_rate * 0.025).round() as usize;
        Self {
            consumer,
            stats,
            source_per_output: f64::from(source_rate) / f64::from(dest_rate.max(1)),
            correction: 0.0,
            phase: 0.0,
            a: [0.0; 2],
            b: [0.0; 2],
            ready: false,
            target: target.clamp(TARGET / 2, CAPACITY / 4),
            gain: 0.0,
            target_gain: if muted {
                0.0
            } else {
                10.0_f32.powf(gain_db / 20.0)
            },
            gain_step: 1.0 / (dest_rate as f32 * 0.01).max(1.0),
        }
    }
    fn begin(&mut self) {
        let available = self.consumer.slots();
        // A suspended callback must resume near the live edge rather than
        // playing an old cue half a second late. The work remains bounded.
        if available > self.target * 4 {
            let discard = available.saturating_sub(self.target * 2);
            for _ in 0..discard {
                let _ = self.consumer.pop();
            }
            self.stats
                .overruns
                .fetch_add(discard as u64, Ordering::Relaxed);
            self.ready = false;
        }
        let available = self.consumer.slots();
        self.stats
            .buffered
            .store(available as u32, Ordering::Relaxed);
        if !self.ready && available >= self.target {
            self.a = self.consumer.pop().unwrap_or([0.0; 2]);
            self.b = self.consumer.pop().unwrap_or([0.0; 2]);
            self.phase = 0.0;
            self.ready = true;
        }
        // PI-like slow integral correction follows clock error, with a small
        // proportional term restoring the target fill. Bound to +/-1%.
        let error = (available as f64 - self.target as f64) / self.target as f64;
        self.correction = (self.correction + error * 0.000002).clamp(-0.01, 0.01);
        self.stats
            .correction
            .store((self.correction as f32).to_bits(), Ordering::Relaxed);
    }
    fn next(&mut self) -> [f32; 2] {
        if !self.ready {
            return [0.0; 2];
        }
        let mut out = [0.0; 2];
        for (i, v) in out.iter_mut().enumerate() {
            *v = self.a[i] + (self.b[i] - self.a[i]) * self.phase as f32;
        }
        let delta = (self.target_gain - self.gain).clamp(-self.gain_step, self.gain_step);
        self.gain += delta;
        for v in &mut out {
            *v *= self.gain;
        }
        let available = self.consumer.slots();
        let error = (available as f64 - self.target as f64) / self.target as f64;
        self.phase +=
            self.source_per_output * (1.0 + self.correction + error.clamp(-1.0, 1.0) * 0.002);
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.a = self.b;
            match self.consumer.pop() {
                Ok(frame) => self.b = frame,
                Err(_) => {
                    self.ready = false;
                    self.a = [0.0; 2];
                    self.b = [0.0; 2];
                    self.gain = 0.0;
                    self.stats.underruns.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
        }
        out
    }
    fn write<T: SizedSample + FromSample<f32>>(
        &mut self,
        out: &mut [T],
        channels: usize,
        map: OutputChannels,
    ) {
        self.begin();
        for frame in out.chunks_exact_mut(channels.max(1)) {
            let sample = self.next();
            write_frame(frame, map, sample, !T::FORMAT.is_float());
        }
    }
}

pub(crate) fn write_frame<T: Sample + FromSample<f32>>(
    frame: &mut [T],
    map: OutputChannels,
    sample: [f32; 2],
    clamp: bool,
) {
    frame.fill(T::EQUILIBRIUM);
    let conv = |v: f32| T::from_sample(if clamp { v.clamp(-1.0, 1.0) } else { v });
    match map {
        OutputChannels::Mono(a) => {
            if let Some(v) = frame.get_mut(a as usize) {
                *v = conv((sample[0] + sample[1]) * 0.5);
            }
        }
        OutputChannels::Stereo(a, b) => {
            if let Some(v) = frame.get_mut(a as usize) {
                *v = conv(sample[0]);
            }
            if let Some(v) = frame.get_mut(b as usize) {
                *v = conv(sample[1]);
            }
        }
    }
}

fn open(
    request: &OutputRouteRequest,
    source_rate: f32,
    consumer: Consumer<[f32; 2]>,
    stats: Arc<Stats>,
) -> Result<(cpal::Stream, String, u32), String> {
    let host = cpal::default_host();
    let device = crate::handle::find_output_device(&host, &request.device).ok_or_else(|| {
        format!(
            "Secondary output '{}' was not found. Reconnect it or choose another device.",
            request.device
        )
    })?;
    let name = crate::handle::device_name(&device);
    let wanted = source_rate.round() as u32;
    let mut configs: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| format!("Cannot query {name}: {e}"))?
        .filter(|c| {
            c.channels() >= request.channels.required()
                && crate::handle::format_rank(c.sample_format()).is_some()
        })
        .collect();
    configs.sort_by_key(|c| {
        (
            u8::from(!(c.min_sample_rate()..=c.max_sample_rate()).contains(&wanted)),
            c.channels(),
            crate::handle::format_rank(c.sample_format()).unwrap_or(255),
        )
    });
    let config = configs
        .first()
        .ok_or_else(|| format!("{name} does not support the chosen output channels"))?;
    let rate = wanted.clamp(config.min_sample_rate(), config.max_sample_rate());
    let format = config.sample_format();
    let config = config.with_sample_rate(rate).config();
    request.channels.validate(config.channels)?;
    let channels = config.channels as usize;
    let map = request.channels;
    let mut reader = Reader::new(
        consumer,
        stats.clone(),
        source_rate,
        rate,
        request.gain_db,
        request.muted,
    );
    let on_error = move |_err: cpal::Error| {
        stats.healthy.store(false, Ordering::Relaxed);
    };
    macro_rules! build {
        ($t:ty) => {
            device.build_output_stream(
                config,
                move |data: &mut [$t], _| crate::rt::guard(|| reader.write(data, channels, map)),
                on_error,
                None,
            )
        };
    }
    let stream = match format {
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U16 => build!(u16),
        SampleFormat::I24 => build!(cpal::I24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U24 => build!(cpal::U24),
        SampleFormat::I8 => build!(i8),
        SampleFormat::U8 => build!(u8),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U64 => build!(u64),
        _ => return Err(format!("{name} uses an unsupported sample format")),
    }
    .map_err(|e| format!("Could not open secondary output {name}: {e}"))?;
    Ok((stream, name, rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "rt-check")]
    #[global_allocator]
    static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

    #[test]
    fn engine_routes_post_fader_tracks_separately_from_the_master() {
        use crate::{
            ChannelRoute, Engine, EngineCommand, EngineConfig, RenderPlan, Telemetry, TrackRoute,
        };
        use peel_dsp::{
            Device, DeviceDescriptor, DeviceEvent, DeviceKind, ParamId, ProcessContext,
        };
        static DESC: DeviceDescriptor = DeviceDescriptor {
            type_id: "test.constant",
            name: "Constant",
            description: "test",
            category: "Test",
            kind: DeviceKind::Instrument,
            version: 1,
            params: &[],
        };
        struct Constant;
        impl Device for Constant {
            fn descriptor(&self) -> &'static DeviceDescriptor {
                &DESC
            }
            fn prepare(&mut self, _: f32, _: usize) {}
            fn reset(&mut self) {}
            fn set_param(&mut self, _: ParamId, _: f32) {}
            fn param(&self, _: ParamId) -> f32 {
                0.0
            }
            fn process(
                &mut self,
                _: &ProcessContext,
                _: &[DeviceEvent],
                l: &mut [f32],
                r: &mut [f32],
            ) {
                l.fill(1.0);
                r.fill(0.5);
            }
        }
        let config = EngineConfig {
            max_block: 256,
            ..EngineConfig::default()
        };
        let (retire, _retired) = RingBuffer::new(64);
        let mut engine = Engine::new(48_000.0, &config, Arc::new(Telemetry::new()), retire, None);
        let mut plan = RenderPlan::empty();
        plan.tracks = vec![TrackRoute::new(Some(1), 7), TrackRoute::new(None, 9)];
        plan.tracks[0].gain = 0.5;
        plan.tracks[1].gain = 0.25;
        plan.master = 1;
        plan.channels = vec![ChannelRoute {
            slot: 0,
            track: 0,
            gain: 1.0,
            pan: 0.0,
            muted: false,
        }];
        engine.handle_command(EngineCommand::SetPlan(Box::new(plan)));
        engine.handle_command(EngineCommand::InstallDevice {
            slot: 0,
            device: Box::new(Constant),
        });
        let (track_tx, mut track_rx) = RingBuffer::new(1024);
        let (master_tx, mut master_rx) = RingBuffer::new(1024);
        let mut taps = vec![];
        for (source, producer) in [
            (OutputSource::TrackMeter(7), track_tx),
            (OutputSource::Master, master_tx),
        ] {
            taps.push(Tap {
                source: Arc::new(AtomicU32::new(source.encode())),
                producer,
                stats: Arc::new(Stats::default()),
            });
        }
        engine.handle_command(EngineCommand::SetOutputRoutes(Box::new(OutputTaps {
            taps,
        })));
        let (mut l, mut r) = ([0.0; 256], [0.0; 256]);
        crate::rt::guard(|| engine.process(&mut l, &mut r));
        for _ in 0..256 {
            assert_eq!(track_rx.pop().unwrap(), [0.5, 0.25]);
            assert_eq!(master_rx.pop().unwrap(), [0.125, 0.0625]);
        }
        assert_eq!(l, [0.125; 256]);
        engine.handle_command(EngineCommand::SetTrackMute {
            track: 0,
            muted: true,
        });
        // Once ramps settle, muted track feeds remain real silence in the FIFO.
        for _ in 0..4 {
            crate::rt::guard(|| engine.process(&mut l, &mut r));
            for _ in 0..256 {
                let _ = track_rx.pop();
                let _ = master_rx.pop();
            }
        }
        crate::rt::guard(|| engine.process(&mut l, &mut r));
        assert_eq!(track_rx.pop().unwrap(), [0.0; 2]);
        assert_eq!(master_rx.pop().unwrap(), [0.0; 2]);
    }

    #[test]
    fn channel_maps_select_only_the_requested_physical_outputs() {
        let mut out = [9.0_f32; 8];
        write_frame(&mut out, OutputChannels::Stereo(4, 7), [0.5, -0.25], false);
        assert_eq!(out, [0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, -0.25]);
        write_frame(&mut out, OutputChannels::Mono(2), [0.5, -0.25], false);
        assert_eq!(out[2], 0.125);
        assert!(OutputChannels::Stereo(1, 1).validate(8).is_err());
        assert!(OutputChannels::Stereo(1, 8).validate(8).is_err());
        assert!(OutputChannels::Stereo(2, 3).validate(8).is_ok());
    }
    #[test]
    fn route_sources_are_separate_and_input_silence_is_explicit() {
        let (tx, mut rx) = RingBuffer::new(16);
        let stats = Arc::new(Stats::default());
        let mut taps = OutputTaps {
            taps: vec![Tap {
                source: Arc::new(AtomicU32::new(OutputSource::TrackMeter(7).encode())),
                producer: tx,
                stats,
            }],
        };
        taps.capture(OutputSource::Master, &[1.0], &[1.0]);
        assert!(rx.pop().is_err());
        taps.capture(OutputSource::TrackMeter(7), &[0.25], &[-0.5]);
        assert_eq!(rx.pop().unwrap(), [0.25, -0.5]);
        taps.silence(OutputSource::TrackMeter(7), 2);
        assert_eq!(rx.pop().unwrap(), [0.0; 2]);
        assert_eq!(rx.pop().unwrap(), [0.0; 2]);
    }
    #[test]
    fn clock_mismatch_stays_bounded_and_an_empty_queue_is_silent() {
        let (mut tx, rx) = RingBuffer::new(CAPACITY);
        let stats = Arc::new(Stats::default());
        let mut reader = Reader::new(rx, stats.clone(), 48_000.0, 44_100, 0.0, false);
        let mut out = [0.0_f32; 882];
        reader.write(&mut out, 2, OutputChannels::default());
        assert!(out.iter().all(|v| *v == 0.0));
        for _ in 0..reader.target {
            tx.push([0.25, -0.25]).unwrap();
        }
        // 48 kHz producer vs a 44.1 kHz consumer running 300 ppm slow.
        let mut frac = 0.0;
        for _ in 0..2000 {
            frac += 480.144;
            let n = frac as usize;
            frac -= n as f64;
            for _ in 0..n {
                tx.push([0.25, -0.25]).unwrap();
            }
            crate::rt::guard(|| reader.write(&mut out, 2, OutputChannels::default()));
            assert!(out.iter().all(|v| v.is_finite()));
            assert!(reader.consumer.slots() < reader.target * 3);
        }
        assert_eq!(stats.underruns.load(Ordering::Relaxed), 0);
        assert_eq!(stats.overruns.load(Ordering::Relaxed), 0);
        assert!(out.iter().any(|v| *v > 0.2));
        for _ in 0..30 {
            reader.write(&mut out, 2, OutputChannels::default());
        }
        assert!(out.iter().all(|v| *v == 0.0));
    }
}
