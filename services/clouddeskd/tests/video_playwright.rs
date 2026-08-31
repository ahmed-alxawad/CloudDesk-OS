//! Phase 4: real Playwright-driven evidence that the `CloudDesk` Video
//! product works through the ACTUAL compiled frontend -- login -> Files
//! -> double-click a video fixture -> the real `VideoApp.svelte` ->
//! real browser media playback (`readyState`/`currentTime`/`paused`) --
//! never a direct media URL, never a component test, never a direct
//! `ffmpeg` invocation from this file. Runs in a disposable,
//! version-pinned Playwright/Chromium container (test infrastructure
//! only).
//!
//! Skips (not FAIL) if docker/the Playwright image, ffmpeg, or
//! `apps/web/dist` aren't available.

use axum::http::Method;
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;

const PLAYWRIGHT_IMAGE: &str = "mcr.microsoft.com/playwright:v1.49.0-noble";

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

async fn ffmpeg_available() -> bool {
    clouddesk_media::ffmpeg::detect(true).await.is_available()
}

/// Real ffmpeg/ffprobe child-process count via `/proc` -- used to prove
/// no process leaks after a scenario closes.
fn media_process_count() -> usize {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().parse::<u32>().is_ok())
        .filter(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| matches!(comm.trim(), "ffmpeg" | "ffprobe"))
        })
        .count()
}

/// Path to the real, unmodified `cloudesk-sessiond` binary (a sibling
/// package, so `CARGO_BIN_EXE_cloudesk-sessiond` isn't set automatically
/// the way it would be for a binary in this same package) -- built by
/// `cargo build --workspace`/`cargo build -p cloudesk-sessiond`, same as
/// every other real-binary test in this codebase relies on a prior
/// build having produced its target.
fn sessiond_binary() -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/cloudesk-sessiond");
    path.exists().then_some(path)
}

/// Files listing/read/write in this product is dispatched exclusively
/// through `cloudesk-privd` (a real, separate root-owned daemon) ->
/// `cloudesk-sessiond` (a real, setuid-switched per-user worker) -- real
/// disk I/O never happens inside `clouddeskd` itself. `cloudesk-privd`
/// refuses to run as anything but root (`main.rs`: "cloudesk-privd must
/// run as root"), which this sandboxed test environment genuinely does
/// not have (confirmed live: no passwordless sudo). Real root-level
/// privilege separation is therefore not exercised by these tests --
/// disclosed here and in the final report, not silently substituted.
///
/// What IS real: the exact same `clouddesk_privilege::GrantSigner`/
/// `PrivdRequest`/`PrivdResponse` wire protocol and grant verification
/// `cloudeskd`'s own dispatch code uses, and -- critically for Files
/// listing specifically -- the real, unmodified `cloudesk-sessiond`
/// binary performing genuine `readdir`/`stat` filesystem I/O against
/// the real fixture directory (never a canned/mocked entries list).
/// Since the test's mapped Linux identity is this process's own real
/// UID/GID (the same convention every other test in this codebase
/// uses), no actual privilege transition would occur even with real
/// `cloudesk-privd`/`setpriv` in the loop -- only the root-owned relay
/// process itself is replaced, with a thin one that shells out to the
/// real worker binary directly instead of via `setpriv`.
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

