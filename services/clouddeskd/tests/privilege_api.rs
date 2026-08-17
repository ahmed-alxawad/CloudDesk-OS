use std::{fs, net::SocketAddr};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, Method, Request, StatusCode},
};
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_privilege::{GrantSigner, PrivdRequest, PrivdResponse, PrivilegedAction, WorkerKind};
use clouddesk_secrets::SecretCipher;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};
use tower::ServiceExt;

fn request(method: Method, uri: &str, body: &Value, cookie: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::USER_AGENT, "privilege-integration-test")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:43124".parse::<SocketAddr>().unwrap(),
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
async fn mapped_worker_grants_are_scoped_signed_and_audited() {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[9_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    fs::write(&secret_path, "one-time-test-secret\n").unwrap();
    let socket_path = directory.path().join("privd.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let key = [77_u8; 32];
    let signer = GrantSigner::new(&key).unwrap();
    let helper = tokio::spawn(async move {
        for request_number in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let length = stream.read_u32().await.unwrap();
            let mut bytes = vec![0_u8; usize::try_from(length).unwrap()];
            stream.read_exact(&mut bytes).await.unwrap();
            let request: PrivdRequest = serde_json::from_slice(&bytes).unwrap();
            signer
                .verify(&request.grant, request.grant.claims.issued_at)
                .unwrap();
            let output = if request_number == 0 {
                assert!(matches!(
                    request.grant.claims.action,
                    PrivilegedAction::SpawnUserWorker {
                        worker: WorkerKind::IdentityProbe,
                        ..
                    }
                ));
                json!({ "identity": "verified" })
            } else {
                assert!(matches!(
                    request.grant.claims.action,
                    PrivilegedAction::LocalFileOperation { .. }
                ));
                json!({ "result": "entries", "entries": [], "capabilities": ["read"] })
            };
            let response = serde_json::to_vec(&PrivdResponse {
                accepted: true,
                message: "action completed".to_owned(),
                output: Some(output),
            })
            .unwrap();
            stream
                .write_u32(u32::try_from(response.len()).unwrap())
                .await
                .unwrap();
            stream.write_all(&response).await.unwrap();
        }
    });
    let app = clouddeskd::application_router_with_privilege(
        directory.path().to_owned(),
        auth,
        secret_path,
        clouddeskd::PrivilegeClient::new(&key, socket_path).unwrap(),
    );

    let bootstrap = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            &json!({
                "secret": "one-time-test-secret",
                "username": "admin",
                "display_name": "Admin",
                "password": "correct horse battery staple"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::CREATED);
    let bootstrap_body: Value =
        serde_json::from_slice(&bootstrap.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let user_id = bootstrap_body["user_id"].as_str().unwrap();

    let login = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/login",
            &json!({
                "username": "admin",
                "password": "correct horse battery staple"
            }),
            None,
        ))
        .await
        .unwrap();
    let set_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    let cookie = set_cookie.split(';').next().unwrap();
    let step_up = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/step-up",
            &json!({ "password": "correct horse battery staple" }),
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(step_up.status(), StatusCode::OK);

    let nobody = clouddesk_linux::lookup_user("nobody")
        .unwrap()
        .expect("test image must provide the standard nobody account");
    let mapping = app
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/users/{user_id}/linux-identity"),
            &json!({ "uid": nobody.uid, "gid": nobody.gid }),
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(mapping.status(), StatusCode::NO_CONTENT);

    let worker = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/privilege/workers",
            &json!({ "worker": "identity_probe" }),
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(worker.status(), StatusCode::OK);

    let files = app
        .oneshot(request(
            Method::POST,
            "/api/v1/files/local/actions",
            &json!({ "operation": "list", "path": "/" }),
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(files.status(), StatusCode::OK);
    helper.await.unwrap();

    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events
         WHERE action IN ('privilege.grant.issue', 'privilege.action.complete')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 4);
    clouddesk_audit::verify_chain(&pool).await.unwrap();
}
