---
name: Peel Studio
description: Pattern-and-playlist DAW. Source of truth is Theme::dark() in crates/peel-ui/src/theme.rs.
colors:
  bg: "#0C0B0A"
  panel: "#1A1917"
  panel-alt: "#211F1D"
  raised: "#2B2926"
  control: "#35322E"
  control-hover: "#423E39"
  control-active: "#504B45"
  line: "#2D2B28"
  line-strong: "#403C37"
  well: "#121110"
  text: "#F3EEE4"
  text-dim: "#B9B2A6"
  text-faint: "#999286"
  text-on-accent: "#1F1305"
  accent: "#F7992B"
  focus: "#7FC8EA"
  record: "#F2553F"
  play: "#9FD65B"
  warn: "#EFC53F"
  shadow: "rgba(0, 0, 0, 0.55)"
  grid-bar: "rgba(255, 245, 230, 0.13)"
  grid-beat: "rgba(255, 245, 230, 0.065)"
  grid-step: "rgba(255, 245, 230, 0.03)"
  key-white: "#E6DFD0"
  step-key-1: "#E8503C"
  step-key-2: "#F28A2E"
  step-key-3: "#F0C63E"
  step-key-4: "#EFE7D6"
  display: "#FFB454"
  key-black: "#252320"
typography:
  caption:
    fontFamily: "Barlow Semi Condensed"
    fontSize: "11.5px"
    fontWeight: 500
  body:
    fontFamily: "Barlow Semi Condensed"
    fontSize: "13px"
    fontWeight: 500
  label:
    fontFamily: "Barlow Semi Condensed"
    fontSize: "13px"
    fontWeight: 600
  heading:
    fontFamily: "Barlow Semi Condensed"
    fontSize: "14px"
    fontWeight: 600
  readout:
    fontFamily: "Barlow Semi Condensed"
    fontSize: "19px"
    fontWeight: 600
    fontFeature: "tnum"
  readout-small:
    fontFamily: "Barlow Semi Condensed"
    fontSize: "12px"
    fontWeight: 500
    fontFeature: "tnum"
rounded:
  s: "2px"
  m: "4px"
  l: "6px"
spacing:
  unit: "4px"
  seam: "2px"
  control-h: "24px"
  control-h-compact: "20px"
  rail-w: "36px"
  header-h: "32px"
  top-bar-h: "40px"
  status-h: "24px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.text-on-accent}"
    typography: "{typography.label}"
    rounded: "{rounded.m}"
    height: "{spacing.control-h}"
  button-secondary:
    backgroundColor: "{colors.control}"
    textColor: "{colors.text}"
    typography: "{typography.label}"
    rounded: "{rounded.m}"
    height: "{spacing.control-h}"
  button-secondary-hover:
    backgroundColor: "{colors.control-hover}"
  button-secondary-active:
    backgroundColor: "{colors.control-active}"
  text-field:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.text}"
    typography: "{typography.body}"
    rounded: "{rounded.m}"
    padding: "0 8px"
    height: "{spacing.control-h}"
---

# Design

The frontmatter is normative and mirrors `Theme::dark()`. Change the theme
first, then this file.

## Reference

The front panel of a Roland TR-808 and the drum machines that followed it:
a warm black panel, cream legends, and step keys in red, orange, yellow
and cream, four to a beat. Peel Studio takes that palette and its sense of
hardware, with controls that have weight and displays that sit in the
panel. It does not copy the artwork. The layout is a pattern-and-playlist
workspace in the FL Studio mould: compact toolbars and continuous editing
surfaces. The chrome stays quiet; clips, notes, lit steps, waveforms and
the playhead are the brightest things on screen, and a channel's color
identifies it everywhere it appears.

## Color

