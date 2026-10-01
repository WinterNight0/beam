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

## No relay at all

```toml
relay = "none"
```

Then only devices that can reach each other directly work: the same LAN, a
shared VPN (Radmin VPN, ZeroTier, Tailscale, …), or a device with a public
address. The `port` setting (default 7820) keeps a device's invite the same
between runs, which matters most here: with no relay, the saved addresses are
the only way to find a peer.
