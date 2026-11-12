//! The window, driven without a screen (`iced_test`): find text, click
//! buttons by their label, and check what changed.

use super::*;
use beam_core::identity::{Identity, KnownPeers, Peer};
use beam_core::transport::endpoint::Bind;
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
    assert!(shows(&gui, "Nothing going out"));
    click(&mut gui, "History");
    assert!(shows(&gui, "Clear history"));
}

#[test]
fn the_theme_switches_between_midnight_and_daylight() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    assert!(gui.dark, "Midnight first");
    let _ = gui.update(Message::Page(Page::Settings));
    assert!(shows(&gui, "Appearance"));
    // The switch is below the fold of a long page, where the simulator
    // cannot click; its message is what the buttons send.
    let _ = gui.update(Message::Dark(false));
    assert!(!gui.dark);
    let _ = gui.update(Message::Dark(true));
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
    Message::Key(keyboard::Key::Named(named), keyboard::Modifiers::empty())
}

/// Ctrl and a letter or digit, as the subscription hands it on.
fn ctrl(c: &str) -> Message {
    Message::Key(
        keyboard::Key::Character(c.into()),
        keyboard::Modifiers::CTRL,
    )
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

// ---------------------------------------------------------------- Patch #4

/// A window on a beam home that pairs over loopback, as the tests need.
fn pairing_window(name: &str) -> (tempfile::TempDir, Store, Gui) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("beam"));
    store.ensure_dirs().unwrap();
    store
        .save_identity(&Identity::generate(name).unwrap(), false)
        .unwrap();
    std::fs::write(store.config_path(), "relay = \"none\"\nport = 0\n").unwrap();
    let mut gui = Gui::for_home(store.clone());
    gui.bind = Bind::Loopback;
    (tmp, store, gui)
}

