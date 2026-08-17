use std::{net::SocketAddr, path::PathBuf};

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, Request, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use clouddesk_auth::{
    AssignedRootAccess, AuthError, AuthService, BootstrapConfiguration, CreateUserRequest,
    LoginRequest, SessionPrincipal,
};
use clouddesk_linux::{lookup_uid, AssignedRoot, LinuxIdentity};
use clouddesk_privilege::{
    GrantSigner, PowerOperation, PrivdRequest, PrivilegedAction, ServiceOperation, ServiceUnit,
    TerminalClientMessage, TerminalServerMessage, WorkerKind,
};
use clouddesk_remote::{
    host_key_fingerprint, validate_hostname, verify_host_key, NewRemoteServer, RemoteError,
    RemoteServerStore,
};
use clouddesk_transfers::{NewTransfer, TransferError, TransferQueue, TransferState};
use clouddesk_vault::{Vault, VaultError};
use clouddesk_vfs::LocalFileOperation;
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
};
use tower_http::{services::ServeDir, trace::TraceLayer};
use zeroize::Zeroizing;

const SESSION_COOKIE: &str = "clouddesk_session";
const MAX_TERMINAL_FRAME: usize = 1024 * 1024;

#[derive(Clone)]
struct AppState {
    version: &'static str,
    auth: Option<AuthService>,
    bootstrap_secret: PathBuf,
    privilege: Option<PrivilegeClient>,
    enforce_hsts: bool,
}

#[derive(Clone)]
pub struct PrivilegeClient {
    signer: GrantSigner,
    socket_path: PathBuf,
}

impl PrivilegeClient {
    pub fn new(key: &[u8], socket_path: PathBuf) -> Result<Self, clouddesk_privilege::GrantError> {
        Ok(Self {
            signer: GrantSigner::new(key)?,
            socket_path,
        })
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

pub fn router(static_dir: PathBuf) -> Router {
    build_router(
        static_dir,
        AppState {
            version: env!("CARGO_PKG_VERSION"),
            auth: None,
            bootstrap_secret: PathBuf::new(),
            privilege: None,
            enforce_hsts: false,
        },
    )
}

pub fn application_router(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
) -> Router {
    application_router_configured(static_dir, auth, bootstrap_secret, true)
}

pub fn application_router_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    enforce_hsts: bool,
) -> Router {
    build_router(
        static_dir,
        AppState {
            version: env!("CARGO_PKG_VERSION"),
            auth: Some(auth),
            bootstrap_secret,
            privilege: None,
            enforce_hsts,
        },
    )
}

pub fn application_router_with_privilege(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    privilege: PrivilegeClient,
) -> Router {
    application_router_with_privilege_configured(
        static_dir,
        auth,
        bootstrap_secret,
        privilege,
        true,
    )
}

pub fn application_router_with_privilege_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    privilege: PrivilegeClient,
    enforce_hsts: bool,
) -> Router {
    build_router(
        static_dir,
        AppState {
            version: env!("CARGO_PKG_VERSION"),
            auth: Some(auth),
            bootstrap_secret,
            privilege: Some(privilege),
            enforce_hsts,
        },
    )
}

#[allow(clippy::too_many_lines)]
fn build_router(static_dir: PathBuf, state: AppState) -> Router {
    let enforce_hsts = state.enforce_hsts;
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/health", get(health))
        .route("/api/v1/setup/status", get(setup_status))
        .route("/api/v1/setup/bootstrap", post(bootstrap))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
        .route("/api/v1/auth/step-up", post(step_up))
        .route("/api/v1/auth/totp/setup", post(totp_setup))
        .route("/api/v1/auth/totp/confirm", post(totp_confirm))
        .route("/api/v1/auth/sessions", get(list_sessions))
        .route("/api/v1/auth/sessions/{session_id}", delete(revoke_session))
        .route(
            "/api/v1/preferences",
            get(get_preferences).put(put_preferences),
        )
        .route("/api/v1/runtime-settings", get(get_runtime_settings))
        .route("/api/v1/users", post(create_user))
        .route("/api/v1/users/{user_id}/roles", post(assign_role))
        .route(
            "/api/v1/users/{user_id}/permissions/{capability}",
            put(set_user_permission),
        )
        .route("/api/v1/users/{user_id}/totp/reset", post(reset_totp))
        .route(
            "/api/v1/users/{user_id}/linux-identity",
            put(set_linux_identity),
        )
        .route(
            "/api/v1/users/{user_id}/assigned-roots",
            post(add_assigned_root),
        )
        .route("/api/v1/privilege/workers", post(spawn_user_worker))
        .route("/api/v1/files/local/actions", post(local_file_action))
        .route("/api/v1/files/local/download", get(download_local_file))
        .route("/api/v1/files/local/upload", post(upload_local_file))
        .route(
            "/api/v1/files/local/uploads",
            post(resumable_upload::create_upload_session),
        )
        .route(
            "/api/v1/files/local/uploads/{upload_id}",
            get(resumable_upload::upload_session_status)
                .put(resumable_upload::upload_chunk)
                .delete(resumable_upload::cancel_upload_session),
        )
        .route(
            "/api/v1/files/local/uploads/{upload_id}/complete",
            post(resumable_upload::finalize_upload_session),
        )
        .route("/api/v1/media/stream", get(stream_media))
        .route("/api/v1/media/preview", get(preview_media))
        .route(
            "/api/v1/vault/secrets",
            get(list_vault_secrets).post(create_vault_secret),
        )
        .route(
            "/api/v1/vault/secrets/{secret_id}",
            put(rotate_vault_secret).delete(delete_vault_secret),
        )
        .route(
            "/api/v1/vault/secrets/{secret_id}/reveal",
            post(reveal_vault_secret),
        )
        .route(
            "/api/v1/transfers",
            get(list_transfers).post(create_transfer),
        )
        .route("/api/v1/transfers/{transfer_id}", get(get_transfer))
        .route(
            "/api/v1/transfers/{transfer_id}/pause",
            post(pause_transfer),
        )
        .route(
            "/api/v1/transfers/{transfer_id}/resume",
            post(resume_transfer),
        )
        .route(
            "/api/v1/transfers/{transfer_id}/cancel",
            post(cancel_transfer),
        )
        .route("/api/v1/system/summary", get(system_summary))
        .route("/api/v1/system/services/control", post(service_control))
        .route("/api/v1/system/power", post(power_control))
        .route("/api/v1/terminal/ws", get(open_terminal_websocket))
        .route(
            "/api/v1/remote/servers",
            get(list_remote_servers).post(create_remote_server),
        )
        .route(
            "/api/v1/remote/servers/{server_id}",
            delete(delete_remote_server),
        )
        .route("/api/v1/remote/host-keys/scan", post(scan_remote_host_keys))
        .route(
            "/api/v1/remote/servers/{server_id}/verify-host-key",
            post(verify_remote_host_key),
        )
        .route("/api/v1/admin/ping", get(admin_ping))
        .fallback_service(ServeDir::new(static_dir).append_index_html_on_directories(true))
        .layer(middleware::from_fn_with_state(enforce_hsts, web_security))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn web_security(
    State(enforce_hsts): State<bool>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let websocket_upgrade = request
        .headers()
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let unsafe_method = websocket_upgrade
        || !matches!(
            *request.method(),
            axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
        );
    let cross_site = request
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| matches!(value, "cross-site" | "none"));
    let origin_mismatch = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| !origin_matches_host(origin, request.headers()));
    if unsafe_method && (cross_site || origin_mismatch) {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "cross-site request rejected",
            }),
        )
            .into_response();
    }

    let mut response = next.run(request).await;
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'self'; connect-src 'self' wss:; img-src 'self' data: blob:; media-src 'self' blob:; style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
        ),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
        ),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    if enforce_hsts {
        response.headers_mut().insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    response
}

fn origin_matches_host(origin: &str, headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    origin
        .parse::<Uri>()
        .ok()
        .and_then(|uri| uri.authority().map(ToString::to_string))
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok",
            service: "cloudeskd",
            version: state.version,
        }),
    )
}

#[derive(Serialize)]
struct SetupStatus {
    bootstrap_required: bool,
}

