//! Property-style tests for `TempoMap` over random segment layouts.
//!
//! Layouts come from a small deterministic generator (splitmix64), so every
//! failure names the seed that reproduces it. No external test dependencies.

use peel_time::{TempoMap, TempoSegment, Time};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A random layout: 1 to 8 segments, strictly increasing starts on a musical
/// grid (first at 0), tempi 20..=300, about half of the non-final segments
/// ramping (the final segment sometimes carries an ignored ramp target).
fn layout(rng: &mut Rng) -> Vec<TempoSegment> {
    let count = 1 + rng.below(8) as usize;
    let mut start = Time::ZERO;
    let mut segments = Vec::with_capacity(count);
    for i in 0..count {
        if i > 0 {
            // Gaps of 1/3 to 40 beats, on a grid of thirds or 960ths.
            let den = if rng.below(2) == 0 { 3 } else { 960 };
            let num = 1 + rng.below(40 * den as u64) as i64;
            start += Time::new(num, den);
        }
        let bpm = rng.range(20.0, 300.0);
        let ramp_to = (rng.below(2) == 0).then(|| rng.range(20.0, 300.0));
        segments.push(TempoSegment {
            start,
            bpm,
            ramp_to,
        });
    }
    segments
}

fn span(segments: &[TempoSegment]) -> f64 {
    segments.last().unwrap().start.as_f64() + 64.0
}

fn describe(seed: u64, segments: &[TempoSegment]) -> String {
    format!("seed {seed}: {segments:?}")
}

const CASES: u64 = 600;

#[test]
fn beat_seconds_beat_round_trip() {
    for seed in 0..CASES {
        let mut rng = Rng::new(seed);
        let segs = layout(&mut rng);
        let map = TempoMap::from_segments(&segs);
        let top = span(&segs);
        for _ in 0..60 {
            let beat = rng.range(0.0, top);
            let sec = map.beat_to_seconds(beat);
            let back = map.seconds_to_beat(sec);
            assert!(
                (back - beat).abs() <= 1e-8 * (1.0 + beat),
                "beat {beat} -> {sec}s -> {back}; {}",
                describe(seed, &segs)
            );
        }
        // Segment boundaries exactly.
        for s in &segs {
            let beat = s.start.as_f64();
            let back = map.seconds_to_beat(map.beat_to_seconds(beat));
            assert!(
                (back - beat).abs() <= 1e-8 * (1.0 + beat),
                "boundary {beat} -> {back}; {}",
                describe(seed, &segs)
            );
        }
    }
}

#[test]
fn seconds_beat_seconds_round_trip() {
    for seed in 0..CASES {
        let mut rng = Rng::new(seed ^ 0xABCD);
        let segs = layout(&mut rng);
        let map = TempoMap::from_segments(&segs);
        let top = map.beat_to_seconds(span(&segs));
        for _ in 0..60 {
            let sec = rng.range(0.0, top);
            let beat = map.seconds_to_beat(sec);
            let back = map.beat_to_seconds(beat);
            assert!(
                (back - sec).abs() <= 1e-8 * (1.0 + sec),
                "{sec}s -> beat {beat} -> {back}s; {}",
                describe(seed, &segs)
            );
        }
    }
}

