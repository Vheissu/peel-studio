use peel_document::{Id, Project};
use peel_mcp::{Control, decode, tools};
use peel_session::Session;
use serde_json::{Value, json};

struct Host {
    s: Session,
    c: Control,
}
impl Host {
    fn new() -> Self {
        Self {
            s: Session::new_silent(Project::empty("Test")),
            c: Control::default(),
        }
    }
    fn call(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let action = decode(&format!("peel_{name}"), args.as_object().unwrap().clone())?;
        let result = self.c.execute(&mut self.s, action);
        self.s.tick();
        result
    }
    fn patch(&mut self, operations: Value) -> Result<Value, String> {
        self.call("edit_project",json!({"expected_project_id":self.s.project().id,"expected_revision":self.s.doc.revision(),"label":"Agent edit","operations":operations}))
    }
    fn channel(&mut self) -> Value {
        self.call(
            "add_channel",
            json!({"name":"Kick","type_id":"peel.drum.kick"}),
        )
        .unwrap()
    }
}

#[test]
fn tools_have_strict_schemas_and_resolvable_references() {
    let tools = tools();
    assert!(tools.len() >= 30);
    let mut names = std::collections::HashSet::new();
    fn check_refs(value: &Value, root: &Value) {
        if let Some(reference) = value.get("$ref").and_then(Value::as_str) {
            assert!(
                root.pointer(reference.strip_prefix('#').unwrap()).is_some(),
                "{reference}"
            );
        }
        match value {
            Value::Object(map) => {
                for v in map.values() {
                    check_refs(v, root);
                }
            }
            Value::Array(values) => {
                for v in values {
                    check_refs(v, root);
                }
            }
            _ => {}
        }
    }
    for tool in tools {
        assert!(names.insert(tool.name.clone()));
        assert!(tool.description.as_ref().is_some_and(|d| !d.is_empty()));
        let schema = Value::Object((*tool.input_schema).clone());
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"].get("action").is_none());
        check_refs(&schema, &schema);
    }
}

#[test]
fn compose_arrange_and_undo_use_the_same_document() {
    let mut h = Host::new();
    let channel = h.channel();
    let pattern = h.s.project().patterns[0].id;
    let lane = h.s.project().arrangement().tracks[0].id;
    h.call("set_notes",json!({"pattern":pattern,"channel":channel["channel"],"notes":[{"start":"1/3","length":"1/3","key":60,"velocity":0.8}]})).unwrap();
    h.call(
        "place_pattern",
        json!({"pattern":pattern,"lane":lane,"start":"0","length":"4"}),
    )
    .unwrap();
    assert_eq!(
        h.s.project().pattern(pattern).unwrap().notes[0]
            .start
            .to_string(),
        "1/3"
    );
    assert_eq!(h.s.project().arrangement().clips.len(), 1);
    h.call("history", json!({"direction":"undo"})).unwrap();
    assert!(h.s.project().arrangement().clips.is_empty());
    assert_eq!(h.s.project().pattern(pattern).unwrap().notes.len(), 1);
    h.call("history", json!({"direction":"redo"})).unwrap();
    assert_eq!(h.s.project().arrangement().clips.len(), 1);
    h.call(
        "transport",
        json!({"mode":"song","command":"play","looping":false}),
    )
    .unwrap();
    assert!(h.s.is_playing());
    h.c.disconnect(&mut h.s);
    assert!(!h.s.is_playing());
}

