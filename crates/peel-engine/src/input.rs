//! Audio input: the realtime half of recording and monitoring.
//!
//! A microphone or instrument arrives on the input device's own callback
//! thread. That thread runs an [`InputProcessor`], which does four things per
//! callback and nothing that can block or allocate:
//!
//! 1. picks the chosen device channels and converts them to float, as a
//!    stereo pair (a mono selection is the same signal on both sides);
//! 2. measures the peak for the input meter;
//! 3. while a take is being captured, copies the frames into the **capture
//!    ring**, a bounded SPSC ring of about two seconds that the control
//!    side's writer thread drains to disk;
//! 4. while monitoring is on, copies them into the **monitor FIFO**, a second
//!    small ring that the output callback reads and adds to its output
//!    (after the master, like the metronome).
//!
//! ```text
//!  input callback ──► capture ring ──► writer thread ──► WAV on disk
//!        │ (frames, peak, timing)
//!        └──────────► monitor FIFO ──► output callback (summed after master)
//! ```
//!
//! # Threads and rules
//!
//! The processor owns the ring producers and holds only atomics otherwise
//! ([`InputShared`]); it never allocates, locks or logs. If the capture ring is
//! full (the disk stalled for two seconds) the frames are dropped, counted in
//! [`InputShared::dropped_frames`], and the take is padded with the same
//! number of silent frames as soon as the ring has room, so the rest of it
//! stays on the timeline. If the monitor FIFO is full the frames are dropped
//! and counted too.
//!
//! The monitor FIFO decouples two callbacks that run on their own schedules.
//! The output side waits until it holds one output block plus one input block
//! (its *prefill*) before it starts playing, so ordinary callback jitter never
//! starves it. When it still runs dry it plays silence for the missing
//! frames, counts an underrun and refills; when the FIFO grows past its
//! ceiling (one side stalled) the oldest frames are discarded. The FIFO
//! length is part of the monitoring latency [`InputShared::monitor_stats`]
//! reports.
//!
//! # Time
//!
//! A take starts at the first input callback after [`InputShared::arm_capture`]
//! in which the transport is playing. That callback records the transport
//! position and the callback timings (see [`CaptureStart`]); the control side
//! turns them into a position on the timeline with
//! [`take_start_beat`](crate::latency::take_start_beat).
//!
//! # Tests without a device
//!
//! [`InputInjector`] runs the very same processor on the caller's thread, so
//! tests and tools can feed any signal through the real capture path.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cpal::{FromSample, Sample};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::latency::{CaptureStart, LatencyReport};
use crate::telemetry::Telemetry;

/// Seconds of audio the capture ring holds.
pub const CAPTURE_SECONDS: f32 = 2.0;
/// Frames the monitor FIFO holds.
pub const MONITOR_FIFO_FRAMES: usize = 16_384;
/// Frames converted per pass in the callback (a stack buffer of this size).
const CHUNK: usize = 256;
/// Silent frames written per push when paying back dropped frames.
const SILENCE_FRAMES: usize = 512;
static SILENCE: [f32; SILENCE_FRAMES * 2] = [0.0; SILENCE_FRAMES * 2];

// ----------------------------------------------------------------------
// Channel selection
// ----------------------------------------------------------------------

/// Which device channels a take records. Channel numbers start at 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputChannels {
    /// One channel, recorded as a mono file.
    Mono(u16),
    /// Two channels, recorded as a stereo file (left, right).
    Stereo(u16, u16),
}

impl Default for InputChannels {
    fn default() -> Self {
        InputChannels::Mono(0)
    }
}

impl InputChannels {
    /// Channels in the recorded file: 1 or 2.
    pub fn count(self) -> u16 {
        match self {
            InputChannels::Mono(_) => 1,
            InputChannels::Stereo(..) => 2,
        }
    }

    /// The highest device channel index used.
    pub fn highest(self) -> u16 {
        match self {
            InputChannels::Mono(a) => a,
            InputChannels::Stereo(a, b) => a.max(b),
        }
    }

    /// The same selection with every index below `device_channels`.
    pub fn clamped(self, device_channels: u16) -> InputChannels {
        let last = device_channels.max(1) - 1;
        match self {
            InputChannels::Mono(a) => InputChannels::Mono(a.min(last)),
            InputChannels::Stereo(a, b) => InputChannels::Stereo(a.min(last), b.min(last)),
        }
    }

    fn pack(self) -> u32 {
        match self {
            InputChannels::Mono(a) => u32::from(a),
            InputChannels::Stereo(a, b) => (1 << 31) | (u32::from(b) << 16) | u32::from(a),
        }
    }

    fn unpack(v: u32) -> InputChannels {
        if v >> 31 == 1 {
            InputChannels::Stereo((v & 0xFFFF) as u16, ((v >> 16) & 0x7FFF) as u16)
        } else {
            InputChannels::Mono((v & 0xFFFF) as u16)
        }
    }
}

// ----------------------------------------------------------------------
// Shared state
// ----------------------------------------------------------------------

/// Where a take is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureState {
    /// Not capturing.
    Idle,
    /// Waiting for the first input callback during which the transport plays.
    Armed,
    /// Copying frames into the capture ring.
    Capturing,
}

const STATE_IDLE: u8 = 0;
const STATE_ARMED: u8 = 1;
const STATE_CAPTURING: u8 = 2;

/// Why input stopped working.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputFailure {
    /// The device was unplugged or disappeared.
    DeviceLost,
    /// The stream reported another error.
    StreamError,
}

/// Monitoring figures for a status line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MonitorStats {
    /// Times the output ran out of input frames and played silence.
    pub underruns: u64,
    /// Frames discarded because a side stalled and the FIFO overflowed.
    pub dropped_frames: u64,
    /// Typical frames waiting in the FIFO (its share of the latency).
    pub fifo_frames: f32,
}

