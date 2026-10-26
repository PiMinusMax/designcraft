$ErrorActionPreference = 'Stop'
$env:CRAFT_FONTS_DIR = (Resolve-Path '..\wt-craft-fonts-main').Path
$env:RUSTFLAGS = '-C target-feature=+crt-static'
$env:DESIGNCRAFT_VERSION = (Get-Content 'win-version.txt').Trim()
cargo build --release -p designcraft -p designcraft-cli
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$out = Join-Path $env:CARGO_TARGET_DIR 'release'
New-Item -ItemType Directory -Force -Path dist\win | Out-Null
Copy-Item (Join-Path $out 'designcraft.exe'), (Join-Path $out 'designcraft-cli.exe') dist\win\ -Force
Get-ChildItem dist\win | Format-Table Name, Length
