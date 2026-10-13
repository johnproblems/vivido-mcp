# Demo footage

`drive.py` speaks MCP to a real `vivido-mcp` connected to a real headless Vivido
session (`demodrive`), with paced narration for recording. The `.tape` files render
each scenario to MP4 via [VHS](https://github.com/charmbracelet/vhs); `record.sh`
re-records everything and derives the GIFs with ffmpeg.

```sh
nix shell nixpkgs#vhs nixpkgs#ffmpeg
VIVIDO_BIN=/path/to/vivido ./demo/record.sh
```

Scenarios: `observe` (discover + read), `act` (type, wait, verify), `tiers`
(observe-only refusal → `--allow input`), `headless` (ephemeral session lifecycle).
Screenshots captured mid-demo land in `assets/shot-*.png` — real rendered frames.
