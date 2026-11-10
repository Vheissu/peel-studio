//! `peel`: headless project tools that reuse the application's own code
//! paths (no simplified renderer).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32};

mod audition;

use peel_document::validate;
use peel_session::{AudioExportSettings, ExportSettings, render_audio, template};

const USAGE: &str = "usage:
  peel info <project>
  peel validate <project>
  peel render <project> <out.wav|mp3|flac|aiff> [options]
  peel devices
  peel songs
  peel audition <device type> <out.wav> [audition options]
  peel script <project> <action.js|lua|py> <out.peelproj>

<project> is a .peelproj file or a built-in song:
  --demo             The first demo song
  --demo=<title>     A demo song by title, as listed by `peel songs`
  --template=<name>  A song starter by name

audition plays a native device on its own: an instrument plays one note
per preset, an effect processes a drum hit and a chord. It prints the peak
and RMS level of each preset.
  --preset <name>   One preset instead of all of them
  --key <0-127>     Note to play (default: 60 for drums, 48 otherwise)
  --seconds <s>     Length per preset (default: 2)
  --set <key=value> Override a parameter after the preset (repeatable)

render options:
  --rate <Hz>       Sample rate (default: 48000)
  --bits <16|24|32> PCM bit depth; 32-bit float is WAV only (default: 24)
  --bitrate <kbps>  MP3 CBR bitrate: 128, 192, 256, 320 (default: 320)
  --tail <seconds>  Effect tail, 0 to 60 seconds (default: 3)
  --mono           Downmix to mono
  --normalize      Normalize each file to -1 dBFS peak
  --no-dither      Disable TPDF dither for integer PCM";

