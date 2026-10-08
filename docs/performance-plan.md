# Transfer speed: findings and plan

Status: **steps 1–5 done (2026-10-03, ADR-0039 and ADR-0040); step 0 partly
done.** This page records what is known about beam's transfer
speed, the agreed order of work, and what each finished step measured. The
background (what limits speed in general) is in
[how-it-works.md §10](how-it-works.md).

**Ground rule for every step:** the user notices nothing except the speed. No
new commands, flags, prompts or settings, and none of the rules in
`CLAUDE.md` change. In particular:

- no file bytes are sent before ACCEPT, and resuming still needs a new Accept
- every chunk is still hash-checked before it counts
- the whole file is still checked before it is kept
- after a crash, resume never trusts a chunk that is not on disk
- no new dependency without asking first

Each step is measured before and after, and recorded as an ADR in
[decisions.md](decisions.md) with its numbers.

---

## 1. What has been measured

| Date | Test | Path | Result |
|---|---|---|---|
| 2026-10-02 | 5.5 GB file, two home networks about 50 km apart, default config | `[Relay]` the whole time (n0's `aps1` relay) | about 1.5 h, so **about 1 MB/s (8 Mbit/s)** |
| 2026-10-02 | The same 5.5 GiB video, two machines on one local network, release build | `[Direct P2P]` the whole time | about 5.5 min in total, including the sender's hashing before and the receiver's check after, so **about 18 MB/s (140 Mbit/s)** overall |
| 2026-10-04 | After steps 1–5. Cross-network; sender's upload about 300 Mbit/s, receiver's download about 50 Mbit/s | `[Relay]` (n0) | about **4 MiB/s (about 34 Mbit/s)**, read from the progress line (+4 MiB, one chunk, per second), so ±1 chunk/s. About 67% of the receiver's download, which caps any path at about 6 MiB/s |

Every cross-network transfer so far has gone through the relay. None has
switched to `[Direct P2P]`.

The 2026-10-04 relayed transfer ran at about 4 MiB/s. The receiver's 50 Mbit/s
download is now the ceiling (about 6 MiB/s), so beam is within about 1.5× of
the best this pair of connections allows, even through n0's relay. The rest of
the gap is the relay (its rate limit, and QUIC carried over its HTTPS
connection) and ordinary overhead. A direct connection could close at most
that 1.5×. To tell which it is: during a transfer, check whether the
receiver's download in Task Manager is near 50 Mbit/s (the line is full) or
near 34 Mbit/s with room left (the relay limits it). For a precise speed, time
the whole transfer, from `accepted` to `Verifying`, rather than the progress
line, which moves in 4 MiB steps.

The same file went **about 16 times faster** directly on a LAN than through the
relay. That confirms the relay path (or the sender's upload) was the limit in
the first test, and that direct connections are worth the most.

On a LAN the round trip is about 1 ms, so the 1.25 MB QUIC window allows more
than 1 GB/s and is not what limits 18 MB/s. What is left is the local network
(Wi-Fi is often around this speed; gigabit Ethernet allows about 110 MB/s), the
disks (an HDD pays heavily for three flushes per chunk), and the two full-file
reads outside the transfer itself. Which one it is depends on whether the test
used Wi-Fi or a cable, and SSDs or HDDs.

**Answer: both PCs were on Wi-Fi, on the same router.** Then every byte
crosses the air twice (sender → router → receiver) on one shared channel, so a
PC-to-PC copy gets roughly half of a single device's Wi-Fi speed: typically
about 125–200 Mbit/s. The measured 140 Mbit/s is in that range, so **the Wi-Fi
was the limit, and beam was close to it.** Two follow-ups:

- **Baseline:** copy the same file between the same two PCs with a Windows
  shared folder. A similar time confirms beam is at the network's limit; a
  clearly shorter one means beam has overhead to find.
- **beam's own ceiling** can only be seen on a faster path: at least one PC,
  ideally both, on a network cable. That is the test that steps 3–5 are
  measured with.

### What that number tells us

Through the relay, a round trip is roughly 60–150 ms, because every byte
travels sender → relay server → receiver. At 100 ms, beam's own limits (the
1.25 MB QUIC window plus one chunk in flight, §2) still allow about 8–12 MB/s.
The measured 1 MB/s is about ten times lower than that, so **beam's code was
not the main limit in this test.** The limit was one of:

- **n0's public relay.** It is a free, shared development service and is
  rate-limited.
- **The sender's home upload speed.** Upload is often much slower than
  download on home internet, and 8 Mbit/s is a plausible upload speed.

Step 0 finds out which of the two it was. The code steps (1–5) still matter:
they decide the speed once the path is fast, on a direct connection, our own
relay, or a LAN. But they will not fix a 1 MB/s relay.

---

## 2. Bottlenecks found in the code

| # | Bottleneck | Where | Cost |
|---|---|---|---|
| B-1 | **Fixed (step 5).** **One 4 MiB chunk in flight.** The sender waits for CHUNK_ACK after every chunk, so the line is idle for one round trip plus the receiver's disk time. | `transfer/sender.rs` (send one chunk, then wait for the answer) | worst on long round trips, as on the relay |
| B-2 | **Fixed (step 3).** **The QUIC stream window was the default, 1.25 MB.** Only the idle timeout is configured. Speed ≤ 1.25 MB ÷ round trip. | `transport/endpoint.rs`, `QuicTransportConfig` | about 12.5 MB/s at 100 ms |
| B-3 | **Fixed (step 4).** **Three flushes and a rename per chunk.** The `.part` file, the hash file and `state.json` are each `sync_all`ed. `state.json` is written with blocking `std` file calls inside async code, which stalls networking on that thread while the disk works. | `transfer/partial.rs` (`store_chunk`, `write_state_file`) | noticeable on slow disks and USB drives |
| B-4 | **Not a bottleneck (step 2).** A debug build is about 7× slower; the release profile needs no tuning. | `Cargo.toml` `[profile.release]` | none in a release build |
| B-5 | **n0's public relay is rate-limited.** | outside the code | likely the 1 MB/s seen above |

---

## 3. The plan

### Step 0: find the real limit (no code changes) — partly done

Done so far: the LAN test (item 3, with the default config) showed a direct
transfer at the Wi-Fi's limit, about 16 times faster than the relay (§1).
Still open: items 1, 2 and 4, and item 3 with `relay = "none"`.

1. Run a speed test on both home connections and note the sender's **upload**
   speed. If it is about 8 Mbit/s, the relay was not the problem.
2. Send the same file in the other direction. If the speed changes a lot, it
   was an upload limit.
3. Send a large file between two machines on one LAN, once with the default
   config and once with `relay = "none"`. The difference shows the relay's
   cost when the network itself is fast.
4. Find out why the connection never switched to direct: carrier-grade NAT on
   one side (compare the router's WAN address with the public IP, as in
   [deploy.md](deploy.md)), a strict router, or something beam could change.
   beam prints no iroh logs today. Seeing iroh's hole-punching decisions would
   need a logging crate (`tracing-subscriber`), which is a new dependency, so
   it must be agreed first (rule 4).

### Step 1: a repeatable benchmark — done

`crates/beam/tests/throughput.rs`, ignored in the normal test run:

```
cargo test --release --test throughput -- --ignored --nocapture
BEAM_BENCH_MIB=1024 BEAM_BENCH_RTT_MS=100 cargo test --release --test throughput -- --ignored --nocapture
```

A real `listen` and sender over iroh on loopback. It times the sender's
hashing, the transfer, the receiver's whole-file check, and the whole send,
and counts lost packets. `BEAM_BENCH_RTT_MS` adds a round trip through a UDP
proxy that delays every packet. Three things had to be right before its
numbers meant anything:

- iroh's hole punching found the direct loopback path around the proxy at
  once, so the proxy faces the sender on IPv6 and the listener on IPv4;
- on Windows, a stray ICMP error ends a UDP receive loop unless it is
  ignored;
- async timers (about 15 ms on Windows) released packets in bursts and capped
  the proxy at about 2.5 MB/s, so it runs on plain threads that poll the
  clock.

Its numbers are for comparing changes on one machine, not a promise about real
networks: loopback has no bandwidth limit and, normally, no loss. A run that
does lose packets comes out much slower; repeat it.

### Step 2: release build and profile — done, nothing to change

| Build | Transfer, 256 MiB, no added delay |
|---|---|
| debug | 18.6 MB/s (hashing 50 MB/s) |
| release | 137 MB/s (hashing about 1,500 MB/s) |
| release + `lto = "fat"`, `codegen-units = 1` | 119–139 MB/s, no measurable gain |

Real transfers must use a release build (yours did). The extra profile
settings were not adopted: they slow every build and gain nothing, because the
time is not spent in CPU work.

### Step 3: larger QUIC windows (fixes B-2) — done

Stream window 16 MiB; connection receive and send windows 32 MiB
(`STREAM_WINDOW`, `CONNECTION_WINDOW` in `transport/endpoint.rs`). The
connection window also closes a gap: noq sets no per-connection limit by
default, so a peer could make us buffer up to 100 stream windows. It is now
32 MiB at most. Old and new versions of beam still work together.

| Added round trip | Old windows | New windows |
|---|---|---|
| 0 (loopback) | 137 MB/s | 138 MB/s (no change expected) |
| 50 ms | 18.0 MB/s | 33–34 MB/s |
| 100 ms | 9.2 MB/s | 16.8–17.3 MB/s |

At 50 and 100 ms the result is now at the limit of one chunk in flight: each
4 MiB chunk takes about one round trip to send and one for its CHUNK_ACK.
That is B-1, step 5.

### Step 4: cheaper disk saves (fixes B-3) — done

- `state.json` is written on tokio's blocking pool, not the async thread.
- A chunk is written (data, then hash) and recorded in the bitmap at once.
  The disk flush is batched: every 8 chunks or 32 MiB, whichever comes
  first, after the last chunk, and whenever a transfer stops early.
- What a crash costs:

  | What happens | Lost |
  |---|---|
  | Connection lost, sender killed, cancelled | nothing |
  | beam on the receiver killed or crashes | nothing: the operating system still writes out what beam gave it |
  | Power loss or OS crash on the receiver | at most the last batch (32 MiB). On resume, `reverify` re-hashes every recorded chunk, drops what did not survive, and asks for it again |

  The bitmap is therefore a claim, and `reverify` (which already ran on every
  resume) is what makes it safe. This amends rule D-10, which flushed each
  chunk before recording it.
- New tests: a killed receiver keeps unflushed chunks; chunks lost to a
  simulated power failure are dropped and re-fetched, and the file still
  comes out right; a batch flushes by itself, by chunk count and by size. The
  existing kill and hang-up tests pass unchanged.

| No added delay, 1 GiB | Before | After |
|---|---|---|
| Transfer | 136–141 MB/s | 165–178 MB/s |
| Whole send (with hashing and the final check) | 116–118 MB/s | 135–144 MB/s |

With a delay the disk is not the limit, and the numbers do not change.

The first version batched by size only (32 MiB). A 2 MiB test file was then
not flushed until it had all arrived, so the kill tests no longer interrupted
anything: one failed, and one passed only by luck. Batching by chunk count as
well keeps the batch proportionate to the chunk size.

The first version also recorded a chunk only after its batch was flushed, so a
killed receiver lost up to 32 MiB. A process kill does not lose written data,
so recording at once removed that cost, for about 5% of the no-delay speed
(one `state.json` rename per chunk).

### Where beam stands after steps 1–4

On this machine, beam's own ceiling went from about 137 to about 175 MB/s, and
on a link with a 50–100 ms round trip it roughly doubled. None of this changes
the 1 MB/s relayed transfer or a Wi-Fi-limited LAN transfer: the path limits
those. What is left in beam's own code is B-1, one chunk in flight, which is
step 5.

### Step 5: several chunks in flight (fixes B-1) — done (ADR-0040)

- The sender keeps up to 4 chunks (16 MiB, the stream window) in flight.
- **A NAK (option B):** the rejected chunk is re-sent after the others, and
  the receiver accepts any chunk still missing, in any order. Duplicates and
  chunks outside the transfer are refused, and each chunk still gets 3
  attempts.
- **Versions:** a new protocol name, `beam/xfer/2`, agreed in the handshake.
  A new sender offers `/2` and `/1`, so old and new beam work together; with
  `/1` it is one chunk at a time.
- **Integrity, made stronger along the way:** the sender checks every chunk it
  reads against the hash taken before the request. A file changed mid-send is
  stopped at its first chunk, with CANCEL and a clear message, instead of
  failing the receiver's whole-file check at the end. That whole-file check,
  committed before Accept, still decides: the promised file or no file,
  whatever the order of chunks.

| Added round trip | One chunk at a time | 4 in flight |
|---|---|---|
| 0 | about 175 MB/s | about 175 MB/s |
| 50 ms | 34 MB/s | 107–110 MB/s |
| 100 ms | 17 MB/s | 72–74 MB/s |
| 200 ms | 8.4 MB/s | 40 MB/s |

### Where beam stands after steps 1–5

| Added round trip | Before step 1 | After step 5 |
|---|---|---|
| 0 (beam's own costs) | 137 MB/s | about 175 MB/s |
| 50 ms | 18 MB/s | about 108 MB/s (6×) |
| 100 ms | 9.2 MB/s | about 73 MB/s (8×) |

beam's own code is no longer what limits a long or fast link: the network,
QUIC's congestion control, the relay or the upload speed is. The two real
transfers measured so far (n0's relay at about 1 MB/s, shared Wi-Fi at about
18 MB/s) were limited by the path, and these steps do not change them. What
remains is the path itself (step 0 and the separate track).

### Separate track: the path itself (fixes B-5)

This is what can change a 1 MB/s relayed transfer. Steps 1–5 cannot.

- **More direct connections.** Act on what step 0.4 finds.
- **Our own relay.** Run `iroh-relay` on a small cloud server and set `relay =`
  in `config.toml`. It only forwards encrypted traffic and stores nothing. The
  trade-off: the team then runs a server, though not a central one, since any
  user can point at any relay. This needs its own decision (ADR) because of
  the "no server run by the team" goal.

---

## 4. Order and expected effect

| Step | Effort | Through n0's relay | On a direct, LAN or fast path |
|---|---|---|---|
| 0. Find the limit | small | tells us what to do | — |
| 1. Benchmark | small | — | — (**done**) |
| 2. Release profile | tiny | none | none: release build only, no tuning (**done**) |
| 3. QUIC windows | small | only if the relay is not the cap | about 1.8× at 50–100 ms (**done**) |
| 4. Disk saves | medium | small | about +25% when the disk is the limit (**done**) |
| 5. Pipelining | large | only if the relay is not the cap | 3–5× at 50–200 ms (**done**) |
| Path track | varies | **large** | — |
