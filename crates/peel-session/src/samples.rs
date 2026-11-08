//! Background sample decoding. Files are decoded and resampled to the
//! engine rate on a worker thread, never on the UI or audio thread.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use peel_document::Id;
use peel_dsp::SampleBuffer;

enum Request {
    Asset(Id, PathBuf, f32),
    Preview(PathBuf, f32, f32),
}

enum Done {
    Asset(Id, Result<Arc<SampleBuffer>, String>),
    Preview(Arc<SampleBuffer>, f32),
}

pub struct SampleLoader {
    tx: Sender<Request>,
    rx: Receiver<Done>,
    cache: HashMap<Id, Arc<SampleBuffer>>,
    pending: HashSet<Id>,
    preview: Option<(Arc<SampleBuffer>, f32)>,
}

impl SampleLoader {
    pub fn new() -> SampleLoader {
        let (tx, req_rx) = channel::<Request>();
        let (done_tx, rx) = channel::<Done>();
        std::thread::Builder::new()
            .name("peel-samples".into())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    match req {
                        Request::Asset(id, path, rate) => {
                            let r = peel_media::load_for_engine(&path, rate)
                                .map(Arc::new)
                                .map_err(|e| e.to_string());
                            if done_tx.send(Done::Asset(id, r)).is_err() {
                                break;
                            }
                        }
                        Request::Preview(path, rate, gain) => {
                            if let Ok(buf) = peel_media::load_for_engine(&path, rate)
                                && done_tx.send(Done::Preview(Arc::new(buf), gain)).is_err()
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
            pending: HashSet::new(),
            preview: None,
        }
    }

    pub fn request(&mut self, id: Id, path: PathBuf, rate: f32) {
        if self.cache.contains_key(&id) || !self.pending.insert(id) {
            return;
        }
        let _ = self.tx.send(Request::Asset(id, path, rate));
    }

    pub fn request_preview(&mut self, path: PathBuf, rate: f32, gain: f32) {
        let _ = self.tx.send(Request::Preview(path, rate, gain));
    }

    pub fn cached(&self, id: Id) -> Option<Arc<SampleBuffer>> {
        self.cache.get(&id).cloned()
    }

    /// Newly finished asset loads.
    pub fn poll(&mut self) -> Vec<(Id, Result<Arc<SampleBuffer>, String>)> {
        let mut out = Vec::new();
        while let Ok(done) = self.rx.try_recv() {
            match done {
                Done::Asset(id, r) => {
                    self.pending.remove(&id);
                    if let Ok(buf) = &r {
                        self.cache.insert(id, buf.clone());
                    }
                    out.push((id, r));
                }
                Done::Preview(buf, gain) => self.preview = Some((buf, gain)),
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
