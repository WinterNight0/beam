//! The rendezvous server and its client.
//!
//! The server maps a Short ID to an iroh endpoint address, so that
//! `beam pair <ID>` can find a device from nine digits a person read aloud. It
//! replaces n0's DNS discovery, which beam does not use; see `docs/n0-data.md`.
//!
//! * [`proto`] — the wire format and the checks both ends make.
//! * [`server`] — the in-memory table and the WebSocket loop.
//! * [`client`] — register and look up.
//!
//! See ADR-0027 for why the server is allowed to be untrusted.

pub mod client;
pub mod proto;
pub mod server;

pub use client::{Found, REFRESH_EVERY, RendezvousClient, RendezvousError};
pub use server::{Registry, ServerConfig, serve};
