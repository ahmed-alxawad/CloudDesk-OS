use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post, put},
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
    media: Option<clouddesk_media::MediaService>,
    library: Option<clouddesk_library::LibraryStore>,
    runtime: Option<Arc<clouddesk_orchestrator::RuntimeManager>>,
    /// Always `false` in every production router constructor. Only the
    /// test-only constructors used by `services/clouddeskd`'s own
    /// integration tests set this `true`, so the disposable
    /// `RuntimeKind::TestFixture` (Task 15/31) can never be reached
    /// through the real product HTTP API merely because the fixture
    /// binary happens to exist on disk.
    runtime_allow_test_kind: bool,
    /// The trusted, server-computed base URL Collabora is configured to
    /// trust as its WOPI host (Task 4/5/61) -- e.g.
    /// `http://host.docker.internal:PORT`. `None` means Office WOPI
    /// session creation is unavailable (distinct from the Office
    /// *runtime* being unavailable/disabled, which `RuntimeManager`
    /// already reports generically).
    office_wopi_host_base: Option<String>,
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
            media: None,
            library: None,
            runtime: None,
            runtime_allow_test_kind: false,
            office_wopi_host_base: None,
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
    application_router_and_media_configured(static_dir, auth, bootstrap_secret, enforce_hsts, None)
}

/// Same as [`application_router_configured`], additionally wiring in the
/// shared media (`FFmpeg`) service, for the privileged-helper-disabled
/// deployment path (media support is independent of the privilege
/// helper).
pub fn application_router_and_media_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    enforce_hsts: bool,
    media: Option<clouddesk_media::MediaService>,
) -> Router {
    application_router_and_media_and_library_configured(
        static_dir,
        auth,
        bootstrap_secret,
        enforce_hsts,
        media,
        None,
    )
}

/// Same as [`application_router_and_media_configured`], additionally
/// wiring in the Music library store. See that function's doc comment
/// for why this is a sibling function rather than a signature change.
pub fn application_router_and_media_and_library_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    enforce_hsts: bool,
    media: Option<clouddesk_media::MediaService>,
    library: Option<clouddesk_library::LibraryStore>,
) -> Router {
    application_router_and_media_and_library_and_runtime_configured(
        static_dir,
        auth,
        bootstrap_secret,
        enforce_hsts,
        media,
        library,
        None,
    )
}

/// Same as [`application_router_and_media_and_library_configured`],
/// additionally wiring in the Phase 6 optional-runtime orchestrator
/// (Code/Office/Browser). `RuntimeKind::TestFixture` is never reachable
/// through this constructor's router (Task 15) -- only
/// [`application_router_and_media_and_library_and_runtime_configured_for_tests`]
/// enables it, and that constructor is not used by `main.rs`.
pub fn application_router_and_media_and_library_and_runtime_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    enforce_hsts: bool,
    media: Option<clouddesk_media::MediaService>,
    library: Option<clouddesk_library::LibraryStore>,
    runtime: Option<Arc<clouddesk_orchestrator::RuntimeManager>>,
) -> Router {
    build_router(
        static_dir,
        AppState {
            version: env!("CARGO_PKG_VERSION"),
            auth: Some(auth),
            bootstrap_secret,
            privilege: None,
            enforce_hsts,
            media,
            library,
            runtime,
            runtime_allow_test_kind: false,
            office_wopi_host_base: None,
        },
    )
}

/// Same as
/// [`application_router_and_media_and_library_and_runtime_configured`],
/// additionally setting the trusted WOPI host base URL (Phase 8) --
/// the one additional piece of server-computed configuration Office
/// session creation needs (Task 4/5/61). A separate constructor rather
/// than widening the existing one's signature, so every pre-existing
/// call site (Code/generic-runtime tests, which have nothing to do
/// with Office) is unaffected.
#[allow(clippy::too_many_arguments)]
pub fn application_router_and_media_and_library_and_runtime_and_office_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    enforce_hsts: bool,
    media: Option<clouddesk_media::MediaService>,
    library: Option<clouddesk_library::LibraryStore>,
    runtime: Option<Arc<clouddesk_orchestrator::RuntimeManager>>,
    office_wopi_host_base: Option<String>,
) -> Router {
    build_router(
        static_dir,
        AppState {
            version: env!("CARGO_PKG_VERSION"),
            auth: Some(auth),
            bootstrap_secret,
            privilege: None,
            enforce_hsts,
            media,
            library,
            runtime,
            runtime_allow_test_kind: false,
            office_wopi_host_base,
        },
    )
}

/// Test-only: same as
/// [`application_router_and_media_and_library_and_runtime_configured`],
/// but also allows `RuntimeKind::TestFixture` through the real HTTP
/// runtime-management routes, so `services/clouddeskd`'s own
/// integration tests can exercise the orchestrator's disposable test
/// fixture through the actual product API surface (Task 21-24) without
/// making the fixture reachable in any production router. `main.rs`
/// never calls this constructor -- only `services/clouddeskd/tests/*`
/// does, the same convention every other test-only constructor in this
/// file already follows (e.g. plain [`router`], unused by `main.rs`).
pub fn application_router_and_media_and_library_and_runtime_configured_for_tests(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    enforce_hsts: bool,
    media: Option<clouddesk_media::MediaService>,
    library: Option<clouddesk_library::LibraryStore>,
    runtime: Option<Arc<clouddesk_orchestrator::RuntimeManager>>,
) -> Router {
    build_router(
        static_dir,
        AppState {
            version: env!("CARGO_PKG_VERSION"),
            auth: Some(auth),
            bootstrap_secret,
            privilege: None,
            enforce_hsts,
            media,
            library,
            runtime,
            runtime_allow_test_kind: true,
            office_wopi_host_base: None,
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
    application_router_with_privilege_and_media_configured(
        static_dir,
        auth,
        bootstrap_secret,
        privilege,
        enforce_hsts,
        None,
    )
}

/// Same as [`application_router_with_privilege_configured`], additionally
/// wiring in the shared media (`FFmpeg`) service. Split out as its own
/// function, rather than adding a required parameter to the existing one,
/// so every prior call site (including tests) keeps compiling unchanged
/// with media support simply absent (`/api/v1/media/jobs*` then answers
/// "unavailable" rather than failing to build).
pub fn application_router_with_privilege_and_media_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    privilege: PrivilegeClient,
    enforce_hsts: bool,
    media: Option<clouddesk_media::MediaService>,
) -> Router {
    application_router_with_privilege_and_media_and_library_configured(
        static_dir,
        auth,
        bootstrap_secret,
        privilege,
        enforce_hsts,
        media,
        None,
    )
}

/// Same as [`application_router_with_privilege_and_media_configured`],
/// additionally wiring in the Music library store.
#[allow(clippy::too_many_arguments)]
pub fn application_router_with_privilege_and_media_and_library_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    privilege: PrivilegeClient,
    enforce_hsts: bool,
    media: Option<clouddesk_media::MediaService>,
    library: Option<clouddesk_library::LibraryStore>,
) -> Router {
    application_router_with_privilege_and_media_and_library_and_runtime_configured(
        static_dir,
        auth,
        bootstrap_secret,
        privilege,
        enforce_hsts,
        media,
        library,
        None,
    )
}

/// Same as
/// [`application_router_with_privilege_and_media_and_library_configured`],
/// additionally wiring in the Phase 6 optional-runtime orchestrator.
#[allow(clippy::too_many_arguments)]
pub fn application_router_with_privilege_and_media_and_library_and_runtime_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    privilege: PrivilegeClient,
    enforce_hsts: bool,
    media: Option<clouddesk_media::MediaService>,
    library: Option<clouddesk_library::LibraryStore>,
    runtime: Option<Arc<clouddesk_orchestrator::RuntimeManager>>,
) -> Router {
    build_router(
        static_dir,
        AppState {
            version: env!("CARGO_PKG_VERSION"),
            auth: Some(auth),
            bootstrap_secret,
            privilege: Some(privilege),
            enforce_hsts,
            media,
            library,
            runtime,
            runtime_allow_test_kind: false,
            office_wopi_host_base: None,
        },
    )
}

