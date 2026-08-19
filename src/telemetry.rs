use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialises structured JSON logging to both stdout and a daily rolling
/// file under `logs/`. The file log always captures `debug` and above so
/// that search diagnostics are available even when the console runs at `info`.
pub fn init() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let console_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let file_appender = tracing_appender::rolling::daily("logs", "rockserver.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Leak the guard so the file writer lives for the whole process.
    std::mem::forget(_guard);

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .json()
                .with_writer(std::io::stdout)
                .with_filter(console_filter),
        )
        .with(
            fmt::layer()
                .json()
                .with_writer(non_blocking)
                .with_filter(EnvFilter::new("debug")),
        )
        .try_init()?;
    Ok(())
}
