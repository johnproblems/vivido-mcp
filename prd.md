# PRD: vivido-mcp — terminal automation as MCP tools

## Summary and verification

`vivido-mcp` is a stdio MCP server that lets any MCP client see and drive Vivido: screenshots arrive as images the model can look at directly, and actions that type or run things are gated by safety tiers.

**Verified, with a narrower gap.**

- The org's only MCP server is `vvagent mcp`, with six mailbox tools (`agent_mesh_*`) and no terminal tools.
- Shell-capable agents (Claude Code, Codex) are already well served: `vivido msg` speaks compact JSON, `run-plan` does act-then-verify, and a bundled Agent Skill teaches it all.
- What a shell can't give: `screenshot` returns a **file path**, so a sandboxed or remote client can't see the image; clients with no shell (desktop chat apps, IDE panes) have no way in; and the skill relies on one blanket shell approval rather than per-action tiers.
- No tool-side approval exists for input injection; protection today is owner-only sockets plus the host agent's shell prompt.

## Goals, non-goals, success criteria

**Goals**

- Tools generated from the live `hello` response (`methods`, `method_capabilities`, `limits`), so the server tracks Vivido versions without hand edits.
- `screenshot` and `capture` return MCP image content (PNG, optionally cropped to a cell region), and delete the temp file afterwards.
- Three tiers: `observe` (default), `input`, `process` (quit, restart, signal, create window), chosen at launch with `--allow`.
- Headless session tools: start an ephemeral session, attach, stop.
- Works against both Vivido and Vivida endpoints.

**Non-goals (v0.1)**

- Pushing events as MCP notifications; clients rarely act on them mid-turn. A `drain_events` tool replaces it.
- Remote access; local owner-only socket only.

**Success criteria**

| Criterion | Target |
| --- | --- |
| Claude Desktop (no shell) runs `ls` in a headless session and reads the result | works with `--allow input` |
| Screenshot shown to the model as an image | yes, no file path in output |
| A mutating tool under default tier | refused with a message naming `--allow` |
| Tool list matches `hello.methods` | 100% of observe methods exposed |

## Tool surface and safety

The server holds one long-lived connection to Vivido's IPC socket (newline-delimited JSON, version 2, `hello` first, up to 64 requests in flight) and maps methods to tools by their declared class.

| Tier | Tools (from `vivido msg` methods) | Default |
| --- | --- | --- |
| observe | `list_windows`, `inspect`, `get_text`, `get_grid`, `transcript`, `screenshot`, `capture`, `wait_text`, `wait_output`, `wait_screen_stable`, `wait_prompt`, `wait_command_finish`, `drain_events`, `vivid_surfaces` | on |
| input | `type_text`, `key`, `paste`, `mouse`, `drop_file` | `--allow input` |
| process | `create_window`, `close_window`, `restart_terminal`, `signal`, `quit`, headless start/stop | `--allow process` |

**Rules**

- A method `hello` marks `mutating` is never placed in `observe`, whatever its class says.
- MCP tool annotations mirror the tier (`readOnlyHint`, `destructiveHint`) so client approval UIs prompt correctly.
- The socket path, `VIVIDO_SOCKET`, and session tokens never appear in tool output.
- `drain_events` keeps a subscription open server-side and returns events since a cursor, using Vivido's `since_event` replay; an `overflow` event is passed through as a visible gap.
- Write a small client of its own rather than depending on the `vivido` crate's `host` module, which opens a new connection per call and pulls in the whole renderer.

## Build plan, testing, risks

A usable observe-only server lands by hour 5; tiers, images, and headless control fill the rest.

| Hours | Milestone |
| --- | --- |
| 0–3 | IPC client: hello, request multiplexing, error mapping |
| 3–5 | MCP stdio server; observe tools generated from `hello` |
| 5–7 | Screenshot as image content, cropping, temp-file cleanup |
| 7–9 | Input and process tiers, annotations, `--allow` |
| 9–12 | Headless lifecycle, `drain_events`, Vivida endpoints, README with client configs |

**Testing.** Integration tests against `vivido --headless --ephemeral` in CI (rendering is real offscreen); a tier test asserting every mutating method is refused by default; manual runs from Claude Desktop and one IDE client.

**Risks**

| Risk | Mitigation |
| --- | --- |
| An agent with `--allow input` can type anything into a shell | Observe-only default; recommend headless ephemeral sessions for agent work |
| IPC protocol version changes | Refuse unknown `protocol_version` with a clear message; generate tools at runtime |
| Large screenshots blow the client's context | Default downscale and cell-region cropping |
| Upstream adds its own MCP mode | Offer this as the starting point upstream |

**Open question.** Should the server also expose the existing Agent Skill text as an MCP prompt, so no-shell clients learn the same workflows?
