//! The full-screen view that plain `beam` opens (ADR-0043).
//!
//! It is another way to look at the same `~/.beam`: every command still
//! works on its own, and `beam ui cli` makes plain `beam` print the help
//! instead. [`app`] holds the state and what each key does, [`view`] draws
//! it, and this module owns the terminal and carries out what the view asks
//! for.
//!
//! The terminal is put back on every way out — a key, an error, or a panic.
//! `ratatui::init` installs a hook that restores raw mode and the screen;
//! the hook added here also turns mouse capture and bracketed paste off
//! first, or a crashed view would leave the terminal printing mouse codes.
//!
//! Keys follow Fresh: in raw mode Ctrl+C is an ordinary key, so it copies;
//! Ctrl+Q leaves.
//!
//! A palette command that asks questions runs in the normal terminal as its
//! own `beam` process ([`run_in_terminal`]): the view leaves the screen, the
//! command prompts exactly as on the command line, and Enter brings the
//! view back. A separate process keeps the command's Ctrl+C its own
//! (ADR-0041) and cannot leave this one's state half changed.

mod add;
pub mod app;
mod browse;
mod clipboard;
mod inbox;
pub mod input;
mod pairing;
pub mod palette;
mod pending;
mod receiving;
mod send;
mod sending;
mod view;

use std::io::{self, Write};
use std::process::Command;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::identity::Store;
use crate::untrusted;
use app::{App, Done, Effect, Event, Key, MouseKind, Snapshot};
use palette::Place;

/// How often `~/.beam` is read again, so a peer paired or an agent started
/// from another terminal shows up without a key press.
const REFRESH: Duration = Duration::from_secs(2);
/// How long a message stays in the status bar.
const FLASH: Duration = Duration::from_secs(4);

/// Runs the view until the person leaves it.
pub fn run(store: &Store) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        previous(info);
    }));
    // The classic Windows console cannot select text while beam has the
    // mouse, and has no Shift+drag: start with clicks off there (ADR-0047).
    let mouse = !classic_console(|name| std::env::var_os(name).is_some());
    let result = execute!(io::stdout(), EnableBracketedPaste)
        .and_then(|()| {
            if mouse {
                execute!(io::stdout(), EnableMouseCapture)
            } else {
                Ok(())
            }
        })
        .and_then(|()| event_loop(&mut terminal, store, mouse));
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

/// How often to look for news from a pairing in the background, and to
/// redraw its countdown.
const PAIRING_TICK: Duration = Duration::from_millis(100);

