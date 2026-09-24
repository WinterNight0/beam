# Runs the same checks as `make check`, for Windows machines without make.
#   powershell -ExecutionPolicy Bypass -File scripts\check.ps1

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

if (-not (Get-Command go -ErrorAction SilentlyContinue)) {
    $goBin = 'C:\Program Files\Go\bin'
    if (Test-Path $goBin) { $env:Path = "$goBin;$env:Path" }
    else { throw 'go was not found on PATH. Install Go from https://go.dev/dl/' }
}

Write-Host '==> gofmt' -ForegroundColor Cyan
$unformatted = & gofmt -l .
if ($unformatted) {
    Write-Host 'These files need gofmt:' -ForegroundColor Red
    $unformatted | ForEach-Object { Write-Host "  $_" }
    exit 1
}

Write-Host '==> go vet' -ForegroundColor Cyan
go vet ./...
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host '==> go test' -ForegroundColor Cyan
go test ./...
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host 'All checks passed.' -ForegroundColor Green
