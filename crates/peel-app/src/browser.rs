//! Browser: native instruments and effects, sample files and patterns.
//! Everything can be dragged onto the rack, mixer or playlist.

use std::path::{Path, PathBuf};

use peel_document::Id;
use peel_dsp::DeviceKind;
use peel_session::Session;
use peel_ui::Id as W;
use peel_ui::{Align, Color, CursorIcon, DragPayload, Rect, Ui, icons, vec2};

use crate::rack;
use crate::util::col;
use crate::workspace::{BrowserTab, DockTab, Focus, Workspace};

/// Payload for in-app drag and drop.
#[derive(Clone, Debug)]
pub enum BrowserItem {
    Device(&'static str),
    /// A device type with a named factory preset.
    Preset(&'static str, &'static str),
    Sample(PathBuf),
    Pattern(Id),
}

const ROW_H: f32 = 28.0;

#[derive(Default)]
struct DirCache {
    dir: Option<PathBuf>,
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
}

fn category_color(cat: &str) -> Color {
    let c = match cat {
        "Drums" => peel_document::PALETTE[0],
        "Synth" => peel_document::PALETTE[6],
        "Keys" => peel_document::PALETTE[4],
        "Sampler" => peel_document::PALETTE[2],
        "EQ" => peel_document::PALETTE[5],
        "Dynamics" => peel_document::PALETTE[1],
        "Delay" | "Reverb" => peel_document::PALETTE[7],
        "Distortion" => peel_document::PALETTE[9],
        "Filter" | "Modulation" => peel_document::PALETTE[3],
        _ => peel_document::PALETTE[11],
    };
    col(c)
}

fn category_icon(cat: &str) -> &'static str {
    match cat {
        "Drums" => icons::LIGHTNING,
        "Synth" => icons::WAVE_SAW,
        "Keys" => icons::PIANO_KEYS,
        "Sampler" => icons::WAVEFORM,
        "EQ" => icons::EQUALIZER,
        "Dynamics" => icons::SLIDERS,
        "Delay" => icons::REPEAT,
        "Reverb" => icons::DROP,
        "Distortion" => icons::LIGHTNING,
        "Filter" => icons::WAVE_SINE,
        "Modulation" => icons::WAVE_TRIANGLE,
        _ => icons::SLIDERS,
    }
}

pub fn draw(ui: &mut Ui, r: Rect, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    ui.scene.rect(r, t.radius_l, t.panel);
    let mut body = r.shrink(8.0);
    let tabs_r = body.cut_top(32.0);
    let tabs = [
        (BrowserTab::Instruments, icons::PIANO_KEYS, "Instruments"),
        (BrowserTab::Effects, icons::SLIDERS, "Effects"),
        (BrowserTab::Samples, icons::WAVEFORM, "Samples"),
        (BrowserTab::Patterns, icons::SQUARES, "Patterns"),
    ];
    let cells = tabs_r.columns(tabs.len(), 4.0);
    for ((tab, icon, tip), cell) in tabs.iter().zip(cells) {
        let b = ui.icon_button(
            W::new(("browser-tab", *tip)),
            cell,
            icon,
            ws.browser_tab == *tab,
            None,
        );
        ui.tooltip(&b, tip);
        if b.clicked {
            ws.browser_tab = *tab;
            ws.browser_scroll = 0.0;
        }
    }
    body.cut_top(8.0);
    let search_r = body.cut_top(30.0);
    let placeholder = match ws.browser_tab {
        BrowserTab::Instruments => "Search instruments",
        BrowserTab::Effects => "Search effects",
        BrowserTab::Samples => "Search this folder",
        BrowserTab::Patterns => "Search patterns",
    };
    ui.text_edit(
        W::new("browser-search"),
        search_r,
        &mut ws.search,
        placeholder,
    );
    body.cut_top(8.0);
    let query = ws.search.to_lowercase();
    match ws.browser_tab {
        BrowserTab::Instruments => devices(ui, body, DeviceKind::Instrument, &query, s, ws),
        BrowserTab::Effects => devices(ui, body, DeviceKind::Effect, &query, s, ws),
        BrowserTab::Samples => samples(ui, body, &query, s, ws),
        BrowserTab::Patterns => patterns(ui, body, &query, s, ws),
    }
}

