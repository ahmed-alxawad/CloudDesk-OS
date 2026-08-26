//! Phase 7: real Playwright-driven evidence that the `CloudDesk` Code
//! product works through the ACTUAL compiled frontend -- login -> Files
//! -> "Open with Code" -> the real authenticated `CloudDesk` proxy -> the
//! real code-server web UI -> real edit/save/terminal/language/debug
//! behavior. `RuntimeManager`/the OCI code-server integration/the
//! authenticated HTTP+WS proxy are already real and extensively proven
//! at the product/API level by `code_runtime.rs` (25 tests); this file
//! closes the one remaining historical gap: real compiled-browser
//! product acceptance. Not rebuilt here.
//!
//! Reuses Phase 4/5/6's `cloudesk-privd`/`cloudesk-sessiond` privilege-
//! relay substitution for Files (same disclosed rationale).
//!
//! Skips (not FAIL) if docker/the Playwright image/the Code image
//! aren't available.

use axum::http::Method;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::process::Command as TokioCommand;

const PLAYWRIGHT_IMAGE: &str = "mcr.microsoft.com/playwright:v1.49.0-noble";
// Phase 7B closure: pinned to the locally-built patched image (see
// crates/config/src/lib.rs's `code_image` doc comment and
// docs/upstream/code-server-ts-duplicate-registration/) rather than
// stock codercom/code-server:4.133.0, which has a confirmed upstream
// TypeScript duplicate-registration defect on Workspace Trust grant.
const CODE_IMAGE: &str = "clouddesk/code-server:4.133.0-patch1";

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
    let code = TokioCommand::new("docker")
        .args(["image", "inspect", CODE_IMAGE])
        .output()
        .await
        .is_ok_and(|o| o.status.success());
    docker && image && code
}

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

/// Phase 7A-2: the *root-owned* installed copy of `cloudesk-sessiond`
/// (`0755`, `root:root`, refreshed by the operator from
/// `target/debug/cloudesk-sessiond` whenever the source changes) --
/// never the repository build artifact directly. Real `setuid(963)`
/// (the disposable Code test identity, genuinely different from this
/// test process's own uid) requires `CAP_SETUID`/root; sudoing the
/// ahmed-writable `target/debug` binary would mean an unprivileged
/// user's own build artifact runs with root privilege on demand, which
/// is exactly the kind of privilege-escalation primitive a real
/// production install must never allow. If this binary is missing, the
/// one-time operator setup (see `CLAUDE_ENGINEERING_CHECKPOINT.md`)
/// has not been done yet.
fn sessiond_binary() -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from("/usr/local/libexec/cloudesk-sessiond-test");
    path.exists().then_some(path)
}

