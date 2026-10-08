//! Command-level tests.
//!
//! Every command is driven through `cli::execute` with in-memory streams and a
//! temporary beam home directory, so nothing here spawns a process or touches
//! the real `~/.beam`.

use std::fs;
use std::path::{Path, PathBuf};

use beam::cli::{EXIT_ERROR, EXIT_NOT_IMPLEMENTED, EXIT_OK, Io, execute};
use beam::identity::{
    HEADER, Identity, KNOWN_PEERS_NAME, PRIVATE_KEY_NAME, PUBLIC_KEY_NAME, Peer, Store,
    decode_public_key,
};
use serde_json::Value;
use tempfile::TempDir;

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs the command tree against `dir`, feeding it `stdin`.
fn run(dir: &Path, stdin: &str, args: &[&str]) -> Outcome {
    let mut input = stdin.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();

    let mut argv: Vec<String> = vec!["--beam-dir".to_string(), dir.display().to_string()];
    argv.extend(args.iter().map(|a| (*a).to_string()));

    let code = {
        let mut io = Io {
            input: &mut input,
            out: &mut out,
            err: &mut err,
        };
        execute(argv, &mut io)
    };

    Outcome {
        code,
        stdout: String::from_utf8(out).expect("stdout is utf-8"),
        stderr: String::from_utf8(err).expect("stderr is utf-8"),
    }
}

fn beam_dir() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join(".beam");
    (tmp, dir)
}

/// A beam home directory that already has an identity.
fn initialised() -> (TempDir, PathBuf) {
    let (tmp, dir) = beam_dir();
    let outcome = run(&dir, "", &["init"]);
    assert_eq!(outcome.code, EXIT_OK, "init failed: {}", outcome.stderr);
    (tmp, dir)
}

/// Writes a known_peers file with two entries.
fn seed_peers(dir: &Path) {
    let store = Store::new(dir);
    let mut known = store.load_known_peers().expect("load known_peers");
    for name in ["alice", "bob"] {
        let identity = Identity::generate(name).expect("generate");
        known
            .add(Peer::new(name, identity.verifying_key()))
            .expect("add peer");
    }
    store.save_known_peers(&known).expect("save known_peers");
}

