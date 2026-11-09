use super::*;
use crate::template;
use peel_dsp::Taper;
use peel_midi_io::MidiEvent;

fn session() -> (Session, Id) {
    let mut project = template::beat_and_melody();
    let eq = template::device("peel.fx.eq", None);
    let device = eq.id;
    project.mixer.tracks[0].inserts.push(eq);
    (Session::new_silent(project), device)
}

fn target(device: Id) -> MidiControlTarget {
    MidiControlTarget::DeviceParam {
        device,
        key: "band2.freq".into(),
    }
}

fn mapping(device: Id) -> MidiMapping {
    MidiMapping {
        controller: 74,
        channel: Some(2),
        target: target(device),
        min: 20.0,
        max: 20_000.0,
    }
}

fn cc(channel: u8, controller: u8, value: u8) -> MidiEvent {
    MidiEvent::ControlChange {
        channel,
        controller,
        value,
    }
}

fn value(s: &Session, device: Id, key: &str) -> f32 {
    crate::find_device(s.project(), device).unwrap().params[key]
}

#[test]
fn controllers_apply_only_when_polled_and_use_logarithmic_parameter_scaling() {
    let (mut s, device) = session();
    s.set_midi_channel_filter(Some(2)).unwrap();
    s.set_midi_control_mappings(vec![mapping(device)]).unwrap();
    s.handle_midi_event(cc(0, 74, 127));
    s.poll_midi();
    assert_eq!(value(&s, device, "band2.freq"), 400.0);

    s.handle_midi_event(cc(2, 74, 0));
    assert_eq!(value(&s, device, "band2.freq"), 400.0);
    s.poll_midi();
    assert_eq!(value(&s, device, "band2.freq"), 20.0);
    s.handle_midi_event(cc(2, 74, 64));
    s.poll_midi();
    let expected = 20.0 * 1000.0_f32.powf(64.0 / 127.0);
    assert!((value(&s, device, "band2.freq") - expected).abs() < 0.01);
    assert!(matches!(
        peel_devices::descriptor("peel.fx.eq")
            .unwrap()
            .param_by_key("band2.freq")
            .unwrap()
            .taper,
        Taper::Log
    ));
    s.handle_midi_event(cc(2, 74, 127));
    s.poll_midi();
    assert_eq!(value(&s, device, "band2.freq"), 20_000.0);
    assert!(
        !s.midi_enabled(),
        "software controls need no hardware ports"
    );
}

#[test]
fn a_controller_flood_is_bounded_and_one_undo_reverses_the_entire_turn() {
    let (mut s, device) = session();
    s.set_midi_control_mappings(vec![mapping(device)]).unwrap();
    for i in 0..100_000 {
        s.handle_midi_event(cc(2, 74, (i % 128) as u8));
    }
    s.handle_midi_event(cc(2, 74, 0));
    s.poll_midi();
    s.handle_midi_event(cc(2, 74, 127));
    s.poll_midi();
    assert_eq!(value(&s, device, "band2.freq"), 20_000.0);
    assert!(std::mem::size_of::<ControlMailbox>() < 5000);
    s.doc.undo();
    assert_eq!(value(&s, device, "band2.freq"), 400.0);
    assert!(!s.doc.can_undo());
}

#[test]
fn controller_bursts_separated_by_idle_get_separate_undo_entries() {
    let (mut s, device) = session();
    s.set_midi_control_mappings(vec![mapping(device)]).unwrap();
    s.handle_midi_event(cc(2, 74, 0));
    s.poll_midi();
    s.io.controls.last_control = Some(Instant::now() - Duration::from_secs(1));
    s.handle_midi_event(cc(2, 74, 127));
    s.poll_midi();
    s.doc.undo();
    assert_eq!(value(&s, device, "band2.freq"), 20.0);
    s.doc.undo();
    assert_eq!(value(&s, device, "band2.freq"), 400.0);
}

#[test]
fn repeated_unchanged_controller_values_do_not_create_undo_steps() {
    let (mut s, device) = session();
    let mut m = mapping(device);
    m.min = 400.0;
    s.set_midi_control_mappings(vec![m]).unwrap();
    let revision = s.doc.revision();
    s.handle_midi_event(cc(2, 74, 0));
    s.poll_midi();
    assert!(!s.doc.can_undo());
    assert_eq!(s.doc.revision(), revision);
}

#[test]
fn learn_uses_the_first_accepted_cc_and_preserves_sustain_and_panic() {
    let (mut s, device) = session();
    s.set_midi_channel_filter(Some(2)).unwrap();
    s.begin_midi_learn(target(device)).unwrap();
    for event in [
        cc(2, 64, 127),
        cc(2, 120, 127),
        cc(2, 123, 127),
        cc(1, 70, 127),
    ] {
        s.handle_midi_event(event);
    }
    s.poll_midi();
    assert!(matches!(
        s.midi_learn_status(),
        MidiLearnStatus::Waiting { .. }
    ));
    s.handle_midi_event(MidiEvent::ModWheel {
        channel: 2,
        value: 127,
    });
    s.handle_midi_event(cc(2, 7, 0));
    s.poll_midi();
    let MidiLearnStatus::Learned { mapping } = s.midi_learn_status() else {
        panic!("not learned")
    };
    assert_eq!(mapping.controller, 1);
    assert_eq!(mapping.channel, Some(2));
    assert_eq!(mapping.target, target(device));
    assert_eq!(value(&s, device, "band2.freq"), 20_000.0);
    let saved = serde_json::to_string(s.midi_control_mappings()).unwrap();
    let restored = serde_json::from_str::<Vec<MidiMapping>>(&saved).unwrap();
    assert_eq!(restored, s.midi_control_mappings());
    s.cancel_midi_learn();
    assert_eq!(s.midi_learn_status(), &MidiLearnStatus::Idle);
}

