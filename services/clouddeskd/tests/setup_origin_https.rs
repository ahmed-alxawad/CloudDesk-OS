//! Publication Pass "RC.5 Corrective Release": real-TLS, real-HTTP/2
//! reproduction and regression coverage for the setup same-origin/CSRF
//! defect discovered against a real public v1.0.1-rc.4 installation.
//!
//! Every other test in this crate that exercises `web_security`'s
//! cross-site rejection (`health.rs`'s `oneshot`-based control,
//! `csrf_playwright.rs`'s real-browser control) runs the router through
//! plain `axum::serve` -- no TLS, no HTTP/2. That is not what
//! `clouddeskd serve` actually does in production
//! (`axum_server::bind_rustls`, whose `RustlsConfig::from_pem_file`
//! advertises `h2` before `http/1.1` in ALPN -- see
//! `services/clouddeskd/src/main.rs`), and it is exactly why this
//! defect reached a real user: HTTP/2 requests carry their authority in
//! the `:authority` pseudo-header, not a `Host` header, and this
//! server's HTTP/2 stack never synthesized one into the header map the
//! application saw. A same-origin browser request (browsers negotiate
//! HTTP/2 whenever a server offers it) was unconditionally rejected as
//! cross-site. This file stands up the real `axum_server::bind_rustls`
//! listener, a real self-signed certificate, and real HTTP/1.1 and
//! HTTP/2 clients to prove the fix and guard against regressing either
//! protocol version or the cross-origin/scheme/port rejections that
//! must remain in place.

use std::net::SocketAddr;

use axum_server::tls_rustls::RustlsConfig;
use serde_json::json;

/// Real, disposable HTTPS `CloudDesk` instance: the actual production
/// listener path (`axum_server::bind_rustls`), a real self-signed
/// certificate (SANs covering every host this test addresses it as),
/// and a real in-memory database -- everything `main.rs::serve` does
/// for the production HTTPS branch, minus the privileged helper and
/// optional runtimes, which are irrelevant to origin validation.
struct HttpsFixture {
    addr: SocketAddr,
    bootstrap_secret: String,
    _dir: tempfile::TempDir,
}

impl HttpsFixture {
    async fn spawn() -> Self {
        // The server side (axum-server/rustls, workspace-pinned to the
        // aws-lc-rs backend) and the client side (reqwest's
        // rustls-tls feature) can each pull in a rustls crypto
        // provider; with more than one candidate available in the
        // dependency graph, rustls refuses to guess. Installing one
        // explicitly, once per process, removes the ambiguity for
        // both roles in this test binary.
        static CRYPTO_PROVIDER: std::sync::Once = std::sync::Once::new();
        CRYPTO_PROVIDER.call_once(|| {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        });

        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        let auth = clouddesk_auth::AuthService::new(
            pool,
            clouddesk_secrets::SecretCipher::new(&[199_u8; 32]).unwrap(),
            clouddesk_auth::AuthPolicy::default(),
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let bootstrap_secret = "setup-origin-https-bootstrap-secret".to_owned();
        let secret_path = dir.path().join("bootstrap.secret");
        std::fs::write(&secret_path, format!("{bootstrap_secret}\n")).unwrap();

        let router = clouddeskd::application_router_configured(
            dir.path().to_owned(),
            auth,
            secret_path,
            true, // enforce_hsts=true: the production HTTPS branch's setting.
        );

        let cert = rcgen::generate_simple_self_signed(vec![
            "localhost".to_owned(),
            "127.0.0.1".to_owned(),
        ])
        .unwrap();
        let cert_pem = cert.cert.pem();
        let key_pem = cert.key_pair.serialize_pem();
        let cert_path = dir.path().join("server.crt");
        let key_path = dir.path().join("server.key");
        std::fs::write(&cert_path, cert_pem).unwrap();
        std::fs::write(&key_path, key_pem).unwrap();

        let tls = RustlsConfig::from_pem_file(&cert_path, &key_path)
            .await
            .unwrap();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let _ = axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                .await;
        });
        // Give the listener a moment to start accepting.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        Self {
            addr,
            bootstrap_secret,
            _dir: dir,
        }
    }

    fn base_url(&self, host: &str) -> String {
        format!("https://{host}:{}", self.addr.port())
    }
}

fn client(http2_only: bool) -> reqwest::Client {
    let builder = reqwest::Client::builder().danger_accept_invalid_certs(true);
    let builder = if http2_only {
        builder.http2_prior_knowledge()
    } else {
        builder.http1_only()
    };
    builder.build().unwrap()
}

