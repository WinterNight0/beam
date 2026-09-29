//! The prompt desk: one question on screen at a time.
//!
//! `beam listen` can be asked two things at once: a known peer wants to send
//! a file 
//!
//! The rules (S-24, ADR-0030):
//!
//! * **One question at a time**, first come first served.
//! * **A question's clock starts when it is asked, not when it is shown.** One
//!   that has waited out its deadline in the queue is answered *no* without
//!   ever being shown, and the screen says so.
//! * **Nothing typed before a question appears can answer it.**
//! * **Transfer questions use `[y/N]`.**

use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use crate::transfer::PromptRequest;
use crate::{ui, untrusted};

/// Set while a question is on screen, so a progress line does not redraw over
/// it. Read by `terminal::TerminalReporter`.
pub(crate) static PROMPT_OPEN: AtomicBool = AtomicBool::new(false);

/// How much longer than a question's deadline its asker waits for the desk,
/// in case the desk is slow to notice the deadline. Beyond it, the asker
/// treats the question as refused.
const REPLY_GRACE: Duration = Duration::from_secs(5);

/// A question for the person at the keyboard.
#[derive(Clone, Debug)]
pub enum Question {
    /// Accept an incoming file? `[y/N]`.
    Transfer(PromptRequest),
}

impl Question {
    /// What the screen says when this question expired in the queue.
    fn expired_note(&self) -> String {
        match self {
            Self::Transfer(r) => format!(
                "A file from {} ({}) was refused: it waited too long behind another question.",
                untrusted::name(&r.peer_name),
                untrusted::name(&r.file_name)
            ),
        }
    }

    /// Whether `answer` says yes to this question.
    fn is_yes(&self, answer: &str) -> bool {
        let answer = answer.trim().to_ascii_lowercase();
        match self {
            Self::Transfer(_) => answer == "y" || answer == "yes",
        }
    }

    /// Draws the question. `left` is shown when the question has waited in
    /// the queue and no longer has its full time.
    fn render(&self, out: &mut dyn Write, left: Option<Duration>) -> std::io::Result<()> {
        let left = left
            .map(|d| format!(" ({} s left)", d.as_secs().max(1)))
            .unwrap_or_default();
        match self {
            Self::Transfer(request) => {
                writeln!(out)?;
                match &request.resume {
                    Some(_) => writeln!(out, "Incoming file (resuming)")?,
                    None => writeln!(out, "Incoming file")?,
                }
                ui::field(out, "From", &untrusted::name(&request.peer_name))?;
                ui::field(out, "Fingerprint", &request.fingerprint)?;
                // Cut in the middle if long, so the extension stays in view.
                ui::field(out, "File", &untrusted::name(&request.file_name))?;
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
                if left.is_empty() {
                    write!(out, "Accept? [y/N]: ")
                } else {
                    write!(out, "Accept? [y/N]{left}: ")
                }
            }

        }
    }
}

/// Somewhere typed lines come from. The terminal's is
/// [`super::terminal::Keyboard`]; the tests script one.
pub trait Lines: Send + 'static {
    /// Throws away anything typed so far.
    fn discard_pending(&mut self);
    /// The next line, waiting at most `timeout`. `None` when time runs out or
    /// input has ended.
    fn next_line(&mut self, timeout: Duration) -> Option<String>;
}

struct Job {
    question: Question,
    deadline: Instant,
    full: Duration,
    reply: Sender<bool>,
}

/// What the desk thread is handed: a question to ask, or a line to show.
enum Msg {
    Ask(Job),
    Notice(String),
}

/// How often an open question checks for notices to show.
const POLL: Duration = Duration::from_millis(50);

/// Hands questions and notices to the desk thread. Cheap to clone; every
/// clone feeds the same queue.
#[derive(Clone)]
pub struct PromptDesk {
    queue: Sender<Msg>,
}

