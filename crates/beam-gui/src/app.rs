//! The window's state and what changes it: iced's `Message` → `update`,
//! the shape `tui::app` has. Drawing is in `view.rs`.
//!
//! GUI EX Patch #2: the shell. Navigation rail, the friends list and a
//! friend's identity, this device, the status bar, Midnight/Daylight, and
//! the first-run "Create this device's identity". Everything is read through
//! `beam-model` and `beam-core`; the pages still to come say so.

use std::time::Duration;

use beam_core::actions::identity::{Created, create};
use beam_core::config::{Config, Relay};
use beam_core::identity::Store;
use beam_core::untrusted;
use beam_model::snapshot::Snapshot;
use iced::{Element, Subscription, Task, window};

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
}

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
            Message::Page(page) => self.page = page,
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
            Message::Refresh => self.reload(),
            Message::Resized(width) => self.width = width,
            Message::DismissNotice => self.notice = None,
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        crate::view::window(self)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            window::resize_events().map(|(_, size)| Message::Resized(size.width)),
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