#[test]
fn patches_are_atomic_validated_and_revision_checked() {
    let mut h = Host::new();
    h.channel();
    let before = h.s.project().clone();
    let history = h.s.doc.history_labels().count();
    for operations in [
        json!([{"op":"replace","path":"/meta/title","value":"Partial"},{"op":"remove","path":"/missing"}]),
        json!([{"op":"replace","path":"/channels/0/track","value":Id::new()}]),
        json!([{"op":"replace","path":"/tempo/0/bpm","value":0}]),
        json!([{"op":"replace","path":"/mixer/tracks/0/output","value":before.mixer.tracks[0].id}]),
        json!([{"op":"replace","path":"/patterns/0/length","value":"0"}]),
        json!([{"op":"replace","path":"/patterns/0/length","value":"-170141183460469231731687303715884105728"}]),
        json!([{"op":"replace","path":"/channels/0/pan","value":5}]),
        json!([{"op":"replace","path":"/id","value":Id::new()}]),
    ] {
        assert!(h.patch(operations).is_err());
        assert_eq!(h.s.project(), &before);
        assert_eq!(h.s.doc.history_labels().count(), history);
    }
    assert!(h.call("edit_project",json!({"expected_project_id":before.id,"expected_revision":before.revision-1,"label":"Stale","operations":[{"op":"replace","path":"/meta/title","value":"Oops"}]})).unwrap_err().contains("Stale"));
    h.patch(json!([{"op":"replace","path":"/meta/title","value":"Agent song"},{"op":"replace","path":"/tempo/0/bpm","value":110}])).unwrap();
    assert_eq!(h.s.project().meta.title, "Agent song");
    assert_eq!(h.s.doc.history_labels().count(), history + 1);
    h.call("history", json!({"direction":"undo"})).unwrap();
    assert_eq!(h.s.project().meta.title, "Test");
    assert_eq!(h.s.project().bpm(), 120.0);
}

#[test]
fn factory_device_discovery_and_range_checks() {
    let mut h = Host::new();
    for desc in peel_devices::all() {
        let result = h.call("devices", json!({"type_id":desc.type_id})).unwrap();
        assert_eq!(
            result["devices"][0]["params"].as_array().unwrap().len(),
            desc.params.len()
        );
    }
    let channel = h.channel();
    let param = peel_devices::descriptor("peel.drum.kick").unwrap().params[0];
    let before = h.s.project().clone();
    let mut params = serde_json::Map::new();
    params.insert(param.key.into(), json!(param.max + 100.0));
    assert!(
        h.call(
            "set_device",
            json!({"device":channel["device"],"params":params})
        )
        .is_err()
    );
    assert_eq!(h.s.project(), &before);
    params.insert(param.key.into(), json!(param.default));
    h.call(
        "set_device",
        json!({"device":channel["device"],"params":params}),
    )
    .unwrap();
}

#[test]
fn patch_supports_automation_sends_markers_and_alternate_arrangements() {
    let mut h = Host::new();
    let channel = h.channel();
    let p = h.s.project();
    let lane = p.arrangement().tracks[0].id;
    let master = p.mixer.tracks[0].id;
    let arrangement = Id::new();
    let operations = json!([
        {"op":"add","path":"/mixer/tracks/1/sends","value":[{"target":master,"gain_db":-12.0}]},
        {"op":"add","path":"/arrangements/0/markers/-","value":{"id":Id::new(),"position":"4","name":"Drop"}},
        {"op":"add","path":"/arrangements/0/clips/-","value":{"id":Id::new(),"track":lane,"start":"0","length":"4","content":{"type":"automation","target":{"kind":"track_pan","track":channel["track"]},"points":[{"position":"0","value":0.0},{"position":"4","value":1.0,"tension":0.5}]}}},
        {"op":"add","path":"/arrangements/-","value":{"id":arrangement,"name":"Alternate","tracks":[],"clips":[]}},
        {"op":"replace","path":"/active_arrangement","value":arrangement}
    ]);
    h.patch(operations).unwrap();
    assert_eq!(h.s.project().active_arrangement, arrangement);
    assert_eq!(h.s.project().arrangements[0].markers.len(), 1);
    assert_eq!(h.s.project().mixer.tracks[1].sends.len(), 1);
}

#[test]
fn invalid_transport_does_not_partially_change_playback() {
    let mut h = Host::new();
    h.call("transport", json!({"command":"play"})).unwrap();
    let mode = h.s.mode();
    assert!(
        h.call(
            "transport",
            json!({"command":"stop","mode":"song","beat":-1})
        )
        .is_err()
    );
    assert!(h.s.is_playing());
    assert_eq!(h.s.mode(), mode);
}

