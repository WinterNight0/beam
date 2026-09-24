# Spikes

Throwaway code that answers a question. Nothing here is part of the product.

Each crate declares its own empty `[workspace]`, so it is detached from the
main workspace: `cargo build` at the repository root does not build any of it,
and a spike cannot change what ships.

| Directory | Question | Findings |
|---|---|---|
| `iroh-transport/` | Can beam's transfer engine run over iroh unchanged, and can beam's Ed25519 key be the peer identity? | [`docs/spikes/transport.md`](../docs/spikes/transport.md) |
| `measure/` | What do the three candidate transports cost in dependencies and build time? | same |

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
