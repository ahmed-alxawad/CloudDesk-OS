# Graph Report - CloudDesk-OS  (2026-08-17)

## Corpus Check
- 99 files · ~52,601 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 1070 nodes · 2898 edges · 64 communities (48 shown, 16 thin omitted)
- Extraction: 98% EXTRACTED · 2% INFERRED · 0% AMBIGUOUS · INFERRED: 45 edges (avg confidence: 0.86)
- Token cost: 0 input · 0 output

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

## God Nodes (most connected - your core abstractions)
1. `String` - 113 edges
2. `ApiError` - 86 edges
3. `State` - 59 edges
4. `AppState` - 57 edges
5. `AuthService` - 51 edges
6. `principal()` - 47 edges
7. `require_auth_service()` - 47 edges
8. `AuthError` - 39 edges
9. `SessionPrincipal` - 34 edges
10. `authorize_request()` - 29 edges

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

## Communities (64 total, 16 thin omitted)

### Community 0 - "JSON Schema Fields"
Cohesion: 0.09
Nodes (127): ConnectInfo, HashMap, HeaderMap, Into, IntoResponse, InvalidHeaderValue, Json, Mutex (+119 more)

### Community 1 - "Configuration System"
Cohesion: 0.08
Nodes (55): AssignedRootAccess, audit_event(), AuthError, AuthPolicy, AuthService, backoff_seconds(), bootstrap_login_authorize_and_revoke(), BootstrapConfiguration (+47 more)

### Community 2 - "Permissions and Manifests"
Cohesion: 0.08
Nodes (55): Arc, aad_context_tampering_detected(), ciphertext_tampering_detected(), load_rejects_wrong_length_key(), load_round_trips_through_file(), nonce_tampering_detected(), plaintext_never_appears_in_ciphertext(), round_trip_requires_the_same_context() (+47 more)

### Community 3 - "Frontend Development Tooling"
Cohesion: 0.06
Nodes (64): action(), GrantClaims, GrantError, GrantSigner, PowerOperation, PrivdRequest, PrivdResponse, PrivilegedAction (+56 more)

### Community 4 - "Frontend Package Scripts"
Cohesion: 0.06
Nodes (31): availableApplications(), closeWindow(), focusWindow(), isAvailable(), openApplication(), scheduleSave(), showNotification(), startMove() (+23 more)

### Community 5 - "Core HTTP Service"
Cohesion: 0.13
Nodes (28): canonical_virtual(), entry_from_metadata(), EntryKind, execute_local(), join_virtual(), LocalFileOperation, LocalFileResult, LocalProvider (+20 more)

### Community 6 - "Application Manifest Schema"
Cohesion: 0.05
Nodes (41): enabled, file_associations, icon, id, name, required_permissions, route, runtime_dependency (+33 more)

### Community 7 - "TypeScript Configuration"
Cohesion: 0.11
Nodes (36): AccessMode, assigned_roots_reject_traversal_and_symlink_escape(), AssignedRoot, LinuxError, LinuxIdentity, lookup_uid(), lookup_user(), read_only_roots_reject_write_authorization() (+28 more)

### Community 8 - "Application Runtime Model"
Cohesion: 0.15
Nodes (28): concurrent_claims_never_claim_one_job_twice(), ensure_updated(), jobs_persist_and_interrupted_work_recovers_after_restart(), local_file_transfer_copies_bytes_and_calculates_checksum(), NewTransfer, now(), parse_state(), parse_strategy() (+20 more)

### Community 9 - "CI and Threat Controls"
Cohesion: 0.05
Nodes (36): dependencies, svelte, @xterm/addon-fit, @xterm/xterm, devDependencies, prettier, prettier-plugin-svelte, svelte-check (+28 more)

### Community 10 - "Product Specifications"
Cohesion: 0.16
Nodes (26): auth_method_name(), host_key_fingerprint(), input(), known_hosts_line(), NewRemoteServer, normalized_tags(), now(), parse_auth_method() (+18 more)

### Community 11 - "SQLite Persistence"
Cohesion: 0.07
Nodes (28): Brave runtime, Code runtime, Continue automatically after the Vault, Core security, Current partial item, Dependency safety, Distribution support, End-of-session checkpoint (+20 more)

### Community 12 - "Core Service CLI"
Cohesion: 0.15
Nodes (17): Config, ConfigError, DatabaseConfig, defaults_to_the_architecture_listener(), PrivilegeConfig, rejects_unknown_configuration_keys(), AsRef, Default (+9 more)

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
Cohesion: 0.28
Nodes (8): Router, application(), bootstrap_login_authorization_and_logout_are_enforced_server_side(), request(), Body, Method, Option, TempDir

### Community 22 - "Graphify Workflow"
Cohesion: 0.42
Nodes (8): Cli, Command, main(), migrate(), Command, PathBuf, Result, serve()

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
Cohesion: 0.53
Nodes (9): application_router(), application_router_configured(), application_router_with_privilege(), application_router_with_privilege_configured(), AssignedRootBody, build_router(), PrivilegeClient, router() (+1 more)

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

## Knowledge Gaps
- **206 isolated node(s):** `name`, `version`, `private`, `type`, `license` (+201 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **16 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `String` connect `Configuration System` to `JSON Schema Fields`, `Permissions and Manifests`, `Frontend Development Tooling`, `Core HTTP Service`, `TypeScript Configuration`, `Application Runtime Model`, `Product Specifications`, `Core Service CLI`, `Workspace Modes`, `HTML Entry Point`, `Linux Identity`?**
  _High betweenness centrality (0.260) - this node is a cross-community bridge._
- **Why does `ApiError` connect `JSON Schema Fields` to `Application Runtime Model`, `Configuration System`, `Product Specifications`, `Permissions and Manifests`?**
  _High betweenness centrality (0.041) - this node is a cross-community bridge._
- **Why does `AuthService` connect `Configuration System` to `PathBuf`, `JSON Schema Fields`, `Permissions and Manifests`?**
  _High betweenness centrality (0.032) - this node is a cross-community bridge._
- **What connects `name`, `version`, `private` to the rest of the system?**
  _206 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `JSON Schema Fields` be split into smaller, more focused modules?**
  _Cohesion score 0.08874224367182114 - nodes in this community are weakly interconnected._
- **Should `Configuration System` be split into smaller, more focused modules?**
  _Cohesion score 0.0822594501718213 - nodes in this community are weakly interconnected._
- **Should `Permissions and Manifests` be split into smaller, more focused modules?**
  _Cohesion score 0.084472049689441 - nodes in this community are weakly interconnected._