#[test]
fn invalid_mapping_updates_are_atomic_and_missing_targets_are_reported_safely() {
    let (mut s, device) = session();
    let valid = mapping(device);
    s.set_midi_control_mappings(vec![valid.clone()]).unwrap();
    for invalid in [
        MidiMapping {
            min: f32::NAN,
            ..valid.clone()
        },
        MidiMapping {
            min: -1.0,
            ..valid.clone()
        },
        MidiMapping {
            max: 21_000.0,
            ..valid.clone()
        },
        MidiMapping {
            controller: 64,
            ..valid.clone()
        },
        MidiMapping {
            channel: Some(16),
            ..valid.clone()
        },
        MidiMapping {
            target: target(Id::new()),
            ..valid.clone()
        },
    ] {
        assert!(s.set_midi_control_mappings(vec![invalid]).is_err());
        assert_eq!(s.midi_control_mappings(), std::slice::from_ref(&valid));
    }
    assert!(s.set_midi_channel_filter(Some(16)).is_err());
    assert_eq!(s.midi_channel_filter(), None);
    s.doc
        .edit("Remove EQ", |p| p.mixer.tracks[0].inserts.clear());
    let revision = s.doc.revision();
    s.handle_midi_event(cc(2, 74, 127));
    s.poll_midi();
    assert_eq!(s.doc.revision(), revision);
    assert!(s.midi_control_error().unwrap().contains("no longer exists"));
}

#[test]
fn gain_and_pan_mappings_clamp_to_safe_document_ranges() {
    let (mut s, _) = session();
    let channel = s.project().channels[0].id;
    let track = s.project().mixer.tracks[0].id;
    let targets = [
        (MidiControlTarget::TrackGain { track }, -60.0, 12.0),
        (MidiControlTarget::TrackPan { track }, -1.0, 1.0),
        (MidiControlTarget::ChannelGain { channel }, -60.0, 12.0),
        (MidiControlTarget::ChannelPan { channel }, -1.0, 1.0),
    ];
    for (i, (target, min, max)) in targets.into_iter().enumerate() {
        s.add_midi_control_mapping(MidiMapping {
            controller: i as u8 + 10,
            channel: None,
            target,
            min,
            max,
        })
        .unwrap();
        s.handle_midi_event(cc(7, i as u8 + 10, 127));
    }
    s.poll_midi();
    assert_eq!(s.project().mixer.track(track).unwrap().gain_db, 12.0);
    assert_eq!(s.project().mixer.track(track).unwrap().pan, 1.0);
    assert_eq!(s.project().channel(channel).unwrap().gain_db, 12.0);
    assert_eq!(s.project().channel(channel).unwrap().pan, 1.0);
    assert!(!s.remove_midi_control_mapping(9));
    assert!(s.remove_midi_control_mapping(0));
    assert_eq!(s.midi_control_mappings().len(), 3);
}

#[test]
fn channel_filter_and_disabling_input_release_notes_and_pedal() {
    let (mut s, _) = session();
    let channel = s.project().channels[0].id;
    s.set_live_target(Some(channel));
    s.set_midi_channel_filter(Some(2)).unwrap();
    let before = s.midi_activity();
    s.handle_midi_event(MidiEvent::NoteOn {
        channel: 1,
        key: 60,
        velocity: 100,
    });
    assert_eq!(s.midi_activity(), before);
    s.handle_midi_event(MidiEvent::NoteOn {
        channel: 2,
        key: 60,
        velocity: 100,
    });
    s.handle_midi_event(MidiEvent::Sustain {
        channel: 2,
        down: true,
    });
    assert_eq!(s.midi_activity(), before + 1);
    let events = s.engine.live_stats().events();
    s.set_midi_channel_filter(Some(3)).unwrap();
    assert!(
        s.engine.live_stats().events() >= events + 2,
        "pedal-up and panic were queued"
    );
    s.handle_midi_event(MidiEvent::NoteOn {
        channel: 3,
        key: 62,
        velocity: 100,
    });
    let events = s.engine.live_stats().events();
    s.set_midi_enabled(false);
    assert!(s.engine.live_stats().events() >= events + 2);
    s.handle_midi_event(MidiEvent::NoteOff {
        channel: 3,
        key: 62,
        velocity: 0,
    });
    let notes = s.take_recorded_midi();
    assert!(
        notes.iter().filter(|n| n.on).all(|n| !n.sustain),
        "filter changes and disable clear the pedal"
    );
}

#[test]
fn a_software_note_on_with_zero_velocity_is_a_note_off() {
    let (mut s, _) = session();
    let channel = s.project().channels[0].id;
    s.set_live_target(Some(channel));
    s.handle_midi_event(MidiEvent::NoteOn {
        channel: 0,
        key: 60,
        velocity: 100,
    });
    s.handle_midi_event(MidiEvent::NoteOn {
        channel: 0,
        key: 60,
        velocity: 0,
    });
    assert_eq!(s.midi_activity(), 1);
    let notes = s.take_recorded_midi();
    assert_eq!(notes.len(), 2);
    assert!(notes[0].on);
    assert!(!notes[1].on);
}
