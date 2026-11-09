//! Channel Rack: one row per instrument with a step grid for the selected
//! pattern. Steps are ordinary notes, so the Piano Roll edits the same data.

use std::path::Path;

use peel_document::{Channel, Id, Note, PALETTE, Time};
use peel_session::{PlayMode, Session, template};
use peel_ui::Id as W;
use peel_ui::{Align, Color, CursorIcon, MenuItem, Rect, Ui, icons, vec2};

use crate::browser::BrowserItem;
use crate::studio::current_pattern;
use crate::util::{SCROLLBAR, col, db_to_fader, fader_to_db, ink_on};
use crate::workspace::{DockTab, Focus, RACK_STEPS, Workspace, rhythm_of};

const ROW_H: f32 = 26.0;
const ROW_GAP: f32 = 2.0;
/// Channel volume and pan knobs.
const KNOB: f32 = 22.0;
const GRAPH_H: f32 = 64.0;
const RULER_H: f32 = 22.0;
/// Width of the mute, name and knob column left of the steps.
const LEAD: f32 = 20.0 + 4.0 + NAME_W + 6.0 + KNOB * 2.0 + 4.0 + 10.0;
const NAME_W: f32 = 124.0;
/// Steps never shrink below this: below it they stop being easy targets,
/// and the grid scrolls instead.
const STEP_MIN_W: f32 = 12.0;
const STEP_MAX_W: f32 = 26.0;
const STEP_GAP: f32 = 2.0;
const BEAT_GAP: f32 = 4.0;
const BAR_GAP: f32 = 8.0;

#[derive(Default)]
struct PaintState {
    active: bool,
    on: bool,
    gesture: u64,
}

/// A note the step grid shows as a hit: the channel's root key and no
/// longer than a beat. Hits between steps show as ticks in their step.
fn is_hit(n: &Note, root: u8) -> bool {
    n.key == root && n.length <= Time::beats(1)
}

/// Steps needed to cover `length`.
fn step_count(length: Time, step: Time) -> usize {
    length
        .ceil_to(step)
        .checked_div(step)
        .map_or(1, |q| q.floor_int())
        .clamp(1, 4096) as usize
}

/// Where each step of the grid sits. Steps group by beat (so triplet steps
/// come in threes) and bars stand further apart.
pub(crate) struct StepLayout {
    pub step: Time,
    /// Left edge of every step, from the grid's left edge.
    pub xs: Vec<f32>,
    /// Width of one step.
    pub w: f32,
    /// Width of the whole grid.
    pub width: f32,
    /// Steps per quarter note.
    per_beat: i64,
    /// Steps per lighter sub-pulse inside a beat (0 for none).
    per_pulse: i64,
}

impl StepLayout {
    pub fn new(step: Time, length: Time, bar: Time, avail: f32) -> StepLayout {
        let count = step_count(length, step);
        let beat = Time::beats(1);
        let gaps: Vec<f32> = (0..count)
            .map(|i| {
                let pos = step.mul_int(i as i64);
                if i == 0 {
                    0.0
                } else if pos.rem_euclid(bar).is_zero() {
                    BAR_GAP
                } else if pos.rem_euclid(beat).is_zero() {
                    BEAT_GAP
                } else {
                    STEP_GAP
                }
            })
            .collect();
        let total_gap: f32 = gaps.iter().sum();
        let w = ((avail - total_gap) / count as f32).clamp(STEP_MIN_W, STEP_MAX_W);
        let mut xs = Vec::with_capacity(count);
        let mut x = 0.0;
        for g in gaps {
            x += g;
            xs.push(x);
            x += w;
        }
        let per_beat = beat
            .checked_div(step)
            .map_or(1, |q| q.floor_int().max(1) as i64);
        let per_pulse = if per_beat > 4 && per_beat % 4 == 0 {
            per_beat / 4
        } else if per_beat > 3 && per_beat % 3 == 0 {
            per_beat / 3
        } else {
            0
        };
        StepLayout {
            step,
            xs,
            w,
            width: x,
            per_beat,
            per_pulse,
        }
    }

    pub fn count(&self) -> usize {
        self.xs.len()
    }

    pub fn pos(&self, i: usize) -> Time {
        self.step.mul_int(i as i64)
    }

    /// The step under `x` (from the grid's left edge), gaps split between
    /// their neighbours.
    pub fn index_at(&self, x: f32) -> Option<usize> {
        if x < -STEP_GAP || x > self.width + STEP_GAP {
            return None;
        }
        let i = self.xs.partition_point(|&l| l <= x);
        let i = i.saturating_sub(1);
        // Past the right half of the gap after step i: the next step.
        match self.xs.get(i + 1) {
            Some(&next) if x >= (self.xs[i] + self.w + next) * 0.5 => Some(i + 1),
            _ => Some(i),
        }
    }

    /// The step a beat position falls in.
    pub fn index_of(&self, pos: Time) -> usize {
        pos.checked_div(self.step)
            .map_or(0, |q| q.floor_int().max(0) as usize)
    }

    /// Beats alternate between two neutral fills so a bar reads in beats.
    fn beat_odd(&self, i: usize) -> bool {
        (i as i64 / self.per_beat) % 2 == 1
    }

    /// The step starts a sub-pulse inside the beat (a sixteenth in a
    /// 1/32 or 1/64 grid, an eighth triplet in a 1/16T grid).
    fn pulse_start(&self, i: usize) -> bool {
        self.per_pulse > 0 && (i as i64 % self.per_beat) % self.per_pulse == 0
    }
}

/// One channel's hits sorted by step: the hit on the step itself, and the
/// offsets of any between it and the next.
struct RowHits {
    on: Vec<Option<Note>>,
    between: Vec<Vec<f32>>,
}

impl RowHits {
    fn new(notes: &[Note], l: &StepLayout) -> RowHits {
        let mut on = vec![None; l.count()];
        let mut between = vec![Vec::new(); l.count()];
        for n in notes {
            let i = l.index_of(n.start);
            if i >= l.count() || n.start.is_negative() {
                continue;
            }
            let rest = n.start - l.pos(i);
            if rest.is_zero() {
                on[i].get_or_insert(*n);
            } else {
                let frac = rest.checked_div(l.step).map_or(0.0, |f| f.as_f64() as f32);
                between[i].push(frac);
            }
        }
        RowHits { on, between }
    }
}

/// The step size that shows every hit in `rows` on a step, coarsest first.
fn step_fitting(rows: &[Vec<Note>]) -> Option<Time> {
    let mut sizes: Vec<Time> = RACK_STEPS.iter().map(|r| r.len).collect();
    sizes.sort_by(|a, b| b.cmp(a));
    sizes.into_iter().find(|&step| {
        rows.iter()
            .flatten()
            .all(|n| n.start.rem_euclid(step).is_zero())
    })
}

