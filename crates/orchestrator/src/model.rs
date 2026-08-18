//! Typed identities and states for the optional-runtime orchestrator.
//!
//! Nothing in this module knows how to launch a process or a container --
//! it exists so the manager, adapters, HTTP layer, and persistence layer
//! all agree on the same vocabulary instead of each inventing their own.

use serde::{Deserialize, Serialize};

/// Which optional heavyweight runtime this is. Deliberately excludes
/// `Media` -- Phase 3's `clouddesk-media` already has a working process
/// lifecycle and is not routed through this manager (see the crate-level
/// docs).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    Code,
    Office,
    Browser,
    /// A disposable fixture used only to prove the orchestrator itself
    /// (Task 31/32). Never selectable through the real product API --
    /// see `is_selectable`.
    TestFixture,
}

impl RuntimeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Office => "office",
            Self::Browser => "browser",
            Self::TestFixture => "test_fixture",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "code" => Some(Self::Code),
            "office" => Some(Self::Office),
            "browser" => Some(Self::Browser),
            "test_fixture" => Some(Self::TestFixture),
            _ => None,
        }
    }

    /// Whether ordinary product surfaces (the real HTTP API, Settings)
    /// should ever offer this kind. `TestFixture` exists purely for this
    /// crate's own test suite and must never be reachable as if it were
    /// a real application -- callers building the public runtime list
    /// filter it out.
    #[must_use]
    pub fn is_selectable(self) -> bool {
        !matches!(self, Self::TestFixture)
    }

    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::Code, Self::Office, Self::Browser, Self::TestFixture]
    }
}

/// State of one runtime *instance* (one user's session of one kind).
/// Deliberately does not conflate "a process exists" with "ready to
/// serve" -- see Task 1/10.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    /// Adapter reports the runtime binary/image isn't present on this
    /// host at all.
    Unavailable,
    /// Available but the administrator has turned the runtime off.
    Disabled,
    Stopped,
    Starting,
    /// Process/container is alive AND has passed its readiness check.
    Running,
    /// Process/container is alive but readiness/health checks are
    /// failing.
    Unhealthy,
    Stopping,
    /// Exited unexpectedly, or exceeded a crash-loop threshold, or a
    /// start/health timeout expired.
    Failed,
}

impl InstanceState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Disabled => "disabled",
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Unhealthy => "unhealthy",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "unavailable" => Self::Unavailable,
            "disabled" => Self::Disabled,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "unhealthy" => Self::Unhealthy,
            "stopping" => Self::Stopping,
            "failed" => Self::Failed,
            _ => Self::Stopped,
        }
    }

    #[must_use]
    pub fn is_terminal_for_reconciliation(self) -> bool {
        matches!(self, Self::Stopped | Self::Failed | Self::Disabled)
    }
}

/// Whether an instance's on-disk state survives termination.
///
/// Represented as a per-instance policy value (Task 7), not hardcoded
/// per `RuntimeKind`, so a future adapter can choose differently --
/// e.g. Browser's own future policy (persistent for
/// administrator/manager/user, ephemeral for guest) is expressed by the
/// *caller* selecting this value when creating the instance, not by the
/// manager special-casing "Browser" internally.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Persistence {
    Persistent,
    Ephemeral,
}

/// A stable identity for one runtime instance. `generation` increments
/// on every start *attempt* for the same `instance_id`, so a stale
/// health-check/exit-handler task from a previous attempt can never be
/// mistaken for the current one (guards against both async-task
/// confusion and OS PID reuse -- see Task 2/27).
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct InstanceId {
    pub kind: RuntimeKind,
    pub owner_user_id: String,
    pub instance_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Generation(pub u64);

/// Typed resource ceilings (Task 13). Enforcement is best-effort and
/// documented per-field in the manager/cgroup modules -- storing a
/// number here is not itself a claim that it's enforced.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ResourcePolicy {
    pub max_instances_global: u32,
    pub max_instances_per_user: u32,
    pub memory_limit_bytes: Option<u64>,
    pub pids_limit: Option<u32>,
    /// CPU quota expressed as a fraction of one core (cgroup v2
    /// `cpu.max` style: `quota_micros / 100_000`).
    pub cpu_quota_fraction: Option<f32>,
    pub start_timeout: std::time::Duration,
    pub health_timeout: std::time::Duration,
    pub stop_timeout: std::time::Duration,
    pub idle_timeout: Option<std::time::Duration>,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self {
            max_instances_global: 8,
            max_instances_per_user: 1,
            memory_limit_bytes: Some(512 * 1024 * 1024),
            pids_limit: Some(64),
            cpu_quota_fraction: Some(1.0),
            start_timeout: std::time::Duration::from_secs(20),
            health_timeout: std::time::Duration::from_secs(10),
            stop_timeout: std::time::Duration::from_secs(10),
            idle_timeout: Some(std::time::Duration::from_mins(30)),
        }
    }
}
