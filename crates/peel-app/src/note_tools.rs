//! Piano Roll note tools.
//!
//! The panel previews the chosen tool on the selected notes (or every note
//! of the channel when nothing is selected) and re-applies it to the
//! original notes whenever a setting changes, so the result can be heard
//! while it is shaped. All previews share one undo gesture: Apply keeps it
//! as a single undo step, Cancel undoes it. Clicking outside the panel
//! applies.

use std::collections::HashSet;

use peel_compose::{ArpOrder, StrumDirection, Tool};
use peel_document::{Id, Note, Time};
use peel_session::Session;
use peel_ui::{Align, Id as W, Rect, Ui};

use crate::inspector::knob_cell;
use crate::workspace::Workspace;

const PANEL: W = W(0x6e07_e700_0015);
const TOOLS: [&str; 10] = [
    "Legato",
    "Articulate",
    "Chop",
    "Arpeggiate",
    "Strum",
    "Flam",
    "Humanize",
    "Velocity",
    "Flip",
    "Tidy",
];
const DIVISIONS: [(i64, i64); 4] = [(1, 32), (1, 16), (1, 8), (1, 4)];
const DIVISION_LABELS: [&str; 4] = ["1/32", "1/16", "1/8", "1/4"];

/// Tool settings, kept between uses.
#[derive(Clone, Debug)]
struct Settings {
    tool: usize,
    gap: usize,
    gate: f32,
    chop_division: usize,
    chop_gate: f32,
    arp_step: usize,
    arp_order: usize,
    arp_octaves: usize,
    arp_gate: f32,
    strum_spread: f32,
    strum_direction: usize,
    strum_velocity: f32,
    flam_count: usize,
    flam_spacing: usize,
    flam_velocity: f32,
    humanize_timing: f32,
    humanize_velocity: f32,
    humanize_length: f32,
    seed: u64,
    multiply: f32,
    offset: f32,
    flip: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            tool: 0,
            gap: 0,
            gate: 0.5,
            chop_division: 1,
            chop_gate: 0.8,
            arp_step: 1,
            arp_order: 0,
            arp_octaves: 0,
            arp_gate: 0.9,
            strum_spread: 0.125,
            strum_direction: 0,
            strum_velocity: 0.2,
            flam_count: 0,
            flam_spacing: 0,
            flam_velocity: 0.5,
            humanize_timing: 0.02,
            humanize_velocity: 0.08,
            humanize_length: 0.1,
            seed: 1,
            multiply: 1.0,
            offset: 0.0,
            flip: 0,
        }
    }
}

fn division(i: usize) -> Time {
    let (n, d) = DIVISIONS[i.min(DIVISIONS.len() - 1)];
    Time::new(n, d)
}

impl Settings {
    fn tool(&self) -> Tool {
        match self.tool {
            0 => Tool::Legato {
                gap: if self.gap == 0 {
                    Time::ZERO
                } else {
                    division(self.gap - 1)
                },
            },
            1 => Tool::Gate { amount: self.gate },
            2 => Tool::Chop {
                division: division(self.chop_division),
                gate: self.chop_gate,
            },
            3 => Tool::Arpeggiate {
                step: division(self.arp_step),
                order: [
                    ArpOrder::Up,
                    ArpOrder::Down,
                    ArpOrder::UpDown,
                    ArpOrder::Random,
                ][self.arp_order.min(3)],
                octaves: self.arp_octaves as u8 + 1,
                gate: self.arp_gate,
                seed: self.seed,
            },
            4 => Tool::Strum {
                spread: Time::from_f64(self.strum_spread as f64, 960),
                direction: [
                    StrumDirection::Up,
                    StrumDirection::Down,
                    StrumDirection::Alternate,
                ][self.strum_direction.min(2)],
                velocity: self.strum_velocity,
            },
            5 => Tool::Flam {
                count: self.flam_count as u8 + 1,
                spacing: division(self.flam_spacing),
                velocity: self.flam_velocity,
            },
            6 => Tool::Humanize {
                timing: Time::from_f64(self.humanize_timing as f64, 960),
                velocity: self.humanize_velocity,
                length: self.humanize_length,
                seed: self.seed,
            },
            7 => Tool::Levels {
                multiply: self.multiply,
                offset: self.offset,
            },
            8 => Tool::Flip {
                time: self.flip != 1,
                pitch: self.flip != 0,
            },
            _ => Tool::Tidy,
        }
    }
}