pub fn draw(ui: &mut Ui, r: Rect, tools: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let pid = current_pattern(s);
    let Some(pattern) = s.project().pattern(pid).cloned() else {
        return;
    };
    let channels: Vec<Channel> = s.project().channels.clone();
    let bar = s.project().time_signature.bar_length();
    // Channels the step grid shows (all hits on the root key); the rest
    // show a preview that opens the piano roll.
    let rows: Vec<(Vec<Note>, bool)> = channels
        .iter()
        .map(|ch| {
            let notes: Vec<Note> = pattern.notes_for(ch.id).copied().collect();
            let steps = notes.iter().all(|n| is_hit(n, ch.root_key));
            (notes, steps)
        })
        .collect();
    let step_rows: Vec<Vec<Note>> = rows
        .iter()
        .filter(|(_, steps)| *steps)
        .map(|(n, _)| n.clone())
        .collect();
    let fits_now = step_rows
        .iter()
        .flatten()
        .all(|n| n.start.rem_euclid(ws.rack_step).is_zero());
    let hint = (!fits_now).then(|| step_fitting(&step_rows));

    let mut body = r.shrink2(6.0, 4.0);
    // The step ruler stays put above the rows: the step size over the name
    // column, bar numbers over the steps.
    let ruler = body.cut_top(RULER_H);
    // Steps scroll sideways under a fixed name column when they need more
    // room than the dock has.
    let grid_x = body.x + LEAD;
    let grid_w = (body.right() - grid_x - 8.0).max(80.0);
    let layout_for = |step: Time| StepLayout::new(step, pattern.length, bar, grid_w);
    let old_step = ws.rack_step;

    {
        // Velocity graph toggle at the left of the header tools; length
        // and swing take the rest from the right.
        let mut row = tools;
        let toggle = row.cut_left(30.0);
        row.cut_left(6.0);
        header_tools(ui, row, s, pid, &pattern);
        let b = ui.icon_button(
            W::new("rack-graph"),
            toggle,
            icons::EQUALIZER,
            ws.rack_graph,
            None,
        );
        ui.name(&b, "Velocity graph");
        ui.tooltip(&b, "Velocity graph for the selected channel");
        if b.clicked {
            ws.rack_graph = !ws.rack_graph;
        }
    }
    step_size_button(
        ui,
        Rect {
            x: ruler.x + 24.0,
            w: NAME_W,
            ..ruler
        }
        .shrink2(0.0, 1.0),
        ws,
        hint,
    );
    let layout = layout_for(ws.rack_step);
    if ws.rack_step != old_step {
        // Keep the same beat at the left edge.
        let old = layout_for(old_step);
        let first = old.index_at(ws.rack_scroll_x).unwrap_or(0);
        let at = layout.index_of(old.pos(first));
        ws.rack_scroll_x = layout.xs.get(at).copied().unwrap_or(0.0);
    }
    let overflow = (layout.width - grid_w).max(0.0);

    // Sideways: trackpad x or Shift+wheel over the rack.
    if overflow > 0.0 && ui.pointer_in(body) {
        let sc = ui.input.scroll;
        let m = ui.input.modifiers;
        if !m.command() && !m.alt {
            if sc.x != 0.0 {
                ws.rack_scroll_x -= sc.x;
                ui.input.scroll.x = 0.0;
            } else if m.shift && sc.y != 0.0 {
                ws.rack_scroll_x -= sc.y;
                ui.input.scroll = peel_ui::Vec2::ZERO;
            }
        }
    }
    let hbar = (overflow > 0.0).then(|| body.cut_bottom(SCROLLBAR + 2.0));
    // Follow playback a page at a time when the playing step leaves view.
    let playing_step = if s.is_playing() && matches!(s.mode(), PlayMode::Pattern(p) if p == pid) {
        let b = s.position().rem_euclid(pattern.length.as_f64());
        Some((b / layout.step.as_f64()).floor() as usize)
    } else {
        None
    };
    if overflow > 0.0
        && let Some(i) = playing_step
        && let Some(&x) = layout.xs.get(i)
        && (x < ws.rack_scroll_x || x + layout.w > ws.rack_scroll_x + grid_w)
    {
        ws.rack_scroll_x = x;
    }
    ws.rack_scroll_x = ws.rack_scroll_x.clamp(0.0, overflow);
    if let Some(hb) = hbar {
        let mut x = ws.rack_scroll_x;
        ui.scrollbar(
            W::new("rack-hbar"),
            Rect {
                x: grid_x,
                w: grid_w,
                h: SCROLLBAR,
                y: hb.y + 2.0,
            },
            true,
            &mut x,
            grid_w,
            layout.width,
        );
        ws.rack_scroll_x = x.clamp(0.0, overflow);
    }

    let graph_h = if ws.rack_graph {
        GRAPH_H + ROW_GAP
    } else {
        0.0
    };
    let content_h = channels.len() as f32 * (ROW_H + ROW_GAP) + ROW_H + 8.0 + graph_h;
    let mut offset = vec2(0.0, ws.rack_scroll);
    ui.scroll_area(
        W::new("rack-scroll"),
        body,
        vec2(body.w, content_h),
        &mut offset,
    );
    ws.rack_scroll = offset.y;

    // Drops onto the rack add channels (instruments and samples only).
    let accepts = |i: &BrowserItem| match i {
        BrowserItem::Device(id) | BrowserItem::Preset(id, _) => {
            peel_devices::descriptor(id).is_some_and(|d| d.kind == peel_dsp::DeviceKind::Instrument)
        }
        BrowserItem::Sample(_) => true,
        BrowserItem::Pattern(_) => false,
        BrowserItem::Plugin { instrument, .. } => *instrument,
    };
    let hovering_drop = ui.drop_hover::<BrowserItem>(body).is_some_and(accepts);
    if hovering_drop && let Some((item, _)) = ui.take_drop::<BrowserItem>(body) {
        let added = match item {
            BrowserItem::Device(type_id) => add_instrument(s, type_id),
            BrowserItem::Preset(type_id, preset) => add_instrument_preset(s, type_id, Some(preset)),
            BrowserItem::Sample(path) => add_sample_channel(s, &path),
            BrowserItem::Pattern(_) => None,
            BrowserItem::Plugin { type_id, name, .. } => add_plugin_channel(s, &type_id, &name),
        };
        if let Some(id) = added {
            select(ws, id);
        }
    }
    for path in ui.take_dropped_files(body) {
        if peel_media::is_audio_file(&path)
            && let Some(id) = add_sample_channel(s, &path)
        {
            select(ws, id);
        }
    }

    let grid = StepGrid {
        layout: &layout,
        view_x: grid_x,
        view_w: grid_w,
        scroll: ws.rack_scroll_x,
    };
    step_ruler(ui, ruler, &grid, bar);

    ui.scene.push_clip(body);
    let paint_id = W::new("rack-paint");
    let mut y = body.y - offset.y;
    for (ch, (notes, as_steps)) in channels.iter().zip(&rows) {
        let row = Rect {
            x: body.x,
            y,
            w: body.w,
            h: ROW_H,
        };
        y += ROW_H + ROW_GAP;
        let selected = ws.selected_channel == Some(ch.id);
        let graph_row = (selected && ws.rack_graph).then(|| {
            let g = Rect {
                x: body.x,
                y,
                w: body.w,
                h: GRAPH_H,
            };
            y += GRAPH_H + ROW_GAP;
            g
        });
        if row.bottom() < body.y || row.y > body.bottom() {
            continue;
        }
        if selected {
            // A flat band behind the whole row, edge to edge.
            ui.scene
                .rect(row.shrink2(-6.0, -1.0), 0.0, t.raised.alpha(0.6));
        }
        let mut cells = row;
        // Mute LED.
        let led_r = cells.cut_left(20.0);
        let led = ui.led(W::new(("rack-led", ch.id)), led_r, !ch.muted, t.play);
        ui.tooltip(&led, if ch.muted { "Unmute" } else { "Mute" });
        if led.clicked {
            let cid = ch.id;
            let label = if ch.muted {
                "Unmute channel"
            } else {
                "Mute channel"
            };
            s.doc.edit(label, |p| {
                if let Some(c) = p.channel_mut(cid) {
                    c.muted = !c.muted;
                }
            });
        }
        cells.cut_left(4.0);
        channel_button(
            ui,
            cells.cut_left(NAME_W).shrink2(0.0, 1.0),
            ch,
            selected,
            s,
            ws,
        );
        cells.cut_left(6.0);
        // Volume and pan.
        let mut vol = db_to_fader(ch.gain_db);
        let vr = Rect::from_center(cells.cut_left(KNOB).center(), KNOB, KNOB);
        let vresp = ui.knob(
            W::new(("rack-vol", ch.id)),
            vr,
            &mut vol,
            0.8,
            col(ch.color),
            false,
        );
        ui.name(&vresp, &format!("{} volume", ch.name));
        ui.tooltip(
            &vresp,
            &format!("Volume {}", crate::util::format_db(ch.gain_db)),
        );
        if vresp.changed {
            let cid = ch.id;
            s.doc
                .edit_gesture(vresp.id.0, "Change channel volume", |p| {
                    if let Some(c) = p.channel_mut(cid) {
                        c.gain_db = fader_to_db(vol);
                    }
                });
        }
        cells.cut_left(4.0);
        let mut pan = (ch.pan + 1.0) * 0.5;
        let presp = ui.knob(
            W::new(("rack-pan", ch.id)),
            Rect::from_center(cells.cut_left(KNOB).center(), KNOB, KNOB),
            &mut pan,
            0.5,
            t.text_dim,
            true,
        );
        ui.name(&presp, &format!("{} pan", ch.name));
        ui.tooltip(&presp, "Pan");
        if presp.changed {
            let cid = ch.id;
            s.doc.edit_gesture(presp.id.0, "Change channel pan", |p| {
                if let Some(c) = p.channel_mut(cid) {
                    c.pan = (pan * 2.0 - 1.0).clamp(-1.0, 1.0);
                }
            });
        }
        if vresp.released || presp.released {
            s.doc.end_gesture();
        }

        if *as_steps {
            let hits = RowHits::new(notes, &layout);
            step_row(ui, &grid, row, ch, &hits, playing_step, paint_id, s, pid);
            if let Some(g) = graph_row {
                velocity_graph(ui, g, &grid, ch, &hits, s, pid);
            }
        } else {
            let preview = Rect {
                x: grid_x,
                w: layout.width.min(grid_w),
                ..row
            };
            note_preview(ui, preview, ch, notes, pattern.length, ws);
        }
    }

    // Add-channel row.
    let add_r = Rect {
        x: body.x + 24.0,
        y: y + 1.0,
        w: NAME_W,
        h: ROW_H - 2.0,
    };
    let add = ui.interact(W::new("rack-add"), add_r);
    let hot = ui.animate_bool(W::new("rack-add-hot"), add.hovered);
    if hot > 0.0 {
        ui.scene.rect(add_r, t.radius_s, t.control.alpha(hot));
    }
    let mut ar = add_r.shrink2(8.0, 0.0);
    ui.icon(ar.cut_left(16.0), icons::PLUS, 14.0, t.text_dim);
    ar.cut_left(6.0);
    ui.text(ar, "Add", t.label, t.text_dim, Align::Start);
    if add.clicked {
        ui.open_popup(W::new("rack-add-menu"), add_r);
    }
    if add.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    ui.scene.pop_clip();

    if hovering_drop {
        ui.scene
            .outline(body.expand(2.0), t.radius_m, 2.0, t.focus.alpha(0.6));
    }

    let instruments: Vec<(&'static str, &'static str)> = peel_devices::instruments()
        .map(|d| (d.type_id, d.name))
        .collect();
    let mut items: Vec<MenuItem> = instruments.iter().map(|(_, n)| MenuItem::new(*n)).collect();
    items.push(MenuItem::Separator);
    items.push(MenuItem::new("Sampler from file…"));
    if let Some(i) = ui.menu(
        W::new("rack-add-menu"),
        vec2(add_r.x, add_r.bottom() + 4.0),
        &items,
    ) {
        if i < instruments.len() {
            if let Some(id) = add_instrument(s, instruments[i].0) {
                select(ws, id);
            }
        } else if let Some(path) =
            crate::dialogs::open(&[("Audio", &["wav", "aif", "aiff", "flac", "mp3", "ogg"])])
            && let Some(id) = add_sample_channel(s, &path)
        {
            select(ws, id);
        }
    }

    if !ui.input.down[0] {
        let st = ui.state::<PaintState>(paint_id);
        if st.active {
            st.active = false;
            s.doc.end_gesture();
        }
    }
}

