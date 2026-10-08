# The background agent: receiving without `beam listen`

`beam listen` has to be open in a terminal for anyone to send you a file. The
**background agent** removes that: it waits for paired devices on its own.
When one sends something, it shows a desktop notification, and you accept or
decline in a terminal with `beam inbox`. Nothing is ever accepted without
you.

It is **off unless you turn it on**. This page covers how to use it, how it
works, what was done to keep it safe, and its limits. The decision record is
ADR-0042 in [decisions.md](decisions.md).

**Using the full-screen view?** Its Pending tab answers the agent's requests
too, with the same rules as `beam inbox` ([tui.md](tui.md) §4.2). And if you
only want to receive while beam is open, you may not need the agent at all:
the **Receiving** switch at the top of Pending (`o`) runs the same receiver
inside the view until you close it (ADR-0044, tui.md §4.2a).

---

## 1. Using it

```
beam service enable           # start the agent at every login, and now
beam service status           # is it running, where does it save, settings
beam inbox                    # answer what paired devices are sending
beam service stop             # stop it now (it still starts at next login)
beam service start            # start it now, without changing login
beam service disable          # stop starting at login, and stop it now
beam receive-dir D:\Incoming  # choose where received files go
beam receive-dir --default    # back to the default
beam service port-mapping on  # router port forwarding for the agent (asks first)
beam agent                    # run the agent in this terminal instead, until Ctrl+C
```

A typical day:

1. Once: `beam service enable`. From then on the agent starts when you log
   in.
2. A paired device runs `beam send <you> report.pdf`. A notification pops up:
   *"alice wants to send report.pdf (2.0 GiB). Open a terminal and run: beam
   inbox (expires in 5 min)"*.
3. You open a terminal and run `beam inbox`. It shows the same prompt as
   `beam listen`: who, their fingerprint, the file and its size.
   ```
   Incoming file
     From          alice
     Fingerprint   SHA256:bd9eb8c3…
     File          report.pdf
     Size          2.0 GiB
   Accept? [y/N]:
   ```
4. `y` starts the transfer, and the inbox shows its progress. Anything else,
   or no answer within **5 minutes**, declines it.

`beam inbox` can stay open to answer requests as they come. Ctrl+C leaves the
inbox; the agent keeps running. The person sending only has to wait. Their
`beam send` waits up to 5 minutes for your answer.

### Where received files go

| | Default | Change it |
|---|---|---|
| **Windows** | Your Downloads folder, asked of Windows itself. If you moved Downloads (for example to `D:\Downloads`), beam finds it there. | `beam receive-dir <folder>` |
| **Linux** | The folder the terminal was in when you ran `beam service start` or `enable` | `beam receive-dir <folder>` |
| **macOS** | Like Linux | `beam receive-dir <folder>` |

The setting is `receive_dir` in `~/.beam/config.toml`. `beam listen` uses it
too when it is given no `--out`; without it, `listen` still saves where it is
run, as before. After changing it, restart a running agent (`beam service
stop`, then `beam service start`).

### Pairing

The agent **does not pair**. To pair a new device, use `beam pair --wait
--name <name>`, which works while the agent runs. Or stop the agent and use
`beam listen`. `beam listen` refuses to start while the agent runs, because
two receivers on one identity would split your peers between them.

---

## 2. How it works

```
 paired device                    your computer
 ─────────────                    ──────────────────────────────────────────────
 beam send you file ──QUIC──►  beam agent (background, no window)
                                 · the same listener as `beam listen`
                                 · only known_peers may ask; no pairing
                                 · holds the request for up to 5 min
                                 · desktop notification ──► "run: beam inbox"
                                        ▲
                                        │ loopback TCP, token required
                                        ▼
                               beam inbox (your terminal)
                                 · the same Accept prompt as `beam listen`
                                 · y / N  ──► the agent accepts or declines
```

* **One listener, two front ends.** The agent runs exactly the code `beam
  listen` runs (`listener::run_until`), with three options changed:
  - pairing off;
  - router port mapping off unless chosen;
  - an Accept window of 5 minutes instead of 60 s.

  Everything about a transfer is unchanged: proven keys, known peers only,
  each chunk hash-checked, the whole file checked before it is kept, partials
  kept for resume, and Ctrl+C semantics (ADR-0041).
* **The agent's "prompt"** does not read a keyboard. It holds the question,
  tells every connected `beam inbox`, shows a notification, and waits for an
  answer. The listener's own timer still ends the question at 5 minutes and
  replies "expired" to the sender.
* **`beam inbox`** connects to the agent on `127.0.0.1` and proves it is you
  with a token. It then shows each request through the same prompt desk
  `beam listen` uses, with the same rules: an answer typed before the
  question appeared does not count, and no answer means no.
