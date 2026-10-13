# AGENTS.md — vivido-mcp contributor notes

## Shape

Single Rust crate. The binary (`src/main.rs`) is a thin CLI over `src/lib.rs` so integration
tests drive the same `Server` the MCP loop uses. No async runtime: the MCP loop issues one tool
call at a time, so the IPC client is strictly sequential over one long-lived connection (plus one
for the `drain_events` subscription).

## Conventions

- Hand-rolled MCP over stdio, like `vvagent mcp`. No MCP SDK dependency.
- Small dependency footprint: serde, clap, sha2/hex, base64, `image` (png only), libc
  (unix), interprocess (windows). Justify additions.
- Tools are generated from the live `hello`, never from a hand-kept list. The only baked-in
  method table is the disconnected-boot fallback in `tools.rs` — keep it in sync with upstream
  `METHODS` + `method_class`, and keep the mock's copy in `tests/common/mod.rs` independent.
- Wire params must match `vivido msg` exactly (nested `target`, wait `common`, numeric Vivid
  conditions). `tests/tiers.rs::wire_shapes_match_the_upstream_contract` pins samples; extend it
  when touching `translate`.
- The mutating flag wins over the class: nothing mutating lands in `observe`. Tier checks run
  before argument validation so refusals never leak behavior.
- Nothing endpoint-shaped reaches tool output. Funnel results through `Server::call`, which
  redacts centrally, and register every new socket path with the `Redactor`.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must all be clean
  (`make check`). Cross-check Windows and macOS with
  `cargo check --target x86_64-pc-windows-msvc` / `--target aarch64-apple-darwin` when touching
  `cfg`-gated code.
