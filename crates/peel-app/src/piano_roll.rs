//! Piano Roll: edits one channel's notes in the selected pattern, with
//! other channels shown as ghost notes for harmonic context.

use std::collections::HashSet;

use peel_document::{Channel, Id, Note, Pattern, Time};
use peel_session::{PlayMode, Session};
use peel_ui::Id as W;
use peel_ui::{Align, Button, Color, CursorIcon, Key, MenuItem, NamedKey, Rect, Ui, icons, vec2};

use crate::studio::current_pattern;
use crate::util::{col, format_length, ink_on, is_black_key, note_name};
use crate::workspace::{Tool, Workspace};

const KEYS_W: f32 = 58.0;
const RULER_H: f32 = 22.0;
const VEL_H: f32 = 72.0;
const EDGE: f32 = 7.0;

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    None,
    Move,
    Resize,
    Marquee,
    Erase,
}

#[derive(Clone)]
struct DragState {
    kind: DragKind,
    anchor: Id,
    start_beat: f64,
    start_key: i32,
    origin: Vec<Note>,
    gesture: u64,
    last_key: u8,
    marquee_from: (f64, f32),
}

impl Default for DragState {
    fn default() -> Self {
        DragState {
            kind: DragKind::None,
            anchor: Id::NIL,
            start_beat: 0.0,
            start_key: 0,
            origin: Vec::new(),
            gesture: 0,
            last_key: 0,
            marquee_from: (0.0, 0.0),
        }
    }
}

#[derive(Default)]
struct RollMemo {
    channel: Option<Id>,
    held_key: Option<u8>,
}

struct View {
    canvas: Rect,
    zoom_x: f32,
    row_h: f32,
    scroll: f32,
    top_key: f32,
}

impl View {
    fn x(&self, beat: f64) -> f32 {
        self.canvas.x + (beat as f32 - self.scroll) * self.zoom_x
    }

    fn beat(&self, x: f32) -> f64 {
        ((x - self.canvas.x) / self.zoom_x + self.scroll) as f64
    }

    fn row_top(&self, key: i32) -> f32 {
        self.canvas.y + (self.top_key - key as f32) * self.row_h
    }

    fn key(&self, y: f32) -> i32 {
        (self.top_key - (y - self.canvas.y) / self.row_h).ceil() as i32
    }
}

