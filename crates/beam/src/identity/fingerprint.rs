//! Fingerprints and short IDs.

use std::fmt;
use std::str::FromStr;

use ed25519_dalek::VerifyingKey;
use sha2::{Digest, Sha256};

use crate::hex;

/// Length in bytes of a peer fingerprint (SHA-256).
pub const FINGERPRINT_SIZE: usize = 32;

/// Bounds a short ID to exactly 9 decimal digits.
const SHORT_ID_MODULUS: u64 = 1_000_000_000;

/// The SHA-256 digest of a raw 32-byte Ed25519 public key.
///
/// This is the canonical, full-strength identifier for a device: users compare
/// it out of band, and it can be compared out of band.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; FINGERPRINT_SIZE]);

impl Fingerprint {
    /// Derives the fingerprint of a public key.
    pub fn of(key: &VerifyingKey) -> Self {
        let digest = Sha256::digest(key.as_bytes());
        let mut bytes = [0u8; FINGERPRINT_SIZE];
        bytes.copy_from_slice(&digest);
        Self(bytes)
    }

    /// The canonical lowercase hex form (64 characters).
    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }

    /// An abbreviated display form for dense output such as tables.
    ///
    /// It is never safe to compare identities by the short form.
    pub fn short(&self) -> String {
        let hex = self.hex();
        format!("SHA256:{}...", &hex[..16])
    }

    /// The 9-digit short ID derived from this fingerprint.
    pub fn short_id(&self) -> ShortId {
        let mut head = [0u8; 8];
        head.copy_from_slice(&self.0[..8]);
        ShortId((u64::from_be_bytes(head) % SHORT_ID_MODULUS) as u32)
    }

    /// The raw digest bytes.
    pub fn as_bytes(&self) -> &[u8; FINGERPRINT_SIZE] {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SHA256:{}", self.hex())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

/// Why a string could not be read as a fingerprint.
#[derive(Debug, thiserror::Error)]
#[error("invalid fingerprint: {0}")]
pub struct ParseFingerprintError(#[from] crate::hex::HexError);

impl FromStr for Fingerprint {
    type Err = ParseFingerprintError;

    /// Accepts the canonical hex form, with or without the `SHA256:` prefix,
    /// ignoring case and any colons used as separators.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        let body = match trimmed.get(..7) {
            Some(prefix) if prefix.eq_ignore_ascii_case("SHA256:") => &trimmed[7..],
            _ => trimmed,
        };
        let digits: String = body.chars().filter(|c| *c != ':').collect();

        let mut bytes = [0u8; FINGERPRINT_SIZE];
        hex::decode_into(&digits, &mut bytes)?;
        Ok(Self(bytes))
    }
}

/// A 9-digit lookup hint derived from a fingerprint.
///
/// It exists only so a human can read an identifier aloud for the very first
/// pairing; it is a routing hint, **not** a security guarantee. Security comes
/// from the future authenticated transport.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct ShortId(u32);

impl ShortId {
    /// The numeric value, always below 10^9.
    pub fn value(self) -> u32 {
        self.0
    }

    /// Groups the digits for readability, e.g. `004 815 162`.
    pub fn grouped(self) -> String {
        let d = self.to_string();
        format!("{} {} {}", &d[0..3], &d[3..6], &d[6..9])
    }
}

impl fmt::Display for ShortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:09}", self.0)
    }
}

/// Why a string could not be read as a short ID.
#[derive(Debug, thiserror::Error)]
pub enum ParseShortIdError {
    #[error("short ID contains invalid character {0:?}")]
    InvalidCharacter(char),
    #[error("short ID must have 9 digits, got {0}")]
    Length(usize),
}

impl FromStr for ShortId {
    type Err = ParseShortIdError;

    /// Accepts 9 digits with optional spaces or dashes between them.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut digits = String::with_capacity(9);
        for c in s.chars() {
            match c {
                '0'..='9' => digits.push(c),
                ' ' | '-' | '\t' => {} // separators are cosmetic
                other => return Err(ParseShortIdError::InvalidCharacter(other)),
            }
        }
        if digits.len() != 9 {
            return Err(ParseShortIdError::Length(digits.len()));
        }
        let value: u64 = digits.parse().expect("9 ASCII digits always parse");
        Ok(ShortId(value as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::keys::encode_public_key;
    use crate::identity::vectors::{VECTORS, verifying_key};

    #[test]
    fn matches_the_fixed_vectors() {
        for vector in &VECTORS {
            let key = verifying_key(vector.name);
            assert_eq!(
                encode_public_key(&key),
                vector.public_b64,
                "{}",
                vector.name
            );

            let fp = Fingerprint::of(&key);
            assert_eq!(fp.hex(), vector.fingerprint_hex, "{}", vector.name);
            assert_eq!(fp.to_string(), format!("SHA256:{}", vector.fingerprint_hex));
            assert_eq!(
                fp.short(),
                format!("SHA256:{}...", &vector.fingerprint_hex[..16])
            );
            assert_eq!(
                fp.short_id().to_string(),
                vector.short_id,
                "{}",
                vector.name
            );
        }
    }

    #[test]
    fn short_id_is_always_nine_digits() {
        // Both extremes of the fingerprint space must land inside 9 digits.
        let mut high = [0u8; FINGERPRINT_SIZE];
        high[..8].fill(0xff);
        for fp in [Fingerprint([0u8; FINGERPRINT_SIZE]), Fingerprint(high)] {
            let id = fp.short_id();
            assert!(u64::from(id.value()) < SHORT_ID_MODULUS);
            assert_eq!(id.to_string().len(), 9);
        }
    }

    #[test]
    fn short_id_groups_digits() {
        assert_eq!(
            "004815162".parse::<ShortId>().unwrap().grouped(),
            "004 815 162"
        );
    }

    #[test]
    fn parses_short_ids() {
        for form in ["004815162", "004 815 162", "004-815-162"] {
            assert_eq!(
                form.parse::<ShortId>().unwrap().value(),
                4_815_162,
                "{form}"
            );
        }
        for bad in ["", "12345678", "1234567890", "12345678a", "004.815.162"] {
            assert!(bad.parse::<ShortId>().is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn parses_fingerprints() {
        let hex = VECTORS[0].fingerprint_hex;
        let colons = hex
            .as_bytes()
            .chunks(2)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join(":");
        for form in [
            hex.to_string(),
            format!("SHA256:{hex}"),
            format!("sha256:{hex}"),
            hex.to_uppercase(),
            colons,
        ] {
            assert_eq!(form.parse::<Fingerprint>().unwrap().hex(), hex, "{form}");
        }
        for bad in ["", "abc", &format!("{hex}00"), &format!("zz{}", &hex[2..])] {
            assert!(bad.parse::<Fingerprint>().is_err(), "accepted {bad:?}");
        }
    }
}
