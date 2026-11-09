//! The app and CLI dispatch this private mode before initializing UI or audio.
use super::{Budget, Language, MAX_JSON, TIME_LIMIT, error};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Serialize, Deserialize)]
struct Request {
    language: Language,
    source: String,
    project: String,
    context: String,
}
#[derive(Serialize, Deserialize)]
struct Response {
    project: String,
    logs: Vec<String>,
}

pub(super) fn execute(
    language: Language,
    source: &str,
    project: &str,
    context: &str,
    budget: &Budget,
) -> Result<String, String> {
    let payload = serde_json::to_string(&Request {
        language,
        source: source.into(),
        project: project.into(),
        context: context.into(),
    })
    .map_err(error)?;
    let mut command = Command::new(std::env::current_exe().map_err(error)?);
    command.arg("--peel-script-worker");
    let bytes = super::process::execute(command, payload, budget)?;
    let response: Result<Response, String> = serde_json::from_slice(&bytes).map_err(error)?;
    let response = response?;
    for line in response.logs {
        budget.log(line);
    }
    Ok(response.project)
}

/// Call at the start of a host's main(), before audio/UI initialization.
/// Returns true when the private worker request was handled.
pub fn dispatch() -> bool {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new("--peel-script-worker")) {
        return false;
    }
    let result = (|| -> Result<Response, String> {
        // The payload is one line; later lines on stdin are host-call replies.
        let bytes = super::host::read_line(&mut std::io::stdin().lock(), MAX_JSON * 3)
            .map_err(|_| "Script worker input is too large".to_string())?
            .ok_or("Script worker received no input")?;
        let request: Request = serde_json::from_slice(&bytes).map_err(error)?;
        if request.source.len() > super::MAX_SOURCE || request.project.len() > MAX_JSON {
            return Err("Script worker input is too large".into());
        }
        let budget = Budget::new(
            Arc::new(AtomicBool::new(false)),
            TIME_LIMIT,
            super::host::Client::Pipe,
        );
        let project = match request.language {
            Language::JavaScript => super::javascript::execute(
                &request.source,
                &request.project,
                &request.context,
                &budget,
            ),
            Language::Lua => {
                super::lua::execute(&request.source, &request.project, &request.context, &budget)
            }
            Language::Python => Err("Python uses its own interpreter".into()),
        }?;
        budget.check()?;
        let logs = budget.logs.lock().unwrap().clone();
        Ok(Response { project, logs })
    })();
    let mut out = std::io::stdout().lock();
    if let Err(e) = serde_json::to_writer(&mut out, &result)
        .map_err(error)
        .and_then(|_| {
            out.write_all(b"\n")
                .and_then(|_| out.flush())
                .map_err(error)
        })
    {
        eprintln!("{e}");
    }
    true
}
