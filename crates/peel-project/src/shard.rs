//! Encoding and decoding of the immutable, content-addressed shards.
//!
//! Three kinds exist: one `core` shard (everything except patterns and
//! arrangements), one `pattern` shard per pattern and one `arrangement`
//! shard per arrangement. Shards are compact JSON with a fixed field order,
//! so identical content always produces identical bytes and an identical
//! hash.

use std::collections::BTreeMap;

use peel_document::{
    Arrangement, AudioAsset, Channel, ClipContent, Id, Mixer, Pattern, Project, ProjectMeta,
    TempoSegment, TimeSignature,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ProjectError, Result};
use crate::manifest::ObjectRef;
use crate::util::hash_hex;

pub(crate) const KIND_CORE: &str = "core";
pub(crate) const KIND_PATTERN: &str = "pattern";
pub(crate) const KIND_ARRANGEMENT: &str = "arrangement";

/// A serialized shard ready to be stored.
pub(crate) struct Encoded {
    pub reference: ObjectRef,
    pub bytes: Vec<u8>,
}

/// `u64` as a decimal string: a generic JSON reader would lose precision on
/// values above 2^53.
mod u64_string {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Text(String),
            Number(u64),
        }
        match Repr::deserialize(d)? {
            Repr::Text(s) => s.trim().parse().map_err(D::Error::custom),
            Repr::Number(n) => Ok(n),
        }
    }
}

#[derive(Serialize)]
struct CoreOut<'a> {
    kind: &'static str,
    meta: &'a ProjectMeta,
    tempo: &'a [TempoSegment],
    time_signature: TimeSignature,
    swing: f32,
    #[serde(with = "u64_string")]
    seed: u64,
    channels: &'a [Channel],
    mixer: &'a Mixer,
    assets: &'a [AudioAsset],
    active_arrangement: Id,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    extensions: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct CoreIn {
    pub kind: String,
    pub meta: ProjectMeta,
    pub tempo: Vec<TempoSegment>,
    pub time_signature: TimeSignature,
    pub swing: f32,
    #[serde(with = "u64_string")]
    pub seed: u64,
    pub channels: Vec<Channel>,
    pub mixer: Mixer,
    pub assets: Vec<AudioAsset>,
    pub active_arrangement: Id,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Serialize)]
struct PatternOut<'a> {
    kind: &'static str,
    pattern: &'a Pattern,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct PatternIn {
    pub kind: String,
    pub pattern: Pattern,
}

#[derive(Serialize)]
struct ArrangementOut<'a> {
    kind: &'static str,
    arrangement: &'a Arrangement,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct ArrangementIn {
    pub kind: String,
    pub arrangement: Arrangement,
}

/// Recursively orders object keys so free-form JSON (project extensions)
/// serializes identically regardless of how it was built.
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), sorted(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn finish(id: Id, value: &impl Serialize) -> Result<Encoded> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| ProjectError::Unsavable(format!("serialization failed: {e}")))?;
    Ok(Encoded {
        reference: ObjectRef {
            id,
            hash: hash_hex(&bytes),
            size: bytes.len() as u64,
        },
        bytes,
    })
}

pub(crate) fn encode_core(p: &Project) -> Result<Encoded> {
    let extensions = p
        .extensions
        .iter()
        .map(|(k, v)| (k.clone(), sorted(v)))
        .collect();
    finish(
        p.id,
        &CoreOut {
            kind: KIND_CORE,
            meta: &p.meta,
            tempo: &p.tempo,
            time_signature: p.time_signature,
            swing: p.swing,
            seed: p.seed,
            channels: &p.channels,
            mixer: &p.mixer,
            assets: &p.assets,
            active_arrangement: p.active_arrangement,
            extensions,
        },
    )
}

pub(crate) fn encode_pattern(p: &Pattern) -> Result<Encoded> {
    finish(
        p.id,
        &PatternOut {
            kind: KIND_PATTERN,
            pattern: p,
        },
    )
}

pub(crate) fn encode_arrangement(a: &Arrangement) -> Result<Encoded> {
    finish(
        a.id,
        &ArrangementOut {
            kind: KIND_ARRANGEMENT,
            arrangement: a,
        },
    )
}

/// Decodes a shard of type `T`, returning warnings for fields in the file
/// that this version does not know and therefore will not keep.
pub(crate) fn decode<T>(bytes: &[u8], what: &str, kind: &str) -> Result<(T, Vec<String>)>
where
    T: DeserializeOwned + Serialize,
{
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| ProjectError::malformed(what, e))?;
    match value.get("kind").and_then(Value::as_str) {
        Some(k) if k == kind => {}
        other => {
            return Err(ProjectError::malformed(
                what,
                format!("expected a {kind:?} shard, found kind {other:?}"),
            ));
        }
    }
    let typed = T::deserialize(&value).map_err(|e| ProjectError::malformed(what, e))?;
    let warnings = unknown_field_warnings(&value, &typed, what);
    Ok((typed, warnings))
}

