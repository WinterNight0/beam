//! What beam tells a person when a send cannot go ahead, written once so
//! `beam send`, the full-screen view and a GUI all say the same thing.
//!
//! Every message comes back already cleaned for the screen (ADR-0034): it
//! can quote the network, such as the reason a peer gave for closing a
//! connection, and a front end must be able to show it as it is. Lines after
//! the first are indented to sit under the text after `beam: `, as the CLI
//! prints it.

use ed25519_dalek::VerifyingKey;

use crate::identity::Fingerprint;
use crate::transfer::{RejectReason, TransferError};
use crate::transport::dial::DialError;
use crate::untrusted;

/// Why the peer could not be reached, in words that say what to do.
pub fn unreachable(peer: &str, key: &VerifyingKey, error: DialError) -> String {
    let fingerprint = Fingerprint::of(key).short();
    untrusted::lines(&match error {
        DialError::Unreachable(why) => format!(
            "{peer} ({fingerprint}) is not reachable ({why}).\n       \
             Either {peer} is not running `beam listen`, {peer} is somewhere beam was not\n       \
             told about, or {peer}'s key has changed because it ran `beam init` again.\n       \
             beam never follows a key change by itself.\n\n       \
             If {peer} is listening, ask for the invite it shows now and run\n           \
             beam pair <{peer}'s invite> --name {peer}\n       \
             That only updates where to find {peer}; it cannot change {peer}'s key.\n\n       \
             WARNING: if {peer} now has a new key, someone could be impersonating {peer}.\n       \
             Check the new fingerprint with {peer} in person before you re-pair:\n           \
             beam remove {peer}\n           \
             beam pair <{peer}'s new invite> --name {peer}"
        ),
        DialError::NoAddress => format!(
            "beam does not know where to find {peer} ({fingerprint}): no relay is set in\n       \
             config.toml and no address was saved for {peer}. Ask for the invite `beam listen`\n       \
             shows on {peer} and run\n           \
             beam pair <{peer}'s invite> --name {peer}"
        ),
        other => other.to_string(),
    })
}

/// A refusal from the peer, or another way the transfer ended badly, said
/// the way a person would say it.
pub fn refused(peer: &str, error: TransferError) -> String {
    untrusted::lines(&match error {
        TransferError::PeerInterrupted => format!(
            "{peer} stopped beam on their side (Ctrl+C), so the transfer was cancelled.\n       \
             Anything {peer} already received is kept: send the same file again later to resume."
        ),
        TransferError::Rejected(RejectReason::Busy) => {
            format!("{peer} is receiving another file; try again later")
        }
        TransferError::Rejected(RejectReason::UnknownPeer) => format!(
            "{peer} does not recognise this device's key.\n       \
             Either {peer} removed this device, or this device's key changed since pairing\n       \
             because `beam init` was run again here.\n\n       \
             WARNING: a changed key is exactly what an impersonator would present, so {peer}\n       \
             must not simply accept it. To re-pair, compare fingerprints in person:\n           \
             on {peer}:  beam remove <this device>, then beam listen\n           \
             here:     beam remove {peer}, then beam pair <{peer}'s invite> --name {peer}"
        ),
        TransferError::Rejected(RejectReason::Declined) => {
            format!("{peer} declined the transfer")
        }
        TransferError::Rejected(RejectReason::Expired) => {
            format!("{peer} did not answer within a minute; the transfer was not accepted")
        }
        other => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> VerifyingKey {
        crate::identity::Identity::generate("x")
            .unwrap()
            .verifying_key()
    }

    /// Text from the other side never reaches a screen as control
    /// characters, whichever front end shows it (ADR-0034, SECURITY.md F-5).
    #[test]
    fn messages_come_back_cleaned() {
        let hostile = "closed by peer: \u{1b}[2J\u{1b}]0;owned\u{7}gone";
        for shown in [
            unreachable("alice", &key(), DialError::Unreachable(hostile.into())),
            refused("alice", TransferError::Cancelled(hostile.into())),
            refused("alice", TransferError::BadRequest(hostile.into())),
        ] {
            assert!(shown.contains("gone"), "the reason is kept: {shown}");
            assert!(
                !shown.chars().any(|c| c.is_control() && c != '\n'),
                "{shown:?}"
            );
        }
    }

    /// The key-change warning is the one beam must never lose (CLAUDE.md
    /// rule 3): both ways a changed key shows up still say so.
    #[test]
    fn both_key_change_messages_warn_of_impersonation() {
        let not_reached = unreachable("alice", &key(), DialError::Unreachable("timed out".into()));
        let unknown = refused("alice", TransferError::Rejected(RejectReason::UnknownPeer));
        for message in [not_reached, unknown] {
            assert!(message.contains("WARNING"), "{message}");
            assert!(message.contains("impersonat"), "{message}");
            assert!(message.contains("in person"), "{message}");
        }
    }

    #[test]
    fn a_decline_and_an_expiry_are_said_plainly() {
        assert_eq!(
            refused("bob", TransferError::Rejected(RejectReason::Declined)),
            "bob declined the transfer"
        );
        assert!(
            refused("bob", TransferError::Rejected(RejectReason::Expired))
                .contains("did not answer")
        );
    }
}
