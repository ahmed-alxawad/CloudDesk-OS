---
description: Resume Codex's CloudDesk-OS work and continue through production-ready v1.0
agent: code
---

Take over CloudDesk-OS from Codex and continue implementation.

Read, in order:

1. `KILO_PROGRESS_CHECKPOINT.md`
2. `KILO_HANDOFF.md`
3. `MISSION.md`
4. `GOAL.md`
5. `ARCHITECTURE.md`
6. `PLAN.md`
7. `CODEX_PROMPT.md`

Inspect Git state and actual source before trusting the checkpoint.

Use Graphify to update/map the current project before broad exploration.

The expected partial task is the Vault conversion from direct installation-key encryption to true per-record envelope encryption. Verify this from the diff/source, finish it completely, add security tests, and run strict Rust validation.

Then DO NOT stop merely because that task passes. Re-read `PLAN.md`, select the next earliest genuinely incomplete v1.0 release item, implement it, validate it, and repeat.

Continue through the production-release gates as far as the available session/tools allow. Never restart completed work, never discard Codex changes, never weaken the security architecture, and never claim production-ready without evidence.

If an external blocker or tool/context limit prevents completion, finish independent work first and leave the exact checkpoint required by `KILO_HANDOFF.md`.
