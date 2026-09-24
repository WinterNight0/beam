//! This device's Ed25519 keypair and the on-disk encodings for it.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use ed25519_dalek::{PUBLIC_KEY_LENGTH, SECRET_KEY_LENGTH, SigningKey, VerifyingKey};

use super::{Fingerprint, ShortId};

/// The literal written into `known_peers` and `id_ed25519.pub`.
pub const KEY_TYPE: &str = "ed25519";

/// Why a key could not be generated, encoded or decoded.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("could not read entropy from the operating system: {0}")]
    Entropy(#[from] getrandom::Error),
    #[error("encode private key: {0}")]
    Encode(ed25519_dalek::pkcs8::Error),
    #[error("parse private key: {0}")]
    Decode(ed25519_dalek::pkcs8::Error),
    #[error("public key is not valid base64")]
    NotBase64,
    #[error("public key must be {PUBLIC_KEY_LENGTH} bytes, got {0}")]
    KeyLength(usize),
    #[error("public key is not a valid ed25519 point")]
    NotOnCurve,
    #[error("unsupported key type {0:?}, want {KEY_TYPE:?}")]
    UnsupportedKeyType(String),
    #[error("public key file must contain `{KEY_TYPE} <base64 key>`")]
    MalformedPublicLine,
}

/// This device's keypair.
///
/// The private half never leaves the device and is never sent to the signaling
/// server.
#[derive(Clone)]
pub struct Identity {
    signing: SigningKey,
    comment: String,
}

impl Identity {
    /// Creates a fresh keypair from operating-system entropy.
    pub fn generate(comment: &str) -> Result<Self, KeyError> {
        let mut seed = [0u8; SECRET_KEY_LENGTH];
        getrandom::fill(&mut seed)?;
        let signing = SigningKey::from_bytes(&seed);

        // `SigningKey` zeroizes its own copy when dropped. Clear ours too; the
        // `black_box` stops the compiler eliding a write to a dead local. See
        // ADR-0011 for why this is best-effort rather than the `zeroize` crate.
        seed.fill(0);
        std::hint::black_box(&seed);

        Ok(Self {
            signing,
            comment: sanitize_comment(comment),
        })
    }

    /// Wraps an existing signing key.
    pub fn from_signing_key(signing: SigningKey, comment: &str) -> Self {
        Self {
            signing,
            comment: sanitize_comment(comment),
        }
    }

    /// The private key. Never serialise this anywhere but `id_ed25519`.
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    /// The public key.
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// The cosmetic comment stored alongside the public key.
    pub fn comment(&self) -> &str {
        &self.comment
    }

    /// Replaces the cosmetic comment.
    pub fn set_comment(&mut self, comment: &str) {
        self.comment = sanitize_comment(comment);
    }

    /// The SHA-256 fingerprint of the public key.
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.verifying_key())
    }

    /// The 9-digit pairing lookup hint.
    pub fn short_id(&self) -> ShortId {
        self.fingerprint().short_id()
    }

    /// Encodes the private key as PEM-wrapped PKCS#8.
    pub fn to_pkcs8_pem(&self) -> Result<String, KeyError> {
        self.signing
            .to_pkcs8_pem(LineEnding::LF)
            .map(|pem| pem.to_string())
            .map_err(KeyError::Encode)
    }

    /// Decodes a PEM-wrapped PKCS#8 Ed25519 private key.
    pub fn from_pkcs8_pem(pem: &str, comment: &str) -> Result<Self, KeyError> {
        let signing = SigningKey::from_pkcs8_pem(pem).map_err(KeyError::Decode)?;
        Ok(Self::from_signing_key(signing, comment))
    }

    /// Renders the single-line public key file: `ed25519 <base64 key> <comment>`.
    pub fn to_public_line(&self) -> String {
        let mut line = format!("{KEY_TYPE} {}", encode_public_key(&self.verifying_key()));
        if !self.comment.is_empty() {
            line.push(' ');
            line.push_str(&self.comment);
        }
        line.push('\n');
        line
    }
}

/// Never derived: a derived `Debug` would print the private key into whatever
/// log or panic message the value lands in.
impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("fingerprint", &self.fingerprint())
            .field("comment", &self.comment)
            .finish_non_exhaustive()
    }
}

