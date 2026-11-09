//! Preallocated CLAP event lists.
//!
//! The input list is a fixed array of event slots. Slots are a union of every
//! event shape the host sends, so each one is the size of the largest and the
//! list never allocates after construction. A slot's address is what the
//! plugin gets from `get`, and `header.size` tells it how much of the slot is
//! meaningful.
//!
//! When the list fills up, note-ons and parameter changes stop being accepted
//! before the slots reserved for note-offs and panic events do, following
//! the engine's overflow rule that releases are never dropped.

use std::ffi::c_void;

use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_EXPRESSION, CLAP_EVENT_PARAM_VALUE,
    clap_event_header, clap_event_midi, clap_event_note, clap_event_note_expression,
    clap_event_param_value, clap_input_events, clap_output_events,
};

use crate::instance::PluginShared;

#[repr(C)]
#[derive(Clone, Copy)]
union EventSlot {
    header: clap_event_header,
    note: clap_event_note,
    expression: clap_event_note_expression,
    param: clap_event_param_value,
    midi: clap_event_midi,
}

fn header(size: usize, time: u32, type_: u16) -> clap_event_header {
    clap_event_header {
        size: size as u32,
        time,
        space_id: CLAP_CORE_EVENT_SPACE_ID,
        type_,
        flags: 0,
    }
}

/// A bounded, reusable list of events for one `process` call.
pub(crate) struct EventList {
    slots: Box<[EventSlot]>,
    len: usize,
    /// Slots only releases may use.
    reserve: usize,
    last_time: u32,
}

