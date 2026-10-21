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
