//! Making this device's identity on a first run, for the front ends that
//! offer it without a terminal: the full-screen view's welcome card
//! (ADR-0045) and the GUI (ADR-0049).
//!
//! The same as `beam init` without `--force`: named after the computer, and
//! never replacing an identity that exists. Replacing one would make every
//! friend see a changed key (CLAUDE.md, rule 3), so only `beam init --force`
//! in a terminal can do that.

use crate::identity::{Identity, Store, hostname};
use crate::untrusted;

/// What came of asking for an identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Created {
    /// Made now. Its Short ID, grouped for reading.
    New { short_id: String },
    /// There already was one; it was left alone.
    AlreadyThere,
}

/// Why no identity could be made. The reason is cleaned: it can quote a
/// path or an error from the system.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CreateFailed(pub String);

/// Makes this device's identity, unless it has one.
pub fn create(store: &Store) -> Result<Created, CreateFailed> {
    if store.has_identity() {
        return Ok(Created::AlreadyThere);
    }
    let identity = Identity::generate(&hostname())
        .map_err(|e| CreateFailed(untrusted::text(&e.to_string())))?;
    store
        .save_identity(&identity, false)
        .map_err(|e| CreateFailed(untrusted::text(&e.to_string())))?;
    Ok(Created::New {
        short_id: identity.short_id().grouped(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_run_makes_the_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("beam"));
        let Created::New { short_id } = create(&store).unwrap() else {
            panic!("no identity made");
        };
        let identity = store.load_identity().unwrap();
        assert_eq!(short_id, identity.short_id().grouped());
    }

    /// Rule 3: an existing key is never replaced, so friends never see a
    /// changed key because someone pressed Create twice.
    #[test]
    fn an_existing_identity_is_never_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("beam"));
        create(&store).unwrap();
        let before = store.load_identity().unwrap().fingerprint().hex();
        assert_eq!(create(&store).unwrap(), Created::AlreadyThere);
        assert_eq!(store.load_identity().unwrap().fingerprint().hex(), before);
    }
}
