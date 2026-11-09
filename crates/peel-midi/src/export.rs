//! Standard MIDI File export.
//!
//! Exports flatten the document to note occurrences first: pattern instances,
//! loops, clip offsets and clip trimming are resolved, muted material and
//! probability are applied according to the options, and only then are
//! positions converted to ticks. Peel stores exact rational beats, so a tick
//! grid can force rounding; every rounded value is counted in the
//! [`ExportReport`] along with everything else MIDI cannot carry.

use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use peel_document::{Channel, ClipContent, Id, NoteExpression, Project, Time};

use crate::error::{MidiError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// One track holding everything.
    Type0,
    /// A tempo/meter track plus one track per channel.
    Type1,
}

/// How note probability is treated. Exporting a probabilistic song without a
/// seed would give a different file every time, so the default is the
/// project's seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbabilityMode {
    /// Export every note regardless of probability.
    Ignore,
    /// Roll each note against the project's seed.
    ProjectSeed,
    /// Roll each note against an explicit seed.
    Seeded(u64),
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// Ticks per quarter note, 1..=32767.
    pub ppq: u16,
    /// Write every tempo segment. When off, only the first tempo is written.
    pub include_tempo_map: bool,
    /// Leave out muted notes, muted channels, muted clips and muted lanes.
    pub respect_mute: bool,
    pub probability: ProbabilityMode,
    /// Channels that play on MIDI channel 10 (percussion).
    pub is_drum: fn(&Channel) -> bool,
    /// GM program to select for a channel, if it has a sensible one.
    pub program_for_channel: fn(&Channel) -> Option<u8>,
    /// Write tracks for channels that have no exported notes.
    pub include_empty_channels: bool,
}

/// Percussion by device family: `peel.drum.*` plays on channel 10.
fn default_is_drum(channel: &Channel) -> bool {
    channel.device.type_id.starts_with("peel.drum.")
}

fn no_program(_: &Channel) -> Option<u8> {
    None
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            format: ExportFormat::Type1,
            ppq: 960,
            include_tempo_map: true,
            respect_mute: true,
            probability: ProbabilityMode::ProjectSeed,
            is_drum: default_is_drum,
            program_for_channel: no_program,
            include_empty_channels: false,
        }
    }
}

/// What the export had to give up or approximate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportReport {
    pub tracks: usize,
    pub notes: usize,
    pub skipped_muted: usize,
    pub skipped_probability: usize,
    /// Notes with probability below 1 exported unconditionally.
    pub probability_ignored: usize,
    /// Notes starting before time zero, which MIDI cannot express.
    pub skipped_negative: usize,
    pub muted_clips_skipped: usize,
    pub audio_clips_omitted: usize,
    /// Start, end and tempo positions that did not land on a tick.
    pub rounded_values: usize,
    /// Largest such error, in ticks.
    pub max_rounding_error_ticks: f64,
    /// Notes that rounded to zero length and were given one tick.
    pub lengthened_notes: usize,
    pub tempo_ramps_approximated: usize,
    /// Notes with per-note pan, which MIDI 1.0 notes cannot carry.
    pub notes_with_pan: usize,
    /// Notes with fine pitch, preserved through MPE pitch bend.
    pub notes_with_fine_pitch: usize,
    /// Notes routed independently through MPE member channels.
    pub mpe_notes: usize,
    /// Pitch snapshots limited to the MIDI MPE range of +/-96 semitones.
    pub pitch_values_clamped: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct MidiExport {
    pub bytes: Vec<u8>,
    pub report: ExportReport,
}

struct FlatNote {
    channel: Id,
    start: Time,
    end: Time,
    key: u8,
    velocity: f32,
    release: f32,
    has_pan: bool,
    fine: f32,
    expression: Option<NoteExpression>,
}

fn overflow<E>(_: E) -> MidiError {
    MidiError::Export("musical time arithmetic overflowed".into())
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Deterministic roll in [0, 1) for one note occurrence.
fn roll(seed: u64, note: Id, salt: u64) -> f64 {
    let h = splitmix(seed ^ splitmix(note.short() ^ splitmix(salt)));
    (h >> 11) as f64 / (1u64 << 53) as f64
}

struct Flattener<'a> {
    project: &'a Project,
    opts: &'a ExportOptions,
    seed: Option<u64>,
    report: ExportReport,
    notes: Vec<FlatNote>,
}

