//! Control-side handle: opens the audio backend, owns the queues and the
//! stream, and exposes telemetry.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, ErrorKind, FromSample, Sample, SampleFormat, SizedSample, StreamConfig,
    SupportedBufferSize,
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::EngineConfig;
use crate::command::{EngineCommand, Retired};
use crate::engine::Engine;
use crate::live::{
    LiveInput, LiveShared, LiveStats, LiveTarget, LiveTiming, RecordedNote, RecordedReceiver,
};
use crate::telemetry::Telemetry;

/// Name reported when no audio device could be used.
pub const NO_DEVICE_NAME: &str = "No audio device";
const NULL_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The audio backend could not be opened and the fallback also failed.
    #[error("audio backend error: {0}")]
    Backend(String),
    /// The fallback clock thread could not be started.
    #[error("could not start the audio thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// The control-side end of a running engine.
///
/// Create it with [`EngineHandle::start`] and keep it for the life of the
/// application. It owns the audio stream: dropping it stops audio and frees
/// the engine and everything still inside it.
///
/// **Thread affinity:** the cpal `Stream` may be `!Send` on some platforms,
/// so the handle should be created, used and dropped on the main thread. All
/// of its methods take `&self` or `&mut self` and never block.
pub struct EngineHandle {
    // Declared first so the stream (and with it the engine) is dropped before
    // the queue ends that the control thread keeps.
    backend: Backend,
    commands: Producer<EngineCommand>,
    retired: Consumer<Retired>,
    telemetry: Arc<Telemetry>,
    healthy: Arc<AtomicBool>,
    sample_rate: f32,
    max_block: usize,
    buffer_frames: u32,
    device_name: String,
    /// Queue slots kept free for commands that must not be lost.
    reserve: usize,
    live: LiveSide,
    /// Why the requested device was not used, if it was not.
    warning: Option<String>,
}

// The stream or thread variants are held only to keep audio alive until drop.
#[allow(dead_code)]
enum Backend {
    Cpal(cpal::Stream),
    Null(NullBackend),
    /// Audio was shut down with [`EngineHandle::stop_audio`].
    Stopped,
}

/// The control-side ends of the live input path of one engine.
struct LiveSide {
    shared: Arc<LiveShared>,
    input: Option<LiveInput>,
    recorded: Option<RecordedReceiver>,
}

impl EngineHandle {
    /// Opens the default output device and starts the engine on its
    /// callback. If no device can be opened, falls back to a silent
    /// real-time clock ([`start_null`](EngineHandle::start_null)) so the
    /// application keeps running; [`device_name`](EngineHandle::device_name)
    /// then reports "No audio device".
    ///
    /// `config.device` selects the output device by the name or id that
    /// [`list_output_devices`] reports. A device that is missing or cannot be
    /// opened falls back to the system default with a logged warning;
    /// [`device_warning`](EngineHandle::device_warning) then says why.
    pub fn start(config: EngineConfig) -> Result<EngineHandle, EngineError> {
        let telemetry = Arc::new(Telemetry::new());
        let healthy = Arc::new(AtomicBool::new(true));
        match open_cpal(&config, &telemetry, &healthy) {
            Ok(opened) => {
                log::info!(
                    "audio output: {} ({} Hz, buffer {} frames)",
                    opened.name,
                    opened.sample_rate,
                    opened.buffer_frames
                );
                Ok(EngineHandle {
                    backend: Backend::Cpal(opened.stream),
                    commands: opened.commands,
                    retired: opened.retired,
                    telemetry,
                    healthy,
                    sample_rate: opened.sample_rate,
                    max_block: config.max_block.clamp(1, 16_384),
                    buffer_frames: opened.buffer_frames,
                    device_name: opened.name,
                    reserve: queue_reserve(&config),
                    live: opened.live,
                    warning: opened.warning,
                })
            }
            Err(reason) => {
                log::warn!("no usable audio device ({reason}); running without audio output");
                let warning = match config.device.as_deref() {
                    Some(want) => format!("Could not open audio device \"{want}\": {reason}"),
                    None => format!("No usable audio device: {reason}"),
                };
                let mut handle = Self::start_null_with(config, telemetry, healthy)?;
                handle.warning = Some(warning);
                Ok(handle)
            }
        }
    }

