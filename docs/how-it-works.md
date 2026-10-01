# How beam works

This page explains, step by step, what happens when two people use beam: from
generating a key, through pairing, to a file arriving intact on the other
computer. It is written for someone who can program but has not read beam's
code. Each part names the module that implements it and the decision record
(ADR, in [decisions.md](decisions.md)) that explains why it is built that way.

Contents:

1. [The big picture](#1-the-big-picture)
2. [Identity: who a device is](#2-identity-who-a-device-is)
3. [The invite: where a device is](#3-the-invite-where-a-device-is)
4. [Pairing: turning an invite into trust](#4-pairing-turning-an-invite-into-trust)
5. [Finding a paired peer again](#5-finding-a-paired-peer-again)
6. [The connection: QUIC, TLS and two protocols](#6-the-connection-quic-tls-and-two-protocols)
7. [The transfer protocol](#7-the-transfer-protocol)
8. [Resuming an interrupted transfer](#8-resuming-an-interrupted-transfer)
9. [Who sees what](#9-who-sees-what)
10. [How fast it can go, and why](#10-how-fast-it-can-go-and-why)
11. [Where everything lives in the code](#11-where-everything-lives-in-the-code)
12. [Glossary](#12-glossary)

---

## 1. The big picture

beam moves a file from one computer to another **directly**. There is no beam
server. The only other machine involved is a **relay**, and it only carries
encrypted bytes it cannot read.

```
        Alice's computer                               Bob's computer
   ┌──────────────────────┐                      ┌──────────────────────┐
   │ ~/.beam/id_ed25519   │                      │ ~/.beam/id_ed25519   │
   │ ~/.beam/known_peers  │                      │ ~/.beam/known_peers  │
   │   bob = key B        │                      │   alice = key A      │
   └──────────┬───────────┘                      └───────────┬──────────┘
              │      ① direct, peer to peer (when possible)   │
              ├══════════════════════════════════════════════►│
              │                                               │
              │   ② through the relay (when not)              │
              └──────────►  ┌────────────────────┐  ◄─────────┘
                            │ relay (n0, public) │
                            │ sees ciphertext,   │
                            │ never file contents│
                            └────────────────────┘
```

Using beam has three phases:

| Phase | Commands | Happens |
|---|---|---|
| **Identity** | `beam init` | once per computer |
| **Pairing** | `beam listen` on one side, `beam pair <invite>` on the other | once per pair of people |
| **Sending** | `beam listen` on the receiver, `beam send <name> <file>` on the sender | every time |

Four rules hold everywhere and are tested (see `requirements.md` and
`threat-model.md`):

1. The receiver accepts **every** file by hand. There is no auto-accept, not
   even when resuming.
2. Only devices in the receiver's `known_peers` can ask. Strangers are refused
   without a prompt.
3. If a peer's key changes, beam stops and warns. It never follows a key change
   by itself.
4. No home-made cryptography. beam uses Ed25519, SHA-256, SPAKE2, HMAC and TLS
   1.3, all from established libraries.

---

## 2. Identity: who a device is

`beam init` generates an **Ed25519 keypair**.

| File | Holds |
|---|---|
| `~/.beam/id_ed25519` | the private key (PKCS#8 PEM, file mode 0600). It never leaves the computer. |
| `~/.beam/id_ed25519.pub` | the public key, `ed25519 <base64> <comment>` |

Everything else about identity comes from the public key:

- **Fingerprint** = SHA-256 of the public key, shown as `SHA256:12b3eac3…`.
  This is what two people compare to be sure they are talking to each other.
- **Short ID** = 9 digits derived from the fingerprint. It is no longer used to
  find anyone (it was, when beam had a server). The pairing code is still bound
  to it inside SPAKE2 (section 4).
- **Network identity.** beam uses [iroh](https://iroh.computer) for networking,
  and in iroh a device's address *is* an Ed25519 public key (its "endpoint
  id"). beam hands iroh its own key, so **a device's beam key and its network
  identity are the same thing.** A connection to key B can only be completed
  by whoever holds B's private key. That one fact carries most of beam's
  security (ADR-0025).

Code: `src/identity/` (keys, fingerprint, Short ID, `known_peers`),
`src/transport/endpoint.rs` (the key becomes the iroh identity).

---

## 3. The invite: where a device is

A key says *who* a device is but not *where* it is on the internet. To pair, the
other side needs both, once. That is the invite (ADR-0036).

### What `beam listen` does to make one

1. **Binds a UDP socket** on port 7820 (`port` in `config.toml`). A fixed port
   keeps the invite the same from one `listen` to the next. If the port is
   taken, it picks a random one and warns.
2. **Connects to the relay** and waits up to 10 seconds for it. Through the
   relay, iroh also learns this computer's **public IP address and port** as
   the internet sees it, behind the home router.
3. **Collects addresses**: the public one from step 2 and the local ones (home
   LAN, VPN adapters such as Radmin VPN's `26.x.x.x`). IPv6 link-local
   addresses are dropped, because they only work with an interface index that
   cannot be carried to another machine. IPv4 comes first, and at most six are
   kept.
4. **Encodes the invite:**

```
beam1 + base32(
    version        1 byte
    public key     32 bytes
    relay          1 byte flag  (+ length + URL if it is not the built-in default)
    addresses      1 byte count + per address: 1 byte family, 4 or 16 bytes IP, 2 bytes port
    checksum       first 4 bytes of SHA-256 of everything above
)
```

- **base32** (lowercase `a–z`, `2–7`) means no symbols. A double-click selects
  the whole invite, and case does not matter.
- **The checksum** catches a typo or a cut-off paste, and beam says "copy it
  again". It is *not* a security feature. Security comes from the connection
  and the pairing code.
- **The default relay costs one byte** instead of its 35-character URL.
- A typical invite is 70–130 characters, depending on how many network
  interfaces the computer has.

### Why it is safe to send by chat

An invite is a **routing hint, not a password**. It contains a public key and
some IP addresses. Someone who intercepts it can try to connect, but cannot
pair without the code, and cannot pretend to be the device because they do not
have its private key. Someone who *changes* it (for example, swaps in their own
key) gets a pairing that fails, or one whose fingerprint does not match what
the other person reads out (section 4).

Code: `src/invite.rs`. Tests: `invite::tests` (format, typos, hostile input),
`tests/pairing.rs::an_invite_with_a_swapped_key_reaches_nobody_and_spends_nothing`.

---

## 4. Pairing: turning an invite into trust

Pairing makes each device store the other's public key, with both people's
informed consent. It happens once.

### Step by step

```
Bob (beam listen)                                    Alice (beam pair <invite> --name bob)
─────────────────                                    ─────────────────────────────────────
shows Invite + Pairing code 043 726
            ── invite by chat, code by voice call ──►
                                                     parses the invite (checksum, format)
                                                     asks: "Pairing code shown on the other device:"
                                                     dials Bob's key, at Bob's addresses / via relay
◄══════════ QUIC + TLS 1.3, protocol "beam/pair/1", both keys proved ══════════►
            (the code is now used up, whatever happens next)
            ◄── Start   {version, short_id, Alice's key, SPAKE2 message A, name hint}
            ──► Reply   {Bob's key, SPAKE2 message B, Bob's confirmation MAC}
            ◄── Confirm {Alice's confirmation MAC}
both screens: PAIRING REQUEST, both fingerprints
            ◄─► Decision {yes/no} from each person ("yes" typed in full)
saves Alice's key                                    saves Bob's key + where Bob is
```

### What each piece does

- **The connection proves the keys.** Before any pairing message, the TLS
  handshake proves Alice holds the private key behind her key, and Bob holds
  his. beam uses those *proved* keys (`remote_id()`), never a key written in a
  message. A message claiming a different key ends the pairing.
- **SPAKE2** (a password-authenticated key exchange, from the `spake2` crate)
  turns the six-digit code into a strong shared secret **without sending the
  code**. Someone who does not know the code gets exactly one guess per
  attempt, and learns nothing they could use to guess offline.
- **Key confirmation**: each side sends
  `HMAC-SHA256(secret, label ‖ role ‖ short_id ‖ Alice's key ‖ Bob's key)`.
  If anyone in the middle substituted a key, the two sides compute different
  values and both fail. Including the role stops a confirmation being echoed
  back, and including the Short ID stops it being reused for another device.
  The Short ID here is derived from Bob's key on both sides; Alice gets it from
  the key in the invite.
- **Two human decisions.** Each person sees both fingerprints and must type
  `yes` in full. `y` is deliberately not enough: pairing is permanent, and the
  prompt looks nothing like the file prompt. No answer within 60 seconds is a
  no.

### Pairing code rules

- Six digits, **single use**: the first connection that tries it spends it,
  right or wrong.
- `listen` makes a new one after every attempt and every 10 minutes.
- A wrong code pauses pairing for 5 seconds, then 10, doubling up to 5 minutes.
  **Three wrong codes in a row turn pairing off** until `listen` is restarted,
  while transfers from paired devices keep working. This bounds guessing to
  three tries per session.

### What gets saved

Both sides add a line to `known_peers`. The joiner (Alice) also saves where the
invite said Bob is:

```
bob  ed25519 ZlkPsCvVC8ccXvMeWhtQe8o8VZDVoYtUVrrgg7tI3Mc=  added=2026-10-01T13:50:23Z addrs=192.168.1.20:7820,202.28.63.102:37038
```

`relay=<url>` is added too, but only if Bob's relay differs from Alice's own.
Peers on the shared default then follow the default if it ever changes.

Code: `src/pairing/` (`code.rs`, `rotation.rs`, `protocol.rs`, `session.rs`),
`src/cli/pair_cmds.rs`. ADR-0026, ADR-0028, ADR-0030, ADR-0036.

---

## 5. Finding a paired peer again

Later, Alice runs `beam send bob report.pdf`. Nothing is looked up anywhere.

### What Alice dials

beam builds an iroh address from Bob's `known_peers` line
(`invite::peer_addr`):

| Part | From |
|---|---|
| Bob's key | the stored key: who must answer |
| a relay URL | Bob's `relay=` if saved, otherwise Alice's own relay |
| direct addresses | Bob's `addrs=` if saved |

### What iroh does with it

```
1. Alice ──"packet for key B"──► relay ──► Bob     (Bob stays connected to the relay
                                                     for as long as `listen` runs)
2. Through the relay, both sides learn each other's public IP:port.
3. Both send UDP packets straight at each other at the same moment.
   Each home router sees an outgoing packet first, so it lets the reply in.
   This is "hole punching".
4a. It works   → traffic moves to the direct path        → [Direct P2P]
4b. It doesn't → traffic keeps going through the relay   → [Relay]
```

At the same time, Alice tries any saved direct addresses, which is enough on
the same LAN or a shared VPN.

The relay is what makes this work across the internet without a server: it
forwards by key, so it can introduce two devices that only know each other's
keys. It stays a third party, but one that only sees encrypted traffic, and it
is needed anyway for networks where hole punching fails. Carrier-grade NAT,
common on mobile networks, is the usual example.

The progress line shows the path, and says so if it changes mid-transfer
(`Path changed: [Relay] -> [Direct P2P]`).

### When it does not work

| Situation | What Alice sees |
|---|---|
| Bob is not running `beam listen` | "bob … is not reachable", after about 20 seconds |
| Bob moved to another network or relay | the same, with: ask for Bob's current invite and run `beam pair <invite> --name bob` |
| Bob ran `beam init` again (new key) | the same, plus an SSH-style WARNING that someone could be impersonating Bob, and how to re-pair after checking the fingerprint in person |
| No relay configured and no address saved | "beam does not know where to find bob", at once |

These look alike on purpose. A new key cannot be told apart from an absent
device, because the address *is* the key. That is exactly why a changed key
can never slip in.

### Updating where a peer is

Running `beam pair <invite>` with the invite of a device **already** in
`known_peers` does not pair again. It only rewrites that device's `addrs=` and
`relay=`. No code, no network, and **never the key or the name**. A forged
invite can at most make a peer unreachable until the real invite is used.

Code: `src/invite.rs` (`peer_addr`, `remember`), `src/transport/dial.rs`,
`src/cli/net_cmds.rs`. ADR-0031, ADR-0032, ADR-0036.

---

## 6. The connection: QUIC, TLS and two protocols

Every connection is **QUIC** (from iroh): encrypted and authenticated with TLS
1.3, using the two devices' Ed25519 keys. Encryption is end to end between the
two beam processes, whether the packets travel directly or through the relay.

`beam listen` runs **one** endpoint that speaks two protocols, chosen at
connection time by name (ALPN). The version is in the name, so a beam that
speaks a different version fails the handshake instead of misreading messages.

| ALPN | Used for |
|---|---|
| `beam/pair/1` | pairing (section 4) |
| `beam/xfer/1` | file transfers (section 7) |

For a transfer, `listen` checks **the key the connection proved** against
`known_peers`:

- **Not there**: refused with `UnknownPeer`. No prompt, and the stranger learns
  nothing, not even whether `listen` is busy.
- **There**: the request is handled. If the request *claims* a different key
  than the connection proved, it is refused as a bad request (ADR-0031).
- **Only one transfer at a time.** A second sender is told in words that the
  receiver is busy.
- **Only one question on screen at a time.** A pairing request and a file
  request arriving together are asked one after the other, each with its own
  deadline (ADR-0030, ADR-0035).

A connection silent for 15 seconds is considered gone; iroh sends keep-alives
every 5 seconds while it is alive.

Code: `src/listener.rs`, `src/transport/endpoint.rs`, `src/cli/desk.rs`.

---

## 7. The transfer protocol

Inside the QUIC stream, beam speaks its own protocol, which is the same one it
used over plain TCP in M2 (ADR-0015, ADR-0016).

### Framing

```
[1 byte message type][4 bytes length, big-endian][payload]
```

Control messages are JSON. File data is binary: a 4-byte chunk index followed
by the bytes. A frame is at most **64 KiB**, and the length is checked before
any memory is allocated. A **chunk** (the unit of hashing) is **4 MiB** and
spans about 64 frames.

### The conversation

```
Sender (beam send)                                      Receiver (beam listen)
──────────────────                                      ──────────────────────
hashes the whole file (SHA-256)   "Hashing …"
TRANSFER_REQUEST {transfer_id, file_name, size,
                  chunk_size, chunk_count, file_sha256} ──►
                                                        checks: known peer? sane request?
                                                        enough free space?
                                                        PROMPT: "Incoming file … Accept? [y/N]"
                                          ◄── ACCEPT {transfer_id, have_bitmap}
                                              (or REJECT {reason}: declined, expired,
                                               unknown peer, busy, no space, bad request)
for each chunk the receiver does not have:
  CHUNK_START {index, len, sha256}                    ──►
  CHUNK_DATA  {index, bytes} × ~64                    ──►
                                                        hash matches? write it, flush,
                                                        record it in the bitmap, flush
                                          ◄── CHUNK_ACK {index}
                                              (or CHUNK_NAK → the sender resends,
                                               up to 3 attempts)
COMPLETE {transfer_id}                                ──►
                                                        re-hashes the whole file
                                          ◄── VERIFYING {done, total} (keep-alives)
                                                        matches file_sha256? move it into place
                                          ◄── COMPLETE {final_name}
"Sent 5.0 GiB to bob, saved on their side as report.pdf"
```

### The rules that make it safe

- **No file bytes before ACCEPT.** The sender's state machine refuses to send
  them. If any arrive anyway, the receiver discards everything and aborts
  (S-3, S-4).
- **Silence is a no.** An unanswered prompt expires after 60 seconds and counts
  as a reject (S-5).
- **Every chunk is verified before it is written**, and the whole file before
  it is kept (S-12).
- **Nothing is overwritten.** The file is built under
  `~/.beam/tmp/<transfer_id>/`, checked, then **renamed** into the destination.
  If the destination is on another drive, it is copied and flushed to disk
  instead, and a failed copy leaves nothing behind in the destination. An
  existing file with the same name is never replaced; the new one gets a
  number (`report (1).pdf`).
- **File names are cleaned.** Only the base name is used, so `..` and path
  separators are removed, and reserved names and right-to-left tricks are
  refused. Anything printed from the other side goes through a filter that
  removes terminal escape sequences (ADR-0017, ADR-0034).
- **A misbehaving paired peer cannot exhaust the receiver**: chunk size at most
  16 MiB, at most 2²² chunks, free space checked before anything is written,
  and a 60-second stall timeout once accepted (ADR-0033).
- **Transfer IDs are random**, and a replayed one is refused (S-11).

### The state machine

Both sides track the transfer in an explicit, unit-tested state machine
(`src/transfer/state.rs`):

```
Requested → AwaitingAccept → Connecting → Transferring → Verifying → Completed
                 │                │             │             │
                 ├─► Rejected     └─────────────┴─────────────┴─► Failed / Cancelled
                 └─► Expired
```

There is deliberately no "Interrupted" or "Reconnecting" state; see section 8.

Code: `src/transfer/` (`message.rs`, `frame.rs`, `sender.rs`, `receiver.rs`,
`state.rs`, `chunk.rs`, `storage.rs`, `paths.rs`).

---

## 8. Resuming an interrupted transfer

If a transfer breaks (Wi-Fi drops, a laptop sleeps, someone presses Ctrl+C),
the chunks already received stay in `~/.beam/tmp/<transfer_id>/`:

| File | Holds |
|---|---|
| `part` | the file being assembled |
| `hashes` | the SHA-256 each chunk should have |
| `state.json` | the **bitmap**: one bit per chunk, set once that chunk is safely on disk |
| `lock` | stops two sessions writing the same partial |

**To resume, run the same `beam send` again.** There is no `beam resume`, and
nothing reconnects by itself. The new request is matched to the partial by
**(sender's fingerprint, file SHA-256, size, chunk size)**, never by an ID the
sender chooses, so nobody can attach to someone else's partial. Then:

1. The receiver re-hashes every chunk the bitmap claims. A damaged one is
   simply fetched again.
2. **The person is asked again**, and the prompt says `(resuming)` and how much
   is already there. A resume is a new transfer, so "resuming needs a new
   Accept" is built into the design rather than being a rule to remember
   (ADR-0020).
3. ACCEPT carries the bitmap, and the sender skips the chunks marked present.

**Crash safety:** a chunk's data is flushed to disk *before* its bit is set and
flushed. After a crash, at worst a chunk that was actually written is fetched
again. Data is never claimed that is not there (D-10, ADR-0022).

Partials are kept after a decline, an expiry or a lost connection, deleted on
success or when the final hash fails, and expire after seven days.
`beam transfers` lists them, and `beam transfers --clear` deletes them.

---

## 9. Who sees what

| Party | Sees | Does not see |
|---|---|---|
| **The peer** | your public key, file name, size, contents | your private key, your other peers |
| **The relay** (n0's by default) | which keys are connected to it, which pairs talk, when, how many bytes, IP addresses | file names, contents, the pairing code, anything inside the encrypted connection |
| **Anyone who reads your invite** | your public key, your addresses, which relay you use | anything else; they cannot pair without the code |
| **Someone on the network path** | that encrypted UDP traffic flows | everything inside it |
| **n0's discovery service** | nothing: beam never uses it (`tests/no_n0_discovery.rs`) | |

Use `relay = "none"` to take n0 out of the picture entirely. Then only devices
that can reach each other directly can connect: the same LAN, a shared VPN, or
a public address. The relay and what it can see are described in detail in
[n0-data.md](n0-data.md). What an attacker can and cannot do, with the test
behind each claim, is in [threat-model.md](threat-model.md).

---

## 10. How fast it can go, and why

A transfer is as fast as the **slowest** of these:

```
speed = min( sender's upload,  receiver's download,  the path,  beam's own limits )
```

- **The sender's upload** is usually the limit on home internet, where upload
  is often much slower than download. Reverse the direction, and the other
  person's upload becomes the limit.
- **The path:** on `[Relay]`, every byte makes a detour through the relay.
  n0's public relay is shared and rate-limited, so large relayed transfers are
  slow whatever the two connections can do.
- **beam's own limits** (measured from the code, not yet changed):
  - QUIC's per-stream window is **1.25 MB** (the default of iroh's QUIC
    library). One stream can only have that much unacknowledged data in
    flight, so speed ≤ 1.25 MB ÷ round-trip time: about 125 MB/s at 10 ms, but
    about 12.5 MB/s at 100 ms.
  - **One chunk at a time.** After each 4 MiB chunk, the sender waits for
    CHUNK_ACK while the receiver hashes, writes and flushes the chunk twice.
    The line is idle for one round trip plus that disk time, every 4 MiB.
  - **Two full reads outside the progress bar**: the sender hashes the whole
    file before asking, and the receiver re-hashes it at the end.

A larger QUIC window, several chunks in flight, and fewer disk flushes are the
candidate improvements. They are to be planned and measured before any change,
because two of them touch the protocol or the crash-safety rules.

---

## 11. Where everything lives in the code

```
crates/beam/src/
  main.rs              entry point; hands the arguments to cli::execute
  cli/                 one file per group of commands
    mod.rs             the command tree (clap)
    identity_cmds.rs   init, whoami, peers, rename, remove
    pair_cmds.rs       pair <invite>, pair --wait, the address update
    net_cmds.rs        listen and send over iroh, and their messages
    transfer_cmds.rs   transfers; the hidden TCP test transport
    desk.rs            one question on screen at a time; notices redraw it
    terminal.rs        keyboard and progress output
  identity/            keys, fingerprint, Short ID, known_peers, ~/.beam store
  invite.rs            the invite format; saving and reading where a peer is
  pairing/
    code.rs            the six-digit code: single use, expiry
    rotation.rs        new codes, back-off, three strikes
    protocol.rs        SPAKE2 + key confirmation + decisions, over any stream
    session.rs         the two roles over iroh (wait / serve / join)
  listener.rs          beam listen as a service: one endpoint, two ALPNs
  transport/
    endpoint.rs        the iroh endpoint from beam's key; port and relay setup
    dial.rs            dialling a peer; following [Direct P2P]/[Relay]
  transfer/            the protocol, the state machine, chunks, partials, commit
  config.rs            ~/.beam/config.toml: relay and port
  untrusted.rs         makes text from the other side safe for the terminal
  ui.rs                formatting helpers
crates/beam/tests/     command, integration, security and two-process tests
docs/                  requirements, decisions (ADRs), test plan, threat model
```

---

## 12. Glossary

| Term | Meaning |
|---|---|
| **ALPN** | The protocol name agreed during the TLS handshake. beam uses it to tell pairing from transfers. |
| **Bitmap** | One bit per chunk, recording which chunks are safely on disk. Used for resume. |
| **Chunk** | 4 MiB of the file. The unit that is hashed, acknowledged and resumed. |
| **CGNAT** | Carrier-grade NAT: the internet provider shares one public address among many customers. Hole punching often fails behind it. |
| **Endpoint id** | iroh's name for a device's address. In beam it is the device's Ed25519 public key. |
| **Fingerprint** | SHA-256 of a public key. What people compare out loud. |
| **Frame** | At most 64 KiB on the wire: one message, or part of a chunk. |
| **Hole punching** | Both devices send packets to each other at the same time, so each router lets the other's packets in. |
| **Invite** | `beam1…`: a device's key, relay and addresses, pasted once to pair. |
| **`known_peers`** | The list of paired devices and their keys: the trust root for receiving. |
| **Partial** | A transfer that stopped part-way, kept on disk for resume. |
| **QUIC** | An encrypted transport over UDP, used by iroh. |
| **Relay** | A public server that forwards encrypted traffic by key, and helps devices find a direct path. |
| **Short ID** | 9 digits derived from the fingerprint, bound into pairing. |
| **SPAKE2** | A password-authenticated key exchange: proves both sides know the code without sending it. |
