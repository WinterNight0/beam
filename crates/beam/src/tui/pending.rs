//! The Pending tab and the Accept pop-up (ADR-0043, step 5).
//!
//! Requests come from the background agent through [`super::inbox`]. They
//! never open a pop-up by themselves — a request arriving while someone is
//! typing must not catch a stray Enter — but they show at once: a count on
//! the tab and in the header, and a line in the status bar. Enter (or a
//! click) on a request opens the Accept pop-up.
//!
//! The pop-up shows what `beam listen` shows (S-6): who, their fingerprint,
//! the file and its size, and what is already here when it resumes. It
//! starts on **Decline**, so Enter alone never accepts; Esc closes it and
//! leaves the request waiting until it expires, which counts as no.

use std::time::Instant;

use super::app::{App, Choice, Effect, Key, Modal, Tab, Target};
use super::inbox::{InboxUpdate, RequestView};

/// A request waiting for an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub id: u64,
    pub request: RequestView,
    pub expires: Instant,
}

/// A transfer the agent is receiving now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receiving {
    pub done: u64,
    pub total: u64,
    pub relay: bool,
}

/// How the view stands with the agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Link {
    /// Not running, or not tried yet.
    #[default]
    Off,
    Connecting,
    Connected,
    /// It was connected and went away, or could not be reached.
    Lost(String),
}

impl App {
    /// What the agent said.
    pub fn on_inbox(&mut self, update: InboxUpdate) {
        match update {
            InboxUpdate::Connected { .. } => self.link = Link::Connected,
            InboxUpdate::Request {
                id,
                request,
                expires_in,
            } => {
                self.flash = Some(format!(
                    "{} wants to send you {}. Open Pending (2) to answer.",
                    request.peer_name, request.file_name
                ));
                self.pending.push(Pending {
                    id,
                    request,
                    expires: Instant::now() + expires_in,
                });
            }
            InboxUpdate::Closed { id } => {
                let was_waiting = self.forget_request(id);
                if was_waiting && self.close_accept(id) {
                    self.flash = Some(
                        "That request closed before you answered (expired, answered in another \
                         inbox, or withdrawn)."
                            .to_string(),
                    );
                }
            }
            InboxUpdate::TooLate { .. } => {
                self.flash =
                    Some("That request had already closed; your answer did not count.".to_string());
            }
            InboxUpdate::Progress { done, total, relay } => {
                self.receiving = Some(Receiving { done, total, relay });
            }
            InboxUpdate::Finished { text, .. } => {
                self.receiving = None;
                self.flash = Some(text);
            }
            InboxUpdate::Gone { reason } => {
                self.pending.clear();
                self.receiving = None;
                if matches!(self.modal, Some(Modal::Accept { .. })) {
                    self.modal = None;
                }
                self.link = Link::Lost(reason);
            }
        }
        self.pending_selected = self
            .pending_selected
            .min(self.pending.len().saturating_sub(1));
    }

