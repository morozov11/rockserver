use std::{env, error::Error, sync::Arc};

use rockserver::{
    config::Config,
    http::TRUSTED_PROXY_TOKEN_ENV,
    persistence::{
        DATABASE_URL_ENV, PostgresAccountStore, PostgresAdminStore, repository_from_env,
    },
    providers::embedding_provider_from_env,
    providers::yandex_llm::YandexLlmProvider,
    providers::yandex_speechkit::YandexSpeechKitRecognizer,
    providers::yandex_speechkit_streaming::YandexSpeechKitStreamingRecognizer,
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
    let database_url = env::var(DATABASE_URL_ENV)?;
    let trusted_proxy_token = env::var(TRUSTED_PROXY_TOKEN_ENV)?;
    let account_store = PostgresAccountStore::connect(&database_url).await?;
    let admin_store = PostgresAdminStore::connect(&database_url).await?;
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

    let unavailable = Arc::new(rockserver::voice::UnavailableSpeechRecognizer)
        as Arc<dyn rockserver::voice::StreamingSpeechRecognizer>;
    let buffered_speech_recognizer = YandexSpeechKitRecognizer::optional_from_env()?
        .map(|provider| Arc::new(provider) as Arc<dyn rockserver::voice::StreamingSpeechRecognizer>)
        .unwrap_or_else(|| Arc::clone(&unavailable));
    let streaming_speech_recognizer = YandexSpeechKitStreamingRecognizer::optional_from_env()?
        .map(|provider| Arc::new(provider) as Arc<dyn rockserver::voice::StreamingSpeechRecognizer>)
        .unwrap_or(unavailable);
    serve(
        listener,
        rockserver::http::router_with_speech_recognizers_bearer_account_admin_store_and_proxy(
            search_service,
            rockserver::voice::SpeechRecognizers::new(
                buffered_speech_recognizer,
                streaming_speech_recognizer,
            ),
            rockserver::http::DEFAULT_VOICE_COMMAND_TIMEOUT,
            config.api_bearer_token,
            account_store,
            admin_store,
            trusted_proxy_token,
        ),
        shutdown_signal(),
    )
    .await?;
    Ok(())
}
