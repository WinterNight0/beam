# beam

Identity-based peer-to-peer file transfer for the terminal.

beam sends a file straight from one computer to another. A small signaling
server only helps two peers find each other — it never stores your files. Every
incoming transfer has to be accepted by hand, and only peers you have paired
with can ask.

> **Status: milestone M2.** Identity management and file transfer work over a
> TCP address you give by hand. Pairing (`pair`, `newcode`) is stubbed and exits
> with code 2. There is no encryption yet and the sender's identity is only
> claimed, not proven — see [What M2 does not protect you from](#what-m2-does-not-protect-you-from).

## Build

Requires Rust 1.88 or newer (edition 2024). On Windows you also need the MSVC
build tools, which `rustup` will point you at.

```
cargo build --release      # binaries in ./target/release
cargo run -p beam -- --help
make check                 # cargo fmt --check, clippy -D warnings, cargo test
```

On Windows without `make`:

```
powershell -ExecutionPolicy Bypass -File scripts\check.ps1
```

## Use

```
beam init                      # generate this device's keypair (once per machine)
beam whoami                    # show your Short ID and fingerprint
beam peers                     # list paired peers
beam rename alice ali          # change a local nickname
beam remove alice              # forget a peer

beam listen                    # wait for transfers
beam send alice project.zip    # send a file to a paired peer

beam pair <ID> --name alice    # first-time pairing          (M4)
beam newcode                   # regenerate the pairing code (M4)
```

Until M4 brings peer discovery, `listen` and `send` need a TCP address. That is
scaffolding, so the flag is hidden:

```
beam listen --addr 127.0.0.1:7777 --out ~/Downloads
beam send alice project.zip --addr 127.0.0.1:7777
```

`listen` binds `127.0.0.1:7777` by default and saves into the current directory.

Global flags: `--beam-dir <path>` (default `$BEAM_DIR`, else `~/.beam`) and
`--json` for machine-readable output.

## Trying it, with two terminals

Both peers can live on one machine. Give each its own `~/.beam` with
`--beam-dir`, and pair them by hand — `beam pair` arrives in M4.

**Set up, once.** In any terminal:

```bash
mkdir -p /tmp/beam-demo/{alice,bob,inbox}
cd /tmp/beam-demo

BEAM_DIR=$PWD/alice beam init
BEAM_DIR=$PWD/bob   beam init

# Each side stores the other's public key, which is what `beam pair` will
# automate in M4.
alice_key=$(BEAM_DIR=$PWD/alice beam whoami --json | grep -o '"public_key": "[^"]*' | cut -d'"' -f4)
bob_key=$(BEAM_DIR=$PWD/bob     beam whoami --json | grep -o '"public_key": "[^"]*' | cut -d'"' -f4)

printf '# beam known_peers v1\nalice  ed25519 %s  added=2026-01-01T00:00:00Z\n' "$alice_key" > bob/known_peers
printf '# beam known_peers v1\nbob    ed25519 %s  added=2026-01-01T00:00:00Z\n' "$bob_key"   > alice/known_peers

head -c 5M /dev/urandom > payload.bin      # something worth sending
```

On Windows use `$env:BEAM_DIR` and `%USERPROFILE%\beam-demo` instead; the
commands are otherwise identical.

**Terminal 1 — the receiver.**

```bash
cd /tmp/beam-demo
BEAM_DIR=$PWD/bob beam listen --addr 127.0.0.1:7777 --out $PWD/inbox
```

**Terminal 2 — the sender.**

```bash
cd /tmp/beam-demo
BEAM_DIR=$PWD/alice beam send bob payload.bin --addr 127.0.0.1:7777
```

Terminal 1 now shows the prompt, and nothing moves until you answer it:

```
Incoming file
  From          alice
  Fingerprint   SHA256:dd0ab8817907e2c7...
  File          payload.bin
  Size          5.0 MiB
Accept? [y/N]:
```

Type `y`. Check that what arrived is what left:

```bash
sha256sum payload.bin inbox/payload.bin    # the two hashes must match
```

**Things worth trying, and what should happen.**

| Try this | What should happen |
|---|---|
| Answer `n` | Both sides say the transfer was declined, and `inbox/` gains nothing |
| Answer nothing for 60 seconds | Both sides say it expired. Silence is a Reject, not a maybe |
| Send the same file twice, answering `y` both times | The second is saved as `payload (1).bin`, and both terminals say so. The first is never overwritten |
| Pipe the answer: `echo y \| beam listen ...` | It does **not** work, on purpose. Each prompt discards anything typed before it appeared, so you cannot pre-answer a question you have not seen |
| Delete `alice` from `bob/known_peers`, then send | Bob refuses without showing a prompt at all, and Alice is told the peer has not paired with her |
| Interrupt the sender mid-transfer with Ctrl+C | The transfer fails and `inbox/` gains nothing. Resuming rather than starting over is M3 |
| `beam listen --addr 0.0.0.0:7777` | It works, and prints a warning explaining why you should not |

## What M2 does not protect you from

M2 is the transfer engine, not the security model. Two things are missing, and
both arrive later:

- **There is no encryption.** Anyone who can see the network path can read the
  file. WebRTC brings DTLS in M5.
- **The sender's identity is claimed, not proven.** The receiver checks the
  public key in a request against its own `known_peers`, but nothing checks that
  the sender holds the matching private key. A public key is public: anyone who
  has seen one can put it in a request. The Noise KK handshake closes this in M6.

So: use `127.0.0.1` for now. `beam listen` warns when you bind anywhere else,
and that warning is worth reading rather than dismissing. See ADR-0018 and
ADR-0019 in [docs/decisions.md](docs/decisions.md).

What *does* hold in M2, and has tests that try to break it: every transfer is
accepted by hand, no flag or config can skip the prompt, unknown senders are
refused without a prompt, data arriving before ACCEPT ends the transfer, every
chunk is verified before it is written, the whole file is verified before it is
saved, and an existing file is never overwritten.

## Files

Everything lives in `~/.beam/`:

```
id_ed25519        private key, PEM-wrapped PKCS#8, mode 0600 — never leaves this device
id_ed25519.pub    ed25519 <base64 key> <comment>
known_peers       one peer per line; the trust root for receiving
tmp/<transfer_id>/  a file being assembled, removed when the transfer ends
```

A received file is built under `tmp/`, verified against the SHA-256 the sender
committed to before you accepted, and only then moved into place. A transfer
that fails at any point leaves nothing behind.

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
crates/beam/
  src/main.rs          CLI entry point
  src/cli/             command definitions
  src/identity/        keys, fingerprints, Short IDs, known_peers, store
  src/ui.rs            terminal output helpers
  tests/cli.rs         command-level tests
crates/beam-server/    signaling server (M4)
docs/                  requirements, design decisions, test plan
```

The project was originally written in Go; see ADR-0010 in
[docs/decisions.md](docs/decisions.md) for why it moved to Rust and what that
changed. The Go implementation is preserved in commit `a1ae4ec`.

See [docs/requirements.md](docs/requirements.md),
[docs/decisions.md](docs/decisions.md) and [docs/test-plan.md](docs/test-plan.md).
