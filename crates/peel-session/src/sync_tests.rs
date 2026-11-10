//! Regression coverage for document edits made outside the device inspector.
use super::*;

#[test]
fn undoing_the_first_parameter_override_clears_the_live_mirror() {
    let mut s = Session::new_silent(template::beat_and_melody());
    let device = s.project().channels[0].device.id;
    let desc = peel_devices::descriptor(&s.project().channels[0].device.type_id).unwrap();
    let param = desc.params[0];
    s.doc.edit("Clear overrides", |p| {
        find_device_mut(p, device).unwrap().params.clear()
    });
    s.tick();
    s.doc.edit("Set parameter", |p| {
        find_device_mut(p, device)
            .unwrap()
            .params
            .insert(param.key.into(), param.max);
    });
    s.tick();
    assert_eq!(s.devices[&device].params.get(param.key), Some(&param.max));
    s.doc.undo();
    s.tick();
    assert!(
        !s.devices[&device].params.contains_key(param.key),
        "removed overrides must be reset, not kept in the engine mirror"
    );
    s.doc.redo();
    s.tick();
    assert_eq!(s.devices[&device].params.get(param.key), Some(&param.max));
}

#[test]
fn edited_device_state_is_installed_and_undo_restores_it() {
    let mut s = Session::new_silent(template::beat_and_melody());
    let device = s.project().channels[0].device.id;
    assert!(s.devices[&device].state.is_none());
    s.doc.edit("Edit state", |p| {
        find_device_mut(p, device).unwrap().state = Some("saved state".into())
    });
    s.tick();
    assert_eq!(s.devices[&device].state.as_deref(), Some("saved state"));
    s.doc.undo();
    s.tick();
    assert!(s.devices[&device].state.is_none());
}

fn wait_for(s: &mut Session, what: &str, mut done: impl FnMut(&Session) -> bool) {
    let start = std::time::Instant::now();
    while !done(s) {
        assert!(
            start.elapsed().as_secs() < 5,
            "timed out waiting for {what}"
        );
        s.tick();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn a_pattern_plays_from_the_top_after_a_song_position() {
    let mut s = Session::new_silent(template::beat_and_melody());
    let pid = s.selected_pattern();
    let len = s.project().pattern(pid).unwrap().length.as_f64();
    s.set_mode(PlayMode::Song);
    s.seek(len + 8.0);
    wait_for(&mut s, "the seek", |s| s.position() > len);
    // Stopped: the playhead used to stay past the pattern's end, where
    // nothing plays and the loop is never taken.
    s.set_mode(PlayMode::Pattern(pid));
    wait_for(&mut s, "the return to the top", |s| s.position() == 0.0);
    s.play();
    wait_for(&mut s, "playback", |s| s.position() > 0.0);
    assert!(s.position() < len, "inside the pattern: {}", s.position());
    s.stop();
}

#[test]
fn play_restarts_a_playhead_left_past_the_end() {
    let mut s = Session::new_silent(template::beat_and_melody());
    let len = s.play_end();
    s.seek(len + 2.0);
    wait_for(&mut s, "the seek", |s| s.position() > len);
    s.play();
    wait_for(&mut s, "playback", |s| {
        s.position() < len && s.position() > 0.0
    });
    assert_eq!(s.start_position(), 0.0);
    s.stop();
}

#[test]
fn a_song_that_reaches_its_end_is_no_longer_playing() {
    let mut s = Session::new_silent(template::beat_and_melody());
    s.set_mode(PlayMode::Song);
    s.set_looping(false);
    let end = s.play_end();
    s.seek(end - 0.05);
    wait_for(&mut s, "the seek", |s| s.position() > 0.0);
    s.play();
    assert!(s.is_playing());
    wait_for(&mut s, "the end of the song", |s| !s.is_playing());
    // The next Play starts over instead of stopping a song that already has.
    s.toggle_play();
    wait_for(&mut s, "the restart", |s| s.position() < end - 1.0);
    assert!(s.is_playing());
    s.stop();
}