| Token | Value | Use |
| --- | --- | --- |
| `bg` | `#0C0B0A` | Seams between panes, rails, status bar, grooves, display windows |
| `panel` | `#1A1917` | Panes and the top bar |
| `panel_alt` | `#211F1D` | Editor canvases, mixer strips |
| `raised` | `#2B2926` | Selected rows, hovered lane headers |
| `control` | `#35322E` | Buttons, channel buttons, knob caps |
| `well` | `#121110` | Recessed trays: transport groups, insert slots, scrollbar tracks |
| `text` | `#F3EEE4` | Primary text, knob pointers, fader index lines |
| `text_dim` | `#B9B2A6` | Secondary text |
| `text_faint` | `#999286` | Labels and hints |
| `accent` | `#F7992B` | Primary action, playhead, the peel |
| `display` | `#FFB454` | Position and tempo figures in the time display |
| `play` | `#9FD65B` | Play state, signal present, meters below -6 dB (orange above, red at clip) |
| `record` | `#F2553F` | Recording and clipping |
| `focus` | `#7FC8EA` | Keyboard focus and drop targets only, never text |
| `step_keys` | `#E8503C` `#F28A2E` `#F0C63E` `#EFE7D6` | Step keys by beat of the bar |

The neutrals lean warm so they sit with the orange; none of them is a
pure grey.

Channel colors come from a 12-entry palette (`peel_document::PALETTE`).
It starts on the step keys' red, orange and yellow and continues round the
wheel at the same weight, and every entry takes dark text. A channel's
color carries into its rack button, piano roll notes, mixer name plate
and playlist clips. Chrome never borrows it.

Light and shadow are literal. Things you press or turn (buttons, knob
caps, fader caps, lit step keys) are lit slightly from above. A lit lamp
spills a little of its color onto the panel. Shadows appear only under
things that float above the workspace: menus, tooltips, sheets and drag
previews. Panes, canvases and text carry no gradients.

## Type

Barlow Semi Condensed (SIL OFL), a low-contrast grotesk drawn from
California highway signage. It reads at a glance, and the semi-condensed
width fits names into narrow mixer strips and rack buttons. One family
carries every role through size and weight. Readouts (position, tempo,
levels) use its tabular figures so numbers do not jitter while they change.

| Role | Size | Weight |
| --- | --- | --- |
| Caption | 11.5 | Medium |
| Body | 13 | Medium |
| Label | 13 | SemiBold |
| Heading | 14 | SemiBold |
| Readout | 19 | SemiBold, tabular |
| Small readout | 12 | Medium, tabular |

Pane titles use Label, not Heading: they name the pane and get out of the
way.

## Shape

4 px base unit. Corner radius 2 px for small controls, step keys and
insert slots, 3 px for clips, 4 px for buttons and menus, 6 px for sheets.
Panes, editor canvases and mixer strips are square.

Panes are not cards. They meet at a 2 px `bg` seam that doubles as the
splitter, and a splitter's grab zone is 8 px wide whatever its seam. The
splitters are handled after the panes draw, so the seam wins over a pane
edge that reaches into it. Inside a pane, a 1 px `line` separates the
header from the body. Editors run edge to edge: the playlist's lane
headers, ruler and canvas meet at separators, as do the piano roll's keys,
canvas and velocity lane. Nothing is nested more than one surface deep.

Pane headers are 32 px, rails 36 px wide, the top bar 40 px and the
status bar 24 px. Rack rows are 26 px with 2 px between them; browser rows
are 24 px.

## Layout

```
top bar:  project · View · undo redo | [playback well] [time well] | meter · open save · Export
          rail | browser | playlist             | right column | rail
               |         | dock (editor tabs)   |              |
status:   notices                                  device · MIDI · settings
```

- The playlist always shows and takes whatever the panes leave.
- The left rail picks the browser tab; clicking the showing tab hides
  the browser. The right rail switches the right column between the
  Inspector and Scripts (never both) and holds the usage-hints toggle
  and keyboard shortcuts. Rails sit on `bg`, so they read as navigation;
  the selected entry gets a 2 px accent bar on the edge facing its pane.
- The dock collapses to its 32 px tab header, so every editor stays one
  click away. Its tabs keep their labels while the editor's tools fit
  beside them, otherwise all four become icons together.
- The pattern picker sits in the dock header beside the Channels and
  Piano roll editors, the work it targets.
- Side panes cap themselves at about a quarter (browser) and a third
  (right column) of narrow windows without losing the user's size.
- The time well is a display window: position and tempo in `display` on
  `bg`.
- The top bar measures itself and sheds, in order: the load figure,
  rewind and fast-forward, the meter, the Export label, the clock, the
  project title and finally the View label.

## Menus

