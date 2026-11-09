use std::collections::{BTreeSet, HashMap};

use peel_document::{ClipContent, DeviceInstance, Id, Project};
use peel_engine::{EngineCommand, EngineConfig, OfflineEngine, Slot};

use crate::compile::{CompileContext, PlayMode, compile};

const SONGS: [super::Template; 5] = [
    ("Rap", super::rap),
    ("Trap", super::trap),
    ("Hip-hop", super::hip_hop),
    ("Synthwave", super::synthwave),
    ("EDM", super::edm),
];

fn is_real(dev: &DeviceInstance) -> Result<(), String> {
    let desc = peel_devices::descriptor(&dev.type_id).ok_or(dev.type_id.clone())?;
    for key in dev.params.keys() {
        if desc.param_by_key(key).is_none() {
            return Err(format!("{}: unknown param {key}", dev.type_id));
        }
    }
    match &dev.preset {
        Some(name)
            if !peel_devices::presets(&dev.type_id)
                .iter()
                .any(|p| p.name == name) =>
        {
            Err(format!("{}: unknown preset {name}", dev.type_id))
        }
        _ => Ok(()),
    }
}

fn all_devices(p: &Project) -> impl Iterator<Item = &DeviceInstance> {
    p.channels
        .iter()
        .map(|c| &c.device)
        .chain(p.mixer.tracks.iter().flat_map(|t| t.inserts.iter()))
}

/// A note as it sounds in the song: device type, beat and key.
#[derive(Clone, Debug)]
struct Hit {
    device: String,
    beat: f64,
    len: f64,
    key: u8,
}

/// Every note the arrangement plays, with pattern clips looped out.
fn hits(p: &Project) -> Vec<Hit> {
    let mut out = Vec::new();
    for clip in &p.arrangement().clips {
        let ClipContent::Pattern { pattern } = &clip.content else {
            continue;
        };
        let pat = p.patterns.iter().find(|x| x.id == *pattern).unwrap();
        let (start, end) = (clip.start.as_f64(), clip.end().as_f64());
        let plen = pat.length.as_f64();
        let mut base = start - clip.offset.as_f64();
        while base < end {
            for n in &pat.notes {
                let beat = base + n.start.as_f64();
                if n.start.as_f64() < plen && beat >= start && beat < end {
                    let device = p.channel(n.channel).unwrap().device.type_id.clone();
                    let len = n.length.as_f64();
                    out.push(Hit {
                        device,
                        beat,
                        len,
                        key: n.key,
                    });
                }
            }
            base += plen;
        }
    }
    out.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    out
}

/// Beat range of the `nth` section called `name`.
fn section(p: &Project, name: &str, nth: usize) -> (f64, f64) {
    let arr = p.arrangement();
    let mut marks: Vec<_> = arr.markers.iter().collect();
    marks.sort_by_key(|m| m.position);
    let i = marks
        .iter()
        .enumerate()
        .filter(|(_, m)| m.name == name)
        .nth(nth)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("no section {name}"));
    let end = marks
        .get(i + 1)
        .map_or(arr.end().as_f64(), |m| m.position.as_f64());
    (marks[i].position.as_f64(), end)
}

fn in_section(p: &Project, name: &str, device: &str) -> Vec<Hit> {
    let (a, b) = section(p, name, 0);
    hits(p)
        .into_iter()
        .filter(|h| h.beat >= a && h.beat < b && h.device == device)
        .collect()
}

fn onsets_per_bar(p: &Project, name: &str) -> f64 {
    let (a, b) = section(p, name, 0);
    let n = hits(p).iter().filter(|h| h.beat >= a && h.beat < b).count();
    n as f64 / ((b - a) / 4.0)
}

/// The main section: where each style is at its fullest.
fn peak_section(name: &str) -> &'static str {
    match name {
        "Synthwave" => "Chorus",
        "EDM" => "Drop",
        _ => "Hook",
    }
}

const DRUMS: [&str; 5] = [
    "peel.drum.kick",
    "peel.drum.snare",
    "peel.drum.hat",
    "peel.drum.clap",
    "peel.drum.tom",
];

#[test]
fn catalog_keeps_its_order_and_offers_every_style() {
    let names: Vec<_> = super::templates().iter().map(|t| t.0).collect();
    assert_eq!(
        &names[..4],
        ["Demo song", "Beat and melody", "House", "Hip-hop"]
    );
    for style in ["Rap", "Trap", "Synthwave", "EDM"] {
        assert!(names.contains(&style), "{style}");
    }
}