pub fn draw(ui: &mut Ui, r: Rect, tools: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let pid = current_pattern(s);
    let Some(pattern) = s.project().pattern(pid).cloned() else {
        return;
    };
    let Some(channel) = ws
        .selected_channel
        .and_then(|c| s.project().channel(c).cloned())
        .or_else(|| s.project().channels.first().cloned())
    else {
        ui.text(
            r,
            "Add an instrument to start writing notes",
            t.body,
            t.text_faint,
            Align::Center,
        );
        return;
    };
    ws.selected_channel = Some(channel.id);

    header(ui, tools, &channel, ws);

    let mut area = r.shrink2(8.0, 0.0);
    area.cut_bottom(8.0);
    let vel_r = area.cut_bottom(VEL_H);
    area.cut_bottom(4.0);
    let mut top = area.cut_top(RULER_H);
    let keys_r = area.cut_left(KEYS_W);
    top.cut_left(KEYS_W);
    let canvas = area;

    // Recenter on the channel's notes when switching channels.
    let memo_id = W::new("roll-memo");
    let switched = ui.state::<RollMemo>(memo_id).channel != Some(channel.id);
    if switched {
        ui.state::<RollMemo>(memo_id).channel = Some(channel.id);
        ws.roll.selection.clear();
        let keys: Vec<u8> = pattern.notes_for(channel.id).map(|n| n.key).collect();
        let center = if keys.is_empty() {
            channel.root_key as f32
        } else {
            keys.iter().map(|&k| k as f32).sum::<f32>() / keys.len() as f32
        };
        ws.roll.top_key = center + canvas.h / ws.roll.row_h * 0.5;
    }

    // Navigation: wheel scrolls keys, Shift+wheel or trackpad x scrolls
    // time, Cmd/Ctrl+wheel zooms time, Alt+wheel zooms keys.
    let nav_r = Rect {
        x: keys_r.x,
        y: top.y,
        w: canvas.right() - keys_r.x,
        h: canvas.bottom() - top.y,
    };
    if ui.pointer_in(nav_r) {
        let sc = ui.input.scroll;
        let m = ui.input.modifiers;
        if sc != peel_ui::Vec2::ZERO || ui.input.zoom != 0.0 {
            if m.command() || ui.input.zoom != 0.0 {
                let p = ui.pointer().unwrap_or_default();
                let before = (p.x - canvas.x) / ws.roll.zoom_x + ws.roll.scroll_beats;
                let f = if ui.input.zoom != 0.0 {
                    1.0 + ui.input.zoom
                } else {
                    (1.0f32 + sc.y * 0.004).clamp(0.5, 2.0)
                };
                ws.roll.zoom_x = (ws.roll.zoom_x * f).clamp(16.0, 640.0);
                ws.roll.scroll_beats = before - (p.x - canvas.x) / ws.roll.zoom_x;
            } else if m.alt {
                ws.roll.row_h = (ws.roll.row_h * (1.0 + sc.y * 0.004)).clamp(8.0, 36.0);
            } else {
                let (dx, dy) = if m.shift && sc.x == 0.0 {
                    (sc.y, 0.0)
                } else {
                    (sc.x, sc.y)
                };
                ws.roll.scroll_beats -= dx / ws.roll.zoom_x;
                ws.roll.top_key += dy / ws.roll.row_h;
            }
            ui.input.scroll = peel_ui::Vec2::ZERO;
            ui.input.zoom = 0.0;
        }
    }
    let len_beats = pattern.length.as_f64() as f32;
    ws.roll.scroll_beats = ws
        .roll
        .scroll_beats
        .clamp(0.0, (len_beats + 8.0 - canvas.w / ws.roll.zoom_x).max(0.0));
    ws.roll.top_key = ws.roll.top_key.clamp(canvas.h / ws.roll.row_h, 128.0);
    let v = View {
        canvas,
        zoom_x: ws.roll.zoom_x,
        row_h: ws.roll.row_h,
        scroll: ws.roll.scroll_beats,
        top_key: ws.roll.top_key,
    };

    grid(ui, &v, &pattern, s, pid);
    ruler(ui, top, &v, &pattern, s, pid);
    keyboard(ui, keys_r, &v, &channel, s, memo_id);

    // Ghost notes from other channels.
    ui.scene.push_clip(canvas);
    if ws.roll.ghosts {
        let channels = s.project().channels.clone();
        for n in pattern.notes.iter().filter(|n| n.channel != channel.id) {
            let c = channels
                .iter()
                .find(|c| c.id == n.channel)
                .map_or(t.text_faint, |c| col(c.color));
            let nr = note_rect(&v, n);
            if nr.intersects(&canvas) {
                ui.scene.rect(nr, 2.0, c.alpha(0.16));
            }
        }
    }
    ui.scene.pop_clip();

    let notes: Vec<Note> = pattern.notes_for(channel.id).copied().collect();
    edit_canvas(ui, &v, &notes, &channel, s, ws, pid, &pattern);
    velocity_lane(ui, vel_r, &v, &notes, &channel, s, ws, pid);
    keyboard_shortcuts(ui, s, ws, pid, &channel, &notes, &pattern);

    // Playhead.
    if s.is_playing() && matches!(s.mode(), PlayMode::Pattern(p) if p == pid) {
        let b = s.position().rem_euclid(pattern.length.as_f64());
        let x = v.x(b);
        if x >= canvas.x && x <= canvas.right() {
            ui.with_layer(peel_ui::LayerId::ABOVE, |ui| {
                ui.scene.push_clip(Rect {
                    y: top.y,
                    h: canvas.bottom() - top.y,
                    ..canvas
                });
                ui.scene.rect(
                    Rect {
                        x: x - 1.0,
                        y: top.y,
                        w: 2.0,
                        h: canvas.bottom() - top.y,
                    },
                    1.0,
                    t.accent,
                );
                ui.scene.triangle(
                    Rect {
                        x: x - 6.0,
                        y: top.y,
                        w: 12.0,
                        h: 8.0,
                    },
                    peel_ui::Dir::Down,
                    0.5,
                    t.accent,
                );
                ui.scene.pop_clip();
            });
        }
    }
}

fn note_rect(v: &View, n: &Note) -> Rect {
    let x0 = v.x(n.start.as_f64());
    let x1 = v.x(n.end().as_f64());
    let y = v.row_top(n.key as i32);
    Rect {
        x: x0 + 0.5,
        y: y + 1.0,
        w: (x1 - x0 - 1.0).max(3.0),
        h: (v.row_h - 2.0).max(3.0),
    }
}

