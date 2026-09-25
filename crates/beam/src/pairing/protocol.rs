//! The pairing protocol: SPAKE2, then key confirmation, then two decisions.
//!
//! It runs over any byte stream. In beam that stream is a QUIC stream on an
//! iroh connection, which means the peer's public key is already *proved* by
//! the time the first message is read: `connection.remote_id()` is a key the
//! peer demonstrated it holds. The protocol's job is to prove the other thing —
//! that the proved key belongs to the person who is holding the pairing code.
//!
//! ```text
//! joiner (types the code)                     waiter (shows the code)
//!   Start    {version, short_id, public_key, spake_a}  ──►
//!                                  ◄──  Reply {public_key, spake_b, confirm_W}
//!   Confirm  {confirm_J}                              ──►
//!   Decision {accept}                ◄──►              Decision {accept}
//! ```
//!
//! * **SPAKE2** turns the six-digit code into a strong shared key. An attacker
//!   who does not know the code gets one guess per run and learns nothing
//!   offline.
//! * **Key confirmation**: each side sends
//!   `HMAC-SHA256(k, label ‖ role ‖ short_id ‖ joiner_key ‖ waiter_key)`. The
//!   MAC covers both public keys, the Short ID that was looked up, and which
//!   role is speaking, so a confirmation cannot be reflected back, replayed
//!   into a run with a different key, or moved to a different Short ID.
//! * **Both public keys are the transport's**, not the messages'. A claimed key
//!   that differs from the connection's proved key ends the run. The key that
//!   is returned — and saved to `known_peers` — is the connection's key.
//! * **Two decisions**: a person on each side sees the other side's
//!   fingerprint and answers `[y/N]`. Neither side saves anything unless both
//!   said yes. No answer in time counts as no.
//!
//! See ADR-0026.

use std::future::Future;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::VerifyingKey;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity as SpakeIdentity, Password, Spake2};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

use super::code::PairingCode;
use crate::identity::{ShortId, decode_public_key, encode_public_key};

/// The pairing protocol version this beam speaks.
pub const VERSION: u8 = 1;

/// How long to wait for any one protocol message.
pub const MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a person has to answer `[y/N]`. The same as a transfer's Accept.
pub const DECISION_TIMEOUT: Duration = crate::transfer::DEFAULT_ACCEPT_TIMEOUT;

/// Largest protocol message. The real ones are a few hundred bytes.
const MAX_MESSAGE: usize = 4096;

/// Domain separation for the SPAKE2 password and identities.
const PAKE_LABEL: &str = "beam-pair-v1";

/// Domain separation for the key confirmation MACs.
const CONFIRM_LABEL: &[u8] = b"beam-pair-confirm-v1";

type HmacSha256 = Hmac<Sha256>;

/// Which side of the pairing this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Ran `beam pair <ID>` and typed the code. SPAKE2 side A.
    Joiner,
    /// Ran `beam pair --wait` and shows the code. SPAKE2 side B.
    Waiter,
}

impl Role {
    fn tag(self) -> u8 {
        match self {
            Self::Joiner => b'J',
            Self::Waiter => b'W',
        }
    }
}

/// What one side knows before the run starts.
#[derive(Clone, Debug)]
pub struct Session {
    pub role: Role,
    /// The waiter's Short ID — the one the joiner looked up.
    pub short_id: ShortId,
    /// This device's public key.
    pub local_key: VerifyingKey,
    /// The peer's public key **as proved by the transport**, i.e. the iroh
    /// connection's `remote_id()`. Never a key taken from a message.
    pub remote_key: VerifyingKey,
    pub message_timeout: Duration,
    pub decision_timeout: Duration,
    /// Joiner only: a suggested nickname for this device, sent to the waiter.
    /// `beam listen` has no `--name` to save a new peer under, so it offers
    /// this one (sanitised, and only as a label; ADR-0030). Not part of the
    /// MAC: it names nothing the protocol relies on.
    pub name_hint: Option<String>,
}

/// What `decide` is asked about: a peer that has proved the code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// The key the transport proved.
    pub key: VerifyingKey,
    /// The joiner's suggested name for itself, if it sent one. Only the waiter
    /// ever sees one.
    pub name_hint: Option<String>,
}

/// Longest name hint accepted. Anything longer is ignored, not truncated.
const MAX_NAME_HINT: usize = 64;

