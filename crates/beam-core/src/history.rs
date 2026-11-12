//! `~/.beam/history.jsonl`: what came and went (ADR-0043, step 6).
//!
//! One JSON line per transfer that reached a person: sent, received,
//! declined, failed or cancelled. `beam send` writes the sender's line; the
//! listener behind `beam listen` and the background agent writes the
//! receiver's. Requests that never reach a person are not written — a
//! stranger refused without a prompt, a sender turned away as busy — so
//! nobody outside `known_peers` can fill this file.
//!
//! The file is private (it names files and peers), kept to the newest
//! [`KEEP`] entries, and rewritten atomically under a lock, since `listen`,
//! the agent and `send` may each be writing. Writing it never fails a
//! transfer: history is a record, not part of the protocol.
//!
//! Strings in it may come from a peer (a file name), so whoever shows them
//! cleans them first (`untrusted`, ADR-0034).

use std::fs::OpenOptions;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::identity::Store;

/// How many entries are kept; older ones are dropped.
pub const KEEP: usize = 1000;

/// Which way a file went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Sent,
    Received,
}

/// How a transfer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The whole file arrived and was verified.
    Done,
    /// The receiver said no, or did not answer in time.
    Declined,
    /// Someone stopped it (Ctrl+C on either side).
    Cancelled,
    /// Anything else: unreachable, a broken connection, a bad request.
    Failed,
}

/// One line of the file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds.
    pub at: u64,
    pub direction: Direction,
    /// This device's nickname for the peer at the time.
    pub peer: String,
    /// The peer's fingerprint, hex, so a renamed friend keeps their history.
    pub fingerprint: String,
    pub file: String,
    pub size: u64,
    pub outcome: Outcome,
    /// A few words more: "saved as report (1).pdf", why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Entry {
    /// An entry stamped now.
    pub fn now(
        direction: Direction,
        peer: &str,
        fingerprint: &str,
        file: &str,
        size: u64,
        outcome: Outcome,
        note: Option<String>,
    ) -> Self {
        Self {
            at: unix_now(),
            direction,
            peer: peer.to_string(),
            fingerprint: fingerprint.trim_start_matches("SHA256:").to_string(),
            file: file.to_string(),
            size,
            outcome,
            note,
        }
    }
}

/// One word for how it ended, from this side.
pub fn outcome_word(outcome: Outcome, direction: Direction) -> &'static str {
    match (outcome, direction) {
        (Outcome::Done, Direction::Sent) => "sent",
        (Outcome::Done, Direction::Received) => "saved",
        (Outcome::Declined, Direction::Sent) => "declined by them",
        (Outcome::Declined, Direction::Received) => "declined",
        (Outcome::Cancelled, _) => "cancelled",
        (Outcome::Failed, _) => "failed",
    }
}

/// How a send ended, for its line: shared by `beam send` and the view.
pub fn send_outcome(
    result: &Result<crate::transfer::SendSummary, crate::transfer::TransferError>,
) -> (Outcome, Option<String>) {
    use crate::transfer::{RejectReason, TransferError};
    match result {
        Ok(summary) => (
            Outcome::Done,
            summary.final_name.as_ref().map(|n| format!("saved as {n}")),
        ),
        Err(TransferError::Rejected(RejectReason::Declined | RejectReason::Expired)) => {
            (Outcome::Declined, None)
        }
        Err(TransferError::PeerInterrupted) => {
            (Outcome::Cancelled, Some("they stopped beam".to_string()))
        }
        Err(e) => (Outcome::Failed, Some(e.to_string())),
    }
}

/// How a send the person stopped ends up in the history: cancelled once the
/// friend was reached, a failure to reach them before that, so it does not
/// count as having seen them.
pub fn stopped_outcome(reached: bool) -> (Outcome, Option<String>) {
    if reached {
        (Outcome::Cancelled, Some("you stopped it".to_string()))
    } else {
        (
            Outcome::Failed,
            Some("cancelled before they were reached".to_string()),
        )
    }
}

