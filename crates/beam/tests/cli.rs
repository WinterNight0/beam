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

#[test]
fn commands_that_wait_for_later_milestones_exit_two() {
    let (_tmp, dir) = initialised();
    let cases: [&[&str]; 2] = [&["pair", "123456789", "--name", "alice"], &["newcode"]];
    for args in cases {
        let outcome = run(&dir, "", args);
        assert_eq!(
            outcome.code, EXIT_NOT_IMPLEMENTED,
            "{args:?} exited {} ({})",
            outcome.code, outcome.stderr
        );
        assert!(
            outcome.stderr.contains("not implemented yet"),
            "{args:?}: {}",
            outcome.stderr
        );
    }
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

#[test]
fn send_without_an_address_explains_that_discovery_is_not_here_yet() {
    let (_tmp, dir) = initialised();
    seed_peers(&dir);
    let outcome = run(&dir, "", &["send", "alice", "anything.txt"]);
    assert_eq!(outcome.code, EXIT_ERROR);
    assert!(
        outcome.stderr.contains("--addr") && outcome.stderr.contains("M4"),
        "unhelpful error: {}",
        outcome.stderr
    );
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
    assert_eq!(
        flags,
        ["addr", "out"],
        "the flags on `beam listen` changed; check that the new one cannot \
         accept a transfer without a person answering the prompt (S-1)"
    );
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
        "init", "whoami", "peers", "pair", "rename", "remove", "listen", "send", "newcode",
    ] {
        assert!(
            outcome.stdout.contains(name),
            "help does not mention {name:?}:\n{}",
            outcome.stdout
        );
    }
}
