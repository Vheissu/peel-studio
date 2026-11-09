# MCP

Peel Studio exposes its session through a local MCP server over stdio.
An agent can create music, edit the arrangement, mix, play, record, import
MIDI and audio, and export finished tracks. Agent edits appear in the
same undo history as mouse and keyboard edits.

## Connect

Build the desktop app and the display-free server:

```sh
cargo build --release -p peel-app -p peel-mcp
```

For a visible app with sound, configure your MCP client to launch
`peel-studio --mcp`. For example, in a client's JSON configuration:

```json
{
  "mcpServers": {
    "peel-studio": {
      "command": "/absolute/path/to/peel-studio/target/release/peel-studio",
      "args": ["--mcp"]
    }
  }
}
```

This starts a new desktop session. It does not attach to a window that is
already open. The agent and the person using that window share the same
project, playback and undo history. The welcome screen is skipped.

For Codex, the equivalent TOML entry is:

```toml
[mcp_servers.peel-studio]
command = "/absolute/path/to/peel-studio/target/release/peel-studio"
args = ["--mcp"]
```

For a display-free session, change the command to `peel-mcp` and remove
`--mcp` from the arguments. It starts with an empty project and a silent
engine; pass `--audio` to hear playback. Either executable accepts a
project path as its last argument. The desktop binary also accepts
`--mcp-headless` for silent operation without a window, including from a
packaged app on macOS:

```json
{
  "mcpServers": {
    "peel-studio": {
      "command": "/Applications/Peel Studio.app/Contents/MacOS/peel-studio",
      "args": ["--mcp-headless"]
    }
  }
}
```

