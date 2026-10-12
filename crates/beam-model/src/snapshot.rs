//! What `~/.beam` holds, read once per frame and made safe to show: this
//! device, the friends and when each was last seen, whether a receiver
//! runs, and what came and went (ADR-0043).
//!
//! Every string is already cleaned for a screen (ADR-0034): `known_peers`
//! and `history.jsonl` can be edited by hand.

use beam_core::agent::status::Running;
use beam_core::identity::{Store, StoreError};
use beam_core::untrusted;

/// This device, for the header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Me {
    pub name: String,
    pub short_id: String,
    pub fingerprint: String,
}

/// One paired peer, ready to draw. Every string is already safe for the
/// terminal (ADR-0034): `known_peers` can be edited by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Friend {
    /// Also the key for rename and remove: `known_peers` only holds names
    /// that pass `validate_name`, which cleaning leaves unchanged.
    pub name: String,
    pub short_id: String,
    /// The fingerprint's hex, without the `SHA256:` prefix.
    pub fingerprint: String,
    pub added: String,
    /// When a transfer with them last got an answer (unix seconds): what
    /// "last seen" means without a server (ADR-0043).
    pub last_seen: Option<u64>,
}

/// Whether the background agent is running (ADR-0042).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Agent {
    Running { receive_dir: String },
    Unreadable,
    Stopped,
}

/// Everything read from `~/.beam` for one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// `None` until `beam init` has been run.
    pub me: Option<Me>,
    /// No identity file at all: the first run, which offers to make one.
    pub needs_setup: bool,
    pub friends: Vec<Friend>,
    pub agent: Agent,
    /// What came and went, oldest first, its strings cleaned for drawing.
    pub history: Vec<beam_core::history::Entry>,
    /// Who runs the agent's receiver, when one runs.
    pub owner: Owner,
    /// Whether `beam listen` runs in this beam home.
    pub listen_elsewhere: bool,
    /// Why something could not be read, shown in the status line.
    pub problem: Option<String>,
}

impl Snapshot {
    /// Reads the identity, the paired peers and the agent's status.
    pub fn load(store: &Store) -> Self {
        let mut problem = None;
        let mut needs_setup = false;
        let me = match store.load_identity() {
            Ok(identity) => Some(Me {
                name: untrusted::name(identity.comment()),
                short_id: identity.short_id().grouped(),
                fingerprint: identity.fingerprint().hex(),
            }),
            Err(StoreError::NoIdentity) => {
                needs_setup = true;
                None
            }
            Err(e) => {
                problem = Some(untrusted::text(&e.to_string()));
                None
            }
        };
        let history: Vec<beam_core::history::Entry> = beam_core::history::read(store)
            .into_iter()
            .map(|mut e| {
                e.peer = untrusted::name(&e.peer);
                e.file = untrusted::name(&e.file);
                e.fingerprint = untrusted::text(&e.fingerprint);
                e.note = e.note.as_deref().map(untrusted::text);
                e
            })
            .collect();
        // Seen = they answered: a transfer that only failed to reach them
        // says nothing about when they were there.
        let last_seen = |fingerprint: &str| {
            history
                .iter()
                .rev()
                .find(|e| {
                    e.fingerprint == fingerprint && e.outcome != beam_core::history::Outcome::Failed
                })
                .map(|e| e.at)
        };
        let friends = match store.load_known_peers() {
            Ok(known) => known
                .peers()
                .iter()
                .map(|p| {
                    let fingerprint = p.fingerprint().hex();
                    Friend {
                        name: untrusted::name(&p.name),
                        short_id: p.short_id().grouped(),
                        last_seen: last_seen(&fingerprint),
                        fingerprint,
                        added: p.added_date(),
                    }
                })
                .collect(),
            Err(e) => {
                problem.get_or_insert_with(|| untrusted::text(&e.to_string()));
                Vec::new()
            }
        };
        let mut owner = Owner::Background;
        let agent = match beam_core::agent::status::read(store) {
            Running::Yes(status) => {
                if status.in_view {
                    owner = if status.pid == std::process::id() {
                        Owner::ThisView
                    } else {
                        Owner::OtherView
                    };
                }
                Agent::Running {
                    receive_dir: untrusted::name(&status.receive_dir),
                }
            }
            Running::Unreadable => Agent::Unreadable,
            Running::No => Agent::Stopped,
        };
        let listen_elsewhere = !matches!(
            beam_core::listen_status::read(store),
            beam_core::listen_status::Listening::No
        );
        Self {
            me,
            needs_setup,
            friends,
            agent,
            history,
            owner,
            listen_elsewhere,
            problem,
        }
    }
}

/// Who runs the receiver the Pending tab talks to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Owner {
    /// The background agent (`beam service start`).
    #[default]
    Background,
    /// This view's Receiving switch.
    ThisView,
    /// Another beam view's switch, in another terminal.
    OtherView,
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_core::history::{self, Direction, Entry, Outcome};
    use beam_core::identity::{Identity, KnownPeers, Peer};

    fn entry(at: u64, fingerprint: &str, outcome: Outcome) -> Entry {
        Entry {
            at,
            direction: Direction::Sent,
            peer: "alice".into(),
            fingerprint: fingerprint.into(),
            file: "a.txt".into(),
            size: 1,
            outcome,
            note: None,
        }
    }

    /// "Last seen" is the last time they answered: a send that never
    /// reached them says nothing about when they were there (ADR-0043).
    #[test]
    fn last_seen_counts_answers_not_attempts_that_never_arrived() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("beam"));
        store.ensure_dirs().unwrap();
        store
            .save_identity(&Identity::generate("me").unwrap(), false)
            .unwrap();
        let alice = Identity::generate("alice").unwrap();
        let mut known = KnownPeers::with_header();
        known
            .add(Peer::new("alice", alice.verifying_key()))
            .unwrap();
        store.save_known_peers(&known).unwrap();
        let fp = alice.fingerprint().hex();
        history::record(&store, &entry(100, &fp, Outcome::Declined));
        history::record(&store, &entry(200, &fp, Outcome::Failed));

        let snapshot = Snapshot::load(&store);
        assert!(snapshot.me.is_some() && !snapshot.needs_setup);
        assert_eq!(snapshot.friends.len(), 1);
        assert_eq!(snapshot.friends[0].last_seen, Some(100));
    }

    /// No identity yet is the first run, not an error.
    #[test]
    fn no_identity_is_the_first_run() {
        let tmp = tempfile::tempdir().unwrap();
        let snapshot = Snapshot::load(&Store::new(tmp.path().join("beam")));
        assert!(snapshot.needs_setup);
        assert!(snapshot.me.is_none());
        assert_eq!(snapshot.problem, None);
    }
}
