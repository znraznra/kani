<#
.SYNOPSIS
  Make a self-contained Kani folder (and zip) that runs on any Windows PC.

.DESCRIPTION
  For the build route used on the Dell: Rust (MSVC) with GTK 4 and GStreamer
  from MSYS2's UCRT64 environment. After `cargo build --release -p cutroom-gtk`,
  run this from the repository:

      powershell -ExecutionPolicy Bypass -File tools\windows\package.ps1

  It makes dist\Kani:
      bin\kani.exe              the program, with every DLL it needs beside it
      bin\gdk-pixbuf-query-loaders.exe
      lib\gstreamer-1.0\        every GStreamer plug-in (decoders, encoders,
                                sound outputs) and their DLLs in bin\
      lib\gdk-pixbuf-2.0\       image loaders (the SVG icons need one)
      libexec\gstreamer-1.0\    the plug-in scanner
      share\                    settings schemas and icon themes
  and dist\Kani-windows.zip with the same.

  kani.exe notices that it sits in such a folder and uses only these copies
  (see crates\cutroom-gtk\src\startup.rs), so it can be started straight from
  bin\kani.exe, from a shortcut, or after copying the folder to another PC.
  Nothing from MSYS2 or any other GTK / GStreamer install is used.

  Needs MSYS2's objdump to read which DLLs each file needs:
      pacman -S mingw-w64-ucrt-x86_64-binutils

.PARAMETER Msys
  The MSYS2 environment the GTK and GStreamer DLLs come from. Default
  C:\msys64\ucrt64 (falls back to C:\msys64\mingw64).

.PARAMETER Exe
  The built program. Default target\release\kani.exe.
#>
param(
  [string]$Msys = "",
  [string]$Exe = "target\release\kani.exe",
  [string]$Out = "dist\Kani"
)

$ErrorActionPreference = "Stop"
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $Root

if (-not $Msys) {
  foreach ($c in @("C:\msys64\ucrt64", "C:\msys64\mingw64")) {
    if (Test-Path "$c\bin\libgtk-4-1.dll") { $Msys = $c; break }
  }
}
if (-not $Msys -or -not (Test-Path "$Msys\bin")) { throw "MSYS2 GTK not found. Pass -Msys C:\path\to\msys64\ucrt64" }
$Objdump = "$Msys\bin\objdump.exe"
if (-not (Test-Path $Objdump)) { throw "objdump missing: in an MSYS2 shell run  pacman -S $(Split-Path $Msys -Leaf | ForEach-Object { if ($_ -eq 'ucrt64') {'mingw-w64-ucrt-x86_64-binutils'} else {'mingw-w64-x86_64-binutils'} })" }
if (-not (Test-Path $Exe)) { throw "Build first: cargo build --release -p cutroom-gtk" }

Write-Host "Packaging $Exe with the DLLs from $Msys into $Out" -ForegroundColor Cyan
if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
$Bin = Join-Path $Out "bin"
New-Item -ItemType Directory -Force $Bin, "$Out\lib\gstreamer-1.0", "$Out\libexec\gstreamer-1.0", "$Out\share\glib-2.0" | Out-Null

# The Microsoft C++ runtime kani.exe (MSVC build) needs: copied in so the PC
# it runs on needs no Visual C++ redistributable.
$System = [Environment]::GetFolderPath("System")
$VcRuntime = @("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll")

$Seen = @{}
$Missing = New-Object System.Collections.Generic.List[string]
function Copy-Deps([string]$File) {
  $names = & $Objdump -p $File 2>$null | Select-String "DLL Name:\s*(\S+)" | ForEach-Object { $_.Matches[0].Groups[1].Value }
  foreach ($name in $names) {
    $key = $name.ToLower()
    if ($Seen.ContainsKey($key)) { continue }
    $Seen[$key] = $true
    $fromMsys = Join-Path "$Msys\bin" $name
    if (Test-Path $fromMsys) {
      Copy-Item $fromMsys $Bin -Force
      Copy-Deps $fromMsys
    } elseif ($VcRuntime -contains $key) {
      Copy-Item (Join-Path $System $name) $Bin -Force
    } elseif (-not (Test-Path (Join-Path $System $name)) -and -not $key.StartsWith("api-ms-") -and -not $key.StartsWith("ext-ms-")) {
      $Missing.Add("$name (needed by $(Split-Path $File -Leaf))")
    }
  }
}

