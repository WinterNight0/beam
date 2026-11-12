//! The yes/no questions beam asks, and the rule that makes them safe: each
//! one **starts on No** (ADR-0043).
//!
//! A question a person answers by habit (Enter, a double click, a click
//! where the button used to be) must never accept a file, pair a device,
//! remove a friend, move a friend to someone else's relay, or cut off a
//! transfer. So an [`Ask`] can only be made by [`Ask::new`], which puts the
//! highlight on [`Choice::No`]. The person can move it to Yes; nothing else
//! can start it there.

use std::time::Instant;

use beam_core::actions::inbox::RequestView;

/// The two answers of a yes/no question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Yes,
    No,
}

impl Choice {
    /// The other answer.
    pub fn other(self) -> Self {
        match self {
            Self::Yes => Self::No,
            Self::No => Self::Yes,
        }
    }
}

/// A question and which answer is highlighted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask<Q> {
    question: Q,
    focus: Choice,
}

impl<Q> Ask<Q> {
    /// The question, highlighting **No**. The only way to make one.
    pub fn new(question: Q) -> Self {
        Self {
            question,
            focus: Choice::No,
        }
    }

    pub fn question(&self) -> &Q {
        &self.question
    }

    pub fn into_question(self) -> Q {
        self.question
    }

    /// The highlighted answer: what Enter would choose.
    pub fn focus(&self) -> Choice {
        self.focus
    }

    /// Moves the highlight to the other answer (arrow keys, Tab).
    pub fn flip(&mut self) {
        self.focus = self.focus.other();
    }

    /// Moves the highlight onto `choice`: the person pointed at it, or has
    /// just chosen it and the question stays up while that is carried out.
    pub fn point_at(&mut self, choice: Choice) {
        self.focus = choice;
    }
}

/// May this friend send this file? No is Decline: the request keeps
/// waiting for nothing, and expires as a no.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    /// The receiver's number for the request.
    pub id: u64,
    pub request: RequestView,
    pub expires: Instant,
}

/// Remove this friend? No keeps them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveFriend {
    pub name: String,
    /// Hex, without `SHA256:`.
    pub fingerprint: String,
}

/// Do the two fingerprints match (pairing)? No saves nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SameFingerprints {
    /// What the new friend will be called.
    pub name: String,
    /// Theirs and ours, hex.
    pub peer: String,
    pub own: String,
}

/// An invite moves a paired friend to another relay: save it? No keeps the
/// relay they have (ADR-0038).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayMove {
    pub name: String,
    pub fingerprint: String,
    /// The relay now and after, described for a person.
    pub old: String,
    pub new: String,
    /// The invite as pasted, to save on a yes.
    pub invite: String,
}

/// A file is still going out or coming in: leave anyway? No stays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeaveMidTransfer;

/// A file is arriving: turn Receiving off anyway? No keeps receiving.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StopMidTransfer;

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming() -> Incoming {
        Incoming {
            id: 1,
            request: RequestView {
                peer_name: "alice".into(),
                fingerprint: "ab12".into(),
                file_name: "report.pdf".into(),
                size: 1,
                resume: None,
            },
            expires: Instant::now(),
        }
    }

    /// Every question beam asks starts on No, whatever it is about.
    #[test]
    fn every_question_starts_on_no() {
        assert_eq!(Ask::new(incoming()).focus(), Choice::No);
        assert_eq!(
            Ask::new(RemoveFriend {
                name: "bob".into(),
                fingerprint: "cd34".into()
            })
            .focus(),
            Choice::No
        );
        assert_eq!(
            Ask::new(SameFingerprints {
                name: "bob".into(),
                peer: "cd34".into(),
                own: "ef56".into()
            })
            .focus(),
            Choice::No
        );
        assert_eq!(
            Ask::new(RelayMove {
                name: "bob".into(),
                fingerprint: "cd34".into(),
                old: "a".into(),
                new: "b".into(),
                invite: "beam1".into()
            })
            .focus(),
            Choice::No
        );
        assert_eq!(Ask::new(LeaveMidTransfer).focus(), Choice::No);
        assert_eq!(Ask::new(StopMidTransfer).focus(), Choice::No);
    }

    /// Only the person moves it: one flip to Yes, another back.
    #[test]
    fn the_highlight_moves_only_when_asked() {
        let mut ask = Ask::new(incoming());
        ask.flip();
        assert_eq!(ask.focus(), Choice::Yes);
        ask.flip();
        assert_eq!(ask.focus(), Choice::No);
        ask.point_at(Choice::Yes);
        assert_eq!(ask.focus(), Choice::Yes);
        assert_eq!(ask.question().id, 1);
    }
}
