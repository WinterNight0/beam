//! The Receiving switch (ADR-0044): the background agent's receiver, run
//! inside the view for as long as the switch is on.
//!
//! It is [`agent::run`] unchanged — pairing off, a five-minute Accept window,
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

use crate::agent::status::{self, Running};
use crate::agent::{self, AgentOptions};
use crate::config::Config;
use crate::identity::Store;
use crate::pairing::Network;
use crate::transport::endpoint::Bind;
use crate::untrusted;

/// What the receiving thread tells the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecvUpdate {
    /// Listening, saving to this folder.
    Started { receive_dir: String },
    /// Not listening any more; why, if it was not asked to stop.
    Stopped(Option<String>),
}

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
    let identity = store.load_identity().map_err(|e| fail(&e))?;
    let config = Config::load(&store.config_path()).map_err(|e| fail(&e))?;
    let (receive_dir, _) = agent::receive_dir(config.receive_dir.as_deref());
    let shown_dir = untrusted::name(&receive_dir.display().to_string());
    let options = AgentOptions {
        network: Network {
            relay: config.relay,
            bind,
            port: config.port,
            advertise: config.advertise,
        },
        receive_dir,
        accept_timeout: agent::AGENT_ACCEPT_TIMEOUT,
        port_mapping: config.agent_port_mapping,
        notify,
        echo: false,
        in_view: true,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(&e))?;
    let result = runtime.block_on(async {
        let receiving = agent::run(identity, store.clone(), options, async {
            // A drop counts as "stop" too: the view went away.
            let _ = stopped.await;
        });
        tokio::pin!(receiving);
        // Say "on" once it is really listening: its status file names this
        // process and carries its invite.
        loop {
            tokio::select! {
                result = &mut receiving => return result,
                () = tokio::time::sleep(Duration::from_millis(50)) => {
                    if let Running::Yes(status) = status::read(store)
                        && status.in_view
                        && status.pid == std::process::id()
                        && status.invite.is_some()
                    {
                        let _ = updates.send(RecvUpdate::Started {
                            receive_dir: shown_dir.clone(),
                        });
                        break;
                    }
                }
            }
        }
        receiving.await
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    result.map_err(|e| fail(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::inbox::tests::with_an_agent::home;

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

    #[test]
    fn it_will_not_start_beside_a_running_listen() {
        let alice = home("alice");
        std::fs::write(alice.store.config_path(), "relay = \"none\"\nport = 0\n").unwrap();
        // A `listen` holds its lock and has published what it offers.
        let mut board =
            crate::listen_status::Board::claim(&alice.store, Duration::from_secs(600)).unwrap();
        let code = crate::pairing::PairingCode::generate().unwrap();
        board
            .on_event(&crate::listener::ListenEvent::Ready {
                invite: crate::invite::Invite {
                    key: alice.identity.verifying_key(),
                    relay: None,
                    addrs: vec!["127.0.0.1:7820".parse().unwrap()],
                },
                fingerprint: alice.identity.fingerprint(),
                code,
                relay: crate::config::Relay::Disabled,
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
            crate::config::toml_string(&alice.inbox.display().to_string())
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
        let invite: crate::invite::Invite = status.invite.as_deref().unwrap().parse().unwrap();
        let mut known = bob.store.load_known_peers().unwrap();
        crate::invite::remember(
            known.lookup_key_mut(&invite.key).unwrap(),
            &invite,
            &crate::config::Relay::Disabled,
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
