//! The large effect editor: a sheet over the workspace that opens from a
//! mixer insert. EQ and Mastering have dedicated editors; every other
//! effect gets its full descriptor panel at editor size.

use peel_document::{DeviceInstance, Id};
use peel_session::Session;
use peel_ui::Id as W;
use peel_ui::{
    Align, Color, CursorIcon, FocusKind, LayerId, Rect, Role, Semantics, Ui, icons, vec2,
};

use crate::util::col;
use crate::workspace::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FxTarget {
    pub track: Id,
    pub device: Id,
}

/// View state of the effect editor. Session only; never saved.
#[derive(Default)]
pub struct FxState {
    pub target: Option<FxTarget>,
    pub mastering: crate::mastering_ui::MasteringState,
    pub eq: crate::eq_editor::EqState,
    /// Live spectra and peak history of the open EQ or Mastering editor.
    pub analysis: crate::analysis_view::Analysis,
    /// Resonance candidates the user chose to hold, per EQ.
    pub held: std::collections::HashMap<Id, Vec<peel_engine::analysis::Resonance>>,
    chain_scroll: f32,
}

impl FxState {
    pub fn open(&mut self, track: Id, device: Id) {
        if self.target.map(|t| t.device) != Some(device) {
            self.eq = Default::default();
        }
        self.target = Some(FxTarget { track, device });
    }

    pub fn close(&mut self) {
        self.target = None;
    }

    pub fn is_open(&self) -> bool {
        self.target.is_some()
    }
}

/// Largest sheet; smaller windows use what they have.
const SHEET_MAX: (f32, f32) = (1120.0, 700.0);
const MARGIN: f32 = 12.0;

/// Where the sheet sits inside `area`.
pub fn sheet_rect(area: Rect) -> Rect {
    let w = (area.w - MARGIN * 2.0).min(SHEET_MAX.0);
    let h = (area.h - MARGIN * 2.0).min(SHEET_MAX.1);
    Rect::from_center(area.center(), w, h)
}

/// Finds the device and its current track, following it if it moved.
fn locate(s: &Session, target: FxTarget) -> Option<(Id, DeviceInstance)> {
    let p = s.project();
    if let Some(d) = p
        .mixer
        .track(target.track)
        .and_then(|t| t.inserts.iter().find(|d| d.id == target.device))
    {
        return Some((target.track, d.clone()));
    }
    p.mixer.tracks.iter().find_map(|t| {
        t.inserts
            .iter()
            .find(|d| d.id == target.device)
            .map(|d| (t.id, d.clone()))
    })
}

/// Draws the editor when one is open. `area` is everything between the top
/// and status bars.
pub fn draw(ui: &mut Ui, area: Rect, s: &mut Session, ws: &mut Workspace) {
    let Some(target) = ws.fx.target else {
        // Closed editors switch the analysis tap off.
        s.set_analysis_device(None);
        return;
    };
    let Some((track, dev)) = locate(s, target) else {
        ws.fx.close();
        s.set_analysis_device(None);
        return;
    };
    let analysed = matches!(dev.type_id.as_str(), "peel.fx.eq" | "peel.fx.mastering");
    ws.fx
        .analysis
        .sync(ui, s, analysed.then_some(dev.id));
    ws.fx.target = Some(FxTarget {
        track,
        device: dev.id,
    });
    let t = ui.theme.clone();
    let sheet = sheet_rect(area);
    ui.with_layer(LayerId::ABOVE, |ui| {
        ui.scene.push_clip_absolute(area);
        ui.scene.rect(area, 0.0, Color::rgba(0.03, 0.04, 0.06, 0.62));
        let scrim = ui.interact(W::new("fx-scrim"), area);
        ui.scene.shadow(sheet, t.radius_l, 18.0, t.shadow);
        ui.scene.rect(sheet, t.radius_l, t.panel);
        let swallow = ui.interact(W::new("fx-sheet"), sheet);
        if scrim.clicked && !ui.pointer().is_some_and(|p| sheet.contains(p)) && !swallow.hovered {
            ws.fx.close();
            ui.scene.pop_clip();
            return;
        }
        let mut body = sheet;
        let head = body.cut_top(t.header_h + 4.0);
        let closed = header(ui, head, track, &dev, s, ws);
        ui.scene.hline(sheet.x, sheet.right(), body.y, t.line);
        if !closed {
            let body = body.shrink2(14.0, 12.0);
            ui.scene.push_clip(body.expand(2.0));
            match dev.type_id.as_str() {
                "peel.fx.eq" => crate::eq_editor::draw(ui, body, track, &dev, s, ws),
                "peel.fx.mastering" => crate::mastering_ui::draw(ui, body, track, &dev, s, ws),
                _ => generic(ui, body, track, &dev, s, ws),
            }
            ui.scene.pop_clip();
        }
        ui.scene.pop_clip();
    });
}

