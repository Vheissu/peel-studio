//! Crash-safe WAV recording.
//!
//! [`WavRecorder`] appends 32-bit float audio to a file while it is being
//! captured. The header is written first, with a data length of zero, so the
//! file is a valid (empty) WAV from the first moment. [`checkpoint`]
//! (WavRecorder::checkpoint) rewrites the two size fields to cover everything
//! written so far, which makes the file a valid, playable WAV at that point
//! even if the process dies before [`finalize`](WavRecorder::finalize). Data
//! goes straight to the file with `write(2)`, so a crash of the application
//! (not of the operating system) loses nothing that was written; [`sync`]
//! (WavRecorder::sync) additionally asks the operating system to put it on the
//! disk.
//!
//! [`repair_wav_header`] fixes the header of a file that was cut off between
//! checkpoints, by trusting the file length instead of the header.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::MediaError;

/// Bytes before the sample data: `RIFF` header, `fmt ` chunk, `data` header.
const HEADER_LEN: u64 = 44;
/// Offset of the RIFF chunk size.
const RIFF_SIZE_AT: u64 = 4;
/// Offset of the data chunk size.
const DATA_SIZE_AT: u64 = 40;
/// WAV chunk sizes are 32 bits. This is the most sample data one file holds.
const MAX_DATA_BYTES: u64 = u32::MAX as u64 - (HEADER_LEN - 8);
const BYTES_PER_SAMPLE: u64 = 4;
/// Samples converted and written per `write` call.
const WRITE_CHUNK: usize = 1 << 14;

/// A finished recording.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedWav {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
    /// Frames (samples per channel) in the file.
    pub frames: u64,
}

impl RecordedWav {
    pub fn seconds(&self) -> f64 {
        self.frames as f64 / f64::from(self.sample_rate.max(1))
    }
}

/// Appends interleaved 32-bit float frames to a WAV file as they arrive.
///
/// Not realtime-safe: it allocates and does file I/O. Run it on a writer
/// thread that drains the capture ring.
pub struct WavRecorder {
    file: File,
    path: PathBuf,
    channels: u16,
    sample_rate: u32,
    frames: u64,
    /// Frames the header on disk currently declares.
    declared_frames: u64,
    scratch: Vec<u8>,
    finished: bool,
}

