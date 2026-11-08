use std::io;
use std::path::{Path, PathBuf};

use crate::lock::LockInfo;

pub type Result<T, E = ProjectError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("not a Peel project: {0}")]
    NotAProject(String),
    #[error(
        "project schema {found} is newer than the newest this version understands ({supported}); \
         update Peel Studio to open it"
    )]
    FutureSchema { found: u32, supported: u32 },
    #[error("malformed {what}: {message}")]
    Malformed { what: String, message: String },
    #[error("project object {hash} is missing")]
    MissingObject { hash: String },
    #[error("project object {hash} is corrupt: its content hashes to {actual}")]
    HashMismatch { hash: String, actual: String },
    #[error("project is invalid: {}", .0.join("; "))]
    Invalid(Vec<String>),
    #[error("cannot save: {0}")]
    Unsavable(String),
    #[error("unsafe path {path:?}: {reason}")]
    UnsafePath { path: String, reason: String },
    #[error("bundle: {0}")]
    Bundle(String),
    #[error("bundle entry {name:?} rejected: {reason}")]
    UnsafeBundleEntry { name: String, reason: String },
    #[error("bundle exceeds safety limits: {0}")]
    BundleLimit(String),
    #[error("{} already exists and is not empty", .0.display())]
    DestinationNotEmpty(PathBuf),
    #[error("project is locked by {0}")]
    Locked(LockInfo),
}

impl ProjectError {
    pub(crate) fn malformed(what: impl Into<String>, message: impl ToString) -> ProjectError {
        ProjectError::Malformed {
            what: what.into(),
            message: message.to_string(),
        }
    }

    /// True when an older saved root might still load: the latest root or one
    /// of its objects is unreadable, corrupt or invalid. A project from a newer
    /// schema is never "recovered" to an older version.
    pub(crate) fn is_recoverable(&self) -> bool {
        matches!(
            self,
            ProjectError::Io { .. }
                | ProjectError::NotAProject(_)
                | ProjectError::Malformed { .. }
                | ProjectError::MissingObject { .. }
                | ProjectError::HashMismatch { .. }
                | ProjectError::Invalid(_)
        )
    }
}

/// Maps an `io::Error` to [`ProjectError::Io`] for `path`.
pub(crate) fn io_at(path: &Path) -> impl FnOnce(io::Error) -> ProjectError + '_ {
    move |source| ProjectError::Io {
        path: path.to_path_buf(),
        source,
    }
}
