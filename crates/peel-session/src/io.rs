//! Audio device selection and live MIDI input for the session.
//!
//! # Audio
//!
//! [`Session::restart_audio`] swaps the whole engine for one on another
//! output device or buffer size. The document is untouched; the new engine is
//! brought to the old one's state: every device is re-created, prepared at the
//! new rate and installed in the slot it had, samples are resent, the plan is
//! recompiled and sent, and the metronome, playhead and live target carry over.
//!
//! # MIDI
//!
//! `peel-midi-io` parses hardware MIDI on the ports' own threads and calls
//! [`MidiBridge::handle`] right there. The bridge pushes the note into the
//! engine's live queue, so a key press reaches the audio thread without
//! waiting for the UI. The session's only per-frame job is [`Session::poll_midi`]:
//! it points the live target at the selected channel's instrument, rescans
//! the ports every couple of seconds and drains the recorded notes.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use peel_document::Id;
use peel_engine::{EngineCommand, EngineConfig, EngineHandle, LiveInput, RecordedNote};
use peel_midi_io::{MidiEvent, MidiInput};

pub use peel_engine::{LiveTiming, OutputDeviceInfo, list_output_devices};
pub use peel_midi_io::{MidiPortInfo, PortSelection};

use crate::{SampleLoader, Session};

/// How often [`Session::poll_midi`] asks for a port rescan.
const MIDI_SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// Recorded notes kept when nobody drains them; the oldest are dropped.
const MAX_PENDING_RECORDED: usize = 16_384;

/// A note played on a MIDI keyboard, with where the transport was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordedMidi {
    /// The channel that was the live target, if it still exists.
    pub channel: Option<Id>,
    pub key: u8,
    /// 0..=1; zero for a note-off.
    pub velocity: f32,
    pub on: bool,
    /// Playhead (quarter notes) from telemetry when the note arrived. It is
    /// the engine's position, which runs ahead of what the player hears by
    /// the output latency (about one or two audio buffers); subtract that
    /// when recording. Telemetry updates once per audio block.
    pub beat: f64,
    /// The transport was running.
    pub playing: bool,
    /// The sustain pedal was down. A note-off with this set does not end the
    /// sound until the pedal lifts.
    pub sustain: bool,
    /// Arrival time in nanoseconds on a monotonic clock, for ordering and
    /// measuring note lengths across audio stalls.
    pub time_ns: u64,
}

/// Hands MIDI events from the port threads to the current engine.
///
/// The engine behind it changes when audio restarts, so the live input is
/// swapped under a mutex that only MIDI threads and `restart_audio` take.
#[derive(Default)]
pub(crate) struct MidiBridge {
    live: Mutex<Option<LiveInput>>,
}

impl MidiBridge {
    pub(crate) fn set(&self, live: Option<LiveInput>) {
        *self.live.lock().unwrap_or_else(PoisonError::into_inner) = live;
    }

    /// Runs on the MIDI callback thread: never blocks on the UI.
    pub(crate) fn handle(&self, event: &MidiEvent) {
        let guard = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(live) = guard.as_ref() else {
            return;
        };
        match *event {
            MidiEvent::NoteOn { key, velocity, .. } => {
                live.note_to_target(key, f32::from(velocity) / 127.0, true);
            }
            MidiEvent::NoteOff { key, .. } => {
                live.note_to_target(key, 0.0, false);
            }
            MidiEvent::Sustain { down, .. } => {
                live.sustain(down);
            }
            MidiEvent::AllNotesOff { .. } | MidiEvent::AllSoundOff { .. } => {
                live.all_notes_off_target();
            }
            // Pitch bend, mod wheel and other controllers are parsed but not
            // mapped to anything yet.
            _ => {}
        }
    }
}

/// Session state for audio device selection and MIDI input.
pub(crate) struct IoState {
    /// Restart on the silent clock instead of a device (tests, tools).
    silent: bool,
    /// What the running engine was started with.
    config: EngineConfig,
    midi: Option<MidiInput>,
    midi_enabled: bool,
    selection: PortSelection,
    bridge: Arc<MidiBridge>,
    timing: LiveTiming,
    live_channel: Option<Id>,
    last_scan: Option<Instant>,
    known_inputs: Vec<MidiPortInfo>,
    recorded: Vec<RecordedNote>,
}