/// State shared by the input callback, the output callback and the control
/// thread. Everything is an atomic except the capture ring's consumer, which
/// only control-side threads lock.
pub struct InputShared {
    // meter and routing
    peak: [AtomicU32; 2],
    selection: AtomicU32,
    // capture
    state: AtomicU8,
    callbacks: AtomicU64,
    captured: AtomicU64,
    dropped: AtomicU64,
    capture_rx: Mutex<Option<Consumer<f32>>>,
    // the device
    open: AtomicBool,
    sample_rate: AtomicU32,
    device_channels: AtomicU32,
    failure: AtomicU8,
    last_error: Mutex<Option<String>>,
    xruns: AtomicU64,
    /// A sample other than digital silence has arrived since the input opened.
    heard: AtomicBool,
    // timing, written by the callbacks, read by the control side
    in_block: AtomicU32,
    in_latency_ns: AtomicU64,
    out_callback_ns: AtomicU64,
    out_lead_ns: AtomicU64,
    out_frames: AtomicU32,
    out_rate: AtomicU32,
    // snapshot at the first frame of a take
    start_ready: AtomicBool,
    start: [AtomicU64; 5],
    // monitoring
    monitor_on: AtomicBool,
    monitor_gain: AtomicU32,
    monitor_supported: AtomicBool,
    monitor_underruns: AtomicU64,
    monitor_dropped: AtomicU64,
    monitor_fill: AtomicU32,
}

fn zero_u64<const N: usize>() -> [AtomicU64; N] {
    std::array::from_fn(|_| AtomicU64::new(0))
}

