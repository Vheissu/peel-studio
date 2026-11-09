//! Translation checks against the fixture's native/MIDI note ports.
#[path = "../tests/fixture/mod.rs"]
mod fixture;

use super::*;
use clap_sys::events::CLAP_EVENT_NOTE_EXPRESSION;

fn device(id: &str) -> ClapDevice {
    ClapDevice::load(&fixture::info(id), fixture::SR, fixture::BLOCK).unwrap()
}
fn push(dev: &mut ClapDevice, offset: u32, kind: EventKind) {
    dev.push_event(&DeviceEvent { offset, kind }, 0, fixture::BLOCK);
}
fn on(dev: &mut ClapDevice, key: u8, voice: u32) {
    push(
        dev,
        10,
        EventKind::NoteOn {
            key,
            voice,
            velocity: 1.0,
        },
    );
}

#[test]
fn native_expression_scopes_active_and_future_notes_without_parameter_edits() {
    let mut dev = device(fixture::SINE);
    let initial = dev.param(100);
    on(&mut dev, 60, 44);
    push(&mut dev, 17, EventKind::PitchBend { semitones: 7.5 });
    push(&mut dev, 23, EventKind::ChannelPressure { value: 0.4 });
    push(
        &mut dev,
        31,
        EventKind::PolyPressure {
            key: 60,
            value: 0.8,
        },
    );
    for (i, time, expression, value) in [
        (1, 17, CLAP_NOTE_EXPRESSION_TUNING, 7.5),
        (2, 23, CLAP_NOTE_EXPRESSION_PRESSURE, f64::from(0.4f32)),
        (
            3,
            31,
            CLAP_NOTE_EXPRESSION_PRESSURE,
            f64::from(0.4f32 + 0.8 - 0.4 * 0.8),
        ),
    ] {
        assert_eq!(
            dev.events.header_at(i).unwrap().type_,
            CLAP_EVENT_NOTE_EXPRESSION
        );
        let event = dev.events.expression_at(i).unwrap();
        assert_eq!(event.header.time, time);
        assert_eq!(
            (
                event.expression_id,
                event.note_id,
                event.port_index,
                event.channel,
                event.key
            ),
            (expression, -1, 0, 0, if i == 3 { 60 } else { -1 })
        );
        assert_eq!(event.value, value);
    }
    dev.events.clear();
    on(&mut dev, 64, 45);
    assert_eq!(dev.events.len(), 3);
    assert_eq!(dev.events.expression_at(1).unwrap().note_id, 45);
    assert_eq!(dev.events.expression_at(1).unwrap().value, 7.5);
    assert_eq!(
        dev.events.expression_at(2).unwrap().value,
        f64::from(0.4f32)
    );
    dev.events.clear();
    assert!(dev.push_expression(47, EventKind::ResetExpression));
    assert_eq!(dev.events.len(), 2);
    for i in 0..2 {
        assert_eq!(dev.events.expression_at(i).unwrap().value, 0.0);
    }
    assert_eq!(dev.voices.items.len(), 2, "reset keeps sounding notes");
    assert_eq!(dev.param(100), initial);
}

