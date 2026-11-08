//! The audio-thread surface of `ClapDevice` must not allocate.
//!
//! This binary installs a global allocator that counts allocations,
//! frees and reallocations made by the *current thread* while armed. The
//! plugin library has its own allocator and is not counted; the fixture
//! plugins allocate nothing in `process` anyway.

mod fixture;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use fixture::*;
use peel_dsp::{Device, DeviceEvent, EventKind, ProcessContext};
use peel_host_clap::ClapDevice;

struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

fn note() {
    // `try_with`: thread-local teardown can allocate on exit paths.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    });
}

// SAFETY: forwards to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Runs `f` and returns how many allocator calls this thread made in it.
fn count_allocations(f: impl FnOnce()) -> u64 {
    COUNT.set(0);
    ARMED.set(true);
    f();
    ARMED.set(false);
    COUNT.get()
}

fn at(offset: u32, kind: EventKind) -> DeviceEvent {
    DeviceEvent { offset, kind }
}

fn busy_events(n: u32, frames: u32) -> Vec<DeviceEvent> {
    let mut events = Vec::new();
    for i in 0..n {
        let offset = i * frames / n.max(1);
        events.push(at(
            offset,
            EventKind::NoteOn {
                key: 40 + (i % 40) as u8,
                velocity: 0.8,
                voice: i,
            },
        ));
        events.push(at(
            offset,
            EventKind::Param {
                id: 100,
                value: (i % 10) as f32 / 10.0,
            },
        ));
        events.push(at(
            offset + 1,
            EventKind::NoteOff {
                key: 40 + (i % 40) as u8,
                velocity: 0.0,
                voice: i,
            },
        ));
    }
    events.sort_by_key(|e| e.offset);
    events
}

#[test]
fn counting_allocator_sees_allocations() {
    // Guards the harness: an allocation inside the closure must be counted.
    let n = count_allocations(|| {
        let v = vec![1u8; 128];
        std::hint::black_box(&v);
    });
    assert!(n >= 2, "counted {n}");
}

#[test]
fn instrument_process_and_friends_do_not_allocate() {
    for id in [SINE, MIDI_SINE] {
        let mut dev = ClapDevice::load(&info(id), SR, BLOCK).unwrap();
        let ctx = ProcessContext::new(SR);
        let (mut l, mut r) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
        let events = busy_events(60, BLOCK as u32);
        let panic = [
            at(10, EventKind::AllNotesOff),
            at(20, EventKind::AllSoundOff),
        ];
        // A warm-up block, so lazy one-time work is not blamed on the audio
        // path: starting the plugin, the first thread-local use.
        dev.process(&ctx, &[], &mut l, &mut r);

        let allocations = count_allocations(|| {
            for block in 0..200 {
                dev.set_param(100, (block % 7) as f32 / 7.0);
                dev.set_param(100, 0.3);
                dev.process(&ctx, &events, &mut l, &mut r);
                dev.process(&ctx, &panic, &mut l, &mut r);
                dev.process(&ctx, &[], &mut l, &mut r);
                std::hint::black_box((dev.is_silent(), dev.param(100), dev.latency()));
                std::hint::black_box(dev.tail_seconds());
                dev.reset();
            }
        });
        assert_eq!(allocations, 0, "{id}: allocator calls on the audio path");
        assert_eq!(dev.stats().process_errors, 0);
    }
}

#[test]
fn effect_process_does_not_allocate() {
    for id in [GAIN, MONO_GAIN] {
        let mut dev = ClapDevice::load(&info(id), SR, BLOCK).unwrap();
        let ctx = ProcessContext {
            playing: true,
            beat_position: 3.5,
            tempo_bpm: 133.0,
            ..ProcessContext::new(SR)
        };
        let input = ramp(BLOCK);
        let (mut l, mut r) = (input.clone(), input.clone());
        let events: Vec<DeviceEvent> = (0..100)
            .map(|i| {
                at(
                    i * 5,
                    EventKind::Param {
                        id: 7,
                        value: (i % 4) as f32 * 0.5,
                    },
                )
            })
            .collect();
        dev.process(&ctx, &[], &mut l, &mut r);

        let allocations = count_allocations(|| {
            for _ in 0..200 {
                l.copy_from_slice(&input);
                r.copy_from_slice(&input);
                dev.set_param(7, 0.5);
                dev.set_param(9, 1.0);
                dev.process(&ctx, &events, &mut l, &mut r);
            }
        });
        assert_eq!(allocations, 0, "{id}");
        assert_eq!(dev.stats().process_errors, 0);
    }
}

#[test]
fn event_overflow_and_block_splitting_do_not_allocate() {
    // 20 000 events overflow the per-block list; a block four times the
    // maximum is split into chunks. Neither may allocate, and releases
    // still get through.
    let mut dev = ClapDevice::load(&info(SINE), SR, 128).unwrap();
    let ctx = ProcessContext::new(SR);
    let frames = 512usize;
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let mut events = Vec::new();
    for i in 0..20_000u32 {
        events.push(at(
            i % 400,
            EventKind::NoteOn {
                key: 60,
                velocity: 0.5,
                voice: i,
            },
        ));
    }
    events.push(at(450, EventKind::AllNotesOff));
    events.sort_by_key(|e| e.offset);
    dev.process(&ctx, &[], &mut l, &mut r);

    let allocations = count_allocations(|| {
        dev.process(&ctx, &events, &mut l, &mut r);
        dev.process(&ctx, &[], &mut l, &mut r);
    });
    assert_eq!(allocations, 0);
    assert!(dev.stats().dropped_events > 0, "the overflow was counted");
    assert!(l.iter().all(|x| x.is_finite()));
    let last = &l[460..];
    assert!(
        last.iter().all(|x| *x == 0.0),
        "AllNotesOff got through the overflow"
    );
}

#[test]
fn loading_does_allocate_so_it_belongs_off_the_audio_thread() {
    // The opposite check: if loading stopped allocating, the counter would be
    // broken or the work would have moved onto the audio path.
    let n = count_allocations(|| {
        let dev = ClapDevice::load(&info(GAIN), SR, BLOCK).unwrap();
        std::hint::black_box(&dev);
    });
    assert!(n > 0);
}
