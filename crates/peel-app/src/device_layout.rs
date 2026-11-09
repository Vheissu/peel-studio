//! Layout and preset helpers for descriptor-driven device panels.
//!
//! Narrow panels (the inspector) stack each parameter group full width.
//! Panels at least [`FLOW_MIN_CELLS`] knob cells wide (the instrument and
//! effect sheets) flow groups left to right in descriptor order and wrap,
//! so a three-knob group no longer leaves a mostly empty row.

use peel_document::{DeviceInstance, Id};
use peel_dsp::{DeviceDescriptor, ParamDescriptor, Preset};
use peel_session::Session;
use peel_ui::Id as W;
use peel_ui::{Align, MenuItem, Rect, Ui, icons, vec2};

use crate::inspector::{CELL_H, CELL_W};

/// Width in cells from which groups flow side by side.
pub(crate) const FLOW_MIN_CELLS: usize = 8;
/// Narrowest a cell may shrink to when packing a flowed line.
const MIN_CELL_W: f32 = 68.0;
/// Space between groups on one flowed line.
const GROUP_GAP: f32 = 16.0;
/// Group heading block, including the space below it.
pub(crate) const HEAD_H: f32 = 30.0;
/// One full-width choice row, including the space below it.
pub(crate) const WIDE_H: f32 = 34.0;
/// Space after each group.
const GROUP_PAD: f32 = 6.0;
/// Cells a group with a wide choice needs at least, so its selector has room.
const WIDE_MIN_CELLS: usize = 3;

/// One descriptor group: its wide choices and its knob-cell parameters.
pub(crate) struct Group {
    pub name: &'static str,
    pub wide: Vec<&'static ParamDescriptor>,
    pub knobs: Vec<&'static ParamDescriptor>,
}

/// Where a group sits, relative to the top left of the panel body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placed {
    pub group: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    /// Knob cells per row inside the group.
    pub cols: usize,
    pub cell_w: f32,
    /// On the first line, so the heading draws no rule above it.
    pub first: bool,
    /// Starts its line; it draws the line's rule across the whole panel.
    pub line_start: bool,
}

pub(crate) struct PanelLayout {
    pub groups: Vec<Group>,
    pub placed: Vec<Placed>,
    /// Height of all groups (without the panel's trailing padding).
    pub height: f32,
}

/// Enumerations with more than two choices get a full-width selector.
pub(crate) fn is_wide_choice(p: &ParamDescriptor) -> bool {
    p.is_choice() && (p.choices.len() > 2 || p.choices != ["Off", "On"])
}

/// Knobs per row and the stretched cell width for a body `w` wide.
pub(crate) fn knob_grid(w: f32) -> (usize, f32) {
    let per_row = ((w - 6.0) / CELL_W).floor().max(1.0) as usize;
    (per_row, (w - 6.0) / per_row as f32)
}

pub(crate) fn groups(desc: &'static DeviceDescriptor) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for p in desc.params {
        let i = match out.iter().position(|g| g.name == p.group) {
            Some(i) => i,
            None => {
                out.push(Group {
                    name: p.group,
                    wide: Vec::new(),
                    knobs: Vec::new(),
                });
                out.len() - 1
            }
        };
        if is_wide_choice(p) {
            out[i].wide.push(p);
        } else {
            out[i].knobs.push(p);
        }
    }
    out
}

fn group_h(g: &Group, cols: usize) -> f32 {
    HEAD_H
        + g.wide.len() as f32 * WIDE_H
        + g.knobs.len().div_ceil(cols.max(1)) as f32 * CELL_H
        + GROUP_PAD
}

/// Cells a group asks for on a flowed line.
fn group_cells(g: &Group) -> usize {
    let min = if g.wide.is_empty() { 0 } else { WIDE_MIN_CELLS };
    g.knobs.len().max(min).max(1)
}

