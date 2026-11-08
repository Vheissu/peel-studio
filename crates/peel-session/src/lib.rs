//! The control-side session.
//!
//! Owns the editable [`Document`], keeps native devices installed in engine
//! slots, compiles render plans when the document changes, and turns
//! realtime-friendly edits (gains, parameters) into direct engine commands
//! so a fader move never rebuilds the plan.

mod audio;
mod compile;
mod export;
mod io;
mod plugins;
mod samples;
pub mod template;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use peel_document::{AudioAsset, DeviceInstance, Document, Id, Project};
use peel_dsp::SampleBuffer;
use peel_dsp::util::db_to_gain;
use peel_engine::{EngineCommand, EngineConfig, EngineHandle, RenderPlan, Slot, Telemetry};

pub use compile::{
    CompileContext, PlanIndex, PlayMode, clip_source_start_seconds, compile, same_structure,
};
pub use export::{ExportJob, ExportSettings, render as render_to_wav};
pub use io::{
    LiveTiming, MidiPortInfo, OutputDeviceInfo, PortSelection, RecordedMidi, list_output_devices,
};
pub use plugins::{PluginRegistry, type_id_for as plugin_type_id};
pub use samples::SampleLoader;

struct DeviceSlot {
    slot: Slot,
    type_id: String,
    params: BTreeMap<String, f32>,
    sample: Option<Id>,
}

/// A short user-facing notice (save result, missing device, errors).
#[derive(Clone, Debug)]
pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

pub struct Session {
    pub doc: Document,
    pub path: Option<PathBuf>,
    engine: EngineHandle,
    devices: HashMap<Id, DeviceSlot>,
    free_slots: Vec<Slot>,
    next_slot: Slot,
    meters: HashMap<Id, u16>,
    next_meter: u16,
    plan: Option<RenderPlan>,
    index: PlanIndex,
    /// Document revision, mode and audio generation the engine was last
    /// synced to.
    synced: Option<(u64, PlayMode, u64)>,
    audio: audio::AudioState,
    mode: PlayMode,
    selected_pattern: Id,
    playing: bool,
    play_start: f64,
    metronome: bool,
    samples: SampleLoader,
    plugins: PluginRegistry,
    notice: Option<Notice>,
    missing: Vec<String>,
    /// Audio device selection and live MIDI input (see `io.rs`).
    io: io::IoState,
}

impl Session {
    /// Starts the audio engine (falling back to a silent clock when no
    /// output device is available) and loads `project`.
    pub fn new(project: Project) -> Session {
        Session::new_with_audio(project, None, EngineConfig::default().buffer_frames)
    }

    /// [`Session::new`] on a chosen output device and buffer size (`None`
    /// device is the system default; see [`list_output_devices`]). A device
    /// that is missing falls back to the default with a warning.
    pub fn new_with_audio(
        project: Project,
        device: Option<String>,
        buffer_frames: Option<u32>,
    ) -> Session {
        let config = EngineConfig {
            device,
            buffer_frames,
            ..EngineConfig::default()
        };
        let engine = match EngineHandle::start(config.clone()) {
            Ok(e) => e,
            Err(e) => {
                log::error!("audio output unavailable: {e}; continuing without sound");
                EngineHandle::start_null(config.clone()).expect("null audio engine")
            }
        };
        Session::with_engine(
            project,
            engine,
            io::IoState::new(config, false),
            PluginRegistry::start_scan(),
        )
    }

    /// A session on the silent software clock (tests, headless tools). It
    /// never opens hardware MIDI ports unless asked to.
    pub fn new_silent(project: Project) -> Session {
        let config = EngineConfig::default();
        let engine = EngineHandle::start_null(config.clone()).expect("null audio engine");
        Session::with_engine(
            project,
            engine,
            io::IoState::new(config, true),
            PluginRegistry::disabled(),
        )
    }

