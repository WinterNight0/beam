//! Receiving (GUI EX Patch #5): the Receiving switch, the requests waiting
//! for an answer, and the Accept question.
//!
//! The same pieces as the full-screen view: the switch's receiver
//! (`beam_core::actions::switch`, which is the background agent's receiver
//! run in this window), the link to whichever receiver runs
//! (`actions::inbox_link`), and the model's rules (`beam_model::requests`).
//! So the rules come with them: only paired friends reach the question,
//! every file waits for a person's answer, no byte is sent before Accept,
//! an unanswered request expires as a no after five minutes, and the
//! receiver takes one answer per request.
//!
//! The window's own safe defaults: a request never opens the question by
//! itself (a stray Enter while typing must not answer it); the question is an
//! `Ask` that starts on Decline, so Enter declines; Escape means "later",
//! leaving it waiting; only a click on Accept accepts.

use std::time::{Duration, Instant};

use beam_core::actions::inbox;
use beam_core::actions::inbox_link::Link;
use beam_core::actions::switch::InView;
use beam_model::ask::{Incoming, LeaveMidTransfer, StopMidTransfer};
use beam_model::requests::{Happened, Taken, Toggle};
use beam_model::{Ask, Choice};
use iced::{Task, window};

use super::{Dialog, Gui, Message, Tone};

#[derive(Debug, Clone)]
pub enum RecvMsg {
    /// The Receiving switch was pressed.
    Toggle,
    /// Open the Accept question for request `id`.
    Review(u64),
    /// The answer to the Accept question.
    Answer(Choice),
    /// The answer to "stop receiving while a file arrives?".
    AnswerStop(Choice),
    /// The answer to "close beam while a file arrives?".
    AnswerLeave(Choice),
    /// Read what the receiver and the link said.
    Tick,
    /// The window's close button.
    CloseRequested(window::Id),
}

/// The switch's receiver and the link, held while they run. Dropping the
/// receiver stops it; dropping the link closes it.
#[derive(Default)]
pub(crate) struct Receivers {
    pub(crate) switch: Option<InView>,
    pub(crate) link: Option<Link>,
}

impl std::fmt::Debug for Receivers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receivers")
            .field("switch", &self.switch.is_some())
            .field("link", &self.link.is_some())
            .finish()
    }
}

/// How long closing the window waits for the receiver to tell a sender.
const STOP_PATIENCE: Duration = Duration::from_secs(3);

impl Gui {
    pub(crate) fn on_receive(&mut self, message: RecvMsg) -> Task<Message> {
        match message {
            RecvMsg::Toggle => self.toggle(),
            RecvMsg::Review(id) => self.review(id),
            RecvMsg::Answer(choice) => self.answer(choice),
            RecvMsg::AnswerStop(choice) => {
                if matches!(self.dialog, Some(Dialog::StopReceiving(_))) {
                    self.dialog = None;
                    if choice == Choice::Yes {
                        self.requests.stop();
                        self.stop_switch();
                    }
                }
            }
            RecvMsg::AnswerLeave(choice) => {
                if let Some(Dialog::Leave(_, id)) = self.dialog.take() {
                    if choice == Choice::Yes {
                        return self.leave(id);
                    }
                    // Stay: a restart that asked is called off too.
                    self.restarting = false;
                }
            }
            RecvMsg::Tick => self.drain_receivers(Instant::now()),
            RecvMsg::CloseRequested(id) => {
                if self.requests.arriving_here() || self.sending_now() {
                    // Starts on No: stay, and let the file finish.
                    self.dialog = Some(Dialog::Leave(Ask::new(LeaveMidTransfer), id));
                    return Task::none();
                }
                return self.leave(id);
            }
        }
        Task::none()
    }

    fn toggle(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        match self.requests.toggle(snapshot) {
            Toggle::Start => {
                if let Some(store) = &self.store {
                    self.receivers.switch =
                        Some(InView::start_on(store.clone(), self.bind, self.notify));
                }
            }
            Toggle::Stop => self.stop_switch(),
            // A file is arriving: ask first; the question starts on No.
            Toggle::AskFirst => {
                self.dialog = Some(Dialog::StopReceiving(Ask::new(StopMidTransfer)));
            }
            Toggle::Taken(taken) => {
                let who = match taken {
                    Taken::Agent => "The background agent already receives for this device.",
                    Taken::OtherView => "Another beam window already receives for this device.",
                    Taken::Listen => "beam listen already receives in a terminal.",
                };
                self.notice = Some((format!("{who} Requests still show up here."), Tone::Info));
            }
            Toggle::Busy => {}
        }
    }

    fn stop_switch(&mut self) {
        if let Some(switch) = &mut self.receivers.switch {
            switch.stop();
        }
    }

