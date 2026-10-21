//! Pairing for front ends that draw a screen: the full-screen view
//! (ADR-0043) and the GUI (ADR-0049). The same [`actions::pair::pair`] that
//! `beam pair` runs, on a background thread, with the screen asking the
//! questions.
//!
//! The thread sends [`Update`]s (the invite and code to show, "type the
//! code", "do the fingerprints match?") and the screen sends [`Answer`]s
//! back. Nothing about the protocol changes: the code is typed before
//! connecting, both people compare fingerprints, the key saved is the one the
//! connection proved, and **a decision left unanswered is a no**.
//!
//! Dropping the [`Worker`] cancels the pairing: the network attempt is
//! dropped, and any question still waiting reads as no. Nothing is saved
//! unless both people confirmed.
//!
//! "Show my invite" binds the configured port, which falls back to a random
//! one when the background agent or `listen` holds it: a temporary port, so
//! the agent itself still never pairs (ADR-0042).
//!
//! Like every action, it reports what happened, not words: a front end
//! words an [`Unpaired`] itself.
//!
//! [`actions::pair::pair`]: super::pair::pair

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::pair::{PairFailure, PairRequest};
use crate::identity::Store;
use crate::invite::Invite;
use crate::pairing::{
    Confirm, ConfirmRequest, Event, PairError, PairingCode, PairingError, Timeouts,
};
use crate::transport::endpoint::Bind;
use crate::untrusted;

/// How long the joiner has to type the code, as in `beam pair`.
const CODE_ENTRY_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What the pairing thread tells the screen.
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
    /// Ask whether the fingerprints match. Fingerprints are hex; `name` is
    /// what the new friend will be called here.
    AskConfirm {
        name: String,
        peer: String,
        own: String,
    },
    /// Paired and saved, or why not.
    Finished(Result<Paired, Unpaired>),
}

/// A friend just paired and saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paired {
    pub name: String,
    /// Hex, without `SHA256:`.
    pub fingerprint: String,
}

/// Why nothing was saved. Every string is already cleaned (ADR-0034): it
/// can quote the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unpaired {
    /// The pairing itself failed or was declined.
    NotPaired { reason: String, code: CodeAfter },
    /// Something before or after the pairing failed: no identity, an
    /// unreadable `known_peers`, a broken `config.toml`.
    Failed(String),
}

/// What a failed pairing did to the code, for the one hint that matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeAfter {
    /// Nothing to say: someone declined, or it never got that far.
    Nothing,
    /// The code this device showed is used up: show the invite again.
    UsedUpHere,
    /// The other device's code is used up: ask them for the new one.
    UsedUpThere,
}

/// What the screen answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The code typed, or `None` to give up.
    Code(Option<String>),
    Confirm(bool),
}

