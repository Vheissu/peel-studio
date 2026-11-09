//! Standard MIDI File import.
//!
//! The parser walks tracks event by event (rather than collecting the whole
//! file first) so that a malformed track can be kept up to the point where it
//! breaks, with the byte offset reported.
//!
//! Timing is exact: a tick becomes `Time::try_new(ticks, ppq)`, a rational
//! number of quarter notes, so triplets and odd resolutions lose nothing.
//! Quantizing is opt-in.
//!
//! Nothing is dropped silently. Every event that does not become project
//! data is counted by kind in [`ImportReport::unsupported`], and every repair
//! (zero-length notes, notes without a note-off, overlapping notes of the
//! same pitch, note-offs without a note-on) is counted in
//! [`ImportReport::repairs`].

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;

use midly::{Format, MetaMessage, MidiMessage, Timing, TrackEvent, TrackEventKind};
use peel_document::{
    Arrangement, Channel, Clip, ClipContent, DEFAULT_ROOT_KEY, DeviceInstance, Id, Marker,
    MixerTrack, Note, PALETTE, Pattern, Project, TempoSegment, Time, TimeSignature, Track,
    validate,
};

use crate::error::{MidiError, Result};
use crate::gm;

/// How notes are distributed over patterns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternLayout {
    /// One pattern per MIDI track (type 2 files always use this).
    PerTrack,
    /// One pattern per MIDI channel, merging tracks. Splits a type 0 file.
    PerMidiChannel,
    /// Everything in a single pattern.
    Combined,
}

/// Optional snapping of imported notes. Off by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantize {
    None,
    /// Snap note starts to the nearest multiple of the grid; lengths keep
    /// their exact value.
    Starts(Time),
    /// Snap starts, and snap lengths too (never shorter than one grid step).
    StartsAndLengths(Time),
}

/// What to do with the sustain pedal (CC 64).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SustainPolicy {
    /// Do not interpret the pedal; count its events in the report.
    Report,
    /// Notes released while the pedal is down sound until it is released (or
    /// the same key is struck again), as on a piano. Pedal events are then
    /// consumed rather than reported.
    ExtendNotes,
}

#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub layout: PatternLayout,
    /// Give each key of the percussion channel its own Peel channel.
    pub split_drums: bool,
    /// Percussion channel, zero-based (9 is MIDI channel 10).
    pub drum_channel: u8,
    /// Device type id and DSP version for melodic channels.
    pub melodic_device: (String, u32),
    /// Device type id, DSP version and channel name for a percussion key.
    pub drum_device_for_key: fn(u8) -> (String, u32, String),
    pub quantize: Quantize,
    pub sustain: SustainPolicy,
    /// Keep what parsed before a malformed event instead of failing.
    pub allow_partial: bool,
    /// Refuse files with more notes than this.
    pub max_notes: usize,
    /// Name of the pattern created by [`PatternLayout::Combined`].
    pub combined_pattern_name: String,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            layout: PatternLayout::PerTrack,
            split_drums: true,
            drum_channel: 9,
            melodic_device: gm::default_melodic_device(),
            drum_device_for_key: gm::default_drum_device,
            quantize: Quantize::None,
            sustain: SustainPolicy::Report,
            allow_partial: true,
            max_notes: 2_000_000,
            combined_pattern_name: "Imported MIDI".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmfFormat {
    /// One multi-channel track.
    Type0,
    /// Simultaneous tracks sharing one tempo map.
    Type1,
    /// Independent sequences, each with its own timeline.
    Type2,
}

/// Where an imported channel came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSource {
    /// Source track; `None` for percussion channels, which are shared by
    /// every track that plays the key.
    pub track: Option<usize>,
    /// Zero-based MIDI channel.
    pub midi_channel: u8,
    /// Percussion key when the channel was split by key.
    pub drum_key: Option<u8>,
    /// First GM program in effect when the channel starts playing.
    pub program: Option<u8>,
    /// Bank select (MSB, or MSB and LSB combined) in effect then.
    pub bank: Option<u16>,
    pub notes: usize,
}

/// A channel created by the import, with its origin.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedChannel {
    /// `channel.track` is [`Id::NIL`] until the import is merged.
    pub channel: Channel,
    pub source: ChannelSource,
}

