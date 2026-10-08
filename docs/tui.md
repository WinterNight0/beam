# The full-screen view: beam without typing commands

Typing `beam` with nothing after it opens a **full-screen view** in the
terminal, laid out like Discord's Friends page. You can see your friends,
send them files, answer what they send you, pair with someone new, and run any
other beam command from a palette at the bottom. Everything the command line
does still works the same way; the view is another way in.

It changes nothing about beam's rules. Every transfer is still accepted by
hand, pairing still needs the code and a fingerprint check, and a key change is
still a hard stop. Where the view asks a question, the safe answer is the one
already highlighted.

The decision record is ADR-0043 in [decisions.md](decisions.md). This page
covers how to use the view, how it works, what keeps it safe, its limits, and
how it is tested.

---

## 1. Opening it, and turning it off

```
beam                 # the full-screen view (on a terminal)
beam --help          # anything typed after `beam` is the normal command line
beam ui cli          # make plain `beam` print the help instead
beam ui tui          # bring the view back
beam ui              # which one plain `beam` opens now
```

* **The first time**, before this device has its identity, the view shows a
  **Welcome to beam** card: "This device doesn't have its beam identity yet
  … Create it now?" with **Create it** (highlighted) and **Not now**. Create
  it does what `beam init` does (the keys are made here, named after the
  computer, and the private key never leaves it), then opens Add friend.
  Not now leaves beam. An existing identity is never replaced.
* Plain `beam` opens the view **only on a real terminal**. If its output goes
  to a pipe or a file (a script), it prints the help as it always did.
* **Anything after `beam`** (a command, `--help`, even `--beam-dir x`) is the
  command line, unchanged. Scripts and the test suite never see the view.
* The choice is the `ui` line in `~/.beam/config.toml` (`"tui"` or `"cli"`;
  no line means `"tui"`). `beam ui` edits that one line and leaves the rest of
  the file alone.

---

## 2. The screen

```
 ◆ beam  WINTER-PC 111 222 333              ⇡ 63 % alice  ● 1 waiting  ● receiving
   Friends   Pending 1   Add friend
╭ FRIENDS — 3 ──────╮╭ @ alice ─────────────────────────────╮╭ Details ───────────╮
│▌ A  alice         ││ → report.pdf  sending 63 %  s to see ││  A  alice          │
│▌    785 807 217   ││ ← photo.png 3.0 MiB WAITING 4:12 left││ SHORT ID           │
│  B  bob           ││ FILES                                ││ 785 807 217        │
│     192 407 777   ││ ← notes.txt  2 KiB  saved ✓  2 h ago ││ FINGERPRINT        │
│  L  lab-pc        ││ → slides.zip 80 MiB sent ✓ yesterday ││ b9ef 5724 47d0 …   │
│     604 118 052   ││                                      ││ PAIRED  2026-09-30 │
│                   ││                                      ││ LAST SEEN 2 h ago  │
╰───────────────────╯╰──────────────────────────────────────╯╰────────────────────╯
  :  commands   ↑↓  friend   s  send   r  rename   x  remove   Ctrl+C  copy   ?  help
```

* **Header.** Your device and Short ID on the left. On the right, as they
  apply:
  - a blue **⇡ 63 % alice** while a file goes out;
  - a blue **⇣ 45 %** while one comes in;
  - a red **● 1 waiting** when a friend is asking to send;
  - green **● receiving** when the background agent runs, grey
    **○ not receiving** when it does not.
* **Friends.** Your paired devices, each with a coloured initial (the colour
  comes from the fingerprint, so it survives a rename) and the Short ID.
* **The middle panel** is the selected friend's inbox: a file going to them
  now, requests from them waiting for an answer, then every file that went
  either way, newest first.
* **Details.** The Short ID, the full fingerprint in groups of four (the way
  people read it out), when you paired, and when they were **last seen**. In a
  narrow terminal this panel folds into the middle one.
* **Pending** lists every waiting request. **Add friend** pairs.
* **The status bar** shows the keys that work right now, and short messages
  ("Copied alice's fingerprint.") on the right.

