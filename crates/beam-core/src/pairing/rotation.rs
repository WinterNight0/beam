//! The pairing code `beam listen` offers, and how it changes.
//!
//! `pair --wait` shows one code and exits after one attempt. `listen` runs for
//! hours, so its code has to renew itself (M5 decision 1), and that removes
//! the person who had to act between guesses. These rules put a bound back
//! (S-23, ADR-0028):
//!
//! * A code works for **one attempt** and for **ten minutes**; after either, a
//!   new one is issued and printed.
//! * After an attempt in which the other side **did not prove the code**, the
//!   next code only appears after a delay: 5 s, then 10 s, doubling, capped
//!   at five minutes.
//! * After **three such failures in a row**, pairing is off for the rest of
//!   this `listen` session. Transfers from paired peers are unaffected.
//!   Restarting `listen` turns pairing back on.
//! * An attempt that proved the code — whatever the people then answered —
//!   resets the count: it was not a guess.
//!
//! This is a pure state machine with the clock passed in, so every rule is
//! tested without waiting for it.

use std::time::{Duration, Instant};

use super::code::{DEFAULT_CODE_TTL, PairingCode};

/// The numbers behind the rules. [`Policy::default`] is what `listen` uses.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub code_ttl: Duration,
    pub first_backoff: Duration,
    pub max_backoff: Duration,
    /// Consecutive failed attempts after which pairing is switched off.
    pub max_failures: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            code_ttl: DEFAULT_CODE_TTL,
            first_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(5 * 60),
            max_failures: 3,
        }
    }
}

/// How an attempt ended, as far as guessing is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    /// The other side proved the code; both people said yes.
    Paired,
    /// The other side proved the code, but somebody said no (or the device
    /// was already paired). Not a guess.
    ProvedButRefused,
    /// The other side did not prove the code: a wrong code, a dropped
    /// connection, or anything else. Counts as a guess.
    Failed,
}

/// Why no code can be handed out right now.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unavailable {
    #[error("another pairing attempt is in progress")]
    InProgress,
    #[error("pairing is paused for {} s after a failed attempt", .0.as_secs().max(1))]
    CoolingDown(Duration),
    #[error(
        "pairing is off after {0} failed attempts; it comes back when `beam listen` is restarted"
    )]
    Disabled(u32),
}

