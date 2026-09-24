//! Partial transfers on disk, and everything that keeps them trustworthy.
//!
//! A partial lives in its own directory under `~/.beam/tmp/`:
//!
//! ```text
//! <id>/state.json   metadata and the have-bitmap, rewritten atomically
//! <id>/part         the file being assembled
//! <id>/hashes       32 bytes per chunk, at fixed offsets
//! <id>/lock         held for as long as a session is using this partial
//! ```
//!
//! The hashes live in their own fixed-layout file rather than in `state.json`,
//! so that `state.json` stays about a kilobyte whatever the file size. Putting
//! them in the JSON would mean rewriting a megabyte of metadata after every
//! 4 MiB chunk of a large transfer.
//!
//! See ADR-0021 for how a partial is matched to an incoming request, and
//! ADR-0022 for the write ordering and the retention rules.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::hex;
use crate::identity::Fingerprint;

use super::bitmap::ChunkBitmap;
use super::chunk::{ChunkPlan, sha256_hex};

/// How long a partial survives without being touched.
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The layout version of `state.json`.
const STATE_VERSION: u32 = 1;

const STATE_FILE: &str = "state.json";
const PART_FILE: &str = "part";
const HASHES_FILE: &str = "hashes";
const LOCK_FILE: &str = "lock";

/// Bytes of hash stored per chunk.
const HASH_BYTES: u64 = 32;

/// What makes two transfers the same transfer.
///
/// Deliberately *not* the transfer id: that is chosen by the sender, so
/// matching on it would let anyone who can guess or replay one attach to
/// somebody else's partial. Every field here is one the sender cannot change
/// without changing which file is being sent. See ADR-0021.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialKey {
    pub peer_fingerprint: Fingerprint,
    pub file_sha256: String,
    pub size: u64,
    pub chunk_size: u32,
}

/// Why a partial could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum PartialError {
    #[error("another beam session is already working on this transfer")]
    Busy,
    #[error("{action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a valid partial transfer: {source}")]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl PartialError {
    fn io(action: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

/// What `state.json` holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartialState {
    pub version: u32,
    /// The fingerprint of the peer this data came from, as `SHA256:...`.
    pub peer_fingerprint: String,
    pub file_name: String,
    pub file_sha256: String,
    pub size: u64,
    pub chunk_size: u32,
    pub chunk_count: u32,
    pub created: String,
    pub updated: String,
    /// The have-bitmap, base64.
    pub have: String,
}

impl PartialState {
    fn matches(&self, key: &PartialKey) -> bool {
        self.version == STATE_VERSION
            && self.peer_fingerprint == key.peer_fingerprint.to_string()
            && self.file_sha256 == key.file_sha256
            && self.size == key.size
            && self.chunk_size == key.chunk_size
    }

    fn updated_at(&self) -> Option<OffsetDateTime> {
        OffsetDateTime::parse(&self.updated, &Rfc3339).ok()
    }
}

/// One partial, as `beam transfers` reports it.
#[derive(Clone, Debug)]
pub struct PartialSummary {
    pub id: String,
    pub peer_fingerprint: String,
    pub file_name: String,
    pub size: u64,
    pub have_bytes: u64,
    pub chunk_count: u32,
    pub have_chunks: u32,
    pub updated: Option<OffsetDateTime>,
    pub expired: bool,
}

impl PartialSummary {
    /// How far along this partial is, 0-100.
    pub fn percent(&self) -> u8 {
        crate::ui::percent(self.have_bytes, self.size)
    }

    /// How long ago it was last written to.
    pub fn age(&self) -> Option<Duration> {
        let updated = self.updated?;
        (OffsetDateTime::now_utc() - updated).try_into().ok()
    }
}

/// Holds the lock file open for as long as a session is using a partial.
///
/// The lock is a file of its own rather than `state.json`, because `state.json`
/// is replaced by rename on every write and a lock held on the old file would
/// quietly stop meaning anything.
#[derive(Debug)]
struct LockGuard {
    file: std::fs::File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// An open partial transfer, locked for this session.
pub struct Partial {
    dir: PathBuf,
    state: PartialState,
    bitmap: ChunkBitmap,
    plan: ChunkPlan,
    part: tokio::fs::File,
    hashes: tokio::fs::File,
    /// Whether this session created it, which decides whether an early failure
    /// should tidy it away (ADR-0022).
    created_here: bool,
    _lock: LockGuard,
}

impl Partial {
    /// Where this partial lives.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The chunks already held and verified.
    pub fn bitmap(&self) -> &ChunkBitmap {
        &self.bitmap
    }

