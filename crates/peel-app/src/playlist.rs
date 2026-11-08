//! Playlist: the arrangement. Pattern clips are linked instances; editing
//! the pattern changes every instance.

use std::collections::HashMap;

use peel_document::{Clip, ClipContent, Id, Pattern, Time, Track};
use peel_session::{PlayMode, Session};
use peel_ui::Id as W;
use peel_ui::{Align, Color, CursorIcon, MenuItem, Rect, Ui, icons, vec2};

use crate::browser::BrowserItem;
use crate::studio::current_pattern;
use crate::util::{col, format_length, ink_on};
use crate::workspace::{DockTab, Tool, Workspace};

const HEAD_W: f32 = 150.0;
const RULER_H: f32 = 26.0;
const EDGE: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Default)]
enum Drag {
    #[default]
    None,
    Move,
    Resize,
    Paint,
    Erase,
    Seek,
    Marquee,
    TrimStart,
    FadeIn,
    FadeOut,
    AutoPoint,
}

/// Which part of a clip the pointer is over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Body,
    Left,
    Right,
    FadeIn,
    FadeOut,
    /// The envelope area of an automation clip.
    AutoBody,
}

#[derive(Clone, Default)]
struct DragState {
    /// Breakpoint being dragged in an automation clip.
    auto_point: Option<usize>,
    kind: Drag,
    clip: Option<Clip>,
    /// Every clip being moved, as it was when the drag started.
    origin: Vec<Clip>,
    start_lane: i32,
    marquee_from: (f64, f32),
    start_beat: f64,
    gesture: u64,
    last_paint: Option<(Id, Time)>,
}

struct View {
    lanes: Rect,
    zoom_x: f32,
    scroll: f32,
    lane_h: f32,
    scroll_y: f32,
}

impl View {
    fn x(&self, beat: f64) -> f32 {
        self.lanes.x + (beat as f32 - self.scroll) * self.zoom_x
    }

    fn beat(&self, x: f32) -> f64 {
        ((x - self.lanes.x) / self.zoom_x + self.scroll) as f64
    }

    fn lane_y(&self, i: usize) -> f32 {
        self.lanes.y + i as f32 * self.lane_h - self.scroll_y
    }

    fn lane_at(&self, y: f32) -> i32 {
        ((y - self.lanes.y + self.scroll_y) / self.lane_h).floor() as i32
    }
}

