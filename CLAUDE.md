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

- Language: Go (single module named `beam`; note the folder name contains a space).
- CLI: `cobra` (or stdlib `flag` if simpler). Optional TUI later with Bubble Tea.
- Identity keys: Ed25519 (stdlib `crypto/ed25519`).
- Pairing: an established PAKE library (e.g. `schollz/pake`, used by croc). Check that it
  is maintained before adopting; propose alternatives if not.
- Peer authentication after pairing: Noise Protocol **KK** handshake (`flynn/noise`),
  bound to the WebRTC DTLS fingerprints (channel binding) so the signaling server cannot
  perform MITM.
- P2P transport: WebRTC data channels via `pion/webrtc` (ICE/STUN; TURN relay fallback).
- Signaling server: Go + WebSocket. Stateless-ish; in-memory presence with heartbeats.
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
`Requested → AwaitingAccept → Connecting → Transferring → (Interrupted → Reconnecting) → Verifying → Completed`
with terminal states `Rejected`, `Expired`, `Failed`, `Cancelled`. Implement it as an
explicit, unit-tested state machine.

## Suggested layout

```
cmd/beam/            CLI entry point
cmd/beam-server/     signaling server
internal/identity/   keygen, fingerprint, short ID, known_peers
internal/pairing/    PAKE pairing flow
internal/auth/       Noise KK handshake + channel binding
internal/transport/  WebRTC connection, ICE, relay
internal/signaling/  client for the signaling server
internal/transfer/   protocol messages, state machine, chunking, resume, integrity
internal/ui/         prompts, progress bar
docs/                requirements, diagrams, test plan, design decisions
```

## Milestones (do them in order; stop for review after each)

- **M0** Project skeleton, Go module, CLI commands stubbed, Makefile, CI-ready `go test ./...`.
- **M1** Identity: `init`, `whoami`, `peers`, `rename`, `remove`, known_peers file handling.
- **M2** Transfer engine over plain TCP on localhost (no server, no crypto yet):
  request → Accept prompt → chunks → per-chunk hash → final hash → atomic commit.
- **M3** Resume with bitmap + interruption tests. Resume requires a new Accept.
- **M4** Signaling server + presence + `listen`/`pair` using PAKE.
- **M5** Replace TCP with WebRTC data channels (STUN), keep the same transfer engine.
- **M6** Noise KK mutual authentication bound to DTLS fingerprints; key-mismatch abort.
- **M7** TURN relay fallback; show `[Direct P2P]` or `[Relay]` in the progress line.
- Stretch (only if time allows): TUI, transfer history, bandwidth limit, folder transfer.

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
- Write tests alongside code. Run `go vet` and `go test ./...` before declaring done.
- Record design decisions in `docs/decisions.md` (short ADR-style entries); this project is
  graded on software engineering artifacts, not only working code.
