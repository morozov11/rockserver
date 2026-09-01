//! One-shot protected administrator bootstrap command.

use std::{env, error::Error, fmt};

use rockserver::{
    admin::AdminBootstrapOutcome,
    admin_bootstrap::{AdminBootstrapConfig, bootstrap_admin},
    persistence::{DATABASE_URL_ENV, PostgresAdminStore},
};

/// Starts the protected command without loading `.env` files or accepting secrets as arguments.
#[tokio::main]
async fn main() -> Result<(), BootstrapCommandError> {
    let config = AdminBootstrapConfig::from_env().map_err(BootstrapCommandError::Configuration)?;
    let database_url =
        env::var(DATABASE_URL_ENV).map_err(|_| BootstrapCommandError::MissingDatabaseUrl)?;
    let store = PostgresAdminStore::connect(&database_url)
        .await
        .map_err(|_| BootstrapCommandError::StoreUnavailable)?;
    match bootstrap_admin(&store, config)
        .await
        .map_err(|_| BootstrapCommandError::StoreUnavailable)?
    {
        AdminBootstrapOutcome::Created => println!("administrator bootstrap completed"),
        AdminBootstrapOutcome::AlreadyExists => {
            println!("administrator bootstrap skipped: administrator already exists")
        }
    }
    Ok(())
}

/// Safe command-line failures that never render configuration values or hashes.
#[derive(Debug)]
enum BootstrapCommandError {
    /// Protected bootstrap input was absent or invalid.
    Configuration(rockserver::admin_bootstrap::AdminBootstrapConfigError),
    /// Database configuration was absent.
    MissingDatabaseUrl,
    /// PostgreSQL could not complete the bootstrap.
    StoreUnavailable,
}

impl fmt::Display for BootstrapCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => error.fmt(formatter),
            Self::MissingDatabaseUrl => write!(formatter, "{DATABASE_URL_ENV} is required"),
            Self::StoreUnavailable => {
                formatter.write_str("administrator bootstrap storage is unavailable")
            }
        }
    }
}

impl Error for BootstrapCommandError {}
