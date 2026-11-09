//! Application menus, described once and shown either as the platform's
//! menu bar (macOS, Windows) or as an in-window strip (see
//! [`Ui::menu_bar`](crate::Ui::menu_bar)).
//!
//! The app returns a [`MenuBar`] from [`App::menu_bar`](crate::App::menu_bar).
//! Choosing an entry, from either form, queues its command id on the [`Ui`]
//! (`take_menu_commands`) for the next frame, so menu actions run on the UI
//! thread inside `update` like any other input. Native menu callbacks never
//! touch the app directly.

use winit::keyboard::{Key, KeyCode, NamedKey};

use crate::input::KeyPress;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuBar {
    pub menus: Vec<Menu>,
}

/// Where a menu sits in the platform's conventions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuRole {
    #[default]
    Normal,
    /// The macOS application menu (its title is the app's name).
    App,
    /// The macOS Window menu: the system adds its window list.
    Window,
    /// The macOS Help menu: the system adds its search field.
    Help,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    pub title: String,
    pub role: MenuRole,
    pub entries: Vec<MenuEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Command(MenuCommand),
    Separator,
    /// A nested menu. The in-window strip cannot nest: it shows `inline`
    /// instead when given, otherwise the submenu's entries under a heading.
    Submenu {
        menu: Menu,
        enabled: bool,
        inline: Option<MenuCommand>,
    },
    /// An item the platform implements itself. Only native menus show
    /// these; the in-window strip skips them.
    Standard(StandardItem),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StandardItem {
    /// The platform's About panel.
    About {
        name: String,
        version: String,
    },
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    Fullscreen,
    BringAllToFront,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuCommand {
    pub id: u32,
    pub label: String,
    pub shortcut: Option<Shortcut>,
    pub enabled: bool,
    /// `Some` makes a check item.
    pub checked: Option<bool>,
}

impl MenuCommand {
    pub fn new(id: u32, label: impl Into<String>) -> MenuCommand {
        MenuCommand {
            id,
            label: label.into(),
            shortcut: None,
            enabled: true,
            checked: None,
        }
    }

    pub fn shortcut(mut self, s: Shortcut) -> MenuCommand {
        self.shortcut = Some(s);
        self
    }

    pub fn enabled(mut self, on: bool) -> MenuCommand {
        self.enabled = on;
        self
    }

    pub fn checked(mut self, on: bool) -> MenuCommand {
        self.checked = Some(on);
        self
    }
}

impl From<MenuCommand> for MenuEntry {
    fn from(c: MenuCommand) -> MenuEntry {
        MenuEntry::Command(c)
    }
}

impl Menu {
    pub fn new(title: impl Into<String>) -> Menu {
        Menu {
            title: title.into(),
            role: MenuRole::Normal,
            entries: Vec::new(),
        }
    }

    pub fn role(mut self, role: MenuRole) -> Menu {
        self.role = role;
        self
    }

    pub fn push(&mut self, entry: impl Into<MenuEntry>) {
        self.entries.push(entry.into());
    }

    pub fn separator(&mut self) {
        // No leading or doubled separators.
        if self
            .entries
            .last()
            .is_some_and(|e| !matches!(e, MenuEntry::Separator))
        {
            self.entries.push(MenuEntry::Separator);
        }
    }

    /// Every command in this menu and its submenus (the inline stand-ins
    /// included), in order.
    pub fn commands(&self) -> Vec<&MenuCommand> {
        let mut out = Vec::new();
        for e in &self.entries {
            match e {
                MenuEntry::Command(c) => out.push(c),
                MenuEntry::Submenu { menu, inline, .. } => {
                    out.extend(menu.commands());
                    out.extend(inline.iter());
                }
                _ => {}
            }
        }
        out
    }
}

impl MenuBar {
    pub fn commands(&self) -> Vec<&MenuCommand> {
        self.menus.iter().flat_map(|m| m.commands()).collect()
    }

    pub fn command(&self, id: u32) -> Option<&MenuCommand> {
        self.commands().into_iter().find(|c| c.id == id)
    }
}

/// A keyboard shortcut. `command` is Cmd on macOS and Ctrl elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: ShortcutKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutKey {
    /// A letter, digit or `,` (lowercase).
    Char(char),
    F(u8),
    Space,
    Home,
    End,
}