fn header(ui: &mut Ui, tools: Rect, ch: &Channel, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let mut row = tools;
    // Channel chip.
    let chip = row.cut_right(150.0);
    ui.scene.rect(chip, t.radius_m, t.control);
    let mut cr = chip.shrink2(10.0, 0.0);
    let sw = cr.cut_left(10.0);
    ui.scene.rect(
        Rect::from_center(sw.center(), 10.0, 10.0),
        3.0,
        col(ch.color),
    );
    cr.cut_left(8.0);
    let name = ui.fit_text(&ch.name, t.label, cr.w);
    ui.text(cr, &name, t.label, t.text, Align::Start);
    row.cut_right(10.0);
    // Ghost toggle.
    let g = ui.icon_button(
        W::new("roll-ghost"),
        row.cut_right(30.0),
        icons::SQUARES,
        ws.roll.ghosts,
        None,
    );
    ui.tooltip(&g, "Show other channels");
    if g.clicked {
        ws.roll.ghosts = !ws.roll.ghosts;
    }
    row.cut_right(6.0);
    // Snap.
    let snap_r = row.cut_right(70.0);
    let sresp = ui.interact(W::new("roll-snap"), snap_r);
    ui.scene.rect(
        snap_r,
        t.radius_m,
        if sresp.hovered {
            t.control_hover
        } else {
            t.control
        },
    );
    let mut sr = snap_r.shrink2(8.0, 0.0);
    ui.icon(sr.cut_left(14.0), icons::MAGNET, 13.0, t.text_dim);
    ui.text(
        sr,
        &format_length(ws.roll.snap),
        t.label,
        t.text,
        Align::Center,
    );
    ui.tooltip(&sresp, "Grid");
    if sresp.clicked {
        ui.open_popup(W::new("roll-snap-menu"), snap_r);
    }
    let snaps = [
        Time::new(1, 1),
        Time::new(1, 2),
        Time::new(1, 3),
        Time::new(1, 4),
        Time::new(1, 6),
        Time::new(1, 8),
    ];
    let items: Vec<MenuItem> = snaps
        .iter()
        .map(|s| MenuItem::new(format_length(*s)).checked(*s == ws.roll.snap))
        .collect();
    if let Some(i) = ui.menu(
        W::new("roll-snap-menu"),
        vec2(snap_r.x, snap_r.bottom() + 4.0),
        &items,
    ) {
        ws.roll.snap = snaps[i];
        ws.roll.note_len = snaps[i];
    }
    row.cut_right(10.0);
    // Tools.
    for (tool, icon, tip) in [
        (Tool::Erase, icons::ERASER, "Erase"),
        (Tool::Select, icons::CURSOR, "Select"),
        (Tool::Draw, icons::PENCIL, "Draw"),
    ] {
        let b = ui.icon_button(
            W::new(("roll-tool", tip)),
            row.cut_right(30.0),
            icon,
            ws.roll.tool == tool,
            None,
        );
        ui.tooltip(&b, tip);
        if b.clicked {
            ws.roll.tool = tool;
        }
        row.cut_right(2.0);
    }
}

fn grid(ui: &mut Ui, v: &View, pattern: &Pattern, s: &Session, _pid: Id) {
    let t = ui.theme.clone();
    let c = v.canvas;
    ui.scene.push_clip(c);
    ui.scene.rect(c, 0.0, t.panel_alt);
    let lo = v.key(c.bottom()) - 1;
    let hi = v.key(c.y) + 1;
    for k in lo.max(0)..=hi.min(127) {
        let y = v.row_top(k);
        let row = Rect {
            x: c.x,
            y,
            w: c.w,
            h: v.row_h,
        };
        if is_black_key(k as u8) {
            ui.scene.rect(row, 0.0, t.bg.mix(t.panel, 0.35));
        }
        if k % 12 == 0 {
            ui.scene.hline(c.x, c.right(), y + v.row_h, t.grid_bar);
        }
    }
    let sig = s.project().time_signature;
    let bar = sig.bar_length().as_f64();
    let snap = v.zoom_x as f64 * 0.25;
    let step = if snap >= 10.0 {
        0.25
    } else if v.zoom_x >= 20.0 {
        1.0
    } else {
        bar
    };
    let first = (v.scroll as f64 / step).floor() * step;
    let mut b = first;
    let end = v.beat(c.right());
    while b <= end {
        let x = v.x(b);
        let color = if (b / bar).fract().abs() < 1e-6 {
            t.grid_bar
        } else if b.fract().abs() < 1e-6 {
            t.grid_beat
        } else {
            t.grid_step
        };
        ui.scene.vline(x, c.y, c.bottom(), color);
        b += step;
    }
    // Shade beyond the pattern end.
    let xe = v.x(pattern.length.as_f64());
    if xe < c.right() {
        ui.scene.rect(
            Rect {
                x: xe,
                y: c.y,
                w: c.right() - xe,
                h: c.h,
            },
            0.0,
            t.bg.alpha(0.55),
        );
        ui.scene.vline(xe, c.y, c.bottom(), t.accent.alpha(0.5));
    }
    ui.scene.pop_clip();
}

