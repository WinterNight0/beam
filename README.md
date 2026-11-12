<p align="center">
  <img src="assets/beam-logo.png" alt="beam logo" width="180">
</p>

# beam

**Send files straight from one computer to another, from a terminal or a desktop window.**

beam is a peer-to-peer file transfer tool built around identity. You pair with
someone once, and after that you send them files by name: `beam send alice
report.pdf`. The file goes directly from your computer to theirs, encrypted end
to end. There is no account, no server to set up, and no cloud storage in
between. And nothing arrives unless the person on the other side says yes.

> **Status: 0.1.0, the first release** (2026-11-12). beam runs on Windows and
> Arch Linux. What is next is in the [roadmap](docs/roadmap.md).

## Features

- **Direct and encrypted.** Transfers run over QUIC (via [iroh](https://iroh.computer)),
  direct when the two computers can reach each other and through a relay when
  they cannot. The relay only ever carries encrypted data.
- **Identity, not accounts.** Each device has its own Ed25519 key. Peers are
  recognised by key, and every connection proves it. Names like `alice` are
  only your own nicknames.
- **Safe pairing.** Pair once with an invite and a short one-time code (SPAKE2).
  Both people then compare fingerprints before anything is saved.
- **You accept every file.** No auto-accept, no "trusted" bypass. Only paired
  peers can ask, and a request nobody answers expires.
- **Resumes interrupted transfers.** Send the same file again and it continues
  from where it stopped. Every chunk, and then the whole file, is checked
  against its SHA-256 hash.
- **Three ways to use it.** Typed commands, a full-screen terminal view, and a
  desktop window. They share the same identity, friends and history.
- **Optional background receiving.** A small agent runs while you are signed
  in. It shows a notification when someone sends, and you answer when you are
  ready.

## Install

**Download.** Each release on the
[Releases page](https://github.com/WinterNight0/beam/releases) has:

| File | For |
|---|---|
| `beam-setup-<version>.exe` | Windows: the setup wizard (below) |
| `beam-<version>-windows-x86_64.zip` | Windows: `beam.exe` and `beam-gui.exe`, to run without installing |
| `SHA256SUMS.txt` | The SHA-256 of each file, to check a download |

The Windows programs are not code-signed yet, so SmartScreen may warn the
first time; *More info → Run anyway* starts them.

**Arch Linux: the AUR.** With an AUR helper such as `yay`:

```
yay -S beam beam-gui     # or only beam, without the desktop window
```

Both are built on your computer from the release's source. Without a
helper: `git clone https://aur.archlinux.org/beam.git`, then `makepkg -si`
in it. `pacman -R beam-gui beam` removes them; neither touches `~/.beam`.

> **Note: not in the AUR yet.** New AUR account registration is temporarily
> closed, so the packages will be added once it reopens. Until then, build
> the same two packages from this repository (`sudo pacman -S --needed
> base-devel git` first):
>
> ```
> git clone https://github.com/WinterNight0/beam
> cd beam/packaging/arch
> makepkg -si              # builds and installs beam and beam-gui
> ```
>
> For the command line only, run `makepkg -s`, then
> `sudo pacman -U beam-<version>-1-x86_64.pkg.tar.zst`.

**From source.** You need [Rust](https://rustup.rs) 1.91 or newer.
On Windows, `rustup` also points you to the MSVC build tools. On Arch Linux,
run `sudo pacman -S --needed base-devel rustup`, then `rustup default stable`.

```
git clone <this repository>
cd beam
cargo build --release
```

This gives you two programs in `target/release`:

| Program | What it is |
|---|---|
| `beam` | The command line and the full-screen terminal view |
| `beam-gui` | The desktop window |

**Windows: the setup wizard.** Run `beam-setup-<version>.exe`. It installs
beam for you only, with no administrator rights, into
`%LOCALAPPDATA%\Programs\beam`. The desktop window is ticked by default;
untick it to install only the command line and the full-screen view. It adds
beam to the Start menu, and can add a desktop shortcut and put the `beam`
command on your PATH. Running a newer
setup updates beam; *Settings → Apps* uninstalls it. Neither touches your keys
or friends. To build the wizard yourself, install
[Inno Setup 6](https://jrsoftware.org/isinfo.php) and
[cargo-about](https://github.com/EmbarkStudios/cargo-about)
(`cargo install cargo-about --locked --features cli`), which lists the
licences of the crates built into beam, and run:

```
powershell -ExecutionPolicy Bypass -File scripts\windows\build-installer.ps1
```

It is written to `target\installer\`.

**Windows, from source without the wizard.**
`powershell -ExecutionPolicy Bypass -File scripts\windows\install.ps1` builds
beam and installs it to the same place, with Start menu shortcuts and the
PATH. It asks whether to include the desktop window (Enter means yes); `-Gui`
or `-NoGui` answers in advance, and `-Uninstall` removes it.

**Linux, from source.** Run `sh scripts/linux/install.sh`. It asks whether to include the
desktop window (Enter means yes; `--gui` or `--no-gui` answers in advance),
builds beam, copies it to `~/.local/bin`, and adds **beam** to your
applications menu with its icon, with no root needed.
`sh scripts/linux/install.sh --uninstall` removes it. The desktop window needs a Wayland or X11 session.

Other Linux distributions and macOS are expected to work, but they are not
tested yet.

## Quick start

Suppose Alice wants to send a file to Bob, and neither has used beam before.

**1. Create an identity, once on each computer.**

```
beam init
```

This makes the device's key pair in `~/.beam`. The private key never leaves the
computer.

**2. Bob waits for Alice.**

```
beam listen
```

```
  Invite        beam1ahqv23dmcwi4bjym…
  Pairing code  685 821
  Fingerprint   SHA256:10a44acf76456739...
```

Bob sends Alice the invite any way he likes, such as chat or email. It is not a
secret: it only says where Bob can be found. He tells her the pairing code
separately, for example by phone or in person.

**3. Alice pairs, using the invite and the code.**

```
beam pair beam1ahqv23dmcwi4bjym… --name bob
```

Alice types the code. Both screens then show both fingerprints. They check that
the fingerprints match, and each types `yes`. From now on they are friends:
Alice knows Bob as `bob`, and Bob can rename her with `beam rename`.

**4. Alice sends.**

```
beam send bob holiday-photos.zip
```

Bob's `listen` asks before a single byte moves:

```
Incoming file
  From          alice
  Fingerprint   SHA256:390f55e08994ce1e...
  File          holiday-photos.zip
  Size          512.0 MiB
Accept? [y/N]:
```

Bob types `y`. Alice's progress line shows `[Direct P2P]` or `[Relay]`,
depending on how the two computers reached each other. The file is checked
against the hash Alice sent, then saved.

Pairing happens once. From then on, sending is just step 4.

To try all of this on one computer, with two terminals, follow
[docs/walkthrough.md](docs/walkthrough.md).

## Three ways to use beam

They use the same `~/.beam` folder, so friends and history are the same in all
three.

**Commands.** Every feature is a `beam` command (see the [command reference](#command-reference)
below). This is the reference way to use beam, and it suits scripts too:
`--json` gives machine-readable output.

**The full-screen view.** `beam` on its own opens a view in the terminal: your
friends on the left, their details and files on the right, and what is waiting
for you in the Pending tab.

| Key | Does |
|---|---|
| `s` | Send to the selected friend (opens a file browser) |
| `o` | Turn **Receiving** on or off while the view is open |
| `:` or Ctrl+P | The command palette, for everything else |
| Ctrl+C / Ctrl+Q | Copy / quit |

The first run offers to create your identity. If you prefer plain `beam` to
print the help instead, run `beam ui cli`. More in [docs/tui.md](docs/tui.md).

**The desktop window.** `beam-gui` has five pages: Friends, Transfers, Inbox,
History and Settings. You can add a friend, send with your system's file
picker, receive with the Receiving switch, and change the receive folder, theme
and zoom. Every question starts on the safe answer. To control the background
agent from Settings, keep `beam` in the same folder as `beam-gui`.

Not every feature is in every front end yet. [docs/parity.md](docs/parity.md)
has the full list.

## Using beam

### Pairing

One device shows an invite and a pairing code. That device can run `beam
listen` (which also receives files), `beam pair --wait --name <name>` (which
only pairs), or use **Add friend → Show my invite** in the window. The other
device runs `beam pair <invite> --name <name>`, or pastes the invite under
**Add friend**.

- The code works for **one attempt** and changes every ten minutes. Run `beam
  whoami` in another terminal to see the current code.
- After three wrong codes in a row, `listen` stops offering pairing until it is
  restarted.
- Pairing is permanent, so it needs `yes` typed in full on both devices.
- Running `beam pair` with the invite of someone you already know only updates
  where to find them. Their key never changes this way.

### Sending and receiving

```
beam send <friend> <file>        # send
beam listen [--out <folder>]     # receive while this terminal is open
```

`listen` saves into the current folder unless you give it `--out`. Each request
shows who sent it, their fingerprint, the file name and size. A request nobody
answers within 60 seconds counts as a no. beam never overwrites an existing
file.

Only one transfer is received at a time. A second sender is told to try again
later.

### Receiving in the background

You don't have to keep `listen` open. The background agent receives for you
while you are signed in:

```
beam service enable              # start it now and at every sign-in
beam inbox                       # answer what it is holding
beam service status              # is it running, and where does it save?
beam service disable             # stop it, and stop starting it at sign-in
beam receive-dir <folder>        # where received files go
```

When someone sends, you get a notification. The request waits up to five
minutes for you to answer in `beam inbox`, in the full-screen view's Pending
tab, or in the window's Inbox. The agent never accepts anything by itself, and
it never pairs. It runs as you, never as a system service. More in
[docs/background-services.md](docs/background-services.md).

On Windows, files go to your Downloads folder by default. On Linux, they go to
the folder where you started the agent.

### Resuming an interrupted transfer

If a transfer stops part way (the connection drops, a laptop closes, someone
presses Ctrl+C), what already arrived is kept. **Send the same file again and
it continues from where it stopped.** There is no separate resume command.

The receiver is asked again, and the prompt says what it is:

```
Incoming file (resuming)
  ...
  Already have  1.6 GiB (40%), from 2 hours ago
Accept? [y/N]:
```

- If the file was changed in between, it starts over.
- Chunks already on disk are checked again, not trusted.
- A partial transfer belongs to the one friend who sent it.
- Declining keeps what you have, so the transfer can still be accepted later.

`beam transfers` lists partial transfers, and `beam transfers --clear [ID]`
deletes them. They are also removed after seven days without use.

### Friends and history

```
beam peers                       # your friends and their fingerprints
beam rename alice ali            # change a nickname (only on this device)
beam remove ali                  # forget a friend; they can no longer send to you
beam whoami                      # your fingerprint, and listen's invite and code
beam history                     # what came and went, newest first
```

### Command reference

```
beam                             open the full-screen view
beam init [--force]              create this device's identity
beam whoami                      show your fingerprint, and listen's invite and code
beam peers                       list friends
beam rename <old> <new>          rename a friend
beam remove <name>               forget a friend
beam pair <invite> --name <n>    pair with a device that shows an invite
beam pair --wait --name <n>      show an invite and wait to be paired with
beam listen [--out <dir>]        receive files (and pairing) in this terminal
beam send <friend> <file>        send a file
beam transfers [--clear [ID]]    list or delete partial transfers
beam history [--clear]           show or delete the transfer history
beam service enable|disable      the background agent, at every sign-in
beam service start|stop|status   the background agent, now
beam service port-mapping on|off let the agent ask the router to forward its port
beam inbox                       answer what the background agent holds
beam receive-dir [<dir>|--default]  where received files go
beam ui [tui|cli]                what plain `beam` opens
beam version                     show the version
```

Every command takes `--beam-dir <path>` (default: `$BEAM_DIR`, otherwise
`~/.beam`) and `--json`. `beam help <command>` explains each one.

### Configuration

There is nothing you have to configure. `~/.beam/config.toml` only changes the
defaults:

```toml
relay = "https://aps1-1.relay.n0.iroh.link./"  # the default relay; or "none"
port  = 7820                                   # UDP port `listen` uses; 0 = random
```

With `relay = "none"`, only devices that can reach each other directly can
connect: on the same network, on a shared VPN, or through a public address.
Testing direct connections between two homes is covered in
[docs/deploy.md](docs/deploy.md).

beam does not use n0's discovery service. The relay is the only outside service
it contacts, and only if you keep the default. See [docs/n0-data.md](docs/n0-data.md).

### Where beam keeps things

Everything is in `~/.beam` (or `--beam-dir`):

| File | What it holds |
|---|---|
| `id_ed25519` | Your private key (mode 0600). Never leaves the device |
| `id_ed25519.pub` | Your public key |
| `known_peers` | Your friends: name, key, and where to find them. Plain text; comments you add are kept |
| `config.toml` | Optional settings (relay, port) |
| `gui.toml` | The desktop window's theme, zoom and renderer |
| `history.jsonl` | What came and went (private) |
| `tmp/` | Partial transfers, until they finish or are cleared |

Running `beam init --force` replaces your key, so every friend has to pair with
you again.

## Security

beam is built around a few rules that no flag or setting can change:

- **Every transfer is accepted by hand**, resumes included. The sender cannot
  send a byte before the Accept.
- **Only friends can ask.** A request from an unknown key is refused without a
  prompt.
- **A changed key is a hard stop.** If a friend's key changes, beam refuses
  with a warning and never updates the stored key by itself. You pair again,
  deliberately.
- **Pairing needs both people.** Nothing is saved until both people have
  compared fingerprints and said yes.
- **Your private key stays on your device.**

What beam does *not* hide: the relay can see when you are online and which
keys talk to each other, when, and how much, but not what is sent. An invite
shows your network addresses to whoever reads it. Text that comes from the
network is cleaned before it is shown, so a file name cannot disguise its
extension or take over your terminal.

Each of these rules has tests that try to break it. The details, and how to
report a vulnerability, are in [SECURITY.md](SECURITY.md) and
[docs/threat-model.md](docs/threat-model.md).

## Documentation

| Document | What it covers |
|---|---|
| [docs/walkthrough.md](docs/walkthrough.md) | Try beam on one machine, step by step, and what to expect |
| [docs/how-it-works.md](docs/how-it-works.md) | The whole system: invites, pairing, finding a peer, the transfer protocol, resuming |
| [docs/tui.md](docs/tui.md) | The full-screen view |
| [docs/background-services.md](docs/background-services.md) | The background agent and `beam inbox` |
| [docs/parity.md](docs/parity.md) | Which feature is in which front end |
| [docs/deploy.md](docs/deploy.md) | The relay, and testing direct connections across networks |
| [docs/n0-data.md](docs/n0-data.md) | What the relay operator can see |
| [docs/threat-model.md](docs/threat-model.md) | Attackers, threats, and the test behind each defence |
| [docs/requirements.md](docs/requirements.md) | What beam must do, numbered, and what it deliberately does not do |
| [docs/decisions.md](docs/decisions.md) | Every design decision (ADRs), with its reasons and costs |
| [docs/roadmap.md](docs/roadmap.md) | What comes next |
| [CHANGELOG.md](CHANGELOG.md) | What each release changed |

## Development

```
make check                       # cargo fmt --check, clippy -D warnings, cargo test
cargo run -p beam -- --help      # run the CLI from source
cargo run --release -p beam-gui  # run the desktop window from source
```

On Windows without `make`, run `powershell -ExecutionPolicy Bypass -File
scripts\windows\check.ps1`.

| Script | What it does |
|---|---|
| `scripts/windows/check.ps1` | `make check` for Windows |
| `scripts/windows/install.ps1` | Build and install from source, for the current user |
| `scripts/windows/build-installer.ps1` | Build the setup wizard (`packaging/windows/beam.iss`, Inno Setup 6) |
| `scripts/windows/package.ps1` | Build a release's Windows downloads into `target/dist/`: the wizard and the zip |
| `scripts/linux/install.sh` | Build and install from source, with menu entries and icons |
| `packaging/arch/PKGBUILD` | The Arch packages `beam` and `beam-gui`, built from a release's source (for the AUR, once it takes new accounts) |

CI builds and tests on Windows and Arch Linux, and on Rust 1.91, the oldest
version beam supports. The code is a Cargo workspace in four layers:

```
crates/beam-core/    identity, pairing, transport, transfers, the agent, and
                     the shared actions every front end uses
crates/beam-model/   what a screen shows; every yes/no question starts on No
crates/beam/         the `beam` program: commands and the full-screen view
crates/beam-gui/     the `beam-gui` program: the desktop window
```

A crate only uses the crates listed above it, and the two front ends never use
each other. Cargo enforces this. Each file is described in
[docs/how-it-works.md §11](docs/how-it-works.md), and which front end offers
which feature in [docs/parity.md](docs/parity.md).

## Contributors

beam is maintained by [WinterNight0](https://github.com/WinterNight0), with
the help of the people who have contributed to it.

## Licence

beam is **source-available**, not open source. The two libraries,
`beam-core` and `beam-model`, are under the **MIT** licence. Everything else,
including the `beam` and `beam-gui` programs, the documentation and the
artwork, is under **PolyForm Strict 1.0.0**. That allows noncommercial use,
such as personal study, testing and hobby projects, but not changing the
software, distributing it, or commercial use. See [LICENSE.md](LICENSE.md) for
the details and the full texts.
