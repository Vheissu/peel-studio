//! Proves the realtime rule: after `prepare`, nothing on the audio path
//! allocates or frees memory.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use peel_dsp::{DeviceEvent, EventKind, ProcessContext};

struct Counting;

static EVENTS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
}

fn note() {
    if TRACK.with(Cell::get) {
        EVENTS.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

#[test]
fn audio_path_never_allocates() {
    for id in REQUIRED {
        let mut dev = fresh(id);
        let desc = dev.descriptor();
        // Everything that will be fed during the tracked section is built
        // up front, because building it allocates.
        let mut l = vec![0.0f32; 256];
        let mut r = vec![0.0f32; 256];
        let mut events: Vec<DeviceEvent> = Vec::new();
        for i in 0..24u32 {
            events.push(DeviceEvent {
                offset: i * 9,
                kind: EventKind::NoteOn {
                    key: 36 + (i * 5 % 50) as u8,
                    velocity: 0.3 + 0.03 * i as f32,
                    voice: i,
                },
            });
        }
        for (i, p) in desc.params.iter().enumerate() {
            events.push(DeviceEvent {
                offset: 100 + i as u32,
                kind: EventKind::Param {
                    id: p.id,
                    value: p.from_normalized(0.8),
                },
            });
        }
        // Expression moves while notes sound must not allocate either.
        for (i, kind) in [
            EventKind::PitchBend { semitones: 3.0 },
            EventKind::PitchBendRange { semitones: 12.0 },
            EventKind::ChannelPressure { value: 0.8 },
            EventKind::PolyPressure {
                key: 41,
                value: 1.0,
            },
            EventKind::PitchBend { semitones: -48.0 },
            EventKind::PitchBend { semitones: 0.0 },
            EventKind::ResetExpression,
            EventKind::PitchBend { semitones: 5.0 },
            EventKind::PolyPressure {
                key: 56,
                value: 0.5,
            },
        ]
        .into_iter()
        .enumerate()
        {
            events.push(DeviceEvent {
                offset: 150 + i as u32,
                kind,
            });
        }
        // Per-voice snapshots, including more voices than the table holds
        // (eviction) and a snapshot arriving before its note.
        for i in 0..80u32 {
            events.push(DeviceEvent {
                offset: 160 + i,
                kind: EventKind::VoiceExpression {
                    voice: i,
                    pitch: -120.0 + 3.0 * i as f32,
                    pressure: (i % 5) as f32 * 0.25,
                    timbre: (i % 3) as f32 * 0.5,
                },
            });
        }
        events.push(DeviceEvent {
            offset: 245,
            kind: EventKind::NoteOn {
                key: 60,
                velocity: 0.7,
                voice: 77,
            },
        });
        events.push(DeviceEvent {
            offset: 200,
            kind: EventKind::NoteOff {
                key: 36,
                velocity: 0.0,
                voice: 0,
            },
        });
        events.sort_by_key(|e| e.offset);
        let tail = [DeviceEvent {
            offset: 10,
            kind: EventKind::AllNotesOff,
        }];
        let silence = [DeviceEvent {
            offset: 0,
            kind: EventKind::AllSoundOff,
        }];
        let ctx = ProcessContext::new(SR);

        // Warm up once outside the tracked region.
        dev.process(&ctx, &[], &mut l, &mut r);

        EVENTS.store(0, Ordering::Relaxed);
        TRACK.with(|t| t.set(true));
        dev.process(&ctx, &events, &mut l, &mut r);
        for _ in 0..30 {
            dev.process(&ctx, &[], &mut l, &mut r);
        }
        for p in desc.params {
            dev.set_param(p.id, p.from_normalized(0.3));
            let _ = dev.param(p.id);
        }
        dev.process(&ctx, &tail, &mut l, &mut r);
        let _ = dev.is_silent();
        dev.process(&ctx, &events, &mut l, &mut r);
        dev.process(&ctx, &silence, &mut l, &mut r);
        dev.reset();
        let swapped = dev.set_sample(None);
        TRACK.with(|t| t.set(false));
        drop(swapped);

        let count = EVENTS.load(Ordering::Relaxed);
        assert_eq!(count, 0, "{id}: {count} allocator calls on the audio path");
    }
}
