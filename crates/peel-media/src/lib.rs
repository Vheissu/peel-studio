//! Audio file decoding, resampling, export and waveform peak caches.
//!
//! Everything here runs on control threads; none of it is realtime-safe.
//!
//! * [`decode_file`] reads WAV, AIFF, FLAC, MP3 and Ogg Vorbis into a planar
//!   32-bit float [`SampleBuffer`] at the file's native rate.
//! * [`resample`] converts a buffer to another rate with a Kaiser-windowed
//!   sinc; [`load_for_engine`] is decode plus resample.
//! * [`write_wav`] writes 16 or 24-bit PCM (optionally TPDF dithered) or
//!   32-bit float, atomically.
//! * [`write_audio`] writes WAV, MP3, FLAC and AIFF with native encoders,
//!   cancellation and atomic replacement of the destination.
//! * [`WavRecorder`] appends float audio to a WAV while it is captured and
//!   keeps the file valid after every checkpoint, so a crash loses nothing
//!   (see [`repair_wav_header`]).
//! * [`Peaks`] is a min/max pyramid for drawing waveforms at any zoom.
//! * [`is_audio_file`] and [`probe`] classify files without decoding them.

mod decode;
mod encode;
mod error;
mod peaks;
mod record;
mod resample;
mod wav;

pub use decode::{AudioInfo, decode_file, is_audio_file, load_for_engine, probe};
pub use encode::{AudioEncoding, AudioFormat, write_audio};
pub use error::MediaError;
pub use peaks::{PeakQuery, Peaks};
pub use peel_dsp::SampleBuffer;
pub use record::{RecordedWav, WavRecorder, repair_wav_header};
pub use resample::resample;
pub use wav::{WavFormat, write_wav};