impl Shortcut {
    pub const fn key(key: ShortcutKey) -> Shortcut {
        Shortcut {
            command: false,
            shift: false,
            alt: false,
            key,
        }
    }

    /// Cmd/Ctrl + `c`.
    pub const fn cmd(c: char) -> Shortcut {
        Shortcut {
            command: true,
            shift: false,
            alt: false,
            key: ShortcutKey::Char(c),
        }
    }

    pub const fn shift(mut self) -> Shortcut {
        self.shift = true;
        self
    }

    pub const fn alt(mut self) -> Shortcut {
        self.alt = true;
        self
    }

    /// The label menus show, in the app's style ("Cmd Shift S").
    pub fn label(&self) -> String {
        let mac = cfg!(target_os = "macos");
        let mut parts: Vec<String> = Vec::new();
        if self.command {
            parts.push(if mac { "Cmd" } else { "Ctrl" }.into());
        }
        if self.alt {
            parts.push(if mac { "Opt" } else { "Alt" }.into());
        }
        if self.shift {
            parts.push("Shift".into());
        }
        parts.push(match self.key {
            ShortcutKey::Char(c) => c.to_uppercase().to_string(),
            ShortcutKey::F(n) => format!("F{n}"),
            ShortcutKey::Space => "Space".into(),
            ShortcutKey::Home => "Home".into(),
            ShortcutKey::End => "End".into(),
        });
        parts.join(" ")
    }

    /// The physical key, for matching when Option/Alt changes the
    /// character.
    pub fn code(&self) -> Option<KeyCode> {
        Some(match self.key {
            ShortcutKey::Char(c) => match c {
                'a' => KeyCode::KeyA,
                'b' => KeyCode::KeyB,
                'c' => KeyCode::KeyC,
                'd' => KeyCode::KeyD,
                'e' => KeyCode::KeyE,
                'f' => KeyCode::KeyF,
                'g' => KeyCode::KeyG,
                'h' => KeyCode::KeyH,
                'i' => KeyCode::KeyI,
                'j' => KeyCode::KeyJ,
                'k' => KeyCode::KeyK,
                'l' => KeyCode::KeyL,
                'm' => KeyCode::KeyM,
                'n' => KeyCode::KeyN,
                'o' => KeyCode::KeyO,
                'p' => KeyCode::KeyP,
                'q' => KeyCode::KeyQ,
                'r' => KeyCode::KeyR,
                's' => KeyCode::KeyS,
                't' => KeyCode::KeyT,
                'u' => KeyCode::KeyU,
                'v' => KeyCode::KeyV,
                'w' => KeyCode::KeyW,
                'x' => KeyCode::KeyX,
                'y' => KeyCode::KeyY,
                'z' => KeyCode::KeyZ,
                ',' => KeyCode::Comma,
                '0'..='9' => [
                    KeyCode::Digit0,
                    KeyCode::Digit1,
                    KeyCode::Digit2,
                    KeyCode::Digit3,
                    KeyCode::Digit4,
                    KeyCode::Digit5,
                    KeyCode::Digit6,
                    KeyCode::Digit7,
                    KeyCode::Digit8,
                    KeyCode::Digit9,
                ][c as usize - '0' as usize],
                _ => return None,
            },
            ShortcutKey::F(n) => match n {
                1 => KeyCode::F1,
                2 => KeyCode::F2,
                3 => KeyCode::F3,
                4 => KeyCode::F4,
                5 => KeyCode::F5,
                6 => KeyCode::F6,
                7 => KeyCode::F7,
                8 => KeyCode::F8,
                9 => KeyCode::F9,
                10 => KeyCode::F10,
                11 => KeyCode::F11,
                12 => KeyCode::F12,
                _ => return None,
            },
            ShortcutKey::Space => KeyCode::Space,
            ShortcutKey::Home => KeyCode::Home,
            ShortcutKey::End => KeyCode::End,
        })
    }

