//! SPIKE-002 mock-up, "Riced": a Linux-ricer desktop look (Hyprland tiles
//! with gaps and a glowing gradient border on the focused one, a waybar
//! strip of pills, pastel schemes, monospace type, a neofetch card) blended
//! with Aero's soft gloss.
//!
//! Not linked to beam: every friend, request and transfer is made up.
//! Keys: Ctrl+1..4 switch workspace, Ctrl+T cycles the colour scheme, Enter
//! or Esc declines a request.

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
    Alignment, Animation, Color, Element, Fill, Font, Padding, Subscription, Task, keyboard, window,
};
use style::{PALETTES, Pal};

fn main() -> iced::Result {
    iced::application(Rice::new, Rice::update, Rice::view)
        .title("beam")
        .theme(|r: &Rice| style::theme(r.pal()))
        .subscription(Rice::subscription)
        .default_font(MONO)
        .decorations(false)
        .window_size((1140.0, 720.0))
        .run()
}

/// The ricer's font. Installed on this machine; a real build would bundle it
/// (it is OFL-licensed) with `.font(include_bytes!(...))`.
const MONO: Font = Font::with_name("JetBrains Mono");
const BOLD: Font = Font {
    weight: Weight::Bold,
    ..MONO
};
const GAP: f32 = 10.0;

struct Friend {
    name: &'static str,
    seen: &'static str,
    fingerprint: &'static str,
}

const FRIENDS: [Friend; 6] = [
    Friend {
        name: "alice",
        seen: "2h ago",
        fingerprint: "SHA256:82f1 4c7d 179a d27f",
    },
    Friend {
        name: "bob",
        seen: "yesterday",
        fingerprint: "SHA256:0b9e 77a1 c3d0 5e12",
    },
    Friend {
        name: "charlie",
        seen: "3d ago",
        fingerprint: "SHA256:f00d 1a2b 3c4d 5e6f",
    },
    Friend {
        name: "dana",
        seen: "never",
        fingerprint: "SHA256:aa11 bb22 cc33 dd44",
    },
    Friend {
        name: "eve",
        seen: "2w ago",
        fingerprint: "SHA256:9a8b 7c6d 5e4f 3a2b",
    },
    Friend {
        name: "frank",
        seen: "just now",
        fingerprint: "SHA256:1234 5678 9abc def0",
    },
];

