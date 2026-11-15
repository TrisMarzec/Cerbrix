//! Server start-up: wires the stores into the router and serves it over HTTPS
//! (operator-supplied certificate) or, for development, plain HTTP.

use std::future::Future;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::http::StatusCode;
use axum_server::Handle;
use axum_server::tls_rustls::RustlsConfig;
use tokio_util::sync::CancellationToken;
use tower_http::timeout::TimeoutLayer;

use crate::api::{AppState, router};
use crate::config::Config;
use crate::gitops::{GitSource, run_sync_loop};
use crate::metrics::VictoriaMetrics;
use crate::state::StateStore;

/// Upper bound for one request, body included, so that a client trickling a body
/// cannot hold a connection forever.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long in-flight requests get to finish after a shutdown signal.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// Runs the server on `listener` until `shutdown` completes, then stops gracefully.
pub async fn serve(
    config: Config,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    install_crypto_provider();

    // Load the certificate first so a bad file stops the start before any connection is made.
    let tls = match (&config.server.tls_cert, &config.server.tls_key) {
        (Some(cert), Some(key)) => Some(load_tls(cert, key).await?),
        (Some(_), None) => anyhow::bail!("server.tls_cert is set but server.tls_key is missing"),
        (None, Some(_)) => anyhow::bail!("server.tls_key is set but server.tls_cert is missing"),
        _ if config.server.insecure_http => {
            tracing::warn!(
                "serving plain HTTP; use only for development or behind a TLS-terminating reverse proxy"
            );
            None
        }
        _ => anyhow::bail!("no TLS certificate configured and server.insecure_http is not enabled"),
    };

    let store = StateStore::connect(&config.database.url)
        .await
        .context("cannot connect to the state database")?;
    let metrics = VictoriaMetrics::new(&config.metrics.url).context("invalid metrics.url")?;
    let shutdown_token = CancellationToken::new();
    let sync_task = config.gitops.enabled.then(|| {
        tracing::info!(
            interval_secs = config.gitops.interval_secs,
            "Git sync enabled"
        );
        tokio::spawn(run_sync_loop(
            store.clone(),
            Arc::new(GitSource::new(&config.gitops)),
            config.gitops.clone(),
            shutdown_token.clone(),
        ))
    });
    let state = AppState {
        store,
        metrics: Arc::new(metrics),
        config: Arc::new(config),
    };
    let app = router(state).layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        REQUEST_TIMEOUT,
    ));

    let handle = Handle::new();
    let signal_handle = handle.clone();
    let signal_token = shutdown_token.clone();
    tokio::spawn(async move {
        shutdown.await;
        signal_token.cancel();
        signal_handle.graceful_shutdown(Some(SHUTDOWN_GRACE));
    });

    let served: anyhow::Result<()> = async {
        let addr = listener
            .local_addr()
            .context("cannot read listen address")?;
        match tls {
            Some(tls) => {
                tracing::info!(%addr, "listening (HTTPS)");
                axum_server::from_tcp_rustls(listener, tls)?
                    .handle(handle)
                    .serve(app.into_make_service())
                    .await?;
            }
            None => {
                tracing::info!(%addr, "listening (HTTP)");
                axum_server::from_tcp(listener)?
                    .handle(handle)
                    .serve(app.into_make_service())
                    .await?;
            }
        }
        Ok(())
    }
    .await;

    // Stop the sync loop however serving ended. A running cycle is aborted: its apply
    // transaction rolls back and the lock connection is closed, which releases the lock.
    shutdown_token.cancel();
    if let Some(task) = sync_task
        && let Err(err) = task.await
    {
        tracing::error!(%err, "Git sync loop ended abnormally");
    }
    tracing::info!("server stopped");
    served
}

/// Resolves when the process receives Ctrl-C or, on Unix, SIGTERM.
pub async fn shutdown_signal() {
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

/// reqwest and sqlx also link rustls with other crypto providers, so rustls cannot pick one
/// by itself; choose aws-lc-rs explicitly. An already installed provider is fine.
fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

async fn load_tls(cert: &std::path::Path, key: &std::path::Path) -> anyhow::Result<RustlsConfig> {
    for (what, path) in [("certificate", cert), ("private key", key)] {
        tokio::fs::metadata(path)
            .await
            .with_context(|| format!("cannot read TLS {what} file {}", path.display()))?;
    }
    RustlsConfig::from_pem_file(cert, key).await.with_context(|| {
        format!(
            "cannot load TLS certificate {} with key {} (not valid PEM, or the key does not match the certificate)",
            cert.display(),
            key.display()
        )
    })
}
