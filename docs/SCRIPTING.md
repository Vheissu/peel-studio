# Scripting

Open **Scripts** from **View**, use the right rail, or press **F8**. Choose an action and
press **Run**. Factory actions transpose notes, report project statistics,
and add a chord with PyTheory. **Add file** installs a `.js`, `.lua` or
`.py` file; **Refresh** reloads installed scripts after editing them in your
text editor.

Scripts are editor actions: they can generate notes, change patterns,
arrange clips, adjust routing and edit device parameters. They can also
call the live session, for example to start playback or export a mix, with
`peel.call`. They don't run in the audio callback. This version doesn't
support custom panels, audio processing, event callbacks or background
extensions.

## Files

Installed scripts live in the `Scripts` folder under Peel's config folder:

| Platform | Folder |
| --- | --- |
| macOS | `~/Library/Application Support/Peel Studio/Scripts` |
| Windows | `%APPDATA%\Peel Studio\Scripts` |
| Linux | `$XDG_CONFIG_HOME/Peel Studio/Scripts`, usually `~/.config/Peel Studio/Scripts` |

The filename becomes the action name. Add file refuses to replace an
existing script. Edit that file in place and refresh to load changes.
Opening a project never runs a script. Scripts and the Python interpreter
setting belong to the user, not the project file.

## API

All three languages receive a `peel` object with the same data:

