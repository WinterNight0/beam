//! The window's state, what changes it, and what it draws: iced's
//! `Message` → `update` → `view`, the shape `tui::app` already has.
//!
//! GUI EX Patch #1 is the skeleton: this device and its friends, read
//! through `beam-model` exactly as the full-screen view reads them. The
//! look (Aero's structure, rice's style) and the flows come in later
//! patches; see `docs/spikes/gui.md`.

use beam_core::identity::Store;
use beam_core::untrusted;
use beam_model::snapshot::Snapshot;
use iced::widget::{button, column, container, scrollable, text};
use iced::{Element, Fill};

#[derive(Debug)]
pub struct Gui {
    /// `None` when no beam home could be found at all.
    store: Option<Store>,
    snapshot: Option<Snapshot>,
    /// Why there is no beam home, already cleaned for drawing.
    problem: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Read `~/.beam` again: a terminal may have changed it.
    Refresh,
}

impl Gui {
    /// The beam home the CLI would use: `$BEAM_DIR`, else `~/.beam`.
    pub fn boot() -> Self {
        match Store::default_dir() {
            Ok(dir) => Self::for_home(Store::new(dir)),
            Err(e) => Self {
                store: None,
                snapshot: None,
                problem: Some(untrusted::text(&e.to_string())),
            },
        }
    }

    pub fn for_home(store: Store) -> Self {
        let snapshot = Snapshot::load(&store);
        Self {
            store: Some(store),
            snapshot: Some(snapshot),
            problem: None,
        }
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Refresh => {
                if let Some(store) = &self.store {
                    self.snapshot = Some(Snapshot::load(store));
                }
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = match (&self.snapshot, &self.problem) {
            (_, Some(problem)) => text(format!("No beam home: {problem}")).into(),
            (Some(snapshot), None) => device(snapshot),
            (None, None) => text("Reading ~/.beam…").into(),
        };
        container(
            column![
                text("beam").size(28),
                body,
                button("Refresh").on_press(Message::Refresh)
            ]
            .spacing(16),
        )
        .padding(24)
        .width(Fill)
        .height(Fill)
        .into()
    }
}

/// This device and its friends. Every string in a `Snapshot` is already
/// cleaned (ADR-0034).
fn device(snapshot: &Snapshot) -> Element<'_, Message> {
    let mut lines = column![].spacing(8);
    match &snapshot.me {
        Some(me) => {
            lines = lines
                .push(text(format!("This device: {}", me.name)).size(20))
                .push(text(format!("Fingerprint SHA256:{}", me.fingerprint)));
        }
        None if snapshot.needs_setup => {
            lines = lines
                .push(text("This device has no identity yet."))
                .push(text("Run `beam init` in a terminal, then Refresh."));
        }
        None => {}
    }
    if let Some(problem) = &snapshot.problem {
        lines = lines.push(text(problem.as_str()));
    }
    if snapshot.me.is_some() {
        let friends = if snapshot.friends.is_empty() {
            column![text("No friends yet.")]
        } else {
            snapshot
                .friends
                .iter()
                .fold(column![].spacing(4), |list, friend| {
                    list.push(text(format!("{}  {}", friend.name, friend.short_id)))
                })
        };
        lines = lines
            .push(text(format!("Friends ({})", snapshot.friends.len())).size(20))
            .push(scrollable(friends));
    }
    lines.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_core::identity::{Identity, KnownPeers, Peer};
    use iced_test::simulator;

    fn home(with_identity: bool, friends: &[&str]) -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("beam"));
        store.ensure_dirs().unwrap();
        if with_identity {
            store
                .save_identity(&Identity::generate("desk").unwrap(), false)
                .unwrap();
        }
        let mut known = KnownPeers::with_header();
        for name in friends {
            let key = Identity::generate(name).unwrap().verifying_key();
            known.add(Peer::new(*name, key)).unwrap();
        }
        store.save_known_peers(&known).unwrap();
        (tmp, store)
    }

    fn shows(gui: &Gui, wanted: &str) -> bool {
        simulator(gui.view()).find(wanted).is_ok()
    }

    #[test]
    fn this_device_and_its_friends_are_shown() {
        let (_tmp, store) = home(true, &["alice", "bob"]);
        let gui = Gui::for_home(store);
        assert!(shows(&gui, "This device: desk"));
        assert!(shows(&gui, "Friends (2)"));
        let alice = &gui.snapshot.as_ref().unwrap().friends[0];
        assert!(shows(&gui, &format!("alice  {}", alice.short_id)));
    }

    #[test]
    fn a_first_run_says_how_to_make_an_identity() {
        let (_tmp, store) = home(false, &[]);
        let gui = Gui::for_home(store);
        assert!(shows(&gui, "This device has no identity yet."));
        assert!(!shows(&gui, "Friends (0)"));
    }

    /// A friend paired in a terminal while the window is open appears on
    /// Refresh.
    #[test]
    fn refresh_reads_the_beam_home_again() {
        let (_tmp, store) = home(true, &[]);
        let mut gui = Gui::for_home(store.clone());
        assert!(shows(&gui, "No friends yet."));

        let mut known = store.load_known_peers().unwrap();
        let key = Identity::generate("carol").unwrap().verifying_key();
        known.add(Peer::new("carol", key)).unwrap();
        store.save_known_peers(&known).unwrap();

        let messages: Vec<Message> = {
            let mut ui = simulator(gui.view());
            ui.click("Refresh").expect("a Refresh button");
            ui.into_messages().collect()
        };
        for message in messages {
            gui.update(message);
        }
        assert!(shows(&gui, "Friends (1)"));
    }
}
