//! Audio region playback: placement, chase, loops, envelopes and declicks.
// The checks index output and source frames side by side on purpose.
#![allow(clippy::needless_range_loop)]

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use common::*;
use peel_dsp::SampleBuffer;
use peel_engine::{AudioRegion, EngineCommand, OfflineEngine, RenderPlan, Retired};
use peel_time::{TempoMap, TempoSegment, Time};

const SRF: f64 = SR as f64;
/// Entry and tail fade at 48 kHz (4 ms).
const DECLICK: usize = 192;
/// Edge fade at 48 kHz (1 ms).
const EDGE: usize = 48;
/// One beat at 120 BPM and 48 kHz.
const BEAT: usize = 24_000;

fn constant(bpm: f64) -> TempoMap {
    TempoMap::constant(bpm)
}

fn ramp_tempo() -> TempoMap {
    TempoMap::from_segments(&[
        TempoSegment {
            start: Time::ZERO,
            bpm: 100.0,
            ramp_to: Some(140.0),
        },
        TempoSegment {
            start: Time::beats(16),
            bpm: 140.0,
            ramp_to: None,
        },
    ])
}

fn mono(frames: usize, value: f32) -> Arc<SampleBuffer> {
    Arc::new(SampleBuffer::new("dc", SR, vec![vec![value; frames]]))
}

/// `sample[i] = i * 1e-5`: every frame is distinct and exactly checkable.
fn ramp(frames: usize) -> Arc<SampleBuffer> {
    let data: Vec<f32> = (0..frames).map(|i| i as f32 * 1e-5).collect();
    Arc::new(SampleBuffer::new("ramp", SR, vec![data]))
}

fn region(sample: &Arc<SampleBuffer>, start: f64, end: f64) -> AudioRegion {
    AudioRegion {
        track: 0,
        sample: sample.clone(),
        start_beat: start,
        end_beat: end,
        source_start_seconds: 0.0,
        gain: 1.0,
        reversed: false,
        fade_in_beats: 0.0,
        fade_out_beats: 0.0,
    }
}

fn audio_plan(
    tempo: TempoMap,
    loop_range: Option<(f64, f64)>,
    length: f64,
    regions: Vec<AudioRegion>,
) -> RenderPlan {
    let mut p = plan(tempo, &[], 0, loop_range, length);
    p.audio = regions;
    p
}

fn play(plan: RenderPlan) -> OfflineEngine {
    let mut off = rig(plan, None);
    off.send(EngineCommand::Play);
    off
}

fn render(off: &mut OfflineEngine, frames: usize) -> Vec<f32> {
    render_blocks(off, frames, 256).0
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-6 + 1e-6 * b.abs()
}