pub fn draw(ui: &mut Ui, r: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    ui.scene.rect(r, t.radius_l, t.panel);
    let mut body = r;
    let head = body.cut_top(38.0);
    header(ui, head, s, ws);
    body = body.shrink2(8.0, 0.0);
    body.cut_bottom(8.0);

    let arr = s.project().arrangement().clone();
    let mut top = body.cut_top(RULER_H);
    let heads = body.cut_left(HEAD_W);
    top.cut_left(HEAD_W);
    let lanes_r = body;

    // Navigation.
    let nav = Rect {
        x: heads.x,
        y: top.y,
        w: lanes_r.right() - heads.x,
        h: lanes_r.bottom() - top.y,
    };
    if ui.pointer_in(nav) {
        let sc = ui.input.scroll;
        let m = ui.input.modifiers;
        if sc != peel_ui::Vec2::ZERO || ui.input.zoom != 0.0 {
            if m.command() || ui.input.zoom != 0.0 {
                let p = ui.pointer().unwrap_or_default();
                let before = (p.x - lanes_r.x) / ws.playlist.zoom_x + ws.playlist.scroll_beats;
                let f = if ui.input.zoom != 0.0 {
                    1.0 + ui.input.zoom
                } else {
                    (1.0f32 + sc.y * 0.004).clamp(0.5, 2.0)
                };
                ws.playlist.zoom_x = (ws.playlist.zoom_x * f).clamp(3.0, 200.0);
                ws.playlist.scroll_beats = before - (p.x - lanes_r.x) / ws.playlist.zoom_x;
            } else if m.alt {
                ws.playlist.lane_h = (ws.playlist.lane_h * (1.0 + sc.y * 0.004)).clamp(28.0, 140.0);
            } else {
                let (dx, dy) = if m.shift && sc.x == 0.0 {
                    (sc.y, 0.0)
                } else {
                    (sc.x, sc.y)
                };
                ws.playlist.scroll_beats -= dx / ws.playlist.zoom_x;
                ws.playlist.scroll_y -= dy;
            }
            ui.input.scroll = peel_ui::Vec2::ZERO;
            ui.input.zoom = 0.0;
        }
    }
    ws.playlist.scroll_beats = ws.playlist.scroll_beats.max(0.0);
    let content_h = (arr.tracks.len() as f32 + 1.0) * ws.playlist.lane_h;
    ws.playlist.scroll_y = ws
        .playlist
        .scroll_y
        .clamp(0.0, (content_h - lanes_r.h).max(0.0));
    let v = View {
        lanes: lanes_r,
        zoom_x: ws.playlist.zoom_x,
        scroll: ws.playlist.scroll_beats,
        lane_h: ws.playlist.lane_h,
        scroll_y: ws.playlist.scroll_y,
    };

    lanes_bg(ui, &v, &arr.tracks, s);
    ruler(ui, top, &v, s);
    track_heads(ui, heads, &v, &arr.tracks, s);
    clips(ui, &v, &arr.tracks, &arr.clips, s, ws);

    // Patterns and samples dragged from the browser, and files from the OS.
    let accepts = |i: &BrowserItem| matches!(i, BrowserItem::Pattern(_) | BrowserItem::Sample(_));
    if ui.drop_hover::<BrowserItem>(lanes_r).is_some_and(accepts)
        && let Some((item, p)) = ui.take_drop::<BrowserItem>(lanes_r)
    {
        let lane = v.lane_at(p.y);
        let beat = Time::from_f64(v.beat(p.x).max(0.0), 960).floor_to(ws.playlist.snap);
        if let Some(track) = arr.tracks.get(lane.max(0) as usize).map(|t| t.id) {
            match item {
                BrowserItem::Pattern(pid) => place_clip(s, track, beat, pid),
                BrowserItem::Sample(path) => {
                    if let Err(e) = s.add_audio_clip(track, beat, &path) {
                        s.notify(format!("Could not add {}: {e}", path.display()), true);
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(p) = ui.pointer().filter(|p| lanes_r.contains(*p)) {
        let files = ui.take_dropped_files(lanes_r);
        let lane = v.lane_at(p.y).max(0) as usize;
        let beat = Time::from_f64(v.beat(p.x).max(0.0), 960).floor_to(ws.playlist.snap);
        for (i, path) in files
            .iter()
            .filter(|f| peel_media::is_audio_file(f))
            .enumerate()
        {
            if let Some(track) = arr.tracks.get(lane + i).map(|t| t.id)
                && let Err(e) = s.add_audio_clip(track, beat, path)
            {
                s.notify(format!("Could not add {}: {e}", path.display()), true);
            }
        }
    }

    if arr.clips.is_empty() {
        let hint = Rect {
            x: lanes_r.x,
            y: lanes_r.y + 4.0,
            w: lanes_r.w,
            h: ws.playlist.lane_h * 2.0,
        };
        ui.text(
            hint,
            "Click a lane to paint the selected pattern",
            t.body,
            t.text_faint,
            Align::Center,
        );
    }

    // Playhead.
    if s.mode() == PlayMode::Song {
        let x = v.x(s.position());
        if x >= lanes_r.x && x <= lanes_r.right() {
            let full = Rect {
                y: top.y,
                h: lanes_r.bottom() - top.y,
                ..lanes_r
            };
            let c = if s.is_playing() {
                t.accent
            } else {
                t.accent.alpha(0.55)
            };
            ui.with_layer(peel_ui::LayerId::ABOVE, |ui| {
                ui.scene.push_clip(full);
                ui.scene.rect(
                    Rect {
                        x: x - 1.0,
                        y: top.y,
                        w: 2.0,
                        h: full.h,
                    },
                    1.0,
                    c,
                );
                ui.scene.triangle(
                    Rect {
                        x: x - 7.0,
                        y: top.y + RULER_H - 10.0,
                        w: 14.0,
                        h: 10.0,
                    },
                    peel_ui::Dir::Down,
                    0.5,
                    c,
                );
                ui.scene.pop_clip();
            });
            // Follow playback.
            if s.is_playing() && x > lanes_r.right() - 40.0 {
                ws.playlist.scroll_beats = s.position() as f32 - 4.0;
            }
        }
    }
}

fn header(ui: &mut Ui, r: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let mut row = r.shrink2(14.0, 6.0);
    let mut title = row.cut_left(110.0);
    ui.icon(title.cut_left(18.0), icons::COLUMNS, 15.0, t.accent);
    title.cut_left(6.0);
    ui.text(title, "Playlist", t.label, t.text, Align::Start);
    for (tool, icon, tip) in [
        (Tool::Draw, icons::PENCIL, "Draw"),
        (Tool::Paint, icons::BRUSH, "Paint"),
        (Tool::Select, icons::CURSOR, "Select"),
        (Tool::Erase, icons::ERASER, "Erase"),
    ] {
        let b = ui.icon_button(
            W::new(("pl-tool", tip)),
            row.cut_left(30.0),
            icon,
            ws.playlist.tool == tool,
            None,
        );
        ui.tooltip(&b, tip);
        if b.clicked {
            ws.playlist.tool = tool;
        }
        row.cut_left(2.0);
    }
    row.cut_left(10.0);
    let snap_r = row.cut_left(78.0);
    let sresp = ui.interact(W::new("pl-snap"), snap_r);
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
        &format_length(ws.playlist.snap),
        t.label,
        t.text,
        Align::Center,
    );
    ui.tooltip(&sresp, "Grid");
    if sresp.clicked {
        ui.open_popup(W::new("pl-snap-menu"), snap_r);
    }
    let snaps = [
        Time::beats(4),
        Time::beats(1),
        Time::new(1, 2),
        Time::new(1, 4),
    ];
    let items: Vec<MenuItem> = snaps
        .iter()
        .map(|x| MenuItem::new(format_length(*x)).checked(*x == ws.playlist.snap))
        .collect();
    if let Some(i) = ui.menu(
        W::new("pl-snap-menu"),
        vec2(snap_r.x, snap_r.bottom() + 4.0),
        &items,
    ) {
        ws.playlist.snap = snaps[i];
    }
    // Selected pattern chip (what painting places).
    let pid = current_pattern(s);
    if let Some(p) = s.project().pattern(pid).cloned() {
        let w = ui.measure(&p.name, t.label).x + 46.0;
        let chip = row.cut_right(w.min(220.0));
        ui.scene.rect(chip, t.radius_m, col(p.color).alpha(0.18));
        let mut cr = chip.shrink2(10.0, 0.0);
        ui.icon(cr.cut_left(16.0), icons::BRUSH, 13.0, col(p.color));
        cr.cut_left(6.0);
        let name = ui.fit_text(&p.name, t.label, cr.w);
        ui.text(cr, &name, t.label, t.text, Align::Start);
    }
}

fn lanes_bg(ui: &mut Ui, v: &View, tracks: &[Track], s: &Session) {
    let t = ui.theme.clone();
    let c = v.lanes;
    ui.scene.push_clip(c);
    ui.scene.rect(c, 0.0, t.panel_alt);
    for (i, _) in tracks.iter().enumerate() {
        let y = v.lane_y(i);
        if i % 2 == 1 {
            ui.scene.rect(
                Rect {
                    x: c.x,
                    y,
                    w: c.w,
                    h: v.lane_h,
                },
                0.0,
                Color::WHITE.alpha(0.012),
            );
        }
        ui.scene.hline(c.x, c.right(), y + v.lane_h, t.line);
    }
    let sig = s.project().time_signature;
    let bar = sig.bar_length().as_f64();
    let step = if v.zoom_x >= 14.0 { 1.0 } else { bar };
    let mut b = (v.scroll as f64 / step).floor() * step;
    while v.x(b) <= c.right() {
        let x = v.x(b);
        let is_bar = (b / bar).fract().abs() < 1e-6;
        let bar_idx = (b / bar).round() as i64;
        let color = if is_bar && bar_idx % 4 == 0 {
            t.grid_bar.alpha(1.6)
        } else if is_bar {
            t.grid_bar
        } else {
            t.grid_step
        };
        ui.scene.vline(x, c.y, c.bottom(), color);
        b += step;
    }
    if let Some((a, e)) = s.project().arrangement().loop_range {
        let (x0, x1) = (v.x(a.as_f64()), v.x(e.as_f64()));
        ui.scene.rect(
            Rect {
                x: x0,
                y: c.y,
                w: x1 - x0,
                h: c.h,
            },
            0.0,
            t.accent.alpha(0.03),
        );
    }
    ui.scene.pop_clip();
}

fn ruler(ui: &mut Ui, r: Rect, v: &View, s: &mut Session) {
    let t = ui.theme.clone();
    ui.scene.push_clip(r);
    ui.scene.rect(r, 0.0, t.panel);
    let bar = s.project().time_signature.bar_length().as_f64();
    let every = if v.zoom_x * bar as f32 >= 36.0 {
        1.0
    } else if v.zoom_x * bar as f32 >= 12.0 {
        4.0
    } else {
        8.0
    };
    let mut b = (v.scroll as f64 / (bar * every)).floor() * bar * every;
    while v.x(b) < r.right() {
        let x = v.x(b);
        ui.scene
            .vline(x, r.y + 12.0, r.bottom(), t.text_faint.alpha(0.5));
        ui.text_at(
            vec2(x + 4.0, r.y + 3.0),
            &format!("{}", (b / bar).round() as i64 + 1),
            t.caption,
            t.text_dim,
        );
        b += bar * every;
    }
    if let Some((a, e)) = s.project().arrangement().loop_range {
        let (x0, x1) = (v.x(a.as_f64()), v.x(e.as_f64()));
        ui.scene.rect(
            Rect {
                x: x0,
                y: r.bottom() - 4.0,
                w: x1 - x0,
                h: 4.0,
            },
            2.0,
            t.accent.alpha(0.7),
        );
    }
    ui.scene.pop_clip();
    let id = W::new("pl-ruler");
    let resp = ui.interact(id, r);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    ui.tooltip(&resp, "Click to move the playhead. Drag to set the loop.");
    let st_id = W::new("pl-ruler-drag");
    if resp.pressed
        && let Some(p) = ui.pointer()
    {
        let beat = Time::from_f64(v.beat(p.x).max(0.0), 960).floor_to(Time::beats(1));
        ui.state::<DragState>(st_id).start_beat = beat.as_f64();
        ui.state::<DragState>(st_id).kind = Drag::Seek;
        if s.mode() != PlayMode::Song {
            s.set_mode(PlayMode::Song);
        }
        s.seek(beat.as_f64());
    }
    if resp.dragging
        && let Some(p) = ui.pointer()
        && ui.state::<DragState>(st_id).kind == Drag::Seek
    {
        let a = ui.state::<DragState>(st_id).start_beat;
        let b = Time::from_f64(v.beat(p.x).max(0.0), 960)
            .round_to(bar_time(s))
            .as_f64();
        if (b - a).abs() >= bar_time(s).as_f64() {
            let (lo, hi) = (a.min(b), a.max(b));
            let lo_t = Time::from_f64(lo, 960).floor_to(bar_time(s));
            let hi_t = Time::from_f64(hi, 960).round_to(bar_time(s));
            if hi_t > lo_t {
                s.doc.edit_gesture(id.0, "Set loop", |p| {
                    p.arrangement_mut().loop_range = Some((lo_t, hi_t))
                });
            }
        }
    }
    if resp.released {
        s.doc.end_gesture();
    }
    if resp.double_clicked {
        s.doc
            .edit("Clear loop", |p| p.arrangement_mut().loop_range = None);
    }
}

fn bar_time(s: &Session) -> Time {
    s.project().time_signature.bar_length()
}

fn track_heads(ui: &mut Ui, r: Rect, v: &View, tracks: &[Track], s: &mut Session) {
    let t = ui.theme.clone();
    ui.scene.push_clip(r);
    for (i, tr) in tracks.iter().enumerate() {
        let y = v.lane_y(i);
        let row = Rect {
            x: r.x,
            y: y + 2.0,
            w: r.w - 6.0,
            h: v.lane_h - 4.0,
        };
        if row.bottom() < r.y || row.y > r.bottom() {
            continue;
        }
        ui.scene.rect(row, t.radius_m, t.raised.alpha(0.55));
        let stripe = tr.color.map(col).unwrap_or_else(|| {
            let first = s
                .project()
                .arrangement()
                .clips
                .iter()
                .find(|c| c.track == tr.id)
                .and_then(|c| match c.content {
                    ClipContent::Pattern { pattern } => {
                        s.project().pattern(pattern).map(|p| col(p.color))
                    }
                    _ => None,
                });
            first.unwrap_or(t.text_faint)
        });
        ui.scene.rect_ex(
            Rect { w: 3.0, ..row },
            [t.radius_m, 0.0, 0.0, t.radius_m],
            stripe,
            0.0,
            Color::TRANSPARENT,
        );
        let mut inner = row.shrink2(10.0, 0.0);
        let led_r = inner.cut_right(20.0);
        let led = ui.led(W::new(("pl-led", tr.id)), led_r, !tr.muted, t.play);
        ui.tooltip(&led, if tr.muted { "Unmute lane" } else { "Mute lane" });
        if led.clicked {
            let tid = tr.id;
            s.doc
                .edit(if tr.muted { "Unmute lane" } else { "Mute lane" }, |p| {
                    if let Some(x) = p.arrangement_mut().tracks.iter_mut().find(|x| x.id == tid) {
                        x.muted = !x.muted;
                    }
                });
        }
        let name = ui.fit_text(&tr.name, t.label, inner.w - 4.0);
        ui.text(
            inner,
            &name,
            t.label,
            if tr.muted {
                t.text_faint
            } else {
                t.text.mix(t.text_dim, 0.2)
            },
            Align::Start,
        );
        let name_resp = ui.interact(W::new(("pl-lane-name", tr.id)), inner);
        ui.tooltip(&name_resp, "Double-click to rename");
        if name_resp.double_clicked {
            *ui.state::<Option<Id>>(W::new("pl-rename-lane-target")) = Some(tr.id);
            crate::rename::open(ui, W::new("pl-rename-lane"), row, &tr.name);
        }
    }
    if let Some(name) = crate::rename::show(ui, W::new("pl-rename-lane"))
        && let Some(lane) = *ui.state::<Option<Id>>(W::new("pl-rename-lane-target"))
    {
        s.doc.edit("Rename lane", |p| {
            if let Some(x) = p.arrangement_mut().tracks.iter_mut().find(|x| x.id == lane) {
                x.name = name;
            }
        });
    }
    // Add lane.
    let y = v.lane_y(tracks.len());
    let add = Rect {
        x: r.x,
        y: y + 6.0,
        w: 36.0,
        h: 28.0,
    };
    let b = ui.icon_button(W::new("pl-add-lane"), add, icons::PLUS, false, None);
    ui.tooltip(&b, "Add lane");
    if b.clicked {
        let n = tracks.len() + 1;
        s.doc.edit("Add lane", |p| {
            p.arrangement_mut().tracks.push(Track {
                id: Id::new(),
                name: format!("Track {n}"),
                color: None,
                muted: false,
            })
        });
    }
    ui.scene.pop_clip();
}

fn clip_rect(v: &View, lane: usize, c: &Clip) -> Rect {
    let x0 = v.x(c.start.as_f64());
    let x1 = v.x(c.end().as_f64());
    Rect {
        x: x0 + 1.0,
        y: v.lane_y(lane) + 2.0,
        w: (x1 - x0 - 2.0).max(4.0),
        h: v.lane_h - 4.0,
    }
}

fn clips(
    ui: &mut Ui,
    v: &View,
    tracks: &[Track],
    clips: &[Clip],
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let lanes = v.lanes;
    let lane_of: HashMap<Id, usize> = tracks
        .iter()
        .enumerate()
        .map(|(i, tr)| (tr.id, i))
        .collect();
    let patterns: HashMap<Id, Pattern> = s
        .project()
        .patterns
        .iter()
        .map(|p| (p.id, (**p).clone()))
        .collect();
    let channel_order: HashMap<Id, usize> = s
        .project()
        .channels
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id, i))
        .collect();
    let id = W::new("pl-canvas");
    let resp = ui.interact(id, lanes);
    let drag_id = W::new("pl-drag");
    let pointer = ui.pointer();

    let hit = pointer.filter(|p| lanes.contains(*p)).and_then(|p| {
        clips.iter().rev().find_map(|c| {
            let lane = *lane_of.get(&c.track)?;
            let cr = clip_rect(v, lane, c);
            if !cr.contains(p) {
                return None;
            }
            let edge = EDGE.min(cr.w * 0.3);
            let in_header = p.y < cr.y + 16.0;
            let part = match (&c.content, in_header) {
                (
                    ClipContent::Audio {
                        fade_in, fade_out, ..
                    },
                    true,
                ) if (p.x - (v.x((c.start + *fade_in).as_f64()))).abs() <= 6.0 => Part::FadeIn,
                (ClipContent::Audio { fade_out, .. }, true)
                    if (p.x - (v.x((c.end() - *fade_out).as_f64()))).abs() <= 6.0 =>
                {
                    Part::FadeOut
                }
                (ClipContent::Automation { .. }, false) => Part::AutoBody,
                _ if p.x > cr.right() - edge => Part::Right,
                _ if p.x < cr.x + edge => Part::Left,
                _ => Part::Body,
            };
            Some((c.clone(), part))
        })
    });
    if resp.hovered && ui.state::<DragState>(drag_id).kind == Drag::None {
        match (ws.playlist.tool, &hit) {
            (Tool::Erase, _) => ui.set_cursor(CursorIcon::Crosshair),
            (_, Some((_, Part::Body))) => ui.set_cursor(CursorIcon::Grab),
            (_, Some((_, Part::AutoBody))) => ui.set_cursor(CursorIcon::Crosshair),
            (_, Some(_)) => ui.set_cursor(CursorIcon::EwResize),
            _ => {}
        }
    }

    let snap = ws.playlist.snap;
    let pid = current_pattern(s);
    if resp.pressed
        && let Some(p) = pointer
    {
        let beat = v.beat(p.x).max(0.0);
        let lane = v.lane_at(p.y);
        let mut st = DragState {
            start_beat: beat,
            start_lane: lane,
            gesture: id.0 ^ (ui.time() * 1000.0) as u64,
            ..Default::default()
        };
        match (ws.playlist.tool, hit.clone()) {
            (Tool::Erase, _) => st.kind = Drag::Erase,
            (tool, Some((c, Part::AutoBody))) if tool != Tool::Erase => {
                ws.playlist.selection.clear();
                ws.playlist.selection.insert(c.id);
                if let Some(lane) = lane_of.get(&c.track)
                    && let ClipContent::Automation { points, .. } = &c.content
                {
                    let body = auto_body(clip_rect(v, *lane, &c));
                    let near = points.iter().position(|pt| {
                        let (x, y) = auto_point_xy(v, &c, body, pt);
                        (x - p.x).abs() <= 7.0 && (y - p.y).abs() <= 7.0
                    });
                    let index = match near {
                        Some(i) => i,
                        None => {
                            let grid = if ui.input.modifiers.alt {
                                Time::new(1, 960)
                            } else {
                                auto_snap(snap)
                            };
                            let pos =
                                Time::from_f64(beat - c.start.as_f64() + c.offset.as_f64(), 960)
                                    .round_to(grid)
                                    .max(Time::ZERO);
                            let value = ((body.bottom() - p.y) / body.h).clamp(0.0, 1.0);
                            let at = points.partition_point(|x| x.position <= pos);
                            let cid = c.id;
                            s.doc.edit_gesture(st.gesture, "Add automation point", |p| {
                                if let Some(cl) =
                                    p.arrangement_mut().clips.iter_mut().find(|x| x.id == cid)
                                    && let ClipContent::Automation { points, .. } = &mut cl.content
                                {
                                    points.insert(
                                        at,
                                        peel_document::AutomationPoint {
                                            position: pos,
                                            value,
                                        },
                                    );
                                }
                            });
                            at
                        }
                    };
                    st.kind = Drag::AutoPoint;
                    st.clip = Some(c);
                    st.auto_point = Some(index);
                }
            }
            (_, Some((c, part))) => {
                let edge = part != Part::Body;
                if !ws.playlist.selection.contains(&c.id) {
                    if !ui.input.modifiers.shift {
                        ws.playlist.selection.clear();
                    }
                    ws.playlist.selection.insert(c.id);
                }
                st.start_lane = lane;
                let selected: Vec<Clip> = s
                    .project()
                    .arrangement()
                    .clips
                    .iter()
                    .filter(|x| ws.playlist.selection.contains(&x.id))
                    .cloned()
                    .collect();
                // Alt-drag duplicates the selection and moves the copies.
                if ui.input.modifiers.alt && !edge && !resp.double_clicked {
                    let copies: Vec<Clip> = selected
                        .iter()
                        .map(|x| Clip {
                            id: Id::new(),
                            ..x.clone()
                        })
                        .collect();
                    let added = copies.clone();
                    s.doc.edit_gesture(st.gesture, "Duplicate clips", |p| {
                        p.arrangement_mut().clips.extend(added)
                    });
                    ws.playlist.selection = copies.iter().map(|x| x.id).collect();
                    let anchor = copies
                        .iter()
                        .zip(&selected)
                        .find(|(_, o)| o.id == c.id)
                        .map(|(n, _)| n.clone());
                    st.kind = Drag::Move;
                    st.clip = anchor;
                    st.origin = copies;
                    *ui.state::<DragState>(drag_id) = st;
                    return;
                }
                st.origin = selected;
                if resp.double_clicked {
                    if let ClipContent::Pattern { pattern } = c.content {
                        s.select_pattern(pattern);
                        ws.dock = DockTab::Rack;
                    }
                } else {
                    if let ClipContent::Pattern { pattern } = c.content {
                        s.select_pattern(pattern);
                    }
                    st.kind = match part {
                        Part::Body => Drag::Move,
                        Part::Right => Drag::Resize,
                        Part::Left => Drag::TrimStart,
                        Part::FadeIn => Drag::FadeIn,
                        Part::FadeOut => Drag::FadeOut,
                        Part::AutoBody => Drag::Move,
                    };
                    st.clip = Some(c);
                }
            }
            (tool, None) if tool != Tool::Select => {
                if let Some(track) = tracks.get(lane.max(0) as usize)
                    && s.project().pattern(pid).is_some()
                {
                    let start = Time::from_f64(beat, 960).floor_to(snap);
                    let new = place_clip_gesture(s, track.id, start, pid, st.gesture);
                    if tool == Tool::Paint {
                        st.kind = Drag::Paint;
                        st.last_paint = Some((track.id, start));
                    } else {
                        st.kind = Drag::Move;
                        st.clip = new;
                    }
                }
            }
            (Tool::Select, None) => {
                if !ui.input.modifiers.shift {
                    ws.playlist.selection.clear();
                }
                st.kind = Drag::Marquee;
                st.marquee_from = (beat, p.y);
            }
            _ => ws.playlist.selection.clear(),
        }
        *ui.state::<DragState>(drag_id) = st;
    }

    // Right click opens the clip menu.
    let menu_id = W::new("pl-clip-menu");
    let mut deleted_point = false;
    if resp.secondary_clicked
        && let Some((c, Part::AutoBody)) = &hit
        && let Some(lane) = lane_of.get(&c.track)
        && let ClipContent::Automation { points, .. } = &c.content
        && let Some(p) = pointer
    {
        let body = auto_body(clip_rect(v, *lane, c));
        if let Some(i) = points.iter().position(|pt| {
            let (x, y) = auto_point_xy(v, c, body, pt);
            (x - p.x).abs() <= 7.0 && (y - p.y).abs() <= 7.0
        }) {
            let cid = c.id;
            s.doc.edit("Delete automation point", |p| {
                if let Some(cl) = p.arrangement_mut().clips.iter_mut().find(|x| x.id == cid)
                    && let ClipContent::Automation { points, .. } = &mut cl.content
                    && points.len() > 1
                {
                    points.remove(i);
                }
            });
            deleted_point = true;
        }
    }
    if resp.secondary_clicked
        && !deleted_point
        && let Some((c, _)) = &hit
    {
        ws.playlist.selection.clear();
        ws.playlist.selection.insert(c.id);
        *ui.state::<Option<Clip>>(menu_id) = Some(c.clone());
        ui.open_popup(
            menu_id,
            Rect::from_center(pointer.unwrap_or_default(), 1.0, 1.0),
        );
    }
    if let Some(c) = ui.state::<Option<Clip>>(menu_id).clone() {
        let at = ui.popup_rect().map(|r| vec2(r.x, r.y)).unwrap_or_default();
        let shared = match c.content {
            ClipContent::Pattern { pattern } => s.project().pattern_usage(pattern) > 1,
            _ => false,
        };
        let items = [
            MenuItem::new("Open pattern"),
            MenuItem::new("Make unique").enabled(shared),
            MenuItem::new("Rename pattern"),
            MenuItem::new(if c.muted { "Unmute clip" } else { "Mute clip" }),
            MenuItem::Separator,
            MenuItem::new("Delete clip").shortcut("Del"),
        ];
        if let ClipContent::Automation { .. } = c.content {
            let items = [
                MenuItem::new(if c.muted { "Unmute clip" } else { "Mute clip" }),
                MenuItem::Separator,
                MenuItem::new("Delete clip").shortcut("Del"),
            ];
            let cid = c.id;
            match ui.menu(menu_id, at, &items) {
                Some(0) => s
                    .doc
                    .edit(if c.muted { "Unmute clip" } else { "Mute clip" }, |p| {
                        if let Some(x) = p.arrangement_mut().clips.iter_mut().find(|x| x.id == cid)
                        {
                            x.muted = !x.muted;
                        }
                    }),
                Some(2) => s.doc.edit("Delete clip", |p| {
                    p.arrangement_mut().clips.retain(|x| x.id != cid)
                }),
                _ => {}
            }
        } else if let ClipContent::Audio { reversed, .. } = c.content {
            let audio_items = [
                MenuItem::new(if reversed { "Play forwards" } else { "Reverse" }),
                MenuItem::new(if c.muted { "Unmute clip" } else { "Mute clip" }),
                MenuItem::Separator,
                MenuItem::new("Delete clip").shortcut("Del"),
            ];
            let cid = c.id;
            match ui.menu(menu_id, at, &audio_items) {
                Some(0) => s.doc.edit(
                    if reversed {
                        "Play clip forwards"
                    } else {
                        "Reverse clip"
                    },
                    |p| {
                        if let Some(x) = p.arrangement_mut().clips.iter_mut().find(|x| x.id == cid)
                            && let ClipContent::Audio { reversed, .. } = &mut x.content
                        {
                            *reversed = !*reversed;
                        }
                    },
                ),
                Some(1) => s
                    .doc
                    .edit(if c.muted { "Unmute clip" } else { "Mute clip" }, |p| {
                        if let Some(x) = p.arrangement_mut().clips.iter_mut().find(|x| x.id == cid)
                        {
                            x.muted = !x.muted;
                        }
                    }),
                Some(3) => s.doc.edit("Delete clip", |p| {
                    p.arrangement_mut().clips.retain(|x| x.id != cid)
                }),
                _ => {}
            }
        } else if let Some(choice) = ui.menu(menu_id, at, &items) {
            let ClipContent::Pattern { pattern } = c.content else {
                return;
            };
            match choice {
                0 => {
                    s.select_pattern(pattern);
                    ws.dock = DockTab::Rack;
                }
                1 => {
                    if let Some(new) = make_unique(s, c.id, pattern) {
                        s.select_pattern(new);
                    }
                }
                2 => {
                    let name = s
                        .project()
                        .pattern(pattern)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    let lane = lane_of.get(&c.track).copied().unwrap_or(0);
                    let anchor = clip_rect(v, lane, &c);
                    *ui.state::<Option<Id>>(W::new("pl-rename-target")) = Some(pattern);
                    crate::rename::open(ui, W::new("pl-rename-pattern"), anchor, &name);
                }
                3 => {
                    let cid = c.id;
                    s.doc
                        .edit(if c.muted { "Unmute clip" } else { "Mute clip" }, |p| {
                            if let Some(x) =
                                p.arrangement_mut().clips.iter_mut().find(|x| x.id == cid)
                            {
                                x.muted = !x.muted;
                            }
                        });
                }
                5 => {
                    let cid = c.id;
                    s.doc.edit("Delete clip", |p| {
                        p.arrangement_mut().clips.retain(|x| x.id != cid)
                    });
                }
                _ => {}
            }
        }
        if !ui.is_popup_open(menu_id) {
            *ui.state::<Option<Clip>>(menu_id) = None;
        }
    }
    if let Some(name) = crate::rename::show(ui, W::new("pl-rename-pattern"))
        && let Some(pid) = *ui.state::<Option<Id>>(W::new("pl-rename-target"))
    {
        s.doc.edit("Rename pattern", |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                pt.name = name;
            }
        });
    }

    let st = ui.state::<DragState>(drag_id).clone();
    if st.kind != Drag::None && ui.input.down[0] {
        if let Some(p) = pointer {
            let beat = v.beat(p.x);
            match st.kind {
                Drag::Move => {
                    if let Some(orig) = &st.clip {
                        let raw = orig.start.as_f64() + (beat - st.start_beat);
                        let start = Time::from_f64(raw.max(0.0), 960)
                            .round_to(snap)
                            .max(Time::ZERO);
                        let mut dt = start - orig.start;
                        let mut dl = v.lane_at(p.y) - st.start_lane;
                        let origin = if st.origin.is_empty() {
                            vec![orig.clone()]
                        } else {
                            st.origin.clone()
                        };
                        // Keep the whole group inside the arrangement.
                        let min_start = origin.iter().map(|c| c.start).min().unwrap_or(Time::ZERO);
                        if min_start + dt < Time::ZERO {
                            dt = -min_start;
                        }
                        let lanes: Vec<i32> = origin
                            .iter()
                            .filter_map(|c| tracks.iter().position(|t| t.id == c.track))
                            .map(|i| i as i32)
                            .collect();
                        let (lo, hi) = (
                            lanes.iter().copied().min().unwrap_or(0),
                            lanes.iter().copied().max().unwrap_or(0),
                        );
                        dl = dl.clamp(-lo, tracks.len() as i32 - 1 - hi);
                        let moves: Vec<(Id, Time, Id)> = origin
                            .iter()
                            .filter_map(|c| {
                                let li = tracks.iter().position(|t| t.id == c.track)? as i32 + dl;
                                Some((c.id, c.start + dt, tracks[li as usize].id))
                            })
                            .collect();
                        let label = if moves.len() == 1 {
                            "Move clip".to_string()
                        } else {
                            format!("Move {} clips", moves.len())
                        };
                        s.doc.edit_gesture(st.gesture, label, |p| {
                            for (id, start, track) in &moves {
                                if let Some(c) =
                                    p.arrangement_mut().clips.iter_mut().find(|c| c.id == *id)
                                {
                                    c.start = *start;
                                    c.track = *track;
                                }
                            }
                        });
                        ui.set_cursor(CursorIcon::Grabbing);
                    }
                }
                Drag::AutoPoint => {
                    if let (Some(orig), Some(i)) = (&st.clip, st.auto_point)
                        && let Some(lane) = lane_of.get(&orig.track)
                        && let ClipContent::Automation { points, .. } = &orig.content
                    {
                        let body = auto_body(clip_rect(v, *lane, orig));
                        let grid = if ui.input.modifiers.alt {
                            Time::new(1, 960)
                        } else {
                            auto_snap(snap)
                        };
                        let lo = if i > 0 {
                            points[i - 1].position
                        } else {
                            Time::ZERO
                        };
                        let hi = points
                            .get(i + 1)
                            .map(|x| x.position)
                            .unwrap_or(orig.offset + orig.length);
                        let pos =
                            Time::from_f64(beat - orig.start.as_f64() + orig.offset.as_f64(), 960)
                                .round_to(grid)
                                .clamp(lo, hi);
                        let value = ((body.bottom() - p.y) / body.h).clamp(0.0, 1.0);
                        let cid = orig.id;
                        s.doc
                            .edit_gesture(st.gesture, "Move automation point", |p| {
                                if let Some(cl) =
                                    p.arrangement_mut().clips.iter_mut().find(|x| x.id == cid)
                                    && let ClipContent::Automation { points, .. } = &mut cl.content
                                    && let Some(pt) = points.get_mut(i)
                                {
                                    pt.position = pos;
                                    pt.value = value;
                                }
                            });
                    }
                }
                Drag::TrimStart => {
                    if let Some(orig) = &st.clip {
                        let raw = orig.start.as_f64() + (beat - st.start_beat);
                        let lo = orig.start - orig.offset;
                        let hi = orig.end() - snap;
                        let new_start = Time::from_f64(raw.max(0.0), 960)
                            .round_to(snap)
                            .clamp(lo.max(Time::ZERO), hi);
                        let d = new_start - orig.start;
                        let cid = orig.id;
                        let (o_off, o_len) = (orig.offset, orig.length);
                        s.doc.edit_gesture(st.gesture, "Trim clip", |p| {
                            if let Some(c) =
                                p.arrangement_mut().clips.iter_mut().find(|c| c.id == cid)
                            {
                                c.start = new_start;
                                c.offset = o_off + d;
                                c.length = o_len - d;
                            }
                        });
                        ui.set_cursor(CursorIcon::EwResize);
                    }
                }
                Drag::FadeIn | Drag::FadeOut => {
                    if let Some(orig) = &st.clip {
                        let fine = if ui.input.modifiers.alt {
                            Time::new(1, 64)
                        } else {
                            Time::new(1, 16)
                        };
                        let cid = orig.id;
                        let fade_in_kind = st.kind == Drag::FadeIn;
                        let raw = if fade_in_kind {
                            beat - orig.start.as_f64()
                        } else {
                            orig.end().as_f64() - beat
                        };
                        let value = Time::from_f64(raw.max(0.0), 960)
                            .round_to(fine)
                            .max(Time::ZERO);
                        s.doc.edit_gesture(
                            st.gesture,
                            if fade_in_kind {
                                "Change fade in"
                            } else {
                                "Change fade out"
                            },
                            |p| {
                                if let Some(c) =
                                    p.arrangement_mut().clips.iter_mut().find(|c| c.id == cid)
                                {
                                    let len = c.length;
                                    if let ClipContent::Audio {
                                        fade_in, fade_out, ..
                                    } = &mut c.content
                                    {
                                        if fade_in_kind {
                                            *fade_in = value.min(len - *fade_out).max(Time::ZERO);
                                        } else {
                                            *fade_out = value.min(len - *fade_in).max(Time::ZERO);
                                        }
                                    }
                                }
                            },
                        );
                        ui.set_cursor(CursorIcon::EwResize);
                    }
                }
                Drag::Marquee => {
                    let (b0, y0) = st.marquee_from;
                    let x0 = v.x(b0);
                    let mr = Rect::from_min_max(
                        vec2(x0.min(p.x), y0.min(p.y)),
                        vec2(x0.max(p.x), y0.max(p.y)),
                    );
                    ui.with_layer(peel_ui::LayerId::ABOVE, |ui| {
                        ui.scene.push_clip(lanes);
                        ui.scene
                            .rect_stroke(mr, 2.0, t.focus.alpha(0.12), 1.0, t.focus.alpha(0.8));
                        ui.scene.pop_clip();
                    });
                    let inside: Vec<Id> = s
                        .project()
                        .arrangement()
                        .clips
                        .iter()
                        .filter(|c| {
                            lane_of
                                .get(&c.track)
                                .is_some_and(|&l| clip_rect(v, l, c).intersects(&mr))
                        })
                        .map(|c| c.id)
                        .collect();
                    if !ui.input.modifiers.shift {
                        ws.playlist.selection.clear();
                    }
                    ws.playlist.selection.extend(inside);
                }
                Drag::Resize => {
                    if let Some(orig) = &st.clip {
                        let raw_end = orig.end().as_f64() + (beat - st.start_beat);
                        let end = Time::from_f64(raw_end, 960).round_to(snap);
                        let len = (end - orig.start).max(snap);
                        let cid = orig.id;
                        s.doc.edit_gesture(st.gesture, "Resize clip", |p| {
                            if let Some(c) =
                                p.arrangement_mut().clips.iter_mut().find(|c| c.id == cid)
                            {
                                c.length = len;
                            }
                        });
                        ui.set_cursor(CursorIcon::EwResize);
                    }
                }
                Drag::Paint => {
                    let lane = v.lane_at(p.y);
                    if let (Some(track), Some(pat)) = (
                        tracks.get(lane.max(0) as usize),
                        s.project().pattern(pid).cloned(),
                    ) {
                        let len = pat.length;
                        let start = Time::from_f64(beat.max(0.0), 960).floor_to(len.max(snap));
                        let occupied = s.project().arrangement().clips.iter().any(|c| {
                            c.track == track.id && c.start < start + len && c.end() > start
                        });
                        if !occupied && st.last_paint != Some((track.id, start)) {
                            place_clip_gesture(s, track.id, start, pid, st.gesture);
                            ui.state::<DragState>(drag_id).last_paint = Some((track.id, start));
                        }
                    }
                }
                Drag::Erase => {
                    if let Some((c, _)) = &hit {
                        let cid = c.id;
                        s.doc.edit_gesture(st.gesture, "Delete clips", |p| {
                            p.arrangement_mut().clips.retain(|x| x.id != cid)
                        });
                    }
                }
                _ => {}
            }
        }
    } else if st.kind != Drag::None {
        ui.state::<DragState>(drag_id).kind = Drag::None;
        s.doc.end_gesture();
    }

    // Keyboard: delete selected clips.
    if !ui.text_focus() && !ws.playlist.selection.is_empty() && ui.pointer_in(lanes) {
        if ui.input.take_named(peel_ui::NamedKey::Delete)
            || ui.input.take_named(peel_ui::NamedKey::Backspace)
        {
            let sel = ws.playlist.selection.clone();
            s.doc.edit("Delete clips", |p| {
                p.arrangement_mut().clips.retain(|c| !sel.contains(&c.id))
            });
            ws.playlist.selection.clear();
        }
        if ui.input.take_command("d", false) {
            let sel = ws.playlist.selection.clone();
            let copies: Vec<Clip> = s
                .project()
                .arrangement()
                .clips
                .iter()
                .filter(|c| sel.contains(&c.id))
                .map(|c| Clip {
                    id: Id::new(),
                    start: c.end(),
                    ..c.clone()
                })
                .collect();
            ws.playlist.selection = copies.iter().map(|c| c.id).collect();
            s.doc.edit("Duplicate clips", |p| {
                p.arrangement_mut().clips.extend(copies)
            });
        }
    }

    // Draw clips.
    let fresh = s.project().arrangement().clips.clone();
    ui.scene.push_clip(lanes);
    for c in &fresh {
        let Some(&lane) = lane_of.get(&c.track) else {
            continue;
        };
        let cr = clip_rect(v, lane, c);
        if !cr.intersects(&lanes) {
            continue;
        }
        let lane_muted_any = tracks.get(lane).is_some_and(|t| t.muted) || c.muted;
        let pattern = match c.content {
            ClipContent::Pattern { pattern } => pattern,
            ClipContent::Automation {
                ref target,
                ref points,
            } => {
                let sel = ws.playlist.selection.contains(&c.id);
                let hovered = hit.as_ref().is_some_and(|(h, _)| h.id == c.id);
                draw_automation_clip(
                    ui,
                    v,
                    c,
                    cr,
                    target,
                    points,
                    sel,
                    hovered,
                    lane_muted_any,
                    s,
                );
                continue;
            }
            ClipContent::Audio {
                asset, reversed, ..
            } => {
                let sel = ws.playlist.selection.contains(&c.id);
                let hovered = hit.as_ref().is_some_and(|(h, _)| h.id == c.id);
                draw_audio_clip(
                    ui,
                    v,
                    c,
                    cr,
                    asset,
                    reversed,
                    sel,
                    hovered,
                    lane_muted_any,
                    s,
                );
                continue;
            }
        };
        let Some(pat) = patterns.get(&pattern) else {
            continue;
        };
        let pc = col(pat.color);
        let sel = ws.playlist.selection.contains(&c.id);
        let hovered = hit.as_ref().is_some_and(|(h, _)| h.id == c.id);
        let lane_muted = tracks.get(lane).is_some_and(|t| t.muted) || c.muted;
        let body = t.panel.mix(pc, if lane_muted { 0.12 } else { 0.30 });
        ui.scene.rect(
            cr,
            t.radius_s,
            if hovered { body.lighten(0.04) } else { body },
        );
        let header = Rect {
            h: 16.0f32.min(cr.h * 0.4),
            ..cr
        };
        ui.scene.rect_ex(
            header,
            [t.radius_s, t.radius_s, 0.0, 0.0],
            pc.alpha(if lane_muted { 0.35 } else { 0.85 }),
            0.0,
            Color::TRANSPARENT,
        );
        if cr.w > 26.0 {
            ui.with_clip(header, |ui| {
                ui.text(
                    header.shrink2(6.0, 0.0),
                    &pat.name,
                    t.caption.weight(peel_ui::FontWeight::SemiBold),
                    ink_on(pc),
                    Align::Start,
                );
            });
        }
        // Notes preview from real pattern data, repeated per loop.
        let notes_r = Rect {
            y: header.bottom() + 3.0,
            h: cr.bottom() - header.bottom() - 6.0,
            ..cr.shrink2(3.0, 0.0)
        };
        if notes_r.h > 6.0 && !pat.notes.is_empty() {
            // One preview row per (channel, key): melodic parts read as
            // pitch, drum parts as separate lanes.
            let mut rows: Vec<(usize, u8)> = pat
                .notes
                .iter()
                .map(|n| {
                    (
                        channel_order.get(&n.channel).copied().unwrap_or(usize::MAX),
                        n.key,
                    )
                })
                .collect();
            rows.sort_unstable();
            rows.dedup();
            let melodic = rows.len() > 12 || rows.windows(2).all(|w| w[0].0 == w[1].0);
            let (lo, hi) = pat
                .notes
                .iter()
                .fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
            let span = if melodic {
                (hi as f32 - lo as f32 + 1.0).max(6.0)
            } else {
                rows.len() as f32
            };
            let row_of = |n: &peel_document::Note| -> f32 {
                if melodic {
                    (n.key - lo) as f32
                } else {
                    let key = (
                        channel_order.get(&n.channel).copied().unwrap_or(usize::MAX),
                        n.key,
                    );
                    (rows.len() - 1 - rows.binary_search(&key).unwrap_or(0)) as f32
                }
            };
            let l = pat.length.as_f64();
            ui.scene.push_clip(cr);
            let mut k = 0i64;
            loop {
                let origin = c.start.as_f64() - c.offset.as_f64() + k as f64 * l;
                if origin >= c.end().as_f64() || k > 512 {
                    break;
                }
                if k > 0 {
                    let x = v.x(origin);
                    ui.scene
                        .vline(x, notes_r.y, notes_r.bottom(), Color::WHITE.alpha(0.08));
                }
                for n in &pat.notes {
                    let b0 = origin + n.start.as_f64();
                    if b0 < c.start.as_f64() || b0 >= c.end().as_f64() {
                        continue;
                    }
                    let x0 = v.x(b0);
                    let x1 = v.x((b0 + n.length.as_f64()).min(c.end().as_f64()));
                    let y = notes_r.bottom() - notes_r.h * (row_of(n) + 0.5) / span;
                    let nh = (notes_r.h / span * 0.7).clamp(1.5, 5.0);
                    let alpha = if lane_muted {
                        0.3
                    } else {
                        0.55 + 0.4 * n.velocity
                    };
                    ui.scene.rect(
                        Rect {
                            x: x0,
                            y: y - nh * 0.5,
                            w: (x1 - x0 - 1.0).max(1.5),
                            h: nh,
                        },
                        1.0,
                        pc.lighten(0.3).alpha(alpha),
                    );
                }
                k += 1;
            }
            ui.scene.pop_clip();
        }
        if sel {
            ui.scene
                .outline(cr, t.radius_s, 1.5, Color::WHITE.alpha(0.85));
        }
        if hovered && !sel {
            ui.scene
                .outline(cr, t.radius_s, 1.0, pc.lighten(0.3).alpha(0.6));
        }
    }
    ui.scene.pop_clip();
}

