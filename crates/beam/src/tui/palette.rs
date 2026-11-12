//! The command palette (`:` or Ctrl+P): every beam command, typed or picked
//! from a list, without leaving the full-screen view (ADR-0043).
//!
//! What is typed is split like a shell line and checked by the CLI's own
//! parser ([`crate::cli::check`]), so the palette accepts exactly what the
//! command line does. Where a command then runs is decided by [`place`]:
//!
//! * **Here** — it only prints something (`peers`, `whoami`, `service
//!   status`, …). It runs inside the view and its output opens in a pop-up.
//! * **Terminal** — it asks questions or keeps running (`send`, `pair`,
//!   `listen`, `service port-mapping on`, …). The view steps aside and the
//!   command runs as its own `beam` process in the normal terminal, with its
//!   normal prompts, warnings and Ctrl+C; Enter comes back.
//!
//! `rename` and `remove` have pop-ups of their own and `quit` leaves. Nothing
//! typed here can answer an incoming transfer: there is no such command.

use std::path::{MAIN_SEPARATOR, Path};

use super::input::Input;

/// Where a command runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Inside the view; output in a pop-up.
    Here,
    /// In the normal terminal, as its own process.
    Terminal,
    /// A pop-up of the view's own (`rename`, `remove`), or `quit`.
    View,
}

impl Place {
    pub fn badge(self) -> &'static str {
        match self {
            Place::Here => "here",
            Place::Terminal => "terminal ↗",
            Place::View => "pop-up",
        }
    }
}

/// The palette while it is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    pub input: Input,
    /// Index into the current suggestions.
    pub selected: usize,
    /// Why the last Enter did not run anything.
    pub error: Option<String>,
    /// Whether the person moved through the list since typing: then Enter
    /// takes the highlighted line rather than what is typed.
    pub moved: bool,
}

/// The longest line the palette takes.
pub const MAX_LINE: usize = 4096;

impl Palette {
    pub fn new() -> Self {
        Self {
            input: Input::new(MAX_LINE),
            ..Self::default()
        }
    }

    /// After the text changed: the list starts again from the top.
    pub fn typed(&mut self) {
        self.selected = 0;
        self.moved = false;
        self.error = None;
    }
}

/// One line of the list under the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// What is shown, with `<placeholders>`.
    pub label: String,
    pub about: String,
    /// What Tab puts in the box: the label up to its first placeholder.
    pub fill: String,
    /// Whether `fill` is a whole command that can run as it is.
    pub complete: bool,
    pub place: Option<Place>,
}

/// Every command, as the palette lists it. `<friend>` is filled in with each
/// friend's name.
const COMMANDS: &[(&str, &str)] = &[
    ("send <friend> <file>", "Send a file to a friend"),
    ("pair <invite> --name <name>", "Pair using someone's invite"),
    (
        "pair --wait --name <name>",
        "Show your invite and wait for them",
    ),
    ("inbox", "Answer what the background agent holds"),
    ("listen", "Wait for transfers and pairing here"),
    ("peers", "List your friends"),
    ("whoami", "Your Short ID, fingerprint and invite"),
    (
        "rename <friend> <new-name>",
        "Change your label for a friend",
    ),
    ("remove <friend>", "Forget a friend"),
    ("history", "What came and went"),
    ("history --clear", "Delete the transfer history"),
    ("transfers", "List partly received files"),
    ("transfers --clear", "Delete partly received files"),
    ("service status", "Is the background agent running?"),
    ("service start", "Start receiving in the background"),
    ("service stop", "Stop the background agent"),
    (
        "service enable",
        "Receive in the background from every login",
    ),
    ("service disable", "Stop starting the agent at login"),
    (
        "service port-mapping on",
        "Let the agent ask the router to forward its port",
    ),
    ("service port-mapping off", "Stop asking the router"),
    ("receive-dir", "Where received files go"),
    ("receive-dir <folder>", "Change where received files go"),
    ("receive-dir --default", "Go back to the default folder"),
    ("ui cli", "Make plain `beam` print the help"),
    ("ui tui", "Make plain `beam` open this view"),
    ("init", "Create this device's identity"),
    ("version", "Which beam this is"),
    ("help", "Every command and option"),
    ("quit", "Leave beam"),
];

/// Splits a line like a shell does: spaces separate words, and "double" or
/// 'single' quotes keep spaces inside one. Backslashes are ordinary
/// characters, so Windows paths need no escaping.
pub fn split(line: &str) -> Result<Vec<String>, String> {
    let (words, open) = split_lenient(line);
    match open {
        Some(quote) => Err(format!("a {quote} quote is not closed")),
        None => Ok(words.into_iter().map(|w| w.text).collect()),
    }
}

