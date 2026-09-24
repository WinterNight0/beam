//! Deterministic key material for tests.
//!
//! The keys come from fixed seeds, so the fingerprints and short IDs below are
//! stable vectors: a change to the derivation breaks the build instead of
//! silently changing every user's identifier.

use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

pub struct Vector {
    pub name: &'static str,
    pub public_b64: &'static str,
    pub fingerprint_hex: &'static str,
    pub short_id: &'static str,
}

pub const VECTORS: [Vector; 2] = [
    Vector {
        name: "alpha",
        public_b64: "4V1sbBWRwKcMoCdgmMZSy3enESln8Qgij/DzRjafNjs=",
        fingerprint_hex: "14e041bc27b36219678cbb5d9c40c38a41b634bd03c25e1c514d1d95ab8536de",
        short_id: "917470233",
    },
    Vector {
        name: "bravo",
        public_b64: "K0yzICzxhJcR7D2z5/JxBpS2pmFS7rGHZ6/DfXJkpD0=",
        fingerprint_hex: "2cfc94c48f5c67e1b942bddc3127eed3fd109289e17e65df0676299c656005e7",
        short_id: "739613153",
    },
];

/// The signing key behind the named vector.
pub fn signing_key(name: &str) -> SigningKey {
    let digest = Sha256::digest(format!("beam-test-vector-{name}").as_bytes());
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest);
    SigningKey::from_bytes(&seed)
}

/// The public key behind the named vector.
pub fn verifying_key(name: &str) -> VerifyingKey {
    signing_key(name).verifying_key()
}
