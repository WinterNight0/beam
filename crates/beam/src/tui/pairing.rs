//! Pairing from the full-screen view (ADR-0043): the same
//! [`pairing::join`](crate::pairing::join) and [`wait`](crate::pairing::wait)
//! that `beam pair` runs, on a background thread, with the view asking the
//! questions.
//!
//! The thread sends [`Update`]s — the invite and code to show, "type the
//! code", "do the fingerprints match?" — and the view sends [`Answer`]s
//! back. Nothing about the protocol changes: the code is typed before
//! connecting, both people compare fingerprints, the key saved is the one the
//! connection proved, and a decision left unanswered is a no.
//!
//! Dropping the [`Worker`] cancels the pairing: the network attempt is
//! dropped, and any question still waiting reads as no. Nothing is saved
//! unless both people confirmed.
//!
//! "Show my invite" binds the configured port, which falls back to a random
//! one when the background agent or `listen` holds it: a temporary port, so
//! the agent itself still never pairs (ADR-0042).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::Config;
use crate::identity::{Peer, Store};
use crate::invite::{self, Invite};
use crate::pairing::{
    CodeSlot, Confirm, ConfirmRequest, Event, Network, PairError, PairingCode, PairingError,
    Timeouts, join, wait,
};
use crate::transport::endpoint::Bind;
use crate::untrusted;

/// How long the joiner has to type the code, as in `beam pair`.
const CODE_ENTRY_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What the pairing thread tells the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    /// Show this invite and code; the code expires after `expires_in`.
    Waiting {
        invite: String,
        code: String,
        expires_in: Duration,
    },
    /// A device connected to the shown invite; the code is now used up.
    Attempt { peer: String },
    /// Connecting to the device in the pasted invite.
    Connecting { peer: String },
    /// Ask for the code shown on the other device.
    AskCode,
    /// Ask whether the fingerprints match. Fingerprints are hex.
    AskConfirm {
        name: String,
        peer: String,
        own: String,
    },
    /// Paired and saved (name, fingerprint hex), or why not.
    Finished(Result<(String, String), String>),
}

/// What the view answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The code typed, or `None` to give up.
    Code(Option<String>),
    Confirm(bool),
}

/// A pairing running in the background.
pub struct Worker {
    pub updates: Receiver<Update>,
    answers: Sender<Answer>,
    /// Dropped with the worker, which cancels the pairing.
    _cancel: tokio::sync::oneshot::Sender<()>,
}

impl Worker {
    pub fn answer(&self, answer: Answer) {
        let _ = self.answers.send(answer);
    }

    /// Pairs with the device in `invite`, saving it as `name`.
    pub fn join(store: Store, invite: Invite, name: String) -> Self {
        Self::start(
            store,
            Role::Join {
                invite: Box::new(invite),
                name,
            },
        )
    }

    /// Shows this device's invite and a code, and takes one attempt. The new
    /// friend is named after what their device suggests.
    pub fn wait(store: Store) -> Self {
        Self::start(store, Role::Wait)
    }

    fn start(store: Store, role: Role) -> Self {
        Self::start_on(store, role, Bind::Any)
    }

    /// `bind` is `Loopback` only in tests, so two workers on one machine
    /// pair without the network.
    fn start_on(store: Store, role: Role, bind: Bind) -> Self {
        let (update_tx, updates) = mpsc::channel();
        let (answers, answer_rx) = mpsc::channel();
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let result = run(&store, role, bind, &update_tx, answer_rx, cancelled);
            let _ = update_tx.send(Update::Finished(result));
        });
        Self {
            updates,
            answers,
            _cancel: cancel,
        }
    }
}

enum Role {
    Join { invite: Box<Invite>, name: String },
    Wait,
}

/// The pairing question, asked by the view.
#[derive(Clone)]
struct ViewConfirm {
    updates: Sender<Update>,
    answers: Arc<Mutex<Receiver<Answer>>>,
    timeout: Duration,
}

impl Confirm for ViewConfirm {
    fn confirm(&mut self, request: &ConfirmRequest) -> std::io::Result<bool> {
        let _ = self.updates.send(Update::AskConfirm {
            name: request.name.clone(),
            peer: request.peer_fingerprint.hex(),
            own: request.own_fingerprint.hex(),
        });
        let answers = self.answers.lock().expect("not poisoned");
        loop {
            match answers.recv_timeout(self.timeout) {
                Ok(Answer::Confirm(yes)) => return Ok(yes),
                Ok(Answer::Code(_)) => continue,
                // Unanswered, or the view went away: no.
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    return Ok(false);
                }
            }
        }
    }
}