/// Reads the worker until `done` holds, within a generous time.
fn pump(gui: &mut Gui, what: &str, done: impl Fn(&Gui) -> bool) {
    for _ in 0..600 {
        let _ = gui.update(Message::Pair(PairMsg::Tick));
        if done(gui) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("never got to: {what} ({:?})", gui.dialog);
}

fn is(gui: &Gui, f: impl Fn(&Pairing) -> bool) -> bool {
    matches!(&gui.dialog, Some(Dialog::Pair(state)) if f(state))
}

/// Two windows up to the fingerprint question: one shows its invite, the
/// other pastes it and types the code.
fn up_to_the_question() -> [(tempfile::TempDir, Store, Gui); 2] {
    let (wt, waiter_home, mut waiter) = pairing_window("lab-pc");
    let (jt, joiner_home, mut joiner) = pairing_window("laptop");
    let _ = waiter.update(Message::AddFriend);
    let _ = waiter.update(Message::Pair(PairMsg::ShowMine));
    pump(&mut waiter, "the invite", |g| {
        is(g, |s| matches!(s, Pairing::Showing { .. }))
    });
    let Some(Dialog::Pair(Pairing::Showing { invite, code, .. })) = &waiter.dialog else {
        unreachable!()
    };
    let (invite, code) = (invite.clone(), code.clone());

    let _ = joiner.update(Message::AddFriend);
    let _ = joiner.update(Message::Pair(PairMsg::Invite(invite)));
    let _ = joiner.update(Message::Pair(PairMsg::Name("lab".into())));
    let _ = joiner.update(Message::Pair(PairMsg::Join));
    pump(&mut joiner, "the code box", |g| {
        is(g, |s| matches!(s, Pairing::Code { .. }))
    });
    let _ = joiner.update(Message::Pair(PairMsg::Code(code)));
    let _ = joiner.update(Message::Pair(PairMsg::SendCode));
    for (gui, who) in [(&mut joiner, "joiner"), (&mut waiter, "waiter")] {
        pump(gui, who, |g| {
            is(g, |s| matches!(s, Pairing::Confirm { .. }))
        });
    }
    [(wt, waiter_home, waiter), (jt, joiner_home, joiner)]
}

fn finished(gui: &mut Gui) {
    pump(gui, "the end", |g| g.worker.is_none());
}

#[test]
fn two_windows_pair_and_both_save_the_other() {
    let [
        (_wt, waiter_home, mut waiter),
        (_jt, joiner_home, mut joiner),
    ] = up_to_the_question();
    // Both see both fingerprints, each from their side.
    let fingerprints = |g: &Gui| match &g.dialog {
        Some(Dialog::Pair(Pairing::Confirm { ask, .. })) => {
            (ask.question().peer.clone(), ask.question().own.clone())
        }
        _ => unreachable!(),
    };
    let (jp, jo) = fingerprints(&joiner);
    assert_eq!(fingerprints(&waiter), (jo, jp));

    for gui in [&mut joiner, &mut waiter] {
        let _ = gui.update(Message::Pair(PairMsg::Compared(true)));
        let _ = gui.update(Message::Pair(PairMsg::Answer(Choice::Yes)));
    }
    finished(&mut joiner);
    finished(&mut waiter);
    assert!(joiner.dialog.is_none() && waiter.dialog.is_none());
    assert_eq!(joiner.selected.as_deref(), Some("lab"));
    assert!(
        joiner_home
            .load_known_peers()
            .unwrap()
            .lookup("lab")
            .is_some()
    );
    assert!(
        waiter_home
            .load_known_peers()
            .unwrap()
            .lookup("laptop")
            .is_some()
    );
}

/// The fingerprint question starts on "Don't pair": Enter answers no, and
/// nothing is saved on either side.
#[test]
fn enter_on_the_fingerprint_question_does_not_pair() {
    let [
        (_wt, waiter_home, mut waiter),
        (_jt, joiner_home, mut joiner),
    ] = up_to_the_question();
    let Some(Dialog::Pair(Pairing::Confirm { ask, .. })) = &joiner.dialog else {
        unreachable!()
    };
    assert_eq!(ask.focus(), Choice::No, "starts on Don't pair");
    let _ = joiner.update(Message::Pair(PairMsg::Compared(true)));
    let _ = joiner.update(key(keyboard::key::Named::Enter));
    let _ = waiter.update(Message::Pair(PairMsg::Compared(true)));
    let _ = waiter.update(Message::Pair(PairMsg::Answer(Choice::Yes)));
    finished(&mut joiner);
    finished(&mut waiter);
    assert!(is(&joiner, |s| matches!(s, Pairing::Failed(_))));
    assert!(joiner_home.load_known_peers().unwrap().is_empty());
    assert!(waiter_home.load_known_peers().unwrap().is_empty());
}

/// Pair without the "we compared them" tick is a no.
#[test]
fn pair_without_the_tick_is_a_no() {
    let [
        (_wt, waiter_home, mut waiter),
        (_jt, joiner_home, mut joiner),
    ] = up_to_the_question();
    let _ = joiner.update(Message::Pair(PairMsg::Answer(Choice::Yes)));
    let _ = waiter.update(Message::Pair(PairMsg::Compared(true)));
    let _ = waiter.update(Message::Pair(PairMsg::Answer(Choice::Yes)));
    finished(&mut joiner);
    finished(&mut waiter);
    assert!(joiner_home.load_known_peers().unwrap().is_empty());
    assert!(waiter_home.load_known_peers().unwrap().is_empty());
}

/// Closing the dialog cancels the pairing: the worker goes, and with it
/// any question still waiting (a no).
#[test]
fn closing_the_dialog_cancels_the_pairing() {
    let (_t, _store, mut gui) = pairing_window("lab-pc");
    let _ = gui.update(Message::AddFriend);
    let _ = gui.update(Message::Pair(PairMsg::ShowMine));
    assert!(gui.worker.is_some());
    let _ = gui.update(key(keyboard::key::Named::Escape));
    assert!(gui.dialog.is_none() && gui.worker.is_none());
}

#[test]
fn this_devices_own_invite_is_refused() {
    let (_t, store, mut gui) = pairing_window("lab-pc");
    let own = beam_core::invite::Invite {
        key: store.load_identity().unwrap().verifying_key(),
        relay: None,
        addrs: vec!["192.0.2.7:7820".parse().unwrap()],
    };
    let _ = gui.update(Message::AddFriend);
    let _ = gui.update(Message::Pair(PairMsg::Invite(own.to_string())));
    let _ = gui.update(Message::Pair(PairMsg::Name("me".into())));
    let _ = gui.update(Message::Pair(PairMsg::Join));
    assert!(is(&gui, |s| matches!(
        s,
        Pairing::Start { error: Some(e), .. } if e.contains("own invite")
    )));
    assert!(gui.worker.is_none(), "nothing reached the network");
}

/// An invite for a friend already paired never pairs again; moving them to
/// another relay is a question that starts on Keep (ADR-0038).
#[test]
fn a_relay_move_starts_on_keep() {
    let (_t, store, mut gui) = pairing_window("lab-pc");
    let alice = Identity::generate("alice").unwrap();
    let mut known = store.load_known_peers().unwrap();
    known
        .add(Peer::new("alice", alice.verifying_key()))
        .unwrap();
    store.save_known_peers(&known).unwrap();
    let moved = beam_core::invite::Invite {
        key: alice.verifying_key(),
        relay: Some("https://relay.example.org/".parse().unwrap()),
        addrs: vec!["192.0.2.7:7820".parse().unwrap()],
    }
    .to_string();
    let relay_saved = || {
        store
            .load_known_peers()
            .unwrap()
            .lookup("alice")
            .unwrap()
            .attr("relay")
            .is_some()
    };
    let paste = |gui: &mut Gui| {
        let _ = gui.update(Message::AddFriend);
        let _ = gui.update(Message::Pair(PairMsg::Invite(moved.clone())));
        let _ = gui.update(Message::Pair(PairMsg::Name("whatever".into())));
        let _ = gui.update(Message::Pair(PairMsg::Join));
    };

    paste(&mut gui);
    let Some(Dialog::RelayMove(ask)) = &gui.dialog else {
        panic!("no relay question: {:?}", gui.dialog);
    };
    assert_eq!(ask.focus(), Choice::No, "starts on Keep");
    let _ = gui.update(key(keyboard::key::Named::Enter));
    assert!(gui.dialog.is_none());
    assert!(!relay_saved(), "Enter kept the relay");

    paste(&mut gui);
    let _ = gui.update(Message::Pair(PairMsg::AnswerRelay(Choice::Yes)));
    assert!(relay_saved(), "a yes moves them");
    assert!(
        gui.worker.is_none(),
        "an existing friend is never paired again"
    );
}

/// The code box takes digits only, and a short code never reaches the
/// network.
#[test]
fn the_code_box_takes_six_digits() {
    let (_t, _store, mut gui) = pairing_window("lab-pc");
    gui.dialog = Some(Dialog::Pair(Pairing::Code {
        code: String::new(),
        error: None,
    }));
    let _ = gui.update(Message::Pair(PairMsg::Code("12a 34".into())));
    assert!(is(
        &gui,
        |s| matches!(s, Pairing::Code { code, .. } if code == "1234")
    ));
    let _ = gui.update(Message::Pair(PairMsg::SendCode));
    assert!(is(&gui, |s| matches!(
        s,
        Pairing::Code { error: Some(_), .. }
    )));
}

// ---------------------------------------------------------------- Patch #5

/// A window with the Receiving switch on, a paired friend (bob) who can
/// send to it over loopback, and the folder files are saved to.
struct Receiving {
    _tmp: tempfile::TempDir,
    gui: Gui,
    store: Store,
    downloads: std::path::PathBuf,
    bob: Identity,
    files: std::path::PathBuf,
}

fn receiving_window() -> Receiving {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("beam"));
    store.ensure_dirs().unwrap();
    let me = Identity::generate("alice").unwrap();
    store.save_identity(&me, false).unwrap();
    let bob = Identity::generate("bob").unwrap();
    let mut known = KnownPeers::with_header();
    known.add(Peer::new("bob", bob.verifying_key())).unwrap();
    store.save_known_peers(&known).unwrap();
    let downloads = tmp.path().join("downloads");
    let files = tmp.path().join("files");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(
        store.config_path(),
        format!(
            "relay = \"none\"\nport = 0\nreceive_dir = '{}'\n",
            downloads.display()
        ),
    )
    .unwrap();
    let mut gui = Gui::for_home(store.clone());
    gui.bind = Bind::Loopback;
    gui.notify = false;
    Receiving {
        _tmp: tmp,
        gui,
        store,
        downloads,
        bob,
        files,
    }
}

