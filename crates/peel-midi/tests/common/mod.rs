#![allow(dead_code)]

use midly::num::{u4, u7, u14, u15, u24, u28};
use midly::{
    Format, Fps, Header, MetaMessage, MidiMessage, PitchBend, Smf, Timing, TrackEvent,
    TrackEventKind,
};

/// A track written with absolute ticks; deltas are computed on `build`.
#[derive(Default)]
pub struct TrackBuilder {
    events: Vec<(u64, TrackEventKind<'static>)>,
}

impl TrackBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn raw(mut self, tick: u64, kind: TrackEventKind<'static>) -> Self {
        self.events.push((tick, kind));
        self
    }

    fn midi(self, tick: u64, ch: u8, message: MidiMessage) -> Self {
        self.raw(
            tick,
            TrackEventKind::Midi {
                channel: u4::new(ch),
                message,
            },
        )
    }

    pub fn on(self, tick: u64, ch: u8, key: u8, vel: u8) -> Self {
        self.midi(
            tick,
            ch,
            MidiMessage::NoteOn {
                key: u7::new(key),
                vel: u7::new(vel),
            },
        )
    }

    pub fn off(self, tick: u64, ch: u8, key: u8) -> Self {
        self.off_vel(tick, ch, key, 64)
    }

    pub fn off_vel(self, tick: u64, ch: u8, key: u8, vel: u8) -> Self {
        self.midi(
            tick,
            ch,
            MidiMessage::NoteOff {
                key: u7::new(key),
                vel: u7::new(vel),
            },
        )
    }

    /// A note-on and its note-off.
    pub fn note(self, start: u64, end: u64, ch: u8, key: u8, vel: u8) -> Self {
        self.on(start, ch, key, vel).off(end, ch, key)
    }

    pub fn cc(self, tick: u64, ch: u8, controller: u8, value: u8) -> Self {
        self.midi(
            tick,
            ch,
            MidiMessage::Controller {
                controller: u7::new(controller),
                value: u7::new(value),
            },
        )
    }

    pub fn program(self, tick: u64, ch: u8, program: u8) -> Self {
        self.midi(
            tick,
            ch,
            MidiMessage::ProgramChange {
                program: u7::new(program),
            },
        )
    }

    pub fn bend(self, tick: u64, ch: u8, value: i16) -> Self {
        self.midi(
            tick,
            ch,
            MidiMessage::PitchBend {
                bend: PitchBend(u14::new((value + 8192) as u16)),
            },
        )
    }

    pub fn aftertouch(self, tick: u64, ch: u8) -> Self {
        self.midi(
            tick,
            ch,
            MidiMessage::ChannelAftertouch { vel: u7::new(10) },
        )
    }

    pub fn tempo(self, tick: u64, micros: u32) -> Self {
        self.raw(
            tick,
            TrackEventKind::Meta(MetaMessage::Tempo(u24::new(micros))),
        )
    }

    pub fn signature(self, tick: u64, numerator: u8, log2_denominator: u8) -> Self {
        self.raw(
            tick,
            TrackEventKind::Meta(MetaMessage::TimeSignature(
                numerator,
                log2_denominator,
                24,
                8,
            )),
        )
    }

    pub fn name(self, tick: u64, name: &'static str) -> Self {
        self.raw(
            tick,
            TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
        )
    }

    pub fn meta(self, tick: u64, meta: MetaMessage<'static>) -> Self {
        self.raw(tick, TrackEventKind::Meta(meta))
    }

    pub fn sysex(self, tick: u64) -> Self {
        self.raw(tick, TrackEventKind::SysEx(&[0x7E, 0x7F, 0x09, 0x01, 0xF7]))
    }

    /// Sorted by tick (stable), closed with an end-of-track at `end`
    /// (or the last event).
    pub fn build(mut self, end: Option<u64>) -> Vec<TrackEvent<'static>> {
        self.events.sort_by_key(|e| e.0);
        let last = self.events.last().map_or(0, |e| e.0);
        self.events.push((
            end.unwrap_or(last),
            TrackEventKind::Meta(MetaMessage::EndOfTrack),
        ));
        let mut prev = 0u64;
        self.events
            .into_iter()
            .map(|(tick, kind)| {
                let delta = u28::new((tick - prev) as u32);
                prev = tick;
                TrackEvent { delta, kind }
            })
            .collect()
    }
}

pub fn smf(format: Format, ppq: u16, tracks: Vec<Vec<TrackEvent<'static>>>) -> Vec<u8> {
    let smf = Smf {
        header: Header::new(format, Timing::Metrical(u15::new(ppq))),
        tracks,
    };
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes).unwrap();
    bytes
}

pub fn smf_smpte(fps: Fps, subframe: u8, tracks: Vec<Vec<TrackEvent<'static>>>) -> Vec<u8> {
    let smf = Smf {
        header: Header::new(Format::Parallel, Timing::Timecode(fps, subframe)),
        tracks,
    };
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes).unwrap();
    bytes
}
