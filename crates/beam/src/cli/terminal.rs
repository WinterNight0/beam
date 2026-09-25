//! The terminal's side of a transfer and of pairing: the prompts and the
//! progress line.

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

/// Lines typed at the keyboard.
///
/// Lines are read by one long-lived thread and handed over a channel, so an
/// answer typed after a question has already expired cannot be picked up by
/// the next question: each question throws away anything typed before it
/// started. The Accept prompt and the pairing prompts share this, so they
/// share that rule.
#[derive(Clone)]
pub struct Keyboard {
    lines: Arc<Mutex<Receiver<String>>>,
}

impl Keyboard {
    /// Starts the stdin reader thread.
    pub fn start() -> Self {
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
        }
    }

    /// Asks a question: throws away anything typed earlier, runs `show` to
    /// print the question, then waits up to `deadline` for one line.
    ///
    /// `None` means no answer — out of time, or stdin closed.
    pub fn ask(
        &self,
        deadline: Duration,
        show: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
    ) -> std::io::Result<Option<String>> {
        let lines = self
            .lines
            .lock()
            .map_err(|_| std::io::Error::other("the stdin reader panicked"))?;

        // Anything typed before this question appeared was meant for
        // something else — very likely a question that has already expired.
        loop {
            match lines.try_recv() {
                Ok(_) => continue,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(None),
            }
        }

        let mut out = std::io::stdout().lock();
        show(&mut out)?;
        out.flush()?;

        match lines.recv_timeout(deadline) {
            Ok(line) => Ok(Some(line)),
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                writeln!(out)?;
                Ok(None)
            }
        }
    }
}

/// Whether a typed answer is a yes. Anything else, including nothing, is no.
pub fn is_yes(answer: Option<&str>) -> bool {
    matches!(
        answer.map(|a| a.trim().to_ascii_lowercase()).as_deref(),
        Some("y" | "yes")
    )
}

/// Asks the person at the keyboard whether to accept a transfer.
#[derive(Clone)]
pub struct TerminalPrompt {
    keyboard: Keyboard,
    deadline: Duration,
}

impl TerminalPrompt {
    /// Starts the stdin reader. `accept_timeout` is the engine's deadline; the
    /// prompt waits a little longer than that and then gives up.
    pub fn new(accept_timeout: Duration) -> Self {
        Self {
            keyboard: Keyboard::start(),
            deadline: accept_timeout + PROMPT_GRACE,
        }
    }
}

impl Prompt for TerminalPrompt {
    fn confirm(&mut self, request: &PromptRequest) -> std::io::Result<bool> {
        let answer = self.keyboard.ask(self.deadline, |out| {
            writeln!(out)?;
            match &request.resume {
                Some(_) => writeln!(out, "Incoming file (resuming)")?,
                None => writeln!(out, "Incoming file")?,
            }
            ui::field(out, "From", &request.peer_name)?;
            ui::field(out, "Fingerprint", &request.fingerprint)?;
            ui::field(out, "File", &request.file_name)?;
            ui::field(out, "Size", &ui::format_bytes(request.size))?;
            if let Some(resume) = &request.resume {
                let mut already = format!(
                    "{} ({}%)",
                    ui::format_bytes(resume.have_bytes),
                    ui::percent(resume.have_bytes, request.size)
                );
                if let Some(age) = resume.age {
                    already.push_str(&format!(", from {}", ui::format_age(age)));
                }
                ui::field(out, "Already have", &already)?;
            }
            write!(out, "Accept? [y/N]: ")
        })?;
        Ok(is_yes(answer.as_deref()))
    }
}

/// Asks whether to save a device that has just proved it knows the pairing
/// code. The same rules as Accept: no answer in time is a no, and there is no
/// way to answer in advance.
#[derive(Clone)]
pub struct TerminalPairConfirm {
    keyboard: Keyboard,
    deadline: Duration,
}

impl TerminalPairConfirm {
    pub fn new(keyboard: Keyboard, decision_timeout: Duration) -> Self {
        Self {
            keyboard,
            deadline: decision_timeout + PROMPT_GRACE,
        }
    }
}

impl crate::pairing::Confirm for TerminalPairConfirm {
    fn confirm(&mut self, request: &crate::pairing::ConfirmRequest) -> std::io::Result<bool> {
        let answer = self.keyboard.ask(self.deadline, |out| {
            writeln!(out)?;
            writeln!(out, "The other device knows the code.")?;
            ui::field(out, "Save as", &request.name)?;
            ui::field(out, "Their key", &request.peer_fingerprint.to_string())?;
            ui::field(out, "Your key", &request.own_fingerprint.to_string())?;
            writeln!(out)?;
            writeln!(
                out,
                "Check that the other screen shows the same two fingerprints, the other way round."
            )?;
            write!(out, "Pair with this device? [y/N]: ")
        })?;
        Ok(is_yes(answer.as_deref()))
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
            Progress::Rechecking => {
                self.finish();
                self.line("Checking what is already here...");
            }
        }
    }
}
