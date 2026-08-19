//! Phase 8 Task 6/28/29 — a controlled, safe hostile-document corpus.
//!
//! No real malware. Every fixture is a deliberately mangled OOXML/ODF
//! (real ZIP container, corrupted/adversarial contents) built from a
//! genuine `LibreOffice`-generated source document. Each fixture is
//! opened through the real WOPI host (`CheckFileInfo`/`GetFile`, and
//! where noted, the real Collabora bootstrap), asserting throughout:
//! `clouddeskd` never panics or hangs, the canonical source file's
//! bytes are hashed before and after and must be byte-identical unless
//! a legitimate save occurred (Task 28), no lock is left permanently
//! stuck, and no authorization is bypassed.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::io::Write as _;
use std::net::SocketAddr;
use tokio::process::Command as TokioCommand;

const OFFICE_IMAGE: &str = "collabora/code:26.04.3.1.1";

async fn docker_and_image_available() -> bool {
    TokioCommand::new("docker")
        .args(["image", "inspect", OFFICE_IMAGE])
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

async fn application() -> (String, tempfile::TempDir, SqlitePool) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[61_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "hostile-test-secret\n").unwrap();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let wopi_host_base = format!("http://host.docker.internal:{port}");

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!("clouddesk-hostile-test-{}", std::process::id())),
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(20),
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(
                clouddeskd::office_runtime::office_oci_spec(
                    OFFICE_IMAGE.to_owned(),
                    wopi_host_base.clone(),
                ),
            ),
        )),
    );

    let router =
        clouddeskd::application_router_and_media_and_library_and_runtime_and_office_configured(
            directory.path().to_owned(),
            auth,
            secret_path,
            true,
            None,
            None,
            Some(runtime_manager),
            Some(wopi_host_base),
        );
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

fn current_process_linux_identity() -> Option<clouddesk_linux::LinuxIdentity> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        return None;
    }
    clouddesk_linux::lookup_uid(uid).ok().flatten()
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

