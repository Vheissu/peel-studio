//! Peel Studio's GPU widget toolkit.
//!
//! Everything on screen is drawn by two pipelines: instanced signed-distance
//! shapes and glyphon text. Widgets are immediate-mode with stable ids; the
//! runner repaints only on input or when something animates.

pub mod app;
pub mod geom;
pub mod icons;
pub mod input;
pub mod render;
pub mod scene;
pub mod text;
pub mod theme;
pub mod ui;
pub mod widgets;

pub use app::{App, WindowControl, WindowOptions, run};
pub use geom::{Color, Rect, Vec2, rect, vec2};
pub use input::{Button, Input, Modifiers};
pub use scene::{Align, Dir, LayerId, Scene};
pub use text::{FontKind, FontWeight, TextStyle};
pub use theme::Theme;
pub use ui::{DragPayload, Id, Response, Ui};
pub use widgets::{MenuItem, meter_pos};
pub use winit::keyboard::{Key, KeyCode, NamedKey};
pub use winit::window::CursorIcon;
