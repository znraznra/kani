# Describes every video/audio file in a folder (default: the current one).
# Needs ffprobe on PATH (part of FFmpeg). Writes footage-report.txt in the current folder.
param([string]$Dir = ".")
if (-not (Get-Command ffprobe -ErrorAction SilentlyContinue)) { Write-Host "ffprobe not found (install FFmpeg)"; exit 1 }
$ext = '.mp4','.mov','.mkv','.webm','.m4v','.avi','.mts','.m2ts','.ts','.mp3','.wav','.m4a','.aac','.flac','.ogg'
$files = Get-ChildItem -Path $Dir -File -Recurse -Depth 1 | Where-Object { $ext -contains $_.Extension.ToLower() } | Sort-Object FullName | Select-Object -First 40
$lines = @("Kani footage report - $(Get-Date -Format yyyy-MM-dd) - Windows", "Folder: $((Resolve-Path $Dir).Path)", "")
foreach ($f in $files) {
  $lines += "=== $($f.Name)  ($([math]::Round($f.Length / 1MB, 1)) MB)"
  $lines += & ffprobe -v error -show_entries "stream=codec_type,codec_name,profile,width,height,r_frame_rate,avg_frame_rate,pix_fmt,color_space,color_transfer,color_primaries,sample_rate,channels,bit_rate,nb_frames:stream_tags=rotate:stream_side_data=rotation:format=format_name,duration,bit_rate" -of "default=nw=1" $f.FullName 2>&1
  $lines += ""
}
$lines | Set-Content -Encoding utf8 footage-report.txt
Write-Host "Wrote footage-report.txt ($($files.Count) files). Paste its contents into the chat."
