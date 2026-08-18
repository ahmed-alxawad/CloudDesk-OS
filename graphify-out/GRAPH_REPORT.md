# Graph Report - CloudDesk-OS  (2026-08-18)

## Corpus Check
- 178 files · ~152,859 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 2413 nodes · 7005 edges · 118 communities (99 shown, 19 thin omitted)
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 100 edges (avg confidence: 0.82)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `11506a0c`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- JSON Schema Fields
- Configuration System
- Permissions and Manifests
- Frontend Development Tooling
- Frontend Package Scripts
- Core HTTP Service
- Application Manifest Schema
- TypeScript Configuration
- Application Runtime Model
- CI and Threat Controls
- Product Specifications
- SQLite Persistence
- Core Service CLI
- Workspace Modes
- Security Invariants
- Phase Zero Delivery
- HTML Entry Point
- Svelte Web Application
- VFS and Transfers
- Product Mission
- Authorization Capabilities
- Health Security Tests
- Graphify Workflow
- Svelte Configuration
- Linux Identity
- Installer and HTTPS
- Vite Configuration
- Svelte Dependency
- Integration Test Strategy
- Security Test Strategy
- privilege/src/lib.rs
- SecretCipher
- transfers/src/lib.rs
- lookup_uid
- audit/src/lib.rs
- request
- apps.ts
- workspace.ts
- request
- cloudesk-privd/src/main.rs
- install.sh
- alpine.sh
- arch.sh
- debian.sh
- distro.sh
- fedora.sh
- rhel.sh
- root_boundary.rs
- vite.config.ts
- finish-clouddesk.md
- Integration Test Strategy
- PathBuf
- read_terminal_frame
- CloudDesk-OS v1.0 Release Validation Matrix
- CloudDesk-OS Security Architecture & Threat Model
- CloudDesk-OS v1.0 Performance & Resource Benchmark Report
- Integration Test Strategy
- Security Test Strategy
- uninstall.sh
- root_boundary.rs
- 2. Requirement-by-Requirement Inventory
- InstanceContext
- exec.rs
- compat.rs
- RuntimeManager
- Disaster/Nightmare Priority Targets
- extract_archive
- music_authorization.rs
- scan.rs
- InstanceId
- .channel_open_session
- WebDavProvider
- CloudDesk-OS — Engineering Checkpoint
- live_lifecycle.rs
- music.ts
- RuntimeKind
- read_acl
- Part 2 — Requirement-by-requirement evidence map
- media_api.rs
- SftpProvider
- SshSession
- Report
- InstanceCgroup
- VfsError
- music_api.rs
- proxy_http
- CloudDesk-OS v1.0 — True Closure: Missing Implementations
- storage.rs
- PortAllocator
- json_request
- CloudDesk-OS v1.0.0 — Disaster/Nightmare Adversarial Test Report
- CloudDesk-OS Live Acceptance Report
- test-runtime-fixture/src/main.rs
- Highlights
- live_oci.rs
- /nightmare-test
- /disaster-test
- CloudDesk-OS v1.0.0-rc.3
- kilo.json
- CloudDesk-OS v1.0 Final Readiness
- CloudDesk-OS Claude Instructions
- String
- runtime_api.rs
- SshSession
- MusicApp.svelte
- probe.rs
- scan_live.rs
- MediaProbe
- RawFormat
- mutate
- ServersApp.svelte

## God Nodes (most connected - your core abstractions)
1. `String` - 282 edges
2. `ApiError` - 153 edges
3. `AppState` - 116 edges
4. `State` - 108 edges
5. `principal()` - 95 edges
6. `VfsError` - 68 edges
7. `require_auth_service()` - 66 edges
8. `AuthService` - 63 edges
9. `RuntimeManager` - 50 edges
10. `LibraryStore` - 44 edges

## Surprising Connections (you probably didn't know these)
- `Monorepo and Process Boundaries` --semantically_similar_to--> `CloudDesk Process Model`  [INFERRED] [semantically similar]
  docs/ARCHITECTURE_DECISIONS.md → Architecture/CloudDesk-OS-spec/ARCHITECTURE.md
