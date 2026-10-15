#!/usr/bin/env bash
# Describes every video/audio file in a folder (default: the current one) so
# Kani's footage handling can be tuned to what you really shoot.
# Needs ffprobe (part of FFmpeg). Writes footage-report.txt next to where you run it.
set -u
dir="${1:-.}"
out="footage-report.txt"
command -v ffprobe >/dev/null || { echo "ffprobe not found (install FFmpeg)"; exit 1; }
{
  echo "Kani footage report - $(date -u +%Y-%m-%d) - $(uname -sr)"
  echo "Folder: $(cd "$dir" && pwd)"
  echo
  find "$dir" -maxdepth 2 -type f \( -iname '*.mp4' -o -iname '*.mov' -o -iname '*.mkv' -o -iname '*.webm' \
      -o -iname '*.m4v' -o -iname '*.avi' -o -iname '*.mts' -o -iname '*.m2ts' -o -iname '*.ts' \
      -o -iname '*.mp3' -o -iname '*.wav' -o -iname '*.m4a' -o -iname '*.aac' -o -iname '*.flac' -o -iname '*.ogg' \) \
      | sort | head -40 | while IFS= read -r f; do
    echo "=== $(basename "$f")  ($(du -h "$f" | cut -f1))"
    ffprobe -v error \
      -show_entries stream=codec_type,codec_name,profile,width,height,r_frame_rate,avg_frame_rate,pix_fmt,color_space,color_transfer,color_primaries,sample_rate,channels,bit_rate,nb_frames:stream_tags=rotate:stream_side_data=rotation:format=format_name,duration,bit_rate \
      -of default=nw=1 "$f" 2>&1
    echo
  done
} > "$out"
echo "Wrote $out ($(grep -c '^===' "$out") files). Paste its contents into the chat."
