//! The background agent as a service (ADR-0042): start it at sign-in or
//! not, start or stop it now. Written once for `beam service` and the GUI's
//! Settings page.
//!
//! What runs is always `beam agent`, from the `beam` program (`exe`): the
//! command-line program is the one that knows how to be the agent, so a
//! front end that is not `beam` itself (the GUI) passes the `beam` next to
//! it. Per user only: an HKCU Run entry or a `systemd --user` unit, never a
//! system service, never administrator rights.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::agent::ipc::{Client, ClientMsg};
use crate::agent::service;
use crate::agent::status::{self, Running};
use crate::identity::{Store, StoreError};

/// How a start ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Started {
    /// Running now, saving to this folder.
    Now { receive_dir: String },
    /// It was running already, saving to this folder.
    Already { receive_dir: String },
}

/// How a stop ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    Now,
    /// It was not running.
    NotRunning,
}

/// Why the service could not do it.
#[derive(Debug, thiserror::Error)]
pub enum ServiceFailure {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Service(#[from] service::ServiceError),
    #[error("the background agent did not start; its log is {}", .log.display())]
    DidNotStart { log: PathBuf },
    #[error("could not reach the agent: {0}")]
    Unreachable(String),
    #[error("the background agent was asked to stop but is still running")]
    StillRunning,
}

/// Starts the agent at every sign-in, and now.
pub fn enable(store: &Store, exe: &Path, here: &Path) -> Result<Started, ServiceFailure> {
    store.load_identity()?;
    service::enable(exe, store.dir(), here)?;
    start(store, exe, here)
}

/// No longer at sign-in, and stopped now.
pub fn disable(store: &Store) -> Result<Stopped, ServiceFailure> {
    service::disable()?;
    stop(store)
}

/// Whether the agent starts at sign-in.
pub fn enabled() -> bool {
    service::is_enabled()
}

/// Starts the agent now, in `here`, and waits until it says it is up.
pub fn start(store: &Store, exe: &Path, here: &Path) -> Result<Started, ServiceFailure> {
    if let Running::Yes(status) = status::read(store) {
        return Ok(Started::Already {
            receive_dir: crate::untrusted::name(&status.receive_dir),
        });
    }
    store.load_identity()?;
    service::start(exe, here, store.dir())?;
    for _ in 0..50 {
        if let Running::Yes(status) = status::read(store) {
            return Ok(Started::Now {
                receive_dir: crate::untrusted::name(&status.receive_dir),
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(ServiceFailure::DidNotStart {
        log: store.agent_log_path(),
    })
}

/// Asks the running agent to stop, the way `listen` stops on Ctrl+C
/// (senders are told, partials kept), and waits until it has.
pub fn stop(store: &Store) -> Result<Stopped, ServiceFailure> {
    let Running::Yes(status) = status::read(store) else {
        return Ok(Stopped::NotRunning);
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ServiceFailure::Unreachable(e.to_string()))?;
    runtime
        .block_on(async {
            let (mut client, _) = Client::connect(status.port, &status.token).await?;
            client.send(&ClientMsg::Stop).await
        })
        .map_err(|e| ServiceFailure::Unreachable(crate::untrusted::lines(&e.to_string())))?;
    for _ in 0..80 {
        if !matches!(status::read(store), Running::Yes(_)) {
            return Ok(Stopped::Now);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(ServiceFailure::StillRunning)
}
