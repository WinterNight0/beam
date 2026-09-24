//! The beam command-line interface.

use std::io::{self, BufReader, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let stdin = io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let stderr = io::stderr();
    let mut err = stderr.lock();

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
