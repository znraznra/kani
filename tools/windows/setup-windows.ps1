<#
.SYNOPSIS
  One-step Windows setup for Kani: installs what is missing, builds, packages.

.DESCRIPTION
  Run from anywhere (double-click setup-windows.bat, or run this in PowerShell).
  It is safe to run again: whatever is already installed is left alone.

    1. MSYS2 (the GTK4 / GStreamer / compiler environment), via winget, or a
       direct download if winget is missing.
    2. Every package Kani needs inside MSYS2 (compiler, Rust, GTK4, GStreamer
       and its plugins, cmake, clang for the speech engine, icons).
    3. A release build of Kani.
    4. dist\Kani   : a folder with kani.exe, every DLL, GStreamer plugins and
                     icons it needs, and Kani.bat to start it.
       dist\Kani-windows.zip : the same, zipped, to copy to another PC.

  Needs: Windows 10 or 11, an internet connection, and about 6 GB of disk.
  The first run takes a while (the speech engine is compiled from source).

.PARAMETER NoWhisper
  Build without speech recognition (much faster, no captions generation).

.PARAMETER MsysRoot
  Where MSYS2 is (or will be installed). Default C:\msys64.

.PARAMETER Run
  Start Kani when done.
#>
param(
  [switch]$NoWhisper,
  [switch]$Run,
  [string]$MsysRoot = "C:\msys64"
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

function Step($text) { Write-Host ""; Write-Host "==> $text" -ForegroundColor Cyan }

function Bash($command) {
  # Runs a command in the MSYS2 MINGW64 environment and stops on failure.
  $bash = Join-Path $MsysRoot "usr\bin\bash.exe"
  $env:MSYSTEM = "MINGW64"
  $env:CHERE_INVOKING = "1"
  & $bash -lc $command
  if ($LASTEXITCODE -ne 0) { throw "A step failed (exit code $LASTEXITCODE): $command" }
}

# ---- 1. MSYS2 --------------------------------------------------------------------
Step "Checking MSYS2"
if (-not (Test-Path (Join-Path $MsysRoot "usr\bin\bash.exe"))) {
  $winget = Get-Command winget -ErrorAction SilentlyContinue
  if ($winget -and $MsysRoot -eq "C:\msys64") {
    Write-Host "Installing MSYS2 with winget (you may see a permission prompt)..."
    winget install -e --id MSYS2.MSYS2 --silent --accept-package-agreements --accept-source-agreements
  }
  if (-not (Test-Path (Join-Path $MsysRoot "usr\bin\bash.exe"))) {
    Write-Host "Downloading the MSYS2 installer..."
    $installer = Join-Path $env:TEMP "msys2-installer.exe"
    Invoke-WebRequest "https://github.com/msys2/msys2-installer/releases/latest/download/msys2-x86_64-latest.exe" -OutFile $installer
    & $installer install --root $MsysRoot --confirm-command
    if ($LASTEXITCODE -ne 0) { throw "The MSYS2 installer failed." }
  }
}
if (-not (Test-Path (Join-Path $MsysRoot "usr\bin\bash.exe"))) {
  throw "MSYS2 could not be installed. Install it from https://www.msys2.org and run this again."
}
Write-Host "MSYS2 found at $MsysRoot"

# ---- 2. Packages -----------------------------------------------------------------
Step "Updating MSYS2 (this can take a few minutes the first time)"
# The first update may close the shell after updating the core; run it twice.
try { Bash "pacman -Syu --noconfirm" } catch { Write-Host "(first update pass ended; continuing)" }
Bash "pacman -Syu --noconfirm"

Step "Installing the libraries and tools Kani needs"
$packages = @(
  "mingw-w64-x86_64-toolchain", "mingw-w64-x86_64-pkgconf", "mingw-w64-x86_64-rust",
  "mingw-w64-x86_64-cmake", "mingw-w64-x86_64-ninja", "mingw-w64-x86_64-make",
  "mingw-w64-x86_64-clang", "mingw-w64-x86_64-clang-tools-extra",
  "mingw-w64-x86_64-gtk4", "mingw-w64-x86_64-libadwaita",
  "mingw-w64-x86_64-gstreamer", "mingw-w64-x86_64-gst-plugins-base",
  "mingw-w64-x86_64-gst-plugins-good", "mingw-w64-x86_64-gst-plugins-bad",
  "mingw-w64-x86_64-gst-plugins-ugly", "mingw-w64-x86_64-gst-libav",
  "mingw-w64-x86_64-adwaita-icon-theme", "mingw-w64-x86_64-hicolor-icon-theme",
  "mingw-w64-x86_64-librsvg", "mingw-w64-x86_64-gdk-pixbuf2",
  "zip", "unzip", "git"
)
Bash ("pacman -S --needed --noconfirm " + ($packages -join " "))

# ---- 3. Build --------------------------------------------------------------------
Step "Building Kani (the first build takes a while)"
$repoUnix = (& (Join-Path $MsysRoot "usr\bin\cygpath.exe") -u $RepoRoot).Trim()
$features = if ($NoWhisper) { "--no-default-features" } else { "" }
$build = @"
set -e
cd '$repoUnix'
export LIBCLANG_PATH=/mingw64/bin
export CMAKE_GENERATOR=Ninja
cargo build --release -p cutroom-gtk $features
"@
Bash $build

# ---- 4. Package ------------------------------------------------------------------
Step "Packaging dist\Kani"
Bash "cd '$repoUnix' && bash tools/windows/package.sh"

$dist = Join-Path $RepoRoot "dist"
Write-Host ""
Write-Host "Done." -ForegroundColor Green
Write-Host "  Run:   $dist\Kani\Kani.bat"
Write-Host "  Share: $dist\Kani-windows.zip (unzip anywhere, run Kani.bat)"
if ($Run) { Start-Process (Join-Path $dist "Kani\Kani.bat") }
