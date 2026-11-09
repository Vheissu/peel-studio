use super::*;

fn ch() -> Id {
    Id::from_u128(7)
}

fn note(start: (i64, i64), len: (i64, i64), key: u8) -> Note {
    Note::new(
        ch(),
        Time::new(start.0, start.1),
        Time::new(len.0, len.1),
        key,
        0.8,
    )
}

fn starts(v: &[Note]) -> Vec<Time> {
    v.iter().map(|n| n.start).collect()
}

/// A C major triad on beat 0 lasting one beat, then a single E on beat 2.
fn chord_and_note() -> Vec<Note> {
    vec![
        note((0, 1), (1, 1), 60),
        note((0, 1), (1, 1), 64),
        note((0, 1), (1, 1), 67),
        note((2, 1), (1, 2), 64),
    ]
}

#[test]
fn legato_reaches_the_next_onset_and_keeps_ids() {
    let src = vec![
        note((0, 1), (1, 4), 60),
        note((1, 1), (1, 4), 62),
        note((3, 1), (1, 4), 64),
    ];
    let out = apply(&src, &Tool::Legato { gap: Time::ZERO });
    assert_eq!(out[0].length, Time::beats(1));
    assert_eq!(out[1].length, Time::beats(2));
    assert_eq!(
        out[2].length,
        Time::new(1, 4),
        "the last note keeps its length"
    );
    assert!(out.iter().zip(&src).all(|(a, b)| a.id == b.id));
    let gapped = apply(
        &src,
        &Tool::Legato {
            gap: Time::new(1, 8),
        },
    );
    assert_eq!(gapped[0].length, Time::new(7, 8));
}

#[test]
fn legato_treats_a_chord_as_one_onset() {
    let out = apply(&chord_and_note(), &Tool::Legato { gap: Time::ZERO });
    assert!(
        out[..3].iter().all(|n| n.length == Time::beats(2)),
        "{out:?}"
    );
}

#[test]
fn gate_scales_lengths_with_a_floor() {
    let out = apply(&[note((0, 1), (1, 1), 60)], &Tool::Gate { amount: 0.5 });
    assert_eq!(out[0].length, Time::new(1, 2));
    let tiny = apply(&[note((0, 1), (1, 1), 60)], &Tool::Gate { amount: 0.0 });
    assert_eq!(tiny[0].length, MIN_LENGTH);
}

#[test]
fn chop_fills_the_note_with_repeats() {
    let out = apply(
        &[note((0, 1), (1, 1), 60)],
        &Tool::Chop {
            division: Time::new(1, 4),
            gate: 0.5,
        },
    );
    assert_eq!(
        starts(&out),
        [
            Time::ZERO,
            Time::new(1, 4),
            Time::new(1, 2),
            Time::new(3, 4)
        ]
    );
    assert!(out.iter().all(|n| n.length == Time::new(1, 8)));
    let ids: std::collections::HashSet<Id> = out.iter().map(|n| n.id).collect();
    assert_eq!(ids.len(), 4, "new notes get new ids");
}

#[test]
fn chop_cuts_the_last_piece_at_the_note_end() {
    let out = apply(
        &[note((0, 1), (3, 8), 60)],
        &Tool::Chop {
            division: Time::new(1, 4),
            gate: 1.0,
        },
    );
    assert_eq!(out.len(), 2);
    assert_eq!(out[1].length, Time::new(1, 8));
}

#[test]
fn arpeggiate_steps_through_the_chord() {
    let tool = |order| Tool::Arpeggiate {
        step: Time::new(1, 4),
        order,
        octaves: 1,
        gate: 1.0,
        seed: 1,
    };
    let up = apply(&chord_and_note()[..3], &tool(ArpOrder::Up));
    assert_eq!(
        up.iter().map(|n| n.key).collect::<Vec<_>>(),
        [60, 64, 67, 60]
    );
    assert_eq!(
        starts(&up),
        [
            Time::ZERO,
            Time::new(1, 4),
            Time::new(1, 2),
            Time::new(3, 4)
        ]
    );
    let down = apply(&chord_and_note()[..3], &tool(ArpOrder::Down));
    assert_eq!(
        down.iter().map(|n| n.key).collect::<Vec<_>>(),
        [67, 64, 60, 67]
    );
    let updown = apply(&chord_and_note()[..3], &tool(ArpOrder::UpDown));
    assert_eq!(
        updown.iter().map(|n| n.key).collect::<Vec<_>>(),
        [60, 64, 67, 64]
    );
}

