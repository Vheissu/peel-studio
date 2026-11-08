use std::fs::File;
use std::path::Path;

use peel_dsp::SampleBuffer;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, Track, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::error::MediaError;
use crate::resample::resample;

/// Largest pre-allocation made from a header's frame count. The header is
/// untrusted; real data grows the buffers beyond this as needed.
const MAX_RESERVE_FRAMES: u64 = 1 << 24;

/// File extensions [`decode_file`] can read.
const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "wave", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga",
];

/// True if the path has an extension of a format [`decode_file`] reads. This
/// looks at the name only; use [`probe`] to inspect the contents.
pub fn is_audio_file(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// What a file contains, read from its headers without decoding the audio.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: usize,
    /// Frames per channel, when the container states it. Some MP3 files do
    /// not.
    pub frames: Option<u64>,
    /// Length in seconds, when known.
    pub duration_seconds: Option<f64>,
    /// Container name as the demuxer reports it: "wave", "aiff", "flac",
    /// "mp3" or "ogg".
    pub format: &'static str,
}

fn open(path: &Path) -> Result<(Box<dyn FormatReader + 'static>, String), MediaError> {
    let file = File::open(path).map_err(|e| MediaError::io(path, e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(&ext.to_ascii_lowercase());
    }
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| map_error(path, e))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((format, name))
}

fn map_error(path: &Path, e: SymphoniaError) -> MediaError {
    match e {
        SymphoniaError::IoError(io) => MediaError::io(path, io),
        SymphoniaError::Unsupported(reason) => MediaError::Unsupported {
            path: path.to_path_buf(),
            reason: reason.to_string(),
        },
        other => MediaError::Decode {
            path: path.to_path_buf(),
            reason: other.to_string(),
        },
    }
}

fn audio_track(format: &dyn FormatReader) -> Option<&Track> {
    format.default_track(TrackType::Audio)
}

/// Reads an audio file's headers.
pub fn probe(path: impl AsRef<Path>) -> Result<AudioInfo, MediaError> {
    let path = path.as_ref();
    let (format, _) = open(path)?;
    let track = audio_track(&*format).ok_or_else(|| MediaError::NoAudio {
        path: path.to_path_buf(),
    })?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| MediaError::NoAudio {
            path: path.to_path_buf(),
        })?;
    let sample_rate = params.sample_rate.ok_or_else(|| MediaError::Unsupported {
        path: path.to_path_buf(),
        reason: "sample rate not stated".to_string(),
    })?;
    let channels = params.channels.as_ref().map_or(0, |c| c.count());
    let duration_seconds = match (track.time_base, track.duration) {
        (Some(tb), Some(d)) => tb.calc_duration(d).map(|t| t.as_secs_f64()),
        _ => None,
    }
    .or_else(|| {
        track
            .num_frames
            .map(|f| f as f64 / f64::from(sample_rate.max(1)))
    });
    Ok(AudioInfo {
        sample_rate,
        channels,
        frames: track.num_frames,
        duration_seconds,
        format: format.format_info().short_name,
    })
}

/// Decodes a WAV, AIFF, FLAC, MP3 or Ogg Vorbis file into planar 32-bit
/// float at the file's own sample rate. The buffer's name is the file name.
///
/// Damaged packets are skipped; a file that yields no audio at all is an
/// error.
pub fn decode_file(path: impl AsRef<Path>) -> Result<SampleBuffer, MediaError> {
    let path = path.as_ref();
    let (mut format, name) = open(path)?;
    let track = audio_track(&*format).ok_or_else(|| MediaError::NoAudio {
        path: path.to_path_buf(),
    })?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| MediaError::NoAudio {
            path: path.to_path_buf(),
        })?
        .clone();
    let track_id = track.id;
    let expected_frames = track.num_frames;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| map_error(path, e))?;

    let mut rate = params.sample_rate;
    let mut planes: Vec<Vec<f32>> = Vec::new();
    let mut scratch: Vec<Vec<f32>> = Vec::new();
    let mut skipped = 0usize;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            // A chained stream announced a new track list. Everything up to
            // here is the first stream; that is what a sample is.
            Err(SymphoniaError::ResetRequired) => break,
            // Truncated files end mid-packet.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(map_error(path, e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = buf.spec();
                rate = Some(spec.rate());
                let n = buf.num_planes();
                if planes.is_empty() {
                    let cap = expected_frames.map_or(0, |f| f.min(MAX_RESERVE_FRAMES)) as usize;
                    planes = (0..n).map(|_| Vec::with_capacity(cap)).collect();
                } else if planes.len() != n {
                    return Err(MediaError::Decode {
                        path: path.to_path_buf(),
                        reason: "channel count changed mid-stream".to_string(),
                    });
                }
                buf.copy_to_vecs_planar::<f32>(&mut scratch);
                for (dst, src) in planes.iter_mut().zip(&scratch) {
                    dst.extend_from_slice(src);
                }
            }
            // The packet is damaged or unreadable; the next one may be fine.
            Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => {
                skipped += 1;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(map_error(path, e)),
        }
    }

    let frames = planes.first().map_or(0, Vec::len);
    if planes.is_empty() || frames == 0 {
        return Err(MediaError::Decode {
            path: path.to_path_buf(),
            reason: if skipped > 0 {
                format!("all {skipped} packets were unreadable")
            } else {
                "the file contains no audio".to_string()
            },
        });
    }
    let rate = rate
        .filter(|&r| r > 0)
        .ok_or_else(|| MediaError::Unsupported {
            path: path.to_path_buf(),
            reason: "sample rate not stated".to_string(),
        })?;
    Ok(SampleBuffer::new(name, rate as f32, planes))
}

/// Decodes a file and converts it to `engine_rate` if it differs.
pub fn load_for_engine(
    path: impl AsRef<Path>,
    engine_rate: f32,
) -> Result<SampleBuffer, MediaError> {
    let decoded = decode_file(path)?;
    if (decoded.sample_rate - engine_rate).abs() < 0.5 {
        Ok(decoded)
    } else {
        Ok(resample(&decoded, engine_rate))
    }
}