fn main() -> ExitCode {
    if peel_extensions::dispatch_worker() {
        return std::process::ExitCode::SUCCESS;
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("info") => info(&args[1..]),
        Some("validate") => check(&args[1..]),
        Some("render") => render(&args[1..]),
        Some("script") => script(&args[1..]),
        Some("audition") => audition::run(&args[1..]),
        Some("songs") => {
            for d in template::demos() {
                println!(
                    "--demo={:<22} {}, {} BPM, {}",
                    format!("\"{}\"", d.title),
                    d.style,
                    d.bpm,
                    d.key
                );
            }
            for (name, _) in template::templates() {
                println!("--template=\"{name}\"");
            }
            Ok(())
        }
        Some("devices") => {
            for d in peel_devices::all() {
                println!(
                    "{:<22} {:<10} {:<10} v{}  {} params",
                    d.type_id,
                    d.name,
                    d.category,
                    d.version,
                    d.params.len()
                );
            }
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// A built-in song named on the command line, if `src` is one:
/// `--demo`, `--demo=<title>` or `--template=<name>` (names ignore case).
fn builtin(src: &str) -> Option<Result<peel_document::Project, String>> {
    let demos = template::demos();
    if src == "--demo" {
        return demos.first().map(|d| Ok((d.make)()));
    }
    if let Some(title) = src.strip_prefix("--demo=") {
        return Some(
            demos
                .iter()
                .find(|d| d.title.eq_ignore_ascii_case(title))
                .map(|d| (d.make)())
                .ok_or_else(|| format!("No demo song called {title}; see `peel songs`")),
        );
    }
    let name = src.strip_prefix("--template=")?;
    Some(
        template::templates()
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, make)| make())
            .ok_or_else(|| format!("No template called {name}; see `peel songs`")),
    )
}

fn load(path: &str) -> Result<peel_document::Project, String> {
    if let Some(project) = builtin(path) {
        return project;
    }
    let loaded = peel_project::load(std::path::Path::new(path)).map_err(|e| e.to_string())?;
    for w in loaded.warnings {
        eprintln!("warning: {w}");
    }
    Ok(loaded.project)
}

fn info(args: &[String]) -> Result<(), String> {
    let p = load(args.first().ok_or(USAGE)?)?;
    println!("title:     {}", p.meta.title);
    println!(
        "tempo:     {:.2} BPM, {}/{}",
        p.bpm(),
        p.time_signature.numerator,
        p.time_signature.denominator
    );
    println!("channels:  {}", p.channels.len());
    for c in &p.channels {
        println!("  {:<16} {}", c.name, c.device.type_id);
    }
    println!("patterns:  {}", p.patterns.len());
    println!("clips:     {}", p.arrangement().clips.len());
    println!("length:    {} beats", p.arrangement().end());
    println!("mixer:     {} tracks", p.mixer.tracks.len());
    Ok(())
}

fn check(args: &[String]) -> Result<(), String> {
    let p = load(args.first().ok_or(USAGE)?)?;
    let issues = validate(&p);
    for i in &issues {
        println!("{:?}: {}", i.severity, i.message);
    }
    if issues.iter().any(|i| i.is_error()) {
        return Err("project has errors".into());
    }
    println!("ok");
    Ok(())
}

fn render_options(args: &[String]) -> Result<(&str, AudioExportSettings), String> {
    let src = args.first().ok_or(USAGE)?.as_str();
    let path = PathBuf::from(args.get(1).ok_or(USAGE)?);
    let format = peel_media::AudioFormat::from_extension(&path).map_err(|e| e.to_string())?;
    let mut settings = AudioExportSettings {
        render: ExportSettings {
            path,
            ..ExportSettings::default()
        },
        format,
        ..AudioExportSettings::default()
    };
    let mut i = 2;
    while i < args.len() {
        let option = args[i].as_str();
        match option {
            "--mono" => settings.mono = true,
            "--normalize" => settings.normalize = true,
            "--no-dither" => settings.dither = false,
            "--rate" | "--bits" | "--bitrate" | "--tail" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| format!("Missing value for {option}"))?;
                let bad = || format!("Invalid value for {option}: {value}");
                match option {
                    "--rate" => settings.render.sample_rate = value.parse().map_err(|_| bad())?,
                    "--bits" => settings.render.bits = value.parse().map_err(|_| bad())?,
                    "--bitrate" => settings.bitrate_kbps = value.parse().map_err(|_| bad())?,
                    "--tail" => settings.render.tail_seconds = value.parse().map_err(|_| bad())?,
                    _ => unreachable!(),
                }
            }
            other => return Err(format!("Unknown option {other}")),
        }
        i += 1;
    }
    settings.validate()?;
    Ok((src, settings))
}

