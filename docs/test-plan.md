# Beam — test plan

## How to run

```
make check                                  # cargo fmt --check + clippy -D warnings + cargo test
powershell -File scripts\check.ps1          # the same, on Windows without make
cargo test known_peers                      # one area
cargo test --lib                            # unit tests only
cargo test --test cli                       # command-level tests only
cargo test --test resume                    # resume and retention
cargo test --test end_to_end                # two real processes
```

`make check` is the gate: a milestone is not done until it passes. CI runs the
same three commands on `ubuntu-latest` and `windows-latest` for every push and
pull request (ADR-0014), so the gate is enforced rather than remembered.

## Test levels

| Level | Where | What it covers |
|-------|-------|----------------|
| Unit | `#[cfg(test)]` modules inside `crates/beam/src/` | derivations, parsing, framing, chunk arithmetic, state machine transitions, path safety |
| Command | `crates/beam/tests/cli.rs` | whole commands driven through `cli::execute` with in-memory streams and a temporary `--beam-dir` |
| Integration | `crates/beam/tests/transfer.rs` | both halves of the engine over an in-memory pipe, plus one pass over a real socket |
| Failure | `crates/beam/tests/transfer.rs` | corruption, refusal, expiry, malformed requests; interruption and crash-and-resume arrive with M3 |
| Security | `tests/transfer.rs::accept_rules`, `tests/cli.rs` | the S-* requirements in `requirements.md`, each with a test that tries to break it |
| Resume | `crates/beam/tests/resume.rs` | partial transfers: what carries over, what is re-fetched, and what survives a failure |
| End to end | `crates/beam/tests/end_to_end.rs` | two real `beam` processes, talking over a real socket, with the prompt answered only once it has actually appeared — including killing one of them mid-transfer |
| Manual | `docs/test-plan.md` | the few things one machine cannot honestly automate: a real second volume, a real full disk |

Command-level tests never spawn a process and never touch the real `~/.beam`:
every test gets a `TempDir` passed via the `--beam-dir` flag. Lints are part of
the gate, not advisory — `clippy` runs with `-D warnings` and the workspace sets
`unsafe_code = "forbid"`.

## Implemented (M0–M1)

### Unit tests — `src/identity/`

| Area | Cases |
|------|-------|
| Fingerprint / Short ID (D-3, ADR-0006) | fixed vectors for two deterministic keys, **carried over unchanged from the Go implementation**; Short ID always 9 digits including the all-zero and all-ones extremes; digit grouping; fingerprint parsing accepts bare hex, `SHA256:` prefix (either case), uppercase and colon-separated forms, and rejects wrong length, bad hex and empty input; short-ID parsing accepts spaces and dashes and rejects wrong digit counts and non-digits |
| Keypair (F-1, S-9) | a generated key's `verifying_key()` matches its signing key; two generations differ; PKCS#8 PEM round-trip; garbage private keys rejected (empty, not PEM, wrong PEM label, bad DER); public-key line round-trip; a comment containing newlines or tabs cannot break the one-line format; bad public key lines rejected (wrong type, bad base64, short key, missing key, comment only) |
| known_peers parsing (D-3, D-4) | space- and tab-separated fields; comments and blank lines; unknown `key=value` attributes preserved; rejects too few fields, wrong key type, bad base64, short key, invalid name, bare token, invalid attribute key, unparseable `added=`, duplicate name, duplicate name differing only in case, and the same public key under two names — every case carrying a line number |
| known_peers editing (F-4, F-5) | `add` stamps `added=` and rejects duplicate names, duplicate keys and invalid names; `lookup` is case-insensitive; `lookup_key` matches by key; `rename` keeps the key, the attributes, the comments and the file order, allows a pure capitalisation change, and rejects collisions and unknown peers; `remove` is case-insensitive and keeps comments; rendering is idempotent |
| Name rules (D-3) | accepts `a`, `Alice-2`, `my.laptop`, `under_score`, 32 characters; rejects empty, spaces, emoji, `/`, `#`, 33 characters |
| Store (D-1, D-2, D-5) | save/load round-trip; `$BEAM_DIR` override, including an empty override treated as unset; `init` creates `known_peers` and `tmp/` but does not clobber an existing `known_peers`; refuses to overwrite an identity without `--force` and leaves the original key intact on refusal; `--force` replaces it; a missing identity reports `NoIdentity`; a public key file that does not match the private key is rejected; a missing public key file is tolerated and re-derived; files are 0600 with no group/other access (skipped on Windows, ADR-0004); `known_peers` is written with LF endings and a header; corrupt files are reported with the path and line number; atomic writes leave no temporary files behind |

