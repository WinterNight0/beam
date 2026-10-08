# Security

beam is a file transfer tool, so a mistake in it can expose files, let someone
pretend to be a trusted device, or put things on a disk that should not be
there. This page is for anyone who uses, reviews or changes beam. It covers:

- what beam protects, and how
- what it stores, and what it exposes on the network
- how to use it safely
- the risks that are known and accepted, or still open
- how its dependencies are checked for vulnerabilities, and the latest result
- how to report a problem

The detailed analysis, with every threat mapped to its mitigation and the test
that proves it, is in [docs/threat-model.md](docs/threat-model.md). How the
system works end to end is in [docs/how-it-works.md](docs/how-it-works.md).

---

## 1. What beam guarantees

Each of these has automated tests that try to break it (`requirements.md`
S-1 to S-34, `threat-model.md`).

| Guarantee | How |
|---|---|
| **Nobody can send you a file you did not accept.** | Every transfer stops at a prompt that only a person can answer. There is no auto-accept flag, setting or trusted-peer bypass, and resuming needs a new Accept. Silence for 60 s is a no. |
| **Only devices you paired with can even ask.** | The receiver checks the sender's key against `known_peers` before any prompt; strangers are refused unseen. |
| **A device cannot pretend to be another.** | Each connection is QUIC/TLS 1.3 keyed by the devices' Ed25519 keys, so a connection proves who is at each end. beam trusts that proof, never a key written in a message. |
| **A changed key is never followed.** | If a peer's key changes, it shows up as "not reachable" or "unknown key", with an SSH-style warning. Re-pairing is a deliberate act. |
| **Pairing cannot be hijacked without the code and both people agreeing.** | SPAKE2 proves both sides know the six-digit code without sending it, and the confirmation binds both keys. Then both people compare fingerprints and type `yes`. A code works once; three wrong codes switch pairing off. |
| **Files arrive exactly as sent, or not at all.** | Every 4 MiB chunk is hashed before it is written, and the whole file before it is kept, against a hash the sender committed to before you accepted. It is built in a private temp folder, then moved into place. Nothing is overwritten. The sender also checks each chunk it reads against the hash it took before asking, so a file changed mid-send is stopped at once (ADR-0040). Chunk order, retries and crashes cannot change the outcome. |
| **A file name cannot escape the folder or trick the screen.** | Only the base name is used; `..`, separators, reserved names and right-to-left tricks are refused; terminal escape sequences are removed from everything printed. |
| **A paired but misbehaving peer cannot exhaust your machine.** | Caps on chunk size and count, frame size checked before allocating, at most 32 MiB of data in flight per connection (ADR-0039), free space checked first, and stall timeouts. |
| **No cryptography of our own.** | Only established, maintained libraries are used (section 3). |

## 2. What beam does *not* protect against

- **A compromised computer.** Malware that can read `~/.beam` has your
  private key, and with it your identity to every peer.
- **Damage after the final check.** beam checks the file as the operating
  system hands it back. Faulty RAM, or a disk that later damages a stored
  file, is beyond what any transfer tool can check.
- **A person who says yes without looking.** Pairing and Accept are only as
  good as the person checking the fingerprint and the file name.
- **Traffic analysis by the relay.** A relay sees which keys talk, when, and
  how much. It cannot see what (section 4).
- **Denial of service.** Anyone who can reach a running `listen` can use up
  its pairing codes (section 6, R-1 and R-8). Network-level flooding is out of
  scope.
- **The hidden `--addr` test transport.** It is unencrypted and does not prove
  keys. It exists only for the test suite, and warns on any non-loopback
  address (R-5).

## 3. Cryptography and libraries

| Purpose | Algorithm | Library |
|---|---|---|
| Device identity | Ed25519 | `ed25519-dalek` |
| Transport encryption and peer authentication | QUIC with TLS 1.3, using the Ed25519 keys | `iroh` `=1.2.0` (pinned exactly, ADR-0025), with `rustls` and `ring` |
| Pairing | SPAKE2 (password-authenticated key exchange) | `spake2` `=0.5.0-pre.0` (pinned exactly, ADR-0026) |
| Pairing confirmation | HMAC-SHA256 | `hmac`, `sha2` |
| Integrity and fingerprints | SHA-256 | `sha2` |
| Randomness | operating system CSPRNG | `getrandom` |

