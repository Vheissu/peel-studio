//! Per-slot event staging and active-note tracking.
//!
//! Everything here is sized when the engine is created. Pushing never
//! allocates; overflow has a defined outcome (see [`EventBuf`]).

use peel_dsp::{DeviceEvent, EventKind};

use crate::MAX_ACTIVE_NOTES;

/// Fixed-capacity, offset-sorted event list for one device slot and one block.
///
/// Capacity layout (all allocated up front):
///
/// * `soft_cap` entries are available to note-ons and everything else.
/// * Note-offs may use up to `hard_cap` entries, so a block full of note-ons
///   can still deliver every note-off.
/// * Two more entries are reserved for the overflow fallback (`AllNotesOff`)
///   and for panic (`AllSoundOff`).
///
/// If a note-off still cannot fit, the buffer is flagged and the engine ends
/// the block with `AllNotesOff` for the slot (and forgets the slot's tracked
/// notes), which keeps note integrity at the cost of the slot's held notes.
pub(crate) struct EventBuf {
    events: Vec<DeviceEvent>,
    soft_cap: usize,
    hard_cap: usize,
    total_cap: usize,
    /// A note-off did not fit; finish with `AllNotesOff`.
    overflow: bool,
    /// Already listed in `Staging::touched`.
    queued: bool,
}

impl EventBuf {
    fn new(soft_cap: usize) -> EventBuf {
        let soft_cap = soft_cap.max(4);
        let reserve = (soft_cap / 8).max(8);
        let hard_cap = soft_cap + reserve;
        let total_cap = hard_cap + 2;
        EventBuf {
            events: Vec::with_capacity(total_cap),
            soft_cap,
            hard_cap,
            total_cap,
            overflow: false,
            queued: false,
        }
    }

    fn reset(&mut self) {
        self.events.clear();
        self.overflow = false;
        self.queued = false;
    }
}

/// Event buffers for every device slot plus the list of slots that received
/// events this block.
pub(crate) struct Staging {
    bufs: Vec<EventBuf>,
    touched: Vec<u32>,
}

impl Staging {
    pub fn new(slots: usize, events_per_block: usize) -> Staging {
        Staging {
            bufs: (0..slots)
                .map(|_| EventBuf::new(events_per_block))
                .collect(),
            touched: Vec::with_capacity(slots),
        }
    }

    #[inline]
    fn mark(&mut self, slot: usize) {
        let buf = &mut self.bufs[slot];
        if !buf.queued {
            buf.queued = true;
            self.touched.push(slot as u32);
        }
    }

    /// Events staged for a slot this block (empty for unknown slots).
    #[inline]
    pub fn events(&self, slot: usize) -> &[DeviceEvent] {
        self.bufs.get(slot).map_or(&[], |b| b.events.as_slice())
    }

    /// Adds a note-on (or any non-release event). Returns false when the
    /// soft capacity is exhausted; the caller counts the dropped note.
    pub fn push_on(&mut self, slot: usize, offset: u32, kind: EventKind) -> bool {
        let Some(buf) = self.bufs.get_mut(slot) else {
            return false;
        };
        if buf.events.len() >= buf.soft_cap {
            return false;
        }
        buf.events.push(DeviceEvent { offset, kind });
        self.mark(slot);
        true
    }

    /// Adds a note-off. It is placed before any note-on at the same offset
    /// unless that note-on starts the very voice being released. Returns
    /// false only when even the reserve is full (the slot is then flagged
    /// for an `AllNotesOff` fallback).
    pub fn push_off(&mut self, slot: usize, offset: u32, kind: EventKind) -> bool {
        let Some(buf) = self.bufs.get_mut(slot) else {
            return false;
        };
        let voice = match kind {
            EventKind::NoteOff { voice, .. } => voice,
            _ => u32::MAX,
        };
        if buf.events.len() >= buf.hard_cap {
            buf.overflow = true;
            self.mark(slot);
            return false;
        }
        let mut pos = buf.events.len();
        while pos > 0 {
            let prev = &buf.events[pos - 1];
            if prev.offset != offset {
                break;
            }
            match prev.kind {
                EventKind::NoteOn { voice: v, .. } => {
                    if v == voice {
                        break;
                    }
                    pos -= 1;
                }
                _ => break,
            }
        }
        // `len < hard_cap <= capacity`, so this never reallocates.
        buf.events.insert(pos, DeviceEvent { offset, kind });
        self.mark(slot);
        true
    }