### Command tests — `tests/cli.rs`

| Area | Cases |
|------|-------|
| `init` (F-1) | creates all three files and prints Short ID and fingerprint; a second `init` exits 1, explains itself, and leaves the existing key untouched; `--force` generates a different key |
| `whoami` (F-2) | `--json` is valid JSON whose `public_key` really does hash to the reported `fingerprint`; without an identity it exits 1 and points the user at `beam init` |
| `peers` (F-3) | empty state prints guidance, not an empty table; the table shows names and fingerprints; `--json` preserves file order and carries `added`; a corrupt `known_peers` exits 1 naming the bad line |
| `rename` (F-4) | renames; rejects unknown peers, collisions and a missing argument |
| `remove` (F-5) | the prompt shows the fingerprint; answering `n` keeps the peer; end-of-input keeps the peer; `y` removes only that peer and leaves the other; `--yes` skips the prompt; unknown peer exits 1 |
| CLI contract (N-5) | stubbed commands exit 2 and say which milestone they belong to; an unknown command exits 1; `--help` lists every planned command; `version` prints |

## Implemented (M2)

### Unit tests — `src/transfer/`, `src/transport/`, `src/hex.rs`

| Area | Cases |
|------|-------|
| Framing (ADR-0015) | every message round-trips; messages stream back to back; an empty stream reads as a clean close and a half-written frame as truncated; a frame declaring 4 GiB is refused *before* allocating; a frame exactly at the 64 KiB limit is accepted; encoding refuses to produce an over-sized frame; unknown frame types, short `CHUNK_DATA` payloads and malformed JSON are refused |
| Messages | transfer ids round-trip through hex and reject wrong lengths and non-hex; two generated ids differ; `TRANSFER_REQUEST` round-trips through JSON; unknown JSON fields are refused; `ACCEPT` defaults to no bitmap; only chunk messages report themselves as carrying file data |
| Chunk arithmetic | empty file, smaller than one chunk, exact multiple, one byte over; chunk lengths always sum to the file size across a grid of sizes and chunk sizes; a realistic 9 MB plan; a declared chunk count that does not match the size is refused, as is a zero chunk size; SHA-256 against known vectors; hashing a stream matches hashing a buffer and reports progress |
| Path safety (ADR-0017) | ordinary names pass through unchanged; seven traversal forms reduce to a base name; twelve names that cannot be reduced are refused with the specific reason; Windows device names refused with or without an extension, while `console.log` and `com10.txt` pass; over-long names refused at the boundary; extensions split as a person would expect; collisions become `report (1).pdf`; `.gitignore` keeps its name; reserving a name creates the file; **and, for every hostile input, the file actually opened is a direct child of the destination directory** |
| State machine | all 12 states × 13 events checked against a hand-written transition table, so both legal transitions and refusals are covered; terminal states accept nothing; cancel and fail work from every live state; file data is allowed in exactly one state; the happy path walks to `Completed`; an interruption round-trips; an illegal event leaves the state untouched; a finished transfer cannot be restarted |
| Transport | `PathKind` labels; loopback detection for IPv4, IPv6 and non-loopback addresses |
| Hex | round-trip, case-insensitive decoding, wrong length and non-hex refused |
| Terminal output | byte counts format the way a person says them; percentages are bounded and an empty file reads as complete; tables line up; `confirm` says no to everything but `y`/`yes`, including end of input |

### Integration tests — `tests/transfer.rs`

| Area | Cases |
|------|-------|
| Happy paths | a file arrives byte for byte, with both sides agreeing on the transfer id and the saved name; an empty file transfers; a file spanning two chunks and a short third transfers; the same happy path over a real `TcpStream` (ADR-0016) |
| Naming | a colliding name becomes `report (1).pdf`, the existing file is untouched, and both sides are told the final name |
| Refusal | a declined transfer leaves nothing on disk |
| Integrity (S-12) | a chunk whose bytes do not match its announced hash is refused and asked for again, and only the correct bytes are written; a chunk that never verifies fails the transfer with nothing written; the sender re-sends a chunk it is NAKed |
| Malformed requests | a replayed transfer id is refused without a prompt (S-11); a hostile file name is refused before anything is created; a chunk count that does not match the declared size is refused |

### End-to-end tests — `tests/end_to_end.rs`

Every other test calls the library directly. That is fast, and blind to
anything that only goes wrong in a whole program.