    fn with_engine(
        project: Project,
        engine: EngineHandle,
        io: io::IoState,
        plugins: PluginRegistry,
    ) -> Session {
        log::info!(
            "audio: {} at {} Hz, block {}",
            engine.device_name(),
            engine.sample_rate(),
            engine.max_block()
        );
        let mode = PlayMode::Pattern(project.patterns.first().map_or(Id::NIL, |p| p.id));
        let mut s = Session {
            doc: Document::new(project),
            path: None,
            engine,
            devices: HashMap::new(),
            free_slots: Vec::new(),
            next_slot: 0,
            meters: HashMap::new(),
            next_meter: 0,
            plan: None,
            index: PlanIndex::default(),
            synced: None,
            audio: audio::AudioState::default(),
            mode,
            selected_pattern: match mode {
                PlayMode::Pattern(p) => p,
                PlayMode::Song => Id::NIL,
            },
            playing: false,
            play_start: 0.0,
            metronome: false,
            samples: SampleLoader::new(),
            plugins,
            notice: None,
            missing: Vec::new(),
            io,
        };
        s.init_io();
        s.sync();
        s
    }

    pub fn project(&self) -> &Project {
        self.doc.project()
    }

    // ---- per-frame upkeep ----

    /// Call once per UI frame: frees retired engine objects, installs
    /// loaded samples and syncs the engine with the document.
    pub fn tick(&mut self) {
        self.engine.collect_garbage();
        if self.plugins.poll() {
            // Plugins referenced by the project can now be created.
            self.synced = None;
        }
        self.plugins.idle();
        for (asset, result) in self.samples.poll() {
            match result {
                Ok(buf) => {
                    self.install_sample(asset, buf);
                    self.audio_loaded(asset);
                }
                Err(e) => {
                    self.audio_failed(asset);
                    self.notify(format!("Could not load sample: {e}"), true);
                }
            }
        }
        if let Some((buf, gain)) = self.samples.poll_preview() {
            let _ = self.engine.send(EngineCommand::Preview {
                sample: Some(buf),
                gain,
            });
        }
        self.sync();
    }

    fn sync(&mut self) {
        let key = (self.doc.revision(), self.mode, self.audio.generation);
        if self.synced == Some(key) {
            return;
        }
        self.sync_devices();
        self.sync_plan();
        self.synced = Some(key);
    }

    fn all_devices(p: &Project) -> Vec<&DeviceInstance> {
        p.channels
            .iter()
            .map(|c| &c.device)
            .chain(p.mixer.tracks.iter().flat_map(|t| t.inserts.iter()))
            .collect()
    }

    fn sync_devices(&mut self) {
        let project = self.doc.project().clone();
        let wanted = Self::all_devices(&project);
        // Remove devices no longer in the document.
        let keep: std::collections::HashSet<Id> = wanted.iter().map(|d| d.id).collect();
        let stale: Vec<Id> = self
            .devices
            .keys()
            .filter(|id| !keep.contains(id))
            .copied()
            .collect();
        for id in stale {
            if let Some(d) = self.devices.remove(&id) {
                let _ = self
                    .engine
                    .send(EngineCommand::RemoveDevice { slot: d.slot });
                self.free_slots.push(d.slot);
                self.plugins.forget(id);
            }
        }
        self.missing.clear();
        for inst in wanted {
            match self.devices.get_mut(&inst.id) {
                Some(slot) if slot.type_id == inst.type_id => {
                    // Parameter changes.
                    let desc = peel_devices::descriptor(&inst.type_id);
                    for (k, v) in &inst.params {
                        if slot.params.get(k) != Some(v) {
                            if let Some(pd) = desc.and_then(|d| d.param_by_key(k)) {
                                let _ = self.engine.send(EngineCommand::SetParam {
                                    slot: slot.slot,
                                    id: pd.id,
                                    value: *v,
                                });
                            }
                            slot.params.insert(k.clone(), *v);
                        }
                    }
                    if slot.sample != inst.sample {
                        slot.sample = inst.sample;
                        let s = slot.slot;
                        self.request_sample(inst.sample, s);
                    }
                }
                _ => self.install_device(inst, &project),
            }
        }
        for t in &project.mixer.tracks {
            if !self.meters.contains_key(&t.id) {
                self.meters.insert(t.id, self.next_meter);
                self.next_meter =
                    self.next_meter.wrapping_add(1) % peel_engine::telemetry::MAX_METERS as u16;
            }
        }
    }

