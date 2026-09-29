//! The beam command tree.
//!
//! Commands live here rather than in the binary so they can be exercised by
//! tests with in-memory streams and a temporary home directory; see ADR-0002.

pub mod desk;
mod identity_cmds;
mod net_cmds;
mod stubs;
mod terminal;
mod transfer_cmds;

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

    /// Boxed because a transfer error is much larger than the others, and an
    /// enum is as big as its biggest variant.
    #[error(transparent)]
    Transfer(Box<crate::transfer::TransferError>),

    #[error(transparent)]
    Partial(Box<crate::transfer::PartialError>),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Parser)]
#[command(
    name = "beam",
    version,
    about = "Direct peer-to-peer file transfer",
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
    /// The private key never leaves this machine. Stage 1 keeps identity
    /// available for the later authenticated peer layer.
    Init {
        /// Replace an existing identity (changes the local identity key)
        #[arg(long)]
        force: bool,
    },

    /// Show this device's Short ID and fingerprint
    Whoami,

    /// List known peers and their fingerprints
    Peers,

    /// Change the local nickname of a stored peer identity
    ///
    /// Only the local nickname changes. The peer's key is untouched, and the
    /// peer is not notified: names are local labels, not identities.
    Rename {
        /// The peer's current nickname
        old_name: String,
        /// The nickname to use from now on
        new_name: String,
    },

    /// Forget a known peer
    ///
    /// The peer can no longer send you files,  with it again
    /// requires a fresh pairing code.
    Remove {
        /// The peer to forget
        name: String,
        /// Do not ask for confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Run the Beam daemon and accept incoming transfers.
    Listen {
        /// Directory to save received files in (default: current directory).
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// TCP port for the daemon (default: 9999).
        #[arg(long, default_value_t = crate::config::DEFAULT_PORT)]
        port: u16,
        /// Bind only to localhost. Useful for local testing.
        #[arg(long, hide = true)]
        loopback: bool,
    },

    /// Send a file directly to another Beam daemon.
    Send {
        /// Recipient address, for example 192.168.1.50 or 192.168.1.50:9999.
        peer: String,
        /// The file to send.
        file: PathBuf,
        /// Chunk size in bytes; useful for development/testing.
        #[arg(long, hide = true, value_name = "BYTES")]
        chunk_size: Option<u32>,
    },

    /// List or clear partially received transfers
    ///
    /// A transfer that was interrupted, declined or left unanswered keeps what
    /// it already received, so that sending the same file again continues
    /// rather than starting over. Partials older than seven days are removed
    /// when `beam listen` starts.
    Transfers {
        /// Delete partials instead of listing them
        #[arg(long)]
        clear: bool,
        /// Which partial to delete; all of them if left out
        id: Option<String>,
        /// Do not ask for confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Show the beam version
    Version,
}

/// The state shared by every command.
pub(crate) struct App {
    pub(crate) store: Store,
    pub(crate) json: bool,
}

/// The command tree, for tests that need to inspect the CLI's own shape.
pub fn command() -> clap::Command {
    Cli::command()
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
            // Errors can quote what a peer or a server sent; the last line of
            // defence before the terminal (ADR-0034).
            let _ = writeln!(
                io.err,
                "beam: {}",
                crate::untrusted::lines(&err.to_string())
            );
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
            Command::Listen { out, port, loopback } => self.listen(out, port, loopback, io),
            Command::Send { peer, file, chunk_size } => self.send(&peer, &file, chunk_size, io),
            Command::Transfers { clear, id, yes } => self.transfers(clear, id.as_deref(), yes, io),
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
