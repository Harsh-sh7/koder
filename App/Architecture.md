# Architecture — AI Harness

A terminal-first coding harness: a native Rust application over a dsh agent
engine, with none of Warp's or dsh's source modified. The engine is the brain
(agent loop, tools, sub-agents, ACP server); the application is the interface
(PTY terminal with blocks, editor, git, observability) and the composition layer
that turns the stock engine into this product.

## Directory layout

```
AI_Har/                      repo root — Makefile MUST live here (hackathon requirement)
├── Makefile
├── .env.example
├── WARP/                    vendored warpdotdev/warp — read-only, never edited
├── Deepseek/                vendored deepseek-ai/deepseek-harness — read-only, never edited
└── App/                     everything we build lives here
    ├── Architecture.md, PRD.md, Prompt.md
    ├── native/              the application: Rust workspace, two crates
    │   ├── crates/harness-core/   headless: PTY, git, files, search, agent model, ACP client
    │   └── crates/harness-app/    the egui interface: panes, palette, dialogs, theme
    ├── scripts/
    │   ├── init-profile.mjs       materializes the `hackathon-harness` dsh profile
    │   ├── run-acp.mjs            `make run`: launches the engine as an ACP server
    │   ├── node-bin.sh            resolves a node dsh accepts (existing, or provisioned)
    │   ├── provision-node.sh      installs the pinned node release into .toolchain/
    │   └── with-node.sh           provision → PATH → ensure pnpm → exec (all node recipes)
    ├── dsh-plugins/
    │   ├── llm-harness-provider/    the AI_API_KEY route source (new)
    │   ├── spec-lock/               prompt-freezing context plugin (new)
    │   └── hackathon-harness-preset/ our composed cordis.yml + harness tools (new)
    └── probe/               handshake probes (mock model server, ACP smoke scripts)
```

Nothing we write lives inside `WARP/` or `Deepseek/`. Both are vendored upstream
checkouts; writing our code inside them would make every future `git pull`
conflict with our work and blur the line between their code and ours during
review. All new code lives in `App/` and reaches into the vendored trees only
through each ecosystem's own extension mechanism: dsh's profile/plugin system,
and (for the native app) ACP — a protocol boundary, not a source boundary.
Consequence: the whole product is exactly `App/` plus a Makefile, and both
upstreams stay cleanly swappable.

## Two upstream repos, different roles

| Repo | Role |
|---|---|
| `Deepseek/` (dsh) | **The engine.** Plugin-based agent runtime. Ships loop guard, sub-agent delegation, session/trajectory logging, terminal/subprocess/shell execution, per-session preset composition, and the ACP server app — all as first-party plugins under `Deepseek/packages/`. |
| `WARP/` | **Nothing is taken from it.** It was evaluated for reuse: its terminal UI is proprietary and not separable from the GUI app, and the user's direction was explicit — own renderer, own visual system, no copied implementation. It stays vendored read-only. The native app is egui over our own widget code. |

## dsh packages reused as-is (no code written into `Deepseek/`)

| dsh package | Maps to |
|---|---|
| `Deepseek/packages/guard/` | loop/turn caps — configured by our preset, never rewritten |
| `Deepseek/packages/subagent/` | the review committee — native delegation |
| `Deepseek/packages/preset/` | the wave-composition mechanism the preset drives |
| `Deepseek/packages/session/` | trajectory/context bookkeeping |
| `Deepseek/packages/terminal/`, `subprocess/`, `shell/` | the agent's executor |
| `Deepseek/packages/interaction/` | human escalation when a cap is exhausted |
| `Deepseek/packages/acp/` | the ACP server `make run` launches and the app speaks to |

## The application — `App/native/`

Two crates, one boundary. `harness-core` is headless and fully testable without a
window; `harness-app` only draws and routes input.

**`harness-core`** (about 6k lines of source): PTY terminals with the block model
(`term.rs` — `portable-pty` + `alacritty_terminal`, so blocks, screen state, and
exit codes are structured facts, not scraped text), git via `git2` (`git.rs`),
files/search via `ignore`+`grep` (`fsops.rs`), workflow/note documents
(`workflow.rs`), the agents-document and price-table model config (`config.rs`),
the session model the UI renders (`agent.rs`), and the **ACP client** (`acp.rs`)
that drives the engine over stdio.