    /// How many bytes are already held.
    pub fn have_bytes(&self) -> u64 {
        (0..self.plan.chunk_count())
            .filter(|i| self.bitmap.get(*i))
            .map(|i| u64::from(self.plan.len_of(i)))
            .sum()
    }

    /// Whether this session created this partial rather than finding it.
    pub fn is_new(&self) -> bool {
        self.created_here
    }

    /// When it was last written to, if it was found rather than created.
    pub fn updated_at(&self) -> Option<OffsetDateTime> {
        self.state.updated_at()
    }

    /// The file being assembled.
    pub fn part_path(&self) -> PathBuf {
        self.dir.join(PART_FILE)
    }

    /// Stores one verified chunk.
    ///
    /// The order here is the whole point of the crash-consistency story: the
    /// data and its hash are on disk and flushed **before** the bitmap says so.
    /// A crash in the middle therefore loses the claim rather than the data —
    /// the chunk is asked for again, which costs bandwidth, instead of being
    /// treated as present when it is not. See ADR-0022.
    pub async fn store_chunk(
        &mut self,
        index: u32,
        bytes: &[u8],
        digest: &str,
    ) -> Result<(), PartialError> {
        let part_path = self.part_path();

        self.part
            .seek(std::io::SeekFrom::Start(self.plan.offset_of(index)))
            .await
            .map_err(|e| PartialError::io("seek in", &part_path, e))?;
        self.part
            .write_all(bytes)
            .await
            .map_err(|e| PartialError::io("write", &part_path, e))?;
        self.part
            .sync_all()
            .await
            .map_err(|e| PartialError::io("flush", &part_path, e))?;

        let mut raw = [0u8; HASH_BYTES as usize];
        hex::decode_into(digest, &mut raw).expect("a hash we just computed is valid hex");
        let hashes_path = self.dir.join(HASHES_FILE);
        self.hashes
            .seek(std::io::SeekFrom::Start(u64::from(index) * HASH_BYTES))
            .await
            .map_err(|e| PartialError::io("seek in", &hashes_path, e))?;
        self.hashes
            .write_all(&raw)
            .await
            .map_err(|e| PartialError::io("write", &hashes_path, e))?;
        self.hashes
            .sync_all()
            .await
            .map_err(|e| PartialError::io("flush", &hashes_path, e))?;

        // Only now is the chunk allowed to count.
        self.bitmap.set(index);
        self.write_state()?;
        Ok(())
    }

    /// Re-hashes everything the bitmap claims and drops whatever does not match.
    ///
    /// Disk is no more trustworthy than the wire: a partial may have been
    /// edited, truncated, or damaged since the last session. A chunk that fails
    /// is simply cleared, and the sender is asked for it again.
    ///
    /// Returns how many chunks were dropped.
    pub async fn reverify(&mut self) -> Result<u32, PartialError> {
        let part_path = self.part_path();
        let hashes_path = self.dir.join(HASHES_FILE);
        let mut dropped = 0;

        for index in 0..self.plan.chunk_count() {
            if !self.bitmap.get(index) {
                continue;
            }

            let mut expected = [0u8; HASH_BYTES as usize];
            let read_hash = async {
                self.hashes
                    .seek(std::io::SeekFrom::Start(u64::from(index) * HASH_BYTES))
                    .await?;
                self.hashes.read_exact(&mut expected).await?;
                Ok::<(), std::io::Error>(())
            }
            .await;
            if read_hash.is_err() {
                self.bitmap.clear(index);
                dropped += 1;
                continue;
            }

            let mut bytes = vec![0u8; self.plan.len_of(index) as usize];
            let read_data = async {
                self.part
                    .seek(std::io::SeekFrom::Start(self.plan.offset_of(index)))
                    .await?;
                self.part.read_exact(&mut bytes).await?;
                Ok::<(), std::io::Error>(())
            }
            .await;
            if read_data.is_err() {
                self.bitmap.clear(index);
                dropped += 1;
                continue;
            }

            if sha256_hex(&bytes) != hex::encode(&expected) {
                self.bitmap.clear(index);
                dropped += 1;
            }
        }

        let _ = (&part_path, &hashes_path);
        if dropped > 0 {
            self.write_state()?;
        }
        Ok(dropped)
    }

