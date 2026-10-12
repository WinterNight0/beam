//! beam sends files directly between two computers.
//!
//! This crate is beam's front ends: the command line ([`cli`]) and the
//! full-screen view ([`tui`]). Everything they share, and every security
//! rule, is in `beam-core` (ADR-0048).
//!
//! The command implementations live in [`cli`] rather than in the binary so
//! that they can be exercised by tests; see `docs/decisions.md` ADR-0002.

pub mod cli;
pub mod tui;
pub mod ui;

// TEMPORARY (ADR-0048, part a of the split): the core's modules under their
// old paths, so this move is only a move. Part b replaces every use with
// `beam_core::…` and removes these lines.
pub use beam_core::{
    actions, agent, config, history, identity, invite, listen_status, listener, pairing, text,
    transfer, transport, untrusted,
};
