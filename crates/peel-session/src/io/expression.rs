//! Transient performance expression; saved device parameters remain untouched.
use super::{MidiBridge, Session};
use peel_dsp::EventKind;
use peel_engine::LiveInput;
use std::sync::atomic::{AtomicU32, Ordering};

pub(super) struct ExpressionState {
    range: AtomicU32,
    bend: AtomicU32,
    pressure: AtomicU32,
    keys: [AtomicU32; 128],
}
impl Default for ExpressionState {
    fn default() -> Self {
        Self {
            range: AtomicU32::new(2.0f32.to_bits()),
            bend: AtomicU32::new(0),
            pressure: AtomicU32::new(0),
            keys: [const { AtomicU32::new(0) }; 128],
        }
    }
}
impl ExpressionState {
    pub(super) fn clear(&self) {
        self.bend.store(0, Ordering::Relaxed);
        self.pressure.store(0, Ordering::Relaxed);
        for key in &self.keys {
            key.store(0, Ordering::Relaxed);
        }
    }
    fn range(&self) -> f32 {
        f32::from_bits(self.range.load(Ordering::Relaxed))
    }
    pub(super) fn push(&self, live: &LiveInput, kind: EventKind) -> bool {
        let (kind, value, destination) = match kind {
            EventKind::PitchBend {
                semitones: normalized,
            } if normalized.is_finite() => {
                let value = normalized.clamp(-1.0, 1.0);
                (
                    EventKind::PitchBend {
                        semitones: value * self.range(),
                    },
                    value,
                    &self.bend,
                )
            }
            EventKind::ChannelPressure { value } if value.is_finite() => {
                let value = value.clamp(0.0, 1.0);
                (EventKind::ChannelPressure { value }, value, &self.pressure)
            }
            EventKind::PolyPressure { key, value } if key < 128 && value.is_finite() => {
                let value = value.clamp(0.0, 1.0);
                (
                    EventKind::PolyPressure { key, value },
                    value,
                    &self.keys[usize::from(key)],
                )
            }
            EventKind::ResetExpression => {
                if !live.expression(kind) {
                    return false;
                }
                self.clear();
                return true;
            }
            _ => return false,
        };
        if !live.expression(kind) {
            return false;
        }
        destination.store(value.to_bits(), Ordering::Relaxed);
        true
    }
}
impl MidiBridge {
    pub(crate) fn clear_key_pressure(&self, key: u8) {
        if let Some(value) = self.expression.keys.get(usize::from(key)) {
            value.store(0, Ordering::Relaxed);
        }
    }
    pub(super) fn change_target(&self, target: &peel_engine::LiveTarget, slot: Option<u32>) {
        let guard = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(live) = guard.as_ref().filter(|_| target.get().is_some()) {
            live.sustain(false);
            live.all_notes_off(peel_engine::LIVE_TARGET_NONE);
        }
        target.set(slot);
        self.expression.clear();
        if let Some(live) = guard.as_ref() {
            live.expression_batch(&[
                EventKind::ResetExpression,
                EventKind::PitchBendRange {
                    semitones: self.expression.range(),
                },
            ]);
        }
    }
    fn expression(&self, kind: EventKind) -> bool {
        let guard = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .as_ref()
            .is_some_and(|live| self.expression.push(live, kind))
    }
}
impl Session {
    /// Sets pitch-wheel sensitivity in semitones, shared by hardware and software input.
    pub fn set_pitch_bend_range(&mut self, semitones: f32) -> Result<(), String> {
        if !semitones.is_finite() || !(0.0..=48.0).contains(&semitones) {
            return Err("Pitch bend range must be between 0 and 48 semitones".into());
        }
        let bridge = &self.io.bridge;
        let guard = bridge
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(live) = guard.as_ref().filter(|live| live.target().get().is_some())
            && !live.expression_batch(&[
                EventKind::PitchBendRange { semitones },
                EventKind::PitchBend {
                    semitones: self.pitch_bend_value() * semitones,
                },
            ])
        {
            return Err("Live expression queue is full".into());
        }
        bridge
            .expression
            .range
            .store(semitones.to_bits(), Ordering::Relaxed);
        Ok(())
    }
    pub fn pitch_bend_range(&self) -> f32 {
        self.io.bridge.expression.range()
    }
    /// Queues a normalized pitch-wheel value (-1..1) for the selected instrument.
    pub fn pitch_bend(&mut self, value: f32) -> bool {
        self.io
            .bridge
            .expression(EventKind::PitchBend { semitones: value })
    }
    pub fn channel_pressure(&mut self, value: f32) -> bool {
        self.io
            .bridge
            .expression(EventKind::ChannelPressure { value })
    }
    pub fn poly_pressure(&mut self, key: u8, value: f32) -> bool {
        self.io
            .bridge
            .expression(EventKind::PolyPressure { key, value })
    }
    /// Centers expression without releasing notes or changing the bend range.
    pub fn reset_expression(&mut self) -> bool {
        self.io.bridge.expression(EventKind::ResetExpression)
    }
    pub fn pitch_bend_value(&self) -> f32 {
        f32::from_bits(self.io.bridge.expression.bend.load(Ordering::Relaxed))
    }
    pub fn channel_pressure_value(&self) -> f32 {
        f32::from_bits(self.io.bridge.expression.pressure.load(Ordering::Relaxed))
    }
    pub fn poly_pressure_value(&self, key: u8) -> f32 {
        self.io
            .bridge
            .expression
            .keys
            .get(usize::from(key))
            .map_or(0.0, |key| f32::from_bits(key.load(Ordering::Relaxed)))
    }
}