async fn setup_status(State(state): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    let auth = require_auth_service(&state)?;
    Ok(Json(SetupStatus {
        bootstrap_required: !auth.has_users().await?,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BootstrapRequest {
    secret: String,
    username: String,
    display_name: String,
    password: String,
    linux_username: Option<String>,
    #[serde(default = "desktop_mode")]
    ui_mode: String,
    #[serde(default)]
    enable_browser: bool,
    #[serde(default)]
    enable_code: bool,
    #[serde(default)]
    enable_office: bool,
}

fn desktop_mode() -> String {
    "desktop".to_owned()
}

async fn bootstrap(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<BootstrapRequest>,
) -> Result<Response, ApiError> {
    if !matches!(request.ui_mode.as_str(), "desktop" | "dashboard") {
        return Err(ApiError::bad_request(
            "ui_mode must be 'desktop' or 'dashboard'",
        ));
    }
    let linux_identity = if let Some(linux_username) = request
        .linux_username
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let identity = clouddesk_linux::lookup_user(linux_username)
            .map_err(|error| ApiError::internal(error.to_string()))?
            .ok_or_else(|| ApiError::bad_request("Linux account does not exist"))?;
        if identity.uid == 0 || identity.gid == 0 {
            return Err(ApiError::bad_request(
                "the initial administrator cannot map to root",
            ));
        }
        Some(identity)
    } else {
        None
    };
    verify_bootstrap_secret(&state.bootstrap_secret, &request.secret)?;
    let auth = require_auth_service(&state)?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    let user_id = auth
        .bootstrap_administrator_configured(
            &request.username,
            &request.display_name,
            &request.password,
            BootstrapConfiguration {
                ui_mode: &request.ui_mode,
                enable_browser: request.enable_browser,
                enable_code: request.enable_code,
                enable_office: request.enable_office,
                linux_identity: linux_identity.map(|identity| (identity.uid, identity.gid)),
            },
            &source_ip,
            &user_agent,
        )
        .await?;

    if let Err(error) = consume_bootstrap_secret(&state.bootstrap_secret) {
        // Database state is authoritative and already prevents a second bootstrap.
        // Do not turn a completed setup into a retry loop because cleanup failed.
        tracing::error!(%error, path = %state.bootstrap_secret.display(), "bootstrap secret cleanup failed");
    }
    Ok((StatusCode::CREATED, Json(json!({ "user_id": user_id }))).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBody {
    username: String,
    password: String,
    second_factor: Option<String>,
    #[serde(default)]
    remember_device: bool,
    device_label: Option<String>,
}

async fn login(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    let login = auth
        .login(LoginRequest {
            username: &body.username,
            password: &body.password,
            second_factor: body.second_factor.as_deref(),
            remember_device: body.remember_device,
            source_ip: &source_ip,
            user_agent: &user_agent,
            device_label: body.device_label.as_deref(),
        })
        .await?;

    let mut response = Json(json!({
        "user_id": login.user_id,
        "username": login.username,
        "expires_at": login.absolute_expires_at
    }))
    .into_response();
    let cookie = format!(
        "{SESSION_COOKIE}={}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
        login.token,
        (login.absolute_expires_at - unix_time()).max(0)
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|error| ApiError::invalid_header(&error))?,
    );
    Ok(response)
}

async fn logout(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let token = session_token(&headers)?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.revoke(token, &source_ip, &user_agent).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "clouddesk_session=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
        ),
    );
    Ok(response)
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionPrincipal>, ApiError> {
    Ok(Json(principal(&state, &headers).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepUpBody {
    password: String,
    second_factor: Option<String>,
}

async fn step_up(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<StepUpBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let token = session_token(&headers)?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    let expires_at = auth
        .step_up(
            token,
            &body.password,
            body.second_factor.as_deref(),
            &source_ip,
            &user_agent,
        )
        .await?;
    Ok(Json(json!({ "step_up_expires_at": expires_at })))
}

async fn totp_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    require_step_up(&principal)?;
    let secret = auth.begin_totp(&principal).await?;
    Ok(Json(json!({
        "secret": secret,
        "algorithm": "SHA1",
        "digits": 6,
        "period": 30
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TotpConfirmBody {
    code: String,
}

async fn totp_confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TotpConfirmBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    require_step_up(&principal)?;
    let recovery_codes = auth.confirm_totp(&principal, &body.code).await?;
    Ok(Json(json!({ "recovery_codes": recovery_codes })))
}

async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({ "sessions": auth.sessions(&principal).await? }),
    ))
}

async fn revoke_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.revoke_session(&principal, &session_id, &source_ip, &user_agent)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct PreferencesResponse {
    ui_mode: String,
    layout: serde_json::Value,
    favorites: serde_json::Value,
    recent: serde_json::Value,
}

async fn get_preferences(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PreferencesResponse>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let row = sqlx::query(
        "SELECT ui_mode, layout_json, favorites_json, recent_json
         FROM user_preferences WHERE user_id = ?",
    )
    .bind(&principal.user_id)
    .fetch_optional(auth.pool())
    .await
    .map_err(AuthError::from)?;
    if let Some(row) = row {
        return Ok(Json(PreferencesResponse {
            ui_mode: row.get("ui_mode"),
            layout: parse_preference_json(&row.get::<String, _>("layout_json"))?,
            favorites: parse_preference_json(&row.get::<String, _>("favorites_json"))?,
            recent: parse_preference_json(&row.get::<String, _>("recent_json"))?,
        }));
    }
    Ok(Json(PreferencesResponse {
        ui_mode: "desktop".to_owned(),
        layout: json!({}),
        favorites: json!([]),
        recent: json!([]),
    }))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreferencesBody {
    ui_mode: String,
    layout: serde_json::Value,
    favorites: serde_json::Value,
    recent: serde_json::Value,
}

async fn put_preferences(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PreferencesBody>,
) -> Result<StatusCode, ApiError> {
    if !matches!(body.ui_mode.as_str(), "desktop" | "dashboard")
        || !body.layout.is_object()
        || !body.favorites.is_array()
        || !body.recent.is_array()
    {
        return Err(ApiError::bad_request("invalid workspace preferences"));
    }
    let encoded =
        serde_json::to_vec(&body).map_err(|error| ApiError::internal(error.to_string()))?;
    if encoded.len() > 256 * 1024 {
        return Err(ApiError::bad_request("workspace preferences are too large"));
    }
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    sqlx::query(
        "INSERT INTO user_preferences (
            user_id, ui_mode, layout_json, favorites_json, recent_json, updated_at
         ) VALUES (?, ?, ?, ?, ?, unixepoch())
         ON CONFLICT(user_id) DO UPDATE SET
            ui_mode = excluded.ui_mode,
            layout_json = excluded.layout_json,
            favorites_json = excluded.favorites_json,
            recent_json = excluded.recent_json,
            updated_at = excluded.updated_at",
    )
    .bind(&principal.user_id)
    .bind(&body.ui_mode)
    .bind(body.layout.to_string())
    .bind(body.favorites.to_string())
    .bind(body.recent.to_string())
    .execute(auth.pool())
    .await
    .map_err(AuthError::from)?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.audit_action(
        &principal,
        "preferences.update",
        "user_preferences",
        Some(principal.user_id.clone()),
        "success",
        json!({ "ui_mode": body.ui_mode }),
        &source_ip,
        &user_agent,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn parse_preference_json(value: &str) -> Result<serde_json::Value, ApiError> {
    serde_json::from_str(value).map_err(|error| ApiError::internal(error.to_string()))
}

async fn get_runtime_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let _principal = principal(&state, &headers).await?;
    let rows = sqlx::query(
        "SELECT key, value_json FROM system_settings
         WHERE key IN (
            'runtime.browser.enabled', 'runtime.code.enabled', 'runtime.office.enabled'
         )",
    )
    .fetch_all(auth.pool())
    .await
    .map_err(AuthError::from)?;
    let mut flags = json!({ "browser": false, "code": false, "office": false });
    for row in rows {
        let key: String = row.get("key");
        let value: bool = serde_json::from_str(row.get::<String, _>("value_json").as_str())
            .map_err(|error| ApiError::internal(error.to_string()))?;
        if let Some(name) = key
            .strip_prefix("runtime.")
            .and_then(|key| key.strip_suffix(".enabled"))
        {
            flags[name] = json!(value);
        }
    }
    Ok(Json(flags))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateUserBody {
    username: String,
    display_name: String,
    password: String,
    role_ids: Vec<String>,
}

async fn create_user(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<CreateUserBody>,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    let role_ids: Vec<&str> = body.role_ids.iter().map(String::as_str).collect();
    let user_id = auth
        .create_user(
            &principal,
            CreateUserRequest {
                username: &body.username,
                display_name: &body.display_name,
                password: &body.password,
                role_ids: &role_ids,
            },
            &source_ip,
            &user_agent,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "user_id": user_id }))).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignRoleBody {
    role_id: String,
}

async fn assign_role(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<AssignRoleBody>,
) -> Result<StatusCode, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.assign_role(&principal, &user_id, &body.role_id, &source_ip, &user_agent)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionBody {
    allow: bool,
}

async fn set_user_permission(
    State(state): State<AppState>,
    Path((user_id, capability)): Path<(String, String)>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PermissionBody>,
) -> Result<StatusCode, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.set_user_permission(
        &principal,
        &user_id,
        &capability,
        body.allow,
        &source_ip,
        &user_agent,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinuxIdentityBody {
    uid: u32,
    gid: u32,
}

async fn set_linux_identity(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LinuxIdentityBody>,
) -> Result<StatusCode, ApiError> {
    let identity = lookup_uid(body.uid)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::bad_request("Linux UID does not exist"))?;
    if body.uid == 0 || body.gid == 0 || identity.gid != body.gid {
        return Err(ApiError::bad_request(
            "Linux UID/GID must identify a non-root account and its primary group",
        ));
    }
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.set_linux_identity(
        &principal,
        &user_id,
        body.uid,
        body.gid,
        &source_ip,
        &user_agent,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignedRootBody {
    path: PathBuf,
    access_mode: AssignedRootAccess,
}

async fn add_assigned_root(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<AssignedRootBody>,
) -> Result<Response, ApiError> {
    let assigned = AssignedRoot::new(&body.path, clouddesk_linux::AccessMode::Read)
        .map_err(|_| ApiError::bad_request("assigned root must be an existing absolute path"))?;
    let canonical = assigned.path().to_string_lossy();
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    let root_id = auth
        .add_assigned_root(
            &principal,
            &user_id,
            &canonical,
            body.access_mode,
            &source_ip,
            &user_agent,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "root_id": root_id }))).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRequest {
    worker: WorkerKind,
}

async fn spawn_user_worker(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<WorkerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let identity = mapped_identity(auth, &principal).await?;
    let action = PrivilegedAction::SpawnUserWorker {
        uid: identity.uid,
        gid: identity.gid,
        worker: body.worker,
    };
    dispatch_privileged_action(&state, &principal, action, connect, &headers).await
}

async fn local_file_action(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(operation): Json<LocalFileOperation>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let identity = mapped_identity(auth, &principal).await?;
    let writable = principal.can("files.local.write");
    let action = PrivilegedAction::LocalFileOperation {
        uid: identity.uid,
        gid: identity.gid,
        root: identity.home.to_string_lossy().into_owned(),
        writable,
        operation,
    };
    dispatch_privileged_action(&state, &principal, action, connect, &headers).await
}

#[derive(Deserialize)]
struct FilePathQuery {
    path: String,
}

async fn download_local_file(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<FilePathQuery>,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "files.local.read",
        false,
        connect,
        &headers,
    )
    .await?;
    let identity = mapped_identity(auth, &principal).await?;
    let path = resolve_safe_path(&identity.home, &query.path)?;
    serve_file_stream(&path, &headers, true).await
}

async fn stream_media(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<FilePathQuery>,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "files.local.read",
        false,
        connect,
        &headers,
    )
    .await?;
    let identity = mapped_identity(auth, &principal).await?;
    let path = resolve_safe_path(&identity.home, &query.path)?;
    serve_file_stream(&path, &headers, false).await
}

async fn preview_media(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<FilePathQuery>,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "files.local.read",
        false,
        connect,
        &headers,
    )
    .await?;
    let identity = mapped_identity(auth, &principal).await?;
    let path = resolve_safe_path(&identity.home, &query.path)?;
    serve_file_stream(&path, &headers, false).await
}

async fn upload_local_file(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<FilePathQuery>,
    mut body: Body,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "files.local.write",
        false,
        connect,
        &headers,
    )
    .await?;
    let identity = mapped_identity(auth, &principal).await?;
    let path = resolve_safe_path(&identity.home, &query.path)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
        }
    }
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut bytes_written = 0_u64;
    while let Some(chunk_result) = body.frame().await {
        let frame = chunk_result.map_err(|_| ApiError::bad_request("upload read error"))?;
        if let Some(data) = frame.data_ref() {
            file.write_all(data)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
            bytes_written += u64::try_from(data.len()).unwrap_or(0);
        }
    }
    file.flush()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.audit_action(
        &principal,
        "files.local.upload",
        "file",
        Some(query.path.clone()),
        "success",
        json!({ "path": query.path, "bytes": bytes_written }),
        &source_ip,
        &user_agent,
    )
    .await?;

    Ok(Json(
        json!({ "status": "uploaded", "path": query.path, "bytes": bytes_written }),
    ))
}

