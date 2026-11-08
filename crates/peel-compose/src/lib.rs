//! Compositional note tools.
//!
//! Every tool is a pure function from a selection of notes to the notes that
//! replace it, so a tool can be previewed by re-applying it to the original
//! selection as its settings change. Results are ordinary notes. Tools that
//! use randomness take a seed and are deterministic for it.
//!
//! Notes that a tool changes one-for-one keep their ids; notes it creates
//! (chops, arpeggio steps, grace notes) get new ones.

use std::collections::BTreeMap;

use peel_document::{Id, Note};
use peel_time::Time;

/// Shortest note any tool produces.
pub const MIN_LENGTH: Time = Time::new_const(1, 64);

/// Arpeggio note order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArpOrder {
    Up,
    Down,
    UpDown,
    Random,
}

/// Strum direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrumDirection {
    Up,
    Down,
    /// Up on the first chord, down on the next, and so on.
    Alternate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tool {
    /// Extends every note to the next onset on its channel, less `gap`.
    Legato { gap: Time },
    /// Scales note lengths (articulation): 0.5 is staccato, 1.0 unchanged.
    Gate { amount: f32 },
    /// Cuts every note into repeats of `division`, each `gate` long.
    Chop { division: Time, gate: f32 },
    /// Plays chords (notes sharing an onset) one note per `step` over the
    /// chord's length, through `octaves` octaves.
    Arpeggiate {
        step: Time,
        order: ArpOrder,
        octaves: u8,
        gate: f32,
        seed: u64,
    },
    /// Spreads each chord's onsets over `spread`, softening later notes by
    /// up to `velocity` (0..=1). Note ends stay where they were.
    Strum {
        spread: Time,
        direction: StrumDirection,
        velocity: f32,
    },
    /// Adds `count` grace notes before every note, `spacing` apart, at
    /// `velocity` times the note's velocity.
    Flam {
        count: u8,
        spacing: Time,
        velocity: f32,
    },
    /// Random offsets up to `timing`, velocity changes up to `velocity`
    /// (absolute) and length changes up to `length` (fraction).
    Humanize {
        timing: Time,
        velocity: f32,
        length: f32,
        seed: u64,
    },
    /// Velocity times `multiply` plus `offset`, clamped to 0..=1.
    Levels { multiply: f32, offset: f32 },
    /// Reverses the selection in time and/or mirrors it in pitch.
    Flip { time: bool, pitch: bool },
    /// Removes exact duplicates and trims same-pitch overlaps.
    Tidy,
}

impl Tool {
    /// Short name for menus and undo labels.
    pub fn name(&self) -> &'static str {
        match self {
            Tool::Legato { .. } => "Legato",
            Tool::Gate { .. } => "Articulate",
            Tool::Chop { .. } => "Chop",
            Tool::Arpeggiate { .. } => "Arpeggiate",
            Tool::Strum { .. } => "Strum",
            Tool::Flam { .. } => "Flam",
            Tool::Humanize { .. } => "Humanize",
            Tool::Levels { .. } => "Velocity levels",
            Tool::Flip { .. } => "Flip",
            Tool::Tidy => "Tidy",
        }
    }
}

/// Applies `tool` to `notes` and returns the notes that replace them,
/// sorted by onset.
pub fn apply(notes: &[Note], tool: &Tool) -> Vec<Note> {
    let mut out = match tool {
        Tool::Legato { gap } => legato(notes, *gap),
        Tool::Gate { amount } => notes
            .iter()
            .map(|n| Note {
                length: scale_len(n.length, *amount),
                ..*n
            })
            .collect(),
        Tool::Chop { division, gate } => chop(notes, *division, *gate),
        Tool::Arpeggiate {
            step,
            order,
            octaves,
            gate,
            seed,
        } => arpeggiate(notes, *step, *order, *octaves, *gate, *seed),
        Tool::Strum {
            spread,
            direction,
            velocity,
        } => strum(notes, *spread, *direction, *velocity),
        Tool::Flam {
            count,
            spacing,
            velocity,
        } => flam(notes, *count, *spacing, *velocity),
        Tool::Humanize {
            timing,
            velocity,
            length,
            seed,
        } => humanize(notes, *timing, *velocity, *length, *seed),
        Tool::Levels { multiply, offset } => notes
            .iter()
            .map(|n| Note {
                velocity: (n.velocity * multiply + offset).clamp(0.0, 1.0),
                ..*n
            })
            .collect(),
        Tool::Flip { time, pitch } => flip(notes, *time, *pitch),
        Tool::Tidy => tidy(notes),
    };
    out.sort_by(|a, b| a.start.cmp(&b.start).then(a.key.cmp(&b.key)));
    out
}

