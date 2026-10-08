//! Sending a file from the full-screen view (ADR-0043, step 7): what
//! `beam send` does, on a background thread, with its progress drawn in
//! the view instead of printed.
//!
//! Nothing about the transfer changes: the same dial, the same engine, the
//! same Accept on the other side before any byte moves, the same history
//! line. Cancelling closes the connection the ADR-0041 way, so the receiver
//! is told who stopped it; whatever it already has is kept for a resume.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use crate::config::Config;
use crate::history;
use crate::identity::{Fingerprint, Store, encode_public_key};
use crate::invite;
use crate::transfer::{Progress, Reporter, SendOptions};
use crate::transport::dial::{dial_transfer, interrupt, send_on};
use crate::transport::endpoint::{self, Bind};
use crate::{ui, untrusted};

/// What the sending thread tells the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendUpdate {
    /// Looking for the friend.
    Dialling,
    Progress(Progress),
    /// How it ended, in words for the person.
    Finished(Result<String, String>),
}

/// A send running in the background.
pub struct Outgoing {
    pub updates: Receiver<SendUpdate>,
    /// Dropped to cancel.
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Outgoing {
    pub fn start(store: Store, peer: String, path: PathBuf) -> Self {
        Self::start_on(store, peer, path, Bind::Any)
    }

    /// `bind` is `Loopback` only in tests.
    fn start_on(store: Store, peer: String, path: PathBuf, bind: Bind) -> Self {
        let (update_tx, updates) = mpsc::channel();
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let result = run(&store, &peer, path, bind, &update_tx, cancelled);
            let _ = update_tx.send(SendUpdate::Finished(result));
        });
        Self {
            updates,
            cancel: Some(cancel),
        }
    }

    /// Stops the send; the receiver is told, and keeps what it has.
    pub fn cancel(&mut self) {
        self.cancel.take();
    }

    /// Cancels, and waits a little for the receiver to be told, so leaving
    /// beam mid-send does not just vanish from under the other side.
    pub fn cancel_and_wait(mut self, patience: Duration) {
        self.cancel();
        let deadline = std::time::Instant::now() + patience;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match self.updates.recv_timeout(left) {
                Ok(SendUpdate::Finished(_)) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }
}

/// Passes the engine's progress to the view.
struct ToView<'a>(&'a Sender<SendUpdate>);

impl Reporter for ToView<'_> {
    fn report(&mut self, progress: Progress) {
        let _ = self.0.send(SendUpdate::Progress(progress));
    }
}