    fn review(&mut self, id: u64) {
        if let Some(p) = self.requests.waiting.iter().find(|p| p.id == id) {
            self.dialog = Some(Dialog::Accept(Ask::new(Incoming {
                id: p.id,
                request: p.request.clone(),
                expires: p.expires,
            })));
        }
    }

    /// Yes from a click on Accept, or the highlighted answer from Enter,
    /// which starts on Decline. The request leaves the list at once, so it
    /// cannot be answered twice; the receiver has the last word.
    pub(crate) fn answer(&mut self, choice: Choice) {
        let Some(Dialog::Accept(ask)) = self.dialog.take() else {
            return;
        };
        let Incoming { id, request, .. } = ask.into_question();
        let accept = choice == Choice::Yes;
        if let Some(link) = &self.receivers.link {
            link.answer(id, accept);
        }
        self.requests.answered(id);
        self.notice = Some(if accept {
            (
                format!(
                    "Receiving {} from {}…",
                    request.file_name, request.peer_name
                ),
                Tone::Success,
            )
        } else {
            (
                format!("Declined {} from {}.", request.file_name, request.peer_name),
                Tone::Info,
            )
        });
    }

    /// Escape on the Accept question: later. It keeps waiting, and expires
    /// as a no if nobody answers.
    pub(crate) fn later(&mut self) {
        if matches!(self.dialog, Some(Dialog::Accept(_))) {
            self.dialog = None;
        }
    }

    /// Connects to the receiver for this beam home, if one runs and the
    /// window is not connected yet. Tried at every refresh, so a background
    /// agent started from a terminal shows up within seconds.
    pub(crate) fn connect_link(&mut self) {
        if self.receivers.link.is_some() {
            return;
        }
        let Some(store) = &self.store else { return };
        if let Ok(address) = inbox::find(store) {
            self.receivers.link = Some(Link::connect(address.port, address.token));
        }
    }

    fn drain_receivers(&mut self, now: Instant) {
        let mut switch_changed = false;
        if let Some(switch) = &self.receivers.switch {
            let updates: Vec<_> = switch.updates.try_iter().collect();
            for update in updates {
                switch_changed = true;
                if let beam_model::requests::SwitchUpdate::Stopped(why) = &update {
                    self.receivers.switch = None;
                    if let Some(why) = why {
                        self.notice = Some((format!("Receiving stopped: {why}"), Tone::Problem));
                    }
                }
                self.requests.on_switch(&update);
            }
        }
        if switch_changed {
            self.reload();
            self.connect_link();
        }
        let mut gone = false;
        if let Some(link) = &self.receivers.link {
            let updates: Vec<_> = link.updates.try_iter().collect();
            for update in updates {
                gone |= matches!(update, beam_model::requests::Update::Gone { .. });
                let happened = self.requests.on_update(update, now);
                self.happened(happened);
            }
        }
        if gone {
            self.receivers.link = None;
        }
        for id in self.requests.prune(now) {
            self.close_accept(id);
        }
    }

    /// Says what the receiver did. A new request is only announced: the
    /// question opens when the person asks for it.
    fn happened(&mut self, happened: Happened) {
        match happened {
            Happened::Asked {
                peer_name,
                file_name,
            } => {
                self.notice = Some((
                    format!("{peer_name} wants to send you {file_name}. Open the Inbox to answer."),
                    Tone::Info,
                ));
            }
            Happened::Closed { id, .. } => {
                if self.close_accept(id) {
                    self.notice = Some((
                        "That request is no longer waiting (answered elsewhere, or it expired)."
                            .into(),
                        Tone::Info,
                    ));
                }
            }
            Happened::TooLate => {
                self.notice = Some((
                    "That answer came too late and did not count.".into(),
                    Tone::Problem,
                ));
            }
            Happened::Finished { text } => {
                self.notice = Some((text, Tone::Success));
                self.reload();
            }
            Happened::Connected | Happened::Progress | Happened::Gone => {}
        }
    }

    /// Closes the Accept question if it is for `id`; whether it was.
    fn close_accept(&mut self, id: u64) -> bool {
        if matches!(&self.dialog, Some(Dialog::Accept(ask)) if ask.question().id == id) {
            self.dialog = None;
            return true;
        }
        false
    }

    /// Closes the window. The receiver stops first, the way `listen` stops on
    /// Ctrl+C: a sender mid-transfer is told, and what arrived is kept.
    fn leave(&mut self, id: window::Id) -> Task<Message> {
        // Restart now: the new beam starts first; if it cannot, this one
        // stays open and says so.
        if !self.start_again_if_restarting() {
            return Task::none();
        }
        self.receivers.link = None;
        if let Some(switch) = self.receivers.switch.take() {
            switch.stop_and_wait(STOP_PATIENCE);
        }
        self.worker = None;
        self.stop_sending_for_close();
        window::close(id)
    }
}

/// What the Accept question says about time left.
pub(crate) fn left(expires: Instant) -> String {
    let secs = expires.saturating_duration_since(Instant::now()).as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}
