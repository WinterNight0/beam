//! The Receiving switch (ADR-0044): the background agent's receiver, run
//! inside the view for as long as the switch is on.
//!
//! It is [`actions::receive`](beam_core::actions::receive), which is
//! [`agent::run`](beam_core::agent::run) unchanged — pairing off, a five-minute Accept window,
//! router port mapping only if `beam service port-mapping on`, a desktop
//! notification per request — with `in_view` set, so `beam service status`
//! and `beam whoami` say who is receiving. It takes the agent's lock, so
//! `beam listen` and a second agent refuse while it runs, and the view's
//! Pending tab reaches it through the same token-protected link it uses for
//! the background agent. Turning the switch off, or leaving beam, stops it
//! the way `listen` stops on Ctrl+C: a sender mid-transfer is told, and what
//! arrived is kept for a resume (ADR-0041).

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use beam_core::actions::receive::{self, Place};
use beam_core::identity::Store;
use beam_core::transport::endpoint::Bind;
use beam_core::untrusted;

/// What the receiving thread tells the view: the model's own message.
pub use beam_model::requests::SwitchUpdate as RecvUpdate;

/// The receiver while the switch is on.
pub struct InView {
    pub updates: Receiver<RecvUpdate>,
    /// Sent (or dropped) to stop.
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl InView {
    pub fn start(store: Store) -> Self {
        Self::start_on(store, Bind::Any, true)
    }

    /// `Loopback` and no notifications only in tests.
    pub(super) fn start_on(store: Store, bind: Bind, notify: bool) -> Self {
        let (update_tx, updates) = mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        std::thread::spawn(move || {
            let why = run(&store, bind, notify, &update_tx, stopped).err();
            let _ = update_tx.send(RecvUpdate::Stopped(why));
        });
        Self {
            updates,
            stop: Some(stop),
        }
    }

    /// Asks it to stop; [`RecvUpdate::Stopped`] follows.
    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }

