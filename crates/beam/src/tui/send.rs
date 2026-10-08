//! Sending from the view (ADR-0043, step 7): pick a file for a friend, watch
//! it go, hide it or cancel it.
//!
//! `s` on a friend (or `:send alice file` in the palette) opens a file box
//! with Tab completion; a file dragged onto the terminal pastes its path,
//! quotes and all, and the quotes are taken off. Sending then runs in
//! [`super::sending`], one file at a time. The pop-up that follows it can be
//! hidden (Esc): the header keeps a "⇡ 45 %" pill, and `s` brings it back.
//! Leaving beam mid-send asks first, starting on No.

use super::app::{App, Choice, Effect, Key, Modal, Target};
use super::input::Input;
use super::palette::MAX_LINE;
use super::sending::SendUpdate;
use crate::transfer::Progress;
use crate::transport::PathKind;

/// Where a send has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Starting,
    Dialling,
    /// Reading the file to hash it.
    Hashing {
        done: u64,
        total: u64,
    },
    /// The request is with them; a person has to say yes.
    Waiting,
    Moving {
        done: u64,
        total: u64,
        relay: bool,
    },
    /// They are checking the whole file.
    Checking {
        done: u64,
        total: u64,
    },
}

/// The one send the view runs at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sending {
    pub peer: String,
    pub file: String,
    pub stage: Stage,
    /// Set when it ended: what to tell the person.
    pub result: Option<Result<String, String>>,
}

impl Sending {
    pub fn running(&self) -> bool {
        self.result.is_none()
    }
}

/// How many paths the file box lists.
pub const FILE_ROWS: usize = 8;

impl App {
    /// `s`: the send pop-up if a send is running, else a file box for the
    /// selected friend.
    pub(super) fn open_send(&mut self) {
        if self.sending.as_ref().is_some_and(Sending::running) {
            self.modal = Some(Modal::SendStatus);
            return;
        }
        if let Some(friend) = self.selected_friend() {
            self.modal = Some(Modal::PickFile {
                peer: friend.name.clone(),
                input: Input::new(MAX_LINE),
                error: None,
                selected: 0,
            });
        }
    }

    /// Paths that complete what is typed in the file box.
    pub fn file_suggestions(&self) -> Vec<String> {
        match &self.modal {
            Some(Modal::PickFile { input, .. }) => (self.paths)(&unquote(input.text()), false),
            _ => Vec::new(),
        }
    }

    pub(super) fn on_pick_file_key(&mut self, modal: Modal, key: Key) -> Option<Effect> {
        let Modal::PickFile {
            peer,
            mut input,
            mut error,
            mut selected,
        } = modal
        else {
            return None;
        };
        let suggestions = (self.paths)(&unquote(input.text()), false);
        match key {
            Key::Esc => return None,
            Key::Enter => {
                let path = unquote(input.text());
                // Nothing typed, or a folder half-typed: take the highlighted
                // line first.
                let chosen = suggestions.get(selected).cloned();
                if let Some(chosen) =
                    chosen.filter(|c| path.is_empty() || *c != path && selected > 0)
                {
                    input.set_text(&chosen);
                    if !chosen.ends_with(['/', '\\']) {
                        return self.start_send(peer, chosen, input);
                    }
                    selected = 0;
                } else if path.is_empty() {
                    error =
                        Some("Type a file's path, or drag a file onto this window.".to_string());
                } else {
                    return self.start_send(peer, path, input);
                }
            }
            Key::Tab => {
                if let Some(chosen) = suggestions.get(selected) {
                    input.set_text(chosen);
                    selected = 0;
                    error = None;
                }
            }
            Key::Up => selected = selected.saturating_sub(1),
            Key::Down => selected = (selected + 1).min(suggestions.len().saturating_sub(1)),
            Key::Char(c) => {
                input.insert(c);
                selected = 0;
                error = None;
            }
            key => {
                if let Some(edit) = super::app::edit_for(key) {
                    input.edit(edit);
                    selected = 0;
                    error = None;
                }
            }
        }
        self.modal = Some(Modal::PickFile {
            peer,
            input,
            error,
            selected,
        });
        None
    }

    /// Asks for the send; the file box stays up until it starts, so a path
    /// that is not a file can be fixed.
    fn start_send(&mut self, peer: String, path: String, mut input: Input) -> Option<Effect> {
        input.set_text(&path);
        self.modal = Some(Modal::PickFile {
            peer: peer.clone(),
            input,
            error: None,
            selected: 0,
        });
        Some(Effect::StartSend { peer, path })
    }

