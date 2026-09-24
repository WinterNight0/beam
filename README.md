# beam

Identity-based peer-to-peer file transfer for the terminal.

beam sends a file straight from one computer to another. A small signaling
server only helps two peers find each other — it never stores your files. Every
incoming transfer has to be accepted by hand, and only peers you have paired
with can ask.

> **Status: milestone M1.** Identity management works. Pairing and transfer
> (`pair`, `listen`, `send`, `newcode`) are stubbed and exit with code 2.

## Build

Requires Go 1.24 or newer.

```
make build            # binaries in ./bin
make check            # gofmt, go vet, go test
go run ./cmd/beam --help
```

On Windows without `make`:

```
go build -o bin\ ./cmd/...
powershell -ExecutionPolicy Bypass -File scripts\check.ps1
```

## Use

```
beam init                      # generate this device's keypair (once per machine)
beam whoami                    # show your Short ID and fingerprint
beam peers                     # list paired peers
beam rename alice ali          # change a local nickname
beam remove alice              # forget a peer

beam listen                    # wait for transfers          (M2)
beam pair <ID> --name alice    # first-time pairing          (M4)
beam send alice project.zip    # send a file to a peer       (M2)
beam newcode                   # regenerate the pairing code (M4)
```

Global flags: `--beam-dir <path>` (default `$BEAM_DIR`, else `~/.beam`) and
`--json` for machine-readable output.

## Files

Everything lives in `~/.beam/`:

```
id_ed25519        private key, PEM-wrapped PKCS#8, mode 0600 — never leaves this device
id_ed25519.pub    ed25519 <base64 key> <comment>
known_peers       one peer per line; the trust root for receiving
tmp/              in-progress transfers (from M2)
```

`known_peers` is plain text and safe to read:

```
# beam known_peers v1
alice  ed25519 4V1sbBWRwKcMoCdgmMZSy3enESln8Qgij/DzRjafNjs=  added=2026-09-24T12:00:00Z
```

Comments, blank lines and attributes beam does not recognise survive edits. A
malformed line is a hard error naming the line number — a peer entry is never
silently dropped.

## Identifiers

**Fingerprint** — `SHA256:` plus the SHA-256 of your public key. This is the
thing to compare out of band, and what the server routes by.

**Short ID** — 9 digits derived from the fingerprint, for reading aloud during
the very first pairing. It is a lookup hint, not a security guarantee: security
comes from the PAKE during pairing and from the stored public key afterwards.

## The rules beam will not bend

- Every transfer is accepted by hand. There is no auto-accept flag, config
  setting, or trusted-peer bypass, and resuming an interrupted transfer needs a
  new Accept.
- Only peers in your `known_peers` may ask to send. Unknown senders are rejected
  without a prompt.
- If a peer's key changes, beam aborts with a warning and never updates the
  stored key by itself. You re-pair, deliberately.

## Layout

```
cmd/beam/            CLI entry point
cmd/beam-server/     signaling server (M4)
internal/cli/        command definitions
internal/identity/   keys, fingerprints, Short IDs, known_peers
internal/ui/         terminal output helpers
docs/                requirements, design decisions, test plan
```

See [docs/requirements.md](docs/requirements.md),
[docs/decisions.md](docs/decisions.md) and [docs/test-plan.md](docs/test-plan.md).
