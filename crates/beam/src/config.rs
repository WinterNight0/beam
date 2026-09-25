//! `~/.beam/config.toml`: where the rendezvous server and the relay are.
//!
//! Both are infrastructure choices rather than identity, so they live in a
//! file a person edits by hand — TOML, not JSON, for that reason. A missing
//! file means the built-in defaults. See ADR-0029.
//!
//! ```toml
//! # Where Short IDs are announced and looked up.
//! rendezvous = "ws://127.0.0.1:8787/v1"
//!
//! # Relay for connections that cannot go direct, or "none".
//! relay = "https://aps1-1.relay.n0.iroh.link./"
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

use iroh::RelayUrl;
use serde::Deserialize;

/// The rendezvous server used when the config file does not name one.
///
/// There is no public beam rendezvous server, so the default is the port
/// `beam-server` listens on by default, on this machine.
pub const DEFAULT_RENDEZVOUS: &str = "ws://127.0.0.1:8787/v1";

/// The relay used when the config file does not name one.
///
/// number 0's Asia-Pacific relay: free, immediately usable, and it carries
/// QUIC it cannot decrypt. It is the development default only; a self-hosted
/// `iroh-relay` replaces it later. See `docs/n0-data.md`.
pub const DEFAULT_RELAY: &str = "https://aps1-1.relay.n0.iroh.link./";

/// The value of `relay` that turns relaying off.
pub const RELAY_NONE: &str = "none";

/// Which relay, if any, connections may fall back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Relay {
    /// Direct connections only. Private, and likely to fail behind CGNAT.
    Disabled,
    /// Fall back through this relay.
    Url(RelayUrl),
}

impl fmt::Display for Relay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str("none (direct connections only)"),
            Self::Url(url) => write!(f, "{url}"),
        }
    }
}

/// The settings beam reads from `config.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `ws://` or `wss://` URL of the rendezvous server.
    pub rendezvous: String,
    /// Relay for connections that cannot go direct.
    pub relay: Relay,
}

/// Why the config file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("{path}: rendezvous must start with ws:// or wss://, not {value:?}")]
    Rendezvous { path: PathBuf, value: String },
    #[error("{path}: relay must be a URL or \"{RELAY_NONE}\", not {value:?}")]
    Relay { path: PathBuf, value: String },
}

/// The file as written. Unknown keys are an error, so a typo such as
/// `realy = "none"` is reported rather than silently ignored.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    rendezvous: Option<String>,
    relay: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rendezvous: DEFAULT_RENDEZVOUS.to_string(),
            relay: Relay::Url(DEFAULT_RELAY.parse().expect("the default relay URL parses")),
        }
    }
}

impl Config {
    /// Reads `path`, or returns the defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Parses the text of a config file. `path` is only used in errors.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        // Windows PowerShell 5.1 writes a byte order mark; see ADR-0014.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.message().to_string(),
        })?;

        let mut config = Self::default();
        if let Some(value) = raw.rendezvous {
            let value = value.trim().to_string();
            if !(value.starts_with("ws://") || value.starts_with("wss://")) {
                return Err(ConfigError::Rendezvous {
                    path: path.to_path_buf(),
                    value,
                });
            }
            config.rendezvous = value;
        }
        if let Some(value) = raw.relay {
            config.relay = parse_relay(value.trim()).ok_or_else(|| ConfigError::Relay {
                path: path.to_path_buf(),
                value: value.clone(),
            })?;
        }
        Ok(config)
    }
}

fn parse_relay(value: &str) -> Option<Relay> {
    if value.eq_ignore_ascii_case(RELAY_NONE) {
        return Some(Relay::Disabled);
    }
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return None;
    }
    value.parse().ok().map(Relay::Url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(text, Path::new("config.toml"))
    }

    #[test]
    fn a_missing_file_means_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.rendezvous, DEFAULT_RENDEZVOUS);
    }

    #[test]
    fn an_empty_file_means_the_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
        assert_eq!(parse("# only a comment\n").unwrap(), Config::default());
    }

    #[test]
    fn both_settings_are_read() {
        let config = parse(
            "rendezvous = \"wss://rv.example.org/v1\"\nrelay = \"https://relay.example.org\"\n",
        )
        .unwrap();
        assert_eq!(config.rendezvous, "wss://rv.example.org/v1");
        assert_eq!(
            config.relay,
            Relay::Url("https://relay.example.org".parse().unwrap())
        );
    }

    #[test]
    fn relay_none_disables_relaying() {
        assert_eq!(parse("relay = \"none\"").unwrap().relay, Relay::Disabled);
        assert_eq!(parse("relay = \"NONE\"").unwrap().relay, Relay::Disabled);
    }

    #[test]
    fn a_typo_in_a_key_is_an_error_not_a_silent_default() {
        let err = parse("realy = \"none\"").unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
        assert!(err.to_string().contains("realy"), "{err}");
    }

    #[test]
    fn a_rendezvous_that_is_not_a_websocket_url_is_refused() {
        let err = parse("rendezvous = \"http://example.org\"").unwrap_err();
        assert!(matches!(err, ConfigError::Rendezvous { .. }), "{err}");
    }

    #[test]
    fn a_relay_that_is_not_a_url_is_refused() {
        for value in ["off", "relay.example.org", ""] {
            let err = parse(&format!("relay = \"{value}\"")).unwrap_err();
            assert!(matches!(err, ConfigError::Relay { .. }), "{value}: {err}");
        }
    }

    #[test]
    fn a_byte_order_mark_is_ignored() {
        let config = parse("\u{feff}relay = \"none\"\n").unwrap();
        assert_eq!(config.relay, Relay::Disabled);
    }

    #[test]
    fn the_default_relay_is_n0s_asia_pacific_relay() {
        match Config::default().relay {
            Relay::Url(url) => assert!(url.to_string().contains("aps1-1.relay.n0.iroh.link")),
            Relay::Disabled => panic!("the development default relays"),
        }
    }
}