fn tick(gui: &mut Gui, what: &str, done: impl Fn(&Gui) -> bool) {
    for _ in 0..600 {
        let _ = gui.update(Message::Receive(RecvMsg::Tick));
        if done(gui) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!(
        "never got to: {what} ({:?}, {:?})",
        gui.requests, gui.dialog
    );
}

/// Turns the switch on and waits until the window's link is connected.
fn switch_on(r: &mut Receiving) {
    let _ = r.gui.update(Message::Receive(RecvMsg::Toggle));
    tick(&mut r.gui, "receiving and connected", |g| {
        g.requests.switch == beam_model::requests::Switch::On
            && g.requests.link == beam_model::requests::Link::Connected
    });
}

/// bob sends `report.pdf` to the window's receiver, on his own thread.
fn bob_sends(r: &Receiving) -> std::thread::JoinHandle<bool> {
    use beam_core::agent::status::{self, Running};
    use beam_core::transfer::{SendOptions, SilentReporter};
    use beam_core::transport::dial::{dial_transfer, send_on};
    use beam_core::transport::endpoint;
    let invite = match status::read(&r.store) {
        Running::Yes(status) => status.invite.expect("an invite"),
        other => panic!("not receiving: {other:?}"),
    };
    let at = invite
        .parse::<beam_core::invite::Invite>()
        .unwrap()
        .endpoint_addr();
    let path = r.files.join("report.pdf");
    std::fs::write(&path, b"the quarterly numbers").unwrap();
    let bob = r.bob.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let endpoint = endpoint::bind(
                &bob,
                &beam_core::config::Relay::Disabled,
                Bind::Loopback,
                0,
                &[],
            )
            .await
            .unwrap();
            let connection = dial_transfer(&endpoint, at).await.expect("dial");
            let mut options = SendOptions::new(
                path,
                beam_core::identity::encode_public_key(&bob.verifying_key()),
            );
            let sent = send_on(&connection, &mut options, &mut SilentReporter)
                .await
                .is_ok();
            endpoint.close().await;
            sent
        })
    })
}

fn waiting_id(gui: &Gui) -> u64 {
    gui.requests.waiting[0].id
}

/// The whole path: a paired friend sends, the request only waits (it never
/// opens a question by itself, and Enter with nothing open answers
/// nothing), the question starts on Decline, and Enter declines: nothing
/// is saved.
#[test]
fn a_request_waits_and_enter_on_the_question_declines() {
    let mut r = receiving_window();
    switch_on(&mut r);
    let sending = bob_sends(&r);
    tick(&mut r.gui, "a request", |g| !g.requests.waiting.is_empty());
    assert!(r.gui.dialog.is_none(), "a request never opens by itself");
    let _ = r.gui.update(key(keyboard::key::Named::Enter));
    assert_eq!(
        r.gui.requests.waiting.len(),
        1,
        "a stray Enter answers nothing"
    );

    let id = waiting_id(&r.gui);
    let _ = r.gui.update(Message::Receive(RecvMsg::Review(id)));
    let Some(Dialog::Accept(ask)) = &r.gui.dialog else {
        panic!("no question: {:?}", r.gui.dialog);
    };
    assert_eq!(ask.focus(), Choice::No, "starts on Decline");
    assert!(shows(&r.gui, "report.pdf"));
    assert!(shows(&r.gui, "bob"));
    let _ = r.gui.update(key(keyboard::key::Named::Enter));

    assert!(!sending.join().unwrap(), "the sender was declined");
    assert!(r.gui.requests.waiting.is_empty());
    assert!(
        !r.downloads.join("report.pdf").exists(),
        "nothing was saved"
    );

    let _ = r.gui.update(Message::Receive(RecvMsg::Toggle));
    tick(&mut r.gui, "off", |g| {
        g.requests.switch == beam_model::requests::Switch::Off
    });
}

/// Only a click on Accept accepts, and then the file arrives intact.
#[test]
fn a_click_on_accept_receives_the_file() {
    let mut r = receiving_window();
    switch_on(&mut r);
    let sending = bob_sends(&r);
    tick(&mut r.gui, "a request", |g| !g.requests.waiting.is_empty());
    let id = waiting_id(&r.gui);
    let _ = r.gui.update(Message::Receive(RecvMsg::Review(id)));
    let _ = r.gui.update(Message::Receive(RecvMsg::Answer(Choice::Yes)));
    assert!(sending.join().unwrap(), "the sender was accepted");
    assert_eq!(
        std::fs::read(r.downloads.join("report.pdf")).unwrap(),
        b"the quarterly numbers"
    );
}