#[test]
fn midi_expression_uses_rpn_sensitivity_exact_wheel_endpoints_and_aftertouch() {
    let mut dev = device(fixture::MIDI_SINE);
    assert!(dev.push_expression(7, EventKind::PitchBendRange { semitones: 12.5 }));
    let expected = [
        [0xb0, 101, 0],
        [0xb0, 100, 0],
        [0xb0, 6, 12],
        [0xb0, 38, 50],
        [0xb0, 101, 127],
        [0xb0, 100, 127],
        [0xe0, 0, 64],
    ];
    for (i, data) in expected.into_iter().enumerate() {
        let event = dev.events.midi_at(i).unwrap();
        assert_eq!(
            (event.header.time, event.port_index, event.data),
            (7, 0, data)
        );
    }
    dev.events.clear();
    for (time, kind) in [
        (11, EventKind::PitchBend { semitones: -12.5 }),
        (13, EventKind::PitchBend { semitones: 12.5 }),
        (15, EventKind::ChannelPressure { value: 1.0 }),
        (
            17,
            EventKind::PolyPressure {
                key: 60,
                value: 0.5,
            },
        ),
    ] {
        assert!(dev.push_expression(time, kind));
    }
    for (i, (time, data)) in [
        (11, [0xe0, 0, 0]),
        (13, [0xe0, 127, 127]),
        (15, [0xd0, 127, 0]),
        (17, [0xa0, 60, 64]),
    ]
    .into_iter()
    .enumerate()
    {
        let event = dev.events.midi_at(i).unwrap();
        assert_eq!((event.header.time, event.data), (time, data));
    }
    dev.events.clear();
    assert!(dev.push_expression(19, EventKind::ResetExpression));
    assert_eq!(dev.events.len(), 3);
    assert_eq!(dev.events.midi_at(0).unwrap().data, [0xe0, 0, 64]);
    assert_eq!(dev.events.midi_at(1).unwrap().data, [0xd0, 0, 0]);
    assert_eq!(dev.events.midi_at(2).unwrap().data, [0xa0, 60, 0]);
    assert_eq!(dev.expression.range, 12.5);
}

#[test]
fn midi_rpn_capacity_check_prevents_partial_controller_selection() {
    let mut dev = device(fixture::MIDI_SINE);
    dev.events = EventList::with_capacity(12, 6);
    assert!(!dev.push_expression(0, EventKind::PitchBendRange { semitones: 24.0 }));
    assert_eq!(dev.events.len(), 0);
    assert_eq!(
        dev.expression.range, 24.0,
        "desired range is retained for retry"
    );
    assert!(dev.pending_expression.range);
    // Releases retain the reserved space even after a rejected expression transaction.
    for _ in 0..12 {
        assert!(dev.events.push_midi(0, 0, [0x80, 60, 0], true));
    }
}

#[test]
fn expression_rejects_nonfinite_values_and_reinitializes_midi_after_reset() {
    let mut dev = device(fixture::MIDI_SINE);
    assert!(!dev.push_expression(
        0,
        EventKind::PitchBend {
            semitones: f32::NAN
        }
    ));
    assert!(!dev.push_expression(
        0,
        EventKind::PolyPressure {
            key: 255,
            value: 0.2
        }
    ));
    assert_eq!(dev.events.len(), 0);
    assert!(dev.push_expression(0, EventKind::PitchBendRange { semitones: 9.0 }));
    dev.reset();
    on(&mut dev, 60, 1);
    assert_eq!(dev.events.len(), 8);
    assert_eq!(dev.events.midi_at(2).unwrap().data, [0xb0, 6, 9]);
    assert_eq!(dev.events.midi_at(7).unwrap().data, [0x90, 60, 127]);
}

#[test]
fn midi_new_strike_clears_old_key_pressure_before_note_on() {
    let mut dev = device(fixture::MIDI_SINE);
    assert!(dev.push_expression(0, EventKind::ChannelPressure { value: 0.4 }));
    assert!(dev.push_expression(
        0,
        EventKind::PolyPressure {
            key: 60,
            value: 0.8
        }
    ));
    dev.events.clear();
    on(&mut dev, 60, 1);
    assert_eq!(dev.events.len(), 2);
    assert_eq!(dev.events.midi_at(0).unwrap().data, [0xa0, 60, 0]);
    assert_eq!(dev.events.midi_at(1).unwrap().data, [0x90, 60, 127]);
    assert_eq!(dev.expression.pressure, 0.4);
    assert_eq!(dev.expression.keys[60], None);
}

fn process_empty(dev: &mut ClapDevice) {
    let (mut l, mut r) = ([0.0; fixture::BLOCK], [0.0; fixture::BLOCK]);
    dev.process(&ProcessContext::new(fixture::SR), &[], &mut l, &mut r);
}

