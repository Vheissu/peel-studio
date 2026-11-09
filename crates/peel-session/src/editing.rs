//! Validation for control-side edits from scripts and external clients.

use crate::Session;
use peel_document::{ClipContent, Id, Project, Time};
use serde_json::Value;
use std::sync::Arc;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Bound time arithmetic before deserializing untrusted rationals into the document.
pub fn time(value: &str) -> Result<Time, String> {
    let (num, den) = value.split_once('/').unwrap_or((value, "1"));
    let num: i64 = num.trim().parse().map_err(err)?;
    let den: i64 = den.trim().parse().map_err(err)?;
    if num.unsigned_abs() > 1_000_000_000_000 || !(1..=1_000_000).contains(&den) {
        return Err("Musical time exceeds supported edit bounds".into());
    }
    let time = Time::try_new(num.into(), den.into()).map_err(err)?;
    if time.as_f64().abs() > 10_000_000.0 {
        return Err("Musical time exceeds 10000000 beats".into());
    }
    Ok(time)
}
pub fn check_times(value: &Value, key: Option<&str>) -> Result<(), String> {
    check_changed_times(value, &Value::Null, key)
}
/// Already loaded unchanged times need not pass the stricter editing bounds.
pub fn check_changed_times(value: &Value, before: &Value, key: Option<&str>) -> Result<(), String> {
    if value == before {
        return Ok(());
    }
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                // Namespaced extension data is opaque JSON, not document time.
                if key == "extensions" {
                    continue;
                }
                check_changed_times(value, &before[key], Some(key))?;
            }
        }
        Value::Array(values) => {
            for (i, value) in values.iter().enumerate() {
                check_changed_times(value, &before[i], key)?;
            }
        }
        Value::String(s)
            if key.is_some_and(|k| {
                matches!(
                    k,
                    "start"
                        | "length"
                        | "offset"
                        | "position"
                        | "fade_in"
                        | "fade_out"
                        | "loop_range"
                )
            }) =>
        {
            time(s)?;
        }
        _ => {}
    }
    Ok(())
}
pub fn validate_edit(p: &Project) -> Result<(), String> {
    if p.mixer.tracks.len() > peel_engine::MAX_TRACKS {
        return Err(format!(
            "At most {} mixer tracks are supported",
            peel_engine::MAX_TRACKS
        ));
    }
    let errors = peel_document::validate(p)
        .into_iter()
        .filter(|i| i.is_error())
        .map(|i| i.message)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    if p.tempo.is_empty()
        || p.tempo[0].start != Time::ZERO
        || p.tempo.iter().any(|t| {
            !(1.0..=999.0).contains(&t.bpm)
                || t.ramp_to.is_some_and(|b| !(1.0..=999.0).contains(&b))
        })
        || p.tempo.windows(2).any(|t| t[1].start <= t[0].start)
    {
        return Err("Tempo must start at zero, be sorted and have BPM in 1..=999".into());
    }
    if p.time_signature.numerator == 0
        || !p.time_signature.denominator.is_power_of_two()
        || p.time_signature.denominator > 64
        || !(0.0..=1.0).contains(&p.swing)
    {
        return Err("Invalid time signature or swing".into());
    }
    if !p.arrangements.iter().any(|a| a.id == p.active_arrangement) {
        return Err("Active arrangement does not exist".into());
    }
    for (gain, pan) in p
        .channels
        .iter()
        .map(|c| (c.gain_db, c.pan))
        .chain(p.mixer.tracks.iter().map(|t| (t.gain_db, t.pan)))
    {
        if !gain.is_finite() || !(-1.0..=1.0).contains(&pan) {
            return Err("Invalid gain or pan".into());
        }
    }
    for n in p.patterns.iter().flat_map(|p| &p.notes) {
        if n.start.is_negative()
            || !(0.0..=1.0).contains(&n.probability)
            || !(0.0..=1.0).contains(&n.release)
            || !(-1.0..=1.0).contains(&n.pan)
            || !n.fine.is_finite()
        {
            return Err("Invalid note start, probability, release, pan or fine pitch".into());
        }
    }
    for a in &p.arrangements {
        if a.loop_range.is_some_and(|(a, b)| a.is_negative() || a >= b) {
            return Err("Invalid loop range".into());
        }
        for c in &a.clips {
            if c.start.is_negative() || c.offset.is_negative() {
                return Err("Clip positions must be nonnegative".into());
            }
            if let ClipContent::Audio {
                source_bpm: Some(b),
                ..
            } = &c.content
                && !(1.0..=999.0).contains(b)
            {
                return Err("Invalid source BPM".into());
            }
            if let ClipContent::Automation { points, .. } = &c.content
                && points
                    .iter()
                    .any(|p| p.position.is_negative() || !(-1.0..=1.0).contains(&p.tension))
            {
                return Err("Invalid automation position or tension".into());
            }
        }
    }
    for d in p
        .channels
        .iter()
        .map(|c| &c.device)
        .chain(p.mixer.tracks.iter().flat_map(|t| &t.inserts))
    {
        if let Some(desc) = peel_devices::descriptor(&d.type_id) {
            for (key, value) in &d.params {
                let param = desc
                    .param_by_key(key)
                    .ok_or_else(|| format!("Unknown parameter {key} on {}", d.type_id))?;
                if !value.is_finite() || !(param.min..=param.max).contains(value) {
                    return Err(format!(
                        "{key} must be between {} and {}",
                        param.min, param.max
                    ));
                }
            }
        }
    }
    Ok(())
}

impl Session {
    /// Commits a validated snapshot as one undo step, rejecting stale work.
    pub fn apply_project_edit(
        &mut self,
        project_id: Id,
        revision: u64,
        label: &str,
        mut project: Project,
    ) -> Result<bool, String> {
        if self.is_recording() {
            return Err("Stop recording before editing the project".into());
        }
        let current = self.project();
        if current.id != project_id || current.revision != revision {
            return Err("The project changed while the script ran. Run it again.".into());
        }
        if project.id != current.id
            || project.revision != current.revision
            || project.format != current.format
            || project.schema != current.schema
        {
            return Err("Project identity, format, schema and revision cannot be changed".into());
        }
        if &project == self.project() {
            return Ok(false);
        }
        validate_edit(&project)?;
        for pattern in &mut project.patterns {
            Arc::make_mut(pattern).sort_notes();
        }
        if &project == self.project() {
            return Ok(false);
        }
        self.doc.edit(label, |p| *p = project);
        Ok(true)
    }
}
