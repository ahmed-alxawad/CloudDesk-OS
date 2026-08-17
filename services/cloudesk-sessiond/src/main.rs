use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "cloudesk-sessiond",
    version,
    about = "CloudDesk per-user worker"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Verify and report the mapped Linux identity.
    Identity {
        #[arg(long)]
        expected_uid: u32,
        #[arg(long)]
        expected_gid: u32,
    },
    /// Execute one typed operation inside a capability-scoped local root.
    Files {
        #[arg(long)]
        expected_uid: u32,
        #[arg(long)]
        expected_gid: u32,
        #[arg(long)]
        root: PathBuf,
        #[arg(long, default_value_t = false)]
        writable: bool,
        #[arg(long)]
        operation: String,
    },
    /// Serve one interactive PTY over a protected Unix socket.
    Terminal {
        #[arg(long)]
        expected_uid: u32,
        #[arg(long)]
        expected_gid: u32,
        #[arg(long)]
        socket: PathBuf,
        #[arg(long, default_value_t = 24)]
        rows: u16,
        #[arg(long, default_value_t = 80)]
        cols: u16,
        #[arg(long)]
        shell: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Identity {
            expected_uid,
            expected_gid,
        } => {
            let identity = cloudesk_sessiond::identity_snapshot(expected_uid, expected_gid)?;
            println!("{}", serde_json::to_string(&identity)?);
        }
        Command::Files {
            expected_uid,
            expected_gid,
            root,
            writable,
            operation,
        } => {
            let operation = serde_json::from_str(&operation)?;
            let result = cloudesk_sessiond::execute_file_operation(
                expected_uid,
                expected_gid,
                &root,
                writable,
                &operation,
            )?;
            println!("{}", serde_json::to_string(&result)?);
        }
        Command::Terminal {
            expected_uid,
            expected_gid,
            socket,
            rows,
            cols,
            shell,
        } => {
            cloudesk_sessiond::serve_terminal(
                expected_uid,
                expected_gid,
                &socket,
                rows,
                cols,
                shell.as_deref(),
            )
            .await?;
        }
    }
    Ok(())
}