- `Data-Driven Applications and Permissions` --semantically_similar_to--> `Manifest-Based Application Model`  [INFERRED] [semantically similar]
  docs/ARCHITECTURE_DECISIONS.md → Architecture/CloudDesk-OS-spec/ARCHITECTURE.md
- `No Generic Root Capability` --semantically_similar_to--> `Critical Security Rules`  [INFERRED] [semantically similar]
  docs/CAPABILITIES.md → Architecture/CloudDesk-OS-spec/ARCHITECTURE.md
- `Architectural Security Invariants` --semantically_similar_to--> `Critical Security Rules`  [INFERRED] [semantically similar]
  docs/THREAT_MODEL.md → Architecture/CloudDesk-OS-spec/ARCHITECTURE.md
- `Phase 0 Development Workflow` --semantically_similar_to--> `Check and Test Workflow`  [INFERRED] [semantically similar]
  README.md → docs/DEVELOPMENT.md

## Import Cycles
- None detected.

## Hyperedges (group relationships)
- **Phase 0 Delivery Contract** — architecture_clouddesk_os_spec_plan_phase_0_foundation, readme_phase_0_status, docs_architecture_decisions_phase_0_architecture_decisions, docs_threat_model_phase_0_security_limitations, tests_integration_readme_phase_0_endpoint_tests [INFERRED 0.95]
- **Server-Side Authorization Chain** — architecture_clouddesk_os_spec_architecture_critical_security_rules, docs_app_manifest_enabled_is_not_authorization, docs_capabilities_backend_authorization_identifiers, docs_development_security_review_checklist, docs_threat_model_architectural_invariants, tests_security_readme_security_boundary_tests [INFERRED 0.95]
- **Optional Runtime Contract** — architecture_clouddesk_os_spec_goal_optional_heavy_applications, architecture_clouddesk_os_spec_architecture_optional_runtime_services, docs_app_manifest_optional_runtime_lifecycle, docs_threat_model_architectural_invariants [INFERRED 0.95]

## Communities (118 total, 19 thin omitted)

### Community 0 - "JSON Schema Fields"
Cohesion: 0.06
Nodes (217): Bytes, ConnectInfo, Into, InvalidHeaderValue, Json, Next, Request, add_assigned_root() (+209 more)

### Community 1 - "Configuration System"
Cohesion: 0.08
Nodes (56): AssignedRootAccess, audit_event(), AuthError, AuthPolicy, AuthService, backoff_seconds(), bootstrap_login_authorize_and_revoke(), BootstrapConfiguration (+48 more)

### Community 2 - "Permissions and Manifests"
Cohesion: 0.08
Nodes (55): aad_context_tampering_detected(), ciphertext_tampering_detected(), load_rejects_wrong_length_key(), load_round_trips_through_file(), nonce_tampering_detected(), plaintext_never_appears_in_ciphertext(), round_trip_requires_the_same_context(), Arc (+47 more)

### Community 3 - "Frontend Development Tooling"
Cohesion: 0.06
Nodes (63): action(), GrantClaims, GrantError, GrantSigner, PowerOperation, PrivdRequest, PrivdResponse, PrivilegedAction (+55 more)

### Community 4 - "Frontend Package Scripts"
Cohesion: 0.13
Nodes (6): availableApplications(), isAvailable(), active, ./lib/apps, ./video, ./lib/workspace

### Community 5 - "Core HTTP Service"
Cohesion: 0.12
Nodes (27): canonical_virtual(), entry_from_metadata(), EntryKind, execute_local(), join_virtual(), LocalFileOperation, LocalFileResult, LocalProvider (+19 more)

### Community 6 - "Application Manifest Schema"
Cohesion: 0.05
Nodes (41): enabled, file_associations, icon, id, name, required_permissions, route, runtime_dependency (+33 more)

### Community 7 - "TypeScript Configuration"
Cohesion: 0.09
Nodes (61): AccessMode, assigned_roots_reject_traversal_and_symlink_escape(), AssignedRoot, LinuxError, LinuxIdentity, lookup_uid(), lookup_user(), read_only_roots_reject_write_authorization() (+53 more)

### Community 8 - "Application Runtime Model"
Cohesion: 0.11
Nodes (34): concurrent_claims_never_claim_one_job_twice(), ensure_updated(), jobs_persist_and_interrupted_work_recovers_after_restart(), local_file_transfer_copies_bytes_and_calculates_checksum(), NewTransfer, now(), parse_state(), parse_strategy() (+26 more)

