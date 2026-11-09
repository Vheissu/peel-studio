use super::*;

fn fixture() -> (Project, Id) {
    let mut p = Project::empty("Expression");
    let ch = Channel {
        id: Id::new(),
        name: "Lead".into(),
        color: PALETTE[0],
        device: DeviceInstance::new("peel.synth.poly", 1),
        track: p.mixer.tracks[0].id,
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        root_key: 60,
    };
    let note = Note::new(ch.id, Time::beats(1), Time::beats(2), 60, 0.8);
    let id = note.id;
    p.channels.push(ch);
    let pattern = p.pattern_mut(p.patterns[0].id).unwrap();
    pattern.notes.push(note);
    pattern.expression.push(NoteExpression {
        note: id,
        pitch: vec![point(0, 0.0), point(1, 7.0)],
        pressure: vec![point(0, 0.2), point(1, 0.8)],
        timbre: vec![point(0, 0.5)],
    });
    (p, id)
}
fn point(beat: i64, value: f32) -> ExpressionPoint {
    ExpressionPoint {
        offset: Time::beats(beat),
        value,
    }
}

#[test]
fn old_patterns_default_to_empty_expression_and_keep_their_json_shape() {
    let p = Project::empty("Old");
    let value = serde_json::to_value(&p).unwrap();
    assert!(value["patterns"][0].get("expression").is_none());
    let loaded: Project = serde_json::from_value(value.clone()).unwrap();
    assert!(loaded.patterns[0].expression.is_empty());
    assert_eq!(serde_json::to_value(loaded).unwrap(), value);
}

#[test]
fn expression_roundtrips_by_note_identity_and_holds_recorded_values() {
    let (p, id) = fixture();
    assert!(validate(&p).iter().all(|i| !i.is_error()));
    let loaded: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(p, loaded);
    let e = loaded.patterns[0].expression_for(id).unwrap();
    assert_eq!(
        e.at(Time::new(1, 2)),
        ExpressionValue {
            pitch: 0.0,
            pressure: 0.2,
            timbre: 0.5
        }
    );
    assert_eq!(
        e.at(Time::beats(1)),
        ExpressionValue {
            pitch: 7.0,
            pressure: 0.8,
            timbre: 0.5
        }
    );
}

#[test]
fn expression_validation_rejects_dangling_duplicate_unsorted_and_invalid_points() {
    let (p, _) = fixture();
    let invalid = |edit: fn(&mut NoteExpression)| {
        let mut candidate = p.clone();
        edit(
            &mut candidate
                .pattern_mut(candidate.patterns[0].id)
                .unwrap()
                .expression[0],
        );
        assert!(validate(&candidate).iter().any(Issue::is_error));
    };
    invalid(|e| e.note = Id::new());
    invalid(|e| e.pitch.push(point(1, 2.0)));
    invalid(|e| e.pitch.swap(0, 1));
    invalid(|e| e.pitch.push(point(3, 2.0)));
    invalid(|e| e.pitch[0].offset = Time::beats(-1));
    invalid(|e| e.pitch[0].value = f32::NAN);
    invalid(|e| e.pitch[0].value = 121.0);
    invalid(|e| e.pressure[0].value = -0.1);
    invalid(|e| e.timbre[0].value = 1.1);
    let mut duplicate = p.clone();
    let pattern = duplicate.pattern_mut(duplicate.patterns[0].id).unwrap();
    pattern.expression.push(pattern.expression[0].clone());
    assert!(validate(&duplicate).iter().any(Issue::is_error));
}

#[test]
fn expression_validation_bounds_recorded_curve_storage() {
    let (mut p, _) = fixture();
    let pattern = p.pattern_mut(p.patterns[0].id).unwrap();
    pattern.expression[0].pitch = (0..=MAX_EXPRESSION_POINTS)
        .map(|i| ExpressionPoint {
            offset: Time::new(i as i64, MAX_EXPRESSION_POINTS as i64),
            value: 0.0,
        })
        .collect();
    assert!(
        validate(&p)
            .iter()
            .any(|i| i.is_error() && i.message.contains("too many expression"))
    );
}

#[test]
fn slicing_expression_chases_the_new_start_and_prunes_deleted_notes() {
    let (mut p, id) = fixture();
    let e =
        p.patterns[0]
            .expression_for(id)
            .unwrap()
            .slice(Id::new(), Time::new(1, 2), Time::beats(1));
    assert_eq!(
        e.pitch,
        vec![
            point(0, 0.0),
            ExpressionPoint {
                offset: Time::new(1, 2),
                value: 7.0
            }
        ]
    );
    let pattern = p.pattern_mut(p.patterns[0].id).unwrap();
    pattern.notes[0].length = Time::new(1, 2);
    pattern.retain_expression();
    assert_eq!(pattern.expression[0].pitch.len(), 1);
    pattern.notes.clear();
    pattern.retain_expression();
    assert!(pattern.expression.is_empty());
}
