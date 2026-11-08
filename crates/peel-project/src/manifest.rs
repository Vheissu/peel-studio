//! The root manifest (`Song.peelproj`): a small JSON file naming the
//! immutable shards that make up one saved revision.

use peel_document::Id;
use serde::{Deserialize, Serialize};

use crate::error::{ProjectError, Result};
use crate::util::is_hash;

pub const MANIFEST_FORMAT: &str = "peel-project-manifest";

/// Largest root manifest or history entry that will be read.
pub(crate) const MAX_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    /// Integer schema of the project model this root describes.
    pub schema: u32,
    pub project_id: Id,
    /// Document revision at the time of the save.
    pub revision: u64,
    pub title: String,
    pub app_version: String,
    /// RFC 3339 UTC time of the save.
    pub saved_at: String,
    /// Device type ids the project needs (channel devices and mixer inserts).
    pub required_capabilities: Vec<String>,
    /// Name of the data directory beside the root, informational.
    pub data_dir: String,
    pub objects: ObjectIndex,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectIndex {
    /// Meta, tempo, signature, channels, mixer, assets and extensions.
    pub core: ObjectRef,
    /// In project order.
    pub patterns: Vec<ObjectRef>,
    /// In project order.
    pub arrangements: Vec<ObjectRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectRef {
    /// Id of the pattern or arrangement (the project id for the core shard).
    pub id: Id,
    /// BLAKE3 hex digest of the shard's bytes; also its file name stem.
    pub hash: String,
    /// Shard length in bytes.
    pub size: u64,
}

impl Manifest {
    /// Every shard the manifest references, core first.
    pub fn objects(&self) -> impl Iterator<Item = &ObjectRef> {
        std::iter::once(&self.objects.core)
            .chain(self.objects.patterns.iter())
            .chain(self.objects.arrangements.iter())
    }

    /// Parses and checks a root manifest: format, schema range and hash shape.
    pub fn parse(bytes: &[u8]) -> Result<Manifest> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| ProjectError::NotAProject(format!("root is not valid JSON: {e}")))?;
        match value.get("format").and_then(|f| f.as_str()) {
            Some(MANIFEST_FORMAT) => {}
            other => {
                return Err(ProjectError::NotAProject(format!(
                    "unexpected format {other:?}"
                )));
            }
        }
        // Check the schema before the full parse so a future layout reports as
        // "newer version" rather than as a parse error.
        if let Some(schema) = value.get("schema").and_then(|s| s.as_u64()) {
            if schema > u64::from(peel_document::SCHEMA_VERSION) {
                return Err(ProjectError::FutureSchema {
                    found: u32::try_from(schema).unwrap_or(u32::MAX),
                    supported: peel_document::SCHEMA_VERSION,
                });
            }
            if schema == 0 {
                return Err(ProjectError::malformed("root manifest", "schema is 0"));
            }
        }
        let manifest: Manifest = serde_json::from_value(value)
            .map_err(|e| ProjectError::malformed("root manifest", e))?;
        for obj in manifest.objects() {
            if !is_hash(&obj.hash) {
                return Err(ProjectError::malformed(
                    "root manifest",
                    format!("object hash {:?} is not a BLAKE3 digest", obj.hash),
                ));
            }
        }
        Ok(manifest)
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("manifest serializes");
        bytes.push(b'\n');
        bytes
    }
}