/// Escape on the question is "later": it closes, and the request keeps
/// waiting for an answer (it expires as a no).
#[test]
fn escape_on_the_question_leaves_the_request_waiting() {
    let mut r = receiving_window();
    switch_on(&mut r);
    let _sending = bob_sends(&r);
    tick(&mut r.gui, "a request", |g| !g.requests.waiting.is_empty());
    let id = waiting_id(&r.gui);
    let _ = r.gui.update(Message::Receive(RecvMsg::Review(id)));
    let _ = r.gui.update(key(keyboard::key::Named::Escape));
    assert!(r.gui.dialog.is_none());
    assert_eq!(r.gui.requests.waiting.len(), 1, "still waiting");
    assert!(!r.downloads.join("report.pdf").exists());
}

/// While a file arrives, the switch and the close button ask first, and
/// both questions start on the safe answer: keep receiving, stay.
#[test]
fn stopping_or_closing_mid_file_asks_and_starts_on_no() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    gui.requests.switch = beam_model::requests::Switch::On;
    gui.requests.receiving = Some(beam_model::requests::Receiving {
        done: 1,
        total: 10,
        relay: false,
    });

    let _ = gui.update(Message::Receive(RecvMsg::Toggle));
    let Some(Dialog::StopReceiving(ask)) = &gui.dialog else {
        panic!("no question: {:?}", gui.dialog);
    };
    assert_eq!(ask.focus(), Choice::No);
    let _ = gui.update(key(keyboard::key::Named::Enter));
    assert_eq!(
        gui.requests.switch,
        beam_model::requests::Switch::On,
        "kept"
    );

    let id = iced::window::Id::unique();
    let _ = gui.update(Message::Receive(RecvMsg::CloseRequested(id)));
    let Some(Dialog::Leave(ask, _)) = &gui.dialog else {
        panic!("no question: {:?}", gui.dialog);
    };
    assert_eq!(ask.focus(), Choice::No, "starts on Stay");
    let _ = gui.update(key(keyboard::key::Named::Escape));
    assert!(gui.dialog.is_none(), "stayed");
}

/// The Inbox count shows on the rail, and the Inbox lists the request.
#[test]
fn a_waiting_request_shows_on_the_rail_and_in_the_inbox() {
    let mut r = receiving_window();
    switch_on(&mut r);
    let _sending = bob_sends(&r);
    tick(&mut r.gui, "a request", |g| !g.requests.waiting.is_empty());
    let _ = r.gui.update(Message::Page(Page::Inbox));
    assert!(shows(&r.gui, "Waiting (1)"));
    assert!(shows(&r.gui, "Review"));
    assert!(shows(&r.gui, "Receiving is on"));
}

// ---------------------------------------------------------------- Patch #6

/// bob's window, paired with alice (`r`), knowing where her switch listens.
fn bobs_window(r: &Receiving) -> (tempfile::TempDir, Gui, std::path::PathBuf) {
    use beam_core::agent::status::{self, Running};
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("beam"));
    store.ensure_dirs().unwrap();
    store.save_identity(&r.bob, false).unwrap();
    std::fs::write(store.config_path(), "relay = \"none\"\nport = 0\n").unwrap();
    let alice = r.store.load_identity().unwrap();
    let mut known = KnownPeers::with_header();
    known
        .add(Peer::new("alice", alice.verifying_key()))
        .unwrap();
    store.save_known_peers(&known).unwrap();
    let invite = match status::read(&r.store) {
        Running::Yes(status) => status.invite.expect("an invite"),
        other => panic!("alice is not receiving: {other:?}"),
    };
    beam_core::actions::pair::save_location(&store, &invite.parse().unwrap(), true).unwrap();
    let file = tmp.path().join("report.pdf");
    std::fs::write(&file, b"the quarterly numbers").unwrap();
    let mut gui = Gui::for_home(store);
    gui.bind = Bind::Loopback;
    (tmp, gui, file)
}

