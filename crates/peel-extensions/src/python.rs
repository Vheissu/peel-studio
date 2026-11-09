//! Optional trusted Python actions, hosted in a separate process.
use super::{Budget, error};
use serde_json::json;
use std::process::Command;

pub(super) fn execute(
    source: &str,
    project: &str,
    context: &str,
    budget: &Budget,
    executable: Option<&std::path::Path>,
) -> Result<String, String> {
    let executable = executable
        .map(|p| p.as_os_str().to_owned())
        .or_else(|| std::env::var_os("PEEL_PYTHON"))
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "python".into()
            } else {
                "python3".into()
            }
        });
    // Build input before starting a child so serialization failures cannot leak it.
    let payload = json!({"source":source,"project":serde_json::from_str::<serde_json::Value>(project).map_err(error)?,"context":serde_json::from_str::<serde_json::Value>(context).map_err(error)?}).to_string();
    let mut command = Command::new(executable);
    command.args(["-I", "-u", "-c", include_str!("python_host.py")]);
    let output = super::process::execute(command, payload, budget).map_err(|e| {
        format!("Python: {e}. Choose a Python 3.10+ interpreter with the required packages.")
    })?;
    let mut result: serde_json::Value =
        serde_json::from_slice(&output).map_err(|e| format!("Invalid Python result: {e}"))?;
    if let Some(logs) = result.get("logs").and_then(|l| l.as_array()) {
        for line in logs {
            if let Some(text) = line.as_str() {
                budget.log(text.into());
            }
        }
    }
    serde_json::to_string(&result["project"].take()).map_err(error)
}
