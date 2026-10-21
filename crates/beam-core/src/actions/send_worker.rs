//! Sending a file from a window: what `beam send` does
//! ([`actions::send::send`](super::send::send)), on a background thread,
//! with its progress drawn instead of printed. Shared by the full-screen
//! view and the GUI.
//!
//! Nothing about the transfer changes: the same dial, the same engine, the
//! same Accept on the other side before any byte moves, the same history
//! line. Cancelling closes the connection the ADR-0041 way, so the receiver
//! is told who stopped it; whatever it already has is kept for a resume.
//!
//! Like every action it reports what happened ([`SendEvent`]); each front
//! end turns that into its own message, words and all, with the `into`
//! function it passes to [`Outgoing::start`].

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use super::send::{SendError, SendRequest, Sent, Step};
use crate::identity::Store;
use crate::transfer::{Progress, Reporter};
use crate::transport::endpoint::Bind;
use crate::untrusted;

/// What a send does, as it runs.
#[derive(Debug)]
pub enum SendEvent {
    /// Looking for the friend.
    Dialling,
    Progress(Progress),
    /// How it ended.
    Finished(Result<Sent, SendError>),
}

/// A send running in the background, reporting as the front end's `U`.
pub struct Outgoing<U> {
    pub updates: Receiver<U>,
    /// Dropped to cancel.
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    /// Set when the thread has finished.
    done: Arc<AtomicBool>,
}

impl<U: Send + 'static> Outgoing<U> {
    /// Sends `path` to the friend called `peer`. `into` turns each event
    /// into the front end's message. `bind` is `Loopback` only in tests.
    pub fn start(
        store: Store,
        peer: String,
        path: PathBuf,
        bind: Bind,
        into: impl Fn(SendEvent) -> U + Send + 'static,
    ) -> Self {
        let (update_tx, updates) = mpsc::channel();
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let done = Arc::new(AtomicBool::new(false));
        let finished = Arc::clone(&done);
        std::thread::spawn(move || {
            let result = run(&store, &peer, path, bind, &update_tx, &into, cancelled);
            let _ = update_tx.send(into(SendEvent::Finished(result)));
            finished.store(true, Ordering::SeqCst);
        });
        Self {
            updates,
            cancel: Some(cancel),
            done,
        }
    }
}

impl<U> Outgoing<U> {
    /// Stops the send; the receiver is told, and keeps what it has.
    pub fn cancel(&mut self) {
        self.cancel.take();
    }

    /// Cancels, and waits a little for the receiver to be told, so closing
    /// the window mid-send does not just vanish from under the other side.
    pub fn cancel_and_wait(mut self, patience: Duration) {
        self.cancel();
        let deadline = std::time::Instant::now() + patience;
        while !self.done.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Passes the engine's progress on.
struct Passing<'a, U, F: Fn(SendEvent) -> U>(&'a Sender<U>, &'a F);

impl<U, F: Fn(SendEvent) -> U> Reporter for Passing<'_, U, F> {
    fn report(&mut self, progress: Progress) {
        let _ = self.0.send((self.1)(SendEvent::Progress(progress)));
    }
}

fn run<U>(
    store: &Store,
    peer: &str,
    path: PathBuf,
    bind: Bind,
    updates: &Sender<U>,
    into: &impl Fn(SendEvent) -> U,
    cancelled: tokio::sync::oneshot::Receiver<()>,
) -> Result<Sent, SendError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| SendError::Endpoint(untrusted::lines(&e.to_string())))?;
    let request = SendRequest {
        peer,
        file: &path,
        chunk_size: None,
        bind,
    };
    let result = runtime.block_on(super::send::send(
        store,
        request,
        &mut Passing(updates, into),
        |step| {
            if let Step::Looking { .. } = step {
                let _ = updates.send(into(SendEvent::Dialling));
            }
        },
        // Dropping the sender counts too: the window went away.
        async {
            let _ = cancelled.await;
        },
    ));
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}
