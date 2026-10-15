# Cuts the first 10 seconds of each file you name, without re-encoding, into .\samples
# Usage: .\make-samples.ps1 clip1.mp4 clip2.mov
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { Write-Host "ffmpeg not found"; exit 1 }
if ($args.Count -eq 0) { Write-Host "Usage: .\make-samples.ps1 file [file ...]"; exit 1 }
New-Item -ItemType Directory -Force samples | Out-Null
$n = 0
foreach ($f in $args) {
  $n++
  $name = "{0:D2}-{1}" -f $n, (Split-Path $f -Leaf)
  & ffmpeg -v error -y -ss 0 -t 10 -i $f -map 0 -c copy "samples\$name"
  if ($LASTEXITCODE -eq 0) { Write-Host "ok: samples\$name" }
}
$mb = [math]::Round(((Get-ChildItem samples | Measure-Object Length -Sum).Sum / 1MB), 1)
Write-Host "samples: $mb MB. Upload the files in .\samples (aim for under 100 MB in total)."
