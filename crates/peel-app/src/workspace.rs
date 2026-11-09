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

/// A named rhythmic value, in quarter-note beats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rhythm {
    pub len: Time,
    /// Compact toolbar form: 1/16, 1/8T, 1/4D.
    pub short: &'static str,
    /// Readable menu form: Sixteenth, Eighth triplet.
    pub name: &'static str,
}

const fn rhythm(num: i64, den: i64, short: &'static str, name: &'static str) -> Rhythm {
    Rhythm {
        len: Time::new_const(num, den),
        short,
        name,
    }
}

/// Plain note values, longest first.
pub const STRAIGHT: [Rhythm; 7] = [
    rhythm(4, 1, "1/1", "Whole"),
    rhythm(2, 1, "1/2", "Half"),
    rhythm(1, 1, "1/4", "Quarter"),
    rhythm(1, 2, "1/8", "Eighth"),
    rhythm(1, 4, "1/16", "Sixteenth"),
    rhythm(1, 8, "1/32", "Thirty-second"),
    rhythm(1, 16, "1/64", "Sixty-fourth"),
];

/// Three in the space of two.
pub const TRIPLETS: [Rhythm; 4] = [
    rhythm(2, 3, "1/4T", "Quarter triplet"),
    rhythm(1, 3, "1/8T", "Eighth triplet"),
    rhythm(1, 6, "1/16T", "Sixteenth triplet"),
    rhythm(1, 12, "1/32T", "Thirty-second triplet"),
];

/// One and a half times the plain value.
pub const DOTTED: [Rhythm; 4] = [
    rhythm(3, 1, "1/2D", "Dotted half"),
    rhythm(3, 2, "1/4D", "Dotted quarter"),
    rhythm(3, 4, "1/8D", "Dotted eighth"),
    rhythm(3, 8, "1/16D", "Dotted sixteenth"),
];

/// Step sizes the Channel Rack offers.
pub const RACK_STEPS: [Rhythm; 7] = [
    STRAIGHT[3],
    STRAIGHT[4],
    STRAIGHT[5],
    STRAIGHT[6],
    TRIPLETS[1],
    TRIPLETS[2],
    TRIPLETS[3],
];

/// The named value for `t`, if it has one.
pub fn rhythm_of(t: Time) -> Option<Rhythm> {
    STRAIGHT
        .iter()
        .chain(&TRIPLETS)
        .chain(&DOTTED)
        .find(|r| r.len == t)
        .copied()
}

/// What a new note in the Piano Roll is as long as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NoteLength {
    /// One grid cell.
    #[default]
    Grid,
    /// The last note clicked, drawn or resized.
    Last,
    /// A chosen value, whatever the grid.
    Fixed(Time),
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
    /// Grid spacing: where clicks, moves and resizes land.
    pub snap: Time,
    /// Length of the last note clicked, drawn or resized.
    pub note_len: Time,
    /// How long new notes are.
    pub length: NoteLength,
    pub tool: Tool,
    pub selection: HashSet<Id>,
    pub ghosts: bool,
    /// Scale highlighted and snapped to while drawing.
    pub scale: Option<crate::harmony::Scale>,
    /// Chord shape stamped when drawing (index into `harmony::CHORDS`).
    pub chord: usize,
    /// What the lower lane edits.
    pub lane: crate::note_expr::Lane,
    /// The note whose expression the lower lane edits.
    pub focus_note: Option<Id>,
    /// Pitch lane range in semitones either side of zero; None fits the note.
    pub pitch_range: Option<f32>,
}

impl RollView {
    /// Length of the next note drawn.
    pub fn draw_length(&self) -> Time {
        match self.length {
            NoteLength::Grid => self.snap,
            NoteLength::Last => self.note_len,
            NoteLength::Fixed(t) => t,
        }
    }
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
    /// Horizontal scroll of the step grid, in pixels.
    pub rack_scroll_x: f32,
    /// What one Channel Rack step is worth (a sixteenth by default).
    pub rack_step: Time,
    pub rack_graph: bool,
    pub octave: i32,
    /// Computer-keyboard notes currently held: physical key -> (channel, key).
    pub held: HashMap<KeyCode, (Id, u8)>,
    pub export: Option<ExportJob>,
    pub browser_scroll: f32,
    /// Notes copied in the Piano Roll.
    pub clipboard: Vec<peel_document::Note>,
    /// Expression of the copied notes, addressed by their original IDs.
    pub clipboard_expr: Vec<peel_document::NoteExpression>,
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
                length: NoteLength::Grid,
                tool: Tool::Draw,
                selection: HashSet::new(),
                ghosts: true,
                scale: None,
                chord: 0,
                lane: Default::default(),
                focus_note: None,
                pitch_range: None,
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
            rack_scroll_x: 0.0,
            rack_step: Time::new(1, 4),
            rack_graph: false,
            octave: 4,
            held: HashMap::new(),
            export: None,
            browser_scroll: 0.0,
            clipboard: Vec::new(),
            clipboard_expr: Vec::new(),
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
