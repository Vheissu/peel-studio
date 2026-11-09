//! MIDI 1.0 channel message parsing.

/// A channel message the application cares about. Channels are 0..=15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiEvent {
    /// A key went down. `velocity` is 1..=127 (a note-on with velocity 0 is
    /// a [`NoteOff`](MidiEvent::NoteOff), as the MIDI specification says).
    NoteOn { channel: u8, key: u8, velocity: u8 },
    /// A key came up. `velocity` is the release velocity (0 for the
    /// note-on-with-velocity-0 form).
    NoteOff { channel: u8, key: u8, velocity: u8 },
    /// Sustain pedal (CC64); down at a value of 64 or more.
    Sustain { channel: u8, down: bool },
    /// Modulation wheel (CC1, most significant byte).
    ModWheel { channel: u8, value: u8 },
    /// Pitch wheel, -8192 (full down) to 8191 (full up), 0 at rest.
    PitchBend { channel: u8, value: i16 },
    /// Channel aftertouch, 0..=127.
    ChannelPressure { channel: u8, value: u8 },
    /// Polyphonic key pressure, 0..=127.
    PolyPressure { channel: u8, key: u8, value: u8 },
    /// Release every note on the channel (CC123).
    AllNotesOff { channel: u8 },
    /// Silence every note on the channel at once (CC120).
    AllSoundOff { channel: u8 },
    /// Any other controller.
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
}

impl MidiEvent {
    pub fn channel(&self) -> u8 {
        match *self {
            MidiEvent::NoteOn { channel, .. }
            | MidiEvent::NoteOff { channel, .. }
            | MidiEvent::Sustain { channel, .. }
            | MidiEvent::ModWheel { channel, .. }
            | MidiEvent::PitchBend { channel, .. }
            | MidiEvent::ChannelPressure { channel, .. }
            | MidiEvent::PolyPressure { channel, .. }
            | MidiEvent::AllNotesOff { channel }
            | MidiEvent::AllSoundOff { channel }
            | MidiEvent::ControlChange { channel, .. } => channel,
        }
    }
}

/// Turns raw MIDI bytes into [`MidiEvent`]s, one parser per input port.
///
/// The operating-system MIDI layers midir wraps deliver whole messages, but
/// the parser also accepts several messages in one buffer and running status
/// (data bytes that repeat the previous channel status), so it does not
/// depend on that. System real-time bytes (clock, active sensing) are
/// skipped wherever they appear and never disturb running status; system
/// exclusive and other system common messages are skipped and cancel it.
/// Truncated or malformed messages are dropped.
#[derive(Clone, Copy, Debug, Default)]
pub struct Parser {
    running: Option<u8>,
}

impl Parser {
    pub fn new() -> Parser {
        Parser::default()
    }

    /// Forgets running status (a port was reopened).
    pub fn reset(&mut self) {
        self.running = None;
    }

    /// Parses every message in `bytes`, calling `f` for each recognized
    /// channel message in order. Never allocates.
    pub fn parse(&mut self, bytes: &[u8], mut f: impl FnMut(MidiEvent)) {
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            let status = if b & 0x80 != 0 {
                i += 1;
                match b {
                    // Real-time: no data, no effect on running status.
                    0xF8..=0xFF => continue,
                    // System common: cancels running status.
                    0xF0..=0xF7 => {
                        self.running = None;
                        i = skip_system_common(bytes, i, b);
                        continue;
                    }
                    _ => {
                        self.running = Some(b);
                        b
                    }
                }
            } else {
                match self.running {
                    Some(status) => status,
                    // Stray data byte with no status to apply it to.
                    None => {
                        i += 1;
                        continue;
                    }
                }
            };

            let want = data_len(status);
            let mut data = [0u8; 2];
            let mut got = 0;
            while got < want && i < bytes.len() {
                let d = bytes[i];
                if d >= 0xF8 {
                    // Real-time bytes may be interleaved with data.
                    i += 1;
                } else if d & 0x80 != 0 {
                    break;
                } else {
                    data[got] = d;
                    got += 1;
                    i += 1;
                }
            }
            if got < want {
                // Truncated, or cut short by the next status byte.
                continue;
            }
            if let Some(event) = decode(status, data[0], data[1]) {
                f(event);
            }
        }
    }
}

