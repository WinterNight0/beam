//! Whether the background agent is running, and how `beam inbox` reaches it.
//!
//! The same two-file pattern as `listen` (ADR-0037):
//!
//! * `agent.lock` is locked for as long as the agent runs, and the operating
//!   system drops the lock when the process ends, however it ends;
//! * `agent.json` is believed only while that lock is held.
//!
//! `agent.json` is private (mode 0600 on Unix; the user profile's own
//! permissions on Windows, ADR-0004), because it holds the token that lets a
//! local client answer transfer requests (ADR-0042).

use std::fs::{File, OpenOptions, TryLockError};

use serde::{Deserialize, Serialize};

use crate::identity::{Store, StoreError};

/// What `agent.json` holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatus {
    /// The agent's process id, for `beam service status`.
    pub pid: u32,
    /// The loopback TCP port `beam inbox` connects to.
    pub port: u16,
    /// What a client must present before the agent tells it anything.
    pub token: String,
    /// Where received files are saved.
    pub receive_dir: String,
    /// Whether the agent asked the router to forward its port.
    pub port_mapping: bool,
    /// When it started, Unix seconds.
    pub started: u64,
    /// The agent's own invite (not secret: key, relay, addresses), once it
    /// is listening. Paired peers already know where to find it.
    #[serde(default)]
    pub invite: Option<String>,
}

/// What a look at the agent finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Running {
    /// No agent is running in this beam home.
    No,
    /// One is, and this is how to reach it.
    Yes(AgentStatus),
    /// One is, but its status could not be read.
    Unreadable,
}

/// The agent's hold on `agent.lock`. Dropping it takes `agent.json` off disk.
pub struct AgentLock {
    store: Store,
    _lock: File,
}

/// Why the agent could not take its lock.
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error("the background agent is already running in this beam home")]
    Taken,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl AgentLock {
    /// Takes the lock, unless another agent in this beam home holds it.
    pub fn claim(store: &Store) -> Result<Self, ClaimError> {
        store.ensure_dirs()?;
        let path = store.agent_lock_path();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| StoreError::io("open", &path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                store: store.clone(),
                _lock: file,
            }),
            Err(TryLockError::WouldBlock) => Err(ClaimError::Taken),
            Err(TryLockError::Error(e)) => Err(StoreError::io("lock", &path, e).into()),
        }
    }

    /// Publishes how to reach this agent.
    pub fn publish(&self, status: &AgentStatus) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(status).expect("agent status serialises");
        self.store.save_agent_status(&json)
    }
}

impl Drop for AgentLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.store.agent_status_path());
    }
}

/// Whether an agent is running in `store`, and how to reach it.
pub fn read(store: &Store) -> Running {
    let Ok(lock) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.agent_lock_path())
    else {
        return Running::No;
    };
    match lock.try_lock_shared() {
        // Nobody holds it: any `agent.json` is left over from a crash.
        Ok(()) => Running::No,
        Err(TryLockError::WouldBlock) => match std::fs::read_to_string(store.agent_status_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            Some(status) => Running::Yes(status),
            None => Running::Unreadable,
        },
        Err(TryLockError::Error(_)) => Running::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> AgentStatus {
        AgentStatus {
            pid: 42,
            port: 50_000,
            token: "secret".into(),
            receive_dir: "/tmp".into(),
            port_mapping: false,
            started: 1,
            invite: None,
        }
    }

    #[test]
    fn a_running_agent_is_seen_and_a_stopped_one_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join(".beam"));
        assert_eq!(read(&store), Running::No);

        let lock = AgentLock::claim(&store).unwrap();
        lock.publish(&status()).unwrap();
        assert_eq!(read(&store), Running::Yes(status()));
        assert!(matches!(AgentLock::claim(&store), Err(ClaimError::Taken)));

        drop(lock);
        assert_eq!(read(&store), Running::No);
        assert!(
            !store.agent_status_path().exists(),
            "a clean stop removes the token"
        );
    }

    /// What a crash leaves: `agent.json` on disk, and nobody holding the lock.
    #[test]
    fn a_leftover_status_file_is_not_believed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join(".beam"));
        let json = serde_json::to_string(&status()).unwrap();
        store.save_agent_status(&json).unwrap();
        assert_eq!(read(&store), Running::No, "no lock file at all");

        File::create(store.agent_lock_path()).unwrap();
        assert_eq!(read(&store), Running::No, "a lock file nobody holds");
    }

    #[cfg(unix)]
    #[test]
    fn the_status_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join(".beam"));
        let lock = AgentLock::claim(&store).unwrap();
        lock.publish(&status()).unwrap();
        let mode = std::fs::metadata(store.agent_status_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
