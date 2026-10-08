# Installs beam for the current Windows user, so `beam` works in any new
# terminal without typing .\beam.exe.
#
#   powershell -ExecutionPolicy Bypass -File scripts\install.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -Uninstall
#
# Or double-click scripts\install.bat.
#
# What it does:
#   1. takes beam.exe from next to this script if there is one (a copy handed
#      over without the source), otherwise builds it: cargo build --release;
#   2. copies it to %LOCALAPPDATA%\Programs\beam (no administrator rights);
#   3. adds that folder to the *user* PATH, once.
# It never touches ~/.beam, where the device key and paired peers live.

param(
    [switch]$Uninstall,
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'Programs\beam'),
    # Copy only; leave PATH alone. For trying the script out.
    [switch]$NoPath
)

$ErrorActionPreference = 'Stop'

function Normalize([string]$dir) {
    return $dir.Trim().TrimEnd('\').ToLowerInvariant()
}

# PATH with `$entry` added at the end, unless an entry already names it.
function Add-PathEntry([string]$path, [string]$entry) {
    $parts = @($path -split ';' | Where-Object { $_.Trim() -ne '' })
    if ($parts | Where-Object { (Normalize $_) -eq (Normalize $entry) }) {
        return ($parts -join ';')
    }
    return (@($parts) + $entry) -join ';'
}

# PATH with every entry naming `$entry` removed.
function Remove-PathEntry([string]$path, [string]$entry) {
    $parts = @($path -split ';' | Where-Object {
        $_.Trim() -ne '' -and (Normalize $_) -ne (Normalize $entry)
    })
    return ($parts -join ';')
}

# The user PATH exactly as stored, without expanding %VARIABLES%, so writing
# it back keeps entries such as %USERPROFILE%\... as they were.
function Get-UserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    try {
        $raw = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
        $kind = if ($raw) { $key.GetValueKind('Path') } else { 'ExpandString' }
        return @{ Value = [string]$raw; Kind = $kind }
    } finally { $key.Close() }
}

function Set-UserPath([string]$value, $kind) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try { $key.SetValue('Path', $value, $kind) } finally { $key.Close() }
    # Tell Windows the environment changed, so new terminals see it.
    if (-not ('Beam.Env' -as [type])) {
        Add-Type -Namespace Beam -Name Env -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam,
    string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
    }
    $result = [UIntPtr]::Zero
    [void][Beam.Env]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero,
        'Environment', 2, 5000, [ref]$result)
}

$exe = Join-Path $Destination 'beam.exe'

if ($Uninstall) {
    if (Test-Path $exe) {
        Remove-Item $exe -Force
        Write-Host "Removed $exe"
    }
    if ((Test-Path $Destination) -and -not (Get-ChildItem $Destination -Force)) {
        Remove-Item $Destination -Force
    }
    if (-not $NoPath) {
        $user = Get-UserPath
        $new = Remove-PathEntry $user.Value $Destination
        if ($new -ne $user.Value) {
            Set-UserPath $new $user.Kind
            Write-Host "Removed $Destination from your PATH."
        }
    }
    Write-Host 'beam is uninstalled. Your keys and paired peers in ~/.beam were not touched.'
    exit 0
}

# 1. Find or build beam.exe.
$bundled = Join-Path $PSScriptRoot 'beam.exe'
if (Test-Path $bundled) {
    $source = $bundled
} else {
    $repo = Join-Path $PSScriptRoot '..'
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
        if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
        else { throw 'No beam.exe next to this script, and cargo was not found to build one. Install Rust from https://rustup.rs/' }
    }
    Write-Host '==> cargo build --release' -ForegroundColor Cyan
    Push-Location $repo
    try {
        cargo build --release -p beam
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    } finally { Pop-Location }
    $source = Join-Path $repo 'target\release\beam.exe'
}

# 2. Copy it into place.
New-Item -ItemType Directory -Force -Path $Destination | Out-Null
try {
    Copy-Item $source $exe -Force
} catch {
    throw "Could not replace $exe. Is beam still running? Stop it (Ctrl+C) and try again. ($_)"
}
Write-Host "Installed $exe"

# 3. Put it on the user PATH.
if (-not $NoPath) {
    $user = Get-UserPath
    $new = Add-PathEntry $user.Value $Destination
    if ($new -ne $user.Value) {
        Set-UserPath $new $user.Kind
        Write-Host "Added $Destination to your PATH."
    } else {
        Write-Host "$Destination is already on your PATH."
    }
}

& $exe --version
Write-Host ''
Write-Host 'Done. Open a NEW terminal and run:  beam --help' -ForegroundColor Green
Write-Host '(Terminals that were already open keep the old PATH.)'
