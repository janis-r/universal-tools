#!/bin/sh
# Run the Rust tests, then each language host against the last WebAssembly build.
# Linux, macOS, and other Unix systems. Windows uses scripts/test.ps1.
set -eu
cd "$(dirname "$0")/.."

cargo test --workspace
(cd targets/js/json-schema && npm test)
python -m unittest discover -s targets/python/json-schema -v
(cd targets/go/json-schema && go test ./...)
