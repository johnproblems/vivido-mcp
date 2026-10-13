# Changelog

All notable changes to vivido-mcp, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.html).

## [Unreleased]

### Added

- `headless_list` observe-tier tool: list registered sessions with liveness, never paths.
- `vivido_debug` prompt: snapshot → history → gaps → headless repro workflow.
- JPEG screenshot output (`format: "jpeg"`, `quality`), for photographic Vivid media.
- Screenshot metadata: `mime`, `bytes`, `max_dim`, `crop_clamped`.
- `subscribe` one-shot collection returns a `cursor` for continuing with `drain_events`.
- `headless_start` echoes `size` and `ephemeral`; `headless_attach` reports `already_attached`.
- `transcript` with `raw` reports `bytes` and `lossy`.
- Prompts include live session context (enabled tiers, attached session).
- `VIVIDO_MCP_ALLOW` and `VIVIDO_BIN` environment fallbacks for `--allow` and `--vivido-bin`.
- `--allow all` alias for `--allow process`.
- `~/` expansion for `--socket`.
- Opt-in per-call timing log via `VIVIDO_MCP_LOG_TIMINGS=1`.
- Recovery hints on common error codes (`window_not_found`, `sequence_gap`, …).
- `SECURITY.md`, `CHANGELOG.md`, pull-request template, Dependabot config, cargo-deny policy.
- Demo footage: GIF + MP4 clips and captured frames in `assets/`, recorded against real
  headless Vivido; `demo/` holds the driver, VHS tapes, and reproduction script.

### Fixed

- CI: unix-only test module no longer breaks the Windows clippy gate.
- CI: real-headless job installs pinned `vivido` from crates.io instead of building
  git HEAD, whose sibling path dependencies lag behind.

### Changed

- Unknown tools and arguments suggest the closest match ("did you mean …?").
- Unknown tool arguments are rejected instead of silently ignored.
- Named keys, modifiers, routes, and levels match case-insensitively and canonicalize.
- `mouse` accepts the dashed `double-click` spelling; Vivid waits accept `snake_case`.
- `drop_file.at` accepts `"col,row"`; `create_window` accepts a `command` array.
- Single strings coerce to one-element lists for `events`, `mods`, `options`, `args`.
- `mouse scroll` with zero amounts and empty `exec` commands fail fast client-side.
- `headless_stop` verifies the endpoint actually went down, escalating when it stays up.
- Tier refusals name the allowance the server started with.
- Disconnected errors suggest starting Vivido or calling `headless_start`.
- Transport errors carry the endpoint label; startup logs the Vivido version and tool count.
- MCP: missing tool names are `-32602`, id-less frames take no reply, `$/…` methods are acked.
- Oversized (>1 MiB) tool results earn a stderr note.

## [0.1.0] - 2026-10-13

Initial release: stdio MCP server generating tools from the live Vivido/Vivida `hello`
handshake, with observe/input/process tiers, screenshots as image content, event polling via
`drain_events`, headless session lifecycle, and workflow prompts.

[Unreleased]: https://github.com/johnproblems/vivido-mcp/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/johnproblems/vivido-mcp/releases/tag/v0.1.0
