//! `peel audition`: renders one native device on its own, preset by preset,
//! so its sound can be heard and measured without a project.

use std::path::PathBuf;

use peel_dsp::{Device, DeviceEvent, DeviceKind, EventKind, Preset, ProcessContext};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;

struct Options {
    type_id: String,
    out: PathBuf,
    preset: Option<String>,
    key: Option<u8>,
    seconds: f32,
    set: Vec<(String, f32)>,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        type_id: args.first().ok_or(crate::USAGE)?.clone(),
        out: PathBuf::from(args.get(1).ok_or(crate::USAGE)?),
        preset: None,
        key: None,
        seconds: 2.0,
        set: Vec::new(),
    };
    let mut i = 2;
    while i < args.len() {
        let option = args[i].as_str();
        i += 1;
        let value = args
            .get(i)
            .ok_or_else(|| format!("Missing value for {option}"))?;
        let bad = || format!("Invalid value for {option}: {value}");
        match option {
            "--preset" => o.preset = Some(value.clone()),
            "--key" => o.key = Some(value.parse().ok().filter(|k| *k < 128).ok_or_else(bad)?),
            "--seconds" => {
                o.seconds = value
                    .parse()
                    .ok()
                    .filter(|s: &f32| s.is_finite() && (0.1..=60.0).contains(s))
                    .ok_or_else(bad)?;
            }
            "--set" => {
                let (key, v) = value.split_once('=').ok_or_else(bad)?;
                o.set.push((key.to_string(), v.parse().map_err(|_| bad())?));
            }
            other => return Err(format!("Unknown option {other}")),
        }
        i += 1;
    }
    Ok(o)
}

fn build(o: &Options, preset: Option<&Preset>) -> Result<Box<dyn Device>, String> {
    let mut dev = peel_devices::create(&o.type_id)
        .ok_or_else(|| format!("No native device called {}; see `peel devices`", o.type_id))?;
    dev.prepare(RATE, BLOCK);
    if let Some(p) = preset {
        peel_devices::apply_preset(dev.as_mut(), p);
    }
    let desc = dev.descriptor();
    for (key, value) in &o.set {
        let p = desc
            .param_by_key(key)
            .ok_or_else(|| format!("{} has no parameter {key}", o.type_id))?;
        dev.set_param(p.id, *value);
    }
    // Recorded instruments load their samples in `prepare`.
    dev.prepare(RATE, BLOCK);
    Ok(dev)
}

/// A drum-like hit, then a sustained three-note chord, then silence: enough
/// to hear what an effect does to a transient, a held sound and its tail.
fn effect_input(frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0f32; frames];
    let mut r = vec![0.0f32; frames];
    let third = frames / 3;
    for i in 0..third.min((0.4 * RATE) as usize) {
        let t = i as f32 / RATE;
        let body = (std::f32::consts::TAU * (55.0 + 140.0 * (-t * 40.0).exp()) * t).sin();
        let v = 0.7 * body * (-t * 9.0).exp();
        l[i] = v;
        r[i] = v;
    }
    let chord = [130.81f32, 164.81, 196.0];
    for i in 0..third {
        let t = i as f32 / RATE;
        let env = (t * 200.0).min(1.0) * ((third - i) as f32 / RATE * 200.0).min(1.0);
        let mut left = 0.0;
        let mut right = 0.0;
        for (n, f) in chord.iter().enumerate() {
            // Band-limited enough for a test tone: ten saw partials.
            for k in 1..=10 {
                let p = std::f32::consts::TAU * f * k as f32 * t;
                let a = 0.12 / k as f32;
                left += a * (p + n as f32).sin();
                right += a * (p + 1.3 * n as f32).sin();
            }
        }
        l[third + i] = left * env;
        r[third + i] = right * env;
    }
    (l, r)
}

fn render(dev: &mut dyn Device, key: u8, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let instrument = dev.descriptor().kind == DeviceKind::Instrument;
    let (mut l, mut r) = if instrument {
        (vec![0.0; frames], vec![0.0; frames])
    } else {
        effect_input(frames)
    };
    let release = frames / 2;
    let mut pos = 0;
    while pos < frames {
        let n = BLOCK.min(frames - pos);
        let mut events = Vec::new();
        if instrument {
            let mut at = |frame: usize, kind: EventKind| {
                if (pos..pos + n).contains(&frame) {
                    events.push(DeviceEvent {
                        offset: (frame - pos) as u32,
                        kind,
                    });
                }
            };
            at(
                0,
                EventKind::NoteOn {
                    key,
                    velocity: 0.85,
                    voice: 1,
                },
            );
            at(
                release,
                EventKind::NoteOff {
                    key,
                    velocity: 0.0,
                    voice: 1,
                },
            );
        }
        let mut ctx = ProcessContext::new(RATE);
        ctx.playing = true;
        ctx.beat_position = pos as f64 * 2.0 / RATE as f64;
        dev.process(&ctx, &events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    (l, r)
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1.0e-9).log10()
}

pub fn run(args: &[String]) -> Result<(), String> {
    let o = options(args)?;
    let desc = peel_devices::descriptor(&o.type_id)
        .ok_or_else(|| format!("No native device called {}; see `peel devices`", o.type_id))?;
    let all = peel_devices::presets(&o.type_id);
    let chosen: Vec<Option<&Preset>> = match &o.preset {
        Some(name) => vec![Some(
            all.iter()
                .find(|p| p.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("{} has no preset called {name}", desc.name))?,
        )],
        None if all.is_empty() => vec![None],
        None => all.iter().map(Some).collect(),
    };
    let key = o
        .key
        .unwrap_or(if desc.category == "Drums" { 60 } else { 48 });
    let frames = (o.seconds * RATE) as usize;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for preset in chosen {
        let mut dev = build(&o, preset)?;
        let (l, r) = render(dev.as_mut(), key, frames);
        let peak = l.iter().chain(&r).fold(0.0f32, |m, x| m.max(x.abs()));
        let head = &l[..l.len().min((0.25 * RATE) as usize)];
        let rms = (head.iter().map(|x| x * x).sum::<f32>() / head.len().max(1) as f32).sqrt();
        out!(
            "{:<24} peak {:6.1} dBFS   first 250 ms {:6.1} dB RMS",
            preset.map_or("(default)", |p| p.name),
            db(peak),
            db(rms)
        );
        left.extend(l);
        right.extend(r);
    }
    peel_media::write_wav(
        &o.out,
        &[&left, &right],
        RATE as u32,
        peel_media::WavFormat {
            bits: 24,
            dither: false,
        },
    )
    .map_err(|e| e.to_string())?;
    out!("wrote {}", o.out.display());
    Ok(())
}
