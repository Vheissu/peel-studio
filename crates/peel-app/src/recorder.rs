//! Note recording into the selected pattern.
//!
//! While Record is armed and the transport plays in Pattern mode, notes from
//! MIDI keyboards and the computer keyboard are written into the pattern
//! being edited. Positions come from the engine and are moved back by the
//! output latency so notes land where the player heard them. Each take is a
//! single undo step.

use std::collections::HashMap;

use peel_document::{Id, Note, Time};
use peel_session::{PlayMode, Session};

#[derive(Default)]
pub struct Recorder {
    pub armed: bool,
    /// Notes held down: (channel, key) -> (start beat, velocity).
    held: HashMap<(Id, u8), (f64, f32)>,
    gesture: Option<u64>,
    takes: u64,
    warned: bool,
}

impl Recorder {
    /// Seconds between the engine's position and what the listener hears.
    fn latency_beats(s: &Session) -> f64 {
        let secs = s.buffer_frames() as f64 * 2.0 / s.sample_rate().max(1.0) as f64;
        secs * s.project().bpm() / 60.0
    }

    fn active(&mut self, s: &mut Session) -> Option<Id> {
        if !self.armed || !s.is_playing() {
            return None;
        }
        match s.mode() {
            PlayMode::Pattern(p) => Some(p),
            PlayMode::Song => {
                if !self.warned {
                    s.notify(
                        "Switch to Pattern mode to record notes into a pattern",
                        true,
                    );
                    self.warned = true;
                }
                None
            }
        }
    }

    /// Records one note event at the engine position `beat`.
    pub fn note(
        &mut self,
        s: &mut Session,
        channel: Id,
        key: u8,
        velocity: f32,
        on: bool,
        beat: f64,
    ) {
        let Some(pid) = self.active(s) else { return };
        let Some(len) = s.project().pattern(pid).map(|p| p.length.as_f64()) else {
            return;
        };
        let pos = (beat - Self::latency_beats(s)).rem_euclid(len.max(0.25));
        if on {
            self.held.insert((channel, key), (pos, velocity));
            return;
        }
        let Some((start, vel)) = self.held.remove(&(channel, key)) else {
            return;
        };
        // A note held across the loop end is cut at the pattern end.
        let end = if pos >= start { pos } else { len };
        let start_t = Time::from_f64(start, 960);
        let length = Time::from_f64((end - start).max(1.0 / 64.0), 960).max(Time::new(1, 64));
        let note = Note::new(channel, start_t, length, key, vel.clamp(0.05, 1.0));
        let gesture = *self.gesture.get_or_insert_with(|| 0x7ec0_0000 + self.takes);
        s.doc.edit_gesture(gesture, "Record notes", |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                pt.notes.push(note);
                pt.sort_notes();
            }
        });
    }

    /// Per-frame upkeep: records MIDI keyboard notes and closes the take
    /// when playback stops.
    pub fn frame(&mut self, s: &mut Session) {
        let notes = s.take_recorded_midi();
        if self.active(s).is_some() {
            for n in notes {
                if let Some(ch) = n.channel {
                    self.note(s, ch, n.key, n.velocity, n.on, n.beat);
                }
            }
        }
        if (!s.is_playing() || !self.armed) && self.gesture.is_some() {
            self.gesture = None;
            self.takes += 1;
            self.held.clear();
            s.doc.end_gesture();
        }
        if !self.armed {
            self.warned = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peel_session::template;

    #[test]
    fn records_notes_into_the_pattern_with_latency_and_wrap() {
        let mut s = Session::new_silent(template::beat_and_melody());
        let pid = s.selected_pattern();
        let piano = s
            .project()
            .channels
            .iter()
            .find(|c| c.name == "Piano")
            .unwrap()
            .id;
        let mut r = Recorder {
            armed: true,
            ..Default::default()
        };
        s.play();
        let lat = Recorder::latency_beats(&s);
        r.note(&mut s, piano, 64, 0.9, true, 1.0 + lat);
        r.note(&mut s, piano, 64, 0.0, false, 1.5 + lat);
        // Held across the 4-beat loop end: cut at the end.
        r.note(&mut s, piano, 67, 0.7, true, 3.5 + lat);
        r.note(&mut s, piano, 67, 0.0, false, 0.25 + lat);
        let notes: Vec<Note> = s
            .project()
            .pattern(pid)
            .unwrap()
            .notes_for(piano)
            .copied()
            .collect();
        assert_eq!(notes.len(), 2);
        assert!(
            (notes[0].start.as_f64() - 1.0).abs() < 0.01
                && (notes[0].length.as_f64() - 0.5).abs() < 0.01
        );
        assert!(
            (notes[1].start.as_f64() - 3.5).abs() < 0.01
                && (notes[1].end().as_f64() - 4.0).abs() < 0.01
        );
        s.stop();
        r.frame(&mut s);
        assert_eq!(s.doc.undo_label(), Some("Record notes"));
        s.doc.undo();
        assert_eq!(
            s.project().pattern(pid).unwrap().notes_for(piano).count(),
            0,
            "one undo removes the take"
        );
    }
}