    /// Stops it and waits, so leaving beam tells a sender before the
    /// process ends.
    pub fn stop_and_wait(mut self, patience: Duration) {
        self.stop();
        let deadline = std::time::Instant::now() + patience;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match self.updates.recv_timeout(left) {
                Ok(RecvUpdate::Stopped(_)) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }
}

fn run(
    store: &Store,
    bind: Bind,
    notify: bool,
    updates: &Sender<RecvUpdate>,
    stopped: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| untrusted::lines(&e.to_string());
    let receiver =
        receive::prepare(store, Place::InView, bind, notify, None).map_err(|e| fail(&e))?;
    let shown_dir = untrusted::name(&receiver.receive_dir().display().to_string());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(&e))?;
    let result = runtime.block_on(receive::run(
        store,
        receiver,
        // A drop counts as "stop" too: the view went away.
        async {
            let _ = stopped.await;
        },
        // Say "on" once it is really receiving.
        || {
            let _ = updates.send(RecvUpdate::Started {
                receive_dir: shown_dir,
            });
        },
    ));
    runtime.shutdown_timeout(Duration::from_secs(1));
    result.map_err(|e| fail(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::inbox::tests::with_an_agent::home;
    use beam_core::agent::status::{self, Running};

    fn next(on: &InView) -> RecvUpdate {
        on.updates
            .recv_timeout(Duration::from_secs(30))
            .expect("an update")
    }

    #[test]
    fn the_switch_starts_a_receiver_that_shuts_out_listen_and_stops_cleanly() {
        let alice = home("alice");
        std::fs::write(alice.store.config_path(), "relay = \"none\"\nport = 0\n").unwrap();
        let mut on = InView::start_on(alice.store.clone(), Bind::Loopback, false);
        assert!(matches!(next(&on), RecvUpdate::Started { .. }));

        // Everyone else sees an in-view receiver holding the agent's place.
        match status::read(&alice.store) {
            Running::Yes(status) => assert!(status.in_view),
            other => panic!("{other:?}"),
        }
        let second = InView::start_on(alice.store.clone(), Bind::Loopback, false);
        assert!(
            matches!(next(&second), RecvUpdate::Stopped(Some(why)) if why.contains("already running")),
            "a second receiver in one home is refused"
        );

        on.stop();
        assert_eq!(next(&on), RecvUpdate::Stopped(None));
        assert!(matches!(status::read(&alice.store), Running::No));
    }

    /// The switch, as the view starts it, does not pair: a stranger who
    /// finds it cannot even open the pairing protocol, so cannot try codes
    /// (safety net for moving the switch into one shared place; R-8).
    #[test]
    fn the_switch_never_offers_pairing() {
        let alice = home("alice");
        std::fs::write(alice.store.config_path(), "relay = \"none\"\nport = 0\n").unwrap();
        let mut on = InView::start_on(alice.store.clone(), Bind::Loopback, false);
        assert!(matches!(next(&on), RecvUpdate::Started { .. }));
        let mut invite = None;
        for _ in 0..600 {
            if let Running::Yes(status) = status::read(&alice.store)
                && let Some(text) = status.invite
            {
                invite = Some(text);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let at = invite
            .expect("the switch published its invite")
            .parse::<beam_core::invite::Invite>()
            .unwrap()
            .endpoint_addr();

        let stranger = home("stranger");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let refused = runtime.block_on(async {
            let endpoint = beam_core::transport::endpoint::bind(
                &stranger.identity,
                &beam_core::config::Relay::Disabled,
                Bind::Loopback,
                0,
                &[],
            )
            .await
            .unwrap();
            let attempt = tokio::time::timeout(
                Duration::from_secs(30),
                endpoint.connect(at, beam_core::transport::endpoint::PAIR_ALPN),
            )
            .await
            .expect("the attempt hung");
            endpoint.close().await;
            attempt.is_err()
        });
        assert!(
            refused,
            "the Receiving switch accepted a pairing connection"
        );

        on.stop();
        assert_eq!(next(&on), RecvUpdate::Stopped(None));
    }

    #[test]
    fn it_will_not_start_beside_a_running_listen() {
        let alice = home("alice");
        std::fs::write(alice.store.config_path(), "relay = \"none\"\nport = 0\n").unwrap();
        // A `listen` holds its lock and has published what it offers.
        let mut board =
            beam_core::listen_status::Board::claim(&alice.store, Duration::from_secs(600)).unwrap();
        let code = beam_core::pairing::PairingCode::generate().unwrap();
        board
            .on_event(&beam_core::listener::ListenEvent::Ready {
                invite: beam_core::invite::Invite {
                    key: alice.identity.verifying_key(),
                    relay: None,
                    addrs: vec!["127.0.0.1:7820".parse().unwrap()],
                },
                fingerprint: alice.identity.fingerprint(),
                code,
                relay: beam_core::config::Relay::Disabled,
            })
            .unwrap();
        let on = InView::start_on(alice.store.clone(), Bind::Loopback, false);
        assert!(matches!(next(&on), RecvUpdate::Stopped(Some(why)) if why.contains("beam listen")),);
    }

    #[test]
    fn with_the_switch_on_a_friend_sends_and_the_view_accepts() {
        use crate::tui::inbox::tests::with_an_agent::pair;
        use crate::tui::inbox::{InboxLink, InboxUpdate};
        use crate::tui::sending::{Outgoing, SendUpdate};

        let (alice, bob) = (home("alice"), home("bob"));
        pair(&alice, "bob", &bob, "alice");
        for store in [&alice.store, &bob.store] {
            std::fs::write(
                store.config_path(),
                "relay = \"none\"
port = 0
",
            )
            .unwrap();
        }
        let config = format!(
            "relay = \"none\"
port = 0
receive_dir = {}
",
            beam_core::config::toml_string(&alice.inbox.display().to_string())
        );
        std::fs::write(alice.store.config_path(), config).unwrap();

        // alice turns Receiving on; her Pending tab connects as it would.
        let mut on = InView::start_on(alice.store.clone(), Bind::Loopback, false);
        assert!(matches!(next(&on), RecvUpdate::Started { .. }));
        let Running::Yes(status) = status::read(&alice.store) else {
            panic!("not receiving")
        };
        let link = InboxLink::connect(status.port, status.token.clone());

        // bob knows where she is, as after pairing with her invite.
        let invite: beam_core::invite::Invite = status.invite.as_deref().unwrap().parse().unwrap();
        let mut known = bob.store.load_known_peers().unwrap();
        beam_core::invite::remember(
            known.lookup_key_mut(&invite.key).unwrap(),
            &invite,
            &beam_core::config::Relay::Disabled,
        );
        bob.store.save_known_peers(&known).unwrap();
        let path = bob.files.join("report.pdf");
        std::fs::write(&path, b"the quarterly numbers").unwrap();
        let out = Outgoing::start_on(bob.store.clone(), "alice".into(), path, Bind::Loopback);

        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let result = loop {
            assert!(std::time::Instant::now() < deadline, "the send never ended");
            while let Ok(update) = link.updates.try_recv() {
                if let InboxUpdate::Request { id, .. } = update {
                    link.answer(id, true);
                }
            }
            if let Ok(SendUpdate::Finished(result)) = out.updates.try_recv() {
                break result;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        result.expect("sent");
        assert_eq!(
            std::fs::read(alice.inbox.join("report.pdf")).unwrap(),
            b"the quarterly numbers"
        );
        on.stop();
        assert_eq!(next(&on), RecvUpdate::Stopped(None));
    }
}