/// Runs both windows until `done` holds for bob's.
fn both(alice: &mut Gui, bob: &mut Gui, what: &str, done: impl Fn(&Gui, &Gui) -> bool) {
    for _ in 0..600 {
        let _ = alice.update(Message::Receive(RecvMsg::Tick));
        let _ = bob.update(Message::Send(SendMsg::Tick));
        if done(alice, bob) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("never got to: {what} ({:?})", bob.sending);
}

fn result(bob: &Gui) -> Option<&Result<String, String>> {
    bob.sending.as_ref().and_then(|s| s.result.as_ref())
}

/// A send from the window waits for the friend's yes; nothing arrives
/// before it, and the file arrives intact after.
#[test]
fn a_send_waits_for_their_yes_then_arrives() {
    let mut r = receiving_window();
    switch_on(&mut r);
    let (_t, mut bob, file) = bobs_window(&r);
    let _ = bob.update(Message::Send(SendMsg::Picked("alice".into(), Some(file))));
    assert_eq!(bob.page, Page::Transfers);
    both(&mut r.gui, &mut bob, "the request", |a, _| {
        !a.requests.waiting.is_empty()
    });
    both(&mut r.gui, &mut bob, "waiting for the yes", |_, b| {
        b.sending.as_ref().map(|s| s.stage) == Some(beam_model::sending::Stage::Waiting)
    });
    assert!(shows(
        &bob,
        "Waiting for alice to accept. Nothing is sent before they do."
    ));
    assert!(
        !r.downloads.join("report.pdf").exists(),
        "nothing before the yes"
    );

    let id = waiting_id(&r.gui);
    let _ = r.gui.update(Message::Receive(RecvMsg::Review(id)));
    let _ = r.gui.update(Message::Receive(RecvMsg::Answer(Choice::Yes)));
    both(&mut r.gui, &mut bob, "the end", |_, b| result(b).is_some());
    assert!(matches!(result(&bob), Some(Ok(words)) if words.contains("Saved on their side")));
    assert_eq!(
        std::fs::read(r.downloads.join("report.pdf")).unwrap(),
        b"the quarterly numbers"
    );
    assert!(bob.outgoing.is_none());
}

#[test]
fn a_declined_send_says_so() {
    let mut r = receiving_window();
    switch_on(&mut r);
    let (_t, mut bob, file) = bobs_window(&r);
    let _ = bob.update(Message::Send(SendMsg::Picked("alice".into(), Some(file))));
    both(&mut r.gui, &mut bob, "the request", |a, _| {
        !a.requests.waiting.is_empty()
    });
    let id = waiting_id(&r.gui);
    let _ = r.gui.update(Message::Receive(RecvMsg::Review(id)));
    let _ = r.gui.update(key(keyboard::key::Named::Enter));
    both(&mut r.gui, &mut bob, "the end", |_, b| result(b).is_some());
    assert!(
        matches!(result(&bob), Some(Err(words)) if words.contains("declined")),
        "{:?}",
        result(&bob)
    );
    assert!(!r.downloads.join("report.pdf").exists());
}

/// Stop while waiting: the friend is told, and the request goes away.
#[test]
fn stop_while_waiting_withdraws_the_request() {
    let mut r = receiving_window();
    switch_on(&mut r);
    let (_t, mut bob, file) = bobs_window(&r);
    let _ = bob.update(Message::Send(SendMsg::Picked("alice".into(), Some(file))));
    both(&mut r.gui, &mut bob, "the request", |a, _| {
        !a.requests.waiting.is_empty()
    });
    let _ = bob.update(Message::Send(SendMsg::Cancel));
    both(&mut r.gui, &mut bob, "stopped, and withdrawn", |a, b| {
        result(b).is_some() && a.requests.waiting.is_empty()
    });
    assert!(matches!(result(&bob), Some(Err(words)) if words.starts_with("Stopped")));
    let _ = bob.update(Message::Send(SendMsg::Dismiss));
    assert!(bob.sending.is_none());
}

/// One at a time, as in the view; a dropped file only proposes, and
/// Enter on that question sends nothing; closing mid-send asks first.
#[test]
fn one_at_a_time_drops_propose_and_closing_asks() {
    let mut r = receiving_window();
    switch_on(&mut r);
    let (_t, mut bob, file) = bobs_window(&r);
    let _ = bob.update(Message::Page(Page::Friends));
    let _ = bob.update(Message::Send(SendMsg::Dropped(file.clone())));
    assert!(matches!(bob.dialog, Some(Dialog::SendDropped { .. })));
    let _ = bob.update(key(keyboard::key::Named::Enter));
    assert!(
        bob.dialog.is_none() && bob.outgoing.is_none(),
        "Enter sent nothing"
    );

    let _ = bob.update(Message::Send(SendMsg::Dropped(file.clone())));
    let _ = bob.update(Message::Send(SendMsg::AnswerDrop(true)));
    assert!(bob.outgoing.is_some(), "a click on Send sends");

    let _ = bob.update(Message::Send(SendMsg::Picked("alice".into(), Some(file))));
    assert!(matches!(&bob.notice, Some((w, _)) if w.starts_with("One file at a time")));

    let id = iced::window::Id::unique();
    let _ = bob.update(Message::Receive(RecvMsg::CloseRequested(id)));
    let Some(Dialog::Leave(ask, _)) = &bob.dialog else {
        panic!("no question: {:?}", bob.dialog);
    };
    assert_eq!(ask.focus(), Choice::No, "starts on Stay");
}

// ---------------------------------------------------------------- Patch #7

fn config_of(store: &Store) -> beam_core::config::Config {
    beam_core::config::Config::load(&store.config_path()).unwrap()
}

/// Router port mapping makes this device findable: turning it on is a
/// question that starts on "Keep it off"; Enter and Escape keep it off.
#[test]
fn port_mapping_on_asks_and_starts_on_keep_it_off() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store.clone());
    for no in [
        key(keyboard::key::Named::Enter),
        key(keyboard::key::Named::Escape),
    ] {
        let _ = gui.update(Message::Settings(SettingsMsg::PortMapping(true)));
        let Some(Dialog::PortMapping(ask)) = &gui.dialog else {
            panic!("no question: {:?}", gui.dialog);
        };
        assert_eq!(ask.focus(), Choice::No, "starts on Keep it off");
        let _ = gui.update(no);
        assert!(gui.dialog.is_none());
        assert!(!config_of(&store).agent_port_mapping, "still off");
    }
    let _ = gui.update(Message::Settings(SettingsMsg::PortMapping(true)));
    let _ = gui.update(Message::Settings(SettingsMsg::AnswerPortMapping(
        Choice::Yes,
    )));
    assert!(config_of(&store).agent_port_mapping, "a click on Turn on");
    let _ = gui.update(Message::Settings(SettingsMsg::PortMapping(false)));
    assert!(
        !config_of(&store).agent_port_mapping,
        "off needs no question"
    );
}

#[test]
fn the_receive_folder_is_saved_and_reset() {
    let (tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store.clone());
    let folder = tmp.path().join("incoming");
    let _ = gui.update(Message::Settings(SettingsMsg::FolderPicked(Some(
        folder.clone(),
    ))));
    assert!(folder.is_dir());
    assert!(config_of(&store).receive_dir.is_some());
    let _ = gui.update(Message::Page(Page::Settings));
    assert!(shows(&gui, "Use the default"));
    let _ = gui.update(Message::Settings(SettingsMsg::DefaultFolder));
    assert_eq!(config_of(&store).receive_dir, None);
}