impl WavRecorder {
    /// Creates `path` (and its parent folders) and writes a valid, empty WAV.
    /// An existing file is never overwritten: the call fails instead.
    pub fn create(
        path: impl AsRef<Path>,
        channels: u16,
        sample_rate: u32,
    ) -> Result<WavRecorder, MediaError> {
        let path = path.as_ref();
        if channels == 0 {
            return Err(MediaError::InvalidArgument("no channels to record".into()));
        }
        if sample_rate == 0 {
            return Err(MediaError::InvalidArgument("sample rate is zero".into()));
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| MediaError::io(parent, e))?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| MediaError::io(path, e))?;
        file.write_all(&header(channels, sample_rate, 0))
            .map_err(|e| MediaError::io(path, e))?;
        Ok(WavRecorder {
            file,
            path: path.to_path_buf(),
            channels,
            sample_rate,
            frames: 0,
            declared_frames: 0,
            scratch: Vec::with_capacity(WRITE_CHUNK * BYTES_PER_SAMPLE as usize),
            finished: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Frames the header on disk declares (what a reader would see after a
    /// crash right now).
    pub fn declared_frames(&self) -> u64 {
        self.declared_frames
    }

    /// Appends interleaved frames. Non-finite samples are written as silence.
    /// `samples.len()` must be a whole number of frames. Fails, writing
    /// nothing, when the file would pass the 4 GiB WAV limit.
    pub fn write(&mut self, samples: &[f32]) -> Result<(), MediaError> {
        let channels = usize::from(self.channels);
        if !samples.len().is_multiple_of(channels) {
            return Err(MediaError::InvalidArgument(
                "samples are not a whole number of frames".into(),
            ));
        }
        let data_bytes = self.frames * u64::from(self.channels) * BYTES_PER_SAMPLE;
        if data_bytes + samples.len() as u64 * BYTES_PER_SAMPLE > MAX_DATA_BYTES {
            return Err(MediaError::Encode {
                path: self.path.clone(),
                reason: "the WAV file reached its 4 GiB limit".into(),
            });
        }
        for chunk in samples.chunks(WRITE_CHUNK - WRITE_CHUNK % channels) {
            self.scratch.clear();
            for &x in chunk {
                let x = if x.is_finite() { x } else { 0.0 };
                self.scratch.extend_from_slice(&x.to_le_bytes());
            }
            self.file
                .write_all(&self.scratch)
                .map_err(|e| MediaError::io(&self.path, e))?;
            self.frames += (chunk.len() / channels) as u64;
        }
        Ok(())
    }

    /// Rewrites the header so it covers everything written so far. The file
    /// is a valid WAV of that length from here on, whatever happens next.
    pub fn checkpoint(&mut self) -> Result<(), MediaError> {
        if self.declared_frames == self.frames {
            return Ok(());
        }
        let path = &self.path;
        let io = |e| MediaError::io(path, e);
        let data_bytes = self.frames * u64::from(self.channels) * BYTES_PER_SAMPLE;
        self.file.seek(SeekFrom::Start(RIFF_SIZE_AT)).map_err(io)?;
        self.file
            .write_all(&((data_bytes + HEADER_LEN - 8) as u32).to_le_bytes())
            .map_err(io)?;
        self.file.seek(SeekFrom::Start(DATA_SIZE_AT)).map_err(io)?;
        self.file
            .write_all(&(data_bytes as u32).to_le_bytes())
            .map_err(io)?;
        self.file
            .seek(SeekFrom::Start(HEADER_LEN + data_bytes))
            .map_err(io)?;
        self.declared_frames = self.frames;
        Ok(())
    }

    /// [`checkpoint`](WavRecorder::checkpoint), then asks the operating system
    /// to write the file to the disk (survives power loss, not only a crash).
    pub fn sync(&mut self) -> Result<(), MediaError> {
        self.checkpoint()?;
        self.file
            .sync_data()
            .map_err(|e| MediaError::io(&self.path, e))
    }

    /// Writes the final header, syncs, and returns the file's description.
    pub fn finalize(mut self) -> Result<RecordedWav, MediaError> {
        self.checkpoint()?;
        self.file
            .sync_all()
            .map_err(|e| MediaError::io(&self.path, e))?;
        self.finished = true;
        Ok(RecordedWav {
            path: self.path.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            frames: self.frames,
        })
    }
}

impl Drop for WavRecorder {
    fn drop(&mut self) {
        // A recorder dropped without `finalize` (an error path, a panic that
        // unwinds) still leaves a valid file behind.
        if !self.finished {
            let _ = self.checkpoint();
        }
    }
}

fn header(channels: u16, sample_rate: u32, frames: u64) -> [u8; HEADER_LEN as usize] {
    let block_align = channels * BYTES_PER_SAMPLE as u16;
    let data_bytes = (frames * u64::from(block_align)) as u32;
    let mut h = [0u8; HEADER_LEN as usize];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(data_bytes + (HEADER_LEN as u32 - 8)).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&3u16.to_le_bytes()); // IEEE float
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    h[28..32].copy_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
    h[32..34].copy_from_slice(&block_align.to_le_bytes());
    h[34..36].copy_from_slice(&32u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

/// Repairs the size fields of a recording that was not finalized, from the
/// file's length. Whole frames after the last checkpoint are kept; a partial
/// frame at the end is cut off. Returns the number of frames in the file.
///
/// Only files written by [`WavRecorder`] (a 44-byte header, 32-bit float) are
/// accepted.
pub fn repair_wav_header(path: impl AsRef<Path>) -> Result<RecordedWav, MediaError> {
    let path = path.as_ref();
    let io = |e| MediaError::io(path, e);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(io)?;
    let mut head = [0u8; HEADER_LEN as usize];
    file.read_exact(&mut head).map_err(io)?;
    let channels = u16::from_le_bytes([head[22], head[23]]);
    let sample_rate = u32::from_le_bytes([head[24], head[25], head[26], head[27]]);
    let format = u16::from_le_bytes([head[20], head[21]]);
    let bits = u16::from_le_bytes([head[34], head[35]]);
    let ours = &head[0..4] == b"RIFF"
        && &head[8..16] == b"WAVEfmt "
        && &head[36..40] == b"data"
        && format == 3
        && bits == 32
        && channels > 0
        && sample_rate > 0;
    if !ours {
        return Err(MediaError::Unsupported {
            path: path.to_path_buf(),
            reason: "not a float WAV written by the recorder".into(),
        });
    }
    let len = file.metadata().map_err(io)?.len();
    let frame_bytes = u64::from(channels) * BYTES_PER_SAMPLE;
    let frames = (len.saturating_sub(HEADER_LEN) / frame_bytes).min(MAX_DATA_BYTES / frame_bytes);
    file.set_len(HEADER_LEN + frames * frame_bytes)
        .map_err(io)?;
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    file.write_all(&header(channels, sample_rate, frames))
        .map_err(io)?;
    file.sync_all().map_err(io)?;
    Ok(RecordedWav {
        path: path.to_path_buf(),
        sample_rate,
        channels,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_file;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("peel-media-rec-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ramp(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| ((i / channels) as f32 * 0.001 + (i % channels) as f32 * 0.25).sin())
            .collect()
    }

    #[test]
    fn a_new_recording_is_a_valid_empty_wav() {
        let dir = scratch("empty");
        let path = dir.join("take.wav");
        let rec = WavRecorder::create(&path, 2, 48_000).unwrap();
        // Readable before a single frame arrives.
        let info = crate::probe(&path).unwrap();
        assert_eq!((info.sample_rate, info.channels), (48_000, 2));
        assert_eq!(info.frames, Some(0));
        drop(rec);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn finalized_audio_decodes_to_exactly_what_was_written() {
        let dir = scratch("final");
        let path = dir.join("nested/take.wav");
        let mut rec = WavRecorder::create(&path, 2, 44_100).unwrap();
        let audio = ramp(10_000, 2);
        for part in audio.chunks(2 * 777) {
            rec.write(part).unwrap();
        }
        let done = rec.finalize().unwrap();
        assert_eq!(
            (done.frames, done.channels, done.sample_rate),
            (10_000, 2, 44_100)
        );
        let buf = decode_file(&path).unwrap();
        assert_eq!(buf.frames(), 10_000);
        for i in 0..10_000 {
            assert_eq!(buf.left()[i], audio[2 * i]);
            assert_eq!(buf.right()[i], audio[2 * i + 1]);
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_checkpointed_file_is_valid_without_finalize() {
        let dir = scratch("crash");
        let path = dir.join("take.wav");
        let mut rec = WavRecorder::create(&path, 1, 48_000).unwrap();
        let audio = ramp(5_000, 1);
        rec.write(&audio[..3_000]).unwrap();
        rec.checkpoint().unwrap();
        rec.write(&audio[3_000..]).unwrap(); // after the last checkpoint
        assert_eq!((rec.frames(), rec.declared_frames()), (5_000, 3_000));

        // The process "dies" here: the recorder never finalizes or drops.
        let copy = dir.join("after-crash.wav");
        fs::copy(&path, &copy).unwrap();
        std::mem::forget(rec);

        let buf = decode_file(&copy).unwrap();
        assert_eq!(buf.frames(), 3_000, "everything up to the checkpoint");
        assert_eq!(&buf.left()[..3_000], &audio[..3_000]);

        // The tail after the checkpoint is still in the file and recoverable.
        let fixed = repair_wav_header(&copy).unwrap();
        assert_eq!(fixed.frames, 5_000);
        let buf = decode_file(&copy).unwrap();
        assert_eq!(buf.left(), &audio[..]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn repair_cuts_off_a_partial_frame_and_refuses_other_files() {
        let dir = scratch("repair");
        let path = dir.join("take.wav");
        let mut rec = WavRecorder::create(&path, 2, 48_000).unwrap();
        rec.write(&ramp(100, 2)).unwrap();
        std::mem::forget(rec);
        // Three stray bytes, as if the process died mid-write.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&[1, 2, 3]).unwrap();
        drop(f);
        assert_eq!(repair_wav_header(&path).unwrap().frames, 100);
        assert_eq!(decode_file(&path).unwrap().frames(), 100);

        let other = dir.join("other.bin");
        fs::write(&other, vec![0u8; 100]).unwrap();
        assert!(repair_wav_header(&other).is_err());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn dropping_without_finalize_still_leaves_a_valid_file() {
        let dir = scratch("drop");
        let path = dir.join("take.wav");
        let mut rec = WavRecorder::create(&path, 2, 48_000).unwrap();
        rec.write(&ramp(1_234, 2)).unwrap();
        drop(rec);
        assert_eq!(decode_file(&path).unwrap().frames(), 1_234);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn bad_input_is_refused_and_files_are_never_overwritten() {
        let dir = scratch("bad");
        let path = dir.join("take.wav");
        assert!(WavRecorder::create(&path, 0, 48_000).is_err());
        assert!(WavRecorder::create(&path, 2, 0).is_err());
        let mut rec = WavRecorder::create(&path, 2, 48_000).unwrap();
        assert!(rec.write(&[0.0; 3]).is_err(), "a partial frame");
        rec.write(&[f32::NAN, f32::INFINITY, 0.5, -0.5]).unwrap();
        assert!(
            WavRecorder::create(&path, 2, 48_000).is_err(),
            "the same file twice"
        );
        rec.finalize().unwrap();
        let buf = decode_file(&path).unwrap();
        assert_eq!(buf.left(), &[0.0, 0.5]);
        assert_eq!(buf.right(), &[0.0, -0.5]);
        fs::remove_dir_all(dir).ok();
    }
}
