//! What the window draws. Words, layout and styles only: every string from
//! `beam-model` arrives cleaned (ADR-0034), and nothing here changes state.

use beam_core::history::{self, Direction, Outcome};
use beam_model::snapshot::{Agent, Friend, Snapshot};
use iced::widget::text::Wrapping;
use iced::widget::{
    button, center, column, container, mouse_area, row, rule, scrollable, space, stack, text,
    text_input,
};
use iced::{Alignment, Color, Element, Fill, Padding};

use crate::app::{Gui, Message, Page, Tone};
use crate::icons;
use crate::theme::{self, BOLD, LG, MD, MONO, Pal, SEMIBOLD, SM, XL, XS, XXL};

/// The navigation rail: its width, the equal gutter left and right of each
/// item, and the box an item's icon is centred in.
const RAIL_WIDTH: f32 = 76.0;
const RAIL_GUTTER: f32 = 6.0;
const RAIL_GLYPH: f32 = 40.0;

pub(crate) fn window(gui: &Gui) -> Element<'_, Message> {
    let p = gui.pal();
    let Some(snapshot) = &gui.snapshot else {
        let why = gui.problem.as_deref().unwrap_or("no beam home");
        return container(empty(
            p,
            icons::WARNING,
            "beam cannot find its home folder",
            why,
        ))
        .center(Fill)
        .style(theme::surface(p))
        .into();
    };
    let body: Element<'_, Message> = if snapshot.needs_setup {
        welcome(gui)
    } else {
        let page: Element<'_, Message> = match gui.page {
            Page::Friends => row![friend_pane(gui, snapshot), friend_detail(gui, snapshot)].into(),
            Page::Transfers => later(
                p,
                icons::TRANSFERS,
                "Transfers",
                "Sending from this window comes in a later version.",
                "Meanwhile, in a terminal: beam send <friend> <file>",
            ),
            Page::Inbox => later(
                p,
                icons::INBOX,
                "Inbox",
                "Receiving in this window comes in a later version.",
                "Meanwhile: beam listen, or the Receiving switch in the full-screen view (beam).",
            ),
            Page::History => later(
                p,
                icons::HISTORY,
                "History",
                "History in this window comes in a later version.",
                "Meanwhile, in a terminal: beam history",
            ),
            Page::Settings => settings(gui, snapshot),
        };
        row![
            rail(gui, snapshot),
            container(page)
                .width(Fill)
                .height(Fill)
                .style(theme::surface(p))
        ]
        .height(Fill)
        .into()
    };
    let frame = column![body, status_bar(gui, snapshot)];
    match &gui.notice {
        None => frame.into(),
        Some((words, tone)) => stack![
            frame,
            container(toast(p, words, *tone))
                .align_right(Fill)
                .align_bottom(Fill)
                .padding(Padding {
                    bottom: 40.0,
                    right: 20.0,
                    ..Padding::ZERO
                }),
        ]
        .into(),
    }
}

// ---------------------------------------------------------------------------
// First run
// ---------------------------------------------------------------------------

