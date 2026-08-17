# Graph Report - CloudDesk-OS  (2026-08-16)

## Corpus Check
- 78 files · ~36,442 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 836 nodes · 2300 edges · 51 communities (37 shown, 14 thin omitted)
- Extraction: 98% EXTRACTED · 2% INFERRED · 0% AMBIGUOUS · INFERRED: 43 edges (avg confidence: 0.86)
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
- Graphify Workflow
- Svelte Configuration
- Linux Identity
- Installer and HTTPS
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
- cloudesk-sessiond/src/main.rs
- cloudesk-privd/src/main.rs
- install.sh
- alpine.sh
- arch.sh
- debian.sh
- distro.sh
- fedora.sh
- rhel.sh
- installer-layout.sh
- root_boundary.rs
- PathBuf

## God Nodes (most connected - your core abstractions)
1. `String` - 97 edges
2. `ApiError` - 71 edges
3. `AuthService` - 50 edges
4. `State` - 50 edges
5. `AppState` - 48 edges
6. `AuthError` - 39 edges
7. `principal()` - 38 edges
8. `require_auth_service()` - 38 edges
9. `SessionPrincipal` - 33 edges
10. `request_metadata()` - 22 edges

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

## Communities (51 total, 14 thin omitted)

### Community 0 - "JSON Schema Fields"
Cohesion: 0.05
Nodes (41): enabled, file_associations, icon, id, name, required_permissions, route, runtime_dependency (+33 more)

### Community 1 - "Configuration System"
Cohesion: 0.15
Nodes (17): Config, ConfigError, DatabaseConfig, defaults_to_the_architecture_listener(), PrivilegeConfig, rejects_unknown_configuration_keys(), AsRef, Default (+9 more)

### Community 2 - "Permissions and Manifests"
Cohesion: 0.12
Nodes (12): Capability, is_known_capability(), AppManifest, is_identifier(), ManifestError, Error, Option, Result (+4 more)

### Community 3 - "Frontend Development Tooling"
Cohesion: 0.05
Nodes (36): dependencies, svelte, @xterm/addon-fit, @xterm/xterm, devDependencies, prettier, prettier-plugin-svelte, svelte-check (+28 more)

### Community 4 - "Frontend Package Scripts"
Cohesion: 0.09
Nodes (53): AssignedRootAccess, audit_event(), AuthError, AuthPolicy, AuthService, backoff_seconds(), bootstrap_login_authorize_and_revoke(), BootstrapConfiguration (+45 more)

### Community 5 - "Core HTTP Service"
Cohesion: 0.09
Nodes (108): ConnectInfo, HashMap, HeaderMap, Into, IntoResponse, InvalidHeaderValue, Json, Mutex (+100 more)

### Community 6 - "Application Manifest Schema"
Cohesion: 0.13
Nodes (27): canonical_virtual(), entry_from_metadata(), EntryKind, execute_local(), join_virtual(), LocalFileOperation, LocalFileResult, LocalProvider (+19 more)

### Community 7 - "TypeScript Configuration"
Cohesion: 0.14
Nodes (13): compilerOptions, allowJs, checkJs, isolatedModules, moduleResolution, noEmit, strict, extends (+5 more)

### Community 8 - "Application Runtime Model"
Cohesion: 0.15
Nodes (14): Manifest-Based Application Model, Optional Runtime Services, CloudDesk Process Model, Optional Heavy Applications, Application Manifest Contract, Enabled Manifest Is Not Authorization, Manifest Schema and Required Fields, Optional Runtime Lifecycle Contract (+6 more)

### Community 9 - "CI and Threat Controls"
Cohesion: 0.18
Nodes (13): CloudDesk CI Workflow, Rust CI Job, Web CI Job, Check and Test Workflow, Development Standards, Module Boundaries, Security Review Checklist, CloudDesk-OS Threat Model (+5 more)

### Community 10 - "Product Specifications"
Cohesion: 0.33
Nodes (9): CloudDesk-OS Architecture, Codex Implementation Prompt, First Incomplete Phase Rule, Task Completion Report Contract, CloudDesk-OS Goals, CloudDesk-OS Mission, Incremental Delivery Rule, CloudDesk-OS v1.0 Implementation Plan (+1 more)

### Community 11 - "SQLite Persistence"
Cohesion: 0.39
Nodes (7): connect(), migrate(), migrations_create_the_expected_baseline_and_shell_tables(), Error, Result, SqlitePool, MigrateError

### Community 12 - "Core Service CLI"
Cohesion: 0.42
Nodes (8): Cli, Command, main(), migrate(), Command, PathBuf, Result, serve()

### Community 13 - "Workspace Modes"
Cohesion: 0.50
Nodes (3): normalizeMode(), WORKSPACE_MODES, WorkspaceMode

### Community 14 - "Security Invariants"
Cohesion: 0.40
Nodes (5): Critical Security Rules, Least Privilege, Security Foundations Before Browser Runtime, No Generic Root Capability, Architectural Security Invariants

### Community 15 - "Phase Zero Delivery"
Cohesion: 0.50
Nodes (4): Phase 0 Repository Standards and Threat Model, Phase 0 Security Limitations, Current Phase 0 Status, Phase 0 Endpoint Integration Tests

### Community 16 - "HTML Entry Point"
Cohesion: 0.67
Nodes (3): Application Mount Point, Frontend main.ts Module Reference, Web Shell HTML Entry Document

