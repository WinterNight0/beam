# SPIKE-001 — P2P transport library

**Run:** 2026-09-24 · **Status:** complete, awaiting a decision · **Timebox:** 2 days

Candidates: `webrtc-rs`, `str0m`, `iroh`.

**Recommendation: iroh.** The reasoning is below, along with the two things
that argue against it and what they would cost.

---

## Summary

| | webrtc-rs | str0m | **iroh** |
|---|---|---|---|
| Latest release | 0.21.0 (2026-09-19) | 0.23.1 (2026-08-21) | **1.2.0** (2026-09-09) |
| Reached 1.0 | no, after 8 years | no | **yes, 2026-06-15** |
| Commits (3 months) | 100+ , **89 by one person** | 55, 20 authors | 88, 17 authors |
| Open issues | 8 | 36 | 175 |
| NAT traversal | ICE; we supply STUN/TURN | ICE only; we supply everything | hole punching **+ relay built in** |
| Relay for CGNAT | self-host coturn | self-host coturn | **4 defaults incl. Asia-Pacific**, self-hostable |
| Our Ed25519 key as identity | no | no | **yes, same type** |
| Fits `AsyncRead + AsyncWrite` | no, message API | no, sans-IO | **yes, directly** |
| Direct dependencies pulled | 172 | 87 | 246 |
| Clean release build | 107 s | 163 s | 181 s |
| Binary cost over beam alone | not measured | not measured | **+11 MiB** |
| Prototype built and run | no | no | **yes** |

Measurements are from this project's Windows 11 machine (Rust 1.98.1, MSVC).
Dependency counts come from `cargo tree` on a crate that depends only on the
library and tokio.

---

## 1. Maintenance

**webrtc-rs** is the port of Pion named in the original brief. It is alive —
last push 2026-09-20 — but two things stand out. It has been pre-1.0 for eight
years, and **89 of the last 100 commits are by a single person**. It is also
mid-rewrite: `0.21.0` sits on a new sans-IO `rtc` core, and the version history
for August and September 2026 reads `alpha.2, beta.1, beta.2, rc.1, rc.2,
0.21.0`. Adopting it means adopting a library in the middle of changing shape,
carried largely by one maintainer.

**str0m** is healthier per-capita — 20 authors on 55 commits — but small (627
stars) and moving fast in a way that costs us: `0.18 → 0.23` in five months, and
pre-1.0 minors are breaking by convention. Each bump would be a small migration.

**iroh** reached **1.0 in June 2026** and is on 1.2 now. That is the only
semver stability commitment among the three. The work is spread across a
company team (n0-computer). The 175 open issues are the largest count here, but
that is a function of project size and traffic (12.5k stars, 1.7M recent
downloads) rather than of neglect; the last push was the day of this spike.

**On maintenance alone, iroh wins**, and it is not close.

## 2. NAT traversal, and Thai mobile networks

This is where a decision made on paper goes wrong. A Thai mobile network puts
the handset behind carrier-grade NAT, and **behind CGNAT hole punching usually
fails**: both peers are behind an address they do not control, and neither can
be reached. A relay is not a nicety there; it is the only path.

- **webrtc-rs** does ICE properly, but expects us to supply STUN and TURN
  servers. TURN is the relay, and it is not optional for CGNAT — so choosing
  webrtc-rs means running coturn, paying for its bandwidth, and keeping it up.
- **str0m** is explicit that it does not do this: "how the user figures out
  local IP addresses… is not something str0m cares about", no STUN client, no
  TURN provisioning. We would build candidate gathering *and* run coturn.
- **iroh** does hole punching and falls back to a relay by itself. It ships
  four default relays run by n0, **including an Asia-Pacific one**
  (`aps1-1.relay.n0.iroh.link`), which is the one that matters for Thailand.
  The relay is also a published crate (`iroh-relay` 1.2.0) and the relay list is
  configurable through `RelayMap`/`RelayConfig`, so self-hosting is a
  configuration change rather than a rewrite.

The practical difference: with iroh, a transfer between a home Wi-Fi and a
mobile hotspot works on day one, over n0's relay if it cannot go direct. With
either WebRTC option, that same scenario needs us to stand up a TURN server
before it works at all.

**This does mean depending on somebody else's infrastructure by default.** See
the objections at the end.

## 3. Security model, and what happens to M6