/// Resumable local-file uploads (`GOAL.md` G3: "large-file and resumable
/// upload support"). Design: chunks are always appended at the session's
/// current `bytes_received` offset — the client resumes an interrupted
/// upload by `GET`ting the session status and sending the remainder of the
/// source file starting at that byte. This keeps chunk handling strictly
/// sequential (no sparse/random-access bookkeeping) while still surviving a
/// dropped connection or browser reload, which is the scenario this
/// requirement exists for. Session state (owner, target path, temp path,
/// byte counts) is persisted in `SQLite` so it survives a `clouddeskd`
/// restart, not just a client reconnect.
pub(crate) mod resumable_upload {
    use super::{
        request_metadata, resolve_safe_path, ApiError, AppState, ConnectInfo, HeaderMap, StatusCode,
    };
    use axum::{body::Body, extract::Path, extract::State, Json};
    use clouddesk_auth::AuthService;
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use sqlx::Row;
    use std::net::SocketAddr;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};

    /// Sessions idle longer than this are treated as abandoned and are
    /// eligible for cleanup (their temp file is deleted and the row
    /// removed).
    const ABANDONED_SESSION_TTL_SECS: i64 = 24 * 60 * 60;

    fn now() -> i64 {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        )
        .unwrap_or(i64::MAX)
    }

    fn upload_dir(home: &std::path::Path) -> std::path::PathBuf {
        home.join(".clouddesk-uploads")
    }

    struct UploadSessionRow {
        virtual_path: String,
        temp_path: String,
        total_size: i64,
        bytes_received: i64,
        expected_sha256: Option<String>,
    }

    async fn load_session(
        auth: &AuthService,
        session_id: &str,
        principal_user_id: &str,
    ) -> Result<UploadSessionRow, ApiError> {
        let row = sqlx::query(
            "SELECT owner_user_id, virtual_path, temp_path, total_size, bytes_received, expected_sha256
             FROM upload_sessions WHERE id = ?",
        )
        .bind(session_id)
        .fetch_optional(auth.pool())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("upload session not found"))?;
        let owner_user_id: String = row.get("owner_user_id");
        // Authorization on every chunk/status/finalize/cancel request: the
        // session must belong to the caller. `files.local.write` is
        // already required by the caller before reaching here.
        if owner_user_id != principal_user_id {
            return Err(ApiError::forbidden());
        }
        Ok(UploadSessionRow {
            virtual_path: row.get("virtual_path"),
            temp_path: row.get("temp_path"),
            total_size: row.get("total_size"),
            bytes_received: row.get("bytes_received"),
            expected_sha256: row.get("expected_sha256"),
        })
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CreateUploadSessionBody {
        path: String,
        total_size: u64,
        #[serde(default)]
        sha256: Option<String>,
    }

    pub(crate) async fn create_upload_session(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
        Json(body): Json<CreateUploadSessionBody>,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        super::authorize_request(
            auth,
            &principal,
            "files.local.write",
            false,
            connect,
            &headers,
        )
        .await?;
        let identity = super::mapped_identity(auth, &principal).await?;

        // Reject destinations outside the caller's VFS root up front, the
        // same way the one-shot upload path does.
        let _ = resolve_safe_path(&identity.home, &body.path)?;

        let dir = upload_dir(&identity.home);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let session_id = clouddesk_auth::random_identifier(24);
        let temp_path = dir.join(format!("{session_id}.part"));
        tokio::fs::File::create(&temp_path)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;

        let timestamp = now();
        sqlx::query(
            "INSERT INTO upload_sessions (
                id, owner_user_id, virtual_path, temp_path, total_size,
                bytes_received, expected_sha256, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?)",
        )
        .bind(&session_id)
        .bind(&principal.user_id)
        .bind(&body.path)
        .bind(temp_path.to_string_lossy().into_owned())
        .bind(i64::try_from(body.total_size).unwrap_or(i64::MAX))
        .bind(&body.sha256)
        .bind(timestamp)
        .bind(timestamp)
        .execute(auth.pool())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

        Ok(Json(json!({
            "upload_id": session_id,
            "bytes_received": 0,
            "total_size": body.total_size,
        })))
    }

    pub(crate) async fn upload_session_status(
        State(state): State<AppState>,
        Path(session_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let session = load_session(auth, &session_id, &principal.user_id).await?;
        Ok(Json(json!({
            "upload_id": session_id,
            "bytes_received": session.bytes_received,
            "total_size": session.total_size,
        })))
    }

    pub(crate) async fn upload_chunk(
        State(state): State<AppState>,
        Path(session_id): Path<String>,
        headers: HeaderMap,
        mut body: Body,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let session = load_session(auth, &session_id, &principal.user_id).await?;

        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&session.temp_path)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        file.seek(std::io::SeekFrom::Start(
            u64::try_from(session.bytes_received).unwrap_or(0),
        ))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

        let mut received_now: i64 = 0;
        while let Some(chunk_result) = body.frame().await {
            let frame = chunk_result.map_err(|_| ApiError::bad_request("upload read error"))?;
            if let Some(data) = frame.data_ref() {
                let remaining = session.total_size - session.bytes_received - received_now;
                if i64::try_from(data.len()).unwrap_or(i64::MAX) > remaining.max(0) {
                    return Err(ApiError::bad_request(
                        "chunk exceeds the declared total upload size",
                    ));
                }
                file.write_all(data)
                    .await
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                received_now += i64::try_from(data.len()).unwrap_or(0);
            }
        }
        file.flush()
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;

        let new_total = session.bytes_received + received_now;
        sqlx::query("UPDATE upload_sessions SET bytes_received = ?, updated_at = ? WHERE id = ?")
            .bind(new_total)
            .bind(now())
            .bind(&session_id)
            .execute(auth.pool())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;

        Ok(Json(json!({
            "upload_id": session_id,
            "bytes_received": new_total,
            "total_size": session.total_size,
        })))
    }

    pub(crate) async fn finalize_upload_session(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        Path(session_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        super::authorize_request(
            auth,
            &principal,
            "files.local.write",
            false,
            connect,
            &headers,
        )
        .await?;
        let identity = super::mapped_identity(auth, &principal).await?;
        let session = load_session(auth, &session_id, &principal.user_id).await?;

        if session.bytes_received != session.total_size {
            return Err(ApiError::bad_request(
                "upload is incomplete: bytes received does not match the declared total size",
            ));
        }

        if let Some(expected) = &session.expected_sha256 {
            let mut file = tokio::fs::File::open(&session.temp_path)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0_u8; 256 * 1024];
            loop {
                use tokio::io::AsyncReadExt;
                let read = file
                    .read(&mut buffer)
                    .await
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            let actual = hex::encode(hasher.finalize());
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(ApiError::bad_request(
                    "checksum mismatch: uploaded content does not match the declared sha256",
                ));
            }
        }

        // Re-resolve the destination now (not just at session creation) so
        // a path that became invalid mid-upload (e.g. an assigned root
        // revoked) is still rejected before the file is placed.
        let destination = resolve_safe_path(&identity.home, &session.virtual_path)?;
        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| ApiError::internal(e.to_string()))?;
            }
        }
        tokio::fs::rename(&session.temp_path, &destination)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;

        sqlx::query("DELETE FROM upload_sessions WHERE id = ?")
            .bind(&session_id)
            .execute(auth.pool())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;

        let (source_ip, user_agent) = request_metadata(connect, &headers);
        auth.audit_action(
            &principal,
            "files.local.upload",
            "file",
            Some(session.virtual_path.clone()),
            "success",
            json!({ "path": session.virtual_path, "bytes": session.total_size, "resumable": true }),
            &source_ip,
            &user_agent,
        )
        .await?;

        Ok(Json(json!({
            "status": "uploaded",
            "path": session.virtual_path,
            "bytes": session.total_size,
        })))
    }

    pub(crate) async fn cancel_upload_session(
        State(state): State<AppState>,
        Path(session_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let session = load_session(auth, &session_id, &principal.user_id).await?;
        let _ = tokio::fs::remove_file(&session.temp_path).await;
        sqlx::query("DELETE FROM upload_sessions WHERE id = ?")
            .bind(&session_id)
            .execute(auth.pool())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    }

    /// Deletes upload sessions (and their temp files) that have not
    /// received a chunk in longer than [`ABANDONED_SESSION_TTL_SECS`].
    /// Called opportunistically on session creation and from a periodic
    /// background sweep (see [`spawn_janitor`]).
    pub(crate) async fn cleanup_abandoned_sessions(pool: &sqlx::SqlitePool) {
        let cutoff = now() - ABANDONED_SESSION_TTL_SECS;
        let Ok(rows) =
            sqlx::query("SELECT id, temp_path FROM upload_sessions WHERE updated_at < ?")
                .bind(cutoff)
                .fetch_all(pool)
                .await
        else {
            return;
        };
        for row in rows {
            let id: String = row.get("id");
            let temp_path: String = row.get("temp_path");
            let _ = tokio::fs::remove_file(&temp_path).await;
            let _ = sqlx::query("DELETE FROM upload_sessions WHERE id = ?")
                .bind(&id)
                .execute(pool)
                .await;
        }
    }

    /// Sweeps abandoned upload sessions every hour for the lifetime of the
    /// process. Mirrors `worker::TransferWorker::spawn`'s pattern.
    pub(crate) fn spawn_janitor(pool: sqlx::SqlitePool) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_hours(1));
            loop {
                interval.tick().await;
                cleanup_abandoned_sessions(&pool).await;
            }
        });
    }
}

