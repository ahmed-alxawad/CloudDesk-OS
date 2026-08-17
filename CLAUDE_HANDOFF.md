# CloudDesk-OS v1.0.0 Claude Disaster/Nightmare Handoff

## Release Under Test

```text
Product:        CloudDesk-OS
Version:        v1.0.0
Release commit: 9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2
Immutable tag:  v1.0.0
Audit branch:   audit/claude-nightmare-v1.0.0
```

Claude must test the behavior represented by the immutable `v1.0.0` release.
Any regression tests or fixes belong only on the audit branch.

---

## Mission

You are the final adversarial release-assurance agent.

Do NOT add features. Do NOT polish working code. Do NOT redesign CloudDesk.

Your job:

```
BREAK IT      CORRUPT IT     INTERRUPT IT    RACE IT
STARVE IT     CRASH IT       SEND HOSTILE INPUT
SIMULATE BAD NETWORKS        SIMULATE BAD DISKS
SIMULATE BAD REMOTES         TEST SECURITY BOUNDARIES
TEST RECOVERY
```

Find bugs that unit, integration, acceptance, and release testing failed to detect.

---

## Token Discipline

Running with low-effort / token-efficient config.

- Do NOT reread the entire repository.
- Use this handoff, git history/diff, targeted searches, existing tests and reports.
- Open spec sections only when relevant to the subsystem under attack.
- Avoid narrating routine tool calls.
- Execute tests; do not explain what you intend to test.

---

## Source of Truth

Read only when required for the subsystem under attack:

```
Architecture/CloudDesk-OS-spec/MISSION.md
Architecture/CloudDesk-OS-spec/GOAL.md
Architecture/CloudDesk-OS-spec/ARCHITECTURE.md
Architecture/CloudDesk-OS-spec/PLAN.md

FINAL_COMPLETION_AUDIT.md
PRODUCTION_READINESS.md
LIVE_ACCEPTANCE_REPORT.md
RELEASE_VALIDATION.md
PERFORMANCE.md
V1_FINAL_CLOSURE.md
docs/SECURITY.md
docs/BACKUP_RESTORE.md
docs/DEPLOYMENT.md
```

---

## Known Good Baseline

Previously passed — **do not blindly trust these; your job is to attack them**:

**Rust gates:**
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

**Frontend gates:**
```bash
cd apps/web && npm run lint && npm run check && npm test && npm run build
```

**Live acceptance** exercised real disposable infrastructure for:
- OpenSSH (password, RSA, Ed25519, encrypted key, keyboard-interactive, ProxyJump)
- SFTP (list, upload, download, rename, mkdir, delete, large streamed file)
- WebDAV (browse, GET, PUT, MKCOL, MOVE, DELETE)
- MinIO/S3 (list, upload, download, multipart >5 MB, copy, delete)
- Cross-provider transfers with SHA-256 hash verification

---

## Critical Security Invariants

A Nightmare test finds a serious defect if **any** of these can be violated.

### Privilege Separation
```
clouddeskd MUST NOT run permanently as root (UID 0).
cloudesk-privd must only expose typed privileged operations.
No arbitrary root-command API must exist.
Web users must never select arbitrary UID/GID execution.
```

### Linux Filesystem Security
Users must not escape their assigned roots. CloudDesk must enforce:
`UID / GID / ownership / mode bits / ACLs / assigned roots`

### Vault Envelope Encryption
```
KEK → wraps → per-record DEK → encrypts → secret
```
Secrets and SSH keys must never be stored plaintext. Tampering must fail closed.

### SSH Host Verification
Host-key mismatch must be rejected. Silent acceptance of unexpected key replacement is a critical defect.

### Transfers
Remote-to-remote data must never transit the user's browser.
Large transfers must use bounded memory (no full-file buffering).

### Optional Runtimes (Brave / Code / Office / FFmpeg)
When disabled, must stop and release resources.
Must not bypass CloudDesk authorization.

---

## Disaster/Nightmare Priority Targets

Attack in roughly this order.

### Authentication (1–10)
1. Concurrent login attempts
2. Brute force (rate limiting verification)
3. Session fixation
4. Stolen/replayed session token
5. Revoked-session reuse
6. Expired-session reuse
7. TOTP replay attack
8. Recovery-code reuse
9. Step-up challenge replay
10. Clock manipulation / TOTP window abuse

### Authorization (11–15)
11. User A reading User B data
12. User A modifying User B data
13. Guest reaching Manager/Admin endpoints directly
14. Stale permissions after role change (no re-login)
15. Direct API call bypassing hidden UI controls

### Privilege Helper (16–25)
16. Malformed IPC message
17. Forged grant (crafted grant ID)
18. Expired grant reuse
19. Replayed grant
20. Grant mutation mid-flight
21. Arbitrary UID request
22. Arbitrary GID request
23. Command injection in service name
24. Environment-variable injection
25. Unix socket permission attacks

### Files / VFS (26–36)
26. `../` path traversal
27. URL/percent-encoded traversal (`%2e%2e%2f`)
28. Symlink escape
29. TOCTOU symlink swap
30. Hardlink surprises
31. Malicious filenames (null bytes, newlines, RTL override)
32. Permission race on concurrent access
33. ACL bypass
34. Archive Zip Slip
35. Huge directory (millions of entries)
36. Millions of tiny files (inode exhaustion)

### Vault (37–45)
37. Ciphertext corruption
38. Nonce corruption / nonce reuse
39. Wrapped-DEK corruption
40. AAD owner field mutation
41. Record-ID mutation (cross-record decryption)
42. Wrong KEK applied
43. KEK rotation interrupted mid-flight
44. Simultaneous secret rotations
45. Delete + recovery attempt

