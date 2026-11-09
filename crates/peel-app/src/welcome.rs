//! Welcome screen: start a new beat, open something recent, or recover
//! unsaved work after a crash.

use std::path::PathBuf;

use peel_ui::{Align, CursorIcon, Id as W, LayerId, Rect, Ui, icons, vec2};

/// What the user picked.
pub enum WelcomeAction {
    /// Start from `peel_session::template::templates()[i]`.
    Template(usize),
    Empty,
    Open,
    OpenPath(PathBuf),
    Recover(PathBuf),
    Close,
}

pub struct WelcomeData<'a> {
    pub recent: &'a [PathBuf],
    pub recovered: &'a [peel_project::AutosaveEntry],
    pub show_at_startup: &'a mut bool,
}

fn big_button(ui: &mut Ui, id: W, r: Rect, icon: &str, label: &str, primary: bool) -> bool {
    let t = ui.theme.clone();
    let resp = ui.interact(id, r);
    let hot = ui.animate_bool(id, resp.hovered);
    let (fill, ink) = if primary {
        (t.accent.lighten(hot * 0.08), t.text_on_accent)
    } else {
        (t.control.mix(t.control_hover, hot), t.text)
    };
    ui.scene.rect(r, t.radius_l, fill);
    let mut inner = r.shrink2(16.0, 0.0);
    ui.icon(inner.cut_left(20.0), icon, 17.0, ink);
    inner.cut_left(12.0);
    ui.text(inner, label, t.label.size(14.0), ink, Align::Start);
    if resp.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    resp.clicked
}

