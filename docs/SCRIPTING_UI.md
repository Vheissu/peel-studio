# Workspace calls

Desktop scripts can read and change the current editor, selection and visible
panels through `peel.call`. These calls run on the UI thread and return the
updated workspace state. They don't create musical undo steps.

```js
const view = peel.call("ui.get", {});
peel.call("ui.set", {
    dock: "mixer",
    panels: { browser: false, inspector: true, editor: true, help: false }
});
```

Lua uses tables for arguments; Python uses dictionaries:

```lua
peel.call("ui.set", {dock = "piano_roll", panels = {editor = true}})
```

```python
peel.call("ui.set", {"dock": "rack", "panels": {"browser": True}})
```

`ui.set` accepts these fields. Omitted fields keep their current values.

| Field | Values |
| --- | --- |
| `dock` | `rack`, `piano_roll`, `mixer`, `perform`; selecting one reveals the editor |
| `browser_tab` | `instruments`, `effects`, `samples`, `patterns`; selecting one reveals the browser |
| `pattern` | ID of an existing pattern |
| `channel` | ID of an existing instrument channel |
| `note_lane` | `velocity`, `pitch`, `pressure` or `timbre` in the Piano Roll |
| `expression_note` | ID of the note to focus; it must belong to the selected pattern and channel |
| `notes` | IDs of notes in the selected pattern; an empty array clears the selection |
| `clips` | IDs of clips in the active arrangement; an empty array clears the selection |
| `focus` | Object with `kind` (`channel`, `track`, `clip`) and `id`, or `{"kind":"none"}` |
| `panels` | Optional booleans: `browser`, `inspector`, `scripts`, `editor`, `help`, `meter`, `status_details` |
| `mixer_size` | `compact`, `expanded` or `full` strip density |
| `mixer_expanded` | Grow the mixer into the available dock area, or restore its earlier height |
| `instrument` | `{"kind":"open","channel":"CHANNEL_ID"}` opens an instrument editor; `{"kind":"close"}` closes it |
| `keyboard` | Enable or disable computer-keyboard note input |
| `audio_io` | Show or hide Audio routing; opening the panel does not open hardware |
| `effect` | `{"kind":"open","device":"INSERT_ID"}` opens an insert editor; `{"kind":"close"}` closes it |
| `sizes` | Optional dimensions in logical pixels: `browser` (180–420), `inspector` (240–480), `editor` height (180–1200, limited by the current window) |

Inspector and Scripts share the right column. Showing one hides the other;
setting both false hides the column. Visibility, sizes and active tabs are saved in user preferences. Pattern,
channel, note and clip selection and focus remain session state. Unknown targets or
invalid options reject the complete `ui.set` request without changing it.

`ui.get` returns those fields and the current focus. `peel.call("tools", {})`
returns the host action catalog, including the desktop workspace calls.
The command-line host has session controls but no desktop workspace.

For transport, device discovery, recording, import/export and project actions,
use the live calls described in [SCRIPTING.md](SCRIPTING.md) and [MCP.md](MCP.md).

`ui.get` also reports `mixer_size`, `mixer_expanded` and `effect` (the open
editor's `track` and `device`, or null). These editor choices do not edit
the music document or add undo steps. An unknown effect rejects the
complete request before any other view setting changes.

`ui.get` reports `instrument` (the channel ID or null), `audio_io` and
`keyboard` too.
Opening an instrument sets the live keyboard target immediately. Effect,
instrument and Audio routing editors share the workspace; one request can
open at most one. All targets are checked before any view state changes.

## Recording

`peel.call("ui.record", {})` reads the desktop recorder's `armed`,
`recording` and `pattern` state. Set `{armed: true}` in Pattern mode to
capture notes and expression from the shared input path; `{armed: false}`
closes held notes and ends the take. Start and stop playback with the
ordinary `transport` call. This desktop call is available to an already
running script; new scripts cannot start while recording is active.

Use `mpe`, `midi_input` and `note_expression` for input configuration,
internal MIDI and saved curves. These host calls work from JavaScript,
Lua and the bundled Python runtime.