/// A panel session: what the tool works on.
struct Active {
    pattern: Id,
    original: Vec<Note>,
    /// Notes the last preview wrote.
    current: Vec<Id>,
    gesture: u64,
    applied: bool,
}

#[derive(Default)]
struct State {
    settings: Settings,
    active: Option<Active>,
    uses: u64,
}

/// Opens the panel for `targets` in pattern `pattern`.
pub fn open(
    ui: &mut Ui,
    anchor: Rect,
    s: &mut Session,
    ws: &mut Workspace,
    pattern: Id,
    targets: Vec<Note>,
) {
    if targets.is_empty() {
        s.notify("Add some notes first", true);
        return;
    }
    let st = ui.state::<State>(PANEL);
    st.uses += 1;
    let gesture = 0x7001_5000_0000 + st.uses;
    st.active = Some(Active {
        pattern,
        current: targets.iter().map(|n| n.id).collect(),
        original: targets,
        gesture,
        applied: false,
    });
    let screen = ui.screen();
    let r = Rect {
        x: (anchor.right() - 480.0).max(screen.x + 8.0),
        y: anchor.bottom() + 6.0,
        w: 480.0,
        h: 316.0,
    };
    ui.open_popup(PANEL, r);
    preview(ui, s, ws);
}

/// Chooses the tool the panel opens with.
pub fn select(ui: &mut Ui, tool: usize) {
    ui.state::<State>(PANEL).settings.tool = tool.min(TOOLS.len() - 1);
}

pub fn is_open(ui: &Ui) -> bool {
    ui.is_popup_open(PANEL)
}

fn preview(ui: &mut Ui, s: &mut Session, ws: &mut Workspace) {
    let st = ui.state::<State>(PANEL);
    let tool = st.settings.tool();
    let Some(a) = st.active.as_mut() else { return };
    let result = peel_compose::apply(&a.original, &tool);
    let mut gone: HashSet<Id> = a.original.iter().map(|n| n.id).collect();
    gone.extend(a.current.iter().copied());
    let pid = a.pattern;
    let label = tool.name();
    let added = result.clone();
    s.doc.edit_gesture(a.gesture, label, |p| {
        if let Some(pt) = p.pattern_mut(pid) {
            pt.notes.retain(|n| !gone.contains(&n.id));
            pt.notes.extend(added);
            pt.sort_notes();
        }
    });
    a.current = result.iter().map(|n| n.id).collect();
    a.applied = true;
    ws.roll.selection = a.current.iter().copied().collect();
}

fn finish(ui: &mut Ui, s: &mut Session, ws: &mut Workspace, keep: bool) {
    let name = ui.state::<State>(PANEL).settings.tool().name();
    let Some(a) = ui.state::<State>(PANEL).active.take() else {
        return;
    };
    if a.applied {
        s.doc.end_gesture();
        // The step is named after the tool that was applied, not the one
        // the preview started with.
        s.doc.relabel_last(name);
        if !keep {
            s.doc.undo();
            ws.roll.selection = a.original.iter().map(|n| n.id).collect();
        }
    }
}

/// Draws the panel while it is open. Call every frame from the Piano Roll.
pub fn panel(ui: &mut Ui, s: &mut Session, ws: &mut Workspace) {
    let open = ui.is_popup_open(PANEL);
    if !open {
        // Closed by clicking outside: keep the result.
        if ui.state::<State>(PANEL).active.is_some() {
            finish(ui, s, ws, true);
        }
        return;
    }
    let r = ui.popup_rect().unwrap_or_default();
    let t = ui.theme.clone();
    let mut settings = ui.state::<State>(PANEL).settings.clone();
    let before = format!("{settings:?}");
    let mut decision: Option<bool> = None;
    let count = ui
        .state::<State>(PANEL)
        .active
        .as_ref()
        .map_or(0, |a| a.original.len());
    ui.popup_panel(PANEL, r, |ui, r| {
        let mut body = r.shrink(14.0);
        // Tool list.
        let mut list = body.cut_left(124.0);
        body.cut_left(14.0);
        for (i, name) in TOOLS.iter().enumerate() {
            let row = list.cut_top(27.0);
            let id = W::new(("note-tool", i));
            let resp = ui.interact_button(id, row, name);
            let on = settings.tool == i;
            if on || resp.hovered {
                ui.scene.rect(
                    row,
                    t.radius_m,
                    if on { t.control } else { t.control.alpha(0.5) },
                );
            }
            if on {
                ui.scene.rect(
                    Rect {
                        w: 3.0,
                        ..row.shrink2(0.0, 6.0)
                    },
                    1.5,
                    t.accent,
                );
            }
            ui.text(
                row.shrink2(12.0, 0.0),
                name,
                t.label,
                if on { t.text } else { t.text_dim },
                Align::Start,
            );
            if resp.clicked {
                settings.tool = i;
            }
        }
        // Buttons.
        let mut buttons = body.cut_bottom(32.0);
        body.cut_bottom(10.0);
        let apply_r = buttons.cut_right(96.0);
        buttons.cut_right(8.0);
        let cancel_r = buttons.cut_right(88.0);
        if ui
            .button_primary(W::new("note-tool-apply"), apply_r, "Apply")
            .clicked
        {
            decision = Some(true);
        }
        if ui
            .button(W::new("note-tool-cancel"), cancel_r, "Cancel")
            .clicked
        {
            decision = Some(false);
        }
        let what = format!("{count} note{}", if count == 1 { "" } else { "s" });
        ui.text(buttons, &what, t.caption, t.text_faint, Align::Start);
        // Settings for the chosen tool.
        ui.text(
            body.cut_top(24.0),
            TOOLS[settings.tool],
            t.heading,
            t.text,
            Align::Start,
        );
        body.cut_top(8.0);
        tool_settings(ui, body, &mut settings);
    });
    let changed = format!("{settings:?}") != before;
    ui.state::<State>(PANEL).settings = settings;
    if changed {
        preview(ui, s, ws);
    }
    if let Some(keep) = decision {
        finish(ui, s, ws, keep);
        ui.close_popup();
    }
}

