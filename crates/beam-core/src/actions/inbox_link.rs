//! A window's link to the receiver (the background agent, or its own
//! Receiving switch): what `beam inbox` does, on a thread, for the
//! full-screen view's Pending tab and the GUI's Inbox (ADR-0042, ADR-0043).
//!
//! A thread connects to the receiver over loopback with the token from the
//! private `agent.json`, exactly as `beam inbox` does, and passes on what it
//! says. A window answers a request only from its Accept question, which
//! starts on Decline. The receiver still decides what counts: one answer per
//! request, the first wins, and a late one is refused (S-39).
//!
//! Everything a request carries is cleaned for the screen, once, on the way
//! through ([`RequestView`], ADR-0034): the file name comes from the sender.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use super::inbox::{Address, ConnectError, Inbox, InboxEvent, RequestView};

/// What the link reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkUpdate {
    Connected {
        receive_dir: String,
    },
    Request {
        id: u64,
        request: RequestView,
        expires_in: Duration,
    },
    /// No longer waiting: answered (here or elsewhere), expired, withdrawn.
    Closed {
        id: u64,
    },
    Progress {
        done: u64,
        total: u64,
        relay: bool,
    },
    Finished {
        ok: bool,
        text: String,
    },
    TooLate {
        id: u64,
    },
    /// The link is gone: the receiver stopped, or could not be reached.
    Gone {
        reason: String,
    },
}

/// A connection to the receiver, on its own thread.
pub struct Link {
    pub updates: Receiver<LinkUpdate>,
    answers: tokio::sync::mpsc::UnboundedSender<(u64, bool)>,
    /// Dropped with the link, which closes it.
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl Link {
    pub fn connect(port: u16, token: String) -> Self {
        let (update_tx, updates) = mpsc::channel();
        let (answers, answer_rx) = tokio::sync::mpsc::unbounded_channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let reason = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    runtime.block_on(serve(port, &token, &update_tx, answer_rx, stopped))
                }
                Err(e) => e.to_string(),
            };
            let _ = update_tx.send(LinkUpdate::Gone { reason });
        });
        Self {
            updates,
            answers,
            _stop: stop,
        }
    }

    /// Passes on an answer, given by the person in the Accept question.
    pub fn answer(&self, id: u64, accept: bool) {
        let _ = self.answers.send((id, accept));
    }
}

/// Runs the link until it ends; returns why.
async fn serve(
    port: u16,
    token: &str,
    updates: &mpsc::Sender<LinkUpdate>,
    mut answers: tokio::sync::mpsc::UnboundedReceiver<(u64, bool)>,
    mut stopped: tokio::sync::oneshot::Receiver<()>,
) -> String {
    let address = Address {
        port,
        token: token.to_string(),
    };
    let (mut inbox, receive_dir) = match Inbox::connect(&address).await {
        Ok(connected) => connected,
        Err(ConnectError::Unreachable(e)) => {
            return format!("could not reach the background agent: {e}");
        }
        Err(ConnectError::Unexpected) => {
            return "the background agent did not answer as expected".to_string();
        }
    };
    let _ = updates.send(LinkUpdate::Connected { receive_dir });
    loop {
        tokio::select! {
            event = inbox.next() => {
                let update = match event {
                    InboxEvent::Request { id, request, expires_in } => LinkUpdate::Request {
                        id,
                        request: request.into(),
                        expires_in,
                    },
                    InboxEvent::Closed { id } => LinkUpdate::Closed { id },
                    InboxEvent::Progress { done, total, relay } => {
                        LinkUpdate::Progress { done, total, relay }
                    }
                    InboxEvent::Finished { ok, text } => LinkUpdate::Finished { ok, text },
                    InboxEvent::TooLate { id } => LinkUpdate::TooLate { id },
                    InboxEvent::Stopping => return "the background agent is stopping".to_string(),
                    InboxEvent::Gone => return "the background agent stopped".to_string(),
                };
                if updates.send(update).is_err() {
                    return String::new();
                }
            }
            Some((id, accept)) = answers.recv() => {
                if !inbox.answer(id, accept).await {
                    return "the background agent stopped".to_string();
                }
            }
            _ = &mut stopped => return String::new(),
        }
    }
}
