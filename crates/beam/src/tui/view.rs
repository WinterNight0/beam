//! Drawing the state in [`super::app`], laid out like Discord's Friends page:
//! friends on the left, the selected friend's transfers in the middle, their
//! details on the right, and a status bar of keys along the bottom.
//!
//! Drawing also records where every clickable thing landed
//! ([`App::areas`]), which is how a mouse click finds what it hit.
//!
//! Every string drawn here was made terminal-safe when the snapshot was read
//! (`untrusted`, ADR-0034); ratatui also never passes control characters
//! through, but that is a second line, not the first.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};

use super::add::{Field, countdown};
use super::app::{Agent, App, Areas, Choice, Friend, Modal, Tab, Target};
use super::inbox::RequestView;
use super::input::Input;
use super::palette::{Place, Suggestion};
use super::pending::{Link, Switch, Taken};
use super::send::{FILE_ROWS, Sending, Stage};

/// Discord's blurple, for the selection and the active tab.
const ACCENT: Color = Color::Rgb(88, 101, 242);
/// Amber, for the Receiving switch while it is off: it should be noticed.
const NOTICE: Color = Color::Rgb(240, 178, 50);
/// Discord's "online" green.
const GOOD: Color = Color::Rgb(35, 165, 90);
const DANGER: Color = Color::Rgb(237, 66, 69);
const MUTED: Color = Color::Rgb(148, 155, 164);
const BORDER: Color = Color::Rgb(78, 80, 88);
/// The row under the cursor, like Discord's hover.
const SELECTED: Color = Color::Rgb(64, 68, 75);
/// The status bar's background.
const BAR: Color = Color::Rgb(43, 45, 49);
/// Avatar colours; a friend's is picked by their fingerprint, so it stays
/// the same across renames.
const AVATARS: [Color; 6] = [
    Color::Rgb(88, 101, 242),
    Color::Rgb(35, 165, 90),
    Color::Rgb(240, 178, 50),
    Color::Rgb(237, 66, 69),
    Color::Rgb(235, 69, 158),
    Color::Rgb(26, 188, 156),
];

/// Below this width the details panel folds into the middle one.
const WIDE: u16 = 90;
const LIST_WIDTH: u16 = 26;
const DETAILS_WIDTH: u16 = 32;
/// Lines per friend in the list: name, then Short ID.
const ROW_HEIGHT: u16 = 2;

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.areas = Areas::default();
    let [header, tabs, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app);
    draw_tabs(frame, tabs, app);
    match (app.snapshot.me.is_some(), app.tab) {
        (false, _) => draw_no_identity(frame, body),
        (true, Tab::Friends) => draw_friends(frame, body, app),
        (true, Tab::Pending) => draw_pending(frame, body, app),
        (true, Tab::AddFriend) => draw_add_friend(frame, body, app),
    }
    draw_footer(frame, footer, app);

    let area = frame.area();
    match app.modal.clone() {
        None => {}
        Some(Modal::Help) => draw_help(frame, area),
        Some(Modal::Rename { from, input, error }) => {
            draw_rename(frame, area, &mut app.areas, &from, &input, error.as_deref());
        }
        Some(Modal::Remove {
            name,
            fingerprint,
            focus,
        }) => draw_remove(frame, area, &mut app.areas, &name, &fingerprint, focus),
        Some(Modal::Output {
            title,
            text,
            scroll,
            failed,
        }) => {
            let scroll = draw_output(frame, area, &title, &text, scroll, failed);
            // Keep the scroll in range, so End then Up moves at once.
            if let Some(Modal::Output { scroll: kept, .. }) = &mut app.modal {
                *kept = scroll;
            }
        }
        Some(Modal::EnterCode { input, error }) => {
            draw_enter_code(frame, area, &mut app.areas, &input, error.as_deref());
        }
        Some(Modal::Busy { text }) => draw_busy(frame, area, &mut app.areas, &text),
        Some(Modal::ShowInvite {
            invite,
            code,
            expires,
            attempt,
        }) => draw_show_invite(
            frame,
            area,
            &mut app.areas,
            &invite,
            &code,
            expires,
            attempt.as_deref(),
        ),
        Some(Modal::ConfirmPair {
            name,
            peer,
            own,
            focus,
        }) => draw_confirm_pair(frame, area, &mut app.areas, &name, &peer, &own, focus),
        Some(Modal::Accept {
            request,
            expires,
            focus,
            ..
        }) => draw_accept(frame, area, &mut app.areas, &request, expires, focus),
        Some(Modal::PickFile {
            peer,
            input,
            error,
            selected,
        }) => {
            let paths = app.file_suggestions();
            draw_pick_file(
                frame,
                area,
                &mut app.areas,
                &peer,
                &input,
                error.as_deref(),
                &paths,
                selected,
            );
        }
        Some(Modal::SendStatus) => {
            if let Some(sending) = app.sending.clone() {
                draw_send_status(frame, area, &mut app.areas, &sending);
            }
        }
        Some(Modal::ConfirmQuit { focus }) => {
            let peer = app
                .sending
                .as_ref()
                .filter(|s| s.running())
                .map(|s| s.peer.clone());
            draw_confirm_quit(frame, area, &mut app.areas, peer.as_deref(), focus);
        }
        Some(Modal::ConfirmStop { focus }) => draw_confirm_stop(frame, area, &mut app.areas, focus),
        Some(Modal::RelayChange {
            name,
            fingerprint,
            old,
            new,
            focus,
            ..
        }) => draw_relay_change(
            frame,
            area,
            &mut app.areas,
            &name,
            &fingerprint,
            &old,
            &new,
            focus,
        ),
    }
    if app.palette.is_some() {
        draw_palette(frame, body, footer, app);
    }
}

/// Lines of suggestions shown at once.
const PALETTE_ROWS: usize = 8;

/// The palette: its text box in the status bar, and its list just above.
fn draw_palette(frame: &mut Frame, body: Rect, footer: Rect, app: &mut App) {
    let suggestions = app.suggestions();
    let Some(palette) = app.palette.as_mut() else {
        return;
    };
    palette.selected = palette.selected.min(suggestions.len().saturating_sub(1));
    let selected = palette.selected;
    let error = palette.error.clone();
    let input = palette.input.clone();

    // The text box, over the status bar: cleared first, or the keys drawn
    // there would show through after the typed text.
    frame.render_widget(Clear, footer);
    frame.render_widget(Block::new().style(Style::new().bg(BAR)), footer);
    let hint = Line::from(vec![
        key_chip("Enter"),
        Span::styled(" run  ", Style::new().fg(MUTED)),
        key_chip("Tab"),
        Span::styled(" complete  ", Style::new().fg(MUTED)),
        key_chip("Esc"),
        Span::styled(" close ", Style::new().fg(MUTED)),
    ]);
    let hint_width = (hint.width() as u16).min(footer.width / 2);
    let [prompt, field, hint_area] = Layout::horizontal([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(hint_width),
    ])
    .areas(footer);
    frame.render_widget(
        Span::styled(" : ", Style::new().bg(ACCENT).fg(Color::White).bold()),
        prompt,
    );
    let field = Rect {
        x: field.x + 1,
        width: field.width.saturating_sub(1),
        ..field
    };
    let scroll = input.scroll(field.width);
    frame.render_widget(
        Paragraph::new(input.text().to_string())
            .scroll((0, scroll))
            .style(Style::new().bg(BAR).fg(Color::White)),
        field,
    );
    if field.width > 0 {
        frame.set_cursor_position((field.x + input.cursor_column(field.width), field.y));
    }
    app.areas.add(field, Target::PaletteInput);
    frame.render_widget(hint.style(Style::new().bg(BAR)), hint_area);

    // The list, growing upwards from the status bar.
    let rows = suggestions.len().min(PALETTE_ROWS);
    let extra = u16::from(error.is_some()) + u16::from(rows == 0 && error.is_none());
    let height = (rows as u16 + extra + 2).min(body.height);
    if height < 3 {
        return;
    }
    let list = Rect {
        x: body.x + 1,
        y: body.bottom() - height,
        width: body.width.saturating_sub(2),
        height,
    };
    frame.render_widget(Clear, list);
    let block = popup_block(" Commands ").padding(Padding::ZERO);
    let inner = block.inner(list);
    frame.render_widget(block, list);
    let mut y = inner.y;
    if let Some(error) = &error {
        frame.render_widget(
            Line::from(vec![
                Span::styled(" ✗ ", Style::new().fg(DANGER).bold()),
                Span::styled(error.clone(), Style::new().fg(DANGER)),
            ]),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
    } else if rows == 0 {
        frame.render_widget(
            Line::from(" No command matches. Try `help`.".fg(MUTED)),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
    let offset = (selected + 1).saturating_sub(rows);
    for (index, suggestion) in suggestions.iter().enumerate().skip(offset).take(rows) {
        if y >= inner.bottom() {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, 1);
        draw_suggestion(frame, rect, suggestion, index == selected);
        app.areas.add(rect, Target::Suggestion(index));
        y += 1;
    }
}

fn draw_suggestion(frame: &mut Frame, rect: Rect, suggestion: &Suggestion, selected: bool) {
    let base = if selected {
        Style::new().bg(SELECTED)
    } else {
        Style::new()
    };
    let badge = suggestion.place.map(|place| {
        let style = match place {
            Place::Terminal => Style::new().fg(ACCENT),
            _ => Style::new().fg(MUTED),
        };
        Span::styled(format!(" {} ", place.badge()), style)
    });
    let badge_width = badge.as_ref().map_or(0, |b| b.width() as u16);
    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(badge_width)]).areas(rect);
    let bar = if selected {
        Span::styled("▌", Style::new().fg(ACCENT))
    } else {
        Span::raw(" ")
    };
    let label_width = 34usize;
    let label = suggestion.label.clone();
    let pad = label_width.saturating_sub(Line::from(label.as_str()).width());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            bar,
            Span::styled(label, Style::new().bold()),
            Span::raw(" ".repeat(pad + 1)),
            Span::styled(suggestion.about.clone(), Style::new().fg(MUTED)),
        ]))
        .style(base),
        left,
    );
    if let Some(badge) = badge {
        frame.render_widget(Paragraph::new(Line::from(badge)).style(base), right);
    }
}

