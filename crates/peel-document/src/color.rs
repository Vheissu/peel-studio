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
/// It starts on the red, orange and yellow of a TR-808's step keys and
/// carries on round the wheel at the same weight: saturated enough to tell
/// lanes apart at a glance, and all light enough to take dark text.
pub const PALETTE: [Color; 12] = [
    Color::hex(0xEC5E48), // 808 red
    Color::hex(0xF5943A), // 808 orange
    Color::hex(0xEFC644), // 808 yellow
    Color::hex(0xA6CE55), // lime
    Color::hex(0x55C48E), // jade
    Color::hex(0x44B9C9), // teal
    Color::hex(0x5E9FEE), // blue
    Color::hex(0x9E88EC), // violet
    Color::hex(0xE07CBB), // pink
    Color::hex(0xF08878), // coral
    Color::hex(0xC9B291), // sand
    Color::hex(0x95A2B0), // slate
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
