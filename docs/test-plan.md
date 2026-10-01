# Beam — test plan

> **Branch `main-test`:** there is no rendezvous server on this branch;
> devices meet through invites and are found again by key through the relay
> (ADR-0036 in `decisions.md`). Rendezvous-server test cases apply to `main`. Here they are replaced by the invite tests (`invite::tests`), `tests/pairing.rs`, and the address tests in `tests/listen.rs` and `tests/cli.rs`.

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
S-7a). When transfers move onto iroh in M5, the handshake proves the sender's
key, and M6 adds a sibling test that presents a known peer's public key without
its private key and expects a refusal — a test that would fail today.

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

## Implemented (M4)

Nothing in this section touches the internet. The integration and end-to-end
tests run a real rendezvous server in-process, pair over real iroh endpoints
bound to `127.0.0.1`, and set `relay = "none"`, so they pass offline and on CI.

### Unit tests

| Module | What is covered |
|---|---|
| `config` | defaults when the file is missing or empty; both keys read; `relay = "none"`; unknown keys, non-WebSocket rendezvous URLs and non-URL relays refused; BOM skipped |
| `transport::endpoint` | the endpoint id is the beam public key byte for byte; a bound endpoint uses the device key and, on loopback, advertises only loopback |
| `pairing::code` | six digits, uniform, leading zeros kept; typed with spaces or dashes; malformed codes refused; `Debug` redacted; **taken once**; **expires** after its TTL and stays gone |
| `pairing::protocol` | the attack tests below, plus declines on either side, an unanswered confirmation, version, oversize, unknown-field and silence handling |
| `rendezvous::proto` | a signed registration verifies; tampered body, another key's signature, garbage signatures, a signature without the domain label; stale and future timestamps; a Short ID the key does not derive; an address for another endpoint; too many addresses; unknown fields; the client dropping lookup entries that do not check out |
| `rendezvous::server` | lookup until expiry; refresh extends and updates; **old timestamps cannot be replayed**; **colliding Short IDs return every entry**; a bounded number of entries per Short ID; closing a connection removes its entries |

### The five conditions of the M4 approval

| Condition | Tests |
|---|---|
| 1. Codes are single use and expire | `pairing::code::a_code_can_be_taken_once`, `a_code_expires_after_its_ttl`; `tests/pairing.rs::a_wrong_code_pairs_nobody_and_uses_the_code_up` (the right code afterwards finds nobody), `an_expired_code_ends_the_wait_and_unregisters`; `tests/end_to_end.rs::pairing::a_wrong_code_between_two_processes_saves_nothing_on_either_side` |
| 2. The MAC covers both keys, Short ID and roles; the saved key is `remote_id()`; a substituted key fails | `pairing::protocol::an_attacker_relaying_with_its_own_key_cannot_complete_pairing`, `a_claimed_key_that_differs_from_the_proved_key_is_refused`, `a_reply_claiming_a_different_key_is_refused`, `confirmations_are_bound_to_the_role`, `confirmations_are_bound_to_both_keys_and_the_short_id`, `the_code_is_bound_to_the_short_id`; `tests/pairing.rs::pairing_returns_the_key_each_side_proved_on_the_connection` |
| 3. Signed, timestamped registrations; derivation checked | `rendezvous::proto` tests above; `tests/pairing.rs::the_server_refuses_registrations_that_do_not_check_out` (a real server, raw requests) |
| 4. The receiver confirms with `[y/N]`, no bypass | `pairing::protocol::if_the_waiter_declines_neither_side_pairs`, `an_unanswered_confirmation_is_a_no`, `nobody_is_asked_to_confirm_after_a_wrong_code`; `tests/pairing.rs::if_the_waiter_says_no_neither_side_pairs`, `a_device_that_is_already_paired_is_not_offered_again`; `tests/cli.rs::pair_has_no_flag_that_could_stand_in_for_the_confirmation`; `tests/end_to_end.rs::pairing::answering_no_to_pairing_saves_nothing_on_either_side` |
| 5. Which command waits | `beam pair --wait` (ADR-0028); `tests/end_to_end.rs::pairing::beam_pair_between_two_processes_then_a_transfer` |