fn run(
    store: &Store,
    role: Role,
    bind: Bind,
    updates: &Sender<Update>,
    answers: Receiver<Answer>,
    cancelled: tokio::sync::oneshot::Receiver<()>,
) -> Result<(String, String), String> {
    let fail = |e: &dyn std::fmt::Display| untrusted::lines(&e.to_string());
    let identity = store.load_identity().map_err(|e| fail(&e))?;
    let known = store.load_known_peers().map_err(|e| fail(&e))?;
    let config = Config::load(&store.config_path()).map_err(|e| fail(&e))?;
    let network = Network {
        relay: config.relay,
        bind,
        port: config.port,
        advertise: config.advertise,
    };
    let timeouts = Timeouts::default();
    let answers = Arc::new(Mutex::new(answers));
    let confirm = ViewConfirm {
        updates: updates.clone(),
        answers: Arc::clone(&answers),
        timeout: timeouts.decision,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(&e))?;

    let (name, invite) = match &role {
        Role::Join { invite, name } => (Some(name.as_str()), Some(&**invite)),
        Role::Wait => (None, None),
    };
    let pairing = crate::pairing::Pairing {
        identity: &identity,
        known: &known,
        name,
        network: &network,
        timeouts,
    };
    let send_event = |event: Event<'_>| {
        let update = match event {
            Event::Waiting {
                invite,
                code,
                expires_in,
            } => Update::Waiting {
                invite: invite.to_string(),
                code: code.grouped(),
                expires_in,
            },
            Event::Attempt { peer } => Update::Attempt { peer: peer.short() },
            Event::Connecting { peer } => Update::Connecting { peer: peer.short() },
        };
        let _ = updates.send(update);
    };

    let result = runtime.block_on(async {
        let pairing_run = async {
            match invite {
                None => {
                    let code = PairingCode::generate()
                        .map_err(|e| PairError::Io(std::io::Error::other(e)))?;
                    wait(
                        &pairing,
                        CodeSlot::new(code, timeouts.code_ttl),
                        confirm,
                        send_event,
                    )
                    .await
                }
                Some(invite) => {
                    let ask_code = || {
                        let updates = updates.clone();
                        let answers = Arc::clone(&answers);
                        async move {
                            tokio::task::spawn_blocking(move || read_code(&updates, &answers))
                                .await
                                .map_err(std::io::Error::other)?
                        }
                    };
                    join(&pairing, invite, ask_code, confirm, send_event).await
                }
            }
        };
        tokio::select! {
            result = pairing_run => result,
            _ = cancelled => Err(PairError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ))),
        }
    });
    // A question still waiting must not hold the thread: the view has gone.
    runtime.shutdown_background();

    let paired = result.map_err(|e| not_paired(&e, invite.is_none()))?;
    // Read the file again: it may have changed while the question was up.
    let mut known = store.load_known_peers().map_err(|e| fail(&e))?;
    let mut peer = Peer::new(paired.name.clone(), paired.key);
    if let Some(invite) = invite {
        invite::remember(&mut peer, invite, &network.relay);
    }
    let fingerprint = peer.fingerprint().hex();
    known.add(peer).map_err(|e| fail(&e))?;
    store.save_known_peers(&known).map_err(|e| fail(&e))?;
    Ok((paired.name, fingerprint))
}

/// Asks the view for the code. The view checks its form before answering,
/// so a malformed code never reaches the network.
fn read_code(
    updates: &Sender<Update>,
    answers: &Mutex<Receiver<Answer>>,
) -> std::io::Result<PairingCode> {
    let _ = updates.send(Update::AskCode);
    let answers = answers.lock().expect("not poisoned");
    loop {
        match answers.recv_timeout(CODE_ENTRY_TIMEOUT) {
            Ok(Answer::Code(Some(text))) => {
                return PairingCode::parse(&text)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e));
            }
            Ok(Answer::Confirm(_)) => continue,
            Ok(Answer::Code(None)) | Err(RecvTimeoutError::Disconnected) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "no pairing code was entered",
                ));
            }
        }
    }
}

