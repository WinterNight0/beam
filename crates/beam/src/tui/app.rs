//! What the full-screen view shows, and what each key or click does to it.
//!
//! Nothing here touches the terminal or the disk: an event goes in, the
//! state changes, and anything that must happen outside — saving a rename,
//! copying to the clipboard — comes back as an [`Effect`] for [`super`] to
//! carry out. That split is what lets the behaviour be unit tested.

use std::time::Instant;

use ratatui::layout::{Position, Rect};

use super::add::{self, AddForm, Field};
use super::browse::{Browser, Fs};
use super::inbox::RequestView;
use super::input::{Edit, Input};
use super::palette::{self, Palette, Place, Suggestion};
use super::pending::{Link, Owner, Pending, Receiving, Switch};
use super::send::Sending;
use crate::agent::status::Running;
use crate::identity::{MAX_NAME_LEN, Store, StoreError};
use crate::untrusted;

/// The tabs along the top, as in Discord's Friends page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Friends,
    Pending,
    AddFriend,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Friends, Tab::Pending, Tab::AddFriend];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Friends => "Friends",
            Tab::Pending => "Pending",
            Tab::AddFriend => "Add friend",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    fn step(self, by: isize) -> Tab {
        let len = Self::ALL.len() as isize;
        Self::ALL[(self.index() as isize + by).rem_euclid(len) as usize]
    }
}

/// A key press, already translated from the terminal's event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    Tab,
    BackTab,
    Enter,
    Esc,
    Backspace,
    Delete,
    DeleteWord,
    /// Ctrl+C. In raw mode the terminal sends it as a key, not a signal, so
    /// it can mean copy, as in Fresh and most editors.
    Copy,
    /// Ctrl+Q: leave beam, from anywhere.
    Quit,
    /// Ctrl+P: open (or close) the command palette, like `:`.
    Palette,
    PageUp,
    PageDown,
    Char(char),
}

/// What the mouse did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Click,
    ScrollUp,
    ScrollDown,
}

/// Something that happened at the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Key(Key),
    /// Text pasted in one piece (bracketed paste).
    Paste(String),
    Mouse {
        kind: MouseKind,
        column: u16,
        row: u16,
    },
}

/// What the view needs done outside itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Rename {
        from: String,
        to: String,
    },
    Remove {
        name: String,
    },
    /// Put `text` on the clipboard; `what` names it for the message.
    Copy {
        what: String,
        text: String,
    },
    /// Run a beam command typed in the palette, `Here` or in the
    /// `Terminal` (see [`palette::place`]).
    Run {
        args: Vec<String>,
        place: Place,
    },
    /// Pair with the device in a pasted invite, saving it as `name`.
    StartJoin {
        invite: String,
        name: String,
    },
    /// Show this device's invite and a code, and wait for one attempt.
    StartWait,
    /// Answer the pairing thread's question.
    Answer(super::pairing::Answer),
    /// Stop the pairing; nothing is saved.
    CancelPair,
    /// Save where an already-paired device is now, after a yes to its new
    /// relay (ADR-0038).
    UpdateLocation {
        invite: String,
    },
    /// Accept or decline request `id` from the background agent.
    AnswerTransfer {
        id: u64,
        accept: bool,
    },
    /// Send the file at `path` to the friend called `peer`.
    StartSend {
        peer: String,
        path: String,
    },
    /// Stop the send in progress; the receiver is told (ADR-0041).
    CancelSend,
    /// Turn the Receiving switch on: listen inside the view (ADR-0044).
    StartReceiving,
    /// Turn it off; a sender mid-transfer is told (ADR-0041).
    StopReceiving,
    /// First run: make this device's identity, as `beam init` does.
    CreateIdentity,
}

/// How an [`Effect`] turned out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// For the status bar.
    Message(String),
    /// Why it did not work.
    Failed(String),
    /// What a command printed, for a pop-up.
    Output {
        title: String,
        text: String,
        failed: bool,
    },
    /// Nothing to show (an answer was passed on).
    Nothing,
    /// A pairing started; its pop-ups follow.
    PairingStarted,
    /// A send started.
    SendStarted { peer: String, file: String },
    /// The identity was just made (first run).
    IdentityCreated { short_id: String },
    /// The invite is for a friend already paired, and moves them to another
    /// relay: ask first (ADR-0038).
    AskRelay {
        name: String,
        fingerprint: String,
        old: String,
        new: String,
        invite: String,
    },
}

/// This device, for the header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Me {
    pub name: String,
    pub short_id: String,
    pub fingerprint: String,
}

/// One paired peer, ready to draw. Every string is already safe for the
/// terminal (ADR-0034): `known_peers` can be edited by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Friend {
    /// Also the key for rename and remove: `known_peers` only holds names
    /// that pass `validate_name`, which cleaning leaves unchanged.
    pub name: String,
    pub short_id: String,
    /// The fingerprint's hex, without the `SHA256:` prefix.
    pub fingerprint: String,
    pub added: String,
    /// When a transfer with them last got an answer (unix seconds): what
    /// "last seen" means without a server (ADR-0043).
    pub last_seen: Option<u64>,
}

/// Whether the background agent is running (ADR-0042).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Agent {
    Running { receive_dir: String },
    Unreadable,
    Stopped,
}

/// Everything read from `~/.beam` for one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// `None` until `beam init` has been run.
    pub me: Option<Me>,
    /// No identity file at all: the first run, which offers to make one.
    pub needs_setup: bool,
    pub friends: Vec<Friend>,
    pub agent: Agent,
    /// What came and went, oldest first, its strings cleaned for drawing.
    pub history: Vec<crate::history::Entry>,
    /// Who runs the agent's receiver, when one runs.
    pub owner: Owner,
    /// Whether `beam listen` runs in this beam home.
    pub listen_elsewhere: bool,
    /// Why something could not be read, shown in the status line.
    pub problem: Option<String>,
}

