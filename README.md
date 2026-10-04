# beam

Identity-based peer-to-peer file transfer for the terminal.

beam sends a file straight from one computer to another. There is no server
to set up: two devices meet once through an *invite* you paste to the other
person, and find each other again by key through a relay that only ever carries
encrypted data. Every incoming transfer has to be accepted by hand, and only
peers you have paired with can ask.

How it all fits together — invites, pairing, finding a peer, hole punching,
the transfer protocol — is explained step by step in
[docs/how-it-works.md](docs/how-it-works.md). What beam protects, what it does
not, how to use it safely and how to report a problem are in
[SECURITY.md](SECURITY.md).

> **Status: milestone M6.** Pairing and file transfer run over iroh: encrypted
> end to end, direct when possible and through a relay when not, with each
> device proving its key on every connection. The threat model —
> [docs/threat-model.md](docs/threat-model.md) — says what an attacker can and
> cannot do, and names the test behind each claim.

## Build

Requires Rust 1.91 or newer (edition 2024). On Windows you also need the MSVC
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

### Installing on Windows, so `beam` works without `.\beam.exe`

Double-click `scripts\install.bat`, or run it from a terminal. It builds beam
in release mode and copies it to `%LOCALAPPDATA%\Programs\beam`, then adds
that folder to your user PATH (no administrator rights needed). Open a new
terminal afterwards and type `beam`. Run it again after pulling changes to
update. `scripts\install.bat -Uninstall` removes it. Neither touches
`~/.beam`, where your key and paired peers live.

To give beam to someone without Rust, put `beam.exe` (from
`target\release`) in a folder with `install.bat` and `install.ps1`.
The script then installs that `beam.exe` instead of building one.

