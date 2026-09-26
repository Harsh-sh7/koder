# Warp (`warpdotdev/warp`) — the design reference

Warp is the terminal that this application's interface is modelled on: the
block-based terminal, the command palette, the panes, and the agent panel are all
borrowed from it deliberately. **Nothing in this repository builds, links, or
reads the Warp checkout.** It is a reading reference for UI work, and it was
removed from the tree to keep the handover small.

> **Restore it only when you want to research the interface:**
>
> ```sh
> git clone --depth 1 https://github.com/warpdotdev/warp.git WARP/warp
> ```

| Fact | Value |
|---|---|
| Upstream | <https://github.com/warpdotdev/warp> |
| Checkout path (this repo's convention) | `WARP/warp` |
| Ref consulted while building | commit `5af88f49f` (`repo-sync/watermark/private-to-public-327-…`) |
| What it is | "an agentic development environment, born out of the terminal" — a Rust desktop app, open-sourced |
| Licenses | UI framework (`crates/warpui_core`, `crates/warpui`) **MIT**; the rest **AGPL v3** (`LICENSE-AGPL`, `LICENSE-MIT`) |
| Build | `./script/bootstrap`, `./script/run`, `./script/presubmit` |
| Community docs | <https://docs.warp.dev>, <https://www.warp.dev/blog/how-warp-works> |

**Licence caution before copying code:** the UI framework crates are MIT and
safe to learn from (and, with attribution, to reuse); everything else is AGPL v3,
which is why this project borrows *concepts and layout*, never source. Our app is
egui, so almost nothing is directly portable anyway.

---

## 1. What we borrow, and where it lives upstream

Paths in the table below are relative to the Warp repository root — `WARP/warp/`
after the clone command in §4, so `crates/warp_terminal/src/model/` lives at
`WARP/warp/crates/warp_terminal/src/model/`.

| Our surface | Warp's counterpart | Where to read it |
|---|---|---|
| **Blocks** in the terminal pane — one card per command, with its command line, output, exit code, and duration (`App/native/crates/harness-core/src/term.rs`, `panes/terminal.rs`) | the block model: a grid of blocks, each with an id, an index, and a filter | `crates/warp_terminal/src/model/` (`blockgrid.rs`, `block_id.rs`, `block_index.rs`, `block_filter.rs`) |
| **Shell integration**: the harness drives the user's own login shell and learns command boundaries from it | the same idea — Warp injects shell hooks so the terminal knows where a command starts and ends | `crates/warp_terminal/src/shell/`, `crates/warp_terminal/src/bootstrap.rs` |
| **Local PTY ownership** (`App/native/crates/harness-core/src/term.rs`) | `local_tty`, `writeable_pty` | `crates/warp_terminal/src/local_tty/`, `…/writeable_pty/` |
| **Command palette** (`⌘K`) and **go-to-file** (`⌘P`) — `App/native/crates/harness-app/src/palette.rs` | the command search / palette | `crates/command/`, `crates/warp_completer/`, `crates/fuzzy_match/` |
| **Editor pane** with syntax highlighting (`panes/editor.rs`, `code.rs`) | the editor crate (rope, syntax, vim mode) | `crates/editor/`, `crates/syntax_tree/`, `crates/languages/`, `crates/vim/`, `crates/sum_tree/` |
| **Session / agent panel**: live activity, sub-agents, background processes | the agent panel and its session views | `crates/ai/`, `crates/ai_types/`, `crates/warp_multi_agent_client/` |
| **The app shell**: sidebar, centre card, panes, floating cards, toasts | Warp's own window and panel framework | `crates/warpui/`, `crates/warpui_core/`, `crates/warpui_extras/`, `app/` |
| **Git pane** (diff, stage, commit) | the code-review / diff surfaces | `crates/warp_files/`, `crates/repo_metadata/`, `crates/git`-adjacent code in `app/` |
| **Search pane** | ripgrep-backed search | `crates/warp_ripgrep/`, `crates/warp_search_core/` |
| **Settings / model dialog** | settings + AI configuration | `crates/settings/`, `crates/settings_value/`, `crates/ai/` |
| **Markdown-ish rendering of agent replies** | markdown parsing and rendering | `crates/markdown_parser/` |
| **Terminal themes / light palette** | theme definitions | `crates/warpui/src/`, `crates/warp_terminal/src/` (themes), `crates/warp_assets/` |

Repo-wide conventions worth reading before UI work in our app: `AGENTS.md`,
`CONTRIBUTING.md`, and `.warp/` (their own agent workflows).

## 2. What our UI takes from Warp, stated as rules

These are the borrowed ideas that are already implemented in
`App/native/crates/harness-app` — keep them when you extend the interface:

1. **A command's output belongs to the command.** Output is not a stream of
   lines appended to a scrollback; it is a block with an owner, an exit code, a
   duration, and a hover affordance ("open what this row is running", "attach
   this output to the agent").
2. **The prompt is never far away.** The composer stays docked at the bottom of
   the centre card, and the transcript above it is the record.
3. **Work in flight is always visible.** A moving element, a name for the work,
   and an elapsed count — at the end of the transcript and at the top of the
   session panel. A blank panel during a long command is a bug.
4. **Chrome is quiet; content is loud.** Flat surfaces, one accent colour, cards
   with a single edge. No gradients, no decoration that does not carry state.
5. **Keys are the fast path, the pointer is the slow path.** `⌘K` palette,
   `⌘P` files, `⌘1…6` views, `⌘O` folder, `⌘,` models, `⌘↩` send — and every one
   of them is also reachable by clicking something visible.

## 3. Where our implementation differs, and why

| Warp | This harness | Why |
|---|---|---|
| A full terminal emulator with its own grid, GPU text, and shell bootstrapping | a PTY + shell-integration **block reader**, and the app's own panes (`term.rs` parses `OSC 133` markers rather than emulating a screen) | The product is an *agent* harness whose terminal must feed the agent, not a general terminal emulator. Blocks are the unit the agent attaches to, so blocks—not cells—are the model. |
| egui-adjacent custom Rust UI toolkit (`warpui`, MIT) | `eframe`/`egui` 0.36 | egui renders offscreen, which is what makes every UI claim in this repo photographable and checkable without a person at the keyboard (`--shot`). |
| The agent is a first-class product surface with its own cloud, Drive, and team features | the agent is the **dsh engine behind ACP**, local, with the composition owning routes, spec, waves, and budget | The harness must *use* the vendor engine and enforce its own token discipline; the UI is the client of that engine. |
| Light and dark themes, per-terminal themes | **light only** | It is a clone of one person's daily driver; a second theme is unearned surface area until the first is finished. |

## 4. Researching a feature here without opening Warp

If you need to know how Warp solves something, do this instead of running the
app:

1. `git clone --depth 1 https://github.com/warpdotdev/warp.git WARP/warp`.
2. Find the crate from §1's table; the crate's `src/` usually has a `*_tests.rs`
   next to the implementation — Warp's tests document behaviour better than its
   comments.
3. Search the repo for the user-visible string (a label, a menu item) to find the
   surface; search for the data structure (a `struct Block`, a `Session`) to find
   the model.
4. Check `crates/warpui/` for how a widget is composed — the shell of *our*
   window (sidebar + centre card + right panel) is the same shape.
5. Read their docs site for the *intent* (what the feature is for), and the code
   only for the *mechanism*.

**Do not** vendor Warp code into `App/`: it is AGPL v3 outside the UI crates and
it is a different UI toolkit. The value here is the interaction design.
