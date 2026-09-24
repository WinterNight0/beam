# Beam — Identity-Based P2P File Transfer (CLI)

University Software Engineering project. Beam is a terminal-only tool that sends files
directly between two computers (peer-to-peer). A small server only helps peers find each
other; file data must not be stored on or normally routed through the server.

## Core user experience

```
beam init                      # generate device keypair (once per machine)
beam listen                    # wait for transfers; shows short ID + pairing code
beam pair <ID> --name alice    # first-time pairing using ID + pairing code
beam send alice project.zip    # send a file to a paired peer
beam peers                     # list paired peers + fingerprints
beam rename alice ali
beam remove alice
beam whoami                    # show own ID + fingerprint
beam newcode                   # regenerate pairing code
```

## Non-negotiable rules

1. **Receiver must explicitly Accept every transfer.** There is NO auto-accept option,
   flag, config, or "trusted peer" bypass. Resuming an interrupted transfer also needs a
   new Accept.
   - Sender must not send any file bytes before receiving ACCEPT.
   - Receiver must discard and abort on any DATA received before it sent ACCEPT.
   - Unanswered requests expire (default 60 s) and count as Reject.
   - The Accept prompt shows: sender name, fingerprint, file name, size.
2. **Receiver only accepts requests from peers in its own `known_peers`.** Unknown senders
   are rejected without prompting.
