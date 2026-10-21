//! Putting a finished file in place, despite Windows.
//!
//! beam never edits a file in place: it writes a temporary file and renames
//! it over the old one (or onto the destination), so a reader sees the old
//! file or the new one, never half of each. On Windows that rename is
//! refused ("Access is denied", or a sharing violation) while another
//! program has the file open, even for a moment: Windows Defender scanning
//! a new file, the search indexer, a backup tool, or a test reading a
//! partial's `state.json`. Failing a whole transfer for that is wrong, so
//! the rename is retried for about a second.
//!
//! Only that error is retried, and only on Windows. Anything else fails at
//! once, as before; on Linux a refused rename is a real permission problem,
//! and waiting would only delay saying so.

use std::io;
use std::path::Path;
use std::time::Duration;

use tempfile::{NamedTempFile, PersistError};

/// Pauses between attempts: about a second in all.
const PAUSES_MS: [u64; 8] = [10, 20, 40, 80, 160, 250, 250, 250];

/// Whether `error` means "someone else has the file open right now".
fn is_momentary(error: &io::Error) -> bool {
    // ERROR_SHARING_VIOLATION (32) and ERROR_LOCK_VIOLATION (33) arrive as
    // raw codes; "Access is denied" (5) as `PermissionDenied`.
    cfg!(windows)
        && (error.kind() == io::ErrorKind::PermissionDenied
            || matches!(error.raw_os_error(), Some(32 | 33)))
}

/// `temp.persist(path)`, retried while the file is momentarily in use.
pub(crate) fn persist(mut temp: NamedTempFile, path: &Path) -> Result<(), PersistError> {
    for pause in PAUSES_MS {
        match temp.persist(path) {
            Ok(_) => return Ok(()),
            Err(e) if is_momentary(&e.error) => {
                temp = e.file;
                std::thread::sleep(Duration::from_millis(pause));
            }
            Err(e) => return Err(e),
        }
    }
    temp.persist(path).map(|_| ())
}

/// `tokio::fs::rename(from, to)`, retried while a file is momentarily in use.
pub(crate) async fn rename(from: &Path, to: &Path) -> io::Result<()> {
    for pause in PAUSES_MS {
        match tokio::fs::rename(from, to).await {
            Ok(()) => return Ok(()),
            Err(e) if is_momentary(&e) => {
                tokio::time::sleep(Duration::from_millis(pause)).await;
            }
            Err(e) => return Err(e),
        }
    }
    tokio::fs::rename(from, to).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn a_replace_with_nobody_in_the_way_happens_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"old").unwrap();
        let mut temp = NamedTempFile::new_in(dir.path()).unwrap();
        temp.write_all(b"new").unwrap();
        persist(temp, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    /// The case that failed transfers: another program holds the file for a
    /// moment (here without letting anyone delete or rename it, as a scanner
    /// may). The replace waits it out instead of failing.
    #[cfg(windows)]
    #[test]
    fn a_replace_waits_out_a_program_that_holds_the_file_for_a_moment() {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_SHARE_READ: u32 = 1;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"old").unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            drop(held);
        });
        let mut temp = NamedTempFile::new_in(dir.path()).unwrap();
        temp.write_all(b"new").unwrap();
        persist(temp, &path).expect("the replace waits for the file to be free");
        release.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    /// A real refusal is not waited on forever: after about a second, the
    /// error comes back.
    #[cfg(windows)]
    #[test]
    fn a_file_that_stays_held_still_fails_after_about_a_second() {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_SHARE_READ: u32 = 1;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"old").unwrap();
        let _held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        let started = std::time::Instant::now();
        let temp = NamedTempFile::new_in(dir.path()).unwrap();
        assert!(persist(temp, &path).is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"old",
            "nothing was half-replaced"
        );
    }
}