fn scale_len(len: Time, amount: f32) -> Time {
    Time::from_f64(len.as_f64() * f64::from(amount.max(0.0)), 960).max(MIN_LENGTH)
}

/// Small deterministic generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in -1..=1.
    fn signed(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

/// Notes grouped by channel, each group sorted by onset then pitch.
fn by_channel(notes: &[Note]) -> BTreeMap<Id, Vec<Note>> {
    let mut m: BTreeMap<Id, Vec<Note>> = BTreeMap::new();
    for n in notes {
        m.entry(n.channel).or_default().push(*n);
    }
    for v in m.values_mut() {
        v.sort_by(|a, b| a.start.cmp(&b.start).then(a.key.cmp(&b.key)));
    }
    m
}

/// Chords: notes on one channel sharing an onset, lowest first.
fn chords(notes: &[Note]) -> Vec<Vec<Note>> {
    let mut out = Vec::new();
    for group in by_channel(notes).into_values() {
        let mut cur: Vec<Note> = Vec::new();
        for n in group {
            if cur.first().is_some_and(|f| f.start != n.start) {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(n);
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out.sort_by_key(|c| c[0].start);
    out
}

fn legato(notes: &[Note], gap: Time) -> Vec<Note> {
    let mut out = Vec::with_capacity(notes.len());
    for group in by_channel(notes).into_values() {
        for (i, n) in group.iter().enumerate() {
            let next = group[i + 1..]
                .iter()
                .map(|x| x.start)
                .find(|s| *s > n.start);
            let length = match next {
                Some(s) => (s - n.start - gap).max(MIN_LENGTH),
                None => n.length,
            };
            out.push(Note { length, ..*n });
        }
    }
    out
}

fn chop(notes: &[Note], division: Time, gate: f32) -> Vec<Note> {
    let division = division.max(MIN_LENGTH);
    let piece = scale_len(division, gate.clamp(0.05, 1.0));
    let mut out = Vec::new();
    for n in notes {
        let mut at = n.start;
        let mut first = true;
        while at < n.end() {
            let length = piece.min(n.end() - at).max(MIN_LENGTH);
            let id = if first { n.id } else { Id::new() };
            out.push(Note {
                id,
                start: at,
                length,
                ..*n
            });
            first = false;
            at += division;
        }
    }
    out
}

fn arpeggiate(
    notes: &[Note],
    step: Time,
    order: ArpOrder,
    octaves: u8,
    gate: f32,
    seed: u64,
) -> Vec<Note> {
    let step = step.max(MIN_LENGTH);
    let length = scale_len(step, gate.clamp(0.05, 1.0));
    let mut rng = Rng(seed);
    let mut out = Vec::new();
    for chord in chords(notes) {
        let start = chord[0].start;
        let end = chord.iter().map(Note::end).max().unwrap_or(start + step);
        let mut keys: Vec<(u8, Note)> = Vec::new();
        for o in 0..octaves.max(1) {
            for n in &chord {
                let k = n.key as u16 + 12 * o as u16;
                if k <= 127 {
                    keys.push((k as u8, *n));
                }
            }
        }
        keys.sort_by_key(|k| k.0);
        let seq: Vec<(u8, Note)> = match order {
            ArpOrder::Up => keys.clone(),
            ArpOrder::Down => keys.iter().rev().copied().collect(),
            ArpOrder::UpDown => {
                let mut v = keys.clone();
                if keys.len() > 2 {
                    v.extend(keys[1..keys.len() - 1].iter().rev().copied());
                }
                v
            }
            ArpOrder::Random => keys.clone(),
        };
        let mut at = start;
        let mut i = 0usize;
        while at < end && !seq.is_empty() {
            let (key, src) = match order {
                ArpOrder::Random => seq[(rng.next() % seq.len() as u64) as usize],
                _ => seq[i % seq.len()],
            };
            out.push(Note {
                id: Id::new(),
                key,
                start: at,
                length: length.min(end - at).max(MIN_LENGTH),
                ..src
            });
            at += step;
            i += 1;
        }
    }
    out
}

fn strum(notes: &[Note], spread: Time, direction: StrumDirection, velocity: f32) -> Vec<Note> {
    let mut out = Vec::new();
    for (ci, mut chord) in chords(notes).into_iter().enumerate() {
        let down = match direction {
            StrumDirection::Up => false,
            StrumDirection::Down => true,
            StrumDirection::Alternate => ci % 2 == 1,
        };
        if down {
            chord.reverse();
        }
        let n = chord.len();
        for (i, note) in chord.into_iter().enumerate() {
            let f = if n > 1 {
                i as f64 / (n - 1) as f64
            } else {
                0.0
            };
            let offset = Time::from_f64(spread.as_f64() * f, 960)
                .min(note.length - MIN_LENGTH)
                .max(Time::ZERO);
            out.push(Note {
                start: note.start + offset,
                length: note.length - offset,
                velocity: (note.velocity * (1.0 - velocity.clamp(0.0, 1.0) * f as f32))
                    .clamp(0.0, 1.0),
                ..note
            });
        }
    }
    out
}

fn flam(notes: &[Note], count: u8, spacing: Time, velocity: f32) -> Vec<Note> {
    let spacing = spacing.max(MIN_LENGTH);
    let grace_len = scale_len(spacing, 0.9);
    let mut out = Vec::with_capacity(notes.len() * (count as usize + 1));
    for n in notes {
        out.push(*n);
        for k in 1..=count as i64 {
            let back = spacing.mul_int(k);
            if back > n.start {
                break;
            }
            let v = (n.velocity * velocity.clamp(0.0, 1.0)).clamp(0.0, 1.0);
            out.push(Note {
                id: Id::new(),
                start: n.start - back,
                length: grace_len,
                velocity: v,
                ..*n
            });
        }
    }
    out
}

fn humanize(notes: &[Note], timing: Time, velocity: f32, length: f32, seed: u64) -> Vec<Note> {
    let mut rng = Rng(seed);
    notes
        .iter()
        .map(|n| {
            let dt = timing.as_f64() * rng.signed();
            let start = Time::from_f64((n.start.as_f64() + dt).max(0.0), 960);
            let dv = velocity as f64 * rng.signed();
            let dl = 1.0 + length.clamp(0.0, 0.95) as f64 * rng.signed();
            Note {
                start,
                velocity: (n.velocity as f64 + dv).clamp(0.01, 1.0) as f32,
                length: Time::from_f64(n.length.as_f64() * dl, 960).max(MIN_LENGTH),
                ..*n
            }
        })
        .collect()
}

fn flip(notes: &[Note], time: bool, pitch: bool) -> Vec<Note> {
    let (Some(lo), Some(hi)) = (
        notes.iter().map(|n| n.start).min(),
        notes.iter().map(Note::end).max(),
    ) else {
        return Vec::new();
    };
    let (kmin, kmax) = notes
        .iter()
        .fold((127u8, 0u8), |(a, b), n| (a.min(n.key), b.max(n.key)));
    notes
        .iter()
        .map(|n| {
            let mut m = *n;
            if time {
                m.start = lo + hi - n.end();
            }
            if pitch {
                m.key = (kmin as i16 + kmax as i16 - n.key as i16).clamp(0, 127) as u8;
            }
            m
        })
        .collect()
}

fn tidy(notes: &[Note]) -> Vec<Note> {
    let mut out: Vec<Note> = Vec::with_capacity(notes.len());
    for group in by_channel(notes).into_values() {
        let mut by_key: BTreeMap<u8, Vec<Note>> = BTreeMap::new();
        for n in group {
            by_key.entry(n.key).or_default().push(n);
        }
        for mut same in by_key.into_values() {
            same.sort_by_key(|n| (n.start, std::cmp::Reverse(n.length)));
            same.dedup_by(|b, a| a.start == b.start);
            for i in 0..same.len() {
                if let Some(next) = same.get(i + 1).map(|x| x.start)
                    && same[i].end() > next
                {
                    same[i].length = (next - same[i].start).max(MIN_LENGTH);
                }
            }
            out.extend(same);
        }
    }
    out
}

#[cfg(test)]
mod tests;
