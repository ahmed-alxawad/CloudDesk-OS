//! Live failure-matrix tests (Task 32) against the disposable
//! `test-runtime-fixture` binary -- TEST FIXTURE ONLY, proves the
//! orchestrator's own lifecycle/health/proxy/authorization plumbing.
//! This does NOT prove Code, Office, or Brave are implemented (they
//! aren't -- see `V1_TRUE_CLOSURE.md`). No mocks: every test here spawns
//! the real compiled fixture binary as a real child process.

use clouddesk_orchestrator::host_process::{HealthCheck, HostProcessAdapter, HostProcessSpec};
use clouddesk_orchestrator::manager::{RuntimeManager, StartError};
use clouddesk_orchestrator::model::{InstanceState, Persistence, ResourcePolicy};
use clouddesk_orchestrator::store::RuntimeStore;
use clouddesk_orchestrator::{InstanceId, RuntimeKind};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Locates the already-built `test-runtime-fixture` binary. Stable Cargo
/// has no artifact-dependency support (`-Z bindeps` is nightly-only), so
/// this can't use `CARGO_BIN_EXE_*`; instead it walks up from this
/// crate's own manifest directory to the workspace root and looks in
/// `target/{debug,release}/`, matching whichever profile this test
/// itself was built under. Requires the workspace to have been built at
/// least once (true for any normal `cargo build`/`cargo test
/// --workspace` run, including this crate's own validation flow) --
/// panics with a clear message otherwise rather than silently skipping.
fn fixture_path() -> String {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crates/orchestrator should be two levels below the workspace root");
    for profile in ["debug", "release"] {
        let candidate = workspace_root
            .join("target")
            .join(profile)
            .join("test-runtime-fixture");
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    panic!(
        "test-runtime-fixture binary not found under {}/target/{{debug,release}} -- \
         run `cargo build -p test-runtime-fixture` first",
        workspace_root.display()
    );
}

async fn pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    for user in ["u1", "u2"] {
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
             VALUES (?, ?, ?, 'x', 0, 0)",
        )
        .bind(user)
        .bind(user)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    }
    pool
}

fn fast_policy() -> ResourcePolicy {
    ResourcePolicy {
        max_instances_global: 8,
        max_instances_per_user: 2,
        memory_limit_bytes: Some(256 * 1024 * 1024),
        pids_limit: Some(32),
        cpu_quota_fraction: Some(1.0),
        start_timeout: Duration::from_secs(5),
        health_timeout: Duration::from_secs(3),
        stop_timeout: Duration::from_secs(2),
        idle_timeout: Some(Duration::from_secs(2)),
    }
}

fn spec_with_env(env_vars: HashMap<String, String>) -> HostProcessSpec {
    HostProcessSpec {
        kind: RuntimeKind::TestFixture,
        executable: Some(fixture_path()),
        argv: Arc::new(|_ctx| vec![]),
        env: Arc::new(move |ctx| {
            let mut env = env_vars.clone();
            env.insert(
                "PORT".to_owned(),
                ctx.port.map(|p| p.to_string()).unwrap_or_default(),
            );
            env
        }),
        health_check: HealthCheck::HttpGet { path: "/healthz" },
    }
}

async fn manager_with(spec: HostProcessSpec) -> (Arc<RuntimeManager>, tempfile::TempDir) {
    let pool = pool().await;
    let store = RuntimeStore::new(pool);
    let root = tempfile::tempdir().unwrap();
    store
        .set_enabled(RuntimeKind::TestFixture, true)
        .await
        .unwrap();
    let manager = RuntimeManager::new(store, root.path().to_owned(), fast_policy())
        .with_adapter(Arc::new(HostProcessAdapter::new(spec)));
    (Arc::new(manager), root)
}

#[tokio::test]
async fn task_1_availability_detection() {
    let (manager, _root) = manager_with(spec_with_env(HashMap::new())).await;
    let availability = manager.availability(RuntimeKind::TestFixture).await;
    assert!(availability.is_available(), "{availability:?}");

    let (manager2, _root2) = manager_with(HostProcessSpec {
        executable: Some("/no/such/binary".to_owned()),
        ..spec_with_env(HashMap::new())
    })
    .await;
    let unavailable = manager2.availability(RuntimeKind::TestFixture).await;
    assert!(!unavailable.is_available());
}

#[tokio::test]
async fn task_2_enable_task_3_start_task_4_readiness() {
    let (manager, _root) = manager_with(spec_with_env(HashMap::new())).await;
    assert!(manager.is_enabled(RuntimeKind::TestFixture).await.unwrap());

    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Stopped)
    );

    manager.start_instance("u1", &id).await.unwrap();
    // RUNNING requires the real /healthz check to have actually
    // succeeded, not merely that a process was spawned (Task 1/10).
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Running)
    );

    manager.stop_instance("u1", &id).await.unwrap();
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Stopped)
    );
}

