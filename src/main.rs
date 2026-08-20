use std::{error::Error, sync::Arc};

use rockserver::{
    config::Config,
    persistence::repository_from_env,
    providers::embedding_provider_from_env,
    providers::yandex_llm::YandexLlmProvider,
    providers::yandex_speechkit::YandexSpeechKitRecognizer,
    search::{
        DeterministicQueryParser, LlmProvider, LlmQueryParser, QueryParser, SearchService,
        SemanticLanguageClassifier, semantic_language_filters_enabled,
    },
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
    let language_classifier = match (&embedding_provider, semantic_language_filters_enabled()?) {
        (Some(provider), true) if provider.supports_semantic_intent_filters() => {
            match SemanticLanguageClassifier::load(provider.clone()).await {
                Ok(classifier) => {
                    tracing::info!("semantic language filters enabled");
                    Some(Arc::new(classifier))
                }
                Err(error) => {
                    tracing::warn!(%error, "semantic language filters unavailable; continuing without them");
                    None
                }
            }
        }
        (_, false) => {
            tracing::info!("semantic language filters disabled by configuration");
            None
        }
        (Some(_), true) => {
            tracing::info!("semantic language filters require a semantic embedding provider");
            None
        }
        (None, true) => None,
    };
    let query_parser: Arc<dyn QueryParser> = match YandexLlmProvider::optional_from_env()? {
        Some(provider) => Arc::new(LlmQueryParser::new(
            Arc::new(provider) as Arc<dyn LlmProvider>
        )),
        None => Arc::new(DeterministicQueryParser),
    };
    let search_service = SearchService::with_providers_and_language_classifier(
        repository,
        query_parser,
        embedding_provider,
        language_classifier,
    );
    let listener = TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "RockServer listening");

    let speech_recognizer = YandexSpeechKitRecognizer::optional_from_env()?
        .map(|provider| Arc::new(provider) as Arc<dyn rockserver::voice::StreamingSpeechRecognizer>)
        .unwrap_or_else(|| Arc::new(rockserver::voice::UnavailableSpeechRecognizer));
    serve(
        listener,
        rockserver::http::router_with_services_and_bearer_token(
            search_service,
            speech_recognizer,
            rockserver::http::DEFAULT_VOICE_COMMAND_TIMEOUT,
            config.api_bearer_token,
        ),
        shutdown_signal(),
    )
    .await?;
    Ok(())
}
