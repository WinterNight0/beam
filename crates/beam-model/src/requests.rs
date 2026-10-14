//! Receiving without `beam listen`: the requests waiting for an answer,
//! the file arriving now, the link to the receiver, and the Receiving switch
//! (ADR-0042, ADR-0043, ADR-0044).
//!
//! Requests never open a question by themselves: one arriving while someone
//! types must not catch a stray Enter. A front end shows that they are
//! there, and opens [`crate::ask::Incoming`] only when the person asks. An
//! answered request leaves the list at once, so it cannot be answered twice;
//! the receiver still has the last word (a late answer comes back as
//! [`Update::TooLate`]).
//!
//! The methods here change the state and say what happened; the words for
//! it are the front end's.

use std::time::{Duration, Instant};

use beam_core::actions::inbox::RequestView;

use crate::snapshot::{Agent, Owner, Snapshot};

/// What the link to the receiver reports (the background agent, or this
/// window's Receiving switch). Defined beside the link that sends it, in
/// `beam-core`, so the view and the GUI share one link (ADR-0048).
pub use beam_core::actions::inbox_link::LinkUpdate as Update;

/// What the Receiving switch's receiver reports; defined beside it.
pub use beam_core::actions::switch::SwitchUpdate;

/// A request waiting for an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub id: u64,
    pub request: RequestView,
    pub expires: Instant,
}

/// A file arriving now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receiving {
    pub done: u64,
    pub total: u64,
    pub relay: bool,
}

/// The Receiving switch (ADR-0044). Off at every start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Switch {
    #[default]
    Off,
    Starting,
    On,
    Stopping,
    /// It stopped by itself, or could not start: why.
    Failed(String),
}

/// What turning the switch on would collide with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// The background agent already receives.
    Agent,
    /// Another beam window's switch is on.
    OtherView,
    /// `beam listen` runs in another terminal.
    Listen,
}

/// How things stand with the receiver.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Link {
    /// Not running, or not tried yet.
    #[default]
    Off,
    Connecting,
    Connected,
    /// It was connected and went away, or could not be reached.
    Lost(String),
}

/// What an [`Update`] did, for the front end to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Happened {
    Connected,
    /// A new request is waiting (never opened by itself).
    Asked {
        peer_name: String,
        file_name: String,
    },
    /// Request `id` closed; `was_waiting` if it was still in the list.
    Closed {
        id: u64,
        was_waiting: bool,
    },
    /// An answer came too late and did not count.
    TooLate,
    Progress,
    /// A file finished arriving (or failed); `text` is cleaned.
    Finished {
        text: String,
    },
    /// The link is gone, and every request with it.
    Gone,
}

/// What pressing the Receiving switch does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// Turning on: start the receiver.
    Start,
    /// Turning off: stop it.
    Stop,
    /// A file is arriving: ask first ([`crate::ask::StopMidTransfer`]).
    AskFirst,
    /// Someone else already receives; nothing changed.
    Taken(Taken),
    /// It is starting or stopping already; nothing changed.
    Busy,
}

/// The receiving side of the screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Requests {
    /// Waiting for an answer, oldest first.
    pub waiting: Vec<Pending>,
    /// Index into `waiting`.
    pub selected: usize,
    /// What is arriving now.
    pub receiving: Option<Receiving>,
    pub link: Link,
    pub switch: Switch,
}

impl Requests {
    /// Applies what the receiver said. `now` dates a new request's expiry.
    pub fn on_update(&mut self, update: Update, now: Instant) -> Happened {
        let happened = match update {
            Update::Connected { .. } => {
                self.link = Link::Connected;
                Happened::Connected
            }
            Update::Request {
                id,
                request,
                expires_in,
            } => {
                let happened = Happened::Asked {
                    peer_name: request.peer_name.clone(),
                    file_name: request.file_name.clone(),
                };
                self.waiting.push(Pending {
                    id,
                    request,
                    expires: now + expires_in,
                });
                happened
            }
            Update::Closed { id } => Happened::Closed {
                id,
                was_waiting: self.forget(id),
            },
            Update::TooLate { .. } => Happened::TooLate,
            Update::Progress { done, total, relay } => {
                self.receiving = Some(Receiving { done, total, relay });
                Happened::Progress
            }
            Update::Finished { text, .. } => {
                self.receiving = None;
                Happened::Finished { text }
            }
            Update::Gone { reason } => {
                self.waiting.clear();
                self.receiving = None;
                self.link = Link::Lost(reason);
                Happened::Gone
            }
        };
        self.keep_selection_in_range();
        happened
    }

    /// Drops requests whose time ran out (with two seconds' grace), in case
    /// the receiver's own "closed" never comes. Returns their ids.
    pub fn prune(&mut self, now: Instant) -> Vec<u64> {
        let expired: Vec<u64> = self
            .waiting
            .iter()
            .filter(|p| p.expires + Duration::from_secs(2) < now)
            .map(|p| p.id)
            .collect();
        for id in &expired {
            self.forget(*id);
        }
        expired
    }

    /// The person answered request `id`: it leaves the list at once, so it
    /// cannot be answered twice here.
    pub fn answered(&mut self, id: u64) {
        self.forget(id);
    }

    /// Requests from the friend with this fingerprint, oldest first.
    pub fn from(&self, fingerprint: &str) -> Vec<&Pending> {
        self.waiting
            .iter()
            .filter(|p| p.request.fingerprint == fingerprint)
            .collect()
    }

    /// Whether a file is arriving through this view's switch.
    pub fn arriving_here(&self) -> bool {
        self.switch == Switch::On && self.receiving.is_some()
    }