fn resolve_safe_path(root: &std::path::Path, virtual_path: &str) -> Result<PathBuf, ApiError> {
    if virtual_path.as_bytes().contains(&0) {
        return Err(ApiError::bad_request("invalid path"));
    }
    let mut relative = PathBuf::new();
    for component in std::path::Path::new(virtual_path).components() {
        match component {
            std::path::Component::Normal(c) => relative.push(c),
            std::path::Component::CurDir | std::path::Component::RootDir => {}
            std::path::Component::ParentDir | std::path::Component::Prefix(_) => {
                return Err(ApiError::bad_request("path traversal denied"));
            }
        }
    }
    let combined = root.join(&relative);
    if combined.exists() {
        let canonical = combined
            .canonicalize()
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let canonical_root = root
            .canonicalize()
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !canonical.starts_with(&canonical_root) {
            return Err(ApiError::bad_request("path traversal denied"));
        }
        Ok(canonical)
    } else {
        let canonical_root = root
            .canonicalize()
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if let Some(parent) = combined.parent() {
            if parent.exists() {
                let canonical_parent = parent
                    .canonicalize()
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                if !canonical_parent.starts_with(&canonical_root) {
                    return Err(ApiError::bad_request("path traversal denied"));
                }
            }
        }
        Ok(combined)
    }
}