**`harness-app`**: an eframe/egui application, own theme and widgets — no Warp
code. The visual system is light-only, with the palette and the widget set in
`theme.rs`/`icons.rs` (rules, hover tints, cards, tiles, meters) and every pane
written against those tokens rather than raw colours. The shell is the client
shape: top strip, sidebar of workspaces and utilities, a center column that
switches between the conversation and the terminal, and a right panel of live
session facts. Panes: terminal (blocks), agent
(chat/tools/waves/committee/caps), editor + diff, file tree, git
(status/log/diff/commit), observability (token and
cost meters, context bar, trajectory), workflows/notes. Plus the command palette,
modal dialogs, and the `--check` self-test harness.

The agent pane renders the engine's *flow*, not a log. Because child sub-agent
steps are not forwarded over ACP, a delegation (`subagent`, `committee`) draws as
one card: the parent's call, its status, and the result text the parent actually
received — the interface never invents visibility the protocol does not carry.
A file edit (`edit` / `write` / `str_replace_editor`) draws as a card with the
diff derived from the call's own arguments, and the latest `todo_write` draws
once, live, at the end of the transcript (dsh emits no ACP `plan` update).
Message rows carry copy / rating / retry / quote-reply actions.

### The UI↔core boundary

Two channels, deliberately different types, both owned by `harness-core`:

- **Agent events**: `std::sync::mpsc` — `AcpClient::spawn(&EnginePaths, Sender<AgentEvent>)`.
  The app drains with `try_recv` each frame. One producer (the ACP reader
  thread), one consumer (the frame).
- **Terminal/core events**: `crossbeam-channel` — PTY readers, search workers,
  and git workers publish `CoreEvent`s the frame drains the same way.

Facts that shaped the client, learned from the engine's own source rather than
guessed:

- `session/new`'s **reply carries the model choices** (`configOptions`) — they
  arrive with `SessionReady`, not as a separate event. Waiting for a separate
  one is a stall; the app's self-check now takes them from the reply.
- The engine's choices are provider-grouped opaque values; the app selects by
  matching the workspace's active `provider/model` against them
  (`state.rs::apply_requested_model`), never by inventing a value.
- `session/set_config_option` replies with the full option list again; a route
  the running engine is not serving can only appear after a restart, and the app
  says so as a warning instead of failing silently.

## The composition — `App/dsh-plugins/`

**`hackathon-harness-preset/cordis.yml`** is the product's control panel, as one
dsh bundle layer applied after the base and ACP bundles. It does two things:

1. **Route ownership.** Every shipped DeepSeek route is mounted `disabled: true`,
   and `@harness/dsh-llm-harness-provider` is mounted as the only route source,
   reading the workspace document `.harness/models.json` and the environment
   variable `AI_API_KEY`. The composition ships **no model**; a model is added in
   the application, which is the only way a route appears. With nothing declared
   anywhere, the provider still serves one built-in placeholder route
   (`unconfigured`, "No model configured") so `session/new` — which names a route
   — keeps working on a workspace nobody has configured; the placeholder cannot
   complete a request, which is what keeps "ships no model" true.
   (`AI_HARNESS_FALLBACK_MODEL` + `AI_HARNESS_FALLBACK_BASE_URL` — both or
   neither — exist for headless deployments that cannot configure a workspace.)
2. **The harness itself**: `@harness/dsh-spec-lock` (the incoming issue is frozen
   at session start, restated whenever context management drops it, amendable only
   with user approval), plus the preset's own contract, wave/committee tools, and
   budget cap (defaults in `cordis.yml`, overridable from a profile patch or
   overlay without editing source).

**Token minimization is designed in, not tuned later:**

- `spec-lock` keeps the issue's exact bytes as the session's anchor, so the
  contract never depends on the model re-deriving it from a long transcript.
- The preset's prompt prefix (`policy.ts`) is **byte-identical across turns** so
  the provider's cache actually hits; live numbers (budget spent, wave in flight)
  appear only in the messages that need them.
- `budget.ts` counts **model tokens**, not money — the cap is on the scarce
  resource. Money is priced separately, in the app, from the price table in
  `.harness/models.json`.
- The committee seats reviewers as **fresh children, not forks**: a fork hands
  every reviewer the parent's whole context and pays for it three times — exactly
  the cost the committee exists to avoid.

