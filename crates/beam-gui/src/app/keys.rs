//! The keyboard (GUI EX Patch #8): every flow works without a mouse.
//!
//! Shortcuts go by the key's Latin letter (`Key::to_latin`), so Ctrl+F is
//! Ctrl+F on a Thai or Russian layout too. They only arrive when no text
//! box took the key: typing in a search box never switches pages.
//!
//! On a question, the marked answer is the one Enter gives. It starts on
//! the safe one (`Ask` starts on No); Tab, or Left and Right, move it, so
//! a yes by keyboard is as deliberate as a click. Escape is always the
//! safe answer ("later" on the Accept question).

use beam_model::Choice;
use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};

use super::{Dialog, Gui, Message, Page, PairMsg, Pairing, RecvMsg, SendMsg, SettingsMsg};

/// The friends list's search box and the history's: Ctrl+F focuses them.
pub(crate) const FRIEND_SEARCH: &str = "friend-search";
pub(crate) const HISTORY_SEARCH: &str = "history-search";

/// Interface zoom steps, on top of the screen's own scaling.
pub const ZOOMS: [f32; 8] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// Where Tab, Left or Right move a question's mark.
#[derive(Clone, Copy)]
enum Steer {
    Flip,
    Onto(Choice),
}

impl Gui {
    pub(crate) fn on_key(&mut self, key: Key, mods: Modifiers) -> Task<Message> {
        if self.dialog.is_some() {
            return self.dialog_key(key, mods);
        }
        let Some(snapshot) = &self.snapshot else {
            return Task::none();
        };
        if snapshot.needs_setup {
            // The welcome card has one button.
            if key == Key::Named(Named::Enter) {
                return self.update(Message::CreateIdentity);
            }
            return Task::none();
        }
        if mods.command() {
            let Key::Character(c) = key.as_ref() else {
                return Task::none();
            };
            let page = match c {
                "1" => Some(Page::Friends),
                "2" => Some(Page::Transfers),
                "3" => Some(Page::Inbox),
                "4" => Some(Page::History),
                "5" => Some(Page::Settings),
                _ => None,
            };
            if let Some(page) = page {
                return self.update(Message::Page(page));
            }
            return match c {
                "f" => self.find(),
                "n" => self.update(Message::AddFriend),
                "o" => match self.selected.clone() {
                    Some(name) => self.update(Message::Send(SendMsg::Pick(name))),
                    None => Task::none(),
                },
                "r" => self.update(Message::Refresh),
                "=" | "+" => self.update(Message::Zoom(self.zoom_step(1))),
                "-" => self.update(Message::Zoom(self.zoom_step(-1))),
                "0" => self.update(Message::Zoom(1.0)),
                _ => Task::none(),
            };
        }
        let on_friends = self.page == Page::Friends;
        match key {
            Key::Named(Named::F5) => self.update(Message::Refresh),
            Key::Named(Named::ArrowDown) if on_friends => {
                self.step_friend(1);
                Task::none()
            }
            Key::Named(Named::ArrowUp) if on_friends => {
                self.step_friend(-1);
                Task::none()
            }
            Key::Named(Named::F2) if on_friends => match self.selected.clone() {
                Some(name) => self.update(Message::Rename(name)),
                None => Task::none(),
            },
            // Asks first, starting on Keep.
            Key::Named(Named::Delete) if on_friends => match self.selected.clone() {
                Some(name) => self.update(Message::Remove(name)),
                None => Task::none(),
            },
            _ => Task::none(),
        }
    }

    /// Ctrl+F: the search box of the page, the friends list's by default.
    fn find(&mut self) -> Task<Message> {
        let id = if self.page == Page::History {
            HISTORY_SEARCH
        } else {
            self.page = Page::Friends;
            FRIEND_SEARCH
        };
        Task::batch([
            iced::widget::operation::focus(id),
            iced::widget::operation::select_all(id),
        ])
    }

