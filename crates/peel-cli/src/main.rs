//! `peel`: headless project tools that reuse the application's own code
//! paths (no simplified renderer).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32};

use peel_document::validate;
use peel_session::{ExportSettings, render_to_wav, template};

const USAGE: &str = "usage:
  peel info <project.peelproj>
  peel validate <project.peelproj>
  peel render <project.peelproj | --demo> <out.wav> [--rate 48000] [--bits 24]
  peel devices";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("info") => info(&args[1..]),
        Some("validate") => check(&args[1..]),
        Some("render") => render(&args[1..]),
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

fn load(path: &str) -> Result<peel_document::Project, String> {
    if path == "--demo" {
        return Ok(template::beat_and_melody());
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

fn render(args: &[String]) -> Result<(), String> {
    let src = args.first().ok_or(USAGE)?;
    let out = PathBuf::from(args.get(1).ok_or(USAGE)?);
    let mut rate = 48_000;
    let mut bits = 24;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--rate" => {
                rate = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("bad --rate")?
            }
            "--bits" => {
                bits = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("bad --bits")?
            }
            other => return Err(format!("unknown option {other}")),
        }
        i += 2;
    }
    let project = load(src)?;
    // Decode sample assets for sample-based devices.
    let base = (src != "--demo").then(|| PathBuf::from(src));
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
    let settings = ExportSettings {
        path: out.clone(),
        sample_rate: rate,
        bits,
        tail_seconds: 3.0,
    };
    let progress = AtomicU32::new(0);
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    render_to_wav(&project, &samples, &settings, &progress, &cancel)?;
    println!(
        "rendered {} in {:.2} s",
        out.display(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}