File, Edit, View, Transport and Help live in the platform menu bar on
macOS (with the application and Window menus). Windows and Linux use an
in-window strip, so File and Edit sit before the View button and
Transport and Help after it; a tight bar keeps File only. Menus, their
shortcuts and the start screen run the same commands (`menus.rs`).

New project (Cmd/Ctrl+N) is blank: no channels, notes, clips or effects.
Templates come from `template::templates()` and demo songs from
`template::demos()`; File lists both. Close project (Cmd/Ctrl+W) returns
to the start screen. New, a template, a demo song, Open, Recent, Recover,
Close, Quit and closing the window first finish any recording, then ask
Save, Don't save or Cancel when there is unsaved work.

## Start screen

One sheet in three columns: start something (New, Open, the templates as
a two-column grid, My templates), hear something (the demo songs) and
pick up where you left off (recovered and recent projects). Each demo
song is a row with a play tile in a step-key color, its title, and its
style, tempo and key. The tallest column sets the sheet's height, and it
fits the minimum window.

## Visibility

The View menu, rails, pane headers and shortcuts (Cmd/Ctrl + Opt/Alt +
B, I, E; F8 for Scripts; F6, F7, F9, F10 reveal their editor) all change
the same `Prefs` flags that scripts read and set with `ui.get`/`ui.set`:
`show_browser`, `show_inspector`, `show_scripts`, `show_editor`,
`show_help`. Pane sizes, tabs, the master meter and status details are
saved next to them and survive opening other projects. Reset layout in
the View menu restores the defaults.

## Controls

| Tier | Look | Used for |
| --- | --- | --- |
| Primary | `accent` fill, `text_on_accent` | Export, Run. One per region. |
| Secondary | `control` fill | Dialog buttons, dropdown values in property rows |
| Quiet | no fill until hovered; tinted when on | Toolbars, rails, transport, pane headers |

Related quiet controls share a `well` with 1 px `line_strong` dividers
between groups (transport, time, editing tools, pattern length). A tool
that is one of a set (Draw, Select, Erase) shows the selection the way
the segmented control does. Tabs are text with a 2 px accent underline.
Property rows put a 72 px caption label left of the value.

Knobs are a value arc in a dark groove around a cap lit from above, with
a cream pointer. The arc carries the value in the control's color, and
bipolar knobs fill from the top. Sizes: 22 px in the rack, 28 px for mixer
pan, 34 px in the inspector. Inspector knob cells are at least 76 px and
stretch to fill the row, so a default inspector shows three columns with
whole parameter names. A name that still does not fit is cut and shown in
full in the knob's tooltip. Faders run in a 5 px groove filled in the
track's color below the cap, with ticks on one side and a cap with a cream
index line. LEDs are lamps: dark when off, and lit with a small halo of
their own color.

When a toolbar runs short, names go before controls: the pattern picker
shrinks to its swatch and the piano roll's channel to its color, both
keeping the name in their tooltip and accessible label.

Wide device panels flow parameter groups left to right in descriptor order,
with 16 px gaps, then wrap onto another line. Cells may shrink to 68 px in
these panels. Narrow panels keep the stacked groups. Layout and scrolling
use the same group measurements.

The preset row has previous and next buttons at editor width. They wrap
through the factory presets, with each load as one undo step. Changed
values show Edited and a Revert button; narrow panels put “edited” in the
preset name and let the preset menu reload it.

## Step grid

Each step is a key. Keys take one of four colors by beat of the bar (red,
orange, yellow, cream), so a bar reads in beats the way a drum machine's
keys do, with a wider gap between beats than between steps. An unlit key
is a dark tint of its color; a hit lights it, dimmer the softer it is
played. The playing step brightens. The channel's own color stays on its
name button, its knobs and its note preview.

The Add menu lists kits above the instruments, under their own heading. A
kit adds one row and one mixer track per piece, as a single undo step, and
is refused whole when the mixer has no room for all of it.

## Rhythm

The rack's Steps picker sets the spacing of drum hits, independently of the
pattern's bar count. It offers eighths through sixty-fourths and triplets.
Names and knobs stay fixed while a long grid scrolls sideways. Existing
hits between the selected steps stay visible and are preserved.

The Piano Roll separates Grid from Note length. Length can follow the grid,
reuse the last note, or use a fixed straight, triplet or dotted value.
Picking a fine grid zooms in once when needed to keep cells clickable.
Grid lines follow the selected division and thin out at low zoom. Clicking
a note only selects it; moving or resizing requires a deliberate drag.

