//! Per-frame draw lists.
//!
//! A [`Scene`] holds ordered layers. Each layer draws its shapes first and its
//! text second, so anything that must cover text (menus, tooltips, drag
//! previews) goes on a later layer.

use bytemuck::{Pod, Zeroable};

use crate::geom::{Color, Rect, Vec2};
use crate::text::TextKey;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ShapeInstance {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub color2: [f32; 4],
    pub radii: [f32; 4],
    /// kind, stroke width, blur, flags/direction
    pub params: [f32; 4],
    pub extra: [f32; 4],
    /// x0, y0, x1, y1
    pub clip: [f32; 4],
}

const KIND_RECT: f32 = 0.0;
const KIND_ARC: f32 = 1.0;
const KIND_LINE: f32 = 2.0;
const KIND_TRIANGLE: f32 = 3.0;

/// Direction a triangle points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Right,
    Down,
    Left,
    Up,
}

/// A positioned piece of cached text.
#[derive(Clone, Debug)]
pub struct TextItem {
    pub key: TextKey,
    pub pos: Vec2,
    pub color: Color,
    pub clip: Rect,
}

#[derive(Default)]
pub struct Layer {
    pub shapes: Vec<ShapeInstance>,
    pub texts: Vec<TextItem>,
}

/// Standard layer slots. Additional layers can be pushed on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerId(pub u16);

impl LayerId {
    pub const BASE: LayerId = LayerId(0);
    /// Content drawn above base text (playheads, selection marquees).
    pub const ABOVE: LayerId = LayerId(1);
    pub const POPUP: LayerId = LayerId(2);
    pub const DRAG: LayerId = LayerId(3);
    pub const TOOLTIP: LayerId = LayerId(4);
}

pub struct Scene {
    pub layers: Vec<Layer>,
    pub(crate) layer: usize,
    clip_stack: Vec<Rect>,
    pub(crate) scale: f32,
}

impl Scene {
    pub fn new() -> Scene {
        Scene {
            layers: (0..5).map(|_| Layer::default()).collect(),
            layer: 0,
            clip_stack: vec![Rect::EVERYTHING],
            scale: 1.0,
        }
    }

