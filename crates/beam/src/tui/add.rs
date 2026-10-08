//! The Add friend tab and the pairing pop-ups (ADR-0043, step 4).
//!
//! The tab is a small form: their invite, a name for them, **Pair**, and
//! **Show my invite**. Pairing then runs in [`super::pairing`] and asks its
//! questions through pop-ups:
//!
//! ```text
//! Pair ─▶ code pop-up ─▶ connecting ─▶ fingerprint check ─▶ paired
//! Show my invite ─▶ invite + code, counting down ─▶ fingerprint check ─▶ paired
//! ```
//!
//! The fingerprint check starts on **No**, as Remove starts on Keep: Enter
//! alone never pairs. Esc in any of them cancels, and nothing is saved.

use std::time::{Duration, Instant};

use super::app::{App, Choice, Effect, Key, Modal, Tab, Target};
use super::input::Input;
use super::pairing::{Answer, Update};
use crate::identity::MAX_NAME_LEN;
use crate::pairing::PairingCode;

/// The longest invite the box takes; real ones are a few hundred characters.
const MAX_INVITE: usize = 2048;
/// "123 456", with room for dashes or extra spaces.
const MAX_CODE: usize = 16;

/// The parts of the Add friend form, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Invite,
    Name,
    Pair,
    ShowMine,
}

impl Field {
    const ORDER: [Field; 4] = [Field::Invite, Field::Name, Field::Pair, Field::ShowMine];

    fn step(self, by: isize) -> Field {
        let i = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0) as isize;
        Self::ORDER[(i + by).rem_euclid(Self::ORDER.len() as isize) as usize]
    }
}

/// The Add friend form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddForm {
    pub invite: Input,
    pub name: Input,
    /// Which part has the keyboard; `None` gives the keys back to the page.
    pub focus: Option<Field>,
    pub error: Option<String>,
}

impl Default for AddForm {
    fn default() -> Self {
        Self {
            invite: Input::new(MAX_INVITE),
            name: Input::new(MAX_NAME_LEN),
            focus: None,
            error: None,
        }
    }
}

impl AddForm {
    fn focused_input(&mut self) -> Option<&mut Input> {
        match self.focus {
            Some(Field::Invite) => Some(&mut self.invite),
            Some(Field::Name) => Some(&mut self.name),
            _ => None,
        }
    }
}

/// Whether a pop-up belongs to pairing.
pub fn is_pairing(modal: &Modal) -> bool {
    matches!(
        modal,
        Modal::EnterCode { .. }
            | Modal::Busy { .. }
            | Modal::ShowInvite { .. }
            | Modal::ConfirmPair { .. }
            | Modal::RelayChange { .. }
    )
}

