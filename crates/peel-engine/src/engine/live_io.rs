//! Applies the live input queue (see [`crate::live`]) to the engine.
//!
//! Runs on the audio thread at the start of every chunk, after the command
//! queue and before the sequence is scheduled. It follows `docs/REALTIME.md`:
//! no allocation, no locks, bounded work per chunk.

use super::Engine;
use crate::live::{LIVE_TARGET_NONE, LiveAction, LiveOp, MAX_EVENTS_PER_BLOCK};
use peel_dsp::EventKind;

impl Engine {
    /// Stages the live events that belong in a chunk of `frames`.
    pub(super) fn drain_live(&mut self, frames: usize) {
        // Taking the scratch out leaves an empty (non-allocating) Vec behind
        // so the router and the engine can borrow `self` in turn.
        let mut actions = std::mem::take(&mut self.live.actions);
        for _ in 0..MAX_EVENTS_PER_BLOCK {
            let Some((event, offset)) = self.live.next(frames) else {
                break;
            };
            actions.clear();
            self.live.route(event, offset, &mut actions);
            for &action in &actions {
                self.apply_live(action);
            }
        }
        actions.clear();
        self.live.actions = actions;
        self.live.finish_chunk(frames);
    }

    fn apply_live(&mut self, action: LiveAction) {
        let offset = action.offset;
        match action.op {
            LiveOp::On {
                slot,
                key,
                velocity,
            } => self.live_note(slot, key, velocity, true, offset),
            LiveOp::Off { slot, key } => self.live_note(slot, key, 0.0, false, offset),
            LiveOp::AllOff { slot } => self.release_live_slot(slot, offset),
            LiveOp::Expression { slot, kind } => {
                if self.slots.get(slot as usize).is_some_and(Option::is_some) {
                    self.stage.push_on(slot as usize, offset, kind);
                }
            }
        }
    }

    /// Releases every live note on a slot (every slot for
    /// [`LIVE_TARGET_NONE`]) `offset` frames into the block.
    fn release_live_slot(&mut self, slot: u32, offset: u32) {
        for (index, device) in self.slots.iter().enumerate() {
            if device.is_some() && (slot == LIVE_TARGET_NONE || index == slot as usize) {
                self.stage
                    .push_on(index, offset, EventKind::ResetExpression);
            }
        }
        let mut i = 0;
        while i < self.active.len() {
            let n = self.active.get(i);
            if n.live && (slot == LIVE_TARGET_NONE || n.slot == slot) {
                let n = self.active.swap_remove(i);
                self.stage_note_off(n, offset);
            } else {
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests;
