# Design decisions

Short ADR-style entries. Each records what was decided, why, and what it costs.

Decisions still to be made are tracked in `spikes.md`. SPIKE-001, the P2P
transport choice, is closed by ADR-0025.

---

## ADR-0001 — Go module named `beam`, single module

**Status:** superseded by ADR-0010 (the project moved to Rust)

**Context.** The project lives in a directory whose path contains spaces and is
inside OneDrive. A module path derived from a repository URL would tie the code
to a hosting choice that has not been made.

**Decision.** One Go module named `beam`. Imports are `beam/internal/...`.

**Consequences.** The module is not `go get`-able from a URL, which is fine for a
terminal application distributed as a binary. If the project is later published,
the module path can be renamed in one commit.

---

## ADR-0002 — Command implementations live outside the binary crate

**Status:** accepted (M0), paths updated by ADR-0011

**Context.** The suggested layout puts the CLI under `cmd/beam/`. Code in
`package main` can only be tested through a compiled binary, which makes
end-to-end command tests slow and awkward.

**Decision.** `cmd/beam/main.go` is three lines: it calls
`cli.Execute(args, stdin, stdout, stderr)` and exits with the returned code.
All command definitions live in `internal/cli`.

**Consequences.** Every command can be exercised in a unit test with an
in-memory stdin/stdout and a temporary beam directory (`--beam-dir`), with no
process spawning. The cost is one extra package compared to the sketch in
CLAUDE.md.

---

## ADR-0003 — Key file formats

**Status:** accepted (M1), PEM label amended by ADR-0013

**Context.** Identity keys must be stored on disk in a format that is stable,
inspectable, and produced by established libraries rather than hand-rolled.

**Decision.**

- `~/.beam/id_ed25519` — PEM block `BEAM PRIVATE KEY` wrapping PKCS#8 DER,
  produced by `crypto/x509.MarshalPKCS8PrivateKey`. Mode 0600.
- `~/.beam/id_ed25519.pub` — one line, `ed25519 <base64 key> <comment>`.
  Mode 0644.

**Consequences.** Both formats come from the standard library; nothing about the
key encoding is invented here. The public key line deliberately mirrors the
`known_peers` token layout, so the same parsing helpers serve both. The PEM label
is beam-specific so that a stray `id_ed25519` is not mistaken for an OpenSSH key
by other tools.

---

## ADR-0004 — File permissions on Windows

**Status:** accepted (M1)

**Context.** The requirement is that the private key is not readable by other
users. Go's `os.FileMode` maps onto Windows ACLs only very loosely: setting
`0600` on Windows sets the read-only attribute at best and does not restrict
other users.

**Decision.** Create private files with mode 0600 on every platform. On
Unix-like systems, `Store.PermissionWarnings` additionally checks the stored
files and warns (it does not fail) when group or other bits are set. On Windows
the check is skipped and this limitation is documented rather than papered over.

**Consequences.** On Windows the protection is whatever the user profile
directory already provides. A real fix would need `golang.org/x/sys/windows` and
explicit ACL manipulation; that is out of scope and would add a dependency for a
platform-specific hardening step. Revisit if the project is graded on Windows
hardening specifically.

---

## ADR-0005 — known_peers is a line-oriented text file with preserved comments

**Status:** accepted (M1)

**Context.** `known_peers` is the trust root for receiving: only peers listed
here may send. It has to be auditable by a human and safe to hand-edit.

**Decision.** A plain text file, one peer per line:

```
<name>  ed25519 <base64 public key>  added=<RFC3339> [key=value ...]
```

Blank lines and `#` comments are preserved verbatim across edits, as are
`key=value` attributes this version does not recognise. Names are matched
case-insensitively and must match `^[A-Za-z0-9._-]{1,32}$`. A malformed line is
a hard error naming the line number — parsing never skips an entry.

**Consequences.** Skipping a bad line could silently drop a peer (annoying) or,
worse, make the file's meaning depend on the parser version. Failing loudly is
the safer default for a trust store. Preserving unknown attributes means a later
milestone can add fields without an older build destroying them. The file is not
JSON so that it reads like `~/.ssh/known_hosts`, which is the mental model
users already have.

---

## ADR-0006 — Fingerprint and Short ID derivation

**Status:** accepted (M1)

**Context.** Two identifiers are needed: one strong enough to compare out of
band, and one short enough to read aloud.

**Decision.**

- Fingerprint = `SHA-256(raw 32-byte public key)`. Canonical form is 64
  lowercase hex characters; displayed as `SHA256:<hex>`, abbreviated to the
  first 16 hex characters in tables.
- Short ID = `uint64(bigendian(fingerprint[0:8])) mod 10^9`, zero-padded to 9
  digits, displayed in groups of three.

**Consequences.** The Short ID is a 30-bit value and collisions are expected at
scale; it is only ever a lookup hint for the very first pairing, never an
authorisation check. The fingerprint is the thing users compare and the thing
the signaling server routes by. The derivation is covered by fixed test vectors
so a future refactor cannot change everyone's identifier unnoticed.

---

## ADR-0007 — Atomic writes for every file in `~/.beam`

**Status:** accepted (M1)

**Context.** A crash or a full disk during a write to `known_peers` or a key
file could leave a truncated trust store.

**Decision.** `writeFileAtomic` writes to a temporary file in the same directory,
`fsync`s it, and renames it into place.

**Consequences.** Same-directory rename is atomic on both NTFS and POSIX file
systems. This also sets the pattern for M2, where a received file is assembled
in `~/.beam/tmp/<transfer_id>/` and only moved into place after the full SHA-256
verifies.

---

## ADR-0008 — Stubbed commands fail with exit code 2

**Status:** accepted (M0)

**Context.** `pair`, `listen`, `send` and `newcode` appear in `--help` from M0 so
the intended surface is visible, but they do nothing until M2–M4.

**Decision.** Each returns a `notImplementedError` naming its milestone;
`cli.Execute` maps that to exit code 2. Exit 0 means success, 1 means a real
error, 2 means not implemented yet.

**Consequences.** Scripts and tests can tell "not built yet" apart from
"failed". No stub ever prints a success message.

---

## ADR-0009 — cobra as the only runtime dependency so far

**Status:** superseded by ADR-0012 (the project moved to Rust)

**Context.** CLAUDE.md allows `cobra` or stdlib `flag`, and requires that new
dependencies be justified.

**Decision.** Use `github.com/spf13/cobra` v1.10.2 (pulls `spf13/pflag` and
`inconshreveable/mousetrap`). No test-assertion library: tests use stdlib
`testing`.

**Consequences.** Ten-odd subcommands, per-command help, flag validation and
shell completion come for free, which matters for a CLI-only product. The cost
is three modules in `go.sum`. Cryptographic and networking dependencies
(`pake`, `flynn/noise`, `pion/webrtc`) are deliberately *not* added yet; each
will be proposed with its own ADR at the milestone that needs it.

---

## ADR-0010 — The implementation language moves from Go to Rust

**Status:** accepted (M1, replacing the Go implementation)

**Context.** M0 and M1 were built in Go, as the project brief specified, and
passed with 47 tests. The team then asked for Rust instead, wanting a systems
language closer to the machine.

**Decision.** Port the project to Rust. The Go implementation is preserved in
commit `a1ae4ec` and can be recovered from git at any time; the working tree
holds only Rust from here on. `CLAUDE.md` was updated so the brief and the code
do not contradict each other.

**Consequences.** The whole downstream stack changes with it, and each
replacement must be checked for maintenance status at the milestone that needs
it, per the project's own dependency rule:

| Planned (Go) | Rust replacement | Needed at |
|---|---|---|
| `pion/webrtc` | `webrtc-rs` | M5 |
| `flynn/noise` | `snow` | M6 |
| `schollz/pake` | `spake2` | M4 |
| goroutines + `net` | `tokio` | M2 |

Nothing about the file formats, the identity model or the security rules
changed: `known_peers`, the fingerprint and the short ID are byte-for-byte what
the Go version produced, which the fixed test vectors in
`crates/beam/src/identity/vectors.rs` pin down. Those vectors were carried over
unchanged from the Go tests and still pass, which is the evidence that the port
is faithful rather than merely compiling.

---

## ADR-0011 — Cargo workspace with a library crate and two binaries

**Status:** accepted (M0, replaces the Go layout in ADR-0001)

**Context.** The suggested layout (`cmd/`, `internal/`) is a Go convention. Rust
needs an equivalent that keeps the command code testable, which is the point
ADR-0002 made.

**Decision.** A Cargo workspace:

```
crates/beam/          lib + bin `beam`
  src/identity/       keys, fingerprints, short IDs, known_peers, store
  src/cli/            command definitions
  src/ui.rs           terminal output helpers
  tests/cli.rs        command-level tests
crates/beam-server/   bin `beam-server` (M4)
```

`src/main.rs` is a handful of lines that calls `beam::cli::execute(args, io)`
and turns the returned code into an `ExitCode`. Later milestones add
`src/transfer/`, `src/transport/`, `src/auth/` and `src/signaling/` as modules
of the same library crate; they are promoted to their own crates only if
compile times demand it.

