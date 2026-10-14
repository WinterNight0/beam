//! Sending (GUI EX Patch #6): a file to a friend, picked with the OS file
//! picker (`rfd`) or dropped on the window, one at a time.
//!
//! The send itself is the shared worker (`beam_core::actions::send_worker`),
//! which is `beam send`: the same dial, the same Accept on the other side
//! before any byte moves, the same history line. Stopping tells the other
//! side, who keeps what arrived for a resume (ADR-0041). The stages are the
//! model's (`beam_model::sending`).
//!
//! A dropped file only proposes: a drop can be an accident, so the window
//! asks "send this to them?" before anything starts.

use std::path::PathBuf;
use std::time::Duration;

use beam_core::actions::send::{SendError, Sent};
use beam_core::actions::send_worker::{Outgoing, SendEvent};
use beam_core::identity::PeerError;
use beam_core::{text, untrusted};
use beam_model::sending::{Sending, Update};
use iced::Task;

use super::{Dialog, Gui, Message, Page, Tone};

#[derive(Debug, Clone)]
pub enum SendMsg {
    /// Open the OS file picker to send to this friend.
    Pick(String),
    /// What the picker returned (`None`: cancelled).
    Picked(String, Option<PathBuf>),
    /// A file dropped on the window.
    Dropped(PathBuf),
    /// The answer to "send this dropped file?".
    AnswerDrop(bool),
    /// Stop the send in progress.
    Cancel,
    /// Clear a finished send from the Transfers page.
    Dismiss,
    /// Read what the worker said.
    Tick,
}

/// The send in progress. Dropping it cancels the send.
pub(crate) struct Running(pub(crate) Outgoing<Update>);

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Running")
    }
}

/// How long closing the window waits for the receiver to be told.
pub(crate) const CANCEL_PATIENCE: Duration = Duration::from_secs(3);

impl Gui {
    pub(crate) fn on_send(&mut self, message: SendMsg) -> Task<Message> {
        match message {
            SendMsg::Pick(friend) => {
                if self.busy_sending() {
                    return Task::none();
                }
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Choose a file to send")
                            .pick_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    move |path| Message::Send(SendMsg::Picked(friend.clone(), path)),
                );
            }
            SendMsg::Picked(friend, Some(path)) => self.start_send(friend, path),
            SendMsg::Picked(_, None) => {}
            SendMsg::Dropped(path) => {
                // Only onto a friend's page, and only when nothing else is
                // open: the drop proposes, the person decides.
                if self.page == Page::Friends
                    && self.dialog.is_none()
                    && let Some(friend) = self.selected.clone()
                {
                    self.dialog = Some(Dialog::SendDropped { friend, path });
                }
            }
            SendMsg::AnswerDrop(yes) => {
                if let Some(Dialog::SendDropped { friend, path }) = self.dialog.take()
                    && yes
                {
                    self.start_send(friend, path);
                }
            }
            SendMsg::Cancel => {
                if let Some(running) = &mut self.outgoing {
                    running.0.cancel();
                }
            }
            SendMsg::Dismiss => {
                if self.sending.as_ref().is_some_and(|s| !s.running()) {
                    self.sending = None;
                }
            }
            SendMsg::Tick => self.drain_send(),
        }
        Task::none()
    }

    /// One send at a time, as in the view.
    fn busy_sending(&mut self) -> bool {
        if self.outgoing.is_some() {
            self.notice = Some((
                "One file at a time: wait for this send to finish, or stop it.".into(),
                Tone::Info,
            ));
            self.page = Page::Transfers;
            return true;
        }
        false
    }

    fn start_send(&mut self, friend: String, path: PathBuf) {
        if self.busy_sending() {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let file = path
            .file_name()
            .map(|n| untrusted::name(&n.to_string_lossy()))
            .unwrap_or_default();
        let name = friend.clone();
        self.outgoing = Some(Running(Outgoing::start(
            store,
            friend.clone(),
            path,
            self.bind,
            move |event| word(&name, event),
        )));
        self.sending = Some(Sending::new(friend, file));
        self.page = Page::Transfers;
    }

    fn drain_send(&mut self) {
        let Some(running) = &self.outgoing else {
            return;
        };
        let updates: Vec<Update> = running.0.updates.try_iter().collect();
        for update in updates {
            let Some(sending) = &mut self.sending else {
                break;
            };
            if let Some(result) = sending.on_update(update) {
                // Kept on the Transfers page until dismissed.
                sending.result = Some(result.clone());
                self.outgoing = None;
                self.notice = Some(match result {
                    Ok(words) => (words, Tone::Success),
                    Err(words) => (words, Tone::Problem),
                });
                self.reload();
                break;
            }
        }
    }

    /// Whether a file is going out right now.
    pub(crate) fn sending_now(&self) -> bool {
        self.outgoing.is_some()
    }

    /// Closing the window: stop the send, and give the receiver a moment to
    /// hear about it.
    pub(crate) fn stop_sending_for_close(&mut self) {
        if let Some(running) = self.outgoing.take() {
            running.0.cancel_and_wait(CANCEL_PATIENCE);
        }
    }
}

/// What happened, as the window says it.
fn word(peer_name: &str, event: SendEvent) -> Update {
    match event {
        SendEvent::Dialling => Update::Dialling,
        SendEvent::Progress(progress) => Update::Progress(progress),
        SendEvent::Finished(result) => {
            Update::Finished(result.map(sent).map_err(|e| failed(peer_name, e)))
        }
    }
}

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
        "Sent {} to {}. Saved on their side as {saved}{skipped}.",
        text::format_bytes(summary.bytes_sent),
        untrusted::name(&sent.peer)
    )
}

fn failed(peer_name: &str, error: SendError) -> String {
    let peer = untrusted::name(peer_name);
    match error {
        SendError::Peer(PeerError::NotFound(_)) => format!("You have no friend named {peer}."),
        SendError::NotAFile(path) => format!(
            "{} is not a file.",
            untrusted::name(&path.display().to_string())
        ),
        SendError::Stopped { reached: true } => format!(
            "Stopped. {peer} was told, and keeps what arrived: send the same file again to resume."
        ),
        SendError::Stopped { reached: false } => {
            format!("Stopped before {peer} was reached; nothing was sent.")
        }
        // Worded and cleaned by the action already.
        SendError::Unreachable(message) | SendError::Refused(message) => message,
        other => untrusted::lines(&other.to_string()),
    }
}
