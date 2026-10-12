//! Pairing with a new friend, and updating where an existing friend is
//! found: what `beam pair` and the view's Add friend do, written once.
//!
//! The protocol itself is [`pairing::join`](crate::pairing::join) and
//! [`wait`](crate::pairing::wait), unchanged: the code is typed before
//! connecting, both people compare fingerprints, the key saved is the one
//! the connection proved, and a question left unanswered is a no. The front
//! end supplies the questions: the code ([`pair`]'s `read_code`) and the
//! fingerprint check (a [`Confirm`]).
//!
//! An invite for a friend already paired never pairs again and never
//! changes a key (rule 3): it only updates where to find them, and a
//! different relay needs a yes first (ADR-0038). [`already_paired`] says
//! what would change; [`save_location`] saves it.

use std::future::Future;

use crate::config::{Config, Relay};
use crate::identity::{Fingerprint, Peer, PeerError, Store, StoreError};
use crate::invite::{self, Invite};
use crate::pairing::{
    CodeSlot, Confirm, Event, Network, PairError, Pairing, PairingCode, Timeouts, join, wait,
};
use crate::transport::endpoint::Bind;

/// Whom to pair with.
#[derive(Clone, Copy, Debug)]
pub struct PairRequest<'a> {
    /// Join the device in this invite; `None` to wait for someone to join.
    pub invite: Option<&'a Invite>,
    /// What to call the new friend; `None` to take their device's hint.
    pub name: Option<&'a str>,
    /// `Loopback` only in tests.
    pub bind: Bind,
}

/// A friend just paired and saved.
#[derive(Clone, Debug)]
pub struct NewFriend {
    pub name: String,
    pub fingerprint: Fingerprint,
}