### Community 9 - "CI and Threat Controls"
Cohesion: 0.05
Nodes (36): dependencies, svelte, @xterm/addon-fit, @xterm/xterm, devDependencies, prettier, prettier-plugin-svelte, svelte-check (+28 more)

### Community 10 - "Product Specifications"
Cohesion: 0.09
Nodes (51): auth_method_name(), host_key_fingerprint(), input(), known_hosts_line(), NewRemoteServer, normalized_tags(), now(), parse_auth_method() (+43 more)

### Community 11 - "SQLite Persistence"
Cohesion: 0.07
Nodes (28): Brave runtime, Code runtime, Continue automatically after the Vault, Core security, Current partial item, Dependency safety, Distribution support, End-of-session checkpoint (+20 more)

### Community 12 - "Core Service CLI"
Cohesion: 0.14
Nodes (19): Config, ConfigError, DatabaseConfig, defaults_to_the_architecture_listener(), MediaConfig, PrivilegeConfig, rejects_unknown_configuration_keys(), AsRef (+11 more)

### Community 13 - "Workspace Modes"
Cohesion: 0.23
Nodes (22): append(), append_in_transaction(), audit_rows_cannot_be_updated_or_deleted(), AuditError, AuditReceipt, canonical_json(), concurrent_writers_produce_one_linear_chain(), database() (+14 more)

### Community 14 - "Security Invariants"
Cohesion: 0.11
Nodes (17): Authentication / authorization / audit, CloudDesk-OS — Codex Progress Checkpoint, CloudDesk shell, Do not assume these are complete, Expected first continuation task, Installer / HTTPS, Known Codex stopping point, Known files created/modified by Codex (+9 more)

### Community 15 - "Phase Zero Delivery"
Cohesion: 0.15
Nodes (12): 1. Critical Encryption Material Warning, 2. Backup Scope, 3. Creating a Backup, 4. Disaster Recovery / Restore Procedure, 5. Master Key Rotation, CloudDesk-OS Backup & Disaster Recovery Guide, Step 1: Perform an Online SQLite Backup, Step 1: Stop Running Services (+4 more)

### Community 16 - "HTML Entry Point"
Cohesion: 0.19
Nodes (10): AppManifest, is_identifier(), ManifestError, Error, Option, Result, Self, Vec (+2 more)

### Community 17 - "Svelte Web Application"
Cohesion: 0.14
Nodes (13): compilerOptions, allowJs, checkJs, isolatedModules, moduleResolution, noEmit, strict, extends (+5 more)

### Community 18 - "VFS and Transfers"
Cohesion: 0.15
Nodes (14): Manifest-Based Application Model, Optional Runtime Services, CloudDesk Process Model, Optional Heavy Applications, Application Manifest Contract, Enabled Manifest Is Not Authorization, Manifest Schema and Required Fields, Optional Runtime Lifecycle Contract (+6 more)

### Community 19 - "Product Mission"
Cohesion: 0.18
Nodes (13): CloudDesk CI Workflow, Rust CI Job, Web CI Job, Check and Test Workflow, Development Standards, Module Boundaries, Security Review Checklist, CloudDesk-OS Threat Model (+5 more)

### Community 20 - "Authorization Capabilities"
Cohesion: 0.33
Nodes (9): CloudDesk-OS Architecture, Codex Implementation Prompt, First Incomplete Phase Rule, Task Completion Report Contract, CloudDesk-OS Goals, CloudDesk-OS Mission, Incremental Delivery Rule, CloudDesk-OS v1.0 Implementation Plan (+1 more)

### Community 21 - "Health Security Tests"
Cohesion: 0.29
Nodes (9): application(), bootstrap_login_authorization_and_logout_are_enforced_server_side(), guest_role_cannot_read_system_summary(), request(), Body, Method, Option, Router (+1 more)

### Community 22 - "Graphify Workflow"
Cohesion: 0.38
Nodes (9): Cli, Command, main(), migrate(), Command, PathBuf, Result, serve() (+1 more)

### Community 23 - "Svelte Configuration"
Cohesion: 0.39
Nodes (7): connect(), migrate(), migrations_create_the_expected_baseline_and_shell_tables(), Error, Result, SqlitePool, MigrateError