/// The step grid as drawn this frame: the layout, the visible column and
/// how far it is scrolled.
struct StepGrid<'a> {
    layout: &'a StepLayout,
    view_x: f32,
    view_w: f32,
    scroll: f32,
}

impl StepGrid<'_> {
    /// Screen rect of step `i` in a row spanning `y..y + h`.
    fn cell(&self, i: usize, y: f32, h: f32) -> Rect {
        Rect {
            x: self.view_x - self.scroll + self.layout.xs[i],
            y,
            w: self.layout.w,
            h,
        }
    }

    fn view(&self, y: f32, h: f32) -> Rect {
        Rect {
            x: self.view_x,
            y,
            w: self.view_w,
            h,
        }
    }

    /// Visible steps, first to last.
    fn visible(&self) -> std::ops::Range<usize> {
        let l = self.layout;
        let lo = l.xs.partition_point(|&x| x + l.w < self.scroll);
        let hi = l.xs.partition_point(|&x| x <= self.scroll + self.view_w);
        lo..hi.max(lo)
    }

    fn index_at(&self, x: f32) -> Option<usize> {
        self.layout.index_at(x - self.view_x + self.scroll)
    }
}

/// The step size menu: readable names with the short value beside them,
/// and the step each item picks.
fn step_menu(current: Time) -> (Vec<MenuItem>, Vec<Option<Time>>) {
    let mut items = vec![MenuItem::Heading("Step size".into())];
    let mut map = vec![None];
    for (i, step) in RACK_STEPS.iter().enumerate() {
        if i > 0 && step.short.ends_with('T') && !RACK_STEPS[i - 1].short.ends_with('T') {
            items.push(MenuItem::Separator);
            map.push(None);
        }
        items.push(
            MenuItem::new(step.name)
                .shortcut(step.short)
                .checked(step.len == current),
        );
        map.push(Some(step.len));
    }
    (items, map)
}

/// Bar numbers over the steps, scrolled with them.
fn step_ruler(ui: &mut Ui, ruler: Rect, grid: &StepGrid, bar: Time) {
    let t = ui.theme.clone();
    let view = grid.view(ruler.y, ruler.h);
    ui.scene.push_clip(view);
    let l = grid.layout;
    for i in grid.visible() {
        let pos = l.pos(i);
        if !pos.rem_euclid(Time::beats(1)).is_zero() {
            continue;
        }
        let x = grid.cell(i, ruler.y, ruler.h).x;
        if pos.rem_euclid(bar).is_zero() {
            let n = pos.checked_div(bar).map_or(0, |q| q.floor_int()) + 1;
            ui.text_at(
                vec2(x + 1.0, ruler.y + 4.0),
                &n.to_string(),
                t.caption,
                t.text_dim,
            );
        } else {
            ui.scene.vline(
                x + 0.5,
                ruler.bottom() - 6.0,
                ruler.bottom() - 2.0,
                t.text_faint.alpha(0.5),
            );
        }
    }
    ui.scene.pop_clip();
}

/// The step size picker: what one step is worth. `hint` is set when some
/// hits fall between steps, with the step size that would show them.
fn step_size_button(ui: &mut Ui, r: Rect, ws: &mut Workspace, hint: Option<Option<Time>>) {
    let t = ui.theme.clone();
    let short = rhythm_of(ws.rack_step).map_or_else(
        || crate::util::format_length(ws.rack_step),
        |r| r.short.to_string(),
    );
    let label = format!("Steps {short}");
    let w = ui.quiet_button_width(false, &label, true).min(r.w);
    let r = Rect { w, ..r };
    let menu_id = W::new("rack-steps-menu");
    let b = ui.quiet_button(
        W::new("rack-steps"),
        r,
        None,
        &label,
        true,
        ui.is_popup_open(menu_id),
    );
    ui.name(&b, "Step size");
    ui.value_text(&b, &short);
    let name = rhythm_of(ws.rack_step).map_or("Custom", |r| r.name);
    let mut tip = format!(
        "Step size: each step is one {} note. Pattern length is in the toolbar",
        name.to_lowercase()
    );
    if let Some(fit) = hint {
        // Some hits sit between steps: say so, and which size shows them.
        ui.scene
            .circle(vec2(r.right() + 4.0, r.center().y), 2.5, t.accent);
        match fit.and_then(rhythm_of) {
            Some(f) => tip.push_str(&format!(
                ". Some hits fall between steps; {} shows them all",
                f.short
            )),
            None => tip.push_str(". Some hits fall between steps"),
        }
    }
    ui.tooltip(&b, &tip);
    if b.clicked {
        ui.open_popup(menu_id, r);
    }
    let (items, map) = step_menu(ws.rack_step);
    if let Some(i) = ui.menu(menu_id, vec2(r.x, r.bottom() + 4.0), &items)
        && let Some(Some(step)) = map.get(i)
    {
        ws.rack_step = *step;
    }
}

