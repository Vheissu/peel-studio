use peel_media::{AudioEncoding, AudioFormat, MediaError, decode_file, write_audio};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("peel-encode-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tone(frequency: f32, rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| 0.5 * (std::f32::consts::TAU * frequency * i as f32 / rate as f32).sin())
        .collect()
}

#[test]
fn lossless_encoders_roundtrip_each_depth_and_rate() {
    let dir = Scratch::new("lossless");
    let cancel = AtomicBool::new(false);
    for format in [AudioFormat::Wav, AudioFormat::Aiff, AudioFormat::Flac] {
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            if format == AudioFormat::Flac && rate > 96_000 {
                continue;
            }
            let left = tone(440.0, rate, 9001);
            let right = tone(880.0, rate, 9001);
            for bits in [16, 24, 32] {
                if bits == 32 && format != AudioFormat::Wav {
                    continue;
                }
                let path = dir.path(&format!(
                    "{}-{rate}-{bits}.{}",
                    format.label(),
                    format.extension()
                ));
                write_audio(
                    &path,
                    &[&left, &right],
                    rate,
                    AudioEncoding {
                        format,
                        bits,
                        dither: false,
                        ..Default::default()
                    },
                    &cancel,
                )
                .unwrap();
                let back = decode_file(&path).unwrap();
                assert_eq!(back.frames(), left.len(), "{path:?}");
                assert_eq!(back.sample_rate, rate as f32);
                assert_eq!(back.channel_count(), 2);
                let tolerance = if bits == 32 {
                    0.0
                } else {
                    0.51 / (1u32 << (bits - 1)) as f32
                };
                for (actual, expected) in back.channels.iter().zip([&left, &right]) {
                    let error = actual
                        .iter()
                        .zip(expected)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0, f32::max);
                    assert!(error <= tolerance, "{path:?}: {error} > {tolerance}");
                }
            }
        }
    }
}

#[test]
fn mp3_has_requested_rate_bitrate_channels_and_gapless_duration() {
    let dir = Scratch::new("mp3");
    let cancel = AtomicBool::new(false);
    for rate in [44_100, 48_000] {
        let left = tone(440.0, rate, rate as usize);
        let right = tone(880.0, rate, rate as usize);
        for bitrate in [128, 192, 256, 320] {
            for channels in [1, 2] {
                let path = dir.path(&format!("{rate}-{bitrate}-{channels}.mp3"));
                let input = [&left[..], &right[..]];
                write_audio(
                    &path,
                    &input[..channels],
                    rate,
                    AudioEncoding {
                        format: AudioFormat::Mp3,
                        bitrate_kbps: bitrate,
                        ..Default::default()
                    },
                    &cancel,
                )
                .unwrap();
                let back = decode_file(&path).unwrap();
                assert_eq!(back.sample_rate, rate as f32);
                assert_eq!(back.channel_count(), channels);
                // Decoder delay and padding are carried in the LAME tag.
                assert_eq!(back.frames(), rate as usize, "{path:?}");
                let bytes = std::fs::read(&path).unwrap();
                // MPEG1 layer III bitrate index is in the first frame header.
                let wanted_index = match bitrate {
                    128 => 9,
                    192 => 11,
                    256 => 13,
                    _ => 14,
                };
                assert_eq!(bytes[2] >> 4, wanted_index, "{path:?}");
                for (ch, expected) in back.channels.iter().zip([&left, &right]) {
                    let signal = expected[2000..rate as usize - 2000]
                        .iter()
                        .map(|x| x * x)
                        .sum::<f32>();
                    let error = ch[2000..rate as usize - 2000]
                        .iter()
                        .zip(&expected[2000..rate as usize - 2000])
                        .map(|(a, b)| (a - b) * (a - b))
                        .sum::<f32>();
                    assert!(
                        error / signal < 0.02,
                        "{path:?}: relative squared error {}",
                        error / signal
                    );
                }
            }
        }
    }
}

#[test]
fn cancellation_and_invalid_options_preserve_existing_files() {
    let dir = Scratch::new("atomic");
    for format in [
        AudioFormat::Wav,
        AudioFormat::Aiff,
        AudioFormat::Flac,
        AudioFormat::Mp3,
    ] {
        let path = dir.path(&format!("mix.{}", format.extension()));
        std::fs::write(&path, b"existing export").unwrap();
        let cancel = AtomicBool::new(true);
        let encoding = AudioEncoding {
            format,
            ..Default::default()
        };
        let err = write_audio(&path, &[&[0.0; 100]], 48_000, encoding, &cancel).unwrap_err();
        assert!(matches!(err, MediaError::Cancelled), "{err}");
        cancel.store(false, Ordering::Relaxed);
        assert!(write_audio(&path, &[&[0.0; 100], &[0.0; 99]], 48_000, encoding, &cancel).is_err());
        assert!(write_audio(&path, &[&[0.0; 100]], 0, encoding, &cancel).is_err());
        let invalid = if format == AudioFormat::Mp3 {
            AudioEncoding {
                bitrate_kbps: 1,
                ..encoding
            }
        } else {
            AudioEncoding {
                bits: 8,
                ..encoding
            }
        };
        assert!(write_audio(&path, &[&[0.0; 100]], 48_000, invalid, &cancel).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing export");
        write_audio(&path, &[&[0.25; 100]], 48_000, encoding, &cancel).unwrap();
        assert_ne!(std::fs::read(&path).unwrap(), b"existing export");
    }
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 4);
}