#[test]
fn time_is_monotonic_in_beats() {
    for seed in 0..CASES {
        let mut rng = Rng::new(seed ^ 0x1234);
        let segs = layout(&mut rng);
        let map = TempoMap::from_segments(&segs);
        let top = span(&segs);
        let mut beats: Vec<f64> = (0..80).map(|_| rng.range(0.0, top)).collect();
        beats.sort_by(f64::total_cmp);
        let mut prev_beat = f64::NEG_INFINITY;
        let mut prev_sec = f64::NEG_INFINITY;
        for beat in beats {
            let sec = map.beat_to_seconds(beat);
            assert!(
                sec >= prev_sec,
                "time went backwards: beat {prev_beat} -> {prev_sec}s, beat {beat} -> {sec}s; {}",
                describe(seed, &segs)
            );
            // Beats that are clearly apart are clearly apart in time too
            // (tempo is capped at 999 BPM, so 1e-6 beat is at least 6e-8 s).
            if beat - prev_beat > 1e-6 {
                assert!(
                    sec > prev_sec,
                    "not strictly increasing; {}",
                    describe(seed, &segs)
                );
            }
            prev_beat = beat;
            prev_sec = sec;
        }
        // And the inverse is monotonic.
        let mut secs: Vec<f64> = (0..80)
            .map(|_| rng.range(0.0, map.beat_to_seconds(top)))
            .collect();
        secs.sort_by(f64::total_cmp);
        let mut prev = f64::NEG_INFINITY;
        for sec in secs {
            let beat = map.seconds_to_beat(sec);
            assert!(
                beat >= prev,
                "beat went backwards; {}",
                describe(seed, &segs)
            );
            prev = beat;
        }
    }
}

#[test]
fn constant_segments_match_the_closed_form() {
    for seed in 0..CASES {
        let mut rng = Rng::new(seed ^ 0x5555);
        let mut segs = layout(&mut rng);
        for s in &mut segs {
            s.ramp_to = None;
        }
        let map = TempoMap::from_segments(&segs);
        let top = span(&segs);
        for _ in 0..40 {
            let beat = rng.range(0.0, top);
            // Sum of whole segments before the beat, plus the part inside.
            let mut expected = 0.0;
            for (i, s) in segs.iter().enumerate() {
                let start = s.start.as_f64();
                let end = segs.get(i + 1).map_or(f64::INFINITY, |n| n.start.as_f64());
                if beat <= start {
                    break;
                }
                expected += (beat.min(end) - start) * 60.0 / s.bpm;
            }
            let got = map.beat_to_seconds(beat);
            assert!(
                (got - expected).abs() <= 1e-9 * (1.0 + expected),
                "beat {beat}: {got} vs {expected}; {}",
                describe(seed, &segs)
            );
        }
    }
}

/// Integrates 60/bpm(b) numerically (Simpson) and compares with the map,
/// which uses the closed form. This cross-checks ramps and the continuity
/// of accumulated time across boundaries.
#[test]
fn ramps_agree_with_numerical_integration() {
    for seed in 0..120 {
        let mut rng = Rng::new(seed ^ 0x7777);
        let segs = layout(&mut rng);
        let map = TempoMap::from_segments(&segs);
        let top = span(&segs).min(200.0);
        let beat = rng.range(0.0, top);
        let steps = 20_000usize; // even
        let h = beat / steps as f64;
        let f = |b: f64| 60.0 / map.bpm_at(b);
        let mut sum = f(0.0) + f(beat);
        for i in 1..steps {
            sum += f(h * i as f64) * if i % 2 == 1 { 4.0 } else { 2.0 };
        }
        let integral = sum * h / 3.0;
        let got = map.beat_to_seconds(beat);
        // Simpson is thrown off slightly where the tempo jumps between
        // segments; bound the error by the jump count times one step.
        let tolerance = 1e-3 * (1.0 + got) + segs.len() as f64 * h * 60.0 / 20.0;
        assert!(
            (got - integral).abs() <= tolerance,
            "beat {beat}: map {got}s, integral {integral}s; {}",
            describe(seed, &segs)
        );
    }
}