impl Session {
    /// A session with the default timeouts.
    pub fn new(
        role: Role,
        short_id: ShortId,
        local_key: VerifyingKey,
        remote_key: VerifyingKey,
    ) -> Self {
        Self {
            role,
            short_id,
            local_key,
            remote_key,
            message_timeout: MESSAGE_TIMEOUT,
            decision_timeout: DECISION_TIMEOUT,
            name_hint: None,
        }
    }

    fn joiner_key(&self) -> &VerifyingKey {
        match self.role {
            Role::Joiner => &self.local_key,
            Role::Waiter => &self.remote_key,
        }
    }

    fn waiter_key(&self) -> &VerifyingKey {
        match self.role {
            Role::Joiner => &self.remote_key,
            Role::Waiter => &self.local_key,
        }
    }
}

/// Why pairing did not complete. In every case, nothing was saved.
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error(
        "the pairing code did not match. If it was typed correctly, someone else may be \
         trying to pair in the other device's place"
    )]
    WrongCode,
    #[error("the other device did not confirm the pairing code; most likely it was typed wrong")]
    NotConfirmed,
    #[error(
        "the other device claimed a public key that is not the one it proved on the \
         connection; someone may be interfering"
    )]
    KeyMismatch,
    #[error(
        "the other device was looking for Short ID {}, not this one",
        crate::untrusted::text(requested)
    )]
    WrongShortId { requested: String },
    #[error("the other device speaks pairing protocol version {0}; this beam speaks {VERSION}")]
    UnsupportedVersion(u8),
    #[error("you did not confirm the pairing")]
    Declined,
    #[error("the other device did not confirm the pairing")]
    DeclinedByPeer,
    #[error("the other device stopped responding")]
    Timeout,
    #[error("the other device closed the connection before pairing finished")]
    Closed,
    #[error(
        "the other device is not accepting pairing right now: {}",
        crate::untrusted::text(.0)
    )]
    Unavailable(String),
    #[error("the other device sent something unexpected: {}", crate::untrusted::text(.0))]
    Protocol(String),
    #[error("pairing failed: {0}")]
    Io(#[from] std::io::Error),
}

/// The protocol messages. JSON, length-prefixed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Message {
    Start {
        version: u8,
        short_id: String,
        public_key: String,
        spake: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name_hint: Option<String>,
    },
    /// The waiter's answer to a Start when it is not taking attempts: cooling
    /// down, switched off, or busy with another attempt. No code is involved.
    Unavailable {
        reason: String,
    },
    Reply {
        public_key: String,
        spake: String,
        confirm: String,
    },
    Confirm {
        confirm: String,
    },
    Decision {
        accept: bool,
    },
}

/// Runs one side of the protocol.
///
/// `decide` is asked — once, and only after the peer has proved it knows the
/// code — whether to pair with the offered key. It is where the pairing
/// prompt goes. It has [`Session::decision_timeout`] to answer; running out of time
/// is a no.
///
/// On success, returns the key to save, which is always
/// [`Session::remote_key`]: the key the transport proved.
pub async fn run<S, F, Fut>(
    stream: &mut S,
    session: &Session,
    code: &PairingCode,
    decide: F,
) -> Result<VerifyingKey, PairingError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce(Offer) -> Fut,
    Fut: Future<Output = std::io::Result<bool>>,
{
    let name_hint = match session.role {
        Role::Joiner => {
            confirm_as_joiner(stream, session, code).await?;
            None
        }
        Role::Waiter => confirm_as_waiter(stream, session, code).await?,
    };

    // The peer knows the code and holds the key the transport proved. Now a
    // person decides.
    let offer = Offer {
        key: session.remote_key,
        name_hint,
    };
    let accept = match tokio::time::timeout(session.decision_timeout, decide(offer)).await {
        Ok(answer) => answer?,
        Err(_) => false,
    };

    send(stream, &Message::Decision { accept }).await?;
    // The peer may still be looking at its own prompt, so allow for that.
    let theirs = recv(stream, session.decision_timeout + session.message_timeout).await?;
    let peer_accepts = match theirs {
        Message::Decision { accept } => accept,
        other => return Err(unexpected("Decision", &other)),
    };

    match (accept, peer_accepts) {
        (false, _) => Err(PairingError::Declined),
        (true, false) => Err(PairingError::DeclinedByPeer),
        (true, true) => Ok(session.remote_key),
    }
}

