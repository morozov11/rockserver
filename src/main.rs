use std::error::Error;

use rockserver::{config::Config, http::router, serve, shutdown_signal, telemetry};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    telemetry::init()?;

    let config = Config::from_env()?;
    let listener = TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "RockServer listening");

    serve(listener, router(), shutdown_signal()).await?;
    Ok(())
}
