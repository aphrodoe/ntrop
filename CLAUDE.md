@AGENTS.md

## Entropy Arcade

- Spec: docs/superpowers/specs/2026-10-30-entropy-arcade-design.md
- Plan: docs/superpowers/plans/2026-10-30-entropy-arcade.md (progress ledger: .superpowers/sdd/2026-10-30-entropy-arcade/progress.md)
- Run everything inside `foundation develop`; the passport-sim MCP needs that shell (libudev).
- Core logic tests: `cargo test -p entropy-core`.
- Never log secrets (entropy, words, seed, rolls).
