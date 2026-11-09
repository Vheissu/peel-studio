//! Bounded child process I/O shared by all scripting languages.
use super::{Budget, MAX_JSON, MAX_LOG_BYTES, error};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn read_capped(mut pipe: impl Read, cap: usize, truncate: bool) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let count = pipe.read(&mut chunk).map_err(error)?;
        if count == 0 {
            return Ok(output);
        }
        if !truncate && output.len().saturating_add(count) > cap {
            return Err("Script output exceeds its size limit".into());
        }
        let keep = count.min(cap.saturating_sub(output.len()));
        output.extend_from_slice(&chunk[..keep]);
    }
}

pub(super) fn execute(
    mut command: Command,
    payload: String,
    budget: &Budget,
) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(error)?;
    let mut input = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    for (kind, pipe, cap) in [
        (0, Box::new(stdout) as Box<dyn Read + Send>, MAX_JSON * 3),
        (1, Box::new(stderr) as Box<dyn Read + Send>, MAX_LOG_BYTES),
    ] {
        let sender = sender.clone();
        thread::spawn(move || {
            let _ = sender.send((kind, read_capped(pipe, cap, kind == 1)));
        });
    }
    thread::spawn(move || {
        let _ = sender.send((
            2,
            input
                .write_all(payload.as_bytes())
                .map(|_| Vec::new())
                .map_err(error),
        ));
    });
    let mut results = [None, None, None];
    let mut status = None;
    // Even if a Python script spawns a child inheriting its pipes, never block
    // joining readers after interpreter exit. Cancellation and the deadline win.
    let result = (|| -> Result<(Vec<u8>, Vec<u8>), String> {
        loop {
            budget.check()?;
            while let Ok((kind, result)) = receiver.try_recv() {
                results[kind] = Some(result);
            }
            if let Some(error) = results
                .iter()
                .filter_map(|r| r.as_ref())
                .find_map(|r| r.as_ref().err())
            {
                return Err(error.clone());
            }
            if status.is_none() {
                status = child.try_wait().map_err(error)?;
            }
            if let Some(status) = status
                && results.iter().all(Option::is_some)
            {
                let output = results[0].take().unwrap()?;
                let errors = results[1].take().unwrap()?;
                if !status.success() {
                    let detail = String::from_utf8_lossy(&errors).trim().to_string();
                    return Err(if detail.is_empty() {
                        format!("Script worker exited with {status}")
                    } else {
                        detail
                    });
                }
                return Ok((output, errors));
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result.map(|(output, _)| output)
}
