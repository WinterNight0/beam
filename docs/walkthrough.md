# Trying beam on one machine

A hands-on walkthrough: two devices, alice and bob, on one computer, each in its
own terminal. It shows pairing, the Accept prompt and a transfer end to end, then
lists things worth trying and what each should do. For everyday use, see the
[README](../README.md).

You need `beam` on your PATH (see the README's *Install*), or replace `beam`
below with the path to the binary.

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

## Things worth trying, and what should happen

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
