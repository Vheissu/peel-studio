//! Offline export. Renders an immutable snapshot of the project with fresh
//! device instances through the same engine code path as playback.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use peel_document::{ClipContent, Id, Project};
use peel_dsp::SampleBuffer;
use peel_engine::{EngineCommand, EngineConfig, OfflineEngine, Slot};

use crate::compile::{CompileContext, PlayMode, compile};

#[derive(Clone, Debug)]
pub struct ExportSettings {
    pub path: PathBuf,
    pub sample_rate: u32,
    /// 16, 24 or 32 (float).
    pub bits: u16,
    /// Seconds rendered after the arrangement ends (reverb and delay tails).
    pub tail_seconds: f32,
}

/// A running export; poll `progress()` and `result()` from the UI.
pub struct ExportJob {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<PathBuf, String>>>>,
}

impl ExportJob {
    /// Starts an export. `loads` are audio files the clips need that the
    /// session has not decoded yet; the export thread decodes them itself.
    pub(crate) fn start(
        project: Project,
        samples: HashMap<Id, Arc<SampleBuffer>>,
        loads: Vec<(Id, PathBuf)>,
        settings: ExportSettings,
    ) -> ExportJob {
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        let (p, c, r) = (progress.clone(), cancel.clone(), result.clone());
        std::thread::Builder::new()
            .name("peel-export".into())
            .spawn(move || {
                let mut samples = samples;
                for (id, path) in loads {
                    match peel_media::load_for_engine(&path, settings.sample_rate as f32) {
                        Ok(buf) => {
                            samples.insert(id, Arc::new(buf));
                        }
                        Err(e) => log::warn!("export: could not load {}: {e}", path.display()),
                    }
                }
                let out = render(&project, &samples, &settings, &p, &c);
                if let Ok(mut slot) = r.lock() {
                    *slot = Some(out);
                }
            })
            .expect("export thread");
        ExportJob {
            progress,
            cancel,
            result,
        }
    }

    /// 0..=1.
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The finished result, once.
    pub fn take_result(&self) -> Option<Result<PathBuf, String>> {
        self.result.lock().ok()?.take()
    }
}

/// Renders the arrangement synchronously (used by the export thread and
/// the command-line tool).
pub fn render(
    project: &Project,
    samples: &HashMap<Id, Arc<SampleBuffer>>,
    settings: &ExportSettings,
    progress: &AtomicU32,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let rate = settings.sample_rate as f32;
    let config = EngineConfig {
        max_block: 512,
        ..EngineConfig::default()
    };
    let mut engine = OfflineEngine::new(rate, &config);

    let mut slots: HashMap<Id, Slot> = HashMap::new();
    let devices = project
        .channels
        .iter()
        .map(|c| &c.device)
        .chain(project.mixer.tracks.iter().flat_map(|t| t.inserts.iter()));
    for (i, inst) in devices.enumerate() {
        let Some(mut device) = peel_devices::create(&inst.type_id) else {
            continue;
        };
        let desc = device.descriptor();
        for (k, v) in &inst.params {
            if let Some(p) = desc.param_by_key(k) {
                device.set_param(p.id, *v);
            }
        }
        device.prepare(rate, config.max_block);
        if let Some(asset) = inst.sample
            && let Some(buf) = samples.get(&asset)
        {
            let _ = device.set_sample(Some(buf.clone()));
        }
        let slot = i as Slot;
        engine.send(EngineCommand::InstallDevice { slot, device });
        slots.insert(inst.id, slot);
    }
    let meters: HashMap<Id, u16> = project
        .mixer
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id, i as u16))
        .collect();
    let audio = clip_audio(project, samples, rate)?;
    let (mut plan, _) = compile(
        project,
        &CompileContext {
            mode: PlayMode::Song,
            slots: &slots,
            meters: &meters,
            samples: &audio,
        },
    );
    plan.loop_range = None;
    let tempo = plan.tempo.clone();
    let end_beat = project.arrangement().end().as_f64();
    if end_beat <= 0.0 {
        return Err("The arrangement is empty. Place patterns in the playlist first.".into());
    }
    let total = (tempo.beat_to_seconds(end_beat) as f32 + settings.tail_seconds.max(0.0)) * rate;
    let total = total.ceil() as usize;
    engine.send(EngineCommand::SetPlan(Box::new(plan)));
    engine.send(EngineCommand::Seek { beat: 0.0 });
    engine.send(EngineCommand::Play);

    let mut left = vec![0.0f32; total];
    let mut right = vec![0.0f32; total];
    let chunk = 4096;
    let mut pos = 0;
    while pos < total {
        if cancel.load(Ordering::Relaxed) {
            return Err("Export cancelled".into());
        }
        let n = chunk.min(total - pos);
        engine.render(&mut left[pos..pos + n], &mut right[pos..pos + n]);
        pos += n;
        progress.store(
            (pos as f32 / total as f32 * 0.95).to_bits(),
            Ordering::Relaxed,
        );
        engine.drain_retired();
    }
    let format = peel_media::WavFormat {
        bits: settings.bits,
        dither: settings.bits < 32,
    };
    peel_media::write_wav(
        &settings.path,
        &[&left, &right],
        settings.sample_rate,
        format,
    )
    .map_err(|e| e.to_string())?;
    progress.store(1f32.to_bits(), Ordering::Relaxed);
    Ok(settings.path.clone())
}

/// Decoded audio for the arrangement's audible audio clips at the export
/// rate. A clip whose audio is missing is an error rather than a silent gap
/// in the finished file.
fn clip_audio(
    project: &Project,
    samples: &HashMap<Id, Arc<SampleBuffer>>,
    rate: f32,
) -> Result<HashMap<Id, Arc<SampleBuffer>>, String> {
    let arr = project.arrangement();
    let mut out = HashMap::new();
    for clip in &arr.clips {
        let ClipContent::Audio { asset, .. } = &clip.content else {
            continue;
        };
        let lane_muted = arr.tracks.iter().any(|t| t.id == clip.track && t.muted);
        if clip.muted || lane_muted || out.contains_key(asset) {
            continue;
        }
        let Some(buf) = samples.get(asset) else {
            let name = project.asset(*asset).map_or("An audio file", |a| &a.name);
            return Err(format!(
                "{name} could not be loaded, so it cannot be exported."
            ));
        };
        let buf = if (buf.sample_rate - rate).abs() < 0.5 {
            buf.clone()
        } else {
            Arc::new(peel_media::resample(buf, rate))
        };
        out.insert(*asset, buf);
    }
    Ok(out)
}
