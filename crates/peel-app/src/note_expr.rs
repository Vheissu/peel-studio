//! Editing helpers for per-note expression (`Pattern::expression`).
//!
//! Every write goes through [`set_curve`], which leaves the curve in the
//! state the document validator accepts: offsets inside the note, sorted and
//! unique, values finite and in range. Copies remap curves to the new note
//! IDs; deletes and shortening prune them.

use std::collections::HashMap;

use peel_document::{ExpressionPoint, Id, NoteExpression, Pattern, Time};

/// What the Piano Roll's lower lane edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lane {
    #[default]
    Velocity,
    Pitch,
    Pressure,
    Timbre,
}

impl Lane {
    pub const ALL: [Lane; 4] = [Lane::Velocity, Lane::Pitch, Lane::Pressure, Lane::Timbre];

    pub fn name(self) -> &'static str {
        match self {
            Lane::Velocity => "Velocity",
            Lane::Pitch => "Pitch",
            Lane::Pressure => "Pressure",
            Lane::Timbre => "Timbre",
        }
    }

    /// Value a note plays at before its first point.
    pub fn neutral(self) -> f32 {
        match self {
            Lane::Timbre => 0.5,
            _ => 0.0,
        }
    }

    /// Stored value range.
    pub fn bounds(self) -> (f32, f32) {
        match self {
            Lane::Pitch => (-120.0, 120.0),
            _ => (0.0, 1.0),
        }
    }

    /// "+3.5 st", "64%".
    pub fn format(self, v: f32) -> String {
        match self {
            Lane::Pitch => {
                if v.abs() < 0.05 {
                    "0 st".to_string()
                } else {
                    format!("{v:+.1} st")
                }
            }
            _ => format!("{:.0}%", v * 100.0),
        }
    }
}

/// Pitch lane ranges, in semitones either side of zero.
pub const PITCH_RANGES: [f32; 6] = [2.0, 12.0, 24.0, 48.0, 96.0, 120.0];

/// The smallest pitch range that shows every value in `points`.
pub fn fit_range(points: &[ExpressionPoint]) -> f32 {
    let peak = points.iter().fold(0.0f32, |m, p| m.max(p.value.abs()));
    PITCH_RANGES
        .iter()
        .copied()
        .find(|r| peak <= *r + 1.0e-3)
        .unwrap_or(120.0)
}

pub fn curve(e: &NoteExpression, lane: Lane) -> &[ExpressionPoint] {
    match lane {
        Lane::Pitch => &e.pitch,
        Lane::Pressure => &e.pressure,
        Lane::Timbre => &e.timbre,
        Lane::Velocity => &[],
    }
}

fn curve_mut(e: &mut NoteExpression, lane: Lane) -> Option<&mut Vec<ExpressionPoint>> {
    match lane {
        Lane::Pitch => Some(&mut e.pitch),
        Lane::Pressure => Some(&mut e.pressure),
        Lane::Timbre => Some(&mut e.timbre),
        Lane::Velocity => None,
    }
}

/// The points of `note` in `lane` (empty when it has none).
pub fn points(pt: &Pattern, note: Id, lane: Lane) -> Vec<ExpressionPoint> {
    pt.expression_for(note)
        .map(|e| curve(e, lane).to_vec())
        .unwrap_or_default()
}

/// Puts `points` into a valid form for a note of `length`: values clamped
/// to the lane, offsets clamped into the note, sorted, and one point per
/// offset (the later one in `points` wins).
pub fn normalize(lane: Lane, length: Time, points: &[ExpressionPoint]) -> Vec<ExpressionPoint> {
    let (lo, hi) = lane.bounds();
    let mut out: Vec<ExpressionPoint> = points
        .iter()
        .filter(|p| p.value.is_finite())
        .map(|p| ExpressionPoint {
            offset: p.offset.max(Time::ZERO).min(length),
            value: p.value.clamp(lo, hi),
        })
        .collect();
    // Stable, so equal offsets keep their order and the last one is kept.
    out.sort_by_key(|p| p.offset);
    let mut dedup: Vec<ExpressionPoint> = Vec::with_capacity(out.len());
    for p in out {
        match dedup.last_mut() {
            Some(last) if last.offset == p.offset => *last = p,
            _ => dedup.push(p),
        }
    }
    dedup
}

