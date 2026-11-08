//! Geometry and color primitives in logical pixels.

use std::ops::{Add, AddAssign, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

pub const fn vec2(x: f32, y: f32) -> Vec2 {
    Vec2 { x, y }
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        vec2(self.x + o.x, self.y + o.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, o: Vec2) {
        self.x += o.x;
        self.y += o.y;
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        vec2(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f32) -> Vec2 {
        vec2(self.x * k, self.y * k)
    }
}

/// Axis-aligned rectangle (`x`, `y` = top-left).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub const fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

impl Rect {
    pub const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };
    /// A rect covering everything (used as "no clip").
    pub const EVERYTHING: Rect = Rect {
        x: -1e7,
        y: -1e7,
        w: 2e7,
        h: 2e7,
    };

    pub fn from_min_max(min: Vec2, max: Vec2) -> Rect {
        Rect {
            x: min.x,
            y: min.y,
            w: (max.x - min.x).max(0.0),
            h: (max.y - min.y).max(0.0),
        }
    }

    pub fn from_center(c: Vec2, w: f32, h: f32) -> Rect {
        Rect {
            x: c.x - w * 0.5,
            y: c.y - h * 0.5,
            w,
            h,
        }
    }

    pub fn min(&self) -> Vec2 {
        vec2(self.x, self.y)
    }

    pub fn max(&self) -> Vec2 {
        vec2(self.right(), self.bottom())
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Vec2 {
        vec2(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn size(&self) -> Vec2 {
        vec2(self.w, self.h)
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        Rect::from_min_max(vec2(x0, y0), vec2(x1.max(x0), y1.max(y0)))
    }

    pub fn union(&self, o: &Rect) -> Rect {
        Rect::from_min_max(
            vec2(self.x.min(o.x), self.y.min(o.y)),
            vec2(self.right().max(o.right()), self.bottom().max(o.bottom())),
        )
    }

    /// Shrinks on all sides.
    pub fn shrink(&self, d: f32) -> Rect {
        self.shrink2(d, d)
    }

    pub fn shrink2(&self, dx: f32, dy: f32) -> Rect {
        Rect {
            x: self.x + dx,
            y: self.y + dy,
            w: (self.w - 2.0 * dx).max(0.0),
            h: (self.h - 2.0 * dy).max(0.0),
        }
    }

    pub fn expand(&self, d: f32) -> Rect {
        Rect {
            x: self.x - d,
            y: self.y - d,
            w: self.w + 2.0 * d,
            h: self.h + 2.0 * d,
        }
    }

    pub fn translate(&self, d: Vec2) -> Rect {
        Rect {
            x: self.x + d.x,
            y: self.y + d.y,
            ..*self
        }
    }

    pub fn with_w(&self, w: f32) -> Rect {
        Rect { w, ..*self }
    }

    pub fn with_h(&self, h: f32) -> Rect {
        Rect { h, ..*self }
    }

    // ---- rect-cut layout: each call removes a slice from `self` ----

    pub fn cut_left(&mut self, w: f32) -> Rect {
        let w = w.min(self.w).max(0.0);
        let r = Rect { w, ..*self };
        self.x += w;
        self.w -= w;
        r
    }

    pub fn cut_right(&mut self, w: f32) -> Rect {
        let w = w.min(self.w).max(0.0);
        self.w -= w;
        Rect {
            x: self.x + self.w,
            w,
            ..*self
        }
    }

    pub fn cut_top(&mut self, h: f32) -> Rect {
        let h = h.min(self.h).max(0.0);
        let r = Rect { h, ..*self };
        self.y += h;
        self.h -= h;
        r
    }

    pub fn cut_bottom(&mut self, h: f32) -> Rect {
        let h = h.min(self.h).max(0.0);
        self.h -= h;
        Rect {
            y: self.y + self.h,
            h,
            ..*self
        }
    }

    /// Splits horizontally into `n` equal columns separated by `gap`.
    pub fn columns(&self, n: usize, gap: f32) -> Vec<Rect> {
        let n = n.max(1);
        let w = (self.w - gap * (n as f32 - 1.0)) / n as f32;
        (0..n)
            .map(|i| Rect {
                x: self.x + i as f32 * (w + gap),
                w,
                ..*self
            })
            .collect()
    }

    /// Splits vertically into `n` equal rows separated by `gap`.
    pub fn rows(&self, n: usize, gap: f32) -> Vec<Rect> {
        let n = n.max(1);
        let h = (self.h - gap * (n as f32 - 1.0)) / n as f32;
        (0..n)
            .map(|i| Rect {
                y: self.y + i as f32 * (h + gap),
                h,
                ..*self
            })
            .collect()
    }

    /// Rounds edges to the physical pixel grid.
    pub fn snap(&self, scale: f32) -> Rect {
        let x0 = (self.x * scale).round() / scale;
        let y0 = (self.y * scale).round() / scale;
        let x1 = (self.right() * scale).round() / scale;
        let y1 = (self.bottom() * scale).round() / scale;
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }
}

/// A color in sRGB space with straight alpha, components 0..=1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    pub const WHITE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };
    pub const BLACK: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };

    pub const fn hex(v: u32) -> Color {
        Color {
            r: ((v >> 16) & 0xff) as f32 / 255.0,
            g: ((v >> 8) & 0xff) as f32 / 255.0,
            b: (v & 0xff) as f32 / 255.0,
            a: 1.0,
        }
    }

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Color {
        Color {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    pub fn alpha(self, a: f32) -> Color {
        Color {
            a: self.a * a,
            ..self
        }
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// Linear interpolation in sRGB space.
    pub fn mix(self, o: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        Color {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }

    pub fn lighten(self, t: f32) -> Color {
        self.mix(Color::WHITE.with_alpha(self.a), t)
    }

    pub fn darken(self, t: f32) -> Color {
        self.mix(Color::BLACK.with_alpha(self.a), t)
    }

    /// Relative luminance (approximate, for choosing text over a fill).
    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn to_rgba8(self) -> [u8; 4] {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        [c(self.r), c(self.g), c(self.b), c(self.a)]
    }
}
