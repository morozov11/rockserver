use std::{error::Error, sync::Arc};

use rockserver::{
    config::Config,
    http::router_with_search_service,
    persistence::repository_from_env,
    providers::deterministic_embedding::DeterministicEmbeddingProvider,
    search::{DeterministicQueryParser, EmbeddingProvider, SearchService},
    serve, shutdown_signal, telemetry,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    telemetry::init()?;

    let config = Config::from_env()?;
    let repository = repository_from_env().await?;
    let embedding_provider = DeterministicEmbeddingProvider::optional_from_env()?
        .map(|provider| Arc::new(provider) as Arc<dyn EmbeddingProvider>);
    let search_service = SearchService::with_providers(
        repository,
        Arc::new(DeterministicQueryParser),
        embedding_provider,
    );
    let listener = TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "RockServer listening");

    serve(
        listener,
        router_with_search_service(search_service),
        shutdown_signal(),
    )
    .await?;
    Ok(())
}