#[test]
fn replacement_and_no_op_edits_preserve_work() {
    let mut h = Host::new();
    h.channel();
    let before = h.s.project().clone();
    h.patch(json!([])).unwrap();
    assert_eq!(h.s.project(), &before);
    assert!(
        h.call("new_project", json!({"title":"Oops"}))
            .unwrap_err()
            .contains("Unsaved")
    );
    assert!(
        h.call("open_project", json!({"path":"/missing.peelproj"}))
            .unwrap_err()
            .contains("Unsaved")
    );
    assert_eq!(h.s.project(), &before);
    h.call("new_project", json!({"title":"New","discard_changes":true}))
        .unwrap();
    assert_eq!(h.s.project().meta.title, "New");
}

#[test]
fn schemas_and_decoding_reject_wrong_arguments() {
    assert!(
        decode(
            "peel_status",
            json!({"extra":true}).as_object().unwrap().clone()
        )
        .is_err()
    );
    assert!(
        decode(
            "peel_status",
            json!({"action":"new_project"}).as_object().unwrap().clone()
        )
        .is_err()
    );
    assert!(decode("peel_unknown", Default::default()).is_err());
    let mut h = Host::new();
    assert!(h.call("new_ids", json!({"count":257})).is_err());
    assert!(
        h.call("save_project", json!({"path":"relative.peelproj"}))
            .is_err()
    );
    assert!(h.call("recording", json!({"command":"start"})).is_err());
    assert!(
        h.call("performance", json!({"command":"scene","scene":1}))
            .is_err()
    );
}

#[test]
fn transforms_preserve_unselected_notes_and_undo_as_one_step() {
    let mut h = Host::new();
    let channel = h.channel();
    let pattern = h.s.project().patterns[0].id;
    let result = h.call("set_notes",json!({"pattern":pattern,"channel":channel["channel"],"notes":[{"start":"0","length":"1","key":60,"velocity":0.8},{"start":"2","length":"1","key":64,"velocity":0.7}]})).unwrap();
    let unselected = h.s.project().patterns[0].notes[1];
    let before = h.s.doc.history_labels().count();
    h.call("transform_notes",json!({"pattern":pattern,"selection":[result["notes"][0]],"tool":{"kind":"chop","division":"1/4","gate":0.8}})).unwrap();
    assert_eq!(h.s.project().patterns[0].notes.len(), 5);
    assert_eq!(h.s.project().patterns[0].notes[4], unselected);
    assert_eq!(h.s.doc.history_labels().count(), before + 1);
    h.call("history", json!({"direction":"undo"})).unwrap();
    assert_eq!(h.s.project().patterns[0].notes.len(), 2);
    let before = h.s.project().clone();
    assert!(
        h.call(
            "transform_notes",
            json!({"pattern":pattern,"tool":{"kind":"chop","division":"-1","gate":0.8}})
        )
        .is_err()
    );
    assert_eq!(h.s.project(), &before);
}

#[test]
fn stale_project_identity_is_rejected_even_at_the_same_revision() {
    let mut h = Host::new();
    let old = h.s.project().id;
    h.call("new_project", json!({"title":"Different"})).unwrap();
    assert_eq!(h.s.doc.revision(), 0);
    assert!(h.call("edit_project",json!({"expected_project_id":old,"expected_revision":0,"label":"Wrong project","operations":[{"op":"replace","path":"/meta/title","value":"Oops"}]})).is_err());
    assert_eq!(h.s.project().meta.title, "Different");
}

#[test]
fn demo_can_be_edited_with_the_current_device_catalog() {
    let mut h = Host::new();
    h.call("new_project", json!({"title":"Demo","demo":true}))
        .unwrap();
    h.patch(json!([{"op":"replace","path":"/meta/artist","value":"Agent"}]))
        .unwrap();
    assert_eq!(h.s.project().meta.artist, "Agent");
}