/// What a command printed, scrollable. Returns the scroll actually used.
fn draw_output(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    text: &str,
    scroll: u16,
    failed: bool,
) -> u16 {
    let lines: Vec<&str> = text.lines().collect();
    let widest = lines
        .iter()
        .map(|l| Line::from(*l).width())
        .max()
        .unwrap_or(0) as u16;
    let width = (widest + 4)
        .max(Line::from(title).width() as u16 + 6)
        .clamp(40, area.width.saturating_sub(4).max(1));
    let height = (lines.len() as u16 + 2).clamp(3, area.height.saturating_sub(4).max(3));
    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);
    let colour = if failed { DANGER } else { ACCENT };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(format!(" {title} ")).bold())
        .border_style(Style::new().fg(colour))
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    let most = (lines.len() as u16).saturating_sub(inner.height);
    let scroll = scroll.min(most);
    if most > 0 {
        let shown = format!(
            " {}-{} of {} ",
            scroll + 1,
            (scroll + inner.height).min(lines.len() as u16),
            lines.len()
        );
        frame.render_widget(
            Paragraph::new(lines.join("\n"))
                .block(block.title_bottom(Line::from(shown).right_aligned()))
                .scroll((scroll, 0)),
            popup,
        );
    } else {
        frame.render_widget(Paragraph::new(lines.join("\n")).block(block), popup);
    }
    scroll
}

fn draw_header(frame: &mut Frame, area: Rect, app: &mut App) {
    let mut left = vec![Span::styled(
        " ◆ beam ",
        Style::new().bg(ACCENT).fg(Color::White).bold(),
    )];
    if let Some(me) = &app.snapshot.me {
        left.push(Span::raw("  "));
        if !me.name.is_empty() {
            left.push(Span::styled(me.name.clone(), Style::new().bold()));
            left.push(Span::raw("  "));
        }
        left.push(Span::styled(me.short_id.clone(), Style::new().fg(MUTED)));
    }
    let agent = match &app.snapshot.agent {
        Agent::Running { .. } => Span::styled(
            " ● receiving ",
            Style::new().bg(GOOD).fg(Color::White).bold(),
        ),
        Agent::Unreadable => Span::styled(
            " ● agent: status unreadable ",
            Style::new().bg(Color::Yellow).fg(Color::Black),
        ),
        Agent::Stopped => Span::styled(" ○ not receiving ", Style::new().bg(BAR).fg(MUTED)),
    };
    let mut right = Vec::new();
    if let Some(out) = app.sending.as_ref().filter(|s| s.running()) {
        let what = match out.stage {
            Stage::Moving { done, total, .. } if total > 0 => {
                format!("{} %", crate::ui::percent(done, total))
            }
            Stage::Waiting => "waiting".to_string(),
            _ => "…".to_string(),
        };
        right.push(Span::styled(
            format!(" ⇡ {what} {} ", out.peer),
            Style::new().bg(ACCENT).fg(Color::White).bold(),
        ));
        right.push(Span::raw(" "));
    }
    if let Some(live) = app.receiving {
        right.push(Span::styled(
            format!(" ⇣ {} % ", crate::ui::percent(live.done, live.total)),
            Style::new().bg(ACCENT).fg(Color::White).bold(),
        ));
        right.push(Span::raw(" "));
    }
    if !app.pending.is_empty() {
        right.push(Span::styled(
            format!(" ● {} waiting ", app.pending.len()),
            Style::new().bg(DANGER).fg(Color::White).bold(),
        ));
        right.push(Span::raw(" "));
    }
    let badge_width = agent.width() as u16;
    right.push(agent);
    let right = Line::from(right);
    let right_width = right.width() as u16;
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    frame.render_widget(Line::from(left), l);
    frame.render_widget(right, r);
    // "○ not receiving" leads to the switch.
    let badge = Rect {
        x: r.right().saturating_sub(badge_width),
        width: badge_width.min(r.width),
        ..r
    };
    app.areas.add(badge, Target::Tab(Tab::Pending));
}

/// Tabs as chips, each recorded so a click can pick it.
fn draw_tabs(frame: &mut Frame, area: Rect, app: &mut App) {
    let mut x = area.x + 1;
    let y = area.y + 1;
    for tab in Tab::ALL {
        let label = match (tab, app.pending.len()) {
            (Tab::Pending, n) if n > 0 => format!(" {} {n} ", tab.title()),
            _ => format!(" {} ", tab.title()),
        };
        let width = (label.chars().count() as u16).min(area.right().saturating_sub(x));
        if width == 0 || y >= area.bottom() {
            break;
        }
        let style = if tab == app.tab {
            Style::new().bg(ACCENT).fg(Color::White).bold()
        } else {
            Style::new().fg(MUTED)
        };
        let chip = Rect::new(x, y, width, 1);
        frame.render_widget(Span::styled(label, style), chip);
        app.areas.add(chip, Target::Tab(tab));
        x += width + 1;
    }
}

fn draw_no_identity(frame: &mut Frame, area: Rect) {
    let text = Text::from(vec![
        Line::from("This device has no identity yet.".bold()),
        Line::from(""),
        Line::from("Press Ctrl+Q to leave, then run `beam init` once to create it."),
        Line::from("The private key it makes never leaves this computer.".fg(MUTED)),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(panel(""))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        centered(area, 66, 8),
    );
}

fn draw_friends(frame: &mut Frame, area: Rect, app: &mut App) {
    let wide = area.width >= WIDE;
    let constraints = if wide {
        vec![
            Constraint::Length(LIST_WIDTH),
            Constraint::Min(20),
            Constraint::Length(DETAILS_WIDTH),
        ]
    } else {
        vec![Constraint::Length(LIST_WIDTH), Constraint::Min(20)]
    };
    let panels = Layout::horizontal(constraints).split(area);

    draw_friend_list(frame, panels[0], app);
    let friend = app.selected_friend().cloned();
    let mut middle = Vec::new();
    if !wide && let Some(f) = &friend {
        middle.extend(details(f));
        middle.push(Line::from(""));
    }
    if let Some(f) = &friend
        && let Some(out) = app
            .sending
            .as_ref()
            .filter(|s| s.running() && s.peer == f.name)
    {
        middle.push(Line::from(vec![
            Span::styled(" → ", Style::new().fg(ACCENT).bold()),
            Span::styled(out.file.clone(), Style::new().bold()),
            Span::raw("  "),
            Span::styled(
                format!(" {} ", stage_words(&out.stage, &out.peer)),
                Style::new().bg(ACCENT).fg(Color::White),
            ),
            Span::styled("  s to see it", Style::new().fg(MUTED)),
        ]));
    }
    if let Some(f) = &friend {
        for p in app.pending_from(&f.fingerprint) {
            middle.push(Line::from(vec![
                Span::styled(" ← ", Style::new().fg(ACCENT).bold()),
                Span::styled(p.request.file_name.clone(), Style::new().bold()),
                Span::raw(format!("  {}  ", crate::ui::format_bytes(p.request.size))),
                Span::styled(" WAITING ", Style::new().bg(DANGER).fg(Color::White).bold()),
                Span::styled(
                    format!("  {} left · Enter to answer", countdown(p.expires)),
                    Style::new().fg(MUTED),
                ),
            ]));
        }
    }
    middle.extend(transfers(friend.as_ref(), &app.snapshot.history));
    let title = friend
        .as_ref()
        .map(|f| format!(" @ {} ", f.name))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(middle)
            .block(panel(title))
            .wrap(Wrap { trim: false }),
        panels[1],
    );
    if wide {
        let lines = friend.as_ref().map(details).unwrap_or_default();
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel(" Details "))
                .wrap(Wrap { trim: false }),
            panels[2],
        );
    }
}

/// The friend list, drawn row by row so each row can be clicked.
fn draw_friend_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let count = app.snapshot.friends.len();
    let block = panel(Line::from(vec![
        Span::styled(" FRIENDS ", Style::new().fg(MUTED).bold()),
        Span::styled(format!("— {count} "), Style::new().fg(MUTED)),
    ]))
    .padding(Padding::ZERO);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.areas.list = inner;
    if count == 0 {
        let text = vec![
            Line::from("No friends yet."),
            Line::from(""),
            Line::from("Open the Add friend tab to pair with someone.".fg(MUTED)),
        ];
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
        return;
    }

    let visible = (inner.height / ROW_HEIGHT).max(1) as usize;
    // Scroll just enough to keep the selection on screen.
    if app.selected < app.list_offset {
        app.list_offset = app.selected;
    } else if app.selected >= app.list_offset + visible {
        app.list_offset = app.selected + 1 - visible;
    }
    app.list_offset = app.list_offset.min(count.saturating_sub(visible));

    for (row, index) in (app.list_offset..count).take(visible).enumerate() {
        let y = inner.y + row as u16 * ROW_HEIGHT;
        let height = ROW_HEIGHT.min(inner.bottom().saturating_sub(y));
        if height == 0 {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, height);
        let friend = &app.snapshot.friends[index];
        let selected = index == app.selected;
        let base = if selected {
            Style::new().bg(SELECTED)
        } else {
            Style::new()
        };
        let bar = if selected {
            Span::styled("▌", Style::new().fg(ACCENT))
        } else {
            Span::raw(" ")
        };
        let lines = vec![
            Line::from(vec![
                bar.clone(),
                avatar(friend),
                Span::raw(" "),
                Span::styled(friend.name.clone(), Style::new().bold()),
            ]),
            Line::from(vec![
                bar,
                Span::styled(format!("    {}", friend.short_id), Style::new().fg(MUTED)),
            ]),
        ];
        frame.render_widget(Paragraph::new(lines).style(base), rect);
        app.areas.add(rect, Target::Friend(index));
    }
}