    /// True when `k` is this shortcut, with exactly these modifiers.
    pub fn matches(&self, k: &KeyPress) -> bool {
        let m = k.modifiers;
        if m.command() != self.command || m.shift != self.shift || m.alt != self.alt {
            return false;
        }
        // Ctrl on macOS is not Cmd: Ctrl+S must not save there.
        if cfg!(target_os = "macos") && m.ctrl {
            return false;
        }
        match self.key {
            ShortcutKey::Char(c) if !self.alt => match &k.key {
                Key::Character(s) => s.to_lowercase() == c.to_string(),
                _ => false,
            },
            ShortcutKey::F(n) => {
                let named = [
                    NamedKey::F1,
                    NamedKey::F2,
                    NamedKey::F3,
                    NamedKey::F4,
                    NamedKey::F5,
                    NamedKey::F6,
                    NamedKey::F7,
                    NamedKey::F8,
                    NamedKey::F9,
                    NamedKey::F10,
                    NamedKey::F11,
                    NamedKey::F12,
                ]
                .get(usize::from(n).wrapping_sub(1))
                .copied();
                named.is_some_and(|f| k.key == Key::Named(f))
                    || (k.code.is_some() && k.code == self.code())
            }
            ShortcutKey::Char(_) => k.code.is_some() && k.code == self.code(),
            ShortcutKey::Space => matches!(k.key, Key::Named(NamedKey::Space)),
            ShortcutKey::Home => matches!(k.key, Key::Named(NamedKey::Home)),
            ShortcutKey::End => matches!(k.key, Key::Named(NamedKey::End)),
        }
    }

    /// Whether the platform menu should own this key. Plain keys (Space,
    /// Home) stay with the window so text fields and the computer keyboard
    /// keep them; function keys are safe.
    pub fn native(&self) -> bool {
        self.command || matches!(self.key, ShortcutKey::F(_))
    }
}

/// The entries the in-window strip shows for `m`, with the command id of
/// each row.
fn flatten(m: &Menu) -> (Vec<crate::MenuItem>, Vec<Option<u32>>) {
    fn row(c: &MenuCommand) -> crate::MenuItem {
        let mut item = crate::MenuItem::new(c.label.clone()).enabled(c.enabled);
        if let Some(s) = &c.shortcut {
            item = item.shortcut(s.label());
        }
        if let Some(on) = c.checked {
            item = item.checked(on);
        }
        item
    }
    fn walk(m: &Menu, items: &mut Vec<crate::MenuItem>, ids: &mut Vec<Option<u32>>) {
        for e in &m.entries {
            match e {
                MenuEntry::Command(c) => {
                    items.push(row(c));
                    ids.push(Some(c.id));
                }
                MenuEntry::Separator => {
                    if items
                        .last()
                        .is_some_and(|i| !matches!(i, crate::MenuItem::Separator))
                    {
                        items.push(crate::MenuItem::Separator);
                        ids.push(None);
                    }
                }
                MenuEntry::Submenu {
                    inline: Some(c), ..
                } => {
                    items.push(row(c));
                    ids.push(Some(c.id));
                }
                MenuEntry::Submenu {
                    menu,
                    enabled,
                    inline: None,
                } => {
                    if *enabled && !menu.entries.is_empty() {
                        items.push(crate::MenuItem::Heading(menu.title.clone()));
                        ids.push(None);
                        walk(menu, items, ids);
                    }
                }
                MenuEntry::Standard(_) => {}
            }
        }
    }
    let mut items = Vec::new();
    let mut ids = Vec::new();
    walk(m, &mut items, &mut ids);
    while matches!(items.last(), Some(crate::MenuItem::Separator)) {
        items.pop();
        ids.pop();
    }
    (items, ids)
}

impl crate::Ui {
    /// Width the in-window strip needs for `bar`.
    pub fn menu_bar_width(&mut self, bar: &MenuBar) -> f32 {
        let t = self.theme.clone();
        bar.menus
            .iter()
            .filter(|m| m.role != MenuRole::App)
            .map(|m| self.measure(&m.title, t.label).x + 18.0)
            .sum()
    }

