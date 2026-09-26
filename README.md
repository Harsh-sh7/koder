# AI Harness

A terminal-first coding harness: a native desktop application over the
`deepseek-harness` (dsh) agent engine. The issue is frozen at session start,
decomposed into parallel waves, executed through real shell commands, and every
iteration has to end in a real pass/fail signal — the project's tests, or a
reviewing committee when there are none. Token budgets are enforced by the
harness, not requested from the model. Nothing is written into the vendored
`WARP/` or `Deepseek/` checkouts; all of our code lives in `App/`.

## Requirements

- macOS or Linux
- Node **^22.19 or >=24** and `pnpm` — but neither needs to be installed
  beforehand: `make setup` resolves a node the engine accepts, installing the
  pinned release into `.toolchain/` (checksum verified against the release's
  own `SHASUMS256.txt`) when the machine has none, and provides a pnpm through
  that node's corepack when there is no pnpm on `PATH`. See
  `App/scripts/provision-node.sh` and `App/scripts/with-node.sh`.
- A stable Rust toolchain (edition 2021) — the application in `App/native`
- The vendored checkouts at the repository root: `Deepseek/deepseek-harness`
  (required; the engine), `WARP/` (read-only reference only — nothing builds or
  reads it). They are not tracked by this repository, so a fresh clone restores
  them first — `make setup` refuses to run without the engine and prints the
  command; see `takeover.md` §1 for both, including the optional `WARP/` clone:

  ```sh
  git clone --depth 1 --branch dsh-v0.1.7-rc.2 \
    https://github.com/deepseek-ai/deepseek-harness.git Deepseek/deepseek-harness
  ```

## Run

```sh
cp .env.example .env      # then put your key in AI_API_KEY
make setup                # toolchains check, substrate install, plugin install, profile
make run                  # the engine comes up as an ACP server and stays up
```

`make run` launches and listens; the issue is supplied afterwards to the running
process (an ACP `session/prompt`), not as a launch argument.

```sh
make app                  # the native application (debug)
make app-release          # the native application (release — use this for a demo)
```

The app starts its own engine when the agent panel needs one, so `make run` is
not a prerequisite for it.

### The window

