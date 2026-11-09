//! An agent must be able to add the first CLAP instance before its dynamic
//! parameter descriptor has ever been cached.
#[path = "../../peel-host-clap/tests/fixture/mod.rs"]
mod fixture;

use peel_document::Project;
use peel_mcp::{Control, decode};
use peel_session::Session;
use serde_json::{Value, json};

#[test]
fn first_clap_instance_can_be_added_and_its_parameters_edited() {
    let mut s = Session::new_silent(Project::empty("Plugins"));
    s.set_plugins(fixture::infos().to_vec());
    let mut c = Control::default();
    let mut call = |name: &str, args: Value| {
        let action = decode(name, args.as_object().unwrap().clone()).unwrap();
        let result = c.execute(&mut s, action).unwrap();
        s.tick();
        result
    };
    let initial = call("peel_devices", json!({"type_id":"clap:dev.peel.test.gain"}));
    assert_eq!(initial["devices"][0]["parameters_available"], false);
    let document = call("peel_read_project", json!({}));
    let master = &document["value"]["mixer"]["tracks"][0]["id"];
    let effect = call(
        "peel_add_effect",
        json!({"track":master,"type_id":"clap:dev.peel.test.gain"}),
    );
    let schema = call("peel_devices", json!({"type_id":"clap:dev.peel.test.gain"}));
    assert!(
        schema["devices"][0]["params"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["key"] == "7")
    );
    call(
        "peel_set_device",
        json!({"device":effect["device"],"params":{"7":0.5}}),
    );
    let status = call("peel_status", json!({}));
    assert_eq!(status["missing_devices"], json!([]));
}