struct Request {
    from: usize,
    file: &'static str,
    size: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Space {
    Friends,
    Pending,
    History,
    Whoami,
}

const SPACES: [(Space, &str); 4] = [
    (Space::Friends, "friends"),
    (Space::Pending, "pending"),
    (Space::History, "history"),
    (Space::Whoami, "whoami"),
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    Side,
    Main,
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

struct Rice {
    scheme: usize,
    wallpaper: wallpaper::Wallpaper,
    space: Space,
    focus: Focus,
    selected: usize,
    search: String,
    receiving: bool,
    requests: Vec<Request>,
    dialog: Option<Dialog>,
    fade: Animation<bool>,
    now: Instant,
    invite: String,
    code: String,
    sending: Option<Sending>,
    toast: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    Space(Space),
    Focus(Focus),
    Scheme(usize),
    NextScheme,
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
    Key(keyboard::Key, keyboard::Modifiers),
    Drag,
    Resize,
    Minimize,
    Maximize,
    Close,
}

impl Rice {
    fn new() -> Self {
        let mut r = Self {
            scheme: 0,
            wallpaper: wallpaper::Wallpaper::new(&PALETTES[0]),
            space: Space::Friends,
            focus: Focus::Main,
            selected: 0,
            search: String::new(),
            receiving: false,
            requests: vec![
                Request {
                    from: 0,
                    file: "project.zip",
                    size: "12.4M",
                },
                Request {
                    from: 2,
                    file: "holiday-photos.zip",
                    size: "1.3G",
                },
            ],
            dialog: None,
            fade: Animation::new(false),
            now: Instant::now(),
            invite: String::new(),
            code: String::new(),
            sending: None,
            toast: None,
        };
        // For screenshots only: `MOCKUP_START=pending,accept,gruvbox`.
        let start = std::env::var("MOCKUP_START").unwrap_or_default();
        for word in start.split(',') {
            match word {
                "pending" => r.space = Space::Pending,
                "history" => r.space = Space::History,
                "whoami" => r.space = Space::Whoami,
                "on" => r.receiving = true,
                "accept" => r.open(Dialog::Accept(0)),
                "invite" => r.open(Dialog::AddInvite),
                "sending" => {
                    r.sending = Some(Sending {
                        to: 1,
                        file: "report-final-v3.pdf",
                        progress: 62.0,
                    })
                }
                "side" => r.focus = Focus::Side,
                name => {
                    if let Some(i) = PALETTES.iter().position(|p| p.name == name) {
                        r.set_scheme(i);
                    }
                }
            }
        }
        r
    }

    fn pal(&self) -> &'static Pal {
        &PALETTES[self.scheme]
    }

    fn set_scheme(&mut self, i: usize) {
        self.scheme = i % PALETTES.len();
        self.wallpaper = wallpaper::Wallpaper::new(self.pal());
    }

    fn open(&mut self, dialog: Dialog) {
        self.dialog = Some(dialog);
        self.now = Instant::now();
        self.fade = Animation::new(false)
            .duration(Duration::from_millis(220))
            .go(true, self.now);
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Space(s) => {
                self.space = s;
                self.focus = Focus::Main;
            }
            Message::Focus(f) => self.focus = f,
            Message::Scheme(i) => self.set_scheme(i),
            Message::NextScheme => self.set_scheme(self.scheme + 1),
            Message::Select(i) => {
                self.selected = i;
                self.focus = Focus::Side;
            }
            Message::Search(s) => self.search = s,
            Message::Receiving(on) => {
                self.receiving = on;
                self.toast = Some(
                    if on {
                        "receiving: on (you still accept every file)"
                    } else {
                        "receiving: off"
                    }
                    .into(),
                );
            }
            Message::Review(i) => self.open(Dialog::Accept(i)),
            Message::Accept | Message::Decline => {
                if let Some(Dialog::Accept(i)) = self.dialog.take() {
                    let r = self.requests.remove(i);
                    let who = FRIENDS[r.from].name;
                    self.toast = Some(match message {
                        Message::Accept => format!("accepted {} from {who}", r.file),
                        _ => format!("declined {} from {who}", r.file),
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
                    self.toast = Some("paired (pretend); the fingerprint check comes next".into());
                }
                _ => {}
            },
            Message::Cancel => self.dialog = None,
            Message::Send(i) => {
                self.sending = Some(Sending {
                    to: i,
                    file: "report-final-v3.pdf",
                    progress: 0.0,
                })
            }
            Message::StopSending => {
                self.sending = None;
                self.toast = Some("send cancelled".into());
            }
            Message::Tick => {
                if let Some(s) = &mut self.sending {
                    s.progress += 0.5;
                    if s.progress >= 100.0 {
                        self.toast = Some(format!("{} → {} done", s.file, FRIENDS[s.to].name));
                        self.sending = None;
                    }
                }
            }
            Message::Frame(now) => self.now = now,
            Message::Key(key, mods) => {
                use keyboard::key::Named;
                match key {
                    keyboard::Key::Named(Named::Enter | Named::Escape) => {
                        return match self.dialog {
                            Some(Dialog::Accept(_)) => self.update(Message::Decline),
                            Some(_) => self.update(Message::Cancel),
                            None => Task::none(),
                        };
                    }
                    keyboard::Key::Character(c) if mods.control() => match c.as_str() {
                        "1" => return self.update(Message::Space(Space::Friends)),
                        "2" => return self.update(Message::Space(Space::Pending)),
                        "3" => return self.update(Message::Space(Space::History)),
                        "4" => return self.update(Message::Space(Space::Whoami)),
                        "t" => return self.update(Message::NextScheme),
                        _ => {}
                    },
                    _ => {}
                }
            }
            Message::Drag => return window::latest().and_then(window::drag),
            Message::Resize => {
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
        let tiles = row![
            self.tile(Focus::Side, self.sidebar(), 280.0),
            self.tile(Focus::Main, self.main(), f32::INFINITY),
        ]
        .spacing(GAP);
        let desktop = column![self.bar(), tiles].spacing(GAP).padding(GAP);
        let grip = container(
            mouse_area(
                text("◢")
                    .size(12)
                    .color(style::alpha(self.pal().overlay, 0.6)),
            )
            .on_press(Message::Resize)
            .interaction(iced::mouse::Interaction::ResizingDiagonallyDown),
        )
        .align_right(Fill)
        .align_bottom(Fill)
        .padding(2);
        let page = stack![
            canvas(&self.wallpaper).width(Fill).height(Fill),
            desktop,
            grip
        ];

        let Some(dialog) = self.dialog else {
            return page.into();
        };
        let t = self.fade.interpolate(0.0, 1.0, self.now);
        let card = match dialog {
            Dialog::Accept(i) => self.accept_dialog(i),
            Dialog::AddInvite => self.invite_dialog(),
            Dialog::AddCode => self.code_dialog(),
        };
        let card = container(card).padding(Padding {
            top: 20.0 * (1.0 - t),
            ..Padding::ZERO
        });
        stack![page, opaque(center(card).style(style::veil(t)))].into()
    }

    /// A Hyprland window: gradient frame when focused, click to focus.
    fn tile<'a>(
        &self,
        which: Focus,
        content: Element<'a, Message>,
        width: f32,
    ) -> Element<'a, Message> {
        let p = self.pal();
        let inner = container(content)
            .padding(14)
            .width(Fill)
            .height(Fill)
            .style(style::tile(p));
        let framed = container(inner)
            .padding(2)
            .width(if width.is_finite() {
                iced::Length::Fixed(width)
            } else {
                Fill
            })
            .height(Fill)
            .style(style::frame(p, self.focus == which));
        mouse_area(framed).on_press(Message::Focus(which)).into()
    }

    /// The waybar: workspaces on the left, title in the middle, modules and
    /// traffic lights on the right. Drag the empty part to move the window.
    fn bar(&self) -> Element<'_, Message> {
        let p = self.pal();
        let spaces = row(SPACES.iter().enumerate().map(|(i, (s, name))| {
            let label = if *s == Space::Pending && !self.requests.is_empty() {
                format!("{} {name} •{}", i + 1, self.requests.len())
            } else {
                format!("{} {name}", i + 1)
            };
            button(text(label).size(13))
                .padding([3, 10])
                .style(style::workspace(p, self.space == *s))
                .on_press(Message::Space(*s))
                .into()
        }))
        .spacing(4);

        let scheme = tooltip(
            button(text(format!("◐ {}", p.name)).size(13))
                .padding([3, 10])
                .style(style::pill(p, p.pink))
                .on_press(Message::NextScheme),
            self.tip("colour scheme (Ctrl+T)"),
            tooltip::Position::Bottom,
        );
        let receiving = container(
            row![
                text(if self.receiving {
                    "⇣ recv"
                } else {
                    "⇣ off"
                })
                .size(13),
                toggler(self.receiving)
                    .on_toggle(Message::Receiving)
                    .size(14)
                    .style(style::switch(p)),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([2, 4])
        .padding(Padding {
            left: 10.0,
            right: 4.0,
            top: 2.0,
            bottom: 2.0,
        })
        .style(style::module(
            p,
            if self.receiving { p.green } else { p.yellow },
        ));
        let speed = self.sending.as_ref().map_or_else(
            || {
                container(text("⇅ idle").size(13))
                    .padding([3, 10])
                    .style(style::module(p, p.teal))
            },
            |_| {
                container(text("⇡ 14.2M/s").size(13))
                    .padding([3, 10])
                    .style(style::module(p, p.teal))
            },
        );
        let clock = container(text("22:41").size(13))
            .padding([3, 10])
            .style(style::module(p, p.accent2));
        let light = |tint: Color, message: Message| {
            button(space())
                .width(13)
                .height(13)
                .style(style::light(tint))
                .on_press(message)
        };

        let bar = container(
            row![
                spaces,
                space::horizontal(),
                text("beam").font(BOLD).size(13).color(p.accent),
                text("@my-laptop").size(13).color(p.subtext),
                space::horizontal(),
                row![scheme, receiving, speed, clock]
                    .spacing(6)
                    .align_y(Alignment::Center),
                space::horizontal().width(8),
                row![
                    light(p.yellow, Message::Minimize),
                    light(p.green, Message::Maximize),
                    light(p.red, Message::Close)
                ]
                .spacing(7),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([6, 8])
        .style(style::bar(p));
        mouse_area(bar).on_press(Message::Drag).into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let p = self.pal();
        let needle = self.search.to_lowercase();
        let tints = p.tints();
        let friends = column(
            FRIENDS
                .iter()
                .enumerate()
                .filter(|(_, f)| f.name.contains(&needle))
                .map(|(i, f)| {
                    let selected = self.selected == i;
                    let marker: Element<'_, Message> = if selected {
                        container(space())
                            .width(3)
                            .height(26)
                            .style(style::tag(p.accent))
                            .into()
                    } else {
                        space().width(3).into()
                    };
                    button(
                        row![
                            marker,
                            avatar(f.name, tints[i], 32.0),
                            column![
                                text(f.name).size(14),
                                text(f.seen).size(11).color(p.overlay)
                            ],
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    )
                    .width(Fill)
                    .padding([5, 6])
                    .style(style::item(p, selected))
                    .on_press(Message::Select(i))
                    .into()
                }),
        )
        .spacing(2);

        column![
            text_input("/ find a friend", &self.search)
                .on_input(Message::Search)
                .padding([8, 12])
                .style(style::input(p)),
            row![
                heading(p, "~/friends"),
                space::horizontal(),
                text("6").size(12).color(p.overlay)
            ],
            scrollable(friends).height(Fill),
            button(text("+ add friend").font(BOLD).size(13))
                .width(Fill)
                .padding([8, 14])
                .style(style::pill(p, p.green))
                .on_press(Message::AddFriend),
        ]
        .spacing(12)
        .into()
    }

    fn main(&self) -> Element<'_, Message> {
        let body = match self.space {
            Space::Friends => self.friends_space(),
            Space::Pending => self.pending_space(),
            Space::History => self.history_space(),
            Space::Whoami => self.whoami_space(),
        };
        let mut content = column![container(body).height(Fill)].spacing(12);
        if let Some(s) = &self.sending {
            content = content.push(self.transfer(s));
        }
        if let Some(t) = &self.toast {
            content = content.push(row![
                text("❯ ").color(self.pal().green),
                text(t).color(self.pal().subtext)
            ]);
        }
        content.into()
    }

    fn friends_space(&self) -> Element<'_, Message> {
        let p = self.pal();
        let tints = p.tints();
        let f = &FRIENDS[self.selected];
        let hero = container(
            row![
                avatar(f.name, tints[self.selected], 64.0),
                column![
                    text(f.name).font(BOLD).size(24),
                    text(format!("last seen {}", f.seen))
                        .size(13)
                        .color(p.subtext),
                    text(f.fingerprint).size(12).color(p.overlay),
                ]
                .spacing(2),
                space::horizontal(),
                button(text("send a file").font(BOLD).size(14))
                    .padding([9, 20])
                    .style(style::pill(p, p.accent))
                    .on_press_maybe(
                        self.sending
                            .is_none()
                            .then_some(Message::Send(self.selected))
                    ),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        )
        .padding(16)
        .style(style::card(p));

        let actions = row![
            button(text("rename").size(13))
                .padding([6, 14])
                .style(style::ghost(p)),
            button(text("fingerprint").size(13))
                .padding([6, 14])
                .style(style::ghost(p)),
            button(text("remove").size(13))
                .padding([6, 14])
                .style(style::ghost(p)),
        ]
        .spacing(8);

        let recent = column![
            heading(p, "recent with this friend"),
            log_line(p, true, "report.pdf", f.name, "2.1M", "accepted", "2h"),
            log_line(p, false, "slides.pptx", f.name, "8.7M", "accepted", "1d"),
        ]
        .spacing(8);

        column![heading(p, "~/friends/selected"), hero, actions, recent]
            .spacing(14)
            .into()
    }

    fn pending_space(&self) -> Element<'_, Message> {
        let p = self.pal();
        let on = self.receiving;
        let tint = if on { p.green } else { p.yellow };
        let banner = container(
            row![
                column![
                    text(if on {
                        "● receiving"
                    } else {
                        "○ not receiving"
                    })
                    .font(BOLD)
                    .size(15)
                    .color(tint),
                    text(if on {
                        "friends can send while beam is open; you still accept every file"
                    } else {
                        "turn it on to let friends send without opening a terminal"
                    })
                    .size(12)
                    .color(p.subtext),
                ]
                .spacing(3),
                space::horizontal(),
                toggler(on)
                    .on_toggle(Message::Receiving)
                    .size(22)
                    .style(style::switch(p)),
            ]
            .align_y(Alignment::Center),
        )
        .padding([12, 14])
        .style(style::tag(tint));

        let tints = p.tints();
        let list: Element<'_, Message> = if self.requests.is_empty() {
            center(text("nothing waiting").color(p.overlay)).into()
        } else {
            column(self.requests.iter().enumerate().map(|(i, r)| {
                let f = &FRIENDS[r.from];
                container(
                    row![
                        avatar(f.name, tints[r.from], 40.0),
                        column![
                            row![
                                text(f.name).font(BOLD).color(p.accent),
                                text(" wants to send").color(p.subtext)
                            ],
                            row![
                                text(r.file),
                                text(format!("  {}", r.size)).color(p.overlay),
                                text("  expires 4:12").color(p.yellow),
                            ]
                            .align_y(Alignment::Center),
                        ]
                        .spacing(3),
                        space::horizontal(),
                        button(text("review").font(BOLD).size(13))
                            .padding([7, 16])
                            .style(style::pill(p, p.accent))
                            .on_press(Message::Review(i)),
                    ]
                    .spacing(14)
                    .align_y(Alignment::Center),
                )
                .padding([10, 14])
                .style(style::card(p))
                .into()
            }))
            .spacing(8)
            .into()
        };
        column![heading(p, "~/pending"), banner, list]
            .spacing(14)
            .into()
    }

    fn history_space(&self) -> Element<'_, Message> {
        let p = self.pal();
        let log = container(
            column![
                row![
                    text("❯ ").color(p.green),
                    text("beam history").color(p.text)
                ],
                log_line(p, true, "report.pdf", "alice", "2.1M", "accepted", "2h"),
                log_line(p, false, "slides.pptx", "bob", "8.7M", "accepted", "1d"),
                log_line(p, true, "game.iso", "charlie", "4.2G", "declined", "3d"),
                log_line(p, false, "notes.txt", "frank", "4K", "accepted", "now"),
                log_line(p, true, "build.tar", "eve", "310M", "expired", "2w"),
                row![text("❯ ").color(p.green), text("▌").color(p.accent)],
            ]
            .spacing(10),
        )
        .padding(16)
        .width(Fill)
        .style(style::card(p));
        column![heading(p, "~/history"), log].spacing(14).into()
    }

    /// A neofetch-style card: the ricer's screenshot staple.
    fn whoami_space(&self) -> Element<'_, Message> {
        let p = self.pal();
        let logo = text("  ██\n  ██\n  ██▀▀▀█▄\n  ██    ██\n  ██    ██\n  ██▄▄▄█▀\n\n  beam")
            .size(16)
            .font(BOLD)
            .color(p.accent);
        let line = |k: &'static str, v: String| -> Element<'_, Message> {
            row![text(k).width(90).color(p.accent).font(BOLD), text(v)].into()
        };
        let blocks = row([
            p.red, p.peach, p.yellow, p.green, p.teal, p.accent2, p.accent, p.pink,
        ]
        .into_iter()
        .map(|c| {
            container(space())
                .width(26)
                .height(16)
                .style(style::avatar(c))
                .into()
        }))
        .spacing(4);
        let info = column![
            row![
                text("you").font(BOLD).color(p.accent),
                text("@").color(p.overlay),
                text("my-laptop").font(BOLD).color(p.accent2)
            ],
            text("──────────────").color(p.overlay),
            line("os", "Windows 11 · Arch Linux".into()),
            line("beam", "0.1.0 (gui preview)".into()),
            line("key", "ed25519".into()),
            line("finger", "SHA256:3f9a 0c11 d2e4".into()),
            line("peers", "6 paired".into()),
            line(
                "receive",
                if self.receiving {
                    "on".into()
                } else {
                    "off".into()
                }
            ),
            line("relay", "n0 (default)".into()),
            line("theme", p.name.into()),
            space::vertical().height(6),
            blocks,
        ]
        .spacing(5);
        let schemes = row(PALETTES.iter().enumerate().map(|(i, s)| {
            let style = if i == self.scheme {
                style::pill(p, s.accent)
            } else {
                style::ghost(p)
            };
            button(text(s.name).size(12))
                .padding([5, 12])
                .style(style)
                .on_press(Message::Scheme(i))
                .into()
        }))
        .spacing(6);
        column![
            heading(p, "~/whoami"),
            container(row![logo, info].spacing(36))
                .padding(20)
                .width(Fill)
                .style(style::card(p)),
            heading(p, "schemes"),
            schemes,
        ]
        .spacing(14)
        .into()
    }

