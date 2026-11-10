use std::collections::{BTreeSet, HashMap};

use peel_document::{ClipContent, DeviceInstance, Id, Project, Time};
use peel_engine::{EngineCommand, EngineConfig, OfflineEngine, Slot};

use crate::compile::{CompileContext, PlayMode, compile};

const SONGS: [super::Template; 6] = [
    ("Rap", super::rap),
    ("Trap", super::trap),
    ("Hip-hop", super::hip_hop),
    ("Synthwave", super::synthwave),
    ("EDM", super::edm),
    ("Rock / Metal", super::rock_metal),
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
    channel: Id,
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
                        channel: n.channel,
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

fn channel_in_section(p: &Project, section_name: &str, channel_name: &str) -> Vec<Hit> {
    let channel = p
        .channels
        .iter()
        .find(|c| c.name == channel_name)
        .unwrap()
        .id;
    let (a, b) = section(p, section_name, 0);
    hits(p)
        .into_iter()
        .filter(|h| h.beat >= a && h.beat < b && h.channel == channel)
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
        "Synthwave" | "Rock / Metal" => "Chorus",
        "EDM" => "Drop",
        _ => "Hook",
    }
}

const DRUMS: [&str; 6] = [
    "peel.drum.acoustic",
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
    assert_eq!(
        names.last(),
        Some(&"Rock / Metal"),
        "appended, not inserted"
    );
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
        let kick = p.channels.iter().find(|c| c.name == "Kick").unwrap().id;
        assert!(peak.iter().any(|h| h.channel == kick), "{name}");
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

    let low = in_section(&p, "Hook", "peel.bass.808");
    assert!(
        low.iter().map(|h| h.key).min() < Some(36),
        "808 sits in the sub range"
    );
    // The jump at the end of each hook bar is a slide: the note before it
    // is still held when it starts.
    let slides = low
        .windows(2)
        .filter(|w| w[0].beat + w[0].len > w[1].beat && w[1].key == w[0].key + 12)
        .count();
    assert!(slides >= 8, "{slides} octave slides in the hook");
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

/// Notes of the pattern called `name` on the channel called `channel`.
fn pattern_notes(p: &Project, name: &str, channel: &str) -> Vec<(Time, Time)> {
    let pat = p.patterns.iter().find(|x| x.name == name).unwrap();
    let ch = p.channels.iter().find(|c| c.name == channel).unwrap().id;
    let mut v: Vec<_> = pat
        .notes
        .iter()
        .filter(|n| n.channel == ch)
        .map(|n| (n.start, n.length))
        .collect();
    v.sort();
    v
}

#[test]
fn rock_metal_starts_on_a_sixteenth_triplet_double_kick() {
    let p = super::rock_metal();
    for (name, preset) in [
        ("Kick", "Kick"),
        ("Snare", "Snare"),
        ("Hat", "Closed Hat"),
        ("Crash", "Crash"),
        ("Toms", "Floor Tom"),
    ] {
        let channel = p.channels.iter().find(|c| c.name == name).unwrap();
        assert_eq!(channel.device.type_id, "peel.drum.acoustic");
        assert_eq!(channel.device.preset.as_deref(), Some(preset));
        assert!(
            channel.device.sample.is_none(),
            "recordings are embedded, not external assets"
        );
    }
    assert_eq!(p.bpm(), 108.0);
    assert_eq!(p.swing, 0.0);
    assert_eq!(
        (p.time_signature.numerator, p.time_signature.denominator),
        (4, 4)
    );

    // The first pattern is the one-bar groove the editor opens on.
    let first = &p.patterns[0];
    assert_eq!(first.name, "Double kick");
    assert_eq!(first.length, Time::beats(4));
    let starter = super::starter("Rock / Metal").unwrap();
    assert_eq!(starter.pattern, first.name);
    assert_eq!(starter.grid, Time::new(1, 6));
    assert!(p.channels.iter().any(|c| c.name == starter.channel));
    assert!(super::starter("Synthwave").is_none());

    // Six kicks a sixteenth triplet apart on beats one and three.
    let triplet = Time::new(1, 6);
    let want: Vec<(Time, Time)> = [0, 2]
        .into_iter()
        .flat_map(|beat| (0..6).map(move |i| (Time::beats(beat) + triplet.mul_int(i), triplet)))
        .collect();
    assert_eq!(pattern_notes(&p, "Double kick", "Kick"), want);

    let at = |beats: &[i64]| -> Vec<Time> { beats.iter().map(|&b| Time::beats(b)).collect() };
    let starts = |ch: &str| -> Vec<Time> {
        pattern_notes(&p, "Double kick", ch)
            .into_iter()
            .map(|n| n.0)
            .collect()
    };
    assert_eq!(starts("Snare"), at(&[1, 3]), "snare on two and four");
    assert_eq!(starts("Hat"), at(&[0, 1, 2, 3]), "a hat on every beat");
    let used: BTreeSet<&str> = first
        .notes
        .iter()
        .map(|n| p.channel(n.channel).unwrap().name.as_str())
        .collect();
    assert_eq!(used, BTreeSet::from(["Kick", "Snare", "Hat"]));

    // The chorus plays it: twelve kicks a bar.
    let (a, b) = section(&p, "Chorus", 0);
    let kicks = channel_in_section(&p, "Chorus", "Kick");
    assert_eq!(kicks.len() as f64, (b - a) / 4.0 * 12.0);
}

#[test]
fn rock_metal_verse_is_a_simpler_rock_beat() {
    let p = super::rock_metal();
    let (a, b) = section(&p, "Verse", 0);
    let bars = (b - a) / 4.0;
    let kicks = channel_in_section(&p, "Verse", "Kick");
    assert!(!kicks.is_empty());
    assert!(
        (kicks.len() as f64) < bars * 6.0,
        "{} kicks over {bars} bars",
        kicks.len()
    );
    assert!(
        kicks.iter().all(|h| (h.beat * 4.0).fract() == 0.0),
        "verse kicks stay on the sixteenth grid"
    );
    let snares = channel_in_section(&p, "Verse", "Snare");
    let backbeat = snares
        .iter()
        .filter(|h| [1.0, 3.0].contains(&beat_in_bar(h)))
        .count();
    assert!(backbeat * 10 >= snares.len() * 8, "snare on two and four");
    let hats = channel_in_section(&p, "Verse", "Hat");
    assert!(
        hats.iter().all(|h| (h.beat * 2.0).fract() == 0.0),
        "eighth hats"
    );
    assert!(hats.len() as f64 >= bars * 7.0);
}

#[test]
fn rock_metal_has_distorted_guitar_and_bass_that_change_by_section() {
    let p = super::rock_metal();
    let presets: BTreeSet<&str> = p
        .channels
        .iter()
        .filter(|c| {
            matches!(
                c.device.type_id.as_str(),
                "peel.guitar.recorded" | "peel.guitar.palm_muted"
            )
        })
        .filter_map(|c| c.device.preset.as_deref())
        .collect();
    assert_eq!(presets, BTreeSet::from(["High Gain", "Metal Chugs"]));

    // Chorus power chords: root, fifth and octave struck together.
    let chorus = in_section(&p, "Chorus", "peel.guitar.recorded");
    let power = chorus.iter().any(|h| {
        let stack: BTreeSet<u8> = chorus
            .iter()
            .filter(|o| (o.beat - h.beat).abs() < 1e-9)
            .map(|o| o.key)
            .collect();
        stack == BTreeSet::from([h.key, h.key + 7, h.key + 12])
    });
    assert!(power, "no power chords in the chorus");

    // Each main section has its own guitar part and a bass under it.
    let mut parts = Vec::new();
    for name in ["Verse", "Chorus", "Breakdown"] {
        let (a, _) = section(&p, name, 0);
        let gtr: BTreeSet<(i64, u8)> = in_section(&p, name, "peel.guitar.recorded")
            .iter()
            .chain(in_section(&p, name, "peel.guitar.palm_muted").iter())
            .filter(|h| h.beat < a + 8.0)
            .map(|h| (((h.beat - a) * 12.0).round() as i64, h.key))
            .collect();
        assert!(!gtr.is_empty(), "{name}: no guitar");
        let bass = in_section(&p, name, "peel.bass.recorded");
        assert!(bass.iter().any(|h| h.key < 40), "{name}: no low bass");
        parts.push((name, gtr));
    }
    for i in 0..parts.len() {
        for j in i + 1..parts.len() {
            assert_ne!(parts[i].1, parts[j].1, "{} and {}", parts[i].0, parts[j].0);
        }
    }

    // The breakdown kick locks to the chugs.
    let chugs: BTreeSet<i64> = in_section(&p, "Breakdown", "peel.guitar.palm_muted")
        .iter()
        .map(|h| (h.beat * 12.0).round() as i64)
        .collect();
    let kicks: BTreeSet<i64> = channel_in_section(&p, "Breakdown", "Kick")
        .iter()
        .map(|h| (h.beat * 12.0).round() as i64)
        .collect();
    assert_eq!(kicks, chugs);
}

// ---------------------------------------------------------------- demos

fn demo(title: &str) -> Project {
    let d = super::demos()
        .into_iter()
        .find(|d| d.title == title)
        .unwrap_or_else(|| panic!("no demo called {title}"));
    (d.make)()
}

fn channel_named<'a>(p: &'a Project, name: &str) -> &'a peel_document::Channel {
    p.channels
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no channel {name}"))
}

