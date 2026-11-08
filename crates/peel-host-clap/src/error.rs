use std::path::PathBuf;

/// Everything that can go wrong while scanning, loading or driving a CLAP
/// plugin.
#[derive(Debug, thiserror::Error)]
pub enum ClapError {
    #[error("no CLAP binary found at {}", .0.display())]
    NoBinary(PathBuf),
    #[error("cannot read {}: {reason}", path.display())]
    Io { path: PathBuf, reason: String },
    #[error("cannot load {}: {reason}", path.display())]
    Load { path: PathBuf, reason: String },
    #[error("{} does not export a clap_entry symbol", .0.display())]
    NoEntry(PathBuf),
    #[error("{} needs CLAP {found} but this host speaks {supported}", path.display())]
    IncompatibleVersion {
        path: PathBuf,
        found: String,
        supported: String,
    },
    #[error("clap_entry.init failed for {}", .0.display())]
    InitFailed(PathBuf),
    #[error("{} has no plugin factory", .0.display())]
    NoFactory(PathBuf),
    #[error("{} does not contain the plugin {id}", path.display())]
    PluginNotFound { path: PathBuf, id: String },
    #[error("plugin {id} refused to be created")]
    CreateFailed { id: String },
    #[error("plugin {id} failed to initialize")]
    PluginInitFailed { id: String },
    #[error("plugin {id} is missing the required entry point {entry}")]
    MissingEntryPoint { id: String, entry: &'static str },
    #[error("plugin {id} failed to activate at {sample_rate} Hz with blocks up to {max_block}")]
    ActivateFailed {
        id: String,
        sample_rate: f32,
        max_block: usize,
    },
    #[error("unsupported plugin: {0}")]
    Unsupported(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("plugin state: {0}")]
    State(String),
    #[error("the plugin scanner crashed on {} ({status})", path.display())]
    ScannerCrashed { path: PathBuf, status: String },
    #[error("the plugin scanner timed out on {}", .0.display())]
    ScannerTimedOut(PathBuf),
    #[error("the plugin scanner failed: {0}")]
    Scanner(String),
    #[error("internal panic while handling {what}: {message}")]
    Panic { what: String, message: String },
}
