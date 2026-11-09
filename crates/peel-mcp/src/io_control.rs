//! Hardware routing and MIDI controls shared by MCP and scripts.
use crate::actions::{KeyPressureInput, MidiMappingInput, MidiTargetInput, OutputRouteInput};
use peel_session::{
    MidiControlTarget, MidiEvent, MidiLearnStatus, MidiMapping, OutputChannels, OutputRoute,
    Session,
};
use serde_json::{Value, json};

pub(crate) fn output_routes(s: &Session) -> Value {
    json!({"primary_channels": s.primary_output_channels(),
        "primary_channel_count": s.primary_output_channel_count(),
        "routes": s.output_routes(), "status": s.output_route_status()})
}

pub(crate) fn configure_routes(
    s: &mut Session,
    routes: Vec<OutputRouteInput>,
) -> Result<(), String> {
    let routes = routes
        .into_iter()
        .map(|r| {
            Ok(OutputRoute {
                name: r.name,
                track: r.track.as_deref().map(crate::control::id).transpose()?,
                device: r.device,
                channels: r.right.map_or(OutputChannels::Mono(r.left), |v| {
                    OutputChannels::Stereo(r.left, v)
                }),
                gain_db: r.gain_db,
                muted: r.muted,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    s.configure_output_routes(routes)
}

pub(crate) fn midi_target(t: MidiTargetInput) -> Result<MidiControlTarget, String> {
    use crate::control::id;
    Ok(match t {
        MidiTargetInput::DeviceParam { device, key } => MidiControlTarget::DeviceParam {
            device: id(&device)?,
            key,
        },
        MidiTargetInput::TrackGain { track } => MidiControlTarget::TrackGain { track: id(&track)? },
        MidiTargetInput::TrackPan { track } => MidiControlTarget::TrackPan { track: id(&track)? },
        MidiTargetInput::ChannelGain { channel } => MidiControlTarget::ChannelGain {
            channel: id(&channel)?,
        },
        MidiTargetInput::ChannelPan { channel } => MidiControlTarget::ChannelPan {
            channel: id(&channel)?,
        },
    })
}

pub(crate) fn configure_midi(
    s: &mut Session,
    filter: Option<u8>,
    mappings: Vec<MidiMappingInput>,
) -> Result<(), String> {
    if filter.is_some_and(|c| c > 15) {
        return Err("MIDI channel filter must be 0..15".into());
    }
    let mappings = mappings
        .into_iter()
        .map(|m| {
            Ok(MidiMapping {
                controller: m.controller,
                channel: m.channel,
                target: midi_target(m.target)?,
                min: m.min,
                max: m.max,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    // Mapping validation completes before either setting changes; the filter is checked above.
    s.set_midi_control_mappings(mappings)?;
    s.set_midi_channel_filter(filter)
}

pub(crate) fn midi_controls(s: &Session) -> Value {
    let learn = match s.midi_learn_status() {
        MidiLearnStatus::Idle => json!({"state":"idle"}),
        MidiLearnStatus::Waiting { target } => json!({"state":"waiting", "target":target}),
        MidiLearnStatus::Learned { mapping } => json!({"state":"learned", "mapping":mapping}),
        MidiLearnStatus::Error { message } => json!({"state":"error", "message":message}),
    };
    json!({"channel_filter":s.midi_channel_filter(), "mappings":s.midi_control_mappings(), "learn":learn, "error":s.midi_control_error(),"expression":expression(s),"mpe":s.mpe_config()})
}

pub(crate) fn send_control(
    s: &mut Session,
    channel: u8,
    controller: u8,
    value: u8,
) -> Result<(), String> {
    if channel > 15 || controller > 127 || value > 127 {
        return Err("MIDI channel must be 0..15; controller and value must be 0..127".into());
    }
    let event = match controller {
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
    };
    s.handle_midi_event(event);
    s.poll_midi();
    Ok(())
}

/// Latest accepted controller state; pressure is sparse and keyed by MIDI note.
pub(crate) fn expression(s: &Session) -> Value {
    let keys: Vec<_> = (0..128u8)
        .filter_map(|key| {
            let value = s.poly_pressure_value(key);
            (value > 0.0).then(|| json!({"key":key,"value":value}))
        })
        .collect();
    json!({"bend":s.pitch_bend_value(),"bend_range":s.pitch_bend_range(),
        "semitones":s.pitch_bend_value()*s.pitch_bend_range(),
        "pressure":s.channel_pressure_value(),"key_pressure":keys})
}

pub(crate) fn send_expression(
    s: &mut Session,
    bend: Option<f32>,
    pressure: Option<f32>,
    keys: Option<Vec<KeyPressureInput>>,
    range: Option<f32>,
    reset: bool,
) -> Result<(), String> {
    if bend.is_some_and(|v| !v.is_finite() || !(-1.0..=1.0).contains(&v)) {
        return Err("bend must be between -1 and 1".into());
    }
    if pressure.is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v)) {
        return Err("pressure must be between 0 and 1".into());
    }
    if range.is_some_and(|v| !v.is_finite() || !(0.0..=48.0).contains(&v)) {
        return Err("bend_range must be between 0 and 48 semitones".into());
    }
    if let Some(keys) = &keys {
        if keys.len() > 128 {
            return Err("At most 128 key-pressure values are supported".into());
        }
        let mut used = [false; 128];
        for k in keys {
            if k.key > 127 || !k.value.is_finite() || !(0.0..=1.0).contains(&k.value) {
                return Err("Key pressure requires key 0..127 and value 0..1".into());
            }
            if std::mem::replace(&mut used[usize::from(k.key)], true) {
                return Err("Key-pressure values must have distinct keys".into());
            }
        }
    }
    if reset && (bend.is_some() || pressure.is_some() || keys.is_some() || range.is_some()) {
        return Err("Send reset separately from expression changes".into());
    }
    // Validate the complete batch before changing any range or live state.
    let has_live_change = reset
        || bend.is_some()
        || pressure.is_some()
        || keys.as_ref().is_some_and(|keys| !keys.is_empty());
    if has_live_change && s.live_target().is_none() {
        return Err("Select an instrument channel before sending expression".into());
    }
    if let Some(range) = range {
        s.set_pitch_bend_range(range)?;
    }
    let queued = if reset {
        s.reset_expression()
    } else {
        let mut ok = true;
        if let Some(v) = bend {
            ok &= s.pitch_bend(v);
        }
        if let Some(v) = pressure {
            ok &= s.channel_pressure(v);
        }
        if let Some(keys) = keys {
            for k in keys {
                ok &= s.poly_pressure(k.key, k.value);
            }
        }
        ok
    };
    if !queued {
        return Err("Expression was not queued; select an instrument channel and retry".into());
    }
    Ok(())
}