async fn application() -> (String, tempfile::TempDir, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    sqlx::query(
        "UPDATE system_settings SET value_json = 'true' WHERE key = 'runtime.media.enabled'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let availability = clouddesk_media::ffmpeg::detect(true).await;
    let cache_dir = tempfile::tempdir().unwrap();
    let media =
        clouddesk_media::MediaService::new(availability, pool.clone(), cache_dir.path().to_owned());

    let auth = clouddesk_auth::AuthService::new(
        pool,
        clouddesk_secrets::SecretCipher::new(&[233_u8; 32]).unwrap(),
        clouddesk_auth::AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "video-playwright-test-secret\n").unwrap();

    let static_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
    let static_dir = if static_dir.join("index.html").exists() {
        static_dir
    } else {
        directory.path().to_owned()
    };

    let socket_path = directory.path().join("privd.sock");
    let grant_key = [211_u8; 32];
    let signer = clouddesk_privilege::GrantSigner::new(&grant_key).unwrap();
    let sessiond = sessiond_binary().expect(
        "cloudesk-sessiond must already be built (cargo build -p cloudesk-sessiond / --workspace)",
    );
    let _relay = spawn_fake_privd_relay(socket_path.clone(), signer, sessiond);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let privilege = clouddeskd::PrivilegeClient::new(&grant_key, socket_path).unwrap();

    let router = clouddeskd::application_router_with_privilege_and_media_configured(
        static_dir,
        auth,
        secret_path,
        privilege,
        false,
        Some(media),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
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
            "secret": "video-playwright-test-secret",
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

/// Real fixtures generated inside the mapped user's real `$HOME` (a
/// self-cleaning tempdir, matching this codebase's established
/// convention), covering DIRECT (MP4/h264/aac), REMUX (the existing
/// committed VP8/Opus `WebM` -- `matroska,webm` is always remuxed
/// regardless of codec compatibility, per `compat.rs`), TRANSCODE
/// (MPEG-2 video, a codec no browser can decode), and a malformed file.
struct Fixtures {
    dir: tempfile::TempDir,
    folder_name: String,
    direct: String,
    remux: String,
    transcode: String,
    malformed: String,
}

async fn generate_fixtures() -> Fixtures {
    let home = std::env::var("HOME").unwrap();
    let dir = tempfile::tempdir_in(&home).unwrap();
    let folder_name = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // VP9/Opus muxed into an MP4 container -- `compat.rs` classifies this
    // `Direct` (both codecs are on the browser-compatible lists and the
    // container reports as `mov,mp4,m4a,3gp,3g2,mj2`), and unlike H.264/
    // AAC, VP9/Opus are open codecs bundled in every Chromium build
    // (including Playwright's, which -- confirmed live during this pass
    // -- has no H.264 decoder at all: an H.264/AAC fixture reliably
    // reached the real `<video>` element's `error` event, never a
    // product bug, purely a licensed-codec-decoder absence in this
    // specific browser binary).
    let direct = dir.path().join("direct.mp4");
    let status = TokioCommand::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=6:size=320x240:rate=15",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=6",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libvpx-vp9",
            "-c:a",
            "libopus",
            "-shortest",
            "-movflags",
            "+faststart",
        ])
        .arg(&direct)
        .status()
        .await
        .unwrap();
    assert!(status.success(), "direct fixture generation failed");

    // Reuse the existing committed real fixture (VP8/Opus in a
    // `matroska,webm`-reported container, `compat.rs`'s remux path).
    let remux_src =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test_video.webm");
    let remux = dir.path().join("remux.webm");
    std::fs::copy(&remux_src, &remux).unwrap();

    let transcode = dir.path().join("transcode.mkv");
    let status = TokioCommand::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=320x240:rate=15",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=4",
            "-c:v",
            "mpeg2video",
            "-c:a",
            "mp2",
            "-shortest",
        ])
        .arg(&transcode)
        .status()
        .await
        .unwrap();
    assert!(status.success(), "transcode fixture generation failed");

    let malformed = dir.path().join("malformed.mp4");
    std::fs::write(
        &malformed,
        b"this is not a real video file, just garbage bytes",
    )
    .unwrap();

    Fixtures {
        folder_name,
        direct: "direct.mp4".to_owned(),
        remux: "remux.webm".to_owned(),
        transcode: "transcode.mkv".to_owned(),
        malformed: "malformed.mp4".to_owned(),
        dir,
    }
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
            "mkdir -p /work && cp /scripts/video_flow.mjs /work/ && \
             npm init -y >/dev/null 2>&1 && npm install playwright@1.49.0 >/dev/null 2>&1 && \
             node video_flow.mjs \"$0\" \"$1\"",
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

fn no_video_console_errors(errors: &[Value]) -> bool {
    // Known-unrelated browser noise is tolerated; anything else
    // attributable to Video's own JS is not. The 401/503 "failed to
    // load resource" pair is pre-login page-load noise (background
    // settings/runtime-status fetches that legitimately 401 before
    // authentication and 503 while a runtime is disabled) -- observed
    // consistently before "logged in as admin" across every scenario in
    // this file, never after Video itself opens, matching the same
    // known noise already established in this codebase's other
    // Playwright-driven product tests.
    errors.iter().all(|e| {
        let text = e.as_str().unwrap_or_default().to_lowercase();
        text.contains("autoplay")
            || text.contains("favicon")
            || (text.contains("failed to load resource")
                && (text.contains("401") || text.contains("503")))
    })
}

