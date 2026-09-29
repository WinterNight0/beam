# Beam Stage 1 Requirements

## Vision

Beam is a direct peer-to-peer file transfer system where **every PC can be
both client and server**. The role depends on the current transfer:

- the sender opens the outgoing connection;
- the receiver's Beam daemon accepts it;
- either PC can reverse those roles on the next transfer.

There is no middle server to deploy or configure for the transfer itself.

## Functional requirements

| ID | Requirement |
|---|---|
| F-1 | A Beam node can run a persistent TCP daemon. |
| F-2 | The daemon listens directly on IP/TCP, default port `9999`. |
| F-3 | `beam send <IP[:PORT]> <FILE>` connects directly to another Beam daemon. |
| F-4 | The sender and receiver use the same Beam binary; there is no separate server binary. |
| F-5 | The receiver explicitly accepts or declines every incoming transfer. |
| F-6 | A transfer uses the existing Beam application protocol over an `AsyncRead + AsyncWrite` stream. |
| F-7 | Interrupted transfers can resume from verified partial data. |
| F-8 | Every chunk is hash-checked and the completed file is SHA-256 verified before commit. |
| F-9 | A destination file is never silently overwritten. |
| F-10 | Multiple Beam nodes may exist on the same LAN; the network protocol does not depend on a central service. |

## Transfer-protocol requirements

| ID | Requirement |
|---|---|
| P-1 | Control messages are framed and validated before use. |
| P-2 | File bytes are not sent before the receiver sends `ACCEPT`. |
| P-3 | Chunk size and chunk count are bounded. |
| P-4 | A transfer ID cannot be reused within one daemon session. |
| P-5 | A partial transfer is matched using file content/size/chunk parameters rather than trusting a transfer ID alone. |
| P-6 | A have-bitmap is validated before it affects which chunks are sent. |
| P-7 | A stalled accepted transfer eventually times out. |
| P-8 | A partially transferred file is committed only after whole-file verification succeeds. |

## Stage 1 security boundary

Stage 1 is intentionally a transport prototype, not the final trust model.

The sender's Ed25519 public key remains in the transfer request so the existing
wire format can support the future trust layer. Direct TCP does **not** prove
that the remote machine owns that key yet.

The receiver therefore relies on:

- direct addressing;
- explicit human Accept/Reject;
- protocol validation;
- per-chunk hashes;
- whole-file SHA-256;
- resource limits and timeouts.

Future stages can add, in order:

1. TLS;
2. cryptographic peer authentication;
3. LAN discovery such as mDNS;
4. known-peer trust;
5. social/human-friendly names and workflows.

## Architecture requirements

The professor-facing architectural inspiration is:

- **SCP:** direct, simple file-transfer experience.
- **FTP:** a persistent daemon listening for incoming connections.
- **Kermit:** application-level reliable transfer, framing, chunking, integrity,
  and resume.

The OSI model is used as a conceptual boundary, not as a claim that Beam is a
Layer-3/4 protocol itself:

```text
Layer 7   Beam transfer protocol
Layer 4   TCP
Layer 3   IP
Layer 2   Ethernet / Wi-Fi
```

## Non-goals for Stage 1

- central rendezvous servers;
- relay servers;
- a separate `beam-server` executable;
- pairing protocols;
- social accounts;
- global usernames;
- NAT traversal;
- internet-wide peer discovery;
- automatic trust of a remote public key.