    /// A native device, or a CLAP plugin restored from its saved state.
    pub(crate) fn create_device(
        &mut self,
        inst: &DeviceInstance,
    ) -> Option<Box<dyn peel_dsp::Device>> {
        if let Some(d) = peel_devices::create(&inst.type_id) {
            return Some(d);
        }
        if inst.type_id.starts_with("clap:") {
            let (rate, block) = (self.engine.sample_rate(), self.engine.max_block());
            match self.plugins.create(inst, rate, block) {
                Ok(d) => return Some(d),
                Err(e) => {
                    if !self.plugins.is_scanning() {
                        log::warn!("{e}");
                    }
                }
            }
        }
        None
    }

    /// Installed CLAP plugins (empty until the background scan finishes).
    pub fn plugins(&self) -> &[peel_host_clap::ClapPluginInfo] {
        self.plugins.plugins()
    }

    pub fn plugins_scanning(&self) -> bool {
        self.plugins.is_scanning()
    }

    /// Looks for newly installed plugins in the background.
    pub fn rescan_plugins(&mut self) {
        self.plugins.rescan();
    }

    /// Replaces the installed plugin set (tests and tools).
    pub fn set_plugins(&mut self, plugins: Vec<peel_host_clap::ClapPluginInfo>) {
        self.plugins.set(plugins);
        self.synced = None;
    }

    /// Stores every live plugin's state in the document (not an undo step).
    pub fn capture_plugin_states(&mut self) {
        let states = self.plugins.capture_states();
        if states.is_empty() {
            return;
        }
        self.doc.patch(|p| {
            for (id, state) in states {
                if let Some(d) = find_device_mut(p, id) {
                    d.state = Some(state);
                }
            }
        });
    }

    fn install_device(&mut self, inst: &DeviceInstance, project: &Project) {
        if let Some(old) = self.devices.remove(&inst.id) {
            let _ = self
                .engine
                .send(EngineCommand::RemoveDevice { slot: old.slot });
            self.free_slots.push(old.slot);
        }
        let Some(mut device) = self.create_device(inst) else {
            self.missing.push(inst.type_id.clone());
            return;
        };
        let desc = device.descriptor();
        for (k, v) in &inst.params {
            if let Some(p) = desc.param_by_key(k) {
                device.set_param(p.id, *v);
            }
        }
        device.prepare(self.engine.sample_rate(), self.engine.max_block());
        let slot = self.free_slots.pop().unwrap_or_else(|| {
            let s = self.next_slot;
            self.next_slot += 1;
            s
        });
        let _ = self
            .engine
            .send(EngineCommand::InstallDevice { slot, device });
        self.devices.insert(
            inst.id,
            DeviceSlot {
                slot,
                type_id: inst.type_id.clone(),
                params: inst.params.clone(),
                sample: inst.sample,
            },
        );
        if inst.sample.is_some() {
            let _ = project;
            self.request_sample(inst.sample, slot);
        }
    }

    fn request_sample(&mut self, asset: Option<Id>, slot: Slot) {
        match asset {
            None => {
                let _ = self
                    .engine
                    .send(EngineCommand::SetSample { slot, sample: None });
            }
            Some(id) => {
                if let Some(buf) = self.samples.cached(id) {
                    let _ = self.engine.send(EngineCommand::SetSample {
                        slot,
                        sample: Some(buf),
                    });
                } else if let Some(asset) = self.doc.project().asset(id).cloned() {
                    let path = self.asset_path(&asset);
                    self.samples.request(id, path, self.engine.sample_rate());
                }
            }
        }
    }

    fn install_sample(&mut self, asset: Id, buf: Arc<SampleBuffer>) {
        let p = self.doc.project();
        let users: Vec<Slot> = Self::all_devices(p)
            .into_iter()
            .filter(|d| d.sample == Some(asset))
            .filter_map(|d| self.devices.get(&d.id).map(|s| s.slot))
            .collect();
        for slot in users {
            let _ = self.engine.send(EngineCommand::SetSample {
                slot,
                sample: Some(buf.clone()),
            });
        }
    }

    /// Filesystem location of an asset.
    pub fn asset_path(&self, asset: &AudioAsset) -> PathBuf {
        if asset.managed
            && let Some(p) = &self.path
        {
            return peel_project::resolve_asset_path(p, asset);
        }
        PathBuf::from(&asset.path)
    }

    /// Cached decoded sample for an asset (for waveform drawing).
    pub fn sample(&self, asset: Id) -> Option<Arc<SampleBuffer>> {
        self.samples.cached(asset)
    }