/// Title, the track's whole chain as chips, bypass, remove and close.
/// Returns true when the editor closed.
fn header(
    ui: &mut Ui,
    head: Rect,
    track: Id,
    dev: &DeviceInstance,
    s: &mut Session,
    ws: &mut Workspace,
) -> bool {
    let t = ui.theme.clone();
    let mut row = head.shrink2(12.0, 5.0);
    let close = ui.icon_button(
        W::new("fx-close"),
        row.cut_right(30.0),
        icons::X,
        false,
        None,
    );
    ui.name(&close, "Close editor");
    ui.tooltip(&close, "Close (Esc)");
    row.cut_right(4.0);
    let remove = ui.icon_button(
        W::new("fx-remove"),
        row.cut_right(30.0),
        icons::TRASH,
        false,
        None,
    );
    ui.name(&remove, "Remove effect");
    ui.tooltip(&remove, "Remove effect");
    row.cut_right(8.0);
    let bypass_r = row.cut_right(70.0);
    let bypass = ui.interact_node(
        W::new("fx-bypass"),
        bypass_r,
        FocusKind::Activate,
        Semantics::new(Role::Button).label("Bypass").toggled(dev.bypass),
    );
    ui.scene.rect(
        bypass_r,
        t.radius_s,
        if dev.bypass {
            t.warn
        } else if bypass.hovered {
            t.control_hover
        } else {
            t.control
        },
    );
    ui.text(
        bypass_r,
        "Bypass",
        t.label,
        if dev.bypass { t.text_on_accent } else { t.text_dim },
        Align::Center,
    );
    ui.focus_ring(W::new("fx-bypass"), bypass_r, t.radius_s);
    if bypass.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    row.cut_right(12.0);

    let tr = s.project().mixer.track(track).cloned();
    let (name, color, is_master) = tr.as_ref().map_or(("", t.text_dim, false), |x| {
        (
            x.name.as_str(),
            col(x.color),
            s.project().mixer.tracks[0].id == x.id,
        )
    });
    let name = name.to_string();
    let swatch = row.cut_left(4.0);
    ui.scene.rect(
        swatch,
        2.0,
        if is_master { t.accent } else { color },
    );
    row.cut_left(8.0);
    let title_w = (ui.measure(&name, t.heading).x + 12.0).min(row.w * 0.3);
    let title = row.cut_left(title_w);
    let shown = ui.fit_text(&name, t.heading, title.w);
    ui.text(title, &shown, t.heading, t.text, Align::Start);
    row.cut_left(8.0);
    ui.divider_v(row.cut_left(1.0));
    row.cut_left(8.0);

    // Chain chips.
    if let Some(tr) = &tr {
        let chips: Vec<(Id, String, bool)> = tr
            .inserts
            .iter()
            .map(|d| {
                (
                    d.id,
                    peel_devices::descriptor(&d.type_id)
                        .map_or("Missing", |x| x.name)
                        .to_string(),
                    d.bypass,
                )
            })
            .collect();
        let widths: Vec<f32> = chips
            .iter()
            .map(|(_, n, _)| ui.measure(n, t.label).x + 36.0)
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + 4.0 * chips.len() as f32;
        let mut off = vec2(ws.fx.chain_scroll, 0.0);
        if total > row.w {
            ui.scroll_area(W::new("fx-chain"), row, vec2(total, row.h), &mut off);
        } else {
            off.x = 0.0;
        }
        ws.fx.chain_scroll = off.x;
        ui.scene.push_clip(row);
        let mut x = row.x - off.x;
        for ((id, n, byp), w) in chips.iter().zip(widths) {
            let cell = Rect {
                x,
                y: row.y,
                w,
                h: row.h - if total > row.w { 4.0 } else { 0.0 },
            };
            x += w + 4.0;
            if cell.right() < row.x || cell.x > row.right() {
                continue;
            }
            let on = *id == dev.id;
            let cid = W::new(("fx-chip", *id));
            let resp = ui.interact_node(
                cid,
                cell,
                FocusKind::Activate,
                Semantics::new(Role::Tab).label(n.clone()).selected(on),
            );
            ui.scene.rect(
                cell,
                t.radius_m,
                if on {
                    t.raised
                } else if resp.hovered {
                    t.control.alpha(0.6)
                } else {
                    t.well
                },
            );
            let mut inner = cell.shrink2(8.0, 0.0);
            let dot = inner.cut_left(10.0);
            ui.scene
                .circle(dot.center(), 3.0, if *byp { t.text_faint } else { t.play });
            inner.cut_left(4.0);
            ui.text(
                inner,
                n,
                t.label,
                if on { t.text } else { t.text_dim },
                Align::Start,
            );
            if on {
                ui.scene.rect(
                    Rect {
                        x: cell.x + 6.0,
                        y: cell.bottom() - 2.0,
                        w: cell.w - 12.0,
                        h: 2.0,
                    },
                    1.0,
                    t.accent,
                );
            }
            ui.focus_ring(cid, cell, t.radius_m);
            if resp.hovered {
                ui.set_cursor(CursorIcon::Pointer);
            }
            if resp.clicked {
                ws.fx.open(track, *id);
            }
        }
        ui.scene.pop_clip();
    }

    if bypass.clicked {
        crate::mixer::toggle_bypass(s, track, dev.id, dev.bypass);
    }
    if remove.clicked {
        let name = peel_devices::descriptor(&dev.type_id).map_or("effect", |d| d.name);
        crate::mixer::remove_insert(s, ws, track, dev.id, name);
        return true;
    }
    if close.clicked {
        ws.fx.close();
        return true;
    }
    false
}

/// Full-width descriptor panel for effects without a dedicated editor.
fn generic(
    ui: &mut Ui,
    body: Rect,
    track: Id,
    dev: &DeviceInstance,
    s: &mut Session,
    ws: &mut Workspace,
) {
    let t = ui.theme.clone();
    let Some(desc) = peel_devices::descriptor(&dev.type_id) else {
        ui.text(
            body.with_h(24.0),
            "This effect is not available",
            t.body,
            t.text_faint,
            Align::Start,
        );
        return;
    };
    let mut c = body;
    let mut top = c.cut_top(28.0);
    if peel_devices::shows_reduction(&dev.type_id) {
        crate::inspector::reduction_meter(ui, top.cut_right(140.0), s, dev.id);
        top.cut_right(8.0);
        ui.text(top.cut_right(110.0), "Reduction", t.caption, t.text_faint, Align::End);
    }
    ui.text(top, desc.name, t.heading, t.text, Align::Start);
    c.cut_top(8.0);
    if peel_devices::accepts_sidechain(&dev.type_id) {
        crate::inspector::sidechain_row(ui, c.cut_top(30.0), s, track, dev);
        c.cut_top(8.0);
    }
    crate::inspector::device_panel(ui, c, dev, desc, s, ws, t.accent);
}
