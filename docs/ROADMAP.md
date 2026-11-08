# Roadmap

The specification describes the finished product. This file tracks what
exists and the order the rest is built in. A row is done only when its UI,
processing, persistence and tests all exist.

## Phase 1: foundation (in progress)

| Area | Scope | Status |
| --- | --- | --- |
| Musical time | Rational positions, tempo map with ramps, meter | Done |
| Document | Model, ids, validation, checkpoint undo with gesture coalescing | Done |
| Device contract | `Device` trait, parameter descriptors, sample handoff | Done |
| Engine | Callback, transport, sequencer, mixer graph, telemetry, offline render | In progress |
| Native instruments | Kick, snare, hat, clap, tom, poly synth, bass, FM keys, piano, sampler | In progress |
| Native effects | EQ, compressor, limiter, delay, reverb, drive, filter, chorus, utility | In progress |
| Media | Decode WAV/AIFF/FLAC/MP3/OGG, resample, WAV export, peaks | In progress |
| Project files | `.peelproj` manifest, content-addressed shards, atomic save | In progress |
| MIDI files | SMF type 0/1 import and export | In progress |
| UI toolkit | SDF shape renderer, text, input, widgets, theme | In progress |
| Editors | Transport, Channel Rack, Piano Roll, Playlist, Mixer, Browser, device panels | In progress |

## Phase 2: recording and audio editing

Audio clips in the Playlist, audio input and recording with take lanes,
waveform editor, time stretch provider, automation clips and lanes,
dedicated Core Audio / WASAPI / ASIO backends, engine process isolation.

## Phase 3: plugins

CLAP and VST3 hosting with isolated plugin workers and scanner, Audio
Units on macOS, plugin delay compensation across the graph.

## Phase 4: the rest of the specification

Performance mode and clip launching, stem separation and analysis,
scripting and controller SDK, remote control, video, mastering assistant,
accessibility tree (AccessKit) completion, localization, installers and
signing.