fn track_named(p: &Project, name: &str) -> Id {
    p.mixer
        .tracks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no mixer track {name}"))
        .id
}

/// The project with only `track` soloed and the master chain removed.
fn soloed(p: &Project, track: &str) -> Project {
    let mut p = p.clone();
    let id = track_named(&p, track);
    p.mixer.tracks[0].inserts.clear();
    p.mixer.track_mut(id).unwrap().solo = true;
    p
}

/// Frequency of `buf` from its rising zero crossings.
fn crossing_hz(buf: &[f32], rate: f32) -> f32 {
    let mut times = Vec::new();
    for i in 1..buf.len() {
        if buf[i - 1] < 0.0 && buf[i] >= 0.0 {
            times.push(i as f32 - 1.0 - buf[i - 1] / (buf[i] - buf[i - 1]));
        }
    }
    assert!(times.len() >= 2, "too few cycles");
    rate * (times.len() - 1) as f32 / (times[times.len() - 1] - times[0])
}

#[test]
fn demo_catalog_opens_with_afterglow_and_describes_each_song() {
    let demos = super::demos();
    let titles: Vec<_> = demos.iter().map(|d| d.title).collect();
    assert_eq!(
        titles,
        ["Afterglow", "Night Shift", "Neon Coast", "Corner Store"]
    );
    for d in &demos {
        let p = (d.make)();
        assert_eq!(p.meta.title, d.title);
        assert!((p.bpm() - f64::from(d.bpm)).abs() < 1e-6, "{}", d.title);
        assert!(!d.style.is_empty() && !d.key.is_empty(), "{}", d.title);
        assert!(!p.meta.notes.is_empty(), "{}", d.title);
    }
    // The first demo is still the template catalog's "Demo song".
    let (name, make) = super::templates()[0];
    assert_eq!(name, "Demo song");
    assert_eq!(make().meta.title, demos[0].title);
}

