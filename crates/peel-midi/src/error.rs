use peel_document::Id;

#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    #[error("not a valid Standard MIDI File: {0}")]
    Parse(String),
    #[error("malformed MIDI data in track {track} at byte offset {offset}: {message}")]
    Malformed {
        track: usize,
        offset: usize,
        message: String,
    },
    #[error("unsupported timing division: {0}")]
    UnsupportedTiming(String),
    #[error("MIDI file exceeds an import limit: {0}")]
    TooLarge(String),
    #[error("pattern {0} is not in the project")]
    UnknownPattern(Id),
    #[error("mixer track {0} is not in the project")]
    UnknownMixerTrack(Id),
    #[error("the import would make the project invalid: {}", .0.join("; "))]
    InvalidMerge(Vec<String>),
    #[error("cannot export MIDI: {0}")]
    Export(String),
}

pub type Result<T, E = MidiError> = std::result::Result<T, E>;
