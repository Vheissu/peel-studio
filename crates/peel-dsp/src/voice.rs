//! Voice allocation with stealing.
//!
//! The allocator tracks bookkeeping only; the instrument owns the voices and
//! keeps a slot's state current. A pool has `capacity` physical slots but only
//! `polyphony` of them may be *sounding* (held or releasing) at once. When a
//! new note needs a slot and the limit is reached, the allocator picks a
//! victim and marks it [`VoiceState::Fading`]; the instrument then fades that
//! voice out over a few milliseconds while the new note starts in a different
//! physical slot, so a steal never clicks. Fading voices do not count towards
//! polyphony. If every slot is busy, the quietest fading voice is cut.

/// Lifecycle of a voice slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceState {
    Free,
    /// Key is down (or the voice has no release stage and plays out).
    Held,
    /// Note-off received; the voice is in its release stage.
    Releasing,
    /// Stolen; fading out quickly. Not counted against polyphony.
    Fading,
}

/// How a victim is chosen when the voice limit is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StealPolicy {
    /// The voice that started earliest.
    Oldest,
    /// The voice with the lowest reported level (ties go to the oldest).
    Quietest,
}

#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub state: VoiceState,
    pub key: u8,
    /// Host voice id from the note event.
    pub id: u32,
    /// Monotonic start order.
    pub age: u64,
    /// Last level reported by the instrument, used by [`StealPolicy::Quietest`].
    pub level: f32,
}

impl Slot {
    const FREE: Slot = Slot {
        state: VoiceState::Free,
        key: 0,
        id: 0,
        age: 0,
        level: 0.0,
    };
}

/// Result of [`VoiceAllocator::allocate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allocation {
    /// Slot the new note should start in (already marked held).
    pub slot: usize,
    /// A voice that was stolen and must now be faded out (already marked
    /// fading). `None` if no voice had to be displaced.
    pub stolen: Option<usize>,
    /// A fading voice that had to be cut hard because the pool was full of
    /// fading voices. The instrument should reset it immediately.
    pub cut: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct VoiceAllocator {
    slots: Vec<Slot>,
    polyphony: usize,
    policy: StealPolicy,
    counter: u64,
}

