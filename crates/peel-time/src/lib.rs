//! Exact musical time for Peel Studio.
//!
//! Musical positions are stored as reduced rational numbers of quarter notes
//! ("beats"). The numerator is a signed 128-bit integer and the denominator is
//! positive and bounded, so ordinary divisions, tuplets and imported MIDI ticks
//! stay exact in the document. Conversion to samples happens only when the
//! engine compiles a render plan, using absolute positions so repeated
//! patterns cannot drift.

mod rational;
mod tempo;

pub use rational::{MAX_DENOMINATOR, Time, TimeError};
pub use tempo::{TempoMap, TempoSegment, TimeSignature};

/// Ticks per quarter note used when a fixed-resolution integer view is needed
/// (MIDI export, UI snapping helpers). The document itself never stores ticks.
pub const DISPLAY_PPQ: i64 = 960;
