#!/usr/bin/env bash
# Verify demo footage: each file must decode, run 10-15s, and carry non-blank
# animated frames. Usage: demo/verify.sh <file>... (defaults to all demo-* clips).
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ $# -eq 0 ]]; then
  set -- assets/demo-*.mp4 assets/demo-*.gif
fi

fail=0
for file in "$@"; do
  echo "--- $file ---"
  if [[ ! -s "$file" ]]; then
    echo "FAIL: missing or empty"; fail=1; continue
  fi
  # Decodes without errors.
  if ! ffmpeg -v error -i "$file" -f null - 2>/tmp/verify-err.txt; then
    echo "FAIL: decode errors:"; cat /tmp/verify-err.txt; fail=1; continue
  fi
  if [[ -s /tmp/verify-err.txt ]]; then
    echo "FAIL: ffmpeg warnings:"; cat /tmp/verify-err.txt; fail=1; continue
  fi
  duration=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$file")
  echo "duration: ${duration}s"
  if ! python3 -c "import sys; sys.exit(0 if 9.5 <= float('$duration') <= 15.5 else 1)"; then
    echo "FAIL: duration outside 10-15s"; fail=1; continue
  fi
  frames=$(ffprobe -v error -select_streams v:0 -show_entries stream=nb_frames -of csv=p=0 "$file")
  echo "frames: $frames"
  dir=$(mktemp -d)
  # Three frames across the runtime: all must be non-trivially sized
  # (non-blank) and first/last must differ (animated).
  ffmpeg -y -v error -ss 1 -i "$file" -frames:v 1 "$dir/a.png"
  mid=$(python3 -c "print(float('$duration') / 2)")
  ffmpeg -y -v error -ss "$mid" -i "$file" -frames:v 1 "$dir/b.png"
  late=$(python3 -c "print(float('$duration') - 1)")
  ffmpeg -y -v error -ss "$late" -i "$file" -frames:v 1 "$dir/c.png"
  sizes=$(wc -c < "$dir/a.png")/$(wc -c < "$dir/b.png")/$(wc -c < "$dir/c.png")
  echo "frame bytes: $sizes"
  if [[ $(wc -c < "$dir/a.png") -lt 2048 || $(wc -c < "$dir/b.png") -lt 2048 || $(wc -c < "$dir/c.png") -lt 2048 ]]; then
    echo "FAIL: a frame looks blank (<2KB)"; fail=1; rm -rf "$dir"; continue
  fi
  if cmp -s "$dir/a.png" "$dir/c.png"; then
    echo "FAIL: first and last frames are identical (not animated)"; fail=1; rm -rf "$dir"; continue
  fi
  echo "PASS"
  rm -rf "$dir"
done

if [[ $fail -ne 0 ]]; then
  echo "VERIFY FAILED" >&2
  exit 1
fi
echo "ALL VERIFIED"
