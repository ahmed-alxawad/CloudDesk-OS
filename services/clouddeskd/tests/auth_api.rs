use std::{fs, net::SocketAddr};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, Method, Request, StatusCode},
    Router,
};
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn application() -> (Router, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool,
        SecretCipher::new(&[9_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    fs::write(&secret_path, "one-time-test-secret\n").unwrap();
    (
        clouddeskd::application_router(directory.path().to_owned(), auth, secret_path),
        directory,
    )
}

fn request(method: Method, uri: &str, body: &str, cookie: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::USER_AGENT, "integration-test")
        .body(Body::from(body.to_owned()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:43123".parse::<SocketAddr>().unwrap(),
    ));
    if let Some(cookie) = cookie {
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
    }
    request
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn bootstrap_login_authorization_and_logout_are_enforced_server_side() {
    let (app, directory) = application().await;

    let unauthorized = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/admin/ping", "", None))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        unauthorized.headers()[header::STRICT_TRANSPORT_SECURITY],
        "max-age=31536000; includeSubDomains"
    );

    let invalid_bootstrap = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            r#"{"secret":"wrong","username":"admin","display_name":"Admin","password":"correct horse battery staple"}"#,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_bootstrap.status(), StatusCode::UNAUTHORIZED);

    let bootstrap = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            r#"{"secret":"one-time-test-secret","username":"admin","display_name":"Admin","password":"correct horse battery staple","ui_mode":"desktop","enable_browser":false,"enable_code":false,"enable_office":false}"#,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::CREATED);
    assert!(!directory.path().join("bootstrap.secret").exists());

    let login = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/login",
            r#"{"username":"admin","password":"correct horse battery staple","remember_device":false}"#,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let set_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("Secure; HttpOnly; SameSite=Strict"));
    let cookie = set_cookie.split(';').next().unwrap();

    let authorized = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/admin/ping", "", Some(cookie)))
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);

    let summary = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/system/summary",
            "",
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);

    let service_without_step_up = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/system/services/control",
            r#"{"unit":"ssh.service","operation":"restart"}"#,
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(service_without_step_up.status(), StatusCode::FORBIDDEN);

    let vault_without_step_up = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/vault/secrets",
            r#"{"kind":"ssh.password","label":"Server","value":"secret"}"#,
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(vault_without_step_up.status(), StatusCode::FORBIDDEN);

    let step_up = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/step-up",
            r#"{"password":"correct horse battery staple"}"#,
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(step_up.status(), StatusCode::OK);

    let vault = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/vault/secrets",
            r#"{"kind":"ssh.password","label":"Server","value":"secret"}"#,
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(vault.status(), StatusCode::CREATED);

    let server = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/remote/servers",
            r#"{"name":"Production","hostname":"server.example","port":22,"username":"deploy","auth_method":"ssh_agent","credential_secret_id":null,"agent_socket_path":"/tmp/test-agent.sock","host_key_type":"ssh-ed25519","host_key_base64":"BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=","proxy_jump_server_id":null,"tags":["production"]}"#,
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(server.status(), StatusCode::CREATED);
    let servers = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/remote/servers",
            "",
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(servers.status(), StatusCode::OK);

    let preferences = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/api/v1/preferences",
            r#"{"ui_mode":"dashboard","layout":{"files":{"x":12,"y":24}},"favorites":["files"],"recent":[]}"#,
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(preferences.status(), StatusCode::NO_CONTENT);

    let preferences = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/preferences",
            "",
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(preferences.status(), StatusCode::OK);

    let runtime_settings = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/runtime-settings",
            "",
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(runtime_settings.status(), StatusCode::OK);

    let transfer = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/transfers",
            r#"{"source":{"provider":"local","path":"source.txt"},"destination":{"provider":"local","path":"destination.txt"},"bytes_total":42}"#,
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(transfer.status(), StatusCode::CREATED);
    let transfer_body: serde_json::Value =
        serde_json::from_slice(&transfer.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let transfer_id = transfer_body["transfer_id"].as_str().unwrap();

    for (operation, expected) in [
        ("pause", StatusCode::NO_CONTENT),
        ("resume", StatusCode::NO_CONTENT),
        ("cancel", StatusCode::NO_CONTENT),
        ("resume", StatusCode::CONFLICT),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/api/v1/transfers/{transfer_id}/{operation}"),
                "{}",
                Some(cookie),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let transfers = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/transfers", "", Some(cookie)))
        .await
        .unwrap();
    assert_eq!(transfers.status(), StatusCode::OK);

    let logout = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/logout",
            "{}",
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);

    let revoked = app
        .oneshot(request(Method::GET, "/api/v1/auth/me", "", Some(cookie)))
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}

/// Regression test for CLAUDE-NIGHTMARE-001: `GET /api/v1/system/summary`
/// only required authentication, not the `system.services.manage`
/// capability that its sibling host-administration endpoints require. A
/// Guest-role user (default capability: `files.local.read` only) could call
/// it directly and read hostname/kernel/memory/load/container-engine data
/// that the Settings UI otherwise only renders for Administrators.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn guest_role_cannot_read_system_summary() {
    let (app, directory) = application().await;

    let bootstrap = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            r#"{"secret":"one-time-test-secret","username":"admin","display_name":"Admin","password":"correct horse battery staple"}"#,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::CREATED);
    let _directory = directory;

    let admin_login = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/login",
            r#"{"username":"admin","password":"correct horse battery staple"}"#,
            None,
        ))
        .await
        .unwrap();
    let admin_cookie = admin_login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let step_up = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/step-up",
            r#"{"password":"correct horse battery staple"}"#,
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(step_up.status(), StatusCode::OK);

    let create_guest = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/users",
            r#"{"username":"guest1","display_name":"Guest One","password":"guest horse battery staple","role_ids":["guest"]}"#,
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create_guest.status(), StatusCode::CREATED);

    let guest_login = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/login",
            r#"{"username":"guest1","password":"guest horse battery staple"}"#,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(guest_login.status(), StatusCode::OK);
    let guest_cookie = guest_login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let create_user_role = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/users",
            r#"{"username":"user1","display_name":"User One","password":"user horse battery staple","role_ids":["user"]}"#,
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create_user_role.status(), StatusCode::CREATED);

    let user_login = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/login",
            r#"{"username":"user1","password":"user horse battery staple"}"#,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(user_login.status(), StatusCode::OK);
    let user_cookie = user_login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let admin_summary = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/system/summary",
            "",
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(admin_summary.status(), StatusCode::OK);

    let guest_summary = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/system/summary",
            "",
            Some(&guest_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(guest_summary.status(), StatusCode::FORBIDDEN);

    // The "user" role has a broad workspace capability set (files, remote,
    // transfers, terminal, optional apps) but not `system.services.manage` —
    // it must be rejected exactly like Guest.
    let user_summary = app
        .oneshot(request(
            Method::GET,
            "/api/v1/system/summary",
            "",
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(user_summary.status(), StatusCode::FORBIDDEN);
}
