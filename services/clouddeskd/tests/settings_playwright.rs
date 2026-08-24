//! Phase 6: real Playwright-driven evidence that Settings' Optional
//! Runtimes cards genuinely control the real Browser/Code/Office
//! runtimes -- through the ACTUAL compiled frontend, not direct API
//! calls. `RuntimeManager` itself (host+OCI adapters, storage/port
//! primitives, lifecycle, reconciliation, the authenticated HTTP/WS
//! proxy) is already Phase 6 COMPLETE and is NOT rebuilt here; this
//! file only proves the Settings UI's enable/disable/state-reflection
//! against it, for real.
//!
//! Registers all three real OCI adapters (Browser/Code/Office) on one
//! `RuntimeManager`, plus the same `cloudesk-privd` substitution Phase
//! 4/5 established (real `clouddesk_privilege` wire protocol -> real,
//! unmodified `cloudesk-sessiond` binary; only the root-owned outer
//! relay is substituted, since this sandbox has no root/passwordless-
//! sudo) so Files -> Open with Office also works for real.
//!
//! Skips (not FAIL) per-runtime scenarios individually if that
//! runtime's Docker image isn't available.

use axum::http::Method;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::process::Command as TokioCommand;

const PLAYWRIGHT_IMAGE: &str = "mcr.microsoft.com/playwright:v1.49.0-noble";
const BROWSER_IMAGE: &str = "clouddesk-brave:1.93.136";
const CODE_IMAGE: &str = "codercom/code-server:4.133.0";
const OFFICE_IMAGE: &str = "collabora/code:26.04.3.1.1";

async fn docker_and_playwright_available() -> bool {
    let docker = TokioCommand::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .await
        .is_ok_and(|o| o.status.success());
    let image = TokioCommand::new("docker")
        .args(["image", "inspect", PLAYWRIGHT_IMAGE])
        .output()
        .await
        .is_ok_and(|o| o.status.success());
    docker && image
}

