//! beam-model's place in the layers (ADR-0048): it sits on the core and
//! under the front ends, so it may use `beam-core` and nothing else from
//! beam, and no terminal or GUI library. A front end draws the model; the
//! model never draws.

#[test]
fn the_model_depends_only_on_the_core() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let deps = manifest
        .split("[dependencies]")
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
    assert_eq!(named, ["beam-core"], "beam-model depends on {named:?}");
}
