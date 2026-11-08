//! Per-port audio buffers handed to a plugin.
//!
//! A CLAP plugin gets one `clap_audio_buffer` for every port it declares, in
//! plugin channel counts, while a Peel `Device` works on a stereo pair. These
//! buffers sit between the two. Everything is allocated once in
//! [`AudioBuffers::new`]; `process` only copies and zeroes.
//!
//! Mapping, with the plugin's main ports as the stereo pair:
//!
//! | Plugin main port | Input from the pair | Output to the pair |
//! | --- | --- | --- |
//! | mono | `(left + right) / 2` | channel 0 to both sides |
//! | stereo | left, right | left, right |
//! | more channels | channels 0 and 1 are left and right, the rest are silent | channels 0 and 1 |
//!
//! Auxiliary ports (sidechains, extra outputs) get silence and their output
//! is discarded.

use clap_sys::audio_buffer::clap_audio_buffer;

use crate::instance::PortInfo;

struct PortBuf {
    channels: usize,
    /// Frames reserved per channel (the maximum block size).
    stride: usize,
    data: Box<[f32]>,
    /// One pointer per channel into `data`, rebuilt before each call.
    ptrs: Box<[*mut f32]>,
}

impl PortBuf {
    fn new(channels: usize, stride: usize) -> PortBuf {
        PortBuf {
            channels,
            stride,
            data: vec![0.0; channels * stride].into_boxed_slice(),
            ptrs: vec![std::ptr::null_mut(); channels].into_boxed_slice(),
        }
    }

    fn channel(&self, ch: usize, frames: usize) -> &[f32] {
        let start = ch * self.stride;
        self.data.get(start..start + frames).unwrap_or(&[])
    }

    fn channel_mut(&mut self, ch: usize, frames: usize) -> &mut [f32] {
        let start = ch * self.stride;
        self.data.get_mut(start..start + frames).unwrap_or(&mut [])
    }

    fn zero(&mut self, frames: usize) {
        for ch in 0..self.channels {
            self.channel_mut(ch, frames).fill(0.0);
        }
    }

    /// Points `ptrs` at the channel starts. Done right before each call so
    /// the pointers are fresh after every safe access to `data`.
    fn rebind(&mut self) {
        let base = self.data.as_mut_ptr();
        for (ch, slot) in self.ptrs.iter_mut().enumerate() {
            *slot = base.wrapping_add(ch * self.stride);
        }
    }

    fn descriptor(&mut self, constant_mask: u64) -> clap_audio_buffer {
        clap_audio_buffer {
            data32: self.ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: self.channels as u32,
            latency: 0,
            constant_mask,
        }
    }

    fn all_channels_mask(&self) -> u64 {
        if self.channels >= 64 {
            u64::MAX
        } else {
            (1u64 << self.channels) - 1
        }
    }
}

pub(crate) struct AudioBuffers {
    inputs: Vec<PortBuf>,
    outputs: Vec<PortBuf>,
    in_desc: Box<[clap_audio_buffer]>,
    out_desc: Box<[clap_audio_buffer]>,
    main_in: Option<usize>,
    main_out: usize,
}

impl AudioBuffers {
    pub(crate) fn new(
        audio_in: &[PortInfo],
        audio_out: &[PortInfo],
        main_in: Option<usize>,
        main_out: usize,
        max_block: usize,
    ) -> AudioBuffers {
        let make = |ports: &[PortInfo]| -> Vec<PortBuf> {
            ports
                .iter()
                .map(|p| PortBuf::new(p.channels as usize, max_block))
                .collect()
        };
        let zero = clap_audio_buffer {
            data32: std::ptr::null_mut(),
            data64: std::ptr::null_mut(),
            channel_count: 0,
            latency: 0,
            constant_mask: 0,
        };
        AudioBuffers {
            inputs: make(audio_in),
            outputs: make(audio_out),
            in_desc: vec![zero; audio_in.len()].into_boxed_slice(),
            out_desc: vec![zero; audio_out.len()].into_boxed_slice(),
            main_in,
            main_out,
        }
    }

