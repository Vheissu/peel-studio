//! Workspace layout: the navigation rails, the View button's menu and the
//! pane toggles (their shortcuts live with the app menus, `menus.rs`).
//! Which panes show is stored in
//! `Prefs` (shared with scripts' `ui.get`/`ui.set`); sizes and tabs live in
//! the `Workspace` and are copied to `Prefs` when they settle.

use peel_ui::{Edge, Id as W, MenuItem, Rect, Ui, icons};

use crate::studio::Studio;
use crate::workspace::{BrowserTab, DockTab};

pub const VIEW_MENU: W = W(0x7a11_0b2e_44d1);

pub const BROWSER_TABS: [(BrowserTab, &str, &str); 4] = [
    (BrowserTab::Instruments, icons::PIANO_KEYS, "Instruments"),
    (BrowserTab::Effects, icons::SLIDERS, "Effects"),
    (BrowserTab::Samples, icons::WAVEFORM, "Samples"),
    (BrowserTab::Patterns, icons::SQUARES, "Patterns"),
];

pub const DOCK_TABS: [(DockTab, &str, &str, &str); 4] = [
    (DockTab::Rack, icons::ROWS, "Channels", "F6"),
    (DockTab::PianoRoll, icons::PIANO_KEYS, "Piano roll", "F7"),
    (DockTab::Mixer, icons::FADERS, "Mixer", "F9"),
    (DockTab::Perform, icons::SQUARES, "Perform", "F10"),
];

#[cfg(target_os = "macos")]
const MOD: &str = "Cmd Opt";
#[cfg(not(target_os = "macos"))]
const MOD: &str = "Ctrl Alt";

/// Opens a browser tab, or hides the browser when that tab is showing.
pub fn browser_tab(studio: &mut Studio, tab: BrowserTab) {
    if studio.prefs.show_browser && studio.ws.browser_tab == tab {
        studio.prefs.show_browser = false;
    } else {
        if studio.ws.browser_tab != tab {
            studio.ws.browser_scroll = 0.0;
        }
        studio.ws.browser_tab = tab;
        studio.prefs.show_browser = true;
    }
    studio.prefs.save();
}

/// Shows an editor in the dock, or collapses the dock when that editor is
/// already showing.
pub fn dock_tab(studio: &mut Studio, tab: DockTab, toggle: bool) {
    if toggle && studio.prefs.show_editor && studio.ws.dock == tab {
        studio.prefs.show_editor = false;
    } else {
        studio.ws.dock = tab;
        studio.prefs.show_editor = true;
    }
    studio.prefs.save();
}

pub fn toggle_browser(studio: &mut Studio) {
    studio.prefs.show_browser = !studio.prefs.show_browser;
    studio.prefs.save();
}

pub fn toggle_inspector(studio: &mut Studio) {
    let on = !studio.prefs.show_inspector;
    studio.prefs.set_inspector(on);
    studio.prefs.save();
}

pub fn toggle_editor(studio: &mut Studio) {
    studio.prefs.show_editor = !studio.prefs.show_editor;
    studio.prefs.save();
}

pub fn toggle_help(studio: &mut Studio) {
    studio.prefs.show_help = !studio.prefs.show_help;
    studio.prefs.save();
}

/// The left rail: one entry per browser tab.
pub fn left_rail(ui: &mut Ui, r: Rect, studio: &mut Studio) {
    let t = ui.theme.clone();
    let mut col = r.shrink2(0.0, 4.0);
    for (tab, icon, label) in BROWSER_TABS {
        let cell = col.cut_top(t.rail_w);
        let on = studio.prefs.show_browser && studio.ws.browser_tab == tab;
        let resp = ui.rail_item(
            W::new(("browser-tab", label)),
            cell,
            icon,
            label,
            on,
            Edge::Right,
        );
        if resp.clicked {
            browser_tab(studio, tab);
        }
        col.cut_top(2.0);
    }
}

