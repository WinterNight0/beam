# Beam — Identity-Based P2P File Transfer (CLI)

University Software Engineering project. Beam is a terminal-only tool that sends files
directly between two computers (peer-to-peer). File data must not be stored on or normally
routed through any server.

> **No rendezvous server (ADR-0036, merged after M6).** Peers meet through an *invite*
> (key + relay + direct addresses, pasted once) and are found again by key through the
> relay. The `rendezvous` module and `beam-server` crate are gone. The milestone list
> below is history: where it mentions the server, ADR-0036 is what holds now. How the
> whole system works is explained in `docs/how-it-works.md`.

## Core user experience

```
beam                           # full-screen view (ADR-0043); `beam ui cli` turns it off
beam init                      # generate device keypair (once per machine)
beam listen                    # wait for transfers; shows invite + pairing code
beam pair <INVITE> --name alice  # first-time pairing using invite + pairing code
beam pair --wait --name bob    # pair without also receiving files
beam send alice project.zip    # send a file to a paired peer
beam peers                     # list paired peers + fingerprints
beam rename alice ali
beam remove alice
beam whoami                    # show own ID + fingerprint
beam service enable            # optional background agent (ADR-0042)
beam inbox                     # accept/decline what the agent holds
beam receive-dir <folder>      # where received files go
beam history                   # what came and went (ADR-0043)
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

- Language: **Rust** (edition 2024, MSRV 1.91). The project was specified in Go and
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
- Rendezvous server: **none** (ADR-0036). The invite carries the
  address for the first meeting; afterwards a peer is dialled by key through its relay
  and at the addresses saved in `known_peers` (`relay=`, `addrs=`). `listen` binds a
  fixed UDP port (`port` in `config.toml`, default 7820) so its invite stays stable.
  **beam does not use n0's discovery service**; see `docs/n0-data.md`.
- Relay: configurable. n0's relay during development, self-hosted `iroh-relay` later.
- Local storage: files under `~/.beam/` (plain text / JSON); SQLite only if needed later.

## Identity model

- `~/.beam/id_ed25519` (private, file mode 0600), `~/.beam/id_ed25519.pub`
- `~/.beam/known_peers` — one line per peer: `name  ed25519 <base64 pubkey>  added=<date>`
- **Fingerprint** = SHA-256 of the public key. The server routes by full fingerprint.
- **Invite** (`beam1…`, base32) carries the key, relay and direct addresses for the
  first pairing. Like the Short ID it replaced, it is a routing hint, not a security
  guarantee; security comes from PAKE during pairing and the full stored public key
  afterwards. The 9-digit **Short ID** is still derived from the fingerprint and bound
  into the SPAKE2 transcript.
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
  src/invite.rs        invites, and where a paired peer is found
  src/transfer/        protocol messages, state machine, chunking, resume, integrity
  src/ui.rs            prompts, progress bar
  tests/               command-level and integration tests
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
  In M4 the receiver waits with `beam pair --wait`; M5 merges it into `listen`
  (ADR-0028). Pairing codes are single use; registrations are signed.
  ADR-0026..0029. *(The server was later removed; see ADR-0036 below.)*
- **M5** Use the iroh transport in real `send`/`listen`, keeping the same transfer
  engine. Show `[Direct P2P]` or `[Relay]` in the progress line. Absorbs the old M7.
  `listen` serves pairing and transfers on one endpoint, one question at a time;
  it renews its own pairing code, so `beam newcode` was dropped. ADR-0028..0032,
  `docs/deploy.md`.
- **M6** Replaces Noise KK. Write `docs/threat-model.md`. Add security tests proving
  impersonation fails at the transport level: an unknown key, a known key without its
  secret key, a rendezvous server returning a wrong address, and a peer that re-ran
  `beam init` (must fail with a clear "re-pair" message). Remove every
  `STRENGTHEN IN M6:` marker and make S-7a pass. Done: also terminal-injection
  defence, limits for a misbehaving paired peer, and redraw-after-notice
  (ADR-0033..0035).
- **Post-M6 (ADR-0036)** No rendezvous server: `listen` shows an invite, `beam pair
  <INVITE>` pairs, peers are dialled by key through the relay and saved addresses,
  `listen` binds a fixed port (7820). Merged 2026-10-02 after a cross-network test.
- **Post-M6 (ADR-0037)** `listen` prints its code once; `beam whoami` shows the live
  invite and code from `~/.beam/listen.json` (trusted only while `listen.lock` is held);
  a joiner with a wrong code is told it may have expired.
- **Post-M6 (ADR-0038)** Security review: relay changes via invite need a yes; invite
  relays must be `https://` on public hosts; CI read-only, actions pinned, `cargo audit`;
  `advertise` config for direct testing. Open: R-8 (scannable `listen`), to be planned.
  `SECURITY.md` holds the audit results and must be updated with each new check.