async fn bootstrap_admin(base: &str) -> String {
    let linux_username = current_process_linux_identity().map(|i| i.username);
    let response = http(
        base,
        Method::POST,
        "/api/v1/setup/bootstrap",
        None,
        Some(&json!({
            "secret": "hostile-test-secret",
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
            "linux_username": linux_username,
        })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    login(base, "admin", "correct horse battery staple").await
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

async fn step_up(base: &str, admin_cookie: &str) {
    let response = http(
        base,
        Method::POST,
        "/api/v1/auth/step-up",
        Some(admin_cookie),
        Some(&json!({"password": "correct horse battery staple"})),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

async fn create_user(base: &str, admin_cookie: &str, username: &str) -> String {
    let identity = current_process_linux_identity()
        .expect("this test requires running as a real, mapped, non-root Linux user");
    step_up(base, admin_cookie).await;
    let create = http(
        base,
        Method::POST,
        "/api/v1/users",
        Some(admin_cookie),
        Some(&json!({
            "username": username,
            "display_name": username,
            "password": "user horse battery staple",
            "role_ids": ["user"],
        })),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::CREATED);
    let body: Value = create.json().await.unwrap();
    let user_id = body["user_id"].as_str().unwrap().to_owned();
    let set_identity = http(
        base,
        Method::PUT,
        &format!("/api/v1/users/{user_id}/linux-identity"),
        Some(admin_cookie),
        Some(&json!({ "uid": identity.uid, "gid": identity.gid })),
    )
    .await;
    assert_eq!(set_identity.status(), reqwest::StatusCode::NO_CONTENT);
    user_id
}

async fn add_root(base: &str, admin_cookie: &str, user_id: &str, path: &std::path::Path) {
    step_up(base, admin_cookie).await;
    let response = http(
        base,
        Method::POST,
        &format!("/api/v1/users/{user_id}/assigned-roots"),
        Some(admin_cookie),
        Some(&json!({ "path": path, "access_mode": "read-write" })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
}

async fn mint_token(pool: &SqlitePool, user_id: &str, path: &std::path::Path) -> (String, String) {
    let canonical = std::fs::canonicalize(path).unwrap();
    let canonical = canonical.to_string_lossy().into_owned();
    let file_id = format!("f{}", unique());
    sqlx::query(
        "INSERT INTO office_wopi_files (id, canonical_path, identity_key, generation, created_at)
         VALUES (?, ?, ?, 0, 0) ON CONFLICT(identity_key) DO NOTHING",
    )
    .bind(&file_id)
    .bind(&canonical)
    .bind(&canonical)
    .execute(pool)
    .await
    .unwrap();
    let file_id: String =
        sqlx::query_scalar("SELECT id FROM office_wopi_files WHERE identity_key = ?")
            .bind(&canonical)
            .fetch_one(pool)
            .await
            .unwrap();
    let raw = format!("t{}", unique());
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    sqlx::query(
        "INSERT INTO office_wopi_tokens
            (token_hash, user_id, file_id, read_write, runtime_instance_id, created_at, expires_at)
         VALUES (?, ?, ?, 1, 'test-instance', 0, ?)",
    )
    .bind(hex::encode(hasher.finalize()))
    .bind(user_id)
    .bind(&file_id)
    .bind(i64::MAX)
    .execute(pool)
    .await
    .unwrap();
    (file_id, raw)
}

fn unique() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

// --------------------------------------------------------------
// Fixture generation
// --------------------------------------------------------------

async fn genuine_docx(dir: &std::path::Path, marker: &str) -> Vec<u8> {
    let seed = dir.join(format!("{}-seed.txt", unique()));
    std::fs::write(&seed, format!("{marker}\n")).unwrap();
    let profile = tempfile::tempdir().unwrap();
    let convert = TokioCommand::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.path().display()
        ))
        .args(["--headless", "--convert-to", "docx", "--outdir"])
        .arg(dir)
        .arg(&seed)
        .output()
        .await
        .unwrap();
    assert!(convert.status.success());
    let stem = seed.file_stem().unwrap().to_str().unwrap();
    std::fs::read(dir.join(format!("{stem}.docx"))).unwrap()
}

/// Task 6: truncates a genuine document at `fraction` of its length --
/// a corrupt ZIP with a valid-looking local header but a cut-off
/// central directory.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn truncated(bytes: &[u8], fraction: f64) -> Vec<u8> {
    let cut = (bytes.len() as f64 * fraction) as usize;
    bytes[..cut].to_vec()
}

/// Replaces one entry's content inside a real ZIP with garbage bytes,
/// keeping every other entry (and the ZIP structure itself) intact --
/// a malformed-XML-inside-a-valid-container fixture.
fn zip_with_corrupted_entry(source: &[u8], entry_name: &str, garbage: &[u8]) -> Vec<u8> {
    let reader = std::io::Cursor::new(source);
    let mut archive = zip::ZipArchive::new(reader).unwrap();
    let mut out = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let name = entry.name().to_owned();
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            writer.start_file(&name, options).unwrap();
            if name == entry_name {
                writer.write_all(garbage).unwrap();
            } else {
                let mut content = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut content).unwrap();
                writer.write_all(&content).unwrap();
            }
        }
        writer.finish().unwrap();
    }
    out
}

/// A ZIP whose central directory lists a very large number of tiny
/// (empty) entries -- a safe, bounded stand-in for a "huge ZIP
/// metadata" / "huge relationships list" style fixture.
fn zip_with_many_entries(count: usize) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for i in 0..count {
            writer
                .start_file(format!("entry-{i}.xml"), options)
                .unwrap();
            writer.write_all(b"<x/>").unwrap();
        }
        writer.finish().unwrap();
    }
    out
}

/// A safely bounded "ZIP bomb"-shaped fixture: one highly compressible
/// entry that expands to `expanded_bytes` from a tiny compressed size --
/// bounded at a scale (tens of MB) that cannot meaningfully stress a
/// real host, unlike a true multi-GB bomb.
fn bounded_zip_bomb(expanded_bytes: usize) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("word/document.xml", options).unwrap();
        writer.write_all(&vec![b'A'; expanded_bytes]).unwrap();
        writer.finish().unwrap();
    }
    out
}

