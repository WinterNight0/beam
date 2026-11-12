# Builds beam's Windows downloads for a release, into target\dist\:
#
#   beam-setup-<version>.exe               the setup wizard (build-installer.ps1)
#   beam-<version>-windows-x86_64.zip      the two programs, to run without installing
#
#   powershell -ExecutionPolicy Bypass -File scripts\windows\package.ps1
#
# The zip holds beam.exe, beam-gui.exe, LICENSE.md, THIRD-PARTY.html (the
# crates built into them) and the licences of the bundled fonts and icons
# (licenses\). Needs what build-installer.ps1 needs: Rust, Inno Setup 6 and
# cargo-about. Arch Linux has no download: the AUR builds
# beam from the release's source (packaging/arch/PKGBUILD, ADR-0058).

param(
    [string]$Iscc = $env:ISCC
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

# Rust writes source paths into the programs (for panic messages). Release
# builds replace the folders of this machine with neutral names, so the
# downloads do not carry the builder's user name or folders. The last
# matching prefix wins, so the widest comes first.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$private = @($env:USERPROFILE, $cargoHome, $repo)
$remap = @(
    "--remap-path-prefix=$($env:USERPROFILE)=~",
    "--remap-path-prefix=$cargoHome=cargo",
    "--remap-path-prefix=$repo=beam"
)
$savedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$env:CARGO_ENCODED_RUSTFLAGS = $remap -join [char]0x1f

try {
    # 1. The wizard; this also builds both programs in release mode.
    & (Join-Path $PSScriptRoot 'build-installer.ps1') -Iscc $Iscc
    if ($LASTEXITCODE -and $LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $savedFlags
}

# No folder of this machine may be left in the programs.
foreach ($exe in 'beam.exe', 'beam-gui.exe') {
    $bytes = [System.IO.File]::ReadAllBytes((Join-Path $repo "target\release\$exe"))
    $text = [System.Text.Encoding]::GetEncoding(28591).GetString($bytes)
    foreach ($p in $private) {
        if ($text.IndexOf($p, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw "$exe still contains the local path $p. Nothing was packaged."
        }
    }
}

$manifest = Get-Content (Join-Path $repo 'Cargo.toml') -Raw
$section = [regex]::Match($manifest, '(?ms)^\[workspace\.package\](.*?)^\[')
$version = [regex]::Match($section.Groups[1].Value, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { throw 'No version under [workspace.package] in Cargo.toml.' }

$dist = Join-Path $repo 'target\dist'
New-Item -ItemType Directory -Force $dist | Out-Null
Copy-Item (Join-Path $repo "target\installer\beam-setup-$version.exe") $dist -Force

# 2. The zip.
$name = "beam-$version-windows-x86_64"
$stage = Join-Path $repo "target\dist-stage\$name"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force (Join-Path $stage 'licenses') | Out-Null
Copy-Item (Join-Path $repo 'target\release\beam.exe'), (Join-Path $repo 'target\release\beam-gui.exe') $stage
Copy-Item (Join-Path $repo 'LICENSE.md') $stage
Copy-Item (Join-Path $repo 'target\licenses\THIRD-PARTY.html') $stage
Copy-Item (Join-Path $repo 'crates\beam-core\LICENSE') (Join-Path $stage 'licenses\beam-core-MIT.txt')
Copy-Item (Join-Path $repo 'crates\beam-model\LICENSE') (Join-Path $stage 'licenses\beam-model-MIT.txt')
Copy-Item (Join-Path $repo 'crates\beam-gui\assets\LICENSES\*') (Join-Path $stage 'licenses')

$zip = Join-Path $dist "$name.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip
Remove-Item (Join-Path $repo 'target\dist-stage') -Recurse -Force

Write-Host "Built in $dist`:" -ForegroundColor Green
Get-ChildItem $dist -File | ForEach-Object { Write-Host "  $($_.Name)" }