fn select(ws: &mut Workspace, id: Id) {
    ws.selected_channel = Some(id);
    ws.focus = Some(Focus::Channel(id));
}

fn header_tools(
    ui: &mut Ui,
    tools: Rect,
    s: &mut Session,
    pid: Id,
    pattern: &peel_document::Pattern,
) {
    let t = ui.theme.clone();
    let mut row = tools;
    // Pattern length: shorter, the length, longer, in one well.
    let bars = (pattern.length.as_f64() / 4.0).max(0.25);
    let well = row.cut_right(28.0 * 2.0 + 56.0 + 4.0);
    ui.well(well);
    let mut len_row = well.shrink(2.0);
    let plus = ui.icon_button(
        W::new("rack-len-plus"),
        len_row.cut_right(28.0),
        icons::PLUS,
        false,
        None,
    );
    let label = if bars == 1.0 {
        "1 bar".to_string()
    } else if bars.fract() == 0.0 {
        format!("{bars:.0} bars")
    } else {
        format!("{bars} bars")
    };
    let len_r = len_row.cut_right(56.0);
    ui.text(len_r, &label, t.label, t.text_dim, Align::Center);
    let len_resp = ui.interact(W::new("rack-len-label"), len_r);
    ui.tooltip(&len_resp, &format!("Pattern length: {label}"));
    let minus = ui.icon_button(
        W::new("rack-len-minus"),
        len_row.cut_right(28.0),
        icons::MINUS,
        false,
        None,
    );
    ui.name(&plus, "Lengthen pattern");
    ui.name(&minus, "Shorten pattern");
    ui.tooltip(&plus, "Lengthen pattern by a bar");
    ui.tooltip(&minus, "Shorten pattern");
    let mut new_len = None;
    if plus.clicked {
        new_len = Some((pattern.length + Time::beats(4)).min(Time::beats(64)));
    }
    if minus.clicked && pattern.length > Time::beats(4) {
        new_len = Some(pattern.length - Time::beats(4));
    } else if minus.clicked {
        new_len = Some(Time::beats(2).max(pattern.length.div_int(2)));
    }
    if let Some(len) = new_len {
        s.doc.edit("Change pattern length", |p| {
            if let Some(pt) = p.pattern_mut(pid) {
                pt.length = len;
            }
        });
    }
    row.cut_right(8.0);
    // Swing: the knob, its label to the left. The label is a hover target
    // of its own (showing the amount) and goes only when there is no room
    // for it beside the velocity toggle; the knob keeps its name and value
    // for tooltips and screen readers either way.
    let mut swing = s.project().swing;
    let kr = row.cut_right(26.0);
    let k = ui.knob(W::new("rack-swing"), kr, &mut swing, 0.0, t.accent, false);
    let amount = format!("Swing {:.0}%", swing * 100.0);
    ui.name(&k, "Swing");
    ui.value_text(&k, &format!("{:.0}%", swing * 100.0));
    ui.tooltip(&k, &amount);
    if k.changed {
        s.doc
            .edit_gesture(k.id.0, "Change swing", |p| p.swing = swing);
    }
    if k.released {
        s.doc.end_gesture();
    }
    let label_w = ui.measure("Swing", t.caption).x + 2.0;
    if row.w >= label_w + 6.0 {
        row.cut_right(6.0);
        let lr = row.cut_right(label_w);
        let lresp = ui.interact(W::new("rack-swing-label"), lr);
        ui.text(lr, "Swing", t.caption, t.text_faint, Align::End);
        ui.tooltip(&lresp, &amount);
    }
}

fn channel_button(
    ui: &mut Ui,
    r: Rect,
    ch: &Channel,
    selected: bool,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let id = W::new(("rack-name", ch.id));
    let resp = ui.interact_button(id, r, &ch.name);
    let hot = ui.animate_bool(id, resp.hovered);
    // One step above the panel; the selected channel steps up again.
    let base = if selected {
        t.control_hover
    } else {
        t.control.mix(t.control_hover, hot)
    };
    ui.scene.rect(r, t.radius_s, base);
    let c = col(ch.color);
    let activity = s.take_channel_activity(ch.id);
    let glow = ui.animate(W::new(("rack-act", ch.id)), activity.min(1.0), 10.0);
    ui.scene.rect_ex(
        Rect { w: 3.0, ..r },
        [t.radius_s, 0.0, 0.0, t.radius_s],
        c,
        0.0,
        Color::TRANSPARENT,
    );
    if glow > 0.01 {
        ui.scene.rect(r, t.radius_s, c.alpha(glow * 0.22));
    }
    let name = ui.fit_text(&ch.name, t.label, r.w - 16.0);
    ui.text(
        r.shrink2(10.0, 0.0),
        &name,
        t.label,
        if selected {
            t.text
        } else {
            t.text.mix(t.text_dim, 0.25)
        },
        Align::Start,
    );
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    let dev = peel_devices::descriptor(&ch.device.type_id).map_or("Missing device", |d| d.name);
    ui.tooltip(&resp, dev);
    if resp.pressed {
        select(ws, ch.id);
    }
    if resp.double_clicked {
        ws.dock = DockTab::PianoRoll;
    }
    let menu_id = W::new(("rack-menu", ch.id));
    if resp.secondary_clicked {
        select(ws, ch.id);
        ui.open_popup(menu_id, r);
    }
    let mut items = vec![
        MenuItem::new("Piano roll").shortcut("F7"),
        MenuItem::new("Instrument"),
        MenuItem::new("Rename channel"),
        MenuItem::new("Clone channel"),
        MenuItem::new("Clear notes in pattern"),
        MenuItem::Separator,
        MenuItem::Heading("Color".into()),
    ];
    let first_color = items.len();
    for (i, pc) in PALETTE.iter().enumerate() {
        items.push(
            MenuItem::new(crate::util::COLOR_NAMES[i])
                .swatch(col(*pc))
                .checked(*pc == ch.color),
        );
    }
    items.push(MenuItem::Separator);
    let delete_idx = items.len();
    items.push(MenuItem::new("Delete channel"));
    if let Some(i) = ui.menu(menu_id, vec2(r.x, r.bottom() + 4.0), &items) {
        let cid = ch.id;
        match i {
            0 => ws.dock = DockTab::PianoRoll,
            1 => crate::instrument_editor::open(s, ws, cid),
            2 => crate::rename::open(ui, W::new(("rename-channel-popup", cid)), r, &ch.name),
            3 => clone_channel(s, cid),
            4 => {
                let pid = current_pattern(s);
                s.doc.edit("Clear notes", |p| {
                    if let Some(pt) = p.pattern_mut(pid) {
                        pt.notes.retain(|n| n.channel != cid);
                        crate::note_expr::prune(pt);
                    }
                });
            }
            i if i >= first_color && i < first_color + PALETTE.len() => {
                let color = PALETTE[i - first_color];
                s.doc.edit("Recolor channel", |p| {
                    if let Some(c) = p.channel_mut(cid) {
                        c.color = color;
                        let track = c.track;
                        if let Some(t) = p.mixer.track_mut(track) {
                            t.color = color;
                        }
                    }
                });
            }
            i if i == delete_idx => delete_channel(s, ws, cid),
            _ => {}
        }
    }
    if let Some(name) = crate::rename::show(ui, W::new(("rename-channel-popup", ch.id))) {
        let cid = ch.id;
        s.doc.edit("Rename channel", |p| {
            if let Some(c) = p.channel_mut(cid) {
                c.name = name.clone();
                let track = c.track;
                if let Some(t) = p.mixer.track_mut(track) {
                    t.name = name;
                }
            }
        });
    }
    // Tiny tag showing the instrument in a strong ink on selection.
    let _ = ink_on(c);
}

