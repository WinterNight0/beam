//! On-disk state under the beam home directory.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

use super::keys::{Identity, KeyError, parse_public_line};
use super::known_peers::{HEADER, KnownPeers, ParseError};

/// Overrides the beam home directory; mainly used by tests.
pub const DIR_ENV: &str = "BEAM_DIR";

/// The directory created under the user's home directory.
pub const DIR_NAME: &str = ".beam";

/// File names inside the beam home directory.
pub const PRIVATE_KEY_NAME: &str = "id_ed25519";
pub const PUBLIC_KEY_NAME: &str = "id_ed25519.pub";
pub const KNOWN_PEERS_NAME: &str = "known_peers";
pub const TMP_NAME: &str = "tmp";
pub const CONFIG_NAME: &str = "config.toml";
/// What a running `beam listen` offers, for `beam whoami` (ADR-0037).
pub const LISTEN_STATUS_NAME: &str = "listen.json";
/// Held locked by a running `beam listen`; `listen.json` counts only while it is.
pub const LISTEN_LOCK_NAME: &str = "listen.lock";
/// What a running background agent offers `beam inbox`: its local port and
/// the token that proves a client is this user (ADR-0042). Private.
pub const AGENT_STATUS_NAME: &str = "agent.json";
/// Held locked by a running background agent; `agent.json` counts only while
/// it is.
pub const AGENT_LOCK_NAME: &str = "agent.lock";
/// What the background agent did, for when nobody was watching.
pub const AGENT_LOG_NAME: &str = "agent.log";

#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;
#[cfg(unix)]
const PUBLIC_FILE_MODE: u32 = 0o644;
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;