The conversation is the centre card, with the session panel on the right. A turn
in flight is drawn while it runs: a turning arc, what the engine is doing, and
how many seconds it has been doing it — at the end of the transcript and again at
the top of the panel — and the panel's **Background processes** list always shows
every shell and every command the engine ran, busy or idle, so a long command is
never a blank screen. **Open folder…** (the workspace row in the sidebar, the
folder in the footer, the title's `⋯` menu, the palette, or `⌘O`) switches the
folder the whole harness works in: the tree, the editor, the shells, and the
agent's tool calls follow it, the engine restarts on it, and the model route
carries over when the new folder has none.

A turn that fails ends there rather than spinning: the transcript says what the
provider said in one sentence, and a row under it offers **try again** on the same
route or a jump to the route dialog. A rate-limited free route (429), a rejected
key (401), and a wrong base URL (404) each name their own repair.

## Adding a model

The composition ships **no model**. A route appears when one is added — in the
application's model dialog, or by writing the workspace document
`.harness/models.json` — and the route a session starts on is that document's
`active` entry. The dialog's `api key` field takes the credential's value and
writes it to the workspace `.env` (gitignored); the route document records only
the variable's name, so a key never lands in a file that gets committed. Until a
route is added, sessions still open: they run on the provider's built-in
placeholder route ("No model configured"), whose requests fail at once, so an
ACP client attached to `make run` on a fresh clone gets a session and a clear
"add a model" instead of an engine error. Model configuration lives in exactly
one file, `App/dsh-plugins/hackathon-harness-preset/cordis.yml`, and every value
in it is overridable from a profile patch or overlay without editing source.

## Where things live

| Path | What |
|---|---|
| `Makefile` | the standardised interface: `setup` `run` `app` `test` `typecheck` `check` `check-native` `check-engine` `clean` |
| `App/native/` | the application — `harness-core` (PTY terminal with blocks, git, files, search, ACP client, session model, token accounting) and `harness-app` (the egui interface) |
| `App/dsh-plugins/` | the composition — the `AI_API_KEY` route provider, the spec lock, and the harness preset (contract, waves, committee, budget cap) |
| `App/scripts/` | `init-profile.mjs` (materializes the dsh profile), `run-acp.mjs` (`make run`), and the node/pnpm provisioning (`node-bin.sh`, `provision-node.sh`, `with-node.sh`) |
| `App/probe/` | handshake probes, including the deterministic mock model server |

`WARP/` and `Deepseek/` are upstream checkouts: read, never edited.

## Verify

Everything below runs without a model credential (`AI_API_KEY=mock-key` is
enough for the engine checks, which open sessions without prompting).

```sh
make check          # plugin typecheck + plugin tests + cargo test --workspace
make check-native   # the app's headless self-test: workspace docs, shell integration,
                    # a real PTY, the file tree, a search, the repository
make check-engine   # boots the engine over ACP and opens a session — with no
                    # model added yet it opens on the placeholder route
```

Two end-to-end tests are opt-in because they need a prepared checkout
(`make setup`) and a model route; each is run with `--ignored`:

```sh
cd App/native && cargo test -p harness-core --test acp_smoke -- --ignored --nocapture
cd App/native && cargo test -p harness-core --test terminal_smoke -- --ignored --nocapture
```

The first is the Rust client against the real engine: handshake, session, the
harness's own `wave_plan` tool, usage accounting, clean turn end. The second
drives the shell integration interactively and proves a command's block carries
its exit code. `node App/probe/acp-smoke.mjs` proves the same handshake from
Node.

The application can also be driven and photographed without a person at the
keyboard, which is how the interface was checked:

```sh
node App/probe/mock-openai.mjs --panel &     # deterministic route, panel-mode replies
./App/native/target/debug/ai-harness --shot /tmp/frame.ppm --shot-delay 8
./App/native/target/debug/ai-harness --pane terminal --ask "run the tests"
./App/native/target/debug/ai-harness --dialog models --shot /tmp/routes.ppm
```

`--shot` writes one PPM of the app's own frame after the delay (it raises the
window while the shot is pending and never takes focus); `--pane` opens straight
into a view; `--dialog models` opens straight into the model dialog; `--ask`
sends a prompt at launch; `--check` is the headless self-test. `mock-openai.mjs`
modes: default text replies, `--tool` (tool calls), `--spec` (a spec round),
`--wave` (a plan plus subagents), `--panel` (a task list twice, a `subagent`
delegation, a created file, an `edit` of it, and a command held open — every card
the transcript and the session panel can draw, with live rows).

## Non-determinism

Model output is non-deterministic: two runs on the same issue will differ in
wording, step counts, and sometimes in how the work is split into waves. The
harness bounds that rather than pretending to remove it — the session's token
budget, the per-turn step cap, and the committee's quorum are all hard limits
enforced by the composition, so a bad run costs a bounded amount and then stops
or escalates. Anything downstream of a model call (test results, exit codes,
git state) is deterministic and is what the pass/fail decisions actually rest
on. The tests above are reproducible because they run against
`App/probe/mock-openai.mjs`, a deterministic model server.

## Credentials

`AI_API_KEY` is the only credential this harness reads, and it is read in
exactly one place: the provider plugin the composition mounts
(`apiKeyEnv: AI_API_KEY`). dsh's own DeepSeek-branded providers — which expect
`DEEPSEEK_API_KEY` — are mounted off. `.env` is gitignored local scratch;
`.env.example` declares `AI_API_KEY`, nothing else. The model dialog's `api key`
field is the one place a value is typed: it is written to the workspace `.env`
(and only `AI_*` names are accepted, because anything else would never be read
back). No key is ever hardcoded, and none is ever written to a route document.

## Docs

- [`App/Architecture.md`](App/Architecture.md) — layout, the UI↔core boundary,
  the composition, control flow, and what was verified how.
- [`App/PRD.md`](App/PRD.md) — goals, requirements, and risk resolutions.
- `App/Prompt.md` — the original design prompt.