    pub(crate) fn begin(&mut self, scale: f32) {
        for l in &mut self.layers {
            l.shapes.clear();
            l.texts.clear();
        }
        self.layer = 0;
        self.clip_stack.clear();
        self.clip_stack.push(Rect::EVERYTHING);
        self.scale = scale;
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn layer_id(&self) -> LayerId {
        LayerId(self.layer as u16)
    }

    pub fn set_layer(&mut self, id: LayerId) -> LayerId {
        let prev = self.layer_id();
        let idx = id.0 as usize;
        while self.layers.len() <= idx {
            self.layers.push(Layer::default());
        }
        self.layer = idx;
        prev
    }

    pub fn clip(&self) -> Rect {
        *self.clip_stack.last().unwrap_or(&Rect::EVERYTHING)
    }

    /// Intersects the clip with `r` until the matching [`Scene::pop_clip`].
    pub fn push_clip(&mut self, r: Rect) {
        let c = self.clip().intersect(&r.snap(self.scale));
        self.clip_stack.push(c);
    }

    /// Replaces the clip (ignoring the parent) — for popups.
    pub fn push_clip_absolute(&mut self, r: Rect) {
        self.clip_stack.push(r);
    }

    pub fn pop_clip(&mut self) {
        if self.clip_stack.len() > 1 {
            self.clip_stack.pop();
        }
    }

    fn clip_arr(&self) -> [f32; 4] {
        let c = self.clip();
        [c.x, c.y, c.right(), c.bottom()]
    }

    fn push(&mut self, inst: ShapeInstance) {
        self.layers[self.layer].shapes.push(inst);
    }

    fn visible(&self, r: &Rect) -> bool {
        r.intersects(&self.clip())
    }

    // ---- shapes ----

    /// Filled rectangle with uniform corner radius.
    pub fn rect(&mut self, r: Rect, radius: f32, color: Color) {
        self.rect_ex(r, [radius; 4], color, 0.0, Color::TRANSPARENT);
    }

    /// Filled rectangle with a border drawn inside its edge.
    pub fn rect_stroke(
        &mut self,
        r: Rect,
        radius: f32,
        fill: Color,
        stroke: f32,
        stroke_color: Color,
    ) {
        self.rect_ex(r, [radius; 4], fill, stroke, stroke_color);
    }

    /// Outline only.
    pub fn outline(&mut self, r: Rect, radius: f32, stroke: f32, color: Color) {
        self.rect_ex(r, [radius; 4], Color::TRANSPARENT, stroke, color);
    }

    /// Full control: per-corner radii (tl, tr, br, bl).
    pub fn rect_ex(
        &mut self,
        r: Rect,
        radii: [f32; 4],
        fill: Color,
        stroke: f32,
        stroke_color: Color,
    ) {
        let r = r.snap(self.scale);
        if r.is_empty()
            || !self.visible(&r)
            || (fill.a <= 0.0 && (stroke <= 0.0 || stroke_color.a <= 0.0))
        {
            return;
        }
        let clip = self.clip_arr();
        self.push(ShapeInstance {
            rect: [r.x, r.y, r.w, r.h],
            color: fill.to_array(),
            color2: stroke_color.to_array(),
            radii,
            params: [KIND_RECT, stroke, 0.0, 0.0],
            extra: [0.0; 4],
            clip,
        });
    }

    /// Vertical gradient fill (top color to bottom color).
    pub fn rect_gradient_v(&mut self, r: Rect, radius: f32, top: Color, bottom: Color) {
        self.gradient(r, radius, top, bottom, 1.0);
    }

    /// Horizontal gradient fill (left color to right color).
    pub fn rect_gradient_h(&mut self, r: Rect, radius: f32, left: Color, right: Color) {
        self.gradient(r, radius, left, right, 2.0);
    }

    fn gradient(&mut self, r: Rect, radius: f32, a: Color, b: Color, flag: f32) {
        let r = r.snap(self.scale);
        if r.is_empty() || !self.visible(&r) {
            return;
        }
        let clip = self.clip_arr();
        self.push(ShapeInstance {
            rect: [r.x, r.y, r.w, r.h],
            color: a.to_array(),
            color2: [0.0; 4],
            radii: [radius; 4],
            params: [KIND_RECT, 0.0, 0.0, flag],
            extra: b.to_array(),
            clip,
        });
    }

    /// Soft shadow under a rounded rect (for floating layers only).
    pub fn shadow(&mut self, r: Rect, radius: f32, blur: f32, color: Color) {
        let clip = self.clip_arr();
        self.push(ShapeInstance {
            rect: [r.x, r.y, r.w, r.h],
            color: color.to_array(),
            color2: [0.0; 4],
            radii: [radius; 4],
            params: [KIND_RECT, 0.0, blur, 0.0],
            extra: [0.0; 4],
            clip,
        });
    }

    pub fn circle(&mut self, center: Vec2, radius: f32, color: Color) {
        let r = Rect::from_center(center, radius * 2.0, radius * 2.0);
        if !self.visible(&r) {
            return;
        }
        let clip = self.clip_arr();
        self.push(ShapeInstance {
            rect: [r.x, r.y, r.w, r.h],
            color: color.to_array(),
            color2: [0.0; 4],
            radii: [radius; 4],
            params: [KIND_RECT, 0.0, 0.0, 0.0],
            extra: [0.0; 4],
            clip,
        });
    }

    pub fn circle_stroke(
        &mut self,
        center: Vec2,
        radius: f32,
        fill: Color,
        stroke: f32,
        stroke_color: Color,
    ) {
        let r = Rect::from_center(center, radius * 2.0, radius * 2.0);
        if !self.visible(&r) {
            return;
        }
        let clip = self.clip_arr();
        self.push(ShapeInstance {
            rect: [r.x, r.y, r.w, r.h],
            color: fill.to_array(),
            color2: stroke_color.to_array(),
            radii: [radius; 4],
            params: [KIND_RECT, stroke, 0.0, 0.0],
            extra: [0.0; 4],
            clip,
        });
    }

    /// Arc with round caps. Angles in radians from 12 o'clock, clockwise.
    pub fn arc(
        &mut self,
        center: Vec2,
        radius: f32,
        thickness: f32,
        start: f32,
        sweep: f32,
        color: Color,
    ) {
        if sweep <= 0.0 {
            return;
        }
        let ext = radius + thickness;
        let r = Rect::from_center(center, ext * 2.0, ext * 2.0);
        if !self.visible(&r) {
            return;
        }
        let clip = self.clip_arr();
        self.push(ShapeInstance {
            rect: [r.x, r.y, r.w, r.h],
            color: color.to_array(),
            color2: [0.0; 4],
            radii: [0.0; 4],
            params: [KIND_ARC, 0.0, 0.0, 0.0],
            extra: [radius, thickness, start, sweep],
            clip,
        });
    }

    /// Line segment with round caps.
    pub fn line(&mut self, a: Vec2, b: Vec2, width: f32, color: Color) {
        let pad = width * 0.5 + 1.0;
        let r = Rect::from_min_max(
            Vec2 {
                x: a.x.min(b.x) - pad,
                y: a.y.min(b.y) - pad,
            },
            Vec2 {
                x: a.x.max(b.x) + pad,
                y: a.y.max(b.y) + pad,
            },
        );
        if !self.visible(&r) {
            return;
        }
        let clip = self.clip_arr();
        self.push(ShapeInstance {
            rect: [r.x, r.y, r.w, r.h],
            color: color.to_array(),
            color2: [0.0; 4],
            radii: [0.0; 4],
            params: [KIND_LINE, width, 0.0, 0.0],
            extra: [a.x, a.y, b.x, b.y],
            clip,
        });
    }

    /// Connected line segments.
    pub fn polyline(&mut self, points: &[Vec2], width: f32, color: Color) {
        for w in points.windows(2) {
            self.line(w[0], w[1], width, color);
        }
    }

    /// Crisp 1-physical-pixel horizontal line.
    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, color: Color) {
        let px = 1.0 / self.scale;
        let y = (y * self.scale).floor() / self.scale;
        self.rect(
            Rect {
                x: x0,
                y,
                w: x1 - x0,
                h: px,
            },
            0.0,
            color,
        );
    }

