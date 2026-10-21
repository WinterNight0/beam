//! SPIKE-002 mock-up, "Aero Modern": Windows 7 glass and gloss on a modern
//! layout, to see how far iced's styling goes.
//!
//! Not linked to beam: every friend, request and transfer is made up. What it
//! shows off:
//!
//! * glossy gradients and coloured glows (`style.rs`, the "stylesheet");
//! * a background drawn by hand on a canvas (`wallpaper.rs`);
//! * its own glass title bar with a red close button, dragging and resizing
//!   (the window has no Windows frame);
//! * a dialog that fades and slides in (`iced::Animation`);
//! * tooltips, a live search, a toggle switch, an animated progress bar.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod style;
mod wallpaper;

use std::time::{Duration, Instant};

use iced::font::Weight;
use iced::widget::{
    button, canvas, center, column, container, mouse_area, opaque, progress_bar, row, scrollable,
    space, stack, text, text_input, toggler, tooltip,
};
use iced::{
    Alignment, Animation, Color, Element, Fill, Font, Padding, Subscription, Task, Theme, color,
    keyboard, window,
};

fn main() -> iced::Result {
    iced::application(Aero::new, Aero::update, Aero::view)
        .title("beam")
        .theme(|_: &Aero| style::theme())
        .subscription(Aero::subscription)
        .default_font(FONT)
        .decorations(false)
        .window_size((1120.0, 700.0))
        .run()
}

/// Segoe UI, as Windows 7 itself used; iced finds it among the system fonts.
const FONT: Font = Font::with_name("Segoe UI");
const BOLD: Font = Font {
    weight: Weight::Bold,
    ..FONT
};
const SEMIBOLD: Font = Font {
    weight: Weight::Semibold,
    ..FONT
};

struct Friend {
    name: &'static str,
    tint: Color,
    seen: &'static str,
    fingerprint: &'static str,
}

const FRIENDS: [Friend; 6] = [
    Friend {
        name: "Alice",
        tint: color!(0xe0479e),
        seen: "last seen 2 hours ago",
        fingerprint: "SHA256:82f1 4c7d 179a d27f",
    },
    Friend {
        name: "Bob",
        tint: color!(0x3fae2a),
        seen: "last seen yesterday",
        fingerprint: "SHA256:0b9e 77a1 c3d0 5e12",
    },
    Friend {
        name: "Charlie",
        tint: color!(0xf09a1a),
        seen: "last seen 3 days ago",
        fingerprint: "SHA256:f00d 1a2b 3c4d 5e6f",
    },
    Friend {
        name: "Dana",
        tint: color!(0x2f95e6),
        seen: "never sent anything",
        fingerprint: "SHA256:aa11 bb22 cc33 dd44",
    },
    Friend {
        name: "Eve",
        tint: color!(0xd8402a),
        seen: "last seen 2 weeks ago",
        fingerprint: "SHA256:9a8b 7c6d 5e4f 3a2b",
    },
    Friend {
        name: "Frank",
        tint: color!(0x8e5bd6),
        seen: "last seen just now",
        fingerprint: "SHA256:1234 5678 9abc def0",
    },
];

