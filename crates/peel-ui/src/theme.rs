//! Design tokens.
//!
//! Reference: an analog mixing console at night. Graphite panels, colored
//! channel caps, amber VU backlight for the primary action, a coral tally
//! light for recording, and crisp white legends. Hierarchy comes from value
//! steps between surfaces rather than borders and shadows; notes,
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
            bg: Color::hex(0x14171D),
            panel: Color::hex(0x1C2129),
            panel_alt: Color::hex(0x20262F),
            raised: Color::hex(0x252D38),
            control: Color::hex(0x2B3441),
            control_hover: Color::hex(0x333E4D),
            control_active: Color::hex(0x3B4859),
            line: Color::hex(0x262D37),
            line_strong: Color::hex(0x323B48),
            well: Color::hex(0x171B22),

            text: Color::hex(0xF1F4F8),
            text_dim: Color::hex(0xACB7C7),
            text_faint: Color::hex(0x7D8899),
            text_on_accent: Color::hex(0x221808),

            accent: Color::hex(0xF2B66D),
            focus: Color::hex(0x60D6E8),
            record: Color::hex(0xF07880),
            play: Color::hex(0x86D49E),
            warn: Color::hex(0xE8C45C),
            shadow: Color::rgba(0.0, 0.0, 0.0, 0.5),

            grid_bar: Color::rgba(1.0, 1.0, 1.0, 0.11),
            grid_beat: Color::rgba(1.0, 1.0, 1.0, 0.055),
            grid_step: Color::rgba(1.0, 1.0, 1.0, 0.025),
            key_white: Color::hex(0xD9DEE6),
            key_black: Color::hex(0x2A313B),

            radius_s: 4.0,
            radius_m: 6.0,
            radius_l: 8.0,
            control_h: 28.0,
            control_h_compact: 24.0,
            rail_w: 40.0,
            header_h: 38.0,

            caption: TextStyle::ui(11.5),
            body: TextStyle::ui(13.0),
            label: TextStyle::ui(13.0).weight(FontWeight::SemiBold),
            heading: TextStyle::ui(15.0).weight(FontWeight::SemiBold),
            readout: TextStyle::ui(20.0).weight(FontWeight::SemiBold).tabular(),
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
