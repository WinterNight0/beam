//! The view's link to the background agent: what `beam inbox` does, for the
//! Pending tab (ADR-0043, step 5; ADR-0042).
//!
//! A thread connects to the agent over loopback with the token from the
//! private `agent.json`, exactly as `beam inbox` does, and passes on what the
//! agent says. The view answers a request only from its Accept pop-up, which
//! starts on Decline. The agent still decides what counts: one answer per
//! request, the first wins, and a late one is refused (S-39).
//!
//! Everything a request carries is cleaned for the terminal here, once
//! (ADR-0034): the file name comes from the sender.

//! (The link itself is `beam_core::actions::inbox_link`, shared with the
//! GUI; this module names it as the view uses it.)

pub use beam_core::actions::inbox::RequestView;
pub use beam_core::actions::inbox_link::Link as InboxLink;

/// What the agent said: the model's own message (beam-model).
pub use beam_model::requests::Update as InboxUpdate;

#[cfg(test)]
pub(super) mod tests {
    // The whole path with a real agent: a paired device sends, the view's
    // link is shown the request, and only its answer decides.
    pub(in crate::tui) mod with_an_agent {
        use std::path::PathBuf;
        use std::time::Duration;

        use super::super::*;
        use beam_core::agent::status::{self, Running};
        use beam_core::agent::{self, AgentOptions};
        use beam_core::config::Relay;
        use beam_core::identity::{Identity, Peer, Store};
        use beam_core::invite::Invite;
        use beam_core::pairing::Network;
        use beam_core::transfer::{SendOptions, SilentReporter, TransferError};
        use beam_core::transport::dial::{dial_transfer, send_on};
        use beam_core::transport::endpoint::{self, Bind};

        pub(in crate::tui) struct Home {
            _tmp: tempfile::TempDir,
            pub store: Store,
            pub identity: Identity,
            pub files: PathBuf,
            pub inbox: PathBuf,
        }

        pub(in crate::tui) fn home(name: &str) -> Home {
            let tmp = tempfile::tempdir().unwrap();
            let store = Store::new(tmp.path().join("beam"));
            let identity = Identity::generate(name).unwrap();
            store.save_identity(&identity, false).unwrap();
            let files = tmp.path().join("files");
            let inbox = tmp.path().join("downloads");
            std::fs::create_dir_all(&files).unwrap();
            Home {
                _tmp: tmp,
                store,
                identity,
                files,
                inbox,
            }
        }

        pub(in crate::tui) fn pair(a: &Home, a_calls_b: &str, b: &Home, b_calls_a: &str) {
            for (store, name, key) in [
                (&a.store, a_calls_b, b.identity.verifying_key()),
                (&b.store, b_calls_a, a.identity.verifying_key()),
            ] {
                let mut known = store.load_known_peers().unwrap();
                known.add(Peer::new(name, key)).unwrap();
                store.save_known_peers(&known).unwrap();
            }
        }

