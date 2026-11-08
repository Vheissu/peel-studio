//! Verifies docs/REALTIME.md for effects: after `prepare`, `process`,
//! `set_param`, `reset` and the query methods never allocate or free.
//!
//! A counting global allocator tallies allocations made on the *current
//! thread* while armed, so tests running in parallel do not disturb each
//! other.

mod common;

use common::*;
use peel_dsp::{DeviceEvent, EventKind, ProcessContext};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static EVENTS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

fn note_event() {
    // `try_with` so allocations during thread teardown cannot panic.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = EVENTS.try_with(|n| n.set(n.get() + 1));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_event();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_event();
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_event();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_event();
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `f` and returns how many allocator calls it made.
fn count_allocations(f: impl FnOnce()) -> usize {
    EVENTS.with(|n| n.set(0));
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    EVENTS.with(Cell::get)
}

#[test]
fn audio_thread_calls_never_touch_the_heap() {
    for id in all_type_ids() {
        let desc = peel_effects::descriptors()
            .iter()
            .find(|d| d.type_id == id)
            .unwrap();
        for preset in peel_effects::presets(id) {
            let mut d = make(id, Some(preset), SR, 256);
            let (src_l, src_r) = program(256 * 40, 77);
            let mut l = src_l.clone();
            let mut r = src_r.clone();
            let mut noise = Noise::new(5);

            // Build the event lists up front; allocating them is control-side
            // work and must not be counted.
            let mut blocks: Vec<Vec<DeviceEvent>> = Vec::new();
            for _ in 0..40 {
                let mut evs = Vec::new();
                for k in 0..desc.params.len().min(6) {
                    let p = &desc.params[(noise.next().abs() * 997.0) as usize % desc.params.len()];
                    evs.push(DeviceEvent {
                        offset: (k * 30) as u32,
                        kind: EventKind::Param {
                            id: p.id,
                            value: p.from_normalized(noise.next().abs()),
                        },
                    });
                }
                blocks.push(evs);
            }
            let ctx = ProcessContext::new(SR);
            // Warm up once so lazily initialized state (none expected) is
            // outside the measured region.
            d.process(&ctx, &[], &mut l[..256], &mut r[..256]);

            let allocations = count_allocations(|| {
                for (i, evs) in blocks.iter().enumerate() {
                    let range = i * 256..(i + 1) * 256;
                    d.process(&ctx, evs, &mut l[range.clone()], &mut r[range]);
                }
                d.set_param(desc.params[0].id, desc.params[0].default);
                let _ = d.param(desc.params[0].id);
                let _ = d.latency();
                let _ = d.tail_seconds();
                let _ = d.is_silent();
                d.reset();
                d.process(&ctx, &[], &mut l[..256], &mut r[..256]);
            });
            assert_eq!(
                allocations, 0,
                "{id}/{}: {allocations} heap calls on the audio thread",
                preset.name
            );
        }
    }
}

#[test]
fn counting_allocator_actually_counts() {
    let n = count_allocations(|| {
        let v: Vec<u8> = vec![1, 2, 3];
        std::hint::black_box(&v);
    });
    assert!(n >= 2, "expected an allocation and a free, saw {n}");
}