/// Why a store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("no identity found; run `beam init` first")]
    NoIdentity,
    #[error("an identity already exists on this device at {0}")]
    IdentityExists(PathBuf),
    #[error("{path} does not match {private}; refusing to guess which one is yours")]
    PublicKeyMismatch { path: PathBuf, private: PathBuf },
    #[error("could not locate your home directory; set {DIR_ENV} instead")]
    NoHomeDirectory,
    #[error("{path}: {source}")]
    KnownPeers {
        path: PathBuf,
        #[source]
        source: ParseError,
    },
    #[error("{path}: {source}")]
    Key {
        path: PathBuf,
        #[source]
        source: KeyError,
    },
    #[error("{action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl StoreError {
    pub(crate) fn io(
        action: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

/// The beam home directory and everything in it.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// A store rooted at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Resolves the default beam home directory, honouring `$BEAM_DIR`.
    pub fn default_dir() -> Result<PathBuf, StoreError> {
        Self::resolve_dir(std::env::var_os(DIR_ENV))
    }

    /// The body of [`Store::default_dir`], with the environment passed in.
    ///
    /// Taking the override as an argument keeps this testable without mutating
    /// the process environment, which is both racy across parallel tests and
    /// `unsafe` in edition 2024.
    pub fn resolve_dir(env_override: Option<std::ffi::OsString>) -> Result<PathBuf, StoreError> {
        if let Some(dir) = env_override
            && !dir.is_empty()
        {
            return Ok(PathBuf::from(dir));
        }
        dirs::home_dir()
            .map(|home| home.join(DIR_NAME))
            .ok_or(StoreError::NoHomeDirectory)
    }

    /// The beam home directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The path of the device private key.
    pub fn private_key_path(&self) -> PathBuf {
        self.dir.join(PRIVATE_KEY_NAME)
    }

    /// The path of the device public key.
    pub fn public_key_path(&self) -> PathBuf {
        self.dir.join(PUBLIC_KEY_NAME)
    }

    /// Path of `config.toml`, which may not exist.
    pub fn config_path(&self) -> PathBuf {
        self.dir.join(CONFIG_NAME)
    }

    /// The path of the `known_peers` database.
    pub fn known_peers_path(&self) -> PathBuf {
        self.dir.join(KNOWN_PEERS_NAME)
    }

    /// Where a running `beam listen` publishes its invite and pairing code.
    pub fn listen_status_path(&self) -> PathBuf {
        self.dir.join(LISTEN_STATUS_NAME)
    }

    /// The lock a running `beam listen` holds.
    pub fn listen_lock_path(&self) -> PathBuf {
        self.dir.join(LISTEN_LOCK_NAME)
    }

    /// Writes `listen.json` atomically. Private: it holds the live pairing
    /// code (ADR-0037).
    pub fn save_listen_status(&self, json: &str) -> Result<(), StoreError> {
        self.ensure_dirs()?;
        write_atomic(
            &self.listen_status_path(),
            json.as_bytes(),
            Visibility::Private,
        )
    }

    /// Where a running background agent publishes how to reach it.
    pub fn agent_status_path(&self) -> PathBuf {
        self.dir.join(AGENT_STATUS_NAME)
    }

    /// The lock a running background agent holds.
    pub fn agent_lock_path(&self) -> PathBuf {
        self.dir.join(AGENT_LOCK_NAME)
    }

    /// The background agent's log.
    pub fn agent_log_path(&self) -> PathBuf {
        self.dir.join(AGENT_LOG_NAME)
    }

    /// Writes `agent.json` atomically. Private: it holds the token that lets
    /// a local client answer transfer requests (ADR-0042).
    pub fn save_agent_status(&self, json: &str) -> Result<(), StoreError> {
        self.ensure_dirs()?;
        write_atomic(
            &self.agent_status_path(),
            json.as_bytes(),
            Visibility::Private,
        )
    }

    /// The directory for in-progress transfers (used from M2 onwards).
    pub fn tmp_path(&self) -> PathBuf {
        self.dir.join(TMP_NAME)
    }

    /// Creates the beam home directory and its `tmp` subdirectory.
    pub fn ensure_dirs(&self) -> Result<(), StoreError> {
        for dir in [self.dir.clone(), self.tmp_path()] {
            fs::create_dir_all(&dir).map_err(|e| StoreError::io("create", &dir, e))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&dir, fs::Permissions::from_mode(DIR_MODE))
                    .map_err(|e| StoreError::io("set permissions on", &dir, e))?;
            }
        }
        Ok(())
    }

    /// Whether a private key is already present.
    pub fn has_identity(&self) -> bool {
        self.private_key_path().exists()
    }

    /// Writes the keypair.
    ///
    /// Unless `force` is set this refuses to overwrite an existing key, because
    /// replacing a key silently would invalidate every pairing other peers hold
    /// for this device.
    pub fn save_identity(&self, identity: &Identity, force: bool) -> Result<(), StoreError> {
        if !force && self.has_identity() {
            return Err(StoreError::IdentityExists(self.private_key_path()));
        }
        self.ensure_dirs()?;

        let pem = identity.to_pkcs8_pem().map_err(|e| StoreError::Key {
            path: self.private_key_path(),
            source: e,
        })?;
        write_atomic(
            &self.private_key_path(),
            pem.as_bytes(),
            Visibility::Private,
        )?;
        write_atomic(
            &self.public_key_path(),
            identity.to_public_line().as_bytes(),
            Visibility::Public,
        )?;

        if !self.known_peers_path().exists() {
            write_atomic(
                &self.known_peers_path(),
                HEADER.as_bytes(),
                Visibility::Private,
            )?;
        }
        Ok(())
    }

    /// Reads the device keypair.
    ///
    /// Verifies that the stored public key file matches the private key; a
    /// mismatch means the directory has been tampered with or corrupted, and is
    /// never repaired silently.
    pub fn load_identity(&self) -> Result<Identity, StoreError> {
        let private_path = self.private_key_path();
        let pem = match fs::read_to_string(&private_path) {
            Ok(pem) => pem,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::NoIdentity);
            }
            Err(e) => return Err(StoreError::io("read", &private_path, e)),
        };
        let mut identity = Identity::from_pkcs8_pem(&pem, "").map_err(|e| StoreError::Key {
            path: private_path.clone(),
            source: e,
        })?;

        let public_path = self.public_key_path();
        match fs::read_to_string(&public_path) {
            // Tolerated: the public half is derivable from the private key.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(StoreError::io("read", &public_path, e)),
            Ok(line) => {
                let (file_key, comment) =
                    parse_public_line(&line).map_err(|e| StoreError::Key {
                        path: public_path.clone(),
                        source: e,
                    })?;
                if file_key != identity.verifying_key() {
                    return Err(StoreError::PublicKeyMismatch {
                        path: public_path,
                        private: private_path,
                    });
                }
                identity.set_comment(&comment);
            }
        }
        Ok(identity)
    }

    /// Reads the peer database. A missing file is an empty database.
    pub fn load_known_peers(&self) -> Result<KnownPeers, StoreError> {
        let path = self.known_peers_path();
        let data = match fs::read_to_string(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(KnownPeers::with_header());
            }
            Err(e) => return Err(StoreError::io("read", &path, e)),
        };
        KnownPeers::parse(&data).map_err(|source| StoreError::KnownPeers { path, source })
    }

    /// Writes the peer database atomically.
    pub fn save_known_peers(&self, peers: &KnownPeers) -> Result<(), StoreError> {
        self.ensure_dirs()?;
        let mut data = peers.render();
        if !data.starts_with('#') {
            data.insert_str(0, HEADER);
        }
        write_atomic(
            &self.known_peers_path(),
            data.as_bytes(),
            Visibility::Private,
        )
    }

    /// Reports private files that are readable by other users.
    ///
    /// On Windows the Unix mode bits are not meaningful, so the check is
    /// skipped; see `docs/decisions.md` ADR-0004.
    pub fn permission_warnings(&self) -> Vec<String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut warnings = Vec::new();
            for path in [self.private_key_path(), self.known_peers_path()] {
                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                let mode = meta.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    warnings.push(format!(
                        "{} has permissions {mode:04o}; expected 0600",
                        path.display()
                    ));
                }
            }
            warnings
        }
        #[cfg(not(unix))]
        {
            Vec::new()
        }
    }
}

