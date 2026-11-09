use std::path::{Path, PathBuf};

/// Why a media operation failed. Every variant that concerns a file carries
/// its path so the message can be shown to the user as it is.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The container or codec is not one of the supported formats.
    #[error("{}: unsupported or unrecognized audio format ({reason})", path.display())]
    Unsupported { path: PathBuf, reason: String },
    /// The file is a valid container with no decodable audio track.
    #[error("{}: no audio track", path.display())]
    NoAudio { path: PathBuf },
    /// The audio data is damaged beyond what the decoder can skip.
    #[error("{}: could not decode audio ({reason})", path.display())]
    Decode { path: PathBuf, reason: String },
    /// An audio file could not be written.
    #[error("{}: could not write audio ({reason})", path.display())]
    Encode { path: PathBuf, reason: String },
    #[error("Export cancelled")]
    Cancelled,
    /// The arguments make no sense (empty channel list, unequal lengths,
    /// unsupported bit depth, zero sample rate).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl MediaError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> MediaError {
        MediaError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