3. **Key mismatch = hard abort** with an SSH-style warning ("key has changed, someone could
   be impersonating ..."). Never auto-update a stored key; the user must re-pair.
4. **Do not invent cryptography.** Use established libraries only. Ask before adding a new
   dependency and explain why.
5. Private keys never leave the device and are never sent to the server.

## Technology

- Language: **Rust** (edition 2024, MSRV 1.88). The project was specified in Go and
  M0/M1 were first built that way; it moved to Rust at the team's request. See
  ADR-0010 in `docs/decisions.md`. The Go implementation is preserved in commit
  `a1ae4ec`.
- Build: cargo workspace; `make check` = `cargo fmt --check` + `cargo clippy -D warnings`
  + `cargo test`. `unsafe_code = "forbid"` workspace-wide.
- CLI: `clap` (derive).
- Identity keys: Ed25519 (`ed25519-dalek`), PKCS#8 PEM via the `pkcs8` feature.
- Pairing: `spake2` (PAKE). Check that it is maintained before adopting; propose
  alternatives if not.
- P2P transport: **iroh, pinned to `=1.2.0`** (QUIC, hole punching, relay fallback).
  Chosen by SPIKE-001; see ADR-0025. The exact pin is deliberate: a silent minor
  bump changes network behaviour.
- Peer authentication: **provided by the transport.** iroh's endpoint identity *is*
  an Ed25519 keypair, so `~/.beam/id_ed25519` is the peer identity and a connection
  cannot exist without the peer proving possession of its private key. There is no
  Noise KK layer; see ADR-0025 for what replaced it.
- Async runtime: `tokio`, introduced at M2 with the transfer engine.
- Rendezvous server: Rust + WebSocket. Maps a Short ID to an iroh endpoint address.
  **beam does not use n0's discovery service**; see `docs/n0-data.md`.
- Relay: configurable. n0's relay during development, self-hosted `iroh-relay` later.
- Local storage: files under `~/.beam/` (plain text / JSON); SQLite only if needed later.

## Identity model

- `~/.beam/id_ed25519` (private, file mode 0600), `~/.beam/id_ed25519.pub`
- `~/.beam/known_peers` — one line per peer: `name  ed25519 <base64 pubkey>  added=<date>`
- **Fingerprint** = SHA-256 of the public key. The server routes by full fingerprint.
- **Short ID** (9 digits) is derived from the fingerprint and used ONLY for the first
  pairing lookup. It is a routing hint, not a security guarantee; security comes from PAKE
  during pairing and the full stored public key afterwards.
- Names like `alice` are local nicknames only. There is no global username registry.

## Transfer protocol (stateful)

Messages (JSON or length-prefixed binary, decide in design):
`TRANSFER_REQUEST {transfer_id, file_name, size, chunk_size, chunk_count, file_sha256, chunk_hashes}`
→ `ACCEPT {transfer_id, have_bitmap}` | `REJECT {transfer_id, reason}`
→ `CHUNK {transfer_id, index, data}` → `CHUNK_ACK {index}` → `COMPLETE` / `CANCEL`

- Default chunk size 4 MiB. Verify each chunk hash before writing; retry bad chunks.
- Track received chunks with a **bitmap** (not a single "resume from index").
- Write to a temp file under `~/.beam/tmp/<transfer_id>/`, verify full SHA-256, then
  atomically move into the destination. Never overwrite existing files silently.
- Transfer IDs are random; reject replayed/expired IDs.

Transfer state machine:
`Requested → AwaitingAccept → Connecting → Transferring → Verifying → Completed`
with terminal states `Rejected`, `Expired`, `Failed`, `Cancelled`. Implement it as an
explicit, unit-tested state machine.

There is deliberately **no** `Interrupted`/`Reconnecting` pair: a broken transfer ends
in `Failed`, and resuming is a *new* transfer that finds chunks already on disk and goes
through the whole machine from the top, prompt included. That is what makes "resuming
needs a new Accept" a property of the design rather than a rule to remember. Resume is
triggered only by running `beam send` again — no automatic reconnection, no
`beam resume`. See ADR-0020.

## Suggested layout

```
crates/beam/
  src/main.rs          CLI entry point
  src/cli/             command definitions (kept out of main so they are testable)
  src/identity/        keygen, fingerprint, short ID, known_peers
  src/pairing/         PAKE pairing flow
  src/transport/       iroh endpoint, connection, relay configuration
  src/rendezvous/      client for the rendezvous server
  src/transfer/        protocol messages, state machine, chunking, resume, integrity
  src/ui.rs            prompts, progress bar
  tests/               command-level and integration tests
crates/beam-server/    rendezvous server
docs/                  requirements, diagrams, test plan, design decisions
```

New areas start as modules of the `beam` library crate and are promoted to their
own workspace crates only if compile times demand it.

## Milestones (do them in order; stop for review after each)

- **M0** Project skeleton, cargo workspace, CLI commands stubbed, Makefile, CI-ready `cargo test`.
- **M1** Identity: `init`, `whoami`, `peers`, `rename`, `remove`, known_peers file handling.
- **M2** Transfer engine over plain TCP on localhost (no server, no crypto yet):
  request → Accept prompt → chunks → per-chunk hash → final hash → atomic commit.
- **M3** Resume with bitmap + interruption tests. Resume requires a new Accept. `beam transfers` lists and clears partials.
- **M4** Rendezvous server mapping Short ID → iroh endpoint address. `listen` shows the
  Short ID and a pairing code; `beam pair` uses `spake2` to exchange and confirm public
  keys. Relay URL is configurable (n0's relay for development). **Do not use n0's DNS
  discovery** — the address comes from our own server. See `docs/n0-data.md`.
- **M5** Use the iroh transport in real `send`/`listen`, keeping the same transfer
  engine. Show `[Direct P2P]` or `[Relay]` in the progress line. Absorbs the old M7.
- **M6** Replaces Noise KK. Write `docs/threat-model.md`. Add security tests proving
  impersonation fails at the transport level: an unknown key, a known key without its
  secret key, a rendezvous server returning a wrong address, and a peer that re-ran
  `beam init` (must fail with a clear "re-pair" message). Remove every
  `STRENGTHEN IN M6:` marker and make S-7a pass.
- Stretch (only if time allows): TUI (`ratatui`), transfer history, bandwidth limit, folder transfer.

## Testing expectations

- Unit tests: chunk splitting, hashing, bitmap, state machine transitions, known_peers
  parsing, fingerprint/short-ID derivation.
- Integration: client↔client on localhost, client↔server, client↔relay.
- Failure tests: connection drop mid-transfer, corrupted chunk, duplicate chunk, peer goes
  offline, server down, disk full, cancel, crash then resume.
- Security tests: data before Accept, request from unknown peer, changed key, replayed
  transfer ID, expired request, tampered chunk.

## Working style

- Plan before coding each milestone; keep changes small and reviewable.
- Write tests alongside code. Run `make check` (fmt, clippy, tests) before declaring done.
- Record design decisions in `docs/decisions.md` (short ADR-style entries); this project is
  graded on software engineering artifacts, not only working code.
