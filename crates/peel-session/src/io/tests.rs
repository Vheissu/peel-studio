use std::sync::atomic::Ordering;

use peel_midi_io::MidiEvent;

use super::*;
use crate::template;

fn session() -> Session {
    Session::new_silent(template::beat_and_melody())
}

fn channel(s: &Session, name: &str) -> Id {
    s.project()
        .channels
        .iter()
        .find(|c| c.name == name)
        .expect("channel")
        .id
}

fn all_channels(s: &Session) -> Vec<Id> {
    s.project().channels.iter().map(|c| c.id).collect()
}

/// Runs the session's per-frame upkeep until `cond` holds.
fn wait_for(s: &mut Session, what: &str, mut cond: impl FnMut(&mut Session) -> bool) {
    let start = Instant::now();
    loop {
        s.tick();
        if cond(s) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(3));
    }
}

fn sounds(s: &mut Session, ch: Id) -> bool {
    s.take_channel_activity(ch) > 0.0
}

fn note_on(key: u8, velocity: u8) -> MidiEvent {
    MidiEvent::NoteOn {
        channel: 0,
        key,
        velocity,
    }
}

fn note_off(key: u8) -> MidiEvent {
    MidiEvent::NoteOff {
        channel: 0,
        key,
        velocity: 0,
    }
}

#[test]
fn restart_reinstalls_every_device_and_resends_the_plan() {
    let mut s = session();
    let channels = all_channels(&s);
    let before = s.devices.len();
    assert!(before >= channels.len());
    assert_eq!(s.buffer_frames(), 256);

    // Before: playing a note on any instrument sounds.
    for &ch in &channels {
        s.live_note(ch, 60, 1.0, true);
        wait_for(&mut s, "a note before the restart", |s| sounds(s, ch));
        s.live_note(ch, 60, 0.0, false);
    }

    s.restart_audio(None, Some(128)).expect("restart");
    assert_eq!(s.buffer_frames(), 128);
    assert_eq!(s.audio_device_name(), "No audio device");
    assert_eq!(s.devices.len(), before, "no slots were lost or invented");
    assert!(!s.is_playing());

    // The plan arrived with the devices: the starter beat has a kick on the
    // first beat, and the master hears it.
    s.play();
    wait_for(&mut s, "the plan to play", |s| {
        s.telemetry().take_master().0 > 0.0
    });
    wait_for(&mut s, "the playhead to move", |s| s.position() > 0.25);
    s.stop();

    // And every instrument is installed again.
    for &ch in &channels {
        s.live_note(ch, 60, 1.0, true);
        wait_for(&mut s, "a note after the restart", |s| sounds(s, ch));
        s.live_note(ch, 60, 0.0, false);
    }
    let t = s.telemetry();
    assert_eq!(t.plans_rejected.load(Ordering::Relaxed), 0);
    assert_eq!(t.retire_leaks.load(Ordering::Relaxed), 0);
}

#[test]
fn restart_keeps_the_playhead_and_a_changed_sample_rate_works() {
    let mut s = session();
    s.seek(6.0);
    wait_for(&mut s, "the seek", |s| (s.position() - 6.0).abs() < 1e-6);
    s.set_metronome(true);

    let wanted = EngineConfig {
        sample_rate: Some(44_100),
        buffer_frames: Some(512),
        ..s.io.config.clone()
    };
    s.restart_with(wanted).expect("restart");
    wait_for(&mut s, "the playhead to come back", |s| {
        (s.position() - 6.0).abs() < 1e-6
    });
    assert_eq!(s.sample_rate(), 44_100.0);
    assert_eq!(s.buffer_frames(), 512);
    assert!(s.metronome());

    // Devices were prepared for the new rate and work.
    let piano = channel(&s, "Piano");
    s.live_note(piano, 60, 1.0, true);
    wait_for(&mut s, "a note at 44.1 kHz", |s| sounds(s, piano));
}