async fn confirm_as_joiner<S>(
    stream: &mut S,
    session: &Session,
    code: &PairingCode,
) -> Result<(), PairingError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (pake, spake_a) = Spake2::<Ed25519Group>::start_a(
        &password(session.short_id, code),
        &spake_identity(Role::Joiner, session.joiner_key()),
        &spake_identity(Role::Waiter, session.waiter_key()),
    );
    send(
        stream,
        &Message::Start {
            version: VERSION,
            short_id: session.short_id.to_string(),
            public_key: encode_public_key(&session.local_key),
            spake: BASE64.encode(&spake_a),
            name_hint: session.name_hint.clone(),
        },
    )
    .await?;

    let (claimed, spake_b, confirm_w) = match recv(stream, session.message_timeout).await? {
        Message::Reply {
            public_key,
            spake,
            confirm,
        } => (public_key, spake, confirm),
        Message::Unavailable { reason } => return Err(PairingError::Unavailable(reason)),
        other => return Err(unexpected("Reply", &other)),
    };
    check_claim(&claimed, &session.remote_key)?;

    let key = Zeroizing::new(
        pake.finish(&decode_b64(&spake_b)?)
            .map_err(|e| PairingError::Protocol(format!("SPAKE2: {e:?}")))?,
    );
    verify_confirmation(&key, Role::Waiter, session, &confirm_w)?;

    send(
        stream,
        &Message::Confirm {
            confirm: BASE64.encode(confirmation(&key, Role::Joiner, session)),
        },
    )
    .await?;
    Ok(())
}

/// Returns the joiner's name hint, if it sent a usable one.
async fn confirm_as_waiter<S>(
    stream: &mut S,
    session: &Session,
    code: &PairingCode,
) -> Result<Option<String>, PairingError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (version, short_id, claimed, spake_a, name_hint) =
        match recv(stream, session.message_timeout).await? {
            Message::Start {
                version,
                short_id,
                public_key,
                spake,
                name_hint,
            } => (version, short_id, public_key, spake, name_hint),
            other => return Err(unexpected("Start", &other)),
        };
    let name_hint = name_hint.filter(|h| h.len() <= MAX_NAME_HINT);
    if version != VERSION {
        return Err(PairingError::UnsupportedVersion(version));
    }
    if short_id != session.short_id.to_string() {
        return Err(PairingError::WrongShortId {
            requested: short_id,
        });
    }
    check_claim(&claimed, &session.remote_key)?;

    let (pake, spake_b) = Spake2::<Ed25519Group>::start_b(
        &password(session.short_id, code),
        &spake_identity(Role::Joiner, session.joiner_key()),
        &spake_identity(Role::Waiter, session.waiter_key()),
    );
    let key = Zeroizing::new(
        pake.finish(&decode_b64(&spake_a)?)
            .map_err(|e| PairingError::Protocol(format!("SPAKE2: {e:?}")))?,
    );

    send(
        stream,
        &Message::Reply {
            public_key: encode_public_key(&session.local_key),
            spake: BASE64.encode(&spake_b),
            confirm: BASE64.encode(confirmation(&key, Role::Waiter, session)),
        },
    )
    .await?;

    let confirm_j = match recv(stream, session.message_timeout).await {
        Ok(Message::Confirm { confirm }) => confirm,
        Ok(other) => return Err(unexpected("Confirm", &other)),
        // The joiner checks our confirmation first and hangs up if the code
        // was wrong, so a close here is almost always a mistyped code.
        Err(PairingError::Closed) => return Err(PairingError::NotConfirmed),
        Err(e) => return Err(e),
    };
    verify_confirmation(&key, Role::Joiner, session, &confirm_j)?;
    Ok(name_hint)
}

/// The SPAKE2 password: the code, bound to the Short ID it was issued for.
fn password(short_id: ShortId, code: &PairingCode) -> Password {
    let text = Zeroizing::new(format!("{PAKE_LABEL}|{short_id}|{}", code.as_str()));
    Password::new(text.as_bytes())
}