#[test]
fn tempo_is_continuous_across_ramps_and_bounded() {
    for seed in 0..CASES {
        let mut rng = Rng::new(seed ^ 0x9999);
        let segs = layout(&mut rng);
        let map = TempoMap::from_segments(&segs);
        let all: Vec<f64> = segs
            .iter()
            .flat_map(|s| std::iter::once(s.bpm).chain(s.ramp_to))
            .collect();
        let (lo, hi) = (
            all.iter().cloned().fold(f64::INFINITY, f64::min),
            all.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        );
        for _ in 0..60 {
            let b = rng.range(0.0, span(&segs));
            let bpm = map.bpm_at(b);
            assert!(
                bpm >= lo - 1e-9 && bpm <= hi + 1e-9,
                "bpm {bpm} outside [{lo}, {hi}] at beat {b}; {}",
                describe(seed, &segs)
            );
        }
        // Accumulated time has no jump at a boundary.
        for s in segs.iter().skip(1) {
            let b = s.start.as_f64();
            let before = map.beat_to_seconds(b - 1e-7);
            let at = map.beat_to_seconds(b);
            // At most 1e-7 beats at the slowest permitted tempo (20 BPM).
            assert!(
                (at - before).abs() <= 1e-7 * 3.0 + 1e-9,
                "time jumps {} s at beat {b}; {}",
                at - before,
                describe(seed, &segs)
            );
        }
    }
}

#[test]
fn segment_order_does_not_matter() {
    for seed in 0..CASES {
        let mut rng = Rng::new(seed ^ 0xCAFE);
        let segs = layout(&mut rng);
        let mut shuffled = segs.clone();
        for i in (1..shuffled.len()).rev() {
            shuffled.swap(i, rng.below(i as u64 + 1) as usize);
        }
        assert_eq!(
            TempoMap::from_segments(&segs),
            TempoMap::from_segments(&shuffled),
            "{}",
            describe(seed, &segs)
        );
    }
}

#[test]
fn samples_round_trip_within_half_a_sample() {
    for seed in 0..CASES {
        let mut rng = Rng::new(seed ^ 0xF00D);
        let segs = layout(&mut rng);
        let map = TempoMap::from_segments(&segs);
        let rate = [44_100.0, 48_000.0, 96_000.0][rng.below(3) as usize];
        let max_bpm = segs
            .iter()
            .flat_map(|s| std::iter::once(s.bpm).chain(s.ramp_to))
            .fold(0.0f64, f64::max);
        // Half a sample, in beats, at the fastest tempo, plus float noise.
        let half_sample_beats = 0.5 * max_bpm / 60.0 / rate + 1e-9;
        for _ in 0..60 {
            let beat = rng.range(0.0, span(&segs));
            let sample = map.beat_to_sample(beat, rate);
            let back = map.sample_to_beat(sample, rate);
            assert!(
                (back - beat).abs() <= half_sample_beats,
                "beat {beat} -> sample {sample} -> {back}; {}",
                describe(seed, &segs)
            );
        }
        // Sample positions never run backwards.
        let mut beats: Vec<f64> = (0..40).map(|_| rng.range(0.0, span(&segs))).collect();
        beats.sort_by(f64::total_cmp);
        let samples: Vec<i64> = beats.iter().map(|b| map.beat_to_sample(*b, rate)).collect();
        assert!(
            samples.windows(2).all(|w| w[0] <= w[1]),
            "{}",
            describe(seed, &segs)
        );
    }
}

#[test]
fn looping_a_pattern_does_not_accumulate_rounding_error() {
    // The start of loop n is converted from its absolute position, so the
    // error stays within half a sample however many times a pattern repeats;
    // summing per-pass sample counts would grow without bound.
    let rate = 48_000.0;
    for bpm in [133.7, 120.0, 97.31, 174.0] {
        let map = TempoMap::constant(bpm);
        let loop_beats = Time::beats(4);
        let ideal_per_loop = 4.0 * 60.0 / bpm * rate;
        let mut worst = 0.0f64;
        for n in 0..=20_000i64 {
            let start = loop_beats.mul_int(n).as_f64();
            let sample = map.beat_to_sample(start, rate) as f64;
            worst = worst.max((sample - ideal_per_loop * n as f64).abs());
        }
        assert!(
            worst <= 0.5 + 1e-3,
            "bpm {bpm}: worst error {worst} samples"
        );
    }
}