impl VoiceAllocator {
    /// Creates a pool. `capacity` is raised to at least `polyphony`; extra
    /// slots hold fading voices.
    pub fn new(capacity: usize, polyphony: usize, policy: StealPolicy) -> VoiceAllocator {
        let polyphony = polyphony.max(1);
        let capacity = capacity.max(polyphony);
        VoiceAllocator {
            slots: vec![Slot::FREE; capacity],
            polyphony,
            policy,
            counter: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    pub fn polyphony(&self) -> usize {
        self.polyphony
    }

    pub fn set_polyphony(&mut self, polyphony: usize) {
        self.polyphony = polyphony.clamp(1, self.slots.len());
    }

    pub fn set_policy(&mut self, policy: StealPolicy) {
        self.policy = policy;
    }

    pub fn slot(&self, i: usize) -> &Slot {
        &self.slots[i]
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// Frees every slot.
    pub fn reset(&mut self) {
        self.slots.fill(Slot::FREE);
        self.counter = 0;
    }

    /// Number of held or releasing voices (what polyphony limits).
    pub fn sounding(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| matches!(s.state, VoiceState::Held | VoiceState::Releasing))
            .count()
    }

    /// Number of slots in use, including fading voices.
    pub fn in_use(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.state != VoiceState::Free)
            .count()
    }

    pub fn is_idle(&self) -> bool {
        self.in_use() == 0
    }

    /// Chooses a slot for a new note, stealing if needed.
    pub fn allocate(&mut self, key: u8, id: u32) -> Allocation {
        let mut stolen = None;
        let mut cut = None;
        if self.sounding() >= self.polyphony
            && let Some(v) = self.pick_victim()
        {
            self.slots[v].state = VoiceState::Fading;
            stolen = Some(v);
        }
        let slot = match self.slots.iter().position(|s| s.state == VoiceState::Free) {
            Some(i) => i,
            None => {
                // Pool exhausted: cut the quietest fading voice (or, failing
                // that, the oldest voice of any kind).
                let victim = self.pick_cut();
                cut = Some(victim);
                victim
            }
        };
        self.counter += 1;
        self.slots[slot] = Slot {
            state: VoiceState::Held,
            key,
            id,
            age: self.counter,
            level: 1.0,
        };
        Allocation { slot, stolen, cut }
    }

    fn pick_victim(&self) -> Option<usize> {
        // Prefer voices that are already releasing; they are the least
        // audible to lose. Otherwise take from the held voices.
        let releasing = self.slots.iter().any(|s| s.state == VoiceState::Releasing);
        let wanted = if releasing {
            VoiceState::Releasing
        } else {
            VoiceState::Held
        };
        let mut best: Option<usize> = None;
        for (i, s) in self.slots.iter().enumerate() {
            if s.state != wanted {
                continue;
            }
            best = match best {
                None => Some(i),
                Some(b) => {
                    let cur = &self.slots[b];
                    let better = match self.policy {
                        StealPolicy::Oldest => s.age < cur.age,
                        StealPolicy::Quietest => {
                            s.level < cur.level || (s.level == cur.level && s.age < cur.age)
                        }
                    };
                    if better { Some(i) } else { Some(b) }
                }
            };
        }
        best
    }

    fn pick_cut(&self) -> usize {
        let mut best = 0;
        let mut best_key = (u8::MAX, f32::MAX, u64::MAX);
        for (i, s) in self.slots.iter().enumerate() {
            let rank = match s.state {
                VoiceState::Fading => 0,
                VoiceState::Releasing => 1,
                _ => 2,
            };
            let key = (rank, s.level, s.age);
            if key.0 < best_key.0
                || (key.0 == best_key.0
                    && (key.1 < best_key.1 || (key.1 == best_key.1 && key.2 < best_key.2)))
            {
                best_key = key;
                best = i;
            }
        }
        best
    }

    /// Marks a held voice as releasing. Returns false if it was not held.
    pub fn release(&mut self, slot: usize) -> bool {
        match self.slots.get_mut(slot) {
            Some(s) if s.state == VoiceState::Held => {
                s.state = VoiceState::Releasing;
                true
            }
            _ => false,
        }
    }

    /// Marks a voice as fading (for example when it is choked), so it stops
    /// counting against polyphony.
    pub fn mark_fading(&mut self, slot: usize) {
        if let Some(s) = self.slots.get_mut(slot)
            && s.state != VoiceState::Free
        {
            s.state = VoiceState::Fading;
        }
    }

    /// Marks every held voice releasing.
    pub fn release_all(&mut self) {
        for s in &mut self.slots {
            if s.state == VoiceState::Held {
                s.state = VoiceState::Releasing;
            }
        }
    }

    pub fn free(&mut self, slot: usize) {
        if let Some(s) = self.slots.get_mut(slot) {
            *s = Slot::FREE;
        }
    }

    /// Updates the level used for quietest-voice stealing.
    pub fn set_level(&mut self, slot: usize, level: f32) {
        if let Some(s) = self.slots.get_mut(slot) {
            s.level = level;
        }
    }

    /// Slots currently holding `id` in the held state (for note-off).
    /// Writes up to `out.len()` slot indices and returns how many were found.
    pub fn find_held(&self, id: u32, out: &mut [usize]) -> usize {
        let mut n = 0;
        for (i, s) in self.slots.iter().enumerate() {
            if s.state == VoiceState::Held && s.id == id && n < out.len() {
                out[n] = i;
                n += 1;
            }
        }
        n
    }
}

/// Linear fade-out used when a voice is stolen or choked, so the cut never
/// clicks. Multiply the voice output by [`StealFade::tick`] every sample and
/// retire the voice once [`StealFade::is_done`].
#[derive(Clone, Copy, Debug)]
pub struct StealFade {
    gain: f32,
    step: f32,
    fading: bool,
}

impl StealFade {
    pub const fn new() -> StealFade {
        StealFade {
            gain: 1.0,
            step: 0.0,
            fading: false,
        }
    }