#[test]
fn full_event_lists_retry_centers_and_resets_without_using_noteoff_reserve() {
    for id in [fixture::SINE, fixture::MIDI_SINE] {
        let mut dev = device(id);
        assert!(dev.push_expression(0, EventKind::PitchBend { semitones: 7.0 }));
        assert!(dev.push_expression(0, EventKind::ChannelPressure { value: 0.6 }));
        assert!(dev.push_expression(
            0,
            EventKind::PolyPressure {
                key: 60,
                value: 0.8
            }
        ));
        // Keep production capacity, filling its ordinary part with parameters.
        dev.events.clear();
        while dev.events.push_param(0, 100, 0, 0.5, false) {}
        assert!(!dev.push_expression(21, EventKind::PitchBend { semitones: 0.0 }));
        assert!(!dev.push_expression(22, EventKind::ChannelPressure { value: 0.0 }));
        assert!(!dev.push_expression(23, EventKind::ResetExpression));
        assert_eq!(dev.expression.bend, 0.0);
        assert_eq!(dev.expression.pressure, 0.0);
        assert!(dev.pending_expression.bend && dev.pending_expression.pressure);
        dev.sleeping = true;
        assert!(
            !dev.is_silent(),
            "deferred expression must wake a sleeping plugin"
        );
        // Expression did not consume a single reserved release slot.
        for _ in 0..RELEASE_RESERVE {
            assert!(dev.events.push_midi(24, 0, [0x80, 60, 0], true));
        }
        assert!(!dev.events.push_midi(24, 0, [0x80, 60, 0], true));
        process_empty(&mut dev);
        assert!(!dev.pending_expression.bend && !dev.pending_expression.pressure);
        assert_eq!(dev.pending_expression.keys, 0);
        for i in 0..dev.events.len() {
            if dev.dialect == Dialect::Clap {
                let e = dev.events.expression_at(i).unwrap();
                assert_eq!((e.header.time, e.value, e.note_id), (0, 0.0, -1));
            } else {
                let e = dev.events.midi_at(i).unwrap();
                assert_eq!(e.header.time, 0);
                assert!([[0xe0, 0, 64], [0xd0, 0, 0], [0xa0, 60, 0]].contains(&e.data));
            }
        }
    }
}

#[test]
fn native_expression_follows_released_tails_without_host_tail_storage() {
    let mut dev = device(fixture::SINE);
    on(&mut dev, 60, 44);
    push(
        &mut dev,
        20,
        EventKind::NoteOff {
            key: 60,
            velocity: 0.0,
            voice: 44,
        },
    );
    assert!(
        dev.voices.items.is_empty(),
        "only physical holds remain tracked"
    );
    dev.events.clear();
    assert!(dev.push_expression(31, EventKind::PitchBend { semitones: 3.0 }));
    assert!(dev.push_expression(35, EventKind::ChannelPressure { value: 0.4 }));
    assert!(dev.push_expression(
        39,
        EventKind::PolyPressure {
            key: 60,
            value: 0.5
        }
    ));
    for (index, key, expression, value) in [
        (0, -1, CLAP_NOTE_EXPRESSION_TUNING, 3.0),
        (1, -1, CLAP_NOTE_EXPRESSION_PRESSURE, f64::from(0.4f32)),
        (2, 60, CLAP_NOTE_EXPRESSION_PRESSURE, f64::from(0.7f32)),
    ] {
        let e = dev.events.expression_at(index).unwrap();
        assert_eq!(
            (e.note_id, e.port_index, e.channel, e.key, e.expression_id),
            (-1, 0, 0, key, expression)
        );
        assert!((e.value - value).abs() < 1e-7);
    }
    dev.events.clear();
    assert!(dev.push_expression(42, EventKind::ResetExpression));
    assert_eq!(dev.events.len(), 2);
    for i in 0..2 {
        let e = dev.events.expression_at(i).unwrap();
        assert_eq!((e.note_id, e.key, e.value), (-1, -1, 0.0));
    }
}