### Community 24 - "Linux Identity"
Cohesion: 0.29
Nodes (7): Cli, Command, main(), Command, Option, PathBuf, Result

### Community 25 - "Installer and HTTPS"
Cohesion: 0.29
Nodes (4): AppDefinition, applications, RawManifest, RuntimeDependency

### Community 26 - "Vite Configuration"
Cohesion: 0.38
Nodes (5): clampWindow(), DEFAULT_PREFERENCES, defaultWindow(), WindowLayout, WorkspacePreferences

### Community 27 - "Svelte Dependency"
Cohesion: 0.33
Nodes (6): mapped_worker_grants_are_scoped_signed_and_audited(), request(), Body, Method, Option, Value

### Community 28 - "Integration Test Strategy"
Cohesion: 0.50
Nodes (3): normalizeMode(), WORKSPACE_MODES, WorkspaceMode

### Community 29 - "Security Test Strategy"
Cohesion: 0.40
Nodes (5): Critical Security Rules, Least Privilege, Security Foundations Before Browser Runtime, No Generic Root Capability, Architectural Security Invariants

### Community 30 - "privilege/src/lib.rs"
Cohesion: 0.40
Nodes (4): Cli, main(), PathBuf, Result

### Community 31 - "SecretCipher"
Cohesion: 0.50
Nodes (4): Phase 0 Repository Standards and Threat Model, Phase 0 Security Limitations, Current Phase 0 Status, Phase 0 Endpoint Integration Tests

### Community 40 - "cloudesk-privd/src/main.rs"
Cohesion: 0.67
Nodes (3): Application Mount Point, Frontend main.ts Module Reference, Web Shell HTML Entry Document

### Community 41 - "install.sh"
Cohesion: 0.67
Nodes (3): Virtual Filesystem Provider Abstraction, Persistent Server-to-Server Transfers, Transfers Belong to the Server

### Community 42 - "alpine.sh"
Cohesion: 0.67
Nodes (3): Secure Resource-Efficient Multi-User Web Desktop, Browser-Native Linux Workspace, Official Distribution Test Matrix

### Community 43 - "arch.sh"
Cohesion: 0.67
Nodes (3): Stable Backend Authorization Identifiers, Capability Change Contract, Capability Registry

### Community 51 - "vite.config.ts"
Cohesion: 0.18
Nodes (10): 1. Direct HTTPS Deployment (Default), 2. Reverse Proxy Deployment, 3. SELinux & AppArmor Hardening, 4. Service Supervision, A. Caddy Example (`/etc/caddy/Caddyfile`), AppArmor (Debian / Ubuntu), B. Nginx Example (`/etc/nginx/sites-available/clouddesk`), CloudDesk-OS Production Deployment Guide (+2 more)

### Community 52 - "finish-clouddesk.md"
Cohesion: 0.11
Nodes (17): AGY Handoff — Finish CloudDesk-OS v1.0, CloudDesk architectural invariants, Codex's last known stopping point, Continue after the Vault — do not stop, Definition of done, Dependency policy, Existing Graphify data, External blockers (+9 more)

### Community 56 - "PathBuf"
Cohesion: 0.06
Nodes (53): BinaryInfo, detect(), FfmpegAvailability, missing_binary_is_unavailable_not_an_error(), probe_binary(), Option, JobOperation, JobState (+45 more)

### Community 57 - "read_terminal_frame"
Cohesion: 0.18
Nodes (10): 1. Executive Summary, 2. Build & Test Verification, 3. Supported Operating System Matrix, 4. Security Verification, 5. Performance & Resource Footprint, 6. Installation, Upgrade, Backup & Recovery, 7. Known Issues & Minor Limitations, 8. External Release Blockers (Requires Project Owner Action) (+2 more)

### Community 58 - "CloudDesk-OS v1.0 Release Validation Matrix"
Cohesion: 0.22
Nodes (8): 1. Core Toolchain & Code Quality Gates, 2. Security Architecture & Threat Model, 3. Storage, File Manager & Background Transfer Engine, 4. Web Desktop & Application Shell, 5. Linux Distribution Compatibility Matrix, 6. Installation & Upgrade Lifecycle, 7. Performance & Resource Footprint, CloudDesk-OS v1.0 Release Validation Matrix

