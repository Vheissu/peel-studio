//! The fixtures in `tests/fixtures` hold 0.25 s of a 440 Hz (left) and 880 Hz
//! (right) sine at amplitude 0.5 and 44.1 kHz. `tone.flac`, `tone.mp3` and
//! `tone.aiff` were made with ffmpeg:
//!
//! ```text
//! ffmpeg -f lavfi -i "aevalsrc=0.5*sin(2*PI*440*t)|0.5*sin(2*PI*880*t):s=44100:d=0.25" \
//!        -c:a flac tone.flac        # also: -c:a libmp3lame -b:a 128k tone.mp3,
//!                                   #       -c:a pcm_s16be -f aiff tone.aiff
//! ```
//!
//! `tone.ogg` came from libsndfile (libvorbis): ffmpeg's built-in Vorbis
//! encoder writes streams that decoders disagree about.

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};

use peel_media::{
    MediaError, Peaks, SampleBuffer, WavFormat, decode_file, is_audio_file, load_for_engine, probe,
    resample, write_wav,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("peel-media-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn entries(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sine(freq: f64, amp: f64, rate: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (amp * (TAU * freq * n as f64 / rate).sin()) as f32)
        .collect()
}

/// Amplitude of the component at `freq`, measured with a Hann window over
/// `x[from..to]` (accurate to a fraction of a percent for steady tones).
fn amplitude_at(x: &[f32], rate: f64, freq: f64, from: usize, to: usize) -> f64 {
    let n = to - from;
    let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
    for (i, &s) in x[from..to].iter().enumerate() {
        let w = 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos();
        let phase = TAU * freq * (from + i) as f64 / rate;
        re += w * f64::from(s) * phase.cos();
        im -= w * f64::from(s) * phase.sin();
        wsum += w;
    }
    2.0 * (re * re + im * im).sqrt() / wsum
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

// ---------------------------------------------------------------------
// WAV writing and decoding
// ---------------------------------------------------------------------

#[test]
fn wav_round_trips_at_every_depth() {
    let dir = Scratch::new("roundtrip");
    let left = sine(440.0, 0.8, 44_100.0, 20_000);
    let right = sine(1_000.0, 0.5, 44_100.0, 20_000);

    for (name, format, tolerance) in [
        ("f32.wav", WavFormat::FLOAT32, 0.0),
        (
            "p24.wav",
            WavFormat {
                bits: 24,
                dither: false,
            },
            0.5 / 8_388_608.0 * 1.01,
        ),
        ("p24d.wav", WavFormat::PCM24, 2.0 / 8_388_608.0),
        (
            "p16.wav",
            WavFormat {
                bits: 16,
                dither: false,
            },
            0.5 / 32_768.0 * 1.01,
        ),
        ("p16d.wav", WavFormat::PCM16, 2.0 / 32_768.0),
    ] {
        let path = dir.path(name);
        write_wav(&path, &[&left, &right], 44_100, format).unwrap();
        let back = decode_file(&path).unwrap();
        assert_eq!(back.name, name);
        assert_eq!(back.sample_rate, 44_100.0);
        assert_eq!(back.channel_count(), 2);
        assert_eq!(back.frames(), 20_000, "{name}");
        let err =
            max_abs_diff(&back.channels[0], &left).max(max_abs_diff(&back.channels[1], &right));
        assert!(err <= tolerance as f32, "{name}: error {err}");
    }
    // Only the finished files are left behind: no temp files.
    assert_eq!(
        dir.entries(),
        ["f32.wav", "p16.wav", "p16d.wav", "p24.wav", "p24d.wav"]
    );
}

#[test]
fn wav_mono_and_multichannel() {
    let dir = Scratch::new("channels");
    let a = sine(220.0, 0.5, 48_000.0, 1000);
    let b = sine(330.0, 0.5, 48_000.0, 1000);
    let c = sine(440.0, 0.5, 48_000.0, 1000);
    write_wav(dir.path("mono.wav"), &[&a], 48_000, WavFormat::FLOAT32).unwrap();
    let mono = decode_file(dir.path("mono.wav")).unwrap();
    assert_eq!(mono.channel_count(), 1);
    assert_eq!(mono.channels[0], a);

    write_wav(
        dir.path("three.wav"),
        &[&a, &b, &c],
        48_000,
        WavFormat::FLOAT32,
    )
    .unwrap();
    let three = decode_file(dir.path("three.wav")).unwrap();
    assert_eq!(three.channel_count(), 3);
    assert_eq!(three.channels[2], c);
}

#[test]
fn dither_is_triangular_noise_of_one_lsb_and_off_means_silence() {
    let dir = Scratch::new("dither");
    let silence = vec![0.0f32; 200_000];
    write_wav(
        dir.path("plain.wav"),
        &[&silence],
        44_100,
        WavFormat {
            bits: 16,
            dither: false,
        },
    )
    .unwrap();
    assert!(
        decode_file(dir.path("plain.wav")).unwrap().channels[0]
            .iter()
            .all(|&x| x == 0.0)
    );

    write_wav(
        dir.path("noisy.wav"),
        &[&silence, &silence],
        44_100,
        WavFormat::PCM16,
    )
    .unwrap();
    let noisy = decode_file(dir.path("noisy.wav")).unwrap();
    let lsb = 1.0 / 32_768.0;
    let mut hist = [0usize; 3];
    for &x in &noisy.channels[0] {
        let q = (x / lsb).round() as i32;
        assert!((-1..=1).contains(&q), "dither of ±1 LSB, got {q}");
        hist[(q + 1) as usize] += 1;
    }
    let n = noisy.frames() as f64;
    // Triangular density: P(0) = 3/4 once rounded, P(±1) = 1/8 each.
    assert!((hist[1] as f64 / n - 0.75).abs() < 0.01, "{hist:?}");
    assert!((hist[0] as f64 / n - 0.125).abs() < 0.01, "{hist:?}");
    assert!((hist[2] as f64 / n - 0.125).abs() < 0.01, "{hist:?}");
    // The channels get independent noise.
    assert_ne!(noisy.channels[0], noisy.channels[1]);
    // Reproducible: same input, same file.
    write_wav(
        dir.path("noisy2.wav"),
        &[&silence, &silence],
        44_100,
        WavFormat::PCM16,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(dir.path("noisy.wav")).unwrap(),
        std::fs::read(dir.path("noisy2.wav")).unwrap()
    );
}

#[test]
fn quantizing_clips_and_cleans_bad_samples() {
    let dir = Scratch::new("clip");
    let wild = [2.0f32, -2.0, f32::NAN, f32::INFINITY, 1.0, -1.0, 0.0];
    write_wav(
        dir.path("clip.wav"),
        &[&wild],
        44_100,
        WavFormat {
            bits: 16,
            dither: false,
        },
    )
    .unwrap();
    let back = decode_file(dir.path("clip.wav")).unwrap();
    let ints: Vec<i32> = back.channels[0]
        .iter()
        .map(|x| (x * 32_768.0).round() as i32)
        .collect();
    assert_eq!(ints, [32_767, -32_768, 0, 0, 32_767, -32_768, 0]);

    write_wav(dir.path("float.wav"), &[&wild], 44_100, WavFormat::FLOAT32).unwrap();
    let back = decode_file(dir.path("float.wav")).unwrap();
    assert_eq!(back.channels[0][0], 2.0, "float output does not clip");
    assert_eq!(back.channels[0][2], 0.0);
}

#[test]
fn writes_are_atomic() {
    let dir = Scratch::new("atomic");
    let x = sine(440.0, 0.5, 44_100.0, 1000);
    let target = dir.path("out.wav");
    write_wav(&target, &[&x], 44_100, WavFormat::FLOAT32).unwrap();
    let original = std::fs::read(&target).unwrap();

    // Bad arguments fail before touching the existing file.
    let short = &x[..10];
    for err in [
        write_wav(&target, &[], 44_100, WavFormat::PCM16).unwrap_err(),
        write_wav(&target, &[&x, short], 44_100, WavFormat::PCM16).unwrap_err(),
        write_wav(&target, &[&x], 0, WavFormat::PCM16).unwrap_err(),
        write_wav(
            &target,
            &[&x],
            44_100,
            WavFormat {
                bits: 8,
                dither: false,
            },
        )
        .unwrap_err(),
    ] {
        assert!(matches!(err, MediaError::InvalidArgument(_)), "{err}");
    }
    assert_eq!(std::fs::read(&target).unwrap(), original);

    // An unwritable destination reports the path and leaves nothing behind.
    let bad = dir.path("missing-dir/out.wav");
    let err = write_wav(&bad, &[&x], 44_100, WavFormat::PCM16).unwrap_err();
    assert!(matches!(err, MediaError::Io { .. }), "{err}");
    assert!(err.to_string().contains("missing-dir"));

    // Overwriting replaces the old file completely.
    write_wav(&target, &[&x[..100]], 44_100, WavFormat::PCM16).unwrap();
    assert_eq!(decode_file(&target).unwrap().frames(), 100);
    assert_eq!(dir.entries(), ["out.wav"]);
}

// ---------------------------------------------------------------------
// Decoding every supported format
// ---------------------------------------------------------------------

fn check_tone(buf: &SampleBuffer, name: &str, amp_tolerance: f64) {
    assert_eq!(buf.name, name);
    assert_eq!(buf.sample_rate, 44_100.0, "{name}");
    assert_eq!(buf.channel_count(), 2, "{name}");
    let frames = buf.frames();
    assert!(
        (11_025_i64 - frames as i64).abs() <= 2_300,
        "{name}: {frames} frames"
    );
    let (from, to) = (1_500, frames - 1_500);
    let l440 = amplitude_at(&buf.channels[0], 44_100.0, 440.0, from, to);
    let r880 = amplitude_at(&buf.channels[1], 44_100.0, 880.0, from, to);
    assert!((l440 - 0.5).abs() < amp_tolerance, "{name}: left {l440}");
    assert!((r880 - 0.5).abs() < amp_tolerance, "{name}: right {r880}");
    // And not crossed over.
    assert!(
        amplitude_at(&buf.channels[0], 44_100.0, 880.0, from, to) < 0.02,
        "{name}"
    );
    assert!(
        amplitude_at(&buf.channels[1], 44_100.0, 440.0, from, to) < 0.02,
        "{name}"
    );
}

#[test]
fn decodes_flac_exactly() {
    let buf = decode_file(fixture("tone.flac")).unwrap();
    check_tone(&buf, "tone.flac", 0.005);
    assert_eq!(buf.frames(), 11_025);
    let expected = sine(440.0, 0.5, 44_100.0, 11_025);
    assert!(max_abs_diff(&buf.channels[0], &expected) < 5e-4);
}

#[test]
fn decodes_aiff_exactly() {
    let buf = decode_file(fixture("tone.aiff")).unwrap();
    check_tone(&buf, "tone.aiff", 0.005);
    assert_eq!(buf.frames(), 11_025);
    let expected = sine(880.0, 0.5, 44_100.0, 11_025);
    assert!(max_abs_diff(&buf.channels[1], &expected) < 5e-4);
}

#[test]
fn decodes_ogg_vorbis() {
    let buf = decode_file(fixture("tone.ogg")).unwrap();
    check_tone(&buf, "tone.ogg", 0.03);
}

#[test]
fn decodes_mp3() {
    let buf = decode_file(fixture("tone.mp3")).unwrap();
    check_tone(&buf, "tone.mp3", 0.04);
}

/// An ID3v2 tag with cover art can contain bytes that look like MP3 frame
/// headers; the decoder has to skip the tag by its declared size.
#[test]
fn mp3_with_a_large_id3_tag_decodes() {
    let original = std::fs::read(fixture("tone.mp3")).unwrap();
    assert_eq!(&original[..3], b"ID3");
    let tag_len = ((original[6] as usize) << 21)
        | ((original[7] as usize) << 14)
        | ((original[8] as usize) << 7)
        | original[9] as usize;
    let audio = &original[10 + tag_len..];

    // One APIC frame whose picture data is full of fake sync words.
    let picture: Vec<u8> = (0..200_000)
        .map(|i| if i % 2 == 0 { 0xFF } else { 0xFB })
        .collect();
    let mut frame_body = vec![0u8]; // text encoding
    frame_body.extend_from_slice(b"image/png\0");
    frame_body.push(3); // front cover
    frame_body.push(0); // empty description
    frame_body.extend_from_slice(&picture);
    let syncsafe = |n: usize| {
        [
            (n >> 21) as u8 & 0x7F,
            (n >> 14) as u8 & 0x7F,
            (n >> 7) as u8 & 0x7F,
            n as u8 & 0x7F,
        ]
    };
    let mut frame = b"APIC".to_vec();
    frame.extend_from_slice(&syncsafe(frame_body.len()));
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&frame_body);
    let mut file = b"ID3\x04\x00\x00".to_vec();
    file.extend_from_slice(&syncsafe(frame.len()));
    file.extend_from_slice(&frame);
    file.extend_from_slice(audio);

    let dir = Scratch::new("id3");
    std::fs::write(dir.path("art.mp3"), &file).unwrap();
    let buf = decode_file(dir.path("art.mp3")).unwrap();
    check_tone(&buf, "art.mp3", 0.04);
    assert_eq!(probe(dir.path("art.mp3")).unwrap().channels, 2);
}

#[test]
fn probe_reads_headers_without_decoding() {
    for (name, format) in [
        ("tone.flac", "flac"),
        ("tone.aiff", "aiff"),
        ("tone.ogg", "ogg"),
        ("tone.mp3", "mp3"),
    ] {
        let info = probe(fixture(name)).unwrap();
        assert_eq!(info.sample_rate, 44_100, "{name}");
        assert_eq!(info.channels, 2, "{name}");
        assert_eq!(info.format, format, "{name}");
        if let Some(secs) = info.duration_seconds {
            assert!((secs - 0.25).abs() < 0.05, "{name}: {secs}");
        }
    }
    let flac = probe(fixture("tone.flac")).unwrap();
    assert_eq!(flac.frames, Some(11_025));
    assert!((flac.duration_seconds.unwrap() - 0.25).abs() < 1e-6);

    let dir = Scratch::new("probe");
    let x = vec![0.0f32; 4800];
    write_wav(dir.path("a.wav"), &[&x], 48_000, WavFormat::PCM16).unwrap();
    let info = probe(dir.path("a.wav")).unwrap();
    assert_eq!(info.sample_rate, 48_000);
    assert_eq!(info.channels, 1);
    assert_eq!(info.frames, Some(4800));
    assert_eq!(info.duration_seconds, Some(0.1));
    assert_eq!(info.format, "wave");
}

#[test]
fn bad_files_report_useful_errors() {
    let dir = Scratch::new("bad");
    let err = decode_file(dir.path("nope.wav")).unwrap_err();
    assert!(matches!(err, MediaError::Io { .. }), "{err}");
    assert!(err.to_string().contains("nope.wav"));
    assert!(probe(dir.path("nope.wav")).is_err());

    std::fs::write(dir.path("text.wav"), b"this is not audio at all, just text").unwrap();
    let err = decode_file(dir.path("text.wav")).unwrap_err();
    assert!(
        matches!(
            err,
            MediaError::Unsupported { .. } | MediaError::Decode { .. }
        ),
        "{err}"
    );

    std::fs::write(dir.path("empty.flac"), b"").unwrap();
    assert!(decode_file(dir.path("empty.flac")).is_err());

    // A WAV cut off mid-data decodes as far as it goes.
    let x = sine(440.0, 0.5, 44_100.0, 10_000);
    write_wav(dir.path("whole.wav"), &[&x], 44_100, WavFormat::PCM16).unwrap();
    let bytes = std::fs::read(dir.path("whole.wav")).unwrap();
    std::fs::write(dir.path("cut.wav"), &bytes[..bytes.len() / 2]).unwrap();
    let cut = decode_file(dir.path("cut.wav")).unwrap();
    assert!(
        cut.frames() > 1000 && cut.frames() < 10_000,
        "{}",
        cut.frames()
    );
}

#[test]
fn is_audio_file_goes_by_extension() {
    for yes in [
        "a.wav",
        "a.WAV",
        "dir/a.flac",
        "a.Mp3",
        "a.aiff",
        "a.aif",
        "a.aifc",
        "a.ogg",
        "a.oga",
        "a.wave",
        "x.y.wav",
    ] {
        assert!(is_audio_file(yes), "{yes}");
    }
    for no in [
        "a",
        "a.txt",
        "a.wav.bak",
        "a.mid",
        "a.peelproj",
        ".wav",
        "wav",
        "dir/",
        "",
    ] {
        assert!(!is_audio_file(no), "{no}");
    }
}

// ---------------------------------------------------------------------
// Resampling
// ---------------------------------------------------------------------

fn tone_buffer(freq: f64, rate: f64, frames: usize) -> SampleBuffer {
    SampleBuffer::new("tone", rate as f32, vec![sine(freq, 0.8, rate, frames)])
}

#[test]
fn resampling_a_sine_keeps_its_frequency_and_level() {
    for (src, dst) in [
        (44_100.0, 48_000.0),
        (48_000.0, 44_100.0),
        (22_050.0, 44_100.0),
        (96_000.0, 48_000.0),
    ] {
        let input = tone_buffer(1_000.0, src, (src * 0.5) as usize);
        let out = resample(&input, dst as f32);
        assert_eq!(out.sample_rate, dst as f32);
        assert_eq!(
            out.frames(),
            (input.frames() as f64 * dst / src).round() as usize
        );
        let margin = 400;
        let (from, to) = (margin, out.frames() - margin);
        let level = amplitude_at(&out.channels[0], dst, 1_000.0, from, to);
        assert!(
            (level - 0.8).abs() < 0.8 * 0.001,
            "{src}->{dst}: level {level} (want 0.8)"
        );
        // Sample-by-sample agreement with the ideal sine at the new rate.
        let ideal = sine(1_000.0, 0.8, dst, out.frames());
        let err = max_abs_diff(&out.channels[0][from..to], &ideal[from..to]);
        assert!(err < 1e-3, "{src}->{dst}: max error {err}");
    }
}

#[test]
fn resampling_removes_what_the_new_rate_cannot_hold() {
    // 30 kHz at 96 kHz is above the 22.05 kHz Nyquist of 44.1 kHz: it must be
    // filtered out, not folded down to 14.1 kHz.
    let input = tone_buffer(30_000.0, 96_000.0, 48_000);
    let out = resample(&input, 44_100.0);
    let (from, to) = (500, out.frames() - 500);
    let folded = amplitude_at(&out.channels[0], 44_100.0, 14_100.0, from, to);
    let peak = out.channels[0][from..to]
        .iter()
        .fold(0.0f32, |p, x| p.max(x.abs()));
    assert!(
        f64::from(peak) < 0.8 * 1e-4,
        "stopband leak {peak} (> -80 dB)"
    );
    assert!(folded < 0.8 * 1e-4);

    // A tone comfortably inside the pass band survives the same conversion.
    let ok = resample(&tone_buffer(15_000.0, 96_000.0, 48_000), 44_100.0);
    let level = amplitude_at(&ok.channels[0], 44_100.0, 15_000.0, from, ok.frames() - 500);
    assert!((level - 0.8).abs() < 0.8 * 0.002, "15 kHz level {level}");
}

#[test]
fn upsampling_leaves_no_images() {
    // 5 kHz at 22.05 kHz doubled: the image at 22.05 - 5 = 17.05 kHz must go.
    let out = resample(&tone_buffer(5_000.0, 22_050.0, 11_025), 44_100.0);
    let (from, to) = (300, out.frames() - 300);
    let image = amplitude_at(&out.channels[0], 44_100.0, 17_050.0, from, to);
    assert!(image < 0.8 * 1e-4, "image {image}");
}

#[test]
fn resampling_preserves_metadata_and_handles_edge_cases() {
    let mut input = SampleBuffer::new(
        "kick.wav",
        44_100.0,
        vec![
            sine(100.0, 0.5, 44_100.0, 4410),
            sine(200.0, 0.5, 44_100.0, 4410),
        ],
    );
    input.root_key = 48;
    input.loop_range = Some((441, 4410));
    let out = resample(&input, 88_200.0);
    assert_eq!(out.name, "kick.wav");
    assert_eq!(out.root_key, 48);
    assert_eq!(out.channel_count(), 2);
    assert_eq!(out.frames(), 8820);
    assert_eq!(out.loop_range, Some((882, 8820)));

    // Same rate: untouched copy.
    let same = resample(&input, 44_100.0);
    assert_eq!(same.channels, input.channels);

    // Empty and one-sample buffers do not panic.
    let empty = SampleBuffer::new("e", 44_100.0, vec![vec![]]);
    assert_eq!(resample(&empty, 48_000.0).frames(), 0);
    let one = SampleBuffer::new("o", 44_100.0, vec![vec![1.0]]);
    assert!(resample(&one, 48_000.0).frames() >= 1);

    // DC passes at unit gain, including the very ends it rolls off from.
    let dc = SampleBuffer::new("dc", 44_100.0, vec![vec![0.5; 20_000]]);
    let out = resample(&dc, 48_000.0);
    for &x in &out.channels[0][500..out.frames() - 500] {
        assert!((x - 0.5).abs() < 1e-5);
    }
}

#[test]
fn load_for_engine_decodes_and_converts() {
    let dir = Scratch::new("engine");
    let x = sine(1_000.0, 0.5, 44_100.0, 44_100);
    write_wav(dir.path("a.wav"), &[&x], 44_100, WavFormat::FLOAT32).unwrap();

    let same = load_for_engine(dir.path("a.wav"), 44_100.0).unwrap();
    assert_eq!(same.sample_rate, 44_100.0);
    assert_eq!(same.channels[0], x);

    let converted = load_for_engine(dir.path("a.wav"), 48_000.0).unwrap();
    assert_eq!(converted.sample_rate, 48_000.0);
    assert_eq!(converted.frames(), 48_000);
    assert_eq!(converted.name, "a.wav");
    let level = amplitude_at(&converted.channels[0], 48_000.0, 1_000.0, 500, 47_500);
    assert!((level - 0.5).abs() < 0.001);
}

// ---------------------------------------------------------------------
// Peaks
// ---------------------------------------------------------------------

/// Deterministic pseudo-random audio-like signal.
fn noise(frames: usize, seed: u64) -> Vec<f32> {
    let mut s = seed | 1;
    (0..frames)
        .map(|n| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let r = (s >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
            r * (0.2 + 0.8 * (n as f32 * 0.001).sin().abs())
        })
        .collect()
}

fn brute(chs: &[Vec<f32>], a: usize, b: usize) -> (f32, f32) {
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for c in chs {
        for &x in &c[a..b] {
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    (lo, hi)
}

#[test]
fn peaks_pyramid_summarizes_the_whole_buffer() {
    let chs = vec![noise(100_000, 1), noise(100_000, 2)];
    let buf = SampleBuffer::new("n", 44_100.0, chs.clone());
    let peaks = Peaks::compute(&buf);
    assert_eq!(peaks.frames(), 100_000);
    assert_eq!(peaks.channels(), 2);
    let blocks: Vec<usize> = peaks.block_sizes().collect();
    assert_eq!(blocks[0], 64);
    assert!(blocks.windows(2).all(|w| w[1] == w[0] * 4));
    assert!(*blocks.last().unwrap() >= 65_536);

    let all: Vec<_> = peaks.query(0, 100_000, 1).collect();
    assert_eq!(all, vec![brute(&chs, 0, 100_000)]);

    let left: Vec<_> = peaks.query_channel(0, 0, 100_000, 1).collect();
    assert_eq!(left, vec![brute(&chs[..1], 0, 100_000)]);
}

#[test]
fn peaks_are_exact_when_columns_align_with_blocks() {
    let chs = vec![noise(64 * 500, 7)];
    let peaks = Peaks::compute(&SampleBuffer::new("n", 44_100.0, chs.clone()));
    let cols: Vec<_> = peaks.query(0, 64 * 500, 500).collect();
    assert_eq!(cols.len(), 500);
    for (i, &c) in cols.iter().enumerate() {
        assert_eq!(c, brute(&chs, i * 64, (i + 1) * 64), "column {i}");
    }
    // A coarser aligned zoom is exact too (4 blocks per column).
    let cols: Vec<_> = peaks.query(0, 64 * 500, 125).collect();
    for (i, &c) in cols.iter().enumerate() {
        assert_eq!(c, brute(&chs, i * 256, (i + 1) * 256), "column {i}");
    }
}

#[test]
fn peaks_never_miss_a_peak_at_any_zoom() {
    let chs = vec![noise(77_777, 3), noise(77_777, 4)];
    let peaks = Peaks::compute(&SampleBuffer::new("n", 44_100.0, chs.clone()));
    let mut seed = 99u64;
    let mut next = |m: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % m as u64) as usize
    };
    for _ in 0..300 {
        let start = next(77_000);
        let len = 1 + next(77_777 - start);
        let end = start + len;
        let columns = 1 + next(400);
        let got: Vec<_> = peaks.query(start, end, columns).collect();
        assert_eq!(got.len(), columns);
        let step = len as f64 / columns as f64;
        for (i, &(lo, hi)) in got.iter().enumerate() {
            let a = (start as f64 + i as f64 * step).floor() as usize;
            if a >= 77_777 {
                assert_eq!((lo, hi), (0.0, 0.0));
                continue;
            }
            let b = ((start as f64 + (i + 1) as f64 * step).ceil() as usize)
                .max(a + 1)
                .min(77_777);
            let (blo, bhi) = brute(&chs, a, b);
            assert!(
                lo <= blo && hi >= bhi,
                "range {start}..{end} x{columns} col {i}"
            );
            // Only the whole blocks touching the column can widen the answer,
            // and the block is never larger than the column (or 64 frames).
            let block = (step as usize).max(64);
            let wlo = a.saturating_sub(block);
            let whi = (b + block).min(77_777);
            let (wide_lo, wide_hi) = brute(&chs, wlo, whi);
            assert!(
                lo >= wide_lo && hi <= wide_hi,
                "range {start}..{end} x{columns} col {i}"
            );
        }
    }
}

#[test]
fn a_single_spike_shows_in_exactly_one_column() {
    let mut x = vec![0.0f32; 100_000];
    x[54_321] = 1.0;
    let peaks = Peaks::compute(&SampleBuffer::new("s", 44_100.0, vec![x]));
    let cols: Vec<_> = peaks.query(0, 100_000, 100).collect();
    for (i, &(lo, hi)) in cols.iter().enumerate() {
        assert_eq!(lo, 0.0);
        assert_eq!(hi, if i == 54 { 1.0 } else { 0.0 }, "column {i}");
    }
}

#[test]
fn peak_queries_handle_degenerate_input() {
    let peaks = Peaks::compute(&SampleBuffer::new("s", 44_100.0, vec![noise(1000, 5)]));
    assert_eq!(peaks.query(10, 10, 5).count(), 0);
    assert_eq!(peaks.query(20, 10, 5).count(), 0);
    assert_eq!(peaks.query(0, 100, 0).count(), 0);
    assert_eq!(peaks.query_channel(3, 0, 100, 4).count(), 0);
    // Past the end: silence columns, same count.
    let past: Vec<_> = peaks.query(5_000, 6_000, 10).collect();
    assert_eq!(past, vec![(0.0, 0.0); 10]);
    // Partly past the end.
    let part: Vec<_> = peaks.query(500, 1_500, 10).collect();
    assert_eq!(part.len(), 10);
    assert!(part[0] != (0.0, 0.0));
    assert_eq!(part[9], (0.0, 0.0));
    // The iterator knows its length.
    assert_eq!(peaks.query(0, 1000, 7).len(), 7);

    let empty = Peaks::compute(&SampleBuffer::new("e", 44_100.0, vec![vec![]]));
    assert_eq!(empty.level_count(), 0);
    assert_eq!(empty.query(0, 100, 10).count(), 0);
    let none = Peaks::compute(&SampleBuffer::default());
    assert_eq!(none.channels(), 0);
    assert_eq!(none.query(0, 100, 10).count(), 0);

    // A single frame.
    let one = Peaks::compute(&SampleBuffer::new("o", 44_100.0, vec![vec![0.25]]));
    assert_eq!(one.query(0, 1, 1).collect::<Vec<_>>(), vec![(0.25, 0.25)]);
}
