//! SPIKE-002: is iced a fit for beam's GUI?
//!
//! Throwaway. A Discord-Friends-style window with the real friends from
//! `~/.beam` (or `$BEAM_DIR`), an Accept pop-up that starts on Decline, and a
//! "Send…" button that opens the OS file picker. Nothing is sent: the question
//! is what building the GUI in iced costs, not whether the network works.

// No console window behind the GUI on Windows (release builds only, so a
// debug build still shows panics).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use beam_core::identity::Store;
use iced::widget::{button, center, column, container, opaque, row, scrollable, stack, text};
use iced::{Element, Fill, Subscription, Task, Theme, keyboard};

fn main() -> iced::Result {
    iced::application(Gui::boot, Gui::update, Gui::view)
        .title("beam")
        .theme(Gui::theme)
        .subscription(Gui::subscription)
        .window_size((900.0, 560.0))
        .run()
}

#[derive(Debug, Clone)]
pub struct Friend {
    pub name: String,
    pub fingerprint: String,
}

/// A transfer request waiting for an answer, as `transfer::Prompt` would
/// hand it over.
#[derive(Debug, Clone)]
pub struct Request {
    pub from: String,
    pub fingerprint: String,
    pub file: String,
    pub size: String,
}

#[derive(Debug, Default)]
pub struct Gui {
    pub friends: Vec<Friend>,
    pub selected: Option<usize>,
    pub request: Option<Request>,
    pub picked: Option<PathBuf>,
    pub status: String,
    pub dark: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(usize),
    SimulateRequest,
    Accept,
    Decline,
    PickFile,
    Picked(Option<PathBuf>),
    ToggleTheme,
    /// Enter or Escape while the pop-up is open.
    Key(keyboard::Key),
}