impl<'a> Flattener<'a> {
    fn new(project: &'a Project, opts: &'a ExportOptions) -> Self {
        let seed = match opts.probability {
            ProbabilityMode::Ignore => None,
            ProbabilityMode::ProjectSeed => Some(project.seed),
            ProbabilityMode::Seeded(s) => Some(s),
        };
        Flattener {
            project,
            opts,
            seed,
            report: ExportReport::default(),
            notes: Vec::new(),
        }
    }

    /// Adds one occurrence of a note, if mute and probability allow it.
    fn push(
        &mut self,
        note: &peel_document::Note,
        expression: Option<&NoteExpression>,
        start: Time,
        end: Time,
        from: Time,
        salt: u64,
    ) {
        let Some(channel) = self.project.channel(note.channel) else {
            return;
        };
        if self.opts.respect_mute && (note.muted || channel.muted) {
            self.report.skipped_muted += 1;
            return;
        }
        if note.probability < 1.0 {
            match self.seed {
                Some(seed) => {
                    if roll(seed, note.id, salt) >= f64::from(note.probability.max(0.0)) {
                        self.report.skipped_probability += 1;
                        return;
                    }
                }
                None => self.report.probability_ignored += 1,
            }
        }
        if note.key > 127 {
            return;
        }
        self.notes.push(FlatNote {
            channel: note.channel,
            start,
            end,
            key: note.key,
            velocity: note.velocity,
            release: note.release,
            has_pan: note.pan != 0.0,
            fine: note.fine,
            expression: expression.map(|e| e.slice(note.id, from, end - start)),
        });
    }
}

/// Exports one pattern as it plays in pattern mode: from its start, once.
pub fn export_pattern(project: &Project, pattern: Id, opts: &ExportOptions) -> Result<MidiExport> {
    let source = project
        .pattern(pattern)
        .ok_or(MidiError::UnknownPattern(pattern))?;
    let mut fl = Flattener::new(project, opts);
    for note in &source.notes {
        let end = note.start.checked_add(note.length).map_err(overflow)?;
        fl.push(
            note,
            source.expression_for(note.id),
            note.start,
            end,
            Time::ZERO,
            0,
        );
    }
    let end = source.length;
    build(project, opts, fl, end)
}

/// Exports the active arrangement: every pattern clip is flattened, loops
/// repeated, offsets and clip ends applied. Audio clips cannot be written to
/// a MIDI file and are counted in the report.
pub fn export_arrangement(project: &Project, opts: &ExportOptions) -> Result<MidiExport> {
    let arrangement = project.arrangement();
    let mut fl = Flattener::new(project, opts);
    for clip in &arrangement.clips {
        let pattern_id = match &clip.content {
            ClipContent::Pattern { pattern } => *pattern,
            ClipContent::Audio { .. } => {
                fl.report.audio_clips_omitted += 1;
                continue;
            }
            // Automation is not exported as MIDI controllers yet.
            ClipContent::Automation { .. } => continue,
        };
        let lane_muted = arrangement
            .tracks
            .iter()
            .find(|t| t.id == clip.track)
            .is_some_and(|t| t.muted);
        if opts.respect_mute && (clip.muted || lane_muted) {
            fl.report.muted_clips_skipped += 1;
            continue;
        }
        let Some(pattern) = project.pattern(pattern_id) else {
            continue;
        };
        let plen = pattern.length;
        if !plen.is_positive() || !clip.length.is_positive() {
            continue;
        }
        let clip_end = clip.start.checked_add(clip.length).map_err(overflow)?;
        // First loop iteration that can still be audible.
        let first = clip
            .offset
            .checked_div(plen)
            .map_err(overflow)?
            .floor_int()
            .max(0);
        let mut k = first;
        loop {
            if k - first > 200_000 {
                return Err(MidiError::Export(format!(
                    "pattern {} loops more than 200000 times in one clip",
                    pattern.name
                )));
            }
            let shift = plen.mul_int(i64::try_from(k).map_err(overflow)?);
            let rel = shift.checked_sub(clip.offset).map_err(overflow)?;
            if rel >= clip.length {
                break;
            }
            for note in &pattern.notes {
                if note.start.is_negative() || note.start >= plen {
                    continue;
                }
                let local = note.start.checked_add(rel).map_err(overflow)?;
                if local >= clip.length {
                    continue;
                }
                let source_start = clip.start.checked_add(local).map_err(overflow)?;
                let start = source_start.max(clip.start);
                let end = source_start
                    .checked_add(note.length)
                    .map_err(overflow)?
                    .min(clip_end);
                if end <= start {
                    continue;
                }
                let from = start.checked_sub(source_start).map_err(overflow)?;
                let salt = (clip.id.short() ^ (k as u64).wrapping_mul(0x9E37_79B9)).wrapping_add(1);
                fl.push(
                    note,
                    pattern.expression_for(note.id),
                    start,
                    end,
                    from,
                    salt,
                );
            }
            k += 1;
        }
    }
    let end = arrangement.end();
    build(project, opts, fl, end)
}

