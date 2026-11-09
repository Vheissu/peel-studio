//! The new catalog survives project saves and renders through the host's
//! real device installation, routing and offline export path.
use peel_document::Id;
use peel_session::{AudioExportSettings, ExportSettings, render_audio, template};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32};

#[test]
fn new_devices_round_trip_and_render_through_the_session() {
    let root = std::env::temp_dir().join(format!("peel-device-expansion-{}", Id::new()));
    std::fs::create_dir_all(&root).unwrap();
    for type_id in [
        "peel.perc.mallet",
        "peel.strings.zither",
        "peel.synth.cross",
    ] {
        let mut project = template::beat_and_melody();
        project.channels.truncate(1);
        let channel = &mut project.channels[0];
        channel.device = template::device(type_id, Some(peel_devices::presets(type_id)[1].name));
        let channel_id = channel.id;
        for pattern in &mut project.patterns {
            let pattern = Arc::make_mut(pattern);
            pattern.notes.retain(|n| n.channel == channel_id);
            for note in &mut pattern.notes {
                note.key = 60;
            }
        }
        project.mixer.tracks[0].inserts =
            ["peel.fx.transient", "peel.fx.crush", "peel.fx.resonator"]
                .into_iter()
                .map(|id| template::device(id, Some(peel_devices::presets(id)[1].name)))
                .collect();
        let path = root.join(format!("{type_id}.peelproj"));
        peel_project::save(&project, &path).unwrap();
        let loaded = peel_project::load(&path).unwrap();
        assert_eq!(loaded.project, project);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let wav = root.join(format!("{type_id}.wav"));
        render_audio(
            &loaded.project,
            &HashMap::new(),
            &AudioExportSettings {
                render: ExportSettings {
                    path: wav.clone(),
                    sample_rate: 48_000,
                    bits: 24,
                    tail_seconds: 0.5,
                },
                dither: false,
                ..AudioExportSettings::default()
            },
            &AtomicU32::new(0),
            &AtomicBool::new(false),
        )
        .unwrap();
        let decoded = peel_media::decode_file(&wav).unwrap();
        let samples = &decoded.channels[0];
        assert!(!samples.is_empty());
        assert!(samples.iter().all(|x| x.is_finite()));
        let peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.005, "{type_id}: silent session render ({peak})");
    }
    std::fs::remove_dir_all(root).unwrap();
}