    /// Back to full gain, not fading (call when a voice starts).
    pub fn reset(&mut self) {
        self.gain = 1.0;
        self.fading = false;
    }

    /// Begins the fade over `ms` milliseconds. Repeated calls keep the first.
    pub fn start(&mut self, sample_rate: f32, ms: f32) {
        if !self.fading {
            self.fading = true;
            self.step = 1.0 / (ms * 0.001 * sample_rate).max(1.0);
        }
    }

    /// Returns the gain for this sample and advances.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.fading {
            self.gain = (self.gain - self.step).max(0.0);
        }
        self.gain
    }

    pub fn is_fading(&self) -> bool {
        self.fading
    }

    #[inline]
    pub fn is_done(&self) -> bool {
        self.fading && self.gain <= 0.0
    }
}

impl Default for StealFade {
    fn default() -> Self {
        StealFade::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_free_slots_first() {
        let mut a = VoiceAllocator::new(6, 4, StealPolicy::Oldest);
        for i in 0..4 {
            let al = a.allocate(60 + i as u8, i);
            assert_eq!(al.slot, i as usize);
            assert!(al.stolen.is_none() && al.cut.is_none());
        }
        assert_eq!(a.sounding(), 4);
    }

    #[test]
    fn steals_oldest_and_fades_it() {
        let mut a = VoiceAllocator::new(6, 3, StealPolicy::Oldest);
        for i in 0..3 {
            a.allocate(60, i);
        }
        let al = a.allocate(70, 99);
        assert_eq!(al.stolen, Some(0));
        assert_ne!(al.slot, 0, "new note must not reuse the fading slot");
        assert_eq!(a.slot(0).state, VoiceState::Fading);
        assert_eq!(a.sounding(), 3);
    }

    #[test]
    fn steals_quietest() {
        let mut a = VoiceAllocator::new(6, 3, StealPolicy::Quietest);
        for i in 0..3 {
            a.allocate(60, i);
        }
        a.set_level(0, 0.9);
        a.set_level(1, 0.1);
        a.set_level(2, 0.5);
        let al = a.allocate(70, 99);
        assert_eq!(al.stolen, Some(1));
    }

    #[test]
    fn prefers_releasing_voices_as_victims() {
        let mut a = VoiceAllocator::new(6, 3, StealPolicy::Oldest);
        for i in 0..3 {
            a.allocate(60, i);
        }
        a.release(2);
        let al = a.allocate(70, 99);
        assert_eq!(al.stolen, Some(2));
    }

    #[test]
    fn cuts_when_pool_is_exhausted() {
        let mut a = VoiceAllocator::new(3, 3, StealPolicy::Oldest);
        for i in 0..3 {
            a.allocate(60, i);
        }
        // Every slot is held; stealing one fades it but there is no free slot.
        let al = a.allocate(70, 99);
        assert!(al.stolen.is_some());
        assert!(al.cut.is_some());
        assert_eq!(al.slot, al.cut.unwrap());
    }

    #[test]
    fn steal_fade_ramps_to_silence() {
        let mut f = StealFade::new();
        assert_eq!(f.tick(), 1.0);
        assert!(!f.is_done());
        f.start(1000.0, 4.0);
        let mut prev = 1.0;
        let mut n = 0;
        while !f.is_done() {
            let g = f.tick();
            assert!(g <= prev);
            prev = g;
            n += 1;
            assert!(n < 100);
        }
        assert!((3..=5).contains(&n), "fade took {n} samples");
        f.reset();
        assert_eq!(f.tick(), 1.0);
    }

    #[test]
    fn find_held_and_free() {
        let mut a = VoiceAllocator::new(4, 4, StealPolicy::Oldest);
        a.allocate(60, 7);
        a.allocate(62, 8);
        let mut out = [0usize; 4];
        assert_eq!(a.find_held(8, &mut out), 1);
        assert_eq!(out[0], 1);
        a.release(1);
        assert_eq!(a.find_held(8, &mut out), 0);
        a.free(1);
        assert_eq!(a.in_use(), 1);
        a.reset();
        assert!(a.is_idle());
    }
}