#[inline]
fn store_max(cell: &AtomicU32, value: f32) {
    let mut cur = cell.load(Ordering::Relaxed);
    while f32::from_bits(cur) < value {
        match cell.compare_exchange_weak(cur, value.to_bits(), Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

impl InputShared {
    /// `output_rate` is the engine's sample rate, for converting the output
    /// callback sizes the engine reports.
    pub(crate) fn new(output_rate: f32) -> InputShared {
        InputShared {
            peak: [AtomicU32::new(0), AtomicU32::new(0)],
            selection: AtomicU32::new(InputChannels::default().pack()),
            state: AtomicU8::new(STATE_IDLE),
            callbacks: AtomicU64::new(0),
            captured: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            capture_rx: Mutex::new(None),
            open: AtomicBool::new(false),
            sample_rate: AtomicU32::new(0),
            device_channels: AtomicU32::new(0),
            failure: AtomicU8::new(0),
            last_error: Mutex::new(None),
            xruns: AtomicU64::new(0),
            heard: AtomicBool::new(false),
            in_block: AtomicU32::new(0),
            in_latency_ns: AtomicU64::new(0),
            out_callback_ns: AtomicU64::new(0),
            out_lead_ns: AtomicU64::new(0),
            out_frames: AtomicU32::new(0),
            out_rate: AtomicU32::new(output_rate.max(1.0) as u32),
            start_ready: AtomicBool::new(false),
            start: zero_u64(),
            monitor_on: AtomicBool::new(false),
            monitor_gain: AtomicU32::new(1.0f32.to_bits()),
            monitor_supported: AtomicBool::new(false),
            monitor_underruns: AtomicU64::new(0),
            monitor_dropped: AtomicU64::new(0),
            monitor_fill: AtomicU32::new(0),
        }
    }

    // ---- device ----

    /// An input stream (or injector) is attached.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    /// The input device's sample rate (0 when none is open).
    pub fn input_sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    /// Length in seconds of the input callbacks as last seen (0 before the
    /// first one).
    pub fn input_block_seconds(&self) -> f64 {
        f64::from(self.in_block.load(Ordering::Relaxed))
            / f64::from(self.sample_rate.load(Ordering::Relaxed).max(1))
    }

    /// Channels the input device delivers (0 when none is open).
    pub fn device_channels(&self) -> u16 {
        self.device_channels.load(Ordering::Relaxed) as u16
    }

    /// Set once the device is lost or its stream fails; cleared by opening
    /// input again.
    pub fn failure(&self) -> Option<InputFailure> {
        match self.failure.load(Ordering::Relaxed) {
            0 => None,
            1 => Some(InputFailure::DeviceLost),
            _ => Some(InputFailure::StreamError),
        }
    }

    /// The last error the input stream reported.
    pub fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// True once any non-zero sample has arrived since the input opened. An
    /// input that stays digitally silent for seconds is usually muted,
    /// unplugged at the interface or, on macOS, denied microphone access (the
    /// system then delivers silence instead of an error).
    pub fn heard_signal(&self) -> bool {
        self.heard.load(Ordering::Relaxed)
    }

    /// Overruns the device reported (it delivered audio we were too slow to
    /// take).
    pub fn xruns(&self) -> u64 {
        self.xruns.load(Ordering::Relaxed)
    }

    /// Input callbacks that have run.
    pub fn callbacks(&self) -> u64 {
        self.callbacks.load(Ordering::Acquire)
    }

    pub(crate) fn attach(
        &self,
        sample_rate: u32,
        device_channels: u16,
        capture: Consumer<f32>,
        monitor_supported: bool,
    ) {
        *self
            .capture_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(capture);
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
        self.device_channels
            .store(u32::from(device_channels), Ordering::Relaxed);
        self.failure.store(0, Ordering::Relaxed);
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.state.store(STATE_IDLE, Ordering::Release);
        self.heard.store(false, Ordering::Relaxed);
        self.monitor_supported
            .store(monitor_supported, Ordering::Relaxed);
        self.in_block.store(0, Ordering::Relaxed);
        self.in_latency_ns.store(0, Ordering::Relaxed);
        let clamped =
            InputChannels::unpack(self.selection.load(Ordering::Relaxed)).clamped(device_channels);
        self.selection.store(clamped.pack(), Ordering::Relaxed);
        self.open.store(true, Ordering::Release);
    }

    pub(crate) fn detach(&self) {
        self.open.store(false, Ordering::Release);
        self.state.store(STATE_IDLE, Ordering::Release);
        self.monitor_supported.store(false, Ordering::Relaxed);
        *self
            .capture_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.sample_rate.store(0, Ordering::Relaxed);
        self.device_channels.store(0, Ordering::Relaxed);
    }

    pub(crate) fn fail(&self, failure: InputFailure, message: String) {
        self.failure.store(
            match failure {
                InputFailure::DeviceLost => 1,
                InputFailure::StreamError => 2,
            },
            Ordering::Relaxed,
        );
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(message);
    }

    pub(crate) fn note_xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }

    // ---- meter and channels ----

    /// Takes (and clears) the input peak since the last call, left and right
    /// (the same value for a mono selection).
    pub fn take_peak(&self) -> (f32, f32) {
        (
            f32::from_bits(self.peak[0].swap(0, Ordering::Relaxed)),
            f32::from_bits(self.peak[1].swap(0, Ordering::Relaxed)),
        )
    }

    /// The channels recorded and monitored.
    pub fn channels(&self) -> InputChannels {
        InputChannels::unpack(self.selection.load(Ordering::Relaxed))
    }

    /// Changes the channels, effective at the next callback. Indices beyond
    /// the device are clamped. Do not change the channel *count* during a
    /// take: the file is already open with the old one.
    pub fn set_channels(&self, channels: InputChannels) {
        let clamped = match self.device_channels() {
            0 => channels,
            n => channels.clamped(n),
        };
        self.selection.store(clamped.pack(), Ordering::Relaxed);
    }

    // ---- capture ----

    pub fn capture_state(&self) -> CaptureState {
        match self.state.load(Ordering::Acquire) {
            STATE_ARMED => CaptureState::Armed,
            STATE_CAPTURING => CaptureState::Capturing,
            _ => CaptureState::Idle,
        }
    }

    /// Starts a take: clears the counters, empties the ring and waits for the
    /// first input callback while the transport plays. Returns false when no
    /// input is open.
    pub fn arm_capture(&self) -> bool {
        if !self.is_open() {
            return false;
        }
        self.state.store(STATE_IDLE, Ordering::SeqCst);
        self.discard_capture();
        self.captured.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.start_ready.store(false, Ordering::Release);
        self.state.store(STATE_ARMED, Ordering::SeqCst);
        true
    }

    /// Stops copying frames into the ring. Frames of a callback that is
    /// running right now may still arrive; wait for [`callbacks`](Self::callbacks)
    /// to advance past its value at this call before treating the ring as
    /// final.
    pub fn end_capture(&self) {
        self.state.store(STATE_IDLE, Ordering::SeqCst);
    }

    /// Frames that went into the ring in this take.
    pub fn captured_frames(&self) -> u64 {
        self.captured.load(Ordering::Acquire)
    }

    /// Frames lost in this take because the ring was full (the take was
    /// padded with silence of the same length).
    pub fn dropped_frames(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The measurements of the take's first frame, once there is one.
    pub fn capture_start(&self) -> Option<CaptureStart> {
        if !self.start_ready.load(Ordering::Acquire) {
            return None;
        }
        let f = |i: usize| f64::from_bits(self.start[i].load(Ordering::Relaxed));
        let age = f(3);
        Some(CaptureStart {
            playhead_beat: f(0),
            output_lead_s: f(1),
            output_block_s: f(2),
            output_age_s: (!age.is_nan()).then_some(age),
            input_latency_s: f(4),
        })
    }

    /// Moves up to `out.len()` interleaved stereo samples (always whole
    /// frames) from the capture ring into `out`; returns how many samples.
    /// Writer-thread side.
    pub fn drain_capture(&self, out: &mut [f32]) -> usize {
        let mut guard = self
            .capture_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(rx) = guard.as_mut() else {
            return 0;
        };
        let n = rx.slots().min(out.len()) & !1;
        if n == 0 {
            return 0;
        }
        match rx.read_chunk(n) {
            Ok(chunk) => {
                let (a, b) = chunk.as_slices();
                out[..a.len()].copy_from_slice(a);
                out[a.len()..a.len() + b.len()].copy_from_slice(b);
                chunk.commit_all();
                n
            }
            Err(_) => 0,
        }
    }

    /// Samples waiting in the capture ring.
    pub fn capture_pending(&self) -> usize {
        self.capture_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(0, |rx| rx.slots())
    }

    fn discard_capture(&self) {
        let mut out = vec![0.0f32; 4096];
        while self.drain_capture(&mut out) > 0 {}
    }

    // ---- monitoring ----

    /// Turns software monitoring on or off and sets its linear gain
    /// (0..=4). Changes are ramped across one output block, so they do not
    /// click.
    pub fn set_monitoring(&self, on: bool, gain: f32) {
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 4.0)
        } else {
            1.0
        };
        self.monitor_gain.store(gain.to_bits(), Ordering::Relaxed);
        self.monitor_on.store(on, Ordering::Relaxed);
    }

    pub fn monitoring(&self) -> (bool, f32) {
        (self.monitor_on.load(Ordering::Relaxed), self.monitor_gain())
    }

    fn monitor_gain(&self) -> f32 {
        f32::from_bits(self.monitor_gain.load(Ordering::Relaxed))
    }

    /// Monitoring needs the input and output to run at the same sample rate;
    /// false when they differ or no input is open.
    pub fn monitor_supported(&self) -> bool {
        self.monitor_supported.load(Ordering::Relaxed)
    }

    pub fn monitor_stats(&self) -> MonitorStats {
        MonitorStats {
            underruns: self.monitor_underruns.load(Ordering::Relaxed),
            dropped_frames: self.monitor_dropped.load(Ordering::Relaxed),
            fifo_frames: f32::from_bits(self.monitor_fill.load(Ordering::Relaxed)),
        }
    }

    // ---- latency ----

    /// Input and output latency as last measured by the callbacks.
    pub fn latency(&self) -> LatencyReport {
        let in_rate = f64::from(self.sample_rate.load(Ordering::Relaxed).max(1));
        let in_block = f64::from(self.in_block.load(Ordering::Relaxed)) / in_rate;
        let input_ns = self.in_latency_ns.load(Ordering::Relaxed);
        let out_rate = f64::from(self.out_rate.load(Ordering::Relaxed).max(1));
        let out_block = f64::from(self.out_frames.load(Ordering::Relaxed)) / out_rate;
        let lead_ns = self.out_lead_ns.load(Ordering::Relaxed);
        LatencyReport {
            input_s: if input_ns > 0 {
                input_ns as f64 * 1e-9
            } else {
                in_block
            },
            output_s: if lead_ns > 0 {
                lead_ns as f64 * 1e-9
            } else {
                out_block
            },
        }
    }

    /// Called by the output callback with its timing. Realtime-safe.
    #[inline]
    pub(crate) fn note_output_timing(&self, callback_ns: u64, lead_ns: u64, frames: usize) {
        self.out_callback_ns.store(callback_ns, Ordering::Relaxed);
        self.out_lead_ns.store(lead_ns, Ordering::Relaxed);
        self.out_frames.store(frames as u32, Ordering::Relaxed);
    }
}

// ----------------------------------------------------------------------
// The input callback
// ----------------------------------------------------------------------

/// Timestamps of one input callback in nanoseconds on the host's stream
/// clock. Zero means the host did not say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallbackTiming {
    /// When the callback ran.
    pub callback_ns: u64,
    /// When its first frame was captured by the converter.
    pub capture_ns: u64,
}

