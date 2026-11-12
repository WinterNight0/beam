//! The file going out: one at a time, from pick to "saved on their side"
//! (ADR-0043). What it shows is the engine's progress, put into the stages
//! a person follows: looking for them, reading the file, waiting for their
//! yes, sending, their check.

use beam_core::transfer::Progress;
use beam_core::transport::PathKind;

/// What the send reports, as it runs ([`beam_core::actions::send`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    /// Looking for the friend.
    Dialling,
    Progress(Progress),
    /// How it ended, in words for the person.
    Finished(Result<String, String>),
}

/// Where a send has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Starting,
    Dialling,
    /// Reading the file to hash it.
    Hashing {
        done: u64,
        total: u64,
    },
    /// The request is with them; a person has to say yes.
    Waiting,
    Moving {
        done: u64,
        total: u64,
        relay: bool,
    },
    /// They are checking the whole file.
    Checking {
        done: u64,
        total: u64,
    },
}

/// The one send running at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sending {
    pub peer: String,
    pub file: String,
    pub stage: Stage,
    /// Set when it ended and the front end keeps it on screen.
    pub result: Option<Result<String, String>>,
}

impl Sending {
    /// A send just started.
    pub fn new(peer: String, file: String) -> Self {
        Self {
            peer,
            file,
            stage: Stage::Starting,
            result: None,
        }
    }

    pub fn running(&self) -> bool {
        self.result.is_none()
    }

    /// Moves the stage on. When the send has ended, returns how, for the
    /// front end to show (and to keep in `result` if it stays on screen).
    pub fn on_update(&mut self, update: Update) -> Option<Result<String, String>> {
        match update {
            Update::Dialling => self.stage = Stage::Dialling,
            Update::Progress(progress) => {
                self.stage = match progress {
                    Progress::Hashing { done, total } => Stage::Hashing { done, total },
                    Progress::AwaitingAccept => Stage::Waiting,
                    Progress::Accepted { path } => Stage::Moving {
                        done: 0,
                        total: 0,
                        relay: path == PathKind::Relay,
                    },
                    Progress::Transferring { done, total, path } => Stage::Moving {
                        done,
                        total,
                        relay: path == PathKind::Relay,
                    },
                    Progress::PeerVerifying { done, total } => Stage::Checking { done, total },
                    _ => self.stage,
                };
            }
            Update::Finished(result) => return Some(result),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sending only starts moving once they said yes; before that it waits.
    #[test]
    fn the_stages_follow_the_engine_and_the_end_is_handed_back() {
        let mut s = Sending::new("alice".into(), "report.pdf".into());
        assert!(s.running());
        assert_eq!(s.on_update(Update::Dialling), None);
        assert_eq!(s.stage, Stage::Dialling);
        s.on_update(Update::Progress(Progress::AwaitingAccept));
        assert_eq!(s.stage, Stage::Waiting);
        s.on_update(Update::Progress(Progress::Transferring {
            done: 5,
            total: 10,
            path: PathKind::Relay,
        }));
        assert_eq!(
            s.stage,
            Stage::Moving {
                done: 5,
                total: 10,
                relay: true
            }
        );
        assert_eq!(
            s.on_update(Update::Finished(Ok("sent".into()))),
            Some(Ok("sent".into()))
        );
        assert!(s.running(), "the front end decides whether to keep it");
    }
}
