//! One-shot editor actions. Runtimes own snapshots, never the live session.
mod javascript;
mod lua;
mod process;
mod python;
mod worker;
pub use python::{bundled_python, default_python_label};
pub use worker::dispatch as dispatch_worker;

use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use peel_document::{Id, Project};
use peel_session::Session;
use serde::{Deserialize, Serialize};

const MAX_SOURCE: usize = 1024 * 1024;
const MAX_JSON: usize = 16 * 1024 * 1024;
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const TIME_LIMIT: Duration = Duration::from_secs(2);
const MAX_LOG_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    JavaScript,
    Lua,
    Python,
}
impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::JavaScript => "JavaScript",
            Self::Lua => "Lua",
            Self::Python => "Python",
        })
    }
}

#[derive(Clone, Debug)]
pub struct Script {
    pub name: String,
    pub language: Language,
    pub source: String,
}
impl Script {
    /// Reads text only; bytecode and unknown extensions aren't accepted.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let language = match path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("js") => Language::JavaScript,
            Some("lua") => Language::Lua,
            Some("py") => Language::Python,
            _ => return Err("Choose a .js, .lua or .py script".into()),
        };
        let mut source = String::new();
        std::fs::File::open(path)
            .map_err(error)?
            .take(MAX_SOURCE as u64 + 1)
            .read_to_string(&mut source)
            .map_err(error)?;
        if source.len() > MAX_SOURCE {
            return Err("Scripts must be at most 1 MiB".into());
        }
        Ok(Self {
            name: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            language,
            source,
        })
    }
}

/// Selection at the moment Run was pressed. IDs are UUID strings in both languages.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ScriptContext {
    pub pattern: Option<Id>,
    pub channel: Option<Id>,
    pub notes: Vec<Id>,
    pub clips: Vec<Id>,
}

pub fn builtin_scripts() -> Vec<Script> {
    [
        (
            "Transpose up an octave",
            Language::JavaScript,
            include_str!("../../../assets/scripts/transpose.js"),
        ),
        (
            "Transpose down an octave",
            Language::Lua,
            include_str!("../../../assets/scripts/transpose.lua"),
        ),
        (
            "Project summary",
            Language::JavaScript,
            include_str!("../../../assets/scripts/summary.js"),
        ),
        (
            "Project summary",
            Language::Lua,
            include_str!("../../../assets/scripts/summary.lua"),
        ),
        (
            "PyTheory: C major chord",
            Language::Python,
            include_str!("../../../assets/scripts/pytheory_chord.py"),
        ),
    ]
    .into_iter()
    .map(|(name, language, source)| Script {
        name: name.into(),
        language,
        source: source.into(),
    })
    .collect()
}

pub struct ScriptOutput {
    pub name: String,
    pub logs: Vec<String>,
    project_id: Id,
    revision: u64,
    project: Project,
    before: Project,
    // Cancellation still wins if execution finished but the UI has not applied it.
    cancelled: Arc<AtomicBool>,
}
impl ScriptOutput {
    /// Only this control-side call can affect the live session.
    pub fn apply(self, session: &mut Session) -> Result<bool, String> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err("Script cancelled".into());
        }
        if session.project().id != self.project_id || session.doc.revision() != self.revision {
            return Err("The project changed while the script ran. Run it again.".into());
        }
        if self.project == self.before {
            return Ok(false);
        }
        if session.project() != &self.before {
            return Err("The project changed while the script ran. Run it again.".into());
        }
        session.apply_project_edit(
            self.project_id,
            self.revision,
            &format!("Script: {}", self.name),
            self.project,
        )
    }
}

pub struct ScriptJob {
    cancelled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<ScriptOutput, String>>,
    finished: bool,
    worker: Option<std::thread::JoinHandle<()>>,
    isolated: bool,
}
impl ScriptJob {
    pub fn start(script: Script, project: Project, context: ScriptContext) -> Self {
        Self::start_with_python(script, project, context, None)
    }
    /// Cooperative in-process VM execution, for embedded hosts and tests.
    /// Desktop hosts should use start_isolated to stop native library stalls.
    /// Selects Python without changing process globals.
    pub fn start_with_python(
        script: Script,
        project: Project,
        context: ScriptContext,
        python: Option<std::path::PathBuf>,
    ) -> Self {
        Self::start_job(script, project, context, python, false)
    }
    /// Uses this executable's dispatch_worker() mode, so native VM calls can be killed.
    pub fn start_isolated(
        script: Script,
        project: Project,
        context: ScriptContext,
        python: Option<std::path::PathBuf>,
    ) -> Self {
        Self::start_job(script, project, context, python, true)
    }
    fn start_job(
        script: Script,
        project: Project,
        context: ScriptContext,
        python: Option<std::path::PathBuf>,
        isolated: bool,
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        // Only owned data crosses the thread boundary; neither VM is Send.
        let process_backed = isolated || script.language == Language::Python;
        let spawn = std::thread::Builder::new()
            .name("peel-script".into())
            .spawn(move || {
                let result =
                    run_with_python(script, project, context, worker_cancel, python, isolated);
                let _ = sender.send(result);
            });
        // A failed spawn drops sender; poll reports a worker failure.
        let worker = spawn.ok();
        Self {
            cancelled,
            receiver,
            finished: false,
            worker,
            isolated: process_backed,
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn poll(&mut self) -> Option<Result<ScriptOutput, String>> {
        if self.finished {
            return None;
        }
        match self.receiver.try_recv() {
            Ok(result) => {
                self.finished = true;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.finished = true;
                Some(Err("Script worker stopped unexpectedly".into()))
            }
        }
    }
}
impl Drop for ScriptJob {
    fn drop(&mut self) {
        // A polled result may outlive this job until it is applied.
        if !self.finished {
            self.cancel();
        }
        // Normal app shutdown must reap an isolated VM before the host exits.
        if self.isolated
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Clone)]
struct Budget {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
    logs: Arc<Mutex<Vec<String>>>,
}
impl Budget {
    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err("Script cancelled".into());
        }
        if Instant::now() >= self.deadline {
            return Err("Script exceeded its time limit".into());
        }
        Ok(())
    }
    fn log(&self, text: String) {
        let mut logs = self.logs.lock().unwrap();
        let used = logs.iter().map(|l| l.len() + 1).sum::<usize>();
        let remaining = MAX_LOG_BYTES.saturating_sub(used);
        if remaining == 0 || logs.len() >= 256 {
            return;
        }
        let mut end = text.len().min(remaining);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        logs.push(text[..end].to_string());
    }
}