impl PromptDesk {
    /// Starts a desk reading from `lines` and drawing on `out`.
    pub fn start(lines: impl Lines, out: impl Write + Send + 'static) -> Self {
        let (queue, incoming) = channel();
        std::thread::spawn(move || serve(incoming, lines, out));
        Self { queue }
    }

    /// A desk on the real keyboard and stdout.
    pub fn terminal(keyboard: super::terminal::Keyboard) -> Self {
        Self::start(keyboard, std::io::stdout())
    }

    /// Asks `question`, blocking until it is answered, refused, or out of
    /// time. `timeout` counts from now, however long the queue is.
    pub fn ask(&self, question: Question, timeout: Duration) -> bool {
        let (reply, answer) = channel();
        let job = Job {
            question,
            deadline: Instant::now() + timeout,
            full: timeout,
            reply,
        };
        if self.queue.send(Msg::Ask(job)).is_err() {
            return false;
        }
        answer.recv_timeout(timeout + REPLY_GRACE).unwrap_or(false)
    }

    /// Shows one or more lines of information. With no question open they are
    /// printed at once. With a question open they are printed below it and
    /// the question is drawn again, with the time it has left, so it is never
    /// left scrolled away half-hidden (M6 item 5).
    pub fn notice(&self, text: impl Into<String>) {
        let _ = self.queue.send(Msg::Notice(text.into()));
    }
}

/// The desk thread: one question at a time, notices in between or on top,
/// until every desk handle is gone.
fn serve(incoming: Receiver<Msg>, mut lines: impl Lines, mut out: impl Write) {
    let mut waiting: VecDeque<Job> = VecDeque::new();
    loop {
        let job = match waiting.pop_front() {
            Some(job) => job,
            None => match incoming.recv() {
                Ok(Msg::Ask(job)) => job,
                Ok(Msg::Notice(text)) => {
                    let _ = writeln!(out, "{text}");
                    let _ = out.flush();
                    continue;
                }
                Err(_) => return,
            },
        };
        ask_one(job, &incoming, &mut waiting, &mut lines, &mut out);
    }
}

/// Puts one question on screen and waits for its answer, showing notices as
/// they come and queueing any question that arrives meanwhile.
fn ask_one(
    job: Job,
    incoming: &Receiver<Msg>,
    waiting: &mut VecDeque<Job>,
    lines: &mut impl Lines,
    out: &mut impl Write,
) {
    let now = Instant::now();
    if now >= job.deadline {
        let _ = writeln!(out, "\n{}", job.question.expired_note());
        let _ = out.flush();
        let _ = job.reply.send(false);
        return;
    }
    // Only mention the time if the question lost some of it in the queue.
    let remaining = job.deadline - now;
    let left = (job.full.saturating_sub(remaining) > Duration::from_secs(1)).then_some(remaining);

    lines.discard_pending();
    PROMPT_OPEN.store(true, Ordering::SeqCst);
    let _ = job.question.render(out, left);
    let _ = out.flush();

    let answer = loop {
        let now = Instant::now();
        if now >= job.deadline {
            break None;
        }
        let slice = (job.deadline - now).min(POLL);
        if let Some(line) = lines.next_line(slice) {
            break Some(line);
        }
        // Between slices: anything to show, or to queue?
        let mut redraw = false;
        while let Ok(msg) = incoming.try_recv() {
            match msg {
                Msg::Notice(text) => {
                    let _ = writeln!(out, "\n{text}");
                    redraw = true;
                }
                Msg::Ask(other) => waiting.push_back(other),
            }
        }
        if redraw {
            let left = job.deadline.saturating_duration_since(Instant::now());
            let _ = job.question.render(out, Some(left));
            let _ = out.flush();
        }
    };
    if answer.is_none() {
        let _ = writeln!(out);
    }
    PROMPT_OPEN.store(false, Ordering::SeqCst);
    let _ = out.flush();

    let yes = answer.is_some_and(|a| job.question.is_yes(&a));
    let _ = job.reply.send(yes);
}

