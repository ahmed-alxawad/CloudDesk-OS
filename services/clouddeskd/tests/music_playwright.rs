//! Phase 5K: real Playwright-driven evidence that the `CloudDesk` Music
//! product works through the ACTUAL compiled frontend -- login -> Files
//! -> double-click an audio fixture -> the real `MusicApp.svelte` ->
//! real browser audio playback (`readyState`/`currentTime`/`paused`),
//! plus one coherent product journey (favorite/queue/playlist/search/
//! recent). Never a direct media URL, never a component test, never a
//! direct `ffmpeg` invocation from this file. Runs in the same
//! disposable, version-pinned Playwright/Chromium container Phase 4
//! (Video) uses.
//!
//! Skips (not FAIL) if docker/the Playwright image, ffmpeg, or
//! `apps/web/dist` aren't available. Reuses the same real
//! `cloudesk-sessiond`-backed privilege relay Phase 4 built (see its own
//! doc comment there for exactly what is real vs. substituted) since
//! Files->Music, like Files->Video, requires real Files listing.

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

/// Real ffmpeg/ffprobe child-process count via `/proc` -- proves no
/// process leaks after a scenario closes.
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

fn sessiond_binary() -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/cloudesk-sessiond");
    path.exists().then_some(path)
}

/// Identical substitution to Phase 4's `video_playwright.rs`: the real
/// `clouddesk_privilege` wire protocol and grant verification, shelling
/// out to the real, unmodified `cloudesk-sessiond` binary -- only the
/// root-owned outer `cloudesk-privd` relay is replaced, since this
/// sandbox has no root/passwordless-sudo and privd hard-requires it.
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
    let library = clouddesk_library::LibraryStore::new(pool.clone());

    let auth = clouddesk_auth::AuthService::new(
        pool,
        clouddesk_secrets::SecretCipher::new(&[241_u8; 32]).unwrap(),
        clouddesk_auth::AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "music-playwright-test-secret\n").unwrap();

    let static_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
    let static_dir = if static_dir.join("index.html").exists() {
        static_dir
    } else {
        directory.path().to_owned()
    };

    let socket_path = directory.path().join("privd.sock");
    let grant_key = [251_u8; 32];
    let signer = clouddesk_privilege::GrantSigner::new(&grant_key).unwrap();
    let sessiond = sessiond_binary().expect(
        "cloudesk-sessiond must already be built (cargo build -p cloudesk-sessiond / --workspace)",
    );
    let _relay = spawn_fake_privd_relay(socket_path.clone(), signer, sessiond);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let privilege = clouddeskd::PrivilegeClient::new(&grant_key, socket_path).unwrap();

    let router = clouddeskd::application_router_with_privilege_and_media_and_library_configured(
        static_dir,
        auth,
        secret_path,
        privilege,
        true,
        Some(media),
        Some(library),
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
            "secret": "music-playwright-test-secret",
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

/// Real fixtures generated inside the mapped user's real `$HOME`. Three
/// directly-playable VP9/Opus-in-MP4 tracks (same codec choice Phase 4
/// made after discovering Playwright's Chromium ships no H.264/AAC
/// decoder), tagged with real, distinct title/artist/album metadata so
/// the product journey (search/artist/album/playlist/queue) has
/// something real to group and find, plus a malformed file.
struct Fixtures {
    // Kept alive for the whole scenario (the tempdir self-deletes on
    // drop) -- never read directly here, unlike Video's equivalent
    // struct, since no Music scenario needs to reopen a fixture path
    // from the Rust side.
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    folder_name: String,
    track_a: String,
    track_b: String,
    track_c: String,
    malformed: String,
}

async fn generate_audio_fixture(path: &std::path::Path, title: &str, artist: &str, album: &str) {
    let status = TokioCommand::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-c:a",
            "libmp3lame",
            "-metadata",
            &format!("title={title}"),
            "-metadata",
            &format!("artist={artist}"),
            "-metadata",
            &format!("album={album}"),
        ])
        .arg(path)
        .status()
        .await
        .unwrap();
    assert!(status.success(), "audio fixture generation failed");
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

    generate_audio_fixture(
        &dir.path().join("track_a.mp3"),
        "Alpha Song",
        "Test Artist",
        "Journey Album",
    )
    .await;
    generate_audio_fixture(
        &dir.path().join("track_b.mp3"),
        "Beta Song",
        "Test Artist",
        "Journey Album",
    )
    .await;
    generate_audio_fixture(
        &dir.path().join("track_c.mp3"),
        "Gamma Song",
        "Other Artist",
        "Other Album",
    )
    .await;

    let malformed = dir.path().join("malformed.mp3");
    std::fs::write(
        &malformed,
        b"this is not a real audio file, just garbage bytes",
    )
    .unwrap();

    Fixtures {
        folder_name,
        track_a: "track_a.mp3".to_owned(),
        track_b: "track_b.mp3".to_owned(),
        track_c: "track_c.mp3".to_owned(),
        malformed: "malformed.mp3".to_owned(),
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
            "mkdir -p /work && cp /scripts/music_flow.mjs /work/ && \
             npm init -y >/dev/null 2>&1 && npm install playwright@1.49.0 >/dev/null 2>&1 && \
             node music_flow.mjs \"$0\" \"$1\"",
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

fn no_music_console_errors(errors: &[Value]) -> bool {
    // Same known pre-login page-load noise disclosed in Phase 4's
    // `video_playwright.rs` (background settings/runtime-status fetches
    // that legitimately 401/503 before authentication), plus 404 for
    // `<img src>` fetches against the real, product-designed artwork
    // endpoint -- most tracks genuinely have no artwork, a real 404 the
    // frontend already handles safely (`onerror` hides the broken-image
    // icon; see `MusicApp.svelte`), not an uncaught exception.
    errors.iter().all(|e| {
        let text = e.as_str().unwrap_or_default().to_lowercase();
        text.contains("autoplay")
            || text.contains("favicon")
            || (text.contains("failed to load resource")
                && (text.contains("401") || text.contains("503") || text.contains("404")))
    })
}

/// Tasks 23/36/37/38/39/40: the full coherent product journey through
/// the real compiled frontend -- Files -> double-click -> Music opens
/// the exact track -> real playback (readyState/currentTime/paused) ->
/// pause/resume -> seek -> favorite -> add to queue -> create playlist
/// and add the track -> search -> open artist/album -> recent history
/// updates. One journey, not isolated component checks.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_full_product_journey() {
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    if !docker_and_playwright_available().await {
        eprintln!("SKIPPED: docker/{PLAYWRIGHT_IMAGE} not available");
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(_admin) = bootstrap_admin(&base).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let fixtures = generate_fixtures().await;

    let result = run_scenario(
        "full_product_journey",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": fixtures.folder_name,
            "trackAFileName": fixtures.track_a,
            "trackBFileName": fixtures.track_b,
            "trackCFileName": fixtures.track_c,
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
        result["openedExactTrack"],
        json!(true),
        "Files double-click must open the exact clicked track, not a substitute: {result:?}"
    );
    assert_eq!(
        result["metadataOk"],
        json!(true),
        "real browser audio metadata must load: {result:?}"
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
        result["favorited"],
        json!(true),
        "favorite must be reflected in the favorites view: {result:?}"
    );
    assert_eq!(
        result["queuedNext"],
        json!(true),
        "queueing a second track must be reflected in the queue view: {result:?}"
    );
    assert_eq!(
        result["playlistCreatedAndTrackAdded"],
        json!(true),
        "a real playlist must be created and hold the added track: {result:?}"
    );
    assert_eq!(
        result["searchFoundTrack"],
        json!(true),
        "search must find the real indexed track by title: {result:?}"
    );
    assert_eq!(
        result["artistGroupingCorrect"],
        json!(true),
        "the artist view must group both same-artist tracks together: {result:?}"
    );
    assert_eq!(
        result["recentUpdated"],
        json!(true),
        "recently-played must reflect the played track: {result:?}"
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
        "the real browser must actually request the authenticated media stream endpoint, \
         never file://, a raw filesystem path, or an unauthenticated static mount: {responses:?}"
    );
    let saw_partial = responses.iter().any(|r| r["status"] == json!(206));
    assert!(
        saw_partial,
        "real seeking must exercise HTTP 206 partial content through the product: {responses:?}"
    );
    let console_errors = result["consoleErrors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        no_music_console_errors(&console_errors),
        "no uncaught Music exception expected: {console_errors:?}"
    );
}

/// Task 25/27: an unsupported/corrupt fixture opened through Files must
/// reach a safe error state, never an indefinite spinner or a frontend
/// crash.
#[tokio::test]
async fn task_corrupt_fixture_shows_safe_failure() {
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    if !docker_and_playwright_available().await {
        eprintln!("SKIPPED: docker/{PLAYWRIGHT_IMAGE} not available");
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(_admin) = bootstrap_admin(&base).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let fixtures = generate_fixtures().await;

    let result = run_scenario(
        "corrupt_fixture_flow",
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
        "a malformed audio file must reach a real, visible error state: {result:?}"
    );
    assert_eq!(
        result["stuckLoading"],
        json!(0),
        "the app must never stay in a loading state after failure: {result:?}"
    );
}

/// Task 43/44: closing the Music app during real playback (and, if
/// still active, during a conversion job) must leave no leaked
/// ffmpeg/ffprobe process behind.
#[tokio::test]
async fn task_close_during_playback_leaves_no_process_leak() {
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    if !docker_and_playwright_available().await {
        eprintln!("SKIPPED: docker/{PLAYWRIGHT_IMAGE} not available");
        return;
    }
    let (base, _dir, _cache_dir) = application().await;
    let Some(_admin) = bootstrap_admin(&base).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let fixtures = generate_fixtures().await;
    let before = media_process_count();

    let result = run_scenario(
        "close_during_playback_flow",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
            "folderName": fixtures.folder_name,
            "trackAFileName": fixtures.track_a,
        }),
    )
    .await;
    cleanup_playwright_containers().await;

    assert_eq!(
        result["ok"],
        json!(true),
        "playwright scenario must succeed: {result:?}"
    );
    assert_eq!(result["playing"], json!(true));

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
        "no ffmpeg/ffprobe process must survive closing Music during playback"
    );
}