/// Whether a file may be read by other users on the machine.
#[derive(Clone, Copy)]
enum Visibility {
    Private,
    Public,
}

/// Writes `data` to a temporary file in the same directory and renames it into
/// place, so a crash mid-write cannot leave a truncated file.
fn write_atomic(path: &Path, data: &[u8], visibility: Visibility) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp =
        NamedTempFile::new_in(dir).map_err(|e| StoreError::io("create a file in", dir, e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = match visibility {
            Visibility::Private => PRIVATE_FILE_MODE,
            Visibility::Public => PUBLIC_FILE_MODE,
        };
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|e| StoreError::io("set permissions on", path, e))?;
    }
    #[cfg(not(unix))]
    let _ = visibility;

    tmp.write_all(data)
        .map_err(|e| StoreError::io("write", path, e))?;
    tmp.as_file()
        .sync_all()
        .map_err(|e| StoreError::io("flush", path, e))?;
    tmp.persist(path)
        .map_err(|e| StoreError::io("replace", path, e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vectors::{VECTORS, verifying_key};
    use crate::identity::{HEADER as PEERS_HEADER, KEY_TYPE, Peer};
    use tempfile::TempDir;

    fn test_store() -> (TempDir, Store) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Store::new(tmp.path().join(".beam"));
        (tmp, store)
    }

    #[test]
    fn resolve_dir_prefers_the_environment_override() {
        let explicit = std::ffi::OsString::from("some/where");
        assert_eq!(
            Store::resolve_dir(Some(explicit)).expect("resolve"),
            PathBuf::from("some/where")
        );

        let fallback = Store::resolve_dir(None).expect("resolve");
        assert!(fallback.ends_with(DIR_NAME), "{}", fallback.display());

        // An empty override is treated as unset.
        let empty = Store::resolve_dir(Some(std::ffi::OsString::new())).expect("resolve");
        assert_eq!(empty, fallback);
    }

    #[test]
    fn saves_and_loads_an_identity() {
        let (_tmp, store) = test_store();
        assert!(!store.has_identity());

        let identity = Identity::generate("laptop").expect("generate");
        store.save_identity(&identity, false).expect("save");
        assert!(store.has_identity());

        let loaded = store.load_identity().expect("load");
        assert_eq!(loaded.signing_key(), identity.signing_key());
        assert_eq!(loaded.fingerprint(), identity.fingerprint());
        assert_eq!(loaded.comment(), "laptop");

        // init also seeds known_peers and the tmp directory.
        assert!(store.known_peers_path().exists());
        assert!(store.tmp_path().is_dir());
    }

    #[test]
    fn refuses_to_overwrite_an_identity_without_force() {
        let (_tmp, store) = test_store();
        let first = Identity::generate("one").expect("generate");
        store.save_identity(&first, false).expect("save");

        let second = Identity::generate("two").expect("generate");
        let err = store.save_identity(&second, false).unwrap_err();
        assert!(matches!(err, StoreError::IdentityExists(_)), "{err}");
        assert_eq!(
            store.load_identity().expect("load").fingerprint(),
            first.fingerprint(),
            "the original key was replaced despite the error"
        );

        store.save_identity(&second, true).expect("force save");
        assert_eq!(
            store.load_identity().expect("load").fingerprint(),
            second.fingerprint(),
            "--force did not replace the key"
        );
    }

    #[test]
    fn re_running_init_keeps_existing_known_peers() {
        let (_tmp, store) = test_store();
        store.ensure_dirs().expect("ensure dirs");
        let existing = format!("{PEERS_HEADER}alice {KEY_TYPE} {}\n", VECTORS[0].public_b64);
        fs::write(store.known_peers_path(), existing).expect("seed");

        let identity = Identity::generate("laptop").expect("generate");
        store.save_identity(&identity, true).expect("save");

        let known = store.load_known_peers().expect("load");
        assert!(
            known.lookup("alice").is_some(),
            "init destroyed known_peers"
        );
    }

    #[test]
    fn missing_identity_is_reported_clearly() {
        let (_tmp, store) = test_store();
        assert!(matches!(
            store.load_identity().unwrap_err(),
            StoreError::NoIdentity
        ));
    }

    #[test]
    fn detects_a_public_key_that_does_not_match_the_private_key() {
        let (_tmp, store) = test_store();
        let identity = Identity::generate("laptop").expect("generate");
        store.save_identity(&identity, false).expect("save");

        let other = Identity::generate("someone else").expect("generate");
        fs::write(store.public_key_path(), other.to_public_line()).expect("overwrite");

        assert!(matches!(
            store.load_identity().unwrap_err(),
            StoreError::PublicKeyMismatch { .. }
        ));
    }

    #[test]
    fn tolerates_a_missing_public_key_file() {
        let (_tmp, store) = test_store();
        let identity = Identity::generate("laptop").expect("generate");
        store.save_identity(&identity, false).expect("save");
        fs::remove_file(store.public_key_path()).expect("remove");

        let loaded = store.load_identity().expect("load");
        assert_eq!(loaded.fingerprint(), identity.fingerprint());
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_not_readable_by_other_users() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_tmp, store) = test_store();
        let identity = Identity::generate("laptop").expect("generate");
        store.save_identity(&identity, false).expect("save");

        for path in [store.private_key_path(), store.known_peers_path()] {
            let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode & 0o077, 0, "{} has mode {mode:04o}", path.display());
        }
        assert!(store.permission_warnings().is_empty());
    }

    #[test]
    fn a_missing_known_peers_file_is_an_empty_database() {
        let (_tmp, store) = test_store();
        assert_eq!(store.load_known_peers().expect("load").len(), 0);
    }

    #[test]
    fn reports_a_corrupt_known_peers_file_with_its_path_and_line() {
        let (_tmp, store) = test_store();
        store.ensure_dirs().expect("ensure dirs");
        fs::write(
            store.known_peers_path(),
            format!("{PEERS_HEADER}alice {KEY_TYPE} not-base64!\n"),
        )
        .expect("write");

        let err = store.load_known_peers().unwrap_err();
        let StoreError::KnownPeers { path, source } = &err else {
            panic!("unexpected error: {err}");
        };
        assert_eq!(path, &store.known_peers_path());
        assert_eq!(source.line, 3);
    }

    #[test]
    fn known_peers_round_trips_with_lf_endings_and_a_header() {
        let (_tmp, store) = test_store();
        let mut known = store.load_known_peers().expect("load");
        known
            .add(Peer::new("alice", verifying_key("alpha")))
            .expect("add");
        store.save_known_peers(&known).expect("save");

        let raw = fs::read_to_string(store.known_peers_path()).expect("read");
        assert!(!raw.contains("\r\n"), "written with CRLF endings");
        assert!(raw.starts_with("# beam known_peers v1"), "{raw}");

        assert!(
            store
                .load_known_peers()
                .expect("reload")
                .lookup("alice")
                .is_some()
        );
    }

    #[test]
    fn atomic_writes_leave_no_temporary_files() {
        let (_tmp, store) = test_store();
        let known = store.load_known_peers().expect("load");
        store.save_known_peers(&known).expect("save");

        for entry in fs::read_dir(store.dir()).expect("read dir") {
            let name = entry.expect("entry").file_name();
            let name = name.to_string_lossy();
            assert!(!name.contains(".tmp"), "left {name} behind");
        }
    }
}
