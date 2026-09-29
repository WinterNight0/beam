//! Beam's local network configuration.
//!
//! Stage 1 deliberately has no rendezvous server or relay. Every Beam node
//! listens directly on TCP and the sender connects to the receiver's daemon.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_PORT: u16 = 9999;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// TCP port used by the Beam daemon.
    pub port: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("read {path}: {source}")]
    Read { path: PathBuf, #[source] source: std::io::Error },
    #[error("{path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("{path}: port must be between 1 and 65535, not {value}")]
    Port { path: PathBuf, value: u32 },
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    port: Option<u32>,
}

impl Default for Config {
    fn default() -> Self { Self { port: DEFAULT_PORT } }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read { path: path.to_path_buf(), source }),
        }
    }

    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(), message: e.message().to_string(),
        })?;
        let mut config = Self::default();
        if let Some(port) = raw.port {
            if port == 0 || port > u16::MAX as u32 {
                return Err(ConfigError::Port { path: path.to_path_buf(), value: port });
            }
            config.port = port as u16;
        }
        Ok(config)
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "port = {}", self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(text, Path::new("config.toml"))
    }

    #[test]
    fn defaults_to_beam_port() {
        assert_eq!(Config::default().port, DEFAULT_PORT);
    }

    #[test]
    fn reads_port() {
        assert_eq!(parse("port = 12345").unwrap().port, 12345);
    }

    #[test]
    fn rejects_invalid_port() {
        assert!(matches!(parse("port = 0"), Err(ConfigError::Port { .. })));
        assert!(matches!(parse("port = 70000"), Err(ConfigError::Port { .. })));
    }
}
