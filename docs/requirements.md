# Beam — requirements

Beam is a terminal-only tool that sends a file directly from one computer to
another. There is no server to run: two devices meet once through an *invite*
one person pastes to the other, and find each other again by key through a
relay (ADR-0036). When a direct connection is impossible, the relay also
carries the encrypted connection (F-14, M5). File data never passes through
anything but the peer and, when needed, the relay.

Requirement IDs are stable. Tests reference them. A requirement that no longer
applies is marked **Withdrawn** with the decision that withdrew it, rather than
deleted or renumbered.

## 1. Functional requirements

| ID | Requirement | Milestone |
|----|-------------|-----------|
| F-1 | `beam init` generates an Ed25519 keypair for this device, once per machine. | M1 |
| F-2 | `beam whoami` shows this device's Short ID and fingerprint. The Short ID is no longer used to find a device; it remains as an identifier the pairing code is bound to (ADR-0036). | M1 |
| F-3 | `beam peers` lists paired peers with their fingerprints. | M1 |
| F-4 | `beam rename <old> <new>` changes a peer's local nickname. | M1 |
| F-5 | `beam remove <name>` forgets a peer, after confirmation. | M1 |
| F-6 | `beam listen` waits for incoming transfers and pairing requests on one iroh endpoint, and shows this device's invite and the current pairing code (ADR-0030, ADR-0036). `beam pair --wait` remains for pairing alone. | M2/M5 |
| F-7 | `beam send <peer> <file>` sends a file to a paired peer. | M2 |
| F-8 | `beam pair <INVITE> --name <name>` performs first-time pairing using an invite and a pairing code, exchanging and confirming public keys with `spake2`. The other device waits with `beam listen` or `beam pair --wait --name <name>`. (Originally a 9-digit Short ID looked up on the rendezvous server; changed by ADR-0036.) | M4 |
| F-13 | ~~A rendezvous server maps a Short ID to an iroh endpoint address.~~ — **Withdrawn (ADR-0036)**. There is no rendezvous server; see F-18. beam still never uses n0's DNS discovery. | M4 |
| F-14 | The relay URL is configurable; n0's relay is the development default and a self-hosted `iroh-relay` replaces it later. | M4 |
| F-9 | ~~`beam newcode`~~ — **dropped in M5.** `beam listen` renews its pairing code by itself after every attempt and every 10 minutes (ADR-0028 amendment). It no longer prints each new one; `beam whoami` shows the current code (F-21, ADR-0037). | M5 |
| F-10 | A transfer resumes after an interruption without re-sending verified chunks. Resuming is triggered only by running `beam send` again: there is no automatic reconnection and no `beam resume`. | M3 |
| F-12 | `beam transfers` lists partially received transfers; `beam transfers --clear` deletes them after confirmation. | M3 |
| F-11 | The progress line states whether the connection is `[Direct P2P]` or `[Relay]`, and says so when the path changes mid-transfer (ADR-0032). | M5 |
| F-15 | `beam send <peer>` dials the peer by its full public key from `known_peers` — through the relay its invite named (or this device's own) and at the direct addresses saved from that invite — and sends over iroh. Nothing is looked up. The M2 TCP path survives only as a hidden, test-only `--addr` flag (ADR-0031, ADR-0036). | M5 |
| F-16 | `listen` receives one transfer at a time. A second sender is told in words that the receiver is busy and to try later (ADR-0030). | M5 |
| F-17 | ~~`docs/deploy.md` explains running `beam-server` behind `wss://`.~~ — **Withdrawn (ADR-0036)**. There is no server to deploy; `deploy.md` now says so and covers pointing beam at another relay. | M5 |
| F-18 | `beam listen` and `beam pair --wait` show an **invite**: `beam1` + base32 of the device's public key, its relay and up to six direct addresses, with a checksum. A damaged or mistyped invite is refused with a clear message before any network traffic, and the message never quotes the pasted text (ADR-0036). | post-M6 |
| F-19 | After pairing, the joiner saves where the invite said its peer is (`addrs=`, and `relay=` when it differs from its own) on the peer's `known_peers` line. Running `beam pair <INVITE>` for a device that is already paired only updates those attributes — no code, no network, never the key or the name (ADR-0036). | post-M6 |
| F-21 | `beam listen` prints its invite and pairing code once, at start; a code renewed after ten minutes is not printed. `beam whoami` shows a running `listen`'s invite, current code and its expiry, or why there is no code (in use, paused, off), or that `listen` is not running. A crashed `listen` never leaves a stale code on show (ADR-0037). | post-M6 |
| F-22 | A joiner whose code did not match is told that the code may have expired, how often codes change, and how to get the current one (ADR-0037). | post-M6 |
| F-29 | `beam` typed with nothing after it opens a full-screen view on a terminal: paired peers, the selected peer's details and fingerprint, and whether the background agent runs. Any argument, or output that is not a terminal, gives the normal CLI (ADR-0043). | post-M6 |
| F-30 | `beam ui cli` makes plain `beam` print the help instead, and `beam ui tui` brings the view back; the choice is kept in `config.toml` (ADR-0043). | post-M6 |
| F-31 | In the full-screen view, `:` or Ctrl+P opens a command palette that runs any beam command: typed as on the command line or picked from a filtered list, with Tab completion of friends and files. Commands that only print show their output in a pop-up; commands that ask questions run in the normal terminal and return to the view (ADR-0043). | post-M6 |
| F-32 | The view's Add friend tab pairs without typing commands: paste their invite and a name, then type their code in a pop-up; or show this device's invite and code. Both people compare fingerprints in a pop-up that starts on No (ADR-0043). | post-M6 |
| F-33 | With the background agent running, the view lists waiting requests (Pending tab, header count, the friend's panel) and answers them in an Accept pop-up that shows sender, fingerprint, file, size and resume state, starts on Decline, and never opens by itself; it shows the agent's progress while a file arrives (ADR-0043). | post-M6 |
| F-34 | beam keeps a private history of transfers that reached a person (`~/.beam/history.jsonl`, newest 1000): `beam history` shows it newest first, `--clear` deletes it, and the view lists each friend's files and when they were last seen (ADR-0043). | post-M6 |
| F-35 | The view sends a file to the selected friend (`s`, or `:send` in the palette): a file browser of every drive with a filter and drag-and-drop (ADR-0045), then a pop-up showing each stage and progress that can be hidden or cancelled; cancelling tells the receiver, and leaving mid-send asks first (ADR-0043). | post-M6 |
| F-36 | A Receiving switch at the top of the view's Pending tab lets paired devices send while beam is open, without the background agent or a second terminal; it is off at every start, stops when beam closes, and asks before stopping a transfer in progress (ADR-0044). | post-M6 |
| F-37 | On a device with no identity, the view offers to create it ("Welcome to beam … Create it now?") with Create it / Not now; it never replaces an existing identity (ADR-0045). | post-M6 |
| F-38 | Sending from the view starts with a file browser: places and every drive, folders first with sizes and ages, a filter, Enter to open or send, Backspace to go up, and typed or dragged paths (ADR-0045). | post-M6 |
| F-26 | An optional background agent (`beam service enable|start`) receives from paired devices without `beam listen`: it shows a desktop notification, and the person answers in `beam inbox` with the same prompt. Requests wait up to 5 minutes; unanswered is declined (ADR-0042). | post-M6 |
| F-27 | `beam receive-dir` shows or sets where received files are saved. The default for the agent is the Windows Downloads folder (as Windows reports it, even if moved) or, on Linux, the folder the agent was started in. `beam listen` uses the setting when given no `--out` (ADR-0042). | post-M6 |
| F-28 | The agent starts at login per user (Windows `HKCU` Run entry, Linux `systemd --user`), never as a system service and never with administrator or root rights; `beam service status` reports it (ADR-0042). | post-M6 |
| F-25 | Ctrl+C in `beam send` or `beam listen` stops a transfer cleanly and the other device is told at once. Both terminals say which side stopped it (known from the QUIC close code, not from text) and that what arrived is kept for a resume. `listen` stops normally and removes `listen.json` (ADR-0041). | post-M6 |
| F-24 | The sender may have up to 4 chunks in flight before their answers; a rejected chunk is re-sent after the others. The transfer protocol version is agreed when connecting (`beam/xfer/2`, falling back to `beam/xfer/1`, one chunk at a time), so old and new versions of beam work together (ADR-0040). | post-M6 |
| F-23 | `advertise` in `config.toml` lists addresses to put first in this device's invite, for a public address beam cannot discover (a port forwarded by hand, with no relay). With `relay = "none"`, `listen` waits briefly for a router port mapping before showing the invite (ADR-0038). | post-M6 |
| F-20 | `beam listen` binds a fixed UDP port (`port` in `config.toml`, default 7820) so its invite stays the same between runs. If the port is taken it uses another and warns (ADR-0036). | post-M6 |

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
| S-7a | The sender's identity is *proven*, not merely claimed. **Met (M6).** Over iroh the receiver identifies the sender by the key the connection proved and refuses a request claiming any other (ADR-0031). Evidence: `tests/listen.rs::impersonation` — an unknown key, a known key without its secret key, a wrong address for a paired key (originally a lying rendezvous server; ADR-0036) — and the process-level re-init tests. The `STRENGTHEN IN M6:` markers are gone. The hidden TCP test transport still only claims keys; that is accepted risk R-5 in `threat-model.md`. | M5/M6 |
| S-8 | A changed peer key is a hard abort with an SSH-style warning. A stored key is never updated automatically; the user must re-pair. A changed key cannot be told apart from an absent one (the address *is* the key), so it shows as "not reachable" to senders and "unknown key" to receivers, both with a WARNING to check the new fingerprint in person before re-pairing; pairing again under the old name is refused with the same warning. **Met (M6)**; `threat-model.md` §4. | M6 |
| S-9 | Private keys never leave the device and are never sent to any server. | M1 |
| S-10 | Peer authentication is provided by the transport: iroh's QUIC/TLS proves possession of the Ed25519 private key behind an endpoint id before a connection exists, so a wrong address — from a tampered invite, a hand-edited `addrs=`, or formerly a rendezvous server — cannot mount a MITM. | M5 |
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
| S-19 | Pairing key confirmation is an HMAC under the SPAKE2 key over both public keys, the Short ID (derived on both sides from the waiter's key) and the speaker's role. Both keys are the ones the iroh connection proved; a key claimed in a message that differs from the proved one ends the pairing, and the key saved to `known_peers` is the connection's `remote_id()` (ADR-0026). | M4 |
| S-20 | ~~A rendezvous registration is signed by the device key with a timestamp …~~ — **Withdrawn (ADR-0036)**. There are no registrations. | M4 |
| S-21 | Pairing saves nothing until a person on **each** device has seen both fingerprints and answered yes. The same no-bypass rules as Accept apply: no flag or config answers it, no answer within 60 s is a no, and input typed before the question is discarded. | M4 |
| S-22 | ~~The rendezvous server keeps registrations in memory only and does not log requests.~~ — **Withdrawn (ADR-0036)**. There is no server. | M4 |
| S-31 | An invite is a routing hint, not a credential. An invite whose key was swapped cannot reach the real device and does not spend its code; a wrong address for a paired key cannot redirect a send (ADR-0036). | post-M6 |
| S-32 | Updating where a paired device is found (F-19) never changes its stored key; a device with a new key is a new pairing (rule 3, ADR-0036). | post-M6 |
| S-33 | A relay from an invite or a saved `relay=` is used only if it is `https://` on a public host; anything else is dropped. `config.toml` may name any relay (ADR-0038). | post-M6 |
| S-34 | An address update that would change a paired device's relay is saved only after the person answers yes to a question showing the old and new relay (ADR-0038). | post-M6 |
| S-36 | The sender checks every chunk it reads for sending against the hash it took of that chunk before the request. A chunk that still does not match after 3 reads, or a file that got shorter, stops the transfer with CANCEL before that chunk is sent (ADR-0040). | post-M6 |
| S-37 | The receiver accepts only chunks it is still missing: a chunk outside the transfer, or one that already arrived, ends the transfer. Each chunk gets at most 3 attempts, whatever order the chunks arrive in (ADR-0040). | post-M6 |
| S-42 | The command palette accepts only what the CLI's own parser accepts, has no way to accept a transfer, and runs interactive commands as a separate `beam` process with their unchanged prompts (ADR-0043). | post-M6 |
| S-43 | The history file is private, is never written for a request refused before the prompt (so a stranger cannot fill it), and its strings are cleaned before they are shown (ADR-0043). | post-M6 |
| S-38 | The agent does not pair: the pairing protocol is not offered (ADR-0042). | post-M6 |
| S-39 | The agent's local link answers only clients on loopback that present the token from the private `agent.json`; until then it reveals nothing and accepts no answer. One request takes one answer; any other is refused as too late (ADR-0042). | post-M6 |
| S-40 | Router port mapping in the agent is off unless turned on with `beam service port-mapping on`, which shows a warning and needs `y` (ADR-0042). | post-M6 |
| S-41 | Notification text from a peer is cleaned and passed to the OS notifier as data (environment variables), never as part of a command or script (ADR-0042). | post-M6 |
| S-35 | Dependencies are audited against the RustSec database on every push, CI's token is read-only, and CI actions are pinned to commit hashes; the latest manual audit is recorded in `SECURITY.md` (ADR-0038). | post-M6 |
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
| D-3 | `known_peers` holds one peer per line: `<name>  ed25519 <base64 key>  added=<RFC3339>`, optionally followed by more `key=value` attributes, which are preserved. beam writes `addrs=` and `relay=` (ADR-0036). | M1 |
| D-4 | A malformed `known_peers` line is a hard error naming the line number; entries are never silently skipped. | M1 |
| D-5 | Files under `~/.beam/` are written atomically (temp file + rename). | M1 |
| D-6 | An incoming file is assembled under `~/.beam/tmp/<transfer_id>/` and only moved into place after its full SHA-256 verifies. | M2 |
| D-7 | An existing destination file is never silently overwritten. | M2 |
| D-8 | Received chunks are tracked with a bitmap, not a single resume index. | M3 |
| D-9 | A transfer may only be resumed when the `file_sha256` and `size` in the new request match the stored transfer. The receiver persists the hashes of the chunks it has already verified, so resumed chunks are checked against the same values as the first attempt. A mismatch starts over as a new transfer. | M3 |
| D-10 | Chunk data and its hash are written **before** the bitmap records the chunk as present, so the record never names a chunk whose bytes were not handed to the operating system. Since ADR-0039 the flush to disk is batched (D-15), and the record is a claim that D-11 checks; until then each chunk was flushed before it was recorded. | M3 |
| D-11 | On resume, every chunk the bitmap claims is re-hashed against the persisted hash before it is offered to the sender; one that fails is simply re-requested. | M3 |
| D-12 | A partial is locked while a session uses it; a second session for the same partial is refused with a clear message. | M3 |
| D-13 | Committing a finished file works when the destination is on a different volume from `~/.beam/tmp`. | M3 |
| D-14 | A partial survives being declined, expiring, or losing its connection; it is discarded on success, on whole-file hash failure, or when it holds nothing. Partials expire after 7 days, swept when `beam listen` starts. | M3 |
| D-15 | Received chunks are recorded at once and flushed to disk in batches (every 8 chunks or 32 MiB), after the last chunk, and whenever a transfer stops early. A killed or crashed beam process loses nothing; power loss or an OS crash can lose at most the last batch, which D-11 detects and requests again (ADR-0039). | post-M6 |

## 4. Non-functional requirements

| ID | Requirement |
|----|-------------|
| N-1 | Terminal only. No GUI, no daemon the user did not start. |
| N-2 | Runs on Windows, macOS and Linux from a single Rust binary with no runtime dependencies. |
| N-3 | Default chunk size 4 MiB. |
| N-4 | `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` pass before any milestone is called done. |
| N-5 | Exit codes: 0 success, 1 error, 2 not implemented yet. |
| N-6 | Nicknames are local labels. There is no global username registry, and a peer is not notified when it is renamed. |
| N-8 | Throughput is measured with `tests/throughput.rs` (release build) before and after any change meant to affect it, and the numbers are recorded in `docs/performance-plan.md` (ADR-0039). |
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

SPIKE-001 is closed: the transport is iroh (ADR-0025). A first cross-network
test on 2026-10-02 — two devices on different home networks about 50 km apart,
no server, default configuration — paired with an invite and transferred a 5 GB
file intact. It was slow. Why it was slow, and how often hole punching falls
back to the relay (for example on Thai mobile CGNAT), is still to be measured;
throughput work will be planned before any change. It ran at about 1 MB/s
over the relay; the findings and the plan are in
[performance-plan.md](performance-plan.md).

## 7. Out of scope

Folder transfer, transfer history, bandwidth limiting and a `ratatui` TUI are
stretch goals, attempted only after M6. Multi-file transfers, a web client and
any kind of account system are out of scope entirely.
