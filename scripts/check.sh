#!/usr/bin/env bash
# Everything CI checks, in the order it fails fastest.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
