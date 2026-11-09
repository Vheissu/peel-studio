//! Design tokens.
//!
//! Reference: a pattern-and-playlist DAW workspace. Neutral graphite
//! chrome, square continuous editor canvases, warm amber for actions and
//! the playhead, lime for signal, and channel colors only on musical
//! content. Panes meet at thin dark seams instead of floating as cards;
//! hierarchy comes from value steps and 1 px separators. Notes, clips,
//! waveforms and the playhead stay the brightest things on screen.

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
            bg: Color::hex(0x0F1011),
            panel: Color::hex(0x1C1E21),
            panel_alt: Color::hex(0x23262A),
            raised: Color::hex(0x2A2D32),
            control: Color::hex(0x31353B),
            control_hover: Color::hex(0x3B4047),
            control_active: Color::hex(0x464C54),
            line: Color::hex(0x2B2E33),
            line_strong: Color::hex(0x3A3E44),
            well: Color::hex(0x151719),

            text: Color::hex(0xEEEFF0),
            text_dim: Color::hex(0xADB1B7),
            text_faint: Color::hex(0x8C9199),
            text_on_accent: Color::hex(0x1E1406),

            accent: Color::hex(0xF0A443),
            focus: Color::hex(0x6EC4E6),
            record: Color::hex(0xEE5F5B),
            play: Color::hex(0x9FD65B),
            warn: Color::hex(0xE8C45C),
            shadow: Color::rgba(0.0, 0.0, 0.0, 0.5),

            grid_bar: Color::rgba(1.0, 1.0, 1.0, 0.12),
            grid_beat: Color::rgba(1.0, 1.0, 1.0, 0.06),
            grid_step: Color::rgba(1.0, 1.0, 1.0, 0.028),
            key_white: Color::hex(0xD6D8DB),
            key_black: Color::hex(0x26282C),

            radius_s: 2.0,
            radius_m: 3.0,
            radius_l: 3.0,
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