impl Snapshot {
    /// Reads the identity, the paired peers and the agent's status.
    pub fn load(store: &Store) -> Self {
        let mut problem = None;
        let mut needs_setup = false;
        let me = match store.load_identity() {
            Ok(identity) => Some(Me {
                name: untrusted::name(identity.comment()),
                short_id: identity.short_id().grouped(),
                fingerprint: identity.fingerprint().hex(),
            }),
            Err(StoreError::NoIdentity) => {
                needs_setup = true;
                None
            }
            Err(e) => {
                problem = Some(untrusted::text(&e.to_string()));
                None
            }
        };
        let history: Vec<crate::history::Entry> = crate::history::read(store)
            .into_iter()
            .map(|mut e| {
                e.peer = untrusted::name(&e.peer);
                e.file = untrusted::name(&e.file);
                e.fingerprint = untrusted::text(&e.fingerprint);
                e.note = e.note.as_deref().map(untrusted::text);
                e
            })
            .collect();
        // Seen = they answered: a transfer that only failed to reach them
        // says nothing about when they were there.
        let last_seen = |fingerprint: &str| {
            history
                .iter()
                .rev()
                .find(|e| {
                    e.fingerprint == fingerprint && e.outcome != crate::history::Outcome::Failed
                })
                .map(|e| e.at)
        };
        let friends = match store.load_known_peers() {
            Ok(known) => known
                .peers()
                .iter()
                .map(|p| {
                    let fingerprint = p.fingerprint().hex();
                    Friend {
                        name: untrusted::name(&p.name),
                        short_id: p.short_id().grouped(),
                        last_seen: last_seen(&fingerprint),
                        fingerprint,
                        added: p.added_date(),
                    }
                })
                .collect(),
            Err(e) => {
                problem.get_or_insert_with(|| untrusted::text(&e.to_string()));
                Vec::new()
            }
        };
        let mut owner = Owner::Background;
        let agent = match crate::agent::status::read(store) {
            Running::Yes(status) => {
                if status.in_view {
                    owner = if status.pid == std::process::id() {
                        Owner::ThisView
                    } else {
                        Owner::OtherView
                    };
                }
                Agent::Running {
                    receive_dir: untrusted::name(&status.receive_dir),
                }
            }
            Running::Unreadable => Agent::Unreadable,
            Running::No => Agent::Stopped,
        };
        let listen_elsewhere = !matches!(
            crate::listen_status::read(store),
            crate::listen_status::Listening::No
        );
        Self {
            me,
            needs_setup,
            friends,
            agent,
            history,
            owner,
            listen_elsewhere,
            problem,
        }
    }
}

/// The two answers of a yes/no pop-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Yes,
    No,
}

/// A pop-up over the page. While one is open, keys go to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modal {
    Help,
    Rename {
        from: String,
        input: Input,
        error: Option<String>,
    },
    /// Starts on `No`: Enter alone keeps the friend.
    Remove {
        name: String,
        fingerprint: String,
        focus: Choice,
    },
    /// What a command run from the palette printed.
    Output {
        title: String,
        text: String,
        /// Lines scrolled off the top; the view keeps it in range.
        scroll: u16,
        failed: bool,
    },
    /// Type the code shown on the other device.
    EnterCode {
        input: Input,
        error: Option<String>,
    },
    /// Pairing is working (connecting, waiting for the other person).
    Busy {
        text: String,
    },
    /// This device's invite and code, while waiting for the other person.
    ShowInvite {
        invite: String,
        code: String,
        expires: Instant,
        /// A device connected (its short fingerprint); the code is used up.
        attempt: Option<String>,
    },
    /// Do the fingerprints match? Starts on `No`. Fingerprints are hex.
    ConfirmPair {
        name: String,
        peer: String,
        own: String,
        focus: Choice,
    },
    /// Which file to send: the browser (ADR-0045).
    Browse(Box<Browser>),
    /// How the send in [`App::sending`] is going.
    SendStatus,
    /// A file is still going out or coming in: leave anyway? Starts on `No`.
    ConfirmQuit {
        focus: Choice,
    },
    /// A file is arriving: turn Receiving off anyway? Starts on `No`.
    ConfirmStop {
        focus: Choice,
    },
    /// May this friend send this file? Starts on `No` (Decline).
    Accept {
        id: u64,
        request: RequestView,
        expires: Instant,
        focus: Choice,
    },
    /// An invite moves a paired friend to another relay. Starts on `No`.
    RelayChange {
        name: String,
        fingerprint: String,
        old: String,
        new: String,
        invite: String,
        focus: Choice,
    },
}

/// A clickable thing the view drew, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Tab(Tab),
    /// A friend in the list, by index.
    Friend(usize),
    Button(Choice),
    /// The text box of the open pop-up.
    Input,
    /// A line in the palette's list, by index.
    Suggestion(usize),
    /// The palette's own text box.
    PaletteInput,
    /// A part of the Add friend form.
    Field(Field),
    /// A waiting request in the Pending tab, by index.
    Request(usize),
    /// A place on the browser's left, by index.
    Place(usize),
    /// The Receiving switch at the top of Pending.
    ReceiveSwitch,
}

/// Where the clickable things are on screen, as of the last frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Areas {
    pub targets: Vec<(Rect, Target)>,
    /// The friend list's inside, for the scroll wheel.
    pub list: Rect,
}

impl Areas {
    pub fn add(&mut self, area: Rect, target: Target) {
        self.targets.push((area, target));
    }

    fn at(&self, column: u16, row: u16) -> Option<(Rect, Target)> {
        let point = Position::new(column, row);
        // Last drawn is on top: a pop-up's buttons win over the page below.
        self.targets
            .iter()
            .rev()
            .find(|(area, _)| area.contains(point))
            .copied()
    }
}

/// The whole state of the view.
#[derive(Debug)]
pub struct App {
    pub snapshot: Snapshot,
    pub tab: Tab,
    /// Index into `snapshot.friends`.
    pub selected: usize,
    /// The first friend shown in the list, kept between frames.
    pub list_offset: usize,
    pub modal: Option<Modal>,
    /// A short message for the status line ("Copied …").
    pub flash: Option<String>,
    pub areas: Areas,
    /// The command palette, while it is open.
    pub palette: Option<Palette>,
    /// Commands run from the palette this session, oldest first.
    pub history: Vec<String>,
    /// Lists files for completion; tests swap in their own.
    pub paths: fn(&str, bool) -> Vec<String>,
    /// The disk, for the file browser; tests swap in their own.
    pub fs: Fs,
    /// Where the browser was last left.
    pub last_dir: Option<std::path::PathBuf>,
    /// The Add friend form.
    pub add: AddForm,
    /// A friend to select once they appear (just paired), by fingerprint.
    pub pending_select: Option<String>,
    /// Requests from the background agent, waiting for an answer.
    pub pending: Vec<Pending>,
    /// Index into `pending`.
    pub pending_selected: usize,
    /// What the agent is receiving now.
    pub receiving: Option<Receiving>,
    /// How the view stands with the agent.
    pub link: Link,
    /// The file going out now, or the one that just ended.
    pub sending: Option<Sending>,
    /// The Receiving switch (ADR-0044).
    pub switch: Switch,
    /// The first-run card's highlighted button: starts on "Create it".
    pub setup_focus: Choice,
    pub quit: bool,
}

/// How many palette commands are remembered.
const HISTORY: usize = 50;