    /// Starts the engine on a silent, sleep-clocked thread. Nothing is
    /// audible, but the transport, sequencer, meters and export all behave as
    /// they would with a device. This is a development and fallback backend:
    /// unlike a device callback its clock is the OS timer, which is why it
    /// must never be used for anything audible.
    pub fn start_null(config: EngineConfig) -> Result<EngineHandle, EngineError> {
        Self::start_null_with(
            config,
            Arc::new(Telemetry::new()),
            Arc::new(AtomicBool::new(true)),
        )
    }

    fn start_null_with(
        config: EngineConfig,
        telemetry: Arc<Telemetry>,
        healthy: Arc<AtomicBool>,
    ) -> Result<EngineHandle, EngineError> {
        let sample_rate = config.sample_rate.unwrap_or(NULL_SAMPLE_RATE) as f32;
        let buffer_frames = config.buffer_frames.unwrap_or(256).max(1);
        let (engine, commands, retired, live) = make_parts(sample_rate, &config, telemetry.clone());
        let max_block = engine.max_block();
        let sample_rate = engine.sample_rate();
        let null = NullBackend::spawn(engine, sample_rate, buffer_frames as usize)?;
        Ok(EngineHandle {
            backend: Backend::Null(null),
            commands,
            retired,
            telemetry,
            healthy,
            sample_rate,
            max_block,
            buffer_frames,
            device_name: NO_DEVICE_NAME.to_string(),
            reserve: queue_reserve(&config),
            live,
            warning: None,
        })
    }

    /// Queues a command for the next audio block.
    ///
    /// Returns the command back if the queue is full. A small reserve of the
    /// queue is kept for stop, pause, panic and note-off commands, so a flood
    /// of parameter changes can never prevent silencing the engine.
    pub fn send(&mut self, cmd: EngineCommand) -> Result<(), EngineCommand> {
        if !is_critical(&cmd) && self.commands.slots() <= self.reserve {
            return Err(cmd);
        }
        self.commands.push(cmd).map_err(|PushError::Full(cmd)| cmd)
    }

    /// Drops everything the engine has retired (old plans, devices and
    /// samples). Call this regularly from the UI thread; it never blocks.
    /// Returns how many objects were freed.
    pub fn collect_garbage(&mut self) -> usize {
        let mut n = 0;
        while let Ok(item) = self.retired.pop() {
            drop(item);
            n += 1;
        }
        n
    }

    pub fn telemetry(&self) -> &Arc<Telemetry> {
        &self.telemetry
    }