/// How long ago `at` was, in words that need no time zone: "just now",
/// "5 min ago", "3 h ago", "yesterday", "4 days ago", "6 weeks ago".
pub fn ago(at: u64, now: u64) -> String {
    let secs = now.saturating_sub(at);
    match secs {
        0..60 => "just now".to_string(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..172_800 => "yesterday".to_string(),
        172_800..1_209_600 => format!("{} days ago", secs / 86_400),
        _ => format!("{} weeks ago", secs / 604_800),
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Adds `entry`. Best effort: a history that cannot be written is not a
/// reason to fail a transfer that worked, so errors are swallowed.
pub fn record(store: &Store, entry: &Entry) {
    let _ = try_record(store, entry);
}

fn try_record(store: &Store, entry: &Entry) -> std::io::Result<()> {
    let _lock = lock(store)?;
    let mut entries = read(store);
    entries.push(entry.clone());
    write(store, &entries)
}

/// Everything in the file, oldest first. Lines that do not parse are
/// skipped: one damaged line must not hide the rest.
pub fn read(store: &Store) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(store.history_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Deletes the history. Whether there was one.
pub fn clear(store: &Store) -> std::io::Result<bool> {
    let _lock = lock(store)?;
    match std::fs::remove_file(store.history_path()) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn write(store: &Store, entries: &[Entry]) -> std::io::Result<()> {
    let start = entries.len().saturating_sub(KEEP);
    let mut text = String::new();
    for entry in &entries[start..] {
        text.push_str(&serde_json::to_string(entry).map_err(std::io::Error::other)?);
        text.push('\n');
    }
    store.save_history(&text).map_err(std::io::Error::other)
}

/// Held while the file is read and rewritten, so two writers do not lose
/// each other's line. Released when dropped.
fn lock(store: &Store) -> std::io::Result<std::fs::File> {
    store.ensure_dirs().map_err(std::io::Error::other)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store.history_lock_path())?;
    file.lock()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(file: &str, outcome: Outcome) -> Entry {
        Entry::now(
            Direction::Received,
            "alice",
            "SHA256:ab12",
            file,
            10,
            outcome,
            None,
        )
    }

    #[test]
    fn entries_are_kept_in_order_and_the_fingerprint_is_hex() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(read(&store).is_empty());
        record(&store, &entry("a.txt", Outcome::Done));
        record(&store, &entry("b.txt", Outcome::Declined));
        let all = read(&store);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].file, "a.txt");
        assert_eq!(all[1].outcome, Outcome::Declined);
        assert_eq!(all[0].fingerprint, "ab12");
    }

    #[test]
    fn only_the_newest_entries_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let many: Vec<Entry> = (0..KEEP + 5)
            .map(|i| entry(&format!("{i}"), Outcome::Done))
            .collect();
        write(&store, &many).unwrap();
        record(&store, &entry("last", Outcome::Done));
        let all = read(&store);
        assert_eq!(all.len(), KEEP);
        assert_eq!(all.last().unwrap().file, "last");
        assert_eq!(all[0].file, "6");
    }

    #[test]
    fn a_damaged_line_hides_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        record(&store, &entry("a.txt", Outcome::Done));
        let path = store.history_path();
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{not json\n");
        std::fs::write(&path, text).unwrap();
        record(&store, &entry("b.txt", Outcome::Done));
        let files: Vec<String> = read(&store).into_iter().map(|e| e.file).collect();
        assert_eq!(files, ["a.txt", "b.txt"]);
    }

    #[test]
    fn ago_needs_no_time_zone() {
        let now = 10_000_000;
        assert_eq!(ago(now - 5, now), "just now");
        assert_eq!(ago(now - 300, now), "5 min ago");
        assert_eq!(ago(now - 3 * 3600, now), "3 h ago");
        assert_eq!(ago(now - 100_000, now), "yesterday");
        assert_eq!(ago(now - 4 * 86_400, now), "4 days ago");
        assert_eq!(ago(now - 3 * 604_800, now), "3 weeks ago");
        assert_eq!(ago(now + 50, now), "just now", "a clock that moved back");
    }

    #[test]
    fn clear_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(!clear(&store).unwrap());
        record(&store, &entry("a.txt", Outcome::Done));
        assert!(clear(&store).unwrap());
        assert!(read(&store).is_empty());
    }

    #[test]
    fn writers_in_parallel_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        std::thread::scope(|s| {
            for t in 0..4 {
                let store = &store;
                s.spawn(move || {
                    for i in 0..10 {
                        record(store, &entry(&format!("{t}-{i}"), Outcome::Done));
                    }
                });
            }
        });
        assert_eq!(read(&store).len(), 40);
    }
}