pub fn draw(ui: &mut Ui, data: WelcomeData) -> Option<WelcomeAction> {
    let t = ui.theme.clone();
    let screen = ui.screen();
    let mut action = None;
    ui.with_layer(LayerId::POPUP, |ui| {
        ui.scene.push_clip_absolute(Rect::EVERYTHING);
        let backdrop = ui.interact(W::new("welcome-backdrop"), screen);
        ui.scene.rect(screen, 0.0, t.bg.alpha(0.78));
        let w = 780.0f32.min(screen.w - 40.0);
        let h = 470.0f32.min(screen.h - 40.0);
        let panel = Rect::from_center(screen.center(), w, h);
        ui.interact(W::new("welcome-panel"), panel);
        ui.scene
            .shadow(panel.translate(vec2(0.0, 10.0)), 14.0, 28.0, t.shadow);
        ui.scene.rect(panel, 14.0, t.panel);
        if backdrop.clicked {
            action = Some(WelcomeAction::Close);
        }

        let mut body = panel.shrink(28.0);
        let mut left = body.cut_left(270.0);
        body.cut_left(28.0);
        let right = body;

        // Brand.
        let brand = left.cut_top(56.0);
        let c = vec2(brand.x + 22.0, brand.center().y);
        ui.scene.circle(c, 20.0, t.accent);
        ui.scene.circle(c, 14.0, t.accent.darken(0.18));
        ui.scene.arc(c, 20.0, 5.0, 0.35, 1.25, t.panel);
        ui.scene.arc(
            c + vec2(15.0, -13.0),
            6.5,
            3.5,
            3.4,
            2.6,
            t.accent.lighten(0.15),
        );
        let mut tr = brand;
        tr.cut_left(56.0);
        ui.text(
            tr.cut_top(32.0),
            "Peel Studio",
            t.heading.size(22.0),
            t.text,
            Align::Start,
        );
        ui.text(
            tr,
            &format!("Version {}", env!("CARGO_PKG_VERSION")),
            t.caption,
            t.text_faint,
            Align::Start,
        );
        left.cut_top(26.0);

        let buttons = [
            (icons::MUSIC_NOTES, "New beat", true, 0),
            (icons::PLUS, "Empty project", false, 1),
            (icons::FOLDER_OPEN, "Open project or MIDI", false, 2),
        ];
        for (icon, label, primary, which) in buttons {
            let r = left.cut_top(46.0);
            left.cut_top(10.0);
            if big_button(ui, W::new(("welcome-btn", which)), r, icon, label, primary) {
                action = Some(match which {
                    0 => WelcomeAction::Template(0),
                    1 => WelcomeAction::Empty,
                    _ => WelcomeAction::Open,
                });
            }
        }
        // More starting points.
        left.cut_top(6.0);
        ui.text(
            left.cut_top(20.0),
            "Start from",
            t.caption,
            t.text_dim,
            Align::Start,
        );
        left.cut_top(4.0);
        let mut row = left.cut_top(34.0);
        for (i, (name, _)) in peel_session::template::templates()
            .into_iter()
            .enumerate()
            .skip(1)
        {
            let w = ui.measure(name, t.label).x + 32.0;
            let r = row.cut_left(w);
            row.cut_left(8.0);
            if ui.button(W::new(("welcome-template", i)), r, name).clicked {
                action = Some(WelcomeAction::Template(i));
            }
        }
        // Startup toggle.
        let toggle_r = left.cut_bottom(26.0);
        let mut tr = toggle_r;
        let led = ui.led(
            W::new("welcome-startup"),
            tr.cut_left(22.0),
            *data.show_at_startup,
            t.play,
        );
        if led.clicked {
            *data.show_at_startup = !*data.show_at_startup;
        }
        tr.cut_left(8.0);
        ui.text(
            tr,
            "Show this at startup",
            t.caption,
            t.text_dim,
            Align::Start,
        );

        // Recent and recovered projects.
        let mut list = right;
        let mut y_rows = Vec::new();
        if !data.recovered.is_empty() {
            ui.text(
                list.cut_top(22.0),
                "Recovered after an unexpected exit",
                t.caption,
                t.record,
                Align::Start,
            );
            for (i, _) in data.recovered.iter().take(3).enumerate() {
                y_rows.push((list.cut_top(44.0), true, i));
                list.cut_top(4.0);
            }
            list.cut_top(12.0);
        }
        ui.text(
            list.cut_top(22.0),
            "Recent",
            t.caption,
            t.text_faint,
            Align::Start,
        );
        if data.recent.is_empty() {
            let mut hint = list.cut_top(40.0);
            hint.cut_top(6.0);
            ui.text(
                hint.cut_top(20.0),
                "Projects you save show up here.",
                t.body,
                t.text_faint,
                Align::Start,
            );
        }
        // Key reference fills whatever room the lists leave.
        let shown = data
            .recent
            .len()
            .min(((list.h - 40.0) / 48.0).max(0.0) as usize);
        let keys_h = list.h - shown as f32 * 48.0 - 30.0;
        if keys_h > 120.0 {
            let mut keys = Rect {
                y: list.bottom() - keys_h,
                h: keys_h,
                ..list
            };
            ui.text(
                keys.cut_top(24.0),
                "Keys",
                t.caption,
                t.text_faint,
                Align::Start,
            );
            let rows = (keys.h / 26.0) as usize;
            crate::keys::draw_list(ui, keys, rows);
            list.h -= keys_h + 8.0;
        }
        for (i, _) in data.recent.iter().enumerate() {
            if list.h < 44.0 {
                break;
            }
            y_rows.push((list.cut_top(44.0), false, i));
            list.cut_top(4.0);
        }
        for (row, recovered, i) in y_rows {
            let id = W::new(("welcome-row", recovered, i));
            let resp = ui.interact(id, row);
            let hot = ui.animate_bool(id, resp.hovered);
            ui.scene
                .rect(row, t.radius_m, t.raised.alpha(0.35 + hot * 0.65));
            let mut inner = row.shrink2(14.0, 6.0);
            let icon_r = inner.cut_left(20.0);
            inner.cut_left(10.0);
            let (title, sub, path) = if recovered {
                let e = &data.recovered[i];
                (
                    e.title.clone(),
                    format!(
                        "Autosaved {}",
                        e.saved_at
                            .replace('T', " ")
                            .chars()
                            .take(16)
                            .collect::<String>()
                    ),
                    e.path.clone(),
                )
            } else {
                let p = &data.recent[i];
                let title = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let sub = p
                    .parent()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                (title, sub, p.clone())
            };
            ui.icon(
                icon_r,
                if recovered {
                    icons::LIGHTNING
                } else {
                    icons::MUSIC_NOTE
                },
                15.0,
                if recovered { t.record } else { t.accent },
            );
            let title = ui.fit_text(&title, t.label, inner.w);
            ui.text(inner.cut_top(18.0), &title, t.label, t.text, Align::Start);
            let sub = ui.fit_text(&sub, t.caption, inner.w);
            ui.text(inner, &sub, t.caption, t.text_faint, Align::Start);
            if resp.hovered {
                ui.set_cursor(CursorIcon::Pointer);
            }
            if resp.clicked {
                action = Some(if recovered {
                    WelcomeAction::Recover(path)
                } else {
                    WelcomeAction::OpenPath(path)
                });
            }
        }
        ui.scene.pop_clip();
    });
    if ui.input.take_named(peel_ui::NamedKey::Escape) {
        action = Some(WelcomeAction::Close);
    }
    action
}
