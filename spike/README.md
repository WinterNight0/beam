# Spikes

Throwaway code that answers a question. Nothing here is part of the product.

Each crate declares its own empty `[workspace]`, so it is detached from the
main workspace: `cargo build` at the repository root does not build any of it,
and a spike cannot change what ships.

| Directory | Question | Findings |
|---|---|---|
| `iroh-transport/` | Can beam's transfer engine run over iroh unchanged, and can beam's Ed25519 key be the peer identity? | [`docs/spikes/transport.md`](../docs/spikes/transport.md) |
| `measure/` | What do the three candidate transports cost in dependencies and build time? | same |
| `gui-iced/` | Is iced a fit for beam's GUI: architecture, file picker, tests, size? | [`docs/spikes/gui.md`](../docs/spikes/gui.md) |

## Running the iroh prototype

```
cd iroh-transport && cargo build --release

# terminal 1
./target/release/iroh-transport-spike listen --beam-dir <dir> --out <dir>

# terminal 2, using the endpoint id and local address the listener printed
./target/release/iroh-transport-spike send --beam-dir <dir> --to <id> --addr <ip:port> <file>
```

`--addr` is only needed on one machine, where iroh's discovery service is not
in play. Across two networks, leave it out; see the testing steps in the
findings.

## Running the GUI prototype

```
cd gui-iced && cargo run --release
cargo test                                  # headless UI tests
ICED_BACKEND=tiny-skia cargo run --release  # force the software renderer
```

It lists the friends in `~/.beam` (or `$BEAM_DIR`) and sends nothing.

## Running the GUI mock-up

```
cd gui-iced && cargo run --release --bin mockup
MOCKUP_START=pending,accept cargo run --release --bin mockup   # open in a given state
```

A styled, made-up beam window (not linked to beam) to show what iced can
look like. Its `src/bin/mockup/style.rs` is the counterpart of a JavaFX
stylesheet; each style function shows the CSS it replaces. Start states:
`pending`, `history`, `on`, `accept`, `invite`, `code`, `sending`, `light`.

The `aero` mock-up is the same window in a Windows 7 "glass and gloss" style
on a modern layout: glossy gradients with a hard step halfway, coloured glows
on hover, a hand-drawn canvas background, its own glass title bar (no Windows
frame; drag to move, corner grip to resize), a dialog that fades in, and
tooltips. Same start states (no `light`).

```
cd gui-iced && cargo run --release --bin aero
```


The `rice` mock-up blends a Linux-ricer desktop look with the soft gloss:
Hyprland-style tiles with gaps and a glowing gradient border on the focused
one, a waybar strip of pills (workspaces, scheme, receiving, speed, clock,
traffic-light window buttons), five colour schemes switchable live
(catppuccin, tokyo-night, gruvbox, nord, rose-pine), monospace type, a
shell-style history and a neofetch card. Keys: Ctrl+1..4 workspaces, Ctrl+T
next scheme. Start states also take a scheme name and `whoami`, `side`.

```
cd gui-iced && cargo run --release --bin rice
```