/// Whether this is the classic Windows console (`conhost`) rather than
/// Windows Terminal or another modern terminal, judged by the variables
/// those set. `is_set` looks a variable up; tests pass their own.
fn classic_console(is_set: impl Fn(&str) -> bool) -> bool {
    cfg!(windows)
        && ![
            "WT_SESSION",
            "TERM_PROGRAM",
            "ConEmuANSI",
            "ALACRITTY_WINDOW_ID",
            "WEZTERM_PANE",
        ]
        .iter()
        .any(|name| is_set(name))
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    store: &Store,
    mouse: bool,
) -> io::Result<()> {
    let mut app = App::new(Snapshot::load(store));
    app.mouse = mouse;
    if !mouse {
        app.flash = Some(
            "Classic console: mouse clicks are off so you can select text. Press m to turn them on."
                .to_string(),
        );
    }
    let mut loaded = Instant::now();
    let mut flash_since: Option<Instant> = None;
    let mut worker: Option<pairing::Worker> = None;
    let mut link: Option<inbox::InboxLink> = None;
    let mut outgoing: Option<sending::Outgoing> = None;
    let mut receiver: Option<receiving::InView> = None;
    connect_inbox(store, &mut link, &mut app);
    while !app.quit {
        terminal.draw(|frame| view::draw(frame, &mut app))?;
        let mut wait = REFRESH.saturating_sub(loaded.elapsed()).min(FLASH);
        if worker.is_some() || outgoing.is_some() || receiver.is_some() {
            wait = wait.min(PAIRING_TICK);
        }
        if let Some(open) = &link {
            let mut gone = false;
            while let Ok(update) = open.updates.try_recv() {
                gone |= matches!(update, inbox::InboxUpdate::Gone { .. });
                app.on_inbox(update);
            }
            if gone {
                link = None;
            }
        }
        app.prune_pending(Instant::now());
        if let Some(on) = &receiver {
            let mut changed = false;
            let mut stopped = false;
            while let Ok(update) = on.updates.try_recv() {
                changed = true;
                stopped |= matches!(update, receiving::RecvUpdate::Stopped(_));
                app.on_switch(update);
            }
            if stopped {
                receiver = None;
            }
            if changed {
                app.refresh(Snapshot::load(store));
                loaded = Instant::now();
                connect_inbox(store, &mut link, &mut app);
            }
        }
        if let Some(running) = &outgoing {
            let mut finished = false;
            while let Ok(update) = running.updates.try_recv() {
                finished |= matches!(update, sending::SendUpdate::Finished(_));
                app.on_send(update);
            }
            if finished {
                outgoing = None;
                app.refresh(Snapshot::load(store));
                loaded = Instant::now();
            }
        }
        if let Some(running) = &worker {
            let mut finished = false;
            while let Ok(update) = running.updates.try_recv() {
                finished |= matches!(update, pairing::Update::Finished(_));
                app.on_pair(update);
            }
            if finished {
                worker = None;
                app.refresh(Snapshot::load(store));
                loaded = Instant::now();
            }
        }
        if event::poll(wait)?
            && let Some(event) = translate(event::read()?)
            && let Some(effect) = app.on_event(event)
        {
            let done = match effect {
                Effect::Run {
                    args,
                    place: Place::Terminal,
                } => run_in_terminal(terminal, store, &args, app.mouse)?,
                Effect::Run { args, .. } => {
                    // Some commands take a few seconds (`service start`
                    // waits for the agent): say so before the screen stops.
                    app.flash = Some(format!("Running beam {}…", shown(&args)));
                    terminal.draw(|frame| view::draw(frame, &mut app))?;
                    run_here(store, &args)
                }
                Effect::StartJoin { invite, name } => match start_join(store, &invite, &name) {
                    Ok(Joining::Started(started)) => {
                        worker = Some(started);
                        Done::PairingStarted
                    }
                    Ok(Joining::Done(done)) => done,
                    Err(reason) => Done::Failed(reason),
                },
                Effect::StartWait => {
                    worker = Some(pairing::Worker::wait(store.clone()));
                    Done::PairingStarted
                }
                Effect::Answer(answer) => {
                    if let Some(running) = &worker {
                        running.answer(answer.clone());
                    }
                    // Giving up on the code ends the pairing too.
                    if answer == pairing::Answer::Code(None) {
                        worker = None;
                        Done::Message("Pairing cancelled. Nothing was saved.".to_string())
                    } else {
                        Done::Nothing
                    }
                }
                Effect::AnswerTransfer { id, accept } => match &link {
                    Some(open) => {
                        open.answer(id, accept);
                        Done::Message(if accept {
                            "Accepted. The file is on its way.".to_string()
                        } else {
                            "Declined.".to_string()
                        })
                    }
                    None => Done::Failed(
                        "Lost the background agent; the request will expire as a no.".to_string(),
                    ),
                },
                Effect::StartSend { peer, path } => {
                    let path = std::path::PathBuf::from(path);
                    if outgoing.is_some() {
                        Done::Failed("A file is already going out; one at a time.".to_string())
                    } else if path.is_dir() {
                        Done::Failed("That is a folder; beam sends one file at a time.".to_string())
                    } else if !path.is_file() {
                        Done::Failed(format!(
                            "No file at {}.",
                            untrusted::name(&path.display().to_string())
                        ))
                    } else {
                        let file = path
                            .file_name()
                            .map(|n| untrusted::name(&n.to_string_lossy()))
                            .unwrap_or_default();
                        outgoing =
                            Some(sending::Outgoing::start(store.clone(), peer.clone(), path));
                        Done::SendStarted { peer, file }
                    }
                }
                Effect::ReadClipboard => match clipboard::paste() {
                    Some(text) if !text.is_empty() => {
                        app.on_event(Event::Paste(text));
                        Done::Nothing
                    }
                    Some(_) => Done::Message("The clipboard is empty.".to_string()),
                    None => Done::Failed("Could not read the clipboard.".to_string()),
                },
                Effect::SetMouse(on) => {
                    if on {
                        execute!(io::stdout(), EnableMouseCapture)?;
                    } else {
                        execute!(io::stdout(), DisableMouseCapture)?;
                    }
                    Done::Nothing
                }
                Effect::StartReceiving => {
                    if receiver.is_none() {
                        receiver = Some(receiving::InView::start(store.clone()));
                    }
                    Done::Nothing
                }
                Effect::StopReceiving => {
                    if let Some(on) = &mut receiver {
                        on.stop();
                    }
                    Done::Nothing
                }
                Effect::CancelSend => {
                    if let Some(running) = &mut outgoing {
                        running.cancel();
                    }
                    Done::Nothing
                }
                Effect::CancelPair => {
                    worker = None;
                    Done::Message("Pairing cancelled. Nothing was saved.".to_string())
                }
                effect => carry_out(store, effect),
            };
            app.effect_done(done);
            app.refresh(Snapshot::load(store));
            loaded = Instant::now();
        }
        match (&app.flash, flash_since) {
            (None, _) => flash_since = None,
            (Some(_), None) => flash_since = Some(Instant::now()),
            (Some(_), Some(since)) if since.elapsed() >= FLASH => {
                app.flash = None;
                flash_since = None;
            }
            _ => {}
        }
        if loaded.elapsed() >= REFRESH {
            app.refresh(Snapshot::load(store));
            loaded = Instant::now();
            connect_inbox(store, &mut link, &mut app);
        }
    }
    // Leaving mid-send (after saying yes to that): tell the receiver.
    if let Some(running) = outgoing {
        running.cancel_and_wait(Duration::from_secs(3));
    }
    // The switch never outlives the view: stop, telling any sender.
    if let Some(on) = receiver {
        on.stop_and_wait(Duration::from_secs(5));
    }
    Ok(())
}

