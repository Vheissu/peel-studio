#!/usr/bin/env bash
# Everything CI checks, in the order it fails fastest.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
# Native Windows CI covers platform-gated code. Opt into local cross-lint
# with PEEL_CHECK_WINDOWS=1 after installing the Rust Windows target and a
# Windows C cross-toolchain (required by the bundled LAME encoder).
if [[ "${PEEL_CHECK_WINDOWS:-0}" == 1 ]] && rustup target list --installed 2>/dev/null | grep -q x86_64-pc-windows-msvc && [[ "$(uname)" != MINGW* ]]; then
  CARGO_FEATURE_PURE=1 CARGO_TARGET_DIR=target/win \
    cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings
fi
cargo test --workspace
