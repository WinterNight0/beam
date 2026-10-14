//! Add a friend (GUI EX Patch #4): the shared pairing worker
//! (`beam_core::actions::pair_worker`) and the shared checks
//! (`actions::pair::plan_join`), the same ones the full-screen view runs.
//!
//! The rules come with them: the code is typed before connecting, both
//! people compare fingerprints, the key saved is the one the connection
//! proved, and a question left unanswered, or abandoned by closing the
//! dialog, is a no. On top, the window's own safe defaults: the fingerprint
//! question is an `Ask` that starts on "Don't pair", Enter and Escape mean
//! no, and Pair works only once "we compared them" is ticked. An invite for
//! a friend already paired never pairs again; moving them to another relay
//! is a second `Ask` that starts on "Keep" (ADR-0038).

use std::time::{Duration, Instant};

use beam_core::actions::pair::{self, JoinPlan, JoinRefused, Location};
use beam_core::actions::pair_worker::{Answer, CodeAfter, Role, Unpaired, Update, Worker};
use beam_core::pairing::PairingCode;
use beam_core::untrusted;
use beam_model::ask::{RelayMove, SameFingerprints};
use beam_model::{Ask, Choice};
use iced::Task;

use super::{Dialog, Gui, Message, Tone};

/// Where the Add friend dialog is.
#[derive(Debug)]
pub enum Pairing {
    /// Paste their invite and choose a name, or show mine instead.
    Start {
        invite: String,
        name: String,
        error: Option<String>,
    },
    /// My invite and code are shown; one attempt is waited for.
    Showing {
        invite: String,
        code: String,
        until: Instant,
    },
    /// Type the code the other device shows. Checked here, so a malformed
    /// code never reaches the network.
    Code { code: String, error: Option<String> },
    /// Something is happening that needs no answer.
    Busy(String),
    /// Do the fingerprints match? Starts on No; Pair needs the tick.
    Confirm {
        ask: Ask<SameFingerprints>,
        compared: bool,
    },
    /// Ended and saved nothing: why, already cleaned.
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum PairMsg {
    Invite(String),
    Name(String),
    /// Pair with the pasted invite.
    Join,
    /// Show this device's invite and a code.
    ShowMine,
    Code(String),
    SendCode,
    Compared(bool),
    /// The answer to the fingerprint question.
    Answer(Choice),
    /// The answer to "move this friend to another relay?".
    AnswerRelay(Choice),
    CopyInvite(String),
    /// Read what the worker has said.
    Tick,
}

/// A running pairing. Dropping it cancels the pairing: any question still
/// waiting reads as no, and nothing is saved.
pub(crate) struct Running(pub(crate) Worker);

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Running")
    }
}

/// The text boxes, so the dialog can put the cursor where typing starts.
pub(crate) const INVITE_INPUT: &str = "pair-invite";
pub(crate) const CODE_INPUT: &str = "pair-code";

impl Gui {
    pub(crate) fn add_friend(&mut self) -> Task<Message> {
        self.worker = None;
        self.dialog = Some(Dialog::Pair(Pairing::Start {
            invite: String::new(),
            name: String::new(),
            error: None,
        }));
        iced::widget::operation::focus(INVITE_INPUT)
    }

    pub(crate) fn on_pair(&mut self, message: PairMsg) -> Task<Message> {
        match message {
            PairMsg::Invite(text) => {
                if let Some(Dialog::Pair(Pairing::Start { invite, error, .. })) = &mut self.dialog {
                    *invite = text;
                    *error = None;
                }
            }
            PairMsg::Name(text) => {
                if let Some(Dialog::Pair(Pairing::Start { name, error, .. })) = &mut self.dialog {
                    *name = text;
                    *error = None;
                }
            }
            PairMsg::Join => self.join(),
            PairMsg::ShowMine => {
                if let Some(store) = &self.store {
                    self.worker =
                        Some(Running(Worker::start(store.clone(), Role::Wait, self.bind)));
                    self.dialog = Some(Dialog::Pair(Pairing::Busy(
                        "Getting this device's invite ready…".into(),
                    )));
                }
            }
            PairMsg::Code(text) => {
                if let Some(Dialog::Pair(Pairing::Code { code, error })) = &mut self.dialog {
                    // Digits only, six at most; spaces are dropped as typed.
                    *code = text.chars().filter(char::is_ascii_digit).take(6).collect();
                    *error = None;
                }
            }
            PairMsg::SendCode => self.send_code(),
            PairMsg::Compared(yes) => {
                if let Some(Dialog::Pair(Pairing::Confirm { compared, .. })) = &mut self.dialog {
                    *compared = yes;
                }
            }
            PairMsg::Answer(choice) => self.answer_fingerprints(choice),
            PairMsg::AnswerRelay(choice) => self.answer_relay(choice),
            PairMsg::CopyInvite(invite) => {
                self.notice = Some(("Invite copied. Send it to your friend.".into(), Tone::Info));
                return iced::clipboard::write(invite);
            }
            PairMsg::Tick => return self.drain_worker(),
        }
        Task::none()
    }