Three of these were checked by mutation — the check removed, the test run,
the check restored: removing both keys from the SPAKE2 identities and the MAC
lets the relaying attacker pair and fails its test; removing the claimed-key
check fails both claimed-key tests; removing the server's Short ID derivation
check fails its test.

### Integration tests — `tests/pairing.rs`

Besides the rows above: an unknown Short ID is reported as "not waiting" with
the command to run; a server that is down is reported as unreachable, naming
`beam-server`; a taken name fails before any network traffic; pairing with your
own Short ID is refused; a registration lasts only as long as its connection.

### End-to-end tests — `tests/end_to_end.rs::pairing`

Two real `beam pair` processes and an in-process server. As with the transfer
tests, every prompt is waited for on the child's stdout before it is answered:
the code prompt, then both `[y/N]` prompts, which must each show both
fingerprints. The successful case then runs `listen` and `send` between the two
freshly paired homes, so pairing is shown to produce a `known_peers` entry the
transfer engine accepts.

### S-17 — `tests/no_n0_discovery.rs`

iroh cannot report which address lookup services a bound endpoint has, so the
test checks the source: no `presets::N0`, pkarr or DNS lookup, `address_lookup`
call or `RelayMode::Default` anywhere in `beam` or `beam-server`, and every
`Endpoint::builder` uses `presets::Minimal`.

### Command tests — `tests/cli.rs`

`pair` needs `--name`; needs a Short ID or `--wait` but not both; a malformed
Short ID, a taken name and a missing identity all fail before the network; a
broken `config.toml` names the file; `newcode` is still a stub (M5).

### Manual: pairing two real machines

On the same Wi-Fi, with `beam-server` on machine A:

```bash
# machine A
beam-server --addr 0.0.0.0:8787
# machine A and B: point ~/.beam/config.toml at it
echo 'rendezvous = "ws://<A-LAN-IP>:8787/v1"' > ~/.beam/config.toml
# machine B
beam pair --wait --name laptop-a
# machine A
beam pair <B's Short ID> --name laptop-b
```

Check: both screens show the same two fingerprints, the other way round; typing
a wrong code fails on both sides and `beam pair --wait` has to be run again; an
unanswered prompt gives up after about a minute; `beam peers` lists the other
machine on both sides afterwards.


## Implemented (M5)

As in M4, nothing here touches the internet: real iroh endpoints bound to
`127.0.0.1`, a real rendezvous server in-process, `relay = "none"`.

### Unit tests

| Module | What is covered |
|---|---|
| `transport` | a route tracker reports each path change once; a fixed route never changes |
| `pairing::rotation` | a used code is replaced; an unused one expires after 10 min, and cannot be taken just before the tick; one attempt at a time; a failure pauses pairing 5 s, then 10 s; **three failures in a row turn pairing off for good**; a proved-but-refused attempt resets the count; the pause is capped at 5 min |
| `pairing::protocol` | the joiner's name hint reaches the waiter's decision; an oversized hint is dropped; an `Unavailable` waiter is reported with its reason |
| `pairing::session` | a usable hint becomes the name; an unusable one is cleaned up or replaced by `peer-<fingerprint>`; a taken name gets a number; only an unproved code counts as a guess |
| `rendezvous` | a key lookup answer must be for the key asked about, at its endpoint; a key lookup finds only that key even under a Short ID collision |
| `cli::desk` | `y` accepts a transfer; **`y` does not confirm pairing** (nor `ye`, `yes please`, empty) — `yes` does; the two prompts share no wording; **two questions at once are asked one after the other**, the second showing its remaining time; a question that expires in the queue is refused unseen and reported; a pasted second line cannot answer the next question |

### Engine tests — `tests/transfer.rs`

A path change mid-transfer is reported exactly once and later updates carry the
new path; a request claiming a key other than the proven one is refused
unprompted; one matching it goes through; a sender turned away as busy is told
"receiving another file; try again later".

### `listen` as a service — `tests/listen.rs`