/// Clearing the history starts on Keep.
#[test]
fn clearing_the_history_asks_and_starts_on_keep() {
    let (_tmp, store) = home(true, &["alice"]);
    let now = beam_core::history::unix_now();
    record(
        &store,
        "alice",
        now - 60,
        "a.txt",
        beam_core::history::Outcome::Done,
    );
    let mut gui = Gui::for_home(store.clone());
    let _ = gui.update(Message::Settings(SettingsMsg::ClearHistory));
    let Some(Dialog::ClearHistory(ask)) = &gui.dialog else {
        panic!("no question: {:?}", gui.dialog);
    };
    assert_eq!(ask.focus(), Choice::No, "starts on Keep");
    let _ = gui.update(key(keyboard::key::Named::Enter));
    assert_eq!(beam_core::history::read(&store).len(), 1, "kept");
    let _ = gui.update(Message::Settings(SettingsMsg::ClearHistory));
    let _ = gui.update(Message::Settings(SettingsMsg::AnswerClear(Choice::Yes)));
    assert!(beam_core::history::read(&store).is_empty());
}

#[test]
fn history_filters_and_search_narrow_the_list() {
    let (_tmp, store) = home(true, &["alice", "bob"]);
    let now = beam_core::history::unix_now();
    record(
        &store,
        "alice",
        now - 300,
        "report.pdf",
        beam_core::history::Outcome::Done,
    );
    record(
        &store,
        "bob",
        now - 200,
        "keys.txt",
        beam_core::history::Outcome::Declined,
    );
    let mut gui = Gui::for_home(store);
    let _ = gui.update(Message::Page(Page::History));
    assert!(shows(&gui, "2 of 2"));
    let _ = gui.update(Message::Settings(SettingsMsg::Filter(
        HistoryFilter::Declined,
    )));
    assert!(shows(&gui, "1 of 2") && shows(&gui, "keys.txt"));
    let _ = gui.update(Message::Settings(SettingsMsg::Filter(HistoryFilter::All)));
    let _ = gui.update(Message::Settings(SettingsMsg::Search("alice".into())));
    assert!(shows(&gui, "1 of 2") && shows(&gui, "report.pdf"));
}

/// Without the beam program next to the window, the page says why the
/// service controls are unavailable instead of registering beam-gui.
#[test]
fn the_service_controls_need_beam_beside_the_window() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    gui.beam_program = None;
    let _ = gui.update(Message::Page(Page::Settings));
    assert!(shows(&gui, "Background service"));
    assert!(shows(&gui, "Not running"));

    // The lookup itself: only a real file called beam counts.
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(settings::beam_in(dir.path()), None);
    let beam = dir
        .path()
        .join(format!("beam{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&beam, b"").unwrap();
    assert_eq!(settings::beam_in(dir.path()), Some(beam));
}

// ---------------------------------------------------------------- Patch #8

fn tab() -> Message {
    key(keyboard::key::Named::Tab)
}

/// Without a mouse: Delete asks, the mark starts on Keep, and Escape keeps
/// whatever the mark is on. A yes takes a deliberate move of the mark.
#[test]
fn removing_by_keyboard_starts_on_keep_and_tab_moves_the_mark() {
    let (_tmp, store) = home(true, &["alice"]);
    let mut gui = Gui::for_home(store.clone());
    let _ = gui.update(key(keyboard::key::Named::Delete));
    let Some(Dialog::Remove(ask)) = &gui.dialog else {
        panic!("no question: {:?}", gui.dialog);
    };
    assert_eq!(ask.focus(), Choice::No, "starts on Keep");
    let _ = gui.update(tab());
    let Some(Dialog::Remove(ask)) = &gui.dialog else {
        panic!("closed by Tab");
    };
    assert_eq!(ask.focus(), Choice::Yes, "Tab moved the mark");
    let _ = gui.update(key(keyboard::key::Named::Escape));
    assert_eq!(store.load_known_peers().unwrap().peers().len(), 1, "kept");

    // beam's default: Remove on the left, Keep (purple) on the right.
    let _ = gui.update(key(keyboard::key::Named::Delete));
    let _ = gui.update(key(keyboard::key::Named::ArrowLeft));
    let _ = gui.update(key(keyboard::key::Named::ArrowRight));
    let _ = gui.update(key(keyboard::key::Named::Enter));
    assert_eq!(
        store.load_known_peers().unwrap().peers().len(),
        1,
        "Right put the mark back on Keep"
    );

    // The other setting swaps the sides, and the arrows follow them.
    let _ = gui.update(Message::Emphasis(Emphasis::Action));
    let _ = gui.update(key(keyboard::key::Named::Delete));
    let Some(Dialog::Remove(ask)) = &gui.dialog else {
        panic!("no question");
    };
    assert_eq!(ask.focus(), Choice::No, "still starts on Keep");
    let _ = gui.update(key(keyboard::key::Named::ArrowRight));
    let _ = gui.update(key(keyboard::key::Named::Enter));
    assert!(store.load_known_peers().unwrap().peers().is_empty());
}

/// Every question with two answers can be steered, and the Accept
/// question's Escape still decides later rather than answering.
#[test]
fn every_question_starts_marked_on_the_safe_answer() {
    let (_tmp, store) = home(true, &["alice"]);
    let mut gui = Gui::for_home(store);
    let opened = [
        Message::Settings(SettingsMsg::PortMapping(true)),
        Message::Settings(SettingsMsg::ClearHistory),
        Message::Remove("alice".into()),
    ];
    for open in opened {
        let _ = gui.update(open);
        let focus = match &gui.dialog {
            Some(Dialog::PortMapping(a)) => a.focus(),
            Some(Dialog::ClearHistory(a)) => a.focus(),
            Some(Dialog::Remove(a)) => a.focus(),
            other => panic!("unexpected: {other:?}"),
        };
        assert_eq!(focus, Choice::No);
        assert!(shows(&gui, "Keep") || shows(&gui, "Keep it off"));
        let _ = gui.update(key(keyboard::key::Named::Escape));
        assert!(gui.dialog.is_none());
    }
}

/// Pairing by keyboard: Pair cannot be pressed before the tick, so Enter
/// on it waits instead of reading "Don't pair"; Space ticks.
#[test]
fn pairing_by_keyboard_waits_for_the_tick() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    gui.dialog = Some(Dialog::Pair(Pairing::Confirm {
        ask: Ask::new(beam_model::ask::SameFingerprints {
            name: "carol".into(),
            peer: "ab".repeat(32),
            own: "cd".repeat(32),
        }),
        compared: false,
    }));
    let _ = gui.update(tab());
    let _ = gui.update(key(keyboard::key::Named::Enter));
    let Some(Dialog::Pair(Pairing::Confirm { ask, compared })) = &gui.dialog else {
        panic!(
            "Enter on an unticked Pair ended the pairing: {:?}",
            gui.dialog
        );
    };
    assert_eq!(ask.focus(), Choice::Yes);
    assert!(!compared);
    let _ = gui.update(key(keyboard::key::Named::Space));
    assert!(matches!(
        &gui.dialog,
        Some(Dialog::Pair(Pairing::Confirm { compared: true, .. }))
    ));
}