/// A labelled segmented row. Returns the row's remaining space.
fn choice(
    ui: &mut Ui,
    body: &mut Rect,
    id: &str,
    label: &str,
    options: &[&str],
    value: &mut usize,
) {
    let t = ui.theme.clone();
    let mut row = body.cut_top(30.0);
    body.cut_top(8.0);
    ui.text(
        row.cut_left(72.0),
        label,
        t.caption,
        t.text_dim,
        Align::Start,
    );
    ui.segmented(W::new(("note-tool-choice", id)), row, options, value);
}

/// A knob for `value` in `lo..=hi`.
#[allow(clippy::too_many_arguments)]
fn knob(
    ui: &mut Ui,
    cell: Rect,
    id: &str,
    label: &str,
    value: &mut f32,
    lo: f32,
    hi: f32,
    default: f32,
    text: &str,
) {
    let t = ui.theme.clone();
    let mut norm = (*value - lo) / (hi - lo);
    let d = (default - lo) / (hi - lo);
    let resp = knob_cell(
        ui,
        W::new(("note-tool-knob", id)),
        cell,
        &mut norm,
        d,
        t.accent,
        lo < 0.0 && hi > 0.0,
        label,
        text,
    );
    if resp.changed {
        *value = lo + norm.clamp(0.0, 1.0) * (hi - lo);
    }
}

