#!/usr/bin/env bash
# Re-record the Phase 2 improvement demos against real headless Vivido.
#
# Requires: vhs, ffmpeg (+ffprobe), python3, cargo, and VIVIDO_BIN pointing at
# a vivido binary (e.g. nix shell nixpkgs#vhs nixpkgs#ffmpeg, then run this).
set -euo pipefail

cd "$(dirname "$0")/.."
export VIVIDO_BIN="${VIVIDO_BIN:-/tmp/vivido-upstream/target/release/vivido}"

cargo build --locked

if [[ ! -x "$VIVIDO_BIN" ]]; then
  echo "VIVIDO_BIN=$VIVIDO_BIN is not executable" >&2
  exit 1
fi

mkdir -p assets
for scenario in spellings strict; do
  echo "=== recording $scenario ==="
  vhs "demo/$scenario.tape"
  ffmpeg -y -v error -i "assets/demo-$scenario.mp4" \
    -vf "fps=10,scale=720:-1:flags=lanczos,palettegen" /tmp/demo-pal.png
  ffmpeg -y -v error -i "assets/demo-$scenario.mp4" -i /tmp/demo-pal.png \
    -lavfi "fps=10,scale=720:-1:flags=lanczos[x];[x][1:v]paletteuse" \
    "assets/demo-$scenario.gif"
done

./demo/verify.sh assets/demo-spellings.mp4 assets/demo-spellings.gif \
  assets/demo-strict.mp4 assets/demo-strict.gif

echo "=== assets ==="
ls -la assets/demo-spellings.* assets/demo-strict.*
