# Builds beam's setup wizard: target\installer\beam-setup-<version>.exe.
#
#   powershell -ExecutionPolicy Bypass -File scripts\windows\build-installer.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\build-installer.ps1 -Iscc "D:\Tools\Inno Setup 6\ISCC.exe"
#
# What it does:
#   1. cargo build --release -p beam -p beam-gui;
#   2. writes target\licenses\THIRD-PARTY.html, the licences of the crates
#      built into them, with cargo-about (packaging\licenses, ADR-0058);
#   3. reads the version from the workspace Cargo.toml;
#   4. compiles packaging\windows\beam.iss with Inno Setup 6 (ADR-0056).
# Needs Inno Setup 6 (winget install JRSoftware.InnoSetup) and cargo-about
# (cargo install cargo-about --locked --features cli). Inno Setup is found
# through -Iscc, then $env:ISCC, then the usual install folders, then PATH.

param(
    [string]$Iscc = $env:ISCC
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

# Inno Setup's compiler.
if (-not $Iscc) {
    $candidates = @(
        'C:\Inno Setup 6\ISCC.exe',
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
    )
    $Iscc = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if (-not $Iscc) {
        $Iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
    }
}
if (-not $Iscc -or -not (Test-Path $Iscc)) {
    throw 'Inno Setup 6 was not found. Install it (winget install JRSoftware.InnoSetup), or pass -Iscc <path to ISCC.exe>.'
}

# 1. Build.
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
    else { throw 'cargo was not found. Install Rust from https://rustup.rs/' }
}
Write-Host '==> cargo build --release -p beam -p beam-gui' -ForegroundColor Cyan
Push-Location $repo
try {
    cargo build --release --locked -p beam -p beam-gui
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    # 2. Third-party licences.
    & cargo about --version *> $null
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo-about was not found. Install it: cargo install cargo-about --locked --features cli'
    }
    Write-Host '==> cargo about generate: target\licenses\THIRD-PARTY.html' -ForegroundColor Cyan
    New-Item -ItemType Directory -Force (Join-Path $repo 'target\licenses') | Out-Null
    cargo about generate --locked --fail --workspace -c packaging\licenses\about.toml `
        -o target\licenses\THIRD-PARTY.html packaging\licenses\about.hbs
    if ($LASTEXITCODE -ne 0) { throw 'cargo about generate failed: see above (a licence not in packaging\licenses\about.toml?).' }
} finally { Pop-Location }

# 3. The version, from [workspace.package] in Cargo.toml.
$manifest = Get-Content (Join-Path $repo 'Cargo.toml') -Raw
$section = [regex]::Match($manifest, '(?ms)^\[workspace\.package\](.*?)^\[')
$version = [regex]::Match($section.Groups[1].Value, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { throw 'No version under [workspace.package] in Cargo.toml.' }

# 4. The wizard.
Write-Host "==> Inno Setup: beam $version" -ForegroundColor Cyan
& $Iscc /Q "/DAppVersion=$version" "/DRepo=$repo" (Join-Path $repo 'packaging\windows\beam.iss')
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$setup = Join-Path $repo "target\installer\beam-setup-$version.exe"
Write-Host "Built $setup" -ForegroundColor Green
