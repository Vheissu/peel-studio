# Roadmap

The specification describes the finished product. This file tracks what
exists and the order the rest is built in. A row is done only when its UI,
processing, persistence and tests all exist.

## Phase 1: foundation

| Area | Scope | Status |
| --- | --- | --- |
| Musical time | Rational positions, tempo map with ramps, meter | Done |
| Document | Model, ids, validation, checkpoint undo with gesture coalescing | Done |
| Device contract | `Device` trait, parameter descriptors, presets, sample handoff | Done |
| Engine | Callback, transport, sequencer, mixer graph, audio regions, telemetry, offline render, allocation checks | Done |
| Native instruments | Kick, snare, hat, clap, tom, Poly, Bass, Tine, Grand, Sampler, 60 presets | Done |
| Native effects | EQ, compressor, limiter, delay, reverb, drive, filter, chorus, utility, 50 presets | Done |
| Media | Decode WAV/AIFF/FLAC/MP3/OGG, resample, WAV export with dither, peaks | Done |
| Project files | `.peelproj` manifest, content-addressed shards, atomic save, history recovery, bundles, autosave, lock | Done |
| MIDI files | SMF import (types 0/1/2) and export with reports | Done |
| UI toolkit | SDF shape renderer, text, input, widgets, theme, popups, drag and drop | Done |
| Editors | Transport, Channel Rack, Piano Roll, Playlist, Mixer, Browser, Inspector | Done |
| Arrangement | Linked pattern clips, Make unique, loop range, audio clips with waveforms | Done |
| Harmony | Scales, chord stamping, quantize, clipboard | Done |
| Workflow | Welcome screen, recent projects, crash recovery, shortcuts overlay, export panel | Done |
| Packaging | macOS app bundle and DMG, release workflow for macOS, Windows and Linux | Done |
| Hardware MIDI input | Live playing with sustain, device hotplug, note recording | Done |
| Audio device selection | Choose output and buffer size | Done |
| Accessibility | AccessKit tree, keyboard focus traversal, named controls | Done |
| Automation | Clips for device parameters, track volume and pan | Done |
| Mixer routing | Bus outputs, post-fader sends, loop-free validation | Done |
| Audio recording | Input devices and channels, input meter, monitoring, armed lanes, durable takes placed with latency compensation | Engine and session done, UI in progress |
| CLAP plugins | Isolated scanning, hosting as native devices, saved state, offline export | Done |

## Phase 2: recording and deeper editing

Take lanes and comping, sample and waveform editor, time stretching,
tempo automation UI, automation curves (tension), dedicated Core Audio /
WASAPI / ASIO backends, engine process isolation.

## Phase 3: plugins

Plugin windows for CLAP, a scan cache and quarantine, VST3 hosting,
isolated plugin workers, Audio Units on macOS, plugin delay compensation
across the graph.

## Phase 4: the rest of the specification

Performance mode and clip launching, stem separation and analysis,
scripting and controller SDK, remote control, video, mastering assistant,
localization, signed installers and auto-update.
