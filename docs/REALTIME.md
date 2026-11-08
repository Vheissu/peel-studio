# Realtime rules

These rules apply to every function reachable from the audio callback: the
engine's block loop, `Device::process`, `Device::set_param`, `Device::reset`,
`Device::set_sample` and anything they call. They implement ENG-RT-001 to
ENG-RT-010 of the specification.

## Never on the audio thread

- Heap allocation or deallocation. This includes `Vec::push` past capacity,
  `String`/`format!`, `Box::new`, `collect()`, `clone()` of owning types,
  and dropping the last `Arc`/`Box` of anything.
- Locks (`Mutex`, `RwLock`), condition variables, channels that can block.
- Filesystem, network, logging macros, `println!`.
- Loading libraries, spawning threads, calling the UI.
- Waiting on another thread.

## How work crosses the boundary

- Control to engine: `EngineCommand` through a bounded `rtrb` SPSC queue.
- Live performance input (MIDI keyboards): `LiveInput` through a second
  bounded `rtrb` ring that only MIDI callback threads write (they share the
  producer behind a mutex the audio thread never takes). The engine drains at
  most 512 events per block, applies the sustain pedal from fixed arrays and
  reports each note to a third ring for recording. Note-ons stop being
  accepted before the ring is full, so note-offs always fit.
- Audio input: the input stream's callback runs `InputProcessor::process`
  under the same rules as the output callback. It writes into two bounded
  SPSC rings (a capture ring that a writer thread drains to disk, and a
  monitor FIFO that the output callback reads), updates atomics (peak, take
  counters, timing) and never allocates, locks or logs. A full ring drops
  frames and counts them; an empty monitor FIFO plays silence and counts an
  underrun. The control side connects the FIFO to the engine with a
  command, so replacing it retires the old one through the retire queue.
- Engine to control: `Retired` objects (old plans, devices, samples) through a
  second bounded queue. The control thread drops them.
- Telemetry: plain atomics in `Telemetry` (`Relaxed`; peaks are a running
  max that the UI swaps to zero on read).

## Preparing resources

All buffers, voice pools, delay lines and event storage are sized in
`Device::prepare` or when a `RenderPlan` is built on the control thread.
A buffer with spare capacity is not enough on its own: every write path
checks capacity and has a defined overflow behavior. For events, note-ons
are dropped before note-offs; note-offs, transport stops and resets are
never dropped.

## Numerical hygiene

- Flush denormals (`peel_dsp::util::sanitize`) in feedback paths.
- Contain non-finite output from a device at the channel/track boundary and
  count it in `Telemetry::non_finite`.
- No automatic hard clipping inside the mixer. Clipping and saturation are
  explicit effect choices.
- No panics in DSP code paths: no `unwrap()` on values that can fail, no
  slice indexing that can be out of bounds for legal parameter ranges.

## Checking

`cargo test -p peel-engine --features rt-check` runs the live, input and
engine paths under the same check (`tests/rt_alloc*.rs`).

Debug builds of `peel-app` can be run with `--features rt-check`, which
installs `assert_no_alloc` around the callback and reports any allocation
made on the audio thread.
