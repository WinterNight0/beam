//! End-to-end tests that run two real `beam` processes.
//!
//! Everything else in the suite calls the library directly, which is fast but
//! blind to anything that only goes wrong in a whole program. The bug that
//! prompted these tests is the example: `main.rs` held a `StdoutLock` for the
//! duration of the run, so the thread that draws the Accept prompt blocked
//! forever the first time it tried to write. Every in-process test passed.
//!
//! These tests therefore wait for the prompt to actually appear on the child's
//! stdout before answering it. A regression of that kind shows up as a timeout
//! here rather than as a mystery in manual testing.
//!
//! No pseudo-terminal is involved: the prompt is written to stdout whether or
//! not stdout is a terminal, and the deadlock was about the lock rather than
//! the tty. Pipes are enough, which is why these run on Windows CI as well as
//! Linux.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// The binary under test, built by cargo for this integration test.
const BEAM: &str = env!("CARGO_BIN_EXE_beam");

/// Long enough to absorb a slow CI runner, short enough that a hang is still a
/// test failure rather than a stuck job.
const PATIENCE: Duration = Duration::from_secs(30);

/// Runs a beam command to completion and returns its stdout.
fn beam(beam_dir: &Path, args: &[&str]) -> String {
    let output = Command::new(BEAM)
        .args(args)
        .env("BEAM_DIR", beam_dir)
        .output()
        .expect("run beam");
    assert!(
        output.status.success(),
        "beam {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is utf-8")
}

/// A child process whose output can be waited on for a particular string.
///
/// Output is pumped byte by byte, because the Accept prompt ends in `[y/N]: `
/// with no newline — as a prompt should — and a line-buffered reader would not
/// see it until something else produced a newline.
struct Watched {
    child: Child,
    stdin: std::process::ChildStdin,
    bytes: Receiver<u8>,
    seen: String,
}

impl Watched {
    fn spawn(beam_dir: &Path, args: &[&str]) -> Self {
        let mut child = Command::new(BEAM)
            .args(args)
            .env("BEAM_DIR", beam_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn beam");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        // Both streams feed one channel: beam reports a finished transfer on
        // stdout and a refused one on stderr, and a test that waits for either
        // should not have to care which.
        let (tx, bytes) = channel();
        for mut stream in [
            Box::new(stdout) as Box<dyn Read + Send>,
            Box::new(stderr) as Box<dyn Read + Send>,
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut byte = [0u8; 1];
                while let Ok(1) = stream.read(&mut byte) {
                    if tx.send(byte[0]).is_err() {
                        return;
                    }
                }
            });
        }