#[test]
fn an_empty_device_name_means_the_default_and_the_request_is_remembered() {
    let mut s = session();
    s.restart_audio(Some("  ".into()), None).unwrap();
    assert_eq!(s.requested_audio_device(), None);
    s.restart_audio(Some("Some Interface".into()), Some(64))
        .unwrap();
    // The silent clock has no devices; the request is still recorded.
    assert_eq!(s.requested_audio_device(), Some("Some Interface"));
    assert_eq!(s.buffer_frames(), 64);
}

#[test]
fn midi_notes_play_the_selected_channel_without_the_ui() {
    let mut s = session();
    let piano = channel(&s, "Piano");
    let kick = channel(&s, "Kick");

    // No target: ignored.
    s.io.bridge.handle(&note_on(60, 100));
    assert_eq!(s.midi_activity(), 0);

    s.set_live_target(Some(piano));
    assert_eq!(s.live_target(), Some(piano));
    s.io.bridge.handle(&note_on(60, 100));
    wait_for(&mut s, "the piano to sound", |s| sounds(s, piano));
    assert!(!sounds(&mut s, kick), "only the selected channel plays");
    assert_eq!(s.midi_activity(), 1);

    // The note is recorded with its channel and velocity.
    s.poll_midi();
    let notes = s.take_recorded_midi();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].channel, Some(piano));
    assert!(notes[0].on && notes[0].key == 60);
    assert!((notes[0].velocity - 100.0 / 127.0).abs() < 1e-6);
    assert!(!notes[0].playing && !notes[0].sustain);
    assert!(s.take_recorded_midi().is_empty());

    // Switching channels while holding the key: the release still ends the
    // piano's note, and the next note goes to the new channel.
    s.set_live_target(Some(kick));
    s.io.bridge.handle(&note_off(60));
    s.io.bridge.handle(&note_on(62, 90));
    wait_for(&mut s, "the kick to sound", |s| sounds(s, kick));

    s.set_live_target(None);
    s.io.bridge.handle(&note_on(64, 90));
    assert_eq!(s.midi_activity(), 2, "no target, no note");
}

#[test]
fn the_pedal_and_all_notes_off_reach_the_engine() {
    let mut s = session();
    let piano = channel(&s, "Piano");
    s.set_live_target(Some(piano));
    let initial = s.engine.live_stats().events();
    s.io.bridge.handle(&MidiEvent::Sustain {
        channel: 0,
        down: true,
    });
    s.io.bridge.handle(&note_on(60, 100));
    s.io.bridge.handle(&note_off(60));
    wait_for(&mut s, "the piano to sound", |s| sounds(s, piano));
    assert_eq!(s.engine.live_stats().events(), initial + 3);

    s.io.bridge.handle(&MidiEvent::AllNotesOff { channel: 0 });
    assert_eq!(s.engine.live_stats().events(), initial + 4);
    let notes = s.take_recorded_midi();
    assert_eq!(notes.len(), 2);
    assert!(notes[1].sustain, "the release was recorded under the pedal");
}

#[test]
fn midi_keeps_working_after_an_audio_restart() {
    let mut s = session();
    let piano = channel(&s, "Piano");
    s.set_live_target(Some(piano));
    s.io.bridge.handle(&note_on(60, 100));
    wait_for(&mut s, "the piano to sound", |s| sounds(s, piano));

    s.restart_audio(None, Some(128)).unwrap();
    // The target moved to the new engine.
    assert_eq!(s.live_target(), Some(piano));
    s.io.bridge.handle(&note_on(64, 100));
    wait_for(&mut s, "the piano to sound again", |s| sounds(s, piano));

    // Notes from before the restart were kept for recording.
    let notes = s.take_recorded_midi();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!((notes[0].key, notes[1].key), (60, 64));
}