**Consequences.** `cli::execute` takes its streams as `&mut dyn BufRead` and
`&mut dyn Write`, so `tests/cli.rs` drives whole commands against a
`TempDir` with no process spawning — the same property the Go version had.
Unit tests live in `#[cfg(test)]` modules beside the code they test, which is
the Rust convention and lets them reach private helpers.

---

## ADR-0012 — Rust dependencies for M0/M1

**Status:** accepted (M0, replaces ADR-0009)

**Context.** The Go version needed exactly one dependency because the standard
library covers crypto, hashing, base64 and home-directory lookup. Rust's
standard library covers none of those, so the same functionality costs more
crates. The project rule is that each one is justified.

**Decision.**

| Crate | Why |
|---|---|
| `clap` (derive) | subcommands, help, flag validation — the cobra equivalent |
| `ed25519-dalek` (`pkcs8`, `pem`) | Ed25519 signing keys and PKCS#8 encoding, from RustCrypto |
| `sha2` | SHA-256 for fingerprints |
| `base64` | public key encoding |
| `getrandom` | operating-system entropy for key generation |
| `time` | RFC 3339 `added=` timestamps |
| `serde` + `serde_json` | `--json` output |
| `thiserror` | error enums that carry structure instead of strings |
| `zeroize` | wiping key material from memory (ADR-0013); already in the tree via `ed25519-dalek` |
| `tempfile` | same-directory temporary files for atomic writes |
| `dirs` | locating the home directory across platforms |

**Consequences.** Eleven direct dependencies against Go's one. All are widely used
and maintained, and the cryptographic ones are RustCrypto's, so ADR rule 4 (do
not invent cryptography) still holds. `rand_core` was considered and dropped:
version 0.10 no longer exposes an `OsRng` feature, and `getrandom` is the same
entropy source with a smaller surface. Networking and PAKE crates are
deliberately absent until the milestones that need them.

---

## ADR-0013 — Handling private key material in Rust

**Status:** accepted (M1, amends ADR-0003)

**Context.** Rust gives finer control over key material than Go did, and with it
three decisions that Go never forced.

**Decision.**

1. **PEM label.** The Go version wrapped PKCS#8 DER in a custom `BEAM PRIVATE
   KEY` label. The `pkcs8` crate fixes the label at the standard `PRIVATE KEY`,
   and fighting it would mean re-wrapping the PEM by hand. The standard label
   is used instead. The file is still PKCS#8 and still mode 0600.
2. **`Debug` is implemented by hand, never derived.** A derived `Debug` on
   `Identity` would print the private key into any log line or panic message
   that formats it. The manual impl shows the fingerprint and comment and
   nothing else.
3. **Secrets live in `Zeroizing` buffers.** `Identity::generate` holds its
   32-byte seed in `Zeroizing<[u8; 32]>`, so the seed is wiped when the function
   returns by any path, including an early `?`. `SigningKey` is `ZeroizeOnDrop`,
   so the key material itself is wiped when the `Identity` is dropped.
   `to_pkcs8_pem` returns `Zeroizing<String>` rather than a plain `String`, and
   that type is carried all the way to the file write: copying the PEM into an
   ordinary `String` would leave the private key sitting in a heap allocation
   that nothing wipes.

**Consequences.** All three points are stronger than the Go version, which had
no way to express any of them. `zeroize` was added to the dependency set for
this (ADR-0012); it was already in the tree as a transitive dependency of
`ed25519-dalek`, so it costs nothing new to build.

`unsafe_code = "forbid"` is set workspace-wide, so beam cannot reach for raw
memory tricks — it relies on `zeroize`'s volatile writes and compiler fences
instead. This is not a defence against an attacker who can read the process's
memory while it runs, or against the operating system paging a secret to disk;
it narrows the window in which a secret sits in memory after it is no longer
needed.

A related rule that is easy to lose: **assertion messages must not print key
material.** The PEM round-trip test originally formatted the private key into
its failure message. It now prints a fixed string.

---

## ADR-0014 — Continuous integration on Linux and Windows

**Status:** accepted (M1)

**Context.** `make check` was the stated gate, but nothing enforced it, and the
team develops on Windows while the project claims to run on Linux and macOS
too. Several of beam's rules are platform-specific by nature: file permission
bits mean nothing on Windows (ADR-0004), and path handling will diverge further
in M2 when incoming file names have to be sanitised.

**Decision.** A GitHub Actions workflow at `.github/workflows/ci.yml` runs
`cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` on both `ubuntu-latest` and `windows-latest`, on every push to the
default branch and on every pull request. `fail-fast` is off so that a failure
on one platform does not hide the other platform's result.

**Consequences.** The gate in `docs/test-plan.md` is now enforced rather than
advisory, and the "known gap" that there was no CI runner is closed. Formatting
and lints are checked twice, once per platform, which is redundant but costs
seconds and keeps the job definition uniform. macOS is deliberately not in the
matrix yet: no code is macOS-specific, and the runners are the expensive ones.
Add it when a milestone introduces platform-specific behaviour beyond the Unix
and Windows split that already exists.

---

## ADR-0015 — Framing: length-prefixed frames, with chunks kept separate

**Status:** accepted (M2)

**Context.** CLAUDE.md leaves the message encoding open ("JSON or length-prefixed
binary, decide in design"). Two sizes are in play and conflating them would be a
mistake: a *chunk* is 4 MiB and is the unit of hashing and, from M3, of resume; a
*frame* is what one read off the wire produces.

**Decision.** Every message is one frame:

```
[1 byte type][4 bytes u32 big-endian payload length][payload]
```

with the payload capped at 64 KiB. A 4 MiB chunk is announced by one
`CHUNK_START` frame and then carried by about 64 `CHUNK_DATA` frames. Control
payloads are JSON with `deny_unknown_fields`; `CHUNK_DATA` payloads are raw
binary prefixed with the chunk index.

The declared length is compared with the limit **before any buffer is
allocated**. A peer announcing a four-gigabyte frame costs five bytes of work.

**Chunk hashes live in `CHUNK_START`, not in `TRANSFER_REQUEST`.** This departs
from the sketch in CLAUDE.md. A 10 GiB file is 2560 chunks, and 2560 hashes is
about 80 KiB raw and more once encoded — past the frame limit. Keeping them in
the request would mean two different limits, one for control frames and one for
data, which is two chances to get a bound wrong. Moving them makes one limit
cover the whole protocol, and costs nothing: the hash for a chunk still arrives
before that chunk's bytes, so the receiver still verifies before writing.

**Integrity is anchored by `file_sha256` in `TRANSFER_REQUEST`.** That value is
committed before the receiver agrees to anything, and the assembled file is
checked against it before the destination name is even reserved. Per-chunk
hashes localise damage so a single bad chunk can be re-requested instead of the
whole file; they are not what makes the transfer trustworthy end to end. A
sender that lies about a chunk hash is caught by the whole-file hash regardless.

**Consequences.** One limit to enforce, one place to enforce it. JSON control
messages stay readable in a packet dump and cost a few hundred bytes each, which
is irrelevant next to the payload. `deny_unknown_fields` means a peer inventing
fields is refused rather than half-understood — the opposite of the choice made
for `known_peers` attributes in ADR-0005, and deliberately so: an unknown
attribute in a local file the user may have edited is probably a newer beam,
while an unknown field arriving over a socket is probably a probe.

This also sets up M3: `ACCEPT` already carries a `have_bitmap` field, always
`None` in M2, so adding resume does not change the wire format.

---

## ADR-0016 — The transfer engine is generic over its byte stream

**Status:** accepted (M2)

**Context.** M2 runs over TCP, M5 replaces that with a WebRTC data channel, and
M7 adds a relay. The transfer engine must not be rewritten each time.

**Decision.** `send_file` and `receive_file` are generic over
`AsyncRead + AsyncWrite + Unpin`. There is no transport trait of beam's own: the
tokio traits already say everything the engine needs. `PathKind` is carried
alongside, purely so the progress line can say `[Direct P2P]` or `[Relay]`.

**Consequences.** Tests drive both halves over `tokio::io::duplex`, an in-memory
pipe, with no sockets and no ports — which is why the engine's test suite runs in
two seconds. One test repeats the happy path over a real `TcpStream` so that
"works in memory" cannot quietly diverge from "works on a socket". M5's job
becomes writing an adapter that presents a data channel as an `AsyncRead +
AsyncWrite`, and the engine and all its tests come along unchanged.

One wrinkle worth recording: the state machine in CLAUDE.md puts `Connecting`
after `AwaitingAccept`, which fits WebRTC, where ICE should not start until the
receiver has agreed. On a plain TCP stream the connection already exists, so in
M2 that state is entered and left in consecutive statements. M5 gives it real
work.

---

## ADR-0017 — Incoming file names are reduced, not trusted

**Status:** accepted (M2)

**Context.** The file name in `TRANSFER_REQUEST` is chosen entirely by the
sender. It is the most obviously hostile input in the protocol: it decides what
path the receiver opens.

**Decision.** A name is first reduced to a bare base name by splitting on both
`/` and `\`, whatever platform is running — a name from a Windows peer must be
cut apart on a Unix receiver too. What survives is then refused if it is:

- empty, `.` or `..`;
- carrying a control character;
- carrying `:` — a drive letter (`C:evil.txt` is relative to another drive) or an
  NTFS alternate data stream (`report.pdf:hidden`);
- carrying `<`, `>`, `"`, `|`, `?` or `*`;
- ending in a dot or a space, which Windows silently strips, so that `evil.txt.`
  and `evil.txt` would be the same file there but different names here;