#[test]
fn implicit_device_reset_replays_desired_expression_for_hardware_readback_consistency() {
    for id in [fixture::SINE, fixture::MIDI_SINE] {
        let mut dev = device(id);
        assert!(dev.push_expression(0, EventKind::PitchBendRange { semitones: 12.0 }));
        assert!(dev.push_expression(0, EventKind::PitchBend { semitones: 6.0 }));
        assert!(dev.push_expression(0, EventKind::ChannelPressure { value: 0.4 }));
        assert!(dev.push_expression(
            0,
            EventKind::PolyPressure {
                key: 60,
                value: 0.5
            }
        ));
        dev.reset();
        assert_eq!(
            (
                dev.expression.bend,
                dev.expression.range,
                dev.expression.pressure
            ),
            (6.0, 12.0, 0.4)
        );
        assert_eq!(dev.expression.keys[60], Some(0.5));
        process_empty(&mut dev);
        assert!(!dev.pending_expression.bend && !dev.pending_expression.pressure);
        assert!(!dev.pending_expression.range);
        assert_eq!(dev.pending_expression.keys, 0);
        if dev.dialect == Dialect::Midi {
            assert_eq!(dev.events.midi_at(2).unwrap().data, [0xb0, 6, 12]);
            assert_eq!(dev.events.midi_at(6).unwrap().data, midi_bend(6.0, 12.0));
            assert!(dev.events.midi_at(8).unwrap().data == [0xd0, 51, 0]);
            assert!(dev.events.midi_at(9).unwrap().data == [0xa0, 60, 64]);
        } else {
            assert_eq!(dev.events.expression_at(0).unwrap().value, 6.0);
            assert_eq!(
                dev.events.expression_at(1).unwrap().value,
                f64::from(0.4f32)
            );
        }
    }
}

