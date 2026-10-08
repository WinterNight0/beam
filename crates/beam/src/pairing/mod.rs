//! First-time pairing: turning an invite and a six-digit code into a public
//! key in `known_peers`, on both devices, with both people's consent.
//!
//! * [`code`] — the code, and the single-use, ten-minute rules.
//! * [`protocol`] — SPAKE2 and key confirmation, over any byte stream.
//! * [`session`] — the two roles over iroh, starting from an invite.
//!
//! The waiting side runs `beam listen` (or `beam pair --wait`); the joiner
//! runs `beam pair <INVITE>`. See ADR-0026, ADR-0028 and ADR-0036.

pub mod code;
pub mod protocol;
pub mod rotation;
pub mod session;

pub use code::{CodeSlot, CodeUnavailable, DEFAULT_CODE_TTL, PairingCode, ParseCodeError};
pub use protocol::{Offer, PairingError, Role};
pub use rotation::{Attempt, Notice, Policy, Rotation, Unavailable};
pub use session::{
    Confirm, ConfirmRequest, Event, Network, PairError, Paired, Pairing, Timeouts, attempt_kind,
    check_name, choose_name, join, refuse_connection, serve, wait,
};
