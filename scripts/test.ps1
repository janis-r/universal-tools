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

Invoke-Expression "$cargo test --workspace"
Push-Location "targets\js\json-schema"
npm test
Pop-Location
python -m unittest discover -s "targets\python\json-schema" -v
Push-Location "targets\go\json-schema"
go test ./...
Pop-Location