### Community 59 - "CloudDesk-OS Security Architecture & Threat Model"
Cohesion: 0.29
Nodes (6): 1. Privilege Separation, 2. Vault Per-Record Envelope Encryption, 3. Sandboxed Virtual Filesystem (VFS), 4. Web & Session Security, 5. Security Vulnerability Reporting, CloudDesk-OS Security Architecture & Threat Model

### Community 60 - "CloudDesk-OS v1.0 Performance & Resource Benchmark Report"
Cohesion: 0.29
Nodes (6): 1. System Resource Targets & Measured Footprint, 2. Frontend Production Bundle Breakdown, 3. Storage & Background Transfer Engine Throughput, 4. Optional Heavy Runtime Footprints (When Enabled), CloudDesk-OS v1.0 Performance & Resource Benchmark Report, Summary

### Community 64 - "root_boundary.rs"
Cohesion: 0.12
Nodes (17): escape_like(), LibraryRoot, LibraryStore, now(), Playlist, row_to_track(), Error, HashMap (+9 more)

### Community 65 - "2. Requirement-by-Requirement Inventory"
Cohesion: 0.33
Nodes (5): 1. Core Platform (15/15 — 100.00%), 2. Applications (8/12 — 66.67%), 3. Remote Infrastructure (6/15 — 40.00%), 4. Production Readiness (16/16 — 100.00%), CloudDesk-OS v1.0 Final Completion Audit

### Community 66 - "InstanceContext"
Cohesion: 0.07
Nodes (44): ArgvBuilder, AsyncRead, AdapterError, Availability, HealthStatus, InstanceContext, Child, Option (+36 more)

### Community 67 - "exec.rs"
Cohesion: 0.09
Nodes (45): ChildStderr, cleanup_workspace(), drain_stderr_bounded(), ExecError, extract_artwork(), extract_subtitle(), job_workspace(), JobLimiter (+37 more)

### Community 68 - "compat.rs"
Cohesion: 0.08
Nodes (24): BTreeMap, audio(), container_is(), decide(), StreamPlan, video(), format_level_tags_are_captured_for_music_metadata(), huge_declared_dimensions_do_not_overflow_parsing() (+16 more)

### Community 69 - "RuntimeManager"
Cohesion: 0.15
Nodes (16): Send, Sync, RuntimeAdapter, key_of(), Arc, Error, Option, Result (+8 more)

### Community 70 - "Disaster/Nightmare Priority Targets"
Cohesion: 0.05
Nodes (37): Authentication (1–10), Authorization (11–15), Bug Handling, CloudDesk-OS v1.0.0 Claude Disaster/Nightmare Handoff, Critical Security Invariants, Disaster/Nightmare Priority Targets, Files / VFS (26–36), Final Verdict (+29 more)

### Community 71 - "extract_archive"
Cohesion: 0.15
Nodes (35): ArchiveFormat, ArchiveOutcome, cleanup_partial_extraction(), copy_with_quota(), create_archive(), create_tar_gz(), create_zip(), EntryKind (+27 more)

### Community 72 - "music_authorization.rs"
Cohesion: 0.23
Nodes (29): a_library_row_is_not_permanent_authorization(), administrator_does_not_bypass_ownership_scoping_on_another_users_rows(), application_with_music(), authorization_outcome_is_identical_regardless_of_which_path_issued_the_request(), authorized_owner_can_perform_every_operation_denied_to_others(), body_json(), bootstrap_admin(), create_user() (+21 more)

### Community 73 - "scan.rs"
Cohesion: 0.11
Nodes (24): collect_candidates(), ffprobe_binary(), fingerprint(), has_audio_extension(), metadata_from_probe(), parse_leading_int(), Error, Instant (+16 more)

### Community 74 - "InstanceId"
Cohesion: 0.20
Nodes (12): Persistence, InstanceRow, now(), row_to_instance(), Error, Option, Result, Self (+4 more)

### Community 75 - ".channel_open_session"
Cohesion: 0.17
Nodes (21): ChannelOpenHandleInner, MockSshServer, Auth, Channel, ChannelId, Error, Handler, Msg (+13 more)

