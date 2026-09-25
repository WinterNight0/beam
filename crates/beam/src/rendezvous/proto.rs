//! The rendezvous wire format, and the checks both ends make.
//!
//! JSON text frames over a WebSocket. There are two requests:
//!
//! * `register` — "Short ID *s* is reachable at *addr*", signed by the device
//!   key. The server checks the signature, the timestamp, and that the key
//!   really derives *s*. So nobody can announce a Short ID without holding a
//!   private key whose fingerprint produces it.
//! * `lookup` — "where is Short ID *s*?" The answer is every live entry for
//!   *s*, and the client checks each one again itself: the server is a
//!   convenience, not an authority.
//!
//! A Short ID is only 30 bits, so a determined attacker *can* grind a key that
//! lands on someone else's Short ID and register it. That is expected and is
//! harmless for pairing — see ADR-0027 — because the lookup returns both
//! entries and only the real device knows the pairing code.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use iroh::{EndpointAddr, TransportAddr};
use serde::{Deserialize, Serialize};

use crate::identity::{Fingerprint, Identity, ShortId, decode_public_key, encode_public_key};
use crate::transport::endpoint::endpoint_id;

/// The version inside a signed registration.
pub const PROTOCOL_VERSION: u8 = 1;

/// The WebSocket path the server answers on.
pub const PATH: &str = "/v1";

/// How far a registration's timestamp may be from the server's clock.
pub const MAX_CLOCK_SKEW: Duration = Duration::from_secs(60);

/// Largest message either side accepts.
pub const MAX_MESSAGE: usize = 16 * 1024;

/// Most addresses one registration may carry.
pub const MAX_ADDRS: usize = 16;

/// Domain separation: a registration signature can never be mistaken for a
/// signature over anything else beam's key signs.
const SIGN_LABEL: &[u8] = b"beam-rendezvous-register-v1\0";

/// A request from a beam client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    /// `body` is the JSON of a [`RegisterBody`], kept as the exact text that
    /// was signed so no re-serialisation can change what is verified.
    Register {
        body: String,
        signature: String,
    },
    Lookup {
        short_id: String,
    },
}

/// A reply from the server.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerMessage {
    Registered {
        ttl_secs: u64,
    },
    Found {
        short_id: String,
        peers: Vec<PeerRecord>,
    },
    Error {
        code: String,
        message: String,
    },
}

/// What a device signs to announce itself.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegisterBody {
    pub version: u8,
    pub short_id: String,
    pub public_key: String,
    /// Unix seconds.
    pub timestamp: i64,
    pub addr: EndpointAddr,
}

/// One entry in a lookup answer.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PeerRecord {
    pub public_key: String,
    pub addr: EndpointAddr,
}

/// A registration whose signature and claims have been checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub short_id: ShortId,
    pub public_key: VerifyingKey,
    pub timestamp: i64,
    pub addr: EndpointAddr,
}

/// Why a registration was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegisterError {
    #[error("malformed registration: {0}")]
    Malformed(String),
    #[error("unsupported registration version {0}")]
    Version(u8),
    #[error("the signature does not verify under the registered public key")]
    BadSignature,
    #[error("the timestamp is {0} seconds away from the server's clock")]
    Stale(i64),
    #[error("the public key does not derive the claimed Short ID")]
    ShortIdMismatch,
    #[error("the address is for a different endpoint than the public key")]
    AddressMismatch,
    #[error("too many addresses; the limit is {MAX_ADDRS}")]
    TooManyAddresses,
    #[error("the timestamp is not newer than the last registration for this key")]
    Replay,
    #[error("this Short ID already has the maximum number of registrations")]
    Full,
}

