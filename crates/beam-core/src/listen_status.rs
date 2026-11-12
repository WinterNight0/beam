//! What a running `beam listen` is offering, for `beam whoami` to show.
//!
//! `listen` prints its invite and pairing code once, when it starts. The code
//! changes after every attempt and every ten minutes, and printing each new
//! one buried long transfers under notices, so instead `listen` writes its
//! current state to `~/.beam/listen.json` and `beam whoami` reads it
//! (ADR-0037).
//!
//! Two files, because a crash must not leave a stale code on show:
//!
//! * `listen.lock` is locked for as long as `listen` runs. The operating
//!   system drops the lock when the process ends, however it ends.
//! * `listen.json` is believed only while that lock is held. Left behind by a
//!   crash, it is simply ignored.
//!
//! `listen.json` is private (mode 0600 on Unix), because it holds a live
//! pairing code. See ADR-0037 for why a code on disk is acceptable.

use std::fs::{File, OpenOptions, TryLockError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::identity::{Store, StoreError};
use crate::listener::ListenEvent;

/// How many times `listen` tries for the lock before deciding another
/// `listen` holds it. `whoami` holds it for an instant while it looks.
const CLAIM_TRIES: u32 = 10;
const CLAIM_PAUSE: Duration = Duration::from_millis(20);

/// What `listen.json` holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenStatus {
    /// The invite `listen` showed when it started.
    pub invite: String,
    pub pairing: PairingStatus,
}

/// Where pairing stands in a running `listen`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PairingStatus {
    /// This code works now, until `expires_at` (Unix seconds).
    Live { code: String, expires_at: u64 },
    /// An attempt is using the code right now.
    InUse,
    /// Paused after a failed attempt, until `until` (Unix seconds).
    Paused { until: u64 },
    /// Off for the rest of this `listen` session.
    Off { failures: u32 },
}

/// What `beam whoami` finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listening {
    /// No `listen` is running in this beam home.
    No,
    /// One is, and this is what it offers.
    Yes(ListenStatus),
    /// One is, but its status could not be read (for example, a different
    /// version of beam wrote it).
    Unreadable,
}

/// Keeps `listen.json` in step with what `listen` is doing.
pub struct Board {
    store: Store,
    /// Held while this `listen` runs. `None` when another `listen` already
    /// holds it: then this one publishes nothing.
    lock: Option<File>,
    code_ttl: Duration,
    status: Option<ListenStatus>,
}

impl Board {
    /// Takes the lock, unless another `listen` in the same beam home has it.
    pub fn claim(store: &Store, code_ttl: Duration) -> Result<Self, StoreError> {
        store.ensure_dirs()?;
        let path = store.listen_lock_path();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| StoreError::io("open", &path, e))?;
        let mut lock = None;
        for _ in 0..CLAIM_TRIES {
            match file.try_lock() {
                Ok(()) => {
                    lock = Some(file);
                    break;
                }
                Err(TryLockError::WouldBlock) => std::thread::sleep(CLAIM_PAUSE),
                Err(TryLockError::Error(e)) => return Err(StoreError::io("lock", &path, e)),
            }
        }
        Ok(Self {
            store: store.clone(),
            lock,
            code_ttl,
            status: None,
        })
    }

    /// Whether this `listen` is the one `whoami` will show.
    pub fn is_publishing(&self) -> bool {
        self.lock.is_some()
    }

    /// Updates `listen.json` for one `listen` event, if it changes anything.
    pub fn on_event(&mut self, event: &ListenEvent) -> Result<(), StoreError> {
        self.on_event_at(event, SystemTime::now())
    }

    fn on_event_at(&mut self, event: &ListenEvent, now: SystemTime) -> Result<(), StoreError> {
        let live = |code: &crate::pairing::PairingCode| PairingStatus::Live {
            code: code.as_str().to_string(),
            expires_at: unix(now + self.code_ttl),
        };
        let next = match (event, &self.status) {
            (
                ListenEvent::Ready {
                    invite: text, code, ..
                },
                _,
            ) => ListenStatus {
                invite: text.to_string(),
                pairing: live(code),
            },
            (event, Some(current)) => {
                let pairing = match event {
                    ListenEvent::NewCode { code, .. } => live(code),
                    ListenEvent::PairingAttempt { .. } => PairingStatus::InUse,
                    ListenEvent::PairingPaused { wait, .. } => PairingStatus::Paused {
                        until: unix(now + *wait),
                    },
                    ListenEvent::PairingDisabled { failures } => PairingStatus::Off {
                        failures: *failures,
                    },
                    _ => return Ok(()),
                };
                ListenStatus {
                    invite: current.invite.clone(),
                    pairing,
                }
            }
            // Nothing to say before `Ready`.
            (_, None) => return Ok(()),
        };
        if self.status.as_ref() == Some(&next) {
            return Ok(());
        }
        if self.lock.is_some() {
            let json = serde_json::to_string_pretty(&next).expect("a status serialises");
            self.store.save_listen_status(&json)?;
        }
        self.status = Some(next);
        Ok(())
    }
}