/// Identical substitution to Phase 4/5/6: the real `clouddesk_privilege`
/// wire protocol and grant verification, shelling out to the real,
/// unmodified `cloudesk-sessiond` binary -- only the root-owned outer
/// `cloudesk-privd` relay is replaced.
#[allow(clippy::too_many_lines)]
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
                        // Real defect fixed during Phase 7A-2: every
                        // prior use of this substitution happened to
                        // request `--expected-uid`/`--expected-gid`
                        // equal to this test process's own real uid
                        // (ahmed), so `cloudesk-sessiond`'s own identity
                        // check (`identity_snapshot`: its *current*
                        // effective uid/gid must already equal
                        // `--expected-uid`/`--expected-gid`, and must
                        // never be root) was trivially satisfied without
                        // any real privilege drop ever happening. Now
                        // that Code acceptance maps its test identity to
                        // a genuinely different uid (963,
                        // `clouddesk-code-test`), sessiond correctly
                        // refuses (confirmed live: "cloudesk-sessiond
                        // must never execute as root" when this was
                        // first tried via plain `sudo -n`, which runs
                        // the target as *root* -- exactly what that
                        // check exists to reject). `cloudesk-sessiond`
                        // never calls `setuid` itself; in production the
                        // real root-owned `cloudesk-privd` relay forks
                        // and drops to the target uid/gid *before*
                        // exec'ing it. This substitution's equivalent is
                        // `sudo -u <target-username>` (the existing
                        // narrow, non-root sudoers grant for the
                        // disposable Code test identity) against the
                        // root-owned installed test copy -- never `sudo`
                        // bare (root) and never the ahmed-writable
                        // `target/debug` build artifact.
                        let username = clouddesk_linux::lookup_uid(*uid)
                            .ok()
                            .flatten()
                            .map(|i| i.username);
                        let command_output = match username {
                            None => Err(format!("no Linux account exists for uid {uid}")),
                            Some(username) => {
                                let mut command = tokio::process::Command::new("sudo");
                                command.arg("-n").arg("-u").arg(&username).arg(&sessiond);
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
                                command.output().await.map_err(|e| e.to_string())
                            }
                        };
                        match command_output {
                            Ok(out) if out.status.success() => {
                                let output: serde_json::Value = serde_json::from_slice(&out.stdout)
                                    .unwrap_or(serde_json::Value::Null);
                                clouddesk_privilege::PrivdResponse {
                                    accepted: true,
                                    message: "action completed".to_owned(),
                                    output: Some(output),
                                }
                            }
                            Ok(out) => {
                                eprintln!(
                                    "[fake-privd-relay] sessiond rejected (status {:?}): stdout={} stderr={}",
                                    out.status.code(),
                                    String::from_utf8_lossy(&out.stdout),
                                    String::from_utf8_lossy(&out.stderr),
                                );
                                clouddesk_privilege::PrivdResponse {
                                    accepted: false,
                                    message: String::from_utf8_lossy(&out.stderr).into_owned(),
                                    output: None,
                                }
                            }
                            Err(message) => clouddesk_privilege::PrivdResponse {
                                accepted: false,
                                message,
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

/// Builds the real product router with a real Code OCI adapter and real
/// Files privilege wiring, and the real compiled `apps/web/dist`
/// frontend.
#[allow(clippy::too_many_lines)]
async fn application() -> (String, tempfile::TempDir, sqlx::SqlitePool) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();

    let auth = clouddesk_auth::AuthService::new(
        pool.clone(),
        clouddesk_secrets::SecretCipher::new(&[211_u8; 32]).unwrap(),
        clouddesk_auth::AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "code-playwright-test-secret\n").unwrap();

    let static_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
    let static_dir = if static_dir.join("index.html").exists() {
        static_dir
    } else {
        directory.path().to_owned()
    };

    let socket_path = directory.path().join("privd.sock");
    let grant_key = [223_u8; 32];
    let signer = clouddesk_privilege::GrantSigner::new(&grant_key).unwrap();
    let sessiond = sessiond_binary().expect(
        "the root-owned test copy at /usr/local/libexec/cloudesk-sessiond-test is missing -- \
         see CLAUDE_ENGINEERING_CHECKPOINT.md for the one-time operator setup, and refresh it \
         from a fresh target/debug/cloudesk-sessiond build after any sessiond source change",
    );
    let _relay = spawn_fake_privd_relay(socket_path.clone(), signer, sessiond);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let privilege = clouddeskd::PrivilegeClient::new(&grant_key, socket_path).unwrap();

    let policy = clouddesk_orchestrator::ResourcePolicy {
        start_timeout: std::time::Duration::from_secs(45),
        health_timeout: std::time::Duration::from_secs(30),
        ..clouddesk_orchestrator::ResourcePolicy::default()
    };
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-code-playwright-test-{}",
                std::process::id()
            )),
            policy,
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(clouddeskd::code_runtime::code_oci_spec(
                CODE_IMAGE.to_owned(),
            )),
        ))
        // Matches `main.rs`'s real production wiring (Phase 7A-2 fix):
        // a genuinely complete Code session (extension host, TS
        // language service, integrated terminal, ripgrep search)
        // exceeds the shared default `pids_limit` (64) -- confirmed
        // live via `spawn ... EAGAIN`.
        .with_kind_policy(
            clouddesk_orchestrator::RuntimeKind::Code,
            clouddesk_orchestrator::ResourcePolicy {
                pids_limit: Some(256),
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        ),
    );

    let router =
        clouddeskd::application_router_with_privilege_and_media_and_library_and_runtime_configured(
            static_dir,
            auth,
            secret_path,
            privilege,
            true,
            None,
            None,
            Some(runtime_manager),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://127.0.0.1:{port}"), directory, pool)
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

/// Real defect fixed during the Phase 7A-2 closure pass: this used to
/// map "admin" to `current_process_linux_username()` -- whichever real
/// OS account happened to be running `cargo test`. On a developer host
/// that's the operator's own account, so every Code acceptance run
/// mounted and wrote persistent code-server state into the operator's
/// actual `/home/<them>` (`.local/share/code-server`,
/// `.config/code-server`), a real test-isolation defect (undiscovered
/// until Phase 7's live product journey needed to reason about profile
/// state across runs). Fixed by mapping "admin" to a real, dedicated,
/// disposable Linux system account (`clouddesk-code-test`, uid/gid
/// 963) created once by the operator specifically for Code acceptance
/// -- the exact same "map a `CloudDesk` user to a real Linux identity"
/// mechanism production uses for any user, not a special-cased test
/// mount. `/api/v1/setup/bootstrap`'s `linux_username` accepts any
/// real, existing, non-root account (see `lib.rs`'s `bootstrap`
/// handler) -- it was never required to be the invoking process's own.
const CODE_TEST_LINUX_USERNAME: &str = "clouddesk-code-test";

/// Fail-closed containment guard (Task 5): refuses to run any Code
/// acceptance test whose resolved host home escapes the disposable
/// fixture root, so a real user's home can never be mounted into a
/// Code container again by accident. Deliberately a canonical-path
/// containment check, not a hardcoded username comparison -- would
/// catch *any* real interactive-user home, not just this host's.
fn assert_disposable_code_test_home(home: &std::path::Path) {
    let canonical = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    assert!(
        canonical.starts_with("/var/lib/clouddesk-code-test"),
        "refusing to run Code acceptance: resolved host home {} is outside \
         the disposable /var/lib/clouddesk-code-test fixture root -- this would mount \
         a real user's home into a Code container",
        canonical.display()
    );
}

/// Resolves and validates the dedicated disposable Code test identity's
/// home. Every Code acceptance test that needs a host path for fixture
/// setup goes through this, never `std::env::var("HOME")`.
fn code_test_home() -> Option<std::path::PathBuf> {
    let identity = clouddesk_linux::lookup_user(CODE_TEST_LINUX_USERNAME)
        .ok()
        .flatten()?;
    assert_disposable_code_test_home(&identity.home);
    Some(identity.home)
}

async fn bootstrap_admin(base: &str) -> Option<String> {
    code_test_home()?;
    let response = http(
        base,
        Method::POST,
        "/api/v1/setup/bootstrap",
        None,
        Some(&json!({
            "secret": "code-playwright-test-secret",
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
            "linux_username": CODE_TEST_LINUX_USERNAME,
        })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    Some(login(base, "admin", "correct horse battery staple").await)
}

async fn enable_code(base: &str, admin_cookie: &str) {
    let response = http(
        base,
        Method::POST,
        "/api/v1/runtimes/code/enable",
        Some(admin_cookie),
        None,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
}

/// Generates a real, deterministic fixture workspace inside the real
/// mapped user's real `$HOME` (Files browses home -- `resolve_safe_path`
/// jails there), matching this codebase's established convention:
/// `README.md`, `src/example.ts` (a genuine TypeScript hover/
/// completion/diagnostic target), `package.json`/`tsconfig.json`,
/// `debug.js` (a minimal Node program for a real local debug session,
/// launched against code-server's own bundled Node binary since the
/// image has no general-purpose `node` on `PATH`), and a real local Git
/// repository (`git init` + one commit) so Source Control has genuine
/// history, not a synthetic stub.
/// Runs `script` as the dedicated disposable `clouddesk-code-test`
/// Linux account (Phase 7A-2 Task 2), never as this test process's own
/// uid. `home`'s permissions are deliberately `0700`, owned by that
/// account alone (the operator's own setup, matching how a real
/// per-user home is never group/world-accessible) -- so fixture setup
/// must run *as* that account, exactly the narrowest correct mechanism
/// for "this test process needs to create files another uid will own",
/// rather than loosening the directory's permissions to work around it.
/// `sudo` itself always starts as root regardless of the target user,
/// so it can read the script file (written by this ahmed-owned
/// process) before dropping privileges to execute it as uid 963.
async fn run_as_code_test_user(script: &str) {
    let script_file = tempfile::NamedTempFile::new().unwrap();
    tokio::fs::write(script_file.path(), script).await.unwrap();
    // `sudo` itself starts as root regardless of the target user, but it
    // execs the script *as* uid 963 -- which then needs to open this
    // file itself. World-readable is fine: the content is fixed,
    // non-sensitive fixture text, and this is a throwaway `/tmp` file,
    // never the disposable home itself.
    let mut perms = tokio::fs::metadata(script_file.path())
        .await
        .unwrap()
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o644);
    tokio::fs::set_permissions(script_file.path(), perms)
        .await
        .unwrap();
    let output = TokioCommand::new("sudo")
        .args([
            "-n",
            "-u",
            CODE_TEST_LINUX_USERNAME,
            "sh",
            &script_file.path().display().to_string(),
        ])
        .output()
        .await
        .expect("failed to invoke sudo -u clouddesk-code-test");
    assert!(
        output.status.success(),
        "fixture setup as {CODE_TEST_LINUX_USERNAME} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Companion to [`run_as_code_test_user`]: reads a file that only the
/// disposable identity can traverse to (its `0700` home).
async fn read_file_as_code_test_user(path: &std::path::Path) -> String {
    let output = TokioCommand::new("sudo")
        .args([
            "-n",
            "-u",
            CODE_TEST_LINUX_USERNAME,
            "cat",
            &path.display().to_string(),
        ])
        .output()
        .await
        .expect("failed to invoke sudo -u clouddesk-code-test");
    assert!(
        output.status.success(),
        "reading {} as {CODE_TEST_LINUX_USERNAME} failed: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Task 4 (Phase 7A-2): live proof of the actual OCI mounts a running
/// Code container has, not just fixture configuration -- asserts the
/// real operator home is never a mount source, and the disposable
/// fixture root is.
async fn assert_code_container_mounts_isolated(container_name_prefix: &str) {
    let ps = TokioCommand::new("docker")
        .args([
            "ps",
            "-q",
            "--filter",
            &format!("name={container_name_prefix}"),
        ])
        .output()
        .await
        .unwrap();
    let id = String::from_utf8_lossy(&ps.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    assert!(
        !id.is_empty(),
        "expected a running Code container to inspect"
    );
    let inspect = TokioCommand::new("docker")
        .args(["inspect", &id, "--format", "{{json .Mounts}}"])
        .output()
        .await
        .unwrap();
    let mounts = String::from_utf8_lossy(&inspect.stdout).into_owned();
    assert!(
        !mounts.contains("/home/ahmed"),
        "real operator home must never be an OCI mount source: {mounts}"
    );
    assert!(
        mounts.contains("/var/lib/clouddesk-code-test"),
        "the disposable fixture home must be an OCI mount source: {mounts}"
    );
}

/// Clears code-server's own persisted state under the disposable test
/// home -- safe (it's the dedicated `clouddesk-code-test` identity's
/// own directory, never `/home/ahmed`), and only used by the test that
/// explicitly represents the "clean profile" case (Phase 7A-2 Task A).
/// The returning-user/already-running tests deliberately never call
/// this.
async fn reset_code_profile(home: &std::path::Path) {
    let script = format!(
        "rm -rf {}/.local/share/code-server {}/.config/code-server\n",
        home.display(),
        home.display()
    );
    run_as_code_test_user(&script).await;
}

async fn generate_fixture(home: &std::path::Path) -> std::path::PathBuf {
    let dir = home.join("phase7-code-fixture");
    let dir_str = dir.display();
    // Idempotent: a leftover fixture from an earlier run of this same
    // test must not make git's "nothing to commit" outcome look like a
    // real failure. Every step below runs as the disposable identity
    // (Task 2) -- content is fixed, test-authored literal text, never
    // interpolated from external input, so embedding it in a shell
    // heredoc script is safe.
    let script = format!(
        r#"set -e
rm -rf {dir_str}
mkdir -p {dir_str}/src {dir_str}/.vscode
cat > {dir_str}/README.md <<'EOF'
# Phase 7 Code fixture

A disposable workspace for compiled-browser acceptance.
EOF
cat > {dir_str}/src/example.ts <<'EOF'
function greet(name: string): string {{
  return `hello ${{name}}`;
}}

const message = greet("CloudDesk");
EOF
cat > {dir_str}/package.json <<'EOF'
{{"name":"phase7-code-fixture","version":"1.0.0","private":true}}
EOF
cat > {dir_str}/tsconfig.json <<'EOF'
{{"compilerOptions":{{"strict":true,"target":"ES2020","module":"commonjs"}},"include":["src"]}}
EOF
cat > {dir_str}/debug.js <<'EOF'
const x = 21;
const y = x * 2;
console.log(y);
EOF
cat > {dir_str}/.vscode/launch.json <<'EOF'
{{
  "version": "0.2.0",
  "configurations": [
    {{
      "type": "node",
      "request": "launch",
      "name": "Debug fixture",
      "program": "${{workspaceFolder}}/debug.js",
      "runtimeExecutable": "/usr/lib/code-server/lib/node",
      "console": "internalConsole",
      "stopOnEntry": false
    }}
  ]
}}
EOF
cd {dir_str}
git init -q
git config user.email phase7@example.invalid
git config user.name "Phase 7 Fixture"
git add -A
git commit -q -m "initial fixture commit"
"#
    );
    run_as_code_test_user(&script).await;
    dir
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
            "mkdir -p /work && cp /scripts/code_flow.mjs /work/ && \
             npm init -y >/dev/null 2>&1 && npm install playwright@1.49.0 >/dev/null 2>&1 && \
             node code_flow.mjs \"$0\" \"$1\"",
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

fn no_code_console_errors(errors: &[Value]) -> bool {
    errors.iter().all(|e| {
        let text = e.as_str().unwrap_or_default().to_lowercase();
        text.contains("autoplay")
            || text.contains("favicon")
            || (text.contains("failed to load resource")
                && (text.contains("401") || text.contains("503") || text.contains("404")))
            // Real code-server's own bundled webview/service-worker
            // scripts emit CSP/module-loader noise unrelated to
            // CloudDesk's own integration code (identical, disclosed
            // reasoning to Phase 6's Code-iframe tolerance).
            || text.contains("content security policy")
            || text.contains("cannot determine uri for module id")
            || text.contains("service worker")
            || text.contains("workbench.desktop.main")
    })
}

/// The one coherent Phase 7 product journey (Task 55): login as User A
/// -> Files -> Open With Code on the TypeScript fixture -> real
/// code-server opens the exact file -> edit -> save -> hover ->
/// completion -> introduce+clear a diagnostic -> integrated terminal
/// whoami/pwd -> git status -> debug run/breakpoint/continue -> close
/// -> reopen -> confirm the saved edit persisted. Independently
/// verifies saved bytes server-side (Task 11), not merely a UI
/// indicator.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_full_product_journey() {
    if !docker_and_playwright_available().await {
        eprintln!("SKIPPED: docker/{PLAYWRIGHT_IMAGE}/{CODE_IMAGE} not available");
        return;
    }
    cleanup_containers(CODE_IMAGE).await;
    let (base, dir, _pool) = application().await;
    let Some(admin_cookie) = bootstrap_admin(&base).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    enable_code(&base, &admin_cookie).await;
    let home = code_test_home().expect("clouddesk-code-test must be resolvable at this point");
    // Task A/8 (clean profile): this test represents the fresh-profile
    // case specifically -- the returning/already-running cases have
    // their own dedicated test. Reset code-server's own persisted state
    // (safe: the disposable test identity's own directory, never
    // `/home/ahmed`) so accumulated state from a prior run of this same
    // test doesn't leak in (duplicate extension registration was
    // observed live after many repeated runs against the same profile).
    reset_code_profile(&home).await;
    let fixture = generate_fixture(&home).await;
    let _ = &dir;

    let result = run_scenario(
        "full_product_journey",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": "phase7-code-fixture",
            "sentinel": "SENTINEL_PHASE7_9f3a1c",
        }),
    )
    .await;
    cleanup_playwright_containers().await;

    assert_eq!(
        result["ok"],
        json!(true),
        "playwright scenario must succeed: {result:?}"
    );
    assert_eq!(result["openedExactFile"], json!(true), "{result:?}");
    assert_eq!(result["editApplied"], json!(true), "{result:?}");
    assert_eq!(result["saved"], json!(true), "{result:?}");
    assert_eq!(result["hoverShowed"], json!(true), "{result:?}");
    assert_eq!(result["completionShowed"], json!(true), "{result:?}");
    assert_eq!(result["diagnosticAppeared"], json!(true), "{result:?}");
    assert_eq!(result["diagnosticCleared"], json!(true), "{result:?}");
    // Phase 7B-10: `--disable-workspace-trust` (Phase 7B-9) was reverted
    // after security review -- CloudDesk users can bring genuinely
    // untrusted content into their authorized storage (uploads,
    // SFTP/S3/SSH remote transfers), and this pinned build gates real
    // automatic-code-execution surfaces (terminal/debug process
    // creation, auto-run tasks, "restricted" workspace settings) behind
    // Workspace Trust -- disabling it globally is a genuine security
    // regression, not merely a test convenience. The standard trust
    // dialog is therefore expected again on a fresh workspace's first
    // terminal PTY creation; `code_flow.mjs` handles it deliberately
    // (Phase 7B-8 found the *previous* driver accidentally confirmed it
    // via a stray Enter keystroke sent assuming terminal focus). The
    // underlying `vscode.typescript-language-features` remote+web
    // duplicate-registration defect this dialog triggers remains a
    // confirmed, unresolved upstream code-server/VS Code Web bug (see
    // Phase 7B-9/7B-10 reports) with no verified safe fix yet.
    assert_eq!(result["trustDialogAppeared"], json!(true), "{result:?}");
    assert_eq!(result["terminalWhoamiOk"], json!(true), "{result:?}");
    assert_eq!(result["terminalPwdOk"], json!(true), "{result:?}");
    assert_eq!(result["gitStatusShowedFile"], json!(true), "{result:?}");
    assert_eq!(result["debugPaused"], json!(true), "{result:?}");
    assert_eq!(result["debugContinuedOk"], json!(true), "{result:?}");
    assert_eq!(result["reopenedPersisted"], json!(true), "{result:?}");

    // Task 11: independently verify the saved bytes server-side, never
    // trusting the editor's own dirty-tab indicator alone. The
    // disposable home's `0700` permissions (Task 2) mean this test
    // process (ahmed) cannot even traverse into it directly -- read
    // through the same disposable identity, like every other fixture
    // operation in this file.
    let saved_content = read_file_as_code_test_user(&fixture.join("src/example.ts")).await;
    assert!(
        saved_content.contains("SENTINEL_PHASE7_9f3a1c"),
        "the saved file on disk must contain the real edit's sentinel: {saved_content}"
    );

    let console_errors = result["consoleErrors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        no_code_console_errors(&console_errors),
        "no uncaught CloudDesk/Code-integration exception expected: {console_errors:?}"
    );

    // Real product behavior, confirmed live during this pass: closing
    // the browser never stops a runtime instance by itself -- Code
    // intentionally stays warm until an explicit `/stop` or the
    // 30-minute idle timeout, so a fresh page load can resume it
    // instantly. Cleanup is therefore an explicit stop through the same
    // API a real client would use, not a bare disconnect.
    let instances = http(
        &base,
        Method::GET,
        "/api/v1/runtime-instances",
        Some(&admin_cookie),
        None,
    )
    .await;
    let instances_body: Value = instances.json().await.unwrap();
    if let Some(code_instance) = instances_body["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["kind"] == json!("code"))
    {
        let id = code_instance["instance_id"].as_str().unwrap();
        let _ = http(
            &base,
            Method::POST,
            &format!("/api/v1/runtime-instances/code/{id}/stop"),
            Some(&admin_cookie),
            None,
        )
        .await;
    }
    let leaked = resident_containers(CODE_IMAGE).await;
    cleanup_containers(CODE_IMAGE).await;
    assert_eq!(
        leaked, 0,
        "an explicit stop must leave no resident Code container"
    );
}

/// Phase 7A-2 Task 4: live proof that a real running Code container's
/// actual OCI mounts source the disposable test identity's home, never
/// the real operator's `/home/ahmed`. Deliberately API-driven (no
/// browser) -- this is a property of the orchestrator/OCI adapter
/// wiring, not the product UI, so the lighter-weight harness is enough.
#[tokio::test]
async fn task_oci_mount_isolation() {
    if !docker_and_playwright_available().await {
        eprintln!("SKIPPED: docker/{PLAYWRIGHT_IMAGE}/{CODE_IMAGE} not available");
        return;
    }
    cleanup_containers(CODE_IMAGE).await;
    let (base, _dir, _pool) = application().await;
    let Some(admin_cookie) = bootstrap_admin(&base).await else {
        eprintln!("skipping: cannot map the disposable Code test Linux identity");
        return;
    };
    enable_code(&base, &admin_cookie).await;

    let created = http(
        &base,
        Method::POST,
        "/api/v1/runtime-instances",
        Some(&admin_cookie),
        Some(&json!({"kind": "code"})),
    )
    .await;
    assert_eq!(created.status(), reqwest::StatusCode::OK);
    let created_body: Value = created.json().await.unwrap();
    let instance_id = created_body["instance_id"].as_str().unwrap().to_owned();

    let mut running = created_body["state"] == json!("running");
    for _ in 0..40 {
        if running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let status = http(
            &base,
            Method::GET,
            &format!("/api/v1/runtime-instances/code/{instance_id}"),
            Some(&admin_cookie),
            None,
        )
        .await;
        let body: Value = status.json().await.unwrap();
        running = body["state"] == json!("running");
    }
    assert!(
        running,
        "Code instance must reach running before mount inspection"
    );

    assert_code_container_mounts_isolated("clouddesk-runtime-").await;

    let _ = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/code/{instance_id}/stop"),
        Some(&admin_cookie),
        None,
    )
    .await;
    cleanup_containers(CODE_IMAGE).await;
}

/// Phase 7A-2 Tasks B/C: proves Files -> Open with Code reliably opens
/// the *requested* file whether the disposable Code profile is
/// completely fresh, already has persisted empty-window state from a
/// prior real session, or Code is already running and a second,
/// distinct file is requested -- through the real compiled frontend
/// each time, never by clearing/faking profile state to force a pass.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_returning_and_already_running_files_to_code() {
    if !docker_and_playwright_available().await {
        eprintln!("SKIPPED: docker/{PLAYWRIGHT_IMAGE}/{CODE_IMAGE} not available");
        return;
    }
    cleanup_containers(CODE_IMAGE).await;
    let (base, dir, _pool) = application().await;
    let Some(admin_cookie) = bootstrap_admin(&base).await else {
        eprintln!("skipping: cannot map the disposable Code test Linux identity");
        return;
    };
    enable_code(&base, &admin_cookie).await;
    let home = code_test_home().expect("clouddesk-code-test must be resolvable at this point");
    let fixture = generate_fixture(&home).await;
    let _ = &dir;

    // --- Task A/B part 1: establish real persisted empty-window state
    // against this SAME disposable profile, then close it, before Files
    // is ever used -- a genuine "returning user" precondition, not a
    // cleared/fresh profile.
    let establish = run_scenario(
        "establish_empty_window_state",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
        }),
    )
    .await;
    cleanup_playwright_containers().await;
    assert_eq!(
        establish["ok"],
        json!(true),
        "establishing empty-window state must succeed: {establish:?}"
    );
    // Gracefully stop through the product's own API (not a raw `docker
    // stop`) so the orchestrator's own instance-state bookkeeping stays
    // correct for the reuse path the next scenario exercises.
    let instances = http(
        &base,
        Method::GET,
        "/api/v1/runtime-instances",
        Some(&admin_cookie),
        None,
    )
    .await;
    let instances_body: Value = instances.json().await.unwrap();
    if let Some(code_instance) = instances_body["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["kind"] == json!("code"))
    {
        let id = code_instance["instance_id"].as_str().unwrap();
        let _ = http(
            &base,
            Method::POST,
            &format!("/api/v1/runtime-instances/code/{id}/stop"),
            Some(&admin_cookie),
            None,
        )
        .await;
    }

    // --- Task B: returning-user profile -- Files -> Open with Code on
    // example.ts must still win over the restored empty-window layout.
    let returning = run_scenario(
        "returning_user_files_to_code",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": "phase7-code-fixture",
        }),
    )
    .await;
    cleanup_playwright_containers().await;
    assert_eq!(
        returning["ok"],
        json!(true),
        "returning-profile Files -> Code must open the requested file: {returning:?}"
    );
    assert_eq!(returning["openedExactFile"], json!(true), "{returning:?}");

    // --- Task C: with Code already running (same page/browser session,
    // no reload), a second, distinct file must become the active tab,
    // reusing the existing runtime rather than creating a duplicate.
    let second_file = run_scenario(
        "already_running_second_file_handoff",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": "phase7-code-fixture",
            "secondFileName": "debug.js",
        }),
    )
    .await;
    cleanup_playwright_containers().await;
    assert_eq!(
        second_file["ok"],
        json!(true),
        "already-running Code must open the second requested file: {second_file:?}"
    );
    assert_eq!(
        second_file["openedSecondFile"],
        json!(true),
        "{second_file:?}"
    );

    // Real product behavior, confirmed live during this pass: closing
    // the browser/window never stops a runtime instance by itself --
    // Code intentionally stays warm (only an explicit `/stop` call or
    // the 30-minute idle timeout ends it), so reopening later is
    // instant. That means "no duplicate instance" must be checked as
    // *exactly one* resident container (the single, correctly-reused
    // instance) immediately after the handoff, not zero after the
    // browser disconnects -- disconnection was never the product's
    // cleanup signal. Cleanup here is this test explicitly stopping it
    // through the same API a real client would use, exactly like
    // `stop_instance` earlier in this test.
    let resident_after_handoff = resident_containers(CODE_IMAGE).await;
    assert_eq!(
        resident_after_handoff, 1,
        "already-running handoff must reuse the single existing Code instance, never create a duplicate"
    );
    let instances = http(
        &base,
        Method::GET,
        "/api/v1/runtime-instances",
        Some(&admin_cookie),
        None,
    )
    .await;
    let instances_body: Value = instances.json().await.unwrap();
    if let Some(code_instance) = instances_body["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["kind"] == json!("code"))
    {
        let id = code_instance["instance_id"].as_str().unwrap();
        let _ = http(
            &base,
            Method::POST,
            &format!("/api/v1/runtime-instances/code/{id}/stop"),
            Some(&admin_cookie),
            None,
        )
        .await;
    }
    let leaked = resident_containers(CODE_IMAGE).await;
    cleanup_containers(CODE_IMAGE).await;
    assert_eq!(
        leaked, 0,
        "an explicit stop must leave no resident Code container"
    );

    let _ = &fixture;
}