/// Whom to pair with.
pub enum Role {
    /// Pair with the device in `invite`, saving it as `name`.
    Join { invite: Box<Invite>, name: String },
    /// Show this device's invite and a code, and take one attempt. The new
    /// friend is named after what their device suggests.
    Wait,
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
            Bind::Any,
        )
    }

    /// Shows this device's invite and a code, and takes one attempt.
    pub fn wait(store: Store) -> Self {
        Self::start(store, Role::Wait, Bind::Any)
    }

    /// `bind` is `Loopback` only in tests, so two workers on one machine
    /// pair without the network.
    pub fn start(store: Store, role: Role, bind: Bind) -> Self {
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

/// The pairing question, asked by the screen.
#[derive(Clone)]
struct ScreenConfirm {
    updates: Sender<Update>,
    answers: Arc<Mutex<Receiver<Answer>>>,
    timeout: Duration,
}

impl Confirm for ScreenConfirm {
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
                // Unanswered, or the screen went away: no.
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
) -> Result<Paired, Unpaired> {
    let fail = |e: &dyn std::fmt::Display| Unpaired::Failed(untrusted::lines(&e.to_string()));
    let answers = Arc::new(Mutex::new(answers));
    let confirm = ScreenConfirm {
        updates: updates.clone(),
        answers: Arc::clone(&answers),
        timeout: Timeouts::default().decision,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(&e))?;

    let (name, invite) = match &role {
        Role::Join { invite, name } => (Some(name.as_str()), Some(&**invite)),
        Role::Wait => (None, None),
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
    let ask_code = || {
        let updates = updates.clone();
        let answers = Arc::clone(&answers);
        async move {
            tokio::task::spawn_blocking(move || read_code(&updates, &answers))
                .await
                .map_err(std::io::Error::other)?
        }
    };
    let request = PairRequest { invite, name, bind };
    let result = runtime.block_on(super::pair::pair(
        store,
        request,
        confirm,
        ask_code,
        send_event,
        // Dropping the worker counts too: the screen went away.
        async {
            let _ = cancelled.await;
        },
    ));
    // A question still waiting must not hold the thread: the screen has gone.
    runtime.shutdown_background();

    match result {
        Ok(friend) => Ok(Paired {
            name: friend.name,
            fingerprint: friend.fingerprint.hex(),
        }),
        Err(PairFailure::NotPaired(e)) => Err(not_paired(&e, invite.is_none())),
        Err(other) => Err(fail(&other)),
    }
}

/// Asks the screen for the code. The screen checks its form before
/// answering, so a malformed code never reaches the network.
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

/// Why pairing failed, and what it did to the code, as `beam pair` tells it.
fn not_paired(error: &PairError, waited: bool) -> Unpaired {
    let code = match (error, waited) {
        (PairError::Pairing(PairingError::Declined | PairingError::DeclinedByPeer), _) => {
            CodeAfter::Nothing
        }
        (PairError::Pairing(_), true) => CodeAfter::UsedUpHere,
        (PairError::Pairing(_), false) => CodeAfter::UsedUpThere,
        _ => CodeAfter::Nothing,
    };
    Unpaired::NotPaired {
        reason: untrusted::lines(&error.to_string()),
        code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unanswered_or_abandoned_question_is_a_no() {
        let (updates, _seen) = mpsc::channel();
        let (answers, rx) = mpsc::channel::<Answer>();
        let mut confirm = ScreenConfirm {
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
        assert!(!confirm.confirm(&request).unwrap(), "the screen went away");
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
            Update::Finished(Err(Unpaired::Failed(reason))) => {
                assert!(reason.contains("beam init"), "{reason}");
            }
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
        std::fs::write(store.config_path(), "relay = \"none\"\nport = 0\n").unwrap();
        (dir, store)
    }

    fn join(store: &Store, invite: &str, name: &str) -> Worker {
        Worker::start(
            store.clone(),
            Role::Join {
                invite: Box::new(invite.parse().unwrap()),
                name: name.to_string(),
            },
            Bind::Loopback,
        )
    }

    #[test]
    fn two_screens_pair_through_the_real_protocol_and_both_save() {
        let (_a, waiter_home) = home("lab-pc");
        let (_b, joiner_home) = home("laptop");

        let waiter = Worker::start(waiter_home.clone(), Role::Wait, Bind::Loopback);
        let (invite, code) = match next(&waiter) {
            Update::Waiting { invite, code, .. } => (invite, code),
            other => panic!("{other:?}"),
        };
        let joiner = join(&joiner_home, &invite, "lab");
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
            matches!(&joined, Update::Finished(Ok(p)) if p.name == "lab"),
            "{joined:?}"
        );
        assert!(
            matches!(&waited, Update::Finished(Ok(p)) if p.name == "laptop"),
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
        let waiter = Worker::start(waiter_home.clone(), Role::Wait, Bind::Loopback);
        let Update::Waiting { invite, code, .. } = next(&waiter) else {
            panic!("no invite")
        };
        let joiner = join(&joiner_home, &invite, "lab");
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

    /// A wrong code uses up the waiter's code, and each side is told which
    /// code is gone, so the hint can say what to do next.
    #[test]
    fn a_wrong_code_says_whose_code_is_used_up() {
        let (_a, waiter_home) = home("lab-pc");
        let (_b, joiner_home) = home("laptop");
        let waiter = Worker::start(waiter_home.clone(), Role::Wait, Bind::Loopback);
        let Update::Waiting { invite, code, .. } = next(&waiter) else {
            panic!("no invite")
        };
        let digits: u32 = code.replace(' ', "").parse().unwrap();
        let wrong = format!("{:06}", (digits + 1) % 1_000_000);
        let joiner = join(&joiner_home, &invite, "lab");
        assert_eq!(next(&joiner), Update::AskCode);
        joiner.answer(Answer::Code(Some(wrong)));

        let code_after = |update: Update| match update {
            Update::Finished(Err(Unpaired::NotPaired { code, .. })) => code,
            other => panic!("{other:?}"),
        };
        assert_eq!(code_after(next(&joiner)), CodeAfter::UsedUpThere);
        assert_eq!(code_after(next(&waiter)), CodeAfter::UsedUpHere);
        assert!(joiner_home.load_known_peers().unwrap().is_empty());
        assert!(waiter_home.load_known_peers().unwrap().is_empty());
    }
}
