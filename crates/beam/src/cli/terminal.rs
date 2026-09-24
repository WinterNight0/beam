//! The terminal's side of a transfer: the Accept prompt and the progress line.

use std::io::{IsTerminal, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::transfer::{Progress, Prompt, PromptRequest, Reporter};
use crate::ui;

/// How much longer than the transfer's own deadline the prompt waits before
/// giving up on the keyboard.
///
/// The engine stops waiting at the deadline, but the thread sitting on stdin
/// has to end by itself: tokio waits for blocking tasks before it shuts down.
const PROMPT_GRACE: Duration = Duration::from_secs(5);

/// Asks the person at the keyboard whether to accept a transfer.
///
/// Lines are read by one long-lived thread and handed over a channel, so an
/// answer typed after a request has already expired cannot be picked up by the
/// next prompt: each prompt throws away anything typed before it started.
#[derive(Clone)]
pub struct TerminalPrompt {
    lines: Arc<Mutex<Receiver<String>>>,
    deadline: Duration,
}

impl TerminalPrompt {
    /// Starts the stdin reader. `accept_timeout` is the engine's deadline; the
    /// prompt waits a little longer than that and then gives up.
    pub fn new(accept_timeout: Duration) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            lines: Arc::new(Mutex::new(rx)),
            deadline: accept_timeout + PROMPT_GRACE,
        }
    }
}

impl Prompt for TerminalPrompt {
    fn confirm(&mut self, request: &PromptRequest) -> std::io::Result<bool> {
        let lines = self
            .lines
            .lock()
            .map_err(|_| std::io::Error::other("the stdin reader panicked"))?;

        // Anything typed before this prompt appeared was meant for something
        // else — very likely a request that has already expired.
        loop {
            match lines.try_recv() {
                Ok(_) => continue,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(false),
            }
        }

        let mut out = std::io::stdout().lock();
        writeln!(out)?;
        writeln!(out, "Incoming file")?;
        ui::field(&mut out, "From", &request.peer_name)?;
        ui::field(&mut out, "Fingerprint", &request.fingerprint)?;
        ui::field(&mut out, "File", &request.file_name)?;
        ui::field(&mut out, "Size", &ui::format_bytes(request.size))?;
        write!(out, "Accept? [y/N]: ")?;
        out.flush()?;

        match lines.recv_timeout(self.deadline) {
            Ok(line) => {
                let answer = line.trim().to_ascii_lowercase();
                Ok(answer == "y" || answer == "yes")
            }
            // Out of time, or stdin closed. Both are a no.
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                writeln!(out)?;
                Ok(false)
            }
        }
    }
}

/// Draws the progress line.
///
/// On a terminal it rewrites one line in place; anywhere else — a pipe, a log,
/// CI — it prints an occasional plain line instead, because carriage returns in
/// a log file are worse than useless.
pub struct TerminalReporter {
    interactive: bool,
    last_drawn: Option<Instant>,
    line_open: bool,
}

/// How often the line is redrawn at most.
const REDRAW_EVERY: Duration = Duration::from_millis(100);

/// How often a non-interactive run prints a line.
const LOG_EVERY: Duration = Duration::from_secs(2);

impl TerminalReporter {
    /// A reporter that adapts to whether stdout is a terminal.
    pub fn new() -> Self {
        Self {
            interactive: std::io::stdout().is_terminal(),
            last_drawn: None,
            line_open: false,
        }
    }

    /// Ends the progress line, if one is open.
    pub fn finish(&mut self) {
        if self.line_open {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out);
            let _ = out.flush();
            self.line_open = false;
        }
    }

    fn due(&mut self, force: bool) -> bool {
        let interval = if self.interactive {
            REDRAW_EVERY
        } else {
            LOG_EVERY
        };
        let now = Instant::now();
        match self.last_drawn {
            Some(last) if !force && now.duration_since(last) < interval => false,
            _ => {
                self.last_drawn = Some(now);
                true
            }
        }
    }

    fn draw(&mut self, text: &str, force: bool) {
        if !self.due(force) {
            return;
        }
        let mut out = std::io::stdout().lock();
        if self.interactive {
            // \r plus trailing spaces, so a shorter line does not leave the
            // tail of a longer one behind.
            let _ = write!(out, "\r{text}    ");
            self.line_open = true;
        } else {
            let _ = writeln!(out, "{text}");
        }
        let _ = out.flush();
    }

    fn line(&mut self, text: &str) {
        self.finish();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}

impl Default for TerminalReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter for TerminalReporter {
    fn report(&mut self, progress: Progress) {
        match progress {
            Progress::Hashing { done, total } => {
                let text = format!(
                    "Hashing {} of {} ({}%)",
                    ui::format_bytes(done),
                    ui::format_bytes(total),
                    ui::percent(done, total)
                );
                self.draw(&text, false);
            }
            Progress::AwaitingAccept => {
                self.finish();
                self.line("Waiting for the peer to accept...");
            }
            Progress::Accepted { path } => {
                self.line(&format!("{} accepted", path.label()));
            }
            Progress::Transferring { done, total, path } => {
                let text = format!(
                    "{} {} of {} ({}%)",
                    path.label(),
                    ui::format_bytes(done),
                    ui::format_bytes(total),
                    ui::percent(done, total)
                );
                self.draw(&text, done >= total);
            }
            Progress::Verifying => {
                self.finish();
                self.line("Verifying...");
            }
        }
    }
}
