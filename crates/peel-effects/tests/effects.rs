//! Behavior every effect must share: registry integrity, silence, finite
//! output at parameter extremes, determinism, block-size independence,
//! sample-accurate parameter events, reset, tails and sample-rate changes.

mod common;

use common::*;
use peel_dsp::{DeviceKind, EventKind, ParamDescriptor};

const TYPE_IDS: [&str; 15] = [
    "peel.fx.eq",
    "peel.fx.compressor",
    "peel.fx.limiter",
    "peel.fx.delay",
    "peel.fx.reverb",
    "peel.fx.drive",
    "peel.fx.filter",
    "peel.fx.chorus",
    "peel.fx.utility",
    "peel.fx.bass_lift",
    "peel.fx.vintage_chorus",
    "peel.fx.flanger",
    "peel.fx.phaser",
    "peel.fx.glitch",
    "peel.fx.multiband",
];

#[test]
fn registry_lists_every_required_type() {
    let ids = all_type_ids();
    assert_eq!(ids, TYPE_IDS);
    for id in TYPE_IDS {
        let d = peel_effects::create(id).expect(id);
        assert_eq!(d.descriptor().type_id, id);
        assert_eq!(d.descriptor().kind, DeviceKind::Effect);
    }
    assert!(peel_effects::create("peel.fx.nope").is_none());
    assert!(peel_effects::presets("peel.fx.nope").is_empty());
}

#[test]
fn descriptors_are_well_formed() {
    for desc in peel_effects::descriptors() {
        let id = desc.type_id;
        assert!(!desc.name.is_empty() && !desc.description.is_empty() && !desc.category.is_empty());
        assert!(!desc.params.is_empty(), "{id}");
        let mut ids = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        for p in desc.params {
            assert!(ids.insert(p.id), "{id}: duplicate param id {}", p.id);
            assert!(keys.insert(p.key), "{id}: duplicate key {}", p.key);
            assert!(!p.name.is_empty() && !p.group.is_empty(), "{id}.{}", p.key);
            assert!(p.min < p.max, "{id}.{}: empty range", p.key);
            assert!(
                p.default >= p.min && p.default <= p.max,
                "{id}.{}: default {} outside {}..{}",
                p.key,
                p.default,
                p.min,
                p.max
            );
            if p.taper == peel_dsp::Taper::Log {
                assert!(p.min > 0.0, "{id}.{}: log taper needs min > 0", p.key);
            }
            if p.is_choice() {
                assert_eq!(p.max as usize + 1, p.choices.len(), "{id}.{}", p.key);
                assert_eq!(p.default.fract(), 0.0);
            }
            assert!(
                p.key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
            );
            // Normalized mapping round-trips.
            let n = p.to_normalized(p.default);
            let back = p.from_normalized(n);
            assert!(
                (back - p.default).abs() <= (p.max - p.min) * 1e-3 + 1e-4,
                "{id}.{}",
                p.key
            );
        }
        // A UI groups by contiguous runs: a group may not be split up.
        let mut seen: Vec<&str> = Vec::new();
        for p in desc.params {
            if seen.last() != Some(&p.group) {
                assert!(
                    !seen.contains(&p.group),
                    "{id}: group {} is not contiguous",
                    p.group
                );
                seen.push(p.group);
            }
        }
    }
}

#[test]
fn defaults_round_trip_through_the_device() {
    for id in TYPE_IDS {
        let d = make(id, None, SR, 256);
        for p in d.descriptor().params {
            assert_eq!(d.param(p.id), p.default, "{id}.{}", p.key);
        }
    }
}

#[test]
fn param_values_are_clamped_and_unknown_ids_ignored() {
    for id in TYPE_IDS {
        let mut d = make(id, None, SR, 256);
        for p in d.descriptor().params {
            d.set_param(p.id, p.max + 1e6);
            assert_eq!(d.param(p.id), p.max, "{id}.{}", p.key);
            d.set_param(p.id, p.min - 1e6);
            assert_eq!(d.param(p.id), p.min, "{id}.{}", p.key);
            d.set_param(p.id, f32::NAN);
            assert!(d.param(p.id).is_finite(), "{id}.{}", p.key);
            d.set_param(p.id, f32::INFINITY);
            assert!(d.param(p.id).is_finite(), "{id}.{}", p.key);
        }
        d.set_param(99_999, 1.0); // must not panic
    }
}

