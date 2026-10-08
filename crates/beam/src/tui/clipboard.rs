//! Ctrl+C in the full-screen view: putting text on the clipboard without a
//! new dependency.
//!
//! The OS's own tool is tried first (`clip.exe`, `pbcopy`, `wl-copy`,
//! `xclip`, `xsel`), the same approach as the agent's notifications
//! (ADR-0042). If none is there — a Linux server over SSH, say — the text
//! goes to the terminal as an OSC 52 sequence, which Windows Terminal and
//! most modern terminals turn into a copy.
//!
//! `clip.exe` is given UTF-16 with a byte order mark, the one encoding it
//! reads without guessing a code page, so Thai and other non-ASCII text in
//! a command's output survives the copy.

use std::io::Write;
use std::process::{Command, Stdio};

use base64::Engine;

/// How the text was copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copied {
    /// By the OS's clipboard tool.
    System,
    /// Handed to the terminal (OSC 52); whether it copies is up to it.
    Terminal,
}

pub fn copy(text: &str) -> Copied {
    for (program, args) in tools() {
        if pipe_to(program, args, text) {
            return Copied::System;
        }
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
    Copied::Terminal
}

/// The bytes a tool reads: UTF-16LE with a BOM for `clip.exe`, else UTF-8.
fn encode(program: &str, text: &str) -> Vec<u8> {
    if program == "clip.exe" {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    } else {
        text.as_bytes().to_vec()
    }
}

fn tools() -> &'static [(&'static str, &'static [&'static str])] {
    if cfg!(windows) {
        &[("clip.exe", &[])]
    } else if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    }
}

/// Runs `program` with `text` on its stdin; whether it took it.
fn pipe_to(program: &str, args: &[&str], text: &str) -> bool {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(&encode(program, text)).is_ok());
    // Dropping stdin above closes it, so the tool sees the end and exits.
    child.wait().is_ok_and(|status| status.success()) && written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_exe_gets_utf16_with_a_byte_order_mark() {
        assert_eq!(encode("clip.exe", "aก"), [0xFF, 0xFE, b'a', 0, 0x01, 0x0E]);
        assert_eq!(encode("pbcopy", "aก"), "aก".as_bytes());
    }
}
