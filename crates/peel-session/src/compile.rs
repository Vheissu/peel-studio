//! Document to [`RenderPlan`] compilation.
//!
//! Pattern events are realized at absolute positions per pattern instance
//! and loop iteration (`a + (p - o + k*L)`), so repeated instances never
//! accumulate rounding drift. Every realized note gets a unique voice id.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use peel_document::{ClipContent, Id, Pattern, Project, Time};
use peel_dsp::util::db_to_gain;
use peel_engine::{ChannelRoute, RenderPlan, SeqEvent, SeqKind, Slot, TrackRoute};
use peel_time::TempoMap;

/// What the transport plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayMode {
    /// Loop the selected pattern.
    Pattern(Id),
    /// Play the active arrangement.
    Song,
}

/// Index maps produced alongside a plan, used to address realtime
/// parameter commands.
#[derive(Clone, Debug, Default)]
pub struct PlanIndex {
    pub channels: HashMap<Id, u16>,
    pub tracks: HashMap<Id, u16>,
}

/// Inputs the compiler needs besides the document.
pub struct CompileContext<'a> {
    pub mode: PlayMode,
    /// Engine slot for each device instance id.
    pub slots: &'a HashMap<Id, Slot>,
    /// Stable meter index for each mixer track id.
    pub meters: &'a HashMap<Id, u16>,
}

pub fn compile(p: &Project, ctx: &CompileContext) -> (RenderPlan, PlanIndex) {
    let mut index = PlanIndex::default();

    // ---- mixer tracks in processing order (feeders before destinations) ----
    let master_id = p.mixer.tracks[0].id;
    let depth = |id: Id| -> usize {
        let mut d = 0;
        let mut cur = id;
        while cur != master_id && d < 64 {
            cur = p
                .mixer
                .track(cur)
                .and_then(|t| t.output)
                .unwrap_or(master_id);
            d += 1;
        }
        d
    };
    let mut order: Vec<usize> = (1..p.mixer.tracks.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(depth(p.mixer.tracks[i].id)));
    order.push(0);
    for (pi, &ti) in order.iter().enumerate() {
        index.tracks.insert(p.mixer.tracks[ti].id, pi as u16);
    }
    let any_solo = p.mixer.tracks.iter().any(|t| t.solo);
    // A track is audible under solo if it is soloed or feeds a soloed track.
    let feeds_solo = |id: Id| -> bool {
        let mut cur = Some(id);
        let mut guard = 0;
        while let Some(c) = cur {
            let Some(t) = p.mixer.track(c) else { break };
            if t.solo {
                return true;
            }
            if c == master_id || guard > 64 {
                break;
            }
            cur = Some(t.output.unwrap_or(master_id));
            guard += 1;
        }
        false
    };
    let tracks: Vec<TrackRoute> = order
        .iter()
        .map(|&ti| {
            let t = &p.mixer.tracks[ti];
            let is_master = ti == 0;
            let output = if is_master {
                None
            } else {
                Some(
                    index.tracks[&t
                        .output
                        .filter(|o| index.tracks.contains_key(o))
                        .unwrap_or(master_id)],
                )
            };
            let solo_muted = any_solo && !is_master && !feeds_solo(t.id);
            TrackRoute {
                inserts: t
                    .inserts
                    .iter()
                    .filter(|d| !d.bypass)
                    .filter_map(|d| ctx.slots.get(&d.id).copied())
                    .collect(),
                gain: db_to_gain(t.gain_db),
                pan: t.pan,
                muted: t.muted || solo_muted,
                output,
                meter: ctx.meters.get(&t.id).copied().unwrap_or(0),
            }
        })
        .collect();

    // ---- channels ----
    let mut channels = Vec::with_capacity(p.channels.len());
    for c in &p.channels {
        let Some(&slot) = ctx.slots.get(&c.device.id) else {
            continue;
        };
        index.channels.insert(c.id, channels.len() as u16);
        channels.push(ChannelRoute {
            slot,
            track: index
                .tracks
                .get(&c.track)
                .copied()
                .unwrap_or(index.tracks[&master_id]),
            gain: db_to_gain(c.gain_db),
            pan: c.pan,
            muted: c.muted,
        });
    }

    // ---- sequence ----
    let mut seq = Vec::new();
    let mut voice: u32 = 1;
    let (length, loop_range) = match ctx.mode {
        PlayMode::Pattern(pid) => {
            let len = p.pattern(pid).map_or(Time::beats(4), |pt| pt.length);
            if let Some(pt) = p.pattern(pid) {
                realize(
                    p,
                    pt,
                    Time::ZERO,
                    Time::ZERO,
                    pt.length,
                    &index,
                    0,
                    &mut voice,
                    &mut seq,
                );
            }
            (len.as_f64(), Some((0.0, len.as_f64())))
        }
        PlayMode::Song => {
            let arr = p.arrangement();
            let muted_tracks: Vec<Id> = arr
                .tracks
                .iter()
                .filter(|t| t.muted)
                .map(|t| t.id)
                .collect();
            for clip in &arr.clips {
                if clip.muted || muted_tracks.contains(&clip.track) {
                    continue;
                }
                if let ClipContent::Pattern { pattern } = clip.content
                    && let Some(pt) = p.pattern(pattern)
                {
                    let salt = clip.id.short();
                    realize(
                        p,
                        pt,
                        clip.start,
                        clip.offset,
                        clip.length,
                        &index,
                        salt,
                        &mut voice,
                        &mut seq,
                    );
                }
            }
            let bar = p.time_signature.bar_length();
            let end = arr.end().ceil_to(bar).max(bar);
            let lr = arr
                .loop_range
                .map(|(a, b)| (a.as_f64(), b.as_f64()))
                .unwrap_or((0.0, end.as_f64()));
            (end.as_f64(), Some(lr))
        }
    };
    seq.sort_by(|a: &SeqEvent, b: &SeqEvent| a.beat.total_cmp(&b.beat).then(a.order.cmp(&b.order)));

    let plan = RenderPlan {
        revision: p.revision,
        tempo: TempoMap::from_segments(&p.tempo),
        sequence: seq,
        length_beats: length,
        loop_range,
        channels,
        audio: Vec::new(),
        tracks,
        master: (order.len() - 1) as u16,
        beats_per_bar: (p.time_signature.bar_length().as_f64()
            / p.time_signature.beat_length().as_f64())
        .round()
        .max(1.0) as u32,
    };
    (plan, index)
}

