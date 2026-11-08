//! The audio-side view of a plugin's parameters: a fixed table the process
//! path searches without allocating, and the queue of pending `set_param`
//! changes.

use crate::events::EventList;

/// One exposed parameter as the audio thread needs it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ParamEntry {
    pub(crate) id: u32,
    /// Opaque pointer the plugin gave in its parameter info; handed back in
    /// every parameter event so the plugin can skip a lookup. Stored as an
    /// integer so the table stays `Send`.
    pub(crate) cookie: usize,
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) stepped: bool,
}

impl ParamEntry {
    /// Clamps (and for stepped parameters rounds) a plain value.
    pub(crate) fn clamp(&self, value: f64) -> f64 {
        let v = value.clamp(self.min, self.max);
        if self.stepped { v.round() } else { v }
    }
}

/// Parameters in plugin order plus an id index, both immutable after load.
pub(crate) struct ParamTable {
    entries: Box<[ParamEntry]>,
    /// `(clap id, index into entries)`, sorted by id.
    by_id: Box<[(u32, u32)]>,
}

impl ParamTable {
    pub(crate) fn new(entries: Vec<ParamEntry>) -> ParamTable {
        let mut by_id: Vec<(u32, u32)> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id, i as u32))
            .collect();
        by_id.sort_unstable();
        ParamTable {
            entries: entries.into_boxed_slice(),
            by_id: by_id.into_boxed_slice(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&ParamEntry> {
        self.entries.get(index)
    }

    /// Binary search; no allocation, safe on the audio thread.
    pub(crate) fn index_of(&self, id: u32) -> Option<usize> {
        self.by_id
            .binary_search_by_key(&id, |&(key, _)| key)
            .ok()
            .and_then(|i| self.by_id.get(i))
            .map(|&(_, index)| index as usize)
    }
}

/// `set_param` calls waiting for the next `process`.
///
/// Keeps only the latest value per parameter, so it can never overflow: its
/// capacity is the parameter count. Order of first change is preserved.
pub(crate) struct PendingParams {
    values: Box<[f64]>,
    queued: Box<[bool]>,
    order: Box<[u32]>,
    len: usize,
}

impl PendingParams {
    pub(crate) fn new(params: usize) -> PendingParams {
        PendingParams {
            values: vec![0.0; params].into_boxed_slice(),
            queued: vec![false; params].into_boxed_slice(),
            order: vec![0; params].into_boxed_slice(),
            len: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn clear(&mut self) {
        for &index in &self.order[..self.len] {
            self.queued[index as usize] = false;
        }
        self.len = 0;
    }

    pub(crate) fn set(&mut self, index: usize, value: f64) {
        let (Some(v), Some(q)) = (self.values.get_mut(index), self.queued.get_mut(index)) else {
            return;
        };
        *v = value;
        if !*q {
            *q = true;
            self.order[self.len] = index as u32;
            self.len += 1;
        }
    }

    /// Moves every pending change into `events` at time 0 and empties the
    /// queue.
    pub(crate) fn drain_into(&mut self, table: &ParamTable, events: &mut EventList) {
        for k in 0..self.len {
            let index = self.order[k] as usize;
            self.queued[index] = false;
            if let Some(entry) = table.get(index) {
                events.push_param(0, entry.id, entry.cookie, self.values[index], true);
            }
        }
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> ParamTable {
        let e = |id, min, max, stepped| ParamEntry {
            id,
            cookie: id as usize * 10,
            min,
            max,
            stepped,
        };
        ParamTable::new(vec![
            e(900, 0.0, 1.0, false),
            e(7, -1.0, 1.0, false),
            e(42, 0.0, 3.0, true),
        ])
    }

    #[test]
    fn ids_map_to_plugin_order() {
        let t = table();
        assert_eq!(t.len(), 3);
        assert_eq!(t.index_of(900), Some(0));
        assert_eq!(t.index_of(7), Some(1));
        assert_eq!(t.index_of(42), Some(2));
        assert_eq!(t.index_of(8), None);
    }

    #[test]
    fn entries_clamp_and_round() {
        let t = table();
        assert_eq!(t.get(1).unwrap().clamp(5.0), 1.0);
        assert_eq!(t.get(2).unwrap().clamp(1.6), 2.0);
        assert_eq!(t.get(2).unwrap().clamp(-4.0), 0.0);
    }

    #[test]
    fn pending_coalesces_and_drains_in_order() {
        let t = table();
        let mut pending = PendingParams::new(t.len());
        pending.set(1, 0.25);
        pending.set(0, 0.5);
        pending.set(1, 0.75);
        assert!(!pending.is_empty());

        let mut events = EventList::with_capacity(8, 2);
        pending.drain_into(&t, &mut events);
        assert!(pending.is_empty());
        assert_eq!(events.len(), 2);
        let first = events.param_at(0).unwrap();
        assert_eq!((first.param_id, first.value), (7, 0.75));
        assert_eq!(first.cookie as usize, 70);
        let second = events.param_at(1).unwrap();
        assert_eq!((second.param_id, second.value), (900, 0.5));

        // A cleared queue can be filled again.
        pending.set(2, 3.0);
        pending.clear();
        assert!(pending.is_empty());
        pending.set(2, 1.0);
        events.clear();
        pending.drain_into(&t, &mut events);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn out_of_range_index_is_ignored() {
        let mut pending = PendingParams::new(2);
        pending.set(5, 1.0);
        assert!(pending.is_empty());
    }
}