#[test]
fn spread_timing_still_plays_every_note_promptly() {
    let mut s = session();
    let piano = channel(&s, "Piano");
    s.set_live_target(Some(piano));
    s.set_midi_timing(LiveTiming::Spread);
    // Notes arrive at arbitrary points in the audio callbacks; each is heard
    // within a couple of blocks and none is lost.
    for key in 40..60u8 {
        s.io.bridge.handle(&note_on(key, 100));
        wait_for(&mut s, "the note to sound", |s| sounds(s, piano));
        s.io.bridge.handle(&note_off(key));
        std::thread::sleep(Duration::from_millis(u64::from(key) % 7));
    }
    assert_eq!(s.midi_activity(), 20);
    assert_eq!(s.engine.live_stats().dropped(), 0);
}

#[test]
fn silent_sessions_do_not_open_hardware_and_polling_is_harmless() {
    let mut s = session();
    assert!(!s.midi_enabled());
    assert!(s.midi_inputs().is_empty());
    for _ in 0..3 {
        s.poll_midi();
    }
    assert_eq!(s.midi_message_count(), 0);
    s.select_midi_input(PortSelection::Nothing);
    s.set_midi_enabled(true);
    assert!(s.midi_enabled());
    s.poll_midi();
    s.set_midi_enabled(false);
    assert!(s.midi_inputs().is_empty());
    s.set_midi_timing(LiveTiming::Spread);
    assert_eq!(s.midi_timing(), LiveTiming::Spread);
}