#[test]
fn presets_are_valid_and_the_first_equals_defaults() {
    for id in TYPE_IDS {
        let presets = peel_effects::presets(id);
        assert!(
            (3..=12).contains(&presets.len()),
            "{id}: {} presets",
            presets.len()
        );
        let desc = peel_effects::descriptors()
            .iter()
            .find(|d| d.type_id == id)
            .unwrap();
        // First preset leaves every parameter at its default.
        let first = make(id, Some(&presets[0]), SR, 256);
        for p in desc.params {
            assert_eq!(
                first.param(p.id),
                p.default,
                "{id}: first preset moves {}",
                p.key
            );
        }
        let mut names = std::collections::HashSet::new();
        for preset in presets {
            assert!(
                names.insert(preset.name),
                "{id}: duplicate preset {}",
                preset.name
            );
            for (key, value) in preset.params {
                let p: &ParamDescriptor = desc.param_by_key(key).unwrap_or_else(|| {
                    panic!("{id}: preset {} has unknown key {key}", preset.name)
                });
                assert!(
                    *value >= p.min && *value <= p.max,
                    "{id}: preset {} sets {key} to {value}, outside {}..{}",
                    preset.name,
                    p.min,
                    p.max
                );
            }
        }
    }
}

#[test]
fn silence_in_gives_silence_out() {
    for id in TYPE_IDS {
        let presets = peel_effects::presets(id);
        for preset in presets {
            let mut d = make(id, Some(preset), SR, 256);
            let mut l = vec![0.0; SR as usize];
            let mut r = vec![0.0; SR as usize];
            run(d.as_mut(), &mut l, &mut r, 256, &[]);
            assert!(
                peak(&l) < 1e-6 && peak(&r) < 1e-6,
                "{id}/{}: peak {} {}",
                preset.name,
                peak(&l),
                peak(&r)
            );
        }
    }
}

#[test]
fn tails_decay_to_silence() {
    // After the signal stops, every effect settles within its reported tail
    // (plus its latency) and `is_silent` agrees.
    for id in TYPE_IDS {
        for preset in peel_effects::presets(id) {
            let mut d = make(id, Some(preset), SR, 256);
            let (mut l, mut r) = program(12_000, 3);
            run(d.as_mut(), &mut l, &mut r, 256, &[]);
            let tail = d.tail_seconds().unwrap_or(30.0).min(31.0);
            let frames = ((tail + 0.3) * SR) as usize + d.latency() as usize;
            let mut l = vec![0.0; frames];
            let mut r = vec![0.0; frames];
            run(d.as_mut(), &mut l, &mut r, 256, &[]);
            let late = (frames as f32 * 0.9) as usize;
            // Within the reported tail the level must fall below -60 dB
            // relative to a 0.5 input; reverbs and delays are measured on
            // the last tenth of the window.
            assert!(
                peak(&l[late..]) < 1e-3 && peak(&r[late..]) < 1e-3,
                "{id}/{}: still {} at {}s (tail {tail}s)",
                preset.name,
                peak(&l[late..]),
                late as f32 / SR
            );
            // And once settled for a long while, the device may say so.
            let mut l = vec![0.0; (SR * 2.0 * tail.max(1.0) + SR) as usize];
            let mut r = l.clone();
            run(d.as_mut(), &mut l, &mut r, 256, &[]);
            assert!(
                d.is_silent(),
                "{id}/{}: not silent after long silence",
                preset.name
            );
        }
    }
}

