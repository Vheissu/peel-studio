//! Per-user view state. Never part of the musical document.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use peel_document::{Id, Time};
use peel_media::AudioFormat;
use peel_session::ExportJob;
use peel_ui::KeyCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockTab {
    Rack,
    PianoRoll,
    Mixer,
    Perform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Mixer strip size. Compact fits many tracks, Expanded names inserts and
/// shows level scales, Full adds sends and numeric fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MixerSize {
    Compact,
    #[default]
    Expanded,
    Full,
}

impl MixerSize {
    pub const ALL: [MixerSize; 3] = [MixerSize::Compact, MixerSize::Expanded, MixerSize::Full];
}

/// What the inspector shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Channel(Id),
    Track(Id),
    /// An audio clip in the Playlist.
    Clip(Id),
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
    /// The tempo strip under the ruler is open.
    pub show_tempo: bool,
    /// Frames left in which to fit the whole song into view (set for a
    /// newly opened project, while the window settles; cleared by any
    /// scroll or zoom).
    pub fit: u8,
}

/// Default pane sizes; the user's sizes are kept in `Prefs`.
pub const BROWSER_W: f32 = 248.0;
pub const INSPECTOR_W: f32 = 300.0;
pub const DOCK_H: f32 = 400.0;

/// Layout of the main window. Which panes show lives in `Prefs`
/// (`show_browser`, `show_inspector`, `show_scripts`, `show_editor`,
/// `show_help`) so scripts and the View menu share one source.
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
    pub mixer_size: MixerSize,
    /// Dock height before the mixer was given the whole window.
    pub mixer_restore_h: Option<f32>,
    /// The computer keyboard plays the selected channel (Z to M, Q to P).
    pub keys_enabled: bool,
    /// The instrument editor and its view state.
    pub inst: crate::instrument_editor::InstState,
    /// The audio input, recording and output routing sheet is open.
    pub io_open: bool,
    /// The large effect editor (insert, EQ, mastering) and its view state.
    pub fx: crate::fx_editor::FxState,
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
    pub export_format: AudioFormat,
    pub export_bitrate: u16,
    pub export_dither: bool,
    pub export_mono: bool,
    pub export_normalize: bool,
    pub export_rate: u32,
    pub export_tail: f32,
    /// Export one file per mixer track instead of the mix.
    pub export_stems: bool,
}

impl Workspace {
    /// A workspace for `project`: the first channel is selected.
    #[cfg(test)]
    pub fn new(project: &peel_document::Project) -> Workspace {
        let mut ws = Workspace::default();
        ws.selected_channel = project.channels.first().map(|c| c.id);
        ws.focus = ws.selected_channel.map(Focus::Channel);
        ws
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            input_meter: (0.0, 0.0),
            dock: DockTab::Rack,
            dock_h: DOCK_H,
            browser_w: BROWSER_W,
            inspector_w: INSPECTOR_W,
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
                show_tempo: false,
                fit: 8,
            },
            mixer_scroll: 0.0,
            keys_enabled: true,
            inst: Default::default(),
            io_open: false,
            mixer_size: MixerSize::default(),
            mixer_restore_h: None,
            fx: Default::default(),
            rack_scroll: 0.0,
            rack_graph: false,
            octave: 4,
            held: HashMap::new(),
            export: None,
            browser_scroll: 0.0,
            clipboard: Vec::new(),
            export_panel: false,
            export_bits: 24,
            export_format: AudioFormat::Wav,
            export_bitrate: 320,
            export_dither: true,
            export_mono: false,
            export_normalize: false,
            export_rate: 48_000,
            export_tail: 3.0,
            export_stems: false,
        }
    }
}