fn max_step(buf: &[f32]) -> f32 {
    buf.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

// ---------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------

#[test]
fn regions_land_on_exact_samples_for_every_block_size() {
    for tempo in [constant(133.3), ramp_tempo()] {
        let sample = mono(300_000, 0.5);
        let r = region(&sample, 1.0 / 3.0, 5.7);
        let make = || audio_plan(tempo.clone(), None, 64.0, vec![r.clone()]);
        let total = 250_000;

        let reference = {
            let mut off = play(make());
            render_blocks(&mut off, total, 256).0
        };
        let start = tempo.beat_to_sample(1.0 / 3.0, SRF) as usize;
        let end = tempo.beat_to_sample(5.7, SRF) as usize;
        assert_eq!(rising_edges(&reference), vec![start], "start sample");
        assert_eq!(falling_edges(&reference), vec![end], "end sample");
        assert!(
            reference[start + 2 * EDGE..end - 2 * EDGE]
                .iter()
                .all(|&x| x == 0.5),
            "the middle is the untouched source"
        );

        for block in [1usize, 64, 256, 1000] {
            let mut off = play(make());
            let (l, r) = render_blocks(&mut off, total, block);
            assert!(l == reference, "left differs for block size {block}");
            assert!(r == reference, "right differs for block size {block}");
        }
    }
}

#[test]
fn a_region_longer_than_its_sample_ends_with_the_sample() {
    let sample = mono(30_000, 0.5);
    let mut off = play(audio_plan(
        constant(120.0),
        None,
        16.0,
        vec![region(&sample, 1.0, 5.0)],
    ));
    let out = render(&mut off, 200_000);
    assert_eq!(rising_edges(&out), vec![BEAT]);
    assert_eq!(falling_edges(&out), vec![BEAT + 30_000]);
}

#[test]
fn source_offset_maps_frames_one_to_one() {
    let sample = ramp(100_000);
    let mut r = region(&sample, 2.0, 4.0);
    r.source_start_seconds = 0.0125; // 600 frames
    let mut off = play(audio_plan(constant(120.0), None, 16.0, vec![r]));
    let out = render(&mut off, 3 * BEAT * 2);

    let start = 2 * BEAT;
    for k in EDGE..2 * BEAT - EDGE {
        assert_eq!(out[start + k], sample.channels[0][600 + k], "frame {k}");
    }
    // The first frame is faded but audible, at exactly the start sample.
    assert_eq!(out[start - 1], 0.0);
    assert!(near(
        out[start],
        sample.channels[0][600] / (EDGE + 1) as f32
    ));
    assert_eq!(out[start + 2 * BEAT], 0.0, "the region ends at its end");
}

#[test]
fn gain_and_channel_handling() {
    let left: Vec<f32> = (0..50_000).map(|i| i as f32 * 1e-5).collect();
    let right: Vec<f32> = left.iter().map(|x| -x).collect();
    let stereo = Arc::new(SampleBuffer::new("st", SR, vec![left.clone(), right]));
    let mut r = region(&stereo, 1.0, 2.0);
    r.gain = 0.5;
    let mut off = play(audio_plan(constant(120.0), None, 8.0, vec![r]));
    let (l, rr) = render_blocks(&mut off, 2 * BEAT, 256);
    for k in 2 * EDGE..20_000 {
        assert_eq!(l[BEAT + k], left[k] * 0.5);
        assert_eq!(rr[BEAT + k], -left[k] * 0.5);
    }

    // A mono sample sounds in both channels.
    let m = ramp(50_000);
    let mut off = play(audio_plan(
        constant(120.0),
        None,
        8.0,
        vec![region(&m, 1.0, 2.0)],
    ));
    let (l, rr) = render_blocks(&mut off, 2 * BEAT, 256);
    assert!(l == rr);
    assert!(l[BEAT + 1000] > 0.0);
}

#[test]
fn reverse_plays_the_window_backwards() {
    let sample = ramp(100_000);
    let mut r = region(&sample, 1.0, 2.0);
    r.source_start_seconds = 0.1; // 4800 frames
    r.reversed = true;
    let mut off = play(audio_plan(constant(120.0), None, 8.0, vec![r]));
    let out = render(&mut off, 3 * BEAT);
    let len = BEAT; // the window is one beat
    for k in EDGE..len - EDGE {
        assert_eq!(
            out[BEAT + k],
            sample.channels[0][4800 + len - 1 - k],
            "frame {k}"
        );
    }
}

#[test]
fn clip_fades_shape_the_edges() {
    let sample = mono(100_000, 1.0);
    let mut r = region(&sample, 1.0, 3.0); // 48000 frames
    r.fade_in_beats = 0.5;
    r.fade_out_beats = 0.25;
    let mut off = play(audio_plan(constant(120.0), None, 8.0, vec![r]));
    let out = render(&mut off, 4 * BEAT);

    let fade_in = BEAT / 2;
    let fade_out = BEAT / 4;
    let start = BEAT;
    let end = 3 * BEAT;
    for k in [EDGE, 100, 1000, 6000, fade_in - 1] {
        let want = (k + 1) as f32 / (fade_in + 1) as f32;
        assert!(near(out[start + k], want), "fade in at {k}");
    }
    assert!(near(out[start + fade_in], 1.0));
    assert!(near(out[end - fade_out - 1], 1.0));
    for m in [EDGE, 100, 1000, fade_out - 1] {
        let want = (m + 1) as f32 / (fade_out + 1) as f32;
        assert!(near(out[end - 1 - m], want), "fade out at {m}");
    }
    assert!(max_step(&out) < 0.02, "no jumps: {}", max_step(&out));
}

#[test]
fn overlapping_regions_sum_and_tracks_route_independently() {
    let a = mono(100_000, 0.25);
    let b = mono(100_000, 0.125);
    let mut rb = region(&b, 2.0, 4.0);
    rb.track = 1;
    let mut p = audio_plan(constant(120.0), None, 16.0, vec![region(&a, 1.0, 3.0), rb]);
    p.tracks = vec![
        peel_engine::TrackRoute::new(Some(2), 0),
        peel_engine::TrackRoute::new(Some(2), 1),
        peel_engine::TrackRoute::new(None, 2),
    ];
    p.master = 2;
    p.tracks[1].gain = 2.0;
    let mut off = play(p);
    let out = render(&mut off, 5 * BEAT);
    let at = |beat_frac: f64| out[(beat_frac * BEAT as f64) as usize];
    assert_eq!(at(1.5), 0.25);
    assert_eq!(
        at(2.5),
        0.25 + 0.125 * 2.0,
        "overlap sums, track gain applies"
    );
    assert_eq!(at(3.5), 0.125 * 2.0);
    assert_eq!(at(4.5), 0.0);
}

#[test]
fn a_muted_track_silences_its_regions() {
    let sample = mono(100_000, 0.5);
    let mut p = audio_plan(constant(120.0), None, 8.0, vec![region(&sample, 0.0, 2.0)]);
    p.tracks[0].muted = true;
    let mut off = play(p);
    let out = render(&mut off, 3 * BEAT);
    assert!(out.iter().all(|&x| x == 0.0));
}

#[test]
fn muting_a_track_during_playback_ramps_without_a_step() {
    let sample = mono(300_000, 0.5);
    let mut off = play(audio_plan(
        constant(120.0),
        None,
        16.0,
        vec![region(&sample, 0.0, 8.0)],
    ));
    let mut out = render(&mut off, 20_000);
    off.send(EngineCommand::SetTrackMute {
        track: 0,
        muted: true,
    });
    out.extend(render(&mut off, 5_000));
    assert_eq!(out[19_999], 0.5);
    assert_eq!(out[24_999], 0.0);
    assert!(
        max_step(&out[10_000..]) < 0.01,
        "{}",
        max_step(&out[10_000..])
    );
}

// ---------------------------------------------------------------------
// Chase, loops and cuts
// ---------------------------------------------------------------------

#[test]
fn seeking_into_a_region_chases_to_the_right_source_frame() {
    let sample = ramp(400_000);
    let mut r = region(&sample, 1.0, 5.0);
    r.source_start_seconds = 0.5; // 24000 frames
    let mut off = play(audio_plan(constant(120.0), None, 16.0, vec![r]));
    let before = render(&mut off, 10_000);
    assert!(before.iter().all(|&x| x == 0.0), "the region has not begun");

    // Beat 3 is two beats into the region.
    off.send(EngineCommand::Seek { beat: 3.0 });
    let out = render(&mut off, 40_000);
    let source_at_seek = 24_000 + 2 * BEAT;
    // The entry fade covers the first few milliseconds.
    for e in 0..DECLICK {
        let want = sample.channels[0][source_at_seek + e] * (e + 1) as f32 / (DECLICK + 1) as f32;
        assert!(
            near(out[e], want),
            "entry fade at {e}: {} vs {want}",
            out[e]
        );
    }
    for e in DECLICK..40_000 {
        assert_eq!(out[e], sample.channels[0][source_at_seek + e], "frame {e}");
    }
}

#[test]
fn playing_from_the_middle_of_a_region_starts_at_its_source_position() {
    let sample = ramp(400_000);
    let mut r = region(&sample, 0.0, 4.0);
    r.reversed = true;
    let mut off = rig(audio_plan(constant(120.0), None, 16.0, vec![r]), None);
    off.send(EngineCommand::Seek { beat: 1.0 });
    off.send(EngineCommand::Play);
    let out = render(&mut off, 5_000);
    // Reversed: output frame k of the region reads source (len-1-k).
    let len = 4 * BEAT;
    let k0 = BEAT;
    for e in DECLICK..5_000 {
        assert_eq!(out[e], sample.channels[0][len - 1 - (k0 + e)], "frame {e}");
    }
}

#[test]
fn a_seek_through_one_continuous_source_never_clicks() {
    let sample = mono(1_000_000, 0.5);
    let mut off = play(audio_plan(
        constant(120.0),
        None,
        64.0,
        vec![region(&sample, 0.0, 30.0)],
    ));
    let mut out = render(&mut off, 40_000);
    for beat in [3.0, 7.5, 1.25] {
        off.send(EngineCommand::Seek { beat });
        out.extend(render(&mut off, 20_000));
    }
    // Tail and entry fade are complementary: a constant source stays
    // constant through every seek (the first pass has its own start fade).
    for (i, &x) in out.iter().enumerate().skip(2 * EDGE) {
        assert!(near(x, 0.5), "sample {i} is {x}");
    }
}

#[test]
fn loop_wrap_cuts_a_region_with_a_tail_and_restarts_it() {
    // Loop of 4 beats (96000 samples). The region crosses the loop end.
    let sample = mono(1_000_000, 0.5);
    let mut off = play(audio_plan(
        constant(120.0),
        Some((0.0, 4.0)),
        4.0,
        vec![region(&sample, 3.0, 6.0)],
    ));
    let loop_len = 4 * BEAT;
    let out = render(&mut off, 3 * loop_len);
    let on: Vec<usize> = (0..3).map(|p| p * loop_len + 3 * BEAT).collect();
    // The tail fades for DECLICK frames past the wrap; its last frame is
    // faintly audible.
    let tail_end: Vec<usize> = (1..=3).map(|p| p * loop_len + DECLICK).collect();
    assert_eq!(rising_edges(&out), on);
    // The third pass is still sounding when the render ends.
    assert_eq!(falling_edges(&out), &tail_end[..2]);
    assert!(
        max_step(&out) < 0.5 / EDGE as f32 + 1e-6,
        "no hard step at the wrap: {}",
        max_step(&out)
    );
    // Silent between the tail and the next pass.
    assert!(
        out[loop_len + DECLICK..loop_len + 3 * BEAT]
            .iter()
            .all(|&x| x == 0.0)
    );
}

#[test]
fn looping_into_a_region_that_spans_the_loop_restarts_its_source() {
    let sample = ramp(1_000_000);
    let mut off = play(audio_plan(
        constant(120.0),
        Some((0.0, 4.0)),
        4.0,
        vec![region(&sample, 0.0, 8.0)],
    ));
    let loop_len = 4 * BEAT;
    let out = render(&mut off, 2 * loop_len);
    for k in EDGE..loop_len - EDGE {
        assert_eq!(out[k], sample.channels[0][k]);
    }
    // After the wrap the source restarts from its first frame.
    for k in DECLICK..loop_len - EDGE {
        assert_eq!(out[loop_len + k], sample.channels[0][k], "frame {k}");
    }
}

#[test]
fn loops_keep_regions_on_the_unrounded_timeline() {
    // 133.3 BPM: a 4 beat loop is 86421.6 samples.
    let bpm = 133.3;
    let loop_len = 4.0 * 60.0 / bpm * SRF;
    let sample = mono(40_000, 0.5);
    let mut off = play(audio_plan(
        constant(bpm),
        Some((0.0, 4.0)),
        4.0,
        vec![region(&sample, 1.0, 1.5)],
    ));
    let passes = 60;
    let total = (loop_len * passes as f64) as usize;
    let out = render_blocks(&mut off, total, 4096).0;
    let beat = 60.0 / bpm * SRF;
    let edges = rising_edges(&out);
    assert!(edges.len() >= passes - 1);
    for (k, &e) in edges.iter().enumerate() {
        let ideal = k as f64 * loop_len + beat;
        assert!((e as f64 - ideal).abs() <= 0.51, "pass {k}: {e} vs {ideal}");
    }
}

#[test]
fn stop_fades_the_region_out_and_returns_the_sample() {
    let sample = mono(1_000_000, 0.5);
    let mut off = play(audio_plan(
        constant(120.0),
        None,
        64.0,
        vec![region(&sample, 0.0, 30.0)],
    ));
    let mut out = render(&mut off, 10_000);
    off.send(EngineCommand::Stop { return_to: 0.0 });
    out.extend(render(&mut off, 2_000));

    assert_eq!(out[9_999], 0.5);
    let tail = &out[10_000..10_000 + DECLICK];
    assert!(tail.windows(2).all(|w| w[1] < w[0]), "monotonic fade-out");
    assert!(tail[0] > 0.49 && tail[DECLICK - 1] < 0.01);
    assert!(out[10_000 + DECLICK..].iter().all(|&x| x == 0.0));
    assert!(max_step(&out[2 * EDGE..]) < 0.5 / DECLICK as f32 + 1e-6);

    // The tail's extra reference came back through the retirement queue.
    let retired = off.drain_retired();
    assert_eq!(
        retired
            .iter()
            .filter(|r| matches!(r, Retired::Sample(_)))
            .count(),
        1
    );
    drop(retired);
    assert_eq!(Arc::strong_count(&sample), 2, "test + plan only");
}

#[test]
fn pause_and_play_resume_at_the_same_source_frame() {
    let sample = ramp(400_000);
    let mut off = play(audio_plan(
        constant(120.0),
        None,
        64.0,
        vec![region(&sample, 0.0, 8.0)],
    ));
    let first = render(&mut off, 10_000);
    for k in EDGE..10_000 {
        assert_eq!(first[k], sample.channels[0][k]);
    }
    off.send(EngineCommand::Pause);
    let gap = render(&mut off, 3_000);
    assert!(gap[DECLICK..].iter().all(|&x| x == 0.0));
    off.send(EngineCommand::Play);
    let resumed = render(&mut off, 5_000);
    for e in DECLICK..5_000 {
        assert_eq!(resumed[e], sample.channels[0][10_000 + e], "frame {e}");
    }
}

#[test]
fn the_end_of_the_material_fades_a_region_that_runs_past_it() {
    let sample = mono(1_000_000, 0.5);
    let mut off = play(audio_plan(
        constant(120.0),
        None,
        2.0,
        vec![region(&sample, 0.0, 8.0)],
    ));
    let out = render(&mut off, 3 * BEAT);
    assert!(!off.is_playing());
    let end = 2 * BEAT;
    assert_eq!(out[end - 1], 0.5);
    assert!(out[end] < 0.5 && out[end] > 0.0, "fading");
    assert!(out[end + DECLICK..].iter().all(|&x| x == 0.0));
    assert!(max_step(&out[2 * EDGE..]) < 0.5 / DECLICK as f32 + 1e-6);
}

// ---------------------------------------------------------------------
// Plan swaps
// ---------------------------------------------------------------------

#[test]
fn a_swap_that_keeps_a_region_is_inaudible() {
    let sample = ramp(600_000);
    let make = |revision: u64| {
        let mut p = audio_plan(
            constant(120.0),
            None,
            64.0,
            vec![region(&sample, 0.5, 12.0)],
        );
        p.revision = revision;
        p
    };
    let reference = {
        let mut off = play(make(1));
        render(&mut off, 120_000)
    };
    let mut off = play(make(1));
    let mut out = render(&mut off, 30_000);
    off.send(EngineCommand::SetPlan(Box::new(make(2))));
    out.extend(render(&mut off, 30_000));
    off.send(EngineCommand::SetPlan(Box::new(make(3))));
    out.extend(render(&mut off, 60_000));
    assert_eq!(out.len(), reference.len());
    for (i, (a, b)) in out.iter().zip(&reference).enumerate() {
        assert!((a - b).abs() < 5e-6, "sample {i}: {a} vs {b}");
    }
}

#[test]
fn removing_a_region_by_plan_swap_fades_it_out() {
    let sample = mono(1_000_000, 0.5);
    let with = audio_plan(
        constant(120.0),
        None,
        64.0,
        vec![region(&sample, 0.0, 30.0)],
    );
    let mut without = with.clone();
    without.audio.clear();
    without.revision = 2;
    let mut off = play(with);
    let mut out = render(&mut off, 10_000);
    off.send(EngineCommand::SetPlan(Box::new(without)));
    out.extend(render(&mut off, 2_000));
    assert!(out[10_000 + DECLICK..].iter().all(|&x| x == 0.0));
    assert!(max_step(&out[2 * EDGE..]) < 0.5 / DECLICK as f32 + 1e-6);

    // The old plan (and its reference to the sample) was retired.
    let retired = off.drain_retired();
    assert!(retired.iter().any(|r| matches!(r, Retired::Plan(_))));
    drop(retired);
    assert_eq!(Arc::strong_count(&sample), 1, "plan and tail both let go");
}

#[test]
fn adding_a_region_under_the_playhead_fades_it_in() {
    let sample = mono(1_000_000, 0.5);
    let empty = audio_plan(constant(120.0), None, 64.0, vec![]);
    let mut with = empty.clone();
    with.audio = vec![region(&sample, 0.0, 30.0)];
    with.revision = 2;
    let mut off = play(empty);
    let mut out = render(&mut off, 10_000);
    off.send(EngineCommand::SetPlan(Box::new(with)));
    out.extend(render(&mut off, 2_000));
    assert!(out[..10_000].iter().all(|&x| x == 0.0));
    assert!(near(out[10_000], 0.5 / (DECLICK + 1) as f32));
    assert!(out[10_000 + DECLICK..].iter().all(|&x| x == 0.5));
}

// ---------------------------------------------------------------------
// Robustness
// ---------------------------------------------------------------------

#[test]
fn a_sample_at_another_rate_plays_at_native_speed() {
    // 24 kHz material: half a second is 12000 frames, 24000 engine frames.
    let data: Vec<f32> = (0..12_000).map(|i| i as f32 * 1e-4).collect();
    let slow = Arc::new(SampleBuffer::new("slow", 24_000.0, vec![data.clone()]));
    let mut off = play(audio_plan(
        constant(120.0),
        None,
        16.0,
        vec![region(&slow, 1.0, 4.0)],
    ));
    let out = render(&mut off, 4 * BEAT);
    // Output frame k reads source position k / 2, linearly interpolated.
    for k in [100usize, 1_001, 10_000, 20_000, 23_990] {
        let pos = k as f64 / 2.0;
        let i = pos.floor() as usize;
        let frac = (pos - i as f64) as f32;
        let want = data[i] + (data.get(i + 1).copied().unwrap_or(0.0) - data[i]) * frac;
        assert!(
            near(out[BEAT + k], want),
            "frame {k}: {} vs {want}",
            out[BEAT + k]
        );
    }
    assert_eq!(out[BEAT + 24_001], 0.0, "material ended");
}

#[test]
fn unusable_regions_are_ignored() {
    let sample = mono(100_000, 0.5);
    let empty = Arc::new(SampleBuffer::new("e", SR, vec![vec![]]));
    // Empty and backwards ranges and non-finite positions.
    let mut regions = vec![
        region(&sample, 2.0, 2.0),
        region(&sample, 3.0, 2.0),
        region(&sample, f64::NAN, 2.0),
        region(&sample, 0.0, f64::INFINITY),
    ];
    let mut bad_track = region(&sample, 0.0, 2.0);
    bad_track.track = 77;
    regions.push(bad_track); // a track the plan does not have
    regions.push(region(&empty, 0.0, 2.0)); // an empty sample
    let mut nan_offset = region(&sample, 0.0, 2.0);
    nan_offset.source_start_seconds = f64::NAN;
    regions.push(nan_offset);
    let mut nan_gain = region(&sample, 0.0, 2.0);
    nan_gain.gain = f32::NAN;
    regions.push(nan_gain);
    let mut off = play(audio_plan(constant(120.0), None, 8.0, regions));
    let out = render(&mut off, 3 * BEAT);
    assert!(out.iter().all(|&x| x == 0.0));
}

#[test]
fn non_finite_source_samples_are_contained() {
    let mut data = vec![0.5f32; 100_000];
    data[1000] = f32::NAN;
    data[2000] = f32::INFINITY;
    let sample = Arc::new(SampleBuffer::new("bad", SR, vec![data]));
    let mut off = play(audio_plan(
        constant(120.0),
        None,
        8.0,
        vec![region(&sample, 0.0, 2.0)],
    ));
    let out = render(&mut off, 3 * BEAT);
    assert!(out.iter().all(|x| x.is_finite()));
    assert_eq!(out[1000], 0.0);
    assert_eq!(out[3000], 0.5);
}

#[test]
fn cuts_beyond_the_tail_pool_end_abruptly_and_are_counted() {
    let sample = mono(1_000_000, 0.5);
    let regions: Vec<AudioRegion> = (0..300).map(|_| region(&sample, 0.0, 30.0)).collect();
    let mut off = play(audio_plan(constant(120.0), None, 64.0, regions));
    render(&mut off, 5_000);
    off.send(EngineCommand::Stop { return_to: 0.0 });
    let out = render(&mut off, 2_000);
    assert_eq!(
        off.telemetry().audio_tail_overflows.load(Ordering::Relaxed),
        300 - 256
    );
    assert!(out.iter().all(|x| x.is_finite()));
    assert_eq!(out[DECLICK + 10], 0.0, "every tail finished");
}

#[test]
fn the_script_sounds_the_same_for_every_block_size() {
    #[derive(Clone, Copy)]
    enum Step {
        Seek(f64),
        Pause,
        Play,
        Swap,
        Stop,
    }
    let sample = ramp(1_500_000);
    let mut second = region(&sample, 5.0, 9.0);
    second.reversed = true;
    second.fade_in_beats = 0.5;
    second.source_start_seconds = 1.0;
    let mut first = region(&sample, 1.0, 12.0);
    first.fade_out_beats = 1.0;
    first.gain = 0.7;
    let base = audio_plan(ramp_tempo(), Some((2.0, 10.0)), 12.0, vec![first, second]);
    // Commands land on multiples of every tested block size.
    let script = [
        (32_000usize, Step::Seek(3.0)),
        (96_000, Step::Swap),
        (128_000, Step::Pause),
        (160_000, Step::Play),
        (256_000, Step::Stop),
        (288_000, Step::Play),
    ];
    let total = 480_000;
    let run = |block: usize| {
        let mut off = play(base.clone());
        let mut l = vec![0.0; total];
        let mut r = vec![0.0; total];
        let mut done = 0;
        let mut next = 0;
        while done < total {
            while next < script.len() && script[next].0 == done {
                off.send(match script[next].1 {
                    Step::Seek(beat) => EngineCommand::Seek { beat },
                    Step::Pause => EngineCommand::Pause,
                    Step::Play => EngineCommand::Play,
                    Step::Stop => EngineCommand::Stop { return_to: 0.0 },
                    Step::Swap => {
                        let mut p = base.clone();
                        p.revision = 2;
                        EngineCommand::SetPlan(Box::new(p))
                    }
                });
                next += 1;
            }
            let until = script.get(next).map_or(total, |s| s.0).min(total);
            let n = block.min(until - done);
            off.render(&mut l[done..done + n], &mut r[done..done + n]);
            done += n;
        }
        l
    };
    let reference = run(256);
    assert!(reference.iter().any(|&x| x != 0.0));
    for block in [1usize, 64, 1000] {
        assert!(run(block) == reference, "differs for block size {block}");
    }
}