#[test]
fn shortcuts_switch_pages_and_walk_the_friends() {
    let (_tmp, store) = home(true, &["alice", "bob", "carol"]);
    let mut gui = Gui::for_home(store);
    for (digit, page) in [
        ("2", Page::Transfers),
        ("3", Page::Inbox),
        ("4", Page::History),
        ("5", Page::Settings),
        ("1", Page::Friends),
    ] {
        let _ = gui.update(ctrl(digit));
        assert_eq!(gui.page, page, "Ctrl+{digit}");
    }
    assert_eq!(gui.selected.as_deref(), Some("alice"));
    let _ = gui.update(key(keyboard::key::Named::ArrowDown));
    let _ = gui.update(key(keyboard::key::Named::ArrowDown));
    assert_eq!(gui.selected.as_deref(), Some("carol"));
    let _ = gui.update(key(keyboard::key::Named::ArrowDown));
    assert_eq!(gui.selected.as_deref(), Some("carol"), "stays at the end");
    let _ = gui.update(key(keyboard::key::Named::ArrowUp));
    assert_eq!(gui.selected.as_deref(), Some("bob"));
    // Up and Down follow the search, as the list does.
    let _ = gui.update(Message::Search("a".into()));
    let _ = gui.update(key(keyboard::key::Named::ArrowDown));
    assert_eq!(gui.selected.as_deref(), Some("alice"));
    // F2 renames, Ctrl+N adds a friend.
    let _ = gui.update(key(keyboard::key::Named::F2));
    assert!(matches!(gui.dialog, Some(Dialog::Rename { .. })));
    let _ = gui.update(key(keyboard::key::Named::Escape));
    let _ = gui.update(ctrl("n"));
    assert!(matches!(gui.dialog, Some(Dialog::Pair(_))));
    // With a dialog open, page shortcuts do nothing.
    let _ = gui.update(ctrl("5"));
    assert_eq!(gui.page, Page::Friends);
}

/// Zoom scales the whole window; the layout's breakpoints follow the room
/// left after zooming.
#[test]
fn zoom_steps_and_the_layout_follows() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    assert_eq!(gui.scale_factor(), 1.0);
    let _ = gui.update(ctrl("="));
    assert_eq!(gui.scale_factor(), 1.1);
    let _ = gui.update(ctrl("-"));
    let _ = gui.update(ctrl("-"));
    assert_eq!(gui.scale_factor(), 0.9);
    for _ in 0..20 {
        let _ = gui.update(ctrl("="));
    }
    assert_eq!(gui.scale_factor(), 2.0, "stops at the largest step");
    let _ = gui.update(ctrl("0"));
    assert_eq!(gui.scale_factor(), 1.0);

    // 1200 px at 150% leaves the layout 800: the narrower friends list.
    let _ = gui.update(Message::Zoom(1.5));
    assert_eq!(gui.pane_width(), 240.0);
    // A resize arrives in zoomed units; the window's own width is kept.
    let _ = gui.update(Message::Resized(800.0));
    assert_eq!(gui.width, 1200.0);
    let _ = gui.update(Message::Page(Page::Settings));
    assert!(shows(&gui, "150%"), "Settings shows the zoom");
}

/// A tiling compositor's narrow column: the friends list keeps only
/// faces, and selecting still works.
#[test]
fn a_very_narrow_window_keeps_only_faces() {
    let (_tmp, store) = home(true, &["alice", "bob"]);
    let mut gui = Gui::for_home(store);
    assert!(shows(&gui, "ALL FRIENDS"));
    let _ = gui.update(Message::Resized(700.0));
    assert!(gui.compact());
    assert_eq!(gui.pane_width(), 64.0);
    assert!(!shows(&gui, "ALL FRIENDS"));
    let _ = gui.update(key(keyboard::key::Named::ArrowDown));
    assert_eq!(gui.selected.as_deref(), Some("bob"));
    let fingerprint = gui.friend("bob").unwrap().fingerprint.clone();
    assert!(shows(
        &gui,
        &format!("SHA256:{}", crate::view::grouped(&fingerprint))
    ));
}