// ------------------------------------------------------------- encoding

enum Body {
    Name(String),
    Tempo(u32),
    Signature(u8, u8),
    Program(u8, u8),
    Port(u8),
    Controller(u8, u8, u8),
    Pitch(u8, u16),
    Pressure(u8, u8),
    NoteOn(u8, u8, u8),
    NoteOff(u8, u8, u8),
}

struct Ev {
    tick: u64,
    order: u8,
    body: Body,
}

struct Rounding {
    rounded: usize,
    max_error: f64,
}

/// Rounds a position to the nearest tick (halves up). `None` for negative
/// positions.
fn to_ticks(t: Time, ppq: u16, r: &mut Rounding) -> Result<Option<u64>> {
    let scaled = t
        .checked_mul(Time::beats(i64::from(ppq)))
        .map_err(overflow)?;
    let (n, d) = (scaled.numerator(), scaled.denominator());
    if n < 0 {
        return Ok(None);
    }
    let whole = n / d;
    let rem = n % d;
    let ticks = if rem == 0 {
        whole
    } else {
        let up = 2 * rem >= d;
        let err = if up { d - rem } else { rem } as f64 / d as f64;
        r.rounded += 1;
        r.max_error = r.max_error.max(err);
        whole + i128::from(up)
    };
    u64::try_from(ticks)
        .map(Some)
        .map_err(|_| MidiError::Export("position is too large for a MIDI file".into()))
}

fn velocity_byte(v: f32) -> u8 {
    (v * 127.0).round().clamp(1.0, 127.0) as u8
}

/// Inverse of the importer's mapping: 0.5 (the model default) is the
/// conventional release velocity 64.
fn release_byte(r: f32) -> u8 {
    if (r - 0.5).abs() < 1e-6 {
        64
    } else {
        (r * 127.0).round().clamp(0.0, 127.0) as u8
    }
}

fn micros_per_quarter(bpm: f64) -> u32 {
    (60_000_000.0 / bpm).round().clamp(1.0, 16_777_215.0) as u32
}

