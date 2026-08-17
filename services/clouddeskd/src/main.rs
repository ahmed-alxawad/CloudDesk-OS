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
        clouddeskd::application_router_with_privilege_configured(
            static_dir,
            auth,
            bootstrap_secret,
            privilege,
            !config.server.development_http,
        )
    } else {
        tracing::warn!("privileged helper integration is disabled");
        clouddeskd::application_router_configured(
            static_dir,
            auth,
            bootstrap_secret,
            !config.server.development_http,
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