- a Windows device name (`CON`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`), with or
  without an extension;
- longer than 255 bytes.

Separators are stripped rather than rejected because the useful part of
`../../etc/passwd` is still `passwd`, and a receiver that refuses the whole
transfer over a path-flavoured name is annoying without being safer.

A name that already exists becomes `report (1).pdf`, `report (2).pdf` and so on,
and the name actually used is reported to both sides. A leading dot belongs to
the stem, so `.gitignore` collides as `.gitignore (1)` rather than
` (1).gitignore`.

The destination is taken by **creating** it with `create_new`, not by checking
whether it exists. That closes the window between deciding a name is free and
using it.

**Consequences.** The test that matters asserts the property rather than the
spelling: for every hostile input, the file actually opened is a direct child of
the destination directory. Testing the exact output string for each input would
pass while still being wrong, if the list of inputs missed a trick; asserting the
parent directory cannot.

An existing file is never overwritten (D-7), and nothing is written under the
destination name until the contents have been verified (D-6): the part file is
assembled under `~/.beam/tmp/<transfer_id>/`, hashed, and only then renamed over
the reserved placeholder.

---

## ADR-0018 — M2 connects over a plain TCP address given on the command line

**Status:** accepted (M2), to be replaced in M4 and M5

**Context.** M2 is specified as "plain TCP on localhost (no server, no crypto
yet)". There is no discovery until M4 and no WebRTC until M5, but the two
commands still need to find each other.

**Decision.** `beam listen --addr <host:port>` and
`beam send <peer> <file> --addr <host:port>`. Both flags are hidden from
`--help`, because they are scaffolding rather than part of the product. `listen`
defaults to `127.0.0.1:7777`; `send` has no default and says plainly that
`--addr` is needed until M4 rather than failing obscurely.

Binding anywhere that is not loopback prints a six-line warning naming exactly
what is missing: no encryption, and an identity that is claimed rather than
proven (ADR-0019).

**Consequences.** Two peers on one machine, or two machines on a trusted LAN,
can exercise the whole transfer engine now. M4 replaces `--addr` on `send` with
a lookup by Short ID through the signaling server; M5 replaces the `TcpStream`
with a data channel. Because the engine is generic over its stream (ADR-0016),
neither change reaches the transfer code.

The warning is deliberately long. A short one would be read as boilerplate, and
the thing being warned about — that anyone who can reach the port and knows a
paired peer's public key can impersonate it — is not boilerplate.

---

## ADR-0019 — In M2 the sender's identity is claimed, not proven

**Status:** accepted (M2), resolved in M6

**Context.** The receiver checks the sender's public key against its own
`known_peers` before showing a prompt, which satisfies the letter of S-7. It does
not satisfy the intent.

**Decision.** Record the gap plainly rather than let the passing S-7 test imply
more than it proves.

`TRANSFER_REQUEST` carries the sender's public key. The receiver checks that the
key is one it has paired with. **Nothing in M2 proves the sender holds the
matching private key.** A public key is public: anyone who has seen one — from a
`beam peers` listing over somebody's shoulder, from a screenshot, from the wire —
can put it in a request and be recognised as that peer.

The Noise KK handshake in M6 is what closes this, by requiring both sides to
prove possession of their static keys before any transfer message is exchanged.

**Consequences.** M2 is safe to use on loopback and defensible on a trusted LAN.
It is not safe on an untrusted network, which is why `listen` warns on a
non-loopback bind (ADR-0018).

Every test that turns on this gap carries a `STRENGTHEN IN M6:` comment, so the
work is greppable rather than remembered. Today
`s7_an_unknown_sender_is_refused_without_a_prompt` proves only that an
*unrecognised* key is turned away. When M6 lands it gains a sibling that presents
a known peer's public key without its private key and expects a refusal — the
test that would fail today.

The same gap applies in the other direction: `beam send alice` checks that
`alice` is in the local `known_peers`, but nothing proves the machine at `--addr`
is alice. In M2 the peer name on the sending side buys a sanity check and no
more.

---

## ADR-0020 — Resume is a new transfer, not a reconnection

**Status:** accepted (M3), amends the state machine in ADR-0008's milestone

**Context.** CLAUDE.md sketches the state machine with
`Transferring → Interrupted → Reconnecting → Transferring`. That shape implies
beam reconnects on its own and picks up where it left off. It also sits badly
beside the rule that resuming needs a new Accept (S-2): if the machine can walk
back into `Transferring` by itself, the Accept has to be bolted on somewhere,
and a rule bolted on is a rule that can come off.

**Decision.** `Interrupted` and `Reconnecting` are removed. A transfer that
loses its connection ends in `Failed`. Resuming is what happens when somebody
runs `beam send <peer> <file>` again: a new transfer, with a new transfer id,
which goes through the whole machine from `Requested`, prompt included. It
simply finds that some chunks are already on disk.

There is no automatic reconnection and no `beam resume` command.

**Consequences.** S-2 stops being a rule anybody has to remember: there is no
code path that reaches `Transferring` without passing through the prompt, and a
test asserts exactly that by walking all 10 states against all 10 events. The
state machine shrank from 12 states and 13 events to 10 and 10.

The cost is that an interruption always needs a person. A large transfer over a
flaky link will stop and wait rather than healing itself. That is the right
trade for a tool whose whole point is that nothing arrives without somebody
agreeing to it, and it can be revisited — a future milestone could reconnect
*automatically within one accepted session*, which is a different thing from
resuming across sessions and would not weaken S-2.

`CLAUDE.md` was updated so the brief and the code agree.

---

## ADR-0021 — A partial is matched by what the sender cannot forge

**Status:** accepted (M3)

**Context.** When a request arrives, the receiver has to decide whether it
continues something already on disk. The obvious key is the transfer id, and it
is the wrong one: the sender chooses it, so anyone who could guess or replay one
could attach to somebody else's partial — reading how much of it exists, or
worse, contributing chunks to it.

**Decision.** A partial is matched on

```
(sender fingerprint, file_sha256, size, chunk_size)
```

and never on the transfer id. Each session uses a fresh random transfer id,
which identifies the session and nothing else. The partial's directory name is
chosen locally and is not derived from anything the sender sent.

**Consequences.** Every field in the key is one the sender cannot change without
changing which file is being sent. The fingerprint is the strongest of them: a
partial belongs to one peer, so `a_partial_is_never_offered_to_a_different_peer`
holds even before the identity is *proven* (which is still M6's job, ADR-0019) —
an unrecognised key never gets as far as the matching step.

Two files with identical contents from the same peer share a partial, which is
correct: they are the same bytes.

A consequence worth stating plainly: if the source file changes between
sessions, `file_sha256` changes, no partial matches, and the transfer starts
fresh. The old partial is left alone rather than deleted, because it still
belongs to the file as it was, and nothing has said that file is unwanted. This
is requirement D-9.

---

## ADR-0022 — Crash consistency, retention, and expiry of partials

**Status:** accepted (M3)

**Context.** A partial is state that outlives the process. Three questions have
to be answered once, in one place, or they drift: what order things are written
in, what survives a failure, and when stale data goes away.

### Layout

```
~/.beam/tmp/<id>/state.json   metadata and the have-bitmap, replaced atomically
~/.beam/tmp/<id>/part         the file being assembled
~/.beam/tmp/<id>/hashes       32 bytes per chunk, at fixed offsets
~/.beam/tmp/<id>/lock         held while a session is using this partial
```

The chunk hashes are a separate fixed-layout file rather than a field in
`state.json`, so `state.json` stays about a kilobyte whatever the file size. In
the JSON, a 10 GiB transfer would mean rewriting roughly a megabyte of metadata
after every 4 MiB chunk — tens of gigabytes of writes to record a few hundred
kilobytes of fact.

The lock is its own file because `state.json` is replaced by rename on every
write, and a lock held on a file that has been renamed away quietly stops
meaning anything.

### Write ordering

For each chunk, in this order:

1. write the chunk into `part` and `fsync`;
2. write its hash into `hashes` and `fsync`;
3. set the bit in the bitmap and replace `state.json` atomically.

**The bitmap is the last thing written.** A crash between any two steps loses
the *claim* rather than the data: the chunk is asked for again, which costs
bandwidth, instead of being counted as present when it is not, which would cost
correctness. The only direction this can be wrong in is the safe one.

### Retention

Decided in one place in `receive_file`, so the rules cannot drift apart:

| Outcome | Partial | Why |
|---|---|---|
| Completed | discarded | it became the file |
| Whole-file hash failed | discarded | it will fail the same way next time |
| Anything else, holding no chunks | discarded | nothing to keep, and an empty directory is clutter in `beam transfers` |
| Declined at the prompt | **kept** | "not now" must not throw away an earlier session's work |
| Unanswered, expired | **kept** | same |
| Connection lost | **kept** | this is what resume is for |
| Not enough disk space | **kept** | free some space and try again |
| Chunk failed its hash repeatedly | **kept** | the chunks that did verify are still good |
| Another session holds the lock | **untouched** | it is not this session's to change |

A peer sending data before ACCEPT (S-4) falls under "holding no chunks" when the
request was fresh, and under "anything else" when a partial already existed —
so a rude peer cannot destroy what a well-behaved session built.

Unknown peers, dangerous file names and malformed requests are refused *before*
a partial is opened, so there is nothing to retain or discard.

### Expiry

Partials expire after **7 days** without being written to. The sweep runs **when
`beam listen` starts**: the one moment beam is both long-lived and certainly
idle. Running it on every request would put a directory scan in the path of
every transfer; running it from `beam transfers` would mean the act of looking
at something destroys it. A partial another session is using is skipped.

`beam transfers` lists partials and marks stale ones `expired` without deleting
them; `beam transfers --clear` deletes, after a confirmation that says what is
about to be lost.

**Consequences.** Two `fsync` calls per 4 MiB chunk, which is the price of the
guarantee. The bitmap can understate what is on disk after a crash, never
overstate it. The retention table is the specification the tests in
`tests/resume.rs` check case by case.

---

## ADR-0023 — Committing a finished file across a volume boundary

**Status:** accepted (M3), fixes a bug in M2

**Context.** M2 committed a finished file with a single `rename` from
`~/.beam/tmp/...` into `--out`. A rename cannot cross volumes. `--out` on
another drive is entirely ordinary — `D:\Downloads` on Windows, a mounted disk
on Linux — so M2 would have failed at the last step of an otherwise successful
transfer, after all the bytes had been moved. This was found by asking the
question, not by hitting it: the machine this was built on has one volume.

**Decision.** `commit` tries the rename first, and on a cross-volume error falls
back to copying into a temporary file **inside the destination directory**,
`fsync`ing it, and renaming from there. The last step is therefore always a
rename within one volume.

Free space is checked on both volumes before the prompt (requirement N-7): the
partial's volume needs the bytes still missing, and the destination's needs the
whole file — but only when they really are different volumes, since otherwise
the commit is a rename and costs nothing.

**Consequences.** A crash during the copy leaves a temporary file in the
destination directory rather than a half-written download wearing the
destination's name, and the temporary is removed on any error path.

Testing this honestly is awkward: a second volume cannot be assumed on a
developer machine or a CI runner. So the copy path is a separate public function
with its own tests, which run everywhere and cover the behaviour that matters —
identical contents, the reserved placeholder replaced, nothing left behind on
failure. What the tests cannot cover is the *dispatch*: that a real cross-volume
rename produces the error we recognise. That is a manual step, written down in
`docs/test-plan.md` rather than assumed.

`same_volume` compares `st_dev` on Unix and the canonical path prefix on
Windows, and answers "not the same" when it cannot tell — the conservative way
round, since being wrong that way only costs a copy and a stricter space check.

---

## ADR-0024 — A have-bitmap is validated, not trusted

**Status:** accepted (M3)

**Context.** The bitmap in ACCEPT decides which chunks the sender does not send.
A receiver that sends a wrong one — through a bug, or deliberately — makes the
sender skip parts of a file.

**Decision.** The sender checks that the bitmap is exactly `ceil(count / 8)`
bytes, and that no bits are set past the last chunk. Either failure aborts the
transfer. Neither is repaired.

The encoding is standard base64 of a little-endian bit array: chunk `i` is bit
`i % 8` of byte `i / 8`.

**Consequences.** Repairing a bad bitmap would mean guessing which parts of a
file to skip, which is exactly the decision that must not be guessed. Aborting
turns a silent wrong file into a loud failure.

The length check is the one that does the work: a bitmap valid for some *other*
transfer is almost always the wrong length for this one, and is refused. The
padding-bits check catches the narrower case of a bitmap that is the right size
but claims chunks that do not exist.

Note what this does **not** protect against: a receiver can still claim to have
chunks it does not have, and the transfer will then fail its whole-file hash.
That is the receiver harming only itself, and the final hash catches it.

---

## ADR-0025 — iroh is the P2P transport

**Status:** accepted (2026-09-24), resolves SPIKE-001, supersedes the WebRTC and
Noise KK parts of the original brief

**Context.** The brief named `pion/webrtc` (ported to Rust as `webrtc-rs`) with a
Noise KK handshake for peer authentication. SPIKE-001 compared that against
`str0m` and `iroh` before M4, because the choice decides how much of M4 exists.
Full findings: [`spikes/transport.md`](spikes/transport.md).

### Decision

**Use `iroh`, pinned to `=1.2.0`.**

The pin is exact, not a caret range. iroh is the transport: a silent minor bump
changes how beam behaves on a network, and that is not something to discover
from a CI failure on an unrelated branch. Bumping it is a deliberate change with
the changelog read, and it gets its own commit.

### What was measured

On this project's Windows 11 machine, Rust 1.98.1, MSVC:

| | webrtc-rs 0.21.0 | str0m 0.23.1 | **iroh 1.2.0** |
|---|---|---|---|
| Reached 1.0 | no, after 8 years | no | **yes, 2026-06-15** |
| Commits, last 3 months | 100+, **89 by one person** | 55, 20 authors | 88, 17 authors |
| Transitive dependencies | 172 | 87 | 246 |
| Clean release build | 107 s | 163 s | 181 s |
| Binary cost over beam | not measured | not measured | **+11 MiB** (2.34 → 13.33) |
| Fits `AsyncRead + AsyncWrite` | no, message API | no, sans-IO | **yes, directly** |
| Relay for CGNAT | run coturn | run coturn | **built in, self-hostable** |
| Prototype built and run | no | no | **yes** |

### Why, in order of weight

1. **Our Ed25519 key is the peer identity, unchanged.**
   `iroh::SecretKey::from_bytes(&[u8; 32])` wraps `ed25519_dalek::SigningKey` —
   the type already in `~/.beam/id_ed25519`. The prototype proved the endpoint
   id *is* beam's public key, byte for byte:

   ```
   beam public_key (base64) : 8LDnMTFuE5FKOlVDzsx8ktLxuZhkWhj+YriN0yI/cS8=
   the same bytes as hex    : f0b0e731316e13914a3a5543cecc7c92d2f1b998645a18fe62b88dd3223f712f
   iroh endpoint id         : f0b0e731316e13914a3a5543cecc7c92d2f1b998645a18fe62b88dd3223f712f
   ```

   `known_peers` needs no migration, and the fingerprint stays
   `SHA256(public key)`.

2. **Identity is proved by the transport, which closes S-7a.**
   `connection.remote_id()` returns `PublicKey`, not `Result<PublicKey>`: there
   is no state in which a connection exists but the peer is merely claimed.
   Noise KK was planned to provide exactly this. It is no longer needed as a
   mechanism — see the new M6 below for what replaces it.

3. **The engine does not change.** iroh's streams implement `tokio::io::AsyncRead`
   and `AsyncWrite`, so the adapter is `tokio::io::join(recv, send)`. The
   prototype ran the real `send_file`/`receive_file` between two processes,
   unmodified, with the Accept prompt and the `known_peers` check intact.

4. **CGNAT has an answer on day one.** Thai mobile networks put both peers behind
   carrier-grade NAT, where hole punching usually fails and a relay is the only
   path. iroh does hole punching and falls back to a relay itself, and ships an
   Asia-Pacific relay. With either WebRTC option we would have to stand up and
   pay for coturn before a phone-to-laptop transfer worked at all.

5. **Maintenance.** iroh is the only candidate past 1.0, and the only one whose
   work is spread across a team. webrtc-rs is carried by one person and is
   mid-rewrite onto a sans-IO core — six pre-releases in two months.

### The two objections, and what we do about them

**Objection 1: infrastructure that is not ours.** By default iroh publishes to
n0's discovery service and relays through n0's servers. For a tool whose pitch
is that a small server only helps peers find each other, that needs an answer
rather than a shrug.

*What we do:* **beam does not use n0's discovery at all.** M4 builds our own
rendezvous server mapping Short ID to an iroh endpoint address, so the address
comes from us. The relay URL is configuration, defaulting to n0's relay during
development and moving to a self-hosted `iroh-relay` later. Exactly what would
otherwise reach n0, and how each part is switched off, is written down in
[`n0-data.md`](n0-data.md) — including the fact that the relay carries QUIC it
cannot decrypt.

**Objection 2: eleven megabytes, and a milestone that disappears.** The binary
grows from 2.34 MiB to 13.33 MiB, and M6 stops being "implement Noise KK",
which was a genuinely instructive piece of work for a software engineering
course.

*What we do:* the size is accepted — it is still one binary with no runtime
dependencies (N-2), which is the property that was actually promised. The
milestone is not deleted but **redirected**: M6 becomes a written threat model
plus security tests that *prove* impersonation fails at the transport level,
including a peer that re-ran `beam init`. Writing down what an attacker can and
cannot do, and then demonstrating it, is the more valuable artifact of the two;
it is also the one that would have been needed *anyway* alongside a hand-rolled
Noise layer.

### Consequences

The roadmap changes; `CLAUDE.md` and `requirements.md` are updated to match.

- **M4** keeps a server, but a much smaller one: Short ID → endpoint address,
  plus SPAKE2 pairing. No ICE brokering, no presence heartbeats for their own
  sake.
- **M5** absorbs the old M7. Swapping TCP for iroh and showing
  `[Direct P2P]`/`[Relay]` are the same small piece of work, because the tag is
  a match on `IncomingAddr::Ip` vs `IncomingAddr::Relay`.
- **M6** replaces Noise KK with a threat model and the tests that back it.
- **M7** no longer exists as a separate milestone.

What we give up: the trust now rests on iroh's TLS stack rather than on a Noise
layer we wrote. That is one well-trodden protocol instead of two stacked ones,
which is usually the safer bet, but it is a larger dependency and it should be
recorded as such rather than glossed.

### Still outstanding

**The cross-network measurements have not been taken.** The prototype was run on
localhost only. What remains unproven is the thing that actually decides whether
beam is usable in Thailand: whether hole punching gets through mobile CGNAT, how
often it falls back to a relay, and how slow the relayed path is.

The steps are written out in [`spikes/transport.md`](spikes/transport.md) —
tests A (home Wi-Fi to hotspot), B (the reverse, since NAT is often asymmetric),
C (mobile to mobile) and D (forced relay). This decision is made without them on
the strength of the other five criteria, and because every alternative is worse
on this specific axis: webrtc-rs and str0m need a TURN server before the same
test could even be run.

If those tests come back showing frequent connection failures rather than
merely relayed connections, that is the result that would reopen this ADR.
Relayed-but-working is expected and is not a reason to revisit.

---

## ADR-0026 — Pairing: SPAKE2, key confirmation over both proved keys, single-use codes

**Status:** accepted (M4)

**Context.** Pairing turns nine digits read aloud (the Short ID) and six digits
read off a screen (the pairing code) into a public key stored in `known_peers`
on both devices. A six-digit code is a weak password, and it crosses a network
that includes a rendezvous server nobody should have to trust (ADR-0027). The
approval of M4 set five conditions; this ADR is where the first four are met.

### Decision

Pairing runs **over an iroh connection** on ALPN `beam/pair/1`. That matters: by
the time the first pairing message is read, iroh's TLS handshake has already
proved that the peer holds the private key for `connection.remote_id()`, which
is its beam public key (ADR-0025). Pairing's job is the other half — proving
that the key belongs to the person holding the code.

```text
joiner (typed the code)                         waiter (shows the code)
  Start    {version, short_id, public_key, spake_A}  ──►
                                   ◄──  Reply {public_key, spake_B, confirm_W}
  Confirm  {confirm_J}                              ──►
  Decision {accept}                ◄──►              Decision {accept}
```

1. **SPAKE2** (`spake2::Ed25519Group`, joiner is side A). The password is
   `beam-pair-v1|<short id>|<code>`, so a code is only good for the Short ID it
   was shown with. The SPAKE2 identities are the role plus the public key
   *the transport proved* for each side.
2. **Key confirmation** — condition 2. Each side sends
   `HMAC-SHA256(k, "beam-pair-confirm-v1" ‖ role ‖ short_id ‖ joiner_key ‖ waiter_key)`
   where `k` is the SPAKE2 key. The MAC names the **speaker's role**, so a
   confirmation cannot be reflected; it covers **both public keys** and the
   **Short ID**, so it cannot be moved to another key or another Short ID.
   Verification is constant-time (`Mac::verify_slice`).
3. **Keys come from the transport, not the messages.** Each message still
   carries the sender's public key, but it must equal the connection's
   `remote_id()` or the run ends with `KeyMismatch`. The key that is returned —
   and saved — is `remote_id()`.
4. **Two people decide** — condition 4. Only after the peer has proved it knows
   the code does each side show both fingerprints and ask `[y/N]`. The rules are
   the Accept rules: no flag, config or trusted-peer bypass; no answer within
   60 s is a no; typed-ahead input is discarded. Both sides exchange their
   decisions and **nothing is written unless both said yes**.
5. **Single use, ten minutes** — condition 1. The waiter's code is spent by
   the first connection that reaches the protocol, *whatever happens next*: a
   wrong guess, a dropped connection and a success all use it up. `beam pair
   --wait` then exits; a new code means running it again. A code also expires
   ten minutes after it is shown. The waiter removes itself from the rendezvous
   server the moment an attempt starts.

### Why this is enough, in numbers

SPAKE2 gives an active attacker who does not know the code exactly one guess
per protocol run and nothing to test offline. With one run per code, a guess
succeeds with probability 10⁻⁶. Even then the attacker has only reached the
fingerprint prompt, where a person who compares the two screens says no.

### What the tests show

| Claim | Test |
|---|---|
| An attacker relaying between two honest devices with its own key, rewriting every claimed key to match, cannot complete pairing | `pairing::protocol::an_attacker_relaying_with_its_own_key_cannot_complete_pairing` |
| A claimed key that is not the proved key is refused, on either side | `a_claimed_key_that_differs_from_the_proved_key_is_refused`, `a_reply_claiming_a_different_key_is_refused` |
| The MAC is bound to role, both keys and Short ID | `confirmations_are_bound_to_the_role`, `confirmations_are_bound_to_both_keys_and_the_short_id` |
| The key returned is the one proved on the iroh connection | `tests/pairing.rs::pairing_returns_the_key_each_side_proved_on_the_connection` |
| A wrong code pairs nobody, asks nobody, and burns the code | `a_wrong_code_pairs_nobody_and_uses_the_code_up`, and two real processes in `tests/end_to_end.rs` |
| Codes expire | `a_code_expires_after_its_ttl`, `an_expired_code_ends_the_wait_and_unregisters` |

The first test was checked by mutation: with the keys removed from the SPAKE2
identities and the MAC, the relaying attacker succeeds and the test fails.

### The crate: `spake2 =0.5.0-pre.0`, pinned

CLAUDE.md asks for a maintenance check before adopting `spake2`. It is
RustCrypto's (`RustCrypto/PAKEs`), the same organisation as `sha2` and the
dalek crates beam already uses. The repository is active — the move to
curve25519-dalek v5 landed in July 2026 — but releases are rare: 0.4.0 in July
2023, then **0.5.0-pre.0 in January 2026**.

We use the pre-release, pinned exactly, because it is built on the same
curve25519-dalek 5, sha2 0.11 and rand_core 0.10 that iroh and beam already
compile. 0.4.0 would add a second, older copy of that whole stack to the binary.
The algorithm and its test vectors did not change between the two. `hmac 0.13`
(RustCrypto) is added for the confirmation. When 0.5.0 is released, the pin moves
in its own commit.

### Consequences

- The final `Decision` messages cross in flight. If the connection drops after
  one side has received the other's yes but before its own yes is delivered,
  one side can save and the other not. The QUIC stream is finished and its
  acknowledgement awaited before closing, which makes this a narrow race, not a
  normal outcome; the fix, if it ever matters, is to pair again.
- A person who learns the code by looking over a shoulder, and connects first,
  pairs *as themselves*. The fingerprint prompt is the defence, which is why it
  shows both fingerprints and asks the person to compare them with the other
  screen.

---

## ADR-0027 — The rendezvous server: signed registrations, and why it need not be trusted

**Status:** accepted (M4), superseded on the `main-test` branch by ADR-0036

**Context.** `beam pair <ID>` needs to turn a Short ID into an iroh endpoint
address. beam does not use n0's DNS discovery (ADR-0025, S-17), so it runs its
own rendezvous server. The approval's third condition: registrations are signed
by the device key with a timestamp; the server verifies the signature, rejects
stale timestamps, and checks that the public key derives the claimed Short ID;
and this ADR states that Short ID collisions can be ground, and why that is
harmless.

### Decision

`beam-server` holds an in-memory table **Short ID → [(public key, endpoint
address)]** and answers two requests, JSON over a WebSocket at `/v1`.

**`register`** carries `body` — the exact JSON text that was signed — and an
Ed25519 signature over `"beam-rendezvous-register-v1\0" ‖ body`. The body holds
the Short ID, public key, Unix timestamp and endpoint address. The server
accepts it only if:

- the signature verifies (`verify_strict`) under the public key in the body;
- the timestamp is within **±60 s** of the server's clock;
- the timestamp is **strictly newer** than the last one accepted for that key,
  so a captured registration cannot be replayed to restore an old address;
- `SHA-256(public key)` derives the claimed **Short ID**;
- the address is for **that key's endpoint id**, has at most 16 entries, and
  contains only IP and relay addresses.

A registration lives **90 s** unless refreshed; the waiter refreshes every 30 s
and the entry is removed at once when its WebSocket closes. The waiter also
closes it as soon as a pairing attempt starts.

**`lookup`** returns **every** live entry for the Short ID. The client checks
each one again — key derives the Short ID, address is that key's endpoint — and
drops any that fail, whatever the server said.

Nothing is written to disk. The server prints two lines at start-up and never
logs a request: a log line tying a Short ID to an IP address is exactly the
record a rendezvous server should not keep.

### Short IDs can be ground — and why that is harmless

A Short ID is `SHA-256(public key)` reduced to nine digits: about **30 bits**
(10⁹ ≈ 2²⁹·⁹). An attacker who wants a particular Short ID generates Ed25519
keys until one lands on it: about 10⁹ key generations and hashes — somewhere
between an hour and a day on one ordinary computer depending on its cores, and
far less on many. **The signature check does
not stop this, and is not meant to**; it only makes the attacker's entry carry
the attacker's own key.

What the attacker then has is a second, correctly signed entry under the
victim's Short ID. That gains nothing:

1. **The real device is still found.** Lookup returns every entry; a collision
   cannot hide the waiting device.
2. **The attacker does not know the code.** The joiner tries each entry in turn.
   Against the attacker's entry SPAKE2 fails on the key confirmation. That costs
   the attacker's one online guess (10⁻⁶), and it does **not** use up the real
   waiter's code, which is only spent by a connection to the real waiter.
3. **Everything after pairing uses the full key.** The Short ID is never used
   again once a key is in `known_peers`, so a collision later means nothing.
4. **A person still confirms a fingerprint** that would not be the one on the
   other screen.

The Short ID is a routing hint for the first lookup, exactly as CLAUDE.md
describes it. The security comes from SPAKE2 and from the stored key.

### What a malicious server — or one ground collision — *can* do

It can **deny service**: refuse registrations, return nothing, return entries
that fail the client's checks, or hand out an address that does not connect.
Grinding eight colliding keys fills a Short ID's eight slots (a bound that exists
so one Short ID cannot grow without limit), which blocks the real device from
registering. That is 8 × 2³⁰ work for a denial of service on one pairing, and it
is accepted for M4.

It **cannot** make a device pair with a key other than the one the person
confirmed. iroh dials by endpoint id, so a wrong address fails the TLS handshake
rather than reaching an impostor; the client re-checks every entry; and SPAKE2
plus the fingerprint prompt stand behind both.

What the server **learns**, while a device is waiting: its Short ID, public key
and the IP addresses in its endpoint address; and the IP of whoever looks it up.
It keeps that in memory for at most 90 seconds after the waiter leaves.

### Transport, and the crate

The server speaks `ws://` and the client also speaks `wss://` (rustls with
webpki roots). The integrity of what the server says does not depend on TLS —
registrations are signed and lookups re-checked — but the privacy of *who looks
up whom* does, so a deployed server should sit behind `wss://`.