fn build(
    project: &Project,
    opts: &ExportOptions,
    fl: Flattener<'_>,
    content_end: Time,
) -> Result<MidiExport> {
    if opts.ppq == 0 || opts.ppq > 0x7FFF {
        return Err(MidiError::Export(format!(
            "ticks per quarter note must be 1 to 32767, not {}",
            opts.ppq
        )));
    }
    let ppq = opts.ppq;
    let Flattener {
        mut report, notes, ..
    } = fl;
    let mut rounding = Rounding {
        rounded: 0,
        max_error: 0.0,
    };

    // MPE members share a port only with their own instrument. Track
    // boundaries alone do not isolate MIDI channels in a type 1 file.
    let expressive: std::collections::HashSet<Id> = notes
        .iter()
        .filter(|n| {
            n.fine != 0.0
                || n.expression.as_ref().is_some_and(|e| {
                    !e.pitch.is_empty() || !e.pressure.is_empty() || !e.timbre.is_empty()
                })
        })
        .map(|n| n.channel)
        .collect();
    let included: Vec<_> = project
        .channels
        .iter()
        .filter(|ch| opts.include_empty_channels || notes.iter().any(|n| n.channel == ch.id))
        .collect();
    if opts.format == ExportFormat::Type0 && !expressive.is_empty() && included.len() > 1 {
        return Err(MidiError::Export(
            "type 0 cannot isolate MPE from other instruments; choose type 1 or export one instrument".into(),
        ));
    }
    if expressive.len() > 127 {
        return Err(MidiError::Export(
            "more than 127 MPE instruments need separate MIDI files".into(),
        ));
    }
    const MELODIC: [u8; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];
    let mut next_melodic = 0usize;
    let mut next_port = u8::from(included.iter().any(|ch| !expressive.contains(&ch.id)));
    let mut midi_channel: Vec<(Id, u8, Option<u8>)> = Vec::new();
    for ch in included {
        let is_mpe = expressive.contains(&ch.id);
        let m = if is_mpe {
            0
        } else if (opts.is_drum)(ch) {
            9
        } else {
            let m = MELODIC[next_melodic % MELODIC.len()];
            next_melodic += 1;
            m
        };
        let port = if is_mpe {
            let port = next_port;
            next_port += 1;
            Some(port)
        } else {
            None
        };
        midi_channel.push((ch.id, m, port));
    }
    if next_melodic > MELODIC.len() {
        report.warnings.push(format!(
            "{next_melodic} melodic channels share the 15 non-percussion MIDI channels"
        ));
    }
    if !expressive.is_empty() {
        report.warnings.push(
            "Note expression uses MPE. Type 1 players must honor MIDI Port events to keep instruments separate.".into(),
        );
    }

    // Conductor events: name, meter, tempo.
    let mut conductor: Vec<Ev> = vec![Ev {
        tick: 0,
        order: 0,
        body: Body::Name(if project.meta.title.is_empty() {
            "Peel Studio".to_string()
        } else {
            project.meta.title.clone()
        }),
    }];
    let ts = project.time_signature;
    let (num, den) =
        if ts.numerator > 0 && ts.denominator.is_power_of_two() && ts.denominator <= 128 {
            (ts.numerator, ts.denominator.trailing_zeros() as u8)
        } else {
            report.warnings.push(format!(
                "time signature {}/{} cannot be written to MIDI; wrote 4/4",
                ts.numerator, ts.denominator
            ));
            (4, 2)
        };
    conductor.push(Ev {
        tick: 0,
        order: 0,
        body: Body::Signature(num, den),
    });
    tempo_events(
        project,
        opts,
        ppq,
        &mut rounding,
        &mut report,
        &mut conductor,
    )?;

    // Notes.
    let mut tracks: Vec<(Option<String>, Vec<Ev>)> = Vec::new();
    let mut last_tick: u64 = 0;
    for (channel_id, m, port) in &midi_channel {
        let channel = project.channel(*channel_id).expect("channel exists");
        let mut evs: Vec<Ev> = vec![Ev {
            tick: 0,
            order: 0,
            body: Body::Name(channel.name.clone()),
        }];
        if opts.format == ExportFormat::Type1 && !expressive.is_empty() {
            evs.push(Ev {
                tick: 0,
                order: 0,
                body: Body::Port(port.unwrap_or(0)),
            });
        }
        if port.is_some() {
            setup_mpe(&mut evs);
        }
        if let Some(program) = (opts.program_for_channel)(channel) {
            if port.is_some() {
                for member in 0..16 {
                    evs.push(Ev {
                        tick: 0,
                        order: 0,
                        body: Body::Program(member, program.min(127)),
                    });
                }
            } else {
                evs.push(Ev {
                    tick: 0,
                    order: 0,
                    body: Body::Program(*m, program.min(127)),
                });
            }
        }
        let mut timed = Vec::new();
        for n in notes.iter().filter(|n| n.channel == *channel_id) {
            let Some(start) = to_ticks(n.start, ppq, &mut rounding)? else {
                report.skipped_negative += 1;
                continue;
            };
            let mut end = to_ticks(n.end, ppq, &mut rounding)?.unwrap_or(start);
            if end <= start {
                end = start + 1;
                report.lengthened_notes += 1;
            }
            timed.push((start, end, n));
        }
        timed.sort_by_key(|(start, _, _)| *start);
        let mut member_end = [0u64; 15];
        for (start, end, n) in timed {
            let midi = if port.is_some() {
                let Some((member, _)) = member_end
                    .iter()
                    .enumerate()
                    .filter(|(_, until)| **until <= start)
                    .min_by_key(|(member, until)| (**until, *member))
                else {
                    return Err(MidiError::Export(format!(
                        "MPE instrument '{}' has more than 15 simultaneous notes at tick {start}; reduce its polyphony or split it into instruments",
                        channel.name
                    )));
                };
                member_end[member] = end;
                let midi = member as u8 + 1;
                expression_events(
                    n,
                    midi,
                    start,
                    end,
                    ppq,
                    &mut rounding,
                    &mut report,
                    &mut evs,
                )?;
                report.mpe_notes += 1;
                midi
            } else {
                *m
            };
            evs.push(Ev {
                tick: start,
                order: 4,
                body: Body::NoteOn(midi, n.key, velocity_byte(n.velocity)),
            });
            evs.push(Ev {
                tick: end,
                order: 2,
                body: Body::NoteOff(midi, n.key, release_byte(n.release)),
            });
            last_tick = last_tick.max(end);
            report.notes += 1;
            report.notes_with_pan += usize::from(n.has_pan);
            report.notes_with_fine_pitch += usize::from(n.fine != 0.0);
        }
        tracks.push((None, evs));
    }
    let content_ticks = to_ticks(content_end, ppq, &mut rounding)?.unwrap_or(0);
    let end_tick = content_ticks.max(last_tick);

    report.rounded_values = rounding.rounded;
    report.max_rounding_error_ticks = rounding.max_error;
    if rounding.rounded > 0 {
        report.warnings.push(format!(
            "{} positions did not fall on a tick of the {ppq} PPQ grid and were rounded (largest error {:.3} ticks)",
            rounding.rounded, rounding.max_error
        ));
    }
    if report.notes_with_pan > 0 {
        report.warnings.push(format!(
            "{} notes with per-note pan were exported without pan",
            report.notes_with_pan
        ));
    }
    if report.pitch_values_clamped > 0 {
        report.warnings.push(format!(
            "{} pitch values exceeded the MPE range and were limited to +/-96 semitones",
            report.pitch_values_clamped
        ));
    }
    if report.audio_clips_omitted > 0 {
        report.warnings.push(format!(
            "{} audio clips are not part of a MIDI file",
            report.audio_clips_omitted
        ));
    }
    if report.skipped_probability > 0 || report.probability_ignored > 0 {
        report.warnings.push(format!(
            "probability: {} notes rolled out, {} exported regardless",
            report.skipped_probability, report.probability_ignored
        ));
    }

    // Lay out tracks.
    let mut all: Vec<Vec<Ev>> = Vec::new();
    match opts.format {
        ExportFormat::Type1 => {
            all.push(conductor);
            all.extend(tracks.into_iter().map(|(_, e)| e));
        }
        ExportFormat::Type0 => {
            let mut merged = conductor;
            for (_, evs) in tracks {
                // Channel names have no place in a single combined track.
                merged.extend(evs.into_iter().filter(|e| !matches!(e.body, Body::Name(_))));
            }
            all.push(merged);
        }
    }
    report.tracks = all.len();

    let bytes = encode(&mut all, end_tick, ppq, opts.format)?;
    Ok(MidiExport { bytes, report })
}