#[test]
fn arpeggiate_spans_octaves_and_random_is_seeded() {
    let two = Tool::Arpeggiate {
        step: Time::new(1, 8),
        order: ArpOrder::Up,
        octaves: 2,
        gate: 1.0,
        seed: 0,
    };
    let out = apply(&chord_and_note()[..3], &two);
    assert_eq!(
        out.iter().map(|n| n.key).take(6).collect::<Vec<_>>(),
        [60, 64, 67, 72, 76, 79]
    );
    let r = |seed| {
        let t = Tool::Arpeggiate {
            step: Time::new(1, 16),
            order: ArpOrder::Random,
            octaves: 1,
            gate: 1.0,
            seed,
        };
        apply(&chord_and_note()[..3], &t)
            .iter()
            .map(|n| n.key)
            .collect::<Vec<_>>()
    };
    assert_eq!(r(5), r(5), "same seed, same result");
    assert_ne!(r(5), r(6));
}

#[test]
fn strum_spreads_onsets_and_keeps_ends() {
    let t = Tool::Strum {
        spread: Time::new(1, 8),
        direction: StrumDirection::Up,
        velocity: 0.5,
    };
    let out = apply(&chord_and_note()[..3], &t);
    assert_eq!(
        starts(&out),
        [Time::ZERO, Time::new(1, 16), Time::new(1, 8)]
    );
    assert!(out.iter().all(|n| n.end() == Time::beats(1)));
    assert!((out[2].velocity - 0.4).abs() < 1e-6);
    let down = apply(
        &chord_and_note()[..3],
        &Tool::Strum {
            spread: Time::new(1, 8),
            direction: StrumDirection::Down,
            velocity: 0.0,
        },
    );
    assert_eq!(down[0].key, 67, "down starts from the top");
}

#[test]
fn flam_adds_grace_notes_before_but_not_before_zero() {
    let t = Tool::Flam {
        count: 2,
        spacing: Time::new(1, 16),
        velocity: 0.5,
    };
    let out = apply(&[note((1, 1), (1, 4), 60), note((0, 1), (1, 4), 62)], &t);
    let graces: Vec<&Note> = out.iter().filter(|n| n.velocity < 0.8).collect();
    assert_eq!(graces.len(), 2, "the note at zero gets none");
    assert_eq!(graces[0].start, Time::new(7, 8));
    assert_eq!(graces[1].start, Time::new(15, 16));
}

#[test]
fn humanize_stays_in_range_and_is_seeded() {
    let src: Vec<Note> = (0..32).map(|i| note((i, 4), (1, 8), 60)).collect();
    let t = Tool::Humanize {
        timing: Time::new(1, 32),
        velocity: 0.1,
        length: 0.2,
        seed: 9,
    };
    let a = apply(&src, &t);
    let b = apply(&src, &t);
    assert_eq!(starts(&a), starts(&b));
    for (n, o) in a.iter().zip(src.iter()) {
        assert!((n.start.as_f64() - o.start.as_f64()).abs() <= 1.0 / 32.0 + 1e-3);
        assert!((n.velocity - 0.8).abs() <= 0.1 + 1e-6);
    }
    assert!(
        a.iter().zip(src.iter()).any(|(n, o)| n.start != o.start),
        "something moved"
    );
}

#[test]
fn levels_scale_and_clamp_velocity() {
    let out = apply(
        &[note((0, 1), (1, 4), 60)],
        &Tool::Levels {
            multiply: 2.0,
            offset: 0.1,
        },
    );
    assert_eq!(out[0].velocity, 1.0);
    let soft = apply(
        &[note((0, 1), (1, 4), 60)],
        &Tool::Levels {
            multiply: 0.5,
            offset: 0.0,
        },
    );
    assert!((soft[0].velocity - 0.4).abs() < 1e-6);
}

#[test]
fn flip_reverses_time_and_mirrors_pitch() {
    let src = vec![note((0, 1), (1, 4), 60), note((1, 1), (1, 1), 67)];
    let t = apply(
        &src,
        &Tool::Flip {
            time: true,
            pitch: false,
        },
    );
    // Span 0..2: the long note now starts at 0, the short one ends at 2.
    assert_eq!(t[0].key, 67);
    assert_eq!(t[0].start, Time::ZERO);
    assert_eq!(t[1].end(), Time::beats(2));
    let p = apply(
        &src,
        &Tool::Flip {
            time: false,
            pitch: true,
        },
    );
    assert_eq!(p.iter().map(|n| n.key).collect::<Vec<_>>(), [67, 60]);
}