    pub(super) fn on_pick_file_click(
        &mut self,
        modal: Modal,
        hit: Option<(ratatui::layout::Rect, Target)>,
        column: u16,
    ) -> Option<Effect> {
        match (modal, hit) {
            (
                Modal::PickFile {
                    peer,
                    mut input,
                    error,
                    selected,
                },
                Some((_, Target::Suggestion(index))),
            ) => {
                // A click picks, as Enter on that line would.
                let Some(chosen) = self.paths_for(&input).get(index).cloned() else {
                    self.modal = Some(Modal::PickFile {
                        peer,
                        input,
                        error,
                        selected,
                    });
                    return None;
                };
                input.set_text(&chosen);
                if chosen.ends_with(['/', '\\']) {
                    self.modal = Some(Modal::PickFile {
                        peer,
                        input,
                        error: None,
                        selected: 0,
                    });
                    return None;
                }
                self.start_send(peer, chosen, input)
            }
            (
                Modal::PickFile {
                    peer,
                    mut input,
                    error,
                    selected,
                },
                Some((area, Target::Input)),
            ) => {
                input.click(column.saturating_sub(area.x), area.width);
                self.modal = Some(Modal::PickFile {
                    peer,
                    input,
                    error,
                    selected,
                });
                None
            }
            (modal @ Modal::PickFile { .. }, Some((_, Target::Button(Choice::Yes)))) => {
                self.on_pick_file_key(modal, Key::Enter)
            }
            (Modal::PickFile { .. }, Some((_, Target::Button(Choice::No)))) => None,
            (modal, _) => {
                self.modal = Some(modal);
                None
            }
        }
    }

    fn paths_for(&self, input: &Input) -> Vec<String> {
        (self.paths)(&unquote(input.text()), false)
    }

    /// Keys in the send pop-up.
    pub(super) fn on_send_status_key(&mut self, key: Key) -> Option<Effect> {
        let running = self.sending.as_ref().is_some_and(Sending::running);
        match key {
            Key::Char('x') | Key::Delete if running => {
                self.modal = Some(Modal::SendStatus);
                Some(Effect::CancelSend)
            }
            Key::Esc | Key::Enter | Key::Char(' ') => {
                if !running {
                    self.sending = None;
                }
                None
            }
            _ => {
                self.modal = Some(Modal::SendStatus);
                None
            }
        }
    }

    pub(super) fn on_send_status_click(&mut self, hit: Option<Target>) -> Option<Effect> {
        match hit {
            Some(Target::Button(Choice::No)) => self.on_send_status_key(Key::Char('x')),
            Some(Target::Button(Choice::Yes)) => self.on_send_status_key(Key::Esc),
            _ => {
                self.modal = Some(Modal::SendStatus);
                None
            }
        }
    }

    /// The sending thread moved on.
    pub fn on_send(&mut self, update: SendUpdate) {
        let Some(sending) = self.sending.as_mut() else {
            return;
        };
        match update {
            SendUpdate::Dialling => sending.stage = Stage::Dialling,
            SendUpdate::Progress(progress) => {
                sending.stage = match progress {
                    Progress::Hashing { done, total } => Stage::Hashing { done, total },
                    Progress::AwaitingAccept => Stage::Waiting,
                    Progress::Accepted { path } => Stage::Moving {
                        done: 0,
                        total: 0,
                        relay: path == PathKind::Relay,
                    },
                    Progress::Transferring { done, total, path } => Stage::Moving {
                        done,
                        total,
                        relay: path == PathKind::Relay,
                    },
                    Progress::PeerVerifying { done, total } => Stage::Checking { done, total },
                    _ => sending.stage,
                };
            }
            SendUpdate::Finished(result) => {
                self.flash = Some(match &result {
                    Ok(message) | Err(message) => message.clone(),
                });
                if matches!(self.modal, Some(Modal::SendStatus)) {
                    sending.result = Some(result);
                } else {
                    // Hidden: the status bar says how it ended.
                    self.sending = None;
                }
            }
        }
    }

    /// Ctrl+Q, `q` or `:quit`: leaves, unless a file is still going out,
    /// which asks first.
    pub(super) fn request_quit(&mut self) {
        let arriving = self.switch == super::pending::Switch::On && self.receiving.is_some();
        if self.sending.as_ref().is_some_and(Sending::running) || arriving {
            self.palette = None;
            self.modal = Some(Modal::ConfirmQuit { focus: Choice::No });
        } else {
            self.quit = true;
        }
    }

    pub(super) fn on_confirm_quit_key(&mut self, focus: Choice, key: Key) {
        let focus = match key {
            Key::Esc => return,
            Key::Enter => {
                self.quit = focus == Choice::Yes;
                return;
            }
            Key::Left | Key::Right | Key::Tab | Key::BackTab => match focus {
                Choice::Yes => Choice::No,
                Choice::No => Choice::Yes,
            },
            _ => focus,
        };
        self.modal = Some(Modal::ConfirmQuit { focus });
    }
}

