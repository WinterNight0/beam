//! The transfer state machine.
//!
//! Written out explicitly, rather than implied by the order of statements in
//! the sender and receiver, so that the rule "no file bytes before ACCEPT"
//! (S-3) is something a test can check rather than something a reader has to
//! trace by hand.
//!
//! There is no `Interrupted` or `Reconnecting` state. A transfer that loses its
//! connection simply fails; resuming is a *new* transfer that happens to find
//! data already on disk, and it goes through the whole machine from the top,
//! prompt included. That is what makes "every resume needs a new Accept" (S-2)
//! a property of the design rather than a rule somebody has to remember to
//! enforce. See ADR-0020.

/// Where a transfer has got to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum State {
    /// Built, not yet sent.
    Requested,
    /// The request is with the peer; nobody has answered.
    AwaitingAccept,
    /// Accepted; the byte stream is being established.
    Connecting,
    /// Chunks are moving.
    Transferring,
    /// All chunks are in; checking the whole-file hash.
    Verifying,

    /// Terminal: the file is written and verified.
    Completed,
    /// Terminal: the peer said no.
    Rejected,
    /// Terminal: nobody answered in time.
    Expired,
    /// Terminal: something broke.
    Failed,
    /// Terminal: somebody stopped it.
    Cancelled,
}

impl State {
    /// Every state, for exhaustive tests.
    pub const ALL: [State; 10] = [
        State::Requested,
        State::AwaitingAccept,
        State::Connecting,
        State::Transferring,
        State::Verifying,
        State::Completed,
        State::Rejected,
        State::Expired,
        State::Failed,
        State::Cancelled,
    ];

    /// Whether the transfer is over, whatever the outcome.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            State::Completed | State::Rejected | State::Expired | State::Failed | State::Cancelled
        )
    }

    /// Whether the receiver has agreed and file bytes may legitimately move.
    ///
    /// The sender consults this before emitting any chunk (S-3), and the
    /// receiver before accepting one (S-4).
    pub fn file_data_allowed(self) -> bool {
        matches!(self, State::Transferring)
    }
}

/// Something that happened to a transfer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Event {
    /// The request went out.
    RequestSent,
    /// The peer accepted.
    Accepted,
    /// The peer declined.
    Declined,
    /// Nobody answered before the deadline (S-5).
    TimedOut,
    /// The byte stream is ready.
    Connected,
    /// Every chunk has been stored.
    ChunksDone,
    /// The whole-file hash matched.
    Verified,
    /// The whole-file hash did not match.
    VerificationFailed,
    /// Somebody stopped the transfer.
    Cancel,
    /// Something broke.
    Fail,
}

impl Event {
    /// Every event, for exhaustive tests.
    pub const ALL: [Event; 10] = [
        Event::RequestSent,
        Event::Accepted,
        Event::Declined,
        Event::TimedOut,
        Event::Connected,
        Event::ChunksDone,
        Event::Verified,
        Event::VerificationFailed,
        Event::Cancel,
        Event::Fail,
    ];
}

/// An event that the current state has no answer for.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("illegal transfer transition: {state:?} cannot handle {event:?}")]
pub struct IllegalTransition {
    pub state: State,
    pub event: Event,
}

/// Where an event takes a state, or `None` if it is not allowed there.
///
/// `Cancel` and `Fail` are accepted from any non-terminal state; nothing at all
/// is accepted from a terminal one.
pub fn next(state: State, event: Event) -> Option<State> {
    if state.is_terminal() {
        return None;
    }
    match event {
        Event::Cancel => return Some(State::Cancelled),
        Event::Fail => return Some(State::Failed),
        _ => {}
    }

    Some(match (state, event) {
        (State::Requested, Event::RequestSent) => State::AwaitingAccept,

        (State::AwaitingAccept, Event::Accepted) => State::Connecting,
        (State::AwaitingAccept, Event::Declined) => State::Rejected,
        (State::AwaitingAccept, Event::TimedOut) => State::Expired,

        (State::Connecting, Event::Connected) => State::Transferring,

        (State::Transferring, Event::ChunksDone) => State::Verifying,

        (State::Verifying, Event::Verified) => State::Completed,
        (State::Verifying, Event::VerificationFailed) => State::Failed,

        _ => return None,
    })
}

/// A transfer's position in the state machine.
#[derive(Clone, Copy, Debug)]
pub struct Machine {
    state: State,
}

impl Machine {
    /// A transfer that has been built but not sent.
    pub fn new() -> Self {
        Self {
            state: State::Requested,
        }
    }

    /// The current state.
    pub fn state(&self) -> State {
        self.state
    }

    /// Whether file bytes may move right now.
    pub fn file_data_allowed(&self) -> bool {
        self.state.file_data_allowed()
    }

    /// Applies an event, or refuses it.
    pub fn apply(&mut self, event: Event) -> Result<State, IllegalTransition> {
        match next(self.state, event) {
            Some(state) => {
                self.state = state;
                Ok(state)
            }
            None => Err(IllegalTransition {
                state: self.state,
                event,
            }),
        }
    }
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The specification, written out by hand. `next` must agree with this and
    /// must reject every pair that is not in it.
    ///
    /// `Cancel` and `Fail` are not listed: they are allowed from every
    /// non-terminal state and are checked separately.
    const LEGAL: [(State, Event, State); 7] = [
        (State::Requested, Event::RequestSent, State::AwaitingAccept),
        (State::AwaitingAccept, Event::Accepted, State::Connecting),
        (State::AwaitingAccept, Event::Declined, State::Rejected),
        (State::AwaitingAccept, Event::TimedOut, State::Expired),
        (State::Connecting, Event::Connected, State::Transferring),
        (State::Transferring, Event::ChunksDone, State::Verifying),
        (State::Verifying, Event::Verified, State::Completed),
    ];