#[test]
#[ignore = "needs an audio output device"]
fn restarting_on_the_real_device_keeps_the_sound() {
    let mut s = Session::new(template::beat_and_melody());
    if s.engine.is_null_backend() {
        return;
    }
    let piano = channel(&s, "Piano");
    s.restart_audio(None, Some(512)).expect("restart");
    assert!(!s.engine.is_null_backend());
    s.live_note(piano, 60, 1.0, true);
    wait_for(&mut s, "the piano to sound", |s| sounds(s, piano));
    // An unknown device falls back to the default and says so.
    s.restart_audio(Some("No Such Interface".into()), None)
        .expect("fallback");
    assert!(s.audio_warning().is_some());
    assert!(!s.engine.is_null_backend());

    // MIDI notes reach the device callback in both timing modes.
    s.set_live_target(Some(piano));
    for timing in [LiveTiming::Immediate, LiveTiming::Spread] {
        s.set_midi_timing(timing);
        for key in 50..56u8 {
            s.io.bridge.handle(&note_on(key, 110));
            wait_for(&mut s, "a MIDI note on the device", |s| sounds(s, piano));
            s.io.bridge.handle(&note_off(key));
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    assert_eq!(s.engine.live_stats().dropped(), 0);
    assert_eq!(
        s.telemetry().overruns.load(Ordering::Relaxed),
        0,
        "no missed deadlines"
    );
}

/// A virtual MIDI keyboard stands in for hardware: its bytes travel through
/// the real system MIDI layer, the port thread, the bridge and the engine's
/// live queue to the instrument, and the time is printed.
#[cfg(unix)]
#[test]
#[ignore = "needs the system MIDI service (CoreMIDI or the ALSA sequencer)"]
fn a_midi_keyboard_plays_the_selected_channel_through_the_session() {
    use midir::os::unix::VirtualOutput;

    let name = "Peel Session Test Keyboard";
    let mut keyboard = midir::MidiOutput::new("Peel session test")
        .expect("MIDI output")
        .create_virtual(name)
        .unwrap_or_else(|_| panic!("could not create a virtual port"));

    let mut s = session();
    let piano = channel(&s, "Piano");
    s.set_midi_enabled(true);
    s.set_live_target(Some(piano));
    wait_for(&mut s, "the keyboard to connect", |s| {
        s.poll_midi();
        s.midi_inputs().iter().any(|n| n.contains(name))
    });
    assert!(s.notice().is_some_and(|n| n.text.contains("connected")));

    let sent = Instant::now();
    keyboard.send(&[0x90, 60, 100]).unwrap();
    wait_for(&mut s, "the piano to sound", |s| sounds(s, piano));
    eprintln!(
        "key press to sound on the silent clock: {:?}",
        sent.elapsed()
    );
    assert_eq!(s.midi_activity(), 1);

    keyboard.send(&[0xB0, 64, 127]).unwrap();
    keyboard.send(&[0x80, 60, 0]).unwrap();
    wait_for(&mut s, "the pedal and release", |s| {
        s.engine.live_stats().events() >= 3
    });
    let notes = s.take_recorded_midi();
    assert_eq!(notes.len(), 2);
    assert!(!notes[1].on && notes[1].sustain);

    // Unplugging is noticed.
    drop(keyboard);
    wait_for(&mut s, "the keyboard to disconnect", |s| {
        s.poll_midi();
        !s.midi_inputs().iter().any(|n| n.contains(name))
    });
}

#[test]
fn expression_accepts_hardware_and_software_with_one_filter_and_readback() {
    let mut s = session();
    assert!(!s.pitch_bend(1.0));
    assert!(!s.channel_pressure(0.5));
    assert!(!s.reset_expression());
    s.set_pitch_bend_range(12.5).unwrap();
    assert_eq!(s.pitch_bend_range(), 12.5);
    let piano = channel(&s, "Piano");
    s.set_live_target(Some(piano));
    s.set_midi_channel_filter(Some(2)).unwrap();
    s.handle_midi_event(MidiEvent::PitchBend {
        channel: 1,
        value: 8191,
    });
    assert_eq!(s.pitch_bend_value(), 0.0);
    s.handle_midi_event(MidiEvent::PitchBend {
        channel: 2,
        value: -8192,
    });
    assert_eq!(s.pitch_bend_value(), -1.0);
    s.io.bridge.hardware_enabled.store(true, Ordering::Release);
    s.io.bridge.handle_hardware(&MidiEvent::PitchBend {
        channel: 2,
        value: 8191,
    });
    assert_eq!(s.pitch_bend_value(), 1.0);
    s.io.bridge.handle_hardware(&MidiEvent::ChannelPressure {
        channel: 2,
        value: 127,
    });
    s.handle_midi_event(MidiEvent::PolyPressure {
        channel: 2,
        key: 60,
        value: 64,
    });
    assert_eq!(s.channel_pressure_value(), 1.0);
    assert_eq!(s.poly_pressure_value(60), 64.0 / 127.0);
    s.handle_midi_event(MidiEvent::NoteOn {
        channel: 2,
        key: 60,
        velocity: 100,
    });
    assert_eq!(s.poly_pressure_value(60), 0.0);
    assert_eq!(s.pitch_bend_value(), 1.0);
    assert_eq!(s.channel_pressure_value(), 1.0);
    assert!(s.poly_pressure(60, 0.75));
    s.live_note(piano, 60, 1.0, true);
    assert_eq!(s.poly_pressure_value(60), 0.0);
    assert!(!s.pitch_bend(f32::NAN));
    assert!(!s.channel_pressure(f32::INFINITY));
    assert!(!s.poly_pressure(128, 0.5));
    assert!(s.set_pitch_bend_range(49.0).is_err());
    assert!(s.set_pitch_bend_range(f32::NAN).is_err());
    assert_eq!(s.pitch_bend_range(), 12.5);
    assert!(s.reset_expression());
    assert_eq!(s.pitch_bend_value(), 0.0);
    assert_eq!(s.channel_pressure_value(), 0.0);
    assert_eq!(s.poly_pressure_value(60), 0.0);
    assert_eq!(s.pitch_bend_range(), 12.5);
    assert!(s.pitch_bend(0.5));
    s.set_midi_channel_filter(None).unwrap();
    assert_eq!(s.pitch_bend_value(), 0.0);
    assert!(s.channel_pressure(0.8));
    s.panic();
    assert_eq!(s.channel_pressure_value(), 0.0);
    assert!(s.pitch_bend(-0.5));
    s.set_live_target(None);
    assert_eq!(s.pitch_bend_value(), 0.0);
    s.set_live_target(Some(piano));
    assert!(s.pitch_bend(0.4));
    s.restart_audio(None, Some(128)).unwrap();
    assert_eq!(s.pitch_bend_value(), 0.0);
    assert_eq!(s.pitch_bend_range(), 12.5);
}