    /// Rewrites `state.json` atomically.
    fn write_state(&mut self) -> Result<(), PartialError> {
        self.state.have = self.bitmap.encode();
        self.state.updated = now_rfc3339();
        write_state_file(&self.dir, &self.state)
    }
}

/// The directory of partial transfers under `~/.beam/tmp`.
#[derive(Clone, Debug)]
pub struct PartialStore {
    root: PathBuf,
}

impl PartialStore {
    /// A store rooted at `~/.beam/tmp`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where partials live.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Finds the partial matching `key`, or starts one, and locks it.
    pub async fn open(
        &self,
        key: &PartialKey,
        file_name: &str,
        plan: ChunkPlan,
    ) -> Result<Partial, PartialError> {
        std::fs::create_dir_all(&self.root)
            .map_err(|e| PartialError::io("create", &self.root, e))?;

        if let Some((dir, state)) = self.find(key)? {
            return self.attach(dir, state, plan, false).await;
        }

        let id = new_id();
        let dir = self.root.join(&id);
        std::fs::create_dir_all(&dir).map_err(|e| PartialError::io("create", &dir, e))?;

        let now = now_rfc3339();
        let state = PartialState {
            version: STATE_VERSION,
            peer_fingerprint: key.peer_fingerprint.to_string(),
            file_name: file_name.to_string(),
            file_sha256: key.file_sha256.clone(),
            size: key.size,
            chunk_size: key.chunk_size,
            chunk_count: plan.chunk_count(),
            created: now.clone(),
            updated: now,
            have: ChunkBitmap::new(plan.chunk_count()).encode(),
        };
        write_state_file(&dir, &state)?;

        self.attach(dir, state, plan, true).await
    }

    /// Opens the files of a partial and takes its lock.
    async fn attach(
        &self,
        dir: PathBuf,
        state: PartialState,
        plan: ChunkPlan,
        created_here: bool,
    ) -> Result<Partial, PartialError> {
        let lock_path = dir.join(LOCK_FILE);
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| PartialError::io("open", &lock_path, e))?;