    /// `VerificationFailed` lands on `Failed`, which the general `Fail` rule
    /// would also produce; it is listed here so the table above stays a list of
    /// distinct outcomes.
    const LEGAL_TO_FAILED: [(State, Event); 1] = [(State::Verifying, Event::VerificationFailed)];

    fn is_specified(state: State, event: Event) -> Option<State> {
        if state.is_terminal() {
            return None;
        }
        if event == Event::Cancel {
            return Some(State::Cancelled);
        }
        if event == Event::Fail {
            return Some(State::Failed);
        }
        if LEGAL_TO_FAILED.contains(&(state, event)) {
            return Some(State::Failed);
        }
        LEGAL
            .iter()
            .find(|(s, e, _)| *s == state && *e == event)
            .map(|(_, _, to)| *to)
    }

    #[test]
    fn every_state_and_event_pair_matches_the_specification() {
        for state in State::ALL {
            for event in Event::ALL {
                assert_eq!(
                    next(state, event),
                    is_specified(state, event),
                    "transition {state:?} + {event:?}"
                );
            }
        }
    }

    #[test]
    fn terminal_states_accept_nothing() {
        for state in State::ALL.into_iter().filter(|s| s.is_terminal()) {
            for event in Event::ALL {
                assert_eq!(
                    next(state, event),
                    None,
                    "terminal {state:?} accepted {event:?}"
                );
            }
        }
    }

    #[test]
    fn cancel_and_fail_work_from_every_live_state() {
        for state in State::ALL.into_iter().filter(|s| !s.is_terminal()) {
            assert_eq!(next(state, Event::Cancel), Some(State::Cancelled));
            assert_eq!(next(state, Event::Fail), Some(State::Failed));
        }
    }

    #[test]
    fn file_data_is_allowed_in_exactly_one_state() {
        let allowed: Vec<State> = State::ALL
            .into_iter()
            .filter(|s| s.file_data_allowed())
            .collect();
        assert_eq!(
            allowed,
            vec![State::Transferring],
            "file bytes must be possible only while transferring (S-3, S-4)"
        );
    }

    #[test]
    fn the_happy_path_walks_from_requested_to_completed() {
        let mut machine = Machine::new();
        assert_eq!(machine.state(), State::Requested);
        assert!(!machine.file_data_allowed());

        for (event, expected) in [
            (Event::RequestSent, State::AwaitingAccept),
            (Event::Accepted, State::Connecting),
            (Event::Connected, State::Transferring),
            (Event::ChunksDone, State::Verifying),
            (Event::Verified, State::Completed),
        ] {
            assert_eq!(machine.apply(event).expect("legal"), expected);
        }
        assert!(machine.state().is_terminal());
    }

    #[test]
    fn file_data_is_not_allowed_until_the_peer_has_accepted() {
        let mut machine = Machine::new();
        assert!(!machine.file_data_allowed());
        machine.apply(Event::RequestSent).expect("legal");
        assert!(
            !machine.file_data_allowed(),
            "allowed while awaiting accept"
        );
        machine.apply(Event::Accepted).expect("legal");
        assert!(!machine.file_data_allowed(), "allowed while connecting");
        machine.apply(Event::Connected).expect("legal");
        assert!(machine.file_data_allowed());
    }

    #[test]
    fn a_lost_connection_ends_the_transfer_rather_than_reconnecting() {
        // There is deliberately no way back from a broken transfer. Resuming is
        // a new transfer, with a new prompt (S-2, ADR-0020).
        let mut machine = Machine::new();
        for event in [Event::RequestSent, Event::Accepted, Event::Connected] {
            machine.apply(event).expect("legal");
        }
        machine.apply(Event::Fail).expect("legal");
        assert_eq!(machine.state(), State::Failed);
        assert!(machine.state().is_terminal());
        for event in Event::ALL {
            assert!(
                machine.apply(event).is_err(),
                "a failed transfer accepted {event:?}"
            );
        }
    }

    #[test]
    fn applying_an_illegal_event_reports_it_and_leaves_the_state_alone() {
        let mut machine = Machine::new();
        let error = machine.apply(Event::Verified).expect_err("illegal");
        assert_eq!(
            error,
            IllegalTransition {
                state: State::Requested,
                event: Event::Verified
            }
        );
        assert_eq!(machine.state(), State::Requested, "state moved on error");
    }

    #[test]
    fn a_finished_transfer_cannot_be_restarted() {
        let mut machine = Machine::new();
        machine.apply(Event::RequestSent).expect("legal");
        machine.apply(Event::Declined).expect("legal");
        assert_eq!(machine.state(), State::Rejected);
        assert!(machine.apply(Event::Accepted).is_err());
        assert!(machine.apply(Event::Cancel).is_err());
        assert_eq!(machine.state(), State::Rejected);
    }
}