fn welcome(gui: &Gui) -> Element<'_, Message> {
    let p = gui.pal();
    let card = container(
        column![
            container(center(text("b").font(BOLD).size(XXL).line_height(1.0)))
                .width(56)
                .height(56)
                .style(theme::avatar(p.accent)),
            text("Welcome to beam").font(BOLD).size(XXL),
            text(
                "beam sends files straight to the people you pair with. \
                 First, this device needs its own identity: a key pair that \
                 proves who you are to your friends."
            )
            .size(MD)
            .color(p.subtext)
            .align_x(Alignment::Center),
            button(
                row![
                    icon(icons::KEY, 13),
                    text("Create this device's identity").font(SEMIBOLD)
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([10, 20])
            .style(theme::primary(p))
            .on_press(Message::CreateIdentity),
            notice(
                p,
                p.info,
                icons::LOCK,
                "The same as beam init. The private key is kept in your beam folder and never leaves this device.",
            ),
        ]
        .spacing(16)
        .align_x(Alignment::Center),
    )
    .padding(32)
    .max_width(520)
    .style(theme::card(p));
    container(card).center(Fill).style(theme::surface(p)).into()
}

// ---------------------------------------------------------------------------
// The frame: rail and status bar
// ---------------------------------------------------------------------------

fn rail<'a>(gui: &'a Gui, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let p = gui.pal();
    // Every rail element is centred on the rail itself: the button fills the
    // rail minus an equal gutter on both sides, its content is centred inside
    // it (a button does not centre by itself), and the selection marker is
    // drawn on top at the left edge instead of taking up room in the row.
    let item = |glyph: char, label: &'static str, page: Page| {
        let selected = gui.page == page;
        let body = button(
            column![
                center(icon(glyph, 20)).width(RAIL_GLYPH).height(30),
                text(label).size(XS)
            ]
            .spacing(2)
            .width(Fill)
            .align_x(Alignment::Center),
        )
        .width(Fill)
        .padding([6, 0])
        .style(theme::rail_item(p, selected))
        .on_press(Message::Page(page));
        let mut item = stack![container(body).padding([0.0, RAIL_GUTTER])];
        if selected {
            item = item.push(
                container(
                    container(space::vertical())
                        .width(3)
                        .height(28)
                        .style(theme::rail_marker(p)),
                )
                .height(Fill)
                .center_y(Fill),
            );
        }
        item
    };
    let logo = container(center(text("b").font(BOLD).size(XL).line_height(1.0)))
        .width(36)
        .height(36)
        .style(theme::avatar(p.accent));
    let me: Element<'_, Message> = match &snapshot.me {
        Some(me) => avatar(
            p,
            &me.name,
            theme::tint_for(&me.fingerprint),
            32.0,
            Some(receiving(p, snapshot).1),
        ),
        None => space::vertical().height(0).into(),
    };
    container(
        column![
            container(logo).padding([14, 0]).center_x(Fill),
            item(icons::PEOPLE, "Friends", Page::Friends),
            item(icons::TRANSFERS, "Transfers", Page::Transfers),
            item(icons::INBOX, "Inbox", Page::Inbox),
            item(icons::HISTORY, "History", Page::History),
            space::vertical(),
            item(icons::SETTINGS, "Settings", Page::Settings),
            container(me).padding([12, 0]).center_x(Fill),
        ]
        .spacing(4)
        .height(Fill),
    )
    .width(RAIL_WIDTH)
    .height(Fill)
    .style(theme::rail(p))
    .into()
}

/// Whether anything receives for this beam home, in words and a colour.
fn receiving(p: &'static Pal, snapshot: &Snapshot) -> (&'static str, Color) {
    match &snapshot.agent {
        Agent::Running { .. } => ("Receiving: on (background agent)", p.success),
        Agent::Stopped => ("Receiving: off", p.warning),
        Agent::Unreadable => ("Receiving: unknown", p.faint),
    }
}

fn status_bar<'a>(gui: &'a Gui, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let p = gui.pal();
    let sep = || text("|").size(SM).color(p.border);
    let item = |glyph: char, words: String, tint: Color| {
        row![
            icon(glyph, 11).color(tint),
            text(words).size(SM).wrapping(Wrapping::None)
        ]
        .spacing(6)
        .align_y(Alignment::Center)
    };
    // One line, always: on a narrow window the least important items go
    // (the port, then this device's fingerprint) instead of wrapping.
    let (receiving, tint) = receiving(p, snapshot);
    let mut bar = row![item(icons::DOWNLOAD, receiving.into(), tint)]
        .spacing(10)
        .align_y(Alignment::Center);
    bar = bar.push(sep()).push(match &gui.config {
        Some(_) => item(
            icons::GLOBE,
            format!("Relay {}", gui.relay_label()),
            p.subtext,
        ),
        None => item(icons::WARNING, "config.toml unreadable".into(), p.danger),
    });
    if let Some(config) = gui.config.as_ref().filter(|_| gui.width >= 1150.0) {
        bar = bar
            .push(sep())
            .push(item(icons::PLUG, format!("UDP {}", config.port), p.subtext));
    }
    bar = bar.push(space::horizontal());
    if let Some(me) = snapshot.me.as_ref().filter(|_| gui.width >= 1000.0) {
        bar = bar
            .push(item(
                icons::KEY,
                format!("This device SHA256:{}", short(&me.fingerprint)),
                p.subtext,
            ))
            .push(sep());
    }
    bar = bar.push(
        text(concat!("beam ", env!("CARGO_PKG_VERSION")))
            .size(SM)
            .wrapping(Wrapping::None),
    );
    container(bar)
        .padding([4, 12])
        .width(Fill)
        .style(theme::status_bar(p))
        .into()
}