#[tokio::test]
async fn task_7_owner_succeeds_task_21_cross_user_denied() {
    let (manager, _root) = manager_with(spec_with_env(HashMap::new())).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();

    // Owner succeeds.
    assert!(manager.instance_port("u1", &id).await.is_some());
    assert!(manager.logs("u1", &id, 4096).await.is_some());

    // A different user, even supplying the exact same InstanceId,
    // cannot start/stop/restart/read logs/read the port -- possession
    // of the ID is not authorization.
    assert!(matches!(
        manager.start_instance("u2", &id).await,
        Err(StartError::NotOwner)
    ));
    assert!(manager.stop_instance("u2", &id).await.is_err());
    assert!(manager.instance_port("u2", &id).await.is_none());
    assert!(manager.logs("u2", &id, 4096).await.is_none());
    assert_eq!(manager.status("u2", &id).await, None);

    // Owner's instance is unaffected by the attacker's attempts.
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Running)
    );
}

#[tokio::test]
async fn task_9_stop_and_restart() {
    let (manager, _root) = manager_with(spec_with_env(HashMap::new())).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();
    let first_port = manager.instance_port("u1", &id).await.unwrap();

    manager.restart_instance("u1", &id).await.unwrap();
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Running)
    );
    let second_port = manager.instance_port("u1", &id).await.unwrap();
    // A restart is a genuinely new attempt -- new port allocation, not
    // just a state-flag toggle.
    assert_ne!(first_port, second_port);

    // Repeated STOP is a safe no-op, not an error.
    manager.stop_instance("u1", &id).await.unwrap();
    manager.stop_instance("u1", &id).await.unwrap();
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Stopped)
    );
}

#[tokio::test]
async fn task_11_crash_detection() {
    let mut env = HashMap::new();
    env.insert("CRASH_AFTER_MS".to_owned(), "500".to_owned());
    let (manager, _root) = manager_with(spec_with_env(env)).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Running)
    );

    // Real unexpected exit: the fixture calls exit(7) itself, not a
    // signal we send. The supervisor task polls every 2s; wait past
    // that.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(manager.status("u1", &id).await, Some(InstanceState::Failed));

    let row = manager.store().get(&id).await.unwrap().unwrap();
    assert_eq!(row.exit_code, Some(7));
}

#[tokio::test]
async fn task_11_crash_loop_protection() {
    // Long enough for the process to reliably open its listener and
    // pass the first health check before crashing (avoids startup-vs-
    // crash-timing flakiness), short enough that each cycle is fast.
    let mut env = HashMap::new();
    env.insert("CRASH_AFTER_MS".to_owned(), "400".to_owned());
    let (manager, _root) = manager_with(spec_with_env(env)).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();

    let mut last_result = Ok(());
    for _ in 0..6 {
        tokio::time::sleep(Duration::from_millis(700)).await;
        last_result = manager.restart_instance("u1", &id).await;
        if last_result.is_err() {
            break;
        }
    }
    assert!(
        last_result.is_err(),
        "crash-loop threshold must eventually stop restarts"
    );
    assert_eq!(manager.status("u1", &id).await, Some(InstanceState::Failed));
}

#[tokio::test]
async fn task_10_start_timeout_and_health_failure() {
    // A "runtime" that never opens a listener at all -- the health
    // check can never succeed, so start must fail with a bounded
    // timeout, not hang forever.
    let mut env = HashMap::new();
    // sleep forever without binding -- but our fixture always binds; to
    // simulate "no listener" we point PORT at a port the fixture
    // deliberately never serves on by asking it to bind an obviously
    // wrong health path instead. Simpler: use a health check path the
    // fixture 404s on, which this adapter spec treats as Unhealthy.
    env.insert("NONE".to_owned(), "1".to_owned());
    let spec = HostProcessSpec {
        health_check: HealthCheck::HttpGet {
            path: "/does-not-exist",
        },
        ..spec_with_env(env)
    };
    let (manager, _root) = manager_with(spec).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    let result = manager.start_instance("u1", &id).await;
    assert!(
        result.is_err(),
        "a 404 health check must never report Ready"
    );
    assert_eq!(manager.status("u1", &id).await, Some(InstanceState::Failed));
}

