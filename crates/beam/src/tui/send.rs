//! Sending from the view (ADR-0043, step 7): pick a file for a friend, watch
//! it go, hide it or cancel it.
//!
//! `s` on a friend opens the file browser ([`super::browse`], ADR-0045);
//! `:send alice file` in the palette skips it. A file dragged onto the
//! terminal pastes its path, quotes and all, and the quotes are taken off.
//! Sending then runs in
//! [`super::sending`], one file at a time. The pop-up that follows it can be
//! hidden (Esc): the header keeps a "⇡ 45 %" pill, and `s` brings it back.
//! Leaving beam mid-send asks first, starting on No.

use super::app::{App, Choice, Effect, Key, Modal, Target};
use super::browse::{Browser, Chosen, Pane, Row};
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

impl App {
    /// `s`: the send pop-up if a send is running, else the file browser for
    /// the selected friend, where it was last left.
    pub(super) fn open_send(&mut self) {
        if self.sending.as_ref().is_some_and(Sending::running) {
            self.modal = Some(Modal::SendStatus);
            return;
        }
        if let Some(friend) = self.selected_friend() {
            let start = self.last_dir.clone().unwrap_or_else(self.fs.start);
            let browser = Browser::open(&self.fs, friend.name.clone(), start);
            self.modal = Some(Modal::Browse(Box::new(browser)));
        }
    }

    /// Keys in the browser. It was taken out of `self`; putting it back
    /// keeps it open.
    pub(super) fn on_browse_key(&mut self, mut b: Box<Browser>, key: Key) -> Option<Effect> {
        let fs = self.fs;
        let typing = !b.filter.text().is_empty();
        match (b.pane, key) {
            (_, Key::Esc) => {
                self.last_dir = Some(b.dir.clone());
                return None;
            }
            (_, Key::Tab | Key::BackTab) => {
                b.pane = match b.pane {
                    Pane::Places => Pane::Files,
                    Pane::Files => Pane::Places,
                };
            }
            (_, Key::Up) => b.move_by(-1),
            (_, Key::Down) => b.move_by(1),
            (_, Key::PageUp) => b.move_by(-10),
            (_, Key::PageDown) => b.move_by(10),
            (Pane::Places, Key::Enter | Key::Right) => b.enter_place(&fs),
            (Pane::Places, Key::Home) => b.place_selected = 0,
            (Pane::Places, Key::End) => b.place_selected = b.places.len().saturating_sub(1),
            (Pane::Places, Key::Char(c)) => {
                // Typing goes to the folder's filter.
                b.pane = Pane::Files;
                b.filter.insert(c);
                b.filtered();
            }
            (Pane::Files, Key::Enter) => {
                if let Chosen::File(path) = b.enter(&fs) {
                    return self.start_send(b, path);
                }
            }
            (Pane::Files, Key::Backspace | Key::Left) if !typing => b.up(&fs),
            (Pane::Files, Key::Right) if !typing => {
                if matches!(b.rows().get(b.selected), Some(Row::Entry(i)) if b.entries[*i].is_dir)
                    && let Chosen::File(path) = b.enter(&fs)
                {
                    return self.start_send(b, path);
                }
            }
            (Pane::Files, Key::Home) if !typing => b.selected = 0,
            (Pane::Files, Key::End) if !typing => {
                b.selected = b.rows().len().saturating_sub(1);
            }
            (Pane::Files, Key::Char(c)) => {
                b.filter.insert(c);
                b.filtered();
            }
            (Pane::Files, key) => {
                if let Some(edit) = super::app::edit_for(key) {
                    b.filter.edit(edit);
                    b.filtered();
                }
            }
            _ => {}
        }
        self.modal = Some(Modal::Browse(b));
        None
    }