#[test]
fn scoped_expression_addresses_same_key_voices_and_release_tails_independently() {
    let mut dev = device(fixture::SINE);
    on(&mut dev, 60, 1001);
    on(&mut dev, 60, 1002);
    dev.events.clear();
    push(
        &mut dev,
        17,
        EventKind::VoiceExpression {
            voice: 1001,
            pitch: 7.0,
            pressure: 0.8,
            timbre: 0.9,
        },
    );
    push(
        &mut dev,
        19,
        EventKind::VoiceExpression {
            voice: 1002,
            pitch: -3.0,
            pressure: 0.2,
            timbre: 0.1,
        },
    );
    assert_eq!(dev.events.len(), 6);
    for (i, id, pitch, pressure, timbre) in
        [(0, 1001, 7.0, 0.8f32, 0.9f32), (3, 1002, -3.0, 0.2, 0.1)]
    {
        for (j, dimension, value) in [
            (0, CLAP_NOTE_EXPRESSION_TUNING, pitch),
            (1, CLAP_NOTE_EXPRESSION_PRESSURE, f64::from(pressure)),
            (
                2,
                clap_sys::events::CLAP_NOTE_EXPRESSION_BRIGHTNESS,
                f64::from(timbre),
            ),
        ] {
            let event = dev.events.expression_at(i + j).unwrap();
            assert_eq!(
                (event.note_id, event.key, event.expression_id),
                (id, 60, dimension)
            );
            assert_eq!(event.value, value);
        }
    }
    push(
        &mut dev,
        21,
        EventKind::NoteOff {
            key: 60,
            velocity: 0.0,
            voice: 1001,
        },
    );
    dev.events.clear();
    push(
        &mut dev,
        23,
        EventKind::VoiceExpression {
            voice: 1001,
            pitch: 4.0,
            pressure: 0.5,
            timbre: 0.5,
        },
    );
    assert_eq!(dev.events.expression_at(0).unwrap().note_id, 1001);
    assert_eq!(dev.events.expression_at(0).unwrap().value, 4.0);
    dev.events.clear();
    assert!(dev.push_expression(30, EventKind::ResetExpression));
    assert_eq!(
        dev.events.len(),
        8,
        "global reset re-applies both scoped voices"
    );
    assert_eq!(dev.events.expression_at(2).unwrap().value, 4.0);
    assert_eq!(dev.events.expression_at(5).unwrap().value, -3.0);
}
#[test]
fn scoped_expression_overflow_keeps_desired_state_and_retries_without_using_release_reserve() {
    let mut dev = device(fixture::SINE);
    on(&mut dev, 60, 77);
    dev.events = EventList::with_capacity(16, 8);
    for _ in 0..8 {
        assert!(dev.events.push_midi(0, 0, [0xb0, 1, 0], false));
    }
    assert!(!dev.push_scoped(17, 77, 12.0, 0.9, 0.1));
    assert!(dev.scoped.entries.iter().any(|e| e.id == 77 && e.dirty));
    for _ in 0..8 {
        assert!(dev.events.push_midi(0, 0, [0x80, 60, 0], true));
    }
    assert_eq!(dev.events.len(), 16);
    process_empty(&mut dev);
    assert!(!dev.scoped.entries.iter().any(|e| e.dirty));
    assert_eq!(dev.events.expression_at(0).unwrap().value, 12.0);
    assert_eq!(
        dev.events.expression_at(1).unwrap().value,
        f64::from(0.9f32)
    );
    dev.events.clear();
    for _ in 0..8 {
        dev.events.push_midi(0, 0, [0xb0, 1, 0], false);
    }
    assert!(!dev.push_scoped(20, 77, 0.0, 0.0, 0.5));
    process_empty(&mut dev);
    assert_eq!(dev.events.expression_at(0).unwrap().value, 0.0);
    assert_eq!(dev.events.expression_at(1).unwrap().value, 0.0);
}
#[test]
fn midi_scoped_notes_get_mpe_members_and_scoped_releases() {
    let mut dev = device(fixture::MIDI_SINE);
    let events = [
        DeviceEvent {
            offset: 0,
            kind: EventKind::NoteOn {
                key: 60,
                velocity: 0.8,
                voice: 101,
            },
        },
        DeviceEvent {
            offset: 0,
            kind: EventKind::VoiceExpression {
                voice: 101,
                pitch: 12.0,
                pressure: 0.8,
                timbre: 0.9,
            },
        },
        DeviceEvent {
            offset: 0,
            kind: EventKind::NoteOn {
                key: 60,
                velocity: 0.7,
                voice: 102,
            },
        },
        DeviceEvent {
            offset: 0,
            kind: EventKind::VoiceExpression {
                voice: 102,
                pitch: -12.0,
                pressure: 0.2,
                timbre: 0.1,
            },
        },
    ];
    let (mut l, mut r) = ([0.0; fixture::BLOCK], [0.0; fixture::BLOCK]);
    dev.process(&ProcessContext::new(fixture::SR), &events, &mut l, &mut r);
    let messages: Vec<_> = (0..dev.events.len())
        .filter_map(|i| dev.events.midi_at(i).map(|e| e.data))
        .collect();
    let ons: Vec<_> = messages.iter().filter(|e| e[0] & 0xf0 == 0x90).collect();
    assert_eq!(ons.len(), 2);
    assert_eq!((ons[0][0] & 15, ons[1][0] & 15), (1, 2));
    assert!(messages.contains(&[0xb0, 100, 6]));
    assert!(messages.contains(&[0xb0, 6, 15]));
    for (channel, pitch, pressure, timbre) in [(1u8, 12.0, 0.8f32, 0.9f32), (2, -12.0, 0.2, 0.1)] {
        let mut wheel = midi_bend(pitch, 96.0);
        wheel[0] |= channel;
        assert!(messages.contains(&wheel));
        assert!(messages.contains(&[0xd0 | channel, (pressure * 127.0).round() as u8, 0]));
        assert!(messages.contains(&[0xb0 | channel, 74, (timbre * 127.0).round() as u8]));
    }
    dev.events.clear();
    push(
        &mut dev,
        10,
        EventKind::NoteOff {
            key: 60,
            velocity: 0.0,
            voice: 101,
        },
    );
    assert_eq!(dev.events.midi_at(0).unwrap().data, [0x81, 60, 0]);
    dev.events.clear();
    push(
        &mut dev,
        10,
        EventKind::VoiceExpression {
            voice: 102,
            pitch: 6.0,
            pressure: 0.5,
            timbre: 0.7,
        },
    );
    assert_eq!(
        dev.events.midi_at(0).unwrap().data[0],
        0xe2,
        "other note still owns its member"
    );
}
#[test]
fn midi_mpe_setup_is_atomic_and_note_capacity_is_bounded() {
    let mut dev = device(fixture::MIDI_SINE);
    dev.scoped.midi_enabled = true;
    dev.events = EventList::with_capacity(20, 12);
    on(&mut dev, 60, 1);
    assert_eq!(dev.events.len(), 0, "failed setup emits no partial RPN");
    dev.events = EventList::with_capacity(2048, 640);
    for i in 0..17 {
        on(&mut dev, 60, 100 + i);
    }
    let ons = (0..dev.events.len())
        .filter_map(|i| dev.events.midi_at(i))
        .filter(|e| e.data[0] & 0xf0 == 0x90)
        .count();
    assert_eq!(ons, 15);
    dev.events.clear();
    dev.release_all(0, true);
    let ccs: Vec<_> = (0..dev.events.len())
        .filter_map(|i| dev.events.midi_at(i))
        .map(|e| e.data)
        .collect();
    for ch in 1..=15 {
        assert!(ccs.contains(&[0xb0 | ch, 120, 0]), "panic channel{ch}");
    }
}