/// Swing offset for a note start (delays off-beat 16ths).
fn swing_offset(start: Time, swing: f32) -> f64 {
    if swing <= 0.0 {
        return 0.0;
    }
    let sixteenth = Time::new(1, 4);
    let in_beat = start.rem_euclid(Time::new(1, 2));
    if in_beat == sixteenth {
        swing.clamp(0.0, 1.0) as f64 / 12.0
    } else {
        0.0
    }
}

/// Deterministic per-note, per-iteration probability decision.
fn passes_probability(seed: u64, note: Id, salt: u64, iteration: i128, prob: f32) -> bool {
    if prob >= 1.0 {
        return true;
    }
    if prob <= 0.0 {
        return false;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (seed, note, salt, iteration).hash(&mut h);
    let v = (h.finish() >> 11) as f64 / (1u64 << 53) as f64;
    v < prob as f64
}

/// Realizes one pattern instance placed at `start` with source `offset`
/// and visible `length`, looping the pattern source.
#[allow(clippy::too_many_arguments)]
fn realize(
    p: &Project,
    pt: &Pattern,
    start: Time,
    offset: Time,
    length: Time,
    index: &PlanIndex,
    salt: u64,
    voice: &mut u32,
    out: &mut Vec<SeqEvent>,
) {
    let l = pt.length;
    if !l.is_positive() || !length.is_positive() {
        return;
    }
    let end = start + length;
    // First iteration whose content can reach the visible window.
    let first_k = offset.checked_div(l).map(|q| q.floor_int()).unwrap_or(0) - 1;
    let mut k = first_k.max(0);
    loop {
        let iter_origin = start - offset + l.mul_int(k as i64);
        if iter_origin >= end {
            break;
        }
        for n in &pt.notes {
            if n.muted {
                continue;
            }
            let Some(&ch) = index.channels.get(&n.channel) else {
                continue;
            };
            // Notes past the source loop length are not played.
            if n.start >= l {
                continue;
            }
            let on = iter_origin + n.start;
            if on < start || on >= end {
                continue;
            }
            if !passes_probability(p.seed, n.id, salt, k, n.probability) {
                continue;
            }
            let note_end = (n.start + n.length).min(l);
            let off = (iter_origin + note_end).min(end);
            if off <= on {
                continue;
            }
            let sw = swing_offset(n.start, p.swing);
            let v = *voice;
            *voice = voice.wrapping_add(1).max(1);
            out.push(SeqEvent {
                beat: on.as_f64() + sw,
                channel: ch,
                order: 1,
                kind: SeqKind::NoteOn {
                    key: n.key,
                    velocity: n.velocity,
                    voice: v,
                },
            });
            out.push(SeqEvent {
                beat: off.as_f64() + sw.min((off - on).as_f64() * 0.5),
                channel: ch,
                order: 0,
                kind: SeqKind::NoteOff {
                    key: n.key,
                    velocity: n.release,
                    voice: v,
                },
            });
        }
        k += 1;
        if k > 1_000_000 {
            break;
        }
    }
}

/// True if two plans differ only in gains, pans and mutes, so realtime
/// commands can update the running plan without a swap.
pub fn same_structure(a: &RenderPlan, b: &RenderPlan) -> bool {
    a.sequence == b.sequence
        && a.tempo == b.tempo
        && a.length_beats == b.length_beats
        && a.loop_range == b.loop_range
        && a.master == b.master
        && a.beats_per_bar == b.beats_per_bar
        && a.channels.len() == b.channels.len()
        && a.tracks.len() == b.tracks.len()
        && a.channels
            .iter()
            .zip(&b.channels)
            .all(|(x, y)| x.slot == y.slot && x.track == y.track)
        && a.tracks
            .iter()
            .zip(&b.tracks)
            .all(|(x, y)| x.inserts == y.inserts && x.output == y.output && x.meter == y.meter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use peel_document::{Arrangement, Channel, Clip, DeviceInstance, MixerTrack, Note, PALETTE};
    use std::sync::Arc;

    fn project() -> (Project, Id, Id) {
        let mut p = Project::empty("t");
        let tr = MixerTrack {
            id: Id::new(),
            name: "A".into(),
            color: PALETTE[0],
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            inserts: vec![],
            output: None,
        };
        let ch = Channel {
            id: Id::new(),
            name: "Kick".into(),
            color: PALETTE[0],
            device: DeviceInstance::new("peel.drum.kick", 1),
            track: tr.id,
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            root_key: 60,
        };
        let cid = ch.id;
        p.mixer.tracks.push(tr);
        p.channels.push(ch);
        let pid = p.patterns[0].id;
        let pat = Arc::make_mut(&mut p.patterns[0]);
        for i in 0..4 {
            pat.notes
                .push(Note::new(cid, Time::beats(i), Time::new(1, 4), 60, 1.0));
        }
        (p, cid, pid)
    }

    fn ctx_for(p: &Project) -> (HashMap<Id, Slot>, HashMap<Id, u16>) {
        let slots = p
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| (c.device.id, i as u32))
            .collect();
        let meters = p
            .mixer
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id, i as u16))
            .collect();
        (slots, meters)
    }

    #[test]
    fn pattern_mode_loops_pattern() {
        let (p, _, pid) = project();
        let (slots, meters) = ctx_for(&p);
        let (plan, _) = compile(
            &p,
            &CompileContext {
                mode: PlayMode::Pattern(pid),
                slots: &slots,
                meters: &meters,
            },
        );
        assert_eq!(plan.sequence.len(), 8);
        assert_eq!(plan.loop_range, Some((0.0, 4.0)));
        assert_eq!(plan.tracks.len(), 2);
        assert_eq!(plan.master, 1);
        assert_eq!(plan.channels[0].track, 0);
    }

    #[test]
    fn song_mode_repeats_long_clips_without_drift() {
        let (mut p, _, pid) = project();
        let lane = p.arrangement().tracks[0].id;
        let arr: &mut Arrangement = p.arrangement_mut();
        arr.clips.push(Clip {
            id: Id::new(),
            track: lane,
            start: Time::beats(8),
            length: Time::beats(400),
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::Pattern { pattern: pid },
        });
        let (slots, meters) = ctx_for(&p);
        let (plan, _) = compile(
            &p,
            &CompileContext {
                mode: PlayMode::Song,
                slots: &slots,
                meters: &meters,
            },
        );
        let ons: Vec<f64> = plan
            .sequence
            .iter()
            .filter(|e| matches!(e.kind, SeqKind::NoteOn { .. }))
            .map(|e| e.beat)
            .collect();
        assert_eq!(ons.len(), 400);
        assert_eq!(ons[0], 8.0);
        assert_eq!(*ons.last().unwrap(), 407.0);
        // Unique voices.
        let mut voices: Vec<u32> = plan
            .sequence
            .iter()
            .filter_map(|e| {
                if let SeqKind::NoteOn { voice, .. } = e.kind {
                    Some(voice)
                } else {
                    None
                }
            })
            .collect();
        voices.sort();
        voices.dedup();
        assert_eq!(voices.len(), 400);
    }

    #[test]
    fn clip_offset_and_trim() {
        let (mut p, _, pid) = project();
        let lane = p.arrangement().tracks[0].id;
        p.arrangement_mut().clips.push(Clip {
            id: Id::new(),
            track: lane,
            start: Time::ZERO,
            length: Time::new(5, 2),
            offset: Time::beats(1),
            muted: false,
            content: ClipContent::Pattern { pattern: pid },
        });
        let (slots, meters) = ctx_for(&p);
        let (plan, _) = compile(
            &p,
            &CompileContext {
                mode: PlayMode::Song,
                slots: &slots,
                meters: &meters,
            },
        );
        let ons: Vec<f64> = plan
            .sequence
            .iter()
            .filter(|e| matches!(e.kind, SeqKind::NoteOn { .. }))
            .map(|e| e.beat)
            .collect();
        // Source beats 1, 2, 3 land at 0, 1, 2; the clip ends at 2.5.
        assert_eq!(ons, vec![0.0, 1.0, 2.0]);
    }

    #[test]
    fn structure_comparison_ignores_gains() {
        let (mut p, _, pid) = project();
        let (slots, meters) = ctx_for(&p);
        let ctx = CompileContext {
            mode: PlayMode::Pattern(pid),
            slots: &slots,
            meters: &meters,
        };
        let (a, _) = compile(&p, &ctx);
        p.mixer.tracks[1].gain_db = -6.0;
        let (b, _) = compile(&p, &ctx);
        assert!(same_structure(&a, &b));
        assert!(a.tracks[0].gain != b.tracks[0].gain);
    }

    #[test]
    fn solo_mutes_other_tracks() {
        let (mut p, _, pid) = project();
        let other = MixerTrack {
            id: Id::new(),
            name: "B".into(),
            color: PALETTE[1],
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: true,
            inserts: vec![],
            output: None,
        };
        p.mixer.tracks.push(other);
        let (slots, meters) = ctx_for(&p);
        let (plan, index) = compile(
            &p,
            &CompileContext {
                mode: PlayMode::Pattern(pid),
                slots: &slots,
                meters: &meters,
            },
        );
        let a = index.tracks[&p.mixer.tracks[1].id] as usize;
        let b = index.tracks[&p.mixer.tracks[2].id] as usize;
        assert!(plan.tracks[a].muted);
        assert!(!plan.tracks[b].muted);
        assert!(!plan.tracks[plan.master as usize].muted);
    }
}
