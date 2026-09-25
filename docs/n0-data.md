# What reaches n0's servers, and how to stop it

beam uses [iroh](https://iroh.computer) as its transport (ADR-0025). iroh is
made by a company called number 0, who also run public infrastructure that iroh
uses **by default**. beam does not accept those defaults wholesale. This page
says exactly what would be sent where, what beam actually does, and how to turn
each piece off.

Written against **iroh 1.2.0**, which is the version beam pins. Check it again
when that pin moves.

## The short version

| Service | iroh's default | **What beam does** |
|---|---|---|
| Discovery (pkarr/DNS) | publishes your endpoint id and relay URL to `dns.iroh.link`, republished every 5 minutes | **not used at all.** beam's own rendezvous server maps a Short ID to an address (M4) |
| Relay | falls back through n0's relays, including one in Asia-Pacific | **one relay, n0's Asia-Pacific one, by default during development**; set by `relay` in `~/.beam/config.toml`, or `"none"`; replaced by a self-hosted `iroh-relay` later |
| File contents | never sent to either | never sent to either |

Nothing beam does sends a file, a file name, a peer nickname, or a private key
anywhere but to the peer.

## Discovery: what it would publish, and why beam does not use it

iroh's `presets::N0` installs three services: a `PkarrPublisher` that announces
where this node can be reached, and a `PkarrResolver` plus `DnsAddressLookup`
that find other nodes the same way. The prototype in `spike/` used this preset,
which is why it could connect using only an endpoint id.

If it were enabled, each node would publish a signed record to n0's pkarr relay
containing:

- **the endpoint id** — which is the device's Ed25519 public key, and therefore
  also determines its beam fingerprint and Short ID;
- **the relay URL** it can be reached through;
- **republished every 5 minutes**, so the record also says the node is online now.

Direct IP addresses are **not** included by default. iroh's own documentation is
explicit about this: the publisher "only publishes the `RelayUrl`, to avoid
leaking IP addresses to the public pkarr server". A different `AddrFilter` would
publish IPs; beam would never set one.

The record is signed by the node and is **public**: anyone who knows an endpoint
id can resolve it and learn that the device exists, which relay it uses, and
roughly when it was last online. That is not catastrophic — it is a public key
and a relay hostname — but it is a standing, third-party-hosted statement that a
particular device is online, and beam's users did not ask for one.

**beam does not enable it.** M4 builds a rendezvous server that maps a Short ID
to an iroh endpoint address, so the lookup that discovery would do is done by a
server the project controls, holding the same information for as long as a peer
is actually listening.

`tests/no_n0_discovery.rs` enforces this: it fails if any beam source file
uses `presets::N0`, a pkarr or DNS address lookup, or n0's default relay map.

In code, the difference is which preset the endpoint is built with:

```rust
// What the spike did, and what beam does not do:
Endpoint::builder(presets::N0)          // publishes to n0, resolves from n0

// What beam does: no discovery services at all, relay set explicitly.
Endpoint::builder(presets::Minimal)     // crypto provider only
    .relay_mode(RelayMode::Custom(relay_map_from_config))
```

`presets::Minimal` sets nothing but the TLS crypto provider. No publisher, no
resolver, no relay.

## The relay: what it carries, and what it cannot read

When two peers cannot reach each other directly — which is the normal case
behind carrier-grade NAT, as on Thai mobile networks — iroh routes the
connection through a relay.

**The relay carries the QUIC connection, and it cannot read it.** Encryption is
end to end between the two endpoints, keyed by their Ed25519 identities; the
relay moves ciphertext. What the relay operator can see is traffic metadata: that
two endpoint ids are talking, when, and how many bytes. That is the same class
of information a TURN server would see, and it is why the milestone plan moves to
a self-hosted relay rather than treating n0's as permanent.

By default iroh uses four n0 relays, one of which is Asia-Pacific
(`aps1-1.relay.n0.iroh.link`). beam keeps that as the **development** default
because it works immediately and costs nothing to try, and because a relayed
connection is still encrypted.

### Choosing a different relay, or none

Relay selection is configuration, not code. In `~/.beam/config.toml`:

```toml
relay = "https://relay.example.org"   # your own iroh-relay
relay = "none"                        # direct connections only
```

which beam turns into one of iroh's relay modes:

| `RelayMode` | Effect |
|---|---|
| `Default` | n0's production relays |
| `Custom(RelayMap)` | your own relays, e.g. a self-hosted `iroh-relay` |
| `Disabled` | no relay at all — only direct connections work |

`Disabled` is the maximum-privacy setting and the one most likely to leave a
transfer unable to connect at all from a mobile network. It is the right choice
on a LAN and the wrong one over the internet.

`iroh-relay` is published as a crate (1.2.0, same release train), so a relay can
be self-hosted. Putting one in or near Thailand is on the roadmap after M5.

## What is never sent anywhere

- **The private key.** It stays in `~/.beam/id_ed25519` (S-9). iroh takes it as
  a `SecretKey` in memory to terminate TLS; it is never transmitted.
- **File contents and file names.** Only to the peer, inside the encrypted
  connection.
- **`known_peers`.** Local only. The rendezvous server learns, while a device
  is waiting to pair, its Short ID, public key and IP addresses, and the IP of
  whoever looks it up — that is what a rendezvous server is for — but never who
  you have paired with. It keeps that in memory only and logs nothing
  (ADR-0027, S-22).
- **Pairing codes and the PAKE exchange.** Pairing proves both sides knew the
  code without sending it; the code never reaches the rendezvous server.

## How to check, rather than trust this page

```bash
beam whoami --json          # the endpoint id beam would use
```

The endpoint id is the hex of the public key in that output. To see whether it
has been published to n0's discovery — it should not have been — resolve it:

```bash
# Returns nothing for a beam node, because beam never publishes.
curl -s "https://dns.iroh.link/pkarr/<endpoint-id>" | head -c 200
```

And to see whether a live transfer is going direct or through a relay, the
progress line says so: `[Direct P2P]` or `[Relay]` (F-11).