/// A friend's initial on their colour, like a Discord avatar.
fn avatar(friend: &Friend) -> Span<'static> {
    let initial = friend
        .name
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('?');
    let colour = u8::from_str_radix(friend.fingerprint.get(..2).unwrap_or("0"), 16).unwrap_or(0);
    Span::styled(
        format!(" {initial} "),
        Style::new()
            .bg(AVATARS[colour as usize % AVATARS.len()])
            .fg(Color::White)
            .bold(),
    )
}

/// The details of one friend: what a person compares when in doubt.
fn details(friend: &Friend) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            avatar(friend),
            Span::raw(" "),
            Span::styled(friend.name.clone(), Style::new().bold()),
        ]),
        Line::from(""),
        heading("SHORT ID"),
        Line::from(friend.short_id.clone()),
        Line::from(""),
        heading("FINGERPRINT (SHA-256)"),
    ];
    lines.extend(
        fingerprint_lines(&friend.fingerprint)
            .into_iter()
            .map(Line::from),
    );
    lines.extend([
        Line::from(""),
        heading("PAIRED"),
        Line::from(friend.added.clone()),
        Line::from(""),
        heading("LAST SEEN"),
        Line::from(match friend.last_seen {
            Some(at) => crate::history::ago(at, crate::history::unix_now()),
            None => "not yet".to_string(),
        }),
        Line::from(""),
        Line::from(vec![
            key_chip("Ctrl+C"),
            Span::styled(" copy fingerprint", Style::new().fg(MUTED)),
        ]),
    ]);
    lines
}

/// The middle panel: what went to and came from this friend, newest first.
fn transfers(friend: Option<&Friend>, history: &[crate::history::Entry]) -> Vec<Line<'static>> {
    use crate::history::{Direction, Outcome};
    let Some(friend) = friend else {
        return vec![Line::from("Pick a friend on the left.".fg(MUTED))];
    };
    let theirs: Vec<&crate::history::Entry> = history
        .iter()
        .rev()
        .filter(|e| e.fingerprint == friend.fingerprint)
        .collect();
    if theirs.is_empty() {
        return vec![
            Line::from(""),
            Line::from(format!("No files with {} yet.", friend.name).fg(MUTED)),
            Line::from(""),
            Line::from(vec![
                Span::styled("Send one:  ", Style::new().fg(MUTED)),
                Span::styled(
                    format!(":send {} <file>", friend.name),
                    Style::new().fg(ACCENT),
                ),
            ]),
        ];
    }
    let now = crate::history::unix_now();
    let mut lines = vec![heading("FILES")];
    for e in theirs {
        let (arrow, arrow_style) = match e.direction {
            Direction::Sent => ("→ ", Style::new().fg(MUTED)),
            Direction::Received => ("← ", Style::new().fg(ACCENT)),
        };
        let result_style = match e.outcome {
            Outcome::Done => Style::new().fg(GOOD),
            Outcome::Declined | Outcome::Cancelled => Style::new().fg(MUTED),
            Outcome::Failed => Style::new().fg(DANGER),
        };
        let mark = if e.outcome == Outcome::Done {
            " ✓"
        } else {
            ""
        };
        let mut spans = vec![
            Span::styled(arrow, arrow_style.bold()),
            Span::styled(e.file.clone(), Style::new().bold()),
            Span::raw(format!("  {}  ", crate::ui::format_bytes(e.size))),
            Span::styled(
                format!(
                    "{}{mark}",
                    crate::history::outcome_word(e.outcome, e.direction)
                ),
                result_style,
            ),
            Span::styled(
                format!("  {}", crate::history::ago(e.at, now)),
                Style::new().fg(MUTED),
            ),
        ];
        if let Some(note) = &e.note
            && e.outcome != Outcome::Done
        {
            spans.push(Span::styled(format!("  · {note}"), Style::new().fg(MUTED)));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn draw_pending(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = panel(" Pending ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The Receiving switch, and under it a line, before anything else.
    let [strip, rule, inner] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    draw_switch(frame, strip, app);
    frame.render_widget(Line::from("─".repeat(rule.width as usize).fg(BORDER)), rule);

    let receiving_here = matches!(app.switch, Switch::On);
    let notice: Option<Vec<Line>> = match (&app.snapshot.agent, &app.link) {
        _ if app.snapshot.listen_elsewhere && !receiving_here => Some(vec![Line::from(
            "Requests to `beam listen` are answered in its own terminal.".fg(MUTED),
        )]),
        (Agent::Stopped, _) => Some(vec![
            Line::from("Nothing can arrive while Receiving is off.".bold()),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "To receive even when beam is closed: ",
                    Style::new().fg(MUTED),
                ),
                Span::styled(":service enable", Style::new().fg(ACCENT)),
                Span::styled(" (the background agent).", Style::new().fg(MUTED)),
            ]),
        ]),
        (Agent::Unreadable, _) => Some(vec![
            Line::from("The background agent is running, but its status could not be read."),
            Line::from(vec![
                Span::raw("Restart it: "),
                Span::styled(":service stop", Style::new().fg(ACCENT)),
                Span::raw(" then "),
                Span::styled(":service start", Style::new().fg(ACCENT)),
            ]),
        ]),
        (Agent::Running { .. }, Link::Lost(reason)) => Some(vec![
            Line::from("Not connected to the receiver.".bold()),
            Line::from(reason.clone().fg(MUTED)),
            Line::from("Trying again…".fg(MUTED)),
        ]),
        (Agent::Running { .. }, Link::Off | Link::Connecting) => {
            Some(vec![Line::from("Connecting…".fg(MUTED))])
        }
        (Agent::Running { .. }, Link::Connected) => None,
    };
    if let Some(lines) = notice {
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    let receive_dir = match &app.snapshot.agent {
        Agent::Running { receive_dir } => receive_dir.clone(),
        _ => String::new(),
    };
    let bottom = if app.receiving.is_some() { 4 } else { 2 };
    let [list, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(bottom)]).areas(inner);

    if app.pending.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("Nothing is waiting.".bold()),
                Line::from(""),
                Line::from(
                    "When a friend sends you a file, it appears here and in the header. \
                     Nothing is ever accepted without you."
                        .fg(MUTED),
                ),
            ])
            .wrap(Wrap { trim: true }),
            list,
        );
    } else {
        let rows = (list.height / 3).max(1) as usize;
        let offset = (app.pending_selected + 1).saturating_sub(rows);
        for (row, index) in (offset..app.pending.len()).take(rows).enumerate() {
            let y = list.y + row as u16 * 3;
            if y + 2 > list.bottom() {
                break;
            }
            let rect = Rect::new(list.x, y, list.width, 2);
            let p = &app.pending[index];
            let selected = index == app.pending_selected;
            let bar = if selected {
                Span::styled("▌", Style::new().fg(ACCENT))
            } else {
                Span::raw(" ")
            };
            let mut second = vec![
                bar.clone(),
                Span::styled(
                    format!(
                        " {} · {} left",
                        crate::ui::format_bytes(p.request.size),
                        countdown(p.expires)
                    ),
                    Style::new().fg(MUTED),
                ),
            ];
            if let Some(resume) = &p.request.resume {
                second.push(Span::styled(format!(" · {resume}"), Style::new().fg(MUTED)));
            }
            let lines = vec![
                Line::from(vec![
                    bar,
                    Span::styled(format!(" {}", p.request.peer_name), Style::new().bold()),
                    Span::raw(" wants to send "),
                    Span::styled(
                        p.request.file_name.clone(),
                        Style::new().bold().fg(Color::White),
                    ),
                ]),
                Line::from(second),
            ];
            let style = if selected {
                Style::new().bg(SELECTED)
            } else {
                Style::new()
            };
            frame.render_widget(Paragraph::new(lines).style(style), rect);
            app.areas.add(rect, Target::Request(index));
        }
    }

    let mut lines = Vec::new();
    if let Some(live) = app.receiving {
        let path = if live.relay {
            "[Relay]"
        } else {
            "[Direct P2P]"
        };
        lines.push(Line::from(vec![
            Span::styled("Receiving  ", Style::new().fg(MUTED)),
            Span::raw(format!(
                "{} of {}  {path}",
                crate::ui::format_bytes(live.done),
                crate::ui::format_bytes(live.total)
            )),
        ]));
        let width = status.width.saturating_sub(8) as u64;
        let filled = (width * live.done.min(live.total))
            .checked_div(live.total)
            .unwrap_or(0);
        lines.push(Line::from(vec![
            Span::styled("█".repeat(filled as usize), Style::new().fg(ACCENT)),
            Span::styled(
                "░".repeat((width - filled) as usize),
                Style::new().fg(BORDER),
            ),
            Span::raw(format!(
                " {:>3} %",
                crate::ui::percent(live.done, live.total)
            )),
        ]));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(vec![
        Span::styled("Received files go to  ", Style::new().fg(MUTED)),
        Span::raw(receive_dir),
    ]));
    frame.render_widget(Paragraph::new(lines), status);
}