/// A word and where it starts in the line, in bytes.
#[derive(Debug)]
struct Word {
    text: String,
    start: usize,
}

/// Like [`split`], but an unclosed quote is allowed (the person is still
/// typing) and reported instead.
fn split_lenient(line: &str) -> (Vec<Word>, Option<char>) {
    let mut words = Vec::new();
    let mut current: Option<Word> = None;
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => {
                quote = Some(c);
                current.get_or_insert(Word {
                    text: String::new(),
                    start: i,
                });
            }
            (None, c) if c.is_whitespace() => {
                if let Some(word) = current.take() {
                    words.push(word);
                }
            }
            (_, c) => current
                .get_or_insert(Word {
                    text: String::new(),
                    start: i,
                })
                .text
                .push(c),
        }
    }
    words.extend(current);
    (words, quote)
}

/// The words of a typed command, without a leading `beam` that a person used
/// to the command line may type out of habit.
pub fn words(line: &str) -> Result<Vec<String>, String> {
    let mut words = split(line)?;
    if words
        .first()
        .is_some_and(|w| w.eq_ignore_ascii_case("beam"))
    {
        words.remove(0);
    }
    Ok(words)
}

/// The subcommand and what follows it, skipping the global flags.
pub fn command_words(words: &[String]) -> &[String] {
    let mut i = 0;
    while i < words.len() {
        match words[i].as_str() {
            "--json" => i += 1,
            "--beam-dir" => i += 2,
            w if w.starts_with("--beam-dir=") => i += 1,
            _ => break,
        }
    }
    &words[i.min(words.len())..]
}

/// Where a command runs; see the module notes.
pub fn place(words: &[String]) -> Place {
    if words
        .iter()
        .any(|w| matches!(w.as_str(), "--help" | "-h" | "--version" | "-V"))
    {
        return Place::Here;
    }
    let rest = command_words(words);
    let arg = |i: usize| rest.get(i).map(String::as_str);
    match arg(0) {
        None => Place::Here,
        Some("quit" | "exit" | "rename" | "remove") => Place::View,
        // In the view, with its progress; the hidden developer flags
        // (`--loopback`, `--chunk-size`, `--addr`) step out.
        Some("send") if !rest.iter().skip(1).any(|w| w.starts_with('-')) => Place::View,
        Some("whoami" | "peers" | "version" | "help" | "ui" | "receive-dir") => Place::Here,
        Some("transfers" | "history") if !rest.iter().any(|w| w == "--clear") => Place::Here,
        Some("service") => match (arg(1), arg(2)) {
            (Some("port-mapping"), Some("on")) => Place::Terminal,
            // status, start, stop, enable, disable, port-mapping off, and
            // `service` alone (its help) ask nothing.
            _ => Place::Here,
        },
        _ => Place::Terminal,
    }
}

/// Lists directory entries that start with `partial`, for Tab completion of
/// a file (or, with `dirs_only`, a folder). Folders end in a separator.
pub fn list_paths(partial: &str, dirs_only: bool) -> Vec<String> {
    let cut = partial.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let (dir, prefix) = partial.split_at(cut);
    let read = if dir.is_empty() {
        std::fs::read_dir(".")
    } else {
        std::fs::read_dir(Path::new(dir))
    };
    let Ok(entries) = read else {
        return Vec::new();
    };
    let prefix_lower = prefix.to_lowercase();
    let mut found: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let is_dir = entry.file_type().ok()?.is_dir();
            let matches = if cfg!(windows) {
                name.to_lowercase().starts_with(&prefix_lower)
            } else {
                name.starts_with(prefix)
            };
            // Hidden files only when asked for by a leading dot.
            let hidden = name.starts_with('.') && !prefix.starts_with('.');
            (matches && !hidden && (is_dir || !dirs_only)).then(|| {
                let sep = if is_dir {
                    MAIN_SEPARATOR.to_string()
                } else {
                    String::new()
                };
                format!("{dir}{name}{sep}")
            })
        })
        .collect();
    found.sort_by_key(|p| p.to_lowercase());
    found.truncate(200);
    found
}

/// A word as it must be typed: quoted if it has a space or a quote in it.
fn quoted(word: &str) -> String {
    if word.is_empty() || word.chars().any(|c| c.is_whitespace() || c == '\'') {
        format!("\"{word}\"")
    } else if word.contains('"') {
        format!("'{word}'")
    } else {
        word.to_string()
    }
}