/// A SPAKE2 identity: the role and the public key the transport proved.
///
/// Putting the keys here as well as in the confirmation MAC means a run in
/// which the two sides disagree about who is who cannot even agree on a key.
fn spake_identity(role: Role, key: &VerifyingKey) -> SpakeIdentity {
    let mut bytes = Vec::with_capacity(PAKE_LABEL.len() + 2 + 32);
    bytes.extend_from_slice(PAKE_LABEL.as_bytes());
    bytes.push(b'|');
    bytes.push(role.tag());
    bytes.extend_from_slice(key.as_bytes());
    SpakeIdentity::new(&bytes)
}

fn confirmation_mac(key: &[u8], speaker: Role, session: &Session) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(CONFIRM_LABEL);
    mac.update(&[speaker.tag()]);
    mac.update(&session.short_id.value().to_be_bytes());
    mac.update(session.joiner_key().as_bytes());
    mac.update(session.waiter_key().as_bytes());
    mac
}

/// The confirmation `speaker` sends.
fn confirmation(key: &[u8], speaker: Role, session: &Session) -> Vec<u8> {
    confirmation_mac(key, speaker, session)
        .finalize()
        .into_bytes()
        .to_vec()
}

/// Checks the confirmation the peer sent, in constant time.
fn verify_confirmation(
    key: &[u8],
    speaker: Role,
    session: &Session,
    received: &str,
) -> Result<(), PairingError> {
    let received = BASE64
        .decode(received)
        .map_err(|_| PairingError::WrongCode)?;
    confirmation_mac(key, speaker, session)
        .verify_slice(&received)
        .map_err(|_| PairingError::WrongCode)
}

/// A key in a message must be the key the transport proved.
fn check_claim(claimed: &str, proved: &VerifyingKey) -> Result<(), PairingError> {
    match decode_public_key(claimed) {
        Ok(key) if key == *proved => Ok(()),
        Ok(_) => Err(PairingError::KeyMismatch),
        Err(e) => Err(PairingError::Protocol(format!("public key: {e}"))),
    }
}

fn decode_b64(text: &str) -> Result<Vec<u8>, PairingError> {
    BASE64
        .decode(text)
        .map_err(|e| PairingError::Protocol(format!("base64: {e}")))
}

/// Answers a joiner's Start with `Unavailable`, without touching any code:
/// the waiter is cooling down, switched off, or busy (ADR-0028). The joiner
/// reports `reason` to its user.
pub async fn refuse<S>(stream: &mut S, reason: &str) -> Result<(), PairingError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send(
        stream,
        &Message::Unavailable {
            reason: reason.to_string(),
        },
    )
    .await
}

fn unexpected(wanted: &str, got: &Message) -> PairingError {
    let name = match got {
        Message::Start { .. } => "Start",
        Message::Unavailable { .. } => "Unavailable",
        Message::Reply { .. } => "Reply",
        Message::Confirm { .. } => "Confirm",
        Message::Decision { .. } => "Decision",
    };
    PairingError::Protocol(format!("expected {wanted}, got {name}"))
}

async fn send<S>(stream: &mut S, message: &Message) -> Result<(), PairingError>
where
    S: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(message).map_err(std::io::Error::other)?;
    debug_assert!(body.len() <= MAX_MESSAGE);
    let len = u16::try_from(body.len()).map_err(std::io::Error::other)?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;
    Ok(())
}

