use std::{env, net::SocketAddr};

pub const BIND_ADDR_ENV: &str = "ROCKSERVER_BIND_ADDR";
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    pub bind_addr: SocketAddr,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let value = env::var(BIND_ADDR_ENV).unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());
        let bind_addr = value
            .parse()
            .map_err(|source| ConfigError { value, source })?;

        Ok(Self { bind_addr })
    }
}

#[derive(Debug)]
pub struct ConfigError {
    value: String,
    source: std::net::AddrParseError,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{BIND_ADDR_ENV} value {:?} is not a valid socket address: {}",
            self.value, self.source
        )
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
