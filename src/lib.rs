/// Service configuration loaded from the process environment.
pub mod config;
/// HTTP routes and transport types.
pub mod http;
/// Persistent station-catalog backends and startup selection.
pub mod persistence;
/// Deterministic station-search domain and catalog boundary.
pub mod search;
/// Structured tracing setup.
pub mod telemetry;

use std::{future::Future, io};

use axum::Router;
use tokio::net::TcpListener;

/// Serves the application until the provided shutdown future completes.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
}

/// Waits for the process Ctrl+C signal used by the current binary entry point.
pub async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install Ctrl+C handler");
    }
}
