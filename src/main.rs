use std::{env, error::Error, fs, sync::Arc};

use rockserver::{
    config::Config,
    http::TRUSTED_PROXY_TOKEN_ENV,
    persistence::{
        DATABASE_URL_ENV, PostgresAccountStore, PostgresAdminStore, PostgresEmbeddingStore,
        PostgresGenreTaxonomySource, repository_from_env,
    },
    providers::embedding_provider_from_env,
    providers::yandex_home::YandexHomeClient,
    providers::yandex_llm::YandexLlmProvider,
    providers::yandex_speechkit::YandexSpeechKitRecognizer,
    providers::yandex_speechkit_streaming::YandexSpeechKitStreamingRecognizer,
    search::{
        DeterministicQueryParser, EmbeddingProviderError, EmbeddingQualityReview, LlmProvider,
        LlmQueryParser, QueryParser, SearchService, SemanticLanguageClassifier,
        semantic_language_filters_enabled,
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
    let embedding_provider = embedding_provider_from_env()?;
    if let Some(provenance) = embedding_provider
        .as_ref()
        .and_then(|provider| provider.provenance())
        && provenance.model == "intfloat/multilingual-e5-small"
    {
        let store = PostgresEmbeddingStore::connect(&database_url).await?;
        let readiness = store.e5_release_readiness(&provenance).await;
        store.close().await;
        if !readiness?.is_ready() {
            return Err(EmbeddingProviderError::safe(
                "E5 release is incomplete or its ANN index is unavailable",
            )
            .into());
        }
        if provenance.version != "onnx-v1" {
            let review_path = env::var("ROCKSERVER_E5_QUALITY_REVIEW_PATH").map_err(|_| {
                EmbeddingProviderError::safe("E5 quality review path is required for a new version")
            })?;
            let review = fs::read(review_path).map_err(|_| {
                EmbeddingProviderError::safe("E5 quality review file is unreadable")
            })?;
            EmbeddingQualityReview::validate_json(&review, &provenance)?;
        }
    }
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
