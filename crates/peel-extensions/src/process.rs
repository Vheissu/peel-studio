//! Bounded child process I/O shared by all scripting languages.
//!
//! Framing is one JSON value per line. The parent writes the initial payload
//! line, then one reply line per host-call frame the child prints. Any other
//! stdout line is the child's final result, which keeps its existing shape.
use super::{Budget, MAX_JSON, MAX_LOG_BYTES, error, host};
use std::io::{BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Final results may carry a 16 MiB project encoded inside a JSON string.
const MAX_FRAME: usize = MAX_JSON * 3;

fn read_capped(mut pipe: impl Read, cap: usize) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let count = pipe.read(&mut chunk).map_err(error)?;
        if count == 0 {
            return Ok(output);
        }
        let keep = count.min(cap.saturating_sub(output.len()));
        output.extend_from_slice(&chunk[..keep]);
    }
}

enum Event {
    Line(Vec<u8>),
    StdoutEnd(Result<(), String>),
    Stderr(Result<Vec<u8>, String>),
    WriteFailed(String),
}

pub(super) fn execute(
    mut command: Command,
    payload: String,
    budget: &Budget,
) -> Result<Vec<u8>, String> {
    if payload.contains('\n') {
        return Err("Script payload must be a single line".into());
    }
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
    // Bounded so a child printing junk can't queue unbounded host memory.
    let (sender, receiver) = mpsc::sync_channel(4);
    {
        let sender = sender.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match host::read_line(&mut reader, MAX_FRAME) {
                    Ok(Some(line)) => {
                        if sender.send(Event::Line(line)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = sender.send(Event::StdoutEnd(Ok(())));
                        return;
                    }
                    Err(e) => {
                        let _ = sender.send(Event::StdoutEnd(Err(e)));
                        return;
                    }
                }
            }
        });
    }
    {
        let sender = sender.clone();
        thread::spawn(move || {
            let _ = sender.send(Event::Stderr(read_capped(stderr, MAX_LOG_BYTES)));
        });
    }
    // stdin stays open for host replies until this sender is dropped.
    let (lines, outgoing) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in outgoing {
            let written = input
                .write_all(line.as_bytes())
                .and_then(|_| input.write_all(b"\n"))
                .and_then(|_| input.flush());
            if let Err(e) = written {
                let _ = sender.send(Event::WriteFailed(e.to_string()));
                return;
            }
        }
    });
    let _ = lines.send(payload);
    let mut lines = Some(lines);
    let mut result_frame = None;
    let mut stdout_done = false;
    let mut errors = None;
    let mut status = None;
    let mut write_error = None;
    // Even if a Python script spawns a child inheriting its pipes, never block
    // joining readers after interpreter exit. Cancellation and the deadline win.
    let result = (|| -> Result<Vec<u8>, String> {
        loop {
            budget.check()?;
            match receiver.recv_timeout(Duration::from_millis(5)) {
                Ok(Event::Line(line)) if line.starts_with(host::FRAME) => {
                    let (name, args) = host::decode(&line)?;
                    // Blocks this monitor, but still honours cancellation and the wall clock.
                    let reply = host::encode(host::call(budget, &name, args)?);
                    if let Some(lines) = &lines {
                        let _ = lines.send(reply);
                    }
                }
                Ok(Event::Line(line)) => {
                    if result_frame.is_some() {
                        return Err("Script worker printed unexpected output".into());
                    }
                    result_frame = Some(line);
                    // No host replies follow a final result.
                    lines = None;
                }
                Ok(Event::StdoutEnd(end)) => {
                    end?;
                    stdout_done = true;
                    lines = None;
                }
                Ok(Event::Stderr(output)) => errors = Some(output?),
                // Usually the child exited early; its own error explains why.
                Ok(Event::WriteFailed(e)) => write_error = Some(e),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            if status.is_none() {
                status = child.try_wait().map_err(error)?;
            }
            if let Some(status) = status
                && stdout_done
                && let Some(errors) = &errors
            {
                if !status.success() {
                    let detail = String::from_utf8_lossy(errors).trim().to_string();
                    return Err(if detail.is_empty() {
                        format!("Script worker exited with {status}")
                    } else {
                        detail
                    });
                }
                if let Some(e) = write_error.take() {
                    return Err(e);
                }
                return result_frame
                    .take()
                    .ok_or_else(|| "Script worker returned no result".into());
            }
        }
    })();
    drop(lines);
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}