fn place_clip(s: &mut Session, track: Id, start: Time, pattern: Id) {
    let gesture = W::new(("place", track, start.as_f64().to_bits())).0;
    place_clip_gesture(s, track, start, pattern, gesture);
    s.doc.end_gesture();
}

fn place_clip_gesture(
    s: &mut Session,
    track: Id,
    start: Time,
    pattern: Id,
    gesture: u64,
) -> Option<Clip> {
    let len = s.project().pattern(pattern)?.length;
    let clip = Clip {
        id: Id::new(),
        track,
        start,
        length: len,
        offset: Time::ZERO,
        muted: false,
        content: ClipContent::Pattern { pattern },
    };
    let out = clip.clone();
    s.doc.edit_gesture(gesture, "Place pattern", |p| {
        p.arrangement_mut().clips.push(clip)
    });
    Some(out)
}

/// Gives one clip its own copy of a shared pattern. Returns the new
/// pattern id.
pub fn make_unique(s: &mut Session, clip: Id, pattern: Id) -> Option<Id> {
    let src = s.project().pattern(pattern)?.as_ref().clone();
    let existing: Vec<String> = s
        .project()
        .patterns
        .iter()
        .map(|p| p.name.clone())
        .collect();
    let mut n = 2;
    let base = src
        .name
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end()
        .to_string();
    let mut name = format!("{base} {n}");
    while existing.contains(&name) {
        n += 1;
        name = format!("{base} {n}");
    }
    let mut copy = src;
    copy.id = Id::new();
    copy.name = name;
    for note in &mut copy.notes {
        note.id = Id::new();
    }
    let new_id = copy.id;
    s.doc.edit("Make unique", |p| {
        p.patterns.push(std::sync::Arc::new(copy));
        if let Some(c) = p.arrangement_mut().clips.iter_mut().find(|c| c.id == clip) {
            c.content = ClipContent::Pattern { pattern: new_id };
        }
    });
    Some(new_id)
}

