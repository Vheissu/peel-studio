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
}

// Both variants are held only to keep the stream or thread alive until drop.
#[allow(dead_code)]
enum Backend {
    Cpal(cpal::Stream),
    Null(NullBackend),
}

impl EngineHandle {
    /// Opens the default output device and starts the engine on its
    /// callback. If no device can be opened, falls back to a silent
    /// real-time clock ([`start_null`](EngineHandle::start_null)) so the
    /// application keeps running; [`device_name`](EngineHandle::device_name)
    /// then reports "No audio device".
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
                })
            }
            Err(reason) => {
                log::warn!("no usable audio device ({reason}); running without audio output");
                Self::start_null_with(config, telemetry, healthy)
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
        let (engine, commands, retired) = make_parts(sample_rate, &config, telemetry.clone());
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
) -> (Engine, Producer<EngineCommand>, Consumer<Retired>) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(config.command_capacity.max(64));
    let (ret_tx, ret_rx) = RingBuffer::new(config.retire_capacity.max(64));
    let engine = Engine::new(sample_rate, config, telemetry, ret_tx, Some(cmd_rx));
    (engine, cmd_tx, ret_rx)
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
                    engine.begin_callback();
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
    name: String,
    sample_rate: f32,
    buffer_frames: u32,
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

fn open_cpal(
    config: &EngineConfig,
    telemetry: &Arc<Telemetry>,
    healthy: &Arc<AtomicBool>,
) -> Result<Opened, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default output device".to_string())?;
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| device.to_string());

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
            let (engine, commands, retired) =
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
                        name,
                        sample_rate: cand.rate as f32,
                        buffer_frames,
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
            engine.begin_callback();
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
            for (o, (a, b)) in out.chunks_exact_mut(2).zip(l.iter().zip(r)) {
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
