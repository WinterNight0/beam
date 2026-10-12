# SPIKE-002 — GUI library

**Run:** 2026-10-08 · **Status:** complete, awaiting a decision · **Branch:** `GUI-Experimental`

Candidates considered: iced, Slint, egui, Tauri, JavaFX. Prototype built: **iced**.

**Recommendation: iced 0.14, pinned exactly, with `rfd` for the OS file
picker.** The reasons and the costs are below.

---

## Why a GUI

Even the full-screen view (ADR-0043) assumes someone comfortable in a
terminal. A GUI is for everyone else. It is planned as a second download that
also contains the `beam` CLI/TUI; the CLI/TUI ships first and stays the
reference.

## Candidates

| | **iced** | Slint | egui | Tauri | JavaFX |
|---|---|---|---|---|---|
| Language | Rust | Rust + `.slint` markup | Rust | Rust + HTML/JS | Java |
| Look out of the box | modern, clean | modern (Fluent/Material) | plain, tool-like | anything (web) | dated unless styled |
| Licence | MIT | royalty-free with a credit, or GPL | MIT | MIT/Apache | GPL + classpath |
| Shape of the code | Elm: message → `update` → `view` | properties, bindings, callbacks | redraw every frame | web front end + Rust commands | FXML + controller |
| Reaches beam's Rust code | directly | directly | directly | through commands | **no**: a separate process |
| Maturity | **pre-1.0, breaks every minor** | 1.x | 0.x, fairly stable | 2.x | mature |
| Extra toolchain | none | none | none | **Node/npm** | **a JVM in the download** |

**JavaFX was ruled out** although the team knows it: the GUI would have to
drive `beam` from outside, and everything the TUI already models (friends,
pending, the safe defaults of the pop-ups) would be written a second time in
Java. Tauri was ruled out for the same second-language cost, plus an npm supply
chain to audit. Between the three Rust libraries, iced was prototyped
because its architecture is the one beam already uses (below) and it looks
modern without effort; Slint is the runner-up.

## What was built

`spike/gui-iced/` (detached from the workspace, throwaway, 326 lines):

* a Discord-Friends-style window: sidebar of friends, a detail pane;
* the friends are the **real** ones from `~/.beam` (`$BEAM_DIR`), read through
  the unchanged `beam` library (`Store::load_known_peers`);
* "Simulate a request" opens the **Accept pop-up**: sender, file, size and
  fingerprint; **Decline is the highlighted button, and Enter or Escape
  decline**; Accept takes a click on Accept;
* "Send…" opens the **OS file picker** (`rfd`) and shows the chosen path
  (nothing is sent);
* a dark/light switch; no console window in release builds on Windows.

## Findings

| Question | Answer |
|---|---|
| Builds with MSRV 1.91, `unsafe_code = forbid`, clippy `-D warnings`, rustfmt? | **Yes**, first time. iced 0.14 needs Rust 1.88. |
| Fits beam's architecture? | **Yes, closely.** iced's `Message` → `update` → `Task` is `tui::app`'s `Event` → `Effect`. Background work is a `Task` (async, on tokio) or a `Subscription` (a stream, e.g. a worker's events). |
| OS file picker? | **Yes**: `rfd::AsyncFileDialog` inside a `Task`, the window stays responsive. |
| Testable without a screen? | **Yes**: `iced_test::simulator` clicks buttons by their text and finds text in the view. 5 tests: friends list and selection, the pop-up shows who/what/fingerprint, Enter/Escape decline, Accept needs a click, the empty state. |
| Runs without a GPU? | **Yes**: iced falls back to its software renderer (tiny-skia); `ICED_BACKEND=tiny-skia` forces it. Both opened the window here. |
| Memory (window open, idle) | **219 MB** with the GPU renderer, **33 MB** with the software one. For a window this simple, the software renderer is worth considering as the default. |
| Release exe size | **11.8 MB** for the spike (it uses only `Store`, so most of beam is left out). A real GUI includes iroh: expect roughly beam's 18.8 MB plus 10 MB. |
| Dependencies | 514 crates in the spike's tree against 382 for beam: iced, its renderers and windowing add about 130. |
| Build time (clean, this machine) | debug 2 min 06 s, release 3 min 08 s, beam included. |
| Arch Linux | **Not run yet.** Needs the Arch CI job (and a desktop for the window itself). Wayland and X11 are both in iced's defaults; `rfd` uses the XDG desktop portal, present on KDE/GNOME, possibly missing on a bare window manager. |
| `cargo audit` | Not run (not installed locally); the CI audit job covers it once the GUI is in the workspace. |

## Costs to accept

* **Breaking changes.** Pin `iced = "=0.14.0"` (and `iced_test`, `rfd`)
  like iroh; each upgrade is its own reviewed commit and short ADR.
* **Thin documentation.** iced says so itself; learning is from examples and
  type signatures. The spike needed no workaround, but it is small.
* **No visual designer.** Layout is Rust code (`column![…]`, `row![…]`).

## What would reopen the decision

* iced fails on Arch (Wayland or X11) in a way that cannot be configured away.
* An iced upgrade we need costs more than a few days.
* Then: Slint (closest to JavaFX's FXML), after checking its licence.

## Next steps, if accepted

1. ADR-0048 records the choice.
2. Split the workspace: `beam-core` (library), `beam` (CLI/TUI), `beam-gui`.
   No user-visible change.
3. Move the TUI's background workers and the shared model out of `tui/` so
   the GUI uses the same code (one place for the safe defaults).
4. Build the GUI first version; release it separately (tag `gui-v*`), bundled
   with the `beam` it was built with.