// ---------------------------------------------------------------------------
// Friends
// ---------------------------------------------------------------------------

fn friend_pane<'a>(gui: &'a Gui, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let p = gui.pal();
    let needle = gui.search.to_lowercase();
    let now = history::unix_now();
    let matching: Vec<&Friend> = snapshot
        .friends
        .iter()
        .filter(|f| f.name.to_lowercase().contains(&needle))
        .collect();
    let mut list = column![].spacing(2);
    if snapshot.friends.is_empty() {
        list = list.push(
            column![
                text("No friends yet").font(SEMIBOLD).size(MD),
                text("Pair in a terminal for now: beam pair <their invite> --name <a name>")
                    .size(SM)
                    .color(p.subtext),
            ]
            .spacing(4)
            .padding(8),
        );
    } else if matching.is_empty() {
        list = list.push(
            container(text("No friend matches.").size(MD).color(p.subtext))
                .padding(16)
                .center_x(Fill),
        );
    } else {
        list = list.push(group_label(p, "ALL FRIENDS", matching.len()));
        for f in matching {
            let selected = gui.selected.as_deref() == Some(f.name.as_str());
            list = list.push(
                button(
                    row![
                        avatar(p, &f.name, theme::tint_for(&f.fingerprint), 34.0, None),
                        column![
                            text(&f.name).font(SEMIBOLD).size(MD),
                            text(format!("Last seen {}", last_seen(f, now)))
                                .size(SM)
                                .color(p.subtext),
                        ]
                        .spacing(1)
                        .width(Fill),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .width(Fill)
                .padding([7, 8])
                .style(theme::list_item(p, selected))
                .on_press(Message::Select(f.name.clone())),
            );
        }
    }
    container(
        column![
            text("Friends").font(BOLD).size(XL),
            text_input("Search friends", &gui.search)
                .on_input(Message::Search)
                .icon(text_input::Icon {
                    font: icons::FONT,
                    code_point: icons::SEARCH,
                    size: Some(14.into()),
                    spacing: 8.0,
                    side: text_input::Side::Left,
                })
                .padding([8, 10])
                .size(MD)
                .style(theme::input(p)),
            scrollable(list).height(Fill),
        ]
        .spacing(12),
    )
    .padding(16)
    .width(gui.pane_width())
    .height(Fill)
    .style(theme::pane(p))
    .into()
}

fn friend_detail<'a>(gui: &'a Gui, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let p = gui.pal();
    let Some(f) = gui
        .selected
        .as_ref()
        .and_then(|name| snapshot.friends.iter().find(|f| &f.name == name))
    else {
        return empty(
            p,
            icons::PEOPLE,
            "No friend selected",
            "Choose a friend on the left to see who they are.",
        );
    };
    let now = history::unix_now();
    let room = gui.width - RAIL_WIDTH - gui.pane_width();

    // Below these content widths the header stacks rather than squeezing.
    let name = text(&f.name).font(BOLD).size(XXL).wrapping(Wrapping::None);
    let paired = chip(p, p.success, icons::SHIELD, "Key saved when you paired");
    let title: Element<'_, Message> = if room < 640.0 {
        column![name, paired].spacing(6).into()
    } else {
        row![name, paired]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
    };
    let header = toolbar(
        p,
        column![
            breadcrumb(p, "Friends", &f.name),
            row![
                avatar(p, &f.name, theme::tint_for(&f.fingerprint), 48.0, None),
                column![
                    title,
                    text(format!(
                        "Short ID {}  ·  paired {}  ·  last seen {}",
                        f.short_id,
                        f.added,
                        last_seen(f, now)
                    ))
                    .size(MD)
                    .color(p.subtext),
                ]
                .spacing(4)
                .width(Fill),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        ]
        .spacing(10),
    );

    // What came and went with this friend, from history.jsonl.
    let theirs = || {
        snapshot
            .history
            .iter()
            .filter(|e| e.fingerprint == f.fingerprint && e.outcome == Outcome::Done)
    };
    let sent = theirs().filter(|e| e.direction == Direction::Sent).count();
    let received = theirs()
        .filter(|e| e.direction == Direction::Received)
        .count();
    let tiles = [
        stat(p, icons::UPLOAD, "Files sent", sent.to_string(), p.accent),
        stat(
            p,
            icons::DOWNLOAD,
            "Files received",
            received.to_string(),
            p.success,
        ),
        stat(p, icons::CLOCK, "Last answer", last_seen(f, now), p.info),
    ];
    let stats: Element<'_, Message> = if room < 700.0 {
        column(tiles).spacing(12).into()
    } else {
        row(tiles).spacing(12).into()
    };

    let identity = section(
        p,
        "Identity",
        column![
            text("Fingerprint (SHA-256 of their public key)")
                .size(SM)
                .color(p.subtext),
            container(
                text(format!("SHA256:{}", grouped(&f.fingerprint)))
                    .font(MONO)
                    .size(MD)
            )
            .padding([10, 12])
            .width(Fill)
            .style(theme::code_block(p)),
            notice(
                p,
                p.info,
                icons::INFO,
                "beam checks this key on every connection. If it ever changes, the transfer stops with a warning and you pair again; it is never updated silently.",
            ),
        ]
        .spacing(12),
    );

    column![
        header,
        scrollable(
            column![stats, identity]
                .spacing(16)
                .padding(Padding::new(24.0).top(18.0))
        )
        .height(Fill),
    ]
    .into()
}

