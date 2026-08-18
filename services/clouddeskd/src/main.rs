use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context;
use axum_server::tls_rustls::RustlsConfig;
use clap::{Parser, Subcommand};
use clouddesk_config::Config;
use clouddesk_secrets::SecretCipher;
use clouddeskd::security::require_unprivileged;
use tracing_subscriber::EnvFilter;
use zeroize::Zeroizing;

#[derive(Debug, Parser)]
#[command(name = "cloudeskd", version, about = "CloudDesk-OS core service")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the API and static web service.
    Serve {
        #[arg(long, default_value = "config/clouddesk.toml")]
        config: PathBuf,
    },
    /// Apply all pending `SQLite` migrations.
    Migrate {
        #[arg(long, default_value = "config/clouddesk.toml")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    require_unprivileged(rustix::process::geteuid().as_raw())?;

    match Cli::parse().command {
        Command::Serve { config } => serve(config).await,
        Command::Migrate { config } => migrate(config).await,
    }
}

#[allow(clippy::too_many_lines)]
async fn serve(config_path: PathBuf) -> anyhow::Result<()> {
    let config = Config::load(&config_path)
        .with_context(|| format!("failed to load {}", config_path.display()))?;

    let address = SocketAddr::new(config.server.address, config.server.port);
    let pool = clouddesk_db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("failed to open SQLite database")?;
    clouddesk_db::migrate(&pool)
        .await
        .context("failed to apply SQLite migrations")?;
    let cipher = SecretCipher::load(&config.security.master_key)?;
    let auth =
        clouddesk_auth::AuthService::new(pool, cipher, clouddesk_auth::AuthPolicy::default())?;
    auth.seed_authorization_model().await?;
    clouddeskd::worker::TransferWorker::new(&auth).spawn();
    clouddeskd::spawn_upload_session_janitor(auth.pool().clone());

    let media_cache_dir: PathBuf = config.media.cache_dir.into();
    let media_enabled = sqlx::query_scalar::<_, String>(
        "SELECT value_json FROM system_settings WHERE key = 'runtime.media.enabled'",
    )
    .fetch_optional(auth.pool())
    .await
    .ok()
    .flatten()
    .and_then(|value| serde_json::from_str::<bool>(&value).ok())
    .unwrap_or(false);
    let media_availability = clouddesk_media::ffmpeg::detect(media_enabled).await;
    tracing::info!(?media_availability, "media/FFmpeg availability detected");
    let media_service = clouddesk_media::MediaService::new(
        media_availability,
        auth.pool().clone(),
        media_cache_dir.clone(),
    );
    // Reconcile any job rows left non-terminal by a previous process that
    // no longer exists (crash/SIGKILL/restart) before accepting traffic.
    let reconciled = clouddesk_media::cleanup_abandoned_jobs(
        media_service.store(),
        &media_cache_dir,
        unix_now(),
    )
    .await
    .unwrap_or(0);
    if reconciled > 0 {
        tracing::warn!(
            count = reconciled,
            "expired abandoned media jobs at startup"
        );
    }

    let library_store = clouddesk_library::LibraryStore::new(auth.pool().clone());

    // Phase 6 optional-runtime orchestrator (Code/Office/Browser).
    // Deliberately registers zero adapters here -- no real Code/Office/
    // Browser adapter exists yet (Phase 7/8/9), so every kind reports
    // `Unavailable` cleanly rather than clouddeskd failing to start
    // (Task 36: a fresh low-resource install never requires these).
    // `RuntimeKind::TestFixture` is never constructed or registered in
    // this production path.
    let runtime_state_dir: PathBuf = config.runtime.state_dir.clone().into();
    let runtime_store = clouddesk_orchestrator::store::RuntimeStore::new(auth.pool().clone());
    let runtime_manager = std::sync::Arc::new(clouddesk_orchestrator::RuntimeManager::new(
        runtime_store,
        runtime_state_dir,
        clouddesk_orchestrator::ResourcePolicy::default(),
    ));
    // Startup reconciliation (Task 16/27): correct any instance rows
    // left non-terminal by a previous process that no longer exists.
    // With zero adapters registered this is currently a no-op in
    // practice, but runs unconditionally so it is exercised the moment
    // a real adapter is registered in a future phase.
    match runtime_manager.reconcile_on_startup().await {
        Ok(count) if count > 0 => {
            tracing::warn!(count, "reconciled stale runtime instance rows at startup");
        }
        Ok(_) => {}
        Err(error) => {
            tracing::error!(%error, "runtime orchestrator startup reconciliation failed");
        }
    }
    runtime_manager.spawn_idle_sweeper();

    let static_dir = config.web.static_dir.into();
    let bootstrap_secret = config.security.bootstrap_secret.into();

    let app = if config.privilege.enabled {
        let grant_key =
            Zeroizing::new(std::fs::read(&config.privilege.grant_key).with_context(|| {
                format!(
                    "failed to read privilege grant key {}",
                    config.privilege.grant_key
                )
            })?);
        let privilege =
            clouddeskd::PrivilegeClient::new(grant_key.as_slice(), config.privilege.socket.into())?;
        clouddeskd::application_router_with_privilege_and_media_and_library_and_runtime_configured(
            static_dir,
            auth,
            bootstrap_secret,
            privilege,
            !config.server.development_http,
            Some(media_service),
            Some(library_store),
            Some(runtime_manager),
        )
    } else {
        tracing::warn!("privileged helper integration is disabled");
        clouddeskd::application_router_and_media_and_library_and_runtime_configured(
            static_dir,
            auth,
            bootstrap_secret,
            !config.server.development_http,
            Some(media_service),
            Some(library_store),
            Some(runtime_manager),
        )
    };

    if config.server.development_http {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .with_context(|| format!("failed to bind {address}"))?;
        tracing::warn!(%address, "development HTTP listener started; do not use in production");
        clouddeskd::serve(listener, app).await?;
    } else {
        let tls = RustlsConfig::from_pem_file(&config.tls.certificate, &config.tls.private_key)
            .await
            .context("failed to load TLS certificate or private key")?;
        tracing::info!(%address, "cloudeskd HTTPS listener started");
        axum_server::bind_rustls(address, tls)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    }
    Ok(())
}

fn unix_now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(0)
}

async fn migrate(config_path: PathBuf) -> anyhow::Result<()> {
    let config = Config::load(&config_path)
        .with_context(|| format!("failed to load {}", config_path.display()))?;
    let pool = clouddesk_db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("failed to open SQLite database")?;
    clouddesk_db::migrate(&pool)
        .await
        .context("failed to apply SQLite migrations")?;
    tracing::info!(database = %config.database.url, "database migrations complete");
    Ok(())
}
