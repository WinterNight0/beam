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

use super::app::{App, Ask, Choice, Effect, Incoming, Key, Modal, StopMidTransfer, Tab, Target};
use super::inbox::InboxUpdate;
use super::receiving::RecvUpdate;
use beam_model::requests::{Happened, Toggle};

pub use beam_model::requests::{Link, Pending, Switch, Taken};

impl App {
    /// Who else receives for this beam home, which makes the switch moot.
    pub fn receiving_elsewhere(&self) -> Option<Taken> {
        self.requests.receiving_elsewhere(&self.snapshot)
    }

    /// `o`, or a click on the switch.
    pub(super) fn toggle_receiving(&mut self) -> Option<Effect> {
        match self.requests.toggle(&self.snapshot) {
            Toggle::Start => Some(Effect::StartReceiving),
            Toggle::Stop => Some(Effect::StopReceiving),
            Toggle::AskFirst => {
                self.modal = Some(Modal::ConfirmStop(Ask::new(StopMidTransfer)));
                None
            }
            Toggle::Busy => None,
            Toggle::Taken(taken) => {
                self.flash = Some(
                    match taken {
                        Taken::Agent => {
                            "The background agent already receives for you. \
                             `:service stop` first to receive in this view instead."
                        }
                        Taken::OtherView => "Another beam window is already receiving.",
                        Taken::Listen => {
                            "`beam listen` is receiving in another terminal. \
                             Stop it (Ctrl+C) to receive here."
                        }
                    }
                    .to_string(),
                );
                None
            }
        }
    }

    /// The receiver thread moved on.
    pub fn on_switch(&mut self, update: RecvUpdate) {
        self.requests.on_switch(&update);
        match update {
            RecvUpdate::Started { receive_dir } => {
                self.flash = Some(format!(
                    "Receiving. Friends can send you files while beam is open; they go to {receive_dir}."
                ));
            }
            RecvUpdate::Stopped(None) => self.flash = Some("Receiving is off.".to_string()),
            RecvUpdate::Stopped(Some(_)) => {}
        }
    }

    pub(super) fn on_confirm_stop_key(
        &mut self,
        mut ask: Ask<StopMidTransfer>,
        key: Key,
    ) -> Option<Effect> {
        match key {
            Key::Esc => return None,
            Key::Enter if ask.focus() == Choice::Yes => {
                self.requests.stop();
                return Some(Effect::StopReceiving);
            }
            Key::Enter => return None,
            Key::Left | Key::Right | Key::Tab | Key::BackTab => ask.flip(),
            _ => {}
        }
        self.modal = Some(Modal::ConfirmStop(ask));
        None
    }

    /// What the agent said.
    pub fn on_inbox(&mut self, update: InboxUpdate) {
        match self.requests.on_update(update, Instant::now()) {
            Happened::Asked {
                peer_name,
                file_name,
            } => {
                self.flash = Some(format!(
                    "{peer_name} wants to send you {file_name}. Open Pending (2) to answer."
                ));
            }
            Happened::Closed { id, was_waiting } => {
                if was_waiting && self.close_accept(id) {
                    self.flash = Some(
                        "That request closed before you answered (expired, answered in another \
                         inbox, or withdrawn)."
                            .to_string(),
                    );
                }
            }
            Happened::TooLate => {
                self.flash =
                    Some("That request had already closed; your answer did not count.".to_string());
            }
            Happened::Finished { text } => self.flash = Some(text),
            Happened::Gone => {
                if matches!(self.modal, Some(Modal::Accept { .. })) {
                    self.modal = None;
                }
            }
            Happened::Connected | Happened::Progress => {}
        }
    }

    /// Drops requests whose time ran out, in case the agent's own "closed"
    /// never comes (it was stopped hard).
    pub fn prune_pending(&mut self, now: Instant) {
        for id in self.requests.prune(now) {
            self.close_accept(id);
        }
    }

    /// Closes the Accept pop-up if it is for `id`; whether it was.
    fn close_accept(&mut self, id: u64) -> bool {
        if matches!(&self.modal, Some(Modal::Accept(ask)) if ask.question().id == id) {
            self.modal = None;
            return true;
        }
        false
    }

    /// Requests from the friend with this fingerprint, oldest first.
    pub fn pending_from(&self, fingerprint: &str) -> Vec<&Pending> {
        self.requests.from(fingerprint)
    }

    /// Opens the Accept pop-up for request `index` in `pending`.
    pub(super) fn open_accept(&mut self, index: usize) {
        if let Some(p) = self.requests.waiting.get(index) {
            self.modal = Some(Modal::Accept(Ask::new(Incoming {
                id: p.id,
                request: p.request.clone(),
                expires: p.expires,
            })));
        }
    }

