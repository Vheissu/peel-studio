//! Control-thread prepared delay compensation for the complete mixer graph.
//!
//! Delay storage is proportional to actual graph edges, never the maximum track
//! pool. The callback only advances circular buffers; replacement storage is
//! retired on the control thread.

use crate::plan::RenderPlan;

/// Largest supported latency of a path, in samples (about 5.5 s at 48 kHz).
pub const MAX_LATENCY: u32 = 262_144;
/// Upper bound on all compensation delay storage together.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct Delay {
    samples: Vec<[f32; 2]>,
    cursor: usize,
    valid: usize,
    pending: usize,
}

impl Delay {
    fn new(samples: u32, budget: &mut usize) -> Result<Self, String> {
        let bytes = samples as usize * std::mem::size_of::<[f32; 2]>();
        *budget = budget
            .checked_add(bytes)
            .ok_or("Compensation storage overflow")?;
        if *budget > MAX_BYTES {
            return Err("Plugin delay compensation exceeds the 64 MiB storage limit".into());
        }
        Ok(Self {
            samples: vec![[0.0; 2]; samples as usize],
            cursor: 0,
            valid: 0,
            pending: 0,
        })
    }

    pub(crate) fn reset(&mut self) {
        // Old cells are not read until the ring has been completely rewritten.
        self.cursor = 0;
        self.valid = 0;
        self.pending = 0;
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.pending > 0
    }

    pub(crate) fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.samples.is_empty() {
            return;
        }
        for i in 0..left.len() {
            let input = [left[i], right[i]];
            let output = if self.valid == self.samples.len() {
                self.samples[self.cursor]
            } else {
                self.valid += 1;
                [0.0; 2]
            };
            if output[0] != 0.0 || output[1] != 0.0 {
                self.pending -= 1;
            }
            if input[0] != 0.0 || input[1] != 0.0 {
                self.pending += 1;
            }
            self.samples[self.cursor] = input;
            self.cursor += 1;
            if self.cursor == self.samples.len() {
                self.cursor = 0;
            }
            left[i] = output[0];
            right[i] = output[1];
        }
    }
}

#[derive(PartialEq)]
struct TrackIdentity {
    meter: u16,
    inserts: Vec<u32>,
    output: Option<u16>,
    sends: Vec<u16>,
    keys: Vec<(u32, u16)>,
}

fn audio_identity(plan: &RenderPlan) -> Vec<[u64; 9]> {
    plan.audio
        .iter()
        .map(|r| {
            [
                u64::from(r.track),
                std::sync::Arc::as_ptr(&r.sample) as usize as u64,
                r.start_beat.to_bits(),
                r.end_beat.to_bits(),
                r.source_start_seconds.to_bits(),
                u64::from(r.gain.to_bits()),
                u64::from(r.reversed),
                r.fade_in_beats.to_bits(),
                r.fade_out_beats.to_bits(),
            ]
        })
        .collect()
}

/// Allocated and validated on the control thread, moved into the callback.
pub struct Compensation {
    pub(crate) channels: Vec<Delay>,
    pub(crate) audio: Vec<Delay>,
    pub(crate) outputs: Vec<Delay>,
    pub(crate) sends: Vec<Vec<Delay>>,
    pub(crate) keys: Vec<Vec<Delay>>,
    pub(crate) click: Delay,
    pub(crate) scratch_l: Vec<f32>,
    pub(crate) scratch_r: Vec<f32>,
    latency: u32,
    master: u16,
    channel_identity: Vec<(u32, u16)>,
    track_identity: Vec<TrackIdentity>,
    audio_identity: Vec<[u64; 9]>,
    resource_revision: u64,
}

impl Compensation {
    pub fn prepare(plan: &RenderPlan, latencies: &[u32], max_block: usize) -> Result<Self, String> {
        let audio_tracks: Vec<bool> = (0..plan.tracks.len())
            .map(|ti| plan.audio.iter().any(|r| r.track as usize == ti))
            .collect();
        Self::prepare_with_audio(plan, latencies, max_block, &audio_tracks)
    }

