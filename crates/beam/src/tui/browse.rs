//! The file browser behind `s` (ADR-0045): every drive and the usual
//! places on the left, the current folder on the right, like an editor's
//! "open folder" dialog.
//!
//! Typing filters the folder. Enter opens a folder or sends a file;
//! Backspace (with nothing typed) goes up. A typed or dragged-in path — one
//! with a slash, or a drive like `D:` — goes straight there, so the old
//! "type the path" way still works.
//!
//! The disk is reached only through [`Fs`], a pair of plain functions, so
//! the whole browser can be tested against a made-up tree.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use super::input::Input;
use super::palette::MAX_LINE;

/// One thing in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix seconds.
    pub modified: Option<u64>,
    /// A dot file, or marked hidden or system on Windows.
    pub hidden: bool,
}

/// A shortcut on the left: a usual folder, or a drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    pub drive: bool,
}

/// How the browser reaches the disk.
#[derive(Clone, Copy, Debug)]
pub struct Fs {
    pub list: fn(&Path) -> std::io::Result<Vec<DirEntry>>,
    pub places: fn() -> Vec<Place>,
    /// Where it opens the first time.
    pub start: fn() -> PathBuf,
}

impl Fs {
    pub fn real() -> Self {
        Self {
            list: list_dir,
            places,
            start: || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

/// Which side has the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Places,
    Files,
}

/// A line on the right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// `..`, the folder above.
    Up,
    /// Index into `Browser::entries`.
    Entry(usize),
}

/// What Enter (or a click) asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chosen {
    Nothing,
    /// Send this file.
    File(PathBuf),
}

/// The browser while it is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Browser {
    /// Who the file is for.
    pub peer: String,
    pub dir: PathBuf,
    /// The folder's contents, folders first, in natural order.
    pub entries: Vec<DirEntry>,
    pub filter: Input,
    /// Index into [`Browser::rows`].
    pub selected: usize,
    pub places: Vec<Place>,
    pub place_selected: usize,
    pub pane: Pane,
    pub error: Option<String>,
}

/// The most entries one folder shows; a folder with more is cut, and says so.
pub const MAX_ENTRIES: usize = 5000;

impl Browser {
    /// Opens at `dir`, or wherever it can if that is gone.
    pub fn open(fs: &Fs, peer: String, dir: PathBuf) -> Self {
        let mut browser = Self {
            peer,
            dir: dir.clone(),
            entries: Vec::new(),
            filter: Input::new(MAX_LINE),
            selected: 0,
            places: (fs.places)(),
            place_selected: 0,
            pane: Pane::Files,
            error: None,
        };
        if browser.go(fs, &dir).is_err() {
            let start = (fs.start)();
            if browser.go(fs, &start).is_err()
                && let Some(first) = browser.places.first().map(|p| p.path.clone())
            {
                let _ = browser.go(fs, &first);
            }
        }
        browser
    }

    /// What is typed, as a path to go to, when it looks like one: it has a
    /// slash, or it is a drive (`D:`), or it starts with `~`.
    pub fn typed_path(&self) -> Option<PathBuf> {
        let text = super::send::unquote(self.filter.text());
        let looks = text.contains(['/', '\\'])
            || text.starts_with('~')
            || (text.len() == 2 && text.ends_with(':'));
        if !looks {
            return None;
        }
        let path = match text.strip_prefix('~') {
            Some(rest) => dirs::home_dir()
                .unwrap_or_default()
                .join(rest.trim_start_matches(['/', '\\'])),
            None if text.len() == 2 => PathBuf::from(format!("{text}\\")),
            None => PathBuf::from(&text),
        };
        Some(if path.is_absolute() {
            path
        } else {
            self.dir.join(path)
        })
    }

