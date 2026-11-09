use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::error::MediaError;

/// Output format for [`write_wav`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavFormat {
    /// 16 or 24 for integer PCM, 32 for IEEE float.
    pub bits: u16,
    /// Add TPDF dither before quantizing. Applies to 16 and 24-bit output
    /// only; float output is never dithered.
    pub dither: bool,
}

impl WavFormat {
    /// 16-bit PCM with TPDF dither (the usual choice for a final mix).
    pub const PCM16: WavFormat = WavFormat {
        bits: 16,
        dither: true,
    };
    /// 24-bit PCM with TPDF dither.
    pub const PCM24: WavFormat = WavFormat {
        bits: 24,
        dither: true,
    };
    /// 32-bit float, undithered.
    pub const FLOAT32: WavFormat = WavFormat {
        bits: 32,
        dither: false,
    };
}

impl Default for WavFormat {
    fn default() -> Self {
        WavFormat::PCM24
    }
}

/// xorshift64* generator for dither. Deterministic, so the same input always
/// produces the same file.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(stream: u64) -> Rng {
        Rng(0x9E37_79B9_7F4A_7C15 ^ stream.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1)
    }

    /// Uniform in `[-0.5, 0.5)`.
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (x >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    }

    /// Triangular noise with a peak of one least-significant bit: the sum of
    /// two independent uniform ±0.5 LSB values.
    pub(crate) fn tpdf(&mut self) -> f64 {
        self.next() + self.next()
    }
}

/// Writes planar float audio as a WAV file.
///
/// The data goes to a temporary file next to `path` and is renamed over it
/// once complete and flushed, so a failed or interrupted export never leaves
/// a truncated file behind or destroys an existing one.
///
/// All channels must have the same length. Samples beyond full scale are
/// clipped when quantizing to integers; non-finite samples are written as
/// silence.
pub fn write_wav(
    path: impl AsRef<Path>,
    channels: &[&[f32]],
    sample_rate: u32,
    format: WavFormat,
) -> Result<(), MediaError> {
    let path = path.as_ref();
    if channels.is_empty() {
        return Err(MediaError::InvalidArgument("no channels to write".into()));
    }
    if channels.len() > usize::from(u16::MAX) {
        return Err(MediaError::InvalidArgument("too many channels".into()));
    }
    if sample_rate == 0 {
        return Err(MediaError::InvalidArgument("sample rate is zero".into()));
    }
    let frames = channels[0].len();
    if channels.iter().any(|c| c.len() != frames) {
        return Err(MediaError::InvalidArgument(
            "channels have different lengths".into(),
        ));
    }
    let sample_format = match format.bits {
        16 | 24 => SampleFormat::Int,
        32 => SampleFormat::Float,
        other => {
            return Err(MediaError::InvalidArgument(format!(
                "unsupported bit depth {other} (use 16, 24 or 32 for float)"
            )));
        }
    };
    let spec = WavSpec {
        channels: channels.len() as u16,
        sample_rate,
        bits_per_sample: format.bits,
        sample_format,
    };

    let tmp = temp_path(path);
    let result = write_to(&tmp, spec, channels, format)
        .and_then(|()| fs::rename(&tmp, path).map_err(|e| MediaError::io(path, e)));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

fn write_to(
    tmp: &Path,
    spec: WavSpec,
    channels: &[&[f32]],
    format: WavFormat,
) -> Result<(), MediaError> {
    let encode = |e: hound::Error| MediaError::Encode {
        path: tmp.to_path_buf(),
        reason: e.to_string(),
    };
    let file = File::create(tmp).map_err(|e| MediaError::io(tmp, e))?;
    let mut buffered = BufWriter::with_capacity(1 << 16, file);
    {
        let mut writer = WavWriter::new(&mut buffered, spec).map_err(encode)?;
        let frames = channels[0].len();
        match format.bits {
            32 => {
                for i in 0..frames {
                    for ch in channels {
                        let x = ch[i];
                        writer
                            .write_sample(if x.is_finite() { x } else { 0.0 })
                            .map_err(encode)?;
                    }
                }
            }
            bits => {
                let scale = f64::from(1u32 << (bits - 1));
                let (lo, hi) = (-scale, scale - 1.0);
                let mut rngs: Vec<Rng> = (0..channels.len() as u64).map(Rng::new).collect();
                for i in 0..frames {
                    for (ch, rng) in channels.iter().zip(rngs.iter_mut()) {
                        let x = f64::from(ch[i]);
                        let x = if x.is_finite() { x } else { 0.0 };
                        let noise = if format.dither { rng.tpdf() } else { 0.0 };
                        let q = (x * scale + noise).round().clamp(lo, hi);
                        writer.write_sample(q as i32).map_err(encode)?;
                    }
                }
            }
        }
        writer.finalize().map_err(encode)?;
    }
    buffered.flush().map_err(|e| MediaError::io(tmp, e))?;
    buffered
        .get_ref()
        .sync_all()
        .map_err(|e| MediaError::io(tmp, e))?;
    Ok(())
}