impl App {
    /// Switches tab; arriving at Add friend puts the cursor in the invite box.
    pub(super) fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::AddFriend && self.add.focus.is_none() && self.snapshot.me.is_some() {
            self.add.focus = Some(Field::Invite);
        }
    }

    /// Whether keys go to the form rather than the page.
    pub(super) fn form_has_keys(&self) -> bool {
        self.tab == Tab::AddFriend && self.add.focus.is_some()
    }

    pub(super) fn paste_into_form(&mut self, text: &str) {
        if let Some(input) = self.add.focused_input() {
            input.paste(text);
            self.add.error = None;
        }
    }

    /// Keys while the form has the keyboard.
    pub(super) fn on_form_key(&mut self, key: Key) -> Option<Effect> {
        let focus = self.add.focus?;
        match key {
            Key::Esc => self.add.focus = None,
            Key::Palette => self.open_palette_from_form(),
            Key::Copy => return self.copy_from_form(),
            Key::Tab | Key::Down => self.add.focus = Some(focus.step(1)),
            Key::BackTab | Key::Up => self.add.focus = Some(focus.step(-1)),
            Key::Enter => match focus {
                Field::Invite => self.add.focus = Some(Field::Name),
                Field::Name | Field::Pair => return self.submit_join(),
                Field::ShowMine => return self.start_wait(),
            },
            Key::Char(c) => {
                if let Some(input) = self.add.focused_input() {
                    input.insert(c);
                    self.add.error = None;
                }
            }
            key => {
                if let (Some(edit), Some(input)) =
                    (super::app::edit_for(key), self.add.focused_input())
                {
                    input.edit(edit);
                    self.add.error = None;
                }
            }
        }
        None
    }

    fn open_palette_from_form(&mut self) {
        self.add.focus = None;
        self.on_key(Key::Palette);
    }

    fn copy_from_form(&mut self) -> Option<Effect> {
        let me = self.snapshot.me.as_ref()?;
        Some(Effect::Copy {
            what: "your fingerprint".to_string(),
            text: format!("SHA256:{}", me.fingerprint),
        })
    }

    fn submit_join(&mut self) -> Option<Effect> {
        let invite = self.add.invite.text().trim().to_string();
        let name = self.add.name.text().trim().to_string();
        if invite.is_empty() {
            self.add.error = Some("Paste their invite first (it starts with beam1).".to_string());
            self.add.focus = Some(Field::Invite);
            return None;
        }
        if name.is_empty() {
            self.add.error = Some("Choose a name for them, such as alice.".to_string());
            self.add.focus = Some(Field::Name);
            return None;
        }
        self.add.error = None;
        Some(Effect::StartJoin { invite, name })
    }

    fn start_wait(&mut self) -> Option<Effect> {
        self.add.error = None;
        Some(Effect::StartWait)
    }

    /// A click on the form.
    pub(super) fn click_field(&mut self, field: Field, column: u16, width: u16) -> Option<Effect> {
        self.add.focus = Some(field);
        match field {
            Field::Invite => self.add.invite.click(column, width),
            Field::Name => self.add.name.click(column, width),
            Field::Pair => return self.submit_join(),
            Field::ShowMine => return self.start_wait(),
        }
        None
    }

    /// The pairing thread moved on.
    pub fn on_pair(&mut self, update: Update) {
        self.modal = match update {
            Update::Waiting {
                invite,
                code,
                expires_in,
            } => Some(Modal::ShowInvite {
                invite,
                code,
                expires: Instant::now() + expires_in,
                attempt: None,
            }),
            Update::Attempt { peer } => match self.modal.take() {
                Some(Modal::ShowInvite {
                    invite,
                    code,
                    expires,
                    ..
                }) => Some(Modal::ShowInvite {
                    invite,
                    code,
                    expires,
                    attempt: Some(peer),
                }),
                other => other,
            },
            Update::Connecting { peer } => Some(Modal::Busy {
                text: format!("Connecting to the device in the invite ({peer})…"),
            }),
            Update::AskCode => Some(Modal::EnterCode {
                input: Input::new(MAX_CODE),
                error: None,
            }),
            Update::AskConfirm { name, peer, own } => Some(Modal::ConfirmPair {
                name,
                peer,
                own,
                focus: Choice::No,
            }),
            Update::Finished(Ok((name, fingerprint))) => {
                self.flash = Some(format!("Paired with {name}."));
                self.add = AddForm::default();
                self.tab = Tab::Friends;
                self.pending_select = Some(fingerprint);
                None
            }
            Update::Finished(Err(reason)) => Some(Modal::Output {
                title: "Not paired".to_string(),
                text: reason,
                scroll: 0,
                failed: true,
            }),
        };
    }

    /// Keys in a pairing pop-up. The pop-up was taken out of `self`.
    pub(super) fn on_pair_modal_key(&mut self, modal: Modal, key: Key) -> Option<Effect> {
        match modal {
            Modal::EnterCode {
                mut input,
                mut error,
            } => {
                match key {
                    Key::Esc => return Some(Effect::Answer(Answer::Code(None))),
                    Key::Enter => return self.answer_code(input),
                    Key::Char(c) if c.is_ascii_digit() || matches!(c, ' ' | '-') => {
                        input.insert(c);
                        error = None;
                    }
                    key => {
                        if let Some(edit) = super::app::edit_for(key) {
                            input.edit(edit);
                            error = None;
                        }
                    }
                }
                self.modal = Some(Modal::EnterCode { input, error });
                None
            }
            Modal::Busy { text } => {
                if key == Key::Esc {
                    return Some(Effect::CancelPair);
                }
                self.modal = Some(Modal::Busy { text });
                None
            }
            Modal::ShowInvite {
                invite,
                code,
                expires,
                attempt,
            } => {
                let effect = match key {
                    Key::Esc => return Some(Effect::CancelPair),
                    Key::Copy | Key::Char('c') | Key::Enter => Some(Effect::Copy {
                        what: "your invite".to_string(),
                        text: invite.clone(),
                    }),
                    _ => None,
                };
                self.modal = Some(Modal::ShowInvite {
                    invite,
                    code,
                    expires,
                    attempt,
                });
                effect
            }
            Modal::ConfirmPair {
                name,
                peer,
                own,
                focus,
            } => {
                let focus = match key {
                    Key::Esc => return self.answer_pair(false),
                    Key::Enter => return self.answer_pair(focus == Choice::Yes),
                    Key::Left | Key::Right | Key::Tab | Key::BackTab => flip(focus),
                    _ => focus,
                };
                self.modal = Some(Modal::ConfirmPair {
                    name,
                    peer,
                    own,
                    focus,
                });
                None
            }
            Modal::RelayChange {
                name,
                fingerprint,
                old,
                new,
                invite,
                focus,
            } => {
                let focus = match key {
                    Key::Esc => return self.keep_relay(),
                    Key::Enter if focus == Choice::Yes => {
                        return Some(Effect::UpdateLocation { invite });
                    }
                    Key::Enter => return self.keep_relay(),
                    Key::Left | Key::Right | Key::Tab | Key::BackTab => flip(focus),
                    _ => focus,
                };
                self.modal = Some(Modal::RelayChange {
                    name,
                    fingerprint,
                    old,
                    new,
                    invite,
                    focus,
                });
                None
            }
            other => {
                self.modal = Some(other);
                None
            }
        }
    }

    /// A click in a pairing pop-up: its buttons, or the code box.
    pub(super) fn on_pair_modal_click(
        &mut self,
        modal: Modal,
        hit: Option<(ratatui::layout::Rect, Target)>,
        column: u16,
    ) -> Option<Effect> {
        match (modal, hit) {
            (Modal::EnterCode { input, .. }, Some((_, Target::Button(Choice::Yes)))) => {
                self.answer_code(input)
            }
            (Modal::EnterCode { mut input, error }, Some((area, Target::Input))) => {
                input.click(column - area.x, area.width);
                self.modal = Some(Modal::EnterCode { input, error });
                None
            }
            (Modal::EnterCode { .. }, Some((_, Target::Button(Choice::No)))) => {
                Some(Effect::Answer(Answer::Code(None)))
            }
            (Modal::Busy { .. }, Some((_, Target::Button(Choice::No)))) => Some(Effect::CancelPair),
            (modal @ Modal::ShowInvite { .. }, Some((_, Target::Button(Choice::Yes)))) => {
                self.modal = Some(modal);
                self.on_key(Key::Copy)
            }
            (Modal::ShowInvite { .. }, Some((_, Target::Button(Choice::No)))) => {
                Some(Effect::CancelPair)
            }
            (Modal::ConfirmPair { .. }, Some((_, Target::Button(answer)))) => {
                self.answer_pair(answer == Choice::Yes)
            }
            (Modal::RelayChange { invite, .. }, Some((_, Target::Button(Choice::Yes)))) => {
                Some(Effect::UpdateLocation { invite })
            }
            (Modal::RelayChange { .. }, Some((_, Target::Button(Choice::No)))) => self.keep_relay(),
            // A click beside a pop-up leaves it open.
            (modal, _) => {
                self.modal = Some(modal);
                None
            }
        }
    }

    fn answer_code(&mut self, input: Input) -> Option<Effect> {
        match PairingCode::parse(input.text()) {
            Ok(code) => {
                self.modal = Some(Modal::Busy {
                    text: "Connecting…".to_string(),
                });
                Some(Effect::Answer(Answer::Code(Some(code.grouped()))))
            }
            // A malformed code never reaches the network, so this is not a
            // guess: the person can simply fix it.
            Err(e) => {
                self.modal = Some(Modal::EnterCode {
                    input,
                    error: Some(e.to_string()),
                });
                None
            }
        }
    }

    fn answer_pair(&mut self, yes: bool) -> Option<Effect> {
        self.modal = Some(Modal::Busy {
            text: if yes {
                "Waiting for the other person to confirm too…".to_string()
            } else {
                "Telling the other device you said no…".to_string()
            },
        });
        Some(Effect::Answer(Answer::Confirm(yes)))
    }

    fn keep_relay(&mut self) -> Option<Effect> {
        self.flash = Some("Nothing was changed.".to_string());
        None
    }
}

fn flip(choice: Choice) -> Choice {
    match choice {
        Choice::Yes => Choice::No,
        Choice::No => Choice::Yes,
    }
}

/// Minutes and seconds left, for the invite's countdown.
pub fn countdown(expires: Instant) -> String {
    let left = expires.saturating_duration_since(Instant::now());
    let secs = left.as_secs();
    if left == Duration::ZERO {
        "expired".to_string()
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}
