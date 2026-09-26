# Context — what this repository is

A terminal-first coding harness: **a native desktop application** (Rust + egui)
over the **`deepseek-harness` (dsh) agent engine**, speaking ACP over stdio.

The agent is not "a chat with tools". The issue is frozen at session start,
decomposed into file-disjoint waves, executed through real shell commands, and
every iteration has to end in a real pass/fail signal — the project's own tests,
or a reviewing committee when no check can settle the question. The token budget
is **enforced by the harness**, not requested from the model.

Read this file for the picture and `takeover.md` for the rules and the queue.
`App/Architecture.md` is the long-form architecture; `App/PRD.md` is the
original product spec; `App/Prompt.md` is the design prompt that started it.

---

## 1. The three priorities, in this order

Every design decision in this repo is ranked against these, and the order is
binding when two of them disagree:

1. **Token efficiency** — the smallest number of model tokens per landed change.
   The harness spends tokens on *proof*, never on rediscovery: the spec is frozen
   so it is not re-derived, `wave_plan` does the partitioning arithmetic so the
   model does not, and reviewers are seated as fresh children so the parent's
   context is never paid for three times over.
2. **Time efficiency** — wall-clock per landed change, mostly by running the
   independent parts of a job at the same time (waves are file-disjoint on
   purpose, so a wave *can* be delegated concurrently).
3. **Structure** — a codebase that is obvious to extend: pure core, thin UI,
   composition over configuration.

Anything that trades 1 for 3 is wrong. Anything that trades 2 for 3 is usually
wrong.

## 2. The rules that cannot break

These are the submission's compliance conditions and the product's safety
properties. They are expanded in `takeover.md` §3 — read that before touching
anything. The short form:

- **`Makefile` at the repository root is the interface**: `make setup` then
  `make run` from a clean clone. `make run` launches and **stays running**; the
  issue arrives afterwards as an ACP `session/prompt` on the live process.
- **`AI_API_KEY` is the only credential.** Never `DEEPSEEK_API_KEY`. No key is
  ever hardcoded, and no key is ever written into a tracked file.
- **Nothing is written into `WARP/` or `Deepseek/`.** They are vendored upstream
  checkouts, read-only, and not tracked by this repository.
- Model configuration lives in **exactly one file**
  (`App/dsh-plugins/hackathon-harness-preset/cordis.yml`) and is overridable
  from a profile patch or overlay **without editing source**.
- Model I/O is **text only** (no image/audio paths).
- Do not commit or push without the owner's explicit request.

## 3. The shape of the system

```
┌─ App/native (Rust, ours) ─────────────────────────────────────────────┐
│  harness-app    egui window: sidebar, cards, transcript, session panel │
│  harness-core   PTY terminal w/ blocks · git · files · search ·        │
│                 ACP client · session model · token accounting          │
└───────────────┬───────────────────────────────────────────────────────┘
                │ ACP on stdin/stdout (JSON-RPC), one process per engine
┌───────────────▼───────────────────────────────────────────────────────┐
│  dsh engine (Deepseek/deepseek-harness, upstream, unmodified)          │
│  profile "hackathon-harness" = dsh-base + dsh-acp-app                  │
│                              + hackathon-harness-preset (ours)         │
└───────────────┬───────────────────────────────────────────────────────┘
                │ mounts three plugins (App/dsh-plugins, ours)
┌───────────────▼───────────────────────────────────────────────────────┐
│  llm-harness-provider   every model route, from .harness/models.json,  │
│                         key read from AI_API_KEY only                  │
│  spec-lock              the frozen session spec, restated not rewritten│
│  harness-preset         operating contract · wave_plan · committee ·   │
│                         budget cap · per-turn step cap                 │
└───────────────────────────────────────────────────────────────────────┘
```

The UI is a **function of state**: `harness-core` reduces ACP events into a
conversation model (pure, no I/O, testable from fixtures); `harness-app` draws
it. The engine owns the loop, the tools, and the session; the app owns the
window, the terminal, and the workspace.

## 4. Layout