/// Configure a lower zone, then explicitly select 96-semitone member bends.
fn setup_mpe(out: &mut Vec<Ev>) {
    let mut rpn = |ch, number, value| {
        for (cc, value) in [
            (101, 0),
            (100, number),
            (6, value),
            (38, 0),
            (101, 127),
            (100, 127),
        ] {
            out.push(Ev {
                tick: 0,
                order: 0,
                body: Body::Controller(ch, cc, value),
            });
        }
    };
    rpn(0, 6, 15);
    rpn(0, 0, 2);
    for ch in 1..16 {
        rpn(ch, 0, 96);
    }
    // A receiver may retain manager controls from the previous performance.
    for body in [
        Body::Pitch(0, 8192),
        Body::Pressure(0, 0),
        Body::Controller(0, 74, 64),
        Body::Controller(0, 64, 0),
    ] {
        out.push(Ev {
            tick: 0,
            order: 0,
            body,
        });
    }
}

fn pitch_byte(value: f32, report: &mut ExportReport) -> u16 {
    report.pitch_values_clamped += usize::from(!(-96.0..=96.0).contains(&value));
    // The two halves of MIDI pitch bend have different cardinalities.
    let value = value.clamp(-96.0, 96.0);
    let scale = if value < 0.0 { 8192.0 } else { 8191.0 };
    (8192.0 + value / 96.0 * scale).round().clamp(0.0, 16383.0) as u16
}