/// Something `listen` should tell the person at the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// A new code is live.
    NewCode {
        code: PairingCode,
        reason: NewCodeReason,
    },
    /// The last attempt failed; the next code comes after this long.
    CoolingDown { failures: u32, wait: Duration },
    /// Pairing is off for the rest of this session.
    Disabled { failures: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewCodeReason {
    /// The first code of the session.
    Start,
    /// The last code was used by an attempt.
    Used,
    /// The last code was shown for its full ten minutes without being used.
    Expired,
    /// A cooling-down period is over.
    CooledDown,
}

#[derive(Debug)]
enum State {
    Live { code: PairingCode, expires: Instant },
    InUse,
    CoolingDown { until: Instant },
    Disabled,
}

/// The code `listen` offers, and the rules for renewing it.
#[derive(Debug)]
pub struct Rotation {
    policy: Policy,
    state: State,
    failures: u32,
}

impl Rotation {
    /// Starts with a fresh code. The notice carries the code to print.
    pub fn start(policy: Policy, now: Instant) -> Result<(Self, Notice), getrandom::Error> {
        let code = PairingCode::generate()?;
        let rotation = Self {
            policy,
            state: State::Live {
                code: code.clone(),
                expires: now + policy.code_ttl,
            },
            failures: 0,
        };
        Ok((
            rotation,
            Notice::NewCode {
                code,
                reason: NewCodeReason::Start,
            },
        ))
    }

    /// Moves time forward: an expired code is replaced, a finished cool-down
    /// issues a code. Call it about once a second.
    pub fn tick(&mut self, now: Instant) -> Result<Option<Notice>, getrandom::Error> {
        let reason = match &self.state {
            State::Live { expires, .. } if now >= *expires => NewCodeReason::Expired,
            State::CoolingDown { until } if now >= *until => NewCodeReason::CooledDown,
            _ => return Ok(None),
        };
        Ok(Some(self.issue(now, reason)?))
    }

    /// Spends the code on an attempt that is starting now.
    pub fn take(&mut self, now: Instant) -> Result<PairingCode, Unavailable> {
        match &self.state {
            State::Live { expires, .. } if now >= *expires => {
                // Expired but not yet ticked over: not usable.
                Err(Unavailable::CoolingDown(Duration::from_secs(1)))
            }
            State::Live { .. } => match std::mem::replace(&mut self.state, State::InUse) {
                State::Live { code, .. } => Ok(code),
                _ => unreachable!("matched Live above"),
            },
            State::InUse => Err(Unavailable::InProgress),
            State::CoolingDown { until } => Err(Unavailable::CoolingDown(
                until.saturating_duration_since(now),
            )),
            State::Disabled => Err(Unavailable::Disabled(self.failures)),
        }
    }

    /// Records how the attempt that took the code ended, and says what
    /// happens next.
    pub fn finish(&mut self, attempt: Attempt, now: Instant) -> Result<Notice, getrandom::Error> {
        debug_assert!(matches!(self.state, State::InUse), "finish without take");
        match attempt {
            Attempt::Paired | Attempt::ProvedButRefused => {
                self.failures = 0;
                self.issue(now, NewCodeReason::Used)
            }
            Attempt::Failed => {
                self.failures += 1;
                if self.failures >= self.policy.max_failures {
                    self.state = State::Disabled;
                    return Ok(Notice::Disabled {
                        failures: self.failures,
                    });
                }
                let wait = self.backoff();
                self.state = State::CoolingDown { until: now + wait };
                Ok(Notice::CoolingDown {
                    failures: self.failures,
                    wait,
                })
            }
        }
    }

    /// Whether pairing has been switched off for this session.
    pub fn is_disabled(&self) -> bool {
        matches!(self.state, State::Disabled)
    }

    /// The code currently on offer, for display.
    pub fn code(&self) -> Option<&PairingCode> {
        match &self.state {
            State::Live { code, .. } => Some(code),
            _ => None,
        }
    }

    /// 5 s after the first failure, doubling, capped.
    fn backoff(&self) -> Duration {
        let doublings = self.failures.saturating_sub(1).min(16);
        (self.policy.first_backoff * 2u32.pow(doublings)).min(self.policy.max_backoff)
    }

    fn issue(&mut self, now: Instant, reason: NewCodeReason) -> Result<Notice, getrandom::Error> {
        let code = PairingCode::generate()?;
        self.state = State::Live {
            code: code.clone(),
            expires: now + self.policy.code_ttl,
        };
        Ok(Notice::NewCode { code, reason })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> (Rotation, Instant) {
        let now = Instant::now();
        let (rotation, notice) = Rotation::start(Policy::default(), now).unwrap();
        assert!(matches!(
            notice,
            Notice::NewCode {
                reason: NewCodeReason::Start,
                ..
            }
        ));
        (rotation, now)
    }

    fn new_code(notice: &Notice) -> &PairingCode {
        match notice {
            Notice::NewCode { code, .. } => code,
            other => panic!("expected a new code, got {other:?}"),
        }
    }

    #[test]
    fn a_used_code_is_replaced_by_a_new_one() {
        let (mut r, now) = started();
        let first = r.take(now).unwrap();
        let notice = r.finish(Attempt::Paired, now).unwrap();
        let second = new_code(&notice).clone();
        assert_ne!(first, second, "a used code was offered again");
        assert_eq!(r.code(), Some(&second));
    }

    #[test]
    fn an_unused_code_expires_after_ten_minutes() {
        let (mut r, now) = started();
        let first = r.code().unwrap().clone();
        assert_eq!(r.tick(now + Duration::from_secs(599)).unwrap(), None);
        let notice = r.tick(now + Duration::from_secs(600)).unwrap().unwrap();
        assert!(matches!(
            notice,
            Notice::NewCode {
                reason: NewCodeReason::Expired,
                ..
            }
        ));
        assert_ne!(new_code(&notice), &first);
    }

    #[test]
    fn an_expired_code_cannot_be_taken_even_before_the_tick() {
        let (mut r, now) = started();
        assert!(r.take(now + Duration::from_secs(600)).is_err());
    }

    #[test]
    fn one_attempt_at_a_time() {
        let (mut r, now) = started();
        r.take(now).unwrap();
        assert_eq!(r.take(now), Err(Unavailable::InProgress));
    }

    #[test]
    fn a_failed_attempt_pauses_pairing_then_the_pause_doubles() {
        let (mut r, t0) = started();

        r.take(t0).unwrap();
        let notice = r.finish(Attempt::Failed, t0).unwrap();
        assert_eq!(
            notice,
            Notice::CoolingDown {
                failures: 1,
                wait: Duration::from_secs(5)
            }
        );
        assert_eq!(r.code(), None);
        assert!(matches!(
            r.take(t0 + Duration::from_secs(4)),
            Err(Unavailable::CoolingDown(_))
        ));
        assert_eq!(r.tick(t0 + Duration::from_secs(4)).unwrap(), None);

        let t1 = t0 + Duration::from_secs(5);
        let notice = r.tick(t1).unwrap().unwrap();
        assert!(matches!(
            notice,
            Notice::NewCode {
                reason: NewCodeReason::CooledDown,
                ..
            }
        ));

        r.take(t1).unwrap();
        assert_eq!(
            r.finish(Attempt::Failed, t1).unwrap(),
            Notice::CoolingDown {
                failures: 2,
                wait: Duration::from_secs(10)
            }
        );
    }

    /// Condition 1 of the M5 approval.
    #[test]
    fn three_failures_in_a_row_turn_pairing_off_for_the_session() {
        let (mut r, mut now) = started();
        for failure in 1..=3 {
            r.take(now).unwrap();
            let notice = r.finish(Attempt::Failed, now).unwrap();
            if failure < 3 {
                now += Duration::from_secs(3600);
                r.tick(now).unwrap();
            } else {
                assert_eq!(notice, Notice::Disabled { failures: 3 });
            }
        }
        assert!(r.is_disabled());
        // It stays off, however long listen keeps running.
        for later in [1, 600, 86_400] {
            let t = now + Duration::from_secs(later);
            assert_eq!(r.tick(t).unwrap(), None);
            assert_eq!(r.take(t), Err(Unavailable::Disabled(3)));
        }
    }

    #[test]
    fn a_proved_code_resets_the_count_even_if_someone_said_no() {
        let (mut r, mut now) = started();
        for _ in 0..2 {
            r.take(now).unwrap();
            r.finish(Attempt::Failed, now).unwrap();
            now += Duration::from_secs(3600);
            r.tick(now).unwrap();
        }
        // Someone who knew the code, but was refused at the prompt.
        r.take(now).unwrap();
        r.finish(Attempt::ProvedButRefused, now).unwrap();
        // Two more failures are then not enough to switch pairing off.
        for _ in 0..2 {
            r.take(now).unwrap();
            let notice = r.finish(Attempt::Failed, now).unwrap();
            assert!(matches!(notice, Notice::CoolingDown { .. }), "{notice:?}");
            now += Duration::from_secs(3600);
            r.tick(now).unwrap();
        }
        assert!(!r.is_disabled());
    }

    #[test]
    fn the_backoff_is_capped() {
        let policy = Policy {
            max_failures: 100,
            ..Policy::default()
        };
        let mut now = Instant::now();
        let (mut r, _) = Rotation::start(policy, now).unwrap();
        let mut last = Duration::ZERO;
        for _ in 0..20 {
            r.take(now).unwrap();
            if let Notice::CoolingDown { wait, .. } = r.finish(Attempt::Failed, now).unwrap() {
                last = wait;
            }
            now += Duration::from_secs(3600);
            r.tick(now).unwrap();
        }
        assert_eq!(last, Duration::from_secs(300));
    }
}
