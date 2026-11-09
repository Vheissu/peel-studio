//! Live host calls. A script asks its owning thread to run one named action
//! and waits for the reply. Nothing here executes session logic; the host's
//! `ScriptJob::pump` callback does that on its own thread.
use super::{Budget, MAX_JSON, error};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// Child processes prefix host-call frames with this exact text.
pub(super) const FRAME: &[u8] = b"{\"peel_host_call\":";
const MAX_NAME: usize = 128;

pub(super) struct Request {
    pub name: String,
    pub args: Value,
    pub reply: mpsc::SyncSender<Result<Value, String>>,
    /// Set when the script stopped waiting; the host then skips the action.
    pub abandoned: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
pub(super) enum Client {
    /// Direct runs without a job (tests and validation helpers).
    #[default]
    None,
    /// A VM thread or process monitor talking to ScriptJob::pump.
    Channel(mpsc::SyncSender<Request>),
    /// An isolated child: frames on stdout, replies on stdin.
    Pipe,
}

/// The outer error stops the script (cancelled, deadline, broken host).
/// The inner error is the action's own failure, which scripts can catch.
pub(super) type Reply = Result<Result<String, String>, String>;

pub(super) fn call(budget: &Budget, name: &str, args: Value) -> Reply {
    budget.check()?;
    if name.is_empty() || name.len() > MAX_NAME {
        return Ok(Err("peel.call needs an action name".into()));
    }
    let args = match args {
        Value::Null => Value::Object(Default::default()),
        Value::Object(fields) => Value::Object(fields),
        _ => return Ok(Err("peel.call arguments must be an object".into())),
    };
    let began = std::time::Instant::now();
    let reply = match &budget.host {
        Client::None => Ok(Err("Live host calls are unavailable here".into())),
        Client::Channel(sender) => channel(budget, sender, name, args),
        Client::Pipe => pipe(name, args),
    };
    // Waiting for the host's thread doesn't count against the script's own budget.
    budget.waited(began.elapsed());
    match reply? {
        Ok(json) if json.len() > MAX_JSON => Ok(Err("Host call result exceeds 16 MiB".into())),
        other => Ok(other),
    }
}

fn channel(budget: &Budget, sender: &mpsc::SyncSender<Request>, name: &str, args: Value) -> Reply {
    let (reply, receiver) = mpsc::sync_channel(1);
    let abandoned = Arc::new(AtomicBool::new(false));
    let mut request = Some(Request {
        name: name.into(),
        args,
        reply,
        abandoned: abandoned.clone(),
    });
    let outcome = loop {
        if let Err(e) = budget.check_waiting() {
            break Err(e);
        }
        if let Some(pending) = request.take() {
            match sender.try_send(pending) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(pending)) => {
                    request = Some(pending);
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    break Ok(Err("The host is no longer accepting calls".into()));
                }
            }
        }
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(Ok(value)) => break Ok(serde_json::to_string(&value).map_err(error)),
            Ok(Err(e)) => break Ok(Err(e)),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Ok(Err("The host dropped the call".into()));
            }
        }
    };
    if outcome.is_err() {
        abandoned.store(true, Ordering::Relaxed);
    }
    outcome
}

// The parent enforces cancellation and deadlines by killing this process.
fn pipe(name: &str, args: Value) -> Reply {
    let frame = serde_json::json!({"peel_host_call": {"name": name, "args": args}}).to_string();
    if frame.len() > MAX_JSON {
        return Ok(Err("peel.call arguments exceed 16 MiB".into()));
    }
    let mut out = std::io::stdout().lock();
    out.write_all(frame.as_bytes())
        .and_then(|_| out.write_all(b"\n"))
        .and_then(|_| out.flush())
        .map_err(error)?;
    drop(out);
    let line = read_line(&mut std::io::stdin().lock(), MAX_JSON * 3 + 64)?
        .ok_or("The host closed the script channel")?;
    response(&line)
}

pub(super) fn response(line: &[u8]) -> Reply {
    let mut value: Value = serde_json::from_slice(line).map_err(error)?;
    if let Some(e) = value.get("error") {
        return Ok(Err(e.as_str().unwrap_or("Host call failed").into()));
    }
    let ok = value.get_mut("ok").ok_or("Invalid host reply")?.take();
    Ok(Ok(serde_json::to_string(&ok).map_err(error)?))
}

/// The parent's reply line for a child's host-call frame.
pub(super) fn encode(result: Result<String, String>) -> String {
    match result {
        Ok(json) => format!("{{\"ok\":{json}}}"),
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    }
}

/// Parse a child's host-call frame into name and arguments.
pub(super) fn decode(line: &[u8]) -> Result<(String, Value), String> {
    let mut value: Value =
        serde_json::from_slice(line).map_err(|e| format!("Invalid host call: {e}"))?;
    let call = value
        .get_mut("peel_host_call")
        .and_then(Value::as_object_mut)
        .ok_or("Invalid host call")?;
    let name = call
        .get("name")
        .and_then(Value::as_str)
        .ok_or("Invalid host call")?
        .to_string();
    Ok((name, call.remove("args").unwrap_or(Value::Null)))
}

/// One newline-terminated frame, or the unterminated tail at EOF.
pub(super) fn read_line(reader: &mut impl BufRead, cap: usize) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    let count = std::io::Read::take(&mut *reader, cap as u64 + 1)
        .read_until(b'\n', &mut line)
        .map_err(error)?;
    if count == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    } else if line.len() > cap {
        return Err("Script output exceeds its size limit".into());
    }
    Ok(Some(line))
}
