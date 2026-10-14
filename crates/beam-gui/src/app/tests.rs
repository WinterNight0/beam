//! The window, driven without a screen (`iced_test`): find text, click
//! buttons by their label, and check what changed.

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

fn click(gui: &mut Gui, label: &str) {
    let messages: Vec<Message> = {
        let mut ui = simulator(gui.view());
        ui.click(label)
            .unwrap_or_else(|_| panic!("no {label:?} to click"));
        ui.into_messages().collect()
    };
    for message in messages {
        let _ = gui.update(message);
    }
}

#[test]
fn the_friends_are_listed_and_the_first_is_shown() {
    let (_tmp, store) = home(true, &["alice", "bob"]);
    let gui = Gui::for_home(store);
    assert!(shows(&gui, "bob"));
    assert_eq!(gui.selected.as_deref(), Some("alice"));
    let alice = &gui.snapshot.as_ref().unwrap().friends[0];
    let fingerprint = format!("SHA256:{}", crate::view::grouped(&alice.fingerprint));
    assert!(
        shows(&gui, &fingerprint),
        "the selected friend's fingerprint"
    );
}

#[test]
fn clicking_a_friend_shows_them() {
    let (_tmp, store) = home(true, &["alice", "bob"]);
    let mut gui = Gui::for_home(store);
    click(&mut gui, "bob");
    assert_eq!(gui.selected.as_deref(), Some("bob"));
}

#[test]
fn search_narrows_the_list() {
    let (_tmp, store) = home(true, &["alice", "bob"]);
    let mut gui = Gui::for_home(store);
    let _ = gui.update(Message::Page(Page::Settings));
    let _ = gui.update(Message::Page(Page::Friends));
    let _ = gui.update(Message::Search("bo".into()));
    // alice stays selected (on the right) but leaves the list.
    let _ = gui.update(Message::Select("bob".into()));
    assert!(shows(&gui, "bob"));
    let _ = gui.update(Message::Search("zz".into()));
    assert!(shows(&gui, "No friend matches."));
}

#[test]
fn the_rail_changes_the_page() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    click(&mut gui, "Settings");
    assert_eq!(gui.page, Page::Settings);
    assert!(shows(&gui, "Appearance"));
    click(&mut gui, "Transfers");
    assert!(shows(&gui, "Not in this window yet"));
}

#[test]
fn the_theme_switches_between_midnight_and_daylight() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    assert!(gui.dark, "Midnight first");
    let _ = gui.update(Message::Page(Page::Settings));
    click(&mut gui, "Daylight");
    assert!(!gui.dark);
    click(&mut gui, "Midnight");
    assert!(gui.dark);
}

/// The first run offers to create the identity, creates it once, and the
/// window then shows the device. Never replacing one is the action's own
/// rule (`actions::identity`).
#[test]
fn a_first_run_creates_the_identity_once() {
    let (_tmp, store) = home(false, &[]);
    let mut gui = Gui::for_home(store.clone());
    assert!(shows(&gui, "Welcome to beam"));
    click(&mut gui, "Create this device's identity");
    let made = store
        .load_identity()
        .expect("an identity")
        .fingerprint()
        .hex();
    assert!(!shows(&gui, "Welcome to beam"));
    assert!(matches!(gui.notice, Some((_, Tone::Success))));

    // Asked again (a stale window, a double click): left alone.
    let _ = gui.update(Message::CreateIdentity);
    assert_eq!(store.load_identity().unwrap().fingerprint().hex(), made);
    assert!(matches!(gui.notice, Some((_, Tone::Info))));
}

#[test]
fn a_device_with_an_identity_is_not_offered_a_new_one() {
    let (_tmp, store) = home(true, &[]);
    let gui = Gui::for_home(store);
    assert!(!shows(&gui, "Create this device's identity"));
}