#[derive(Default)]
struct Expanded(std::collections::HashSet<&'static str>);

enum Row {
    Heading(&'static str),
    Device(&'static peel_dsp::DeviceDescriptor, bool),
    Preset(&'static peel_dsp::DeviceDescriptor, &'static str),
}

fn devices(
    ui: &mut Ui,
    r: Rect,
    kind: DeviceKind,
    query: &str,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let expanded_id = W::new("browser-expanded");
    if let Ok(t) = std::env::var("PEEL_EXPAND")
        && let Some(d) = peel_devices::descriptor(&t)
    {
        ui.state::<Expanded>(expanded_id).0.insert(d.type_id);
    }
    let mut list: Vec<&'static peel_dsp::DeviceDescriptor> =
        peel_devices::all().filter(|d| d.kind == kind).collect();
    list.sort_by_key(|d| d.category);
    // Build rows; a search matches devices, categories and preset names.
    let mut rows: Vec<Row> = Vec::new();
    let mut last = "";
    for d in &list {
        let presets = peel_devices::presets(d.type_id);
        let name_hit = query.is_empty()
            || d.name.to_lowercase().contains(query)
            || d.category.to_lowercase().contains(query);
        let preset_hits: Vec<&'static str> = presets
            .iter()
            .map(|p| p.name)
            .filter(|n| !query.is_empty() && n.to_lowercase().contains(query))
            .collect();
        if !name_hit && preset_hits.is_empty() {
            continue;
        }
        if d.category != last {
            rows.push(Row::Heading(d.category));
            last = d.category;
        }
        let open =
            !preset_hits.is_empty() || ui.state::<Expanded>(expanded_id).0.contains(d.type_id);
        rows.push(Row::Device(d, open));
        if open {
            let shown: Vec<&'static str> = if preset_hits.is_empty() {
                presets.iter().map(|p| p.name).collect()
            } else {
                preset_hits
            };
            for p in shown {
                rows.push(Row::Preset(d, p));
            }
        }
    }
    if rows.is_empty() {
        ui.text(
            r.with_h(40.0),
            "Nothing matches",
            t.body,
            t.text_faint,
            Align::Center,
        );
        return;
    }
    let content_h = rows.len() as f32 * ROW_H + 8.0;
    let mut off = vec2(0.0, ws.browser_scroll);
    ui.scroll_area(W::new("browser-scroll"), r, vec2(r.w, content_h), &mut off);
    ws.browser_scroll = off.y;
    ui.scene.push_clip(r);
    let mut y = r.y - off.y;
    for row_kind in rows {
        let row = Rect {
            x: r.x,
            y,
            w: r.w - 8.0,
            h: ROW_H,
        };
        y += ROW_H;
        if row.bottom() < r.y || row.y > r.bottom() {
            continue;
        }
        match row_kind {
            Row::Heading(h) => {
                ui.text(
                    Rect {
                        y: row.y + 6.0,
                        h: row.h - 6.0,
                        ..row.shrink2(6.0, 0.0)
                    },
                    h,
                    t.caption,
                    t.text_faint,
                    Align::Start,
                );
            }
            Row::Device(d, open) => {
                let id = W::new(("browser-dev", d.type_id));
                let mut cells = row.shrink2(4.0, 0.0);
                let has_presets = !peel_devices::presets(d.type_id).is_empty();
                let caret_r = cells.cut_left(16.0);
                let resp = ui.interact(
                    id,
                    Rect {
                        x: caret_r.right(),
                        w: row.right() - caret_r.right(),
                        ..row
                    },
                );
                let hot = ui.animate_bool(id, resp.hovered);
                if hot > 0.0 {
                    ui.scene.rect(row, t.radius_m, t.raised.alpha(hot));
                }
                if has_presets {
                    let caret = ui.interact(id.with("caret"), caret_r);
                    ui.icon(
                        caret_r,
                        if open {
                            icons::CARET_DOWN
                        } else {
                            icons::CARET_RIGHT
                        },
                        11.0,
                        if caret.hovered { t.text } else { t.text_faint },
                    );
                    ui.tooltip(&caret, "Presets");
                    if caret.clicked {
                        let set = &mut ui.state::<Expanded>(expanded_id).0;
                        if !set.remove(d.type_id) {
                            set.insert(d.type_id);
                        }
                    }
                }
                let cc = category_color(d.category);
                let icon_r = cells.cut_left(22.0);
                ui.scene.rect(
                    Rect::from_center(icon_r.center(), 22.0, 22.0),
                    5.0,
                    cc.alpha(0.16),
                );
                ui.icon(icon_r, category_icon(d.category), 13.0, cc);
                cells.cut_left(8.0);
                ui.text(cells, d.name, t.label, t.text, Align::Start);
                if hot > 0.2 {
                    ui.icon(
                        cells.cut_right(18.0),
                        icons::PLUS,
                        13.0,
                        t.text_dim.alpha(hot),
                    );
                }
                ui.tooltip(&resp, d.description);
                if resp.hovered {
                    ui.set_cursor(CursorIcon::Grab);
                }
                if resp.dragging && ui.drag_distance() > 6.0 && ui.dragging_payload().is_none() {
                    ui.start_drag(DragPayload {
                        data: Box::new(BrowserItem::Device(d.type_id)),
                        label: d.name.to_string(),
                        color: cc,
                    });
                }
                if resp.clicked {
                    add_device(s, ws, d, None);
                }
            }
            Row::Preset(d, preset) => {
                let id = W::new(("browser-preset", d.type_id, preset));
                let resp = ui.interact(id, row);
                let hot = ui.animate_bool(id, resp.hovered);
                if hot > 0.0 {
                    ui.scene.rect(row, t.radius_m, t.raised.alpha(hot * 0.8));
                }
                let mut cells = row.shrink2(4.0, 0.0);
                cells.cut_left(16.0 + 22.0 + 8.0);
                let cc = category_color(d.category);
                let dot = cells.cut_left(12.0);
                ui.scene
                    .circle(dot.center() - vec2(4.0, 0.0), 2.5, cc.alpha(0.8));
                ui.text(
                    cells,
                    preset,
                    t.body,
                    t.text_dim.mix(t.text, hot),
                    Align::Start,
                );
                if resp.hovered {
                    ui.set_cursor(CursorIcon::Grab);
                }
                if resp.dragging && ui.drag_distance() > 6.0 && ui.dragging_payload().is_none() {
                    ui.start_drag(DragPayload {
                        data: Box::new(BrowserItem::Preset(d.type_id, preset)),
                        label: preset.to_string(),
                        color: cc,
                    });
                }
                if resp.clicked {
                    add_device(s, ws, d, Some(preset));
                }
            }
        }
    }
    ui.scene.pop_clip();
}

fn add_device(
    s: &mut Session,
    ws: &mut Workspace,
    d: &'static peel_dsp::DeviceDescriptor,
    preset: Option<&str>,
) {
    match d.kind {
        DeviceKind::Instrument => {
            if let Some(id) = rack::add_instrument_preset(s, d.type_id, preset) {
                ws.selected_channel = Some(id);
                ws.focus = Some(Focus::Channel(id));
                ws.dock = DockTab::Rack;
            }
        }
        DeviceKind::Effect => {
            // Insert on the focused mixer track, or the selected channel's track.
            let track = match ws.focus {
                Some(Focus::Track(t)) => Some(t),
                _ => ws
                    .selected_channel
                    .and_then(|c| s.project().channel(c))
                    .map(|c| c.track),
            };
            if let Some(track) = track {
                crate::mixer::add_insert_preset(s, track, d.type_id, preset);
                ws.focus = Some(Focus::Track(track));
            }
        }
    }
}

fn read_dir(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let hidden = p
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if p.is_dir() {
                dirs.push(p);
            } else if peel_media::is_audio_file(&p) {
                files.push(p);
            }
        }
    }
    dirs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    (dirs, files)
}

