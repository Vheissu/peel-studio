//! Hardware MIDI input for Peel Studio.
//!
//! [`MidiInput`] opens the system's MIDI input ports (Core MIDI on macOS,
//! WinMM on Windows, ALSA on Linux, through `midir`), parses MIDI 1.0 channel
//! messages into [`MidiEvent`]s and hands them to a handler **on the port's
//! own callback thread**. The handler is where a musician's key press goes
//! straight to the audio engine without waiting for a UI frame; this crate
//! knows nothing about the engine.
//!
//! ```no_run
//! use peel_midi_io::{MidiEvent, MidiInput};
//!
//! let mut midi = MidiInput::new(|event| {
//!     if let MidiEvent::NoteOn { key, velocity, .. } = event {
//!         println!("note {key} at {velocity}");
//!     }
//! });
//! // Every couple of seconds from the UI thread:
//! midi.poll_ports();
//! ```
//!
//! # Platform notes
//!
//! * macOS: USB keyboards need no permission. Bluetooth MIDI devices must be
//!   paired in Audio MIDI Setup first.
//! * Windows: WinMM lets one program open a device at a time, so a keyboard
//!   another application holds fails to open and is retried with a growing
//!   delay. Port names are truncated to 31 characters and often generic.
//! * Linux: needs the ALSA sequencer (`/dev/snd/seq`). Without it there are
//!   simply no ports. "Midi Through" is listed like any other port.

mod input;
mod message;

pub use input::{MidiHandler, MidiInput, MidiPortInfo, PortSelection, list_inputs};
pub use message::{MidiEvent, Parser};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn listing_and_opening_ports_works_without_hardware() {
        // Whatever the machine has (often nothing), nothing panics and the
        // open ports are a subset of the ports seen.
        let listed = list_inputs();
        let midi = MidiInput::new(|_| {});
        midi.flush();
        let seen = midi.ports();
        eprintln!("listed {listed:?}, opened {:?}", midi.connected());
        for port in &seen {
            assert!(!port.id.is_empty());
        }
        for port in midi.connected() {
            assert!(seen.contains(&port));
        }
        // A second scan finds nothing new.
        midi.poll_ports(); // takes the change flag left by the first scan
        midi.flush();
        assert!(!midi.poll_ports());
        midi.close_all();
        midi.flush();
        assert!(midi.connected().is_empty());
    }

    #[test]
    fn selecting_nothing_opens_nothing() {
        let midi = MidiInput::with_selection(|_| {}, PortSelection::Nothing);
        midi.flush();
        assert!(midi.connected().is_empty());
        midi.select(PortSelection::Only("no such port".into()));
        midi.flush();
        assert!(midi.connected().is_empty());
        assert_eq!(midi.selection(), PortSelection::Only("no such port".into()));
        assert_eq!(midi.event_count(), 0);
    }

    fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let start = Instant::now();
        while !cond() {
            assert!(
                start.elapsed() < Duration::from_secs(8),
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A virtual keyboard (a MIDI source made by this test) stands in for
    /// hardware: it must be found by a scan, its bytes must reach the handler
    /// through the real system MIDI path, and it must be dropped when it goes
    /// away.
    #[cfg(unix)]
    #[test]
    #[ignore = "needs the system MIDI service (CoreMIDI or the ALSA sequencer)"]
    fn a_virtual_keyboard_is_found_delivers_notes_and_is_dropped_when_it_leaves() {
        use midir::os::unix::VirtualOutput;
        use std::sync::Mutex;

        let name = "Peel Test Keyboard";
        let mut keyboard = midir::MidiOutput::new("Peel test")
            .expect("MIDI output")
            .create_virtual(name)
            .unwrap_or_else(|_| panic!("could not create a virtual port"));

        let received: Arc<Mutex<Vec<MidiEvent>>> = Arc::default();
        let sink = received.clone();
        let midi = MidiInput::new(move |event| sink.lock().unwrap().push(*event));
        let ours = |midi: &MidiInput| midi.connected().iter().any(|p| p.name.contains(name));
        wait_until("the virtual keyboard to connect", || {
            midi.poll_ports();
            ours(&midi)
        });

        // Running status (second note) and a velocity-0 note-off go through
        // the system layer as the keyboard sent them.
        keyboard.send(&[0x90, 60, 100]).unwrap();
        keyboard.send(&[0xB0, 64, 127]).unwrap();
        keyboard.send(&[0x90, 60, 0]).unwrap();
        wait_until("three events", || received.lock().unwrap().len() >= 3);
        assert_eq!(
            received.lock().unwrap()[..3],
            [
                MidiEvent::NoteOn {
                    channel: 0,
                    key: 60,
                    velocity: 100
                },
                MidiEvent::Sustain {
                    channel: 0,
                    down: true
                },
                MidiEvent::NoteOff {
                    channel: 0,
                    key: 60,
                    velocity: 0
                },
            ]
        );
        assert!(midi.event_count() >= 3);

        // Unplugging closes the connection at the next scan.
        drop(keyboard);
        wait_until("the virtual keyboard to disconnect", || {
            midi.poll_ports();
            !ours(&midi)
        });
    }

    /// Plug in a MIDI keyboard and play a note within 30 seconds.
    #[test]
    #[ignore = "needs a MIDI keyboard"]
    fn a_connected_keyboard_delivers_notes() {
        let notes = Arc::new(AtomicUsize::new(0));
        let counter = notes.clone();
        let midi = MidiInput::new(move |event| {
            if matches!(event, MidiEvent::NoteOn { .. }) {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        });
        midi.flush();
        eprintln!("connected: {:?}", midi.connected());
        assert!(!midi.connected().is_empty(), "no MIDI input found");
        let start = Instant::now();
        while notes.load(Ordering::Relaxed) == 0 {
            assert!(start.elapsed() < Duration::from_secs(30), "no note played");
            std::thread::sleep(Duration::from_millis(20));
            midi.poll_ports();
        }
    }
}
