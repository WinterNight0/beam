//! What the window draws. Words, layout and styles only: every string from
//! `beam-model` arrives cleaned (ADR-0034), and nothing here changes state.

use beam_core::history::{self, Direction, Entry, Outcome};
use beam_core::text::format_bytes;
use beam_model::Choice;
use beam_model::ask::{Incoming, RelayMove};
use beam_model::requests::{Switch, Taken};
use beam_model::sending::{Sending, Stage};
use beam_model::snapshot::{Agent, Friend, Owner, Snapshot};
use iced::widget::text::Wrapping;
use iced::widget::{
    button, center, checkbox, column, container, mouse_area, opaque, progress_bar, row, rule,
    scrollable, space, stack, text, text_input, toggler, tooltip,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Padding};

use crate::app::{
    Dialog, Gui, HistoryFilter, Message, Page, PairMsg, Pairing, RecvMsg, SendMsg, SettingsMsg,
    Tone,
};
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
            Page::Transfers => transfers_page(gui, snapshot),
            Page::Inbox => inbox_page(gui, snapshot),
            Page::History => history_page(gui, snapshot),
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
    let mut layers = stack![column![body, status_bar(gui, snapshot)]];
    if let Some((words, tone)) = &gui.notice {
        layers = layers.push(
            container(toast(p, words, *tone))
                .align_right(Fill)
                .align_bottom(Fill)
                .padding(Padding {
                    bottom: 40.0,
                    right: 20.0,
                    ..Padding::ZERO
                }),
        );
    }
    if let Some(open) = &gui.dialog {
        let card = match open {
            Dialog::Rename { from, to, error } => rename_dialog(p, from, to, error.as_deref()),
            Dialog::Remove(ask) => {
                remove_dialog(p, &ask.question().name, &ask.question().fingerprint)
            }
            Dialog::Pair(state) => pairing_dialog(gui, state),
            Dialog::RelayMove(ask) => relay_dialog(p, ask.question()),
            Dialog::Accept(ask) => accept_dialog(gui, ask.question()),
            Dialog::StopReceiving(_) => stop_dialog(p),
            Dialog::Leave(..) => leave_dialog(p),
            Dialog::SendDropped { friend, path } => drop_dialog(p, friend, path),
            Dialog::PortMapping(_) => port_mapping_dialog(p),
            Dialog::ClearHistory(_) => clear_dialog(p),
        };
        // Clicks do not reach the window behind a dialog.
        layers = layers.push(opaque(center(card).style(theme::veil(p, 1.0))));
    }
    layers.into()
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
    let item = |glyph: char, label: &'static str, page: Page, count: usize| {
        let selected = gui.page == page;
        let mut glyph_layer = stack![center(icon(glyph, 20)).width(RAIL_GLYPH).height(30)];
        if count > 0 {
            glyph_layer = glyph_layer.push(
                container(
                    container(center(
                        text(count.to_string()).size(XS).font(BOLD).line_height(1.0),
                    ))
                    .width(18)
                    .height(18)
                    .style(theme::badge(p)),
                )
                .width(RAIL_GLYPH)
                .align_right(RAIL_GLYPH),
            );
        }
        let body = button(
            column![glyph_layer, text(label).size(XS)]
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
            item(icons::PEOPLE, "Friends", Page::Friends, 0),
            item(
                icons::TRANSFERS,
                "Transfers",
                Page::Transfers,
                usize::from(gui.sending_now())
            ),
            item(
                icons::INBOX,
                "Inbox",
                Page::Inbox,
                gui.requests.waiting.len()
            ),
            item(icons::HISTORY, "History", Page::History, 0),
            space::vertical(),
            item(icons::SETTINGS, "Settings", Page::Settings, 0),
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
        Agent::Running { .. } if snapshot.owner == Owner::ThisView => {
            ("Receiving: on (this window)", p.success)
        }
        Agent::Running { .. } if snapshot.owner == Owner::OtherView => {
            ("Receiving: on (another beam window)", p.success)
        }
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
                text("Add one with the + button: paste their invite, or show yours.")
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
        // Friends who answered in the last week first: who you are most
        // likely looking for.
        const RECENT: u64 = 7 * 86_400;
        let recent = |f: &&Friend| {
            f.last_seen
                .is_some_and(|at| now.saturating_sub(at) < RECENT)
        };
        let (active, others): (Vec<&Friend>, Vec<&Friend>) = matching.into_iter().partition(recent);
        let both = !active.is_empty() && !others.is_empty();
        for (label, group) in [("RECENTLY ACTIVE", active), ("EVERYONE ELSE", others)] {
            if group.is_empty() {
                continue;
            }
            let label = if both { label } else { "ALL FRIENDS" };
            list = list.push(group_label(p, label, group.len()));
            for f in group {
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
    }
    container(
        column![
            row![
                text("Friends").font(BOLD).size(XL),
                space::horizontal(),
                tip(
                    p,
                    button(icon(icons::ADD, 14))
                        .padding(8)
                        .style(theme::primary(p))
                        .on_press(Message::AddFriend),
                    "Add a friend",
                ),
            ]
            .align_y(Alignment::Center),
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
    let actions = row![
        tip(
            p,
            button(
                row![icon(icons::SEND, 13), text("Send a file").font(SEMIBOLD)]
                    .spacing(8)
                    .align_y(Alignment::Center)
            )
            .padding([7, 14])
            .style(theme::primary(p))
            .on_press(Message::Send(SendMsg::Pick(f.name.clone()))),
            "Choose a file, or drop one on this page; they answer before anything moves",
        ),
        tip(
            p,
            button(
                row![icon(icons::EDIT, 13), text("Rename").size(MD)]
                    .spacing(8)
                    .align_y(Alignment::Center)
            )
            .padding([7, 14])
            .style(theme::secondary(p))
            .on_press(Message::Rename(f.name.clone())),
            "A new name on this device only; they never see it",
        ),
        tip(
            p,
            button(
                row![icon(icons::DELETE, 13), text("Remove").size(MD)]
                    .spacing(8)
                    .align_y(Alignment::Center)
            )
            .padding([7, 14])
            .style(theme::danger(p))
            .on_press(Message::Remove(f.name.clone())),
            "Forget this friend's key; you can pair again later",
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let who = row![
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
    .align_y(Alignment::Center);
    let head: Element<'_, Message> = if room < 780.0 {
        column![who, actions].spacing(12).into()
    } else {
        row![who, actions]
            .spacing(16)
            .align_y(Alignment::Center)
            .into()
    };
    let header = toolbar(
        p,
        column![breadcrumb(p, "Friends", &f.name), head].spacing(10),
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

    let copy = tip(
        p,
        button(
            row![icon(icons::COPY, 12), text("Copy").size(SM)]
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .padding([4, 10])
        .style(theme::ghost(p))
        .on_press(Message::CopyFingerprint(f.fingerprint.clone())),
        "Copy the full fingerprint, to compare it with theirs",
    );
    let identity = section(
        p,
        "Identity",
        Some(copy),
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

    // Newest first, the last few; the History page will hold the rest.
    let recent: Vec<&Entry> = snapshot
        .history
        .iter()
        .rev()
        .filter(|e| e.fingerprint == f.fingerprint)
        .take(5)
        .collect();
    let activity = section(
        p,
        "Recent activity",
        None,
        if recent.is_empty() {
            empty(
                p,
                icons::HISTORY,
                "Nothing yet",
                "Files you send to or receive from this friend are listed here.",
            )
        } else {
            activity_table(p, &recent, now)
        },
    );

    column![
        header,
        scrollable(
            column![waiting_from(gui, f), stats, identity, activity]
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
    let mut body = column![
        received_settings(gui),
        network_settings(gui),
        service_settings(gui, snapshot),
        appearance
    ]
    .spacing(16);
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
                "Saved in config.toml in your beam folder: the command line uses the same settings."
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

fn chip<'a>(
    p: &'static Pal,
    tint: Color,
    glyph: char,
    words: impl text::IntoFragment<'a>,
) -> Element<'a, Message> {
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
    title: impl text::IntoFragment<'a>,
    trailing: Option<Element<'a, Message>>,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut head = row![text(title).font(SEMIBOLD).size(MD).width(Fill)].align_y(Alignment::Center);
    if let Some(trailing) = trailing {
        head = head.push(trailing);
    }
    container(column![
        container(head)
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

fn tip<'a>(
    p: &'static Pal,
    content: impl Into<Element<'a, Message>>,
    words: &'a str,
) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(words).size(SM))
            .padding([5, 9])
            .style(theme::card(p)),
        tooltip::Position::Bottom,
    )
    .gap(6)
    .into()
}

/// A friend's recent transfers: direction, file, size, result, when.
fn activity_table<'a>(p: &'static Pal, rows: &[&'a Entry], now: u64) -> Element<'a, Message> {
    let header = row![
        space::horizontal().width(FillPortion(1)),
        column_head(p, "FILE", 6),
        column_head(p, "SIZE", 2),
        column_head(p, "RESULT", 3),
        column_head(p, "WHEN", 3),
    ]
    .spacing(12);
    let lines = rows.iter().map(|e| {
        let (glyph, tint) = match e.direction {
            Direction::Sent => (icons::UPLOAD, p.accent),
            Direction::Received => (icons::DOWNLOAD, p.success),
        };
        let (rtint, rglyph) = match e.outcome {
            Outcome::Done => (p.success, icons::CHECK),
            Outcome::Declined => (p.warning, icons::CLOSE),
            Outcome::Cancelled => (p.faint, icons::CLOSE),
            Outcome::Failed => (p.danger, icons::WARNING),
        };
        let mut file = column![text(&e.file).size(MD)];
        if let Some(note) = &e.note {
            file = file.push(text(note).size(XS).color(p.faint));
        }
        row![
            // A fixed tile, centred in its column (not stretched to it).
            container(
                container(center(icon(glyph, 12)))
                    .width(26)
                    .height(26)
                    .style(theme::icon_tile(p, tint))
            )
            .center_x(FillPortion(1)),
            file.width(FillPortion(6)),
            text(format_bytes(e.size))
                .size(MD)
                .color(p.subtext)
                .width(FillPortion(2)),
            container(chip(
                p,
                rtint,
                rglyph,
                history::outcome_word(e.outcome, e.direction)
            ))
            .width(FillPortion(3)),
            text(history::ago(e.at, now))
                .size(MD)
                .color(p.subtext)
                .width(FillPortion(3)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    });
    column![
        container(header).padding(Padding::ZERO.bottom(8.0)),
        column(lines).spacing(10)
    ]
    .into()
}

fn column_head<'a>(p: &'static Pal, name: &'a str, share: u16) -> Element<'a, Message> {
    text(name)
        .size(XS)
        .font(BOLD)
        .color(p.faint)
        .width(FillPortion(share))
        .into()
}

/// A dialog: title bar with an icon and a close button, body, and a footer
/// strip holding the buttons, the safe one on the right and highlighted.
fn dialog<'a>(
    p: &'static Pal,
    glyph: char,
    title: &'a str,
    body: impl Into<Element<'a, Message>>,
    footer: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(column![
        container(
            row![
                icon(glyph, 14).color(p.accent),
                text(title).font(SEMIBOLD).size(LG).width(Fill),
                button(icon(icons::CLOSE, 10))
                    .padding(8)
                    .style(theme::ghost(p))
                    .on_press(Message::CloseDialog),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(Padding::new(8.0).left(20.0)),
        rule::horizontal(1).style(move |_| rule::Style {
            color: p.divider,
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        }),
        container(body).padding([20, 22]),
        container(footer)
            .padding([12, 22])
            .width(Fill)
            .style(theme::dialog_footer(p)),
    ])
    .width(500)
    .style(theme::dialog(p))
    .into()
}

fn rename_dialog<'a>(
    p: &'static Pal,
    from: &'a str,
    to: &'a str,
    error: Option<&'a str>,
) -> Element<'a, Message> {
    let mut body = column![
        text(format!("A new name for {from} on this device")).size(MD),
        text_input("Name", to)
            .id(crate::app::RENAME_INPUT)
            .on_input(Message::RenameText)
            .on_submit(Message::ConfirmRename)
            .padding(10)
            .size(MD)
            .style(theme::input(p)),
    ]
    .spacing(10);
    body = body.push(match error {
        Some(why) => notice(p, p.danger, icons::WARNING, why),
        None => text("Names are local nicknames: your friend never sees this one.")
            .size(SM)
            .color(p.subtext)
            .into(),
    });
    dialog(
        p,
        icons::EDIT,
        "Rename friend",
        body,
        row![
            space::horizontal(),
            button(text("Cancel").size(MD))
                .padding([7, 18])
                .style(theme::secondary(p))
                .on_press(Message::CloseDialog),
            button(text("Rename").font(SEMIBOLD).size(MD))
                .padding([7, 18])
                .style(theme::primary(p))
                .on_press_maybe((!to.trim().is_empty()).then_some(Message::ConfirmRename)),
        ]
        .spacing(8),
    )
}

/// Remove a friend? Keep is the highlighted, safe answer (`Ask` starts on
/// No), and Enter or Escape keep them; only a click on Remove removes.
fn remove_dialog<'a>(p: &'static Pal, name: &'a str, fingerprint: &'a str) -> Element<'a, Message> {
    dialog(
        p,
        icons::DELETE,
        "Remove friend",
        column![
            row![
                avatar(p, name, theme::tint_for(fingerprint), 40.0, None),
                column![
                    text(format!("Remove {name}?")).font(SEMIBOLD).size(LG),
                    text(format!("SHA256:{}", short(fingerprint)))
                        .font(MONO)
                        .size(SM)
                        .color(p.subtext),
                ]
                .spacing(2),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            text(
                "beam forgets their key: neither of you can send to the other \
                 until you pair again. Your history with them is kept."
            )
            .size(MD)
            .color(p.subtext),
        ]
        .spacing(14),
        row![
            space::horizontal(),
            button(text("Remove").size(MD))
                .padding([7, 18])
                .style(theme::danger(p))
                .on_press(Message::AnswerRemove(Choice::Yes)),
            button(text("Keep").font(SEMIBOLD).size(MD))
                .padding([7, 18])
                .style(theme::primary(p))
                .on_press(Message::AnswerRemove(Choice::No)),
        ]
        .spacing(8),
    )
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

// ---------------------------------------------------------------------------
// Add a friend (GUI EX Patch #4)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum StepState {
    Done,
    Current,
    Todo,
}

/// Invite → Code → Verify, for the person who pasted an invite.
fn stepper<'a>(p: &'static Pal, at: usize) -> Element<'a, Message> {
    let state = |n: usize| match n.cmp(&at) {
        std::cmp::Ordering::Less => StepState::Done,
        std::cmp::Ordering::Equal => StepState::Current,
        std::cmp::Ordering::Greater => StepState::Todo,
    };
    let dot = |n: usize, label: &'a str| {
        let s = state(n);
        let (bg, fg) = match s {
            StepState::Done => (p.success, Color::WHITE),
            StepState::Current => (p.accent, p.on_accent),
            StepState::Todo => (p.raised, p.faint),
        };
        let mark: Element<'a, Message> = if s == StepState::Done {
            icon(icons::CHECK, 11).color(fg).into()
        } else {
            text((n + 1).to_string())
                .font(BOLD)
                .size(SM)
                .color(fg)
                .line_height(1.0)
                .into()
        };
        row![
            container(center(mark))
                .width(24)
                .height(24)
                .style(move |_: &iced::Theme| container::Style {
                    background: Some(bg.into()),
                    border: iced::Border {
                        color: if s == StepState::Todo { p.border } else { bg },
                        width: 1.5,
                        radius: 999.0.into(),
                    },
                    ..container::Style::default()
                }),
            text(label).size(MD).color(if s == StepState::Todo {
                p.faint
            } else {
                p.text
            }),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    };
    let line = || container(divider(p)).width(Fill);
    row![
        dot(0, "Invite"),
        line(),
        dot(1, "Code"),
        line(),
        dot(2, "Verify")
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn divider<'a>(p: &'static Pal) -> Element<'a, Message> {
    rule::horizontal(1)
        .style(move |_| rule::Style {
            color: p.divider,
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        })
        .into()
}

fn cancel<'a>(p: &'static Pal, label: &'a str) -> Element<'a, Message> {
    button(text(label).size(MD))
        .padding([7, 18])
        .style(theme::secondary(p))
        .on_press(Message::CloseDialog)
        .into()
}

fn pairing_dialog<'a>(gui: &'a Gui, state: &'a Pairing) -> Element<'a, Message> {
    let p = gui.pal();
    let pair = Message::Pair;
    match state {
        Pairing::Start {
            invite,
            name,
            error,
        } => {
            let mut body = column![
                stepper(p, 0),
                text("Paste the invite your friend sent you")
                    .font(SEMIBOLD)
                    .size(LG),
                text("They find it under Show my invite in beam, or with beam whoami in a terminal. It starts with beam1.")
                    .size(MD)
                    .color(p.subtext),
                text_input("beam1…", invite)
                    .id(crate::app::pairing::INVITE_INPUT)
                    .on_input(move |t| pair(PairMsg::Invite(t)))
                    .on_submit(pair(PairMsg::Join))
                    .padding(10)
                    .font(MONO)
                    .size(MD)
                    .style(theme::input(p)),
                text("What to call them on this device").size(SM).color(p.subtext),
                text_input("A name, like alice", name)
                    .on_input(move |t| pair(PairMsg::Name(t)))
                    .on_submit(pair(PairMsg::Join))
                    .padding(10)
                    .size(MD)
                    .style(theme::input(p)),
            ]
            .spacing(10);
            body = body.push(match error {
                Some(why) => notice(p, p.danger, icons::WARNING, why),
                None => notice(
                    p,
                    p.info,
                    icons::INFO,
                    "An invite only says where to find them. The pairing code and the fingerprint check prove who they are.",
                ),
            });
            let ready = !invite.trim().is_empty() && !name.trim().is_empty();
            dialog(
                p,
                icons::ADD,
                "Add a friend",
                body,
                row![
                    button(
                        row![
                            icon(icons::KEY, 12),
                            text("Show my invite instead").size(MD)
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                    )
                    .padding([7, 12])
                    .style(theme::ghost(p))
                    .on_press(pair(PairMsg::ShowMine)),
                    space::horizontal(),
                    cancel(p, "Cancel"),
                    button(text("Next").font(SEMIBOLD).size(MD))
                        .padding([7, 18])
                        .style(theme::primary(p))
                        .on_press_maybe(ready.then_some(pair(PairMsg::Join))),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
        }
        Pairing::Showing {
            invite,
            code,
            until,
        } => {
            let left = Gui::code_left(*until).as_secs();
            dialog(
                p,
                icons::KEY,
                "Show my invite",
                column![
                    text("Send this invite to your friend")
                        .font(SEMIBOLD)
                        .size(LG),
                    row![
                        container(
                            text(invite)
                                .font(MONO)
                                .size(SM)
                                .wrapping(Wrapping::WordOrGlyph)
                        )
                        .padding([8, 10])
                        .width(Fill)
                        .style(theme::code_block(p)),
                        button(
                            row![icon(icons::COPY, 12), text("Copy").size(SM)]
                                .spacing(6)
                                .align_y(Alignment::Center)
                        )
                        .padding([6, 10])
                        .style(theme::secondary(p))
                        .on_press(pair(PairMsg::CopyInvite(invite.clone()))),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    text("On their device they choose Add friend, paste it, and type this code:")
                        .size(MD)
                        .color(p.subtext),
                    container(text(code).font(MONO).size(34))
                        .padding([8, 18])
                        .style(theme::code_block(p)),
                    text(format!(
                        "Works for one attempt. Expires in {} min {:02} s.",
                        left / 60,
                        left % 60
                    ))
                    .size(SM)
                    .color(p.subtext),
                ]
                .spacing(12),
                row![space::horizontal(), cancel(p, "Cancel")],
            )
        }
        Pairing::Code { code, error } => {
            let mut body = column![
                stepper(p, 1),
                text("Type the pairing code").font(SEMIBOLD).size(LG),
                text("Your friend's beam shows a 6-digit code next to the invite. It works for one attempt.")
                    .size(MD)
                    .color(p.subtext),
                text_input("000000", code)
                    .id(crate::app::pairing::CODE_INPUT)
                    .on_input(move |t| pair(PairMsg::Code(t)))
                    .on_submit(pair(PairMsg::SendCode))
                    .padding(12)
                    .font(MONO)
                    .size(26)
                    .width(220)
                    .style(theme::input(p)),
            ]
            .spacing(12);
            if let Some(why) = error {
                body = body.push(notice(p, p.danger, icons::WARNING, why));
            }
            dialog(
                p,
                icons::ADD,
                "Add a friend",
                body,
                row![
                    space::horizontal(),
                    cancel(p, "Cancel"),
                    button(text("Next").font(SEMIBOLD).size(MD))
                        .padding([7, 18])
                        .style(theme::primary(p))
                        .on_press_maybe((code.len() == 6).then_some(pair(PairMsg::SendCode))),
                ]
                .spacing(8),
            )
        }
        Pairing::Busy(what) => dialog(
            p,
            icons::ADD,
            "Add a friend",
            row![icon(icons::CLOCK, 16).color(p.accent), text(what).size(MD)]
                .spacing(12)
                .align_y(Alignment::Center),
            row![space::horizontal(), cancel(p, "Cancel")],
        ),
        Pairing::Confirm { ask, compared } => {
            let q = ask.question();
            dialog(
                p,
                icons::SHIELD,
                "Add a friend",
                column![
                    stepper(p, 2),
                    text(format!("Compare fingerprints with {}", q.name))
                        .font(SEMIBOLD)
                        .size(LG),
                    text("Read them to each other on a call or in person. Both must match exactly, on both devices.")
                        .size(MD)
                        .color(p.subtext),
                    fingerprint_card(p, &format!("{} (their device)", q.name), &q.peer),
                    fingerprint_card(p, "This device", &q.own),
                    checkbox(*compared)
                        .label("We compared both fingerprints and they match")
                        .on_toggle(move |yes| pair(PairMsg::Compared(yes)))
                        .size(16)
                        .text_size(MD),
                ]
                .spacing(12),
                row![
                    space::horizontal(),
                    button(text("Pair").size(MD))
                        .padding([7, 18])
                        .style(theme::secondary(p))
                        .on_press_maybe(compared.then_some(pair(PairMsg::Answer(Choice::Yes)))),
                    // The safe answer, highlighted: Enter and Escape choose it.
                    button(text("Don't pair").font(SEMIBOLD).size(MD))
                        .padding([7, 18])
                        .style(theme::primary(p))
                        .on_press(pair(PairMsg::Answer(Choice::No))),
                ]
                .spacing(8),
            )
        }
        Pairing::Failed(why) => dialog(
            p,
            icons::WARNING,
            "Not paired",
            notice(p, p.danger, icons::WARNING, why),
            row![
                space::horizontal(),
                cancel(p, "Close"),
                button(text("Try again").font(SEMIBOLD).size(MD))
                    .padding([7, 18])
                    .style(theme::primary(p))
                    .on_press(Message::AddFriend),
            ]
            .spacing(8),
        ),
    }
}

fn fingerprint_card<'a>(p: &'static Pal, whose: &str, hex: &str) -> Element<'a, Message> {
    column![
        text(whose.to_string()).size(SM).color(p.subtext),
        container(text(format!("SHA256:{}", grouped(hex))).font(MONO).size(MD))
            .padding([10, 12])
            .width(Fill)
            .style(theme::code_block(p)),
    ]
    .spacing(4)
    .into()
}

/// An invite would move a paired friend to another relay. Keep is the
/// highlighted, safe answer (ADR-0038).
fn relay_dialog<'a>(p: &'static Pal, q: &'a RelayMove) -> Element<'a, Message> {
    dialog(
        p,
        icons::GLOBE,
        "Move to another relay?",
        column![
            text(format!("{} is already your friend.", q.name))
                .font(SEMIBOLD)
                .size(LG),
            text("This invite would change only where to find them; their key stays. But it moves them to another relay:")
                .size(MD)
                .color(p.subtext),
            row![
                text("Now").size(SM).color(p.subtext).width(70),
                text(&q.old).font(MONO).size(SM)
            ]
            .spacing(8),
            row![
                text("After").size(SM).color(p.subtext).width(70),
                text(&q.new).font(MONO).size(SM)
            ]
            .spacing(8),
            notice(
                p,
                p.warning,
                icons::WARNING,
                "Whoever runs a relay sees when you send to this friend, and from where, and can block it. Say yes only if your friend told you they changed relay.",
            ),
        ]
        .spacing(10),
        row![
            space::horizontal(),
            button(text("Move").size(MD))
                .padding([7, 18])
                .style(theme::secondary(p))
                .on_press(Message::Pair(PairMsg::AnswerRelay(Choice::Yes))),
            button(text("Keep the current relay").font(SEMIBOLD).size(MD))
                .padding([7, 18])
                .style(theme::primary(p))
                .on_press(Message::Pair(PairMsg::AnswerRelay(Choice::No))),
        ]
        .spacing(8),
    )
}

// ---------------------------------------------------------------------------
// Receiving (GUI EX Patch #5)
// ---------------------------------------------------------------------------

fn inbox_page<'a>(gui: &'a Gui, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let p = gui.pal();
    let r = &gui.requests;
    let elsewhere = r.receiving_elsewhere(snapshot);
    let on = matches!(r.switch, Switch::On | Switch::Starting);
    let (title, words, tint) = match (&r.switch, elsewhere) {
        (Switch::On, _) => (
            "Receiving is on",
            "Friends can send to you while this window is open. You answer every file; unanswered ones count as declined after 5 minutes.".to_string(),
            p.success,
        ),
        (Switch::Starting, _) => ("Starting…", "Getting ready to receive.".to_string(), p.info),
        (Switch::Stopping, _) => (
            "Stopping…",
            "A sender in the middle of a file is told; what arrived is kept for a resume.".to_string(),
            p.warning,
        ),
        (Switch::Failed(why), _) => ("Receiving stopped", why.clone(), p.danger),
        (Switch::Off, Some(Taken::Agent)) => (
            "The background agent receives",
            "It runs without this window. Its requests show up here, and you answer them here.".to_string(),
            p.success,
        ),
        (Switch::Off, Some(Taken::OtherView)) => (
            "Another beam window receives",
            "Its requests show up here too.".to_string(),
            p.info,
        ),
        (Switch::Off, Some(Taken::Listen)) => (
            "beam listen receives in a terminal",
            "Answer its requests there.".to_string(),
            p.info,
        ),
        (Switch::Off, None) => (
            "Receiving is off",
            "Turn it on to let friends send to you while this window is open, without a terminal.".to_string(),
            p.warning,
        ),
    };
    let mut about = column![
        text(title).font(BOLD).size(LG),
        text(words).size(MD).color(p.subtext)
    ]
    .spacing(4)
    .width(Fill);
    if let Some(Agent::Running { receive_dir }) = Some(&snapshot.agent) {
        about = about.push(
            row![
                icon(icons::FOLDER, 12).color(p.subtext),
                text(format!("Saving to {receive_dir}"))
                    .size(SM)
                    .color(p.subtext)
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }
    let banner = container(
        row![
            container(center(icon(icons::DOWNLOAD, 18)))
                .width(40)
                .height(40)
                .style(theme::icon_tile(p, tint)),
            about,
            tip(
                p,
                toggler(on)
                    .on_toggle(|_| Message::Receive(RecvMsg::Toggle))
                    .size(22)
                    .style(theme::switch(p)),
                "The Receiving switch: receive while this window is open",
            ),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .padding([14, 16])
    .width(Fill)
    .style(theme::notice(p, tint));

    let mut body = column![banner].spacing(16);
    if let Some(arriving) = &r.receiving {
        let share = if arriving.total == 0 {
            0.0
        } else {
            arriving.done as f32 / arriving.total as f32 * 100.0
        };
        body = body.push(section(
            p,
            "Arriving now",
            None,
            column![
                row![
                    text(format!(
                        "{} of {}",
                        format_bytes(arriving.done),
                        format_bytes(arriving.total)
                    ))
                    .size(MD)
                    .width(Fill),
                    if arriving.relay {
                        chip(p, p.warning, icons::GLOBE, "Relay")
                    } else {
                        chip(p, p.info, icons::LINK, "Direct P2P")
                    },
                    text(format!("{share:.0}%")).font(BOLD).size(LG),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                progress_bar(0.0..=100.0, share)
                    .girth(6)
                    .style(theme::progress(p, p.success)),
            ]
            .spacing(10),
        ));
    }
    let list: Element<'_, Message> = if r.waiting.is_empty() {
        empty(
            p,
            icons::INBOX,
            "Nothing waiting",
            "When a friend sends you a file, it waits here for your answer. It never opens by itself.",
        )
    } else {
        let rows = r.waiting.iter().map(|w| {
            let q = &w.request;
            let urgent = w
                .expires
                .saturating_duration_since(std::time::Instant::now())
                < std::time::Duration::from_secs(60);
            let mut file = column![
                text(&q.file_name).size(MD),
                text(format_bytes(q.size)).size(XS).color(p.faint)
            ];
            if let Some(resume) = &q.resume {
                file = file.push(text(resume).size(XS).color(p.faint));
            }
            row![
                row![
                    avatar(p, &q.peer_name, theme::tint_for(&q.fingerprint), 28.0, None),
                    column![
                        text(&q.peer_name).font(SEMIBOLD).size(MD),
                        text(format!("SHA256:{}", short(&q.fingerprint)))
                            .font(MONO)
                            .size(XS)
                            .color(p.faint)
                    ]
                ]
                .spacing(10)
                .align_y(Alignment::Center)
                .width(FillPortion(3)),
                file.width(FillPortion(4)),
                text(crate::app::receiving::left(w.expires))
                    .font(MONO)
                    .size(MD)
                    .color(if urgent { p.danger } else { p.subtext })
                    .width(FillPortion(1)),
                container(
                    button(text("Review").font(SEMIBOLD).size(MD))
                        .padding([5, 14])
                        .style(theme::secondary(p))
                        .on_press(Message::Receive(RecvMsg::Review(w.id)))
                )
                .align_right(FillPortion(2)),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        });
        column![
            row![
                column_head(p, "FROM", 3),
                column_head(p, "FILE", 4),
                column_head(p, "LEFT", 1),
                space::horizontal().width(FillPortion(2)),
            ]
            .spacing(12),
            column(rows).spacing(12),
        ]
        .spacing(10)
        .into()
    };
    body = body.push(section(
        p,
        format!("Waiting ({})", r.waiting.len()),
        None,
        list,
    ));
    column![
        toolbar(
            p,
            page_title(
                p,
                "Inbox",
                "Files your friends want to send you. Every one waits for your answer."
            )
        ),
        scrollable(body.padding(Padding::new(24.0).top(18.0))).height(Fill),
    ]
    .into()
}

/// May this friend send this file? Decline is highlighted: Enter declines,
/// Escape decides later, and only a click on Accept accepts (rule 1).
fn accept_dialog<'a>(gui: &'a Gui, q: &'a Incoming) -> Element<'a, Message> {
    let p = gui.pal();
    let r = &q.request;
    let saved_to = match &gui.snapshot.as_ref().map(|s| &s.agent) {
        Some(Agent::Running { receive_dir }) => format!("  ·  saved to {receive_dir}"),
        _ => String::new(),
    };
    let mut file = column![
        text(&r.file_name).font(SEMIBOLD).size(LG),
        text(format!("{}{saved_to}", format_bytes(r.size)))
            .size(SM)
            .color(p.subtext),
    ]
    .spacing(2);
    if let Some(resume) = &r.resume {
        file = file.push(text(resume).size(SM).color(p.info));
    }
    dialog(
        p,
        icons::DOWNLOAD,
        "Incoming file",
        column![
            row![
                avatar(p, &r.peer_name, theme::tint_for(&r.fingerprint), 44.0, None),
                column![
                    text(&r.peer_name).font(BOLD).size(XL),
                    text("wants to send you a file").size(MD).color(p.subtext),
                ]
                .spacing(2)
                .width(Fill),
                chip(
                    p,
                    p.subtext,
                    icons::CLOCK,
                    crate::app::receiving::left(q.expires)
                ),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
            container(
                row![
                    container(center(icon(icons::DOCUMENT, 18)))
                        .width(40)
                        .height(40)
                        .style(theme::icon_tile(p, p.accent)),
                    file,
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .padding(12)
            .width(Fill)
            .style(theme::card(p)),
            column![
                row![
                    icon(icons::SHIELD, 12).color(p.success),
                    text("The key saved when you paired; beam checked it on this connection")
                        .size(SM)
                        .color(p.success)
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                container(
                    text(format!("SHA256:{}", grouped(&r.fingerprint)))
                        .font(MONO)
                        .size(SM)
                )
                .padding([8, 10])
                .width(Fill)
                .style(theme::code_block(p)),
            ]
            .spacing(6),
            text("Only accept files you are expecting. Enter declines; Escape decides later.")
                .size(SM)
                .color(p.subtext),
        ]
        .spacing(16),
        row![
            space::horizontal(),
            button(text("Accept").size(MD))
                .padding([7, 18])
                .style(theme::secondary(p))
                .on_press(Message::Receive(RecvMsg::Answer(Choice::Yes))),
            button(text("Decline").font(SEMIBOLD).size(MD))
                .padding([7, 18])
                .style(theme::primary(p))
                .on_press(Message::Receive(RecvMsg::Answer(Choice::No))),
        ]
        .spacing(8),
    )
}

/// A file is arriving: stop anyway? Keep receiving is the highlighted answer.
fn stop_dialog<'a>(p: &'static Pal) -> Element<'a, Message> {
    dialog(
        p,
        icons::WARNING,
        "Stop receiving?",
        text("A file is arriving. Stopping tells the sender; what arrived so far is kept, and they can send again to resume.")
            .size(MD),
        row![
            space::horizontal(),
            button(text("Stop").size(MD))
                .padding([7, 18])
                .style(theme::danger(p))
                .on_press(Message::Receive(RecvMsg::AnswerStop(Choice::Yes))),
            button(text("Keep receiving").font(SEMIBOLD).size(MD))
                .padding([7, 18])
                .style(theme::primary(p))
                .on_press(Message::Receive(RecvMsg::AnswerStop(Choice::No))),
        ]
        .spacing(8),
    )
}

/// A file is arriving: close beam anyway? Stay is the highlighted answer.
fn leave_dialog<'a>(p: &'static Pal) -> Element<'a, Message> {
    dialog(
        p,
        icons::WARNING,
        "Close beam?",
        text("A file is still arriving. Closing stops it; the sender is told, what arrived is kept, and they can send again to resume.")
            .size(MD),
        row![
            space::horizontal(),
            button(text("Close anyway").size(MD))
                .padding([7, 18])
                .style(theme::danger(p))
                .on_press(Message::Receive(RecvMsg::AnswerLeave(Choice::Yes))),
            button(text("Stay").font(SEMIBOLD).size(MD))
                .padding([7, 18])
                .style(theme::primary(p))
                .on_press(Message::Receive(RecvMsg::AnswerLeave(Choice::No))),
        ]
        .spacing(8),
    )
}

/// On a friend's page: their files waiting for an answer, with the way to
/// answer. Nothing opens by itself.
fn waiting_from<'a>(gui: &'a Gui, f: &'a Friend) -> Element<'a, Message> {
    let p = gui.pal();
    let theirs = gui.requests.from(&f.fingerprint);
    let Some(first) = theirs.first() else {
        return space::vertical().height(0).into();
    };
    let words = match theirs.len() {
        1 => format!("{} wants to send you {}.", f.name, first.request.file_name),
        n => format!("{} wants to send you {n} files.", f.name),
    };
    container(
        row![
            icon(icons::INBOX, 14).color(p.info),
            text(words).size(MD).width(Fill),
            button(text("Review").font(SEMIBOLD).size(MD))
                .padding([5, 14])
                .style(theme::primary(p))
                .on_press(Message::Receive(RecvMsg::Review(first.id))),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([10, 14])
    .width(Fill)
    .style(theme::notice(p, p.info))
    .into()
}

// ---------------------------------------------------------------------------
// Sending (GUI EX Patch #6)
// ---------------------------------------------------------------------------

fn transfers_page<'a>(gui: &'a Gui, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let p = gui.pal();
    let send_to = gui
        .selected
        .clone()
        .filter(|name| snapshot.friends.iter().any(|f| &f.name == name));
    let action = button(
        row![icon(icons::SEND, 13), text("Send a file").font(SEMIBOLD)]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([8, 16])
    .style(theme::primary(p))
    .on_press_maybe(send_to.clone().map(|f| Message::Send(SendMsg::Pick(f))));
    let subtitle = match &send_to {
        Some(friend) => format!(
            "Files going out. Send a file goes to {friend}; choose another friend on the Friends page."
        ),
        None => "Files going out. Choose a friend on the Friends page first.".to_string(),
    };
    let now: Element<'_, Message> = match &gui.sending {
        Some(s) => sending_card(p, s),
        None => empty(
            p,
            icons::TRANSFERS,
            "Nothing going out",
            "Send a file from a friend's page, or drop a file on it. They answer before anything moves.",
        ),
    };
    let sent: Vec<&Entry> = snapshot
        .history
        .iter()
        .rev()
        .filter(|e| e.direction == Direction::Sent)
        .take(5)
        .collect();
    let recent = if sent.is_empty() {
        empty(
            p,
            icons::HISTORY,
            "Nothing sent yet",
            "Files you send are listed here.",
        )
    } else {
        activity_table(p, &sent, history::unix_now())
    };
    column![
        toolbar(
            p,
            row![
                column![
                    text("Transfers").font(BOLD).size(XXL),
                    text(subtitle).size(MD).color(p.subtext)
                ]
                .spacing(4)
                .width(Fill),
                action,
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        ),
        scrollable(
            column![
                section(p, "Now", None, now),
                section(p, "Sent recently", None, recent)
            ]
            .spacing(16)
            .padding(Padding::new(24.0).top(18.0)),
        )
        .height(Fill),
    ]
    .into()
}

/// The send in progress, in the stages a person follows, or how it ended.
fn sending_card<'a>(p: &'static Pal, s: &'a Sending) -> Element<'a, Message> {
    let percent = |done: u64, total: u64| {
        if total == 0 {
            0.0
        } else {
            done as f32 / total as f32 * 100.0
        }
    };
    let (what, share, path): (String, Option<f32>, Option<bool>) = match s.stage {
        Stage::Starting => ("Starting…".into(), None, None),
        Stage::Dialling => (format!("Looking for {}…", s.peer), None, None),
        Stage::Hashing { done, total } => (
            format!(
                "Reading the file: {} of {}",
                format_bytes(done),
                format_bytes(total)
            ),
            Some(percent(done, total)),
            None,
        ),
        Stage::Waiting => (
            format!(
                "Waiting for {} to accept. Nothing is sent before they do.",
                s.peer
            ),
            None,
            None,
        ),
        Stage::Moving { done, total, relay } => (
            format!("Sending: {} of {}", format_bytes(done), format_bytes(total)),
            Some(percent(done, total)),
            Some(relay),
        ),
        Stage::Checking { done, total } => (
            format!("{} is checking the whole file", s.peer),
            Some(percent(done, total)),
            None,
        ),
    };
    let mut head = row![
        container(center(icon(icons::UPLOAD, 16)))
            .width(38)
            .height(38)
            .style(theme::icon_tile(p, p.accent)),
        column![
            text(&s.file).font(SEMIBOLD).size(LG),
            text(format!("to {}", s.peer)).size(SM).color(p.subtext),
        ]
        .spacing(2)
        .width(Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if let Some(relay) = path {
        head = head.push(if relay {
            chip(p, p.warning, icons::GLOBE, "Relay")
        } else {
            chip(p, p.info, icons::LINK, "Direct P2P")
        });
    }
    let mut card = column![].spacing(12);
    match &s.result {
        None => {
            head = head.push(
                button(
                    row![icon(icons::CLOSE, 11), text("Stop").size(MD)]
                        .spacing(6)
                        .align_y(Alignment::Center),
                )
                .padding([6, 12])
                .style(theme::secondary(p))
                .on_press(Message::Send(SendMsg::Cancel)),
            );
            card = card.push(head).push(text(what).size(MD).color(p.subtext));
            if let Some(share) = share {
                card = card.push(
                    progress_bar(0.0..=100.0, share)
                        .girth(6)
                        .style(theme::progress(p, p.accent)),
                );
            }
        }
        Some(result) => {
            head = head.push(
                button(text("Dismiss").size(MD))
                    .padding([6, 12])
                    .style(theme::ghost(p))
                    .on_press(Message::Send(SendMsg::Dismiss)),
            );
            let (tint, glyph, words) = match result {
                Ok(words) => (p.success, icons::CHECK, words),
                Err(words) => (p.danger, icons::WARNING, words),
            };
            card = card.push(head).push(notice(p, tint, glyph, words));
        }
    }
    card.into()
}

/// A file was dropped on a friend's page. A drop may be an accident, so the
/// highlighted answer sends nothing; Enter and Escape choose it too.
fn drop_dialog<'a>(
    p: &'static Pal,
    friend: &'a str,
    path: &'a std::path::Path,
) -> Element<'a, Message> {
    let file = path
        .file_name()
        .map(|n| beam_core::untrusted::name(&n.to_string_lossy()))
        .unwrap_or_default();
    dialog(
        p,
        icons::SEND,
        "Send this file?",
        column![
            text(format!("Send {file} to {friend}?"))
                .font(SEMIBOLD)
                .size(LG),
            text("They are asked first: nothing moves until they accept.")
                .size(MD)
                .color(p.subtext),
        ]
        .spacing(8),
        row![
            space::horizontal(),
            button(text("Send").size(MD))
                .padding([7, 18])
                .style(theme::secondary(p))
                .on_press(Message::Send(SendMsg::AnswerDrop(true))),
            button(text("Don't send").font(SEMIBOLD).size(MD))
                .padding([7, 18])
                .style(theme::primary(p))
                .on_press(Message::Send(SendMsg::AnswerDrop(false))),
        ]
        .spacing(8),
    )
}

// ---------------------------------------------------------------------------
// History and settings (GUI EX Patch #7)
// ---------------------------------------------------------------------------

fn history_page<'a>(gui: &'a Gui, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let p = gui.pal();
    let segment = |label: &'static str, filter: HistoryFilter| {
        button(text(label).size(MD))
            .padding([5, 12])
            .style(theme::segment(p, gui.history_filter == filter))
            .on_press(Message::Settings(SettingsMsg::Filter(filter)))
    };
    let controls = row![
        container(
            row![
                segment("All", HistoryFilter::All),
                segment("Sent", HistoryFilter::Sent),
                segment("Received", HistoryFilter::Received),
                segment("Declined", HistoryFilter::Declined),
                segment("Problems", HistoryFilter::Problems),
            ]
            .spacing(2)
        )
        .padding(3)
        .style(theme::segments(p)),
        space::horizontal(),
        text_input("Filter by file or friend", &gui.history_search)
            .on_input(|t| Message::Settings(SettingsMsg::Search(t)))
            .icon(text_input::Icon {
                font: icons::FONT,
                code_point: icons::FILTER,
                size: Some(14.into()),
                spacing: 8.0,
                side: text_input::Side::Left,
            })
            .padding([7, 10])
            .size(MD)
            .width(260)
            .style(theme::input(p)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let needle = gui.history_search.to_lowercase();
    let rows: Vec<&Entry> = snapshot
        .history
        .iter()
        .rev()
        .filter(|e| match gui.history_filter {
            HistoryFilter::All => true,
            HistoryFilter::Sent => e.direction == Direction::Sent,
            HistoryFilter::Received => e.direction == Direction::Received,
            HistoryFilter::Declined => e.outcome == Outcome::Declined,
            HistoryFilter::Problems => matches!(e.outcome, Outcome::Failed | Outcome::Cancelled),
        })
        .filter(|e| {
            needle.is_empty()
                || e.file.to_lowercase().contains(&needle)
                || e.peer.to_lowercase().contains(&needle)
        })
        .collect();
    let shown = rows.len().min(200);
    let table = if rows.is_empty() {
        empty(
            p,
            icons::HISTORY,
            if snapshot.history.is_empty() {
                "Nothing yet"
            } else {
                "Nothing matches"
            },
            "Every file that comes or goes is listed here, newest first.",
        )
    } else {
        history_table(p, &rows[..shown], history::unix_now())
    };
    column![
        toolbar(
            p,
            row![
                column![
                    text("History").font(BOLD).size(XXL),
                    text("What came and went on this device. Kept only here, in your beam folder (the newest 1000).")
                        .size(MD)
                        .color(p.subtext)
                ]
                .spacing(4)
                .width(Fill),
                button(
                    row![icon(icons::DELETE, 12), text("Clear history").size(MD)]
                        .spacing(8)
                        .align_y(Alignment::Center)
                )
                .padding([7, 14])
                .style(theme::secondary(p))
                .on_press_maybe(
                    (!snapshot.history.is_empty())
                        .then_some(Message::Settings(SettingsMsg::ClearHistory))
                ),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        ),
        scrollable(
            column![
                controls,
                section(
                    p,
                    format!("{} of {}", rows.len(), snapshot.history.len()),
                    None,
                    table
                ),
            ]
            .spacing(14)
            .padding(Padding::new(24.0).top(18.0)),
        )
        .height(Fill),
    ]
    .into()
}

/// The whole history: direction, file, friend, size, result, when.
fn history_table<'a>(p: &'static Pal, rows: &[&'a Entry], now: u64) -> Element<'a, Message> {
    let header = row![
        space::horizontal().width(FillPortion(1)),
        column_head(p, "FILE", 5),
        column_head(p, "FRIEND", 3),
        column_head(p, "SIZE", 2),
        column_head(p, "RESULT", 3),
        column_head(p, "WHEN", 2),
    ]
    .spacing(12);
    let lines = rows.iter().map(|e| {
        let (glyph, tint) = match e.direction {
            Direction::Sent => (icons::UPLOAD, p.accent),
            Direction::Received => (icons::DOWNLOAD, p.success),
        };
        let (rtint, rglyph) = match e.outcome {
            Outcome::Done => (p.success, icons::CHECK),
            Outcome::Declined => (p.warning, icons::CLOSE),
            Outcome::Cancelled => (p.faint, icons::CLOSE),
            Outcome::Failed => (p.danger, icons::WARNING),
        };
        let mut file = column![text(&e.file).size(MD)];
        if let Some(note) = &e.note {
            file = file.push(text(note).size(XS).color(p.faint));
        }
        row![
            container(
                container(center(icon(glyph, 12)))
                    .width(26)
                    .height(26)
                    .style(theme::icon_tile(p, tint))
            )
            .center_x(FillPortion(1)),
            file.width(FillPortion(5)),
            row![
                avatar(p, &e.peer, theme::tint_for(&e.fingerprint), 22.0, None),
                text(&e.peer).size(MD)
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(FillPortion(3)),
            text(format_bytes(e.size))
                .size(MD)
                .color(p.subtext)
                .width(FillPortion(2)),
            container(chip(
                p,
                rtint,
                rglyph,
                history::outcome_word(e.outcome, e.direction)
            ))
            .width(FillPortion(3)),
            text(history::ago(e.at, now))
                .size(MD)
                .color(p.subtext)
                .width(FillPortion(2)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    });
    column![
        container(header).padding(Padding::ZERO.bottom(8.0)),
        column(lines).spacing(10)
    ]
    .into()
}

fn received_settings(gui: &Gui) -> Element<'_, Message> {
    let p = gui.pal();
    let chosen = gui.config.as_ref().and_then(|c| c.receive_dir.clone());
    let shown = match &chosen {
        Some(dir) => beam_core::untrusted::name(&dir.display().to_string()),
        None if cfg!(windows) => format!(
            "{} (your Downloads folder; the default)",
            beam_core::untrusted::name(
                &beam_core::agent::receive_dir(None).0.display().to_string()
            )
        ),
        None => "Not chosen: the background agent would save where it was started".into(),
    };
    settings_group(
        p,
        "Received files",
        vec![setting(
            p,
            "Save received files to",
            "Used by the background agent, the Receiving switch, and beam listen without --out.",
            column![
                container(text(shown).font(MONO).size(SM))
                    .padding([7, 10])
                    .max_width(380)
                    .style(theme::code_block(p)),
                row![
                    button(text("Browse…").size(MD))
                        .padding([6, 12])
                        .style(theme::secondary(p))
                        .on_press(Message::Settings(SettingsMsg::ChooseFolder)),
                    button(text("Use the default").size(MD))
                        .padding([6, 12])
                        .style(theme::ghost(p))
                        .on_press_maybe(
                            chosen
                                .is_some()
                                .then_some(Message::Settings(SettingsMsg::DefaultFolder))
                        ),
                ]
                .spacing(8),
            ]
            .spacing(8)
            .align_x(Alignment::End)
            .into(),
        )],
    )
}

fn network_settings(gui: &Gui) -> Element<'_, Message> {
    let p = gui.pal();
    let mapping = gui.config.as_ref().is_some_and(|c| c.agent_port_mapping);
    let port = gui
        .config
        .as_ref()
        .map_or_else(|| "unknown".into(), |c| c.port.to_string());
    column![
        settings_group(
            p,
            "Network",
            vec![
                setting(
                    p,
                    "Relay",
                    "Helps two devices find each other; files go direct whenever they can.",
                    text(gui.relay_label()).font(MONO).size(SM).into(),
                ),
                setting(
                    p,
                    "Listening port (UDP)",
                    "Fixed, so this device's invite stays the same.",
                    text(port).font(MONO).size(SM).into(),
                ),
                setting(
                    p,
                    "Router port mapping",
                    "Lets friends reach the background agent directly. It also makes this device findable by scanning.",
                    toggler(mapping)
                        .on_toggle(|on| Message::Settings(SettingsMsg::PortMapping(on)))
                        .size(20)
                        .style(theme::switch(p))
                        .into(),
                ),
            ],
        ),
        text("The relay and the port are changed in config.toml in your beam folder; they affect every friend.")
            .size(SM)
            .color(p.subtext),
    ]
    .spacing(8)
    .into()
}

fn service_settings<'a>(gui: &'a Gui, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let p = gui.pal();
    let beam = gui.beam_beside();
    let folder = gui.agent_folder();
    let running = matches!(snapshot.agent, Agent::Running { .. });
    let background = running && snapshot.owner == Owner::Background;
    let can_start = beam.is_some() && folder.is_some();
    let state = match (&snapshot.agent, snapshot.owner) {
        (Agent::Running { .. }, Owner::Background) => chip(p, p.success, icons::CHECK, "Running"),
        (Agent::Running { .. }, _) => chip(p, p.info, icons::INFO, "A beam window receives"),
        (Agent::Unreadable, _) => chip(p, p.faint, icons::WARNING, "Unknown"),
        (Agent::Stopped, _) => chip(p, p.faint, icons::CLOSE, "Not running"),
    };
    let mut rows = vec![
        setting(
            p,
            "Start at sign-in",
            "Runs beam's background agent for your user only (never as an administrator), so friends can send while this window is closed. Every file still waits for your answer.",
            toggler(gui.at_sign_in)
                .on_toggle_maybe(
                    (can_start || gui.at_sign_in)
                        .then_some(|on| Message::Settings(SettingsMsg::AtSignIn(on))),
                )
                .size(20)
                .style(theme::switch(p))
                .into(),
        ),
        setting(
            p,
            "Now",
            "Requests to the background agent show up in the Inbox.",
            row![
                state,
                button(text(if background { "Stop" } else { "Start" }).size(MD))
                    .padding([6, 12])
                    .style(theme::secondary(p))
                    .on_press_maybe(if background {
                        Some(Message::Settings(SettingsMsg::StopAgent))
                    } else if !running && can_start {
                        Some(Message::Settings(SettingsMsg::StartAgent))
                    } else {
                        None
                    }),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        ),
    ];
    let why = match (&beam, &folder) {
        (None, _) => Some(
            "The background agent is the beam program, which is not next to beam-gui here. Install them together to use it from this page.",
        ),
        (_, None) => Some("Choose where received files go first: the agent saves there."),
        _ => None,
    };
    if let Some(why) = why {
        rows.push(notice(p, p.info, icons::INFO, why));
    }
    settings_group(p, "Background service", rows)
}

/// Router port mapping on? "Keep it off" is highlighted (the CLI's y/N).
fn port_mapping_dialog<'a>(p: &'static Pal) -> Element<'a, Message> {
    dialog(
        p,
        icons::ROUTER,
        "Turn on router port mapping?",
        column![
            notice(
                p,
                p.warning,
                icons::WARNING,
                "Your router will forward a port to this computer while the background agent runs. Anyone scanning the internet can then find that beam runs here.",
            ),
            text("Paired friends can usually reach you without it, through the relay. Turn it on only if direct connections fail.")
                .size(MD)
                .color(p.subtext),
        ]
        .spacing(12),
        row![
            space::horizontal(),
            button(text("Turn on").size(MD))
                .padding([7, 18])
                .style(theme::secondary(p))
                .on_press(Message::Settings(SettingsMsg::AnswerPortMapping(Choice::Yes))),
            button(text("Keep it off").font(SEMIBOLD).size(MD))
                .padding([7, 18])
                .style(theme::primary(p))
                .on_press(Message::Settings(SettingsMsg::AnswerPortMapping(Choice::No))),
        ]
        .spacing(8),
    )
}

/// Clear the whole history? Keep is highlighted.
fn clear_dialog<'a>(p: &'static Pal) -> Element<'a, Message> {
    dialog(
        p,
        icons::DELETE,
        "Clear history?",
        text("Every line of what came and went on this device is deleted. Friends and files are not touched.")
            .size(MD),
        row![
            space::horizontal(),
            button(text("Clear").size(MD))
                .padding([7, 18])
                .style(theme::danger(p))
                .on_press(Message::Settings(SettingsMsg::AnswerClear(Choice::Yes))),
            button(text("Keep").font(SEMIBOLD).size(MD))
                .padding([7, 18])
                .style(theme::primary(p))
                .on_press(Message::Settings(SettingsMsg::AnswerClear(Choice::No))),
        ]
        .spacing(8),
    )
}
