//! What a person does with beam (send, pair, receive), written once for
//! every front end: the CLI, the full-screen view and, later, a GUI.
//!
//! This is the second of beam's layers (ADR-0048). It may use the core
//! modules (`identity`, `transfer`, `transport`, `listener`, …); the core
//! never uses it. Nothing here prints, draws or reads a key: a front end is
//! handed plain text and events, and decides how to show them.

pub mod messages;
