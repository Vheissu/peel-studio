//! `PEEL_MPE_TAKE=1` (development only): plays a short MPE performance into
//! the selected pattern through the real MIDI input path, with the pattern
//! recorder armed, then stops and shows the result in the Piano Roll. Used
//! for screenshots of recorded expression; pair it with `PEEL_SILENT=1` so
//! nothing reaches the speakers.
//!
//! Two notes on the same key arrive on different member channels: one
//! scoops up and slides with vibrato, the other leans down, each with its
//! own pressure and CC74.

use std::time::Instant;

use peel_session::{MidiEvent, MpeConfig, MpeMode, PlayMode};
use peel_ui::Ui;

use crate::note_expr::Lane;
use crate::studio::Studio;
use crate::workspace::DockTab;

/// Performance length in seconds (two bars at 120 BPM is 4 s).
const TAKE: f32 = 3.6;

#[derive(Default)]
pub struct DevTake {
    start: Option<Instant>,
    last: f32,
    done: bool,
}

const KEY: u8 = 62;
const CHANNELS: [u8; 2] = [1, 2];

fn bend(semitones: f32, range: f32) -> i16 {
    ((semitones / range) * 8192.0)
        .round()
        .clamp(-8192.0, 8191.0) as i16
}

/// Pitch (semitones), pressure and CC74 of voice `v` at `t` seconds.
fn shape(v: usize, t: f32) -> (f32, f32, f32) {
    let u = (t / TAKE).clamp(0.0, 1.0);
    if v == 0 {
        let scoop = -2.0 * (1.0 - (t / 0.25).min(1.0));
        let slide = if u > 0.45 {
            5.0 * ((u - 0.45) / 0.2).min(1.0)
        } else {
            0.0
        };
        let vib = if u > 0.7 {
            0.4 * (t * 6.0 * std::f32::consts::TAU).sin()
        } else {
            0.0
        };
        (
            scoop + slide + vib,
            0.35 + 0.6 * (u * 1.6).min(1.0),
            0.3 + 0.6 * u,
        )
    } else {
        let lean = -3.0 * ((u - 0.3) / 0.5).clamp(0.0, 1.0);
        (lean, 0.8 - 0.6 * u, 0.7 - 0.5 * u)
    }
}

pub fn frame(studio: &mut Studio, ui: &mut Ui) {
    if std::env::var_os("PEEL_MPE_TAKE").is_none() || studio.dev_take.done {
        return;
    }
    ui.request_repaint();
    let s = &mut studio.session;
    let Some(start) = studio.dev_take.start else {
        let config = MpeConfig {
            mode: MpeMode::Lower,
            ..MpeConfig::default()
        };
        if s.configure_mpe(config).is_err() {
            studio.dev_take.done = true;
            return;
        }
        let pid = s.selected_pattern();
        s.set_mode(PlayMode::Pattern(pid));
        s.seek(0.0);
        studio.recorder.armed = true;
        s.play();
        studio.ws.dock = DockTab::PianoRoll;
        studio.dev_take.start = Some(Instant::now());
        return;
    };
    let t = start.elapsed().as_secs_f32();
    let range = s.mpe_config().member_bend_range;
    let lead = 0.15;
    if studio.dev_take.last < lead && t >= lead {
        for (v, ch) in CHANNELS.iter().enumerate() {
            let (p, pr, tb) = shape(v, 0.0);
            s.handle_midi_event(MidiEvent::PitchBend {
                channel: *ch,
                value: bend(p, range),
            });
            s.handle_midi_event(MidiEvent::ChannelPressure {
                channel: *ch,
                value: (pr * 127.0) as u8,
            });
            s.handle_midi_event(MidiEvent::ControlChange {
                channel: *ch,
                controller: 74,
                value: (tb * 127.0) as u8,
            });
            s.handle_midi_event(MidiEvent::NoteOn {
                channel: *ch,
                key: KEY,
                velocity: 100,
            });
        }
    } else if t > lead && t < lead + TAKE {
        let pt = t - lead;
        for (v, ch) in CHANNELS.iter().enumerate() {
            let (p, pr, tb) = shape(v, pt);
            s.handle_midi_event(MidiEvent::PitchBend {
                channel: *ch,
                value: bend(p, range),
            });
            s.handle_midi_event(MidiEvent::ChannelPressure {
                channel: *ch,
                value: (pr * 127.0) as u8,
            });
            s.handle_midi_event(MidiEvent::ControlChange {
                channel: *ch,
                controller: 74,
                value: (tb * 127.0) as u8,
            });
        }
    } else if t >= lead + TAKE && studio.dev_take.last < lead + TAKE {
        for ch in CHANNELS {
            s.handle_midi_event(MidiEvent::NoteOff {
                channel: ch,
                key: KEY,
                velocity: 64,
            });
        }
    } else if t > lead + TAKE + 0.3 {
        studio.recorder.armed = false;
        s.stop();
        // Show the first recorded note's pitch curve.
        let pid = s.selected_pattern();
        let first = s.project().pattern(pid).and_then(|p| {
            p.expression
                .iter()
                .filter_map(|e| p.notes.iter().find(|n| n.id == e.note))
                .min_by_key(|n| (n.start, n.channel))
                .map(|n| n.id)
        });
        if let Some(n) = first {
            studio.ws.roll.selection = std::iter::once(n).collect();
            studio.ws.roll.focus_note = Some(n);
        }
        studio.ws.roll.lane = match std::env::var("PEEL_MPE_LANE").as_deref() {
            Ok("pressure") => Lane::Pressure,
            Ok("timbre") => Lane::Timbre,
            _ => Lane::Pitch,
        };
        studio.ws.roll.scroll_beats = 0.0;
        if let Ok(name) = std::env::var("PEEL_INSTRUMENT")
            && let Some(c) = s
                .project()
                .channels
                .iter()
                .find(|c| c.name == name)
                .map(|c| c.id)
        {
            crate::instrument_editor::open(s, &mut studio.ws, c);
        }
        studio.dev_take.done = true;
    }
    s.poll_midi();
    studio.dev_take.last = t;
}
