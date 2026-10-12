//! beam's layers (ADR-0048): the core never uses anything above it.
//!
//! The core is everything that makes beam secure and correct: identity,
//! pairing, transfers, the listener, the agent. Above it sit the actions
//! (`actions/`, one copy of each flow) and the front ends (`cli/`, `tui/`,
//! `ui.rs`). A core module that printed, drew, or called into a front end
//! would tie every front end to that one, and is refused here.
//!
//! Once the core is its own crate, Cargo enforces the front-end half of
//! this by itself; the `actions` half stays a test.

use std::path::{Path, PathBuf};

/// The core, as paths under `src/`.
const CORE: &[&str] = &[
    "agent",
    "config.rs",
    "hex.rs",
    "history.rs",
    "identity",
    "invite.rs",
    "listen_status.rs",
    "listener.rs",
    "pairing",
    "text.rs",
    "transfer",
    "transport",
    "untrusted.rs",
];

/// What the core may not name.
const ABOVE: &[&str] = &["actions", "cli", "tui", "ui"];

fn rust_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            rust_files(&entry.unwrap().path(), out);
        }
    } else if path.extension().is_some_and(|e| e == "rs") {
        out.push(path.to_path_buf());
    }
}

/// `crate::cli`, `crate::{ui, …}`, `crate::cli::…` and the like.
fn names_a_layer_above(line: &str) -> Option<&'static str> {
    let code = line.split("//").next().unwrap_or("");
    ABOVE.iter().copied().find(|layer| {
        let direct = format!("crate::{layer}");
        let at = code.match_indices(&direct).any(|(i, _)| {
            !code[i + direct.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
        });
        let braced = code.find("crate::{").is_some_and(|i| {
            code[i + "crate::{".len()..]
                .split(['}', ','])
                .any(|name| name.trim() == *layer)
        });
        at || braced
    })
}

#[test]
fn the_core_never_uses_the_layers_above_it() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    for part in CORE {
        let path = src.join(part);
        assert!(
            path.exists(),
            "{} is listed as core but is gone",
            path.display()
        );
        rust_files(&path, &mut files);
    }
    let mut found = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            if let Some(layer) = names_a_layer_above(line) {
                found.push(format!(
                    "{}:{}: uses {layer}: {}",
                    file.display(),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "the core reaches up a layer:\n{}",
        found.join("\n")
    );
}

#[test]
fn every_module_is_placed_in_a_layer() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let placed: Vec<&str> = CORE
        .iter()
        .copied()
        .chain(["actions", "cli", "tui", "ui.rs", "lib.rs", "main.rs"])
        .collect();
    for entry in std::fs::read_dir(&src).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            placed.contains(&name.as_str()),
            "src/{name} is in no layer: add it to CORE in tests/layers.rs, or to the layer it belongs to"
        );
    }
}

#[test]
fn the_check_catches_what_it_should() {
    assert_eq!(names_a_layer_above("use crate::cli::Io;"), Some("cli"));
    assert_eq!(
        names_a_layer_above("use crate::{text, ui, untrusted};"),
        Some("ui")
    );
    assert_eq!(
        names_a_layer_above("    crate::actions::messages::refused(p, e)"),
        Some("actions")
    );
    assert_eq!(names_a_layer_above("use crate::untrusted;"), None);
    assert_eq!(
        names_a_layer_above("use crate::{transfer, transport};"),
        None
    );
    assert_eq!(
        names_a_layer_above("// see crate::cli for the commands"),
        None
    );
}