#[test]
fn every_template_is_valid_and_uses_real_devices() {
    for (name, make) in super::templates() {
        let p = make();
        let issues = peel_document::validate(&p);
        assert!(issues.iter().all(|i| !i.is_error()), "{name}: {issues:?}");
        assert!(!p.arrangement().clips.is_empty(), "{name}");
        for dev in all_devices(&p) {
            assert!(!dev.type_id.starts_with("clap:"), "{name}: {}", dev.type_id);
            is_real(dev).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
}

#[test]
fn song_instruments_and_effects_all_load_a_factory_preset() {
    // `device()` silently skips a misspelled preset, so check it stuck.
    for (name, make) in SONGS {
        for dev in all_devices(&make()) {
            assert!(
                dev.preset.is_some(),
                "{name}: {} has no preset",
                dev.type_id
            );
        }
    }
}

#[test]
fn beat_and_melody_layout_is_stable() {
    let p = super::beat_and_melody();
    assert_eq!(p.channels.len(), 7);
    assert_eq!(p.patterns.len(), 3);
}

#[test]
fn blank_project_is_truly_empty() {
    let p = super::empty();
    assert!(peel_document::validate(&p).iter().all(|i| !i.is_error()));
    assert!(p.channels.is_empty());
    assert_eq!(p.mixer.tracks.len(), 1, "only the master");
    assert!(p.mixer.tracks[0].inserts.is_empty());
    assert!(p.mixer.tracks[0].sends.is_empty());
    assert_eq!(p.patterns.len(), 1);
    assert!(p.patterns[0].notes.is_empty());
    assert!(p.patterns[0].expression.is_empty());
    let arr = p.arrangement();
    assert!(arr.clips.is_empty() && arr.markers.is_empty());
    assert!(p.assets.is_empty());
}

#[test]
fn songs_are_arranged_in_sections_with_bass_and_harmony() {
    for (name, make) in SONGS {
        let p = make();
        let arr = p.arrangement();
        let bars = arr.end().as_f64() / 4.0;
        assert!((32.0..=48.0).contains(&bars), "{name}: {bars} bars");
        let names: Vec<_> = arr.markers.iter().map(|m| m.name.as_str()).collect();
        assert!(names.len() >= 4, "{name}: {names:?}");
        assert_eq!(names.first(), Some(&"Intro"), "{name}");
        assert!(names.contains(&peak_section(name)), "{name}: {names:?}");

        // Every section has something playing and every pattern is used.
        let all = hits(&p);
        for m in &arr.markers {
            let (a, b) = section(&p, &m.name, 0);
            assert!(
                all.iter().any(|h| h.beat >= a && h.beat < b),
                "{name}: {} is empty",
                m.name
            );
        }
        for pat in &p.patterns {
            assert!(!pat.notes.is_empty(), "{name}: {}", pat.name);
            let used = arr.clips.iter().any(
                |c| matches!(c.content, ClipContent::Pattern { pattern } if pattern == pat.id),
            );
            assert!(used, "{name}: {} is never placed", pat.name);
        }

        // The full section has drums, a bass part below C3 and chords.
        let (a, b) = section(&p, peak_section(name), 0);
        let peak: Vec<_> = all.iter().filter(|h| h.beat >= a && h.beat < b).collect();
        let tonal: Vec<_> = peak
            .iter()
            .filter(|h| !DRUMS.contains(&h.device.as_str()))
            .collect();
        assert!(peak.iter().any(|h| h.device == "peel.drum.kick"), "{name}");
        assert!(tonal.iter().any(|h| h.key < 48), "{name}: no bass");
        let chord = tonal.iter().any(|h| {
            let stack = tonal
                .iter()
                .filter(|o| o.device == h.device && (o.beat - h.beat).abs() < 0.05)
                .count();
            stack >= 3
        });
        assert!(chord, "{name}: no chords");
        assert!(
            onsets_per_bar(&p, peak_section(name)) > onsets_per_bar(&p, "Intro"),
            "{name}"
        );
    }
}

fn beat_in_bar(h: &Hit) -> f64 {
    h.beat.rem_euclid(4.0)
}

#[test]
fn rap_verse_leaves_room_for_a_vocal() {
    let p = super::rap();
    let verse = onsets_per_bar(&p, "Verse");
    let hook = onsets_per_bar(&p, "Hook");
    assert!(
        verse < hook * 0.6,
        "verse {verse} vs hook {hook} notes per bar"
    );
    let (a, b) = section(&p, "Verse", 0);
    assert!(b - a >= 16.0 * 4.0, "a full sixteen-bar verse");
    assert!(in_section(&p, "Verse", "peel.drum.clap").is_empty());
    assert!(!in_section(&p, "Hook", "peel.drum.clap").is_empty());
}

#[test]
fn trap_snare_is_half_time_and_hats_roll() {
    let p = super::trap();
    let snares = in_section(&p, "Hook", "peel.drum.snare");
    assert!(!snares.is_empty());
    assert!(
        snares.iter().all(|h| (beat_in_bar(h) - 2.0).abs() < 1e-9),
        "snare off beat three"
    );

    let hats: Vec<f64> = in_section(&p, "Hook", "peel.drum.hat")
        .iter()
        .map(|h| h.beat)
        .collect();
    let gaps: BTreeSet<i64> = hats
        .windows(2)
        .map(|w| ((w[1] - w[0]) * 96.0).round() as i64)
        .filter(|&g| g > 0)
        .collect();
    assert!(
        gaps.iter().any(|&g| g < 24),
        "no roll faster than sixteenths: {gaps:?}"
    );
    assert!(
        gaps.iter().any(|&g| g % 24 != 0 && g % 12 != 0),
        "no triplet roll: {gaps:?}"
    );

    let low = in_section(&p, "Hook", "peel.synth.bass");
    assert!(
        low.iter().map(|h| h.key).min() < Some(36),
        "808 sits in the sub range"
    );
}

#[test]
fn hip_hop_is_a_swung_boom_bap() {
    let p = super::hip_hop();
    assert!(p.swing > 0.3);
    let snares = in_section(&p, "Verse", "peel.drum.snare");
    let backbeat = snares
        .iter()
        .filter(|h| [1.0, 3.0].contains(&beat_in_bar(h)))
        .count();
    assert!(backbeat * 10 >= snares.len() * 8, "snare on two and four");
    let kicks = in_section(&p, "Verse", "peel.drum.kick");
    let off_grid = kicks
        .iter()
        .filter(|h| beat_in_bar(h).fract() != 0.0)
        .count();
    assert!(off_grid > 0, "kick is syncopated, not four on the floor");
    let names: Vec<_> = p
        .arrangement()
        .markers
        .iter()
        .map(|m| m.name.clone())
        .collect();
    assert_eq!(
        names.iter().filter(|n| *n == "Hook").count(),
        2,
        "{names:?}"
    );
}

#[test]
fn synthwave_drives_with_eighths_and_a_sixteenth_arp() {
    let p = super::synthwave();
    let hats = in_section(&p, "Verse", "peel.drum.hat");
    let eighths = hats
        .iter()
        .filter(|h| (h.beat * 2.0).fract() == 0.0)
        .count();
    assert!(eighths * 10 >= hats.len() * 9);
    // Something plays every sixteenth through the chorus.
    let (a, b) = section(&p, "Chorus", 0);
    let starts: BTreeSet<i64> = hits(&p)
        .iter()
        .filter(|h| h.beat >= a && h.beat < b && h.device == "peel.synth.poly" && h.len <= 0.25)
        .map(|h| (h.beat * 4.0).round() as i64)
        .collect();
    assert_eq!(starts.len() as f64, (b - a) * 4.0, "arp is continuous");
}

#[test]
fn edm_builds_into_a_four_on_the_floor_drop() {
    let p = super::edm();
    let (drop_a, drop_b) = section(&p, "Drop", 0);
    let kicks = in_section(&p, "Drop", "peel.drum.kick");
    assert_eq!(kicks.len() as f64, drop_b - drop_a, "a kick on every beat");
    let (_, build_end) = section(&p, "Build", 0);
    assert_eq!(build_end, drop_a, "the build leads straight into the drop");
    assert!(in_section(&p, "Build", "peel.drum.kick").is_empty());

    // The snare roll speeds up through the build.
    let roll: Vec<f64> = in_section(&p, "Build", "peel.drum.snare")
        .iter()
        .map(|h| h.beat)
        .collect();
    let first = roll[1] - roll[0];
    let last = roll[roll.len() - 1] - roll[roll.len() - 2];
    assert!(last < first / 4.0, "{first} -> {last}");

    // Bass and chords pump under the kick.
    let kick_track = p
        .channels
        .iter()
        .find(|c| c.device.type_id == "peel.drum.kick")
        .unwrap()
        .track;
    let keyed = p
        .mixer
        .tracks
        .iter()
        .filter(|t| t.inserts.iter().any(|d| d.sidechain == Some(kick_track)))
        .count();
    assert!(keyed >= 2);
}

#[test]
fn styles_differ_in_groove_and_sound() {
    let mut grooves = Vec::new();
    let mut palettes = Vec::new();
    for (name, make) in SONGS {
        let p = make();
        let groove: BTreeSet<(String, i64)> = in_section_all(&p, peak_section(name))
            .into_iter()
            .filter(|h| DRUMS.contains(&h.device.as_str()))
            .map(|h| (h.device, (beat_in_bar_raw(h.beat) * 48.0).round() as i64))
            .collect();
        let sounds: BTreeSet<(String, Option<String>)> = p
            .channels
            .iter()
            .map(|c| (c.device.type_id.clone(), c.device.preset.clone()))
            .collect();
        grooves.push((name, groove));
        palettes.push((name, sounds));
    }
    for i in 0..SONGS.len() {
        for j in i + 1..SONGS.len() {
            assert_ne!(
                grooves[i].1, grooves[j].1,
                "{} and {}",
                grooves[i].0, grooves[j].0
            );
            let shared = palettes[i].1.intersection(&palettes[j].1).count();
            let smaller = palettes[i].1.len().min(palettes[j].1.len());
            assert!(
                shared * 2 < smaller,
                "{} and {} share most sounds",
                palettes[i].0,
                palettes[j].0
            );
        }
    }
}

fn beat_in_bar_raw(beat: f64) -> f64 {
    beat.rem_euclid(8.0)
}

fn in_section_all(p: &Project, name: &str) -> Vec<Hit> {
    let (a, b) = section(p, name, 0);
    hits(p)
        .into_iter()
        .filter(|h| h.beat >= a && h.beat < b)
        .collect()
}

/// Renders `seconds` of the song from `beat` through the real engine.
fn render(p: &Project, beat: f64, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let rate = 44_100.0;
    let config = EngineConfig {
        max_block: 512,
        ..EngineConfig::default()
    };
    let mut engine = OfflineEngine::new(rate, &config);
    let mut slots: HashMap<Id, Slot> = HashMap::new();
    for (i, inst) in all_devices(p).enumerate() {
        let mut device = peel_devices::create(&inst.type_id).unwrap();
        let desc = device.descriptor();
        for (k, v) in &inst.params {
            if let Some(param) = desc.param_by_key(k) {
                device.set_param(param.id, *v);
            }
        }
        device.prepare(rate, config.max_block);
        engine.send(EngineCommand::InstallDevice {
            slot: i as Slot,
            device,
        });
        slots.insert(inst.id, i as Slot);
    }
    let meters: HashMap<Id, u16> = p
        .mixer
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id, i as u16))
        .collect();
    let samples = HashMap::new();
    let (mut plan, _) = compile(
        p,
        &CompileContext {
            mode: PlayMode::Song,
            slots: &slots,
            meters: &meters,
            samples: &samples,
        },
    );
    plan.loop_range = None;
    engine.try_set_plan(Box::new(plan)).unwrap();
    engine.send(EngineCommand::Seek { beat });
    engine.send(EngineCommand::Play);
    let frames = (rate * seconds) as usize;
    let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
    for (lc, rc) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        engine.render(lc, rc);
        engine.drain_retired();
    }
    (l, r)
}

