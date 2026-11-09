//! The platform menu bar (macOS and Windows) built from a [`MenuBar`] with
//! muda.
//!
//! Everything here runs on the main thread. Choosing an item only posts its
//! id to the event loop through an `EventLoopProxy`; the runner queues it on
//! the `Ui` and the app acts on it in its next frame.
//!
//! Shortcuts: on macOS, items carry key equivalents for Cmd and function-key
//! shortcuts, so the menu takes those keys (the window never sees them, and
//! nothing runs twice). Plain keys such as Space stay with the window. On
//! Windows the accelerators are shown but not translated (the event loop
//! does not call `TranslateAcceleratorW`): the keys reach the window and
//! the app's own shortcut handling, again exactly once.

use std::collections::HashMap;
use std::sync::Mutex;

use muda::accelerator::Accelerator;
use muda::{AboutMetadata, CheckMenuItem, MenuEvent, MenuId, PredefinedMenuItem, Submenu};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::menu::{Menu, MenuBar, MenuCommand, MenuEntry, Shortcut, ShortcutKey, StandardItem};

enum Item {
    Plain(muda::MenuItem),
    Check(CheckMenuItem),
}

pub(crate) struct NativeMenu {
    menu: muda::Menu,
    last: MenuBar,
    items: HashMap<u32, Item>,
    /// Submenus in depth-first order, matching `walk_submenus`.
    submenus: Vec<Submenu>,
    #[cfg(target_os = "windows")]
    hwnd: isize,
}

/// Starts forwarding menu choices to the event loop. Ids that are not ours
/// (the platform's own items) are dropped.
pub(crate) fn forward_events<T: From<u32> + Send + 'static>(proxy: EventLoopProxy<T>) {
    let proxy = Mutex::new(proxy);
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if let Ok(id) = e.id().0.parse::<u32>()
            && let Ok(p) = proxy.lock()
        {
            let _ = p.send_event(T::from(id));
        }
    }));
}

pub(crate) fn stop_events() {
    MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
}

impl NativeMenu {
    /// Builds and installs `bar`; `None` when the platform refused it.
    pub(crate) fn install(bar: &MenuBar, window: &Window) -> Option<NativeMenu> {
        let _ = window;
        let mut native = NativeMenu {
            menu: muda::Menu::new(),
            last: MenuBar::default(),
            items: HashMap::new(),
            submenus: Vec::new(),
            #[cfg(target_os = "windows")]
            hwnd: hwnd(window)?,
        };
        native.build(bar).ok()?;
        native.attach().ok()?;
        Some(native)
    }

    /// Brings the platform menu in line with `bar`: labels, enabled and
    /// check states update in place; a different structure rebuilds it.
    pub(crate) fn sync(&mut self, bar: &MenuBar) {
        if *bar == self.last {
            // Native check items toggle before the event reaches the app.
            // A radio-style choice or a blocked command may keep the same
            // model, so restore its checks even when nothing else changed.
            for c in bar.commands() {
                if let Some(Item::Check(item)) = self.items.get(&c.id)
                    && let Some(on) = c.checked
                    && item.is_checked() != on
                {
                    item.set_checked(on);
                }
            }
            return;
        }
        if shape(bar) != shape(&self.last) {
            self.detach();
            let old = std::mem::replace(&mut self.menu, muda::Menu::new());
            self.items.clear();
            self.submenus.clear();
            if let Err(e) = self.build(bar).and_then(|()| self.attach()) {
                log::warn!("menu rebuild failed: {e}");
                // Keep the old menu rather than none.
                self.menu = old;
                let _ = self.attach();
            }
            return;
        }
        for c in bar.commands() {
            match self.items.get(&c.id) {
                Some(Item::Plain(i)) => {
                    if i.text() != c.label {
                        i.set_text(&c.label);
                    }
                    if i.is_enabled() != c.enabled {
                        i.set_enabled(c.enabled);
                    }
                }
                Some(Item::Check(i)) => {
                    if i.text() != c.label {
                        i.set_text(&c.label);
                    }
                    if i.is_enabled() != c.enabled {
                        i.set_enabled(c.enabled);
                    }
                    let on = c.checked.unwrap_or(false);
                    if i.is_checked() != on {
                        i.set_checked(on);
                    }
                }
                None => {}
            }
        }
        let mut subs = Vec::new();
        for m in &bar.menus {
            walk_submenus(m, true, &mut subs);
        }
        for (sub, (title, enabled)) in self.submenus.iter().zip(subs) {
            if sub.text() != title {
                sub.set_text(title);
            }
            if sub.is_enabled() != enabled {
                sub.set_enabled(enabled);
            }
        }
        self.last = bar.clone();
    }

    fn build(&mut self, bar: &MenuBar) -> muda::Result<()> {
        for m in &bar.menus {
            let sub = self.submenu(m, true)?;
            self.menu.append(&sub)?;
            #[cfg(target_os = "macos")]
            match m.role {
                crate::menu::MenuRole::Window => sub.set_as_windows_menu_for_nsapp(),
                crate::menu::MenuRole::Help => sub.set_as_help_menu_for_nsapp(),
                _ => {}
            }
        }
        self.last = bar.clone();
        Ok(())
    }