* **Starting and stopping**, always as you, never as administrator or root:

  | | At login (`enable`) | Now (`start`) | Stop |
  |---|---|---|---|
  | Windows | a value under `HKCU\…\CurrentVersion\Run` that runs `beam service start` | launched through PowerShell `Start-Process`, window hidden | `beam service stop` asks the agent to stop |
  | Linux | a `systemd --user` unit, `~/.config/systemd/user/beam-agent.service` | `systemctl --user start`, or a detached process without systemd | same; or `systemctl --user stop` (SIGTERM) |
  | macOS | not yet | a detached process | same |

  The login entry and the unit include `--beam-dir`, so the agent at login
  uses the identity of whoever enabled it.
* **Notifications** use what the OS already has, so no new dependency:

  | | Tool | Notes |
  |---|---|---|
  | Windows | Windows PowerShell 5.1 shows a toast through the built-in WinRT toast API, under PowerShell's own app id | needs no registration |
  | Linux | `notify-send` | needs a desktop session |
  | macOS | `osascript` | |

  At most one notification every 10 seconds. A notification only informs;
  it cannot accept.
* **Files under `~/.beam/`:**

  | File | Holds | Sensitivity |
  |---|---|---|
  | `agent.json` | the agent's process id, its local port, **the token**, its receive folder, its invite | **Private** (0600 on Unix; the user profile's permissions on Windows). Believed only while `agent.lock` is held; removed on a clean stop. |
  | `agent.lock` | nothing; held locked while the agent runs | not sensitive |
  | `agent.log` | what the agent did: requests, answers, results | private-ish: it names peers and files |
  | `config.toml` | `receive_dir`, `agent_port_mapping` | not secret |

---

## 3. Security

The question asked before building this: *does a background agent, even as
an option, make beam vulnerable?* The answer is: **not if it is built this
way.** It adds no new remote attack path. It lengthens the time beam is
reachable, which is handled by turning pairing off and leaving port mapping
off. It adds one local surface, the link to `beam inbox`, which is limited
to the same user.

### What could go wrong, and what stops it

