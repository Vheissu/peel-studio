//! Design tokens.
//!
//! Reference: the front panel of a Roland TR-808 and the drum machines
//! that followed it. A warm black panel, cream legends, one orange stripe,
//! and step keys in red, orange, yellow and cream, four to a beat. Peel
//! Studio borrows that palette and its sense of hardware (controls with
//! weight, displays that sit in the panel), not its artwork.
//!
//! Panes meet at thin dark seams instead of floating as cards; hierarchy
//! comes from value steps and 1 px separators. Channel colors appear only
//! on musical content, so notes, clips, waveforms and the playhead stay
//! the brightest things on screen.

use crate::geom::Color;
use crate::text::{FontWeight, TextStyle};

#[derive(Clone, Debug)]
pub struct Theme {
    // Surfaces, darkest to lightest.
    pub bg: Color,
    pub panel: Color,
    pub panel_alt: Color,
    pub raised: Color,
    pub control: Color,
    pub control_hover: Color,
    pub control_active: Color,
    /// Separators where a value step is not enough.
    pub line: Color,
    pub line_strong: Color,
    /// Recessed tray behind a group of related quiet controls.
    pub well: Color,

    // Text.
    pub text: Color,
    pub text_dim: Color,
    pub text_faint: Color,
    pub text_on_accent: Color,

    // Signals.
    pub accent: Color,
    pub focus: Color,
    pub record: Color,
    pub play: Color,
    pub warn: Color,
    pub shadow: Color,

    // Editor grids.
    pub grid_bar: Color,
    pub grid_beat: Color,
    pub grid_step: Color,
    pub key_white: Color,
    pub key_black: Color,
    /// Step keys by beat of the bar: red, orange, yellow, cream.
    pub step_keys: [Color; 4],
    /// Lit readouts inside a display window (position, tempo).
    pub display: Color,

    // Metrics.
    pub radius_s: f32,
    pub radius_m: f32,
    pub radius_l: f32,
    pub control_h: f32,
    pub control_h_compact: f32,
    /// Width of the navigation rails beside the side panels.
    pub rail_w: f32,
    /// Height of pane headers (browser, inspector, playlist, dock).
    pub header_h: f32,

    // Type.
    pub caption: TextStyle,
    pub body: TextStyle,
    pub label: TextStyle,
    pub heading: TextStyle,
    pub readout: TextStyle,
    pub readout_small: TextStyle,
}

impl Theme {
    pub fn dark() -> Theme {
        Theme {
            bg: Color::hex(0x0C0B0A),
            panel: Color::hex(0x1A1917),
            panel_alt: Color::hex(0x211F1D),
            raised: Color::hex(0x2B2926),
            control: Color::hex(0x35322E),
            control_hover: Color::hex(0x423E39),
            control_active: Color::hex(0x504B45),
            line: Color::hex(0x2D2B28),
            line_strong: Color::hex(0x403C37),
            well: Color::hex(0x121110),

            text: Color::hex(0xF3EEE4),
            text_dim: Color::hex(0xB9B2A6),
            text_faint: Color::hex(0x999286),
            text_on_accent: Color::hex(0x1F1305),

            accent: Color::hex(0xF7992B),
            focus: Color::hex(0x7FC8EA),
            record: Color::hex(0xF2553F),
            play: Color::hex(0x9FD65B),
            warn: Color::hex(0xEFC53F),
            shadow: Color::rgba(0.0, 0.0, 0.0, 0.55),

            grid_bar: Color::rgba(1.0, 0.96, 0.9, 0.13),
            grid_beat: Color::rgba(1.0, 0.96, 0.9, 0.065),
            grid_step: Color::rgba(1.0, 0.96, 0.9, 0.03),
            key_white: Color::hex(0xE6DFD0),
            key_black: Color::hex(0x252320),
            step_keys: [
                Color::hex(0xE8503C),
                Color::hex(0xF28A2E),
                Color::hex(0xF0C63E),
                Color::hex(0xEFE7D6),
            ],
            display: Color::hex(0xFFB454),

            radius_s: 2.0,
            radius_m: 4.0,
            radius_l: 6.0,
            control_h: 24.0,
            control_h_compact: 20.0,
            rail_w: 36.0,
            header_h: 32.0,

            caption: TextStyle::ui(11.5),
            body: TextStyle::ui(13.0),
            label: TextStyle::ui(13.0).weight(FontWeight::SemiBold),
            heading: TextStyle::ui(14.0).weight(FontWeight::SemiBold),
            readout: TextStyle::ui(19.0).weight(FontWeight::SemiBold).tabular(),
            readout_small: TextStyle::ui(12.0).weight(FontWeight::Medium).tabular(),
        }
    }

    /// Meter color for a linear peak value.
    pub fn meter_color(&self, peak: f32) -> Color {
        if peak >= 1.0 {
            self.record
        } else if peak >= 0.708 {
            // above -3 dB
            self.accent
        } else {
            self.play
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::dark()
    }
}