/// Compares the keys present in the file with the keys of the parsed value
/// serialized back out. Anything only in the file is dropped on the next save.
pub(crate) fn unknown_field_warnings(
    file: &Value,
    typed: &impl Serialize,
    what: &str,
) -> Vec<String> {
    let Ok(typed) = serde_json::to_value(typed) else {
        return Vec::new();
    };
    let mut found: BTreeMap<String, usize> = BTreeMap::new();
    collect_unknown(file, &typed, &mut String::new(), &mut found);
    found
        .into_iter()
        .map(|(path, n)| {
            format!(
                "{what}: field `{path}` is not understood by this version and will not be kept \
                 when the project is saved ({n} occurrence{})",
                if n == 1 { "" } else { "s" }
            )
        })
        .collect()
}

fn collect_unknown(
    file: &Value,
    typed: &Value,
    path: &mut String,
    out: &mut BTreeMap<String, usize>,
) {
    match (file, typed) {
        (Value::Object(f), Value::Object(t)) => {
            for (key, fv) in f {
                // Project extensions are free-form and always preserved.
                if path.is_empty() && key == "extensions" {
                    continue;
                }
                let len = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
                match t.get(key) {
                    Some(tv) => collect_unknown(fv, tv, path, out),
                    None => *out.entry(path.clone()).or_default() += 1,
                }
                path.truncate(len);
            }
        }
        (Value::Array(f), Value::Array(t)) => {
            let len = path.len();
            path.push_str("[]");
            for (fv, tv) in f.iter().zip(t) {
                collect_unknown(fv, tv, path, out);
            }
            path.truncate(len);
        }
        _ => {}
    }
}

/// Floating-point values must be finite: JSON has no NaN or infinity, and a
/// `null` where a number belongs would make the project unreadable.
pub(crate) fn check_finite(p: &Project) -> Result<()> {
    let mut bad: Vec<String> = Vec::new();
    let mut chk = |what: String, v: f64| {
        if !v.is_finite() {
            bad.push(format!("{what} is not a finite number"));
        }
    };
    chk("swing".into(), f64::from(p.swing));
    for s in &p.tempo {
        chk("tempo bpm".into(), s.bpm);
        if let Some(r) = s.ramp_to {
            chk("tempo ramp target".into(), r);
        }
    }
    for c in &p.channels {
        chk(format!("channel {} gain", c.name), f64::from(c.gain_db));
        chk(format!("channel {} pan", c.name), f64::from(c.pan));
        for (k, v) in &c.device.params {
            chk(format!("channel {} parameter {k}", c.name), f64::from(*v));
        }
    }
    for t in &p.mixer.tracks {
        chk(format!("mixer track {} gain", t.name), f64::from(t.gain_db));
        chk(format!("mixer track {} pan", t.name), f64::from(t.pan));
        for d in &t.inserts {
            for (k, v) in &d.params {
                chk(
                    format!("mixer track {} parameter {k}", t.name),
                    f64::from(*v),
                );
            }
        }
    }
    for pat in &p.patterns {
        for n in &pat.notes {
            for (field, v) in [
                ("velocity", n.velocity),
                ("release", n.release),
                ("pan", n.pan),
                ("fine", n.fine),
                ("probability", n.probability),
            ] {
                chk(format!("pattern {} note {field}", pat.name), f64::from(v));
            }
        }
    }
    for a in &p.arrangements {
        for c in &a.clips {
            if let ClipContent::Audio { gain_db, .. } = &c.content {
                chk(
                    format!("arrangement {} clip gain", a.name),
                    f64::from(*gain_db),
                );
            }
        }
    }
    bad.sort();
    bad.dedup();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(ProjectError::Unsavable(bad.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_survives_above_2_pow_53() {
        let mut p = Project::empty("Seed");
        p.seed = u64::MAX - 7;
        let enc = encode_core(&p).unwrap();
        let text = String::from_utf8(enc.bytes.clone()).unwrap();
        assert!(text.contains("\"seed\":\"18446744073709551608\""), "{text}");
        let (core, warnings): (CoreIn, _) = decode(&enc.bytes, "core", KIND_CORE).unwrap();
        assert_eq!(core.seed, p.seed);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn identical_content_has_identical_hash() {
        let p = Project::empty("Same");
        let a = encode_pattern(&p.patterns[0]).unwrap();
        let b = encode_pattern(&p.patterns[0]).unwrap();
        assert_eq!(a.reference.hash, b.reference.hash);
    }

    #[test]
    fn unknown_fields_are_reported() {
        let p = Project::empty("Unknown");
        let enc = encode_pattern(&p.patterns[0]).unwrap();
        let mut v: Value = serde_json::from_slice(&enc.bytes).unwrap();
        v["pattern"]["future_field"] = Value::from(1);
        v["extra"] = Value::from("x");
        let bytes = serde_json::to_vec(&v).unwrap();
        let (_, warnings): (PatternIn, _) = decode(&bytes, "pattern", KIND_PATTERN).unwrap();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("pattern.future_field")));
    }

    #[test]
    fn wrong_kind_is_rejected() {
        let p = Project::empty("Kind");
        let enc = encode_pattern(&p.patterns[0]).unwrap();
        assert!(decode::<CoreIn>(&enc.bytes, "core", KIND_CORE).is_err());
    }

    #[test]
    fn non_finite_floats_are_refused() {
        let mut p = Project::empty("NaN");
        p.swing = f32::NAN;
        assert!(matches!(check_finite(&p), Err(ProjectError::Unsavable(_))));
    }
}