        // std's own advisory lock, which has been available since Rust 1.89.
        // fs4 offers the same thing, but a standard-library lock is one less
        // piece of behaviour that depends on a dependency's choices.
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(PartialError::Busy),
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(PartialError::io("lock", &lock_path, e));
            }
        }
        let lock = LockGuard { file: lock_file };

        let part_path = dir.join(PART_FILE);
        let part = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part_path)
            .await
            .map_err(|e| PartialError::io("open", &part_path, e))?;

        let hashes_path = dir.join(HASHES_FILE);
        let hashes = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&hashes_path)
            .await
            .map_err(|e| PartialError::io("open", &hashes_path, e))?;

        // A bitmap that does not describe this transfer is treated as empty
        // rather than trusted; the data is still there to be re-verified.
        let bitmap = ChunkBitmap::decode(&state.have, plan.chunk_count())
            .unwrap_or_else(|_| ChunkBitmap::new(plan.chunk_count()));

        Ok(Partial {
            dir,
            state,
            bitmap,
            plan,
            part,
            hashes,
            created_here,
            _lock: lock,
        })
    }

    /// The partial matching `key`, if there is one.
    fn find(&self, key: &PartialKey) -> Result<Option<(PathBuf, PartialState)>, PartialError> {
        for (dir, state) in self.read_all()? {
            if state.matches(key) {
                return Ok(Some((dir, state)));
            }
        }
        Ok(None)
    }

    /// Every partial that parses, with its directory.
    fn read_all(&self) -> Result<Vec<(PathBuf, PartialState)>, PartialError> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(PartialError::io("read", &self.root, e)),
        };

        let mut found = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let state_path = dir.join(STATE_FILE);
            let Ok(text) = std::fs::read_to_string(&state_path) else {
                continue;
            };
            // A directory that does not parse is left alone rather than
            // deleted: it may be a newer beam's, and it is not in the way.
            if let Ok(state) = serde_json::from_str::<PartialState>(&text) {
                found.push((dir, state));
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(found)
    }

    /// Everything `beam transfers` needs to print.
    pub fn list(&self, max_age: Duration) -> Result<Vec<PartialSummary>, PartialError> {
        let now = OffsetDateTime::now_utc();
        let mut summaries = Vec::new();

        for (dir, state) in self.read_all()? {
            let bitmap = ChunkBitmap::decode(&state.have, state.chunk_count)
                .unwrap_or_else(|_| ChunkBitmap::new(state.chunk_count));
            let plan = ChunkPlan::new(state.size, state.chunk_size.max(1));
            let have_bytes = (0..state.chunk_count)
                .filter(|i| bitmap.get(*i))
                .map(|i| u64::from(plan.len_of(i)))
                .sum();

            let updated = state.updated_at();
            let expired = updated
                .map(|u| (now - u).whole_seconds() as u64 > max_age.as_secs())
                .unwrap_or(false);

            summaries.push(PartialSummary {
                id: dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                peer_fingerprint: state.peer_fingerprint,
                file_name: state.file_name,
                size: state.size,
                have_bytes,
                chunk_count: state.chunk_count,
                have_chunks: bitmap.count(),
                updated,
                expired,
            });
        }
        Ok(summaries)
    }

    /// Deletes one partial by id.
    ///
    /// A partial another session is using is left alone: the lock is taken
    /// first, so a running transfer cannot have the ground pulled from under it.
    pub fn remove(&self, id: &str) -> Result<bool, PartialError> {
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return Ok(false);
        }

        let lock_path = dir.join(LOCK_FILE);
        if let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            && matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
        {
            return Err(PartialError::Busy);
        }

        std::fs::remove_dir_all(&dir).map_err(|e| PartialError::io("remove", &dir, e))?;
        Ok(true)
    }

    /// Deletes partials that have not been touched for `max_age`.
    ///
    /// Runs when `beam listen` starts, which is the only moment beam is both
    /// long-lived and certain to be idle. Returns the ids removed.
    pub fn sweep_expired(&self, max_age: Duration) -> Result<Vec<String>, PartialError> {
        let mut removed = Vec::new();
        for summary in self.list(max_age)? {
            if !summary.expired {
                continue;
            }
            match self.remove(&summary.id) {
                // In use by another session, so not actually stale.
                Ok(_) | Err(PartialError::Busy) => {}
                Err(e) => return Err(e),
            }
            removed.push(summary.id);
        }
        Ok(removed)
    }
}

fn write_state_file(dir: &Path, state: &PartialState) -> Result<(), PartialError> {
    let path = dir.join(STATE_FILE);
    let json = serde_json::to_vec_pretty(state).expect("partial state always serialises");

    let mut temp = tempfile::Builder::new()
        .prefix(".state-")
        .tempfile_in(dir)
        .map_err(|e| PartialError::io("create a file in", dir, e))?;
    std::io::Write::write_all(&mut temp, &json).map_err(|e| PartialError::io("write", &path, e))?;
    temp.as_file()
        .sync_all()
        .map_err(|e| PartialError::io("flush", &path, e))?;
    temp.persist(&path)
        .map_err(|e| PartialError::io("replace", &path, e.error))?;
    Ok(())
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero is a valid nanosecond")
        .format(&Rfc3339)
        .expect("the current time always formats")
}

/// A directory name for a new partial, chosen locally.
///
/// Never the sender's transfer id: see ADR-0021.
fn new_id() -> String {
    let mut bytes = [0u8; 16];
    // Falling back to the clock keeps a working, if less random, name; the id
    // is a directory label, not a secret.
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = OffsetDateTime::now_utc().unix_timestamp_nanos() as u128;
        bytes.copy_from_slice(&nanos.to_be_bytes());
    }
    hex::encode(&bytes)
}