/// A damaged known_peers is said on every page; the friends list does not
/// pretend there are none.
#[test]
fn an_unreadable_friends_file_is_said_not_hidden() {
    let (_tmp, store) = home(true, &["alice"]);
    std::fs::write(store.known_peers_path(), "alice\n").unwrap();
    let mut gui = Gui::for_home(store);
    assert!(shows(&gui, "Friends could not be read"));
    assert!(!shows(&gui, "No friends yet"));
    for page in [Page::Inbox, Page::History, Page::Settings] {
        let _ = gui.update(Message::Page(page));
        assert!(shows(&gui, "Try again"), "the problem is shown on {page:?}");
    }
}

/// Many friends and a long history: everything loads and draws, "last
/// seen" is found in one pass (`Snapshot::load`), and a history table cut
/// to its newest rows says so.
#[test]
fn many_friends_and_a_long_history_load_and_draw() {
    let names: Vec<String> = (0..400).map(|i| format!("friend{i:03}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (_tmp, store) = home(true, &refs);
    let now = beam_core::history::unix_now();
    for i in 0..2_000u64 {
        let who = &names[(i as usize * 7) % names.len()];
        record(
            &store,
            who,
            now - 10_000 + i,
            "f.bin",
            beam_core::history::Outcome::Done,
        );
    }
    let mut gui = Gui::for_home(store);
    let _ = gui.update(Message::Refresh);
    let _ = simulator(gui.view());
    let _ = gui.update(Message::Page(Page::History));
    assert!(
        shows(
            &gui,
            "Showing the newest 200. Filter or search to find older ones, or run beam history in a terminal."
        ),
        "a cut list says so"
    );
    assert_eq!(gui.snapshot.as_ref().unwrap().friends.len(), 400);
    assert!(
        gui.snapshot
            .as_ref()
            .unwrap()
            .friends
            .iter()
            .all(|f| f.last_seen.is_some())
    );
}

/// Opening Settings never waits for the OS: asking whether the agent
/// starts at sign-in runs `reg` or `systemctl`, and on the window's
/// thread that froze the window for seconds. The switch is off and still
/// until the answer arrives.
#[test]
fn opening_settings_does_not_wait_for_the_os() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    let _ = gui.update(Message::Page(Page::Settings));
    assert_eq!(gui.page, Page::Settings);
    assert_eq!(gui.at_sign_in, None, "not asked on the window's thread");
    let _ = gui.update(Message::Settings(SettingsMsg::AtSignInRead(true)));
    assert_eq!(gui.at_sign_in, Some(true));
}

// ---------------------------------------------------------------- Patch #9

/// Theme, zoom, the highlighted answer and software rendering are saved
/// in gui.toml beside config.toml, and a new window opens with them.
#[test]
fn preferences_are_saved_beside_the_config_and_read_back() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store.clone());
    assert!(gui.dark && gui.zoom == 1.0 && gui.emphasis == Emphasis::Safe && !gui.software);
    let _ = gui.update(Message::Dark(false));
    let _ = gui.update(Message::Zoom(1.25));
    let _ = gui.update(Message::Emphasis(Emphasis::Action));
    let _ = gui.update(Message::Software(true));
    assert_eq!(
        store.gui_prefs_path().parent(),
        store.config_path().parent(),
        "in the beam folder, beside config.toml"
    );
    assert!(store.gui_prefs_path().is_file());

    let again = Gui::for_home(store.clone());
    assert!(!again.dark);
    assert_eq!(again.zoom, 1.25);
    assert_eq!(again.emphasis, Emphasis::Action);
    assert!(again.software);
    // The command line's settings are not touched.
    assert!(
        !store.config_path().exists() || {
            let text = std::fs::read_to_string(store.config_path()).unwrap();
            !text.contains("zoom") && !text.contains("software")
        }
    );
}

/// A damaged gui.toml: the defaults, and a notice saying why.
#[test]
fn a_damaged_preferences_file_uses_the_defaults_and_says_so() {
    let (_tmp, store) = home(true, &[]);
    std::fs::write(store.gui_prefs_path(), "theme = \"purple\"\n").unwrap();
    let gui = Gui::for_home(store);
    assert!(gui.dark && gui.zoom == 1.0 && !gui.software);
    assert!(matches!(gui.notice, Some((_, Tone::Problem))));
}

/// Restart now closes like the close button. If that asks "Close beam?"
/// (a file is moving), answering Stay, or closing the question, calls the
/// restart off, so a later close does not start beam again.
#[test]
fn staying_calls_a_restart_off() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    for dismiss in [
        key(keyboard::key::Named::Escape),
        Message::Receive(RecvMsg::AnswerLeave(Choice::No)),
        Message::CloseDialog,
    ] {
        gui.restarting = true;
        gui.dialog = Some(Dialog::Leave(
            Ask::new(beam_model::ask::LeaveMidTransfer),
            window::Id::unique(),
        ));
        let _ = gui.update(dismiss);
        assert!(gui.dialog.is_none());
        assert!(!gui.restarting);
    }
}

/// Restart now appears only while the switch and the running window
/// differ; a renderer the person chose with ICED_BACKEND is said instead.
#[test]
fn restart_now_appears_only_when_the_renderer_would_change() {
    let (_tmp, store) = home(true, &[]);
    let mut gui = Gui::for_home(store);
    gui.renderer = crate::renderer::Chosen::Default;
    let _ = gui.update(Message::Page(Page::Settings));
    assert!(!shows(&gui, "Restart now"));
    let _ = gui.update(Message::Software(true));
    assert!(shows(&gui, "Restart now"));
    // Started in software by beam: nothing to change.
    gui.renderer = crate::renderer::Chosen::Beam;
    assert!(!shows(&gui, "Restart now"));
    gui.renderer = crate::renderer::Chosen::Person("wgpu".into());
    assert!(!shows(&gui, "Restart now"));
    assert!(shows(
        &gui,
        "Set for this window by ICED_BACKEND=wgpu; this switch applies when that is not set."
    ));
}