    /// The negotiated stream sample rate. Devices must be prepared at this
    /// rate and at [`max_block`](EngineHandle::max_block).
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Name of the output device, or "No audio device" on the null backend.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Frames per device callback as negotiated (best effort; the callback
    /// may deliver other sizes).
    pub fn buffer_frames(&self) -> u32 {
        self.buffer_frames
    }

    /// False after the audio stream reported that the device disappeared or
    /// the stream must be rebuilt. The UI should offer to restart audio.
    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// True when running on the silent fallback clock.
    pub fn is_null_backend(&self) -> bool {
        matches!(self.backend, Backend::Null(_))
    }

    /// Why the audio device that was asked for is not the one in use: it
    /// was not found, could not be opened, or no device works at all.
    pub fn device_warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// Stops the audio stream and frees the device, the engine and every
    /// plan, device and sample inside it. Commands sent afterwards go
    /// nowhere. Used to release a device before opening another engine on it.
    pub fn stop_audio(&mut self) {
        self.backend = Backend::Stopped;
        self.healthy.store(false, Ordering::Relaxed);
    }

    // ---- live input ----

    /// Takes the producer end of the live input queue (once; later calls
    /// return `None`). It is `Clone + Send`: move it to the MIDI thread(s).
    /// Notes sent through it reach the instrument chosen by
    /// [`live_target`](EngineHandle::live_target) without touching this
    /// handle or the command queue.
    pub fn take_live_input(&mut self) -> Option<LiveInput> {
        self.live.input.take()
    }

    /// The instrument slot live notes play. Set it when the selected channel
    /// changes; `None` ignores live notes.
    pub fn live_target(&self) -> LiveTarget {
        self.live.shared.target().clone()
    }

    /// Counters for an input activity light (`note_ons` rises with each key).
    pub fn live_stats(&self) -> &LiveStats {
        self.live.shared.stats()
    }

    /// Chooses whether live events start at the next block boundary
    /// ([`LiveTiming::Immediate`], the default) or keep their arrival
    /// spacing ([`LiveTiming::Spread`]).
    pub fn set_live_timing(&self, timing: LiveTiming) {
        self.live.shared.set_timing(timing);
    }

    pub fn live_timing(&self) -> LiveTiming {
        self.live.shared.timing()
    }

    /// Moves every note recorded since the last call into `out` (oldest
    /// first) and returns how many.
    pub fn drain_recorded(&mut self, out: &mut Vec<RecordedNote>) -> usize {
        self.live.recorded.as_mut().map_or(0, |r| r.drain_into(out))
    }
}

fn queue_reserve(config: &EngineConfig) -> usize {
    (config.command_capacity / 16).clamp(8, 256)
}

/// Commands that must get through even when the queue is nearly full.
fn is_critical(cmd: &EngineCommand) -> bool {
    matches!(
        cmd,
        EngineCommand::Stop { .. }
            | EngineCommand::Pause
            | EngineCommand::Panic
            | EngineCommand::LiveNote { on: false, .. }
    )
}

fn make_parts(
    sample_rate: f32,
    config: &EngineConfig,
    telemetry: Arc<Telemetry>,
) -> (Engine, Producer<EngineCommand>, Consumer<Retired>, LiveSide) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(config.command_capacity.max(64));
    let (ret_tx, ret_rx) = RingBuffer::new(config.retire_capacity.max(64));
    let mut engine = Engine::new(sample_rate, config, telemetry, ret_tx, Some(cmd_rx));
    let live = LiveSide {
        shared: engine.live_shared().clone(),
        input: engine.take_live_input(),
        recorded: engine.take_recorded(),
    };
    (engine, cmd_tx, ret_rx, live)
}

// ----------------------------------------------------------------------
// Null backend
// ----------------------------------------------------------------------