fn last_seen(f: &Friend, now: u64) -> String {
    f.last_seen
        .map_or_else(|| "never".into(), |at| history::ago(at, now))
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

fn settings<'a>(gui: &'a Gui, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let p = gui.pal();
    let segment = |glyph: char, label: &'static str, dark: bool| {
        button(
            row![icon(glyph, 12), text(label).size(MD)]
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .padding([5, 12])
        .style(theme::segment(p, gui.dark == dark))
        .on_press(Message::Dark(dark))
    };
    let appearance = settings_group(
        p,
        "Appearance",
        vec![setting(
            p,
            "Theme",
            "Midnight (dark) or Daylight (light).",
            container(
                row![
                    segment(icons::MOON, "Midnight", true),
                    segment(icons::SUN, "Daylight", false)
                ]
                .spacing(2),
            )
            .padding(3)
            .style(theme::segments(p))
            .into(),
        )],
    );
    let mut device = Vec::new();
    if let Some(me) = &snapshot.me {
        device.push(setting(
            p,
            "Device name",
            "What friends see when you pair. Set by beam init from the computer's name.",
            text(&me.name).font(SEMIBOLD).size(MD).into(),
        ));
        device.push(setting(
            p,
            "Short ID",
            "Part of the pairing check.",
            text(&me.short_id).font(MONO).size(MD).into(),
        ));
        device.push(setting(
            p,
            "Fingerprint",
            "SHA-256 of this device's public key. Friends compare it when pairing.",
            container(
                text(format!("SHA256:{}", short(&me.fingerprint)))
                    .font(MONO)
                    .size(SM),
            )
            .padding([7, 10])
            .style(theme::code_block(p))
            .into(),
        ));
        device.push(setting(
            p,
            "Private key",
            "Kept in your beam folder; it never leaves this device.",
            chip(p, p.success, icons::LOCK, "On this device only"),
        ));
    }
    let mut body = column![appearance].spacing(16);
    if !device.is_empty() {
        body = body.push(settings_group(p, "This device", device));
    }
    if let Some(problem) = &gui.problem {
        body = body.push(notice(p, p.danger, icons::WARNING, problem));
    }
    column![
        toolbar(
            p,
            page_title(
                p,
                "Settings",
                "More settings come with the pages that use them."
            )
        ),
        scrollable(
            container(body)
                .padding(Padding::new(24.0).top(18.0))
                .max_width(900)
        )
        .height(Fill),
    ]
    .into()
}