- **Post-M6 (ADR-0039)** Throughput steps 1-4: `tests/throughput.rs` benchmark (ignored;
  run in release, `BEAM_BENCH_RTT_MS` adds delay), QUIC stream window 16 MiB and
  connection window 32 MiB; each chunk recorded at once, disk flushed in batches (8 chunks
  / 32 MiB, and when a transfer stops); `reverify` on resume is what makes an unflushed
  record safe. Measure any speed change with the benchmark.
- **Post-M6 (ADR-0040)** Throughput step 5: up to 4 chunks in flight on ALPN
  `beam/xfer/2` (sender offers `/2` and `/1`; `listen` prefers `/2`; `/1` = one at a time).
  A NAK'd chunk is re-sent after the others; the receiver takes any *missing* chunk in any
  order and refuses duplicates or unknown indices. The sender checks each chunk it reads
  against the hash from its first pass and CANCELs on mismatch (`SourceChanged`).
  The whole-file hash before commit remains the integrity anchor.
- **Post-M6 (ADR-0041)** Ctrl+C in `send`/`listen` closes the connection with QUIC
  application code 2 (`CLOSE_INTERRUPTED`); the peer reports "<name> stopped beam on their
  side". `listener::run_until` stops `listen` cleanly (tells senders, keeps partials, one
  `Stopped` event, removes `listen.json`).
- **Post-M6 (ADR-0042)** Background agent (`src/agent/`): `beam agent` runs
  `listener::run_until` with pairing off, port mapping per `agent_port_mapping`
  (default off; `beam service port-mapping on` warns + y/N), 5 min Accept window;
  its Prompt hands requests to `beam inbox` over loopback TCP gated by the token
  in private `agent.json` and shows an OS notification (PowerShell toast /
  notify-send / osascript, text via env vars). Per-user only: HKCU Run entry or
  `systemd --user`; never a system service. `beam receive-dir` sets `receive_dir`
  (Windows default: real Downloads folder; Linux: start folder). `listen` and the
  agent exclude each other. See `docs/background-services.md`.
- **Post-M6 (ADR-0043)** Full-screen view (`src/tui/`, ratatui 0.30 +
  crossterm via its re-export): plain `beam` on a terminal opens it unless
  `ui = "cli"` (`beam ui cli|tui`); any argument means the normal CLI
  (`cli::start` vs `cli::execute`). Discord Friends layout. `tui::app` is pure
  state (unit tested), `tui::view` draws (TestBackend tests) and records click
  areas. Ctrl+C copies, Ctrl+Q quits (as in Fresh). Steps: 1 view + toggle,
  2 mouse/text boxes/rename/remove, 3 command palette (`:`/Ctrl+P, checked by
  `cli::check`; `palette::place` = here / terminal (own process) / pop-up),
  4 add friend (`tui::pairing` runs `pairing::join`/`wait` on a thread; code
  pop-up → fingerprint check starting on No; port falls back so the agent never
  pairs), 5 pending (`tui::inbox` = the `beam inbox` link; requests never pop
  up by themselves; Accept pop-up starts on Decline), 6 history (`src/history.rs`,
  private `history.jsonl`, written by `send` and the listener; `beam history`;
  last seen = newest answered line), 7 send (`tui::sending` = `beam send` on a
  thread, shared messages/history; one at a time; hide/cancel; quit asks),
  8 docs (`docs/tui.md`) (done). No online dots.
- **Post-M6 (ADR-0044)** Receiving switch at the top of the view's Pending tab
  (`o` or click; amber OFF / green ON): runs `agent::run` inside the view
  (`tui::receiving`, `in_view` in `agent.json`): pairing off, 5 min, agent port
  mapping, notifications; agent lock so listen/agent refuse; off at every start,
  stops with the view; asks before stopping a transfer.
- **Post-M6 (ADR-0045)** First-run welcome card in the view (no identity →
  "Create it now?", Create it = `beam init`, never replaces); `s` opens a file
  browser (`tui::browse`: places + every drive, natural sort, filter, typed or
  dragged paths, `Fs` injectable for tests).
- **Next (to be planned):** the path itself — why cross-network transfers stay on the relay
  (performance plan step 0). n0's public relay stays rate-limited.
- Stretch (only if time allows): TUI (`ratatui`), transfer history, bandwidth limit, folder transfer.

## Testing expectations

- Unit tests: chunk splitting, hashing, bitmap, state machine transitions, known_peers
  parsing, fingerprint/short-ID derivation.
- Integration: client↔client on localhost, client↔relay.
- Failure tests: connection drop mid-transfer, corrupted chunk, duplicate chunk, peer goes
  offline, server down, disk full, cancel, crash then resume.
- Security tests: data before Accept, request from unknown peer, changed key, replayed
  transfer ID, expired request, tampered chunk.

## Working style

- Plan before coding each milestone; keep changes small and reviewable.
- Write tests alongside code. Run `make check` (fmt, clippy, tests) before declaring done.
- Record design decisions in `docs/decisions.md` (short ADR-style entries); this project is
  graded on software engineering artifacts, not only working code.
