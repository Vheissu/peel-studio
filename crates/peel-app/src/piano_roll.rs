//! Piano Roll: edits one channel's notes in the selected pattern, with
//! other channels shown as ghost notes for harmonic context.

use std::collections::HashSet;

use std::collections::HashMap;

use peel_document::{Channel, ExpressionPoint, Id, Note, Pattern, Time};
use peel_session::{PlayMode, Session};
use peel_ui::Id as W;
use peel_ui::{Align, Button, Color, CursorIcon, Key, MenuItem, NamedKey, Rect, Ui, icons, vec2};

use crate::note_expr::{Lane, PITCH_RANGES, fit_range, points, set_curve};
use crate::studio::current_pattern;
use crate::util::{SCROLLBAR, col, format_length, ink_on, is_black_key, note_name};
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
    /// Expression of `origin` when the drag began (resizing prunes from it).
    origin_expr: Vec<peel_document::NoteExpression>,
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
            origin_expr: Vec::new(),
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

    let mut actions = header(ui, tools, &channel, ws);
    let quantize = actions.quantize;
    // `PEEL_NOTE_TOOLS=<tool index>` opens the note tools for screenshots.
    if let Ok(v) = std::env::var("PEEL_NOTE_TOOLS")
        && !std::mem::replace(ui.state::<bool>(W::new("debug-note-tools")), true)
    {
        crate::note_tools::select(ui, v.parse().unwrap_or(0));
        actions.note_tools = ui.widget_rect(W::new("roll-note-tools"));
    }

    // The editor runs edge to edge in the dock: keys, canvas and the
    // velocity lane meet at separators rather than sitting in boxes.
    let mut area = r;
    let vel_r = area.cut_bottom(VEL_H);
    area.cut_bottom(1.0);
    let hbar_row = area.cut_bottom(SCROLLBAR);
    let vbar_col = area.cut_right(SCROLLBAR);
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
    let view_beats = canvas.w / ws.roll.zoom_x;
    let mut scroll = ws.roll.scroll_beats;
    ui.scrollbar(
        W::new("roll-hbar"),
        Rect {
            x: canvas.x,
            w: canvas.w,
            ..hbar_row
        },
        true,
        &mut scroll,
        view_beats,
        (len_beats + 8.0).max(view_beats),
    );
    ws.roll.scroll_beats = scroll;
    // Keys run top to bottom from 128, in pixels.
    let mut key_px = (128.0 - ws.roll.top_key) * ws.roll.row_h;
    if ui.scrollbar(
        W::new("roll-vbar"),
        Rect {
            y: canvas.y,
            h: canvas.h,
            ..vbar_col
        },
        false,
        &mut key_px,
        canvas.h,
        128.0 * ws.roll.row_h,
    ) {
        ws.roll.top_key = 128.0 - key_px / ws.roll.row_h;
    }
    let v = View {
        canvas,
        zoom_x: ws.roll.zoom_x,
        row_h: ws.roll.row_h,
        scroll: ws.roll.scroll_beats,
        top_key: ws.roll.top_key,
    };

    grid(ui, &v, &pattern, s, ws.roll.scale);
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
    lower_lane(ui, vel_r, &v, &notes, &channel, s, ws, pid);
    let paste_at = ui
        .pointer()
        .filter(|p| canvas.contains(*p))
        .map(|p| Time::from_f64(v.beat(p.x).max(0.0), 960).floor_to(ws.roll.snap));
    keyboard_shortcuts(
        ui, s, ws, pid, &channel, &notes, &pattern, paste_at, quantize,
    );
    if let Some(anchor) = actions.note_tools {
        let targets: Vec<Note> = if ws.roll.selection.is_empty() {
            notes.clone()
        } else {
            notes
                .iter()
                .filter(|n| ws.roll.selection.contains(&n.id))
                .copied()
                .collect()
        };
        crate::note_tools::open(ui, anchor, s, ws, pid, targets);
    }
    crate::note_tools::panel(ui, s, ws);

    // Playhead.
    if s.is_playing() && matches!(s.mode(), PlayMode::Pattern(p) if p == pid) {
        let b = s.position().rem_euclid(pattern.length.as_f64());
        let x = v.x(b);
        // Follow playback the same way the Playlist does.
        let last = std::mem::replace(ui.state::<f64>(W::new("roll-follow-last")), b);
        let wrapped = b + 1e-6 < last;
        let out = x < canvas.x || x > canvas.right() - 40.0;
        if (x > canvas.right() - 40.0 && x <= canvas.right() + 40.0) || (wrapped && out) {
            ws.roll.scroll_beats = (b as f32 - 1.0).max(0.0);
        }
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

/// What the header asked for this frame.
struct HeaderActions {
    quantize: bool,
    /// The note tools button was clicked (its rect anchors the panel).
    note_tools: Option<Rect>,
}

fn header(ui: &mut Ui, tools: Rect, ch: &Channel, ws: &mut Workspace) -> HeaderActions {
    let t = ui.theme.clone();
    let mut row = tools;
    // Narrow docks drop the chord and scale names (icons stay, tinted when
    // active) before anything else.
    let compact = row.w < 600.0;
    // Tighter still (the minimum window): the channel shows as its swatch,
    // its name in the tooltip and accessible label.
    let tight = row.w < 470.0;
    // Cell width of the icon controls.
    let cw = if tight { 28.0 } else { 30.0 };
    // The channel being edited: swatch and name, quiet.
    let chip_w = if tight {
        18.0
    } else {
        (ui.measure(&ch.name, t.label).x + 26.0).min(if compact { 96.0 } else { 140.0 })
    };
    let chip = row.cut_right(chip_w);
    let mut cr = chip;
    let sw = cr.cut_left(10.0);
    ui.scene.rect(
        Rect::from_center(sw.center(), 10.0, 10.0),
        3.0,
        col(ch.color),
    );
    if tight {
        let resp = ui.interact(W::new("roll-channel-chip"), chip);
        ui.tooltip(&resp, &format!("Editing {}", ch.name));
    } else {
        cr.cut_left(8.0);
        let name = ui.fit_text(&ch.name, t.label, cr.w);
        ui.text(cr, &name, t.label, t.text, Align::Start);
    }
    ui.describe_label(W::new("roll-channel"), chip, &ch.name);
    row.cut_right(if tight { 6.0 } else { 8.0 });
    ui.divider_v(row.cut_right(1.0));
    row.cut_right(6.0);

    // Ghosts, grid, chord stamp and scale.
    let g = ui.icon_button(
        W::new("roll-ghost"),
        row.cut_right(cw),
        icons::SQUARES,
        ws.roll.ghosts,
        None,
    );
    ui.name(&g, "Show other channels");
    ui.tooltip(&g, "Show other channels");
    if g.clicked {
        ws.roll.ghosts = !ws.roll.ghosts;
    }
    row.cut_right(2.0);
    let snap_r = row.cut_right(if tight { 48.0 } else { 62.0 });
    let sresp = if tight {
        // Value only: the magnet goes, the name stays in the tooltip.
        let r = ui.quiet_button(
            W::new("roll-snap"),
            snap_r,
            None,
            &format_length(ws.roll.snap),
            false,
            ui.is_popup_open(W::new("roll-snap-menu")),
        );
        ui.name(&r, "Grid");
        ui.value_text(&r, &format_length(ws.roll.snap));
        ui.tooltip(&r, "Grid");
        r
    } else {
        ui.value_button(
            W::new("roll-snap"),
            snap_r,
            icons::MAGNET,
            &format_length(ws.roll.snap),
            "Grid",
            false,
        )
    };
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
    row.cut_right(2.0);
    // Chord stamp.
    let on = ws.roll.chord != 0;
    let chord_label = if on {
        crate::harmony::CHORDS[ws.roll.chord].0
    } else {
        "Chord"
    };
    let chord_r = row.cut_right(if compact { cw } else { 88.0 });
    let cresp = ui.value_button(
        W::new("roll-chord"),
        chord_r,
        icons::MUSIC_NOTES,
        if compact { "" } else { chord_label },
        "Stamp a chord with every click",
        on,
    );
    ui.value_text(&cresp, chord_label);
    if cresp.clicked {
        ui.open_popup(W::new("roll-chord-menu"), chord_r);
    }
    let items: Vec<MenuItem> = crate::harmony::CHORDS
        .iter()
        .enumerate()
        .map(|(i, (n, _))| MenuItem::new(*n).checked(i == ws.roll.chord))
        .collect();
    if let Some(i) = ui.menu(
        W::new("roll-chord-menu"),
        vec2(chord_r.x, chord_r.bottom() + 4.0),
        &items,
    ) {
        ws.roll.chord = i;
    }
    row.cut_right(2.0);
    // Scale.
    let on = ws.roll.scale.is_some();
    let scale_label = ws
        .roll
        .scale
        .map(|s| s.name())
        .unwrap_or_else(|| "Scale".into());
    let scale_r = row.cut_right(if compact { cw } else { 112.0 });
    let sresp = ui.value_button(
        W::new("roll-scale"),
        scale_r,
        icons::PIANO_KEYS,
        if compact { "" } else { &scale_label },
        "Highlight a scale and snap notes to it",
        on,
    );
    ui.value_text(&sresp, &scale_label);
    if sresp.clicked {
        ui.open_popup(W::new("roll-scale-menu"), scale_r);
    }
    let mut items = vec![MenuItem::new("No scale").checked(ws.roll.scale.is_none())];
    for (i, (name, _)) in crate::harmony::SCALES.iter().enumerate() {
        items.push(MenuItem::new(*name).checked(ws.roll.scale.is_some_and(|s| s.kind == i)));
    }
    items.push(MenuItem::Separator);
    items.push(MenuItem::Heading("Root".into()));
    let root_base = items.len();
    for (i, n) in crate::harmony::NOTE_NAMES.iter().enumerate() {
        items.push(MenuItem::new(*n).checked(ws.roll.scale.is_some_and(|s| s.root as usize == i)));
    }
    if let Some(i) = ui.menu(
        W::new("roll-scale-menu"),
        vec2(scale_r.x, scale_r.bottom() + 4.0),
        &items,
    ) {
        let current = ws
            .roll
            .scale
            .unwrap_or(crate::harmony::Scale { root: 0, kind: 0 });
        if i == 0 {
            ws.roll.scale = None;
        } else if i <= crate::harmony::SCALES.len() {
            ws.roll.scale = Some(crate::harmony::Scale {
                kind: i - 1,
                ..current
            });
        } else if i >= root_base {
            ws.roll.scale = Some(crate::harmony::Scale {
                root: (i - root_base) as u8,
                ..current
            });
        }
    }
    if tight {
        row.cut_right(4.0);
    } else {
        row.cut_right(6.0);
        ui.divider_v(row.cut_right(1.0));
        row.cut_right(6.0);
    }

    // Note actions, then the editing tools in a well.
    let nt = ui.icon_button(
        W::new("roll-note-tools"),
        row.cut_right(cw),
        icons::SPARKLE,
        crate::note_tools::is_open(ui),
        None,
    );
    ui.name(&nt, "Note tools");
    ui.tooltip(&nt, "Note tools: legato, chop, arpeggiate, strum and more");
    let q = ui.icon_button(
        W::new("roll-quantize"),
        row.cut_right(cw),
        icons::GRID,
        false,
        None,
    );
    ui.name(&q, "Quantize");
    ui.tooltip(&q, "Quantize to the grid (selection, or all notes)");
    row.cut_right(if tight { 4.0 } else { 6.0 });
    let tools = [
        (Tool::Draw, icons::PENCIL, "Draw"),
        (Tool::Select, icons::CURSOR, "Select"),
        (Tool::Erase, icons::ERASER, "Erase"),
    ];
    let well = row.cut_right(tools.len() as f32 * cw + 4.0);
    ui.well(well);
    let mut cells = well.shrink(2.0);
    for (tool, icon, tip) in tools {
        let b = ui.tool_button(
            W::new(("roll-tool", tip)),
            cells.cut_left(cw),
            icon,
            tip,
            ws.roll.tool == tool,
        );
        if b.clicked {
            ws.roll.tool = tool;
        }
    }
    HeaderActions {
        quantize: q.clicked,
        note_tools: nt.clicked.then_some(nt.rect),
    }
}

fn grid(
    ui: &mut Ui,
    v: &View,
    pattern: &Pattern,
    s: &Session,
    scale: Option<crate::harmony::Scale>,
) {
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
        match scale {
            Some(sc) => {
                if !sc.contains(k as u8) {
                    ui.scene.rect(row, 0.0, t.bg.mix(t.panel, 0.25));
                } else if (k - sc.root as i32).rem_euclid(12) == 0 {
                    ui.scene.rect(row, 0.0, t.accent.alpha(0.07));
                }
            }
            None => {
                if is_black_key(k as u8) {
                    ui.scene.rect(row, 0.0, t.bg.mix(t.panel, 0.35));
                }
            }
        }
        if k % 12 == 0 {
            ui.scene.hline(c.x, c.right(), y + v.row_h, t.grid_bar);
        } else if v.row_h >= 8.0 {
            ui.scene.hline(c.x, c.right(), y + v.row_h, t.grid_step);
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
        if black {
            // The neighbouring white keys continue behind the black one,
            // so the keyboard reads as one instrument rather than strips.
            ui.scene.rect(
                Rect {
                    x: r.x,
                    y,
                    w: r.w - 1.0,
                    h: v.row_h,
                },
                0.0,
                t.key_white,
            );
        }
        ui.scene.rect_ex(
            key_r.shrink2(0.0, 0.5),
            [0.0, 1.5, 1.5, 0.0],
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
                Color::hex(0x4C4F54),
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
    ui.describe(
        id,
        peel_ui::Role::Canvas,
        &format!("Piano roll, {}", ch.name),
        &format!("{} notes", notes.len()),
    );
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
            ws.roll.focus_note = Some(n.id);
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
            st.origin_expr = crate::note_expr::gather(pattern, st.origin.iter().map(|x| x.id));
            ws.roll.note_len = n.length;
            if !edge {
                s.live_note(ch.id, n.key, n.velocity, true);
                s.live_note(ch.id, n.key, 0.0, false);
            }
        } else if ws.roll.tool == Tool::Draw || ws.roll.tool == Tool::Paint {
            // Create a note (or a stamped chord) and keep dragging it.
            let start = Time::from_f64(beat, 960).floor_to(snap);
            let root = match ws.roll.scale {
                Some(sc) => sc.snap(key),
                None => key,
            };
            let keys = crate::harmony::chord_keys(ws.roll.chord, root, ws.roll.scale);
            let notes: Vec<Note> = keys
                .iter()
                .map(|&k| Note::new(ch.id, start, ws.roll.note_len, k, 0.8))
                .collect();
            let Some(anchor) = notes.first().copied() else {
                return;
            };
            let label = if notes.len() > 1 {
                "Add chord"
            } else {
                "Add note"
            };
            let added = notes.clone();
            s.doc.edit_gesture(st.gesture, label, |p| {
                if let Some(pt) = p.pattern_mut(pid) {
                    pt.notes.extend(added);
                    pt.sort_notes();
                }
            });
            ws.roll.selection = notes.iter().map(|n| n.id).collect();
            ws.roll.focus_note = Some(anchor.id);
            st.kind = DragKind::Move;
            st.anchor = anchor.id;
            st.last_key = anchor.key;
            st.origin = notes.clone();
            for n in &notes {
                s.live_note(ch.id, n.key, 0.8, true);
            }
            for n in &notes {
                s.live_note(ch.id, n.key, 0.0, false);
            }
            // Lengthen the pattern if the note lands past its end.
            if anchor.end() > pattern.length {
                let bars = anchor.end().ceil_to(Time::beats(4));
                s.doc.edit_gesture(st.gesture, label, |p| {
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
                crate::note_expr::prune(pt);
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
                        let mut dk = v.key(p.y).clamp(0, 127) - st.start_key;
                        if let Some(sc) = ws.roll.scale {
                            dk = sc.snap(anchor.key as i32 + dk) - anchor.key as i32;
                        }
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
                        let origin_expr = st.origin_expr.clone();
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
                                // Shortening trims a curve; lengthening
                                // again in the same drag brings it back.
                                crate::note_expr::insert(pt, origin_expr);
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
                                crate::note_expr::prune(pt);
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
            1.5,
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
                w: 1.5,
                h: (nr.h - 4.0).max(1.0),
            },
            0.0,
            ink_on(fill).alpha(0.3),
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

/// The lane under the notes: the lane picker in the label column, then
/// velocity bars or the chosen expression curve.
#[allow(clippy::too_many_arguments)]
fn lower_lane(
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
    let label = Rect {
        x: r.x,
        w: KEYS_W,
        ..r
    };
    ui.scene.rect(r, 0.0, t.panel_alt);
    ui.scene.rect(label, 0.0, t.panel);
    ui.scene.vline(label.right() - 0.5, r.y, r.bottom(), t.line);
    lane_picker(ui, label, ws);
    if ws.roll.lane == Lane::Velocity {
        velocity_lane(ui, r, v, notes, ch, s, ws, pid);
    } else {
        expression_lane(ui, r, v, notes, ch, s, ws, pid);
    }
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
    let lane = Rect {
        x: v.canvas.x,
        w: v.canvas.w,
        ..r
    };
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

#[allow(clippy::too_many_arguments)]
fn keyboard_shortcuts(
    ui: &mut Ui,
    s: &mut Session,
    ws: &mut Workspace,
    pid: Id,
    ch: &Channel,
    notes: &[Note],
    pattern: &Pattern,
    paste_at: Option<Time>,
    quantize: bool,
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
    // Paste works without a selection.
    if ui.input.take_command("v", false) && !ws.clipboard.is_empty() {
        let first = ws
            .clipboard
            .iter()
            .map(|n| n.start)
            .min()
            .unwrap_or(Time::ZERO);
        let last = ws
            .clipboard
            .iter()
            .map(|n| n.end())
            .max()
            .unwrap_or(Time::ZERO);
        let at = paste_at
            .unwrap_or_else(|| (first + (last - first).ceil_to(ws.roll.snap)).max(Time::ZERO));
        let mut map = HashMap::new();
        let copies: Vec<Note> = ws
            .clipboard
            .iter()
            .map(|n| {
                let id = Id::new();
                map.insert(n.id, id);
                Note {
                    id,
                    channel: ch.id,
                    start: at + (n.start - first),
                    ..*n
                }
            })
            .collect();
        let curves = crate::note_expr::remap(&ws.clipboard_expr, &map);
        let needed = copies.iter().map(|n| n.end()).max().unwrap_or(Time::ZERO);
        let len = pattern.length;
        ws.roll.selection = copies.iter().map(|n| n.id).collect();
        s.doc.edit(format!("Paste {} notes", copies.len()), |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                pt.notes.extend(copies);
                pt.sort_notes();
                if needed > len {
                    pt.length = needed.ceil_to(Time::beats(4));
                }
                crate::note_expr::insert(pt, curves);
            }
        });
        return;
    }
    if quantize {
        let targets: HashSet<Id> = if sel.is_empty() {
            notes.iter().map(|n| n.id).collect()
        } else {
            sel.iter().map(|n| n.id).collect()
        };
        let grid = ws.roll.snap;
        s.doc.edit("Quantize notes", |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                for n in pt.notes.iter_mut().filter(|n| targets.contains(&n.id)) {
                    n.start = n.start.round_to(grid).max(Time::ZERO);
                    n.length = n.length.round_to(grid).max(grid);
                }
                pt.sort_notes();
                crate::note_expr::prune(pt);
            }
        });
    }
    if sel.is_empty() {
        return;
    }
    let copy = ui.input.take_command("c", false);
    let cut = ui.input.take_command("x", false);
    if copy || cut {
        ws.clipboard = sel.clone();
        ws.clipboard_expr = crate::note_expr::gather(pattern, sel.iter().map(|n| n.id));
    }
    if cut {
        let ids: HashSet<Id> = sel.iter().map(|n| n.id).collect();
        s.doc.edit(format!("Cut {} notes", sel.len()), |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                pt.notes.retain(|n| !ids.contains(&n.id));
                crate::note_expr::prune(pt);
            }
        });
        ws.roll.selection.clear();
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
                    crate::note_expr::prune(pt);
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
        let mut map = HashMap::new();
        let copies: Vec<Note> = sel
            .iter()
            .map(|n| {
                let id = Id::new();
                map.insert(n.id, id);
                Note {
                    id,
                    start: n.start + span,
                    ..*n
                }
            })
            .collect();
        let curves = crate::note_expr::remap(
            &crate::note_expr::gather(pattern, sel.iter().map(|n| n.id)),
            &map,
        );
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
                crate::note_expr::insert(pt, curves);
            }
        });
        ws.roll.selection = new_ids;
    }
}

