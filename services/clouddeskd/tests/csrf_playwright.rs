//! Phase 16D Part 8-13: real, two-origin CSRF browser control.
//!
//! `services/clouddeskd/tests/health.rs::cross_site_mutations_are_rejected_before_routing`
//! already proves the server-side middleware rejects a request carrying
//! `Origin: https://evil.example` / `Sec-Fetch-Site: cross-site` headers,
//! using a handcrafted `oneshot` request. That is not the same claim as
//! "a real browser, genuinely navigating cross-origin with an
//! authenticated session cookie already set, cannot pull off this
//! attack" -- the property depends on the browser actually attaching
//! (or refusing to attach) the session cookie and setting those headers
//! itself, not on this project's own code constructing them correctly.
//!
//! This test stands up two real, disposable, different-port HTTP
//! origins on the same host (a different port is a different origin to
//! a browser -- scheme+host+port -- sufficient to exercise real
//! `SameSite`/`Sec-Fetch-Site`/`Origin` behavior without a second real
//! domain), logs a real Chromium instance into the real `CloudDesk`
//! origin, then navigates it to the "attacker" origin, whose page
//! attempts a real cross-origin `fetch()` with `credentials: 'include'`
//! against `PUT /api/v1/preferences` -- a genuine, real, JSON,
//! session-authenticated settings-mutation endpoint.

use std::net::SocketAddr;

use axum::http::Method;
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;

const PLAYWRIGHT_IMAGE: &str = "mcr.microsoft.com/playwright:v1.49.0-noble";

async fn docker_available() -> bool {
    TokioCommand::new("docker")
        .arg("info")
        .output()
        .await
        .is_ok_and(|output| output.status.success())
}

/// A minimal real `CloudDesk` server: the actual compiled router, a
/// real bootstrap, a real login -- no runtime/OCI machinery, since
/// none of it is relevant to a CSRF/cookie-attachment question.
async fn spawn_cloudesk_origin() -> (String, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = clouddesk_auth::AuthService::new(
        pool,
        clouddesk_secrets::SecretCipher::new(&[211_u8; 32]).unwrap(),
        clouddesk_auth::AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "csrf-test-bootstrap-secret\n").unwrap();
    // Publication Pass K2 found this live: this harness spawns a plain
    // (non-TLS) axum::serve listener below, but was passing
    // application_router's enforce_hsts=true default -- the same
    // signal main.rs only ever pairs with the real
    // axum_server::bind_rustls/HTTPS branch. origin_matches_host's own
    // scheme check (added for the rc.5/rc.6 HTTP/2 same-origin fix)
    // takes enforce_hsts as "this deployment is HTTPS", so it expected
    // Origin: https://... against a browser that, correctly, reported
    // Origin: http://... for this genuinely-HTTP listener -- rejecting
    // even the test's own legitimate same-origin request. false here
    // matches what this harness actually serves.
    let router = clouddeskd::application_router_configured(
        directory.path().to_owned(),
        auth,
        secret_path,
        false,
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    (format!("http://127.0.0.1:{}", addr.port()), directory)
}

/// A real, disposable, plain-HTTP "attacker" origin serving a real
/// attack page over a real HTTP response -- not a Playwright request
/// interception (an earlier draft of this test used
/// `page.route().fulfill()` to fake the response, which Chromium
/// treats as an opaque/`null`-origin document, defeating the entire
/// point of a *real* cross-origin test). The page content is fully
/// under this test's control and auditable, embedding the real
/// `CloudDesk` origin as a query parameter so the served HTML can target
/// the right `fetch()` URL without any client-side guessing.
async fn spawn_attacker_origin(cloudesk_base: String) -> String {
    let app = axum::Router::new().fallback(move || {
        let cloudesk_base = cloudesk_base.clone();
        async move {
            axum::response::Html(format!(
                r"<!doctype html><html><body>
<script>
window.__attackResult = (async () => {{
  try {{
    const r = await fetch({target:?}, {{
      method: 'PUT',
      credentials: 'include',
      headers: {{ 'Content-Type': 'application/json' }},
      body: JSON.stringify({{
        ui_mode: 'dashboard',
        layout: {{ attacker: true }},
        favorites: [],
        recent: []
      }})
    }});
    return {{ status: r.status, body: await r.text().catch(() => '') }};
  }} catch (e) {{
    return {{ error: String(e) }};
  }}
}})();
</script>
</body></html>",
                target = format!("{cloudesk_base}/api/v1/preferences"),
            ))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn bootstrap_admin(cloudesk_base: &str) {
    let client = reqwest::Client::new();
    let response = client
        .request(
            Method::POST,
            format!("{cloudesk_base}/api/v1/setup/bootstrap"),
        )
        .json(&json!({
            "secret": "csrf-test-bootstrap-secret",
            "username": "csrfadmin",
            "display_name": "CSRF Test Admin",
            "password": "correct horse battery staple csrf",
            "ui_mode": "desktop",
            "enable_browser": false,
            "enable_code": false,
            "enable_office": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        201,
        "bootstrap must succeed for this test to be meaningful"
    );
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
            "mkdir -p /work && cp /scripts/csrf_flow.mjs /work/ && \
             npm init -y >/dev/null 2>&1 && npm install playwright@1.49.0 >/dev/null 2>&1 && \
             node csrf_flow.mjs \"$0\" \"$1\"",
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
        json!({ "ok": false, "error": format!("could not parse playwright output: {e}"), "stdout": stdout.to_string() })
    })
}

#[tokio::test]
async fn csrf_cross_origin_fetch_with_credentials_is_rejected() {
    if !docker_available().await {
        clouddesk_test_support::blocked_by_environment(
            "csrf_cross_origin_fetch_with_credentials_is_rejected",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (cloudesk_base, _directory) = spawn_cloudesk_origin().await;
    bootstrap_admin(&cloudesk_base).await;
    let attacker_base = spawn_attacker_origin(cloudesk_base.clone()).await;

    let result = run_scenario(
        "csrf_cross_origin_attack",
        &json!({
            "cloudeskBase": cloudesk_base,
            "attackerBase": attacker_base,
            "username": "csrfadmin",
            "password": "correct horse battery staple csrf",
        }),
    )
    .await;
    eprintln!("csrf cross-origin attack result: {result}");

    assert!(
        result.get("error").is_none(),
        "scenario itself failed to run: {result:?}"
    );

    // Part 11: positive control -- the legitimate, same-origin mutation
    // must have succeeded, proving the endpoint/fixture is functional.
    assert_eq!(
        result["legitimateMutation"]["status"],
        json!(204),
        "the legitimate same-origin PUT must succeed (200/204) for this test to prove anything: {result:?}"
    );

    // Part 12: cookie security actually observed live from the browser.
    assert_eq!(result["sessionCookie"]["secure"], json!(true));
    assert_eq!(result["sessionCookie"]["httpOnly"], json!(true));
    assert_eq!(result["sessionCookie"]["sameSite"], json!("Strict"));

    // Part 10: PASS requires the mutation demonstrably never happened
    // server-side -- not merely that the attacker's own fetch()
    // reported an error (a frontend-visible signal alone is explicitly
    // insufficient evidence per this pass's own instructions).
    assert_eq!(
        result["attackerPayloadLanded"],
        json!(false),
        "the cross-origin attacker payload (ui_mode=dashboard, layout.attacker=true) must never \
         reach server-side state -- got final state {:?}",
        result["finalState"]
    );
}