    fn transfer<'a>(&self, s: &'a Sending) -> Element<'a, Message> {
        let p = self.pal();
        container(
            column![
                row![
                    text("⇡ ").color(p.accent),
                    text(s.file),
                    text(" → ").color(p.overlay),
                    text(FRIENDS[s.to].name).color(p.accent2),
                    space::horizontal(),
                    text(format!("{:>3.0}%  [direct p2p]  14.2M/s", s.progress))
                        .size(12)
                        .color(p.subtext),
                    space::horizontal().width(10),
                    button(text("cancel").size(12))
                        .padding([4, 12])
                        .style(style::pill(p, p.red))
                        .on_press(Message::StopSending),
                ]
                .align_y(Alignment::Center),
                progress_bar(0.0..=100.0, s.progress)
                    .girth(8)
                    .style(style::progress(p)),
            ]
            .spacing(10),
        )
        .padding([12, 14])
        .style(style::card(p))
        .into()
    }

    // -----------------------------------------------------------------------
    // Dialogs: a floating, focused tile
    // -----------------------------------------------------------------------

    fn floating<'a>(&self, title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
        let p = self.pal();
        let inner = container(column![heading(p, title), content].spacing(14))
            .padding(20)
            .width(Fill)
            .style(style::tile(p));
        container(inner)
            .padding(2)
            .width(480)
            .style(style::frame(p, true))
            .into()
    }

    /// Decline is the filled default; Enter and Esc decline.
    fn accept_dialog(&self, i: usize) -> Element<'_, Message> {
        let p = self.pal();
        let r = &self.requests[i];
        let f = &FRIENDS[r.from];
        self.floating(
            "~/incoming",
            column![
                row![
                    avatar(f.name, p.tints()[r.from], 52.0),
                    column![
                        text(f.name).font(BOLD).size(20),
                        text("wants to send you a file").size(13).color(p.subtext)
                    ]
                ]
                .spacing(14)
                .align_y(Alignment::Center),
                container(row![
                    text(r.file).font(BOLD),
                    space::horizontal(),
                    text(r.size).color(p.overlay)
                ])
                .padding([10, 14])
                .style(style::card(p)),
                column![
                    text("fingerprint").size(11).color(p.accent),
                    text(f.fingerprint).size(13)
                ]
                .spacing(3),
                text("only accept files you expect · enter/esc declines")
                    .size(12)
                    .color(p.overlay),
                row![
                    space::horizontal(),
                    button(text("accept").size(14))
                        .padding([8, 20])
                        .style(style::ghost(p))
                        .on_press(Message::Accept),
                    button(text("decline").font(BOLD).size(14))
                        .padding([8, 20])
                        .style(style::pill(p, p.accent))
                        .on_press(Message::Decline),
                ]
                .spacing(10),
            ]
            .spacing(14)
            .into(),
        )
    }

    fn invite_dialog(&self) -> Element<'_, Message> {
        let p = self.pal();
        self.floating(
            "~/add-friend",
            column![
                text("paste the invite your friend sent (beam1…)")
                    .size(13)
                    .color(p.subtext),
                text_input("beam1…", &self.invite)
                    .on_input(Message::Invite)
                    .on_submit(Message::Next)
                    .padding(10)
                    .style(style::input(p)),
                row![
                    space::horizontal(),
                    button(text("cancel").size(14))
                        .padding([8, 20])
                        .style(style::ghost(p))
                        .on_press(Message::Cancel),
                    button(text("next").font(BOLD).size(14))
                        .padding([8, 20])
                        .style(style::pill(p, p.accent))
                        .on_press_maybe((!self.invite.trim().is_empty()).then_some(Message::Next)),
                ]
                .spacing(10),
            ]
            .spacing(14)
            .into(),
        )
    }

    fn code_dialog(&self) -> Element<'_, Message> {
        let p = self.pal();
        self.floating(
            "~/pairing-code",
            column![
                text("your friend's beam shows a 6-digit code")
                    .size(13)
                    .color(p.subtext),
                text_input("______", &self.code)
                    .on_input(Message::Code)
                    .on_submit(Message::Next)
                    .padding(12)
                    .size(26)
                    .style(style::input(p)),
                row![
                    space::horizontal(),
                    button(text("cancel").size(14))
                        .padding([8, 20])
                        .style(style::ghost(p))
                        .on_press(Message::Cancel),
                    button(text("pair").font(BOLD).size(14))
                        .padding([8, 20])
                        .style(style::pill(p, p.accent))
                        .on_press_maybe((self.code.len() == 6).then_some(Message::Next)),
                ]
                .spacing(10),
            ]
            .spacing(14)
            .into(),
        )
    }

    fn tip<'a>(&self, s: &'a str) -> Element<'a, Message> {
        container(text(s).size(12))
            .padding([5, 9])
            .style(style::card(self.pal()))
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![keyboard::listen().filter_map(|event| match event {
            keyboard::Event::KeyPressed { key, modifiers, .. } => {
                Some(Message::Key(key, modifiers))
            }
            _ => None,
        })];
        if self.sending.is_some() {
            subs.push(iced::time::every(Duration::from_millis(80)).map(|_| Message::Tick));
        }
        if self.fade.is_animating(self.now) {
            subs.push(window::frames().map(Message::Frame));
        }
        Subscription::batch(subs)
    }
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

