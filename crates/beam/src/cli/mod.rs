//! The beam command tree.
//!
//! Commands live here rather than in the binary so they can be exercised by
//! tests with in-memory streams and a temporary home directory; see ADR-0002.

mod identity_cmds;
mod stubs;

use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};

use crate::identity::{PeerError, Store, StoreError};

/// Process exit codes.
pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_NOT_IMPLEMENTED: i32 = 2;

const LONG_ABOUT: &str = "\
beam sends files directly between two computers.

A peer must be paired before it can send you anything, and every incoming
transfer has to be accepted by hand. There is no auto-accept.";

/// The streams a command reads from and writes to.
///
/// Passing these explicitly is what lets the tests drive whole commands
/// without spawning a process.
pub struct Io<'a> {
    pub input: &'a mut dyn BufRead,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// Why a command failed.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("`beam {command}` is not implemented yet (planned for milestone {milestone})")]
    NotImplemented {
        command: &'static str,
        milestone: &'static str,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Peer(#[from] PeerError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Parser)]
#[command(
    name = "beam",
    version,
    about = "Identity-based peer-to-peer file transfer",
    long_about = LONG_ABOUT
)]
struct Cli {
    /// beam home directory (default $BEAM_DIR, else ~/.beam)
    #[arg(long, global = true, value_name = "PATH")]
    beam_dir: Option<PathBuf>,

    /// Machine-readable JSON output
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate this device's identity keypair
    ///
    /// The private key never leaves this machine and is never sent to the
    /// signaling server. Run this once per machine.
    Init {
        /// Replace an existing identity (invalidates every existing pairing)
        #[arg(long)]
        force: bool,
    },

    /// Show this device's Short ID and fingerprint
    Whoami,

    /// List paired peers and their fingerprints
    Peers,

    /// Change the local nickname of a paired peer
    ///
    /// Only the local nickname changes. The peer's key is untouched, and the
    /// peer is not notified: names are local labels, not identities.
    Rename {
        /// The peer's current nickname
        old_name: String,
        /// The nickname to use from now on
        new_name: String,
    },

    /// Forget a paired peer
    ///
    /// The peer can no longer send you files, and pairing with it again
    /// requires a fresh pairing code.
    Remove {
        /// The peer to forget
        name: String,
        /// Do not ask for confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Pair with a peer for the first time using its Short ID and pairing code
    Pair {
        /// The peer's 9-digit Short ID
        short_id: String,
        /// Local nickname to store the peer under
        #[arg(long)]
        name: Option<String>,
    },

    /// Wait for incoming transfers and show the pairing code
    Listen,

    /// Send a file to a paired peer
    Send {
        /// The nickname of a paired peer
        peer: String,
        /// The file to send
        file: PathBuf,
    },

    /// Regenerate this device's pairing code
    Newcode,

    /// Show the beam version
    Version,
}

/// The state shared by every command.
pub(crate) struct App {
    pub(crate) store: Store,
    pub(crate) json: bool,
}

/// Runs the command tree and returns the process exit code.
pub fn execute<I, T>(args: I, io: &mut Io<'_>) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut argv: Vec<OsString> = vec![OsString::from("beam")];
    argv.extend(args.into_iter().map(Into::into));

    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(err) => {
            // Help and version are a successful outcome, and belong on stdout.
            return match err.kind() {
                ErrorKind::DisplayHelp
                | ErrorKind::DisplayVersion
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    let _ = write!(io.out, "{err}");
                    EXIT_OK
                }
                _ => {
                    let _ = write!(io.err, "{err}");
                    EXIT_ERROR
                }
            };
        }
    };

    let dir = match cli.beam_dir.clone() {
        Some(dir) => dir,
        None => match Store::default_dir() {
            Ok(dir) => dir,
            Err(e) => {
                let _ = writeln!(io.err, "beam: {e}");
                return EXIT_ERROR;
            }
        },
    };

    let app = App {
        store: Store::new(dir),
        json: cli.json,
    };

    let result = match cli.command {
        Some(command) => app.run(command, io),
        None => {
            let _ = write!(io.out, "{}", Cli::command().render_long_help());
            Ok(())
        }
    };

    match result {
        Ok(()) => EXIT_OK,
        Err(err) => {
            let _ = writeln!(io.err, "beam: {err}");
            match err {
                CommandError::NotImplemented { .. } => EXIT_NOT_IMPLEMENTED,
                _ => EXIT_ERROR,
            }
        }
    }
}

impl App {
    fn run(&self, command: Command, io: &mut Io<'_>) -> Result<(), CommandError> {
        match command {
            Command::Init { force } => self.init(force, io),
            Command::Whoami => self.whoami(io),
            Command::Peers => self.peers(io),
            Command::Rename { old_name, new_name } => self.rename(&old_name, &new_name, io),
            Command::Remove { name, yes } => self.remove(&name, yes, io),
            Command::Pair { .. } => Err(stubs::not_implemented("pair", "M4")),
            Command::Listen => Err(stubs::not_implemented("listen", "M2")),
            Command::Send { .. } => Err(stubs::not_implemented("send", "M2")),
            Command::Newcode => Err(stubs::not_implemented("newcode", "M4")),
            Command::Version => stubs::version(io),
        }
    }

    /// Prints any file-permission warnings to stderr.
    pub(crate) fn warn_permissions(&self, io: &mut Io<'_>) {
        for warning in self.store.permission_warnings() {
            let _ = writeln!(io.err, "beam: warning: {warning}");
        }
    }
}