#[test]
fn closing_input_and_disconnecting_release_the_audio_input() {
    let mut h = Host::new();
    let _input = h.s.open_injected_input(48_000, 1);
    assert!(h.s.input_is_open());
    h.call("close_input", json!({})).unwrap();
    assert!(!h.s.input_is_open());
    let _input = h.s.open_injected_input(48_000, 1);
    h.c.disconnect(&mut h.s);
    assert!(!h.s.input_is_open());
}

#[test]
fn controller_tools_share_hardware_filter_learn_and_parameter_paths() {
    let mut h = Host::new();
    let ch = h.channel();
    let param = peel_devices::descriptor("peel.drum.kick").unwrap().params[0];
    let mapping = json!({"controller":21,"channel":0,"target":{"kind":"device_param","device":ch["device"],"key":param.key},"min":param.min,"max":param.max});
    h.call(
        "configure_midi_controls",
        json!({"channel_filter":0,"mappings":[mapping]}),
    )
    .unwrap();
    h.call(
        "midi_control",
        json!({"channel":0,"controller":21,"value":127}),
    )
    .unwrap();
    assert_eq!(
        h.s.project().channels[0].device.params[param.key],
        param.max
    );
    h.call(
        "midi_control",
        json!({"channel":1,"controller":21,"value":0}),
    )
    .unwrap();
    assert_eq!(
        h.s.project().channels[0].device.params[param.key],
        param.max
    );
    let before = h.call("midi_controls", json!({})).unwrap();
    assert!(h.call("configure_midi_controls", json!({"channel_filter":2,"mappings":[{"controller":21,"target":{"kind":"device_param","device":ch["device"],"key":"missing"},"min":0,"max":1}]})).is_err());
    assert_eq!(h.call("midi_controls", json!({})).unwrap(), before);
    assert!(
        h.call(
            "midi_control",
            json!({"channel":16,"controller":21,"value":1})
        )
        .is_err()
    );
    h.call(
        "midi_learn",
        json!({"target":{"kind":"track_pan","track":ch["track"]}}),
    )
    .unwrap();
    assert_eq!(
        h.call("midi_controls", json!({})).unwrap()["learn"]["state"],
        "waiting"
    );
    h.call(
        "midi_control",
        json!({"channel":0,"controller":22,"value":127}),
    )
    .unwrap();
    let learned = h.call("midi_controls", json!({})).unwrap();
    assert_eq!(learned["learn"]["state"], "learned");
    assert_eq!(learned["learn"]["mapping"]["controller"], 22);
    let track = h.s.project().channels[0].track;
    assert_eq!(h.s.project().mixer.track(track).unwrap().pan, 1.0);
}