    /// Checks the pasted invite and the name before any network, then
    /// pairs, or only updates where an existing friend is found.
    fn join(&mut self) {
        let Some(Dialog::Pair(Pairing::Start { invite, name, .. })) = &self.dialog else {
            return;
        };
        let (text, name) = (invite.clone(), name.trim().to_string());
        let Some(store) = self.store.clone() else {
            return;
        };
        let refused = |gui: &mut Self, why: String| {
            if let Some(Dialog::Pair(Pairing::Start { error, .. })) = &mut gui.dialog {
                *error = Some(why);
            }
        };
        match pair::plan_join(&store, &text, &name) {
            Ok(JoinPlan::Pair(invite)) => {
                let role = Role::Join { invite, name };
                self.worker = Some(Running(Worker::start(store, role, self.bind)));
                self.dialog = Some(Dialog::Pair(Pairing::Busy("Starting…".into())));
            }
            Ok(JoinPlan::AlreadyPaired { invite, location }) => {
                if location.changes_relay() {
                    self.dialog = Some(Dialog::RelayMove(Ask::new(relay_move(&location, &text))));
                } else {
                    self.save_location(&invite.to_string(), false);
                }
            }
            Err(JoinRefused::OwnInvite) => refused(
                self,
                "That is this device's own invite. Ask your friend for theirs.".into(),
            ),
            Err(e) => refused(self, untrusted::lines(&e.to_string())),
        }
    }

    fn send_code(&mut self) {
        let Some(Dialog::Pair(Pairing::Code { code, .. })) = &self.dialog else {
            return;
        };
        match PairingCode::parse(code) {
            Ok(parsed) => {
                let Some(running) = &self.worker else { return };
                running.0.answer(Answer::Code(Some(parsed.grouped())));
                self.dialog = Some(Dialog::Pair(Pairing::Busy(
                    "Checking the code with your friend's device…".into(),
                )));
            }
            Err(e) => {
                if let Some(Dialog::Pair(Pairing::Code { error, .. })) = &mut self.dialog {
                    *error = Some(untrusted::text(&e.to_string()));
                }
            }
        }
    }

    fn answer_fingerprints(&mut self, choice: Choice) {
        let Some(Dialog::Pair(Pairing::Confirm { compared, .. })) = &self.dialog else {
            return;
        };
        // Yes needs the tick; anything else is no.
        let yes = choice == Choice::Yes && *compared;
        if let Some(running) = &self.worker {
            running.0.answer(Answer::Confirm(yes));
        }
        self.dialog = Some(Dialog::Pair(Pairing::Busy(if yes {
            "Waiting for your friend to answer…".into()
        } else {
            "Not pairing…".into()
        })));
    }

    fn answer_relay(&mut self, choice: Choice) {
        let Some(Dialog::RelayMove(ask)) = self.dialog.take() else {
            return;
        };
        let question = ask.into_question();
        if choice == Choice::Yes {
            self.save_location(&question.invite, true);
        } else {
            self.notice = Some((
                format!("Kept {}'s relay. Nothing was changed.", question.name),
                Tone::Info,
            ));
        }
    }

    /// Saves where an already-paired friend is found now. Never their key.
    fn save_location(&mut self, invite_text: &str, relay_change_agreed: bool) {
        let Some(store) = &self.store else { return };
        let saved = invite_text
            .trim()
            .parse()
            .map_err(|e: beam_core::invite::InviteError| e.to_string())
            .and_then(|invite| {
                pair::save_location(store, &invite, relay_change_agreed).map_err(|e| e.to_string())
            });
        self.dialog = None;
        self.notice = Some(match saved {
            Ok(name) => (
                format!(
                    "{} is already your friend. Updated where to find them; their key is unchanged.",
                    untrusted::name(&name)
                ),
                Tone::Success,
            ),
            Err(why) => (
                format!("Nothing was saved: {}", untrusted::lines(&why)),
                Tone::Problem,
            ),
        });
        self.reload();
    }