        Self {
            child,
            stdin,
            bytes,
            seen: String::new(),
        }
    }

    /// Reads until `needle` has appeared, or gives up.
    fn wait_for(&mut self, needle: &str) {
        let deadline = Instant::now() + PATIENCE;
        while !self.seen.contains(needle) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.bytes.recv_timeout(left) {
                Ok(byte) => self.seen.push(byte as char),
                Err(RecvTimeoutError::Timeout) => panic!(
                    "waited {PATIENCE:?} for {needle:?} and never saw it.\n\
                     What the process did print:\n{}",
                    self.seen
                ),
                Err(RecvTimeoutError::Disconnected) => panic!(
                    "the process ended before printing {needle:?}.\n\
                     What it did print:\n{}",
                    self.seen
                ),
            }
        }
    }

    /// Waits for the process to exit by itself.
    fn exit_status(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll the child") {
                // Collect whatever it printed last, for the assertions.
                std::thread::sleep(Duration::from_millis(50));
                while let Ok(byte) = self.bytes.try_recv() {
                    self.seen.push(byte as char);
                }
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the process did not exit within {PATIENCE:?}. It printed:\n{}",
                self.seen
            );
            while let Ok(byte) = self.bytes.try_recv() {
                self.seen.push(byte as char);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The value printed after `label` in a `  Label   value` detail line.
    fn field(&self, label: &str) -> String {
        let line = self
            .seen
            .lines()
            .find(|line| line.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("no {label:?} line in:\n{}", self.seen));
        line.trim_start()
            .trim_start_matches(label)
            .trim()
            .to_string()
    }

    fn answer(&mut self, text: &str) {
        writeln!(self.stdin, "{text}").expect("write to the child's stdin");
        self.stdin.flush().expect("flush");
    }

    /// The address `beam listen` reports, once it has reported one.
    fn listening_on(&mut self) -> String {
        self.wait_for("Waiting for transfers");
        let line = self
            .seen
            .lines()
            .find(|line| line.trim_start().starts_with("Listening"))
            .unwrap_or_else(|| panic!("no Listening line in:\n{}", self.seen));
        line.trim_start()
            .trim_start_matches("Listening")
            .trim()
            .to_string()
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Two beam homes that have paired with each other, plus somewhere to work.
struct Demo {
    _tmp: tempfile::TempDir,
    alice: PathBuf,
    bob: PathBuf,
    inbox: PathBuf,
    payload: PathBuf,
}

fn public_key_of(beam_dir: &Path) -> String {
    let json: serde_json::Value =
        serde_json::from_str(&beam(beam_dir, &["whoami", "--json"])).expect("whoami json");
    json["public_key"].as_str().expect("public_key").to_string()
}

fn demo(payload_len: usize) -> Demo {
    let tmp = tempfile::tempdir().expect("tempdir");
    let alice = tmp.path().join("alice");
    let bob = tmp.path().join("bob");
    let inbox = tmp.path().join("inbox");
    std::fs::create_dir_all(&inbox).expect("create inbox");

    beam(&alice, &["init"]);
    beam(&bob, &["init"]);

    // Pair them by writing known_peers directly, which keeps the transfer
    // tests independent of the network. `beam_pair_between_two_processes`
    // below pairs through the real command.
    let entry = |name: &str, key: &str| {
        format!("# beam known_peers v1\n{name}  ed25519 {key}  added=2026-01-01T00:00:00Z\n")
    };
    std::fs::write(
        bob.join("known_peers"),
        entry("alice", &public_key_of(&alice)),
    )
    .expect("write bob's known_peers");
    std::fs::write(
        alice.join("known_peers"),
        entry("bob", &public_key_of(&bob)),
    )
    .expect("write alice's known_peers");

    let payload = tmp.path().join("payload.bin");
    let bytes: Vec<u8> = (0..payload_len)
        .map(|i| ((i * 37 + 11) % 251) as u8)
        .collect();
    std::fs::write(&payload, &bytes).expect("write payload");

    Demo {
        _tmp: tmp,
        alice,
        bob,
        inbox,
        payload,
    }
}

#[test]
fn two_processes_complete_a_transfer() {
    let demo = demo(600_000);

    // Port 0 lets the operating system choose, and `listen` prints what it got.
    let mut listener = Watched::spawn(
        &demo.bob,
        &[
            "listen",
            "--addr",
            "127.0.0.1:0",
            "--out",
            demo.inbox.to_str().expect("utf-8 path"),
        ],
    );
    let addr = listener.listening_on();

    let sender = Command::new(BEAM)
        .args([
            "send",
            "bob",
            demo.payload.to_str().expect("utf-8 path"),
            "--addr",
            &addr,
        ])
        .env("BEAM_DIR", &demo.alice)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn send");

    // The assertion that catches a deadlocked prompt: nothing is answered until
    // the question has actually reached us.
    listener.wait_for("Incoming file");
    listener.wait_for("[y/N]: ");
    assert!(
        listener.seen.contains("From") && listener.seen.contains("SHA256:"),
        "the prompt did not name the sender:\n{}",
        listener.seen
    );

    listener.answer("y");
    listener.wait_for("saved as payload.bin");

    let sent = sender.wait_with_output().expect("wait for send");
    assert!(
        sent.status.success(),
        "send failed: {}",
        String::from_utf8_lossy(&sent.stderr)
    );

    let source = std::fs::read(&demo.payload).expect("read source");
    let received = std::fs::read(demo.inbox.join("payload.bin")).expect("read received");
    assert_eq!(
        received, source,
        "the file that arrived is not the one sent"
    );
}

#[test]
fn answering_no_between_two_processes_saves_nothing() {
    let demo = demo(50_000);

    let mut listener = Watched::spawn(
        &demo.bob,
        &[
            "listen",
            "--addr",
            "127.0.0.1:0",
            "--out",
            demo.inbox.to_str().expect("utf-8 path"),
        ],
    );
    let addr = listener.listening_on();

    let sender = Command::new(BEAM)
        .args([
            "send",
            "bob",
            demo.payload.to_str().expect("utf-8 path"),
            "--addr",
            &addr,
        ])
        .env("BEAM_DIR", &demo.alice)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn send");

    listener.wait_for("[y/N]: ");
    listener.answer("n");
    listener.wait_for("declined");

    let sent = sender.wait_with_output().expect("wait for send");
    assert!(!sent.status.success(), "a declined send reported success");
    assert!(
        String::from_utf8_lossy(&sent.stderr).contains("declined"),
        "the sender was not told it was declined: {}",
        String::from_utf8_lossy(&sent.stderr)
    );

    assert!(
        std::fs::read_dir(&demo.inbox)
            .expect("read inbox")
            .next()
            .is_none(),
        "a declined transfer saved something"
    );
}

/// Kills a process mid-transfer and resumes, for real.
///
/// The in-process resume tests model an interruption by dropping a future.
/// These kill an operating-system process instead, which is the case the
/// crash-consistency rules in ADR-0022 actually exist for: a `SIGKILL` gives
/// nothing a chance to tidy up, so what survives is exactly what had already
/// reached the disk.
mod killed {
    use super::*;

    /// A chunk size small enough that a 2 MiB file takes a while in chunks.
    const CHUNK: &str = "65536";

    /// Runs a transfer, kills `victim` once some chunks have landed, then runs
    /// it again and checks the file arrives intact.
    fn interrupt_and_resume(victim: Victim) {
        let demo = demo(2 * 1024 * 1024);
        let work = demo.bob.join("tmp");

        // --- first attempt, cut short -------------------------------------
        {
            let mut listener = Watched::spawn(
                &demo.bob,
                &[
                    "listen",
                    "--addr",
                    "127.0.0.1:0",
                    "--out",
                    demo.inbox.to_str().expect("utf-8 path"),
                ],
            );
            let addr = listener.listening_on();

            let mut sender = Command::new(BEAM)
                .args([
                    "send",
                    "bob",
                    demo.payload.to_str().expect("utf-8 path"),
                    "--addr",
                    &addr,
                    "--chunk-size",
                    CHUNK,
                ])
                .env("BEAM_DIR", &demo.alice)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn send");

            listener.wait_for("[y/N]: ");
            listener.answer("y");

            wait_for_partial_progress(&work);

            match victim {
                Victim::Sender => {
                    sender.kill().expect("kill the sender");
                    let _ = sender.wait();
                }
                Victim::Receiver => {
                    // Dropping `Watched` kills the listener.
                    drop(listener);
                    let _ = sender.kill();
                    let _ = sender.wait();
                }
            }
        }

        let carried = partial_chunks(&work);
        assert!(
            carried > 0,
            "nothing survived the interruption, so there is nothing to resume"
        );
        assert!(
            !demo.inbox.join("payload.bin").exists(),
            "an interrupted transfer produced a file"
        );

        // --- second attempt, all the way ----------------------------------
        let mut listener = Watched::spawn(
            &demo.bob,
            &[
                "listen",
                "--addr",
                "127.0.0.1:0",
                "--out",
                demo.inbox.to_str().expect("utf-8 path"),
            ],
        );
        let addr = listener.listening_on();

        let sender = Command::new(BEAM)
            .args([
                "send",
                "bob",
                demo.payload.to_str().expect("utf-8 path"),
                "--addr",
                &addr,
                "--chunk-size",
                CHUNK,
            ])
            .env("BEAM_DIR", &demo.alice)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn send");

        // Every resume is accepted afresh (S-2), and the prompt says so.
        listener.wait_for("Incoming file (resuming)");
        listener.wait_for("Already have");
        listener.wait_for("[y/N]: ");
        listener.answer("y");
        listener.wait_for("saved as payload.bin");

        let sent = sender.wait_with_output().expect("wait for send");
        assert!(
            sent.status.success(),
            "the resumed send failed: {}",
            String::from_utf8_lossy(&sent.stderr)
        );
        let said = String::from_utf8_lossy(&sent.stdout);
        assert!(
            said.contains("already there"),
            "the sender did not report skipping anything: {said}"
        );

        assert_eq!(
            std::fs::read(demo.inbox.join("payload.bin")).expect("read result"),
            std::fs::read(&demo.payload).expect("read source"),
            "the resumed file differs from the one that was sent"
        );
    }

    enum Victim {
        Sender,
        Receiver,
    }

    #[test]
    fn killing_the_sender_mid_transfer_then_resuming() {
        interrupt_and_resume(Victim::Sender);
    }

    #[test]
    fn killing_the_receiver_mid_transfer_then_resuming() {
        interrupt_and_resume(Victim::Receiver);
    }

    /// Waits until a partial reports at least two stored chunks.
    fn wait_for_partial_progress(work: &Path) {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            if partial_chunks(work) >= 2 {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no chunks reached the disk within {PATIENCE:?}");
    }

    /// How many chunks the partials under `work` claim, read straight from the
    /// state files rather than through beam, so the test is checking the disk.
    fn partial_chunks(work: &Path) -> u32 {
        let Ok(entries) = std::fs::read_dir(work) else {
            return 0;
        };
        let mut total = 0;
        for entry in entries.flatten() {
            let state = entry.path().join("state.json");
            let Ok(text) = std::fs::read_to_string(&state) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let Some(have) = json["have"].as_str() else {
                continue;
            };
            use base64::Engine as _;
            if let Ok(bits) = base64::engine::general_purpose::STANDARD.decode(have) {
                total += bits.iter().map(|b| b.count_ones()).sum::<u32>();
            }
        }
        total
    }
}

/// `beam pair` between two real processes, through a real rendezvous server,
/// followed by a transfer between the two devices it paired.
///
/// Every prompt is waited for before it is answered, as above. That is what
/// catches a prompt that never reaches the screen, and it also proves the
/// ordering the design promises: nobody is asked `[y/N]` until the code has
/// been checked.
mod pairing {
    use super::*;

    /// A rendezvous server on a free loopback port, run on its own runtime
    /// for as long as the value lives.
    struct Server {
        url: String,
        _runtime: tokio::runtime::Runtime,
    }

    fn server() -> Server {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .expect("bind the rendezvous server");
        let addr = listener.local_addr().expect("local addr");
        runtime.spawn(beam::rendezvous::serve(
            listener,
            beam::rendezvous::ServerConfig::default(),
        ));
        Server {
            url: format!("ws://{addr}/v1"),
            _runtime: runtime,
        }
    }

    /// Points a beam home at the test server, with no relay: this test must
    /// not touch the internet.
    fn configure(beam_dir: &Path, url: &str) {
        std::fs::write(
            beam_dir.join("config.toml"),
            format!("rendezvous = \"{url}\"\nrelay = \"none\"\n"),
        )
        .expect("write config.toml");
    }

    fn two_homes(server: &Server) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let alice = tmp.path().join("alice");
        let bob = tmp.path().join("bob");
        beam(&alice, &["init"]);
        beam(&bob, &["init"]);
        configure(&alice, &server.url);
        configure(&bob, &server.url);
        (tmp, alice, bob)
    }

    /// A field from `beam whoami`.
    fn whoami_field(beam_dir: &Path, label: &str) -> String {
        let out = beam(beam_dir, &["whoami"]);
        let line = out
            .lines()
            .find(|l| l.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("no {label} in {out}"));
        line.trim_start()
            .trim_start_matches(label)
            .trim()
            .to_string()
    }

    #[test]
    fn beam_pair_between_two_processes_then_a_transfer() {
        let server = server();
        let (tmp, alice, bob) = two_homes(&server);

        // bob waits; alice joins.
        let mut waiter = Watched::spawn(&bob, &["pair", "--wait", "--name", "alice", "--loopback"]);
        waiter.wait_for("The code works for one attempt only.");
        let short_id = waiter.field("Short ID");
        let code = waiter.field("Pairing code");
        assert_eq!(short_id.replace(' ', "").len(), 9, "{short_id:?}");
        assert_eq!(code.replace(' ', "").len(), 6, "{code:?}");

        let mut joiner =
            Watched::spawn(&alice, &["pair", &short_id, "--name", "bob", "--loopback"]);
        joiner.wait_for("Pairing code shown on the other device: ");
        joiner.answer(&code);

        // Both people are asked, and both are shown both fingerprints.
        waiter.wait_for("[y/N]: ");
        joiner.wait_for("[y/N]: ");
        let alice_fp = whoami_field(&alice, "Fingerprint");
        let bob_fp = whoami_field(&bob, "Fingerprint");
        for (who, watched) in [("waiter", &waiter), ("joiner", &joiner)] {
            assert!(
                watched.seen.contains(&alice_fp) && watched.seen.contains(&bob_fp),
                "the {who}'s prompt does not show both fingerprints:\n{}",
                watched.seen
            );
        }
        waiter.answer("y");
        joiner.answer("y");
        waiter.wait_for("Paired with alice.");
        joiner.wait_for("Paired with bob.");
        assert!(waiter.exit_status().success(), "{}", waiter.seen);
        assert!(joiner.exit_status().success(), "{}", joiner.seen);

        // Each side stored the other's real key.
        let bobs_peers = std::fs::read_to_string(bob.join("known_peers")).unwrap();
        let alices_peers = std::fs::read_to_string(alice.join("known_peers")).unwrap();
        assert!(bobs_peers.contains(&public_key_of(&alice)), "{bobs_peers}");
        assert!(
            alices_peers.contains(&public_key_of(&bob)),
            "{alices_peers}"
        );

        // And the pairing is good for what it is for: a transfer.
        let inbox = tmp.path().join("inbox");
        std::fs::create_dir_all(&inbox).expect("create inbox");
        let payload = tmp.path().join("hello.txt");
        std::fs::write(&payload, b"paired, then sent").unwrap();
        let mut listener = Watched::spawn(
            &bob,
            &[
                "listen",
                "--addr",
                "127.0.0.1:0",
                "--out",
                inbox.to_str().unwrap(),
            ],
        );
        let addr = listener.listening_on();
        let sender = Command::new(BEAM)
            .args(["send", "bob", payload.to_str().unwrap(), "--addr", &addr])
            .env("BEAM_DIR", &alice)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn send");
        listener.wait_for("[y/N]: ");
        assert!(listener.seen.contains("alice"), "{}", listener.seen);
        listener.answer("y");
        listener.wait_for("saved as hello.txt");
        assert!(sender.wait_with_output().unwrap().status.success());
        assert_eq!(
            std::fs::read(inbox.join("hello.txt")).unwrap(),
            b"paired, then sent"
        );
    }

    #[test]
    fn a_wrong_code_between_two_processes_saves_nothing_on_either_side() {
        let server = server();
        let (_tmp, alice, bob) = two_homes(&server);

        let mut waiter = Watched::spawn(&bob, &["pair", "--wait", "--name", "alice", "--loopback"]);
        waiter.wait_for("The code works for one attempt only.");
        let short_id = waiter.field("Short ID");
        let code: u32 = waiter
            .field("Pairing code")
            .replace(' ', "")
            .parse()
            .unwrap();
        let wrong = format!("{:06}", (code + 1) % 1_000_000);

        let mut joiner =
            Watched::spawn(&alice, &["pair", &short_id, "--name", "bob", "--loopback"]);
        joiner.wait_for("Pairing code shown on the other device: ");
        joiner.answer(&wrong);

        joiner.wait_for("not paired");
        waiter.wait_for("not paired");
        assert!(!joiner.exit_status().success());
        assert!(!waiter.exit_status().success());
        assert!(!joiner.seen.contains("[y/N]"), "{}", joiner.seen);
        assert!(!waiter.seen.contains("[y/N]"), "{}", waiter.seen);
        assert!(waiter.seen.contains("beam pair --wait"), "{}", waiter.seen);

        for home in [&alice, &bob] {
            let peers = std::fs::read_to_string(home.join("known_peers")).unwrap_or_default();
            let entries = peers
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .count();
            assert_eq!(entries, 0, "{} gained a peer:\n{peers}", home.display());
        }
    }

    #[test]
    fn answering_no_to_pairing_saves_nothing_on_either_side() {
        let server = server();
        let (_tmp, alice, bob) = two_homes(&server);

        let mut waiter = Watched::spawn(&bob, &["pair", "--wait", "--name", "alice", "--loopback"]);
        waiter.wait_for("The code works for one attempt only.");
        let short_id = waiter.field("Short ID");
        let code = waiter.field("Pairing code");

        let mut joiner =
            Watched::spawn(&alice, &["pair", &short_id, "--name", "bob", "--loopback"]);
        joiner.wait_for("Pairing code shown on the other device: ");
        joiner.answer(&code);

        waiter.wait_for("[y/N]: ");
        joiner.wait_for("[y/N]: ");
        waiter.answer("n");
        joiner.answer("y");
        waiter.wait_for("not paired");
        joiner.wait_for("not paired");
        assert!(!waiter.exit_status().success());
        assert!(!joiner.exit_status().success());

        for home in [&alice, &bob] {
            let peers = std::fs::read_to_string(home.join("known_peers")).unwrap_or_default();
            let entries = peers
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .count();
            assert_eq!(entries, 0, "{} gained a peer:\n{peers}", home.display());
        }
    }
}