/// Draws an audio clip with its waveform from the cached peak pyramid.
#[allow(clippy::too_many_arguments)]
fn draw_audio_clip(
    ui: &mut Ui,
    v: &View,
    c: &Clip,
    cr: Rect,
    asset: Id,
    reversed: bool,
    sel: bool,
    hovered: bool,
    muted: bool,
    s: &Session,
) {
    let t = ui.theme.clone();
    let wave = Color::hex(0x56B3C2);
    let body = t.panel.mix(wave, if muted { 0.10 } else { 0.22 });
    ui.scene.rect(
        cr,
        t.radius_s,
        if hovered { body.lighten(0.04) } else { body },
    );
    let header = Rect {
        h: 16.0f32.min(cr.h * 0.4),
        ..cr
    };
    ui.scene.rect_ex(
        header,
        [t.radius_s, t.radius_s, 0.0, 0.0],
        wave.alpha(if muted { 0.35 } else { 0.8 }),
        0.0,
        Color::TRANSPARENT,
    );
    let name = s
        .project()
        .asset(asset)
        .map(|a| a.name.clone())
        .unwrap_or_default();
    if cr.w > 26.0 {
        let label = if reversed {
            format!("{name}  (reversed)")
        } else {
            name
        };
        ui.with_clip(header, |ui| {
            ui.text(
                header.shrink2(6.0, 0.0),
                &label,
                t.caption.weight(peel_ui::FontWeight::SemiBold),
                ink_on(wave),
                Align::Start,
            );
        });
    }
    let body_r = Rect {
        y: header.bottom() + 2.0,
        h: cr.bottom() - header.bottom() - 4.0,
        ..cr
    };
    if body_r.h < 4.0 {
        return;
    }
    let Some(peaks) = s.audio_peaks(asset) else {
        if s.audio_loading(asset) {
            ui.text(body_r, "Loading", t.caption, t.text_faint, Align::Center);
        } else {
            ui.text(body_r, "Missing file", t.caption, t.record, Align::Center);
        }
        return;
    };
    // Map each pixel column to source frames through the tempo map.
    let tempo = peel_time::TempoMap::from_segments(&s.project().tempo);
    let rate = s.sample_rate() as f64;
    let source0 = peel_session::clip_source_start_seconds(&tempo, c);
    let clip_sec0 = tempo.beat_to_seconds(c.start.as_f64());
    let clip_sec1 = tempo.beat_to_seconds(c.end().as_f64());
    let frames = peaks.frames() as f64;
    let x0 = cr.x.max(v.lanes.x).floor();
    let x1 = cr.right().min(v.lanes.right()).ceil();
    let cols = (x1 - x0).max(1.0) as usize;
    let mid = body_r.center().y;
    let amp = body_r.h * 0.48;
    ui.scene.push_clip(cr);
    let frame_at = |x: f32| -> f64 {
        let sec = tempo.beat_to_seconds(v.beat(x));
        let into = if reversed {
            (clip_sec1 - sec) + source0
        } else {
            (sec - clip_sec0) + source0
        };
        (into * rate).clamp(0.0, frames)
    };
    let (mut prev_end, color) = (
        frame_at(x0),
        wave.lighten(0.3).alpha(if muted { 0.35 } else { 0.9 }),
    );
    for i in 0..cols {
        let x = x0 + i as f32;
        let f_end = frame_at(x + 1.0);
        let (a, b) = if prev_end <= f_end {
            (prev_end, f_end)
        } else {
            (f_end, prev_end)
        };
        prev_end = f_end;
        if b - a < 1.0 || a >= frames {
            continue;
        }
        if let Some((lo, hi)) = peaks
            .query(a as usize, (b as usize).max(a as usize + 1), 1)
            .next()
        {
            let y0 = mid - hi.clamp(-1.0, 1.0) * amp;
            let y1 = mid - lo.clamp(-1.0, 1.0) * amp;
            ui.scene.rect(
                Rect {
                    x,
                    y: y0,
                    w: 1.0,
                    h: (y1 - y0).max(1.0),
                },
                0.0,
                color,
            );
        }
    }
    // Fades: shade the attenuated part, draw the ramp and its handle.
    if let ClipContent::Audio {
        fade_in, fade_out, ..
    } = c.content
    {
        let fi = v.x((c.start + fade_in).as_f64()) - cr.x;
        let fo = cr.right() - v.x((c.end() - fade_out).as_f64());
        let shade = t.bg.alpha(0.55);
        if fi > 1.0 {
            for i in 0..fi as usize {
                let g = i as f32 / fi;
                ui.scene.rect(
                    Rect {
                        x: cr.x + i as f32,
                        y: body_r.y,
                        w: 1.0,
                        h: body_r.h,
                    },
                    0.0,
                    shade.alpha(0.55 * (1.0 - g)),
                );
            }
            ui.scene.line(
                vec2(cr.x, body_r.bottom()),
                vec2(cr.x + fi, body_r.y),
                1.5,
                Color::WHITE.alpha(0.7),
            );
        }
        if fo > 1.0 {
            for i in 0..fo as usize {
                let g = i as f32 / fo;
                ui.scene.rect(
                    Rect {
                        x: cr.right() - i as f32 - 1.0,
                        y: body_r.y,
                        w: 1.0,
                        h: body_r.h,
                    },
                    0.0,
                    shade.alpha(0.55 * (1.0 - g)),
                );
            }
            ui.scene.line(
                vec2(cr.right() - fo, body_r.y),
                vec2(cr.right(), body_r.bottom()),
                1.5,
                Color::WHITE.alpha(0.7),
            );
        }
        if hovered || sel || fi > 1.0 || fo > 1.0 {
            for x in [cr.x + fi, cr.right() - fo] {
                ui.scene.rect(
                    Rect::from_center(
                        vec2(x.clamp(cr.x + 4.0, cr.right() - 4.0), header.bottom() - 1.0),
                        7.0,
                        7.0,
                    ),
                    1.5,
                    Color::WHITE.alpha(0.9),
                );
            }
        }
    }
    ui.scene.pop_clip();
    if sel {
        ui.scene
            .outline(cr, t.radius_s, 1.5, Color::WHITE.alpha(0.85));
    }
}

