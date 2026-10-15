//! Pairing codes: six digits, single use, ten minutes.
//!
//! A pairing code is a weak password on purpose — a person reads it off one
//! screen and types it into another. SPAKE2 makes a weak password safe to use
//! over a network: an attacker learns nothing offline, and gets exactly one
//! guess per protocol run. That makes the number of runs the security
//! parameter, so a code is **burned by the first attempt that uses it**,
//! successful or not. One run, one guess, at most a one-in-a-million chance.
//! See ADR-0026.

use std::fmt;
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

/// How many digits a code has.
pub const CODE_DIGITS: usize = 6;

/// How long a code stays usable after it is shown.
pub const DEFAULT_CODE_TTL: Duration = Duration::from_secs(10 * 60);

/// 10^6: the number of distinct codes.
const CODE_SPACE: u32 = 1_000_000;

/// A pairing code. Its `Debug` output is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode(Zeroizing<String>);

/// Why typed text is not a pairing code.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("a pairing code is {CODE_DIGITS} digits, such as 123 456")]
pub struct ParseCodeError;

/// Why a code cannot be used.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodeUnavailable {
    #[error("the pairing code has expired")]
    Expired,
    #[error("the pairing code has already been used; each code works for one attempt")]
    Burned,
}

impl PairingCode {
    /// A fresh code, uniformly distributed over all six-digit strings.
    pub fn generate() -> Result<Self, getrandom::Error> {
        // Rejection sampling: 2^32 is not a multiple of 10^6, and plain `%`
        // would make the low codes very slightly more likely.
        let limit = u32::MAX - (u32::MAX % CODE_SPACE);
        loop {
            let mut bytes = [0u8; 4];
            getrandom::fill(&mut bytes)?;
            let value = u32::from_be_bytes(bytes);
            if value < limit {
                return Ok(Self(Zeroizing::new(format!("{:06}", value % CODE_SPACE))));
            }
        }
    }

    /// Reads a code as a person typed it. Spaces and dashes are ignored, so
    /// `123 456` and `123-456` both work.
    pub fn parse(text: &str) -> Result<Self, ParseCodeError> {
        let digits: String = text
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect();
        if digits.len() != CODE_DIGITS || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseCodeError);
        }
        Ok(Self(Zeroizing::new(digits)))
    }

    /// The six digits.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The code as it is shown: `123 456`.
    pub fn grouped(&self) -> String {
        format!("{} {}", &self.0[..3], &self.0[3..])
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingCode(<redacted>)")
    }
}

/// The waiting side's one code, and the rules for spending it.
///
/// [`CodeSlot::take`] hands the code out at most once. It is called when an
/// attempt *starts*, before anything about the attempt is known, so a wrong
/// guess, a dropped connection and a success all use it up alike.
#[derive(Debug)]
pub struct CodeSlot {
    code: Option<PairingCode>,
    issued: Instant,
    ttl: Duration,
}

impl CodeSlot {
    /// A slot holding `code`, issued now.
    pub fn new(code: PairingCode, ttl: Duration) -> Self {
        Self::issued_at(code, ttl, Instant::now())
    }

    /// A slot holding `code`, issued at `issued`. For tests.
    pub fn issued_at(code: PairingCode, ttl: Duration, issued: Instant) -> Self {
        Self {
            code: Some(code),
            issued,
            ttl,
        }
    }

    /// When the code stops working.
    pub fn expires_at(&self) -> Instant {
        self.issued + self.ttl
    }

    /// A copy of the code, for showing it, without spending it. `None` once it
    /// has been taken.
    pub fn peek(&self) -> Option<PairingCode> {
        self.code.clone()
    }

    /// Whether the code has expired at `now`.
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at()
    }

    /// Spends the code for one attempt.
    ///
    /// Expiry is checked first, so an expired code reports as expired even if
    /// it was never used.
    pub fn take(&mut self, now: Instant) -> Result<PairingCode, CodeUnavailable> {
        if self.is_expired(now) {
            self.code = None;
            return Err(CodeUnavailable::Expired);
        }
        self.code.take().ok_or(CodeUnavailable::Burned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_are_six_digits() {
        for _ in 0..200 {
            let code = PairingCode::generate().unwrap();
            assert_eq!(code.as_str().len(), CODE_DIGITS);
            assert!(code.as_str().bytes().all(|b| b.is_ascii_digit()));
        }
    }

    #[test]
    fn generated_codes_differ() {
        let codes: std::collections::HashSet<String> = (0..50)
            .map(|_| PairingCode::generate().unwrap().as_str().to_string())
            .collect();
        // 50 draws from a million: a collision is possible but the whole set
        // collapsing to a handful is not.
        assert!(codes.len() > 45, "{} distinct codes of 50", codes.len());
    }

    #[test]
    fn a_code_keeps_its_leading_zeros() {
        let code = PairingCode::parse("004 211").unwrap();
        assert_eq!(code.as_str(), "004211");
        assert_eq!(code.grouped(), "004 211");
    }

    #[test]
    fn typed_codes_may_have_spaces_and_dashes() {
        for typed in ["123456", "123 456", "123-456", " 12 34 56 "] {
            assert_eq!(PairingCode::parse(typed).unwrap().as_str(), "123456");
        }
    }

    #[test]
    fn malformed_codes_are_refused() {
        for typed in ["", "12345", "1234567", "12345a", "１２３４５６", "123.456"] {
            assert_eq!(PairingCode::parse(typed), Err(ParseCodeError), "{typed:?}");
        }
    }

    #[test]
    fn debug_output_does_not_show_the_code() {
        let code = PairingCode::parse("987654").unwrap();
        assert!(!format!("{code:?}").contains("987654"));
    }

    #[test]
    fn a_code_can_be_taken_once() {
        let now = Instant::now();
        let mut slot =
            CodeSlot::issued_at(PairingCode::parse("111222").unwrap(), DEFAULT_CODE_TTL, now);

        assert_eq!(slot.take(now).unwrap().as_str(), "111222");
        assert_eq!(slot.take(now), Err(CodeUnavailable::Burned));
        assert_eq!(slot.take(now), Err(CodeUnavailable::Burned));
    }

    #[test]
    fn a_code_expires_after_its_ttl() {
        let issued = Instant::now();
        let ttl = Duration::from_secs(600);
        let mut slot = CodeSlot::issued_at(PairingCode::parse("111222").unwrap(), ttl, issued);

        assert!(!slot.is_expired(issued + ttl - Duration::from_secs(1)));
        assert!(slot.is_expired(issued + ttl));
        assert_eq!(slot.take(issued + ttl), Err(CodeUnavailable::Expired));
        // And it stays gone: expiry is not undone by asking again.
        assert_eq!(slot.take(issued), Err(CodeUnavailable::Burned));
    }

    #[test]
    fn the_default_ttl_is_ten_minutes() {
        assert_eq!(DEFAULT_CODE_TTL, Duration::from_secs(600));
    }
}
