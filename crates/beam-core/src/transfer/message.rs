//! The messages that cross the wire, and the transfer identifier.
//!
//! Control messages are JSON so they are readable in a packet dump and cheap to
//! evolve; chunk payloads are raw binary. See ADR-0015.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::hex;

/// The unit of hashing and (from M3) of resume. Not the unit of framing.
pub const CHUNK_SIZE: u32 = 4 * 1024 * 1024;

/// Length in bytes of a transfer identifier.
pub const TRANSFER_ID_SIZE: usize = 16;

/// A random per-transfer identifier.
///
/// Randomness is what makes a replayed identifier detectable; see S-11.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId([u8; TRANSFER_ID_SIZE]);

impl TransferId {
    /// Draws a fresh identifier from operating-system entropy.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0u8; TRANSFER_ID_SIZE];
        getrandom::fill(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// Builds an identifier from raw bytes.
    pub fn from_bytes(bytes: [u8; TRANSFER_ID_SIZE]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8; TRANSFER_ID_SIZE] {
        &self.0
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(&self.0))
    }
}

impl fmt::Debug for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TransferId({self})")
    }
}

/// Why a string could not be read as a transfer identifier.
#[derive(Debug, thiserror::Error)]
#[error("invalid transfer id: {0}")]
pub struct ParseTransferIdError(#[from] hex::HexError);

impl FromStr for TransferId {
    type Err = ParseTransferIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0u8; TRANSFER_ID_SIZE];
        hex::decode_into(s, &mut bytes)?;
        Ok(Self(bytes))
    }
}

impl Serialize for TransferId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TransferId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The opening message. Carries everything the receiver needs to decide, and
/// everything that binds the sender to a specific file.
///
/// There is deliberately no sender-supplied nickname here: the Accept prompt
/// shows the name the *receiver* stored in its own `known_peers`, so a sender
/// cannot influence how it is described to the person answering the prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferRequest {
    pub transfer_id: TransferId,
    /// The sender's Ed25519 public key, base64. In M2 this is *claimed*, not
    /// proven; see ADR-0019.
    pub sender_public_key: String,
    pub file_name: String,
    pub size: u64,
    pub chunk_size: u32,
    pub chunk_count: u32,
    /// SHA-256 of the whole file, hex. This is the anchor for end-to-end
    /// integrity, and the value M3 will match a resumed transfer against.
    pub file_sha256: String,
}

/// The receiver agrees to the transfer. Only a human answering the prompt
/// produces this message; see S-1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accept {
    pub transfer_id: TransferId,
    /// Which chunks the receiver already holds. Always `None` in M2; M3 fills
    /// it in for resume. The field exists now so that adding resume does not
    /// change the wire format.
    #[serde(default)]
    pub have_bitmap: Option<String>,
}

/// Why a transfer was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// The person said no.
    Declined,
    /// The sender is not in the receiver's `known_peers` (S-7). No prompt was
    /// shown.
    UnknownPeer,
    /// Nobody answered the prompt in time (S-5).
    Expired,
    /// The request did not make sense: bad file name, inconsistent sizes, a
    /// replayed transfer id.
    BadRequest,
    /// Another beam session on the receiver is already working on this exact
    /// transfer, and two sessions must not write to one partial (ADR-0022).
    Busy,
    /// There is not enough free space to finish, so nobody is asked to agree
    /// to something that cannot succeed.
    NoSpace,
}

impl RejectReason {
    /// A sentence for the sender's terminal.
    pub fn explain(self) -> &'static str {
        match self {
            Self::Declined => "the peer declined the transfer",
            Self::UnknownPeer => "the peer has not paired with you",
            Self::Expired => "the peer did not answer in time",
            Self::BadRequest => "the peer rejected the request as malformed",
            Self::Busy => "the peer is receiving another file; try again later",
            Self::NoSpace => "the peer does not have enough free disk space",
        }
    }
}

/// The receiver refuses the transfer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reject {
    pub transfer_id: TransferId,
    pub reason: RejectReason,
}

/// Announces one chunk: its index, its length, and the hash the receiver must
/// verify before the bytes are written (S-12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkStart {
    pub index: u32,
    pub len: u32,
    /// SHA-256 of this chunk, hex.
    pub sha256: String,
}

/// The receiver stored a chunk successfully.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkAck {
    pub index: u32,
}

/// Why a chunk was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NakReason {
    /// The chunk did not match the hash announced for it.
    HashMismatch,
    /// The chunk did not carry the announced number of bytes.
    LengthMismatch,
}

/// The receiver rejects a chunk and expects it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkNak {
    pub index: u32,
    pub reason: NakReason,
}

/// Sent by the sender to mean "that was the last chunk", and by the receiver to
/// mean "verified and written", in which case `final_name` is the name the file
/// was actually saved under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Complete {
    pub transfer_id: TransferId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_name: Option<String>,
}

/// The largest chunk a request may propose. The receiver holds one chunk in
/// memory while it verifies it, so this bounds what a peer can make it
/// allocate. Four times the default.
pub const MAX_CHUNK_SIZE: u32 = 16 * 1024 * 1024;