#[test]
fn every_demo_is_valid_and_uses_real_devices_and_presets() {
    for d in super::demos() {
        let p = (d.make)();
        let issues = peel_document::validate(&p);
        assert!(
            issues.iter().all(|i| !i.is_error()),
            "{}: {issues:?}",
            d.title
        );
        for dev in all_devices(&p) {
            is_real(dev).unwrap_or_else(|e| panic!("{}: {e}", d.title));
        }
        for c in &p.channels {
            assert!(c.device.preset.is_some(), "{}: {}", d.title, c.name);
        }
    }
}

#[test]
fn every_demo_is_a_mixed_and_arranged_song() {
    for d in super::demos() {
        let p = (d.make)();
        let arr = p.arrangement();
        let bars = arr.end().as_f64() / 4.0;
        assert!(bars >= 48.0, "{}: only {bars} bars", d.title);
        let mut marks: Vec<_> = arr.markers.iter().collect();
        marks.sort_by_key(|m| m.position);
        assert!(marks.len() >= 5, "{}: {} sections", d.title, marks.len());
        assert_eq!(marks[0].name, "Intro", "{}", d.title);
        assert_eq!(marks[0].position, Time::ZERO, "{}", d.title);

        // Every pattern is placed, and every lane in use is named.
        for pat in &p.patterns {
            let used = arr.clips.iter().any(
                |c| matches!(&c.content, ClipContent::Pattern { pattern } if *pattern == pat.id),
            );
            assert!(used, "{}: {} is never placed", d.title, pat.name);
            assert!(!pat.notes.is_empty(), "{}: {}", d.title, pat.name);
        }
        for clip in &arr.clips {
            let lane = arr.tracks.iter().find(|t| t.id == clip.track).unwrap();
            assert!(!lane.name.is_empty(), "{}: unnamed lane", d.title);
        }

        // A real mix: buses, returns fed by sends, a keyed effect,
        // automation and a master chain that ends in a limiter.
        let buses = p.mixer.tracks.len() - 1 - p.channels.len();
        assert!(buses >= 3, "{}: {buses} buses", d.title);
        let sends: usize = p.mixer.tracks.iter().map(|t| t.sends.len()).sum();
        assert!(sends >= 6, "{}: {sends} sends", d.title);
        assert!(
            all_devices(&p).any(|dev| dev.sidechain.is_some()),
            "{}: nothing is keyed",
            d.title
        );
        let automation = arr
            .clips
            .iter()
            .filter(|c| matches!(c.content, ClipContent::Automation { .. }))
            .count();
        assert!(
            automation >= 3,
            "{}: {automation} automation clips",
            d.title
        );
        let last = p.mixer.tracks[0].inserts.last().expect("master chain");
        assert!(
            matches!(
                last.type_id.as_str(),
                "peel.fx.mastering" | "peel.fx.limiter"
            ),
            "{}: master ends in {}",
            d.title,
            last.type_id
        );
    }
}

