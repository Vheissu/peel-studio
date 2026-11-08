use peel_dsp::SampleBuffer;

/// Frames summarized by one entry of the finest level.
const BASE_BLOCK: usize = 64;
/// Each coarser level merges this many entries of the one below.
const FANOUT: usize = 4;

/// One resolution of the pyramid: `(min, max)` per block of `block` frames.
struct Level {
    block: usize,
    /// `data[channel][block_index]`.
    data: Vec<Vec<(f32, f32)>>,
}

/// A multiresolution min/max summary of a sample buffer, for drawing
/// waveforms at any zoom without touching the samples.
///
/// The finest level holds one `(min, max)` pair per 64 frames; each coarser
/// level merges four entries of the previous one, up to a single entry. The
/// pyramid costs about 3 % of the sample memory per channel and is built in
/// one pass.
pub struct Peaks {
    frames: usize,
    channels: usize,
    levels: Vec<Level>,
}

impl Peaks {
    /// Builds the pyramid for every channel of `buf`.
    pub fn compute(buf: &SampleBuffer) -> Peaks {
        let frames = buf.frames();
        let channels = buf.channels.len();
        let mut levels = Vec::new();
        if frames > 0 && channels > 0 {
            let base: Vec<Vec<(f32, f32)>> = buf
                .channels
                .iter()
                .map(|c| {
                    c[..frames]
                        .chunks(BASE_BLOCK)
                        .map(|chunk| {
                            chunk.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| {
                                // Non-finite samples would poison the display.
                                let x = if x.is_finite() { x } else { 0.0 };
                                (lo.min(x), hi.max(x))
                            })
                        })
                        .collect()
                })
                .collect();
            levels.push(Level {
                block: BASE_BLOCK,
                data: base,
            });
            while levels.last().is_some_and(|l| l.data[0].len() > 1) {
                let prev = levels.last().expect("checked above");
                let data = prev
                    .data
                    .iter()
                    .map(|ch| {
                        ch.chunks(FANOUT)
                            .map(|c| {
                                c.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &(a, b)| {
                                    (lo.min(a), hi.max(b))
                                })
                            })
                            .collect()
                    })
                    .collect();
                let block = prev.block * FANOUT;
                levels.push(Level { block, data });
            }
        }
        Peaks {
            frames,
            channels,
            levels,
        }
    }

    /// Frames in the source buffer.
    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Number of resolutions (zero for an empty buffer).
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Frames summarized by one entry of each level, finest first.
    pub fn block_sizes(&self) -> impl Iterator<Item = usize> + '_ {
        self.levels.iter().map(|l| l.block)
    }

    /// `columns` `(min, max)` pairs covering frames `start_frame..end_frame`,
    /// merged across all channels, picking the coarsest level that still has
    /// at least one entry per column.
    ///
    /// Zoomed in below the finest level (fewer than 64 frames per column)
    /// the pairs are those of the 64-frame block under each column: draw the
    /// raw samples instead at that zoom. Columns outside the buffer are
    /// `(0.0, 0.0)`. An empty range or zero columns gives an empty iterator.
    pub fn query(&self, start_frame: usize, end_frame: usize, columns: usize) -> PeakQuery<'_> {
        self.query_inner(None, start_frame, end_frame, columns)
    }

    /// Like [`query`](Peaks::query) for a single channel.
    pub fn query_channel(
        &self,
        channel: usize,
        start_frame: usize,
        end_frame: usize,
        columns: usize,
    ) -> PeakQuery<'_> {
        self.query_inner(Some(channel), start_frame, end_frame, columns)
    }

    fn query_inner(
        &self,
        channel: Option<usize>,
        start: usize,
        end: usize,
        columns: usize,
    ) -> PeakQuery<'_> {
        let valid = end > start && columns > 0 && !self.levels.is_empty();
        let valid = valid && channel.is_none_or(|c| c < self.channels);
        let step = if valid {
            (end - start) as f64 / columns as f64
        } else {
            1.0
        };
        // Coarsest level whose block fits in a column.
        let level = self
            .levels
            .iter()
            .rposition(|l| l.block as f64 <= step)
            .unwrap_or(0);
        PeakQuery {
            peaks: self,
            channel,
            start,
            step,
            level,
            next: 0,
            columns: if valid { columns } else { 0 },
        }
    }
}

/// Iterator returned by [`Peaks::query`]: exactly one `(min, max)` per column.
pub struct PeakQuery<'a> {
    peaks: &'a Peaks,
    channel: Option<usize>,
    start: usize,
    step: f64,
    level: usize,
    next: usize,
    columns: usize,
}

impl Iterator for PeakQuery<'_> {
    type Item = (f32, f32);

    fn next(&mut self) -> Option<(f32, f32)> {
        if self.next >= self.columns {
            return None;
        }
        let i = self.next;
        self.next += 1;
        let frames = self.peaks.frames;
        let a = (self.start as f64 + i as f64 * self.step).floor() as usize;
        if a >= frames {
            return Some((0.0, 0.0));
        }
        let b = ((self.start as f64 + (i + 1) as f64 * self.step).ceil() as usize)
            .max(a + 1)
            .min(frames);
        let level = &self.peaks.levels[self.level];
        let first = a / level.block;
        let last = b.div_ceil(level.block).min(level.data[0].len());
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let mut scan = |ch: &Vec<(f32, f32)>| {
            for &(l, h) in &ch[first..last] {
                lo = lo.min(l);
                hi = hi.max(h);
            }
        };
        match self.channel {
            Some(c) => scan(&level.data[c]),
            None => level.data.iter().for_each(&mut scan),
        }
        if lo > hi {
            Some((0.0, 0.0))
        } else {
            Some((lo, hi))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.columns - self.next;
        (n, Some(n))
    }
}

impl ExactSizeIterator for PeakQuery<'_> {}
