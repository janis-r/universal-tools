#!/bin/sh
# Compile the JSON Schema crate to the WebAssembly module shared by every host.
# Linux, macOS, and other Unix systems. Windows uses scripts/build-wasm.ps1.
set -eu
cd "$(dirname "$0")/.."

cargo build -p ut-json-schema --target wasm32-unknown-unknown --release
mkdir -p targets/wasm/json-schema
cp target/wasm32-unknown-unknown/release/ut_json_schema.wasm \
  targets/wasm/json-schema/ut_json_schema.wasm