/// The send's stage, in a few words.
fn stage_words(stage: &Stage, peer: &str) -> String {
    match *stage {
        Stage::Starting | Stage::Dialling => format!("looking for {peer}…"),
        Stage::Hashing { done, total } => {
            format!("reading the file {} %", crate::ui::percent(done, total))
        }
        Stage::Waiting => format!("waiting for {peer} to accept"),
        Stage::Moving { done, total, .. } if total > 0 => {
            format!("sending {} %", crate::ui::percent(done, total))
        }
        Stage::Moving { .. } => "accepted".to_string(),
        Stage::Checking { .. } => format!("{peer} is checking the file"),
    }
}

/// A bar `width` cells wide, `done` of `total` filled.
fn bar(done: u64, total: u64, width: u16) -> Line<'static> {
    let width = u64::from(width.saturating_sub(7));
    let filled = (width * done.min(total)).checked_div(total).unwrap_or(0);
    Line::from(vec![
        Span::styled("█".repeat(filled as usize), Style::new().fg(ACCENT)),
        Span::styled(
            "░".repeat((width - filled) as usize),
            Style::new().fg(BORDER),
        ),
        Span::raw(format!(" {:>3} %", crate::ui::percent(done, total))),
    ])
}

/// Which file to send, with the paths that complete what is typed.
#[allow(clippy::too_many_arguments)]
fn draw_pick_file(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    peer: &str,
    input: &Input,
    error: Option<&str>,
    paths: &[String],
    selected: usize,
) {
    let rows = paths.len().min(FILE_ROWS);
    let popup = centered(area, 72, 10 + rows as u16);
    frame.render_widget(Clear, popup);
    let block = popup_block(format!(" Send a file to {peer} "));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [note, field, problem, list, _, buttons] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(rows as u16),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Line::from("Type its path (Tab completes), or drag the file onto this window.".fg(MUTED)),
        note,
    );
    draw_input(frame, field, areas, input);
    if let Some(error) = error {
        frame.render_widget(Line::from(error.to_string().fg(DANGER)), problem);
    }
    let offset = (selected + 1).saturating_sub(rows);
    for (row, (index, path)) in paths.iter().enumerate().skip(offset).take(rows).enumerate() {
        let rect = Rect::new(list.x, list.y + row as u16, list.width, 1);
        let chosen = index == selected;
        let folder = path.ends_with(['/', '\\']);
        let style = if chosen {
            Style::new().bg(SELECTED)
        } else {
            Style::new()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if chosen { "▌" } else { " " }, Style::new().fg(ACCENT)),
                Span::styled(
                    untrusted_path(path),
                    if folder {
                        Style::new().fg(ACCENT)
                    } else {
                        Style::new()
                    },
                ),
            ]))
            .style(style),
            rect,
        );
        areas.add(rect, Target::Suggestion(index));
    }
    draw_buttons(
        frame,
        buttons,
        areas,
        ("Send", "Cancel"),
        Choice::Yes,
        false,
    );
}

/// A file name from this disk can still hold control characters.
fn untrusted_path(path: &str) -> String {
    crate::untrusted::text(path)
}

/// How the send is going; Hide keeps it going in the header.
fn draw_send_status(frame: &mut Frame, area: Rect, areas: &mut Areas, sending: &Sending) {
    let popup = centered(area, 66, 11);
    frame.render_widget(Clear, popup);
    let block = popup_block(format!(" Sending to {} ", sending.peer));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [file, _, stage, progress, _, buttons] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Line::from(vec![
            Span::styled("File  ", Style::new().fg(MUTED)),
            Span::styled(sending.file.clone(), Style::new().bold().fg(Color::White)),
        ]),
        file,
    );
    match &sending.result {
        Some(Ok(message)) => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("✓ ", Style::new().fg(GOOD).bold()),
                    Span::raw(message.clone()),
                ]))
                .wrap(Wrap { trim: true }),
                stage,
            );
            draw_one_choice(frame, buttons, areas, "Close");
        }
        Some(Err(message)) => {
            let [line, _] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(0)]).areas(Rect {
                    height: stage.height + progress.height + 1,
                    ..stage
                });
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("✗ ", Style::new().fg(DANGER).bold()),
                    Span::raw(message.clone()),
                ]))
                .wrap(Wrap { trim: true }),
                line,
            );
            draw_one_choice(frame, buttons, areas, "Close");
        }
        None => {
            let mut lines = vec![Line::from(stage_words(&sending.stage, &sending.peer))];
            match sending.stage {
                Stage::Waiting => lines.push(Line::from(
                    "Nothing is sent until they say yes. Hide this and keep using beam.".fg(MUTED),
                )),
                Stage::Moving { relay, total, .. } if total > 0 => lines.push(Line::from(
                    if relay { "[Relay]" } else { "[Direct P2P]" }.fg(MUTED),
                )),
                _ => {}
            }
            frame.render_widget(Paragraph::new(lines), stage);
            match sending.stage {
                Stage::Hashing { done, total }
                | Stage::Moving { done, total, .. }
                | Stage::Checking { done, total }
                    if total > 0 =>
                {
                    frame.render_widget(bar(done, total, progress.width), progress);
                }
                _ => {}
            }
            draw_buttons(
                frame,
                buttons,
                areas,
                ("Hide", "Cancel send"),
                Choice::Yes,
                false,
            );
        }
    }
}

/// One button at the right, recorded as "yes" (close).
fn draw_one_choice(frame: &mut Frame, area: Rect, areas: &mut Areas, label: &str) {
    let label = format!(" {label} ");
    let width = label.chars().count() as u16;
    if area.width < width || area.height == 0 {
        return;
    }
    let rect = Rect::new(area.right() - width, area.y, width, 1);
    frame.render_widget(
        Span::styled(label, Style::new().bg(ACCENT).fg(Color::White).bold()),
        rect,
    );
    areas.add(rect, Target::Button(Choice::Yes));
}

/// Leaving beam mid-send. Starts on Stay.
/// `peer` is who a file is going to; `None` means one is coming in.
fn draw_confirm_quit(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    peer: Option<&str>,
    focus: Choice,
) {
    let popup = centered(area, 62, 8);
    frame.render_widget(Clear, popup);
    let (title, first, second) = match peer {
        Some(peer) => (
            " A file is still going out ",
            format!("Leaving beam cancels the send to {peer}."),
            format!("{peer} is told, and keeps what arrived: sending again resumes."),
        ),
        None => (
            " A file is arriving ",
            "Leaving beam stops Receiving and cancels it.".to_string(),
            "The sender is told, and what arrived is kept: sending again resumes.".to_string(),
        ),
    };
    let block = popup_block(title);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [text, _, buttons] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(vec![Line::from(first), Line::from(second.fg(MUTED))])
            .wrap(Wrap { trim: true }),
        text,
    );
    draw_buttons(frame, buttons, areas, ("Leave", "Stay"), focus, true);
}

/// The Receiving switch at the top of Pending (ADR-0044). Bright while off,
/// so it is noticed; it names whoever else receives when it is moot.
fn draw_switch(frame: &mut Frame, area: Rect, app: &mut App) {
    let chip = |text: &str, bg: Color, fg: Color| {
        Span::styled(text.to_string(), Style::new().bg(bg).fg(fg).bold())
    };
    let elsewhere = app.receiving_elsewhere();
    let (switch, title_colour, line, hint): (Span, Color, String, String) = match (
        &app.switch,
        elsewhere,
    ) {
        (Switch::Starting, _) => (
            chip(" ◌  STARTING ", SELECTED, Color::White),
            MUTED,
            "Starting to receive…".to_string(),
            String::new(),
        ),
        (Switch::Stopping, _) => (
            chip(" ◌  STOPPING ", SELECTED, Color::White),
            MUTED,
            "Stopping…".to_string(),
            String::new(),
        ),
        (Switch::On, _) => (
            chip(" ●  ON ", GOOD, Color::White),
            GOOD,
            match &app.snapshot.agent {
                Agent::Running { receive_dir } => {
                    format!(
                        "Friends can send you files while beam is open · saving to {receive_dir}"
                    )
                }
                _ => "Friends can send you files while beam is open".to_string(),
            },
            "o or click to turn off · it stops when you leave beam".to_string(),
        ),
        (_, Some(Taken::Agent)) => (
            chip(" ●  AGENT ", GOOD, Color::White),
            GOOD,
            "Receiving through the background agent, even when beam is closed".to_string(),
            ":service stop stops it".to_string(),
        ),
        (_, Some(Taken::OtherView)) => (
            chip(" ●  ELSEWHERE ", GOOD, Color::White),
            GOOD,
            "Another beam window is receiving for you".to_string(),
            String::new(),
        ),
        (_, Some(Taken::Listen)) => (
            chip(" ●  LISTEN ", GOOD, Color::White),
            GOOD,
            "`beam listen` is receiving in another terminal".to_string(),
            String::new(),
        ),
        (Switch::Failed(why), None) => (
            chip(" ○  OFF ", NOTICE, Color::Black),
            NOTICE,
            format!("Could not start receiving: {why}"),
            "o or click to try again".to_string(),
        ),
        (Switch::Off, None) => (
            chip(" ○  OFF ", NOTICE, Color::Black),
            NOTICE,
            "Turn on so friends can send you files".to_string(),
            "press o, or click · pairing stays in Add friend".to_string(),
        ),
    };
    let [top, bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let title = Span::styled(" RECEIVING  ", Style::new().fg(title_colour).bold());
    let switch_x = top.x + title.width() as u16;
    let switch_width = switch.width() as u16;
    frame.render_widget(
        Line::from(vec![title, switch, Span::raw("  "), Span::raw(line)]),
        top,
    );
    if !hint.is_empty() {
        frame.render_widget(
            Line::from(vec![
                Span::raw(" ".repeat(13)),
                Span::styled(hint, Style::new().fg(MUTED)),
            ]),
            bottom,
        );
    }
    let clickable = elsewhere.is_none() || matches!(app.switch, Switch::On);
    if clickable {
        app.areas.add(
            Rect::new(
                switch_x,
                top.y,
                switch_width.min(top.right().saturating_sub(switch_x)),
                1,
            ),
            Target::ReceiveSwitch,
        );
    }
}

/// A file is arriving: turn Receiving off anyway? Starts on Keep.
fn draw_confirm_stop(frame: &mut Frame, area: Rect, areas: &mut Areas, focus: Choice) {
    let popup = centered(area, 62, 8);
    frame.render_widget(Clear, popup);
    let block = popup_block(" A file is arriving ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [text, _, buttons] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Turning Receiving off cancels it."),
            Line::from(
                "The sender is told, and what arrived is kept: sending again resumes.".fg(MUTED),
            ),
        ])
        .wrap(Wrap { trim: true }),
        text,
    );
    draw_buttons(
        frame,
        buttons,
        areas,
        ("Turn off", "Keep receiving"),
        focus,
        true,
    );
}

