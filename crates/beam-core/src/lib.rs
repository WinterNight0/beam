//! The core of beam: everything that makes it secure and correct, and the
//! actions every front end shares (ADR-0048).
//!
//! A peer must be paired before it can send anything, and every incoming
//! transfer has to be accepted by hand. There is no auto-accept.
//!
//! Two layers live here:
//!
//! * **core**: identity, pairing, the transfer engine, the transport, the
//!   listener and the background agent. The security rules are enforced
//!   here, whatever a front end does.
//! * **actions** ([`actions`]): what a person does (send, pair, receive,
//!   answer the inbox), written once for the CLI, the full-screen view and
//!   a GUI. The core never uses them (`tests/layers.rs`).
//!
//! Nothing in this crate prints to a terminal, draws or reads a key: it does
//! not depend on a terminal or GUI library, so it cannot.

pub mod actions;
pub mod agent;
pub mod config;
pub(crate) mod hex;
pub mod history;
pub mod identity;
pub mod invite;
pub mod listen_status;
pub mod listener;
pub mod pairing;
pub(crate) mod replace;
pub mod text;
pub mod transfer;
pub mod transport;
pub mod untrusted;
