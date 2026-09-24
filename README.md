# beam

Identity-based peer-to-peer file transfer for the terminal.

beam sends a file straight from one computer to another. A small signaling
server only helps two peers find each other — it never stores your files. Every
incoming transfer has to be accepted by hand, and only peers you have paired
with can ask.

> **Status: milestone M3.** Identity, file transfer and resume work over a TCP
> address you give by hand. Pairing (`pair`, `newcode`) is stubbed and exits
> with code 2. There is no encryption yet and the sender's identity is only
> claimed, not proven — see [What this does not protect you from](#what-this-does-not-protect-you-from).

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
beam transfers                 # list partly received transfers
beam transfers --clear         # discard them

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

<details>
<summary><strong>The same setup in PowerShell</strong></summary>

Every command below was run on Windows PowerShell 5.1.

```powershell
$demo = "$env:USERPROFILE\beam-demo"
New-Item -ItemType Directory -Force -Path "$demo\alice", "$demo\bob", "$demo\inbox" | Out-Null
Set-Location $demo

$env:BEAM_DIR = "$demo\alice"; beam init
$env:BEAM_DIR = "$demo\bob";   beam init

# Each side stores the other's public key, which `beam pair` will automate in M4.
$env:BEAM_DIR = "$demo\alice"; $aliceKey = (beam whoami --json | ConvertFrom-Json).public_key
$env:BEAM_DIR = "$demo\bob";   $bobKey   = (beam whoami --json | ConvertFrom-Json).public_key

"# beam known_peers v1`nalice  ed25519 $aliceKey  added=2026-01-01T00:00:00Z" |
    Set-Content "$demo\bob\known_peers" -Encoding utf8
"# beam known_peers v1`nbob  ed25519 $bobKey  added=2026-01-01T00:00:00Z" |
    Set-Content "$demo\alice\known_peers" -Encoding utf8

# 5 MiB of something worth sending
$bytes = New-Object byte[] (5MB)
(New-Object Random 1).NextBytes($bytes)
[System.IO.File]::WriteAllBytes("$demo\payload.bin", $bytes)

beam peers        # should list the other side
```

`Set-Content -Encoding utf8` writes a byte-order mark on PowerShell 5.1. beam
ignores a leading BOM in `known_peers` for exactly this reason, so the natural
command works rather than failing with a baffling parse error on line 1.

**Terminal 1 — the receiver.**

```powershell
$demo = "$env:USERPROFILE\beam-demo"
$env:BEAM_DIR = "$demo\bob"
beam listen --addr 127.0.0.1:7777 --out "$demo\inbox"
```

**Terminal 2 — the sender.**

```powershell
$demo = "$env:USERPROFILE\beam-demo"
$env:BEAM_DIR = "$demo\alice"
beam send bob "$demo\payload.bin" --addr 127.0.0.1:7777
```

Answer the prompt in terminal 1 with `y`, then check the two hashes match:

```powershell
Get-FileHash "$demo\payload.bin" -Algorithm SHA256
Get-FileHash "$demo\inbox\payload.bin" -Algorithm SHA256
```

The table of things to try below applies unchanged; only the shell differs.

</details>

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
| Interrupt the sender mid-transfer with Ctrl+C, then send the same file again | The second run says `(resuming)` and `Already have`, sends only the rest, and the finished file still matches. `beam transfers` shows the partial in between |
| Interrupt it, then answer `n` | The partial survives; `beam transfers` still lists it, and sending again picks it up |
| Interrupt it, then edit `payload.bin` and send again | It starts from zero, because it is now a different file |
| `beam listen --addr 0.0.0.0:7777` | It works, and prints a warning explaining why you should not |

## Resuming

If a transfer stops part-way — the link drops, a laptop closes, somebody hits
Ctrl+C — what already arrived is kept. **Send the same file again and it carries
on from where it stopped.**

```
beam send bob big.iso --addr 127.0.0.1:7777     # interrupted at 40%
beam send bob big.iso --addr 127.0.0.1:7777     # continues from 40%
```

There is no `beam resume` and nothing reconnects by itself. Running `send` again
is the whole interface.

A resume is a new transfer that happens to find data on disk, so **it is
accepted like any other**, and the prompt says what it is:

```
Incoming file (resuming)
  From          alice
  Fingerprint   SHA256:dd0ab8817907e2c7...
  File          big.iso
  Size          4.0 GiB
  Already have  1.6 GiB (40%), from 2 hours ago
Accept? [y/N]:
```

Saying no keeps what you already have, so you can accept it later. What is
waiting:

```
beam transfers
ID        FILE      SIZE     HAVE  UPDATED
3f9a1c22  big.iso   4.0 GiB  40%   2 hours ago

beam transfers --clear            # discard everything, after confirming
beam transfers --clear 3f9a1c22   # discard one
```

Partials are dropped after seven days without use, tidied up when `beam listen`
starts.

A few things worth knowing:

- **Edit the file and it starts over.** The transfer is identified by the file's
  SHA-256, so a changed file is a different transfer. The old partial is left
  alone rather than quietly mixed in.
- **What is on disk is re-checked, not assumed.** Every chunk already held is
  re-hashed before the sender is told about it. A chunk damaged since last time
  is simply fetched again.
- **A partial belongs to one peer.** It is matched on the sender's fingerprint
  as well as the file, so nobody else can attach to it or learn it exists.
- **One session at a time.** A partial is locked while it is in use; a second
  `beam listen` receiving the same file is told so rather than both writing.
- **Space is checked before you are asked**, so you are not interrupted to agree
  to something that cannot finish. Only the missing bytes have to fit.

## What this does not protect you from

This is the transfer engine, not the security model. Two things are missing,
and both arrive later:

- **There is no encryption.** Anyone who can see the network path can read the
  file. WebRTC brings DTLS in M5.
- **The sender's identity is claimed, not proven.** The receiver checks the
  public key in a request against its own `known_peers`, but nothing checks that
  the sender holds the matching private key. A public key is public: anyone who
  has seen one can put it in a request. The Noise KK handshake closes this in M6.

So: use `127.0.0.1` for now. `beam listen` warns when you bind anywhere else,
and that warning is worth reading rather than dismissing. See ADR-0018 and
ADR-0019 in [docs/decisions.md](docs/decisions.md).

What *does* hold, and has tests that try to break it: every transfer is
accepted by hand — including every resume — and no flag or config can skip the
prompt; unknown senders are refused without a prompt; data arriving before
ACCEPT ends the transfer; every chunk is verified before it is written and
re-verified before it is reused; the whole file is verified before it is saved;
an existing file is never overwritten; and a partial transfer belongs to the one
peer it came from.

## Files

Everything lives in `~/.beam/`:

```
id_ed25519        private key, PEM-wrapped PKCS#8, mode 0600 — never leaves this device
id_ed25519.pub    ed25519 <base64 key> <comment>
known_peers       one peer per line; the trust root for receiving
tmp/<id>/         a transfer in progress: state.json, part, hashes, lock
```

A received file is built under `tmp/`, verified against the SHA-256 the sender
committed to before you accepted, and only then moved into place — by a rename
when it can, by a copy when the destination is on another drive.

Inside a `tmp/<id>/` directory, `state.json` records which chunks have arrived
and `hashes` records what each one should be. The bitmap in `state.json` is
always written **after** the chunk data has been flushed, so a crash loses the
claim rather than the data: the chunk is fetched again instead of being trusted
when it should not be.

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