impl IoState {
    pub(crate) fn new(config: EngineConfig, silent: bool) -> IoState {
        IoState {
            silent,
            config,
            midi: None,
            midi_enabled: !silent,
            selection: PortSelection::All,
            bridge: Arc::new(MidiBridge::default()),
            timing: LiveTiming::default(),
            live_channel: None,
            last_scan: None,
            known_inputs: Vec::new(),
            recorded: Vec::new(),
        }
    }
}

fn start_engine(config: &EngineConfig, silent: bool) -> EngineHandle {
    let started = if silent {
        EngineHandle::start_null(config.clone())
    } else {
        EngineHandle::start(config.clone())
    };
    match started {
        Ok(engine) => engine,
        Err(e) => {
            log::error!("audio output unavailable: {e}; continuing without sound");
            EngineHandle::start_null(config.clone()).expect("null audio engine")
        }
    }
}

impl Session {
    /// Connects the engine's live input to the MIDI bridge and opens the
    /// hardware ports. Called once when the session is built.
    pub(crate) fn init_io(&mut self) {
        self.io.bridge.set(self.engine.take_live_input());
        self.engine.set_live_timing(self.io.timing);
        if self.io.midi_enabled {
            self.start_midi();
        }
    }

    // ------------------------------------------------------------------
    // Audio device
    // ------------------------------------------------------------------

    /// The output devices the system offers. Enumeration queries every
    /// device (tens of milliseconds), so call it when a settings panel opens.
    pub fn audio_devices(&self) -> Vec<OutputDeviceInfo> {
        list_output_devices()
    }

    /// Name of the output device in use, or "No audio device".
    pub fn audio_device_name(&self) -> &str {
        self.engine.device_name()
    }

    /// The device last asked for, which may differ from the one in use if it
    /// was missing (`None` is the system default).
    pub fn requested_audio_device(&self) -> Option<&str> {
        self.io.config.device.as_deref()
    }

    /// Why the requested device is not the one in use, if it is not.
    pub fn audio_warning(&self) -> Option<&str> {
        self.engine.device_warning()
    }

    /// Frames per device callback as negotiated; 0 if the backend does not
    /// say.
    pub fn buffer_frames(&self) -> u32 {
        self.engine.buffer_frames()
    }

    /// False after the device disappeared or the stream failed; offer
    /// [`restart_audio`](Session::restart_audio).
    pub fn audio_is_healthy(&self) -> bool {
        self.engine.is_healthy()
    }

    /// Restarts audio on `device` (`None` is the system default) with a
    /// device buffer of `buffer_frames` (`None` lets the device choose).
    ///
    /// Playback stops. The project and every sound survive: devices are
    /// re-created and installed in their old slots at the new sample rate,
    /// samples are resent (re-decoded if the rate changed), and the plan is
    /// recompiled and sent. A device that is missing falls back to the
    /// default and a notice says so. If the new engine cannot open any real
    /// device while the old one could, the old settings are restored and the
    /// error says why.
    pub fn restart_audio(
        &mut self,
        device: Option<String>,
        buffer_frames: Option<u32>,
    ) -> Result<(), String> {
        let wanted = EngineConfig {
            device: device.filter(|d| !d.trim().is_empty()),
            buffer_frames: buffer_frames.filter(|&n| n > 0),
            ..self.io.config.clone()
        };
        self.restart_with(wanted)
    }

    pub(crate) fn restart_with(&mut self, wanted: EngineConfig) -> Result<(), String> {
        let previous = self.io.config.clone();
        let was_null = self.engine.is_null_backend();
        let old_rate = self.engine.sample_rate();
        let position = if self.playing {
            self.play_start
        } else {
            self.position()
        };
        let metronome = self.metronome;

        // Quiet the old engine, keep what the player recorded, and give its
        // device back before another engine opens it.
        self.stop();
        self.engine.collect_garbage();
        self.engine.drain_recorded(&mut self.io.recorded);
        self.io.bridge.set(None);
        self.engine.stop_audio();

        let mut engine = start_engine(&wanted, self.io.silent);
        let mut config = wanted;
        let mut failure = None;
        if engine.is_null_backend() && !was_null && !self.io.silent {
            // Nothing opened. Put the old setup back instead of leaving the
            // user without sound.
            failure = Some(
                engine
                    .device_warning()
                    .unwrap_or("no usable audio device")
                    .to_string(),
            );
            engine = start_engine(&previous, false);
            config = previous;
        }
        self.engine = engine;
        self.io.config = config;
        self.reinstall_engine(old_rate, position, metronome);

        if let Some(reason) = failure {
            let message = format!("Could not switch audio output: {reason}");
            self.notify(message.clone(), true);
            return Err(message);
        }
        if let Some(warning) = self.engine.device_warning().map(str::to_string) {
            self.notify(warning, true);
        }
        log::info!(
            "audio restarted: {} at {} Hz, buffer {}",
            self.engine.device_name(),
            self.engine.sample_rate(),
            self.engine.buffer_frames()
        );
        Ok(())
    }