/// Why pairing failed, with the one hint that matters, as `beam pair` says it.
fn not_paired(error: &PairError, waited: bool) -> String {
    let hint = match (error, waited) {
        (PairError::Pairing(PairingError::Declined | PairingError::DeclinedByPeer), _) => "",
        (PairError::Pairing(_), true) => {
            "\nThe code is used up. Choose Show my invite again for a new one."
        }
        (PairError::Pairing(_), false) => {
            "\nThe other device's code is now used up. Ask them for the new one."
        }
        _ => "",
    };
    untrusted::lines(&format!("Not paired: {error}. Nothing was saved.{hint}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unanswered_or_abandoned_question_is_a_no() {
        let (updates, _seen) = mpsc::channel();
        let (answers, rx) = mpsc::channel::<Answer>();
        let mut confirm = ViewConfirm {
            updates,
            answers: Arc::new(Mutex::new(rx)),
            timeout: Duration::from_millis(50),
        };
        let request = ConfirmRequest {
            role: crate::pairing::Role::Joiner,
            name: "alice".to_string(),
            peer_fingerprint: crate::identity::Identity::generate("a")
                .unwrap()
                .fingerprint(),
            own_fingerprint: crate::identity::Identity::generate("b")
                .unwrap()
                .fingerprint(),
        };
        assert!(!confirm.confirm(&request).unwrap(), "timed out");

        answers.send(Answer::Confirm(true)).unwrap();
        assert!(confirm.confirm(&request).unwrap());

        drop(answers);
        assert!(!confirm.confirm(&request).unwrap(), "the view went away");
    }

    #[test]
    fn a_cancelled_code_question_stops_the_join() {
        let (updates, seen) = mpsc::channel();
        let (answers, rx) = mpsc::channel();
        answers.send(Answer::Code(None)).unwrap();
        let err = read_code(&updates, &Mutex::new(rx)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(seen.recv().unwrap(), Update::AskCode);
    }

    #[test]
    fn a_failure_with_no_identity_comes_back_as_finished() {
        let dir = tempfile::tempdir().unwrap();
        let worker = Worker::wait(Store::new(dir.path()));
        match worker
            .updates
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
        {
            Update::Finished(Err(reason)) => assert!(reason.contains("beam init"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    /// The next update, skipping progress, within a generous time.
    fn next(worker: &Worker) -> Update {
        loop {
            match worker.updates.recv_timeout(Duration::from_secs(30)) {
                Ok(Update::Attempt { .. } | Update::Connecting { .. }) => continue,
                Ok(update) => return update,
                Err(e) => panic!("no update: {e}"),
            }
        }
    }

    fn home(name: &str) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let identity = crate::identity::Identity::generate(name).unwrap();
        store.save_identity(&identity, false).unwrap();
        std::fs::write(
            store.config_path(),
            "relay = \"none\"
port = 0
",
        )
        .unwrap();
        (dir, store)
    }

    #[test]
    fn two_views_pair_through_the_real_protocol_and_both_save() {
        let (_a, waiter_home) = home("lab-pc");
        let (_b, joiner_home) = home("laptop");

        let waiter = Worker::start_on(waiter_home.clone(), Role::Wait, Bind::Loopback);
        let (invite, code) = match next(&waiter) {
            Update::Waiting { invite, code, .. } => (invite, code),
            other => panic!("{other:?}"),
        };
        let joiner = Worker::start_on(
            joiner_home.clone(),
            Role::Join {
                invite: Box::new(invite.parse().unwrap()),
                name: "lab".to_string(),
            },
            Bind::Loopback,
        );
        assert_eq!(next(&joiner), Update::AskCode);
        joiner.answer(Answer::Code(Some(code)));

        // Both people see the same pair of fingerprints, each from their side.
        let (joiner_sees, waiter_sees) = (next(&joiner), next(&waiter));
        match (&joiner_sees, &waiter_sees) {
            (
                Update::AskConfirm {
                    name,
                    peer: jp,
                    own: jo,
                },
                Update::AskConfirm {
                    name: wn,
                    peer: wp,
                    own: wo,
                },
            ) => {
                assert_eq!(name, "lab");
                assert_eq!(
                    wn, "laptop",
                    "the waiter names the friend after their device"
                );
                assert_eq!((jp, jo), (wo, wp));
            }
            other => panic!("{other:?}"),
        }
        joiner.answer(Answer::Confirm(true));
        waiter.answer(Answer::Confirm(true));

        let joined = next(&joiner);
        let waited = next(&waiter);
        assert!(
            matches!(&joined, Update::Finished(Ok((name, _))) if name == "lab"),
            "{joined:?}"
        );
        assert!(
            matches!(&waited, Update::Finished(Ok((name, _))) if name == "laptop"),
            "{waited:?}"
        );
        assert!(
            joiner_home
                .load_known_peers()
                .unwrap()
                .lookup("lab")
                .is_some()
        );
        assert!(
            waiter_home
                .load_known_peers()
                .unwrap()
                .lookup("laptop")
                .is_some()
        );
    }

    #[test]
    fn a_no_on_either_side_saves_nothing_on_both() {
        let (_a, waiter_home) = home("lab-pc");
        let (_b, joiner_home) = home("laptop");
        let waiter = Worker::start_on(waiter_home.clone(), Role::Wait, Bind::Loopback);
        let Update::Waiting { invite, code, .. } = next(&waiter) else {
            panic!("no invite")
        };
        let joiner = Worker::start_on(
            joiner_home.clone(),
            Role::Join {
                invite: Box::new(invite.parse().unwrap()),
                name: "lab".to_string(),
            },
            Bind::Loopback,
        );
        assert_eq!(next(&joiner), Update::AskCode);
        joiner.answer(Answer::Code(Some(code)));
        assert!(matches!(next(&joiner), Update::AskConfirm { .. }));
        assert!(matches!(next(&waiter), Update::AskConfirm { .. }));
        joiner.answer(Answer::Confirm(true));
        waiter.answer(Answer::Confirm(false));

        assert!(matches!(next(&joiner), Update::Finished(Err(_))));
        assert!(matches!(next(&waiter), Update::Finished(Err(_))));
        assert!(joiner_home.load_known_peers().unwrap().is_empty());
        assert!(waiter_home.load_known_peers().unwrap().is_empty());
    }
}
