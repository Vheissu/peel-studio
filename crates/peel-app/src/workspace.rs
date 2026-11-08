//! Per-user view state. Never part of the musical document.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use peel_document::{Id, Time};
use peel_session::ExportJob;
use peel_ui::KeyCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockTab {
    Rack,
    PianoRoll,
    Mixer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserTab {
    Instruments,
    Effects,
    Samples,
    Patterns,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Draw,
    Paint,
    Select,
    Erase,
    Slice,
}

/// What the inspector shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Channel(Id),
    Track(Id),
}

pub struct RollView {
    /// Pixels per quarter note.
    pub zoom_x: f32,
    /// Pixels per key row.
    pub row_h: f32,
    /// Horizontal scroll in beats.
    pub scroll_beats: f32,
    /// Highest visible key at the top edge (fractional).
    pub top_key: f32,
    pub snap: Time,
    pub note_len: Time,
    pub tool: Tool,
    pub selection: HashSet<Id>,
    pub ghosts: bool,
    /// Scale highlighted and snapped to while drawing.
    pub scale: Option<crate::harmony::Scale>,
    /// Chord shape stamped when drawing (index into `harmony::CHORDS`).
    pub chord: usize,
}

pub struct PlaylistView {
    pub zoom_x: f32,
    pub scroll_beats: f32,
    pub scroll_y: f32,
    pub lane_h: f32,
    pub snap: Time,
    pub tool: Tool,
    pub selection: HashSet<Id>,
}

pub struct Workspace {
    /// Input peak with decay (left, right), read once per frame.
    pub input_meter: (f32, f32),
    pub dock: DockTab,
    pub dock_h: f32,
    pub browser_w: f32,
    pub inspector_w: f32,
    pub browser_tab: BrowserTab,
    pub search: String,
    pub sample_dir: Option<PathBuf>,
    pub selected_channel: Option<Id>,
    pub focus: Option<Focus>,
    pub roll: RollView,
    pub playlist: PlaylistView,
    pub mixer_scroll: f32,
    pub rack_scroll: f32,
    pub rack_graph: bool,
    pub octave: i32,
    /// Computer-keyboard notes currently held: physical key -> (channel, key).
    pub held: HashMap<KeyCode, (Id, u8)>,
    pub export: Option<ExportJob>,
    pub browser_scroll: f32,
    /// Notes copied in the Piano Roll.
    pub clipboard: Vec<peel_document::Note>,
    pub export_panel: bool,
    pub export_bits: u16,
    pub export_rate: u32,
    pub export_tail: f32,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            input_meter: (0.0, 0.0),
            dock: DockTab::Rack,
            dock_h: 400.0,
            browser_w: 236.0,
            inspector_w: 300.0,
            browser_tab: BrowserTab::Instruments,
            search: String::new(),
            sample_dir: dirs::audio_dir().or_else(dirs::home_dir),
            selected_channel: None,
            focus: None,
            roll: RollView {
                zoom_x: 96.0,
                row_h: 14.0,
                scroll_beats: 0.0,
                top_key: 84.0,
                snap: Time::new(1, 4),
                note_len: Time::new(1, 4),
                tool: Tool::Draw,
                selection: HashSet::new(),
                ghosts: true,
                scale: None,
                chord: 0,
            },
            playlist: PlaylistView {
                zoom_x: 22.0,
                scroll_beats: 0.0,
                scroll_y: 0.0,
                lane_h: 52.0,
                snap: Time::beats(1),
                tool: Tool::Draw,
                selection: HashSet::new(),
            },
            mixer_scroll: 0.0,
            rack_scroll: 0.0,
            rack_graph: false,
            octave: 4,
            held: HashMap::new(),
            export: None,
            browser_scroll: 0.0,
            clipboard: Vec::new(),
            export_panel: false,
            export_bits: 24,
            export_rate: 48_000,
            export_tail: 3.0,
        }
    }
}