/// May this friend send this file? The same facts as `beam listen` (S-6).
fn draw_accept(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    request: &RequestView,
    expires: std::time::Instant,
    focus: Choice,
) {
    let height = if request.resume.is_some() { 18 } else { 16 };
    let popup = centered(area, 64, height);
    frame.render_widget(Clear, popup);
    let block = popup_block(" Incoming file ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [text, _, buttons] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(request.peer_name.clone(), Style::new().bold()),
            Span::raw(" wants to send you a file."),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("File   ", Style::new().fg(MUTED)),
            Span::styled(
                request.file_name.clone(),
                Style::new().bold().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("Size   ", Style::new().fg(MUTED)),
            Span::raw(crate::ui::format_bytes(request.size)),
        ]),
    ];
    if let Some(resume) = &request.resume {
        lines.push(Line::from(vec![
            Span::styled("Resume ", Style::new().fg(MUTED)),
            Span::raw(resume.clone()),
        ]));
        lines.push(Line::from(
            "A resumed transfer needs your yes again.".fg(MUTED),
        ));
    }
    lines.push(Line::from(""));
    lines.push(heading("FROM THE DEVICE WITH FINGERPRINT"));
    lines.extend(
        fingerprint_lines(&request.fingerprint)
            .into_iter()
            .map(Line::from),
    );
    lines.push(Line::from(""));
    lines.push(Line::from(
        format!("Answer within {}; no answer means no.", countdown(expires)).fg(MUTED),
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), text);
    draw_buttons(frame, buttons, areas, ("Accept", "Decline"), focus, false);
}

/// The Add friend form: their invite and a name, or show this device's own.
fn draw_add_friend(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = panel(" Add friend ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let form = &app.add;
    let focus = form.focus;
    let [
        intro,
        label1,
        invite_box,
        label2,
        name_row,
        error_row,
        _,
        or_row,
        mine_row,
        _,
        fp,
    ] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(
            "Pairing makes two devices friends. It is done once, and both people compare \
             fingerprints before it counts.",
        )
        .style(Style::new().fg(MUTED))
        .wrap(Wrap { trim: true }),
        intro,
    );
    frame.render_widget(heading("THEIR INVITE  (starts with beam1)"), label1);
    let invite = form.invite.clone();
    let name = form.name.clone();
    let error = form.error.clone();
    draw_field(
        frame,
        invite_box,
        &mut app.areas,
        &invite,
        focus == Some(Field::Invite),
        Field::Invite,
    );
    frame.render_widget(heading("A NAME FOR THEM"), label2);
    let [name_box, _, pair_button, _] = Layout::horizontal([
        Constraint::Length(28),
        Constraint::Length(2),
        Constraint::Length(10),
        Constraint::Min(0),
    ])
    .areas(name_row);
    draw_field(
        frame,
        name_box,
        &mut app.areas,
        &name,
        focus == Some(Field::Name),
        Field::Name,
    );
    let pair_line = Rect {
        y: pair_button.y + 1,
        height: 1,
        ..pair_button
    };
    draw_button(
        frame,
        pair_line,
        &mut app.areas,
        " Pair ",
        focus == Some(Field::Pair),
        Field::Pair,
    );
    if let Some(error) = error {
        frame.render_widget(
            Line::from(vec![
                Span::styled("✗ ", Style::new().fg(DANGER).bold()),
                Span::styled(error, Style::new().fg(DANGER)),
            ]),
            error_row,
        );
    }
    frame.render_widget(Line::from("── or ──".fg(MUTED)), or_row);
    let [mine_button, mine_note] =
        Layout::horizontal([Constraint::Length(18), Constraint::Min(0)]).areas(mine_row);
    draw_button(
        frame,
        mine_button,
        &mut app.areas,
        " Show my invite ",
        focus == Some(Field::ShowMine),
        Field::ShowMine,
    );
    frame.render_widget(
        Line::from("  they pair with you; the name comes from their device".fg(MUTED)),
        mine_note,
    );
    if let Some(me) = &app.snapshot.me {
        let mut lines = vec![heading("YOUR FINGERPRINT, TO COMPARE WHEN ASKED")];
        lines.extend(
            fingerprint_lines(&me.fingerprint)
                .into_iter()
                .map(Line::from),
        );
        frame.render_widget(Paragraph::new(lines), fp);
    }
}

/// A text box on the form; its border lights up when it has the keyboard.
fn draw_field(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    input: &Input,
    focused: bool,
    field: Field,
) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { ACCENT } else { BORDER }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(input.text().to_string()).scroll((0, input.scroll(inner.width))),
        inner,
    );
    if focused && inner.width > 0 && inner.height > 0 {
        frame.set_cursor_position((inner.x + input.cursor_column(inner.width), inner.y));
    }
    areas.add(inner, Target::Field(field));
}

fn draw_button(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    label: &str,
    focused: bool,
    field: Field,
) {
    let style = if focused {
        Style::new().bg(ACCENT).fg(Color::White).bold()
    } else {
        Style::new().bg(SELECTED).fg(Color::White)
    };
    let width = (label.chars().count() as u16).min(area.width);
    let rect = Rect {
        width,
        height: area.height.min(1),
        ..area
    };
    frame.render_widget(Span::styled(label.to_string(), style), rect);
    areas.add(rect, Target::Field(field));
}

/// Type the code shown on the other device.
fn draw_enter_code(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    input: &Input,
    error: Option<&str>,
) {
    let popup = centered(area, 52, 10);
    frame.render_widget(Clear, popup);
    let block = popup_block(" Pairing code ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [note, field, problem, _, buttons] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new("Type the code shown on their screen. It works for one attempt.")
            .style(Style::new().fg(MUTED))
            .wrap(Wrap { trim: true }),
        note,
    );
    let field = Rect {
        width: field.width.min(20),
        ..field
    };
    draw_input(frame, field, areas, input);
    if let Some(error) = error {
        frame.render_widget(Line::from(error.to_string().fg(DANGER)), problem);
    }
    draw_buttons(
        frame,
        buttons,
        areas,
        ("Pair", "Cancel"),
        Choice::Yes,
        false,
    );
}

/// Pairing is working; Esc or Cancel stops it.
fn draw_busy(frame: &mut Frame, area: Rect, areas: &mut Areas, text: &str) {
    let popup = centered(area, 56, 7);
    frame.render_widget(Clear, popup);
    let block = popup_block(" Pairing ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [line, _, buttons] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(text.to_string()).wrap(Wrap { trim: true }),
        line,
    );
    draw_one_button(frame, buttons, areas, "Cancel");
}

/// This device's invite and code, while waiting for the other person.
fn draw_show_invite(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    invite: &str,
    code: &str,
    expires: std::time::Instant,
    attempt: Option<&str>,
) {
    let popup = centered(area, 72, 17);
    frame.render_widget(Clear, popup);
    let block = popup_block(" Your invite ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [note, invite_area, _, code_row, expiry, _, status, buttons] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(
            "Send them this invite any way you like. In their beam: Add friend, paste it, \
             then type the code below.",
        )
        .style(Style::new().fg(MUTED))
        .wrap(Wrap { trim: true }),
        note,
    );
    frame.render_widget(
        Paragraph::new(invite.to_string())
            .style(Style::new().fg(ACCENT))
            .wrap(Wrap { trim: false }),
        invite_area,
    );
    frame.render_widget(
        Line::from(vec![
            Span::styled("Pairing code   ", Style::new().fg(MUTED)),
            Span::styled(code.to_string(), Style::new().bold().fg(Color::White)),
        ]),
        code_row,
    );
    frame.render_widget(
        Line::from(format!("Expires in {} · works for one attempt", countdown(expires)).fg(MUTED)),
        expiry,
    );
    let line = match attempt {
        Some(peer) => {
            Line::from(format!("A device is pairing ({peer}). The code is now used up…").fg(GOOD))
        }
        None => Line::from("Waiting for them…"),
    };
    frame.render_widget(line, status);
    draw_buttons(
        frame,
        buttons,
        areas,
        ("Copy invite", "Cancel"),
        Choice::Yes,
        false,
    );
}