## Mixer

Strips sit side by side with a 1 px seam. Each name sits on a plate in
the channel color, like a console's scribble strip; the selected strip's
plate is at full strength. Insert slots are recessed into a `well`.
Meters are LED ladders, one lamp every 3 px. Strips scroll sideways
with the master pinned on the right. Three sizes
in the dock header (Compact 64 px, Expanded 92, Full 124) decide which
rows a strip has; the dock height decides how many insert and send rows
fit, so the fader keeps at least about 96 px. When it cannot, rows go in a
fixed order: the send list becomes a Sends menu, then the pan legend, then
an insert row. Strip size and area are separate choices: Expand mixer makes
the mixer take the whole area between the panes while the playlist steps
aside, so a tall window shows long faders and full send lists.

Every strip shows real engine state: a scrolling insert chain (all inserts,
a dot that bypasses, a click that opens the editor, a right-click menu),
output and send routing, pan with its legend, a fader with a dB scale and
a number you can drag or type, a stereo meter with 1.2 s peak hold, a
peak readout that latches coral at 0 dBFS and clears on click, mute and
solo. Master carries a Mastering button that adds the effect once and
opens it. Telemetry is read once per frame and shared with the top-bar
meter.

## Effect editor

An insert click opens a sheet over the workspace (up to 1120 x 700, on the
overlay layer, between the top and status bars) so it fits any pane
combination at 1200 x 720. The header names the track, lists its whole
chain as chips, and holds Bypass, Remove and Close. Esc, Close or a click
on the dimmed area closes it. EQ and Mastering have their own editors;
every other effect shows its full parameter panel at editor size. The
inspector keeps its compact panels.

Resonator shows its four tuned notes beside the title, read from the same
tuning helper as its audio engine. Percussion has its own browser category
with a bell icon for Mallet.

## EQ

The plot is log frequency, 20 Hz to 20 kHz, with two scales that never
share a label: gain in `accent` on the right (the transfer function and
nodes), dBFS in `text_faint` on the left (spectra). The curve is the
effect's own `eq_response_db`; each enabled band fills its contribution in
its colour at 10 %. Spectra come from the analysis tap: Pre in `text_faint`,
Post in `play`, peak hold as a thin line, plus a 128-point output peak
history under the plot with peak and RMS figures (dBFS, never LUFS). With
no new frame for a quarter second the spectra fall and candidates clear.
Nodes: drag for frequency and gain, scroll for Q, double-click to switch a
band, Shift for fine, arrow keys for gain. The rail repeats every value as
a numeric field, with presets, A/B compare and Reset above the plot.

The Resonance tab lists up to three sustained narrow peaks as candidates
to check by ear. Preview draws the curve a cut would make; audio does not
change. Apply writes a bounded bell cut (at most 6 dB) to the selected
bell, or the first free one; if both are in use the button reads Replace
Band N and needs a second click. Held results stay with the EQ until
cleared.

## Mastering

Simple and Advanced are two views of the same parameters; switching never
changes a value. A chain strip (Input, Tone, Compress, Width, Limit)
summarizes each stage, and a click opens it in Advanced. Meters are real:
input and output peak and RMS in dBFS (labelled as such), the ceiling mark,
and gain reduction as a number, a bar and a history. The tone and
compressor curves are computed from the same filters and constants the
effect uses.

## Instrument editor

A sheet like the effect editor, opened from a channel's context menu in
the rack, the piano-keys button in the inspector, or its menu. Opening is
not an edit. The header lists every channel as a chip so you can move from
one instrument to the next without closing; the body is the instrument's
full parameter panel (plugin instruments show the same panel: the host
does not embed native plugin windows, and the sheet says so). Below it
sits a piano of three or four octaves from the current octave's C. The
computer keyboard letters are printed on the keys they play. Click plays,
dragging across keys glides, release stops. The rail holds Velocity,
Octave, Pitch and Pressure, a Keyboard switch for the computer keys, a
Sustain switch, MIDI status (an activity light, port summary, channel
filter, bend range) and the controller mapper: pick a parameter, Learn,
move a control; each mapping lists its CC and the parameter's live value,
with Unmap. Labels say what is supported: notes, sustain, pitch bend,
aftertouch and CC knobs, all for the one selected instrument.

