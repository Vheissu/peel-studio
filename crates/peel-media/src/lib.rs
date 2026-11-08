//! Audio file decoding, WAV export and waveform peak caches.
//!
//! Required API (implemented by the engine/media work stream):
//!
//! * `decode_file(path) -> Result<SampleBuffer, MediaError>`
//! * `resample(&SampleBuffer, target_rate) -> SampleBuffer`
//! * `write_wav(path, &[&[f32]], sample_rate, WavFormat) -> Result<(), MediaError>`
//! * `Peaks::compute(&SampleBuffer)` multiresolution min/max pyramid
//! * `is_audio_file(path) -> bool`