/// Do the fingerprints match? Starts on No.
fn draw_confirm_pair(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    name: &str,
    peer: &str,
    own: &str,
    focus: Choice,
) {
    let popup = centered(area, 62, 19);
    frame.render_widget(Clear, popup);
    let block = popup_block(" Check fingerprints ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [text, _, buttons] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let mut lines = vec![
        Line::from(format!(
            "Pairing with the device that will be saved as {name}."
        )),
        Line::from(
            "Read both out to each other, or compare screens. Pair only if both match.".fg(MUTED),
        ),
        Line::from(""),
        heading("THEIR FINGERPRINT"),
    ];
    lines.extend(fingerprint_lines(peer).into_iter().map(Line::from));
    lines.push(Line::from(""));
    lines.push(heading("YOURS"));
    lines.extend(fingerprint_lines(own).into_iter().map(Line::from));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), text);
    draw_buttons(frame, buttons, areas, ("They match", "No"), focus, false);
}

/// An invite moves a paired friend to another relay (ADR-0038).
#[allow(clippy::too_many_arguments)]
fn draw_relay_change(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    name: &str,
    fingerprint: &str,
    old: &str,
    new: &str,
    focus: Choice,
) {
    let popup = centered(area, 70, 18);
    frame.render_widget(Clear, popup);
    let block = popup_block(format!(" {name} is already your friend "));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [text, _, buttons] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let mut lines = vec![
        Line::from(format!("This invite moves {name} to a different relay.")),
        Line::from(""),
        Line::from(vec![
            Span::styled("Relay now    ", Style::new().fg(MUTED)),
            Span::raw(old.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Relay after  ", Style::new().fg(MUTED)),
            Span::raw(new.to_string()),
        ]),
        Line::from(""),
        Line::from(format!(
            "Whoever runs a relay can see when you send to {name} and can block it, but cannot read the files."
        )),
        Line::from(format!("Only say yes if {name} told you it changed relay.").bold()),
        Line::from(""),
        heading("FINGERPRINT"),
    ];
    lines.extend(fingerprint_lines(fingerprint).into_iter().map(Line::from));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), text);
    draw_buttons(
        frame,
        buttons,
        areas,
        ("Use the new relay", "Keep"),
        focus,
        false,
    );
}

/// One button at the right, recorded as "no" (cancel).
fn draw_one_button(frame: &mut Frame, area: Rect, areas: &mut Areas, label: &str) {
    let label = format!(" {label} ");
    let width = label.chars().count() as u16;
    if area.width < width || area.height == 0 {
        return;
    }
    let rect = Rect::new(area.right() - width, area.y, width, 1);
    frame.render_widget(
        Span::styled(label, Style::new().bg(SELECTED).fg(Color::White)),
        rect,
    );
    areas.add(rect, Target::Button(Choice::No));
}

/// The status bar: keys for this page on the left; a message, or a
/// problem reading `~/.beam`, on the right.
fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(Block::new().style(Style::new().bg(BAR)), area);
    let keys: &[(&str, &str)] = match (&app.modal, app.tab) {
        (Some(Modal::Rename { .. }), _) => &[("Enter", "save"), ("Esc", "cancel")],
        (Some(Modal::Remove { .. }), _) => {
            &[("←→", "choose"), ("Enter", "confirm"), ("Esc", "keep")]
        }
        (Some(Modal::Help), _) => &[("any key", "close")],
        (Some(Modal::EnterCode { .. }), _) => &[("Enter", "pair"), ("Esc", "cancel")],
        (Some(Modal::Busy { .. }), _) => &[("Esc", "cancel")],
        (Some(Modal::ShowInvite { .. }), _) => &[("Ctrl+C", "copy invite"), ("Esc", "cancel")],
        (Some(Modal::ConfirmPair { .. } | Modal::RelayChange { .. }), _) => {
            &[("←→", "choose"), ("Enter", "confirm"), ("Esc", "no")]
        }
        (Some(Modal::PickFile { .. }), _) => &[
            ("Tab", "complete"),
            ("↑↓", "choose"),
            ("Enter", "send"),
            ("Esc", "cancel"),
        ],
        (Some(Modal::SendStatus), _) => &[("Esc", "hide"), ("x", "cancel the send")],
        (Some(Modal::ConfirmQuit { .. } | Modal::ConfirmStop { .. }), _) => {
            &[("←→", "choose"), ("Enter", "confirm")]
        }
        (Some(Modal::Accept { .. }), _) => &[
            ("←→", "choose"),
            ("Enter", "answer"),
            ("d", "decline"),
            ("Esc", "later"),
        ],
        (None, Tab::Pending) if !app.pending.is_empty() => &[
            ("↑↓", "request"),
            ("Enter", "answer"),
            ("o", "receiving on/off"),
            (":", "commands"),
            ("Tab", "tab"),
            ("?", "help"),
            ("Ctrl+Q", "quit"),
        ],
        (None, Tab::Pending) => &[
            ("o", "receiving on/off"),
            (":", "commands"),
            ("Tab", "tab"),
            ("?", "help"),
            ("Ctrl+Q", "quit"),
        ],
        (None, Tab::AddFriend) if app.add.focus.is_some() => &[
            ("Tab", "next"),
            ("Enter", "go"),
            ("Esc", "leave the form"),
            ("Ctrl+P", "commands"),
            ("Ctrl+Q", "quit"),
        ],
        (Some(Modal::Output { .. }), _) => &[
            ("↑↓", "scroll"),
            ("Ctrl+C", "copy"),
            (":", "commands"),
            ("Esc", "close"),
        ],
        (None, Tab::Friends) => &[
            (":", "commands"),
            ("↑↓", "friend"),
            ("s", "send"),
            ("r", "rename"),
            ("x", "remove"),
            ("Ctrl+C", "copy"),
            ("Tab", "tab"),
            ("?", "help"),
            ("Ctrl+Q", "quit"),
        ],
        (None, _) => &[
            (":", "commands"),
            ("Ctrl+C", "copy"),
            ("Tab", "tab"),
            ("?", "help"),
            ("Ctrl+Q", "quit"),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    for (key, what) in keys {
        spans.push(key_chip(key));
        spans.push(Span::styled(format!(" {what}  "), Style::new().fg(MUTED)));
    }
    let message = match (&app.snapshot.problem, &app.flash) {
        (Some(problem), _) => Some(Span::styled(
            format!(" ! {problem} "),
            Style::new().fg(Color::Yellow).bold(),
        )),
        (None, Some(flash)) => Some(Span::styled(
            format!(" {flash} "),
            Style::new().fg(Color::White).bold(),
        )),
        (None, None) => None,
    };
    let right = message
        .as_ref()
        .map_or(0, |m| m.width() as u16)
        .min(area.width / 2);
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right)]).areas(area);
    frame.render_widget(Line::from(spans).style(Style::new().bg(BAR)), l);
    if let Some(message) = message {
        frame.render_widget(Line::from(message).style(Style::new().bg(BAR)), r);
    }
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let rows = [
        (":  or  Ctrl+P", "commands: every beam command, here"),
        ("↑ ↓  k j  wheel", "move between friends"),
        ("click", "pick a friend, a tab or a button"),
        ("Tab  ← →  1 2 3", "switch tab"),
        ("Enter", "answer a waiting request"),
        ("s", "send a file to the selected friend"),
        ("o", "receiving on/off, while beam is open"),
        ("r", "rename the selected friend"),
        ("x  Delete", "remove the selected friend"),
        ("Ctrl+C", "copy the fingerprint shown"),
        ("Shift + drag", "select text with the mouse"),
        ("Esc", "close a pop-up"),
        ("Ctrl+Q  (or q)", "leave beam"),
    ];
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!(" {k:<18}"), Style::new().bold()),
                Span::styled(*v, Style::new().fg(MUTED)),
            ])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(
        " Every command still works: `beam --help`.".fg(MUTED),
    ));
    lines.push(Line::from(
        " `beam ui cli` makes plain `beam` print the help instead.".fg(MUTED),
    ));
    let popup = centered(area, 64, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(popup_block(" Keys ")), popup);
}

fn draw_rename(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    from: &str,
    input: &Input,
    error: Option<&str>,
) {
    let popup = centered(area, 56, 11);
    frame.render_widget(Clear, popup);
    let block = popup_block(format!(" Rename {from} "));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [note, field, problem, _, buttons] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(format!(
            "Only your label for them changes; {from} is not told. Use A-Z a-z 0-9 . _ -"
        ))
        .style(Style::new().fg(MUTED))
        .wrap(Wrap { trim: true }),
        note,
    );
    draw_input(frame, field, areas, input);
    if let Some(error) = error {
        frame.render_widget(
            Paragraph::new(error.to_string()).style(Style::new().fg(DANGER)),
            problem,
        );
    }
    draw_buttons(
        frame,
        buttons,
        areas,
        ("Save", "Cancel"),
        Choice::Yes,
        false,
    );
}

fn draw_remove(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    name: &str,
    fingerprint: &str,
    focus: Choice,
) {
    let popup = centered(area, 60, 12);
    frame.render_widget(Clear, popup);
    let block = popup_block(format!(" Remove {name}? "));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [text, _, buttons] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let mut lines = vec![
        Line::from(format!("{name} will no longer be able to send you files.")),
        Line::from("To be friends again, you would have to pair again.".fg(MUTED)),
        Line::from(""),
        heading("FINGERPRINT"),
    ];
    lines.extend(fingerprint_lines(fingerprint).into_iter().map(Line::from));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), text);
    draw_buttons(frame, buttons, areas, ("Remove", "Keep"), focus, true);
}

