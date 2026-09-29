# Beam salvage result

## Kept

- `crates/beam/src/transfer/` — the existing transfer protocol and reliable file-transfer engine.
- `crates/beam/src/identity/` — retained for the future trust/authentication/social layer.
- CLI, terminal UI, progress reporting, partial-transfer management, and file storage.

## Reworked

- `transport/` — replaced iroh endpoint/dial code with direct TCP client + daemon.
- `listener.rs` — now starts the direct TCP Beam daemon.
- `cli/net_cmds.rs` — direct-address `beam send` and foreground daemon command.
- `config.rs` — reduced to local TCP port configuration.
- receiver policy — Stage 1 can accept an unpaired direct peer, while retaining the public-key field for future authentication.

## Removed

- `crates/beam-server/`
- `crates/beam/src/rendezvous/`
- `crates/beam/src/pairing/`
- iroh-specific transport files
- iroh/rendezvous/pairing dependencies
- obsolete integration tests tied to the old centralized architecture
- obsolete Go Makefile and old iroh/server project documentation/spikes

## Validation limitation

The execution environment used for this salvage build does not contain the
Rust/Cargo toolchain, so `cargo check`, `cargo test`, and `cargo clippy` could
not be executed here. The package has therefore been checked by source-level
reference/dependency inspection, and `Cargo.lock` was intentionally removed so
Cargo can regenerate it from the new dependency graph on the target machine.
