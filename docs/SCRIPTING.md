# Scripting

Open **Scripts** in the status bar or press **F8**. Choose an action and
press **Run**. Factory actions transpose notes, report project statistics,
and add a chord with PyTheory. **Add file** installs a `.js`, `.lua` or
`.py` file; **Refresh** reloads installed scripts after editing them in your
text editor.

Scripts are editor actions: they can generate notes, change patterns,
arrange clips, adjust routing and edit device parameters. They don't run
in the audio callback. This version doesn't support custom panels, live
callbacks, audio processing or background extensions.

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

Python is optional and runs in a separate process. Install Python 3.10 or
newer, then select a `.py` action. **Choose…** selects an interpreter,
including one inside a virtual environment; Peel remembers that choice.
**System** clears it. If no interpreter is selected, Peel uses
`PEEL_PYTHON`, then `python3` (`python` on Windows) from the launch environment.

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

[PyTheory](https://pytheory.org/) can be imported from a Python action.
Install it in a virtual environment, then choose that environment's Python
in the Scripts panel:

```sh
python3 -m venv ~/.peel-python
~/.peel-python/bin/python -m pip install pytheory
```

On Windows, use `py -3 -m venv .peel-python` and
`.peel-python\Scripts\python.exe -m pip install pytheory`.

Select a melodic instrument and a pattern, then run **PyTheory: C major
chord**. The example calls `TonedScale(tonic="C4")["major"].triad()`, converts
the tones to MIDI keys and adds three notes at beat zero. It uses Peel's
instrument for playback; PyTheory doesn't need to open an audio device.
The example was verified with PyTheory 0.57.13.

See the full action in
[`assets/scripts/pytheory_chord.py`](../assets/scripts/pytheory_chord.py).
Other Python packages can be used the same way. Peel doesn't install
packages automatically.

## Edits

Each run receives a snapshot of the project and selection. On successful
completion, Peel validates the result and commits changed data as **one undo
step** named `Script: <name>`. A read-only script adds no undo entry.
An exception, invalid edit, cancellation or timeout discards the result.
If the project or captured plugin state changed during execution, Peel rejects a stale edit;
run the action again against the new state.

Run is unavailable while recording. A result is also refused if recording
starts while the script runs. Closing the panel doesn't stop a run; use
**Cancel**. Its output stays in the panel until another result replaces it.

## Limits

JavaScript and Lua have a 64 MiB VM memory limit and a two-second execution
budget. The desktop and CLI run their VMs in child processes, so Cancel
and the deadline also stop native library calls. Lua results also have depth and node
limits before conversion to the document. Neither language has direct
filesystem, network, environment or process APIs.

Python has a ten-second deadline but no VM memory limit. Cancel or timeout
terminates the interpreter; it doesn't contain or clean up subprocesses a
trusted script may have started. Execution limits are intended to stop
mistakes, not to isolate hostile native code or plugins.

Source files are limited to 1 MiB; input and output project JSON to 16 MiB.
Output is capped at 16 KiB and 256 log messages. Existing document and
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
isn't overwritten unless you supply that same path.