| # | Threat | Mitigation | Test |
|---|---|---|---|
| A-1 | **Always reachable.** R-8 (a fixed port that can be found by scanning) is no longer limited to "while `listen` runs". | The agent **does not pair**; the pairing protocol is not even offered in the handshake, so nobody who finds it can try codes. Router **port mapping is off** unless turned on after a warning. | `the_agent_does_not_pair` |
| A-2 | **A stranger sends.** | Same as `listen`: the key the connection proved must be in `known_peers`, else it is refused before anything is asked. The inbox never hears of it. | `an_unpaired_device_is_refused_without_a_request` |
| A-3 | **Auto-accept through the back door:** accept from the notification, "always accept from alice", accept on timeout. | None exists. A notification cannot answer. Only an answer in `beam inbox` accepts, and silence is a no (rule 1, S-5). | `an_unanswered_request_expires_and_saves_nothing` |
| A-4 | **Another program answers for you** through the local link. This is the main new surface: answering there *is* the Accept. | The agent listens on **loopback only**. Before saying anything (not even that a request is waiting), it requires the **token** from `agent.json`: 32 random bytes, compared in constant time, in a file only you can read. A client gets 5 s to present it. Lines are capped at 64 KiB, and at most 8 clients may connect. | `a_client_without_the_token_learns_nothing_and_cannot_answer` |
| A-5 | **Two answers to one request**, from two inboxes, or a late answer. | The first answer takes the request; any other is told "too late" and changes nothing. | `the_first_answer_decides_and_a_second_is_too_late` |
| A-6 | **Running with system rights.** A root daemon writes files as root. A Windows service running as SYSTEM, started from a folder the user can write to (`%LOCALAPPDATA%`), would let anyone who can replace that file become SYSTEM: a classic privilege escalation. | **Per-user only.** No system service, no administrator or root rights, no `sudo`. | by construction (`service.rs`) |
| A-7 | **Text injection** through a file name, into the notification or the inbox. | The name is cleaned by `untrusted` first. Notification text travels in environment variables, never inside a command or script, and Windows XML-escapes it. | `notify::the_text_travels_as_data_not_as_code` |
| A-8 | **Notification flood** from a paired device. | One request at a time (the listener's single slot), at most one notification per 10 s, and each request still needs your answer. | by construction |
| A-9 | **A crashed agent's token left on disk.** | `agent.json` is believed only while `agent.lock` is held, and the OS releases the lock when the process dies. A clean stop deletes the file. | `status::a_leftover_status_file_is_not_believed`, `a_stop_request_stops_the_agent_and_takes_its_token_off_disk` |
| A-10 | **Turning port mapping on without understanding it.** | `beam service port-mapping on` prints a warning and needs `y`. Anything else leaves it off. | manual (section 5) |

### What remains (accepted risks)

* **R-9: presence is visible all day.** While the agent runs, it stays
  connected to its relay, so the relay (n0's, by default) can see when this
  device is online. This is R-3 for longer. `relay = "none"` avoids it, at
  the cost of direct-only connections.
* **R-8 with port mapping on.** If you turn it on, the agent's port can be
  found from the internet for as long as it runs. Finding it gives a public
  key and a connection that is refused unless the key is paired. With pairing
  off, there are no codes to try.
* **Same-user malware** can read `agent.json` and answer for you. It can also
  read your private key, which is already listed in SECURITY.md as out of
  scope. The token keeps out *other* users and remote machines, not
  programs running as you.

---

## 4. Limitations

* **Windows: a console window flashes briefly at login,** while `beam service
  start` runs from the login entry and launches the hidden agent. The agent
  itself has no visible window.
* **Linux was not run on a real Linux machine during development.** The
  Linux code paths (the systemd unit, `systemctl`, SIGTERM handling,
  `notify-send`) are compiled and unit-tested by CI on Arch Linux, and were
  reviewed. Run section 5 on Linux before relying on it.
* **Linux without a desktop** (a server): no notifications. The request is in
  `agent.log`, and `beam inbox` shows it. Under `systemd --user`,
  `notify-send` also needs the session's D-Bus address. Most desktop
  distributions provide it; if notifications do not appear, check
  `systemctl --user show-environment` for `DBUS_SESSION_BUS_ADDRESS`.
* **macOS:** no start-at-login yet. `beam service start` works after logging
  in.
* **An older `beam send`** waits only 60 s for an answer. With an agent, it
  gives up after 60 s even though the agent would wait 5 minutes. A current
  `beam send` waits the full time.
* **Settings are read when the agent starts.** After `beam receive-dir` or
  `beam service port-mapping`, restart the agent.
* **One agent per beam home,** and not at the same time as `beam listen`.
* **The agent saves to one folder.** The free-space check happens before you
  are asked (N-7), so the folder must be known in advance; there is no
  per-request "save as".

---

## 5. Testing

### Automated (`cargo test`)

| What | Test |
|---|---|
| A request is shown in the inbox with the usual fields, accepted there, and saved to the receive folder | `tests/agent.rs::a_request_is_accepted_in_the_inbox_and_saved_to_the_receive_folder` |
| Declining saves nothing | `tests/agent.rs::declining_in_the_inbox_saves_nothing` |
| No answer: expired, nothing saved, a late answer is "too late" | `tests/agent.rs::an_unanswered_request_expires_and_saves_nothing` |
| The first answer decides | `tests/agent.rs::the_first_answer_decides_and_a_second_is_too_late` |
| No token: silence, no answer accepted; a silent client is dropped | `tests/agent.rs::a_client_without_the_token_learns_nothing_and_cannot_answer` |
| The agent does not pair | `tests/agent.rs::the_agent_does_not_pair` |
| Strangers are refused unseen | `tests/agent.rs::an_unpaired_device_is_refused_without_a_request` |
| `service stop` stops it and removes the token | `tests/agent.rs::a_stop_request_stops_the_agent_and_takes_its_token_off_disk` |
| One agent per home | `tests/agent.rs::a_second_agent_in_the_same_home_is_refused` |
| Status file and lock; a crash leftover is not believed; the file is private | `agent::status::*` |
| Token randomness, exact comparison, strict messages, line limit | `agent::ipc::*` |
| Notification text is data, not code | `agent::notify::*` |
| The login entry and unit carry the right command and beam home | `agent::service::*` (plus an ignored Windows test that round-trips through the registry) |
| `receive_dir` and `agent_port_mapping` settings; editing one key keeps the rest | `config::*` |

### Manual (real processes)

Run with throwaway beam homes (`--beam-dir`) so your own `~/.beam` stays as
it is.

1. `beam receive-dir` shows the default (on Windows, the real Downloads
   folder, even if moved).
2. `beam receive-dir <folder>`, then `beam service start`. Then:
   - `beam service status` shows it running;
   - `beam whoami` mentions it;
   - `beam listen` refuses to start.
3. From a paired home, `beam send`. A notification appears. Then
   `beam inbox`: the prompt appears; `y`; the file arrives intact in the
   folder.
4. `beam service port-mapping on`: the warning appears; `n` leaves it off,
   and `y` turns it on.
5. `beam service stop`: the agent stops, and `agent.json` is gone.
6. On Linux, additionally:
   - `beam service enable`, then `systemctl --user status beam-agent`;
   - log out and in, and confirm it is running;
   - `beam service disable`.

Steps 1 to 5 were run on Windows on 2026-10-04, and all behaved as described.
The first run found that a directly spawned agent inherited
`service start`'s pipe handles, so a script reading that output hung until
the agent stopped. Starting the agent through `Start-Process` fixed it.

---

## 6. Questions

**Why not a real Windows service or a system daemon?** A Windows service runs
in session 0, where it cannot show you a notification. Running with system
rights would also be dangerous (A-6). A per-user agent can do everything this
feature needs.

**Why not accept from the notification?** Because the Accept has to show who,
the fingerprint, the file and the size, and has to be answered on purpose
(rule 1). A toast button would invite clicking without reading.

**Why a token over loopback, rather than a Unix socket or a named pipe?** One
code path for every platform, and no `unsafe` code: a Windows named pipe
restricted to one user needs a raw security descriptor. The token gives the
same "this user only" guarantee (A-4).

**Does it use more battery or data?** Very little while idle: keep-alives to
the relay every few seconds, as `beam listen` does.
