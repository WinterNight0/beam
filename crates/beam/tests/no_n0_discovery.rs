//! S-17: beam never installs n0's discovery services.
//!
//! iroh's `presets::N0` publishes the device's endpoint id and relay URL to
//! n0's pkarr server every five minutes and resolves other devices the same
//! way. beam replaces that with its own rendezvous server and builds its
//! endpoint from `presets::Minimal`; `docs/n0-data.md` explains why.
//!
//! iroh does not offer a way to ask a bound endpoint which address lookup
//! services it has, so this is checked where the decision is made: in the
//! source. Like the flag allowlists in `cli.rs`, the test fails the moment
//! anyone reaches for one of these, and makes them read this first.

use std::path::{Path, PathBuf};

/// Anything that would install discovery, or fall back to n0's full default
/// relay set instead of the configured one.
const FORBIDDEN: [&str; 7] = [
    "presets::N0",
    "PkarrPublisher",
    "PkarrResolver",
    "DnsAddressLookup",
    ".address_lookup(",
    "MdnsAddressLookup",
    "RelayMode::Default",
];

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// The source of both crates, without comments: the rule is about what the
/// code does, and the doc comments that explain the rule have to name the
/// things they forbid.
fn code_lines() -> Vec<(PathBuf, usize, String)> {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&crate_dir.join("src"), &mut files);
    rust_files(&crate_dir.join("../beam-server/src"), &mut files);
    assert!(files.len() > 10, "found only {} source files", files.len());

    let mut lines = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read source file");
        for (number, line) in text.lines().enumerate() {
            let code = match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            };
            lines.push((file.clone(), number + 1, code.to_string()));
        }
    }
    lines
}

#[test]
fn no_discovery_service_is_ever_installed() {
    for (file, number, code) in code_lines() {
        for needle in FORBIDDEN {
            assert!(
                !code.contains(needle),
                "{}:{number} uses {needle}. beam must not install n0's discovery \
                 or default relays; see docs/n0-data.md and S-17.",
                file.display()
            );
        }
    }
}

#[test]
fn the_endpoint_is_built_from_the_minimal_preset() {
    let builders: Vec<_> = code_lines()
        .into_iter()
        .filter(|(_, _, code)| code.contains("Endpoint::builder("))
        .collect();
    assert!(!builders.is_empty(), "no endpoint is built anywhere");
    for (file, number, code) in builders {
        assert!(
            code.contains("Endpoint::builder(presets::Minimal)"),
            "{}:{number} builds an endpoint from something other than \
             presets::Minimal: {code}",
            file.display()
        );
    }
}
