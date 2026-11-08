//! `clap_istream` and `clap_ostream` over byte buffers, for the `state`
//! extension. Both run on the main thread only.

use std::ffi::c_void;

use clap_sys::stream::{clap_istream, clap_ostream};

/// Largest state blob accepted from a plugin. A runaway plugin cannot make
/// the host allocate without bound.
pub(crate) const MAX_STATE_BYTES: usize = 1 << 30;

/// Collects what a plugin writes.
pub(crate) struct StateWriter {
    pub(crate) bytes: Vec<u8>,
    pub(crate) overflowed: bool,
}

impl StateWriter {
    pub(crate) fn new() -> StateWriter {
        StateWriter {
            bytes: Vec::new(),
            overflowed: false,
        }
    }

    /// The stream to hand to `clap_plugin_state.save`. It points at `self`,
    /// so `self` must stay put until the call returns.
    pub(crate) fn stream(&mut self) -> clap_ostream {
        clap_ostream {
            ctx: (self as *mut StateWriter).cast(),
            write: Some(write),
        }
    }
}

extern "C" fn write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    // SAFETY: `ctx` is the `StateWriter` that built the stream and is alive
    // for the call.
    let Some(writer) = (unsafe {
        stream
            .as_ref()
            .and_then(|s| s.ctx.cast::<StateWriter>().as_mut())
    }) else {
        return -1;
    };
    if buffer.is_null() {
        return -1;
    }
    let Ok(size) = usize::try_from(size) else {
        return -1;
    };
    if writer.bytes.len().saturating_add(size) > MAX_STATE_BYTES {
        writer.overflowed = true;
        return -1;
    }
    // SAFETY: the plugin promises `size` readable bytes at `buffer`.
    let chunk = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), size) };
    writer.bytes.extend_from_slice(chunk);
    size as i64
}

/// Serves a byte slice to a plugin that is loading state.
pub(crate) struct StateReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> StateReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> StateReader<'a> {
        StateReader { data, pos: 0 }
    }

    /// The stream to hand to `clap_plugin_state.load`. It points at `self`,
    /// so `self` must stay put until the call returns.
    pub(crate) fn stream(&mut self) -> clap_istream {
        clap_istream {
            ctx: (self as *mut StateReader).cast(),
            read: Some(read),
        }
    }
}

extern "C" fn read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    // SAFETY: `ctx` is the `StateReader` that built the stream and is alive
    // for the call. The lifetime parameter is erased; the reader only hands
    // out copies of its bytes.
    let Some(reader) = (unsafe {
        stream
            .as_ref()
            .and_then(|s| s.ctx.cast::<StateReader<'static>>().as_mut())
    }) else {
        return -1;
    };
    if buffer.is_null() {
        return -1;
    }
    let remaining = reader.data.len() - reader.pos;
    let count = remaining.min(usize::try_from(size).unwrap_or(usize::MAX));
    if count == 0 {
        return 0;
    }
    // SAFETY: the plugin promises `size >= count` writable bytes at
    // `buffer`; the source range is inside `data`.
    unsafe {
        std::ptr::copy_nonoverlapping(
            reader.data[reader.pos..].as_ptr(),
            buffer.cast::<u8>(),
            count,
        );
    }
    reader.pos += count;
    count as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_collects_chunks() {
        let mut w = StateWriter::new();
        let s = w.stream();
        let f = s.write.unwrap();
        assert_eq!(unsafe { f(&s, b"abc".as_ptr().cast(), 3) }, 3);
        assert_eq!(unsafe { f(&s, b"de".as_ptr().cast(), 2) }, 2);
        assert_eq!(w.bytes, b"abcde");
        assert!(!w.overflowed);
        assert_eq!(unsafe { f(&s, std::ptr::null(), 1) }, -1);
    }

    #[test]
    fn reader_serves_partial_reads_then_eof() {
        let data = *b"0123456789";
        let mut r = StateReader::new(&data);
        let s = r.stream();
        let f = s.read.unwrap();
        let mut buf = [0u8; 4];
        assert_eq!(unsafe { f(&s, buf.as_mut_ptr().cast(), 4) }, 4);
        assert_eq!(&buf, b"0123");
        let mut big = [0u8; 16];
        assert_eq!(unsafe { f(&s, big.as_mut_ptr().cast(), 16) }, 6);
        assert_eq!(&big[..6], b"456789");
        assert_eq!(unsafe { f(&s, big.as_mut_ptr().cast(), 16) }, 0);
    }
}