The bug that prompted these tests is the example. `main.rs` held a `StdoutLock`
for the duration of the run, so the thread that draws the Accept prompt blocked
forever the first time it tried to write to stdout. Every unit test, every
command test and all nineteen engine tests passed. The program was unusable.

These tests spawn `beam listen` and `beam send` as real processes and **wait for
the prompt to appear on the child's output before answering it**. That is the
part that matters: a prompt that never arrives is a timeout and a failed test,
not a mystery discovered by hand later.

| Test | What it covers |
|------|----------------|
| `two_processes_complete_a_transfer` | `listen` on port 0 reports the address it actually got; the prompt appears and names the sender and fingerprint; answering `y` completes the transfer; the received bytes equal the sent bytes; both processes exit cleanly |
| `answering_no_between_two_processes_saves_nothing` | answering `n` is honoured across the process boundary: the sender exits non-zero and is told it was declined, and nothing is written |

**No pseudo-terminal is involved, and these run on Windows CI.** The prompt is
written to stdout whether or not stdout is a terminal, and the deadlock was
about the lock rather than the tty, so pipes reproduce it. Output is pumped a
byte at a time because the prompt ends in `[y/N]: ` with no newline — a
line-buffered reader would not see it until something else produced a newline,
which is a mistake worth recording since it cost an afternoon during M2.

The child's stdout and stderr are merged into one stream, because beam reports a
completed transfer on stdout and a refused one on stderr.

### The six Accept rules — `tests/transfer.rs::accept_rules` and `tests/cli.rs`

| Rule | Test | What it proves |
|------|------|----------------|
| S-1 — a person accepts every transfer, with no bypass | `s1_the_prompt_is_the_only_route_to_accepting`, plus `listen_has_no_flag_that_could_stand_in_for_the_prompt` and `no_command_offers_an_auto_accept_switch` in `tests/cli.rs` | refusing at the prompt stops the transfer dead, and the CLI carries no flag that could answer for the person. The CLI test is an **allowlist** of `listen`'s flags, so any new flag fails the test until somebody has decided it is not a bypass |
| S-3 — no file bytes before ACCEPT | `s3_the_sender_emits_no_file_bytes_before_accept` | a hand-written peer that never accepts records every frame the sender sends. It sees exactly one: `TRANSFER_REQUEST` |
| S-4 — data before ACCEPT is discarded and the transfer aborts | `s4_data_before_accept_aborts_and_discards` | a rude sender pushes chunks straight after the request; the receiver returns `DataBeforeAccept`, and both the destination and the work directory are empty afterwards |
| S-5 — silence is a Reject | `s5_an_unanswered_request_expires_as_a_reject` and `s5_the_default_deadline_is_sixty_seconds` | the first proves the mechanism with a 200 ms deadline — the person is asked, nobody answers, both sides see `Expired`, nothing is written. The second asserts the real deadline is 60 s. Split in two because a test that waits a minute does not get run, and `tokio::time::pause` cannot help: its clock only advances while the runtime is idle, and the prompt deliberately occupies a blocking thread |
| S-6 — the prompt shows sender, fingerprint, name and size | `s6_the_prompt_shows_who_what_and_how_big` | all four fields are captured from the real prompt call. The name shown is the one the *receiver* stored, not one the sender supplied — `TRANSFER_REQUEST` has no nickname field at all |
| S-7 — only peers in `known_peers` may ask | `s7_an_unknown_sender_is_refused_without_a_prompt` | an unrecognised key is refused, the sender is told why, nothing is written, **and the prompt is never called**. Marked `STRENGTHEN IN M6:` — see below |

**What S-7's test does not prove.** It shows that an *unrecognised* key is turned
away. It does not show that a sender presenting a *recognised* key holds the
matching private key, because in M2 nothing checks that (ADR-0019, requirement
S-7a). When the Noise KK handshake lands in M6, this test gains a sibling that
replays a known peer's public key without its private key and expects a refusal —
a test that would fail today.

## Implemented (M3)

### Unit tests

