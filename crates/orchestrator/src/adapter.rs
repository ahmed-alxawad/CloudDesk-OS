//! The narrow adapter interface every runtime implementation (host
//! process, OCI container, future Code/Office/Browser-specific logic)
//! must implement. Nothing in this trait accepts a caller-supplied
//! executable, argv, image name, or mount -- adapters are compiled-in,
//! trusted server code/configuration (Task 3).

use crate::model::{InstanceId, ResourcePolicy};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability {
    Available { detail: String },
    Unavailable { reason: String },
}

impl Availability {
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthStatus {
    Ready,
    NotReadyYet,
    Unhealthy,
}

/// Everything an adapter needs to launch/manage one instance, built
/// entirely from server-side state -- never deserialized from a
/// request body.
#[derive(Clone, Debug)]
pub struct InstanceContext {
    pub id: InstanceId,
    pub generation: u64,
    /// The server-owned root all instance storage lives under (see
    /// `crate::storage`), kept alongside `state_dir` so adapters can
    /// call `crate::storage::remove_instance_state_dir` without having
    /// to reverse-engineer it from `state_dir`'s ancestors.
    pub runtime_root: PathBuf,
    /// This instance's private, already-created (see `crate::storage`)
    /// directory. The adapter must confine all instance state here.
    pub state_dir: PathBuf,
    pub policy: ResourcePolicy,
    /// Loopback port reserved for this instance, if the adapter is a
    /// network service (Task 18). `None` for adapters with no network
    /// surface.
    pub port: Option<u16>,
}

/// An opaque, adapter-defined handle to a *running* attempt. The
/// manager never inspects its contents -- it exists so `health`/`stop`/
/// `kill`/`logs` can be called against the same live process/container
/// that `start` produced, without the manager needing to know whether
/// that's a PID, a container ID, or something else.
pub enum RunningHandle {
    Process(tokio::process::Child),
    /// Used by adapters (like the OCI adapter) whose "handle" is just an
    /// external identifier the adapter's own CLI/API calls operate on.
    Opaque(String),
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("runtime is not available on this host: {0}")]
    Unavailable(String),
    #[error("failed to prepare instance storage: {0}")]
    Storage(String),
    #[error("failed to start: {0}")]
    Start(String),
    #[error("failed to stop cleanly: {0}")]
    Stop(String),
    #[error("operation timed out")]
    Timeout,
}

/// The typed adapter interface (Task 3). All methods are given only
/// `InstanceContext`/a previously-returned handle -- there is no method
/// that accepts a raw command line, image reference, or mount spec from
/// outside this trait's own (trusted, compiled-in) implementation.
#[async_trait::async_trait]
pub trait RuntimeAdapter: Send + Sync {
    fn kind(&self) -> crate::model::RuntimeKind;

    /// Cheap, side-effect-free check for whether this runtime *could* be
    /// started on this host (binary/image present, etc.) -- must never
    /// itself start a resident process.
    async fn availability(&self) -> Availability;

    /// Creates whatever on-disk state the instance needs before
    /// starting (Task 6). Idempotent.
    async fn prepare(&self, ctx: &InstanceContext) -> Result<(), AdapterError>;

    /// Launches the instance. Must not block waiting for readiness --
    /// that is `health`'s job (Task 1: do not report running merely
    /// because a process was spawned).
    async fn start(&self, ctx: &InstanceContext) -> Result<RunningHandle, AdapterError>;

    async fn health(&self, ctx: &InstanceContext, handle: &RunningHandle) -> HealthStatus;

    /// Whether the underlying process/container has genuinely exited
    /// (Task 30 of the Phase 7 closure pass: a real gap found while
    /// testing crash recovery for an OCI-backed runtime -- unlike
    /// `RunningHandle::Process`, whose exit the manager's supervisor
    /// loop detects directly via `try_wait`, a `RunningHandle::Opaque`
    /// handle gives the supervisor no way to distinguish "briefly
    /// unhealthy" from "gone for good", so it never escalated past
    /// `Unhealthy` to a terminal `Failed` state on a real crash).
    /// Default `false` preserves existing behavior for adapters (like
    /// `HostProcessAdapter`) that already detect exit through their own
    /// handle type; `OciAdapter` overrides this with a real
    /// `docker/podman inspect` check.
    async fn is_gone(&self, ctx: &InstanceContext, handle: &RunningHandle) -> bool {
        let _ = (ctx, handle);
        false
    }

    /// Requests graceful shutdown (e.g. SIGTERM to a process group, or
    /// a container stop). Must return promptly; the manager applies its
    /// own bounded wait before calling `kill`.
    async fn stop(
        &self,
        ctx: &InstanceContext,
        handle: &mut RunningHandle,
    ) -> Result<(), AdapterError>;

    /// Forceful termination, including any descendants (Task 30).
    async fn kill(&self, ctx: &InstanceContext, handle: &mut RunningHandle);

    /// Removes ephemeral instance state. Never touches persistent
    /// profile data -- the manager only calls this when the instance's
    /// `Persistence` policy is `Ephemeral`.
    async fn cleanup(&self, ctx: &InstanceContext) -> Result<(), AdapterError>;

    /// Bounded log tail. `max_bytes` is enforced by the caller
    /// regardless of what the adapter returns.
    async fn logs(
        &self,
        ctx: &InstanceContext,
        handle: &RunningHandle,
        max_bytes: usize,
    ) -> Vec<u8>;

    /// The environment this adapter would launch a process/container
    /// with, for inspection/testing only (Task 4/35: proving secrets
    /// are never inherited). Adapters with no process environment
    /// (none currently) may return an empty map.
    fn describe_environment(&self, ctx: &InstanceContext) -> HashMap<String, String> {
        let _ = ctx;
        HashMap::new()
    }
}
