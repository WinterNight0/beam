# Runs the same checks as `make check`, for Windows machines without make.
#   powershell -ExecutionPolicy Bypass -File scripts\check.ps1

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
    else { throw 'cargo was not found on PATH. Install Rust from https://rustup.rs/' }
}

Write-Host '==> cargo fmt --check' -ForegroundColor Cyan
cargo fmt --all --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host '==> cargo clippy' -ForegroundColor Cyan
cargo clippy --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host '==> cargo test' -ForegroundColor Cyan
cargo test
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host 'All checks passed.' -ForegroundColor Green