    /// What the worker said since the last look.
    fn drain_worker(&mut self) -> Task<Message> {
        let mut updates = Vec::new();
        if let Some(running) = &self.worker {
            while let Ok(update) = running.0.updates.try_recv() {
                updates.push(update);
            }
        }
        let mut task = Task::none();
        for update in updates {
            task = self.on_update(update);
        }
        task
    }

    fn on_update(&mut self, update: Update) -> Task<Message> {
        let state = match update {
            Update::Waiting {
                invite,
                code,
                expires_in,
            } => Pairing::Showing {
                invite,
                code,
                until: Instant::now() + expires_in,
            },
            Update::Attempt { peer } => Pairing::Busy(format!(
                "A device ({peer}…) is connecting. The code is now used up."
            )),
            Update::Connecting { peer } => {
                Pairing::Busy(format!("Connecting to your friend's device ({peer}…)…"))
            }
            Update::AskCode => {
                self.dialog = Some(Dialog::Pair(Pairing::Code {
                    code: String::new(),
                    error: None,
                }));
                return iced::widget::operation::focus(CODE_INPUT);
            }
            Update::AskConfirm { name, peer, own } => Pairing::Confirm {
                ask: Ask::new(SameFingerprints {
                    name: untrusted::name(&name),
                    peer,
                    own,
                }),
                compared: false,
            },
            Update::Finished(Ok(paired)) => {
                self.worker = None;
                self.dialog = None;
                self.notice = Some((format!("Paired with {}.", paired.name), Tone::Success));
                self.selected = Some(paired.name);
                self.reload();
                return Task::none();
            }
            Update::Finished(Err(unpaired)) => {
                self.worker = None;
                Pairing::Failed(words(&unpaired))
            }
        };
        // A dialog closed meanwhile stays closed.
        if matches!(self.dialog, Some(Dialog::Pair(_))) {
            self.dialog = Some(Dialog::Pair(state));
        }
        Task::none()
    }

    /// Enter and Escape in the pairing dialogs. Enter on a question gives the
    /// highlighted answer, which starts on no; Escape always means no.
    pub(crate) fn pair_key(&mut self, named: iced::keyboard::key::Named) {
        use iced::keyboard::key::Named;
        match (&self.dialog, named) {
            (Some(Dialog::Pair(Pairing::Confirm { ask, .. })), Named::Enter) => {
                let choice = ask.focus();
                self.answer_fingerprints(choice);
            }
            (Some(Dialog::Pair(Pairing::Confirm { .. })), Named::Escape) => {
                self.answer_fingerprints(Choice::No);
            }
            (Some(Dialog::RelayMove(ask)), Named::Enter) => {
                let choice = ask.focus();
                self.answer_relay(choice);
            }
            (Some(Dialog::RelayMove(_)), Named::Escape) => self.answer_relay(Choice::No),
            (Some(Dialog::Pair(Pairing::Start { .. })), Named::Enter) => self.join(),
            (Some(Dialog::Pair(Pairing::Code { .. })), Named::Enter) => self.send_code(),
            (Some(Dialog::Pair(_)), Named::Escape) => self.close_dialog(),
            _ => {}
        }
    }

    /// Closes whatever is open. Closing a pairing cancels it: the worker is
    /// dropped, a question still waiting reads as no, nothing is saved.
    pub(crate) fn close_dialog(&mut self) {
        self.dialog = None;
        self.worker = None;
    }

    /// Minutes left on the shown code, for the dialog.
    pub(crate) fn code_left(until: Instant) -> Duration {
        until.saturating_duration_since(Instant::now())
    }
}

/// The relay question, in a person's words: "your own relay" when a friend
/// uses this device's.
fn relay_move(location: &Location, invite: &str) -> RelayMove {
    let describe = |relay: &Option<String>| match relay {
        Some(url) => untrusted::text(url),
        None => format!("your own relay ({})", location.own_relay),
    };
    RelayMove {
        name: untrusted::name(&location.name),
        fingerprint: location.fingerprint.hex(),
        old: describe(&location.relay_now),
        new: describe(&location.relay_after),
        invite: invite.trim().to_string(),
    }
}

/// How a pairing that saved nothing is told, as `beam pair` tells it.
pub(crate) fn words(unpaired: &Unpaired) -> String {
    match unpaired {
        Unpaired::Failed(reason) => reason.clone(),
        Unpaired::NotPaired { reason, code } => {
            let hint = match code {
                CodeAfter::Nothing => "",
                CodeAfter::UsedUpHere => {
                    " The code is used up: choose Show my invite again for a new one."
                }
                CodeAfter::UsedUpThere => " Their code is now used up: ask them for the new one.",
            };
            format!("Not paired: {reason}. Nothing was saved.{hint}")
        }
    }
}