/// The right rail: Inspector and Scripts at the top, hints and keyboard
/// shortcuts at the bottom.
pub fn right_rail(ui: &mut Ui, r: Rect, studio: &mut Studio) {
    let t = ui.theme.clone();
    let mut col = r.shrink2(0.0, 4.0);
    let cell = col.cut_top(t.rail_w);
    let insp = ui.rail_item(
        W::new("rail-inspector"),
        cell,
        icons::SLIDERS,
        &format!("Inspector ({MOD} I)"),
        studio.prefs.show_inspector,
        Edge::Left,
    );
    if insp.clicked {
        toggle_inspector(studio);
    }
    col.cut_top(2.0);
    let cell = col.cut_top(t.rail_w);
    let scripts = ui.rail_item(
        W::new("scripts-open"),
        cell,
        icons::CODE,
        "Scripts (F8)",
        studio.prefs.show_scripts,
        Edge::Left,
    );
    // A running script, or one whose last run failed, marks the entry.
    let dot = if studio.scripts.is_running() {
        Some(t.accent)
    } else if studio
        .scripts
        .last
        .as_ref()
        .is_some_and(|o| o.result.is_err())
    {
        Some(t.record)
    } else {
        None
    };
    if let Some(c) = dot {
        ui.scene
            .circle(peel_ui::vec2(cell.right() - 10.0, cell.y + 10.0), 3.0, c);
    }
    if scripts.clicked {
        crate::scripts::toggle(studio);
    }

    let keys_r = col.cut_bottom(t.rail_w);
    col.cut_bottom(2.0);
    let help_r = col.cut_bottom(t.rail_w);
    let help = ui.rail_item(
        W::new("rail-help"),
        help_r,
        icons::QUESTION,
        if studio.prefs.show_help {
            "Hide usage hints"
        } else {
            "Show usage hints"
        },
        studio.prefs.show_help,
        Edge::Left,
    );
    if help.clicked {
        toggle_help(studio);
    }
    let keys = ui.rail_item(
        W::new("keys-help"),
        keys_r,
        icons::KEYBOARD,
        "Keyboard shortcuts (F1)",
        studio.show_keys,
        Edge::Left,
    );
    if keys.clicked {
        studio.show_keys = !studio.show_keys;
    }
}

/// The View button and its menu (top bar).
pub fn view_button(ui: &mut Ui, r: Rect, studio: &mut Studio, icon_only: bool) {
    let open = ui.is_popup_open(VIEW_MENU);
    let resp = if icon_only {
        let resp = ui.icon_button(W::new("view"), r, icons::LAYOUT, open, None);
        ui.name(&resp, "View");
        resp
    } else {
        ui.quiet_button(W::new("view"), r, Some(icons::LAYOUT), "View", true, open)
    };
    ui.tooltip(&resp, "Choose which panes show");
    if resp.clicked {
        ui.open_popup(VIEW_MENU, r);
    }
    let p = &studio.prefs;
    let items = [
        MenuItem::new("Browser")
            .shortcut(format!("{MOD} B"))
            .checked(p.show_browser),
        MenuItem::new("Inspector")
            .shortcut(format!("{MOD} I"))
            .checked(p.show_inspector),
        MenuItem::new("Scripts")
            .shortcut("F8")
            .checked(p.show_scripts),
        MenuItem::new("Editor")
            .shortcut(format!("{MOD} E"))
            .checked(p.show_editor),
        MenuItem::Separator,
        MenuItem::new("Tempo lane").checked(studio.ws.playlist.show_tempo),
        MenuItem::new("Master meter").checked(p.show_meter),
        MenuItem::new("Device in status bar").checked(p.show_status_details),
        MenuItem::new("Usage hints").checked(p.show_help),
        MenuItem::Separator,
        MenuItem::new("Reset layout"),
    ];
    if let Some(i) = ui.menu(VIEW_MENU, Ui::below(r), &items) {
        match i {
            0 => toggle_browser(studio),
            1 => toggle_inspector(studio),
            2 => crate::scripts::toggle(studio),
            3 => toggle_editor(studio),
            5 => studio.ws.playlist.show_tempo = !studio.ws.playlist.show_tempo,
            6 => {
                studio.prefs.show_meter = !studio.prefs.show_meter;
                studio.prefs.save();
            }
            7 => {
                studio.prefs.show_status_details = !studio.prefs.show_status_details;
                studio.prefs.save();
            }
            8 => toggle_help(studio),
            10 => reset_layout(studio),
            _ => {}
        }
    }
}

/// Restores the default panes, sizes and tabs.
pub fn reset_layout(studio: &mut Studio) {
    studio.prefs.reset_layout();
    let d = crate::workspace::Workspace::default();
    studio.ws.browser_w = d.browser_w;
    studio.ws.inspector_w = d.inspector_w;
    studio.ws.dock_h = d.dock_h;
    studio.ws.dock = d.dock;
    studio.ws.browser_tab = d.browser_tab;
    studio.prefs.save();
}

/// Saves pane sizes and tabs once they settle (no drag in progress).
pub fn persist(ui: &Ui, studio: &mut Studio) {
    if ui.any_active() {
        return;
    }
    if studio.prefs.capture_layout(&studio.ws) {
        studio.prefs.save();
    }
}