| Area | Cases |
|------|-------|
| Bitmap (S-15, ADR-0024) | a new bitmap holds nothing; set and clear touch one chunk only; every index in a range of lengths can be set independently; out-of-range indices are ignored rather than panicking; a full bitmap is complete; an empty transfer is complete immediately; round-trip through the encoding for eight lengths; the encoded length is one bit per chunk; **decoding refuses a bitmap of the wrong length, one with bits past the end of the transfer, one that is not base64, and one that is valid for a different transfer** |
| Free space (N-7) | a directory is on the same volume as itself; a path that does not exist is treated as a different volume, which is the conservative answer; free space is reported for a real directory; a transfer that fits is allowed; one larger than the disk is refused with a message naming what was needed and what was free; **only the missing bytes have to fit**, so a nearly finished resume is not refused over the size it already has |
| Commit (D-13, ADR-0023) | the rename path moves the file and removes the part file; **the copy path produces identical contents**, replaces the placeholder the receiver reserved, and leaves nothing behind when it fails |
| State machine (S-2, ADR-0020) | all 10 states × 10 events against a hand-written table; `Interrupted` and `Reconnecting` are gone, and a lost connection ends in `Failed` with no way back |
| Age formatting | reads the way a person would say it, and never "0 seconds ago" |

### Resume tests — `tests/resume.rs`

| Required case | Test | What it establishes |
|---|---|---|
| Interrupted, then resumed | `an_interrupted_transfer_resumes_and_sends_only_what_is_missing` | the second session prompts again (S-2) and says it is a resume; the sender skips exactly the bytes already held; the finished file matches; no partial is left |
| A stored chunk is corrupted on disk | `a_corrupted_stored_chunk_is_re_fetched_rather_than_trusted` | the damaged chunk is dropped from the bitmap and re-requested, and the finished file is correct |
| The source file changed between sessions | `a_changed_source_file_starts_over_instead_of_resuming` | nothing is reused, the prompt does not call it a resume, the new file is correct, and the old partial is left alone (D-9) |
| A malformed have-bitmap | `a_malformed_have_bitmap_aborts_the_sender` | four kinds — too short, too long, bits past the end, not base64 — each abort the sender |
| A resume from a peer that is not paired | `a_partial_is_never_offered_to_a_different_peer` | a stranger asking for the same file is refused before the prompt, learns nothing, and leaves the partial untouched (S-14) |
| A second session for the same partial | `a_second_session_for_the_same_partial_is_refused` | refused with `Busy`, no prompt shown, partial undamaged (D-12) |
| A resume declined at the prompt | `declining_a_resume_keeps_the_partial_for_next_time` | the partial survives *and is then used* by a third session that completes |

Retention, rule by rule, against the table in ADR-0022:

| Rule | Test |
|---|---|
| A finished transfer discards its partial | `a_finished_transfer_leaves_no_partial` |
| A partial whose file fails the whole-file hash is discarded, and nothing reaches the destination | `a_file_that_fails_its_final_hash_is_discarded_and_never_written` — every chunk hash is honest, so only the final check catches it |
| A declined resume keeps it | `declining_a_resume_keeps_the_partial_for_next_time` |
| A declined *fresh* transfer keeps nothing | `declining_a_fresh_transfer_leaves_nothing_behind` |
| An interruption keeps it | `an_interrupted_transfer_resumes_and_sends_only_what_is_missing` |
| A peer sending data before ACCEPT cannot destroy an existing partial | `data_before_accept_cannot_destroy_an_existing_partial` |
| A blocked second session changes nothing | `a_second_session_for_the_same_partial_is_refused` |
| A stranger's request changes nothing | `a_partial_is_never_offered_to_a_different_peer` |

### Killed-process tests — `tests/end_to_end.rs`

| Test | What it covers |
|---|---|
| `killed::killing_the_sender_mid_transfer_then_resuming` | the sender is killed outright; the partial on disk is what had been flushed; the next run prompts as a resume, says "Already have", and produces the right file |
| `killed::killing_the_receiver_mid_transfer_then_resuming` | the same with the receiver killed, which is the case the write ordering in ADR-0022 exists for: a `SIGKILL` gives nothing a chance to tidy up |

### `beam transfers` — `tests/cli.rs`

Empty state; listing with percentage and a stale entry marked expired; `--json`;
confirmation before clearing, with the prompt naming what is about to be lost;
declining keeps the partial; clearing one by an abbreviated id leaves the others;
an unknown id and an ambiguous id are both errors rather than guesses.

### Two flaky tests, and why they were flaky

Worth recording, because both were found by running the suite repeatedly rather
than once, and both would have failed in CI eventually.

- **Interrupting on a timer is a race the test loses.** An in-memory pipe moves
  a few hundred kilobytes in microseconds, so a transfer meant to be cut short
  sometimes finished first, and the test then found no partial. Interruptions
  are now driven by a hand-written sender that sends exactly *n* chunks and
  hangs up, which leaves the same partial every time.