/// The realtime core of input: one call per device callback.
///
/// Created by [`EngineHandle::open_input`](crate::EngineHandle::open_input);
/// owned by the input stream's callback (or an [`InputInjector`]).
pub struct InputProcessor {
    shared: Arc<InputShared>,
    telemetry: Arc<Telemetry>,
    capture: Producer<f32>,
    monitor: Option<Producer<f32>>,
    channels: usize,
    sample_rate: u32,
    /// Capture frames dropped and not yet replaced by silence.
    gap: u64,
}

impl InputProcessor {
    pub(crate) fn new(
        shared: Arc<InputShared>,
        telemetry: Arc<Telemetry>,
        capture: Producer<f32>,
        monitor: Option<Producer<f32>>,
        channels: u16,
        sample_rate: u32,
    ) -> InputProcessor {
        InputProcessor {
            shared,
            telemetry,
            capture,
            monitor,
            channels: usize::from(channels).max(1),
            sample_rate,
            gap: 0,
        }
    }

    /// Handles one device callback of interleaved samples.
    ///
    /// Realtime-safe: no allocation, no locks, no waiting.
    pub fn process<T>(&mut self, data: &[T], timing: CallbackTiming)
    where
        T: Sample,
        f32: FromSample<T>,
    {
        let ch = self.channels;
        let frames = data.len() / ch;
        let shared = &*self.shared;
        if frames > 0 {
            shared.in_block.store(frames as u32, Ordering::Relaxed);
            if timing.capture_ns > 0 && timing.callback_ns > timing.capture_ns {
                shared
                    .in_latency_ns
                    .store(timing.callback_ns - timing.capture_ns, Ordering::Relaxed);
            }
        }

        // A take begins at the first callback with the transport running.
        let mut state = shared.state.load(Ordering::Acquire);
        if state == STATE_ARMED && frames > 0 && self.telemetry.playing.load(Ordering::Relaxed) {
            if shared
                .state
                .compare_exchange(
                    STATE_ARMED,
                    STATE_CAPTURING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                publish_start(shared, &self.telemetry, frames, self.sample_rate, timing);
                state = STATE_CAPTURING;
            } else {
                state = shared.state.load(Ordering::Acquire);
            }
        }
        let capturing = state == STATE_CAPTURING;
        let monitoring = self.monitor.is_some() && shared.monitor_on.load(Ordering::Relaxed);

        let sel = InputChannels::unpack(shared.selection.load(Ordering::Relaxed));
        let last = ch - 1;
        let (a, b) = match sel {
            InputChannels::Mono(a) => (usize::from(a).min(last), usize::from(a).min(last)),
            InputChannels::Stereo(a, b) => (usize::from(a).min(last), usize::from(b).min(last)),
        };

        let mut peak = [0.0f32; 2];
        let mut buf = [0.0f32; CHUNK * 2];
        for block in data.chunks(CHUNK * ch) {
            let n = block.len() / ch;
            for (out, frame) in buf
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(block.chunks_exact(ch))
            {
                let l = clean(frame[a].to_sample::<f32>());
                let r = clean(frame[b].to_sample::<f32>());
                peak[0] = peak[0].max(l.abs());
                peak[1] = peak[1].max(r.abs());
                out[0] = l;
                out[1] = r;
            }
            let stereo = &buf[..n * 2];
            if capturing {
                push_with_debt(
                    &mut self.capture,
                    stereo,
                    &mut self.gap,
                    &shared.captured,
                    &shared.dropped,
                );
            }
            if monitoring && let Some(tx) = self.monitor.as_mut() {
                let pushed = push_frames(tx, stereo);
                if pushed < n {
                    shared
                        .monitor_dropped
                        .fetch_add((n - pushed) as u64, Ordering::Relaxed);
                }
            }
        }
        store_max(&shared.peak[0], peak[0]);
        store_max(&shared.peak[1], peak[1]);
        if (peak[0] > 0.0 || peak[1] > 0.0) && !shared.heard.load(Ordering::Relaxed) {
            shared.heard.store(true, Ordering::Relaxed);
        }
        // Last: a reader that sees this count has seen all of the callback's
        // frames in the ring.
        shared.callbacks.fetch_add(1, Ordering::Release);
    }
}

#[inline]
fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Pushes whole stereo frames that fit; returns how many frames went in.
fn push_frames(tx: &mut Producer<f32>, stereo: &[f32]) -> usize {
    let frames = stereo.len() / 2;
    let fit = (tx.slots() / 2).min(frames);
    if fit > 0 {
        let _ = tx.push_entire_slice(&stereo[..fit * 2]);
    }
    fit
}