/// A parsed file, ready to merge into a project.
#[derive(Clone, Debug)]
pub struct MidiImport {
    pub channels: Vec<ImportedChannel>,
    pub patterns: Vec<Pattern>,
    /// Always starts at beat 0.
    pub tempo: Vec<TempoSegment>,
    /// First time signature in the file, or 4/4.
    pub time_signature: TimeSignature,
    /// Marker meta events, in file order.
    pub markers: Vec<(Time, String)>,
    /// Name of a note-free first track (the conductor track of a type 1 file).
    pub title: Option<String>,
    /// Type 2 patterns are independent sequences; merging places them one
    /// after another instead of side by side.
    pub sequential: bool,
    pub report: ImportReport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedEvents {
    pub description: String,
    pub count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repairs {
    /// Note-on and note-off at the same tick; given a length of one tick.
    pub zero_length_notes: usize,
    /// Note-ons never released; ended at the end of their track.
    pub unterminated_notes: usize,
    /// A note struck again before the same pitch was released.
    pub overlapping_same_pitch: usize,
    /// Note-offs with no sounding note.
    pub orphan_note_offs: usize,
    /// Notes moved by [`Quantize`].
    pub notes_quantized: usize,
    /// Tracks cut short by malformed data: (track, byte offset, message).
    pub malformed_tracks: Vec<(usize, usize, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramUse {
    pub program: u8,
    pub name: &'static str,
    pub notes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelReport {
    pub midi_channel: u8,
    pub is_drum: bool,
    pub notes: usize,
    pub key_range: (u8, u8),
    pub velocity_range: (u8, u8),
    pub programs: Vec<ProgramUse>,
    pub bank: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackReport {
    pub index: usize,
    pub name: Option<String>,
    pub notes: usize,
    pub events: usize,
    pub channels: Vec<ChannelReport>,
    /// End of the track (its end-of-track event, or the last event).
    pub end: Time,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeySignatureEvent {
    pub position: Time,
    /// Negative for flats, positive for sharps.
    pub accidentals: i8,
    pub minor: bool,
}

/// Free text from the file kept for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaEntry {
    pub track: usize,
    pub position: Time,
    pub kind: &'static str,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct ImportReport {
    pub format: SmfFormat,
    /// Ticks per quarter note, or `None` for SMPTE timing.
    pub ticks_per_quarter: Option<u16>,
    pub duration: Time,
    pub total_notes: usize,
    pub tracks: Vec<TrackReport>,
    pub tempo_changes: usize,
    pub time_signatures: Vec<(Time, TimeSignature)>,
    pub key_signatures: Vec<KeySignatureEvent>,
    /// Every event kind that did not become project data, with its count.
    pub unsupported: Vec<UnsupportedEvents>,
    pub repairs: Repairs,
    /// Text events, lyrics, cue points and the like (capped).
    pub metadata: Vec<MetaEntry>,
    pub warnings: Vec<String>,
}

const MAX_METADATA: usize = 2000;

impl fmt::Display for ImportReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.format {
            SmfFormat::Type0 => "type 0",
            SmfFormat::Type1 => "type 1",
            SmfFormat::Type2 => "type 2",
        };
        match self.ticks_per_quarter {
            Some(ppq) => writeln!(f, "SMF {kind}, {ppq} ticks per quarter note")?,
            None => writeln!(f, "SMF {kind}, SMPTE timing")?,
        }
        writeln!(
            f,
            "{} notes in {} tracks, {} quarter notes long",
            self.total_notes,
            self.tracks.len(),
            self.duration
        )?;
        for t in &self.tracks {
            writeln!(
                f,
                "  track {}: {} ({} notes, {} events)",
                t.index,
                t.name.as_deref().unwrap_or("unnamed"),
                t.notes,
                t.events
            )?;
            for c in &t.channels {
                let progs: Vec<String> = c
                    .programs
                    .iter()
                    .map(|p| format!("{} {}", p.program, p.name))
                    .collect();
                writeln!(
                    f,
                    "    channel {}{}: {} notes, keys {}-{}, velocity {}-{}{}",
                    c.midi_channel + 1,
                    if c.is_drum { " (percussion)" } else { "" },
                    c.notes,
                    c.key_range.0,
                    c.key_range.1,
                    c.velocity_range.0,
                    c.velocity_range.1,
                    if progs.is_empty() {
                        String::new()
                    } else {
                        format!(", programs: {}", progs.join(", "))
                    }
                )?;
            }
        }
        writeln!(f, "{} tempo changes", self.tempo_changes)?;
        for (pos, ts) in &self.time_signatures {
            writeln!(
                f,
                "  time signature {}/{} at {pos}",
                ts.numerator, ts.denominator
            )?;
        }
        for u in &self.unsupported {
            writeln!(f, "not imported: {} x{}", u.description, u.count)?;
        }
        let r = &self.repairs;
        for (n, what) in [
            (r.zero_length_notes, "zero-length notes lengthened"),
            (
                r.unterminated_notes,
                "notes without a note-off ended at track end",
            ),
            (
                r.overlapping_same_pitch,
                "overlapping same-pitch notes paired in order",
            ),
            (r.orphan_note_offs, "note-offs without a note-on ignored"),
            (r.notes_quantized, "notes quantized"),
        ] {
            if n > 0 {
                writeln!(f, "repaired: {n} {what}")?;
            }
        }
        for (track, offset, msg) in &r.malformed_tracks {
            writeln!(f, "track {track} cut short at byte {offset}: {msg}")?;
        }
        for w in &self.warnings {
            writeln!(f, "warning: {w}")?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------- scanning

/// Converts ticks to musical time.
#[derive(Clone, Copy)]
struct TickClock {
    /// Ticks per quarter note is `num / den`.
    num: i128,
    den: i128,
}

impl TickClock {
    fn time(self, ticks: u64) -> Result<Time> {
        Time::try_new(i128::from(ticks) * self.den, self.num)
            .map_err(|e| MidiError::UnsupportedTiming(format!("tick {ticks} is out of range: {e}")))
    }
}

struct PendingNote {
    start: u64,
    vel: u8,
    program: Option<u8>,
    bank: Option<u16>,
}

struct RawNote {
    chan: u8,
    key: u8,
    start: u64,
    end: u64,
    vel: u8,
    release: Option<u8>,
    program: Option<u8>,
    bank: Option<u16>,
}

struct HeldNote {
    chan: u8,
    key: u8,
    note: PendingNote,
    release: u8,
}

#[derive(Default)]
struct TrackScan {
    name: Option<String>,
    events: usize,
    end_tick: u64,
    notes: Vec<RawNote>,
}

#[derive(Default)]
struct Collected {
    /// (track, tick, microseconds per quarter note)
    tempo: Vec<(usize, u64, u32)>,
    /// (track, tick, numerator, log2 denominator)
    signatures: Vec<(usize, u64, u8, u8)>,
    key_signatures: Vec<(u64, i8, bool)>,
    markers: Vec<(u64, String)>,
    unsupported: BTreeMap<String, u64>,
    metadata: Vec<(usize, u64, &'static str, String)>,
    repairs: Repairs,
    warnings: Vec<String>,
    total_notes: usize,
}

impl Collected {
    fn skip(&mut self, what: impl Into<String>) {
        *self.unsupported.entry(what.into()).or_default() += 1;
    }

    fn text(&mut self, track: usize, tick: u64, kind: &'static str, bytes: &[u8]) {
        self.skip(format!("{kind} (kept in report metadata)"));
        if self.metadata.len() < MAX_METADATA {
            self.metadata.push((
                track,
                tick,
                kind,
                String::from_utf8_lossy(bytes).into_owned(),
            ));
        }
    }
}

fn controller_name(cc: u8) -> String {
    let name = match cc {
        0 => "Bank Select MSB",
        1 => "Modulation",
        2 => "Breath",
        4 => "Foot Controller",
        5 => "Portamento Time",
        6 => "Data Entry MSB",
        7 => "Channel Volume",
        8 => "Balance",
        10 => "Pan",
        11 => "Expression",
        32 => "Bank Select LSB",
        38 => "Data Entry LSB",
        64 => "Sustain Pedal",
        65 => "Portamento",
        66 => "Sostenuto",
        67 => "Soft Pedal",
        71 => "Resonance",
        72 => "Release Time",
        73 => "Attack Time",
        74 => "Brightness",
        91 => "Reverb Send",
        93 => "Chorus Send",
        98 => "NRPN LSB",
        99 => "NRPN MSB",
        100 => "RPN LSB",
        101 => "RPN MSB",
        120 => "All Sound Off",
        121 => "Reset All Controllers",
        123 => "All Notes Off",
        _ => return format!("Control Change {cc}"),
    };
    format!("Control Change {cc} ({name})")
}

struct TrackState {
    pending: HashMap<(u8, u8), VecDeque<PendingNote>>,
    held: Vec<HeldNote>,
    pedal: [bool; 16],
    program: [Option<u8>; 16],
    bank_msb: [Option<u8>; 16],
    bank_lsb: [Option<u8>; 16],
}

impl TrackState {
    fn bank(&self, chan: u8) -> Option<u16> {
        let c = usize::from(chan);
        self.bank_msb[c].map(|msb| (u16::from(msb) << 7) | self.bank_lsb[c].map_or(0, u16::from))
    }
}

fn finish_note(
    scan: &mut TrackScan,
    repairs: &mut Repairs,
    chan: u8,
    key: u8,
    note: PendingNote,
    end: u64,
    release: Option<u8>,
) {
    let end = if end <= note.start {
        repairs.zero_length_notes += 1;
        note.start + 1
    } else {
        end
    };
    scan.notes.push(RawNote {
        chan,
        key,
        start: note.start,
        end,
        vel: note.vel,
        release,
        program: note.program,
        bank: note.bank,
    });
}

/// Reads one track's events into notes and global events.
fn scan_track(
    index: usize,
    mut events: midly::EventIter<'_>,
    raw_len: usize,
    opts: &ImportOptions,
    ctx: &mut Collected,
) -> Result<TrackScan> {
    let mut scan = TrackScan::default();
    let mut st = TrackState {
        pending: HashMap::new(),
        held: Vec::new(),
        pedal: [false; 16],
        program: [None; 16],
        bank_msb: [None; 16],
        bank_lsb: [None; 16],
    };
    let mut tick: u64 = 0;
    let mut ended = false;

    loop {
        let before = events.unread().len();
        let next = events.next();
        // midly stops quietly at an event it cannot parse; it can be told
        // apart from a clean end of data because bytes were left unread.
        let broken = |message: String, ctx: &mut Collected| -> Result<()> {
            let offset = raw_len.saturating_sub(before);
            if !opts.allow_partial {
                return Err(MidiError::Malformed {
                    track: index,
                    offset,
                    message,
                });
            }
            ctx.repairs.malformed_tracks.push((index, offset, message));
            Ok(())
        };
        let event: TrackEvent<'_> = match next {
            Some(Ok(e)) => e,
            Some(Err(e)) => {
                broken(e.to_string(), ctx)?;
                break;
            }
            None => {
                if before > 0 {
                    broken("truncated or malformed event".to_string(), ctx)?;
                }
                break;
            }
        };
        tick += u64::from(event.delta.as_int());
        scan.events += 1;
        if ended {
            ctx.skip("Events after End of Track");
            continue;
        }
        scan.end_tick = tick;

        match event.kind {
            TrackEventKind::Midi { channel, message } => {
                let chan = channel.as_int();
                let ci = usize::from(chan);
                match message {
                    MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                        let key = key.as_int();
                        ctx.total_notes += 1;
                        if ctx.total_notes > opts.max_notes {
                            return Err(MidiError::TooLarge(format!(
                                "more than {} notes",
                                opts.max_notes
                            )));
                        }
                        if opts.sustain == SustainPolicy::ExtendNotes {
                            // Striking a key again ends its pedal-held copy.
                            let mut i = 0;
                            while i < st.held.len() {
                                if st.held[i].chan == chan && st.held[i].key == key {
                                    let h = st.held.remove(i);
                                    finish_note(
                                        &mut scan,
                                        &mut ctx.repairs,
                                        h.chan,
                                        h.key,
                                        h.note,
                                        tick,
                                        Some(h.release),
                                    );
                                } else {
                                    i += 1;
                                }
                            }
                        }
                        let pending = PendingNote {
                            start: tick,
                            vel: vel.as_int(),
                            program: st.program[ci],
                            bank: st.bank(chan),
                        };
                        let queue = st.pending.entry((chan, key)).or_default();
                        if !queue.is_empty() {
                            ctx.repairs.overlapping_same_pitch += 1;
                        }
                        queue.push_back(pending);
                    }
                    MidiMessage::NoteOn { key, .. } => {
                        // Velocity 0 is a note-off with no release velocity.
                        note_off(
                            &mut st,
                            &mut scan,
                            ctx,
                            opts,
                            chan,
                            key.as_int(),
                            None,
                            tick,
                        );
                    }
                    MidiMessage::NoteOff { key, vel } => {
                        note_off(
                            &mut st,
                            &mut scan,
                            ctx,
                            opts,
                            chan,
                            key.as_int(),
                            Some(vel.as_int()),
                            tick,
                        );
                    }
                    MidiMessage::Controller { controller, value } => {
                        let cc = controller.as_int();
                        let value = value.as_int();
                        match cc {
                            0 => st.bank_msb[ci] = Some(value),
                            32 => st.bank_lsb[ci] = Some(value),
                            _ => {}
                        }
                        if cc == 64 && opts.sustain == SustainPolicy::ExtendNotes {
                            let down = value >= 64;
                            st.pedal[ci] = down;
                            if !down {
                                let mut i = 0;
                                while i < st.held.len() {
                                    if st.held[i].chan == chan {
                                        let h = st.held.remove(i);
                                        finish_note(
                                            &mut scan,
                                            &mut ctx.repairs,
                                            h.chan,
                                            h.key,
                                            h.note,
                                            tick,
                                            Some(h.release),
                                        );
                                    } else {
                                        i += 1;
                                    }
                                }
                            }
                        } else {
                            ctx.skip(controller_name(cc));
                        }
                    }
                    MidiMessage::ProgramChange { program } => {
                        st.program[ci] = Some(program.as_int());
                        ctx.skip("Program Change (reported per channel, not applied)");
                    }
                    MidiMessage::Aftertouch { .. } => ctx.skip("Polyphonic Aftertouch"),
                    MidiMessage::ChannelAftertouch { .. } => ctx.skip("Channel Aftertouch"),
                    MidiMessage::PitchBend { .. } => ctx.skip("Pitch Bend"),
                }
            }
            TrackEventKind::SysEx(_) => ctx.skip("System Exclusive"),
            TrackEventKind::Escape(_) => ctx.skip("Escape Sequence"),
            TrackEventKind::Meta(meta) => match meta {
                MetaMessage::EndOfTrack => {
                    ended = true;
                }
                MetaMessage::TrackName(b) => {
                    let name = String::from_utf8_lossy(b).trim().to_string();
                    if scan.name.is_none() && !name.is_empty() {
                        scan.name = Some(name);
                    } else {
                        ctx.text(index, tick, "Track Name", b);
                    }
                }
                MetaMessage::Tempo(us) => ctx.tempo.push((index, tick, us.as_int())),
                MetaMessage::TimeSignature(n, dd, _, _) => {
                    ctx.signatures.push((index, tick, n, dd))
                }
                MetaMessage::KeySignature(sf, minor) => ctx.key_signatures.push((tick, sf, minor)),
                MetaMessage::Marker(b) => {
                    ctx.markers
                        .push((tick, String::from_utf8_lossy(b).trim().to_string()));
                }
                MetaMessage::Text(b) => ctx.text(index, tick, "Text", b),
                MetaMessage::Copyright(b) => ctx.text(index, tick, "Copyright", b),
                MetaMessage::InstrumentName(b) => ctx.text(index, tick, "Instrument Name", b),
                MetaMessage::Lyric(b) => ctx.text(index, tick, "Lyric", b),
                MetaMessage::CuePoint(b) => ctx.text(index, tick, "Cue Point", b),
                MetaMessage::ProgramName(b) => ctx.text(index, tick, "Program Name", b),
                MetaMessage::DeviceName(b) => ctx.text(index, tick, "Device Name", b),
                MetaMessage::TrackNumber(_) => ctx.skip("Sequence Number"),
                MetaMessage::MidiChannel(_) => ctx.skip("MIDI Channel Prefix"),
                MetaMessage::MidiPort(_) => ctx.skip("MIDI Port Prefix"),
                MetaMessage::SmpteOffset(_) => ctx.skip("SMPTE Offset"),
                MetaMessage::SequencerSpecific(_) => ctx.skip("Sequencer-Specific Meta Event"),
                MetaMessage::Unknown(id, _) => ctx.skip(format!("Unknown Meta Event 0x{id:02X}")),
            },
        }
    }

    // Notes still sounding when the track ends.
    let end = scan.end_tick;
    let mut dangling: Vec<(u8, u8, PendingNote)> = Vec::new();
    for ((chan, key), queue) in st.pending.drain() {
        for note in queue {
            dangling.push((chan, key, note));
        }
    }
    dangling.sort_by_key(|(c, k, n)| (n.start, *c, *k));
    for (chan, key, note) in dangling {
        ctx.repairs.unterminated_notes += 1;
        finish_note(&mut scan, &mut ctx.repairs, chan, key, note, end, None);
    }
    for h in std::mem::take(&mut st.held) {
        finish_note(
            &mut scan,
            &mut ctx.repairs,
            h.chan,
            h.key,
            h.note,
            end,
            Some(h.release),
        );
    }
    Ok(scan)
}

#[allow(clippy::too_many_arguments)]
fn note_off(
    st: &mut TrackState,
    scan: &mut TrackScan,
    ctx: &mut Collected,
    opts: &ImportOptions,
    chan: u8,
    key: u8,
    release: Option<u8>,
    tick: u64,
) {
    let Some(note) = st.pending.get_mut(&(chan, key)).and_then(|q| q.pop_front()) else {
        ctx.repairs.orphan_note_offs += 1;
        return;
    };
    if opts.sustain == SustainPolicy::ExtendNotes && st.pedal[usize::from(chan)] {
        st.held.push(HeldNote {
            chan,
            key,
            note,
            release: release.unwrap_or(64),
        });
    } else {
        finish_note(scan, &mut ctx.repairs, chan, key, note, tick, release);
    }
}

// ------------------------------------------------------------ public API

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum GroupKey {
    Melodic { track: usize, chan: u8 },
    Drum { key: u8 },
}

/// Parses a Standard MIDI File (types 0, 1 and 2, and RIFF-wrapped `.rmi`)
/// into channels, patterns, tempo and meter plus a full report.
pub fn import_smf(bytes: &[u8], opts: &ImportOptions) -> Result<MidiImport> {
    if bytes.len() > 256 * 1024 * 1024 {
        return Err(MidiError::TooLarge("file is larger than 256 MiB".into()));
    }
    guard_header_chunks(bytes)?;
    let (header, mut tracks) = midly::parse(bytes).map_err(|e| MidiError::Parse(e.to_string()))?;
    let declared_tracks = tracks.size_hint().0;

    let format = match header.format {
        Format::SingleTrack => SmfFormat::Type0,
        Format::Parallel => SmfFormat::Type1,
        Format::Sequential => SmfFormat::Type2,
    };
    let (clock, ticks_per_quarter, smpte) = match header.timing {
        Timing::Metrical(ppq) => {
            let ppq = ppq.as_int();
            if ppq == 0 {
                return Err(MidiError::UnsupportedTiming(
                    "0 ticks per quarter note".into(),
                ));
            }
            (
                TickClock {
                    num: i128::from(ppq),
                    den: 1,
                },
                Some(ppq),
                false,
            )
        }
        Timing::Timecode(fps, subframe) => {
            if subframe == 0 {
                return Err(MidiError::UnsupportedTiming(
                    "SMPTE timing with 0 ticks per frame".into(),
                ));
            }
            // Absolute time cannot be mapped to beats without a tempo; use
            // the default 120 BPM (2 quarter notes per second).
            let (fps_num, fps_den) = match fps.as_int() {
                29 => (30_000i128, 1001i128),
                n => (i128::from(n), 1),
            };
            (
                TickClock {
                    num: fps_num * i128::from(subframe),
                    den: fps_den * 2,
                },
                None,
                true,
            )
        }
    };

    let mut ctx = Collected::default();
    let mut scans: Vec<TrackScan> = Vec::new();
    let mut yielded = 0usize;
    while let Some(track) = tracks.next() {
        yielded += 1;
        let index = yielded - 1;
        let events = match track {
            Ok(e) => e,
            Err(e) => {
                let offset = bytes.len().saturating_sub(tracks.unread().len());
                if !opts.allow_partial {
                    return Err(MidiError::Malformed {
                        track: index,
                        offset,
                        message: e.to_string(),
                    });
                }
                ctx.repairs
                    .malformed_tracks
                    .push((index, offset, e.to_string()));
                scans.push(TrackScan::default());
                continue;
            }
        };
        let raw_len = events.unread().len();
        let scan = scan_track(index, events, raw_len, opts, &mut ctx)?;
        // Convert track-relative offsets (bytes from the track start) into
        // file offsets for malformed-track reports.
        let track_start = bytes.len().saturating_sub(tracks.unread().len() + raw_len);
        for m in ctx
            .repairs
            .malformed_tracks
            .iter_mut()
            .filter(|m| m.0 == index)
        {
            m.1 += track_start;
        }
        scans.push(scan);
    }
    if yielded != declared_tracks {
        ctx.warnings.push(format!(
            "the header declares {declared_tracks} tracks but {yielded} were found"
        ));
    }
    if format == SmfFormat::Type0 && yielded > 1 {
        ctx.warnings
            .push("a type 0 file has more than one track; treated like type 1".into());
    }
    if smpte {
        ctx.warnings.push(
            "SMPTE timing has no musical tempo; positions assume 120 BPM and tempo events are ignored"
                .into(),
        );
    }
    if format == SmfFormat::Type2 && opts.layout != PatternLayout::PerTrack {
        ctx.warnings
            .push("type 2 sequences are independent; imported one pattern per sequence".into());
    }

    // Tempo and meter. Type 2 sequences each have their own timeline; only
    // the first sequence's tempo and meter are adopted and the rest counted.
    let adopt = |track: usize| format != SmfFormat::Type2 || track == 0;
    let mut tempo_events: Vec<(u64, u32)> = Vec::new();
    for &(track, tick, us) in &ctx.tempo.clone() {
        if smpte {
            ctx.skip("Tempo (ignored with SMPTE timing)");
        } else if us == 0 {
            ctx.skip("Tempo of 0 microseconds per quarter note");
        } else if adopt(track) {
            tempo_events.push((tick, us));
        } else {
            ctx.skip("Tempo (in a later independent sequence)");
        }
    }
    let (tempo, tempo_changes) = build_tempo(&tempo_events, clock)?;

    let mut signatures: Vec<(Time, TimeSignature)> = Vec::new();
    for &(track, tick, n, dd) in &ctx.signatures.clone() {
        let position = clock.time(tick)?;
        if !adopt(track) {
            ctx.skip("Time Signature (in a later independent sequence)");
        } else if n == 0 || dd > 7 {
            ctx.warnings.push(format!(
                "ignored an unrepresentable time signature {n}/2^{dd} at {position}"
            ));
            ctx.skip("Time Signature (unrepresentable)");
        } else {
            signatures.push((
                position,
                TimeSignature {
                    numerator: n,
                    denominator: 1u8 << dd,
                },
            ));
        }
    }
    signatures.sort_by_key(|a| a.0);
    let time_signature = signatures.first().map(|s| s.1).unwrap_or_default();
    if signatures.len() > 1 {
        ctx.unsupported.insert(
            "Time Signature change after the first (listed in report.time_signatures)".to_string(),
            signatures.len() as u64 - 1,
        );
    }

    let key_signatures = ctx
        .key_signatures
        .iter()
        .map(|&(tick, sf, minor)| {
            Ok(KeySignatureEvent {
                position: clock.time(tick)?,
                accidentals: sf,
                minor,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if !key_signatures.is_empty() {
        ctx.unsupported.insert(
            "Key Signature (listed in report.key_signatures)".to_string(),
            key_signatures.len() as u64,
        );
    }
    let markers = ctx
        .markers
        .iter()
        .map(|(tick, name)| Ok((clock.time(*tick)?, name.clone())))
        .collect::<Result<Vec<_>>>()?;

    let bar = time_signature.bar_length();
    let build = Builder {
        opts,
        clock,
        bar,
        format,
    };
    let mut import = build.assemble(&scans, &mut ctx)?;

    import.tempo = tempo;
    import.time_signature = time_signature;
    import.markers = markers;
    import.sequential = format == SmfFormat::Type2;

    let metadata = ctx
        .metadata
        .iter()
        .map(|(track, tick, kind, text)| {
            Ok(MetaEntry {
                track: *track,
                position: clock.time(*tick)?,
                kind,
                text: text.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut duration = Time::ZERO;
    for t in &import.report.tracks {
        duration = duration.max(t.end);
    }
    let r = &mut import.report;
    r.format = format;
    r.ticks_per_quarter = ticks_per_quarter;
    r.duration = duration;
    r.tempo_changes = tempo_changes;
    r.time_signatures = signatures;
    r.key_signatures = key_signatures;
    r.unsupported = ctx
        .unsupported
        .into_iter()
        .map(|(description, count)| UnsupportedEvents { description, count })
        .collect();
    r.repairs = ctx.repairs;
    r.metadata = metadata;
    r.warnings = ctx.warnings;
    r.total_notes = import.patterns.iter().map(|p| p.notes.len()).sum();
    Ok(import)
}

/// The SMF data inside a RIFF (`.rmi`) wrapper, or `bytes` unchanged.
fn smf_data(bytes: &[u8]) -> &[u8] {
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"RMID") {
        return bytes;
    }
    let mut rest = &bytes[12..];
    while rest.len() >= 8 {
        let id = &rest[..4];
        let len = u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
        let body = &rest[8..];
        let data = &body[..len.min(body.len())];
        if id == b"data" {
            return data;
        }
        rest = &body[len.saturating_add(len & 1).min(body.len())..];
    }
    bytes
}

/// `midly` computes the SMPTE frame rate of every `MThd` chunk with an
/// unchecked negation, which overflows (a panic in debug builds) when the
/// division's high byte is 0x80. Hostile or damaged files must produce an
/// error, so every header chunk is checked first, walking chunks exactly as
/// `midly` does.
fn guard_header_chunks(bytes: &[u8]) -> Result<()> {
    let mut rest = smf_data(bytes);
    while rest.len() >= 8 {
        let id = &rest[..4];
        let len = u32::from_be_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
        let body = &rest[8..];
        let data = &body[..len.min(body.len())];
        if id == b"MThd" && data.len() >= 6 && data[4] == 0x80 {
            return Err(MidiError::UnsupportedTiming(
                "invalid SMPTE frame rate in the header".into(),
            ));
        }
        rest = &body[data.len()..];
    }
    Ok(())
}

fn build_tempo(events: &[(u64, u32)], clock: TickClock) -> Result<(Vec<TempoSegment>, usize)> {
    let mut sorted: Vec<(u64, u32)> = events.to_vec();
    sorted.sort_by_key(|e| e.0);
    let mut segments: Vec<TempoSegment> = Vec::new();
    for (tick, us) in sorted {
        let start = clock.time(tick)?;
        let bpm = 60_000_000.0 / f64::from(us);
        match segments.last_mut() {
            // Several tempo events at one instant: the last one wins.
            Some(last) if last.start == start => last.bpm = bpm,
            _ => segments.push(TempoSegment {
                start,
                bpm,
                ramp_to: None,
            }),
        }
    }
    if segments.first().is_none_or(|s| s.start != Time::ZERO) {
        segments.insert(
            0,
            TempoSegment {
                start: Time::ZERO,
                bpm: 120.0,
                ramp_to: None,
            },
        );
    }
    // Repeated identical tempi are not changes.
    let mut deduped: Vec<TempoSegment> = Vec::with_capacity(segments.len());
    for s in segments {
        if deduped.last().is_none_or(|l| l.bpm != s.bpm) {
            deduped.push(s);
        }
    }
    let changes = deduped.len() - 1;
    Ok((deduped, changes))
}

struct Builder<'a> {
    opts: &'a ImportOptions,
    clock: TickClock,
    bar: Time,
    format: SmfFormat,
}

impl Builder<'_> {
    fn assemble(&self, scans: &[TrackScan], ctx: &mut Collected) -> Result<MidiImport> {
        let opts = self.opts;
        let group_of = |track: usize, n: &RawNote| {
            if opts.split_drums && n.chan == opts.drum_channel {
                GroupKey::Drum { key: n.key }
            } else {
                GroupKey::Melodic {
                    track,
                    chan: n.chan,
                }
            }
        };

        // Group notes into channels.
        let mut groups: BTreeMap<GroupKey, Vec<(usize, &RawNote)>> = BTreeMap::new();
        for (ti, scan) in scans.iter().enumerate() {
            for n in &scan.notes {
                groups.entry(group_of(ti, n)).or_default().push((ti, n));
            }
        }
        let melodic_per_track: BTreeMap<usize, BTreeSet<u8>> = {
            let mut m: BTreeMap<usize, BTreeSet<u8>> = BTreeMap::new();
            for k in groups.keys() {
                if let GroupKey::Melodic { track, chan } = k {
                    m.entry(*track).or_default().insert(*chan);
                }
            }
            m
        };

        let mut channels: Vec<ImportedChannel> = Vec::new();
        let mut channel_of: BTreeMap<GroupKey, Id> = BTreeMap::new();
        for (key, notes) in &groups {
            let mut by_start: Vec<&(usize, &RawNote)> = notes.iter().collect();
            by_start.sort_by_key(|(_, n)| n.start);
            let first_program = by_start.iter().find_map(|(_, n)| n.program);
            let first_bank = by_start.iter().find_map(|(_, n)| n.bank);
            let (name, device, source, root_key) = match *key {
                GroupKey::Drum { key: drum_key } => {
                    let (type_id, version, name) = (opts.drum_device_for_key)(drum_key);
                    (
                        name,
                        DeviceInstance::new(type_id, version),
                        ChannelSource {
                            track: None,
                            midi_channel: opts.drum_channel,
                            drum_key: Some(drum_key),
                            program: first_program,
                            bank: first_bank,
                            notes: notes.len(),
                        },
                        drum_key,
                    )
                }
                GroupKey::Melodic { track, chan } => {
                    let track_name = scans[track].name.clone();
                    let program_name = first_program.map(|p| gm::program_name(p).to_string());
                    let multi = melodic_per_track.get(&track).is_some_and(|s| s.len() > 1);
                    let name = if multi {
                        format!(
                            "{} (ch {})",
                            program_name
                                .or(track_name)
                                .unwrap_or_else(|| format!("Track {}", track + 1)),
                            chan + 1
                        )
                    } else {
                        track_name
                            .or(program_name)
                            .unwrap_or_else(|| format!("Track {}", track + 1))
                    };
                    (
                        name,
                        DeviceInstance::new(opts.melodic_device.0.clone(), opts.melodic_device.1),
                        ChannelSource {
                            track: Some(track),
                            midi_channel: chan,
                            drum_key: None,
                            program: first_program,
                            bank: first_bank,
                            notes: notes.len(),
                        },
                        DEFAULT_ROOT_KEY,
                    )
                }
            };
            let id = Id::new();
            channel_of.insert(*key, id);
            channels.push(ImportedChannel {
                channel: Channel {
                    id,
                    name,
                    color: PALETTE[channels.len() % PALETTE.len()],
                    device,
                    track: Id::NIL,
                    gain_db: 0.0,
                    pan: 0.0,
                    muted: false,
                    root_key,
                },
                source,
            });
        }

        // Patterns. Type 2 sequences are independent whatever was asked.
        let layout = if self.format == SmfFormat::Type2 {
            PatternLayout::PerTrack
        } else {
            opts.layout
        };
        #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
        enum PatKey {
            Track(usize),
            Channel(u8),
            All,
        }
        let mut buckets: BTreeMap<PatKey, Vec<(usize, &RawNote)>> = BTreeMap::new();
        for (ti, scan) in scans.iter().enumerate() {
            for n in &scan.notes {
                let key = match layout {
                    PatternLayout::PerTrack => PatKey::Track(ti),
                    PatternLayout::PerMidiChannel => PatKey::Channel(n.chan),
                    PatternLayout::Combined => PatKey::All,
                };
                buckets.entry(key).or_default().push((ti, n));
            }
        }

        let mut patterns: Vec<Pattern> = Vec::new();
        for (pkey, notes) in &buckets {
            let mut name = match pkey {
                PatKey::Track(t) => scans[*t]
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Track {}", t + 1)),
                PatKey::Channel(c) if *c == opts.drum_channel => "Percussion".to_string(),
                PatKey::Channel(c) => format!("Channel {}", c + 1),
                PatKey::All => opts.combined_pattern_name.clone(),
            };
            if name.is_empty() {
                name = "Imported".into();
            }
            let mut content_end = Time::ZERO;
            // Tracks contributing to this pattern also contribute their
            // end-of-track, so trailing rests keep the pattern length.
            let contributing: BTreeSet<usize> = notes.iter().map(|(t, _)| *t).collect();
            for t in &contributing {
                content_end = content_end.max(self.clock.time(scans[*t].end_tick)?);
            }
            let mut out_notes: Vec<Note> = Vec::with_capacity(notes.len());
            for (ti, n) in notes {
                let channel = channel_of[&group_of(*ti, n)];
                let (start, length, moved) = self.note_times(n)?;
                if moved {
                    ctx.repairs.notes_quantized += 1;
                }
                let mut note = Note::new(channel, start, length, n.key, f32::from(n.vel) / 127.0);
                note.release = release_from_midi(n.release);
                content_end = content_end.max(start + length);
                out_notes.push(note);
            }
            let bars = content_end.ceil_to(self.bar);
            let length = if bars.is_positive() { bars } else { self.bar };
            let mut pattern = Pattern {
                id: Id::new(),
                name,
                color: PALETTE[patterns.len() % PALETTE.len()],
                length,
                notes: out_notes,
                expression: Vec::new(),
            };
            pattern.sort_notes();
            patterns.push(pattern);
        }

        // Report.
        let mut tracks: Vec<TrackReport> = Vec::new();
        for (ti, scan) in scans.iter().enumerate() {
            let mut by_chan: BTreeMap<u8, Vec<&RawNote>> = BTreeMap::new();
            for n in &scan.notes {
                by_chan.entry(n.chan).or_default().push(n);
            }
            let channels = by_chan
                .into_iter()
                .map(|(chan, notes)| {
                    let mut programs: BTreeMap<u8, usize> = BTreeMap::new();
                    for n in &notes {
                        if let Some(p) = n.program {
                            *programs.entry(p).or_default() += 1;
                        }
                    }
                    let keys = notes.iter().map(|n| n.key);
                    let vels = notes.iter().map(|n| n.vel);
                    ChannelReport {
                        midi_channel: chan,
                        is_drum: chan == opts.drum_channel,
                        notes: notes.len(),
                        key_range: (keys.clone().min().unwrap_or(0), keys.max().unwrap_or(0)),
                        velocity_range: (vels.clone().min().unwrap_or(0), vels.max().unwrap_or(0)),
                        programs: programs
                            .into_iter()
                            .map(|(program, notes)| ProgramUse {
                                program,
                                name: gm::program_name(program),
                                notes,
                            })
                            .collect(),
                        bank: notes.iter().find_map(|n| n.bank),
                    }
                })
                .collect();
            tracks.push(TrackReport {
                index: ti,
                name: scan.name.clone(),
                notes: scan.notes.len(),
                events: scan.events,
                channels,
                end: self.clock.time(scan.end_tick)?,
            });
        }
        let title = match scans.first() {
            Some(first) if first.notes.is_empty() => first.name.clone(),
            _ => None,
        };

        Ok(MidiImport {
            channels,
            patterns,
            tempo: Vec::new(),
            time_signature: TimeSignature::default(),
            markers: Vec::new(),
            title,
            sequential: false,
            report: ImportReport {
                format: self.format,
                ticks_per_quarter: None,
                duration: Time::ZERO,
                total_notes: 0,
                tracks,
                tempo_changes: 0,
                time_signatures: Vec::new(),
                key_signatures: Vec::new(),
                unsupported: Vec::new(),
                repairs: Repairs::default(),
                metadata: Vec::new(),
                warnings: Vec::new(),
            },
        })
    }

    /// Exact (or optionally quantized) start and length. The flag is true
    /// when quantizing changed the note.
    fn note_times(&self, n: &RawNote) -> Result<(Time, Time, bool)> {
        let start = self.clock.time(n.start)?;
        let length = self.clock.time(n.end)? - start;
        Ok(match self.opts.quantize {
            Quantize::None => (start, length, false),
            Quantize::Starts(grid) if grid.is_positive() => {
                let s = start.round_to(grid);
                (s, length, s != start)
            }
            Quantize::StartsAndLengths(grid) if grid.is_positive() => {
                let s = start.round_to(grid);
                let l = length.round_to(grid).max(grid);
                (s, l, s != start || l != length)
            }
            _ => (start, length, false),
        })
    }
}

/// MIDI release velocity to Peel's 0..=1. 64 (the near-universal default)
/// and "none" both map to the model's default of 0.5, which keeps the
/// round trip lossless for notes that never had a release velocity.
pub(crate) fn release_from_midi(v: Option<u8>) -> f32 {
    match v {
        None | Some(64) => 0.5,
        Some(v) => f32::from(v) / 127.0,
    }
}

// ----------------------------------------------------------------- merge

/// What a merge is allowed to change besides adding channels and patterns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeOptions {
    /// Replace the project's tempo map with the file's.
    pub adopt_tempo: bool,
    /// Replace the project's time signature with the file's.
    pub adopt_time_signature: bool,
    /// Place each pattern as a clip at the start of the active arrangement
    /// (side by side, or one after another for type 2 files).
    pub place_clips: bool,
    /// Add the file's markers to the active arrangement.
    pub add_markers: bool,
}

impl MergeOptions {
    /// Importing a whole song into a new, empty project.
    pub fn new_project() -> MergeOptions {
        MergeOptions {
            adopt_tempo: true,
            adopt_time_signature: true,
            place_clips: true,
            add_markers: true,
        }
    }
}

impl Default for MergeOptions {
    /// Adding to an existing song: its tempo and meter stay as they are.
    fn default() -> Self {
        MergeOptions {
            adopt_tempo: false,
            adopt_time_signature: false,
            place_clips: true,
            add_markers: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub channels: Vec<Id>,
    pub patterns: Vec<Id>,
    pub clips: Vec<Id>,
    /// Playlist lanes created for the clips.
    pub lanes: Vec<Id>,
    pub mixer_tracks_created: usize,
}

impl MidiImport {
    /// Adds the imported channels and patterns to `project`.
    ///
    /// `mixer_track_for_channel` decides where each channel is routed: it may
    /// return an existing mixer track or create one in the project it is
    /// given. The merge works on a copy and replaces `project` only if the
    /// result validates, so a failure leaves the project untouched. Run it
    /// inside one `Document::edit` for a single undo step.
    pub fn merge_into(
        self,
        project: &mut Project,
        opts: &MergeOptions,
        mut mixer_track_for_channel: impl FnMut(&mut Project, &ImportedChannel) -> Id,
    ) -> Result<MergeReport> {
        let mut work = project.clone();
        let mut report = MergeReport::default();

        for mut imported in self.channels {
            let track = mixer_track_for_channel(&mut work, &imported);
            if work.mixer.track(track).is_none() {
                return Err(MidiError::UnknownMixerTrack(track));
            }
            imported.channel.track = track;
            imported.channel.color = work.next_channel_color();
            report.channels.push(imported.channel.id);
            work.channels.push(imported.channel);
        }

        let mut cursor = Time::ZERO;
        let mut shared_lane: Option<Id> = None;
        for mut pattern in self.patterns {
            pattern.color = PALETTE[work.patterns.len() % PALETTE.len()];
            let (id, length, name) = (pattern.id, pattern.length, pattern.name.clone());
            report.patterns.push(id);
            work.patterns.push(std::sync::Arc::new(pattern));
            if !opts.place_clips {
                continue;
            }
            let start = if self.sequential { cursor } else { Time::ZERO };
            let end = start + length;
            let arrangement = work.arrangement_mut();
            let lane = if self.sequential {
                match shared_lane.filter(|l| lane_is_free(arrangement, *l, start, end)) {
                    Some(l) => l,
                    None => {
                        let l = free_or_new_lane(arrangement, start, end, &name, &mut report.lanes);
                        shared_lane = Some(l);
                        l
                    }
                }
            } else {
                free_or_new_lane(arrangement, start, end, &name, &mut report.lanes)
            };
            let clip = Clip {
                id: Id::new(),
                track: lane,
                start,
                length,
                offset: Time::ZERO,
                muted: false,
                content: ClipContent::Pattern { pattern: id },
            };
            report.clips.push(clip.id);
            arrangement.clips.push(clip);
            cursor = end;
        }

        if opts.add_markers {
            let arrangement = work.arrangement_mut();
            for (position, name) in self.markers {
                arrangement.markers.push(Marker {
                    id: Id::new(),
                    position,
                    name,
                });
            }
        }
        if opts.adopt_tempo && !self.tempo.is_empty() {
            work.tempo = self.tempo;
        }
        if opts.adopt_time_signature {
            work.time_signature = self.time_signature;
        }

        let errors: Vec<String> = validate(&work)
            .into_iter()
            .filter(|i| i.is_error())
            .map(|i| i.message)
            .collect();
        if !errors.is_empty() {
            return Err(MidiError::InvalidMerge(errors));
        }
        *project = work;
        Ok(report)
    }

    /// Like [`merge_into`](Self::merge_into), routing every channel to a new
    /// mixer track of its own, named and colored like the channel.
    pub fn merge_with_new_mixer_tracks(
        self,
        project: &mut Project,
        opts: &MergeOptions,
    ) -> Result<MergeReport> {
        let mut created = 0usize;
        let mut report = self.merge_into(project, opts, |p, ic| {
            let track = MixerTrack {
                id: Id::new(),
                name: ic.channel.name.clone(),
                color: ic.channel.color,
                gain_db: 0.0,
                pan: 0.0,
                muted: false,
                solo: false,
                inserts: Vec::new(),
                output: None,
                sends: Vec::new(),
            };
            let id = track.id;
            p.mixer.tracks.push(track);
            created += 1;
            id
        })?;
        report.mixer_tracks_created = created;
        Ok(report)
    }
}

fn lane_is_free(arrangement: &Arrangement, lane: Id, start: Time, end: Time) -> bool {
    !arrangement
        .clips
        .iter()
        .any(|c| c.track == lane && c.start < end && start < c.start + c.length)
}

fn free_or_new_lane(
    arrangement: &mut Arrangement,
    start: Time,
    end: Time,
    name: &str,
    created: &mut Vec<Id>,
) -> Id {
    if let Some(t) = arrangement
        .tracks
        .iter()
        .find(|t| lane_is_free(arrangement, t.id, start, end))
    {
        return t.id;
    }
    let lane = Track {
        id: Id::new(),
        name: name.to_string(),
        color: None,
        muted: false,
    };
    let id = lane.id;
    arrangement.tracks.push(lane);
    created.push(id);
    id
}
