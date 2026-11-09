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