### Community 76 - "WebDavProvider"
Cohesion: 0.19
Nodes (10): Client, Handle, Method, Option, PathBuf, Response, Result, Self (+2 more)

### Community 77 - "CloudDesk-OS — Engineering Checkpoint"
Cohesion: 0.07
Nodes (28): Actual live authentication methods verified (through the real product path), CloudDesk-OS — Engineering Checkpoint, Current commit, Current commit (Phase 5), Current phase, Historical: Phase 4's own "next phase" notes (superseded by Phase 5, now complete), Historical: Phase 4 — what was built (Video Application, preserved, unchanged), Last completed phase (+20 more)

### Community 78 - "live_lifecycle.rs"
Cohesion: 0.19
Nodes (27): environment_never_leaks_the_orchestrator_process_env(), fast_policy(), fixture_path(), manager_with(), pool(), resource_limits_are_enforced_admission_control(), Arc, HashMap (+19 more)

### Community 79 - "music.ts"
Cohesion: 0.17
Nodes (19): formatTime(), isTerminalJobState(), JobState, playbackUrl(), StreamPlan, truncateForDisplay(), hasPlayedEnoughToRecord(), insertIntoQueue() (+11 more)

### Community 80 - "RuntimeKind"
Cohesion: 0.10
Nodes (14): InstanceRuntime, LiveInstance, HashMap, Instant, Mutex, PathBuf, Generation, InstanceState (+6 more)

### Community 81 - "read_acl"
Cohesion: 0.18
Nodes (23): AclEntry, AclQualifierKind, format_entry_spec(), parse_getfacl_output(), parse_permissions(), read_acl(), resolve_real_path(), Option (+15 more)

### Community 82 - "Part 2 — Requirement-by-requirement evidence map"
Cohesion: 0.08
Nodes (23): CloudDesk-OS v1.0 — Release Evidence Audit, Evidence categories (do not inflate), G10 — Server-to-server transfers, G11 — Auth & authorization, G12 — Linux identity & permissions, G13 — Secrets, G14 — Audit trail, G15 — Server administration (+15 more)

### Community 83 - "media_api.rs"
Cohesion: 0.28
Nodes (23): a_users_media_job_is_invisible_and_uncontrollable_by_another_user(), application_with_media(), audio_track_ordinal_is_threaded_through_to_the_remux_job(), bootstrap_and_login(), current_process_linux_username(), ffmpeg_available(), generate_mkv_fixture(), generate_subtitled_fixture() (+15 more)

### Community 84 - "SftpProvider"
Cohesion: 0.22
Nodes (8): FileAttributes, Handle, PathBuf, Result, Self, SftpSession, Vec, SftpProvider

### Community 85 - "SshSession"
Cohesion: 0.08
Nodes (32): Attrs, ChannelOpenHandle, connect(), list_root_succeeds_against_a_non_chrooted_sftp_server(), MockServer, MockSession, NonChrootSftp, Arc (+24 more)

### Community 86 - "Report"
Cohesion: 0.32
Nodes (15): base64_of(), main(), Report, Result, Self, T, run_missing_applications(), run_s3() (+7 more)

### Community 87 - "InstanceCgroup"
Cohesion: 0.18
Nodes (13): CgroupError, CgroupSupport, detect(), detect_never_panics_and_reports_a_real_answer(), InstanceCgroup, own_cgroup_path(), Drop, Error (+5 more)

### Community 88 - "VfsError"
Cohesion: 0.26
Nodes (8): Client, Handle, Result, Self, Vec, S3Provider, VfsError, SdkConfig

### Community 89 - "music_api.rs"
Cohesion: 0.24
Nodes (20): application_with_music(), artwork_extraction_and_sidecar_fallback(), body_json(), bootstrap_and_login(), current_process_linux_username(), ffmpeg_available(), full_library_lifecycle(), generate_track() (+12 more)

### Community 90 - "proxy_http"
Cohesion: 0.17
Nodes (18): proxy_http(), proxy_ws(), ProxyError, resolve_upstream(), HeaderMap, IntoResponse, Method, Response (+10 more)

