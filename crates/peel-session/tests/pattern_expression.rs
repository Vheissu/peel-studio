//! Saved expression reaches the real native synth through playback and export.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32};

use peel_document::{
    Channel, Clip, ClipContent, DeviceInstance, ExpressionPoint, Id, Note, NoteExpression, PALETTE,
    Project, Time,
};
use peel_engine::{EngineCommand as Cmd, EngineConfig, OfflineEngine};
use peel_session::{CompileContext, ExportSettings, PlayMode, compile, render_to_wav};

const RATE: usize = 48_000;
fn project(pressure: f32, expressed: bool) -> Project {
    let mut p = Project::empty("Recorded expression");
    let mut device = DeviceInstance::new("peel.synth.poly", 1);
    for (key, value) in [
        ("osc1.wave", 0.0),
        ("osc2.wave", 0.0),
        ("osc.mix", 0.0),
        ("osc.sub", 0.0),
        ("osc.noise", 0.0),
        ("unison.voices", 0.0),
        ("filter.cutoff", 10_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("filter.velocity", 0.0),
        ("filter.drive", 0.0),
        ("filter.resonance", 0.0),
        ("amp.attack", 0.001),
        ("amp.decay", 0.01),
        ("amp.sustain", 1.0),
        ("out.gain", -12.0),
    ] {
        device.params.insert(key.into(), value);
    }
    let channel = Id::new();
    p.channels.push(Channel {
        id: channel,
        name: "Poly".into(),
        color: PALETTE[0],
        device,
        track: p.mixer.tracks[0].id,
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        root_key: 60,
    });
    let pid = p.patterns[0].id;
    let pt = p.pattern_mut(pid).unwrap();
    pt.notes = vec![
        Note::new(channel, Time::ZERO, Time::beats(4), 60, 0.5),
        Note::new(channel, Time::ZERO, Time::beats(4), 60, 0.5),
    ];
    if expressed {
        for (i, note) in pt.notes.iter().enumerate() {
            pt.expression.push(NoteExpression {
                note: note.id,
                pitch: vec![
                    ExpressionPoint {
                        offset: Time::ZERO,
                        value: 0.0,
                    },
                    ExpressionPoint {
                        offset: Time::beats(1),
                        value: if i == 0 { 3.0 } else { -3.0 },
                    },
                ],
                pressure: vec![
                    ExpressionPoint {
                        offset: Time::ZERO,
                        value: 0.0,
                    },
                    ExpressionPoint {
                        offset: Time::beats(1),
                        value: if i == 0 { pressure } else { 0.0 },
                    },
                ],
                timbre: vec![ExpressionPoint {
                    offset: Time::ZERO,
                    value: 0.5,
                }],
            });
        }
    }
    let lane = p.arrangement().tracks[0].id;
    p.arrangement_mut().clips.push(Clip {
        id: Id::new(),
        track: lane,
        start: Time::ZERO,
        length: Time::beats(4),
        offset: Time::ZERO,
        muted: false,
        content: ClipContent::Pattern { pattern: pid },
    });
    assert!(peel_document::validate(&p).iter().all(|i| !i.is_error()));
    p
}
fn render(p: &Project, block: usize) -> Vec<f32> {
    let config = EngineConfig {
        max_block: 512,
        ..EngineConfig::default()
    };
    let mut e = OfflineEngine::new(RATE as f32, &config);
    let mut device = peel_devices::create("peel.synth.poly").unwrap();
    for (key, value) in &p.channels[0].device.params {
        let param = device.descriptor().param_by_key(key).unwrap();
        device.set_param(param.id, *value);
    }
    device.prepare(RATE as f32, 512);
    e.send(Cmd::InstallDevice { slot: 0, device });
    let slots = HashMap::from([(p.channels[0].device.id, 0)]);
    let meters = HashMap::from([(p.mixer.tracks[0].id, 0)]);
    let (plan, _) = compile(
        p,
        &CompileContext {
            mode: PlayMode::Song,
            slots: &slots,
            meters: &meters,
            samples: &HashMap::new(),
        },
    );
    e.try_set_plan(Box::new(plan)).unwrap();
    e.send(Cmd::Play);
    let mut output = vec![0.0; RATE * 2];
    let mut right = vec![0.0; block];
    for left in output.chunks_mut(block) {
        e.render(left, &mut right[..left.len()]);
    }
    output
}
fn band(out: &[f32], frequency: f64) -> f64 {
    (-2..=2)
        .map(|delta| {
            let f = frequency + f64::from(delta);
            let mut a = 0.0;
            let mut b = 0.0;
            for (i, &value) in out.iter().enumerate() {
                let phase = std::f64::consts::TAU * f * i as f64 / RATE as f64;
                let window =
                    0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / out.len() as f64).cos();
                a += f64::from(value) * phase.cos() * window;
                b += f64::from(value) * phase.sin() * window;
            }
            (a * a + b * b).sqrt() / out.len() as f64
        })
        .fold(0.0, f64::max)
}
#[test]
fn same_key_recorded_notes_bend_in_opposite_directions_and_pressure_stays_per_voice() {
    let dry = render(&project(0.0, false), 256);
    let bent = render(&project(0.0, true), 256);
    let pressed = render(&project(0.8, true), 256);
    let window = RATE..RATE + RATE / 2;
    let high = 261.625565 * 2f64.powf(3.0 / 12.0);
    let low = 261.625565 * 2f64.powf(-3.0 / 12.0);
    for frequency in [high, low] {
        let baseline = band(&dry[window.clone()], frequency);
        let moved = band(&bent[window.clone()], frequency);
        assert!(
            moved > baseline * 5.0 + 0.0001,
            "{frequency}: bent{moved}, plain{baseline}"
        );
    }
    let high_gain = band(&pressed[window.clone()], high) / band(&bent[window.clone()], high);
    let other_gain = band(&pressed[window.clone()], low) / band(&bent[window], low);
    assert!((1.25..1.8).contains(&high_gain), "pressure gain{high_gain}");
    assert!(
        (other_gain - 1.0).abs() < 0.05,
        "other voice changed by{other_gain}"
    );
}
#[test]
fn saved_expression_renders_the_same_in_exports_and_different_block_sizes() {
    let p = project(0.6, true);
    let small = render(&p, 37);
    let large = render(&p, 512);
    let difference = small
        .iter()
        .zip(&large)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(difference < 1e-5, "block difference{difference}");
    let path = std::env::temp_dir().join(format!("peel-expression-{}.wav", Id::new()));
    render_to_wav(
        &p,
        &HashMap::new(),
        &ExportSettings {
            path: path.clone(),
            sample_rate: RATE as u32,
            bits: 32,
            tail_seconds: 0.0,
        },
        &AtomicU32::new(0),
        &AtomicBool::new(false),
    )
    .unwrap();
    let exported = peel_media::decode_file(&path).unwrap();
    assert_eq!(exported.frames(), small.len());
    let difference = small
        .iter()
        .zip(exported.left())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(difference < 1e-5, "export difference{difference}");
    std::fs::remove_file(path).unwrap();
}
