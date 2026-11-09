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
    /// Configured source was deleted or is temporarily unavailable.
    Disabled,
}
impl OutputSource {
    fn encode(self) -> u32 {
        match self {
            Self::Master => MASTER,
            Self::TrackMeter(m) => u32::from(m),
            Self::Disabled => DISABLED,
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
    /// Estimated FIFO delay plus reported output lead (or one callback).
    pub latency_ms: f32,
    /// FIFO delay only; its current fill already includes the prefill.
    pub buffered_latency_ms: f32,
    pub output_latency_ms: f32,
    pub clock_correction_ppm: f32,
}

struct Stats {
    healthy: AtomicBool,
    underruns: AtomicU64,
    overruns: AtomicU64,
    buffered: AtomicU32,
    correction: AtomicU32,
    callback_frames: AtomicU32,
    output_lead_ns: AtomicU64,
}
impl Default for Stats {
    fn default() -> Self {
        Self {
            healthy: AtomicBool::new(true),
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            buffered: AtomicU32::new(0),
            correction: AtomicU32::new(0),
            callback_frames: AtomicU32::new(0),
            output_lead_ns: AtomicU64::new(0),
        }
    }
}

/// Source identity and epoch share one atomic: an overlapping old-source
/// producer can only publish frames tagged with the old epoch.
struct RouteControl {
    source: AtomicU64,
    gain: AtomicU32,
}
impl RouteControl {
    fn new(source: OutputSource, gain_db: f32, muted: bool) -> Self {
        Self {
            source: AtomicU64::new(u64::from(source.encode())),
            gain: AtomicU32::new(
                if muted {
                    0.0_f32
                } else {
                    10.0_f32.powf(gain_db / 20.0)
                }
                .to_bits(),
            ),
        }
    }
    fn set_source(&self, source: Option<OutputSource>) {
        let wanted = source.map_or(DISABLED, OutputSource::encode);
        let old = self.source.load(Ordering::Relaxed);
        if old as u32 != wanted {
            let epoch = ((old >> 32) as u32).wrapping_add(1);
            self.source.store(
                (u64::from(epoch) << 32) | u64::from(wanted),
                Ordering::Release,
            );
        }
    }
    fn levels(&self, gain_db: f32, muted: bool) {
        self.gain.store(
            if muted {
                0.0_f32
            } else {
                10.0_f32.powf(gain_db / 20.0)
            }
            .to_bits(),
            Ordering::Relaxed,
        );
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    epoch: u32,
    samples: [f32; 2],
}
impl From<[f32; 2]> for Frame {
    fn from(samples: [f32; 2]) -> Self {
        Self { epoch: 0, samples }
    }
}
struct Tap {
    control: Arc<RouteControl>,
    producer: Producer<Frame>,
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
            let identity = tap.control.source.load(Ordering::Acquire);
            if identity as u32 != source {
                continue;
            }
            let epoch = (identity >> 32) as u32;
            for (&l, &r) in left.iter().zip(right) {
                let clean = |v: f32| if v.is_finite() { v } else { 0.0 };
                if tap
                    .producer
                    .push(Frame {
                        epoch,
                        samples: [clean(l), clean(r)],
                    })
                    .is_err()
                {
                    tap.stats.overruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    pub(crate) fn silence(&mut self, source: OutputSource, frames: usize) {
        let source = source.encode();
        for tap in &mut self.taps {
            let identity = tap.control.source.load(Ordering::Acquire);
            if identity as u32 != source {
                continue;
            }
            let epoch = (identity >> 32) as u32;
            for _ in 0..frames {
                if tap
                    .producer
                    .push(Frame {
                        epoch,
                        samples: [0.0; 2],
                    })
                    .is_err()
                {
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
    _stream: Option<cpal::Stream>,
    control: Arc<RouteControl>,
    requested_device: String,
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
        Self::validate_requests(requests)?;
        let mut routes = Vec::with_capacity(requests.len());
        let mut taps = Vec::with_capacity(requests.len());
        for request in requests {
            let (tx, rx) = RingBuffer::new(CAPACITY);
            let stats = Arc::new(Stats::default());
            let control = Arc::new(RouteControl::new(
                request.source,
                request.gain_db,
                request.muted,
            ));
            let (stream, name, destination_rate) =
                open(request, rate, rx, stats.clone(), control.clone())?;
            // A prepared stream consumes only silence until the engine installs
            // the taps. Failed preparation therefore leaves all current feeds intact.
            stream
                .play()
                .map_err(|e| format!("Could not start secondary output {name}: {e}"))?;
            routes.push(RouteStream {
                _stream: Some(stream),
                control: control.clone(),
                requested_device: request.device.clone(),
                stats: stats.clone(),
                device: name,
                sample_rate: destination_rate,
                source_rate: rate,
                channels: request.channels,
            });
            taps.push(Tap {
                control,
                producer: tx,
                stats,
            });
        }
        Ok((Self { routes }, Box::new(OutputTaps { taps })))
    }
    pub(crate) fn validate_requests(requests: &[OutputRouteRequest]) -> Result<(), String> {
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
        Ok(())
    }
    /// Levels and source can change without reopening hardware or replacing
    /// the tap set. Only device/channel topology requires new streams.
    pub(crate) fn update_in_place(&self, requests: &[OutputRouteRequest]) -> bool {
        if requests.len() != self.routes.len()
            || !self.routes.iter().zip(requests).all(|(route, request)| {
                route.requested_device == request.device
                    && route.channels == request.channels
                    && route.stats.healthy.load(Ordering::Relaxed)
            })
        {
            return false;
        }
        for (route, request) in self.routes.iter().zip(requests) {
            route.control.levels(request.gain_db, request.muted);
            route.control.set_source(Some(request.source));
        }
        true
    }
    pub(crate) fn set_source(&self, index: usize, source: Option<OutputSource>) {
        if let Some(route) = self.routes.get(index) {
            route.control.set_source(source);
        }
    }
    pub(crate) fn infos(&self) -> Vec<OutputRouteInfo> {
        self.routes
            .iter()
            .map(|r| {
                let buffered = r.stats.buffered.load(Ordering::Relaxed) as usize;
                let buffered_latency_ms = buffered as f32 / r.source_rate.max(1.0) * 1000.0;
                let reported_ns = r.stats.output_lead_ns.load(Ordering::Relaxed);
                let output_latency_ms = if reported_ns > 0 {
                    reported_ns as f32 / 1_000_000.0
                } else {
                    r.stats.callback_frames.load(Ordering::Relaxed) as f32
                        / r.sample_rate.max(1) as f32
                        * 1000.0
                };
                OutputRouteInfo {
                    device: r.device.clone(),
                    sample_rate: r.sample_rate,
                    channels: r.channels,
                    healthy: r.stats.healthy.load(Ordering::Relaxed),
                    underruns: r.stats.underruns.load(Ordering::Relaxed),
                    overruns: r.stats.overruns.load(Ordering::Relaxed),
                    buffered_frames: buffered,
                    latency_ms: buffered_latency_ms + output_latency_ms,
                    buffered_latency_ms,
                    output_latency_ms,
                    clock_correction_ppm: f32::from_bits(
                        r.stats.correction.load(Ordering::Relaxed),
                    ) * 1_000_000.0,
                }
            })
            .collect()
    }
}

struct Reader {
    consumer: Consumer<Frame>,
    stats: Arc<Stats>,
    control: Arc<RouteControl>,
    identity: u64,
    source_per_output: f64,
    correction: f64,
    phase: f64,
    a: [f32; 2],
    b: [f32; 2],
    pending: Option<Frame>,
    ready: bool,
    target: usize,
    gain: f32,
    gain_step: f32,
    fade_samples: u32,
    fade_remaining: u32,
    fade_start: [f32; 2],
    last: [f32; 2],
}
impl Reader {
    fn new(
        consumer: Consumer<Frame>,
        stats: Arc<Stats>,
        control: Arc<RouteControl>,
        source_rate: f32,
        dest_rate: u32,
    ) -> Self {
        let target = (source_rate * 0.025).round() as usize;
        let identity = control.source.load(Ordering::Acquire);
        Self {
            consumer,
            stats,
            control,
            identity,
            source_per_output: f64::from(source_rate) / f64::from(dest_rate.max(1)),
            correction: 0.0,
            phase: 0.0,
            a: [0.0; 2],
            b: [0.0; 2],
            pending: None,
            ready: false,
            target: target.clamp(TARGET / 2, CAPACITY / 4),
            gain: 0.0,
            gain_step: 1.0 / (dest_rate as f32 * 0.01).max(1.0),
            fade_samples: (dest_rate / 200).max(1),
            fade_remaining: 0,
            fade_start: [0.0; 2],
            last: [0.0; 2],
        }
    }
    fn pop_current(&mut self) -> Option<[f32; 2]> {
        let epoch = (self.identity >> 32) as u32;
        if let Some(frame) = self.pending.take()
            && frame.epoch == epoch
        {
            return Some(frame.samples);
        }
        // At most the FIFO's fixed capacity can be discarded, even when a
        // source changes while the primary callback is still writing.
        for _ in 0..CAPACITY {
            let frame = self.consumer.pop().ok()?;
            if frame.epoch == epoch {
                return Some(frame.samples);
            }
        }
        None
    }
    fn discard_old_epoch(&mut self) {
        let epoch = (self.identity >> 32) as u32;
        if self.pending.is_some_and(|frame| frame.epoch == epoch) {
            return;
        }
        self.pending = None;
        for _ in 0..CAPACITY {
            let Ok(frame) = self.consumer.pop() else {
                return;
            };
            if frame.epoch == epoch {
                self.pending = Some(frame);
                return;
            }
        }
    }
    fn begin(&mut self) {
        let identity = self.control.source.load(Ordering::Acquire);
        if identity != self.identity {
            self.identity = identity;
            self.fade_start = self.last;
            self.fade_remaining = self.fade_samples;
            self.ready = false;
            self.phase = 0.0;
            self.a = [0.0; 2];
            self.b = [0.0; 2];
            self.pending = None;
            self.gain = 0.0;
            self.correction = 0.0;
        }
        if !self.ready {
            self.discard_old_epoch();
        }
        let enabled = self.identity as u32 != DISABLED;
        let available = self.consumer.slots() + usize::from(self.pending.is_some());
        // Leave slack at the maximum target too: a full 16k queue must
        // resume at the live edge rather than keep old audio indefinitely.
        if available > (self.target * 4).min(CAPACITY - 1) {
            let discard = available.saturating_sub(self.target * 2);
            self.pending = None;
            for _ in 0..discard {
                let _ = self.consumer.pop();
            }
            self.stats
                .overruns
                .fetch_add(discard as u64, Ordering::Relaxed);
            self.ready = false;
            self.gain = 0.0;
        }
        let available = self.consumer.slots() + usize::from(self.pending.is_some());
        self.stats
            .buffered
            .store(available as u32, Ordering::Relaxed);
        if enabled
            && !self.ready
            && self.fade_remaining == 0
            && available >= self.target
            && let Some(a) = self.pop_current()
            && let Some(b) = self.pop_current()
        {
            self.a = a;
            self.b = b;
            self.phase = 0.0;
            self.ready = true;
        }
        let error = (available as f64 - self.target as f64) / self.target as f64;
        if enabled {
            self.correction = (self.correction + error * 0.000002).clamp(-0.01, 0.01);
        }
        self.stats
            .correction
            .store((self.correction as f32).to_bits(), Ordering::Relaxed);
    }
    fn next(&mut self) -> [f32; 2] {
        if self.fade_remaining > 0 {
            let scale = self.fade_remaining as f32 / self.fade_samples as f32;
            self.fade_remaining -= 1;
            self.last = [self.fade_start[0] * scale, self.fade_start[1] * scale];
            return self.last;
        }
        if !self.ready || self.identity as u32 == DISABLED {
            self.last = [0.0; 2];
            return self.last;
        }
        let mut out = [0.0; 2];
        for (i, v) in out.iter_mut().enumerate() {
            *v = self.a[i] + (self.b[i] - self.a[i]) * self.phase as f32;
        }
        let target = f32::from_bits(self.control.gain.load(Ordering::Relaxed));
        self.gain += (target - self.gain).clamp(-self.gain_step, self.gain_step);
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
            match self.pop_current() {
                Some(frame) => self.b = frame,
                None => {
                    self.ready = false;
                    self.a = [0.0; 2];
                    self.b = [0.0; 2];
                    self.gain = 0.0;
                    if self.control.source.load(Ordering::Acquire) == self.identity {
                        self.stats.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    break;
                }
            }
        }
        self.last = out;
        out
    }
    fn write<T: SizedSample + FromSample<f32>>(
        &mut self,
        out: &mut [T],
        channels: usize,
        map: OutputChannels,
    ) {
        self.stats
            .callback_frames
            .store((out.len() / channels.max(1)) as u32, Ordering::Relaxed);
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
    consumer: Consumer<Frame>,
    stats: Arc<Stats>,
    control: Arc<RouteControl>,
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
    let mut reader = Reader::new(consumer, stats.clone(), control, source_rate, rate);
    let on_error = move |_err: cpal::Error| {
        stats.healthy.store(false, Ordering::Relaxed);
    };
    macro_rules! build {
        ($t:ty) => {
            device.build_output_stream(
                config,
                move |data: &mut [$t], info| {
                    crate::rt::guard(|| {
                        let timing = info.timestamp();
                        reader.stats.output_lead_ns.store(
                            timing
                                .playback
                                .saturating_duration_since(timing.callback)
                                .as_nanos()
                                .min(u128::from(u64::MAX)) as u64,
                            Ordering::Relaxed,
                        );
                        reader.write(data, channels, map)
                    })
                },
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
                control: Arc::new(RouteControl::new(source, 0.0, false)),
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
            assert_eq!(track_rx.pop().unwrap().samples, [0.5, 0.25]);
            assert_eq!(master_rx.pop().unwrap().samples, [0.125, 0.0625]);
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
        assert_eq!(track_rx.pop().unwrap().samples, [0.0; 2]);
        assert_eq!(master_rx.pop().unwrap().samples, [0.0; 2]);
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
                control: Arc::new(RouteControl::new(OutputSource::TrackMeter(7), 0.0, false)),
                producer: tx,
                stats,
            }],
        };
        taps.capture(OutputSource::Master, &[1.0], &[1.0]);
        assert!(rx.pop().is_err());
        taps.capture(OutputSource::TrackMeter(7), &[0.25], &[-0.5]);
        assert_eq!(rx.pop().unwrap().samples, [0.25, -0.5]);
        taps.silence(OutputSource::TrackMeter(7), 2);
        assert_eq!(rx.pop().unwrap().samples, [0.0; 2]);
        assert_eq!(rx.pop().unwrap().samples, [0.0; 2]);
    }
    fn simulated_route() -> (OutputStreams, Producer<Frame>, Reader, Arc<RouteControl>) {
        let (tx, rx) = RingBuffer::new(CAPACITY);
        let stats = Arc::new(Stats::default());
        let control = Arc::new(RouteControl::new(OutputSource::Master, 0.0, false));
        let reader = Reader::new(rx, stats.clone(), control.clone(), 48_000.0, 48_000);
        let streams = OutputStreams {
            routes: vec![RouteStream {
                _stream: None,
                control: control.clone(),
                requested_device: "simulated".into(),
                stats,
                device: "simulated".into(),
                sample_rate: 48_000,
                source_rate: 48_000.0,
                channels: OutputChannels::default(),
            }],
        };
        (streams, tx, reader, control)
    }
    #[test]
    fn live_levels_preserve_stream_and_tap_identity_and_ramp_without_faults() {
        let (streams, mut tx, mut reader, control) = simulated_route();
        let mut out = [0.0_f32; 960];
        for _ in 0..reader.target {
            tx.push([0.25, 0.25].into()).unwrap();
        }
        crate::rt::guard(|| reader.write(&mut out, 2, OutputChannels::default()));
        let identity = control.source.load(Ordering::Relaxed);
        let request = OutputRouteRequest {
            source: OutputSource::Master,
            device: "simulated".into(),
            channels: OutputChannels::default(),
            gain_db: -6.0,
            muted: false,
        };
        assert!(streams.update_in_place(std::slice::from_ref(&request)));
        assert!(Arc::ptr_eq(&streams.routes[0].control, &reader.control));
        assert_eq!(control.source.load(Ordering::Relaxed), identity);
        assert!(reader.ready, "the live reader was not reset");
        for _ in 0..480 {
            tx.push([0.25, 0.25].into()).unwrap();
        }
        crate::rt::guard(|| reader.write(&mut out, 2, OutputChannels::default()));
        assert!((out[958] - 0.25 * 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.002);
        assert!(out.windows(2).all(|pair| (pair[1] - pair[0]).abs() < 0.001));
        let muted = OutputRouteRequest {
            muted: true,
            ..request.clone()
        };
        assert!(streams.update_in_place(&[muted]));
        for _ in 0..480 {
            tx.push([0.25, 0.25].into()).unwrap();
        }
        crate::rt::guard(|| reader.write(&mut out, 2, OutputChannels::default()));
        assert_eq!(out[958], 0.0);
        assert_eq!(control.source.load(Ordering::Relaxed), identity);
        assert_eq!(reader.stats.underruns.load(Ordering::Relaxed), 0);
        let changed = OutputRouteRequest {
            channels: OutputChannels::Stereo(2, 3),
            ..request
        };
        assert!(
            !streams.update_in_place(&[changed]),
            "only topology changes need new hardware"
        );
    }
    #[test]
    fn source_epochs_flush_stale_audio_and_disable_fades_without_an_underrun() {
        let (_streams, mut tx, mut reader, control) = simulated_route();
        let mut out = [0.0_f32; 960];
        for _ in 0..reader.target {
            tx.push([0.5, 0.5].into()).unwrap();
        }
        reader.write(&mut out, 2, OutputChannels::default());
        let last = out[958];
        assert!(last > 0.49);
        control.set_source(None);
        crate::rt::guard(|| reader.write(&mut out, 2, OutputChannels::default()));
        assert_eq!(out[0], last);
        assert!(
            out.as_chunks::<2>()
                .0
                .windows(2)
                .all(|pair| pair[1][0] <= pair[0][0] && (pair[1][0] - pair[0][0]).abs() < 0.003)
        );
        assert_eq!(out[958], 0.0);
        assert_eq!(reader.consumer.slots(), 0, "old FIFO is discarded");
        assert_eq!(reader.stats.underruns.load(Ordering::Relaxed), 0);
        control.set_source(Some(OutputSource::TrackMeter(7)));
        let epoch = (control.source.load(Ordering::Relaxed) >> 32) as u32;
        // Model the previous producer completing an overlapping old block.
        for _ in 0..200 {
            tx.push(Frame {
                epoch: 0,
                samples: [0.9; 2],
            })
            .unwrap();
        }
        for _ in 0..reader.target {
            tx.push(Frame {
                epoch,
                samples: [-0.5; 2],
            })
            .unwrap();
        }
        crate::rt::guard(|| reader.write(&mut out, 2, OutputChannels::default()));
        assert!(
            out.iter().all(|v| *v == 0.0),
            "switch fades/reprimes before the new source"
        );
        crate::rt::guard(|| reader.write(&mut out, 2, OutputChannels::default()));
        assert!(
            out.iter().all(|v| *v <= 0.0),
            "no stale positive old-source samples"
        );
        assert!(out.iter().any(|v| *v < -0.4));
        assert_eq!(reader.stats.underruns.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn route_latency_separates_buffered_audio_from_reported_device_delay() {
        let (streams, _tx, _reader, _control) = simulated_route();
        let stats = &streams.routes[0].stats;
        stats.buffered.store(1200, Ordering::Relaxed);
        stats.callback_frames.store(480, Ordering::Relaxed);
        let info = &streams.infos()[0];
        assert_eq!(info.buffered_latency_ms, 25.0);
        assert_eq!(info.output_latency_ms, 10.0);
        assert_eq!(
            info.latency_ms, 35.0,
            "prefill is already in actual FIFO fill"
        );
        stats.output_lead_ns.store(15_000_000, Ordering::Relaxed);
        assert_eq!(
            streams.infos()[0].latency_ms,
            40.0,
            "reported device lead takes precedence"
        );
    }

    #[test]
    fn clock_mismatch_stays_bounded_and_an_empty_queue_is_silent() {
        let (mut tx, rx) = RingBuffer::new(CAPACITY);
        let stats = Arc::new(Stats::default());
        let mut reader = Reader::new(
            rx,
            stats.clone(),
            Arc::new(RouteControl::new(OutputSource::Master, 0.0, false)),
            48_000.0,
            44_100,
        );
        let mut out = [0.0_f32; 882];
        reader.write(&mut out, 2, OutputChannels::default());
        assert!(out.iter().all(|v| *v == 0.0));
        for _ in 0..reader.target {
            tx.push([0.25, -0.25].into()).unwrap();
        }
        // 48 kHz producer vs a 44.1 kHz consumer running 300 ppm slow.
        let mut frac = 0.0;
        for _ in 0..2000 {
            frac += 480.144;
            let n = frac as usize;
            frac -= n as f64;
            for _ in 0..n {
                tx.push([0.25, -0.25].into()).unwrap();
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