#[test]
fn failed_publication_cleans_temporary_files() {
    let dir = Scratch::new("failed");
    let destination = dir.path("is-a-directory.flac");
    std::fs::create_dir(&destination).unwrap();
    write_audio(
        &destination,
        &[&[0.0; 9000]],
        48_000,
        AudioEncoding {
            format: AudioFormat::Flac,
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    assert!(destination.is_dir());
}

#[test]
fn cancelling_during_encoding_removes_partial_output() {
    let dir = Scratch::new("cancel-running");
    let path = dir.path("mix.flac");
    std::fs::write(&path, b"existing mix").unwrap();
    let cancel = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let task = scope.spawn(|| {
            let input = tone(440.0, 48_000, 2_000_000);
            write_audio(
                &path,
                &[&input],
                48_000,
                AudioEncoding {
                    format: AudioFormat::Flac,
                    ..Default::default()
                },
                &cancel,
            )
        });
        let started = std::time::Instant::now();
        loop {
            if std::fs::read_dir(&dir.0).unwrap().count() > 1 {
                break;
            }
            assert!(
                !task.is_finished(),
                "encoding completed before cancellation"
            );
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            std::thread::yield_now();
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(task.join().unwrap(), Err(MediaError::Cancelled)));
    });
    assert_eq!(std::fs::read(&path).unwrap(), b"existing mix");
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}

#[test]
fn mono_24_bit_aiff_pads_an_odd_data_chunk() {
    let dir = Scratch::new("aiff-pad");
    let path = dir.path("one.aiff");
    write_audio(
        &path,
        &[&[0.5]],
        44_100,
        AudioEncoding {
            format: AudioFormat::Aiff,
            dither: false,
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 58);
    assert_eq!(
        u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize + 8,
        bytes.len()
    );
    assert_eq!(decode_file(path).unwrap().channels[0], [0.5]);
}

#[test]
fn formats_handle_nonfinite_and_clipped_samples() {
    let dir = Scratch::new("sanitize");
    let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 2.0, -2.0, 0.5];
    for format in [AudioFormat::Wav, AudioFormat::Aiff, AudioFormat::Flac] {
        let path = dir.path(&format!("clean.{}", format.extension()));
        write_audio(
            &path,
            &[&input],
            48_000,
            AudioEncoding {
                format,
                bits: 16,
                dither: false,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let back = decode_file(path).unwrap();
        assert_eq!(&back.channels[0][..3], &[0.0; 3]);
        assert_eq!(&back.channels[0][3..], &[32767.0 / 32768.0, -1.0, 0.5]);
    }
}

#[test]
fn extensions_and_codec_constraints_are_explicit() {
    assert_eq!(
        AudioFormat::from_extension(Path::new("mix.MP3")).unwrap(),
        AudioFormat::Mp3
    );
    assert_eq!(
        AudioFormat::from_extension(Path::new("mix.aif")).unwrap(),
        AudioFormat::Aiff
    );
    assert!(AudioFormat::from_extension(Path::new("mix.ogg")).is_err());
    assert!(
        AudioEncoding {
            format: AudioFormat::Mp3,
            ..Default::default()
        }
        .validate(96_000)
        .is_err()
    );
    assert!(
        AudioEncoding {
            format: AudioFormat::Flac,
            bits: 32,
            ..Default::default()
        }
        .validate(48_000)
        .is_err()
    );
    assert!(
        AudioEncoding {
            format: AudioFormat::Aiff,
            bits: 32,
            ..Default::default()
        }
        .validate(48_000)
        .is_err()
    );
}

#[test]
fn long_flac_transients_roundtrip_without_expanding_predictors() {
    let dir = Scratch::new("transient-flac");
    let mut rng = 1u64;
    let mut left = tone(440.0, 44_100, 717_560);
    for (i, x) in left.iter_mut().enumerate() {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let noise = (rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
        let decay = (-((i % 5513) as f32) / 1200.0).exp();
        *x = (*x * 0.4 + noise * 0.6) * decay;
        if i / 4096 % 17 == 0 {
            *x = 0.0;
        }
    }
    let right = tone(880.0, 44_100, left.len());
    let input = [&left[..], &right[..]];
    for channels in [1, 2] {
        let path = dir.path("transients.flac");
        write_audio(
            &path,
            &input[..channels],
            44_100,
            AudioEncoding {
                format: AudioFormat::Flac,
                dither: false,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let back = decode_file(&path).unwrap();
        // This source previously produced 33 MB frames with wrapped
        // STREAMINFO sizes and lost audio in strict decoders. Compression
        // may fall back to verbatim but must stay near the raw PCM size.
        let raw_bytes = left.len() * channels * 3;
        let overhead = left.len().div_ceil(4096) * 32 + 42;
        assert!(std::fs::metadata(&path).unwrap().len() as usize <= raw_bytes + overhead);
        assert_eq!(back.frames(), left.len());
        assert_eq!(back.channel_count(), channels);
        for (actual, expected) in back.channels.iter().zip(&input[..channels]) {
            let error = actual
                .iter()
                .zip(*expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(error < 1.0 / 8_388_608.0, "{error}");
        }
    }
}
