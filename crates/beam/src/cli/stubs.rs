//! Small commands with nothing else to live with.
//!
//! Until M5 this also held the stubs for commands planned for later
//! milestones. Every planned command now exists; `newcode` was dropped in M5
//! (ADR-0028), because `listen` renews its code by itself.

use super::{CommandError, Io};

/// `beam version`.
pub(super) fn version(io: &mut Io<'_>) -> Result<(), CommandError> {
    writeln!(io.out, "beam {}", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
