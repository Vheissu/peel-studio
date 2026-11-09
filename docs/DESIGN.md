# Design

## Reference

An analog mixing console at night. Graphite panels, a colored cap on every
channel, amber VU backlight, a coral tally light when recording, and crisp
white legends. Peel Studio borrows the console's logic rather than its
skeuomorphism: color identifies a channel everywhere it appears, and the
interface stays dark and quiet so notes, waveforms and the playhead are the
brightest things on screen.

## Color

| Token | Value | Use |
| --- | --- | --- |
| `bg` | `#14171D` | Window background and gutters between panes |
| `panel` | `#1C2129` | Panes (browser, playlist, dock, inspector) |
| `panel_alt` | `#20262F` | Editor canvases inside a pane |
| `raised` | `#252D38` | Selected rows, active tabs, step cells |
| `control` | `#2B3441` | Buttons and knob bodies |
| `well` | `#171B22` | Recessed tray behind a group of quiet controls |
| `text` | `#F1F4F8` | Primary text |
| `text_dim` | `#ACB7C7` | Secondary text |
| `text_faint` | `#7D8899` | Labels and hints |
| `accent` | `#F2B66D` | Primary action, playhead, tempo (VU backlight amber) |
| `play` | `#86D49E` | Play state, signal present, meters below -6 dB |
| `record` | `#F07880` | Recording and clipping |
| `focus` | `#60D6E8` | Keyboard focus and selection outlines only, never text |

Channel colors come from a 12-entry palette modeled on console knob caps
and patch cables (`peel_document::PALETTE`). A channel's color carries into
its rack button, piano roll notes, mixer cap and playlist clips.

Hierarchy comes from value steps between surfaces. There are no decorative
gradients, glows or borders; shadows appear only under things that float
(menus, tooltips, drag previews, fader caps).

## Type

Barlow Semi Condensed (SIL OFL), a low-contrast grotesk drawn from
California highway signage. Signage type is built to be read at a glance,
which is what a musician needs from across a desk, and the semi-condensed
width fits names into narrow mixer strips. One family carries every role
through size and weight. Readouts (position, tempo, levels) use its
tabular figures so numbers do not jitter while they change.

| Role | Size | Weight |
| --- | --- | --- |
| Caption | 11.5 | Medium |
| Body | 13 | Medium |
| Label | 13 | SemiBold |
| Heading | 15 | SemiBold |
| Readout | 18 to 20 | SemiBold, tabular |

## Shape

4 px base unit; groups of 8, 12, 16 and 24. Corner radius 4 px for small
controls, 6 px for buttons and rows, 8 px for panes. Panes sit on the
background with a 6 px gutter. Nothing is nested more than one surface
deep. Pane headers are 38 px, rails 40 px wide, the top bar 44 px and the
status bar 24 px.

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
- The dock collapses to its 38 px tab header, so every editor stays one
  click away. Its tabs keep their labels while the editor's tools fit
  beside them, otherwise all four become icons together.
- The pattern picker sits in the dock header beside the Channels and
  Piano roll editors, the work it targets.
- Side panes cap themselves at about a quarter (browser) and a third
  (right column) of narrow windows without losing the user's size.
- The top bar measures itself and sheds, in order: the load figure,
  rewind and fast-forward, the meter, the Export label, the clock, the
  project title and finally the View label.

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
Property rows put a 72 px caption label left of the value. Knob cells
are at least 76 px and stretch to fill the row, so a default inspector
shows three columns with whole parameter names; a name that still does
not fit is cut and shown in full in the knob's tooltip. When a toolbar runs
short, names go before controls: the pattern picker shrinks to its swatch
and the piano roll's channel to its color, both keeping the name in their
tooltip and accessible label. LEDs are flat
dots without a halo.

## Mixer

Strips scroll sideways with the master pinned on the right. Three sizes
in the dock header (Compact 64 px, Expanded 92, Full 124) decide which
rows a strip has; the dock height decides how many insert and send rows
fit, so the fader keeps at least about 96 px. When it cannot, rows go in a
fixed order: the send list becomes a Sends menu, then the pan legend, then
an insert row. Grow mixer gives the dock the whole window height.

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
