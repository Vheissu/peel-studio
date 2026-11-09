use super::*;

#[test]
fn lifetime_track_churn_never_aliases_a_live_meter_or_cue_source() {
    let mut p = template::empty();
    let retained = p.mixer.tracks[0].id;
    let mut meters = HashMap::new();
    reconcile_track_meters(&p, &mut meters);
    let original = meters[&retained];
    for _ in 0..peel_engine::telemetry::MAX_METERS * 3 {
        let mut track = p.mixer.tracks[0].clone();
        track.id = Id::new();
        let id = track.id;
        p.mixer.tracks.push(track);
        reconcile_track_meters(&p, &mut meters);
        assert_eq!(meters[&retained], original);
        assert_ne!(meters[&retained], meters[&id]);
        assert_eq!(meters.len(), 2);
        p.mixer.tracks.pop();
        reconcile_track_meters(&p, &mut meters);
        assert_eq!(meters.len(), 1);
    }
}

#[test]
fn capacity_and_missing_meters_never_fall_back_to_the_master() {
    let mut p = template::empty();
    while p.mixer.tracks.len() <= peel_engine::MAX_TRACKS {
        let mut track = p.mixer.tracks[0].clone();
        track.id = Id::new();
        p.mixer.tracks.push(track);
    }
    assert!(
        editing::validate_edit(&p)
            .unwrap_err()
            .contains("mixer tracks")
    );
    let mut meters = HashMap::new();
    reconcile_track_meters(&p, &mut meters);
    assert_eq!(meters.len(), peel_engine::MAX_TRACKS);
    let distinct: std::collections::HashSet<_> = meters.values().collect();
    assert_eq!(distinct.len(), meters.len());
    let (plan, _) = compile(
        &p,
        &CompileContext {
            mode: PlayMode::Song,
            slots: &HashMap::new(),
            meters: &meters,
            samples: &HashMap::new(),
        },
    );
    assert_eq!(
        plan.tracks.iter().filter(|t| t.meter == u16::MAX).count(),
        1
    );
    assert_eq!(plan.tracks.iter().filter(|t| t.meter == 0).count(), 1);
    p.mixer.tracks.pop();
    assert!(editing::validate_edit(&p).is_ok());
}

#[test]
fn live_meters_survive_reordering_and_restoring_tracks_without_collisions() {
    let mut p = template::beat_and_melody();
    let mut meters = HashMap::new();
    reconcile_track_meters(&p, &mut meters);
    let old = meters.clone();
    p.mixer.tracks[1..].reverse();
    reconcile_track_meters(&p, &mut meters);
    assert_eq!(meters, old);
    let removed = p.mixer.tracks.pop().unwrap();
    reconcile_track_meters(&p, &mut meters);
    assert!(!meters.contains_key(&removed.id));
    p.mixer.tracks.push(removed);
    reconcile_track_meters(&p, &mut meters);
    assert_eq!(
        meters
            .values()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        meters.len()
    );
}
