use std::{fs, path::PathBuf};

use anyhow::Context;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "cloudesk-privd",
    version,
    about = "CloudDesk narrow privileged helper"
)]
struct Cli {
    #[arg(long, default_value = "/run/clouddesk/privd.sock")]
    socket: PathBuf,
    #[arg(long, default_value = "/etc/clouddesk/keys/privd-grant.key")]
    grant_key: PathBuf,
    #[arg(long)]
    allowed_peer_uid: u32,
    #[arg(long)]
    socket_gid: u32,
    #[arg(long, default_value = "/opt/clouddesk/bin/cloudesk-sessiond")]
    sessiond: PathBuf,
    #[arg(long, default_value = "/usr/bin/setpriv")]
    setpriv: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    if rustix::process::geteuid().as_raw() != 0 {
        anyhow::bail!("cloudesk-privd must run as root");
    }
    let cli = Cli::parse();
    let key = fs::read(&cli.grant_key)
        .with_context(|| format!("failed to read {}", cli.grant_key.display()))?;
    cloudesk_privd::run(
        cloudesk_privd::PrivdConfig {
            socket_path: cli.socket,
            socket_gid: cli.socket_gid,
            allowed_peer_uid: cli.allowed_peer_uid,
            sessiond_path: cli.sessiond,
            setpriv_path: cli.setpriv,
        },
        &key,
    )
    .await
}