/// Same as
/// [`application_router_with_privilege_and_media_and_library_and_runtime_configured`],
/// additionally setting the trusted WOPI host base URL -- see
/// [`application_router_and_media_and_library_and_runtime_and_office_configured`].
#[allow(clippy::too_many_arguments)]
pub fn application_router_with_privilege_and_media_and_library_and_runtime_and_office_configured(
    static_dir: PathBuf,
    auth: AuthService,
    bootstrap_secret: PathBuf,
    privilege: PrivilegeClient,
    enforce_hsts: bool,
    media: Option<clouddesk_media::MediaService>,
    library: Option<clouddesk_library::LibraryStore>,
    runtime: Option<Arc<clouddesk_orchestrator::RuntimeManager>>,
    office_wopi_host_base: Option<String>,
) -> Router {
    build_router(
        static_dir,
        AppState {
            version: env!("CARGO_PKG_VERSION"),
            auth: Some(auth),
            bootstrap_secret,
            privilege: Some(privilege),
            enforce_hsts,
            media,
            library,
            runtime,
            runtime_allow_test_kind: false,
            office_wopi_host_base,
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
        .route(
            "/api/v1/users/{user_id}/assigned-roots/{root_id}",
            delete(remove_assigned_root),
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
        .route("/api/v1/media/availability", get(media::availability))
        .route("/api/v1/media/probe", post(media::probe))
        .route("/api/v1/media/jobs", post(media::create_job))
        .route(
            "/api/v1/media/jobs/{job_id}",
            get(media::job_status).delete(media::cancel_job),
        )
        .route("/api/v1/media/jobs/{job_id}/output", get(media::job_output))
        .route("/api/v1/media/subtitles", post(media::subtitles))
        .route(
            "/api/v1/media/resume",
            get(media::get_resume).put(media::put_resume),
        )
        .route(
            "/api/v1/music/roots",
            get(music::list_roots).post(music::add_root),
        )
        .route("/api/v1/music/roots/{root_id}", delete(music::remove_root))
        .route("/api/v1/music/roots/{root_id}/scan", post(music::scan_root))
        .route("/api/v1/music/tracks", get(music::list_tracks))
        .route(
            "/api/v1/music/tracks/{track_id}/artwork",
            get(music::artwork),
        )
        .route("/api/v1/music/artists", get(music::list_artists))
        .route("/api/v1/music/albums", get(music::list_albums))
        .route("/api/v1/music/search", get(music::search))
        .route(
            "/api/v1/music/playlists",
            get(music::list_playlists).post(music::create_playlist),
        )
        .route(
            "/api/v1/music/playlists/{playlist_id}",
            get(music::playlist_entries)
                .put(music::rename_playlist)
                .delete(music::delete_playlist),
        )
        .route(
            "/api/v1/music/playlists/{playlist_id}/entries",
            post(music::add_playlist_entry),
        )
        .route(
            "/api/v1/music/playlists/{playlist_id}/entries/{entry_id}",
            delete(music::remove_playlist_entry),
        )
        .route(
            "/api/v1/music/playlists/{playlist_id}/reorder",
            put(music::reorder_playlist),
        )
        .route("/api/v1/music/favorites", get(music::list_favorites))
        .route(
            "/api/v1/music/favorites/{track_id}",
            put(music::favorite).delete(music::unfavorite),
        )
        .route(
            "/api/v1/music/recent",
            get(music::recently_played).post(music::record_played),
        )
        .route(
            "/api/v1/music/queue",
            get(music::get_queue).put(music::set_queue),
        )
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
        .route("/api/v1/runtimes", get(runtime::list_kinds))
        .route(
            "/api/v1/code/workspaces",
            get(runtime::list_code_workspaces),
        )
        .route("/api/v1/runtimes/{kind}/enable", post(runtime::enable))
        .route("/api/v1/runtimes/{kind}/disable", post(runtime::disable))
        .route(
            "/api/v1/runtime-instances",
            get(runtime::list_instances).post(runtime::create_instance),
        )
        .route(
            "/api/v1/runtime-instances/{kind}/{instance_id}",
            get(runtime::instance_status),
        )
        .route(
            "/api/v1/runtime-instances/{kind}/{instance_id}/stop",
            post(runtime::stop_instance),
        )
        .route(
            "/api/v1/runtime-instances/{kind}/{instance_id}/restart",
            post(runtime::restart_instance),
        )
        .route(
            "/api/v1/runtime-instances/{kind}/{instance_id}/logs",
            get(runtime::instance_logs),
        )
        .route(
            "/api/v1/runtime-instances/{kind}/{instance_id}/proxy-ws",
            get(runtime::ws_proxy),
        )
        .route(
            "/api/v1/runtime-instances/{kind}/{instance_id}/proxy",
            any(runtime::http_proxy_root),
        )
        .route(
            "/api/v1/runtime-instances/{kind}/{instance_id}/proxy/",
            any(runtime::http_proxy_root),
        )
        .route(
            "/api/v1/runtime-instances/{kind}/{instance_id}/proxy/{*upstream_path}",
            any(runtime::http_proxy),
        )
        .route("/api/v1/office/sessions", post(wopi_api::open_session))
        .route(
            "/wopi/files/{id}",
            get(wopi_api::check_file_info).post(wopi_api::file_operation),
        )
        .route(
            "/wopi/files/{id}/contents",
            get(wopi_api::get_file).post(wopi_api::put_file),
        )
        .route(
            "/api/v1/runtime-instances/office/{instance_id}/office-proxy-ws",
            get(wopi_api::office_ws_proxy),
        )
        .route(
            "/api/v1/runtime-instances/office/{instance_id}/office-proxy-ws/{*upstream_path}",
            get(wopi_api::office_ws_proxy_path),
        )
        .route(
            "/api/v1/runtime-instances/office/{instance_id}/office-proxy",
            any(wopi_api::office_http_proxy_root),
        )
        .route(
            "/api/v1/runtime-instances/office/{instance_id}/office-proxy/",
            any(wopi_api::office_http_proxy_root),
        )
        .route(
            "/api/v1/runtime-instances/office/{instance_id}/office-proxy/{*upstream_path}",
            any(wopi_api::office_http_proxy),
        )
        .fallback_service(ServeDir::new(static_dir).append_index_html_on_directories(true))
        .layer(middleware::from_fn_with_state(enforce_hsts, web_security))
        .layer(TraceLayer::new_for_http().make_span_with(make_redacted_span))
        .with_state(state)
}

/// Phase 8 Task 43/70: WOPI access tokens travel in the `access_token`
/// query parameter (the WOPI protocol's own convention -- not a header
/// `CloudDesk` chose). `TraceLayer`'s default span construction logs the
/// full request URI including that query string, which would otherwise
/// leak every token into `clouddeskd`'s own logs. This redacts any
/// `access_token` (or generically-named `token`) query value before it
/// ever reaches a tracing span, for every route, not just `/wopi/*`.
fn make_redacted_span(request: &Request<Body>) -> tracing::Span {
    let redacted = redact_token_query(request.uri());
    tracing::info_span!(
        "request",
        method = %request.method(),
        uri = %redacted,
    )
}

fn redact_token_query(uri: &Uri) -> String {
    let path = uri.path();
    let Some(query) = uri.query() else {
        return path.to_owned();
    };
    let redacted_query: Vec<String> = query
        .split('&')
        .map(|pair| {
            let key = pair.split('=').next().unwrap_or_default();
            if key.eq_ignore_ascii_case("access_token") || key.eq_ignore_ascii_case("token") {
                format!("{key}=***")
            } else {
                pair.to_owned()
            }
        })
        .collect();
    format!("{path}?{}", redacted_query.join("&"))
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
            'runtime.browser.enabled', 'runtime.code.enabled', 'runtime.office.enabled',
            'runtime.media.enabled'
         )",
    )
    .fetch_all(auth.pool())
    .await
    .map_err(AuthError::from)?;
    let mut flags = json!({ "browser": false, "code": false, "office": false, "media": false });
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

/// Admin-only revocation (Phase 7 Task 2 test coverage: "revoked
/// assignment fails"). Any Code/Files authorization that re-resolves
/// this `root_id` afterward fails closed once the row is gone -- see
/// `resolve_workspace`/`resolve_own_assigned_root`.
async fn remove_assigned_root(
    State(state): State<AppState>,
    Path((user_id, root_id)): Path<(String, String)>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    let (source_ip, user_agent) = request_metadata(connect, &headers);
    auth.remove_assigned_root(&principal, &root_id, &source_ip, &user_agent)
        .await?;

    // Phase 7 closure Task 3: prefer immediate termination over the
    // weaker "no NEW access is authorized" policy alone -- a running
    // container's OS-level bind mount cannot be revoked in place (no
    // live-remount primitive, same constraint as workspace switching),
    // so if the affected user's live Code instance is currently
    // mounting exactly this workspace, stop it now rather than leaving
    // that mount reachable until whatever restart/switch happens to
    // come next.
    if let Some(runtime) = &state.runtime {
        terminate_code_instance_using_workspace(runtime, &user_id, &root_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// See `remove_assigned_root`. Best-effort: any failure to read the
/// live instance/marker just means there is nothing to terminate (a
/// stopped/never-started instance holds no mount to revoke), not an
/// error the revocation request itself should fail on.
async fn terminate_code_instance_using_workspace(
    runtime: &clouddesk_orchestrator::RuntimeManager,
    owner_user_id: &str,
    root_id: &str,
) {
    let Ok(existing) = runtime.store().list_for_owner(owner_user_id).await else {
        return;
    };
    let Some(row) = existing
        .into_iter()
        .find(|row| row.kind == clouddesk_orchestrator::RuntimeKind::Code)
    else {
        return;
    };
    let id = clouddesk_orchestrator::InstanceId {
        kind: clouddesk_orchestrator::RuntimeKind::Code,
        owner_user_id: owner_user_id.to_owned(),
        instance_id: row.instance_id,
    };
    let Ok(state_dir) = runtime.instance_state_dir(&id) else {
        return;
    };
    let Ok(raw) = tokio::fs::read(state_dir.join(crate::code_runtime::CODE_IDENTITY_MARKER)).await
    else {
        return;
    };
    let Ok(marker) = serde_json::from_slice::<crate::code_runtime::CodeIdentityMarker>(&raw) else {
        return;
    };
    if marker.workspace_id.as_deref() == Some(root_id) {
        let _ = runtime.stop_instance(owner_user_id, &id).await;
    }
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
    // ACL edits are gated by their own `files.permissions.change`
    // capability (see `PrivilegedAction::required_capability`), not the
    // blanket `files.local.write` every other mutation shares — a
    // principal with only the ACL capability must still get a writable
    // `LocalProvider`, or the authorized operation would fail as if it
    // were read-only.
    let is_set_acl = matches!(operation, LocalFileOperation::SetAcl { .. });
    let writable = principal.can("files.local.write")
        || (is_set_acl && principal.can("files.permissions.change"));
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
/// HTTP surface over `clouddesk_media`. Every handler resolves the
/// caller's path through the same `resolve_safe_path` VFS authorization
/// every other file endpoint uses (never a raw filesystem path from the
/// client), and every job lookup is owner-scoped through
/// `MediaJobStore::get` so one user can never observe or control another
/// user's probe/remux/transcode job.
/// HTTP surface over `clouddesk_library`. Every handler is scoped to the
/// caller's own `owner_user_id` -- the store layer itself refuses to
/// return or mutate another user's rows, so cross-user isolation holds
/// even if a handler here forgot to check (defense in depth, not the
/// only guard). Library roots are ordinary VFS-authorized paths (same
/// `resolve_safe_path` as every other file endpoint); scanning/artwork
/// reuse Phase 3's `MediaService` rather than reimplementing any
/// `ffmpeg` invocation.
pub(crate) mod music {
    use super::{
        request_metadata, resolve_safe_path, ApiError, AppState, ConnectInfo, HeaderMap, Path,
        Query, State,
    };
    use axum::{
        http::StatusCode,
        response::{IntoResponse, Response},
        Json,
    };
    use clouddesk_library::LibraryStore;
    use serde::Deserialize;
    use serde_json::json;
    use std::net::SocketAddr;

    fn require_library(state: &AppState) -> Result<&LibraryStore, ApiError> {
        state
            .library
            .as_ref()
            .ok_or_else(ApiError::library_unavailable)
    }

    #[derive(Deserialize)]
    pub(crate) struct AddRootBody {
        path: String,
    }

    pub(crate) async fn add_root(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
        Json(body): Json<AddRootBody>,
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
        // Reject a root outside the caller's own VFS root up front --
        // the same authorization every other file endpoint uses. The
        // resolved real path is not stored; scans re-resolve it fresh
        // every time (see `scan_root` below), so a later assigned-root
        // change is honored on the next scan rather than baked in here.
        let _ = resolve_safe_path(&identity.home, &body.path)?;
        let library = require_library(&state)?;
        let root = library
            .add_root(&principal.user_id, &body.path)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let (source_ip, user_agent) = request_metadata(connect, &headers);
        auth.audit_action(
            &principal,
            "music.library_root.configured",
            "music_library_root",
            Some(root.id.clone()),
            "success",
            json!({ "path": body.path }),
            &source_ip,
            &user_agent,
        )
        .await?;
        Ok(Json(json!({ "id": root.id, "path": root.virtual_path })))
    }

    pub(crate) async fn list_roots(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let roots = library
            .list_roots(&principal.user_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(json!(roots
            .into_iter()
            .map(|r| json!({ "id": r.id, "path": r.virtual_path }))
            .collect::<Vec<_>>())))
    }

    pub(crate) async fn remove_root(
        State(state): State<AppState>,
        Path(root_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        library
            .remove_root(&principal.user_id, &root_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    }

    pub(crate) async fn scan_root(
        State(state): State<AppState>,
        Path(root_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let identity = super::mapped_identity(auth, &principal).await?;
        let library = require_library(&state)?;
        let root = library
            .get_root(&principal.user_id, &root_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .ok_or_else(|| ApiError::not_found("library root not found"))?;
        // Re-resolved fresh on every scan (Task 11's reauthorization
        // discipline, same as Video/media): a root whose assigned-root
        // authorization was revoked since it was added is rejected here,
        // not silently scanned anyway.
        let real_root = resolve_safe_path(&identity.home, &root.virtual_path)?;
        let summary = clouddesk_library::scan_root(
            library,
            &principal.user_id,
            &root.id,
            &real_root,
            &root.virtual_path,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(serde_json::to_value(&summary).unwrap_or(json!({}))))
    }

    #[derive(Deserialize)]
    pub(crate) struct PageQuery {
        #[serde(default)]
        limit: Option<i64>,
        #[serde(default)]
        offset: Option<i64>,
    }

    pub(crate) async fn list_tracks(
        State(state): State<AppState>,
        Query(query): Query<PageQuery>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let limit = query.limit.unwrap_or(200);
        let offset = query.offset.unwrap_or(0);
        let tracks = library
            .list_tracks(&principal.user_id, limit, offset)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let total = library
            .count_tracks(&principal.user_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(json!({ "tracks": tracks, "total": total })))
    }

    pub(crate) async fn list_artists(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let artists = library
            .list_artists(&principal.user_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(json!(artists)))
    }

    pub(crate) async fn list_albums(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let albums = library
            .list_albums(&principal.user_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(json!(albums
            .into_iter()
            .map(|(album, artist, year)| json!({ "album": album, "artist": artist, "year": year }))
            .collect::<Vec<_>>())))
    }

    #[derive(Deserialize)]
    pub(crate) struct SearchQuery {
        q: String,
    }

    pub(crate) async fn search(
        State(state): State<AppState>,
        Query(query): Query<SearchQuery>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let results = library
            .search(&principal.user_id, &query.q, 100)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(json!(results)))
    }

    /// Serves cover art for `track_id`: embedded artwork first (via
    /// Phase 3's `MediaService::extract_artwork`), falling back to a
    /// `cover.jpg`/`folder.jpg`-style sidecar file in the track's own
    /// (already-authorized) directory -- never an arbitrary path a tag
    /// could point at. 404 if neither exists; that is the overwhelmingly
    /// common case (most files have no artwork) and must not read as an
    /// error.
    pub(crate) async fn artwork(
        State(state): State<AppState>,
        Path(track_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<Response, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let identity = super::mapped_identity(auth, &principal).await?;
        let library = require_library(&state)?;
        let track = library
            .get_track(&principal.user_id, &track_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .ok_or_else(|| ApiError::not_found("track not found"))?;
        let real_path = resolve_safe_path(&identity.home, &track.virtual_path)?;

        if let Some(media) = &state.media {
            if let Ok((artwork_path, workspace)) = media.extract_artwork(&real_path).await {
                let bytes = tokio::fs::read(&artwork_path).await;
                let _ = tokio::fs::remove_dir_all(&workspace).await;
                if let Ok(bytes) = bytes {
                    let mut response = (StatusCode::OK, bytes).into_response();
                    response.headers_mut().insert(
                        axum::http::header::CONTENT_TYPE,
                        axum::http::HeaderValue::from_static("image/jpeg"),
                    );
                    return Ok(response);
                }
            }
        }

        // No embedded artwork (or media unavailable) -- try a sidecar
        // file in the same directory, itself reached only through the
        // same VFS authorization as the track.
        if let Some(parent) = real_path.parent() {
            for name in ["cover.jpg", "cover.png", "folder.jpg", "folder.png"] {
                let candidate = parent.join(name);
                if let Ok(metadata) = tokio::fs::metadata(&candidate).await {
                    const MAX_SIDECAR_BYTES: u64 = 10 * 1024 * 1024;
                    if metadata.is_file() && metadata.len() <= MAX_SIDECAR_BYTES {
                        return super::serve_file_stream(&candidate, &headers, false).await;
                    }
                }
            }
        }
        Err(ApiError::not_found("no artwork available for this track"))
    }

    #[derive(Deserialize)]
    pub(crate) struct CreatePlaylistBody {
        name: String,
    }

    pub(crate) async fn create_playlist(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
        Json(body): Json<CreatePlaylistBody>,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let playlist = library
            .create_playlist(&principal.user_id, &body.name)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let (source_ip, user_agent) = request_metadata(connect, &headers);
        auth.audit_action(
            &principal,
            "music.playlist.created",
            "music_playlist",
            Some(playlist.id.clone()),
            "success",
            json!({}),
            &source_ip,
            &user_agent,
        )
        .await?;
        Ok(Json(json!({ "id": playlist.id, "name": playlist.name })))
    }

    pub(crate) async fn list_playlists(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let playlists = library
            .list_playlists(&principal.user_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(json!(playlists)))
    }

    pub(crate) async fn playlist_entries(
        State(state): State<AppState>,
        Path(playlist_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let entries = library
            .playlist_entries(&principal.user_id, &playlist_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .ok_or_else(|| ApiError::not_found("playlist not found"))?;
        Ok(Json(json!(entries
            .into_iter()
            .map(|(entry_id, track)| json!({ "entry_id": entry_id, "track": track }))
            .collect::<Vec<_>>())))
    }

    #[derive(Deserialize)]
    pub(crate) struct RenamePlaylistBody {
        name: String,
    }

    pub(crate) async fn rename_playlist(
        State(state): State<AppState>,
        Path(playlist_id): Path<String>,
        headers: HeaderMap,
        Json(body): Json<RenamePlaylistBody>,
    ) -> Result<StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let ok = library
            .rename_playlist(&principal.user_id, &playlist_id, &body.name)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !ok {
            return Err(ApiError::not_found("playlist not found"));
        }
        Ok(StatusCode::NO_CONTENT)
    }

    pub(crate) async fn delete_playlist(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        Path(playlist_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        library
            .delete_playlist(&principal.user_id, &playlist_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let (source_ip, user_agent) = request_metadata(connect, &headers);
        auth.audit_action(
            &principal,
            "music.playlist.deleted",
            "music_playlist",
            Some(playlist_id),
            "success",
            json!({}),
            &source_ip,
            &user_agent,
        )
        .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    #[derive(Deserialize)]
    pub(crate) struct AddEntryBody {
        track_id: String,
    }

    pub(crate) async fn add_playlist_entry(
        State(state): State<AppState>,
        Path(playlist_id): Path<String>,
        headers: HeaderMap,
        Json(body): Json<AddEntryBody>,
    ) -> Result<StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let ok = library
            .add_playlist_entry(&principal.user_id, &playlist_id, &body.track_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !ok {
            return Err(ApiError::not_found("playlist or track not found"));
        }
        Ok(StatusCode::NO_CONTENT)
    }

    pub(crate) async fn remove_playlist_entry(
        State(state): State<AppState>,
        Path((playlist_id, entry_id)): Path<(String, String)>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let ok = library
            .remove_playlist_entry(&principal.user_id, &playlist_id, &entry_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !ok {
            return Err(ApiError::not_found("playlist not found"));
        }
        Ok(StatusCode::NO_CONTENT)
    }

    #[derive(Deserialize)]
    pub(crate) struct ReorderBody {
        entry_ids: Vec<String>,
    }

    pub(crate) async fn reorder_playlist(
        State(state): State<AppState>,
        Path(playlist_id): Path<String>,
        headers: HeaderMap,
        Json(body): Json<ReorderBody>,
    ) -> Result<StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let ok = library
            .reorder_playlist(&principal.user_id, &playlist_id, &body.entry_ids)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !ok {
            return Err(ApiError::not_found("playlist not found"));
        }
        Ok(StatusCode::NO_CONTENT)
    }

    pub(crate) async fn list_favorites(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let favorites = library
            .list_favorites(&principal.user_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(json!(favorites)))
    }

    pub(crate) async fn favorite(
        State(state): State<AppState>,
        Path(track_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let ok = library
            .favorite(&principal.user_id, &track_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !ok {
            return Err(ApiError::not_found("track not found"));
        }
        Ok(StatusCode::NO_CONTENT)
    }

    pub(crate) async fn unfavorite(
        State(state): State<AppState>,
        Path(track_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        library
            .unfavorite(&principal.user_id, &track_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    }

    pub(crate) async fn recently_played(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let recent = library
            .recently_played(&principal.user_id, 50)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(json!(recent)))
    }

    #[derive(Deserialize)]
    pub(crate) struct RecordPlayedBody {
        track_id: String,
    }

    pub(crate) async fn record_played(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(body): Json<RecordPlayedBody>,
    ) -> Result<StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let ok = library
            .record_played(&principal.user_id, &body.track_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !ok {
            return Err(ApiError::not_found("track not found"));
        }
        Ok(StatusCode::NO_CONTENT)
    }

    pub(crate) async fn get_queue(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let library = require_library(&state)?;
        let queue = library
            .get_queue(&principal.user_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(json!({ "track_ids": queue })))
    }

    #[derive(Deserialize)]
    pub(crate) struct SetQueueBody {
        track_ids: Vec<String>,
    }

    pub(crate) async fn set_queue(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(body): Json<SetQueueBody>,
    ) -> Result<StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.write") {
            return Err(ApiError::forbidden());
        }
        if body.track_ids.len() > 2000 {
            return Err(ApiError::bad_request("queue too large"));
        }
        let library = require_library(&state)?;
        library
            .set_queue(&principal.user_id, &body.track_ids)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    }
}

pub(crate) mod media {
    use super::{
        request_metadata, resolve_safe_path, ApiError, AppState, ConnectInfo, HeaderMap, Path,
        State,
    };
    use axum::{
        http::{header, HeaderValue, StatusCode},
        response::{IntoResponse, Response},
        Json,
    };
    use clouddesk_media::{JobOperation, MediaService};
    use serde::Deserialize;
    use serde_json::json;
    use sqlx::Row;
    use std::net::SocketAddr;

    fn require_media(state: &AppState) -> Result<&MediaService, ApiError> {
        state.media.as_ref().ok_or_else(ApiError::media_unavailable)
    }

    #[derive(Deserialize)]
    pub(crate) struct PathBody {
        path: String,
    }

    pub(crate) async fn availability(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let _principal = super::principal(&state, &headers).await?;
        Ok(Json(match &state.media {
            Some(media) => serde_json::to_value(media.availability())
                .map_err(|e| ApiError::internal(e.to_string()))?,
            None => json!({ "state": "disabled" }),
        }))
    }

    pub(crate) async fn probe(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
        Json(body): Json<PathBody>,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        super::authorize_request(
            auth,
            &principal,
            "files.local.read",
            false,
            connect,
            &headers,
        )
        .await?;
        let identity = super::mapped_identity(auth, &principal).await?;
        let real_path = resolve_safe_path(&identity.home, &body.path)?;
        let media = require_media(&state)?;
        let (probe, plan) = media.probe(&real_path).await.map_err(|e| match e {
            clouddesk_media::MediaServiceError::Unavailable => ApiError::media_unavailable(),
            other => ApiError::bad_request_owned(other.to_string()),
        })?;
        Ok(Json(json!({ "probe": probe, "plan": plan })))
    }

    #[derive(Deserialize)]
    pub(crate) struct CreateJobBody {
        path: String,
        operation: JobOperationBody,
        /// Which audio stream to keep, by its 0-based position among the
        /// source's audio streams (see `MediaProbe::audio_streams()`
        /// ordering) -- never a raw `ffmpeg` map expression. `None` keeps
        /// whatever the default track selection would be.
        #[serde(default)]
        audio_track_ordinal: Option<u32>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum JobOperationBody {
        Remux,
        Transcode,
    }

    pub(crate) async fn create_job(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
        Json(body): Json<CreateJobBody>,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        super::authorize_request(auth, &principal, "apps.media.use", false, connect, &headers)
            .await?;
        let identity = super::mapped_identity(auth, &principal).await?;
        let real_path = resolve_safe_path(&identity.home, &body.path)?;
        let media = require_media(&state)?;
        let operation = match body.operation {
            JobOperationBody::Remux => JobOperation::Remux,
            JobOperationBody::Transcode => JobOperation::Transcode,
        };
        let selection = clouddesk_media::exec::TrackSelection {
            audio_track_ordinal: body.audio_track_ordinal,
        };
        let job = media
            .start_job(
                &principal.user_id,
                &body.path,
                real_path,
                operation,
                selection,
            )
            .await
            .map_err(|e| match e {
                clouddesk_media::MediaServiceError::Unavailable => ApiError::media_unavailable(),
                clouddesk_media::MediaServiceError::Busy => {
                    ApiError::too_many_requests("too many concurrent media jobs; try again shortly")
                }
                other => ApiError::internal(other.to_string()),
            })?;
        let (source_ip, user_agent) = request_metadata(connect, &headers);
        auth.audit_action(
            &principal,
            "media.job.requested",
            "media_job",
            Some(job.id.clone()),
            "success",
            json!({ "operation": format!("{:?}", job.operation) }),
            &source_ip,
            &user_agent,
        )
        .await?;
        Ok(Json(json!({ "job_id": job.id, "state": job.state })))
    }

    pub(crate) async fn job_status(
        State(state): State<AppState>,
        Path(job_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("apps.media.use") {
            return Err(ApiError::forbidden());
        }
        let media = require_media(&state)?;
        let job = media
            .store()
            .get(&principal.user_id, &job_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .ok_or_else(|| ApiError::not_found("media job not found"))?;
        Ok(Json(json!({
            "job_id": job.id,
            "state": job.state,
            "operation": job.operation,
            "error_class": job.error_class,
        })))
    }

    pub(crate) async fn cancel_job(
        State(state): State<AppState>,
        Path(job_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<axum::http::StatusCode, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("apps.media.use") {
            return Err(ApiError::forbidden());
        }
        let media = require_media(&state)?;
        media
            .cancel_job(&principal.user_id, &job_id)
            .await
            .map_err(|e| match e {
                clouddesk_media::MediaServiceError::NotFound => {
                    ApiError::not_found("media job not found")
                }
                other => ApiError::internal(other.to_string()),
            })?;
        Ok(axum::http::StatusCode::NO_CONTENT)
    }

    pub(crate) async fn job_output(
        State(state): State<AppState>,
        Path(job_id): Path<String>,
        headers: HeaderMap,
    ) -> Result<Response, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("apps.media.use") {
            return Err(ApiError::forbidden());
        }
        let media = require_media(&state)?;
        let job = media
            .store()
            .get(&principal.user_id, &job_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .ok_or_else(|| ApiError::not_found("media job not found"))?;
        if job.state != clouddesk_media::JobState::Completed {
            return Err(ApiError::bad_request("media job has no output yet"));
        }
        let Some(output_path) = job.output_path else {
            return Err(ApiError::internal("completed job has no recorded output"));
        };
        super::serve_file_stream(std::path::Path::new(&output_path), &headers, false).await
    }

    #[derive(Deserialize)]
    pub(crate) struct SubtitleBody {
        path: String,
        stream_index: u32,
    }

    /// Extracts one subtitle track to `WebVTT` and returns it directly in
    /// the response body -- no job/polling, since a text stream extracts
    /// in well under a second. `stream_index` is validated against a
    /// fresh probe of the caller's own authorized file before being
    /// passed to `ffmpeg`, so a client can never point extraction at a
    /// stream that isn't genuinely a subtitle track on a file it's
    /// authorized to read.
    pub(crate) async fn subtitles(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
        Json(body): Json<SubtitleBody>,
    ) -> Result<Response, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        super::authorize_request(
            auth,
            &principal,
            "files.local.read",
            false,
            connect,
            &headers,
        )
        .await?;
        let identity = super::mapped_identity(auth, &principal).await?;
        let real_path = resolve_safe_path(&identity.home, &body.path)?;
        let media = require_media(&state)?;

        let (probe, _plan) = media
            .probe(&real_path)
            .await
            .map_err(|e| ApiError::bad_request_owned(e.to_string()))?;
        if !probe
            .subtitle_streams()
            .iter()
            .any(|stream| stream.index == body.stream_index)
        {
            return Err(ApiError::bad_request(
                "stream_index is not a subtitle stream on this file",
            ));
        }

        let (vtt_path, workspace) = media
            .extract_subtitle(&real_path, body.stream_index)
            .await
            .map_err(|e| match e {
                clouddesk_media::MediaServiceError::Unavailable => ApiError::media_unavailable(),
                other => ApiError::bad_request_owned(other.to_string()),
            })?;
        let bytes = tokio::fs::read(&vtt_path).await;
        let _ = tokio::fs::remove_dir_all(&workspace).await;
        let bytes = bytes.map_err(|e| ApiError::internal(e.to_string()))?;

        let mut response = (StatusCode::OK, bytes).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/vtt; charset=utf-8"),
        );
        Ok(response)
    }

    #[derive(Deserialize)]
    pub(crate) struct ResumePathQuery {
        path: String,
    }

    /// Playback resume position is keyed by (owner, virtual path) -- see
    /// the `media_playback_state` migration -- never exposed or writable
    /// for any user other than the caller.
    pub(crate) async fn get_resume(
        State(state): State<AppState>,
        axum::extract::Query(query): axum::extract::Query<ResumePathQuery>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        let row = sqlx::query(
            "SELECT position_seconds, duration_seconds, updated_at FROM media_playback_state
             WHERE owner_user_id = ? AND virtual_path = ?",
        )
        .bind(&principal.user_id)
        .bind(&query.path)
        .fetch_optional(auth.pool())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(Json(match row {
            Some(row) => json!({
                "position_seconds": row.get::<f64, _>("position_seconds"),
                "duration_seconds": row.get::<Option<f64>, _>("duration_seconds"),
                "updated_at": row.get::<i64, _>("updated_at"),
            }),
            None => json!(null),
        }))
    }

    #[derive(Deserialize)]
    pub(crate) struct PutResumeBody {
        path: String,
        position_seconds: f64,
        #[serde(default)]
        duration_seconds: Option<f64>,
    }

    pub(crate) async fn put_resume(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(body): Json<PutResumeBody>,
    ) -> Result<StatusCode, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        if !principal.can("files.local.read") {
            return Err(ApiError::forbidden());
        }
        if !body.position_seconds.is_finite() || body.position_seconds < 0.0 {
            return Err(ApiError::bad_request(
                "position_seconds must be a finite, non-negative number",
            ));
        }
        sqlx::query(
            "INSERT INTO media_playback_state
                (owner_user_id, virtual_path, position_seconds, duration_seconds, updated_at)
             VALUES (?, ?, ?, ?, unixepoch())
             ON CONFLICT (owner_user_id, virtual_path) DO UPDATE SET
                position_seconds = excluded.position_seconds,
                duration_seconds = excluded.duration_seconds,
                updated_at = excluded.updated_at",
        )
        .bind(&principal.user_id)
        .bind(&body.path)
        .bind(body.position_seconds)
        .bind(body.duration_seconds)
        .execute(auth.pool())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    }
}

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

/// Parses a single `start-end` (or `start-` / `-suffix_len`) byte-range
/// spec against a file of `total_len` bytes.
///
/// Returns `None` if the spec isn't syntactically a range at all (caller
/// should ignore `Range` and serve the full body). Returns
/// `Some(Err(()))` if it parses but is unsatisfiable against `total_len`
/// (caller should answer 416). Returns `Some(Ok((start, end)))`
/// (inclusive, clamped to `total_len - 1`) otherwise.
fn parse_single_range(spec: &str, total_len: u64) -> Option<Result<(u64, u64), ()>> {
    let (start_str, end_str) = spec.split_once('-')?;
    if start_str.is_empty() {
        // Suffix range: `-N` means "the last N bytes".
        let suffix_len: u64 = end_str.parse().ok()?;
        if suffix_len == 0 || total_len == 0 {
            return Some(Err(()));
        }
        let start = total_len.saturating_sub(suffix_len);
        return Some(Ok((start, total_len - 1)));
    }
    let start: u64 = start_str.parse().ok()?;
    let end: Option<u64> = if end_str.is_empty() {
        None
    } else {
        Some(end_str.parse().ok()?)
    };
    if start >= total_len {
        return Some(Err(()));
    }
    let end = end.unwrap_or(total_len - 1).min(total_len - 1);
    if start > end {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
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
        // A single `bytes=start-end` spec only -- a comma means the client
        // asked for multiple ranges, which this endpoint doesn't support;
        // per RFC 7233 §3.1 a server that can't honor multipart ranges may
        // just ignore Range entirely and serve the full 200 body below,
        // rather than misparsing the second range into a bogus single one.
        if let Some(range_spec) = range_header
            .strip_prefix("bytes=")
            .filter(|spec| !spec.contains(','))
        {
            match parse_single_range(range_spec, total_len) {
                Some(Ok((start, end))) => {
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
                // Syntactically a range, but out of bounds (start beyond
                // EOF, reversed start>end, etc.) -- RFC 7233 §4.4 requires
                // 416, not silently serving the whole file as if Range had
                // never been sent.
                Some(Err(())) => {
                    let mut response =
                        (StatusCode::RANGE_NOT_SATISFIABLE, Body::empty()).into_response();
                    let headers_mut = response.headers_mut();
                    headers_mut.insert(
                        header::CONTENT_RANGE,
                        HeaderValue::from_str(&format!("bytes */{total_len}"))
                            .map_err(|e| ApiError::internal(e.to_string()))?,
                    );
                    return Ok(response);
                }
                // Not parseable as a range at all -- ignore it and serve
                // the full body (RFC 7233 §2.1: a malformed Range header
                // MUST be ignored, not rejected).
                None => {}
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

    /// Like `bad_request`, but the (non-secret, already-typed) reason
    /// isn't known until runtime -- e.g. a probe/malformed-media error
    /// message -- so it can't be a `&'static str`. Reported as the public
    /// message rather than only logged: these are all typed, user-facing
    /// "this input is bad" outcomes, never raw internals.
    fn bad_request_owned(message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            public_message: "request could not be processed",
            internal: Some(message),
        }
    }

    fn too_many_requests(message: &'static str) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
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

    fn media_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            public_message: "media/FFmpeg support is disabled or unavailable",
            internal: None,
        }
    }

    fn library_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            public_message: "the music library service is not initialized",
            internal: None,
        }
    }

    fn runtime_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            public_message: "runtime management is not initialized",
            internal: None,
        }
    }

    fn conflict(message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            public_message: message,
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

    /// Like `bad_gateway`, but the reason (an upstream/proxy failure
    /// detail) isn't known until runtime -- logged, never sent to the
    /// client as-is (an internal `reqwest`/`tungstenite` error string
    /// could otherwise leak upstream addressing details).
    fn bad_gateway_owned(message: String) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            public_message: "runtime proxy request failed",
            internal: Some(message),
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
            AuthError::UnknownAssignedRoot => Self::not_found("workspace not found"),
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

impl From<clouddesk_orchestrator::proxy::ProxyError> for ApiError {
    fn from(error: clouddesk_orchestrator::proxy::ProxyError) -> Self {
        use clouddesk_orchestrator::proxy::ProxyError;
        match error {
            // Same discipline as every other cross-user lookup in this
            // file: a proxy target that doesn't exist and one that
            // belongs to someone else are indistinguishable to the
            // caller (Task 5/21).
            ProxyError::NotFound => Self::not_found("runtime instance not found"),
            ProxyError::NotRunning => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                public_message: "runtime instance is not currently running",
                internal: None,
            },
            ProxyError::Upstream(message) => Self::bad_gateway_owned(message),
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

/// Periodically removes expired WOPI lock rows (Phase 8 Task 1).
///
/// Purely storage hygiene: `wopi::get_lock` already treats an expired
/// row as absent on every read path, so a legitimate new LOCK is never
/// blocked by an abandoned session regardless of when this runs. The
/// sweep only stops `office_locks` from accumulating dead rows.
pub fn spawn_office_lock_janitor(pool: sqlx::SqlitePool) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_mins(1));
        loop {
            ticker.tick().await;
            match crate::wopi::sweep_expired_locks(&pool).await {
                Ok(removed) if removed > 0 => {
                    tracing::debug!(removed, "swept expired Office WOPI locks");
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(?error, "Office WOPI lock sweep failed"),
            }
        }
    });
}

/// Phase 6 optional-runtime (Code/Office/Browser) HTTP surface. Every
/// handler here is a thin, ownership-scoped wrapper around the single
/// shared `clouddesk_orchestrator::RuntimeManager` already verified by
/// `crates/orchestrator`'s own live test suite -- this module never
/// spawns a process, opens a container, or picks an upstream address
/// itself (Task 1: do not create a second runtime manager).
pub(crate) mod runtime {
    use super::{
        request_metadata, ApiError, AppState, ConnectInfo, HeaderMap, Method, Path, State, Uri,
    };
    use axum::{
        extract::ws::WebSocketUpgrade,
        http::StatusCode,
        response::{IntoResponse, Response},
        Json,
    };
    use clouddesk_auth::{AuthService, SessionPrincipal};
    use clouddesk_orchestrator::{
        proxy::{proxy_http, proxy_ws},
        Availability, InstanceId, Persistence, RuntimeKind, RuntimeManager, StartError, StopError,
    };
    use serde::Deserialize;
    use serde_json::json;
    use std::{net::SocketAddr, sync::Arc};

    /// Bounded log read (Task 12) -- never client-configurable, so a
    /// caller cannot force an unbounded response.
    const MAX_RUNTIME_LOG_BYTES: usize = 64 * 1024;

    /// Explicit, deliberate WebSocket proxy bounds (Phase 6 closure
    /// Task 2) -- an authenticated client sending oversized frames must
    /// have the connection closed, not be allowed to force unbounded
    /// buffering.
    const MAX_RUNTIME_WS_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
    const MAX_RUNTIME_WS_FRAME_BYTES: usize = 1024 * 1024;

    pub(crate) fn require_runtime(state: &AppState) -> Result<&Arc<RuntimeManager>, ApiError> {
        state
            .runtime
            .as_ref()
            .ok_or_else(ApiError::runtime_unavailable)
    }

    /// Rejects anything that isn't a real, product-selectable runtime
    /// kind -- including a syntactically valid `"test_fixture"` outside
    /// of this crate's own test constructors (Task 2/15). Parse failure
    /// and "not selectable here" return the identical error, so the
    /// response can never be used to fingerprint whether the fixture
    /// kind exists on this build.
    fn parse_selectable_kind(state: &AppState, value: &str) -> Result<RuntimeKind, ApiError> {
        RuntimeKind::parse(value)
            .filter(|kind| kind.is_selectable() || state.runtime_allow_test_kind)
            .ok_or_else(|| ApiError::bad_request("unknown runtime kind"))
    }

    /// The existing RBAC capability that gates *using* an already-
    /// enabled runtime of this kind (reusing `apps.<kind>.use`, already
    /// seeded for the `user`/`manager` roles -- Task 3 explicitly
    /// prefers an existing matching namespace over inventing a parallel
    /// one). `None` for `TestFixture`: reachable only when
    /// `runtime_allow_test_kind` already gated it, so no separate
    /// product capability is meaningful for it.
    fn kind_capability(kind: RuntimeKind) -> Option<&'static str> {
        match kind {
            RuntimeKind::Code => Some("apps.code.use"),
            RuntimeKind::Office => Some("apps.office.use"),
            RuntimeKind::Browser => Some("apps.browser.use"),
            RuntimeKind::TestFixture => None,
        }
    }

    /// Trusted, compiled-in per-kind default -- never accepted from the
    /// request body. Code/Office instances keep a persistent per-user
    /// profile; Browser/the test fixture do not.
    fn default_persistence(kind: RuntimeKind) -> Persistence {
        match kind {
            RuntimeKind::Code | RuntimeKind::Office => Persistence::Persistent,
            RuntimeKind::Browser | RuntimeKind::TestFixture => Persistence::Ephemeral,
        }
    }

    pub(crate) fn map_start_error(error: StartError) -> ApiError {
        match error {
            StartError::UnknownKind | StartError::Unavailable(_) => ApiError::runtime_unavailable(),
            StartError::Disabled => ApiError::conflict("runtime is currently disabled"),
            StartError::PerUserLimitReached | StartError::GlobalLimitReached => {
                ApiError::too_many_requests("runtime instance limit reached; try again shortly")
            }
            StartError::StartTimeout => ApiError::bad_gateway("runtime failed to become ready"),
            // Not-owner is reported identically to not-found (Task 21):
            // an instance ID's owner is never disclosed to a caller who
            // isn't it.
            StartError::NotFound | StartError::NotOwner => {
                ApiError::not_found("runtime instance not found")
            }
            StartError::Adapter(e) => ApiError::internal(e.to_string()),
            StartError::Storage(e) => ApiError::internal(e.to_string()),
            StartError::Db(e) => ApiError::internal(e.to_string()),
        }
    }

    fn map_stop_error(error: StopError) -> ApiError {
        match error {
            StopError::NotFound | StopError::NotOwner => {
                ApiError::not_found("runtime instance not found")
            }
            StopError::Db(e) => ApiError::internal(e.to_string()),
        }
    }

    /// Strips ANSI/other control sequences (Task 12) and lossily
    /// decodes non-UTF-8 bytes, so runtime stdout/stderr can never
    /// smuggle terminal-control sequences or invalid UTF-8 into a JSON
    /// response. Newlines and tabs are preserved for readability.
    fn sanitize_log_text(raw: &[u8]) -> String {
        // Replacing a single hostile control byte with U+FFFD (3 bytes
        // in UTF-8) can make the sanitized string *larger* than the
        // raw input it came from -- re-enforcing the byte bound here
        // (rather than trusting the caller's already-bounded input) is
        // what actually keeps the API response bounded (Task 12
        // finding: a bound on the captured buffer alone doesn't bound
        // what sanitization can expand it into).
        let mut out = String::with_capacity(raw.len());
        for c in String::from_utf8_lossy(raw).chars() {
            let mapped = if c == '\n' || c == '\t' || !c.is_control() {
                c
            } else {
                '\u{FFFD}'
            };
            if out.len() + mapped.len_utf8() > MAX_RUNTIME_LOG_BYTES {
                break;
            }
            out.push(mapped);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    async fn audit_runtime_instance(
        auth: &AuthService,
        principal: &SessionPrincipal,
        action: &str,
        kind: RuntimeKind,
        instance_id: &str,
        result: &str,
        connect: SocketAddr,
        headers: &HeaderMap,
    ) -> Result<(), ApiError> {
        let (source_ip, user_agent) = request_metadata(connect, headers);
        auth.audit_action(
            principal,
            action,
            "runtime_instance",
            Some(instance_id.to_owned()),
            result,
            json!({ "kind": kind.as_str() }),
            &source_ip,
            &user_agent,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn list_kinds(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let _principal = super::principal(&state, &headers).await?;
        let runtime = require_runtime(&state)?;
        let mut kinds = Vec::new();
        for kind in RuntimeKind::all() {
            let kind = *kind;
            if !kind.is_selectable() && !state.runtime_allow_test_kind {
                continue;
            }
            let (available, detail) = match runtime.availability(kind).await {
                Availability::Available { detail } => (true, detail),
                Availability::Unavailable { reason } => (false, reason),
            };
            let enabled = runtime.is_enabled(kind).await.unwrap_or(false);
            let instance_count = runtime.store().list_all().await.map_or(0, |rows| {
                rows.iter()
                    .filter(|row| {
                        row.kind == kind
                            && !matches!(
                                row.state,
                                clouddesk_orchestrator::InstanceState::Stopped
                                    | clouddesk_orchestrator::InstanceState::Failed
                            )
                    })
                    .count()
            });
            kinds.push(json!({
                "kind": kind.as_str(),
                "available": available,
                "detail": detail,
                "enabled": enabled,
                "instance_count": instance_count,
            }));
        }
        Ok(Json(json!({ "runtimes": kinds })))
    }

    async fn set_enabled(
        state: &AppState,
        kind_str: &str,
        connect: SocketAddr,
        headers: &HeaderMap,
        target: bool,
    ) -> Result<StatusCode, ApiError> {
        let auth = super::require_auth_service(state)?;
        let principal = super::principal(state, headers).await?;
        super::authorize_request(auth, &principal, "runtime.admin", false, connect, headers)
            .await?;
        let kind = parse_selectable_kind(state, kind_str)?;
        let runtime = require_runtime(state)?;
        let (source_ip, user_agent) = request_metadata(connect, headers);
        auth.audit_action(
            &principal,
            if target {
                "runtime.enable.requested"
            } else {
                "runtime.disable.requested"
            },
            "runtime_kind",
            Some(kind.as_str().to_owned()),
            "success",
            json!({}),
            &source_ip,
            &user_agent,
        )
        .await?;
        runtime
            .set_enabled(kind, target)
            .await
            .map_err(map_start_error)?;
        auth.audit_action(
            &principal,
            if target {
                "runtime.enabled"
            } else {
                "runtime.disabled"
            },
            "runtime_kind",
            Some(kind.as_str().to_owned()),
            "success",
            json!({}),
            &source_ip,
            &user_agent,
        )
        .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    pub(crate) async fn enable(
        State(state): State<AppState>,
        Path(kind): Path<String>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        set_enabled(&state, &kind, connect, &headers, true).await
    }

    pub(crate) async fn disable(
        State(state): State<AppState>,
        Path(kind): Path<String>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        set_enabled(&state, &kind, connect, &headers, false).await
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct CreateInstanceBody {
        kind: String,
        /// Code only (Task 2 of the Phase 7 closure pass): the
        /// `assigned_roots.id` to open as this instance's workspace.
        /// Never a raw host path. Omitted -> reopen the user's
        /// last-used workspace, falling back to their home directory.
        #[serde(default)]
        workspace_id: Option<String>,
        /// Code only (Task 10 foundation, Files -> Code deep link): a
        /// workspace-relative file to additionally open. Validated
        /// server-side against the resolved workspace before use --
        /// never an absolute path taken from the client.
        #[serde(default)]
        open_relative_file: Option<String>,
        /// Code only (Task 10, Files -> Code deep link): an
        /// already-Files-authorized absolute host path (the same kind
        /// of value the Files app already validated belongs to this
        /// user's home or one of their assigned roots). The server
        /// alone determines which workspace contains it and derives
        /// the safe relative path -- this is never used as a raw mount
        /// target itself. Mutually exclusive with `workspace_id` /
        /// `open_relative_file`.
        #[serde(default)]
        open_absolute_path: Option<String>,
    }

    /// Stages the trusted identity/workspace marker Code's OCI closures
    /// read at start time (Task 10/11/15/2). Resolves and re-authorizes
    /// the workspace fresh every call -- this is the reauthorization
    /// point required on every new start, restart, and switch (Task
    /// 11): a revoked or deleted `workspace_id` here fails the request
    /// rather than silently mounting stale access.
    /// Task 10: given an absolute path the Files app already validated
    /// belongs to this user (home or one of their assigned roots),
    /// authoritatively determines -- server-side, never trusting a
    /// client-supplied workspace ID for this -- which workspace
    /// contains it and the safe relative path within it. Neither the
    /// user's home nor any assigned root is trusted merely because the
    /// path *looks* like it's inside one; each candidate is re-resolved
    /// (re-authorized) via `resolve_own_assigned_root` and the
    /// canonicalized real path is checked against the canonicalized
    /// candidate root.
    async fn resolve_deep_link_workspace(
        auth: &AuthService,
        principal: &SessionPrincipal,
        home: &str,
        absolute_path: &str,
    ) -> Result<(Option<String>, String), ApiError> {
        let canonical = tokio::fs::canonicalize(absolute_path)
            .await
            .map_err(|_| ApiError::bad_request("file not found"))?;

        // Security defect fixed during the Phase 7 closure pass: an
        // assigned root that happens to be *nested inside* the user's
        // own home (a realistic layout, not just a test artifact) used
        // to always lose to home, because home was checked first and
        // returned immediately. That silently widened a `read`-only
        // root's access to home's read-write default merely because
        // the file also happened to sit under home -- i.e. it could
        // upgrade a read-only file to a read-write mount through the
        // deep-link path alone. Fixed by evaluating every candidate
        // (home, plus every assigned root) and picking the *longest*
        // matching canonical prefix -- the most specific containing
        // root always wins, regardless of check order.
        let mut best: Option<(Option<String>, String, usize)> = None;
        if let Ok(relative) = canonical.strip_prefix(home) {
            best = Some((None, relative.to_string_lossy().into_owned(), home.len()));
        }
        for root in auth.list_own_assigned_roots(principal).await? {
            let Ok(resolved) = auth.resolve_own_assigned_root(principal, &root.id).await else {
                continue;
            };
            if let Ok(relative) = canonical.strip_prefix(&resolved.path) {
                let candidate_len = resolved.path.len();
                if best.as_ref().is_none_or(|(_, _, len)| candidate_len > *len) {
                    best = Some((
                        Some(resolved.id),
                        relative.to_string_lossy().into_owned(),
                        candidate_len,
                    ));
                }
            }
        }
        best.map(|(id, relative, _)| (id, relative))
            .ok_or_else(ApiError::forbidden)
    }

    #[allow(clippy::too_many_arguments)]
    async fn stage_code_marker(
        auth: &AuthService,
        principal: &SessionPrincipal,
        runtime: &RuntimeManager,
        id: &InstanceId,
        requested_workspace: Option<&str>,
        open_relative_file: Option<String>,
        open_absolute_path: Option<String>,
    ) -> Result<(), ApiError> {
        let identity = super::mapped_identity(auth, principal).await.map_err(|_| {
            ApiError::bad_request(
                "no valid Linux identity is mapped for this account; contact an administrator",
            )
        })?;
        let home = identity.home.to_string_lossy().into_owned();
        // Deep-link resolution (`open_absolute_path`) has already done
        // its own authoritative workspace match by the time it returns
        // -- including the "this file is under home itself" case,
        // which is *not* the same thing as "no workspace_id was
        // requested at all". Feeding that `None` back into
        // `resolve_workspace`'s generic `requested` parameter would
        // collide with its own "no explicit request -> infer the
        // user's last-used workspace" fallback (a real bug caught by
        // `task_1_deep_link_backend_resolution`'s cross-user case: a
        // deep-linked file that resolves to home could silently be
        // evaluated against a *different*, previously-selected
        // workspace instead). So the deep-link branch builds its
        // `ResolvedWorkspace` directly rather than going back through
        // the ambiguous generic path.
        let (workspace, open_relative_file) = if let Some(absolute) = open_absolute_path {
            let (workspace_id, relative) =
                resolve_deep_link_workspace(auth, principal, &home, &absolute).await?;
            let workspace = match workspace_id {
                None => crate::code_runtime::ResolvedWorkspace {
                    workspace_id: None,
                    path: home.clone(),
                    read_write: true,
                },
                Some(id) => {
                    crate::code_runtime::resolve_workspace(auth, principal, &home, Some(&id))
                        .await?
                }
            };
            (workspace, Some(relative))
        } else {
            let workspace =
                crate::code_runtime::resolve_workspace(auth, principal, &home, requested_workspace)
                    .await?;
            (workspace, open_relative_file)
        };
        // Task 10 foundation: the requested relative file must actually
        // live under the resolved workspace root -- never trust the
        // client-supplied relative path alone.
        if let Some(relative) = &open_relative_file {
            let candidate = std::path::Path::new(&workspace.path).join(relative);
            let canonical = tokio::fs::canonicalize(&candidate)
                .await
                .map_err(|_| ApiError::bad_request("file not found in workspace"))?;
            if !canonical.starts_with(&workspace.path) {
                return Err(ApiError::bad_request("file is outside the workspace"));
            }
        }
        let state_dir = runtime
            .instance_state_dir(id)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let marker = crate::code_runtime::CodeIdentityMarker {
            uid: identity.uid,
            gid: identity.gid,
            home,
            workspace_id: workspace.workspace_id,
            workspace_path: workspace.path,
            workspace_read_write: workspace.read_write,
            open_relative_file,
        };
        let marker_json =
            serde_json::to_vec(&marker).map_err(|e| ApiError::internal(e.to_string()))?;
        tokio::fs::write(
            state_dir.join(crate::code_runtime::CODE_IDENTITY_MARKER),
            marker_json,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(())
    }

    /// Code (Task 2): the per-user instance limit is 1 (a Code instance
    /// row is never deleted between uses -- it is reused across
    /// restarts), so "switch workspace" -- or simply reopening Code --
    /// must reuse that same instance/row rather than creating another
    /// one (which would immediately trip `PerUserLimitReached`). If a
    /// row already exists for this user, this stops it (idempotent
    /// no-op if it isn't currently live) and returns its ID for the
    /// caller to re-stage and `start_instance` again -- deliberately
    /// NOT `restart_instance`, whose crash-loop counter exists for
    /// genuine crash loops, not intentional user-driven workspace
    /// switches. `start_instance` alone still bumps the generation
    /// (Task 2 item 4: "new runtime generation/spec with new mount"),
    /// which is all a switch actually needs. Settings/extensions
    /// survive because they live on the separate, always-mounted
    /// profile directory, not the workspace mount.
    async fn existing_code_instance(
        runtime: &RuntimeManager,
        owner_user_id: &str,
    ) -> Option<InstanceId> {
        let existing = runtime.store().list_for_owner(owner_user_id).await.ok()?;
        let row = existing
            .into_iter()
            .find(|row| row.kind == RuntimeKind::Code)?;
        let id = InstanceId {
            kind: RuntimeKind::Code,
            owner_user_id: owner_user_id.to_owned(),
            instance_id: row.instance_id,
        };
        let _ = runtime.stop_instance(owner_user_id, &id).await;
        Some(id)
    }

    /// See Task 2 item 5's call site: only invoked after a successful
    /// `start_instance`.
    async fn persist_last_workspace(
        auth: &AuthService,
        runtime: &RuntimeManager,
        owner_user_id: &str,
        id: &InstanceId,
    ) -> Result<(), ApiError> {
        let marker_path = runtime
            .instance_state_dir(id)
            .map_err(|e| ApiError::internal(e.to_string()))?
            .join(crate::code_runtime::CODE_IDENTITY_MARKER);
        if let Some(marker) = tokio::fs::read(marker_path).await.ok().and_then(|raw| {
            serde_json::from_slice::<crate::code_runtime::CodeIdentityMarker>(&raw).ok()
        }) {
            let _ = crate::code_runtime::set_last_workspace_id(
                auth.pool(),
                owner_user_id,
                marker.workspace_id.as_deref(),
            )
            .await;
        }
        Ok(())
    }

    pub(crate) async fn create_instance(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
        Json(body): Json<CreateInstanceBody>,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        let kind = parse_selectable_kind(&state, &body.kind)?;
        if kind != RuntimeKind::Code
            && (body.workspace_id.is_some() || body.open_relative_file.is_some())
        {
            return Err(ApiError::bad_request(
                "workspace_id/open_relative_file only apply to kind=code",
            ));
        }
        if let Some(capability) = kind_capability(kind) {
            super::authorize_request(auth, &principal, capability, false, connect, &headers)
                .await?;
        }
        let runtime = require_runtime(&state)?;

        let reused_code_instance = if kind == RuntimeKind::Code {
            existing_code_instance(runtime, &principal.user_id).await
        } else {
            None
        };
        let id = if let Some(id) = reused_code_instance {
            id
        } else {
            runtime
                .create_instance(&principal.user_id, kind, default_persistence(kind))
                .await
                .map_err(map_start_error)?
        };

        // Code needs the caller's real mapped Linux identity and
        // resolved workspace staged before it starts, so the OCI
        // adapter's `run_as`/`extra_mounts`/`extra_env`/`command`
        // closures (which only see `InstanceContext`, not the
        // authenticated session) can run it as that user against that
        // workspace rather than the image's default user/folder (Task
        // 10/11/15/2 of the Phase 7 closure pass). Written as a small
        // trusted marker file in the instance's own state directory --
        // never a value taken directly from the request body without
        // server-side re-authorization.
        if kind == RuntimeKind::Code {
            stage_code_marker(
                auth,
                &principal,
                runtime,
                &id,
                body.workspace_id.as_deref(),
                body.open_relative_file.clone(),
                body.open_absolute_path.clone(),
            )
            .await?;
        }

        audit_runtime_instance(
            auth,
            &principal,
            "runtime.instance.start_requested",
            kind,
            &id.instance_id,
            "success",
            connect,
            &headers,
        )
        .await?;
        if let Err(error) = runtime.start_instance(&principal.user_id, &id).await {
            audit_runtime_instance(
                auth,
                &principal,
                "runtime.instance.failed",
                kind,
                &id.instance_id,
                "failure",
                connect,
                &headers,
            )
            .await?;
            return Err(map_start_error(error));
        }
        audit_runtime_instance(
            auth,
            &principal,
            "runtime.instance.started",
            kind,
            &id.instance_id,
            "success",
            connect,
            &headers,
        )
        .await?;

        // Task 2 item 5: persist the newly selected workspace as "last
        // used" only now that `start_instance` has returned success --
        // which (see `RuntimeManager::start_instance`) only happens
        // once the instance is actually confirmed `Running`. A failed
        // or still-starting selection never becomes the default.
        if kind == RuntimeKind::Code {
            persist_last_workspace(auth, runtime, &principal.user_id, &id).await?;
        }

        let state_now = runtime.status(&principal.user_id, &id).await;
        Ok(Json(json!({
            "kind": kind.as_str(),
            "instance_id": id.instance_id,
            "state": state_now.map_or("stopped", clouddesk_orchestrator::InstanceState::as_str),
        })))
    }

    /// Self-service (Task 2): lists only the caller's own authorized
    /// Code workspaces -- their `assigned_roots`, plus the always-
    /// available default (home). Never exposes raw host paths or
    /// another user's assignments.
    pub(crate) async fn list_code_workspaces(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        let roots = auth.list_own_assigned_roots(&principal).await?;
        let mut workspaces = vec![json!({
            "id": null,
            "label": "Home",
            "read_write": true,
            "default": true,
        })];
        for root in roots {
            workspaces.push(json!({
                "id": root.id,
                "label": root.label,
                "read_write": root.read_write,
                "default": false,
            }));
        }
        let runtime = require_runtime(&state)?;
        let last_workspace_id =
            crate::code_runtime::last_workspace_id(auth.pool(), &principal.user_id)
                .await
                .unwrap_or(None);
        let _ = runtime; // reserved: future availability-per-workspace enrichment
        Ok(Json(json!({
            "workspaces": workspaces,
            "last_workspace_id": last_workspace_id,
        })))
    }

    pub(crate) async fn list_instances(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        let runtime = require_runtime(&state)?;
        let rows = runtime
            .store()
            .list_for_owner(&principal.user_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let mut instances = Vec::new();
        for row in rows {
            if !row.kind.is_selectable() && !state.runtime_allow_test_kind {
                continue;
            }
            let id = InstanceId {
                kind: row.kind,
                owner_user_id: principal.user_id.clone(),
                instance_id: row.instance_id.clone(),
            };
            let live_state = runtime
                .status(&principal.user_id, &id)
                .await
                .unwrap_or(row.state);
            instances.push(json!({
                "kind": row.kind.as_str(),
                "instance_id": row.instance_id,
                "state": live_state.as_str(),
                "persistence": match row.persistence {
                    Persistence::Persistent => "persistent",
                    Persistence::Ephemeral => "ephemeral",
                },
                "created_at": row.created_at,
                "updated_at": row.updated_at,
                "restart_count": row.restart_count,
                "failure_message": row.failure_message,
            }));
            // Deliberately omitted: `port`/`pid` (Task 10 -- internal
            // coordinates are never part of the normal API response;
            // the proxy routes below are the only sanctioned path to
            // an instance's network surface).
        }
        Ok(Json(json!({ "instances": instances })))
    }

    fn instance_id_from_path(
        state: &AppState,
        kind_str: &str,
        instance_id: String,
        owner_user_id: String,
    ) -> Result<InstanceId, ApiError> {
        let kind = parse_selectable_kind(state, kind_str)?;
        Ok(InstanceId {
            kind,
            owner_user_id,
            instance_id,
        })
    }

    pub(crate) async fn instance_status(
        State(state): State<AppState>,
        Path((kind, instance_id)): Path<(String, String)>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        let id = instance_id_from_path(&state, &kind, instance_id, principal.user_id.clone())?;
        let runtime = require_runtime(&state)?;
        let state_now = runtime
            .status(&principal.user_id, &id)
            .await
            .ok_or_else(|| ApiError::not_found("runtime instance not found"))?;
        Ok(Json(json!({
            "kind": id.kind.as_str(),
            "instance_id": id.instance_id,
            "state": state_now.as_str(),
        })))
    }

    pub(crate) async fn stop_instance(
        State(state): State<AppState>,
        Path((kind, instance_id)): Path<(String, String)>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        let id = instance_id_from_path(&state, &kind, instance_id, principal.user_id.clone())?;
        let runtime = require_runtime(&state)?;
        runtime
            .stop_instance(&principal.user_id, &id)
            .await
            .map_err(map_stop_error)?;
        audit_runtime_instance(
            auth,
            &principal,
            "runtime.instance.stopped",
            id.kind,
            &id.instance_id,
            "success",
            connect,
            &headers,
        )
        .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    pub(crate) async fn restart_instance(
        State(state): State<AppState>,
        Path((kind, instance_id)): Path<(String, String)>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        let id = instance_id_from_path(&state, &kind, instance_id, principal.user_id.clone())?;
        let runtime = require_runtime(&state)?;

        // Task 11 (reauthorization): a restart must re-resolve the
        // user's workspace against their *current* assigned roots, not
        // blindly replay a marker written possibly long ago. Passing no
        // explicit `workspace_id` reuses the same "reopen last-used
        // workspace, falling back to home if it was deleted/revoked"
        // path as an implicit `create_instance` (see
        // `resolve_workspace`) -- a restart never hard-fails merely
        // because a previously-used workspace vanished.
        if id.kind == RuntimeKind::Code {
            stage_code_marker(auth, &principal, runtime, &id, None, None, None).await?;
        }

        match runtime.restart_instance(&principal.user_id, &id).await {
            Ok(()) => {
                audit_runtime_instance(
                    auth,
                    &principal,
                    "runtime.instance.started",
                    id.kind,
                    &id.instance_id,
                    "success",
                    connect,
                    &headers,
                )
                .await?;
            }
            Err(error) => {
                audit_runtime_instance(
                    auth,
                    &principal,
                    "runtime.instance.failed",
                    id.kind,
                    &id.instance_id,
                    "failure",
                    connect,
                    &headers,
                )
                .await?;
                return Err(map_start_error(error));
            }
        }
        let state_now = runtime.status(&principal.user_id, &id).await;
        Ok(Json(json!({
            "kind": id.kind.as_str(),
            "instance_id": id.instance_id,
            "state": state_now.map_or("stopped", clouddesk_orchestrator::InstanceState::as_str),
        })))
    }

    pub(crate) async fn instance_logs(
        State(state): State<AppState>,
        Path((kind, instance_id)): Path<(String, String)>,
        headers: HeaderMap,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        let id = instance_id_from_path(&state, &kind, instance_id, principal.user_id.clone())?;
        let runtime = require_runtime(&state)?;
        let raw = runtime
            .logs(&principal.user_id, &id, MAX_RUNTIME_LOG_BYTES)
            .await
            .ok_or_else(|| ApiError::not_found("runtime instance not found"))?;
        Ok(Json(json!({ "logs": sanitize_log_text(&raw) })))
    }

    /// The single authenticated, ownership-scoped HTTP proxy leg (Task
    /// 8). The only thing this handler derives from the request is the
    /// sub-path/query *within* the caller's own already-authorized
    /// instance -- the upstream host and port always come from
    /// `RuntimeManager::instance_port`, which is itself ownership-
    /// scoped; there is no parameter anywhere in this function that
    /// accepts a client-chosen host, port, scheme, or URL, which is
    /// what makes arbitrary-upstream SSRF structurally impossible here
    /// rather than merely untested.
    async fn http_proxy_inner(
        state: AppState,
        kind: String,
        instance_id: String,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> Result<Response, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        let id = instance_id_from_path(&state, &kind, instance_id, principal.user_id.clone())?;
        let runtime = require_runtime(&state)?;
        let prefix = format!(
            "/api/v1/runtime-instances/{}/{}/proxy",
            id.kind.as_str(),
            id.instance_id
        );
        let full = uri.path_and_query().map_or("/", |pq| pq.as_str());
        let upstream_path = full.strip_prefix(&prefix).unwrap_or("");
        let upstream_path = if upstream_path.is_empty() {
            "/"
        } else {
            upstream_path
        };
        Ok(proxy_http(
            runtime,
            &principal.user_id,
            &id,
            method,
            upstream_path,
            &headers,
            body.to_vec(),
        )
        .await?)
    }

    pub(crate) async fn http_proxy(
        State(state): State<AppState>,
        Path((kind, instance_id, _upstream_path)): Path<(String, String, String)>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> Result<Response, ApiError> {
        http_proxy_inner(state, kind, instance_id, method, uri, headers, body).await
    }

    /// Real defect fixed during the Phase 7 closure pass: axum's
    /// `{*upstream_path}` wildcard segment does not match a request
    /// whose path ends exactly at the route prefix (with or without a
    /// trailing slash) -- confirmed with a minimal standalone
    /// reproduction (`Router::new().route("/proxy/{*rest}", ...)`,
    /// requests to both `/proxy` and `/proxy/` returned 404 before
    /// ever reaching a handler). That is exactly the URL
    /// `CodeApp.svelte` uses as its iframe `src`
    /// (`.../proxy/`, nothing after it) -- so the Code IDE would never
    /// have loaded at all for a real user. This second route,
    /// registered for the bare prefix (both with and without a
    /// trailing slash), reuses the identical ownership-scoped proxy
    /// logic with an empty upstream path.
    pub(crate) async fn http_proxy_root(
        State(state): State<AppState>,
        Path((kind, instance_id)): Path<(String, String)>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> Result<Response, ApiError> {
        http_proxy_inner(state, kind, instance_id, method, uri, headers, body).await
    }

    /// The WebSocket counterpart of [`http_proxy`] -- same ownership
    /// scoping, same "no client-chosen upstream" guarantee (Task 9).
    pub(crate) async fn ws_proxy(
        websocket: WebSocketUpgrade,
        State(state): State<AppState>,
        Path((kind, instance_id)): Path<(String, String)>,
        headers: HeaderMap,
    ) -> Result<Response, ApiError> {
        let principal = super::principal(&state, &headers).await?;
        let id = instance_id_from_path(&state, &kind, instance_id, principal.user_id.clone())?;
        let runtime = require_runtime(&state)?.clone();
        let owner_user_id = principal.user_id.clone();
        // Explicit, deliberate bounds (Phase 6 closure Task 2) rather
        // than axum's library defaults (64 MiB message / 16 MiB frame)
        // -- an authenticated but hostile client must not be able to
        // force unbounded memory use through this proxy.
        Ok(websocket
            .max_message_size(MAX_RUNTIME_WS_MESSAGE_BYTES)
            .max_frame_size(MAX_RUNTIME_WS_FRAME_BYTES)
            .on_upgrade(move |socket| async move {
                proxy_ws(&runtime, &owner_user_id, &id, socket).await;
            })
            .into_response())
    }
}

/// Phase 8: `CloudDesk`'s own WOPI host HTTP surface, plus the Office
/// session-creation endpoint Files uses. Mirrors `runtime`'s split:
/// pure domain logic lives in `crate::wopi`, this module holds only the
/// axum handlers.
///
/// The four `/wopi/*` handlers deliberately never call `principal()`/
/// require a `CloudDesk` session cookie (Task 29) -- Collabora's own
/// server calls these directly, authenticating purely via the scoped
/// WOPI access token in the `access_token` query parameter. That token
/// is single-purpose: it is verified only by `crate::wopi::verify_token`
/// and can never substitute for a `CloudDesk` session on any other route
/// (Task 66's regression test proves this from the other direction).
pub(crate) mod wopi_api {
    use super::{
        request_metadata, ApiError, AppState, ConnectInfo, HeaderMap, Method, Path, State, Uri,
    };
    use axum::{
        body::Body,
        extract::{ws::WebSocketUpgrade, Query},
        http::StatusCode,
        response::{IntoResponse, Response},
        Json,
    };
    use clouddesk_auth::AuthService;
    use clouddesk_orchestrator::{
        proxy::{proxy_http, proxy_ws_path},
        InstanceId, Persistence, RuntimeKind,
    };
    use serde::Deserialize;
    use serde_json::json;
    use std::net::SocketAddr;

    /// Office file-size policy ceiling (Task 32) -- `CloudDesk` core stays
    /// lightweight; Office is optional/heavy, but even so a single
    /// document is bounded, not arbitrary.
    const MAX_OFFICE_FILE_BYTES: u64 = 200 * 1024 * 1024;

    fn wopi_error_to_api(e: crate::wopi::WopiError) -> ApiError {
        use crate::wopi::WopiError;
        match e {
            WopiError::NotAuthorized => ApiError::forbidden(),
            WopiError::NotFound => ApiError::not_found("file not found"),
            WopiError::InvalidToken | WopiError::TokenExpired | WopiError::FileMismatch => {
                ApiError::unauthorized()
            }
            WopiError::Database(e) => ApiError::internal(e.to_string()),
            WopiError::Io(e) => ApiError::internal(e.to_string()),
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OpenSessionBody {
        path: String,
        /// When set, `path` is a virtual path on this user-owned SFTP
        /// `RemoteServer` (Phase 8 remote-VFS closure, Task 1/2) rather
        /// than a local filesystem path.
        #[serde(default)]
        server_id: Option<String>,
    }

    #[derive(Deserialize)]
    pub(crate) struct TokenQuery {
        access_token: String,
    }

    /// Finds a real administrator user ID to use as the shared Office
    /// runtime instance's bookkeeping owner (Task 47: Office is modeled
    /// as one approved shared service, not a per-user instance like
    /// Code -- `runtime_instances.owner_user_id` still has a real
    /// foreign-key-checked value, but it carries no authorization
    /// meaning whatsoever: every actual document access is
    /// independently re-authorized per WOPI token, never via this
    /// owner field).
    async fn shared_owner(auth: &clouddesk_auth::AuthService) -> Result<String, ApiError> {
        sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM user_roles WHERE role_id = 'administrator' LIMIT 1",
        )
        .fetch_optional(auth.pool())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::bad_request("no administrator exists yet"))
    }

    /// Reuses a live shared Office instance if one exists, or creates
    /// and starts one -- never per-user, per Task 47. `Ephemeral`
    /// persistence: Collabora holds no `CloudDesk`-authoritative state of
    /// its own (no mounts at all), so there is nothing meaningful to
    /// persist across a stop.
    async fn ensure_office_instance(
        runtime: &clouddesk_orchestrator::RuntimeManager,
        owner_user_id: &str,
    ) -> Result<InstanceId, ApiError> {
        let existing = runtime
            .store()
            .list_for_owner(owner_user_id)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        // Only a row that can still be started is worth reusing. A
        // `Failed` instance is one whose container is gone (crashed, or
        // killed out from under us); reusing it hands back a session
        // pointing at a dead upstream, and since nothing ever clears the
        // row, Office would stay permanently broken for that owner.
        // Replace it with a fresh instance instead.
        let reusable = existing.into_iter().find(|r| {
            r.kind == RuntimeKind::Office
                && !matches!(
                    r.state,
                    clouddesk_orchestrator::InstanceState::Failed
                        | clouddesk_orchestrator::InstanceState::Unavailable
                )
        });
        if let Some(row) = reusable {
            let id = InstanceId {
                kind: RuntimeKind::Office,
                owner_user_id: owner_user_id.to_owned(),
                instance_id: row.instance_id,
            };
            // Idempotent: a no-op success if already Running/Starting.
            runtime
                .start_instance(owner_user_id, &id)
                .await
                .map_err(super::runtime::map_start_error)?;
            if office_instance_reachable(runtime, owner_user_id, &id).await {
                return Ok(id);
            }
            // The row looked healthy but the upstream is gone -- a
            // container killed out from under us, before the
            // orchestrator's own health sweep has reconciled the row.
            // Restart it in place: Office is a single shared instance, so
            // creating a second one would only hit the per-owner limit
            // and leave Office broken for everyone until something else
            // happened to notice.
            tracing::warn!(
                instance_id = %id.instance_id,
                "Office runtime unreachable despite a live instance row; restarting it"
            );
            runtime
                .restart_instance(owner_user_id, &id)
                .await
                .map_err(super::runtime::map_start_error)?;
            return Ok(id);
        }

        let id = runtime
            .create_instance(owner_user_id, RuntimeKind::Office, Persistence::Ephemeral)
            .await
            .map_err(super::runtime::map_start_error)?;
        runtime
            .start_instance(owner_user_id, &id)
            .await
            .map_err(super::runtime::map_start_error)?;
        Ok(id)
    }

    /// Whether the managed Collabora behind `id` actually answers. Uses
    /// its real discovery endpoint -- the same probe the session opener
    /// depends on -- rather than trusting the orchestrator's cached row
    /// state, which lags a container that died out of band.
    async fn office_instance_reachable(
        runtime: &clouddesk_orchestrator::RuntimeManager,
        owner_user_id: &str,
        id: &InstanceId,
    ) -> bool {
        let Some(port) = runtime.instance_port(owner_user_id, id).await else {
            return false;
        };
        crate::office_runtime::fetch_discovery(&format!("http://127.0.0.1:{port}"))
            .await
            .is_ok()
    }

    /// `Files -> Office`: authorizes `path` exactly like Code's deep
    /// link does (never a raw path handed onward -- Task 6/36/37),
    /// ensures the shared Office runtime is running, issues a scoped
    /// WOPI token, and returns a same-origin editor URL rewritten to go
    /// through `CloudDesk`'s own authenticated proxy rather than
    /// Collabora's raw address (Task 4/24/26).
    pub(crate) async fn open_session(
        State(state): State<AppState>,
        ConnectInfo(connect): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
        Json(body): Json<OpenSessionBody>,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let principal = super::principal(&state, &headers).await?;
        super::authorize_request(
            auth,
            &principal,
            "apps.office.use",
            false,
            connect,
            &headers,
        )
        .await?;
        let runtime = super::runtime::require_runtime(&state)?;

        let resolved = if let Some(server_id) = &body.server_id {
            let store = clouddesk_remote::RemoteServerStore::new(auth.pool().clone());
            crate::wopi::resolve_and_register_remote_file(
                auth.pool(),
                &store,
                &principal.user_id,
                server_id,
                &body.path,
            )
            .await
            .map_err(wopi_error_to_api)?
        } else {
            let identity = super::mapped_identity(auth, &principal).await?;
            let home = identity.home.to_string_lossy().into_owned();
            crate::wopi::resolve_and_register_file(
                auth.pool(),
                auth,
                &principal,
                &home,
                std::path::Path::new(&body.path),
            )
            .await
            .map_err(wopi_error_to_api)?
        };

        let owner = shared_owner(auth).await?;
        let id = ensure_office_instance(runtime, &owner).await?;
        let port = runtime
            .instance_port(&owner, &id)
            .await
            .ok_or_else(|| ApiError::bad_gateway("office runtime failed to become ready"))?;

        let token = crate::wopi::issue_token(
            auth.pool(),
            &principal.user_id,
            &resolved.file_id,
            resolved.read_write,
            &id.instance_id,
        )
        .await
        .map_err(wopi_error_to_api)?;

        let extension = resolved
            .canonical_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let discovery = crate::office_runtime::fetch_discovery(&format!("http://127.0.0.1:{port}"))
            .await
            .map_err(|e| ApiError::bad_gateway_owned(format!("discovery failed: {e:?}")))?;
        let action =
            crate::office_runtime::select_action(&discovery, extension, resolved.read_write)
                .ok_or_else(|| ApiError::bad_request("unsupported office format"))?;

        let proxy_prefix = format!(
            "/api/v1/runtime-instances/office/{}/office-proxy",
            id.instance_id
        );
        let action_path = crate::office_runtime::path_and_query(&action.urlsrc);
        let wopi_src = format!(
            "{}/wopi/files/{}",
            state
                .office_wopi_host_base
                .as_deref()
                .unwrap_or("http://host.docker.internal"),
            resolved.file_id
        );
        let separator = if action_path.contains('?') { '&' } else { '?' };
        let editor_url = format!(
            "{proxy_prefix}{action_path}{separator}WOPISrc={}&access_token={token}",
            urlencode(&wopi_src)
        );

        audit_office(
            auth,
            &principal,
            "office.session.opened",
            &resolved.file_id,
            connect,
            &headers,
        )
        .await?;

        Ok(Json(json!({
            "instance_id": id.instance_id,
            "file_id": resolved.file_id,
            "editor_url": editor_url,
            "read_write": resolved.read_write,
        })))
    }

    /// Minimal, dependency-free percent-encoder for the one query-value
    /// this module builds (`WOPISrc`) -- avoids adding a `url`/
    /// `percent-encoding` crate dependency for a single call site.
    fn urlencode(value: &str) -> String {
        use std::fmt::Write;
        let mut out = String::with_capacity(value.len());
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(byte as char);
                }
                _ => {
                    let _ = write!(out, "%{byte:02X}");
                }
            }
        }
        out
    }

    async fn audit_office(
        auth: &clouddesk_auth::AuthService,
        principal: &clouddesk_auth::SessionPrincipal,
        action: &str,
        file_id: &str,
        connect: SocketAddr,
        headers: &HeaderMap,
    ) -> Result<(), ApiError> {
        let (source_ip, user_agent) = request_metadata(connect, headers);
        // Never the raw path/token (Task 44) -- only the opaque file ID.
        auth.audit_action(
            principal,
            action,
            "office_file",
            Some(file_id.to_owned()),
            "success",
            json!({}),
            &source_ip,
            &user_agent,
        )
        .await?;
        Ok(())
    }

    /// WOPI `CheckFileInfo` (Task 9). Returns only the fields Collabora
    /// actually needs; never the absolute server path or any internal
    /// secret.
    pub(crate) async fn check_file_info(
        State(state): State<AppState>,
        Path(file_id): Path<String>,
        Query(query): Query<TokenQuery>,
    ) -> Result<Json<serde_json::Value>, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let verified =
            crate::wopi::verify_token(auth.pool(), auth, &query.access_token, Some(&file_id))
                .await
                .map_err(wopi_error_to_api)?;
        let (size, version) = if let Some(server_id) = &verified.remote_server_id {
            let vault = clouddesk_vault::Vault::new(auth.pool().clone(), auth.secret_cipher());
            let remote_path = verified.canonical_path.to_string_lossy().into_owned();
            let (size, mtime) = crate::wopi::remote::stat(
                auth.pool(),
                &vault,
                &verified.user_id,
                server_id,
                &remote_path,
            )
            .await
            .map_err(|_| ApiError::not_found("file not found"))?;
            let version =
                crate::wopi::current_version_from_stat(auth.pool(), &file_id, size, mtime)
                    .await
                    .map_err(wopi_error_to_api)?;
            (size, version)
        } else {
            let metadata = tokio::fs::metadata(&verified.canonical_path)
                .await
                .map_err(|_| ApiError::not_found("file not found"))?;
            let version =
                crate::wopi::current_version(auth.pool(), &file_id, &verified.canonical_path)
                    .await
                    .map_err(wopi_error_to_api)?;
            (metadata.len(), version)
        };
        let base_name = verified
            .canonical_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Json(json!({
            "BaseFileName": base_name,
            "Size": size,
            "Version": version,
            "OwnerId": verified.user_id,
            "UserId": verified.user_id,
            "UserFriendlyName": verified.user_id,
            "UserCanWrite": verified.read_write,
            "UserCanNotWriteRelative": true,
            "UserCanRename": false,
            "SupportsLocks": true,
            "SupportsGetLock": true,
            "SupportsUpdate": verified.read_write,
            "ReadOnly": !verified.read_write,
            "DisableExport": false,
            "DisablePrint": false,
            "DisableCopy": false,
        })))
    }

    /// WOPI `GetFile` (Task 10): streamed, bounded, freshly
    /// re-authorized on every call.
    pub(crate) async fn get_file(
        State(state): State<AppState>,
        Path(file_id): Path<String>,
        Query(query): Query<TokenQuery>,
    ) -> Result<Response, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let verified =
            crate::wopi::verify_token(auth.pool(), auth, &query.access_token, Some(&file_id))
                .await
                .map_err(wopi_error_to_api)?;
        if let Some(server_id) = &verified.remote_server_id {
            let vault = clouddesk_vault::Vault::new(auth.pool().clone(), auth.secret_cipher());
            let remote_path = verified.canonical_path.to_string_lossy().into_owned();
            let bytes = crate::wopi::remote::read(
                auth.pool(),
                &vault,
                &verified.user_id,
                server_id,
                &remote_path,
                usize::try_from(MAX_OFFICE_FILE_BYTES).unwrap_or(usize::MAX),
            )
            .await
            .map_err(|_| ApiError::not_found("file not found"))?;
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(bytes))
                .unwrap_or_else(|_| ApiError::internal("response build failed").into_response()));
        }
        let file = tokio::fs::File::open(&verified.canonical_path)
            .await
            .map_err(|_| ApiError::not_found("file not found"))?;
        let stream = tokio_util::io::ReaderStream::new(file);
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from_stream(stream))
            .unwrap_or_else(|_| ApiError::internal("stream build failed").into_response()))
    }

    /// WOPI `PutFile` (Task 11/12): requires write authorization, a
    /// valid lock for any existing non-empty file (matching real
    /// Collabora save behavior), rejects if the file changed out of
    /// band since the lock was acquired (Task 13/17), streams to a
    /// bounded temporary file, and atomically renames over the
    /// original -- the original is never truncated or touched until
    /// the new content has been fully and safely received.
    pub(crate) async fn put_file(
        State(state): State<AppState>,
        Path(file_id): Path<String>,
        Query(query): Query<TokenQuery>,
        headers: HeaderMap,
        body: Body,
    ) -> Result<Response, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let verified =
            crate::wopi::verify_token(auth.pool(), auth, &query.access_token, Some(&file_id))
                .await
                .map_err(wopi_error_to_api)?;
        if !verified.read_write {
            return Ok(StatusCode::FORBIDDEN.into_response());
        }

        if let Some(server_id) = verified.remote_server_id.clone() {
            return put_file_remote(auth, file_id, headers, body, verified, server_id).await;
        }

        let existing_size = tokio::fs::metadata(&verified.canonical_path)
            .await
            .map_or(0, |m| m.len());
        if existing_size > 0 {
            let lock = crate::wopi::get_lock(auth.pool(), &file_id)
                .await
                .map_err(wopi_error_to_api)?;
            let presented_lock = headers
                .get("X-WOPI-Lock")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            match &lock {
                Some(l) if l.lock_value == presented_lock => {}
                Some(l) => {
                    return Ok((
                        StatusCode::CONFLICT,
                        [("X-WOPI-Lock", l.lock_value.clone())],
                    )
                        .into_response());
                }
                None => return Ok(StatusCode::CONFLICT.into_response()),
            }
            // External-modification check (Task 13/17): the file must
            // still match what it was when this lock was acquired.
            if let Ok(Some((snap_size, snap_mtime))) =
                crate::wopi::lock_snapshot(auth.pool(), &file_id).await
            {
                if let Ok((live_size, live_mtime)) =
                    crate::wopi::stat_snapshot(&verified.canonical_path).await
                {
                    if (live_size, live_mtime) != (snap_size, snap_mtime) {
                        return Ok(StatusCode::CONFLICT.into_response());
                    }
                }
            }
        }

        let parent = verified
            .canonical_path
            .parent()
            .ok_or_else(|| ApiError::internal("file has no parent directory"))?;
        let tmp_path = parent.join(format!(
            ".cloudesk-office-{}.tmp",
            clouddesk_auth::random_identifier(8)
        ));
        let write_result = stream_to_bounded_file(&tmp_path, body).await;
        let bytes_written = match write_result {
            Ok(n) => n,
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(e);
            }
        };
        // Carry the original's permission bits onto the replacement
        // before it takes the original's place. `rename` replaces the
        // inode wholesale, so without this a document saved from Office
        // silently takes the daemon's umask default -- widening a
        // deliberately private 0600 document to 0644.
        if let Ok(metadata) = tokio::fs::metadata(&verified.canonical_path).await {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            if let Err(e) =
                tokio::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode)).await
            {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(ApiError::internal(e.to_string()));
            }
        }
        if let Err(e) = tokio::fs::rename(&tmp_path, &verified.canonical_path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(ApiError::internal(e.to_string()));
        }
        let _ = crate::wopi::bump_generation(auth.pool(), &file_id).await;
        // The lock's own snapshot must reflect the just-written state,
        // so a subsequent save in the same locked session compares
        // against what CloudDesk itself just wrote, not the pre-save
        // state.
        if let Ok((size, mtime)) = crate::wopi::stat_snapshot(&verified.canonical_path).await {
            let _ = sqlx::query(
                "UPDATE office_locks SET snapshot_size = ?, snapshot_mtime = ? WHERE file_id = ?",
            )
            .bind(i64::try_from(size).unwrap_or(i64::MAX))
            .bind(mtime)
            .bind(&file_id)
            .execute(auth.pool())
            .await;
        }
        let _ = bytes_written;
        Ok(StatusCode::OK.into_response())
    }

    /// The remote-VFS leg of `PutFile` (Phase 8 remote-VFS closure,
    /// Task 2/3/4): same lock/conflict/authorization rules as the local
    /// path, but the bytes go to a user-owned SFTP `RemoteServer`
    /// through `wopi::remote::write_safely` rather than `tokio::fs`.
    /// Collabora never sees the SSH credential -- it is resolved fresh
    /// from Vault, server-side, only here (Task 26).
    async fn put_file_remote(
        auth: &AuthService,
        file_id: String,
        headers: HeaderMap,
        body: Body,
        verified: crate::wopi::VerifiedToken,
        server_id: String,
    ) -> Result<Response, ApiError> {
        let remote_path = verified.canonical_path.to_string_lossy().into_owned();
        let vault = clouddesk_vault::Vault::new(auth.pool().clone(), auth.secret_cipher());

        let existing = crate::wopi::remote::stat(
            auth.pool(),
            &vault,
            &verified.user_id,
            &server_id,
            &remote_path,
        )
        .await
        .ok();
        if let Some((existing_size, _)) = existing {
            if existing_size > 0 {
                let lock = crate::wopi::get_lock(auth.pool(), &file_id)
                    .await
                    .map_err(wopi_error_to_api)?;
                let presented_lock = headers
                    .get("X-WOPI-Lock")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                match &lock {
                    Some(l) if l.lock_value == presented_lock => {}
                    Some(l) => {
                        return Ok((
                            StatusCode::CONFLICT,
                            [("X-WOPI-Lock", l.lock_value.clone())],
                        )
                            .into_response());
                    }
                    None => return Ok(StatusCode::CONFLICT.into_response()),
                }
                // Remote conflict-safety (Task 5): compare live remote
                // (size, mtime) against the lock's own snapshot -- the
                // same detection strategy as the local path, using SFTP's
                // best-available change signal (no ETag/version exists
                // for this provider).
                if let Ok(Some((snap_size, snap_mtime))) =
                    crate::wopi::lock_snapshot(auth.pool(), &file_id).await
                {
                    if let Ok((live_size, live_mtime)) = crate::wopi::remote::stat(
                        auth.pool(),
                        &vault,
                        &verified.user_id,
                        &server_id,
                        &remote_path,
                    )
                    .await
                    {
                        if (live_size, live_mtime) != (snap_size, snap_mtime) {
                            return Ok(StatusCode::CONFLICT.into_response());
                        }
                    }
                }
            }
        }

        let content = match collect_bounded_body(body).await {
            Ok(bytes) => bytes,
            Err(e) => return Err(e),
        };

        if let Err(e) = crate::wopi::remote::write_safely(
            auth.pool(),
            &vault,
            &verified.user_id,
            &server_id,
            &remote_path,
            content,
        )
        .await
        {
            return Ok(wopi_error_to_api(e).into_response());
        }

        let _ = crate::wopi::bump_generation(auth.pool(), &file_id).await;
        if let Ok((size, mtime)) = crate::wopi::remote::stat(
            auth.pool(),
            &vault,
            &verified.user_id,
            &server_id,
            &remote_path,
        )
        .await
        {
            let _ = sqlx::query(
                "UPDATE office_locks SET snapshot_size = ?, snapshot_mtime = ? WHERE file_id = ?",
            )
            .bind(i64::try_from(size).unwrap_or(i64::MAX))
            .bind(mtime)
            .bind(&file_id)
            .execute(auth.pool())
            .await;
        }
        Ok(StatusCode::OK.into_response())
    }

    /// Reads a request body into memory, bounded by
    /// `MAX_OFFICE_FILE_BYTES` -- used for the remote-VFS save path,
    /// which has no local temp-file staging area to stream into first.
    async fn collect_bounded_body(body: Body) -> Result<Vec<u8>, ApiError> {
        use futures_util::StreamExt;
        let mut buffer = Vec::new();
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ApiError::bad_request("upload read error"))?;
            buffer.extend_from_slice(&chunk);
            if buffer.len() as u64 > MAX_OFFICE_FILE_BYTES {
                return Err(ApiError::bad_request(
                    "file exceeds the configured Office size policy",
                ));
            }
        }
        Ok(buffer)
    }

    async fn stream_to_bounded_file(path: &std::path::Path, body: Body) -> Result<u64, ApiError> {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::File::create(path)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let mut written: u64 = 0;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ApiError::bad_request("upload read error"))?;
            written += chunk.len() as u64;
            if written > MAX_OFFICE_FILE_BYTES {
                return Err(ApiError::bad_request(
                    "file exceeds the configured Office size policy",
                ));
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
        }
        // sync_all, not just flush: flush only drains userspace buffers,
        // so a host crash between the rename and the kernel's own
        // writeback could publish a rename to data that never reached
        // disk. The document must be durable before it replaces the
        // original (Task 2/12).
        file.sync_all()
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(written)
    }

    /// Dispatches `LOCK`/`UNLOCK`/`REFRESH_LOCK`/`GET_LOCK` per the
    /// `X-WOPI-Override` header (Task 14/15/16), following the real
    /// WOPI lock header semantics: the caller's `X-WOPI-Lock` value must
    /// match the currently held lock for anything except acquiring a
    /// fresh one; a conflict always echoes the *current* lock value back
    /// in the response's own `X-WOPI-Lock` header, exactly as the WOPI
    /// spec (and real Collabora, verified live) expects.
    pub(crate) async fn file_operation(
        State(state): State<AppState>,
        Path(file_id): Path<String>,
        Query(query): Query<TokenQuery>,
        headers: HeaderMap,
    ) -> Result<Response, ApiError> {
        let auth = super::require_auth_service(&state)?;
        let verified =
            crate::wopi::verify_token(auth.pool(), auth, &query.access_token, Some(&file_id))
                .await
                .map_err(wopi_error_to_api)?;
        let Some(override_header) = headers
            .get("X-WOPI-Override")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
        else {
            return Ok(StatusCode::BAD_REQUEST.into_response());
        };
        let presented_lock = headers
            .get("X-WOPI-Lock")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        // Task 15: the WOPI spec caps lock identifiers at 1024 chars.
        // Without this bound an attacker with any valid token could
        // persist arbitrarily large strings into `office_locks`, so
        // refuse oversized values outright rather than storing them.
        if presented_lock.len() > MAX_WOPI_LOCK_BYTES {
            return Ok(StatusCode::BAD_REQUEST.into_response());
        }

        match override_header.as_str() {
            "LOCK" => {
                if !verified.read_write {
                    return Ok(StatusCode::FORBIDDEN.into_response());
                }
                let current = crate::wopi::get_lock(auth.pool(), &file_id)
                    .await
                    .map_err(wopi_error_to_api)?;
                match current {
                    None => {
                        crate::wopi::acquire_lock(
                            auth.pool(),
                            &file_id,
                            &presented_lock,
                            &verified.user_id,
                            &verified.canonical_path,
                        )
                        .await
                        .map_err(wopi_error_to_api)?;
                        Ok(StatusCode::OK.into_response())
                    }
                    Some(l) if l.lock_value == presented_lock => {
                        // Idempotent re-LOCK with the same value refreshes it.
                        crate::wopi::refresh_lock(auth.pool(), &file_id)
                            .await
                            .map_err(wopi_error_to_api)?;
                        Ok(StatusCode::OK.into_response())
                    }
                    Some(l) => {
                        Ok((StatusCode::CONFLICT, [("X-WOPI-Lock", l.lock_value)]).into_response())
                    }
                }
            }
            "REFRESH_LOCK" => {
                let current = crate::wopi::get_lock(auth.pool(), &file_id)
                    .await
                    .map_err(wopi_error_to_api)?;
                match current {
                    Some(l) if l.lock_value == presented_lock => {
                        crate::wopi::refresh_lock(auth.pool(), &file_id)
                            .await
                            .map_err(wopi_error_to_api)?;
                        Ok(StatusCode::OK.into_response())
                    }
                    Some(l) => {
                        Ok((StatusCode::CONFLICT, [("X-WOPI-Lock", l.lock_value)]).into_response())
                    }
                    None => Ok(StatusCode::NOT_FOUND.into_response()),
                }
            }
            "UNLOCK" => {
                let current = crate::wopi::get_lock(auth.pool(), &file_id)
                    .await
                    .map_err(wopi_error_to_api)?;
                match current {
                    Some(l) if l.lock_value == presented_lock => {
                        crate::wopi::release_lock(auth.pool(), &file_id)
                            .await
                            .map_err(wopi_error_to_api)?;
                        Ok(StatusCode::OK.into_response())
                    }
                    Some(l) => {
                        Ok((StatusCode::CONFLICT, [("X-WOPI-Lock", l.lock_value)]).into_response())
                    }
                    None => Ok(StatusCode::CONFLICT.into_response()),
                }
            }
            "GET_LOCK" => {
                let current = crate::wopi::get_lock(auth.pool(), &file_id)
                    .await
                    .map_err(wopi_error_to_api)?;
                let value = current.map(|l| l.lock_value).unwrap_or_default();
                Ok((StatusCode::OK, [("X-WOPI-Lock", value)]).into_response())
            }
            "RENAME_FILE" => {
                // Task 39: rename is never advertised/supported unless
                // CloudDesk VFS authorizes it -- v1 does not, so this is
                // an explicit, honest rejection, not a silently ignored
                // no-op.
                Ok(StatusCode::NOT_IMPLEMENTED.into_response())
            }
            _ => Ok(StatusCode::BAD_REQUEST.into_response()),
        }
    }

    /// The WOPI specification's own documented ceiling for a lock
    /// identifier. Anything longer is refused rather than persisted
    /// (Task 15).
    const MAX_WOPI_LOCK_BYTES: usize = 1024;

    const MAX_OFFICE_WS_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
    const MAX_OFFICE_WS_FRAME_BYTES: usize = 1024 * 1024;

    /// Office's own proxy leg, deliberately separate from the generic
    /// per-owner `runtime::http_proxy*`/`ws_proxy` Code uses: Office is
    /// a *shared* runtime (Task 47) -- its `runtime_instances` row is
    /// owned, for bookkeeping only, by a fixed administrator (see
    /// `shared_owner`), never by the actual authenticated user reaching
    /// it. Reusing the ownership-scoped generic proxy here would 404
    /// for every real user except that one administrator (found live,
    /// not assumed: `task_58_real_collabora_driven_wopi_callback`
    /// caught this exact defect). Authorization here is instead the
    /// `apps.office.use` capability plus a live `CloudDesk` session --
    /// document-level authorization was already independently enforced
    /// when the WOPI token was issued and is re-checked on every WOPI
    /// call regardless of this proxy leg.
    async fn office_proxy_owner(
        state: &AppState,
        headers: &HeaderMap,
    ) -> Result<(clouddesk_auth::SessionPrincipal, String), ApiError> {
        let auth = super::require_auth_service(state)?;
        let principal = super::principal(state, headers).await?;
        if !principal.can("apps.office.use") {
            return Err(ApiError::forbidden());
        }
        let owner = shared_owner(auth).await?;
        Ok((principal, owner))
    }

    async fn office_http_proxy_inner(
        state: AppState,
        instance_id: String,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> Result<Response, ApiError> {
        let (_principal, owner) = office_proxy_owner(&state, &headers).await?;
        let runtime = super::runtime::require_runtime(&state)?;
        let id = InstanceId {
            kind: RuntimeKind::Office,
            owner_user_id: owner.clone(),
            instance_id,
        };
        let prefix = format!(
            "/api/v1/runtime-instances/office/{}/office-proxy",
            id.instance_id
        );
        let full = uri.path_and_query().map_or("/", |pq| pq.as_str());
        let upstream_path = full.strip_prefix(&prefix).unwrap_or("");
        let upstream_path = if upstream_path.is_empty() {
            "/"
        } else {
            upstream_path
        };
        Ok(proxy_http(
            runtime,
            &owner,
            &id,
            method,
            upstream_path,
            &headers,
            body.to_vec(),
        )
        .await?)
    }

    pub(crate) async fn office_http_proxy(
        State(state): State<AppState>,
        Path((instance_id, _upstream_path)): Path<(String, String)>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> Result<Response, ApiError> {
        office_http_proxy_inner(state, instance_id, method, uri, headers, body).await
    }

    pub(crate) async fn office_http_proxy_root(
        State(state): State<AppState>,
        Path(instance_id): Path<String>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> Result<Response, ApiError> {
        office_http_proxy_inner(state, instance_id, method, uri, headers, body).await
    }

    pub(crate) async fn office_ws_proxy(
        websocket: WebSocketUpgrade,
        state: State<AppState>,
        instance_id: Path<String>,
        headers: HeaderMap,
    ) -> Result<Response, ApiError> {
        office_ws_proxy_inner(websocket, state, instance_id.0, "/ws".to_owned(), headers).await
    }

    /// Real Collabora's WebSocket endpoint is per-document and
    /// per-session (`/cool/{docKey}/ws?WOPISrc=...&access_token=...`,
    /// constructed client-side from the editor bootstrap page it just
    /// loaded through `office_http_proxy`) -- not a fixed path the way
    /// code-server's is. This route accepts that real upstream path so
    /// the browser's own WebSocket URL is honoured verbatim, rather than
    /// forcing every Office WS connection through a `/ws` path
    /// Collabora never serves.
    pub(crate) async fn office_ws_proxy_path(
        websocket: WebSocketUpgrade,
        state: State<AppState>,
        Path((instance_id, _upstream_path)): Path<(String, String)>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Result<Response, ApiError> {
        let prefix = format!("/api/v1/runtime-instances/office/{instance_id}/office-proxy-ws");
        let full = uri.path_and_query().map_or("/ws", |pq| pq.as_str());
        let upstream_path = full.strip_prefix(&prefix).unwrap_or("/ws").to_owned();
        office_ws_proxy_inner(websocket, state, instance_id, upstream_path, headers).await
    }

    async fn office_ws_proxy_inner(
        websocket: WebSocketUpgrade,
        State(state): State<AppState>,
        instance_id: String,
        upstream_path: String,
        headers: HeaderMap,
    ) -> Result<Response, ApiError> {
        let (_principal, owner) = office_proxy_owner(&state, &headers).await?;
        let runtime = super::runtime::require_runtime(&state)?.clone();
        let id = InstanceId {
            kind: RuntimeKind::Office,
            owner_user_id: owner.clone(),
            instance_id,
        };
        Ok(websocket
            .max_message_size(MAX_OFFICE_WS_MESSAGE_BYTES)
            .max_frame_size(MAX_OFFICE_WS_FRAME_BYTES)
            .on_upgrade(move |socket| async move {
                proxy_ws_path(&runtime, &owner, &id, &upstream_path, socket).await;
            })
            .into_response())
    }
}

pub mod code_runtime;
pub mod office_runtime;
pub mod wopi;
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
