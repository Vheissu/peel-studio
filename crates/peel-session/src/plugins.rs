//! Third-party CLAP plugins: discovery, instances and saved state.
//!
//! Scanning runs on a background thread, in a separate process when the
//! `peel-clap-scan` helper sits next to the executable (so a crashing plugin
//! cannot take the app down), otherwise in process. Instances are created on
//! the control thread, restored from their saved state, and wrapped as
//! ordinary [`peel_dsp::Device`]s for the engine.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use base64::Engine as _;
use peel_document::{DeviceInstance, Id};
use peel_dsp::Device;
use peel_host_clap::{ClapDevice, ClapHandle, ClapPluginInfo, ClapRole};

/// Device type id for a plugin.
pub fn type_id_for(info: &ClapPluginInfo) -> String {
    format!("clap:{}", info.id)
}

#[derive(Default)]
pub struct PluginRegistry {
    rx: Option<Receiver<Vec<ClapPluginInfo>>>,
    plugins: Vec<ClapPluginInfo>,
    scanned: bool,
    handles: HashMap<Id, ClapHandle>,
    /// Instances that failed to load; not retried until the next scan.
    failed: HashSet<Id>,
}

impl PluginRegistry {
    /// Starts scanning the standard plugin folders in the background.
    pub fn start_scan() -> PluginRegistry {
        let mut r = PluginRegistry::default();
        r.rescan();
        r
    }

    /// Scans again in the background; live instances are unaffected.
    pub fn rescan(&mut self) {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("peel-plugin-scan".into())
            .spawn(move || {
                let plugins = match peel_host_clap::default_scanner_path() {
                    Some(scanner) => {
                        peel_host_clap::scan_isolated(
                            &scanner,
                            &peel_host_clap::search_paths(),
                            Duration::from_secs(15),
                        )
                        .plugins
                    }
                    None => peel_host_clap::scan(),
                };
                let _ = tx.send(plugins);
            })
            .ok();
        self.rx = Some(rx);
        self.scanned = false;
    }

    /// Uses `plugins` as the installed set (tests, or a cached scan).
    pub fn set(&mut self, plugins: Vec<ClapPluginInfo>) {
        self.rx = None;
        self.accept(plugins);
    }

    fn accept(&mut self, mut plugins: Vec<ClapPluginInfo>) {
        plugins.retain(|p| matches!(p.role, ClapRole::Instrument | ClapRole::AudioEffect));
        plugins.sort_by_key(|p| p.name.to_lowercase());
        plugins.dedup_by(|a, b| a.id == b.id);
        log::info!("found {} CLAP plugins", plugins.len());
        self.plugins = plugins;
        self.scanned = true;
        self.failed.clear();
    }

    /// An empty registry that never scans (tests, headless tools).
    pub fn disabled() -> PluginRegistry {
        PluginRegistry {
            scanned: true,
            ..Default::default()
        }
    }

    /// Takes a finished scan. Returns true when new plugins arrived.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Ok(plugins) => {
                self.rx = None;
                self.accept(plugins);
                true
            }
            Err(_) => false,
        }
    }

    pub fn plugins(&self) -> &[ClapPluginInfo] {
        &self.plugins
    }

    pub fn is_scanning(&self) -> bool {
        !self.scanned
    }

    pub fn info(&self, type_id: &str) -> Option<&ClapPluginInfo> {
        let id = type_id.strip_prefix("clap:")?;
        self.plugins.iter().find(|p| p.id == id)
    }

    /// Creates a plugin instance for `inst`, restoring its saved state.
    pub fn create(
        &mut self,
        inst: &DeviceInstance,
        sample_rate: f32,
        max_block: usize,
    ) -> Result<Box<dyn Device>, String> {
        if self.failed.contains(&inst.id) {
            return Err(format!("{} failed to load", inst.type_id));
        }
        let dev = match self.instantiate(inst, sample_rate, max_block) {
            Ok(d) => d,
            Err(e) => {
                if self.info(&inst.type_id).is_some() {
                    self.failed.insert(inst.id);
                }
                return Err(e);
            }
        };
        self.handles.insert(inst.id, dev.handle());
        Ok(Box::new(dev))
    }

    /// A separate instance for offline rendering. The caller keeps the
    /// handle alive on the main thread for as long as the device runs.
    pub fn create_offline(
        &self,
        inst: &DeviceInstance,
        sample_rate: f32,
        max_block: usize,
    ) -> Result<(Box<dyn Device>, ClapHandle), String> {
        let dev = self.instantiate(inst, sample_rate, max_block)?;
        dev.set_offline_render(true);
        let handle = dev.handle();
        Ok((Box::new(dev), handle))
    }

    fn instantiate(
        &self,
        inst: &DeviceInstance,
        sample_rate: f32,
        max_block: usize,
    ) -> Result<ClapDevice, String> {
        let info = self
            .info(&inst.type_id)
            .ok_or_else(|| format!("{} is not installed", inst.type_id))?
            .clone();
        let mut dev = ClapDevice::load(&info, sample_rate, max_block).map_err(|e| e.to_string())?;
        if let Some(state) = &inst.state {
            match base64::engine::general_purpose::STANDARD.decode(state) {
                Ok(bytes) => {
                    if let Err(e) = dev.load_state(&bytes) {
                        log::warn!("{}: could not restore state: {e}", info.name);
                    }
                }
                Err(e) => log::warn!("{}: corrupt saved state: {e}", info.name),
            }
        }
        Ok(dev)
    }

    /// The plugin's own display text for a parameter value.
    pub fn param_text(&self, device: Id, id: peel_dsp::ParamId, value: f32) -> Option<String> {
        self.handles.get(&device)?.param_text(id, value)
    }

    pub fn forget(&mut self, device: Id) {
        self.handles.remove(&device);
    }

    /// Main-thread housekeeping every plugin asks for; call once a frame.
    pub fn idle(&self) {
        for h in self.handles.values() {
            let _ = h.idle();
        }
    }

    /// Current state of every live plugin instance, base64-encoded.
    pub fn capture_states(&self) -> Vec<(Id, String)> {
        self.handles
            .iter()
            .filter_map(|(id, h)| {
                h.save_state()
                    .ok()
                    .map(|b| (*id, base64::engine::general_purpose::STANDARD.encode(b)))
            })
            .collect()
    }
}
