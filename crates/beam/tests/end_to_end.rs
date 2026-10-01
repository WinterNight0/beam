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

    /// Reads until `needle` has appeared `count` times in all.
    fn wait_for_count(&mut self, needle: &str, count: usize) {
        let deadline = Instant::now() + PATIENCE;
        while self.seen.matches(needle).count() < count {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.bytes.recv_timeout(left) {
                Ok(byte) => self.seen.push(byte as char),
                Err(_) => panic!(
                    "waited for {needle:?} to appear {count} times; the process printed:\n{}",
                    self.seen
                ),
            }
        }
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
    pub(super) fn wait_for_partial_progress(work: &Path) {
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

/// `beam pair` between two real processes, with an invite and no server,
/// followed by a transfer between the two devices it paired.
///
/// Every prompt is waited for before it is answered, as above. That is what
/// catches a prompt that never reaches the screen, and it also proves the
/// ordering the design promises: nobody is asked `[y/N]` until the code has
/// been checked.
mod pairing {
    use super::*;

    /// The end of the pairing question. It asks for `yes` in full, and looks
    /// nothing like the Accept prompt's `[y/N]`.
    pub(super) const PAIR_PROMPT: &str = "Type \"yes\" to pair, anything else to refuse";

    /// A UDP port on loopback that is free right now.
    pub(super) fn free_port() -> u16 {
        std::net::UdpSocket::bind("127.0.0.1:0")
            .and_then(|socket| socket.local_addr())
            .expect("a free UDP port")
            .port()
    }

    /// Gives a beam home its own `listen` port and no relay: these tests must
    /// not touch the internet, and with a fixed port the address in a home's
    /// invite stays the same from one `listen` to the next (ADR-0036).
    pub(super) fn configure(beam_dir: &Path) -> u16 {
        let port = free_port();
        std::fs::write(
            beam_dir.join("config.toml"),
            format!("relay = \"none\"\nport = {port}\n"),
        )
        .expect("write config.toml");
        port
    }

    fn two_homes() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let alice = tmp.path().join("alice");
        let bob = tmp.path().join("bob");
        beam(&alice, &["init"]);
        beam(&bob, &["init"]);
        configure(&alice);
        configure(&bob);
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
        let (tmp, alice, bob) = two_homes();

        // bob waits; alice joins with the invite bob shows. No server.
        let mut waiter = Watched::spawn(&bob, &["pair", "--wait", "--name", "alice", "--loopback"]);
        waiter.wait_for("The code works for one attempt only.");
        let invite = waiter.field("Invite");
        let code = waiter.field("Pairing code");
        assert!(invite.starts_with("beam1"), "{invite:?}");
        assert_eq!(code.replace(' ', "").len(), 6, "{code:?}");

        let mut joiner = Watched::spawn(&alice, &["pair", &invite, "--name", "bob", "--loopback"]);
        joiner.wait_for("Pairing code shown on the other device: ");
        joiner.answer(&code);

        // Both people are asked, and both are shown both fingerprints.
        waiter.wait_for(PAIR_PROMPT);
        joiner.wait_for(PAIR_PROMPT);
        let alice_fp = whoami_field(&alice, "Fingerprint");
        let bob_fp = whoami_field(&bob, "Fingerprint");
        for (who, watched) in [("waiter", &waiter), ("joiner", &joiner)] {
            assert!(
                watched.seen.contains(&alice_fp) && watched.seen.contains(&bob_fp),
                "the {who}'s prompt does not show both fingerprints:\n{}",
                watched.seen
            );
        }
        waiter.answer("yes");
        joiner.answer("yes");
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

        // alice saved where the invite said bob is, next to bob's key.
        assert!(alices_peers.contains("addrs=127.0.0.1:"), "{alices_peers}");

        // And the pairing is good for what it is for: a transfer, over iroh,
        // to the address alice saved — still with no server anywhere.
        let inbox = tmp.path().join("inbox");
        std::fs::create_dir_all(&inbox).expect("create inbox");
        let payload = tmp.path().join("hello.txt");
        std::fs::write(&payload, b"paired, then sent").unwrap();
        let mut listener = Watched::spawn(
            &bob,
            &["listen", "--loopback", "--out", inbox.to_str().unwrap()],
        );
        listener.wait_for("Waiting for transfers");
        let sender = Command::new(BEAM)
            .args(["send", "bob", payload.to_str().unwrap(), "--loopback"])
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
        let (_tmp, alice, bob) = two_homes();

        let mut waiter = Watched::spawn(&bob, &["pair", "--wait", "--name", "alice", "--loopback"]);
        waiter.wait_for("The code works for one attempt only.");
        let invite = waiter.field("Invite");
        let code: u32 = waiter
            .field("Pairing code")
            .replace(' ', "")
            .parse()
            .unwrap();
        let wrong = format!("{:06}", (code + 1) % 1_000_000);

        let mut joiner = Watched::spawn(&alice, &["pair", &invite, "--name", "bob", "--loopback"]);
        joiner.wait_for("Pairing code shown on the other device: ");
        joiner.answer(&wrong);

        joiner.wait_for("not paired");
        waiter.wait_for("not paired");
        assert!(!joiner.exit_status().success());
        assert!(!waiter.exit_status().success());
        assert!(!joiner.seen.contains(PAIR_PROMPT), "{}", joiner.seen);
        assert!(!waiter.seen.contains(PAIR_PROMPT), "{}", waiter.seen);
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
        let (_tmp, alice, bob) = two_homes();

        let mut waiter = Watched::spawn(&bob, &["pair", "--wait", "--name", "alice", "--loopback"]);
        waiter.wait_for("The code works for one attempt only.");
        let invite = waiter.field("Invite");
        let code = waiter.field("Pairing code");

        let mut joiner = Watched::spawn(&alice, &["pair", &invite, "--name", "bob", "--loopback"]);
        joiner.wait_for("Pairing code shown on the other device: ");
        joiner.answer(&code);

        waiter.wait_for(PAIR_PROMPT);
        joiner.wait_for(PAIR_PROMPT);
        waiter.answer("n");
        joiner.answer("yes");
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

/// M5: the real transport. `listen` and `send` without `--addr`, over iroh on
/// loopback, each peer found at the address saved for it — every Accept rule
/// and resume, as real processes, with every prompt waited for before it is
/// answered.
mod over_iroh {
    use super::pairing::{PAIR_PROMPT, configure};
    use super::*;

    /// Saves, in `beam_dir`'s known_peers, that `name` listens on `port` —
    /// what `beam pair <invite>` would have saved.
    fn point_at(beam_dir: &Path, name: &str, port: u16) {
        let path = beam_dir.join("known_peers");
        let peers: String = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| {
                if line.starts_with(&format!("{name} ")) {
                    format!("{line} addrs=127.0.0.1:{port}\n")
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        std::fs::write(&path, peers).unwrap();
    }

    /// Two homes paired by hand, each with its own port, alice knowing where
    /// bob listens, plus a payload.
    fn setup(payload_len: usize) -> Demo {
        let demo = demo(payload_len);
        configure(&demo.alice);
        let bobs_port = configure(&demo.bob);
        point_at(&demo.alice, "bob", bobs_port);
        demo
    }

    fn listen(demo: &Demo) -> Watched {
        let mut listener = Watched::spawn(
            &demo.bob,
            &[
                "listen",
                "--loopback",
                "--out",
                demo.inbox.to_str().expect("utf-8 path"),
            ],
        );
        listener.wait_for("Waiting for transfers");
        listener
    }

    fn send(demo: &Demo, extra: &[&str]) -> Watched {
        let mut args = vec![
            "send",
            "bob",
            demo.payload.to_str().expect("utf-8 path"),
            "--loopback",
        ];
        args.extend_from_slice(extra);
        Watched::spawn(&demo.alice, &args)
    }

    #[test]
    fn a_transfer_over_iroh_is_accepted_by_hand_and_arrives_intact() {
        let demo = setup(600_000);
        let mut listener = listen(&demo);
        let mut sender = send(&demo, &[]);

        listener.wait_for("Incoming file");
        listener.wait_for("[y/N]: ");
        assert!(listener.seen.contains("alice"), "{}", listener.seen);
        listener.answer("y");
        listener.wait_for("saved as payload.bin");

        assert!(sender.exit_status().success(), "{}", sender.seen);
        // On loopback with the relay off, the path is direct — and the
        // progress output says so (F-11).
        assert!(sender.seen.contains("[Direct P2P]"), "{}", sender.seen);
        assert_eq!(
            std::fs::read(demo.inbox.join("payload.bin")).unwrap(),
            std::fs::read(&demo.payload).unwrap()
        );
    }

    #[test]
    fn answering_no_over_iroh_saves_nothing_and_says_so() {
        let demo = setup(10_000);
        let mut listener = listen(&demo);
        let mut sender = send(&demo, &[]);

        listener.wait_for("[y/N]: ");
        listener.answer("n");
        sender.wait_for("bob declined the transfer");
        assert!(!sender.exit_status().success());
        assert!(!demo.inbox.join("payload.bin").exists());
    }

    /// S-7 on the proved key, as processes: a device bob does not know is
    /// refused with no prompt on bob's screen, and is told why.
    #[test]
    fn an_unpaired_sender_is_refused_without_a_prompt() {
        let demo = setup(10_000);
        // bob forgets alice; alice still knows bob.
        beam(&demo.bob, &["remove", "alice", "--yes"]);
        let mut listener = listen(&demo);
        let mut sender = send(&demo, &[]);

        sender.wait_for("does not recognise this device");
        assert!(!sender.exit_status().success());
        assert!(sender.seen.contains("re-pair"), "{}", sender.seen);
        std::thread::sleep(Duration::from_millis(300));
        while let Ok(byte) = listener.bytes.try_recv() {
            listener.seen.push(byte as char);
        }
        assert!(!listener.seen.contains("[y/N]"), "{}", listener.seen);
    }

    /// Resume over iroh: kill the sender mid-transfer, send again; the second
    /// run asks again (S-2), says it is a resume, and sends only the rest.
    #[test]
    fn killing_the_sender_over_iroh_then_resuming() {
        let demo = setup(2 * 1024 * 1024);
        let work = demo.bob.join("tmp");
        let mut listener = listen(&demo);

        let mut first = send(&demo, &["--chunk-size", "65536"]);
        listener.wait_for("[y/N]: ");
        listener.answer("y");
        super::killed::wait_for_partial_progress(&work);
        let _ = first.child.kill();
        let _ = first.child.wait();

        // The receiver notices the sender is gone (QUIC idle timeout, 15 s)
        // and keeps the partial.
        listener.wait_for("transfer from alice failed");
        assert!(!demo.inbox.join("payload.bin").exists());

        let mut second = send(&demo, &["--chunk-size", "65536"]);
        listener.wait_for("Incoming file (resuming)");
        listener.wait_for("Already have");
        listener.answer("y");
        listener.wait_for("(resumed;");
        assert!(second.exit_status().success(), "{}", second.seen);
        assert!(second.seen.contains("was already there"), "{}", second.seen);
        assert_eq!(
            std::fs::read(demo.inbox.join("payload.bin")).unwrap(),
            std::fs::read(&demo.payload).unwrap()
        );
    }

    /// ADR-0030 and condition 2 of the M5 answers: a second sender while one
    /// transfer is open is told, in words, to try later.
    #[test]
    fn a_second_sender_is_told_the_receiver_is_busy() {
        let demo = setup(10_000);
        // carol, also paired with bob.
        let carol = demo._tmp.path().join("carol");
        beam(&carol, &["init"]);
        configure(&carol);
        let carol_key = public_key_of(&carol);
        let bob_key = public_key_of(&demo.bob);
        let mut bobs = std::fs::read_to_string(demo.bob.join("known_peers")).unwrap();
        bobs.push_str(&format!(
            "carol  ed25519 {carol_key}  added=2026-01-01T00:00:00Z\n"
        ));
        std::fs::write(demo.bob.join("known_peers"), bobs).unwrap();
        let bobs_port: u16 = std::fs::read_to_string(demo.bob.join("config.toml"))
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("port = "))
            .and_then(|p| p.trim().parse().ok())
            .unwrap();
        std::fs::write(
            carol.join("known_peers"),
            format!(
                "# beam known_peers v1\nbob  ed25519 {bob_key}  added=2026-01-01T00:00:00Z addrs=127.0.0.1:{bobs_port}\n"
            ),
        )
        .unwrap();

        let mut listener = listen(&demo);
        let mut first = send(&demo, &[]);
        listener.wait_for("[y/N]: ");

        let mut second = Watched::spawn(
            &carol,
            &["send", "bob", demo.payload.to_str().unwrap(), "--loopback"],
        );
        second.wait_for("bob is receiving another file; try again later");
        assert!(!second.exit_status().success());
        listener.wait_for("Turned away a file from carol");

        listener.answer("y");
        assert!(first.exit_status().success(), "{}", first.seen);
    }

    /// M6: bob re-ran `beam init`. alice's send cannot find bob's old key,
    /// and says so with an SSH-style warning and the way to re-pair. Nothing
    /// follows the new key by itself (rule 3, S-8).
    #[test]
    fn a_receiver_that_re_ran_init_gets_a_re_pair_warning_not_a_transfer() {
        let demo = setup(10_000);
        beam(&demo.bob, &["init", "--force"]);
        let mut listener = listen(&demo);
        let mut sender = send(&demo, &[]);

        sender.wait_for("beam pair <bob's new invite> --name bob");
        assert!(!sender.exit_status().success());
        for needle in [
            "not reachable",
            "beam init",
            "WARNING",
            "impersonating bob",
            "beam remove bob",
        ] {
            assert!(
                sender.seen.contains(needle),
                "no {needle:?} in:\n{}",
                sender.seen
            );
        }
        std::thread::sleep(Duration::from_millis(300));
        while let Ok(byte) = listener.bytes.try_recv() {
            listener.seen.push(byte as char);
        }
        assert!(!listener.seen.contains("[y/N]"), "{}", listener.seen);
    }

    /// M6: alice re-ran `beam init`. bob does not know her new key: refused
    /// with no prompt on bob's screen, and alice is told why, with the
    /// warning that a changed key is what an impersonator would present.
    #[test]
    fn a_sender_that_re_ran_init_is_refused_with_a_re_pair_warning() {
        let demo = setup(10_000);
        beam(&demo.alice, &["init", "--force"]);
        let mut listener = listen(&demo);
        let mut sender = send(&demo, &[]);

        sender.wait_for("here:     beam remove bob, then beam pair <bob's invite> --name bob");
        assert!(!sender.exit_status().success());
        for needle in [
            "does not recognise this device's key",
            "WARNING",
            "impersonator",
        ] {
            assert!(
                sender.seen.contains(needle),
                "no {needle:?} in:\n{}",
                sender.seen
            );
        }
        listener.wait_for("transfer from");
        assert!(!listener.seen.contains("[y/N]"), "{}", listener.seen);
    }

    /// `listen` offers pairing as well as transfers, and names the new peer
    /// from the joiner's host name. Condition 4: the pairing prompt is its
    /// own thing, and `y` does not confirm it.
    #[test]
    fn listen_pairs_but_only_with_yes_in_full() {
        let tmp = tempfile::tempdir().unwrap();
        let (alice, bob) = (tmp.path().join("alice"), tmp.path().join("bob"));
        let inbox = tmp.path().join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        beam(&alice, &["init"]);
        beam(&bob, &["init"]);
        configure(&alice);
        configure(&bob);

        let mut listener = Watched::spawn(
            &bob,
            &["listen", "--loopback", "--out", inbox.to_str().unwrap()],
        );
        listener.wait_for("Waiting for transfers");
        let invite = listener.field("Invite");

        // ADR-0037: `whoami` shows what the running `listen` offers.
        let shown = beam(&bob, &["whoami"]);
        assert!(shown.contains("beam listen is running"), "{shown}");
        assert!(shown.contains(&invite), "{shown}");
        let digits = |text: &str| -> String {
            let line = text
                .lines()
                .find(|l| l.trim_start().starts_with("Pairing code"))
                .unwrap_or_else(|| panic!("no Pairing code line in:\n{text}"));
            line.chars().filter(|c| c.is_ascii_digit()).collect()
        };
        assert_eq!(digits(&shown), digits(&listener.seen));

        // First attempt: bob answers `y`, which is not a yes to pairing.
        let code = listener.field("Pairing code");
        let mut joiner = Watched::spawn(&alice, &["pair", &invite, "--name", "bob", "--loopback"]);
        joiner.wait_for("Pairing code shown on the other device: ");
        joiner.answer(&code);
        listener.wait_for("PAIRING REQUEST - this is permanent");
        listener.wait_for(PAIR_PROMPT);
        assert!(!listener.seen.contains("[y/N]"), "{}", listener.seen);
        joiner.wait_for(PAIR_PROMPT);
        joiner.answer("yes");
        listener.answer("y");
        joiner.wait_for("not paired");
        listener.wait_for("Not paired with");
        assert!(!joiner.exit_status().success());
        let peers = std::fs::read_to_string(bob.join("known_peers")).unwrap_or_default();
        assert!(!peers.contains(&public_key_of(&alice)), "{peers}");

        // A proved code that was then refused is not a guess: a new code is
        // issued at once. `listen` does not print it (ADR-0037); it says
        // where to find it, and `whoami` shows it. With `yes`, pairing goes
        // through.
        listener.wait_for("`beam whoami` shows the new one.");
        let code = digits(&beam(&bob, &["whoami"]));
        assert_eq!(code.len(), 6, "{code:?}");
        assert_ne!(code, digits(&listener.seen), "the code did not change");
        let grouped = format!("{} {}", &code[..3], &code[3..]);
        assert!(
            !listener.seen.contains(&grouped),
            "listen printed the new code:\n{}",
            listener.seen
        );
        let mut joiner = Watched::spawn(&alice, &["pair", &invite, "--name", "bob", "--loopback"]);
        joiner.wait_for("Pairing code shown on the other device: ");
        joiner.answer(&code);
        listener.wait_for_count(PAIR_PROMPT, 2);
        joiner.wait_for(PAIR_PROMPT);
        listener.answer("yes");
        joiner.answer("yes");
        joiner.wait_for("Paired with bob.");
        listener.wait_for("Paired with");
        assert!(joiner.exit_status().success(), "{}", joiner.seen);
        let peers = std::fs::read_to_string(bob.join("known_peers")).unwrap();
        assert!(peers.contains(&public_key_of(&alice)), "{peers}");
    }
}
