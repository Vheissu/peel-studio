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
