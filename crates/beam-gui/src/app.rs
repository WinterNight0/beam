//! The window's state and what changes it: iced's `Message` → `update`,
//! the shape `tui::app` has. Drawing is in `view.rs`.
//!
//! GUI EX Patch #2: the shell. Navigation rail, the friends list and a
//! friend's identity, this device, the status bar, Midnight/Daylight, and
//! the first-run "Create this device's identity". Patch #3: friends: groups,
//! recent activity, Copy, rename, and remove behind a question that starts
//! on Keep (`beam_model::Ask`). Patch #4: Add friend (`app/pairing.rs`).
//! Everything is read and changed through
//! `beam-model` and `beam-core`; the pages still to come say so.

use std::time::Duration;

use beam_core::actions::friends;
use beam_core::actions::identity::{Created, create};
use beam_core::config::{Config, Relay};
use beam_core::identity::Store;
use beam_core::transport::endpoint::Bind;
use beam_core::untrusted;
use beam_model::requests::Requests;
use beam_model::snapshot::Snapshot;
use beam_model::{
    Ask, Choice,
    ask::{Incoming, LeaveMidTransfer, RelayMove, RemoveFriend, StopMidTransfer},
};

pub(crate) mod pairing;
pub(crate) mod receiving;
pub(crate) mod sending;
pub(crate) mod settings;
use iced::{Element, Subscription, Task, keyboard, window};
pub use pairing::{PairMsg, Pairing};
pub use receiving::RecvMsg;
pub use sending::SendMsg;
pub use settings::{Filter as HistoryFilter, SettingsMsg};

use crate::theme::{self, Pal};

/// The pages on the navigation rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Friends,
    Transfers,
    Inbox,
    History,
    Settings,
}

/// The tone of a notice: what it is about, not how loud it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Problem,
}

#[derive(Debug)]
pub struct Gui {
    /// `None` when no beam home could be found at all.
    pub(crate) store: Option<Store>,
    pub(crate) snapshot: Option<Snapshot>,
    /// Relay and port, for the status bar. `None` if `config.toml` cannot
    /// be read; the problem is then shown instead.
    pub(crate) config: Option<Config>,
    /// Why there is no beam home or no config, already cleaned for drawing.
    pub(crate) problem: Option<String>,
    pub(crate) page: Page,
    /// The selected friend, by name: the key `known_peers` uses, so the
    /// selection survives a refresh that reorders nothing but adds a friend.
    pub(crate) selected: Option<String>,
    pub(crate) search: String,
    pub(crate) dark: bool,
    /// The window's width. Tiling compositors (Niri, Hyprland, Sway) size
    /// windows themselves and ignore the minimum size, so the layout adapts.
    pub(crate) width: f32,
    pub(crate) notice: Option<(String, Tone)>,
    /// The one question or form open over the window, if any.
    pub(crate) dialog: Option<Dialog>,
    /// A pairing in progress; dropping it cancels the pairing.
    pub(crate) worker: Option<pairing::Running>,
    /// Where a pairing or the Receiving switch listens: `Loopback` only in
    /// tests.
    pub(crate) bind: Bind,
    /// Requests waiting, the file arriving, the switch (`beam-model`).
    pub(crate) requests: Requests,
    /// The switch's receiver and the link to the receiver, while they run.
    pub(crate) receivers: receiving::Receivers,
    /// A desktop notification per request; off only in tests.
    pub(crate) notify: bool,
    /// The send in progress; dropping it cancels the send.
    pub(crate) outgoing: Option<sending::Running>,
    /// Where the send is, or how it ended (kept until dismissed).
    pub(crate) sending: Option<beam_model::sending::Sending>,
    /// Whether the background agent starts at sign-in; read when the
    /// Settings page opens and after a change (it asks the OS).
    pub(crate) at_sign_in: bool,
    pub(crate) history_filter: settings::Filter,
    pub(crate) history_search: String,
    /// The `beam` program next to this window, which the background
    /// service runs; `None` if it is not there.
    pub(crate) beam_program: Option<std::path::PathBuf>,
}