#[allow(clippy::too_many_arguments)]
fn step_row(
    ui: &mut Ui,
    grid: &StepGrid,
    row: Rect,
    ch: &Channel,
    hits: &RowHits,
    playing: Option<usize>,
    paint_id: W,
    s: &mut Session,
    pid: Id,
) {
    let t = ui.theme.clone();
    let c = col(ch.color);
    let l = grid.layout;
    let view = grid.view(row.y, row.h);
    ui.scene.push_clip(view);
    for i in grid.visible() {
        let cell = grid.cell(i, row.y + 1.0, row.h - 2.0);
        let pos = l.pos(i);
        let note = hits.on[i];
        let between = &hits.between[i];
        let on = note.is_some();
        let id = W::new(("step", ch.id, i));
        // Only the visible part of a step at the edge takes the pointer.
        let target = cell.intersect(&view);
        let label = if between.is_empty() {
            format!("{} step {}", ch.name, i + 1)
        } else {
            format!(
                "{} step {}, {} between steps",
                ch.name,
                i + 1,
                between.len()
            )
        };
        let resp = ui.interact_button(id, target, &label);
        let off_fill = if l.beat_odd(i) {
            t.control.lighten(0.04)
        } else {
            t.control.darken(0.18)
        };
        let off_fill = if l.pulse_start(i) {
            off_fill.lighten(0.05)
        } else {
            off_fill
        };
        let hot = resp.hovered as i32 as f32;
        if let Some(n) = note {
            let fill = c.mix(t.control, (1.0 - n.velocity) * 0.55);
            ui.scene.rect(cell, t.radius_s, fill.lighten(hot * 0.1));
        } else {
            ui.scene
                .rect(cell, t.radius_s, off_fill.lighten(hot * 0.08));
        }
        // Hits between steps: a tick where each one lands, so nothing
        // hides at a coarse step size.
        for f in between {
            let x = cell.x + cell.w * f;
            ui.scene.rect(
                Rect {
                    x: x - 1.0,
                    y: cell.y + 3.0,
                    w: 2.0,
                    h: cell.h - 6.0,
                },
                1.0,
                if on { ink_on(c).alpha(0.7) } else { c },
            );
        }
        if playing == Some(i) {
            ui.scene.rect(
                cell,
                t.radius_s,
                Color::WHITE.alpha(if on { 0.30 } else { 0.12 }),
            );
        }
        if resp.hovered {
            ui.set_cursor(CursorIcon::Pointer);
            if !between.is_empty() {
                ui.tooltip(
                    &resp,
                    &format!(
                        "{} between steps. A finer step size shows {}",
                        if between.len() == 1 {
                            "1 hit".to_string()
                        } else {
                            format!("{} hits", between.len())
                        },
                        if between.len() == 1 { "it" } else { "them" }
                    ),
                );
            }
        }
        // Paint: the first step touched decides on/off for the whole drag.
        let pointer_inside = ui.pointer().is_some_and(|p| target.contains(p));
        let (paint_active, paint_on, gesture) = {
            let st = ui.state::<PaintState>(paint_id);
            (st.active, st.on, st.gesture)
        };
        if resp.pressed {
            let want = !on;
            let gesture = id.0 ^ 0x5eed;
            {
                let st = ui.state::<PaintState>(paint_id);
                st.active = true;
                st.on = want;
                st.gesture = gesture;
            }
            set_step(s, pid, ch, pos, l.step, want, gesture);
            if want {
                s.live_note(ch.id, ch.root_key, 0.8, true);
                s.live_note(ch.id, ch.root_key, 0.0, false);
            }
        } else if paint_active && ui.input.down[0] && pointer_inside && on != paint_on {
            set_step(s, pid, ch, pos, l.step, paint_on, gesture);
        }
        // Keyboard or assistive activation (no mouse press behind it).
        if resp.clicked && !paint_active && !resp.pressed {
            set_step(s, pid, ch, pos, l.step, !on, id.0 ^ 0x6b);
            s.doc.end_gesture();
        }
        if resp.secondary_clicked && on {
            set_step(s, pid, ch, pos, l.step, false, id.0 ^ 0xdead);
            s.doc.end_gesture();
        }
        // Wheel over a lit step adjusts its velocity.
        if resp.hovered && on && ui.input.scroll.y != 0.0 && ui.input.modifiers.alt {
            let dv = ui.input.scroll.y / 400.0;
            ui.input.scroll.y = 0.0;
            let cid = ch.id;
            s.doc.edit_gesture(id.0 ^ 0x7e1, "Change velocity", |p| {
                if let Some(pt) = p.pattern_mut(pid)
                    && let Some(n) = pt
                        .notes
                        .iter_mut()
                        .find(|n| n.channel == cid && n.start == pos)
                {
                    n.velocity = (n.velocity + dv).clamp(0.05, 1.0);
                }
            });
        }
    }
    ui.scene.pop_clip();
}

/// Turns the step at `pos` on (a hit one step long) or off. Hits between
/// steps are never touched.
fn set_step(s: &mut Session, pid: Id, ch: &Channel, pos: Time, len: Time, on: bool, gesture: u64) {
    let cid = ch.id;
    let key = ch.root_key;
    let label = if on { "Add steps" } else { "Remove steps" };
    s.doc.edit_gesture(gesture, label, |p| {
        if let Some(pt) = p.pattern_mut(pid) {
            let exists = pt.notes.iter().any(|n| n.channel == cid && n.start == pos);
            if on && !exists {
                pt.notes.push(Note::new(cid, pos, len, key, 0.8));
                pt.sort_notes();
            } else if !on {
                pt.notes.retain(|n| !(n.channel == cid && n.start == pos));
                crate::note_expr::prune(pt);
            }
        }
    });
}

fn note_preview(
    ui: &mut Ui,
    grid: Rect,
    ch: &Channel,
    notes: &[Note],
    length: Time,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let r = grid.shrink2(0.0, 1.0);
    let id = W::new(("rack-preview", ch.id));
    let resp = ui.interact(id, r);
    let hot = ui.animate_bool(id, resp.hovered);
    ui.scene
        .rect(r, t.radius_s, t.well.mix(t.control, hot * 0.4));
    let (lo, hi) = notes
        .iter()
        .fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
    let span = (hi as f32 - lo as f32 + 1.0).max(8.0);
    let len = length.as_f64().max(0.25) as f32;
    let c = col(ch.color);
    ui.scene.push_clip(r);
    for beat in 1..(len.ceil() as i32) {
        let x = r.x + r.w * beat as f32 / len;
        ui.scene.vline(
            x,
            r.y + 3.0,
            r.bottom() - 3.0,
            if beat % 4 == 0 {
                t.grid_bar
            } else {
                t.grid_beat
            },
        );
    }
    for n in notes {
        let x0 = r.x + r.w * n.start.as_f64() as f32 / len;
        let w = (r.w * n.length.as_f64() as f32 / len).max(2.0);
        let y = r.bottom() - 4.0 - (r.h - 10.0) * ((n.key - lo) as f32 + 0.5) / span;
        ui.scene.rect(
            Rect {
                x: x0,
                y: y - 1.5,
                w,
                h: 3.0,
            },
            1.5,
            c,
        );
    }
    ui.scene.pop_clip();
    ui.tooltip(&resp, "Open in the piano roll");
    if resp.clicked {
        ws.selected_channel = Some(ch.id);
        ws.focus = Some(Focus::Channel(ch.id));
        ws.dock = DockTab::PianoRoll;
    }
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
}

// ---- channel operations ----

/// Adds a native instrument channel routed to a new mixer track.
pub fn add_instrument(s: &mut Session, type_id: &str) -> Option<Id> {
    add_instrument_preset(s, type_id, None)
}

/// Adds an instrument channel with a factory preset applied.
pub fn add_instrument_preset(s: &mut Session, type_id: &str, preset: Option<&str>) -> Option<Id> {
    if crate::util::mixer_full(s) {
        return None;
    }
    let desc = peel_devices::descriptor(type_id)?;
    // New drum channels start with the acoustic kit. Explicit presets and
    // existing project instances keep their own settings.
    let default = match type_id {
        "peel.drum.kick" => Some("Acoustic Kick"),
        "peel.drum.snare" => Some("Acoustic Snare"),
        "peel.drum.hat" => Some("Acoustic Closed"),
        "peel.drum.tom" => Some("Acoustic Rack"),
        _ => None,
    };
    let dev = template::device(type_id, preset.or(default));
    let name = preset
        .filter(|p| {
            !peel_devices::presets(type_id)
                .first()
                .is_some_and(|f| f.name == *p)
        })
        .unwrap_or(desc.name)
        .to_string();
    let mut new_id = None;
    // The recorded kit's Full Kit preset follows GM drum keys. Start its
    // steps on the kick; per-piece presets use the usual C5 root.
    let root = match type_id {
        "peel.drum.acoustic" if preset.is_none_or(|p| p == "Full Kit") => 36,
        "peel.guitar.recorded" => 40,
        "peel.bass.recorded" => 28,
        _ => 60,
    };
    s.doc.edit(format!("Add {name}"), |p| {
        let n = p.channels.len();
        new_id = Some(template::add_channel(p, &name, n * 5 + 2, dev, root));
    });
    new_id
}

