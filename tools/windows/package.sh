#!/usr/bin/env bash
# Gathers kani.exe and everything it needs into dist/Kani (run inside the
# MSYS2 MINGW64 shell; setup-windows.ps1 does that for you).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/dist/Kani"
MINGW=/mingw64

[ -f "$ROOT/target/release/kani.exe" ] || { echo "Build first: cargo build --release -p cutroom-gtk"; exit 1; }

rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/lib/gstreamer-1.0" "$OUT/libexec/gstreamer-1.0" "$OUT/share/glib-2.0"
cp "$ROOT/target/release/kani.exe" "$OUT/bin/"

# Copy the MinGW DLLs a binary needs, and theirs in turn.
copy_deps() {
  ldd "$1" 2>/dev/null | awk '/=> \/mingw64\// {print $3}' | while read -r dll; do
    if [ ! -f "$OUT/bin/$(basename "$dll")" ]; then
      cp "$dll" "$OUT/bin/"
      copy_deps "$dll"
    fi
  done
}
copy_deps "$OUT/bin/kani.exe"

# GStreamer: plugins and the helper that scans them.
cp "$MINGW"/lib/gstreamer-1.0/*.dll "$OUT/lib/gstreamer-1.0/"
cp "$MINGW/libexec/gstreamer-1.0/gst-plugin-scanner.exe" "$OUT/libexec/gstreamer-1.0/"
for f in "$OUT"/lib/gstreamer-1.0/*.dll "$OUT/libexec/gstreamer-1.0/gst-plugin-scanner.exe"; do
  copy_deps "$f"
done

# Image loaders (SVG icons and the rest) and the tool that lists them.
LOADER_DIR="$(ls -d "$MINGW"/lib/gdk-pixbuf-2.0/*/loaders | head -n1)"
LOADER_VER="$(basename "$(dirname "$LOADER_DIR")")"
mkdir -p "$OUT/lib/gdk-pixbuf-2.0/$LOADER_VER/loaders"
cp "$LOADER_DIR"/*.dll "$OUT/lib/gdk-pixbuf-2.0/$LOADER_VER/loaders/"
cp "$MINGW/bin/gdk-pixbuf-query-loaders.exe" "$OUT/bin/"
copy_deps "$OUT/bin/gdk-pixbuf-query-loaders.exe"
for f in "$OUT/lib/gdk-pixbuf-2.0/$LOADER_VER/loaders"/*.dll; do copy_deps "$f"; done

# Settings schemas and icons.
cp -r "$MINGW/share/glib-2.0/schemas" "$OUT/share/glib-2.0/"
mkdir -p "$OUT/share/icons"
for theme in Adwaita hicolor; do
  [ -d "$MINGW/share/icons/$theme" ] && cp -r "$MINGW/share/icons/$theme" "$OUT/share/icons/"
done

# The launcher: points everything at this folder, then starts the program.
cat > "$OUT/Kani.bat" <<BAT
@echo off
setlocal
set "HERE=%~dp0"
set "PATH=%HERE%bin;%PATH%"
set "DATA=%LOCALAPPDATA%\\Kani"
if not exist "%DATA%" mkdir "%DATA%"
set "GST_PLUGIN_PATH=%HERE%lib\\gstreamer-1.0"
set "GST_PLUGIN_SYSTEM_PATH="
set "GST_PLUGIN_SCANNER=%HERE%libexec\\gstreamer-1.0\\gst-plugin-scanner.exe"
set "GST_REGISTRY=%DATA%\\gst-registry.bin"
set "GSETTINGS_SCHEMA_DIR=%HERE%share\\glib-2.0\\schemas"
set "XDG_DATA_DIRS=%HERE%share"
set "GDK_PIXBUF_MODULEDIR=%HERE%lib\\gdk-pixbuf-2.0\\$LOADER_VER\\loaders"
set "GDK_PIXBUF_MODULE_FILE=%DATA%\\loaders.cache"
if not exist "%GDK_PIXBUF_MODULE_FILE%" powershell -NoProfile -Command "& '%HERE%bin\\gdk-pixbuf-query-loaders.exe' (Get-ChildItem '%GDK_PIXBUF_MODULEDIR%\\*.dll').FullName | Set-Content -Encoding ascii '%GDK_PIXBUF_MODULE_FILE%'"
start "" "%HERE%bin\\kani.exe" %*
BAT
# Windows wants CRLF in batch files.
sed -i 's/$/\r/' "$OUT/Kani.bat"

# A zip to carry to another PC.
rm -f "$ROOT/dist/Kani-windows.zip"
(cd "$ROOT/dist" && zip -qr Kani-windows.zip Kani)

echo "Packaged: $OUT"
echo "Zip:      $ROOT/dist/Kani-windows.zip"
