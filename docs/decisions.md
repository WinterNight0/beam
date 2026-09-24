# Design decisions

Short ADR-style entries. Each records what was decided, why, and what it costs.

---

## ADR-0001 — Go module named `beam`, single module

**Status:** accepted (M0)

**Context.** The project lives in a directory whose path contains spaces and is
inside OneDrive. A module path derived from a repository URL would tie the code
to a hosting choice that has not been made.

**Decision.** One Go module named `beam`. Imports are `beam/internal/...`.

**Consequences.** The module is not `go get`-able from a URL, which is fine for a
terminal application distributed as a binary. If the project is later published,
the module path can be renamed in one commit.

---

## ADR-0002 — Command implementations live in `internal/cli`, not `package main`

**Status:** accepted (M0)

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

**Status:** accepted (M1)

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

**Status:** accepted (M0)

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
