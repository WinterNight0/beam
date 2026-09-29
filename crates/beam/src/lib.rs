//! beam sends files directly between two computers.
//!
//! Every node can send or receive over a direct TCP connection. Incoming
//! transfers are still presented to the user for acceptance.
//!
//! The command implementations live in [`cli`] rather than in the binary so
//! that they can be exercised by tests; see `docs/decisions.md` ADR-0002.

pub mod cli;
pub mod config;
pub(crate) mod hex;
pub mod identity;
pub mod listener;
pub mod transfer;
pub mod transport;
pub mod ui;
pub mod untrusted;
