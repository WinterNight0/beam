//! Turning a name a peer sent into a file this machine is willing to create.
//!
//! Everything here treats the incoming name as hostile. The name is reduced to
//! a bare base name, anything still dangerous is refused, and the destination
//! is reserved with `create_new` so that two transfers arriving at once cannot
//! both decide the same name is free. See ADR-0017.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// The longest name we will create, in bytes.
const MAX_NAME_BYTES: usize = 255;

/// How many `name (n).ext` variants to try before giving up.
const MAX_COLLISION_ATTEMPTS: u32 = 1000;

/// Base names Windows refuses to create, with or without an extension.
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters no file name may contain on either platform family.
const FORBIDDEN: [char; 6] = ['<', '>', '"', '|', '?', '*'];

/// Why an incoming file name was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NameError {
    #[error("the peer sent an empty file name")]
    Empty,
    #[error("the peer sent {0:?}, which does not name a file")]
    NotAName(String),
    #[error("the peer sent a file name containing a control character")]
    ControlCharacter,
    #[error("the peer sent a file name containing {0:?}")]
    ForbiddenCharacter(char),
    #[error("the peer sent {0:?}, which is a reserved device name on Windows")]
    Reserved(String),
    #[error("the peer sent a file name ending in a dot or a space")]
    TrailingDotOrSpace,
    #[error("the peer sent a file name of {0} bytes, the limit is {MAX_NAME_BYTES}")]
    TooLong(usize),
}

/// Reduces a name from a peer to a safe base name, or refuses it.
///
/// Directory separators are stripped rather than rejected — `../../etc/passwd`
/// becomes `passwd` — because the useful part of such a name is still the last
/// segment. What cannot be reduced to a plain name is refused outright.
pub fn sanitize_file_name(raw: &str) -> Result<String, NameError> {
    if raw.is_empty() {
        return Err(NameError::Empty);
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err(NameError::ControlCharacter);
    }

    // Both separators, whatever platform we are on: a name from a Windows peer
    // reaching a Unix receiver must still be cut apart.
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");

    if base.is_empty() || base == "." || base == ".." {
        return Err(NameError::NotAName(raw.to_string()));
    }
    // A colon is a drive letter (`C:evil.txt` is relative to another drive) or
    // an NTFS alternate data stream (`report.pdf:hidden`). Neither is a name.
    if let Some(c) = base.chars().find(|c| *c == ':' || FORBIDDEN.contains(c)) {
        return Err(NameError::ForbiddenCharacter(c));
    }
    // Windows silently strips these, so `evil.txt.` and `evil.txt` are the same
    // file there but different names here.
    if base.ends_with('.') || base.ends_with(' ') {
        return Err(NameError::TrailingDotOrSpace);
    }
    if is_windows_reserved(base) {
        return Err(NameError::Reserved(base.to_string()));
    }
    if base.len() > MAX_NAME_BYTES {
        return Err(NameError::TooLong(base.len()));
    }

    Ok(base.to_string())
}

/// Whether a name collides with a Windows device, ignoring any extension.
fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    WINDOWS_RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// Splits a name into the part before the extension and the extension itself.
///
/// A leading dot belongs to the stem, so `.gitignore` keeps its whole name
/// instead of becoming ` (1).gitignore`.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(index) if index > 0 => (&name[..index], &name[index..]),
        _ => (name, ""),
    }
}

