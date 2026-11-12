//! The Receiving switch (ADR-0044): the background agent's receiver, run
//! inside a window for as long as the switch is on. Shared by the
//! full-screen view and the GUI.
//!
//! It is [`receive::run`](super::receive::run), which is
//! [`agent::run`](crate::agent::run) unchanged: pairing off, a five-minute
//! Accept window, router port mapping only if `beam service port-mapping
//! on`, a desktop notification per request, with `in_view` set, so `beam
//! service status` and `beam whoami` say who is receiving. It takes the
//! agent's lock, so `beam listen` and a second agent refuse while it runs,
//! and the window's Inbox reaches it through the same token-protected link
//! ([`inbox_link`](super::inbox_link)) it uses for the background agent.
//! Turning the switch off, or closing the window, stops it the way `listen`
//! stops on Ctrl+C: a sender mid-transfer is told, and what arrived is kept
//! for a resume (ADR-0041).

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use super::receive::{self, Place};
use crate::identity::Store;
use crate::transport::endpoint::Bind;
use crate::untrusted;

/// What the switch's receiver reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchUpdate {
    /// Receiving, saving to this folder (cleaned).
    Started { receive_dir: String },
    /// Not receiving any more; why (cleaned), if it was not asked to stop.
    Stopped(Option<String>),
}

/// The receiver while the switch is on.
pub struct InView {
    pub updates: Receiver<SwitchUpdate>,
    /// Sent (or dropped) to stop.
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl InView {
    pub fn start(store: Store) -> Self {
        Self::start_on(store, Bind::Any, true)
    }

    /// `Loopback` and no notifications only in tests.
    pub fn start_on(store: Store, bind: Bind, notify: bool) -> Self {
        let (update_tx, updates) = mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        std::thread::spawn(move || {
            let why = run(&store, bind, notify, &update_tx, stopped).err();
            let _ = update_tx.send(SwitchUpdate::Stopped(why));
        });
        Self {
            updates,
            stop: Some(stop),
        }
    }

    /// Asks it to stop; [`SwitchUpdate::Stopped`] follows.
    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }

    /// Stops it and waits, so closing the window tells a sender before the
    /// process ends.
    pub fn stop_and_wait(mut self, patience: Duration) {
        self.stop();
        let deadline = std::time::Instant::now() + patience;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match self.updates.recv_timeout(left) {
                Ok(SwitchUpdate::Stopped(_)) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }
}

fn run(
    store: &Store,
    bind: Bind,
    notify: bool,
    updates: &Sender<SwitchUpdate>,
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
        // A drop counts as "stop" too: the window went away.
        async {
            let _ = stopped.await;
        },
        // Say "on" once it is really receiving.
        || {
            let _ = updates.send(SwitchUpdate::Started {
                receive_dir: shown_dir,
            });
        },
    ));
    runtime.shutdown_timeout(Duration::from_secs(1));
    result.map_err(|e| fail(&e))
}