/// Pushes into the capture ring, first paying back any frames dropped
/// earlier with silence so that the take keeps its length.
fn push_with_debt(
    tx: &mut Producer<f32>,
    stereo: &[f32],
    gap: &mut u64,
    captured: &AtomicU64,
    dropped: &AtomicU64,
) {
    while *gap > 0 {
        let want = (*gap).min(SILENCE_FRAMES as u64) as usize;
        let fit = push_frames(tx, &SILENCE[..want * 2]);
        *gap -= fit as u64;
        if fit < want {
            break;
        }
    }
    let frames = stereo.len() / 2;
    if *gap > 0 {
        *gap += frames as u64;
        dropped.fetch_add(frames as u64, Ordering::Relaxed);
        return;
    }
    let fit = push_frames(tx, stereo);
    if fit < frames {
        let lost = (frames - fit) as u64;
        *gap += lost;
        dropped.fetch_add(lost, Ordering::Relaxed);
    }
    if fit > 0 {
        captured.fetch_add(fit as u64, Ordering::Release);
    }
}

/// Records what the first frame of a take looked like. Called once per take.
fn publish_start(
    shared: &InputShared,
    telemetry: &Telemetry,
    frames: usize,
    sample_rate: u32,
    timing: CallbackTiming,
) {
    let in_block_s = frames as f64 / f64::from(sample_rate.max(1));
    let input_latency_s = if timing.capture_ns > 0 && timing.callback_ns > timing.capture_ns {
        ((timing.callback_ns - timing.capture_ns) as f64 * 1e-9).min(1.0)
    } else {
        in_block_s
    };
    let out_rate = f64::from(shared.out_rate.load(Ordering::Relaxed).max(1));
    let out_frames = shared.out_frames.load(Ordering::Relaxed);
    let output_block_s = if out_frames > 0 {
        f64::from(out_frames) / out_rate
    } else {
        in_block_s
    };
    let lead_ns = shared.out_lead_ns.load(Ordering::Relaxed);
    let output_lead_s = if lead_ns > 0 {
        (lead_ns as f64 * 1e-9).min(1.0)
    } else {
        output_block_s
    };
    let out_cb = shared.out_callback_ns.load(Ordering::Relaxed);
    let age = (timing.callback_ns > 0 && out_cb > 0 && timing.callback_ns >= out_cb)
        .then(|| (timing.callback_ns - out_cb) as f64 * 1e-9)
        .filter(|age| *age <= 2.0 * output_block_s);

    let values = [
        telemetry.beat(),
        output_lead_s,
        output_block_s,
        age.unwrap_or(f64::NAN),
        input_latency_s,
    ];
    for (cell, v) in shared.start.iter().zip(values) {
        cell.store(v.to_bits(), Ordering::Relaxed);
    }
    shared.start_ready.store(true, Ordering::Release);
}

// ----------------------------------------------------------------------
// Monitor FIFO, output side
// ----------------------------------------------------------------------

/// The output end of the monitor FIFO. Lives in the engine; the control side
/// sends one with [`EngineCommand::SetInputMonitor`](crate::EngineCommand)
/// when it opens an input.
pub struct MonitorReceiver {
    rx: Consumer<f32>,
    primed: bool,
    gain: f32,
    fill: f32,
}

/// A monitor FIFO: the producer goes to the input callback, the receiver to
/// the engine.
pub(crate) fn monitor_fifo() -> (Producer<f32>, MonitorReceiver) {
    let (tx, rx) = RingBuffer::new(MONITOR_FIFO_FRAMES * 2);
    (
        tx,
        MonitorReceiver {
            rx,
            primed: false,
            gain: 0.0,
            fill: 0.0,
        },
    )
}

