//! Exercise the real subprocess and stdio transport, without a display/audio device.
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Value>,
    sequence: u64,
    dir: std::path::PathBuf,
}
impl Client {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_peel-mcp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (send, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let value: Value = serde_json::from_str(&line.unwrap())
                    .expect("stdout must contain only MCP messages");
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        let dir = std::env::temp_dir().join(format!("peel-mcp-{}", peel_document::Id::new()));
        std::fs::create_dir(&dir).unwrap();
        Self {
            child,
            stdin,
            replies,
            sequence: 0,
            dir,
        }
    }
    fn send(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{value}").unwrap();
        stdin.flush().unwrap();
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.sequence += 1;
        let id = self.sequence;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let reply = self
                .replies
                .recv_timeout(Duration::from_secs(30))
                .expect("MCP response timeout");
            if reply["id"] == id {
                return reply;
            }
        }
    }
    fn initialize(&mut self) {
        let reply = self.request("initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"peel-test","version":"1"}}));
        assert_eq!(reply["result"]["protocolVersion"], "2025-11-25", "{reply}");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    }
    fn call(&mut self, name: &str, args: Value) -> Value {
        let reply = self.request(
            "tools/call",
            json!({"name":format!("peel_{name}"),"arguments":args}),
        );
        assert!(reply.get("error").is_none(), "{reply}");
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        reply["result"]["structuredContent"].clone()
    }
    fn finish(&mut self) {
        self.stdin.take();
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "server did not stop on EOF"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn agent_can_compose_save_reopen_import_and_render_over_stdio() {
    let mut c = Client::new();
    c.initialize();
    assert!(c.request("ping", json!({})).get("result").is_some());
    let listed = c.request("tools/list", json!({}));
    assert!(
        listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "peel_edit_project")
    );
    c.call("new_project", json!({"title":"Agent beat"}));
    let device = c.call("devices", json!({"type_id":"peel.drum.kick"}));
    assert!(
        !device["devices"][0]["params"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let channel = c.call(
        "add_channel",
        json!({"name":"Kick","type_id":"peel.drum.kick"}),
    );
    let doc = c.call("read_project", json!({}));
    let pattern = doc["value"]["patterns"][0]["id"].clone();
    let lane = doc["value"]["arrangements"][0]["tracks"][0]["id"].clone();
    c.call("set_notes",json!({"pattern":pattern,"channel":channel["channel"],"notes":[{"start":"0","length":"1/4","key":60,"velocity":0.9},{"start":"2","length":"1/4","key":60,"velocity":0.8}]}));
    c.call(
        "place_pattern",
        json!({"pattern":pattern,"lane":lane,"start":"0","length":"4"}),
    );
    let project_path = c.dir.join("agent.peelproj");
    c.call("save_project", json!({"path":project_path}));
    c.call("new_project", json!({"title":"Other"}));
    c.call("open_project", json!({"path":project_path}));
    let doc = c.call("read_project", json!({"pointer":"/patterns/0/notes"}));
    assert_eq!(doc["value"].as_array().unwrap().len(), 2);
    let midi = c.dir.join("notes.mid");
    c.call("export_midi", json!({"path":midi}));
    assert!(std::fs::read(&midi).unwrap().starts_with(b"MThd"));
    c.call("import_midi", json!({"path":midi}));
    let audio = c.dir.join("mix.wav");
    let job = c.call("export_audio", json!({"path":audio,"tail_seconds":0.0}))["job"].clone();
    let start = Instant::now();
    loop {
        let status = c.call("export_job", json!({"job":job}));
        if status["done"] == true {
            assert!(status["error"].is_null(), "{status}");
            assert_eq!(status["path"], json!(audio));
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(10));
    }
    let samples = peel_media::decode_file(&audio).unwrap();
    assert!(samples.frames() > 0);
    assert!(
        samples.left().iter().any(|v| v.abs() > 0.01),
        "render must contain audible music"
    );
    // Completion is retained, rather than consumed by the first poll.
    assert_eq!(c.call("export_job", json!({"job":job}))["done"], true);
    let browsing = c.call("browse_files", json!({"directory":c.dir,"limit":100}));
    assert!(
        browsing["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "mix.wav")
    );
    let imported = c.call(
        "import_audio",
        json!({"path":audio,"lane":lane,"start":"4"}),
    );
    assert!(imported["clip"].is_string());
    assert_eq!(
        c.call("read_project", json!({"pointer":"/assets"}))["value"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    c.finish();
}

#[test]
fn protocol_and_tool_errors_leave_the_server_usable() {
    let mut c = Client::new();
    c.initialize();
    let unknown = c.request("tools/call", json!({"name":"not_a_tool"}));
    assert!(unknown.get("error").is_some());
    let invalid = c.request(
        "tools/call",
        json!({"name":"peel_new_ids","arguments":{"count":0}}),
    );
    assert_eq!(invalid["result"]["isError"], true);
    let extra = c.request(
        "tools/call",
        json!({"name":"peel_status","arguments":{"action":"new_project"}}),
    );
    assert_eq!(extra["result"]["isError"], true);
    let malformed = c.request(
        "tools/call",
        json!({"name":"peel_live_note","arguments":{"channel":12}}),
    );
    assert_eq!(malformed["result"]["isError"], true);
    let status = c.call("status", json!({}));
    assert_eq!(status["title"], "Untitled");
    c.finish();
}