fn mime_for_path(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tiff" | "tif" => "image/tiff",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "txt" | "log" | "md" | "toml" | "yaml" | "yml" | "sh" | "rs" | "ts" | "js" | "css"
        | "html" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn serve_file_stream(
    path: &std::path::Path,
    headers: &HeaderMap,
    attachment: bool,
) -> Result<Response, ApiError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| ApiError::not_found("file not found"))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !metadata.is_file() {
        return Err(ApiError::bad_request("target is not a regular file"));
    }
    let total_len = metadata.len();
    let content_type = mime_for_path(&path.to_string_lossy());
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");

    if let Some(range_header) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        if let Some(range_spec) = range_header.strip_prefix("bytes=") {
            let mut parts = range_spec.split('-');
            let start = parts
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let end = parts
                .next()
                .and_then(|s| {
                    if s.is_empty() {
                        None
                    } else {
                        s.parse::<u64>().ok()
                    }
                })
                .unwrap_or(total_len.saturating_sub(1));

            if start <= end && start < total_len {
                let end = end.min(total_len.saturating_sub(1));
                let chunk_size = end - start + 1;
                file.seek(std::io::SeekFrom::Start(start))
                    .await
                    .map_err(|e| ApiError::internal(e.to_string()))?;

                let stream = tokio_util::io::ReaderStream::new(file.take(chunk_size));
                let body = Body::from_stream(stream);

                let mut response = (StatusCode::PARTIAL_CONTENT, body).into_response();
                let headers_mut = response.headers_mut();
                headers_mut.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
                headers_mut.insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes {start}-{end}/{total_len}"))
                        .map_err(|e| ApiError::internal(e.to_string()))?,
                );
                headers_mut.insert(
                    header::CONTENT_LENGTH,
                    HeaderValue::from_str(&chunk_size.to_string())
                        .map_err(|e| ApiError::internal(e.to_string()))?,
                );
                headers_mut.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_str(content_type)
                        .map_err(|e| ApiError::internal(e.to_string()))?,
                );
                return Ok(response);
            }
        }
    }

    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let mut response = (StatusCode::OK, body).into_response();
    let headers_mut = response.headers_mut();
    headers_mut.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers_mut.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&total_len.to_string())
            .map_err(|e| ApiError::internal(e.to_string()))?,
    );
    headers_mut.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).map_err(|e| ApiError::internal(e.to_string()))?,
    );
    if attachment {
        headers_mut.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
                .map_err(|e| ApiError::internal(e.to_string()))?,
        );
    }
    Ok(response)
}

async fn list_vault_secrets(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(auth, &principal, "secrets.manage", false, connect, &headers).await?;
    let vault = Vault::new(auth.pool().clone(), auth.secret_cipher());
    Ok(Json(
        json!({ "secrets": vault.list(&principal.user_id).await? }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateVaultSecretBody {
    kind: String,
    label: String,
    value: String,
}

async fn create_vault_secret(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<CreateVaultSecretBody>,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(auth, &principal, "secrets.manage", true, connect, &headers).await?;
    let value = Zeroizing::new(body.value);
    let vault = Vault::new(auth.pool().clone(), auth.secret_cipher());
    let id = vault
        .create(
            &principal.user_id,
            &body.kind,
            &body.label,
            value.as_bytes(),
        )
        .await?;
    audit_vault_action(
        auth,
        &principal,
        "vault.secret.create",
        &id,
        connect,
        &headers,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "secret_id": id }))).into_response())
}