/// The most chunks a request may describe. The receiver keeps a bit and a
/// hash per chunk on disk, so this bounds that state — about 700 KB of bitmap
/// at the limit — however small a chunk size a peer proposes. With the
/// default 4 MiB chunks it allows files up to 16 TiB.
pub const MAX_CHUNK_COUNT: u32 = 1 << 22;

/// The receiver is checking the whole file's hash. Sent about once a second
/// while it does, so that a large file's verification is not mistaken for a
/// stalled peer (ADR-0033).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyProgress {
    pub done: u64,
    pub total: u64,
}

/// Either side abandons the transfer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub transfer_id: TransferId,
    pub reason: String,
}

/// One message on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    TransferRequest(TransferRequest),
    Accept(Accept),
    Reject(Reject),
    ChunkStart(ChunkStart),
    /// A slice of one chunk's bytes. A 4 MiB chunk spans many of these.
    ChunkData {
        index: u32,
        bytes: Vec<u8>,
    },
    ChunkAck(ChunkAck),
    ChunkNak(ChunkNak),
    Complete(Complete),
    Cancel(Cancel),
    Verifying(VerifyProgress),
}

impl Message {
    /// A short name for logs and error messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::TransferRequest(_) => "TRANSFER_REQUEST",
            Self::Accept(_) => "ACCEPT",
            Self::Reject(_) => "REJECT",
            Self::ChunkStart(_) => "CHUNK_START",
            Self::ChunkData { .. } => "CHUNK_DATA",
            Self::ChunkAck(_) => "CHUNK_ACK",
            Self::ChunkNak(_) => "CHUNK_NAK",
            Self::Complete(_) => "COMPLETE",
            Self::Cancel(_) => "CANCEL",
            Self::Verifying(_) => "VERIFYING",
        }
    }

    /// Whether this message carries file bytes.
    ///
    /// The receiver uses this to enforce S-4: anything for which this is true,
    /// arriving before it has sent ACCEPT, aborts the transfer.
    pub fn carries_file_data(&self) -> bool {
        matches!(self, Self::ChunkStart(_) | Self::ChunkData { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_id() -> TransferId {
        TransferId::from_bytes([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ])
    }

    #[test]
    fn transfer_id_round_trips_through_hex() {
        let id = sample_id();
        assert_eq!(id.to_string(), "00112233445566778899aabbccddeeff");
        assert_eq!(id.to_string().parse::<TransferId>().unwrap(), id);
    }

    #[test]
    fn transfer_id_rejects_bad_text() {
        for bad in ["", "abc", &"0".repeat(31), &"0".repeat(33), &"z".repeat(32)] {
            assert!(bad.parse::<TransferId>().is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn generated_transfer_ids_differ() {
        let a = TransferId::generate().expect("generate");
        let b = TransferId::generate().expect("generate");
        assert_ne!(a, b);
    }

    #[test]
    fn transfer_request_round_trips_through_json() {
        let request = TransferRequest {
            transfer_id: sample_id(),
            sender_public_key: "4V1sbBWRwKcMoCdgmMZSy3enESln8Qgij/DzRjafNjs=".to_string(),
            file_name: "project.zip".to_string(),
            size: 9_000_000,
            chunk_size: CHUNK_SIZE,
            chunk_count: 3,
            file_sha256: "a".repeat(64),
        };
        let json = serde_json::to_string(&request).expect("serialize");
        let back: TransferRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, request);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // A peer that invents fields is either a newer beam or an attacker
        // probing the parser; either way this version refuses to guess.
        let json =
            r#"{"transfer_id":"00112233445566778899aabbccddeeff","reason":"declined","extra":1}"#;
        assert!(serde_json::from_str::<Reject>(json).is_err());
    }

    #[test]
    fn accept_defaults_to_no_bitmap() {
        let json = r#"{"transfer_id":"00112233445566778899aabbccddeeff"}"#;
        let accept: Accept = serde_json::from_str(json).expect("deserialize");
        assert_eq!(accept.have_bitmap, None);
    }

    #[test]
    fn only_chunk_messages_carry_file_data() {
        let id = sample_id();
        let carrying = [
            Message::ChunkStart(ChunkStart {
                index: 0,
                len: 1,
                sha256: "b".repeat(64),
            }),
            Message::ChunkData {
                index: 0,
                bytes: vec![1],
            },
        ];
        for message in &carrying {
            assert!(message.carries_file_data(), "{}", message.kind_name());
        }

        let not_carrying = [
            Message::Accept(Accept {
                transfer_id: id,
                have_bitmap: None,
            }),
            Message::ChunkAck(ChunkAck { index: 0 }),
            Message::Complete(Complete {
                transfer_id: id,
                final_name: None,
            }),
            Message::Cancel(Cancel {
                transfer_id: id,
                reason: "x".to_string(),
            }),
        ];
        for message in &not_carrying {
            assert!(!message.carries_file_data(), "{}", message.kind_name());
        }
    }
}