fn extreme_checks(d: &mut dyn peel_dsp::Device, what: &str, limit: f32) {
    // Impulse, loud sine, noise, then silence: all finite and sane.
    let n = 24_000;
    let (mut l, mut r) = program(n, 11);
    // Make it hot to stress saturation and resonance.
    for v in l.iter_mut().chain(r.iter_mut()) {
        *v *= 1.8;
    }
    run(d, &mut l, &mut r, 128, &[]);
    for (i, v) in l.iter().chain(r.iter()).enumerate() {
        assert!(v.is_finite(), "{what}: non-finite at {i}");
        assert!(v.abs() < limit, "{what}: runaway {v} at {i}");
    }
    let mut l = vec![0.0; 24_000];
    let mut r = vec![0.0; 24_000];
    run(d, &mut l, &mut r, 128, &[]);
    for v in l.iter().chain(r.iter()) {
        assert!(v.is_finite() && v.abs() < limit, "{what}: bad tail {v}");
    }
}

#[test]
fn output_is_finite_with_every_parameter_at_min_and_max() {
    for id in TYPE_IDS {
        let desc = peel_effects::descriptors()
            .iter()
            .find(|d| d.type_id == id)
            .unwrap();
        for p in desc.params {
            for (label, value) in [("min", p.min), ("max", p.max)] {
                let mut d = make(id, None, SR, 128);
                d.set_param(p.id, value);
                extreme_checks(d.as_mut(), &format!("{id}.{} at {label}", p.key), 200.0);
            }
        }
        for (label, pick) in [("all min", false), ("all max", true)] {
            let mut d = make(id, None, SR, 128);
            for p in desc.params {
                d.set_param(p.id, if pick { p.max } else { p.min });
            }
            d.prepare(SR, 128);
            // Everything at maximum can legitimately add up to well over
            // 100 dB of gain in an EQ; only finiteness is required there.
            extreme_checks(d.as_mut(), &format!("{id} {label}"), 1e9);
        }
    }
}

#[test]
fn presets_produce_sane_audio() {
    for id in TYPE_IDS {
        for preset in peel_effects::presets(id) {
            let mut d = make(id, Some(preset), SR, 256);
            let (mut l, mut r) = program(30_000, 5);
            run(d.as_mut(), &mut l, &mut r, 256, &[]);
            assert!(
                l.iter()
                    .chain(r.iter())
                    .all(|v| v.is_finite() && v.abs() < 16.0),
                "{id}/{}",
                preset.name
            );
            assert!(
                peak(&l) > 1e-3 || peak(&r) > 1e-3,
                "{id}/{} is silent",
                preset.name
            );
        }
    }
}

#[test]
fn nan_and_infinity_in_the_input_do_not_poison_state() {
    for id in TYPE_IDS {
        let mut d = make(id, None, SR, 256);
        let mut l = vec![f32::NAN; 512];
        let mut r = vec![f32::INFINITY; 512];
        run(d.as_mut(), &mut l, &mut r, 256, &[]);
        let (mut l, mut r) = program(SR as usize * 2, 8);
        run(d.as_mut(), &mut l, &mut r, 256, &[]);
        assert!(
            l[SR as usize..]
                .iter()
                .chain(r[SR as usize..].iter())
                .all(|v| v.is_finite()),
            "{id} stayed poisoned"
        );
    }
}

#[test]
fn garbage_transport_values_are_harmless() {
    for id in TYPE_IDS {
        let mut d = make(id, None, SR, 256);
        let desc = d.descriptor();
        // Turn on every tempo-synced path that exists.
        for key in ["sync", "lfo.sync"] {
            if let Some(p) = desc.param_by_key(key) {
                d.set_param(p.id, 1.0);
            }
        }
        for (tempo, beat) in [
            (f64::NAN, f64::NAN),
            (0.0, -5.0),
            (f64::INFINITY, f64::INFINITY),
            (-120.0, 1e300),
        ] {
            let mut ctx = peel_dsp::ProcessContext::new(SR);
            ctx.tempo_bpm = tempo;
            ctx.beat_position = beat;
            ctx.playing = true;
            let (mut l, mut r) = program(4_096, 12);
            for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
                d.process(&ctx, &[], cl, cr);
            }
            assert!(
                l.iter().chain(r.iter()).all(|v| v.is_finite()),
                "{id} with tempo {tempo}"
            );
        }
        // And it recovers once the transport is sane again.
        let ctx = peel_dsp::ProcessContext::new(SR);
        let (mut l, mut r) = program(SR as usize, 13);
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            d.process(&ctx, &[], cl, cr);
        }
        assert!(
            l.iter().chain(r.iter()).all(|v| v.is_finite()),
            "{id} did not recover"
        );
    }
}

