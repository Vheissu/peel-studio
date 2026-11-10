//! Background sample decoding. Files are decoded and resampled to the
//! engine rate on a worker thread, never on the UI or audio thread.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use peel_document::Id;
use peel_dsp::SampleBuffer;
use peel_media::Peaks;

enum Request {
    Asset(Id, PathBuf, f32),
    /// A file to audition: path, engine rate, gain, request number.
    Preview(PathBuf, f32, f32, u64),
    /// One note of a native instrument: type, preset, engine rate, gain,
    /// request number.
    Audition(String, Option<String>, f32, f32, u64),
}

/// A decoded asset with its waveform summary, both built on the worker.
type Loaded = (Arc<SampleBuffer>, Arc<Peaks>);

enum Done {
    Asset(Id, Result<Loaded, String>),
    Preview(Arc<SampleBuffer>, f32, u64),
}

pub struct SampleLoader {
    tx: Sender<Request>,
    rx: Receiver<Done>,
    cache: HashMap<Id, Arc<SampleBuffer>>,
    peaks: HashMap<Id, Arc<Peaks>>,
    pending: HashSet<Id>,
    preview: Option<(Arc<SampleBuffer>, f32)>,
    /// Number of the latest preview request. Older ones are skipped by the
    /// worker and dropped on arrival, so only the last click is heard.
    preview_no: Arc<AtomicU64>,
}

impl SampleLoader {
    pub fn new() -> SampleLoader {
        let (tx, req_rx) = channel::<Request>();
        let (done_tx, rx) = channel::<Done>();
        let preview_no = Arc::new(AtomicU64::new(0));
        let latest = preview_no.clone();
        std::thread::Builder::new()
            .name("peel-samples".into())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    match req {
                        Request::Asset(id, path, rate) => {
                            // Decoding and the peak pyramid both stay off the
                            // UI thread.
                            let r = peel_media::load_for_engine(&path, rate)
                                .map(|buf| {
                                    let peaks = Arc::new(Peaks::compute(&buf));
                                    (Arc::new(buf), peaks)
                                })
                                .map_err(|e| e.to_string());
                            if done_tx.send(Done::Asset(id, r)).is_err() {
                                break;
                            }
                        }
                        Request::Preview(path, rate, gain, no) => {
                            if latest.load(Ordering::Relaxed) == no
                                && let Ok(buf) = peel_media::load_for_engine(&path, rate)
                                && done_tx
                                    .send(Done::Preview(Arc::new(buf), gain, no))
                                    .is_err()
                            {
                                break;
                            }
                        }
                        Request::Audition(type_id, preset, rate, gain, no) => {
                            if latest.load(Ordering::Relaxed) == no
                                && let Some(buf) =
                                    crate::audition::render(&type_id, preset.as_deref(), rate)
                                && done_tx
                                    .send(Done::Preview(Arc::new(buf), gain, no))
                                    .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            })
            .expect("sample loader thread");
        SampleLoader {
            tx,
            rx,
            cache: HashMap::new(),
            peaks: HashMap::new(),
            pending: HashSet::new(),
            preview: None,
            preview_no,
        }
    }

    pub fn request(&mut self, id: Id, path: PathBuf, rate: f32) {
        if self.cache.contains_key(&id) || !self.pending.insert(id) {
            return;
        }
        let _ = self.tx.send(Request::Asset(id, path, rate));
    }

    pub fn request_preview(&mut self, path: PathBuf, rate: f32, gain: f32) {
        let no = self.next_preview();
        let _ = self.tx.send(Request::Preview(path, rate, gain, no));
    }

    /// Asks for one note of a native instrument on the preview route.
    pub fn request_audition(&mut self, type_id: &str, preset: Option<&str>, rate: f32, gain: f32) {
        let no = self.next_preview();
        let _ = self.tx.send(Request::Audition(
            type_id.to_string(),
            preset.map(str::to_string),
            rate,
            gain,
            no,
        ));
    }

    /// Drops any preview still being prepared.
    pub fn cancel_preview(&mut self) {
        self.next_preview();
        self.preview = None;
    }

    fn next_preview(&mut self) -> u64 {
        self.preview_no.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn cached(&self, id: Id) -> Option<Arc<SampleBuffer>> {
        self.cache.get(&id).cloned()
    }

    /// Forgets decoded assets that are not in `keep` (after a project is
    /// replaced). The engine keeps whatever it is still playing.
    pub fn retain_assets(&mut self, keep: &HashSet<Id>) {
        self.cache.retain(|id, _| keep.contains(id));
        self.peaks.retain(|id, _| keep.contains(id));
    }

    /// Waveform summary of a decoded asset.
    pub fn peaks(&self, id: Id) -> Option<Arc<Peaks>> {
        self.peaks.get(&id).cloned()
    }

    /// True while a load for `id` has been requested and not finished.
    pub fn is_pending(&self, id: Id) -> bool {
        self.pending.contains(&id)
    }

    /// Newly finished asset loads.
    pub fn poll(&mut self) -> Vec<(Id, Result<Arc<SampleBuffer>, String>)> {
        let mut out = Vec::new();
        while let Ok(done) = self.rx.try_recv() {
            match done {
                Done::Asset(id, r) => {
                    self.pending.remove(&id);
                    out.push((
                        id,
                        r.map(|(buf, peaks)| {
                            self.cache.insert(id, buf.clone());
                            self.peaks.insert(id, peaks);
                            buf
                        }),
                    ));
                }
                Done::Preview(buf, gain, no) => {
                    if self.preview_no.load(Ordering::Relaxed) == no {
                        self.preview = Some((buf, gain));
                    }
                }
            }
        }
        out
    }

    pub fn poll_preview(&mut self) -> Option<(Arc<SampleBuffer>, f32)> {
        self.preview.take()
    }
}

impl Default for SampleLoader {
    fn default() -> Self {
        SampleLoader::new()
    }
}
