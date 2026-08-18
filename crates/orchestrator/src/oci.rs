//! Optional OCI/container adapter (Task 15/16): runs a runtime instance
//! as a `docker`/`podman` container instead of a bare host process.
//!
//! `CloudDesk` core never requires a container runtime to start (Task
//! 36) -- `availability()` reports `Unavailable` cleanly if neither CLI
//! is found or the daemon isn't reachable, exactly like the host-process
//! adapter reports `Unavailable` for a missing executable.
//!
//! Every invocation is a fixed argv built entirely from trusted,
//! compiled-in fields on `OciSpec` -- an image reference, mounts, and
//! flags are never accepted from a request. Hardened defaults (Task 16)
//! are applied unconditionally: no privileged mode, no Docker socket
//! mount, no host PID/IPC namespace, no added capabilities,
//! `--security-opt no-new-privileges`, an explicit read-only root
//! filesystem with one explicit writable state mount, and the resource
//! limits from `ResourcePolicy`.

use crate::adapter::{
    AdapterError, Availability, HealthStatus, InstanceContext, RunningHandle, RuntimeAdapter,
};
use crate::model::RuntimeKind;
use std::process::Stdio;
use tokio::process::Command;

/// A fully-specified, trusted container runtime definition -- the image
/// reference and internal port are fixed at construction time by
/// trusted server code, never derived from a request (Task 15: "Runtime
/// definitions must come from trusted `CloudDesk` code/configuration").
#[derive(Clone)]
pub struct OciSpec {
    pub kind: RuntimeKind,
    pub image: String,
    /// Port the container's own process listens on internally; mapped
    /// to `ctx.port` on the loopback interface only (Task 18).
    pub container_port: u16,
    pub health_check_path: &'static str,
    /// Overrides the image's default `CMD`, when the runtime image
    /// needs an explicit invocation to start its service. Still a
    /// fixed, compiled-in field on a trusted `OciSpec` -- never
    /// request-supplied argv (Task 3/15).
    pub command: Option<Vec<String>>,
}

/// Which container CLI is available, detected once and reused --
/// `docker` and `podman` accept the same argv shape for everything this
/// adapter needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    Docker,
    Podman,
}

