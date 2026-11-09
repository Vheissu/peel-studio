//! Saved note curves and the same live MPE path used by hardware controllers.
use crate::actions::{ExpressionPointInput, MidiMessageInput, MpeModeInput};
use crate::control::{commit, id, time};
use peel_document::{ExpressionPoint, NoteExpression};
use peel_session::{MidiEvent, MpeMode, Session};
use serde_json::{Value, json};

pub(crate) fn note_expression(
    s: &mut Session,
    pattern: &str,
    note: &str,
    pitch: Option<Vec<ExpressionPointInput>>,
    pressure: Option<Vec<ExpressionPointInput>>,
    timbre: Option<Vec<ExpressionPointInput>>,
) -> Result<Value, String> {
    let (pid, nid) = (id(pattern)?, id(note)?);
    let pattern = s.project().pattern(pid).ok_or("Pattern does not exist")?;
    let length = pattern
        .notes
        .iter()
        .find(|n| n.id == nid)
        .ok_or("Note does not exist in this pattern")?
        .length;
    let mut curve = pattern
        .expression_for(nid)
        .cloned()
        .unwrap_or(NoteExpression {
            note: nid,
            ..Default::default()
        });
    let change = pitch.is_some() || pressure.is_some() || timbre.is_some();
    for (destination, input, min, max) in [
        (&mut curve.pitch, pitch, -120.0, 120.0),
        (&mut curve.pressure, pressure, 0.0, 1.0),
        (&mut curve.timbre, timbre, 0.0, 1.0),
    ] {
        if let Some(input) = input {
            if input.len() > peel_document::MAX_EXPRESSION_POINTS {
                return Err("Too many expression points".into());
            }
            let points = input.into_iter().map(|p| {
                let offset = time(&p.offset)?;
                if offset.is_negative() || offset > length || !p.value.is_finite() || !(min..=max).contains(&p.value) {
                    return Err("Expression offset must be within the note; pitch is -120..120, pressure and timbre are 0..1".into());
                }
                Ok(ExpressionPoint { offset, value:p.value })
            }).collect::<Result<Vec<_>,String>>()?;
            if points.windows(2).any(|w| w[0].offset >= w[1].offset) {
                return Err("Expression points must have increasing, distinct offsets".into());
            }
            *destination = points;
        }
    }
    if change {
        let mut project = s.project().clone();
        let pattern = project.pattern_mut(pid).unwrap();
        pattern.expression.retain(|e| e.note != nid);
        if !curve.pitch.is_empty() || !curve.pressure.is_empty() || !curve.timbre.is_empty() {
            pattern.expression.push(curve.clone());
        }
        commit(s, "Edit note expression", project)?;
    }
    Ok(json!({"expression":curve,"revision":s.doc.revision()}))
}

pub(crate) fn mpe(
    s: &mut Session,
    mode: Option<MpeModeInput>,
    members: Option<u8>,
    member: Option<f32>,
    manager: Option<f32>,
) -> Result<Value, String> {
    let change = mode.is_some() || members.is_some() || member.is_some() || manager.is_some();
    let mut config = s.mpe_config();
    if let Some(mode) = mode {
        config.mode = match mode {
            MpeModeInput::Off => MpeMode::Off,
            MpeModeInput::Lower => MpeMode::Lower,
            MpeModeInput::Upper => MpeMode::Upper,
        };
    }
    if let Some(v) = members {
        config.members = v;
    }
    if let Some(v) = member {
        config.member_bend_range = v;
    }
    if let Some(v) = manager {
        config.manager_bend_range = v;
    }
    if change {
        s.configure_mpe(config)?;
    }
    Ok(json!({"config":s.mpe_config()}))
}

pub(crate) fn midi_input(
    s: &mut Session,
    channel: u8,
    event: MidiMessageInput,
) -> Result<Value, String> {
    if channel > 15 {
        return Err("MIDI channel must be 0..15".into());
    }
    let byte = |v: u8| {
        if v <= 127 {
            Ok(v)
        } else {
            Err("MIDI key, velocity and controller values must be 0..127".to_string())
        }
    };
    let event = match event {
        MidiMessageInput::NoteOn { key, velocity } => MidiEvent::NoteOn {
            channel,
            key: byte(key)?,
            velocity: byte(velocity)?,
        },
        MidiMessageInput::NoteOff { key, velocity } => MidiEvent::NoteOff {
            channel,
            key: byte(key)?,
            velocity: byte(velocity)?,
        },
        MidiMessageInput::PitchBend { value } => {
            if !(-8192..=8191).contains(&value) {
                return Err("Pitch bend must be -8192..8191".into());
            }
            MidiEvent::PitchBend { channel, value }
        }
        MidiMessageInput::ChannelPressure { value } => MidiEvent::ChannelPressure {
            channel,
            value: byte(value)?,
        },
        MidiMessageInput::PolyPressure { key, value } => MidiEvent::PolyPressure {
            channel,
            key: byte(key)?,
            value: byte(value)?,
        },
        MidiMessageInput::ControlChange { controller, value } => {
            let (controller, value) = (byte(controller)?, byte(value)?);
            match controller {
                1 => MidiEvent::ModWheel { channel, value },
                64 => MidiEvent::Sustain {
                    channel,
                    down: value >= 64,
                },
                120 => MidiEvent::AllSoundOff { channel },
                123 => MidiEvent::AllNotesOff { channel },
                _ => MidiEvent::ControlChange {
                    channel,
                    controller,
                    value,
                },
            }
        }
    };
    s.handle_midi_event(event);
    s.poll_midi();
    Ok(json!({"mpe":s.mpe_config(),"expression":crate::io_control::expression(s)}))
}