### Community 91 - "CloudDesk-OS v1.0 — True Closure: Missing Implementations"
Cohesion: 0.11
Nodes (18): 10. SCP transfers, 11. SSH agent forwarding, 12. Keyboard-interactive authentication, 13. SSH certificate authentication, 14. ProxyJump / bastion hosts (product wiring) — **CLOSED (for transfers/SFTP; remote terminal not yet wired — see #16)**, 15. Real distro-matrix installer/service verification, 16. Remote terminal over SSH (new item, discovered during Phase 2), 1. FFmpeg probing / remux / transcoding — **CLOSED** (Phase 3, this session) (+10 more)

### Community 92 - "storage.rs"
Cohesion: 0.27
Nodes (14): create_dir_symlink_safe(), creates_the_expected_layout_with_restrictive_permissions(), id(), instance_state_dir(), is_safe_segment(), kind_dir_name(), refuses_to_follow_a_preplanted_symlink(), remove_instance_state_dir() (+6 more)

### Community 93 - "PortAllocator"
Cohesion: 0.24
Nodes (9): allocated_port_is_bound_only_to_loopback(), allocates_distinct_ports_and_releases_them(), PortAllocator, PortError, ReservedPort, HashSet, Mutex, Result (+1 more)

### Community 94 - "json_request"
Cohesion: 0.33
Nodes (14): application(), bootstrap_and_login(), current_process_linux_username(), json_request(), request(), resumable_upload_rejects_checksum_mismatch(), resumable_upload_round_trips_across_multiple_chunks(), resumable_upload_session_is_isolated_per_user() (+6 more)

### Community 95 - "CloudDesk-OS v1.0.0 — Disaster/Nightmare Adversarial Test Report"
Cohesion: 0.14
Nodes (13): A note on trusting prior "PASS" claims, CloudDesk-OS v1.0.0 — Disaster/Nightmare Adversarial Test Report, Environment note, Findings, Gates, ID: CLAUDE-NIGHTMARE-001, ID: CLAUDE-NIGHTMARE-002, ID: CLAUDE-NIGHTMARE-003 (+5 more)

### Community 96 - "CloudDesk-OS Live Acceptance Report"
Cohesion: 0.18
Nodes (10): Applications with no backend implementation, CloudDesk-OS Live Acceptance Report, Conclusion, Real MinIO/S3 (clouddesk_remote::s3::S3Provider), Real OpenSSH server (clouddesk_remote::ssh::SshSession), Real SFTP server (clouddesk_remote::sftp::SftpProvider — CLAUDE-NIGHTMARE-003/-004 regressions), Real WebDAV server (clouddesk_remote::webdav::WebDavProvider), SSH authentication surface with no implementation (+2 more)

### Community 97 - "test-runtime-fixture/src/main.rs"
Cohesion: 0.18
Nodes (13): decode_hex(), echo(), handle_socket(), ignore_sigterm(), main(), HashMap, IntoResponse, Option (+5 more)

### Community 98 - "Highlights"
Cohesion: 0.20
Nodes (9): 1. Multi-User Web Desktop Platform, 2. Native Applications & Media, 3. Integrated Runtimes, 4. Remote Infrastructure & Transfers, 5. Security & Privilege Separation, 6. Linux Distribution Support & Efficiency, CloudDesk-OS v1.0.0 Release Notes, Highlights (+1 more)

### Community 99 - "live_oci.rs"
Cohesion: 0.46
Nodes (7): docker_available(), manager_with_oci(), probe_spec(), Arc, TempDir, task_15_availability_reports_unavailable_or_available_honestly(), task_16_hardened_container_full_lifecycle_start_health_stop_cleanup()

### Community 100 - "/nightmare-test"
Cohesion: 0.33
Nodes (5): Attack surface (this run), Hard safety rules (non-negotiable, override anything else), /nightmare-test, Procedure, Scope reminder

### Community 101 - "/disaster-test"
Cohesion: 0.40
Nodes (4): /disaster-test, Hard safety rules (non-negotiable, override anything else), Procedure, Scope reminder

### Community 102 - "CloudDesk-OS v1.0.0-rc.3"
Cohesion: 0.40
Nodes (4): Changes, CloudDesk-OS v1.0.0-rc.3, Conclusion, Release Candidate 3

### Community 103 - "kilo.json"
Cohesion: 0.50
Nodes (3): plugin, $schema, file:///home/ahmed/Documents/VsCode/Projects/CloudDesk-OS/.kilo/plugins/graphify.js

### Community 104 - "CloudDesk-OS v1.0 Final Readiness"
Cohesion: 0.50
Nodes (3): CloudDesk-OS v1.0 Final Readiness, Engineering Closure Checklist, Metrics

### Community 107 - "String"
Cohesion: 0.50
Nodes (3): Environmental blockers (not implementation gaps), Phase 6 — Optional Runtime Orchestrator: Executable Evidence Matrix, Summary

### Community 108 - "runtime_api.rs"
Cohesion: 0.15
Nodes (57): MaybeTlsStream, application_with_oci_runtime(), application_with_runtime(), application_with_runtime_env(), body_json(), bootstrap_admin(), connect_ws(), create_user() (+49 more)

### Community 109 - "SshSession"
Cohesion: 0.20
Nodes (14): Box, Duration, Error, Handle, Handler, Option, PublicKey, Result (+6 more)

### Community 114 - "scan_live.rs"
Cohesion: 0.29
Nodes (9): active, addAclEntry(), isAudio(), isVideo(), load(), loadAcl(), open(), selected (+1 more)

### Community 115 - "MediaProbe"
Cohesion: 0.25
Nodes (11): canManageRuntimes(), describeRuntimeError(), describeRuntimeStatus(), DISPLAY_NAMES, isProductRuntimeKind(), PRODUCT_RUNTIME_KINDS, ProductRuntimeKind, runtimeDisplayName() (+3 more)

### Community 117 - "RawFormat"
Cohesion: 0.29
Nodes (4): loadRuntimes(), serviceControl(), toggleRuntime(), ./runtime

### Community 118 - "mutate"
Cohesion: 0.25
Nodes (11): copySelected(), createArchive(), createFolder(), extractArchive(), isArchive(), join(), parent(), renameSelected() (+3 more)

### Community 119 - "ServersApp.svelte"
Cohesion: 0.38
Nodes (3): load(), remove(), save()

## Knowledge Gaps
- **359 isolated node(s):** `$schema`, `file:///home/ahmed/Documents/VsCode/Projects/CloudDesk-OS/.kilo/plugins/graphify.js`, `name`, `version`, `private` (+354 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **19 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `String` connect `Configuration System` to `JSON Schema Fields`, `Permissions and Manifests`, `Frontend Development Tooling`, `Core HTTP Service`, `TypeScript Configuration`, `Application Runtime Model`, `Product Specifications`, `Core Service CLI`, `Workspace Modes`, `HTML Entry Point`, `Linux Identity`, `PathBuf`, `root_boundary.rs`, `InstanceContext`, `exec.rs`, `compat.rs`, `RuntimeManager`, `extract_archive`, `music_authorization.rs`, `scan.rs`, `InstanceId`, `.channel_open_session`, `WebDavProvider`, `live_lifecycle.rs`, `read_acl`, `media_api.rs`, `SftpProvider`, `SshSession`, `Report`, `InstanceCgroup`, `VfsError`, `music_api.rs`, `proxy_http`, `storage.rs`, `json_request`, `test-runtime-fixture/src/main.rs`, `runtime_api.rs`, `SshSession`?**
  _High betweenness centrality (0.433) - this node is a cross-community bridge._
- **Why does `test_key_public_base64()` connect `.channel_open_session` to `Configuration System`?**
  _High betweenness centrality (0.042) - this node is a cross-community bridge._
- **What connects `$schema`, `file:///home/ahmed/Documents/VsCode/Projects/CloudDesk-OS/.kilo/plugins/graphify.js`, `name` to the rest of the system?**
  _359 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `JSON Schema Fields` be split into smaller, more focused modules?**
  _Cohesion score 0.06084379358437936 - nodes in this community are weakly interconnected._
- **Should `Configuration System` be split into smaller, more focused modules?**
  _Cohesion score 0.08058068588260046 - nodes in this community are weakly interconnected._
- **Should `Permissions and Manifests` be split into smaller, more focused modules?**
  _Cohesion score 0.084472049689441 - nodes in this community are weakly interconnected._
- **Should `Frontend Development Tooling` be split into smaller, more focused modules?**
  _Cohesion score 0.059125085440874914 - nodes in this community are weakly interconnected._