    /// Who else receives for this beam home, which makes the switch moot.
    pub fn receiving_elsewhere(&self, snapshot: &Snapshot) -> Option<Taken> {
        let mine = matches!(
            self.switch,
            Switch::Starting | Switch::On | Switch::Stopping
        );
        match (&snapshot.agent, snapshot.owner) {
            (Agent::Running { .. } | Agent::Unreadable, Owner::Background) if !mine => {
                Some(Taken::Agent)
            }
            (Agent::Running { .. }, Owner::OtherView) => Some(Taken::OtherView),
            _ if snapshot.listen_elsewhere => Some(Taken::Listen),
            _ => None,
        }
    }

    /// The person pressed the switch.
    pub fn toggle(&mut self, snapshot: &Snapshot) -> Toggle {
        match self.switch {
            Switch::On if self.receiving.is_some() => Toggle::AskFirst,
            Switch::On => self.stop(),
            Switch::Starting | Switch::Stopping => Toggle::Busy,
            Switch::Off | Switch::Failed(_) => {
                if let Some(taken) = self.receiving_elsewhere(snapshot) {
                    return Toggle::Taken(taken);
                }
                self.switch = Switch::Starting;
                Toggle::Start
            }
        }
    }

    /// Stops receiving: after a yes to [`crate::ask::StopMidTransfer`], or
    /// from [`Self::toggle`] when nothing is arriving.
    pub fn stop(&mut self) -> Toggle {
        self.switch = Switch::Stopping;
        Toggle::Stop
    }

    /// Applies what the switch's receiver said.
    pub fn on_switch(&mut self, update: &SwitchUpdate) {
        self.switch = match update {
            SwitchUpdate::Started { .. } => Switch::On,
            SwitchUpdate::Stopped(None) => Switch::Off,
            SwitchUpdate::Stopped(Some(why)) => Switch::Failed(why.clone()),
        };
    }

    fn forget(&mut self, id: u64) -> bool {
        let before = self.waiting.len();
        self.waiting.retain(|p| p.id != id);
        self.keep_selection_in_range();
        self.waiting.len() != before
    }

    fn keep_selection_in_range(&mut self) {
        self.selected = self.selected.min(self.waiting.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: u64, who: &str) -> Update {
        Update::Request {
            id,
            request: RequestView {
                peer_name: who.into(),
                fingerprint: format!("{who}-fp"),
                file_name: "report.pdf".into(),
                size: 10,
                resume: None,
            },
            expires_in: Duration::from_secs(300),
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            me: None,
            needs_setup: false,
            friends: Vec::new(),
            agent: Agent::Stopped,
            history: Vec::new(),
            owner: Owner::Background,
            listen_elsewhere: false,
            problem: None,
        }
    }

    /// A request is only listed: nothing in the model answers it.
    #[test]
    fn a_request_waits_until_answered_and_cannot_be_answered_twice() {
        let mut r = Requests::default();
        let now = Instant::now();
        assert!(matches!(
            r.on_update(request(1, "alice"), now),
            Happened::Asked { .. }
        ));
        r.on_update(request(2, "bob"), now);
        assert_eq!(r.waiting.len(), 2);
        assert_eq!(r.from("bob-fp").len(), 1);
        r.answered(1);
        assert_eq!(r.waiting.len(), 1);
        assert_eq!(
            r.on_update(Update::Closed { id: 1 }, now),
            Happened::Closed {
                id: 1,
                was_waiting: false
            }
        );
    }

    #[test]
    fn expired_requests_go_even_without_word_from_the_receiver() {
        let mut r = Requests::default();
        let now = Instant::now();
        r.on_update(request(1, "alice"), now);
        assert!(r.prune(now + Duration::from_secs(301)).is_empty(), "grace");
        assert_eq!(r.prune(now + Duration::from_secs(303)), vec![1]);
        assert!(r.waiting.is_empty());
    }

    #[test]
    fn a_lost_link_takes_every_request_with_it() {
        let mut r = Requests::default();
        let now = Instant::now();
        r.on_update(request(1, "alice"), now);
        r.on_update(
            Update::Progress {
                done: 1,
                total: 2,
                relay: false,
            },
            now,
        );
        assert_eq!(
            r.on_update(Update::Gone { reason: "x".into() }, now),
            Happened::Gone
        );
        assert!(r.waiting.is_empty() && r.receiving.is_none());
        assert_eq!(r.link, Link::Lost("x".into()));
    }

    /// Turning Receiving off while a file arrives asks first; the switch
    /// does not fight the agent or `listen`.
    #[test]
    fn the_switch_asks_before_cutting_off_a_file_and_never_fights() {
        let mut r = Requests::default();
        let mut snap = snapshot();
        assert_eq!(r.toggle(&snap), Toggle::Start);
        assert_eq!(r.toggle(&snap), Toggle::Busy);
        r.on_switch(&SwitchUpdate::Started {
            receive_dir: "in".into(),
        });
        r.receiving = Some(Receiving {
            done: 1,
            total: 2,
            relay: false,
        });
        assert!(r.arriving_here());
        assert_eq!(r.toggle(&snap), Toggle::AskFirst);
        assert_eq!(r.switch, Switch::On, "asking changes nothing");

        let mut off = Requests::default();
        snap.listen_elsewhere = true;
        assert_eq!(off.toggle(&snap), Toggle::Taken(Taken::Listen));
        snap.listen_elsewhere = false;
        snap.agent = Agent::Running {
            receive_dir: "in".into(),
        };
        assert_eq!(off.toggle(&snap), Toggle::Taken(Taken::Agent));
        assert_eq!(off.switch, Switch::Off);
    }
}
