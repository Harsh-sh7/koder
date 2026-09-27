# Takeover — the operating manual

You are taking over a working harness, not starting one. Everything in
`context.md` holds; this file is the rules you must not break, the loop you work
in, and the queue of what to build next.

Read `context.md` first if you have not.

---

## 1. First: restore the vendored checkouts

The two upstream checkouts are **gitignored and not tracked**, and they were
removed to keep the handover small. `Deepseek/deepseek-harness` is a hard
dependency — our plugins `link:` into its packages and the engine runs from its
source. `WARP/` is a read-only design reference; nothing builds or reads it, so
restore it only when you want to research it.

```sh
git clone --depth 1 --branch dsh-v0.1.7-rc.2 \
  https://github.com/deepseek-ai/deepseek-harness.git Deepseek/deepseek-harness

# optional, read-only reference for UI work
git clone --depth 1 https://github.com/warpdotdev/warp.git WARP/warp
```

The pinned ref is what this project was built and verified against
(`dsh-v0.1.7-rc.2`, commit `477b4f4205`, root package `@deepseek-ai/dsh-root`
0.1.7-rc.2). Upgrading dsh is allowed but is a **deliberate task** with its own
verification, not something to do in passing — see `deepseek_reference.md` §7.

If the checkout is missing, `make setup` stops with that exact command in its
message rather than failing halfway through an install.

## 2. Get it running

```sh
cp .env.example .env      # put a key in AI_API_KEY, or leave it and use the mock
make setup                # node/pnpm (provisioned if missing) → substrate install
                          # → our plugin workspace → the dsh profile
make run                  # engine comes up as an ACP server on stdio, stays up
make app                  # the native window (debug)
make app-release          # the native window (release — this is what a demo uses)
```

- `make run` **launches and listens**; the issue is supplied afterwards as an ACP
  `session/prompt` to the running process — never as a launch argument.
- `make app` starts its own engine when it needs one; `make run` is not a
  prerequisite for the app.
- `make setup` is idempotent. Re-run it after moving the checkout: the profile's
  `link:` dependencies are rewritten to the new absolute paths.
- The default model is OpenRouter's DeepSeek; `AI_MODEL`/`AI_BASE_URL` in
  `.env` choose another, and a route added in the app's model dialog (`⌘,`,
  written to `.harness/models.json`) wins over both. Without `AI_API_KEY`,
  prompts fail immediately with a clear message.

## 3. The rules (compliance — breaking one of these fails the deliverable)

1. **`Makefile` stays at the repository root** and stays the standard interface:
   `setup` `run` `app` `app-release` `test` `typecheck` `check` `check-native`
   `check-engine` `clean`. New work goes through these targets or is invisible.
2. **`make setup` then `make run` works from a clean clone** (after §1's vendored
   restore, which the setup guard prints). `make run` must come up and **stay
   up** — it is an ACP server, not a one-shot CLI.
3. **`AI_API_KEY` is the only credential.** Never `DEEPSEEK_API_KEY`. It is read
   in exactly one place — `App/dsh-plugins/llm-harness-provider` (`apiKeyEnv:
   AI_API_KEY`) — and dsh's DeepSeek-branded providers are mounted **off** in
   `cordis.yml`. `.env.example` declares `AI_API_KEY` and nothing else; `.env` is
   gitignored scratch and **must never be committed**.
4. **No hardcoded credentials anywhere.** The model dialog writes the key to the
   workspace `.env`; a route document records only the variable's *name*.
5. **Nothing is written into `WARP/` or `Deepseek/`.** All of our code lives in
   `App/`. If a change seems to need an upstream edit, it needs a plugin instead
   (`deepseek_reference.md` §4 explains the seam).
6. **Model configuration lives in one file**:
   `App/dsh-plugins/hackathon-harness-preset/cordis.yml`. Every value in it must
   stay overridable from a profile patch or a session overlay **without editing
   source**. Adding a model must never require a code change.
