//! Audio-thread side of the retirement queue.
//!
//! Plans, devices and samples replaced on the audio thread are handed back
//! through a bounded queue and freed by the control thread. If the queue is
//! full the object is parked in a small preallocated array and re-offered on
//! later blocks. Only if that array is also full is the object leaked, which
//! is preferable to freeing memory on the audio thread; it is counted in
//! [`Telemetry::retire_leaks`](crate::Telemetry::retire_leaks).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rtrb::{Producer, PushError};

use crate::command::Retired;
use crate::telemetry::Telemetry;

/// Objects that can wait for the control thread when the queue is full.
pub(crate) const PENDING_RETIRE: usize = 256;

pub(crate) struct Retirer {
    tx: Producer<Retired>,
    pending: Vec<Option<Retired>>,
    pending_count: usize,
    telemetry: Arc<Telemetry>,
}

impl Retirer {
    pub fn new(tx: Producer<Retired>, telemetry: Arc<Telemetry>) -> Retirer {
        Retirer {
            tx,
            pending: (0..PENDING_RETIRE).map(|_| None).collect(),
            pending_count: 0,
            telemetry,
        }
    }

    /// Whether ownership-critical transfers can be accepted without leaking.
    pub fn can_accept(&self) -> bool {
        self.pending_count < self.pending.len() || self.tx.slots() > 0
    }

    /// Hands an object to the control thread. Never frees it here.
    pub fn retire(&mut self, item: Retired) {
        if self.pending_count > 0 {
            self.flush();
        }
        if self.pending_count == 0 {
            match self.tx.push(item) {
                Ok(()) => {}
                Err(PushError::Full(item)) => self.park(item),
            }
        } else {
            self.park(item);
        }
    }

    fn park(&mut self, item: Retired) {
        for slot in self.pending.iter_mut() {
            if slot.is_none() {
                *slot = Some(item);
                self.pending_count += 1;
                return;
            }
        }
        self.telemetry.retire_leaks.fetch_add(1, Ordering::Relaxed);
        std::mem::forget(item);
    }

    /// Re-offers parked objects to the queue. Cheap when nothing is parked.
    pub fn flush(&mut self) {
        if self.pending_count == 0 {
            return;
        }
        for slot in self.pending.iter_mut() {
            if let Some(item) = slot.take() {
                match self.tx.push(item) {
                    Ok(()) => self.pending_count -= 1,
                    Err(PushError::Full(item)) => {
                        *slot = Some(item);
                        break;
                    }
                }
            }
        }
    }
}
