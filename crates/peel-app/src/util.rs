//! Small helpers shared by the editors.

use peel_document::{Color as DocColor, Time, TimeSignature};
use peel_ui::Color;

/// Names for the twelve palette colors, in palette order.
pub const COLOR_NAMES: [&str; 12] = [
    "Tomato", "Amber", "Mustard", "Olive", "Jade", "Teal", "Blue", "Lavender", "Rose", "Salmon",
    "Beige", "Steel",
];

/// Document color to UI color.
pub fn col(c: DocColor) -> Color {
    Color::from_rgb8(c.r, c.g, c.b)
}

/// Text color that reads on top of a fill.
pub fn ink_on(fill: Color) -> Color {
    if fill.luminance() > 0.55 {
        Color::hex(0x161A20)
    } else {
        Color::hex(0xF7F9FB)
    }
}

/// "bar.beat.sixteenth" (1-based) for a position in quarter notes.
pub fn format_position(beat: f64, sig: TimeSignature) -> String {
    let bar_len = sig.bar_length().as_f64();
    let beat_len = sig.beat_length().as_f64();
    let beat = beat.max(0.0);
    let bar = (beat / bar_len).floor();
    let in_bar = beat - bar * bar_len;
    let b = (in_bar / beat_len).floor();
    let sub = ((in_bar - b * beat_len) / (beat_len / 4.0)).floor();
    format!("{}.{}.{}", bar as i64 + 1, b as i64 + 1, sub as i64 + 1)
}

/// Musical length label for a grid or note length.
pub fn format_length(t: Time) -> String {
    let v = t.as_f64();
    let table: [(f64, &str); 12] = [
        (4.0, "1 bar"),
        (2.0, "1/2"),
        (1.0, "1/4"),
        (0.5, "1/8"),
        (1.0 / 3.0, "1/8T"),
        (0.25, "1/16"),
        (1.0 / 6.0, "1/16T"),
        (0.125, "1/32"),
        (0.75, "1/8D"),
        (1.5, "1/4D"),
        (8.0, "2 bars"),
        (16.0, "4 bars"),
    ];
    for (b, l) in table {
        if (v - b).abs() < 1e-9 {
            return l.to_string();
        }
    }
    format!("{v:.2}")
}

/// Note name, using C4 = MIDI 60.
pub fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[(key % 12) as usize], key as i32 / 12 - 1)
}

pub fn is_black_key(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

pub use peel_document::{db_to_fader, fader_to_db};

pub fn format_db(db: f32) -> String {
    if db <= -95.0 {
        "-inf".into()
    } else {
        format!("{db:+.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fader_law_round_trips() {
        for db in [-48.0f32, -12.0, -6.0, 0.0, 3.0, 6.0] {
            assert!((fader_to_db(db_to_fader(db)) - db).abs() < 0.05, "{db}");
        }
    }

    #[test]
    fn positions() {
        let sig = TimeSignature::default();
        assert_eq!(format_position(0.0, sig), "1.1.1");
        assert_eq!(format_position(4.25, sig), "2.1.2");
        assert_eq!(format_position(7.5, sig), "2.4.3");
        assert_eq!(note_name(60), "C4");
    }
}