        /// Starts the agent and returns its status once its invite is up.
        pub(in crate::tui) async fn start_agent(
            home: &Home,
        ) -> (status::AgentStatus, tokio::sync::oneshot::Sender<()>) {
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let options = AgentOptions {
                network: Network {
                    relay: Relay::Disabled,
                    bind: Bind::Loopback,
                    port: 0,
                    advertise: Vec::new(),
                },
                receive_dir: home.inbox.clone(),
                accept_timeout: Duration::from_secs(60),
                port_mapping: false,
                notify: false,
                echo: None,
                in_view: false,
            };
            let (identity, store) = (home.identity.clone(), home.store.clone());
            tokio::spawn(async move {
                agent::run(identity, store, options, async {
                    let _ = stopped.await;
                })
                .await
            });
            for _ in 0..600 {
                if let Running::Yes(status) = status::read(&home.store)
                    && status.invite.is_some()
                {
                    return (status, stop);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("the agent never came up");
        }

        fn send(
            from: &Home,
            at: iroh::EndpointAddr,
            bytes: &[u8],
        ) -> tokio::task::JoinHandle<Result<beam_core::transfer::SendSummary, TransferError>>
        {
            let path = from.files.join("report.pdf");
            std::fs::write(&path, bytes).unwrap();
            let identity = from.identity.clone();
            tokio::spawn(async move {
                let endpoint = endpoint::bind(&identity, &Relay::Disabled, Bind::Loopback, 0, &[])
                    .await
                    .unwrap();
                let connection = dial_transfer(&endpoint, at).await.expect("dial the agent");
                let mut options = SendOptions::new(
                    path,
                    beam_core::identity::encode_public_key(&identity.verifying_key()),
                );
                options.accept_timeout = Duration::from_secs(60);
                let result = send_on(&connection, &mut options, &mut SilentReporter).await;
                endpoint.close().await;
                result
            })
        }

        /// The next update `want` matches, within a generous time.
        pub(in crate::tui) async fn next(
            link: &InboxLink,
            want: impl Fn(&InboxUpdate) -> bool,
        ) -> InboxUpdate {
            for _ in 0..600 {
                while let Ok(update) = link.updates.try_recv() {
                    if want(&update) {
                        return update;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("never saw the update");
        }

        async fn request_from_bob(
            accept: bool,
        ) -> (
            Home,
            Result<beam_core::transfer::SendSummary, TransferError>,
        ) {
            let (alice, bob) = (home("alice"), home("bob"));
            pair(&alice, "bob", &bob, "alice");
            let (status, _stop) = start_agent(&alice).await;
            let link = InboxLink::connect(status.port, status.token.clone());
            next(&link, |u| matches!(u, InboxUpdate::Connected { .. })).await;

            let at = status
                .invite
                .as_deref()
                .unwrap()
                .parse::<Invite>()
                .unwrap()
                .endpoint_addr();
            let sending = send(&bob, at, b"the quarterly numbers");
            let InboxUpdate::Request { id, request, .. } =
                next(&link, |u| matches!(u, InboxUpdate::Request { .. })).await
            else {
                unreachable!()
            };
            assert_eq!(
                request.peer_name, "bob",
                "this device's name for the sender"
            );
            assert_eq!(request.file_name, "report.pdf");
            assert_eq!(request.fingerprint, bob.identity.fingerprint().hex());
            link.answer(id, accept);
            let result = tokio::time::timeout(Duration::from_secs(30), sending)
                .await
                .expect("the sender finished")
                .unwrap();
            (alice, result)
        }

        /// The receiver's history, once its line is there. The agent answers
        /// the sender first and writes the line just after, so the sender
        /// can finish a moment before it exists.
        async fn history_of(home: &Home) -> Vec<beam_core::history::Entry> {
            for _ in 0..100 {
                let history = beam_core::history::read(&home.store);
                if !history.is_empty() {
                    return history;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            beam_core::history::read(&home.store)
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn accepting_in_the_view_saves_the_file() {
            let (alice, result) = request_from_bob(true).await;
            result.expect("sent");
            let saved = std::fs::read(alice.inbox.join("report.pdf")).unwrap();
            assert_eq!(saved, b"the quarterly numbers");
            let history = history_of(&alice).await;
            assert_eq!(history.len(), 1, "{history:?}");
            assert_eq!(
                history[0].direction,
                beam_core::history::Direction::Received
            );
            assert_eq!(history[0].outcome, beam_core::history::Outcome::Done);
            assert_eq!(
                (history[0].peer.as_str(), history[0].file.as_str()),
                ("bob", "report.pdf")
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn declining_in_the_view_saves_nothing() {
            let (alice, result) = request_from_bob(false).await;
            assert!(result.is_err(), "{result:?}");
            assert!(!alice.inbox.join("report.pdf").exists());
            let history = history_of(&alice).await;
            assert_eq!(history.len(), 1, "{history:?}");
            assert_eq!(history[0].outcome, beam_core::history::Outcome::Declined);
            assert_eq!(history[0].file, "report.pdf", "the declined file is named");
        }
    }
}
