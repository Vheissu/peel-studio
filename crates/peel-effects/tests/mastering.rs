//! End-to-end mastering behavior, including the delay and final ceiling.

mod common;

use common::*;
use peel_dsp::{Device, DeviceEvent, EventKind, ProcessContext};
use peel_effects::mastering::{self, Mastering};

fn transparent(sr: f32) -> Mastering {
    let mut m = Mastering::new();
    m.set_param(mastering::ID_RATIO, 1.0);
    m.prepare(sr, 256);
    m
}

#[test]
fn reports_the_limiter_delay_and_passes_a_quiet_impulse_unchanged() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut m = transparent(sr);
        let latency = m.latency() as usize;
        assert_eq!(latency, (sr * 0.004).round() as usize);
        assert!(m.tail_seconds().unwrap() >= latency as f32 / sr);
        let mut l = vec![0.0; latency + 256];
        let mut r = l.clone();
        l[7] = 0.1;
        r[7] = -0.05;
        m.process(&ProcessContext::new(sr), &[], &mut l, &mut r);
        assert_eq!(l.iter().position(|v| v.abs() > 1e-6), Some(latency + 7));
        assert!((l[latency + 7] - 0.1).abs() < 1e-6);
        assert!((r[latency + 7] + 0.05).abs() < 1e-6);
        m.set_param(mastering::ID_TRUE_PEAK, 0.0);
        m.set_param(mastering::ID_ATTACK, 0.1);
        assert_eq!(m.latency() as usize, latency);
    }
}

#[test]
fn every_preset_keeps_hot_stereo_program_below_the_final_ceiling() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        for preset in &mastering::PRESETS {
            let mut m = make("peel.fx.mastering", Some(preset), sr, 256);
            m.set_param(mastering::ID_INPUT, 18.0);
            m.set_param(mastering::ID_LOW, 6.0);
            m.set_param(mastering::ID_HIGH, 6.0);
            m.set_param(mastering::ID_MAKEUP, 12.0);
            m.set_param(mastering::ID_WIDTH, 2.0);
            m.set_param(mastering::ID_CEILING, -2.0);
            m.prepare(sr, 256);
            let ceiling = peel_dsp::util::db_to_gain(-2.0);
            let ctx = ProcessContext::new(sr);
            let mut noise = Noise::new(123);
            let mut maximum = 0.0f32;
            let mut reduction = 0.0f32;
            for _ in 0..160 {
                let mut l = [0.0; 256];
                let mut r = [0.0; 256];
                for (l, r) in l.iter_mut().zip(&mut r) {
                    *l = noise.next() * 2.0;
                    *r = noise.next() * 2.0;
                }
                m.process(&ctx, &[], &mut l, &mut r);
                assert!(l.iter().chain(&r).all(|v| v.is_finite()));
                maximum = maximum.max(peak(&l)).max(peak(&r));
                reduction = reduction.max(m.reduction_db().unwrap());
            }
            assert!(
                maximum <= ceiling + 1e-6,
                "{} / {sr}: {maximum}",
                preset.name
            );
            assert!(
                maximum > ceiling * 0.2,
                "unexpected silence: {} / {sr}: {maximum}",
                preset.name
            );
            assert!(reduction > 6.0);
        }
    }
}

#[test]
fn width_zero_sums_to_mono_and_width_does_not_change_a_mono_source() {
    let ctx = ProcessContext::new(SR);
    let (source_l, source_r) = program(8192, 41);
    let mut narrow = transparent(SR);
    narrow.set_param(mastering::ID_WIDTH, 0.0);
    narrow.prepare(SR, 256);
    let (mut l, mut r) = (source_l, source_r);
    run(&mut narrow, &mut l, &mut r, 256, &[]);
    assert_eq!(l, r);

    let src: Vec<f32> = (0..4096).map(|i| sine(440.0, i) * 0.1).collect();
    let mut reference = Vec::new();
    for width in [0.0, 1.0, 2.0] {
        let mut m = transparent(SR);
        m.set_param(mastering::ID_WIDTH, width);
        m.prepare(SR, 4096);
        let mut l = src.clone();
        let mut r = src.clone();
        m.process(&ctx, &[], &mut l, &mut r);
        assert_eq!(l, r);
        if reference.is_empty() {
            reference = l;
        } else {
            assert_eq!(reference, l);
        }
    }
}

#[test]
fn reset_flushes_filters_envelopes_and_the_lookahead_buffer() {
    let mut m = Mastering::new();
    m.set_param(mastering::ID_LOW, 6.0);
    m.set_param(mastering::ID_HIGH, -3.0);
    m.set_param(mastering::ID_INPUT, 12.0);
    m.prepare(SR, 1024);
    let (mut l, mut r) = program(8192, 75);
    run(&mut m, &mut l, &mut r, 256, &[]);
    m.reset();
    let mut l = [0.0; 1024];
    let mut r = l;
    m.process(&ProcessContext::new(SR), &[], &mut l, &mut r);
    assert_eq!(peak(&l), 0.0);
    assert_eq!(peak(&r), 0.0);
    assert_eq!(m.gain_reduction_db(), 0.0);
    assert_eq!(m.param(mastering::ID_LOW), 6.0);
}

#[test]
fn automated_ceiling_still_limits_and_gain_parameters_remain_independent() {
    let mut m = transparent(SR);
    m.set_param(mastering::ID_INPUT, 12.0);
    m.prepare(SR, 2048);
    let mut l = [0.9; 2048];
    let mut r = [-0.9; 2048];
    let event = DeviceEvent {
        offset: 700,
        kind: EventKind::Param {
            id: mastering::ID_CEILING,
            value: -6.0,
        },
    };
    m.process(&ProcessContext::new(SR), &[event], &mut l, &mut r);
    let ceiling = peel_dsp::util::db_to_gain(-6.0);
    assert!(
        l[700..]
            .iter()
            .chain(&r[700..])
            .all(|v| v.abs() <= ceiling + 1e-6)
    );
    assert_eq!(m.param(mastering::ID_INPUT), 12.0);
    assert_eq!(m.param(mastering::ID_MAKEUP), 0.0);
    assert_eq!(m.param(mastering::ID_CEILING), -6.0);
}