#[test]
fn every_demo_section_sounds_with_headroom_and_stays_under_the_ceiling() {
    let db = |x: f32| 20.0 * x.max(1e-9).log10();
    for d in super::demos() {
        let p = (d.make)();
        let mut dry = p.clone();
        dry.mixer.tracks[0].inserts.clear();
        let mut levels = Vec::new();
        for m in &p.arrangement().markers {
            // Two bars into the section, past any filtered pickup.
            let at = m.position.as_f64() + 8.0;
            let (l, r) = render(&dry, at, 2.0);
            assert!(
                l.iter().chain(&r).all(|s| s.is_finite()),
                "{} {}",
                d.title,
                m.name
            );
            let (peak, rms) = peak_and_rms(&l, &r);
            // Afterglow predates this rule and runs a little hotter.
            let limit = if d.title == "Afterglow" { 3.0 } else { -0.5 };
            assert!(
                db(peak) < limit,
                "{} {} peaks at {:.1} dB before the master chain",
                d.title,
                m.name,
                db(peak)
            );
            assert!(
                db(rms) > -40.0,
                "{} {} is nearly silent ({:.1} dB)",
                d.title,
                m.name,
                db(rms)
            );
            levels.push(db(rms));
            // Through the master chain nothing passes the limiter.
            let (l, r) = render(&p, at, 2.0);
            let (peak, _) = peak_and_rms(&l, &r);
            assert!(
                db(peak) < -0.2,
                "{} {} leaves the master at {:.1} dB",
                d.title,
                m.name,
                db(peak)
            );
        }
        let loudest = levels.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            loudest > -24.0,
            "{}: loudest section {loudest:.1} dB",
            d.title
        );
    }
}