    fn prepare_with_audio(
        plan: &RenderPlan,
        latencies: &[u32],
        max_block: usize,
        audio_tracks: &[bool],
    ) -> Result<Self, String> {
        let n = plan.tracks.len();
        if n == 0 || n > crate::MAX_TRACKS || plan.master as usize >= n {
            return Err("Invalid mixer graph for plugin delay compensation".into());
        }
        let latency = |slot: u32| -> Result<u32, String> {
            let value = latencies.get(slot as usize).copied().unwrap_or(0);
            if value > MAX_LATENCY {
                return Err(format!(
                    "Device {slot} reports unsupported latency ({value} samples; maximum {MAX_LATENCY})"
                ));
            }
            Ok(value)
        };
        let checked = |a: u32, b: u32| -> Result<u32, String> {
            a.checked_add(b)
                .filter(|&v| v <= MAX_LATENCY)
                .ok_or_else(|| {
                    format!("Mixer path exceeds the {MAX_LATENCY}-sample compensation limit")
                })
        };
        let mut input = vec![0u32; n];
        let mut output = vec![0u32; n];
        for ch in &plan.channels {
            let track = ch.track as usize;
            if track >= n {
                continue;
            }
            input[track] = input[track].max(latency(ch.slot)?);
        }
        for (ti, track) in plan.tracks.iter().enumerate() {
            let mut prefix = 0;
            for &insert in &track.inserts {
                if let Some(&(_, key)) = track.keys.iter().find(|k| k.0 == insert) {
                    let key = key as usize;
                    if key >= ti {
                        return Err("Sidechain routing must follow mixer order".into());
                    }
                    input[ti] = input[ti].max(output[key].saturating_sub(prefix));
                }
                prefix = checked(prefix, latency(insert)?)?;
            }
            output[ti] = checked(input[ti], prefix)?;
            for dest in track
                .output
                .into_iter()
                .chain(track.sends.iter().map(|s| s.dest))
            {
                let dest = dest as usize;
                if dest <= ti || dest >= n {
                    return Err("Audio routing must follow mixer order".into());
                }
                input[dest] = input[dest].max(output[ti]);
            }
        }
        let mut budget = 0;
        let mut channels = Vec::with_capacity(plan.channels.len());
        for ch in &plan.channels {
            channels.push(Delay::new(
                input
                    .get(ch.track as usize)
                    .copied()
                    .unwrap_or(0)
                    .saturating_sub(latency(ch.slot)?),
                &mut budget,
            )?);
        }
        let mut audio = Vec::with_capacity(n);
        let mut outputs = Vec::with_capacity(n);
        let mut sends = Vec::with_capacity(n);
        let mut keys = Vec::with_capacity(n);
        for (ti, track) in plan.tracks.iter().enumerate() {
            // Empty tracks have no independent timeline source to delay.
            let audio_latency = if audio_tracks.get(ti) == Some(&true) {
                input[ti]
            } else {
                0
            };
            audio.push(Delay::new(audio_latency, &mut budget)?);
            let edge = track
                .output
                .map_or(0, |dest| input[dest as usize] - output[ti]);
            outputs.push(Delay::new(edge, &mut budget)?);
            let mut track_sends = Vec::with_capacity(track.sends.len());
            for send in &track.sends {
                track_sends.push(Delay::new(
                    input[send.dest as usize] - output[ti],
                    &mut budget,
                )?);
            }
            sends.push(track_sends);
            let mut track_keys = Vec::with_capacity(track.inserts.len());
            let mut prefix = 0;
            for &insert in &track.inserts {
                let key_delay = track
                    .keys
                    .iter()
                    .find(|k| k.0 == insert)
                    .map_or(0, |&(_, key)| input[ti] + prefix - output[key as usize]);
                track_keys.push(Delay::new(key_delay, &mut budget)?);
                prefix += latency(insert)?;
            }
            keys.push(track_keys);
        }
        let latency = output[plan.master as usize];
        let click = Delay::new(latency, &mut budget)?;
        Ok(Self {
            channels,
            audio,
            outputs,
            sends,
            keys,
            click,
            scratch_l: vec![0.0; max_block],
            scratch_r: vec![0.0; max_block],
            latency,
            master: plan.master,
            channel_identity: plan.channels.iter().map(|c| (c.slot, c.track)).collect(),
            track_identity: plan
                .tracks
                .iter()
                .map(|t| TrackIdentity {
                    meter: t.meter,
                    inserts: t.inserts.clone(),
                    output: t.output,
                    sends: t.sends.iter().map(|s| s.dest).collect(),
                    keys: t.keys.clone(),
                })
                .collect(),
            audio_identity: audio_identity(plan),
            resource_revision: 0,
        })
    }

    pub fn latency_samples(&self) -> u32 {
        self.latency
    }