7. **Text-only model I/O.** No image, audio, or file-attachment paths to the
   model.
8. **Non-determinism is documented, not hidden.** The README's section is part of
   the deliverable: model output varies run to run, the harness bounds it
   (budget, step cap, committee quorum), and everything downstream of a model
   call is deterministic.
9. **Never commit or push without an explicit request.** The repository has no
   commits yet; that is the owner's call to make.
10. **The UI must not lie.** No fake progress, no invented numbers. Every panel
    shows something the harness actually knows (this is why the failure row
    exists and why the provider's raw JSON is translated rather than echoed).

## 4. Verify your work — the loop

Nothing here is proven by reading it. The loop is: **run it, photograph it,
count it.**

```sh
make check          # plugin typecheck + plugin tests + cargo test --workspace
make check-native   # the app's headless self-test: workspace docs, shell
                    # integration, a real PTY, the file tree, a search, the repo
make check-engine   # boots the engine over ACP and opens a session (no key needed)
```

**The deterministic model** — `App/probe/mock-openai.mjs` — is how a *turn* is
proven without a network or a credential:

```sh
node App/probe/mock-openai.mjs --panel &        # or --tool --spec --wave --fail
```

Point a scratch workspace at it — `.harness/models.json` with
`baseURL: http://127.0.0.1:8899/v1`, `"api": "openai-completions"`,
`apiKeyEnv: "AI_API_KEY"`, and a `.env` with `AI_API_KEY=mock-key`. The `api`
field is **required** for a route outside the installed catalog, or
`session/new` fails with "needs an api".

**The window can be photographed without a person at the keyboard** (and without
screen-recording permission — the app renders offscreen and writes its own frame):

```sh
./App/native/target/debug/ai-harness --workspace <dir> \
    [--pane terminal|editor|git|workflows|meter] \
    [--dialog models|folder] [--ask "prompt"] \
    --shot /tmp/frame.ppm --shot-delay 8
sips -s format png /tmp/frame.ppm --out /tmp/frame.png
```

Read the PNG back and look at it. This is how every UI claim in this repo was
checked, including the busy-turn frames (arc, activity card, running processes,
the failure row).

**Measure, do not guess.** Two examples that cost real time here:
`top -l 4 -s 2 -pid <pid> -stats pid,cpu,time` settles "the app is spinning"
(false alarms read 25–40% for the first seconds after launch, then fall to <1%
release / ~2% debug); and a frozen-looking `make app-release` terminal is just
cargo's last progress line — the build finished and the window is up (the
Makefile now prints `starting the window` to say so).

## 5. Conventions

- **Doc comments on every item**, in a specific shape: a one-line summary, then
  `/// @param name what it is` / `/// @returns what comes back`. The codebase is
  written to be read; match it.
- **Comments explain *why*, never *what*.** A comment is warranted for a hidden
  constraint, a subtle invariant, or a workaround — not to restate the code. If
  removing it would not confuse a reader, do not write it.
- **Rust**: `harness-core` is pure and testable (no I/O in the model layer);
  `harness-app` is drawing and event wiring. Tests live in the module (`#[cfg(test)]
  mod tests`) and use scratch directories, never the network.
- **TypeScript plugins**: `tsc -p tsconfig.json` is the build (they ship as TS
  and are loaded as TS). Tests are `node --import tsx --test tests/*.test.ts`
  and compose the plugin over real dsh services with fakes injected through
  `ctx.provide(name, fake as never)`.
- **Every button works.** The UI has been audited end to end; a control that
  cannot do anything either does something visible (a toast saying why not) or
  does not exist. Do not add decorative controls.
- **The failure path is part of the feature.** A turn that fails ends the run,
  says what the provider said in one sentence, and offers the repair (retry /
  another route). Never leave a spinner implying work that is not happening.

## 6. The work queue

Ordered by the owner's priorities (§1 of `context.md`). Each item says what
"done" looks like.

### 6.1 Sub-agent orchestration, proven end to end *(owner's explicit ask)*