    /// Zeroes every buffer for a chunk of `frames`.
    pub(crate) fn clear(&mut self, frames: usize) {
        for p in self.inputs.iter_mut().chain(self.outputs.iter_mut()) {
            p.zero(frames);
        }
    }

    /// Copies the stereo pair into the main input port.
    pub(crate) fn load_main_input(&mut self, left: &[f32], right: &[f32]) {
        let Some(port) = self.main_in.and_then(|i| self.inputs.get_mut(i)) else {
            return;
        };
        let frames = left.len().min(right.len());
        if port.channels == 1 {
            for ((d, l), r) in port.channel_mut(0, frames).iter_mut().zip(left).zip(right) {
                *d = (l + r) * 0.5;
            }
        } else {
            port.channel_mut(0, frames).copy_from_slice(&left[..frames]);
            port.channel_mut(1, frames)
                .copy_from_slice(&right[..frames]);
        }
    }

    /// Builds the descriptor arrays for a call. Returns pointers and counts
    /// for `clap_process`: `(inputs, input count, outputs, output count)`.
    /// The pointers stay valid until the next call to `bind` or `clear`.
    pub(crate) fn bind(&mut self) -> (*const clap_audio_buffer, u32, *mut clap_audio_buffer, u32) {
        for (i, port) in self.inputs.iter_mut().enumerate() {
            port.rebind();
            // Auxiliary inputs are silent: say so, so plugins can skip them.
            let mask = if Some(i) == self.main_in {
                0
            } else {
                port.all_channels_mask()
            };
            self.in_desc[i] = port.descriptor(mask);
        }
        for (i, port) in self.outputs.iter_mut().enumerate() {
            port.rebind();
            self.out_desc[i] = port.descriptor(0);
        }
        let inputs = if self.in_desc.is_empty() {
            std::ptr::null()
        } else {
            self.in_desc.as_ptr()
        };
        let outputs = if self.out_desc.is_empty() {
            std::ptr::null_mut()
        } else {
            self.out_desc.as_mut_ptr()
        };
        (
            inputs,
            self.in_desc.len() as u32,
            outputs,
            self.out_desc.len() as u32,
        )
    }

    /// Expands channels the plugin marked constant (`constant_mask`): only
    /// their first sample is valid.
    fn expand_constant_channels(&mut self, frames: usize) {
        for (port, desc) in self.outputs.iter_mut().zip(self.out_desc.iter()) {
            if desc.constant_mask == 0 {
                continue;
            }
            for ch in 0..port.channels.min(64) {
                if desc.constant_mask & (1 << ch) != 0 {
                    let chan = port.channel_mut(ch, frames);
                    if let Some(&first) = chan.first() {
                        chan.fill(first);
                    }
                }
            }
        }
    }