| Requirement | Test |
|---|---|
| A transfer over iroh, direct path reported | `a_paired_peer_sends_a_file_over_iroh` |
| S-1 decline | `a_declined_transfer_saves_nothing` |
| S-5 expiry | `an_unanswered_transfer_expires` |
| S-7 on the proved key | `an_unpaired_device_is_refused_without_a_prompt` |
| S-25 / ADR-0031 | `a_paired_device_claiming_another_ones_key_is_refused` |
| S-3 / S-4 | `data_before_accept_is_discarded` (a raw client over iroh) |
| S-2 resume | `an_interrupted_transfer_resumes_with_a_new_accept` |
| F-16 busy | `a_second_transfer_while_one_is_open_is_told_to_try_later` |
| Pairing inside `listen`, named from the hint, code renewed | `listen_pairs_and_names_the_device_from_its_hint` |
| **S-23: three failures switch pairing off, transfers unaffected** | `three_failed_attempts_switch_pairing_off_but_not_transfers` |
| S-23: attempts during a pause do not count | `an_attempt_during_a_pause_is_refused_without_counting` |
| **S-24: a pairing and a transfer at once, through the real desk** | `a_pairing_and_a_transfer_at_once_are_asked_one_at_a_time` |
| ADR-0031: a re-initialised peer is not found under its old key | `dialling_a_key_nobody_holds_any_more_finds_nobody` |

### End-to-end tests over iroh — `tests/end_to_end.rs::over_iroh`

Real `beam listen --loopback` and `beam send --loopback` processes, found
through a real rendezvous server, every prompt waited for on the child's stdout
before it is answered:

- a transfer accepted by hand arrives intact, and the sender's output says
  `[Direct P2P]`;
- answering `n` saves nothing and the sender is told;
- an unpaired sender is refused with no prompt on the receiver's screen, and is
  told how to re-pair;
- **the sender killed mid-transfer, then sent again**: the receiver notices
  within the 15 s idle timeout, the second run shows `(resuming)` and
  `Already have`, sends only the rest, and the file matches;
- a second sender while a prompt is open is told `bob is receiving another
  file; try again later`;
- `listen` pairs, but **answering `y` does not pair**; the proved-but-refused
  attempt gets a new code at once, and `yes` then pairs.

The M4 pairing tests now answer `yes`, since the pairing prompt asks for it.

### Command tests — `tests/cli.rs`

`newcode` is gone; the `listen` allowlist is `addr`, `loopback`, `out` (the two
hidden flags choose a transport and an address, and neither answers anything);
`send` to a peer that is not listening names both causes and how to re-pair;
`send` with the rendezvous server down says so.

### Manual: a real path change, and a real relay

Automated tests run with the relay off, so the `[Relay]` label and a change of
path are checked by hand, on two networks (the steps in
`spikes/transport.md`, with beam itself):

1. Machine A on home Wi-Fi runs `beam listen`; machine B on a phone hotspot
   runs `beam send`. Both use the default relay.
2. Expect `[Relay] accepted` at first, and — if hole punching succeeds —
   `Path changed: [Relay] -> [Direct P2P]` a few seconds in. Over two mobile
   networks behind CGNAT, expect it to stay `[Relay]`.
3. With `relay = "none"` on both, over those same two networks, expect the
   connection to fail rather than relay.

### Manual: the rendezvous server restarting under `listen`

Stop `beam-server` while `beam listen` runs: `listen` warns on stderr that it
cannot register and keeps retrying every 5 s. Start the server again: `listen`
prints "Registered with the rendezvous server again." and is findable again.


## Implemented (M6)

`docs/threat-model.md` maps every threat to the test that demonstrates its
mitigation; this section lists the tests M6 added.

### Impersonation, at the transport level — `tests/listen.rs::impersonation`

| M6 case | Test |
|---|---|
| An unknown key | `an_unknown_key_is_refused_without_a_prompt` |
| A known key without its secret key | `a_known_public_key_without_its_secret_key_gets_nowhere` — the sibling the last `STRENGTHEN IN M6` marker was waiting for |
| A rendezvous server returning a wrong address | `a_rendezvous_that_returns_the_wrong_address_cannot_redirect_a_send` (the attacker endpoint completes no handshake), `a_rendezvous_answer_for_another_key_is_ignored` |
| A peer that re-ran `beam init` | `tests/end_to_end.rs::over_iroh::a_receiver_that_re_ran_init_gets_a_re_pair_warning_not_a_transfer`, `a_sender_that_re_ran_init_is_refused_with_a_re_pair_warning` — real processes; the message says re-pair and warns about impersonation |