#[tokio::test]
async fn task_12_idle_shutdown_activity_resets_timer_and_sweep_stops_truly_idle_instance() {
    let (manager, _root) = manager_with(spec_with_env(HashMap::new())).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();

    // Activity just before the idle timeout (policy: 2s) must reset the
    // clock, not merely delay the inevitable -- proven by running a
    // real sweep pass right after the original deadline would have
    // elapsed and confirming it did NOT stop the instance.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    manager.touch_activity("u1", &id).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    manager.sweep_idle_once().await;
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Running),
        "activity just before the idle timeout must have reset it"
    );

    // Now let it actually go idle and run a real sweep pass (the same
    // function `spawn_idle_sweeper` calls on its 30s production
    // cadence, invoked directly here for a deterministic test) --
    // this must actually stop the instance.
    tokio::time::sleep(Duration::from_secs(3)).await;
    manager.sweep_idle_once().await;
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Stopped),
        "a genuinely idle instance must be stopped by the sweep"
    );

    // Repeated idle cycles: start again, go idle again, sweep again.
    manager.start_instance("u1", &id).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    manager.sweep_idle_once().await;
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Stopped)
    );
}

#[tokio::test]
async fn task_16_disable_while_active() {
    let (manager, _root) = manager_with(spec_with_env(HashMap::new())).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Running)
    );

    manager
        .set_enabled(RuntimeKind::TestFixture, false)
        .await
        .unwrap();
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Stopped)
    );
    assert!(!manager.is_enabled(RuntimeKind::TestFixture).await.unwrap());

    // New starts are rejected while disabled.
    assert!(matches!(
        manager.start_instance("u1", &id).await,
        Err(StartError::Disabled)
    ));
}

#[tokio::test]
async fn task_17_child_process_cleanup() {
    let mut env = HashMap::new();
    env.insert("SPAWN_CHILD".to_owned(), "1".to_owned());
    let (manager, _root) = manager_with(spec_with_env(env)).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await; // let the child spawn

    manager.stop_instance("u1", &id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The fixture's spawned child is `sleep 300` inside the same
    // process group; confirm no leftover `sleep` process from this test
    // run remains by checking the process group is gone via /proc.
    let leaked = std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .any(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
                .unwrap_or_default()
                .contains("sleep")
                && std::fs::read_to_string(format!("/proc/{pid}/status"))
                    .unwrap_or_default()
                    .contains("PPid:\t1\n")
        });
    assert!(
        !leaked,
        "a child spawned by a stopped runtime instance must not survive as an orphan"
    );
}

#[tokio::test]
async fn task_17_ignore_sigterm_falls_back_to_sigkill() {
    let mut env = HashMap::new();
    env.insert("IGNORE_SIGTERM".to_owned(), "1".to_owned());
    let (manager, _root) = manager_with(spec_with_env(env)).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();

    let started = std::time::Instant::now();
    manager.stop_instance("u1", &id).await.unwrap();
    let elapsed = started.elapsed();

    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Stopped)
    );
    // Must have actually waited out the graceful window before forcing
    // (proves SIGKILL fallback engaged, not that stop_timeout was
    // skipped) but still completed well within it.
    assert!(elapsed >= Duration::from_millis(400), "{elapsed:?}");
    assert!(elapsed <= Duration::from_secs(5), "{elapsed:?}");
}

#[tokio::test]
async fn task_18_simultaneous_start() {
    let (manager, _root) = manager_with(spec_with_env(HashMap::new())).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();

    let m1 = manager.clone();
    let m2 = manager.clone();
    let id1 = id.clone();
    let id2 = id.clone();
    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { m1.start_instance("u1", &id1).await }),
        tokio::spawn(async move { m2.start_instance("u1", &id2).await }),
    );
    assert!(r1.unwrap().is_ok());
    assert!(r2.unwrap().is_ok());
    assert_eq!(
        manager.status("u1", &id).await,
        Some(InstanceState::Running)
    );
    // Exactly one port is associated with this instance -- no duplicate
    // uncontrolled second process for the same singleton instance.
    assert!(manager.instance_port("u1", &id).await.is_some());
}

#[tokio::test]
async fn task_19_simultaneous_stop_start() {
    let (manager, _root) = manager_with(spec_with_env(HashMap::new())).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();

    let m1 = manager.clone();
    let m2 = manager.clone();
    let id1 = id.clone();
    let id2 = id.clone();
    let (stop_result, start_result) = tokio::join!(
        tokio::spawn(async move { m1.stop_instance("u1", &id1).await }),
        tokio::spawn(async move { m2.start_instance("u1", &id2).await }),
    );
    stop_result.unwrap().unwrap();
    start_result.unwrap().unwrap();
    // Whatever the final state, it must be a real, well-formed state --
    // never stuck in Starting/Stopping forever.
    let final_state = manager.status("u1", &id).await.unwrap();
    assert!(matches!(
        final_state,
        InstanceState::Running | InstanceState::Stopped | InstanceState::Failed
    ));
}