fn ruler(ui: &mut Ui, r: Rect, v: &View, pattern: &Pattern, s: &mut Session, pid: Id) {
    let t = ui.theme.clone();
    ui.scene.rect(r, 0.0, t.panel);
    ui.scene.push_clip(r);
    let bar = s.project().time_signature.bar_length().as_f64();
    let mut b = (v.scroll as f64 / bar).floor() * bar;
    while v.x(b) < r.right() {
        let x = v.x(b);
        ui.scene
            .vline(x, r.y + 10.0, r.bottom(), t.text_faint.alpha(0.5));
        ui.text_at(
            vec2(x + 4.0, r.y + 2.0),
            &format!("{}", (b / bar) as i64 + 1),
            t.caption,
            t.text_dim,
        );
        b += bar;
    }
    // Pattern length handle.
    let xe = v.x(pattern.length.as_f64());
    let handle = Rect {
        x: xe - 6.0,
        y: r.y,
        w: 12.0,
        h: r.h,
    };
    let id = W::new("roll-len-handle");
    let resp = ui.interact(id, handle);
    ui.scene.triangle(
        Rect {
            x: xe - 6.0,
            y: r.bottom() - 9.0,
            w: 12.0,
            h: 9.0,
        },
        peel_ui::Dir::Down,
        0.5,
        if resp.hovered || resp.active {
            t.accent
        } else {
            t.accent.alpha(0.6)
        },
    );
    if resp.hovered || resp.active {
        ui.set_cursor(CursorIcon::EwResize);
    }
    ui.tooltip(&resp, "Drag to change the pattern length");
    if resp.dragging
        && let Some(p) = ui.pointer()
    {
        let beat = v.beat(p.x).max(0.25);
        let len = Time::from_f64(beat, 4)
            .round_to(Time::beats(1))
            .max(Time::beats(1));
        if len != pattern.length {
            s.doc.edit_gesture(id.0, "Change pattern length", |p| {
                if let Some(pt) = p.pattern_mut(pid) {
                    pt.length = len;
                }
            });
        }
    }
    if resp.released {
        s.doc.end_gesture();
    }
    ui.scene.pop_clip();
}

fn keyboard(ui: &mut Ui, r: Rect, v: &View, ch: &Channel, s: &mut Session, memo_id: W) {
    let t = ui.theme.clone();
    ui.scene.push_clip(r);
    ui.scene.rect(r, 0.0, t.panel);
    let lo = v.key(r.bottom()) - 1;
    let hi = v.key(r.y) + 1;
    let held = ui.state::<RollMemo>(memo_id).held_key;
    for k in lo.max(0)..=hi.min(127) {
        let y = v.row_top(k);
        let black = is_black_key(k as u8);
        let pressed = held == Some(k as u8);
        let key_r = if black {
            Rect {
                x: r.x,
                y,
                w: r.w * 0.62,
                h: v.row_h,
            }
        } else {
            Rect {
                x: r.x,
                y,
                w: r.w - 1.0,
                h: v.row_h,
            }
        };
        let fill = if pressed {
            col(ch.color)
        } else if black {
            t.key_black
        } else {
            t.key_white
        };
        ui.scene.rect_ex(
            key_r.shrink2(0.0, 0.5),
            [0.0, 3.0, 3.0, 0.0],
            fill,
            0.0,
            Color::TRANSPARENT,
        );
        if k % 12 == 0 && v.row_h >= 10.0 {
            ui.text(
                Rect {
                    x: r.x,
                    w: r.w - 6.0,
                    ..key_r
                },
                &note_name(k as u8),
                t.caption.size(10.5),
                Color::hex(0x4A5260),
                Align::End,
            );
        }
    }
    ui.scene.pop_clip();
    let id = W::new("roll-keys");
    let resp = ui.interact(id, r);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    if (resp.pressed || resp.dragging)
        && let Some(p) = ui.pointer()
    {
        let k = v.key(p.y).clamp(0, 127) as u8;
        let memo = ui.state::<RollMemo>(memo_id);
        if memo.held_key != Some(k) {
            let prev = memo.held_key.replace(k);
            if let Some(pk) = prev {
                s.live_note(ch.id, pk, 0.0, false);
            }
            s.live_note(ch.id, k, 0.85, true);
        }
    }
    if (resp.released || (!ui.input.down[0] && held.is_some()))
        && let Some(pk) = ui.state::<RollMemo>(memo_id).held_key.take()
    {
        s.live_note(ch.id, pk, 0.0, false);
    }
}