impl Gui {
    fn boot() -> Self {
        let friends = Store::default_dir()
            .ok()
            .map(Store::new)
            .and_then(|store| store.load_known_peers().ok())
            .map(|peers| {
                peers
                    .peers()
                    .into_iter()
                    .map(|p| Friend {
                        name: p.name.clone(),
                        fingerprint: p.fingerprint().short(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self::with_friends(friends)
    }

    pub fn with_friends(friends: Vec<Friend>) -> Self {
        Self {
            selected: (!friends.is_empty()).then_some(0),
            friends,
            dark: true,
            ..Self::default()
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Select(i) => self.selected = Some(i),
            Message::SimulateRequest => {
                let from = self.selected.and_then(|i| self.friends.get(i));
                self.request = Some(Request {
                    from: from.map_or("alice".into(), |f| f.name.clone()),
                    fingerprint: from.map_or("SHA256:ab12…".into(), |f| f.fingerprint.clone()),
                    file: "project.zip".into(),
                    size: "12.4 MB".into(),
                });
            }
            Message::Accept => {
                if let Some(r) = self.request.take() {
                    self.status = format!("Accepted {} from {}", r.file, r.from);
                }
            }
            // The safe answer: what Enter and Escape give, as in the TUI.
            Message::Decline => {
                if let Some(r) = self.request.take() {
                    self.status = format!("Declined {} from {}", r.file, r.from);
                }
            }
            Message::Key(key) => {
                use keyboard::key::Named;
                if self.request.is_some()
                    && matches!(key, keyboard::Key::Named(Named::Enter | Named::Escape))
                {
                    return self.update(Message::Decline);
                }
            }
            Message::PickFile => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Choose a file to send")
                            .pick_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    Message::Picked,
                );
            }
            Message::Picked(path) => {
                if let Some(p) = &path {
                    self.status = format!("Would send {}", p.display());
                }
                self.picked = path;
            }
            Message::ToggleTheme => self.dark = !self.dark,
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let friends = column(self.friends.iter().enumerate().map(|(i, f)| {
            let style = if self.selected == Some(i) {
                button::primary
            } else {
                button::text
            };
            button(text(&f.name))
                .width(Fill)
                .style(style)
                .on_press(Message::Select(i))
                .into()
        }))
        .spacing(4);
        let sidebar = column![
            text("Friends").size(20),
            if self.friends.is_empty() {
                Element::from(text("No friends yet.").style(text::secondary))
            } else {
                scrollable(friends).into()
            },
        ]
        .spacing(12)
        .padding(16)
        .width(220);

        let detail: Element<'_, Message> = match self.selected.and_then(|i| self.friends.get(i)) {
            Some(f) => column![
                text(&f.name).size(28),
                text(format!("Fingerprint {}", f.fingerprint)).style(text::secondary),
                row![
                    button("Send…")
                        .style(button::primary)
                        .on_press(Message::PickFile),
                    button("Simulate a request")
                        .style(button::secondary)
                        .on_press(Message::SimulateRequest),
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into(),
            None => column![
                text("Welcome to beam").size(28),
                button("Simulate a request").on_press(Message::SimulateRequest),
            ]
            .spacing(12)
            .into(),
        };
        let main = column![
            row![
                container(detail).width(Fill),
                button(if self.dark { "Light" } else { "Dark" })
                    .style(button::text)
                    .on_press(Message::ToggleTheme),
            ],
            container(text(&self.status).style(text::secondary))
                .height(Fill)
                .align_bottom(Fill),
        ]
        .padding(24);

        let page = row![
            container(sidebar)
                .style(container::bordered_box)
                .height(Fill),
            main
        ];

        match &self.request {
            None => page.into(),
            Some(r) => stack![page, opaque(center(Self::accept_card(r)).style(dim))].into(),
        }
    }

    /// The Accept pop-up. Decline is the highlighted, default button; Accept
    /// needs a deliberate click (CLAUDE.md rule 1, ADR-0043 safe defaults).
    fn accept_card(r: &Request) -> Element<'_, Message> {
        container(
            column![
                text(format!("{} wants to send you a file", r.from)).size(20),
                text(format!("{}  ({})", r.file, r.size)),
                text(format!("Fingerprint {}", r.fingerprint)).style(text::secondary),
                row![
                    button("Decline")
                        .style(button::primary)
                        .on_press(Message::Decline),
                    button("Accept")
                        .style(button::secondary)
                        .on_press(Message::Accept),
                ]
                .spacing(8),
            ]
            .spacing(10),
        )
        .padding(24)
        .width(420)
        .style(container::rounded_box)
        .into()
    }

    fn theme(&self) -> Theme {
        if self.dark { Theme::Dark } else { Theme::Light }
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().filter_map(|event| match event {
            keyboard::Event::KeyPressed { key, .. } => Some(Message::Key(key)),
            _ => None,
        })
    }
}

fn dim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(
            iced::Color {
                a: 0.6,
                ..iced::Color::BLACK
            }
            .into(),
        ),
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_test::simulator;

    fn alice() -> Gui {
        Gui::with_friends(vec![
            Friend {
                name: "alice".into(),
                fingerprint: "SHA256:aaaa".into(),
            },
            Friend {
                name: "bob".into(),
                fingerprint: "SHA256:bbbb".into(),
            },
        ])
    }

    fn click(gui: &mut Gui, label: &str) {
        let messages: Vec<Message> = {
            let mut ui = simulator(gui.view());
            ui.click(label).expect(label);
            ui.into_messages().collect()
        };
        for m in messages {
            let _ = gui.update(m);
        }
    }

    #[test]
    fn friends_are_listed_and_clicking_one_selects_it() {
        let mut gui = alice();
        click(&mut gui, "bob");
        assert_eq!(gui.selected, Some(1));
        let mut ui = simulator(gui.view());
        assert!(ui.find("Fingerprint SHA256:bbbb").is_ok());
    }

    #[test]
    fn the_accept_popup_shows_who_what_and_the_fingerprint() {
        let mut gui = alice();
        click(&mut gui, "Simulate a request");
        let mut ui = simulator(gui.view());
        assert!(ui.find("alice wants to send you a file").is_ok());
        assert!(ui.find("project.zip  (12.4 MB)").is_ok());
        assert!(ui.find("Fingerprint SHA256:aaaa").is_ok());
    }

    #[test]
    fn enter_and_escape_decline_never_accept() {
        use keyboard::key::Named;
        for key in [Named::Enter, Named::Escape] {
            let mut gui = alice();
            let _ = gui.update(Message::SimulateRequest);
            let _ = gui.update(Message::Key(keyboard::Key::Named(key)));
            assert!(gui.request.is_none());
            assert!(
                gui.status.starts_with("Declined"),
                "{key:?}: {}",
                gui.status
            );
        }
    }

    #[test]
    fn accept_takes_a_click_on_accept() {
        let mut gui = alice();
        let _ = gui.update(Message::SimulateRequest);
        click(&mut gui, "Accept");
        assert!(gui.status.starts_with("Accepted"));
    }

    #[test]
    fn with_no_friends_it_says_so() {
        let gui = Gui::with_friends(vec![]);
        let mut ui = simulator(gui.view());
        assert!(ui.find("No friends yet.").is_ok());
    }
}
