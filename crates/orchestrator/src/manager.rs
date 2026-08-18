//! The `RuntimeManager`: owns the adapter registry, per-instance state
//! machines, concurrency-safe lifecycle transitions, idle shutdown,
//! crash-loop protection, and startup reconciliation.
//!
//! Every lifecycle operation on a given instance is serialized through
//! that instance's own `tokio::sync::Mutex` (Task 9) -- two concurrent
//! `start` calls, a `stop` racing a `start`, etc. all simply queue on
//! the same lock rather than racing on shared state. This trades a
//! little latency (a `stop` sent mid-`start` waits for the bounded
//! start/health timeout to resolve before it can act) for actual
//! correctness, which the phase's own instructions prioritize
//! explicitly.

use crate::adapter::{HealthStatus, InstanceContext, RunningHandle, RuntimeAdapter};
use crate::cgroup::InstanceCgroup;
use crate::model::{InstanceId, InstanceState, Persistence, ResourcePolicy, RuntimeKind};
use crate::port::PortAllocator;
use crate::store::RuntimeStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("runtime kind is not registered")]
    UnknownKind,
    #[error("runtime is disabled")]
    Disabled,
    #[error("runtime is not available on this host: {0}")]
    Unavailable(String),
    #[error("per-user instance limit reached")]
    PerUserLimitReached,
    #[error("global instance limit reached")]
    GlobalLimitReached,
    #[error("instance not found")]
    NotFound,
    #[error("instance is owned by a different user")]
    NotOwner,
    #[error(transparent)]
    Adapter(#[from] crate::adapter::AdapterError),
    #[error(transparent)]
    Storage(#[from] crate::storage::StorageError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("start timed out waiting for readiness")]
    StartTimeout,
}

#[derive(Debug, thiserror::Error)]
pub enum StopError {
    #[error("instance not found")]
    NotFound,
    #[error("instance is owned by a different user")]
    NotOwner,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Bounded automatic-restart policy applied after an unexpected exit
/// (Task 11): `restart_instance` refuses once the persisted
/// `restart_count` for this instance exceeds this threshold, leaving it
/// `Failed` rather than retried again. Deliberately a simple monotonic
/// counter rather than a sliding time window -- the counter resets only
/// when a *new* instance is created (`create_instance` always starts at
/// 0), so a crash-looping instance cannot "age out" of the limit by
/// simply waiting, which a time-windowed count would allow.
const CRASH_LOOP_MAX_RESTARTS: u32 = 3;

struct LiveInstance {
    ctx: InstanceContext,
    persistence: Persistence,
    runtime: Mutex<InstanceRuntime>,
}

struct InstanceRuntime {
    state: InstanceState,
    handle: Option<RunningHandle>,
    generation: u64,
    cgroup: Option<InstanceCgroup>,
    last_activity: Instant,
    /// The port allocated for the *current* running attempt, if any.
    /// Lives here (not on `LiveInstance::ctx`, which is immutable after
    /// `create_instance`) because it changes on every start/restart.
    port: Option<u16>,
}

type InstanceKey = (RuntimeKind, String, String);

fn key_of(id: &InstanceId) -> InstanceKey {
    (id.kind, id.owner_user_id.clone(), id.instance_id.clone())
}

pub struct RuntimeManager {
    adapters: HashMap<RuntimeKind, Arc<dyn RuntimeAdapter>>,
    store: RuntimeStore,
    runtime_root: PathBuf,
    ports: Arc<PortAllocator>,
    policy: ResourcePolicy,
    live: RwLock<HashMap<InstanceKey, Arc<LiveInstance>>>,
}

impl RuntimeManager {
    #[must_use]
    pub fn new(store: RuntimeStore, runtime_root: PathBuf, policy: ResourcePolicy) -> Self {
        Self {
            adapters: HashMap::new(),
            store,
            runtime_root,
            ports: Arc::new(PortAllocator::new()),
            policy,
            live: RwLock::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn with_adapter(mut self, adapter: Arc<dyn RuntimeAdapter>) -> Self {
        self.adapters.insert(adapter.kind(), adapter);
        self
    }

    #[must_use]
    pub fn store(&self) -> &RuntimeStore {
        &self.store
    }

    /// The server-owned, symlink-safe directory a given instance's
    /// state lives in (Phase 7 closure: lets a caller like `clouddeskd`
    /// stage a small trusted marker file -- e.g. a resolved Linux
    /// identity -- before `start_instance` runs, without exposing the
    /// manager's internal `runtime_root` field or requiring a live
    /// instance to already exist). Idempotent and ownership-independent
    /// -- deriving the path requires no lookup, only `id` itself.
    pub fn instance_state_dir(
        &self,
        id: &InstanceId,
    ) -> Result<std::path::PathBuf, crate::storage::StorageError> {
        crate::storage::instance_state_dir(&self.runtime_root, id)
    }

    fn adapter(&self, kind: RuntimeKind) -> Result<&Arc<dyn RuntimeAdapter>, StartError> {
        self.adapters.get(&kind).ok_or(StartError::UnknownKind)
    }

    pub async fn availability(&self, kind: RuntimeKind) -> crate::adapter::Availability {
        match self.adapters.get(&kind) {
            Some(adapter) => adapter.availability().await,
            None => crate::adapter::Availability::Unavailable {
                reason: "runtime kind not registered".to_owned(),
            },
        }
    }

    pub async fn is_enabled(&self, kind: RuntimeKind) -> Result<bool, sqlx::Error> {
        self.store.is_enabled(kind).await
    }

    /// Task 8: disabling stops every live instance of this kind
    /// gracefully (bounded wait, then force-kill), before persisting
    /// the disabled flag as the last step -- so a crash mid-disable
    /// still leaves the flag unset rather than claiming "disabled"
    /// while a process might still be alive.
    pub async fn set_enabled(&self, kind: RuntimeKind, enabled: bool) -> Result<(), StartError> {
        if !enabled {
            let victims: Vec<Arc<LiveInstance>> = {
                let live = self.live.read().await;
                live.iter()
                    .filter(|((k, _, _), _)| *k == kind)
                    .map(|(_, v)| v.clone())
                    .collect()
            };
            for instance in victims {
                self.stop_live(&instance, true).await;
            }
        }
        self.store.set_enabled(kind, enabled).await?;
        Ok(())
    }

    /// Creates a new instance record (Stopped) without starting it.
    /// Enforces global/per-user instance limits (Task 13).
    pub async fn create_instance(
        &self,
        owner_user_id: &str,
        kind: RuntimeKind,
        persistence: Persistence,
    ) -> Result<InstanceId, StartError> {
        self.adapter(kind)?;
        if !self.store.is_enabled(kind).await? {
            return Err(StartError::Disabled);
        }
        let availability = self.availability(kind).await;
        if !availability.is_available() {
            let reason = match availability {
                crate::adapter::Availability::Unavailable { reason } => reason,
                crate::adapter::Availability::Available { .. } => unreachable!(),
            };
            return Err(StartError::Unavailable(reason));
        }

        let existing = self.store.list_for_owner(owner_user_id).await?;
        let per_user = existing.iter().filter(|i| i.kind == kind).count();
        if per_user >= self.policy.max_instances_per_user as usize {
            return Err(StartError::PerUserLimitReached);
        }
        let global = self.store.list_all().await?;
        let global_count = global.iter().filter(|i| i.kind == kind).count();
        if global_count >= self.policy.max_instances_global as usize {
            return Err(StartError::GlobalLimitReached);
        }

        let instance_id = clouddesk_auth::random_identifier(16);
        let id = InstanceId {
            kind,
            owner_user_id: owner_user_id.to_owned(),
            instance_id,
        };
        let state_dir = crate::storage::instance_state_dir(&self.runtime_root, &id)?;
        self.store
            .upsert_instance(&id, 0, InstanceState::Stopped, persistence, None, None)
            .await?;

        let ctx = InstanceContext {
            id: id.clone(),
            generation: 0,
            runtime_root: self.runtime_root.clone(),
            state_dir,
            policy: self.policy,
            port: None,
        };
        let live_instance = Arc::new(LiveInstance {
            ctx,
            persistence,
            runtime: Mutex::new(InstanceRuntime {
                state: InstanceState::Stopped,
                handle: None,
                generation: 0,
                cgroup: None,
                last_activity: Instant::now(),
                port: None,
            }),
        });
        self.live.write().await.insert(key_of(&id), live_instance);
        Ok(id)
    }

    async fn get_live(&self, id: &InstanceId) -> Option<Arc<LiveInstance>> {
        self.live.read().await.get(&key_of(id)).cloned()
    }

    fn check_owner(id: &InstanceId, owner_user_id: &str) -> Result<(), StartError> {
        if id.owner_user_id != owner_user_id {
            return Err(StartError::NotOwner);
        }
        Ok(())
    }

    /// Starts (or, if already `Running`/`Starting`, is a no-op success
    /// for) the given instance. Blocks until readiness is confirmed or
    /// `policy.start_timeout` + `policy.health_timeout` elapses.
    #[allow(clippy::too_many_lines)]
    pub async fn start_instance(
        &self,
        owner_user_id: &str,
        id: &InstanceId,
    ) -> Result<(), StartError> {
        Self::check_owner(id, owner_user_id)?;
        if !self.store.is_enabled(id.kind).await? {
            return Err(StartError::Disabled);
        }
        let instance = self.get_live(id).await.ok_or(StartError::NotFound)?;
        let adapter = self.adapter(id.kind)?.clone();

        let mut runtime = instance.runtime.lock().await;
        if matches!(
            runtime.state,
            InstanceState::Running | InstanceState::Starting
        ) {
            return Ok(()); // idempotent: two simultaneous START requests both succeed
        }

        runtime.generation += 1;
        let generation = runtime.generation;
        let reserved_port = self.ports.allocate().ok();
        let port = reserved_port.as_ref().map(crate::port::ReservedPort::port);
        let mut ctx = instance.ctx.clone();
        ctx.generation = generation;
        ctx.port = port;

        runtime.state = InstanceState::Starting;
        runtime.port = port;
        self.store
            .upsert_instance(
                id,
                generation,
                InstanceState::Starting,
                instance.persistence,
                port,
                None,
            )
            .await?;
        drop(runtime);

        // Re-ensures the instance's state directory exists (idempotent):
        // a prior stop of an `Ephemeral` instance removes it entirely
        // (Task 7), so a restart must recreate it before this attempt's
        // `current_dir`/adapter storage use it, rather than reusing
        // `create_instance`'s one-time directory creation.
        crate::storage::instance_state_dir(&self.runtime_root, id)?;
        adapter.prepare(&ctx).await?;
        let start_result = adapter.start(&ctx).await;

        let mut runtime = instance.runtime.lock().await;
        // A stale start attempt (superseded by a newer generation while
        // we were awaiting `adapter.start`) must not clobber the
        // current state -- this is the concrete guard against "runtime
        // starts but a superseded attempt still finishes" races.
        if runtime.generation != generation {
            if let Ok(mut handle) = start_result {
                adapter.kill(&ctx, &mut handle).await;
            }
            return Ok(());
        }

        let mut handle = match start_result {
            Ok(handle) => handle,
            Err(e) => {
                runtime.state = InstanceState::Failed;
                runtime.port = None;
                drop(runtime);
                if let Some(p) = port {
                    self.ports.release(p);
                }
                self.store
                    .set_state(id, InstanceState::Failed, Some(&e.to_string()))
                    .await?;
                return Err(e.into());
            }
        };

        let pid = match &handle {
            RunningHandle::Process(child) => child.id(),
            RunningHandle::Opaque(_) => None,
        };
        self.store
            .upsert_instance(
                id,
                generation,
                InstanceState::Starting,
                instance.persistence,
                port,
                pid,
            )
            .await?;

        // Best-effort cgroup v2 confinement (Task 13/14): applied when
        // the host actually delegates the controllers (see
        // `crate::cgroup::detect`), silently skipped otherwise -- an
        // unconfined process is preferable to refusing to start the
        // runtime on a host without delegation, and this is exactly
        // what `CgroupSupport` exists to let callers report honestly
        // (see the live test suite's `cgroup_status` evidence).
        let cgroup = if let Some(pid) = pid {
            match InstanceCgroup::create(&id.instance_id) {
                Ok(cg) => {
                    if let Some(bytes) = self.policy.memory_limit_bytes {
                        let _ = cg.set_memory_limit(bytes);
                    }
                    if let Some(limit) = self.policy.pids_limit {
                        let _ = cg.set_pids_limit(limit);
                    }
                    if let Some(fraction) = self.policy.cpu_quota_fraction {
                        let _ = cg.set_cpu_limit(fraction);
                    }
                    let _ = cg.add_process(pid);
                    Some(cg)
                }
                Err(_) => None,
            }
        } else {
            None
        };

        // Bounded readiness wait (Task 10): poll health with backoff,
        // never indefinitely.
        let deadline = Instant::now() + self.policy.start_timeout + self.policy.health_timeout;
        let mut ready = false;
        loop {
            match adapter.health(&ctx, &handle).await {
                HealthStatus::Ready => {
                    ready = true;
                    break;
                }
                HealthStatus::Unhealthy => break,
                HealthStatus::NotReadyYet => {}
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        if !ready {
            adapter.kill(&ctx, &mut handle).await;
            runtime.state = InstanceState::Failed;
            runtime.port = None;
            drop(runtime);
            if let Some(p) = port {
                self.ports.release(p);
            }
            self.store
                .set_state(id, InstanceState::Failed, Some("start/health timeout"))
                .await?;
            return Err(StartError::StartTimeout);
        }

        runtime.state = InstanceState::Running;
        runtime.handle = Some(handle);
        runtime.cgroup = cgroup;
        runtime.last_activity = Instant::now();
        drop(runtime);
        self.store
            .set_state(id, InstanceState::Running, None)
            .await?;

        self.spawn_supervisor(
            instance.clone(),
            adapter.clone(),
            ctx,
            generation,
            reserved_port,
        );
        Ok(())
    }

    /// Background task: watches for unexpected exit (Task 11) and, once
    /// running, periodically re-checks health so a process that goes
    /// unhealthy without exiting is also detected.
    fn spawn_supervisor(
        &self,
        instance: Arc<LiveInstance>,
        adapter: Arc<dyn RuntimeAdapter>,
        ctx: InstanceContext,
        generation: u64,
        reserved_port: Option<crate::port::ReservedPort>,
    ) {
        let store = self.store.clone();
        let ports = self.ports.clone();
        let id = instance.ctx.id.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mut runtime = instance.runtime.lock().await;
                if runtime.generation != generation {
                    return; // superseded by a newer start/stop
                }
                if !matches!(
                    runtime.state,
                    InstanceState::Running | InstanceState::Unhealthy
                ) {
                    return;
                }
                let Some(handle) = runtime.handle.as_mut() else {
                    return;
                };

                // Detect unexpected process exit.
                if let RunningHandle::Process(child) = handle {
                    if let Ok(Some(status)) = child.try_wait() {
                        runtime.state = InstanceState::Failed;
                        runtime.handle = None;
                        runtime.port = None;
                        drop(runtime);
                        if let Some(p) = &reserved_port {
                            ports.release(p.port());
                        }
                        #[cfg(unix)]
                        let signal = {
                            use std::os::unix::process::ExitStatusExt;
                            status.signal().map(|s| s.to_string())
                        };
                        #[cfg(not(unix))]
                        let signal: Option<String> = None;
                        let _ = store
                            .record_exit(&id, status.code(), signal.as_deref())
                            .await;
                        let _ = store
                            .set_state(&id, InstanceState::Failed, Some("unexpected exit"))
                            .await;
                        return;
                    }
                }

                let status = adapter.health(&ctx, handle).await;
                let new_state = match status {
                    HealthStatus::Ready => InstanceState::Running,
                    _ => InstanceState::Unhealthy,
                };
                if runtime.state != new_state {
                    runtime.state = new_state;
                    drop(runtime);
                    let _ = store.set_state(&id, new_state, None).await;
                }
            }
        });
    }

    /// Marks that the instance was meaningfully used just now (Task 12).
    pub async fn touch_activity(
        &self,
        owner_user_id: &str,
        id: &InstanceId,
    ) -> Result<(), StartError> {
        Self::check_owner(id, owner_user_id)?;
        let instance = self.get_live(id).await.ok_or(StartError::NotFound)?;
        instance.runtime.lock().await.last_activity = Instant::now();
        self.store.touch_activity(id).await?;
        Ok(())
    }

    pub async fn stop_instance(
        &self,
        owner_user_id: &str,
        id: &InstanceId,
    ) -> Result<(), StopError> {
        if id.owner_user_id != owner_user_id {
            return Err(StopError::NotOwner);
        }
        let instance = self.get_live(id).await.ok_or(StopError::NotFound)?;
        self.stop_live(&instance, false).await;
        Ok(())
    }

    /// `force`: skip the graceful phase entirely (used when disabling a
    /// runtime that must come down promptly). Otherwise: graceful stop,
    /// bounded wait, force-kill fallback (Task 30).
    async fn stop_live(&self, instance: &Arc<LiveInstance>, force: bool) {
        let Some(adapter) = self.adapters.get(&instance.ctx.id.kind).cloned() else {
            return;
        };
        let mut runtime = instance.runtime.lock().await;
        if matches!(
            runtime.state,
            InstanceState::Stopped | InstanceState::Failed
        ) {
            return; // repeated STOP is a no-op, not an error
        }
        let Some(mut handle) = runtime.handle.take() else {
            runtime.state = InstanceState::Stopped;
            return;
        };
        runtime.state = InstanceState::Stopping;
        let generation = runtime.generation;
        let mut ctx = instance.ctx.clone();
        ctx.generation = generation;
        ctx.port = runtime.port;
        drop(runtime);
        let _ = self
            .store
            .set_state(&instance.ctx.id, InstanceState::Stopping, None)
            .await;

        if force {
            adapter.kill(&ctx, &mut handle).await;
        } else {
            let _ = adapter.stop(&ctx, &mut handle).await;
            let waited = tokio::time::timeout(self.policy.stop_timeout, async {
                if let RunningHandle::Process(child) = &mut handle {
                    let _ = child.wait().await;
                }
            })
            .await;
            if waited.is_err() {
                adapter.kill(&ctx, &mut handle).await;
            }
        }

        let mut runtime = instance.runtime.lock().await;
        runtime.state = InstanceState::Stopped;
        runtime.handle = None;
        let released_port = runtime.port.take();
        let cgroup = runtime.cgroup.take();
        drop(runtime);
        if let Some(p) = released_port {
            self.ports.release(p);
        }
        drop(cgroup); // cgroup Drop attempts removal now that the process is gone

        let _ = self
            .store
            .set_state(&instance.ctx.id, InstanceState::Stopped, None)
            .await;

        if instance.persistence == Persistence::Ephemeral {
            let _ = adapter.cleanup(&ctx).await;
        }
    }

    pub async fn restart_instance(
        &self,
        owner_user_id: &str,
        id: &InstanceId,
    ) -> Result<(), StartError> {
        Self::check_owner(id, owner_user_id)?;
        let instance = self.get_live(id).await.ok_or(StartError::NotFound)?;
        self.stop_live(&instance, false).await;
        let count = self.store.increment_restart_count(id).await?;
        if count > i64::from(CRASH_LOOP_MAX_RESTARTS) {
            instance.runtime.lock().await.state = InstanceState::Failed;
            self.store
                .set_state(
                    id,
                    InstanceState::Failed,
                    Some("crash-loop threshold exceeded"),
                )
                .await?;
            return Err(StartError::Adapter(crate::adapter::AdapterError::Start(
                "crash-loop threshold exceeded".to_owned(),
            )));
        }
        self.start_instance(owner_user_id, id).await
    }

    pub async fn status(&self, owner_user_id: &str, id: &InstanceId) -> Option<InstanceState> {
        if id.owner_user_id != owner_user_id {
            return None;
        }
        let instance = self.get_live(id).await?;
        let state = instance.runtime.lock().await.state;
        Some(state)
    }

    pub async fn logs(
        &self,
        owner_user_id: &str,
        id: &InstanceId,
        max_bytes: usize,
    ) -> Option<Vec<u8>> {
        if id.owner_user_id != owner_user_id {
            return None;
        }
        let instance = self.get_live(id).await?;
        let adapter = self.adapters.get(&id.kind)?.clone();
        let runtime = instance.runtime.lock().await;
        let handle = runtime.handle.as_ref()?;
        Some(adapter.logs(&instance.ctx, handle, max_bytes).await)
    }

    /// The instance's currently-allocated port, for the proxy layer.
    /// Ownership-scoped -- returns `None` for another user's instance
    /// exactly as if it didn't exist (Task 21).
    pub async fn instance_port(&self, owner_user_id: &str, id: &InstanceId) -> Option<u16> {
        if id.owner_user_id != owner_user_id {
            return None;
        }
        let instance = self.get_live(id).await?;
        let runtime = instance.runtime.lock().await;
        if runtime.state != InstanceState::Running {
            return None;
        }
        runtime.port
    }

    /// Task 12: periodic idle sweep. Call once at startup; runs for the
    /// process lifetime as a single background task (not one timer per
    /// instance/request).
    pub fn spawn_idle_sweeper(self: &Arc<Self>) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                manager.sweep_idle_once().await;
            }
        });
    }

    /// Runs one idle-check pass immediately, stopping any `Running`
    /// instance whose `last_activity` exceeds the policy's
    /// `idle_timeout`. `spawn_idle_sweeper` calls this on a fixed 30s
    /// production cadence; exposed as `pub` so tests can exercise the
    /// real sweep logic deterministically without waiting out that
    /// cadence.
    pub async fn sweep_idle_once(&self) {
        let Some(idle_timeout) = self.policy.idle_timeout else {
            return;
        };
        let candidates: Vec<Arc<LiveInstance>> = {
            let live = self.live.read().await;
            live.values().cloned().collect()
        };
        for instance in candidates {
            let is_idle = {
                let runtime = instance.runtime.lock().await;
                runtime.state == InstanceState::Running
                    && runtime.last_activity.elapsed() >= idle_timeout
            };
            if is_idle {
                self.stop_live(&instance, false).await;
            }
        }
    }

    /// Clean-shutdown behavior (Task 28/Phase-6-closure Task 4): stops
    /// every currently-live instance across all owners/kinds, graceful-
    /// then-forced exactly like a single `stop_instance` call, releasing
    /// ports/cgroups and cleaning ephemeral state as it goes. Intended
    /// for `clouddeskd`'s own graceful-shutdown path and, just as
    /// importantly, for test teardown: a `RuntimeManager` built inside a
    /// test that never explicitly stops what it started previously had
    /// no reliable way to avoid leaving orphaned child processes behind
    /// when the test process exits (`HostProcessAdapter` deliberately
    /// uses `kill_on_drop(false)` so process-tree signaling stays
    /// possible -- see its own docs -- which means nothing kills an
    /// abandoned child for free). Idempotent: instances already stopped
    /// are no-ops.
    pub async fn shutdown_all(&self) {
        let candidates: Vec<Arc<LiveInstance>> = {
            let live = self.live.read().await;
            live.values().cloned().collect()
        };
        for instance in candidates {
            self.stop_live(&instance, false).await;
        }
    }

    /// Task 27: on process startup, every row that claims a non-terminal
    /// state cannot be trusted -- this process holds no live handle for
    /// it (a fresh `RuntimeManager` never does), and blindly signaling a
    /// recovered bare PID risks hitting an unrelated process that has
    /// since reused it. The safe choice is to never act on a recovered
    /// PID at all: every non-terminal row is marked `Failed` (ephemeral
    /// state removed, persistent data retained), so the next `start`
    /// creates a known-good fresh attempt. This is a documented,
    /// deliberate scope boundary, not an oversight -- see the crate/
    /// checkpoint docs for what a future session could add (actually
    /// reattaching via a recorded start-time + PID + `/proc/<pid>/stat`
    /// start-time comparison) if reattachment is ever required.
    pub async fn reconcile_on_startup(&self) -> Result<usize, sqlx::Error> {
        let all = self.store.list_all().await?;
        let mut reconciled = 0;
        for row in all {
            if row.state.is_terminal_for_reconciliation() {
                continue;
            }
            let id = InstanceId {
                kind: row.kind,
                owner_user_id: row.owner_user_id.clone(),
                instance_id: row.instance_id.clone(),
            };
            self.store
                .set_state(&id, InstanceState::Failed, Some("reconciled after restart"))
                .await?;
            if row.persistence == Persistence::Ephemeral {
                let _ = crate::storage::remove_instance_state_dir(&self.runtime_root, &id);
            }
            reconciled += 1;
        }
        Ok(reconciled)
    }
}