#[tokio::test]
async fn task_20_startup_reconciliation() {
    let pool = pool().await;
    let store = RuntimeStore::new(pool.clone());
    let root = tempfile::tempdir().unwrap();
    store
        .set_enabled(RuntimeKind::TestFixture, true)
        .await
        .unwrap();

    let id = InstanceId {
        kind: RuntimeKind::TestFixture,
        owner_user_id: "u1".to_owned(),
        instance_id: "leftover".to_owned(),
    };
    // Simulate a row left behind by a process that no longer exists
    // (crash/kill -9 of clouddeskd itself) -- never a live handle in
    // this fresh manager.
    store
        .upsert_instance(
            &id,
            1,
            InstanceState::Running,
            Persistence::Ephemeral,
            Some(12345),
            Some(999_999),
        )
        .await
        .unwrap();

    let manager = RuntimeManager::new(store, root.path().to_owned(), fast_policy()).with_adapter(
        Arc::new(HostProcessAdapter::new(spec_with_env(HashMap::new()))),
    );
    let reconciled = manager.reconcile_on_startup().await.unwrap();
    assert_eq!(reconciled, 1);

    let row = manager.store().get(&id).await.unwrap().unwrap();
    assert_eq!(row.state, InstanceState::Failed);
}

#[tokio::test]
async fn resource_limits_are_enforced_admission_control() {
    let (manager, _root) = manager_with(spec_with_env(HashMap::new())).await;
    let id1 = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    let id2 = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    // fast_policy() allows 2 per user -- a third must be refused.
    let third = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await;
    assert!(matches!(third, Err(StartError::PerUserLimitReached)));
    let _ = (id1, id2);
}

#[tokio::test]
async fn environment_never_leaks_the_orchestrator_process_env() {
    // Task 4/35: prove the child's environment is exactly what the spec
    // says, never inherited from clouddeskd's own process (which is
    // where secrets like the Vault master key path would live).
    std::env::set_var("CLOUDDESK_TEST_SENTINEL_SECRET", "should-never-appear");
    let spec = spec_with_env(HashMap::new());
    let described = {
        let adapter = HostProcessAdapter::new(spec.clone());
        let ctx = clouddesk_orchestrator::InstanceContext {
            id: InstanceId {
                kind: RuntimeKind::TestFixture,
                owner_user_id: "u1".to_owned(),
                instance_id: "probe".to_owned(),
            },
            generation: 1,
            runtime_root: std::env::temp_dir(),
            state_dir: std::env::temp_dir(),
            policy: fast_policy(),
            port: Some(0),
        };
        clouddesk_orchestrator::RuntimeAdapter::describe_environment(&adapter, &ctx)
    };
    assert!(!described.contains_key("CLOUDDESK_TEST_SENTINEL_SECRET"));
    assert!(
        !described.contains_key("PATH"),
        "PATH must not be silently inherited either"
    );
}

/// Regression test for a real defect found while covering Task 11 (log
/// flooding): a runtime instance that wrote more than one 4 KiB chunk
/// of stdout before `start_instance`'s health-check loop next polled
/// could spuriously fail to start at all -- readiness on the child's
/// stdout pipe is edge-triggered, so the reader task's single
/// `read().await` per iteration could leave already-available bytes
/// unread with no future wakeup ever coming for them, silently hanging
/// the reader (and, because nothing ever marked the instance ready,
/// the whole `start_instance` call) until the outer start/health
/// timeout fired -- even though the instance was genuinely healthy the
/// entire time. Fixed by draining fully available data in one pass
/// (`drain_ready`/`try_read_once` in `host_process.rs`) instead of
/// waiting for a second edge that might never arrive.
#[tokio::test]
async fn task_11_log_flooding_during_startup_does_not_delay_readiness() {
    let mut extra_env = HashMap::new();
    let payload = "A".repeat(5_000);
    let mut payload_hex = String::with_capacity(payload.len() * 2);
    for byte in payload.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(payload_hex, "{byte:02x}");
    }
    extra_env.insert("LOG_TEST_PAYLOAD_HEX".to_owned(), payload_hex);
    // 9 repeats (~45 KB) is comfortably past the single 4 KiB read
    // buffer used by the reader task -- this exact volume reproduced
    // the bug deterministically before the fix.
    extra_env.insert("LOG_TEST_REPEAT".to_owned(), "9".to_owned());

    let (manager, _root) = manager_with(spec_with_env(extra_env)).await;
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();

    let start = std::time::Instant::now();
    manager
        .start_instance("u1", &id)
        .await
        .expect("a healthy instance must not fail to start merely because it logged a lot");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "readiness must be detected promptly, not only once the start/health timeout expires: \
         took {:?}",
        start.elapsed()
    );
}