fn samples(ui: &mut Ui, r: Rect, query: &str, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let mut body = r;
    let nav = body.cut_top(28.0);
    body.cut_top(4.0);
    let Some(dir) = ws.sample_dir.clone() else {
        return;
    };
    let cache_id = W::new("browser-dir-cache");
    if ui.state::<DirCache>(cache_id).dir.as_ref() != Some(&dir) {
        let (dirs, files) = read_dir(&dir);
        let c = ui.state::<DirCache>(cache_id);
        c.dir = Some(dir.clone());
        c.dirs = dirs;
        c.files = files;
    }
    let mut nav_row = nav;
    let up = ui.icon_button(
        W::new("browser-up"),
        nav_row.cut_left(28.0),
        icons::CARET_LEFT,
        false,
        None,
    );
    ui.tooltip(&up, "Parent folder");
    if up.clicked
        && let Some(parent) = dir.parent()
    {
        ws.sample_dir = Some(parent.to_path_buf());
        ws.browser_scroll = 0.0;
    }
    let pick = ui.icon_button(
        W::new("browser-pick"),
        nav_row.cut_right(28.0),
        icons::FOLDER_OPEN,
        false,
        None,
    );
    ui.tooltip(&pick, "Choose a folder");
    if pick.clicked
        && let Some(p) = crate::dialogs::folder(&dir)
    {
        ws.sample_dir = Some(p);
        ws.browser_scroll = 0.0;
    }
    nav_row.cut_left(4.0);
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string());
    let name = ui.fit_text(&name, t.label, nav_row.w);
    ui.text(nav_row, &name, t.label, t.text_dim, Align::Start);

    let (dirs, files) = {
        let c = ui.state::<DirCache>(cache_id);
        (c.dirs.clone(), c.files.clone())
    };
    let matches = |p: &PathBuf| {
        query.is_empty()
            || p.file_name()
                .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(query))
    };
    let dirs: Vec<PathBuf> = dirs.into_iter().filter(matches).collect();
    let files: Vec<PathBuf> = files.into_iter().filter(matches).collect();
    let total = dirs.len() + files.len();
    let mut off = vec2(0.0, ws.browser_scroll);
    ui.scroll_area(
        W::new("browser-scroll"),
        body,
        vec2(body.w, total as f32 * ROW_H + 8.0),
        &mut off,
    );
    ws.browser_scroll = off.y;
    if total == 0 {
        ui.text(
            body.with_h(48.0),
            "No audio files here",
            t.body,
            t.text_faint,
            Align::Center,
        );
    }
    ui.scene.push_clip(body);
    let mut y = body.y - off.y;
    for (i, p) in dirs.iter().chain(files.iter()).enumerate() {
        let row = Rect {
            x: body.x,
            y,
            w: body.w - 8.0,
            h: ROW_H,
        };
        y += ROW_H;
        if row.bottom() < body.y || row.y > body.bottom() {
            continue;
        }
        let is_dir = i < dirs.len();
        let id = W::new(("browser-file", p));
        let resp = ui.interact(id, row);
        let hot = ui.animate_bool(id, resp.hovered);
        if hot > 0.0 {
            ui.scene.rect(row, t.radius_m, t.raised.alpha(hot));
        }
        let mut cells = row.shrink2(8.0, 0.0);
        let icon = if is_dir {
            icons::FOLDER
        } else {
            icons::WAVEFORM
        };
        ui.icon(
            cells.cut_left(18.0),
            icon,
            14.0,
            if is_dir { t.text_dim } else { t.accent },
        );
        cells.cut_left(8.0);
        let fname = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let fname = ui.fit_text(&fname, t.body, cells.w);
        ui.text(cells, &fname, t.body, t.text, Align::Start);
        if resp.hovered {
            ui.set_cursor(if is_dir {
                CursorIcon::Pointer
            } else {
                CursorIcon::Grab
            });
        }
        if is_dir {
            if resp.clicked {
                ws.sample_dir = Some(p.clone());
                ws.browser_scroll = 0.0;
            }
        } else {
            if resp.pressed {
                s.preview_file(p);
            }
            if resp.dragging && ui.drag_distance() > 6.0 && ui.dragging_payload().is_none() {
                let label = p
                    .file_stem()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ui.start_drag(DragPayload {
                    data: Box::new(BrowserItem::Sample(p.clone())),
                    label,
                    color: t.accent,
                });
            }
            ui.tooltip(&resp, "Click to listen, drag onto the Channel Rack");
        }
    }
    ui.scene.pop_clip();
}