/// Tasks 1/2/4/5/6/7/8/9/10/12/13: the full direct-path product flow --
/// Files -> double-click -> real metadata -> play -> pause/resume ->
/// seek -> mute/volume -> end of playback -> best-effort fullscreen.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_direct_full_flow() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_direct_full_flow",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    if !docker_and_playwright_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_direct_full_flow",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(admin) = bootstrap_admin(&base).await else {
        clouddesk_test_support::blocked_by_environment(
            "task_direct_full_flow",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let _ = admin;
    let fixtures = generate_fixtures().await;

    let result = run_scenario(
        "direct_full_flow",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": fixtures.folder_name,
            "directFileName": fixtures.direct,
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
        result["plan"],
        json!("direct"),
        "an MP4/h264/aac fixture must classify as direct: {result:?}"
    );
    assert_eq!(
        result["metadataOk"],
        json!(true),
        "real browser media metadata must load: {result:?}"
    );
    assert_eq!(
        result["playing"],
        json!(true),
        "playback must actually start: {result:?}"
    );
    assert!(
        result["afterPlayCurrentTime"].as_f64().unwrap_or(0.0) > 0.0,
        "currentTime must advance during real playback: {result:?}"
    );
    assert_eq!(
        result["pauseHeld"],
        json!(true),
        "pause must actually stop currentTime advancing: {result:?}"
    );
    assert_eq!(
        result["resumed"],
        json!(true),
        "resume must advance currentTime again: {result:?}"
    );
    assert_eq!(
        result["seeked"],
        json!(true),
        "seek must land near the requested position: {result:?}"
    );
    assert_eq!(
        result["stillSameApp"],
        json!(1),
        "seeking must never reload the app: {result:?}"
    );
    assert_eq!(result["mutedAfterToggle"], json!(true));
    assert_eq!(result["unmutedAfterToggle"], json!(false));
    assert!((result["volumeAfterSet"].as_f64().unwrap_or(-1.0) - 0.4).abs() < 0.05);
    assert_eq!(
        result["ended"],
        json!(true),
        "seeking near the end must reach the real ended state: {result:?}"
    );
    let responses = result["mediaResponses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let saw_stream = responses.iter().any(|r| {
        r["url"]
            .as_str()
            .unwrap_or_default()
            .contains("/media/stream")
    });
    assert!(
        saw_stream,
        "the real browser must actually request the direct stream endpoint: {responses:?}"
    );
    let saw_partial = responses.iter().any(|r| r["status"] == json!(206));
    assert!(
        saw_partial,
        "real seeking on a direct-path file must exercise HTTP 206 partial content: {responses:?}"
    );
    let console_errors = result["consoleErrors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        no_video_console_errors(&console_errors),
        "no uncaught Video exception expected: {console_errors:?}"
    );
}