/// CC74 reserves byte 64 for exact neutral, matching the live MPE path.
fn timbre_byte(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.5 {
        (value * 128.0).round() as u8
    } else {
        (64.0 + (value - 0.5) * 126.0).round() as u8
    }
}

fn control_byte(value: f32) -> u8 {
    (value * 127.0).round().clamp(0.0, 127.0) as u8
}

#[allow(clippy::too_many_arguments)]
fn expression_events(
    note: &FlatNote,
    channel: u8,
    start: u64,
    end: u64,
    ppq: u16,
    rounding: &mut Rounding,
    report: &mut ExportReport,
    out: &mut Vec<Ev>,
) -> Result<()> {
    let initial = note
        .expression
        .as_ref()
        .map(|e| e.at(Time::ZERO))
        .unwrap_or_default();
    let fine = note.fine / 100.0;
    for body in [
        Body::Pitch(channel, pitch_byte(initial.pitch + fine, report)),
        Body::Pressure(channel, control_byte(initial.pressure)),
        Body::Controller(channel, 74, timbre_byte(initial.timbre)),
    ] {
        out.push(Ev {
            tick: start,
            order: 3,
            body,
        });
    }
    let Some(expression) = &note.expression else {
        return Ok(());
    };
    for (kind, points) in [
        (0, &expression.pitch),
        (1, &expression.pressure),
        (2, &expression.timbre),
    ] {
        for point in points.iter().filter(|p| p.offset > Time::ZERO) {
            let at = note.start.checked_add(point.offset).map_err(overflow)?;
            let tick = to_ticks(at, ppq, rounding)?
                .unwrap_or(start)
                .clamp(start, end);
            let body = match kind {
                0 => Body::Pitch(channel, pitch_byte(point.value + fine, report)),
                1 => Body::Pressure(channel, control_byte(point.value)),
                _ => Body::Controller(channel, 74, timbre_byte(point.value)),
            };
            // At an onset tick, the final rounded controller wins before On.
            // At an ending tick it still precedes Off and the next note reset.
            out.push(Ev {
                tick,
                order: if tick == start { 3 } else { 1 },
                body,
            });
        }
    }
    Ok(())
}

fn tempo_events(
    project: &Project,
    opts: &ExportOptions,
    ppq: u16,
    rounding: &mut Rounding,
    report: &mut ExportReport,
    out: &mut Vec<Ev>,
) -> Result<()> {
    let mut segments = project.tempo.clone();
    segments.sort_by_key(|s| s.start);
    if segments.is_empty() {
        segments.push(peel_document::TempoSegment {
            start: Time::ZERO,
            bpm: 120.0,
            ramp_to: None,
        });
    }
    if !opts.include_tempo_map {
        segments.truncate(1);
        segments[0].ramp_to = None;
        segments[0].start = Time::ZERO;
    }
    for (i, seg) in segments.iter().enumerate() {
        let start = if i == 0 { Time::ZERO } else { seg.start };
        let next = segments.get(i + 1).map(|n| n.start);
        match (seg.ramp_to, next) {
            (Some(target), Some(next)) if next > start && (target - seg.bpm).abs() > 1e-9 => {
                // Approximate the linear ramp with a step per beat (at most
                // 128 steps), each at the tempo of its midpoint.
                let beats = (next - start).as_f64().ceil().clamp(1.0, 128.0) as i64;
                report.tempo_ramps_approximated += 1;
                for step in 0..beats {
                    let at = start
                        .checked_add(
                            (next - start)
                                .checked_mul(Time::new(step, beats))
                                .map_err(overflow)?,
                        )
                        .map_err(overflow)?;
                    let frac = (step as f64 + 0.5) / beats as f64;
                    let bpm = seg.bpm + (target - seg.bpm) * frac;
                    push_tempo(out, at, bpm, ppq, rounding)?;
                }
            }
            _ => push_tempo(out, start, seg.bpm, ppq, rounding)?,
        }
    }
    Ok(())
}

