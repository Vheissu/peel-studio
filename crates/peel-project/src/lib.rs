//! Project persistence for Peel Studio.
//!
//! A project on disk is a small root manifest plus a directory of immutable,
//! content-addressed shards:
//!
//! ```text
//! Song.peelproj                      root manifest (JSON), replaced atomically
//! Song.peeldata/
//!   objects/<blake3>.json            core, pattern and arrangement shards
//!   media/<blake3>.<ext>             managed audio
//!   history/<seq>-<hash16>.peelproj  previous roots, for recovery
//!   lock                             advisory session lock
//! ```
//!
//! * [`save`] writes new shards first, then atomically replaces the root;
//!   unchanged shards are not rewritten.
//! * [`load`] verifies every shard against its hash and validates the
//!   project; if the latest root is damaged it falls back to the newest
//!   history root that loads.
//! * [`export_bundle`] / [`import_bundle`] move a project as one hardened ZIP.
//! * [`autosave`] reuses the same save path under a private directory.
//! * [`save_template`] stores a self-contained copy in a template library.
//! * [`ProjectLock`] keeps two sessions from writing one project.
//!
//! Unknown fields from a newer minor version are reported as warnings on
//! load and are not preserved on save, except for `Project::extensions`.

mod autosave;
mod bundle;
mod error;
mod history;
mod load;
mod lock;
mod manifest;
mod media;
mod save;
mod shard;
mod template;
mod util;

pub use autosave::{
    AutosaveEntry, autosave, autosave_from, autosave_path, discard_autosave, list_autosaves,
};
pub use bundle::{BundleLimits, BundleReport, export_bundle, import_bundle, import_bundle_with};
pub use error::{ProjectError, Result};
pub use load::{LoadedProject, load, read_manifest};
pub use lock::{LockInfo, LockStatus, ProjectLock};
pub use manifest::{MANIFEST_FORMAT, Manifest, ObjectIndex, ObjectRef};
pub use media::{AudioProbe, data_dir_for, import_asset, import_asset_with, resolve_asset_path};
pub use save::{
    DEFAULT_HISTORY_LIMIT, GcReport, SaveOptions, SaveReport, collect_garbage,
    required_capabilities, save, save_as, save_with,
};
pub use template::{
    MAX_TEMPLATE_NAME_CHARS, TemplateEntry, TemplateError, list_templates, load_template,
    save_template, template_name,
};