The view reads `~/.beam` again every two seconds, so a friend paired or an
agent started in another terminal shows up without a key press.

---

## 3. Keys and the mouse

| Key | Does |
|---|---|
| `:` or **Ctrl+P** | the command palette (section 4.5) |
| ↑ ↓, `k` `j`, the wheel | move in the list |
| Tab, ← →, `1` `2` `3`, a click | switch tab |
| **Enter** | answer a waiting request from the selected friend (or on Pending) |
| `s` | send a file to the selected friend; while one is going, show it |
| `r` | rename the selected friend |
| `x`, Delete | remove the selected friend |
| **Ctrl+C** | **copy** the fingerprint shown (or the invite, or a command's output) |
| Esc | close a pop-up; in the Add friend form, give the keys back to the page |
| **Ctrl+Q** | **leave beam**, from anywhere (`q` too, when not typing) |
| `?` | the key list |

**Ctrl+C copies; it does not quit.** That is how Fresh and most editors use
it. In a full-screen view the terminal sends Ctrl+C as an ordinary key, not as
a "stop" signal. To leave, press **Ctrl+Q**.

**The mouse.** Clicks pick tabs, friends, requests, palette lines and
buttons, and put the cursor where you click in a text box. While beam has the
mouse, the terminal's own drag-to-select is off; **hold Shift while dragging**
to select text anyway.

**Text boxes** (palette, rename, invite, name, code, browser filter) take typing,
← →, Home/End, Ctrl+← → by word, Backspace, Delete, Ctrl+Backspace or Ctrl+W
for a whole word, and paste. Thai, accents and emoji move as one character
each.

---

## 4. Doing things

### 4.1 Sending a file

1. Select the friend and press **`s`** (or type `:send alice report.pdf` to
   skip straight to sending).
2. A **file browser** opens where you last left it (the first time, in the
   folder beam was started from):

   ```
   ╭ Send a file to alice ───────────────────────────────────────────────╮
   │ PLACES        E:\Projects\reports                                    │
   │  Home         ╭ type to filter, or a path ──────────────────────────╮ │
   │  Desktop      │ q3                                                  │ │
   │  Documents    ╰─────────────────────────────────────────────────────╯ │
   │  Downloads    ▌q3-report.pdf                    2.0 MiB  2 days ago  │
   │  This folder   q3-slides.pptx                  14.1 MiB  yesterday   │
   │ DRIVES                                                               │
   │  C:  D:  E:                                         Send    Cancel   │
   ```

   * **Left:** your usual folders, then **every drive** (on Linux and macOS:
     `/` and mounted disks). **Tab** switches sides; Enter or a click jumps.
   * **Right:** the folder. Folders come first, then files, in natural order
     (`report-2` before `report-10`), with sizes and how old each file is.
     Hidden and system files stay hidden; type a `.` to see dot files.
   * **Type** to filter. **Enter** opens a folder or sends the file;
     **Backspace** (with nothing typed) or ← goes up a folder; `..` does too.
     A click picks a line; a second click opens or sends it. The wheel scrolls.
   * **Type or paste a path** (anything with a slash, a drive like `D:`, or
     `~`) and Enter goes straight there. **Dragging a file onto the window**
     pastes its path, without the quotes Windows adds: Enter sends it.
   * A folder that cannot be opened says why, and the browser stays where it
     was.
3. Once a file is chosen, a pop-up follows the send:
   - looking for alice…
   - reading the file (on a big file this takes a moment)
   - waiting for alice to accept: **nothing is sent until they say yes**
   - sending, with a bar and `[Direct P2P]` or `[Relay]`
   - alice is checking the file
   - ✓ *Sent 2.0 MiB to alice, saved on their side as report.pdf.*

While it runs, **Esc hides** the pop-up (the header keeps the progress, and
`s` brings it back), and **Cancel send** (or `x`) stops it. alice is told who
stopped it and keeps what arrived, so sending the same file again resumes.
Leaving beam mid-send asks first.

One file at a time, and files only, not folders.

### 4.2 Answering a request

When a friend sends you something, you see it at once, wherever you are: the
red **● 1 waiting** in the header, **Pending 1** on the tab, a line in the
status bar, and a **WAITING** row in that friend's panel.

A request **never opens a pop-up by itself**. Otherwise a request arriving
while you type would catch a stray Enter. Open it with **Enter** (on the
friend, or on Pending) or a click. The pop-up shows what `beam listen` shows:

```
╭ Incoming file ─────────────────────────────────────╮
│ alice wants to send you a file.                    │
│ File   report.pdf                                  │
│ Size   2.0 MiB                                     │
│ FROM THE DEVICE WITH FINGERPRINT                   │
│ b9ef 5724 47d0 9971 …                              │
│ Answer within 4:12; no answer means no.            │
│                               Accept    Decline    │
╰────────────────────────────────────────────────────╯
```

It **starts on Decline**: Enter alone declines, and `d` declines too.
Accepting takes a deliberate ← then Enter, or a click on **Accept**. **Esc**
means "later": the request keeps waiting, and if nobody answers it expires as
a no. A resumed transfer shows what is already here, and needs a yes again.

### 4.2a Receiving: the switch at the top of Pending

Nobody can send you a file unless something on your device is receiving.
The top of the Pending tab is a switch for that:

```
╭ Pending ─────────────────────────────────────────────────────────────╮
│  RECEIVING   ○  OFF   Turn on so friends can send you files          │
│              press o, or click · pairing stays in Add friend         │
│ ──────────────────────────────────────────────────────────────────── │
│ Nothing can arrive while Receiving is off.                           │
```

* **`o`** (from any tab) or a click turns it on. While it is **off**, it is
  drawn in amber so it is noticed; the header's **○ not receiving** is
  clickable and leads here.
* **On** (green): friends can send you files **while beam is open**. Requests
  come into this tab and the same Accept pop-up, files go to your receive
  folder (`beam receive-dir`), and a desktop notification says when a request
  arrives, in case beam is behind another window.
* It **stops when you turn it off or leave beam**, and it is off every time
  beam starts: your device is never reachable without you choosing it that
  day. Turning it off, or leaving, while a file is arriving asks first
  (starting on Keep); the sender is told and keeps what arrived (ADR-0041).
* **Pairing stays in Add friend**: the switch does not show a pairing code.
* If the **background agent** already receives (`beam service enable`), the
  strip says so and there is nothing to switch; it keeps receiving when beam
  is closed. If `beam listen` runs in another terminal, the strip says that
  too, and its requests are answered there.

Under the hood the switch runs the background agent's receiver inside the
view for as long as it is on, with the same rules (section 6, ADR-0044).

### 4.3 Pairing with someone new

**They gave you an invite:** open **Add friend** (the cursor is already in
the invite box), paste it, press Tab, type a name for them, and press Enter
(or click **Pair**). Then:

1. a pop-up asks for the **6-digit code** shown on their screen. Only digits
   go in, and a code of the wrong length is caught before anything is sent;
2. both of you see **Check fingerprints**: theirs and yours. Read them out to
   each other, or compare screens. It **starts on No**;
3. when both say they match, they are your friend, selected in the list.

**You want to give them yours:** **Show my invite** shows your invite, the
code and a countdown. **Copy invite** (or Ctrl+C) copies it; send it any way
you like. When they connect, the pop-up says so and the fingerprint check
follows. The new friend is named after what their device suggests; rename
them with `r`.

Esc or Cancel stops a pairing at any point, and nothing is saved.

**An invite from someone already your friend** only updates where to find
them. Their key never changes this way. If it would move them to a different
relay, a pop-up asks first and starts on **Keep** (ADR-0038).

### 4.4 Renaming and removing

`r` opens a box with the current name; only your label changes, and they are
not told. `x` asks **Remove alice?** with their fingerprint, and **starts on
Keep**. To be friends again after a removal, you pair again.

### 4.5 Every other command: the palette

**`:`** or **Ctrl+P** opens a box at the bottom with a list above it.

* Type a command exactly as on the command line (`service status`,
  `send alice "C:\My Files\a.zip"`; a leading `beam` is ignored), or a few
  letters and Enter (`who` runs `whoami`).
* An unfinished command fills itself in and waits (`send` + Enter becomes
  `send alice `). **Tab** completes friends and file paths. An empty palette
  shows your recent commands first.
* A wrong command stays open with the reason, in the command line's own
  words.

Each line shows **where it runs**:

| Badge | Commands | What happens |
|---|---|---|
| **here** | `peers`, `whoami`, `history`, `transfers`, `service status`, `service start/stop/enable/disable`, `receive-dir`, `ui`, `help` | Runs inside the view; what it prints opens in a scrollable pop-up (Ctrl+C copies it). |
| **pop-up** | `send`, `rename`, `remove`, `quit` | The view's own screens (sections 4.1 and 4.4). |
| **terminal ↗** | `pair`, `listen`, `inbox`, `init`, `transfers --clear`, `history --clear`, `service port-mapping on`, `send` with a developer flag | The view steps aside and the command runs as **its own beam process** in the normal terminal, with its usual questions and warnings. "Press Enter to go back to beam" returns. |

### 4.6 History and "last seen"

beam keeps a record of every transfer that reached a person, in
`~/.beam/history.jsonl` (section 5.4). The friend's panel shows theirs, and
the palette's `history` (or `beam history`) shows everyone's:

```
WHEN       WHO            FILE        SIZE     RESULT
2 h ago    from alice     photo.png   3.0 MiB  declined
yesterday  sent to alice  slides.zip  80 MiB   sent (saved as slides.zip)
```

**Last seen** is the last time that friend was actually there: a transfer
that went through, was declined, or was stopped once it had reached them. A
failed attempt to reach them, or a send cancelled before it reached them,
does not count. There are **no "online" dots**: beam has no server that could know,
and asking each friend "are you there?" would tell every paired device when
you are online. Times are relative ("5 min ago", "yesterday"), which needs no
time zone.

---

## 5. How it works

### 5.1 The pieces

```
crates/beam/src/tui/
  mod.rs        the terminal: setup and restore, the event loop, carrying out effects
  app.rs        all the state, and what each key and click does to it (no I/O)
  view.rs       draws the state; records where each clickable thing landed
  input.rs      one text box with a cursor
  palette.rs    splitting, checking, listing and placing palette commands
  add.rs        the Add friend form and the pairing pop-ups
  pairing.rs    pairing::join / wait on a background thread
  pending.rs    the Pending tab, the Accept pop-up and the Receiving switch
  receiving.rs  the agent's receiver, run while the switch is on
  inbox.rs      the link to the background agent, as `beam inbox` has it
  send.rs       opening the browser, and the send pop-up
  browse.rs     the file browser: places and drives, folders, filter, the disk
  sending.rs    `beam send` on a background thread
  clipboard.rs  Ctrl+C
crates/beam/src/history.rs   history.jsonl
```

Drawing uses **ratatui** with its **crossterm** backend, the same pair as the
Fresh editor; crossterm comes through ratatui so the two cannot disagree.

### 5.2 State in, effect out

`app.rs` never touches the terminal, the disk or the network. A key, a click
or a paste goes in; the state changes; anything that must happen outside
(save a rename, copy, start a send, answer a request) comes back as an
**effect**, which `mod.rs` carries out and answers with a **done**. That is
what makes the behaviour testable without a terminal: almost every rule on
this page is a unit test on `App`.

`view.rs` draws whatever the state is. While drawing, it records where each
clickable thing landed, so a click is matched against the real layout, not a
guess about it.

### 5.3 Long work runs on threads

Three things take time, and each runs on its own thread with its own tokio
runtime, talking to the view through channels:

| Worker | Runs | The view sends | The view gets |
|---|---|---|---|
| `pairing.rs` | the same `pairing::join` / `pairing::wait` as `beam pair` | the code; whether the fingerprints match | invite and code, "type the code", "do they match?", the result |
| `inbox.rs` | the `beam inbox` link to the agent: loopback, token from `agent.json` | accept or decline request *n* | requests, closed, progress, finished, gone |
| `sending.rs` | the same dial, `send_on` and history line as `beam send` | cancel | each stage and its progress, the result |

The view never waits on them: it checks their channels ten times a second
while they work, so the screen stays responsive. Dropping a worker cancels
it: a pairing stops, a send closes its connection the way Ctrl+C does in
`beam send`, and a question still waiting reads as no.

### 5.4 History

`beam send` writes the sender's line, and the code behind both `beam listen`
and the background agent writes the receiver's, so every way of moving a file
is recorded. The file is rewritten atomically under a lock (`history.lock`),
so `listen`, the agent and `send` writing at once do not lose each other's
lines, and only the newest 1000 entries are kept. Writing history can never
fail a transfer.

### 5.5 Leaving

On every way out (Ctrl+Q, an error, a crash), the terminal is put back: raw
mode off, the normal screen back, mouse capture and bracketed paste off. A
crash restores it before the panic message is printed. A command stepped out
to the terminal gets the normal screen while it runs, and the view comes back
after Enter.

---

## 6. Security

The view adds no network surface of its own: everything it does on the
network is the existing `pair`, `send` and `inbox` code. What it adds is a
**new way to give answers**, so the work was making every answer as
deliberate as typing `y` at the command line.

### What could go wrong, and what stops it

| # | Threat | Mitigation | Test |
|---|---|---|---|
| V-1 | **Accepting by accident:** a request arrives while you type, and Enter lands on it. | A request never opens a pop-up by itself. The Accept pop-up starts on **Decline**; accepting takes a move to Accept, or a click on it. Esc leaves the request waiting, and expiry is a no (rule 1, S-5). | `a_request_is_listed_and_announced_but_opens_nothing_by_itself`, `the_accept_pop_up_starts_on_decline`, `accepting_takes_a_deliberate_move_to_accept`, `esc_leaves_the_request_waiting` |
| V-2 | **A back door to accept** through the palette, or a command the CLI would refuse. | The palette accepts only what the CLI's own parser accepts (`cli::check`). There is no accept command. Commands that ask questions run as a separate beam process with their reviewed prompts unchanged. | `nothing_typed_in_the_palette_can_accept_a_transfer`, `commands_run_where_they_belong`, `a_wrong_command_says_why_and_stays_open` |
| V-3 | **The receiver's answer is not the one that counts:** two places answer, or a late answer. | The view uses the agent's own link (ADR-0042 A-4, A-5): the first answer wins, a late one is refused and the view says so, and an answered request leaves the list at once. | `accepting_in_the_view_saves_the_file`, `declining_in_the_view_saves_nothing`, `a_request_closed_elsewhere_closes_its_pop_up_and_says_so` |
| V-4 | **Pairing without really checking.** | The real `pairing::join` / `wait` run unchanged. The fingerprint check starts on **No**. A malformed code is caught before the network, so a typo is never a guess. Cancelling saves nothing. A question nobody answers is a no. | `two_views_pair_through_the_real_protocol_and_both_save`, `a_no_on_either_side_saves_nothing_on_both`, `the_fingerprint_check_starts_on_no`, `the_code_pop_up_takes_digits_and_never_sends_a_malformed_code`, `an_unanswered_or_abandoned_question_is_a_no` |
| V-5 | **Moving a friend to an attacker's relay** with a forged invite pasted into Add friend. | As in `beam pair` (ADR-0038): their key never changes this way, and a relay change asks first, starting on **Keep**. Own invites and taken names are refused before the network. | `a_relay_change_starts_on_no`, `pair_checks_the_invite_and_the_name_before_any_network` |
| V-6 | **The background agent pairing** because the view can show an invite. | "Show my invite" runs its own short pairing on its own endpoint; if the agent holds port 7820, it falls back to a temporary one. The agent never offers pairing (ADR-0042 A-1). | by construction; `tests/agent.rs::the_agent_does_not_pair` |
| V-7 | **Terminal injection** through a file name, a nickname, an error or a command's output, now in a full-screen view. | Everything from outside is cleaned by `untrusted` before it is drawn (ADR-0034): when `~/.beam` is read, when a request arrives, when output is captured. Pasted text is cleaned to one line. ratatui also never passes control characters through. | `a_request_is_cleaned_and_its_resume_described`, `a_paste_is_one_clean_line`, `tests/injection.rs` |
| V-8 | **Removing a friend, or losing a send, by accident.** | Remove starts on **Keep**. Leaving mid-send asks first, starting on **Stay**; leaving anyway tells the receiver, who keeps what arrived. | `remove_starts_on_keep_so_enter_alone_removes_nothing`, `leaving_mid_send_asks_first_and_no_is_the_default`, `cancelling_while_they_decide_tells_them_and_is_recorded` |
| V-9 | **Keystrokes going to the wrong place:** `q` quitting or digits switching tabs while typing an invite. | Text boxes take all printable keys; only Ctrl+Q, Ctrl+P and Esc act on the page. | `typing_in_the_form_does_not_switch_tabs_or_quit_and_esc_gives_keys_back`, `ctrl_q_quits_from_anywhere_and_q_only_from_the_page` |
| V-10 | **The history file as a leak or a target:** it names files and peers, and a stranger could try to fill it. | Private (owner-only). Not written for anything refused before a prompt. Newest 1000 only. Cleaned before it is shown. `beam history --clear` deletes it. | `history::tests::*`, `send_to_a_peer_that_is_not_listening_says_how_to_re_pair` (`tests/cli.rs`) |
| V-11 | **Copying through a command line** where other programs could see it. | The text goes to the OS clipboard tool on its **standard input**, never as an argument. `clip.exe` gets UTF-16 so nothing is garbled. Only an explicit Ctrl+C copies. | `clip_exe_gets_utf16_with_a_byte_order_mark` |
| V-12 | **A broken terminal** after a crash: raw mode, mouse codes on screen. | A panic hook turns mouse capture and paste mode off, then ratatui's restores the screen. | by construction (`tui::run`) |
| V-13 | **The Receiving switch makes the device reachable** without the person realising, or longer than meant. | Off at every start, never remembered; on only by `o` or a click; drawn in amber while off and green while on; stops when beam closes. It runs the agent's receiver: pairing off, router port mapping only if turned on for the agent, five-minute answer window, Accept starting on Decline (ADR-0044). | `the_switch_turns_on_and_off_and_says_so`, `with_the_switch_on_a_friend_sends_and_the_view_accepts` |
| V-14 | **Two receivers on one identity** (the switch, the agent, `beam listen`) splitting requests between them. | The switch takes the agent's lock: a second switch or agent is refused, and `beam listen` refuses while it is on; the switch will not start beside either. | `the_switch_starts_a_receiver_that_shuts_out_listen_and_stops_cleanly`, `it_will_not_start_beside_a_running_listen`, `the_switch_will_not_fight_the_agent_or_listen` |
| V-15 | **Losing a file by switching off** mid-transfer. | Turning off or leaving while one arrives asks first, starting on Keep; the sender is told, and the partial is kept for a resume. | `turning_off_mid_transfer_asks_first_and_no_is_the_default`, `leaving_while_a_file_arrives_asks_first` |

### What remains (accepted risks)

* **Anything on your screen can be seen.** The view shows friends' names,
  fingerprints and file names, as `beam peers` and `beam history` do.
* **What you copy is on the clipboard,** where other programs you run can
  read it. Invites and fingerprints are public by design; a command's output
  may name files.
* **Same-user malware** can read `history.jsonl`, as it can read `agent.json`
  and your private key (SECURITY.md, "not protected against").

---

## 7. Limitations

* **One file at a time, and no folders.** Zip a folder first.
* **The browser shows at most 5000 entries** of one folder; type to find the
  rest. Network places that are not mapped to a drive letter are not listed
  (type or paste their path instead).
* **The Receiving switch lasts only while beam is open.** To receive when it
  is closed, use the background agent (`beam service enable`). With
  `beam listen` running instead, its requests are answered in that terminal.
* **No "online" status,** by design (section 4.6).
* **Palette history lasts until you leave beam.**
* **Mouse capture turns off normal text selection;** Shift+drag still works.
* **The old Windows console** (`conhost`, not Windows Terminal) may show the
  colours less faithfully. Copying uses `clip.exe` on Windows, so it works
  there too.
* **On a Linux server with no clipboard tool,** copy falls back to the
  terminal (OSC 52). Most modern terminals honour it, including over SSH;
  some do not.
* **Show my invite while the agent runs** uses a temporary port. The friend
  saves that address, so later sends find you through the relay until you
  give them an invite from `beam listen`; nothing breaks, it is only less
  direct at first.
* **Linux was checked by CI and review,** not run by hand on a Linux desktop,
  as for the background agent.

---

## 8. Testing

### Automated (`cargo test`)

About a hundred tests, in four layers:

* **Behaviour, without a terminal** (`tui::app`, `pending`, `send`, `add`,
  `palette`, `input`): every key and click rule on this page, including each
  "starts on No".
* **Screens**, drawn into ratatui's in-memory `TestBackend` and read back as
  text (`tui::view`): the header badges, the friend's panel, every pop-up, a
  narrow terminal, and a 1×1 terminal that must not crash. Clicks are tested
  against where things were actually drawn.
* **Real protocol, real processes' code** on loopback:
  - two views **pair** each other (yes on both sides; no on one side);
  - a real **background agent** gets a request, and the view **accepts**
    (file saved) or **declines** (nothing saved);
  - the view **sends** to a real agent: accepted, declined, and cancelled
    while the other person decides.

  Each also checks the history line.
* **Commands:** `beam ui`, "arguments never open the view", `beam history`,
  and history written by a real `beam send`.

Run them with `cargo test`, or one area with `cargo test -p beam --lib tui`.

### Manual (two terminals, throwaway beam homes)

On Windows 11 with Windows Terminal. Each step was checked by the team in a
real terminal before the next was built (step 8, the Receiving switch, is
next to be checked by hand; its end-to-end path is an automated test):

1. `beam` opens the view; `beam | more` prints help; `beam ui cli` / `tui`.
2. Mouse, Ctrl+C copy (paste it elsewhere), Ctrl+Q, rename, remove.
3. Palette: `:peers` (here), `:transfers --clear` (terminal, then back),
   Ctrl+C inside a stepped-out command.
4. Add friend between two homes: Show my invite in one, paste and type the
   code in the other, compare fingerprints.
5. With `:service start` in one home, `:send` from the other; the request
   appears, is answered from Pending, and progress shows.
6. History and last seen after those transfers; `:history`.
7. `s`, drag a file in, Hide and `s` again, Cancel send, quit mid-send.
8. Receiving: with no agent, press `o` in one home (amber OFF turns green ON),
   send from the other, answer in Pending; `o` mid-transfer asks first;
   Ctrl+Q while on stops it (`beam service status` says not running);
   `beam listen` in a third terminal refuses while it is on.

---

## 9. Questions

**Can I get the old behaviour back?** `beam ui cli`. Plain `beam` then prints
the help, and nothing else changes.

**Why doesn't Ctrl+C quit?** In the view it copies, as in editors. Ctrl+Q
quits. On the command line (`beam send`, `beam listen`), Ctrl+C still stops
and tells the other side (ADR-0041).

**Why didn't the request pop up?** So a key you were already pressing cannot
answer it. Look for the red badge, and press Enter.

**Is answering in the view the same as `beam inbox`?** Yes: the same link to
the agent, the same token, the same rule that the first answer counts. You
can have both open; whichever answers first decides.

**Where are my transfers?** In the friend's panel, in `:history`, and in
`~/.beam/history.jsonl`. `beam history --clear` deletes the record (not the
files).