    fn sync_plan(&mut self) {
        let slots: HashMap<Id, Slot> = self.devices.iter().map(|(id, d)| (*id, d.slot)).collect();
        // A pattern-mode selection that no longer exists falls back.
        if let PlayMode::Pattern(pid) = self.mode
            && self.doc.project().pattern(pid).is_none()
        {
            self.mode = PlayMode::Pattern(
                self.doc
                    .project()
                    .patterns
                    .first()
                    .map_or(Id::NIL, |p| p.id),
            );
        }
        let samples = self.clip_samples();
        let (plan, index) = compile(
            self.doc.project(),
            &CompileContext {
                mode: self.mode,
                slots: &slots,
                meters: &self.meters,
                samples: &samples,
            },
        );
        match &self.plan {
            Some(old) if same_structure(old, &plan) => {
                for (i, (a, b)) in old.channels.iter().zip(&plan.channels).enumerate() {
                    let channel = i as u16;
                    if a.gain != b.gain {
                        let _ = self.engine.send(EngineCommand::SetChannelGain {
                            channel,
                            gain: b.gain,
                        });
                    }
                    if a.pan != b.pan {
                        let _ = self.engine.send(EngineCommand::SetChannelPan {
                            channel,
                            pan: b.pan,
                        });
                    }
                    if a.muted != b.muted {
                        let _ = self.engine.send(EngineCommand::SetChannelMute {
                            channel,
                            muted: b.muted,
                        });
                    }
                }
                for (i, (a, b)) in old.tracks.iter().zip(&plan.tracks).enumerate() {
                    let track = i as u16;
                    if a.gain != b.gain {
                        let _ = self.engine.send(EngineCommand::SetTrackGain {
                            track,
                            gain: b.gain,
                        });
                    }
                    if a.pan != b.pan {
                        let _ = self
                            .engine
                            .send(EngineCommand::SetTrackPan { track, pan: b.pan });
                    }
                    if a.muted != b.muted {
                        let _ = self.engine.send(EngineCommand::SetTrackMute {
                            track,
                            muted: b.muted,
                        });
                    }
                    for (si, (x, y)) in a.sends.iter().zip(&b.sends).enumerate() {
                        if x.gain != y.gain {
                            let _ = self.engine.send(EngineCommand::SetSendGain {
                                track,
                                send: si as u16,
                                gain: y.gain,
                            });
                        }
                    }
                }
                self.plan = Some(plan);
            }
            _ => {
                let _ = self
                    .engine
                    .send(EngineCommand::SetPlan(Box::new(plan.clone())));
                self.plan = Some(plan);
            }
        }
        self.index = index;
    }

