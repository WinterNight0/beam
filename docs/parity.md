# Which front end offers what

beam has one core and several front ends (ADR-0048): the command line
(**CLI**), the full-screen view (**View**, ADR-0043) and, later, a **GUI**
(SPIKE-002). A feature is written once, in `beam-core` (the flow) and
`beam-model` (what a screen shows), and each front end then offers it in its
own way. This table says which front end offers which feature, so a gap is
written down rather than forgotten.

The security rules do not depend on this table. They are enforced in the
core, so a front end that lacks a feature simply does not offer it; it can
never offer it less safely.

**Key:** ✔ offered on its own screen · ⌨ in the View through the command
palette (`:` or Ctrl+P), which runs the CLI command in the view or in a
terminal · — not offered · *planned* not built yet.

| Feature | CLI | View | GUI | Shared code |
|---|---|---|---|---|
| Create this device's identity | `beam init` | ✔ first-run welcome | *planned* | `identity` |
| See this device: name, fingerprint, invite | `beam whoami` | ✔ header; ⌨ `whoami` | *planned* | `identity`, `listen_status` |
| List friends, last seen | `beam peers` | ✔ Friends tab | *planned* | `beam-model::snapshot` |
| Rename / remove a friend (Remove starts on Keep) | `beam rename`, `beam remove` | ✔ `r`, `x` | *planned* | `beam-model::ask` |
| Pair by pasting an invite (fingerprint check starts on No) | `beam pair <INVITE>` | ✔ Add friend | *planned* | `actions::pair`, `beam-model::ask` |
| Pair by showing your invite | `beam pair --wait`, `beam listen` | ✔ Show my invite | *planned* | `actions::pair` |
| Update where a friend is (a relay move needs a yes) | `beam pair <INVITE>` | ✔ Add friend | *planned* | `actions::pair::save_location` |
| Send a file | `beam send` | ✔ `s` + file browser; ⌨ `send` | *planned*, with the OS file picker | `actions::send`, `beam-model::sending` |
| Receive with a prompt per file | `beam listen` | ⌨ `listen` (in a terminal) | *planned* | `listener` |
| Receive while open, without a terminal | — (use `beam listen`) | ✔ Receiving switch | *planned* | `actions::receive`, `beam-model::requests` |
| Answer requests (Accept starts on Decline) | `beam inbox` | ✔ Pending tab | *planned* | `actions::inbox`, `beam-model::ask` |
| Background agent: enable, start, stop, status | `beam service …` | ⌨ `service …` | *planned* | `agent` |
| Router port mapping for the agent (asks first) | `beam service port-mapping on` | ⌨ (in a terminal) | *planned* | `agent` |
| Where received files go | `beam receive-dir` | ⌨ `receive-dir` | *planned* | `config` |
| History, and clearing it | `beam history [--clear]` | ✔ Friends panel; ⌨ `history` | *planned* | `history`, `beam-model::snapshot` |
| Partial transfers, and clearing them | `beam transfers [--clear]` | ⌨ `transfers` | *planned* | `transfer::partial` |
| Choose what plain `beam` opens | `beam ui cli\|tui` | ⌨ `ui` | — | `config` |
| Copy, paste, mouse in old consoles | — | ✔ Ctrl+C / Ctrl+V, `m` (ADR-0047) | — | — |

## Keeping it current

* A new feature adds a row here in the same patch, with ✔ for each front
  end it ships in and *planned* for the others (CLAUDE.md, "Adding a
  feature").
* The CLI always has every feature: it is the reference front end, and the
  View's palette reaches the rest through it.
* A front end may lag; it may never weaken a rule. If offering a feature in
  a front end would need a rule changed, the rule stays and the feature
  waits.