    /// A click in the browser: a place jumps there; a line is picked, and a
    /// second click on it opens it (or sends it).
    pub(super) fn on_browse_click(
        &mut self,
        mut b: Box<Browser>,
        hit: Option<Target>,
    ) -> Option<Effect> {
        let fs = self.fs;
        match hit {
            Some(Target::Place(index)) => {
                b.place_selected = index;
                b.enter_place(&fs);
            }
            Some(Target::Suggestion(index)) => {
                if b.pane == Pane::Files && b.selected == index {
                    if let Chosen::File(path) = b.enter(&fs) {
                        return self.start_send(b, path);
                    }
                } else {
                    b.pane = Pane::Files;
                    b.selected = index;
                }
            }
            Some(Target::Button(Choice::Yes)) => return self.on_browse_key(b, Key::Enter),
            Some(Target::Button(Choice::No)) => {
                self.last_dir = Some(b.dir.clone());
                return None;
            }
            _ => {}
        }
        self.modal = Some(Modal::Browse(b));
        None
    }

    /// Scrolling over the browser moves through the side it is over.
    pub(super) fn on_browse_wheel(&mut self, up: bool) {
        if let Some(Modal::Browse(b)) = &mut self.modal {
            b.move_by(if up { -3 } else { 3 });
        }
    }

    /// Asks for the send; the browser stays up until it starts, so a problem
    /// can be shown there.
    fn start_send(&mut self, b: Box<Browser>, path: std::path::PathBuf) -> Option<Effect> {
        let peer = b.peer.clone();
        self.last_dir = Some(b.dir.clone());
        self.modal = Some(Modal::Browse(b));
        Some(Effect::StartSend {
            peer,
            path: path.display().to_string(),
        })
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
            needs_setup: false,
            problem: None,
        });
        app.fs = crate::tui::browse::tests::fake();
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
    fn s_opens_the_browser_and_enter_goes_in_then_sends() {
        let mut app = app();
        app.on_key(Key::Char('s'));
        assert!(matches!(app.modal, Some(Modal::Browse(_))));
        assert_eq!(app.on_key(Key::Enter), None, "docs opens");
        let Some(Effect::StartSend { peer, path }) = app.on_key(Key::Enter) else {
            panic!("a.txt is sent")
        };
        assert_eq!(peer, "alice");
        assert_eq!(
            std::path::PathBuf::from(path),
            std::path::PathBuf::from("/home/me/docs").join("a.txt")
        );
    }

    #[test]
    fn typing_filters_backspace_goes_up_and_a_dragged_path_is_sent() {
        let mut app = app();
        app.on_key(Key::Char('s'));
        app.on_key(Key::Enter); // into docs
        app.on_key(Key::Backspace);
        let Some(Modal::Browse(b)) = &app.modal else {
            panic!()
        };
        assert_eq!(b.dir, std::path::PathBuf::from("/home/me"));

        for c in "note".chars() {
            app.on_key(Key::Char(c));
        }
        assert!(
            matches!(app.on_key(Key::Enter), Some(Effect::StartSend { path, .. }) if path.ends_with("notes.txt"))
        );

        // A dragged-in path that is not there says so.
        let mut app = app_with_browser();
        app.on_event(Event::Paste("\"/nowhere/x.zip\"".into()));
        assert_eq!(app.on_key(Key::Enter), None);
        let Some(Modal::Browse(b)) = &app.modal else {
            panic!()
        };
        assert!(b.error.as_deref().unwrap().contains("Nothing at"));
    }

    #[test]
    fn esc_closes_and_the_browser_reopens_where_it_was_left() {
        let mut app = app();
        app.on_key(Key::Char('s'));
        app.on_key(Key::Enter); // into docs
        app.on_key(Key::Esc);
        assert!(app.modal.is_none());
        app.on_key(Key::Char('s'));
        let Some(Modal::Browse(b)) = &app.modal else {
            panic!()
        };
        assert_eq!(b.dir, std::path::PathBuf::from("/home/me/docs"));
    }

    #[test]
    fn tab_goes_to_the_places_and_enter_jumps() {
        let mut app = app();
        app.on_key(Key::Char('s'));
        app.on_key(Key::Tab);
        app.on_key(Key::Down);
        app.on_key(Key::Enter);
        let Some(Modal::Browse(b)) = &app.modal else {
            panic!()
        };
        assert_eq!(b.dir, std::path::PathBuf::from("/"));
        assert_eq!(b.pane, Pane::Files);
    }

    fn app_with_browser() -> App {
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