The fake rendezvous server in these tests answers every request with a scripted
reply; the attacker is a real iroh endpoint on loopback that counts completed
handshakes.

### Terminal injection — `tests/injection.rs`, `untrusted` and `transfer::paths` unit tests

- File names with CSI, OSC (window title), `\r`, 8-bit CSI: refused before the
  prompt, and no ESC reaches the screen.
- A name with U+202E: refused before the prompt (`BidiControl`).
- **A Thai name with vowels and tone marks**: shown and saved unchanged.
- **An emoji with ZWJ**: accepted, saved as sent, shown as `👨<U+200D>👩…`.
- ZWSP in a name: accepted, shown as `<U+200B>`.
- **A long name that would hide `.exe` if cut at the end**: shown cut in the
  middle, ending `.pdf.exe` (the test first proves the naive cut hides it).
- A hostile host-name hint becomes a plain nickname; the pairing prompt shows a
  hostile name only through the sanitizer.
- A CANCEL reason and a rendezvous error full of escapes reach the error text
  without them; REJECT reasons are an enum and cannot carry text.
- Unit: every ANSI form, `\r` overwrite, other controls, length caps, idempotence.

### A misbehaving paired peer — `tests/hostile_peer.rs`, `tests/listen.rs`

| Case | Before M6 | Test |
|---|---|---|
| 64 TiB, validly described | refused for space, **after** writing a per-chunk bitmap (13 s) | `a_sixty_four_tib_request_is_refused_for_space_without_a_prompt` — now before anything is written |
| A size whose chunk count wraps `u32` | could be declared as the wrapped value | `a_size_whose_chunk_count_would_wrap_is_refused` |
| 1 GiB in 1-byte chunks | accepted: a billion chunks of state | `a_request_with_too_many_chunks_is_refused` |
| `chunk_size` over 16 MiB | allocated up to 4 GiB per chunk | `a_chunk_size_over_the_cap_is_refused` |
| A 1 GiB frame header | refused ✓ | `an_oversized_frame_is_refused_from_its_header` |
| Malformed / unknown / extra-field frames | refused ✓ | `malformed_frames_are_refused_without_a_prompt` |
| Silent after ACCEPT | waited for ever | `a_sender_that_goes_quiet_after_accept_is_given_up_on` |
| Silent mid-chunk | waited for ever | `a_sender_that_goes_quiet_mid_chunk_is_given_up_on` |
| A receiver that never ACKs | the sender waited for ever | `a_receiver_that_never_acknowledges_is_given_up_on` |
| **Slow final verification** (M6 answer 3) | — | `a_slow_final_verification_does_not_trip_the_stall_timeout`, and its control `without_keepalives_the_same_verification_would_stall` |
| Holding `listen`'s slot | held for ever | `a_paired_peer_holding_the_slot_is_dropped_and_the_next_can_send` (over iroh) |

### Redraw after a notice — `cli::desk` unit tests

`a_notice_during_a_question_redraws_the_question` (and the answer still
counts), `a_notice_with_no_question_open_is_printed_at_once`,
`notices_do_not_let_a_queued_question_jump_in`.

### Mutation checks

Each check removed, the tests run, the check restored: bidi rejection in file
names → the U+202E test fails; keep-alives during verification → the
slow-verification test fails; the key check on a rendezvous answer → the
"answer for another key" impersonation test fails. (M4 and M5 mutation checks
still stand.)


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
- Pairing across two real networks, and through a real relay, is a manual
  step (README); the automated tests pair on loopback with the relay off so
  that CI never depends on the internet.
- Short ID collisions are tested at the table level, not by grinding a real
  colliding key: that would take ~2³⁰ key generations per test run. The part
  that matters — every entry is returned, and the joiner moves past one that
  fails the code — is covered by `colliding_short_ids_return_every_entry` and
  the candidate loop in `pairing::session::join`.
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