#[allow(clippy::too_many_arguments)]
fn edit_canvas(
    ui: &mut Ui,
    v: &View,
    notes: &[Note],
    ch: &Channel,
    s: &mut Session,
    ws: &mut Workspace,
    pid: Id,
    pattern: &Pattern,
) {
    let t = ui.theme.clone();
    let c = v.canvas;
    let color = col(ch.color);
    let id = W::new("roll-canvas");
    let resp = ui.interact(id, c);
    let drag_id = W::new("roll-drag");
    let pointer = ui.pointer();

    // Hit test topmost note under the pointer.
    let hit = pointer.filter(|p| c.contains(*p)).and_then(|p| {
        notes
            .iter()
            .rev()
            .find(|n| note_rect(v, n).contains(p))
            .map(|n| {
                let nr = note_rect(v, n);
                (*n, p.x > nr.right() - EDGE.min(nr.w * 0.35))
            })
    });

    // Cursor feedback.
    if resp.hovered && ui.state::<DragState>(drag_id).kind == DragKind::None {
        match (ws.roll.tool, hit) {
            (Tool::Erase, _) => ui.set_cursor(CursorIcon::Crosshair),
            (_, Some((_, true))) => ui.set_cursor(CursorIcon::EwResize),
            (_, Some(_)) => ui.set_cursor(CursorIcon::Grab),
            (Tool::Select, None) => ui.set_cursor(CursorIcon::Crosshair),
            _ => ui.set_cursor(CursorIcon::Default),
        }
    }

    let snap = ws.roll.snap;
    if resp.pressed
        && let Some(p) = pointer
    {
        let beat = v.beat(p.x).max(0.0);
        let key = v.key(p.y).clamp(0, 127);
        let marquee = ws.roll.tool == Tool::Select && hit.is_none()
            || (hit.is_none() && ui.input.modifiers.command());
        let mut st = DragState {
            start_beat: beat,
            start_key: key,
            gesture: id.0 ^ (ui.time() * 1000.0) as u64,
            ..Default::default()
        };
        if ws.roll.tool == Tool::Erase {
            st.kind = DragKind::Erase;
        } else if marquee {
            st.kind = DragKind::Marquee;
            st.marquee_from = (beat, p.y);
            if !ui.input.modifiers.shift {
                ws.roll.selection.clear();
            }
        } else if let Some((n, edge)) = hit {
            if !ws.roll.selection.contains(&n.id) {
                if !ui.input.modifiers.shift {
                    ws.roll.selection.clear();
                }
                ws.roll.selection.insert(n.id);
            }
            st.kind = if edge {
                DragKind::Resize
            } else {
                DragKind::Move
            };
            st.anchor = n.id;
            st.last_key = n.key;
            st.origin = notes
                .iter()
                .filter(|x| ws.roll.selection.contains(&x.id))
                .copied()
                .collect();
            ws.roll.note_len = n.length;
            if !edge {
                s.live_note(ch.id, n.key, n.velocity, true);
                s.live_note(ch.id, n.key, 0.0, false);
            }
        } else if ws.roll.tool == Tool::Draw || ws.roll.tool == Tool::Paint {
            // Create a note and keep dragging it.
            let start = Time::from_f64(beat, 960).floor_to(snap);
            let n = Note::new(ch.id, start, ws.roll.note_len, key as u8, 0.8);
            let nid = n.id;
            s.doc.edit_gesture(st.gesture, "Add note", |p| {
                if let Some(pt) = p.pattern_mut(pid) {
                    pt.notes.push(n);
                    pt.sort_notes();
                }
            });
            ws.roll.selection.clear();
            ws.roll.selection.insert(nid);
            st.kind = DragKind::Move;
            st.anchor = nid;
            st.last_key = key as u8;
            st.origin = vec![n];
            s.live_note(ch.id, key as u8, 0.8, true);
            s.live_note(ch.id, key as u8, 0.0, false);
            // Lengthen the pattern if the note lands past its end.
            if n.end() > pattern.length {
                let bars = n.end().ceil_to(Time::beats(4));
                s.doc.edit_gesture(st.gesture, "Add note", |p| {
                    if let Some(pt) = p.pattern_mut(pid) {
                        pt.length = bars;
                    }
                });
            }
        }
        *ui.state::<DragState>(drag_id) = st;
    }

    // Right button erases (click or drag).
    if ui.input.down[Button::Right as usize]
        && resp.hovered
        && let Some((n, _)) = hit
    {
        let gid = id.0 ^ 0xe7a5e;
        s.doc.edit_gesture(gid, "Delete notes", |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                pt.notes.retain(|x| x.id != n.id);
            }
        });
        ws.roll.selection.remove(&n.id);
    }
    if ui.input.released(Button::Right) {
        s.doc.end_gesture();
    }

    let st = ui.state::<DragState>(drag_id).clone();
    if st.kind != DragKind::None && ui.input.down[0] {
        if let Some(p) = pointer {
            let beat = v.beat(p.x);
            match st.kind {
                DragKind::Move => {
                    if let Some(anchor) = st.origin.iter().find(|n| n.id == st.anchor) {
                        let raw = anchor.start.as_f64() + (beat - st.start_beat);
                        let new_start = Time::from_f64(raw.max(0.0), 960).round_to(snap);
                        let dt = new_start - anchor.start;
                        let dk = v.key(p.y).clamp(0, 127) - st.start_key;
                        let origin = st.origin.clone();
                        let min_start = origin.iter().map(|n| n.start).min().unwrap_or(Time::ZERO);
                        let dt = if min_start + dt < Time::ZERO {
                            -min_start
                        } else {
                            dt
                        };
                        let key_now = (anchor.key as i32 + dk).clamp(0, 127) as u8;
                        s.doc.edit_gesture(
                            st.gesture,
                            format!(
                                "Move {} note{}",
                                origin.len(),
                                if origin.len() == 1 { "" } else { "s" }
                            ),
                            |p| {
                                if let Some(pt) = p.pattern_mut(pid) {
                                    for o in &origin {
                                        if let Some(n) = pt.notes.iter_mut().find(|n| n.id == o.id)
                                        {
                                            n.start = o.start + dt;
                                            n.key = (o.key as i32 + dk).clamp(0, 127) as u8;
                                        }
                                    }
                                    pt.sort_notes();
                                }
                            },
                        );
                        if key_now != st.last_key {
                            s.live_note(ch.id, key_now, 0.75, true);
                            s.live_note(ch.id, key_now, 0.0, false);
                            ui.state::<DragState>(drag_id).last_key = key_now;
                        }
                        ui.set_cursor(CursorIcon::Grabbing);
                    }
                }
                DragKind::Resize => {
                    if let Some(anchor) = st.origin.iter().find(|n| n.id == st.anchor) {
                        let raw_end = anchor.end().as_f64() + (beat - st.start_beat);
                        let new_end = Time::from_f64(raw_end, 960).round_to(snap);
                        let dl = new_end - anchor.end();
                        let origin = st.origin.clone();
                        let mut last_len = anchor.length;
                        s.doc.edit_gesture(st.gesture, "Resize notes", |p| {
                            if let Some(pt) = p.pattern_mut(pid) {
                                for o in &origin {
                                    if let Some(n) = pt.notes.iter_mut().find(|n| n.id == o.id) {
                                        n.length = (o.length + dl).max(snap);
                                        if o.id == anchor.id {
                                            last_len = n.length;
                                        }
                                    }
                                }
                            }
                        });
                        ws.roll.note_len = last_len;
                        ui.set_cursor(CursorIcon::EwResize);
                    }
                }
                DragKind::Erase => {
                    if let Some((n, _)) = hit {
                        s.doc.edit_gesture(st.gesture, "Delete notes", |p| {
                            if let Some(pt) = p.pattern_mut(pid) {
                                pt.notes.retain(|x| x.id != n.id);
                            }
                        });
                    }
                }
                DragKind::Marquee => {
                    let (b0, y0) = st.marquee_from;
                    let x0 = v.x(b0);
                    let mr = Rect::from_min_max(
                        vec2(x0.min(p.x), y0.min(p.y)),
                        vec2(x0.max(p.x), y0.max(p.y)),
                    );
                    ui.with_layer(peel_ui::LayerId::ABOVE, |ui| {
                        ui.scene.push_clip(c);
                        ui.scene
                            .rect_stroke(mr, 2.0, t.focus.alpha(0.12), 1.0, t.focus.alpha(0.8));
                        ui.scene.pop_clip();
                    });
                    let inside: HashSet<Id> = notes
                        .iter()
                        .filter(|n| note_rect(v, n).intersects(&mr))
                        .map(|n| n.id)
                        .collect();
                    if !ui.input.modifiers.shift {
                        ws.roll.selection = inside;
                    } else {
                        ws.roll.selection.extend(inside);
                    }
                }
                _ => {}
            }
        }
    } else if st.kind != DragKind::None {
        // Drag finished: a plain click on a note with the draw tool keeps it
        // selected; a click on an empty spot made a note.
        ui.state::<DragState>(drag_id).kind = DragKind::None;
        s.doc.end_gesture();
    }

    // Draw notes (after edits so they reflect this frame's changes).
    let fresh: Vec<Note> = s
        .project()
        .pattern(pid)
        .map(|pt| pt.notes_for(ch.id).copied().collect())
        .unwrap_or_default();
    ui.scene.push_clip(c);
    for n in &fresh {
        let nr = note_rect(v, n);
        if !nr.intersects(&c) {
            continue;
        }
        let sel = ws.roll.selection.contains(&n.id);
        let fill = color.mix(t.panel_alt, (1.0 - n.velocity) * 0.45);
        let hovered = hit.is_some_and(|(h, _)| h.id == n.id);
        let fill = if sel {
            fill.lighten(0.18)
        } else if hovered {
            fill.lighten(0.08)
        } else {
            fill
        };
        ui.scene.rect_stroke(
            nr,
            3.0,
            fill,
            1.0,
            if sel {
                Color::WHITE.alpha(0.9)
            } else {
                color.darken(0.35)
            },
        );
        // Velocity tick on the left edge.
        ui.scene.rect(
            Rect {
                x: nr.x + 2.0,
                y: nr.y + 2.0,
                w: 2.0,
                h: (nr.h - 4.0).max(1.0),
            },
            1.0,
            ink_on(fill).alpha(0.35),
        );
        if nr.w > 30.0 && v.row_h >= 13.0 {
            ui.text(
                Rect {
                    x: nr.x + 7.0,
                    w: nr.w - 9.0,
                    ..nr
                },
                &note_name(n.key),
                t.caption.size(10.5),
                ink_on(fill).alpha(0.8),
                Align::Start,
            );
        }
    }
    ui.scene.pop_clip();
}

