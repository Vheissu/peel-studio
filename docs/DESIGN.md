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
deep.

## Icons

Phosphor (MIT), regular weight for tools and navigation, fill weight for
transport. Icons replace labels where the meaning is standard (play, stop,
undo, tools); anything ambiguous keeps a word or a tooltip.

## Motion

Hover and toggle states ease over roughly 60 ms. Meters use real ballistics
(fast attack, ~20 dB/s release, 1.2 s peak hold). Nothing animates while
idle, so a stopped project draws no frames.