    /// Copies the main output into the stereo pair and returns the peak
    /// absolute sample of what was copied.
    pub(crate) fn store_main_output(&mut self, left: &mut [f32], right: &mut [f32]) -> f32 {
        let frames = left.len().min(right.len());
        self.expand_constant_channels(frames);
        let Some(port) = self.outputs.get(self.main_out) else {
            return 0.0;
        };
        let c0 = port.channel(0, frames);
        let c1 = if port.channels >= 2 {
            port.channel(1, frames)
        } else {
            c0
        };
        let mut peak = 0.0f32;
        for (((l, r), a), b) in left.iter_mut().zip(right.iter_mut()).zip(c0).zip(c1) {
            *l = *a;
            *r = *b;
            peak = peak.max(a.abs()).max(b.abs());
        }
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(channels: u32, is_main: bool) -> PortInfo {
        PortInfo { channels, is_main }
    }

    #[test]
    fn stereo_input_round_trips_to_the_pair() {
        let mut b = AudioBuffers::new(&[port(2, true)], &[port(2, true)], Some(0), 0, 8);
        b.clear(4);
        b.load_main_input(&[1.0, 2.0, 3.0, 4.0], &[-1.0, -2.0, -3.0, -4.0]);
        let (ins, n_in, outs, n_out) = b.bind();
        assert_eq!((n_in, n_out), (1, 1));
        assert!(!ins.is_null() && !outs.is_null());
        let input = unsafe { &*ins };
        assert_eq!(input.channel_count, 2);
        assert_eq!(input.constant_mask, 0);
        let ch0 = unsafe { std::slice::from_raw_parts(*input.data32, 4) };
        assert_eq!(ch0, [1.0, 2.0, 3.0, 4.0]);

        // The "plugin" copies input to output.
        let out = unsafe { &*outs };
        for ch in 0..2 {
            let src = unsafe { std::slice::from_raw_parts(*input.data32.add(ch), 4) };
            let dst = unsafe { std::slice::from_raw_parts_mut(*out.data32.add(ch), 4) };
            dst.copy_from_slice(src);
        }
        let (mut l, mut r) = ([0.0; 4], [0.0; 4]);
        let peak = b.store_main_output(&mut l, &mut r);
        assert_eq!(l, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(r, [-1.0, -2.0, -3.0, -4.0]);
        assert_eq!(peak, 4.0);
    }

    #[test]
    fn mono_input_is_averaged_and_mono_output_is_duplicated() {
        let mut b = AudioBuffers::new(&[port(1, true)], &[port(1, true)], Some(0), 0, 4);
        b.clear(2);
        b.load_main_input(&[1.0, 0.5], &[0.0, 0.5]);
        let (ins, _, outs, _) = b.bind();
        let input = unsafe { &*ins };
        let mono = unsafe { std::slice::from_raw_parts(*input.data32, 2) };
        assert_eq!(mono, [0.5, 0.5]);
        let out = unsafe { &*outs };
        unsafe { std::slice::from_raw_parts_mut(*out.data32, 2) }.copy_from_slice(&[0.25, -0.25]);
        let (mut l, mut r) = ([0.0; 2], [0.0; 2]);
        b.store_main_output(&mut l, &mut r);
        assert_eq!(l, [0.25, -0.25]);
        assert_eq!(r, [0.25, -0.25]);
    }

    #[test]
    fn extra_channels_and_aux_ports_are_handled() {
        let mut b = AudioBuffers::new(
            &[port(4, true), port(2, false)],
            &[port(6, true), port(2, false)],
            Some(0),
            0,
            4,
        );
        b.clear(2);
        b.load_main_input(&[1.0, 1.0], &[2.0, 2.0]);
        let (ins, n_in, outs, n_out) = b.bind();
        assert_eq!((n_in, n_out), (2, 2));
        let descs = unsafe { std::slice::from_raw_parts(ins, 2) };
        assert_eq!(descs[0].constant_mask, 0);
        assert_eq!(descs[1].constant_mask, 0b11, "aux input flagged silent");
        let ch3 = unsafe { std::slice::from_raw_parts(*descs[0].data32.add(3), 2) };
        assert_eq!(ch3, [0.0, 0.0]);

        let outd = unsafe { std::slice::from_raw_parts(outs, 2) };
        unsafe { std::slice::from_raw_parts_mut(*outd[0].data32.add(1), 2) }
            .copy_from_slice(&[0.5, 0.5]);
        let (mut l, mut r) = ([9.0; 2], [9.0; 2]);
        b.store_main_output(&mut l, &mut r);
        assert_eq!((l, r), ([0.0; 2], [0.5; 2]));
    }

    #[test]
    fn constant_channels_are_expanded() {
        let mut b = AudioBuffers::new(&[], &[port(2, true)], None, 0, 4);
        b.clear(4);
        let (ins, n_in, outs, _) = b.bind();
        assert!(ins.is_null() && n_in == 0);
        let out = unsafe { &mut *outs };
        // Plugin writes only the first sample of channel 1 and flags it.
        unsafe { *(*out.data32.add(1)) = 0.75 };
        out.constant_mask = 0b10;
        let (mut l, mut r) = ([1.0; 4], [1.0; 4]);
        b.store_main_output(&mut l, &mut r);
        assert_eq!(l, [0.0; 4]);
        assert_eq!(r, [0.75; 4]);
    }
}