fn peak_and_rms(l: &[f32], r: &[f32]) -> (f32, f32) {
    let peak = l.iter().chain(r).fold(0.0f32, |m, s| m.max(s.abs()));
    let sum: f64 = l
        .iter()
        .chain(r)
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum();
    (peak, (sum / (l.len() + r.len()) as f64).sqrt() as f32)
}

#[test]
fn every_song_section_sounds_with_headroom() {
    let db = |x: f32| 20.0 * x.max(1e-9).log10();
    for (name, make) in SONGS {
        // Without the master limiter, so it cannot hide a hot mix.
        let mut p = make();
        p.mixer.tracks[0].inserts.clear();
        let mut levels = Vec::new();
        for m in &p.arrangement().markers {
            let (l, r) = render(&p, m.position.as_f64(), 2.0);
            assert!(
                l.iter().chain(&r).all(|s| s.is_finite()),
                "{name} {}",
                m.name
            );
            let (peak, rms) = peak_and_rms(&l, &r);
            assert!(
                db(peak) < -2.5,
                "{name} {} peaks at {:.1} dB",
                m.name,
                db(peak)
            );
            levels.push((m.name.clone(), db(rms)));
        }
        let loudest = levels.iter().map(|l| l.1).fold(f32::MIN, f32::max);
        for (section, rms) in &levels {
            assert!(
                loudest - rms < 18.0,
                "{name} {section} is {rms:.1} dB vs {loudest:.1} dB"
            );
        }
    }
}
