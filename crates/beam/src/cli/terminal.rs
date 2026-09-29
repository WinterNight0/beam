//! The terminal's side of a transfer : the keyboard and the
//! progress line. The questions themselves go through `desk`.

use std::io::{IsTerminal, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::transfer::{Progress, Reporter};
use crate::ui;

/// Lines typed at the keyboard.
///
/// Lines are read by one long-lived thread and handed over a channel, so an
/// answer typed after a question has already expired cannot be picked up by
/// the next question: each question throws away anything typed before it
/// started. The Accept prompt uses this same input discipline so stale input cannot
/// answer a later question.
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

impl super::desk::Lines for Keyboard {
    fn discard_pending(&mut self) {
        if let Ok(lines) = self.lines.lock() {
            while lines.try_recv().is_ok() {}
        }
    }

    fn next_line(&mut self, timeout: Duration) -> Option<String> {
        let lines = self.lines.lock().ok()?;
        lines.recv_timeout(timeout).ok()
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
    /// Under `listen`, whole lines go through the desk, so they cannot land
    /// in the middle of an open question (M6 item 5).
    desk: Option<super::desk::PromptDesk>,
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
            desk: None,
        }
    }

    /// A reporter whose whole lines go through `desk`.
    pub fn with_desk(desk: super::desk::PromptDesk) -> Self {
        Self {
            desk: Some(desk),
            ..Self::new()
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
        // A question is on screen; redrawing would write over it. The next
        // update after it is answered catches the line up.
        if super::desk::PROMPT_OPEN.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
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
        if let Some(desk) = &self.desk {
            desk.notice(text);
            return;
        }
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
            Progress::PeerVerifying { done, total } => {
                let text = format!(
                    "The peer is verifying the file: {} of {} ({}%)",
                    ui::format_bytes(done),
                    ui::format_bytes(total),
                    ui::percent(done, total)
                );
                self.draw(&text, done >= total);
            }
            Progress::PathChanged { from, to } => {
                self.finish();
                self.line(&format!("Path changed: {} -> {}", from.label(), to.label()));
            }
        }
    }
}