    /// Move compatible ring ownership, never copy/free it on the callback.
    /// Musical edits preserve their already-rendered audio; a changed route,
    /// source or device generation starts with clean history.
    pub(crate) fn inherit_history(&mut self, old: &mut Self) {
        if self.master != old.master
            || self.resource_revision != old.resource_revision
            || self.channel_identity != old.channel_identity
            || self.track_identity != old.track_identity
            || self.audio_identity != old.audio_identity
        {
            return;
        }
        let move_delay = |new: &mut Delay, previous: &mut Delay| {
            if new.samples.len() == previous.samples.len() {
                std::mem::swap(new, previous);
            }
        };
        for (new, previous) in self
            .channels
            .iter_mut()
            .zip(&mut old.channels)
            .chain(self.audio.iter_mut().zip(&mut old.audio))
            .chain(self.outputs.iter_mut().zip(&mut old.outputs))
            .chain(
                self.sends
                    .iter_mut()
                    .flatten()
                    .zip(old.sends.iter_mut().flatten()),
            )
            .chain(
                self.keys
                    .iter_mut()
                    .flatten()
                    .zip(old.keys.iter_mut().flatten()),
            )
        {
            move_delay(new, previous);
        }
        move_delay(&mut self.click, &mut old.click);
    }

    pub(crate) fn reset(&mut self) {
        for delay in self
            .channels
            .iter_mut()
            .chain(self.audio.iter_mut())
            .chain(self.outputs.iter_mut())
            .chain(self.sends.iter_mut().flatten())
            .chain(self.keys.iter_mut().flatten())
        {
            delay.reset();
        }
        self.click.reset();
    }
}

/// Control-side state shared by realtime and offline frontends.
pub(crate) struct Control {
    pub plan: Option<RenderPlan>,
    pub audio_tracks: Vec<bool>,
    pub audio_identity: Vec<[u64; 9]>,
    pub resource_revision: u64,
    pub latencies: Vec<u32>,
    pub bypassed: Vec<bool>,
    pub pending: Vec<Option<u32>>,
    pub pending_generation: Vec<u64>,
    pub error: Option<String>,
    pub revision: u64,
}

impl Control {
    pub fn new(slots: usize) -> Self {
        Self {
            plan: None,
            audio_tracks: Vec::new(),
            audio_identity: Vec::new(),
            resource_revision: 0,
            latencies: vec![0; slots],
            bypassed: vec![false; slots],
            pending: vec![None; slots],
            pending_generation: vec![0; slots],
            error: None,
            revision: 0,
        }
    }

    pub fn remember_plan(&mut self, plan: &RenderPlan) {
        self.audio_identity = audio_identity(plan);
        self.audio_tracks = (0..plan.tracks.len())
            .map(|ti| plan.audio.iter().any(|r| r.track as usize == ti))
            .collect();
        // Graph metadata must not keep samples, plugin state or sequence data
        // alive after the engine retires the actual render plan.
        self.plan = Some(RenderPlan {
            revision: plan.revision,
            tempo: plan.tempo.clone(),
            sequence: Vec::new(),
            expression_notes: Vec::new(),
            expression_chase: Vec::new(),
            note_identities: Vec::new(),
            note_identity_index: Vec::new(),
            length_beats: plan.length_beats,
            loop_range: plan.loop_range,
            channels: plan.channels.clone(),
            audio: Vec::new(),
            automation: Vec::new(),
            tracks: plan.tracks.clone(),
            master: plan.master,
            beats_per_bar: plan.beats_per_bar,
        });
    }

    pub fn prepare(
        &self,
        plan: &RenderPlan,
        max_block: usize,
        change: Option<(usize, u32, bool)>,
    ) -> Result<Box<Compensation>, String> {
        let mut latencies = self.latencies.clone();
        for (i, latency) in latencies.iter_mut().enumerate() {
            if self.bypassed[i] {
                *latency = 0;
            }
        }
        if let Some((slot, latency, bypass)) = change {
            let Some(dest) = latencies.get_mut(slot) else {
                return Err("Invalid device slot".into());
            };
            *dest = if bypass { 0 } else { latency };
        }
        let cached = self
            .plan
            .as_ref()
            .is_some_and(|cached| std::ptr::eq(cached, plan));
        let mut compensation = if cached {
            let mut prepared =
                Compensation::prepare_with_audio(plan, &latencies, max_block, &self.audio_tracks)?;
            prepared.audio_identity = self.audio_identity.clone();
            prepared
        } else {
            Compensation::prepare(plan, &latencies, max_block)?
        };
        compensation.resource_revision = self
            .resource_revision
            .wrapping_add(u64::from(change.is_some()));
        Ok(Box::new(compensation))
    }
}
