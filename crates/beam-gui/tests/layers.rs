//! beam-gui's place in the layers (ADR-0048, ADR-0049): a front end on the
//! core and the model. It never uses the other front end (`beam`: the CLI
//! and the full-screen view) or a terminal library, so every flow and every
//! safe default it offers comes from the code the CLI and the view share.

#[test]
fn the_gui_depends_only_on_the_core_the_model_iced_and_rfd() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let deps = manifest
        .split("\n[dependencies]")
        .nth(1)
        .expect("a [dependencies] table")
        .split("\n[")
        .next()
        .unwrap();
    let named: Vec<&str> = deps
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split(['.', ' ', '=']).next().unwrap())
        .collect();
    assert_eq!(
        named,
        ["beam-core", "beam-model", "iced", "rfd"],
        "beam-gui depends on {named:?}"
    );
}