| Member | Meaning |
| --- | --- |
| `apiVersion` | `1` |
| `project` | An editable copy of the complete project document |
| `context.pattern` | Current pattern ID, or null |
| `context.channel` | Selected instrument channel ID, or null |
| `context.notes` | Selected Piano Roll note IDs |
| `context.clips` | Selected Playlist clip IDs |
| `newId()` | A fresh ID for a new document object |
| `log(text)` | Captured output shown in the Scripts panel |
| `call(name, args)` | Run a live session action and return its result; see [Live calls](#live-calls) |

JavaScript uses objects and arrays; Lua uses tables; Python uses dictionaries
and lists (`peel.project["patterns"]`, `peel.context["channel"]`). Lua arrays
start at 1. IDs are 32 hexadecimal digits without hyphens. Use `newId()`
for each new note, channel, clip or other object.

Times are exact **strings in quarter notes**: `"4"` is one bar in 4/4,
`"1/2"` is an eighth note and `"1/3"` is an eighth-note triplet.
Keep these values as strings. Python's `fractions.Fraction` can do exact
time arithmetic; convert the result with `str()` when writing it back.

`project.seed` and `project.revision` are decimal strings so JavaScript and
Lua preserve all 64 bits. Leave revision unchanged. Extra project data with
other integers outside ±9,007,199,254,740,991 currently prevents a script
from running, rather than rounding that data.

The document shape is defined in
[`peel-document/src/model.rs`](../crates/peel-document/src/model.rs).
[`MCP.md`](MCP.md) also describes project fields, device discovery and
musical time. Scripts can change any musical fields in that document;
project identity, format, schema, revision and the audio asset list must
stay unchanged. Existing audio assets can be reused.

## Live calls

`peel.call(name, args)` runs one action in the live session and waits for
its result. The names and arguments are the MCP tools in [`MCP.md`](MCP.md)
without the `peel_` prefix: `peel_transport` becomes `"transport"`. Arguments
are an object (a table in Lua, a dictionary in Python) and may be omitted.

```js
peel.call("transport", {command: "play", looping: true});
const status = peel.call("status");
peel.log(`Playing at beat ${status.beat}`);
```

```lua
local added = peel.call("add_pattern", {name = "Fill", length = "4"})
peel.call("transport", {command = "stop"})
```

```python
job = peel.call("export_audio", {"path": "/Users/me/mix.wav"})["job"]
while not peel.call("export_job", {"job": job})["done"]:
    pass
```

Call `peel.call("help")` for the full action guide, or `"devices"` for the
instrument and effect catalog. The actions cover project status and reading,
editing and history, files, MIDI and audio import and export, device and
effect setup, transport, audio and MIDI device discovery and configuration,
recording, and performance clips. The desktop also supports
[`ui.get`, `ui.set` and `tools`](SCRIPTING_UI.md) for workspace selection,
panel visibility and action discovery. Hosts may add their own action names; an
unknown name is an error. Live calls only reach these actions: there is no
access to the audio callback, DSP or other application internals.

A failed action raises a normal script error with the action's message:
`Error` in JavaScript, a `pcall`-able error in Lua and `peel.HostError` in
Python. Catch it to continue. Results are plain JSON values. JavaScript numbers cannot preserve integers
above 2^53 in live-call results; use `peel.project` for the lossless project
snapshot and patch only the fields you intend to change. In Lua, JSON
null is `peel.null` and empty arrays keep their array identity; `nil`,
omitted arguments and `peel.null` all mean no arguments. Use `peel.null`
inside an argument table for an explicit null.

Calls run on the host's main thread, between frames. Each call is
synchronous, so a script sees playback and edits it made through earlier
calls. The UI and audio keep running while a script waits.

### Live calls and the snapshot

`peel.project` is still the snapshot taken when the script started. Live
calls don't update it; read fresh state with
`peel.call("read_project", {pointer: "/patterns"})`.

Live calls take effect immediately and keep their own behaviour. An edit
through `edit_project`, `add_pattern` or a similar action is its own undo
step. Saving, exporting, playback, recording and device changes happen when
called. **They aren't rolled back** if the script later fails, times out or
is cancelled; undo reverses document edits one action at a time.

A script that only uses live calls, or leaves its snapshot unchanged,
succeeds even if its calls changed or replaced the project. A script that
both makes live edits and changes `peel.project` fails at the end: the
snapshot is stale, and Peel won't overwrite the live edits with it. Use one
approach per run, or make every edit through `edit_project`.

## JavaScript

JavaScript runs in embedded QuickJS. No Node.js or browser APIs are
provided. Scripts are synchronous, single files; imports, timers and async
actions aren't supported.

```js
// Halve the velocity of selected notes.
const ids = new Set(peel.context.notes);
for (const pattern of peel.project.patterns) {
    if (pattern.id !== peel.context.pattern) continue;
    for (const note of pattern.notes) {
        if (ids.has(note.id)) note.velocity *= 0.5;
    }
}
peel.log(`Changed ${ids.size} selected notes.`);
```

## Lua

Lua 5.4 is bundled. The base, table, string, math and UTF-8 libraries are
available. File, OS, package, debug and coroutine libraries aren't exposed;
file and code loaders are removed.

```lua
for _, channel in ipairs(peel.project.channels) do
    if channel.id == peel.context.channel then
        channel.pan = 0
        peel.log("Centered " .. channel.name)
    end
end
```

Existing JSON arrays retain their array identity, including empty arrays.
Use `peel.array()` to create an empty array and `peel.null` to write an
explicit JSON null. Use `{}` for a new object.

## Python

Packaged Peel Studio releases include a private Python 3.12 runtime,
PyTheory and its dependencies. Python actions run locally in a separate
process; no Python installation, package installation or first-run download
is needed. Select **PyTheory: C major chord** and press **Run**.

**Choose…** selects a custom interpreter for scripts that need other packages.
Peel remembers that choice. **Default** restores automatic selection: a custom
`PEEL_PYTHON` environment setting takes precedence, followed by the bundled
runtime. Development builds without a bundle fall back to `python3`
(`python` on Windows). Create a developer bundle with the command in
[packaging/python/README.md](../packaging/python/README.md).

Python scripts have normal local access to files, the network and processes.
Only run Python files you trust. Undo restores project edits; it cannot
reverse a Python script's filesystem or other external effects. Python runs
in isolated mode (`-I`), so it uses the selected interpreter's packages and
ignores `PYTHONPATH` and user site packages. It isn't an OS sandbox.

```python
from fractions import Fraction

for pattern in peel.project["patterns"]:
    if pattern["id"] == peel.context["pattern"]:
        for note in pattern["notes"]:
            note["length"] = str(Fraction(note["length"]) / 2)
peel.log("Shortened notes in the current pattern.")
```

### PyTheory

[PyTheory](https://pytheory.org/) is ready to import in the bundled runtime:

```python
from pytheory import TonedScale
```

Select a melodic instrument and a pattern, then run **PyTheory: C major
chord**. The example calls `TonedScale(tonic="C4")["major"].triad()`, converts
the tones to MIDI keys and adds three notes at beat zero. It uses Peel's
instrument for playback; PyTheory doesn't need to open an audio device.
The example was verified with PyTheory 0.57.13.

See the full action in
[`assets/scripts/pytheory_chord.py`](../assets/scripts/pytheory_chord.py).
For extra Python packages, choose your own virtual environment. The shipped
runtime stays unchanged so moving the app and verifying its signature keep
working. Optional PyTheory extras (live MIDI, Link and diagrams) aren't bundled.
PyTheory's separate audio-playback functions may need platform libraries,
such as PortAudio on Linux; the factory action plays notes through Peel.

## Edits

Each run receives a snapshot of the project and selection. On successful
completion, Peel validates the result and commits changed data as **one undo
step** named `Script: <name>`. A read-only script adds no undo entry.
An exception, invalid edit, cancellation or timeout discards the snapshot
result. Live calls already made stay in effect. If the project or captured
plugin state changed during execution, including through the script's own
live calls, Peel rejects a stale edit; run the action again against the new
state.

Run is unavailable while recording. A result is also refused if recording
starts while the script runs. Closing the panel doesn't stop a run; use
**Cancel**. Its output stays in the panel until another result replaces it.

## Limits

JavaScript and Lua have a 64 MiB VM memory limit and a two-second execution
budget. Time spent waiting for live calls doesn't count against the budget,
but every run, including Python, stops after 60 seconds in total. The desktop and CLI run their VMs in child processes, so Cancel
and the deadline also stop native library calls. Lua results also have depth and node
limits before conversion to the document. Neither language has direct
filesystem, network, environment or process APIs.

Python has a ten-second deadline but no VM memory limit. Cancel or timeout
terminates the interpreter; it doesn't contain or clean up subprocesses a
trusted script may have started. Execution limits are intended to stop
mistakes, not to isolate hostile native code or plugins.

Source files are limited to 1 MiB; input and output project JSON to 16 MiB.
Output is capped at 16 KiB and 256 log messages. Live call arguments and
results are limited to 16 MiB each, and the host runs a few calls per frame. Existing document and
parameter validation applies, including MIDI key/velocity ranges, routing
references and bounded rational time values.

## Command line

The same runner is available without the desktop UI:

```sh
cargo run -p peel-cli -- script song.peelproj action.js edited.peelproj
cargo run -p peel-cli -- script --demo assets/scripts/summary.lua demo-copy.peelproj
PEEL_PYTHON=/path/to/venv/bin/python cargo run -p peel-cli -- script song.peelproj action.py edited.peelproj
```

CLI context uses the first pattern and first instrument channel, with no
selected notes or clips. The output path is explicit; the source project
isn't overwritten unless you supply that same path, or the script calls
`save_project` without a path.

The CLI answers live calls from a silent session: playback and
`configure_audio` never open an audio output, but `configure_recording` can
still open an input such as a microphone. After the script
finishes, the CLI waits for exports it started, saves the session (including
live edits) to the output path, then stops playback and closes any input. A
failed script saves nothing, but its earlier live calls, such as exports or
`save_project`, have already happened.
