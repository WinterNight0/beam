//! Sending a file from the full-screen view (ADR-0043, step 7): the shared
//! send worker (`beam_core::actions::send_worker`, also used by the GUI),
//! with the view's own words for how a send ended.
//!
//! Nothing about the transfer changes: the same dial, the same engine, the
//! same Accept on the other side before any byte moves, the same history
//! line. Cancelling closes the connection the ADR-0041 way, so the receiver
//! is told who stopped it; whatever it already has is kept for a resume.

use std::path::PathBuf;

use beam_core::actions::send::{SendError, Sent};
use beam_core::actions::send_worker::{self, SendEvent};
use beam_core::identity::{PeerError, Store};
use beam_core::transport::endpoint::Bind;
use beam_core::{text, untrusted};

/// What the sending thread tells the view: the model's own message.
pub use beam_model::sending::Update as SendUpdate;

/// A send running in the background, reporting in the view's words.
pub type Outgoing = send_worker::Outgoing<SendUpdate>;

/// Sends `path` to the friend called `peer`.
pub fn start(store: Store, peer: String, path: PathBuf) -> Outgoing {
    start_on(store, peer, path, Bind::Any)
}

/// `bind` is `Loopback` only in tests.
pub(super) fn start_on(store: Store, peer: String, path: PathBuf, bind: Bind) -> Outgoing {
    let name = peer.clone();
    Outgoing::start(store, peer, path, bind, move |event| word(&name, event))
}

/// What happened, as the view says it.
fn word(peer_name: &str, event: SendEvent) -> SendUpdate {
    match event {
        SendEvent::Dialling => SendUpdate::Dialling,
        SendEvent::Progress(progress) => SendUpdate::Progress(progress),
        SendEvent::Finished(result) => {
            SendUpdate::Finished(result.map(sent).map_err(|e| failed(peer_name, e)))
        }
    }
}

/// A send that arrived, in the view's words.
fn sent(sent: Sent) -> String {
    let summary = sent.summary;
    let saved = summary
        .final_name
        .as_deref()
        .map(untrusted::name)
        .unwrap_or_else(|| untrusted::name(&sent.file_name));
    let skipped = if summary.bytes_skipped > 0 {
        format!(
            " ({} was already there)",
            text::format_bytes(summary.bytes_skipped)
        )
    } else {
        String::new()
    };
    format!(
        "Sent {} to {}, saved on their side as {saved}{skipped}.",
        text::format_bytes(summary.bytes_sent),
        sent.peer
    )
}

