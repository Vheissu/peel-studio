//! Bounded per-note expression and MIDI MPE channel allocation.
use super::device::*;
use clap_sys::events::{
    CLAP_NOTE_EXPRESSION_BRIGHTNESS, CLAP_NOTE_EXPRESSION_PRESSURE, CLAP_NOTE_EXPRESSION_TUNING,
};
use peel_dsp::{DeviceEvent, EventKind};

#[derive(Clone, Copy)]
pub(super) struct ScopedEntry {
    pub id: i32,
    pub key: i16,
    pub pitch: f32,
    pub pressure: f32,
    pub timbre: f32,
    pub held: bool,
    pub explicit: bool,
    pub dirty: bool,
    pub age: u64,
}
#[derive(Clone, Copy, Default)]
struct Member {
    id: Option<i32>,
    key: u8,
    held: bool,
    configured: bool,
    age: u64,
}
pub(super) struct Scoped {
    pub entries: Vec<ScopedEntry>,
    pub midi_enabled: bool,
    zone_configured: bool,
    members: [Member; 15],
    age: u64,
}
impl Default for Scoped {
    fn default() -> Self {
        Self {
            entries: Vec::with_capacity(MAX_TRACKED_NOTES),
            midi_enabled: false,
            zone_configured: false,
            members: [Member::default(); 15],
            age: 0,
        }
    }
}
impl Scoped {
    pub fn reset(&mut self) {
        self.entries.clear();
        self.members.fill(Member::default());
        self.zone_configured = false;
    }
    fn entry(&self, id: i32) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }
    pub fn note_on(&mut self, id: i32, key: u8, pitch: f32, pressure: f32) -> bool {
        self.age = self.age.wrapping_add(1);
        let entry = ScopedEntry {
            id,
            key: i16::from(key),
            pitch,
            pressure,
            timbre: 0.5,
            held: true,
            explicit: false,
            dirty: false,
            age: self.age,
        };
        if let Some(i) = self.entry(id) {
            self.entries[i] = entry;
            return true;
        }
        if self.entries.len() < self.entries.capacity() {
            self.entries.push(entry);
            return true;
        }
        if let Some((i, _)) = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.held)
            .min_by_key(|(_, e)| e.age)
        {
            self.entries[i] = entry;
            return true;
        }
        false
    }
    pub fn note_off(&mut self, id: i32) {
        self.age = self.age.wrapping_add(1);
        if let Some(i) = self.entry(id) {
            self.entries[i].held = false;
            self.entries[i].age = self.age;
        }
        for member in &mut self.members {
            if member.id == Some(id) {
                member.held = false;
                member.age = self.age;
            }
        }
    }
    fn channel(&self, id: i32) -> Option<usize> {
        self.members.iter().position(|m| m.id == Some(id))
    }
}
impl ClapDevice {
    pub(super) fn mark_scoped_block(&mut self, events: &[DeviceEvent]) {
        if self.dialect == Dialect::Midi
            && events
                .iter()
                .any(|e| matches!(e.kind, EventKind::VoiceExpression { .. }))
        {
            self.scoped.midi_enabled = true;
        }
    }
    pub(super) fn midi_member_on(&mut self, time: u32, id: i32, key: u8, velocity: f32) -> bool {
        let choice = self
            .scoped
            .channel(id)
            .or_else(|| self.scoped.members.iter().position(|m| m.id.is_none()))
            .or_else(|| {
                self.scoped
                    .members
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| !m.held)
                    .min_by_key(|(_, m)| m.age)
                    .map(|(i, _)| i)
            });
        let Some(index) = choice else {
            return false;
        };
        let zone_extra = if self.scoped.zone_configured { 0 } else { 8 };
        let member_extra = if self.scoped.members[index].configured {
            0
        } else {
            6
        };
        if !self.events.has_room(zone_extra + member_extra + 4) {
            return false;
        }
        if !self.scoped.zone_configured {
            for data in [
                [0xb0, 101, 0],
                [0xb0, 100, 6],
                [0xb0, 6, 15],
                [0xb0, 101, 127],
                [0xb0, 100, 127],
                [0xe0, 0, 64],
                [0xd0, 0, 0],
                [0xb0, 74, 64],
            ] {
                self.events.push_midi(time, 0, data, false);
            }
            self.scoped.zone_configured = true;
        }
        let channel = index as u8 + 1;
        if !self.scoped.members[index].configured {
            for (cc, value) in [(101, 0), (100, 0), (6, 96), (38, 0), (101, 127), (100, 127)] {
                self.events
                    .push_midi(time, 0, [0xb0 | channel, cc, value], false);
            }
        }
        let pitch = self.expression.bend;
        let pressure = self.expression.pressure;
        let mut bend = midi_bend(pitch, 96.0);
        bend[0] |= channel;
        self.events.push_midi(time, 0, bend, false);
        self.events.push_midi(
            time,
            0,
            [0xd0 | channel, (pressure * 127.0).round() as u8, 0],
            false,
        );
        self.events
            .push_midi(time, 0, [0xb0 | channel, 74, 64], false);
        self.events.push_midi(
            time,
            0,
            [0x90 | channel, key, midi_velocity(velocity)],
            false,
        );
        self.scoped.age = self.scoped.age.wrapping_add(1);
        self.scoped.members[index] = Member {
            id: Some(id),
            key,
            held: true,
            configured: true,
            age: self.scoped.age,
        };
        true
    }
    pub(super) fn midi_member_off(&mut self, time: u32, id: i32, key: u8, velocity: f32) -> bool {
        let Some(index) = self.scoped.channel(id) else {
            return true;
        };
        self.events.push_midi(
            time,
            0,
            [
                0x80 | (index as u8 + 1),
                key,
                (unit(velocity) * 127.0).round() as u8,
            ],
            true,
        )
    }
    pub(super) fn push_scoped(
        &mut self,
        time: u32,
        voice: u32,
        pitch: f32,
        pressure: f32,
        timbre: f32,
    ) -> bool {
        if !pitch.is_finite() || !pressure.is_finite() || !timbre.is_finite() {
            return false;
        }
        let id = note_id(voice);
        let Some(index) = self.scoped.entry(id) else {
            return false;
        };
        let entry = &mut self.scoped.entries[index];
        entry.pitch = pitch.clamp(-120.0, 120.0);
        entry.pressure = pressure.clamp(0.0, 1.0);
        entry.timbre = timbre.clamp(0.0, 1.0);
        entry.explicit = true;
        entry.dirty = true;
        self.emit_scoped(time, index)
    }
    fn emit_scoped(&mut self, time: u32, index: usize) -> bool {
        let e = self.scoped.entries[index];
        if !self.events.has_room(3) {
            return false;
        }
        match self.dialect {
            Dialect::Clap => {
                self.events.push_expression(
                    time,
                    CLAP_NOTE_EXPRESSION_TUNING,
                    e.id,
                    e.key,
                    f64::from(e.pitch),
                );
                self.events.push_expression(
                    time,
                    CLAP_NOTE_EXPRESSION_PRESSURE,
                    e.id,
                    e.key,
                    f64::from(e.pressure),
                );
                self.events.push_expression(
                    time,
                    CLAP_NOTE_EXPRESSION_BRIGHTNESS,
                    e.id,
                    e.key,
                    f64::from(e.timbre),
                );
            }
            Dialect::Midi => {
                let Some(index) = self.scoped.channel(e.id) else {
                    self.scoped.entries[index].dirty = false;
                    return true;
                };
                let channel = index as u8 + 1;
                let mut bend = midi_bend(e.pitch, 96.0);
                bend[0] |= channel;
                self.events.push_midi(time, 0, bend, false);
                self.events.push_midi(
                    time,
                    0,
                    [0xd0 | channel, (e.pressure * 127.0).round() as u8, 0],
                    false,
                );
                self.events.push_midi(
                    time,
                    0,
                    [0xb0 | channel, 74, (e.timbre * 127.0).round() as u8],
                    false,
                );
            }
            Dialect::None => {}
        }
        self.scoped.entries[index].dirty = false;
        true
    }
    pub(super) fn flush_scoped(&mut self) {
        for i in 0..self.scoped.entries.len() {
            if self.scoped.entries[i].dirty && !self.emit_scoped(0, i) {
                break;
            }
        }
    }
    pub(super) fn reapply_scoped(&mut self, time: u32) {
        for i in 0..self.scoped.entries.len() {
            if self.scoped.entries[i].explicit {
                self.emit_scoped(time, i);
            }
        }
    }
    pub(super) fn scoped_reapply_count(&self) -> usize {
        if self.dialect == Dialect::Clap {
            self.scoped.entries.iter().filter(|e| e.explicit).count() * 3
        } else {
            0
        }
    }
    pub(super) fn midi_mpe_global_count(&self) -> usize {
        self.scoped
            .members
            .iter()
            .filter(|m| m.id.is_some())
            .count()
            * 3
    }
    pub(super) fn midi_mpe_global(&mut self, time: u32, kind: EventKind) -> bool {
        if !self.events.has_room(self.midi_mpe_global_count()) {
            return false;
        }
        for index in 0..15 {
            let member = self.scoped.members[index];
            let Some(id) = member.id else {
                continue;
            };
            if self
                .scoped
                .entry(id)
                .is_some_and(|i| self.scoped.entries[i].explicit)
            {
                continue;
            }
            let channel = index as u8 + 1;
            let (pitch, pressure, timbre) = match kind {
                EventKind::ResetExpression => (0.0, 0.0, 0.5),
                _ => {
                    let c = self.expression.pressure;
                    let k = self.expression.keys[usize::from(member.key)].unwrap_or(0.0);
                    (self.expression.bend, c + k - c * k, 0.5)
                }
            };
            let mut bend = midi_bend(pitch, 96.0);
            bend[0] |= channel;
            self.events.push_midi(time, 0, bend, false);
            self.events.push_midi(
                time,
                0,
                [0xd0 | channel, (pressure * 127.0).round() as u8, 0],
                false,
            );
            self.events.push_midi(
                time,
                0,
                [0xb0 | channel, 74, (timbre * 127.0_f32).round() as u8],
                false,
            );
        }
        true
    }
    pub(super) fn release_mpe_members(&mut self, time: u32, choke: bool) {
        if !self.scoped.midi_enabled {
            return;
        }
        for index in 0..15 {
            if self.scoped.members[index].id.is_some() {
                let ch = index as u8 + 1;
                self.events
                    .push_midi(time, 0, [0xb0 | ch, if choke { 120 } else { 123 }, 0], true);
            }
        }
    }
}
