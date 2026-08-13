use std::error::Error;

use rockserver::{
    config::Config, http::router_with_repository, persistence::repository_from_env, serve,
    shutdown_signal, telemetry,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    telemetry::init()?;

    let config = Config::from_env()?;
    let repository = repository_from_env().await?;
    let listener = TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "RockServer listening");

    serve(
        listener,
        router_with_repository(repository),
        shutdown_signal(),
    )
    .await?;
    Ok(())
}