/// A friend paired in a terminal while the window is open appears without
/// a click; one removed there disappears and is no longer selected.
#[test]
fn a_refresh_picks_up_changes_made_in_a_terminal() {
    let (_tmp, store) = home(true, &["alice"]);
    let mut gui = Gui::for_home(store.clone());
    let mut known = store.load_known_peers().unwrap();
    let key = Identity::generate("carol").unwrap().verifying_key();
    known.add(Peer::new("carol", key)).unwrap();
    known.remove("alice").unwrap();
    store.save_known_peers(&known).unwrap();

    let _ = gui.update(Message::Refresh);
    assert!(shows(&gui, "carol"));
    assert_eq!(gui.selected.as_deref(), Some("carol"));
}

/// Tiling compositors size the window below its minimum: the status bar
/// then drops items instead of wrapping.
#[test]
fn a_narrow_window_keeps_the_status_bar_to_one_line() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    assert!(shows(&gui, "UDP 7820"), "{:?}", gui.config);
    let _ = gui.update(Message::Resized(820.0));
    assert!(!shows(&gui, "UDP 7820"));
    assert_eq!(gui.pane_width(), 240.0);
}

#[test]
fn the_relay_is_shown_by_its_host() {
    let (_tmp, store) = home(true, &[]);
    std::fs::write(
        store.config_path(),
        "relay = \"https://relay.example.org./\"\n",
    )
    .unwrap();
    let gui = Gui::for_home(store);
    assert_eq!(gui.relay_label(), "relay.example.org");

    let (_tmp, store) = home(true, &[]);
    std::fs::write(store.config_path(), "relay = \"none\"\n").unwrap();
    assert_eq!(Gui::for_home(store).relay_label(), "none (direct only)");
}

#[test]
fn the_status_bar_says_whether_anything_receives() {
    let (_tmp, store) = home(true, &[]);
    let gui = Gui::for_home(store);
    assert!(shows(&gui, "Receiving: off"));
}

/// The same friend gets the same colour everywhere: it comes from their
/// fingerprint, not from their local name or their place in the list.
#[test]
fn a_friends_colour_comes_from_their_fingerprint() {
    let a = "82f14c7d";
    assert_eq!(theme::tint_for(a), theme::tint_for(a));
    let colours: std::collections::HashSet<String> = (0..=255u8)
        .map(|b| format!("{:?}", theme::tint_for(&format!("{b:02x}"))))
        .collect();
    assert_eq!(colours.len(), theme::TINTS.len(), "every colour is used");
}

// ---------------------------------------------------------------- Patch #3

fn key(named: keyboard::key::Named) -> Message {
    Message::Key(keyboard::Key::Named(named))
}

#[test]
fn rename_changes_the_name_and_keeps_the_friend_selected() {
    let (_tmp, store) = home(true, &["alice"]);
    let mut gui = Gui::for_home(store.clone());
    click(&mut gui, "Rename");
    assert!(shows(&gui, "A new name for alice on this device"));
    let _ = gui.update(Message::RenameText("ali".into()));
    let _ = gui.update(key(keyboard::key::Named::Enter));
    assert!(gui.dialog.is_none());
    assert_eq!(gui.selected.as_deref(), Some("ali"));
    let known = store.load_known_peers().unwrap();
    assert!(known.lookup("ali").is_some() && known.lookup("alice").is_none());
}

/// A refused name keeps the form open with the reason, and changes nothing.
#[test]
fn a_refused_rename_says_why_and_changes_nothing() {
    let (_tmp, store) = home(true, &["alice", "bob"]);
    let mut gui = Gui::for_home(store.clone());
    let _ = gui.update(Message::Rename("alice".into()));
    let _ = gui.update(Message::RenameText("bob".into()));
    let _ = gui.update(Message::ConfirmRename);
    assert!(matches!(
        gui.dialog,
        Some(Dialog::Rename { error: Some(_), .. })
    ));
    assert!(store.load_known_peers().unwrap().lookup("alice").is_some());
    let _ = gui.update(key(keyboard::key::Named::Escape));
    assert!(gui.dialog.is_none());
}

