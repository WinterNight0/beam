# Beam Stage 1 Architecture

## Goal

Beam is a peer-to-peer file transfer system in which every computer runs the
same Beam daemon. A transfer dynamically assigns roles:

- sender = the node that initiates the connection;
- receiver = the node whose daemon accepts the connection.

There is no separate Beam server and no middle rendezvous/relay service in
Stage 1.

## Network position

The daemon is an application-layer service built directly on the Layer 3/4
networking stack:

```text
Layer 7   Beam transfer protocol
          framing / messages / chunks / resume / integrity
              |
Layer 4   TCP
              |
Layer 3   IP
              |
Layer 2   Ethernet / Wi-Fi
```

Strictly speaking, Beam itself is not an OSI Layer-3/4 protocol: its transfer
messages are application-layer data. The daemon's network boundary is simply
kept close to TCP/IP instead of being hidden behind HTTP or a cloud service.

## Daemon model

```text
              Beam Node
                  |
          +-------+-------+
          |               |
       listen          connect
          |               |
       receiver         sender
          |               |
          +-------+-------+
                  |
             TCP/IP peer
```

The same binary implements both sides.

## Salvaged transfer engine

The existing transfer engine is transport-independent. It consumes an
`AsyncRead + AsyncWrite` stream and provides:

- protocol framing;
- transfer messages;
- chunking;
- per-chunk SHA-256 verification;
- whole-file SHA-256 verification;
- partial-file storage;
- bitmap-based resume;
- destination collision handling;
- transfer state validation;
- explicit receiver acceptance.

Only the transport layer was replaced.

## Stage 1 security boundary

Stage 1 intentionally does not require prior pairing. The sender's Ed25519
public key remains in `TransferRequest` as a forward-compatible claim, but
direct TCP does not prove possession of that key yet.

The receiver still explicitly accepts every transfer.

Future stages can add:

1. TLS;
2. cryptographic peer authentication;
3. mDNS LAN discovery;
4. known-peer trust;
5. social/human-friendly peer names and workflows.

Those layers should sit above the direct daemon rather than becoming a
replacement for it.

## Current commands

```text
beam listen
beam send <IP[:PORT]> <FILE>
beam transfers
beam transfers --clear
```

Default TCP port: `9999`.
