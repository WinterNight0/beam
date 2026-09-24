# Beam — test plan

## How to run

```
make check                                  # gofmt + go vet + go test
powershell -File scripts\check.ps1          # the same, on Windows without make
go test ./... -run TestKnownPeers -v        # one area
```

`make check` is the gate: a milestone is not done until it passes.

## Test levels

| Level | Where | What it covers |
|-------|-------|----------------|
| Unit | `internal/identity`, later `internal/transfer` | derivations, parsing, state machine transitions |
| Command | `internal/cli` | whole commands driven through `cli.Execute` with an in-memory stdin/stdout and a temporary `--beam-dir` |
| Integration | from M2 | two engines over a real socket on localhost, then client↔server and client↔relay |
| Failure | from M2 | interruption, corruption, disconnection, disk full, cancel, crash-and-resume |
| Security | from M2 | the S-* requirements in `requirements.md`, each with a test that tries to break it |

Command-level tests never spawn a process and never touch the real `~/.beam`:
every test gets `t.TempDir()` via the `--beam-dir` flag.

## Implemented (M0–M1)

### `internal/identity`

| Area | Cases |
|------|-------|
| Fingerprint / Short ID (D-3, ADR-0006) | fixed vectors for two deterministic keys; Short ID always 9 digits including the all-zero and all-ones extremes; display grouping; `ParseFingerprint` accepts hex, `SHA256:` prefix and colon-separated forms and rejects wrong length, bad hex and empty input; `ParseShortID` accepts spaces and dashes, rejects wrong digit counts and non-digits |
| Keypair (F-1, S-9) | generated keys sign and verify; two generations differ; PKCS#8 PEM round-trip; garbage private keys rejected (not PEM, wrong PEM label, bad DER); public key line round-trip; a comment containing newlines or tabs cannot break the one-line format; bad public key lines rejected (wrong type, bad base64, short key, missing key) |
| known_peers parsing (D-3, D-4) | space- and tab-separated fields; comments and blank lines; unknown `key=value` attributes preserved; rejects too few fields, wrong key type, bad base64, short key, invalid name, bare token, invalid attribute key, unparseable `added=`, duplicate name, duplicate name differing only in case, and the same public key under two names — each with a `*ParseError` carrying a line number |
| known_peers editing (F-4, F-5) | `Add` stamps `added=` and rejects duplicate names, duplicate keys and invalid names; `Lookup` is case-insensitive; `LookupKey` matches by key; `Rename` keeps the key, the attributes, the comments and the file order, allows a pure capitalisation change, and rejects collisions and unknown peers; `Remove` is case-insensitive and keeps comments; rendering is idempotent |
| Name rules (D-3) | accepts `a`, `Alice-2`, `my.laptop`, `under_score`, 32 characters; rejects empty, spaces, emoji, `/`, `#`, 33 characters |
| Store (D-1, D-2, D-5) | save/load round-trip; `$BEAM_DIR` override; `init` creates `known_peers` and `tmp/` but does not clobber an existing `known_peers`; refuses to overwrite an identity without `--force` and leaves the original key intact on refusal; `--force` replaces it; missing identity reports `ErrNoIdentity`; a public key file that does not match the private key is rejected; a missing public key file is tolerated and re-derived; files are 0600 with no group/other bits (skipped on Windows, ADR-0004); `known_peers` is written with LF endings and a header; corrupt files are reported with the file name and line number; atomic writes leave no `.tmp` files behind |

### `internal/cli`

| Area | Cases |
|------|-------|
| `init` (F-1) | creates all three files and prints Short ID and fingerprint; a second `init` fails with exit 1, explains itself, and leaves the existing key untouched; `--force` generates a different key |
| `whoami` (F-2) | `--json` is valid JSON whose `public_key` really does hash to the reported `fingerprint`; without an identity it exits 1 and points the user at `beam init` |
| `peers` (F-3) | empty state prints guidance, not an empty table; the table shows names and fingerprints; `--json` preserves file order and carries `added`; a corrupt `known_peers` exits 1 naming the bad line |
| `rename` (F-4) | renames; rejects unknown peers, collisions and a missing argument |
| `remove` (F-5) | the prompt shows the fingerprint; answering `n` keeps the peer; end-of-input keeps the peer; `y` removes only that peer; `--yes` skips the prompt; unknown peer exits 1 |
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