    /// The in-window menu strip, for platforms without a native menu bar.
    /// Returns the command chosen this frame. Hovering another title while
    /// one menu is open switches to it, as in a menu bar.
    pub fn menu_bar(&mut self, id: crate::Id, r: crate::Rect, bar: &MenuBar) -> Option<u32> {
        let t = self.theme.clone();
        let shown: Vec<(usize, &Menu)> = bar
            .menus
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role != MenuRole::App)
            .collect();
        let popup = |i: usize| id.with(("menu", i));
        let open_one = shown
            .iter()
            .find(|(i, _)| self.is_popup_open(popup(*i)))
            .map(|(i, _)| *i);
        let mut x = r.x;
        let mut chosen = None;
        for (i, m) in shown {
            let w = self.measure(&m.title, t.label).x + 18.0;
            let cell = crate::Rect { x, w, ..r };
            x += w;
            let open = open_one == Some(i);
            let resp = self.quiet_button(id.with(("title", i)), cell, None, &m.title, false, open);
            if (resp.clicked && !open) || (open_one.is_some_and(|o| o != i) && resp.hovered) {
                self.open_popup(popup(i), cell);
            }
            let (items, ids) = flatten(m);
            if let Some(k) = self.menu(popup(i), crate::Ui::below(cell), &items) {
                chosen = ids.get(k).copied().flatten();
            }
        }
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modifiers;

    fn press(key: Key, code: Option<KeyCode>, command: bool, shift: bool, alt: bool) -> KeyPress {
        let mac = cfg!(target_os = "macos");
        KeyPress {
            key,
            code,
            repeat: false,
            modifiers: Modifiers {
                shift,
                alt,
                logo: command && mac,
                ctrl: command && !mac,
            },
            consumed: false,
        }
    }

    #[test]
    fn shortcuts_match_their_exact_modifiers() {
        let s = Key::Character("s".into());
        let save = Shortcut::cmd('s');
        let save_as = Shortcut::cmd('s').shift();
        assert!(save.matches(&press(s.clone(), Some(KeyCode::KeyS), true, false, false)));
        assert!(!save.matches(&press(s.clone(), Some(KeyCode::KeyS), true, true, false)));
        assert!(save_as.matches(&press(
            "S".into_key(),
            Some(KeyCode::KeyS),
            true,
            true,
            false
        )));
        assert!(!save.matches(&press(s, Some(KeyCode::KeyS), false, false, false)));
        // Option changes the character on macOS: matched by physical key.
        let editor = Shortcut::cmd('e').alt();
        let typed = press("´".into_key(), Some(KeyCode::KeyE), true, false, true);
        assert!(editor.matches(&typed));
        assert!(!Shortcut::cmd('e').matches(&typed));
        // Function keys with or without a physical code.
        let f6 = Shortcut::key(ShortcutKey::F(6));
        assert!(f6.matches(&press(Key::Named(NamedKey::F6), None, false, false, false)));
        assert!(!f6.matches(&press(Key::Named(NamedKey::F7), None, false, false, false)));
    }

    #[test]
    fn only_command_and_function_keys_go_to_native_menus() {
        assert!(Shortcut::cmd('n').native());
        assert!(Shortcut::key(ShortcutKey::F(8)).native());
        assert!(!Shortcut::key(ShortcutKey::Space).native());
        assert!(!Shortcut::key(ShortcutKey::Char('l')).native());
    }

    #[test]
    fn the_strip_flattens_submenus() {
        let mut file = Menu::new("File");
        file.push(MenuCommand::new(1, "New"));
        let mut recent = Menu::new("Open recent");
        recent.push(MenuCommand::new(2, "Song"));
        file.push(MenuEntry::Submenu {
            menu: recent,
            enabled: true,
            inline: None,
        });
        let mut styles = Menu::new("New from template");
        styles.push(MenuCommand::new(3, "House"));
        file.push(MenuEntry::Submenu {
            menu: styles,
            enabled: true,
            inline: Some(MenuCommand::new(4, "New from template…")),
        });
        file.separator();
        file.push(MenuEntry::Standard(StandardItem::Hide));
        let (items, ids) = flatten(&file);
        assert_eq!(ids, vec![Some(1), None, Some(2), Some(4)]);
        assert!(matches!(&items[1], crate::MenuItem::Heading(h) if h == "Open recent"));
        let bar = MenuBar { menus: vec![file] };
        assert_eq!(bar.command(3).map(|c| c.label.as_str()), Some("House"));
    }

    trait IntoKey {
        fn into_key(self) -> Key;
    }

    impl IntoKey for &str {
        fn into_key(self) -> Key {
            Key::Character(self.into())
        }
    }
}