/// Renders a public key as standard padded base64.
pub fn encode_public_key(key: &VerifyingKey) -> String {
    BASE64.encode(key.as_bytes())
}

/// Parses a standard base64 Ed25519 public key.
pub fn decode_public_key(s: &str) -> Result<VerifyingKey, KeyError> {
    let raw = BASE64.decode(s).map_err(|_| KeyError::NotBase64)?;
    let bytes: [u8; PUBLIC_KEY_LENGTH] = raw
        .as_slice()
        .try_into()
        .map_err(|_| KeyError::KeyLength(raw.len()))?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| KeyError::NotOnCurve)
}

/// Parses the single-line public key file, returning the key and its comment.
pub fn parse_public_line(line: &str) -> Result<(VerifyingKey, String), KeyError> {
    let mut fields = line.split_whitespace();
    let key_type = fields.next().ok_or(KeyError::MalformedPublicLine)?;
    if key_type != KEY_TYPE {
        return Err(KeyError::UnsupportedKeyType(key_type.to_string()));
    }
    let encoded = fields.next().ok_or(KeyError::MalformedPublicLine)?;
    let key = decode_public_key(encoded)?;
    let comment = fields.collect::<Vec<_>>().join(" ");
    Ok((key, comment))
}

/// Keeps a comment on one line; it is cosmetic metadata only.
fn sanitize_comment(comment: &str) -> String {
    comment
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vectors::{VECTORS, signing_key, verifying_key};

    #[test]
    fn generate_produces_a_usable_keypair() {
        let identity = Identity::generate("laptop").expect("generate");
        assert_eq!(identity.comment(), "laptop");
        assert_eq!(
            identity.verifying_key(),
            identity.signing_key().verifying_key()
        );
    }

    #[test]
    fn generate_is_random() {
        let a = Identity::generate("").expect("generate");
        let b = Identity::generate("").expect("generate");
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn private_key_round_trips_through_pem() {
        let identity = Identity::from_signing_key(signing_key("alpha"), "laptop");
        let pem = identity.to_pkcs8_pem().expect("encode");
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"), "{pem}");

        let loaded = Identity::from_pkcs8_pem(&pem, "laptop").expect("decode");
        assert_eq!(loaded.signing_key(), identity.signing_key());
        assert_eq!(loaded.fingerprint(), identity.fingerprint());
    }

    #[test]
    fn rejects_garbage_private_keys() {
        let cases = [
            ("empty", ""),
            ("not pem", "just some text"),
            (
                "wrong label",
                "-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----\n",
            ),
            (
                "bad der",
                "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
            ),
        ];
        for (name, pem) in cases {
            assert!(
                Identity::from_pkcs8_pem(pem, "").is_err(),
                "accepted {name}"
            );
        }
    }

    #[test]
    fn public_line_round_trips() {
        let identity = Identity::from_signing_key(signing_key("bravo"), "desktop");
        let line = identity.to_public_line();
        assert_eq!(line, format!("ed25519 {} desktop\n", VECTORS[1].public_b64));

        let (key, comment) = parse_public_line(&line).expect("parse");
        assert_eq!(key, verifying_key("bravo"));
        assert_eq!(comment, "desktop");
    }

    #[test]
    fn comment_cannot_break_the_one_line_format() {
        let identity = Identity::from_signing_key(signing_key("alpha"), "my\nhost\tname ");
        let line = identity.to_public_line();
        assert_eq!(line.matches('\n').count(), 1, "{line:?}");
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn rejects_garbage_public_lines() {
        let cases = [
            ("empty", String::new()),
            ("missing key", "ed25519\n".to_string()),
            ("wrong type", format!("ssh-rsa {}\n", VECTORS[0].public_b64)),
            ("bad base64", "ed25519 not-base64!!\n".to_string()),
            ("short key", "ed25519 AAAA\n".to_string()),
            ("only comment", "comment only\n".to_string()),
        ];
        for (name, line) in cases {
            assert!(parse_public_line(&line).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn rejects_public_keys_of_the_wrong_length() {
        assert!(decode_public_key("AAAAAAAA").is_err());
    }
}
