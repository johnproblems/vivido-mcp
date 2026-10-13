## What

<!-- One paragraph: the behavior change, not the diff walk. -->

## Tier / wire impact

<!-- Does this add, move, or re-tier a tool? Change wire params? If yes, name the tools. -->

- [ ] No tool surface change, or the change is covered below.

## Verification

<!-- Focused tests first, then gates. Paste commands, not just "tests pass". -->

- [ ] New/updated maintained tests: `cargo test …`
- [ ] `make check` (fmt, clippy `-D warnings`, full suite)
- [ ] Cross-checks if `cfg`-gated code changed: Windows + macOS targets
- [ ] `CHANGELOG.md` updated under Unreleased

## Security checklist

- [ ] No socket path, token, or endpoint material reaches tool output (redactor covers new paths).
- [ ] Nothing mutating lands in the observe tier; refusals still name `--allow`.
- [ ] No new network, listening socket, or privilege change.