Pitch and Pressure are horizontal springs in the Play section. Pitch draws
from a centre tick and reads in semitones of the chosen Bend range; Pressure
draws from the left and reads in percent. Holding the mouse drives the
instrument, and letting go returns each to rest. When the mouse is not on a
control it shows the value the session last accepted, so a hardware wheel
moves it too and the editor never sends a stale centre over the top. Neither
is saved in the project. The Bend range picker (Off to 48 st) is a machine
setting kept in preferences and applied at startup and project reset.

MIDI has an MPE row under the port summary: Off, Lower zone or Upper zone.
Off keeps the Channel filter and Bend range. A zone replaces them with
Members (count and channels, numbered from 1, with the manager channel in
the tooltip), Note bend (the member channels' range) and Zone bend (the
manager's). The setup is a machine preference applied at startup and
project reset; the rows show the live setup, so a controller that changes
it over RPN is not overwritten.

Notes never hang. Mouse notes end on release, when the pointer leaves the
window with the button up, when the editor closes, when the target changes
and when the window loses focus; the same focus loss ends held computer
keys and centres a held Pitch or Pressure, and Panic silences every channel.

## Slides

A mono instrument that glides (the 808 with Glide on, the mono bass in a
legato mode) slides when notes overlap. The Piano Roll draws each slide as
a cream line from a dot on the held note to the note it moves to, over a
dark underlay so it reads on any note color. It follows last-note
priority: when the newer note ends first, a second line returns to the
note still held. The lines are drawn from the notes, never edited
directly; move or resize a note and they follow.

## Piano roll lanes

The lane under the notes has a picker in the key column: Velocity, Pitch,
Pressure, Timbre, stacked as a vertical segmented control (arrow keys move
it). Velocity works as it always has. The other three edit one note at a
time: the only selected note, or the one last clicked. Its span is tinted
and its curve drawn in the channel color; other notes' curves show faint
behind it. Clicking another note's span in the lane switches to it.

Curves are recorded events held until the next one, so they draw as steps,
never ramps. Before its first point a note plays the neutral value (0 st,
0 %, 50 %), drawn as a thin lead-in. Pitch reads in semitones over a range
that fits the note (±2 to ±120 st; scroll to change it), Pressure and
Timbre in percent. Click inside the note to add a point, drag to move it
(snapped to the grid; Alt for free), right-click or Delete over a point to
remove it, Clear to empty the lane. Each gesture is one undo step. Points
stay inside the note and in order. Dense recorded takes show the line and
only the handle under the pointer.

Curves move with their notes: copy, paste, duplicate, the note tools and
Make unique carry them; delete, erase and cut drop them; shortening a note
trims them.

## Audio sheet

One sheet, opened from the Audio button in the status bar, for the machine's
audio routing. Left: input device (listed, never opened until chosen),
channels (mono or stereo pair), a live level, monitoring with the engine's
own availability and latency text, then the recording workflow: choose a
lane (or make a new one), press Record, with the reason shown when a take
cannot start; the offset adds to the measured round-trip latency. Right:
the main output's channel pair and up to four extra outputs, each a master
or track source on any output device and channel pair with level, mute and
the stream's health, latency and drift. Extra outputs are drift-corrected
cue feeds and the sheet says they are not sample-locked. Input choices,
outputs, the MIDI channel and controller mappings are saved as machine
preferences, never in a project; mappings that name another project's
devices are kept and reported, not applied.

## Help

Usage hints are optional and off by default. With hints on they show as
caption text; with hints off an info mark stands in, carrying the text
as its tooltip and accessible description. Warnings (Python's file
access, missing devices, audio errors) are not hints and always show.
Empty states are one quiet line at the top of the pane, never a large
centered icon.

## Icons

Phosphor (MIT), regular weight for tools and navigation, fill weight for
transport. Icons replace labels where the meaning is standard (play, stop,
undo, tools); anything ambiguous keeps a word or a tooltip.

## Motion

Hover and toggle states ease over roughly 60 ms. Meters use real ballistics
(fast attack, ~20 dB/s release, 1.2 s peak hold). Nothing animates while
idle, so a stopped project draws no frames.