/// Everything a successful open hands to the shared state and the engine.
pub(crate) struct InputParts {
    pub capture: Consumer<f32>,
    pub monitor: Option<MonitorReceiver>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Builds the processor for an input at `sample_rate` with `channels`, and
/// the other ends of its rings. Monitoring is wired only when the input runs
/// at the engine's `output_rate`.
pub(crate) fn new_processor(
    shared: &Arc<InputShared>,
    telemetry: &Arc<Telemetry>,
    sample_rate: u32,
    channels: u16,
    output_rate: u32,
) -> (InputProcessor, InputParts) {
    let (capture_tx, capture_rx) = capture_ring(sample_rate);
    let (monitor_tx, monitor_rx) = if sample_rate == output_rate {
        let (tx, rx) = monitor_fifo();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let processor = InputProcessor::new(
        shared.clone(),
        telemetry.clone(),
        capture_tx,
        monitor_tx,
        channels,
        sample_rate,
    );
    (
        processor,
        InputParts {
            capture: capture_rx,
            monitor: monitor_rx,
            sample_rate,
            channels,
        },
    )
}

/// The capture ring for an input at `sample_rate`.
pub(crate) fn capture_ring(sample_rate: u32) -> (Producer<f32>, Consumer<f32>) {
    let frames = (f64::from(sample_rate) * f64::from(CAPTURE_SECONDS)) as usize;
    RingBuffer::new(frames.max(4096) * 2)
}

impl MonitorReceiver {
    /// Adds the monitored input to `out_l`/`out_r`. Realtime-safe.
    pub(crate) fn mix_into(&mut self, shared: &InputShared, out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = out_l.len().min(out_r.len());
        if frames == 0 {
            return;
        }
        let slots = self.rx.slots();
        if !shared.monitor_on.load(Ordering::Relaxed) {
            // Off: throw away whatever is queued so switching on later starts
            // from live audio, and fade in from silence.
            if slots > 0
                && let Ok(chunk) = self.rx.read_chunk(slots)
            {
                chunk.commit_all();
            }
            self.primed = false;
            self.gain = 0.0;
            self.fill = 0.0;
            shared.monitor_fill.store(0, Ordering::Relaxed);
            return;
        }

        let mut avail = slots / 2;
        let in_block = shared.in_block.load(Ordering::Relaxed) as usize;
        let prefill = (frames + in_block).min(MONITOR_FIFO_FRAMES / 2);
        let ceiling = prefill + 2 * frames.max(in_block);
        if avail > ceiling {
            // One side stalled and the FIFO filled up: keep only the newest.
            let drop = avail - prefill;
            if let Ok(chunk) = self.rx.read_chunk(drop * 2) {
                chunk.commit_all();
            }
            shared
                .monitor_dropped
                .fetch_add(drop as u64, Ordering::Relaxed);
            avail = prefill;
        }
        if !self.primed && avail >= prefill {
            self.primed = true;
        }
        self.fill += (avail as f32 - self.fill) * 0.05;
        shared
            .monitor_fill
            .store(self.fill.to_bits(), Ordering::Relaxed);

        let target = shared.monitor_gain();
        let take = if self.primed { avail.min(frames) } else { 0 };
        if self.primed && take < frames {
            shared.monitor_underruns.fetch_add(1, Ordering::Relaxed);
            self.primed = false;
        }
        if take > 0
            && let Ok(chunk) = self.rx.read_chunk(take * 2)
        {
            let (a, b) = chunk.as_slices();
            let step = (target - self.gain) / frames as f32;
            let mut gain = self.gain;
            let mut i = 0;
            for part in [a, b] {
                for pair in part.as_chunks::<2>().0 {
                    out_l[i] += pair[0] * gain;
                    out_r[i] += pair[1] * gain;
                    gain += step;
                    i += 1;
                }
            }
            chunk.commit_all();
        }
        self.gain = target;
    }
}

// ----------------------------------------------------------------------
// Injection
// ----------------------------------------------------------------------

/// Feeds frames through the real input path without a device.
///
/// Obtained from [`EngineHandle::open_injected_input`](crate::EngineHandle::open_injected_input).
/// `push` runs [`InputProcessor::process`] on the calling thread, so a test
/// or a headless tool exercises the same code the device callback runs:
/// channel selection, metering, the capture ring, the monitor FIFO and the
/// take-start snapshot.
pub struct InputInjector {
    processor: InputProcessor,
    timing: CallbackTiming,
}

impl InputInjector {
    pub(crate) fn new(processor: InputProcessor) -> InputInjector {
        InputInjector {
            processor,
            timing: CallbackTiming::default(),
        }
    }

    /// Sample rate of the injected audio.
    pub fn sample_rate(&self) -> u32 {
        self.processor.sample_rate
    }

    /// Channels per injected frame.
    pub fn channels(&self) -> u16 {
        self.processor.channels as u16
    }

    /// Timestamps given to every following push (zero fields mean "not
    /// reported", like a host that gives no timing).
    pub fn set_timing(&mut self, timing: CallbackTiming) {
        self.timing = timing;
    }

    /// Makes the input report that its device failed, as an unplugged
    /// interface would.
    pub fn fail_device(&mut self, message: &str) {
        self.processor
            .shared
            .fail(InputFailure::DeviceLost, message.to_string());
    }

    /// Delivers one callback of interleaved frames in any sample format a
    /// device can use (`f32`, `i16`, ...).
    pub fn push<T>(&mut self, interleaved: &[T])
    where
        T: Sample,
        f32: FromSample<T>,
    {
        self.processor.process(interleaved, self.timing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rig {
        shared: Arc<InputShared>,
        telemetry: Arc<Telemetry>,
        input: InputProcessor,
    }

    fn rig(channels: u16, monitor: bool) -> (Rig, Option<MonitorReceiver>) {
        let shared = Arc::new(InputShared::new(48_000.0));
        let telemetry = Arc::new(Telemetry::new());
        let (cap_tx, cap_rx) = capture_ring(48_000);
        let (mon_tx, mon_rx) = monitor_fifo();
        shared.attach(48_000, channels, cap_rx, monitor);
        let input = InputProcessor::new(
            shared.clone(),
            telemetry.clone(),
            cap_tx,
            monitor.then_some(mon_tx),
            channels,
            48_000,
        );
        (
            Rig {
                shared,
                telemetry,
                input,
            },
            monitor.then_some(mon_rx),
        )
    }

    fn drain(shared: &InputShared) -> Vec<f32> {
        let mut all = Vec::new();
        let mut buf = vec![0.0f32; 1024];
        loop {
            let n = shared.drain_capture(&mut buf);
            if n == 0 {
                return all;
            }
            all.extend_from_slice(&buf[..n]);
        }
    }

    #[test]
    fn channel_selections_round_trip_and_clamp() {
        for sel in [
            InputChannels::Mono(0),
            InputChannels::Mono(7),
            InputChannels::Stereo(0, 1),
            InputChannels::Stereo(5, 2),
        ] {
            assert_eq!(InputChannels::unpack(sel.pack()), sel);
        }
        assert_eq!(
            InputChannels::Stereo(6, 7).clamped(2),
            InputChannels::Stereo(1, 1)
        );
        assert_eq!(InputChannels::Mono(3).clamped(0), InputChannels::Mono(0));
        assert_eq!(InputChannels::Stereo(4, 1).highest(), 4);
        assert_eq!(InputChannels::Mono(4).count(), 1);
    }

    #[test]
    fn nothing_is_captured_until_armed_and_the_transport_plays() {
        let (mut r, _) = rig(1, false);
        r.input.process(&[0.5f32; 64], CallbackTiming::default());
        assert_eq!(r.shared.capture_pending(), 0, "idle");
        assert!(r.shared.arm_capture());
        r.input.process(&[0.5f32; 64], CallbackTiming::default());
        assert_eq!(r.shared.capture_state(), CaptureState::Armed);
        assert_eq!(
            r.shared.capture_pending(),
            0,
            "armed but the transport is stopped"
        );

        r.telemetry.playing.store(true, Ordering::Relaxed);
        r.telemetry.set_beat(3.5);
        r.input.process(&[0.25f32; 64], CallbackTiming::default());
        assert_eq!(r.shared.capture_state(), CaptureState::Capturing);
        assert_eq!(r.shared.captured_frames(), 64);
        let start = r.shared.capture_start().expect("snapshot");
        assert_eq!(start.playhead_beat, 3.5);
        // With no host timing, one buffer of input latency is assumed.
        assert!((start.input_latency_s - 64.0 / 48_000.0).abs() < 1e-12);

        r.shared.end_capture();
        r.input.process(&[0.25f32; 64], CallbackTiming::default());
        assert_eq!(r.shared.captured_frames(), 64, "idle again");
    }

    #[test]
    fn the_chosen_channels_are_recorded_as_a_stereo_pair() {
        let (mut r, _) = rig(4, false);
        r.telemetry.playing.store(true, Ordering::Relaxed);
        assert!(r.shared.arm_capture());
        // Four channels per frame: 1, 2, 3, 4 (scaled), then 5, 6, 7, 8.
        let data: Vec<f32> = (0..2)
            .flat_map(|f| (0..4).map(move |c| (f * 4 + c + 1) as f32 * 0.1))
            .collect();

        r.shared.set_channels(InputChannels::Mono(2));
        r.input.process(&data, CallbackTiming::default());
        r.shared.set_channels(InputChannels::Stereo(3, 0));
        r.input.process(&data, CallbackTiming::default());
        let got = drain(&r.shared);
        let want = [
            0.3, 0.3, 0.7, 0.7, // mono channel 2 on both sides
            0.4, 0.1, 0.8, 0.5, // stereo 3 then 0
        ];
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-6, "{got:?}");
        }
        // The meter saw the loudest selected sample on each side.
        let (pl, pr) = r.shared.take_peak();
        assert!(
            (pl - 0.8).abs() < 1e-6 && (pr - 0.7).abs() < 1e-6,
            "{pl} {pr}"
        );
        assert_eq!(r.shared.take_peak(), (0.0, 0.0), "reading clears the peak");
    }

    #[test]
    fn integer_devices_and_junk_samples_are_converted_safely() {
        let (mut r, _) = rig(1, false);
        r.telemetry.playing.store(true, Ordering::Relaxed);
        r.shared.arm_capture();
        r.input
            .process(&[i16::MAX, i16::MIN, 0, 16384], CallbackTiming::default());
        r.input
            .process(&[f32::NAN, f32::INFINITY, -0.5], CallbackTiming::default());
        let got = drain(&r.shared);
        assert_eq!(got.len(), 14);
        assert!((got[0] - 1.0).abs() < 1e-3 && (got[2] + 1.0).abs() < 1e-3);
        assert!((got[6] - 0.5).abs() < 1e-3);
        assert_eq!(&got[8..14], &[0.0, 0.0, 0.0, 0.0, -0.5, -0.5]);
    }

    #[test]
    fn the_ring_wraps_around_without_losing_or_reordering_frames() {
        let (mut r, _) = rig(1, false);
        r.telemetry.playing.store(true, Ordering::Relaxed);
        r.shared.arm_capture();
        // The ring holds 96000 frames; push and drain several times that,
        // with odd callback sizes, so the indices wrap many times.
        let mut next = 0u32;
        let mut seen = 0u32;
        let mut buf = vec![0.0f32; 5000];
        for round in 0..400 {
            let n = [333usize, 1024, 97, 4000][round % 4];
            let data: Vec<f32> = (0..n).map(|i| (next + i as u32) as f32).collect();
            next += n as u32;
            r.input.process(&data, CallbackTiming::default());
            if round % 3 != 2 {
                loop {
                    let got = r.shared.drain_capture(&mut buf);
                    if got == 0 {
                        break;
                    }
                    for pair in buf[..got].as_chunks::<2>().0 {
                        assert_eq!((pair[0], pair[1]), (seen as f32, seen as f32));
                        seen += 1;
                    }
                }
            }
        }
        assert!(next > 96_000 * 4, "wrapped the ring several times: {next}");
        assert_eq!(r.shared.dropped_frames(), 0);
        for pair in drain(&r.shared).as_chunks::<2>().0 {
            assert_eq!(pair[0], seen as f32);
            seen += 1;
        }
        assert_eq!(seen, next, "every frame arrived once, in order");
    }

    #[test]
    fn a_full_ring_drops_frames_and_pads_the_take_with_silence() {
        let (mut r, _) = rig(1, false);
        r.telemetry.playing.store(true, Ordering::Relaxed);
        r.shared.arm_capture();
        let cap = 96_000usize;
        // Fill the ring exactly, then overflow it by 1000 frames.
        r.input
            .process(&vec![1.0f32; cap], CallbackTiming::default());
        r.input
            .process(&vec![2.0f32; 1000], CallbackTiming::default());
        assert_eq!(r.shared.dropped_frames(), 1000);
        // The writer catches up; later audio is preceded by 1000 silent frames
        // so everything after the hole stays at the right time.
        let mut head = vec![0.0f32; 2 * 50_000];
        let n = r.shared.drain_capture(&mut head);
        assert_eq!(n, head.len());
        r.input
            .process(&vec![3.0f32; 100], CallbackTiming::default());
        let rest = drain(&r.shared);
        let frames = rest.len() / 2;
        assert_eq!(frames, (cap - 50_000) + 1000 + 100);
        let (ones, tail) = rest.split_at(2 * (cap - 50_000));
        assert!(ones.iter().all(|x| *x == 1.0));
        let (silence, threes) = tail.split_at(2 * 1000);
        assert!(silence.iter().all(|x| *x == 0.0), "padding");
        assert!(threes.iter().all(|x| *x == 3.0));
    }

    #[test]
    fn monitoring_waits_for_a_prefill_then_plays_the_input() {
        let (mut r, mon) = rig(1, true);
        let mut mon = mon.unwrap();
        r.shared.set_monitoring(true, 0.5);
        let block = 128;
        let mut l = vec![0.0f32; block];
        let mut rt = vec![0.0f32; block];

        // One input block is not enough to start (prefill is output + input).
        r.input
            .process(&vec![0.8f32; block], CallbackTiming::default());
        mon.mix_into(&r.shared, &mut l, &mut rt);
        assert!(l.iter().all(|x| *x == 0.0), "still priming");
        assert_eq!(r.shared.monitor_stats().underruns, 0);

        r.input
            .process(&vec![0.8f32; block], CallbackTiming::default());
        mon.mix_into(&r.shared, &mut l, &mut rt);
        // Output reaches the target gain after the first block's ramp.
        assert!(l[block - 1] > 0.0 && l[block - 1] <= 0.4 + 1e-6);
        l.fill(0.0);
        rt.fill(0.0);
        r.input
            .process(&vec![0.8f32; block], CallbackTiming::default());
        mon.mix_into(&r.shared, &mut l, &mut rt);
        assert!(l.iter().all(|x| (*x - 0.4).abs() < 1e-6), "gain 0.5 of 0.8");
        assert_eq!(l, rt, "mono input is centred");
    }

    #[test]
    fn monitoring_reports_an_underrun_and_recovers() {
        let (mut r, mon) = rig(1, true);
        let mut mon = mon.unwrap();
        r.shared.set_monitoring(true, 1.0);
        let block = 64;
        let mut l = vec![0.0f32; block];
        let mut rt = vec![0.0f32; block];
        for _ in 0..4 {
            r.input
                .process(&vec![0.5f32; block], CallbackTiming::default());
        }
        mon.mix_into(&r.shared, &mut l, &mut rt);
        assert!(l[block - 1] > 0.0, "playing");
        // The input stops delivering; the output keeps asking.
        for _ in 0..8 {
            l.fill(0.0);
            mon.mix_into(&r.shared, &mut l, &mut rt);
        }
        assert_eq!(
            r.shared.monitor_stats().underruns,
            1,
            "one run dry, then priming"
        );
        assert!(l.iter().all(|x| *x == 0.0), "silence while starved");
        // Input returns: playback resumes after the prefill is rebuilt.
        for _ in 0..4 {
            r.input
                .process(&vec![0.5f32; block], CallbackTiming::default());
        }
        l.fill(0.0);
        mon.mix_into(&r.shared, &mut l, &mut rt);
        assert!(l.iter().any(|x| *x > 0.0));
    }

    #[test]
    fn a_stalled_output_never_lets_the_fifo_grow_without_bound() {
        let (mut r, mon) = rig(1, true);
        let mut mon = mon.unwrap();
        r.shared.set_monitoring(true, 1.0);
        for _ in 0..200 {
            r.input
                .process(&vec![0.1f32; 256], CallbackTiming::default());
        }
        let mut l = vec![0.0f32; 256];
        let mut rt = vec![0.0f32; 256];
        mon.mix_into(&r.shared, &mut l, &mut rt);
        let stats = r.shared.monitor_stats();
        assert!(
            stats.dropped_frames > 0,
            "old frames were discarded: {stats:?}"
        );
        assert!(
            mon.rx.slots() / 2 <= 2 * 256 + 2 * 256,
            "latency stays bounded"
        );
    }

    #[test]
    fn switching_monitoring_off_discards_what_is_queued() {
        let (mut r, mon) = rig(1, true);
        let mut mon = mon.unwrap();
        r.shared.set_monitoring(true, 1.0);
        for _ in 0..3 {
            r.input
                .process(&vec![0.9f32; 128], CallbackTiming::default());
        }
        r.shared.set_monitoring(false, 1.0);
        let mut l = vec![0.0f32; 128];
        let mut rt = vec![0.0f32; 128];
        mon.mix_into(&r.shared, &mut l, &mut rt);
        assert!(l.iter().all(|x| *x == 0.0));
        assert_eq!(mon.rx.slots(), 0);
        r.input
            .process(&vec![0.9f32; 128], CallbackTiming::default());
        assert_eq!(mon.rx.slots(), 0, "not fed while off");
    }

    #[test]
    fn the_input_remembers_whether_it_ever_heard_anything() {
        let (mut r, _) = rig(2, false);
        assert!(!r.shared.heard_signal());
        r.input.process(&[0.0f32; 512], CallbackTiming::default());
        assert!(!r.shared.heard_signal(), "digital silence is not signal");
        // Channel 0 of the second frame (the selection is mono channel 0).
        r.input
            .process(&[0.0f32, 0.0, 1e-6, 0.0], CallbackTiming::default());
        assert!(r.shared.heard_signal());
    }

    #[test]
    fn host_timing_sets_the_latency_report() {
        let (mut r, _) = rig(1, false);
        r.shared.note_output_timing(5_000_000, 10_000_000, 256);
        r.input.process(
            &[0.0f32; 256],
            CallbackTiming {
                callback_ns: 7_000_000,
                capture_ns: 2_000_000,
            },
        );
        let lat = r.shared.latency();
        assert!((lat.input_s - 0.005).abs() < 1e-9);
        assert!((lat.output_s - 0.010).abs() < 1e-9);
    }

    #[test]
    fn the_take_start_uses_comparable_callback_clocks_and_ignores_others() {
        let (mut r, _) = rig(1, false);
        r.telemetry.playing.store(true, Ordering::Relaxed);
        r.telemetry.set_beat(8.0);
        // Output callback 1 ms before the input callback, 256 frames long.
        r.shared.note_output_timing(9_000_000, 10_000_000, 256);
        r.shared.arm_capture();
        r.input.process(
            &[0.0f32; 128],
            CallbackTiming {
                callback_ns: 10_000_000,
                capture_ns: 6_000_000,
            },
        );
        let s = r.shared.capture_start().unwrap();
        assert!((s.output_age_s.unwrap() - 0.001).abs() < 1e-9);
        assert!((s.output_lead_s - 0.010).abs() < 1e-9);
        assert!((s.output_block_s - 256.0 / 48_000.0).abs() < 1e-12);
        assert!((s.input_latency_s - 0.004).abs() < 1e-9);

        // Clocks that are seconds apart cannot be compared.
        r.shared.arm_capture();
        r.shared.note_output_timing(1, 10_000_000, 256);
        r.input.process(
            &[0.0f32; 128],
            CallbackTiming {
                callback_ns: 50_000_000_000,
                capture_ns: 0,
            },
        );
        assert_eq!(r.shared.capture_start().unwrap().output_age_s, None);
    }
}
