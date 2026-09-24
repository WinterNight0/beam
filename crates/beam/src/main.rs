//! The beam command-line interface.

use std::io::{self, BufReader, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    // These are the handles, not locks. `beam listen` prints its Accept prompt
    // from a separate thread, and holding a `StdoutLock` here for the whole run
    // would deadlock that thread the moment it tried to draw the prompt. The
    // handles lock for the duration of each write instead.
    let mut input = BufReader::new(io::stdin());
    let mut out = io::stdout();
    let mut err = io::stderr();

    let code = {
        let mut streams = beam::cli::Io {
            input: &mut input,
            out: &mut out,
            err: &mut err,
        };
        beam::cli::execute(std::env::args_os().skip(1), &mut streams)
    };

    let _ = out.flush();
    let _ = err.flush();
    ExitCode::from(code as u8)
}