struct Request {
    from: usize,
    file: &'static str,
    size: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tab {
    Friends,
    Pending,
    History,
}

#[derive(Clone, Copy)]
enum Dialog {
    Accept(usize),
    AddInvite,
    AddCode,
}

struct Sending {
    to: usize,
    file: &'static str,
    progress: f32,
}

struct Aero {
    wallpaper: wallpaper::Wallpaper,
    tab: Tab,
    selected: Option<usize>,
    search: String,
    receiving: bool,
    requests: Vec<Request>,
    dialog: Option<Dialog>,
    /// The dialog's fade-in: false → true over 250 ms.
    fade: Animation<bool>,
    now: Instant,
    invite: String,
    code: String,
    sending: Option<Sending>,
    toast: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    Tab(Tab),
    Select(usize),
    Search(String),
    Receiving(bool),
    Review(usize),
    Accept,
    Decline,
    AddFriend,
    Invite(String),
    Code(String),
    Next,
    Cancel,
    Send(usize),
    StopSending,
    Tick,
    Frame(Instant),
    Key(keyboard::Key),
    DragWindow,
    ResizeWindow,
    Minimize,
    Maximize,
    Close,
}

impl Aero {
    fn new() -> Self {
        let mut a = Self {
            wallpaper: wallpaper::Wallpaper::default(),
            tab: Tab::Friends,
            selected: Some(0),
            search: String::new(),
            receiving: false,
            requests: vec![
                Request {
                    from: 0,
                    file: "project.zip",
                    size: "12.4 MB",
                },
                Request {
                    from: 2,
                    file: "holiday-photos.zip",
                    size: "1.3 GB",
                },
            ],
            dialog: None,
            fade: Animation::new(false).duration(Duration::from_millis(250)),
            now: Instant::now(),
            invite: String::new(),
            code: String::new(),
            sending: None,
            toast: None,
        };
        // For screenshots only: `MOCKUP_START=pending,accept` opens in that state.
        let start = std::env::var("MOCKUP_START").unwrap_or_default();
        for word in start.split(',') {
            match word {
                "pending" => a.tab = Tab::Pending,
                "history" => a.tab = Tab::History,
                "on" => a.receiving = true,
                "accept" => a.open(Dialog::Accept(0)),
                "invite" => a.open(Dialog::AddInvite),
                "code" => a.open(Dialog::AddCode),
                "sending" => {
                    a.sending = Some(Sending {
                        to: 1,
                        file: "report-final-v3.pdf",
                        progress: 62.0,
                    })
                }
                _ => {}
            }
        }
        a
    }

    fn open(&mut self, dialog: Dialog) {
        self.dialog = Some(dialog);
        self.now = Instant::now();
        self.fade = Animation::new(false)
            .duration(Duration::from_millis(250))
            .go(true, self.now);
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tab(tab) => self.tab = tab,
            Message::Select(i) => self.selected = Some(i),
            Message::Search(s) => self.search = s,
            Message::Receiving(on) => {
                self.receiving = on;
                self.toast = Some(if on {
                    "Receiving is on: friends can send to you while beam is open.".into()
                } else {
                    "Receiving is off.".into()
                });
            }
            Message::Review(i) => self.open(Dialog::Accept(i)),
            Message::Accept | Message::Decline => {
                if let Some(Dialog::Accept(i)) = self.dialog.take() {
                    let r = self.requests.remove(i);
                    let who = FRIENDS[r.from].name;
                    self.toast = Some(match message {
                        Message::Accept => format!("Receiving {} from {who}…", r.file),
                        _ => format!("Declined {} from {who}.", r.file),
                    });
                }
            }
            Message::AddFriend => {
                self.invite.clear();
                self.code.clear();
                self.open(Dialog::AddInvite);
            }
            Message::Invite(s) => self.invite = s,
            Message::Code(s) => {
                self.code = s.chars().filter(char::is_ascii_digit).take(6).collect()
            }
            Message::Next => match self.dialog {
                Some(Dialog::AddInvite) if !self.invite.trim().is_empty() => {
                    self.open(Dialog::AddCode)
                }
                Some(Dialog::AddCode) if self.code.len() == 6 => {
                    self.dialog = None;
                    self.toast =
                        Some("Paired (pretend). The fingerprint check would come next.".into());
                }
                _ => {}
            },
            Message::Cancel => self.dialog = None,
            Message::Send(i) => {
                self.sending = Some(Sending {
                    to: i,
                    file: "report-final-v3.pdf",
                    progress: 0.0,
                });
            }
            Message::StopSending => {
                self.sending = None;
                self.toast = Some("Send cancelled.".into());
            }
            Message::Tick => {
                if let Some(s) = &mut self.sending {
                    s.progress += 0.5;
                    if s.progress >= 100.0 {
                        self.toast = Some(format!("{} sent to {}.", s.file, FRIENDS[s.to].name));
                        self.sending = None;
                    }
                }
            }
            Message::Frame(now) => self.now = now,
            // Enter and Escape on the Accept pop-up decline, never accept.
            Message::Key(keyboard::Key::Named(
                keyboard::key::Named::Enter | keyboard::key::Named::Escape,
            )) => {
                return match self.dialog {
                    Some(Dialog::Accept(_)) => self.update(Message::Decline),
                    Some(_) => self.update(Message::Cancel),
                    None => Task::none(),
                };
            }
            Message::Key(_) => {}
            // The window has no Windows frame, so beam moves and sizes it.
            Message::DragWindow => return window::latest().and_then(window::drag),
            Message::ResizeWindow => {
                return window::latest()
                    .and_then(|id| window::drag_resize(id, window::Direction::SouthEast));
            }
            Message::Minimize => return window::latest().and_then(|id| window::minimize(id, true)),
            Message::Maximize => return window::latest().and_then(window::toggle_maximize),
            Message::Close => return window::latest().and_then(window::close),
        }
        Task::none()
    }