/// Why pairing, or saving where a friend is, did not happen.
#[derive(Debug, thiserror::Error)]
pub enum PairFailure {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Peer(#[from] PeerError),
    #[error("{0}")]
    Config(String),
    /// The pairing itself failed; nothing was saved.
    #[error(transparent)]
    NotPaired(PairError),
    /// The friend was removed while the question was open.
    #[error("that device is no longer paired")]
    NoLongerPaired,
    /// A relay change was about to be saved without the person's yes.
    #[error("the invite moves that device to a different relay, which needs a yes first")]
    RelayChangeNotAgreed,
}

/// The network as `config.toml` describes it.
pub fn network(store: &Store, bind: Bind) -> Result<Network, PairFailure> {
    let config =
        Config::load(&store.config_path()).map_err(|e| PairFailure::Config(e.to_string()))?;
    Ok(Network {
        relay: config.relay,
        bind,
        port: config.port,
        advertise: config.advertise,
    })
}

/// Pairs, until it ends or `stop` completes, and saves the new friend only
/// if both people confirmed.
pub async fn pair<C, R, Fut>(
    store: &Store,
    request: PairRequest<'_>,
    confirm: C,
    read_code: R,
    events: impl FnMut(Event<'_>),
    stop: impl Future<Output = ()>,
) -> Result<NewFriend, PairFailure>
where
    C: Confirm + Clone,
    R: FnOnce() -> Fut,
    Fut: Future<Output = std::io::Result<PairingCode>>,
{
    let identity = store.load_identity()?;
    let known = store.load_known_peers()?;
    let network = network(store, request.bind)?;
    let timeouts = Timeouts::default();
    let pairing = Pairing {
        identity: &identity,
        known: &known,
        name: request.name,
        network: &network,
        timeouts,
    };
    let run = async {
        match request.invite {
            None => {
                let code =
                    PairingCode::generate().map_err(|e| PairError::Io(std::io::Error::other(e)))?;
                wait(
                    &pairing,
                    CodeSlot::new(code, timeouts.code_ttl),
                    confirm,
                    events,
                )
                .await
            }
            Some(invite) => join(&pairing, invite, read_code, confirm, events).await,
        }
    };
    let paired = tokio::select! {
        result = run => result,
        () = stop => Err(PairError::Io(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        ))),
    }
    .map_err(PairFailure::NotPaired)?;

    // Read the file again: it may have changed while the question was up.
    let mut known = store.load_known_peers()?;
    let mut peer = Peer::new(paired.name.clone(), paired.key);
    if let Some(invite) = request.invite {
        invite::remember(&mut peer, invite, &network.relay);
    }
    let fingerprint = peer.fingerprint();
    known.add(peer)?;
    store.save_known_peers(&known)?;
    Ok(NewFriend {
        name: paired.name,
        fingerprint,
    })
}

/// What an invite for an already-paired friend would change.
#[derive(Clone, Debug)]
pub struct Location {
    /// The friend's name as stored (not cleaned: it is ours).
    pub name: String,
    pub fingerprint: Fingerprint,
    /// The relay saved for the friend now, and after; `None` means "this
    /// device's own relay".
    pub relay_now: Option<String>,
    pub relay_after: Option<String>,
    /// This device's own relay, for describing `None`.
    pub own_relay: Relay,
}

impl Location {
    /// A different relay is the one change worth a question: whoever runs a
    /// relay sees when this device sends to that friend, and from where, and
    /// can block it. A key is public, so anyone can make an invite that
    /// names it with their own relay (ADR-0038). Addresses alone are saved
    /// without asking: the relay still reaches the friend.
    pub fn changes_relay(&self) -> bool {
        self.relay_now != self.relay_after
    }
}

/// If `invite` is a friend already paired, what saving it would change;
/// `None` if it is someone new.
pub fn already_paired(store: &Store, invite: &Invite) -> Result<Option<Location>, PairFailure> {
    let known = store.load_known_peers()?;
    let Some(peer) = known.lookup_key(&invite.key) else {
        return Ok(None);
    };
    let own_relay = network(store, Bind::Any)?.relay;
    Ok(Some(Location {
        name: peer.name.clone(),
        fingerprint: peer.fingerprint(),
        relay_now: peer.attr(invite::RELAY_ATTR).map(str::to_string),
        relay_after: invite::relay_attr(invite, &own_relay),
        own_relay,
    }))
}

/// Saves where an already-paired friend is found now. Never touches the
/// key. Returns the friend's name as stored.
///
/// A different relay is saved only with `relay_change_agreed`: the person
/// was shown [`Location`] and said yes (ADR-0038). Without it, a relay
/// change is refused and nothing is saved, so a front end that forgot to
/// ask cannot move a friend to someone else's relay.
pub fn save_location(
    store: &Store,
    invite: &Invite,
    relay_change_agreed: bool,
) -> Result<String, PairFailure> {
    let own_relay = network(store, Bind::Any)?.relay;
    let mut known = store.load_known_peers()?;
    let peer = known
        .lookup_key_mut(&invite.key)
        .ok_or(PairFailure::NoLongerPaired)?;
    let changes_relay =
        peer.attr(invite::RELAY_ATTR).map(str::to_string) != invite::relay_attr(invite, &own_relay);
    if changes_relay && !relay_change_agreed {
        return Err(PairFailure::RelayChangeNotAgreed);
    }
    let name = peer.name.clone();
    invite::remember(peer, invite, &own_relay);
    store.save_known_peers(&known)?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    /// A beam home with one friend, alice, on this device's own relay.
    fn home_with_alice() -> (tempfile::TempDir, Store, Identity) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("beam"));
        store.ensure_dirs().unwrap();
        std::fs::write(store.config_path(), "relay = \"none\"\n").unwrap();
        let alice = Identity::generate("alice").unwrap();
        let mut known = crate::identity::KnownPeers::with_header();
        known
            .add(Peer::new("alice", alice.verifying_key()))
            .unwrap();
        store.save_known_peers(&known).unwrap();
        (tmp, store, alice)
    }

    fn invite(key: &Identity, relay: Option<&str>) -> Invite {
        Invite {
            key: key.verifying_key(),
            relay: relay.map(|r| r.parse().unwrap()),
            addrs: vec!["192.0.2.7:7820".parse().unwrap()],
        }
    }

    /// ADR-0038 at the one place every front end saves through: moving a
    /// friend to another relay needs the person's yes.
    #[test]
    fn a_relay_change_is_not_saved_without_a_yes() {
        let (_tmp, store, alice) = home_with_alice();
        let moved = invite(&alice, Some("https://relay.example.org/"));
        let location = already_paired(&store, &moved).unwrap().unwrap();
        assert!(location.changes_relay());

        assert!(matches!(
            save_location(&store, &moved, false),
            Err(PairFailure::RelayChangeNotAgreed)
        ));
        let known = store.load_known_peers().unwrap();
        let peer = known.lookup("alice").unwrap();
        assert_eq!(peer.attr(invite::RELAY_ATTR), None, "nothing was saved");
        assert_eq!(peer.attr(invite::ADDRS_ATTR), None, "nothing was saved");

        assert_eq!(save_location(&store, &moved, true).unwrap(), "alice");
        let known = store.load_known_peers().unwrap();
        assert_eq!(
            known.lookup("alice").unwrap().attr(invite::RELAY_ATTR),
            Some("https://relay.example.org/")
        );
    }

    /// New addresses on the same relay are saved without a question, and
    /// the key never changes (rule 3).
    #[test]
    fn new_addresses_are_saved_and_the_key_is_kept() {
        let (_tmp, store, alice) = home_with_alice();
        let same_relay = invite(&alice, None);
        assert!(
            !already_paired(&store, &same_relay)
                .unwrap()
                .unwrap()
                .changes_relay()
        );
        save_location(&store, &same_relay, false).unwrap();
        let known = store.load_known_peers().unwrap();
        let peer = known.lookup("alice").unwrap();
        assert_eq!(peer.attr(invite::ADDRS_ATTR), Some("192.0.2.7:7820"));
        assert_eq!(peer.public_key, alice.verifying_key());
    }

    /// Someone new is not "already paired".
    #[test]
    fn a_stranger_is_not_already_paired() {
        let (_tmp, store, _alice) = home_with_alice();
        let stranger = Identity::generate("stranger").unwrap();
        assert!(
            already_paired(&store, &invite(&stranger, None))
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            save_location(&store, &invite(&stranger, None), true),
            Err(PairFailure::NoLongerPaired)
        ));
    }
}
