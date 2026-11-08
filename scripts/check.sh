#!/usr/bin/env bash
# Everything CI checks, in the order it fails fastest.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
# Platform-gated code is linted for Windows too when the target is
# installed (rustup target add x86_64-pc-windows-msvc). BLAKE3 builds
# without its C and assembly parts so no Windows toolchain is needed.
if rustup target list --installed 2>/dev/null | grep -q x86_64-pc-windows-msvc && [[ "$(uname)" != MINGW* ]]; then
  CARGO_FEATURE_PURE=1 CARGO_TARGET_DIR=target/win \
    cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings
fi
cargo test --workspace