impl Drop for Board {
    /// A clean exit takes the code off disk. After a crash the file stays, but
    /// the lock is gone with the process, so nobody believes it.
    fn drop(&mut self) {
        if self.lock.is_some() {
            let _ = std::fs::remove_file(self.store.listen_status_path());
        }
    }
}

/// What a running `listen` in `store` offers, if one is running.
pub fn read(store: &Store) -> Listening {
    let Ok(lock) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.listen_lock_path())
    else {
        // No lock file: `listen` never ran here.
        return Listening::No;
    };
    match lock.try_lock_shared() {
        // We could take it, so nobody holds it: nothing is running, and any
        // `listen.json` is left over from a crash. Dropping `lock` releases it.
        Ok(()) => Listening::No,
        Err(TryLockError::WouldBlock) => match std::fs::read_to_string(store.listen_status_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            Some(status) => Listening::Yes(status),
            None => Listening::Unreadable,
        },
        Err(TryLockError::Error(_)) => Listening::Unreadable,
    }
}

/// Seconds since the Unix epoch.
pub fn unix(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Relay;
    use crate::identity::Fingerprint;
    use crate::identity::vectors;
    use crate::invite::Invite;
    use crate::pairing::PairingCode;
    use crate::pairing::rotation::NewCodeReason;

    const TTL: Duration = Duration::from_secs(600);

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join(".beam"));
        (tmp, store)
    }

    fn invite() -> Invite {
        Invite {
            key: vectors::verifying_key("alpha"),
            relay: None,
            addrs: vec!["192.168.1.20:7820".parse().unwrap()],
        }
    }

    fn ready(code: &str) -> ListenEvent {
        ListenEvent::Ready {
            invite: invite(),
            fingerprint: Fingerprint::of(&vectors::verifying_key("alpha")),
            code: PairingCode::parse(code).unwrap(),
            relay: Relay::Disabled,
        }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn nothing_running_means_nothing_to_show() {
        let (_tmp, store) = store();
        assert_eq!(read(&store), Listening::No);
    }

    #[test]
    fn a_running_listen_shows_its_invite_and_the_current_code() {
        let (_tmp, store) = store();
        let mut board = Board::claim(&store, TTL).unwrap();
        assert!(board.is_publishing());
        board.on_event_at(&ready("111111"), at(1000)).unwrap();
        assert_eq!(
            read(&store),
            Listening::Yes(ListenStatus {
                invite: invite().to_string(),
                pairing: PairingStatus::Live {
                    code: "111111".into(),
                    expires_at: 1600
                },
            })
        );

        // Ten minutes on, the code renews by itself: nothing is printed, but
        // `whoami` sees the new one.
        board
            .on_event_at(
                &ListenEvent::NewCode {
                    code: PairingCode::parse("222222").unwrap(),
                    reason: NewCodeReason::Expired,
                },
                at(1600),
            )
            .unwrap();
        let Listening::Yes(status) = read(&store) else {
            panic!("not listening")
        };
        assert_eq!(
            status.pairing,
            PairingStatus::Live {
                code: "222222".into(),
                expires_at: 2200
            }
        );
        assert_eq!(status.invite, invite().to_string(), "the invite is kept");
    }

    #[test]
    fn attempts_pauses_and_switching_off_are_shown_without_a_code() {
        let (_tmp, store) = store();
        let mut board = Board::claim(&store, TTL).unwrap();
        board.on_event_at(&ready("111111"), at(1000)).unwrap();
        let peer = Fingerprint::of(&vectors::verifying_key("bravo"));
        let pairing = || match read(&store) {
            Listening::Yes(status) => status.pairing,
            other => panic!("{other:?}"),
        };

        board
            .on_event_at(&ListenEvent::PairingAttempt { peer }, at(1001))
            .unwrap();
        assert_eq!(pairing(), PairingStatus::InUse);

        board
            .on_event_at(
                &ListenEvent::PairingPaused {
                    failures: 1,
                    wait: Duration::from_secs(5),
                },
                at(1002),
            )
            .unwrap();
        assert_eq!(pairing(), PairingStatus::Paused { until: 1007 });

        board
            .on_event_at(&ListenEvent::PairingDisabled { failures: 3 }, at(1003))
            .unwrap();
        assert_eq!(pairing(), PairingStatus::Off { failures: 3 });
    }

    #[test]
    fn a_status_left_by_a_crash_is_not_believed() {
        let (_tmp, store) = store();
        {
            let mut board = Board::claim(&store, TTL).unwrap();
            board.on_event_at(&ready("111111"), at(1000)).unwrap();
            // Simulate a crash: the lock goes, the file stays.
            board.lock = None;
        }
        assert!(store.listen_status_path().exists(), "the file is left over");
        assert_eq!(read(&store), Listening::No);
    }

    #[test]
    fn a_clean_exit_takes_the_code_off_disk() {
        let (_tmp, store) = store();
        {
            let mut board = Board::claim(&store, TTL).unwrap();
            board.on_event_at(&ready("111111"), at(1000)).unwrap();
            assert!(store.listen_status_path().exists());
        }
        assert!(!store.listen_status_path().exists());
        assert_eq!(read(&store), Listening::No);
    }

    #[test]
    fn a_second_listen_in_the_same_home_does_not_overwrite_the_first() {
        let (_tmp, store) = store();
        let mut first = Board::claim(&store, TTL).unwrap();
        first.on_event_at(&ready("111111"), at(1000)).unwrap();

        let mut second = Board::claim(&store, TTL).unwrap();
        assert!(!second.is_publishing());
        second.on_event_at(&ready("999999"), at(1000)).unwrap();
        drop(second);

        match read(&store) {
            Listening::Yes(status) => assert_eq!(
                status.pairing,
                PairingStatus::Live {
                    code: "111111".into(),
                    expires_at: 1600
                }
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn events_before_ready_and_unrelated_events_change_nothing() {
        let (_tmp, store) = store();
        let mut board = Board::claim(&store, TTL).unwrap();
        board
            .on_event_at(&ListenEvent::PairingDisabled { failures: 3 }, at(1000))
            .unwrap();
        assert!(!store.listen_status_path().exists());
        board.on_event_at(&ready("111111"), at(1000)).unwrap();
        let peer = Fingerprint::of(&vectors::verifying_key("bravo"));
        board
            .on_event_at(&ListenEvent::TransferTurnedAway { peer }, at(1001))
            .unwrap();
        assert!(matches!(
            read(&store),
            Listening::Yes(ListenStatus {
                pairing: PairingStatus::Live { .. },
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn the_status_file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_tmp, store) = store();
        let mut board = Board::claim(&store, TTL).unwrap();
        board.on_event_at(&ready("111111"), at(1000)).unwrap();
        let mode = std::fs::metadata(store.listen_status_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