Copy-Item $Exe $Bin
Copy-Deps (Join-Path $Bin "kani.exe")

# GStreamer plug-ins and the scanner.
Copy-Item "$Msys\lib\gstreamer-1.0\*.dll" "$Out\lib\gstreamer-1.0\"
Copy-Item "$Msys\libexec\gstreamer-1.0\gst-plugin-scanner.exe" "$Out\libexec\gstreamer-1.0\"
Get-ChildItem "$Out\lib\gstreamer-1.0\*.dll" | ForEach-Object { Copy-Deps $_.FullName }
Copy-Deps "$Out\libexec\gstreamer-1.0\gst-plugin-scanner.exe"

# Image loaders and the tool that lists them (Kani runs it on first start).
$LoaderDir = Get-ChildItem "$Msys\lib\gdk-pixbuf-2.0\*\loaders" -Directory | Select-Object -First 1
$LoaderVer = Split-Path (Split-Path $LoaderDir.FullName -Parent) -Leaf
New-Item -ItemType Directory -Force "$Out\lib\gdk-pixbuf-2.0\$LoaderVer\loaders" | Out-Null
Copy-Item "$($LoaderDir.FullName)\*.dll" "$Out\lib\gdk-pixbuf-2.0\$LoaderVer\loaders\"
Get-ChildItem "$Out\lib\gdk-pixbuf-2.0\$LoaderVer\loaders\*.dll" | ForEach-Object { Copy-Deps $_.FullName }
Copy-Item "$Msys\bin\gdk-pixbuf-query-loaders.exe" $Bin
Copy-Deps "$Bin\gdk-pixbuf-query-loaders.exe"

# Settings schemas and icon themes.
Copy-Item -Recurse "$Msys\share\glib-2.0\schemas" "$Out\share\glib-2.0\"
New-Item -ItemType Directory -Force "$Out\share\icons" | Out-Null
foreach ($theme in @("Adwaita", "hicolor")) {
  if (Test-Path "$Msys\share\icons\$theme") { Copy-Item -Recurse "$Msys\share\icons\$theme" "$Out\share\icons\" }
}

# A note for whoever opens the folder.
@"
Kani $((Get-Item $Exe).VersionInfo.ProductVersion)
Start bin\kani.exe (make a shortcut to it if you like). Everything it needs is in this folder.
Kani is portable: its settings, logs and caches live in the data folder here, never in AppData.
If Kani ever closes badly, it starts in safe mode the next time (no hardware decoding,
no graphics-chip drawing). Start-up steps are logged to data\logs\startup.log.
"@ | Set-Content -Encoding UTF8 "$Out\README.txt"

if ($Missing.Count -gt 0) {
  Write-Host "These DLLs were not found in MSYS2 or Windows (Kani may still run if they are optional):" -ForegroundColor Yellow
  $Missing | Sort-Object -Unique | ForEach-Object { Write-Host "  $_" }
}

$Zip = "dist\Kani-windows.zip"
if (Test-Path $Zip) { Remove-Item $Zip }
Compress-Archive -Path $Out -DestinationPath $Zip
$size = "{0:N0} MB" -f ((Get-ChildItem -Recurse $Out | Measure-Object Length -Sum).Sum / 1MB)
Write-Host "Done: $Out ($size), $Zip" -ForegroundColor Green
Write-Host "Test it: rename C:\msys64 for a moment (or run on another PC) and start $Out\bin\kani.exe"