fn avatar<'a>(name: &str, tint: Color, size: f32) -> Element<'a, Message> {
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_ascii_uppercase()
        .to_string();
    container(center(text(initial).font(BOLD).size(size * 0.42)))
        .width(size)
        .height(size)
        .style(style::avatar(tint))
        .into()
}

fn heading<'a>(p: &'static Pal, s: &'a str) -> Element<'a, Message> {
    text(s).size(12).font(BOLD).color(p.overlay).into()
}

/// One line of history, coloured like a riced shell prompt.
fn log_line<'a>(
    p: &'static Pal,
    sent: bool,
    file: &'a str,
    who: &'a str,
    size: &'a str,
    outcome: &'a str,
    when: &'a str,
) -> Element<'a, Message> {
    let (arrow, link, tint) = if sent {
        ("⇡", " → ", p.accent)
    } else {
        ("⇣", " ← ", p.green)
    };
    let outcome_tint = match outcome {
        "accepted" => p.green,
        "declined" => p.red,
        _ => p.yellow,
    };
    row![
        text(format!("{arrow} ")).color(tint),
        text(file).width(150),
        text(link).color(p.overlay),
        text(who).width(80).color(p.accent2),
        text(size).width(60).color(p.subtext),
        container(text(outcome).size(11))
            .padding([1, 8])
            .style(style::tag(outcome_tint)),
        space::horizontal(),
        text(when).color(p.overlay),
    ]
    .align_y(Alignment::Center)
    .into()
}