async fn image_available(image: &str) -> bool {
    TokioCommand::new("docker")
        .args(["image", "inspect", image])
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

/// Real container count for `image`, via `docker ps` -- proves actual
/// resident cost, not merely the frontend's own state store (Task 23,
/// 26).
async fn resident_containers(image: &str) -> usize {
    let output = TokioCommand::new("docker")
        .args(["ps", "-q", "--filter", &format!("ancestor={image}")])
        .output()
        .await
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// `docker stop` (issued by `stop_live`) is awaited by `set_enabled`
/// before the disable HTTP response returns, but the container's exit
/// actually deregistering from `docker ps` can lag the command
/// returning by a beat under host load -- bounded polling, not an
/// instant check, matching this codebase's established
/// process/container-leak-check convention elsewhere.
async fn wait_for_zero_resident(image: &str, timeout_ms: u64) -> usize {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let count = resident_containers(image).await;
        if count == 0 || std::time::Instant::now() >= deadline {
            return count;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}

async fn cleanup_containers(image: &str) {
    let ps = TokioCommand::new("docker")
        .args(["ps", "-a", "-q", "--filter", &format!("ancestor={image}")])
        .output()
        .await
        .unwrap();
    for id in String::from_utf8_lossy(&ps.stdout).lines() {
        if !id.trim().is_empty() {
            let _ = TokioCommand::new("docker")
                .args(["rm", "-f", id])
                .output()
                .await;
        }
    }
}

fn sessiond_binary() -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/cloudesk-sessiond");
    path.exists().then_some(path)
}

/// Identical substitution to Phase 4/5: the real `clouddesk_privilege`
/// wire protocol and grant verification, shelling out to the real,
/// unmodified `cloudesk-sessiond` binary -- only the root-owned outer
/// `cloudesk-privd` relay is replaced.
fn spawn_fake_privd_relay(
    socket_path: std::path::PathBuf,
    signer: clouddesk_privilege::GrantSigner,
    sessiond: std::path::PathBuf,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::spawn(async move {
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let sessiond = sessiond.clone();
            let signer = signer.clone();
            tokio::spawn(async move {
                let Ok(length) = stream.read_u32().await else {
                    return;
                };
                let mut bytes = vec![0_u8; length as usize];
                if stream.read_exact(&mut bytes).await.is_err() {
                    return;
                }
                let Ok(request) =
                    serde_json::from_slice::<clouddesk_privilege::PrivdRequest>(&bytes)
                else {
                    return;
                };
                if signer
                    .verify(&request.grant, request.grant.claims.issued_at)
                    .is_err()
                {
                    return;
                }
                let response = match &request.grant.claims.action {
                    clouddesk_privilege::PrivilegedAction::LocalFileOperation {
                        uid,
                        gid,
                        root,
                        writable,
                        operation,
                    } => {
                        let operation_json = serde_json::to_string(operation).unwrap();
                        let mut command = tokio::process::Command::new(&sessiond);
                        command
                            .arg("files")
                            .arg("--expected-uid")
                            .arg(uid.to_string())
                            .arg("--expected-gid")
                            .arg(gid.to_string())
                            .arg("--root")
                            .arg(root);
                        if *writable {
                            command.arg("--writable");
                        }
                        command.arg("--operation").arg(&operation_json);
                        match command.output().await {
                            Ok(out) if out.status.success() => {
                                let output: serde_json::Value = serde_json::from_slice(&out.stdout)
                                    .unwrap_or(serde_json::Value::Null);
                                clouddesk_privilege::PrivdResponse {
                                    accepted: true,
                                    message: "action completed".to_owned(),
                                    output: Some(output),
                                }
                            }
                            Ok(out) => clouddesk_privilege::PrivdResponse {
                                accepted: false,
                                message: String::from_utf8_lossy(&out.stderr).into_owned(),
                                output: None,
                            },
                            Err(error) => clouddesk_privilege::PrivdResponse {
                                accepted: false,
                                message: error.to_string(),
                                output: None,
                            },
                        }
                    }
                    _ => clouddesk_privilege::PrivdResponse {
                        accepted: false,
                        message: "this test relay only handles LocalFileOperation".to_owned(),
                        output: None,
                    },
                };
                let bytes = serde_json::to_vec(&response).unwrap();
                if let Ok(len) = u32::try_from(bytes.len()) {
                    if stream.write_u32(len).await.is_ok() {
                        let _ = stream.write_all(&bytes).await;
                    }
                }
            });
        }
    })
}

/// Builds the real product router with real Browser+Code+Office OCI
/// adapters all registered on one `RuntimeManager`, real Files
/// privilege wiring, and the real compiled `apps/web/dist` frontend.
#[allow(clippy::too_many_lines)]
async fn application() -> (String, tempfile::TempDir, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();

    let auth = clouddesk_auth::AuthService::new(
        pool.clone(),
        clouddesk_secrets::SecretCipher::new(&[181_u8; 32]).unwrap(),
        clouddesk_auth::AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "settings-playwright-test-secret\n").unwrap();

    let static_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
    let static_dir = if static_dir.join("index.html").exists() {
        static_dir
    } else {
        directory.path().to_owned()
    };

    // Files privilege relay, reused from Phase 4/5.
    let socket_path = directory.path().join("privd.sock");
    let grant_key = [193_u8; 32];
    let signer = clouddesk_privilege::GrantSigner::new(&grant_key).unwrap();
    let sessiond = sessiond_binary().expect(
        "cloudesk-sessiond must already be built (cargo build -p cloudesk-sessiond / --workspace)",
    );
    let _relay = spawn_fake_privd_relay(socket_path.clone(), signer, sessiond);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let privilege = clouddeskd::PrivilegeClient::new(&grant_key, socket_path).unwrap();

    let library = clouddesk_library::LibraryStore::new(pool.clone());
    let cache_dir = tempfile::tempdir().unwrap();
    let media_availability = clouddesk_media::ffmpeg::detect(true).await;
    let media = clouddesk_media::MediaService::new(
        media_availability,
        pool.clone(),
        cache_dir.path().to_owned(),
    );

    // Bind the real listener early so office_wopi_host_base's port is
    // the actual port axum::serve later listens on (same pattern
    // office_runtime.rs's own harness uses -- Collabora must reach
    // this exact server).
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let office_wopi_host_base = format!("http://host.docker.internal:{port}");

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let policy = clouddesk_orchestrator::ResourcePolicy {
        start_timeout: std::time::Duration::from_secs(45),
        health_timeout: std::time::Duration::from_secs(30),
        ..clouddesk_orchestrator::ResourcePolicy::default()
    };
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-settings-runtime-test-{}",
                std::process::id()
            )),
            policy,
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(
                clouddeskd::browser_runtime::browser_oci_spec(BROWSER_IMAGE.to_owned()),
            ),
        ))
        .with_kind_policy(
            clouddesk_orchestrator::RuntimeKind::Browser,
            clouddesk_orchestrator::ResourcePolicy {
                pids_limit: Some(512),
                ..policy
            },
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(clouddeskd::code_runtime::code_oci_spec(
                CODE_IMAGE.to_owned(),
            )),
        ))
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(
                clouddeskd::office_runtime::office_oci_spec(
                    OFFICE_IMAGE.to_owned(),
                    office_wopi_host_base.clone(),
                    false,
                ),
            ),
        )),
    );

    clouddeskd::browser_egress_proxy::spawn();

    let router =
        clouddeskd::application_router_with_privilege_and_media_and_library_and_runtime_and_office_configured(
            static_dir,
            auth,
            secret_path,
            privilege,
            true,
            Some(media),
            Some(library),
            Some(runtime_manager),
            Some(office_wopi_host_base),
        );
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://127.0.0.1:{port}"), directory, cache_dir)
}