#[cfg(test)]
fn run(
    script: Script,
    project: Project,
    context: ScriptContext,
    cancelled: Arc<AtomicBool>,
) -> Result<ScriptOutput, String> {
    run_with_python(script, project, context, cancelled, None, false)
}
fn run_with_python(
    script: Script,
    project: Project,
    context: ScriptContext,
    cancelled: Arc<AtomicBool>,
    python: Option<std::path::PathBuf>,
    isolated: bool,
) -> Result<ScriptOutput, String> {
    if script.source.len() > MAX_SOURCE {
        return Err("Scripts must be at most 1 MiB".into());
    }
    let budget = Budget {
        cancelled: cancelled.clone(),
        deadline: Instant::now()
            + if script.language == Language::Python {
                Duration::from_secs(10)
            } else {
                TIME_LIMIT
            },
        logs: Default::default(),
    };
    budget.check()?;
    let mut wire = serde_json::to_value(&project).map_err(error)?;
    // Both JS numbers and Lua's unsigned conversion can lose high u64 bits.
    // These two document fields are decimal strings in API v1 in every language.
    wire["seed"] = project.seed.to_string().into();
    wire["revision"] = project.revision.to_string().into();
    check_safe_integers(&wire)?;
    let input = serde_json::to_string(&wire).map_err(error)?;
    if input.len() > MAX_JSON {
        return Err("Project is too large for a script (16 MiB limit)".into());
    }
    let context = serde_json::to_string(&context).map_err(error)?;
    let result = if isolated && script.language != Language::Python {
        worker::execute(script.language, &script.source, &input, &context, &budget)
    } else {
        match script.language {
            Language::JavaScript => javascript::execute(&script.source, &input, &context, &budget),
            Language::Lua => lua::execute(&script.source, &input, &context, &budget),
            Language::Python => {
                python::execute(&script.source, &input, &context, &budget, python.as_deref())
            }
        }
    };
    // Preserve a useful limit/cancellation message over a VM's generic interruption.
    budget.check()?;
    let output = result.map_err(|e| format!("{}: {e}", script.name))?;
    if output.len() > MAX_JSON {
        return Err("Script result exceeds 16 MiB".into());
    }
    let mut value: serde_json::Value = serde_json::from_str(&output).map_err(error)?;
    for field in ["seed", "revision"] {
        let number = value
            .get(field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("{field} must remain a decimal string"))?
            .parse::<u64>()
            .map_err(error)?;
        value[field] = number.into();
    }
    peel_session::editing::check_changed_times(&value, &wire, None)?;
    let edited: Project = serde_json::from_value(value).map_err(error)?;
    // Scripts may reuse existing assets, but cannot make the host read new paths.
    if edited.assets != project.assets {
        return Err("Scripts cannot change audio asset files".into());
    }
    if edited != project {
        peel_session::editing::validate_edit(&edited)?;
    }
    Ok(ScriptOutput {
        name: script.name,
        logs: budget.logs.lock().unwrap().clone(),
        project_id: project.id,
        revision: project.revision,
        project: edited,
        before: project,
        cancelled,
    })
}
fn check_safe_integers(value: &serde_json::Value) -> Result<(), String> {
    use serde_json::Value;
    match value {
        Value::Number(n)
            if n.as_u64().is_some_and(|n| n > 9_007_199_254_740_991)
                || n.as_i64().is_some_and(|n| n < -9_007_199_254_740_991) =>
        {
            return Err(
                "Project extension data contains an integer outside the scripting safe range"
                    .into(),
            );
        }
        Value::Object(fields) => {
            for v in fields.values() {
                check_safe_integers(v)?;
            }
        }
        Value::Array(values) => {
            for v in values {
                check_safe_integers(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn error(e: impl fmt::Display) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests;