    /// Brings a fresh engine to the state of the one it replaced. Extend
    /// this when something else lives in the engine (for example audio clip
    /// buffers): the new engine starts completely empty.
    fn reinstall_engine(&mut self, old_rate: f32, position: f64, metronome: bool) {
        let rate = self.engine.sample_rate();
        let block = self.engine.max_block();
        if (rate - old_rate).abs() > 0.5 {
            // Cached samples were resampled for the old rate; decode again.
            self.samples = SampleLoader::new();
        }

        // Plugins are rebuilt from their current state.
        self.capture_plugin_states();
        let project = self.doc.project().clone();

        // Every device in the slot it had, so the plan's indices stay valid.
        let installed: Vec<_> = self
            .devices
            .iter()
            .map(|(id, d)| (*id, d.slot, d.type_id.clone(), d.params.clone(), d.sample))
            .collect();
        for (id, slot, type_id, params, sample) in installed {
            let device = match crate::find_device(&project, id) {
                Some(inst) if type_id.starts_with("clap:") => self.create_device(inst),
                _ => peel_devices::create(&type_id),
            };
            let Some(mut device) = device else {
                continue;
            };
            let desc = device.descriptor();
            for (key, value) in &params {
                if let Some(p) = desc.param_by_key(key) {
                    device.set_param(p.id, *value);
                }
            }
            device.prepare(rate, block);
            if self
                .engine
                .send(EngineCommand::InstallDevice { slot, device })
                .is_err()
            {
                log::warn!("engine queue full while reinstalling {type_id}");
            }
            if sample.is_some() {
                self.request_sample(sample, slot);
            }
        }

        // The plan carries gains, pans, mutes and routing.
        self.plan = None;
        self.synced = None;
        self.sync();

        let _ = self
            .engine
            .send(EngineCommand::Metronome { enabled: metronome });
        let _ = self.engine.send(EngineCommand::Seek { beat: position });

        self.io.bridge.set(self.engine.take_live_input());
        self.engine.set_live_timing(self.io.timing);
        self.refresh_live_target();
    }

    // ------------------------------------------------------------------
    // Live target
    // ------------------------------------------------------------------

    /// Chooses the channel MIDI keyboard notes play: normally the one
    /// selected in the channel rack. `None` ignores incoming notes.
    pub fn set_live_target(&mut self, channel: Option<Id>) {
        self.io.live_channel = channel;
        self.refresh_live_target();
    }

    pub fn live_target(&self) -> Option<Id> {
        self.io.live_channel
    }

    /// Points the engine at the target channel's current instrument slot.
    fn refresh_live_target(&self) {
        let slot = self
            .io
            .live_channel
            .and_then(|id| self.doc.project().channel(id))
            .and_then(|channel| self.devices.get(&channel.device.id))
            .map(|d| d.slot);
        let target = self.engine.live_target();
        if target.get() != slot {
            target.set(slot);
        }
    }

    /// How live notes are placed inside an audio block. `Immediate` (the
    /// default) is the lowest latency; `Spread` removes timing jitter at the
    /// cost of one more buffer of latency.
    pub fn set_midi_timing(&mut self, timing: LiveTiming) {
        self.io.timing = timing;
        self.engine.set_live_timing(timing);
    }

    pub fn midi_timing(&self) -> LiveTiming {
        self.io.timing
    }

    // ------------------------------------------------------------------
    // MIDI ports
    // ------------------------------------------------------------------

    /// Names of the MIDI inputs that are open now.
    pub fn midi_inputs(&self) -> Vec<String> {
        self.io
            .midi
            .as_ref()
            .map(|m| m.connected().into_iter().map(|p| p.name).collect())
            .unwrap_or_default()
    }