/// A page still to come: what it will do, and how to do it today.
fn later<'a>(
    p: &'static Pal,
    glyph: char,
    title: &'a str,
    words: &'a str,
    meanwhile: &'a str,
) -> Element<'a, Message> {
    column![
        toolbar(p, page_title(p, title, words)),
        container(empty(p, glyph, "Not in this window yet", meanwhile)).height(Fill),
    ]
    .into()
}

// ---------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------

/// An icon glyph. Line height 1.0, or the glyph sits high in its box; 1.25x
/// because Fluent's glyphs fill less of their em box than text does.
fn icon<'a>(glyph: char, size: u32) -> text::Text<'a> {
    text(glyph.to_string())
        .font(icons::FONT)
        .size(size as f32 * 1.25)
        .line_height(1.0)
}

/// Initials for an avatar: the first letter of the first two words.
fn initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

fn avatar<'a>(
    p: &'static Pal,
    name: &str,
    tint: Color,
    size: f32,
    dot: Option<Color>,
) -> Element<'a, Message> {
    let face = container(center(
        text(initials(name))
            .font(SEMIBOLD)
            .size(size * 0.38)
            .line_height(1.0),
    ))
    .width(size)
    .height(size)
    .style(theme::avatar(tint));
    match dot {
        None => face.into(),
        Some(c) => stack![
            face,
            container(
                container(space::horizontal())
                    .width(11)
                    .height(11)
                    .style(theme::dot(p, c))
            )
            .width(size)
            .height(size)
            .align_right(size)
            .align_bottom(size),
        ]
        .into(),
    }
}

fn chip<'a>(p: &'static Pal, tint: Color, glyph: char, words: &'a str) -> Element<'a, Message> {
    container(
        row![icon(glyph, 10), text(words).size(XS).font(SEMIBOLD)]
            .spacing(5)
            .align_y(Alignment::Center),
    )
    .padding([3, 9])
    .style(theme::chip(p, tint))
    .into()
}

fn toolbar<'a>(p: &'static Pal, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding([16, 24])
        .width(Fill)
        .style(theme::toolbar(p))
        .into()
}

fn breadcrumb<'a>(p: &'static Pal, page: &'a str, here: &'a str) -> Element<'a, Message> {
    row![
        text(page).size(SM).color(p.faint),
        icon(icons::CHEVRON_RIGHT, 8).color(p.faint),
        text(here).size(SM).color(p.subtext),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn page_title<'a>(p: &'static Pal, title: &'a str, subtitle: &'a str) -> Element<'a, Message> {
    column![
        text(title).font(BOLD).size(XXL),
        text(subtitle).size(MD).color(p.subtext)
    ]
    .spacing(4)
    .into()
}

fn group_label<'a>(p: &'static Pal, label: &'a str, count: usize) -> Element<'a, Message> {
    container(
        row![
            text(label).size(XS).font(BOLD).color(p.faint),
            text(count.to_string()).size(XS).color(p.faint)
        ]
        .spacing(6),
    )
    .padding([6, 8])
    .into()
}