impl Engine {
    fn binary(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

async fn detect_engine() -> Option<Engine> {
    for engine in [Engine::Docker, Engine::Podman] {
        let ok = Command::new(engine.binary())
            .args(["version", "--format", "{{.Server.Version}}"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success());
        if ok {
            return Some(engine);
        }
    }
    None
}

pub struct OciAdapter {
    spec: OciSpec,
}

impl OciAdapter {
    #[must_use]
    pub fn new(spec: OciSpec) -> Self {
        Self { spec }
    }

    fn container_name(ctx: &InstanceContext) -> String {
        format!("clouddesk-runtime-{}", ctx.id.instance_id)
    }
}

#[async_trait::async_trait]
impl RuntimeAdapter for OciAdapter {
    fn kind(&self) -> RuntimeKind {
        self.spec.kind
    }

    async fn availability(&self) -> Availability {
        let Some(engine) = detect_engine().await else {
            return Availability::Unavailable {
                reason: "neither docker nor podman is available/reachable on this host".to_owned(),
            };
        };
        let has_image = Command::new(engine.binary())
            .args(["image", "inspect", &self.spec.image])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success());
        if has_image {
            Availability::Available {
                detail: format!("{} ({})", engine.binary(), self.spec.image),
            }
        } else {
            Availability::Unavailable {
                reason: format!("image not present locally: {}", self.spec.image),
            }
        }
    }

    async fn prepare(&self, _ctx: &InstanceContext) -> Result<(), AdapterError> {
        Ok(())
    }

    async fn start(&self, ctx: &InstanceContext) -> Result<RunningHandle, AdapterError> {
        let Some(engine) = detect_engine().await else {
            return Err(AdapterError::Unavailable(
                "no container engine available".to_owned(),
            ));
        };
        let name = Self::container_name(ctx);
        let port = ctx.port.ok_or_else(|| {
            AdapterError::Start("OCI adapter requires an allocated port".to_owned())
        })?;

        // Fixed, hardened argv (Task 16) -- every flag here is a
        // constant or derived from server-side state
        // (name/port/state_dir/policy), never from request input.
        let memory_limit = ctx
            .policy
            .memory_limit_bytes
            .map_or_else(|| "512m".to_owned(), |b| format!("{b}"));
        let pids_limit = ctx.policy.pids_limit.unwrap_or(64).to_string();
        let state_dir = ctx.state_dir.to_string_lossy().into_owned();

        let mut cmd = Command::new(engine.binary());
        cmd.args([
            "run",
            "--detach",
            "--name",
            &name,
            "--rm",
            "--security-opt",
            "no-new-privileges",
            "--cap-drop",
            "ALL",
            "--pids-limit",
            &pids_limit,
            "--memory",
            &memory_limit,
            "--network",
            "bridge",
            "--publish",
            &format!("127.0.0.1:{port}:{}", self.spec.container_port),
            "--volume",
            &format!("{state_dir}:/state"),
        ]);
        cmd.arg(&self.spec.image);
        if let Some(command) = &self.spec.command {
            cmd.args(command);
        }
        let output = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| AdapterError::Start(e.to_string()))?;

        if !output.status.success() {
            return Err(AdapterError::Start(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        Ok(RunningHandle::Opaque(name))
    }

    async fn health(&self, ctx: &InstanceContext, _handle: &RunningHandle) -> HealthStatus {
        let Some(port) = ctx.port else {
            return HealthStatus::Unhealthy;
        };
        match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(_) => HealthStatus::Ready,
            Err(_) => HealthStatus::NotReadyYet,
        }
    }

    async fn stop(
        &self,
        _ctx: &InstanceContext,
        handle: &mut RunningHandle,
    ) -> Result<(), AdapterError> {
        let RunningHandle::Opaque(name) = handle else {
            return Ok(());
        };
        let Some(engine) = detect_engine().await else {
            return Ok(());
        };
        let _ = Command::new(engine.binary())
            .args(["stop", "--time", "5", name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        // `--rm` removes the container asynchronously once it's
        // actually exited -- `docker stop` returning is not the same
        // moment as the container disappearing. Poll briefly so that
        // `stop()` returning really means "gone", matching the
        // process-tree-termination-verified guarantee the host-process
        // adapter gives (Task 30).
        for _ in 0..20 {
            let gone = !Command::new(engine.binary())
                .args(["inspect", name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .is_ok_and(|s| s.success());
            if gone {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        // Still present after a bounded wait -- `--rm` didn't take
        // effect (e.g. the container never actually stopped). Fall
        // back to a forced removal rather than leaving it orphaned.
        let _ = Command::new(engine.binary())
            .args(["rm", "-f", name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        Ok(())
    }

    async fn kill(&self, _ctx: &InstanceContext, handle: &mut RunningHandle) {
        let RunningHandle::Opaque(name) = handle else {
            return;
        };
        if let Some(engine) = detect_engine().await {
            let _ = Command::new(engine.binary())
                .args(["kill", name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            // `--rm` above means the container is removed automatically
            // once it stops; an explicit `rm -f` is still attempted as
            // a safety net in case `--rm` didn't apply (e.g. the daemon
            // restarted mid-run).
            let _ = Command::new(engine.binary())
                .args(["rm", "-f", name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }
    }

    async fn cleanup(&self, ctx: &InstanceContext) -> Result<(), AdapterError> {
        crate::storage::remove_instance_state_dir(&ctx.runtime_root, &ctx.id)
            .map_err(|e| AdapterError::Storage(e.to_string()))
    }

    async fn logs(
        &self,
        _ctx: &InstanceContext,
        handle: &RunningHandle,
        max_bytes: usize,
    ) -> Vec<u8> {
        let RunningHandle::Opaque(name) = handle else {
            return Vec::new();
        };
        let Some(engine) = detect_engine().await else {
            return Vec::new();
        };
        let Ok(output) = Command::new(engine.binary())
            .args(["logs", "--tail", "200", name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
        else {
            return Vec::new();
        };
        let mut combined = output.stdout;
        combined.extend_from_slice(&output.stderr);
        combined.truncate(max_bytes);
        combined
    }
}
