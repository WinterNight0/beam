//! beam's layers (ADR-0048): the core never uses anything above it.
//!
//! The core is everything that makes beam secure and correct: identity,
//! pairing, transfers, the listener, the agent. Above it sit the actions
//! (`actions/`, one copy of each flow, in this crate too) and the front ends
//! (the `beam` crate: CLI and full-screen view).
//!
//! Cargo enforces the front-end half: this crate does not depend on `beam`
//! or on any terminal or GUI library, so it cannot name them. The actions
//! share this crate with the core, so that half is checked here.

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

/// The layers above the core, as they would be named from inside it.
const ABOVE: &[&str] = &["actions", "beam", "beam_model", "cli", "tui", "ui"];

fn rust_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            rust_files(&entry.unwrap().path(), out);
        }
    } else if path.extension().is_some_and(|e| e == "rs") {
        out.push(path.to_path_buf());
    }
}

/// `crate::actions`, `crate::{actions, …}`, `beam::cli` and the like.
fn names_a_layer_above(line: &str) -> Option<&'static str> {
    let code = line.split("//").next().unwrap_or("");
    let names = |prefix: &str, layer: &str| {
        let direct = format!("{prefix}{layer}");
        code.match_indices(&direct).any(|(i, _)| {
            let before_ok = !code[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_');
            let after_ok =
                !code[i + direct.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_');
            before_ok && after_ok
        })
    };
    ABOVE.iter().copied().find(|layer| {
        let braced = code.find("crate::{").is_some_and(|i| {
            code[i + "crate::{".len()..]
                .split(['}', ','])
                .any(|name| name.trim() == *layer)
        });
        // A crate above, named by its own path: `beam::…`, `beam_model::…`.
        let crate_above = |name: &str| {
            *layer == name
                && code
                    .match_indices(&format!("{name}::"))
                    .any(|(i, _)| !code[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
        };
        let front_end_crate = crate_above("beam") || crate_above("beam_model");
        names("crate::", layer) || front_end_crate || braced
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
    let placed: Vec<&str> = CORE.iter().copied().chain(["actions", "lib.rs"]).collect();
    for entry in std::fs::read_dir(&src).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            placed.contains(&name.as_str()),
            "src/{name} is in no layer: add it to CORE in tests/layers.rs, or under actions/"
        );
    }
}

/// The crate that holds the core lists no front-end library: the compiler,
/// not this test, then keeps terminal and GUI code out of it.
#[test]
fn the_core_crate_depends_on_no_front_end_library() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for front_end in [
        "ratatui",
        "crossterm",
        "clap",
        "iced",
        "rfd",
        "\nbeam ",
        "beam-model",
    ] {
        assert!(
            !manifest.contains(&format!("{front_end}.workspace"))
                && !manifest.contains(&format!("{front_end} =")),
            "beam-core depends on {front_end:?}"
        );
    }
}

#[test]
fn the_check_catches_what_it_should() {
    assert_eq!(
        names_a_layer_above("    crate::actions::messages::refused(p, e)"),
        Some("actions")
    );
    assert_eq!(
        names_a_layer_above("use crate::{actions, untrusted};"),
        Some("actions")
    );
    assert_eq!(names_a_layer_above("use beam::cli::Io;"), Some("beam"));
    assert_eq!(
        names_a_layer_above("use beam_model::Ask;"),
        Some("beam_model")
    );
    assert_eq!(names_a_layer_above("use crate::untrusted;"), None);
    assert_eq!(
        names_a_layer_above("use crate::{transfer, transport};"),
        None
    );
    assert_eq!(names_a_layer_above("// see crate::actions"), None);
    assert_eq!(names_a_layer_above("use beam_core::identity;"), None);
}
