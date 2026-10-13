#!/usr/bin/env bash
# Re-record demo footage against a real headless Vivido.
#
# Requires: vhs, ffmpeg, python3, cargo, and VIVIDO_BIN pointing at a vivido
# binary (e.g. nix shell nixpkgs#vhs nixpkgs#ffmpeg, then run this script).
set -euo pipefail

cd "$(dirname "$0")/.."
export VIVIDO_BIN="${VIVIDO_BIN:-/tmp/vivido-upstream/target/release/vivido}"

cargo build --locked

if ! "$VIVIDO_BIN" msg --target demodrive ping >/dev/null 2>&1; then
  echo "starting demo session demodrive…"
  "$VIVIDO_BIN" --headless --session demodrive --headless-size 1280x720px
fi

mkdir -p assets
for scenario in observe act tiers headless; do
  echo "=== recording $scenario ==="
  vhs "demo/$scenario.tape"
  ffmpeg -y -v error -i "assets/demo-$scenario.mp4" \
    -vf "fps=10,scale=720:-1:flags=lanczos,palettegen" /tmp/demo-pal.png
  ffmpeg -y -v error -i "assets/demo-$scenario.mp4" -i /tmp/demo-pal.png \
    -lavfi "fps=10,scale=720:-1:flags=lanczos[x];[x][1:v]paletteuse" \
    "assets/demo-$scenario.gif"
done

echo "=== assets ==="
ls -la assets/