fn patterns(ui: &mut Ui, r: Rect, query: &str, s: &mut Session, ws: &mut Workspace) {
    let t = ui.theme.clone();
    let list: Vec<(Id, String, peel_document::Color, usize, f64)> = s
        .project()
        .patterns
        .iter()
        .filter(|p| query.is_empty() || p.name.to_lowercase().contains(query))
        .map(|p| {
            (
                p.id,
                p.name.clone(),
                p.color,
                s.project().pattern_usage(p.id),
                p.length.as_f64(),
            )
        })
        .collect();
    let selected = s.selected_pattern();
    let row_h = 34.0;
    let mut off = vec2(0.0, ws.browser_scroll);
    ui.scroll_area(
        W::new("browser-scroll"),
        r,
        vec2(r.w, list.len() as f32 * row_h + 8.0),
        &mut off,
    );
    ws.browser_scroll = off.y;
    ui.scene.push_clip(r);
    let mut y = r.y - off.y;
    for (pid, name, color, uses, len) in list {
        let row = Rect {
            x: r.x,
            y,
            w: r.w - 8.0,
            h: row_h - 4.0,
        };
        y += row_h;
        let id = W::new(("browser-pattern", pid));
        let resp = ui.interact(id, row);
        let hot = ui.animate_bool(id, resp.hovered);
        let on = pid == selected;
        ui.scene.rect(
            row,
            t.radius_m,
            if on {
                t.raised
            } else {
                t.raised.alpha(hot * 0.7)
            },
        );
        let c = col(color);
        let mut cells = row.shrink2(8.0, 0.0);
        let sw = cells.cut_left(14.0);
        ui.scene
            .rect(Rect::from_center(sw.center(), 14.0, 14.0), 4.0, c);
        cells.cut_left(10.0);
        let info = format!(
            "{}  ×{}",
            crate::util::format_length(peel_document::Time::from_f64(len, 4)),
            uses
        );
        let info_w = ui.measure(&info, t.caption).x;
        let info_r = cells.cut_right(info_w + 2.0);
        ui.text(info_r, &info, t.caption, t.text_faint, Align::End);
        let name = ui.fit_text(&name, t.label, cells.w - 6.0);
        ui.text(cells, &name, t.label, t.text, Align::Start);
        if resp.pressed {
            s.select_pattern(pid);
        }
        if resp.dragging && ui.drag_distance() > 6.0 && ui.dragging_payload().is_none() {
            ui.start_drag(DragPayload {
                data: Box::new(BrowserItem::Pattern(pid)),
                label: name.clone(),
                color: c,
            });
        }
        if resp.hovered {
            ui.set_cursor(CursorIcon::Grab);
        }
        ui.tooltip(&resp, "Drag into the playlist");
    }
    ui.scene.pop_clip();
}