    /// Adds `AllNotesOff` / `AllSoundOff`. Never fails: the last entry is
    /// overwritten if the buffer is completely full.
    pub fn push_control(&mut self, slot: usize, offset: u32, kind: EventKind) {
        let Some(buf) = self.bufs.get_mut(slot) else {
            return;
        };
        let ev = DeviceEvent { offset, kind };
        if buf.events.len() >= buf.total_cap {
            if let Some(last) = buf.events.last_mut() {
                *last = ev;
            }
        } else {
            buf.events.push(ev);
        }
        self.mark(slot);
    }

    /// Resolves overflow flags: slots whose note-offs did not fit end the
    /// block with `AllNotesOff` and lose their tracked notes. Returns how
    /// many slots overflowed.
    pub fn finish_chunk(&mut self, active: &mut ActiveNotes) -> u32 {
        let mut overflows = 0;
        for i in 0..self.touched.len() {
            let slot = self.touched[i] as usize;
            let buf = &mut self.bufs[slot];
            if buf.overflow {
                let offset = buf.events.last().map_or(0, |e| e.offset);
                let ev = DeviceEvent {
                    offset,
                    kind: EventKind::AllNotesOff,
                };
                if buf.events.len() >= buf.total_cap {
                    if let Some(last) = buf.events.last_mut() {
                        *last = ev;
                    }
                } else {
                    buf.events.push(ev);
                }
                buf.overflow = false;
                active.remove_slot(slot as u32);
                overflows += 1;
            }
        }
        overflows
    }

    /// Number of slots that received events this block.
    #[inline]
    pub fn touched_len(&self) -> usize {
        self.touched.len()
    }

    #[inline]
    pub fn touched_slot(&self, i: usize) -> usize {
        self.touched[i] as usize
    }

    /// Clears every staged event. Call once the block has been rendered.
    pub fn clear(&mut self) {
        for i in 0..self.touched.len() {
            let slot = self.touched[i] as usize;
            self.bufs[slot].reset();
        }
        self.touched.clear();
    }
}

/// One sounding note the engine started and has not yet released.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ActiveNote {
    pub slot: u32,
    pub key: u8,
    pub voice: u32,
    /// Started by a `LiveNote` command rather than the sequence.
    pub live: bool,
    /// Scratch flag used while reconciling voices on plan swap.
    pub keep: bool,
}

/// Fixed-capacity table of sounding notes. Removal is O(n) and unordered;
/// n is the polyphony actually sounding, which is small.
pub(crate) struct ActiveNotes {
    notes: Vec<ActiveNote>,
}

impl ActiveNotes {
    pub fn new() -> ActiveNotes {
        ActiveNotes {
            notes: Vec::with_capacity(MAX_ACTIVE_NOTES),
        }
    }

    #[inline]
    pub fn is_full(&self) -> bool {
        self.notes.len() >= MAX_ACTIVE_NOTES
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [ActiveNote] {
        &mut self.notes
    }

    #[inline]
    pub fn get(&self, i: usize) -> ActiveNote {
        self.notes[i]
    }

    /// Adds a note. The caller checked `is_full`.
    #[inline]
    pub fn push(&mut self, note: ActiveNote) {
        if self.notes.len() < MAX_ACTIVE_NOTES {
            self.notes.push(note);
        }
    }

    #[inline]
    pub fn swap_remove(&mut self, i: usize) -> ActiveNote {
        self.notes.swap_remove(i)
    }

    /// True if a sequence note with this voice id is sounding on the slot.
    pub fn contains_voice(&self, slot: u32, voice: u32) -> bool {
        self.notes
            .iter()
            .any(|n| !n.live && n.slot == slot && n.voice == voice)
    }

    /// Removes the sequence note with this voice id on this slot.
    pub fn remove_voice(&mut self, slot: u32, voice: u32) -> Option<ActiveNote> {
        let i = self
            .notes
            .iter()
            .rposition(|n| !n.live && n.slot == slot && n.voice == voice)?;
        Some(self.notes.swap_remove(i))
    }

    /// Removes the live note for `(slot, key)`.
    pub fn remove_live(&mut self, slot: u32, key: u8) -> Option<ActiveNote> {
        let i = self
            .notes
            .iter()
            .rposition(|n| n.live && n.slot == slot && n.key == key)?;
        Some(self.notes.swap_remove(i))
    }

    /// Forgets every note on a slot (device replaced or removed).
    pub fn remove_slot(&mut self, slot: u32) {
        self.notes.retain(|n| n.slot != slot);
    }

    pub fn clear(&mut self) {
        self.notes.clear();
    }
}
