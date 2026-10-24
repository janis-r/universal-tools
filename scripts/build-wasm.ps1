$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$mingw = Join-Path $env:USERPROFILE ".local\llvm-mingw\llvm-mingw-20261020-ucrt-aarch64\bin"
if (Test-Path $mingw) {
  $env:PATH = "$mingw;$env:PATH"
}

$cargo = "cargo"
if (Get-Command rustup -ErrorAction SilentlyContinue) {
  $installed = rustup toolchain list
  if ($installed -match "stable-aarch64-pc-windows-gnullvm") {
    $cargo = "cargo +stable-aarch64-pc-windows-gnullvm"
  }
}

Invoke-Expression "$cargo build -p ut-json-schema --target wasm32-unknown-unknown --release"
New-Item -ItemType Directory -Force -Path "targets\wasm\json-schema" | Out-Null
Copy-Item `
  "target\wasm32-unknown-unknown\release\ut_json_schema.wasm" `
  "targets\wasm\json-schema\ut_json_schema.wasm" `
  -Force