The WebSocket crate is `tokio-websockets 0.13`, which iroh's relay client
already compiles, with the `server` feature added. `futures-util` (sink helpers
only, also already in iroh's tree) is needed to drive it. Both were approved at
the start of M4.

The server's code lives in `beam::rendezvous`, and `beam-server` is a wrapper
around `serve()`, so beam's own tests run a real server in-process.

---

## ADR-0028 — In M4 the waiting side is `beam pair --wait`; M5 merges it into `beam listen`

**Status:** accepted (M4), amended (M5) — see the end of this entry

**Context.** CLAUDE.md's user experience has `beam listen` show the Short ID and
pairing code. In M4, though, `listen` still receives transfers over the
development TCP transport (ADR-0018), while pairing needs an iroh endpoint and
the rendezvous server. Condition 5 of the M4 approval: say clearly which command
waits for pairing in M4, and how it becomes part of `listen` in M5.

### Decision

**In M4 the receiver waits with `beam pair --wait --name <name>`.**

```text
device B:  beam pair --wait --name alice      shows Short ID + code, waits
device A:  beam pair 123456789 --name bob     looks up, asks for the code
```

Both ends name the other device up front, and both confirm a fingerprint before
anything is saved (ADR-0026). `pair --wait` takes one attempt and exits, so the
code lives exactly as long as the process: running it again is how a new code is
made. For that reason **`beam newcode` stays a stub until M5**, where there is a
long-running process for it to act on.

**In M5, `beam listen` becomes the waiting side.** It opens one iroh endpoint
with two ALPNs, `beam/pair/1` and the transfer protocol's, registers with the
rendezvous server, and shows the Short ID and a code next to "waiting for
transfers". A pairing attempt spends the code as it does now; `listen` keeps
receiving transfers but stops offering pairing until `beam newcode` gives it a
fresh code. How `newcode` reaches the running `listen` is designed in the M5
plan. The pairing prompt and the Accept prompt already share one keyboard reader
(`cli::terminal::Keyboard`), so they cannot steal each other's answers. `beam
pair --wait` stays, for pairing without also accepting files.

**Hidden `--loopback` flag.** Like `--addr` on `listen`/`send`, `pair` has a
hidden development flag that advertises only `127.0.0.1`. It exists so the
end-to-end tests can pair two beam homes on one machine without depending on
the network; ordinary use never needs it. It is on the `pair` flag allowlist
test, which also proves there is no flag that answers the `[y/N]`.

### Consequences

In M4 a receiver runs two commands in turn, `pair --wait` and then `listen`.
That is a milestone seam, not the intended experience, and it closes in M5.

### Amendment (M5): no `newcode`; the code renews itself, with a bound on guessing

**Status:** accepted (M5), replaces the "`beam newcode`" paragraph above.

M5 dropped `beam newcode` rather than build a way for one process to reach
another (M5 decision 1). **`beam listen` renews its pairing code by itself** and
prints each new one:

- after **every attempt** that used it, successful or not;
- after **ten minutes** unused.

That removes something M4 relied on. With `pair --wait`, one code meant one
guess and then the process exited: a person had to act before anyone could
guess again. A `listen` that renews its code automatically would let an
attacker guess forever while nobody watches. So the renewal is bounded
(S-23, `pairing::rotation`):

- An attempt in which the other side **did not prove the code** — a wrong code,
  a dropped connection, anything short of a verified key confirmation — is a
  **failure**. After one, the next code appears only after a pause: 5 s, then
  10 s, doubling, capped at five minutes.
- **After three failures in a row, pairing is off** for the rest of the
  `listen` session, with a message saying so and why. **Transfers from paired
  devices keep working.** Restarting `listen` turns pairing back on (M5
  answer 1).
- An attempt that proved the code but was then refused at a prompt, or was
  for a device already paired, is not a guess and **resets** the count.
- An attempt that arrives during a pause, while pairing is off, or while
  another attempt is in progress is answered **`Unavailable`** with the
  reason. It uses no code and does not count.

The arithmetic: with three guesses per session at 10⁻⁶ each, an attacker's
chance per `listen` session is 3 × 10⁻⁶, and every failed attempt is printed on
the screen of the person running `listen`. Even a correct guess only reaches the
pairing prompt (ADR-0030).

`beam pair --wait` is unchanged: one code, one attempt, then it exits.

**Naming the new peer.** `listen` has no `--name`, so it cannot know what to
call a device that pairs with it. The joiner sends its own host name as a
*hint* in the pairing `Start` message; `listen` turns it into a valid, unused
nickname (`DESKTOP-7Q2`, `laptop-2`, or `peer-<fingerprint>` when the hint is
missing or unusable) and shows it in the prompt as "Save as". It is only a
label — the key is what is trusted — and `beam rename` changes it. The hint is
not covered by the confirmation MAC because nothing relies on it.

---

## ADR-0029 — `config.toml`, the relay setting, and what M4 added to the build

**Status:** accepted (M4)

**Context.** The rendezvous server and the relay are infrastructure, and they
change between a laptop demo, a campus deployment and a self-hosted setup. They
are also the first settings beam has.

### Decision

`~/.beam/config.toml`, optional, two keys:

```toml
rendezvous = "ws://127.0.0.1:8787/v1"            # the default
relay      = "https://aps1-1.relay.n0.iroh.link./"  # the default; or "none"
```

- **TOML, not JSON**, because people edit it by hand (approved in the M4 plan).
  Unknown keys are an error, so `realy = "none"` is reported rather than
  silently ignored; a leading byte-order mark is skipped, as for `known_peers`.
- **The default rendezvous is `beam-server`'s own default address on this
  machine.** There is no public beam server, and inventing one would be worse
  than a default that obviously needs changing for two machines.
- **The relay is one URL, or `none`.** It becomes `RelayMode::custom([url])`:
  exactly the relay configured, not n0's four-relay default map. `none` gives
  direct connections only — maximally private, likely to fail behind mobile
  CGNAT (see `n0-data.md`). The development default is n0's Asia-Pacific relay,
  to be replaced by a self-hosted `iroh-relay` later (F-14).
- The endpoint is built from `presets::Minimal`. `tests/no_n0_discovery.rs`
  fails if the code ever installs a discovery service or n0's default relays
  (S-17).

### What M4 added to the build

| Crate | Why | Already in iroh's tree? |
|---|---|---|
| `iroh =1.2.0` | the transport (ADR-0025) | — |
| `spake2 =0.5.0-pre.0` | the PAKE (ADR-0026) | no; adds `hkdf` |
| `hmac 0.13` | key confirmation (ADR-0026) | no |
| `tokio-websockets 0.13` | rendezvous WebSocket (ADR-0027) | yes |
| `futures-util 0.3` (`sink` only) | to drive the WebSocket (ADR-0027) | yes |
| `toml 1.1` (`parse`, `serde`) | this file | shares its parser crates with iroh's `toml_edit` |

Each was approved before it was added. The duplicate crate versions in
`cargo tree -d` all come from inside iroh's own dependency tree.

**MSRV 1.89 → 1.91.** iroh 1.2.0 requires Rust 1.91. The exact pin makes this a
hard floor rather than a choice.

---

## ADR-0030 — `beam listen` serves pairing and transfers, and asks one question at a time

**Status:** accepted (M5)

**Context.** M5 decision 2: `listen` serves pairing and transfers on one
endpoint, and a pairing request and a transfer request can arrive together.

### Decision

**One endpoint, two ALPNs.** `listen` binds one iroh endpoint answering
`beam/pair/1` and `beam/xfer/1`, registers it once with the rendezvous server
— the same registration serves a Short ID lookup (pairing) and a key lookup
(`send`, ADR-0031) — and refreshes it every 30 s, reconnecting if the server
goes away. Each incoming connection gets its own task, dispatched on its ALPN.
The service is `beam::listener`, in the library, so tests run it against real
endpoints; the CLI only prints its events.

**One question at a time (S-24).** Every question — Accept a file, confirm a
pairing — goes to one *prompt desk* (`cli::desk`), a thread that owns the
keyboard and shows questions first come, first served.

- A question's **60 s starts when it is asked**, not when it reaches the
  screen. One that waits out its time behind another is answered **no without
  being shown**, and the screen says so ("A file from alice (report.pdf) was
  refused: it waited too long behind another question").
- A question that did wait shows how much time it has left:
  `Accept? [y/N] (41 s left):`.
- Input typed before a question appears is discarded, including lines pasted
  along with the answer to the previous question.
- While a question is on screen, the progress line does not redraw over it.

**The two questions look different (M5 answer 4).** Accepting a file is the
familiar `Incoming file … Accept? [y/N]:`. Pairing is permanent, so it has a
banner — `PAIRING REQUEST - this is permanent` — says what pairing allows and
how to undo it, and ends `Type "yes" to pair, anything else to refuse:`.
**`y` does not pair**; only `yes` does. The same prompt is used on both sides
of a pairing, `beam pair <ID>` included.

**One transfer at a time.** A transfer holds `listen`'s single slot from its
connection until it ends, prompt included. A second transfer from a paired
device is refused with `Busy`, which the sender's `beam send` reports in words:
"bob is receiving another file; try again later" (M5 answer 2). Pairing is not
a transfer and can run alongside one; its question simply queues. A sender that
bob has **not** paired with is refused as unknown *before* the slot is
considered, so a stranger cannot learn whether bob is busy.

**Noticing a vanished sender.** A killed sender sends no QUIC close. beam sets
the connection idle timeout to **15 s** (iroh keep-alives are every 5 s), so
`listen` gives up the slot within 15 s instead of noq's default 30 s, and the
partial is kept for a resume.

### Consequences

- Informational lines (a new pairing code, a turned-away sender) can print
  while a question is on screen. They are not questions, and never answer one.
- `listen` is now long-lived network software: it is online and findable
  through the rendezvous server for as long as it runs. That is recorded in
  `n0-data.md`.

---

## ADR-0031 — `send` finds a peer by its full key, and the connection's proof outranks the request

**Status:** accepted (M5)

**Context.** M5 decision 3.

### Decision

**Lookup by key.** The rendezvous protocol gains `lookup_key {public_key}`,
answered from the key every signed registration already carries. `beam send
bob` reads bob's key from `known_peers`, looks it up, and dials the endpoint id
that *is* that key. The client re-checks that the answer is for that key at
that key's endpoint, as it does for Short ID lookups. After pairing, the Short
ID is never used again.

**The connection's key is the sender.** iroh's handshake proves the dialled
key (ADR-0025), and `send` asserts `remote_id()` equals the key in
`known_peers` before sending anything. On the receiving side, `listen` gives
the engine `ReceiveOptions::proven_sender = remote_id()`. The sender is looked
up by **that** key, and a `TRANSFER_REQUEST` whose `sender_public_key`
differs from it is refused (`BadRequest`) without a prompt — even when the
claimed key belongs to another paired peer. This is what S-7a asked for; M6
writes the threat model around it, adds the impersonation tests, and removes the
`STRENGTHEN IN M6:` markers.

**After `beam init`.** A device that re-ran `beam init` has a new key and a new
Short ID. Nothing links them to the old ones — the rendezvous server cannot
know two keys are "the same device", and must not be able to. So:

- **Sending to** a peer that re-ran init finds no registration for the stored
  key. From the sender's side that is indistinguishable from the peer not
  running `listen`, and the message says both, with the fix:

  > bob (SHA256:…) is not reachable. Either it is not running `beam listen`,
  > or it ran `beam init` again and has a new key. In that case you must
  > re-pair: `beam remove bob`, then `beam pair <its Short ID> --name bob`.

- **Receiving from** a peer that re-ran init: its new key is not in
  `known_peers`, so it is refused as unknown, unprompted (S-7), and its `send`
  says the receiver does not recognise this device's key and how to re-pair.

The stored key is never updated to follow a new one (rule 3 of CLAUDE.md). M6
adds the test that proves a key change is caught and reported.

---

## ADR-0032 — The progress line follows the path, and says when it changes

**Status:** accepted (M5)

**Context.** F-11 and M5 decision 4: show `[Direct P2P]` or `[Relay]`, and
update it if the path changes mid-transfer — which is iroh's normal behaviour:
a connection often starts on the relay and moves to a direct path once hole
punching succeeds.

### Decision

The transport publishes the connection's **selected** path on a
`tokio::sync::watch` channel (`transport::dial::watch_route`), fed by iroh's
`Connection::paths_stream()`, which yields a snapshot whenever the selected
path changes. A selected relay path is `[Relay]`; a selected IP path is
`[Direct P2P]`.

The engine reads the channel rather than a fixed value: `SendOptions::route`
and `ReceiveOptions::route` replace M2's `path_kind`. Each progress update
carries the current path, and a change is reported once, as its own line:

```
[Relay] accepted
[Relay] 1.2 MiB of 40.0 MiB (3%)
Path changed: [Relay] -> [Direct P2P]
[Direct P2P] 9.8 MiB of 40.0 MiB (24%)
```

The TCP test transport and the in-memory tests use a fixed route.

### What is and is not tested

The engine's side — a change mid-transfer is reported exactly once and every
later update carries the new path — is tested by flipping the channel during a
real transfer (`a_path_change_mid_transfer_is_reported`). That iroh's
selected-path snapshots map to the right label is exercised on loopback, where
the path is direct. A **real** relay-to-direct change needs a relay and two
networks, so it is a manual step in the test plan.

---

## ADR-0033 — Limits for a paired peer that misbehaves

**Status:** accepted (M6)

**Context.** Pairing establishes who a peer is, not that it behaves. M6 item 4
asked for the existing limits to be confirmed against a paired peer that
claims absurd sizes, holds the transfer slot without sending, or sends
oversized or malformed frames. Checking them found three gaps, closed here.

### Decision

**Chunk size is capped at 16 MiB** (`MAX_CHUNK_SIZE`). The receiver holds one
chunk in memory while it verifies it, and before M6 a request could set
`chunk_size` to `u32::MAX` and have the receiver allocate 4 GiB per chunk.

**Chunk count is capped at 2²² (4 194 304)** (`MAX_CHUNK_COUNT`). The receiver
keeps a bit and a hash per chunk on disk. A 1 GiB file in 1-byte chunks
described a billion chunks, and a size near `u64::MAX` wrapped the `u32` count
to a small number the peer could then declare. The count is now computed in
`u64` and checked against the cap. With 4 MiB chunks the cap allows 16 TiB.

**Free space is checked before a new partial is created.** A request that
cannot fit is refused before any per-chunk state is written for it. Before M6
the partial — and its bitmap, 45 MB for a petabyte — was created first and
discarded after; the test for it took 13 s and now takes milliseconds. A
resume still gets the full check afterwards, on the missing bytes only.

**A stall timeout of 60 s**, on both sides, once a transfer is accepted: the
receiver waiting for the next frame, the sender waiting for an acknowledgement.
Before M6 a paired peer that stopped sending — while keeping its connection
alive, which iroh's keep-alives do by themselves — held `listen`'s one transfer
slot for ever. `listen` now also frees the slot as soon as a transfer ends,
rather than after lingering for the peer to close.

**Verification sends keep-alives (M6 answer 3).** Checking the final hash of a
large file can take longer than the sender's stall timeout, so the receiver
sends a `VERIFYING {done, total}` frame (type 10) about once a second while it
hashes. Each one resets the sender's timeout, and the sender shows it as "The
peer is verifying the file: …". This was chosen over suspending the timeout
during verification because a suspended timeout is exactly the window a
malicious receiver would use to hold a sender. Tested with a verification
slowed to three times the sender's stall timeout, and with a control run in
which the same verification without keep-alives does trip it.

### Already handled, now tested

Frames over 64 KiB are refused from their header, before the payload is read or
allocated; malformed JSON, unknown frame types, unknown fields and chunks
before a request are refused unprompted; a silent peer before its request
meets the accept timeout. `tests/hostile_peer.rs` covers each.

---

## ADR-0034 — Text from the other side never reaches the terminal raw

**Status:** accepted (M6)

**Context.** File names, pairing hints, cancel reasons, error messages that
quote what a peer sent, and the rendezvous server's error text are all chosen
by someone else. A terminal interprets some characters as commands: ANSI
sequences recolour, clear, move the cursor, rewrite lines already read or set
the window title; `\r` prints a fake fingerprint over the real one; a
right-to-left override makes `invoice‮fdp.exe` read as `invoiceexe.pdf`.

### Decision

One module, `beam::untrusted`, with three entry points — `text` (one line),
`lines` (keeps our own line breaks), `name` (file names and nicknames):

1. **Escape sequences are removed whole** — CSI, OSC, DCS/SOS/PM/APC, their
   8-bit C1 forms, two-byte `ESC x` — so no `[31m` debris remains.
2. **Other control characters are removed** (C0 including `\r`, DEL, C1); a tab
   becomes a space.
3. **Invisible direction and joining characters are shown as `<U+XXXX>`**: bidi
   overrides and isolates, LRM/RLM/ALM, ZWSP/ZWNJ/ZWJ, word joiner, BOM.
4. **Length is capped**: 300 characters for text; 60 for names, cut **in the
   middle** keeping at least the last twelve characters and always the whole
   final extension, so a long name cannot push `.exe` out of view (M6
   answer 5).

**In file names (M6 answer 1):** bidi overrides and isolates (U+202A–E,
U+2066–9) are **refused**, like control characters: on disk the name would
disguise itself in every file manager, not just in beam's prompt. Zero-width
characters and LRM/RLM are **allowed** — ZWSP is common in Thai text copied
from the web, ZWJ is part of many emoji — and are made visible in the prompt.
Thai vowels and tone marks are ordinary letters and pass through unchanged.

**Where it is applied — twice.** At the source, in the `Display` of every error
that carries a peer's or server's string (`Cancelled`, `BadRequest`,
`NameError`, malformed-frame errors, the pairing `Unavailable`/`WrongShortId`/
`Protocol`, the rendezvous `Refused`/`Protocol`), so no construction site can
be missed. And at the screen: every `ui::field` value, every prompt, every
`listen` notice, and every error line `beam` prints go through it as well.

Found while writing the tests: the pairing prompt printed the chosen name raw
in its "Once paired, … `beam remove …`" lines. Fixed; covered by
`the_pairing_prompt_cannot_be_driven_by_the_name`.

---

## ADR-0035 — A notice during an open question redraws the question

**Status:** accepted (M6), extends ADR-0030

**Context.** M6 item 5. `listen` prints notices — a new pairing code, a sender
turned away, a failed transfer — and before M6 they could land in the middle of
an open question, scrolling it away half-drawn.

### Decision

While `listen` runs, **the desk is the only thing that writes to the screen.**
`PromptDesk::notice` queues a notice alongside questions. With no question
open it is printed at once. With one open, the desk prints it on its own line
and **draws the question again** with the time it has left; the answer still
counts, and input is not discarded mid-question. `listen`'s events and the
progress reporter's one-off lines ("Verifying…", "Path changed…") both go
through it; the redrawn progress line is already suppressed while a question is
open. Warnings that used to go to stderr now come through the desk too, so they
cannot break a question either.

The desk waits for the keyboard in 50 ms slices so that notices are shown
promptly; a question still gets its whole deadline.

---

## ADR-0036 — No rendezvous server: invites, and peers found by key through the relay

**Status:** proposed, on the `main-test` branch. Supersedes ADR-0027 there;
amends ADR-0026, ADR-0029 and ADR-0031.

**Context.** The rendezvous server (ADR-0027) is the one piece of beam that
someone has to run. There is no public one, so the default in `config.toml`
points at `127.0.0.1`, and two people on different machines have to set up a
server and edit `config.toml` on both sides before they can pair. That is the
opposite of what beam is for: an easy, all-in-one peer-to-peer tool.

Two facts make the server unnecessary:

1. **iroh can reach a peer with just its key and its home relay URL.** The
   relay forwards by endpoint id, and iroh then hole-punches to a direct path
   when the network allows it. beam already keeps every paired peer's full key
   in `known_peers`, and already relays through one default relay. So after
   pairing, the lookup the server did ("where is the holder of this key?") is
   one the relay answers anyway. iroh 1.2.0 opens a connection to any relay URL
   it is given, not only its own, so peers on different relays still work.
2. **The first meeting only needs the waiting device's key and address once.**
   That fits in a line of text a person can paste into a chat.

Three alternatives were considered and rejected:

* **A built-in public rendezvous server**, for example through a named
  Cloudflare tunnel. Zero configuration for users, but someone has to keep it
  running, and pairing stops whenever it is down.
* **n0's discovery service (pkarr/DNS)** or the **BitTorrent DHT**. These are
  not ours to run, but they publish presence to a public third party
  (`n0-data.md`), and the DHT would need a new dependency (rule 4).
* **A VPN such as Radmin VPN, ZeroTier or Tailscale.** These hide a
  coordination server rather than removing it. beam works over them anyway,
  because a VPN address is just another direct address in an invite.

### Decision

**There is no rendezvous server.** The `rendezvous` module and the
`beam-server` crate are removed.

* **`beam listen` (and `beam pair --wait`) shows an invite** instead of a Short
  ID:
  `beam1` + base32(version ‖ public key ‖ relay ‖ up to 6 direct addresses ‖
  4-byte checksum). The built-in default relay costs one byte, not its URL.
  base32 is lowercase letters and digits, so a double-click selects the whole
  invite and retyping it is case-insensitive. The checksum turns a typo into
  "copy it again" instead of a pairing that fails for no visible reason. Parse
  errors never quote the pasted text (ADR-0034).
* **`beam pair <INVITE> --name <name>`** connects to the address in the invite
  and runs the unchanged SPAKE2 protocol (ADR-0026). The Short ID the code is
  bound to is derived from the invite's key on both sides, so the protocol and
  its tests are untouched.
* **The joiner saves where the invite said its peer is**, as attributes on the
  peer's `known_peers` line: `addrs=<ip:port,…>` and, only when it differs from
  this device's own relay, `relay=<url>`. A peer on the shared default relay
  then follows the default if it ever changes. `known_peers` already preserved
  unknown attributes, so the file format does not change.
* **`beam send` dials the peer's key** with the saved relay (or this device's
  own) and the saved addresses. Nothing is looked up.
* **`listen` binds a fixed UDP port**, `port` in `config.toml` (default 7820),
  so that its invite and the addresses peers saved stay the same from one run
  to the next. If the port is taken, it falls back to a random one and warns.
  `rendezvous` is dropped from `config.toml`; an old file that still has it
  loads, and the key is ignored.
* **Pairing again with an already-paired device's invite only updates where to
  find it.** No code, no network, and never the key or the name. This is how
  a peer that moved networks or relays is found again.

### Security

The invite is a **routing hint, not a credential**, exactly as the Short ID
was (CLAUDE.md, identity model):

* A connection to the key in an invite only completes against the holder of
  that key, and `remote_id()` is checked again (ADR-0025, ADR-0031).
* An invite whose key was swapped for an attacker's cannot reach the real
  waiter. If the attacker also runs the endpoint, the attacker still has to
  know the code, and both people still see and confirm fingerprints
  (ADR-0026). Tested by
  `an_invite_with_a_swapped_key_reaches_nobody_and_spends_nothing`.
* A wrong address for a paired key, from a tampered invite or a hand-edited
  `addrs=`, cannot redirect a send: the handshake fails before any byte is
  sent. Tested by
  `impersonation::a_wrong_address_for_a_paired_key_cannot_redirect_a_send`.
* The address-update path changes only `relay=` and `addrs=`. Rule 3 holds: a
  stored key is never replaced, and a new key means `beam remove` and pairing
  again. The worst a forged update can do is make a peer unreachable until the
  next real invite.
* As before, the invite and the code can travel together. If someone could
  read *and* rewrite that channel, the fingerprint comparison is what stops
  them, exactly as with a Short ID and a code.

### Consequences

* **Zero setup:** `beam init`, `beam listen`, `beam pair <invite>`,
  `beam send`. No server, no `config.toml`, no ports to forward.
* **One third party remains: the relay.** Across NAT, especially carrier-grade
  NAT, something has to coordinate hole punching and carry traffic when it
  fails. The relay carries only end-to-end-encrypted QUIC (`n0-data.md`).
  n0's public relay is free but rate-limited and meant for development.
  `relay` in `config.toml` points elsewhere, for example to a self-hosted
  `iroh-relay`.
* **With `relay = "none"`, only devices that can reach each other directly
  work:** the same LAN, a shared VPN, or a public address. The joiner can send
  to the waiter at the addresses it saved. The waiter has no address for the
  joiner until it is given the joiner's invite. `send` says so instead of
  timing out.
* **An invite is about 70–130 characters**, depending on how many network
  interfaces the device has. It is meant to be pasted, not read aloud.
* **Presence is no longer held by a server of ours.** The relay sees which keys
  are connected to it, as it already did.
* **What was lost:** a 9-digit ID that can be read over the phone, and the
  server-side tests of signed registrations, which go with the server. The
  main branch keeps the rendezvous design, so both can be compared.