/// A bordered text box with the terminal's own cursor in it.
fn draw_input(frame: &mut Frame, area: Rect, areas: &mut Areas, input: &Input) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let scroll = input.scroll(inner.width);
    frame.render_widget(
        Paragraph::new(input.text().to_string()).scroll((0, scroll)),
        inner,
    );
    if inner.width > 0 && inner.height > 0 {
        frame.set_cursor_position((inner.x + input.cursor_column(inner.width), inner.y));
    }
    areas.add(inner, Target::Input);
}

/// Two buttons, right-aligned; the focused one is filled. `danger` paints a
/// focused "yes" red, for a choice that cannot be undone.
fn draw_buttons(
    frame: &mut Frame,
    area: Rect,
    areas: &mut Areas,
    (yes, no): (&str, &str),
    focus: Choice,
    danger: bool,
) {
    let yes_label = format!(" {yes} ");
    let no_label = format!(" {no} ");
    let yes_width = yes_label.chars().count() as u16;
    let no_width = no_label.chars().count() as u16;
    let total = yes_width + 2 + no_width;
    if area.width < total || area.height == 0 {
        return;
    }
    let x = area.right() - total;
    let yes_area = Rect::new(x, area.y, yes_width, 1);
    let no_area = Rect::new(x + yes_width + 2, area.y, no_width, 1);
    let style = |choice: Choice| {
        if choice == focus {
            let bg = if danger && choice == Choice::Yes {
                DANGER
            } else {
                ACCENT
            };
            Style::new()
                .bg(bg)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().bg(SELECTED).fg(Color::White)
        }
    };
    frame.render_widget(Span::styled(yes_label, style(Choice::Yes)), yes_area);
    frame.render_widget(Span::styled(no_label, style(Choice::No)), no_area);
    areas.add(yes_area, Target::Button(Choice::Yes));
    areas.add(no_area, Target::Button(Choice::No));
}

fn panel<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .border_style(Style::new().fg(BORDER))
        .padding(Padding::horizontal(1))
}

fn popup_block<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title.into().bold())
        .border_style(Style::new().fg(ACCENT))
        .padding(Padding::horizontal(1))
}

fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::new().fg(MUTED).add_modifier(Modifier::BOLD),
    ))
}

fn key_chip(key: &str) -> Span<'static> {
    Span::styled(
        format!(" {key} "),
        Style::new().bg(SELECTED).fg(Color::White).bold(),
    )
}

/// A 64-digit hex fingerprint as four lines of four groups, the way people
/// read it out to each other.
fn fingerprint_lines(hex: &str) -> Vec<String> {
    let groups: Vec<&str> = hex
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or("?"))
        .collect();
    groups.chunks(4).map(|row| row.join(" ")).collect()
}