/// Replaces the curve of `note` in `lane`. Removes the note's entry when all
/// three curves end up empty. Does nothing for unknown notes.
pub fn set_curve(pt: &mut Pattern, note: Id, lane: Lane, points: &[ExpressionPoint]) {
    let Some(length) = pt.notes.iter().find(|n| n.id == note).map(|n| n.length) else {
        return;
    };
    let clean = normalize(lane, length, points);
    let i = match pt.expression.iter().position(|e| e.note == note) {
        Some(i) => i,
        None if clean.is_empty() => return,
        None => {
            pt.expression.push(NoteExpression {
                note,
                ..Default::default()
            });
            pt.expression.len() - 1
        }
    };
    if let Some(c) = curve_mut(&mut pt.expression[i], lane) {
        *c = clean;
    }
    let e = &pt.expression[i];
    if e.pitch.is_empty() && e.pressure.is_empty() && e.timbre.is_empty() {
        pt.expression.remove(i);
    }
}

/// Drops curves of deleted notes and points past shortened notes.
pub fn prune(pt: &mut Pattern) {
    pt.retain_expression();
    pt.expression
        .retain(|e| !(e.pitch.is_empty() && e.pressure.is_empty() && e.timbre.is_empty()));
}

/// The curves of `notes` (by ID), for a clipboard or a duplicate.
pub fn gather(pt: &Pattern, notes: impl IntoIterator<Item = Id>) -> Vec<NoteExpression> {
    notes
        .into_iter()
        .filter_map(|id| pt.expression_for(id).cloned())
        .collect()
}

/// Copies of `curves` addressed to new notes: `map` takes an old note ID to
/// the ID of its copy. Curves without a copy are dropped.
pub fn remap(curves: &[NoteExpression], map: &HashMap<Id, Id>) -> Vec<NoteExpression> {
    curves
        .iter()
        .filter_map(|e| {
            map.get(&e.note)
                .map(|&note| NoteExpression { note, ..e.clone() })
        })
        .collect()
}

/// Adds `curves` to the pattern, replacing any for the same notes.
pub fn insert(pt: &mut Pattern, curves: Vec<NoteExpression>) {
    for c in curves {
        pt.expression.retain(|e| e.note != c.note);
        pt.expression.push(c);
    }
    prune(pt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use peel_document::Note;

    fn pt(len: Time) -> (Pattern, Id) {
        let mut p = Pattern {
            id: Id::new(),
            name: "P".into(),
            color: peel_document::PALETTE[0],
            length: Time::beats(4),
            notes: Vec::new(),
            expression: Vec::new(),
        };
        let n = Note::new(Id::new(), Time::ZERO, len, 60, 0.8);
        let id = n.id;
        p.notes.push(n);
        (p, id)
    }

    fn at(beats: i64, value: f32) -> ExpressionPoint {
        ExpressionPoint {
            offset: Time::new(beats, 4),
            value,
        }
    }

    #[test]
    fn writes_stay_valid() {
        let (mut p, id) = pt(Time::beats(1));
        set_curve(
            &mut p,
            id,
            Lane::Pitch,
            &[
                at(3, 500.0),
                at(1, f32::NAN),
                at(-2, -3.0),
                at(9, 4.0),
                at(3, 7.0),
            ],
        );
        let c = points(&p, id, Lane::Pitch);
        assert_eq!(c, vec![at(0, -3.0), at(3, 7.0), at(4, 4.0)]);
        set_curve(&mut p, id, Lane::Pressure, &[at(1, 2.0)]);
        assert_eq!(points(&p, id, Lane::Pressure), vec![at(1, 1.0)]);
        // Emptying every curve removes the entry.
        set_curve(&mut p, id, Lane::Pitch, &[]);
        set_curve(&mut p, id, Lane::Pressure, &[]);
        assert!(p.expression.is_empty());
    }

    #[test]
    fn remap_and_prune_follow_notes() {
        let (mut p, id) = pt(Time::beats(2));
        set_curve(&mut p, id, Lane::Timbre, &[at(0, 0.2), at(6, 0.9)]);
        let copy = Id::new();
        let curves = remap(&gather(&p, [id]), &HashMap::from([(id, copy)]));
        assert_eq!(curves[0].note, copy);
        assert_eq!(curves[0].timbre, vec![at(0, 0.2), at(6, 0.9)]);
        p.notes[0].length = Time::beats(1);
        prune(&mut p);
        assert_eq!(points(&p, id, Lane::Timbre), vec![at(0, 0.2)]);
        p.notes.clear();
        prune(&mut p);
        assert!(p.expression.is_empty());
    }
}