fn render(args: &[String]) -> Result<(), String> {
    let (src, settings) = render_options(args)?;
    let rate = settings.render.sample_rate;
    let project = load(src)?;
    // Decode sample assets for sample-based devices.
    let base = builtin(src).is_none().then(|| PathBuf::from(src));
    let mut samples = HashMap::new();
    for a in &project.assets {
        let path = match (&base, a.managed) {
            (Some(b), true) => peel_project::resolve_asset_path(b, a),
            _ => PathBuf::from(&a.path),
        };
        match peel_media::load_for_engine(&path, rate as f32) {
            Ok(buf) => {
                samples.insert(a.id, Arc::new(buf));
            }
            Err(e) => eprintln!("warning: could not load {}: {e}", path.display()),
        }
    }
    let progress = AtomicU32::new(0);
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    render_audio(&project, &samples, &settings, &progress, &cancel)?;
    println!(
        "rendered {} in {:.2} s",
        settings.render.path.display(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Uses the same snapshot runner, validation and commit path as the desktop.
/// `peel.call` runs MCP actions against a silent session on this thread.
fn script(args: &[String]) -> Result<(), String> {
    if args.len() != 3 {
        return Err(USAGE.into());
    }
    let project = load(&args[0])?;
    let context = peel_extensions::ScriptContext {
        pattern: project.patterns.first().map(|p| p.id),
        channel: project.channels.first().map(|c| c.id),
        ..Default::default()
    };
    let action = peel_extensions::Script::from_path(std::path::Path::new(&args[1]))?;
    // The session exists before the script starts so live calls act on it.
    let mut session = peel_session::Session::new_silent(project.clone());
    if !args[0].starts_with("--demo") && !args[0].starts_with("--template=") {
        session.path = Some(std::fs::canonicalize(&args[0]).map_err(|e| e.to_string())?);
    }
    let mut control = peel_mcp::Control::default();
    let mut job = peel_extensions::ScriptJob::start_isolated(action, project, context, None);
    let result = loop {
        control.tick();
        job.pump(|name, arguments| live_call(&mut control, &mut session, name, arguments));
        if let Some(result) = job.poll() {
            break result;
        }
        session.tick();
        session.poll_midi();
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    let saved = result.and_then(|output| {
        for line in &output.logs {
            println!("{line}");
        }
        finish_exports(&mut control, &mut session);
        output.apply(&mut session)?;
        session.save(Some(PathBuf::from(&args[2])))
    });
    // Stops playback, previews, inputs and any export still running.
    control.disconnect(&mut session);
    println!("Saved {}", saved?.display());
    Ok(())
}

fn live_call(
    control: &mut peel_mcp::Control,
    session: &mut peel_session::Session,
    name: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let serde_json::Value::Object(arguments) = arguments else {
        return Err("Arguments must be an object".into());
    };
    let action = peel_mcp::decode(&format!("peel_{name}"), arguments)?;
    let result = control.execute(session, action);
    // Synchronize before the script observes an edit or playback change.
    session.tick();
    result
}

/// Exports a script started keep their files: wait rather than truncate them.
fn finish_exports(control: &mut peel_mcp::Control, session: &mut peel_session::Session) {
    let Ok(status) = live_call(control, session, "status", serde_json::json!({})) else {
        return;
    };
    for job in status["export_jobs"].as_array().into_iter().flatten() {
        let Some(job) = job.as_u64() else { continue };
        loop {
            control.tick();
            let state = live_call(
                control,
                session,
                "export_job",
                serde_json::json!({"job": job}),
            );
            match state {
                Ok(state) if state["done"] == false => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Ok(state) => {
                    if let Some(error) = state["error"].as_str() {
                        eprintln!("warning: export {job} failed: {error}");
                    }
                    break;
                }
                Err(_) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Result<AudioExportSettings, String> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        render_options(&args).map(|(_, options)| options)
    }

    #[test]
    fn format_comes_from_extension_and_flags_are_not_positional() {
        let o = options(&[
            "--demo",
            "song.MP3",
            "--mono",
            "--bitrate",
            "192",
            "--normalize",
            "--rate",
            "44100",
            "--tail",
            "0",
        ])
        .unwrap();
        assert_eq!(o.format, peel_media::AudioFormat::Mp3);
        assert_eq!(o.bitrate_kbps, 192);
        assert_eq!(o.render.sample_rate, 44_100);
        assert_eq!(o.render.tail_seconds, 0.0);
        assert!(o.mono && o.normalize);
        let o = options(&["--demo", "song.flac", "--bits", "16", "--no-dither"]).unwrap();
        assert_eq!(o.render.bits, 16);
        assert!(!o.dither);
    }

    #[test]
    fn invalid_options_fail_before_loading_the_project() {
        for args in [
            vec!["missing.peelproj", "song.mp3", "--rate", "96000"],
            vec!["--demo", "song.flac", "--bits", "32"],
            vec!["--demo", "song.wav", "--tail", "NaN"],
            vec!["--demo", "song.wav", "--tail", "-1"],
            vec!["--demo", "song.mp3", "--bitrate", "123"],
            vec!["--demo", "song.wav", "--bits"],
            vec!["--demo", "song.wav", "--wat"],
            vec!["--demo", "song.unknown"],
        ] {
            assert!(options(&args).is_err(), "{args:?}");
        }
    }
}