## Control flow

```
issue arrives (an ACP session — `make run` has already launched and is listening)
   -> spec-lock freezes it
   -> the preset composes waves from the file-conflict graph
      -> the engine's terminal/subprocess/shell executes each subtask
         -> tests present? run them:  pass -> next wave / done
         |                            fail -> re-enter wave composition (that subtask)
         -> tests absent/broken -> the committee (correctness / adversarial / style)
            votes — quorum, not unanimity
                                   pass -> next wave / done
                                   fail -> re-enter wave composition (that subtask)
   -> guard wraps the whole loop: hard cap -> escalation via interaction, or stop
```

The native app sits outside this loop as an ACP client: it opens a session on
the workspace, sends the prompt, renders the events (messages, tool cards, wave
plans, committee reports, usage), and can interrupt. It does not implement its
own agent loop — there is exactly one, in the engine.

## Launch paths

- `make setup` — installs the dsh substrate, installs `App/`'s plugin workspace,
  then `init-profile.mjs` materializes `$DSH_HOME/profiles/hackathon-harness`
  with `link:` dependencies resolved to this checkout. Idempotent. It needs
  neither node nor pnpm to be installed beforehand: every node-using recipe goes
  through `with-node.sh`, which resolves an interpreter dsh accepts — the one on
  `PATH` when it is suitable, otherwise the pinned release downloaded into
  `.toolchain/node` and checksum-verified against the release's own
  `SHASUMS256.txt` — then provides a pnpm through that node's corepack (npm
  fallback) when the machine has none. No container and no system-wide install;
  `.toolchain/` is gitignored scratch. (`HARNESS_NODE` points at an existing
  interpreter; `HARNESS_NODE_VERSION` forces the download path.)
- `make run` — `run-acp.mjs` starts the engine as an ACP server on stdio,
  workspace at the repo root, with the session's starting route patched in from,
  in order: the document's `active` entry, the deployment fallback
  (`AI_HARNESS_FALLBACK_MODEL`/`_BASE_URL`), or the provider's built-in
  placeholder route. The acp row's shipped default names a provider this
  composition disables, so a launch that patched nothing could not open a
  session at all. It comes up and stays up; the issue arrives as a
  `session/prompt` on the running process. A no-model launch says so on stderr
  and starts anyway — sessions open on the placeholder, which cannot complete a
  request.
