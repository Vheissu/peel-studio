# Peel Studio

An open source, pattern-based music workstation for Windows, macOS and
Linux. Build a beat or a melody as a pattern, paint it into an arrangement,
change the source once and every instance follows.

Peel Studio is native Rust end to end: a `wgpu` interface (Metal, D3D12,
Vulkan), a lock-free realtime engine and project-owned instruments and
effects. There is no web view.

![The Peel Studio workspace: browser, playlist, channel rack and device panel](docs/images/studio.png)

> Early development. The foundation (document model, realtime engine,
> native instruments and effects, UI toolkit and the core editors) works
> today; see [docs/ROADMAP.md](docs/ROADMAP.md) for what is next.

## What works today

- Pattern workflow: a Channel Rack step sequencer (with a velocity graph)
  and a Piano Roll edit the same notes; patterns are painted into the
  Playlist as linked clips, and Make unique splits one off.
- Ten native instruments (five synthesized drums, a polyphonic synth, a
  mono bass, an FM electric piano, a modelled piano and a sampler) and nine
  effects (EQ with an interactive curve, compressor, limiter, delay, reverb,
  drive, filter, chorus, utility), with over a hundred factory presets in
  the Browser. Installed CLAP plugins show up next to them.
- Arrangement: pattern, audio and automation clips; trims, fades, marquee
  selection, Alt-drag copies, loop range, lane management.
- Mixing: console-style strips with inserts, bus routing, post-fader sends,
  pan, faders, meters, mute and solo; automation of any device parameter,
  track volume or pan.
- Playing and recording: computer keyboard and hardware MIDI keyboards with
  sustain, note recording into patterns, scales and chord stamping,
  quantize, swing, tap tempo.
- Exact rational musical time, sample-accurate playback that cannot drift,
  and a realtime audio path verified to make no allocations.
- Open project files with atomic saves, autosave and crash recovery,
  Standard MIDI File import and export, WAV export, undo history.
- Screen reader support through AccessKit and keyboard focus traversal.

| Piano Roll with a scale | Mixer and EQ |
| --- | --- |
| ![Piano Roll](docs/images/piano-roll.png) | ![Mixer](docs/images/mixer.png) |

## Build

You need Rust (the exact toolchain is pinned in `rust-toolchain.toml`, and
`rustup` installs it automatically).

```sh
cargo run --release            # the desktop app
cargo run -p peel-cli -- help  # headless tools
cargo test --workspace
```

Linux also needs ALSA development headers (`libasound2-dev` on Debian and
Ubuntu) and a Vulkan driver.

To build a macOS app bundle and disk image in `dist/`:

```sh
scripts/package-macos.sh
```

Set `SIGN_ID` to a Developer ID identity to sign for distribution;
without it the bundle is ad-hoc signed for local use.

## Headless tools

```sh
peel render song.peelproj mix.wav --rate 48000 --bits 24
peel render --demo demo.wav       # the starter project
peel info song.peelproj
peel validate song.peelproj
peel devices                      # every native instrument and effect
```

## Layout

| Path | Contents |
| --- | --- |
| `crates/` | The Cargo workspace. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains each crate |
| `assets/` | Fonts and icons, each with its license |
| `docs/` | Architecture, realtime rules, roadmap |
| `open-source-fl-studio-alternative-spec.md` | The full product specification |

## License

GPL-3.0-or-later. Bundled fonts and icons keep their own licenses; see
[assets/LICENSES.md](assets/LICENSES.md).

Peel Studio is an independent project. It is not affiliated with
Image-Line, and it does not use FL Studio code, artwork, presets or samples.