/// What can be open over the window. One at a time, like the view's pop-ups.
#[derive(Debug)]
pub enum Dialog {
    /// A new nickname for `from`. `error` is why the last try was refused,
    /// cleaned.
    Rename {
        from: String,
        to: String,
        error: Option<String>,
    },
    /// Remove this friend? An `Ask`: it starts on Keep, and Enter or Escape
    /// keep them. Only a click on Remove removes.
    Remove(Ask<RemoveFriend>),
    /// Add a friend: paste an invite or show mine, then compare
    /// fingerprints (`app/pairing.rs`).
    Pair(Pairing),
    /// An invite moves a paired friend to another relay: save it? Starts on
    /// Keep (ADR-0038).
    RelayMove(Ask<RelayMove>),
    /// May this friend send this file? Starts on Decline (`app/receiving.rs`).
    Accept(Ask<Incoming>),
    /// Stop receiving while a file arrives? Starts on No.
    StopReceiving(Ask<StopMidTransfer>),
    /// Close beam while a file arrives or goes out? Starts on No (stay).
    Leave(Ask<LeaveMidTransfer>, window::Id),
    /// A file was dropped on a friend's page: send it to them? Nothing
    /// starts without a click on Send (`app/sending.rs`).
    SendDropped {
        friend: String,
        path: std::path::PathBuf,
    },
    /// Turn router port mapping on? Starts on "Keep it off".
    PortMapping(Ask<settings::PortMappingOn>),
    /// Clear the whole history? Starts on Keep.
    ClearHistory(Ask<settings::ClearHistory>),
}

#[derive(Debug, Clone)]
pub enum Message {
    Page(Page),
    Select(String),
    Search(String),
    Dark(bool),
    /// First run: make this device's identity (`beam init`).
    CreateIdentity,
    /// Read `~/.beam` again: a terminal may have changed it.
    Refresh,
    Resized(f32),
    DismissNotice,
    /// Copy a friend's full fingerprint, to paste where they can compare it.
    CopyFingerprint(String),
    Rename(String),
    RenameText(String),
    ConfirmRename,
    Remove(String),
    /// The answer to the Remove question.
    AnswerRemove(Choice),
    CloseDialog,
    /// Open the Add friend dialog.
    AddFriend,
    Pair(PairMsg),
    Receive(RecvMsg),
    Send(SendMsg),
    Settings(SettingsMsg),
    /// Enter or Escape, for the open dialog.
    Key(keyboard::Key),
}

/// The rename form's text box, so it can be focused when the form opens.
pub(crate) const RENAME_INPUT: &str = "rename";

/// The window's size when it opens, and the smallest a stacking window
/// manager lets it get. A tiling one may go smaller; the layout copes.
pub const SIZE: (f32, f32) = (1200.0, 660.0);
pub const MIN_SIZE: (f32, f32) = (1000.0, 600.0);

impl Gui {
    /// The beam home the CLI would use: `$BEAM_DIR`, else `~/.beam`.
    pub fn boot() -> Self {
        match Store::default_dir() {
            Ok(dir) => Self::for_home(Store::new(dir)),
            Err(e) => Self {
                problem: Some(untrusted::text(&e.to_string())),
                ..Self::empty(None)
            },
        }
    }

    pub fn for_home(store: Store) -> Self {
        let mut gui = Self::empty(Some(store));
        gui.reload();
        gui.connect_link();
        gui
    }

