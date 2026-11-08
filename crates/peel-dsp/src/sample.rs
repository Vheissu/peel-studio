/// Decoded audio held in memory, planar 32-bit float.
///
/// Created and released on control threads only. Devices hold it through an
/// `Arc` and hand the previous one back via [`crate::Device::set_sample`] so
/// the audio thread never frees it.
#[derive(Clone, Debug, Default)]
pub struct SampleBuffer {
    /// Display name (original file name).
    pub name: String,
    pub sample_rate: f32,
    /// One `Vec` per channel, all the same length.
    pub channels: Vec<Vec<f32>>,
    /// MIDI key at which the sample plays at its original pitch.
    pub root_key: u8,
    /// Optional loop region in frames (start inclusive, end exclusive).
    pub loop_range: Option<(usize, usize)>,
}

impl SampleBuffer {
    pub fn new(name: impl Into<String>, sample_rate: f32, channels: Vec<Vec<f32>>) -> SampleBuffer {
        SampleBuffer {
            name: name.into(),
            sample_rate,
            channels,
            root_key: 60,
            loop_range: None,
        }
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn duration_seconds(&self) -> f32 {
        if self.sample_rate > 0.0 {
            self.frames() as f32 / self.sample_rate
        } else {
            0.0
        }
    }

    /// Left channel (or mono).
    pub fn left(&self) -> &[f32] {
        self.channels.first().map_or(&[], Vec::as_slice)
    }

    /// Right channel, or the left channel for mono material.
    pub fn right(&self) -> &[f32] {
        self.channels
            .get(1)
            .or(self.channels.first())
            .map_or(&[], Vec::as_slice)
    }
}