struct NullBackend {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl NullBackend {
    fn spawn(
        mut engine: Engine,
        sample_rate: f32,
        block: usize,
    ) -> Result<NullBackend, std::io::Error> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("peel-null-audio".into())
            .spawn(move || {
                let mut left = vec![0.0f32; block];
                let mut right = vec![0.0f32; block];
                let period = Duration::from_secs_f64(block as f64 / sample_rate as f64);
                let mut deadline = Instant::now() + period;
                while !flag.load(Ordering::Relaxed) {
                    engine.begin_callback_frames(block);
                    engine.render(&mut left, &mut right);
                    engine.end_callback(block);
                    let now = Instant::now();
                    if deadline > now {
                        std::thread::sleep(deadline - now);
                    } else if now - deadline > period * 8 {
                        // Fell far behind (suspend, debugger): resynchronize
                        // instead of rendering a burst.
                        deadline = now;
                    }
                    deadline += period;
                }
            })?;
        Ok(NullBackend {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for NullBackend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ----------------------------------------------------------------------
// cpal backend
// ----------------------------------------------------------------------

struct Opened {
    stream: cpal::Stream,
    commands: Producer<EngineCommand>,
    retired: Consumer<Retired>,
    live: LiveSide,
    name: String,
    sample_rate: f32,
    buffer_frames: u32,
    warning: Option<String>,
}

fn format_rank(f: SampleFormat) -> Option<u8> {
    Some(match f {
        SampleFormat::F32 => 0,
        SampleFormat::I16 => 1,
        SampleFormat::I32 => 2,
        SampleFormat::F64 => 3,
        SampleFormat::U16 => 4,
        SampleFormat::I24 => 5,
        SampleFormat::U32 => 6,
        SampleFormat::U24 => 7,
        SampleFormat::I8 => 8,
        SampleFormat::U8 => 9,
        SampleFormat::I64 => 10,
        SampleFormat::U64 => 11,
        _ => return None,
    })
}

struct Candidate {
    channels: u16,
    rate: u32,
    format: SampleFormat,
    buffer: SupportedBufferSize,
}

// ----------------------------------------------------------------------
// Device enumeration
// ----------------------------------------------------------------------

/// Sample rates worth offering in a picker, when a device supports them.
const COMMON_RATES: [u32; 7] = [22_050, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// An audio output device the engine can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDeviceInfo {
    /// Human-readable name. Not guaranteed unique (two identical interfaces
    /// share one); use [`id`](OutputDeviceInfo::id) to tell them apart.
    pub name: String,
    /// Backend identifier, stable across runs where the platform allows it.
    /// Either this or the name is accepted by [`EngineConfig::device`].
    pub id: String,
    /// The system default output.
    pub is_default: bool,
    /// Most channels any supported format offers.
    pub max_channels: u16,
    /// Supported sample-rate ranges in Hz, merged and sorted.
    pub sample_rate_ranges: Vec<(u32, u32)>,
    /// Common rates inside the ranges (22.05 kHz up to 192 kHz).
    pub sample_rates: Vec<u32>,
    /// The device's preferred rate, if it reports one.
    pub default_sample_rate: Option<u32>,
    /// Smallest and largest buffer in frames, if the platform says. Many
    /// backends report nothing; any size may still work.
    pub buffer_frames: Option<(u32, u32)>,
}

/// Lists the output devices of the default audio host.
///
/// Enumeration queries every device and can take tens of milliseconds, so
/// call it when a settings panel opens, not every frame. Returns an empty
/// list when the host has no audio devices.
pub fn list_output_devices() -> Vec<OutputDeviceInfo> {
    let host = cpal::default_host();
    let default_id = host.default_output_device().and_then(|d| d.id().ok());
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };
    devices
        .filter_map(|device| describe_device(&device, default_id.as_ref()))
        .collect()
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| device.to_string())
}

fn describe_device(
    device: &cpal::Device,
    default_id: Option<&cpal::DeviceId>,
) -> Option<OutputDeviceInfo> {
    let id = device.id().ok();
    let ranges: Vec<_> = device.supported_output_configs().ok()?.collect();
    if ranges.is_empty() {
        return None;
    }
    let merged = merge_ranges(
        ranges
            .iter()
            .map(|r| (r.min_sample_rate(), r.max_sample_rate()))
            .collect(),
    );
    let sample_rates = common_rates(&merged);
    let buffer = union_buffer_sizes(ranges.iter().filter_map(|r| match *r.buffer_size() {
        SupportedBufferSize::Range { min, max } => Some((min, max)),
        SupportedBufferSize::Unknown => None,
    }));
    Some(OutputDeviceInfo {
        name: device_name(device),
        is_default: id.is_some() && id.as_ref() == default_id,
        id: id.map(|i| i.to_string()).unwrap_or_default(),
        max_channels: ranges.iter().map(|r| r.channels()).max().unwrap_or(0),
        sample_rate_ranges: merged,
        sample_rates,
        default_sample_rate: device.default_output_config().ok().map(|c| c.sample_rate()),
        buffer_frames: buffer,
    })
}

/// Sorts sample-rate ranges and joins the ones that overlap.
fn merge_ranges(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (lo, hi) in ranges {
        match merged.last_mut() {
            Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    merged
}

fn common_rates(ranges: &[(u32, u32)]) -> Vec<u32> {
    COMMON_RATES
        .into_iter()
        .filter(|r| ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(r)))
        .collect()
}

/// Smallest minimum and largest maximum of the buffer ranges, if any.
fn union_buffer_sizes(sizes: impl Iterator<Item = (u32, u32)>) -> Option<(u32, u32)> {
    sizes.fold(None, |acc, (min, max)| {
        Some(acc.map_or((min, max), |(lo, hi)| (lo.min(min), hi.max(max))))
    })
}

/// Index of the device `wanted` names among `(id, name)` pairs: an id match
/// first, then an exact name, then a name ignoring case.
fn match_device(devices: &[(String, String)], wanted: &str) -> Option<usize> {
    devices
        .iter()
        .position(|(id, _)| id == wanted)
        .or_else(|| devices.iter().position(|(_, name)| name == wanted))
        .or_else(|| {
            devices
                .iter()
                .position(|(_, name)| name.eq_ignore_ascii_case(wanted))
        })
}

fn find_output_device(host: &cpal::Host, wanted: &str) -> Option<cpal::Device> {
    let devices: Vec<cpal::Device> = host.output_devices().ok()?.collect();
    let keys: Vec<(String, String)> = devices
        .iter()
        .map(|d| {
            let id = d.id().map(|i| i.to_string()).unwrap_or_default();
            (id, device_name(d))
        })
        .collect();
    let pos = match_device(&keys, wanted)?;
    devices.into_iter().nth(pos)
}

// ----------------------------------------------------------------------
// Opening a device
// ----------------------------------------------------------------------

fn open_cpal(
    config: &EngineConfig,
    telemetry: &Arc<Telemetry>,
    healthy: &Arc<AtomicBool>,
) -> Result<Opened, String> {
    let host = cpal::default_host();
    let mut warning = None;
    if let Some(wanted) = config.device.as_deref().filter(|d| !d.is_empty()) {
        match find_output_device(&host, wanted) {
            Some(device) => match open_device(device, config, telemetry, healthy) {
                Ok(opened) => return Ok(opened),
                Err(reason) => {
                    log::warn!(
                        "audio device \"{wanted}\" could not be opened ({reason}); using the default"
                    );
                    warning = Some(format!(
                        "Could not open audio device \"{wanted}\" ({reason}); using the default device"
                    ));
                }
            },
            None => {
                log::warn!("audio device \"{wanted}\" not found; using the default");
                warning = Some(format!(
                    "Audio device \"{wanted}\" was not found; using the default device"
                ));
            }
        }
    }
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default output device".to_string())?;
    let mut opened = open_device(device, config, telemetry, healthy)?;
    opened.warning = warning;
    Ok(opened)
}

fn open_device(
    device: cpal::Device,
    config: &EngineConfig,
    telemetry: &Arc<Telemetry>,
    healthy: &Arc<AtomicBool>,
) -> Result<Opened, String> {
    let name = device_name(&device);

    let default_rate = device.default_output_config().ok().map(|c| c.sample_rate());
    let wanted_rate = config
        .sample_rate
        .or(default_rate)
        .unwrap_or(NULL_SAMPLE_RATE);
    let ranges: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| format!("cannot query {name}: {e}"))?
        .collect();

    let mut candidates: Vec<(u8, u8, u8, Candidate)> = ranges
        .iter()
        .filter_map(|r| {
            let fmt_rank = format_rank(r.sample_format())?;
            let (min, max) = (r.min_sample_rate(), r.max_sample_rate());
            let rate_ok = (min..=max).contains(&wanted_rate);
            let rate = if rate_ok {
                wanted_rate
            } else {
                default_rate
                    .filter(|d| (min..=max).contains(d))
                    .unwrap_or_else(|| wanted_rate.clamp(min, max))
            };
            let ch_rank = match r.channels() {
                2 => 0,
                0 => return None,
                1 => 2,
                _ => 1,
            };
            Some((
                u8::from(!rate_ok),
                ch_rank,
                fmt_rank,
                Candidate {
                    channels: r.channels(),
                    rate,
                    format: r.sample_format(),
                    buffer: *r.buffer_size(),
                },
            ))
        })
        .collect();
    candidates.sort_by_key(|(a, b, c, _)| (*a, *b, *c));
    if candidates.is_empty() {
        return Err(format!("{name} offers no usable output format"));
    }

    let mut last_error = String::new();
    for (_, _, _, cand) in candidates.iter().take(4) {
        let requested = config.buffer_frames.map(|n| match cand.buffer {
            SupportedBufferSize::Range { min, max } => n.clamp(min, max),
            SupportedBufferSize::Unknown => n,
        });
        let sizes = match requested {
            Some(n) => [BufferSize::Fixed(n), BufferSize::Default],
            None => [BufferSize::Default, BufferSize::Default],
        };
        for (i, size) in sizes.into_iter().enumerate() {
            if i == 1 && sizes[0] == sizes[1] {
                break;
            }
            let stream_config = StreamConfig {
                channels: cand.channels,
                sample_rate: cand.rate,
                buffer_size: size,
            };
            // A failed attempt drops its engine, so each attempt gets its own.
            let (engine, commands, retired, live) =
                make_parts(cand.rate as f32, config, telemetry.clone());
            let max_block = engine.max_block();
            match build_stream(
                &device,
                cand.format,
                stream_config,
                engine,
                max_block,
                telemetry.clone(),
                healthy.clone(),
            ) {
                Ok(stream) => {
                    if let Err(e) = stream.play() {
                        last_error = format!("could not start {name}: {e}");
                        continue;
                    }
                    let buffer_frames = stream
                        .buffer_size()
                        .ok()
                        .or(match size {
                            BufferSize::Fixed(n) => Some(n),
                            BufferSize::Default => None,
                        })
                        .unwrap_or(0);
                    return Ok(Opened {
                        stream,
                        commands,
                        retired,
                        live,
                        name,
                        sample_rate: cand.rate as f32,
                        buffer_frames,
                        warning: None,
                    });
                }
                Err(e) => {
                    last_error = format!("{name} {:?} {} ch: {e}", cand.format, cand.channels);
                }
            }
        }
    }
    Err(last_error)
}

fn build_stream(
    device: &cpal::Device,
    format: SampleFormat,
    config: StreamConfig,
    engine: Engine,
    max_block: usize,
    telemetry: Arc<Telemetry>,
    healthy: Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = config.channels as usize;
    let on_error = move |err: cpal::Error| match err.kind() {
        ErrorKind::Xrun => {
            telemetry.overruns.fetch_add(1, Ordering::Relaxed);
            log::warn!("audio xrun: {err}");
        }
        ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {
            log::warn!("audio stream: {err}");
        }
        _ => {
            log::error!("audio stream error: {err}");
            healthy.store(false, Ordering::Relaxed);
        }
    };
    macro_rules! build {
        ($t:ty) => {{
            let cb = make_callback::<$t>(engine, channels, max_block);
            device.build_output_stream(config, cb, on_error, None)
        }};
    }
    match format {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::F64 => build!(f64),
        SampleFormat::U16 => build!(u16),
        SampleFormat::I24 => build!(cpal::I24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U24 => build!(cpal::U24),
        SampleFormat::I8 => build!(i8),
        SampleFormat::U8 => build!(u8),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U64 => build!(u64),
        other => Err(cpal::Error::with_message(
            ErrorKind::UnsupportedConfig,
            format!("unsupported sample format {other}"),
        )),
    }
}

/// The audio callback for sample type `T`. Preallocates its stereo scratch,
/// then per call renders the device buffer in `max_block` chunks and
/// converts to the device's format and channel count.
fn make_callback<T>(
    mut engine: Engine,
    channels: usize,
    max_block: usize,
) -> impl FnMut(&mut [T], &cpal::OutputCallbackInfo) + Send + 'static
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let mut left = vec![0.0f32; max_block];
    let mut right = vec![0.0f32; max_block];
    let clamp = !T::FORMAT.is_float();
    let channels = channels.max(1);
    move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
        crate::rt::guard(|| {
            let frames = data.len() / channels;
            engine.begin_callback_frames(frames);
            let mut done = 0;
            while done < frames {
                let n = (frames - done).min(max_block);
                engine.render(&mut left[..n], &mut right[..n]);
                write_interleaved(
                    &mut data[done * channels..(done + n) * channels],
                    channels,
                    &left[..n],
                    &right[..n],
                    clamp,
                );
                done += n;
            }
            for x in &mut data[frames * channels..] {
                *x = T::EQUILIBRIUM;
            }
            engine.end_callback(frames);
        });
    }
}

/// Interleaves stereo into `channels` device channels: mono is the mid mix,
/// stereo passes through, extra channels are silent.
fn write_interleaved<T: Sample + FromSample<f32>>(
    out: &mut [T],
    channels: usize,
    l: &[f32],
    r: &[f32],
    clamp: bool,
) {
    let conv = |x: f32| -> T { T::from_sample(if clamp { x.clamp(-1.0, 1.0) } else { x }) };
    match channels {
        1 => {
            for (o, (a, b)) in out.iter_mut().zip(l.iter().zip(r)) {
                *o = conv((a + b) * 0.5);
            }
        }
        2 => {
            let (pairs, _) = out.as_chunks_mut::<2>();
            for (o, (a, b)) in pairs.iter_mut().zip(l.iter().zip(r)) {
                o[0] = conv(*a);
                o[1] = conv(*b);
            }
        }
        _ => {
            for (o, (a, b)) in out.chunks_exact_mut(channels).zip(l.iter().zip(r)) {
                o[0] = conv(*a);
                o[1] = conv(*b);
                for x in &mut o[2..] {
                    *x = T::EQUILIBRIUM;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(i, n)| (i.to_string(), n.to_string()))
            .collect()
    }

    #[test]
    fn a_device_is_found_by_id_then_name_then_loose_name() {
        let devices = pairs(&[
            ("coreaudio:A", "MacBook Speakers"),
            ("coreaudio:B", "USB Interface"),
            ("coreaudio:C", "USB Interface"),
        ]);
        assert_eq!(match_device(&devices, "coreaudio:C"), Some(2));
        assert_eq!(match_device(&devices, "USB Interface"), Some(1));
        assert_eq!(match_device(&devices, "macbook speakers"), Some(0));
        assert_eq!(match_device(&devices, "Headphones"), None);
        assert_eq!(match_device(&[], "anything"), None);
    }

    #[test]
    fn sample_rate_ranges_merge_and_list_common_rates() {
        let merged = merge_ranges(vec![(44_100, 48_000), (8_000, 16_000), (47_000, 96_000)]);
        assert_eq!(merged, vec![(8_000, 16_000), (44_100, 96_000)]);
        assert_eq!(common_rates(&merged), vec![44_100, 48_000, 88_200, 96_000]);
        assert!(common_rates(&[]).is_empty());
        // A fixed-rate device is a one-point range.
        assert_eq!(common_rates(&[(48_000, 48_000)]), vec![48_000]);
    }

    #[test]
    fn buffer_ranges_union() {
        assert_eq!(union_buffer_sizes(std::iter::empty()), None);
        assert_eq!(
            union_buffer_sizes([(64, 2048), (32, 1024)].into_iter()),
            Some((32, 2048))
        );
    }

    #[test]
    fn the_null_backend_ignores_the_requested_device_and_stops_cleanly() {
        let mut h = EngineHandle::start_null(EngineConfig {
            device: Some("No Such Interface".into()),
            buffer_frames: Some(128),
            ..EngineConfig::default()
        })
        .unwrap();
        assert_eq!(h.device_name(), NO_DEVICE_NAME);
        assert_eq!(h.buffer_frames(), 128);
        assert!(h.device_warning().is_none());
        assert!(h.is_healthy());
        h.stop_audio();
        assert!(!h.is_healthy());
        assert!(!h.is_null_backend());
    }

    #[test]
    fn enumerating_devices_never_panics() {
        // Content depends on the machine: CI runners often have none.
        let devices = list_output_devices();
        assert!(devices.iter().filter(|d| d.is_default).count() <= 1);
        for d in &devices {
            assert!(!d.name.is_empty());
            assert!(d.max_channels > 0);
            assert!(!d.sample_rate_ranges.is_empty());
        }
    }

    #[test]
    #[ignore = "needs an audio output device"]
    fn an_unknown_device_falls_back_to_the_default_with_a_warning() {
        let h = EngineHandle::start(EngineConfig {
            device: Some("No Such Interface".into()),
            ..EngineConfig::default()
        })
        .unwrap();
        assert!(!h.is_null_backend());
        let warning = h.device_warning().expect("a warning");
        assert!(warning.contains("No Such Interface"), "{warning}");
    }

    #[test]
    #[ignore = "needs an audio output device"]
    fn each_listed_device_can_be_selected_by_name() {
        for info in list_output_devices() {
            let h = EngineHandle::start(EngineConfig {
                device: Some(info.name.clone()),
                ..EngineConfig::default()
            })
            .unwrap();
            if !h.is_null_backend() && h.device_warning().is_none() {
                assert_eq!(h.device_name(), info.name);
            }
        }
    }
}
