//! Peel Studio's musical document: identities, model, validation and
//! transactional undo. Pure data; no device, GUI or engine dependencies.

mod color;
mod history;
mod id;
mod model;
mod validate;

pub use color::{Color, PALETTE};
pub use history::Document;
pub use id::Id;
pub use model::*;
pub use validate::{Issue, Severity, validate};

pub use peel_time::{TempoSegment, Time, TimeSignature};

use std::collections::BTreeMap;
use std::sync::Arc;

impl Project {
    /// An empty project: master track, one arrangement with eight lanes and
    /// one empty one-bar pattern.
    pub fn empty(title: impl Into<String>) -> Project {
        let master = MixerTrack {
            id: Id::new(),
            name: "Master".into(),
            color: Color::hex(0xACB7C7),
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            inserts: Vec::new(),
            output: None,
        };
        let tracks = (1..=8)
            .map(|i| Track {
                id: Id::new(),
                name: format!("Track {i}"),
                color: None,
                muted: false,
            })
            .collect();
        let arrangement = Arrangement {
            id: Id::new(),
            name: "Arrangement".into(),
            tracks,
            clips: Vec::new(),
            markers: Vec::new(),
            loop_range: None,
        };
        let pattern = Pattern {
            id: Id::new(),
            name: "Pattern 1".into(),
            color: PALETTE[6],
            length: Time::beats(4),
            notes: Vec::new(),
        };
        Project {
            format: FORMAT.to_string(),
            schema: SCHEMA_VERSION,
            id: Id::new(),
            revision: 0,
            meta: ProjectMeta {
                title: title.into(),
                ..Default::default()
            },
            tempo: vec![TempoSegment {
                start: Time::ZERO,
                bpm: 120.0,
                ramp_to: None,
            }],
            time_signature: TimeSignature::default(),
            swing: 0.0,
            seed: uuid::Uuid::new_v4().as_u64_pair().0,
            channels: Vec::new(),
            patterns: vec![Arc::new(pattern)],
            active_arrangement: arrangement.id,
            arrangements: vec![Arc::new(arrangement)],
            mixer: Mixer {
                tracks: vec![master],
            },
            assets: Vec::new(),
            extensions: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_channel() -> Project {
        let mut p = Project::empty("Test");
        let track = MixerTrack {
            id: Id::new(),
            name: "Kick".into(),
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
            track: track.id,
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            root_key: DEFAULT_ROOT_KEY,
        };
        p.mixer.tracks.push(track);
        p.channels.push(ch);
        p
    }

    #[test]
    fn empty_project_is_valid() {
        let p = Project::empty("Song");
        assert!(validate(&p).is_empty(), "{:?}", validate(&p));
    }

    #[test]
    fn undo_redo_restores_states() {
        let p = with_channel();
        let ch = p.channels[0].id;
        let pat = p.patterns[0].id;
        let mut doc = Document::new(p);
        doc.edit("Add step", |p| {
            p.pattern_mut(pat).unwrap().notes.push(Note::new(
                ch,
                Time::ZERO,
                Time::new(1, 4),
                60,
                0.8,
            ));
        });
        assert_eq!(doc.project().pattern(pat).unwrap().notes.len(), 1);
        assert_eq!(doc.undo().as_deref(), Some("Add step"));
        assert!(doc.project().pattern(pat).unwrap().notes.is_empty());
        assert_eq!(doc.redo().as_deref(), Some("Add step"));
        assert_eq!(doc.project().pattern(pat).unwrap().notes.len(), 1);
    }

    #[test]
    fn gestures_coalesce() {
        let mut doc = Document::new(with_channel());
        for i in 0..10 {
            doc.edit_gesture(7, "Change tempo", |p| p.set_bpm(120.0 + i as f64));
        }
        doc.end_gesture();
        assert_eq!(doc.history_labels().count(), 1);
        doc.undo();
        assert_eq!(doc.project().bpm(), 120.0);
    }

    #[test]
    fn checkpoints_share_untouched_patterns() {
        let mut p = with_channel();
        p.patterns.push(Arc::new(Pattern {
            id: Id::new(),
            name: "B".into(),
            color: PALETTE[1],
            length: Time::beats(4),
            notes: vec![],
        }));
        let a = p.patterns[0].id;
        let mut doc = Document::new(p);
        doc.edit("Rename", |p| {
            p.pattern_mut(a).unwrap().name = "Intro".into()
        });
        doc.undo();
        doc.redo();
        let shared = Arc::strong_count(&doc.project().patterns[1]);
        assert!(
            shared >= 2,
            "untouched pattern should be shared with history"
        );
    }

    #[test]
    fn json_round_trip() {
        let p = with_channel();
        let json = serde_json::to_string_pretty(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn detects_dangling_references() {
        let mut p = with_channel();
        p.channels[0].track = Id::new();
        assert!(validate(&p).iter().any(Issue::is_error));
    }
}
