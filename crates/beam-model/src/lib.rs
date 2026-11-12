//! beam's view model (ADR-0048): the state a screen shows, without the
//! screen.
//!
//! The full-screen view and a GUI draw the same things: this device, the
//! friends and when each was last seen, the requests waiting for an answer,
//! the Receiving switch, the file going out, and the questions beam asks.
//! That state, and the rules for changing it, live here once, so both front
//! ends behave the same and a rule is tested once.
//!
//! The most important rule is in [`ask`]: every yes/no question that could
//! accept a file, pair a device, remove a friend, move a friend to another
//! relay or stop a transfer **starts on No**, and there is no way to build
//! one that starts on Yes.
//!
//! What stays in a front end: keys, clicks, text boxes, layout and words
//! that only fit its screen. This crate depends on `beam-core` and nothing
//! else from beam; it has no terminal or GUI library.

pub mod ask;
pub mod requests;
pub mod sending;
pub mod snapshot;

pub use ask::{Ask, Choice};
