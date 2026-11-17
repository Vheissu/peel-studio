# Contributing

Bug reports, fixes and new devices are welcome. For anything larger than a
fix, open an issue first so we can agree on the approach before you spend
time on it. [docs/ROADMAP.md](docs/ROADMAP.md) lists what's planned.

## Setup

Install Rust with `rustup`. The toolchain is pinned in
`rust-toolchain.toml` and installs itself on first build. You also need a
C compiler for the bundled LAME encoder, and on Linux the ALSA headers
(`libasound2-dev`) and a Vulkan driver.

```sh
cargo run --release
```

## Checks

Run the same checks CI runs:

```sh
scripts/check.sh
```

That's `cargo fmt --check`, `cargo clippy -D warnings` and the full test
suite. CI runs it on macOS, Windows and Linux.

## Ground rules

- The audio thread must not allocate, lock or block.
  [docs/REALTIME.md](docs/REALTIME.md) has the rules and the tests that
  enforce them. For engine or device changes, also run
  `cargo test -p peel-engine --features rt-check`. A change that breaks
  them won't be merged.
- Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) to see which crate
  owns what. Keep UI code out of the engine and engine code out of the UI.
- New bundled assets (samples, fonts, icons) need a redistributable
  license and a row in [assets/LICENSES.md](assets/LICENSES.md) in the
  same commit.
- Don't add code, presets, samples or artwork taken from commercial
  products.
- Keep commits focused, and write messages that say what changed.

## License

By contributing you agree that your work is licensed under
GPL-3.0-or-later, the same as the rest of the project.
