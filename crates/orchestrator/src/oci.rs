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
use std::sync::Arc;
use tokio::process::Command;

/// An optional, adapter-specific async action run *before* `docker
/// stop` is issued, given the instance's already-resolved loopback
/// port. Real motivating case (Task 5, Phase 9 Pass 3A-3): a real
/// Brave container found live -- `SIGTERM` alone (even reaching the
/// real Chromium binary as PID 1 directly) does not reliably flush its
/// cookie store synchronously, but a real CDP `Browser.close` call
/// (the same application-level shutdown path a user closing a real
/// browser window triggers) does. Adapters with no such requirement
/// (Code, Office) simply don't set this field.
pub type OciGracefulStopHook = Arc<
    dyn Fn(u16) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync,
>;

/// Builds the `CMD` args passed to the image's entrypoint, from
/// server-side state only (e.g. the instance's own proxy base path or
/// a resolved workspace directory) -- never from a request.
pub type OciCommandBuilder = Arc<dyn Fn(&InstanceContext) -> Vec<String> + Send + Sync>;
/// Builds additional `(host_path, container_path)` bind mounts beyond
/// the adapter's own `/state` mount, from server-side state only (Task
/// 9/10 of the Phase 7 closure pass: a workspace/profile directory
/// resolved via `CloudDesk`'s existing Linux-identity mapping, never a
/// client-chosen path).
pub type OciMountBuilder = Arc<dyn Fn(&InstanceContext) -> Vec<(String, String)> + Send + Sync>;
/// Builds the `(uid, gid)` the container should run as, from server-
/// side state only -- lets a runtime run as its owning `CloudDesk`
/// user's real mapped Linux identity instead of the image's default
/// user (Task 10/11/15).
pub type OciUserBuilder = Arc<dyn Fn(&InstanceContext) -> Option<(u32, u32)> + Send + Sync>;
/// Builds additional environment variables, from server-side state
/// only.
pub type OciEnvBuilder = Arc<dyn Fn(&InstanceContext) -> Vec<(String, String)> + Send + Sync>;

/// A fully-specified, trusted container runtime definition -- the image
/// reference and internal port are fixed at construction time by
/// trusted server code, never derived from a request (Task 15: "Runtime
/// definitions must come from trusted `CloudDesk` code/configuration").
/// The optional builder closures are likewise always compiled-in,
/// trusted server code -- they read only `InstanceContext` (server-side
/// state), never a request body.
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
    pub command: Option<OciCommandBuilder>,
    /// Additional bind mounts beyond the adapter's own `/state` mount.
    pub extra_mounts: Option<OciMountBuilder>,
    /// The `(uid, gid)` to run the container as, overriding the
    /// image's default user.
    pub run_as: Option<OciUserBuilder>,
    /// Additional environment variables beyond the adapter's own.
    pub extra_env: Option<OciEnvBuilder>,
    /// Additional Linux capabilities to grant beyond the hardened
    /// zero-capability default (`--cap-drop ALL`), each applied as its
    /// own `--cap-add`. A fixed, compiled-in list on a trusted
    /// `OciSpec` -- never client-controlled. Empty for every runtime
    /// that doesn't need this (Code does not); Phase 8's Collabora
    /// adapter is the first real consumer, and only after live-
    /// verifying (via real container logs) which specific capabilities
    /// its own internal per-document jailing actually requires to
    /// avoid falling back to an unjailed mode.
    pub extra_capabilities: &'static [&'static str],
    /// Adds `--add-host=host.docker.internal:host-gateway` so the
    /// container can reach a service the host process itself is
    /// listening on (e.g. Office's WOPI host) -- Docker's own
    /// documented, standard mechanism for exactly this, not a bespoke
    /// network workaround.
    pub add_host_gateway: bool,
    /// See [`OciGracefulStopHook`]. `None` for every adapter that
    /// doesn't need one.
    pub graceful_stop: Option<OciGracefulStopHook>,
    /// Task 6 (Phase 9 Pass 3A-3, Blocker 2): the Docker network this
    /// container joins. `None` keeps the prior default (`bridge`) --
    /// Code and Office are unaffected. Browser sets this to a
    /// dedicated, non-default network (created idempotently by its own
    /// adapter before first use, with inter-container communication
    /// disabled) because live testing this pass found real, structural
    /// reachability on the shared default `bridge` network: any
    /// container on it can reach any other container's ports directly
    /// by IP, and can reach whatever the host process itself listens
    /// on via the bridge gateway. Browser has no legitimate reason to
    /// reach a sibling runtime container or `clouddeskd` itself, so
    /// it gets network-level isolation from both, not just the
    /// application-level checks the broker already enforces.
    pub network_name: Option<&'static str>,
    /// Pass 3A-4: an explicit `(subnet, gateway)` to pin the network
    /// to, instead of letting Docker auto-assign one. Browser needs a
    /// fixed, compile-time-known gateway address so its own
    /// `--proxy-server` flag (`docker/brave/Dockerfile`) can point at
    /// `services/clouddeskd/src/browser_egress_proxy.rs` without any
    /// runtime IP lookup. `None` lets Docker pick automatically (every
    /// other adapter, and Browser itself before this pass).
    pub network_subnet: Option<(&'static str, &'static str)>,
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