/// Connects to the background agent if it runs and the view is not
/// connected yet; tried again at every refresh, so starting the agent from
/// the palette shows up within seconds.
fn connect_inbox(store: &Store, link: &mut Option<inbox::InboxLink>, app: &mut App) {
    if link.is_some() {
        return;
    }
    match crate::agent::status::read(store) {
        crate::agent::status::Running::Yes(status) => {
            *link = Some(inbox::InboxLink::connect(status.port, status.token));
            if !matches!(app.link, pending::Link::Lost(_)) {
                app.link = pending::Link::Connecting;
            }
        }
        _ => app.link = pending::Link::Off,
    }
}

/// Does what the view asked for, other than running a command.
fn carry_out(store: &Store, effect: Effect) -> Done {
    let fail = |e: &dyn std::fmt::Display| untrusted::text(&e.to_string());
    let result = match effect {
        Effect::Rename { from, to } => (|| {
            let mut known = store.load_known_peers().map_err(|e| fail(&e))?;
            known.rename(&from, &to).map_err(|e| fail(&e))?;
            store.save_known_peers(&known).map_err(|e| fail(&e))?;
            Ok(format!("Renamed {from} to {to}."))
        })(),
        Effect::Remove { name } => (|| {
            let mut known = store.load_known_peers().map_err(|e| fail(&e))?;
            known.remove(&name).map_err(|e| fail(&e))?;
            store.save_known_peers(&known).map_err(|e| fail(&e))?;
            Ok(format!("Removed {name}."))
        })(),
        Effect::Copy { what, text } => Ok(match clipboard::copy(&text) {
            clipboard::Copied::System => format!("Copied {what}."),
            clipboard::Copied::Terminal => format!("Copied {what} (through the terminal)."),
        }),
        Effect::Run { args, .. } => return run_here(store, &args),
        Effect::CreateIdentity => return create_identity(store),
        Effect::UpdateLocation { invite } => return update_location(store, &invite),
        // Pairing effects need the worker, which the event loop holds.
        Effect::StartJoin { .. }
        | Effect::StartWait
        | Effect::Answer(_)
        | Effect::CancelPair
        | Effect::AnswerTransfer { .. }
        | Effect::StartSend { .. }
        | Effect::CancelSend
        | Effect::StartReceiving
        | Effect::StopReceiving
        | Effect::ReadClipboard
        | Effect::SetMouse(_) => {
            return Done::Nothing;
        }
    };
    match result {
        Ok(message) => Done::Message(message),
        Err(reason) => Done::Failed(reason),
    }
}

