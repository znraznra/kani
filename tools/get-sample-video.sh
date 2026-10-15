#!/usr/bin/env bash
# Downloads Big Buck Bunny (640x360 H.264 + AAC, 60 s, CC-BY, (c) Blender Foundation) for testing,
# and tells you how to point Kani's real-footage test at it.
set -eu
out="${1:-big_buck_bunny.mp4}"
url="https://github.com/mediaelement/mediaelement-files/raw/master/big_buck_bunny.mp4"
command -v curl >/dev/null || { echo "curl not found"; exit 1; }
curl -fL -o "$out" "$url"
echo "Saved $out ($(du -h "$out" | cut -f1))."
echo "Run the real-footage playback test with:"
echo "  CUTROOM_SAMPLE_VIDEO=\"$(cd "$(dirname "$out")" && pwd)/$(basename "$out")\" cargo test -p cutroom-gtk preview"