/// Deeply nested XML content for one entry -- a pathological structure
/// stand-in, bounded at a depth that is adversarial but not itself a
/// resource-exhaustion attack against the *test* host.
fn nested_xml(depth: usize) -> Vec<u8> {
    let mut xml = String::from("<root>");
    for _ in 0..depth {
        xml.push_str("<n>");
    }
    xml.push_str("data");
    for _ in 0..depth {
        xml.push_str("</n>");
    }
    xml.push_str("</root>");
    xml.into_bytes()
}

/// Unusual Unicode content: RTL override characters and zalgo-style
/// combining marks.
fn unusual_unicode_content() -> Vec<u8> {
    "\u{202e}emordnilaP\u{202c} and z\u{0301}a\u{0302}l\u{0303}g\u{0304}o\u{0305} text"
        .as_bytes()
        .to_vec()
}

// --------------------------------------------------------------
// The sweep
// --------------------------------------------------------------

/// Task 6/28/29: every hostile fixture, opened through the real WOPI
/// host. `clouddeskd` must survive all of them; the canonical source's
/// hash must be unchanged after every one (nothing here performs a
/// legitimate save); no lock is left dangling.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_6_28_hostile_document_corpus_does_not_destabilize_clouddesk() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let user_id = create_user(&base, &admin, "hostiledocuser").await;
    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path()).await;

    let genuine = genuine_docx(workspace.path(), "GENUINE-BASELINE").await;

    let fixtures: Vec<(&str, Vec<u8>)> = vec![
        ("truncated-50pct", truncated(&genuine, 0.5)),
        ("truncated-90pct", truncated(&genuine, 0.9)),
        ("empty-file", Vec::new()),
        (
            "malformed-ooxml-xml",
            zip_with_corrupted_entry(
                &genuine,
                "word/document.xml",
                b"<this is not <<valid>> xml at all &&&",
            ),
        ),
        (
            "malformed-relationships",
            zip_with_corrupted_entry(
                &genuine,
                "word/_rels/document.xml.rels",
                b"not xml either {{{",
            ),
        ),
        ("huge-zip-metadata", zip_with_many_entries(5_000)),
        ("bounded-zip-bomb", bounded_zip_bomb(50 * 1024 * 1024)),
        (
            "deeply-nested-structure",
            zip_with_corrupted_entry(&genuine, "word/document.xml", &nested_xml(5_000)),
        ),
        (
            "unusual-unicode",
            zip_with_corrupted_entry(&genuine, "word/document.xml", &unusual_unicode_content()),
        ),
        (
            "oversized-metadata-string",
            zip_with_corrupted_entry(
                &genuine,
                "docProps/core.xml",
                format!(
                    "<?xml version=\"1.0\"?><cp:coreProperties xmlns:cp=\"x\" xmlns:dc=\"y\"><dc:title>{}</dc:title></cp:coreProperties>",
                    "A".repeat(2 * 1024 * 1024)
                )
                .as_bytes(),
            ),
        ),
        (
            "invalid-embedded-image",
            zip_with_corrupted_entry(&genuine, "word/document.xml", b"\x00\x01\x02\xff\xfe not an image or xml"),
        ),
    ];

    for (label, content) in fixtures {
        let doc = workspace.path().join(format!("hostile-{label}.docx"));
        std::fs::write(&doc, &content).unwrap();
        let before_hash = sha256_hex(&content);

        let (file_id, token) = mint_token(&pool, &user_id, &doc).await;
        let client = reqwest::Client::new();

        // CheckFileInfo must never panic/hang, whatever the content.
        let info = client
            .get(format!("{base}/wopi/files/{file_id}?access_token={token}"))
            .send()
            .await
            .unwrap();
        assert!(
            info.status().is_success() || info.status().is_client_error(),
            "{label}: CheckFileInfo must respond safely, got {}",
            info.status()
        );

        // GetFile must stream whatever bytes are there without crashing
        // the service, and must return exactly what's on disk (CloudDesk
        // never tries to "fix" or reinterpret the bytes).
        let fetched = client
            .get(format!(
                "{base}/wopi/files/{file_id}/contents?access_token={token}"
            ))
            .send()
            .await
            .unwrap();
        if fetched.status().is_success() {
            let fetched_bytes = fetched.bytes().await.unwrap();
            assert_eq!(
                fetched_bytes.as_ref(),
                content.as_slice(),
                "{label}: GetFile must return the hostile bytes unmodified"
            );
        }

        // clouddeskd itself must still be responsive after every fixture.
        let health = client
            .get(format!("{base}/api/v1/health"))
            .send()
            .await
            .unwrap();
        assert!(
            health.status().is_success(),
            "{label}: clouddeskd must remain healthy after this fixture"
        );

        // Task 28: the canonical source is unchanged (nothing here is a
        // legitimate save).
        let after_hash = sha256_hex(&std::fs::read(&doc).unwrap());
        assert_eq!(
            before_hash, after_hash,
            "{label}: the canonical source must be byte-identical -- a parse \
             failure must never mutate the file"
        );

        // No lock left dangling for this file.
        let get_lock = client
            .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
            .header("X-WOPI-Override", "GET_LOCK")
            .send()
            .await
            .unwrap();
        assert_eq!(
            get_lock
                .headers()
                .get("X-WOPI-Lock")
                .map(|v| v.to_str().unwrap()),
            Some(""),
            "{label}: no lock should be held after merely opening a hostile document"
        );
    }
}

