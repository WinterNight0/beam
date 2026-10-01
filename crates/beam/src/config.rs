//! `~/.beam/config.toml`: the relay, and the port `listen` uses.
//!
//! Both are network choices rather than identity, so they live in a file a
//! person edits by hand — TOML, not JSON, for that reason. A missing file
//! means the built-in defaults, and the defaults are meant to be all anyone
//! needs: there is no server to point beam at. See ADR-0029 and ADR-0036.
//!
//! ```toml
//! # Relay for connections that cannot go direct, or "none".
//! relay = "https://aps1-1.relay.n0.iroh.link./"
//!
//! # UDP port `beam listen` binds, so its invite stays the same between
//! # runs. 0 picks a random port each time.
//! port = 7820
//!
//! # Extra addresses to put first in this device's invite. For testing
//! # direct connections with `relay = "none"` behind a router whose port
//! # you forwarded by hand: beam cannot discover that address itself.
//! advertise = ["203.0.113.7:7820"]
//! ```

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use iroh::RelayUrl;
use serde::Deserialize;

/// The relay used when the config file does not name one.
///
/// number 0's Asia-Pacific relay: free, immediately usable, and it carries
/// QUIC it cannot decrypt. It is the development default only; a self-hosted
/// `iroh-relay` replaces it later. See `docs/n0-data.md`.
pub const DEFAULT_RELAY: &str = "https://aps1-1.relay.n0.iroh.link./";

/// The UDP port `beam listen` binds when the config file does not name one.
///
/// A fixed port keeps a device's invite — and the addresses its peers saved
/// from it — the same from one `listen` to the next. If the port is taken,
/// `listen` falls back to a random one and says so.
pub const DEFAULT_PORT: u16 = 7820;

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
    /// Relay for connections that cannot go direct. It is also where peers
    /// find each other: a peer is reached by its key through its relay.
    pub relay: Relay,
    /// UDP port for `beam listen`; 0 means a random one.
    pub port: u16,
    /// Addresses to put first in this device's invite (ADR-0038).
    pub advertise: Vec<SocketAddr>,
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
    #[error("{path}: relay must be a URL or \"{RELAY_NONE}\", not {value:?}")]
    Relay { path: PathBuf, value: String },
    #[error(
        "{path}: advertise entries must be IP:port with a real address and a port, \
         such as \"203.0.113.7:7820\", not {value:?}"
    )]
    Advertise { path: PathBuf, value: String },
}

/// The file as written. Unknown keys are an error, so a typo such as
/// `realy = "none"` is reported rather than silently ignored.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    /// Accepted so a config written for the rendezvous server still loads,
    /// and otherwise ignored: beam no longer uses one (ADR-0036).
    #[allow(dead_code)]
    rendezvous: Option<String>,
    relay: Option<String>,
    port: Option<u16>,
    advertise: Option<Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            relay: Relay::Url(DEFAULT_RELAY.parse().expect("the default relay URL parses")),
            port: DEFAULT_PORT,
            advertise: Vec::new(),
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
        if let Some(value) = raw.relay {
            config.relay = parse_relay(value.trim()).ok_or_else(|| ConfigError::Relay {
                path: path.to_path_buf(),
                value: value.clone(),
            })?;
        }
        if let Some(port) = raw.port {
            config.port = port;
        }
        for value in raw.advertise.unwrap_or_default() {
            let addr = value
                .trim()
                .parse::<SocketAddr>()
                .ok()
                .filter(|a| !a.ip().is_unspecified() && a.port() != 0)
                .ok_or_else(|| ConfigError::Advertise {
                    path: path.to_path_buf(),
                    value: value.clone(),
                })?;
            config.advertise.push(addr);
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
        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn an_empty_file_means_the_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
        assert_eq!(parse("# only a comment\n").unwrap(), Config::default());
    }

    #[test]
    fn both_settings_are_read() {
        let config = parse("relay = \"https://relay.example.org\"\nport = 0\n").unwrap();
        assert_eq!(config.port, 0);
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
    fn a_config_written_for_the_rendezvous_server_still_loads() {
        let config = parse("rendezvous = \"wss://rv.example.org/v1\"\nrelay = \"none\"\n").unwrap();
        assert_eq!(config.relay, Relay::Disabled);
    }

    #[test]
    fn a_port_that_does_not_fit_is_refused() {
        for value in ["70000", "-1", "\"7820\""] {
            let err = parse(&format!("port = {value}")).unwrap_err();
            assert!(matches!(err, ConfigError::Parse { .. }), "{value}: {err}");
        }
    }

    #[test]
    fn a_relay_that_is_not_a_url_is_refused() {
        for value in ["off", "relay.example.org", ""] {
            let err = parse(&format!("relay = \"{value}\"")).unwrap_err();
            assert!(matches!(err, ConfigError::Relay { .. }), "{value}: {err}");
        }
    }

    #[test]
    fn advertised_addresses_are_read_and_checked() {
        let config =
            parse("relay = \"none\"\nadvertise = [\"203.0.113.7:7820\", \"[2001:db8::7]:7820\"]\n")
                .unwrap();
        assert_eq!(config.relay, Relay::Disabled);
        assert_eq!(
            config.advertise,
            [
                "203.0.113.7:7820".parse::<SocketAddr>().unwrap(),
                "[2001:db8::7]:7820".parse().unwrap()
            ]
        );
        assert!(Config::default().advertise.is_empty());
        for bad in [
            "203.0.113.7",
            "0.0.0.0:7820",
            "203.0.113.7:0",
            "example.org:7820",
        ] {
            let err = parse(&format!("advertise = [\"{bad}\"]")).unwrap_err();
            assert!(matches!(err, ConfigError::Advertise { .. }), "{bad}: {err}");
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
