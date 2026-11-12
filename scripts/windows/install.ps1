# Installs beam from source for the current Windows user: builds it, copies
# beam.exe (and beam-gui.exe) into place, adds Start menu shortcuts, and puts
# `beam` on the PATH. For people building beam themselves; everyone else uses
# the setup wizard (scripts\windows\build-installer.ps1 makes it).
#
#   powershell -ExecutionPolicy Bypass -File scripts\windows\install.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\install.ps1 -NoGui
#   powershell -ExecutionPolicy Bypass -File scripts\windows\install.ps1 -Gui
#   powershell -ExecutionPolicy Bypass -File scripts\windows\install.ps1 -Uninstall
#
# Without -Gui or -NoGui it asks whether to install the desktop window
# (beam-gui), and the answer starts on yes.
#
# What it does:
#   1. cargo build --release -p beam [-p beam-gui];
#   2. copies them to %LOCALAPPDATA%\Programs\beam (no administrator rights),
#      the same folder the setup wizard uses;
#   3. adds Start menu shortcuts: "beam" (the window) and "beam (terminal)";
#   4. adds that folder to the *user* PATH, once.
# Without the window, an earlier install of it is removed. It never touches
# ~/.beam, where the device key and paired peers live.

param(
    [switch]$Uninstall,
    # With the desktop window, without asking.
    [switch]$Gui,
    # Only the command line and the full-screen view.
    [switch]$NoGui,
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'Programs\beam'),
    # Copy only; leave PATH and the Start menu alone. For trying the script out.
    [switch]$NoPath
)

$ErrorActionPreference = 'Stop'
$menu = Join-Path ([Environment]::GetFolderPath('Programs')) 'beam'
$guiLink = Join-Path $menu 'beam.lnk'

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

function New-Shortcut([string]$path, [string]$target, [string]$description) {
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut($path)
    $link.TargetPath = $target
    $link.WorkingDirectory = $env:USERPROFILE
    $link.Description = $description
    $link.Save()
}

if ($Uninstall) {
    foreach ($p in 'beam.exe', 'beam-gui.exe') {
        $exe = Join-Path $Destination $p
        if (Test-Path $exe) {
            Remove-Item $exe -Force
            Write-Host "Removed $exe"
        }
    }
    if ((Test-Path $Destination) -and -not (Get-ChildItem $Destination -Force)) {
        Remove-Item $Destination -Force
    }
    if (-not $NoPath) {
        if (Test-Path $menu) { Remove-Item $menu -Recurse -Force }
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

if ($Gui -and $NoGui) { throw 'Choose -Gui or -NoGui, not both.' }
if (-not $Gui -and -not $NoGui) {
    $answer = Read-Host 'Install the desktop window (beam-gui) too? [Y/n]'
    $NoGui = $answer -match '^\s*n'
}
$programs = if ($NoGui) { @('beam.exe') } else { @('beam.exe', 'beam-gui.exe') }
$packages = $programs | ForEach-Object { '-p'; [IO.Path]::GetFileNameWithoutExtension($_) }

# 1. Build.
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
    else { throw 'cargo was not found. Install Rust from https://rustup.rs/' }
}
Write-Host "==> cargo build --release $($packages -join ' ')" -ForegroundColor Cyan
Push-Location $repo
try {
    cargo build --release @packages
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally { Pop-Location }

# 2. Copy them into place.
New-Item -ItemType Directory -Force -Path $Destination | Out-Null
foreach ($p in $programs) {
    $exe = Join-Path $Destination $p
    try {
        Copy-Item (Join-Path $repo "target\release\$p") $exe -Force
    } catch {
        throw "Could not replace $exe. Is beam still running? Close it (and stop the background agent: beam service stop), then try again. ($_)"
    }
    Write-Host "Installed $exe"
}
if ($NoGui) {
    $old = Join-Path $Destination 'beam-gui.exe'
    if (Test-Path $old) {
        Remove-Item $old -Force
        Write-Host "Removed the desktop window from an earlier install: $old"
    }
}

if (-not $NoPath) {
    # 3. Start menu.
    New-Item -ItemType Directory -Force -Path $menu | Out-Null
    if ($NoGui) {
        if (Test-Path $guiLink) { Remove-Item $guiLink -Force }
    } else {
        New-Shortcut $guiLink (Join-Path $Destination 'beam-gui.exe') 'Send files straight to the people you pair with'
    }
    New-Shortcut (Join-Path $menu 'beam (terminal).lnk') (Join-Path $Destination 'beam.exe') "beam's full-screen view in a terminal"
    Write-Host "Added beam to the Start menu."

    # 4. PATH.
    $user = Get-UserPath
    $new = Add-PathEntry $user.Value $Destination
    if ($new -ne $user.Value) {
        Set-UserPath $new $user.Kind
        Write-Host "Added $Destination to your PATH."
    } else {
        Write-Host "$Destination is already on your PATH."
    }
}

& (Join-Path $Destination 'beam.exe') --version
Write-Host ''
Write-Host 'Done. Open beam from the Start menu, or a NEW terminal and run:  beam --help' -ForegroundColor Green
Write-Host '(Terminals that were already open keep the old PATH.)'