/// Lays out every group of `desc` in a panel body `w` wide.
pub(crate) fn layout(desc: &'static DeviceDescriptor, w: f32) -> PanelLayout {
    let groups = groups(desc);
    let (per_row, cell_w) = knob_grid(w);
    let avail = w - 6.0;
    let mut placed = Vec::with_capacity(groups.len());
    let mut y = 0.0;
    if per_row < FLOW_MIN_CELLS {
        for (i, g) in groups.iter().enumerate() {
            placed.push(Placed {
                group: i,
                x: 0.0,
                y,
                w: avail,
                cols: per_row,
                cell_w,
                first: i == 0,
                line_start: true,
            });
            y += group_h(g, per_row);
        }
        return PanelLayout {
            groups,
            placed,
            height: y,
        };
    }
    // Greedy lines in descriptor order; never reorder groups.
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut line: Vec<usize> = Vec::new();
    let mut line_cells = 0;
    for (i, g) in groups.iter().enumerate() {
        let n = group_cells(g);
        if n as f32 * MIN_CELL_W > avail {
            // Too wide for any line: alone, wrapping its knobs inside.
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                line_cells = 0;
            }
            lines.push(vec![i]);
            continue;
        }
        let fits = (line_cells + n) as f32 * MIN_CELL_W + line.len() as f32 * GROUP_GAP <= avail;
        if !line.is_empty() && !fits {
            lines.push(std::mem::take(&mut line));
            line_cells = 0;
        }
        line.push(i);
        line_cells += n;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    for (li, line) in lines.iter().enumerate() {
        let first = li == 0;
        if let [only] = line[..]
            && group_cells(&groups[only]) as f32 * MIN_CELL_W > avail
        {
            placed.push(Placed {
                group: only,
                x: 0.0,
                y,
                w: avail,
                cols: per_row,
                cell_w,
                first,
                line_start: true,
            });
            y += group_h(&groups[only], per_row);
            continue;
        }
        let cells: usize = line.iter().map(|&i| group_cells(&groups[i])).sum();
        let gaps = (line.len() - 1) as f32 * GROUP_GAP;
        let cw = cell_w.min((avail - gaps) / cells as f32);
        let mut x = 0.0;
        let mut line_h: f32 = 0.0;
        for (k, &i) in line.iter().enumerate() {
            let n = group_cells(&groups[i]);
            let gw = n as f32 * cw;
            placed.push(Placed {
                group: i,
                x,
                y,
                w: gw,
                cols: n,
                cell_w: cw,
                first,
                line_start: k == 0,
            });
            line_h = line_h.max(group_h(&groups[i], n));
            x += gw + GROUP_GAP;
        }
        y += line_h;
    }
    PanelLayout {
        groups,
        placed,
        height: y,
    }
}

// ---- presets ----

/// Index of the device's loaded factory preset, if any.
pub(crate) fn preset_index(presets: &[Preset], dev: &DeviceInstance) -> Option<usize> {
    let name = dev.preset.as_deref()?;
    presets.iter().position(|p| p.name == name)
}

/// The value a parameter has right after loading `preset`.
fn preset_value(p: &ParamDescriptor, preset: &Preset) -> f32 {
    preset
        .params
        .iter()
        .find(|(k, _)| *k == p.key)
        .map_or(p.default, |(_, v)| *v)
}

/// True when any parameter differs from what loading `preset` would set.
pub(crate) fn is_edited(desc: &DeviceDescriptor, preset: &Preset, dev: &DeviceInstance) -> bool {
    desc.params.iter().any(|p| {
        let want = p.clamp(preset_value(p, preset));
        let have = p.clamp(dev.params.get(p.key).copied().unwrap_or(p.default));
        (want - have).abs() > 1.0e-5 * want.abs().max(1.0)
    })
}

/// Loads `preset` onto a device as one undoable edit: every parameter goes
/// back to its default, then the preset's values apply.
pub(crate) fn load_preset(s: &mut Session, device: Id, desc: &DeviceDescriptor, preset: &Preset) {
    s.doc.edit(format!("Load preset {}", preset.name), |p| {
        if let Some(d) = peel_session::find_device_mut(p, device) {
            for pd in desc.params {
                d.params.insert(pd.key.to_string(), pd.default);
            }
            for (k, v) in preset.params {
                d.params.insert((*k).to_string(), *v);
            }
            d.preset = Some(preset.name.to_string());
        }
    });
}

/// The preset `step` places from the loaded one, wrapping at the ends.
/// With none loaded, forward starts at the first and back at the last.
pub(crate) fn step_preset(len: usize, current: Option<usize>, step: i32) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let n = len as i32;
    Some(match current {
        Some(i) => (i as i32 + step).rem_euclid(n) as usize,
        None if step >= 0 => 0,
        None => len - 1,
    })
}