### Community 17 - "Svelte Web Application"
Cohesion: 0.08
Nodes (27): availableApplications(), closeWindow(), focusWindow(), isAvailable(), openApplication(), scheduleSave(), showNotification(), startMove() (+19 more)

### Community 18 - "VFS and Transfers"
Cohesion: 0.67
Nodes (3): Virtual Filesystem Provider Abstraction, Persistent Server-to-Server Transfers, Transfers Belong to the Server

### Community 19 - "Product Mission"
Cohesion: 0.67
Nodes (3): Secure Resource-Efficient Multi-User Web Desktop, Browser-Native Linux Workspace, Official Distribution Test Matrix

### Community 20 - "Authorization Capabilities"
Cohesion: 0.67
Nodes (3): Stable Backend Authorization Identifiers, Capability Change Contract, Capability Registry

### Community 30 - "privilege/src/lib.rs"
Cohesion: 0.08
Nodes (50): action(), GrantClaims, GrantError, GrantSigner, PowerOperation, PrivdRequest, PrivdResponse, PrivilegedAction (+42 more)

### Community 31 - "SecretCipher"
Cohesion: 0.11
Nodes (26): Arc, round_trip_requires_the_same_context(), AsRef, Error, Path, Result, Self, Vec (+18 more)

### Community 32 - "transfers/src/lib.rs"
Cohesion: 0.16
Nodes (26): concurrent_claims_never_claim_one_job_twice(), ensure_updated(), jobs_persist_and_interrupted_work_recovers_after_restart(), NewTransfer, now(), parse_state(), parse_strategy(), random_id() (+18 more)

### Community 33 - "lookup_uid"
Cohesion: 0.11
Nodes (36): AccessMode, assigned_roots_reject_traversal_and_symlink_escape(), AssignedRoot, LinuxError, LinuxIdentity, lookup_uid(), lookup_user(), read_only_roots_reject_write_authorization() (+28 more)

### Community 34 - "audit/src/lib.rs"
Cohesion: 0.23
Nodes (22): append(), append_in_transaction(), audit_rows_cannot_be_updated_or_deleted(), AuditError, AuditReceipt, canonical_json(), concurrent_writers_produce_one_linear_chain(), database() (+14 more)

### Community 35 - "request"
Cohesion: 0.28
Nodes (8): Router, application(), bootstrap_login_authorization_and_logout_are_enforced_server_side(), request(), Body, Method, Option, TempDir

### Community 36 - "apps.ts"
Cohesion: 0.29
Nodes (4): AppDefinition, applications, RawManifest, RuntimeDependency

### Community 37 - "workspace.ts"
Cohesion: 0.38
Nodes (5): clampWindow(), DEFAULT_PREFERENCES, defaultWindow(), WindowLayout, WorkspacePreferences

### Community 38 - "request"
Cohesion: 0.33
Nodes (6): mapped_worker_grants_are_scoped_signed_and_audited(), request(), Body, Method, Option, Value

### Community 39 - "cloudesk-sessiond/src/main.rs"
Cohesion: 0.29
Nodes (7): Cli, Command, main(), Command, Option, PathBuf, Result

### Community 40 - "cloudesk-privd/src/main.rs"
Cohesion: 0.40
Nodes (4): Cli, main(), PathBuf, Result

### Community 49 - "root_boundary.rs"
Cohesion: 0.23
Nodes (14): Vec, TerminalClientMessage, TerminalServerMessage, Output, assert_child(), binary_sibling(), invoke_child(), privileged_boundary_child() (+6 more)

### Community 50 - "PathBuf"
Cohesion: 0.53
Nodes (9): application_router(), application_router_configured(), application_router_with_privilege(), application_router_with_privilege_configured(), AssignedRootBody, build_router(), PrivilegeClient, router() (+1 more)

## Knowledge Gaps
- **108 isolated node(s):** `name`, `version`, `private`, `type`, `license` (+103 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **14 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `String` connect `Frontend Package Scripts` to `transfers/src/lib.rs`, `Configuration System`, `audit/src/lib.rs`, `lookup_uid`, `Permissions and Manifests`, `Core HTTP Service`, `Application Manifest Schema`, `cloudesk-sessiond/src/main.rs`, `root_boundary.rs`, `privilege/src/lib.rs`, `SecretCipher`?**
  _High betweenness centrality (0.270) - this node is a cross-community bridge._
- **Why does `ApiError` connect `Core HTTP Service` to `transfers/src/lib.rs`, `Frontend Package Scripts`, `SecretCipher`?**
  _High betweenness centrality (0.045) - this node is a cross-community bridge._
- **Why does `AuthService` connect `Frontend Package Scripts` to `PathBuf`, `Core HTTP Service`, `SecretCipher`?**
  _High betweenness centrality (0.038) - this node is a cross-community bridge._
- **What connects `name`, `version`, `private` to the rest of the system?**
  _108 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `JSON Schema Fields` be split into smaller, more focused modules?**
  _Cohesion score 0.04878048780487805 - nodes in this community are weakly interconnected._
- **Should `Permissions and Manifests` be split into smaller, more focused modules?**
  _Cohesion score 0.12380952380952381 - nodes in this community are weakly interconnected._
- **Should `Frontend Development Tooling` be split into smaller, more focused modules?**
  _Cohesion score 0.05405405405405406 - nodes in this community are weakly interconnected._