Today: the engine links `@deepseek-ai/dsh-subagent`; the preset's `committee`
seats reviewers through `ctx.subagents` as fresh children (`committeeProvider:
spawn`); `wave_plan` partitions a job into file-disjoint waves; the app draws a
delegation card (`agent_card`) and lists subagents in the session panel; the
mock's `--panel` mode exercises the path visually.

Missing: a **real** delegated sub-task driven to completion and verified —
parent delegates → child runs its own tool calls → result returns to the parent →
the app shows the child's work and its cost. Prove it with the mock (a canned
delegation) **and** with a real route, and add the proof to `make check-engine`
or a new probe. Then, and only then, build on it:

- Parallel wave execution: the planner already guarantees intra-wave file
  disjointness, so a wave may be delegated together. Make the app show wave *k*
  of *n*, with its members' states, and make the budget accounting attribute
  child spend to the session.
- A subagent's own context policy: children start fresh, so hand them the frozen
  spec digest (`spec_get`) rather than the parent's transcript.
- Failure semantics: a child that fails must not corrupt the parent's transcript
  or leave the wave half-applied.

### 6.2 Token efficiency

- **Context pollution is the enemy.** Audit what enters the model's context each
  turn: transcript growth, tool results, logs. Prefer digests, tails, and
  pointers over full text. The spec lock already does this (one pointer line
  instead of the whole spec) — extend the pattern.
- **Cache-aware ordering.** Keep the stable prefix stable (contract, spec
  pointer, tool schemas) so a provider's prompt cache can hit; report the
  cached-token share the engine already returns in the meter.