/// What to list under the palette for `line`.
///
/// `paths` lists file-system entries (see [`list_paths`]); tests pass their
/// own. `history` is newest last.
pub fn suggest(
    line: &str,
    friends: &[String],
    history: &[String],
    paths: &dyn Fn(&str, bool) -> Vec<String>,
) -> Vec<Suggestion> {
    let (mut words, _) = split_lenient(line);
    let mut offset = 0;
    if words
        .first()
        .is_some_and(|w| w.text.eq_ignore_ascii_case("beam") && words.len() > 1)
    {
        offset = words[1].start;
        words.remove(0);
    }
    let typing_new_word = line.ends_with(char::is_whitespace) || words.is_empty();
    // The word being typed, and the words before it.
    let (done, current) = if typing_new_word {
        (&words[..], None)
    } else {
        (&words[..words.len() - 1], words.last())
    };
    let before = &line[..current.map_or(line.len(), |w| w.start)];
    let partial = current.map_or("", |w| w.text.as_str());
    let command = done.first().map(|w| w.text.as_str());

    match (command, done.len()) {
        // The friend for send / rename / remove.
        (Some(cmd @ ("send" | "rename" | "remove")), 1) => {
            return friends
                .iter()
                .filter(|f| f.to_lowercase().starts_with(&partial.to_lowercase()))
                .map(|f| {
                    let complete = cmd == "remove";
                    let fill = if complete {
                        format!("{before}{f}")
                    } else {
                        format!("{before}{f} ")
                    };
                    Suggestion {
                        label: f.clone(),
                        about: "friend".to_string(),
                        fill,
                        complete,
                        place: Some(place(&[cmd.to_string()])),
                    }
                })
                .collect();
        }
        // The file for send, the folder for receive-dir.
        (Some(cmd @ ("send" | "receive-dir")), n)
            if (cmd == "send" && n == 2)
                || (cmd == "receive-dir" && n == 1 && !partial.starts_with('-')) =>
        {
            let dirs_only = cmd == "receive-dir";
            return paths(partial, dirs_only)
                .into_iter()
                .map(|p| {
                    let is_dir = p.ends_with(['/', '\\']);
                    Suggestion {
                        fill: format!("{before}{}", quoted(&p)),
                        label: p,
                        about: if is_dir { "folder" } else { "file" }.to_string(),
                        // A folder for `send` needs another Tab, into it.
                        complete: dirs_only || !is_dir,
                        place: Some(if dirs_only {
                            Place::Here
                        } else {
                            Place::Terminal
                        }),
                    }
                })
                .collect();
        }
        _ => {}
    }

    let query = line[offset..].trim().to_lowercase();
    let mut out: Vec<(u8, Suggestion)> = Vec::new();
    if query.is_empty() {
        for (i, past) in history.iter().rev().enumerate().take(5) {
            let place = words_place(past);
            out.push((
                0,
                Suggestion {
                    label: past.clone(),
                    about: if i == 0 { "last used" } else { "recent" }.to_string(),
                    fill: past.clone(),
                    complete: true,
                    place,
                },
            ));
        }
    }
    for (template, about) in COMMANDS {
        let labels: Vec<String> = if template.contains("<friend>") {
            if friends.is_empty() {
                vec![template.to_string()]
            } else {
                friends
                    .iter()
                    .map(|f| template.replace("<friend>", f))
                    .collect()
            }
        } else {
            vec![template.to_string()]
        };
        for label in labels {
            let Some(score) = score(&query, &label) else {
                continue;
            };
            let (fill, complete) = match label.find('<') {
                Some(i) => (label[..i].to_string(), false),
                None => (label.clone(), true),
            };
            let place = words_place(&label);
            out.push((
                score + 1,
                Suggestion {
                    label,
                    about: about.to_string(),
                    fill,
                    complete,
                    place,
                },
            ));
        }
    }
    // Stable: equal scores keep the order above.
    out.sort_by_key(|(score, _)| *score);
    out.into_iter().map(|(_, s)| s).collect()
}

fn words_place(line: &str) -> Option<Place> {
    words(line).ok().map(|w| place(&w))
}