fn knob_row(body: &mut Rect, n: usize) -> Vec<Rect> {
    let row = body.cut_top(70.0);
    (0..n)
        .map(|i| Rect {
            x: row.x + i as f32 * 82.0,
            w: 82.0,
            ..row
        })
        .collect()
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

fn beats(v: f32) -> String {
    // In sixteenths, the unit the grid shows.
    format!("{:.2} / 16", v * 4.0)
}

fn tool_settings(ui: &mut Ui, mut body: Rect, s: &mut Settings) {
    let t = ui.theme.clone();
    match s.tool {
        0 => {
            choice(
                ui,
                &mut body,
                "gap",
                "Gap",
                &["None", "1/32", "1/16", "1/8"],
                &mut s.gap,
            );
            hint(ui, body, "Each note lasts until the next one starts.");
        }
        1 => {
            let c = knob_row(&mut body, 1);
            let v = s.gate;
            knob(
                ui,
                c[0],
                "gate",
                "Length",
                &mut s.gate,
                0.1,
                2.0,
                1.0,
                &pct(v),
            );
            hint(ui, body, "Shorter for staccato, longer to overlap.");
        }
        2 => {
            choice(
                ui,
                &mut body,
                "chop",
                "Every",
                &DIVISION_LABELS,
                &mut s.chop_division,
            );
            let c = knob_row(&mut body, 1);
            let v = s.chop_gate;
            knob(
                ui,
                c[0],
                "chop-gate",
                "Gate",
                &mut s.chop_gate,
                0.1,
                1.0,
                0.8,
                &pct(v),
            );
        }
        3 => {
            choice(
                ui,
                &mut body,
                "arp-step",
                "Step",
                &DIVISION_LABELS,
                &mut s.arp_step,
            );
            choice(
                ui,
                &mut body,
                "arp-order",
                "Order",
                &["Up", "Down", "Up-down", "Random"],
                &mut s.arp_order,
            );
            choice(
                ui,
                &mut body,
                "arp-oct",
                "Octaves",
                &["1", "2", "3"],
                &mut s.arp_octaves,
            );
            let c = knob_row(&mut body, 1);
            let v = s.arp_gate;
            knob(
                ui,
                c[0],
                "arp-gate",
                "Gate",
                &mut s.arp_gate,
                0.1,
                1.0,
                0.9,
                &pct(v),
            );
            if s.arp_order == 3 {
                reseed(ui, c[0].translate(peel_ui::vec2(90.0, 18.0)), s);
            }
        }
        4 => {
            choice(
                ui,
                &mut body,
                "strum-dir",
                "Direction",
                &["Up", "Down", "Alternate"],
                &mut s.strum_direction,
            );
            let c = knob_row(&mut body, 2);
            let (a, b) = (s.strum_spread, s.strum_velocity);
            knob(
                ui,
                c[0],
                "strum-spread",
                "Spread",
                &mut s.strum_spread,
                0.0,
                0.5,
                0.125,
                &beats(a),
            );
            knob(
                ui,
                c[1],
                "strum-vel",
                "Fade",
                &mut s.strum_velocity,
                0.0,
                1.0,
                0.2,
                &pct(b),
            );
        }
        5 => {
            choice(
                ui,
                &mut body,
                "flam-count",
                "Hits",
                &["1", "2", "3"],
                &mut s.flam_count,
            );
            choice(
                ui,
                &mut body,
                "flam-space",
                "Spacing",
                &DIVISION_LABELS[..3],
                &mut s.flam_spacing,
            );
            let c = knob_row(&mut body, 1);
            let v = s.flam_velocity;
            knob(
                ui,
                c[0],
                "flam-vel",
                "Velocity",
                &mut s.flam_velocity,
                0.05,
                1.0,
                0.5,
                &pct(v),
            );
        }
        6 => {
            let c = knob_row(&mut body, 3);
            let (a, b, d) = (s.humanize_timing, s.humanize_velocity, s.humanize_length);
            knob(
                ui,
                c[0],
                "hum-time",
                "Timing",
                &mut s.humanize_timing,
                0.0,
                0.125,
                0.02,
                &beats(a),
            );
            knob(
                ui,
                c[1],
                "hum-vel",
                "Velocity",
                &mut s.humanize_velocity,
                0.0,
                0.3,
                0.08,
                &pct(b),
            );
            knob(
                ui,
                c[2],
                "hum-len",
                "Length",
                &mut s.humanize_length,
                0.0,
                0.5,
                0.1,
                &pct(d),
            );
            body.cut_top(6.0);
            reseed(ui, body.cut_top(30.0).with_w(140.0), s);
        }
        7 => {
            let c = knob_row(&mut body, 2);
            let (a, b) = (s.multiply, s.offset);
            knob(
                ui,
                c[0],
                "lev-mul",
                "Scale",
                &mut s.multiply,
                0.0,
                2.0,
                1.0,
                &pct(a),
            );
            knob(
                ui,
                c[1],
                "lev-off",
                "Offset",
                &mut s.offset,
                -0.5,
                0.5,
                0.0,
                &format!("{:+.0}", b * 100.0),
            );
            hint(ui, body, "Scales every velocity, then adds the offset.");
        }
        8 => {
            choice(
                ui,
                &mut body,
                "flip",
                "Flip",
                &["Time", "Pitch", "Both"],
                &mut s.flip,
            );
            hint(
                ui,
                body,
                "Time plays the notes backwards; pitch mirrors them around their middle.",
            );
        }
        _ => {
            ui.text_wrapped(
                body.min(),
                body.w,
                "Removes notes that repeat another at the same pitch and time, and shortens same-pitch notes that run into the next.",
                t.body,
                t.text_dim,
            );
        }
    }
}

fn hint(ui: &mut Ui, body: Rect, text: &str) {
    let t = ui.theme.clone();
    ui.text_wrapped(
        body.min() + peel_ui::vec2(0.0, 4.0),
        body.w,
        text,
        t.caption,
        t.text_faint,
    );
}

fn reseed(ui: &mut Ui, r: Rect, s: &mut Settings) {
    if ui
        .button(W::new("note-tool-seed"), r, "New variation")
        .clicked
    {
        s.seed = s
            .seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
    }
}