    /// Every MIDI input the system offers, open or not (as of the last scan).
    pub fn midi_available_inputs(&self) -> Vec<MidiPortInfo> {
        self.io.midi.as_ref().map(|m| m.ports()).unwrap_or_default()
    }

    /// Opens every input (the default) or a single one.
    pub fn select_midi_input(&mut self, selection: PortSelection) {
        self.io.selection = selection.clone();
        if let Some(midi) = &self.io.midi {
            midi.select(selection);
        }
    }

    /// Turns hardware MIDI input on or off. It is on for [`Session::new`]
    /// and off for [`Session::new_silent`].
    pub fn set_midi_enabled(&mut self, on: bool) {
        self.io.midi_enabled = on;
        match (on, self.io.midi.is_some()) {
            (true, false) => self.start_midi(),
            (false, true) => {
                self.io.midi = None;
                self.io.known_inputs.clear();
            }
            _ => {}
        }
    }

    pub fn midi_enabled(&self) -> bool {
        self.io.midi_enabled
    }

    fn start_midi(&mut self) {
        let bridge = self.io.bridge.clone();
        self.io.midi = Some(MidiInput::with_selection(
            move |event| bridge.handle(event),
            self.io.selection.clone(),
        ));
        self.io.last_scan = Some(Instant::now());
    }

    /// Call once per UI frame. Cheap: it points the live target at the
    /// selected channel's instrument, drains recorded notes so the ring
    /// never fills, and every couple of seconds asks for a port rescan
    /// (the work happens on the MIDI port thread, not here).
    pub fn poll_midi(&mut self) {
        self.refresh_live_target();
        self.engine.drain_recorded(&mut self.io.recorded);
        let excess = self.io.recorded.len().saturating_sub(MAX_PENDING_RECORDED);
        if excess > 0 {
            self.io.recorded.drain(..excess);
        }

        let due = self
            .io
            .last_scan
            .is_none_or(|t| t.elapsed() >= MIDI_SCAN_INTERVAL);
        let Some(midi) = &self.io.midi else {
            return;
        };
        if due {
            midi.poll_ports();
            self.io.last_scan = Some(Instant::now());
        }
        let now = midi.connected();
        if now != self.io.known_inputs {
            let joined: Vec<String> = now
                .iter()
                .filter(|p| !self.io.known_inputs.contains(p))
                .map(|p| p.name.clone())
                .collect();
            let left: Vec<String> = self
                .io
                .known_inputs
                .iter()
                .filter(|p| !now.contains(p))
                .map(|p| p.name.clone())
                .collect();
            self.io.known_inputs = now;
            if !joined.is_empty() {
                self.notify(
                    format!("MIDI input connected: {}", joined.join(", ")),
                    false,
                );
            } else if !left.is_empty() {
                self.notify(
                    format!("MIDI input disconnected: {}", left.join(", ")),
                    false,
                );
            }
        }
    }

    /// Notes that arrived on the MIDI keyboard since the last call, oldest
    /// first, for recording into a pattern. Also returns notes recorded
    /// before an audio restart.
    pub fn take_recorded_midi(&mut self) -> Vec<RecordedMidi> {
        self.engine.drain_recorded(&mut self.io.recorded);
        let notes = std::mem::take(&mut self.io.recorded);
        notes
            .into_iter()
            .map(|n| RecordedMidi {
                channel: self.channel_for_slot(n.slot),
                key: n.key,
                velocity: n.velocity,
                on: n.on,
                beat: n.beat,
                playing: n.playing,
                sustain: n.sustain,
                time_ns: n.time_ns,
            })
            .collect()
    }

    fn channel_for_slot(&self, slot: peel_engine::Slot) -> Option<Id> {
        self.doc
            .project()
            .channels
            .iter()
            .find(|c| {
                self.devices
                    .get(&c.device.id)
                    .is_some_and(|d| d.slot == slot)
            })
            .map(|c| c.id)
    }

    /// Notes (not other messages) played on MIDI keyboards so far; compare
    /// with the previous value to flash an input light.
    pub fn midi_activity(&self) -> u64 {
        self.engine.live_stats().note_ons()
    }

    /// All MIDI messages received on the open ports so far, including
    /// controllers the application does not use.
    pub fn midi_message_count(&self) -> u64 {
        self.io.midi.as_ref().map_or(0, |m| m.event_count())
    }
}

#[cfg(test)]
mod tests;
