//! The link to a running receiver (the background agent, or the view's
//! Receiving switch): what `beam inbox` and the view's Pending tab use to
//! see requests and answer them (ADR-0042, ADR-0043).
//!
//! The link is loopback TCP, opened with the token from the private
//! `agent.json`. The agent still decides what counts: one answer per
//! request, the first wins, a late one is refused (S-39). A front end
//! answers only when the person has said yes or no; nothing here answers by
//! itself.
//!
//! Text the sender chose (the file name) and text the agent passes on are
//! cleaned for the screen before they leave here (ADR-0034), except the raw
//! [`RequestInfo`], which a front end turns into its own question:
//! [`RequestView`] is its cleaned form.

use std::time::Duration;

use crate::agent::ipc::{AgentMsg, Client, ClientMsg, RequestInfo};
use crate::agent::status::{self, Running};
use crate::identity::Store;
use crate::{text, untrusted};

/// Where a running receiver listens for its inbox, and the token it wants.
#[derive(Clone, Debug)]
pub struct Address {
    pub port: u16,
    pub token: String,
}

/// Why no receiver could be found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindError {
    NotRunning,
    /// One is running, but its status file cannot be read.
    Unreadable,
}

/// The receiver running in this beam home, if any.
pub fn find(store: &Store) -> Result<Address, FindError> {
    match status::read(store) {
        Running::Yes(status) => Ok(Address {
            port: status.port,
            token: status.token,
        }),
        Running::No => Err(FindError::NotRunning),
        Running::Unreadable => Err(FindError::Unreadable),
    }
}

/// Why the link could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectError {
    Unreachable(String),
    /// It answered, but not with a welcome.
    Unexpected,
}

/// What the receiver says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InboxEvent {
    /// A paired device wants to send; answer with [`Inbox::answer`].
    Request {
        id: u64,
        request: RequestInfo,
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
    /// A transfer ended; `text` is cleaned.
    Finished {
        ok: bool,
        text: String,
    },
    /// An answer came after the request had closed, and did not count.
    TooLate {
        id: u64,
    },
    /// The receiver is stopping.
    Stopping,
    /// The link is gone.
    Gone,
}

/// An open link to the receiver.
pub struct Inbox {
    client: Client,
}

impl Inbox {
    /// Opens the link. Returns it with the folder the receiver saves to,
    /// cleaned for the screen.
    pub async fn connect(address: &Address) -> Result<(Self, String), ConnectError> {
        let (client, welcome) = Client::connect(address.port, &address.token)
            .await
            .map_err(|e| ConnectError::Unreachable(e.to_string()))?;
        let AgentMsg::Welcome { receive_dir, .. } = welcome else {
            return Err(ConnectError::Unexpected);
        };
        Ok((Self { client }, untrusted::name(&receive_dir)))
    }

    /// The next thing the receiver says.
    pub async fn next(&mut self) -> InboxEvent {
        loop {
            let Ok(msg) = self.client.next().await else {
                return InboxEvent::Gone;
            };
            return match msg {
                AgentMsg::Request {
                    id,
                    request,
                    expires_in,
                } => InboxEvent::Request {
                    id,
                    request,
                    expires_in: Duration::from_secs(expires_in),
                },
                AgentMsg::Closed { id } => InboxEvent::Closed { id },
                AgentMsg::Progress { done, total, relay } => {
                    InboxEvent::Progress { done, total, relay }
                }
                AgentMsg::Finished { ok, text } => InboxEvent::Finished {
                    ok,
                    text: untrusted::text(&text),
                },
                AgentMsg::TooLate { id } => InboxEvent::TooLate { id },
                AgentMsg::Stopping => InboxEvent::Stopping,
                AgentMsg::Welcome { .. } => continue,
            };
        }
    }

    /// Passes on the person's answer. `false` if the receiver is gone.
    pub async fn answer(&mut self, id: u64, accept: bool) -> bool {
        self.client
            .send(&ClientMsg::Answer { id, accept })
            .await
            .is_ok()
    }
}

/// A transfer request, cleaned and ready to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestView {
    /// This device's nickname for the sender (never the sender's own claim).
    pub peer_name: String,
    /// The sender's fingerprint as hex, without `SHA256:`.
    pub fingerprint: String,
    pub file_name: String,
    pub size: u64,
    /// "continues: 1.2 GiB of 2.0 GiB (60 %) already here, from 3 hours ago".
    pub resume: Option<String>,
}

impl From<RequestInfo> for RequestView {
    fn from(r: RequestInfo) -> Self {
        let resume = r.resume.as_ref().map(|x| {
            let age = x
                .age_secs
                .map(|s| format!(", from {}", text::format_age(Duration::from_secs(s))))
                .unwrap_or_default();
            format!(
                "continues: {} of {} ({} %) already here{age}",
                text::format_bytes(x.have_bytes),
                text::format_bytes(r.size),
                text::percent(x.have_bytes, r.size)
            )
        });
        let fingerprint = r.fingerprint.trim_start_matches("SHA256:").to_string();
        Self {
            peer_name: untrusted::name(&r.peer_name),
            fingerprint: untrusted::text(&fingerprint),
            file_name: untrusted::name(&r.file_name),
            size: r.size,
            resume,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ipc::ResumeWire;

    #[test]
    fn a_request_is_cleaned_and_its_resume_described() {
        let view = RequestView::from(RequestInfo {
            peer_name: "alice".into(),
            fingerprint: "SHA256:ab12".into(),
            file_name: "evil\x1b[31m.txt".into(),
            size: 2048,
            resume: Some(ResumeWire {
                have_bytes: 1024,
                have_chunks: 1,
                chunk_count: 2,
                age_secs: None,
            }),
        });
        assert_eq!(view.fingerprint, "ab12");
        assert!(!view.file_name.contains('\x1b'), "{:?}", view.file_name);
        let resume = view.resume.unwrap();
        assert!(resume.contains("50 %"), "{resume}");
    }
}