    /// Crisp 1-physical-pixel vertical line.
    pub fn vline(&mut self, x: f32, y0: f32, y1: f32, color: Color) {
        let px = 1.0 / self.scale;
        let x = (x * self.scale).floor() / self.scale;
        self.rect(
            Rect {
                x,
                y: y0,
                w: px,
                h: y1 - y0,
            },
            0.0,
            color,
        );
    }

    /// Triangle filling `r`, pointing in `dir`. `round` softens corners.
    pub fn triangle(&mut self, r: Rect, dir: Dir, round: f32, color: Color) {
        if !self.visible(&r) {
            return;
        }
        let clip = self.clip_arr();
        let d = match dir {
            Dir::Right => 0.0,
            Dir::Down => 1.0,
            Dir::Left => 2.0,
            Dir::Up => 3.0,
        };
        self.push(ShapeInstance {
            rect: [r.x, r.y, r.w, r.h],
            color: color.to_array(),
            color2: [0.0; 4],
            radii: [round, 0.0, 0.0, 0.0],
            params: [KIND_TRIANGLE, 0.0, 0.0, d],
            extra: [0.0; 4],
            clip,
        });
    }

    // ---- text ----

    pub(crate) fn push_text(&mut self, key: TextKey, pos: Vec2, color: Color) {
        let clip = self.clip();
        self.layers[self.layer].texts.push(TextItem {
            key,
            pos,
            color,
            clip,
        });
    }

    pub fn shape_count(&self) -> usize {
        self.layers.iter().map(|l| l.shapes.len()).sum()
    }
}

impl Default for Scene {
    fn default() -> Self {
        Scene::new()
    }
}

/// Convenience: text style and alignment for [`crate::Ui::text`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}
