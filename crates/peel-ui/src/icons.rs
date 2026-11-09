//! Phosphor icon codepoints (MIT, see `assets/fonts/phosphor/LICENSE`).
//! Draw with `Ui::icon` (regular weight) or `Ui::icon_fill`.

pub const PLAY: &str = "\u{e3d0}";
pub const PAUSE: &str = "\u{e39e}";
pub const STOP: &str = "\u{e46c}";
pub const RECORD: &str = "\u{e3ee}";
pub const REPEAT: &str = "\u{e3f6}";
pub const SKIP_BACK: &str = "\u{e5a4}";
pub const SKIP_FORWARD: &str = "\u{e5a6}";
pub const REWIND: &str = "\u{e6a8}";
pub const FAST_FORWARD: &str = "\u{e6a6}";
pub const METRONOME: &str = "\u{ec8e}";
pub const WAVEFORM: &str = "\u{e802}";
pub const PIANO_KEYS: &str = "\u{e9c8}";
pub const FADERS: &str = "\u{e228}";
pub const SLIDERS: &str = "\u{e434}";
pub const FOLDER: &str = "\u{e24a}";
pub const FOLDER_OPEN: &str = "\u{e256}";
pub const MUSIC_NOTES: &str = "\u{e340}";
pub const MUSIC_NOTE: &str = "\u{e33c}";
pub const BELL: &str = "\u{e0ce}";
pub const PLUS: &str = "\u{e3d4}";
pub const MINUS: &str = "\u{e32a}";
pub const X: &str = "\u{e4f6}";
pub const SEARCH: &str = "\u{e30c}";
pub const GEAR: &str = "\u{e272}";
pub const SAVE: &str = "\u{e248}";
pub const UNDO: &str = "\u{e038}";
pub const REDO: &str = "\u{e036}";
pub const PENCIL: &str = "\u{e3b4}";
pub const BRUSH: &str = "\u{e6f0}";
pub const ERASER: &str = "\u{e21e}";
pub const CURSOR: &str = "\u{e1dc}";
pub const SCISSORS: &str = "\u{eae0}";
pub const SPEAKER: &str = "\u{e44a}";
pub const SPEAKER_MUTE: &str = "\u{e45a}";
pub const HEADPHONES: &str = "\u{e2a6}";
pub const DOTS: &str = "\u{e1fe}";
pub const GRIP: &str = "\u{eae2}";
pub const CARET_DOWN: &str = "\u{e136}";
pub const CARET_RIGHT: &str = "\u{e13a}";
pub const CARET_LEFT: &str = "\u{e138}";
pub const CARET_UP: &str = "\u{e13c}";
pub const GRID: &str = "\u{e296}";
pub const LIST: &str = "\u{e2f0}";
pub const SPARKLE: &str = "\u{e6a2}";
pub const WAVE_SINE: &str = "\u{ea9a}";
pub const WAVE_SQUARE: &str = "\u{ea9e}";
pub const WAVE_SAW: &str = "\u{ea9c}";
pub const WAVE_TRIANGLE: &str = "\u{eaa0}";
pub const EXPORT: &str = "\u{eaf0}";
pub const FILE_AUDIO: &str = "\u{ea20}";
pub const CPU: &str = "\u{e610}";
pub const EQUALIZER: &str = "\u{ebbc}";
pub const DROP: &str = "\u{e210}";
pub const KNIFE: &str = "\u{e636}";
pub const LOCK: &str = "\u{e2fa}";
pub const LINK: &str = "\u{e2e2}";
pub const COPY: &str = "\u{e1ca}";
pub const TRASH: &str = "\u{e4a6}";
pub const MAGNET: &str = "\u{e680}";
pub const LIGHTNING: &str = "\u{e2de}";
pub const PLUGS: &str = "\u{e946}";
pub const STAR: &str = "\u{e46a}";
pub const SQUARES: &str = "\u{e464}";
pub const ROWS: &str = "\u{e5a2}";
pub const COLUMNS: &str = "\u{e546}";
pub const QUESTION: &str = "\u{e3e8}";
pub const KEYBOARD: &str = "\u{e2d8}";
pub const LAYOUT: &str = "\u{e6d6}";
pub const CODE: &str = "\u{e1bc}";
pub const CARET_DOUBLE_LEFT: &str = "\u{e128}";
pub const CARET_DOUBLE_RIGHT: &str = "\u{e12a}";
pub const INFO: &str = "\u{e2ce}";

/// Plain-language names for the icons, used as accessible names when an
/// icon is the only content of a control.
const NAMES: &[(&str, &str)] = &[
    (PLAY, "Play"),
    (PAUSE, "Pause"),
    (STOP, "Stop"),
    (RECORD, "Record"),
    (REPEAT, "Loop"),
    (METRONOME, "Metronome"),
    (WAVEFORM, "Waveform"),
    (PIANO_KEYS, "Piano keys"),
    (FADERS, "Faders"),
    (SLIDERS, "Sliders"),
    (FOLDER, "Folder"),
    (FOLDER_OPEN, "Open folder"),
    (MUSIC_NOTES, "Music notes"),
    (MUSIC_NOTE, "Music note"),
    (BELL, "Bell"),
    (PLUS, "Plus"),
    (MINUS, "Minus"),
    (X, "Close"),
    (SEARCH, "Search"),
    (GEAR, "Settings"),
    (SAVE, "Save"),
    (UNDO, "Undo"),
    (REDO, "Redo"),
    (PENCIL, "Pencil"),
    (BRUSH, "Brush"),
    (ERASER, "Eraser"),
    (CURSOR, "Select"),
    (SCISSORS, "Scissors"),
    (SPEAKER, "Speaker"),
    (SPEAKER_MUTE, "Muted speaker"),
    (HEADPHONES, "Headphones"),
    (DOTS, "More"),
    (GRIP, "Drag handle"),
    (CARET_DOWN, "Down"),
    (CARET_RIGHT, "Right"),
    (CARET_LEFT, "Left"),
    (CARET_UP, "Up"),
    (GRID, "Grid"),
    (LIST, "List"),
    (SPARKLE, "Assist"),
    (WAVE_SINE, "Sine wave"),
    (WAVE_SQUARE, "Square wave"),
    (WAVE_SAW, "Sawtooth wave"),
    (WAVE_TRIANGLE, "Triangle wave"),
    (EXPORT, "Export"),
    (FILE_AUDIO, "Audio file"),
    (CPU, "CPU"),
    (EQUALIZER, "Equalizer"),
    (DROP, "Drop"),
    (KNIFE, "Slice"),
    (LOCK, "Lock"),
    (LINK, "Link"),
    (COPY, "Copy"),
    (TRASH, "Trash"),
    (MAGNET, "Magnet"),
    (LIGHTNING, "Lightning"),
    (PLUGS, "Plug"),
    (STAR, "Star"),
    (SQUARES, "Tiles"),
    (ROWS, "Rows"),
    (COLUMNS, "Columns"),
    (QUESTION, "Help"),
    (KEYBOARD, "Keyboard"),
    (LAYOUT, "Layout"),
    (CODE, "Code"),
    (CARET_DOUBLE_LEFT, "Collapse left"),
    (CARET_DOUBLE_RIGHT, "Collapse right"),
    (INFO, "Information"),
];

/// The accessible name of an icon glyph from this module, if it has one.
pub fn name(glyph: &str) -> Option<&'static str> {
    NAMES.iter().find(|(g, _)| *g == glyph).map(|(_, n)| *n)
}
