//! Host-process runtime adapter (Task 17): runs a runtime instance as a
//! direct child process, with a typed, fixed argv/environment/working
//! directory -- never a shell, never a caller-supplied command.

use crate::adapter::{
    AdapterError, Availability, HealthStatus, InstanceContext, RunningHandle, RuntimeAdapter,
};
use crate::model::RuntimeKind;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// How to determine readiness once the process is alive. Every variant
/// is fixed, compiled-in behavior -- there is no "arbitrary check
/// expression from config" variant.
#[derive(Clone)]
pub enum HealthCheck {
    /// The process being alive is sufficient (used by the test fixture
    /// for the "process spawned but not readiness-checked" contrast
    /// case, and by adapters with no network surface at all).
    ProcessAlive,
    /// A bare TCP connect to `ctx.port` succeeding counts as ready.
    TcpConnect,
    /// An HTTP GET to `ctx.port` + this path returning any 2xx counts
    /// as ready. Implemented with a raw `TcpStream` (no HTTP client
    /// dependency needed for a one-line request/status-line check).
    HttpGet { path: &'static str },
}

/// Builds the fixed argv for a launch, from server-side state only.
pub type ArgvBuilder = Arc<dyn Fn(&InstanceContext) -> Vec<String> + Send + Sync>;
/// Builds the *entire* child environment, from server-side state only.
pub type EnvBuilder = Arc<dyn Fn(&InstanceContext) -> HashMap<String, String> + Send + Sync>;

/// A fully-specified, trusted host-process runtime definition. Adapters
/// for Code/Office/Browser (once implemented) and the test fixture are
/// all just different `HostProcessSpec` values -- the executable path,
/// argv, and environment are fixed at construction time by trusted
/// server code, never derived from a request.
#[derive(Clone)]
pub struct HostProcessSpec {
    pub kind: RuntimeKind,
    /// Absolute path to the executable, or `None` if this kind's real
    /// implementation doesn't exist yet (Task 38: minimal placeholder
    /// adapters that report `Unavailable`).
    pub executable: Option<String>,
    pub argv: ArgvBuilder,
    /// This is the *entire* environment the child receives --
    /// `Command::env_clear()` is always applied first, so nothing from
    /// `clouddeskd`'s own process environment (which may hold the Vault
    /// master key path, database URL, etc.) is ever inherited (Task 4).
    pub env: EnvBuilder,
    pub health_check: HealthCheck,
}

pub struct HostProcessAdapter {
    spec: HostProcessSpec,
    /// Bounded ring buffer of captured stdout+stderr, shared with the
    /// background reader task spawned in `start`.
    logs: Arc<tokio::sync::Mutex<Vec<u8>>>,
}

const MAX_CAPTURED_LOG_BYTES: usize = 64 * 1024;

impl HostProcessAdapter {
    #[must_use]
    pub fn new(spec: HostProcessSpec) -> Self {
        Self {
            spec,
            logs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl RuntimeAdapter for HostProcessAdapter {
    fn kind(&self) -> RuntimeKind {
        self.spec.kind
    }

    async fn availability(&self) -> Availability {
        let Some(executable) = &self.spec.executable else {
            return Availability::Unavailable {
                reason: "no implementation configured for this runtime yet".to_owned(),
            };
        };
        if tokio::fs::metadata(executable).await.is_ok() {
            Availability::Available {
                detail: executable.clone(),
            }
        } else {
            Availability::Unavailable {
                reason: format!("executable not found: {executable}"),
            }
        }
    }

    async fn prepare(&self, ctx: &InstanceContext) -> Result<(), AdapterError> {
        // The state directory itself is created by `crate::storage`
        // before this is called; nothing kind-specific to prepare for
        // the generic host-process case beyond that.
        let _ = ctx;
        Ok(())
    }

    async fn start(&self, ctx: &InstanceContext) -> Result<RunningHandle, AdapterError> {
        let Some(executable) = &self.spec.executable else {
            return Err(AdapterError::Unavailable(
                "no implementation configured".to_owned(),
            ));
        };
        let argv = (self.spec.argv)(ctx);
        let env = (self.spec.env)(ctx);

        let mut command = Command::new(executable);
        command
            .args(&argv)
            .current_dir(&ctx.state_dir)
            .env_clear()
            .envs(&env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        // A new session/process-group leader so the whole tree can be
        // signaled together on stop/kill (Task 30).
        // `pre_exec` closures run between fork and exec, so they must be
        // async-signal-safe -- `rustix::process::setsid()` is a single
        // direct syscall with no allocation, satisfying that. This is
        // the one narrowly-scoped, reviewed use of `unsafe` in this
        // crate; it exists to make process-tree termination (Task 30)
        // possible at all (SIGTERM/SIGKILL to the whole group), not to
        // run caller-supplied code.
        #[cfg(unix)]
        #[allow(unsafe_code)]
        unsafe {
            #[allow(unused_imports)]
            use std::os::unix::process::CommandExt as _;
            command.pre_exec(|| {
                rustix::process::setsid()
                    .map(|_| ())
                    .map_err(std::io::Error::from)
            });
        }

        let mut child = command
            .spawn()
            .map_err(|e| AdapterError::Start(e.to_string()))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let logs = self.logs.clone();
        tokio::spawn(async move {
            let mut buf = [0_u8; 4096];
            let mut stdout = stdout;
            let mut stderr = stderr;
            loop {
                let mut progressed = false;
                if let Some(out) = stdout.as_mut() {
                    if let Ok(n) = out.read(&mut buf).await {
                        if n > 0 {
                            append_bounded(&logs, &buf[..n]).await;
                            progressed = true;
                        } else {
                            stdout = None;
                        }
                    } else {
                        stdout = None;
                    }
                }
                if let Some(err) = stderr.as_mut() {
                    if let Ok(n) = err.read(&mut buf).await {
                        if n > 0 {
                            append_bounded(&logs, &buf[..n]).await;
                            progressed = true;
                        } else {
                            stderr = None;
                        }
                    } else {
                        stderr = None;
                    }
                }
                if stdout.is_none() && stderr.is_none() {
                    break;
                }
                if !progressed {
                    tokio::task::yield_now().await;
                }
            }
        });

        Ok(RunningHandle::Process(child))
    }

    async fn health(&self, ctx: &InstanceContext, handle: &RunningHandle) -> HealthStatus {
        let RunningHandle::Process(child) = handle else {
            return HealthStatus::Unhealthy;
        };
        if let Some(id) = child.id() {
            if !process_alive(id) {
                return HealthStatus::Unhealthy;
            }
        } else {
            return HealthStatus::Unhealthy;
        }

        match &self.spec.health_check {
            HealthCheck::ProcessAlive => HealthStatus::Ready,
            HealthCheck::TcpConnect => {
                let Some(port) = ctx.port else {
                    return HealthStatus::Unhealthy;
                };
                match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                    Ok(_) => HealthStatus::Ready,
                    Err(_) => HealthStatus::NotReadyYet,
                }
            }
            HealthCheck::HttpGet { path } => {
                let Some(port) = ctx.port else {
                    return HealthStatus::Unhealthy;
                };
                match http_get_status(port, path).await {
                    Some(status) if (200..300).contains(&status) => HealthStatus::Ready,
                    Some(_) => HealthStatus::Unhealthy,
                    None => HealthStatus::NotReadyYet,
                }
            }
        }
    }

    async fn stop(
        &self,
        _ctx: &InstanceContext,
        handle: &mut RunningHandle,
    ) -> Result<(), AdapterError> {
        let RunningHandle::Process(child) = handle else {
            return Ok(());
        };
        let Some(pid) = child.id() else {
            return Ok(());
        };
        // Signal the whole process group (negative pid), not just the
        // immediate child, so descendants get the graceful-shutdown
        // signal too (Task 30).
        #[cfg(unix)]
        {
            let group_id = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap_or(i32::MAX));
            if let Some(group_id) = group_id {
                let _ =
                    rustix::process::kill_process_group(group_id, rustix::process::Signal::TERM);
            }
        }
        Ok(())
    }

    async fn kill(&self, _ctx: &InstanceContext, handle: &mut RunningHandle) {
        let RunningHandle::Process(child) = handle else {
            return;
        };
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            let group_id = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap_or(i32::MAX));
            if let Some(group_id) = group_id {
                let _ =
                    rustix::process::kill_process_group(group_id, rustix::process::Signal::KILL);
            }
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }

    async fn cleanup(&self, ctx: &InstanceContext) -> Result<(), AdapterError> {
        crate::storage::remove_instance_state_dir(&ctx.runtime_root, &ctx.id)
            .map_err(|e| AdapterError::Storage(e.to_string()))
    }

    async fn logs(
        &self,
        _ctx: &InstanceContext,
        _handle: &RunningHandle,
        max_bytes: usize,
    ) -> Vec<u8> {
        let buf = self.logs.lock().await;
        let start = buf.len().saturating_sub(max_bytes);
        buf[start..].to_vec()
    }

    fn describe_environment(&self, ctx: &InstanceContext) -> HashMap<String, String> {
        (self.spec.env)(ctx)
    }
}

async fn append_bounded(logs: &tokio::sync::Mutex<Vec<u8>>, data: &[u8]) {
    let mut buf = logs.lock().await;
    buf.extend_from_slice(data);
    if buf.len() > MAX_CAPTURED_LOG_BYTES {
        let excess = buf.len() - MAX_CAPTURED_LOG_BYTES;
        buf.drain(0..excess);
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    rustix::process::Pid::from_raw(i32::try_from(pid).unwrap_or(i32::MAX))
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}
#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    true
}

/// Minimal, dependency-free HTTP status check: connect, send a fixed
/// GET request (fixed method/path/headers -- `path` is a `&'static str`
/// baked into the adapter spec, never request-supplied), read just
/// enough of the response to parse the status line. Returns `None` on
/// any connection/parse failure (treated as "not ready yet", not an
/// error -- a runtime that hasn't opened its listener yet is normal
/// during startup).
async fn http_get_status(port: u16, path: &str) -> Option<u16> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut buf = vec![0_u8; 512];
    let n = stream.read(&mut buf).await.ok()?;
    let response = String::from_utf8_lossy(&buf[..n]);
    let status_line = response.lines().next()?;
    let mut parts = status_line.split_whitespace();
    parts.next()?; // "HTTP/1.1"
    parts.next()?.parse::<u16>().ok()
}
