//! Keyboard reference, shown on the Welcome screen and as an overlay.

use peel_ui::{Align, Rect, Ui, vec2};

pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Space", "Play / stop"),
    ("L", "Pattern / Song"),
    ("Home  /  End", "Go to start / end"),
    ("PgUp  /  PgDn", "Back / forward one bar"),
    ("Z – M  /  Q – P", "Play the selected instrument"),
    ("−  /  =", "Octave down / up"),
    ("F6  F7  F8  F9", "Channels, Piano roll, Scripts, Mixer"),
    ("Cmd Z  /  Cmd ⇧ Z", "Undo / redo"),
    ("Cmd C  X  V", "Copy, cut, paste notes"),
    ("Cmd D", "Duplicate notes or clips"),
    ("Del", "Delete selection"),
    ("↑  ↓", "Transpose notes (⇧ octave)"),
    ("Cmd scroll", "Zoom"),
    ("Right click", "Erase notes, clip menu"),
    ("Cmd S  /  Cmd O", "Save / open"),
    ("Cmd E", "Export audio"),
];

/// Draws key caps and descriptions in `r`; returns the height used.
pub fn draw_list(ui: &mut Ui, r: Rect, rows: usize) -> f32 {
    let t = ui.theme.clone();
    let row_h = 26.0;
    let cmd = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    let mut y = r.y;
    for (keys, what) in SHORTCUTS.iter().take(rows) {
        let keys = keys.replace("Cmd", cmd);
        let row = Rect {
            x: r.x,
            y,
            w: r.w,
            h: row_h,
        };
        let mut x = row.x;
        for part in keys.split("  ") {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if part == "/" || part == "–" {
                ui.text(
                    Rect { x, w: 12.0, ..row },
                    part,
                    t.caption,
                    t.text_faint,
                    Align::Center,
                );
                x += 14.0;
                continue;
            }
            let w = ui.measure(part, t.caption).x + 12.0;
            let cap = Rect {
                x,
                y: row.y + 3.0,
                w,
                h: row_h - 6.0,
            };
            ui.scene.rect(cap, 4.0, t.control);
            ui.scene.rect(
                Rect {
                    y: cap.bottom() - 2.0,
                    h: 2.0,
                    ..cap
                }
                .shrink2(1.0, 0.0),
                1.0,
                t.bg.alpha(0.6),
            );
            ui.text(cap, part, t.caption, t.text, Align::Center);
            x += w + 4.0;
        }
        let desc = Rect {
            x: row.x + 170.0,
            w: row.w - 170.0,
            ..row
        };
        ui.text(desc, what, t.caption, t.text_dim, Align::Start);
        y += row_h;
    }
    y - r.y
}

/// Full-screen overlay with every shortcut. Returns true when closed.
pub fn overlay(ui: &mut Ui) -> bool {
    let t = ui.theme.clone();
    let screen = ui.screen();
    let mut close = false;
    ui.with_layer(peel_ui::LayerId::POPUP, |ui| {
        ui.scene.push_clip_absolute(Rect::EVERYTHING);
        let bg = ui.interact(peel_ui::Id::new("keys-backdrop"), screen);
        ui.scene.rect(screen, 0.0, t.bg.alpha(0.72));
        let h = SHORTCUTS.len() as f32 * 26.0 + 90.0;
        let panel = Rect::from_center(screen.center(), 460.0, h);
        ui.interact(peel_ui::Id::new("keys-panel"), panel);
        ui.scene
            .shadow(panel.translate(vec2(0.0, 8.0)), 12.0, 24.0, t.shadow);
        ui.scene.rect(panel, 12.0, t.panel);
        let mut body = panel.shrink(24.0);
        ui.text(
            body.cut_top(28.0),
            "Keyboard",
            t.heading,
            t.text,
            Align::Start,
        );
        body.cut_top(12.0);
        draw_list(ui, body, SHORTCUTS.len());
        if bg.clicked {
            close = true;
        }
        ui.scene.pop_clip();
    });
    close || ui.input.take_named(peel_ui::NamedKey::Escape)
}