- **A prompt that answers instantly can win a race no person would.** The tests
  about data arriving *while the prompt is open* depend on the data winning. A
  prompt returning in nanoseconds sometimes beat it. Those tests now use a
  prompt that takes 300 ms, as a person does.

## Manual test steps

Some things cannot honestly be covered by an automated test on one machine.
These are short, and worth running before a release.

### Cross-volume commit (D-13, ADR-0023)

The copy path has unit tests, but nothing automated proves that a *real*
cross-volume rename produces the error that triggers it. Needs a second volume:
another drive, a USB stick, or a mounted image.

```bash
# Linux: a small tmpfs makes a second volume without extra hardware
sudo mkdir -p /mnt/beamtest && sudo mount -t tmpfs -o size=64M tmpfs /mnt/beamtest
sudo chown "$USER" /mnt/beamtest

beam listen --addr 127.0.0.1:7777 --out /mnt/beamtest      # ~/.beam is on / 
# from another terminal, send a file and accept it
sha256sum payload.bin /mnt/beamtest/payload.bin            # must match
sudo umount /mnt/beamtest
```

```powershell
# Windows: any second drive letter will do
beam listen --addr 127.0.0.1:7777 --out D:\beam-inbox
# from another terminal, send a file and accept it
Get-FileHash payload.bin -Algorithm SHA256
Get-FileHash D:\beam-inbox\payload.bin -Algorithm SHA256   # must match
```

Expected: the transfer completes, the hashes match, and no `.beam-commit-*`
file is left in the destination directory.

### Too little disk space (N-7)

```bash
# Linux: a tiny tmpfs as the destination
sudo mount -t tmpfs -o size=1M tmpfs /mnt/beamtest
beam listen --addr 127.0.0.1:7777 --out /mnt/beamtest
# send something larger than 1 MiB
```

Expected: the sender is told the peer has not enough free disk space, **no
prompt appears on the receiver**, and nothing is written.

### The prompt cannot be answered in advance

```bash
echo y | beam listen --addr 127.0.0.1:7777
```

Expected: the piped `y` is discarded and the prompt still waits. Answering a
question you have not seen is exactly what S-1 forbids.

## Planned

### M4 — rendezvous server and pairing

Registration and expiry; lookup by Short ID returns an iroh endpoint address;
server restart; server unreachable; a Short ID that is not registered; a
successful SPAKE2 pairing writes exactly one `known_peers` entry on each side; a
wrong pairing code fails on both sides and writes nothing; `newcode` invalidates
the previous code; the relay URL is read from configuration; and a test that
beam never installs n0's discovery services (S-17).

### M5 — the iroh transport

The M2 and M3 transfer tests re-run unchanged over an iroh stream, which is the
point of ADR-0016 and what the spike prototype already demonstrated once. The
progress line reports `[Direct P2P]` or `[Relay]` from `IncomingAddr` (F-11).

### M6 — threat model and the security tests that back it

A written `docs/threat-model.md`, and tests proving impersonation fails at the
transport level rather than at ours: an unknown key; a known key held by someone
without the matching secret key; a rendezvous server returning the wrong address
for a Short ID; and a peer that re-ran `beam init`, which must fail with a
message telling the user to re-pair (S-8).

Every `STRENGTHEN IN M6:` marker is removed as its case becomes covered, and
S-7a moves from "outstanding" to "met".

## Known gaps

- File permissions are only asserted on Unix-like systems; on Windows the mode
  bits carry no meaning (ADR-0004).
- Zeroization (ADR-0013) is not covered by a test: there is no portable way to
  assert that a buffer was wiped, since reading it after the wipe is exactly
  what the type system prevents. It is enforced by the types instead —
  `Zeroizing` and `ZeroizeOnDrop` — and by review.
- macOS is not in the CI matrix (ADR-0014).
- The Accept prompt's own rendering is not covered by a test: it writes to
  the real stdout from a blocking thread. What it shows is asserted through
  the engine instead, in `s6_the_prompt_shows_who_what_and_how_big`.
- `beam listen` blocks until a peer connects, so its success path has no
  *command-level* test; it is covered by `tests/end_to_end.rs`, which runs it as
  a real process, and by the manual walkthrough in `README.md`.
- The cross-volume *dispatch* — that a real cross-volume rename produces the
  error that triggers the copy path — is a manual step above, not an automated
  test. The copy path itself is covered.
- Disk-space exhaustion is checked by unit tests against absurd sizes rather
  than by actually filling a disk; the manual step above does it for real.
- The progress line's rendering is not asserted anywhere. It adapts to whether
  stdout is a terminal, and the end-to-end tests see the non-terminal form.
