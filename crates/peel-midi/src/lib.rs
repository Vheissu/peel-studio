//! Standard MIDI File import and export for Peel Studio.
//!
//! * [`import_smf`] turns a file (SMF type 0, 1 or 2, or RIFF `.rmi`) into
//!   channels, patterns, tempo and meter plus an [`ImportReport`] that lists
//!   everything that was not imported. [`MidiImport::merge_into`] adds the
//!   result to a project, validated and all-or-nothing.
//! * [`export_pattern`] and [`export_arrangement`] write type 0 or type 1
//!   files with an [`ExportReport`] of every approximation.
//!
//! Pitch bend, channel and polyphonic aftertouch, and CC74 become saved
//! note expression on import. MPE zone configuration and bend sensitivity
//! determine how member and manager controls combine. Expressive exports
//! use MPE to keep overlapping notes independent.
//!
//! The crate knows nothing about concrete devices: device type ids come from
//! [`ImportOptions`], and percussion detection on export from
//! [`ExportOptions::is_drum`].

mod error;
mod export;
mod gm;
mod import;

pub use error::{MidiError, Result};
pub use export::{
    ExportFormat, ExportOptions, ExportReport, MidiExport, ProbabilityMode, export_arrangement,
    export_pattern,
};
pub use gm::{default_drum_device, default_melodic_device, drum_name, program_name};
pub use import::{
    ChannelReport, ChannelSource, ImportOptions, ImportReport, ImportedChannel, KeySignatureEvent,
    MergeOptions, MergeReport, MetaEntry, MidiImport, PatternLayout, ProgramUse, Quantize, Repairs,
    SmfFormat, SustainPolicy, TrackReport, UnsupportedEvents, import_smf,
};