impl EventList {
    pub(crate) fn with_capacity(capacity: usize, reserve: usize) -> EventList {
        let capacity = capacity.max(2);
        // SAFETY: every union field is plain data for which all-zero bytes
        // are valid (integers, floats, null pointers).
        let zero: EventSlot = unsafe { std::mem::zeroed() };
        EventList {
            slots: vec![zero; capacity].into_boxed_slice(),
            len: 0,
            reserve: reserve.min(capacity - 1),
            last_time: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.len = 0;
        self.last_time = 0;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Time of the last event. New events are never placed before it, which
    /// keeps the list sorted even if the caller's offsets are not.
    #[cfg(test)]
    pub(crate) fn last_time(&self) -> u32 {
        self.last_time
    }

    /// Reserves a complete noncritical transaction before any messages are written.
    pub(crate) fn has_room(&self, count: usize) -> bool {
        count <= (self.slots.len() - self.reserve).saturating_sub(self.len)
    }

    pub(crate) fn push_expression(
        &mut self,
        time: u32,
        expression_id: i32,
        note_id: i32,
        key: i16,
        value: f64,
    ) -> bool {
        let Some((slot, time)) = self.slot(false, time) else {
            return false;
        };
        slot.expression = clap_event_note_expression {
            header: header(
                size_of::<clap_event_note_expression>(),
                time,
                CLAP_EVENT_NOTE_EXPRESSION,
            ),
            expression_id,
            note_id,
            port_index: 0,
            channel: 0,
            key,
            value,
        };
        true
    }

    #[cfg(test)]
    pub(crate) fn expression_at(&self, index: usize) -> Option<clap_event_note_expression> {
        (index < self.len).then(|| unsafe { self.slots[index].expression })
    }

    fn slot(&mut self, critical: bool, time: u32) -> Option<(&mut EventSlot, u32)> {
        let limit = if critical {
            self.slots.len()
        } else {
            self.slots.len() - self.reserve
        };
        if self.len >= limit {
            return None;
        }
        let time = time.max(self.last_time);
        self.last_time = time;
        let index = self.len;
        self.len += 1;
        Some((&mut self.slots[index], time))
    }

    /// Pushes a note event of the given CLAP type. `-1` is the wildcard for
    /// `note_id`, `port`, `channel` and `key`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_note(
        &mut self,
        type_: u16,
        time: u32,
        note_id: i32,
        port: i16,
        channel: i16,
        key: i16,
        velocity: f64,
        critical: bool,
    ) -> bool {
        let Some((slot, time)) = self.slot(critical, time) else {
            return false;
        };
        slot.note = clap_event_note {
            header: header(size_of::<clap_event_note>(), time, type_),
            note_id,
            port_index: port,
            channel,
            key,
            velocity,
        };
        true
    }

    /// Pushes a parameter value for the whole plugin (no note scope).
    pub(crate) fn push_param(
        &mut self,
        time: u32,
        param_id: u32,
        cookie: usize,
        value: f64,
        critical: bool,
    ) -> bool {
        let Some((slot, time)) = self.slot(critical, time) else {
            return false;
        };
        slot.param = clap_event_param_value {
            header: header(
                size_of::<clap_event_param_value>(),
                time,
                CLAP_EVENT_PARAM_VALUE,
            ),
            param_id,
            cookie: cookie as *mut c_void,
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value,
        };
        true
    }

    /// Pushes a three byte MIDI 1.0 message.
    pub(crate) fn push_midi(
        &mut self,
        time: u32,
        port: u16,
        data: [u8; 3],
        critical: bool,
    ) -> bool {
        let Some((slot, time)) = self.slot(critical, time) else {
            return false;
        };
        slot.midi = clap_event_midi {
            header: header(
                size_of::<clap_event_midi>(),
                time,
                clap_sys::events::CLAP_EVENT_MIDI,
            ),
            port_index: port,
            data,
        };
        true
    }

    /// The header of event `index`, as the plugin reads it.
    #[cfg(test)]
    pub(crate) fn header_at(&self, index: usize) -> Option<clap_event_header> {
        // SAFETY: every variant starts with a `clap_event_header`.
        (index < self.len).then(|| unsafe { self.slots[index].header })
    }

    #[cfg(test)]
    pub(crate) fn param_at(&self, index: usize) -> Option<clap_event_param_value> {
        // SAFETY: only called on slots written by `push_param`.
        (index < self.len).then(|| unsafe { self.slots[index].param })
    }

    #[cfg(test)]
    pub(crate) fn midi_at(&self, index: usize) -> Option<clap_event_midi> {
        // SAFETY: only called on slots written by `push_midi`.
        (index < self.len).then(|| unsafe { self.slots[index].midi })
    }
}

/// A `clap_input_events` reading `list`. Valid while `list` is not moved or
/// mutated.
pub(crate) fn input_events(list: &EventList) -> clap_input_events {
    clap_input_events {
        ctx: (list as *const EventList).cast_mut().cast(),
        size: Some(input_size),
        get: Some(input_get),
    }
}

fn list_of<'a>(events: *const clap_input_events) -> Option<&'a EventList> {
    // SAFETY: the plugin passes back the struct we gave it, whose `ctx` is
    // the `EventList` it was built from.
    let ctx = unsafe { events.as_ref() }?.ctx;
    unsafe { ctx.cast::<EventList>().as_ref() }
}

extern "C" fn input_size(events: *const clap_input_events) -> u32 {
    list_of(events).map_or(0, |l| l.len as u32)
}

extern "C" fn input_get(events: *const clap_input_events, index: u32) -> *const clap_event_header {
    match list_of(events) {
        Some(l) if (index as usize) < l.len => {
            (&l.slots[index as usize] as *const EventSlot).cast()
        }
        _ => std::ptr::null(),
    }
}

/// A `clap_output_events` that applies parameter changes made by the plugin
/// (its own GUI, internal modulation, preset loads) to `shared`'s parameter
/// mirror. Other event types are accepted and ignored.
pub(crate) fn output_events(shared: &PluginShared) -> clap_output_events {
    clap_output_events {
        ctx: (shared as *const PluginShared).cast_mut().cast(),
        try_push: Some(output_try_push),
    }
}

extern "C" fn output_try_push(
    events: *const clap_output_events,
    event: *const clap_event_header,
) -> bool {
    // SAFETY: as in `list_of`; `ctx` is the `PluginShared` that built it.
    let Some(shared) = (unsafe {
        events
            .as_ref()
            .and_then(|e| e.ctx.cast::<PluginShared>().as_ref())
    }) else {
        return false;
    };
    // SAFETY: the plugin passes a valid event for the duration of the call.
    let Some(head) = (unsafe { event.as_ref() }) else {
        return false;
    };
    if head.space_id == CLAP_CORE_EVENT_SPACE_ID
        && head.type_ == CLAP_EVENT_PARAM_VALUE
        && head.size as usize >= size_of::<clap_event_param_value>()
    {
        // SAFETY: the size check above covers the whole struct.
        let ev = unsafe { std::ptr::read_unaligned(event.cast::<clap_event_param_value>()) };
        shared.apply_plugin_param(ev.param_id, ev.value);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap_sys::events::{CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON};

    #[test]
    fn plugin_reads_events_through_the_callbacks() {
        let mut list = EventList::with_capacity(8, 2);
        assert!(list.push_note(CLAP_EVENT_NOTE_ON, 4, 7, 0, 0, 60, 0.5, false));
        assert!(list.push_param(9, 12, 0xABCD, 0.25, false));
        let ffi = input_events(&list);
        let size = ffi.size.unwrap();
        let get = ffi.get.unwrap();
        assert_eq!(unsafe { size(&ffi) }, 2);

        let first = unsafe { &*get(&ffi, 0) };
        assert_eq!((first.type_, first.time), (CLAP_EVENT_NOTE_ON, 4));
        assert_eq!(first.size as usize, size_of::<clap_event_note>());
        let note = unsafe { &*(first as *const clap_event_header as *const clap_event_note) };
        assert_eq!((note.note_id, note.key, note.velocity), (7, 60, 0.5));

        let second = unsafe { &*get(&ffi, 1) };
        assert_eq!((second.type_, second.time), (CLAP_EVENT_PARAM_VALUE, 9));
        let param = list.param_at(1).unwrap();
        assert_eq!(param.param_id, 12);
        assert_eq!(param.cookie as usize, 0xABCD);
        assert_eq!(param.value, 0.25);

        assert!(unsafe { get(&ffi, 2) }.is_null());
    }

    #[test]
    fn releases_keep_their_reserve() {
        let mut list = EventList::with_capacity(6, 2);
        for i in 0..4 {
            assert!(list.push_note(CLAP_EVENT_NOTE_ON, 0, i, 0, 0, 60, 1.0, false));
        }
        // Full for note-ons and params, still open for releases.
        assert!(!list.push_note(CLAP_EVENT_NOTE_ON, 0, 9, 0, 0, 60, 1.0, false));
        assert!(!list.push_param(0, 1, 0, 0.0, false));
        assert!(list.push_note(CLAP_EVENT_NOTE_OFF, 0, 0, 0, 0, 60, 0.0, true));
        assert!(list.push_note(CLAP_EVENT_NOTE_OFF, 0, 1, 0, 0, 60, 0.0, true));
        assert!(!list.push_note(CLAP_EVENT_NOTE_OFF, 0, 2, 0, 0, 60, 0.0, true));
        assert_eq!(list.len(), list.capacity());
        list.clear();
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn times_never_go_backwards() {
        let mut list = EventList::with_capacity(8, 1);
        list.push_midi(10, 0, [0x90, 60, 100], false);
        list.push_midi(3, 0, [0x80, 60, 0], false);
        assert_eq!(list.header_at(0).unwrap().time, 10);
        assert_eq!(list.header_at(1).unwrap().time, 10);
        assert_eq!(list.midi_at(1).unwrap().data, [0x80, 60, 0]);
        assert_eq!(list.last_time(), 10);
    }

    #[test]
    fn filling_and_clearing_never_allocates_more() {
        let mut list = EventList::with_capacity(16, 4);
        let ptr = list.slots.as_ptr();
        for round in 0..100 {
            for i in 0..40 {
                list.push_note(CLAP_EVENT_NOTE_ON, i, round, 0, 0, 60, 1.0, false);
            }
            list.clear();
        }
        assert_eq!(list.slots.as_ptr(), ptr);
    }
}