async fn recv<S>(stream: &mut S, timeout: Duration) -> Result<Message, PairingError>
where
    S: AsyncRead + Unpin,
{
    let read = async {
        let mut len = [0u8; 2];
        stream.read_exact(&mut len).await?;
        let len = usize::from(u16::from_be_bytes(len));
        if len > MAX_MESSAGE {
            return Err(PairingError::Protocol(format!(
                "a {len}-byte message; the limit is {MAX_MESSAGE}"
            )));
        }
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).await?;
        serde_json::from_slice(&body).map_err(|e| PairingError::Protocol(e.to_string()))
    };
    match tokio::time::timeout(timeout, read).await {
        Ok(Err(PairingError::Io(e))) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            Err(PairingError::Closed)
        }
        Ok(result) => result,
        Err(_) => Err(PairingError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vectors;
    use tokio::io::{DuplexStream, duplex};

    fn key(name: &str) -> VerifyingKey {
        vectors::verifying_key(name)
    }

    /// A key that is neither alpha nor bravo: the attacker's.
    fn mallory() -> VerifyingKey {
        vectors::verifying_key("mallory")
    }

    fn short_id_of(name: &str) -> ShortId {
        crate::identity::Fingerprint::of(&key(name)).short_id()
    }

    fn code(text: &str) -> PairingCode {
        PairingCode::parse(text).unwrap()
    }

    /// alpha joins, bravo waits: the honest case.
    fn honest_sessions() -> (Session, Session) {
        let short_id = short_id_of("bravo");
        (
            Session::new(Role::Joiner, short_id, key("alpha"), key("bravo")),
            Session::new(Role::Waiter, short_id, key("bravo"), key("alpha")),
        )
    }

    fn quick(mut session: Session) -> Session {
        session.message_timeout = Duration::from_secs(5);
        session.decision_timeout = Duration::from_secs(5);
        session
    }

    async fn yes(_: Offer) -> std::io::Result<bool> {
        Ok(true)
    }

    async fn no(_: Offer) -> std::io::Result<bool> {
        Ok(false)
    }

    /// Runs both sides over an in-memory pipe.
    async fn pair(
        joiner: Session,
        joiner_code: &str,
        waiter: Session,
        waiter_code: &str,
    ) -> (
        Result<VerifyingKey, PairingError>,
        Result<VerifyingKey, PairingError>,
    ) {
        let (mut a, mut b) = duplex(64 * 1024);
        let (jc, wc) = (code(joiner_code), code(waiter_code));
        let (joiner, waiter) = (quick(joiner), quick(waiter));
        tokio::join!(
            async {
                let r = run(&mut a, &joiner, &jc, yes).await;
                drop(a);
                r
            },
            async {
                let r = run(&mut b, &waiter, &wc, yes).await;
                drop(b);
                r
            },
        )
    }

    #[tokio::test]
    async fn the_same_code_pairs_and_each_side_gets_the_other_sides_key() {
        let (joiner, waiter) = honest_sessions();
        let (j, w) = pair(joiner, "123456", waiter, "123456").await;
        assert_eq!(j.unwrap(), key("bravo"));
        assert_eq!(w.unwrap(), key("alpha"));
    }

    #[tokio::test]
    async fn a_wrong_code_fails_on_both_sides() {
        let (joiner, waiter) = honest_sessions();
        let (j, w) = pair(joiner, "123456", waiter, "123457").await;
        assert!(matches!(j, Err(PairingError::WrongCode)), "{j:?}");
        assert!(matches!(w, Err(PairingError::NotConfirmed)), "{w:?}");
    }

    #[tokio::test]
    async fn the_code_is_bound_to_the_short_id() {
        // Same code, but the joiner thinks it is talking to a different Short
        // ID. The waiter refuses outright.
        let (mut joiner, waiter) = honest_sessions();
        joiner.short_id = short_id_of("alpha");
        let (j, w) = pair(joiner, "123456", waiter, "123456").await;
        assert!(matches!(w, Err(PairingError::WrongShortId { .. })), "{w:?}");
        assert!(j.is_err());
    }

    /// Condition 2 of the M4 approval: an attacker in the middle substitutes
    /// its own public key. Mallory relays between alpha and bravo, so the
    /// transport proves *Mallory's* key to each of them, and Mallory rewrites
    /// every claimed key to its own so the claims match what was proved.
    /// Mallory does not know the code. Pairing must fail on both sides.
    #[tokio::test]
    async fn an_attacker_relaying_with_its_own_key_cannot_complete_pairing() {
        let short_id = short_id_of("bravo");
        let joiner = quick(Session::new(
            Role::Joiner,
            short_id,
            key("alpha"),
            mallory(),
        ));
        let waiter = quick(Session::new(
            Role::Waiter,
            short_id,
            key("bravo"),
            mallory(),
        ));

        let (mut alpha_side, mut to_alpha) = duplex(64 * 1024);
        let (mut bravo_side, mut to_bravo) = duplex(64 * 1024);
        let jc = code("123456");
        let wc = code("123456");

        let proxy = async {
            // alpha → bravo: Start, with the key rewritten.
            if let Ok(Message::Start {
                version,
                short_id,
                spake,
                name_hint,
                ..
            }) = recv(&mut to_alpha, Duration::from_secs(5)).await
            {
                let rewritten = Message::Start {
                    version,
                    short_id,
                    public_key: encode_public_key(&mallory()),
                    spake,
                    name_hint,
                };
                let _ = send(&mut to_bravo, &rewritten).await;
            }
            // bravo → alpha: Reply, with the key rewritten.
            if let Ok(Message::Reply { spake, confirm, .. }) =
                recv(&mut to_bravo, Duration::from_secs(5)).await
            {
                let rewritten = Message::Reply {
                    public_key: encode_public_key(&mallory()),
                    spake,
                    confirm,
                };
                let _ = send(&mut to_alpha, &rewritten).await;
            }
            // Pass anything else straight through until one side hangs up.
            loop {
                tokio::select! {
                    m = recv(&mut to_alpha, Duration::from_secs(5)) => match m {
                        Ok(m) => { let _ = send(&mut to_bravo, &m).await; }
                        Err(_) => break,
                    },
                    m = recv(&mut to_bravo, Duration::from_secs(5)) => match m {
                        Ok(m) => { let _ = send(&mut to_alpha, &m).await; }
                        Err(_) => break,
                    },
                }
            }
            drop(to_alpha);
            drop(to_bravo);
        };

        let (j, w, ()) = tokio::join!(
            async {
                let r = run(&mut alpha_side, &joiner, &jc, yes).await;
                drop(alpha_side);
                r
            },
            async {
                let r = run(&mut bravo_side, &waiter, &wc, yes).await;
                drop(bravo_side);
                r
            },
            proxy,
        );
        // Each side derived its SPAKE2 key over a different pair of public
        // keys than the other, so the confirmations cannot match.
        assert!(matches!(j, Err(PairingError::WrongCode)), "joiner: {j:?}");
        assert!(w.is_err(), "waiter: {w:?}");
    }

    /// The other half of condition 2: the claimed key in a message differs
    /// from the key the connection proved. Refused before any SPAKE2 work.
    #[tokio::test]
    async fn a_claimed_key_that_differs_from_the_proved_key_is_refused() {
        let short_id = short_id_of("bravo");
        // The waiter's transport proved alpha; the joiner claims mallory.
        let (mut a, mut b) = duplex(64 * 1024);
        let waiter = quick(Session::new(
            Role::Waiter,
            short_id,
            key("bravo"),
            key("alpha"),
        ));
        let spoofed = Message::Start {
            version: VERSION,
            short_id: short_id.to_string(),
            public_key: encode_public_key(&mallory()),
            spake: BASE64.encode([0x41u8; 33]),
            name_hint: None,
        };
        send(&mut a, &spoofed).await.unwrap();
        let w = run(&mut b, &waiter, &code("123456"), yes).await;
        assert!(matches!(w, Err(PairingError::KeyMismatch)), "{w:?}");
    }

    /// And from the joiner's side: the waiter's Reply claims a key other than
    /// the one the joiner connected to.
    #[tokio::test]
    async fn a_reply_claiming_a_different_key_is_refused() {
        let (joiner, waiter) = honest_sessions();
        let (mut a, mut b) = duplex(64 * 1024);
        let joiner = quick(joiner);
        let waiter = quick(waiter);
        let c = code("123456");

        let honest_waiter_with_a_lying_reply = async {
            // Behave honestly up to the Reply, then lie about our key.
            let Message::Start { spake, .. } = recv(&mut b, Duration::from_secs(5)).await.unwrap()
            else {
                panic!("expected Start")
            };
            let (pake, spake_b) = Spake2::<Ed25519Group>::start_b(
                &password(waiter.short_id, &c),
                &spake_identity(Role::Joiner, waiter.joiner_key()),
                &spake_identity(Role::Waiter, waiter.waiter_key()),
            );
            let k = pake.finish(&BASE64.decode(spake).unwrap()).unwrap();
            let reply = Message::Reply {
                public_key: encode_public_key(&mallory()),
                spake: BASE64.encode(&spake_b),
                confirm: BASE64.encode(confirmation(&k, Role::Waiter, &waiter)),
            };
            send(&mut b, &reply).await.unwrap();
            drop(b);
        };
        let (j, ()) = tokio::join!(
            run(&mut a, &joiner, &c, yes),
            honest_waiter_with_a_lying_reply
        );
        assert!(matches!(j, Err(PairingError::KeyMismatch)), "{j:?}");
    }

    /// The MAC names the speaker's role, so the waiter's own confirmation
    /// cannot be sent back to it as the joiner's.
    #[test]
    fn confirmations_are_bound_to_the_role() {
        let (joiner, _) = honest_sessions();
        let k = [7u8; 32];
        assert_ne!(
            confirmation(&k, Role::Joiner, &joiner),
            confirmation(&k, Role::Waiter, &joiner)
        );
    }

    /// ... and to both public keys and the Short ID.
    #[test]
    fn confirmations_are_bound_to_both_keys_and_the_short_id() {
        let (base, _) = honest_sessions();
        let k = [7u8; 32];
        let reference = confirmation(&k, Role::Joiner, &base);

        let mut other_joiner = base.clone();
        other_joiner.local_key = mallory();
        let mut other_waiter = base.clone();
        other_waiter.remote_key = mallory();
        let mut other_id = base.clone();
        other_id.short_id = short_id_of("alpha");

        for (what, session) in [
            ("joiner key", other_joiner),
            ("waiter key", other_waiter),
            ("short id", other_id),
        ] {
            assert_ne!(
                confirmation(&k, Role::Joiner, &session),
                reference,
                "changing the {what} did not change the MAC"
            );
        }
    }

    #[tokio::test]
    async fn if_the_waiter_declines_neither_side_pairs() {
        let (joiner, waiter) = honest_sessions();
        let (mut a, mut b) = duplex(64 * 1024);
        let (joiner, waiter) = (quick(joiner), quick(waiter));
        let c = code("123456");
        let (j, w) = tokio::join!(run(&mut a, &joiner, &c, yes), run(&mut b, &waiter, &c, no));
        assert!(matches!(j, Err(PairingError::DeclinedByPeer)), "{j:?}");
        assert!(matches!(w, Err(PairingError::Declined)), "{w:?}");
    }

    #[tokio::test]
    async fn if_the_joiner_declines_neither_side_pairs() {
        let (joiner, waiter) = honest_sessions();
        let (mut a, mut b) = duplex(64 * 1024);
        let (joiner, waiter) = (quick(joiner), quick(waiter));
        let c = code("123456");
        let (j, w) = tokio::join!(run(&mut a, &joiner, &c, no), run(&mut b, &waiter, &c, yes));
        assert!(matches!(j, Err(PairingError::Declined)), "{j:?}");
        assert!(matches!(w, Err(PairingError::DeclinedByPeer)), "{w:?}");
    }

    /// No answer in time is a no, exactly like an unanswered Accept.
    #[tokio::test]
    async fn an_unanswered_confirmation_is_a_no() {
        let (joiner, waiter) = honest_sessions();
        let (mut a, mut b) = duplex(64 * 1024);
        let joiner = quick(joiner);
        let mut waiter = quick(waiter);
        waiter.decision_timeout = Duration::from_millis(200);
        let c = code("123456");
        let never = |_| std::future::pending::<std::io::Result<bool>>();
        let (j, w) = tokio::join!(
            run(&mut a, &joiner, &c, yes),
            run(&mut b, &waiter, &c, never)
        );
        assert!(matches!(w, Err(PairingError::Declined)), "{w:?}");
        assert!(matches!(j, Err(PairingError::DeclinedByPeer)), "{j:?}");
    }

    /// The prompt is only reached after the code has been proved: a wrong
    /// code must never put a question in front of the user.
    #[tokio::test]
    async fn nobody_is_asked_to_confirm_after_a_wrong_code() {
        let (joiner, waiter) = honest_sessions();
        let (mut a, mut b) = duplex(64 * 1024);
        let (joiner, waiter) = (quick(joiner), quick(waiter));
        let asked = std::sync::atomic::AtomicBool::new(false);
        let tattle = |_| {
            asked.store(true, std::sync::atomic::Ordering::SeqCst);
            async { Ok(true) }
        };
        let tattle2 = |_| {
            asked.store(true, std::sync::atomic::Ordering::SeqCst);
            async { Ok(true) }
        };
        let (jc, wc) = (code("111111"), code("222222"));
        let (j, w) = tokio::join!(
            async {
                let r = run(&mut a, &joiner, &jc, tattle).await;
                drop(a);
                r
            },
            run(&mut b, &waiter, &wc, tattle2)
        );
        assert!(j.is_err() && w.is_err());
        assert!(!asked.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_future_protocol_version_is_refused() {
        let (_, waiter) = honest_sessions();
        let (mut a, mut b): (DuplexStream, DuplexStream) = duplex(4096);
        let start = Message::Start {
            version: VERSION + 1,
            short_id: waiter.short_id.to_string(),
            public_key: encode_public_key(&key("alpha")),
            spake: BASE64.encode([0x41u8; 33]),
            name_hint: None,
        };
        send(&mut a, &start).await.unwrap();
        let w = run(&mut b, &quick(waiter), &code("123456"), yes).await;
        assert!(
            matches!(w, Err(PairingError::UnsupportedVersion(2))),
            "{w:?}"
        );
    }

    #[tokio::test]
    async fn an_oversized_message_is_refused_before_it_is_read() {
        let (_, waiter) = honest_sessions();
        let (mut a, mut b) = duplex(4096);
        a.write_all(&u16::MAX.to_be_bytes()).await.unwrap();
        let w = run(&mut b, &quick(waiter), &code("123456"), yes).await;
        assert!(matches!(w, Err(PairingError::Protocol(_))), "{w:?}");
    }

    #[tokio::test]
    async fn unknown_fields_are_refused() {
        let (_, waiter) = honest_sessions();
        let (mut a, mut b) = duplex(4096);
        let body = br#"{"type":"decision","accept":true,"auto":true}"#;
        a.write_all(&(body.len() as u16).to_be_bytes())
            .await
            .unwrap();
        a.write_all(body).await.unwrap();
        let w = run(&mut b, &quick(waiter), &code("123456"), yes).await;
        assert!(matches!(w, Err(PairingError::Protocol(_))), "{w:?}");
    }

    /// `beam listen` names a new peer from the joiner's hint; the hint only
    /// reaches the waiter's decision, after the code is proved.
    #[tokio::test]
    async fn the_joiners_name_hint_reaches_the_waiters_decision() {
        let (mut joiner, waiter) = honest_sessions();
        joiner.name_hint = Some("alices-laptop".into());
        let (mut a, mut b) = duplex(64 * 1024);
        let (joiner, waiter) = (quick(joiner), quick(waiter));
        let c = code("123456");
        let seen = std::sync::Mutex::new(None);
        let record = |offer: Offer| {
            *seen.lock().unwrap() = Some(offer);
            async { Ok(true) }
        };
        let (j, w) = tokio::join!(
            run(&mut a, &joiner, &c, yes),
            run(&mut b, &waiter, &c, record)
        );
        j.unwrap();
        w.unwrap();
        let offer = seen.into_inner().unwrap().expect("the waiter decided");
        assert_eq!(offer.name_hint.as_deref(), Some("alices-laptop"));
        assert_eq!(offer.key, key("alpha"));
    }

    #[tokio::test]
    async fn an_oversized_name_hint_is_dropped() {
        let (mut joiner, waiter) = honest_sessions();
        joiner.name_hint = Some("x".repeat(MAX_NAME_HINT + 1));
        let (mut a, mut b) = duplex(64 * 1024);
        let (joiner, waiter) = (quick(joiner), quick(waiter));
        let c = code("123456");
        let seen = std::sync::Mutex::new(None);
        let record = |offer: Offer| {
            *seen.lock().unwrap() = Some(offer.name_hint);
            async { Ok(true) }
        };
        let (j, w) = tokio::join!(
            run(&mut a, &joiner, &c, yes),
            run(&mut b, &waiter, &c, record)
        );
        j.unwrap();
        w.unwrap();
        assert_eq!(seen.into_inner().unwrap(), Some(None));
    }

    /// A waiter that is cooling down or switched off answers `Unavailable`,
    /// and the joiner reports why.
    #[tokio::test]
    async fn an_unavailable_waiter_is_reported_as_such() {
        let (joiner, _) = honest_sessions();
        let (mut a, mut b) = duplex(64 * 1024);
        let joiner = quick(joiner);
        let c = code("123456");
        let waiter = async {
            recv(&mut b, Duration::from_secs(5)).await.unwrap();
            refuse(&mut b, "pairing is paused").await.unwrap();
        };
        let (j, ()) = tokio::join!(run(&mut a, &joiner, &c, yes), waiter);
        match j {
            Err(PairingError::Unavailable(reason)) => assert_eq!(reason, "pairing is paused"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn silence_times_out() {
        let (_, mut waiter) = honest_sessions();
        waiter.message_timeout = Duration::from_millis(100);
        let (_a, mut b) = duplex(4096);
        let w = run(&mut b, &waiter, &code("123456"), yes).await;
        assert!(matches!(w, Err(PairingError::Timeout)), "{w:?}");
    }
}