impl RegisterError {
    /// The stable code sent in an `error` reply.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "malformed",
            Self::Version(_) => "version",
            Self::BadSignature => "bad_signature",
            Self::Stale(_) => "stale_timestamp",
            Self::ShortIdMismatch => "short_id_mismatch",
            Self::AddressMismatch => "address_mismatch",
            Self::TooManyAddresses => "too_many_addresses",
            Self::Replay => "replay",
            Self::Full => "full",
        }
    }
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn signed_bytes(body: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(SIGN_LABEL.len() + body.len());
    bytes.extend_from_slice(SIGN_LABEL);
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

/// Builds a signed `register` request.
pub fn sign_registration(
    identity: &Identity,
    addr: &EndpointAddr,
    timestamp: i64,
) -> ClientMessage {
    let body = RegisterBody {
        version: PROTOCOL_VERSION,
        short_id: identity.short_id().to_string(),
        public_key: encode_public_key(&identity.verifying_key()),
        timestamp,
        addr: addr.clone(),
    };
    let body = serde_json::to_string(&body).expect("a registration always serialises");
    let signature = identity.signing_key().sign(&signed_bytes(&body));
    ClientMessage::Register {
        body,
        signature: BASE64.encode(signature.to_bytes()),
    }
}

/// Checks a `register` request at time `now` (Unix seconds).
///
/// The order matters only for which error is reported: the signature is
/// checked before anything the signature is supposed to vouch for.
pub fn verify_registration(
    body: &str,
    signature: &str,
    now: i64,
) -> Result<Registration, RegisterError> {
    let parsed: RegisterBody =
        serde_json::from_str(body).map_err(|e| RegisterError::Malformed(e.to_string()))?;
    if parsed.version != PROTOCOL_VERSION {
        return Err(RegisterError::Version(parsed.version));
    }
    let public_key = decode_public_key(&parsed.public_key)
        .map_err(|e| RegisterError::Malformed(format!("public key: {e}")))?;

    let signature = BASE64
        .decode(signature)
        .ok()
        .and_then(|bytes| <[u8; 64]>::try_from(bytes.as_slice()).ok())
        .map(|bytes| Signature::from_bytes(&bytes))
        .ok_or(RegisterError::BadSignature)?;
    public_key
        .verify_strict(&signed_bytes(body), &signature)
        .map_err(|_| RegisterError::BadSignature)?;

    let skew = now - parsed.timestamp;
    if skew.unsigned_abs() > MAX_CLOCK_SKEW.as_secs() {
        return Err(RegisterError::Stale(skew));
    }

    let short_id: ShortId = parsed
        .short_id
        .parse()
        .map_err(|e| RegisterError::Malformed(format!("short id: {e}")))?;
    if Fingerprint::of(&public_key).short_id() != short_id {
        return Err(RegisterError::ShortIdMismatch);
    }
    if parsed.addr.id != endpoint_id(&public_key) {
        return Err(RegisterError::AddressMismatch);
    }
    if parsed.addr.addrs.len() > MAX_ADDRS {
        return Err(RegisterError::TooManyAddresses);
    }
    if parsed
        .addr
        .addrs
        .iter()
        .any(|a| !matches!(a, TransportAddr::Ip(_) | TransportAddr::Relay(_)))
    {
        return Err(RegisterError::Malformed("unsupported address kind".into()));
    }

    Ok(Registration {
        short_id,
        public_key,
        timestamp: parsed.timestamp,
        addr: parsed.addr,
    })
}

/// The client's own check of a lookup answer: an entry is used only if its
/// key derives the Short ID that was asked for and its address is that key's
/// endpoint. A server that lies gets its entries dropped here.
pub fn verify_record(
    requested: ShortId,
    record: &PeerRecord,
) -> Option<(VerifyingKey, EndpointAddr)> {
    let key = decode_public_key(&record.public_key).ok()?;
    if Fingerprint::of(&key).short_id() != requested {
        return None;
    }
    if record.addr.id != endpoint_id(&key) {
        return None;
    }
    Some((key, record.addr.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vectors;

    const NOW: i64 = 1_790_000_000;

    fn alpha() -> Identity {
        vectors::identity("alpha")
    }

    fn addr_of(identity: &Identity) -> EndpointAddr {
        EndpointAddr::new(endpoint_id(&identity.verifying_key()))
            .with_ip_addr("127.0.0.1:4433".parse().unwrap())
    }

    fn parts(message: ClientMessage) -> (String, String) {
        match message {
            ClientMessage::Register { body, signature } => (body, signature),
            other => panic!("{other:?}"),
        }
    }

    /// Re-signs an edited body with `identity`, to test the checks that come
    /// after the signature.
    fn resign(identity: &Identity, body: &RegisterBody) -> (String, String) {
        let body = serde_json::to_string(body).unwrap();
        let signature = identity.signing_key().sign(&signed_bytes(&body));
        (body, BASE64.encode(signature.to_bytes()))
    }

    fn body_of(text: &str) -> RegisterBody {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_signed_registration_verifies() {
        let identity = alpha();
        let (body, sig) = parts(sign_registration(&identity, &addr_of(&identity), NOW));
        let reg = verify_registration(&body, &sig, NOW).unwrap();
        assert_eq!(reg.short_id, identity.short_id());
        assert_eq!(reg.public_key, identity.verifying_key());
        assert_eq!(reg.addr, addr_of(&identity));
    }

    #[test]
    fn a_tampered_body_fails_the_signature() {
        let identity = alpha();
        let (body, sig) = parts(sign_registration(&identity, &addr_of(&identity), NOW));
        let tampered = body.replace("127.0.0.1:4433", "10.6.6.6:4433");
        assert_ne!(tampered, body);
        assert_eq!(
            verify_registration(&tampered, &sig, NOW),
            Err(RegisterError::BadSignature)
        );
    }

    #[test]
    fn a_signature_by_another_key_fails() {
        // bravo signs a body that claims to be alpha.
        let identity = alpha();
        let (body, _) = parts(sign_registration(&identity, &addr_of(&identity), NOW));
        let (_, sig) = resign(&vectors::identity("bravo"), &body_of(&body));
        assert_eq!(
            verify_registration(&body, &sig, NOW),
            Err(RegisterError::BadSignature)
        );
    }

    #[test]
    fn garbage_signatures_fail() {
        let identity = alpha();
        let (body, _) = parts(sign_registration(&identity, &addr_of(&identity), NOW));
        for sig in [
            "",
            "not base64!",
            &BASE64.encode([0u8; 63]),
            &BASE64.encode([0u8; 64]),
        ] {
            assert_eq!(
                verify_registration(&body, sig, NOW),
                Err(RegisterError::BadSignature),
                "{sig:?}"
            );
        }
    }

    #[test]
    fn stale_and_future_timestamps_are_refused() {
        let identity = alpha();
        let skew = MAX_CLOCK_SKEW.as_secs() as i64;
        for (signed_at, ok) in [
            (NOW, true),
            (NOW - skew, true),
            (NOW + skew, true),
            (NOW - skew - 1, false),
            (NOW + skew + 1, false),
            (NOW - 86_400, false),
        ] {
            let (body, sig) = parts(sign_registration(&identity, &addr_of(&identity), signed_at));
            let result = verify_registration(&body, &sig, NOW);
            assert_eq!(
                result.is_ok(),
                ok,
                "signed at NOW{:+}: {result:?}",
                signed_at - NOW
            );
            if !ok {
                assert!(matches!(result, Err(RegisterError::Stale(_))));
            }
        }
    }

    #[test]
    fn a_short_id_the_key_does_not_derive_is_refused() {
        // Correctly signed by alpha, but claiming bravo's Short ID.
        let identity = alpha();
        let (body, _) = parts(sign_registration(&identity, &addr_of(&identity), NOW));
        let mut edited = body_of(&body);
        edited.short_id = vectors::identity("bravo").short_id().to_string();
        let (body, sig) = resign(&identity, &edited);
        assert_eq!(
            verify_registration(&body, &sig, NOW),
            Err(RegisterError::ShortIdMismatch)
        );
    }

    #[test]
    fn an_address_for_another_endpoint_is_refused() {
        let identity = alpha();
        let bravo = vectors::identity("bravo");
        let (body, sig) = parts(sign_registration(&identity, &addr_of(&bravo), NOW));
        assert_eq!(
            verify_registration(&body, &sig, NOW),
            Err(RegisterError::AddressMismatch)
        );
    }

    #[test]
    fn too_many_addresses_are_refused() {
        let identity = alpha();
        let mut addr = EndpointAddr::new(endpoint_id(&identity.verifying_key()));
        for port in 0..=MAX_ADDRS as u16 {
            addr = addr.with_ip_addr(std::net::SocketAddr::from(([127, 0, 0, 1], 1000 + port)));
        }
        let (body, sig) = parts(sign_registration(&identity, &addr, NOW));
        assert_eq!(
            verify_registration(&body, &sig, NOW),
            Err(RegisterError::TooManyAddresses)
        );
    }

    #[test]
    fn unknown_fields_in_the_body_are_refused() {
        let identity = alpha();
        let (body, _) = parts(sign_registration(&identity, &addr_of(&identity), NOW));
        let body = body.replacen('{', r#"{"admin":true,"#, 1);
        let signature = identity.signing_key().sign(&signed_bytes(&body));
        let result = verify_registration(&body, &BASE64.encode(signature.to_bytes()), NOW);
        assert!(
            matches!(result, Err(RegisterError::Malformed(_))),
            "{result:?}"
        );
    }

    #[test]
    fn the_signature_is_domain_separated() {
        // A signature over the bare body, without the label, must not verify.
        let identity = alpha();
        let (body, _) = parts(sign_registration(&identity, &addr_of(&identity), NOW));
        let bare = identity.signing_key().sign(body.as_bytes());
        assert_eq!(
            verify_registration(&body, &BASE64.encode(bare.to_bytes()), NOW),
            Err(RegisterError::BadSignature)
        );
    }

    #[test]
    fn the_client_drops_lookup_entries_that_do_not_check_out() {
        let identity = alpha();
        let requested = identity.short_id();
        let good = PeerRecord {
            public_key: encode_public_key(&identity.verifying_key()),
            addr: addr_of(&identity),
        };
        assert!(verify_record(requested, &good).is_some());

        // A key that does not derive the Short ID that was asked for.
        let bravo = vectors::identity("bravo");
        let wrong_key = PeerRecord {
            public_key: encode_public_key(&bravo.verifying_key()),
            addr: addr_of(&bravo),
        };
        assert!(verify_record(requested, &wrong_key).is_none());

        // The right key, pointing at someone else's endpoint.
        let wrong_addr = PeerRecord {
            public_key: good.public_key.clone(),
            addr: addr_of(&bravo),
        };
        assert!(verify_record(requested, &wrong_addr).is_none());
    }

    #[test]
    fn messages_round_trip_as_tagged_json() {
        let lookup = ClientMessage::Lookup {
            short_id: "123456789".into(),
        };
        let text = serde_json::to_string(&lookup).unwrap();
        assert_eq!(text, r#"{"type":"lookup","short_id":"123456789"}"#);
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&text).unwrap(),
            lookup
        );
    }
}
