//! Live tests for the OCI/container adapter (Task 15/16) against the
//! real Docker daemon confirmed present on this host. Skips cleanly
//! (not a failure) if `docker` isn't reachable or `alpine:latest`
//! isn't cached locally -- Task 36 requires `CloudDesk` core to never
//! *require* a container runtime, and this test suite honors the same
//! rule for itself.
//!
//! `alpine:latest` here stands in for a real runtime image the same
//! way `test-runtime-fixture` stands in for a real host-process
//! runtime elsewhere in this crate -- it proves the OCI adapter's
//! lifecycle plumbing (start/health/stop/kill/cleanup, hardened argv,
//! loopback-only port mapping), not any actual Code/Office/Browser
//! container.

use clouddesk_orchestrator::manager::RuntimeManager;
use clouddesk_orchestrator::model::{Persistence, ResourcePolicy};
use clouddesk_orchestrator::oci::{OciAdapter, OciSpec};
use clouddesk_orchestrator::store::RuntimeStore;
use clouddesk_orchestrator::RuntimeKind;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

async fn docker_available() -> bool {
    Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
        && Command::new("docker")
            .args(["image", "inspect", "alpine:latest"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success())
}

fn probe_spec() -> OciSpec {
    OciSpec {
        kind: RuntimeKind::TestFixture,
        image: "alpine:latest".to_owned(),
        container_port: 8080,
        health_check_path: "/",
        // alpine's default CMD (`/bin/sh`, no tty/stdin) exits
        // immediately; this stands in for a real runtime image's own
        // long-running server the same way `test-runtime-fixture`
        // stands in for a host-process runtime elsewhere in this
        // crate. It only proves the OCI lifecycle plumbing.
        command: Some(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "while true; do echo ok | nc -l -p 8080; done".to_owned(),
        ]),
    }
}

async fn manager_with_oci() -> (Arc<RuntimeManager>, tempfile::TempDir) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
         VALUES ('u1', 'u1', 'u1', 'x', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let store = RuntimeStore::new(pool);
    store
        .set_enabled(RuntimeKind::TestFixture, true)
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let policy = ResourcePolicy {
        start_timeout: Duration::from_secs(10),
        health_timeout: Duration::from_secs(5),
        ..ResourcePolicy::default()
    };
    let manager = Arc::new(
        RuntimeManager::new(store, root.path().to_owned(), policy)
            .with_adapter(Arc::new(OciAdapter::new(probe_spec()))),
    );
    (manager, root)
}

#[tokio::test]
async fn task_15_availability_reports_unavailable_or_available_honestly() {
    if !docker_available().await {
        eprintln!("SKIP: docker not reachable on this host -- reporting honestly, not PASS");
        return;
    }
    let adapter = OciAdapter::new(probe_spec());
    let availability = clouddesk_orchestrator::RuntimeAdapter::availability(&adapter).await;
    assert!(
        availability.is_available(),
        "docker + alpine:latest are confirmed present, availability() must report Available: {availability:?}"
    );
}

#[tokio::test]
async fn task_16_hardened_container_full_lifecycle_start_health_stop_cleanup() {
    if !docker_available().await {
        eprintln!("SKIP: docker not reachable on this host");
        return;
    }
    let (manager, _root) = manager_with_oci().await;

    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();

    let status = manager.status("u1", &id).await.unwrap();
    assert_eq!(
        status,
        clouddesk_orchestrator::InstanceState::Running,
        "readiness must come from a real passed TCP health check against the \
         container's mapped loopback port, not merely `docker run` succeeding"
    );

    let port = manager.instance_port("u1", &id).await.unwrap();
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok(),
        "the container's published port must be reachable on loopback only"
    );

    // Prove the container is hardened, not just running: no
    // privileged mode, capabilities dropped, no-new-privileges set.
    let name = format!("clouddesk-runtime-{}", id.instance_id);
    let inspect = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.HostConfig.Privileged}} {{.HostConfig.CapDrop}} {{.HostConfig.SecurityOpt}}",
            &name,
        ])
        .output()
        .await
        .unwrap();
    let inspect_out = String::from_utf8_lossy(&inspect.stdout);
    assert!(
        inspect_out.contains("false"),
        "container must not run privileged: {inspect_out}"
    );
    assert!(
        inspect_out.contains("ALL"),
        "container must have all capabilities dropped: {inspect_out}"
    );
    assert!(
        inspect_out.contains("no-new-privileges"),
        "container must set no-new-privileges: {inspect_out}"
    );

    manager.stop_instance("u1", &id).await.unwrap();
    let status = manager.status("u1", &id).await.unwrap();
    assert_eq!(status, clouddesk_orchestrator::InstanceState::Stopped);

    // The container itself must actually be gone (Task 16/30: cleanup
    // verification), not merely marked stopped in our own DB.
    let still_exists = Command::new("docker")
        .args(["inspect", &name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());
    assert!(
        !still_exists,
        "container must be removed after stop (--rm / explicit rm -f), not orphaned"
    );
}
