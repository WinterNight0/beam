//! Renaming and removing a friend, once for every front end: `beam rename`
//! and `beam remove`, the full-screen view and the GUI.
//!
//! A name is only this device's nickname for a key (CLAUDE.md, identity
//! model): renaming never touches the key, and removing forgets the key, so
//! that device can no longer send here until it is paired again. Asking
//! before a removal is the front end's job; the model's `Ask<RemoveFriend>`
//! starts on No.

use crate::identity::{PeerError, Store, StoreError};

/// Why `known_peers` was not changed. Typed, so `beam` keeps its own
/// messages and exit codes; a screen cleans the text before drawing it.
#[derive(Debug, thiserror::Error)]
pub enum FriendError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Peer(#[from] PeerError),
}

/// Gives the friend called `from` the name `to` on this device.
pub fn rename(store: &Store, from: &str, to: &str) -> Result<(), FriendError> {
    let mut known = store.load_known_peers()?;
    known.rename(from, to)?;
    store.save_known_peers(&known)?;
    Ok(())
}

/// Forgets the friend called `name`, key and all.
pub fn remove(store: &Store, name: &str) -> Result<(), FriendError> {
    let mut known = store.load_known_peers()?;
    known.remove(name)?;
    store.save_known_peers(&known)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Identity, KnownPeers, Peer};

    fn home(names: &[&str]) -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("beam"));
        store.ensure_dirs().unwrap();
        let mut known = KnownPeers::with_header();
        for name in names {
            known
                .add(Peer::new(
                    *name,
                    Identity::generate(name).unwrap().verifying_key(),
                ))
                .unwrap();
        }
        store.save_known_peers(&known).unwrap();
        (tmp, store)
    }

    #[test]
    fn a_rename_keeps_the_key() {
        let (_tmp, store) = home(&["alice"]);
        let key = store
            .load_known_peers()
            .unwrap()
            .lookup("alice")
            .unwrap()
            .public_key;
        rename(&store, "alice", "ali").unwrap();
        let known = store.load_known_peers().unwrap();
        assert!(known.lookup("alice").is_none());
        assert_eq!(known.lookup("ali").unwrap().public_key, key);
    }

    #[test]
    fn a_rename_onto_another_friend_changes_nothing() {
        let (_tmp, store) = home(&["alice", "bob"]);
        let err = rename(&store, "alice", "bob").unwrap_err();
        assert!(
            matches!(err, FriendError::Peer(PeerError::Exists(_))),
            "{err}"
        );
        assert!(store.load_known_peers().unwrap().lookup("alice").is_some());
    }

    #[test]
    fn a_removed_friend_is_forgotten() {
        let (_tmp, store) = home(&["alice", "bob"]);
        remove(&store, "alice").unwrap();
        let known = store.load_known_peers().unwrap();
        assert!(known.lookup("alice").is_none());
        assert!(known.lookup("bob").is_some());
        assert!(matches!(
            remove(&store, "alice"),
            Err(FriendError::Peer(PeerError::NotFound(_)))
        ));
    }
}