| Path | What it is |
|---|---|
| `Makefile` | the standardised interface (root, as the submission requires) |
| `App/native/` | the application — `harness-core` (engine + services) and `harness-app` (egui) |
| `App/dsh-plugins/` | our three dsh plugins (the agent's brain and its safety rails) |
| `App/scripts/` | `init-profile.mjs` (materializes the dsh profile), `run-acp.mjs` (`make run`), node/pnpm provisioning |
| `App/probe/` | handshake probes and the deterministic mock model server |
| `App/*.md` | `Architecture.md`, `PRD.md`, `Prompt.md` |
| `Deepseek/` | vendored `deepseek-ai/deepseek-harness` — the engine. **Restored, not tracked** (`takeover.md` §1) |
| `WARP/` | vendored `warpdotdev/warp` — read-only design reference. Nothing builds or reads it |
| `.harness/` | per-workspace state: `models.json`, `prices.json`, `workflows/`, `notes/`, `transcripts/` |
| `.dsh/` | the dsh home the profile is materialized into |

`WARP/` and `Deepseek/` are **deleted from a fresh clone** (they are large and
gitignored). Their reference docs are `warp_reference.md` and
`deepseek_reference.md`.

## 5. Decisions that are locked

Each has a reason; do not relitigate without reading it.

| Decision | Why |
|---|---|
| **Native Rust application, our own renderer** (eframe/egui 0.36) | The deliverable is a product, not a web page. A real PTY and real OS integration were required, and the engine is a local process on stdio. |
| **egui, not a custom wgpu renderer** | Immediate mode maps onto "UI is a function of state" with no retained-tree bookkeeping, and it renders offscreen, which is how every UI claim in this repo is photographed and verified. |
| **The engine is dsh behind ACP, unmodified** | The harness must *use* the upstream agent engine, not reimplement it. All of our behaviour is mounted as plugins, which keeps the upstream checkout pristine and upgradable. |
| **No model ships with the composition** | A route is data (`AI_API_KEY` + `.harness/models.json`), and the composition mounts off every shipped route. A fresh clone opens sessions on the provider's placeholder route and says "add a model" instead of erroring. |
| **`AI_API_KEY` is the only credential** | One key, one place it is read (the provider plugin). dsh's DeepSeek-branded providers expect `DEEPSEEK_API_KEY` and are mounted **off**. |
| **Light theme only, Qoder-style shell** | The UI is a deliberate clone of the owner's daily driver: sidebar with the literal Qoder labels, the centre card, the live session panel. |
| **Budget is enforced by the composition, not requested from the model** | A 400k cap, a warn-at 80%, a per-turn step cap, and at most two approved escalations. A runaway run costs a bounded amount and then stops. |
| **Reviewers are `spawn`ed children, not forks** | A fork hands every reviewer the parent's whole context; paying for that three times is exactly the cost the committee exists to avoid. |

## 6. What exists today, and how it is proven

**Application** (`App/native`, ~19k lines): PTY terminal with shell integration
and per-command blocks; file tree; editor with syntax highlighting; git pane with
diff/stage/commit; content search; go-to-file palette; command palette; the
conversation view (user/assistant bubbles, thinking, tool cards, change cards
with diffs, delegation cards, to-do cards, plan cards, message actions:
copy/rate/retry/quote/export); the right-hand session panel (live activity,
subagents, background processes with states and spinners); the model/route
dialog; workflow and observability panes; notes; workspace switching (`⌘O`).

**Composition** (`App/dsh-plugins`): the `AI_API_KEY` provider over
`.harness/models.json`; the spec lock; the operating contract; `wave_plan`;
`committee`; the budget cap with warnings and escalations; the `orchestrate`
skill.

**Proof, all runnable without a model credential:**

```sh
make check          # plugin typecheck + plugin tests + cargo test --workspace
make check-native   # the app's headless self-test (17 stages)
make check-engine   # boots the engine over ACP and opens a session
```

Plus the opt-in end-to-end pair (`--ignored`), the node probe
(`App/probe/acp-smoke.mjs`), and the screenshot path (`--shot`, `--ask`,
`--pane`, `--dialog`) that photographs the real window offscreen.

Current counts: **66 `harness-core` tests + 4 `harness-app` tests + 1 terminal
smoke**, three plugin test suites, and two ignored end-to-end tests.

## 7. What is open

The work queue lives in `takeover.md` §6. The headline items:

- **Sub-agent orchestration end to end** — the engine links
  `@deepseek-ai/dsh-subagent` and the preset seats committee reviewers as
  children; the wave model is implemented and tested. What is *not* yet proven
  is a real delegated sub-task running to completion through the app.
- **Token / time / context efficiency features** — see the queue; this is the
  owner's first priority and the reason the handover exists.
- **Responsive layout** — the window's panels are fixed-width today
  (sidebar 236, session panel 312). The owner started this and handed it over.
- **No initial commit yet** — the repository has no commits; everything is
  untracked. Do not commit without being asked.
- **Demo route** — the harness ships no model, so a demo needs either a real key
  in `.env` or the mock (`App/probe/mock-openai.mjs`).

## 8. Docs map

| File | Read it for |
|---|---|
| `context.md` | this file — the picture and the decisions |
| `takeover.md` | the rules, the workflow, and the work queue |
| `deepseek_reference.md` | the engine: its repo, its plugin model, the packages we link, the traps |
| `warp_reference.md` | the design reference: its repo, and which of its crates map to which of our panes |
| `App/Architecture.md` | layout, the UI↔core boundary, control flow, launch paths, what was verified how |
| `App/PRD.md` | goals, requirements, risks |
| `README.md` | the user-facing tour (run, add a model, verify, non-determinism) |
