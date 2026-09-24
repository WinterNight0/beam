# Spikes

A spike is a short, timeboxed investigation whose only deliverable is a decision
and an ADR. Code written during a spike is throwaway unless the ADR says
otherwise.

---

## SPIKE-001 — P2P transport library

**Run before:** M4 (signaling server and pairing)
**Timebox:** 2 days
**Outcome:** an ADR recording the choice and the runner-up

### Why before M4, not before M5

The transport lands in M5, but the choice has to be made a milestone earlier,
because it decides how much of M4 there is to build. `webrtc-rs` and `str0m`
both assume beam supplies its own signaling server, which is what M4 describes.
`iroh` ships its own discovery and relay infrastructure, and choosing it could
shrink M4 to a thin wrapper or remove the custom server from the project
entirely. Building M4 first and then discovering that would waste the milestone.

### Candidates

- **`webrtc-rs`** — the direct port of the Pion stack named in the original
  brief.
- **`str0m`** — a sans-IO WebRTC implementation: no runtime of its own, the
  caller drives the state machine and owns the sockets.
- **`iroh`** — not WebRTC. QUIC connections between public-key-addressed nodes,
  with its own discovery and relays.

### What to evaluate

| Criterion | What to look for |
|---|---|
| Maintenance | release cadence, open issue backlog, whether a named team or one person carries it. This is the project's own dependency rule, and it is the first filter, not the last. |
| NAT traversal | ICE support, what fraction of real connections go direct, how hard host/srflx/relay candidate gathering is to drive. |
| Relay fallback | is a TURN or relay path available without the team standing up its own infrastructure, and what it costs to self-host. |
| Data channel or stream API | what the transfer engine actually writes into. M2's transport trait is defined over `AsyncRead + AsyncWrite`, so the question is how cleanly each candidate produces a stream that satisfies it. |
| Fit with the Ed25519 identity | beam already has an Ed25519 device key. Can the transport be bound to it, and does that binding survive the Noise KK channel binding planned for M6? `iroh` addresses nodes by public key natively, which may overlap with or conflict with beam's own identity model — that overlap is the main thing to understand. |
| Sans-IO versus runtime-owning | whether the library insists on its own task structure, and what that does to testability. M2's transfer tests run over an in-memory duplex; a candidate that cannot be driven that way costs test coverage. |

### Method

1. Read each project's release history and issue tracker. Drop any candidate
   that fails the maintenance filter before writing code.
2. For the survivors, write the smallest program that connects two peers on
   different networks and moves one file's worth of bytes through the
   M2 transport trait.
3. Record connection setup time, whether the path was direct or relayed, and
   how much code the adapter took.
4. Write the ADR: the choice, the runner-up, what would make the team revisit.

### Exit criteria

The spike is done when the ADR is merged. If no candidate is clearly better,
the ADR says so and records the tie-break reason explicitly — "we picked the one
the brief named" is an acceptable tie-break, as long as it is written down as a
tie-break rather than presented as a finding.