    fn submenu(&mut self, m: &Menu, enabled: bool) -> muda::Result<Submenu> {
        let sub = Submenu::new(&m.title, enabled);
        self.submenus.push(sub.clone());
        for e in &m.entries {
            match e {
                MenuEntry::Command(c) => self.command(&sub, c)?,
                MenuEntry::Separator => sub.append(&PredefinedMenuItem::separator())?,
                MenuEntry::Submenu { menu, enabled, .. } => {
                    let child = self.submenu(menu, *enabled)?;
                    sub.append(&child)?;
                }
                MenuEntry::Standard(s) => {
                    if let Some(item) = standard(s) {
                        sub.append(&item)?;
                    }
                }
            }
        }
        Ok(sub)
    }

    fn command(&mut self, sub: &Submenu, c: &MenuCommand) -> muda::Result<()> {
        let id = MenuId::new(c.id.to_string());
        let accel = c.shortcut.as_ref().and_then(accelerator);
        match c.checked {
            Some(on) => {
                let item = CheckMenuItem::with_id(id, &c.label, c.enabled, on, accel);
                sub.append(&item)?;
                self.items.insert(c.id, Item::Check(item));
            }
            None => {
                let item = muda::MenuItem::with_id(id, &c.label, c.enabled, accel);
                sub.append(&item)?;
                self.items.insert(c.id, Item::Plain(item));
            }
        }
        Ok(())
    }

    fn attach(&self) -> muda::Result<()> {
        #[cfg(target_os = "macos")]
        self.menu.init_for_nsapp();
        #[cfg(target_os = "windows")]
        // SAFETY: `hwnd` came from the live winit window this menu belongs
        // to, and the runner drops the menu before the window.
        unsafe {
            self.menu
                .init_for_hwnd_with_theme(self.hwnd, muda::MenuTheme::Dark)?;
        }
        Ok(())
    }

    fn detach(&self) {
        #[cfg(target_os = "macos")]
        self.menu.remove_for_nsapp();
        #[cfg(target_os = "windows")]
        // SAFETY: as in `attach`.
        unsafe {
            let _ = self.menu.remove_for_hwnd(self.hwnd);
        }
    }
}

impl Drop for NativeMenu {
    fn drop(&mut self) {
        self.detach();
    }
}

#[cfg(target_os = "windows")]
fn hwnd(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// `bar` without the parts that can change in place.
fn shape(bar: &MenuBar) -> MenuBar {
    fn strip(m: &Menu) -> Menu {
        Menu {
            title: String::new(),
            role: m.role,
            entries: m
                .entries
                .iter()
                .map(|e| match e {
                    MenuEntry::Command(c) => MenuEntry::Command(MenuCommand {
                        label: String::new(),
                        enabled: true,
                        checked: c.checked.map(|_| false),
                        ..c.clone()
                    }),
                    MenuEntry::Submenu { menu, .. } => MenuEntry::Submenu {
                        menu: strip(menu),
                        enabled: true,
                        inline: None,
                    },
                    other => other.clone(),
                })
                .collect(),
        }
    }
    MenuBar {
        menus: bar.menus.iter().map(strip).collect(),
    }
}

fn walk_submenus(m: &Menu, enabled: bool, out: &mut Vec<(String, bool)>) {
    out.push((m.title.clone(), enabled));
    for e in &m.entries {
        if let MenuEntry::Submenu { menu, enabled, .. } = e {
            walk_submenus(menu, *enabled, out);
        }
    }
}

fn accelerator(s: &Shortcut) -> Option<Accelerator> {
    // macOS gives the menu every key equivalent it holds, so only the
    // shortcuts that cannot be typed into a field go there.
    if cfg!(target_os = "macos") && !s.native() {
        return None;
    }
    let mut text = String::new();
    if s.command {
        text.push_str("CmdOrCtrl+");
    }
    if s.alt {
        text.push_str("Alt+");
    }
    if s.shift {
        text.push_str("Shift+");
    }
    match s.key {
        ShortcutKey::Char(',') => text.push_str("Comma"),
        ShortcutKey::Char(c) if c.is_ascii_digit() => text.push_str(&format!("Digit{c}")),
        ShortcutKey::Char(c) if c.is_ascii_alphabetic() => {
            text.push_str(&format!("Key{}", c.to_ascii_uppercase()))
        }
        ShortcutKey::Char(_) => return None,
        ShortcutKey::F(n) => text.push_str(&format!("F{n}")),
        ShortcutKey::Space => text.push_str("Space"),
        ShortcutKey::Home => text.push_str("Home"),
        ShortcutKey::End => text.push_str("End"),
    }
    text.parse().ok()
}

fn standard(s: &StandardItem) -> Option<PredefinedMenuItem> {
    Some(match s {
        StandardItem::About { name, version } => PredefinedMenuItem::about(
            Some(&format!("About {name}")),
            Some(AboutMetadata {
                name: Some(name.clone()),
                version: Some(version.clone()),
                ..Default::default()
            }),
        ),
        StandardItem::Services => PredefinedMenuItem::services(None),
        StandardItem::Hide => PredefinedMenuItem::hide(None),
        StandardItem::HideOthers => PredefinedMenuItem::hide_others(None),
        StandardItem::ShowAll => PredefinedMenuItem::show_all(None),
        StandardItem::Minimize => PredefinedMenuItem::minimize(None),
        StandardItem::Zoom => PredefinedMenuItem::maximize(None),
        StandardItem::Fullscreen => PredefinedMenuItem::fullscreen(None),
        StandardItem::BringAllToFront => PredefinedMenuItem::bring_all_to_front(None),
    })
}