/// Answers transfer prompts through the desk.
#[derive(Clone)]
pub struct DeskPrompt {
    desk: PromptDesk,
    timeout: Duration,
}

impl DeskPrompt {
    pub fn new(desk: PromptDesk, timeout: Duration) -> Self {
        Self { desk, timeout }
    }
}

impl crate::transfer::Prompt for DeskPrompt {
    fn confirm(&mut self, request: &PromptRequest) -> std::io::Result<bool> {
        Ok(self
            .desk
            .ask(Question::Transfer(request.clone()), self.timeout))
    }
}

/// A channel-backed stand-in for the keyboard, and a shared screen, for tests
/// here and in `tests/`.
#[doc(hidden)]
pub mod testing {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Lines the test types. One message may hold several lines separated
    /// by a newline, which models a paste: they arrive together, and the lines
    /// after the first are pending input from then on.
    pub struct ScriptedLines {
        typed: Receiver<String>,
        pending: std::collections::VecDeque<String>,
    }

    impl Lines for ScriptedLines {
        fn discard_pending(&mut self) {
            self.pending.clear();
            while self.typed.try_recv().is_ok() {}
        }
        fn next_line(&mut self, timeout: Duration) -> Option<String> {
            if let Some(line) = self.pending.pop_front() {
                return Some(line);
            }
            match self.typed.recv_timeout(timeout) {
                Ok(text) => {
                    let mut lines = text.split('\n').map(str::to_string);
                    let first = lines.next();
                    self.pending.extend(lines);
                    first
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
            }
        }
    }

    /// A screen the test can read back.
    #[derive(Clone, Default)]
    pub struct Screen(pub Arc<Mutex<Vec<u8>>>);

    impl Screen {
        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }

        /// Waits until the screen shows `needle`, or panics after `patience`.
        pub fn wait_for(&self, needle: &str, patience: Duration) {
            let end = Instant::now() + patience;
            while !self.text().contains(needle) {
                assert!(
                    Instant::now() < end,
                    "never saw {needle:?}; the screen shows:\n{}",
                    self.text()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Write for Screen {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A desk with a scripted keyboard: returns the desk, the keyboard to type
    /// into, and the screen.
    pub fn desk() -> (PromptDesk, Sender<String>, Screen) {
        let (typed, lines) = channel();
        let screen = Screen::default();
        let lines = ScriptedLines {
            typed: lines,
            pending: Default::default(),
        };
        let desk = PromptDesk::start(lines, screen.clone());
        (desk, typed, screen)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::desk;
    use super::*;

    fn transfer(peer: &str, file: &str) -> Question {
        Question::Transfer(PromptRequest {
            peer_name: peer.into(),
            fingerprint: "unverified key".into(),
            file_name: file.into(),
            size: 1234,
            resume: None,
        })
    }

    const TIMEOUT: Duration = Duration::from_secs(5);

    fn ask_later(desk: &PromptDesk, question: Question) -> std::thread::JoinHandle<bool> {
        let desk = desk.clone();
        std::thread::spawn(move || desk.ask(question, TIMEOUT))
    }

    #[test]
    fn a_transfer_is_accepted_with_y() {
        let (desk, typed, screen) = desk();
        let answer = ask_later(&desk, transfer("direct peer", "a.txt"));
        screen.wait_for("Accept? [y/N]: ", TIMEOUT);
        typed.send("y".into()).unwrap();
        assert!(answer.join().unwrap());
    }

    #[test]
    fn a_transfer_question_can_be_declined() {
        let (desk, typed, screen) = desk();
        let answer = ask_later(&desk, transfer("direct peer", "a.txt"));
        screen.wait_for("Accept? [y/N]: ", TIMEOUT);
        typed.send("n".into()).unwrap();
        assert!(!answer.join().unwrap());
    }
}
