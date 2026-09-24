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

    // Pair them by hand; `beam pair` arrives in M4.
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