/// A card with a title strip: the basic block of every page.
fn section<'a>(
    p: &'static Pal,
    title: &'a str,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(column![
        container(text(title).font(SEMIBOLD).size(MD))
            .padding([8, 16])
            .width(Fill)
            .style(theme::card_header(p)),
        container(content).padding(16).width(Fill),
    ])
    .width(Fill)
    .style(theme::card(p))
    .into()
}

fn stat<'a>(
    p: &'static Pal,
    glyph: char,
    label: &'a str,
    value: String,
    tint: Color,
) -> Element<'a, Message> {
    container(
        row![
            container(center(icon(glyph, 16)))
                .width(38)
                .height(38)
                .style(theme::icon_tile(p, tint)),
            column![
                text(label).size(SM).color(p.subtext),
                text(value).font(BOLD).size(XL).wrapping(Wrapping::None)
            ]
            .spacing(1),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([12, 14])
    .width(Fill)
    .style(theme::card(p))
    .into()
}

fn notice<'a>(p: &'static Pal, tint: Color, glyph: char, words: &'a str) -> Element<'a, Message> {
    container(
        row![icon(glyph, 14).color(tint), text(words).size(MD)]
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .padding([10, 14])
    .width(Fill)
    .style(theme::notice(p, tint))
    .into()
}

fn empty<'a>(p: &'static Pal, glyph: char, title: &'a str, words: &'a str) -> Element<'a, Message> {
    container(
        column![
            container(center(icon(glyph, 22)))
                .width(52)
                .height(52)
                .style(theme::icon_tile(p, p.faint)),
            text(title).font(SEMIBOLD).size(LG),
            text(words).size(MD).color(p.subtext),
        ]
        .spacing(8)
        .align_x(Alignment::Center),
    )
    .padding(28)
    .center_x(Fill)
    .into()
}

fn settings_group<'a>(
    p: &'static Pal,
    title: &'a str,
    rows: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut list = column![];
    let n = rows.len();
    for (k, r) in rows.into_iter().enumerate() {
        list = list.push(r);
        if k + 1 < n {
            list = list.push(rule::horizontal(1).style(move |_| rule::Style {
                color: p.divider,
                radius: 0.0.into(),
                fill_mode: rule::FillMode::Full,
                snap: true,
            }));
        }
    }
    column![
        text(title).font(SEMIBOLD).size(LG),
        container(list).width(Fill).style(theme::card(p)),
    ]
    .spacing(10)
    .into()
}

fn setting<'a>(
    p: &'static Pal,
    label: &'a str,
    help: &'a str,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    container(
        row![
            column![
                text(label).font(SEMIBOLD).size(MD),
                text(help).size(SM).color(p.subtext)
            ]
            .spacing(3)
            .width(Fill),
            control,
        ]
        .spacing(24)
        .align_y(Alignment::Center),
    )
    .padding([14, 16])
    .into()
}

fn toast<'a>(p: &'static Pal, words: &'a str, tone: Tone) -> Element<'a, Message> {
    let (glyph, tint) = match tone {
        Tone::Info => (icons::INFO, p.info),
        Tone::Success => (icons::CHECK, p.success),
        Tone::Problem => (icons::WARNING, p.danger),
    };
    mouse_area(
        container(
            row![
                container(center(icon(glyph, 13)))
                    .width(28)
                    .height(28)
                    .style(theme::icon_tile(p, tint)),
                text(words).size(MD).width(Fill),
                button(icon(icons::CLOSE, 9))
                    .padding(6)
                    .style(theme::ghost(p))
                    .on_press(Message::DismissNotice),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .padding([10, 12])
        .width(420)
        .style(theme::dialog(p)),
    )
    .into()
}

/// A fingerprint's hex in groups of four, for reading aloud.
pub(crate) fn grouped(hex: &str) -> String {
    hex.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The start and the end of a fingerprint, for places with little room.
fn short(hex: &str) -> String {
    if hex.len() <= 12 {
        return grouped(hex);
    }
    format!("{} … {}", grouped(&hex[..8]), &hex[hex.len() - 4..])
}