    /// Up and Down walk the list as it is drawn: search applied, the
    /// recently active first.
    fn step_friend(&mut self, by: isize) {
        let (active, others) = self.listed(beam_core::history::unix_now());
        let names: Vec<String> = active
            .into_iter()
            .chain(others)
            .map(|f| f.name.clone())
            .collect();
        if names.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|s| names.iter().position(|n| n == s));
        let next = match at {
            Some(i) => i.saturating_add_signed(by).min(names.len() - 1),
            None if by > 0 => 0,
            None => names.len() - 1,
        };
        self.selected = Some(names[next].clone());
    }

    /// The next zoom step up (`by` = 1) or down (-1), within `ZOOMS`.
    pub(crate) fn zoom_step(&self, by: isize) -> f32 {
        let at = ZOOMS
            .iter()
            .position(|z| (z - self.zoom).abs() < 0.01)
            .unwrap_or(2);
        ZOOMS[at.saturating_add_signed(by).min(ZOOMS.len() - 1)]
    }

    fn dialog_key(&mut self, key: Key, mods: Modifiers) -> Task<Message> {
        let steer = match key.as_ref() {
            Key::Named(Named::Tab) => Some(Steer::Flip),
            // The risky answer is on the left, the safe one on the right.
            Key::Named(Named::ArrowLeft) => Some(Steer::Onto(Choice::Yes)),
            Key::Named(Named::ArrowRight) => Some(Steer::Onto(Choice::No)),
            _ => None,
        };
        if let Some(steer) = steer {
            if self.steer(steer) {
                return Task::none();
            }
            // A form: Tab goes to its next text box.
            if key == Key::Named(Named::Tab) {
                return if mods.shift() {
                    iced::widget::operation::focus_previous()
                } else {
                    iced::widget::operation::focus_next()
                };
            }
            return Task::none();
        }
        // Space ticks "we compared both fingerprints", as a click would.
        if key == Key::Named(Named::Space)
            && let Some(Dialog::Pair(Pairing::Confirm { compared, .. })) = &self.dialog
        {
            let yes = !*compared;
            return self.update(Message::Pair(PairMsg::Compared(yes)));
        }
        let Key::Named(named) = key else {
            return Task::none();
        };
        self.answer_key(named)
    }

    /// Moves the mark of the question that is open, if it is one.
    fn steer(&mut self, steer: Steer) -> bool {
        fn go<Q>(ask: &mut beam_model::Ask<Q>, steer: Steer) {
            match steer {
                Steer::Flip => ask.flip(),
                Steer::Onto(choice) => ask.point_at(choice),
            }
        }
        match &mut self.dialog {
            Some(Dialog::Remove(ask)) => go(ask, steer),
            Some(Dialog::RelayMove(ask)) => go(ask, steer),
            Some(Dialog::Accept(ask)) => go(ask, steer),
            Some(Dialog::StopReceiving(ask)) => go(ask, steer),
            Some(Dialog::Leave(ask, _)) => go(ask, steer),
            Some(Dialog::PortMapping(ask)) => go(ask, steer),
            Some(Dialog::ClearHistory(ask)) => go(ask, steer),
            Some(Dialog::Pair(Pairing::Confirm { ask, .. })) => go(ask, steer),
            _ => return false,
        }
        true
    }

    /// Enter and Escape on a dialog. Enter gives the marked answer, which
    /// starts on the safe one; Escape on the Accept question is "later",
    /// on the others the safe answer.
    fn answer_key(&mut self, named: Named) -> Task<Message> {
        if matches!(self.dialog, Some(Dialog::Pair(_) | Dialog::RelayMove(_))) {
            self.pair_key(named);
            return Task::none();
        }
        let enter = named == Named::Enter;
        let escape = named == Named::Escape;
        match &self.dialog {
            Some(Dialog::Accept(ask)) if enter => {
                let choice = ask.focus();
                self.answer(choice);
            }
            Some(Dialog::Accept(_)) if escape => self.later(),
            Some(Dialog::StopReceiving(ask)) if enter || escape => {
                let choice = if enter { ask.focus() } else { Choice::No };
                return self.on_receive(RecvMsg::AnswerStop(choice));
            }
            Some(Dialog::Leave(ask, _)) if enter || escape => {
                let choice = if enter { ask.focus() } else { Choice::No };
                return self.on_receive(RecvMsg::AnswerLeave(choice));
            }
            Some(Dialog::PortMapping(ask)) if enter || escape => {
                let choice = if enter { ask.focus() } else { Choice::No };
                return self.on_settings(SettingsMsg::AnswerPortMapping(choice));
            }
            Some(Dialog::ClearHistory(ask)) if enter || escape => {
                let choice = if enter { ask.focus() } else { Choice::No };
                return self.on_settings(SettingsMsg::AnswerClear(choice));
            }
            // A drop may be an accident: Enter and Escape both send nothing.
            Some(Dialog::SendDropped { .. }) if enter || escape => {
                return self.on_send(SendMsg::AnswerDrop(false));
            }
            Some(Dialog::Remove(ask)) if enter => {
                let answer = ask.focus();
                self.answer_remove(answer);
            }
            Some(Dialog::Remove(_)) if escape => self.answer_remove(Choice::No),
            Some(Dialog::Rename { .. }) if enter => self.rename(),
            Some(_) if escape => self.dialog = None,
            _ => {}
        }
        Task::none()
    }
}
