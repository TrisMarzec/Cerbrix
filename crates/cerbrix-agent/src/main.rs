use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use cerbrix_agent::agent::{self, NotEnrolled};
use cerbrix_agent::config::AgentConfig;
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "cerbrix-agent", version, about = "Cerbrix monitoring agent")]
struct Cli {
    /// Path to the TOML configuration file (default: /etc/cerbrix/agent.toml).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = match AgentConfig::load(cli.config.as_deref()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("configuration error: {err}");
            return ExitCode::from(2);
        }
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: cannot start the async runtime: {err}");
            return ExitCode::from(1);
        }
    };
    let result = runtime.block_on(async {
        let shutdown = CancellationToken::new();
        let on_signal = shutdown.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            tracing::info!("shutdown requested");
            on_signal.cancel();
        });
        agent::run(config, shutdown).await
    });
    // A collector stuck in a blocking call must not keep the process alive.
    runtime.shutdown_timeout(Duration::from_secs(5));

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.downcast_ref::<NotEnrolled>().is_some() => {
            eprintln!("configuration error: {err}");
            ExitCode::from(2)
        }
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}

/// Resolves on Ctrl-C or, on Unix, SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