Key material is zeroised when dropped (`zeroize`). `unsafe` code is forbidden
across the workspace (`unsafe_code = "forbid"`).

## 4. What beam stores, and what it exposes

### On disk, under `~/.beam/`

| File | Contains | Sensitivity |
|---|---|---|
| `id_ed25519` | the device's private key | **Secret.** Mode 0600 on Linux/macOS. On Windows, the user-profile folder's permissions protect it (ADR-0004). Never leaves the device. |
| `known_peers` | paired devices: names, public keys, saved addresses | Private (0600). The trust root for receiving; anyone who can edit it can add a trusted device. |
| `listen.json` | while `listen` runs: its invite and the live pairing code | Private (0600). Single-use code, at most ten minutes old (ADR-0037). |
| `listen.lock` | nothing; held locked while `listen` runs | Not sensitive. |
| `agent.json` | while the background agent runs: its local port and **the token** that lets `beam inbox` answer requests | **Private** (0600). Believed only while `agent.lock` is held; removed on a clean stop (ADR-0042). |
| `agent.log` | what the background agent did: requests, answers, results | Names peers and files. |
| `history.jsonl` | one line per transfer that reached a person: when, which way, the peer's nickname and fingerprint, file name, size, how it ended (ADR-0043) | **Private** (0600): names peers and files. Newest 1000 kept; `beam history --clear` deletes it. A request refused before any prompt (a stranger, a busy listener) is not written, so nobody outside `known_peers` can fill it. |
| `history.lock` | nothing; held while `history.jsonl` is rewritten | Not sensitive. |
| `config.toml` | relay, port, advertised addresses, receive folder, agent port mapping | Not secret. |
| `tmp/<id>/` | partly received files | As sensitive as the files themselves. |

### On the network

