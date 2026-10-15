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

`improve.py` proves the improvement batches the same way: `spellings` drives
forgiving spellings (key aliases, `SIGWINCH`, `"10S"`, `Click`) on a fresh
ephemeral session and verifies with text plus a settled JPEG capture; `strict`
shows fail-fast CLI/MCP behavior (tier suggestion, refusal with the env fix,
prefix suggestions, `invalid_args` battery, `-32600`/`-32602`) and ends with a
JPEG capture reporting raw bytes. Every line is real MCP output — no faked frames.

```sh
nix shell nixpkgs#vhs nixpkgs#ffmpeg
VIVIDO_BIN=/path/to/vivido ./demo/record-improvements.sh
```

`record-improvements.sh` builds, records `spellings`/`strict` via their `.tape`
files, derives the GIFs, and runs `verify.sh`, which checks every clip decodes,
runs 10–15s, and carries non-blank animated frames (`verify.sh` with no arguments
checks all `assets/demo-*` clips).