#[test]
fn hardware_route_tools_validate_before_opening_devices() {
    let mut h = Host::new();
    let initial = h.call("output_routes", json!({})).unwrap();
    assert!(
        h.call("configure_output_channels", json!({"left":65535}))
            .is_err()
    );
    assert_eq!(
        h.call("output_routes", json!({})).unwrap()["primary_channels"],
        initial["primary_channels"]
    );
    assert!(h.call("configure_output_routes", json!({"routes":[{"name":"Cue","track":Id::new(),"device":"Not a device","left":0,"right":1}]})).is_err());
    assert!(
        h.call("output_routes", json!({})).unwrap()["routes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    h.call("configure_output_routes", json!({"routes":[]}))
        .unwrap();
}

#[test]
fn mastering_is_discoverable_and_editable_through_scripts_host_actions() {
    let mut h = Host::new();
    let catalog = h
        .call("devices", json!({"type_id":"peel.fx.mastering"}))
        .unwrap();
    assert_eq!(
        catalog["devices"][0]["params"].as_array().unwrap().len(),
        13
    );
    assert!(
        catalog["devices"][0]["presets"]
            .as_array()
            .unwrap()
            .contains(&json!("Balanced"))
    );
    let master = h.s.project().mixer.tracks[0].id;
    let effect = h
        .call(
            "add_effect",
            json!({"track":master,"type_id":"peel.fx.mastering"}),
        )
        .unwrap();
    h.call(
        "set_device",
        json!({"device":effect["device"],"preset":"Balanced","params":{"ceiling":-1.2}}),
    )
    .unwrap();
    assert_eq!(
        h.s.project().mixer.tracks[0].inserts[0].params["ceiling"],
        -1.2
    );
}

#[test]
fn midi_expression_is_transient_and_validates_the_complete_batch() {
    let mut h = Host::new();
    h.channel();
    let channel = h.s.project().channels[0].id;
    h.s.set_live_target(Some(channel));
    let before = h.s.project().clone();
    h.call(
        "midi_expression",
        json!({"bend_range":12,"bend":0.5,"pressure":0.7,"key_pressure":[{"key":60,"value":0.8}]}),
    )
    .unwrap();
    let state = h.call("midi_expression", json!({})).unwrap();
    assert_eq!(state["bend"], 0.5);
    assert_eq!(state["semitones"], 6.0);
    assert!((state["pressure"].as_f64().unwrap() - 0.7).abs() < 1e-5);
    for invalid in [
        json!({"bend":2,"bend_range":2}),
        json!({"pressure":-1}),
        json!({"bend_range":49}),
        json!({"reset":true,"bend":0}),
        json!({"key_pressure":[{"key":60,"value":0.1},{"key":60,"value":0.2}]}),
        json!({"key_pressure":[{"key":128,"value":0.2}]}),
    ] {
        assert!(h.call("midi_expression", invalid).is_err());
        assert_eq!(h.call("midi_expression", json!({})).unwrap(), state);
    }
    assert_eq!(h.s.project(), &before);
    h.call("midi_expression", json!({"reset":true})).unwrap();
    let state = h.call("midi_expression", json!({})).unwrap();
    assert_eq!(state["bend"], 0.0);
    assert_eq!(state["pressure"], 0.0);
    assert_eq!(state["bend_range"], 12.0);
    assert_eq!(state["key_pressure"], json!([]));
    assert_eq!(h.s.project(), &before);
}

#[test]
fn expression_requires_a_target_before_changing_live_state_or_range() {
    let mut h = Host::new();
    h.call("midi_expression", json!({"bend_range": 7})).unwrap();
    assert!(
        h.call("midi_expression", json!({"bend_range": 12, "bend": 0.5}))
            .is_err()
    );
    let state = h.call("midi_expression", json!({})).unwrap();
    assert_eq!(state["bend_range"], 7.0);
    assert_eq!(state["bend"], 0.0);
    assert!(h.call("midi_expression", json!({"reset": true})).is_err());
}

#[test]
fn saved_expression_is_editable_validated_and_preserved_by_note_tools() {
    let mut h = Host::new();
    let channel = h.channel();
    let pid = h.s.selected_pattern();
    let result=h.call("set_notes",json!({"pattern":pid,"channel":channel["channel"],"notes":[{"start":"0","length":"2","key":60,"velocity":0.8}]})).unwrap();
    let note = result["notes"][0].clone();
    h.call("note_expression",json!({"pattern":pid,"note":note,"pitch":[{"offset":"0","value":0},{"offset":"1/2","value":7}],"pressure":[{"offset":"0","value":0.2},{"offset":"1","value":0.8}]})).unwrap();
    let saved = h
        .call("note_expression", json!({"pattern":pid,"note":note}))
        .unwrap()["expression"]
        .clone();
    for bad in [
        json!({"pitch":[{"offset":"3","value":7}]}),
        json!({"pitch":[{"offset":"0","value":121}]}),
        json!({"pressure":[{"offset":"0","value":-0.1}]}),
        json!({"pitch":[{"offset":"0","value":0},{"offset":"0","value":1}]}),
        json!({"timbre":[{"offset":"-1","value":0.5}]}),
    ] {
        let mut args = bad.as_object().unwrap().clone();
        args.insert("pattern".into(), json!(pid));
        args.insert("note".into(), note.clone());
        assert!(h.call("note_expression", Value::Object(args)).is_err());
        assert_eq!(
            h.call("note_expression", json!({"pattern":pid,"note":note}))
                .unwrap()["expression"],
            saved
        );
    }
    h.call(
        "transform_notes",
        json!({"pattern":pid,"selection":[note],"tool":{"kind":"chop","division":"1","gate":1}}),
    )
    .unwrap();
    let p = h.s.project().pattern(pid).unwrap();
    assert_eq!(p.notes.len(), 2);
    assert_eq!(p.expression.len(), 2);
    let later = p
        .notes
        .iter()
        .find(|n| n.start == peel_document::Time::beats(1))
        .unwrap();
    assert_eq!(
        p.expression_for(later.id)
            .unwrap()
            .at(peel_document::Time::ZERO)
            .pitch,
        7.0
    );
    h.call(
        "set_notes",
        json!({"pattern":pid,"channel":channel["channel"],"notes":[]}),
    )
    .unwrap();
    assert!(h.s.project().pattern(pid).unwrap().expression.is_empty());
}

#[test]
fn mpe_tools_route_equal_pitches_to_distinct_voices_without_editing_the_project() {
    let mut h = Host::new();
    let channel = h.channel();
    h.call("configure_midi", json!({"channel":channel["channel"]}))
        .unwrap();
    let before = h.s.project().clone();
    let config = h.call("mpe", json!({"mode":"lower","members":4})).unwrap()["config"].clone();
    assert_eq!(config["member_bend_range"], 48.0);
    for bad in [
        json!({"members":0}),
        json!({"member_bend_range":97}),
        json!({"manager_bend_range":-1}),
    ] {
        assert!(h.call("mpe", bad).is_err());
        assert_eq!(h.call("mpe", json!({})).unwrap()["config"], config);
    }
    for channel in [1, 2] {
        h.call(
            "midi_input",
            json!({"channel":channel,"event":{"kind":"note_on","key":60,"velocity":100}}),
        )
        .unwrap();
    }
    h.call(
        "midi_input",
        json!({"channel":1,"event":{"kind":"pitch_bend","value":8191}}),
    )
    .unwrap();
    h.call(
        "midi_input",
        json!({"channel":2,"event":{"kind":"channel_pressure","value":100}}),
    )
    .unwrap();
    let events = h.s.take_recorded_performance();
    let ons: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, peel_session::RecordedPerformanceKind::NoteOn { .. }))
        .collect();
    assert_eq!(ons.len(), 2);
    assert_ne!(ons[0].voice, ons[1].voice);
    assert!(events.iter().any(|e|e.voice==ons[0].voice&&matches!(e.kind,peel_session::RecordedPerformanceKind::Expression{pitch,..} if pitch==48.0)));
    assert!(events.iter().any(|e|e.voice==ons[1].voice&&matches!(e.kind,peel_session::RecordedPerformanceKind::Expression{pressure,..} if pressure>0.7)));
    assert!(
        h.call(
            "midi_input",
            json!({"channel":16,"event":{"kind":"note_on","key":60,"velocity":100}})
        )
        .is_err()
    );
    assert!(
        h.call(
            "midi_input",
            json!({"channel":1,"event":{"kind":"pitch_bend","value":8192}})
        )
        .is_err()
    );
    assert_eq!(h.s.project(), &before);
}

#[test]
fn explicit_mpe_off_disables_rpn_reactivation_even_if_runtime_already_reports_off() {
    let mut h = Host::new();
    h.channel();
    h.call("mpe", json!({"mode":"lower"})).unwrap();
    fn rpn(h: &mut Host, count: u8) {
        for (cc, value) in [(101, 0), (100, 6), (6, count)] {
            h.call("midi_input",json!({"channel":0,"event":{"kind":"control_change","controller":cc,"value":value}})).unwrap();
        }
    }
    rpn(&mut h, 0);
    assert_eq!(h.call("mpe", json!({})).unwrap()["config"]["mode"], "off");
    h.call("mpe", json!({"mode":"off"})).unwrap();
    rpn(&mut h, 15);
    assert_eq!(h.call("mpe", json!({})).unwrap()["config"]["mode"], "off");
}
