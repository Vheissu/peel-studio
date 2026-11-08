use std::fmt;

/// An sRGB color, serialized as `#rrggbb`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b }
    }

    pub const fn hex(v: u32) -> Color {
        Color {
            r: (v >> 16) as u8,
            g: (v >> 8) as u8,
            b: v as u8,
        }
    }

    pub fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }
}

/// Default palette for channels, patterns and tracks.
///
/// Derived from the colored knob caps and patch cables of an analog
/// console: saturated enough to tell lanes apart at a glance, muted enough
/// that notes and waveforms stay the brightest thing on screen.
pub const PALETTE: [Color; 12] = [
    Color::hex(0xE8735A), // tomato cap
    Color::hex(0xF2B66D), // amber (VU backlight)
    Color::hex(0xD9C35C), // mustard
    Color::hex(0x9CC46B), // olive green
    Color::hex(0x5FBF9A), // jade
    Color::hex(0x56B3C2), // patch-cable teal
    Color::hex(0x6E9FE0), // console blue
    Color::hex(0xA58BE0), // lavender
    Color::hex(0xD982B5), // rose
    Color::hex(0xE58C8C), // salmon
    Color::hex(0xB8A48C), // tape beige
    Color::hex(0x8E9AAB), // steel
];

impl fmt::Debug for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl serde::Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&format_args!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b))
    }
}

impl<'de> serde::Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        let s = String::deserialize(d)?;
        let h = s.trim_start_matches('#');
        if h.len() != 6 {
            return Err(serde::de::Error::custom("color must be #rrggbb"));
        }
        let v = u32::from_str_radix(h, 16).map_err(serde::de::Error::custom)?;
        Ok(Color::hex(v))
    }
}
