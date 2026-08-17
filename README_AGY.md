# Run CloudDesk-OS with Antigravity CLI

Copy `AGY_HANDOFF.md` into the root of the existing CloudDesk-OS repository.

Keep the existing specification files under:

`Architecture/CloudDesk-OS-spec/`

From the repository root, launch Antigravity CLI in automatic permission mode:

```bash
agy --dangerously-skip-permissions
```

Then submit the contents of `AGY_GOAL_PROMPT.txt` using AGY's `/goal` workflow.

Do not move/delete the existing `graphify-out/`, `.codex/`, `AGENTS.md`, specifications, or Codex-created source files before AGY inspects Git state.