#[allow(clippy::too_many_arguments)]
fn velocity_lane(
    ui: &mut Ui,
    r: Rect,
    v: &View,
    notes: &[Note],
    ch: &Channel,
    s: &mut Session,
    ws: &mut Workspace,
    pid: Id,
) {
    let t = ui.theme.clone();
    let lane = Rect {
        x: v.canvas.x,
        w: v.canvas.w,
        ..r
    };
    let label = Rect {
        x: r.x,
        w: KEYS_W,
        ..r
    };
    ui.scene.rect(r, t.radius_s, t.panel_alt);
    ui.text(
        label.shrink2(8.0, 0.0),
        "Velocity",
        t.caption,
        t.text_faint,
        Align::Start,
    );
    let color = col(ch.color);
    ui.scene.push_clip(lane);
    let inner = lane.shrink2(0.0, 8.0);
    for n in notes {
        let x = v.x(n.start.as_f64());
        if x < lane.x - 4.0 || x > lane.right() + 4.0 {
            continue;
        }
        let h = inner.h * n.velocity;
        let sel = ws.roll.selection.contains(&n.id);
        let c = if sel { color.lighten(0.25) } else { color };
        ui.scene.rect(
            Rect {
                x: x - 1.0,
                y: inner.bottom() - h,
                w: 2.0,
                h,
            },
            1.0,
            c.alpha(0.8),
        );
        ui.scene.circle(vec2(x, inner.bottom() - h), 3.5, c);
    }
    ui.scene.pop_clip();
    let id = W::new("roll-vel");
    let resp = ui.interact(id, lane);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Crosshair);
    }
    if (resp.pressed || resp.dragging)
        && let Some(p) = ui.pointer()
    {
        let vel = ((inner.bottom() - p.y) / inner.h).clamp(0.02, 1.0);
        let only_sel = !ws.roll.selection.is_empty();
        let targets: Vec<Id> = notes
            .iter()
            .filter(|n| (v.x(n.start.as_f64()) - p.x).abs() <= 5.0)
            .filter(|n| !only_sel || ws.roll.selection.contains(&n.id))
            .map(|n| n.id)
            .collect();
        if !targets.is_empty() {
            s.doc.edit_gesture(id.0, "Change velocity", |p| {
                if let Some(pt) = p.pattern_mut(pid) {
                    for n in pt.notes.iter_mut().filter(|n| targets.contains(&n.id)) {
                        n.velocity = vel;
                    }
                }
            });
        }
    }
    if resp.released {
        s.doc.end_gesture();
    }
}