/// Picks a free name in `dir` and creates the file, so the name is taken.
///
/// Returns the open file and the name it ended up with. Creating the file here
/// rather than checking whether it exists is what closes the gap between
/// deciding a name is free and using it.
pub fn reserve_destination(dir: &Path, name: &str) -> std::io::Result<(File, PathBuf, String)> {
    let (stem, extension) = split_extension(name);

    for attempt in 0..MAX_COLLISION_ATTEMPTS {
        let candidate = if attempt == 0 {
            name.to_string()
        } else {
            format!("{stem} ({attempt}){extension}")
        };
        let path = dir.join(&candidate);

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path, candidate)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!(
            "{name} and {MAX_COLLISION_ATTEMPTS} variations of it already exist in {}",
            dir.display()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_names_pass_through() {
        for name in [
            "report.pdf",
            "project.zip",
            "a",
            ".gitignore",
            "two words.txt",
            "ünïcode.txt",
            "name.tar.gz",
        ] {
            assert_eq!(sanitize_file_name(name).as_deref(), Ok(name));
        }
    }

    #[test]
    fn traversal_attempts_are_reduced_to_a_base_name() {
        let cases = [
            ("../../etc/passwd", "passwd"),
            ("..\\..\\windows\\system32\\evil.dll", "evil.dll"),
            ("/etc/passwd", "passwd"),
            ("/absolute/path/report.pdf", "report.pdf"),
            ("foo/../../bar.txt", "bar.txt"),
            ("./relative.txt", "relative.txt"),
            ("dir/sub/deep.txt", "deep.txt"),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                sanitize_file_name(raw).as_deref(),
                Ok(expected),
                "input {raw:?}"
            );
        }
    }

    #[test]
    fn names_that_cannot_be_reduced_are_refused() {
        let cases: [(&str, NameError); 12] = [
            ("", NameError::Empty),
            (".", NameError::NotAName(".".to_string())),
            ("..", NameError::NotAName("..".to_string())),
            ("foo/", NameError::NotAName("foo/".to_string())),
            ("../..", NameError::NotAName("../..".to_string())),
            ("a\0b", NameError::ControlCharacter),
            ("a\nb", NameError::ControlCharacter),
            ("C:evil.txt", NameError::ForbiddenCharacter(':')),
            ("report.pdf:hidden", NameError::ForbiddenCharacter(':')),
            ("what?.txt", NameError::ForbiddenCharacter('?')),
            ("evil.txt.", NameError::TrailingDotOrSpace),
            ("evil.txt ", NameError::TrailingDotOrSpace),
        ];
        for (raw, expected) in cases {
            assert_eq!(sanitize_file_name(raw), Err(expected), "input {raw:?}");
        }
    }

    #[test]
    fn windows_device_names_are_refused() {
        for name in [
            "CON",
            "con",
            "NUL",
            "nul.txt",
            "COM1",
            "lpt9.tar.gz",
            "AUX",
            "PRN",
        ] {
            assert!(
                matches!(sanitize_file_name(name), Err(NameError::Reserved(_))),
                "accepted {name:?}"
            );
        }
        // These merely start with the same letters and are perfectly fine.
        for name in ["console.log", "connection.txt", "com10.txt", "nulls.csv"] {
            assert!(sanitize_file_name(name).is_ok(), "refused {name:?}");
        }
    }

    #[test]
    fn overlong_names_are_refused() {
        let long = format!("{}.txt", "x".repeat(300));
        assert!(matches!(
            sanitize_file_name(&long),
            Err(NameError::TooLong(_))
        ));
        let at_limit = "x".repeat(MAX_NAME_BYTES);
        assert!(sanitize_file_name(&at_limit).is_ok());
    }

    #[test]
    fn a_sanitized_name_always_stays_in_the_destination_directory() {
        // The real property: whatever a peer sends, the file we open is a
        // direct child of the directory we chose.
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        let hostile = [
            "../../etc/passwd",
            "..\\..\\windows\\system32\\evil.dll",
            "/etc/passwd",
            "foo/../../bar.txt",
            "./relative.txt",
            "deep/nested/path/file.bin",
        ];
        for raw in hostile {
            let name = sanitize_file_name(raw).expect("should reduce to a base name");
            let (_file, path, _final_name) = reserve_destination(dir, &name).expect("reserve");
            assert_eq!(path.parent(), Some(dir), "escaped with {raw:?} -> {path:?}");
        }
    }

    #[test]
    fn extensions_split_the_way_a_person_would_expect() {
        assert_eq!(split_extension("report.pdf"), ("report", ".pdf"));
        assert_eq!(split_extension("name.tar.gz"), ("name.tar", ".gz"));
        assert_eq!(split_extension("noextension"), ("noextension", ""));
        assert_eq!(split_extension(".gitignore"), (".gitignore", ""));
    }

    #[test]
    fn colliding_names_are_numbered() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        let expected = ["report.pdf", "report (1).pdf", "report (2).pdf"];
        for want in expected {
            let (_file, path, name) = reserve_destination(dir, "report.pdf").expect("reserve");
            assert_eq!(name, want);
            assert_eq!(path, dir.join(want));
        }
    }

    #[test]
    fn a_dotfile_collides_without_losing_its_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        let (_f, _p, first) = reserve_destination(dir, ".gitignore").expect("reserve");
        assert_eq!(first, ".gitignore");
        let (_f, _p, second) = reserve_destination(dir, ".gitignore").expect("reserve");
        assert_eq!(second, ".gitignore (1)");
    }

    #[test]
    fn reserving_a_name_actually_creates_the_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (_file, path, _name) = reserve_destination(tmp.path(), "held.bin").expect("reserve");
        assert!(path.exists(), "the name was not reserved on disk");
    }
}
