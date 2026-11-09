//! Native file encoders. Encoding happens on the export worker, in blocks,
//! and publishes the destination only after the entire file is durable.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{MediaError, wav::Rng};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioFormat {
    #[default]
    Wav,
    Mp3,
    Flac,
    Aiff,
}

impl AudioFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Aiff => "aiff",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Mp3 => "MP3",
            Self::Flac => "FLAC",
            Self::Aiff => "AIFF",
        }
    }

    /// Common rates offered by the export dialog. WAV and AIFF also accept
    /// other rates up to 384 kHz; the FLAC encoder accepts rates up to 96 kHz.
    pub fn supported_sample_rates(self) -> &'static [u32] {
        match self {
            Self::Mp3 => &[44_100, 48_000],
            Self::Flac => &[44_100, 48_000, 88_200, 96_000],
            _ => &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000],
        }
    }

    pub fn from_extension(path: &Path) -> Result<Self, MediaError> {
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match extension.to_ascii_lowercase().as_str() {
            "wav" | "wave" => Ok(Self::Wav),
            "mp3" => Ok(Self::Mp3),
            "flac" => Ok(Self::Flac),
            "aif" | "aiff" => Ok(Self::Aiff),
            _ => Err(MediaError::InvalidArgument(format!(
                "unsupported export extension '{extension}' (use wav, mp3, flac or aiff)"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AudioEncoding {
    pub format: AudioFormat,
    /// WAV: 16/24-bit PCM or 32-bit float; FLAC/AIFF: 16/24-bit PCM.
    /// Ignored for MP3.
    pub bits: u16,
    /// MP3 constant bitrate. Ignored for lossless formats.
    pub bitrate_kbps: u16,
    /// TPDF noise before PCM quantization. Ignored for MP3 and float WAV.
    pub dither: bool,
}

impl Default for AudioEncoding {
    fn default() -> Self {
        Self {
            format: AudioFormat::Wav,
            bits: 24,
            bitrate_kbps: 320,
            dither: true,
        }
    }
}

impl AudioEncoding {
    pub fn validate(self, sample_rate: u32) -> Result<(), MediaError> {
        if !(1..=384_000).contains(&sample_rate) {
            return Err(MediaError::InvalidArgument(
                "sample rate must be between 1 and 384000 Hz".into(),
            ));
        }
        if self.format == AudioFormat::Flac && sample_rate > 96_000 {
            return Err(MediaError::InvalidArgument(
                "FLAC sample rate must be 96000 Hz or lower".into(),
            ));
        }
        let valid_bits = match self.format {
            AudioFormat::Wav => matches!(self.bits, 16 | 24 | 32),
            AudioFormat::Aiff | AudioFormat::Flac => matches!(self.bits, 16 | 24),
            AudioFormat::Mp3 => {
                if !self.format.supported_sample_rates().contains(&sample_rate) {
                    return Err(MediaError::InvalidArgument(
                        "MP3 sample rate must be 44100 or 48000 Hz".into(),
                    ));
                }
                if !matches!(self.bitrate_kbps, 128 | 192 | 256 | 320) {
                    return Err(MediaError::InvalidArgument(
                        "MP3 bitrate must be 128, 192, 256 or 320 kbps".into(),
                    ));
                }
                true
            }
        };
        if !valid_bits {
            return Err(MediaError::InvalidArgument(format!(
                "unsupported {} bit depth {}",
                self.format.label(),
                self.bits
            )));
        }
        Ok(())
    }
}

const BLOCK: usize = 4096;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn check_cancel(cancel: &AtomicBool) -> Result<(), MediaError> {
    if cancel.load(Ordering::Relaxed) {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

fn encode_error(path: &Path, reason: impl std::fmt::Display) -> MediaError {
    MediaError::Encode {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

/// Writes mono or stereo planar audio using a native encoder. Failed or
/// cancelled writes leave an existing destination untouched and remove their
/// temporary files. No external encoder executable is required.
pub fn write_audio(
    path: impl AsRef<Path>,
    channels: &[&[f32]],
    sample_rate: u32,
    encoding: AudioEncoding,
    cancel: &AtomicBool,
) -> Result<(), MediaError> {
    let path = path.as_ref();
    encoding.validate(sample_rate)?;
    if channels.is_empty() || channels.len() > 2 {
        return Err(MediaError::InvalidArgument(
            "export requires one or two channels".into(),
        ));
    }
    let frames = channels[0].len();
    if channels.iter().any(|c| c.len() != frames) {
        return Err(MediaError::InvalidArgument(
            "channels have different lengths".into(),
        ));
    }
    // RIFF and AIFF both use 32-bit container and data lengths. Reject an
    // oversized file before allocating a temporary file or truncating data.
    if matches!(encoding.format, AudioFormat::Wav | AudioFormat::Aiff) {
        let bytes = frames as u64 * channels.len() as u64 * u64::from(encoding.bits / 8);
        if bytes > u64::from(u32::MAX) - 128 {
            return Err(MediaError::InvalidArgument(
                "audio exceeds this format's 4 GB file limit".into(),
            ));
        }
    }
    check_cancel(cancel)?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = path.with_file_name(name);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| MediaError::io(path, e))?;
    let result = (|| {
        let mut output = BufWriter::with_capacity(1 << 16, file);
        match encoding.format {
            AudioFormat::Wav => {
                write_wave(&mut output, channels, sample_rate, encoding, cancel, path)
            }
            AudioFormat::Aiff => {
                write_aiff(&mut output, channels, sample_rate, encoding, cancel, path)
            }
            AudioFormat::Mp3 => {
                write_mp3(&mut output, channels, sample_rate, encoding, cancel, path)
            }
            AudioFormat::Flac => {
                write_flac(&mut output, channels, sample_rate, encoding, cancel, path)
            }
        }?;
        output.flush().map_err(|e| MediaError::io(path, e))?;
        output
            .get_ref()
            .sync_all()
            .map_err(|e| MediaError::io(path, e))?;
        drop(output);
        check_cancel(cancel)?;
        fs::rename(&temp, path).map_err(|e| MediaError::io(path, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

fn quantize(x: f32, bits: u16, dither: bool, rng: &mut Rng) -> i32 {
    let scale = f64::from(1u32 << (bits - 1));
    let noise = if dither { rng.tpdf() } else { 0.0 };
    (f64::from(clean(x)) * scale + noise)
        .round()
        .clamp(-scale, scale - 1.0) as i32
}

fn write_wave(
    out: &mut BufWriter<File>,
    channels: &[&[f32]],
    rate: u32,
    enc: AudioEncoding,
    cancel: &AtomicBool,
    path: &Path,
) -> Result<(), MediaError> {
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate: rate,
        bits_per_sample: enc.bits,
        sample_format: if enc.bits == 32 {
            hound::SampleFormat::Float
        } else {
            hound::SampleFormat::Int
        },
    };
    let mut writer = hound::WavWriter::new(out, spec).map_err(|e| encode_error(path, e))?;
    let mut rngs: Vec<_> = (0..channels.len() as u64).map(Rng::new).collect();
    for from in (0..channels[0].len()).step_by(BLOCK) {
        check_cancel(cancel)?;
        for i in from..(from + BLOCK).min(channels[0].len()) {
            for (ch, rng) in channels.iter().zip(&mut rngs) {
                if enc.bits == 32 {
                    writer.write_sample(clean(ch[i]))
                } else {
                    writer.write_sample(quantize(ch[i], enc.bits, enc.dither, rng))
                }
                .map_err(|e| encode_error(path, e))?;
            }
        }
    }
    writer.finalize().map_err(|e| encode_error(path, e))
}

fn write_aiff(
    out: &mut BufWriter<File>,
    channels: &[&[f32]],
    rate: u32,
    enc: AudioEncoding,
    cancel: &AtomicBool,
    path: &Path,
) -> Result<(), MediaError> {
    let frames = channels[0].len();
    let bytes = (frames * channels.len() * usize::from(enc.bits / 8)) as u32;
    let pad = bytes & 1;
    let mut header = Vec::with_capacity(54);
    header.extend_from_slice(b"FORM");
    header.extend_from_slice(&(46 + bytes + pad).to_be_bytes());
    header.extend_from_slice(b"AIFFCOMM\0\0\0\x12");
    header.extend_from_slice(&(channels.len() as u16).to_be_bytes());
    header.extend_from_slice(&(frames as u32).to_be_bytes());
    header.extend_from_slice(&enc.bits.to_be_bytes());
    // AIFF stores an 80-bit extended sample rate (explicit integer bit).
    let exponent = 31 - rate.leading_zeros();
    header.extend_from_slice(&((16383 + exponent) as u16).to_be_bytes());
    header.extend_from_slice(&(u64::from(rate) << (63 - exponent)).to_be_bytes());
    header.extend_from_slice(b"SSND");
    header.extend_from_slice(&(bytes + 8).to_be_bytes());
    header.extend_from_slice(&[0; 8]); // offset and block size
    out.write_all(&header)
        .map_err(|e| MediaError::io(path, e))?;
    let mut rngs: Vec<_> = (0..channels.len() as u64).map(Rng::new).collect();
    for from in (0..frames).step_by(BLOCK) {
        check_cancel(cancel)?;
        for i in from..(from + BLOCK).min(frames) {
            for (ch, rng) in channels.iter().zip(&mut rngs) {
                let q = quantize(ch[i], enc.bits, enc.dither, rng).to_be_bytes();
                out.write_all(&q[4 - usize::from(enc.bits / 8)..])
                    .map_err(|e| MediaError::io(path, e))?;
            }
        }
    }
    if pad != 0 {
        out.write_all(&[0]).map_err(|e| MediaError::io(path, e))?;
    }
    Ok(())
}

fn write_mp3(
    out: &mut BufWriter<File>,
    channels: &[&[f32]],
    rate: u32,
    enc: AudioEncoding,
    cancel: &AtomicBool,
    path: &Path,
) -> Result<(), MediaError> {
    use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushGap, MonoPcm, Quality, VbrMode};
    let bitrate = match enc.bitrate_kbps {
        128 => Bitrate::Kbps128,
        192 => Bitrate::Kbps192,
        256 => Bitrate::Kbps256,
        _ => Bitrate::Kbps320,
    };
    let mut builder =
        Builder::new().ok_or_else(|| encode_error(path, "could not initialize MP3 encoder"))?;
    let build_err = |e| encode_error(path, e);
    builder
        .set_num_channels(channels.len() as u8)
        .map_err(build_err)?;
    builder.set_sample_rate(rate).map_err(build_err)?;
    builder
        .set_output_sample_rate(std::num::NonZeroU32::new(rate))
        .map_err(build_err)?;
    builder.set_brate(bitrate).map_err(build_err)?;
    builder.set_vbr_mode(VbrMode::Off).map_err(build_err)?;
    builder.set_quality(Quality::Best).map_err(build_err)?;
    // The Info/LAME frame carries encoder delay and padding, enabling exact
    // duration and gapless playback even for CBR exports.
    builder.set_to_write_vbr_tag(true).map_err(build_err)?;
    let mut encoder = builder.build().map_err(build_err)?;
    let mut encoded = Vec::with_capacity(mp3lame_encoder::max_required_buffer_size(BLOCK));
    let mut left = Vec::with_capacity(BLOCK);
    let mut right = Vec::with_capacity(BLOCK);
    for from in (0..channels[0].len()).step_by(BLOCK) {
        check_cancel(cancel)?;
        let to = (from + BLOCK).min(channels[0].len());
        left.clear();
        left.extend(
            channels[0][from..to]
                .iter()
                .map(|&x| clean(x).clamp(-1.0, 1.0)),
        );
        encoded.clear();
        if channels.len() == 1 {
            encoder.encode_to_vec(MonoPcm(&left), &mut encoded)
        } else {
            right.clear();
            right.extend(
                channels[1][from..to]
                    .iter()
                    .map(|&x| clean(x).clamp(-1.0, 1.0)),
            );
            encoder.encode_to_vec(
                DualPcm {
                    left: &left,
                    right: &right,
                },
                &mut encoded,
            )
        }
        .map_err(|e| encode_error(path, e))?;
        out.write_all(&encoded)
            .map_err(|e| MediaError::io(path, e))?;
    }
    encoded.clear();
    encoder
        .flush_to_vec::<FlushGap>(&mut encoded)
        .map_err(|e| encode_error(path, e))?;
    out.write_all(&encoded)
        .map_err(|e| MediaError::io(path, e))?;
    let mut tag = Vec::with_capacity(encoder.lame_tag_size());
    if encoder.lame_tag_encode_to_vec(&mut tag).is_some() {
        out.seek(SeekFrom::Start(encoder.id3v2_tag_size() as u64))
            .map_err(|e| MediaError::io(path, e))?;
        out.write_all(&tag).map_err(|e| MediaError::io(path, e))?;
    }
    Ok(())
}

fn write_flac(
    out: &mut BufWriter<File>,
    channels: &[&[f32]],
    rate: u32,
    enc: AudioEncoding,
    cancel: &AtomicBool,
    path: &Path,
) -> Result<(), MediaError> {
    use flacenc::{
        bitsink::ByteSink,
        component::{BitRepr, Stream},
        error::Verify,
        source::{Context, Fill, FrameBuf},
    };
    let mut config = flacenc::config::Encoder::default();
    config.subframe_coding.fixed.order_sel = flacenc::config::OrderSel::BitCount;
    let config = config
        .into_verified()
        .map_err(|(_, e)| encode_error(path, e))?;
    let mut verbatim = flacenc::config::Encoder::default();
    verbatim.subframe_coding.use_fixed = false;
    verbatim.subframe_coding.use_lpc = false;
    let verbatim = verbatim
        .into_verified()
        .map_err(|(_, e)| encode_error(path, e))?;
    let mut stream = Stream::new(rate as usize, channels.len(), usize::from(enc.bits))
        .map_err(|e| encode_error(path, e))?;
    stream
        .stream_info_mut()
        .set_block_sizes(BLOCK, BLOCK)
        .map_err(|e| encode_error(path, e))?;
    let mut sink = ByteSink::new();
    stream.write(&mut sink).map_err(|e| encode_error(path, e))?;
    out.write_all(sink.as_slice())
        .map_err(|e| MediaError::io(path, e))?;
    let mut framebuf =
        FrameBuf::with_size(channels.len(), BLOCK).map_err(|e| encode_error(path, e))?;
    let mut context = Context::new(usize::from(enc.bits), channels.len());
    let mut quantized = Vec::with_capacity(BLOCK * channels.len());
    let mut rngs: Vec<_> = (0..channels.len() as u64).map(Rng::new).collect();
    for (number, from) in (0..channels[0].len()).step_by(BLOCK).enumerate() {
        check_cancel(cancel)?;
        quantized.clear();
        for i in from..(from + BLOCK).min(channels[0].len()) {
            for (ch, rng) in channels.iter().zip(&mut rngs) {
                quantized.push(quantize(ch[i], enc.bits, enc.dither, rng));
            }
        }
        framebuf
            .fill_interleaved(&quantized)
            .map_err(|e| encode_error(path, e))?;
        context
            .fill_interleaved(&quantized)
            .map_err(|e| encode_error(path, e))?;
        let mut frame =
            flacenc::encode_fixed_size_frame(&config, &framebuf, number, stream.stream_info())
                .map_err(|e| encode_error(path, e))?;
        // Some 24-bit transients make flacenc's predictor cost search choose
        // Rice codes far larger than raw PCM (occasionally exceeding FLAC's
        // 24-bit frame-size field). Count the finished frame before writing
        // it and fall back to constant/verbatim subframes when it expands.
        // Allow ample room for the frame header, subframe headers and CRC.
        let raw_bound = quantized.len() * usize::from(enc.bits) + 256;
        if frame.count_bits() > raw_bound {
            frame = flacenc::encode_fixed_size_frame(
                &verbatim,
                &framebuf,
                number,
                stream.stream_info(),
            )
            .map_err(|e| encode_error(path, e))?;
        }
        stream.stream_info_mut().update_frame_info(&frame);
        sink.clear();
        frame.write(&mut sink).map_err(|e| encode_error(path, e))?;
        out.write_all(sink.as_slice())
            .map_err(|e| MediaError::io(path, e))?;
    }
    stream
        .stream_info_mut()
        .set_md5_digest(&context.md5_digest());
    stream
        .stream_info_mut()
        .set_total_samples(channels[0].len());
    // A fixed-block stream declares the nominal block size in both fields;
    // the final frame may be shorter. update_frame_info includes that final
    // frame in its minimum, which would incorrectly signal variable blocks
    // to strict decoders and can fall below the 16-sample STREAMINFO limit.
    stream
        .stream_info_mut()
        .set_block_sizes(BLOCK, BLOCK)
        .map_err(|e| encode_error(path, e))?;
    sink.clear();
    stream.write(&mut sink).map_err(|e| encode_error(path, e))?;
    out.seek(SeekFrom::Start(0))
        .map_err(|e| MediaError::io(path, e))?;
    out.write_all(sink.as_slice())
        .map_err(|e| MediaError::io(path, e))?;
    Ok(())
}