#[test]
fn all_sound_off_clears_global_and_scoped_expression_before_new_notes() {
    for id in [fixture::SINE, fixture::MIDI_SINE] {
        let mut dev = device(id);
        on(&mut dev, 60, 51);
        assert!(dev.push_expression(0, EventKind::PitchBend { semitones: 7.0 }));
        assert!(dev.push_expression(0, EventKind::ChannelPressure { value: 0.8 }));
        assert!(dev.push_scoped(0, 51, 12.0, 0.9, 0.7));
        push(&mut dev, 0, EventKind::AllSoundOff);
        assert_eq!(dev.expression.bend, 0.0);
        assert_eq!(dev.expression.pressure, 0.0);
        assert!(dev.scoped.entries.is_empty());
        process_empty(&mut dev);
        dev.events.clear();
        on(&mut dev, 60, 52);
        assert!(
            dev.scoped
                .entries
                .iter()
                .all(|e| e.pitch == 0.0 && e.pressure == 0.0 && e.timbre == 0.5)
        );
    }
}

#[test]
fn midi_mpe_only_note_dialect_is_recognized_and_legacy_releases_survive_mode_entry() {
    let port = NotePortInfo {
        supported_dialects: CLAP_NOTE_DIALECT_MIDI_MPE,
    };
    assert_eq!(pick_dialect(&[port]), Dialect::Midi);
    let mut dev = device(fixture::MIDI_SINE);
    on(&mut dev, 60, 100);
    dev.scoped.midi_enabled = true;
    dev.events.clear();
    push(
        &mut dev,
        0,
        EventKind::NoteOff {
            key: 60,
            velocity: 0.0,
            voice: 100,
        },
    );
    assert_eq!(
        dev.events.midi_at(0).unwrap().data,
        [0x80, 60, 0],
        "pre-MPE notes still release on channel zero"
    );
}

#[test]
fn midi_all_notes_off_releases_member_allocation_for_the_next_phrase() {
    let mut dev = device(fixture::MIDI_SINE);
    dev.scoped.midi_enabled = true;
    for voice in 1..=15 {
        on(&mut dev, 60, voice);
    }
    dev.release_all(0, false);
    dev.events.clear();
    on(&mut dev, 60, 1000);
    assert!(
        (0..dev.events.len())
            .filter_map(|i| dev.events.midi_at(i))
            .any(|e| e.data[0] & 0xf0 == 0x90)
    );
}