/// Adds a channel playing an installed plugin instrument.
pub fn add_plugin_channel(s: &mut Session, type_id: &str, name: &str) -> Option<Id> {
    if crate::util::mixer_full(s) {
        return None;
    }
    let dev = peel_document::DeviceInstance::new(type_id, 1);
    let mut new_id = None;
    s.doc.edit(format!("Add {name}"), |p| {
        let n = p.channels.len();
        new_id = Some(template::add_channel(p, name, n * 5 + 2, dev, 60));
    });
    new_id
}

/// Adds a sampler channel playing `path`.
pub fn add_sample_channel(s: &mut Session, path: &Path) -> Option<Id> {
    if crate::util::mixer_full(s) {
        return None;
    }
    let asset = match s.reference_audio_file(path) {
        Ok(a) => a,
        Err(e) => {
            s.notify(format!("Could not read {}: {e}", path.display()), true);
            return None;
        }
    };
    let mut dev = template::device("peel.sampler", None);
    dev.sample = Some(asset.id);
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Sample".into());
    let mut new_id = None;
    s.doc.edit(format!("Add {name}"), |p| {
        if p.asset(asset.id).is_none() {
            p.assets.push(asset.clone());
        }
        let n = p.channels.len();
        new_id = Some(template::add_channel(p, &name, n * 5 + 2, dev, 60));
    });
    new_id
}

fn clone_channel(s: &mut Session, cid: Id) {
    if crate::util::mixer_full(s) {
        return;
    }
    let Some(src) = s.project().channel(cid).cloned() else {
        return;
    };
    s.doc.edit(format!("Clone {}", src.name), |p| {
        let mut dev = src.device.clone();
        dev.id = Id::new();
        let n = p.channels.len();
        let id = template::add_channel(p, &format!("{} 2", src.name), n * 5 + 2, dev, src.root_key);
        if let Some(c) = p.channel_mut(id) {
            c.color = src.color;
            c.gain_db = src.gain_db;
            c.pan = src.pan;
        }
    });
}

pub fn delete_channel(s: &mut Session, ws: &mut Workspace, cid: Id) {
    let Some(ch) = s.project().channel(cid).cloned() else {
        return;
    };
    s.doc.edit(format!("Delete {}", ch.name), |p| {
        p.channels.retain(|c| c.id != cid);
        for pt in p.patterns.iter_mut() {
            if pt.uses_channel(cid) {
                let pt = std::sync::Arc::make_mut(pt);
                pt.notes.retain(|n| n.channel != cid);
                crate::note_expr::prune(pt);
            }
        }
        // Remove the channel's private mixer track if nothing else uses it.
        let used = p.channels.iter().any(|c| c.track == ch.track);
        if !used && p.mixer.tracks.first().is_some_and(|m| m.id != ch.track) {
            p.mixer.tracks.retain(|t| t.id != ch.track);
            for t in p.mixer.tracks.iter_mut() {
                if t.output == Some(ch.track) {
                    t.output = None;
                }
            }
        }
    });
    if ws.selected_channel == Some(cid) {
        ws.selected_channel = s.project().channels.first().map(|c| c.id);
        ws.focus = ws.selected_channel.map(Focus::Channel);
    }
}