async fn setup_status(fixture: &HttpsFixture, http2: bool) -> serde_json::Value {
    client(http2)
        .get(format!(
            "{}/api/v1/setup/status",
            fixture.base_url("127.0.0.1")
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn bootstrap_attempt(
    fixture: &HttpsFixture,
    http2: bool,
    request_host: &str,
    origin: &str,
) -> reqwest::StatusCode {
    client(http2)
        .post(format!(
            "{}/api/v1/setup/bootstrap",
            fixture.base_url(request_host)
        ))
        .header(reqwest::header::ORIGIN, origin)
        .json(&json!({
            // Deliberately wrong: every ALLOW-class case in this file
            // asserts the request got PAST origin validation (never a
            // 403 "cross-site request rejected"), not that bootstrap
            // itself succeeded -- so an invalid secret is used
            // throughout to guarantee no test here can accidentally
            // create a real administrator account.
            "secret": "deliberately-wrong-secret",
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
        }))
        .send()
        .await
        .unwrap()
        .status()
}

async fn bootstrap_attempt_raw_origin_header(
    fixture: &HttpsFixture,
    http2: bool,
    origin_header_value: &[u8],
) -> reqwest::StatusCode {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ORIGIN,
        reqwest::header::HeaderValue::from_bytes(origin_header_value).unwrap(),
    );
    client(http2)
        .post(format!(
            "{}/api/v1/setup/bootstrap",
            fixture.base_url("127.0.0.1")
        ))
        .headers(headers)
        .json(&json!({"secret": "x", "username": "a", "display_name": "A", "password": "x"}))
        .send()
        .await
        .unwrap()
        .status()
}

const FORBIDDEN: reqwest::StatusCode = reqwest::StatusCode::FORBIDDEN;

/// Reproduces the exact real-world failure: a same-origin HTTP/2
/// request against `https://127.0.0.1:<port>` was rejected with 403
/// "cross-site request rejected" before this fix, purely because the
/// HTTP/2 stack never populated a `Host` header the old
/// `origin_matches_host` exclusively looked for.
#[tokio::test]
async fn http2_same_origin_setup_request_is_not_rejected_as_cross_site() {
    let fixture = HttpsFixture::spawn().await;
    let status = setup_status(&fixture, true).await;
    assert_eq!(status["bootstrap_required"], true);

    let status =
        bootstrap_attempt(&fixture, true, "127.0.0.1", &fixture.base_url("127.0.0.1")).await;
    assert_ne!(
        status, FORBIDDEN,
        "same-origin HTTP/2 request must not be rejected as cross-site"
    );
}

#[tokio::test]
async fn http1_1_same_origin_setup_request_is_not_rejected_as_cross_site() {
    let fixture = HttpsFixture::spawn().await;
    let status =
        bootstrap_attempt(&fixture, false, "127.0.0.1", &fixture.base_url("127.0.0.1")).await;
    assert_ne!(
        status, FORBIDDEN,
        "same-origin HTTP/1.1 request must not be rejected as cross-site"
    );
}

#[tokio::test]
async fn http2_same_origin_localhost_is_not_rejected_as_cross_site() {
    let fixture = HttpsFixture::spawn().await;
    let status =
        bootstrap_attempt(&fixture, true, "localhost", &fixture.base_url("localhost")).await;
    assert_ne!(status, FORBIDDEN, "same-origin localhost must be allowed");
}

#[tokio::test]
async fn http2_foreign_origin_is_rejected() {
    let fixture = HttpsFixture::spawn().await;
    let status = bootstrap_attempt(&fixture, true, "127.0.0.1", "https://evil.example").await;
    assert_eq!(
        status, FORBIDDEN,
        "a genuinely foreign origin must be rejected"
    );
}

/// Added by this fix, not merely preserved: the previous
/// `origin_matches_host` compared only host:port, never scheme, so an
/// `Origin: http://...` would have passed unmodified against an HTTPS
/// deployment.
#[tokio::test]
async fn http2_scheme_mismatch_origin_is_rejected() {
    let fixture = HttpsFixture::spawn().await;
    let origin = format!("http://127.0.0.1:{}", fixture.addr.port());
    let status = bootstrap_attempt(&fixture, true, "127.0.0.1", &origin).await;
    assert_eq!(
        status, FORBIDDEN,
        "an http:// Origin against an HTTPS deployment must be rejected"
    );
}

#[tokio::test]
async fn http2_wrong_port_origin_is_rejected() {
    let fixture = HttpsFixture::spawn().await;
    let status = bootstrap_attempt(&fixture, true, "127.0.0.1", "https://127.0.0.1:1").await;
    assert_eq!(status, FORBIDDEN, "a mismatched port must be rejected");
}

#[tokio::test]
async fn http2_malformed_origin_is_rejected() {
    let fixture = HttpsFixture::spawn().await;
    let status = bootstrap_attempt_raw_origin_header(&fixture, true, b"not-a-url").await;
    assert_eq!(
        status, FORBIDDEN,
        "a malformed Origin header must be rejected"
    );
}

/// End-to-end live acceptance: a real same-origin HTTP/2 bootstrap with
/// the correct secret actually creates the administrator, flips
/// `bootstrap_required` to false, and the new administrator can log in
/// -- not just "not a 403".
#[tokio::test]
async fn http2_same_origin_bootstrap_creates_administrator_end_to_end() {
    let fixture = HttpsFixture::spawn().await;
    let base = fixture.base_url("127.0.0.1");

    let resp = client(true)
        .post(format!("{base}/api/v1/setup/bootstrap"))
        .header(reqwest::header::ORIGIN, &base)
        .json(&json!({
            "secret": fixture.bootstrap_secret,
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::CREATED, "body={body}");

    let status = setup_status(&fixture, true).await;
    assert_eq!(status["bootstrap_required"], false);

    let login = client(true)
        .post(format!("{base}/api/v1/auth/login"))
        .header(reqwest::header::ORIGIN, &base)
        .json(&json!({"username": "admin", "password": "correct horse battery staple"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        login.status(),
        reqwest::StatusCode::OK,
        "the administrator created via the same-origin HTTP/2 bootstrap must be able to log in"
    );
}
