# Beam — requirements

> **Branch `main-test`:** there is no rendezvous server on this branch;
> devices meet through invites and are found again by key through the relay
> (ADR-0036 in `decisions.md`). Requirements that mention the server, its registrations or the Short ID lookup (S-18 to S-22) apply to `main`; on this branch the invite takes their place, and the security properties they protect are covered by the tests named in ADR-0036 and the revised `threat-model.md`.

Beam is a terminal-only tool that sends a file directly from one computer to
another. A rendezvous server helps two devices find each other; file data never
passes through it. When a direct connection is impossible, the encrypted
connection is carried by a relay (F-14, M5).

Requirement IDs are stable. Tests reference them.

## 1. Functional requirements

| ID | Requirement | Milestone |
|----|-------------|-----------|
| F-1 | `beam init` generates an Ed25519 keypair for this device, once per machine. | M1 |
| F-2 | `beam whoami` shows this device's Short ID and fingerprint. | M1 |
| F-3 | `beam peers` lists paired peers with their fingerprints. | M1 |
| F-4 | `beam rename <old> <new>` changes a peer's local nickname. | M1 |
| F-5 | `beam remove <name>` forgets a peer, after confirmation. | M1 |
| F-6 | `beam listen` waits for incoming transfers and pairing requests on one iroh endpoint, and shows the Short ID and the current pairing code (ADR-0030). `beam pair --wait` remains for pairing alone. | M2/M5 |
| F-7 | `beam send <peer> <file>` sends a file to a paired peer. | M2 |
| F-8 | `beam pair <ID> --name <name>` performs first-time pairing using a Short ID and a pairing code, exchanging and confirming public keys with `spake2`. The other device waits with `beam pair --wait --name <name>`. | M4 |
| F-13 | A rendezvous server maps a Short ID to an iroh endpoint address. beam never uses n0's DNS discovery. | M4 |
| F-14 | The relay URL is configurable; n0's relay is the development default and a self-hosted `iroh-relay` replaces it later. | M4 |
| F-9 | ~~`beam newcode`~~ — **dropped in M5.** `beam listen` renews its pairing code by itself after every attempt and every 10 minutes, and prints the new one (ADR-0028 amendment). | M5 |
| F-10 | A transfer resumes after an interruption without re-sending verified chunks. Resuming is triggered only by running `beam send` again: there is no automatic reconnection and no `beam resume`. | M3 |
| F-12 | `beam transfers` lists partially received transfers; `beam transfers --clear` deletes them after confirmation. | M3 |
| F-11 | The progress line states whether the connection is `[Direct P2P]` or `[Relay]`, and says so when the path changes mid-transfer (ADR-0032). | M5 |
| F-15 | `beam send <peer>` finds the peer through the rendezvous server by its full public key from `known_peers`, and sends over iroh. The M2 TCP path survives only as a hidden, test-only `--addr` flag (ADR-0031). | M5 |
| F-16 | `listen` receives one transfer at a time. A second sender is told in words that the receiver is busy and to try later (ADR-0030). | M5 |
| F-17 | `docs/deploy.md` explains running `beam-server` behind `wss://` on a VPS with a reverse proxy or through Cloudflare Tunnel, and pointing clients at it. | M5 |

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
| S-7a | The sender's identity is *proven*, not merely claimed. **Met (M6).** Over iroh the receiver identifies the sender by the key the connection proved and refuses a request claiming any other (ADR-0031). Evidence: `tests/listen.rs::impersonation` — an unknown key, a known key without its secret key, a rendezvous server returning a wrong address or another key — and the process-level re-init tests. The `STRENGTHEN IN M6:` markers are gone. The hidden TCP test transport still only claims keys; that is accepted risk R-5 in `threat-model.md`. | M5/M6 |
| S-8 | A changed peer key is a hard abort with an SSH-style warning. A stored key is never updated automatically; the user must re-pair. A changed key cannot be told apart from an absent one (the address *is* the key), so it shows as "not reachable" to senders and "unknown key" to receivers, both with a WARNING to check the new fingerprint in person before re-pairing; pairing again under the old name is refused with the same warning. **Met (M6)**; `threat-model.md` §4. | M6 |
| S-9 | Private keys never leave the device and are never sent to the rendezvous server. | M1 |
| S-10 | Peer authentication is provided by the transport: iroh's QUIC/TLS proves possession of the Ed25519 private key behind an endpoint id before a connection exists, so a rendezvous server returning a wrong address cannot mount a MITM. | M5 |
| S-16 | A written threat model (`docs/threat-model.md`) states what an attacker can and cannot do, backed by tests that demonstrate each claim. | M6 |
| S-17 | beam publishes nothing to third-party infrastructure by default beyond relayed (encrypted) traffic; what would otherwise be published, and how to disable it, is documented in `docs/n0-data.md`. | M4 |
| S-18 | A pairing code is six digits, **single use** and expires after 10 minutes. The first attempt that uses it spends it, whatever the outcome; a wrong guess never leaves the code usable (ADR-0026). | M4 |
| S-23 | Guessing is bounded when `listen` renews its own code. An attempt that did not prove the code pauses pairing (5 s, doubling, capped at 5 min); **three in a row turn pairing off** for the rest of the `listen` session, with a clear message, while transfers from paired peers keep working. Restarting `listen` turns it back on. An attempt that proved the code resets the count; one that arrives during a pause is refused without counting (ADR-0028 amendment). | M5 |
| S-24 | Only one question is on screen at a time. Questions queue; each keeps its own deadline counted from when it was asked, and one that expires in the queue is refused unseen and reported. Input typed before a question appears cannot answer it (ADR-0030). | M5 |
| S-25 | Over iroh, the connection's `remote_id()` must equal the key in `known_peers`: `send` checks the key it dialled, and `listen` looks the sender up by the proved key and refuses a request claiming another. A device that re-ran `beam init` is not followed to its new key; the user is told to re-pair (ADR-0031). | M5 |
| S-26 | The pairing question looks unlike the Accept prompt — its own banner and wording — and only `yes` typed in full confirms it; `y` does not. Pairing is permanent (ADR-0030). | M5 |
| S-27 | A paired peer cannot exhaust the receiver: chunk size ≤ 16 MiB, chunk count ≤ 2²², frames ≤ 64 KiB checked from the header, free space checked before any state is written, and a 60 s stall timeout on both sides once accepted — with keep-alives during the final verification so a large file is not mistaken for a stall (ADR-0033). | M6 |
| S-28 | Text from the other side is never printed raw: control characters and whole ANSI sequences are removed, bidi and zero-width characters shown as `<U+XXXX>`, and length capped, names cut in the middle keeping the extension. File names containing bidi overrides or isolates are refused; zero-width characters are allowed (Thai, emoji) and shown (ADR-0034). | M6 |
| S-29 | A notice printed while a question is open is followed by the question again, with its remaining time (ADR-0035). | M6 |
| S-30 | `docs/threat-model.md` names every asset, actor and threat, maps each mitigation to its ADR and a test, and lists the accepted risks. | M6 |
| S-19 | Pairing key confirmation is an HMAC under the SPAKE2 key over both public keys, the Short ID and the speaker's role. Both keys are the ones the iroh connection proved; a key claimed in a message that differs from the proved one ends the pairing, and the key saved to `known_peers` is the connection's `remote_id()` (ADR-0026). | M4 |
| S-20 | A rendezvous registration is signed by the device key with a timestamp. The server rejects a bad signature, a timestamp more than 60 s from its clock or not newer than the last for that key, and a key that does not derive the claimed Short ID. Clients re-check every lookup answer (ADR-0027). | M4 |
| S-21 | Pairing saves nothing until a person on **each** device has seen both fingerprints and answered yes. The same no-bypass rules as Accept apply: no flag or config answers it, no answer within 60 s is a no, and input typed before the question is discarded. | M4 |
| S-22 | The rendezvous server keeps registrations in memory only and does not log requests. | M4 |
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
Rust at the team's request. See ADR-0010 in `decisions.md`. The transport was
then chosen by SPIKE-001: **iroh**, not WebRTC, which removes the need for a
Noise KK layer — see ADR-0025.
Nothing in sections 1-4 changed as a result: the file formats, the identity
derivations and every security rule are unchanged, and the same fixed test
vectors pass in both implementations.

## 6. Open investigations

SPIKE-001 is closed: the transport is iroh (ADR-0025). What remains unproven is
the cross-network behaviour — whether hole punching gets through Thai mobile
CGNAT, how often it falls back to a relay, and how slow that path is. The steps
are in `spikes/transport.md`, and the result would only reopen the decision if
connections fail outright rather than merely relay.

## 7. Out of scope

Folder transfer, transfer history, bandwidth limiting and a `ratatui` TUI are
stretch goals, attempted only after M6. Multi-file transfers, a web client and
any kind of account system are out of scope entirely.
