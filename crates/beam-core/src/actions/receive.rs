//! Receiving without `beam listen`: the background agent (`beam agent`,
//! ADR-0042) and the view's Receiving switch (ADR-0044), which are the same
//! receiver run from two places.
//!
//! Both are [`agent::run`]: pairing is never offered, every request waits
//! for a person's answer for up to five minutes, router port mapping only
//! if `agent_port_mapping` is on. The settings are made here, once, so the
//! two can never drift apart; only where it runs (`in_view`), desktop
//! notifications and where its log lines go differ.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use crate::agent::status::{self, Running};
use crate::agent::{self, AGENT_ACCEPT_TIMEOUT, AgentError, AgentOptions};
use crate::config::Config;
use crate::identity::{Identity, Store, StoreError};
use crate::pairing::Network;
use crate::transport::endpoint::Bind;

/// Where the receiver runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// `beam agent`: the background agent, in its own process.
    Background,
    /// The view's Receiving switch, for as long as the view is open.
    InView,
}

/// Why the receiver could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum ReceiveError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Config(String),
}

/// A receiver ready to run. (No `Debug`: it holds the identity.)
pub struct Receiver {
    identity: Identity,
    pub options: AgentOptions,
}

impl Receiver {
    /// The folder received files go to.
    pub fn receive_dir(&self) -> &PathBuf {
        &self.options.receive_dir
    }
}

/// Sets up the receiver from `config.toml`. `notify` shows a desktop
/// notification per request; `echo` gets each log line too.
pub fn prepare(
    store: &Store,
    place: Place,
    bind: Bind,
    notify: bool,
    echo: Option<fn(&str)>,
) -> Result<Receiver, ReceiveError> {
    let identity = store.load_identity()?;
    let config =
        Config::load(&store.config_path()).map_err(|e| ReceiveError::Config(e.to_string()))?;
    Ok(Receiver {
        identity,
        options: options(config, place, bind, notify, echo),
    })
}

/// The agent's settings: the same rules wherever it runs.
fn options(
    config: Config,
    place: Place,
    bind: Bind,
    notify: bool,
    echo: Option<fn(&str)>,
) -> AgentOptions {
    let (receive_dir, _) = agent::receive_dir(config.receive_dir.as_deref());
    AgentOptions {
        network: Network {
            relay: config.relay,
            bind,
            port: config.port,
            advertise: config.advertise,
        },
        receive_dir,
        accept_timeout: AGENT_ACCEPT_TIMEOUT,
        port_mapping: config.agent_port_mapping,
        notify,
        echo,
        in_view: place == Place::InView,
    }
}

/// Runs the receiver until `stop` completes. `started` is called once it
/// is really receiving: its status file names this process and carries its
/// invite.
pub async fn run(
    store: &Store,
    receiver: Receiver,
    stop: impl Future<Output = ()>,
    started: impl FnOnce(),
) -> Result<(), AgentError> {
    let in_view = receiver.options.in_view;
    let receiving = agent::run(receiver.identity, store.clone(), receiver.options, stop);
    tokio::pin!(receiving);
    loop {
        tokio::select! {
            result = &mut receiving => return result,
            () = tokio::time::sleep(Duration::from_millis(50)) => {
                if let Running::Yes(status) = status::read(store)
                    && status.in_view == in_view
                    && status.pid == std::process::id()
                    && status.invite.is_some()
                {
                    started();
                    break;
                }
            }
        }
    }
    receiving.await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The background agent and the Receiving switch follow the same rules:
    /// the same Accept window, the same port-mapping setting, the same
    /// network and folder. Only where they run differs.
    #[test]
    fn both_places_receive_by_the_same_rules() {
        let folder = std::env::temp_dir();
        let config = Config::parse(
            &format!(
                "relay = \"none\"\nport = 7999\nagent_port_mapping = true\nreceive_dir = '{}'\n",
                folder.display()
            ),
            std::path::Path::new("config.toml"),
        )
        .unwrap();
        let background = options(config.clone(), Place::Background, Bind::Any, true, None);
        let in_view = options(config, Place::InView, Bind::Any, true, None);
        assert!(!background.in_view);
        assert!(in_view.in_view);
        for options in [&background, &in_view] {
            assert_eq!(options.accept_timeout, AGENT_ACCEPT_TIMEOUT);
            assert!(options.port_mapping);
            assert_eq!(options.network.port, 7999);
            assert_eq!(options.receive_dir, folder);
        }
    }

    /// Port mapping stays off unless the person turned it on.
    #[test]
    fn port_mapping_is_off_unless_asked_for() {
        let config =
            Config::parse("relay = \"none\"\n", std::path::Path::new("config.toml")).unwrap();
        for place in [Place::Background, Place::InView] {
            assert!(!options(config.clone(), place, Bind::Any, false, None).port_mapping);
        }
    }
}