(A `.bat` wrapper alone would not help: Windows finds a command without a
path only in the folders on PATH, and PowerShell never looks in the current
folder. So the real fix is putting beam's folder on PATH.)

## Use

```
beam init                      # generate this device's keypair (once per machine)
beam whoami                    # show your fingerprint
beam peers                     # list paired peers
beam rename alice ali          # change a local nickname
beam remove alice              # forget a peer

beam listen                    # wait for transfers and pairing: shows invite + code
beam send alice project.zip    # send a file to a paired peer
beam transfers                 # list partly received transfers
beam transfers --clear         # discard them

beam pair <INVITE> --name bob  # pair with a listening device, typing its code
beam pair --wait --name alice  # pair without also receiving files

beam service enable            # optional: receive in the background, from login
beam inbox                     # accept or decline what the background agent holds
beam service status|stop       # see or stop the background agent
beam receive-dir <folder>      # where received files go
```

The background agent is optional. With it on, paired devices can send while
`beam listen` is closed: you get a notification and answer in `beam inbox`.
It never accepts anything by itself, and it does not pair. See
[docs/background-services.md](docs/background-services.md).

`listen` saves into the current directory unless given `--out <dir>`. Its
pairing code works once and changes every ten minutes. It is shown once, when
`listen` starts; `beam whoami` in another terminal shows the current one. After three wrong codes
in a row it stops offering pairing until it is restarted.

Global flags: `--beam-dir <path>` (default `$BEAM_DIR`, else `~/.beam`) and
`--json` for machine-readable output.

There is nothing to configure. `~/.beam/config.toml` exists only to change the
defaults:

```toml
relay = "https://aps1-1.relay.n0.iroh.link./"  # the default; or "none"
port  = 7820                                   # UDP port `listen` uses; 0 = random
```

With `relay = "none"`, only devices that can reach each other directly work: the
same LAN, a shared VPN (Radmin VPN, ZeroTier, Tailscale…), or a public address.
To test real direct P2P between two homes, follow the step-by-step guide in
[docs/deploy.md](docs/deploy.md); `advertise = ["<public IP>:7820"]` puts a
hand-forwarded public address in the invite.

beam never uses n0's discovery service; the relay is the only n0 infrastructure
it touches, and only if you leave the default. See [docs/n0-data.md](docs/n0-data.md).

## Trying it on one machine

Both devices can live on one machine: give each its own beam home with
`BEAM_DIR`. You need two terminals, one for each device.

**Set up, once.**

```bash
mkdir -p /tmp/beam-demo/{alice,bob,inbox}
cd /tmp/beam-demo
BEAM_DIR=$PWD/alice beam init
BEAM_DIR=$PWD/bob   beam init
head -c 5M /dev/urandom > payload.bin      # something worth sending
```

Offline, the default relay cannot be reached and beam spends ten seconds
finding that out each time it starts. To skip it, put `relay = "none"` in
`alice/config.toml` and `bob/config.toml`.

**Terminal 1 — bob listens.** One command waits for both pairing and files:

```bash
cd /tmp/beam-demo && BEAM_DIR=$PWD/bob beam listen --out $PWD/inbox
```

```
  Invite        beam1ahqv23dmcwi4bjymuatwbgggklfxpjyrfft7ccbcr7ypgrrwt43dwaicatakqaiud2gajsyaoed2fkjcxgtwg
  Pairing code  685 821
  Fingerprint   SHA256:10a44acf76456739...
  Relay         https://aps1-1.relay.n0.iroh.link./
  Saving to     /tmp/beam-demo/inbox

To pair a new device, send it the invite and run on it:  beam pair <invite> --name <a name for this one>
The pairing code works once and changes every 10 minutes; `beam whoami` shows the current one.
Waiting for transfers. Every one has to be accepted by hand. Ctrl+C to stop.
```

The invite holds bob's public key, his relay and his addresses. Send it to the
other person any way you like — chat, email, LINE. It is not a secret: it only
says where bob is, and pairing still needs the code and both people saying yes.
With `port` fixed (the default), it stays the same each time `listen` starts.

**Terminal 2 — alice pairs with bob**, pasting the invite from terminal 1, and
types the code when asked:

```bash
cd /tmp/beam-demo && BEAM_DIR=$PWD/alice beam pair beam1ahqv23dm... --name bob
```

Both terminals then show the pairing question. It looks deliberately unlike the
file prompt, because pairing is permanent:

```
================================================================
  PAIRING REQUEST - this is permanent
================================================================
A device that knows your pairing code wants to pair with you.
  Save as       alices-laptop
  Their key     SHA256:390f55e08994ce1e...
  Your key      SHA256:10a44acf76456739...

Once paired, alices-laptop can send you files (each one still needs your Accept)
until you run `beam remove alices-laptop`. Check that the other screen shows
the same two fingerprints, the other way round.
Type "yes" to pair, anything else to refuse:
```

Type `yes` in both — `y` is not enough here. bob saves alice under the name her
machine suggested; `beam rename` changes it.

**Then send.** Still in terminal 2:

```bash
BEAM_DIR=$PWD/alice beam send bob payload.bin
```

Terminal 1 asks, and nothing moves until you answer:

```
Incoming file
  From          alices-laptop
  Fingerprint   SHA256:390f55e08994ce1e...
  File          payload.bin
  Size          5.0 MiB
Accept? [y/N]:
```

Type `y`. Terminal 2 shows the path as it goes — `[Direct P2P]` here, `[Relay]`
when the two devices cannot reach each other directly — and says if it changes
mid-transfer. Check that what arrived is what left:

```bash
sha256sum payload.bin inbox/payload.bin    # the two hashes must match
```

<details>
<summary><strong>The same in PowerShell</strong></summary>

Every command below was run on Windows PowerShell 5.1.

```powershell
$demo = "$env:USERPROFILE\beam-demo"
New-Item -ItemType Directory -Force -Path "$demo\alice", "$demo\bob", "$demo\inbox" | Out-Null
Set-Location $demo
$env:BEAM_DIR = "$demo\alice"; beam init
$env:BEAM_DIR = "$demo\bob";   beam init

# 5 MiB of something worth sending
$bytes = New-Object byte[] (5MB)
(New-Object Random 1).NextBytes($bytes)
[System.IO.File]::WriteAllBytes("$demo\payload.bin", $bytes)
```

**Terminal 1 — bob listens.**

```powershell
$demo = "$env:USERPROFILE\beam-demo"
$env:BEAM_DIR = "$demo\bob"
beam listen --out "$demo\inbox"
```

**Terminal 2 — alice pairs, then sends.** Paste the invite from terminal 1 and
type the code when asked; type `yes` in both terminals at the pairing question.

```powershell
$demo = "$env:USERPROFILE\beam-demo"
$env:BEAM_DIR = "$demo\alice"
beam pair <bob's invite> --name bob
beam send bob "$demo\payload.bin"
```

Answer the file prompt in terminal 1 with `y`, then check the two hashes match:

```powershell
Get-FileHash "$demo\payload.bin" -Algorithm SHA256
Get-FileHash "$demo\inbox\payload.bin" -Algorithm SHA256
```

A `config.toml` written with PowerShell 5.1's `Set-Content -Encoding utf8` gets
a byte-order mark; beam skips it. The table below applies unchanged.

</details>

**Things worth trying, and what should happen.**

| Try this | What should happen |
|---|---|
| Answer the pairing question with `y` | Not paired. Only `yes` pairs |
| Type a wrong pairing code | Both sides fail, nobody is asked, nothing is saved. `listen` pauses pairing for 5 s, then shows a new code |
| Type three wrong codes in a row | `listen` turns pairing **off** until it is restarted, and says someone may be guessing. Files from paired devices still arrive |
| Leave `listen` running for ten minutes | Nothing is printed, but `beam whoami` shows a new code; the old one no longer works, and a joiner who types it is told the code may have expired |
| Run `beam whoami` while `listen` runs, and after stopping it | It shows `listen`'s invite, the current code and when it expires; afterwards, that `listen` is not running |
| Answer a file with `n` | Both sides say it was declined, and `inbox/` gains nothing |
| Answer nothing for 60 seconds | Both sides say it expired. Silence is a Reject, not a maybe |
| Send twice at once, from two paired devices | The second sender is told `bob is receiving another file; try again later` |
| Start pairing while a file prompt is open | The pairing question waits until the file question is answered — one question on screen at a time — and still expires 60 s after it arrived |
| Pipe the answer: `echo y \| beam listen ...` | It does **not** work, on purpose. Each question discards anything typed before it appeared |
| `beam remove alices-laptop` on bob, then send from alice | Bob refuses without a prompt, and alice is told bob does not recognise her key and how to re-pair |
| Run `beam init --force` on bob, then send from alice | alice is told bob is not reachable — not listening, moved, or re-initialised — and how to update the address or re-pair |
| Run `beam pair <bob's invite> --name bob` again on alice | Nothing is asked: alice only updates where to find bob. His key is never changed this way |
| Kill the sender mid-transfer with Ctrl+C, then send again | `listen` notices within 15 s. The second run says `(resuming)` and `Already have`, sends only the rest, and the finished file still matches |
| Start a second `listen` while one runs | It warns that port 7820 is taken and uses another; its invite still works |

## Resuming

If a transfer stops part-way — the link drops, a laptop closes, somebody hits
Ctrl+C — what already arrived is kept. **Send the same file again and it carries
on from where it stopped.**

```
beam send bob big.iso     # interrupted at 40%
beam send bob big.iso     # continues from 40%
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

Since M5 the transfer runs over iroh's QUIC: **encrypted end to end**, and each
side **proves** the key it claims before a byte moves — the receiver identifies
the sender by the key the connection proved, not by what the request says. A
relay, if one is used, carries ciphertext.

What is still true, and worth knowing:

- **The relay knows when you are online.** While `beam listen` runs, it is
  connected to the relay under its public key, and the relay sees that two
  keys talk, when, and how much — not what. See [docs/n0-data.md](docs/n0-data.md).
- **An invite shows your addresses** to whoever reads it. It cannot make anyone
  pair with the wrong device: the connection proves the key, and pairing needs
  the code and both fingerprints confirmed.
- **Pairing trusts the fingerprint check.** Someone who learns your code and
  connects first reaches the pairing question as themselves. That is why the
  question shows both fingerprints and needs `yes` typed in full.
- **Anything a peer or server sends is shown safely**: escape sequences are
  removed, invisible direction and joining characters are shown as
  `<U+XXXX>`, and a long file name is cut in the middle so its real extension
  stays visible. A name that uses a right-to-left override to disguise itself
  is refused.

The full list — assets, attackers, threats, the test for each mitigation, and
the risks accepted on purpose — is [docs/threat-model.md](docs/threat-model.md).

What has tests that try to break it: every transfer is accepted by hand —
including every resume — and no flag or config can skip the prompt; pairing
needs `yes` on both devices; unknown senders are refused without a prompt; a
sender claiming someone else's key is refused; data arriving before ACCEPT ends
the transfer; every chunk is verified before it is written and re-verified
before it is reused; the whole file is verified before it is saved; an existing
file is never overwritten; a partial transfer belongs to the one peer it came
from; and a pairing code allows three guesses per `listen` session at most.

## Files

Everything lives in `~/.beam/`:

```
id_ed25519        private key, PEM-wrapped PKCS#8, mode 0600 — never leaves this device
id_ed25519.pub    ed25519 <base64 key> <comment>
known_peers       one peer per line; the trust root for receiving
config.toml       optional: the relay, and the port `listen` uses
tmp/<id>/         a transfer in progress: state.json, part, hashes, lock
```

A received file is built under `tmp/`, verified against the SHA-256 the sender
committed to before you accepted, and only then moved into place — by a rename
when it can, by a copy when the destination is on another drive.

Inside a `tmp/<id>/` directory, `state.json` records which chunks have arrived
and `hashes` records what each one should be. The bitmap in `state.json` is
always written **after** the chunk data, and the disk is flushed every 8 chunks.
The bitmap is a claim, not proof: on resume every claimed chunk is re-hashed,
so one lost to a power cut is fetched again instead of being trusted.

`known_peers` is plain text and safe to read:

```
# beam known_peers v1
alice  ed25519 4V1sbBWRwKcMoCdgmMZSy3enESln8Qgij/DzRjafNjs=  added=2026-09-24T12:00:00Z addrs=192.168.1.20:7820
```

`addrs=` (and `relay=`, when a peer uses a different relay) is where the peer's
invite said it can be found. Only *where* is ever updated this way, never the key.

Comments, blank lines and attributes beam does not recognise survive edits. A
malformed line is a hard error naming the line number — a peer entry is never
silently dropped.

## Identifiers

**Fingerprint** — `SHA256:` plus the SHA-256 of your public key. This is the
thing to compare out of band, and what both screens show when pairing.

**Invite** — `beam1…`, the public key, relay and direct addresses of a device
waiting to pair, for pasting to the other person once. It is a routing hint, not
a security guarantee: security comes from the PAKE during pairing and from the
stored public key afterwards.

**Short ID** — 9 digits derived from the fingerprint. The pairing code is bound
to it inside the PAKE.

## The rules beam will not bend

- Every transfer is accepted by hand. There is no auto-accept flag, config
  setting, or trusted-peer bypass, and resuming an interrupted transfer needs a
  new Accept.
- Only peers in your `known_peers` may ask to send. Unknown senders are rejected
  without a prompt.
- If a peer's key changes, beam aborts with a warning and never updates the
  stored key by itself. You re-pair, deliberately.
- Pairing saves nothing until a person on each device has compared both
  fingerprints and answered yes. A pairing code works for one attempt.

## Layout

```
crates/beam/
  src/main.rs          CLI entry point
  src/cli/             command definitions
  src/identity/        keys, fingerprints, Short IDs, known_peers, store
  src/pairing/         pairing codes, SPAKE2 + key confirmation, the two roles
  src/invite.rs        invites, and where a paired peer is found
  src/transfer/        protocol, state machine, chunking, resume, integrity
  src/transport/       iroh endpoint; the M2 TCP stand-in
  src/config.rs        ~/.beam/config.toml
  src/ui.rs            terminal output helpers
  tests/               command, integration and two-process tests
docs/                  requirements, design decisions, test plan
```

Transfer speed, what has been measured and the plan to improve it:
[docs/performance-plan.md](docs/performance-plan.md).

The project was originally written in Go; see ADR-0010 in
[docs/decisions.md](docs/decisions.md) for why it moved to Rust and what that
changed. The Go implementation is preserved in commit `a1ae4ec`.

See [docs/requirements.md](docs/requirements.md),
[docs/decisions.md](docs/decisions.md) and [docs/test-plan.md](docs/test-plan.md).