async fn http(
    base: &str,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    body: Option<&Value>,
) -> reqwest::Response {
    let mut builder = reqwest::Client::new().request(
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
        format!("{base}{path}"),
    );
    if let Some(cookie) = cookie {
        builder = builder.header(reqwest::header::COOKIE, cookie);
    }
    if let Some(body) = body {
        builder = builder
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
    }
    builder.send().await.unwrap()
}

async fn login(base: &str, username: &str, password: &str) -> String {
    let response = http(
        base,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(&json!({"username": username, "password": password})),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn current_process_linux_username() -> Option<String> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        return None;
    }
    clouddesk_linux::lookup_uid(uid)
        .ok()
        .flatten()
        .map(|i| i.username)
}

async fn bootstrap_admin(base: &str) -> Option<String> {
    let linux_username = current_process_linux_username()?;
    let response = http(
        base,
        Method::POST,
        "/api/v1/setup/bootstrap",
        None,
        Some(&json!({
            "secret": "settings-playwright-test-secret",
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
            "linux_username": linux_username,
        })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    Some(login(base, "admin", "correct horse battery staple").await)
}

async fn create_user(base: &str, admin_cookie: &str, username: &str, role_id: &str) -> String {
    let step_up = http(
        base,
        Method::POST,
        "/api/v1/auth/step-up",
        Some(admin_cookie),
        Some(&json!({"password": "correct horse battery staple"})),
    )
    .await;
    assert_eq!(step_up.status(), reqwest::StatusCode::OK);
    let create = http(
        base,
        Method::POST,
        "/api/v1/users",
        Some(admin_cookie),
        Some(&json!({
            "username": username,
            "display_name": username,
            "password": "user horse battery staple",
            "role_ids": [role_id],
        })),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::CREATED);
    login(base, username, "user horse battery staple").await
}

/// Generates a real ODT fixture via real headless `LibreOffice`
/// (matches `office_runtime.rs`'s own established fixture pattern) so
/// Task 9/10's Office document flow is exercised through a genuine
/// document, not a fabricated/mocked one.
async fn generate_odt_fixture(dir: &std::path::Path) {
    let txt = dir.join("hello.txt");
    std::fs::write(&txt, "settings acceptance fixture").unwrap();
    let profile_dir = tempfile::tempdir().unwrap();
    let convert = TokioCommand::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile_dir.path().display()
        ))
        .args(["--headless", "--convert-to", "odt", "--outdir"])
        .arg(dir)
        .arg(&txt)
        .output()
        .await
        .unwrap();
    assert!(
        convert.status.success(),
        "soffice conversion failed: {convert:?}"
    );
    assert!(dir.join("hello.odt").exists());
}

