//! First-time pairing: turning a Short ID and a six-digit code into a public
//! key in `known_peers`, on both devices, with both people's consent.
//!
//! * [`code`] — the code, and the single-use, ten-minute rules.
//! * [`protocol`] — SPAKE2 and key confirmation, over any byte stream.
//! * [`session`] — the two roles over the rendezvous server and iroh.
//!
//! In M4 the waiting side runs `beam pair --wait`. In M5 that waiting moves
//! into `beam listen`, next to incoming transfers. See ADR-0026 and ADR-0028.

pub mod code;
pub mod protocol;
pub mod rotation;
pub mod session;

pub use code::{CodeSlot, CodeUnavailable, DEFAULT_CODE_TTL, PairingCode, ParseCodeError};
pub use protocol::{Offer, PairingError, Role};
pub use rotation::{Attempt, Notice, Policy, Rotation, Unavailable};
pub use session::{
    Confirm, ConfirmRequest, Event, Network, PairError, Paired, Pairing, Timeouts, attempt_kind,
    choose_name, join, refuse_connection, serve, wait,
};