### SQLite (46–51)
46. Locked DB (concurrent access from external process)
47. Concurrent writers
48. Corrupt DB file
49. Disk full during transaction
50. Interrupted schema migration
51. Abrupt kill (SIGKILL) during sensitive update

### SSH (52–60)
52. Hostile SSH server (wrong host key, bad algorithms)
53. Host-key replacement — must reject silently
54. Broken ProxyJump hop
55. Bastion dies mid-session
56. Authentication timeout
57. SSH agent unavailable / corrupted
58. Malformed keyboard-interactive prompts
59. Encrypted key wrong passphrase
60. Connection storm (many simultaneous SSH sessions)

### SFTP (61–65)
61. Connection loss mid-upload
62. Connection loss mid-download
63. Destination disappears during transfer
64. Permission changes mid-transfer
65. Truncated remote file

### WebDAV (66–71)
66. Partial PUT (connection severed mid-upload)
67. Malformed PROPFIND
68. Redirect loops / bad redirects
69. Hostile XML (billion laughs, XXE)
70. MOVE failure (target locked)
71. Connection loss mid-operation

### S3 / Object Storage (72–78)
72. Failed multipart part upload
73. Multipart completion failure
74. Orphaned multipart cleanup verification
75. Wrong credentials injected mid-job
76. Endpoint outage during transfer
77. Bad ETag / checksum mismatch
78. Object overwritten concurrently

### Transfers (79–89)
79. Kill `clouddeskd` halfway through transfer
80. Restart transfer worker
81. Full server restart mid-transfer
82. Browser closes / WebSocket drops
83. Source disappears mid-transfer
84. Destination disk fills up
85. Concurrent jobs to same destination
86. Cancel at completion boundary (race)
87. Pause/restart race
88. Retry storm
89. Transfer history corruption

### HTTP / Media (90–99)
90. Malformed `Range` header
91. Huge range request
92. Overlapping / unsatisfiable ranges
93. Zero-byte file streaming
94. Malformed / truncated video
95. FFmpeg hang / infinite loop
96. Decompression bomb / media bomb
97. Malicious SVG (script injection)
98. Malformed PDF
99. Huge image dimensions (width × height overflow)

### Terminal (100–105)
100. WebSocket origin attack
101. Invalid terminal resize (negative dimensions, max int)
102. Binary garbage in PTY stream
103. Rapid connect/disconnect cycle
104. Terminal process never exits (zombie)
105. Terminal process attempts UID escape

### Optional Runtimes (106–114)
106. Brave crash → cleanup verification
107. Code crash → cleanup verification
108. Office crash → cleanup verification
109. FFmpeg crash → cleanup verification
110. Runtime disabled while active session running
111. Runtime restarted while user session active
112. User A runtime data readable by User B
113. Guest Brave profile persistence across sessions
114. Host filesystem escape attempt from runtime

### Host Administration (115–120)
115. Simultaneous power/service requests
116. Attempt to modify protected CloudDesk service via admin API
117. Malformed service name (injection)
118. Package-manager failure mid-install
119. Firewall-rule failure halfway applied
120. Reboot during active transfer

### Resource Exhaustion (121–128)
121. Memory pressure (OOM trigger)
122. Disk full
123. File-descriptor exhaustion
124. Process exhaustion (`ulimit`)
125. Connection flood
126. Extremely slow clients (Slowloris-style)
127. Transfer queue flood
128. Audit-log flood

### Installer / Recovery (129–135)
129. Installer killed halfway
130. Installer rerun on existing installation
131. Upgrade interrupted mid-migration
132. Master key missing at startup
133. Backup with missing key
134. Restore with wrong key
135. Filesystem permissions altered after restore

---

## Test Safety

All destructive tests MUST use:

```
temporary directories    test databases    containers / VMs
test SSH servers         test MinIO        test WebDAV servers
temporary CloudDesk installations
```

NEVER intentionally corrupt:
- Owner's actual home directory or real production data
- Git repository metadata
- Real SSH credentials or real Vault keys
- Other repositories

---

## Git Rules

```
v1.0.0  →  IMMUTABLE. Do NOT move, recreate, delete, or force-update.
```

All work lives only on: `audit/claude-nightmare-v1.0.0`

Do not push. Do not publish. Do not deploy. Do not sign releases.

---

## Bug Handling

For every genuine defect:
1. Reproduce it reliably.
2. Classify severity.
3. Write a regression test.
4. Make the smallest safe fix.
5. Run regression test → run related subsystem tests → check for regressions.
6. Document it.

Do not fix a bug you cannot reproduce unless the defect is statically undeniable.

---

## Report

Create `CLAUDE_NIGHTMARE_REPORT.md`. For every finding:

```
ID:
Severity:           CRITICAL | HIGH | MEDIUM | LOW | INFORMATIONAL
Subsystem:
Release affected:   v1.0.0
Reproduction:
Expected:
Actual:
Security impact:
Data-loss impact:
Availability impact:
Root cause:
Fix:
Regression test:
Retest:
```

### Final Verdict

Unresolved CRITICAL or HIGH → `NIGHTMARE TEST: FAIL`
All CRITICAL/HIGH fixed and regression-tested, no additional blockers → `NIGHTMARE TEST: PASS`

Do not claim PASS just because ordinary tests pass.

---

## Installed Disaster/Nightmare Commands

Claude Code `2.1.178` is installed. Inspection of `~/.claude/skills/` and `~/.claude/plugins/` found:

- **Installed skill:** `/graphify` (knowledge graph)
- No `/disaster` command detected.
- No `/nightmare` command detected.

```
Disaster command: VERIFY INTERACTIVELY WITH `/`
Nightmare command: VERIFY INTERACTIVELY WITH `/`
```

Use the `/` menu inside an interactive Claude Code session for the authoritative command list.