    /// The lines on the right, after the filter.
    pub fn rows(&self) -> Vec<Row> {
        if self.typed_path().is_some() {
            return Vec::new();
        }
        let filter = self.filter.text().to_lowercase();
        let mut rows = Vec::new();
        if filter.is_empty() && self.dir.parent().is_some() {
            rows.push(Row::Up);
        }
        let show_hidden = filter.starts_with('.');
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.hidden && !show_hidden {
                continue;
            }
            if filter.is_empty() || entry.name.to_lowercase().contains(&filter) {
                rows.push(Row::Entry(i));
            }
        }
        rows
    }

    /// Goes into `dir`. On failure nothing changes and the reason is shown.
    pub fn go(&mut self, fs: &Fs, dir: &Path) -> Result<(), ()> {
        match (fs.list)(dir) {
            Ok(mut entries) => {
                sort(&mut entries);
                let cut = entries.len() > MAX_ENTRIES;
                entries.truncate(MAX_ENTRIES);
                self.dir = dir.to_path_buf();
                self.entries = entries;
                self.filter = Input::new(MAX_LINE);
                // The first real line, not `..`.
                let rows = self.rows();
                self.selected = usize::from(rows.first() == Some(&Row::Up) && rows.len() > 1);
                self.error =
                    cut.then(|| format!("Showing the first {MAX_ENTRIES}; type to find the rest."));
                Ok(())
            }
            Err(e) => {
                self.error = Some(format!(
                    "Can't open {}: {}",
                    crate::untrusted::name(&dir.display().to_string()),
                    crate::untrusted::text(&e.to_string())
                ));
                Err(())
            }
        }
    }

    /// Up one folder, with the cursor on the one just left.
    pub fn up(&mut self, fs: &Fs) {
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else {
            // At the top of a drive: the drives are on the left.
            self.pane = Pane::Places;
            return;
        };
        let left = self
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        if self.go(fs, &parent).is_ok()
            && let Some(left) = left
        {
            let rows = self.rows();
            if let Some(at) = rows
                .iter()
                .position(|r| matches!(r, Row::Entry(i) if self.entries[*i].name == left))
            {
                self.selected = at;
            }
        }
    }

    /// Enter on the right: a folder opens, `..` goes up, a file is chosen; a
    /// typed path goes to that folder or chooses that file.
    pub fn enter(&mut self, fs: &Fs) -> Chosen {
        if let Some(path) = self.typed_path() {
            if path.is_dir() {
                let _ = self.go(fs, &path);
            } else if path.is_file() {
                return Chosen::File(path);
            } else {
                self.error = Some(format!(
                    "Nothing at {}.",
                    crate::untrusted::name(&path.display().to_string())
                ));
            }
            return Chosen::Nothing;
        }
        match self.rows().get(self.selected) {
            Some(Row::Up) => self.up(fs),
            Some(Row::Entry(i)) => {
                let entry = &self.entries[*i];
                let path = self.dir.join(&entry.name);
                if entry.is_dir {
                    let _ = self.go(fs, &path);
                } else {
                    return Chosen::File(path);
                }
            }
            None => {}
        }
        Chosen::Nothing
    }

    /// Enter on the left: go to that place.
    pub fn enter_place(&mut self, fs: &Fs) {
        if let Some(place) = self.places.get(self.place_selected).cloned()
            && self.go(fs, &place.path).is_ok()
        {
            self.pane = Pane::Files;
        }
    }

    pub fn move_by(&mut self, by: isize) {
        let (len, at) = match self.pane {
            Pane::Places => (self.places.len(), &mut self.place_selected),
            Pane::Files => (self.rows().len(), &mut self.selected),
        };
        let last = len.saturating_sub(1) as isize;
        *at = (*at as isize + by).clamp(0, last.max(0)) as usize;
    }

    /// After the filter changed: the first match is highlighted.
    pub fn filtered(&mut self) {
        self.selected = 0;
        self.error = None;
    }
}

/// Folders first, then files; within each, case-insensitive natural order,
/// so `chapter-2` comes before `chapter-10`.
pub fn sort(entries: &mut [DirEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| natural(&a.name, &b.name))
    });
}

fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (x, y) = (take(&mut a), take(&mut b));
                let (xs, ys) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = xs.len().cmp(&ys.len()).then_with(|| xs.cmp(ys));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Reads a folder from the disk. Entries that cannot be looked at are
/// skipped rather than failing the whole folder.
pub fn list_dir(dir: &Path) -> std::io::Result<Vec<DirEntry>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Follows a link, so a linked folder opens like a folder.
        let Ok(meta) = std::fs::metadata(entry.path()) else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        out.push(DirEntry {
            hidden: is_hidden(&name, &meta),
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified,
            name,
        });
        if out.len() > MAX_ENTRIES * 2 {
            break;
        }
    }
    Ok(out)
}

#[cfg(windows)]
fn is_hidden(name: &str, meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    name.starts_with('.') || meta.file_attributes() & (HIDDEN | SYSTEM) != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _meta: &std::fs::Metadata) -> bool {
    name.starts_with('.')
}

/// The usual folders, then every drive (Windows) or the root and mounted
/// disks (Linux, macOS).
pub fn places() -> Vec<Place> {
    let mut out = Vec::new();
    let mut add = |label: &str, path: Option<PathBuf>, drive: bool| {
        if let Some(path) = path.filter(|p| p.is_dir())
            && !out.iter().any(|p: &Place| p.path == path)
        {
            out.push(Place {
                label: label.to_string(),
                path,
                drive,
            });
        }
    };
    add("Home", dirs::home_dir(), false);
    add("Desktop", dirs::desktop_dir(), false);
    add("Documents", dirs::document_dir(), false);
    add("Downloads", dirs::download_dir(), false);
    add("This folder", std::env::current_dir().ok(), false);
    if cfg!(windows) {
        for letter in 'A'..='Z' {
            let root = PathBuf::from(format!("{letter}:\\"));
            if std::fs::metadata(&root).is_ok() {
                add(&format!("{letter}:"), Some(root), true);
            }
        }
    } else {
        add("/", Some(PathBuf::from("/")), true);
        let user = std::env::var("USER").unwrap_or_default();
        for base in [
            format!("/media/{user}"),
            format!("/run/media/{user}"),
            "/mnt".to_string(),
            "/Volumes".to_string(),
        ] {
            if let Ok(entries) = std::fs::read_dir(&base) {
                for entry in entries.flatten() {
                    let label = entry.file_name().to_string_lossy().into_owned();
                    add(&label, Some(entry.path()), true);
                }
            }
        }
    }
    out
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A made-up tree: /home/me with docs/ (a.txt, report-2.pdf,
    /// report-10.pdf), music/, .secret, notes.txt.
    pub fn fake() -> Fs {
        Fs {
            list: |dir| {
                let file = |name: &str| DirEntry {
                    name: name.into(),
                    is_dir: false,
                    size: 1024,
                    modified: Some(0),
                    hidden: name.starts_with('.'),
                };
                let folder = |name: &str| DirEntry {
                    is_dir: true,
                    size: 0,
                    ..file(name)
                };
                let unix = dir.to_string_lossy().replace('\\', "/");
                match unix.trim_end_matches('/') {
                    "" => Ok(vec![folder("home")]),
                    "/home" => Ok(vec![folder("me")]),
                    "/home/me" => Ok(vec![
                        file("notes.txt"),
                        folder("music"),
                        file(".secret"),
                        folder("docs"),
                    ]),
                    "/home/me/docs" => Ok(vec![
                        file("report-10.pdf"),
                        file("a.txt"),
                        file("report-2.pdf"),
                    ]),
                    "/home/me/music" => Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "access denied",
                    )),
                    _ => Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "not found",
                    )),
                }
            },
            places: || {
                vec![
                    Place {
                        label: "Home".into(),
                        path: PathBuf::from("/home/me"),
                        drive: false,
                    },
                    Place {
                        label: "/".into(),
                        path: PathBuf::from("/"),
                        drive: true,
                    },
                ]
            },
            start: || PathBuf::from("/home/me"),
        }
    }

    fn names(b: &Browser) -> Vec<String> {
        b.rows()
            .iter()
            .map(|r| match r {
                Row::Up => "..".to_string(),
                Row::Entry(i) => b.entries[*i].name.clone(),
            })
            .collect()
    }

    fn open() -> Browser {
        Browser::open(&fake(), "alice".into(), PathBuf::from("/home/me"))
    }

    #[test]
    fn folders_come_first_hidden_files_stay_hidden_and_the_cursor_skips_dot_dot() {
        let b = open();
        assert_eq!(names(&b), ["..", "docs", "music", "notes.txt"]);
        assert_eq!(b.selected, 1);
    }

    #[test]
    fn numbers_sort_as_numbers() {
        let mut b = open();
        b.go(&fake(), Path::new("/home/me/docs")).unwrap();
        assert_eq!(names(&b), ["..", "a.txt", "report-2.pdf", "report-10.pdf"]);
    }

    #[test]
    fn enter_opens_a_folder_and_chooses_a_file_and_up_returns_to_where_it_was() {
        let fs = fake();
        let mut b = open();
        assert_eq!(b.enter(&fs), Chosen::Nothing, "docs opens");
        assert_eq!(b.dir, PathBuf::from("/home/me/docs"));
        b.move_by(1);
        assert_eq!(
            b.enter(&fs),
            Chosen::File(PathBuf::from("/home/me/docs").join("report-2.pdf"))
        );
        b.up(&fs);
        assert_eq!(b.dir, PathBuf::from("/home/me"));
        assert_eq!(
            names(&b)[b.selected],
            "docs",
            "the cursor is on the folder just left"
        );
    }

    #[test]
    fn typing_filters_and_a_dot_shows_hidden_files() {
        let mut b = open();
        b.filter.paste("O");
        b.filtered();
        assert_eq!(names(&b), ["docs", "notes.txt"]);
        let mut b = open();
        b.filter.paste(".s");
        assert_eq!(names(&b), [".secret"]);
    }

    #[test]
    fn a_folder_that_cannot_be_opened_says_why_and_stays_put() {
        let fs = fake();
        let mut b = open();
        b.move_by(1); // music
        assert_eq!(b.enter(&fs), Chosen::Nothing);
        assert_eq!(b.dir, PathBuf::from("/home/me"));
        assert!(b.error.as_deref().unwrap().contains("access denied"));
    }

    #[test]
    fn places_jump_and_up_from_the_top_goes_to_the_places() {
        let fs = fake();
        let mut b = open();
        b.pane = Pane::Places;
        b.place_selected = 1;
        b.enter_place(&fs);
        assert_eq!(b.dir, PathBuf::from("/"));
        assert_eq!(b.pane, Pane::Files);
        b.up(&fs);
        assert_eq!(b.pane, Pane::Places, "nothing above a drive's top");
    }

    #[test]
    fn a_typed_path_is_taken_as_a_path() {
        let b = {
            let mut b = open();
            b.filter.paste("\"docs/\"");
            b
        };
        assert_eq!(
            b.typed_path(),
            Some(PathBuf::from("/home/me").join("docs/"))
        );
        assert!(b.rows().is_empty(), "the list steps aside for a path");
        let mut b = open();
        b.filter.paste("q3");
        assert_eq!(b.typed_path(), None, "a plain word filters");
        b.filter = Input::new(MAX_LINE);
        b.filter.paste("D:");
        assert_eq!(b.typed_path(), Some(PathBuf::from("D:\\")));
    }

    #[test]
    fn a_missing_start_folder_falls_back() {
        let b = Browser::open(&fake(), "alice".into(), PathBuf::from("/gone"));
        assert_eq!(b.dir, PathBuf::from("/home/me"));
    }

    #[test]
    fn the_real_disk_lists_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        let mut entries = list_dir(dir.path()).unwrap();
        sort(&mut entries);
        let names: Vec<_> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_dir))
            .collect();
        assert_eq!(names, [("a", true), ("b.txt", false)]);
        assert!(!places().is_empty(), "at least the current folder");
    }
}