    fn empty(store: Option<Store>) -> Self {
        Self {
            store,
            snapshot: None,
            config: None,
            problem: None,
            page: Page::Friends,
            selected: None,
            search: String::new(),
            dark: true,
            width: SIZE.0,
            notice: None,
            dialog: None,
            worker: None,
            bind: Bind::Any,
            requests: Requests::default(),
            receivers: receiving::Receivers::default(),
            notify: true,
            outgoing: None,
            sending: None,
            at_sign_in: false,
            history_filter: settings::Filter::All,
            history_search: String::new(),
            beam_program: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().and_then(settings::beam_in)),
        }
    }

    fn reload(&mut self) {
        let Some(store) = &self.store else { return };
        self.snapshot = Some(Snapshot::load(store));
        match Config::load(&store.config_path()) {
            Ok(config) => {
                self.config = Some(config);
                self.problem = None;
            }
            Err(e) => {
                self.config = None;
                self.problem = Some(untrusted::text(&e.to_string()));
            }
        }
        // A friend removed in a terminal is no longer selected.
        let still_there = |name: &String| {
            self.snapshot
                .as_ref()
                .is_some_and(|s| s.friends.iter().any(|f| &f.name == name))
        };
        if !self.selected.as_ref().is_some_and(still_there) {
            self.selected = self
                .snapshot
                .as_ref()
                .and_then(|s| s.friends.first())
                .map(|f| f.name.clone());
        }
    }

    pub fn pal(&self) -> &'static Pal {
        if self.dark {
            &theme::DARK
        } else {
            &theme::LIGHT
        }
    }

    pub fn theme(&self) -> iced::Theme {
        theme::theme(self.pal())
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Page(page) => {
                if page == Page::Settings {
                    self.at_sign_in = beam_core::actions::service::enabled();
                }
                self.page = page;
            }
            Message::Select(name) => {
                self.selected = Some(name);
                self.page = Page::Friends;
            }
            Message::Search(text) => self.search = text,
            Message::Dark(dark) => self.dark = dark,
            Message::CreateIdentity => {
                if let Some(store) = &self.store {
                    self.notice = Some(match create(store) {
                        Ok(Created::New { short_id }) => (
                            format!("This device's identity is ready. Short ID {short_id}."),
                            Tone::Success,
                        ),
                        Ok(Created::AlreadyThere) => (
                            "This device already has its identity; it was left alone.".into(),
                            Tone::Info,
                        ),
                        Err(e) => (format!("Could not create the identity: {e}"), Tone::Problem),
                    });
                    self.reload();
                }
            }
            Message::Refresh => {
                self.reload();
                self.connect_link();
            }
            Message::Resized(width) => self.width = width,
            Message::DismissNotice => self.notice = None,
            Message::CopyFingerprint(hex) => {
                self.notice = Some(("Fingerprint copied.".into(), Tone::Info));
                return iced::clipboard::write(format!("SHA256:{hex}"));
            }
            Message::Rename(name) => {
                self.dialog = Some(Dialog::Rename {
                    to: name.clone(),
                    from: name,
                    error: None,
                });
                // Ready to type over: focused, the old name selected.
                return Task::batch([
                    iced::widget::operation::focus(RENAME_INPUT),
                    iced::widget::operation::select_all(RENAME_INPUT),
                ]);
            }
            Message::RenameText(text) => {
                if let Some(Dialog::Rename { to, error, .. }) = &mut self.dialog {
                    *to = text;
                    *error = None;
                }
            }
            Message::ConfirmRename => self.rename(),
            Message::Remove(name) => {
                let fingerprint = self
                    .friend(&name)
                    .map(|f| f.fingerprint.clone())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::Remove(Ask::new(RemoveFriend { name, fingerprint })));
            }
            Message::AnswerRemove(answer) => self.answer_remove(answer),
            Message::CloseDialog => self.close_dialog(),
            Message::AddFriend => return self.add_friend(),
            Message::Pair(message) => return self.on_pair(message),
            Message::Receive(message) => return self.on_receive(message),
            Message::Send(message) => return self.on_send(message),
            Message::Settings(message) => return self.on_settings(message),
            Message::Key(key) => return self.on_key(key),
        }
        Task::none()
    }

    /// Enter and Escape on a dialog. On the Remove question both keep the
    /// friend: Enter answers the highlighted choice, which starts on Keep.
    fn on_key(&mut self, key: keyboard::Key) -> Task<Message> {
        use keyboard::key::Named;
        let keyboard::Key::Named(named) = key else {
            return Task::none();
        };
        if matches!(self.dialog, Some(Dialog::Pair(_) | Dialog::RelayMove(_))) {
            self.pair_key(named);
            return Task::none();
        }
        // Enter gives the highlighted answer, which starts on the safe one;
        // Escape on the Accept question is "later", on the others "no".
        let enter = named == keyboard::key::Named::Enter;
        let escape = named == keyboard::key::Named::Escape;
        match &self.dialog {
            Some(Dialog::Accept(ask)) if enter => {
                let choice = ask.focus();
                self.answer(choice);
                return Task::none();
            }
            Some(Dialog::Accept(_)) if escape => {
                self.later();
                return Task::none();
            }
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
            _ => {}
        }
        match (&self.dialog, named) {
            (Some(Dialog::Remove(ask)), Named::Enter) => {
                let answer = ask.focus();
                self.answer_remove(answer);
            }
            (Some(Dialog::Rename { .. }), Named::Enter) => self.rename(),
            (Some(_), Named::Escape) => self.dialog = None,
            _ => {}
        }
        Task::none()
    }

    fn rename(&mut self) {
        let Some(Dialog::Rename { from, to, .. }) = &self.dialog else {
            return;
        };
        let (from, to) = (from.clone(), to.trim().to_string());
        if to == from {
            self.dialog = None;
            return;
        }
        let Some(store) = &self.store else { return };
        match friends::rename(store, &from, &to) {
            Ok(()) => {
                self.dialog = None;
                if self.selected.as_deref() == Some(from.as_str()) {
                    self.selected = Some(to.clone());
                }
                self.notice = Some((format!("Renamed {from} to {to}."), Tone::Success));
                self.reload();
            }
            // Stays open with the reason, so the name can be fixed.
            Err(e) => {
                if let Some(Dialog::Rename { error, .. }) = &mut self.dialog {
                    *error = Some(untrusted::text(&e.to_string()));
                }
            }
        }
    }

    fn answer_remove(&mut self, answer: Choice) {
        let Some(Dialog::Remove(ask)) = self.dialog.take() else {
            return;
        };
        if answer == Choice::No {
            return;
        }
        let Some(store) = &self.store else { return };
        let name = ask.into_question().name;
        self.notice = Some(match friends::remove(store, &name) {
            Ok(()) => (
                format!("Removed {name}. Pair again to send to each other."),
                Tone::Success,
            ),
            Err(e) => (
                format!(
                    "Could not remove {name}: {}",
                    untrusted::text(&e.to_string())
                ),
                Tone::Problem,
            ),
        });
        self.reload();
    }

    pub(crate) fn friend(&self, name: &str) -> Option<&beam_model::snapshot::Friend> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.friends.iter().find(|f| f.name == name))
    }

    pub fn view(&self) -> Element<'_, Message> {
        crate::view::window(self)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let receiving = if self.receivers.switch.is_some() || self.receivers.link.is_some() {
            iced::time::every(Duration::from_millis(250)).map(|_| Message::Receive(RecvMsg::Tick))
        } else {
            Subscription::none()
        };
        let sending = if self.outgoing.is_some() {
            iced::time::every(Duration::from_millis(150)).map(|_| Message::Send(SendMsg::Tick))
        } else {
            Subscription::none()
        };
        let pairing = if self.worker.is_some() {
            iced::time::every(Duration::from_millis(100)).map(|_| Message::Pair(PairMsg::Tick))
        } else {
            Subscription::none()
        };
        Subscription::batch([
            pairing,
            receiving,
            sending,
            window::events().filter_map(|(_, event)| match event {
                window::Event::FileDropped(path) => Some(Message::Send(SendMsg::Dropped(path))),
                _ => None,
            }),
            window::close_requests().map(|id| Message::Receive(RecvMsg::CloseRequested(id))),
            window::resize_events().map(|(_, size)| Message::Resized(size.width)),
            keyboard::listen().filter_map(|event| match event {
                keyboard::Event::KeyPressed { key, .. } => Some(Message::Key(key)),
                _ => None,
            }),
            // Friends paired or removed in a terminal show up by themselves.
            iced::time::every(Duration::from_secs(2)).map(|_| Message::Refresh),
        ])
    }

    /// The friends list: narrower when the window is.
    pub(crate) fn pane_width(&self) -> f32 {
        if self.width < 1000.0 { 240.0 } else { 300.0 }
    }

    /// The relay, as the status bar shows it: its host, or "none".
    pub(crate) fn relay_label(&self) -> String {
        match self.config.as_ref().map(|c| &c.relay) {
            None => "unknown".into(),
            Some(Relay::Disabled) => "none (direct only)".into(),
            Some(relay) => {
                let url = relay.to_string();
                // iroh writes hosts fully qualified ("…iroh.link./").
                let host = url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .trim_end_matches('.');
                untrusted::text(host)
            }
        }
    }
}

#[cfg(test)]
mod tests;