#[test]
fn tidy_removes_duplicates_and_trims_overlaps() {
    let src = vec![
        note((0, 1), (1, 1), 60),
        note((0, 1), (1, 2), 60),
        note((1, 2), (1, 1), 60),
        note((0, 1), (2, 1), 64),
    ];
    let out = apply(&src, &Tool::Tidy);
    let c: Vec<&Note> = out.iter().filter(|n| n.key == 60).collect();
    assert_eq!(c.len(), 2, "the shorter duplicate is gone");
    assert_eq!(
        c[0].length,
        Time::new(1, 2),
        "trimmed to the next same-pitch onset"
    );
    assert!(
        out.iter()
            .any(|n| n.key == 64 && n.length == Time::beats(2)),
        "other pitches untouched"
    );
}

#[test]
fn every_tool_handles_an_empty_selection() {
    let tools = [
        Tool::Legato { gap: Time::ZERO },
        Tool::Gate { amount: 0.5 },
        Tool::Chop {
            division: Time::new(1, 4),
            gate: 1.0,
        },
        Tool::Arpeggiate {
            step: Time::new(1, 4),
            order: ArpOrder::Up,
            octaves: 1,
            gate: 1.0,
            seed: 0,
        },
        Tool::Strum {
            spread: Time::new(1, 8),
            direction: StrumDirection::Up,
            velocity: 0.0,
        },
        Tool::Flam {
            count: 1,
            spacing: Time::new(1, 16),
            velocity: 0.5,
        },
        Tool::Humanize {
            timing: Time::new(1, 32),
            velocity: 0.1,
            length: 0.1,
            seed: 0,
        },
        Tool::Levels {
            multiply: 1.0,
            offset: 0.0,
        },
        Tool::Flip {
            time: true,
            pitch: true,
        },
        Tool::Tidy,
    ];
    for t in &tools {
        assert!(apply(&[], t).is_empty(), "{}", t.name());
    }
}

fn curve(note: Id, initial: f32, later: f32) -> NoteExpression {
    NoteExpression {
        note,
        pitch: vec![
            peel_document::ExpressionPoint {
                offset: Time::ZERO,
                value: initial,
            },
            peel_document::ExpressionPoint {
                offset: Time::new(1, 2),
                value: later,
            },
        ],
        ..Default::default()
    }
}

#[test]
fn chop_and_arpeggio_remap_expression_to_their_exact_source_notes() {
    let mut a = note((0, 1), (1, 1), 60);
    let b = note((0, 1), (1, 1), 60);
    a.id = Id::from_u128(42);
    let expression = [curve(a.id, 1.0, 5.0), curve(b.id, -1.0, -5.0)];
    let (out, curves) = apply_with_expression(
        &[a, b],
        &expression,
        &Tool::Chop {
            division: Time::new(1, 2),
            gate: 1.0,
        },
    );
    assert_eq!(out.len(), 4);
    assert_eq!(curves.len(), 4);
    for source in [a, b] {
        let inherited = curves.iter().find(|e| e.note == source.id).unwrap();
        assert_eq!(
            inherited.pitch[0].value,
            if source.id == a.id { 1.0 } else { -1.0 }
        );
    }
    let tails: Vec<_> = out
        .iter()
        .filter(|n| n.start == Time::new(1, 2))
        .map(|n| curves.iter().find(|e| e.note == n.id).unwrap().pitch[0].value)
        .collect();
    assert!(
        tails.contains(&5.0) && tails.contains(&-5.0),
        "same-key voices keep separate curves: {tails:?}"
    );
    let (arp, curves) = apply_with_expression(
        &[a],
        &expression,
        &Tool::Arpeggiate {
            step: Time::new(1, 2),
            order: ArpOrder::Up,
            octaves: 1,
            gate: 1.0,
            seed: 3,
        },
    );
    assert_eq!(curves[0].note, arp[0].id);
    assert_eq!(curves[1].note, arp[1].id);
    assert_eq!(curves[1].pitch[0].value, 5.0);
}

#[test]
fn shortened_and_deleted_notes_do_not_leave_dangling_expression() {
    let a = note((1, 1), (1, 1), 60);
    let b = note((1, 1), (1, 1), 60);
    let expression = [curve(a.id, 1.0, 5.0), curve(b.id, -1.0, -5.0)];
    let (_, short) = apply_with_expression(&[a], &expression, &Tool::Gate { amount: 0.25 });
    assert_eq!(short[0].pitch.len(), 1);
    let (out, curves) = apply_with_expression(&[a, b], &expression, &Tool::Tidy);
    assert_eq!(out.len(), 1);
    assert_eq!(curves.len(), 1);
    assert_eq!(curves[0].note, out[0].id);
    let (out, curves) = apply_with_expression(
        &[a],
        &expression,
        &Tool::Flam {
            count: 2,
            spacing: Time::new(1, 8),
            velocity: 0.5,
        },
    );
    assert_eq!(curves.len(), out.len());
    assert!(out.iter().all(|n| curves.iter().any(|e| e.note == n.id)));
}