MCP is enabled only when requested on the command line. It opens no network
port. Standard output carries protocol messages; diagnostics go to standard
error. The server uses the official Rust MCP SDK for negotiation, framing
and protocol errors. See the [SDK](https://github.com/modelcontextprotocol/rust-sdk)
and [stdio specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

## Tools

Tool schemas describe every argument and are generated from the same Rust
types that decode requests. Call `peel_help` for this guide, `peel_status`
for live state, and `peel_read_project` for the document and its revision.

| Work | Tools |
| --- | --- |
| Inspect | `peel_status`, `peel_read_project`, `peel_devices`, `peel_browse_files`, `peel_help` |
| Files | `peel_new_project`, `peel_open_project`, `peel_save_project` |
| Compose | `peel_add_channel`, `peel_add_pattern`, `peel_set_notes`, `peel_transform_notes`, `peel_place_pattern` |
| Edit | `peel_edit_project`, `peel_new_ids`, `peel_history` |
| Mix | `peel_add_effect`, `peel_set_device`; document edits for routing, sends, sidechains, automation, mute, solo and faders |
| Play | `peel_transport`, `peel_live_note`, `peel_preview_audio` |
| Import | `peel_import_audio`, `peel_import_midi` |
| Export | `peel_export_audio`, `peel_export_job`, `peel_export_midi`, `peel_bounce_clip` |
| Hardware | `peel_audio_devices`, `peel_configure_audio`, `peel_configure_midi`, `peel_rescan_plugins` |
| Routing | `peel_output_routes`, `peel_configure_output_routes`, `peel_configure_output_channels` |
| Controllers | `peel_midi_controls`, `peel_configure_midi_controls`, `peel_midi_learn`, `peel_midi_control` |
| Record | `peel_configure_recording`, `peel_arm_lane`, `peel_recording`, `peel_close_input` |
| Perform | `peel_performance_slots`, `peel_performance` |

`peel_devices` discovers the current native catalog and installed CLAP
plugins. Pass a `type_id` to read parameter keys, plain-value ranges,
choices and factory presets. Instrument additions are discovered without
changing the MCP server. CLAP descriptors become available when the plugin
has been loaded and the background scan finishes. Silent hosts skip automatic plugin scans; call `peel_rescan_plugins` to
scan on demand, or use the desktop or `peel-mcp --audio`.

## Compose

A typical sequence is:

1. Create a project with `peel_new_project` and a title.
2. Call `peel_devices`, then `peel_add_channel` with an instrument's
   `type_id`. Keep the returned channel, device and mixer track IDs.
3. Read `/patterns` and `/arrangements` with `peel_read_project` to find
   the initial pattern and arrangement lanes, or create another pattern.
4. Call `peel_set_notes` with the pattern and channel IDs. For a quarter
   note, use `{"start":"0","length":"1","key":60,"velocity":0.8}`.
5. Place the pattern on a lane with `peel_place_pattern`. Switch transport
   to `song` mode and play, or export the arrangement without playing it.
6. Save with `peel_save_project` and an absolute path ending in `.peelproj`.

Times are exact strings in quarter notes: `"4"` is one 4/4 bar, `"1/4"`
is a sixteenth note, and `"1/3"` is a triplet division. MIDI keys use
0..127; key 60 is middle C. Velocities use 0..1. Device values use the
plain units returned by `peel_devices`. Automation values are normalized
0..1 across the target's range. UUIDs identify objects; names can change.

## Edit

`peel_edit_project` exposes every musical document field. Use it for
advanced note properties, tempo ramps, automation curves, audio trims and
fades, new lanes and buses, routing, sends, markers, alternate arrangements,
or plugin state. It accepts RFC 6902 JSON Patch operations: `add`,
`remove`, `replace`, `move`, `copy` and `test`. Paths are JSON Pointers;
array `/-` appends an item. Pass the `project_id` and `revision` returned by `peel_read_project` as
`expected_project_id` and `expected_revision`. Read the relevant subtree to learn its shape,
and use `peel_new_ids` when constructing new objects.

For example, after reading the project's current revision:

```json
{
  "expected_project_id": "uuid-from-peel-read-project",
  "expected_revision": 12,
  "label": "Set tempo and pan",
  "operations": [
    {"op":"replace","path":"/tempo/0/bpm","value":110},
    {"op":"replace","path":"/channels/0/pan","value":-0.25}
  ]
}
```

The entire candidate is decoded and validated before it replaces the
current state. A failed patch, broken reference, routing cycle or invalid
value leaves the project and undo history unchanged. Successful batches
make one undo step. Project identity, format, schema and revision cannot
be changed by a patch. Empty edits do not create history entries.

If someone edits the project between your read and write, the revision
check rejects your patch. Read again and rebuild the edit using the new
state. Avoid blindly retrying array indices. New/open operations refuse
to discard unsaved changes unless `discard_changes` is explicitly true.
Stop recording before editing the document or replacing the project.

MCP times are bounded to 10 million beats, numerators to one trillion and
denominators to one million. Requests are queued in a bounded channel and
executed on the session's owning thread, outside the realtime audio path.

## Export

`peel_export_audio` snapshots the arrangement and returns a job ID.
Choose WAV, MP3, FLAC or AIFF through the output extension. Pass `stems`
for one file per active mixer track. Quality options match the app's export
settings. Poll `peel_export_job` until `done` is true; check `error` before
using `path`. Pass `cancel: true` to request cancellation. Completed
results remain readable for the life of the connection. One MCP export
runs at a time, with at most 256 retained jobs.

Imports and exports use absolute local filesystem paths and the process's
normal filesystem permissions. Recording tools may open the microphone.
The client controls which calls its agent may make. Disconnecting cancels
MCP export jobs and recording, closes audio input, stops playback and silences live notes;
files already written remain. Save before disconnecting. In desktop mode,
the window remains available for the user to continue working.

The tools operate on the musical session. They do not automate pane sizing,
file chooser dialogs or third-party plugin editor windows. Plugin parameters
and saved state are accessible through the document and device tools.

## Output routes

`peel_output_routes` reads primary channel mapping and additional output health.
`peel_configure_output_channels` maps primary playback to `left` or a `left`/`right`
pair. Hardware channel indices start at zero.

`peel_configure_output_routes` takes `routes`, an array of up to four destinations.
Each entry has `name`, `device` (ID or exact name from `peel_audio_devices`),
`left`, optional `right`, optional `track` (mixer track ID; omit for master),
`gain_db` (default 0) and `muted` (default false). An empty array closes the
additional outputs. Invalid routes or a failed device opening leave the current
configuration intact.

Additional outputs are buffered, drift-corrected monitor feeds. They can feed a
cue interface while the main output uses the computer speakers. Independent
hardware clocks are not phase-aligned. Use an OS aggregate device for
synchronous multichannel playback. The master route excludes metronome,
preview and direct input monitoring.

## Controllers

`peel_configure_midi_controls` replaces `mappings` and sets `channel_filter`
(0–15; omit for omni). A mapping has `controller` (0–127), optional `channel`,
`target`, `min` and `max`. Targets are:

```json
{"kind":"device_param","device":"DEVICE_ID","key":"cutoff"}
```

Other target kinds are `track_gain` / `track_pan` with `track`, and
`channel_gain` / `channel_pan` with `channel`. Device ranges are plain values
from `peel_devices`; logarithmic controls retain their parameter curve.
A mapping for an exact MIDI channel overrides the same CC's omni mapping
only on that channel. Removing the override restores the omni binding.
Duplicate exact controller/channel bindings are rejected. CC64, CC120 and
CC123 retain their sustain and panic roles.

`peel_midi_learn` with a target waits for the next accepted controller; omit
`target` to cancel. `peel_midi_controls` reads mappings, filter, Learn status and
stale-target errors. `peel_midi_control` sends `channel`, `controller` and `value`
through the same filter and mapping path as hardware, so scripts can operate
learned controls without a physical controller. These calls configure the live
host; mappings do not travel in project files.
