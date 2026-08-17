# Codex Prompt

You are implementing **CloudDesk-OS**, a lightweight multi-user web desktop for Linux servers.

Before coding, read these files in order:

1. `MISSION.md`
2. `GOAL.md`
3. `ARCHITECTURE.md`
4. `PLAN.md`

Treat `ARCHITECTURE.md` security rules as mandatory.

Use the planned stack: a lightweight Svelte + TypeScript frontend and a Rust/Tokio/Axum core with SQLite. The main service must never run as root. Privileged Linux operations must go through the narrow `cloudesk-privd` design. Preserve Linux UID/GID, permissions, ownership, and ACL behavior.

If the repository is new, begin with **Phase 0** of `PLAN.md`; otherwise identify the first incomplete phase and continue from there. Do not try to implement all of v1.0 in one change.

Requirements that must remain true:

- default port is `9870`;
- Desktop mode is default, Dashboard mode is optional;
- Browser, Code, and Office are optional v1.0 runtimes that can be enabled/disabled;
- secrets are encrypted at rest;
- remote-to-remote transfers never use the user's browser as the data path;
- Debian, Ubuntu, RHEL, Fedora, Rocky, AlmaLinux, Arch, and Alpine are official targets;
- all authorization is enforced server-side;
- security-relevant actions are audited.

For each implementation step:

- keep modules small and testable;
- add migrations when schema changes;
- add unit/integration/security tests;
- avoid unnecessary dependencies and resident services;
- update documentation when behavior changes;
- keep the project runnable.

At the end of each task, report: changed files, tests run, known limitations, and the next recommended PLAN.md item.