async fn reveal_vault_secret(
    State(state): State<AppState>,
    Path(secret_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(auth, &principal, "secrets.manage", true, connect, &headers).await?;
    let vault = Vault::new(auth.pool().clone(), auth.secret_cipher());
    let value = vault.reveal(&principal.user_id, &secret_id).await?;
    let value = Zeroizing::new(
        String::from_utf8(value.to_vec())
            .map_err(|_| ApiError::bad_request("secret is not UTF-8 text"))?,
    );
    audit_vault_action(
        auth,
        &principal,
        "vault.secret.reveal",
        &secret_id,
        connect,
        &headers,
    )
    .await?;
    Ok(Json(json!({ "value": value.as_str() })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RotateVaultSecretBody {
    value: String,
}

async fn rotate_vault_secret(
    State(state): State<AppState>,
    Path(secret_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<RotateVaultSecretBody>,
) -> Result<StatusCode, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(auth, &principal, "secrets.manage", true, connect, &headers).await?;
    let value = Zeroizing::new(body.value);
    let vault = Vault::new(auth.pool().clone(), auth.secret_cipher());
    vault
        .rotate(&principal.user_id, &secret_id, value.as_bytes())
        .await?;
    audit_vault_action(
        auth,
        &principal,
        "vault.secret.rotate",
        &secret_id,
        connect,
        &headers,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_vault_secret(
    State(state): State<AppState>,
    Path(secret_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(auth, &principal, "secrets.manage", true, connect, &headers).await?;
    let vault = Vault::new(auth.pool().clone(), auth.secret_cipher());
    vault.delete(&principal.user_id, &secret_id).await?;
    audit_vault_action(
        auth,
        &principal,
        "vault.secret.delete",
        &secret_id,
        connect,
        &headers,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_transfers(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "transfers.create",
        false,
        connect,
        &headers,
    )
    .await?;
    let queue = TransferQueue::new(auth.pool().clone());
    Ok(Json(
        json!({ "transfers": queue.list_owner(&principal.user_id, 200).await? }),
    ))
}

async fn get_transfer(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "transfers.create",
        false,
        connect,
        &headers,
    )
    .await?;
    let queue = TransferQueue::new(auth.pool().clone());
    Ok(Json(json!({
        "transfer": queue.get_owned(&transfer_id, &principal.user_id).await?
    })))
}

async fn create_transfer(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(transfer): Json<NewTransfer>,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "transfers.create",
        false,
        connect,
        &headers,
    )
    .await?;
    let queue = TransferQueue::new(auth.pool().clone());
    let transfer_id = queue.enqueue(&principal.user_id, &transfer).await?;
    audit_transfer_action(
        auth,
        &principal,
        "transfer.create",
        &transfer_id,
        json!({ "source": transfer.source, "destination": transfer.destination }),
        connect,
        &headers,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "transfer_id": transfer_id })),
    )
        .into_response())
}

async fn pause_transfer(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    control_transfer(
        &state,
        &transfer_id,
        TransferState::Paused,
        "transfers.cancel",
        "transfer.pause",
        connect,
        &headers,
    )
    .await
}

async fn resume_transfer(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    control_transfer(
        &state,
        &transfer_id,
        TransferState::Queued,
        "transfers.create",
        "transfer.resume",
        connect,
        &headers,
    )
    .await
}

async fn cancel_transfer(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    control_transfer(
        &state,
        &transfer_id,
        TransferState::Cancelled,
        "transfers.cancel",
        "transfer.cancel",
        connect,
        &headers,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn control_transfer(
    state: &AppState,
    transfer_id: &str,
    target: TransferState,
    capability: &str,
    action: &str,
    connect: SocketAddr,
    headers: &HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth = require_auth_service(state)?;
    let principal = principal(state, headers).await?;
    authorize_request(auth, &principal, capability, false, connect, headers).await?;
    let queue = TransferQueue::new(auth.pool().clone());
    queue.get_owned(transfer_id, &principal.user_id).await?;
    queue.set_state(transfer_id, target).await?;
    audit_transfer_action(
        auth,
        &principal,
        action,
        transfer_id,
        json!({ "state": target }),
        connect,
        headers,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn audit_transfer_action(
    auth: &AuthService,
    principal: &SessionPrincipal,
    action: &str,
    transfer_id: &str,
    metadata: serde_json::Value,
    connect: SocketAddr,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let (source_ip, user_agent) = request_metadata(connect, headers);
    auth.audit_action(
        principal,
        action,
        "transfer",
        Some(transfer_id.to_owned()),
        "success",
        metadata,
        &source_ip,
        &user_agent,
    )
    .await?;
    Ok(())
}

async fn system_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    if !principal.can("system.services.manage") {
        return Err(ApiError::forbidden());
    }
    let hostname = std::fs::read_to_string("/etc/hostname")
        .unwrap_or_else(|_| "unknown".to_owned())
        .trim()
        .to_owned();
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_else(|_| "unknown".to_owned())
        .trim()
        .to_owned();
    let uptime_seconds = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|value| value.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or_default();
    let load_average = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|value| {
            value
                .split_whitespace()
                .take(3)
                .filter_map(|part| part.parse::<f64>().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let memory = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let memory_total_kib = proc_memory_value(&memory, "MemTotal:");
    let memory_available_kib = proc_memory_value(&memory, "MemAvailable:");
    Ok(Json(json!({
        "hostname": hostname,
        "kernel": kernel,
        "uptime_seconds": uptime_seconds,
        "load_average": load_average,
        "memory_total_kib": memory_total_kib,
        "memory_available_kib": memory_available_kib,
        "container_engines": {
            "docker": std::path::Path::new("/var/run/docker.sock").exists(),
            "podman": std::path::Path::new("/run/podman/podman.sock").exists()
        }
    })))
}

fn proc_memory_value(contents: &str, key: &str) -> Option<u64> {
    contents.lines().find_map(|line| {
        let value = line.strip_prefix(key)?;
        value.split_whitespace().next()?.parse().ok()
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceControlBody {
    unit: String,
    operation: ServiceOperation,
}

async fn service_control(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<ServiceControlBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    let unit =
        ServiceUnit::new(&body.unit).map_err(|_| ApiError::bad_request("invalid service unit"))?;
    dispatch_privileged_action(
        &state,
        &principal,
        PrivilegedAction::ServiceControl {
            unit,
            operation: body.operation,
        },
        connect,
        &headers,
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PowerControlBody {
    operation: PowerOperation,
}

async fn power_control(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PowerControlBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    dispatch_privileged_action(
        &state,
        &principal,
        PrivilegedAction::Power {
            operation: body.operation,
        },
        connect,
        &headers,
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostKeyScanBody {
    hostname: String,
    port: u16,
}

#[derive(Clone, Serialize)]
struct ScannedHostKey {
    key_type: String,
    key_base64: String,
    fingerprint: String,
}

async fn list_remote_servers(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "remote.servers.read",
        false,
        connect,
        &headers,
    )
    .await?;
    let store = RemoteServerStore::new(auth.pool().clone());
    Ok(Json(
        json!({ "servers": store.list(&principal.user_id).await? }),
    ))
}

async fn create_remote_server(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(server): Json<NewRemoteServer>,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "remote.servers.manage",
        true,
        connect,
        &headers,
    )
    .await?;
    let store = RemoteServerStore::new(auth.pool().clone());
    let server_id = store.create(&principal.user_id, &server).await?;
    audit_remote_action(
        auth,
        &principal,
        "remote.server.create",
        &server_id,
        "success",
        json!({ "hostname": server.hostname, "port": server.port }),
        connect,
        &headers,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "server_id": server_id }))).into_response())
}

async fn delete_remote_server(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "remote.servers.manage",
        true,
        connect,
        &headers,
    )
    .await?;
    RemoteServerStore::new(auth.pool().clone())
        .delete(&principal.user_id, &server_id)
        .await?;
    audit_remote_action(
        auth,
        &principal,
        "remote.server.delete",
        &server_id,
        "success",
        json!({}),
        connect,
        &headers,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn scan_remote_host_keys(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(target): Json<HostKeyScanBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "remote.servers.manage",
        true,
        connect,
        &headers,
    )
    .await?;
    let keys = scan_host_keys(&target.hostname, target.port).await?;
    audit_remote_action(
        auth,
        &principal,
        "remote.host_key.scan",
        &target.hostname,
        "success",
        json!({ "port": target.port, "fingerprints": keys.iter().map(|key| &key.fingerprint).collect::<Vec<_>>() }),
        connect,
        &headers,
    )
    .await?;
    Ok(Json(json!({
        "keys": keys,
        "untrusted": true,
        "warning": "Verify a fingerprint through an independent trusted channel before saving it."
    })))
}

async fn verify_remote_host_key(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "remote.servers.read",
        false,
        connect,
        &headers,
    )
    .await?;
    let store = RemoteServerStore::new(auth.pool().clone());
    let server = store.get(&principal.user_id, &server_id).await?;
    let pinned = store
        .pinned_host_key(&principal.user_id, &server_id)
        .await?;
    let scanned = scan_host_keys(&server.hostname, server.port).await?;
    let verified = scanned.iter().any(|key| {
        key.key_type == pinned.key_type
            && verify_host_key(&pinned.key_base64, &key.key_base64).is_ok()
    });
    audit_remote_action(
        auth,
        &principal,
        "remote.host_key.verify",
        &server_id,
        if verified { "success" } else { "denied" },
        json!({ "hostname": server.hostname, "port": server.port }),
        connect,
        &headers,
    )
    .await?;
    if !verified {
        return Err(RemoteError::HostKeyChanged.into());
    }
    Ok(Json(
        json!({ "verified": true, "fingerprint": server.host_key_fingerprint }),
    ))
}

async fn scan_host_keys(hostname: &str, port: u16) -> Result<Vec<ScannedHostKey>, ApiError> {
    validate_hostname(hostname)?;
    let executable = ["/usr/bin/ssh-keyscan", "/bin/ssh-keyscan"]
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
        .ok_or_else(|| ApiError::internal("ssh-keyscan is not installed"))?;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        tokio::process::Command::new(executable)
            .args(["-T", "5", "-p", &port.to_string(), hostname])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output(),
    )
    .await
    .map_err(|_| ApiError::bad_gateway("SSH host-key scan timed out"))?
    .map_err(|error| ApiError::internal(error.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut keys = Vec::new();
    for line in stdout.lines().filter(|line| !line.starts_with('#')) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3
            || !matches!(
                fields[1],
                "ssh-ed25519"
                    | "ssh-rsa"
                    | "ecdsa-sha2-nistp256"
                    | "ecdsa-sha2-nistp384"
                    | "ecdsa-sha2-nistp521"
            )
        {
            continue;
        }
        let Ok(fingerprint) = host_key_fingerprint(fields[2]) else {
            continue;
        };
        let key = ScannedHostKey {
            key_type: fields[1].to_owned(),
            key_base64: fields[2].to_owned(),
            fingerprint,
        };
        if !keys
            .iter()
            .any(|existing: &ScannedHostKey| existing.key_type == key.key_type)
        {
            keys.push(key);
        }
    }
    if keys.is_empty() {
        return Err(ApiError::bad_gateway(
            "remote host returned no supported SSH host key",
        ));
    }
    Ok(keys)
}

#[allow(clippy::too_many_arguments)]
async fn audit_remote_action(
    auth: &AuthService,
    principal: &SessionPrincipal,
    action: &str,
    server_id: &str,
    result: &str,
    metadata: serde_json::Value,
    connect: SocketAddr,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let (source_ip, user_agent) = request_metadata(connect, headers);
    auth.audit_action(
        principal,
        action,
        "remote_server",
        Some(server_id.to_owned()),
        result,
        metadata,
        &source_ip,
        &user_agent,
    )
    .await?;
    Ok(())
}

async fn open_terminal_websocket(
    websocket: WebSocketUpgrade,
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "terminal.local.open",
        false,
        connect,
        &headers,
    )
    .await?;
    let identity = mapped_identity(auth, &principal).await?;
    let privilege = state
        .privilege
        .as_ref()
        .ok_or_else(ApiError::privilege_unavailable)?;
    let action = PrivilegedAction::OpenTerminalSession {
        uid: identity.uid,
        gid: identity.gid,
        rows: 24,
        cols: 80,
        shell: None,
    };
    let action_metadata =
        serde_json::to_value(&action).map_err(|error| ApiError::internal(error.to_string()))?;
    let grant = privilege
        .signer
        .issue(
            &principal.user_id,
            &principal.session_id_hash,
            action,
            unix_time(),
            30,
        )
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let terminal_id = grant.claims.nonce.clone();
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.audit_action(
        &principal,
        "privilege.grant.issue",
        "terminal_session",
        Some(terminal_id.clone()),
        "success",
        action_metadata.clone(),
        &source_ip,
        &user_agent,
    )
    .await?;
    let response = cloudesk_privd::request(&privilege.socket_path, &PrivdRequest { grant }).await;
    let result = match &response {
        Ok(response) if response.accepted => "success",
        Ok(_) => "denied",
        Err(_) => "failure",
    };
    auth.audit_action(
        &principal,
        "privilege.action.complete",
        "terminal_session",
        Some(terminal_id.clone()),
        result,
        action_metadata,
        &source_ip,
        &user_agent,
    )
    .await?;
    let response = response.map_err(|error| ApiError::internal(error.to_string()))?;
    if !response.accepted {
        return Err(ApiError::privilege_rejected());
    }
    let socket_path = response
        .output
        .and_then(|output| {
            output
                .get("socket_path")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .ok_or_else(|| ApiError::internal("terminal helper returned no socket path"))?;
    let auth = auth.clone();
    Ok(websocket
        .on_upgrade(move |socket| {
            bridge_terminal_websocket(
                socket,
                PathBuf::from(socket_path),
                auth,
                principal,
                terminal_id,
                source_ip,
                user_agent,
            )
        })
        .into_response())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn bridge_terminal_websocket(
    mut websocket: WebSocket,
    socket_path: PathBuf,
    auth: AuthService,
    principal: SessionPrincipal,
    terminal_id: String,
    source_ip: String,
    user_agent: String,
) {
    let stream = match UnixStream::connect(&socket_path).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::warn!(%error, path = %socket_path.display(), "terminal worker connection failed");
            let _ = auth
                .audit_action(
                    &principal,
                    "terminal.session.start",
                    "terminal_session",
                    Some(terminal_id),
                    "failure",
                    json!({}),
                    &source_ip,
                    &user_agent,
                )
                .await;
            return;
        }
    };
    if let Err(error) = auth
        .audit_action(
            &principal,
            "terminal.session.start",
            "terminal_session",
            Some(terminal_id.clone()),
            "success",
            json!({}),
            &source_ip,
            &user_agent,
        )
        .await
    {
        tracing::error!(%error, "could not audit terminal session start");
        return;
    }
    let (mut terminal_reader, mut terminal_writer) = stream.into_split();
    let mut result = "success";
    loop {
        tokio::select! {
            client = websocket.recv() => {
                let message = match client {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => {
                        tracing::debug!(%error, "terminal WebSocket receive failed");
                        result = "failure";
                        break;
                    }
                    None => break,
                };
                let command = match message {
                    Message::Binary(data) => TerminalClientMessage::Data { data: data.to_vec() },
                    Message::Text(text) => match serde_json::from_str(text.as_str()) {
                        Ok(command) => command,
                        Err(_) => continue,
                    },
                    Message::Close(_) => TerminalClientMessage::Close,
                    Message::Ping(data) => {
                        if websocket.send(Message::Pong(data)).await.is_err() {
                            result = "failure";
                            break;
                        }
                        continue;
                    }
                    Message::Pong(_) => continue,
                };
                let close = matches!(command, TerminalClientMessage::Close);
                if write_terminal_frame(&mut terminal_writer, &command).await.is_err() {
                    result = "failure";
                    break;
                }
                if close { break; }
            }
            server = read_terminal_frame::<_, TerminalServerMessage>(&mut terminal_reader) => {
                match server {
                    Ok(TerminalServerMessage::Output { data }) => {
                        if websocket.send(Message::Binary(data.into())).await.is_err() { break; }
                    }
                    Ok(message @ (TerminalServerMessage::Exit { .. }
                    | TerminalServerMessage::Error { .. })) => {
                        let text = serde_json::to_string(&message).unwrap_or_else(|_| "{\"type\":\"error\"}".to_owned());
                        let _ = websocket.send(Message::Text(text.into())).await;
                        if matches!(message, TerminalServerMessage::Error { .. }) { result = "failure"; }
                        break;
                    }
                    Err(error) => {
                        tracing::debug!(%error, "terminal worker stream ended");
                        break;
                    }
                }
            }
        }
    }
    if let Err(error) = auth
        .audit_action(
            &principal,
            "terminal.session.stop",
            "terminal_session",
            Some(terminal_id),
            result,
            json!({}),
            &source_ip,
            &user_agent,
        )
        .await
    {
        tracing::error!(%error, "could not audit terminal session stop");
    }
}

async fn read_terminal_frame<R, T>(reader: &mut R) -> anyhow::Result<T>
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let length = reader.read_u32().await? as usize;
    if length == 0 || length > MAX_TERMINAL_FRAME {
        anyhow::bail!("terminal frame is invalid");
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn write_terminal_frame<W, T>(writer: &mut W, value: &T) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_TERMINAL_FRAME {
        anyhow::bail!("terminal frame is too large");
    }
    writer.write_u32(u32::try_from(bytes.len())?).await?;
    writer.write_all(&bytes).await?;
    Ok(())
}

async fn authorize_request(
    auth: &AuthService,
    principal: &SessionPrincipal,
    capability: &str,
    step_up: bool,
    connect: SocketAddr,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let allowed = principal.can(capability);
    let (source_ip, user_agent) = request_metadata(connect, headers);
    auth.audit_authorization(
        Some(principal),
        capability,
        allowed,
        &source_ip,
        &user_agent,
    )
    .await?;
    if !allowed {
        return Err(ApiError::forbidden());
    }
    if step_up {
        require_step_up(principal)?;
    }
    Ok(())
}

async fn audit_vault_action(
    auth: &AuthService,
    principal: &SessionPrincipal,
    action: &str,
    secret_id: &str,
    connect: SocketAddr,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let (source_ip, user_agent) = request_metadata(connect, headers);
    auth.audit_action(
        principal,
        action,
        "vault_secret",
        Some(secret_id.to_owned()),
        "success",
        json!({}),
        &source_ip,
        &user_agent,
    )
    .await?;
    Ok(())
}

async fn mapped_identity(
    auth: &AuthService,
    principal: &SessionPrincipal,
) -> Result<LinuxIdentity, ApiError> {
    let mapping = auth.linux_identity(principal).await?;
    let identity = lookup_uid(mapping.uid)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::bad_request("mapped Linux UID no longer exists"))?;
    if identity.gid != mapping.gid || mapping.uid == 0 || mapping.gid == 0 {
        return Err(ApiError::bad_request(
            "mapped Linux identity is no longer valid",
        ));
    }
    Ok(identity)
}

async fn dispatch_privileged_action(
    state: &AppState,
    principal: &SessionPrincipal,
    action: PrivilegedAction,
    connect: SocketAddr,
    headers: &HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(state)?;
    let capability = action.required_capability();
    let allowed = principal.can(capability);
    let (source_ip, user_agent) = request_metadata(connect, headers);
    auth.audit_authorization(
        Some(principal),
        capability,
        allowed,
        &source_ip,
        &user_agent,
    )
    .await?;
    if !allowed {
        return Err(ApiError::forbidden());
    }
    if action.requires_step_up() {
        require_step_up(principal)?;
    }

    let privilege = state
        .privilege
        .as_ref()
        .ok_or_else(ApiError::privilege_unavailable)?;
    let action_metadata =
        serde_json::to_value(&action).map_err(|error| ApiError::internal(error.to_string()))?;
    let grant = privilege
        .signer
        .issue(
            &principal.user_id,
            &principal.session_id_hash,
            action,
            unix_time(),
            30,
        )
        .map_err(|error| ApiError::internal(error.to_string()))?;
    auth.audit_action(
        principal,
        "privilege.grant.issue",
        "privileged_action",
        Some(grant.claims.nonce.clone()),
        "success",
        action_metadata.clone(),
        &source_ip,
        &user_agent,
    )
    .await?;

    let response = cloudesk_privd::request(&privilege.socket_path, &PrivdRequest { grant }).await;
    let (result, accepted) = match &response {
        Ok(response) if response.accepted => ("success", true),
        Ok(_) => ("denied", false),
        Err(_) => ("failure", false),
    };
    auth.audit_action(
        principal,
        "privilege.action.complete",
        "privileged_action",
        None,
        result,
        action_metadata,
        &source_ip,
        &user_agent,
    )
    .await?;
    let response = response.map_err(|error| ApiError::internal(error.to_string()))?;
    if !accepted {
        return Err(ApiError::privilege_rejected());
    }
    Ok(Json(
        json!({ "message": response.message, "output": response.output }),
    ))
}

async fn reset_totp(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.reset_totp(&principal, &user_id, &source_ip, &user_agent)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_ping(
    State(state): State<AppState>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let allowed = principal.can("users.manage");
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.audit_authorization(
        Some(&principal),
        "users.manage",
        allowed,
        &source_ip,
        &user_agent,
    )
    .await?;
    if !allowed {
        return Err(ApiError::forbidden());
    }
    Ok(Json(json!({ "status": "authorized" })))
}

async fn principal(state: &AppState, headers: &HeaderMap) -> Result<SessionPrincipal, ApiError> {
    require_auth_service(state)?
        .authenticate(session_token(headers)?)
        .await
        .map_err(ApiError::from)
}

fn require_auth_service(state: &AppState) -> Result<&AuthService, ApiError> {
    state.auth.as_ref().ok_or_else(ApiError::not_initialized)
}

fn require_step_up(principal: &SessionPrincipal) -> Result<(), ApiError> {
    if principal.has_fresh_step_up() {
        Ok(())
    } else {
        Err(ApiError::step_up_required())
    }
}

fn session_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    let cookies = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(ApiError::unauthorized)?;
    cookies
        .split(';')
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(name, value)| (name == SESSION_COOKIE).then_some(value))
        .filter(|value| !value.is_empty())
        .ok_or_else(ApiError::unauthorized)
}

fn request_metadata(connect: SocketAddr, headers: &HeaderMap) -> (String, String) {
    let source_ip = connect.ip().to_string();
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .chars()
        .take(512)
        .collect();
    (source_ip, user_agent)
}

fn verify_bootstrap_secret(path: &PathBuf, supplied: &str) -> Result<(), ApiError> {
    let expected =
        std::fs::read_to_string(path).map_err(|error| ApiError::bootstrap_unavailable(&error))?;
    let expected_hash = Sha256::digest(expected.trim().as_bytes());
    let supplied_hash = Sha256::digest(supplied.trim().as_bytes());
    if bool::from(expected_hash.ct_eq(&supplied_hash)) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn consume_bootstrap_secret(path: &std::path::Path) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let length = usize::try_from(file.metadata()?.len()).unwrap_or_default();
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&vec![0_u8; length])?;
    file.sync_all()?;
    drop(file);
    std::fs::remove_file(path)
}

pub async fn serve(listener: tokio::net::TcpListener, app: Router) -> Result<(), std::io::Error> {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install terminate handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

struct ApiError {
    status: StatusCode,
    public_message: &'static str,
    internal: Option<String>,
}

impl ApiError {
    fn bad_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            public_message: message,
            internal: None,
        }
    }

    fn not_found(message: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            public_message: message,
            internal: None,
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            public_message: "authentication required",
            internal: None,
        }
    }

    fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            public_message: "permission denied",
            internal: None,
        }
    }

    fn step_up_required() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            public_message: "recent step-up authentication required",
            internal: None,
        }
    }

    fn not_initialized() -> Self {
        Self::internal("authentication service is not initialized")
    }

    fn privilege_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            public_message: "privileged helper is not enabled",
            internal: None,
        }
    }

    fn privilege_rejected() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            public_message: "privileged helper rejected the action",
            internal: None,
        }
    }

    fn bad_gateway(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            public_message: message,
            internal: None,
        }
    }

    fn bootstrap_unavailable(error: &std::io::Error) -> Self {
        Self::internal(format!("bootstrap secret is unavailable: {error}"))
    }

    fn invalid_header(error: &axum::http::header::InvalidHeaderValue) -> Self {
        Self::internal(format!("could not create session cookie: {error}"))
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            public_message: "internal server error",
            internal: Some(message.into()),
        }
    }
}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::InvalidCredentials | AuthError::InvalidSecondFactor => Self {
                status: StatusCode::UNAUTHORIZED,
                public_message: "invalid credentials",
                internal: None,
            },
            AuthError::InvalidSession => Self::unauthorized(),
            AuthError::RateLimited => Self {
                status: StatusCode::TOO_MANY_REQUESTS,
                public_message: "too many authentication attempts; try again later",
                internal: None,
            },
            AuthError::InvalidUsername | AuthError::InvalidPassword => {
                Self::bad_request("invalid account details")
            }
            AuthError::BootstrapComplete => Self {
                status: StatusCode::CONFLICT,
                public_message: "initial setup is already complete",
                internal: None,
            },
            AuthError::TotpNotConfigured => Self::bad_request("TOTP is not configured"),
            AuthError::PermissionDenied => Self::forbidden(),
            AuthError::StepUpRequired => Self::step_up_required(),
            AuthError::UnknownRole | AuthError::UnknownCapability => {
                Self::bad_request("unknown role or capability")
            }
            AuthError::UnknownUser => Self::bad_request("unknown user"),
            AuthError::InvalidLinuxIdentity => Self::bad_request("invalid Linux identity"),
            AuthError::LinuxIdentityNotMapped => {
                Self::bad_request("user has no mapped Linux identity")
            }
            other => Self::internal(other.to_string()),
        }
    }
}