#[test]
fn clearing_a_scoped_override_resumes_shared_expression_without_retriggering() {
    let mut dev = device(fixture::SINE);
    on(&mut dev, 60, 500);
    assert!(dev.push_expression(0, EventKind::PitchBend { semitones: 2.0 }));
    assert!(dev.push_expression(0, EventKind::ChannelPressure { value: 0.3 }));
    assert!(dev.push_expression(
        0,
        EventKind::PolyPressure {
            key: 60,
            value: 0.5
        }
    ));
    assert!(dev.push_scoped(0, 500, 12.0, 0.9, 0.8));
    dev.events.clear();
    push(&mut dev, 17, EventKind::ClearVoiceExpression { voice: 500 });
    assert_eq!(dev.events.len(), 3);
    assert_eq!(dev.voices.items.len(), 1);
    assert_eq!(dev.events.expression_at(0).unwrap().value, 2.0);
    assert_eq!(
        dev.events.expression_at(1).unwrap().value,
        f64::from(0.3f32 + 0.5 - 0.3 * 0.5)
    );
    assert_eq!(dev.events.expression_at(2).unwrap().value, 0.5);
    assert!(
        dev.scoped
            .entries
            .iter()
            .all(|e| e.id != 500 || !e.explicit)
    );
    dev.events.clear();
    assert!(dev.push_expression(20, EventKind::PitchBend { semitones: 5.0 }));
    assert_eq!(
        dev.events.len(),
        1,
        "shared updates no longer reapply the old scoped override"
    );
    assert_eq!(dev.events.expression_at(0).unwrap().value, 5.0);
}
#[test]
fn a_deferred_clear_uses_the_latest_shared_values_and_keeps_release_capacity() {
    let mut dev = device(fixture::SINE);
    on(&mut dev, 60, 501);
    assert!(dev.push_scoped(0, 501, 12.0, 1.0, 1.0));
    dev.events = EventList::with_capacity(16, 8);
    for _ in 0..8 {
        dev.events.push_midi(0, 0, [0xb0, 1, 0], false);
    }
    assert!(!dev.clear_scoped(17, 501));
    assert!(!dev.push_expression(18, EventKind::PitchBend { semitones: -3.0 }));
    assert!(!dev.push_expression(19, EventKind::ChannelPressure { value: 0.25 }));
    for _ in 0..8 {
        assert!(dev.events.push_midi(20, 0, [0x80, 60, 0], true));
    }
    process_empty(&mut dev);
    let tuning: Vec<_> = (0..dev.events.len())
        .filter_map(|i| dev.events.expression_at(i))
        .filter(|e| e.expression_id == CLAP_NOTE_EXPRESSION_TUNING)
        .collect();
    assert!(tuning.iter().any(|e| e.note_id == 501 && e.value == -3.0));
    assert!(
        dev.scoped
            .entries
            .iter()
            .all(|e| e.id != 501 || (!e.explicit && !e.dirty))
    );
}
#[test]
fn clearing_midi_expression_keeps_the_member_and_follows_shared_controls() {
    let mut dev = device(fixture::MIDI_SINE);
    dev.scoped.midi_enabled = true;
    on(&mut dev, 60, 502);
    assert!(dev.push_scoped(0, 502, 24.0, 0.9, 0.1));
    assert!(dev.push_expression(0, EventKind::PitchBend { semitones: 3.0 }));
    assert!(dev.push_expression(0, EventKind::ChannelPressure { value: 0.25 }));
    dev.events.clear();
    push(&mut dev, 17, EventKind::ClearVoiceExpression { voice: 502 });
    assert_eq!(dev.events.len(), 3);
    let mut expected = midi_bend(3.0, 96.0);
    expected[0] = 0xe1;
    assert_eq!(dev.events.midi_at(0).unwrap().data, expected);
    assert_eq!(dev.events.midi_at(1).unwrap().data, [0xd1, 32, 0]);
    assert_eq!(dev.events.midi_at(2).unwrap().data, [0xb1, 74, 64]);
    dev.events.clear();
    assert!(dev.push_expression(19, EventKind::PitchBend { semitones: -6.0 }));
    let mut expected = midi_bend(-6.0, 96.0);
    expected[0] = 0xe1;
    assert_eq!(dev.events.midi_at(0).unwrap().data, expected);
    dev.events.clear();
    push(
        &mut dev,
        20,
        EventKind::NoteOff {
            key: 60,
            velocity: 0.0,
            voice: 502,
        },
    );
    assert_eq!(dev.events.midi_at(0).unwrap().data, [0x81, 60, 0]);
}
