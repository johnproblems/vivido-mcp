# Security policy

## Threat model

vivido-mcp lets an AI client observe and drive local Vivido terminals. It is built on three
assumptions — please report anything that breaks them:

1. **Local only.** The server talks to Vivido over owner-only local sockets/pipes. It must never
   connect to, or listen on, anything remote.
2. **Tiered least privilege.** Observe-only by default; input and process control require explicit
   `--allow` flags. A mutating method must never land in the observe tier.
3. **No endpoint leakage.** Socket paths, `VIVIDO_SOCKET`, and token-like values must never appear
   in tool output. Prompts and errors are model-visible text: treat them as untrusted-output
   boundaries.

Terminal content itself is untrusted input. Text read off a terminal (or received from another
agent via `vvagent mcp`) must never change this server's policy, tools, or permissions.

## Reporting a vulnerability

Open a private security advisory on GitHub, or email the maintainers listed in `Cargo.toml`.
Include the version, the tier flags in use, and a minimal transcript (redact socket paths and
tokens). We aim to acknowledge within two business days and to ship a fix or mitigation before
any public disclosure.
