use std::{
    env, fs, io,
    path::Path,
    thread,
    time::{Duration, SystemTime},
};

use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialises structured JSON logging to both stdout and a daily rolling
/// file under `logs/`, or the directory selected by `ROCKSERVER_LOG_DIR`. The file log always
/// captures `debug` and above so that search diagnostics are available even when the console runs
/// at `info`. Daily files older than the configured retention period are removed by RockServer
/// itself, including while the process keeps running.
pub fn init() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let console_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let log_directory = env::var("ROCKSERVER_LOG_DIR").unwrap_or_else(|_| "logs".to_owned());
    let retention_days = log_retention_days();
    fs::create_dir_all(&log_directory)?;
    let file_appender = tracing_appender::rolling::daily(&log_directory, "rockserver.log");
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
    start_log_retention(log_directory, retention_days);
    Ok(())
}

const DEFAULT_LOG_RETENTION_DAYS: u64 = 14;
const MAX_LOG_RETENTION_DAYS: u64 = 3650;

fn log_retention_days() -> u64 {
    env::var("ROCKSERVER_LOG_RETENTION_DAYS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|days| (1..=MAX_LOG_RETENTION_DAYS).contains(days))
        .unwrap_or(DEFAULT_LOG_RETENTION_DAYS)
}

fn start_log_retention(log_directory: String, retention_days: u64) {
    prune_expired_logs(Path::new(&log_directory), retention_days, SystemTime::now());
    let _ = thread::Builder::new()
        .name("rockserver-log-retention".to_owned())
        .spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(24 * 60 * 60));
                prune_expired_logs(Path::new(&log_directory), retention_days, SystemTime::now());
            }
        });
}

fn prune_expired_logs(log_directory: &Path, retention_days: u64, now: SystemTime) {
    let cutoff = now - Duration::from_secs(retention_days * 24 * 60 * 60);
    match remove_logs_older_than(log_directory, cutoff) {
        Ok(removed) if removed > 0 => {
            tracing::info!(removed, retention_days, "expired log files removed")
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "could not prune expired log files"),
    }
}

fn remove_logs_older_than(log_directory: &Path, cutoff: SystemTime) -> io::Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(log_directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if !file_type.is_file() || !file_name.starts_with("rockserver.log.") {
            continue;
        }
        if entry.metadata()?.modified()? < cutoff {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    use super::remove_logs_older_than;

    #[test]
    fn removes_only_expired_rockserver_log_files() {
        let directory =
            std::env::temp_dir().join(format!("rockserver-telemetry-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let expired = directory.join("rockserver.log.2026-08-01");
        let unrelated = directory.join("other.log");
        fs::write(&expired, "expired").unwrap();
        fs::write(&unrelated, "keep").unwrap();

        let removed =
            remove_logs_older_than(&directory, SystemTime::now() + Duration::from_secs(1)).unwrap();

        assert_eq!(removed, 1);
        assert!(!expired.exists());
        assert!(unrelated.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