    // ---- notices ----

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        if error {
            log::warn!("{text}");
        }
        self.notice = Some(Notice {
            text,
            error,
            at: Instant::now(),
        });
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice
            .as_ref()
            .filter(|n| n.at.elapsed().as_secs_f32() < 4.0)
    }

    /// Device types referenced by the project that are not installed.
    pub fn missing_devices(&self) -> &[String] {
        &self.missing
    }

    // ---- transport ----

    pub fn mode(&self) -> PlayMode {
        self.mode
    }

    /// The pattern being edited (and played in pattern mode).
    pub fn selected_pattern(&self) -> Id {
        if self.doc.project().pattern(self.selected_pattern).is_some() {
            self.selected_pattern
        } else {
            self.doc
                .project()
                .patterns
                .first()
                .map_or(Id::NIL, |p| p.id)
        }
    }

    pub fn select_pattern(&mut self, id: Id) {
        self.selected_pattern = id;
        if let PlayMode::Pattern(_) = self.mode {
            self.mode = PlayMode::Pattern(id);
        }
    }

    pub fn set_mode(&mut self, mode: PlayMode) {
        if let PlayMode::Pattern(p) = mode {
            self.selected_pattern = p;
        }
        if self.mode != mode {
            let was_song = self.mode == PlayMode::Song;
            self.mode = mode;
            if was_song != (mode == PlayMode::Song) && self.playing {
                // Switching between pattern and song restarts from the top.
                self.sync();
                let _ = self.engine.send(EngineCommand::Seek { beat: 0.0 });
            }
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn play(&mut self) {
        self.sync();
        self.play_start = self.position();
        let _ = self.engine.send(EngineCommand::Play);
        self.playing = true;
    }

    /// Stops and returns to where playback started.
    pub fn stop(&mut self) {
        let back = if self.playing { self.play_start } else { 0.0 };
        let _ = self.engine.send(EngineCommand::Stop { return_to: back });
        self.playing = false;
    }

    pub fn toggle_play(&mut self) {
        if self.playing {
            self.stop()
        } else {
            self.play()
        }
    }

    pub fn seek(&mut self, beat: f64) {
        let _ = self.engine.send(EngineCommand::Seek {
            beat: beat.max(0.0),
        });
        self.play_start = beat.max(0.0);
    }

    pub fn panic(&mut self) {
        let _ = self.engine.send(EngineCommand::Panic);
    }

    /// Playhead in quarter notes.
    pub fn position(&self) -> f64 {
        self.engine.telemetry().beat()
    }

    pub fn metronome(&self) -> bool {
        self.metronome
    }

    pub fn set_metronome(&mut self, on: bool) {
        self.metronome = on;
        let _ = self.engine.send(EngineCommand::Metronome { enabled: on });
    }

    // ---- live input and audition ----

    /// Plays a note on a channel's instrument immediately.
    pub fn live_note(&mut self, channel: Id, key: u8, velocity: f32, on: bool) {
        let Some(ch) = self.doc.project().channel(channel) else {
            return;
        };
        if let Some(d) = self.devices.get(&ch.device.id) {
            let _ = self.engine.send(EngineCommand::LiveNote {
                slot: d.slot,
                key,
                velocity,
                on,
            });
        }
    }

    /// Auditions an audio file on the preview route.
    pub fn preview_file(&mut self, path: &Path) {
        self.samples
            .request_preview(path.to_path_buf(), self.engine.sample_rate(), 0.7);
    }

    pub fn stop_preview(&mut self) {
        let _ = self.engine.send(EngineCommand::Preview {
            sample: None,
            gain: 0.0,
        });
    }

    // ---- parameters ----

    /// Sets a device parameter as part of a gesture: the engine hears it
    /// immediately and the document records one undo step per gesture.
    pub fn set_param(&mut self, device: Id, key: &str, value: f32, gesture: u64) {
        let label = peel_devices::descriptor(self.device_type(device).unwrap_or_default().as_str())
            .and_then(|d| d.param_by_key(key))
            .map_or_else(
                || "Change parameter".to_string(),
                |p| format!("Change {}", p.name),
            );
        self.doc.edit_gesture(gesture, label, |p| {
            if let Some(d) = find_device_mut(p, device) {
                d.params.insert(key.to_string(), value);
            }
        });
        if let Some(slot) = self.devices.get_mut(&device) {
            if let Some(pd) =
                peel_devices::descriptor(&slot.type_id).and_then(|d| d.param_by_key(key))
            {
                let _ = self.engine.send(EngineCommand::SetParam {
                    slot: slot.slot,
                    id: pd.id,
                    value,
                });
            }
            slot.params.insert(key.to_string(), value);
        }
    }

    fn device_type(&self, device: Id) -> Option<String> {
        find_device(self.doc.project(), device).map(|d| d.type_id.clone())
    }

    // ---- telemetry ----

    pub fn telemetry(&self) -> &Arc<Telemetry> {
        self.engine.telemetry()
    }

    /// Peak of a mixer track since the last call.
    pub fn take_track_meter(&self, track: Id) -> (f32, f32) {
        self.meters
            .get(&track)
            .map_or((0.0, 0.0), |&m| self.engine.telemetry().take_meter(m))
    }

    /// Output peak of a channel's instrument since the last call.
    pub fn take_channel_activity(&self, channel: Id) -> f32 {
        let Some(c) = self.doc.project().channel(channel) else {
            return 0.0;
        };
        self.devices
            .get(&c.device.id)
            .map_or(0.0, |d| self.engine.telemetry().take_slot(d.slot))
    }

    pub fn cpu_load(&self) -> f32 {
        self.engine.telemetry().cpu_load()
    }

    pub fn device_name(&self) -> &str {
        self.engine.device_name()
    }

    pub fn sample_rate(&self) -> f32 {
        self.engine.sample_rate()
    }

    // ---- files ----

    /// Saves to `path` (or the current path).
    pub fn save(&mut self, path: Option<PathBuf>) -> Result<PathBuf, String> {
        let path = path.or_else(|| self.path.clone()).ok_or("no file chosen")?;
        let path = if path.extension().is_some_and(|e| e == "peelproj") {
            path
        } else {
            path.with_extension("peelproj")
        };
        let mut project = self.doc.project().clone();
        if project.meta.title == "Untitled" || project.meta.title.is_empty() {
            project.meta.title = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let title = project.meta.title.clone();
            self.doc.edit("Rename project", |p| p.meta.title = title);
        }
        self.capture_plugin_states();
        peel_project::save(self.doc.project(), &path).map_err(|e| e.to_string())?;
        self.doc.mark_saved();
        self.path = Some(path.clone());
        self.notify(
            format!(
                "Saved {}",
                path.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            ),
            false,
        );
        Ok(path)
    }

    /// Opens a project, replacing the current one.
    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let loaded = peel_project::load(path).map_err(|e| e.to_string())?;
        for w in &loaded.warnings {
            log::warn!("{w}");
        }
        self.replace_project(loaded.project);
        self.path = Some(path.to_path_buf());
        Ok(())
    }

    /// Replaces the document (new project, template, open).
    pub fn replace_project(&mut self, project: Project) {
        self.stop();
        self.doc.replace(project);
        self.path = None;
        self.mode = PlayMode::Pattern(
            self.doc
                .project()
                .patterns
                .first()
                .map_or(Id::NIL, |p| p.id),
        );
        if let PlayMode::Pattern(p) = self.mode {
            self.selected_pattern = p;
        }
        self.synced = None;
        self.plan = None;
        self.audio.failed.clear();
        let keep: std::collections::HashSet<Id> =
            self.doc.project().assets.iter().map(|a| a.id).collect();
        self.samples.retain_assets(&keep);
        self.sync();
    }

    /// Renders the arrangement to a WAV file on a background thread.
    pub fn export(&mut self, settings: ExportSettings) -> ExportJob {
        // Plugins get their own instances, built here on the main thread
        // from their current state.
        self.capture_plugin_states();
        let mut prebuilt = export::Prebuilt::new();
        let mut plugin_handles = Vec::new();
        let project = self.doc.project().clone();
        let instances = project
            .channels
            .iter()
            .map(|c| &c.device)
            .chain(project.mixer.tracks.iter().flat_map(|t| t.inserts.iter()))
            .filter(|d| d.type_id.starts_with("clap:"));
        for inst in instances {
            match self
                .plugins
                .create_offline(inst, settings.sample_rate as f32, 512)
            {
                Ok((dev, handle)) => {
                    prebuilt.insert(inst.id, dev);
                    plugin_handles.push(handle);
                }
                Err(e) => log::warn!("export: {e}"),
            }
        }
        let samples: HashMap<Id, Arc<SampleBuffer>> = self
            .doc
            .project()
            .assets
            .iter()
            .filter_map(|a| self.samples.cached(a.id).map(|b| (a.id, b)))
            .collect();
        ExportJob::start(
            self.doc.project().clone(),
            samples,
            self.export_loads(),
            prebuilt,
            plugin_handles,
            settings,
        )
    }

    /// Imports an audio file as an asset referenced in place (not copied).
    pub fn reference_audio_file(&mut self, path: &Path) -> Result<AudioAsset, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        if let Some(existing) = self.doc.project().assets.iter().find(|a| a.hash == hash) {
            return Ok(existing.clone());
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(AudioAsset {
            id: Id::new(),
            hash,
            name,
            path: path.to_string_lossy().into_owned(),
            managed: false,
            sample_rate: 0,
            channels: 0,
            frames: 0,
        })
    }

    /// Gain to apply for a dB fader value (helper for UIs).
    pub fn db_to_gain(db: f32) -> f32 {
        db_to_gain(db)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.engine.send(EngineCommand::Panic);
    }
}