    /// Drops requests whose time ran out, in case the agent's own "closed"
    /// never comes (it was stopped hard).
    pub fn prune_pending(&mut self, now: Instant) {
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|p| p.expires + std::time::Duration::from_secs(2) < now)
            .map(|p| p.id)
            .collect();
        for id in expired {
            self.forget_request(id);
            self.close_accept(id);
        }
    }

    fn forget_request(&mut self, id: u64) -> bool {
        let before = self.pending.len();
        self.pending.retain(|p| p.id != id);
        self.pending_selected = self
            .pending_selected
            .min(self.pending.len().saturating_sub(1));
        self.pending.len() != before
    }

    /// Closes the Accept pop-up if it is for `id`; whether it was.
    fn close_accept(&mut self, id: u64) -> bool {
        if matches!(&self.modal, Some(Modal::Accept { id: open, .. }) if *open == id) {
            self.modal = None;
            return true;
        }
        false
    }

    /// Requests from the friend with this fingerprint, oldest first.
    pub fn pending_from(&self, fingerprint: &str) -> Vec<&Pending> {
        self.pending
            .iter()
            .filter(|p| p.request.fingerprint == fingerprint)
            .collect()
    }

    /// Opens the Accept pop-up for request `index` in `pending`.
    pub(super) fn open_accept(&mut self, index: usize) {
        if let Some(p) = self.pending.get(index) {
            self.modal = Some(Modal::Accept {
                id: p.id,
                request: p.request.clone(),
                expires: p.expires,
                focus: Choice::No,
            });
        }
    }

    /// Enter on the page: answer a request, if there is one to answer here.
    pub(super) fn open_accept_here(&mut self) {
        match self.tab {
            Tab::Pending => self.open_accept(self.pending_selected),
            Tab::Friends => {
                let Some(friend) = self.selected_friend() else {
                    return;
                };
                let fingerprint = friend.fingerprint.clone();
                if let Some(index) = self
                    .pending
                    .iter()
                    .position(|p| p.request.fingerprint == fingerprint)
                {
                    self.open_accept(index);
                }
            }
            Tab::AddFriend => {}
        }
    }

    /// Keys in the Accept pop-up. It was taken out of `self`.
    pub(super) fn on_accept_key(&mut self, modal: Modal, key: Key) -> Option<Effect> {
        let Modal::Accept {
            id,
            request,
            expires,
            focus,
        } = modal
        else {
            self.modal = Some(modal);
            return None;
        };
        let focus = match key {
            // Later: the request keeps waiting, and expiry is a no.
            Key::Esc => return None,
            Key::Enter => return self.answer_transfer(id, focus == Choice::Yes),
            Key::Char('d' | 'n') => return self.answer_transfer(id, false),
            Key::Left | Key::Right | Key::Tab | Key::BackTab => match focus {
                Choice::Yes => Choice::No,
                Choice::No => Choice::Yes,
            },
            _ => focus,
        };
        self.modal = Some(Modal::Accept {
            id,
            request,
            expires,
            focus,
        });
        None
    }

    pub(super) fn on_accept_click(&mut self, modal: Modal, hit: Option<Target>) -> Option<Effect> {
        match (&modal, hit) {
            (Modal::Accept { id, .. }, Some(Target::Button(answer))) => {
                self.answer_transfer(*id, answer == Choice::Yes)
            }
            _ => {
                self.modal = Some(modal);
                None
            }
        }
    }

    fn answer_transfer(&mut self, id: u64, accept: bool) -> Option<Effect> {
        // Gone from the list at once, so it cannot be answered twice here.
        // The agent has the last word: a late answer comes back TooLate.
        self.forget_request(id);
        self.modal = None;
        Some(Effect::AnswerTransfer { id, accept })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::tui::app::{Agent, Snapshot};

    fn app() -> App {
        App::new(Snapshot {
            me: None,
            friends: Vec::new(),
            agent: Agent::Stopped,
            history: Vec::new(),
            problem: None,
        })
    }

    fn request(id: u64, who: &str) -> InboxUpdate {
        InboxUpdate::Request {
            id,
            request: RequestView {
                peer_name: who.into(),
                fingerprint: format!("{who}-fp"),
                file_name: "report.pdf".into(),
                size: 2048,
                resume: None,
            },
            expires_in: Duration::from_secs(300),
        }
    }

    #[test]
    fn a_request_is_listed_and_announced_but_opens_nothing_by_itself() {
        let mut app = app();
        app.on_inbox(request(7, "alice"));
        assert_eq!(app.pending.len(), 1);
        assert!(app.modal.is_none(), "no pop-up steals keys");
        assert!(
            app.flash
                .as_deref()
                .unwrap()
                .contains("alice wants to send")
        );
    }

    #[test]
    fn the_accept_pop_up_starts_on_decline() {
        let mut app = app();
        app.on_inbox(request(7, "alice"));
        app.tab = Tab::Pending;
        app.on_key(Key::Enter);
        assert!(matches!(
            app.modal,
            Some(Modal::Accept {
                focus: Choice::No,
                ..
            })
        ));
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::AnswerTransfer {
                id: 7,
                accept: false
            }),
            "Enter alone declines"
        );
        assert!(app.pending.is_empty());
    }

    #[test]
    fn accepting_takes_a_deliberate_move_to_accept() {
        let mut app = app();
        app.on_inbox(request(7, "alice"));
        app.tab = Tab::Pending;
        app.on_key(Key::Enter);
        app.on_key(Key::Left);
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::AnswerTransfer {
                id: 7,
                accept: true
            })
        );
    }

    #[test]
    fn esc_leaves_the_request_waiting() {
        let mut app = app();
        app.on_inbox(request(7, "alice"));
        app.tab = Tab::Pending;
        app.on_key(Key::Enter);
        assert_eq!(app.on_key(Key::Esc), None);
        assert!(app.modal.is_none());
        assert_eq!(app.pending.len(), 1);
    }

    #[test]
    fn a_request_closed_elsewhere_closes_its_pop_up_and_says_so() {
        let mut app = app();
        app.on_inbox(request(7, "alice"));
        app.on_inbox(request(8, "bob"));
        app.tab = Tab::Pending;
        app.on_key(Key::Enter);
        app.on_inbox(InboxUpdate::Closed { id: 7 });
        assert!(app.modal.is_none());
        assert!(
            app.flash
                .as_deref()
                .unwrap()
                .contains("closed before you answered")
        );
        assert_eq!(app.pending.len(), 1);
        assert_eq!(app.pending[0].id, 8);
    }

    #[test]
    fn progress_then_finish_and_a_lost_agent_clears_everything() {
        let mut app = app();
        app.on_inbox(InboxUpdate::Progress {
            done: 1,
            total: 2,
            relay: true,
        });
        assert!(app.receiving.is_some());
        app.on_inbox(InboxUpdate::Finished {
            ok: true,
            text: "Saved report.pdf".into(),
        });
        assert!(app.receiving.is_none());
        assert_eq!(app.flash.as_deref(), Some("Saved report.pdf"));

        app.on_inbox(request(9, "alice"));
        app.on_inbox(InboxUpdate::Gone {
            reason: "stopped".into(),
        });
        assert!(app.pending.is_empty());
        assert_eq!(app.link, Link::Lost("stopped".into()));
    }

    #[test]
    fn expired_requests_are_dropped_even_without_word_from_the_agent() {
        let mut app = app();
        app.on_inbox(request(7, "alice"));
        app.prune_pending(Instant::now() + Duration::from_secs(400));
        assert!(app.pending.is_empty());
    }
}