#[test]
fn init_creates_an_identity() {
    let (_tmp, dir) = beam_dir();
    let outcome = run(&dir, "", &["init"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);

    for expected in ["Short ID", "Fingerprint", "SHA256:"] {
        assert!(
            outcome.stdout.contains(expected),
            "output is missing {expected:?}:\n{}",
            outcome.stdout
        );
    }
    for name in [PRIVATE_KEY_NAME, PUBLIC_KEY_NAME, KNOWN_PEERS_NAME] {
        assert!(dir.join(name).exists(), "{name} was not created");
    }
}

#[test]
fn init_refuses_to_overwrite() {
    let (_tmp, dir) = initialised();
    let before = run(&dir, "", &["whoami", "--json"]).stdout;

    let outcome = run(&dir, "", &["init"]);
    assert_eq!(outcome.code, EXIT_ERROR, "a second init succeeded");
    assert!(
        outcome.stderr.contains("already exists"),
        "unhelpful error: {}",
        outcome.stderr
    );
    assert_eq!(
        run(&dir, "", &["whoami", "--json"]).stdout,
        before,
        "the identity changed even though init failed"
    );

    let forced = run(&dir, "", &["init", "--force"]);
    assert_eq!(forced.code, EXIT_OK, "{}", forced.stderr);
    assert_ne!(
        run(&dir, "", &["whoami", "--json"]).stdout,
        before,
        "--force did not generate a new key"
    );
}

#[test]
fn whoami_reports_the_key_on_disk() {
    let (_tmp, dir) = initialised();
    let outcome = run(&dir, "", &["whoami", "--json"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);

    let json: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
    assert_eq!(json["short_id"].as_str().expect("short_id").len(), 9);
    assert_eq!(json["key_type"], "ed25519");
    assert_eq!(json["dir"], dir.display().to_string());

    let key = decode_public_key(json["public_key"].as_str().expect("public_key")).expect("key");
    assert_eq!(
        json["fingerprint"],
        beam::identity::Fingerprint::of(&key).to_string(),
        "the reported fingerprint does not match the reported key"
    );
}

/// ADR-0037: `whoami` says whether `listen` is running, and without it there
/// is no code to show.
#[test]
fn whoami_says_when_listen_is_not_running() {
    let (_tmp, dir) = initialised();
    let text = run(&dir, "", &["whoami"]);
    assert_eq!(text.code, EXIT_OK, "{}", text.stderr);
    assert!(
        text.stdout.contains("beam listen is not running"),
        "{}",
        text.stdout
    );
    assert!(!text.stdout.contains("Pairing code"), "{}", text.stdout);

    let json = run(&dir, "", &["whoami", "--json"]);
    let json: Value = serde_json::from_str(&json.stdout).expect("valid JSON");
    assert!(json["listening"].is_null(), "{json}");
}

#[test]
fn whoami_without_an_identity_points_at_init() {
    let (_tmp, dir) = beam_dir();
    let outcome = run(&dir, "", &["whoami"]);
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(
        outcome.stderr.contains("beam init"),
        "error does not point at `beam init`: {}",
        outcome.stderr
    );
}

#[test]
fn peers_guides_the_user_when_empty() {
    let (_tmp, dir) = initialised();
    let outcome = run(&dir, "", &["peers"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);
    assert!(
        outcome.stdout.contains("No paired peers"),
        "{}",
        outcome.stdout
    );
}

#[test]
fn peers_lists_names_and_fingerprints() {
    let (_tmp, dir) = initialised();
    seed_peers(&dir);

    let outcome = run(&dir, "", &["peers"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);
    for expected in ["NAME", "FINGERPRINT", "alice", "bob", "SHA256:"] {
        assert!(
            outcome.stdout.contains(expected),
            "table is missing {expected:?}:\n{}",
            outcome.stdout
        );
    }

    let json: Value =
        serde_json::from_str(&run(&dir, "", &["peers", "--json"]).stdout).expect("valid JSON");
    let list = json.as_array().expect("an array");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["name"], "alice", "file order was not preserved");
    assert_eq!(list[1]["name"], "bob");
    assert!(list[0]["added"].is_string(), "added timestamp is missing");
}

#[test]
fn peers_reports_a_corrupt_file() {
    let (_tmp, dir) = initialised();
    fs::write(
        dir.join(KNOWN_PEERS_NAME),
        format!("{HEADER}alice ed25519 not-base64!\n"),
    )
    .expect("write known_peers");

    let outcome = run(&dir, "", &["peers"]);
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(
        outcome.stderr.contains("line 3"),
        "error does not name the bad line: {}",
        outcome.stderr
    );
}

#[test]
fn rename_changes_only_the_nickname() {
    let (_tmp, dir) = initialised();
    seed_peers(&dir);

    assert_eq!(run(&dir, "", &["rename", "alice", "ali"]).code, EXIT_OK);
    let listing = run(&dir, "", &["peers"]).stdout;
    assert!(
        listing.contains("ali") && !listing.contains("alice"),
        "{listing}"
    );

    assert_eq!(run(&dir, "", &["rename", "nobody", "x"]).code, EXIT_ERROR);
    assert_eq!(run(&dir, "", &["rename", "ali", "bob"]).code, EXIT_ERROR);
    assert_eq!(run(&dir, "", &["rename", "ali"]).code, EXIT_ERROR);
}

#[test]
fn remove_needs_confirmation() {
    let (_tmp, dir) = initialised();
    seed_peers(&dir);

    // Declining the prompt keeps the peer.
    let declined = run(&dir, "n\n", &["remove", "alice"]);
    assert_eq!(declined.code, EXIT_OK, "{}", declined.stderr);
    assert!(declined.stdout.contains("Cancelled"), "{}", declined.stdout);
    assert!(run(&dir, "", &["peers"]).stdout.contains("alice"));

    // End of input is also a no.
    assert_eq!(run(&dir, "", &["remove", "alice"]).code, EXIT_OK);
    assert!(
        run(&dir, "", &["peers"]).stdout.contains("alice"),
        "peer was removed when the prompt got no answer"
    );

    // Confirming removes exactly that peer.
    let confirmed = run(&dir, "y\n", &["remove", "alice"]);
    assert_eq!(confirmed.code, EXIT_OK, "{}", confirmed.stderr);
    let listing = run(&dir, "", &["peers"]).stdout;
    assert!(!listing.contains("alice"), "{listing}");
    assert!(
        listing.contains("bob"),
        "removed the wrong peer:\n{listing}"
    );

    assert_eq!(run(&dir, "", &["remove", "bob", "--yes"]).code, EXIT_OK);
    assert_eq!(
        run(&dir, "", &["remove", "nobody", "--yes"]).code,
        EXIT_ERROR
    );
}

#[test]
fn the_remove_prompt_shows_the_fingerprint() {
    let (_tmp, dir) = initialised();
    seed_peers(&dir);
    let outcome = run(&dir, "n\n", &["remove", "alice"]);
    assert!(
        outcome.stdout.contains("SHA256:"),
        "the prompt does not show the fingerprint:\n{}",
        outcome.stdout
    );
}

/// M5 dropped `newcode`: `listen` renews its own code (ADR-0028). The
/// command is gone rather than kept as a stub that does nothing.
#[test]
fn newcode_no_longer_exists() {
    let (_tmp, dir) = initialised();
    let outcome = run(&dir, "", &["newcode"]);
    assert_eq!(outcome.code, EXIT_ERROR, "{}", outcome.stderr);
    assert!(
        outcome.stderr.contains("unrecognized subcommand"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn send_is_no_longer_a_stub() {
    // A successful `listen` blocks until a peer connects, so it cannot be
    // tested this way; `listen_without_an_identity_points_at_init` below covers
    // the listen half by checking it fails for a real reason rather than by
    // reporting itself unimplemented.
    let (_tmp, dir) = initialised();
    let outcome = run(
        &dir,
        "",
        &["send", "alice", "nothing.txt", "--addr", "127.0.0.1:1"],
    );
    assert_ne!(
        outcome.code, EXIT_NOT_IMPLEMENTED,
        "send still reports itself as unimplemented"
    );
}

/// An invite for `key` that points at a loopback port nobody is listening on.
fn invite_for(key: ed25519_dalek::VerifyingKey) -> String {
    let port = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    beam::invite::Invite {
        key,
        relay: None,
        addrs: vec![std::net::SocketAddr::from(([127, 0, 0, 1], port))],
    }
    .to_string()
}

/// ADR-0031 and ADR-0036: a paired device that does not answer at its saved
/// address — not listening, moved, or re-initialised, which all look the same
/// from here — gets a message that says all three, and says how to re-pair.
#[test]
fn send_to_a_peer_that_is_not_listening_says_how_to_re_pair() {
    let (tmp, dir) = initialised();
    seed_peers(&dir);
    std::fs::write(dir.join("config.toml"), "relay = \"none\"\n").unwrap();
    // alice "listens" on a port where nothing is.
    let store = Store::new(&dir);
    let mut known = store.load_known_peers().unwrap();
    let key = known.lookup("alice").unwrap().public_key;
    let invite: beam::invite::Invite = invite_for(key).parse().unwrap();
    beam::invite::remember(
        known.lookup_key_mut(&key).unwrap(),
        &invite,
        &beam::config::Relay::Disabled,
    );
    store.save_known_peers(&known).unwrap();
    let file = tmp.path().join("note.txt");
    std::fs::write(&file, b"hello").unwrap();

    let outcome = run(
        &dir,
        "",
        &["send", "alice", file.to_str().unwrap(), "--loopback"],
    );
    assert_eq!(outcome.code, EXIT_ERROR);
    for needle in [
        "alice",
        "not reachable",
        "beam listen",
        "beam init",
        "re-pair",
        "beam remove alice",
        "beam pair <alice's invite> --name alice",
    ] {
        assert!(
            outcome.stderr.contains(needle),
            "no {needle:?} in: {}",
            outcome.stderr
        );
    }

    // The attempt is in the history, as a failure to reach alice (ADR-0043).
    let history = beam::history::read(&store);
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!(history[0].direction, beam::history::Direction::Sent);
    assert_eq!(history[0].outcome, beam::history::Outcome::Failed);
    assert_eq!(history[0].file, "note.txt");
    assert_eq!(
        history[0].fingerprint,
        Store::new(&dir)
            .load_known_peers()
            .unwrap()
            .lookup("alice")
            .unwrap()
            .fingerprint()
            .hex()
    );
    let shown = run(&dir, "", &["history"]);
    assert!(
        shown.stdout.contains("sent to alice") && shown.stdout.contains("failed"),
        "{}",
        shown.stdout
    );
    let cleared = run(&dir, "", &["history", "--clear", "--yes"]);
    assert_eq!(cleared.code, EXIT_OK, "{}", cleared.stderr);
    assert!(beam::history::read(&store).is_empty());
}

/// With the relay off and no address saved there is nowhere to look; beam says
/// so, and says how to fix it, instead of waiting for a timeout.
#[test]
fn send_with_no_relay_and_no_saved_address_says_where_to_get_one() {
    let (tmp, dir) = initialised();
    seed_peers(&dir);
    std::fs::write(dir.join("config.toml"), "relay = \"none\"\n").unwrap();
    let file = tmp.path().join("note.txt");
    std::fs::write(&file, b"hello").unwrap();
    let outcome = run(
        &dir,
        "",
        &["send", "alice", file.to_str().unwrap(), "--loopback"],
    );
    assert_eq!(outcome.code, EXIT_ERROR);
    for needle in [
        "does not know where to find alice",
        "beam pair <alice's invite>",
    ] {
        assert!(
            outcome.stderr.contains(needle),
            "no {needle:?} in: {}",
            outcome.stderr
        );
    }
}

#[test]
fn send_to_an_unpaired_peer_fails_before_connecting() {
    let (_tmp, dir) = initialised();
    // Port 1 is not listening; reaching the network at all would hang or error
    // differently, so this also shows the peer check comes first.
    let outcome = run(
        &dir,
        "",
        &["send", "nobody", "x.txt", "--addr", "127.0.0.1:1"],
    );
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(
        outcome.stderr.contains("peer not found"),
        "unexpected error: {}",
        outcome.stderr
    );
}

#[test]
fn listen_without_an_identity_points_at_init() {
    let (_tmp, dir) = beam_dir();
    let outcome = run(&dir, "", &["listen"]);
    assert_eq!(
        outcome.code, EXIT_ERROR,
        "listen exited {} — a stub would exit {EXIT_NOT_IMPLEMENTED}",
        outcome.code
    );
    assert!(
        outcome.stderr.contains("beam init"),
        "unexpected error: {}",
        outcome.stderr
    );
}

/// S-1, the CLI half: no flag on `beam listen` can stand in for a person
/// answering the prompt.
///
/// This is an allowlist rather than a denylist on purpose. A denylist only
/// catches the names somebody thought of in advance; an allowlist means any new
/// flag on `listen` fails this test until somebody has looked at it and decided
/// it is not an accept bypass.
#[test]
fn listen_has_no_flag_that_could_stand_in_for_the_prompt() {
    let command = beam::cli::command();
    let listen = command
        .find_subcommand("listen")
        .expect("listen is a subcommand");

    let mut flags: Vec<&str> = listen
        .get_arguments()
        .filter_map(|arg| arg.get_long())
        .collect();
    flags.sort_unstable();

    // `beam-dir`, `json` and `help` are added by clap from the root command,
    // so what is listed here is exactly what `listen` declares for itself.
    // `addr` and `loopback` are hidden development flags that choose the
    // transport and the address; neither can answer anything.
    assert_eq!(
        flags,
        ["addr", "loopback", "out"],
        "the flags on `beam listen` changed; check that the new one cannot \
         accept a transfer without a person answering the prompt (S-1)"
    );
}

/// The pairing confirmation has the same no-bypass rule as Accept (condition
/// 4 of the M4 approval): no flag can answer `[y/N]` in advance. Allowlisted
/// for the same reason as `listen` above.
#[test]
fn pair_has_no_flag_that_could_stand_in_for_the_confirmation() {
    let command = beam::cli::command();
    let pair = command
        .find_subcommand("pair")
        .expect("pair is a subcommand");

    let mut flags: Vec<&str> = pair
        .get_arguments()
        .filter_map(|arg| arg.get_long())
        .collect();
    flags.sort_unstable();

    assert_eq!(
        flags,
        ["loopback", "name", "wait"],
        "the flags on `beam pair` changed; check that the new one cannot save a \
         peer without a person confirming its fingerprint"
    );
}

#[test]
fn pair_needs_a_name() {
    let (_tmp, dir) = initialised();
    let outcome = run(&dir, "", &["pair", "123456789"]);
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(outcome.stderr.contains("--name"), "{}", outcome.stderr);
}

#[test]
fn pair_needs_an_invite_or_wait_but_not_both() {
    let (_tmp, dir) = initialised();
    let neither = run(&dir, "", &["pair", "--name", "alice"]);
    assert_eq!(neither.code, EXIT_ERROR, "{}", neither.stderr);

    let both = run(
        &dir,
        "",
        &["pair", "123456789", "--wait", "--name", "alice"],
    );
    assert_eq!(both.code, EXIT_ERROR, "{}", both.stderr);
    assert!(
        both.stderr.contains("cannot be used with"),
        "{}",
        both.stderr
    );
}

#[test]
fn pair_with_something_that_is_not_an_invite_fails_before_the_network() {
    let (_tmp, dir) = initialised();
    // A 9-digit Short ID, as the rendezvous version of beam used, is not an
    // invite; nor is anything else that does not start with beam1.
    for text in ["123456789", "beam1notreallyaninvite"] {
        let outcome = run(&dir, "", &["pair", text, "--name", "alice"]);
        assert_eq!(outcome.code, EXIT_ERROR);
        assert!(
            outcome.stderr.contains("invite"),
            "{text}: {}",
            outcome.stderr
        );
        assert!(
            !outcome.stdout.contains("Pairing code"),
            "{text}: asked for a code: {}",
            outcome.stdout
        );
    }
}

#[test]
fn pair_with_a_name_that_is_taken_fails_before_the_network() {
    let (_tmp, dir) = initialised();
    seed_peers(&dir);
    let stranger = Identity::generate("carol").unwrap().verifying_key();
    let outcome = run(
        &dir,
        "",
        &["pair", &invite_for(stranger), "--name", "alice"],
    );
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(
        outcome.stderr.contains("already exists"),
        "{}",
        outcome.stderr
    );
}

/// ADR-0038 (F-1): an invite that would move a paired device to a different
/// relay is not applied without a yes — a peer's key is public, so anyone can
/// make such an invite, and a relay sees when you send and can block it.
#[test]
fn an_invite_that_changes_a_peers_relay_needs_a_yes() {
    let (_tmp, dir) = initialised();
    seed_peers(&dir);
    let store = Store::new(&dir);
    let key = store
        .load_known_peers()
        .unwrap()
        .lookup("alice")
        .unwrap()
        .public_key;
    let invite = beam::invite::Invite {
        key,
        relay: Some("https://relay.example.org/".parse().unwrap()),
        addrs: vec!["203.0.113.7:7820".parse().unwrap()],
    }
    .to_string();

    // Refused: nothing at all is saved, not even the addresses.
    let no = run(&dir, "n\n", &["pair", &invite, "--name", "alice"]);
    assert_eq!(no.code, EXIT_OK, "{}", no.stderr);
    for needle in [
        "different relay",
        "https://relay.example.org/",
        "can block it",
        "Nothing was changed",
    ] {
        assert!(
            no.stdout.contains(needle),
            "no {needle:?} in: {}",
            no.stdout
        );
    }
    let alice = store.load_known_peers().unwrap();
    let alice = alice.lookup("alice").unwrap();
    assert_eq!(alice.attr("relay"), None);
    assert_eq!(alice.attr("addrs"), None);

    // Accepted: the relay and the addresses are saved; the key never changes.
    let yes = run(&dir, "y\n", &["pair", &invite, "--name", "alice"]);
    assert_eq!(yes.code, EXIT_OK, "{}", yes.stderr);
    let known = store.load_known_peers().unwrap();
    let alice = known.lookup("alice").unwrap();
    assert_eq!(alice.attr("relay"), Some("https://relay.example.org/"));
    assert_eq!(alice.attr("addrs"), Some("203.0.113.7:7820"));
    assert_eq!(alice.public_key, key);

    // The same invite again changes nothing about the relay, so it is not asked.
    let again = run(&dir, "", &["pair", &invite, "--name", "alice"]);
    assert_eq!(again.code, EXIT_OK, "{}", again.stderr);
    assert!(
        !again.stdout.contains("different relay"),
        "{}",
        again.stdout
    );
}

/// A config for testing direct connections without a relay is accepted, and
/// a broken `advertise` entry names the file.
#[test]
fn a_no_relay_config_with_an_advertised_address_is_accepted() {
    let (_tmp, dir) = initialised();
    std::fs::write(
        dir.join("config.toml"),
        "relay = \"none\"\nport = 7820\nadvertise = [\"203.0.113.7:7820\"]\n",
    )
    .unwrap();
    let config = beam::config::Config::load(&dir.join("config.toml")).unwrap();
    assert_eq!(config.relay, beam::config::Relay::Disabled);
    assert_eq!(config.advertise.len(), 1);

    std::fs::write(dir.join("config.toml"), "advertise = [\"my-pc:7820\"]\n").unwrap();
    let outcome = run(&dir, "", &["pair", "--wait", "--name", "alice"]);
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(outcome.stderr.contains("config.toml"), "{}", outcome.stderr);
    assert!(outcome.stderr.contains("advertise"), "{}", outcome.stderr);
}

/// ADR-0036: the invite of a device that is already paired updates where to
/// find it — without a code, without the network, and without touching its
/// key or its name.
#[test]
fn pairing_again_with_a_known_devices_invite_only_updates_its_address() {
    let (_tmp, dir) = initialised();
    seed_peers(&dir);
    let store = Store::new(&dir);
    let key = store
        .load_known_peers()
        .unwrap()
        .lookup("alice")
        .unwrap()
        .public_key;
    let invite = invite_for(key);

    let outcome = run(&dir, "", &["pair", &invite, "--name", "ali"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);
    assert!(
        outcome.stdout.contains("alice is already paired"),
        "{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("beam rename alice ali"),
        "{}",
        outcome.stdout
    );
    assert!(
        !outcome.stdout.contains("Pairing code"),
        "{}",
        outcome.stdout
    );

    let known = store.load_known_peers().unwrap();
    let alice = known.lookup("alice").expect("still called alice");
    assert_eq!(alice.public_key, key, "the key is unchanged");
    assert!(
        alice
            .attr("addrs")
            .is_some_and(|a| a.starts_with("127.0.0.1:")),
        "{:?}",
        alice.attr("addrs")
    );
    assert_eq!(known.len(), 2, "no peer was added");
}

#[test]
fn pair_without_an_identity_points_at_init() {
    let (_tmp, dir) = beam_dir();
    let outcome = run(&dir, "", &["pair", "--wait", "--name", "alice"]);
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(outcome.stderr.contains("beam init"), "{}", outcome.stderr);
}

#[test]
fn pair_with_a_broken_config_says_which_file() {
    let (_tmp, dir) = initialised();
    std::fs::write(dir.join("config.toml"), "realy = \"none\"\n").unwrap();
    let outcome = run(&dir, "", &["pair", "--wait", "--name", "alice"]);
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(outcome.stderr.contains("config.toml"), "{}", outcome.stderr);
}

/// S-1 again, from the other direction: nothing anywhere in the CLI is named
/// like an auto-accept switch.
#[test]
fn no_command_offers_an_auto_accept_switch() {
    const FORBIDDEN: [&str; 8] = [
        "accept",
        "auto",
        "auto-accept",
        "trust",
        "trusted",
        "no-confirm",
        "unattended",
        "batch",
    ];

    let command = beam::cli::command();
    for sub in command.get_subcommands() {
        for arg in sub.get_arguments() {
            let long = arg.get_long().unwrap_or_default();
            assert!(
                !FORBIDDEN.contains(&long),
                "`beam {} --{long}` looks like a way to skip the Accept prompt",
                sub.get_name()
            );
        }
    }
}

#[test]
fn an_unknown_command_is_an_error() {
    let (_tmp, dir) = beam_dir();
    assert_eq!(
        run(&dir, "", &["definitely-not-a-command"]).code,
        EXIT_ERROR
    );
}

#[test]
fn version_prints() {
    let (_tmp, dir) = beam_dir();
    let outcome = run(&dir, "", &["version"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);
    assert!(outcome.stdout.starts_with("beam "), "{}", outcome.stdout);
}

#[test]
fn help_lists_every_planned_command() {
    let (_tmp, dir) = beam_dir();
    let outcome = run(&dir, "", &["--help"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);
    for name in [
        "init",
        "whoami",
        "peers",
        "pair",
        "rename",
        "remove",
        "listen",
        "send",
        "transfers",
    ] {
        assert!(
            outcome.stdout.contains(name),
            "help does not mention {name:?}:\n{}",
            outcome.stdout
        );
    }
}

/// Writes a fake partial transfer, the way an interrupted session would leave
/// one, so the listing and clearing commands can be tested without a peer.
fn seed_partial(dir: &Path, id: &str, file_name: &str, size: u64, have_chunks: u32) {
    let work = dir.join("tmp").join(id);
    std::fs::create_dir_all(&work).expect("create partial dir");

    let chunk_size = 1024u32;
    let chunk_count = size.div_ceil(u64::from(chunk_size)) as u32;
    let mut bits = vec![0u8; chunk_count.div_ceil(8) as usize];
    for i in 0..have_chunks.min(chunk_count) {
        bits[(i / 8) as usize] |= 1 << (i % 8);
    }
    use base64::Engine as _;
    let have = base64::engine::general_purpose::STANDARD.encode(&bits);

    let state = serde_json::json!({
        "version": 1,
        "peer_fingerprint": "SHA256:0000000000000000000000000000000000000000000000000000000000000000",
        "file_name": file_name,
        "file_sha256": "0".repeat(64),
        "size": size,
        "chunk_size": chunk_size,
        "chunk_count": chunk_count,
        "created": "2026-09-01T00:00:00Z",
        "updated": "2026-09-01T00:00:00Z",
        "have": have,
    });
    std::fs::write(
        work.join("state.json"),
        serde_json::to_string_pretty(&state).expect("serialise"),
    )
    .expect("write state");
    std::fs::write(work.join("part"), vec![0u8; size as usize]).expect("write part");
}

#[test]
fn transfers_says_so_when_there_are_none() {
    let (_tmp, dir) = initialised();
    let outcome = run(&dir, "", &["transfers"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);
    assert!(
        outcome.stdout.contains("No partially received transfers"),
        "{}",
        outcome.stdout
    );
}

#[test]
fn transfers_lists_what_is_partly_received() {
    let (_tmp, dir) = initialised();
    seed_partial(&dir, "aaaabbbbccccdddd", "project.zip", 10_240, 5);

    let outcome = run(&dir, "", &["transfers"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);
    for expected in ["ID", "FILE", "project.zip", "50%"] {
        assert!(
            outcome.stdout.contains(expected),
            "listing is missing {expected:?}:\n{}",
            outcome.stdout
        );
    }

    let json: Value =
        serde_json::from_str(&run(&dir, "", &["transfers", "--json"]).stdout).expect("valid JSON");
    let list = json.as_array().expect("an array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["file_name"], "project.zip");
    assert_eq!(list[0]["percent"], 50);
    assert_eq!(
        list[0]["expired"], true,
        "a partial from 2026-09-01 is stale"
    );
}

#[test]
fn clearing_transfers_asks_first() {
    let (_tmp, dir) = initialised();
    seed_partial(&dir, "aaaabbbbccccdddd", "project.zip", 10_240, 5);

    // Declining keeps it, exactly like `beam remove`.
    let declined = run(&dir, "n\n", &["transfers", "--clear"]);
    assert_eq!(declined.code, EXIT_OK, "{}", declined.stderr);
    assert!(declined.stdout.contains("Cancelled"), "{}", declined.stdout);
    assert!(
        run(&dir, "", &["transfers"]).stdout.contains("project.zip"),
        "a declined clear deleted the partial anyway"
    );

    // The prompt says what is about to be lost.
    assert!(
        declined.stdout.contains("project.zip") && declined.stdout.contains("of"),
        "the confirmation did not say what would be discarded:\n{}",
        declined.stdout
    );

    let confirmed = run(&dir, "y\n", &["transfers", "--clear"]);
    assert_eq!(confirmed.code, EXIT_OK, "{}", confirmed.stderr);
    assert!(
        run(&dir, "", &["transfers"])
            .stdout
            .contains("No partially received transfers"),
        "the partial survived a confirmed clear"
    );
}

#[test]
fn clearing_one_transfer_leaves_the_others() {
    let (_tmp, dir) = initialised();
    seed_partial(&dir, "aaaabbbbccccdddd", "keep-me.zip", 4_096, 2);
    seed_partial(&dir, "11112222333344ff", "delete-me.zip", 4_096, 2);

    // The id may be given in the short form the listing prints.
    let outcome = run(&dir, "", &["transfers", "--clear", "11112222", "--yes"]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);

    let listing = run(&dir, "", &["transfers"]).stdout;
    assert!(listing.contains("keep-me.zip"), "{listing}");
    assert!(!listing.contains("delete-me.zip"), "{listing}");
}

#[test]
fn clearing_an_unknown_transfer_is_an_error() {
    let (_tmp, dir) = initialised();
    seed_partial(&dir, "aaaabbbbccccdddd", "project.zip", 4_096, 2);

    let outcome = run(
        &dir,
        "",
        &["transfers", "--clear", "nothing-like-this", "--yes"],
    );
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(
        outcome.stderr.contains("no partial transfer starts with"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn an_ambiguous_transfer_id_is_refused_rather_than_guessed() {
    let (_tmp, dir) = initialised();
    seed_partial(&dir, "aaaa1111", "one.zip", 4_096, 2);
    seed_partial(&dir, "aaaa2222", "two.zip", 4_096, 2);

    let outcome = run(&dir, "", &["transfers", "--clear", "aaaa", "--yes"]);
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(outcome.stderr.contains("matches 2"), "{}", outcome.stderr);
    assert!(
        run(&dir, "", &["transfers"]).stdout.contains("one.zip"),
        "an ambiguous id deleted something anyway"
    );
}

#[test]
fn beam_ui_switches_what_plain_beam_opens_and_keeps_the_rest_of_the_config() {
    let (_tmp, dir) = beam_dir();
    fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    fs::write(&config, "# mine\nport = 0\n").unwrap();

    let shown = run(&dir, "", &["ui"]);
    assert_eq!(shown.code, EXIT_OK, "{}", shown.stderr);
    assert!(
        shown.stdout.contains("opens the full-screen view"),
        "{}",
        shown.stdout
    );

    let cli = run(&dir, "", &["ui", "cli"]);
    assert_eq!(cli.code, EXIT_OK, "{}", cli.stderr);
    assert!(cli.stdout.contains("now prints the help"), "{}", cli.stdout);
    let text = fs::read_to_string(&config).unwrap();
    assert_eq!(text, "# mine\nport = 0\nui = \"cli\"\n");

    let json = run(&dir, "", &["--json", "ui"]);
    let value: Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(value["ui"], "cli");

    let tui = run(&dir, "", &["ui", "tui"]);
    assert_eq!(tui.code, EXIT_OK, "{}", tui.stderr);
    assert_eq!(fs::read_to_string(&config).unwrap(), "# mine\nport = 0\n");

    let bad = run(&dir, "", &["ui", "gui"]);
    assert_eq!(bad.code, EXIT_ERROR);
}

#[test]
fn arguments_never_open_the_full_screen_view() {
    // `--beam-dir` alone is an argument: the help, not the view.
    let (_tmp, dir) = beam_dir();
    let outcome = run(&dir, "", &[]);
    assert_eq!(outcome.code, EXIT_OK, "{}", outcome.stderr);
    assert!(outcome.stdout.contains("Usage"), "{}", outcome.stdout);
    assert!(outcome.stdout.contains("beam ui cli"), "{}", outcome.stdout);
}