/// First run: makes this device's identity exactly as `beam init` does,
/// named after the computer. Never replaces one that exists.
fn create_identity(store: &Store) -> Done {
    if store.has_identity() {
        return Done::Message("This device already has its identity.".to_string());
    }
    let made = crate::identity::Identity::generate(&crate::cli::identity_cmds::hostname())
        .map_err(|e| e.to_string())
        .and_then(|identity| {
            store
                .save_identity(&identity, false)
                .map(|()| identity.short_id().grouped())
                .map_err(|e| e.to_string())
        });
    match made {
        Ok(short_id) => Done::IdentityCreated { short_id },
        Err(e) => Done::Failed(format!(
            "Could not create the identity: {}",
            untrusted::text(&e)
        )),
    }
}

/// What came of Pair on the Add friend form.
enum Joining {
    Started(pairing::Worker),
    /// No pairing needed: the invite is for a friend already paired.
    Done(Done),
}

/// Checks the form before any network: a damaged invite, this device's own,
/// a name that is taken. An invite for a friend already paired only updates
/// where to find them, asking first if it changes their relay (ADR-0038), as
/// `beam pair` does.
fn start_join(store: &Store, text: &str, name: &str) -> Result<Joining, String> {
    let fail = |e: &dyn std::fmt::Display| untrusted::lines(&e.to_string());
    let invite: crate::invite::Invite = text.parse().map_err(|e| fail(&e))?;
    let identity = store.load_identity().map_err(|e| fail(&e))?;
    if invite.key == identity.verifying_key() {
        return Err("That is this device's own invite. Ask them for theirs.".to_string());
    }
    let known = store.load_known_peers().map_err(|e| fail(&e))?;
    if let Some(peer) = known.lookup_key(&invite.key) {
        let config = crate::config::Config::load(&store.config_path()).map_err(|e| fail(&e))?;
        let old = peer.attr(crate::invite::RELAY_ATTR).map(str::to_string);
        let new = crate::invite::relay_attr(&invite, &config.relay);
        if old != new {
            let describe = |relay: &Option<String>| match relay {
                Some(url) => untrusted::text(url),
                None => format!("your own relay ({})", config.relay),
            };
            return Ok(Joining::Done(Done::AskRelay {
                name: untrusted::name(&peer.name),
                fingerprint: peer.fingerprint().hex(),
                old: describe(&old),
                new: describe(&new),
                invite: text.to_string(),
            }));
        }
        return Ok(Joining::Done(update_location(store, text)));
    }
    crate::pairing::check_name(&known, name).map_err(|e| fail(&e))?;
    Ok(Joining::Started(pairing::Worker::join(
        store.clone(),
        invite,
        name.to_string(),
    )))
}

/// Saves where an already-paired friend is now. Never touches the key.
fn update_location(store: &Store, text: &str) -> Done {
    let fail = |e: &dyn std::fmt::Display| Done::Failed(untrusted::lines(&e.to_string()));
    let invite: crate::invite::Invite = match text.parse() {
        Ok(invite) => invite,
        Err(e) => return fail(&e),
    };
    let config = match crate::config::Config::load(&store.config_path()) {
        Ok(config) => config,
        Err(e) => return fail(&e),
    };
    let mut known = match store.load_known_peers() {
        Ok(known) => known,
        Err(e) => return fail(&e),
    };
    let Some(peer) = known.lookup_key_mut(&invite.key) else {
        return Done::Failed("That friend is no longer paired.".to_string());
    };
    let name = untrusted::name(&peer.name);
    crate::invite::remember(peer, &invite, &config.relay);
    match store.save_known_peers(&known) {
        Ok(()) => Done::Message(format!(
            "{name} is already your friend. Updated where to find them; their key is unchanged."
        )),
        Err(e) => fail(&e),
    }
}

/// `args` with this view's beam home in front, unless they name one.
fn with_beam_dir(store: &Store, args: &[String]) -> Vec<String> {
    let named = args
        .iter()
        .any(|a| a == "--beam-dir" || a.starts_with("--beam-dir="));
    let mut out = Vec::with_capacity(args.len() + 2);
    if !named {
        out.push("--beam-dir".to_string());
        out.push(store.dir().display().to_string());
    }
    out.extend(args.iter().cloned());
    out
}