/// The note the expression lanes edit: the only selected note, else the
/// last note clicked while it is still selected (or nothing is), else the
/// earliest selected note.
fn focus_note(ws: &Workspace, notes: &[Note]) -> Option<Note> {
    let pick = |id: Id| notes.iter().find(|n| n.id == id).copied();
    let sel = &ws.roll.selection;
    if sel.len() == 1
        && let Some(n) = sel.iter().next().and_then(|id| pick(*id))
    {
        return Some(n);
    }
    if let Some(n) = ws.roll.focus_note.and_then(pick)
        && (sel.is_empty() || sel.contains(&n.id))
    {
        return Some(n);
    }
    notes.iter().find(|n| sel.contains(&n.id)).copied()
}

/// The lane picker in the label column.
fn lane_picker(ui: &mut Ui, label: Rect, ws: &mut Workspace) {
    let mut sel = Lane::ALL
        .iter()
        .position(|l| *l == ws.roll.lane)
        .unwrap_or(0);
    let names: Vec<&str> = Lane::ALL.iter().map(|l| l.name()).collect();
    if ui.segmented_column(W::new("roll-lane"), label.shrink(2.0), &names, &mut sel) {
        ws.roll.lane = Lane::ALL[sel];
    }
}

#[derive(Clone, Default)]
struct PointDrag {
    note: Option<Id>,
    /// Index of the dragged point in `origin` after insertion.
    index: usize,
    origin: Vec<ExpressionPoint>,
    gesture: u64,
    label: String,
    /// Range the lane had when the drag began, so it does not rescale
    /// under the pointer.
    range: f32,
}