impl From<VaultError> for ApiError {
    fn from(error: VaultError) -> Self {
        match error {
            VaultError::NotFound => Self {
                status: StatusCode::NOT_FOUND,
                public_message: "secret not found",
                internal: None,
            },
            VaultError::InvalidKind | VaultError::InvalidLabel | VaultError::InvalidValue => {
                Self::bad_request("invalid secret")
            }
            other => Self::internal(other.to_string()),
        }
    }
}

impl From<TransferError> for ApiError {
    fn from(error: TransferError) -> Self {
        match error {
            TransferError::NotFound => Self {
                status: StatusCode::NOT_FOUND,
                public_message: "transfer not found",
                internal: None,
            },
            TransferError::InvalidEndpoint | TransferError::TooLarge => {
                Self::bad_request("invalid transfer")
            }
            TransferError::InvalidTransition => Self {
                status: StatusCode::CONFLICT,
                public_message: "invalid transfer state transition",
                internal: None,
            },
            other => Self::internal(other.to_string()),
        }
    }
}

impl From<RemoteError> for ApiError {
    fn from(error: RemoteError) -> Self {
        match error {
            RemoteError::NotFound => Self {
                status: StatusCode::NOT_FOUND,
                public_message: "remote server not found",
                internal: None,
            },
            RemoteError::HostKeyChanged => Self {
                status: StatusCode::CONFLICT,
                public_message: "SSH host key changed; connection refused",
                internal: None,
            },
            RemoteError::InvalidName
            | RemoteError::InvalidHostname
            | RemoteError::InvalidUsername
            | RemoteError::InvalidHostKey
            | RemoteError::InvalidCredentialReference
            | RemoteError::InvalidProxyJump
            | RemoteError::InvalidTags => Self::bad_request("invalid remote server configuration"),
            other => Self::internal(other.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let Some(internal) = self.internal {
            tracing::error!(%internal, "API request failed");
        }
        (
            self.status,
            Json(ErrorResponse {
                error: self.public_message,
            }),
        )
            .into_response()
    }
}

fn unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

/// Starts the background sweep that deletes abandoned resumable-upload
/// sessions (see [`resumable_upload`]).
pub fn spawn_upload_session_janitor(pool: sqlx::SqlitePool) {
    resumable_upload::spawn_janitor(pool);
}

pub mod worker;

pub mod security {
    use thiserror::Error;

    #[derive(Debug, Error, Eq, PartialEq)]
    #[error("cloudeskd refuses to run as root; use the dedicated clouddesk service account")]
    pub struct RootProcessError;

    pub fn require_unprivileged(effective_uid: u32) -> Result<(), RootProcessError> {
        if effective_uid == 0 {
            Err(RootProcessError)
        } else {
            Ok(())
        }
    }
}