/// Velocity bars under the selected channel's steps; drag across to draw.
fn velocity_graph(
    ui: &mut Ui,
    row: Rect,
    grid: &StepGrid,
    ch: &Channel,
    hits: &RowHits,
    s: &mut Session,
    pid: Id,
) {
    let t = ui.theme.clone();
    let c = col(ch.color);
    let l = grid.layout;
    ui.scene.rect(row, 0.0, t.well);
    ui.text(
        Rect {
            x: row.x + 24.0,
            w: NAME_W,
            ..row
        },
        "Velocity",
        t.caption,
        t.text_faint,
        Align::Start,
    );
    let view = grid.view(row.y, row.h);
    let inner = view.shrink2(0.0, 6.0);
    let id = W::new(("rack-velocity", ch.id));
    let resp = ui.interact(id, inner);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Crosshair);
    }
    ui.scene.push_clip(view);
    for i in grid.visible() {
        let cell = grid.cell(i, inner.y, inner.h);
        ui.scene.rect(cell, 0.0, t.control.darken(0.18));
        if let Some(n) = hits.on[i] {
            let h = cell.h * n.velocity;
            ui.scene.rect(
                Rect {
                    y: cell.bottom() - h,
                    h,
                    ..cell
                },
                2.0,
                c.alpha(0.85),
            );
        }
    }
    ui.scene.pop_clip();
    if (resp.pressed || resp.dragging)
        && let Some(p) = ui.pointer()
        && let Some(i) = grid.index_at(p.x)
        && hits.on.get(i).is_some_and(|n| n.is_some())
    {
        let pos = l.pos(i);
        let vel = ((inner.bottom() - p.y) / inner.h).clamp(0.05, 1.0);
        let cid = ch.id;
        s.doc.edit_gesture(id.0, "Change velocity", |p| {
            if let Some(pt) = p.pattern_mut(pid)
                && let Some(n) = pt
                    .notes
                    .iter_mut()
                    .find(|n| n.channel == cid && n.start == pos)
            {
                n.velocity = vel;
            }
        });
    }
    if resp.released {
        s.doc.end_gesture();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! Step size and step grid interaction, driven frame by frame.

    use peel_document::{ExpressionPoint, Id, Note, Pattern, Time};
    use peel_session::{Session, template};
    use peel_ui::{App, Id as W, Rect, Ui, Vec2, WindowControl, vec2};

    use super::{STEP_MIN_W, step_menu};
    use crate::note_expr::{Lane, set_curve};
    use crate::studio::Studio;

    #[test]
    fn new_drum_channels_use_the_acoustic_kit_and_allow_electronic_presets() {
        let mut session = Session::new_silent(template::empty());
        for (kind, preset, control) in [
            ("peel.drum.kick", "Acoustic Kick", "body.acoustic"),
            ("peel.drum.snare", "Acoustic Snare", "body.acoustic"),
            ("peel.drum.hat", "Acoustic Closed", "tone.acoustic"),
            ("peel.drum.tom", "Acoustic Rack", "body.acoustic"),
        ] {
            let id = super::add_instrument(&mut session, kind).unwrap();
            let ch = session.project().channel(id).unwrap();
            assert_eq!(ch.device.preset.as_deref(), Some(preset));
            assert_eq!(ch.device.params.get(control), Some(&1.0));
            assert_eq!(ch.name, peel_devices::descriptor(kind).unwrap().name);
        }
        let id =
            super::add_instrument_preset(&mut session, "peel.drum.kick", Some("Punch")).unwrap();
        let kick = &session.project().channel(id).unwrap().device;
        assert_eq!(kick.preset.as_deref(), Some("Punch"));
        assert_eq!(kick.params.get("body.acoustic"), Some(&0.0));
    }

    #[test]
    fn recorded_kit_steps_start_on_a_playable_drum_key() {
        let mut session = Session::new_silent(template::empty());
        for preset in [
            None,
            Some("Full Kit"),
            Some("Kick"),
            Some("Snare"),
            Some("Crash"),
        ] {
            let id =
                super::add_instrument_preset(&mut session, "peel.drum.acoustic", preset).unwrap();
            let ch = session.project().channel(id).unwrap();
            assert_eq!(
                ch.root_key,
                if preset.is_none_or(|p| p == "Full Kit") {
                    36
                } else {
                    60
                }
            );
            assert_eq!(ch.device.type_id, "peel.drum.acoustic");
        }
    }

    #[test]
    fn recorded_strings_start_on_their_low_e_notes() {
        let mut s = Session::new_silent(template::empty());
        for (kind, root) in [("peel.guitar.recorded", 40), ("peel.bass.recorded", 28)] {
            let id = super::add_instrument(&mut s, kind).unwrap();
            assert_eq!(s.project().channel(id).unwrap().root_key, root);
        }
    }

    /// The real Studio against a silent engine, with scripted input.
    pub(crate) struct Harness {
        pub ui: Ui,
        pub studio: Studio,
        win: WindowControl,
        time: f64,
        pub size: Vec2,
    }

    impl Harness {
        pub fn new() -> Harness {
            Harness::sized(1440.0, 900.0)
        }

        pub fn sized(w: f32, h: f32) -> Harness {
            let session = Session::new_silent(template::beat_and_melody());
            let mut h = Harness {
                ui: Ui::new(),
                studio: Studio::new(session),
                win: WindowControl::headless(),
                time: 0.0,
                size: vec2(w, h),
            };
            h.frames(3);
            h
        }

        pub fn frame(&mut self) {
            self.time += 1.0 / 60.0;
            let r = Rect {
                x: 0.0,
                y: 0.0,
                w: self.size.x,
                h: self.size.y,
            };
            self.ui.begin_frame(r, 1.0, self.time);
            self.studio.update(&mut self.ui, &self.win);
            self.ui.end_frame();
        }

        pub fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.frame();
            }
        }

        #[track_caller]
        pub fn rect(&self, id: W) -> Rect {
            match self.ui.widget_rect(id) {
                Some(r) => r,
                None => panic!("widget {id:?} not on screen"),
            }
        }

        pub fn move_to(&mut self, p: Vec2) {
            let old = self.ui.input.pointer.unwrap_or(p);
            self.ui.input.pointer = Some(p);
            self.ui.input.pointer_delta = p - old;
            self.ui.input.had_event = true;
            self.frame();
        }

        pub fn press(&mut self) {
            self.ui.input.down[0] = true;
            self.ui.input.pressed[0] = true;
            self.ui.input.press_pos[0] = self.ui.input.pointer.unwrap_or_default();
            self.frame();
        }

        pub fn release(&mut self) {
            self.ui.input.down[0] = false;
            self.ui.input.released[0] = true;
            self.frame();
        }

        pub fn click_at(&mut self, p: Vec2) {
            self.move_to(p);
            self.move_to(p);
            self.press();
            self.release();
        }

        pub fn click(&mut self, id: W) {
            let c = self.rect(id).center();
            self.click_at(c);
        }

        pub fn drag(&mut self, from: Vec2, to: Vec2, steps: usize) {
            self.move_to(from);
            self.move_to(from);
            self.press();
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                self.move_to(from + (to - from) * t);
            }
            self.release();
        }

        pub fn session(&self) -> &Session {
            &self.studio.session
        }

        pub fn pattern(&self) -> Pattern {
            let pid = self.session().selected_pattern();
            (**self.session().project().pattern(pid).unwrap()).clone()
        }

        pub fn notes(&self, ch: Id) -> Vec<Note> {
            self.pattern().notes_for(ch).copied().collect()
        }

        pub fn history(&self) -> usize {
            self.session().doc.history_labels().count()
        }

        pub fn channel(&self, name: &str) -> Id {
            self.session()
                .project()
                .channels
                .iter()
                .find(|c| c.name == name)
                .unwrap()
                .id
        }

        /// Picks a step size from the rack's own menu.
        pub fn pick_step(&mut self, step: Time) {
            self.click(W::new("rack-steps"));
            self.frame();
            let (_, map) = step_menu(self.studio.ws.rack_step);
            let i = map.iter().position(|m| *m == Some(step)).unwrap();
            self.click(W::new("rack-steps-menu").with(("item", i)));
            self.frame();
            assert_eq!(self.studio.ws.rack_step, step);
        }

        /// Accessible name of a widget (accessibility must be on).
        pub fn label(&self, id: W) -> String {
            self.ui
                .accessibility_node(id)
                .and_then(|n| n.label())
                .unwrap_or_default()
                .to_string()
        }

        /// Accessible value of a widget (accessibility must be on).
        pub fn value(&self, id: W) -> String {
            self.ui
                .accessibility_node(id)
                .and_then(|n| n.value())
                .unwrap_or_default()
                .to_string()
        }

        #[track_caller]
        fn step(&self, ch: Id, i: usize) -> Rect {
            self.rect(W::new(("step", ch, i)))
        }
    }

    /// Starts of `notes` other than those in `before`.
    fn added(before: &[Note], after: &[Note]) -> Vec<Note> {
        after
            .iter()
            .filter(|n| !before.iter().any(|b| b.id == n.id))
            .copied()
            .collect()
    }

    #[test]
    fn fine_steps_place_a_fast_kick_burst_between_the_beats() {
        let mut h = Harness::new();
        let kick = h.channel("Kick");
        let original = h.notes(kick);
        assert_eq!(original.len(), 4);

        // Thirty-seconds: the sixteenth hits stay where they are, and the
        // steps between them take new hits.
        h.pick_step(Time::new(1, 8));
        assert_eq!(h.notes(kick), original, "a view change edits nothing");
        let before = h.notes(kick);
        let undo_before = h.history();
        // A quick double: paint steps 1 to 3 in one drag (between beat 1's
        // hit on step 0 and beat 2's on step 8).
        let a = h.step(kick, 1).center();
        let b = h.step(kick, 3).center();
        h.drag(a, b, 9);
        let mut burst = added(&before, &h.notes(kick));
        burst.sort_by_key(|n| n.start);
        let starts: Vec<Time> = burst.iter().map(|n| n.start).collect();
        assert_eq!(
            starts,
            vec![Time::new(1, 8), Time::new(1, 4), Time::new(3, 8)]
        );
        assert!(burst.iter().all(|n| n.length == Time::new(1, 8)));
        assert!(burst.iter().all(|n| n.key == 60));
        assert_eq!(h.history(), undo_before + 1, "one drag, one undo step");
        h.studio.session.doc.undo();
        h.frame();
        assert_eq!(h.notes(kick), before, "undo takes the whole burst back");

        // The original sixteenth hit on beat 2 (step 8 at 1/32) is still a
        // step: clicking it removes exactly it, undo brings it back intact.
        let beat2 = original
            .iter()
            .find(|n| n.start == Time::beats(1))
            .copied()
            .unwrap();
        h.click(W::new(("step", kick, 8usize)));
        assert!(h.notes(kick).iter().all(|n| n.id != beat2.id));
        h.studio.session.doc.undo();
        h.frame();
        let back = h.notes(kick);
        let n = back.iter().find(|n| n.id == beat2.id).unwrap();
        assert_eq!(*n, beat2, "length and velocity survive");

        // Sixty-fourths right after the downbeat.
        h.pick_step(Time::new(1, 16));
        let before = h.notes(kick);
        h.click(W::new(("step", kick, 1usize)));
        h.click(W::new(("step", kick, 2usize)));
        let mut new = added(&before, &h.notes(kick));
        new.sort_by_key(|n| n.start);
        assert_eq!(
            new.iter().map(|n| (n.start, n.length)).collect::<Vec<_>>(),
            vec![
                (Time::new(1, 16), Time::new(1, 16)),
                (Time::new(1, 8), Time::new(1, 16)),
            ]
        );

        // Sixteenth triplets: exact thirds of an eighth, no drift even
        // far into the bar.
        h.pick_step(Time::new(1, 6));
        let before = h.notes(kick);
        for i in [1usize, 2, 4, 23] {
            h.click(W::new(("step", kick, i)));
        }
        let mut new = added(&before, &h.notes(kick));
        new.sort_by_key(|n| n.start);
        assert_eq!(
            new.iter().map(|n| n.start).collect::<Vec<_>>(),
            vec![
                Time::new(1, 6),
                Time::new(1, 3),
                Time::new(2, 3),
                Time::new(23, 6)
            ]
        );
        assert!(new.iter().all(|n| n.length == Time::new(1, 6)));
    }

    #[test]
    fn triplet_steps_group_in_threes_per_beat() {
        let mut h = Harness::new();
        let kick = h.channel("Kick");
        h.pick_step(Time::new(1, 3));
        // Twelve eighth-triplet steps in a 4/4 bar, a beat gap after every
        // third and plain gaps inside the beat.
        assert!(h.ui.widget_rect(W::new(("step", kick, 11usize))).is_some());
        assert!(h.ui.widget_rect(W::new(("step", kick, 12usize))).is_none());
        let gap = |h: &Harness, i: usize| h.step(kick, i + 1).x - h.step(kick, i).right();
        assert!((gap(&h, 0) - gap(&h, 1)).abs() < 0.01);
        assert!(gap(&h, 2) > gap(&h, 1) + 1.0, "beat break after step 3");
        assert!((gap(&h, 3) - gap(&h, 1)).abs() < 0.01);
        assert!(gap(&h, 5) > gap(&h, 4) + 1.0, "and after step 6");
    }

    #[test]
    fn changing_step_size_or_grid_never_touches_notes_or_expression() {
        let mut h = Harness::new();
        let kick = h.channel("Kick");
        let pid = h.session().selected_pattern();
        let first = h.notes(kick)[0].id;
        // A hit with a pitch curve and one between sixteenths.
        h.studio.session.doc.edit("setup", |p| {
            let pt = p.pattern_mut(pid).unwrap();
            pt.notes
                .push(Note::new(kick, Time::new(5, 12), Time::new(1, 12), 60, 0.4));
            pt.sort_notes();
            set_curve(
                pt,
                first,
                Lane::Pitch,
                &[
                    ExpressionPoint {
                        offset: Time::ZERO,
                        value: 0.5,
                    },
                    ExpressionPoint {
                        offset: Time::new(1, 8),
                        value: -1.0,
                    },
                ],
            );
        });
        h.frame();
        let before = h.pattern();
        let undo = h.history();
        for step in crate::workspace::RACK_STEPS {
            h.pick_step(step.len);
            h.frames(2);
        }
        h.studio.ws.dock = crate::workspace::DockTab::PianoRoll;
        h.frames(2);
        for r in crate::workspace::STRAIGHT
            .iter()
            .chain(&crate::workspace::TRIPLETS)
        {
            crate::piano_roll::set_grid(&mut h.studio.ws.roll, r.len);
            h.frames(2);
        }
        assert_eq!(h.pattern(), before, "notes and expression unchanged");
        assert_eq!(h.history(), undo, "no edits recorded");
    }

    #[test]
    fn hits_between_coarse_steps_stay_and_are_left_alone() {
        let mut h = Harness::new();
        let kick = h.channel("Kick");
        // A sixteenth hit on the "e" of beat 1, then switch to eighths:
        // it falls inside step 0.
        h.click(W::new(("step", kick, 1usize)));
        let e = h
            .notes(kick)
            .into_iter()
            .find(|n| n.start == Time::new(1, 4))
            .unwrap();
        h.pick_step(Time::new(1, 2));
        // Still a step row (not the piano-roll preview), and the hit's step
        // reports it.
        assert!(h.ui.widget_rect(W::new(("rack-preview", kick))).is_none());
        h.ui.set_accessibility_active(true);
        h.frames(2);
        let label = h.label(W::new(("step", kick, 0usize)));
        assert!(label.contains("1 between steps"), "{label}");
        // Toggling the on-step hit and adding the next step leave it be.
        h.click(W::new(("step", kick, 0usize)));
        h.click(W::new(("step", kick, 1usize)));
        let notes = h.notes(kick);
        assert!(notes.contains(&e), "the between-step hit is untouched");
        assert!(notes.iter().all(|n| n.start != Time::ZERO));
        assert!(notes.iter().any(|n| n.start == Time::new(1, 2)));
    }

    #[test]
    fn late_steps_in_a_long_pattern_are_reachable_at_the_minimum_window() {
        let mut h = Harness::sized(1200.0, 720.0);
        let kick = h.channel("Kick");
        let pid = h.session().selected_pattern();
        h.studio.session.doc.edit("lengthen", |p| {
            p.pattern_mut(pid).unwrap().length = Time::beats(16);
        });
        h.frame();
        h.pick_step(Time::new(1, 8));
        let last = 127usize;
        let name_x = h.rect(W::new(("rack-name", kick))).x;
        let first = h.step(kick, 0);
        assert!(first.w >= STEP_MIN_W - 0.01, "steps keep a usable width");
        assert!(h.ui.widget_rect(W::new("rack-hbar")).is_some());
        // Scroll sideways (trackpad) until the last step shows.
        let over = h.step(kick, 0).center();
        h.move_to(over);
        for _ in 0..200 {
            if let Some(r) = h.ui.widget_rect(W::new(("step", kick, last)))
                && r.w >= STEP_MIN_W - 0.01
            {
                break;
            }
            h.ui.input.scroll = vec2(-120.0, 0.0);
            h.frame();
        }
        let r = h.step(kick, last);
        assert!(r.w >= STEP_MIN_W - 0.01, "the last step is whole: {r:?}");
        let dock_right = h.rect(W::new("rack-hbar")).right();
        assert!(r.right() <= dock_right, "inside the rack");
        assert_eq!(
            h.rect(W::new(("rack-name", kick))).x,
            name_x,
            "the name column does not scroll"
        );
        let before = h.notes(kick);
        h.click(W::new(("step", kick, last)));
        let new = added(&before, &h.notes(kick));
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].start, Time::new(127, 8));
        assert_eq!(new[0].length, Time::new(1, 8));
        // Velocity graph lines up with the scrolled steps.
        h.click(W::new(("rack-name", kick)));
        h.click(W::new("rack-graph"));
        h.frame();
        let g = h.rect(W::new(("rack-velocity", kick)));
        let cell = h.step(kick, last);
        h.drag(
            vec2(cell.center().x, g.y + g.h * 0.75),
            vec2(cell.center().x, g.y + g.h * 0.7),
            2,
        );
        let n = h
            .notes(kick)
            .into_iter()
            .find(|n| n.id == new[0].id)
            .unwrap();
        assert!(n.velocity < 0.5, "graph edits the late hit: {}", n.velocity);
    }

    #[test]
    fn the_rack_toolbar_fits_the_step_size_at_both_window_sizes() {
        for (w, h_px) in [(1440.0, 900.0), (1200.0, 720.0)] {
            let mut h = Harness::sized(w, h_px);
            let ids = [
                "pattern-add",
                "rack-graph",
                "rack-swing-label",
                "rack-swing",
                "rack-len-minus",
                "rack-len-label",
                "rack-len-plus",
                "dock-toggle",
            ];
            let rects: Vec<Rect> = ids
                .iter()
                .map(|id| {
                    h.ui.widget_rect(W::new(*id))
                        .unwrap_or_else(|| panic!("{w}x{h_px}: {id} on screen"))
                })
                .collect();
            for (i, pair) in rects.windows(2).enumerate() {
                assert!(
                    pair[0].right() <= pair[1].x + 0.01,
                    "{w}x{h_px}: {} {:?} overlaps {} {:?}",
                    ids[i],
                    pair[0],
                    ids[i + 1],
                    pair[1]
                );
            }
            // The step size sits over the channel names, below the toolbar.
            let steps = h.rect(W::new("rack-steps"));
            let kick = h.channel("Kick");
            let name = h.rect(W::new(("rack-name", kick)));
            assert!(steps.y >= rects[1].bottom(), "{w}: below the toolbar");
            assert!(steps.bottom() <= name.y, "{w}: above the first channel");
            assert!(steps.x >= name.x - 0.01 && steps.right() <= name.right() + 0.01);
            h.ui.set_accessibility_active(true);
            h.frames(2);
            assert_eq!(h.label(W::new("rack-steps")), "Step size");
            assert_eq!(
                h.value(W::new("rack-steps")),
                "1/16",
                "{w}: sixteenths by default"
            );
            // The step menu opens and picks with the same layout.
            h.pick_step(Time::new(1, 12));
            assert!(h.ui.widget_rect(W::new("rack-swing-label")).is_some());
        }
    }
}