#[test]
fn processing_is_deterministic() {
    for id in TYPE_IDS {
        for preset in peel_effects::presets(id) {
            let (src_l, src_r) = program(40_000, 21);
            let mut out = Vec::new();
            for _ in 0..2 {
                let mut d = make(id, Some(preset), SR, 256);
                let (mut l, mut r) = (src_l.clone(), src_r.clone());
                run(d.as_mut(), &mut l, &mut r, 256, &[]);
                out.push((l, r));
            }
            assert_eq!(out[0], out[1], "{id}/{}", preset.name);
        }
    }
}

#[test]
fn results_do_not_depend_on_block_size() {
    for id in TYPE_IDS {
        for preset in peel_effects::presets(id) {
            let (src_l, src_r) = program(40_000, 33);
            let mut results = Vec::new();
            for block in [32usize, 256, 77, 1024] {
                let mut d = make(id, Some(preset), SR, block.max(256));
                let (mut l, mut r) = (src_l.clone(), src_r.clone());
                run(d.as_mut(), &mut l, &mut r, block, &[]);
                results.push((block, l, r));
            }
            let (_, base_l, base_r) = &results[0];
            for (block, l, r) in &results[1..] {
                let worst = base_l
                    .iter()
                    .zip(l)
                    .chain(base_r.iter().zip(r))
                    .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
                assert!(
                    worst < 2e-4,
                    "{id}/{}: block {block} differs from block 32 by {worst}",
                    preset.name
                );
            }
        }
    }
}

#[test]
fn parameter_events_split_the_block_sample_accurately() {
    for id in TYPE_IDS {
        let desc = peel_effects::descriptors()
            .iter()
            .find(|d| d.type_id == id)
            .unwrap();
        for p in desc.params {
            let (src_l, src_r) = program(2_048, 17);
            let target = p.from_normalized(0.8);
            let at = 700usize;

            // Via an event inside one big block.
            let mut a = make(id, None, SR, 2_048);
            let (mut al, mut ar) = (src_l.clone(), src_r.clone());
            run(
                a.as_mut(),
                &mut al,
                &mut ar,
                2_048,
                &[(
                    at,
                    EventKind::Param {
                        id: p.id,
                        value: target,
                    },
                )],
            );

            // By hand: process up to the frame, set the parameter, continue.
            let mut b = make(id, None, SR, 2_048);
            let (mut bl, mut br) = (src_l.clone(), src_r.clone());
            run(b.as_mut(), &mut bl[..at], &mut br[..at], 2_048, &[]);
            b.set_param(p.id, target);
            run(b.as_mut(), &mut bl[at..], &mut br[at..], 2_048, &[]);

            assert_eq!(al, bl, "{id}.{}: left differs", p.key);
            assert_eq!(ar, br, "{id}.{}: right differs", p.key);
            assert_eq!(a.param(p.id), b.param(p.id));
        }
    }
}