/// A path as typed or pasted: a file dragged onto the terminal arrives in
/// quotes, and sometimes with `& ` in front (PowerShell).
pub fn unquote(text: &str) -> String {
    let text = text.trim();
    let text = text.strip_prefix("& ").unwrap_or(text).trim();
    for quote in ['"', '\''] {
        if let Some(inner) = text.strip_prefix(quote).and_then(|t| t.strip_suffix(quote)) {
            return inner.to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{Agent, Event, Friend, Snapshot};
    use crate::tui::pending::Owner;

    fn app() -> App {
        let mut app = App::new(Snapshot {
            me: None,
            friends: vec![Friend {
                name: "alice".into(),
                short_id: "1".into(),
                fingerprint: "aa".into(),
                added: "x".into(),
                last_seen: None,
            }],
            agent: Agent::Stopped,
            history: Vec::new(),
            owner: Owner::Background,
            listen_elsewhere: false,
            problem: None,
        });
        app.paths = |partial, _| {
            ["notes/", "report.pdf"]
                .iter()
                .filter(|p| p.starts_with(partial))
                .map(|p| p.to_string())
                .collect()
        };
        app
    }

    #[test]
    fn a_dragged_path_loses_its_quotes() {
        assert_eq!(
            unquote("\"C:\\My Files\\a b.txt\""),
            "C:\\My Files\\a b.txt"
        );
        assert_eq!(unquote("& 'D:\\x.zip' "), "D:\\x.zip");
        assert_eq!(unquote("plain.txt"), "plain.txt");
    }

    #[test]
    fn s_opens_a_file_box_and_enter_sends_what_is_typed() {
        let mut app = app();
        app.on_key(Key::Char('s'));
        app.on_event(Event::Paste("\"report.pdf\"".into()));
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::StartSend {
                peer: "alice".into(),
                path: "report.pdf".into()
            })
        );
    }

    #[test]
    fn tab_completes_and_a_folder_waits_for_more() {
        let mut app = app();
        app.on_key(Key::Char('s'));
        app.on_key(Key::Char('n'));
        app.on_key(Key::Tab);
        match &app.modal {
            Some(Modal::PickFile { input, .. }) => assert_eq!(input.text(), "notes/"),
            other => panic!("{other:?}"),
        }
        // Enter on an empty box takes the first line, a folder: no send yet.
        let mut app = app_with_box();
        assert_eq!(app.on_key(Key::Enter), None);
        app.on_key(Key::Down);
        assert!(matches!(
            app.on_key(Key::Enter),
            Some(Effect::StartSend { .. })
        ));
    }

    fn app_with_box() -> App {
        let mut app = app();
        app.on_key(Key::Char('s'));
        app
    }

    #[test]
    fn progress_moves_through_the_stages_and_hiding_keeps_it_going() {
        let mut app = app();
        app.sending = Some(Sending {
            peer: "alice".into(),
            file: "report.pdf".into(),
            stage: Stage::Starting,
            result: None,
        });
        app.modal = Some(Modal::SendStatus);
        app.on_send(SendUpdate::Progress(Progress::AwaitingAccept));
        assert_eq!(app.sending.as_ref().unwrap().stage, Stage::Waiting);
        app.on_send(SendUpdate::Progress(Progress::Transferring {
            done: 5,
            total: 10,
            path: PathKind::Relay,
        }));
        assert_eq!(
            app.sending.as_ref().unwrap().stage,
            Stage::Moving {
                done: 5,
                total: 10,
                relay: true
            }
        );
        app.on_key(Key::Esc);
        assert!(app.modal.is_none());
        assert!(app.sending.is_some(), "hidden, still going");
        app.on_key(Key::Char('s'));
        assert_eq!(app.modal, Some(Modal::SendStatus), "s brings it back");
        assert_eq!(app.on_key(Key::Char('x')), Some(Effect::CancelSend));
    }

    #[test]
    fn a_finished_send_says_so_and_closes() {
        let mut app = app();
        app.sending = Some(Sending {
            peer: "alice".into(),
            file: "report.pdf".into(),
            stage: Stage::Waiting,
            result: None,
        });
        app.on_send(SendUpdate::Finished(Ok("Sent 10 B to alice".into())));
        assert!(app.sending.is_none(), "hidden: only the status bar");
        assert_eq!(app.flash.as_deref(), Some("Sent 10 B to alice"));
    }

    #[test]
    fn leaving_mid_send_asks_first_and_no_is_the_default() {
        let mut app = app();
        app.sending = Some(Sending {
            peer: "alice".into(),
            file: "report.pdf".into(),
            stage: Stage::Waiting,
            result: None,
        });
        app.on_key(Key::Quit);
        assert!(!app.quit);
        assert_eq!(app.modal, Some(Modal::ConfirmQuit { focus: Choice::No }));
        app.on_key(Key::Enter);
        assert!(!app.quit, "Enter alone stays");
        app.on_key(Key::Quit);
        app.on_key(Key::Left);
        app.on_key(Key::Enter);
        assert!(app.quit);
    }
}
