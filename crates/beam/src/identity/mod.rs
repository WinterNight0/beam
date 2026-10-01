//! Device identity: keys, fingerprints, short IDs and the `known_peers` store.

mod fingerprint;
mod keys;
mod known_peers;
mod store;

#[cfg(test)]
pub(crate) mod vectors;

pub use fingerprint::{
    FINGERPRINT_SIZE, Fingerprint, ParseFingerprintError, ParseShortIdError, ShortId,
};
pub use keys::{
    Identity, KEY_TYPE, KeyError, decode_public_key, encode_public_key, parse_public_line,
};
pub use known_peers::{
    Attr, HEADER, KnownPeers, ParseError, ParseErrorKind, Peer, PeerError, validate_name,
};
pub use store::{
    CONFIG_NAME, DIR_ENV, DIR_NAME, KNOWN_PEERS_NAME, LISTEN_LOCK_NAME, LISTEN_STATUS_NAME,
    PRIVATE_KEY_NAME, PUBLIC_KEY_NAME, Store, StoreError, TMP_NAME,
};
