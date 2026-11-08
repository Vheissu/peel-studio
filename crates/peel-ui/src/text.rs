//! Shaped-text cache on top of cosmic-text.
//!
//! Shaping is expensive, so every distinct (string, style, wrap width) is
//! shaped once and reused across frames until it goes unused for a while.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use glyphon::cosmic_text::{FeatureTag, FontFeatures};
use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight, Wrap};

use crate::geom::{Vec2, vec2};

const FONT_REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/barlow/BarlowSemiCondensed-Regular.ttf");
const FONT_MEDIUM: &[u8] =
    include_bytes!("../../../assets/fonts/barlow/BarlowSemiCondensed-Medium.ttf");
const FONT_SEMIBOLD: &[u8] =
    include_bytes!("../../../assets/fonts/barlow/BarlowSemiCondensed-SemiBold.ttf");
const FONT_BOLD: &[u8] =
    include_bytes!("../../../assets/fonts/barlow/BarlowSemiCondensed-Bold.ttf");
const FONT_ICONS: &[u8] = include_bytes!("../../../assets/fonts/phosphor/Phosphor.ttf");
const FONT_ICONS_FILL: &[u8] = include_bytes!("../../../assets/fonts/phosphor/Phosphor-Fill.ttf");

const UI_FAMILY: &str = "Barlow Semi Condensed";
const ICON_FAMILY: &str = "Phosphor";
const ICON_FILL_FAMILY: &str = "Phosphor-Fill";

/// Frames an unused shaped string survives in the cache.
const EVICT_AFTER: u64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontWeight {
    Regular,
    Medium,
    SemiBold,
    Bold,
}

impl FontWeight {
    fn to_weight(self) -> Weight {
        match self {
            FontWeight::Regular => Weight(400),
            FontWeight::Medium => Weight(500),
            FontWeight::SemiBold => Weight(600),
            FontWeight::Bold => Weight(700),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontKind {
    Ui,
    Icon,
    IconFill,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub weight: FontWeight,
    pub font: FontKind,
    /// Tabular (fixed-width) digits for positions, tempo and levels.
    pub tabular: bool,
}

impl TextStyle {
    pub const fn ui(size: f32) -> TextStyle {
        TextStyle {
            size,
            weight: FontWeight::Medium,
            font: FontKind::Ui,
            tabular: false,
        }
    }

    pub const fn icon(size: f32) -> TextStyle {
        TextStyle {
            size,
            weight: FontWeight::Regular,
            font: FontKind::Icon,
            tabular: false,
        }
    }

    pub const fn icon_fill(size: f32) -> TextStyle {
        TextStyle {
            size,
            weight: FontWeight::Regular,
            font: FontKind::IconFill,
            tabular: false,
        }
    }

    pub const fn weight(self, weight: FontWeight) -> TextStyle {
        TextStyle { weight, ..self }
    }

    pub const fn tabular(self) -> TextStyle {
        TextStyle {
            tabular: true,
            ..self
        }
    }

    pub const fn size(self, size: f32) -> TextStyle {
        TextStyle { size, ..self }
    }
}

/// Handle to a shaped string in the cache.
pub type TextKey = u64;

pub(crate) struct Entry {
    pub(crate) buffer: Buffer,
    size: Vec2,
    last_used: u64,
}

pub struct TextSystem {
    pub font_system: FontSystem,
    cache: HashMap<TextKey, Entry>,
    frame: u64,
}

impl TextSystem {
    pub fn new() -> TextSystem {
        let mut font_system = FontSystem::new();
        let db = font_system.db_mut();
        for data in [
            FONT_REGULAR,
            FONT_MEDIUM,
            FONT_SEMIBOLD,
            FONT_BOLD,
            FONT_ICONS,
            FONT_ICONS_FILL,
        ] {
            db.load_font_data(data.to_vec());
        }
        TextSystem {
            font_system,
            cache: HashMap::new(),
            frame: 0,
        }
    }

    fn key(text: &str, style: &TextStyle, max_width: Option<f32>) -> TextKey {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        style.size.to_bits().hash(&mut h);
        style.weight.hash(&mut h);
        style.font.hash(&mut h);
        style.tabular.hash(&mut h);
        max_width.map(f32::to_bits).hash(&mut h);
        h.finish()
    }

    /// Shapes (or reuses) a string and returns its key and size.
    pub fn layout(
        &mut self,
        text: &str,
        style: TextStyle,
        max_width: Option<f32>,
    ) -> (TextKey, Vec2) {
        let key = Self::key(text, &style, max_width);
        let frame = self.frame;
        if let Some(e) = self.cache.get_mut(&key) {
            e.last_used = frame;
            return (key, e.size);
        }
        let line_height = (style.size * 1.25).ceil();
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(style.size, line_height));
        buffer.set_wrap(if max_width.is_some() {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        });
        buffer.set_size(max_width, None);
        let family = match style.font {
            FontKind::Ui => Family::Name(UI_FAMILY),
            FontKind::Icon => Family::Name(ICON_FAMILY),
            FontKind::IconFill => Family::Name(ICON_FILL_FAMILY),
        };
        let mut attrs = Attrs::new().family(family).weight(style.weight.to_weight());
        if style.tabular {
            let mut features = FontFeatures::new();
            features.enable(FeatureTag::new(b"tnum"));
            attrs = attrs.font_features(features);
        }
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        let mut w: f32 = 0.0;
        let mut lines = 0;
        for run in buffer.layout_runs() {
            w = w.max(run.line_w);
            lines += 1;
        }
        let size = vec2(w.ceil(), line_height * lines.max(1) as f32);
        self.cache.insert(
            key,
            Entry {
                buffer,
                size,
                last_used: frame,
            },
        );
        (key, size)
    }

    pub fn measure(&mut self, text: &str, style: TextStyle) -> Vec2 {
        self.layout(text, style, None).1
    }

    pub(crate) fn parts(&mut self) -> (&mut FontSystem, &HashMap<TextKey, Entry>) {
        (&mut self.font_system, &self.cache)
    }

    pub fn buffer(&self, key: TextKey) -> Option<&Buffer> {
        self.cache.get(&key).map(|e| &e.buffer)
    }

    /// Drops strings not used recently. Call once per frame.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        if self.frame.is_multiple_of(60) {
            let frame = self.frame;
            self.cache.retain(|_, e| frame - e.last_used < EVICT_AFTER);
        }
    }

    pub fn cached(&self) -> usize {
        self.cache.len()
    }
}

impl Default for TextSystem {
    fn default() -> Self {
        TextSystem::new()
    }
}
