use std::{error::Error, sync::Arc};

use rockserver::{
    config::Config,
    persistence::repository_from_env,
    providers::embedding_provider_from_env,
    providers::yandex_llm::YandexLlmProvider,
    providers::yandex_speechkit::YandexSpeechKitRecognizer,
    search::{DeterministicQueryParser, LlmProvider, LlmQueryParser, QueryParser, SearchService},
    serve, shutdown_signal, telemetry,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // Local development may supply ignored environment variables; deployed processes use env only.
    dotenvy::dotenv().ok();
    telemetry::init()?;

    let config = Config::from_env()?;
    let repository = repository_from_env().await?;
    let embedding_provider = embedding_provider_from_env()?;
    let query_parser: Arc<dyn QueryParser> = match YandexLlmProvider::optional_from_env()? {
        Some(provider) => Arc::new(LlmQueryParser::new(
            Arc::new(provider) as Arc<dyn LlmProvider>
        )),
        None => Arc::new(DeterministicQueryParser),
    };
    let search_service =
        SearchService::with_providers(repository, query_parser, embedding_provider);
    let listener = TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "RockServer listening");

    let speech_recognizer = YandexSpeechKitRecognizer::optional_from_env()?
        .map(|provider| {
            Arc::new(provider) as Arc<dyn rockserver::speech::StreamingSpeechRecognizer>
        })
        .unwrap_or_else(|| Arc::new(rockserver::speech::UnavailableSpeechRecognizer));
    serve(
        listener,
        rockserver::http::router_with_services(
            search_service,
            speech_recognizer,
            rockserver::http::DEFAULT_VOICE_COMMAND_TIMEOUT,
        ),
        shutdown_signal(),
    )
    .await?;
    Ok(())
}
