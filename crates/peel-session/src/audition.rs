//! Renders one note of a native instrument so the browser can play it
//! before anything is added to the project.

use peel_dsp::{DeviceEvent, DeviceKind, EventKind, ProcessContext, SampleBuffer};

const BLOCK: usize = 256;
/// Longest audition, held note and tail together.
const MAX_S: f32 = 3.0;
/// The tail is cut once it stays below this level.
const SILENCE: f32 = 0.000_3;

/// The key that shows a sound off: drums on their root, basses and guitars
/// in their own register, everything else at middle C.
pub fn key(type_id: &str, preset: Option<&str>) -> u8 {
    match type_id {
        // The whole kit follows GM drum keys; play its kick.
        "peel.drum.acoustic" if preset.is_none_or(|p| p == "Full Kit") => 36,
        "peel.bass.808" | "peel.synth.bass" | "peel.bass.recorded" => 36,
        "peel.guitar.recorded" | "peel.guitar.palm_muted" | "peel.guitar.electric" => 52,
        _ => 60,
    }
}

/// Plays one note on a new instance of `type_id` with `preset` applied and
/// returns the recording, or `None` for an unknown device, an effect or an
/// instrument that makes no sound on its own (a sampler with no sample).
pub fn render(type_id: &str, preset: Option<&str>, rate: f32) -> Option<SampleBuffer> {
    let desc = peel_devices::descriptor(type_id)?;
    if desc.kind != DeviceKind::Instrument {
        return None;
    }
    let mut dev = peel_devices::create(type_id)?;
    dev.prepare(rate, BLOCK);
    if let Some(p) = preset.and_then(|name| {
        peel_devices::presets(type_id)
            .iter()
            .find(|p| p.name == name)
    }) {
        peel_devices::apply_preset(dev.as_mut(), p);
        // Recorded instruments choose their samples in `prepare`.
        dev.prepare(rate, BLOCK);
    }
    let key = key(type_id, preset);
    // A drum is a hit; anything else is held long enough to hear it sustain.
    let held = if desc.category == "Drums" { 0.15 } else { 0.7 };
    let release = (held * rate) as usize;
    let frames = (MAX_S * rate) as usize;
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let mut pos = 0;
    while pos < frames {
        let n = BLOCK.min(frames - pos);
        let mut events = Vec::new();
        if pos == 0 {
            events.push(DeviceEvent {
                offset: 0,
                kind: EventKind::NoteOn {
                    key,
                    velocity: 0.85,
                    voice: 1,
                },
            });
        }
        if (pos..pos + n).contains(&release) {
            events.push(DeviceEvent {
                offset: (release - pos) as u32,
                kind: EventKind::NoteOff {
                    key,
                    velocity: 0.0,
                    voice: 1,
                },
            });
        }
        let mut ctx = ProcessContext::new(rate);
        ctx.playing = true;
        ctx.beat_position = pos as f64 * 2.0 / rate as f64;
        dev.process(&ctx, &events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    // Drop the silent end, then fade the last few milliseconds so a tail
    // that reaches the limit does not click.
    let end = (0..frames)
        .rev()
        .find(|&i| l[i].abs() > SILENCE || r[i].abs() > SILENCE)?
        + 1;
    l.truncate(end);
    r.truncate(end);
    let fade = ((0.02 * rate) as usize).min(end);
    for i in 0..fade {
        let g = i as f32 / fade as f32;
        l[end - 1 - i] *= g;
        r[end - 1 - i] *= g;
    }
    let name = preset.unwrap_or(desc.name);
    Some(SampleBuffer::new(name, rate, vec![l, r]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_instrument_preset_auditions_with_sound_and_headroom() {
        for d in peel_devices::all().filter(|d| d.kind == DeviceKind::Instrument) {
            let presets = peel_devices::presets(d.type_id);
            let names: Vec<Option<&str>> = if presets.is_empty() {
                vec![None]
            } else {
                presets.iter().map(|p| Some(p.name)).collect()
            };
            for preset in names {
                let Some(buf) = render(d.type_id, preset, 48_000.0) else {
                    // Only a sampler with nothing loaded is silent.
                    assert_eq!(d.type_id, "peel.sampler", "{} {preset:?}", d.type_id);
                    continue;
                };
                let peak = buf
                    .channels
                    .iter()
                    .flatten()
                    .fold(0.0f32, |m, x| m.max(x.abs()));
                assert!(
                    (0.02..=1.0).contains(&peak),
                    "{} {preset:?} peaks at {peak}",
                    d.type_id
                );
                assert!(buf.frames() <= (MAX_S * 48_000.0) as usize);
                let last = buf.channels[0][buf.frames() - 1].abs();
                assert!(last < 0.01, "{} {preset:?} ends at {last}", d.type_id);
            }
        }
    }

    #[test]
    fn effects_and_unknown_devices_have_no_audition() {
        assert!(render("peel.fx.nothing", None, 48_000.0).is_none());
        let fx = peel_devices::all()
            .find(|d| d.kind == DeviceKind::Effect)
            .unwrap();
        assert!(render(fx.type_id, None, 48_000.0).is_none());
    }
}