    // -----------------------------------------------------------------------
    // Layout
    // -----------------------------------------------------------------------

    fn view(&self) -> Element<'_, Message> {
        let body = column![
            self.title_bar(),
            row![self.sidebar(), self.main()]
                .spacing(12)
                .padding(Padding {
                    top: 0.0,
                    ..Padding::new(12.0)
                }),
        ];
        let grip = container(
            mouse_area(text("◢").size(14).color(style::white(0.4)))
                .on_press(Message::ResizeWindow)
                .interaction(iced::mouse::Interaction::ResizingDiagonallyDown),
        )
        .align_right(Fill)
        .align_bottom(Fill)
        .padding(2);

        let page = stack![canvas(&self.wallpaper).width(Fill).height(Fill), body, grip];

        let Some(dialog) = self.dialog else {
            return page.into();
        };
        let t = self.fade.interpolate(0.0, 1.0, self.now);
        let card = match dialog {
            Dialog::Accept(i) => self.accept_dialog(i),
            Dialog::AddInvite => self.invite_dialog(),
            Dialog::AddCode => self.code_dialog(),
        };
        // Slides up 24 px while the veil fades in.
        let card = container(card).padding(Padding {
            top: 24.0 * (1.0 - t),
            ..Padding::ZERO
        });
        stack![page, opaque(center(card).style(style::veil(t)))].into()
    }

    /// Win7's glass caption, drawn by beam: drag it to move the window.
    fn title_bar(&self) -> Element<'_, Message> {
        let caption = |label: &'static str,
                       message: Message,
                       style: fn(&Theme, button::Status) -> button::Style| {
            button(center(text(label).size(13)))
                .width(if matches!(message, Message::Close) {
                    46
                } else {
                    30
                })
                .height(22)
                .style(style)
                .on_press(message)
        };
        let bar = container(
            row![
                container(center(text("b").font(BOLD).size(13)))
                    .width(22)
                    .height(22)
                    .style(style::orb(style::SKY)),
                text("beam").font(SEMIBOLD).size(14),
                muted("  ·  my-laptop", 13),
                space::horizontal(),
                caption("—", Message::Minimize, style::caption),
                caption("☐", Message::Maximize, style::caption),
                caption("✕", Message::Close, style::close),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([8, 12])
        .style(style::title_bar);
        mouse_area(bar).on_press(Message::DragWindow).into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let needle = self.search.to_lowercase();
        let friends = column(
            FRIENDS
                .iter()
                .enumerate()
                .filter(|(_, f)| f.name.to_lowercase().contains(&needle))
                .map(|(i, f)| {
                    button(
                        row![
                            orb(f.name, f.tint, 34.0),
                            column![text(f.name).font(SEMIBOLD), muted(f.seen, 12)]
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    )
                    .width(Fill)
                    .padding([6, 8])
                    .style(style::item(self.selected == Some(i)))
                    .on_press(Message::Select(i))
                    .into()
                }),
        )
        .spacing(3);

        let me = container(
            row![
                orb("Y", color!(0x18b0e8), 34.0),
                column![
                    text("my-laptop").font(SEMIBOLD),
                    muted(
                        if self.receiving {
                            "Receiving"
                        } else {
                            "Not receiving"
                        },
                        12
                    )
                ],
                space::horizontal(),
                tooltip(
                    toggler(self.receiving)
                        .on_toggle(Message::Receiving)
                        .size(20)
                        .style(style::switch),
                    tip("Let friends send to you while beam is open"),
                    tooltip::Position::Top,
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([10, 12])
        .style(style::card);

        container(
            column![
                text_input("Find a friend", &self.search)
                    .on_input(Message::Search)
                    .padding([8, 10])
                    .style(style::input),
                muted("FRIENDS — 6", 12).font(BOLD),
                scrollable(friends).height(Fill),
                me,
            ]
            .spacing(12),
        )
        .padding(12)
        .width(270)
        .height(Fill)
        .style(style::panel)
        .into()
    }

    fn main(&self) -> Element<'_, Message> {
        let pending = self.requests.len();
        let mut pending_label = row![text("Pending")].spacing(6).align_y(Alignment::Center);
        if pending > 0 {
            pending_label = pending_label.push(
                container(text(pending).size(11).font(BOLD))
                    .padding([1, 7])
                    .style(style::badge),
            );
        }
        let header = row![
            text("Friends").font(BOLD).size(18),
            space::horizontal().width(8),
            tab_button(text("All").into(), Tab::Friends, self.tab),
            tab_button(pending_label.into(), Tab::Pending, self.tab),
            tab_button(text("History").into(), Tab::History, self.tab),
            space::horizontal(),
            button(text("Add friend").font(SEMIBOLD))
                .padding([7, 16])
                .style(style::leaf)
                .on_press(Message::AddFriend),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let body = match self.tab {
            Tab::Friends => self.friends_tab(),
            Tab::Pending => self.pending_tab(),
            Tab::History => self.history_tab(),
        };

        let mut content = column![header, container(body).height(Fill)].spacing(16);
        if let Some(s) = &self.sending {
            content = content.push(self.transfer_card(s));
        }
        if let Some(t) = &self.toast {
            content = content.push(muted(t, 13));
        }
        container(content)
            .padding(16)
            .width(Fill)
            .height(Fill)
            .style(style::panel)
            .into()
    }

    fn friends_tab(&self) -> Element<'_, Message> {
        let rows = FRIENDS.iter().enumerate().map(|(i, f)| {
            let send = button(text("Send a file").font(SEMIBOLD))
                .padding([6, 14])
                .style(style::sky)
                .on_press_maybe(self.sending.is_none().then_some(Message::Send(i)));
            let more = tooltip(
                button(text("•••")).padding([6, 10]).style(style::clear),
                tip("Rename, remove, fingerprint"),
                tooltip::Position::Left,
            );
            let card = container(
                row![
                    orb(f.name, f.tint, 42.0),
                    column![text(f.name).font(SEMIBOLD).size(16), muted(f.seen, 13)].spacing(2),
                    space::horizontal(),
                    send,
                    more,
                ]
                .spacing(14)
                .align_y(Alignment::Center),
            )
            .padding([10, 14])
            .style(style::card);
            mouse_area(card).on_press(Message::Select(i)).into()
        });
        column![
            muted("ALL FRIENDS — 6", 12).font(BOLD),
            scrollable(column(rows).spacing(8)).height(Fill)
        ]
        .spacing(12)
        .into()
    }

    fn pending_tab(&self) -> Element<'_, Message> {
        let on = self.receiving;
        let banner = container(
            row![
                column![
                    text(if on { "Receiving is ON" } else { "Receiving is OFF" }).font(BOLD).size(16),
                    muted(
                        if on {
                            "Friends can send to you while beam is open. You still accept every file."
                        } else {
                            "Turn it on to let friends send to you without opening a terminal."
                        },
                        13,
                    ),
                ]
                .spacing(2),
                space::horizontal(),
                toggler(on).on_toggle(Message::Receiving).size(24).style(style::switch),
            ]
            .align_y(Alignment::Center),
        )
        .padding([12, 16])
        .style(style::banner(on));

        let requests: Element<'_, Message> = if self.requests.is_empty() {
            center(muted("Nothing waiting. Requests show up here.", 14)).into()
        } else {
            column(self.requests.iter().enumerate().map(|(i, r)| {
                let f = &FRIENDS[r.from];
                container(
                    row![
                        orb(f.name, f.tint, 42.0),
                        column![
                            text(format!("{} wants to send you a file", f.name)).font(SEMIBOLD),
                            muted(format!("{}  ·  {}  ·  expires in 4:12", r.file, r.size), 13),
                        ]
                        .spacing(2),
                        space::horizontal(),
                        button(text("Review").font(SEMIBOLD))
                            .padding([6, 14])
                            .style(style::sky)
                            .on_press(Message::Review(i)),
                    ]
                    .spacing(14)
                    .align_y(Alignment::Center),
                )
                .padding([10, 14])
                .style(style::card)
                .into()
            }))
            .spacing(8)
            .into()
        };

        column![
            banner,
            muted(format!("WAITING — {}", self.requests.len()), 12).font(BOLD),
            requests
        ]
        .spacing(14)
        .into()
    }

    fn history_tab(&self) -> Element<'_, Message> {
        let entries = [
            (
                true,
                "Sent report.pdf to Alice",
                "2.1 MB  ·  accepted",
                "2 hours ago",
            ),
            (
                false,
                "Received slides.pptx from Bob",
                "8.7 MB  ·  accepted",
                "yesterday",
            ),
            (
                true,
                "Sent game.iso to Charlie",
                "4.2 GB  ·  declined",
                "3 days ago",
            ),
            (
                false,
                "Received notes.txt from Frank",
                "4 KB  ·  accepted",
                "just now",
            ),
        ];
        column(entries.into_iter().map(|(sent, what, detail, when)| {
            let (arrow, tint) = if sent {
                ("↑", style::SKY)
            } else {
                ("↓", style::LEAF)
            };
            container(
                row![
                    container(center(text(arrow).size(18).font(BOLD)))
                        .width(36)
                        .height(36)
                        .style(style::orb(tint)),
                    column![text(what).font(SEMIBOLD), muted(detail, 13)].spacing(2),
                    space::horizontal(),
                    muted(when, 13),
                ]
                .spacing(14)
                .align_y(Alignment::Center),
            )
            .padding([8, 14])
            .style(style::card)
            .into()
        }))
        .spacing(8)
        .into()
    }

    fn transfer_card<'a>(&self, s: &'a Sending) -> Element<'a, Message> {
        let f = &FRIENDS[s.to];
        container(
            column![
                row![
                    text(format!("Sending {} to {}", s.file, f.name)).font(SEMIBOLD),
                    space::horizontal(),
                    muted(
                        format!("{:.0}%  ·  [Direct P2P]  ·  14.2 MB/s", s.progress),
                        13
                    ),
                    button(text("Cancel"))
                        .padding([4, 12])
                        .style(style::ember)
                        .on_press(Message::StopSending),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                progress_bar(0.0..=100.0, s.progress)
                    .girth(14)
                    .style(style::progress),
            ]
            .spacing(10),
        )
        .padding([12, 16])
        .style(style::card)
        .into()
    }

    // -----------------------------------------------------------------------
    // Dialogs
    // -----------------------------------------------------------------------

    /// The Accept pop-up. Decline is the glossy blue default; Enter and Esc
    /// decline (the same safe default as the TUI).
    fn accept_dialog(&self, i: usize) -> Element<'_, Message> {
        let r = &self.requests[i];
        let f = &FRIENDS[r.from];
        let file =
            container(column![text(r.file).font(SEMIBOLD).size(16), muted(r.size, 13)].spacing(2))
                .padding([10, 14])
                .width(Fill)
                .style(style::card);
        dialog_box(
            "Incoming file",
            column![
                row![
                    orb(f.name, f.tint, 50.0),
                    column![
                        text(f.name).font(BOLD).size(20),
                        muted("wants to send you a file", 14)
                    ]
                ]
                .spacing(14)
                .align_y(Alignment::Center),
                file,
                column![
                    muted("Fingerprint", 12).font(BOLD),
                    text(f.fingerprint).font(Font::MONOSPACE).size(13)
                ]
                .spacing(4),
                muted(
                    "Only accept files you are expecting. Enter or Esc declines.",
                    13
                ),
                row![
                    space::horizontal(),
                    button(text("Accept"))
                        .padding([8, 20])
                        .style(style::clear)
                        .on_press(Message::Accept),
                    button(text("Decline").font(SEMIBOLD))
                        .padding([8, 20])
                        .style(style::sky)
                        .on_press(Message::Decline),
                ]
                .spacing(10),
            ]
            .spacing(16),
        )
    }

    fn invite_dialog(&self) -> Element<'_, Message> {
        dialog_box(
            "Add a friend",
            column![
                muted(
                    "Paste the invite your friend sent you. It starts with beam1.",
                    14
                ),
                text_input("beam1…", &self.invite)
                    .on_input(Message::Invite)
                    .on_submit(Message::Next)
                    .padding(10)
                    .style(style::input),
                row![
                    space::horizontal(),
                    button(text("Cancel"))
                        .padding([8, 20])
                        .style(style::clear)
                        .on_press(Message::Cancel),
                    button(text("Next").font(SEMIBOLD))
                        .padding([8, 20])
                        .style(style::sky)
                        .on_press_maybe((!self.invite.trim().is_empty()).then_some(Message::Next)),
                ]
                .spacing(10),
            ]
            .spacing(16),
        )
    }

    fn code_dialog(&self) -> Element<'_, Message> {
        dialog_box(
            "Enter the pairing code",
            column![
                muted("Your friend's beam shows a 6-digit code. Type it here.", 14),
                text_input("123456", &self.code)
                    .on_input(Message::Code)
                    .on_submit(Message::Next)
                    .padding(12)
                    .size(24)
                    .font(Font::MONOSPACE)
                    .style(style::input),
                row![
                    space::horizontal(),
                    button(text("Cancel"))
                        .padding([8, 20])
                        .style(style::clear)
                        .on_press(Message::Cancel),
                    button(text("Pair").font(SEMIBOLD))
                        .padding([8, 20])
                        .style(style::sky)
                        .on_press_maybe((self.code.len() == 6).then_some(Message::Next)),
                ]
                .spacing(10),
            ]
            .spacing(16),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![keyboard::listen().filter_map(|event| match event {
            keyboard::Event::KeyPressed { key, .. } => Some(Message::Key(key)),
            _ => None,
        })];
        if self.sending.is_some() {
            subs.push(iced::time::every(Duration::from_millis(80)).map(|_| Message::Tick));
        }
        // Redraw every frame only while the dialog is still fading in.
        if self.fade.is_animating(self.now) {
            subs.push(window::frames().map(Message::Frame));
        }
        Subscription::batch(subs)
    }
}