fn run(
    store: &Store,
    peer_name: &str,
    path: PathBuf,
    bind: Bind,
    updates: &Sender<SendUpdate>,
    cancelled: tokio::sync::oneshot::Receiver<()>,
) -> Result<String, String> {
    let fail = |e: &dyn std::fmt::Display| untrusted::lines(&e.to_string());
    let identity = store.load_identity().map_err(|e| fail(&e))?;
    let known = store.load_known_peers().map_err(|e| fail(&e))?;
    let peer = known
        .lookup(peer_name)
        .ok_or_else(|| format!("You have no friend named {peer_name}."))?;
    let (peer_name, peer_key) = (peer.name.clone(), peer.public_key);
    let config = Config::load(&store.config_path()).map_err(|e| fail(&e))?;
    let peer_addr = invite::peer_addr(peer, &config.relay);
    if !path.is_file() {
        return Err(format!("{} is not a file.", path.display()));
    }
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut options = SendOptions::new(&path, encode_public_key(&identity.verifying_key()));
    // As `beam send`: outlast the longest receiver's question (ADR-0042).
    options.accept_timeout = crate::agent::AGENT_ACCEPT_TIMEOUT + Duration::from_secs(15);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(&e))?;
    let mut ended = None;
    let result = runtime.block_on(async {
        let endpoint = endpoint::bind(&identity, &config.relay, bind, 0, &[])
            .await
            .map_err(|e| fail(&e))?;
        let live = std::sync::Mutex::new(None);
        let attempt = async {
            let _ = updates.send(SendUpdate::Dialling);
            let connection = dial_transfer(&endpoint, peer_addr).await.map_err(|e| {
                ended = Some((history::Outcome::Failed, Some("not reachable".to_string())));
                crate::cli::net_cmds::unreachable_message(&peer_name, &peer_key, e).to_string()
            })?;
            *live.lock().expect("not poisoned") = Some(connection.clone());
            let sent = send_on(&connection, &mut options, &mut ToView(updates)).await;
            ended = Some(history::send_outcome(&sent));
            sent.map_err(|e| crate::cli::net_cmds::refused_message(&peer_name, e).to_string())
        };
        let outcome = tokio::select! {
            outcome = attempt => outcome,
            _ = cancelled => {
                let connection = live.lock().expect("not poisoned").take();
                if let Some(connection) = &connection {
                    interrupt(connection);
                }
                ended = Some(history::stopped_outcome(connection.is_some()));
                Err(if connection.is_some() {
                    format!("Cancelled. {peer_name} was told, and keeps what arrived: send the same file again to resume.")
                } else {
                    format!("Cancelled before {peer_name} was reached; nothing was sent.")
                })
            }
        };
        endpoint.close().await;
        outcome
    });
    runtime.shutdown_timeout(Duration::from_secs(1));

    if let Some((outcome, note)) = ended {
        history::record(
            store,
            &history::Entry::now(
                history::Direction::Sent,
                &peer_name,
                &Fingerprint::of(&peer_key).hex(),
                &file_name,
                size,
                outcome,
                note,
            ),
        );
    }
    let summary = result?;
    let saved = summary
        .final_name
        .as_deref()
        .map(untrusted::name)
        .unwrap_or_else(|| untrusted::name(&file_name));
    let skipped = if summary.bytes_skipped > 0 {
        format!(
            " ({} was already there)",
            ui::format_bytes(summary.bytes_skipped)
        )
    } else {
        String::new()
    };
    Ok(format!(
        "Sent {} to {peer_name}, saved on their side as {saved}{skipped}.",
        ui::format_bytes(summary.bytes_sent)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::inbox::InboxLink;
    use crate::tui::inbox::InboxUpdate;
    use crate::tui::inbox::tests::with_an_agent::{home, next, pair, start_agent};

    /// Every update until the send ends, within a generous time.
    async fn until_finished(
        out: &Outgoing,
        link: &InboxLink,
        answer: Option<bool>,
    ) -> Vec<SendUpdate> {
        let mut seen = Vec::new();
        for _ in 0..600 {
            while let Ok(update) = link.updates.try_recv() {
                if let (InboxUpdate::Request { id, .. }, Some(accept)) = (&update, answer) {
                    link.answer(*id, accept);
                }
            }
            while let Ok(update) = out.updates.try_recv() {
                let done = matches!(update, SendUpdate::Finished(_));
                seen.push(update);
                if done {
                    return seen;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the send never ended: {seen:?}");
    }

    /// bob's view sends to alice's agent.
    async fn bob_sends(
        answer: Option<bool>,
        cancel_while_waiting: bool,
    ) -> (
        crate::tui::inbox::tests::with_an_agent::Home,
        Vec<SendUpdate>,
        crate::tui::inbox::tests::with_an_agent::Home,
    ) {
        let (alice, bob) = (home("alice"), home("bob"));
        pair(&alice, "bob", &bob, "alice");
        std::fs::write(
            bob.store.config_path(),
            "relay = \"none\"
",
        )
        .unwrap();
        let (status, _stop) = start_agent(&alice).await;
        // bob knows where alice is, as after pairing with her invite.
        let invite: crate::invite::Invite = status.invite.as_deref().unwrap().parse().unwrap();
        let mut known = bob.store.load_known_peers().unwrap();
        crate::invite::remember(
            known.lookup_key_mut(&invite.key).unwrap(),
            &invite,
            &crate::config::Relay::Disabled,
        );
        bob.store.save_known_peers(&known).unwrap();
        let link = InboxLink::connect(status.port, status.token.clone());
        next(&link, |u| matches!(u, InboxUpdate::Connected { .. })).await;

        let path = bob.files.join("report.pdf");
        std::fs::write(&path, b"the quarterly numbers").unwrap();
        let mut out = Outgoing::start_on(bob.store.clone(), "alice".into(), path, Bind::Loopback);
        if cancel_while_waiting {
            next(&link, |u| matches!(u, InboxUpdate::Request { .. })).await;
            out.cancel();
        }
        let seen = until_finished(&out, &link, answer).await;
        (alice, seen, bob)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_send_from_the_view_waits_for_the_yes_then_arrives() {
        let (alice, seen, bob) = bob_sends(Some(true), false).await;
        assert!(seen.contains(&SendUpdate::Dialling), "{seen:?}");
        assert!(
            seen.contains(&SendUpdate::Progress(Progress::AwaitingAccept)),
            "{seen:?}"
        );
        let Some(SendUpdate::Finished(Ok(message))) = seen.last() else {
            panic!("{seen:?}")
        };
        assert!(
            message.contains("saved on their side as report.pdf"),
            "{message}"
        );
        assert_eq!(
            std::fs::read(alice.inbox.join("report.pdf")).unwrap(),
            b"the quarterly numbers"
        );
        let history = crate::history::read(&bob.store);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].direction, crate::history::Direction::Sent);
        assert_eq!(history[0].outcome, crate::history::Outcome::Done);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_decline_is_said_plainly_and_nothing_arrives() {
        let (alice, seen, bob) = bob_sends(Some(false), false).await;
        let Some(SendUpdate::Finished(Err(message))) = seen.last() else {
            panic!("{seen:?}")
        };
        assert!(message.contains("declined"), "{message}");
        assert!(!alice.inbox.join("report.pdf").exists());
        assert_eq!(
            crate::history::read(&bob.store)[0].outcome,
            crate::history::Outcome::Declined
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelling_while_they_decide_tells_them_and_is_recorded() {
        let (alice, seen, bob) = bob_sends(None, true).await;
        let Some(SendUpdate::Finished(Err(message))) = seen.last() else {
            panic!("{seen:?}")
        };
        assert!(message.starts_with("Cancelled"), "{message}");
        assert!(!alice.inbox.join("report.pdf").exists());
        assert_eq!(
            crate::history::read(&bob.store)[0].outcome,
            crate::history::Outcome::Cancelled
        );
    }
}