fn push_tempo(
    out: &mut Vec<Ev>,
    at: Time,
    bpm: f64,
    ppq: u16,
    rounding: &mut Rounding,
) -> Result<()> {
    let tick = to_ticks(at, ppq, rounding)?.unwrap_or(0);
    let us = micros_per_quarter(bpm);
    // Several tempi on one tick: the last one wins.
    out.retain(|e| !(e.tick == tick && matches!(e.body, Body::Tempo(_))));
    out.push(Ev {
        tick,
        order: 0,
        body: Body::Tempo(us),
    });
    Ok(())
}

fn encode(
    tracks: &mut [Vec<Ev>],
    end_tick: u64,
    ppq: u16,
    format: ExportFormat,
) -> Result<Vec<u8>> {
    // Names must outlive the events that borrow them.
    for evs in tracks.iter_mut() {
        evs.sort_by_key(|e| (e.tick, e.order));
    }
    let mut smf_tracks: Vec<Vec<TrackEvent<'_>>> = Vec::with_capacity(tracks.len());
    for evs in tracks.iter() {
        let mut out: Vec<TrackEvent<'_>> = Vec::with_capacity(evs.len() + 1);
        let mut prev = 0u64;
        let delta = |prev: &mut u64, tick: u64| -> Result<u28> {
            let d = tick - *prev;
            *prev = tick;
            u32::try_from(d)
                .ok()
                .and_then(u28::try_from)
                .ok_or_else(|| {
                    MidiError::Export(format!(
                        "a gap of {d} ticks is too long for a MIDI delta time; use a lower PPQ"
                    ))
                })
        };
        for e in evs {
            let kind = match &e.body {
                Body::Name(n) => TrackEventKind::Meta(MetaMessage::TrackName(n.as_bytes())),
                Body::Tempo(us) => TrackEventKind::Meta(MetaMessage::Tempo(u24::new(*us))),
                Body::Signature(n, d) => {
                    TrackEventKind::Meta(MetaMessage::TimeSignature(*n, *d, 24, 8))
                }
                Body::Port(port) => TrackEventKind::Meta(MetaMessage::MidiPort(u7::new(*port))),
                Body::Controller(ch, controller, value) => TrackEventKind::Midi {
                    channel: u4::new(*ch),
                    message: MidiMessage::Controller {
                        controller: u7::new(*controller),
                        value: u7::new(*value),
                    },
                },
                Body::Pitch(ch, value) => TrackEventKind::Midi {
                    channel: u4::new(*ch),
                    message: MidiMessage::PitchBend {
                        bend: midly::PitchBend(midly::num::u14::new(*value)),
                    },
                },
                Body::Pressure(ch, value) => TrackEventKind::Midi {
                    channel: u4::new(*ch),
                    message: MidiMessage::ChannelAftertouch {
                        vel: u7::new(*value),
                    },
                },
                Body::Program(ch, p) => TrackEventKind::Midi {
                    channel: u4::new(*ch),
                    message: MidiMessage::ProgramChange {
                        program: u7::new(*p),
                    },
                },
                Body::NoteOn(ch, key, vel) => TrackEventKind::Midi {
                    channel: u4::new(*ch),
                    message: MidiMessage::NoteOn {
                        key: u7::new(*key),
                        vel: u7::new(*vel),
                    },
                },
                Body::NoteOff(ch, key, vel) => TrackEventKind::Midi {
                    channel: u4::new(*ch),
                    message: MidiMessage::NoteOff {
                        key: u7::new(*key),
                        vel: u7::new(*vel),
                    },
                },
            };
            out.push(TrackEvent {
                delta: delta(&mut prev, e.tick)?,
                kind,
            });
        }
        let end = end_tick.max(prev);
        out.push(TrackEvent {
            delta: delta(&mut prev, end)?,
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });
        smf_tracks.push(out);
    }
    let header = Header::new(
        match format {
            ExportFormat::Type0 => Format::SingleTrack,
            ExportFormat::Type1 => Format::Parallel,
        },
        Timing::Metrical(u15::new(ppq)),
    );
    let smf = Smf {
        header,
        tracks: smf_tracks,
    };
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes)
        .map_err(|e| MidiError::Export(e.to_string()))?;
    Ok(bytes)
}
