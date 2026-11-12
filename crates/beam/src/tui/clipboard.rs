//! Ctrl+C in the full-screen view: putting text on the clipboard without a
//! new dependency.
//!
//! The OS's own tool is tried first (`clip.exe`, `pbcopy`, `wl-copy`,
//! `xclip`, `xsel`), the same approach as the agent's notifications
//! (ADR-0042). If none is there — a Linux server over SSH, say — the text
//! goes to the terminal as an OSC 52 sequence, which Windows Terminal and
//! most modern terminals turn into a copy.
//!
//! Ctrl+V the other way round: Windows Terminal and most terminals paste by
//! themselves, but the classic Windows console does not, so beam reads the
//! clipboard with the same kind of tools ([`paste`]) and pastes it itself.
//!
//! `clip.exe` is given UTF-16 with a byte order mark, the one encoding it
//! reads without guessing a code page, so Thai and other non-ASCII text in
//! a command's output survives the copy.

use std::io::Write;
use std::process::{Command, Stdio};

use base64::Engine;

/// The most text a paste brings in.
const MAX_PASTE: usize = 64 * 1024;

/// Reads the clipboard, for Ctrl+V in terminals that do not paste for us
/// (the classic Windows console). `None` if no tool could read it.
pub fn paste() -> Option<String> {
    let mut text = read_with_tool()?;
    // Copied lines usually end with a line break nobody meant to paste.
    while text.ends_with(['\r', '\n']) {
        text.pop();
    }
    if text.len() > MAX_PASTE {
        let mut cut = MAX_PASTE;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    Some(text)
}

fn read_with_tool() -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Get-Clipboard -Raw",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
    #[cfg(not(windows))]
    {
        let tools: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
            &[("pbpaste", &[])]
        } else {
            &[
                ("wl-paste", &["--no-newline"]),
                ("xclip", &["-selection", "clipboard", "-o"]),
                ("xsel", &["--clipboard", "--output"]),
            ]
        };
        tools.iter().find_map(|(program, args)| {
            let out = Command::new(program)
                .args(*args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        })
    }
}

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
