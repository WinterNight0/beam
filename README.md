# Beam

Beam is a direct peer-to-peer file transfer daemon.

Every Beam node can be both **sender and receiver**. The role is decided by
which side starts a transfer; there is no separate Beam server installation
and no middle rendezvous server in Stage 1.

## Architecture

```text
PC A                                      PC B
┌─────────────────────┐                  ┌─────────────────────┐
│ Beam daemon         │                  │ Beam daemon         │
│                     │                  │                     │
│ TCP listener        │◄──── TCP/IP ───►│ TCP listener        │
│ transfer engine     │                  │ transfer engine     │
└─────────────────────┘                  └─────────────────────┘
       sender                                  receiver
```

The design takes inspiration from three different systems:

- **SCP** — simple direct file-transfer user experience.
- **FTP** — a persistent daemon that listens for incoming connections.
- **Kermit** — reliable application-level framing, chunking, integrity checks,
  and resumable transfers.

Beam's transfer engine is deliberately transport-independent: it operates on
an asynchronous byte stream. Stage 1 supplies that stream with ordinary TCP.

## Stage 1

The first milestone intentionally does **not** solve the complete trust/social
layer. It proves the core networking model first:

1. A Beam daemon listens on a TCP port (default `9999`).
2. Another Beam node connects directly to that daemon.
3. The connection carries Beam's existing framed transfer protocol.
4. The receiver can accept or decline the transfer.
5. Chunk hashes, whole-file SHA-256 verification, partial files, and resume
   continue to come from the salvaged transfer engine.

Authentication, stronger peer identity, LAN discovery, and social mechanics
can be layered on later without replacing the transfer protocol.

## Usage

Start the receiver daemon:

```bash
beam listen
```

Send directly by IP:

```bash
beam send 192.168.1.50 my_file.zip
```

The default port is `9999`. A port can be specified explicitly:

```bash
beam send 192.168.1.50:9999 my_file.zip
```

The receiver still gets an explicit Accept prompt before file bytes are sent.

## Salvaged components

The following existing Beam components remain the foundation of the project:

- `transfer/` — framing, messages, chunking, state machine, resume, storage,
  and integrity verification.
- `identity/` — retained for the later authenticated/social layer.
- CLI/UI and transfer progress handling.

The old centralized rendezvous service, standalone `beam-server`, and iroh
transport were removed because they contradict Beam's direct daemon model.