| What | Who can see it |
|---|---|
| **The invite** (key, relay, IP addresses) | Whoever you send it to, and anyone who reads that chat. It is not a password, but it reveals your addresses. |
| **The relay** (n0's public one by default) | Sees your public IP, which keys connect through it, when, and how many bytes. Cannot read or alter the encrypted connection. Set `relay = "none"` to avoid it (section 5). |
| **UDP port 7820** while `listen` runs | `listen` binds a fixed port so its invite stays stable. iroh also asks your router to forward it (UPnP, NAT-PMP or PCP) when the router allows that. So a running `listen` may be reachable from the internet. Anyone who reaches it can try to pair (R-8), but transfers from unknown keys are refused. |
| **n0's discovery service** | Nothing: beam never uses it, and a test fails if anyone adds it (`tests/no_n0_discovery.rs`). |

## 5. Using beam safely

1. **Compare fingerprints out loud** (on a call, or in person) every time you
   pair. Both screens show both fingerprints. If they do not match, type
   anything but `yes`.
2. **Send the invite and the code by different channels**: the invite by
   chat, the code by voice. Someone who could read *and* rewrite your chat
   could otherwise swap in their own invite (R-2).
3. **Treat a "your peer moved to a different relay" question with
   suspicion.** beam asks before saving a relay change, because a peer's key
   is public and anyone can make an invite that names it. Say yes only if the
   peer told you it changed relay (ADR-0038).
4. **Heed the key-change warning.** "Not reachable … key has changed" or
   "does not recognise this device's key" is exactly what an impersonator
   would cause. Check the new fingerprint in person before re-pairing.
5. **Read the Accept prompt**: who it is from, the fingerprint, the file name
   and the size. Say no to anything unexpected.
6. **Stop `listen` when you do not need it.** While it runs it is reachable,
   and its pairing code is live.
7. **The background agent is optional.** Turn it on only if you want to
   receive without `beam listen` open. It never accepts by itself: answer in
   `beam inbox`. Leave its router port mapping off unless you need it. See
   [docs/background-services.md](docs/background-services.md).
8. **In the full-screen view, the safe answer is the one highlighted.**
   Accept, the pairing fingerprint check, Remove and a relay change all
   start on No; a request never opens by itself. Move to Yes only on
   purpose. In the view, **Ctrl+C copies and Ctrl+Q quits**. See
   [docs/tui.md](docs/tui.md).
9. **For maximum privacy**, use `relay = "none"`, or a relay you run. Without
   a relay, only direct connections work; the testing guide in
   [docs/deploy.md](docs/deploy.md) shows how.

## 6. Known risks

These are accepted on purpose, with their reasons, in
[threat-model.md §5](docs/threat-model.md):

| | Risk |
|---|---|
| R-1 | Anyone who can reach `listen` can use up pairing codes, and three failed attempts switch pairing off until restart. Transfers are unaffected. |
| R-2 | If the invite and the code travel together, someone who can rewrite that channel could pair in the real device's place. The fingerprint check is what stops this. |
| R-3 | The relay sees metadata: which keys talk, when, how much. |
| R-4 | A forged invite for an already-paired device can change its saved *addresses* without asking, which can make it unreachable. It cannot change the key, and a relay change now needs a yes. |
| R-5 | The hidden TCP test transport is unencrypted and does not prove keys. |
| R-6 | A crashed or silent sender can hold `listen`'s single transfer slot for 15–60 s. |
| R-7 | Notices can print while a question is open (it is redrawn). |
| R-9 | The optional background agent keeps the device reachable all day: the relay sees when it is online, and with port mapping turned on its port can be found. It does not pair, and only paired keys may ask (`docs/background-services.md`). |
| **R-8** | **Open.** With a fixed port and router port mapping, a running `listen` can be found by scanning, and anyone can use up its pairing codes or learn its public key. Fixing it trades away usability or connectivity, so it is planned rather than patched (ADR-0038). |

## 7. Vulnerability status

### Latest check: 2026-10-02

| Checked | Result |
|---|---|
| All 393 crates in `Cargo.lock` against [OSV.dev](https://osv.dev) (RustSec advisories plus GitHub security advisories) | **No known vulnerabilities.** One informational notice: RUSTSEC-2024-0436, `paste` is unmaintained. It is used only at compile time, comes in through iroh's Linux network monitoring, and does not ship in the binary. No action needed. |
| The 39 crates added on 2026-10-05 for the full-screen view (ratatui 0.30.2, crossterm 0.29.0 and what they use; ADR-0043) against OSV.dev | **No known vulnerabilities.** |
| The `spike/` prototype lockfiles | No vulnerabilities; the same `paste` notice in two of them. These are research code and do not ship. |
| [CISA KEV](https://www.cisa.gov/known-exploited-vulnerabilities-catalog) catalogue (actively exploited vulnerabilities; version 2026.09.30, 1,730 entries) | **No entry** for any component beam uses. |
| Rust toolchain (1.98.1) | Newer than the fixes for every past Rust standard-library CVE. |

### Ongoing

- **CI runs `cargo audit`** on every push and pull request (the `audit` job in
  `.github/workflows/ci.yml`). A vulnerability fails the build; an
  unmaintained-crate notice is shown as a warning.
- **To check by hand:**
  ```
  cargo install cargo-audit --locked
  cargo audit
  ```
- **Dependencies are pinned** in `Cargo.lock`. `iroh` and `spake2` are pinned
  to exact versions, because their behaviour is part of beam's security, and
  bumping either is its own reviewed change.
- **CI actions are pinned to commit hashes**, and the CI token is read-only.

### Security review log

| Date | Finding | Severity | Status |
|---|---|---|---|
| 2026-10-02 | F-1: an invite for a paired device could silently move it to an attacker's relay, which then sees when you send to it and can block it | Medium | **Fixed**: a relay change needs a yes (ADR-0038) |
| 2026-10-02 | F-2: a fixed port with router port mapping makes `listen` findable by scanning; anyone can use up its pairing codes | Low | **Open**, planned: R-8 |
| 2026-10-02 | F-3: an invite could name any `http://` or local relay URL, making beam connect to local services or talk to a relay in plain text | Low | **Fixed**: invites accept only `https://` relays on public hosts (ADR-0038) |
| 2026-10-02 | F-4: CI token had default permissions; actions referenced by movable tags; no automatic dependency audit | Low | **Fixed**: read-only token, actions pinned to commits, `cargo audit` job |
| M6 | Impersonation, terminal injection, hostile-peer limits | — | Fixed and tested in M6; `threat-model.md` |

## 8. Reporting a vulnerability

Please **do not open a public issue** for a security problem. Report it
privately, through GitHub's **"Report a vulnerability"** button on this
repository's Security tab, or directly to the maintainers. Include:

- what an attacker can do
- the steps or input that show it
- the beam version or commit

Only the latest `main` is supported. Fixes are made there and recorded in the
review log above and in `docs/decisions.md`.