- **Per-turn cost attribution.** The meter shows occupancy; make a turn's own
  spend visible (a per-turn delta in the transcript's footer) so a human can see
  which step was expensive.
- **Compaction policy.** Decide and document when the harness compacts, what it
  keeps verbatim, and prove the spec survives compaction (the spec lock's
  restatement exists for exactly this).
- **Fewer, better tool calls.** Every tool call is a round trip of context.
  Where the harness can compute something the model would otherwise re-derive
  (wave partitioning is the model), add a tool that returns it once.

### 6.3 Time efficiency

- Concurrency where the plan already allows it (see 6.1).
- The engine's cold start is ~5 s to `initialize` and ~15–20 s to a first
  session on this machine (tsx compiling the plugin graph). Profile it, then
  make it visible in the UI (the app says "waiting for the engine session"
  today) and, if possible, warm it (the app already keeps the process across
  sessions — `new_task` reuses it; the folder switch deliberately does not).
- The UI must stay responsive while a turn runs: the repaint ladder is 16 ms
  while busy, 200 ms while a toast is up, 500 ms idle. Never pin the frame rate
  for work that is not happening (a stale `Running` state once did exactly that;
  see §5's failure-path rule).

### 6.4 Context efficiency (the model's context, the human's context)

- The human's context: the transcript is the product's memory. Cards collapse
  what is settled (a change card shows a diff summary, the newest to-do card
  replaces the list), and the panel shows what is *live*. Keep that discipline
  when adding anything.
- The model's context: one fact, one place. Anything already in the frozen spec
  or the operating contract should be referenced, not restated.
- The export path (`⌘⇧E`, `export_transcript`) is how a session leaves the app —
  keep it complete (messages, ratings, tool cards, tokens) so a hand-off to a
  fresh session does not need the whole history replayed.

### 6.5 Responsive layout *(the owner started this; it is unowned now)*

Every panel is fixed-width today: sidebar 236, session panel 312, the tree
246 default. The window's minimum is 900×560, so with the tree open the centre
card is crushed. Make widths scale with the window, collapse the sidebar to an
icon rail when narrow, draw the session panel over the card when there is no
room beside it, and cap the transcript at a readable column. The capture path in
§4 is how you prove it at several window sizes.

### 6.6 Smaller, still open

- The **Mobbin MCP** server is registered in the Qoder client for UI-library
  research (owner's account, one-time OAuth). Use it only for design reference.
- The engine's own error text is sometimes bodyless (`429 status code (no
  body)`); `describe_failure` handles it, but richer provider detail would let
  the transcript say more.
- `App/probe/mock-openai.mjs --fail` answers every request with OpenRouter's 429
  shape — use it for any new failure-path work.

## 7. Where to look for what

| Concern | File |
|---|---|
| Window, panels, keys, repaint cadence, screenshot | `App/native/crates/harness-app/src/app.rs` |
| All app state, engine lifecycle, workspace switch, failure translation | `.../harness-app/src/state.rs` |
| Conversation view: transcript, cards, composer, live rows | `.../harness-app/src/panes/conversation.rs` |
| Sidebar, session panel, activity card, footer, fab, toasts | `.../harness-app/src/shell.rs` |
| Theme tokens, spinner, card/badge/action primitives | `.../harness-app/src/theme.rs` |
| Model/route dialog, folder picker, about | `.../harness-app/src/dialogs.rs` |
| Command palette, go-to-file | `.../harness-app/src/palette.rs` |
| ACP client (transport, framing, sessions) | `.../harness-core/src/acp.rs` |
| Session model: transcript, run state, meter (pure) | `.../harness-core/src/agent.rs` |
| PTY, shell integration, blocks | `.../harness-core/src/term.rs` |
| Workspace documents (`models.json`, `prices.json`, config) | `.../harness-core/src/config.rs` |
| Git, files, search | `.../harness-core/src/{git,fsops}.rs` |
| The route provider (`AI_API_KEY`, the models document) | `App/dsh-plugins/llm-harness-provider/src` |
| Frozen spec | `App/dsh-plugins/spec-lock/src` |
| Contract, waves, committee, budget | `App/dsh-plugins/hackathon-harness-preset/src` |
| What the agent is told to do | `App/dsh-plugins/hackathon-harness-preset/skills/orchestrate.md` |
| The engine launcher (workspace, route, overlay) | `App/scripts/run-acp.mjs` |
| The profile (bundles, link deps) | `App/scripts/init-profile.mjs` |

## 8. Traps that cost real debugging time here

1. **The engine has no `--acp` flag.** The ACP bridge *is* the profile's
   application: `hackathon-harness` composes `@deepseek-ai/dsh-acp-app`, whose
   app reads stdin until EOF. **Stdout carries ACP and nothing else** — every
   diagnostic goes to stderr.
2. **The route the first session opens on is data.** The acp row ships a default
   provider this composition disables, so a launch that patched nothing could
   not open a session at all. `run-acp.mjs` patches it from, in order: the
   workspace document's `active` entry, the repo document, the deployment
   fallback, the provider's placeholder.
3. **The provider resolves `modelsFile` against the engine's cwd** (always the
   repo), so a relative path silently reads the wrong document after a folder
   switch. The generated overlay therefore carries an **absolute** path.
4. **A route outside the installed catalog needs `"api": "openai-completions"`**,
   or `session/new` fails with "needs an api; the installed catalog does not
   describe it".
5. **A failed prompt must end the run.** Leaving `RunState::Running` after
   `RequestFailed` keeps the badge at "working", the arc turning, and the app at
   60 fps forever. Fixed in `harness-core/src/agent.rs`; two tests guard it.
6. **`make app` and `make app-release` are two commands plus an `exec`** on
   purpose — cargo's progress bar stays on screen when the build ends, so one
   `cargo run` line leaves the terminal looking frozen while the window is open.
7. **Node**: dsh needs `^22.19.0 || >=24`. This machine's system node is older,
   which is why `make setup` provisions a pinned release into `.toolchain/` and
   every node-using recipe goes through `App/scripts/with-node.sh`.

## 9. How to report work

Say what you ran and what it showed. "The test passes" is weaker than "`make
check` is green, and here is the frame the app drew". If something is unproven,
say which part is unproven — the owner reads these reports to decide what to
demo.
