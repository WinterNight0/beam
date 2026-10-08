# Deploying beam

**There is no server to deploy.** Since ADR-0036 beam has no rendezvous server:
two devices meet through an invite one person pastes to the other, and find
each other again by key through a relay. The rendezvous server this page used
to describe, and how to run it behind `wss://` or Cloudflare Tunnel, is in the
project history (commit `fc86508`, the end of M6).

## The one piece of infrastructure: the relay

A relay forwards the first packets between two devices that cannot reach each
other yet, helps them punch through their NATs, and carries the connection when
that fails. It only ever sees end-to-end-encrypted QUIC (`n0-data.md`).

By default beam uses number 0's public Asia-Pacific relay. It is free, needs no
setup, and is meant for development: it is shared and rate-limited, so large
relayed transfers are slow.

To use another relay, set it on **every** device that should use it:

```toml
# ~/.beam/config.toml
relay = "https://relay.example.org"
```

A device saves a peer's relay from that peer's invite when it differs from its
own, so two devices on different relays can still reach each other.

Running your own relay means running iroh's `iroh-relay` server, pinned to the
same iroh version beam uses (`=1.2.0`, ADR-0025). Its setup — a public host
name, TLS, and the UDP port used for address discovery — is documented by the
iroh project and has **not** been tried with beam yet. Record it here, with what
was verified, when it is.

## No relay at all: testing real direct P2P

```toml
# ~/.beam/config.toml — on both devices
relay = "none"
```

With no relay, nothing but the two devices takes part. Every connection that
works is a true direct connection, and the progress line always says
`[Direct P2P]`. The cost is that devices must be able to reach each other
directly. That works:

| Situation | Works without a relay? |
|---|---|
| Same LAN or Wi-Fi | Yes, out of the box |
| Same VPN (Radmin VPN, ZeroTier, Tailscale, …) | Yes, out of the box: the VPN address is in the invite |
| Different networks; the receiver's router supports UPnP, NAT-PMP or PCP | Usually: beam asks the router to forward the port and puts the public address in the invite by itself |
| Different networks; you forward the receiver's port by hand | Yes, with `advertise` (below) |
| The receiver is behind carrier-grade NAT (common on mobile data, and on some home internet) | **No.** Its router has no public address to forward. Use a relay, or swap roles so the other side receives |

### Step by step: two homes on different networks

Call the device that receives first **A**, and the other **B**.

1. **Both:** `beam init`, then create `~/.beam/config.toml` with
   `relay = "none"`. On Windows: `%USERPROFILE%\.beam\config.toml`.
2. **A: check you are not behind carrier-grade NAT.** Compare the WAN IP on
   your router's status page with what a "what is my IP" website shows. If
   they differ, or the router's WAN IP starts with `100.64.` to `100.127.`,
   you are behind CGNAT; swap roles, or test on another connection.
3. **A: make UDP port 7820 reachable.** Either your router does it by itself
   (UPnP; many home routers have it on), or forward it by hand: in the
   router's "port forwarding" or "virtual server" page, forward **UDP 7820**
   to A's LAN address. If you forwarded by hand, add your public address to
   A's config so it goes into the invite:
   ```toml
   relay = "none"
   advertise = ["<A's public IP>:7820"]
   ```
4. **A:** `beam listen`. On Windows, allow beam through the firewall when
   asked. Check the invite: `beam whoami` shows it. Send it to B by chat, and
   read the pairing code to B on a call.
5. **B:** `beam pair <A's invite> --name a`, type the code, and compare
   fingerprints; both type `yes`. B can now send to A: `beam send a <file>`.
6. **For the other direction**, B must be reachable too: repeat steps 2–4 on
   B. Then A runs `beam pair <B's invite> --name b`. They are already paired,
   so this only saves where B is: no code, no question.

### If it does not connect

- **"not reachable … timed out"**: A's port is not reachable from outside.
  Check the forward is UDP, not TCP; check it points at A's current LAN
  address; check the firewall allowed beam; and check for CGNAT (step 2).
- **"beam does not know where to find …"**: there is no relay and no saved
  address. Get the peer's invite and run `beam pair <invite> --name <name>`.
- **It works on the same Wi-Fi but not from outside**: the invite has only
  LAN addresses. Add `advertise`, or check that UPnP is on.
- **A second `listen` warns that port 7820 is in use**: the forward points at
  7820, but this `listen` is on another port. Stop the other one.

### What to record for the report

Note which situation from the table above each side was in, whether the
connection worked, and the transfer time for the same file. Then compare with
the default relay setting. Without a relay, every working connection is truly
direct: no third party takes part in the transfer at all.
