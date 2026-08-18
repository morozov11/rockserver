use std::{env, net::SocketAddr};

/// Environment variable that overrides the address on which the HTTP service listens.
pub const BIND_ADDR_ENV: &str = "ROCKSERVER_BIND_ADDR";
/// Loopback address used when no listener address is configured.
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";
/// Environment variable containing the credential required by RockCast API callers.
pub const API_BEARER_TOKEN_ENV: &str = "ROCKSERVER_API_BEARER_TOKEN";

#[derive(Clone, Debug, Eq, PartialEq)]
/// Runtime settings required to start the RockServer HTTP process.
pub struct Config {
    /// Address on which the HTTP service listens.
    pub bind_addr: SocketAddr,
    /// Secret Bearer credential used by application API callers.
    pub api_bearer_token: String,
}

impl Config {
    /// Loads startup settings from the process environment without exposing secret values.
    pub fn from_env() -> Result<Self, ConfigError> {
        let value = env::var(BIND_ADDR_ENV).unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());
        let bind_addr = value
            .parse()
            .map_err(|source| ConfigError::InvalidBindAddress { value, source })?;

        let api_bearer_token =
            env::var(API_BEARER_TOKEN_ENV).map_err(|_| ConfigError::MissingApiBearerToken)?;
        if api_bearer_token.len() < 32 {
            return Err(ConfigError::ApiBearerTokenTooShort);
        }

        Ok(Self {
            bind_addr,
            api_bearer_token,
        })
    }
}

#[derive(Debug)]
/// Safe startup configuration failures.
pub enum ConfigError {
    /// The configured bind address is not a socket address.
    InvalidBindAddress {
        value: String,
        source: std::net::AddrParseError,
    },
    /// No application API credential was configured.
    MissingApiBearerToken,
    /// The configured application API credential is too short for a production secret.
    ApiBearerTokenTooShort,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBindAddress { value, source } => write!(
                formatter,
                "{BIND_ADDR_ENV} value {value:?} is not a valid socket address: {source}",
            ),
            Self::MissingApiBearerToken => write!(
                formatter,
                "{API_BEARER_TOKEN_ENV} must be configured to protect application API endpoints",
            ),
            Self::ApiBearerTokenTooShort => write!(
                formatter,
                "{API_BEARER_TOKEN_ENV} must contain at least 32 characters",
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidBindAddress { source, .. } => Some(source),
            Self::MissingApiBearerToken | Self::ApiBearerTokenTooShort => None,
        }
    }
}
