use std::{env, error::Error, sync::Arc};

use rockserver::{
    config::Config,
    http::TRUSTED_PROXY_TOKEN_ENV,
    persistence::{
        DATABASE_URL_ENV, PostgresAccountStore, PostgresAdminStore, PostgresGenreTaxonomySource,
        repository_from_env,
    },
    providers::embedder::{EmbedderClient, EmbedderConfig, EmbedderGateway, EmbedderLifecycle},
    providers::yandex_home::YandexHomeClient,
    providers::yandex_llm::YandexLlmProvider,
    providers::yandex_speechkit::YandexSpeechKitRecognizer,
    providers::yandex_speechkit_streaming::YandexSpeechKitStreamingRecognizer,
    search::{DeterministicQueryParser, LlmProvider, LlmQueryParser, QueryParser, SearchService},
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
    // The embedding service client configures vector search (without an address,
    // the process starts in the no-vectors mode; when configured, the background
    // task enables the vector branch after Info and schema verification).
    let embedder_config = EmbedderConfig::from_env()?;
    let (embedding_provider, embedder_lifecycle) = match embedder_config {
        Some(embedder) => {
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
        None => (None, None),
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
    let mut search_service =
        SearchService::with_providers(repository, query_parser, embedding_provider);
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
