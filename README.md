# Peel Studio

An open source, pattern-based music workstation for Windows, macOS and
Linux. Build a beat or a melody as a pattern, paint it into an arrangement,
change the source once and every instance follows.

Peel Studio is native Rust end to end: a `wgpu` interface (Metal, D3D12,
Vulkan), a lock-free realtime engine and project-owned instruments and
effects. There is no web view.

> Early development. The foundation (document model, engine, native devices,
> UI toolkit and the core editors) is being built now. See
> [docs/ROADMAP.md](docs/ROADMAP.md).

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
