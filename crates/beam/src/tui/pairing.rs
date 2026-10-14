//! Pairing from the full-screen view (ADR-0043): the shared pairing worker
//! (`beam_core::actions::pair_worker`, ADR-0048), which the GUI runs too,
//! and the view's own words for how it ended.
//!
//! The worker runs the same flow as `beam pair` on a background thread; the
//! view shows its [`Update`]s and sends [`Answer`]s back. A question left
//! unanswered, or abandoned by closing the view, is a no.

use beam_core::actions::pair_worker::CodeAfter;
pub use beam_core::actions::pair_worker::{Answer, Paired, Unpaired, Update, Worker};

/// How a pairing that saved nothing is told in the view, as `beam pair`
/// tells it. The reason is already cleaned by the worker.
pub fn words(unpaired: &Unpaired) -> String {
    match unpaired {
        Unpaired::Failed(reason) => reason.clone(),
        Unpaired::NotPaired { reason, code } => {
            let hint = match code {
                CodeAfter::Nothing => "",
                CodeAfter::UsedUpHere => {
                    "\nThe code is used up. Choose Show my invite again for a new one."
                }
                CodeAfter::UsedUpThere => {
                    "\nThe other device's code is now used up. Ask them for the new one."
                }
            };
            format!("Not paired: {reason}. Nothing was saved.{hint}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The view's wording is unchanged by the move to the shared worker.
    #[test]
    fn a_failed_pairing_is_worded_as_before() {
        assert_eq!(
            words(&Unpaired::NotPaired {
                reason: "wrong code".into(),
                code: CodeAfter::UsedUpHere,
            }),
            "Not paired: wrong code. Nothing was saved.\n\
             The code is used up. Choose Show my invite again for a new one."
        );
        assert_eq!(
            words(&Unpaired::NotPaired {
                reason: "declined".into(),
                code: CodeAfter::Nothing,
            }),
            "Not paired: declined. Nothing was saved."
        );
        assert_eq!(
            words(&Unpaired::Failed("no identity".into())),
            "no identity"
        );
    }
}