/// Finds a device instance anywhere in the project.
pub fn find_device(p: &Project, id: Id) -> Option<&DeviceInstance> {
    p.channels
        .iter()
        .map(|c| &c.device)
        .chain(p.mixer.tracks.iter().flat_map(|t| t.inserts.iter()))
        .find(|d| d.id == id)
}

pub fn find_device_mut(p: &mut Project, id: Id) -> Option<&mut DeviceInstance> {
    if let Some(c) = p.channels.iter_mut().find(|c| c.device.id == id) {
        return Some(&mut c.device);
    }
    p.mixer
        .tracks
        .iter_mut()
        .flat_map(|t| t.inserts.iter_mut())
        .find(|d| d.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_reopen_round_trip() {
        let dir = std::env::temp_dir().join(format!("peel-session-test-{}", Id::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Groove.peelproj");
        let mut a = Session::new_silent(template::beat_and_melody());
        let saved = a.save(Some(path.clone())).expect("save");
        assert_eq!(saved, path);
        assert!(!a.doc.is_dirty());
        assert_eq!(a.project().meta.title, "Groove");

        let mut b = Session::new_silent(template::empty());
        b.open(&path).expect("open");
        let (pa, pb) = (a.project(), b.project());
        assert_eq!(pa.channels, pb.channels);
        assert_eq!(pa.patterns, pb.patterns);
        assert_eq!(pa.arrangements, pb.arrangements);
        assert_eq!(pa.mixer, pb.mixer);
        assert_eq!(pa.bpm(), pb.bpm());
        assert!(b.missing_devices().is_empty(), "{:?}", b.missing_devices());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exported_volume_automation_fades_the_song_out() {
        use peel_document::{AutomationPoint, AutomationTarget, Clip, ClipContent, Time};
        let dir = std::env::temp_dir().join(format!("peel-auto-test-{}", Id::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("fade.wav");
        let mut project = template::beat_and_melody();
        let master = project.mixer.tracks[0].id;
        let lane = project.arrangement().tracks[5].id;
        let unity = peel_document::db_to_fader(0.0);
        project.arrangement_mut().clips.push(Clip {
            id: Id::new(),
            track: lane,
            start: Time::ZERO,
            length: Time::beats(32),
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::Automation {
                target: AutomationTarget::TrackGain { track: master },
                points: vec![
                    AutomationPoint {
                        position: Time::beats(16),
                        value: unity,
                    },
                    AutomationPoint {
                        position: Time::beats(28),
                        value: 0.0,
                    },
                ],
            },
        });
        assert!(
            peel_document::validate(&project)
                .iter()
                .all(|i| !i.is_error())
        );
        let settings = ExportSettings {
            path: out.clone(),
            sample_rate: 44_100,
            bits: 24,
            tail_seconds: 0.0,
        };
        let progress = std::sync::atomic::AtomicU32::new(0);
        let cancel = std::sync::atomic::AtomicBool::new(false);
        render_to_wav(&project, &HashMap::new(), &settings, &progress, &cancel).expect("render");
        let buf = peel_media::decode_file(&out).expect("decode");
        let sec = |beat: f64| (beat * 60.0 / 118.0 * 44_100.0) as usize;
        let rms = |a: f64, b: f64| {
            let s = &buf.left()[sec(a)..sec(b)];
            (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
        };
        let before = rms(8.0, 16.0);
        let after = rms(28.5, 31.5);
        assert!(before > 0.05, "playing before the fade: {before}");
        assert!(
            after < before * 0.01,
            "silent after the fade: {after} vs {before}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_renders_audio() {
        let dir = std::env::temp_dir().join(format!("peel-export-test-{}", Id::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("mix.wav");
        let project = template::beat_and_melody();
        let settings = ExportSettings {
            path: out.clone(),
            sample_rate: 44_100,
            bits: 16,
            tail_seconds: 1.0,
        };
        let progress = std::sync::atomic::AtomicU32::new(0);
        let cancel = std::sync::atomic::AtomicBool::new(false);
        render_to_wav(&project, &HashMap::new(), &settings, &progress, &cancel).expect("render");
        let buf = peel_media::decode_file(&out).expect("decode");
        assert_eq!(buf.sample_rate, 44_100.0);
        let secs = buf.duration_seconds();
        let expected = 32.0 * 60.0 / 118.0 + 1.0;
        assert!((secs - expected).abs() < 0.05, "{secs} vs {expected}");
        let peak = buf.left().iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.1 && peak <= 1.0, "peak {peak}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