#[test]
fn night_shift_slides_its_808_between_overlapping_notes() {
    let p = demo("Night Shift");
    let sub = channel_named(&p, "808");
    assert_eq!(sub.device.type_id, "peel.bass.808");
    assert!(sub.device.params["glide.mode"] >= 1.0, "the 808 must glide");
    assert!(sub.device.params["glide.time"] > 0.05);

    // In the hook pattern, a note that runs into the next one on another
    // key is a slide; every other note stops before its neighbour starts.
    let pat = p.patterns.iter().find(|x| x.name == "Hook 808").unwrap();
    let notes: Vec<_> = pat.notes.iter().filter(|n| n.channel == sub.id).collect();
    let mut slides = Vec::new();
    for w in notes.windows(2) {
        if w[0].end() > w[1].start {
            assert_ne!(w[0].key, w[1].key, "an overlap that goes nowhere");
            slides.push((w[0].key, w[1].key));
        } else {
            assert!(w[0].end() < w[1].start, "a restart needs a gap");
        }
    }
    assert_eq!(slides, [(29, 41), (37, 32), (34, 46), (36, 31)]);
    assert!(
        notes.iter().all(|n| (24..=48).contains(&n.key)),
        "808 notes belong in the sub range"
    );

    // And the engine plays them that way: on the first slide the pitch
    // climbs from F1 to F2 without a new attack.
    let (hook, _) = section(&p, "Hook", 0);
    let solo = soloed(&p, "808");
    let rate = 44_100.0;
    let step = 60.0 / 142.0 / 4.0;
    let (l, _) = render(&solo, hook, 2.0);
    let at = |a: f32, b: f32| &l[(a * step * rate) as usize..(b * step * rate) as usize];
    let before = crossing_hz(at(10.6, 12.8), rate);
    let after = crossing_hz(at(14.8, 15.9), rate);
    assert!((before / 43.65 - 1.0).abs() < 0.02, "held at {before} Hz");
    assert!((after / 87.31 - 1.0).abs() < 0.02, "landed on {after} Hz");
    let mid = crossing_hz(at(13.3, 14.3), rate);
    assert!(
        mid > before * 1.1 && mid < after * 0.95,
        "between the two at {mid} Hz"
    );
    // A restart would begin near silence; a slide keeps sounding.
    let rms = |b: &[f32]| (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt();
    assert!(rms(at(13.0, 13.4)) > 0.5 * rms(at(12.0, 13.0)));
}

#[test]
fn night_shift_is_half_time_with_rolling_hats() {
    let p = demo("Night Shift");
    assert_eq!(p.swing, 0.0);
    let snares = channel_in_section(&p, "Hook", "Snare");
    let on_three = snares
        .iter()
        .filter(|h| (beat_in_bar(h) - 2.0).abs() < 1e-9)
        .count();
    assert!(on_three >= 16, "a snare on beat three of every bar");
    assert!(on_three * 10 >= snares.len() * 8);
    let hats: Vec<f64> = channel_in_section(&p, "Hook", "Hat")
        .iter()
        .map(|h| h.beat)
        .collect();
    let gaps: BTreeSet<i64> = hats
        .windows(2)
        .map(|w| ((w[1] - w[0]) * 96.0).round() as i64)
        .filter(|&g| g > 0)
        .collect();
    assert!(gaps.contains(&12), "thirty-second rolls: {gaps:?}");
    assert!(gaps.contains(&16), "sixteenth-triplet rolls: {gaps:?}");
    assert!(gaps.contains(&48), "eighths in between: {gaps:?}");
    // The kick lands with the 808's attacks, never on the snare.
    let kicks = channel_in_section(&p, "Hook", "Kick");
    assert!(kicks.iter().all(|h| (beat_in_bar(h) - 2.0).abs() > 1e-9));
}

#[test]
fn neon_coast_gates_its_snare_plate_with_the_dry_snare() {
    let p = demo("Neon Coast");
    let plate = p.mixer.track(track_named(&p, "Snare Plate")).unwrap();
    let kinds: Vec<_> = plate.inserts.iter().map(|d| d.type_id.as_str()).collect();
    assert_eq!(kinds, ["peel.fx.reverb", "peel.fx.gate"]);
    let snare = channel_named(&p, "Snare");
    assert_eq!(plate.inserts[1].sidechain, Some(snare.track));
    let feeds = p.mixer.track(snare.track).unwrap();
    assert!(feeds.sends.iter().any(|s| s.target == plate.id));

    // Heard on its own in the verse, the plate rings while the gate holds
    // and is gone well before the next snare, two beats later.
    let (verse, _) = section(&p, "Verse", 0);
    let solo = soloed(&p, "Snare Plate");
    let rate = 44_100.0;
    let beat = 60.0 / 108.0;
    // Bar three of the verse; the snare is on beats two and four.
    let (l, r) = render(&solo, verse + 8.0, 4.0 * beat);
    let rms = |from: f32, to: f32| {
        let (a, b) = ((from * rate) as usize, (to * rate) as usize);
        let sum: f32 = l[a..b].iter().chain(&r[a..b]).map(|x| x * x).sum();
        (sum / (2 * (b - a)) as f32).sqrt()
    };
    let ringing = rms(beat + 0.03, beat + 0.18);
    let shut = rms(beat + 0.45, beat + 1.0);
    assert!(ringing > 0.01, "the plate is silent ({ringing})");
    assert!(
        shut < ringing * 0.05,
        "the gate left the tail open: {ringing} then {shut}"
    );
}

#[test]
fn neon_coast_drives_on_a_sixteenth_arp_and_bass() {
    let p = demo("Neon Coast");
    let (a, b) = section(&p, "Chorus", 0);
    for name in ["Arp", "Bass"] {
        let starts: BTreeSet<i64> = channel_in_section(&p, "Chorus", name)
            .iter()
            .map(|h| (h.beat * 4.0).round() as i64)
            .collect();
        assert_eq!(
            starts.len() as f64,
            (b - a) * 4.0,
            "{name} plays every sixteenth"
        );
    }
    let snares = channel_in_section(&p, "Chorus", "Snare");
    assert!(
        snares.iter().all(|h| [1.0, 3.0].contains(&beat_in_bar(h))),
        "snare on two and four"
    );
    assert!(channel_in_section(&p, "Verse", "Lead").is_empty());
    assert!(!channel_in_section(&p, "Chorus", "Lead").is_empty());
}

#[test]
fn corner_store_is_swung_and_runs_its_drums_through_tape() {
    let p = demo("Corner Store");
    assert!(p.swing > 0.3);
    let bus = p.mixer.track(track_named(&p, "Drum Bus")).unwrap();
    let kinds: Vec<_> = bus.inserts.iter().map(|d| d.type_id.as_str()).collect();
    assert!(kinds.contains(&"peel.fx.crush") && kinds.contains(&"peel.fx.tape"));
    let master: Vec<_> = p.mixer.tracks[0]
        .inserts
        .iter()
        .map(|d| d.type_id.as_str())
        .collect();
    assert!(master.contains(&"peel.fx.tape"), "{master:?}");
    let snares = channel_in_section(&p, "Verse", "Snare");
    let backbeat = snares
        .iter()
        .filter(|h| [1.0, 3.0].contains(&beat_in_bar(h)))
        .count();
    assert!(backbeat * 10 >= snares.len() * 8, "snare on two and four");
    let kicks = channel_in_section(&p, "Verse", "Kick");
    assert!(
        kicks.iter().any(|h| beat_in_bar(h).fract() != 0.0),
        "the kick is syncopated"
    );
    // The sax leads the middle section, not the first verse.
    assert!(channel_in_section(&p, "Verse", "Sax").is_empty());
    assert!(!channel_in_section(&p, "Middle", "Sax").is_empty());
}