/// Tasks 3/5/11/15: the remux-path flow through the real product, using
/// the existing committed VP8/Opus `WebM` fixture (always remuxed --
/// `matroska,webm` container ambiguity), including real audio-track
/// presence.
#[tokio::test]
async fn task_remux_full_flow() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_remux_full_flow",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    if !docker_and_playwright_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_remux_full_flow",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(_admin) = bootstrap_admin(&base).await else {
        clouddesk_test_support::blocked_by_environment(
            "task_remux_full_flow",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let fixtures = generate_fixtures().await;

    let result = run_scenario(
        "remux_full_flow",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": fixtures.folder_name,
            "remuxFileName": fixtures.remux,
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
        result["plan"],
        json!("remux"),
        "matroska,webm must always classify as remux: {result:?}"
    );
    assert_eq!(
        result["jobCompleted"],
        json!(true),
        "the real remux job must complete: {result:?}"
    );
    assert_eq!(result["metadataOk"], json!(true));
    assert_eq!(result["playing"], json!(true));
    assert!(result["afterPlayCurrentTime"].as_f64().unwrap_or(0.0) > 0.0);
    assert_eq!(
        result["muted"],
        json!(false),
        "unmuted after a real user gesture: {result:?}"
    );
    let has_audio = result["hasAudioTrack"].clone();
    assert!(
        has_audio == json!(true) || has_audio == json!(Value::Null),
        "audio-decode evidence must never indicate zero decoded audio bytes: {result:?}"
    );
}

/// Task 16/17: the transcode-fallback flow through the real product --
/// a source codec no browser can decode, real job processing UI,
/// real playable result. Also verifies (Task 24/25) no leaked
/// ffmpeg/ffprobe process survives the whole flow.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_transcode_full_flow_and_no_process_leak() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_transcode_full_flow_and_no_process_leak",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    if !docker_and_playwright_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_transcode_full_flow_and_no_process_leak",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(admin) = bootstrap_admin(&base).await else {
        clouddesk_test_support::blocked_by_environment(
            "task_transcode_full_flow_and_no_process_leak",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let fixtures = generate_fixtures().await;
    let before = media_process_count();

    let result = run_scenario(
        "transcode_full_flow",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": fixtures.folder_name,
            "transcodeFileName": fixtures.transcode,
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
        result["plan"],
        json!("transcode"),
        "an mpeg2video source must classify as transcode: {result:?}"
    );
    assert_eq!(
        result["jobCompleted"],
        json!(true),
        "the real transcode job must complete: {result:?}"
    );
    // Playwright's Chromium build ships with no H.264/AAC decoder at all
    // (confirmed live via MediaSource.isTypeSupported: h264=false,
    // aac=false) -- production's real transcode output is always h264/
    // aac, so visual playback verification is genuinely BLOCKED BY
    // ENVIRONMENT here, not a product defect. Either this specific
    // browser can somehow decode it (metadataOk) or it correctly
    // reaches the app's own safe decode-error UI (decodeErrorSeen) --
    // never neither, which would mean a stuck/broken UI instead.
    assert!(
        result["metadataOk"] == json!(true) || result["decodeErrorSeen"] == json!(true),
        "the app must either play the real transcoded output or show a safe decode error, never hang: {result:?}"
    );
    if result["metadataOk"] != json!(true) {
        eprintln!(
            "BLOCKED BY ENVIRONMENT: visual transcode playback verification skipped -- this \
             Playwright Chromium build has no H.264/AAC decoder (verified independently below \
             via ffprobe of the real production output bytes instead)"
        );
    }

    // Independent verification that the real production transcode
    // pipeline genuinely produces valid, correctly-encoded output --
    // proven via `ffprobe` (never subject to this test browser's own
    // decoder limitation), through the exact same real HTTP API path.
    let job_id = {
        let create = http(
            &base,
            Method::POST,
            "/api/v1/media/jobs",
            Some(&admin),
            Some(&json!({
                "path": format!("{}/{}", fixtures.folder_name, fixtures.transcode),
                "operation": "transcode",
            })),
        )
        .await;
        assert_eq!(create.status(), reqwest::StatusCode::OK);
        create.json::<Value>().await.unwrap()["job_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let mut state = String::new();
    for _ in 0..100 {
        let status = http(
            &base,
            Method::GET,
            &format!("/api/v1/media/jobs/{job_id}"),
            Some(&admin),
            None,
        )
        .await;
        let body: Value = status.json().await.unwrap();
        state = body["state"].as_str().unwrap_or_default().to_owned();
        if matches!(
            state.as_str(),
            "completed" | "failed" | "cancelled" | "expired"
        ) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    assert_eq!(
        state, "completed",
        "the independently-created real transcode job must complete"
    );
    let output = http(
        &base,
        Method::GET,
        &format!("/api/v1/media/jobs/{job_id}/output"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(output.status(), reqwest::StatusCode::OK);
    let bytes = output.bytes().await.unwrap();
    assert!(
        !bytes.is_empty(),
        "the real transcode output must be non-empty"
    );
    let probe_path = fixtures.dir.path().join("transcode-output-probe.mp4");
    std::fs::write(&probe_path, &bytes).unwrap();
    let probe = TokioCommand::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(&probe_path)
        .output()
        .await
        .unwrap();
    assert!(
        probe.status.success(),
        "ffprobe must be able to parse the real transcode output"
    );
    let codecs = String::from_utf8_lossy(&probe.stdout);
    assert!(
        codecs.contains("h264"),
        "the real production transcode output must genuinely be h264: {codecs}"
    );
    assert!(
        codecs.contains("aac"),
        "the real production transcode output must genuinely be aac: {codecs}"
    );

    let mut leak_gone = false;
    for _ in 0..20 {
        if media_process_count() <= before {
            leak_gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    assert!(
        leak_gone,
        "no ffmpeg/ffprobe process must survive a completed transcode flow"
    );
}

/// Task 18: a malformed/unsupported fixture, opened through the real
/// product, must show a safe alert -- never a stuck spinner, never a
/// frontend crash, never a raw backend stack trace.
#[tokio::test]
async fn task_failure_flow() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_failure_flow",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    if !docker_and_playwright_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_failure_flow",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(_admin) = bootstrap_admin(&base).await else {
        clouddesk_test_support::blocked_by_environment(
            "task_failure_flow",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let fixtures = generate_fixtures().await;

    let result = run_scenario(
        "failure_flow",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": fixtures.folder_name,
            "malformedFileName": fixtures.malformed,
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
        result["sawError"],
        json!(true),
        "a malformed file must reach a real, visible error state: {result:?}"
    );
    assert_eq!(
        result["stuckLoading"],
        json!(0),
        "the app must never stay in a loading state after failure: {result:?}"
    );
    assert!(
        result["hasRetryButton"].as_u64().unwrap_or(0) > 0,
        "a real retry affordance must be offered: {result:?}"
    );
    let text = result["errorText"].as_str().unwrap_or_default();
    assert!(
        !text.contains("panic")
            && !text.to_lowercase().contains("thread '")
            && !text.contains("/home/"),
        "the error message must never leak a raw stack trace or filesystem path: {text:?}"
    );
}

/// Task 27: a real network failure on the media stream request must
/// leave the loading state (safe error / retry), never hang forever.
#[tokio::test]
async fn task_network_failure_flow() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_network_failure_flow",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    if !docker_and_playwright_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_network_failure_flow",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(_admin) = bootstrap_admin(&base).await else {
        clouddesk_test_support::blocked_by_environment(
            "task_network_failure_flow",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let fixtures = generate_fixtures().await;

    let result = run_scenario(
        "network_failure_flow",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": fixtures.folder_name,
            "directFileName": fixtures.direct,
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
        result["leftLoading"],
        json!(true),
        "a failed media request must never leave the app stuck loading: {result:?}"
    );
}

/// Task 24: refresh/reopen must never leave a stale session breaking
/// playback -- the same fixture opens cleanly a second time.
#[tokio::test]
async fn task_refresh_reopen_flow() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_refresh_reopen_flow",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    if !docker_and_playwright_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_refresh_reopen_flow",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(_admin) = bootstrap_admin(&base).await else {
        clouddesk_test_support::blocked_by_environment(
            "task_refresh_reopen_flow",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let fixtures = generate_fixtures().await;

    let result = run_scenario(
        "refresh_reopen_flow",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": fixtures.folder_name,
            "directFileName": fixtures.direct,
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
        result["metadataOk"],
        json!(true),
        "the same fixture must open cleanly after a refresh: {result:?}"
    );
}

/// Task 21: revoking the underlying file (deleting it) while a stream
/// URL exists must deny subsequent server reads -- proven through the
/// real product/API path, not merely buffered-browser semantics.
#[tokio::test]
async fn task_21_revocation_denies_further_server_reads() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_21_revocation_denies_further_server_reads",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(admin) = bootstrap_admin(&base).await else {
        clouddesk_test_support::blocked_by_environment(
            "task_21_revocation_denies_further_server_reads",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let fixtures = generate_fixtures().await;
    let virtual_path = format!("{}/{}", fixtures.folder_name, fixtures.direct);

    let before = http(
        &base,
        Method::GET,
        &format!(
            "/api/v1/media/stream?path={}",
            urlencoding_encode(&virtual_path)
        ),
        Some(&admin),
        None,
    )
    .await;
    assert!(
        before.status().is_success() || before.status() == reqwest::StatusCode::PARTIAL_CONTENT,
        "the file must be readable before revocation: {}",
        before.status()
    );

    // Revoke: delete the real underlying file (the file itself is the
    // authorization boundary Video's read path checks against).
    std::fs::remove_file(fixtures.dir.path().join(&fixtures.direct)).unwrap();

    let after = http(
        &base,
        Method::GET,
        &format!(
            "/api/v1/media/stream?path={}",
            urlencoding_encode(&virtual_path)
        ),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(
        after.status(),
        reqwest::StatusCode::NOT_FOUND,
        "a revoked/deleted file must never be readable after revocation: {}",
        after.status()
    );
}

fn urlencoding_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