    /// Enter on the page: answer a request, if there is one to answer here.
    pub(super) fn open_accept_here(&mut self) {
        match self.tab {
            Tab::Pending => self.open_accept(self.requests.selected),
            Tab::Friends => {
                let Some(friend) = self.selected_friend() else {
                    return;
                };
                let fingerprint = friend.fingerprint.clone();
                if let Some(index) = self
                    .requests
                    .waiting
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
        let Modal::Accept(mut ask) = modal else {
            self.modal = Some(modal);
            return None;
        };
        let id = ask.question().id;
        match key {
            // Later: the request keeps waiting, and expiry is a no.
            Key::Esc => return None,
            Key::Enter => return self.answer_transfer(id, ask.focus() == Choice::Yes),
            Key::Char('d' | 'n') => return self.answer_transfer(id, false),
            Key::Left | Key::Right | Key::Tab | Key::BackTab => ask.flip(),
            _ => {}
        }
        self.modal = Some(Modal::Accept(ask));
        None
    }

    pub(super) fn on_accept_click(&mut self, modal: Modal, hit: Option<Target>) -> Option<Effect> {
        match (&modal, hit) {
            (Modal::Accept(ask), Some(Target::Button(answer))) => {
                self.answer_transfer(ask.question().id, answer == Choice::Yes)
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
        self.requests.answered(id);
        self.modal = None;
        Some(Effect::AnswerTransfer { id, accept })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::tui::app::{Agent, Owner, Snapshot};
    use crate::tui::inbox::RequestView;
    use beam_model::requests::Receiving;

    fn app() -> App {
        App::new(Snapshot {
            me: None,
            friends: Vec::new(),
            agent: Agent::Stopped,
            history: Vec::new(),
            owner: Owner::Background,
            listen_elsewhere: false,
            needs_setup: false,
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
        assert_eq!(app.requests.waiting.len(), 1);
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
            &app.modal,
            Some(Modal::Accept(ask)) if ask.focus() == Choice::No
        ));
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::AnswerTransfer {
                id: 7,
                accept: false
            }),
            "Enter alone declines"
        );
        assert!(app.requests.waiting.is_empty());
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
        assert_eq!(app.requests.waiting.len(), 1);
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
        assert_eq!(app.requests.waiting.len(), 1);
        assert_eq!(app.requests.waiting[0].id, 8);
    }

    #[test]
    fn progress_then_finish_and_a_lost_agent_clears_everything() {
        let mut app = app();
        app.on_inbox(InboxUpdate::Progress {
            done: 1,
            total: 2,
            relay: true,
        });
        assert!(app.requests.receiving.is_some());
        app.on_inbox(InboxUpdate::Finished {
            ok: true,
            text: "Saved report.pdf".into(),
        });
        assert!(app.requests.receiving.is_none());
        assert_eq!(app.flash.as_deref(), Some("Saved report.pdf"));

        app.on_inbox(request(9, "alice"));
        app.on_inbox(InboxUpdate::Gone {
            reason: "stopped".into(),
        });
        assert!(app.requests.waiting.is_empty());
        assert_eq!(app.requests.link, Link::Lost("stopped".into()));
    }

    #[test]
    fn the_switch_turns_on_and_off_and_says_so() {
        let mut app = app();
        assert_eq!(app.on_key(Key::Char('o')), Some(Effect::StartReceiving));
        assert_eq!(app.requests.switch, Switch::Starting);
        assert_eq!(app.on_key(Key::Char('o')), None, "nothing while starting");
        app.on_switch(RecvUpdate::Started {
            receive_dir: "D:/Downloads".into(),
        });
        assert_eq!(app.requests.switch, Switch::On);
        assert!(app.flash.as_deref().unwrap().contains("D:/Downloads"));
        assert_eq!(app.on_key(Key::Char('o')), Some(Effect::StopReceiving));
        app.on_switch(RecvUpdate::Stopped(None));
        assert_eq!(app.requests.switch, Switch::Off);
    }

    #[test]
    fn turning_off_mid_transfer_asks_first_and_no_is_the_default() {
        let mut app = app();
        app.requests.switch = Switch::On;
        app.requests.receiving = Some(Receiving {
            done: 1,
            total: 2,
            relay: false,
        });
        assert_eq!(app.on_key(Key::Char('o')), None);
        assert_eq!(
            app.modal,
            Some(Modal::ConfirmStop(Ask::new(StopMidTransfer)))
        );
        assert_eq!(app.on_key(Key::Enter), None, "Enter alone keeps receiving");
        assert_eq!(app.requests.switch, Switch::On);
        app.on_key(Key::Char('o'));
        app.on_key(Key::Left);
        assert_eq!(app.on_key(Key::Enter), Some(Effect::StopReceiving));
    }

    #[test]
    fn the_switch_will_not_fight_the_agent_or_listen() {
        let mut app = app();
        app.snapshot.agent = Agent::Running {
            receive_dir: "x".into(),
        };
        assert_eq!(app.on_key(Key::Char('o')), None);
        assert!(app.flash.as_deref().unwrap().contains("background agent"));

        let mut app = app_fn();
        app.snapshot.listen_elsewhere = true;
        assert_eq!(app.on_key(Key::Char('o')), None);
        assert!(app.flash.as_deref().unwrap().contains("beam listen"));
    }

    fn app_fn() -> App {
        app()
    }

    #[test]
    fn leaving_while_a_file_arrives_asks_first() {
        let mut app = app();
        app.requests.switch = Switch::On;
        app.requests.receiving = Some(Receiving {
            done: 1,
            total: 2,
            relay: false,
        });
        app.on_key(Key::Quit);
        assert!(!app.quit);
        assert_eq!(
            app.modal,
            Some(Modal::ConfirmQuit(Ask::new(
                crate::tui::app::LeaveMidTransfer
            )))
        );
    }

    #[test]
    fn a_receiver_that_could_not_start_says_why() {
        let mut app = app();
        app.on_key(Key::Char('o'));
        app.on_switch(RecvUpdate::Stopped(Some("port in use".into())));
        assert_eq!(app.requests.switch, Switch::Failed("port in use".into()));
        assert_eq!(
            app.on_key(Key::Char('o')),
            Some(Effect::StartReceiving),
            "try again"
        );
    }

    #[test]
    fn expired_requests_are_dropped_even_without_word_from_the_agent() {
        let mut app = app();
        app.on_inbox(request(7, "alice"));
        app.prune_pending(Instant::now() + Duration::from_secs(400));
        assert!(app.requests.waiting.is_empty());
    }
}