/// Value to lane y and back for one lane.
#[derive(Clone, Copy)]
struct Scale {
    lane: Lane,
    top: f32,
    h: f32,
    range: f32,
}

impl Scale {
    fn y(&self, v: f32) -> f32 {
        match self.lane {
            Lane::Pitch => self.top + self.h * 0.5 * (1.0 - v / self.range),
            _ => self.top + self.h * (1.0 - v),
        }
    }

    fn value(&self, y: f32) -> f32 {
        let u = ((y - self.top) / self.h.max(1.0)).clamp(0.0, 1.0);
        match self.lane {
            Lane::Pitch => {
                let v = (1.0 - 2.0 * u) * self.range;
                // A small dead zone makes zero easy to hit.
                if v.abs() < self.range * 0.02 { 0.0 } else { v }
            }
            _ => 1.0 - u,
        }
    }
}

/// Held-value curve of one note: flat from each point to the next, so the
/// drawing shows exactly what plays (no invented ramps).
#[allow(clippy::too_many_arguments)]
fn draw_steps(
    ui: &mut Ui,
    v: &View,
    sc: &Scale,
    n: &Note,
    points: &[ExpressionPoint],
    color: Color,
    width: f32,
    lead: Color,
) {
    let x0 = v.x(n.start.as_f64());
    let x1 = v.x(n.end().as_f64());
    let x_at = |p: &ExpressionPoint| v.x((n.start + p.offset).as_f64());
    let neutral = sc.y(sc.lane.neutral());
    // Before the first point the note plays the neutral value.
    let first = points.first().map_or(x1, x_at);
    if first > x0 {
        ui.scene
            .line(vec2(x0, neutral), vec2(first, neutral), 1.0, lead);
    }
    let mut prev_y = neutral;
    for (i, p) in points.iter().enumerate() {
        let x = x_at(p);
        let y = sc.y(p.value);
        let next = points.get(i + 1).map_or(x1, x_at);
        if (y - prev_y).abs() > 0.5 {
            ui.scene.line(vec2(x, prev_y), vec2(x, y), width, color);
        }
        ui.scene
            .line(vec2(x, y), vec2(next.max(x), y), width, color);
        prev_y = y;
    }
}