/// The envelope area of an automation clip (below its header).
fn auto_body(cr: Rect) -> Rect {
    let header = 16.0f32.min(cr.h * 0.4);
    Rect {
        y: cr.y + header + 4.0,
        h: (cr.h - header - 8.0).max(4.0),
        ..cr
    }
}

fn auto_point_xy(
    v: &View,
    c: &Clip,
    body: Rect,
    pt: &peel_document::AutomationPoint,
) -> (f32, f32) {
    let beat = (c.start + pt.position - c.offset).as_f64();
    (v.x(beat), body.bottom() - pt.value * body.h)
}

/// Breakpoints snap four times finer than the playlist grid.
fn auto_snap(snap: Time) -> Time {
    snap.div_int(4).max(Time::new(1, 16))
}

/// A readable name for what an automation clip controls.
pub fn automation_label(s: &Session, target: &peel_document::AutomationTarget) -> (String, Color) {
    use peel_document::AutomationTarget as T;
    let p = s.project();
    match target {
        T::Param { device, param } => {
            let pname = peel_session::find_device(p, *device)
                .and_then(|d| peel_devices::descriptor(&d.type_id))
                .and_then(|d| d.param_by_key(param))
                .map_or(param.as_str(), |pd| pd.name);
            if let Some(ch) = p.channels.iter().find(|c| c.device.id == *device) {
                return (format!("{} · {pname}", ch.name), col(ch.color));
            }
            if let Some(t) = p
                .mixer
                .tracks
                .iter()
                .find(|t| t.inserts.iter().any(|d| d.id == *device))
            {
                let dev = t
                    .inserts
                    .iter()
                    .find(|d| d.id == *device)
                    .and_then(|d| peel_devices::descriptor(&d.type_id))
                    .map_or("", |d| d.name);
                return (format!("{} {dev} · {pname}", t.name), col(t.color));
            }
            (format!("Missing · {pname}"), Color::hex(0x8E9AAB))
        }
        T::TrackGain { track } | T::TrackPan { track } => {
            let what = if matches!(target, T::TrackGain { .. }) {
                "Volume"
            } else {
                "Pan"
            };
            match p.mixer.track(*track) {
                Some(t) => (format!("{} · {what}", t.name), col(t.color)),
                None => (format!("Missing · {what}"), Color::hex(0x8E9AAB)),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_automation_clip(
    ui: &mut Ui,
    v: &View,
    c: &Clip,
    cr: Rect,
    target: &peel_document::AutomationTarget,
    points: &[peel_document::AutomationPoint],
    sel: bool,
    hovered: bool,
    muted: bool,
    s: &Session,
) {
    let t = ui.theme.clone();
    let (label, color) = automation_label(s, target);
    ui.scene.rect(
        cr,
        t.radius_s,
        t.panel.mix(color, if muted { 0.06 } else { 0.12 }),
    );
    let header = Rect {
        h: 16.0f32.min(cr.h * 0.4),
        ..cr
    };
    ui.scene.rect_ex(
        header,
        [t.radius_s, t.radius_s, 0.0, 0.0],
        color.alpha(if muted { 0.3 } else { 0.55 }),
        0.0,
        Color::TRANSPARENT,
    );
    if cr.w > 26.0 {
        ui.with_clip(header, |ui| {
            let mut hr = header.shrink2(6.0, 0.0);
            ui.icon(hr.cut_left(12.0), icons::WAVE_SINE, 11.0, t.text);
            hr.cut_left(4.0);
            ui.text(
                hr,
                &label,
                t.caption.weight(peel_ui::FontWeight::SemiBold),
                t.text,
                Align::Start,
            );
        });
    }
    let body = auto_body(cr);
    ui.scene.push_clip(cr);
    ui.scene
        .hline(cr.x, cr.right(), body.center().y, t.grid_step);
    if !points.is_empty() {
        let line = color.lighten(0.25).alpha(if muted { 0.4 } else { 1.0 });
        let mut pts: Vec<peel_ui::Vec2> = points
            .iter()
            .map(|pt| {
                let (x, y) = auto_point_xy(v, c, body, pt);
                vec2(x, y)
            })
            .collect();
        // Hold the first and last values to the clip edges.
        if let (Some(f), Some(l)) = (pts.first().copied(), pts.last().copied()) {
            pts.insert(0, vec2(cr.x, f.y));
            pts.push(vec2(cr.right(), l.y));
        }
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let top = a.y.min(b.y);
            let _ = top;
            ui.scene.rect(
                Rect {
                    x: a.x,
                    y: a.y.max(b.y),
                    w: (b.x - a.x).max(0.0),
                    h: body.bottom() - a.y.max(b.y),
                },
                0.0,
                color.alpha(0.10),
            );
        }
        ui.scene.polyline(&pts, 1.75, line);
        for pt in points {
            let (x, y) = auto_point_xy(v, c, body, pt);
            let r = if hovered || sel { 4.0 } else { 3.0 };
            ui.scene.circle_stroke(vec2(x, y), r, line, 1.5, t.bg);
        }
    }
    ui.scene.pop_clip();
    if sel {
        ui.scene
            .outline(cr, t.radius_s, 1.5, Color::WHITE.alpha(0.85));
    }
}

/// Adds an automation clip for `target` on the first empty lane (or a new
/// one), spanning the arrangement, flat at `value`. Returns the clip id.
pub fn create_automation(
    s: &mut Session,
    ws: &mut Workspace,
    target: peel_document::AutomationTarget,
    value: f32,
) -> Id {
    let arr = s.project().arrangement().clone();
    let bar = s.project().time_signature.bar_length();
    let length = arr.end().ceil_to(bar).max(bar.mul_int(8));
    let free_lane = arr
        .tracks
        .iter()
        .find(|t| !arr.clips.iter().any(|c| c.track == t.id))
        .map(|t| t.id);
    let (label, _) = automation_label(s, &target);
    let clip_id = Id::new();
    let value = value.clamp(0.0, 1.0);
    s.doc.edit(format!("Automate {label}"), |p| {
        let lane = match free_lane {
            Some(l) => l,
            None => {
                let t = Track {
                    id: Id::new(),
                    name: "Automation".into(),
                    color: None,
                    muted: false,
                };
                let id = t.id;
                p.arrangement_mut().tracks.push(t);
                id
            }
        };
        p.arrangement_mut().clips.push(Clip {
            id: clip_id,
            track: lane,
            start: Time::ZERO,
            length,
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::Automation {
                target,
                points: vec![
                    peel_document::AutomationPoint {
                        position: Time::ZERO,
                        value,
                    },
                    peel_document::AutomationPoint {
                        position: length,
                        value,
                    },
                ],
            },
        });
    });
    ws.playlist.selection.clear();
    ws.playlist.selection.insert(clip_id);
    s.notify(
        format!("Created automation for {label}. Click the clip to add points."),
        false,
    );
    clip_id
}