- The workspace the agent works in travels separately from the launch directory:
  the client names it in `session/new` `cwd`, and `run-acp.mjs` reads
  `HARNESS_WORKSPACE` (which the app sets) to know whose route document to read.
  The generated overlay also points the provider plugin at that document by
  absolute path, because the plugin resolves a relative `modelsFile` against the
  engine's working directory — always this repository. Without that row, a
  session opened on another folder fails to open at all ("no adapter registered
  for provider …"), since the route it was told to start on lives in the folder's
  document, not the repository's.
- `make app` / `make app-release` — the native application. It starts its own
  engine when the agent panel needs one, so `make run` is not a prerequisite.
  Switching folders from the app (`⌘O`, the workspace row, the footer's folder
  name, the title menu, or the palette) restarts the engine with the new
  `HARNESS_WORKSPACE`, so the tree, editor, shells, and tool calls all point at
  it.

## Credential boundary

- `AI_API_KEY` is read in exactly one place: the mounted
  `@harness/dsh-llm-harness-provider` (`apiKeyEnv: AI_API_KEY`).
- The shipped DeepSeek providers (which read `DEEPSEEK_API_KEY`) are mounted off
  in `cordis.yml`; `WARP/app/ai/`'s cloud credential is never consulted because no
  Warp code runs.
- `.env` is gitignored local scratch; `.env.example` declares `AI_API_KEY=` and
  the two optional fallback variables, nothing else.
- The **value** is typed in exactly one place: the model dialog's `api key`
  field, which writes it to the workspace `.env` under the route's key variable
  (and sets it for the app's own environment, which the engine child inherits).
  A route document records the variable's *name*, never its value;
  `write_dotenv_value` refuses a name outside `AI_*` rather than write a pair
  nothing would read back.

## Verified, and how

- `make check` — plugin typecheck + plugin tests + `cargo test --workspace`
  (63 core tests including the terminal smoke test), green.
- **The toolchain path**: `make setup` was run end to end three ways — with the
  machine's own node on `PATH` (nothing downloaded, ~0.1 s), with node and pnpm
  stripped from `PATH` (the pinned release downloaded, checksum-verified,
  unpacked, and pnpm enabled through its corepack), and with
  `HARNESS_NODE_VERSION` forcing that download. `make run`'s launcher was then
  proven to answer ACP on the provisioned interpreter
  (`SMOKE_ENGINE=… node App/probe/acp-smoke.mjs`).
- `make check-native` / `make check-engine` — the app's `--check` self-test:
  15 passed without the engine, 17 with it and no model configured, including
  PTY block production, the credential line (it reports which variable the
  active route names and whether that variable holds a value — on a fresh clone
  with the probe placeholder it warns "mock-key", which is the first failure a
  new user meets), and, for the engine, a live handshake → session → model
  choices (the session opens on the placeholder route and the choices read
  "No model configured").
- **The key has a place to go**: `write_dotenv_value` is unit-tested both ways
  (a write replaces the named variable in place and leaves comments and other
  names alone; a new name is appended; a name outside `AI_*`, or an empty or
  multi-line value, is refused and writes nothing). The dialog's field is
  captured through `--dialog models --shot`, which shows the masked input, the
  hint naming `.env`, and the standing line — with the probe placeholder in
  `.env` it reads "AI_API_KEY is still the probe placeholder (mock-key); paste a
  real one".
- **The credential reaches the request**: a probe workspace was pointed at a
  local recording endpoint, and a real engine session (`acp-smoke.mjs` with
  `WORKSPACE`/`AI_API_KEY` in the launch environment) sent
  `authorization: Bearer sentinel-key-abc123` — the value from the environment,
  which is the value the dialog's key field writes to `.env`. A second run with
  the route's `apiKeyEnv` renamed to `AI_NVIDIA_KEY` sent
  `Bearer sentinel-named-456`, so the route's own variable name is what the
  engine reads, not a hardcoded one.
- **Session creation is never a dead end**: `node App/probe/acp-smoke.mjs`
  (direct spawn) and `SMOKE_ENGINE='make run' node App/probe/acp-smoke.mjs`
  (the documented launcher) both open a session on an unconfigured workspace;
  with the mock route written to `.harness/models.json` the same probe opens on
  `default/deepseek-chat`, unchanged.
- `cargo test -p harness-core --test acp_smoke -- --ignored --nocapture` — the
  full Rust-client path against the real engine: handshake
  (`deepseek-harness-acp 0.0.1`), session with the mock route, the harness's own
  `wave_plan` tool returning a 2-wave plan, usage accounting, clean turn end.
  Also proven from Node by `App/probe/acp-smoke.mjs`.
- **The window itself**: the app can photograph its own frame
  (`--shot FILE.ppm`, `--shot-delay SECONDS`), which is what the interface work
  was checked against — the composer row at both 1480×940 and a 920×700 window,
  the transcript's cards (a delegation card, a created file's card with its
  `+N` lines, an edit card's `+1 -1` diff, a running command row, and the live
  `to-dos · N of M done` card), the message action row (copy / rating / refresh
  retry / reply), the Session card, and both right-panel lists with live rows
  (`node App/probe/mock-openai.mjs --panel`, route written to `.harness/models.json`;
  the mode drives a task list twice, a `subagent` delegation, a created file, an
  `edit` of it, and a command held open — every card the transcript and the
  session panel can draw, with live rows). A capture raises the window while the
  shot is pending and takes no keyboard focus; there are no other capture-only
  behaviours in the interface. `--pane VIEW` opens straight into one view,
  `--dialog models` straight into the model dialog, and `--ask PROMPT` sends a
  prompt at launch — all three exist for the same reason.
- **Not verified here**: pixels are only as good as the capture path, so
  hover-only states, drags, and the platform window decorations are unverified
  by machine; everything load-bearing (state, layout arithmetic, tool routing)
  is covered by the tests above.

## Non-determinism, documented

Model output is non-deterministic by nature; the composition bounds it (caps,
waves, quorum) but does not pretend to fix it. The mock model server
(`App/probe/mock-openai.mjs`) is deterministic, which is what makes the
handshake tests above reproducible. Real runs against a real provider will vary
in wording and step counts; the budget cap is what makes that variation safe.
