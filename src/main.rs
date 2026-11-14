use std::{env, error::Error, sync::Arc};

use rockserver::{
    config::Config,
    http::TRUSTED_PROXY_TOKEN_ENV,
    persistence::{
        DATABASE_URL_ENV, PostgresAccountStore, PostgresAdminStore, PostgresGenreTaxonomySource,
        repository_from_env,
    },
    providers::embedder::{EmbedderClient, EmbedderConfig, EmbedderGateway, EmbedderLifecycle},
    providers::yandex_llm::YandexLlmProvider,
    providers::yandex_speechkit::YandexSpeechKitRecognizer,
    providers::yandex_speechkit_streaming::YandexSpeechKitStreamingRecognizer,
    providers::{embedding_provider_from_env, yandex_home::YandexHomeClient},
    search::{
        DeterministicQueryParser, LlmProvider, LlmQueryParser, QueryParser, SearchService,
        SemanticLanguageClassifier, semantic_language_filters_enabled,
    },
    serve, shutdown_signal, telemetry,
    voice::{CommandInterpreter, DeterministicCommandInterpreter, LlmCommandInterpreter},
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
    rockserver::http::recover_station_icon_imports(&account_store).await;
    let admin_store = PostgresAdminStore::connect(&database_url).await?;
    // The embedding service client takes precedence over the legacy local
    // providers (onnx-local remains available until roadmap stage E3, and the
    // old release-readiness startup gate is superseded by the E2 lifecycle:
    // the process starts in the no-vectors mode and the background task
    // enables the vector branch after Info and schema verification).
    let embedder_config = EmbedderConfig::from_env()?;
    let (embedding_provider, embedder_lifecycle) = match embedder_config {
        Some(embedder) => {
            if env::var("ROCKSERVER_SEMANTIC_PROVIDER").is_ok_and(|value| !value.trim().is_empty())
            {
                tracing::warn!(
                    "ROCKSERVER_SEMANTIC_PROVIDER is ignored while ROCKSERVER_EMBEDDER_ADDR is set"
                );
            }
            let service = Arc::new(EmbedderClient::lazy(embedder)?);
            let gateway = Arc::new(EmbedderGateway::disabled());
            let lifecycle = EmbedderLifecycle::new(
                Arc::clone(&service),
                Arc::clone(&gateway),
                database_url.clone(),
            );
            (
                Some(gateway as Arc<dyn rockserver::search::EmbeddingProvider>),
                Some(lifecycle),
            )
        }
        None => (embedding_provider_from_env()?, None),
    };
    let language_classifier = match (&embedding_provider, semantic_language_filters_enabled()?) {
        (Some(provider), true) if provider.supports_semantic_intent_filters() => {
            match SemanticLanguageClassifier::load(provider.clone()).await {
                Ok(classifier) => {
                    tracing::info!("semantic language diagnostics enabled");
                    Some(Arc::new(classifier))
                }
                Err(error) => {
                    tracing::warn!(%error, "semantic language diagnostics unavailable; continuing without them");
                    None
                }
            }
        }
        (_, false) => {
            tracing::info!("semantic language diagnostics disabled by configuration");
            None
        }
        (Some(_), true) => {
            tracing::info!("semantic language diagnostics require a semantic embedding provider");
            None
        }
        (None, true) => None,
    };
    let structured_provider = YandexLlmProvider::optional_from_env()?
        .map(|provider| Arc::new(provider) as Arc<dyn LlmProvider>);
    let has_structured_provider = structured_provider.is_some();
    let query_parser: Arc<dyn QueryParser> = match &structured_provider {
        Some(provider) => Arc::new(LlmQueryParser::new(Arc::clone(provider))),
        None => Arc::new(DeterministicQueryParser),
    };
    let voice_command_interpreter: Arc<dyn CommandInterpreter> = match structured_provider {
        Some(provider) => Arc::new(LlmCommandInterpreter::new(provider)),
        None => Arc::new(DeterministicCommandInterpreter),
    };
    let mut search_service = SearchService::with_providers_and_language_classifier(
        repository,
        query_parser,
        embedding_provider,
        language_classifier,
    );
    if has_structured_provider {
        let taxonomy_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect_lazy(&database_url)?;
        search_service = search_service.with_taxonomy_source(Arc::new(
            PostgresGenreTaxonomySource::from_pool(taxonomy_pool),
        ));
    }
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
    let yandex_home = YandexHomeClient::optional_from_env()?;
    if let Some(lifecycle) = embedder_lifecycle {
        tokio::spawn(lifecycle.run());
    }
    serve(
        listener,
        rockserver::http::router_with_speech_recognizers_bearer_account_admin_store_proxy_and_voice_interpreter(
            search_service,
            (
                rockserver::voice::SpeechRecognizers::new(
                    buffered_speech_recognizer,
                    streaming_speech_recognizer,
                ),
                voice_command_interpreter,
            ),
            rockserver::http::DEFAULT_VOICE_COMMAND_TIMEOUT,
            config.api_bearer_token,
            account_store,
            admin_store,
            trusted_proxy_token,
            yandex_home,
        ),
        shutdown_signal(),
    )
    .await?;
    Ok(())
}
