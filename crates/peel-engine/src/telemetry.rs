//! Lock-free engine telemetry read by the UI at display rate.
//!
//! The engine writes with relaxed atomics; peaks accumulate as a running
//! maximum until the UI reads (and clears) them, so short transients between
//! frames are never missed.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const MAX_METERS: usize = 512;
pub const MAX_SLOTS: usize = 1024;

pub struct Telemetry {
    /// One selected insert's pre/post audio, copied without blocking the callback.
    pub analysis: crate::analysis::AnalysisTap,
    pub playing: AtomicBool,
    /// Current content position in quarter notes (f64 bits).
    beat: AtomicU64,
    /// Callback time / deadline, 0..=1+ (f32 bits), smoothed.
    cpu_load: AtomicU32,
    /// Count of blocks that missed their deadline or underran.
    pub overruns: AtomicU64,
    /// Revision of the active plan.
    pub plan_revision: AtomicU64,
    /// Actual stream sample rate.
    pub sample_rate: AtomicU32,
    /// Peak per meter, left/right interleaved (f32 bits).
    meters: Box<[AtomicU32]>,
    /// Output peak per device slot (f32 bits), used for activity lights.
    slots: Box<[AtomicU32]>,
    /// Per-slot gain reduction in dB (f32 bits), max since last read.
    reduction: Box<[AtomicU32]>,
    /// Master output peak (f32 bits), left/right.
    master: [AtomicU32; 2],
    /// Number of non-finite samples contained at graph boundaries.
    pub non_finite: AtomicU64,
    /// Note-ons dropped because a per-block event buffer or the active-note
    /// table was full. Note-offs are never dropped for this reason.
    pub dropped_notes: AtomicU64,
    /// Times a channel's note-offs overflowed the reserved capacity and were
    /// replaced by a single `AllNotesOff` (note integrity fallback).
    pub note_overflows: AtomicU64,
    /// Plans refused by the engine because they exceed its fixed capacities
    /// or contain invalid indices.
    pub plans_rejected: AtomicU64,
    /// Retired objects that could neither be queued nor parked and were
    /// leaked rather than freed on the audio thread. Should always be zero.
    pub retire_leaks: AtomicU64,
    /// Audio regions that were sounding when the transport was cut (stop,
    /// seek, loop wrap, plan swap) but could not get a declick tail because
    /// every tail voice was busy. They end abruptly instead.
    pub audio_tail_overflows: AtomicU64,
}

fn zeros(n: usize) -> Box<[AtomicU32]> {
    (0..n).map(|_| AtomicU32::new(0)).collect()
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

impl Telemetry {
    pub fn new() -> Telemetry {
        Telemetry {
            analysis: crate::analysis::AnalysisTap::new(),
            playing: AtomicBool::new(false),
            beat: AtomicU64::new(0f64.to_bits()),
            cpu_load: AtomicU32::new(0),
            overruns: AtomicU64::new(0),
            plan_revision: AtomicU64::new(0),
            sample_rate: AtomicU32::new(0),
            meters: zeros(MAX_METERS * 2),
            slots: zeros(MAX_SLOTS),
            reduction: zeros(MAX_SLOTS),
            master: [AtomicU32::new(0), AtomicU32::new(0)],
            non_finite: AtomicU64::new(0),
            dropped_notes: AtomicU64::new(0),
            note_overflows: AtomicU64::new(0),
            plans_rejected: AtomicU64::new(0),
            retire_leaks: AtomicU64::new(0),
            audio_tail_overflows: AtomicU64::new(0),
        }
    }

    // ---- engine side (realtime-safe) ----

    pub fn set_beat(&self, beat: f64) {
        self.beat.store(beat.to_bits(), Ordering::Relaxed);
    }

    pub fn set_cpu_load(&self, load: f32) {
        self.cpu_load.store(load.to_bits(), Ordering::Relaxed);
    }

    pub fn push_meter(&self, meter: u16, left: f32, right: f32) {
        let i = meter as usize * 2;
        if i + 1 < self.meters.len() {
            store_max(&self.meters[i], left);
            store_max(&self.meters[i + 1], right);
        }
    }

    pub fn push_slot(&self, slot: u32, peak: f32) {
        if let Some(cell) = self.slots.get(slot as usize) {
            store_max(cell, peak);
        }
    }

    pub fn push_reduction(&self, slot: u32, db: f32) {
        if let Some(cell) = self.reduction.get(slot as usize) {
            store_max(cell, db);
        }
    }

    pub fn push_master(&self, left: f32, right: f32) {
        store_max(&self.master[0], left);
        store_max(&self.master[1], right);
    }

    // ---- UI side ----

    pub fn beat(&self) -> f64 {
        f64::from_bits(self.beat.load(Ordering::Relaxed))
    }

    pub fn cpu_load(&self) -> f32 {
        f32::from_bits(self.cpu_load.load(Ordering::Relaxed))
    }

    /// Takes (and clears) the peak of a meter since the last read.
    pub fn take_meter(&self, meter: u16) -> (f32, f32) {
        let i = meter as usize * 2;
        if i + 1 >= self.meters.len() {
            return (0.0, 0.0);
        }
        (
            f32::from_bits(self.meters[i].swap(0, Ordering::Relaxed)),
            f32::from_bits(self.meters[i + 1].swap(0, Ordering::Relaxed)),
        )
    }

    pub fn take_slot(&self, slot: u32) -> f32 {
        self.slots
            .get(slot as usize)
            .map_or(0.0, |c| f32::from_bits(c.swap(0, Ordering::Relaxed)))
    }

    /// Largest gain reduction a slot reported since the last call (dB).
    pub fn take_reduction(&self, slot: u32) -> f32 {
        self.reduction
            .get(slot as usize)
            .map_or(0.0, |c| f32::from_bits(c.swap(0, Ordering::Relaxed)))
    }

    pub fn take_master(&self) -> (f32, f32) {
        (
            f32::from_bits(self.master[0].swap(0, Ordering::Relaxed)),
            f32::from_bits(self.master[1].swap(0, Ordering::Relaxed)),
        )
    }
}

impl Default for Telemetry {
    fn default() -> Self {
        Telemetry::new()
    }
}
