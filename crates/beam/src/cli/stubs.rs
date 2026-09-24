//! Commands that are declared now but land in a later milestone.
//!
//! They appear in `beam --help` from M0 on, so the intended surface is visible,
//! and each fails loudly with exit code 2 rather than pretending to work.

use super::{CommandError, Io};

/// Builds the error a stubbed command returns.
pub(super) fn not_implemented(command: &'static str, milestone: &'static str) -> CommandError {
    CommandError::NotImplemented { command, milestone }
}

/// `beam version`.
pub(super) fn version(io: &mut Io<'_>) -> Result<(), CommandError> {
    writeln!(io.out, "beam {}", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
