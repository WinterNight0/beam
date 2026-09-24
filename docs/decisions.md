# Design decisions

Short ADR-style entries. Each records what was decided, why, and what it costs.

Decisions still to be made are tracked in `spikes.md`; the P2P transport
library (SPIKE-001, due before M4) is the open one.

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