#[test]
fn events_for_notes_are_ignored_and_all_sound_off_clears_tails() {
    let note_events = [
        (
            10usize,
            EventKind::NoteOn {
                key: 60,
                velocity: 1.0,
                voice: 1,
            },
        ),
        (
            500,
            EventKind::NoteOff {
                key: 60,
                velocity: 0.0,
                voice: 1,
            },
        ),
        (900, EventKind::AllNotesOff),
    ];
    for id in TYPE_IDS {
        let (src_l, src_r) = program(4_096, 2);
        let mut plain = make(id, None, SR, 4_096);
        let (mut pl, mut pr) = (src_l.clone(), src_r.clone());
        run(plain.as_mut(), &mut pl, &mut pr, 4_096, &[]);
        let mut noted = make(id, None, SR, 4_096);
        let (mut nl, mut nr) = (src_l.clone(), src_r.clone());
        run(noted.as_mut(), &mut nl, &mut nr, 4_096, &note_events);
        assert_eq!(pl, nl, "{id} reacted to notes");
        assert_eq!(pr, nr);

        // Panic: AllSoundOff silences the ringing tail immediately.
        let mut d = make(id, None, SR, 256);
        let (mut l, mut r) = program(12_000, 4);
        run(d.as_mut(), &mut l, &mut r, 256, &[]);
        let mut l = vec![0.0; 256];
        let mut r = vec![0.0; 256];
        run(
            d.as_mut(),
            &mut l,
            &mut r,
            256,
            &[(0, EventKind::AllSoundOff)],
        );
        let limiter_or_drive_pad = d.latency() as usize;
        assert!(
            peak(&l[limiter_or_drive_pad.min(255)..]) < 1e-6
                && peak(&r[limiter_or_drive_pad.min(255)..]) < 1e-6,
            "{id}: sound after AllSoundOff"
        );
    }
}

#[test]
fn reset_clears_state() {
    for id in TYPE_IDS {
        let mut d = make(id, None, SR, 256);
        let (mut l, mut r) = program(12_000, 6);
        run(d.as_mut(), &mut l, &mut r, 256, &[]);
        d.reset();
        let mut l = vec![0.0; 24_000];
        let mut r = vec![0.0; 24_000];
        run(d.as_mut(), &mut l, &mut r, 256, &[]);
        assert!(
            peak(&l) < 1e-6 && peak(&r) < 1e-6,
            "{id}: reset left {}",
            peak(&l)
        );
    }
}

#[test]
fn prepare_again_at_other_sample_rates() {
    for id in TYPE_IDS {
        let mut d = make(id, None, SR, 256);
        let mut lat = Vec::new();
        for sr in [44_100.0f32, 96_000.0, 22_050.0, 48_000.0] {
            d.prepare(sr, 512);
            lat.push(d.latency());
            let (mut l, mut r) = program(20_000, 9);
            let mut ctx = peel_dsp::ProcessContext::new(sr);
            ctx.tempo_bpm = 133.0;
            for (cl, cr) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
                d.process(&ctx, &[], cl, cr);
            }
            assert!(
                l.iter()
                    .chain(r.iter())
                    .all(|v| v.is_finite() && v.abs() < 50.0),
                "{id} at {sr}"
            );
        }
        // Latency, when there is any, tracks the sample rate sensibly.
        assert!(lat.iter().all(|l| *l < 2_000), "{id}: {lat:?}");
    }
}

#[test]
fn processes_odd_and_empty_buffers() {
    for id in TYPE_IDS {
        let mut d = make(id, None, SR, 64);
        let ctx = peel_dsp::ProcessContext::new(SR);
        d.process(&ctx, &[], &mut [], &mut []);
        for n in [1usize, 2, 3, 7, 63, 64] {
            let mut l = vec![0.25; n];
            let mut r = vec![-0.25; n];
            d.process(&ctx, &[], &mut l, &mut r);
            assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()));
        }
        // Larger than the prepared maximum block must still be safe.
        let mut l = vec![0.1; 5_000];
        let mut r = vec![0.1; 5_000];
        d.process(&ctx, &[], &mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()));
    }
}

#[test]
fn processing_cost_is_comfortably_realtime() {
    // Ten seconds of stereo audio per effect (every preset's worth of work
    // is similar); even debug-profile test builds run effects at opt-level 3.
    for id in TYPE_IDS {
        let mut d = make(id, None, SR, 512);
        let (mut l, mut r) = program(SR as usize * 10, 1);
        let start = std::time::Instant::now();
        run(d.as_mut(), &mut l, &mut r, 512, &[]);
        let secs = start.elapsed().as_secs_f32();
        eprintln!("{id}: {:.1}x realtime", 10.0 / secs.max(1e-6));
        assert!(secs < 5.0, "{id} took {secs}s for 10s of audio");
    }
}