/// A send that did not arrive, in the view's words.
fn failed(peer_name: &str, error: SendError) -> String {
    match error {
        SendError::Peer(PeerError::NotFound(_)) => {
            format!("You have no friend named {peer_name}.")
        }
        SendError::NotAFile(path) => format!("{} is not a file.", path.display()),
        SendError::Stopped { reached: true } => format!(
            "Cancelled. {peer_name} was told, and keeps what arrived: send the same file again to resume."
        ),
        SendError::Stopped { reached: false } => {
            format!("Cancelled before {peer_name} was reached; nothing was sent.")
        }
        // Worded and cleaned by the action already.
        SendError::Unreachable(message) | SendError::Refused(message) => message,
        other => untrusted::lines(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::inbox::InboxLink;
    use crate::tui::inbox::InboxUpdate;
    use crate::tui::inbox::tests::with_an_agent::{home, next, pair, start_agent};
    use beam_core::transfer::Progress;
    use std::time::Duration;

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
        let invite: beam_core::invite::Invite = status.invite.as_deref().unwrap().parse().unwrap();
        let mut known = bob.store.load_known_peers().unwrap();
        beam_core::invite::remember(
            known.lookup_key_mut(&invite.key).unwrap(),
            &invite,
            &beam_core::config::Relay::Disabled,
        );
        bob.store.save_known_peers(&known).unwrap();
        let link = InboxLink::connect(status.port, status.token.clone());
        next(&link, |u| matches!(u, InboxUpdate::Connected { .. })).await;

        let path = bob.files.join("report.pdf");
        std::fs::write(&path, b"the quarterly numbers").unwrap();
        let mut out = start_on(bob.store.clone(), "alice".into(), path, Bind::Loopback);
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
        let history = beam_core::history::read(&bob.store);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].direction, beam_core::history::Direction::Sent);
        assert_eq!(history[0].outcome, beam_core::history::Outcome::Done);
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
            beam_core::history::read(&bob.store)[0].outcome,
            beam_core::history::Outcome::Declined
        );
    }

    /// bob, with two friends whose agents are both up and reachable: a send
    /// meant for one must never reach the other (safety net for moving the
    /// send flow into one shared place).
    struct TwoFriends {
        alice: crate::tui::inbox::tests::with_an_agent::Home,
        alice_link: InboxLink,
        carol: crate::tui::inbox::tests::with_an_agent::Home,
        carol_link: InboxLink,
        bob: crate::tui::inbox::tests::with_an_agent::Home,
        /// Dropping these stops the agents.
        _stops: Vec<tokio::sync::oneshot::Sender<()>>,
    }

    async fn bob_with_two_friends() -> TwoFriends {
        let (alice, carol, bob) = (home("alice"), home("carol"), home("bob"));
        pair(&alice, "bob", &bob, "alice");
        pair(&carol, "bob", &bob, "carol");
        std::fs::write(bob.store.config_path(), "relay = \"none\"\n").unwrap();
        let mut stops = Vec::new();
        let mut links = Vec::new();
        for friend in [&alice, &carol] {
            let (status, stop) = start_agent(friend).await;
            stops.push(stop);
            let invite: beam_core::invite::Invite =
                status.invite.as_deref().unwrap().parse().unwrap();
            let mut known = bob.store.load_known_peers().unwrap();
            beam_core::invite::remember(
                known.lookup_key_mut(&invite.key).unwrap(),
                &invite,
                &beam_core::config::Relay::Disabled,
            );
            bob.store.save_known_peers(&known).unwrap();
            let link = InboxLink::connect(status.port, status.token.clone());
            next(&link, |u| matches!(u, InboxUpdate::Connected { .. })).await;
            links.push(link);
        }
        let carol_link = links.pop().unwrap();
        let alice_link = links.pop().unwrap();
        TwoFriends {
            alice,
            alice_link,
            carol,
            carol_link,
            bob,
            _stops: stops,
        }
    }

    /// The file is at carol's, nothing is at alice's, and alice was never
    /// even asked.
    fn only_carol_got_it(two: &TwoFriends, file: &str) {
        assert_eq!(
            std::fs::read(two.carol.inbox.join(file)).unwrap(),
            b"for carol only"
        );
        assert!(!two.alice.inbox.join(file).exists());
        while let Ok(update) = two.alice_link.updates.try_recv() {
            assert!(
                !matches!(update, InboxUpdate::Request { .. }),
                "alice was asked: {update:?}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_send_from_the_view_reaches_the_chosen_friend_and_no_other() {
        let two = bob_with_two_friends().await;
        let path = two.bob.files.join("for-carol.txt");
        std::fs::write(&path, b"for carol only").unwrap();
        let out = start_on(two.bob.store.clone(), "carol".into(), path, Bind::Loopback);
        let seen = until_finished(&out, &two.carol_link, Some(true)).await;
        assert!(
            matches!(seen.last(), Some(SendUpdate::Finished(Ok(_)))),
            "{seen:?}"
        );
        only_carol_got_it(&two, "for-carol.txt");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn beam_send_reaches_the_chosen_friend_and_no_other() {
        let two = bob_with_two_friends().await;
        let path = two.bob.files.join("for-carol.txt");
        std::fs::write(&path, b"for carol only").unwrap();
        let argv = vec![
            "--beam-dir".to_string(),
            two.bob.store.dir().display().to_string(),
            "send".to_string(),
            "carol".to_string(),
            path.display().to_string(),
            "--loopback".to_string(),
        ];
        // `beam send` runs its own runtime, so it gets a plain thread.
        let cli = std::thread::spawn(move || {
            let mut input: &[u8] = b"";
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = crate::cli::execute(
                argv,
                &mut crate::cli::Io {
                    input: &mut input,
                    out: &mut out,
                    err: &mut err,
                },
            );
            (code, String::from_utf8_lossy(&err).into_owned())
        });
        let InboxUpdate::Request { id, .. } = next(&two.carol_link, |u| {
            matches!(u, InboxUpdate::Request { .. })
        })
        .await
        else {
            unreachable!()
        };
        two.carol_link.answer(id, true);
        let (code, err) = tokio::task::spawn_blocking(move || cli.join().unwrap())
            .await
            .unwrap();
        assert_eq!(code, crate::cli::EXIT_OK, "{err}");
        only_carol_got_it(&two, "for-carol.txt");
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
            beam_core::history::read(&bob.store)[0].outcome,
            beam_core::history::Outcome::Cancelled
        );
    }
}
