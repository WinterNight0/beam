# Beam — test plan

## How to run

```
make check                                  # cargo fmt --check + clippy -D warnings + cargo test
powershell -File scripts\check.ps1          # the same, on Windows without make
cargo test known_peers                      # one area
cargo test --lib                            # unit tests only
cargo test --test cli                       # command-level tests only
```

`make check` is the gate: a milestone is not done until it passes.

## Test levels

| Level | Where | What it covers |
|-------|-------|----------------|
| Unit | `#[cfg(test)]` modules inside `crates/beam/src/identity/`, later `src/transfer/` | derivations, parsing, state machine transitions |
| Command | `crates/beam/tests/cli.rs` | whole commands driven through `cli::execute` with in-memory streams and a temporary `--beam-dir` |
| Integration | from M2 | two engines over a real socket on localhost, then client↔server and client↔relay |
| Failure | from M2 | interruption, corruption, disconnection, disk full, cancel, crash-and-resume |
| Security | from M2 | the S-* requirements in `requirements.md`, each with a test that tries to break it |

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

## Planned

### M2 — transfer engine over TCP on localhost

Unit: chunk splitting at boundaries (empty file, smaller than one chunk, exact
multiple, one byte over); per-chunk and whole-file hashing; the state machine —
every legal transition, and every illegal transition rejected.

Integration: send and receive a file end to end; the received bytes and SHA-256
match the source; the destination is only written after verification.

Security, one test per requirement: DATA arriving before ACCEPT aborts the
transfer and discards what was received (S-3, S-4); a request from a peer absent
from `known_peers` is rejected with no prompt shown (S-7); an unanswered request
expires after 60 s and is treated as a Reject (S-5); the Accept prompt contains
the sender name, fingerprint, file name and size (S-6); there is no code path,
flag or config key that accepts without the prompt (S-1); a tampered chunk fails
its hash and is re-requested (S-12); an existing destination file is not
overwritten (D-7).

Failure: connection dropped mid-transfer; corrupted chunk; duplicate chunk;
cancel from either side; disk full while writing the temp file.

### M3 — resume

Bitmap set/clear/count/serialisation; resume after an interruption transfers
only missing chunks; a resumed transfer prompts for a new Accept (S-2); a
replayed transfer ID is rejected (S-11); crash and restart mid-transfer.

### M4 — signaling server and pairing

Presence registration and heartbeat expiry; lookup by Short ID; server restart;
server unreachable; a successful PAKE pairing writes exactly one `known_peers`
entry on each side; a wrong pairing code fails on both sides and writes nothing;
`newcode` invalidates the previous code.

### M5–M7 — WebRTC, Noise, relay

The M2 transfer tests re-run unchanged over a data channel; ICE failure falls
back to relay; the Noise KK handshake is bound to the DTLS fingerprint and a
substituted fingerprint aborts; a changed peer key aborts with the SSH-style
warning and does not update `known_peers` (S-8); the progress line reports
`[Direct P2P]` or `[Relay]` correctly (F-11).

## Known gaps

- File permissions are only asserted on Unix-like systems; on Windows the mode
  bits carry no meaning (ADR-0004).
- There is no CI runner configured yet. `make check` is the contract a runner
  would execute.
- `Identity::generate` zeroizes its seed buffer on a best-effort basis rather
  than with the `zeroize` crate (ADR-0013); there is no test for that, because
  there is no portable way to assert it.