/// How well `label` matches what was typed: lower is better, `None` is no
/// match. A prefix beats words matched in order, which beats letters
/// matched in order.
fn score(query: &str, label: &str) -> Option<u8> {
    let label = label.to_lowercase();
    if query.is_empty() || label.starts_with(query) {
        return Some(0);
    }
    let mut label_words = label.split_whitespace();
    if query
        .split_whitespace()
        .all(|q| label_words.any(|l| l.starts_with(q)))
    {
        return Some(1);
    }
    let mut chars = label.chars();
    query
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|q| chars.any(|l| l == q))
        .then_some(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(line: &str) -> Vec<String> {
        words(line).unwrap()
    }

    fn no_paths(_: &str, _: bool) -> Vec<String> {
        Vec::new()
    }

    fn friends() -> Vec<String> {
        vec!["alice".to_string(), "bob".to_string()]
    }

    #[test]
    fn a_line_splits_like_a_shell_and_keeps_windows_paths() {
        assert_eq!(
            w(r#"send alice "C:\My Files\a b.txt""#),
            ["send", "alice", r"C:\My Files\a b.txt"]
        );
        assert_eq!(w("send  alice 'it\"s.txt'"), ["send", "alice", "it\"s.txt"]);
        assert_eq!(w("beam peers"), ["peers"], "a leading `beam` is dropped");
        assert!(split("send alice \"open").is_err());
    }

    #[test]
    fn commands_run_where_they_belong() {
        for line in [
            "peers",
            "whoami",
            "--json peers",
            "service status",
            "service start",
            "service port-mapping off",
            "transfers",
            "receive-dir D:\\x",
            "send --help",
            "ui cli",
        ] {
            assert_eq!(place(&w(line)), Place::Here, "{line}");
        }
        for line in [
            "send alice x --loopback",
            "pair --wait --name a",
            "listen",
            "inbox",
            "init --force",
            "transfers --clear",
            "service port-mapping on",
            "agent",
        ] {
            assert_eq!(place(&w(line)), Place::Terminal, "{line}");
        }
        for line in ["rename a b", "remove a", "quit", "send alice x"] {
            assert_eq!(place(&w(line)), Place::View, "{line}");
        }
    }

    #[test]
    fn letters_find_commands_best_match_first() {
        let list = suggest("pe", &friends(), &[], &no_paths);
        assert_eq!(list[0].label, "peers");
        assert!(list[0].complete);

        let list = suggest("serv st", &friends(), &[], &no_paths);
        let labels: Vec<&str> = list.iter().map(|s| s.label.as_str()).collect();
        assert!(
            labels.starts_with(&["service status", "service start", "service stop"]),
            "{labels:?}"
        );

        assert!(suggest("zzzz", &friends(), &[], &no_paths).is_empty());
    }

    #[test]
    fn friend_templates_are_listed_once_per_friend() {
        let list = suggest("send", &friends(), &[], &no_paths);
        assert_eq!(list[0].label, "send alice <file>");
        assert_eq!(list[0].fill, "send alice ");
        assert!(!list[0].complete, "a file is still needed");
        assert_eq!(list[1].label, "send bob <file>");
    }

    #[test]
    fn the_second_word_completes_a_friend() {
        let list = suggest("send b", &friends(), &[], &no_paths);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].fill, "send bob ");

        let list = suggest("remove a", &friends(), &[], &no_paths);
        assert_eq!(list[0].fill, "remove alice");
        assert!(list[0].complete);
    }

    #[test]
    fn the_file_for_send_completes_from_the_disk_and_is_quoted() {
        let paths = |partial: &str, dirs_only: bool| {
            assert_eq!(partial, "My");
            assert!(!dirs_only);
            vec!["My Files\\".to_string(), "My.txt".to_string()]
        };
        let list = suggest("send alice My", &friends(), &[], &paths);
        assert_eq!(list[0].fill, "send alice \"My Files\\\"");
        assert!(!list[0].complete, "a folder needs another Tab");
        assert_eq!(list[1].fill, "send alice My.txt");
        assert!(list[1].complete);
    }

    #[test]
    fn an_empty_palette_offers_recent_commands_first() {
        let history = vec!["peers".to_string(), "send alice a.txt".to_string()];
        let list = suggest("", &friends(), &history, &no_paths);
        assert_eq!(list[0].label, "send alice a.txt");
        assert_eq!(list[0].about, "last used");
        assert_eq!(list[1].label, "peers");
        assert!(list.len() > 2, "then every command");
    }

    #[test]
    fn listing_real_paths_finds_files_and_marks_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.pdf"), b"x").unwrap();
        std::fs::write(dir.path().join(".hidden"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("reports")).unwrap();
        let base = format!("{}{MAIN_SEPARATOR}", dir.path().display());
        let found = list_paths(&format!("{base}rep"), false);
        assert_eq!(
            found,
            [
                format!("{base}report.pdf"),
                format!("{base}reports{MAIN_SEPARATOR}")
            ]
        );
        assert_eq!(list_paths(&format!("{base}rep"), true).len(), 1);
        assert!(
            list_paths(&base, false)
                .iter()
                .all(|p| !p.contains(".hidden"))
        );
    }
}