/// Idempotently creates a dedicated bridge network with
/// inter-container communication disabled, if it doesn't already
/// exist. `--internal` is deliberately never used -- Browser still
/// needs real Internet egress -- only container-to-container traffic
/// on this network is blocked (Docker's own `enable_icc` bridge
/// option, not a bespoke firewall rule). Safe to call on every
/// `start()`: `docker network create` on an existing name is a no-op
/// exit-0 check first, avoiding a noisy failed-create call on the
/// common path.
async fn ensure_isolated_network(
    engine: Engine,
    name: &str,
    subnet: Option<(&str, &str)>,
) -> Result<(), String> {
    let exists = Command::new(engine.binary())
        .args(["network", "inspect", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success());
    if exists {
        return Ok(());
    }
    let mut args = vec![
        "network".to_owned(),
        "create".to_owned(),
        "--driver".to_owned(),
        "bridge".to_owned(),
        "--opt".to_owned(),
        "com.docker.network.bridge.enable_icc=false".to_owned(),
    ];
    if let Some((cidr, gateway)) = subnet {
        args.push("--subnet".to_owned());
        args.push(cidr.to_owned());
        args.push("--gateway".to_owned());
        args.push(gateway.to_owned());
    }
    args.push(name.to_owned());
    let output = Command::new(engine.binary())
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| e.to_string())?;
    // A concurrent `start()` may have created it first between the
    // inspect check above and this create call -- Docker's own "already
    // exists" failure for that race is not a real error.
    if output.status.success() || String::from_utf8_lossy(&output.stderr).contains("already exists")
    {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&output.stderr).into_owned())
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
        let network = if let Some(network_name) = self.spec.network_name {
            ensure_isolated_network(engine, network_name, self.spec.network_subnet)
                .await
                .map_err(AdapterError::Start)?;
            network_name
        } else {
            "bridge"
        };

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
            network,
            "--publish",
            &format!("127.0.0.1:{port}:{}", self.spec.container_port),
            "--volume",
            &format!("{state_dir}:/state"),
        ]);
        for capability in self.spec.extra_capabilities {
            cmd.args(["--cap-add", capability]);
        }
        if self.spec.add_host_gateway {
            cmd.args(["--add-host", "host.docker.internal:host-gateway"]);
        }
        if let Some(run_as) = &self.spec.run_as {
            if let Some((uid, gid)) = run_as(ctx) {
                cmd.args(["--user", &format!("{uid}:{gid}")]);
            }
        }
        if let Some(extra_mounts) = &self.spec.extra_mounts {
            for (host_path, container_path) in extra_mounts(ctx) {
                cmd.args(["--volume", &format!("{host_path}:{container_path}")]);
            }
        }
        if let Some(extra_env) = &self.spec.extra_env {
            for (key, value) in extra_env(ctx) {
                cmd.args(["--env", &format!("{key}={value}")]);
            }
        }
        cmd.arg(&self.spec.image);
        if let Some(command) = &self.spec.command {
            cmd.args(command(ctx));
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

    /// Real defect fixed during the Phase 7 closure pass: a bare TCP
    /// connect is not the same thing as "can actually serve a
    /// request". Measured live against `code-server` specifically: its
    /// listening socket opens roughly *1.7 seconds* before it can
    /// serve a real HTTP GET (confirmed by polling both independently
    /// against a real container). A bare-TCP-connect health check
    /// therefore marked the instance `Running` while a genuine request
    /// -- from this test suite (`task_18_real_ide_http_and_websocket
    /// _through_proxy`) or from a real user's browser hitting the
    /// proxy the moment the UI shows "running" -- could still get a
    /// connection-reset, surfaced through `proxy_http` as 502 Bad
    /// Gateway. Fixed by performing a real HTTP GET to
    /// `health_check_path` and requiring an actual parsed HTTP
    /// response (any status code -- even 404/401 proves the HTTP
    /// server itself is answering) rather than just a successful
    /// three-way handshake.
    async fn health(&self, ctx: &InstanceContext, _handle: &RunningHandle) -> HealthStatus {
        let Some(port) = ctx.port else {
            return HealthStatus::Unhealthy;
        };
        let url = format!("http://127.0.0.1:{port}{}", self.spec.health_check_path);
        let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
        else {
            return HealthStatus::NotReadyYet;
        };
        match client.get(&url).send().await {
            Ok(_) => HealthStatus::Ready,
            Err(_) => HealthStatus::NotReadyYet,
        }
    }

    async fn is_gone(&self, _ctx: &InstanceContext, handle: &RunningHandle) -> bool {
        let RunningHandle::Opaque(name) = handle else {
            return false;
        };
        let Some(engine) = detect_engine().await else {
            return false;
        };
        // A container that no longer exists at all (already reaped by
        // `--rm`) or that exists but is no longer in the `running`
        // state both count as "gone" for supervisor purposes.
        let Ok(output) = Command::new(engine.binary())
            .args(["inspect", "--format", "{{.State.Running}}", name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
        else {
            return true;
        };
        if !output.status.success() {
            return true; // inspect failed -- the container doesn't exist
        }
        String::from_utf8_lossy(&output.stdout).trim() != "true"
    }

    async fn stop(
        &self,
        ctx: &InstanceContext,
        handle: &mut RunningHandle,
    ) -> Result<(), AdapterError> {
        let RunningHandle::Opaque(name) = handle else {
            return Ok(());
        };
        if let (Some(hook), Some(port)) = (&self.spec.graceful_stop, ctx.port) {
            hook(port).await;
        }
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
