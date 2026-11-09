//! Applies the live input queue (see [`crate::live`]) to the engine.
//!
//! Runs on the audio thread at the start of every chunk, after the command
//! queue and before the sequence is scheduled. It follows `docs/REALTIME.md`:
//! no allocation, no locks, bounded work per chunk.

use super::{Engine, clean_velocity, stage_off};
use crate::events::ActiveNote;
use crate::live::{LIVE_TARGET_NONE, LiveAction, LiveOp, MAX_EVENTS_PER_BLOCK};
use peel_dsp::EventKind;

impl Engine {
    /// Stages the live events that belong in a chunk of `frames`.
    pub(super) fn drain_live(&mut self, frames: usize) {
        // Keep deferred order: a reset must not leapfrog a newer wheel value.
        for (slot, kind) in &mut self.live.deferred_expression {
            if !self.slots.get(*slot as usize).is_some_and(Option::is_some)
                || self.stage.push_on(*slot as usize, 0, *kind)
            {
                *slot = LIVE_TARGET_NONE;
            }
        }
        self.live
            .deferred_expression
            .retain(|(slot, _)| *slot != LIVE_TARGET_NONE);
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
            LiveOp::VoiceOn {
                slot,
                key,
                velocity,
                voice,
                pitch,
                pressure,
                timbre,
            } => {
                if self.scoped_live_note(slot, key, velocity, voice, true, offset) {
                    // One queued operation owns both events, even at the chunk's
                    // live-event budget boundary. The note preflights two slots.
                    self.stage.push_on(
                        slot as usize,
                        offset,
                        EventKind::VoiceExpression {
                            voice,
                            pitch,
                            pressure,
                            timbre,
                        },
                    );
                }
            }
            LiveOp::VoiceOff { slot, key, voice } => {
                self.scoped_live_note(slot, key, 0.0, voice, false, offset);
            }
            LiveOp::On {
                slot,
                key,
                velocity,
            } => self.live_note(slot, key, velocity, true, offset),
            LiveOp::Off { slot, key } => self.live_note(slot, key, 0.0, false, offset),
            LiveOp::AllOff { slot } => self.release_live_slot(slot, offset),
            LiveOp::Expression { slot, kind } => {
                if self.slots.get(slot as usize).is_some_and(Option::is_some) {
                    if self.stage.push_on(slot as usize, offset, kind) {
                        self.live.clear_deferred_expression(slot, kind);
                    } else {
                        self.live.defer_expression(slot, kind);
                    }
                }
            }
        }
    }

    fn scoped_live_note(
        &mut self,
        slot: u32,
        key: u8,
        velocity: f32,
        voice: u32,
        on: bool,
        offset: u32,
    ) -> bool {
        let s = slot as usize;
        if on {
            if !self.slots.get(s).is_some_and(Option::is_some) {
                return false;
            }
            if let Some(previous) = self.active.remove_live_voice(slot, voice) {
                self.stage_note_off(previous, offset);
            }
            if self.active.is_full()
                || !self.stage.has_on_room(s, 2)
                || !self.stage.push_on(
                    s,
                    offset,
                    EventKind::NoteOn {
                        key,
                        velocity: clean_velocity(velocity),
                        voice,
                    },
                )
            {
                self.telemetry
                    .dropped_notes
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return false;
            }
            self.active.push(ActiveNote {
                slot,
                key,
                voice,
                source_voice: 0,
                live: true,
                keep: true,
            });
        } else if self.active.remove_live_voice(slot, voice).is_some() {
            stage_off(
                &mut self.stage,
                &mut self.active,
                &self.telemetry,
                s,
                offset,
                EventKind::NoteOff {
                    key,
                    velocity: 0.0,
                    voice,
                },
            );
        }
        true
    }

    /// Releases every live note on a slot (every slot for
    /// [`LIVE_TARGET_NONE`]) `offset` frames into the block.
    fn release_live_slot(&mut self, slot: u32, offset: u32) {
        for (index, device) in self.slots.iter().enumerate() {
            if device.is_some()
                && (slot == LIVE_TARGET_NONE || index == slot as usize)
                && !self
                    .stage
                    .push_on(index, offset, EventKind::ResetExpression)
            {
                self.live
                    .defer_expression(index as u32, EventKind::ResetExpression);
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