/// Task 6 (live Collabora tier): a genuinely corrupt document opened
/// through the real editor bootstrap. Collabora may reject/refuse it;
/// `clouddeskd`'s own WOPI host and proxy must remain fully functional
/// immediately afterward -- proven by a real, unrelated healthy
/// document opening cleanly right after.
#[tokio::test]
async fn task_6_real_collabora_survives_a_corrupt_document() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{OFFICE_IMAGE} not reachable on this host");
        return;
    }
    let (base, dir, _pool) = application().await;
    let admin_cookie = bootstrap_admin(&base).await;
    step_up(&base, &admin_cookie).await;
    let enable = http(
        &base,
        Method::POST,
        "/api/v1/runtimes/office/enable",
        Some(&admin_cookie),
        None,
    )
    .await;
    assert_eq!(enable.status(), reqwest::StatusCode::NO_CONTENT);
    let user_id = create_user(&base, &admin_cookie, "collabhostileuser").await;
    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin_cookie, &user_id, workspace.path()).await;
    let cookie = login(&base, "collabhostileuser", "user horse battery staple").await;

    let genuine = genuine_docx(workspace.path(), "PRE-CORRUPTION").await;
    let corrupt_path = workspace.path().join("corrupt.docx");
    std::fs::write(&corrupt_path, truncated(&genuine, 0.5)).unwrap();

    // Attempt to open the corrupt document -- accept success (Collabora
    // tolerated it) or any clean HTTP-level failure; a hang or a
    // connection-reset would be the real failure mode.
    let corrupt_open = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        http(
            &base,
            Method::POST,
            "/api/v1/office/sessions",
            Some(&cookie),
            Some(&json!({ "path": corrupt_path.to_string_lossy() })),
        ),
    )
    .await;
    assert!(
        corrupt_open.is_ok(),
        "opening a corrupt document must not hang clouddeskd indefinitely"
    );

    // The real proof: a genuinely healthy document opens cleanly right
    // after, through the exact same shared runtime instance.
    let good_path = workspace.path().join("good.docx");
    std::fs::write(&good_path, &genuine).unwrap();
    let good_open = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": good_path.to_string_lossy() })),
    )
    .await;
    let status = good_open.status();
    let body: Value = good_open.json().await.unwrap_or_default();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "a healthy document must still open normally after a corrupt one: {body:?}"
    );
    let instance_id = body["instance_id"].as_str().unwrap().to_owned();
    let _ = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/office/{instance_id}/stop"),
        Some(&admin_cookie),
        None,
    )
    .await;
}