fn keyboard_shortcuts(
    ui: &mut Ui,
    s: &mut Session,
    ws: &mut Workspace,
    pid: Id,
    ch: &Channel,
    notes: &[Note],
    pattern: &Pattern,
) {
    if ui.text_focus() {
        return;
    }
    let sel: Vec<Note> = notes
        .iter()
        .filter(|n| ws.roll.selection.contains(&n.id))
        .copied()
        .collect();
    if ui.input.take_command("a", false) {
        ws.roll.selection = notes.iter().map(|n| n.id).collect();
    }
    if sel.is_empty() {
        return;
    }
    let ids: HashSet<Id> = sel.iter().map(|n| n.id).collect();
    if ui.input.take_named(NamedKey::Delete) || ui.input.take_named(NamedKey::Backspace) {
        s.doc.edit(
            format!(
                "Delete {} note{}",
                sel.len(),
                if sel.len() == 1 { "" } else { "s" }
            ),
            |p| {
                if let Some(pt) = p.pattern_mut(pid) {
                    pt.notes.retain(|n| !ids.contains(&n.id));
                }
            },
        );
        ws.roll.selection.clear();
        return;
    }
    let up = ui
        .input
        .take_key(&Key::Named(NamedKey::ArrowUp), false, None);
    let down = ui
        .input
        .take_key(&Key::Named(NamedKey::ArrowDown), false, None);
    if up || down {
        let octave = ui.input.modifiers.shift;
        let d: i32 = if octave { 12 } else { 1 } * if up { 1 } else { -1 };
        s.doc.edit("Transpose notes", |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                for n in pt.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
                    n.key = (n.key as i32 + d).clamp(0, 127) as u8;
                }
            }
        });
        if let Some(n) = sel.first() {
            let k = (n.key as i32 + d).clamp(0, 127) as u8;
            s.live_note(ch.id, k, 0.7, true);
            s.live_note(ch.id, k, 0.0, false);
        }
    }
    if ui.input.take_command("d", false) {
        let start = sel.iter().map(|n| n.start).min().unwrap_or(Time::ZERO);
        let end = sel.iter().map(|n| n.end()).max().unwrap_or(Time::ZERO);
        let span = (end - start).ceil_to(ws.roll.snap).max(ws.roll.snap);
        let copies: Vec<Note> = sel
            .iter()
            .map(|n| Note {
                id: Id::new(),
                start: n.start + span,
                ..*n
            })
            .collect();
        let new_ids: HashSet<Id> = copies.iter().map(|n| n.id).collect();
        let needed = copies.iter().map(|n| n.end()).max().unwrap_or(Time::ZERO);
        let len = pattern.length;
        s.doc.edit("Duplicate notes", |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                pt.notes.extend(copies);
                pt.sort_notes();
                if needed > len {
                    pt.length = needed.ceil_to(Time::beats(4));
                }
            }
        });
        ws.roll.selection = new_ids;
    }
}
