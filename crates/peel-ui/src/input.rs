//! Input collected from window events between frames.

use std::path::PathBuf;

use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

use crate::geom::{Vec2, vec2};

const DOUBLE_CLICK_S: f64 = 0.35;
const DOUBLE_CLICK_DIST: f32 = 5.0;
/// Logical pixels per scroll "line".
const LINE_PX: f32 = 36.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Cmd on macOS.
    pub logo: bool,
}

impl Modifiers {
    /// The platform's primary shortcut modifier (Cmd on macOS, Ctrl elsewhere).
    pub fn command(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.logo
        } else {
            self.ctrl
        }
    }

    pub fn none(&self) -> bool {
        !self.shift && !self.ctrl && !self.alt && !self.logo
    }
}

#[derive(Clone, Debug)]
pub struct KeyPress {
    pub key: Key,
    pub code: Option<KeyCode>,
    pub repeat: bool,
    pub modifiers: Modifiers,
    pub consumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left = 0,
    Right = 1,
    Middle = 2,
}

#[derive(Debug, Default)]
pub struct Input {
    pub pointer: Option<Vec2>,
    pub pointer_delta: Vec2,
    pub down: [bool; 3],
    pub pressed: [bool; 3],
    pub released: [bool; 3],
    pub press_pos: [Vec2; 3],
    pub double_clicked: bool,
    /// Scroll in logical pixels; positive y scrolls content up (wheel down).
    pub scroll: Vec2,
    /// Trackpad pinch zoom delta (positive = zoom in).
    pub zoom: f32,
    pub keys: Vec<KeyPress>,
    pub keys_released: Vec<KeyPress>,
    pub text: String,
    pub modifiers: Modifiers,
    pub dropped_files: Vec<PathBuf>,
    pub hovering_files: bool,
    /// True if any event arrived since the last frame.
    pub had_event: bool,
    /// The window lost keyboard focus since the last frame: key-up events
    /// for held keys will never arrive, so held notes must be released.
    pub focus_lost: bool,
    last_click_time: f64,
    last_click_pos: Vec2,
    scale: f32,
}

impl Input {
    pub fn new() -> Input {
        Input {
            scale: 1.0,
            last_click_time: -10.0,
            ..Default::default()
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.max(0.1);
    }

    /// Feeds one window event. `now` is seconds since start.
    pub fn handle(&mut self, event: &WindowEvent, now: f64) {
        self.had_event = true;
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let p = vec2(
                    position.x as f32 / self.scale,
                    position.y as f32 / self.scale,
                );
                if let Some(old) = self.pointer {
                    self.pointer_delta += p - old;
                }
                self.pointer = Some(p);
            }
            WindowEvent::CursorLeft { .. } => {
                if !self.down.iter().any(|d| *d) {
                    self.pointer = None;
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                let pos = self.pointer.unwrap_or_default();
                match state {
                    ElementState::Pressed => {
                        self.down[b] = true;
                        self.pressed[b] = true;
                        self.press_pos[b] = pos;
                        if b == 0 {
                            if now - self.last_click_time < DOUBLE_CLICK_S
                                && (pos - self.last_click_pos).length() < DOUBLE_CLICK_DIST
                            {
                                self.double_clicked = true;
                                self.last_click_time = -10.0;
                            } else {
                                self.last_click_time = now;
                                self.last_click_pos = pos;
                            }
                        }
                    }
                    ElementState::Released => {
                        self.down[b] = false;
                        self.released[b] = true;
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(x, y) => vec2(*x * LINE_PX, *y * LINE_PX),
                    MouseScrollDelta::PixelDelta(p) => {
                        vec2(p.x as f32 / self.scale, p.y as f32 / self.scale)
                    }
                };
                self.scroll += d;
            }
            WindowEvent::PinchGesture { delta, .. } => {
                self.zoom += *delta as f32;
            }
            WindowEvent::ModifiersChanged(m) => {
                let s: ModifiersState = m.state();
                self.modifiers = Modifiers {
                    shift: s.shift_key(),
                    ctrl: s.control_key(),
                    alt: s.alt_key(),
                    logo: s.super_key(),
                };
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let code = match event.physical_key {
                    PhysicalKey::Code(c) => Some(c),
                    PhysicalKey::Unidentified(_) => None,
                };
                let press = KeyPress {
                    key: event.logical_key.clone(),
                    code,
                    repeat: event.repeat,
                    modifiers: self.modifiers,
                    consumed: false,
                };
                match event.state {
                    ElementState::Pressed => {
                        if let Some(t) = &event.text
                            && !self.modifiers.command()
                            && !self.modifiers.ctrl
                        {
                            for ch in t.chars().filter(|c| !c.is_control()) {
                                self.text.push(ch);
                            }
                        }
                        self.keys.push(press);
                    }
                    ElementState::Released => self.keys_released.push(press),
                }
            }
            WindowEvent::HoveredFile(_) => self.hovering_files = true,
            WindowEvent::HoveredFileCancelled => self.hovering_files = false,
            WindowEvent::DroppedFile(path) => {
                self.hovering_files = false;
                self.dropped_files.push(path.clone());
            }
            WindowEvent::Focused(false) => {
                self.focus_lost = true;
                self.down = [false; 3];
                self.modifiers = Modifiers::default();
            }
            _ => {}
        }
    }

    /// Clears per-frame state after a frame has been built.
    pub fn end_frame(&mut self) {
        self.pointer_delta = Vec2::ZERO;
        self.pressed = [false; 3];
        self.released = [false; 3];
        self.focus_lost = false;
        self.double_clicked = false;
        self.scroll = Vec2::ZERO;
        self.zoom = 0.0;
        self.keys.clear();
        self.keys_released.clear();
        self.text.clear();
        self.dropped_files.clear();
        self.had_event = false;
    }

    pub fn is_down(&self, b: Button) -> bool {
        self.down[b as usize]
    }

    pub fn pressed(&self, b: Button) -> bool {
        self.pressed[b as usize]
    }

    pub fn released(&self, b: Button) -> bool {
        self.released[b as usize]
    }

    /// Consumes and returns true if `key` was pressed with exactly these
    /// modifiers (shift is ignored when `ignore_shift`).
    pub fn take_key(&mut self, key: &Key, command: bool, shift: Option<bool>) -> bool {
        for k in self.keys.iter_mut() {
            if k.consumed || &k.key != key {
                continue;
            }
            if k.modifiers.command() != command {
                continue;
            }
            if let Some(s) = shift
                && k.modifiers.shift != s
            {
                continue;
            }
            k.consumed = true;
            return true;
        }
        false
    }

    pub fn take_named(&mut self, key: NamedKey) -> bool {
        self.take_key(&Key::Named(key), false, None)
    }

    /// Consumes a character shortcut such as `Cmd+Z` (`ch` lowercase).
    pub fn take_command(&mut self, ch: &str, shift: bool) -> bool {
        for k in self.keys.iter_mut() {
            if k.consumed || !k.modifiers.command() || k.modifiers.shift != shift {
                continue;
            }
            if let Key::Character(c) = &k.key
                && c.to_lowercase() == ch
            {
                k.consumed = true;
                return true;
            }
        }
        false
    }
}
