//! beam sends files directly between two computers.
//!
//! A peer must be paired before it can send anything, and every incoming
//! transfer has to be accepted by hand. There is no auto-accept.
//!
//! The command implementations live in [`cli`] rather than in the binary so
//! that they can be exercised by tests; see `docs/decisions.md` ADR-0002.

pub mod cli;
pub mod config;
pub(crate) mod hex;
pub mod identity;
pub mod listener;
pub mod pairing;
pub mod rendezvous;
pub mod transfer;
pub mod transport;
pub mod ui;
pub mod untrusted;