impl App {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot,
            tab: Tab::Friends,
            selected: 0,
            list_offset: 0,
            modal: None,
            flash: None,
            areas: Areas::default(),
            palette: None,
            history: Vec::new(),
            paths: palette::list_paths,
            fs: Fs::real(),
            last_dir: None,
            add: AddForm::default(),
            pending_select: None,
            pending: Vec::new(),
            pending_selected: 0,
            receiving: None,
            link: Link::Off,
            sending: None,
            switch: Switch::Off,
            setup_focus: Choice::Yes,
            quit: false,
        }
    }

    /// The palette's list for what is typed in it now.
    pub fn suggestions(&self) -> Vec<Suggestion> {
        match &self.palette {
            Some(palette) => self.suggestions_for(palette),
            None => Vec::new(),
        }
    }

    fn suggestions_for(&self, palette: &Palette) -> Vec<Suggestion> {
        let friends: Vec<String> = self
            .snapshot
            .friends
            .iter()
            .map(|f| f.name.clone())
            .collect();
        palette::suggest(palette.input.text(), &friends, &self.history, &self.paths)
    }

    /// The friend under the cursor, if there is one.
    pub fn selected_friend(&self) -> Option<&Friend> {
        self.snapshot.friends.get(self.selected)
    }

    /// Takes in a fresh read of `~/.beam`, keeping the cursor on the same
    /// friend if they are still there.
    pub fn refresh(&mut self, snapshot: Snapshot) {
        let keep = self
            .pending_select
            .take()
            .or_else(|| self.selected_friend().map(|f| f.fingerprint.clone()));
        self.snapshot = snapshot;
        self.selected = keep
            .and_then(|fp| {
                self.snapshot
                    .friends
                    .iter()
                    .position(|f| f.fingerprint == fp)
            })
            .unwrap_or(self.selected)
            .min(self.snapshot.friends.len().saturating_sub(1));
    }

    /// Handles one event. Anything to be done outside comes back.
    pub fn on_event(&mut self, event: Event) -> Option<Effect> {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Paste(text) => {
                if let Some(palette) = &mut self.palette {
                    palette.input.paste(&text);
                    palette.typed();
                } else if let Some(
                    Modal::Rename { input, error, .. } | Modal::EnterCode { input, error },
                ) = &mut self.modal
                {
                    input.paste(&text);
                    *error = None;
                } else if let Some(Modal::Browse(b)) = &mut self.modal {
                    b.pane = super::browse::Pane::Files;
                    b.filter = Input::new(palette::MAX_LINE);
                    b.filter.paste(&text);
                    b.filtered();
                } else if self.modal.is_none() && self.form_has_keys() {
                    self.paste_into_form(&text);
                }
                None
            }
            Event::Mouse { kind, column, row } => self.on_mouse(kind, column, row),
        }
    }

    /// How an [`Effect`] turned out.
    pub fn effect_done(&mut self, done: Done) {
        match done {
            Done::Output {
                title,
                text,
                failed,
            } => {
                self.modal = Some(Modal::Output {
                    title,
                    text,
                    scroll: 0,
                    failed,
                });
            }
            Done::Message(message) => {
                if matches!(
                    self.modal,
                    Some(Modal::Rename { .. } | Modal::Remove { .. })
                ) {
                    self.modal = None;
                }
                self.flash = Some(message);
            }
            Done::Failed(reason) => match &mut self.modal {
                // Stay open, so the name (or the path) can be fixed.
                Some(Modal::Rename { error, .. }) => *error = Some(reason),
                Some(Modal::Browse(b)) => b.error = Some(reason),
                // A pairing that could not start: say why under the form.
                None if self.tab == Tab::AddFriend => self.add.error = Some(reason),
                _ => {
                    self.modal = None;
                    self.flash = Some(reason);
                }
            },
            Done::Nothing => {}
            Done::IdentityCreated { short_id } => {
                self.flash = Some(format!(
                    "This device is ready (Short ID {short_id}). Next: add a friend."
                ));
                self.tab = Tab::AddFriend;
                self.add.focus = Some(Field::Invite);
            }
            Done::SendStarted { peer, file } => {
                self.sending = Some(Sending {
                    peer,
                    file,
                    stage: super::send::Stage::Starting,
                    result: None,
                });
                self.modal = Some(Modal::SendStatus);
            }
            Done::PairingStarted => {
                self.modal = Some(Modal::Busy {
                    text: "Starting…".to_string(),
                });
            }
            Done::AskRelay {
                name,
                fingerprint,
                old,
                new,
                invite,
            } => {
                self.modal = Some(Modal::RelayChange {
                    name,
                    fingerprint,
                    old,
                    new,
                    invite,
                    focus: Choice::No,
                });
            }
        }
    }

    pub fn on_key(&mut self, key: Key) -> Option<Effect> {
        if key == Key::Quit {
            if matches!(self.modal, Some(Modal::ConfirmQuit { .. })) {
                self.quit = true;
            } else {
                self.request_quit();
            }
            return None;
        }
        self.flash = None;
        if let Some(palette) = self.palette.take() {
            return self.on_palette_key(palette, key);
        }
        match self.modal.take() {
            Some(modal @ Modal::Accept { .. }) => self.on_accept_key(modal, key),
            Some(Modal::Browse(b)) => self.on_browse_key(b, key),
            Some(Modal::SendStatus) => self.on_send_status_key(key),
            Some(Modal::ConfirmQuit { focus }) => {
                self.on_confirm_quit_key(focus, key);
                None
            }
            Some(Modal::ConfirmStop { focus }) => self.on_confirm_stop_key(focus, key),
            Some(modal) if add::is_pairing(&modal) => self.on_pair_modal_key(modal, key),
            Some(modal) => self.on_modal_key(modal, key),
            None if self.snapshot.needs_setup => self.on_setup_key(key),
            None if self.form_has_keys() => self.on_form_key(key),
            None => self.on_page_key(key),
        }
    }

    /// The first-run card: create this device's identity, or leave.
    fn on_setup_key(&mut self, key: Key) -> Option<Effect> {
        match key {
            Key::Enter if self.setup_focus == Choice::Yes => return Some(Effect::CreateIdentity),
            Key::Enter | Key::Char('n') => self.request_quit(),
            Key::Char('y') => return Some(Effect::CreateIdentity),
            Key::Left | Key::Right | Key::Tab | Key::BackTab => {
                self.setup_focus = match self.setup_focus {
                    Choice::Yes => Choice::No,
                    Choice::No => Choice::Yes,
                };
            }
            Key::Char(':') | Key::Palette => self.open_palette(),
            Key::Char('?') => self.modal = Some(Modal::Help),
            _ => {}
        }
        None
    }

    pub(super) fn open_palette(&mut self) {
        self.modal = None;
        self.palette = Some(Palette::new());
    }

    /// Keys while the palette is open. It was taken out of `self`; putting
    /// it back keeps it open.
    fn on_palette_key(&mut self, mut palette: Palette, key: Key) -> Option<Effect> {
        let suggestions = self.suggestions_for(&palette);
        match key {
            Key::Esc | Key::Palette => return None,
            Key::Enter => return self.submit(palette),
            Key::Tab => {
                if let Some(s) = suggestions.get(palette.selected) {
                    palette.input.set_text(&s.fill);
                    palette.typed();
                }
            }
            Key::Up => {
                palette.selected = palette.selected.saturating_sub(1);
                palette.moved = true;
            }
            Key::Down => {
                palette.selected = (palette.selected + 1).min(suggestions.len().saturating_sub(1));
                palette.moved = true;
            }
            Key::Char(c) => {
                palette.input.insert(c);
                palette.typed();
            }
            key => {
                if let Some(edit) = edit_for(key) {
                    palette.input.edit(edit);
                    if matches!(edit, Edit::Backspace | Edit::Delete | Edit::DeleteWord) {
                        palette.typed();
                    }
                }
            }
        }
        self.palette = Some(palette);
        None
    }

    /// Enter in the palette: run what is typed if it is a whole command;
    /// otherwise take the chosen suggestion, and run that if it is whole.
    fn submit(&mut self, mut palette: Palette) -> Option<Effect> {
        let chosen = self
            .suggestions_for(&palette)
            .get(palette.selected)
            .cloned();
        let text = palette.input.text().trim().to_string();
        let checked = palette::words(&text).and_then(|words| {
            if words.is_empty() {
                return Err("type a command, or pick one from the list".to_string());
            }
            let leaving = matches!(
                palette::command_words(&words).first().map(String::as_str),
                Some("quit" | "exit")
            );
            if leaving {
                Ok(words)
            } else {
                crate::cli::check(&words).map(|()| words)
            }
        });
        let take_choice = palette.moved || checked.is_err();
        if take_choice && let Some(choice) = chosen.filter(|c| c.fill.trim() != text) {
            palette.input.set_text(&choice.fill);
            palette.typed();
            if choice.complete {
                return self.submit(palette);
            }
            self.palette = Some(palette);
            return None;
        }
        match checked {
            Ok(words) => {
                self.remember(&text);
                self.run(words)
            }
            Err(reason) => {
                palette.error = Some(untrusted::text(&reason));
                self.palette = Some(palette);
                None
            }
        }
    }

    fn remember(&mut self, line: &str) {
        self.history.retain(|past| past != line);
        self.history.push(line.to_string());
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
    }

    /// Runs a checked command from the palette.
    fn run(&mut self, words: Vec<String>) -> Option<Effect> {
        let place = palette::place(&words);
        if place != Place::View {
            return Some(Effect::Run { args: words, place });
        }
        let rest = palette::command_words(&words);
        let operands: Vec<&String> = rest
            .iter()
            .skip(1)
            .filter(|w| !w.starts_with('-'))
            .collect();
        match (rest.first().map(String::as_str), operands.as_slice()) {
            (Some("rename"), [from, to]) => Some(Effect::Rename {
                from: from.to_string(),
                to: to.to_string(),
            }),
            (Some("send"), [peer, path]) => Some(Effect::StartSend {
                peer: peer.to_string(),
                path: super::send::unquote(path),
            }),
            (Some("remove"), [name]) => {
                match self.snapshot.friends.iter().position(|f| &f.name == *name) {
                    Some(index) => {
                        self.tab = Tab::Friends;
                        self.selected = index;
                        self.open_remove();
                    }
                    None => self.flash = Some(format!("You have no friend named {name}.")),
                }
                None
            }
            _ => {
                self.request_quit();
                None
            }
        }
    }

    fn on_page_key(&mut self, key: Key) -> Option<Effect> {
        let last = self.snapshot.friends.len().saturating_sub(1);
        match key {
            Key::Char('q') => self.request_quit(),
            Key::Char('s') if self.tab == Tab::Friends => self.open_send(),
            Key::Char('o') => return self.toggle_receiving(),
            Key::Char('?') => self.modal = Some(Modal::Help),
            Key::Char(':') | Key::Palette => self.open_palette(),
            Key::Copy => return self.copy(),
            Key::Tab | Key::Right | Key::Char('l') => self.set_tab(self.tab.step(1)),
            Key::BackTab | Key::Left | Key::Char('h') => self.set_tab(self.tab.step(-1)),
            Key::Char('1') => self.set_tab(Tab::Friends),
            Key::Char('2') => self.set_tab(Tab::Pending),
            Key::Char('3') => self.set_tab(Tab::AddFriend),
            Key::Enter | Key::Char('i') if self.tab == Tab::AddFriend => {
                self.set_tab(Tab::AddFriend)
            }
            Key::Up | Key::Char('k') if self.tab == Tab::Pending => {
                self.pending_selected = self.pending_selected.saturating_sub(1);
            }
            Key::Down | Key::Char('j') if self.tab == Tab::Pending => {
                self.pending_selected =
                    (self.pending_selected + 1).min(self.pending.len().saturating_sub(1));
            }
            Key::Enter => self.open_accept_here(),
            Key::Up | Key::Char('k') => self.selected = self.selected.saturating_sub(1),
            Key::Down | Key::Char('j') => self.selected = (self.selected + 1).min(last),
            Key::Home | Key::Char('g') => self.selected = 0,
            Key::End | Key::Char('G') => self.selected = last,
            Key::Char('r') => self.open_rename(),
            Key::Char('x') | Key::Delete => self.open_remove(),
            _ => {}
        }
        None
    }

    fn open_rename(&mut self) {
        if self.tab != Tab::Friends {
            return;
        }
        if let Some(friend) = self.selected_friend() {
            self.modal = Some(Modal::Rename {
                from: friend.name.clone(),
                input: Input::with_text(&friend.name, MAX_NAME_LEN),
                error: None,
            });
        }
    }

    fn open_remove(&mut self) {
        if self.tab != Tab::Friends {
            return;
        }
        if let Some(friend) = self.selected_friend() {
            self.modal = Some(Modal::Remove {
                name: friend.name.clone(),
                fingerprint: friend.fingerprint.clone(),
                focus: Choice::No,
            });
        }
    }

    /// Ctrl+C: copies what a person most often needs to read out or paste
    /// on this page.
    fn copy(&mut self) -> Option<Effect> {
        let effect = match self.tab {
            Tab::Friends => self.selected_friend().map(|f| Effect::Copy {
                what: format!("{}'s fingerprint", f.name),
                text: format!("SHA256:{}", f.fingerprint),
            }),
            Tab::AddFriend | Tab::Pending => self.snapshot.me.as_ref().map(|me| Effect::Copy {
                what: "your fingerprint".to_string(),
                text: format!("SHA256:{}", me.fingerprint),
            }),
        };
        if effect.is_none() {
            self.flash = Some("Nothing to copy here. Ctrl+Q leaves beam.".to_string());
        }
        effect
    }

    /// Keys while a pop-up is open. The pop-up was taken out of `self`;
    /// putting it back is what keeps it open.
    fn on_modal_key(&mut self, modal: Modal, key: Key) -> Option<Effect> {
        match modal {
            Modal::Help => None, // Any key closes the help, and does nothing else.
            Modal::Output {
                title,
                text,
                scroll,
                failed,
            } => {
                let scroll = match key {
                    Key::Esc | Key::Enter | Key::Char('q') => return None,
                    Key::Copy => {
                        let effect = Effect::Copy {
                            what: "the output".to_string(),
                            text: text.clone(),
                        };
                        self.modal = Some(Modal::Output {
                            title,
                            text,
                            scroll,
                            failed,
                        });
                        return Some(effect);
                    }
                    Key::Char(':') | Key::Palette => {
                        self.open_palette();
                        return None;
                    }
                    Key::Up | Key::Char('k') => scroll.saturating_sub(1),
                    Key::Down | Key::Char('j') => scroll.saturating_add(1),
                    Key::PageUp => scroll.saturating_sub(10),
                    Key::PageDown | Key::Char(' ') => scroll.saturating_add(10),
                    Key::Home | Key::Char('g') => 0,
                    Key::End | Key::Char('G') => u16::MAX,
                    _ => scroll,
                };
                self.modal = Some(Modal::Output {
                    title,
                    text,
                    scroll,
                    failed,
                });
                None
            }
            Modal::Rename {
                from,
                mut input,
                mut error,
            } => {
                match key {
                    Key::Esc => return None,
                    Key::Enter => {
                        let to = input.text().trim().to_string();
                        let effect = (to != from).then(|| Effect::Rename {
                            from: from.clone(),
                            to,
                        });
                        if effect.is_some() {
                            self.modal = Some(Modal::Rename { from, input, error });
                        }
                        return effect;
                    }
                    Key::Char(c) => input.insert(c),
                    key => {
                        if let Some(edit) = edit_for(key) {
                            input.edit(edit);
                        }
                    }
                }
                if matches!(
                    key,
                    Key::Char(_) | Key::Backspace | Key::Delete | Key::DeleteWord
                ) {
                    error = None;
                }
                self.modal = Some(Modal::Rename { from, input, error });
                None
            }
            Modal::Remove {
                name,
                fingerprint,
                focus,
            } => {
                let focus = match key {
                    Key::Esc => return None,
                    Key::Enter => {
                        return self.answer_remove(name, fingerprint, focus);
                    }
                    Key::Left | Key::Right | Key::Tab | Key::BackTab => match focus {
                        Choice::Yes => Choice::No,
                        Choice::No => Choice::Yes,
                    },
                    _ => focus,
                };
                self.modal = Some(Modal::Remove {
                    name,
                    fingerprint,
                    focus,
                });
                None
            }
            pairing => self.on_pair_modal_key(pairing, key),
        }
    }

    fn answer_remove(
        &mut self,
        name: String,
        fingerprint: String,
        answer: Choice,
    ) -> Option<Effect> {
        match answer {
            Choice::No => None,
            Choice::Yes => {
                self.modal = Some(Modal::Remove {
                    name: name.clone(),
                    fingerprint,
                    focus: Choice::Yes,
                });
                Some(Effect::Remove { name })
            }
        }
    }

    fn on_mouse(&mut self, kind: MouseKind, column: u16, row: u16) -> Option<Effect> {
        let last = self.snapshot.friends.len().saturating_sub(1);
        match kind {
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let up = kind == MouseKind::ScrollUp;
                if matches!(self.modal, Some(Modal::Browse(_))) {
                    self.on_browse_wheel(up);
                    return None;
                }
                if self.palette.is_some() {
                    return self.on_key(if up { Key::Up } else { Key::Down });
                }
                if let Some(Modal::Output { scroll, .. }) = &mut self.modal {
                    *scroll = if up {
                        scroll.saturating_sub(3)
                    } else {
                        scroll.saturating_add(3)
                    };
                    return None;
                }
                let over_list = self.areas.list.contains(Position::new(column, row));
                if self.modal.is_none() && self.tab == Tab::Friends && over_list {
                    self.selected = match kind {
                        MouseKind::ScrollUp => self.selected.saturating_sub(1),
                        _ => (self.selected + 1).min(last),
                    };
                }
                None
            }
            MouseKind::Click => {
                let hit = self.areas.at(column, row);
                self.flash = None;
                if let Some(mut palette) = self.palette.take() {
                    match hit {
                        Some((_, Target::Suggestion(index))) => {
                            palette.selected = index;
                            palette.moved = true;
                            return self.submit(palette);
                        }
                        Some((area, Target::PaletteInput)) => {
                            palette.input.click(column - area.x, area.width);
                            self.palette = Some(palette);
                        }
                        // A click anywhere else closes the palette.
                        _ => {}
                    }
                    return None;
                }
                match (self.modal.take(), hit) {
                    (None, Some((_, Target::Tab(tab)))) => self.set_tab(tab),
                    (None, Some((area, Target::Field(field)))) => {
                        return self.click_field(field, column.saturating_sub(area.x), area.width);
                    }
                    (None, Some((_, Target::Button(answer)))) if self.snapshot.needs_setup => {
                        self.setup_focus = answer;
                        return self.on_setup_key(Key::Enter);
                    }
                    (None, Some((_, Target::ReceiveSwitch))) => return self.toggle_receiving(),
                    (Some(Modal::ConfirmStop { .. }), Some((_, Target::Button(answer)))) => {
                        if answer == Choice::Yes {
                            return Some(Effect::StopReceiving);
                        }
                    }
                    (None, Some((_, Target::Request(index)))) => {
                        self.pending_selected = index;
                        self.open_accept(index);
                    }
                    (Some(Modal::Browse(b)), hit) => {
                        return self.on_browse_click(b, hit.map(|(_, target)| target));
                    }
                    (Some(Modal::SendStatus), hit) => {
                        return self.on_send_status_click(hit.map(|(_, target)| target));
                    }
                    (Some(Modal::ConfirmQuit { .. }), Some((_, Target::Button(answer)))) => {
                        self.quit = answer == Choice::Yes;
                    }
                    (Some(modal @ Modal::Accept { .. }), hit) => {
                        return self.on_accept_click(modal, hit.map(|(_, target)| target));
                    }
                    (Some(modal), hit) if add::is_pairing(&modal) => {
                        return self.on_pair_modal_click(modal, hit, column);
                    }
                    (None, Some((_, Target::Friend(index)))) => {
                        self.selected = index.min(last);
                    }
                    (Some(Modal::Help), _) => {}
                    (
                        Some(Modal::Remove {
                            name, fingerprint, ..
                        }),
                        Some((_, Target::Button(answer))),
                    ) => return self.answer_remove(name, fingerprint, answer),
                    (
                        Some(Modal::Rename { from, input, error }),
                        Some((_, Target::Button(answer))),
                    ) => {
                        self.modal = Some(Modal::Rename { from, input, error });
                        return match answer {
                            Choice::Yes => self.on_key(Key::Enter),
                            Choice::No => {
                                self.modal = None;
                                None
                            }
                        };
                    }
                    (
                        Some(Modal::Rename {
                            from,
                            mut input,
                            error,
                        }),
                        Some((area, Target::Input)),
                    ) => {
                        input.click(column - area.x, area.width);
                        self.modal = Some(Modal::Rename { from, input, error });
                    }
                    // A click beside a pop-up leaves it open.
                    (modal, _) => self.modal = modal,
                }
                None
            }
        }
    }
}