/// The safe default (`Ask`): the Remove question starts on Keep, so Enter
/// keeps the friend, as do Escape and the Keep button.
#[test]
fn remove_starts_on_keep_so_enter_escape_and_keep_remove_nothing() {
    for answer in [
        key(keyboard::key::Named::Enter),
        key(keyboard::key::Named::Escape),
        Message::AnswerRemove(Choice::No),
    ] {
        let (_tmp, store) = home(true, &["alice"]);
        let mut gui = Gui::for_home(store.clone());
        click(&mut gui, "Remove");
        let Some(Dialog::Remove(ask)) = &gui.dialog else {
            panic!("no Remove question");
        };
        assert_eq!(ask.focus(), Choice::No, "starts on Keep");
        let _ = gui.update(answer.clone());
        assert!(gui.dialog.is_none(), "{answer:?}");
        assert!(
            store.load_known_peers().unwrap().lookup("alice").is_some(),
            "{answer:?} removed alice"
        );
    }
}

#[test]
fn only_a_click_on_remove_removes() {
    let (_tmp, store) = home(true, &["alice", "bob"]);
    let mut gui = Gui::for_home(store.clone());
    let _ = gui.update(Message::Remove("alice".into()));
    // The dialog's own Remove button (the header one is behind the veil).
    let _ = gui.update(Message::AnswerRemove(Choice::Yes));
    assert!(store.load_known_peers().unwrap().lookup("alice").is_none());
    assert_eq!(gui.selected.as_deref(), Some("bob"), "selection moves on");
    assert!(matches!(gui.notice, Some((_, Tone::Success))));
}

#[test]
fn copy_puts_the_fingerprint_on_the_clipboard_and_says_so() {
    let (_tmp, store) = home(true, &["alice"]);
    let mut gui = Gui::for_home(store);
    click(&mut gui, "Copy");
    assert_eq!(
        gui.notice.as_ref().map(|(w, _)| w.as_str()),
        Some("Fingerprint copied.")
    );
}

fn record(store: &Store, name: &str, at: u64, file: &str, outcome: beam_core::history::Outcome) {
    let fingerprint = store
        .load_known_peers()
        .unwrap()
        .lookup(name)
        .unwrap()
        .fingerprint()
        .hex();
    beam_core::history::record(
        store,
        &beam_core::history::Entry {
            at,
            direction: beam_core::history::Direction::Sent,
            peer: name.into(),
            fingerprint,
            file: file.into(),
            size: 2048,
            outcome,
            note: None,
        },
    );
}

#[test]
fn a_friends_recent_activity_is_listed_with_its_result() {
    let (_tmp, store) = home(true, &["alice"]);
    let now = beam_core::history::unix_now();
    record(
        &store,
        "alice",
        now - 3600,
        "report.pdf",
        beam_core::history::Outcome::Done,
    );
    record(
        &store,
        "alice",
        now - 60,
        "keys.txt",
        beam_core::history::Outcome::Declined,
    );
    let gui = Gui::for_home(store);
    assert!(shows(&gui, "report.pdf"));
    assert!(shows(&gui, "keys.txt"));
    assert!(shows(&gui, "declined by them"), "the core's own words");
}

/// Friends who answered this week come first, under their own heading.
#[test]
fn recently_active_friends_are_grouped_first() {
    let (_tmp, store) = home(true, &["alice", "bob"]);
    let now = beam_core::history::unix_now();
    record(
        &store,
        "bob",
        now - 600,
        "a.txt",
        beam_core::history::Outcome::Done,
    );
    let gui = Gui::for_home(store);
    assert!(shows(&gui, "RECENTLY ACTIVE"));
    assert!(shows(&gui, "EVERYONE ELSE"));
}
