#!/usr/bin/env bash
# Cuts the first 10 seconds of each file you name, without re-encoding, into ./samples
# Usage: make-samples.sh clip1.mp4 clip2.mov ...
set -u
command -v ffmpeg >/dev/null || { echo "ffmpeg not found"; exit 1; }
[ $# -gt 0 ] || { echo "Usage: $0 file [file ...]"; exit 1; }
mkdir -p samples
n=0
for f in "$@"; do
  n=$((n + 1))
  base="$(basename "$f")"
  ffmpeg -v error -y -ss 0 -t 10 -i "$f" -map 0 -c copy "samples/$(printf '%02d' "$n")-${base}" \
    && echo "ok: samples/$(printf '%02d' "$n")-${base}"
done
du -sh samples
echo "Upload the files in ./samples (aim for under 100 MB in total)."
