//! The app and CLI dispatch this private mode before initializing UI or audio.
use super::{Budget, Language, MAX_JSON, TIME_LIMIT, error};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Instant;

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
        let mut bytes = Vec::new();
        std::io::stdin()
            .take((MAX_JSON * 3 + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(error)?;
        if bytes.len() > MAX_JSON * 3 {
            return Err("Script worker input is too large".into());
        }
        let request: Request = serde_json::from_slice(&bytes).map_err(error)?;
        if request.source.len() > super::MAX_SOURCE || request.project.len() > MAX_JSON {
            return Err("Script worker input is too large".into());
        }
        let budget = Budget {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + TIME_LIMIT,
            logs: Default::default(),
        };
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
    if let Err(e) = serde_json::to_writer(std::io::stdout(), &result) {
        eprintln!("{e}");
    }
    true
}