/// A command as the person typed it, for titles and messages.
fn shown(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.chars().any(char::is_whitespace) {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs a command that only prints, inside this process, and returns what
/// it printed for a pop-up. It gets no input: a question it might ask reads
/// end of file, which every beam prompt takes as no.
fn run_here(store: &Store, args: &[String]) -> Done {
    let mut input: &[u8] = &[];
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = {
        let mut io = crate::cli::Io {
            input: &mut input,
            out: &mut out,
            err: &mut err,
        };
        crate::cli::execute(with_beam_dir(store, args), &mut io)
    };
    out.extend_from_slice(&err);
    // The CLI already cleans what it prints; this is the second line.
    let text = String::from_utf8_lossy(&out)
        .lines()
        .map(untrusted::text)
        .collect::<Vec<_>>()
        .join("\n");
    let title = format!("beam {}", shown(args));
    if text.trim().is_empty() {
        return if code == crate::cli::EXIT_OK {
            Done::Message(format!("{title}: done."))
        } else {
            Done::Failed(format!("{title} failed."))
        };
    }
    Done::Output {
        title,
        text,
        failed: code != crate::cli::EXIT_OK,
    }
}

/// Steps out of the view to run a command in the normal terminal, as its
/// own `beam` process, then waits for Enter and comes back.
fn run_in_terminal(
    terminal: &mut ratatui::DefaultTerminal,
    store: &Store,
    args: &[String],
    mouse: bool,
) -> io::Result<Done> {
    let mut stdout = io::stdout();
    execute!(
        stdout,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    disable_raw_mode()?;
    terminal.show_cursor()?;

    let line = shown(args);
    writeln!(stdout, "\n── beam {line} ──\n")?;
    let status = {
        // Ctrl+C now reaches both processes. It is the command's to act on
        // (send and listen tell the other side, ADR-0041); this guard only
        // keeps it from ending the view as well.
        let _guard = CtrlCGuard::new();
        let result = std::env::current_exe()
            .and_then(|exe| Command::new(exe).args(with_beam_dir(store, args)).status());
        write!(stdout, "\nPress Enter to go back to beam. ")?;
        stdout.flush()?;
        let mut answer = String::new();
        let _ = io::stdin().read_line(&mut answer);
        result
    };

    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    if mouse {
        execute!(stdout, EnableMouseCapture)?;
    }
    terminal.clear()?;
    Ok(match status {
        Ok(status) if status.success() => Done::Message(format!("beam {line}: finished.")),
        Ok(_) => Done::Failed(format!("beam {line} stopped with an error.")),
        Err(e) => Done::Failed(format!("Could not start beam {line}: {e}")),
    })
}

/// While alive, Ctrl+C does not end this process. tokio's listener replaces
/// the default "exit" reaction for as long as it exists (on Windows; on
/// Unix it stays replaced, which is harmless: in raw mode Ctrl+C is a key).
/// Fields drop in order: the listener before the runtime it belongs to.
struct CtrlCGuard {
    #[cfg(windows)]
    _listener: Option<tokio::signal::windows::CtrlC>,
    #[cfg(unix)]
    _listener: Option<tokio::signal::unix::Signal>,
    _runtime: Option<tokio::runtime::Runtime>,
}

impl CtrlCGuard {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok();
        let listener = runtime.as_ref().and_then(|rt| {
            let _entered = rt.enter();
            #[cfg(windows)]
            let listener = tokio::signal::windows::ctrl_c().ok();
            #[cfg(unix)]
            let listener =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();
            listener
        });
        Self {
            _listener: listener,
            _runtime: runtime,
        }
    }
}

/// The terminal's event as one of ours, or `None` for what the view does
/// not use.
fn translate(event: event::Event) -> Option<Event> {
    match event {
        event::Event::Key(key) => translate_key(key).map(Event::Key),
        event::Event::Paste(text) => Some(Event::Paste(text)),
        event::Event::Mouse(mouse) => translate_mouse(mouse),
        _ => None,
    }
}

fn translate_mouse(mouse: MouseEvent) -> Option<Event> {
    let kind = match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => MouseKind::Click,
        MouseEventKind::ScrollUp => MouseKind::ScrollUp,
        MouseEventKind::ScrollDown => MouseKind::ScrollDown,
        _ => return None,
    };
    Some(Event::Mouse {
        kind,
        column: mouse.column,
        row: mouse.row,
    })
}

/// Windows also reports key releases; only presses count.
fn translate_key(key: KeyEvent) -> Option<Key> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if ctrl || alt {
        return match key.code {
            KeyCode::Char('c' | 'C') if ctrl => Some(Key::Copy),
            // The classic Windows console delivers these as keys; Windows
            // Terminal pastes by itself and never sends them.
            KeyCode::Char('v' | 'V') if ctrl => Some(Key::Paste),
            KeyCode::Insert if ctrl => Some(Key::Copy),
            KeyCode::Char('q' | 'Q') if ctrl => Some(Key::Quit),
            KeyCode::Char('p' | 'P') if ctrl => Some(Key::Palette),
            KeyCode::Char('w' | 'W') if ctrl => Some(Key::DeleteWord),
            KeyCode::Backspace => Some(Key::DeleteWord),
            KeyCode::Left => Some(Key::WordLeft),
            KeyCode::Right => Some(Key::WordRight),
            _ => None,
        };
    }
    if key.code == KeyCode::Insert && key.modifiers.contains(KeyModifiers::SHIFT) {
        return Some(Key::Paste);
    }
    Some(match key.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Char(c) => Key::Char(c),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Identity, Peer};

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn ctrl_c_copies_ctrl_q_quits_and_other_ctrl_keys_are_ignored() {
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(
            translate_key(press(KeyCode::Char('c'), ctrl)),
            Some(Key::Copy)
        );
        assert_eq!(
            translate_key(press(KeyCode::Char('q'), ctrl)),
            Some(Key::Quit)
        );
        assert_eq!(translate_key(press(KeyCode::Char('a'), ctrl)), None);
        assert_eq!(
            translate_key(press(KeyCode::Left, ctrl)),
            Some(Key::WordLeft)
        );
        assert_eq!(
            translate_key(press(KeyCode::Char('c'), KeyModifiers::NONE)),
            Some(Key::Char('c'))
        );
        assert_eq!(
            translate_key(press(KeyCode::Char('C'), KeyModifiers::SHIFT)),
            Some(Key::Char('C'))
        );
    }

    #[test]
    fn the_classic_console_keys_paste_and_copy() {
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(
            translate_key(press(KeyCode::Char('v'), ctrl)),
            Some(Key::Paste)
        );
        assert_eq!(
            translate_key(press(KeyCode::Insert, KeyModifiers::SHIFT)),
            Some(Key::Paste)
        );
        assert_eq!(translate_key(press(KeyCode::Insert, ctrl)), Some(Key::Copy));
        assert_eq!(
            translate_key(press(KeyCode::Char('C'), ctrl | KeyModifiers::SHIFT)),
            Some(Key::Copy)
        );
    }

    #[test]
    fn the_classic_console_is_told_apart_from_modern_terminals() {
        let none = |_: &str| false;
        let windows_terminal = |name: &str| name == "WT_SESSION";
        let vs_code = |name: &str| name == "TERM_PROGRAM";
        assert_eq!(classic_console(none), cfg!(windows));
        assert!(!classic_console(windows_terminal));
        assert!(!classic_console(vs_code));
    }

    #[test]
    fn a_key_release_is_not_a_second_press() {
        let mut key = press(KeyCode::Down, KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(translate_key(key), None);
    }

    #[test]
    fn left_clicks_and_the_wheel_count_and_mouse_moves_do_not() {
        let mouse = |kind| MouseEvent {
            kind,
            column: 4,
            row: 7,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            translate_mouse(mouse(MouseEventKind::Down(MouseButton::Left))),
            Some(Event::Mouse {
                kind: MouseKind::Click,
                column: 4,
                row: 7
            })
        );
        assert!(translate_mouse(mouse(MouseEventKind::ScrollDown)).is_some());
        assert_eq!(translate_mouse(mouse(MouseEventKind::Moved)), None);
        assert_eq!(
            translate_mouse(mouse(MouseEventKind::Down(MouseButton::Right))),
            None
        );
    }

    #[test]
    fn rename_and_remove_change_known_peers() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let mut known = store.load_known_peers().unwrap();
        let key = Identity::generate("x").unwrap().verifying_key();
        known.add(Peer::new("alice", key)).unwrap();
        store.save_known_peers(&known).unwrap();

        let bad = carry_out(
            &store,
            Effect::Rename {
                from: "alice".to_string(),
                to: "no spaces".to_string(),
            },
        );
        assert!(
            matches!(&bad, Done::Failed(r) if r.contains("invalid peer name")),
            "{bad:?}"
        );

        let done = carry_out(
            &store,
            Effect::Rename {
                from: "alice".to_string(),
                to: "ali".to_string(),
            },
        );
        assert!(matches!(done, Done::Message(_)), "{done:?}");
        let known = store.load_known_peers().unwrap();
        assert_eq!(known.lookup("ali").unwrap().public_key, key);

        let done = carry_out(
            &store,
            Effect::Remove {
                name: "ali".to_string(),
            },
        );
        assert!(matches!(done, Done::Message(_)), "{done:?}");
        assert!(store.load_known_peers().unwrap().is_empty());
    }

    #[test]
    fn a_command_run_here_uses_this_views_home_and_shows_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let done = run_here(&store, &["peers".to_string()]);
        match done {
            Done::Output {
                title,
                text,
                failed,
            } => {
                assert_eq!(title, "beam peers");
                assert!(text.contains("No paired peers yet"), "{text}");
                assert!(!failed);
            }
            other => panic!("{other:?}"),
        }
        // A failure is marked, and its message shown.
        let done = run_here(&store, &["whoami".to_string()]);
        assert!(
            matches!(&done, Done::Output { failed: true, text, .. } if text.contains("beam init")),
            "{done:?}"
        );
    }

    fn invite_for(key: ed25519_dalek::VerifyingKey) -> String {
        crate::invite::Invite {
            key,
            relay: None,
            addrs: vec!["127.0.0.1:7820".parse().unwrap()],
        }
        .to_string()
    }

    #[test]
    fn pair_checks_the_invite_and_the_name_before_any_network() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let me = Identity::generate("me").unwrap();
        store.save_identity(&me, false).unwrap();

        let err = start_join(&store, "beam1notreal", "dana").err().unwrap();
        assert!(!err.is_empty());

        let own = invite_for(me.verifying_key());
        let err = start_join(&store, &own, "dana").err().unwrap();
        assert!(err.contains("own invite"), "{err}");

        // A friend already paired, same relay: only where to find them.
        let bob = Identity::generate("bob").unwrap();
        let mut known = store.load_known_peers().unwrap();
        known.add(Peer::new("bob", bob.verifying_key())).unwrap();
        store.save_known_peers(&known).unwrap();
        let bobs = invite_for(bob.verifying_key());
        match start_join(&store, &bobs, "whatever") {
            Ok(Joining::Done(Done::Message(m))) => {
                assert!(m.contains("already your friend"), "{m}")
            }
            Ok(Joining::Done(other)) => panic!("{other:?}"),
            Ok(Joining::Started(_)) => panic!("started a pairing"),
            Err(e) => panic!("{e}"),
        }

        // A new device under a taken name is refused before connecting.
        let carol = Identity::generate("carol").unwrap();
        let carols = invite_for(carol.verifying_key());
        let err = start_join(&store, &carols, "bob").err().unwrap();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn the_first_run_makes_an_identity_once_and_never_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("beam"));
        let Done::IdentityCreated { short_id } = create_identity(&store) else {
            panic!("not created")
        };
        let identity = store.load_identity().unwrap();
        assert_eq!(identity.short_id().grouped(), short_id);
        assert!(Snapshot::load(&store).me.is_some());
        assert!(!Snapshot::load(&store).needs_setup);

        assert!(matches!(create_identity(&store), Done::Message(_)));
        assert_eq!(
            store.load_identity().unwrap().fingerprint(),
            identity.fingerprint(),
            "the key is never replaced"
        );
    }

    #[test]
    fn the_home_is_added_once() {
        let store = Store::new("/h");
        let args = with_beam_dir(&store, &["peers".to_string()]);
        assert_eq!(args[0], "--beam-dir");
        assert_eq!(args.last().unwrap(), "peers");
        let own = vec![
            "--beam-dir".to_string(),
            "/x".to_string(),
            "peers".to_string(),
        ];
        assert_eq!(with_beam_dir(&store, &own), own);
    }
}