// ---------------------------------------------------------------------------
// Small reusable pieces
// ---------------------------------------------------------------------------

fn orb<'a>(name: &str, tint: Color, size: f32) -> Element<'a, Message> {
    let initial = name.chars().next().unwrap_or('?').to_string();
    container(center(text(initial).font(BOLD).size(size * 0.45)))
        .width(size)
        .height(size)
        .style(style::orb(tint))
        .into()
}

fn muted<'a>(s: impl text::IntoFragment<'a>, size: u32) -> text::Text<'a> {
    text(s).size(size).color(style::DIM)
}

fn tip(s: &str) -> Element<'_, Message> {
    container(text(s).size(12))
        .padding([5, 9])
        .style(style::card)
        .into()
}

fn tab_button<'a>(label: Element<'a, Message>, tab: Tab, current: Tab) -> Element<'a, Message> {
    button(label)
        .padding([5, 12])
        .style(style::tab(tab == current))
        .on_press(Message::Tab(tab))
        .into()
}

/// A dialog with a darker glass caption strip, like a Win7 dialog box.
fn dialog_box<'a>(
    title: &'a str,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(column![
        container(text(title).font(SEMIBOLD).size(15)).padding([10, 20]),
        container(content).padding(Padding {
            top: 6.0,
            ..Padding::new(22.0)
        }),
    ])
    .width(480)
    .style(style::dialog)
    .into()
}
