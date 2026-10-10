#!/usr/bin/env bash
# Build the Loopz release binary.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --manifest-path src-tauri/Cargo.toml
echo "OK: src-tauri/target/release/loopz"
