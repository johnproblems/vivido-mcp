# Changelog

All notable changes to vivido-mcp, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.html).

## [Unreleased]

## [0.2.0] - 2026-10-15

### Added

- `--allow read-only` alias for the default observe tier, and closest-match
  suggestions for unknown `--allow` values.
- `VIVIDO_MCP_MAX_IMAGE_DIM` environment fallback for `--max-image-dim`, and
  `VIVIDO_MCP_OVERSIZE_BYTES` to tune the oversize-result stderr note.
- Screenshot metadata `encoded_bytes` (raw encoded size alongside base64 `bytes`).
- Recovery hints for `invalid_params` and `not_supported` server errors.
- `key` aliases (`Return`, `Esc`, `Del`, `Ins`, `Up`/`Down`/`Left`/`Right`,
  `Space`, `PageUp`/`PageDown`), and `SIG`-prefixed signals (`SIGINT`).
- Session context now reports the tool count alongside tiers and attachment.
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

- `capture` accepts the documented `format`/`quality` instead of rejecting them
  as unknown arguments.
- Failed screenshots no longer leak their temp PNG; `quality` is validated for
  PNG output too, and `crop` rejects unknown keys.
- `drain_events`/`subscribe` reject non-string `events` entries instead of
  silently widening to all kinds.
- `headless_start` rejects mistyped `session`/`size`/`program`/`ephemeral`,
  `args` without `program`, and non-string args entries instead of silently
  defaulting; `headless_stop` validates `force` the same way.
- `set_geometry_batch` rejects unknown keys inside items.
- Mouse `path` `duration` out of range reports its own 1ms–30s range (including 0).
- `config` distinguishes "needs `options` or `reset`" from "not both".
- CI: unix-only test module no longer breaks the Windows clippy gate.
- CI: real-headless job installs pinned `vivido` from crates.io instead of building
  git HEAD, whose sibling path dependencies lag behind.

### Changed

- Unknown `--allow` values, tools, arguments, and MCP methods suggest the closest
  match ("did you mean …?"); tool suggestions fall back to prefix matches.
- Tier refusals name the `VIVIDO_MCP_ALLOW` alternative for flag-awkward clients.
- Durations accept uppercase suffixes and surrounding whitespace; mouse
  buttons/actions, `set_level` dashes, `vivid_target`, trace categories, and Vivid
  wait conditions match case-insensitively; `drop_file.at` accepts `{col,row}`.
- `drop_file` on a directory, `exec` with NUL bytes, non-boolean `capture`
  flags, mistyped `window_id`, and non-string `headless_attach`/`headless_stop`
  targets fail fast with precise `invalid_args`.
- Passthrough tools reject non-object arguments; `prompts/get` rejects unknown
  and non-object arguments.
- Schemas document the accepted coercions: single-string `events`, dashed
  `double-click`, and `key` aliases.
- MCP: frames without `"jsonrpc": "2.0"` are `-32600`; non-object `arguments`
  are `-32602`; parse errors carry a `line_preview`; protocol-version skew logs
  a warning.
- An explicit `--socket` that refuses connections fails fast at resolve; a stale
  inherited `VIVIDO_SOCKET` logs a fallback note; registries with empty sockets
  are skipped.
- Empty `--socket`/`--vivido-bin` fail fast; `--help` documents every env var;
  headless sizes cap at 16384 per dimension.
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

[Unreleased]: https://github.com/johnproblems/vivido-mcp/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/johnproblems/vivido-mcp/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/johnproblems/vivido-mcp/releases/tag/v0.1.0
