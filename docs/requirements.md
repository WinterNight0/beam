# Beam — requirements

Beam is a terminal-only tool that sends a file directly from one computer to
another. A signaling server helps two peers find each other; file data is never
stored on the server and is only routed through it when a direct connection is
impossible (TURN relay, M7).

Requirement IDs are stable. Tests reference them.

## 1. Functional requirements

| ID | Requirement | Milestone |
|----|-------------|-----------|
| F-1 | `beam init` generates an Ed25519 keypair for this device, once per machine. | M1 |
| F-2 | `beam whoami` shows this device's Short ID and fingerprint. | M1 |
| F-3 | `beam peers` lists paired peers with their fingerprints. | M1 |
| F-4 | `beam rename <old> <new>` changes a peer's local nickname. | M1 |
| F-5 | `beam remove <name>` forgets a peer, after confirmation. | M1 |
| F-6 | `beam listen` waits for incoming transfers and shows the Short ID and pairing code. | M2/M4 |
| F-7 | `beam send <peer> <file>` sends a file to a paired peer. | M2 |
| F-8 | `beam pair <ID> --name <name>` performs first-time pairing using a Short ID and a pairing code. | M4 |
| F-9 | `beam newcode` regenerates this device's pairing code. | M4 |
| F-10 | A transfer resumes after an interruption without re-sending verified chunks. Resuming is triggered only by running `beam send` again: there is no automatic reconnection and no `beam resume`. | M3 |
| F-12 | `beam transfers` lists partially received transfers; `beam transfers --clear` deletes them after confirmation. | M3 |
| F-11 | The progress line states whether the connection is `[Direct P2P]` or `[Relay]`. | M7 |

## 2. Security requirements

These are non-negotiable. Where a requirement conflicts with convenience,
convenience loses.

| ID | Requirement | Milestone |
|----|-------------|-----------|
| S-1 | The receiver explicitly accepts **every** transfer. There is no auto-accept flag, config setting or trusted-peer bypass. | M2 |
| S-2 | Resuming an interrupted transfer requires a new Accept. A resume is a new transfer that finds data on disk, so it passes through the prompt like any other; there is no state the machine can re-enter without it (ADR-0020). The prompt says it is a resume and how much is already held. | M3 |
| S-3 | The sender sends no file bytes before it has received ACCEPT. | M2 |
| S-4 | The receiver discards the transfer and aborts if any DATA arrives before it sent ACCEPT. | M2 |
| S-5 | An unanswered request expires (default 60 s) and counts as a Reject. | M2 |
| S-6 | The Accept prompt shows sender name, sender fingerprint, file name and size. | M2 |
| S-7 | The receiver only accepts requests from peers present in its own `known_peers`; unknown senders are rejected without prompting. | M2 |
| S-7a | The sender's identity is *proven*, not merely claimed. In M2 the receiver checks the public key a sender presents against `known_peers`, but nothing proves the sender holds the matching private key; see ADR-0019. Tests that turn on this gap are marked `STRENGTHEN IN M6:`. | M6 |
| S-8 | A changed peer key is a hard abort with an SSH-style warning. A stored key is never updated automatically; the user must re-pair. | M6 |
| S-9 | Private keys never leave the device and are never sent to the signaling server. | M1 |
| S-10 | Peer authentication uses a Noise KK handshake (`snow`) bound to the WebRTC DTLS fingerprints, so the signaling server cannot mount a MITM. | M6 |
| S-11 | Transfer IDs are random; replayed or expired IDs are rejected. | M2/M3 |
| S-14 | A partial transfer is matched by (sender fingerprint, file_sha256, size, chunk_size) and never by a sender-supplied transfer ID, so no peer can attach to another peer's partial. | M3 |
| S-15 | A have-bitmap from a peer is validated — exact length, no bits past the end — and a bad one aborts the transfer rather than being repaired. | M3 |
| S-12 | Every chunk is verified against its hash before it is written, and the whole file against its SHA-256 before it is committed. | M2 |
| S-13 | No cryptography is invented: only established, maintained libraries are used. | all |

## 3. Data and integrity requirements

| ID | Requirement | Milestone |
|----|-------------|-----------|
| D-1 | Identity and peer state live under `~/.beam/` as plain text. | M1 |
| D-2 | The private key file is created with mode 0600. See ADR-0004 for the Windows limitation. | M1 |
| D-3 | `known_peers` holds one peer per line: `<name>  ed25519 <base64 key>  added=<RFC3339>`. | M1 |
| D-4 | A malformed `known_peers` line is a hard error naming the line number; entries are never silently skipped. | M1 |
| D-5 | Files under `~/.beam/` are written atomically (temp file + rename). | M1 |
| D-6 | An incoming file is assembled under `~/.beam/tmp/<transfer_id>/` and only moved into place after its full SHA-256 verifies. | M2 |
| D-7 | An existing destination file is never silently overwritten. | M2 |
| D-8 | Received chunks are tracked with a bitmap, not a single resume index. | M3 |
| D-9 | A transfer may only be resumed when the `file_sha256` and `size` in the new request match the stored transfer. The receiver persists the hashes of the chunks it has already verified, so resumed chunks are checked against the same values as the first attempt. A mismatch starts over as a new transfer. | M3 |
| D-10 | Chunk data and its hash are written and flushed to disk **before** the bitmap records the chunk as present, so a crash loses the claim rather than the data. | M3 |
| D-11 | On resume, every chunk the bitmap claims is re-hashed against the persisted hash before it is offered to the sender; one that fails is simply re-requested. | M3 |
| D-12 | A partial is locked while a session uses it; a second session for the same partial is refused with a clear message. | M3 |
| D-13 | Committing a finished file works when the destination is on a different volume from `~/.beam/tmp`. | M3 |
| D-14 | A partial survives being declined, expiring, or losing its connection; it is discarded on success, on whole-file hash failure, or when it holds nothing. Partials expire after 7 days, swept when `beam listen` starts. | M3 |

## 4. Non-functional requirements

| ID | Requirement |
|----|-------------|
| N-1 | Terminal only. No GUI, no daemon the user did not start. |
| N-2 | Runs on Windows, macOS and Linux from a single Rust binary with no runtime dependencies. |
| N-3 | Default chunk size 4 MiB. |
| N-4 | `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` pass before any milestone is called done. |
| N-5 | Exit codes: 0 success, 1 error, 2 not implemented yet. |
| N-6 | Nicknames are local labels. There is no global username registry, and a peer is not notified when it is renamed. |
| N-7 | Free space is checked before the Accept prompt, on the volume holding partials and — when it differs — the destination volume, counting only the bytes still missing plus a small margin. Too little space is refused clearly rather than discovered part-way through. |

## 5. Implementation language

The project was specified in Go and M0/M1 were first built that way; it moved to
Rust at the team's request. See ADR-0010 in `decisions.md` for the switch and
the library replacements it forces (`webrtc-rs`, `snow`, `spake2`, `tokio`).
Nothing in sections 1-4 changed as a result: the file formats, the identity
derivations and every security rule are unchanged, and the same fixed test
vectors pass in both implementations.

## 6. Open investigations

Before M4 the team runs SPIKE-001, a timeboxed comparison of `webrtc-rs`,
`str0m` and `iroh` as the P2P transport, and records the choice as an ADR. The
result can change how much of M4 there is to build, which is why it happens
before that milestone rather than before M5. See `spikes.md`.

## 7. Out of scope

Folder transfer, transfer history, bandwidth limiting and a `ratatui` TUI are
stretch goals, attempted only after M7. Multi-file transfers, a web client and
any kind of account system are out of scope entirely.