#[allow(clippy::too_many_arguments)]
fn expression_lane(
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
    let lane_kind = ws.roll.lane;
    let lane = Rect {
        x: v.canvas.x,
        w: v.canvas.w,
        ..r
    };
    let inner = lane.shrink2(0.0, 8.0);
    let color = col(ch.color);
    let Some(pattern) = s.project().pattern(pid).cloned() else {
        return;
    };
    let id = W::new("roll-expr");
    let drag_id = id.with("drag");
    let focus = focus_note(ws, notes);
    let focus_points = focus.map_or_else(Vec::new, |n| points(&pattern, n.id, lane_kind));
    let dragging = ui.state::<PointDrag>(drag_id).note.is_some();
    let range = match lane_kind {
        Lane::Pitch if dragging => ui.state::<PointDrag>(drag_id).range,
        Lane::Pitch => ws
            .roll
            .pitch_range
            .unwrap_or_else(|| fit_range(&focus_points)),
        _ => 1.0,
    };
    let sc = Scale {
        lane: lane_kind,
        top: inner.y,
        h: inner.h,
        range,
    };

    // Reference line at the neutral value (centre for pitch and timbre).
    if lane_kind != Lane::Pressure {
        ui.scene.hline(
            lane.x,
            lane.right(),
            sc.y(lane_kind.neutral()).round() + 0.5,
            t.line_strong,
        );
    }

    ui.scene.push_clip(lane);
    // Other notes' curves, faint, for context.
    for n in notes.iter().filter(|n| Some(n.id) != focus.map(|f| f.id)) {
        let pts = points(&pattern, n.id, lane_kind);
        if pts.is_empty() {
            continue;
        }
        let (x0, x1) = (v.x(n.start.as_f64()), v.x(n.end().as_f64()));
        if x1 < lane.x || x0 > lane.right() {
            continue;
        }
        draw_steps(
            ui,
            v,
            &sc,
            n,
            &pts,
            color.alpha(0.35),
            1.0,
            Color::TRANSPARENT,
        );
    }
    // The focus note: its span, its curve and its points.
    let pointer = ui.pointer().filter(|p| lane.contains(*p));
    let mut hot_point = None;
    if let Some(n) = focus {
        let (x0, x1) = (v.x(n.start.as_f64()), v.x(n.end().as_f64()));
        ui.scene.rect(
            Rect {
                x: x0,
                y: lane.y,
                w: (x1 - x0).max(1.0),
                h: lane.h,
            },
            0.0,
            color.alpha(0.07),
        );
        ui.scene
            .vline(x0.round() + 0.5, lane.y, lane.bottom(), color.alpha(0.5));
        ui.scene
            .vline(x1.round() - 0.5, lane.y, lane.bottom(), color.alpha(0.25));
        draw_steps(ui, v, &sc, &n, &focus_points, color, 2.0, color.alpha(0.45));
        hot_point = pointer.and_then(|p| {
            focus_points.iter().position(|pt| {
                let c = vec2(v.x((n.start + pt.offset).as_f64()), sc.y(pt.value));
                (c.x - p.x).abs() <= 6.0 && (c.y - p.y).abs() <= 6.0
            })
        });
        let active = ui.state::<PointDrag>(drag_id).clone();
        // Sparse curves show every handle; a dense recorded take shows the
        // line alone and only the handle under the pointer or being dragged.
        let span = (x1 - x0).max(1.0);
        let sparse = span / focus_points.len().max(1) as f32 >= 10.0;
        for (i, pt) in focus_points.iter().enumerate() {
            let on = hot_point == Some(i) || (active.note == Some(n.id) && active.index == i);
            if !sparse && !on {
                continue;
            }
            let c = vec2(v.x((n.start + pt.offset).as_f64()), sc.y(pt.value));
            ui.scene.circle(
                c,
                if on { 4.5 } else { 3.5 },
                if on { color.lighten(0.3) } else { color },
            );
            ui.scene.circle(c, 1.5, t.panel_alt);
        }
    }
    ui.scene.pop_clip();

    // Top-right: the lane's scale and Clear.
    let mut bar = Rect {
        x: lane.right() - 160.0,
        y: lane.y + 3.0,
        w: 156.0,
        h: 18.0,
    };
    if !focus_points.is_empty() {
        let clear = ui.button(W::new("roll-expr-clear"), bar.cut_right(46.0), "Clear");
        ui.tooltip(
            &clear,
            &format!(
                "Remove this note's {} curve",
                lane_kind.name().to_lowercase()
            ),
        );
        if clear.clicked
            && let Some(n) = focus
        {
            let kind = lane_kind;
            s.doc
                .edit(format!("Clear {}", kind.name().to_lowercase()), |p| {
                    if let Some(pt) = p.pattern_mut(pid) {
                        set_curve(pt, n.id, kind, &[]);
                    }
                });
        }
        bar.cut_right(6.0);
    }
    let scale_text = match lane_kind {
        Lane::Pitch => format!("±{} st", range.round() as i32),
        _ => "0–100%".to_string(),
    };
    ui.text(bar, &scale_text, t.caption, t.text_faint, Align::End);

    // One quiet line when there is nothing to show.
    let empty = match focus {
        None => Some("Select a note to edit its expression".to_string()),
        Some(_) if focus_points.is_empty() => Some(format!(
            "No {} recorded for this note",
            lane_kind.name().to_lowercase()
        )),
        _ => None,
    };
    if let Some(text) = empty {
        ui.text(
            Rect {
                x: lane.x + 8.0,
                y: lane.y + 3.0,
                w: (lane.w - 180.0).max(40.0),
                h: 18.0,
            },
            &text,
            t.caption,
            t.text_faint,
            Align::Start,
        );
    }

    let resp = ui.interact(id, lane);
    ui.describe(
        id,
        peel_ui::Role::Canvas,
        &format!("{} lane", lane_kind.name()),
        &focus.map_or("No note".to_string(), |n| {
            format!("{}, {} points", note_name(n.key), focus_points.len())
        }),
    );
    // Scroll changes the pitch range.
    if lane_kind == Lane::Pitch && resp.hovered && ui.input.scroll.y != 0.0 && !dragging {
        let i = PITCH_RANGES
            .iter()
            .position(|r| (*r - range).abs() < 0.5)
            .unwrap_or(0) as i32;
        let step = if ui.input.scroll.y > 0.0 { -1 } else { 1 };
        let j = (i + step).clamp(0, PITCH_RANGES.len() as i32 - 1) as usize;
        ws.roll.pitch_range = Some(PITCH_RANGES[j]);
        ui.input.scroll = peel_ui::Vec2::ZERO;
    }

    // Which note's span the pointer is over (for picking another note).
    let over_note = |x: f32| -> Option<Note> {
        let beat = v.beat(x);
        let mut under = notes
            .iter()
            .filter(|n| n.start.as_f64() <= beat && beat < n.end().as_f64());
        let first = under.clone().next().copied();
        under
            .find(|n| !points(&pattern, n.id, lane_kind).is_empty())
            .copied()
            .or(first)
    };
    let snap = ws.roll.snap;
    let offset_at = |n: &Note, x: f32, free: bool| -> Time {
        let beat = v.beat(x).max(0.0);
        let abs = Time::from_f64(beat, 960);
        let abs = if free { abs } else { abs.round_to(snap) };
        (abs - n.start).max(Time::ZERO).min(n.length)
    };

    if resp.hovered {
        let in_focus = pointer.zip(focus).is_some_and(|(p, n)| {
            v.beat(p.x) >= n.start.as_f64() && v.beat(p.x) < n.end().as_f64()
        });
        ui.set_cursor(if hot_point.is_some() {
            CursorIcon::Grab
        } else if in_focus {
            CursorIcon::Crosshair
        } else {
            CursorIcon::Default
        });
    }

    if resp.pressed
        && let Some(p) = ui.pointer()
    {
        let free = ui.input.modifiers.alt;
        let gesture = id.0 ^ (ui.time() * 1000.0) as u64;
        let name = lane_kind.name().to_lowercase();
        match (focus, hot_point) {
            (Some(n), Some(i)) => {
                *ui.state::<PointDrag>(drag_id) = PointDrag {
                    note: Some(n.id),
                    index: i,
                    origin: focus_points.clone(),
                    gesture,
                    label: format!("Move {name} point"),
                    range,
                };
            }
            (Some(n), None)
                if v.beat(p.x) >= n.start.as_f64() && v.beat(p.x) < n.end().as_f64() =>
            {
                // Insert a point and keep dragging it.
                let new = ExpressionPoint {
                    offset: offset_at(&n, p.x, free),
                    value: sc.value(p.y),
                };
                let mut origin: Vec<ExpressionPoint> = focus_points
                    .iter()
                    .copied()
                    .filter(|q| q.offset != new.offset)
                    .collect();
                let index = origin.partition_point(|q| q.offset < new.offset);
                origin.insert(index, new);
                let label = format!("Add {name} point");
                let pts = origin.clone();
                s.doc.edit_gesture(gesture, label.clone(), |doc| {
                    if let Some(pt) = doc.pattern_mut(pid) {
                        set_curve(pt, n.id, lane_kind, &pts);
                    }
                });
                *ui.state::<PointDrag>(drag_id) = PointDrag {
                    note: Some(n.id),
                    index,
                    origin,
                    gesture,
                    label,
                    range,
                };
            }
            _ => {
                // Pick the note under the pointer to edit it.
                if let Some(n) = over_note(p.x) {
                    ws.roll.focus_note = Some(n.id);
                    ws.roll.selection = std::iter::once(n.id).collect();
                }
            }
        }
    }

    let st = ui.state::<PointDrag>(drag_id).clone();
    if let Some(note) = st.note {
        if ui.input.down[0] {
            if let (Some(p), Some(n)) = (ui.pointer(), notes.iter().find(|n| n.id == note)) {
                let free = ui.input.modifiers.alt;
                let tick = Time::new(1, 960);
                let lo = if st.index == 0 {
                    Time::ZERO
                } else {
                    st.origin[st.index - 1].offset + tick
                };
                let hi = st
                    .origin
                    .get(st.index + 1)
                    .map_or(n.length, |q| q.offset - tick);
                let offset = offset_at(n, p.x, free).max(lo).min(hi.max(lo));
                let s2 = Scale {
                    range: st.range,
                    ..sc
                };
                let value = s2.value(p.y);
                let mut pts = st.origin.clone();
                pts[st.index] = ExpressionPoint { offset, value };
                s.doc.edit_gesture(st.gesture, st.label.clone(), |doc| {
                    if let Some(pt) = doc.pattern_mut(pid) {
                        set_curve(pt, note, lane_kind, &pts);
                    }
                });
                // Value tag beside the point while it moves.
                let at = vec2(v.x((n.start + offset).as_f64()), s2.y(value));
                let text = lane_kind.format(value);
                ui.with_layer(peel_ui::LayerId::ABOVE, |ui| {
                    let w = 58.0;
                    let tag = Rect {
                        x: (at.x + 8.0).min(lane.right() - w),
                        y: (at.y - 20.0).max(lane.y),
                        w,
                        h: 16.0,
                    };
                    ui.scene.rect(tag, t.radius_s, t.raised);
                    ui.text(tag, &text, t.readout_small, t.text, Align::Center);
                });
                ui.set_cursor(CursorIcon::Grabbing);
            }
        } else {
            *ui.state::<PointDrag>(drag_id) = PointDrag::default();
            s.doc.end_gesture();
        }
    } else if let (Some(i), Some(n), Some(p)) = (hot_point, focus, pointer) {
        // Hover readout for a point.
        let pt = focus_points[i];
        let _ = p;
        let tip = format!(
            "{} at {:.2} beats into the note",
            lane_kind.format(pt.value),
            pt.offset.as_f64()
        );
        let r = Rect {
            x: v.x((n.start + pt.offset).as_f64()) - 6.0,
            y: sc.y(pt.value) - 6.0,
            w: 12.0,
            h: 12.0,
        };
        let hover = peel_ui::Response {
            id: id.with(("pt", i)),
            rect: r,
            hovered: true,
            ..resp
        };
        ui.tooltip(&hover, &tip);
        // Right-click or Delete removes the point.
        let delete = resp.secondary_clicked
            || ui.input.take_named(NamedKey::Delete)
            || ui.input.take_named(NamedKey::Backspace);
        if delete {
            let mut pts = focus_points.clone();
            pts.remove(i);
            let kind = lane_kind;
            s.doc.edit(
                format!("Delete {} point", kind.name().to_lowercase()),
                |doc| {
                    if let Some(pt) = doc.pattern_mut(pid) {
                        set_curve(pt, n.id, kind, &pts);
                    }
                },
            );
        }
    }
}