/// The text-box edit a key stands for, if any.
pub(super) fn edit_for(key: Key) -> Option<Edit> {
    Some(match key {
        Key::Left => Edit::Left,
        Key::Right => Edit::Right,
        Key::WordLeft => Edit::WordLeft,
        Key::WordRight => Edit::WordRight,
        Key::Home => Edit::Home,
        Key::End => Edit::End,
        Key::Backspace => Edit::Backspace,
        Key::Delete => Edit::Delete,
        Key::DeleteWord => Edit::DeleteWord,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::super::pairing::{Answer, Update};
    use super::*;

    fn friend(name: &str, fingerprint: &str) -> Friend {
        Friend {
            name: name.to_string(),
            short_id: "123 456 789".to_string(),
            fingerprint: fingerprint.to_string(),
            added: "2026-10-05".to_string(),
            last_seen: None,
        }
    }

    fn snapshot(friends: Vec<Friend>) -> Snapshot {
        Snapshot {
            me: None,
            friends,
            agent: Agent::Stopped,
            history: Vec::new(),
            owner: Owner::Background,
            listen_elsewhere: false,
            needs_setup: false,
            problem: None,
        }
    }

    fn three() -> App {
        App::new(snapshot(vec![
            friend("alice", "aa"),
            friend("bob", "bb"),
            friend("carol", "cc"),
        ]))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(Key::Char(c));
        }
    }

    fn click(app: &mut App, column: u16, row: u16) -> Option<Effect> {
        app.on_event(Event::Mouse {
            kind: MouseKind::Click,
            column,
            row,
        })
    }

    #[test]
    fn the_cursor_moves_and_stops_at_both_ends() {
        let mut app = three();
        app.on_key(Key::Up);
        assert_eq!(app.selected, 0);
        for _ in 0..5 {
            app.on_key(Key::Down);
        }
        assert_eq!(app.selected, 2);
        app.on_key(Key::Char('g'));
        assert_eq!(app.selected, 0);
        app.on_key(Key::End);
        assert_eq!(app.selected_friend().unwrap().name, "carol");
    }

    #[test]
    fn moving_with_no_friends_does_not_panic() {
        let mut app = App::new(snapshot(Vec::new()));
        for key in [
            Key::Down,
            Key::Up,
            Key::End,
            Key::Home,
            Key::Char('r'),
            Key::Delete,
        ] {
            app.on_key(key);
        }
        assert_eq!(app.selected, 0);
        assert!(app.selected_friend().is_none());
        assert!(app.modal.is_none());
    }

    #[test]
    fn tabs_wrap_both_ways_and_numbers_jump() {
        let mut app = three();
        app.on_key(Key::BackTab);
        assert_eq!(app.tab, Tab::AddFriend);
        app.on_key(Key::Tab);
        assert_eq!(app.tab, Tab::Friends);
        app.on_key(Key::Char('2'));
        assert_eq!(app.tab, Tab::Pending);
    }

    #[test]
    fn ctrl_q_quits_from_anywhere_and_q_only_from_the_page() {
        let mut app = three();
        app.on_key(Key::Char('r'));
        app.on_key(Key::Quit);
        assert!(app.quit, "Ctrl+Q works inside a pop-up");

        let mut app = three();
        app.on_key(Key::Char('q'));
        assert!(app.quit);

        let mut app = three();
        app.on_key(Key::Char('r'));
        app.on_key(Key::Char('q'));
        assert!(!app.quit, "in a text box, q is a letter");
    }

    #[test]
    fn ctrl_c_copies_and_never_quits() {
        let mut app = three();
        app.on_key(Key::Down);
        let effect = app.on_key(Key::Copy);
        assert_eq!(
            effect,
            Some(Effect::Copy {
                what: "bob's fingerprint".to_string(),
                text: "SHA256:bb".to_string()
            })
        );
        assert!(!app.quit);

        let mut empty = App::new(snapshot(Vec::new()));
        assert_eq!(empty.on_key(Key::Copy), None);
        assert!(!empty.quit);
        assert!(empty.flash.as_deref().unwrap().contains("Ctrl+Q"));
    }

    #[test]
    fn esc_closes_a_pop_up_but_does_not_quit() {
        let mut app = three();
        app.on_key(Key::Esc);
        assert!(!app.quit);
        app.on_key(Key::Char('?'));
        app.on_key(Key::Esc);
        assert!(app.modal.is_none() && !app.quit);
    }

    #[test]
    fn the_help_closes_on_any_key_and_that_key_does_nothing_else() {
        let mut app = three();
        app.on_key(Key::Char('?'));
        assert_eq!(app.modal, Some(Modal::Help));
        app.on_key(Key::Down);
        assert!(app.modal.is_none() && !app.quit);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn rename_edits_the_name_and_asks_for_the_change() {
        let mut app = three();
        app.on_key(Key::Char('r'));
        app.on_key(Key::Backspace);
        app.on_key(Key::Backspace);
        type_text(&mut app, "cia");
        let effect = app.on_key(Key::Enter);
        assert_eq!(
            effect,
            Some(Effect::Rename {
                from: "alice".to_string(),
                to: "alicia".to_string()
            })
        );
        // The pop-up waits for the result; an error keeps it open.
        app.effect_done(Done::Failed("name taken".to_string()));
        match &app.modal {
            Some(Modal::Rename { error, .. }) => assert_eq!(error.as_deref(), Some("name taken")),
            other => panic!("{other:?}"),
        }
        app.effect_done(Done::Message("Renamed".to_string()));
        assert!(app.modal.is_none());
        assert_eq!(app.flash.as_deref(), Some("Renamed"));
    }

    #[test]
    fn rename_to_the_same_name_just_closes() {
        let mut app = three();
        app.on_key(Key::Char('r'));
        assert_eq!(app.on_key(Key::Enter), None);
        assert!(app.modal.is_none());
    }

    #[test]
    fn a_paste_goes_into_the_rename_box() {
        let mut app = three();
        app.on_key(Key::Char('r'));
        app.on_event(Event::Paste("-2\n".to_string()));
        match &app.modal {
            Some(Modal::Rename { input, .. }) => assert_eq!(input.text(), "alice-2"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn remove_starts_on_keep_so_enter_alone_removes_nothing() {
        let mut app = three();
        app.on_key(Key::Char('x'));
        assert!(matches!(
            app.modal,
            Some(Modal::Remove {
                focus: Choice::No,
                ..
            })
        ));
        assert_eq!(app.on_key(Key::Enter), None);
        assert!(app.modal.is_none());

        app.on_key(Key::Delete);
        app.on_key(Key::Left);
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::Remove {
                name: "alice".to_string()
            })
        );
    }

    #[test]
    fn a_click_picks_a_tab_a_friend_or_a_button() {
        let mut app = three();
        app.areas
            .add(Rect::new(0, 1, 10, 1), Target::Tab(Tab::Pending));
        app.areas.add(Rect::new(0, 5, 20, 2), Target::Friend(2));
        click(&mut app, 3, 5);
        assert_eq!(app.selected, 2);
        click(&mut app, 3, 1);
        assert_eq!(app.tab, Tab::Pending);

        // With a pop-up open, the page below does not take clicks.
        app.tab = Tab::Friends;
        app.on_key(Key::Char('x'));
        click(&mut app, 3, 1);
        assert_eq!(app.tab, Tab::Friends);
        assert!(
            app.modal.is_some(),
            "a click beside the pop-up leaves it open"
        );

        app.areas
            .add(Rect::new(40, 10, 8, 1), Target::Button(Choice::Yes));
        assert_eq!(
            click(&mut app, 42, 10),
            Some(Effect::Remove {
                name: "carol".to_string()
            })
        );
    }

    #[test]
    fn the_wheel_scrolls_the_list_only_over_the_list() {
        let mut app = three();
        app.areas.list = Rect::new(0, 3, 20, 10);
        let wheel = |app: &mut App, kind, row| {
            app.on_event(Event::Mouse {
                kind,
                column: 2,
                row,
            })
        };
        wheel(&mut app, MouseKind::ScrollDown, 5);
        wheel(&mut app, MouseKind::ScrollDown, 5);
        assert_eq!(app.selected, 2);
        wheel(&mut app, MouseKind::ScrollUp, 30);
        assert_eq!(app.selected, 2, "outside the list the wheel does nothing");
    }

    fn open_with(app: &mut App, text: &str) {
        app.on_key(Key::Char(':'));
        app.on_event(Event::Paste(text.to_string()));
    }

    fn no_paths(_: &str, _: bool) -> Vec<String> {
        Vec::new()
    }

    #[test]
    fn colon_and_ctrl_p_open_the_palette_and_esc_closes_it() {
        let mut app = three();
        app.on_key(Key::Char(':'));
        assert!(app.palette.is_some());
        app.on_key(Key::Esc);
        assert!(app.palette.is_none() && !app.quit);
        app.on_key(Key::Palette);
        assert!(app.palette.is_some());
        app.on_key(Key::Char('q'));
        assert!(!app.quit, "in the palette, q is a letter");
    }

    #[test]
    fn a_whole_command_runs_where_it_belongs_and_is_remembered() {
        let mut app = three();
        open_with(&mut app, "peers");
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::Run {
                args: vec!["peers".to_string()],
                place: Place::Here
            })
        );
        assert!(app.palette.is_none());
        assert_eq!(app.history, ["peers"]);

        open_with(&mut app, "beam send alice \"my file.txt\"");
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::StartSend {
                peer: "alice".into(),
                path: "my file.txt".into()
            }),
            "sending runs in the view"
        );
    }

    #[test]
    fn a_few_letters_and_enter_run_the_best_match() {
        let mut app = three();
        open_with(&mut app, "who");
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::Run {
                args: vec!["whoami".to_string()],
                place: Place::Here
            })
        );
    }

    #[test]
    fn enter_on_an_unfinished_command_fills_it_in_and_waits() {
        let mut app = three();
        app.paths = no_paths;
        open_with(&mut app, "send");
        assert_eq!(app.on_key(Key::Enter), None);
        let palette = app.palette.as_ref().expect("still open");
        assert_eq!(
            palette.input.text(),
            "send alice ",
            "the file is still needed"
        );
    }

    #[test]
    fn tab_completes_and_arrows_choose() {
        let mut app = three();
        open_with(&mut app, "send b");
        app.on_key(Key::Tab);
        assert_eq!(app.palette.as_ref().unwrap().input.text(), "send bob ");

        let mut app = three();
        open_with(&mut app, "service st");
        app.on_key(Key::Down);
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::Run {
                args: vec!["service".into(), "start".into()],
                place: Place::Here
            }),
            "the highlighted line wins once the person moved to it"
        );
    }

    #[test]
    fn a_wrong_command_says_why_and_stays_open() {
        let mut app = three();
        open_with(&mut app, "send alice a.txt --bogus");
        assert_eq!(app.on_key(Key::Enter), None);
        let error = app.palette.as_ref().unwrap().error.clone().unwrap();
        assert!(error.contains("--bogus"), "{error}");
        app.on_key(Key::Backspace);
        assert!(app.palette.as_ref().unwrap().error.is_none());
    }

    #[test]
    fn rename_remove_and_quit_use_the_views_own_pop_ups() {
        let mut app = three();
        open_with(&mut app, "rename bob robert");
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::Rename {
                from: "bob".into(),
                to: "robert".into()
            })
        );

        open_with(&mut app, "remove -y carol");
        assert_eq!(
            app.on_key(Key::Enter),
            None,
            "-y does not skip the question"
        );
        assert!(matches!(
            app.modal,
            Some(Modal::Remove {
                focus: Choice::No,
                ..
            })
        ));
        assert_eq!(app.selected_friend().unwrap().name, "carol");

        app.modal = None;
        open_with(&mut app, "quit");
        app.on_key(Key::Enter);
        assert!(app.quit);
    }

    #[test]
    fn nothing_typed_in_the_palette_can_accept_a_transfer() {
        for line in ["accept", "y", "yes", "inbox --accept", "listen --yes"] {
            let mut app = three();
            open_with(&mut app, line);
            let effect = app.on_key(Key::Enter);
            if let Some(Effect::Run { args, .. }) = &effect {
                // Only real commands get through, and none of them accepts.
                assert!(crate::cli::check(args).is_ok(), "{line}: {args:?}");
                assert!(
                    !args.iter().any(|a| a.contains("accept") || a == "--yes"),
                    "{line}: {args:?}"
                );
            }
        }
    }

    #[test]
    fn output_scrolls_copies_and_closes() {
        let mut app = three();
        app.effect_done(Done::Output {
            title: "beam peers".into(),
            text: "a\nb".into(),
            failed: false,
        });
        app.on_key(Key::Down);
        assert!(matches!(app.modal, Some(Modal::Output { scroll: 1, .. })));
        assert_eq!(
            app.on_key(Key::Copy),
            Some(Effect::Copy {
                what: "the output".into(),
                text: "a\nb".into()
            })
        );
        app.on_key(Key::Esc);
        assert!(app.modal.is_none());
    }

    fn with_me(mut app: App) -> App {
        app.snapshot.me = Some(Me {
            name: "me".into(),
            short_id: "111 222 333".into(),
            fingerprint: "ff".into(),
        });
        app
    }

    #[test]
    fn the_add_friend_tab_puts_the_cursor_in_the_invite_box() {
        let mut app = with_me(three());
        app.on_key(Key::Char('3'));
        assert_eq!(app.add.focus, Some(Field::Invite));
        app.on_event(Event::Paste("  beam1abc\n".to_string()));
        app.on_key(Key::Tab);
        type_text(&mut app, "dana");
        assert_eq!(app.add.invite.text(), "beam1abc");
        assert_eq!(app.add.name.text(), "dana");
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::StartJoin {
                invite: "beam1abc".into(),
                name: "dana".into()
            })
        );
    }

    #[test]
    fn typing_in_the_form_does_not_switch_tabs_or_quit_and_esc_gives_keys_back() {
        let mut app = with_me(three());
        app.on_key(Key::Char('3'));
        type_text(&mut app, "q12");
        assert!(!app.quit);
        assert_eq!(app.tab, Tab::AddFriend);
        assert_eq!(app.add.invite.text(), "q12");
        app.on_key(Key::Esc);
        app.on_key(Key::Char('1'));
        assert_eq!(app.tab, Tab::Friends);
    }

    #[test]
    fn the_form_says_what_is_missing() {
        let mut app = with_me(three());
        app.on_key(Key::Char('3'));
        app.on_key(Key::Tab);
        assert_eq!(app.on_key(Key::Enter), None);
        assert!(app.add.error.as_deref().unwrap().contains("invite"));
        assert_eq!(app.add.focus, Some(Field::Invite));
        type_text(&mut app, "beam1x");
        app.on_key(Key::Tab);
        assert_eq!(app.on_key(Key::Enter), None);
        assert!(app.add.error.as_deref().unwrap().contains("name"));
    }

    #[test]
    fn show_my_invite_waits_and_its_pop_up_copies_and_cancels() {
        let mut app = with_me(three());
        app.on_key(Key::Char('3'));
        app.on_key(Key::BackTab);
        assert_eq!(app.add.focus, Some(Field::ShowMine));
        assert_eq!(app.on_key(Key::Enter), Some(Effect::StartWait));
        app.effect_done(Done::PairingStarted);
        app.on_pair(Update::Waiting {
            invite: "beam1mine".into(),
            code: "123 456".into(),
            expires_in: std::time::Duration::from_secs(600),
        });
        assert_eq!(
            app.on_key(Key::Copy),
            Some(Effect::Copy {
                what: "your invite".into(),
                text: "beam1mine".into()
            })
        );
        app.on_pair(Update::Attempt {
            peer: "SHA256:ab".into(),
        });
        assert!(matches!(
            &app.modal,
            Some(Modal::ShowInvite {
                attempt: Some(_),
                ..
            })
        ));
        assert_eq!(app.on_key(Key::Esc), Some(Effect::CancelPair));
    }

    #[test]
    fn the_code_pop_up_takes_digits_and_never_sends_a_malformed_code() {
        let mut app = with_me(three());
        app.on_pair(Update::AskCode);
        type_text(&mut app, "12x3");
        assert_eq!(app.on_key(Key::Enter), None, "too short: not sent");
        match &app.modal {
            Some(Modal::EnterCode { input, error }) => {
                assert_eq!(input.text(), "123", "letters are not typed");
                assert!(error.is_some());
            }
            other => panic!("{other:?}"),
        }
        type_text(&mut app, "456");
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::Answer(Answer::Code(Some("123 456".into()))))
        );
        assert!(matches!(app.modal, Some(Modal::Busy { .. })));
    }

    #[test]
    fn the_fingerprint_check_starts_on_no() {
        let mut app = with_me(three());
        app.on_pair(Update::AskConfirm {
            name: "dana".into(),
            peer: "aa".into(),
            own: "ff".into(),
        });
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::Answer(Answer::Confirm(false))),
            "Enter alone never pairs"
        );

        app.on_pair(Update::AskConfirm {
            name: "dana".into(),
            peer: "aa".into(),
            own: "ff".into(),
        });
        app.on_key(Key::Left);
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::Answer(Answer::Confirm(true)))
        );
    }

    #[test]
    fn pairing_done_selects_the_new_friend_and_a_failure_says_why() {
        let mut app = with_me(three());
        app.tab = Tab::AddFriend;
        app.on_pair(Update::Finished(Ok(("dana".into(), "dd".into()))));
        assert_eq!(app.tab, Tab::Friends);
        assert_eq!(app.flash.as_deref(), Some("Paired with dana."));
        let mut friends = three().snapshot.friends;
        friends.push(friend("dana", "dd"));
        app.refresh(snapshot(friends));
        assert_eq!(app.selected_friend().unwrap().name, "dana");

        app.on_pair(Update::Finished(Err("Not paired: declined".into())));
        assert!(matches!(
            &app.modal,
            Some(Modal::Output { failed: true, .. })
        ));
    }

    #[test]
    fn a_relay_change_starts_on_no() {
        let mut app = with_me(three());
        app.effect_done(Done::AskRelay {
            name: "bob".into(),
            fingerprint: "bb".into(),
            old: "a".into(),
            new: "b".into(),
            invite: "beam1x".into(),
        });
        assert_eq!(app.on_key(Key::Enter), None);
        assert_eq!(app.flash.as_deref(), Some("Nothing was changed."));
        app.effect_done(Done::AskRelay {
            name: "bob".into(),
            fingerprint: "bb".into(),
            old: "a".into(),
            new: "b".into(),
            invite: "beam1x".into(),
        });
        app.on_key(Key::Right);
        assert_eq!(
            app.on_key(Key::Enter),
            Some(Effect::UpdateLocation {
                invite: "beam1x".into()
            })
        );
    }

    fn first_run() -> App {
        let mut app = three();
        app.snapshot.needs_setup = true;
        app
    }

    #[test]
    fn the_first_run_offers_to_create_the_identity_and_enter_does_it() {
        let mut app = first_run();
        assert_eq!(app.on_key(Key::Enter), Some(Effect::CreateIdentity));
        app.effect_done(Done::IdentityCreated {
            short_id: "111 222 333".into(),
        });
        assert_eq!(app.tab, Tab::AddFriend, "next: add a friend");
        assert_eq!(app.add.focus, Some(Field::Invite));
        assert!(app.flash.as_deref().unwrap().contains("111 222 333"));
    }

    #[test]
    fn not_now_leaves_and_keys_do_nothing_else_meanwhile() {
        let mut app = first_run();
        for key in [Key::Char('s'), Key::Char('o'), Key::Char('3'), Key::Down] {
            assert_eq!(app.on_key(key), None, "{key:?}");
        }
        assert_eq!(app.tab, Tab::Friends);
        app.on_key(Key::Right);
        assert_eq!(app.setup_focus, Choice::No);
        assert_eq!(app.on_key(Key::Enter), None);
        assert!(app.quit);
    }

    #[test]
    fn a_refresh_keeps_the_cursor_on_the_same_friend() {
        let mut app = three();
        app.on_key(Key::Down);
        assert_eq!(app.selected_friend().unwrap().name, "bob");

        // alice was removed elsewhere; bob moved up one place.
        app.refresh(snapshot(vec![friend("bob", "bb"), friend("carol", "cc")]));
        assert_eq!(app.selected_friend().unwrap().name, "bob");

        // bob is gone too: the cursor stays in range.
        app.refresh(snapshot(vec![friend("carol", "cc")]));
        assert_eq!(app.selected_friend().unwrap().name, "carol");
    }
}