/// A `width` × `height` rectangle in the middle of `area`, shrunk to fit.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::app::{Event, Key, Me, MouseKind, Snapshot};
    use crate::tui::pending::Owner;

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    const HEX: &str = "3f9a77c1000000000000000000000000000000000000000000000000c21dead0";

    fn app(friends: &[&str], agent: Agent) -> App {
        App::new(Snapshot {
            me: Some(Me {
                name: "winter-pc".to_string(),
                short_id: "111 222 333".to_string(),
                fingerprint: HEX.to_string(),
            }),
            friends: friends
                .iter()
                .map(|name| Friend {
                    name: name.to_string(),
                    short_id: "123 456 789".to_string(),
                    fingerprint: HEX.to_string(),
                    added: "2026-09-30".to_string(),
                    last_seen: None,
                })
                .collect(),
            agent,
            history: Vec::new(),
            owner: Owner::Background,
            listen_elsewhere: false,
            problem: None,
        })
    }

    #[test]
    fn the_friends_page_shows_the_list_and_the_selected_friends_details() {
        let mut app = app(&["alice", "bob"], Agent::Stopped);
        app.on_key(Key::Down);
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("winter-pc"), "{screen}");
        assert!(screen.contains("not receiving"), "{screen}");
        assert!(screen.contains("FRIENDS — 2"), "{screen}");
        assert!(screen.contains(" A  alice"), "{screen}");
        assert!(screen.contains("▌ B  bob"), "{screen}");
        assert!(screen.contains("@ bob"), "{screen}");
        assert!(
            screen.contains("3f9a 77c1"),
            "the fingerprint is grouped:\n{screen}"
        );
        assert!(screen.contains("2026-09-30"), "{screen}");
        assert!(screen.contains("Ctrl+Q"), "{screen}");
    }

    #[test]
    fn clicking_where_a_friend_was_drawn_selects_them() {
        let mut app = app(&["alice", "bob", "carol"], Agent::Stopped);
        render(&mut app, 120, 30);
        let (area, _) = *app
            .areas
            .targets
            .iter()
            .find(|(_, t)| *t == Target::Friend(2))
            .expect("carol was drawn");
        app.on_event(Event::Mouse {
            kind: MouseKind::Click,
            column: area.x + 3,
            row: area.y + 1,
        });
        assert_eq!(app.selected_friend().unwrap().name, "carol");
    }

    #[test]
    fn clicking_a_tab_chip_opens_it() {
        let mut app = app(&["alice"], Agent::Stopped);
        render(&mut app, 120, 30);
        let (area, _) = *app
            .areas
            .targets
            .iter()
            .find(|(_, t)| *t == Target::Tab(Tab::AddFriend))
            .unwrap();
        app.on_event(Event::Mouse {
            kind: MouseKind::Click,
            column: area.x,
            row: area.y,
        });
        assert_eq!(app.tab, Tab::AddFriend);
    }

    #[test]
    fn a_long_list_scrolls_to_keep_the_selection_visible() {
        let names: Vec<String> = (0..30).map(|i| format!("peer{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut app = app(&refs, Agent::Stopped);
        app.on_key(Key::End);
        let screen = render(&mut app, 120, 20);
        assert!(screen.contains("peer29"), "{screen}");
        assert!(!screen.contains("peer00"), "{screen}");
    }

    #[test]
    fn a_narrow_terminal_folds_the_details_into_the_middle() {
        let mut app = app(&["alice"], Agent::Stopped);
        let screen = render(&mut app, 70, 30);
        assert!(!screen.contains(" Details "), "{screen}");
        assert!(screen.contains("FINGERPRINT"), "{screen}");
    }

    #[test]
    fn no_identity_says_to_run_init() {
        let mut app = app(&[], Agent::Stopped);
        app.snapshot.me = None;
        let screen = render(&mut app, 100, 20);
        assert!(screen.contains("beam init"), "{screen}");
    }

    #[test]
    fn the_pending_tab_tells_whether_anyone_can_send() {
        let mut app = app(&[], Agent::Stopped);
        app.on_key(Key::Char('2'));
        assert!(render(&mut app, 100, 20).contains("Turn on so friends can send you files"));

        app.snapshot.agent = Agent::Running {
            receive_dir: "D:\\Downloads".to_string(),
        };
        app.link = Link::Connected;
        let screen = render(&mut app, 100, 20);
        assert!(screen.contains("● receiving"), "{screen}");
        assert!(screen.contains("D:\\Downloads"), "{screen}");
    }

    #[test]
    fn the_rename_pop_up_shows_the_box_the_error_and_buttons() {
        let mut app = app(&["alice"], Agent::Stopped);
        app.on_key(Key::Char('r'));
        app.effect_done(crate::tui::app::Done::Failed(
            "invalid peer name".to_string(),
        ));
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("Rename alice"), "{screen}");
        assert!(screen.contains("invalid peer name"), "{screen}");
        assert!(
            screen.contains(" Save ") && screen.contains(" Cancel "),
            "{screen}"
        );
        assert!(app.areas.targets.iter().any(|(_, t)| *t == Target::Input));
    }

    #[test]
    fn the_remove_pop_up_shows_who_and_their_fingerprint() {
        let mut app = app(&["alice"], Agent::Stopped);
        app.on_key(Key::Char('x'));
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("Remove alice?"), "{screen}");
        assert!(screen.contains(" Keep "), "{screen}");
        assert!(screen.contains("3f9a 77c1"), "{screen}");
    }

    #[test]
    fn a_message_shows_in_the_status_bar() {
        let mut app = app(&["alice"], Agent::Stopped);
        app.flash = Some("Copied alice's fingerprint".to_string());
        let screen = render(&mut app, 140, 20);
        assert!(screen.contains("Copied alice's fingerprint"), "{screen}");
    }

    #[test]
    fn the_help_lists_the_keys_over_the_page() {
        let mut app = app(&["alice"], Agent::Stopped);
        app.on_key(Key::Char('?'));
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains(" Keys "), "{screen}");
        assert!(screen.contains("Shift + drag"), "{screen}");
        assert!(screen.contains("beam ui cli"), "{screen}");
    }

    #[test]
    fn the_palette_lists_commands_above_its_box_and_records_them_for_clicks() {
        let mut app = app(&["alice"], Agent::Stopped);
        app.paths = |_, _| Vec::new();
        app.on_key(Key::Char(':'));
        app.on_event(Event::Paste("serv".to_string()));
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains(" Commands "), "{screen}");
        assert!(screen.contains("service status"), "{screen}");
        assert!(screen.contains(" :  serv"), "{screen}");
        assert!(screen.contains("here"), "{screen}");
        assert!(
            app.areas
                .targets
                .iter()
                .any(|(_, t)| *t == Target::Suggestion(0))
        );

        app.palette.as_mut().unwrap().input.set_text("send alice ");
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("No command matches"), "{screen}");
    }

    #[test]
    fn the_palette_shows_why_a_command_is_wrong() {
        let mut app = app(&["alice"], Agent::Stopped);
        app.on_key(Key::Char(':'));
        app.on_event(Event::Paste("peers --bogus".to_string()));
        app.on_key(Key::Enter);
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("✗"), "{screen}");
        assert!(screen.contains("--bogus"), "{screen}");
    }

    #[test]
    fn output_opens_in_a_pop_up_and_scrolls_within_range() {
        let mut app = app(&["alice"], Agent::Stopped);
        let text: Vec<String> = (1..=50).map(|i| format!("line {i}")).collect();
        app.effect_done(crate::tui::app::Done::Output {
            title: "beam peers".to_string(),
            text: text.join("\n"),
            failed: false,
        });
        app.on_key(Key::End);
        let screen = render(&mut app, 100, 24);
        assert!(screen.contains("beam peers"), "{screen}");
        assert!(screen.contains("line 50"), "{screen}");
        assert!(screen.contains("of 50"), "{screen}");
        match app.modal {
            Some(Modal::Output { scroll, .. }) => assert!(scroll < 50, "{scroll}"),
            ref other => panic!("{other:?}"),
        }
        app.on_key(Key::Up);
        let screen = render(&mut app, 100, 24);
        assert!(!screen.contains("line 50"), "Up moved at once:\n{screen}");
    }

    fn a_request(app: &mut App, who: &str) {
        app.link = Link::Connected;
        app.on_inbox(crate::tui::inbox::InboxUpdate::Request {
            id: 7,
            request: RequestView {
                peer_name: who.into(),
                fingerprint: HEX.into(),
                file_name: "report.pdf".into(),
                size: 2 * 1024 * 1024,
                resume: None,
            },
            expires_in: std::time::Duration::from_secs(300),
        });
    }

    #[test]
    fn a_waiting_request_shows_in_the_header_the_tab_and_the_friends_inbox() {
        let mut app = app(
            &["alice"],
            Agent::Running {
                receive_dir: "D:/Downloads".into(),
            },
        );
        a_request(&mut app, "alice");
        let screen = render(&mut app, 130, 30);
        assert!(screen.contains("● 1 waiting"), "{screen}");
        assert!(screen.contains(" Pending 1 "), "{screen}");
        assert!(
            screen.contains("report.pdf") && screen.contains("WAITING"),
            "{screen}"
        );

        app.on_key(Key::Char('2'));
        let screen = render(&mut app, 130, 30);
        assert!(
            screen.contains("alice wants to send report.pdf"),
            "{screen}"
        );
        assert!(screen.contains("D:/Downloads"), "{screen}");
        assert!(
            app.areas
                .targets
                .iter()
                .any(|(_, t)| *t == Target::Request(0))
        );
    }

    #[test]
    fn the_accept_pop_up_shows_who_what_how_big_and_the_fingerprint() {
        let mut app = app(
            &["alice"],
            Agent::Running {
                receive_dir: "D:/Downloads".into(),
            },
        );
        a_request(&mut app, "alice");
        app.on_key(Key::Char('2'));
        app.on_key(Key::Enter);
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("Incoming file"), "{screen}");
        assert!(
            screen.contains("alice wants to send you a file"),
            "{screen}"
        );
        assert!(screen.contains("2.0 MiB"), "{screen}");
        assert!(screen.contains("3f9a 77c1"), "{screen}");
        assert!(
            screen.contains(" Accept ") && screen.contains(" Decline "),
            "{screen}"
        );
    }

    #[test]
    fn pending_says_why_nothing_can_arrive() {
        let mut app = app(&[], Agent::Stopped);
        app.on_key(Key::Char('2'));
        assert!(render(&mut app, 100, 20).contains("Nothing can arrive while Receiving is off"));
        app.snapshot.agent = Agent::Running {
            receive_dir: "x".into(),
        };
        app.link = Link::Lost("the background agent stopped".into());
        assert!(render(&mut app, 100, 20).contains("Not connected to the receiver"));
    }

    #[test]
    fn a_friends_panel_lists_their_files_newest_first_and_when_last_seen() {
        use crate::history::{Direction, Entry, Outcome};
        let mut app = app(&["alice"], Agent::Stopped);
        let now = crate::history::unix_now();
        let entry = |file: &str, direction, outcome, ago: u64| Entry {
            at: now - ago,
            direction,
            peer: "alice".into(),
            fingerprint: HEX.into(),
            file: file.into(),
            size: 1024,
            outcome,
            note: None,
        };
        app.snapshot.history = vec![
            entry("old.zip", Direction::Sent, Outcome::Done, 3 * 86_400),
            entry("photo.png", Direction::Received, Outcome::Declined, 7200),
        ];
        app.snapshot.friends[0].last_seen = Some(now - 7200);
        let screen = render(&mut app, 130, 30);
        let photo = screen.find("photo.png").expect(&screen);
        let old = screen.find("old.zip").expect(&screen);
        assert!(
            photo < old,
            "newest first:
{screen}"
        );
        assert!(screen.contains("sent ✓"), "{screen}");
        assert!(screen.contains("declined"), "{screen}");
        assert!(
            screen.contains("LAST SEEN") && screen.contains("2 h ago"),
            "{screen}"
        );
    }

    #[test]
    fn sending_shows_a_file_box_then_a_progress_pop_up_and_a_header_pill() {
        let mut app = app(&["alice"], Agent::Stopped);
        app.paths = |_, _| vec!["report.pdf".into(), "notes/".into()];
        app.on_key(Key::Char('s'));
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("Send a file to alice"), "{screen}");
        assert!(
            screen.contains("report.pdf") && screen.contains("notes/"),
            "{screen}"
        );

        app.effect_done(crate::tui::app::Done::SendStarted {
            peer: "alice".into(),
            file: "report.pdf".into(),
        });
        app.on_send(crate::tui::sending::SendUpdate::Progress(
            crate::transfer::Progress::Transferring {
                done: 50,
                total: 100,
                path: crate::transport::PathKind::Direct,
            },
        ));
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("Sending to alice"), "{screen}");
        assert!(screen.contains("sending 50 %"), "{screen}");
        assert!(screen.contains("[Direct P2P]"), "{screen}");
        assert!(screen.contains("⇡ 50 % alice"), "{screen}");
        assert!(screen.contains("Cancel send"), "{screen}");
    }

    #[test]
    fn the_switch_sits_on_top_of_pending_and_stands_out_while_off() {
        let mut app = app(&["alice"], Agent::Stopped);
        app.on_key(Key::Char('2'));
        let screen = render(&mut app, 120, 24);
        let switch = screen.find("RECEIVING").expect(&screen);
        let nothing = screen.find("Nothing can arrive").expect(&screen);
        assert!(switch < nothing, "the switch comes first:\n{screen}");
        assert!(
            screen.contains("○  OFF") && screen.contains("press o, or click"),
            "{screen}"
        );
        assert!(
            app.areas
                .targets
                .iter()
                .any(|(_, t)| *t == Target::ReceiveSwitch)
        );

        app.switch = Switch::On;
        app.snapshot.agent = Agent::Running {
            receive_dir: "D:/Downloads".into(),
        };
        app.snapshot.owner = Owner::ThisView;
        app.link = Link::Connected;
        let screen = render(&mut app, 140, 24);
        assert!(screen.contains("●  ON"), "{screen}");
        assert!(screen.contains("saving to D:/Downloads"), "{screen}");
        assert!(screen.contains("Nothing is waiting"), "{screen}");
    }

    #[test]
    fn the_switch_names_the_agent_when_the_agent_receives() {
        let mut app = app(
            &["alice"],
            Agent::Running {
                receive_dir: "x".into(),
            },
        );
        app.on_key(Key::Char('2'));
        let screen = render(&mut app, 120, 24);
        assert!(screen.contains("●  AGENT"), "{screen}");
        assert!(
            !app.areas
                .targets
                .iter()
                .any(|(_, t)| *t == Target::ReceiveSwitch)
        );
    }

    #[test]
    fn not_receiving_in_the_header_leads_to_the_switch() {
        let mut app = app(&["alice"], Agent::Stopped);
        render(&mut app, 120, 24);
        let (area, _) = *app
            .areas
            .targets
            .iter()
            .find(|(area, t)| *t == Target::Tab(Tab::Pending) && area.y == 0)
            .expect("the header badge is clickable");
        app.on_event(Event::Mouse {
            kind: MouseKind::Click,
            column: area.x + 1,
            row: 0,
        });
        assert_eq!(app.tab, Tab::Pending);
    }

    #[test]
    fn a_tiny_terminal_does_not_panic() {
        let mut app = app(&["alice", "bob"], Agent::Stopped);
        for (w, h) in [(1, 1), (10, 4), (30, 6), (60, 8)] {
            for key in [
                Key::Char('?'),
                Key::Esc,
                Key::Char('r'),
                Key::Esc,
                Key::Char(':'),
                Key::Char('s'),
                Key::Esc,
                Key::Char('x'),
            ] {
                app.on_key(key);
                render(&mut app, w, h);
            }
            app.on_key(Key::Esc);
            app.on_key(Key::Tab);
            render(&mut app, w, h);
        }
    }
}