**iroh's identity is an Ed25519 keypair, and it is the same type beam already
stores.** `iroh::SecretKey::from_bytes(&[u8; 32])` wraps
`ed25519_dalek::SigningKey` — the type in `~/.beam/id_ed25519`. The prototype
passes beam's key straight in, with no conversion:

```rust
let secret = SecretKey::from_bytes(&identity.signing_key().to_bytes());
```

Measured in the prototype, on a real transfer:

```
beam public_key (base64) : 8LDnMTFuE5FKOlVDzsx8ktLxuZhkWhj+YriN0yI/cS8=
the same bytes as hex    : f0b0e731316e13914a3a5543cecc7c92d2f1b998645a18fe62b88dd3223f712f
iroh endpoint id         : f0b0e731316e13914a3a5543cecc7c92d2f1b998645a18fe62b88dd3223f712f
```

The endpoint id **is** the device's public key. beam's fingerprint stays what it
is — `SHA256(public key)` — so `known_peers` needs no new column and no
migration.

Connections are QUIC with TLS using raw public keys: there is no certificate
chain and no CA, the peer's key *is* the credential. The shape of the API says
it plainly — `connection.remote_id()` returns `PublicKey`, **not**
`Result<PublicKey>`. There is no state in which a connection exists but the
peer's identity is merely claimed.

By contrast, webrtc-rs and str0m authenticate with DTLS certificate
fingerprints exchanged through signaling. Our Ed25519 key is not that identity,
which is exactly why the brief plans a Noise KK handshake bound to the DTLS
fingerprint for M6: a second authentication layer, using our key, channel-bound
so the signaling server cannot substitute itself.

### What this does to M6 and to S-7a

Requirement **S-7a** — "the sender's identity is proven, not merely claimed" —
is the gap ADR-0019 records. With iroh it is closed by the transport. **M6's
Noise KK handshake becomes unnecessary as a mechanism.** What replaces it:

1. **The proof** is the QUIC/TLS handshake. A connection cannot exist without
   the peer having demonstrated possession of the private key for the endpoint
   id.
2. **The authorisation check** becomes: compare `connection.remote_id()`
   against `known_peers` before accepting a transfer. That is the same check
   `receive_file` already does — it just moves from a *claimed* key in
   `TRANSFER_REQUEST` to a *proved* key from the transport, and the
   `sender_public_key` field in the request stops being load-bearing.
3. **Rule 3 (key mismatch = hard abort)** becomes stronger, not weaker: a peer
   whose key changed cannot even complete a handshake as the old identity, so
   the SSH-style warning fires on a mismatch between the id we dialled and the
   id stored for that nickname.

The channel-binding argument in the brief was about a signaling server being
unable to mount a MITM. With iroh there is no signaling server in that role, so
the attack it defends against does not arise in the same form.

**What we would be giving up**, and it should be said out loud: the trust would
rest on iroh's TLS stack rather than on a Noise layer we wrote and control. That
is a smaller attack surface to reason about in one sense (one well-trodden
protocol instead of two stacked ones) and a larger dependency in another. It
also means the project's "Noise KK" milestone — a thing the brief explicitly
asks for, and a thing worth learning — disappears. That is a course-work
consideration as much as an engineering one, and it is the user's call, not
mine.

## 4. Fit with our transport trait and 64 KiB frames

beam's engine is generic over `AsyncRead + AsyncWrite + Unpin` (ADR-0016). How
each candidate meets that:

- **iroh**: `RecvStream` implements `tokio::io::AsyncRead` and `SendStream`
  implements `tokio::io::AsyncWrite` (verified in the `noq` 1.3.0 source, which
  is iroh's QUIC layer). The entire adapter in the prototype is one line:

  ```rust
  let stream = tokio::io::join(recv_half, send_half);
  ```

  QUIC streams are byte streams, so the 64 KiB frame size is unaffected and
  there is no message-size ceiling to design around.

- **webrtc-rs 0.21**: `DataChannel` is a sealed trait with a message API —
  `send`, `try_send`, `outstanding_bytes`, buffered-amount thresholds and
  events. There is **no** `AsyncRead`/`AsyncWrite` and no `detach` in this
  version. We would write the adapter ourselves, including back-pressure off
  the buffered-amount thresholds, and reconcile SCTP message limits with our
  64 KiB frames.

- **str0m**: sans-IO by design. The caller owns the sockets and must obey the
  "single-mutation invariant" — every mutation followed by draining
  `poll_output` to `Output::Timeout` before the next. Data channels expose
  `Channel::write`. This is the largest adapter of the three, and it would sit
  between our async engine and a synchronous state machine.

## 5. Windows, dependencies, build time, size

Everything below was measured on this machine.

| | webrtc-rs | str0m | iroh |
|---|---|---|---|
| Transitive dependencies | 172 | 87 | 246 |
| Clean `cargo build --release` | 107 s | 163 s | 181 s |

Build times are close enough that they should not decide anything; str0m's is
inflated by compiling a C crypto backend.

**Binary size** is the one real cost against iroh. beam's own release binary is
**2.34 MiB**; the prototype — beam plus iroh — is **13.33 MiB**. About **11 MiB**
for the transport. That is a lot for a CLI whose job is to move files, though it
is still a single binary with no runtime dependencies (N-2). Sizes for the other
two were not measured, because no prototype was built for them; their dependency
counts suggest webrtc-rs would land somewhere between.

**Windows**: the iroh prototype was built and run on Windows 11 with the MSVC
toolchain, and moved a 2 MiB file between two processes. The other two were
compiled on Windows but not exercised.

## 6. Roadmap impact

| Milestone | Today | With iroh |
|---|---|---|
| **M4** signaling server + pairing | custom WebSocket server, presence, heartbeats, PAKE pairing | the **server largely disappears**; pairing (PAKE) stays, because trust is a different problem from reachability. See the caveat below. |
| **M5** replace TCP with WebRTC | the big one: ICE, DTLS, data channels | **a few days**: swap the TCP stream for an iroh stream. The engine does not change — the prototype proves that. |
| **M6** Noise KK + key-mismatch abort | a second authentication layer | **unnecessary as a mechanism**; becomes a `known_peers` check against the proved `remote_id`, plus the mismatch abort |
| **M7** TURN relay + `[Direct P2P]`/`[Relay]` | stand up and pay for coturn | relay is built in; the tag is a one-line match on `IncomingAddr::Ip` vs `IncomingAddr::Relay`, which the prototype already prints |

**The caveat on M4, found by running the thing rather than reading about it.**
Connecting by bare public key needs discovery. With no address hint the
prototype failed with "No addressing information available", and only worked
once the listener's address was passed explicitly. iroh's `presets::N0` solves
this by publishing each node's address to n0's DNS/pkarr service and resolving
from it. So M4 becomes a choice:

- **use n0's discovery** — nothing to build, but every beam node publishes its
  addresses to a third party; or
- **keep a small beam service** that hands out an `EndpointAddr` for a Short ID
  — which is close to the signaling server already planned, but much smaller,
  because it only carries an address rather than brokering an ICE negotiation.

Either way M4 shrinks. The second option keeps the project's own answer to "a
small server only helps peers find each other", which is the sentence the brief
opens with.

---

## The prototype

`spike/iroh-transport/` — deliberately outside the workspace, so a spike cannot
change what the product builds. It depends on the real `beam` crate and calls
`beam::transfer::send_file` and `receive_file` **unmodified**.

```
iroh-transport-spike listen --beam-dir <dir> --out <dir>
iroh-transport-spike send   --beam-dir <dir> --to <endpoint-id> [--addr <ip:port>] <file>
```

A run on this machine, two separate processes:

```
endpoint id : ecd673538b3f8e31dc4c34739a98538b3e8dc745b94b812dd057e43035f8e153
local addr  : 127.0.0.1:53355

connected: f0b0e731316e13914a3a5543cecc7c92d2f1b998645a18fe62b88dd3223f712f
path     : [Direct P2P] 127.0.0.1:53384

Incoming file
  From        alice
  Fingerprint SHA256:300446c9c68a2abef4c0e8a1ab7569d5c6d9b3869aad888651b2502a3fc30c00
  File        payload.bin
  Size        2097152
Accept? [y/N]: received 2097152 from alice, saved as payload.bin
```

Note what that output demonstrates: the Accept prompt still happens, the peer is
still looked up in `known_peers` by fingerprint, and the transfer engine is the
one that has 173 tests against it. Only the bytes underneath changed.

No prototype was built for webrtc-rs or str0m. Both need a signaling channel
before two processes can talk at all — which is the M4 work this spike is meant
to inform — so a prototype would have cost days and answered questions that the
API review above already answers.

---

## Testing across two networks

The localhost run proves the plumbing. It does not prove NAT traversal, which is
the entire question for Thai mobile networks. These steps need **two machines on
genuinely different networks** — a laptop on home Wi-Fi and a second machine (or
the same laptop) on a phone's mobile hotspot. Using two devices on the same
Wi-Fi proves nothing, because they can reach each other directly.

### Before you start

Build the prototype on both machines, and give each a beam identity that has
paired with the other:

```bash
cd spike/iroh-transport && cargo build --release
```

Set up identities exactly as in the README's two-terminal walkthrough
(`beam init` on each, then write each other's public key into `known_peers`).
The prototype reads `~/.beam` through `--beam-dir`, so the same directories
work.

**These steps use iroh's public discovery and relay servers, run by n0.** Your
endpoint id and IP addresses are published to them. That is the thing being
tested; if it is not acceptable, the answer is self-hosting, and that is its own
experiment.

### Test A — home Wi-Fi to mobile hotspot

**Machine 1, on home Wi-Fi** (the receiver):

```bash
./target/release/iroh-transport-spike listen \
    --beam-dir /path/to/bob --out ~/Downloads
```

Write down the `endpoint id`. **Do not pass `--addr` this time** — the point is
to see whether discovery finds it.

**Machine 2, on the mobile hotspot** (the sender):

```bash
./target/release/iroh-transport-spike send \
    --beam-dir /path/to/alice --to <endpoint-id> ./bigfile.bin
```

Answer `y` at the prompt on machine 1.

**What to record:**

1. Did it connect at all? If it hangs for more than about 30 seconds,
   discovery did not resolve the endpoint id — note that, it matters.
2. The `path :` line on the receiver. `[Direct P2P] <ip>` means hole punching
   worked through CGNAT. `[Relay] <url>` means it fell back — note **which**
   relay URL, since an Asia-Pacific one is the difference between usable and
   painful from Thailand.
3. Roughly how long a 100 MiB file takes, and whether that changes between the
   direct and relayed cases.
4. Whether the hash matches: `sha256sum bigfile.bin ~/Downloads/bigfile.bin`.

### Test B — the same, in the other direction

Swap the roles: listen on the hotspot machine, send from the Wi-Fi machine.
Worth doing separately, because NAT behaviour is often asymmetric — one
direction can punch through where the other cannot.

### Test C — mobile to mobile

If two phones with hotspots are available, run both ends behind mobile CGNAT.
This is the hardest case and the most likely to be `[Relay]`. If beam has to
work between two people who are both on mobile data, this is the test that says
whether it will.

### Test D — the relay path on purpose

Force the relayed case, to see what the worst path costs:

```bash
# On the receiver, block direct connections by allowing only the relay.
# (In the prototype this needs presets::N0DisableRelay inverted — currently
#  a code change; note it as work if the relayed timing matters to you.)
```

If that is too fiddly, Test C usually produces a relayed connection anyway.

### What the results should change

- **Mostly `[Direct P2P]`** — iroh's hole punching handles Thai CGNAT, and M7
  becomes a display detail.
- **Mostly `[Relay]`, acceptable speed** — the default relays are doing the
  work. Decide then whether to depend on n0's relays or self-host one closer to
  Thailand.
- **`[Relay]` and too slow, or frequent failures to connect** — that is the
  result that would change the recommendation, and it is worth knowing before
  M5 rather than after.

---

## The case against iroh

Two honest objections, so the decision is made with both sides visible.

**1. Infrastructure that is not ours.** Out of the box, iroh publishes node
addresses to n0's discovery service and relays through n0's servers. For a tool
whose pitch is that only a small server helps peers find each other, and that it
never sees your files, this needs care. Mitigations: relays and discovery are
both self-hostable (`iroh-relay` is published), and the relay carries encrypted
QUIC it cannot read. But "self-hostable" is only true if somebody hosts it.

**2. Eleven megabytes, and a milestone that disappears.** The binary grows from
2.34 MiB to 13.33 MiB. And M6 — implementing Noise KK, which is a genuinely
educational piece of work for a software engineering course — stops being
necessary. Choosing iroh trades a hand-built security layer for a library's. On
engineering grounds that is usually right. On course-work grounds it might not
be, and that is not my call.

If either objection is decisive, the fallback is **webrtc-rs**, not str0m: it
matches the original brief, it is complete enough to build on, and the M4
signaling server is work we had already planned. The costs to accept then are
the single-maintainer risk, the mid-rewrite API churn, running a TURN server for
CGNAT, and writing the byte-stream adapter ourselves.

str0m is the right choice only if we want to own the ICE state machine
deliberately. Nothing in this project wants that.