/// Data bytes that follow a channel status byte.
fn data_len(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

/// Index after a system common message that starts at `bytes[i - 1]`.
fn skip_system_common(bytes: &[u8], mut i: usize, status: u8) -> usize {
    let skip_data = |i: usize, n: usize| {
        let mut j = i;
        let mut left = n;
        while left > 0 && j < bytes.len() && bytes[j] & 0x80 == 0 {
            j += 1;
            left -= 1;
        }
        j
    };
    match status {
        // System exclusive: up to and including the terminator.
        0xF0 => {
            while i < bytes.len() {
                let b = bytes[i];
                if b == 0xF7 {
                    return i + 1;
                }
                if b & 0x80 != 0 && b < 0xF8 {
                    // A new status byte ends the message without a terminator.
                    return i;
                }
                i += 1;
            }
            i
        }
        0xF1 | 0xF3 => skip_data(i, 1),
        0xF2 => skip_data(i, 2),
        _ => i,
    }
}

fn decode(status: u8, d0: u8, d1: u8) -> Option<MidiEvent> {
    let channel = status & 0x0F;
    Some(match status & 0xF0 {
        0x80 => MidiEvent::NoteOff {
            channel,
            key: d0,
            velocity: d1,
        },
        0x90 if d1 == 0 => MidiEvent::NoteOff {
            channel,
            key: d0,
            velocity: 0,
        },
        0x90 => MidiEvent::NoteOn {
            channel,
            key: d0,
            velocity: d1,
        },
        0xA0 => MidiEvent::PolyPressure {
            channel,
            key: d0,
            value: d1,
        },
        0xD0 => MidiEvent::ChannelPressure { channel, value: d0 },
        0xB0 => match d0 {
            64 => MidiEvent::Sustain {
                channel,
                down: d1 >= 64,
            },
            1 => MidiEvent::ModWheel { channel, value: d1 },
            120 => MidiEvent::AllSoundOff { channel },
            123 => MidiEvent::AllNotesOff { channel },
            controller => MidiEvent::ControlChange {
                channel,
                controller,
                value: d1,
            },
        },
        0xE0 => MidiEvent::PitchBend {
            channel,
            value: ((d1 as i16) << 7 | d0 as i16) - 8192,
        },
        // Program changes are not used.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(parser: &mut Parser, bytes: &[u8]) -> Vec<MidiEvent> {
        let mut out = Vec::new();
        parser.parse(bytes, |e| out.push(e));
        out
    }

    fn parse(bytes: &[u8]) -> Vec<MidiEvent> {
        parse_all(&mut Parser::new(), bytes)
    }

    #[test]
    fn note_on_and_off() {
        assert_eq!(
            parse(&[0x90, 60, 100]),
            vec![MidiEvent::NoteOn {
                channel: 0,
                key: 60,
                velocity: 100
            }]
        );
        assert_eq!(
            parse(&[0x83, 72, 40]),
            vec![MidiEvent::NoteOff {
                channel: 3,
                key: 72,
                velocity: 40
            }]
        );
        assert_eq!(parse(&[0x9F, 0, 127])[0].channel(), 15);
    }

    #[test]
    fn note_on_with_velocity_zero_is_a_note_off() {
        assert_eq!(
            parse(&[0x91, 64, 0]),
            vec![MidiEvent::NoteOff {
                channel: 1,
                key: 64,
                velocity: 0
            }]
        );
    }

    #[test]
    fn sustain_pedal_switches_at_64() {
        let down = |v| parse(&[0xB0, 64, v]);
        assert_eq!(
            down(127),
            vec![MidiEvent::Sustain {
                channel: 0,
                down: true
            }]
        );
        assert_eq!(
            down(64),
            vec![MidiEvent::Sustain {
                channel: 0,
                down: true
            }]
        );
        assert_eq!(
            down(63),
            vec![MidiEvent::Sustain {
                channel: 0,
                down: false
            }]
        );
        assert_eq!(
            down(0),
            vec![MidiEvent::Sustain {
                channel: 0,
                down: false
            }]
        );
    }

    #[test]
    fn channel_mode_and_other_controllers() {
        assert_eq!(
            parse(&[0xB2, 123, 0]),
            vec![MidiEvent::AllNotesOff { channel: 2 }]
        );
        assert_eq!(
            parse(&[0xB2, 120, 0]),
            vec![MidiEvent::AllSoundOff { channel: 2 }]
        );
        assert_eq!(
            parse(&[0xB0, 1, 99]),
            vec![MidiEvent::ModWheel {
                channel: 0,
                value: 99
            }]
        );
        assert_eq!(
            parse(&[0xB0, 7, 100]),
            vec![MidiEvent::ControlChange {
                channel: 0,
                controller: 7,
                value: 100
            }]
        );
    }

    #[test]
    fn pitch_bend_is_centered_on_zero() {
        let bend = |lsb, msb| match parse(&[0xE0, lsb, msb])[0] {
            MidiEvent::PitchBend { value, .. } => value,
            other => panic!("{other:?}"),
        };
        assert_eq!(bend(0x00, 0x40), 0);
        assert_eq!(bend(0x00, 0x00), -8192);
        assert_eq!(bend(0x7F, 0x7F), 8191);
        assert_eq!(bend(0x01, 0x40), 1);
    }

    #[test]
    fn unused_channel_messages_are_ignored() {
        assert!(parse(&[0xC0, 5]).is_empty(), "program change");
    }

    #[test]
    fn running_status_applies_the_last_status_to_bare_data() {
        let mut p = Parser::new();
        let events = parse_all(&mut p, &[0x90, 60, 100, 62, 90, 64, 0]);
        assert_eq!(
            events,
            vec![
                MidiEvent::NoteOn {
                    channel: 0,
                    key: 60,
                    velocity: 100
                },
                MidiEvent::NoteOn {
                    channel: 0,
                    key: 62,
                    velocity: 90
                },
                MidiEvent::NoteOff {
                    channel: 0,
                    key: 64,
                    velocity: 0
                },
            ]
        );
        // Running status carries over to the next buffer.
        let next = parse_all(&mut p, &[65, 80]);
        assert_eq!(
            next,
            vec![MidiEvent::NoteOn {
                channel: 0,
                key: 65,
                velocity: 80
            }]
        );
    }

    #[test]
    fn running_status_works_for_one_data_byte_messages() {
        let mut p = Parser::new();
        assert!(parse_all(&mut p, &[0xC1, 3, 4, 5]).is_empty());
        // And a new status replaces it.
        assert_eq!(parse_all(&mut p, &[0xB0, 64, 127, 64, 0]).len(), 2);
    }

    #[test]
    fn bare_data_without_a_status_is_dropped() {
        assert!(parse(&[60, 100]).is_empty());
        let mut p = Parser::new();
        parse_all(&mut p, &[0x90, 60, 100]);
        p.reset();
        assert!(parse_all(&mut p, &[61, 100]).is_empty());
    }

    #[test]
    fn real_time_bytes_do_not_disturb_messages_or_running_status() {
        let mut p = Parser::new();
        let events = parse_all(&mut p, &[0xF8, 0x90, 60, 0xF8, 100, 0xFE, 62, 90]);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(
            events[0],
            MidiEvent::NoteOn {
                channel: 0,
                key: 60,
                velocity: 100
            }
        );
    }

    #[test]
    fn system_common_and_sysex_cancel_running_status_and_are_skipped() {
        let mut p = Parser::new();
        parse_all(&mut p, &[0x90, 60, 100]);
        let events = parse_all(&mut p, &[0xF0, 0x7E, 0x00, 0x09, 0x01, 0xF7, 61, 100]);
        assert!(events.is_empty(), "no status after sysex: {events:?}");
        // A message following a sysex in the same buffer still parses.
        let events = parse_all(&mut p, &[0xF0, 1, 2, 3, 0xF7, 0x90, 60, 100]);
        assert_eq!(events.len(), 1);
        // Song position (two data bytes) and time code (one) are skipped.
        let events = parse_all(&mut p, &[0xF2, 1, 2, 0xF1, 5, 0x90, 60, 100]);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn unterminated_sysex_ends_at_the_next_status_byte() {
        let events = parse(&[0xF0, 1, 2, 3, 0x90, 60, 100]);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn several_messages_in_one_buffer() {
        let events = parse(&[0x90, 60, 100, 0x80, 60, 0, 0xB0, 64, 127]);
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn truncated_and_malformed_messages_are_dropped() {
        assert!(parse(&[0x90]).is_empty());
        assert!(parse(&[0x90, 60]).is_empty());
        assert!(parse(&[]).is_empty());
        // A status byte inside the data cuts the message short; the new
        // status is parsed normally.
        let events = parse(&[0x90, 60, 0x91, 61, 80]);
        assert_eq!(
            events,
            vec![MidiEvent::NoteOn {
                channel: 1,
                key: 61,
                velocity: 80
            }]
        );
    }

    #[test]
    fn every_two_byte_input_is_handled_without_panicking() {
        let mut p = Parser::new();
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                parse_all(&mut p, &[a, b]);
                parse_all(&mut p, &[a, b, 0x40]);
            }
        }
    }
    #[test]
    fn aftertouch_preserves_channels_running_status_and_realtime_interleaving() {
        assert_eq!(
            parse(&[0xD5, 0, 64, 0xF8, 127, 0xA3, 60, 30, 61, 0xFE, 100]),
            vec![
                MidiEvent::ChannelPressure {
                    channel: 5,
                    value: 0
                },
                MidiEvent::ChannelPressure {
                    channel: 5,
                    value: 64
                },
                MidiEvent::ChannelPressure {
                    channel: 5,
                    value: 127
                },
                MidiEvent::PolyPressure {
                    channel: 3,
                    key: 60,
                    value: 30
                },
                MidiEvent::PolyPressure {
                    channel: 3,
                    key: 61,
                    value: 100
                },
            ]
        );
        assert!(parse(&[0xA0, 60]).is_empty());
        assert!(parse(&[0xD0]).is_empty());
    }
}