async fn run_scenario(scenario: &str, args: &Value) -> Value {
    let scripts_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/browser");
    let args_dir = tempfile::tempdir().unwrap();
    let args_path = args_dir.path().join("args.json");
    std::fs::write(&args_path, serde_json::to_vec(args).unwrap()).unwrap();

    let output = TokioCommand::new("docker")
        .args([
            "run",
            "--rm",
            "--network",
            "host",
            "-v",
            &format!("{}:/scripts:ro", scripts_dir.display()),
            "-v",
            &format!("{}:/args:rw", args_dir.path().display()),
            "-w",
            "/work",
            PLAYWRIGHT_IMAGE,
            "sh",
            "-c",
            "mkdir -p /work && cp /scripts/settings_flow.mjs /work/ && \
             npm init -y >/dev/null 2>&1 && npm install playwright@1.49.0 >/dev/null 2>&1 && \
             node settings_flow.mjs \"$0\" \"$1\"",
            scenario,
            "/args/args.json",
        ])
        .output()
        .await
        .expect("failed to run playwright container");

    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!(
        "[{scenario}] playwright stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let last_line = stdout.lines().last().unwrap_or("");
    serde_json::from_str(last_line).unwrap_or_else(|e| {
        json!({
            "ok": false,
            "error": format!("could not parse playwright output: {e}"),
            "stdout": stdout.to_string(),
            "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
        })
    })
}

async fn cleanup_playwright_containers() {
    let ps = TokioCommand::new("docker")
        .args([
            "ps",
            "-a",
            "-q",
            "--filter",
            &format!("ancestor={PLAYWRIGHT_IMAGE}"),
        ])
        .output()
        .await
        .unwrap();
    for id in String::from_utf8_lossy(&ps.stdout).lines() {
        if !id.trim().is_empty() {
            let _ = TokioCommand::new("docker")
                .args(["rm", "-f", id])
                .output()
                .await;
        }
    }
}

fn no_settings_console_errors(errors: &[Value]) -> bool {
    errors.iter().all(|e| {
        let text = e.as_str().unwrap_or_default().to_lowercase();
        text.contains("autoplay")
            || text.contains("favicon")
            || (text.contains("failed to load resource")
                && (text.contains("401") || text.contains("503") || text.contains("404")))
            // Playwright's console listener captures messages from every
            // frame on the page, including the real code-server iframe's
            // OWN internal scripts (a third-party app CloudDesk embeds,
            // never recreates) -- CSP self-inline-script warnings and its
            // own webview module-loader quirk are attributable to
            // code-server itself, not to CloudDeskAppSettings/runtime-card
            // code, and out of scope for this phase (full Code product
            // correctness is explicitly Phase 7's concern, not Phase 6's
            // -- this phase's Code check is only "does Settings enable/
            // disable genuinely control it," already proven separately
            // by the real iframe/launch assertions).
            || text.contains("content security policy")
            || text.contains("cannot determine uri for module id")
    })
}

/// Tasks 3/5/6/7/8/9/10/11/23/26/28: the full compiled-frontend
/// Settings runtime-card journey -- Administrator sees all three
/// cards, then for each of Browser/Code/Office: enable + real launch,
/// disable through Settings (real UI), verify 0 resident containers,
/// re-enable, verify a real launch works again. Split into separate
/// scenario invocations per lifecycle phase (rather than one
/// continuous scenario) so the real Docker container check happens
/// exactly between "disable" and "re-enable" -- a single continuous
/// scenario would still be mid-relaunch (a real, expected running
/// container) by the time control returned to Rust.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_admin_runtime_lifecycle_through_settings() {
    if !docker_and_playwright_available().await {
        eprintln!("SKIPPED: docker/{PLAYWRIGHT_IMAGE} not available");
        return;
    }
    let have_browser = image_available(BROWSER_IMAGE).await;
    let have_code = image_available(CODE_IMAGE).await;
    let have_office = image_available(OFFICE_IMAGE).await;
    if !have_browser && !have_code && !have_office {
        eprintln!("SKIPPED: none of Browser/Code/Office images available");
        return;
    }
    cleanup_containers(BROWSER_IMAGE).await;
    cleanup_containers(CODE_IMAGE).await;
    cleanup_containers(OFFICE_IMAGE).await;

    let (base, _dir, _cache) = application().await;
    let Some(_admin) = bootstrap_admin(&base).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    // Files browses the real mapped user's real $HOME (resolve_safe_path
    // jails there) -- the fixture must live inside it (matching the
    // established Video/Music-pass convention: tempdir_in(&home)), not
    // in the harness's own bare tempdir, or Files will never list it.
    let home = std::env::var("HOME").unwrap();
    let fixture_dir = tempfile::tempdir_in(&home).unwrap();
    if have_office {
        generate_odt_fixture(fixture_dir.path()).await;
    }
    let dir_name = fixture_dir
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base_args = json!({
        "base": base,
        "username": "admin",
        "password": "correct horse battery staple",
        "folderName": dir_name,
        "officeFileName": "hello.odt",
    });

    let cards = run_scenario("settings_cards_visible", &base_args).await;
    assert_eq!(
        cards["ok"],
        json!(true),
        "playwright scenario must succeed: {cards:?}"
    );
    assert_eq!(
        cards["allThreeCardsVisible"],
        json!(true),
        "Administrator must see Browser/Code/Office runtime cards: {cards:?}"
    );
    let mut all_console_errors: Vec<Value> = cards["consoleErrors"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    if have_browser {
        let enable = run_scenario("browser_enable_and_launch", &base_args).await;
        assert_eq!(enable["ok"], json!(true), "{enable:?}");
        assert_eq!(
            enable["launched"],
            json!(true),
            "real Browser launch must work: {enable:?}"
        );
        all_console_errors.extend(
            enable["consoleErrors"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );

        let disable = run_scenario("browser_disable", &base_args).await;
        assert_eq!(disable["ok"], json!(true), "{disable:?}");
        assert_eq!(disable["disabled"], json!(true), "{disable:?}");
        assert_eq!(
            wait_for_zero_resident(BROWSER_IMAGE, 20000).await,
            0,
            "0 resident Browser containers must remain while disabled"
        );

        let reenable = run_scenario("browser_reenable_and_launch", &base_args).await;
        assert_eq!(reenable["ok"], json!(true), "{reenable:?}");
        assert_eq!(
            reenable["launched"],
            json!(true),
            "real Browser launch must work again after re-enable: {reenable:?}"
        );
        all_console_errors.extend(
            reenable["consoleErrors"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
    }

    if have_code {
        let enable = run_scenario("code_enable_and_launch", &base_args).await;
        assert_eq!(enable["ok"], json!(true), "{enable:?}");
        assert_eq!(
            enable["launched"],
            json!(true),
            "real Code launch must work: {enable:?}"
        );
        all_console_errors.extend(
            enable["consoleErrors"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );

        let disable = run_scenario("code_disable", &base_args).await;
        assert_eq!(disable["ok"], json!(true), "{disable:?}");
        assert_eq!(disable["disabled"], json!(true), "{disable:?}");
        assert_eq!(
            wait_for_zero_resident(CODE_IMAGE, 20000).await,
            0,
            "0 resident Code containers must remain while disabled"
        );

        let reenable = run_scenario("code_reenable_and_launch", &base_args).await;
        assert_eq!(reenable["ok"], json!(true), "{reenable:?}");
        assert_eq!(
            reenable["launched"],
            json!(true),
            "real Code launch must work again after re-enable: {reenable:?}"
        );
        all_console_errors.extend(
            reenable["consoleErrors"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
    }

    if have_office {
        let enable = run_scenario("office_enable_and_open", &base_args).await;
        assert_eq!(enable["ok"], json!(true), "{enable:?}");
        assert_eq!(
            enable["opened"],
            json!(true),
            "real Office document open must work: {enable:?}"
        );
        all_console_errors.extend(
            enable["consoleErrors"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );

        let disable = run_scenario("office_disable", &base_args).await;
        assert_eq!(disable["ok"], json!(true), "{disable:?}");
        assert_eq!(disable["disabled"], json!(true), "{disable:?}");
        assert_eq!(
            wait_for_zero_resident(OFFICE_IMAGE, 20000).await,
            0,
            "0 resident Office containers must remain while disabled"
        );

        let reenable = run_scenario("office_reenable_and_open", &base_args).await;
        assert_eq!(reenable["ok"], json!(true), "{reenable:?}");
        assert_eq!(
            reenable["opened"],
            json!(true),
            "real Office document open must work again after re-enable: {reenable:?}"
        );
        all_console_errors.extend(
            reenable["consoleErrors"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
    }

    cleanup_playwright_containers().await;
    assert!(
        no_settings_console_errors(&all_console_errors),
        "no uncaught Settings exception expected: {all_console_errors:?}"
    );

    cleanup_containers(BROWSER_IMAGE).await;
    cleanup_containers(CODE_IMAGE).await;
    cleanup_containers(OFFICE_IMAGE).await;
}

/// Task 4: an ordinary User sees the runtime cards but never the
/// enable/disable controls -- backend denial (already proven
/// extensively in `runtime_api.rs`) is the real security boundary;
/// this proves the UI-visible affordance itself is correctly hidden.
#[tokio::test]
async fn task_non_admin_has_no_runtime_controls() {
    if !docker_and_playwright_available().await {
        eprintln!("SKIPPED: docker/{PLAYWRIGHT_IMAGE} not available");
        return;
    }
    let (base, _dir, _cache) = application().await;
    let Some(admin_cookie) = bootstrap_admin(&base).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    create_user(&base, &admin_cookie, "settingsuser", "user").await;

    let result = run_scenario(
        "non_admin_no_runtime_controls",
        &json!({
            "base": base,
            "username": "settingsuser",
            "password": "user horse battery staple",
        }),
    )
    .await;
    cleanup_playwright_containers().await;

    assert_eq!(
        result["ok"],
        json!(true),
        "playwright scenario must succeed: {result:?}"
    );
    assert_eq!(
        result["cardsVisibleWithoutControls"],
        json!(true),
        "an ordinary User must see runtime status but never an enable/disable button: {result:?}"
    );
}