/// Value width from which the row shows previous and next buttons.
const ARROWS_MIN_W: f32 = 220.0;
/// Value width from which the row shows Edited and Revert beside them.
const EDITED_MIN_W: f32 = 420.0;

/// The preset row: label, previous, the preset menu, next, and (when the
/// values have moved off the loaded preset) Edited and Revert. Every load
/// goes through [`load_preset`], so each one is a single undo step.
pub(crate) fn preset_row(
    ui: &mut Ui,
    r: Rect,
    dev: &DeviceInstance,
    desc: &'static DeviceDescriptor,
    presets: &'static [Preset],
    s: &mut Session,
) {
    let t = ui.theme.clone();
    let mut row = r;
    ui.text(
        row.cut_left(72.0),
        "Preset",
        t.caption,
        t.text_faint,
        Align::Start,
    );
    let current = preset_index(presets, dev);
    let edited = current.is_some_and(|i| is_edited(desc, &presets[i], dev));
    let mut pick: Option<usize> = None;

    // Right side first: Revert and Edited (wide rows only), then next.
    if row.w >= EDITED_MIN_W {
        let revert_w = ui.quiet_button_width(false, "Revert", false);
        let edited_w = ui.measure("Edited", t.caption).x;
        let mut side = row.cut_right(revert_w + 8.0 + edited_w + 8.0);
        if edited {
            let rb = side.cut_right(revert_w);
            let revert = ui.quiet_button(
                W::new(("preset-revert", dev.id)),
                rb,
                None,
                "Revert",
                false,
                false,
            );
            ui.tooltip(&revert, "Reload the preset's values");
            if revert.clicked {
                pick = current;
            }
            side.cut_right(8.0);
            ui.text(
                side.cut_right(edited_w),
                "Edited",
                t.caption,
                t.text_faint,
                Align::End,
            );
        }
    }
    let arrows = row.w >= ARROWS_MIN_W;
    let (prev_r, next_r) = if arrows {
        let next = row.cut_right(28.0);
        row.cut_right(4.0);
        let prev = row.cut_left(28.0);
        row.cut_left(4.0);
        (Some(prev), Some(next))
    } else {
        (None, None)
    };
    for (r, step, icon, name) in [
        (prev_r, -1, icons::CARET_LEFT, "Previous preset"),
        (next_r, 1, icons::CARET_RIGHT, "Next preset"),
    ] {
        let Some(r) = r else { continue };
        let b = ui.icon_button(W::new(("preset-step", dev.id, step)), r, icon, false, None);
        ui.name(&b, name);
        ui.tooltip(&b, name);
        if b.clicked {
            pick = step_preset(presets.len(), current, step);
        }
    }

    // The menu. Narrow rows say "edited" in the value; choosing the
    // checked preset again reverts it.
    let id = W::new(("preset", dev.id));
    let mut label = dev.preset.clone().unwrap_or_else(|| "Default".into());
    let wide = r.w - 72.0 >= EDITED_MIN_W;
    if edited && !wide {
        label.push_str(" (edited)");
    }
    let resp = ui.dropdown(id, row, &label, "Preset");
    if resp.clicked {
        ui.open_popup(id.with("menu"), row);
    }
    let items: Vec<MenuItem> = presets
        .iter()
        .map(|p| MenuItem::new(p.name).checked(dev.preset.as_deref() == Some(p.name)))
        .collect();
    if let Some(i) = ui.menu(id.with("menu"), vec2(row.x, row.bottom() + 4.0), &items) {
        pick = Some(i);
    }
    if let Some(i) = pick {
        load_preset(s, dev.id, desc, &presets[i]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(type_id: &str) -> &'static DeviceDescriptor {
        peel_devices::descriptor(type_id).unwrap()
    }

    fn overlaps(a: &Placed, b: &Placed, ha: f32, hb: f32) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + hb && b.y < a.y + ha
    }

    #[test]
    fn narrow_panels_stack_every_group_full_width() {
        for d in peel_devices::all() {
            let l = layout(d, 300.0);
            assert_eq!(l.placed.len(), l.groups.len());
            for (i, p) in l.placed.iter().enumerate() {
                assert_eq!(p.group, i, "{}", d.type_id);
                assert_eq!(p.x, 0.0);
                assert_eq!(p.w, 294.0);
                assert_eq!(p.first, i == 0);
            }
        }
    }

    #[test]
    fn wide_panels_flow_in_order_without_overlap_or_overflow() {
        for w in [640.0, 770.0, 1096.0] {
            for d in peel_devices::all() {
                let l = layout(d, w);
                let order: Vec<usize> = l.placed.iter().map(|p| p.group).collect();
                assert_eq!(
                    order,
                    (0..l.groups.len()).collect::<Vec<_>>(),
                    "{}",
                    d.type_id
                );
                let heights: Vec<f32> = l
                    .placed
                    .iter()
                    .map(|p| group_h(&l.groups[p.group], p.cols))
                    .collect();
                for (i, a) in l.placed.iter().enumerate() {
                    assert!(
                        a.x + a.w <= w - 6.0 + 0.01,
                        "{} overflows at {w}",
                        d.type_id
                    );
                    assert!(
                        a.cell_w >= MIN_CELL_W - 0.01,
                        "{} cells too narrow",
                        d.type_id
                    );
                    let g = &l.groups[a.group];
                    assert!(g.knobs.len() <= a.cols * g.knobs.len().div_ceil(a.cols).max(1));
                    for (j, b) in l.placed.iter().enumerate().skip(i + 1) {
                        assert!(
                            !overlaps(a, b, heights[i], heights[j]),
                            "{}: groups {i} and {j} overlap at {w}",
                            d.type_id
                        );
                    }
                }
                let bottom = l
                    .placed
                    .iter()
                    .zip(&heights)
                    .map(|(p, h)| p.y + h)
                    .fold(0.0f32, f32::max);
                assert!((bottom - l.height).abs() < 0.01, "{}", d.type_id);
            }
        }
    }

    #[test]
    fn flowing_shortens_the_sheet_panel() {
        // A device with several small groups gets shorter at sheet width.
        let d = desc("peel.drum.tom");
        let stacked = layout(d, 300.0).height;
        let flowed = layout(d, 770.0);
        assert!(flowed.height < stacked);
        assert!(
            flowed.placed.iter().all(|p| p.first),
            "tom fits on one line"
        );
    }

    #[test]
    fn step_wraps_and_starts_from_the_ends() {
        assert_eq!(step_preset(0, None, 1), None);
        assert_eq!(step_preset(4, None, 1), Some(0));
        assert_eq!(step_preset(4, None, -1), Some(3));
        assert_eq!(step_preset(4, Some(3), 1), Some(0));
        assert_eq!(step_preset(4, Some(0), -1), Some(3));
        assert_eq!(step_preset(4, Some(1), 1), Some(2));
    }

    #[test]
    fn loading_marks_clean_editing_marks_edited_and_undo_restores() {
        let mut s = Session::new_silent(peel_session::template::beat_and_melody());
        let ch = s.project().channels[0].clone();
        let d = desc(&ch.device.type_id);
        let presets = peel_devices::presets(d.type_id);
        assert!(presets.len() >= 2, "first channel needs presets");
        let dev = ch.device.id;
        let find = |s: &Session| s.project().channel(ch.id).unwrap().device.clone();

        load_preset(&mut s, dev, d, &presets[1]);
        let loaded = find(&s);
        assert_eq!(preset_index(presets, &loaded), Some(1));
        assert!(!is_edited(d, &presets[1], &loaded));

        let p = d.params.iter().find(|p| !p.is_choice()).unwrap();
        let v = p.from_normalized(1.0 - p.to_normalized(preset_value(p, &presets[1])).round());
        s.set_param(dev, p.key, v, 77);
        s.doc.end_gesture();
        assert!(is_edited(d, &presets[1], &find(&s)));

        // Revert is a reload: clean again, and one undo returns the edit.
        load_preset(&mut s, dev, d, &presets[1]);
        assert!(!is_edited(d, &presets[1], &find(&s)));
        s.doc.undo();
        assert!(is_edited(d, &presets[1], &find(&s)));
        s.doc.undo();
        assert!(!is_edited(d, &presets[1], &find(&s)));
    }
}
